#![cfg_attr(not(any(test, feature = "librime")), allow(dead_code))]

use crate::secure_directory::{
    open_or_create_owned_leaf_directory, physical_runtime_path, private_create, private_regular_file,
    sync_directory, validate_owned_directory,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};

const BLACKLIST_FILE_NAME: &str = "agent_personal_blacklist_v1.json";
const BLACKLIST_FORMAT_VERSION: u32 = 1;
const MAX_BLACKLIST_ENTRIES: usize = 4096;
const MAX_BLACKLIST_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct BlacklistEntry {
    reading: String,
    text: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct BlacklistDocument {
    format_version: u32,
    entries: Vec<BlacklistEntry>,
}

#[derive(Debug)]
pub(crate) struct Store {
    path: Option<PathBuf>,
    entries: HashSet<BlacklistEntry>,
}

impl Default for Store {
    fn default() -> Self {
        Self {
            path: None,
            entries: HashSet::new(),
        }
    }
}

impl Store {
    fn open(user_dir: &Path) -> io::Result<Self> {
        let user_dir = physical_runtime_path(user_dir);
        let directory = open_or_create_owned_leaf_directory(&user_dir, 0o700)?;
        validate_owned_directory(&directory, "blacklist parent")?;
        let path = user_dir.join(BLACKLIST_FILE_NAME);
        let bytes = match fs::symlink_metadata(&path) {
            Ok(metadata) => {
                if !private_regular_file(&metadata)
                    || metadata.len() > MAX_BLACKLIST_BYTES
                {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "blacklist file failed ownership, type, link, mode, or size validation",
                    ));
                }
                fs::read(&path)?
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(error) => return Err(error),
        };
        let entries = if bytes.is_empty() {
            HashSet::new()
        } else {
            let document: BlacklistDocument = serde_json::from_slice(&bytes).map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("invalid blacklist: {error}"),
                )
            })?;
            if document.format_version != BLACKLIST_FORMAT_VERSION
                || document.entries.len() > MAX_BLACKLIST_ENTRIES
                || document.entries.iter().any(|entry| !valid_entry(entry))
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "blacklist schema, count, or entry is invalid",
                ));
            }
            document.entries.into_iter().collect()
        };
        Ok(Self {
            path: Some(path),
            entries,
        })
    }

    /// The Rime user directory this store lives in (None for the in-memory default).
    pub(crate) fn directory(&self) -> Option<PathBuf> {
        self.path.as_ref()?.parent().map(Path::to_path_buf)
    }

    pub(crate) fn contains(&self, reading: &str, text: &str) -> bool {
        self.entries.contains(&BlacklistEntry {
            reading: reading.to_string(),
            text: text.to_string(),
        })
    }

    pub(crate) fn insert(&mut self, reading: &str, text: &str) -> io::Result<bool> {
        let entry = BlacklistEntry {
            reading: reading.to_string(),
            text: text.to_string(),
        };
        if !valid_entry(&entry) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "blacklist entry is empty or too large",
            ));
        }
        if self.entries.contains(&entry) {
            return Ok(false);
        }
        if self.entries.len() >= MAX_BLACKLIST_ENTRIES {
            return Err(io::Error::new(
                io::ErrorKind::StorageFull,
                "blacklist entry cap reached",
            ));
        }
        let mut next = self.entries.clone();
        next.insert(entry);
        self.persist(&next)?;
        self.entries = next;
        Ok(true)
    }

    fn persist(&self, entries: &HashSet<BlacklistEntry>) -> io::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let mut sorted = entries.iter().cloned().collect::<Vec<_>>();
        sorted.sort();
        let bytes = serde_json::to_vec_pretty(&BlacklistDocument {
            format_version: BLACKLIST_FORMAT_VERSION,
            entries: sorted,
        })
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        if bytes.len() as u64 > MAX_BLACKLIST_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::StorageFull,
                "blacklist document is too large",
            ));
        }
        let parent = path.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "blacklist path has no parent")
        })?;
        let temporary = parent.join(format!(".{BLACKLIST_FILE_NAME}.{}.tmp", std::process::id()));
        let mut file = private_create(OpenOptions::new().write(true).create_new(true))
            .open(&temporary)?;
        let write_result = (|| {
            file.write_all(&bytes)?;
            file.sync_all()?;
            fs::rename(&temporary, path)?;
            sync_directory(parent)
        })();
        if write_result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        write_result
    }
}

fn valid_entry(entry: &BlacklistEntry) -> bool {
    !entry.reading.is_empty()
        && entry.reading.len() <= 256
        && !entry.text.is_empty()
        && entry.text.len() <= 1024
        && entry.reading.bytes().all(|byte| byte.is_ascii_lowercase())
}

#[cfg_attr(not(feature = "librime"), allow(dead_code))]
pub(crate) fn shared_blacklist(user_dir: &Path) -> io::Result<Arc<Mutex<Store>>> {
    static STORES: OnceLock<Mutex<std::collections::HashMap<PathBuf, Weak<Mutex<Store>>>>> =
        OnceLock::new();
    let path = physical_runtime_path(user_dir);
    let stores = STORES.get_or_init(|| Mutex::new(std::collections::HashMap::new()));
    let mut stores = stores
        .lock()
        .map_err(|_| io::Error::other("blacklist registry lock poisoned"))?;
    if let Some(store) = stores.get(&path).and_then(Weak::upgrade) {
        return Ok(store);
    }
    let store = Arc::new(Mutex::new(Store::open(&path)?));
    stores.insert(path, Arc::downgrade(&store));
    Ok(store)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temporary_directory() -> PathBuf {
        static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "agent-ime-blacklist-{}-{nonce}-{}",
            std::process::id(),
            NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        path
    }

    #[test]
    fn persistent_store_is_scoped_and_reloadable() {
        let directory = temporary_directory();
        let mut store = Store::open(&directory).unwrap();
        assert!(store.insert("zhge", "栅格").unwrap());
        assert!(store.contains("zhge", "栅格"));
        assert!(!store.contains("zhg", "栅格"));
        assert!(!store.contains("mariadb", "栅格"));
        let reloaded = Store::open(&directory).unwrap();
        assert!(reloaded.contains("zhge", "栅格"));
        let mode = fs::metadata(directory.join(BLACKLIST_FILE_NAME))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o077, 0);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn unsafe_or_corrupt_files_fail_closed() {
        let directory = temporary_directory();
        let path = directory.join(BLACKLIST_FILE_NAME);
        fs::write(&path, b"not-json").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(Store::open(&directory).is_err());
        fs::write(&path, br#"{"formatVersion":1,"entries":[]}"#).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(Store::open(&directory).is_err());
        fs::remove_dir_all(directory).unwrap();
    }
}
