//! Migrate a supplied Rime user-directory copy, or an already exported text file.
//! cargo run --offline --example migrate_learning -- --copy-user-dir COPY --shared-dir SHARED --output-dir NEW
//! cargo run --offline --example migrate_learning -- --export-text EXPORT.txt --output-dir NEW
//! The first form exports a second, private copy; Rime never opens the supplied directory.
use agent_ime_core::learning_migration::{Workspace, export_copy, import_backup, read_export, reject_live_path};
#[cfg(test)]
use agent_ime_core::learning_migration::{copy_user_database, private_directory};
#[cfg(test)]
use agent_ime_core::zhixu::learning::LearningStore;
use std::fs;
use std::io;
use std::path::PathBuf;
#[cfg(test)]
use std::path::Path;
#[cfg(test)]
use std::time::{SystemTime, UNIX_EPOCH};

fn run(arguments: &[String]) -> io::Result<usize> {
    let invalid = || io::Error::new(io::ErrorKind::InvalidInput,
        "usage: --copy-user-dir COPY --shared-dir SHARED --output-dir NEW | --export-text EXPORT.txt --output-dir NEW");
    let (mut source, mut shared, mut text, mut output) = (None, None, None, None);
    if arguments.len() % 2 != 0 { return Err(invalid()); }
    for pair in arguments.chunks_exact(2) {
        let slot = match pair[0].as_str() {
            "--copy-user-dir" => &mut source,
            "--shared-dir" => &mut shared,
            "--export-text" => &mut text,
            "--output-dir" => &mut output,
            _ => return Err(invalid()),
        };
        if slot.replace(PathBuf::from(&pair[1])).is_some() { return Err(invalid()); }
    }
    let output = output.ok_or_else(invalid)?;
    reject_live_path(&output)?;
    match fs::symlink_metadata(output.join(agent_ime_core::zhixu::learning::FILE_NAME)) {
        Ok(_) => return Err(io::Error::new(io::ErrorKind::AlreadyExists,
            "migration requires a new learning file; choose a fresh output directory")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let workspace = Workspace::new()?;
    let exported = match (source, shared, text) {
        (Some(source), Some(shared), None) => {
            reject_live_path(&source)?;
            reject_live_path(&shared)?;
            export_copy(&source, &shared, &workspace)?
        }
        (None, None, Some(text)) => { reject_live_path(&text)?; text },
        _ => return Err(invalid()),
    };
    let text = read_export(&exported)?;
    import_backup(&text, &output, false)
}

fn main() {
    // Pull in the host's native shim archive, also when this example only uses core APIs.
    let _ = zhizi_host::RANKING_BUDGET;
    match run(&std::env::args().skip(1).collect::<Vec<_>>()) {
        Ok(count) => println!("Imported {count} entries into agent_zhixu_learning_v1.json"),
        Err(error) => { eprintln!("Learning migration failed: {error}"); std::process::exit(1); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn scratch(label: &str) -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!("aim-migrate-learning-{label}-{}-{}",
            std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir(&root).unwrap();
        root
    }

    #[test]
    fn staging_copies_only_the_database_and_preserves_the_source() {
        let root = scratch("copy");
        let source = root.join("copy");
        let destination = root.join("staging");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&destination).unwrap();
        fs::create_dir(source.join("pinyin_simp.userdb")).unwrap();
        let file = source.join("pinyin_simp.userdb/000001.ldb");
        fs::write(&file, b"synthetic database bytes").unwrap();
        fs::write(source.join("unrelated-private-file"), b"do not copy").unwrap();
        copy_user_database(&source, &destination).unwrap();
        assert_eq!(fs::read(destination.join("pinyin_simp.userdb/000001.ldb")).unwrap(), b"synthetic database bytes");
        assert_eq!(fs::read(&file).unwrap(), b"synthetic database bytes");
        assert!(!destination.join("unrelated-private-file").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_database_is_an_error() {
        let root = scratch("missing");
        assert!(copy_user_database(&root, &root.join("out")).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn exported_text_imports_into_a_private_learning_file() {
        let root = scratch("text-import");
        let source = root.join("export.txt");
        let output = root.join("learned");
        fs::write(&source, "#@/tick\t10\nzai gao \t再搞\tc=7 d=4 t=10\ncai wo \t裁我\tc=3 d=2 t=9\n").unwrap();
        let arguments = vec!["--export-text".into(), source.to_str().unwrap().into(),
            "--output-dir".into(), output.to_str().unwrap().into()];
        assert_eq!(run(&arguments).unwrap(), 2);
        let store = LearningStore::load(&output, SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64);
        assert_eq!(store.entries().iter().find(|entry| entry.text == "再搞").unwrap().count, 7);
        let saved = fs::read(output.join(agent_ime_core::zhixu::learning::FILE_NAME)).unwrap();
        fs::write(&source, "no valid entries").unwrap();
        assert!(run(&arguments).is_err());
        assert_eq!(fs::read(output.join(agent_ime_core::zhixu::learning::FILE_NAME)).unwrap(), saved);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(output.join(agent_ime_core::zhixu::learning::FILE_NAME)).unwrap().permissions().mode() & 0o077, 0);
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn migration_requires_a_new_learning_file() {
        let root = scratch("new-output");
        let source = root.join("export.txt");
        fs::write(&source, "#@/tick\t10\nzai gao \t再搞\tc=7 d=4 t=10\n").unwrap();
        let arguments = |output: &Path| vec!["--export-text".into(), source.to_str().unwrap().into(),
            "--output-dir".into(), output.to_str().unwrap().into()];
        let empty = root.join("empty");
        private_directory(&empty).unwrap();
        assert_eq!(run(&arguments(&empty)).unwrap(), 1, "an existing empty directory is allowed");
        let name = agent_ime_core::zhixu::learning::FILE_NAME;
        let saved = fs::read(empty.join(name)).unwrap();
        assert!(run(&arguments(&empty)).is_err(), "an existing valid library must not be merged");
        assert_eq!(fs::read(empty.join(name)).unwrap(), saved);
        let damaged = root.join("damaged");
        private_directory(&damaged).unwrap();
        fs::write(damaged.join(name), b"not a learning document").unwrap();
        assert!(run(&arguments(&damaged)).is_err(), "a damaged library is still existing data");
        assert_eq!(fs::read(damaged.join(name)).unwrap(), b"not a learning document");
        #[cfg(unix)]
        {
            let linked = root.join("linked");
            private_directory(&linked).unwrap();
            let missing_target = root.join("missing-target");
            std::os::unix::fs::symlink(&missing_target, linked.join(name)).unwrap();
            assert!(run(&arguments(&linked)).is_err(), "a dangling symlink must not be replaced");
            assert_eq!(fs::read_link(linked.join(name)).unwrap(), missing_target);
            assert!(!missing_target.exists());
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn incomplete_exports_are_rejected_without_changing_the_output() {
        let root = scratch("incomplete-export");
        let source = root.join("export.txt");
        let output = root.join("learned");
        let arguments = vec!["--export-text".into(), source.to_str().unwrap().into(),
            "--output-dir".into(), output.to_str().unwrap().into()];
        for text in [
            "再搞\tzai gao\t7\n",
            "zai gao\t再搞\tc=7 d=2 t=1\n",
            "#@/tick\t2\nzai gao\t再搞\tc=7 t=1\n",
            "#@/tick\t2\nzai gao\t再搞\tc=7 d=2\n",
            "#@/tick\t2\nzai gao\t再搞\tc=7 d=NaN t=1\n",
            "#@/tick\t2\nzai gao\t再搞\tc=7 d=-1 t=1\n",
            "#@/tick\t2\nzai gao\t再搞\tc=7 d=2 t=3\n",
        ] {
            fs::write(&source, text).unwrap();
            assert!(run(&arguments).is_err(), "a complete backup is required");
            assert!(!output.exists(), "invalid input must not create a learning directory");
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_or_conflicting_arguments_are_rejected() {
        assert!(run(&[]).is_err());
        assert!(run(&["--output-dir".into()]).is_err());
        assert!(run(&["--output-dir".into(), "unused".into(), "--output-dir".into(), "unused".into()]).is_err());
        assert!(reject_live_path(Path::new("/synthetic/Library/Application Support/AgentInputMethod/Rime")).is_err());
    }

    #[cfg(feature = "librime")]
    #[test]
    fn a_synthetic_database_copy_exports_through_the_real_shim() {
        use std::collections::BTreeMap;
        use std::process::Command;
        fn files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
            let mut result = BTreeMap::new();
            for entry in fs::read_dir(root).unwrap() {
                let entry = entry.unwrap();
                if entry.file_type().unwrap().is_dir() {
                    result.extend(files(&entry.path()));
                } else {
                    result.insert(entry.path(), fs::read(entry.path()).unwrap());
                }
            }
            result
        }
        let root = scratch("real-export");
        let worker = |action: &str| {
            let result = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "tests::rime_migration_worker", "--ignored", "--nocapture"])
                .current_dir(&root)
                .env("AIM_MIGRATION_TEST_ROOT", &root)
                .env("AIM_MIGRATION_TEST_ACTION", action)
                .output().unwrap();
            assert!(result.status.success(), "isolated {action} failed: {}{}",
                String::from_utf8_lossy(&result.stdout), String::from_utf8_lossy(&result.stderr));
        };
        worker("create");
        let before = files(&root.join("copy"));
        assert!(root.join("copy/pinyin_simp.userdb").is_dir());
        worker("migrate");
        assert!(!root.join("sync").exists(), "backups must stay inside the private migration workspace");
        assert_eq!(files(&root.join("copy")), before, "the supplied copy is never opened by librime");
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64;
        let store = LearningStore::load(&root.join("learned"), now);
        assert_eq!(store.entries().iter().find(|entry| entry.text == "再搞").unwrap().count, 7);
        assert_eq!(store.entries().iter().find(|entry| entry.text == "裁我").unwrap().count, 3);
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(feature = "librime")]
    fn fidelity_rows() -> Vec<(String, Vec<String>)> {
        let syllables = ["ta", "na", "ba", "ma", "da", "gen", "zai", "you", "dao",
            "wo", "ni", "ren", "hao", "de", "la", "bo", "po", "mo"];
        let mut rows = Vec::new();
        for length in [1, 2, 4] {
            for (index, syllable) in syllables.iter().enumerate() {
                let reading = match length {
                    1 => syllable.to_string(),
                    2 => format!("{syllable} hao"),
                    _ => format!("{syllable} de hao ma"),
                };
                let texts = (0..6).map(|variant| {
                    if length == 1 && index == 0 {
                        ["它", "他", "塔", "踏", "榻", "獭"][variant].to_string()
                    } else {
                        char::from_u32(0x4e00 + (rows.len() * 6 + variant) as u32)
                            .unwrap().to_string().repeat(length)
                    }
                }).collect();
                rows.push((reading, texts));
            }
        }
        rows
    }

    #[cfg(feature = "librime")]
    fn write_fidelity_fixture(shared: &Path, snapshot: &Path) {
        fs::write(shared.join("default.yaml"), "config_version: '1'\nschema_list:\n  - schema: agent_pinyin\nmenu:\n  page_size: 9\n").unwrap();
        fs::write(shared.join("agent_pinyin.schema.yaml"), r#"schema:
  schema_id: agent_pinyin
  name: Synthetic migration fidelity
  version: '1'
engine:
  processors: [speller, selector, navigator, express_editor]
  segmentors: [abc_segmentor]
  translators: [script_translator]
  filters: [uniquifier]
speller:
  alphabet: abcdefghijklmnopqrstuvwxyz
  delimiter: " '"
translator:
  dictionary: pinyin_simp
  enable_sentence: true
  enable_user_dict: true
menu:
  page_size: 9
"#).unwrap();
        let mut dictionary = String::from("---\nname: pinyin_simp\nversion: '1'\nsort: by_weight\n...\n");
        let mut backup = String::from("# Rime user dictionary\n#@/db_name\tpinyin_simp\n#@/db_type\tuserdb\n#@/rime_version\t1.16.1\n#@/tick\t10000\n");
        for (reading, texts) in fidelity_rows() {
            for (variant, text) in texts.iter().enumerate() {
                dictionary.push_str(&format!("{text}\t{reading}\t{}\n", 1000 - variant * 10));
                if variant < 5 {
                    let (count, dynamics, tick) = [(3, 5.0, 10000), (37, 0.1, 1),
                        (12, 3.0, 9980), (8, 2.0, 9970), (1, 0.5, 9900)][variant];
                    backup.push_str(&format!("{reading} \t{text}\tc={count} d={dynamics} t={tick}\n"));
                }
            }
        }
        fs::write(shared.join("pinyin_simp.dict.yaml"), dictionary).unwrap();
        fs::write(snapshot, backup).unwrap();
    }

    #[cfg(feature = "librime")]
    #[test]
    fn real_backup_preserves_dynamics_and_fifty_four_reading_orders() {
        use std::process::Command;
        let root = scratch("fidelity");
        for action in ["create-fidelity", "migrate-fidelity", "compare-fidelity"] {
            let result = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "tests::rime_migration_worker", "--ignored", "--nocapture"])
                .current_dir(&root)
                .env("AIM_MIGRATION_TEST_ROOT", &root)
                .env("AIM_MIGRATION_TEST_ACTION", action)
                .output().unwrap();
            assert!(result.status.success(), "isolated {action} failed: {}{}",
                String::from_utf8_lossy(&result.stdout), String::from_utf8_lossy(&result.stderr));
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(feature = "librime")]
    #[test]
    fn runtime_worker_accepts_an_existing_empty_rime_database() {
        use std::process::Command;
        let root = scratch("empty-runtime");
        for action in ["create-empty", "migrate-runtime"] {
            let result = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "tests::rime_migration_worker", "--ignored", "--nocapture"])
                .current_dir(&root)
                .env("AIM_MIGRATION_TEST_ROOT", &root)
                .env("AIM_MIGRATION_TEST_ACTION", action)
                .output().unwrap();
            assert!(result.status.success(), "isolated {action} failed: {}{}",
                String::from_utf8_lossy(&result.stdout), String::from_utf8_lossy(&result.stderr));
        }
        let json: serde_json::Value = serde_json::from_slice(&fs::read(root.join("copy").join(agent_ime_core::zhixu::learning::FILE_NAME)).unwrap()).unwrap();
        assert_eq!(json["migrated"], true);
        assert_eq!(json["entries"].as_array().unwrap().len(), 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(feature = "librime")]
    #[test]
    fn runtime_worker_migrates_while_the_original_host_remains_open() {
        use std::process::Command;
        let root = scratch("live-switch");
        for action in ["create-fidelity", "switch-live"] {
            let result = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "tests::rime_migration_worker", "--ignored", "--nocapture"])
                .current_dir(&root)
                .env("AIM_MIGRATION_TEST_ROOT", &root)
                .env("AIM_MIGRATION_TEST_ACTION", action)
                .output().unwrap();
            assert!(result.status.success(), "isolated {action} failed: {}{}",
                String::from_utf8_lossy(&result.stdout), String::from_utf8_lossy(&result.stderr));
        }
        assert!(!root.join("sync").exists());
        fs::remove_dir_all(root).unwrap();
    }

    // Each invocation gets its own librime globals and closes the database before
    // the parent snapshots/copies it. No schemas or owner files are needed.
    #[cfg(feature = "librime")]
    #[test]
    #[ignore = "invoked in an isolated child by the real-export test"]
    fn rime_migration_worker() {
        use std::ffi::{CString, c_char, c_int};
        unsafe extern "C" {
            fn aim_rime_initialize(shared: *const c_char, user: *const c_char) -> c_int;
            fn aim_rime_restore_user_dict(snapshot: *const c_char) -> c_int;
        }
        let root = PathBuf::from(std::env::var_os("AIM_MIGRATION_TEST_ROOT").unwrap());
        let action = std::env::var("AIM_MIGRATION_TEST_ACTION").unwrap();
        let (shared, user, output) = (root.join("shared"), root.join("copy"), root.join("learned"));
        if action == "create-empty" {
            use zhizi_host::{Host, HostConfig, Key};
            private_directory(&shared).unwrap();
            private_directory(&user).unwrap();
            write_fidelity_fixture(&shared, &root.join("unused-snapshot.txt"));
            let mut host = Host::open(&HostConfig { shared_dir: shared, user_dir: user.clone(), app_id: "zhizi.eval".into() }).unwrap();
            host.process(Key::Text("t".into()), 0, false);
            host.process(Key::Text("a".into()), 0, false);
            host.process(Key::Escape, 0, false);
            assert!(user.join("pinyin_simp.userdb").is_dir());
        } else if action == "create-fidelity" {
            private_directory(&shared).unwrap();
            private_directory(&user).unwrap();
            let snapshot = root.join("synthetic.userdb.txt");
            write_fidelity_fixture(&shared, &snapshot);
            let c_path = |path: &Path| CString::new(path.to_str().unwrap()).unwrap();
            let (shared_c, user_c, snapshot_c) = (c_path(&shared), c_path(&user), c_path(&snapshot));
            assert_ne!(unsafe { aim_rime_initialize(shared_c.as_ptr(), user_c.as_ptr()) }, 0);
            assert_ne!(unsafe { aim_rime_restore_user_dict(snapshot_c.as_ptr()) }, 0);
            #[repr(C)]
            struct RimeString { data: *mut c_char, len: usize }
            unsafe extern "C" {
                fn aim_rime_session_create() -> usize;
                fn aim_rime_session_destroy(session: usize);
                fn aim_rime_process_key(session: usize, key: c_int, mask: c_int, commit: *mut RimeString) -> c_int;
                fn aim_rime_select_candidate(session: usize, index: usize, commit: *mut RimeString) -> c_int;
                fn aim_rime_string_free(value: *mut RimeString);
            }
            let session = unsafe { aim_rime_session_create() };
            assert_ne!(session, 0);
            for _ in 0..3 {
                let mut commit = RimeString { data: std::ptr::null_mut(), len: 0 };
                for key in b"ta" {
                    assert_ne!(unsafe { aim_rime_process_key(session, *key as c_int, 0, &mut commit) }, 0);
                    unsafe { aim_rime_string_free(&mut commit) };
                }
                assert_ne!(unsafe { aim_rime_select_candidate(session, 0, &mut commit) }, 0);
                let text = unsafe { std::slice::from_raw_parts(commit.data as *const u8, commit.len) };
                assert_eq!(text, "它".as_bytes());
                unsafe { aim_rime_string_free(&mut commit) };
            }
            unsafe { aim_rime_session_destroy(session) };
        } else if action == "switch-live" {
            use zhizi_host::{Host, HostConfig, Key, LearningMode};
            let mut host = Host::open(&HostConfig { shared_dir: shared.clone(), user_dir: user.clone(), app_id: "zhizi.eval".into() }).unwrap();
            let database = user.join("pinyin_simp.userdb");
            let files = || -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
                fs::read_dir(&database).unwrap().map(|entry| {
                    let path = entry.unwrap().path();
                    let bytes = fs::read(&path).unwrap();
                    (path, bytes)
                }).collect()
            };
            let before = files();
            let migrate = || {
                let result = std::process::Command::new(std::env::current_exe().unwrap())
                    .args(["--exact", "tests::rime_migration_worker", "--ignored", "--nocapture"])
                    .current_dir(&root)
                    .env("AIM_MIGRATION_TEST_ROOT", &root)
                    .env("AIM_MIGRATION_TEST_ACTION", "migrate-runtime")
                    .output().unwrap();
                assert!(result.status.success(), "runtime worker failed: {}{}",
                    String::from_utf8_lossy(&result.stdout), String::from_utf8_lossy(&result.stderr));
            };
            migrate();
            assert_eq!(files(), before, "the worker must never open or mutate the live Rime database");
            let library = user.join(agent_ime_core::zhixu::learning::FILE_NAME);
            let migrated = fs::read(&library).unwrap();
            migrate();
            assert_eq!(fs::read(&library).unwrap(), migrated, "second invocation must retain existing learning byte for byte");
            let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64;
            assert_eq!(LearningStore::load(&user, now).entries().len(), 270);
            for (reading, _) in fidelity_rows() {
                let mut pages = Vec::new();
                for mode in [LearningMode::Rime, LearningMode::Zhixu] {
                    assert!(host.set_learning(mode));
                    let mut view = Default::default();
                    for letter in reading.replace(' ', "").chars() {
                        view = host.process(Key::Text(letter.to_string()), 0, false);
                    }
                    let page: Vec<_> = view.candidates.iter().take(5).map(|candidate| candidate.text.clone()).collect();
                    assert_eq!(page.len(), 5);
                    if reading == "ta" { assert_eq!(page[0], "它"); }
                    pages.push(page);
                    host.process(Key::Escape, 0, false);
                }
                assert_eq!(pages[0], pages[1], "switch changed {reading} on the already-open session");
            }
        } else if action == "migrate-runtime" {
            let shared = shared.to_str().unwrap().as_bytes();
            let user = user.to_str().unwrap().as_bytes();
            assert_eq!(unsafe { agent_ime_core::aim_learning_migrate(shared.as_ptr(), shared.len(), user.as_ptr(), user.len()) }, 0);
            assert_eq!(unsafe { agent_ime_core::aim_learning_available(user.as_ptr(), user.len()) }, 1);
        } else if action == "migrate-fidelity" {
            let workspace = Workspace::new().unwrap();
            let exported = export_copy(&user, &shared, &workspace).unwrap();
            assert!(workspace.0.join("sync").is_dir());
            assert!(!root.join("sync").exists());
            let text = read_export(&exported).unwrap();
            let tick = text.lines().find_map(|line| line.strip_prefix("#@/tick\t"))
                .expect("backup must retain the logical clock").parse::<u64>().unwrap();
            assert!(tick > 10000, "real choices must advance the clock");
            let metadata = |wanted: &str| {
                let line = text.lines().find(|line| line.split('\t').nth(1) == Some(wanted)).unwrap();
                let value = line.split('\t').nth(2).unwrap();
                let field = |prefix: &str| value.split_whitespace().find_map(|part| part.strip_prefix(prefix)).unwrap();
                (field("c=").parse::<u32>().unwrap(), field("d=").parse::<f64>().unwrap(), field("t=").parse::<u64>().unwrap())
            };
            let old = metadata("他");
            let recent = metadata("它");
            assert!(old.0 > recent.0 && old.1 < recent.1 && old.2 < recent.2,
                "real exported counts and recency must disagree: {old:?}, {recent:?}");
            let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64;
            let mut store = LearningStore::default();
            assert_eq!(store.import_rime_export(&text, now).unwrap(), 270);
            store.save(&output).unwrap();
            let workspace_path = workspace.0.clone();
            drop(workspace);
            assert!(!workspace_path.exists(), "temporary database and sync snapshots must be removed");
            copy_user_database(&user, &output).unwrap();
        } else if action == "compare-fidelity" {
            use zhizi_host::{Host, HostConfig, Key, LearningMode};
            let config = HostConfig { shared_dir: shared, user_dir: output, app_id: "zhizi.eval".into() };
            let mut host = Host::open(&config).unwrap();
            for (reading, _) in fidelity_rows() {
                let mut spellings = vec![reading.replace(' ', ""), reading.replace(' ', "'")];
                spellings.dedup();
                for raw in spellings {
                    let mut pages = Vec::new();
                    for mode in [LearningMode::Rime, LearningMode::Zhixu] {
                        assert!(host.set_learning(mode));
                        let mut view = Default::default();
                        for letter in raw.chars() {
                            view = host.process(Key::Text(letter.to_string()), 0, false);
                        }
                        let top: Vec<_> = view.candidates.iter().take(5).map(|candidate| candidate.text.clone()).collect();
                        assert_eq!(top.len(), 5, "{reading}: {view:?}");
                        if reading == "ta" { assert_eq!(top[0], "它", "{mode:?}: {top:?}"); }
                        pages.push(top);
                        host.process(Key::Escape, 0, false);
                    }
                    assert_eq!(pages[0], pages[1], "migration changed {reading} ({raw})");
                }
            }
        } else if action == "create" {
            private_directory(&shared).unwrap();
            private_directory(&user).unwrap();
            let export = root.join("synthetic.txt");
            fs::write(&export, "#@/db_name\tpinyin_simp\n#@/db_type\tuserdb\n#@/tick\t10\nzai gao \t再搞\tc=7 d=4 t=10\ncai wo \t裁我\tc=3 d=2 t=9\n").unwrap();
            let c_path = |path: &Path| CString::new(path.to_str().unwrap()).unwrap();
            let (shared, user, export) = (c_path(&shared), c_path(&user), c_path(&export));
            assert_ne!(unsafe { aim_rime_initialize(shared.as_ptr(), user.as_ptr()) }, 0);
            assert_ne!(unsafe { aim_rime_restore_user_dict(export.as_ptr()) }, 0);
        } else {
            assert_eq!(action, "migrate");
            let arguments = vec!["--copy-user-dir".into(), user.to_str().unwrap().into(),
                "--shared-dir".into(), shared.to_str().unwrap().into(),
                "--output-dir".into(), output.to_str().unwrap().into()];
            assert_eq!(run(&arguments).unwrap(), 2);
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_new_output_below_a_live_directory_alias_is_rejected() {
        use std::os::unix::fs::symlink;
        let root = scratch("output-alias");
        let live = root.join("Application Support/AgentInputMethod");
        fs::create_dir_all(&live).unwrap();
        let alias = root.join("alias");
        symlink(&live, &alias).unwrap();
        assert!(reject_live_path(&alias.join("new-output")).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn staging_rejects_symlinks_in_the_copy() {
        use std::os::unix::fs::symlink;
        let root = scratch("symlink");
        let source = root.join("copy");
        let destination = root.join("staging");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&destination).unwrap();
        fs::create_dir(source.join("pinyin_simp.userdb")).unwrap();
        fs::write(root.join("outside"), b"must not follow").unwrap();
        symlink(root.join("outside"), source.join("pinyin_simp.userdb/000001.ldb")).unwrap();
        assert!(copy_user_database(&source, &destination).is_err());
        assert!(!destination.join("pinyin_simp.userdb/000001.ldb").exists());
        fs::remove_dir_all(root).unwrap();
    }
}
