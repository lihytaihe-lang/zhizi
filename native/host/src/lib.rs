//! The platform-neutral half of a 知字 input method shell.
//!
//! A shell (Windows TSF now, Linux later) turns platform key events into [`Key`]s,
//! calls [`Host::process`], and draws the returned [`View`]: the composition text,
//! the candidate row, and any text to commit. Everything else lives here and is the
//! same on every platform: the core (Rime, mixer, slips, English words), the
//! 顺手·知字 ranking on top of it, and the quality records.

pub mod quality;
pub mod ranking;
pub mod settings;

use agent_ime_core::{
    AimCandidate, AimKeyEvent, AimResult, AimSession, aim_result_free,
    aim_session_composition_alternatives, aim_session_composition_edits, aim_session_composition_guessed,
    aim_session_context_complete_mask,
    aim_session_context_group_mask, aim_session_create, aim_session_destroy,
    aim_session_lead_with_composition, aim_session_offer_composition, aim_session_peek_following_pages,
    aim_session_process,
    aim_session_reorder_context_candidates, aim_session_set_mode, aim_session_set_preceding_text,
    aim_session_set_candidate_context, aim_session_set_typo_correction,
};
use ranking::{COMPOSITION_COMMENT, COMPOSITION_OFFER_COMMENT, Done, Job, Worker};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Keys the core understands. Digits, letters and punctuation travel as `Text`;
/// the core decides whether a digit selects a candidate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Key {
    Text(String),
    Backspace,
    Left,
    Right,
    Space,
    Enter,
    Escape,
    PageUp,
    PageDown,
    Up,
    Down,
    /// Shift pressed and released alone: toggles Chinese / English.
    ShiftTap,
    CapsLock,
    FocusLost,
    Deactivate,
    /// Remove the highlighted learned candidate (the shell's delete shortcut).
    DeleteCandidate,
    /// Never offer the highlighted candidate again (Ctrl+Shift+Backspace).
    BlacklistCandidate,
}

pub mod modifiers {
    pub const SHIFT: u32 = 1 << 0;
    pub const CONTROL: u32 = 1 << 1;
    pub const ALT: u32 = 1 << 2;
    pub const META: u32 = 1 << 3;
    pub const CAPS_LOCK: u32 = 1 << 4;
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Candidate {
    pub text: String,
    /// Source label such as 「整句 · 顺手·知字」; empty for ordinary words.
    pub comment: String,
}

/// What the shell draws after a key.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct View {
    /// False: the key was not for the input method; let the application have it.
    pub consumed: bool,
    /// Composition (pinyin being typed); empty when nothing is composing.
    pub preedit: String,
    /// Caret position inside `preedit`, in UTF-8 bytes.
    pub cursor_utf8: usize,
    /// Text to insert into the document now (may be non-empty with an empty preedit).
    pub commit: String,
    pub candidates: Vec<Candidate>,
    pub highlighted: usize,
    pub page: usize,
    pub is_last_page: bool,
    /// The core is in direct English mode.
    pub english_mode: bool,
}

pub struct HostConfig {
    /// Rime shared data shipped with the input method.
    pub shared_dir: PathBuf,
    /// Per-user Rime data (learning, blacklist, English choices).
    pub user_dir: PathBuf,
    /// Application identifier (process name); learning order is per application.
    pub app_id: String,
}

/// How long a key waits for 顺手·知字 before showing the decoder's order; a later
/// result arrives through `Host::poll_ranking`.
pub const RANKING_BUDGET: Duration = Duration::from_millis(15);

/// A segment picked mid-composition stays in front of the letters left (`到时候wo xuan yi xuan`);
/// it is not in the document yet, so the model reads it as part of the text before the caret.
fn with_picked_text(context: String, preedit: &str) -> String {
    match preedit.find(|c: char| c.is_ascii_alphabetic()) {
        Some(at) if !preedit[..at].is_ascii() => context + &preedit[..at],
        _ => context,
    }
}

/// What 顺手·知字 did for the page on screen (for quality records).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModelObservation {
    /// unchanged, reordered, composed, no_group, unavailable, pending
    pub outcome: &'static str,
    pub group_size: usize,
    pub latency_ms: Option<f64>,
    /// The result arrived after the page was first shown.
    pub late: bool,
}

struct Pending {
    generation: u64,
    view: View,
    slots: Vec<usize>,
    homophone: Vec<bool>,
    composition: Vec<String>,
    /// Letters changed to reach each composition entry from what was typed.
    edits: Vec<u8>,
    /// The typed letters are not whole pinyin (the first is a guess too).
    guessed: bool,
}

/// One C3 decision in full, for local evaluation (`Host::last_composition`).
#[derive(Clone, Debug, Default)]
pub struct CompositionTrace {
    pub texts: Vec<String>,
    pub edits: Vec<u8>,
    pub scores: Vec<f64>,
}

pub struct Host {
    session: *mut AimSession,
    recent: String,
    worker: Option<Worker>,
    generation: u64,
    pending: Option<Pending>,
    observation: ModelObservation,
    last_composition: CompositionTrace,
    recorder: Option<quality::Recorder>,
    app_id: String,
    /// Unique per key: the core claims each selection's ID once before Rime may learn.
    next_delivery_id: u64,
}

// The session is only touched through &mut self.
unsafe impl Send for Host {}

impl Host {
    pub fn open(config: &HostConfig) -> Result<Self, String> {
        let shared = path_bytes(&config.shared_dir)?;
        let user = path_bytes(&config.user_dir)?;
        let session = unsafe {
            aim_session_create(
                shared.as_ptr(),
                shared.len(),
                user.as_ptr(),
                user.len(),
                config.app_id.as_ptr(),
                config.app_id.len(),
            )
        };
        if session.is_null() {
            return Err("知字核心启动失败".into());
        }
        unsafe { aim_session_set_typo_correction(session, 1) };
        // The parallel second layer (digits 6-0) has no room on the shell's panels: with it on, the digits pick candidates
        // nobody can see. Off, like the Mac default; the shells that draw it can turn it on.
        unsafe { aim_session_set_candidate_context(session, std::ptr::null(), 0, b"[]".as_ptr(), 2, 0, 0) };
        Ok(Self {
            session,
            recent: String::new(),
            worker: None,
            generation: 0,
            pending: None,
            observation: ModelObservation::default(),
            last_composition: CompositionTrace::default(),
            recorder: None,
            app_id: config.app_id.clone(),
            // Start from the clock so IDs never repeat across restarts within the
            // ledger's window.
            next_delivery_id: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_micros() as u64)
                .unwrap_or(1)
                .max(1),
        })
    }

    /// Turns on quality records (test builds: on by default; uploaded hourly).
    pub fn enable_recording(&mut self, config: quality::RecorderConfig) {
        self.recorder = Some(quality::Recorder::new(config, &self.app_id));
    }

    /// The focused application changed (learning order and records are per application).
    pub fn set_app(&mut self, app_id: &str) {
        self.app_id = app_id.to_string();
        if let Some(recorder) = &mut self.recorder {
            recorder.set_app(app_id);
        }
        self.clear_context();
    }

    /// Turns on 顺手·知字. `notify` runs on the model thread when a late result is
    /// ready; the shell should then call `poll_ranking` on its own thread.
    pub fn enable_model(&mut self, scorer: Box<dyn ranking::Scorer>, notify: Option<Box<dyn Fn() + Send>>) {
        self.worker = Some(Worker::start(scorer, notify));
    }

    /// The owner's settled habits from their own quality records (one JSON per line).
    pub fn set_owner_style(&mut self, records: &str) {
        agent_ime_core::ranking::load_owner_style(records);
    }

    pub fn model_observation(&self) -> &ModelObservation {
        &self.observation
    }

    /// The last C3 decision's candidates, edits and scores (local evaluation only).
    pub fn last_composition(&self) -> &CompositionTrace {
        &self.last_composition
    }

    fn composition_edits(&mut self) -> Vec<u8> {
        let mut buffer = vec![0u8; 256];
        let written = unsafe { aim_session_composition_edits(self.session, buffer.as_mut_ptr(), buffer.len()) };
        if written <= 0 {
            return Vec::new();
        }
        buffer.truncate(written as usize);
        String::from_utf8_lossy(&buffer).split('\t').filter_map(|v| v.parse().ok()).collect()
    }

    /// A late 顺手·知字 result for the page on screen, if one arrived and no key came
    /// in between: the page to redraw.
    pub fn poll_ranking(&mut self) -> Option<View> {
        let generation = self.pending.as_ref()?.generation;
        let done = self.worker.as_ref()?.try_take(generation)?;
        let pending = self.pending.take()?;
        let view = self.apply_ranking(pending, done, true);
        if let Some(recorder) = &mut self.recorder {
            recorder.refresh(&view, &self.observation);
        }
        Some(view)
    }

    /// Feeds one key. `secure`: a password field; the core passes keys through and
    /// nothing is learned or recorded.
    pub fn process(&mut self, key: Key, modifiers: u32, secure: bool) -> View {
        let (kind, text) = key_parts(&key);
        let context = self.recent.clone();
        unsafe { aim_session_set_preceding_text(self.session, context.as_ptr(), context.len()) };
        let event = AimKeyEvent {
            kind,
            text: text.as_ptr(),
            text_len: text.len(),
            modifiers,
            security_state: if secure { 1 } else { 0 },
            delivery_id: {
                self.next_delivery_id += 1;
                self.next_delivery_id
            },
        };
        let mut result = std::mem::MaybeUninit::<AimResult>::uninit();
        let status = unsafe { aim_session_process(self.session, &event, result.as_mut_ptr()) };
        if status != 0 {
            return View::default();
        }
        let mut result = unsafe { result.assume_init() };
        let view = view_from(&result);
        let selected = result.selection_event_id != 0;
        unsafe { aim_result_free(&mut result) };
        let view = self.after_core(&key, view, context.clone(), secure);
        if let Some(recorder) = &mut self.recorder {
            recorder.observe(&key, &view, selected, &context, &self.observation, secure);
        }
        view
    }

    fn after_core(&mut self, key: &Key, view: View, context: String, secure: bool) -> View {
        if !view.commit.is_empty() && !secure {
            self.note_committed(&view.commit);
        }
        self.generation += 1;
        self.pending = None;
        // As on macOS: only keys that produce a fresh page are ranked; navigation
        // keeps the order the core stored.
        // Picking a segment (digit or Space) that leaves letters to convert shows a fresh page too.
        let picked_segment = !with_picked_text(String::new(), &view.preedit).is_empty()
            && match key {
                Key::Space => true,
                Key::Text(text) => text.chars().all(|c| c.is_ascii_digit()),
                _ => false,
            };
        let fresh_page = picked_segment
            || match key {
                Key::Text(text) => text.chars().all(|c| c.is_ascii_alphabetic() || c == '\''),
                Key::Backspace | Key::PageUp | Key::PageDown => true,
                _ => false,
            };
        if secure || !fresh_page || view.preedit.is_empty() || !view.commit.is_empty() {
            self.observation = ModelObservation { outcome: "unavailable", ..Default::default() };
            return view;
        }
        // A segment picked mid-composition is not in the document yet: the model reads it as context.
        let context = with_picked_text(context, &view.preedit);
        self.rank(view, context)
    }

    fn rank(&mut self, view: View, context: String) -> View {
        self.last_composition = CompositionTrace::default();
        if self.worker.is_none() {
            self.observation = ModelObservation { outcome: "unavailable", ..Default::default() };
            return view;
        }
        let complete = unsafe { aim_session_context_complete_mask(self.session) };
        let homophone_mask = unsafe { aim_session_context_group_mask(self.session) };
        let mask = complete | homophone_mask;
        let slots: Vec<usize> = (0..view.candidates.len().min(32)).filter(|i| mask & (1 << i) != 0).collect();
        let homophone: Vec<bool> = slots.iter().map(|i| homophone_mask & (1 << i) != 0).collect();
        let alternatives = self.composition_alternatives();
        let mut edits = self.composition_edits();
        if edits.len() != alternatives.len() + 1 {
            edits = vec![0; alternatives.len() + 1];
        }
        let composition = match view.candidates.first() {
            Some(first) if !alternatives.is_empty() => {
                std::iter::once(first.text.clone()).chain(alternatives).collect()
            }
            _ => Vec::new(),
        };
        if slots.len() < 2 && composition.is_empty() {
            self.observation = ModelObservation { outcome: "no_group", group_size: slots.len(), ..Default::default() };
            return view;
        }
        let group: Vec<String> = slots.iter().map(|&i| view.candidates[i].text.clone()).collect();
        let generation = self.generation;
        let worker = self.worker.as_ref().expect("checked above");
        worker.submit(Job { generation, context, composition: composition.clone(), group });
        let guessed = !composition.is_empty() && unsafe { aim_session_composition_guessed(self.session) } != 0;
        let pending = Pending { generation, view: view.clone(), slots, homophone, composition, edits, guessed };
        match worker.wait(generation, RANKING_BUDGET) {
            Some(done) => self.apply_ranking(pending, done, false),
            None => {
                self.observation = ModelObservation {
                    outcome: "pending",
                    group_size: pending.slots.len(),
                    ..Default::default()
                };
                self.pending = Some(pending);
                view
            }
        }
    }

    /// Which candidates of the current page the context model scores: those that cover the whole
    /// input, plus the homophone group (the masks the ranking uses). Evaluation / training data only.
    pub fn scored_page_slots(&self) -> Vec<usize> {
        let mask = unsafe { aim_session_context_complete_mask(self.session) | aim_session_context_group_mask(self.session) };
        (0..32).filter(|i| mask & (1 << i) != 0).collect()
    }

    /// The core's whole-reading compositions for the current input (C3 candidates).
    pub fn composition_alternatives(&mut self) -> Vec<String> {
        let mut buffer = vec![0u8; 4096];
        let written = unsafe {
            aim_session_composition_alternatives(self.session, buffer.as_mut_ptr(), buffer.len())
        };
        if written <= 0 {
            return Vec::new();
        }
        buffer.truncate(written as usize);
        String::from_utf8_lossy(&buffer)
            .split('\t')
            .filter(|text| !text.is_empty())
            .map(str::to_string)
            .collect()
    }

    fn apply_ranking(&mut self, pending: Pending, done: Done, late: bool) -> View {
        let latency_ms = Some((done.elapsed.as_secs_f64() * 100_000.0).round() / 100.0);
        let mut view = pending.view;
        let mut offer: Option<String> = None;
        // C3: a whole-reading composition that clearly beats the decoder's first,
        // scored as P(text | context) x P(keys | text): the model's log probability
        // minus a penalty per letter changed to reach it (zhuangixnban: 装新版, one swap,
        // over 换个新版, a several-letter heavy-garble guess the model alone prefers).
        if let Some(scores) = &done.composition_scores {
            self.last_composition = CompositionTrace {
                texts: pending.composition.clone(),
                edits: pending.edits.clone(),
                scores: scores.clone(),
            };
            let edits: Vec<f64> = (0..scores.len()).map(|i| f64::from(pending.edits.get(i).copied().unwrap_or(0))).collect();
            offer = ranking::composition_offer(&pending.composition, scores, &edits)
                .map(|best| pending.composition[best].clone());
            if let Some(best) = ranking::composition_lead(&pending.composition, scores, &edits, pending.guessed) {
                let text = &pending.composition[best];
                if unsafe { aim_session_lead_with_composition(self.session, text.as_ptr(), text.len()) } == 0 {
                    let lead = Candidate { text: text.clone(), comment: COMPOSITION_COMMENT.into() };
                    // A three-letter English word in the last slot (mac, key, env) keeps it.
                    let english = view.candidates.get(4).filter(|c| c.comment == "英文 · 原样" && c.text.len() == 3).cloned();
                    view.candidates = std::iter::once(lead)
                        .chain(view.candidates.into_iter().filter(|c| &c.text != text && Some(c) != english.as_ref()))
                        .take(if english.is_some() { 4 } else { 5 })
                        .chain(english.clone())
                        .collect();
                    view.highlighted = 0;
                    self.observation = ModelObservation {
                        outcome: "composed",
                        group_size: pending.composition.len(),
                        latency_ms,
                        late,
                    };
                    return view;
                }
            }
        }
        // C2 / C1 over the complete-coverage group.
        let Some(scores) = done.group_scores.filter(|_| pending.slots.len() >= 2) else {
            self.observation = ModelObservation { outcome: "no_group", group_size: pending.slots.len(), latency_ms, late };
            return self.offer_composition(view, offer);
        };
        let texts: Vec<String> = pending.slots.iter().map(|&slot| view.candidates[slot].text.clone()).collect();
        let group_order = ranking::group_order(&texts, &scores, &pending.homophone);
        let mut order: Vec<usize> = (0..view.candidates.len()).collect();
        for (rank, &slot) in pending.slots.iter().enumerate() {
            order[slot] = pending.slots[group_order[rank]];
        }
        let identity: Vec<usize> = (0..view.candidates.len()).collect();
        if order == identity
            || unsafe { aim_session_reorder_context_candidates(self.session, order.as_ptr(), order.len()) } != 0
        {
            self.observation = ModelObservation { outcome: "unchanged", group_size: pending.slots.len(), latency_ms, late };
            return self.offer_composition(view, offer);
        }
        view.candidates = order.iter().map(|&i| view.candidates[i].clone()).collect();
        self.observation = ModelObservation { outcome: "reordered", group_size: pending.slots.len(), latency_ms, late };
        self.offer_composition(view, offer)
    }

    /// No composition leads: the best one still takes the last first-page slot when it
    /// is close (`ranking::composition_offer`), as the core's own page has it.
    fn offer_composition(&mut self, mut view: View, offer: Option<String>) -> View {
        let Some(text) = offer else { return view };
        if view.page != 0 || view.candidates.iter().take(5).any(|candidate| candidate.text == text) {
            return view;
        }
        if unsafe { aim_session_offer_composition(self.session, text.as_ptr(), text.len()) } != 0 {
            return view;
        }
        let slot = 4.min(view.candidates.len());
        view.candidates.insert(slot, Candidate { text, comment: COMPOSITION_OFFER_COMMENT.into() });
        view.candidates.truncate(5);
        view
    }

    /// The candidates of the next `pages` pages without moving the core's page (the
    /// `+` expanded grid draws the current row plus these).
    pub fn peek_following_pages(&mut self, pages: usize) -> Vec<Candidate> {
        let mut buffer = vec![0u8; 64 * 1024];
        let length = unsafe {
            aim_session_peek_following_pages(self.session, pages, buffer.as_mut_ptr(), buffer.len())
        };
        if length <= 0 {
            return Vec::new();
        }
        buffer.truncate(length as usize);
        String::from_utf8_lossy(&buffer)
            .split('\n')
            .filter(|text| !text.is_empty())
            .map(|text| Candidate {
                text: text.to_string(),
                comment: String::new(),
            })
            .collect()
    }

    /// Typing-slip correction on or off (the shell's switch; on by default).
    pub fn set_typo_correction(&mut self, enabled: bool) {
        unsafe { aim_session_set_typo_correction(self.session, u8::from(enabled)) };
    }

    /// Starts the session in direct English (true) or Chinese mode: the mode remembered for the application.
    pub fn set_english_mode(&mut self, english: bool) {
        unsafe { aim_session_set_mode(self.session, u32::from(english)) };
    }

    /// The text the application really has before the caret (read by the shell through its text interface): the context the next
    /// candidates are ranked against and the punctuation direction follows. Empty text changes nothing, since some applications
    /// cannot be read at all and the shell then keeps what it committed itself.
    pub fn set_context(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.recent.clear();
        self.note_committed(text);
    }

    /// Text the shell committed by other means (or typed while the input method was
    /// in English): the context the next candidates are ranked against.
    pub fn note_committed(&mut self, text: &str) {
        self.recent.push_str(text);
        let excess = self.recent.chars().count().saturating_sub(32);
        if excess > 0 {
            self.recent = self.recent.chars().skip(excess).collect();
        }
    }

    /// The caret moved to another document or application: forget the context.
    pub fn clear_context(&mut self) {
        self.recent.clear();
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        unsafe { aim_session_destroy(self.session) };
    }
}

fn path_bytes(path: &Path) -> Result<Vec<u8>, String> {
    path.to_str()
        .map(|value| value.as_bytes().to_vec())
        .ok_or_else(|| format!("路径不是 UTF-8：{}", path.display()))
}

fn key_parts(key: &Key) -> (u32, &[u8]) {
    match key {
        Key::Text(text) => (0, text.as_bytes()),
        Key::Backspace => (1, &[]),
        Key::Left => (2, &[]),
        Key::Right => (3, &[]),
        Key::Space => (4, &[]),
        Key::Enter => (5, &[]),
        Key::Escape => (6, &[]),
        Key::PageUp => (7, &[]),
        Key::PageDown => (8, &[]),
        Key::Up => (9, &[]),
        Key::Down => (10, &[]),
        Key::ShiftTap => (11, &[]),
        Key::CapsLock => (12, &[]),
        Key::FocusLost => (13, &[]),
        Key::Deactivate => (14, &[]),
        Key::DeleteCandidate => (17, &[]),
        Key::BlacklistCandidate => (18, &[]),
    }
}

fn text(data: *const u8, length: usize) -> String {
    if data.is_null() || length == 0 {
        return String::new();
    }
    String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(data, length) }).into_owned()
}

fn view_from(result: &AimResult) -> View {
    let candidates = if result.candidates.is_null() {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(result.candidates, result.candidate_count) }
    };
    View {
        consumed: result.consumed != 0,
        preedit: text(result.marked_text, result.marked_text_len),
        cursor_utf8: result.marked_cursor_utf8,
        commit: text(result.commit_text, result.commit_text_len),
        candidates: candidates.iter().map(candidate).collect(),
        highlighted: result.highlighted_index,
        page: result.page_no,
        is_last_page: result.is_last_page != 0,
        english_mode: result.mode == 1,
    }
}

fn candidate(item: &AimCandidate) -> Candidate {
    Candidate {
        text: text(item.text, item.text_len),
        comment: text(item.comment, item.comment_len),
    }
}

#[cfg(test)]
mod picked_segment_tests {
    use super::with_picked_text;

    #[test]
    fn a_segment_picked_mid_composition_joins_the_model_context() {
        assert_eq!(with_picked_text("前文".into(), "到时候wo xuan yi xuan"), "前文到时候");
        assert_eq!(with_picked_text("前文".into(), "dao shi h"), "前文");
        assert_eq!(with_picked_text("前文".into(), "到时候"), "前文");
        assert_eq!(with_picked_text(String::new(), ""), "");
    }
}

#[cfg(test)]
mod context_tests {
    use super::*;

    #[test]
    fn the_document_text_replaces_what_the_shell_remembers() {
        let mut host = Host {
            session: std::ptr::null_mut(),
            recent: "旧的".into(),
            worker: None,
            generation: 0,
            pending: None,
            observation: ModelObservation::default(),
            last_composition: CompositionTrace::default(),
            recorder: None,
            app_id: String::new(),
            next_delivery_id: 1,
        };
        host.set_context("");
        assert_eq!(host.recent, "旧的", "unreadable or empty: keep what the shell has");
        host.set_context("今天天气不错，");
        assert_eq!(host.recent, "今天天气不错，");
        host.set_context(&"字".repeat(40));
        assert_eq!(host.recent.chars().count(), 32, "the context stays short");
        std::mem::forget(host); // no session to destroy
    }
}
