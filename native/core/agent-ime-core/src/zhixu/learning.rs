//! Local, bounded learning facts shared by Zhixu's personal dictionary and ranking.

use crate::secure_directory::{
    open_or_create_owned_leaf_directory, physical_runtime_path, private_create,
    private_regular_file, sync_directory, validate_owned_directory,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::{Arc, OnceLock};

#[cfg(unix)]
#[path = "learning_lock_unix.rs"]
mod learning_lock;
#[cfg(windows)]
#[path = "learning_lock_windows.rs"]
mod learning_lock;

pub const FILE_NAME: &str = "agent_zhixu_learning_v1.json";
pub const MAX_ENTRIES: usize = 4096;
pub const MAX_BYTES: u64 = 1024 * 1024;
pub const MAX_AGE_MS: u64 = 45 * 24 * 60 * 60 * 1000;
const FORMAT_VERSION: u32 = 1;
const MAX_COUNT: u32 = 1_000_000;
const MAX_IMPORT_BYTES: usize = 16 * 1024 * 1024;
const MAX_DELIVERY_IDS: usize = 2048;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u32)]
pub enum LearningMode {
    #[default]
    Rime = 0,
    Zhixu = 1,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Entry {
    pub reading: String,
    pub text: String,
    pub count: u32,
    pub last_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rime: Option<RimeWeight>,
}

/// librime 1.16.1 UserDbValue. Tick counts selections, not wall-clock time.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RimeWeight {
    pub commits: u32,
    pub dynamic: f64,
    pub tick: u64,
    pub updates: u32,
}

impl RimeWeight {
    fn decayed(&self, tick: u64) -> f64 {
        self.dynamic * (-(tick.saturating_sub(self.tick) as f64) / 200.0).exp()
    }

    fn score(&self, tick: u64) -> f64 {
        let tick = tick.saturating_add(1);
        let dynamic = self.decayed(tick);
        let frequency = self.commits as f64 / tick as f64
            * (1.0 - (-(tick as f64) / 10000.0).exp()).powi(10);
        let scale = 200.50041666649332;
        let exponent = 2.0 * dynamic / scale * std::f64::consts::LN_2;
        // Keep extreme but finite imported weights sortable without exp overflow.
        if exponent > 700.0 {
            return if frequency < 1.0 {
                exponent + ((1.0 - frequency) / 3.0).ln()
            } else if frequency == 1.0 { 0.0 } else { f64::EPSILON.ln() };
        }
        let probability = if dynamic < 20.0 {
            frequency + (0.5 - frequency) * dynamic / scale
        } else {
            frequency + (1.0 - frequency) * ((2.0 * dynamic / scale).exp2() - 1.0) / 3.0
        };
        probability.max(f64::EPSILON).ln()
    }
}

impl Entry {
    pub(crate) fn is_learned(&self) -> bool {
        self.rime.as_ref().is_none_or(|weight| weight.updates != 0)
    }

    pub(crate) fn score(&self, tick: u64) -> f64 {
        self.rime.as_ref().map_or(self.count as f64, |weight| weight.score(tick))
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Choice {
    pub app: String,
    pub raw: String,
    pub reading: String,
    pub text: String,
    pub target: usize,
    pub count: u32,
    pub last_ms: u64,
    pub alternatives: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LearningStore {
    // Shared with readers: callers on every key take a reference-counted
    // snapshot instead of copying every learned word.
    entries: Arc<Vec<Entry>>,
    choices: Vec<Choice>,
    pub(crate) migrated: bool,
    pub(crate) rime_tick: u64,
    pub(crate) owner_seeded: bool,
    pub(crate) legacy_choices_imported: bool,
    delivery_ids: VecDeque<u64>,
    baseline: Option<Box<Document>>,
    reset_entries: HashSet<(String, String)>,
    forgotten_words: HashSet<(String, String)>,
    reset_choices: HashSet<(String, String, String)>,
    entry_overflow: HashMap<(String, String), u32>,
    choice_overflow: HashMap<(String, String, String), u32>,
    pending_rime_ticks: u64,
    pending_rime: HashMap<(String, String), RimeWeight>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Document {
    format_version: u32,
    #[serde(default)]
    migrated: bool,
    #[serde(default)]
    rime_tick: u64,
    entries: Vec<Entry>,
    #[serde(default)]
    choices: Vec<Choice>,
    #[serde(default)]
    owner_seeded: bool,
    #[serde(default)]
    legacy_choices_imported: bool,
    #[serde(default)]
    delivery_ids: VecDeque<u64>,
}

/// The caller supplies syllable boundaries from the selected candidate, not a
/// guessed segmentation of the keys (which may contain abbreviations or typos).
pub fn normalize_reading(reading: &str) -> Option<String> {
    if reading.len() > 512 {
        return None;
    }
    static SYLLABLES: OnceLock<HashSet<&'static str>> = OnceLock::new();
    let syllables =
        SYLLABLES.get_or_init(|| crate::mixer::PINYIN_SYLLABLES.split_whitespace().collect());
    let reading = reading.to_lowercase().replace("u:", "v").replace('ü', "v");
    let parts: Vec<_> = reading
        .split(|c: char| c == '\'' || c.is_ascii_whitespace())
        .filter(|part| !part.is_empty())
        .collect();
    if parts.is_empty() || parts.len() > 64 || parts.iter().any(|part| !syllables.contains(part)) {
        return None;
    }
    Some(parts.join(" "))
}

fn valid_text(text: &str) -> bool {
    (1..=64).contains(&text.chars().count())
        && text.chars().all(|c| {
            ('\u{3400}'..='\u{9fff}').contains(&c) || ('\u{f900}'..='\u{faff}').contains(&c)
        })
}

fn valid_word(reading: &str, text: &str) -> bool {
    valid_text(text)
        && normalize_reading(reading).as_deref() == Some(reading)
        && reading.split(' ').count() == text.chars().count()
}

fn valid_entry(entry: &Entry) -> bool {
    valid_word(&entry.reading, &entry.text) && (1..=MAX_COUNT).contains(&entry.count)
        && entry.rime.as_ref().is_none_or(|weight| weight.dynamic.is_finite()
            && weight.dynamic >= 0.0 && weight.dynamic <= MAX_COUNT as f64
            && weight.commits > 0 && weight.updates <= weight.commits && weight.tick < u64::MAX)
}

fn valid_choice(choice: &Choice) -> bool {
    valid_word(&choice.reading, &choice.text)
        && (1..=128).contains(&choice.app.len())
        && !choice.app.chars().any(char::is_control)
        && (1..=128).contains(&choice.raw.len())
        && choice
            .raw
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b == b'\'' || b == b' ')
        && choice.target <= 8
        && (1..=MAX_COUNT).contains(&choice.count)
        && choice.alternatives.len() <= 9
        && choice
            .alternatives
            .iter()
            .all(|text| valid_text(text) && text.chars().count() == choice.text.chars().count())
}

fn same_entry(a: &Entry, b: &Entry) -> bool {
    a.reading == b.reading && a.text == b.text
}

fn same_choice(a: &Choice, b: &Choice) -> bool {
    a.app == b.app && a.raw == b.raw && a.text == b.text
}

fn entry_key(entry: &Entry) -> (String, String) {
    (entry.reading.clone(), entry.text.clone())
}

fn choice_key(choice: &Choice) -> (String, String, String) {
    (choice.app.clone(), choice.raw.clone(), choice.text.clone())
}

impl LearningStore {
    /// Missing, foreign, oversized or damaged files never block typing.
    pub fn load(user_dir: &Path, now: u64) -> Self {
        let mut store = Self::read_current(user_dir).unwrap_or_default();
        store.baseline = Some(Box::new(store.document()));
        store.expire(now);
        store.limit();
        store
    }

    /// Distinguishes a missing library from invalid existing data during migration.
    pub(crate) fn existing_file_valid(user_dir: &Path) -> io::Result<bool> {
        match fs::symlink_metadata(physical_runtime_path(user_dir).join(FILE_NAME)) {
            Ok(_) => Self::read_current_validated(user_dir, true).map(|_| true),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error),
        }
    }

    fn read_current(user_dir: &Path) -> io::Result<Self> {
        Self::read_current_validated(user_dir, false)
    }

    fn read_current_validated(user_dir: &Path, strict: bool) -> io::Result<Self> {
        let path = physical_runtime_path(user_dir).join(FILE_NAME);
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound && !strict => return Ok(Self::default()),
            Err(error) => return Err(error),
        };
        if !private_regular_file(&metadata) || metadata.len() > MAX_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unsafe or oversized learning file",
            ));
        }
        let mut bytes = Vec::new();
        fs::File::open(&path)?
            .take(MAX_BYTES + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "oversized learning file",
            ));
        }
        let document = serde_json::from_slice::<Document>(&bytes)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "damaged learning file"))?;
        if document.format_version != FORMAT_VERSION
            || document.rime_tick == u64::MAX
            || document.entries.len() > MAX_ENTRIES
            || document.choices.len() > MAX_ENTRIES
            || document.delivery_ids.len() > MAX_DELIVERY_IDS
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unsupported or unbounded learning file",
            ));
        }
        if strict && (document.entries.iter().any(|entry| !valid_entry(entry)
            || entry.rime.as_ref().is_some_and(|weight| weight.tick > document.rime_tick))
            || document.choices.iter().any(|choice| !valid_choice(choice)))
        {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid learning records"));
        }
        let mut store = Self {
            migrated: document.migrated,
            rime_tick: document.rime_tick,
            entries: Arc::new(document.entries.into_iter().filter(|entry| valid_entry(entry)
                && entry.rime.as_ref().is_none_or(|weight| weight.tick <= document.rime_tick)).collect()),
            choices: document.choices.into_iter().filter(valid_choice).collect(),
            owner_seeded: document.owner_seeded,
            legacy_choices_imported: document.legacy_choices_imported,
            delivery_ids: document
                .delivery_ids
                .into_iter()
                .filter(|id| *id != 0)
                .collect(),
            ..Self::default()
        };
        store.limit();
        let mut seen = HashSet::new();
        store.delivery_ids.retain(|id| seen.insert(*id));
        Ok(store)
    }

    fn document(&self) -> Document {
        Document {
            format_version: FORMAT_VERSION,
            migrated: self.migrated,
            rime_tick: self.rime_tick,
            entries: self.entries.to_vec(),
            choices: self.choices.clone(),
            owner_seeded: self.owner_seeded,
            legacy_choices_imported: self.legacy_choices_imported,
            delivery_ids: self.delivery_ids.clone(),
        }
    }

    /// Merge only changes since the last read/commit while holding a stable file
    /// lock. Contention has a zero-wait timeout; errors retain the pending delta.
    pub fn save(&mut self, user_dir: &Path) -> io::Result<()> {
        self.save_before_replace(user_dir, || Ok(()))
    }

    /// First migration must not merge or replace learning created by another writer.
    pub(crate) fn save_new(&mut self, user_dir: &Path) -> io::Result<()> {
        self.save_inner(user_dir, || Ok(()), true)
    }

    fn save_before_replace(
        &mut self,
        user_dir: &Path,
        before_replace: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<()> {
        self.save_inner(user_dir, before_replace, false)
    }

    fn save_inner(
        &mut self,
        user_dir: &Path,
        before_replace: impl FnOnce() -> io::Result<()>,
        require_new: bool,
    ) -> io::Result<()> {
        let user_dir = physical_runtime_path(user_dir);
        let directory = open_or_create_owned_leaf_directory(&user_dir, 0o700)?;
        validate_owned_directory(&directory, "Zhixu learning parent")?;
        let _lock = learning_lock::LearningLock::acquire(&user_dir)?;
        if require_new {
            match fs::symlink_metadata(user_dir.join(FILE_NAME)) {
                Ok(_) => return Err(io::Error::new(io::ErrorKind::AlreadyExists,
                    "migration requires a new learning file")),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {},
                Err(error) => return Err(error),
            }
        }
        let current = Self::read_current(&user_dir).unwrap_or_else(|error| {
            // Do not log paths, learned text, or parser errors containing user data.
            eprintln!("zhixu_learning_merge_read_as_empty: {:?}", error.kind());
            Self::default()
        });
        let mut merged = self.merge(current);
        let bytes = serde_json::to_vec(&merged.document())
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        if bytes.len() as u64 > MAX_BYTES
            || merged.entries.len() > MAX_ENTRIES
            || merged.choices.len() > MAX_ENTRIES
        {
            return Err(io::Error::new(
                io::ErrorKind::StorageFull,
                "Zhixu learning document is too large",
            ));
        }
        let temporary = user_dir.join(format!(".{FILE_NAME}.{}.tmp", std::process::id()));
        let mut file =
            private_create(OpenOptions::new().write(true).create_new(true)).open(&temporary)?;
        let result = (|| {
            file.write_all(&bytes)?;
            file.sync_all()?;
            drop(file);
            before_replace()?;
            fs::rename(&temporary, user_dir.join(FILE_NAME))?;
            // Rename committed even if directory sync later fails. Advancing the
            // baseline here prevents retrying the same counts twice in that case.
            merged.baseline = Some(Box::new(merged.document()));
            *self = merged;
            sync_directory(&user_dir)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }

    fn merge(&self, mut current: Self) -> Self {
        let baseline = self.baseline.as_deref();
        let old_entries = baseline.map_or(&[][..], |base| base.entries.as_slice());
        let old_choices = baseline.map_or(&[][..], |base| base.choices.as_slice());
        // Append this process's selection batch after the disk clock. Importing
        // a baseline is not a selection, including concurrent first migrations.
        let start_tick = self.rime_tick.saturating_sub(self.pending_rime_ticks);
        current.migrated |= self.migrated;
        current.rime_tick = current.rime_tick.max(start_tick)
            .saturating_add(self.pending_rime_ticks).min(u64::MAX - 1);
        let mut entry_baseline = HashMap::with_capacity(old_entries.len());
        for old in old_entries {
            entry_baseline.entry(entry_key(old)).or_insert(old);
        }
        let local_entries: HashSet<_> = self.entries.iter().map(entry_key).collect();
        Arc::make_mut(&mut current.entries).retain(|entry| {
            let key = entry_key(entry);
            !self.forgotten_words.contains(&key) && entry_baseline.get(&key).is_none_or(|old| {
                (!self.reset_entries.contains(&key) && local_entries.contains(&key))
                    || entry.last_ms > old.last_ms
                    || entry.count > old.count
            })
        });
        let mut entry_positions = HashMap::with_capacity(current.entries.len());
        for (position, entry) in current.entries.iter().enumerate() {
            entry_positions.entry(entry_key(entry)).or_insert(position);
        }
        for entry in self.entries.iter() {
            let key = entry_key(entry);
            let old = entry_baseline
                .get(&key)
                .copied()
                .filter(|_| !self.reset_entries.contains(&key) && !self.forgotten_words.contains(&key));
            let pending = self.pending_rime.get(&key);
            let delta = if entry.rime.is_some() && (entry_positions.contains_key(&key)
                || old.is_some_and(|entry| entry.rime.is_some())) {
                pending.map_or(0, |weight| weight.commits).min(MAX_COUNT)
            } else {
                entry.count.saturating_sub(old.map_or(0, |old| old.count))
                    .saturating_add(self.entry_overflow.get(&key).copied().unwrap_or(0)).min(MAX_COUNT)
            };
            if delta == 0 && old == Some(entry) { continue; }
            if let Some(&position) = entry_positions.get(&key) {
                let existing = &mut Arc::make_mut(&mut current.entries)[position];
                existing.count = existing.count.saturating_add(delta).min(MAX_COUNT);
                existing.last_ms = existing.last_ms.max(entry.last_ms);
                if let Some(pending) = pending {
                    let disk = existing.rime.as_ref();
                    existing.rime = Some(RimeWeight {
                        commits: disk.map_or(0, |weight| weight.commits).saturating_add(pending.commits),
                        dynamic: disk.map_or(0.0, |weight| weight.decayed(current.rime_tick))
                            + pending.decayed(self.rime_tick),
                        tick: current.rime_tick,
                        updates: disk.map_or(0, |weight| weight.updates).saturating_add(pending.updates),
                    });
                } else if existing.rime.is_none() {
                    existing.rime = entry.rime.clone();
                }
            } else if delta != 0 {
                let mut entry = entry.clone();
                entry.count = delta;
                if let Some(pending) = pending {
                    let contribution = pending.decayed(self.rime_tick);
                    let previous = if old.is_some_and(|entry| entry.rime.is_some()) {
                        // A removed or damaged disk row only recovers our new events.
                        0.0
                    } else {
                        entry.rime.as_ref().map_or(0.0, |weight|
                            (weight.decayed(self.rime_tick) - contribution).max(0.0))
                            * (-(current.rime_tick.saturating_sub(self.rime_tick) as f64) / 200.0).exp()
                    };
                    let imported = old.is_none();
                    entry.rime = Some(RimeWeight {
                        commits: if imported { entry.rime.as_ref().map_or(delta, |weight| weight.commits) } else { pending.commits },
                        dynamic: previous + contribution,
                        tick: current.rime_tick,
                        updates: pending.updates,
                    });
                }
                entry_positions.insert(key, current.entries.len());
                Arc::make_mut(&mut current.entries).push(entry);
            }
        }
        let mut choice_baseline = HashMap::with_capacity(old_choices.len());
        for old in old_choices {
            choice_baseline.entry(choice_key(old)).or_insert(old);
        }
        let local_choices: HashSet<_> = self.choices.iter().map(choice_key).collect();
        current.choices.retain(|choice| {
            let key = choice_key(choice);
            !self.forgotten_words.contains(&(choice.reading.clone(), choice.text.clone()))
                && choice_baseline.get(&key).is_none_or(|old| {
                (!self.reset_choices.contains(&key) && local_choices.contains(&key))
                    || choice.last_ms > old.last_ms
                    || choice.count > old.count
            })
        });
        let mut choice_positions = HashMap::with_capacity(current.choices.len());
        for (position, choice) in current.choices.iter().enumerate() {
            choice_positions
                .entry(choice_key(choice))
                .or_insert(position);
        }
        for choice in &self.choices {
            let key = choice_key(choice);
            let old = choice_baseline
                .get(&key)
                .copied()
                .filter(|_| !self.reset_choices.contains(&key)
                    && !self.forgotten_words.contains(&(choice.reading.clone(), choice.text.clone())));
            let delta = choice
                .count
                .saturating_sub(old.map_or(0, |old| old.count))
                .saturating_add(self.choice_overflow.get(&key).copied().unwrap_or(0))
                .min(MAX_COUNT);
            if delta == 0 && old == Some(choice) {
                continue;
            }
            if let Some(&position) = choice_positions.get(&key) {
                let existing = &mut current.choices[position];
                existing.count = existing.count.saturating_add(delta).min(MAX_COUNT);
                existing.last_ms = existing.last_ms.max(choice.last_ms);
                existing.target = existing.target.min(choice.target);
                if existing.count >= 3 {
                    existing.target = 0;
                }
                existing
                    .alternatives
                    .retain(|text| choice.alternatives.contains(text));
            } else if delta != 0 {
                let mut choice = choice.clone();
                choice.count = delta;
                choice_positions.insert(key, current.choices.len());
                current.choices.push(choice);
            }
        }
        if self.owner_seeded && baseline.is_none_or(|base| !base.owner_seeded) {
            current.owner_seeded = true;
        }
        if self.legacy_choices_imported && baseline.is_none_or(|base| !base.legacy_choices_imported)
        {
            current.legacy_choices_imported = true;
        }
        let old_ids: HashSet<_> = baseline
            .into_iter()
            .flat_map(|base| base.delivery_ids.iter().copied())
            .collect();
        let mut current_ids: HashSet<_> = current.delivery_ids.iter().copied().collect();
        for &id in &self.delivery_ids {
            if id != 0 && !old_ids.contains(&id) && current_ids.insert(id) {
                current.delivery_ids.push_back(id);
            }
        }
        while current.delivery_ids.len() > MAX_DELIVERY_IDS {
            current.delivery_ids.pop_front();
        }
        current.limit();
        current
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// The same entries as a cheap shared snapshot.
    pub(crate) fn shared_entries(&self) -> Arc<Vec<Entry>> {
        self.entries.clone()
    }
    pub fn choices(&self) -> &[Choice] {
        &self.choices
    }

    pub(crate) fn has_delivery(&self, id: u64) -> bool {
        self.delivery_ids.contains(&id)
    }

    /// Facts and their event claims are persisted in the same atomic document.
    pub(crate) fn remember_deliveries(&mut self, ids: &[u64]) {
        for &id in ids {
            if id != 0 && !self.delivery_ids.contains(&id) {
                self.delivery_ids.push_back(id);
            }
        }
        while self.delivery_ids.len() > MAX_DELIVERY_IDS {
            self.delivery_ids.pop_front();
        }
        self.limit();
    }

    pub fn record_choice(&mut self, mut choice: Choice) -> bool {
        let Some(reading) = normalize_reading(&choice.reading) else {
            return false;
        };
        choice.reading = reading;
        choice.alternatives.retain(|text| {
            valid_text(text)
                && text.chars().count() == choice.text.chars().count()
                && text != &choice.text
        });
        choice.alternatives.sort();
        choice.alternatives.dedup();
        choice.alternatives.truncate(9);
        if !valid_choice(&choice) {
            return false;
        }
        self.expire(choice.last_ms);
        if let Some(existing) = self.choices.iter_mut().find(|entry| {
            entry.app == choice.app && entry.raw == choice.raw && entry.text == choice.text
        }) {
            let overflow = existing
                .count
                .saturating_add(choice.count)
                .saturating_sub(MAX_COUNT);
            if overflow != 0 {
                let pending = self
                    .choice_overflow
                    .entry(choice_key(existing))
                    .or_default();
                *pending = pending.saturating_add(overflow).min(MAX_COUNT);
            }
            existing.count = existing.count.saturating_add(choice.count).min(MAX_COUNT);
            existing.target = existing.target.min(choice.target);
            if existing.count >= 3 {
                existing.target = 0;
            }
            existing.last_ms = existing.last_ms.max(choice.last_ms);
            // Only these challengers were present for every counted choice.
            // A newly seen challenger must not inherit earlier pair evidence.
            existing
                .alternatives
                .retain(|text| choice.alternatives.contains(text));
        } else {
            if self
                .baseline
                .as_ref()
                .is_some_and(|base| base.choices.iter().any(|old| same_choice(old, &choice)))
            {
                self.reset_choices.insert(choice_key(&choice));
            }
            if choice.count >= 3 {
                choice.target = 0;
            }
            self.choices.push(choice);
        }
        self.limit();
        true
    }

    pub fn record(&mut self, reading: &str, text: &str, now: u64) -> bool {
        let Some(reading) = normalize_reading(reading) else {
            return false;
        };
        if !valid_word(&reading, text) {
            return false;
        }
        self.expire(now);
        if self.migrated {
            if self.rime_tick == u64::MAX - 1 { return false; }
            self.rime_tick += 1;
            self.pending_rime_ticks += 1;
            let pending = self.pending_rime.entry((reading.clone(), text.into())).or_insert(RimeWeight {
                commits: 0, dynamic: 0.0, tick: self.rime_tick, updates: 0,
            });
            pending.dynamic = 1.0 + pending.decayed(self.rime_tick);
            pending.commits = pending.commits.saturating_add(1);
            pending.tick = self.rime_tick;
            pending.updates = pending.updates.saturating_add(1);
        }
        if let Some(entry) = Arc::make_mut(&mut self.entries)
            .iter_mut()
            .find(|entry| entry.reading == reading && entry.text == text)
        {
            if entry.count == MAX_COUNT {
                let pending = self.entry_overflow.entry(entry_key(entry)).or_default();
                *pending = pending.saturating_add(1).min(MAX_COUNT);
            }
            if self.migrated {
                let weight = entry.rime.get_or_insert(RimeWeight {
                    commits: entry.count, dynamic: 0.0, tick: self.rime_tick, updates: 0,
                });
                weight.dynamic = 1.0 + weight.decayed(self.rime_tick);
                weight.commits = weight.commits.saturating_add(1);
                weight.tick = self.rime_tick;
                weight.updates = weight.updates.saturating_add(1);
            }
            entry.count = entry.count.saturating_add(1).min(MAX_COUNT);
            entry.last_ms = entry.last_ms.max(now);
        } else {
            if self.baseline.as_ref().is_some_and(|base| {
                base.entries
                    .iter()
                    .any(|old| old.reading == reading && old.text == text)
            }) {
                self.reset_entries.insert((reading.clone(), text.into()));
            }
            Arc::make_mut(&mut self.entries).push(Entry {
                reading,
                text: text.into(),
                count: 1,
                last_ms: now,
                rime: self.migrated.then_some(RimeWeight {
                    commits: 1, dynamic: 1.0, tick: self.rime_tick, updates: 1,
                }),
            });
        }
        self.limit();
        true
    }

    /// Explicit deletion removes both word weights and positional choices for
    /// this reading. The tombstone is applied to the latest disk rows under save's
    /// lock, including rows added since this process last loaded the library.
    pub fn forget(&mut self, reading: &str, text: &str) -> bool {
        let Some(reading) = normalize_reading(reading) else { return false; };
        if !valid_word(&reading, text) { return false; }
        self.forgotten_words.insert((reading.clone(), text.into()));
        Arc::make_mut(&mut self.entries).retain(|entry| entry.reading != reading || entry.text != text);
        self.choices.retain(|choice| choice.reading != reading || choice.text != text);
        self.prune_overflow();
        true
    }

    pub fn expire(&mut self, now: u64) {
        if self.entries.iter().any(|entry| now.saturating_sub(entry.last_ms) > MAX_AGE_MS) {
            Arc::make_mut(&mut self.entries)
                .retain(|entry| now.saturating_sub(entry.last_ms) <= MAX_AGE_MS);
        }
        self.choices
            .retain(|entry| now.saturating_sub(entry.last_ms) <= MAX_AGE_MS);
        self.prune_overflow();
    }

    fn prune_overflow(&mut self) {
        // Saturated facts still have new increments, even though their visible
        // count no longer changes. Keep this retry state bounded by live facts.
        if !self.entry_overflow.is_empty() {
            let keys: HashSet<_> = self.entries.iter().map(entry_key).collect();
            self.entry_overflow.retain(|key, _| keys.contains(key));
        }
        if !self.pending_rime.is_empty() {
            let keys: HashSet<_> = self.entries.iter().map(entry_key).collect();
            self.pending_rime.retain(|key, _| keys.contains(key));
        }
        if !self.choice_overflow.is_empty() {
            let keys: HashSet<_> = self.choices.iter().map(choice_key).collect();
            self.choice_overflow.retain(|key, _| keys.contains(key));
        }
    }

    fn limit(&mut self) {
        let entries = Arc::make_mut(&mut self.entries);
        entries.sort_by(|a, b| {
            b.last_ms
                .cmp(&a.last_ms)
                .then_with(|| b.count.cmp(&a.count))
                .then_with(|| a.text.cmp(&b.text))
                .then_with(|| a.reading.cmp(&b.reading))
        });
        let mut words = HashSet::new();
        entries.retain(|entry| words.insert((entry.reading.clone(), entry.text.clone())));
        entries.truncate(MAX_ENTRIES);
        self.choices.sort_by(|a, b| {
            b.last_ms
                .cmp(&a.last_ms)
                .then_with(|| b.count.cmp(&a.count))
                .then_with(|| a.app.cmp(&b.app))
                .then_with(|| a.raw.cmp(&b.raw))
                .then_with(|| a.text.cmp(&b.text))
        });
        let mut choices = HashSet::new();
        self.choices.retain(|entry| {
            choices.insert((entry.app.clone(), entry.raw.clone(), entry.text.clone()))
        });
        self.choices.truncate(MAX_ENTRIES);
        // Upper bounds include JSON punctuation, escaped app names and numeric fields.
        let word_bytes = |entry: &Entry| entry.reading.len() + entry.text.len() + 100 + if entry.rime.is_some() { 120 } else { 0 };
        let choice_bytes = |entry: &Choice| {
            entry.app.len() * 6
                + entry.raw.len()
                + entry.reading.len()
                + entry.text.len()
                + entry
                    .alternatives
                    .iter()
                    .map(|text| text.len() + 3)
                    .sum::<usize>()
                + 200
        };
        let mut bytes = 200
            + self.delivery_ids.len() * 21
            + self.entries.iter().map(word_bytes).sum::<usize>()
            + self.choices.iter().map(choice_bytes).sum::<usize>();
        while bytes > MAX_BYTES as usize {
            let word_last = self.entries.last().map(|entry| entry.last_ms);
            let choice_last = self.choices.last().map(|entry| entry.last_ms);
            if word_last.is_some() && (choice_last.is_none() || word_last <= choice_last) {
                bytes -= word_bytes(&Arc::make_mut(&mut self.entries).pop().unwrap());
            } else if let Some(choice) = self.choices.pop() {
                bytes -= choice_bytes(&choice);
            } else {
                break;
            }
        }
        self.prune_overflow();
    }

    /// Backup rows retain c/d/t and the database logical clock. Ordinary three-
    /// column exports cannot reconstruct recency; they still retain Rime's live order.
    pub fn import_rime_export(&mut self, text: &str, now: u64) -> io::Result<usize> {
        if text.len() > MAX_IMPORT_BYTES {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "Rime export is too large"));
        }
        let header_tick = text.lines().find_map(|line| line.strip_prefix("#@/tick\t"))
            .map(|tick| tick.trim().parse::<u64>())
            .transpose().map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid Rime clock"))?;
        if header_tick == Some(u64::MAX) {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid Rime clock"));
        }
        let mut imported_entries = Vec::new();
        for line in text.lines().filter(|line| !line.is_empty() && !line.starts_with('#')) {
            if line.len() > 2048 { continue; }
            let columns: Vec<_> = line.split('\t').collect();
            if columns.len() != 3 { continue; }
            let fields: HashMap<_, _> = columns[2].split_whitespace().filter_map(|field| field.split_once('=')).collect();
            let (text, reading, count) = if let Some(count) = fields.get("c") {
                (columns[1], columns[0], *count)
            } else { (columns[0], columns[1], columns[2].trim()) };
            let Some(reading) = normalize_reading(reading) else { continue; };
            let Ok(count) = count.parse::<u32>() else { continue; };
            if count == 0 || !valid_word(&reading, text) { continue; }
            let (dynamic, tick) = if fields.contains_key("c") {
                let (Some(dynamic), Some(tick)) = (fields.get("d"), fields.get("t")) else { continue; };
                let (Ok(dynamic), Ok(tick)) = (dynamic.parse::<f64>(), tick.parse::<u64>()) else { continue; };
                (dynamic, tick)
            } else { (0.0, 0) };
            if header_tick.is_some_and(|clock| tick > clock) { continue; }
            let entry = Entry { reading, text: text.into(), count: count.min(MAX_COUNT), last_ms: now,
                rime: Some(RimeWeight { commits: count, dynamic, tick, updates: 0 }) };
            if valid_entry(&entry) { imported_entries.push(entry); }
        }
        if imported_entries.is_empty() { return Ok(0); }
        let imported = imported_entries.len();
        self.expire(now);
        self.migrated = true;
        self.rime_tick = self.rime_tick.max(header_tick.unwrap_or_else(|| imported_entries.iter()
            .filter_map(|entry| entry.rime.as_ref().map(|weight| weight.tick)).max().unwrap_or(0)));
        let mut entries: HashMap<_, _> = Arc::make_mut(&mut self.entries).drain(..).map(|entry| (entry_key(&entry), entry)).collect();
        for entry in imported_entries {
            let key = entry_key(&entry);
            if let Some(existing) = entries.get_mut(&key) {
                // Importing twice must not erase selections made since migration.
                if existing.rime.as_ref().is_none_or(|weight| weight.updates == 0) {
                    *existing = entry;
                }
            } else {
                if self.baseline.as_ref().is_some_and(|base| base.entries.iter().any(|old| same_entry(old, &entry))) {
                    self.reset_entries.insert(key.clone());
                }
                entries.insert(key, entry);
            }
        }
        self.entries = Arc::new(entries.into_values().collect());
        self.limit();
        Ok(imported)
    }

    pub(crate) fn use_rime_order(&self, raw: &str) -> bool {
        let raw = raw.replace([' ', '\''], "");
        self.migrated && !self.entries.iter().any(|entry| entry.is_learned()
            && entry.reading.replace(' ', "") == raw)
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn scratch(label: &str) -> std::path::PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let directory = std::env::temp_dir().join(format!(
            "aim-zhixu-learning-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&directory).unwrap();
        fs::canonicalize(directory).unwrap()
    }

    fn choice(text: &str, now: u64) -> Choice {
        Choice {
            app: "test".into(),
            raw: "dao".into(),
            reading: "dao".into(),
            text: text.into(),
            target: 2,
            count: 1,
            last_ms: now,
            alternatives: vec!["道".into(), "倒".into()],
        }
    }

    const DYNAMICS_EXPORT: &str = "#@/tick\t1000\nta \t他\tc=37 d=12 t=1\nta \t它\tc=7 d=8 t=999\n";

    fn migrated_store() -> LearningStore {
        let mut store = LearningStore::default();
        store.import_rime_export(DYNAMICS_EXPORT, 100).unwrap();
        store
    }

    fn assert_same_dynamics(actual: &LearningStore, expected: &LearningStore) {
        assert_eq!(actual.rime_tick, expected.rime_tick);
        assert_eq!(actual.entries.len(), expected.entries.len());
        for entry in expected.entries.iter() {
            let found = actual.entries.iter().find(|e| same_entry(e, entry)).unwrap();
            assert_eq!(found.count, entry.count);
            let (a, b) = (found.rime.as_ref().unwrap(), entry.rime.as_ref().unwrap());
            assert_eq!((a.commits, a.updates), (b.commits, b.updates));
            assert!((a.decayed(actual.rime_tick) - b.decayed(expected.rime_tick)).abs() < 1e-12);
        }
    }

    #[test]
    fn forget_removes_native_and_migrated_facts_and_choices_after_restart() {
        for migrated in [false, true] {
            let dir = scratch("forget-restart");
            let mut store = LearningStore::default();
            if migrated {
                store.import_rime_export("行\txing\t12\n行\thang\t7\n型\txing\t3\n", 100).unwrap();
            } else {
                for (reading, text) in [("xing", "行"), ("hang", "行"), ("xing", "型")] {
                    store.record(reading, text, 100);
                }
            }
            for (app, raw, reading, text) in [
                ("one", "xing", "xing", "行"), ("two", "x", "xing", "行"),
                ("one", "hang", "hang", "行"), ("one", "xing", "xing", "型"),
            ] {
                let mut selected = choice(text, 100);
                selected.app = app.into();
                selected.raw = raw.into();
                selected.reading = reading.into();
                store.record_choice(selected);
            }
            store.save(&dir).unwrap();
            let mut stale = LearningStore::load(&dir, 100);
            store.record("xing", "行", 101);
            assert!(store.forget(" XING ", "行"));
            assert!(!store.entries().iter().any(|entry| entry.reading == "xing" && entry.text == "行"));
            assert!(!store.choices().iter().any(|choice| choice.reading == "xing" && choice.text == "行"));
            store.save(&dir).unwrap();
            stale.save(&dir).unwrap();
            let saved = LearningStore::load(&dir, 101);
            assert_eq!(saved.entries().len(), 2);
            assert_eq!(saved.choices().len(), 2);
            assert!(saved.entries().iter().any(|entry| entry.reading == "hang" && entry.text == "行"));
            assert!(saved.entries().iter().any(|entry| entry.reading == "xing" && entry.text == "型"));
            assert_eq!(saved.migrated, migrated);
            fs::remove_dir_all(dir).unwrap();
        }
    }

    #[test]
    fn forget_deletes_concurrent_disk_facts_and_preserves_other_words() {
        let dir = scratch("forget-concurrent");
        let mut deleting = LearningStore::load(&dir, 100);
        let mut peer = LearningStore::load(&dir, 100);
        peer.record("dao", "到", 100);
        peer.record_choice(choice("到", 100));
        peer.record("dao", "道", 100);
        peer.save(&dir).unwrap();
        assert!(deleting.forget("dao", "到"));
        deleting.save(&dir).unwrap();
        let saved = LearningStore::load(&dir, 100);
        assert_eq!(saved.entries().len(), 1);
        assert_eq!(saved.entries()[0].text, "道");
        assert!(saved.choices().is_empty());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn forget_retries_atomically_and_relearning_starts_a_new_count() {
        let dir = scratch("forget-retry");
        let mut store = migrated_store();
        store.save(&dir).unwrap();
        let original = fs::read(dir.join(FILE_NAME)).unwrap();
        assert!(store.forget("ta", "他"));
        let pending = store.clone();
        let held = learning_lock::LearningLock::acquire(&dir).unwrap();
        assert_eq!(store.save(&dir).unwrap_err().kind(), io::ErrorKind::TimedOut);
        drop(held);
        assert!(store.save_before_replace(&dir, || Err(io::Error::other("injected"))).is_err());
        assert_eq!(fs::read(dir.join(FILE_NAME)).unwrap(), original);
        assert_eq!(store, pending);
        store.save(&dir).unwrap();
        assert!(!LearningStore::load(&dir, 101).entries().iter().any(|entry| entry.text == "他"));
        store.record("ta", "它", 101);
        assert!(store.forget("ta", "它"));
        store.record("ta", "它", 102);
        store.save(&dir).unwrap();
        store.save(&dir).unwrap();
        let saved = LearningStore::load(&dir, 102);
        assert_eq!(saved.entries().len(), 1);
        let entry = &saved.entries()[0];
        assert_eq!(entry.count, 1);
        assert_eq!(entry.rime.as_ref().unwrap().commits, 1);
        assert_eq!(entry.rime.as_ref().unwrap().updates, 1);
        assert!(!store.forget("bad-reading", "它"));
        assert!(!store.forget("ta", "hello"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn migration_concurrent_initialization_does_not_recount_the_import() {
        let dir = scratch("migration-concurrent-init");
        let mut a = migrated_store();
        let mut b = migrated_store();
        a.record("ta", "他", 101);
        b.record("ta", "它", 102);
        a.save(&dir).unwrap();
        b.save(&dir).unwrap();
        let mut expected = migrated_store();
        expected.record("ta", "他", 101);
        expected.record("ta", "它", 102);
        assert_same_dynamics(&LearningStore::load(&dir, 102), &expected);
        a.save(&dir).unwrap();
        b.save(&dir).unwrap();
        assert_same_dynamics(&LearningStore::load(&dir, 102), &expected);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn migration_concurrent_updates_and_failed_save_preserve_decay() {
        let dir = scratch("migration-concurrent");
        migrated_store().save(&dir).unwrap();
        let mut a = LearningStore::load(&dir, 100);
        let mut b = LearningStore::load(&dir, 100);
        let mut expected = migrated_store();
        for (reading, text) in [("ta", "他"), ("zai gao", "再搞"), ("ta", "他")] {
            a.record(reading, text, 101);
            expected.record(reading, text, 101);
        }
        for (reading, text) in [("ta", "它"), ("zai gao", "再搞")] {
            b.record(reading, text, 102);
            expected.record(reading, text, 102);
        }
        a.save(&dir).unwrap();
        let disk = fs::read(dir.join(FILE_NAME)).unwrap();
        assert!(b.save_before_replace(&dir, || Err(io::Error::other("injected"))).is_err());
        assert_eq!(fs::read(dir.join(FILE_NAME)).unwrap(), disk);
        b.save(&dir).unwrap();
        b.save(&dir).unwrap();
        assert_same_dynamics(&LearningStore::load(&dir, 102), &expected);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn migration_score_matches_librime_reference_and_stays_finite() {
        for (commits, dynamic, tick, clock, expected) in [
            (37, 12.0, 1, 1000, -8.509056844386274),
            (7, 8.0, 999, 1000, -3.924521963740788),
            (200, 200.0, 100000, 100000, -0.013805305644961137),
        ] {
            let weight = RimeWeight { commits, dynamic, tick, updates: 0 };
            assert!((weight.score(clock) - expected).abs() < 1e-12);
        }
        let extreme = RimeWeight { commits: 37, dynamic: MAX_COUNT as f64, tick: 1000, updates: 0 };
        assert!(extreme.score(1000).is_finite());
    }

    #[test]
    fn migration_retains_4096_short_entries_within_the_byte_limit() {
        let mut export = String::from("#@/tick\t1000\n");
        for i in 0..MAX_ENTRIES + 66 {
            export.push_str(&format!("ta \t{}\tc=1 d=1 t=1000\n", char::from_u32(0x4e00 + i as u32).unwrap()));
        }
        let mut store = LearningStore::default();
        assert_eq!(store.import_rime_export(&export, 100).unwrap(), MAX_ENTRIES + 66);
        assert_eq!(store.entries().len(), MAX_ENTRIES);
        assert!(serde_json::to_vec(&store.document()).unwrap().len() <= MAX_BYTES as usize);
    }

    #[test]
    fn migration_rejects_invalid_weights_and_reads_legacy_documents() {
        let dir = scratch("migration-validation");
        let mut future = LearningStore::default();
        assert_eq!(future.import_rime_export("#@/tick\t1000\nta \t它\tc=7 d=8 t=1001\n", 100).unwrap(), 0);
        assert!(!future.migrated);
        let mut store = migrated_store();
        store.save(&dir).unwrap();
        let saved = store.document();
        for field in ["clock", "future", "updates"] {
            let mut document = saved.clone();
            match field {
                "clock" => document.rime_tick = u64::MAX,
                "future" => document.entries[0].rime.as_mut().unwrap().tick = 1001,
                _ => document.entries[0].rime.as_mut().unwrap().updates = u32::MAX,
            }
            fs::write(dir.join(FILE_NAME), serde_json::to_vec(&document).unwrap()).unwrap();
            let loaded = LearningStore::load(&dir, 100);
            assert!(loaded.entries().len() < saved.entries.len(), "{field}");
        }
        let mut legacy = serde_json::to_value(saved).unwrap();
        legacy.as_object_mut().unwrap().remove("migrated");
        legacy.as_object_mut().unwrap().remove("rimeTick");
        for row in legacy["entries"].as_array_mut().unwrap() { row.as_object_mut().unwrap().remove("rime"); }
        fs::write(dir.join(FILE_NAME), serde_json::to_vec(&legacy).unwrap()).unwrap();
        let loaded = LearningStore::load(&dir, 100);
        assert!(!loaded.migrated);
        assert_eq!(loaded.entries().len(), 2);
        assert!(loaded.entries().iter().all(|entry| entry.rime.is_none()));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn migration_retains_rime_dynamics_and_logical_clock() {
        let mut store = LearningStore::default();
        store.import_rime_export("#@/tick\t1000\nta \t他\tc=37 d=12 t=1\nta \t它\tc=7 d=8 t=999\n", 100).unwrap();
        let document = serde_json::to_value(store.document()).unwrap();
        assert_eq!(document["rimeTick"], 1000);
        assert_eq!(document["migrated"], true);
        let row = document["entries"].as_array().unwrap().iter().find(|row| row["text"] == "它").unwrap();
        assert_eq!(row["rime"]["dynamic"], 8.0);
        assert_eq!(row["rime"]["tick"], 999);
        assert_eq!(row["rime"]["updates"], 0);
    }

    #[test]
    fn independent_sessions_merge_only_their_new_counts() {
        use std::sync::{Arc, Barrier};
        let dir = scratch("concurrent");
        let mut initial = LearningStore::default();
        initial.record("dao", "到", 100);
        initial.record_choice(choice("到", 100));
        initial.remember_deliveries(&[1]);
        initial.save(&dir).unwrap();
        let barrier = Arc::new(Barrier::new(2));
        let threads: Vec<_> = (0..2)
            .map(|worker| {
                let dir = dir.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    let mut store = LearningStore::load(&dir, 100);
                    for round in 0..3 {
                        store.record("dao", "到", 101 + worker + round);
                        store.record_choice(choice("到", 101 + worker + round));
                        let (reading, text) = if worker == 0 {
                            ("zai gao", "再搞")
                        } else {
                            ("cai wo", "裁我")
                        };
                        store.record(reading, text, 101 + round);
                        store.remember_deliveries(&[10 + worker * 10 + round]);
                        barrier.wait();
                        if worker == 0 {
                            store.save(&dir).unwrap();
                            store.save(&dir).unwrap();
                        }
                        barrier.wait();
                        if worker == 1 {
                            store.save(&dir).unwrap();
                            store.save(&dir).unwrap();
                        }
                        barrier.wait();
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        let saved = LearningStore::load(&dir, 110);
        assert_eq!(
            saved
                .entries()
                .iter()
                .find(|e| e.text == "到")
                .unwrap()
                .count,
            7
        );
        assert_eq!(saved.choices()[0].count, 7);
        assert_eq!(saved.choices()[0].target, 0);
        assert_eq!(saved.choices()[0].last_ms, 104);
        for text in ["再搞", "裁我"] {
            assert_eq!(
                saved
                    .entries()
                    .iter()
                    .find(|e| e.text == text)
                    .unwrap()
                    .count,
                3
            );
        }
        assert_eq!(saved.delivery_ids.len(), 7);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn expiry_removes_observed_counts_without_erasing_a_concurrent_refresh() {
        let dir = scratch("delete");
        let mut initial = LearningStore::default();
        initial.record("dao", "到", 100);
        initial.record("zai gao", "再搞", 100);
        initial.record_choice(choice("到", 100));
        initial.save(&dir).unwrap();
        let mut expired = LearningStore::load(&dir, 100);
        let mut refreshed = LearningStore::load(&dir, 100);
        let now = MAX_AGE_MS + 101;
        refreshed.record("dao", "到", now);
        refreshed.record_choice(choice("到", now));
        refreshed.save(&dir).unwrap();
        expired.expire(now);
        expired.save(&dir).unwrap();
        let saved = LearningStore::load(&dir, now);
        assert_eq!(saved.entries().len(), 1);
        assert_eq!(saved.entries()[0].text, "到");
        assert_eq!(saved.entries()[0].count, 1);
        assert_eq!(saved.choices()[0].count, 1);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn damaged_disk_receives_only_the_unsaved_delta() {
        let dir = scratch("damaged-delta");
        let mut store = LearningStore::default();
        store.record("dao", "到", 100);
        store.record_choice(choice("到", 100));
        store.remember_deliveries(&[1]);
        store.save(&dir).unwrap();
        store.record("dao", "到", 101);
        store.record_choice(choice("到", 101));
        store.remember_deliveries(&[2]);
        fs::write(dir.join(FILE_NAME), b"broken").unwrap();
        store.save(&dir).unwrap();
        let saved = LearningStore::load(&dir, 101);
        assert_eq!(saved.entries()[0].count, 1);
        assert_eq!(saved.choices()[0].count, 1);
        assert_eq!(saved.delivery_ids, VecDeque::from([2]));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn saturated_counts_still_retain_new_increments_when_disk_is_damaged() {
        let dir = scratch("saturated-delta");
        let mut store = LearningStore::default();
        store
            .import_rime_export(&format!("到\tdao\t{MAX_COUNT}\n"), 100)
            .unwrap();
        let mut many = choice("到", 100);
        many.count = MAX_COUNT;
        store.record_choice(many);
        store.save(&dir).unwrap();
        store.record("dao", "到", 100);
        store.record_choice(choice("到", 100));
        fs::write(dir.join(FILE_NAME), b"broken").unwrap();
        store.save(&dir).unwrap();
        let saved = LearningStore::load(&dir, 100);
        assert_eq!(saved.entries().len(), 1);
        assert_eq!(saved.entries()[0].count, 1);
        assert_eq!(saved.choices()[0].count, 1);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn lock_timeout_retains_delta_for_the_next_save() {
        use std::time::{Duration, Instant};
        let dir = scratch("timeout");
        let mut store = LearningStore::default();
        store.record("dao", "到", 100);
        store.save(&dir).unwrap();
        store.record("dao", "到", 101);
        store.remember_deliveries(&[2]);
        let pending = store.clone();
        let original = fs::read(dir.join(FILE_NAME)).unwrap();
        let held = learning_lock::LearningLock::acquire(&dir).unwrap();
        let started = Instant::now();
        assert_eq!(
            store.save(&dir).unwrap_err().kind(),
            io::ErrorKind::TimedOut
        );
        assert!(started.elapsed() < Duration::from_millis(100));
        assert_eq!(store, pending);
        assert_eq!(fs::read(dir.join(FILE_NAME)).unwrap(), original);
        drop(held);
        store.record("dao", "到", 102);
        store.save(&dir).unwrap();
        store.save(&dir).unwrap();
        let saved = LearningStore::load(&dir, 102);
        assert_eq!(saved.entries()[0].count, 3);
        assert!(saved.has_delivery(2));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn failure_after_temporary_sync_preserves_disk_and_pending_delta() {
        let dir = scratch("before-replace");
        let mut store = LearningStore::default();
        store.record("dao", "到", 100);
        store.save(&dir).unwrap();
        store.record("dao", "到", 101);
        let pending = store.clone();
        let original = fs::read(dir.join(FILE_NAME)).unwrap();
        assert!(
            store
                .save_before_replace(&dir, || {
                    let temporary = dir.join(format!(".{FILE_NAME}.{}.tmp", std::process::id()));
                    assert!(fs::metadata(temporary).unwrap().len() > 0);
                    Err(io::Error::other("simulated crash before replacement"))
                })
                .is_err()
        );
        assert_eq!(fs::read(dir.join(FILE_NAME)).unwrap(), original);
        assert_eq!(store, pending);
        let mut peer = LearningStore::load(&dir, 101);
        peer.record("zai gao", "再搞", 102);
        peer.save(&dir).unwrap();
        store.save(&dir).unwrap();
        store.save(&dir).unwrap();
        let saved = LearningStore::load(&dir, 102);
        assert_eq!(
            saved
                .entries()
                .iter()
                .find(|entry| entry.text == "到")
                .unwrap()
                .count,
            2
        );
        assert_eq!(
            saved
                .entries()
                .iter()
                .find(|entry| entry.text == "再搞")
                .unwrap()
                .count,
            1
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn merge_preserves_newer_metadata_and_bounded_delivery_union() {
        let dir = scratch("union");
        let mut initial = LearningStore::default();
        initial.record("dao", "到", 100);
        initial.record_choice(choice("到", 100));
        initial.remember_deliveries(&(1..=MAX_DELIVERY_IDS as u64).collect::<Vec<_>>());
        initial.save(&dir).unwrap();
        let mut first = LearningStore::load(&dir, 100);
        let mut second = LearningStore::load(&dir, 100);
        first.record("dao", "到", 200);
        first.record_choice(choice("到", 200));
        first.owner_seeded = true;
        first.remember_deliveries(&[MAX_DELIVERY_IDS as u64 + 1]);
        first.save(&dir).unwrap();
        second.record("dao", "到", 150);
        let mut different = choice("到", 150);
        different.alternatives = vec!["道".into()];
        second.record_choice(different);
        second.legacy_choices_imported = true;
        second.remember_deliveries(&[MAX_DELIVERY_IDS as u64 + 2]);
        second.save(&dir).unwrap();
        let saved = LearningStore::load(&dir, 200);
        assert_eq!(saved.entries()[0].count, 3);
        assert_eq!(saved.entries()[0].last_ms, 200);
        assert_eq!(saved.choices()[0].count, 3);
        assert_eq!(saved.choices()[0].last_ms, 200);
        assert_eq!(saved.choices()[0].alternatives, vec!["道"]);
        assert!(saved.owner_seeded && saved.legacy_choices_imported);
        assert_eq!(saved.delivery_ids.len(), MAX_DELIVERY_IDS);
        assert!(saved.has_delivery(MAX_DELIVERY_IDS as u64 + 1));
        assert!(saved.has_delivery(MAX_DELIVERY_IDS as u64 + 2));
        assert_eq!(
            saved.delivery_ids.iter().collect::<HashSet<_>>().len(),
            MAX_DELIVERY_IDS
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn stale_unmodified_session_does_not_resurrect_deleted_facts() {
        let dir = scratch("no-resurrection");
        let mut initial = LearningStore::default();
        initial.record("dao", "到", 100);
        initial.record_choice(choice("到", 100));
        initial.save(&dir).unwrap();
        let mut stale = LearningStore::load(&dir, 100);
        let mut expired = LearningStore::load(&dir, MAX_AGE_MS + 101);
        expired.save(&dir).unwrap();
        stale.save(&dir).unwrap();
        let saved = LearningStore::load(&dir, 100);
        assert!(saved.entries().is_empty());
        assert!(saved.choices().is_empty());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn normalized_syllables_count_and_decay_are_shared() {
        let mut store = LearningStore::default();
        assert!(store.record(" ZAI'  GAO ", "再搞", 10));
        assert!(store.record("zai gao", "再搞", 20));
        assert_eq!(
            store.entries(),
            &[Entry {
                reading: "zai gao".into(),
                text: "再搞".into(),
                count: 2,
                last_ms: 20,
                rime: None,
            }]
        );
        assert_eq!(normalize_reading("LÜ"), Some("lv".into()));
        assert!(
            !store.record("zaigao", "再搞", 30),
            "never guess syllable boundaries"
        );
        assert!(!store.record("zai gao", "再", 30));
        assert!(!store.record("zzz", "字", 30));
        store.expire(20 + MAX_AGE_MS);
        assert_eq!(store.entries().len(), 1);
        store.expire(21 + MAX_AGE_MS);
        assert!(store.entries().is_empty());
    }

    #[test]
    fn atomic_private_storage_rejects_damage_and_oversize() {
        let dir = scratch("storage");
        let mut store = LearningStore::default();
        assert!(store.record("cai wo", "裁我", 100));
        store.save(&dir).unwrap();
        assert_eq!(LearningStore::load(&dir, 100), store);
        let original = fs::read(dir.join(FILE_NAME)).unwrap();
        let temporary = dir.join(format!(".{FILE_NAME}.{}.tmp", std::process::id()));
        fs::write(&temporary, "collision").unwrap();
        assert!(store.save(&dir).is_err());
        assert_eq!(
            fs::read(dir.join(FILE_NAME)).unwrap(),
            original,
            "failed write preserves old document"
        );
        fs::remove_file(&temporary).unwrap();
        store.save(&dir).unwrap();
        assert!(!temporary.exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(dir.join(FILE_NAME))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        fs::write(dir.join(FILE_NAME), b"broken").unwrap();
        assert!(LearningStore::load(&dir, 100).entries().is_empty());
        fs::OpenOptions::new()
            .write(true)
            .open(dir.join(FILE_NAME))
            .unwrap()
            .set_len(MAX_BYTES + 1)
            .unwrap();
        assert!(LearningStore::load(&dir, 100).entries().is_empty());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn oldest_entries_are_evicted_at_the_limit() {
        let mut store = LearningStore::default();
        for i in 0..=MAX_ENTRIES {
            assert!(store.record(
                "zi",
                &char::from_u32(0x4e00 + i as u32).unwrap().to_string(),
                i as u64
            ));
        }
        assert_eq!(store.entries().len(), MAX_ENTRIES);
        assert!(!store.entries().iter().any(|e| e.last_ms == 0));
    }

    #[test]
    fn synthetic_exports_import_counts_without_clock_guessing() {
        let mut store = LearningStore::default();
        let export = "# Rime user dictionary\n再搞\tzai gao\t7\n裁我\tcai wo\t3\n删除\tshan chu\t-1\n坏词\tinvalid\t99\n";
        assert_eq!(store.import_rime_export(export, 123).unwrap(), 2);
        assert_eq!(
            store
                .entries()
                .iter()
                .find(|e| e.text == "再搞")
                .unwrap()
                .count,
            7
        );
        assert!(store.entries().iter().all(|e| e.last_ms == 123));
        assert_eq!(store.import_rime_export(export, 124).unwrap(), 2);
        assert_eq!(
            store
                .entries()
                .iter()
                .find(|e| e.text == "再搞")
                .unwrap()
                .count,
            7,
            "reimport does not count the same picks twice"
        );
        assert_eq!(
            store
                .import_rime_export(
                    "#@/db_type\tuserdb\nzai gao\t再搞\tc=9 d=0.1 t=999999\n",
                    125
                )
                .unwrap(),
            1
        );
        assert_eq!(
            store
                .entries()
                .iter()
                .find(|e| e.text == "再搞")
                .unwrap()
                .count,
            9
        );
        assert_eq!(
            store
                .entries()
                .iter()
                .find(|e| e.text == "再搞")
                .unwrap()
                .last_ms,
            125
        );
    }

    #[test]
    fn choices_keep_raw_keys_separate_and_share_storage_and_expiry() {
        let dir = scratch("choices");
        let mut store = LearningStore::default();
        let choice = Choice {
            app: "test".into(),
            raw: "dao".into(),
            reading: "dao".into(),
            text: "到".into(),
            target: 2,
            count: 1,
            last_ms: 100,
            alternatives: vec!["道".into()],
        };
        assert!(store.record_choice(choice.clone()));
        assert!(store.record_choice(choice.clone()));
        assert_eq!(store.choices()[0].count, 2);
        assert_eq!(store.choices()[0].target, 2);
        assert!(store.choices().iter().all(|entry| entry.raw != "d"));
        assert!(store.record_choice(choice));
        assert_eq!(store.choices()[0].target, 0);
        assert!(store.record("dao", "到", 100));
        store.save(&dir).unwrap();
        assert_eq!(LearningStore::load(&dir, 100), store);
        store.expire(101 + MAX_AGE_MS);
        assert!(store.entries().is_empty());
        assert!(store.choices().is_empty());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn pair_evidence_does_not_assign_old_counts_to_new_alternatives() {
        let mut store = LearningStore::default();
        let mut choice = Choice {
            app: "test".into(),
            raw: "dao".into(),
            reading: "dao".into(),
            text: "到".into(),
            target: 2,
            count: 1,
            last_ms: 100,
            alternatives: vec!["道".into(), "倒".into()],
        };
        assert!(store.record_choice(choice.clone()));
        choice.alternatives = vec!["道".into(), "刀".into()];
        assert!(store.record_choice(choice));
        assert_eq!(
            store.choices()[0].alternatives,
            vec!["道"],
            "only challengers seen on every counted choice can share its count"
        );
    }

    #[test]
    fn long_words_are_bounded_by_serialized_size_and_migration_keeps_high_counts() {
        let dir = scratch("byte-limit");
        let reading = std::iter::repeat_n("zhuang", 64)
            .collect::<Vec<_>>()
            .join(" ");
        let mut export = String::new();
        for i in 0..2000 {
            let word = format!("{}{}", "庄".repeat(63), char::from_u32(0x4e00 + i).unwrap());
            export.push_str(&format!("{word}\t{reading}\t{}\n", i + 1));
        }
        let mut store = LearningStore::default();
        assert_eq!(store.import_rime_export(&export, 100).unwrap(), 2000);
        assert!(store.entries().len() < 2000);
        assert_eq!(store.entries()[0].count, 2000);
        store.save(&dir).unwrap();
        assert!(fs::metadata(dir.join(FILE_NAME)).unwrap().len() <= MAX_BYTES);
        assert_eq!(LearningStore::load(&dir, 100), store);
        fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn symlink_and_public_file_are_not_loaded() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let dir = scratch("file-safety");
        let mut store = LearningStore::default();
        assert!(store.record("cai wo", "裁我", 100));
        store.save(&dir).unwrap();
        fs::rename(dir.join(FILE_NAME), dir.join("real.json")).unwrap();
        symlink(dir.join("real.json"), dir.join(FILE_NAME)).unwrap();
        assert!(LearningStore::load(&dir, 100).entries().is_empty());
        fs::remove_file(dir.join(FILE_NAME)).unwrap();
        fs::rename(dir.join("real.json"), dir.join(FILE_NAME)).unwrap();
        fs::set_permissions(dir.join(FILE_NAME), fs::Permissions::from_mode(0o644)).unwrap();
        assert!(LearningStore::load(&dir, 100).entries().is_empty());
        fs::remove_dir_all(dir).unwrap();
    }
}
