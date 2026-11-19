//! Per-user learning management used by the Linux settings window. Input and
//! output are private pipes; no learning text is written to diagnostic logs.
#[cfg(unix)]
mod unix {
    use agent_ime_core::zhixu::learning::{self, LearningWordBackup, ManagementKey};
    use serde::{Deserialize, Serialize};
    use serde_json::{Value, json};
    use std::{
        fs::{self, File, OpenOptions},
        io::{self, Read, Write},
        os::unix::{
            fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
            io::AsRawFd,
        },
        path::{Path, PathBuf},
        time::{SystemTime, UNIX_EPOCH},
    };
    #[cfg(target_os = "linux")]
    const NOFOLLOW: i32 = 0x20000;
    #[cfg(target_os = "macos")]
    const NOFOLLOW: i32 = 0x100;
    unsafe extern "C" {
        fn geteuid() -> u32;
        fn flock(fd: i32, operation: i32) -> i32;
    }
    #[derive(Deserialize)]
    #[serde(tag = "op", rename_all = "snake_case")]
    enum Request {
        List,
        BlacklistList,
        BlacklistRemove {
            keys: Vec<agent_ime_core::blacklist::BlacklistEntry>,
        },
        Export,
        Preview {
            text: String,
        },
        Import {
            text: String,
        },
        Delete {
            keys: Vec<ManagementKey>,
        },
        Undo,
    }
    #[derive(Serialize, Deserialize)]
    struct Undo {
        format_version: u32,
        created_ms: u64,
        words: Vec<LearningWordBackup>,
    }
    const UNDO_AGE: u64 = 30 * 60 * 1000;
    fn now() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    }
    fn invalid(message: &str) -> io::Error {
        io::Error::new(io::ErrorKind::InvalidData, message)
    }
    fn private_directory(path: &Path) -> io::Result<()> {
        match fs::create_dir(path) {
            Ok(()) => fs::set_permissions(path, fs::Permissions::from_mode(0o700))?,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => (),
            Err(e) => return Err(e),
        }
        let m = fs::symlink_metadata(path)?;
        if !m.is_dir() || m.uid() != unsafe { geteuid() } || m.mode() & 0o077 != 0 {
            return Err(invalid("private journal permissions"));
        }
        Ok(())
    }
    fn lock(directory: &Path) -> io::Result<File> {
        let f = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(NOFOLLOW)
            .open(directory.join("management.lock"))?;
        let m = f.metadata()?;
        if !m.is_file() || m.uid() != unsafe { geteuid() } || m.mode() & 0o077 != 0 {
            return Err(invalid("private lock permissions"));
        }
        if unsafe { flock(f.as_raw_fd(), 2 | 4) } != 0 {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "another learning manager is active",
            ));
        }
        Ok(f)
    }
    fn write_undo(directory: &Path, words: &[LearningWordBackup]) -> io::Result<()> {
        let temporary = directory.join(format!("undo-{}.tmp", std::process::id()));
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)?;
        let result = (|| {
            serde_json::to_writer(
                &mut f,
                &Undo {
                    format_version: 1,
                    created_ms: now(),
                    words: words.to_vec(),
                },
            )?;
            f.flush()?;
            f.sync_all()?;
            fs::rename(&temporary, directory.join("latest.json"))?;
            File::open(directory)?.sync_all()
        })();
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
        result
    }
    fn read_undo(directory: &Path) -> io::Result<Undo> {
        let mut f = OpenOptions::new()
            .read(true)
            .custom_flags(NOFOLLOW)
            .open(directory.join("latest.json"))?;
        let m = f.metadata()?;
        if !m.is_file()
            || m.uid() != unsafe { geteuid() }
            || m.mode() & 0o077 != 0
            || m.len() > 8 * 1024 * 1024
        {
            return Err(invalid("invalid undo file"));
        }
        let mut text = String::new();
        f.read_to_string(&mut text)?;
        let undo: Undo = serde_json::from_str(&text)?;
        if undo.format_version != 1 || undo.created_ms > now() || now() - undo.created_ms > UNDO_AGE
        {
            return Err(invalid("undo expired"));
        }
        Ok(undo)
    }
    fn dispatch(data: &Path, request: Request) -> io::Result<Value> {
        fs::create_dir_all(data)?;
        let journal = data.join(".learning-undo-v1");
        private_directory(&journal)?;
        let _lock = lock(&journal)?;
        let user = data.join("Rime");
        match request {
            Request::List => Ok(
                json!({"snapshot":learning::management_list(&user)?,"undoAvailable":read_undo(&journal).is_ok()}),
            ),
            Request::BlacklistList => {
                Ok(json!({"items":agent_ime_core::blacklist::managed_list(&user)?}))
            }
            Request::BlacklistRemove { keys } => {
                Ok(json!({"removed":agent_ime_core::blacklist::managed_remove(&user,&keys)?}))
            }
            Request::Export => Ok(json!({"text":learning::management_export(&user)?})),
            Request::Preview { text } => {
                Ok(json!({"preview":learning::management_preview(&user,&text)?}))
            }
            Request::Import { text } => {
                let preview = learning::management_import(&user, &text)?;
                let _ = fs::remove_file(journal.join("latest.json"));
                Ok(json!({"imported":preview}))
            }
            Request::Delete { keys } => {
                let removed = learning::management_delete_journaled(&user, &keys, |words| {
                    write_undo(&journal, words)
                })?;
                Ok(json!({"deleted":removed.len()}))
            }
            Request::Undo => {
                let undo = read_undo(&journal)?;
                let restored = learning::management_restore(&user, &undo.words)?;
                fs::remove_file(journal.join("latest.json"))?;
                File::open(&journal)?.sync_all()?;
                Ok(json!({"restored":restored}))
            }
        }
    }
    pub fn main() {
        let result = (|| -> Result<Value, Box<dyn std::error::Error>> {
            let args: Vec<_> = std::env::args().skip(1).collect();
            if args.len() != 1 {
                return Err("user data directory required".into());
            };
            let mut input = String::new();
            io::stdin()
                .take(8 * 1024 * 1024 + 1)
                .read_to_string(&mut input)?;
            if input.len() > 8 * 1024 * 1024 {
                return Err("backup too large".into());
            };
            Ok(dispatch(
                &PathBuf::from(&args[0]),
                serde_json::from_str(&input)?,
            )?)
        })();
        match result {
            Ok(v) => println!("{v}"),
            Err(e) => {
                println!("{}", json!({"error":format!("{e}")}));
                std::process::exit(1)
            }
        }
    }
    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn durable_undo_precedes_delete_and_restores_once() {
            let root =
                std::env::temp_dir().join(format!("zhizi-linux-learning-{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(root.join("Rime")).unwrap();
            let mut store = learning::LearningStore::default();
            assert!(store.record("ce shi", "测试", now()));
            store.save(&root.join("Rime")).unwrap();
            let result = dispatch(
                &root,
                Request::Delete {
                    keys: vec![ManagementKey {
                        reading: "ce shi".into(),
                        text: "测试".into(),
                    }],
                },
            )
            .unwrap();
            assert_eq!(result["deleted"], 1);
            assert_eq!(
                learning::management_list(&root.join("Rime"))
                    .unwrap()
                    .items
                    .len(),
                0
            );
            assert!(root.join(".learning-undo-v1/latest.json").exists());
            dispatch(&root, Request::Undo).unwrap();
            assert_eq!(
                learning::management_list(&root.join("Rime"))
                    .unwrap()
                    .items
                    .len(),
                1
            );
            assert!(dispatch(&root, Request::Undo).is_err());
            fs::remove_dir_all(root).unwrap();
        }
        #[test]
        fn rejects_linked_private_journal() {
            let root =
                std::env::temp_dir().join(format!("zhizi-linux-link-{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(root.join("other")).unwrap();
            std::os::unix::fs::symlink(root.join("other"), root.join(".learning-undo-v1")).unwrap();
            assert!(dispatch(&root, Request::List).is_err());
            fs::remove_dir_all(root).unwrap();
        }
    }
}
#[cfg(unix)]
fn main() {
    unix::main()
}
#[cfg(not(unix))]
fn main() {
    eprintln!("This bridge is for the Unix settings window");
    std::process::exit(2)
}
