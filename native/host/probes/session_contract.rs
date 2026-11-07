//! Y3 observable Session + Mixer comparison using the same materialized Y0 keys.
use super::{parse_script, Action};
use crate::state::{KeyEvent, KeyKind, LearningIntent, ResultModel, SecurityState, Session};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct Visible {
    preedit: String,
    cursor_utf8: usize,
    candidates: Vec<String>,
    secondary_candidates: Vec<String>,
    highlighted_index: usize,
    page_no: usize,
    is_last_page: bool,
    commit_text: String,
    status: i32,
    consumed: bool,
}

impl From<ResultModel> for Visible {
    fn from(result: ResultModel) -> Self {
        Self {
            preedit: result.marked_text, cursor_utf8: result.marked_cursor_utf8,
            candidates: result.candidates.into_iter().map(|candidate| candidate.text).collect(),
            secondary_candidates: result.secondary_candidates.into_iter().map(|candidate| candidate.text).collect(),
            highlighted_index: result.highlighted_index, page_no: result.page_no,
            is_last_page: result.is_last_page, commit_text: result.commit_text,
            status: result.status, consumed: result.consumed,
        }
    }
}

fn differences(a: &Visible, b: &Visible) -> Vec<&'static str> {
    let mut fields = Vec::new();
    macro_rules! field {
        ($name:ident) => { if a.$name != b.$name { fields.push(stringify!($name)); } };
    }
    field!(preedit); field!(cursor_utf8);
    if a.candidates != b.candidates {
        let mut a = a.candidates.clone();
        let mut b = b.candidates.clone();
        a.sort(); b.sort();
        fields.push(if a == b { "candidate_order" } else { "candidate_membership" });
    }
    field!(secondary_candidates); field!(highlighted_index); field!(page_no);
    field!(is_last_page); field!(commit_text); field!(status); field!(consumed);
    fields
}

// Decoder-only intent flags have no independent host key. Use actual host
// selection semantics: first preview -> Space, explicit visible slot -> digit.
// A 999th slot has no product key; an empty text event observes its no-op UI
// result, and the mapping is retained in the trace rather than hidden as a pass.
fn event(action: &Action, delivery_id: u64) -> (KeyEvent, &'static str) {
    let (kind, text, mapping) = match action {
        Action::Type(text) => (KeyKind::Text, text.clone(), "text"),
        Action::Backspace => (KeyKind::Backspace, String::new(), "backspace"),
        Action::Left => (KeyKind::Left, String::new(), "left"),
        Action::Right => (KeyKind::Right, String::new(), "right"),
        Action::PageDown => (KeyKind::PageDown, String::new(), "page_down"),
        Action::PageUp => (KeyKind::PageUp, String::new(), "page_up"),
        Action::Next => (KeyKind::Down, String::new(), "down"),
        Action::Prev => (KeyKind::Up, String::new(), "up"),
        Action::NextLinear => (KeyKind::LinearNext, String::new(), "linear_next"),
        Action::PrevLinear => (KeyKind::LinearPrevious, String::new(), "linear_previous"),
        Action::Select(0, LearningIntent::NoLearn | LearningIntent::RequiredSegmentCommit) =>
            (KeyKind::Space, String::new(), "space_preview"),
        Action::Select(index, _) if *index < 5 =>
            (KeyKind::Text, (index + 1).to_string(), "visible_digit"),
        Action::Select(_, _) => (KeyKind::Text, String::new(), "unrepresentable_slot_observation"),
        Action::CommitRaw => (KeyKind::Enter, String::new(), "enter"),
        Action::Clear | Action::ClearAfterSelection => (KeyKind::Escape, String::new(), "escape"),
    };
    (KeyEvent { kind, text, modifiers: 0, security: SecurityState::NonSecure, delivery_id }, mapping)
}

#[derive(Debug, Serialize, Deserialize)]
struct Step {
    line: usize,
    action: String,
    host_mapping: String,
    delivery_id: u64,
    visible: Visible,
}

#[derive(Debug, Serialize, Deserialize)]
struct Trace {
    name: String,
    steps: Vec<Step>,
}

fn trace(session: &mut Session, source: &str, script_id: u64) -> Vec<Step> {
    parse_script(source).unwrap().iter().enumerate().map(|(step, (line, action))| {
        let delivery_id = (script_id << 32) | (step as u64 + 1);
        let (event, mapping) = event(action, delivery_id);
        Step { line: *line, action: format!("{action:?}"), host_mapping: mapping.into(), delivery_id,
            visible: session.process(event).into() }
    }).collect()
}

#[derive(Debug, Default, Serialize)]
struct Summary {
    scripts: usize,
    steps: usize,
    first_page_steps: usize,
    first_page_candidate_mismatches: usize,
    mismatched_steps: usize,
    unrepresentable_slot_observations: usize,
    by_kind: std::collections::BTreeMap<String, usize>,
    differences: Vec<serde_json::Value>,
}

fn compare(a: &[Trace], b: &[Trace]) -> Summary {
    assert!(!a.is_empty(), "no scripts cannot pass");
    assert_eq!(a.len(), b.len(), "script count");
    let mut summary = Summary { scripts: a.len(), ..Summary::default() };
    for (a, b) in a.iter().zip(b) {
        assert_eq!(a.name, b.name, "script identity");
        assert_eq!(a.steps.len(), b.steps.len(), "step count");
        for (step, (a_step, b_step)) in a.steps.iter().zip(&b.steps).enumerate() {
            assert_eq!((a_step.line, &a_step.action, a_step.delivery_id, &a_step.host_mapping),
                (b_step.line, &b_step.action, b_step.delivery_id, &b_step.host_mapping), "action identity");
            summary.steps += 1;
            if a_step.host_mapping == "unrepresentable_slot_observation" { summary.unrepresentable_slot_observations += 1; }
            let fields = differences(&a_step.visible, &b_step.visible);
            if a_step.visible.page_no == 0 && b_step.visible.page_no == 0 {
                summary.first_page_steps += 1;
                if a_step.visible.candidates != b_step.visible.candidates { summary.first_page_candidate_mismatches += 1; }
            }
            if !fields.is_empty() {
                summary.mismatched_steps += 1;
                for field in &fields { *summary.by_kind.entry((*field).into()).or_default() += 1; }
                summary.differences.push(serde_json::json!({ "script": a.name, "step": step + 1,
                    "line": a_step.line, "action": a_step.action, "fields": fields,
                    "a": a_step.visible, "b": b_step.visible }));
            }
        }
    }
    summary
}

fn preedit_and_cursor_match(summary: &Summary) -> bool {
    ["preedit", "cursor_utf8"].iter().all(|field| summary.by_kind.get(*field).copied().unwrap_or(0) == 0)
}

#[test]
fn session_contract_parity_gate_keeps_candidate_review_separate() {
    let source = "type n\ntype i\n";
    let build = || Session::new(Box::<crate::state::MockDecoder>::default(), "y3b-parity".into());
    let a = vec![Trace { name: "one".into(), steps: trace(&mut build(), source, 1) }];
    let mut b = vec![Trace { name: "one".into(), steps: trace(&mut build(), source, 1) }];
    b[0].steps[1].visible.candidates.push("异".into());
    b[0].steps[1].visible.page_no += 1;
    let candidate_difference = compare(&a, &b);
    assert_eq!(candidate_difference.mismatched_steps, 1);
    assert!(preedit_and_cursor_match(&candidate_difference));
    b[0].steps[1].visible.preedit.push(' ');
    assert!(!preedit_and_cursor_match(&compare(&a, &b)));
    b[0].steps[1].visible.preedit = a[0].steps[1].visible.preedit.clone();
    b[0].steps[1].visible.cursor_utf8 += 1;
    assert!(!preedit_and_cursor_match(&compare(&a, &b)));
}

#[test]
fn session_contract_observes_order_highlight_preedit_and_commit() {
    let a = Visible { preedit: "ni hao".into(), candidates: vec!["你好".into(), "你号".into()], ..Visible::default() };
    let mut b = a.clone();
    assert!(differences(&a, &b).is_empty());
    b.candidates.reverse();
    assert_eq!(differences(&a, &b), ["candidate_order"]);
    b.preedit = "nihao".into();
    b.highlighted_index = 1;
    b.commit_text = "你好".into();
    assert_eq!(differences(&a, &b), ["preedit", "candidate_order", "highlighted_index", "commit_text"]);
}

#[test]
fn session_contract_membership_and_errors_are_not_order_only() {
    let a = Visible { candidates: vec!["你好".into(), "你号".into()], consumed: true, ..Visible::default() };
    let mut b = a.clone();
    b.candidates[1] = "拟好".into();
    b.status = -10;
    b.consumed = false;
    b.secondary_candidates.push("hello".into());
    assert_eq!(differences(&a, &b), ["candidate_membership", "secondary_candidates", "status", "consumed"]);
}

#[test]
fn session_contract_maps_every_y0_action_without_bypassing_session() {
    let source = "type n\nbackspace\nleft\nright\npagedown\npageup\nnext\nprev\nnext_linear\nprev_linear\nselect 0 no_learn\nselect 0 required\nselect 2 deliberate\nselect 999\ncommit_raw\nclear\nclear_after_selection\n";
    let mapped: Vec<_> = parse_script(source).unwrap().iter().map(|(_, action)| {
        let (key, mapping) = event(action, 19);
        assert_eq!(key.delivery_id, 19);
        assert_eq!(key.security, SecurityState::NonSecure);
        (key.kind, key.text, mapping)
    }).collect();
    assert_eq!(mapped.len(), 17);
    assert_eq!(mapped[10].0, KeyKind::Space);
    assert_eq!(mapped[11].0, KeyKind::Space);
    assert_eq!(mapped[12], (KeyKind::Text, "3".into(), "visible_digit"));
    assert_eq!(mapped[13], (KeyKind::Text, String::new(), "unrepresentable_slot_observation"));
    assert_eq!(mapped[14].0, KeyKind::Enter);
    assert_eq!(mapped[15].0, KeyKind::Escape);
    assert_eq!(mapped[16].0, KeyKind::Escape);
}

#[test]
fn session_contract_detects_session_output_changes_and_keeps_ids() {
    let source = "clear\ntype n\ntype i\nselect 0 no_learn\nclear\n";
    let build = || Session::new(Box::<crate::state::MockDecoder>::default(), "y3-unit".into());
    let a = vec![Trace { name: "one".into(), steps: trace(&mut build(), source, 4) }];
    let mut b = vec![Trace { name: "one".into(), steps: trace(&mut build(), source, 4) }];
    assert_eq!(compare(&a, &b).mismatched_steps, 0);
    assert_eq!(a[0].steps[3].delivery_id, (4 << 32) | 4);
    b[0].steps[3].visible.commit_text.push('错');
    assert_eq!(compare(&a, &b).by_kind.get("commit_text"), Some(&1));
}

#[test]
#[should_panic(expected = "no scripts cannot pass")]
fn session_contract_empty_suite_never_passes() { compare(&[], &[]); }

    #[test]
    fn session_contract_product_keeps_whole_sentence_alternatives_on_deep_pages() {
        use crate::zhixu::loader::{DictionaryData, Entry};
        use crate::zhixu::Index;
        use std::sync::Arc;
        let dictionary = Arc::new(Index::from_data(DictionaryData { entries: [
            ("你", "ni", 10000), ("拟", "ni", 8000), ("好", "hao", 10000),
            ("号", "hao", 8000), ("我们", "wo men", 10000),
        ].into_iter().map(|(text, reading, weight)| Entry {
            text: text.into(), reading: reading.into(), weight, layer: "pinyin_simp".into(),
        }).collect() }));
        let _dictionary = crate::zhixu::pin_snapshot(Some(dictionary.clone()));
        let _learning = crate::zhixu::pin_learning(None);
        let generated = crate::mixer::decoder_whole_candidates("nihaowomen", &[], 0, false);
        assert!(generated.iter().any(|candidate| candidate.text == "你号我们"), "existing block-3 generator fixture");
        let mut session = Session::new_native("y3-whole-alternative".into(),
            std::sync::Arc::new(std::sync::Mutex::new(crate::blacklist::Store::default())), dictionary).unwrap();
        session.configure_learning(crate::zhixu::learning::LearningMode::Zhixu).unwrap();
        session.configure_zhixu_whole(true).unwrap();
    let source = "nihaowomen".chars().map(|letter| format!("type {letter}\n")).collect::<String>();
    let mut steps = trace(&mut session, &source, 1);
    let mut page = steps.pop().unwrap().visible;
    let mut candidates = page.candidates.clone();
    for id in 1..100 {
        if page.is_last_page { break; }
        let old_page = page.page_no;
        page = session.process(event(&Action::PageDown, (2 << 32) | id).0).into();
        assert!(page.page_no > old_page, "page scan must progress");
        candidates.extend(page.candidates.clone());
    }
    assert!(page.is_last_page, "scan must reach the actual last page");
    for expected in ["你好我们", "你号我们", "拟好我们", "拟号我们"] {
        assert!(candidates.iter().any(|candidate| candidate == expected), "missing block-3 whole alternative {expected}: {candidates:?}");
    }
}

    #[test]
    fn session_contract_native_single_character_deletion_restores_dictionary_order() {
        use crate::zhixu::learning::{LearningMode, LearningStore};
        use crate::zhixu::loader::{DictionaryData, Entry};
        let now = crate::personal_picks::now_ms();
        let directory = std::env::temp_dir().join(format!("zhixu-y3b-single-{}-{now}", std::process::id()));
        let dictionary = crate::zhixu::Index::from_data(DictionaryData { entries: [
            ("你", 10000), ("拟", 5000), ("泥", 4000), ("倪", 3000),
        ].into_iter().map(|(text, weight)| Entry {
            text: text.into(), reading: "ni".into(), weight, layer: "pinyin_simp".into(),
        }).collect() });
        let dictionary = std::sync::Arc::new(dictionary);
        let _dictionary = crate::zhixu::pin_snapshot(Some(dictionary.clone()));
        let _learning = crate::zhixu::pin_learning(None);
        let build = || {
            let mut session = Session::new_native("y3b.single".into(),
                crate::blacklist::shared_blacklist(&directory).unwrap(), dictionary.clone()).unwrap();
            session.configure_learning(LearningMode::Zhixu).unwrap();
            session
        };
    let baseline = trace(&mut build(), "type n\ntype i\n", 1).pop().unwrap().visible.candidates;
    assert_eq!(baseline, ["你", "拟", "泥", "倪"]);
    let mut store = LearningStore::load(&directory, now);
    for _ in 0..3 { assert!(store.record("ni", "泥", now)); }
    store.save(&directory).unwrap();
    let mut session = build();
    let before = trace(&mut session, "type n\ntype i\n", 2).pop().unwrap().visible;
    assert_eq!(before.candidates[0], "泥");
    let deleted = session.process(KeyEvent { kind: KeyKind::DeleteCandidate, text: String::new(),
        modifiers: crate::state::MOD_CONTROL, security: SecurityState::NonSecure, delivery_id: 20 });
    assert_eq!(deleted.status, 0);
    assert_eq!(Visible::from(deleted).candidates, baseline);
    drop(session);
    assert!(LearningStore::load(&directory, now).entries().is_empty());
    assert_eq!(trace(&mut build(), "type n\ntype i\n", 3).pop().unwrap().visible.candidates, baseline);
    std::fs::remove_dir_all(directory).unwrap();
}

// The legacy Rime session (A side) retired with the engine: Y3 now re-runs the
// zhixu (B) side over the frozen corpus and byte-compares with the archived
// b.json. Scripts are rebuilt from the archive itself, exactly as the Y0
// baseline entry does.
#[test]
#[ignore]
fn session_contract_zhixu_frozen_baseline() {
    use super::{frozen_script, script_paths};
    use std::fs;
    use std::path::PathBuf;
    let shared = PathBuf::from(std::env::var_os("Y0_SHARED").expect("Y0_SHARED isolated public-only fixture"));
    let baseline = PathBuf::from(std::env::var_os("Y3_BASELINE").expect("Y3_BASELINE decompressed frozen b.json"));
    let expected = fs::read(&baseline).unwrap();
    let archived: Vec<Trace> = serde_json::from_slice(&expected).unwrap();
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let output = std::env::temp_dir().join(format!("zhixu-y3-frozen-{}-{stamp}", std::process::id()));
    let scripts = output.join("scripts");
    for trace in &archived {
        let path = scripts.join(&trace.name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let steps: Vec<(usize, String)> = trace.steps.iter().map(|step| (step.line, step.action.clone())).collect();
        fs::write(&path, frozen_script(&steps)).unwrap();
    }
    let user = output.join("user");
    fs::create_dir(&user).unwrap();
    fs::write(user.join("agent_pinyin.custom.yaml"),
        "patch:\n  translator/enable_user_dict: false\n  translator/enable_encoder: false\n  translator/encode_commit_history: false\n").unwrap();
    let dictionary = crate::zhixu::loader::load(&shared, &user).unwrap();
    let pinned = std::sync::Arc::new(crate::zhixu::Index::from_data(dictionary));
    let _dictionary = crate::zhixu::pin_snapshot(Some(pinned.clone()));
    let _learning = crate::zhixu::pin_learning(None);
    // The product constructor: the native decoder from the pinned dictionary,
    // with an empty in-memory store per script.
    let mut traces = Vec::new();
    for (index, path) in script_paths(&scripts).into_iter().enumerate() {
        let name = path.strip_prefix(&scripts).unwrap().display().to_string();
        let source = fs::read_to_string(&path).unwrap();
        let app_id = format!("y3-public-{index}");
        let mut session = Session::new_native(app_id,
            std::sync::Arc::new(std::sync::Mutex::new(crate::blacklist::Store::default())), pinned.clone()).unwrap();
        session.configure_learning(crate::zhixu::learning::LearningMode::Zhixu).unwrap();
        let steps = trace(&mut session, &source, index as u64 + 1);
        println!("Y3_SCRIPT\tzhixu\t{name}\tsteps={}", steps.len());
        traces.push(Trace { name, steps });
    }
    let fresh = serde_json::to_vec(&traces).unwrap();
    fs::write(output.join("b.json"), &fresh).unwrap();
    assert!(fresh == expected,
        "zhixu Y3 session trace differs from the frozen baseline; fresh copy: {}", output.join("b.json").display());
    println!("Y3_BASELINE_MATCH\t{}", output.display());
}
