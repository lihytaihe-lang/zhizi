#![cfg_attr(not(any(test, feature = "librime")), allow(dead_code))]

//! The English the owner picked for an input (`mini` -> mini over 迷你, `opencode` ->
//! opencode). A deliberate pick of English remembers it, so the next time the same
//! input leads with it; a later deliberate pick of Chinese for that input forgets it.
//! Local only, next to the blacklist in the Rime user directory.

use crate::secure_directory::{
    open_or_create_owned_leaf_directory, physical_runtime_path, private_create, private_regular_file,
    sync_directory, validate_owned_directory,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};

const FILE_NAME: &str = "agent_personal_english_v1.json";
const FORMAT_VERSION: u32 = 1;
const MAX_ENTRIES: usize = 4096;
const MAX_PENDING: usize = 256;
const MAX_BYTES: u64 = 512 * 1024;

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Document {
    format_version: u32,
    /// input (lowercase letters) -> committed English text
    entries: BTreeMap<String, String>,
    /// Words committed once with Enter that are also whole pinyin (mini, long): the second Enter, in any
    /// process, makes them a choice. Written only while non-empty, so a file without any reads as before.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pending: BTreeSet<String>,
}

#[derive(Debug, Default)]
pub(crate) struct Store {
    path: Option<PathBuf>,
    entries: BTreeMap<String, String>,
    pending: BTreeSet<String>,
}

impl Store {
    fn open(user_dir: &Path) -> io::Result<Self> {
        let user_dir = physical_runtime_path(user_dir);
        let directory = open_or_create_owned_leaf_directory(&user_dir, 0o700)?;
        validate_owned_directory(&directory, "English choices parent")?;
        let path = user_dir.join(FILE_NAME);
        let (entries, pending) = match fs::symlink_metadata(&path) {
            Ok(metadata) => {
                if !private_regular_file(&metadata)
                    || metadata.len() > MAX_BYTES
                {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "English choices file failed ownership, type, link, mode, or size validation",
                    ));
                }
                // A damaged file only loses what was learned; it never blocks typing.
                serde_json::from_slice::<Document>(&fs::read(&path)?)
                    .ok()
                    .filter(|document| {
                        document.format_version == FORMAT_VERSION
                            && document.entries.len() <= MAX_ENTRIES
                            && document.entries.iter().all(|(input, text)| valid(input, text))
                    })
                    .map(|document| (document.entries, document.pending))
                    .unwrap_or_default()
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => (BTreeMap::new(), BTreeSet::new()),
            Err(error) => return Err(error),
        };
        Ok(Self {
            path: Some(path),
            entries,
            pending: pending.into_iter().filter(|word| valid(word, word)).take(MAX_PENDING).collect(),
        })
    }

    pub(crate) fn get(&self, input: &str) -> Option<&str> {
        self.entries.get(input).map(String::as_str)
    }

    pub(crate) fn remember(&mut self, input: &str, text: &str) -> io::Result<()> {
        if !valid(input, text) || self.get(input) == Some(text) {
            return Ok(());
        }
        if !self.entries.contains_key(input) && self.entries.len() >= MAX_ENTRIES {
            return Ok(());
        }
        let mut next = self.entries.clone();
        next.insert(input.to_string(), text.to_string());
        self.persist(&next, &self.pending)?;
        self.entries = next;
        Ok(())
    }

    /// `word` was committed with Enter. True from the second time (in any process); the first time is kept.
    pub(crate) fn note_enter(&mut self, word: &str) -> bool {
        if self.pending.contains(word) {
            let mut next = self.pending.clone();
            next.remove(word);
            if self.persist(&self.entries, &next).is_ok() {
                self.pending = next;
            }
            return true;
        }
        if valid(word, word) && self.pending.len() < MAX_PENDING {
            let mut next = self.pending.clone();
            next.insert(word.to_string());
            if self.persist(&self.entries, &next).is_ok() {
                self.pending = next;
            }
        }
        false
    }

    pub(crate) fn forget(&mut self, input: &str) -> io::Result<()> {
        if !self.entries.contains_key(input) {
            return Ok(());
        }
        let mut next = self.entries.clone();
        next.remove(input);
        self.persist(&next, &self.pending)?;
        self.entries = next;
        Ok(())
    }

    fn persist(&self, entries: &BTreeMap<String, String>, pending: &BTreeSet<String>) -> io::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let bytes = serde_json::to_vec_pretty(&Document {
            format_version: FORMAT_VERSION,
            entries: entries.clone(),
            pending: pending.clone(),
        })
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::StorageFull,
                "English choices document is too large",
            ));
        }
        let parent = path.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "English choices path has no parent")
        })?;
        let temporary = parent.join(format!(".{FILE_NAME}.{}.tmp", std::process::id()));
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

/// The input is lowercase letters; the text is short printable ASCII that starts
/// with those letters (opencode, Windows for `wind`).
pub(crate) fn valid(input: &str, text: &str) -> bool {
    (2..=64).contains(&input.len())
        && input.bytes().all(|byte| byte.is_ascii_lowercase())
        && (1..=128).contains(&text.len())
        && text.bytes().all(|byte| byte.is_ascii_graphic() || byte == b' ')
        && text.to_ascii_lowercase().starts_with(input)
}

#[cfg_attr(not(feature = "librime"), allow(dead_code))]
pub(crate) fn shared(user_dir: &Path) -> io::Result<Arc<Mutex<Store>>> {
    static STORES: OnceLock<Mutex<std::collections::HashMap<PathBuf, Weak<Mutex<Store>>>>> =
        OnceLock::new();
    let path = physical_runtime_path(user_dir);
    let stores = STORES.get_or_init(|| Mutex::new(std::collections::HashMap::new()));
    let mut stores = stores
        .lock()
        .map_err(|_| io::Error::other("English choices registry lock poisoned"))?;
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
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn remembers_forgets_and_survives_reopening() {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "aim-english-choices-{}-{nanos}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        fs::create_dir(&directory).unwrap();
        let directory = fs::canonicalize(&directory).unwrap();
        let mut store = Store::open(&directory).unwrap();
        store.remember("mini", "mini").unwrap();
        store.remember("wind", "Windows").unwrap();
        store.remember("mini", "迷你").unwrap();
        store.remember("ok", "fine").unwrap();
        let reopened = Store::open(&directory).unwrap();
        assert_eq!(reopened.get("mini"), Some("mini"));
        assert_eq!(reopened.get("wind"), Some("Windows"));
        assert_eq!(reopened.get("ok"), None);
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let mode = fs::metadata(directory.join(FILE_NAME)).unwrap().mode();
            assert_eq!(mode & 0o077, 0);
        }
        let mut store = reopened;
        assert!(!store.note_enter("long"));
        let mut again = Store::open(&directory).unwrap();
        assert!(again.note_enter("long"), "the first Enter survives a new process");
        assert!(!Store::open(&directory).unwrap().note_enter("long"), "and is spent by the second");
        store.forget("mini").unwrap();
        assert_eq!(Store::open(&directory).unwrap().get("mini"), None);
        fs::remove_dir_all(directory).unwrap();
    }
}
