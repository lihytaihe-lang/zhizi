//! Persists the built Zhixu index in the user's runtime directory so a new
//! process starts from the file instead of re-parsing the text dictionaries.
//!
//! The file is only an optimization: a missing key mismatch, read error, or
//! damaged body falls back to the on-the-spot build, and the rewrite happens
//! on the background prewarm thread. Invalidation covers the dictionary layer
//! set, the shared and user dictionary fingerprints, the cache format version,
//! and a per-source core build identity, so a cache written by another build
//! or another dictionary state is never trusted.

use super::loader;
use super::Index;
use crate::lexicon_layers::DictionaryLayers;
use crate::secure_directory::{
    open_existing_owned_leaf_directory, physical_runtime_path, private_create,
    private_regular_file, sync_directory, validate_owned_directory,
};
use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, SystemTime};

pub(super) const FILE_NAME: &str = "agent_zhixu_index_cache_v1.bin";
const MAGIC: &[u8; 4] = b"AZXI";
pub(super) const CACHE_VERSION: u32 = 1;
/// The deployed dictionary text stays under 64 MiB; the frozen index with its
/// tries and whole-reading structures needs a wider bound, still far below any
/// allocation a damaged header could trick the reader into.
const MAX_CACHE_BYTES: u64 = 192 * 1024 * 1024;

/// Per-source identity of this core build (see `build.rs`); changes whenever
/// any core source, manifest, or bundled resource changes.
pub(super) fn core_build_id() -> u64 {
    static BUILD_ID: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *BUILD_ID.get_or_init(|| u64::from_str_radix(env!("AGENT_IME_CORE_BUILD_ID"), 16).unwrap())
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

pub(crate) struct Writer {
    bytes: Vec<u8>,
}

impl Writer {
    pub(crate) fn new() -> Self {
        Self { bytes: Vec::new() }
    }

    pub(crate) fn u8(&mut self, value: u8) {
        self.bytes.push(value);
    }

    pub(crate) fn u32(&mut self, value: u32) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    pub(crate) fn u64(&mut self, value: u64) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    pub(crate) fn i64(&mut self, value: i64) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    pub(crate) fn bytes(&mut self, value: &[u8]) {
        self.bytes.extend_from_slice(value);
    }

    pub(crate) fn str(&mut self, value: &str) {
        self.u32(value.len() as u32);
        self.bytes(value.as_bytes());
    }

    pub(crate) fn slice_u32(&mut self, values: &[u32]) {
        for value in values {
            self.u32(*value);
        }
    }

    pub(crate) fn map_u32_ids(&mut self, values: &HashMap<String, Vec<u32>>) {
        self.u64(values.len() as u64);
        for (key, ids) in values {
            self.str(key);
            self.u64(ids.len() as u64);
            self.slice_u32(ids);
        }
    }

    pub(crate) fn string_map(&mut self, values: &HashMap<String, String>) {
        self.u64(values.len() as u64);
        for (key, value) in values {
            self.str(key);
            self.str(value);
        }
    }

    pub(crate) fn string_map_vec(&mut self, values: &HashMap<String, Vec<String>>) {
        self.u64(values.len() as u64);
        for (key, list) in values {
            self.str(key);
            self.u64(list.len() as u64);
            for value in list {
                self.str(value);
            }
        }
    }

    pub(crate) fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

pub(crate) struct Reader<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Self {
        Self { data, at: 0 }
    }

    pub(crate) fn remaining(&self) -> usize {
        self.data.len() - self.at
    }

    pub(crate) fn at_end(&self) -> bool {
        self.at == self.data.len()
    }

    pub(crate) fn invalid(&self, message: &str) -> io::Error {
        io::Error::new(io::ErrorKind::InvalidData, message)
    }

    /// A u64 element count that a damaged file cannot inflate past `bound`.
    pub(crate) fn count(&mut self, bound: usize) -> io::Result<usize> {
        let count = self.u64()? as usize;
        if count > bound {
            return Err(self.invalid("cached index count exceeds the remaining bytes"));
        }
        Ok(count)
    }

    fn take(&mut self, len: usize) -> io::Result<&'a [u8]> {
        if self.remaining() < len {
            return Err(self.invalid("cached index is truncated"));
        }
        let part = &self.data[self.at..self.at + len];
        self.at += len;
        Ok(part)
    }

    pub(crate) fn u8(&mut self) -> io::Result<u8> {
        Ok(self.take(1)?[0])
    }

    pub(crate) fn u32(&mut self) -> io::Result<u32> {
        let bytes = self.take(4)?;
        Ok(u32::from_le_bytes(bytes.try_into().unwrap()))
    }

    pub(crate) fn u64(&mut self) -> io::Result<u64> {
        let bytes = self.take(8)?;
        Ok(u64::from_le_bytes(bytes.try_into().unwrap()))
    }

    pub(crate) fn i64(&mut self) -> io::Result<i64> {
        let bytes = self.take(8)?;
        Ok(i64::from_le_bytes(bytes.try_into().unwrap()))
    }

    pub(crate) fn bytes(&mut self, len: usize) -> io::Result<&'a [u8]> {
        self.take(len)
    }

    pub(crate) fn str(&mut self) -> io::Result<&'a str> {
        let len = self.u32()? as usize;
        let bytes = self.take(len)?;
        std::str::from_utf8(bytes).map_err(|_| self.invalid("cached index string is not UTF-8"))
    }

    pub(crate) fn slice_u32(&mut self, len: usize) -> io::Result<Vec<u32>> {
        let bytes = self.take(len.checked_mul(4).ok_or_else(|| self.invalid("cached index size overflow"))?)?;
        Ok(bytes
            .chunks_exact(4)
            .map(|chunk| u32::from_le_bytes(chunk.try_into().unwrap()))
            .collect())
    }

    pub(crate) fn map_u32_ids(&mut self, id_bound: usize) -> io::Result<HashMap<String, Vec<u32>>> {
        let count = self.count(self.remaining())?;
        let mut values = HashMap::with_capacity(count);
        for _ in 0..count {
            let key = self.str()?.to_string();
            let ids = self.count(self.remaining() / 4)?;
            let ids = self.slice_u32(ids)?;
            if ids.iter().any(|id| *id as usize >= id_bound) {
                return Err(self.invalid("cached index initial id is not an entry"));
            }
            values.insert(key, ids);
        }
        Ok(values)
    }

    pub(crate) fn string_map(&mut self) -> io::Result<HashMap<String, String>> {
        let count = self.count(self.remaining())?;
        let mut values = HashMap::with_capacity(count);
        for _ in 0..count {
            let key = self.str()?.to_string();
            let value = self.str()?.to_string();
            values.insert(key, value);
        }
        Ok(values)
    }

    pub(crate) fn string_map_vec(&mut self) -> io::Result<HashMap<String, Vec<String>>> {
        let count = self.count(self.remaining())?;
        let mut values = HashMap::with_capacity(count);
        for _ in 0..count {
            let key = self.str()?.to_string();
            let list = self.count(self.remaining())?;
            let mut entries = Vec::with_capacity(list);
            for _ in 0..list {
                entries.push(self.str()?.to_string());
            }
            values.insert(key, entries);
        }
        Ok(values)
    }
}

fn write_time(out: &mut Writer, time: SystemTime) {
    let (seconds, nanos) = time
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|since| (since.as_secs(), since.subsec_nanos()))
        .unwrap_or((0, 0));
    out.u64(seconds);
    out.u32(nanos);
}

/// The invalidation key: cache format version, core build identity, the
/// dictionary layer set, and both dictionary fingerprints.
pub(super) fn cache_key(
    layers: &DictionaryLayers,
    shared: &[(String, SystemTime, u64, [u64; 6])],
    user: &[(String, SystemTime, u64, [u64; 6])],
) -> Vec<u8> {
    let mut out = Writer::new();
    out.u32(CACHE_VERSION);
    out.u64(core_build_id());
    let mut disabled: Vec<_> = layers.disabled.iter().cloned().collect();
    disabled.sort();
    out.u64(disabled.len() as u64);
    for layer in disabled {
        out.str(&layer);
    }
    out.u64(layers.packs.len() as u64);
    for (id, path) in &layers.packs {
        out.str(id);
        out.str(&path.to_string_lossy());
    }
    for fingerprint in [shared, user] {
        out.u64(fingerprint.len() as u64);
        for (name, modified, len, identity) in fingerprint {
            out.str(name);
            write_time(&mut out, *modified);
            out.u64(*len);
            for part in identity {
                out.u64(*part);
            }
        }
    }
    out.into_bytes()
}

fn cache_path(user_dir: &Path) -> PathBuf {
    physical_runtime_path(user_dir).join(FILE_NAME)
}

/// Reads the cached index for exactly this key; None always means "build on
/// the spot", never an error the caller must handle.
pub(super) fn load(user_dir: &Path, key: &[u8]) -> Option<Index> {
    let path = cache_path(user_dir);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(_) => return None,
    };
    if !private_regular_file(&metadata) || metadata.len() > MAX_CACHE_BYTES {
        return None;
    }
    let bytes = fs::read(&path).ok()?;
    (|| -> Option<Index> {
        let mut header = Reader::new(&bytes);
        if header.bytes(4).ok()? != MAGIC {
            return None;
        }
        if header.u32().ok()? != CACHE_VERSION || header.u64().ok()? != core_build_id() {
            return None;
        }
        let key_len = header.count(header.remaining()).ok()?;
        if header.bytes(key_len).ok()? != key {
            return None;
        }
        let body_len = header.count(header.remaining().saturating_sub(8)).ok()?;
        let body = header.bytes(body_len).ok()?;
        let checksum = header.u64().ok()?;
        if !header.at_end() || fnv1a64(body) != checksum {
            return None;
        }
        let mut body_reader = Reader::new(body);
        Index::cache_read(&mut body_reader)
            .ok()
            .filter(|_| body_reader.at_end())
    })()
}

pub(super) fn save(user_dir: &Path, key: &[u8], index: &Index) -> io::Result<()> {
    let mut body = Writer::new();
    index.cache_write(&mut body);
    let body = body.into_bytes();
    let mut file = Writer::new();
    file.bytes(MAGIC);
    file.u32(CACHE_VERSION);
    file.u64(core_build_id());
    file.u64(key.len() as u64);
    file.bytes(key);
    file.u64(body.len() as u64);
    let checksum = fnv1a64(&body);
    file.bytes(&body);
    file.u64(checksum);
    let bytes = file.into_bytes();
    if bytes.len() as u64 > MAX_CACHE_BYTES {
        return Err(io::Error::new(io::ErrorKind::StorageFull, "Zhixu index cache is too large"));
    }
    let user_dir = physical_runtime_path(user_dir);
    let directory = open_existing_owned_leaf_directory(&user_dir)?;
    validate_owned_directory(&directory, "Zhixu index cache parent")?;
    static TEMPORARY_COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let temporary = user_dir.join(format!(
        ".{FILE_NAME}.{}.{}.tmp",
        std::process::id(),
        TEMPORARY_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
    ));
    let mut file = private_create(OpenOptions::new().write(true).create_new(true)).open(&temporary)?;
    let result = (|| {
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, user_dir.join(FILE_NAME))?;
        sync_directory(&user_dir)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

struct ScheduledWrite {
    shared: PathBuf,
    user: PathBuf,
    key: Vec<u8>,
    generation: u64,
    /// Weak so a retired generation is never kept alive only by the write it
    /// still owes; the prewarm thread writes through its own strong handle.
    index: std::sync::Weak<Index>,
}

struct WriteState {
    /// One registration per built generation: dictionary churn registers new
    /// entries without losing the writes earlier builds still owe.
    scheduled: Vec<ScheduledWrite>,
    /// Writers between claim and finish; flush waits these out.
    in_flight: usize,
}

static WRITE_STATE: Mutex<WriteState> = Mutex::new(WriteState { scheduled: Vec::new(), in_flight: 0 });
static WRITE_SIGNAL: Condvar = Condvar::new();

/// Registers the cache write this build owes.
pub(super) fn schedule_write(shared: &Path, user: &Path, key: &[u8], index: &Arc<Index>) {
    let Ok(mut state) = WRITE_STATE.lock() else { return };
    state.scheduled.push(ScheduledWrite {
        shared: shared.to_path_buf(),
        user: user.to_path_buf(),
        key: key.to_vec(),
        generation: index.generation,
        index: Arc::downgrade(index),
    });
}

fn perform_write(item: &ScheduledWrite, index: &Index) {
    let result = (|| {
        if !index.whole_ready() {
            return Err(io::Error::new(io::ErrorKind::WouldBlock, "index is not warmed yet"));
        }
        // A dictionary that changed since the build invalidates this cache
        // before it is written; the next build writes the fresh one.
        if loader::current_cache_key(&item.shared, &item.user).as_deref() != Some(&item.key) {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "dictionaries changed during the cache write"));
        }
        save(&item.user, &item.key, index)
    })();
    if let Err(error) = result {
        // No paths or dictionary contents in the log, only the failure kind.
        // A vanished directory is the normal teardown race, not a diagnosis.
        if error.kind() != io::ErrorKind::NotFound {
            eprintln!("zhixu_index_cache_write_failed: {:?}", error.kind());
        }
    }
}

fn lock_writes() -> std::sync::MutexGuard<'static, WriteState> {
    WRITE_STATE.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Runs on the prewarm thread after `warm_whole`: claims one matching
/// registration, if any, and writes the file through the thread's own handle.
pub(super) fn run_scheduled(index: &Arc<Index>) {
    let item = {
        let mut state = lock_writes();
        state
            .scheduled
            .iter()
            .position(|item| item.generation == index.generation)
            .map(|at| {
                state.in_flight += 1;
                state.scheduled.remove(at)
            })
    };
    let Some(item) = item else { return };
    perform_write(&item, index);
    let mut state = lock_writes();
    state.in_flight -= 1;
    WRITE_SIGNAL.notify_all();
}

/// Waits until every scheduled cache write has been written or dropped. True
/// when settled; False on timeout. After the deadline unclaimed registrations
/// are written inline so a missing worker thread cannot lose the cache.
pub(crate) fn flush(timeout: Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    let mut state = lock_writes();
    loop {
        if state.scheduled.is_empty() && state.in_flight == 0 {
            return true;
        }
        let now = std::time::Instant::now();
        if now >= deadline {
            while let Some(item) = state.scheduled.pop() {
                // A retired generation owes nothing: its dictionaries moved on anyway.
                if let Some(index) = item.index.upgrade() {
                    state.in_flight += 1;
                    drop(state);
                    perform_write(&item, &index);
                    state = lock_writes();
                    state.in_flight -= 1;
                }
            }
            return state.scheduled.is_empty() && state.in_flight == 0;
        }
        let waited = WRITE_SIGNAL
            .wait_timeout(state, deadline - now)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state = waited.0;
    }
}

/// The write registry is process-global; tests that schedule or flush serialize on it.
#[cfg(test)]
pub(super) fn tests_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The FFI flush test schedules through the same registry; keep its internals
/// here where the registry is visible.
#[cfg(test)]
pub(crate) fn ffi_test_schedule(root: &Path, dictionary: &Arc<Index>) -> bool {
    let _serialized = tests_lock();
    let shared = root.join("shared");
    let user = root.join("user");
    std::fs::create_dir_all(&shared).unwrap();
    let key = loader::current_cache_key(&shared, &user).unwrap();
    schedule_write(&shared, &user, &key, dictionary);
    user.join(FILE_NAME).is_file()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zhixu::loader::{DictionaryData, Entry};
    use std::sync::atomic::{AtomicU64, Ordering};

    fn fixture_index() -> Index {
        let entries = [
            ("你", "ni", 500, "first"),
            ("泥", "ni", 400, "first"),
            ("好", "hao", 300, "first"),
            ("你好", "ni hao", 900, "second"),
            ("拟好", "ni hao", 800, "second"),
            ("西安", "xi an", 700, "first"),
            ("Redis", "redis", 100, "agent_tech_terms"),
            ("npm", "npm", 100, "agent_english"),
        ]
        .into_iter()
        .map(|(text, reading, weight, layer)| Entry {
            text: text.into(),
            reading: reading.into(),
            weight,
            layer: layer.into(),
        })
        .collect();
        Index::from_data(DictionaryData { entries })
    }

    /// Deterministic empty layer set: the defaults with no user directory.
    fn empty_layers() -> DictionaryLayers {
        crate::lexicon_layers::dictionary_layers(Path::new("/nonexistent-zhixu-index-cache-fixture"))
    }

    fn scratch(label: &str) -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "zhixu-index-cache-{label}-{}-{}-{}",
            std::process::id(),
            SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir_all(&path).unwrap();
        fs::canonicalize(&path).unwrap()
    }

    fn query_groups(index: &Index, mask: u32) -> Vec<String> {
        let _mask = crate::zhixu::fuzzy::pin_mask(mask);
        let mut lines = Vec::new();
        for reading in ["ni", "ni hao", "hao", "xi an", "nihao", "redis", "zz"] {
            lines.push(format!(
                "reading {reading}: {:?}",
                index.words_for_reading(reading).collect::<Vec<_>>()
            ));
            lines.push(format!(
                "whole {reading}: {:?}",
                index.whole_words_for_compact_reading(reading).map(|word| (word.text, word.reading, word.weight, word.syllables, word.reading_hash, word.lexical_score)).collect::<Vec<_>>()
            ));
            lines.push(format!(
                "whole-ranked {reading}: {:?}",
                index.ranked_whole_words_for_compact_reading(reading, 2).map(|word| (word.text, word.weight)).collect::<Vec<_>>()
            ));
            let mut prefixes = Vec::new();
            index.visit_whole_words_for_prefixes(reading, 0, |end, word| prefixes.push((end, word.text)));
            lines.push(format!("prefixes {reading}: {prefixes:?}"));
            let mut plain_prefixes = Vec::new();
            index.visit_words_for_prefixes(reading, |end, text, spaced, weight| {
                plain_prefixes.push((end, text, spaced, weight));
            });
            lines.push(format!("plain-prefixes {reading}: {plain_prefixes:?}"));
        }
        for initials in ["n", "nh", "x", "zz"] {
            lines.push(format!(
                "initials {initials}: {:?}",
                index.words_for_initials(initials).map(|(text, reading, weight)| (text, reading, weight)).collect::<Vec<_>>()
            ));
        }
        for text in ["你", "你好", "西安", "Redis", "无"] {
            lines.push(format!("readings-for {text}: {:?}", index.readings_for_text(text).collect::<Vec<_>>()));
            lines.push(format!("all-readings-for {text}: {:?}", index.all_readings_for_text(text).collect::<Vec<_>>()));
            lines.push(format!("vocabulary {text}: {}", index.vocabulary_word(text, index.readings_for_text(text).next().unwrap_or(""))));
        }
        for (text, reading, weight) in index.owner_words() {
            lines.push(format!("owner {text} {reading} {weight}"));
        }
        let mut compact_paths = Vec::new();
        index.visit_compact_paths(
            &[vec!["ni".into(), "h".into()], vec!["x".into(), "i".into()]],
            |path, text, reading, weight| compact_paths.push((path.to_vec(), text, reading, weight)),
        );
        lines.push(format!("compact-paths: {compact_paths:?}"));
        let mut sorted_project: Vec<_> = index.project.iter().collect();
        sorted_project.sort();
        lines.push(format!("project: {sorted_project:?}"));
        for (key, value) in [(&index.english, "english"), (&index.tech, "tech")] {
            let mut sorted: Vec<_> = key.iter().collect();
            sorted.sort();
            lines.push(format!("{value}: {sorted:?}"));
        }
        lines.push(format!("embedded: {:?}", index.embedded.iter().collect::<Vec<_>>()));
        lines
    }

    fn trie_walk(index: &Index) -> Vec<String> {
        let mut lines = Vec::new();
        fn walk(index: &Index, node: usize, path: String, lines: &mut Vec<String>) {
            for letter in b'a'..=b'z' {
                if let Some(child) = index.trie.letter_child(node, letter) {
                    let child_path = format!("{path}{}", letter as char);
                    lines.push(format!(
                        "{child_path}: boundary={:?} terminal={:?} terminals={:?}",
                        index.trie.boundary_children(child).collect::<Vec<_>>(),
                        index.trie.terminal(child),
                        index.trie.terminals(child),
                    ));
                    walk(index, child, child_path, lines);
                }
            }
        }
        walk(index, 0, String::new(), &mut lines);
        lines
    }

    #[test]
    fn cached_index_matches_the_built_index_field_by_field_and_query_by_query() {
        let built = fixture_index();
        built.warm_whole();
        let key = cache_key(&empty_layers(), &[], &[]);
        let directory = scratch("equal");
        save(&directory, &key, &built).unwrap();
        let loaded = load(&directory, &key).expect("the just-written cache loads");
        assert!(loaded.whole_ready(), "the warmed structures come back warm");
        assert_ne!(loaded.generation, built.generation, "generations are runtime state");
        assert!(built.cache_equals(&loaded));
        for mask in [0, crate::zhixu::fuzzy::FUZZY_MASK_ALL] {
            assert_eq!(query_groups(&built, mask), query_groups(&loaded, mask), "mask {mask}");
        }
        assert_eq!(trie_walk(&built), trie_walk(&loaded));
        let describe_words = |index: &Index| {
            index.entry_words("ni hao").into_iter()
                .map(|word| (word.text, word.weight, word.layers)).collect::<Vec<_>>()
        };
        assert_eq!(describe_words(&built), describe_words(&loaded));
        assert_eq!(built.projection_words().collect::<Vec<_>>(), loaded.projection_words().collect::<Vec<_>>());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn unwarmed_index_round_trips_and_lazy_warms() {
        let built = fixture_index();
        let key = cache_key(&empty_layers(), &[], &[]);
        let directory = scratch("unwarmed");
        save(&directory, &key, &built).unwrap();
        let loaded = load(&directory, &key).expect("the unwarmed cache loads");
        assert!(!loaded.whole_ready());
        assert_eq!(loaded.words_for_reading("ni hao").next().unwrap().0, "你好");
        loaded.warm_whole();
        assert!(loaded.whole_ready());
        built.warm_whole();
        assert!(built.cache_equals(&loaded));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn fallback_dictionary_round_trips_through_the_cache() {
        let built = crate::zhixu::fallback();
        built.warm_whole();
        let key = cache_key(&empty_layers(), &[], &[]);
        let directory = scratch("fallback");
        save(&directory, &key, &built).unwrap();
        let metadata = fs::metadata(directory.join(FILE_NAME)).unwrap();
        println!(
            "INDEX_CACHE_FILE\tentries={}\tbytes={}",
            built.entry_count(),
            metadata.len()
        );
        assert!(metadata.len() > 1_000_000, "the fixture must exercise the real scale");
        assert!(metadata.len() as u64 <= MAX_CACHE_BYTES);
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            assert_eq!(metadata.mode() & 0o777, 0o600, "the cache file is owner-only");
        }
        let loaded = load(&directory, &key).expect("the fallback-sized cache loads");
        assert!(loaded.whole_ready());
        assert!(built.cache_equals(&loaded));
        assert_eq!(query_groups(&built, 0), query_groups(&loaded, 0));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn mismatches_truncation_corruption_and_permissions_all_miss() {
        let built = fixture_index();
        built.warm_whole();
        let key = cache_key(&empty_layers(), &[], &[]);
        let directory = scratch("invalid");
        save(&directory, &key, &built).unwrap();
        let path = directory.join(FILE_NAME);
        let bytes = fs::read(&path).unwrap();
        let other_key = cache_key(
            &empty_layers(),
            &[("other.dict.yaml".into(), SystemTime::UNIX_EPOCH, 1, [7; 6])],
            &[],
        );
        assert_ne!(other_key, key);
        assert!(load(&directory, &other_key).is_none(), "a different dictionary key is a miss");
        // Truncated at every plausible boundary.
        for cut in [0, 4, 12, 24, bytes.len() / 2, bytes.len() - 1] {
            fs::write(&path, &bytes[..cut]).unwrap();
            assert!(load(&directory, &key).is_none(), "truncation at {cut} must miss");
        }
        fs::write(&path, &bytes).unwrap();
        // A flipped body byte breaks the checksum; a flipped key byte breaks the key.
        let mut flipped = bytes.clone();
        let middle = flipped.len() / 2;
        flipped[middle] ^= 0x40;
        fs::write(&path, &flipped).unwrap();
        assert!(load(&directory, &key).is_none(), "a corrupted body must miss");
        let mut flipped_header = bytes.clone();
        flipped_header[40] ^= 0x01;
        fs::write(&path, &flipped_header).unwrap();
        assert!(load(&directory, &key).is_none(), "a corrupted key must miss");
        fs::write(&path, &bytes).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
            assert!(load(&directory, &key).is_none(), "a group-readable cache file must miss");
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        }
        // A newer format version is refused even when the rest matches.
        let mut newer = bytes.clone();
        newer[4..8].copy_from_slice(&(CACHE_VERSION + 1).to_le_bytes());
        fs::write(&path, &newer).unwrap();
        assert!(load(&directory, &key).is_none(), "a newer format version must miss");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn schedule_write_and_flush_settle_the_file() {
        let _serialized = tests_lock();
        let index = Arc::new(fixture_index());
        index.warm_whole();
        let directory = scratch("flush");
        let shared = directory.join("shared");
        let user = directory.join("user");
        fs::create_dir_all(&shared).unwrap();
        fs::create_dir_all(&user).unwrap();
        fs::write(shared.join("agent_pinyin.dict.yaml"), "---\nname: agent_pinyin\n...\n你\tni\t100\n").unwrap();
        let key = loader::current_cache_key(&shared, &user).unwrap();
        assert!(flush(Duration::from_millis(0)), "nothing scheduled settles immediately");
        schedule_write(&shared, &user, &key, &index);
        assert!(flush(Duration::from_millis(100)), "flush settles the scheduled write");
        assert!(user.join(FILE_NAME).is_file(), "flush wrote the cache file");
        assert_eq!(load(&user, &key).map(|loaded| loaded.words_for_reading("ni").count()), Some(2));
        // A dictionary change between build and write cancels the write.
        fs::remove_file(user.join(FILE_NAME)).unwrap();
        schedule_write(&shared, &user, &key, &index);
        fs::write(shared.join("agent_pinyin.dict.yaml"), "---\nname: agent_pinyin\n...\n泥\tni\t100\n").unwrap();
        assert!(flush(Duration::from_millis(100)), "flush settles even when the write is dropped");
        assert!(!user.join(FILE_NAME).exists(), "no stale cache file was written");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn run_scheduled_writes_only_the_matching_generation() {
        let _serialized = tests_lock();
        let index = Arc::new(fixture_index());
        index.warm_whole();
        let directory = scratch("generation");
        let key = cache_key(&empty_layers(), &[], &[]);
        schedule_write(Path::new("/nonexistent-shared"), &directory, &key, &index);
        run_scheduled(&Arc::new(fixture_index()));
        assert!(!directory.join(FILE_NAME).exists(), "another generation's worker does not write");
        run_scheduled(&index);
        assert!(directory.join(FILE_NAME).is_file());
        // A destination that cannot hold a safe private file: the failure is contained.
        schedule_write(Path::new("/nonexistent-shared"), Path::new("/"), &key, &index);
        run_scheduled(&index);
        assert!(flush(Duration::from_millis(0)));
        fs::remove_dir_all(directory).unwrap();
    }
}
