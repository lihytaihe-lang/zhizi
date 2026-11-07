//! Delayed learning follows the host's existing key and commit observations.

use super::Index;
use super::learning::{Choice, Entry, LearningMode, LearningStore};
use crate::personal_picks;
use crate::ranking::OwnerStyle;
use crate::secure_directory::physical_runtime_path;
use crate::state::{KeyEvent, KeyKind, SecurityState};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};

const RETENTION_MS: u64 = 5_000;
const MAX_PENDING: usize = 32;

fn next_revision() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

struct Knowledge {
    directory: Option<PathBuf>,
    loaded: bool,
    store: LearningStore,
    revision: u64,
    deleted_words: HashMap<(String, String), u64>,
    dirty: bool,
    overlay_dirty: bool,
    overlay_words: Vec<(String, String)>,
    overlay: Option<Arc<Index>>,
    overlay_includes_migrated: bool,
    whole_overlay: Option<(u64, Option<Arc<Index>>)>,
    style_dirty: bool,
    style: Option<Arc<OwnerStyle>>,
}

impl Knowledge {
    fn new(directory: Option<PathBuf>) -> Self {
        Self {
            directory,
            loaded: false,
            store: LearningStore::default(),
            revision: next_revision(),
            deleted_words: HashMap::new(),
            dirty: false,
            overlay_dirty: false,
            overlay_words: Vec::new(),
            overlay: None,
            overlay_includes_migrated: false,
            whole_overlay: None,
            style_dirty: true,
            style: None,
        }
    }

    fn load(&mut self) {
        if !self.loaded {
            if let Some(directory) = &self.directory {
                self.store = LearningStore::load(directory, personal_picks::now_ms());
            }
            self.loaded = true;
            self.revision = next_revision();
            self.overlay_dirty = true;
            self.style_dirty = true;
        }
    }

    fn persist(&mut self) -> bool {
        if self.dirty {
            let saved = self
                .directory
                .as_ref()
                .is_none_or(|directory| self.store.save(directory).is_ok());
            // Saving can also import facts committed by another process.
            self.revision = next_revision();
            self.overlay_dirty = true;
            self.style_dirty = true;
            if saved {
                self.dirty = false;
            }
        }
        !self.dirty
    }

    fn overlay(&mut self) -> Option<Arc<Index>> {
        self.overlay_with_migrated(false)
    }

    fn overlay_with_migrated(&mut self, include_migrated: bool) -> Option<Arc<Index>> {
        if !self.overlay_dirty && self.overlay_includes_migrated == include_migrated {
            return self.overlay.clone();
        }
        let mut words: Vec<_> = self
            .store
            .entries()
            .iter()
            .filter(|entry| (include_migrated || entry.is_learned()) && entry.text.chars().count() >= 2)
            .map(|entry| (entry.reading.clone(), entry.text.clone()))
            .collect();
        words.sort();
        if words != self.overlay_words {
            self.overlay = (!words.is_empty()).then(|| {
                let entries = self.store.entries().iter()
                    .filter(|entry| include_migrated || entry.is_learned()).cloned().collect::<Vec<_>>();
                Arc::new(Index::from_learning(&entries))
            });
            self.overlay_words = words;
        }
        self.overlay_dirty = false;
        self.overlay_includes_migrated = include_migrated;
        self.overlay.clone()
    }

    fn style(&mut self) -> Arc<OwnerStyle> {
        if self.style_dirty {
            self.style = Some(Arc::new(OwnerStyle::from_learning(self.store.choices())));
            self.style_dirty = false;
        }
        self.style.as_ref().unwrap().clone()
    }

    fn changed(&mut self) {
        self.revision = next_revision();
        self.dirty = true;
        self.overlay_dirty = true;
        self.style_dirty = true;
    }
}

fn registry() -> &'static Mutex<HashMap<PathBuf, Weak<Mutex<Knowledge>>>> {
    static REGISTRY: OnceLock<Mutex<HashMap<PathBuf, Weak<Mutex<Knowledge>>>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

struct Pending {
    text: String,
    raw: String,
    choices: Vec<Choice>,
    ids: Vec<u64>,
    committed_ms: u64,
    revision: u64,
}

pub(crate) struct LearningSession {
    pub(crate) mode: LearningMode,
    knowledge: Arc<Mutex<Knowledge>>,
    segments: Vec<(Choice, u64)>,
    invalid_segment: bool,
    pending: VecDeque<Pending>,
}

impl Default for LearningSession {
    fn default() -> Self {
        Self {
            mode: LearningMode::Rime,
            knowledge: Arc::new(Mutex::new(Knowledge::new(None))),
            segments: Vec::new(),
            invalid_segment: false,
            pending: VecDeque::new(),
        }
    }
}

impl LearningSession {
    pub(crate) fn revision(&self) -> u64 {
        #[cfg(test)]
        let _timing = crate::mixer::WholeLearningTiming::start();
        self.knowledge.lock().map_or(0, |knowledge| knowledge.revision)
    }

    pub(crate) fn style(&self) -> Option<Arc<OwnerStyle>> {
        #[cfg(test)]
        let _timing = crate::mixer::WholeLearningTiming::start();
        if self.mode != LearningMode::Zhixu {
            return None;
        }
        self.knowledge
            .lock()
            .ok()
            .map(|mut knowledge| knowledge.style())
    }
    /// Binding a default-mode session must not read or create learning data.
    pub(crate) fn attach(&mut self, directory: &Path) {
        let directory = physical_runtime_path(directory);
        if self
            .knowledge
            .lock()
            .is_ok_and(|knowledge| knowledge.directory.as_ref() == Some(&directory))
        {
            return;
        }
        self.clear_segments();
        self.pending.clear();
        let Ok(mut registry) = registry().lock() else {
            return;
        };
        registry.retain(|_, knowledge| knowledge.strong_count() > 0);
        self.knowledge = if let Some(knowledge) = registry.get(&directory).and_then(Weak::upgrade) {
            knowledge
        } else {
            let knowledge = Arc::new(Mutex::new(Knowledge::new(Some(directory.clone()))));
            registry.insert(directory, Arc::downgrade(&knowledge));
            knowledge
        };
    }

    pub(crate) fn set_mode(&mut self, mode: LearningMode, index: &Index) {
        if self.mode != mode {
            self.clear_segments();
            self.pending.clear();
        }
        self.mode = mode;
        self.seed_if_needed(index);
    }

    /// The runtime dictionary can finish loading after the mode is enabled.
    /// A persisted marker prevents expired seeds being revived on the next launch.
    pub(crate) fn seed_if_needed(&self, index: &Index) {
        #[cfg(test)]
        let _timing = crate::mixer::WholeLearningTiming::start();
        if self.mode != LearningMode::Zhixu {
            return;
        }
        let Ok(mut knowledge) = self.knowledge.lock() else {
            return;
        };
        knowledge.load();
        if knowledge.store.owner_seeded || knowledge.store.migrated {
            return;
        }
        let seeds = index.owner_words();
        if seeds.is_empty() {
            return;
        }
        let now = personal_picks::now_ms();
        for (text, reading, _) in seeds {
            if !knowledge
                .store
                .entries()
                .iter()
                .any(|entry| entry.text == text && entry.reading == reading)
            {
                knowledge.store.record(&reading, &text, now);
            }
        }
        knowledge.store.owner_seeded = true;
        knowledge.changed();
        knowledge.persist();
    }

    pub(crate) fn entries(&self) -> Arc<Vec<Entry>> {
        #[cfg(test)]
        let _timing = crate::mixer::WholeLearningTiming::start();
        if self.mode != LearningMode::Zhixu {
            return Arc::default();
        }
        self.knowledge
            .lock()
            .map(|knowledge| knowledge.store.shared_entries())
            .unwrap_or_default()
    }

    pub(crate) fn use_rime_order(&self, raw: &str) -> bool {
        #[cfg(test)]
        let _timing = crate::mixer::WholeLearningTiming::start();
        self.mode == LearningMode::Zhixu && self.knowledge.lock()
            .is_ok_and(|knowledge| knowledge.store.use_rime_order(raw))
    }

    pub(crate) fn rime_tick(&self) -> u64 {
        #[cfg(test)]
        let _timing = crate::mixer::WholeLearningTiming::start();
        self.knowledge.lock().map_or(0, |knowledge| knowledge.store.rime_tick)
    }

    pub(crate) fn choices(&self) -> Vec<Choice> {
        #[cfg(test)]
        let _timing = crate::mixer::WholeLearningTiming::start();
        if self.mode != LearningMode::Zhixu {
            return Vec::new();
        }
        self.knowledge
            .lock()
            .map(|knowledge| knowledge.store.choices().to_vec())
            .unwrap_or_default()
    }

    pub(crate) fn overlay(&self) -> Option<Arc<Index>> {
        #[cfg(test)]
        let _timing = crate::mixer::WholeLearningTiming::start();
        if self.mode != LearningMode::Zhixu {
            return None;
        }
        self.knowledge
            .lock()
            .ok()
            .and_then(|mut knowledge| knowledge.overlay())
    }

    pub(crate) fn whole_overlay(&self) -> Option<Arc<Index>> {
        #[cfg(test)]
        let _timing = crate::mixer::WholeLearningTiming::start();
        if self.mode != LearningMode::Zhixu { return None; }
        self.knowledge.lock().ok().and_then(|mut knowledge| {
            if knowledge.whole_overlay.as_ref().is_none_or(|(revision, _)| *revision != knowledge.revision) {
                let overlay = knowledge.store.entries().iter().any(|entry| entry.text.chars().count() >= 2)
                    .then(|| Arc::new(Index::from_learning(knowledge.store.entries())));
                knowledge.whole_overlay = Some((knowledge.revision, overlay));
            }
            knowledge.whole_overlay.as_ref().and_then(|(_, overlay)| overlay.clone())
        })
    }

    /// Called only after a successful selection. An unresolved reading invalidates
    /// the whole assembled phrase, rather than learning it under guessed keys.
    pub(crate) fn stage(&mut self, choice: Option<Choice>, id: u64) {
        if self.mode != LearningMode::Zhixu {
            return;
        }
        if id == 0 {
            self.invalid_segment = true;
            return;
        }
        if self.segments.iter().any(|(_, seen)| *seen == id)
            || self.pending.iter().any(|pending| pending.ids.contains(&id))
            || self
                .knowledge
                .lock()
                .is_ok_and(|knowledge| knowledge.store.has_delivery(id))
        {
            return;
        }
        let Some(mut choice) = choice else {
            self.invalid_segment = true;
            return;
        };
        choice.count = 1;
        let mut checked = LearningStore::default();
        if !checked.record_choice(choice) || self.segments.len() >= 64 {
            self.invalid_segment = true;
            return;
        }
        let choice = checked.choices()[0].clone();
        if self
            .segments
            .first()
            .is_some_and(|(first, _)| first.app != choice.app)
        {
            self.invalid_segment = true;
            return;
        }
        self.segments.push((choice, id));
    }

    pub(crate) fn commit(&mut self, text: &str, id: u64, now: u64) {
        #[cfg(test)]
        let _timing = crate::mixer::WholeLearningTiming::start();
        if self.mode != LearningMode::Zhixu {
            return;
        }
        let invalid = self.invalid_segment;
        let segments = std::mem::take(&mut self.segments);
        self.invalid_segment = false;
        if invalid || id == 0 || segments.is_empty() {
            return;
        }
        let selected: String = segments
            .iter()
            .map(|(choice, _)| choice.text.as_str())
            .collect();
        if retained_text(text) != selected || selected.chars().count() > 64 {
            return;
        }
        let raw: String = segments
            .iter()
            .map(|(choice, _)| choice.raw.as_str())
            .collect();
        let reading = segments
            .iter()
            .map(|(choice, _)| choice.reading.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        let mut ids: Vec<_> = segments.iter().map(|(_, id)| *id).collect();
        if !ids.contains(&id) {
            ids.push(id);
        }
        let mut choices: Vec<_> = segments
            .into_iter()
            .map(|(mut choice, _)| {
                choice.last_ms = now;
                choice
            })
            .collect();
        if choices.len() > 1 {
            choices.push(Choice {
                app: choices[0].app.clone(),
                raw: raw.clone(),
                reading,
                text: selected.clone(),
                target: 0,
                count: 1,
                last_ms: now,
                alternatives: Vec::new(),
            });
        }
        if self.pending.len() == MAX_PENDING {
            self.pending.pop_front();
        }
        let revision = self.revision();
        self.pending.push_back(Pending {
            text: selected,
            raw,
            choices,
            ids,
            committed_ms: now,
            revision,
        });
    }

    pub(crate) fn key(&mut self, event: &KeyEvent, composing: bool, now: u64) {
        #[cfg(test)]
        let _timing = crate::mixer::WholeLearningTiming::start();
        if self.mode != LearningMode::Zhixu {
            return;
        }
        if event.security != SecurityState::NonSecure {
            self.clear_segments();
            self.pending.clear();
            return;
        }
        if matches!(
            event.kind,
            KeyKind::FocusLost | KeyKind::Deactivate | KeyKind::Sleep | KeyKind::Resume
        ) {
            if !composing {
                // Switching away or losing focus confirms what was committed (Q15);
                // sleep and resume cannot tell, so they only keep what already matured.
                let confirmed = matches!(event.kind, KeyKind::FocusLost | KeyKind::Deactivate);
                self.flush_pending(now, confirmed);
            }
            self.clear_segments();
            self.pending.clear();
            return;
        }
        // Without a replacement-range signal, deleting any idle text can refer
        // to any waiting commit. Dropping all of them is the conservative choice.
        if event.kind == KeyKind::Backspace && !composing {
            self.pending.clear();
            self.clear_segments();
            return;
        }
        if matches!(event.kind, KeyKind::Escape | KeyKind::Enter) {
            self.clear_segments();
        }
        // Do not change the page between displaying a candidate and receiving
        // its digit. Mature observations can wait for the next idle boundary.
        if !composing { self.flush_pending(now, false); }
    }

    fn flush_pending(&mut self, now: u64, confirmed: bool) {
        let Ok(mut knowledge) = self.knowledge.lock() else {
            return;
        };
        knowledge.load();
        let before = (
            knowledge.store.entries().len(),
            knowledge.store.choices().len(),
        );
        knowledge.store.expire(now);
        if before
            != (
                knowledge.store.entries().len(),
                knowledge.store.choices().len(),
            )
        {
            knowledge.changed();
        }
        while self
            .pending
            .front()
            .is_some_and(|pending| {
                confirmed || now.saturating_sub(pending.committed_ms) >= RETENTION_MS
            })
        {
            let pending = self.pending.pop_front().unwrap();
            if pending
                .ids
                .iter()
                .any(|id| knowledge.store.has_delivery(*id))
            {
                continue;
            }
            // Another app session can delete a word during the retention window.
            // Drop its entire older group so a removed segment cannot be learned
            // again through the assembled phrase; later choices remain learnable.
            if pending.choices.iter().any(|choice| knowledge.deleted_words
                .get(&(choice.reading.clone(), choice.text.clone()))
                .is_some_and(|revision| pending.revision < *revision)) {
                continue;
            }
            for choice in pending.choices {
                knowledge
                    .store
                    .record(&choice.reading, &choice.text, choice.last_ms);
                knowledge.store.record_choice(choice);
            }
            knowledge.store.remember_deliveries(&pending.ids);
            knowledge.changed();
        }
        knowledge.persist();
    }

    pub(crate) fn forget(&mut self, reading: &str, text: &str) -> Result<(), i32> {
        let reading = super::learning::normalize_reading(reading).ok_or(-105)?;
        let shared = Arc::clone(&self.knowledge);
        let mut knowledge = shared.lock().map_err(|_| -105)?;
        knowledge.load();
        if !knowledge.store.forget(&reading, text) { return Err(-105); }
        // A failed write keeps the deletion pending; earlier queued selections
        // must not undo that request when the next idle boundary retries it.
        self.forget_pending(&reading, text);
        knowledge.changed();
        let revision = knowledge.revision;
        knowledge.deleted_words.insert((reading, text.into()), revision);
        if !knowledge.persist() { return Err(-105); }
        Ok(())
    }

    /// The outer Mixer has its own delayed observations but shares Knowledge
    /// with the decoder, so both layers discard this word's in-flight learning.
    pub(crate) fn forget_pending(&mut self, reading: &str, text: &str) {
        let Some(reading) = super::learning::normalize_reading(reading) else { return; };
        let matches = |choice: &Choice| choice.reading == reading && choice.text == text;
        self.pending.retain(|pending| !pending.choices.iter().any(matches));
        let before = self.segments.len();
        self.segments.retain(|(choice, _)| !matches(choice));
        if self.segments.len() != before { self.invalid_segment = true; }
    }

    pub(crate) fn withdraw(&mut self, raw: &str, text: &str) -> bool {
        let before = self.pending.len();
        let raw = compact(raw);
        self.pending
            .retain(|pending| compact(&pending.raw) != raw || pending.text != retained_text(text));
        self.pending.len() != before
    }

    pub(crate) fn clear_segments(&mut self) {
        self.segments.clear();
        self.invalid_segment = false;
    }

    pub(crate) fn import_choices(&self, choices: Vec<Choice>) {
        if self.mode != LearningMode::Zhixu {
            return;
        }
        let Ok(mut knowledge) = self.knowledge.lock() else {
            return;
        };
        knowledge.load();
        if knowledge.store.legacy_choices_imported {
            return;
        }
        let now = personal_picks::now_ms();
        for choice in choices {
            if now.saturating_sub(choice.last_ms) > super::learning::MAX_AGE_MS
                || knowledge.store.choices().iter().any(|old| {
                    old.app == choice.app && old.raw == choice.raw && old.text == choice.text
                })
            {
                continue;
            }
            if !knowledge.store.migrated && !knowledge
                .store
                .entries()
                .iter()
                .any(|entry| entry.reading == choice.reading && entry.text == choice.text)
            {
                knowledge
                    .store
                    .record(&choice.reading, &choice.text, choice.last_ms);
            }
            knowledge.store.record_choice(choice);
        }
        knowledge.store.legacy_choices_imported = true;
        knowledge.changed();
        knowledge.persist();
    }

    #[cfg(test)]
    pub(crate) fn remember_for_test(&self, reading: &str, text: &str, now: u64) {
        if let Ok(mut knowledge) = self.knowledge.lock() {
            knowledge.store.record(reading, text, now);
            knowledge.revision = next_revision();
            knowledge.overlay_dirty = true;
        }
    }
}

fn retained_text(text: &str) -> &str {
    text.trim_end_matches(|c: char| {
        c.is_ascii_punctuation()
            || matches!(
                c,
                ' ' | '，'
                    | '。'
                    | '！'
                    | '？'
                    | '；'
                    | '：'
                    | '、'
                    | '“'
                    | '”'
                    | '‘'
                    | '’'
                    | '（'
                    | '）'
                    | '《'
                    | '》'
                    | '…'
                    | '—'
            )
    })
}

fn compact(raw: &str) -> String {
    raw.chars()
        .filter(|c| *c != '\'' && !c.is_ascii_whitespace())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::loader::{DictionaryData, Entry as DictionaryEntry};
    use super::*;
    use crate::state::{KeyKind, SecurityState};
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn index() -> Index {
        Index::from_data(DictionaryData::default())
    }
    fn session() -> LearningSession {
        let mut session = LearningSession::default();
        session.set_mode(LearningMode::Zhixu, &index());
        session
    }
    fn choice(reading: &str, text: &str) -> Choice {
        Choice {
            app: "test".into(),
            raw: reading.replace(' ', ""),
            reading: reading.into(),
            text: text.into(),
            target: 1,
            count: 1,
            last_ms: 100,
            alternatives: Vec::new(),
        }
    }
    fn key(kind: KeyKind) -> KeyEvent {
        KeyEvent {
            kind,
            text: "a".into(),
            modifiers: 0,
            security: SecurityState::NonSecure,
            delivery_id: 99,
        }
    }
    fn scratch(label: &str) -> std::path::PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "aim-zhixu-session-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&dir).unwrap();
        fs::canonicalize(dir).unwrap()
    }

    #[test]
    fn forget_persists_and_invalidates_shared_views_without_replaying_pending() {
        let dir = scratch("forget");
        let now = personal_picks::now_ms();
        let mut store = LearningStore::default();
        store.import_rime_export("再搞\tzai gao\t12\n", now).unwrap();
        let mut selected = choice("zai gao", "再搞");
        selected.last_ms = now;
        store.record_choice(selected);
        store.save(&dir).unwrap();
        let mut peer = session();
        let mut session = session();
        session.attach(&dir);
        session.set_mode(LearningMode::Zhixu, &index());
        peer.attach(&dir);
        peer.set_mode(LearningMode::Zhixu, &index());
        assert!(session.whole_overlay().unwrap().vocabulary_word("再搞", "zaigao"));
        let style = peer.style().unwrap();
        assert_eq!(peer.choices().len(), 1);
        let revision = peer.revision();
        session.stage(Some(choice("zai gao", "再搞")), 1);
        session.commit("再搞", 1, now);
        session.stage(Some(choice("dao", "到")), 2);
        session.commit("到", 2, now);
        session.forget("zai gao", "再搞").unwrap();
        assert!(session.entries().is_empty());
        assert!(peer.entries().is_empty());
        assert!(peer.revision() != revision);
        assert!(peer.choices().is_empty());
        assert!(!Arc::ptr_eq(&style, &peer.style().unwrap()));
        assert!(session.whole_overlay().is_none());
        assert!(session.overlay().is_none());
        assert_eq!(session.pending.len(), 1);
        session.flush_pending(now + RETENTION_MS, false);
        assert_eq!(session.entries().len(), 1);
        assert_eq!(session.entries()[0].text, "到");
        drop(peer);
        drop(session);
        let saved = LearningStore::load(&dir, now + RETENTION_MS);
        assert_eq!(saved.entries().len(), 1);
        assert_eq!(saved.entries()[0].text, "到");
        assert!(!saved.choices().iter().any(|choice| choice.text == "再搞"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn forget_cancels_peer_pending_groups_but_allows_new_choices() {
        let dir = scratch("forget-peer");
        let now = personal_picks::now_ms();
        let mut store = LearningStore::default();
        store.record("zai", "再", now);
        store.save(&dir).unwrap();
        let mut deleting = session();
        let mut peer = session();
        deleting.attach(&dir);
        deleting.set_mode(LearningMode::Zhixu, &index());
        peer.attach(&dir);
        peer.set_mode(LearningMode::Zhixu, &index());
        peer.stage(Some(choice("zai", "再")), 1);
        peer.stage(Some(choice("gao", "搞")), 2);
        peer.commit("再搞", 2, now);
        peer.stage(Some(choice("zai", "再")), 3);
        peer.commit("再", 3, now);
        peer.stage(Some(choice("dao", "到")), 4);
        peer.commit("到", 4, now);
        deleting.forget(" ZAI ", "再").unwrap();
        peer.flush_pending(now + RETENTION_MS, false);
        let saved = LearningStore::load(&dir, now + RETENTION_MS);
        assert_eq!(saved.entries().len(), 1, "old peer observations must not restore a deleted word or its assembled group");
        assert_eq!(saved.entries()[0].text, "到");
        assert_eq!(saved.choices().len(), 1);
        assert_eq!(saved.choices()[0].text, "到");
        peer.stage(Some(choice("zai", "再")), 5);
        peer.commit("再", 5, now + RETENTION_MS + 1);
        peer.flush_pending(now + RETENTION_MS * 2 + 1, false);
        let saved = LearningStore::load(&dir, now + RETENTION_MS * 2 + 1);
        assert_eq!(saved.entries().len(), 2);
        assert_eq!(saved.entries().iter().find(|entry| entry.text == "再").unwrap().count, 1);
        assert_eq!(saved.choices().iter().find(|choice| choice.text == "再").unwrap().count, 1);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn forget_pending_only_invalidates_the_matching_segment_group() {
        let mut session = session();
        session.stage(Some(choice("zai", "再")), 1);
        session.stage(Some(choice("gao", "搞")), 2);
        session.forget_pending("zai", "再");
        assert_eq!(session.segments.len(), 1);
        assert_eq!(session.segments[0].0.text, "搞");
        assert!(session.invalid_segment);
        session.commit("再搞", 2, 100);
        session.flush_pending(5100, false);
        assert!(session.entries().is_empty());
        session.stage(Some(choice("dao", "到")), 3);
        session.forget_pending("zai", "再");
        session.commit("到", 3, 200);
        session.flush_pending(5200, false);
        assert_eq!(session.entries()[0].text, "到");
    }

    #[test]
    fn forget_reports_persistence_failure_and_retries_the_same_deletion() {
        let dir = scratch("forget-write-failure");
        let now = personal_picks::now_ms();
        let mut store = LearningStore::default();
        store.record("dao", "到", now);
        store.save(&dir).unwrap();
        let original = fs::read(dir.join(super::super::learning::FILE_NAME)).unwrap();
        let temporary = dir.join(format!(".{}.{}.tmp", super::super::learning::FILE_NAME, std::process::id()));
        fs::write(&temporary, b"existing temporary").unwrap();
        let mut session = session();
        session.attach(&dir);
        session.stage(Some(choice("dao", "到")), 1);
        session.commit("到", 1, now);
        assert_eq!(session.forget("dao", "到"), Err(-105));
        assert!(session.pending.is_empty(), "a failed save must not replay observations queued before deletion");
        assert_eq!(fs::read(dir.join(super::super::learning::FILE_NAME)).unwrap(), original);
        fs::remove_file(temporary).unwrap();
        session.flush_pending(now + RETENTION_MS, false);
        assert!(LearningStore::load(&dir, now + RETENTION_MS).entries().is_empty());
        session.forget("dao", "到").unwrap();
        assert!(LearningStore::load(&dir, now).entries().is_empty());
        assert_eq!(session.forget("bad-reading", "到"), Err(-105));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn segmented_commit_waits_for_retention_then_becomes_a_whole_word() {
        let mut session = session();
        session.stage(Some(choice("zai", "再")), 1);
        session.stage(Some(choice("gao", "搞")), 2);
        session.commit("再搞，", 2, 100);
        assert!(session.entries().is_empty());
        session.key(&key(KeyKind::Text), false, 5099);
        assert!(session.entries().is_empty());
        session.key(&key(KeyKind::Text), false, 5100);
        assert!(
            session
                .entries()
                .iter()
                .any(|entry| entry.reading == "zai gao" && entry.text == "再搞")
        );
        assert!(session.overlay().unwrap().vocabulary_word("再搞", "zaigao"));
        assert!(
            !session.overlay().unwrap().vocabulary_word("再", "zai"),
            "single characters never enter composition overlay"
        );
    }

    #[test]
    fn deleted_or_unverified_commits_never_learn() {
        for kind in [KeyKind::Backspace, KeyKind::Sleep, KeyKind::Resume] {
            let mut session = session();
            session.stage(Some(choice("cai wo", "裁我")), 1);
            session.commit("裁我", 1, 100);
            session.key(&key(kind), false, 1000);
            session.key(&key(KeyKind::Text), false, 5200);
            assert!(session.entries().is_empty(), "{kind:?}");
        }
        for committed in ["裁我们", "裁我abc", "别的"] {
            let mut session = session();
            session.stage(Some(choice("cai wo", "裁我")), 1);
            session.commit(committed, 1, 100);
            session.key(&key(KeyKind::Text), false, 5100);
            assert!(session.entries().is_empty(), "{committed}");
        }
        let mut session = session();
        session.stage(Some(choice("cai wo", "裁我")), 0);
        session.commit("裁我", 0, 100);
        session.key(&key(KeyKind::Text), false, 5100);
        assert!(session.entries().is_empty());
    }

    #[test]
    fn learning_waits_for_idle_before_changing_a_displayed_page() {
        let mut session = session();
        session.stage(Some(choice("cai wo", "裁我")), 1);
        session.commit("裁我", 1, 100);
        session.key(&key(KeyKind::Space), true, 5100);
        assert!(session.entries().is_empty(), "the displayed selection cannot move under its key");
        session.key(&key(KeyKind::Text), false, 5200);
        assert_eq!(session.entries()[0].text, "裁我");
    }

    #[test]
    fn switching_away_confirms_a_commit_that_has_not_waited_out_retention() {
        for kind in [KeyKind::FocusLost, KeyKind::Deactivate] {
            let mut session = session();
            session.stage(Some(choice("cai wo", "裁我")), 1);
            session.commit("裁我", 1, 100);
            session.key(&key(kind), false, 1000);
            assert_eq!(session.entries().len(), 1, "{kind:?}");
        }
        // Mid-composition the page must not move, and a deleted commit still never learns.
        let mut session = session();
        session.stage(Some(choice("cai wo", "裁我")), 1);
        session.commit("裁我", 1, 100);
        session.key(&key(KeyKind::Backspace), false, 500);
        session.key(&key(KeyKind::FocusLost), false, 1000);
        assert!(session.entries().is_empty());
    }

    #[test]
    fn focus_boundary_keeps_commits_that_already_passed_retention() {
        for kind in [
            KeyKind::FocusLost,
            KeyKind::Deactivate,
            KeyKind::Sleep,
            KeyKind::Resume,
        ] {
            let mut session = session();
            session.stage(Some(choice("cai wo", "裁我")), 1);
            session.commit("裁我", 1, 100);
            session.key(&key(kind), false, 5100);
            assert_eq!(session.entries().len(), 1, "{kind:?}");
        }
    }

    #[test]
    fn unavailable_segment_or_secure_transition_discards_the_group() {
        let mut session = session();
        session.stage(None, 1);
        session.stage(Some(choice("gao", "搞")), 2);
        session.commit("再搞", 2, 100);
        session.key(&key(KeyKind::Text), false, 5100);
        assert!(session.entries().is_empty());
        session.stage(Some(choice("cai wo", "裁我")), 3);
        session.commit("裁我", 3, 6000);
        let mut secure = key(KeyKind::Text);
        secure.security = SecurityState::Secure;
        session.key(&secure, false, 12000);
        session.key(&key(KeyKind::Text), false, 13000);
        assert!(session.entries().is_empty());
    }

    #[test]
    fn shared_directory_deduplicates_events_and_survives_restart() {
        let dir = scratch("shared");
        let mut first = LearningSession::default();
        let mut second = LearningSession::default();
        for session in [&mut first, &mut second] {
            session.attach(&dir);
            session.set_mode(LearningMode::Zhixu, &index());
            session.stage(Some(choice("cai wo", "裁我")), 7);
            session.commit("裁我", 7, 100);
        }
        first.key(&key(KeyKind::Text), false, 5100);
        second.key(&key(KeyKind::Text), false, 5100);
        assert_eq!(first.entries().len(), 1);
        assert_eq!(second.entries()[0].count, 1);
        assert_eq!(first.choices()[0].count, 1);
        drop(first);
        drop(second);
        let mut restarted = LearningSession::default();
        restarted.attach(&dir);
        restarted.set_mode(LearningMode::Zhixu, &index());
        // The timestamps above are intentionally old; persisted data must decay.
        assert!(restarted.entries().is_empty());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn retained_learning_survives_restart_and_expired_seeds_are_not_reimported() {
        let dir = scratch("restart");
        let now = crate::personal_picks::now_ms();
        let mut session = session();
        session.attach(&dir);
        session.set_mode(LearningMode::Zhixu, &index());
        session.stage(Some(choice("cai wo", "裁我")), 71);
        session.stage(Some(choice("cai wo", "裁我")), 71);
        session.commit("裁我", 71, now);
        session.key(&key(KeyKind::Text), false, now + RETENTION_MS);
        assert_eq!(session.entries()[0].count, 1);
        drop(session);
        let mut restarted = LearningSession::default();
        restarted.attach(&dir);
        restarted.set_mode(LearningMode::Zhixu, &index());
        assert_eq!(restarted.entries()[0].text, "裁我");
        assert_eq!(restarted.entries()[0].count, 1);
        restarted.stage(Some(choice("cai wo", "裁我")), 71);
        restarted.commit("裁我", 71, now);
        restarted.key(&key(KeyKind::Text), false, now + RETENTION_MS);
        assert_eq!(
            restarted.entries()[0].count,
            1,
            "replayed physical delivery is still claimed after restart"
        );
        drop(restarted);
        // Declare the removal against the version being replaced; a fresh
        // independent store now merges additions instead of overwriting peers.
        let mut expired = LearningStore::load(&dir, now + super::super::learning::MAX_AGE_MS + 1);
        expired.record("zai gao", "再搞", 1);
        expired.owner_seeded = true;
        expired.save(&dir).unwrap();
        let seeded = Index::from_data(DictionaryData {
            entries: vec![DictionaryEntry {
                text: "再搞".into(),
                reading: "zai gao".into(),
                weight: 800,
                layer: "agent_owner_words".into(),
            }],
        });
        let mut restarted = LearningSession::default();
        restarted.attach(&dir);
        restarted.set_mode(LearningMode::Zhixu, &seeded);
        assert!(restarted.entries().is_empty());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn withdraw_and_pending_limit_drop_unconfirmed_facts() {
        let mut session = session();
        for id in 1..=40 {
            session.stage(Some(choice("cai wo", "裁我")), id);
            session.commit("裁我", id, 100);
        }
        assert_eq!(session.pending.len(), MAX_PENDING);
        assert!(session.withdraw("cai'wo", "裁我，"));
        assert!(!session.withdraw("caiwo", "裁我"));
        session.key(&key(KeyKind::Text), false, 5100);
        assert!(session.entries().is_empty());
        session.stage(Some(choice("zai", "再")), 100);
        session.key(&key(KeyKind::Escape), true, 5200);
        session.stage(Some(choice("gao", "搞")), 101);
        session.commit("再搞", 101, 5300);
        session.key(&key(KeyKind::Text), false, 10300);
        assert!(session.entries().is_empty());
    }

    #[test]
    fn legacy_choices_are_imported_only_once() {
        let dir = scratch("legacy");
        let mut session = session();
        session.attach(&dir);
        session.set_mode(LearningMode::Zhixu, &index());
        let mut old = choice("cai wo", "裁我");
        old.last_ms = crate::personal_picks::now_ms();
        old.count = 4;
        session.import_choices(vec![old.clone()]);
        session.import_choices(vec![old.clone()]);
        assert_eq!(session.choices()[0].count, 4);
        drop(session);
        let mut restarted = LearningSession::default();
        restarted.attach(&dir);
        restarted.set_mode(LearningMode::Zhixu, &index());
        old.count = 8;
        restarted.import_choices(vec![old]);
        assert_eq!(restarted.choices()[0].count, 4);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn style_snapshot_is_reused_until_choices_change_or_expire() {
        let mut session = session();
        let empty = session.style().unwrap();
        assert!(Arc::ptr_eq(&empty, &session.style().unwrap()));
        let now = crate::personal_picks::now_ms();
        let mut settled = choice("de", "的");
        settled.raw = "d".into();
        settled.count = 10;
        settled.last_ms = now;
        settled.alternatives = vec!["到".into()];
        session.import_choices(vec![settled]);
        let learned = session.style().unwrap();
        assert!(!Arc::ptr_eq(&empty, &learned));
        assert_eq!(learned.letter("d"), Some("的"));
        assert!(Arc::ptr_eq(&learned, &session.style().unwrap()));
        session.key(
            &key(KeyKind::Text),
            false,
            now + super::super::learning::MAX_AGE_MS + 1,
        );
        assert_eq!(session.style().unwrap().letter("d"), None);
        assert!(!Arc::ptr_eq(&learned, &session.style().unwrap()));
    }

    fn migrated_session(count: usize) -> LearningSession {
        let session = session();
        let mut export = String::new();
        for offset in 0..count {
            export.push_str(&format!("裁{}\tcai wo\t1\n", char::from_u32(0x4e00 + offset as u32).unwrap()));
        }
        let mut knowledge = session.knowledge.lock().unwrap();
        assert_eq!(knowledge.store.import_rime_export(&export, 100).unwrap(), count);
        knowledge.store.record("zai gao", "再搞", 100);
        knowledge.changed();
        drop(knowledge);
        session
    }

    #[test]
    fn whole_overlay_is_reused_across_normal_overlay_queries() {
        let session = migrated_session(8);
        let whole = session.whole_overlay().unwrap();
        let normal = session.overlay().unwrap();
        assert!(!normal.vocabulary_word("裁一", "caiwo"));
        assert!(whole.vocabulary_word("裁一", "caiwo"));
        assert!(Arc::ptr_eq(&whole, &session.whole_overlay().unwrap()));
        assert!(Arc::ptr_eq(&normal, &session.overlay().unwrap()));
        session.remember_for_test("cai wo", "裁我", 101);
        let changed = session.whole_overlay().unwrap();
        assert!(!Arc::ptr_eq(&whole, &changed));
        assert!(changed.vocabulary_word("裁我", "caiwo"));
    }

    #[test]
    fn learning_revision_tracks_writes_expiry_and_shared_knowledge() {
        let mut session = session();
        let loaded = session.revision();
        session.remember_for_test("cai wo", "裁我", 100);
        let learned = session.revision();
        assert_ne!(loaded, learned);
        let whole = session.whole_overlay().unwrap();
        session.remember_for_test("cai wo", "裁我", 101);
        assert_ne!(learned, session.revision(), "counts affect whole candidate ranking");
        assert!(!Arc::ptr_eq(&whole, &session.whole_overlay().unwrap()));
        let current = session.revision();
        session.whole_overlay();
        session.overlay();
        assert_eq!(current, session.revision(), "queries do not change learning facts");
        session.key(&key(KeyKind::Text), false, 102 + super::super::learning::MAX_AGE_MS);
        assert_ne!(current, session.revision());
        assert!(session.whole_overlay().is_none());

        let first_dir = scratch("revision-first");
        let second_dir = scratch("revision-second");
        session.attach(&first_dir);
        let attached = session.revision();
        assert_ne!(current, attached);
        let mut peer = LearningSession::default();
        peer.attach(&first_dir);
        assert_eq!(attached, peer.revision());
        peer.set_mode(LearningMode::Zhixu, &index());
        assert_ne!(attached, session.revision(), "loading shared facts invalidates peers");
        peer.remember_for_test("zai gao", "再搞", 100);
        assert_eq!(session.revision(), peer.revision());
        session.attach(&second_dir);
        assert_ne!(session.revision(), peer.revision(), "different stores never alias versions");
        fs::remove_dir_all(first_dir).unwrap();
        fs::remove_dir_all(second_dir).unwrap();
    }

    /// Synthetic migrated entries only; run with `cargo test --offline --release whole_overlay_timing -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn whole_overlay_timing() {
        let session = migrated_session(512);
        let mut previous = session.whole_overlay().unwrap();
        let started = std::time::Instant::now();
        let mut rebuilt = 0;
        for _ in 0..100 {
            std::hint::black_box(session.overlay());
            let next = session.whole_overlay().unwrap();
            rebuilt += usize::from(!Arc::ptr_eq(&previous, &next));
            previous = next;
        }
        eprintln!("whole_overlay alternations=100 migrated_entries=512 changed_arcs={rebuilt} elapsed_us={}", started.elapsed().as_micros());
    }

    #[test]
    fn persist_refreshes_cached_views_with_a_peers_facts() {
        let dir = scratch("merge-cache");
        let now = crate::personal_picks::now_ms();
        let mut knowledge = Knowledge::new(Some(dir.clone()));
        knowledge.load();
        knowledge.store.record("cai wo", "裁我", now);
        knowledge.changed();
        assert!(!knowledge.overlay().unwrap().vocabulary_word("再搞", "zaigao"));
        let session = LearningSession { mode: LearningMode::Zhixu,
            knowledge: Arc::new(Mutex::new(Knowledge::new(None))), ..LearningSession::default() };
        // Use the same shared view that the mixer reads, including imported facts.
        let before_merge = knowledge.revision;
        let whole = Arc::new(Index::from_learning(knowledge.store.entries()));
        knowledge.whole_overlay = Some((before_merge, Some(whole.clone())));
        assert_eq!(knowledge.style().letter("d"), None);
        let mut peer = LearningStore::load(&dir, now);
        peer.record("zai gao", "再搞", now);
        let mut preference = choice("de", "的");
        preference.raw = "d".into();
        preference.count = 10;
        preference.last_ms = now;
        preference.alternatives = vec!["到".into()];
        peer.record_choice(preference);
        peer.save(&dir).unwrap();
        knowledge.persist();
        assert_ne!(before_merge, knowledge.revision);
        *session.knowledge.lock().unwrap() = knowledge;
        let merged = session.whole_overlay().unwrap();
        assert!(merged.vocabulary_word("再搞", "zaigao"));
        assert!(!Arc::ptr_eq(&whole, &merged));
        let mut knowledge = session.knowledge.lock().unwrap();
        assert!(!knowledge.dirty);
        assert!(knowledge.overlay().unwrap().vocabulary_word("再搞", "zaigao"));
        assert_eq!(knowledge.style().letter("d"), Some("的"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn rime_mode_never_loads_or_writes_and_owner_seed_can_arrive_later() {
        let dir = scratch("seed");
        let mut session = LearningSession::default();
        session.attach(&dir);
        session.stage(Some(choice("cai wo", "裁我")), 1);
        session.commit("裁我", 1, 100);
        session.key(&key(KeyKind::Text), false, 5100);
        assert!(session.entries().is_empty());
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 0);
        session.set_mode(LearningMode::Zhixu, &index());
        let seeded = Index::from_data(DictionaryData {
            entries: vec![DictionaryEntry {
                text: "再搞".into(),
                reading: "zai gao".into(),
                weight: 800,
                layer: "agent_owner_words".into(),
            }],
        });
        session.seed_if_needed(&seeded);
        session.seed_if_needed(&seeded);
        assert_eq!(session.entries().len(), 1);
        assert_eq!(session.entries()[0].count, 1);
        let first = session.overlay().unwrap();
        session.remember_for_test("zai gao", "再搞", crate::personal_picks::now_ms());
        assert!(
            Arc::ptr_eq(&first, &session.overlay().unwrap()),
            "counts do not change composition weights or rebuild its index"
        );
        drop(session);
        let mut restarted = LearningSession::default();
        restarted.attach(&dir);
        restarted.set_mode(LearningMode::Zhixu, &seeded);
        assert_eq!(
            restarted.entries()[0].count,
            1,
            "in-memory test increment was not persisted"
        );
        fs::remove_dir_all(dir).unwrap();
    }
}
