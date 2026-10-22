//! Unix store for the managed custom config: every operation is fd-relative under a
//! stable, owned directory, so a swapped symlink or hard link is never followed.

use super::*;
use crate::secure_directory::{effective_user_id, open_or_create_owned_leaf_directory};
use std::ffi::{CStr, CString};
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::raw::{c_char, c_int, c_uint};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

const O_RDONLY: c_int = 0;
const O_WRONLY: c_int = 1;
#[cfg(target_os = "macos")]
const O_NOFOLLOW: c_int = 0x0000_0100;
#[cfg(target_os = "macos")]
const O_NONBLOCK: c_int = 0x0000_0004;
#[cfg(target_os = "macos")]
const O_CREAT: c_int = 0x0000_0200;
#[cfg(target_os = "macos")]
const O_EXCL: c_int = 0x0000_0800;
#[cfg(target_os = "macos")]
const O_CLOEXEC: c_int = 0x0100_0000;
#[cfg(target_os = "linux")]
const O_NOFOLLOW: c_int = 0x0002_0000;
#[cfg(target_os = "linux")]
const O_NONBLOCK: c_int = 0x0000_0800;
#[cfg(target_os = "linux")]
const O_CREAT: c_int = 0x0000_0040;
#[cfg(target_os = "linux")]
const O_EXCL: c_int = 0x0000_0080;
#[cfg(target_os = "linux")]
const O_CLOEXEC: c_int = 0x0008_0000;

unsafe extern "C" {
    fn openat(directory_descriptor: c_int, path: *const c_char, flags: c_int, ...) -> c_int;
    fn renameat(
        old_directory_descriptor: c_int,
        old_path: *const c_char,
        new_directory_descriptor: c_int,
        new_path: *const c_char,
    ) -> c_int;
    fn unlinkat(directory_descriptor: c_int, path: *const c_char, flags: c_int) -> c_int;
}

pub(crate) struct CustomConfigStore {
    directory: StableDirectory,
}

impl CustomConfigStore {
    pub(crate) fn open_or_create(path: &Path) -> io::Result<Self> {
        Ok(Self {
            directory: StableDirectory::open_or_create(path)?,
        })
    }

    pub(crate) fn read(&self) -> io::Result<Option<Vec<u8>>> {
        self.directory.read_optional(CONFIG_FILE_NAME)
    }

    pub(crate) fn read_managed(&self) -> io::Result<Option<Vec<u8>>> {
        let contents = self.read()?;
        if contents
            .as_deref()
            .is_some_and(|value| !is_managed_fuzzy_config(value))
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "custom config is not managed by Agent Input Method",
            ));
        }
        Ok(contents)
    }

    pub(crate) fn replace(
        &self,
        contents: &[u8],
        expected_current: Option<&[u8]>,
    ) -> io::Result<()> {
        let candidates = (0..TEMPORARY_ATTEMPTS).map(|_| {
            format!(
                ".agent-pinyin-custom-{}-{}.tmp",
                std::process::id(),
                NEXT_TEMPORARY.fetch_add(1, Ordering::Relaxed)
            )
        });
        self.replace_with_candidates(contents, expected_current, candidates)
    }

    pub(crate) fn remove(&self, expected_current: &[u8]) -> io::Result<()> {
        self.directory.verify_path_identity()?;
        let mut current = self
            .directory
            .open_optional(CONFIG_FILE_NAME)?
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "custom config is missing"))?;
        let actual = self
            .directory
            .read_validated(CONFIG_FILE_NAME, &mut current)?;
        if actual != expected_current {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "custom config ownership changed",
            ));
        }
        self.directory
            .verify_child_identity(CONFIG_FILE_NAME, &current)?;
        self.directory.verify_path_identity()?;
        self.directory.unlink(CONFIG_FILE_NAME)?;
        self.directory.file.sync_all()?;
        if self.directory.open_optional(CONFIG_FILE_NAME)?.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "custom config reappeared after removal",
            ));
        }
        self.directory.verify_path_identity()
    }

    fn replace_with_candidates<I, S>(
        &self,
        contents: &[u8],
        expected_current: Option<&[u8]>,
        candidates: I,
    ) -> io::Result<()>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        if contents.len() > MAXIMUM_CONFIG_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "custom config exceeds its bound",
            ));
        }
        self.directory.verify_path_identity()?;
        for candidate in candidates {
            let name = candidate.as_ref();
            match self.directory.create_exclusive_regular(name) {
                Ok(mut temporary) => {
                    let result =
                        self.finish_replace(name, &mut temporary, contents, expected_current);
                    if result.is_err() {
                        let _ = self.directory.unlink_if_identity(name, &temporary);
                    }
                    return result;
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "no exclusive custom config temporary name was available",
        ))
    }

    fn finish_replace(
        &self,
        temporary_name: &str,
        temporary: &mut File,
        contents: &[u8],
        expected_current: Option<&[u8]>,
    ) -> io::Result<()> {
        temporary.write_all(contents)?;
        temporary.sync_all()?;
        StableDirectory::validate_single_link_regular(temporary)?;
        self.directory
            .verify_child_identity(temporary_name, temporary)?;
        self.directory
            .verify_expected(CONFIG_FILE_NAME, expected_current)?;
        self.directory.verify_path_identity()?;
        self.directory.rename(temporary_name, CONFIG_FILE_NAME)?;
        self.directory.file.sync_all()?;
        self.directory
            .verify_child_identity(CONFIG_FILE_NAME, temporary)?;
        self.directory.verify_path_identity()
    }

    #[cfg(test)]
    fn replace_with_test_temporary(
        &self,
        contents: &[u8],
        expected_current: Option<&[u8]>,
        temporary_name: &str,
    ) -> io::Result<()> {
        self.replace_with_candidates(contents, expected_current, [temporary_name])
    }
}

struct StableDirectory {
    file: File,
    path: PathBuf,
}

impl StableDirectory {
    fn open_or_create(path: &Path) -> io::Result<Self> {
        let directory = Self {
            file: open_or_create_owned_leaf_directory(path, 0o700)?,
            path: path.to_path_buf(),
        };
        directory.verify_path_identity()?;
        Ok(directory)
    }

    fn open_optional(&self, name: &str) -> io::Result<Option<File>> {
        let name = leaf_name(name)?;
        match self.open_at(&name, O_RDONLY | O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC, 0) {
            Ok(file) => {
                Self::validate_single_link_regular(&file)?;
                Ok(Some(file))
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn read_optional(&self, name: &str) -> io::Result<Option<Vec<u8>>> {
        self.verify_path_identity()?;
        let Some(mut file) = self.open_optional(name)? else {
            self.verify_path_identity()?;
            return Ok(None);
        };
        let contents = self.read_validated(name, &mut file)?;
        self.verify_path_identity()?;
        Ok(Some(contents))
    }

    fn read_validated(&self, name: &str, file: &mut File) -> io::Result<Vec<u8>> {
        Self::validate_single_link_regular(file)?;
        let metadata = file.metadata()?;
        if metadata.len() > MAXIMUM_CONFIG_BYTES as u64 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "custom config exceeds its bound",
            ));
        }
        file.seek(SeekFrom::Start(0))?;
        let mut contents = Vec::with_capacity(metadata.len() as usize);
        Read::by_ref(file)
            .take((MAXIMUM_CONFIG_BYTES + 1) as u64)
            .read_to_end(&mut contents)?;
        if contents.len() > MAXIMUM_CONFIG_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "custom config grew beyond its bound",
            ));
        }
        Self::validate_single_link_regular(file)?;
        self.verify_child_identity(name, file)?;
        Ok(contents)
    }

    fn verify_expected(&self, name: &str, expected: Option<&[u8]>) -> io::Result<()> {
        match (self.open_optional(name)?, expected) {
            (None, None) => Ok(()),
            (Some(mut file), Some(expected)) => {
                let actual = self.read_validated(name, &mut file)?;
                if actual == expected {
                    Ok(())
                } else {
                    Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "custom config ownership changed",
                    ))
                }
            }
            _ => Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "custom config existence changed",
            )),
        }
    }

    fn create_exclusive_regular(&self, name: &str) -> io::Result<File> {
        let name = leaf_name(name)?;
        let file = self.open_at(
            &name,
            O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC,
            0o600,
        )?;
        Self::validate_single_link_regular(&file)?;
        self.verify_path_identity()?;
        Ok(file)
    }

    fn verify_child_identity(&self, name: &str, expected: &File) -> io::Result<()> {
        let current = self.open_optional(name)?.ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "custom config entry disappeared")
        })?;
        if !same_identity(&current.metadata()?, &expected.metadata()?) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "custom config entry changed identity",
            ));
        }
        Ok(())
    }

    fn validate_single_link_regular(file: &File) -> io::Result<()> {
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.nlink() != 1 || metadata.uid() != effective_user_id() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "custom config entry is not a single-link regular file",
            ));
        }
        Ok(())
    }

    fn open_at(&self, name: &CStr, flags: c_int, mode: c_uint) -> io::Result<File> {
        let descriptor = unsafe { openat(self.file.as_raw_fd(), name.as_ptr(), flags, mode) };
        if descriptor < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(unsafe { File::from_raw_fd(descriptor) })
        }
    }

    fn rename(&self, from: &str, to: &str) -> io::Result<()> {
        let from = leaf_name(from)?;
        let to = leaf_name(to)?;
        if unsafe {
            renameat(
                self.file.as_raw_fd(),
                from.as_ptr(),
                self.file.as_raw_fd(),
                to.as_ptr(),
            )
        } == 0
        {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    fn unlink(&self, name: &str) -> io::Result<()> {
        let name = leaf_name(name)?;
        if unsafe { unlinkat(self.file.as_raw_fd(), name.as_ptr(), 0) } == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    fn unlink_if_identity(&self, name: &str, expected: &File) -> io::Result<()> {
        let current = self.open_optional(name)?.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "temporary custom config disappeared",
            )
        })?;
        if !same_identity(&current.metadata()?, &expected.metadata()?) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "temporary custom config changed identity",
            ));
        }
        self.unlink(name)?;
        self.file.sync_all()
    }

    fn verify_path_identity(&self) -> io::Result<()> {
        let current = fs::symlink_metadata(&self.path)?;
        if current.file_type().is_symlink()
            || !current.is_dir()
            || current.uid() != effective_user_id()
            || !same_identity(&current, &self.file.metadata()?)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Rime user directory path changed identity",
            ));
        }
        Ok(())
    }
}

fn leaf_name(name: &str) -> io::Result<CString> {
    if name.is_empty() || name == "." || name == ".." || name.contains('/') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "custom config leaf is invalid",
        ));
    }
    CString::new(name).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "custom config leaf contains NUL",
        )
    })
}

fn same_identity(lhs: &fs::Metadata, rhs: &fs::Metadata) -> bool {
    lhs.dev() == rhs.dev() && lhs.ino() == rhs.ino()
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::sync::atomic::{AtomicU64, Ordering};

    fn temporary_root() -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let root = PathBuf::from("/private/tmp").join(format!(
            "agent-ime-custom-config-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        root
    }

    #[test]
    fn normal_write_remove_and_rollback_preserve_expected_contents() {
        let root = temporary_root();
        let user = root.join("Rime");
        let store = CustomConfigStore::open_or_create(&user).unwrap();
        let first = fuzzy_config(1);
        let second = fuzzy_config(2);

        assert_eq!(store.read().unwrap(), None);
        store.replace(first.as_bytes(), None).unwrap();
        assert_eq!(store.read().unwrap().as_deref(), Some(first.as_bytes()));
        assert_eq!(
            fs::metadata(user.join(CONFIG_FILE_NAME))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );

        store
            .replace(second.as_bytes(), Some(first.as_bytes()))
            .unwrap();
        assert_eq!(store.read().unwrap().as_deref(), Some(second.as_bytes()));
        store
            .replace(first.as_bytes(), Some(second.as_bytes()))
            .unwrap();
        assert_eq!(store.read().unwrap().as_deref(), Some(first.as_bytes()));

        assert!(store.remove(b"foreign").is_err());
        assert_eq!(store.read().unwrap().as_deref(), Some(first.as_bytes()));
        store.remove(first.as_bytes()).unwrap();
        assert_eq!(store.read().unwrap(), None);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn destination_symlink_and_hardlink_do_not_touch_external_sentinels() {
        for hard_link in [false, true] {
            let root = temporary_root();
            let user = root.join("Rime");
            fs::create_dir(&user).unwrap();
            let external = root.join(if hard_link {
                "hardlink-sentinel"
            } else {
                "symlink-sentinel"
            });
            fs::write(&external, b"do-not-touch").unwrap();
            fs::set_permissions(&external, fs::Permissions::from_mode(0o640)).unwrap();
            let destination = user.join(CONFIG_FILE_NAME);
            if hard_link {
                fs::hard_link(&external, &destination).unwrap();
            } else {
                symlink(&external, &destination).unwrap();
            }
            let store = CustomConfigStore::open_or_create(&user).unwrap();

            assert!(store.read().is_err());
            assert!(store.replace(fuzzy_config(1).as_bytes(), None).is_err());
            assert!(store.remove(b"do-not-touch").is_err());
            assert_eq!(fs::read(&external).unwrap(), b"do-not-touch");
            assert_eq!(
                fs::metadata(&external).unwrap().permissions().mode() & 0o777,
                0o640
            );
            assert!(fs::symlink_metadata(&destination).is_ok());
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn foreign_regular_destination_is_not_claimed_as_managed_or_removed() {
        let root = temporary_root();
        let user = root.join("Rime");
        fs::create_dir(&user).unwrap();
        let destination = user.join(CONFIG_FILE_NAME);
        fs::write(&destination, b"foreign-custom-config").unwrap();
        let store = CustomConfigStore::open_or_create(&user).unwrap();

        assert!(store.read_managed().is_err());
        assert!(store.remove(fuzzy_config(1).as_bytes()).is_err());
        assert_eq!(fs::read(&destination).unwrap(), b"foreign-custom-config");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn temporary_symlink_and_hardlink_are_never_followed_or_removed() {
        for hard_link in [false, true] {
            let root = temporary_root();
            let user = root.join("Rime");
            fs::create_dir(&user).unwrap();
            let external = root.join(if hard_link {
                "hardlink-temp-sentinel"
            } else {
                "symlink-temp-sentinel"
            });
            fs::write(&external, b"do-not-touch-temp").unwrap();
            fs::set_permissions(&external, fs::Permissions::from_mode(0o640)).unwrap();
            let temporary_name = ".fixed-security-test.tmp";
            let temporary = user.join(temporary_name);
            if hard_link {
                fs::hard_link(&external, &temporary).unwrap();
            } else {
                symlink(&external, &temporary).unwrap();
            }
            let store = CustomConfigStore::open_or_create(&user).unwrap();

            assert!(
                store
                    .replace_with_test_temporary(fuzzy_config(1).as_bytes(), None, temporary_name,)
                    .is_err()
            );
            assert_eq!(fs::read(&external).unwrap(), b"do-not-touch-temp");
            assert_eq!(
                fs::metadata(&external).unwrap().permissions().mode() & 0o777,
                0o640
            );
            assert!(fs::symlink_metadata(&temporary).is_ok());
            assert!(!user.join(CONFIG_FILE_NAME).exists());
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn stable_directory_descriptor_never_writes_through_replacement_symlink() {
        let root = temporary_root();
        let user = root.join("Rime");
        fs::create_dir(&user).unwrap();
        let store = CustomConfigStore::open_or_create(&user).unwrap();
        let moved = root.join("moved-Rime");
        fs::rename(&user, &moved).unwrap();
        let external = root.join("external-Rime");
        fs::create_dir(&external).unwrap();
        let sentinel = external.join("sentinel");
        fs::write(&sentinel, b"external-do-not-touch").unwrap();
        symlink(&external, &user).unwrap();

        assert!(store.replace(fuzzy_config(1).as_bytes(), None).is_err());
        assert!(store.read().is_err());
        assert_eq!(fs::read(&sentinel).unwrap(), b"external-do-not-touch");
        assert!(!external.join(CONFIG_FILE_NAME).exists());
        assert!(!moved.join(CONFIG_FILE_NAME).exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn symlinked_user_directory_is_rejected_without_external_write() {
        let root = temporary_root();
        let user = root.join("Rime");
        let external = root.join("external-user-directory");
        fs::create_dir(&external).unwrap();
        let sentinel = external.join("sentinel");
        fs::write(&sentinel, b"external-do-not-touch").unwrap();
        symlink(&external, &user).unwrap();

        assert!(CustomConfigStore::open_or_create(&user).is_err());
        assert_eq!(fs::read(&sentinel).unwrap(), b"external-do-not-touch");
        assert!(!external.join(CONFIG_FILE_NAME).exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn symlinked_ancestor_is_rejected_without_external_write() {
        let root = temporary_root();
        let moved = root.with_extension("moved");
        fs::rename(&root, &moved).unwrap();
        let external = root.with_extension("external");
        fs::create_dir(&external).unwrap();
        let sentinel = external.join("sentinel");
        fs::write(&sentinel, b"external-do-not-touch").unwrap();
        symlink(&external, &root).unwrap();

        assert!(CustomConfigStore::open_or_create(&root.join("Rime")).is_err());
        assert_eq!(fs::read(&sentinel).unwrap(), b"external-do-not-touch");
        assert!(!external.join("Rime").exists());

        fs::remove_file(&root).unwrap();
        fs::remove_dir_all(moved).unwrap();
        fs::remove_dir_all(external).unwrap();
    }

    #[test]
    fn only_exact_generated_fuzzy_configs_are_managed() {
        for mask in [1, 2, 0x80, 0xff] {
            assert!(is_managed_fuzzy_config(fuzzy_config(mask).as_bytes()));
            assert!(is_managed_fuzzy_config(
                legacy_fuzzy_config_v2(mask).as_bytes()
            ));
            assert!(is_managed_fuzzy_config(
                legacy_fuzzy_config_v1(mask).as_bytes()
            ));
        }
        assert!(!is_managed_fuzzy_config(b"foreign"));
        assert!(!is_managed_fuzzy_config(fuzzy_config(0).as_bytes()));
        assert!(!is_managed_fuzzy_config(fuzzy_config(0x100).as_bytes()));
        let mut changed = fuzzy_config(1);
        changed.push_str("# foreign change\n");
        assert!(!is_managed_fuzzy_config(changed.as_bytes()));
        let mut changed_legacy = legacy_fuzzy_config_v1(1);
        changed_legacy.push_str("# foreign change\n");
        assert!(!is_managed_fuzzy_config(changed_legacy.as_bytes()));
    }

    #[test]
    fn fuzzy_config_preserves_complete_base_algebra_and_real_derivations() {
        let config = fuzzy_config((1 << 0) | (1 << 5));
        for erase in ["^m$", "^n$", "^ng$", "^hm$", "^hng$"] {
            assert!(config.contains(&format!("    - erase/{erase}/")));
        }
        assert!(config.contains("    - xform/^zh/z/"));
        assert!(config.contains("    - derive/^z/zh/"));
        assert!(config.contains("    - xform/ang$/an/"));
        assert!(config.contains("    - derive/an$/ang/"));
        assert!(!config.contains("    - abbrev/^z/zh/"));
    }
}
