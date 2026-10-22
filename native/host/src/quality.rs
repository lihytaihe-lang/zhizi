//! Quality records, the same JSON lines as the macOS QualityRecorder
//! (native/macos/Sources/AgentInputMethodHostKit/QualityRecorder.swift) so the analysis
//! scripts read both, plus `platform`, `build` and `device`. One record per composition:
//! the pinyin, the last page shown, what was committed and how, backspaces, pages, time
//! and what 顺手·知字 did. Preceding text keeps only its last 8 characters, with long
//! tokens, long digit runs and e-mail addresses hidden. Secure fields are never recorded.
//!
//! A record waits five seconds before it is written, so deletions in that window land in
//! the same line (`deletedAfterCommit`; deleting all of it and typing again also sets
//! `redoAfterCommit`). Picking a reading in parts records each pick in `segments` and what
//! reached the app in `committedText`; `peakLength` keeps how long the longest reading typed was (only the length) and
//! `firstCandidate` page 0's first when the owner paged.

use crate::{Key, ModelObservation, View};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

const PRECEDING_WINDOW: usize = 8;
const REDO_WINDOW_MS: i64 = 5_000;
const MAX_CANDIDATES: usize = 12;

pub struct RecorderConfig {
    /// The events.jsonl to append to (its directory is created).
    pub path: PathBuf,
    pub platform: String,
    pub build: String,
    /// Machine, for example the CPU name.
    pub device: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ContextModelRecord {
    enabled: bool,
    outcome: String,
    group_size: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    latency_ms: Option<f64>,
    late: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Event {
    id: String,
    recorded_at_ms: i64,
    pinyin: String,
    candidates: Vec<String>,
    candidate_comments: Vec<String>,
    selected_index: usize,
    selected_text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    selected_candidate_comment: Option<String>,
    app_hash: String,
    preceding_text: String,
    fuzzy_mask: u32,
    typo_correction_enabled: bool,
    active_packs: Vec<String>,
    secondary_enabled: bool,
    max_page: usize,
    outcome: &'static str,
    duration_ms: i64,
    backspaces: usize,
    redo_after_commit: bool,
    candidate_count: usize,
    context_model: ContextModelRecord,
    platform: String,
    build: String,
    device: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    committed_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    segments: Option<Vec<Segment>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    peak_length: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    first_candidate: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    deleted_after_commit: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
struct Segment {
    text: String,
    index: usize,
    page: usize,
}

/// The whole reading as last shown before any segment was picked.
struct Whole {
    candidates: Vec<String>,
    comments: Vec<String>,
    model: ContextModelRecord,
}

struct Pending {
    pinyin: String,
    candidates: Vec<String>,
    comments: Vec<String>,
    highlighted: usize,
    preceding: String,
    started_ms: i64,
    max_page: usize,
    backspaces: usize,
    candidate_count: usize,
    model: ContextModelRecord,
    page: usize,
    peak_length: usize,
    first_candidate: Option<String>,
    whole: Option<Whole>,
    picked_prefix: String,
    segments: Vec<Segment>,
}

struct Waiting {
    event: Event,
    deadline_ms: i64,
    committed_chars: usize,
    deleted_chars: usize,
}

pub struct Recorder {
    config: RecorderConfig,
    app_hash: String,
    pending: Option<Pending>,
    waiting: Option<Waiting>,
    counter: u64,
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn hex_sha256(value: &str) -> String {
    Sha256::digest(value.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

fn sanitized_pinyin(value: &str) -> Option<String> {
    let compact: String = value.to_lowercase().chars().filter(|c| !c.is_whitespace()).collect();
    (!compact.is_empty() && compact.len() <= 128 && compact.bytes().all(|b| b.is_ascii_lowercase() || b == b'\''))
        .then_some(compact)
}

/// Picked text (leading non-letters) and the reading still being typed: "会长yi xie" →
/// ("会长", "yixie"). None when the preedit is not a pinyin composition.
fn split_preedit(value: &str) -> Option<(String, String)> {
    let first = value.find(|c: char| c.is_ascii_alphabetic())?;
    let prefix = value[..first].trim();
    if prefix.chars().any(|c| c.is_ascii() && !c.is_whitespace()) || prefix.len() > 256 {
        return None;
    }
    Some((prefix.to_string(), sanitized_pinyin(&value[first..])?))
}

fn picked_index(key: &Key, pending: &Pending) -> usize {
    match key {
        Key::Text(text) => text
            .parse::<usize>()
            .ok()
            .filter(|d| (1..=pending.candidates.len()).contains(d))
            .map(|d| d - 1)
            .unwrap_or(pending.highlighted),
        _ => pending.highlighted,
    }
}

/// Index of `text` among the recorded candidates, appending it (or replacing the last of
/// twelve) when the page never showed it.
fn placing(text: &str, pending: &mut Pending) -> usize {
    if let Some(index) = pending.candidates.iter().position(|c| c == text) {
        return index;
    }
    if pending.candidates.len() >= MAX_CANDIDATES {
        pending.candidates.pop();
        pending.comments.pop();
    }
    pending.candidates.push(text.to_string());
    pending.comments.push(String::new());
    pending.candidates.len() - 1
}

fn sanitized_candidate(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty() && trimmed.len() <= 256 && !trimmed.chars().any(char::is_control)).then(|| trimmed.to_string())
}

/// Hides e-mail addresses, ASCII tokens of 12+ characters and digit runs of 6+, then
/// keeps the last 8 characters (UTF-16 units, as on macOS).
/// Test builds record and upload only after the person said yes (the installer asks; the Start
/// menu can switch it off again). The choice is `<data>/consent.json`; no file means no.
pub fn consent_granted(data_dir: &std::path::Path) -> bool {
    std::fs::read(data_dir.join("consent.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .is_some_and(|value| value["granted"] == true)
}

pub fn set_consent(data_dir: &std::path::Path, granted: bool) -> std::io::Result<()> {
    std::fs::create_dir_all(data_dir)?;
    let at_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let temporary = data_dir.join("consent.json.tmp");
    std::fs::write(&temporary, serde_json::json!({ "granted": granted, "atMs": at_ms, "notice": "win-v1" }).to_string())?;
    std::fs::rename(temporary, data_dir.join("consent.json"))
}

pub(crate) fn sanitized_preceding_text(value: &str) -> String {
    const HIDDEN: &str = "[已隐藏]";
    let token = |c: char| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '%' | '+' | '@');
    let mut out = String::new();
    let chars: Vec<char> = value.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if token(chars[i]) {
            let start = i;
            while i < chars.len() && token(chars[i]) {
                i += 1;
            }
            let run: String = chars[start..i].iter().collect();
            let email = run.contains('@') && run.rsplit('@').next().is_some_and(|domain| domain.contains('.'));
            let long_token = run
                .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
                .any(|part| part.len() >= 12);
            let long_digits = run.split(|c: char| !c.is_ascii_digit()).any(|part| part.len() >= 6);
            if email || long_token || long_digits {
                out.push_str(HIDDEN);
            } else {
                out.push_str(&run);
            }
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    let mut kept: Vec<char> = Vec::new();
    let mut units = 0;
    for c in out.chars().rev() {
        if units + c.len_utf16() > PRECEDING_WINDOW {
            break;
        }
        units += c.len_utf16();
        kept.push(c);
    }
    kept.into_iter().rev().collect()
}

impl Recorder {
    pub fn new(config: RecorderConfig, app_id: &str) -> Self {
        Self { app_hash: hex_sha256(app_id), config, pending: None, waiting: None, counter: 0 }
    }

    pub fn set_app(&mut self, app_id: &str) {
        self.flush();
        self.pending = None;
        self.app_hash = hex_sha256(app_id);
    }

    fn event_id(&mut self) -> String {
        self.counter += 1;
        let seed = format!("{}-{}-{}-{}", now_ms(), std::process::id(), self.counter, self.app_hash);
        let h = hex_sha256(&seed);
        format!("{}-{}-4{}-a{}-{}", &h[0..8], &h[8..12], &h[13..16], &h[17..20], &h[20..32])
    }

    fn write(&self, event: &Event) {
        let Ok(mut line) = serde_json::to_vec(event) else { return };
        line.push(b'\n');
        if line.len() > 16 * 1024 {
            return;
        }
        if let Some(parent) = self.config.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&self.config.path) {
            let _ = file.write_all(&line);
        }
    }

    /// Writes the record waiting out its redo window.
    pub fn flush(&mut self) {
        if let Some(waiting) = self.waiting.take() {
            self.write(&waiting.event);
        }
    }

    /// A late 顺手·知字 result redrew the page: record that page instead.
    pub fn refresh(&mut self, view: &View, model: &ModelObservation) {
        let Some(current) = &mut self.pending else { return };
        let shown: Vec<(String, String)> = view
            .candidates
            .iter()
            .take(MAX_CANDIDATES)
            .filter_map(|c| Some((sanitized_candidate(&c.text)?, sanitized_candidate(&c.comment).unwrap_or_default())))
            .collect();
        current.candidates = shown.iter().map(|c| c.0.clone()).collect();
        current.comments = shown.iter().map(|c| c.1.clone()).collect();
        current.highlighted = view.highlighted.min(shown.len().saturating_sub(1));
        current.model = ContextModelRecord {
            enabled: true,
            outcome: model.outcome.to_string(),
            group_size: model.group_size,
            latency_ms: model.latency_ms,
            late: model.late,
        };
    }

    /// After every processed key. `selected`: the core reported a candidate selection.
    pub fn observe(
        &mut self,
        key: &Key,
        view: &View,
        selected: bool,
        preceding: &str,
        model: &ModelObservation,
        secure: bool,
    ) {
        let now = now_ms();
        if secure {
            self.pending = None;
            return;
        }
        if let Some(waiting) = &mut self.waiting {
            if now > waiting.deadline_ms {
                self.flush();
            } else if self.pending.is_none() && *key == Key::Backspace && view.preedit.is_empty() {
                waiting.deleted_chars += 1;
            } else if matches!(key, Key::Text(_)) && !view.preedit.is_empty() {
                if waiting.deleted_chars > 0 {
                    waiting.event.deleted_after_commit = Some(waiting.deleted_chars);
                }
                if waiting.deleted_chars >= waiting.committed_chars {
                    waiting.event.redo_after_commit = true;
                }
                self.flush();
            }
        }

        if !view.preedit.is_empty() {
            let Some((prefix, pinyin)) = split_preedit(&view.preedit) else { return };
            let shown: Vec<(String, String)> = view
                .candidates
                .iter()
                .take(MAX_CANDIDATES)
                .filter_map(|c| Some((sanitized_candidate(&c.text)?, sanitized_candidate(&c.comment).unwrap_or_default())))
                .collect();
            let count = view.page * 5 + view.candidates.len();
            let record = ContextModelRecord {
                enabled: true,
                outcome: model.outcome.to_string(),
                group_size: model.group_size,
                latency_ms: model.latency_ms,
                late: model.late,
            };
            let current = self.pending.get_or_insert_with(|| Pending {
                pinyin: pinyin.clone(),
                highlighted: 0,
                candidates: Vec::new(),
                comments: Vec::new(),
                preceding: sanitized_preceding_text(preceding),
                started_ms: now,
                max_page: view.page,
                backspaces: 0,
                candidate_count: count,
                model: record.clone(),
                page: 0,
                peak_length: 0,
                first_candidate: None,
                whole: None,
                picked_prefix: String::new(),
                segments: Vec::new(),
            });
            if prefix != current.picked_prefix {
                if prefix.len() > current.picked_prefix.len() && prefix.starts_with(&current.picked_prefix) {
                    // A segment was picked from the page shown before this key.
                    if current.segments.is_empty() {
                        current.whole = Some(Whole {
                            candidates: current.candidates.clone(),
                            comments: current.comments.clone(),
                            model: current.model.clone(),
                        });
                    }
                    let index = picked_index(key, current);
                    current.segments.push(Segment {
                        text: prefix[current.picked_prefix.len()..].to_string(),
                        index,
                        page: current.page,
                    });
                } else {
                    // Backspace undid picks: keep the ones still in the preedit.
                    while !current.segments.is_empty()
                        && !prefix.starts_with(&current.segments.iter().map(|s| s.text.as_str()).collect::<String>())
                    {
                        current.segments.pop();
                    }
                    if current.segments.is_empty() {
                        current.whole = None;
                    }
                }
                current.picked_prefix = prefix.clone();
            }
            if prefix.is_empty() {
                current.peak_length = current.peak_length.max(pinyin.len());
                current.pinyin = pinyin;
                if view.page == 0 {
                    current.first_candidate = shown.first().map(|c| c.0.clone());
                }
            }
            current.highlighted = view.highlighted.min(shown.len().saturating_sub(1));
            current.candidates = shown.iter().map(|c| c.0.clone()).collect();
            current.comments = shown.iter().map(|c| c.1.clone()).collect();
            current.page = view.page;
            current.max_page = current.max_page.max(view.page);
            current.candidate_count = current.candidate_count.max(count);
            current.model = record;
            if *key == Key::Backspace {
                current.backspaces += 1;
            }
            return;
        }

        let Some(mut completed) = self.pending.take() else { return };
        if *key == Key::Backspace {
            completed.backspaces += 1;
        }
        let committed = sanitized_candidate(&view.commit);
        let mut segments = None;
        let (outcome, selected_index, selected_text, comment) = if selected {
            let index = picked_index(key, &completed);
            let Some(text) = completed.candidates.get(index).cloned() else { return };
            match (completed.whole.take(), &committed) {
                (Some(whole), Some(all)) => {
                    // Picked in parts: describe the whole reading, and what was put together.
                    let mut picks = std::mem::take(&mut completed.segments);
                    picks.push(Segment { text, index, page: completed.page });
                    segments = Some(picks);
                    completed.candidates = whole.candidates;
                    completed.comments = whole.comments;
                    completed.model = whole.model;
                    let index = placing(all, &mut completed);
                    ("committed_candidate", index, all.clone(), None)
                }
                _ => {
                    let comment = completed.comments.get(index).filter(|c| !c.is_empty()).cloned();
                    ("committed_candidate", index, text, comment)
                }
            }
        } else if let Some(raw) = committed.clone() {
            if let Some(whole) = completed.whole.take() {
                completed.candidates = whole.candidates;
                completed.comments = whole.comments;
                completed.model = whole.model;
            }
            let index = placing(&raw, &mut completed);
            ("committed_raw", index, raw, None)
        } else {
            ("abandoned", 0, String::new(), None)
        };
        let peak_length = (completed.peak_length > completed.pinyin.len()).then_some(completed.peak_length);
        let first_candidate = if completed.max_page > 0 { completed.first_candidate.clone() } else { None };
        let event = Event {
            id: self.event_id(),
            recorded_at_ms: now,
            pinyin: completed.pinyin,
            candidate_count: completed.candidate_count.max(completed.candidates.len()),
            candidates: completed.candidates,
            candidate_comments: completed.comments,
            selected_index,
            selected_text: selected_text.clone(),
            selected_candidate_comment: comment,
            app_hash: self.app_hash.clone(),
            preceding_text: completed.preceding,
            fuzzy_mask: 0,
            typo_correction_enabled: true,
            active_packs: Vec::new(),
            secondary_enabled: true,
            max_page: completed.max_page,
            outcome,
            duration_ms: (now - completed.started_ms).max(0),
            backspaces: completed.backspaces,
            redo_after_commit: false,
            context_model: completed.model,
            platform: self.config.platform.clone(),
            build: self.config.build.clone(),
            device: self.config.device.clone(),
            committed_text: (outcome != "abandoned").then(|| committed.clone().unwrap_or_else(|| selected_text.clone())),
            segments,
            peak_length,
            first_candidate,
            deleted_after_commit: None,
        };
        self.flush();
        if outcome == "abandoned" || selected_text.is_empty() {
            self.write(&event);
        } else {
            let committed_chars = event.committed_text.as_deref().unwrap_or(&selected_text).chars().count().max(1);
            self.waiting = Some(Waiting { event, deadline_ms: now + REDO_WINDOW_MS, committed_chars, deleted_chars: 0 });
        }
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        self.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_is_recorded_without_a_yes() {
        let dir = std::env::temp_dir().join(format!("zhizi-consent-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(!consent_granted(&dir), "no file means no");
        set_consent(&dir, true).unwrap();
        assert!(consent_granted(&dir));
        set_consent(&dir, false).unwrap();
        assert!(!consent_granted(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn preceding_text_hides_tokens_and_keeps_eight_characters() {
        assert_eq!(sanitized_preceding_text("我们明天见"), "我们明天见");
        assert_eq!(sanitized_preceding_text("电话13812345678"), "电话[已隐藏]");
        assert_eq!(sanitized_preceding_text("key sk-abcdefghijklmnop"), "ey [已隐藏]");
        assert_eq!(sanitized_preceding_text("一二三四五六七八九十"), "三四五六七八九十");
    }

    #[test]
    fn pinyin_is_recorded_only_while_composing_letters() {
        assert_eq!(sanitized_pinyin("wo zong an").as_deref(), Some("wozongan"));
        assert_eq!(sanitized_pinyin("我 zong"), None);
        assert_eq!(split_preedit("会长yi xie"), Some(("会长".into(), "yixie".into())));
        assert_eq!(split_preedit("hui chang"), Some((String::new(), "huichang".into())));
        assert_eq!(split_preedit("会长"), None);
    }

    fn page(preedit: &str, commit: &str, texts: &[&str]) -> View {
        View {
            preedit: preedit.into(),
            commit: commit.into(),
            candidates: texts
                .iter()
                .map(|t| crate::Candidate { text: (*t).into(), comment: String::new() })
                .collect(),
            ..View::default()
        }
    }

    #[test]
    fn picks_in_parts_record_the_whole_reading_and_what_was_put_together() {
        let path = std::env::temp_dir().join(format!("zhizi-quality-{}.jsonl", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let config = RecorderConfig { path: path.clone(), platform: "test".into(), build: "0".into(), device: "0".into() };
        let model = ModelObservation::default();
        {
            let mut recorder = Recorder::new(config, "app");
            let key = Key::Text("e".into());
            recorder.observe(&key, &page("hui chang yi xie", "", &["会场一些", "会场", "会长"]), false, "", &model, false);
            recorder.observe(&Key::Text("3".into()), &page("会长yi xie", "", &["一些", "依稀"]), true, "", &model, false);
            recorder.observe(&Key::Space, &page("", "会长一些", &[]), true, "", &model, false);
        }
        let line = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        let event: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(event["pinyin"], "huichangyixie");
        assert_eq!(event["selectedText"], "会长一些");
        assert_eq!(event["selectedIndex"], 3);
        assert_eq!(event["committedText"], "会长一些");
        assert_eq!(
            event["segments"],
            serde_json::json!([{"text": "会长", "index": 2, "page": 0}, {"text": "一些", "index": 0, "page": 0}])
        );
    }
}
