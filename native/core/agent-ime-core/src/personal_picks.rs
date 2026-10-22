#![cfg_attr(not(any(test, feature = "librime")), allow(dead_code))]

//! The candidates the owner deliberately picked (not the first one) for an input, kept across restarts.
//!
//! Until 2026-10-17 these lived only in the IME process and decayed after 128 other deliberate picks, so
//! a pick was forgotten by the end of the day or by the next restart (`nikk` -> 你看看 picked 8 times, still
//! third; every build handed to the owner restarted the process). Now they sit next to the blacklist and the
//! English choices in the Rime user directory, with the number of picks and the time of the last one: a pick
//! moves up at most three places the first time, to the first place once it was made three times, and is
//! forgotten after 45 days without being picked again. Local only.

use crate::secure_directory::{
    open_or_create_owned_leaf_directory, physical_runtime_path, private_create, private_regular_file,
    sync_directory, validate_owned_directory,
};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

const FILE_NAME: &str = "agent_personal_picks_v1.json";
const FORMAT_VERSION: u32 = 1;
pub(crate) const MAX_ENTRIES: usize = 4096;
const MAX_BYTES: u64 = 1024 * 1024;
/// A pick that was not repeated for this long is forgotten.
pub(crate) const MAX_AGE_MS: u64 = 45 * 24 * 60 * 60 * 1000;

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct Entry {
    pub app: String,
    pub reading: String,
    pub text: String,
    /// Where the picked candidate is put (0 = first); the lowest ever reached.
    pub target: usize,
    pub count: u32,
    pub last_ms: u64,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Document {
    format_version: u32,
    entries: Vec<Entry>,
}

pub(crate) fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn valid(entry: &Entry) -> bool {
    (1..=128).contains(&entry.app.len())
        && (1..=64).contains(&entry.reading.len())
        && entry.reading.bytes().all(|b| b.is_ascii_lowercase() || b == b'\'' || b == b' ')
        && (1..=64).contains(&entry.text.chars().count())
        && !entry.text.chars().any(char::is_control)
        && entry.target <= 8
        && (1..=1_000_000).contains(&entry.count)
}

/// What the user directory holds; a missing, damaged or foreign file reads as nothing (typing is never blocked).
pub(crate) fn load(user_dir: &Path) -> Vec<Entry> {
    let user_dir = physical_runtime_path(user_dir);
    let path = user_dir.join(FILE_NAME);
    let Ok(metadata) = fs::symlink_metadata(&path) else {
        return Vec::new();
    };
    if !private_regular_file(&metadata) || metadata.len() > MAX_BYTES {
        return Vec::new();
    }
    let Ok(bytes) = fs::read(&path) else {
        return Vec::new();
    };
    match serde_json::from_slice::<Document>(&bytes) {
        Ok(document) if document.format_version == FORMAT_VERSION && document.entries.len() <= MAX_ENTRIES => {
            document.entries.into_iter().filter(valid).collect()
        }
        _ => Vec::new(),
    }
}

pub(crate) fn save(user_dir: &Path, entries: &[Entry]) -> io::Result<()> {
    let user_dir = physical_runtime_path(user_dir);
    let directory = open_or_create_owned_leaf_directory(&user_dir, 0o700)?;
    validate_owned_directory(&directory, "Personal picks parent")?;
    let path = user_dir.join(FILE_NAME);
    let bytes = serde_json::to_vec(&Document { format_version: FORMAT_VERSION, entries: entries.to_vec() })
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(io::Error::new(io::ErrorKind::StorageFull, "personal picks document is too large"));
    }
    let temporary = user_dir.join(format!(".{FILE_NAME}.{}.tmp", std::process::id()));
    let mut file = private_create(OpenOptions::new().write(true).create_new(true)).open(&temporary)?;
    let result = (|| {
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, &path)?;
        sync_directory(&user_dir)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    pub(crate) fn scratch(label: &str) -> std::path::PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "aim-personal-picks-{label}-{}-{nanos}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        fs::create_dir(&directory).unwrap();
        fs::canonicalize(&directory).unwrap()
    }

    fn entry(reading: &str, text: &str, count: u32) -> Entry {
        Entry { app: "app".into(), reading: reading.into(), text: text.into(), target: 0, count, last_ms: 1 }
    }

    #[test]
    fn saved_picks_come_back_and_bad_files_read_as_nothing() {
        let directory = scratch("roundtrip");
        assert!(load(&directory).is_empty());
        save(&directory, &[entry("nikk", "你看看", 3), entry("zhge", "这个", 1)]).unwrap();
        assert_eq!(load(&directory), vec![entry("nikk", "你看看", 3), entry("zhge", "这个", 1)]);
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            assert_eq!(fs::metadata(directory.join(FILE_NAME)).unwrap().mode() & 0o077, 0, "private file");
        }
        // an entry with a capital letter in the reading is dropped, the rest is kept
        save(&directory, &[entry("Nikk", "你看看", 1), entry("nam", "那么", 2)]).unwrap();
        assert_eq!(load(&directory), vec![entry("nam", "那么", 2)]);
        fs::write(directory.join(FILE_NAME), b"not json").unwrap();
        assert!(load(&directory).is_empty());
        fs::remove_dir_all(directory).unwrap();
    }
}
