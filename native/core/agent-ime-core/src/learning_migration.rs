//! Shared migration pipeline. Rime only opens a private database copy.
use crate::zhixu::learning::LearningStore;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use std::sync::atomic::{AtomicU64, Ordering};

const MAX_DATABASE_FILES: usize = 4096;
const MAX_DATABASE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_EXPORT_BYTES: u64 = 16 * 1024 * 1024;

pub fn private_directory(path: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)
}

fn private_file(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

pub fn reject_live_path(path: &Path) -> io::Result<()> {
    let forbidden = |path: &Path| {
        let normalized = path.to_string_lossy().replace('\\', "/").to_ascii_lowercase();
        normalized.contains("/application support/agentinputmethod")
            || normalized.contains("/appdata/roaming/agentinputmethod")
    };
    let error = || io::Error::new(io::ErrorKind::PermissionDenied,
        "use a separate copy and output directory, never the active input method directory");
    let absolute = if path.is_absolute() { path.to_path_buf() } else { std::env::current_dir()?.join(path) };
    if forbidden(&absolute) { return Err(error()); }
    let resolved = if absolute.exists() {
        fs::canonicalize(&absolute)?
    } else if let (Some(parent), Some(name)) = (absolute.parent(), absolute.file_name()) {
        // The output leaf may not exist yet, but an existing parent can be an alias.
        fs::canonicalize(parent).map(|parent| parent.join(name)).unwrap_or(absolute)
    } else {
        absolute
    };
    if forbidden(&resolved) { return Err(error()); }
    Ok(())
}

pub fn copy_user_database(source: &Path, destination: &Path) -> io::Result<()> {
    copy_user_database_before_check(source, destination, || {})
}

fn copy_user_database_before_check(source: &Path, destination: &Path, before_check: impl FnOnce()) -> io::Result<()> {
    let source = source.join("pinyin_simp.userdb");
    let destination = destination.join("pinyin_simp.userdb");
    fn copy(source: &Path, destination: &Path, depth: usize, files: &mut usize, bytes: &mut u64) -> io::Result<()> {
        let metadata = fs::symlink_metadata(source)?;
        if metadata.file_type().is_symlink() || depth > 8 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "database copies may not contain symlinks or deep directories"));
        }
        if metadata.is_dir() {
            private_directory(destination)?;
            for entry in fs::read_dir(source)? {
                let entry = entry?;
                copy(&entry.path(), &destination.join(entry.file_name()), depth + 1, files, bytes)?;
            }
        } else if metadata.is_file() {
            *files += 1;
            *bytes = bytes.saturating_add(metadata.len());
            if *files > MAX_DATABASE_FILES || *bytes > MAX_DATABASE_BYTES {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "database copy exceeds the migration size limit"));
            }
            let mut input = File::open(source)?.take(metadata.len() + 1);
            let mut output = private_file(destination)?;
            let copied = io::copy(&mut input, &mut output)?;
            if copied != metadata.len() {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "database copy changed during staging"));
            }
            output.flush()?;
        } else {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "database copy contains a non-regular file"));
        }
        Ok(())
    }
    if !fs::symlink_metadata(&source)?.is_dir() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "pinyin_simp.userdb must be a copied directory"));
    }
    let before = database_manifest(&source)?;
    copy(&source, &destination, 0, &mut 0, &mut 0)?;
    before_check();
    if before != database_manifest(&source)? {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "database changed during staging; retry when idle"));
    }
    Ok(())
}

fn database_manifest(root: &Path) -> io::Result<std::collections::BTreeMap<PathBuf, (bool, u64, SystemTime)>> {
    fn visit(root: &Path, depth: usize, manifest: &mut std::collections::BTreeMap<PathBuf, (bool, u64, SystemTime)>, bytes: &mut u64) -> io::Result<()> {
        let metadata = fs::symlink_metadata(root)?;
        if metadata.file_type().is_symlink() || (!metadata.is_dir() && !metadata.is_file()) || depth > 8 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "database contains an unsafe entry"));
        }
        if metadata.is_file() { *bytes = bytes.saturating_add(metadata.len()); }
        if manifest.len() >= MAX_DATABASE_FILES || *bytes > MAX_DATABASE_BYTES {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "database copy exceeds the migration size limit"));
        }
        manifest.insert(root.to_owned(), (metadata.is_dir(), metadata.len(), metadata.modified()?));
        if metadata.is_dir() {
            for entry in fs::read_dir(root)? { visit(&entry?.path(), depth + 1, manifest, bytes)?; }
        }
        Ok(())
    }
    let mut manifest = std::collections::BTreeMap::new();
    visit(root, 0, &mut manifest, &mut 0)?;
    Ok(manifest)
}

pub struct Workspace(pub PathBuf);
impl Workspace {
    pub fn new() -> io::Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
        let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("aim-learning-migration-{}-{now}-{sequence}", std::process::id()));
        private_directory(&path)?;
        Ok(Self(fs::canonicalize(path)?))
    }
}
impl Drop for Workspace {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
}

#[cfg(feature = "librime")]
pub fn export_copy(source: &Path, shared: &Path, workspace: &Workspace) -> io::Result<PathBuf> {
    use std::ffi::{CString, c_char, c_int};
    unsafe extern "C" {
        fn aim_rime_initialize(shared: *const c_char, user: *const c_char) -> c_int;
        fn aim_rime_backup_user_dict(dictionary: *const c_char, output: *const c_char) -> c_int;
    }
    let user = workspace.0.join("user");
    private_directory(&user)?;
    copy_user_database(source, &user)?;
    // librime otherwise writes a second snapshot to ./sync outside this workspace.
    let sync = workspace.0.join("sync");
    private_directory(&sync)?;
    let sync_path = serde_json::to_string(sync.to_str().ok_or_else(||
        io::Error::new(io::ErrorKind::InvalidInput, "path is not UTF-8"))?)
        .map_err(io::Error::other)?;
    let mut installation = private_file(&user.join("installation.yaml"))?;
    writeln!(installation, "sync_dir: {sync_path}")?;
    let output = workspace.0.join("export.txt");
    // The directory is private even if librime creates its text export with a wider file mode.
    let path = |value: &Path| CString::new(value.to_str().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path is not UTF-8"))?)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL"));
    let (shared, user, output_c) = (path(shared)?, path(&user)?, path(&output)?);
    if unsafe { aim_rime_initialize(shared.as_ptr(), user.as_ptr()) } == 0 {
        return Err(io::Error::other("librime initialization failed"));
    }
    // The plain text export drops d/t and the logical clock. A backup retains them.
    if unsafe { aim_rime_backup_user_dict(c"pinyin_simp".as_ptr(), output_c.as_ptr()) } == 0 {
        return Err(io::Error::other("librime export failed"));
    }
    Ok(output)
}

#[cfg(not(feature = "librime"))]
pub fn export_copy(_source: &Path, _shared: &Path, _workspace: &Workspace) -> io::Result<PathBuf> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "exporting a database copy requires the librime feature"))
}

pub fn read_export(path: &Path) -> io::Result<String> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.len() > MAX_EXPORT_BYTES {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "export must be a regular text file of at most 16 MiB"));
    }
    let mut text = String::new();
    File::open(path)?.take(MAX_EXPORT_BYTES + 1).read_to_string(&mut text)?;
    if text.len() as u64 > MAX_EXPORT_BYTES {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "export exceeds 16 MiB"));
    }
    validate_export_text(&text)?;
    Ok(text)
}

fn validate_export_text(text: &str) -> io::Result<()> {
    if text.len() as u64 > MAX_EXPORT_BYTES {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "export exceeds 16 MiB"));
    }
    let invalid = || io::Error::new(io::ErrorKind::InvalidData,
        "migration requires a complete Rime backup with its logical clock and c/d/t fields; plain exports lose recency");
    let tick = text.lines().find_map(|line| line.strip_prefix("#@/tick\t"))
        .and_then(|value| value.trim().parse::<u64>().ok()).filter(|tick| *tick < u64::MAX)
        .ok_or_else(invalid)?;
    for line in text.lines().filter(|line| !line.is_empty() && !line.starts_with('#')) {
        let columns: Vec<_> = line.split('\t').collect();
        if columns.len() != 3 { return Err(invalid()); }
        let mut commits = None;
        let mut dynamic = None;
        let mut last_tick = None;
        for part in columns[2].split_whitespace() {
            if let Some(value) = part.strip_prefix("c=") { commits = value.parse::<i32>().ok(); }
            if let Some(value) = part.strip_prefix("d=") { dynamic = value.parse::<f64>().ok(); }
            if let Some(value) = part.strip_prefix("t=") { last_tick = value.parse::<u64>().ok(); }
        }
        if commits.is_none()
            || !dynamic.is_some_and(|value| value.is_finite() && value >= 0.0)
            || !last_tick.is_some_and(|value| value <= tick)
        {
            return Err(invalid());
        }
    }
    Ok(())
}

/// Import a complete backup into a new library without merging existing learning.
pub fn import_backup(text: &str, output: &Path, allow_empty: bool) -> io::Result<usize> {
    validate_export_text(text)?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64;
    let mut store = LearningStore::default();
    let imported = store.import_rime_export(text, now)?;
    if imported == 0 && !allow_empty {
        return Err(io::Error::new(io::ErrorKind::InvalidData,
            "export contains no supported learned entries; output was not changed"));
    }
    store.migrated = true;
    store.rime_tick = text.lines().find_map(|line| line.strip_prefix("#@/tick\t"))
        .and_then(|value| value.trim().parse().ok()).unwrap_or(0);
    store.save_new(output)?;
    Ok(imported)
}

pub fn learning_available(user: &Path) -> io::Result<bool> {
    LearningStore::existing_file_valid(user)
}

/// Called only by a dedicated worker before initializing any Rime sessions.
/// The source database is read only; all Rime calls use the private copy.
pub fn migrate_if_needed(shared: &Path, user: &Path) -> io::Result<()> {
    if learning_available(user)? { return Ok(()); }
    if !fs::symlink_metadata(user)?.is_dir() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "user directory must be a real directory"));
    }
    let text = match fs::symlink_metadata(user.join("pinyin_simp.userdb")) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => "#@/tick\t0\n".to_owned(),
        Err(error) => return Err(error),
        Ok(_) => {
            let workspace = Workspace::new()?;
            let exported = export_copy(user, shared, &workspace)?;
            read_export(&exported)?
        }
    };
    match import_backup(&text, user, true) {
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            if learning_available(user)? { Ok(()) } else { Err(error) }
        }
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zhixu::learning::FILE_NAME;

    #[test]
    fn first_import_keeps_rime_metadata_and_second_import_cannot_overwrite() {
        let workspace = Workspace::new().unwrap();
        let output = workspace.0.join("learned");
        let backup = "#@/tick\t10\nzai gao \t再搞\tc=7 d=4 t=10\n";
        assert_eq!(import_backup(backup, &output, false).unwrap(), 1);
        let before = fs::read(output.join(FILE_NAME)).unwrap();
        let json: serde_json::Value = serde_json::from_slice(&before).unwrap();
        assert_eq!(json["rimeTick"], 10);
        assert!(import_backup(backup, &output, false).is_err());
        assert_eq!(fs::read(output.join(FILE_NAME)).unwrap(), before);
        assert!(migrate_if_needed(&workspace.0.join("unused-shared"), &output).is_ok());
        assert_eq!(fs::read(output.join(FILE_NAME)).unwrap(), before);
    }

    #[test]
    fn fresh_user_without_a_rime_database_gets_an_empty_migrated_library() {
        let workspace = Workspace::new().unwrap();
        let user = workspace.0.join("user");
        private_directory(&user).unwrap();
        migrate_if_needed(&workspace.0.join("unused-shared"), &user).unwrap();
        let bytes = fs::read(user.join(FILE_NAME)).unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["migrated"], true);
        assert_eq!(json["entries"].as_array().unwrap().len(), 0);
        migrate_if_needed(&workspace.0.join("unused-shared"), &user).unwrap();
        assert_eq!(fs::read(user.join(FILE_NAME)).unwrap(), bytes);
    }

    #[test]
    fn damaged_existing_library_fails_without_overwriting() {
        let workspace = Workspace::new().unwrap();
        let user = workspace.0.join("user");
        private_directory(&user).unwrap();
        fs::write(user.join(FILE_NAME), b"damaged learning").unwrap();
        assert!(migrate_if_needed(&workspace.0, &user).is_err());
        assert_eq!(fs::read(user.join(FILE_NAME)).unwrap(), b"damaged learning");
    }

    #[test]
    fn empty_backup_is_allowed_only_for_the_runtime_migration() {
        let workspace = Workspace::new().unwrap();
        let output = workspace.0.join("learned");
        assert!(import_backup("#@/tick\t0\n", &output, false).is_err());
        assert!(!output.exists());
        assert_eq!(import_backup("#@/tick\t0\n", &output, true).unwrap(), 0);
        assert!(output.join(FILE_NAME).is_file());
    }

    #[test]
    fn incomplete_backup_does_not_create_a_library() {
        let workspace = Workspace::new().unwrap();
        let output = workspace.0.join("learned");
        for text in ["再搞\tzai gao\t7\n", "#@/tick\t2\nzai gao\t再搞\tc=7 d=2 t=3\n"] {
            assert!(import_backup(text, &output, true).is_err());
            assert!(!output.exists());
        }
    }
    #[test]
    fn database_changes_during_copy_are_rejected() {
        let workspace = Workspace::new().unwrap();
        let source = workspace.0.join("source");
        let target = workspace.0.join("target");
        private_directory(&source).unwrap();
        private_directory(&target).unwrap();
        private_directory(&source.join("pinyin_simp.userdb")).unwrap();
        let log = source.join("pinyin_simp.userdb/000001.log");
        fs::write(&log, b"stable before copy").unwrap();
        assert!(copy_user_database_before_check(&source, &target, || {
            fs::write(&log, b"appended while copying").unwrap();
        }).is_err());
        assert_eq!(fs::read(&log).unwrap(), b"appended while copying");
    }

    #[test]
    fn first_publish_refuses_to_merge_a_library_created_since_staging() {
        let workspace = Workspace::new().unwrap();
        let output = workspace.0.join("learned");
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64;
        let mut staged = LearningStore::default();
        staged.import_rime_export("#@/tick\t10\nzai gao \t再搞\tc=7 d=4 t=10\n", now).unwrap();
        let mut other = LearningStore::default();
        other.record("cai wo", "裁我", now);
        other.save(&output).unwrap();
        let before = fs::read(output.join(FILE_NAME)).unwrap();
        assert_eq!(staged.save_new(&output).unwrap_err().kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(output.join(FILE_NAME)).unwrap(), before);
        assert_eq!(LearningStore::load(&output, now).entries().len(), 1);
    }

    #[test]
    fn readiness_ffi_distinguishes_missing_valid_and_damaged_files() {
        let workspace = Workspace::new().unwrap();
        let user = workspace.0.join("user");
        private_directory(&user).unwrap();
        let path = user.to_str().unwrap().as_bytes();
        let available = || unsafe { crate::aim_learning_available(path.as_ptr(), path.len()) };
        assert_eq!(available(), 0);
        assert_eq!(unsafe { crate::aim_learning_available(std::ptr::null(), 0) }, -2);
        import_backup("#@/tick\t10\nzai gao \t再搞\tc=7 d=4 t=10\n", &user, true).unwrap();
        assert_eq!(available(), 1);
        let mut json: serde_json::Value = serde_json::from_slice(&fs::read(user.join(FILE_NAME)).unwrap()).unwrap();
        json["entries"][0]["count"] = serde_json::json!(0);
        fs::write(user.join(FILE_NAME), serde_json::to_vec(&json).unwrap()).unwrap();
        assert_eq!(available(), -3, "invalid records must not silently become an empty learning library");
        fs::write(user.join(FILE_NAME), b"broken").unwrap();
        assert_eq!(available(), -3);
    }

    #[cfg(unix)]
    #[test]
    fn existing_symlinks_are_not_followed_or_replaced() {
        let workspace = Workspace::new().unwrap();
        let user = workspace.0.join("user");
        private_directory(&user).unwrap();
        let missing = workspace.0.join("missing");
        std::os::unix::fs::symlink(&missing, user.join(FILE_NAME)).unwrap();
        assert!(learning_available(&user).is_err());
        assert!(migrate_if_needed(&workspace.0, &user).is_err());
        assert_eq!(fs::read_link(user.join(FILE_NAME)).unwrap(), missing);
        assert!(!missing.exists());
    }

}
