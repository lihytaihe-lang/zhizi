//! The platform-neutral half of a 知字 input method shell.
//!
//! A shell (Windows TSF now, Linux later) turns platform key events into [`Key`]s,
//! calls [`Host::process`], and draws the returned [`View`]: the composition text,
//! the candidate row, and any text to commit. Everything else lives here and is the
//! same on every platform: the core (the Zhixu decoder, mixer, slips, English words),
//! the 顺手·知字 ranking on top of it, and the quality records.

pub mod quality;
pub mod ranking;
pub mod settings;
pub mod zhiju;
pub mod generation;

/// The learning provider: Zhixu only. The retired Rime rule stays rejected by the
/// core ABI (`aim_session_set_learning(0)` answers -2).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LearningMode {
    #[default]
    Zhixu = 1,
}

use agent_ime_core::{
    AimCandidate, AimKeyEvent, AimResult, AimSession, aim_result_free,
    aim_session_composition_alternatives, aim_session_composition_edits, aim_session_composition_guessed,
    aim_session_context_complete_mask,
    aim_session_context_group_mask, aim_session_create, aim_session_current_page, aim_session_destroy,
    aim_session_lead_with_composition, aim_session_offer_composition, aim_session_page_texts,
    aim_session_peek_following_pages,
    aim_session_process,
    aim_session_reorder_context_candidates, aim_session_set_mode, aim_session_set_preceding_text,
    aim_session_set_candidate_context, aim_session_set_typo_correction, aim_session_set_learning, aim_session_set_fuzzy,
    aim_session_abbreviation_alternatives, aim_session_offer_abbreviation,
};
use ranking::{Done, Job, Worker};
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
    /// Dictionary data shipped with the input method (the `Rime` resource directory).
    pub shared_dir: PathBuf,
    /// Per-user data (learning, blacklist, English choices).
    pub user_dir: PathBuf,
    /// Application identifier (process name); learning order is per application.
    pub app_id: String,
}

/// Initial wait for synchronous callers. Later results arrive through
/// `Host::poll_ranking`.
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

/// Which part of the model's decisions to apply (local evaluation only; the shells never set it).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Ablation {
    /// Both: whole-reading compositions (C3) and the group order (C2/C1).
    #[default]
    None,
    /// Group order only; compositions are never led with or offered.
    RerankOnly,
    /// Compositions only; the group order is left as the core made it.
    JoinOnly,
}

struct Pending {
    generation: u64,
    context: String,
    view: View,
    slots: Vec<usize>,
    homophone: Vec<bool>,
    composition: Vec<String>,
    /// Normalized typing cost for each composition entry.
    edits: Vec<f64>,
    /// The typed letters are not whole pinyin (the first is a guess too).
    guessed: bool,
}

struct PendingOffer {
    generation: u64,
    view: View,
    texts: Vec<String>,
}

/// One C3 decision in full, for local evaluation (`Host::last_composition`).
#[derive(Clone, Debug, Default)]
pub struct CompositionTrace {
    pub texts: Vec<String>,
    pub edits: Vec<f64>,
    pub scores: Vec<f64>,
}

/// The C2/C1 group the model scored for the last key (local evaluation only).
#[derive(Clone, Debug, Default)]
pub struct GroupTrace {
    pub texts: Vec<String>,
    pub scores: Vec<f64>,
    pub homophone: Vec<bool>,
}

pub struct Host {
    session: *mut AimSession,
    recent: String,
    worker: Option<Worker>,
    ranking_wait: Duration,
    generation: u64,
    pending: Option<Pending>,
    pending_offer: Option<PendingOffer>,
    zhiju_worker: Option<generation::Worker>,
    zhiju_enabled: bool,
    context_ranking_enabled: bool,
    pending_generation: Option<generation::Done>,
    pending_verification: Option<(generation::Done, String)>,
    pending_gen_offer: Option<(View, String)>,
    generation_observation: generation::Observation,
    observation: ModelObservation,
    last_composition: CompositionTrace,
    last_group: GroupTrace,
    ablation: Ablation,
    recorder: Option<quality::Recorder>,
    app_id: String,
    /// Unique per key: the core claims each selection's ID once before the learning
    /// store may retain it.
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
            ranking_wait: RANKING_BUDGET,
            generation: 0,
            pending: None,
            pending_offer: None,
            zhiju_worker: None,
            zhiju_enabled: true,
            context_ranking_enabled: true,
            pending_generation: None,
            pending_verification: None,
            pending_gen_offer: None,
            generation_observation: generation::Observation::default(),
            observation: ModelObservation::default(),
            last_composition: CompositionTrace::default(),
            last_group: GroupTrace::default(),
            ablation: Ablation::None,
            recorder: None,
            app_id: config.app_id.clone(),
            // Start from the clock so IDs never repeat across restarts within the
            // learning store's window.
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

    /// IBus serves several applications in one process. Recreate the core session
    /// with its real learning domain while retaining the loaded model workers.
    /// The caller reapplies the application's saved switches and input mode.
    pub fn change_application(&mut self, config: &HostConfig) -> Result<(), String> {
        let mut fresh = Self::open(config)?;
        self.process(Key::FocusLost, 0, false);
        std::mem::swap(&mut self.session, &mut fresh.session);
        self.set_app(&config.app_id);
        Ok(())
    }

    /// Turns on 顺手·知字. `notify` runs on the model thread when a late result is
    /// ready; the shell should then call `poll_ranking` on its own thread.
    pub fn enable_model(&mut self, scorer: Box<dyn ranking::Scorer>, notify: Option<Box<dyn Fn() + Send>>) {
        self.start_ranking(scorer, notify, RANKING_BUDGET);
    }

    /// For shells that redraw from a model notification on their input thread.
    /// Publish the decoder's page immediately so a key callback never waits on inference.
    pub fn enable_deferred_model(&mut self, scorer: Box<dyn ranking::Scorer>, notify: Box<dyn Fn() + Send>) {
        self.start_ranking(scorer, Some(notify), Duration::ZERO);
    }

    fn start_ranking(
        &mut self,
        scorer: Box<dyn ranking::Scorer>,
        notify: Option<Box<dyn Fn() + Send>>,
        wait: Duration,
    ) {
        let worker = Worker::start(scorer, notify);
        worker.advance(self.generation);
        self.worker = Some(worker);
        self.ranking_wait = wait;
    }

    /// Loads the independent 知句 worker; call from the shell's loader, never a key callback.
    pub fn enable_zhiju(&mut self, vocabulary: &Path, model: Box<dyn generation::Distribution>,
        notify: Option<Box<dyn Fn() + Send>>) -> Result<(), String> {
        let worker = generation::Worker::start(vocabulary, model, notify)?;
        worker.advance(self.generation);
        self.zhiju_worker = Some(worker);
        Ok(())
    }

    pub fn set_zhiju_enabled(&mut self, enabled: bool) {
        self.zhiju_enabled = enabled;
        if !enabled {
            if let Some(worker) = &self.zhiju_worker { worker.advance(u64::MAX); worker.clear(); }
            self.pending_generation = None;
            self.pending_verification = None;
            self.pending_gen_offer = None;
        }
    }

    pub fn set_context_ranking_enabled(&mut self, enabled: bool) {
        self.context_ranking_enabled = enabled;
        if !enabled {
            self.pending = None;
            self.pending_offer = None;
            if let Some(worker) = &self.worker { worker.advance(u64::MAX); }
            if let Some((done, _)) = self.pending_verification.take() { self.pending_generation = Some(done); }
        }
    }

    /// The owner's settled habits from their own quality records (one JSON per line).
    pub fn set_owner_style(&mut self, records: &str) {
        agent_ime_core::ranking::load_owner_style(records);
    }

    /// Selects the learning provider. Zhixu is the only provider: every session
    /// learns Zhixu from the start, and the core ABI rejects every other mode.
    pub fn set_learning(&mut self, mode: LearningMode) -> bool {
        unsafe { aim_session_set_learning(self.session, mode as u32) == 0 }
    }

    /// Enables fuzzy-pinyin groups (bit 0 z/zh … bit 7 in/ing), as the menu does.
    pub fn set_fuzzy(&mut self, mask: u32) -> bool {
        unsafe { aim_session_set_fuzzy(self.session, mask) == 0 }
    }

    /// Local evaluation: apply only the group order or only the compositions.
    pub fn set_ablation(&mut self, ablation: Ablation) {
        self.ablation = ablation;
    }

    pub fn model_observation(&self) -> &ModelObservation {
        &self.observation
    }

    pub fn generation_observation(&self) -> &generation::Observation { &self.generation_observation }

    /// The last C3 decision's candidates, edits and scores (local evaluation only).
    pub fn last_composition(&self) -> &CompositionTrace {
        &self.last_composition
    }

    /// The last C2/C1 group's texts, scores and homophone flags (local evaluation only).
    pub fn last_group(&self) -> &GroupTrace {
        &self.last_group
    }

    fn composition_edits(&mut self) -> Vec<f64> {
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
        let view = self.poll_models()?;
        if let Some(recorder) = &mut self.recorder {
            recorder.refresh(&view, &self.observation);
        }
        Some(view)
    }

    fn poll_models(&mut self) -> Option<View> {
        if let Some(pending) = &self.pending {
            if let Some(done) = self.worker.as_ref().and_then(|w| w.try_take(pending.generation)) {
                let pending = self.pending.take()?;
                return Some(self.finish_ranking(pending, done, true));
            }
        }
        if let Some(done) = self.worker.as_ref().and_then(|w| w.try_comparison(self.generation)) {
            match done.id {
                0 => {
                    if let Some(pending) = self.pending_offer.take() {
                        if pending.generation == self.generation && self.page_matches(&pending.view) {
                            if let Some(scores) = done.scores.filter(|s| s.len() == pending.texts.len()) {
                                let best = (0..scores.len().saturating_sub(1)).max_by(|&a, &b| scores[a].total_cmp(&scores[b]));
                                if let Some(best) = best.filter(|&i| done.elapsed <= Duration::from_millis(8)
                                    && scores[i] > scores[scores.len() - 1] + 2.0) {
                                    let text = &pending.texts[best];
                                    if unsafe { aim_session_offer_abbreviation(self.session, text.as_ptr(), text.len()) } == 0 {
                                        return Some(self.current_page(pending.view));
                                    }
                                }
                            }
                        }
                    }
                }
                1 => {
                    if let Some((generated, native)) = self.pending_verification.take() {
                        if self.zhiju_enabled && self.core_page_texts().first() == Some(&native) {
                            let texts = [generated.candidates[0].text.clone(), native];
                            let guessed = unsafe { aim_session_composition_guessed(self.session) } != 0;
                            let veto = done.elapsed <= Duration::from_millis(8) && done.scores.as_ref().is_some_and(|scores|
                                ranking::composition_lead(&texts, scores, &[0.0, 0.0], guessed) == Some(1));
                            return self.apply_generation(generated, veto);
                        }
                    }
                }
                2 => {
                    if let Some((view, text)) = self.pending_gen_offer.take() {
                        if self.zhiju_enabled && self.page_matches(&view) && done.elapsed <= Duration::from_millis(8) {
                            if let Some(scores) = done.scores.filter(|s| s.len() == 3) {
                                if ranking::composition_offer(&[view.candidates[0].text.clone(), text.clone()], &scores[..2], &[0.0, 0.0]) == Some(1)
                                    && scores[1] > scores[2] + 2.0 {
                                    return Some(self.offer_composition(view, Some(text)));
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        if let Some(done) = self.zhiju_worker.as_ref().and_then(|w| w.take(self.generation)) {
            self.generation_observation = done.observation.clone();
            if !done.candidates.is_empty() { self.pending_generation = Some(done); }
        }
        // A fast GEN result waits for the primary page; a partial native word's
        // shorter likelihood must never veto a completed sentence.
        if self.pending.is_none() && self.zhiju_enabled {
            if let Some(done) = self.pending_generation.take() {
                let native = self.core_page_texts().first()?.clone();
                if self.context_ranking_enabled && self.worker.is_some() && self.observation.outcome == "composed"
                    && native != done.candidates[0].text {
                    self.worker.as_ref()?.compare(ranking::Comparison { id: 1, generation: self.generation,
                        context: done.job.context.clone(), texts: vec![done.candidates[0].text.clone(), native.clone()] });
                    self.pending_verification = Some((done, native));
                } else { return self.apply_generation(done, false); }
            }
        }
        None
    }

    fn page_matches(&self, view: &View) -> bool {
        self.core_page_texts() == view.candidates.iter().map(|c| c.text.clone()).collect::<Vec<_>>()
    }

    fn apply_generation(&mut self, done: generation::Done, veto: bool) -> Option<View> {
        if done.job.generation != self.generation || done.elapsed >= generation::BUDGET || !self.zhiju_enabled { return None; }
        let mut result = AimResult::default();
        let status = unsafe { agent_ime_core::aim_session_apply_zhiju_snapshot(self.session,
            done.job.snapshot.0, i32::from(!veto), &mut result) };
        let mut view = view_from(&result);
        unsafe { aim_result_free(&mut result); }
        if status != 0 && status > -10 { return None; }
        view.consumed = done.job.original.consumed;
        view.commit = done.job.original.commit;
        if !veto && status == 0 {
            self.generation_observation.outcome = "applied";
            self.pending_offer = None;
            if let Some(first) = view.candidates.first_mut() { first.comment = "整句 · 知句".into(); }
        } else if !veto && status <= -10 && self.context_ranking_enabled && self.worker.is_some()
            && view.page == 0 && view.candidates.len() == 5 {
            let text = done.candidates[0].text.clone();
            if !view.candidates.iter().any(|c| c.text == text) {
                self.worker.as_ref()?.compare(ranking::Comparison { id: 2, generation: self.generation,
                    context: done.job.context, texts: vec![view.candidates[0].text.clone(), text.clone(), view.candidates[4].text.clone()] });
                self.pending_gen_offer = Some((view.clone(), text));
            }
        }
        if veto { self.generation_observation.outcome = "vetoed"; }
        else if status != 0 { self.generation_observation.outcome = "rejected"; }
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

    /// Reads the page after a model decision, preserving the key event's result.
    fn current_page(&mut self, original: View) -> View {
        let mut result = std::mem::MaybeUninit::<AimResult>::uninit();
        if unsafe { aim_session_current_page(self.session, result.as_mut_ptr()) } != 0 {
            return original;
        }
        let mut result = unsafe { result.assume_init() };
        let mut view = view_from(&result);
        unsafe { aim_result_free(&mut result) };
        view.consumed = original.consumed;
        view.commit = original.commit;
        view
    }

    fn after_core(&mut self, key: &Key, view: View, context: String, secure: bool) -> View {
        if !view.commit.is_empty() && !secure {
            self.note_committed(&view.commit);
        }
        self.generation += 1;
        self.pending = None;
        self.pending_offer = None;
        self.pending_generation = None;
        self.pending_verification = None;
        self.pending_gen_offer = None;
        self.generation_observation = generation::Observation { outcome: "not_applicable", ..Default::default() };
        if let Some(worker) = &self.worker { worker.advance(self.generation); }
        if let Some(worker) = &self.zhiju_worker { worker.advance(self.generation); }
        if secure || view.preedit.is_empty() || matches!(key, Key::FocusLost | Key::Deactivate | Key::Escape) {
            if let Some(worker) = &self.zhiju_worker { worker.clear(); }
        }
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
        if self.zhiju_enabled && view.page == 0 {
            if let Some(worker) = &self.zhiju_worker {
                let mut snapshot = std::ptr::null_mut();
                if unsafe { agent_ime_core::aim_session_zhiju_snapshot(self.session, &mut snapshot) } == 0 && !snapshot.is_null() {
                    worker.submit(generation::Job { generation: self.generation, snapshot: generation::Snapshot(snapshot),
                        original: view.clone(), context: context.clone() });
                    self.generation_observation.outcome = "pending";
                }
            }
        }
        self.rank(view, context)
    }

    fn rank(&mut self, view: View, context: String) -> View {
        self.last_composition = CompositionTrace::default();
        self.last_group = GroupTrace::default();
        if self.worker.is_none() || !self.context_ranking_enabled {
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
            edits = vec![0.0; alternatives.len() + 1];
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
        worker.submit(Job { generation, context: context.clone(), composition: composition.clone(), group });
        let guessed = !composition.is_empty() && unsafe { aim_session_composition_guessed(self.session) } != 0;
        let pending = Pending { generation, context, view: view.clone(), slots, homophone, composition, edits, guessed };
        let ready = if self.ranking_wait.is_zero() {
            worker.try_take(generation)
        } else {
            worker.wait(generation, self.ranking_wait)
        };
        match ready {
            Some(done) => self.finish_ranking(pending, done, false),
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

    fn finish_ranking(&mut self, pending: Pending, done: Done, late: bool) -> View {
        let context = pending.context.clone();
        let view = self.apply_ranking(pending, done, late);
        if view.page == 0 && view.candidates.len() == 5
            && matches!(self.observation.outcome, "unchanged" | "reordered" | "composed") {
            let mut buffer = vec![0u8; 4096];
            let length = unsafe { aim_session_abbreviation_alternatives(self.session, buffer.as_mut_ptr(), buffer.len()) };
            if length > 0 && length as usize <= buffer.len() {
                let mut texts: Vec<String> = String::from_utf8_lossy(&buffer[..length as usize]).split('\t')
                    .filter(|s| !s.is_empty()).map(str::to_string).collect();
                // Bind the alternatives to the actual post-C2/C3 fifth, scored
                // alongside them rather than using a score from an earlier batch.
                if !texts.is_empty() {
                    texts.push(view.candidates[4].text.clone());
                    self.worker.as_ref().unwrap().compare(ranking::Comparison {
                        id: 0, generation: self.generation, context, texts: texts.clone(),
                    });
                    self.pending_offer = Some(PendingOffer { generation: self.generation, view: view.clone(), texts });
                }
            }
        }
        view
    }

    /// Which candidates of the current page the context model scores: those that cover the whole
    /// input, plus the homophone group (the masks the ranking uses). Evaluation / training data only.
    pub fn scored_page_slots(&self) -> Vec<usize> {
        let mask = unsafe { aim_session_context_complete_mask(self.session) | aim_session_context_group_mask(self.session) };
        (0..32).filter(|i| mask & (1 << i) != 0).collect()
    }

    /// Texts of the page digit keys commit (`aim_session_page_texts`). Read-only: one retry when the
    /// buffer is short; an empty page is an empty vec.
    pub fn core_page_texts(&self) -> Vec<String> {
        let mut capacity = 2048usize;
        for _ in 0..2 {
            let mut buffer = vec![0u8; capacity];
            let written = unsafe {
                aim_session_page_texts(self.session, buffer.as_mut_ptr(), buffer.len())
            };
            if written >= 0 {
                buffer.truncate(written as usize);
                let text = String::from_utf8_lossy(&buffer);
                if text.is_empty() {
                    return Vec::new();
                }
                return text.split('\t').map(str::to_string).collect();
            }
            let Some(needed) = written.checked_neg() else {
                return Vec::new();
            };
            capacity = needed as usize;
        }
        Vec::new()
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

    fn apply_ranking(&mut self, pending: Pending, mut done: Done, late: bool) -> View {
        match self.ablation {
            Ablation::None => {}
            Ablation::RerankOnly => done.composition_scores = None,
            Ablation::JoinOnly => done.group_scores = None,
        }
        let latency_ms = Some((done.elapsed.as_secs_f64() * 100_000.0).round() / 100.0);
        let view = pending.view;
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
            let edits: Vec<f64> = (0..scores.len()).map(|i| pending.edits.get(i).copied().unwrap_or(0.0)).collect();
            offer = ranking::composition_offer(&pending.composition, scores, &edits)
                .map(|best| pending.composition[best].clone());
            if let Some(best) = ranking::composition_lead(&pending.composition, scores, &edits, pending.guessed) {
                let text = &pending.composition[best];
                if unsafe { aim_session_lead_with_composition(self.session, text.as_ptr(), text.len()) } == 0 {
                    self.observation = ModelObservation {
                        outcome: "composed",
                        group_size: pending.composition.len(),
                        latency_ms,
                        late,
                    };
                    return self.current_page(view);
                }
            }
        }
        // C2 / C1 over the complete-coverage group.
        let Some(scores) = done.group_scores.filter(|_| pending.slots.len() >= 2) else {
            self.observation = ModelObservation { outcome: "no_group", group_size: pending.slots.len(), latency_ms, late };
            return self.offer_composition(view, offer);
        };
        let texts: Vec<String> = pending.slots.iter().map(|&slot| view.candidates[slot].text.clone()).collect();
        self.last_group = GroupTrace { texts: texts.clone(), scores: scores.clone(), homophone: pending.homophone.clone() };
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
        let view = self.current_page(view);
        self.observation = ModelObservation { outcome: "reordered", group_size: pending.slots.len(), latency_ms, late };
        self.offer_composition(view, offer)
    }

    /// No composition leads: the best one still takes the last first-page slot when it
    /// is close (`ranking::composition_offer`), as the core's own page has it.
    fn offer_composition(&mut self, view: View, offer: Option<String>) -> View {
        let Some(text) = offer else { return view };
        if view.page != 0 || view.candidates.iter().take(5).any(|candidate| candidate.text == text) {
            return view;
        }
        if unsafe { aim_session_offer_composition(self.session, text.as_ptr(), text.len()) } != 0 {
            return view;
        }
        self.current_page(view)
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
    /// candidates are ranked against and the punctuation direction follows. Empty text clears the context; when the application
    /// cannot be read, the shell must skip this call to keep what it committed itself.
    pub fn set_context(&mut self, text: &str) {
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
        self.generation += 1;
        self.pending = None;
        self.pending_offer = None;
        self.pending_generation = None;
        self.pending_verification = None;
        self.pending_gen_offer = None;
        if let Some(worker) = &self.worker { worker.advance(self.generation); }
        if let Some(worker) = &self.zhiju_worker { worker.advance(self.generation); worker.clear(); }
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
            ranking_wait: RANKING_BUDGET,
            generation: 0,
            pending: None,
            pending_offer: None,
            zhiju_worker: None,
            zhiju_enabled: true,
            context_ranking_enabled: true,
            pending_generation: None,
            pending_verification: None,
            pending_gen_offer: None,
            generation_observation: generation::Observation::default(),
            observation: ModelObservation::default(),
            last_composition: CompositionTrace::default(),
            last_group: GroupTrace::default(),
            ablation: Ablation::None,
            recorder: None,
            app_id: String::new(),
            next_delivery_id: 1,
        };
        assert_eq!(LearningMode::default(), LearningMode::Zhixu);
        assert!(!host.set_learning(LearningMode::Zhixu), "a closed session cannot configure learning");
        assert_ne!(unsafe { aim_session_set_learning(host.session, 2) }, 0);
        host.set_context("");
        assert_eq!(host.recent, "", "a successfully read empty document clears stale context");
        host.set_context("今天天气不错，");
        assert_eq!(host.recent, "今天天气不错，");
        host.set_context(&"字".repeat(40));
        assert_eq!(host.recent.chars().count(), 32, "the context stays short");
        std::mem::forget(host); // no session to destroy
    }
}

#[cfg(test)]
mod page_texts_tests {
    use super::*;

    static SESSIONS: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Small, public test words for page reads and forced model decisions.
    fn minimal_directories(name: &str) -> (PathBuf, PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "zhizi-host-page-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let shared = root.join("shared");
        let user = root.join("user");
        std::fs::create_dir_all(&shared).unwrap();
        std::fs::create_dir_all(&user).unwrap();
        std::fs::write(
            shared.join("agent_pinyin.dict.yaml"),
            "---\nname: agent_pinyin\nsort: by_weight\nimport_tables:\n  - pinyin_simp\n...\n",
        )
        .unwrap();
        std::fs::write(
            shared.join("pinyin_simp.dict.yaml"),
            concat!(
                "---\nname: pinyin_simp\nsort: by_weight\n...\n你\tni\t100\n好\thao\t80\n",
                "在\tzai\t1000\n再\tzai\t900\n等\tdeng\t1000\n我\two\t1000\n一下\tyi xia\t1000\n",
                "泥\tni\t90\n拟\tni\t80\n逆\tni\t70\n腻\tni\t60\n",
                "跑\tpao\t1000\n泡\tpao\t900\n抛\tpao\t800\n炮\tpao\t700\n",
                "到\tdao\t1000\n道\tdao\t900\n发\tfa\t1000\n法\tfa\t900\n",
                "实施\tshi shi\t30000\n事实\tshi shi\t29000\n试试\tshi shi\t28000\n实时\tshi shi\t27000\n适时\tshi shi\t26000\n",
                "史实\tshi shi\t25000\n失实\tshi shi\t24000\n逝世\tshi shi\t23000\n失势\tshi shi\t22000\n时事\tshi shi\t21000\n",
                "外\twai\t100\n们\tmen\t100\n想\txiang\t100\n星\txing\t90\n吃\tchi\t100\n菜\tcai\t100\n我们\two men\t100\n",
            ),
        )
        .unwrap();
        (root, shared, user)
    }

    #[test]
    fn core_page_texts_is_empty_then_matches_the_process_page() {
        let _serial = SESSIONS.lock().unwrap_or_else(|error| error.into_inner());
        let (root, shared, user) = minimal_directories("texts");
        let mut host = Host::open(&HostConfig {
            shared_dir: shared,
            user_dir: user,
            app_id: "zhizi.page-texts-test".into(),
        })
        .expect("open");
        assert_eq!(host.core_page_texts(), Vec::<String>::new());
        host.process(Key::Text("n".into()), 0, false);
        let view = host.process(Key::Text("i".into()), 0, false);
        let shown: Vec<String> = view.candidates.iter().map(|candidate| candidate.text.clone()).collect();
        let core = host.core_page_texts();
        let count = core.len().min(shown.len());
        assert!(count > 0, "typing ni yields candidates, core={core:?} shown={shown:?}");
        assert_eq!(&core[..count], &shown[..count], "core={core:?} shown={shown:?}");
        drop(host);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn application_change_replaces_core_domain_and_clears_pending_page_but_retains_models() {
        let _serial = SESSIONS.lock().unwrap_or_else(|error| error.into_inner());
        let (root, shared, user) = minimal_directories("ibus-app-change");
        let config = |app: &str| HostConfig {
            shared_dir: shared.clone(), user_dir: user.clone(), app_id: app.into(),
        };
        let mut host = Host::open(&config("gtk3-im:first")).unwrap();
        host.enable_model(Box::new(OffpageScorer { delay: Duration::ZERO }), None);
        host.process(Key::Text("n".into()), 0, false);
        host.set_context("上一个应用");
        let old_session = host.session;
        host.change_application(&config("gtk3-im:second")).unwrap();
        assert_ne!(host.session, old_session);
        assert_eq!(host.app_id, "gtk3-im:second");
        assert!(host.recent.is_empty() && host.pending.is_none());
        assert!(host.worker.is_some());
        assert!(host.core_page_texts().is_empty());
        assert!(host.process(Key::Text("i".into()), 0, false).preedit.starts_with('i'));
        drop(host);
        std::fs::remove_dir_all(root).unwrap();
    }

    fn with_input(name: &str, input: &str, check: impl FnOnce(&mut Host, View)) {
        let _serial = SESSIONS.lock().unwrap_or_else(|error| error.into_inner());
        let (root, shared, user) = minimal_directories(name);
        let mut host = Host::open(&HostConfig {
            shared_dir: shared,
            user_dir: user,
            app_id: format!("zhizi.page-{name}-test"),
        }).expect("open");
        host.set_context("现在这个");
        let mut view = View::default();
        for character in input.chars() {
            view = host.process(Key::Text(character.to_string()), 0, false);
        }
        assert!(!view.candidates.is_empty(), "{input}");
        check(&mut host, view);
        drop(host);
        std::fs::remove_dir_all(root).unwrap();
    }

    fn apply_scores(host: &mut Host, view: View, preferred: Option<(&str, f64)>, group_first: Option<usize>) -> View {
        let mask = unsafe { aim_session_context_complete_mask(host.session) | aim_session_context_group_mask(host.session) };
        let homophone_mask = unsafe { aim_session_context_group_mask(host.session) };
        let slots: Vec<usize> = (0..view.candidates.len()).filter(|i| mask & (1 << i) != 0).collect();
        let homophone = slots.iter().map(|i| homophone_mask & (1 << i) != 0).collect();
        let composition: Vec<String> = std::iter::once(view.candidates[0].text.clone())
            .chain(host.composition_alternatives()).collect();
        let composition_scores = preferred.map(|(text, margin)| {
            assert!(composition.iter().skip(1).any(|candidate| candidate == text), "{composition:?}");
            composition.iter().map(|candidate| if candidate == text { 0.0 } else { -margin }).collect()
        });
        let group_scores = group_first.map(|first| {
            assert!(slots.len() >= 2, "{slots:?}");
            (0..slots.len()).map(|index| if index == first { 0.0 } else { -400.0 }).collect()
        });
        let pending = Pending {
            generation: host.generation,
            context: host.recent.clone(),
            view,
            slots,
            homophone,
            composition,
            edits: host.composition_edits(),
            guessed: unsafe { aim_session_composition_guessed(host.session) } != 0,
        };
        host.apply_ranking(pending, Done {
            generation: host.generation,
            composition_scores,
            group_scores,
            elapsed: Duration::from_millis(1),
        }, false)
    }

    fn assert_core_page(host: &Host, view: &View) {
        let shown: Vec<_> = view.candidates.iter().map(|candidate| candidate.text.clone()).collect();
        assert_eq!(shown, host.core_page_texts());
        let mut result = AimResult::default();
        assert_eq!(unsafe { agent_ime_core::aim_session_current_page(host.session, &mut result) }, 0);
        let core = view_from(&result);
        unsafe { aim_result_free(&mut result) };
        assert_eq!(view.candidates, core.candidates);
        assert_eq!(view.preedit, core.preedit);
        assert_eq!(view.cursor_utf8, core.cursor_utf8);
        assert_eq!(view.highlighted, core.highlighted);
        assert_eq!(view.page, core.page);
        assert_eq!(view.is_last_page, core.is_last_page);
        assert_eq!(view.english_mode, core.english_mode);
        assert!(view.consumed, "reading the page preserves the consumed key");
    }

    #[test]
    fn model_led_sentence_matches_the_core_page() {
        with_input("lead", "zaidengwoyixia", |host, view| {
            let view = apply_scores(host, view, Some(("再等我一下", 40.0)), None);
            assert_eq!(host.observation.outcome, "composed");
            assert_eq!(view.candidates[0].text, "再等我一下");
            assert_eq!(view.candidates[0].comment, agent_ime_core::ranking::COMPOSITION_COMMENT);
            assert_core_page(host, &view);
        });
    }

    #[test]
    fn offered_sentence_matches_the_core_page() {
        with_input("offer", "paodaofa", |host, mut view| {
            let target = host.composition_alternatives().into_iter()
                .find(|text| !view.candidates.iter().any(|candidate| candidate.text == *text))
                .expect("a composed alternative outside the page");
            view.preedit = "stale".into();
            for candidate in &mut view.candidates { candidate.comment = "stale".into(); }
            let view = apply_scores(host, view, Some((&target, 3.0)), None);
            assert_eq!(host.observation.outcome, "no_group");
            assert_ne!(view.candidates[0].text, target);
            assert_eq!(view.candidates.last().unwrap().text, target);
            assert_eq!(view.candidates.last().unwrap().comment, "整句 · 备选");
            assert_core_page(host, &view);
        });
    }

    #[test]
    fn reordered_candidates_match_the_core_page() {
        with_input("reorder", "ni", |host, mut view| {
            let preferred = view.candidates[1].text.clone();
            view.preedit = "stale".into();
            for candidate in &mut view.candidates { candidate.comment = "stale".into(); }
            let view = apply_scores(host, view, None, Some(1));
            assert_eq!(host.observation.outcome, "reordered");
            assert_eq!(view.candidates[0].text, preferred);
            assert_core_page(host, &view);
        });
    }

    #[test]
    fn an_identity_ranking_keeps_the_unchanged_observation() {
        with_input("identity", "ni", |host, view| {
            let original = view.clone();
            let view = apply_scores(host, view, None, Some(0));
            assert_eq!(host.observation.outcome, "unchanged");
            assert_eq!(view, original);
            assert_core_page(host, &view);
        });
    }

    #[test]
    fn three_letter_english_stays_on_the_core_page_after_a_sentence_leads() {
        with_input("english", "pdf", |host, view| {
            assert_eq!(view.candidates[4].text, "pdf");
            let target = host.composition_alternatives().into_iter()
                .find(|text| !view.candidates.iter().any(|candidate| candidate.text == *text))
                .expect("a composed alternative outside the page");
            let view = apply_scores(host, view, Some((&target, 40.0)), None);
            assert_eq!(host.observation.outcome, "composed");
            assert_eq!(view.candidates[0].text, target);
            assert_eq!(view.candidates[4].text, "pdf");
            assert_eq!(view.candidates[4].comment, "英文 · 原样");
            assert_core_page(host, &view);
        });
    }

    #[test]
    fn a_model_decision_refreshes_page_fields_and_preserves_the_key_result() {
        with_input("refresh", "zaidengwoyixia", |host, mut view| {
            // A previous shell view must not supply any page fields after the core applies an intent.
            view.preedit = "stale".into();
            view.cursor_utf8 = 1;
            view.is_last_page = !view.is_last_page;
            view.commit = "先前提交".into();
            for candidate in &mut view.candidates { candidate.comment = "stale".into(); }
            let view = apply_scores(host, view, Some(("再等我一下", 40.0)), None);
            assert_eq!(host.observation.outcome, "composed");
            assert_eq!(view.commit, "先前提交");
            assert_core_page(host, &view);
        });
    }

    struct OffpageScorer { delay: Duration }
    impl ranking::Scorer for OffpageScorer {
        fn score(&mut self, _: &str, texts: &[String]) -> Result<Vec<f64>, String> {
            if texts.iter().any(|s| s == "时事") {
                std::thread::sleep(self.delay);
                return Ok(texts.iter().map(|s| if s == "时事" { 0.0 } else { -100.0 }).collect());
            }
            Ok(vec![-10.0; texts.len()])
        }
    }

    #[test]
    fn optional_offpage_word_preserves_four_slots_and_digit_five_commits_it() {
        with_input("offpage", "shishi", |host, view| {
            assert!(!view.candidates.iter().any(|c| c.text == "时事"));
            host.enable_model(Box::new(OffpageScorer { delay: Duration::ZERO }), None);
            let mut shown = host.rank(view, "关注国内外新闻".into());
            let primary = shown.clone();
            let deadline = std::time::Instant::now() + Duration::from_secs(2);
            while std::time::Instant::now() < deadline && shown.candidates[4].text != "时事" {
                if let Some(next) = host.poll_ranking() { shown = next; }
                std::thread::sleep(Duration::from_millis(1));
            }
            assert_eq!(shown.candidates[4].text, "时事");
            assert_eq!(shown.candidates[..4], primary.candidates[..4]);
            assert_core_page(host, &shown);
            assert_eq!(host.process(Key::Text("5".into()), 0, false).commit, "时事");
        });
    }

    #[test]
    fn optional_offpage_comparison_cannot_apply_after_its_eight_ms_budget() {
        with_input("offpage-budget", "shishi", |host, view| {
            host.enable_model(Box::new(OffpageScorer { delay: Duration::from_millis(12) }), None);
            let view = host.rank(view, "关注国内外新闻".into());
            std::thread::sleep(Duration::from_millis(40));
            assert!(host.poll_ranking().is_none());
            assert_eq!(host.core_page_texts(), view.candidates.iter().map(|c| c.text.clone()).collect::<Vec<_>>());
        });
    }

    #[test]
    fn deferred_ranking_returns_a_selectable_page_and_applies_on_notification() {
        struct HeldScorer {
            release: Option<std::sync::mpsc::Receiver<()>>,
        }
        impl ranking::Scorer for HeldScorer {
            fn score(&mut self, _context: &str, texts: &[String]) -> Result<Vec<f64>, String> {
                if let Some(release) = self.release.take() {
                    release.recv_timeout(Duration::from_secs(2)).map_err(|error| error.to_string())?;
                }
                Ok(vec![-10.0; texts.len()])
            }
        }

        with_input("deferred-ranking", "shishi", |host, decoder_page| {
            let (release, wait) = std::sync::mpsc::channel();
            let (notify, ready) = std::sync::mpsc::channel();
            host.enable_deferred_model(Box::new(HeldScorer { release: Some(wait) }), Box::new(move || {
                let _ = notify.send(());
            }));
            let initial = host.rank(decoder_page.clone(), "当地消息".into());
            assert_eq!(initial.candidates, decoder_page.candidates);
            assert_eq!(host.model_observation().outcome, "pending");
            assert_core_page(host, &initial);
            release.send(()).unwrap();
            ready.recv_timeout(Duration::from_secs(2)).unwrap();
            let redrawn = host.poll_ranking().expect("completed ranking from notification");
            assert_core_page(host, &redrawn);
            let selected = redrawn.candidates[0].text.clone();
            assert_eq!(host.process(Key::Text("1".into()), 0, false).commit, selected);
        });

        with_input("deferred-rank-select", "shishi", |host, decoder_page| {
            let (release, wait) = std::sync::mpsc::channel();
            host.enable_deferred_model(Box::new(HeldScorer { release: Some(wait) }), Box::new(|| {}));
            let shown = host.rank(decoder_page, "当地消息".into());
            assert_eq!(host.model_observation().outcome, "pending");
            let selected = shown.candidates[0].text.clone();
            assert_eq!(host.process(Key::Text("1".into()), 0, false).commit, selected);
            assert!(host.pending.is_none());
            let _ = release.send(());
            assert!(host.poll_ranking().is_none());
            assert!(host.core_page_texts().is_empty());
        });
    }

    #[derive(Default)]
    struct SyntheticAudit {
        calls: std::sync::atomic::AtomicUsize,
        commits: std::sync::atomic::AtomicUsize,
        clears: std::sync::atomic::AtomicUsize,
    }
    impl SyntheticAudit {
        fn load(&self, order: std::sync::atomic::Ordering) -> usize { self.calls.load(order) }
    }
    struct SyntheticDistribution { calls: std::sync::Arc<SyntheticAudit>, delay: Duration, fail: bool }
    impl generation::Distribution for SyntheticDistribution {
        fn score(&mut self, tokens: &[i32], rows: usize, cols: usize, vocab: usize) -> Result<Vec<f32>, String> {
            use std::sync::atomic::Ordering;
            self.calls.calls.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(self.delay);
            if self.fail { return Err("synthetic model error".into()); }
            let mut scores = vec![-100.0; rows * vocab];
            for (row, prefix) in tokens.chunks_exact(cols).enumerate() {
                let length = prefix.iter().take_while(|&&v| v != 0).count();
                let target = [4, 5, 6, 7][length.saturating_sub(2).min(3)];
                scores[row * vocab + target] = -0.1;
            }
            Ok(scores)
        }
        fn commit_prefixes(&mut self) { self.calls.commits.fetch_add(1, std::sync::atomic::Ordering::SeqCst); }
        fn clear(&mut self) { self.calls.clears.fetch_add(1, std::sync::atomic::Ordering::SeqCst); }
    }

    fn with_generation(name: &str, delay: Duration, fail: bool, check: impl FnOnce(&mut Host, std::sync::Arc<SyntheticAudit>)) {
        let _serial = SESSIONS.lock().unwrap_or_else(|error| error.into_inner());
        let (root, shared, user) = minimal_directories(name);
        let vocab = root.join("itos.json");
        std::fs::write(&vocab, r#"["<pad>","<bos>","<unk>","外","我","们","想","吃","星","菜"]"#).unwrap();
        let mut host = Host::open(&HostConfig { shared_dir: shared, user_dir: user, app_id: format!("zhizi.gen-{name}") }).unwrap();
        let calls = std::sync::Arc::new(SyntheticAudit::default());
        host.enable_zhiju(&vocab, Box::new(SyntheticDistribution { calls: calls.clone(), delay, fail }), None).unwrap();
        host.set_context_ranking_enabled(false);
        host.set_context("外");
        check(&mut host, calls);
        drop(host);
        std::fs::remove_dir_all(root).unwrap();
    }

    fn enter(host: &mut Host, keys: &str, secure: bool) -> View {
        let mut view = View::default();
        for c in keys.chars() { view = host.process(Key::Text(c.to_string()), 0, secure); }
        view
    }

    #[test]
    fn generation_without_context_ranking_uses_real_core_and_commits_shown_first() {
        with_generation("independent", Duration::ZERO, false, |host, calls| {
            let mut view = enter(host, "wmxc", false);
            let deadline = std::time::Instant::now() + Duration::from_secs(2);
            while std::time::Instant::now() < deadline && view.candidates.first().is_none_or(|c| c.comment != "整句 · 知句") {
                if let Some(next) = host.poll_ranking() { view = next; }
                std::thread::sleep(Duration::from_millis(1));
            }
            assert!(calls.load(std::sync::atomic::Ordering::SeqCst) > 0);
            assert!(calls.commits.load(std::sync::atomic::Ordering::SeqCst) > 0);
            assert_eq!(view.candidates[0].text, "我们想吃");
            assert_eq!(view.candidates[0].comment, "整句 · 知句");
            assert_eq!(view.candidates.iter().map(|c| c.text.clone()).collect::<Vec<_>>(), host.core_page_texts());
            assert_eq!(host.process(Key::Text("1".into()), 0, false).commit, view.candidates[0].text);
        });
    }

    #[test]
    fn disabled_or_secure_generation_never_calls_the_model() {
        with_generation("disabled-secure", Duration::ZERO, false, |host, calls| {
            host.set_zhiju_enabled(false);
            enter(host, "wmxc", false);
            host.process(Key::Escape, 0, false);
            host.set_zhiju_enabled(true);
            enter(host, "wmxc", true);
            std::thread::sleep(Duration::from_millis(10));
            assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
            assert!(host.poll_ranking().is_none());
        });
    }

    #[test]
    fn generation_error_or_timeout_keeps_the_basic_page() {
        for (name, delay, fail) in [("error", Duration::ZERO, true), ("timeout", Duration::from_millis(45), false)] {
            with_generation(name, delay, fail, |host, calls| {
                let view = enter(host, "wmxc", false);
                std::thread::sleep(Duration::from_millis(80));
                assert!(calls.load(std::sync::atomic::Ordering::SeqCst) > 0);
                assert_eq!(calls.commits.load(std::sync::atomic::Ordering::SeqCst), 0, "failed/late inference cannot enter the shared prefix cache");
                assert!(host.poll_ranking().is_none());
                assert_core_page(host, &view);
            });
        }
    }

    #[test]
    fn focus_clear_cancels_a_completed_generation_before_it_is_polled() {
        with_generation("focus-clear", Duration::ZERO, false, |host, calls| {
            let view = enter(host, "wmxc", false);
            let deadline = std::time::Instant::now() + Duration::from_secs(1);
            while calls.load(std::sync::atomic::Ordering::SeqCst) < 4 && std::time::Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(1));
            }
            host.clear_context();
            let deadline = std::time::Instant::now()+Duration::from_secs(1);
            while calls.clears.load(std::sync::atomic::Ordering::SeqCst)==0 && std::time::Instant::now()<deadline {
                std::thread::sleep(Duration::from_millis(1));
            }
            assert!(calls.clears.load(std::sync::atomic::Ordering::SeqCst)>0);
            assert!(host.poll_ranking().is_none());
            assert_core_page(host, &view);
        });
    }
}
