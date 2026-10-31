//! Test-only, direct Decoder contract probe; Y3's Session comparison is separate.
#[path = "session_contract.rs"]
mod session_contract;
use crate::state::{BoundaryPurpose, CompositionBoundary, Decoder, DecoderSnapshot, LearningIntent};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq)]
enum Action {
    Type(String), Backspace, Left, Right, PageDown, PageUp, Next, Prev,
    NextLinear, PrevLinear, Select(usize, LearningIntent), CommitRaw, Clear,
    ClearAfterSelection,
}

fn parse_script(source: &str) -> Result<Vec<(usize, Action)>, String> {
    let mut actions = Vec::new();
    for (line, text) in source.lines().enumerate() {
        let text = text.trim();
        if text.is_empty() || text.starts_with('#') { continue; }
        let words: Vec<_> = text.split_whitespace().collect();
        let action = match words.as_slice() {
            ["type", letter] if letter.len() == 1 && (letter.as_bytes()[0].is_ascii_lowercase() || *letter == "'") => Action::Type((*letter).into()),
            ["backspace"] => Action::Backspace,
            ["left"] => Action::Left,
            ["right"] => Action::Right,
            ["pagedown"] => Action::PageDown,
            ["pageup"] => Action::PageUp,
            ["next"] => Action::Next,
            ["prev"] => Action::Prev,
            ["next_linear"] => Action::NextLinear,
            ["prev_linear"] => Action::PrevLinear,
            ["commit_raw"] => Action::CommitRaw,
            ["clear"] => Action::Clear,
            ["clear_after_selection"] => Action::ClearAfterSelection,
            ["select", index] | ["select", index, "deliberate"] => Action::Select(index.parse().map_err(|_| format!("line {}: invalid index", line + 1))?, LearningIntent::DeliberateSelection),
            ["select", index, "no_learn"] => Action::Select(index.parse().map_err(|_| format!("line {}: invalid index", line + 1))?, LearningIntent::NoLearn),
            ["select", index, "required"] => Action::Select(index.parse().map_err(|_| format!("line {}: invalid index", line + 1))?, LearningIntent::RequiredSegmentCommit),
            _ => return Err(format!("line {}: invalid action {text:?}", line + 1)),
        };
        actions.push((line + 1, action));
    }
    if actions.is_empty() { return Err("empty script".into()); }
    Ok(actions)
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
struct Shape {
    covered_letters: Option<usize>,
    whole: bool,
    single_character: bool,
}

fn structure_equal(left: &[Shape], right: &[Shape]) -> bool {
    let mut left = left.to_vec();
    let mut right = right.to_vec();
    left.sort();
    right.sort();
    left == right
}

// Independent observation, not the product's prefix heuristic. Only accept a
// complete rendered suffix; never treat inferred Candidate.reading_key as a span.
fn confirmed_and_remaining(snapshot: &DecoderSnapshot) -> Option<(String, usize)> {
    fn letters(value: &str) -> String {
        value.chars().filter_map(|ch| match ch {
            'ü' | 'Ü' => Some('v'),
            ch if ch.is_ascii_alphabetic() => Some(ch.to_ascii_lowercase()),
            _ => None,
        }).collect()
    }
    fn spelling(value: &str) -> String {
        letters(value).replace("jv", "ju").replace("qv", "qu")
            .replace("xv", "xu").replace("yv", "yu")
            .replace("nve", "nue").replace("lve", "lue")
    }
    if snapshot.raw_input.is_empty() && snapshot.preedit.is_empty() { return Some((String::new(), 0)); }
    if snapshot.raw_input == snapshot.preedit && snapshot.raw_input.chars().all(|ch| ch == '\'') {
        return Some((String::new(), 0));
    }
    let at = snapshot.preedit.find(|ch: char| ch.is_ascii_alphabetic() || matches!(ch, 'ü' | 'Ü'))?;
    let (prefix, tail) = snapshot.preedit.split_at(at);
    if !tail.chars().all(|ch| ch.is_ascii_alphabetic() || matches!(ch, 'ü' | 'Ü' | ' ' | '\'')) { return None; }
    let raw = spelling(&snapshot.raw_input);
    let suffix = spelling(tail);
    if suffix.is_empty() || !raw.ends_with(&suffix) || (prefix.is_empty() && raw != suffix) { return None; }
    let prefix = if prefix.chars().all(|ch| ch == '\'') && snapshot.raw_input.starts_with(prefix) { "" } else { prefix };
    Some((prefix.to_string(), letters(tail).len()))
}

fn observed_boundary(decoder: &dyn Decoder, snapshot: &DecoderSnapshot) -> Option<(String, usize)> {
    if decoder.native_editing() {
        decoder.composition_boundary(BoundaryPurpose::Editing).map(|boundary|
            (boundary.confirmed, boundary.unconfirmed_raw.bytes().filter(u8::is_ascii_alphabetic).count()))
    } else { confirmed_and_remaining(snapshot) }
}

fn apply(decoder: &mut dyn Decoder, action: &Action, delivery_id: u64) -> Result<String, i32> {
    match action {
        Action::Type(letter) => decoder.input(letter),
        Action::Backspace => decoder.backspace(),
        Action::Left => decoder.move_left(),
        Action::Right => decoder.move_right(),
        Action::PageDown => decoder.page_down(),
        Action::PageUp => decoder.page_up(),
        Action::Next => decoder.select_next(),
        Action::Prev => decoder.select_previous(),
        Action::NextLinear => decoder.select_next_linear(),
        Action::PrevLinear => decoder.select_previous_linear(),
        Action::Select(index, intent) => decoder.select_candidate(*index, delivery_id, *intent),
        Action::CommitRaw => Ok(decoder.commit_raw()),
        Action::Clear => { decoder.clear(); Ok(String::new()) }
        Action::ClearAfterSelection => { decoder.clear_after_selection(); Ok(String::new()) }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
struct State {
    raw: String,
    preedit: String,
    cursor_utf8: usize,
    confirmed: Option<String>,
    remaining_letters: Option<usize>,
    highlighted_index: usize,
    page_no: usize,
    is_last_page: bool,
}

fn state(snapshot: &DecoderSnapshot) -> State {
    let boundary = confirmed_and_remaining(snapshot);
    State {
        raw: snapshot.raw_input.clone(), preedit: snapshot.preedit.clone(), cursor_utf8: snapshot.cursor_utf8,
        confirmed: boundary.as_ref().map(|(prefix, _)| prefix.clone()),
        remaining_letters: boundary.map(|(_, letters)| letters),
        highlighted_index: snapshot.highlighted_index, page_no: snapshot.page_no,
        is_last_page: snapshot.is_last_page,
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
struct Queries {
    all_count: Option<usize>,
    // (requested maximum, returned count, complete sequence)
    prefixes: Vec<(usize, Option<(usize, bool)>)>,
    raw_echo: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Step {
    line: usize,
    delivery_id: u64,
    action: String,
    state: State,
    result: Result<String, i32>,
    shapes: Vec<Shape>,
    // Diagnostic only: candidate spelling and ordering are never compared.
    candidate_texts: Vec<String>,
    secondary_count: usize,
    queries: Queries,
    observation_errors: Vec<String>,
    legacy_boundary_checks: usize,
}

fn differences(left: &Step, right: &Step) -> Vec<String> {
    let mut differences = Vec::new();
    macro_rules! field {
        ($name:ident) => { if left.state.$name != right.state.$name { differences.push(stringify!($name).into()); } };
    }
    field!(raw); field!(preedit); field!(cursor_utf8); field!(confirmed); field!(remaining_letters);
    field!(highlighted_index); field!(page_no); field!(is_last_page);
    if left.result.as_ref().ok() != right.result.as_ref().ok() { differences.push("commit_text".into()); }
    if left.result.as_ref().err() != right.result.as_ref().err() { differences.push("return_status".into()); }
    if !structure_equal(&left.shapes, &right.shapes) { differences.push("candidate_structure".into()); }
    if left.secondary_count != right.secondary_count { differences.push("secondary_structure".into()); }
    if left.queries != right.queries { differences.push("query_contract".into()); }
    // Unknown coverage or an internally inconsistent observation is not a pass,
    // even if both implementations have the same uncertainty.
    if !left.observation_errors.is_empty() || !right.observation_errors.is_empty() { differences.push("observation_error".into()); }
    differences
}

fn same_snapshot(left: &DecoderSnapshot, right: &DecoderSnapshot) -> bool {
    state(left) == state(right) && left.candidates == right.candidates && left.secondary_candidates == right.secondary_candidates
}

// Frozen pre-Y2 consumer logic, deliberately independent of the new default
// implementation. Editing and Mixing had different acceptance/normalization rules.
fn legacy_editing_prefix(snapshot: &DecoderSnapshot) -> Option<&str> {
    fn spelling(text: &str) -> String {
        text.chars().filter_map(|ch| match ch {
            ' ' | '\'' => None,
            'ü' | 'Ü' => Some('v'),
            ch => Some(ch),
        }).collect()
    }
    let raw = spelling(&snapshot.raw_input);
    let preedit = snapshot.preedit.as_str();
    let mut end = preedit.len();
    let mut boundary = None;
    loop {
        let start = preedit[..end].trim_end_matches(|ch: char|
            ch.is_ascii_lowercase() || matches!(ch, 'ü' | 'Ü' | '\'')).len();
        if start == end { break; }
        let suffix = spelling(&preedit[start..]);
        if suffix.is_empty() || !raw.ends_with(&suffix) { break; }
        boundary = Some(start);
        end = preedit[..start].trim_end_matches(' ').len();
        if end == start { break; }
    }
    boundary.and_then(|at| {
        let before = preedit[..at].trim_end_matches(' ');
        if before.chars().next_back().is_some_and(|ch| ch.is_ascii_lowercase() || matches!(ch, 'ü' | 'Ü' | '\'')) {
            None
        } else { Some(&preedit[..at]) }
    })
}

fn legacy_mixing_boundary(snapshot: &DecoderSnapshot) -> Option<CompositionBoundary> {
    let at = snapshot.preedit.find(|ch: char| ch.is_ascii_alphabetic())?;
    let (prefix, tail) = snapshot.preedit.split_at(at);
    if prefix.is_ascii() { return None; }
    let rest: String = tail.chars().filter_map(|ch| match ch {
        'ü' | 'Ü' => Some('v'),
        ch if ch.is_ascii_alphabetic() => Some(ch.to_ascii_lowercase()),
        _ => None,
    }).collect();
    (!rest.is_empty()).then(|| CompositionBoundary { confirmed: prefix.into(), unconfirmed_raw: rest })
}

fn legacy_boundary_errors(snapshot: &DecoderSnapshot, editing: Option<CompositionBoundary>,
    mixing: Option<CompositionBoundary>) -> Vec<String> {
    let mut errors = Vec::new();
    if editing.as_ref().map(|boundary| boundary.confirmed.as_str()) != legacy_editing_prefix(snapshot) {
        errors.push("editing_boundary_changed_legacy_result".into());
    }
    if mixing != legacy_mixing_boundary(snapshot) { errors.push("mixing_boundary_changed_legacy_result".into()); }
    errors
}

fn query_contract(decoder: &dyn Decoder, page: &DecoderSnapshot) -> (Queries, Vec<String>) {
    let mut errors = Vec::new();
    if !decoder.native_editing() {
        errors.extend(legacy_boundary_errors(page, decoder.composition_boundary(BoundaryPurpose::Editing),
            decoder.composition_boundary(BoundaryPurpose::Mixing)));
    } else {
        let boundary = decoder.composition_boundary(BoundaryPurpose::Editing);
        if boundary != decoder.composition_boundary(BoundaryPurpose::Mixing)
            || boundary.as_ref().is_none_or(|boundary|
                !page.raw_input.ends_with(&boundary.unconfirmed_raw) || !page.preedit.starts_with(&boundary.confirmed)) {
            errors.push("native_boundary_inconsistent_with_raw_and_preedit".into());
        }
    }
    let all = decoder.snapshot_all_candidates();
    let mut prefixes = Vec::new();
    for maximum in [0, 1, 5, 8] {
        let prefix = decoder.snapshot_candidate_prefix(maximum);
        if let Some(prefix) = &prefix {
            if prefix.candidates.len() > maximum || prefix.page_no != 0 || prefix.highlighted_index != 0 {
                errors.push(format!("prefix_{maximum}_metadata"));
            }
            if let Some(all) = &all {
                if prefix.candidates != all.candidates.iter().take(maximum).cloned().collect::<Vec<_>>()
                    || prefix.is_last_page != (all.candidates.len() <= maximum) {
                    errors.push(format!("prefix_{maximum}_sequence"));
                }
            }
        }
        prefixes.push((maximum, prefix.map(|value| (value.candidates.len(), value.is_last_page))));
    }
    if let Some(all) = &all {
        if !all.is_last_page || all.page_no != 0 || all.highlighted_index != 0
            || all.raw_input != page.raw_input || all.preedit != page.preedit || all.cursor_utf8 != page.cursor_utf8
            || all.candidates.iter().enumerate().any(|(i, item)| item.source_index != Some(i)) {
            errors.push("all_metadata_or_source_index".into());
        }
    }
    let raw_echo = decoder.whole_prefix_has_raw_echo();
    if let Some(all) = &all {
        if !page.raw_input.is_empty() && !all.candidates.is_empty() {
            let expected = all.candidates.iter().any(|c| c.text.is_ascii() && c.text.eq_ignore_ascii_case(&page.raw_input));
            if raw_echo != expected { errors.push("raw_echo_full_sequence".into()); }
        }
    }
    if !same_snapshot(page, &decoder.snapshot()) { errors.push("query_mutated_page".into()); }
    (Queries { all_count: all.map(|value| value.candidates.len()), prefixes, raw_echo }, errors)
}

fn coverage(before: &DecoderSnapshot, after: &DecoderSnapshot, selected: &Result<String, i32>) -> Option<usize> {
    selected_coverage(confirmed_and_remaining(before), confirmed_and_remaining(after), after.raw_input.is_empty(), selected)
}

fn selected_coverage(before: Option<(String, usize)>, after: Option<(String, usize)>, cleared: bool,
    selected: &Result<String, i32>) -> Option<usize> {
    let (_, initial) = before?;
    let (_, final_letters) = after?;
    let commit = selected.as_ref().ok()?;
    if cleared && commit.is_empty() { return None; }
    initial.checked_sub(final_letters)
}

fn selected_text_matches(confirmed: &str, candidate: &str, commit: &str, after_confirmed: &str,
    cleared: bool) -> bool {
    let expected = confirmed.to_string() + candidate;
    if cleared { !commit.is_empty() && commit == expected && after_confirmed.is_empty() }
    else { commit.is_empty() && after_confirmed == expected }
}

// Rime replays the prefix for each visible candidate. An explicitly unlearned
// native decoder may fork an independently replayed composition after matching
// every snapshot; every shadow still performs a real candidate selection.
// Factories must isolate learning (the real runner disables UserDB in a temp schema).
fn trace_script(factory: &mut dyn FnMut() -> Box<dyn Decoder>, actions: &[(usize, Action)], script_id: u64, next_id: &mut u64) -> Vec<Step> {
    trace_script_with_spans(factory, actions, script_id, next_id, None)
}

struct SpanObservation {
    spans: Vec<Option<usize>>,
    snapshot_matches: bool,
    forks: Option<Vec<Box<dyn Decoder>>>,
}

type SpanObserver<'a> = dyn FnMut(&[(usize, Action)], &DecoderSnapshot) -> SpanObservation + 'a;

fn trace_script_with_spans(factory: &mut dyn FnMut() -> Box<dyn Decoder>, actions: &[(usize, Action)], script_id: u64, next_id: &mut u64,
    mut observe_spans: Option<&mut SpanObserver<'_>>) -> Vec<Step> {
    fn run(decoder: &mut dyn Decoder, action: &Action, next_id: &mut u64) -> Result<String, i32> {
        *next_id += 1;
        apply(decoder, action, (1 << 63) | *next_id)
    }
    let mut decoder = factory();
    let mut steps = Vec::new();
    for (index, (line, action)) in actions.iter().enumerate() {
        assert!(script_id < 1 << 31 && actions.len() < u32::MAX as usize);
        let delivery_id = (script_id << 32) | (index as u64 + 1);
        let result = apply(decoder.as_mut(), action, delivery_id);
        let page = decoder.snapshot();
        let (queries, mut errors) = query_contract(decoder.as_ref(), &page);
        if page.cursor_utf8 > page.preedit.len() || !page.preedit.is_char_boundary(page.cursor_utf8) {
            errors.push("cursor_not_utf8_boundary".into());
        }
        let before = observed_boundary(decoder.as_ref(), &page);
        if !page.raw_input.is_empty() && before.is_none() { errors.push("unknown_confirmed_boundary".into()); }
        let mut forks = None;
        let explicit_spans = observe_spans.as_mut().map(|observe| {
            let observation = observe(&actions[..=index], &page);
            if !observation.snapshot_matches || observation.spans.len() != page.candidates.len() { errors.push("explicit_span_replay_changed_state".into()); }
            if let Some(shadows) = observation.forks {
                if shadows.len() != page.candidates.len() { errors.push("copied_shadow_count".into()); }
                forks = Some(shadows.into_iter());
            }
            observation.spans
        });
        let mut shapes = Vec::new();
        for (slot, candidate) in page.candidates.iter().enumerate() {
            let explicit = explicit_spans.as_ref().and_then(|spans| spans.get(slot)).copied().flatten();
            let mut scratch = forks.as_mut().and_then(Iterator::next).unwrap_or_else(|| {
                let mut scratch = factory();
                for (_, action) in &actions[..=index] { let _ = run(scratch.as_mut(), action, next_id); }
                scratch
            });
            if !same_snapshot(&page, &scratch.snapshot()) || observed_boundary(scratch.as_ref(), &scratch.snapshot()) != before {
                errors.push(format!("candidate_{slot}_replay_changed_state"));
            }
            let selected = run(scratch.as_mut(), &Action::Select(slot, LearningIntent::DeliberateSelection), next_id);
            let after = scratch.snapshot();
            let after_boundary = observed_boundary(scratch.as_ref(), &after);
            if scratch.native_editing() && !before.as_ref().zip(after_boundary.as_ref()).zip(selected.as_ref().ok())
                .is_some_and(|(((confirmed, _), (after_confirmed, _)), commit)| selected_text_matches(
                    confirmed, &candidate.text, commit, after_confirmed, after.raw_input.is_empty())) {
                errors.push(format!("candidate_{slot}_selected_text_mismatch"));
            }
            let covered = selected_coverage(before.clone(), after_boundary,
                after.raw_input.is_empty(), &selected);
            if explicit_spans.is_some() && explicit != covered { errors.push(format!("candidate_{slot}_span_disagrees_with_selection")); }
            if covered.is_none() || covered == Some(0) { errors.push(format!("candidate_{slot}_unknown_coverage")); }
            shapes.push(Shape {
                covered_letters: covered,
                whole: covered.is_some() && covered == before.as_ref().map(|(_, letters)| *letters),
                single_character: candidate.text.chars().count() == 1,
            });
        }
        let mut observed_state = state(&page);
        observed_state.confirmed = before.as_ref().map(|(confirmed, _)| confirmed.clone());
        observed_state.remaining_letters = before.map(|(_, count)| count);
        steps.push(Step {
            line: *line, delivery_id, action: format!("{action:?}"), state: observed_state, result, shapes,
            candidate_texts: page.candidates.iter().map(|c| c.text.clone()).collect(),
            secondary_count: page.secondary_candidates.len(), queries, observation_errors: errors,
            legacy_boundary_checks: if decoder.native_editing() { 0 } else { 2 },
        });
    }
    steps
}

#[derive(Debug, Serialize, Deserialize)]
struct ScriptTrace { name: String, steps: Vec<Step> }

#[derive(Debug, Default, Serialize, Deserialize)]
struct Summary {
    total_scripts: usize,
    skipped_scripts: Vec<serde_json::Value>,
    skipped_script_count: usize,
    scripts: usize,
    steps: usize,
    mismatched_scripts: usize,
    mismatched_steps: usize,
    state_mismatched_steps: usize,
    intentional_sogou_steps: usize,
    residual_mismatched_steps: usize,
    intentional_sogou_by_kind: BTreeMap<String, usize>,
    intentional_sogou_differences: Vec<serde_json::Value>,
    candidate_mismatched_steps: usize,
    query_mismatched_steps: usize,
    reference_legacy_boundary_checks: usize,
    tested_legacy_boundary_checks: usize,
    final_input_scripts: usize,
    final_input_structure_matches: usize,
    by_kind: BTreeMap<String, usize>,
    first_differences: Vec<serde_json::Value>,
}

impl Summary {
    fn passed(&self) -> bool { self.scripts > 0 && self.mismatched_steps == 0 }
}

fn compare(left: &[ScriptTrace], right: &[ScriptTrace]) -> Summary {
    assert_eq!(left.len(), right.len(), "missing scripts");
    let mut summary = Summary::default();
    for (left, right) in left.iter().zip(right) {
        assert_eq!(left.name, right.name, "script ordering changed");
        assert_eq!(left.steps.len(), right.steps.len(), "missing steps");
        summary.scripts += 1;
        summary.total_scripts += 1;
        if left.steps.iter().all(|step| step.action.starts_with("Type(")) {
            summary.final_input_scripts += 1;
            if let (Some(left), Some(right)) = (left.steps.last(), right.steps.last()) {
                if left.state.page_no == 0 && right.state.page_no == 0
                    && left.observation_errors.is_empty() && right.observation_errors.is_empty()
                    && structure_equal(&left.shapes, &right.shapes) {
                    summary.final_input_structure_matches += 1;
                }
            }
        }
        let mut first = true;
        for (index, (left_step, right_step)) in left.steps.iter().zip(&right.steps).enumerate() {
            assert_eq!((left_step.line, &left_step.action), (right_step.line, &right_step.action));
            assert_eq!(left_step.delivery_id, right_step.delivery_id, "primary event identity must not depend on candidate count");
            summary.steps += 1;
            summary.reference_legacy_boundary_checks += left_step.legacy_boundary_checks;
            summary.tested_legacy_boundary_checks += right_step.legacy_boundary_checks;
            let kinds = differences(left_step, right_step);
            if kinds.is_empty() { continue; }
            summary.mismatched_steps += 1;
            if kinds.iter().any(|kind| !matches!(kind.as_str(), "candidate_structure" | "secondary_structure" | "query_contract")) {
                summary.state_mismatched_steps += 1;
            }
            if kinds.iter().any(|kind| matches!(kind.as_str(), "candidate_structure" | "secondary_structure")) {
                summary.candidate_mismatched_steps += 1;
            }
            if kinds.iter().any(|kind| kind == "query_contract") { summary.query_mismatched_steps += 1; }
            for kind in &kinds { *summary.by_kind.entry(kind.clone()).or_default() += 1; }
            if first {
                first = false;
                summary.mismatched_scripts += 1;
                summary.first_differences.push(serde_json::json!({
                    "script": left.name, "step": index + 1, "line": left_step.line,
                    "action": left_step.action, "kinds": kinds, "reference": left_step, "tested": right_step,
                }));
            }
        }
    }
    summary.residual_mismatched_steps = summary.mismatched_steps;
    summary
}

fn compare_zhixu(left: &[ScriptTrace], right: &[ScriptTrace]) -> Summary {
    let mut summary = compare(left, right);
    for (left, right) in left.iter().zip(right) {
        for (index, (reference, tested)) in left.steps.iter().zip(&right.steps).enumerate().skip(1) {
            let previous = &right.steps[index - 1];
            // Coverage measures the actual raw consumed by selection. In native
            // editing even a displayed prefix choice commits and ends the whole
            // composition, so its operational span becomes the remaining input.
            let editing_shapes: Vec<_> = previous.shapes.iter().map(|shape| Shape {
                covered_letters: tested.state.remaining_letters, whole: true,
                single_character: shape.single_character,
            }).collect();
            let retained_shapes = tested.shapes == previous.shapes
                || (tested.state.cursor_utf8 < tested.state.preedit.len() && tested.shapes == editing_shapes);
            if !matches!(tested.action.as_str(), "Left" | "Right")
                || tested.state.raw.is_empty() || tested.state.raw != previous.state.raw
                || reference.state.raw != tested.state.raw || reference.state.preedit != tested.state.preedit
                || reference.state.cursor_utf8 != tested.state.cursor_utf8 || reference.state.confirmed != tested.state.confirmed
                || reference.state.remaining_letters != tested.state.remaining_letters
                || tested.result != Ok(String::new()) || reference.result != Ok(String::new())
                || tested.candidate_texts != previous.candidate_texts
                || !retained_shapes || tested.queries != previous.queries
                || tested.state.page_no != previous.state.page_no
                || tested.state.highlighted_index != previous.state.highlighted_index
                || tested.state.is_last_page != previous.state.is_last_page
                || !tested.observation_errors.is_empty() || !reference.observation_errors.is_empty() {
                continue;
            }
            let kinds = differences(reference, tested);
            let intentional: Vec<_> = kinds.iter().filter(|kind| matches!(kind.as_str(),
                "candidate_structure" | "query_contract" | "page_no" | "highlighted_index" | "is_last_page"))
                .cloned().collect();
            if intentional.is_empty() { continue; }
            summary.intentional_sogou_steps += 1;
            for kind in &intentional { *summary.intentional_sogou_by_kind.entry(kind.clone()).or_default() += 1; }
            if intentional.len() == kinds.len() { summary.residual_mismatched_steps -= 1; }
            summary.intentional_sogou_differences.push(serde_json::json!({
                "script": right.name, "step": index + 1, "line": tested.line,
                "action": tested.action, "kinds": intentional,
                "reason": "Caret movement retains the complete reading menu and pager instead of Rime's focused segment menu",
                "basis": ["SogouKeyModelTests.testCoreCaretBoundariesKeepWholeReadingMenuAndReturnToEnd",
                    "state::tests::editing_menu_keeps_all_four_fields_while_caret_is_inside",
                    "state::tests::editing_rime_partial_selection_finishes_displayed_candidate"],
            }));
        }
    }
    summary
}

#[cfg(feature = "librime")]
mod real {
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};

    fn script_paths(path: &Path) -> Vec<PathBuf> {
        if path.is_file() { return vec![path.to_path_buf()]; }
        let mut files = Vec::new();
        for entry in fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() { files.extend(script_paths(&path)); }
            else if path.extension().is_some_and(|ext| ext == "keys") { files.push(path); }
        }
        files.sort();
        files
    }

    // Future Decoder implementations are added here, without changing the runner
    // or comparator. trace_script also accepts any test-supplied Decoder factory.
    fn decoder(kind: &str, shared: &str, user: &str) -> Box<dyn Decoder> {
        match kind {
            "rime" => Box::new(crate::rime::RimeDecoder::new(shared, user).expect("real librime")),
            "mock" => Box::<crate::state::MockDecoder>::default(),
            "zhixu" => Box::new(crate::zhixu::decoder::ZhixuDecoder::new()),
            _ => panic!("unknown tested Decoder {kind:?}"),
        }
    }

    fn materialize(source: &str, factory: &mut dyn FnMut() -> Box<dyn Decoder>, next_id: &mut u64) -> String {
        let mut lines = Vec::new();
        let mut resolve = false;
        for line in source.lines() {
            if line.trim() == "# resolve_last_page" {
                loop {
                    let actions = parse_script(&lines.join("\n")).unwrap();
                    *next_id += 1;
                    let trace = trace_script(factory, &actions, *next_id, next_id);
                    let last = trace.last().unwrap();
                    assert!(!last.shapes.is_empty() && last.observation_errors.is_empty(), "last-page resolution needs valid candidates");
                    if last.state.is_last_page {
                        lines.push(format!("# resolved_last_page page={}", last.state.page_no));
                        break;
                    }
                    if trace.len() >= 2 && last.action == "PageDown" {
                        assert!(last.state.page_no > trace[trace.len() - 2].state.page_no, "pager did not reach the end");
                    }
                    lines.push("pagedown".into());
                }
                continue;
            }
            if line.trim() == "# resolve_prefix" { resolve = true; continue; }
            if resolve && !line.trim().is_empty() && !line.trim().starts_with('#') {
                assert!(matches!(parse_script(line).unwrap()[0].1, Action::Select(_, LearningIntent::DeliberateSelection)));
                loop {
                    let actions = parse_script(&lines.join("\n")).unwrap();
                    *next_id += 1;
                    let trace = trace_script(factory, &actions, *next_id, next_id);
                    let last = trace.last().unwrap();
                    assert!(last.observation_errors.is_empty(), "cannot resolve from an uncertain observation: {last:?}");
                    if let Some(slot) = last.shapes.iter().position(|shape| !shape.whole && shape.covered_letters.is_some_and(|n| n > 0)) {
                        lines.push(format!("# resolved_prefix page={} covered_letters={}", last.state.page_no, last.shapes[slot].covered_letters.unwrap()));
                        lines.push(format!("select {slot} deliberate"));
                        break;
                    }
                    assert!(!last.state.is_last_page && !last.shapes.is_empty(), "no partial candidate for {source}");
                    if trace.len() >= 2 && last.action == "PageDown" {
                        assert!(last.state.page_no > trace[trace.len() - 2].state.page_no, "partial-candidate pager made no progress");
                    }
                    lines.push("pagedown".into());
                }
                resolve = false;
            } else { lines.push(line.to_string()); }
        }
        assert!(!resolve, "dangling resolve_prefix marker");
        lines.join("\n") + "\n"
    }

    #[test]
    #[ignore]
    fn decoder_contract_worker() {
        let Ok(kind) = std::env::var("Y0_WORKER_KIND") else { panic!("run decoder_contract_compare, not the worker directly") };
        let suite = PathBuf::from(std::env::var_os("Y0_SCRIPTS").expect("Y0_SCRIPTS"));
        let shared = std::env::var("Y0_SHARED").expect("Y0_SHARED");
        let output = PathBuf::from(std::env::var_os("Y0_WORKER_OUTPUT").unwrap());
        let user = output.with_extension("user");
        fs::create_dir(&user).unwrap();
        // Prevent diagnostic shadow selections from teaching subsequent sessions.
        // Only this newly created directory is writable; production resources stay untouched.
        fs::write(user.join("agent_pinyin.custom.yaml"), "patch:\n  translator/enable_user_dict: false\n  translator/enable_encoder: false\n  translator/encode_commit_history: false\n").unwrap();
        // Both implementations read exactly this public fixture. Pinning also
        // removes background dictionary-publication timing from the comparison.
        let dictionary = crate::zhixu::loader::load(Path::new(&shared), &user).expect("public-only dictionary");
        let _dictionary = crate::zhixu::pin_snapshot(Some(std::sync::Arc::new(crate::zhixu::Index::from_data(dictionary))));
        let _learning = crate::zhixu::pin_learning(None);
        let mut factory = || decoder(if kind == "resolve" { "rime" } else { &kind }, &shared, user.to_str().unwrap());
        drop(factory());
        if kind == "rime" || kind == "resolve" {
            let deployed = fs::read_to_string(user.join("build/agent_pinyin.schema.yaml")).unwrap();
            assert!(deployed.contains("enable_user_dict: false"), "probe must disable UserDB");
        }
        let paths = script_paths(&suite);
        assert!(!paths.is_empty(), "no scripts");
        let mut traces = Vec::new();
        let mut next_id = 0;
        if kind == "resolve" { fs::create_dir(&output).unwrap(); }
        for (script_index, path) in paths.into_iter().enumerate() {
            let relative = if suite.is_file() { Path::new(path.file_name().unwrap()) } else { path.strip_prefix(&suite).unwrap() };
            let name = relative.display().to_string();
            let source = fs::read_to_string(&path).unwrap();
            if kind == "resolve" {
                let resolved = materialize(&source, &mut factory, &mut next_id);
                let destination = output.join(relative);
                fs::create_dir_all(destination.parent().unwrap()).unwrap();
                fs::write(destination, resolved).unwrap();
                println!("Y0_RESOLVED\t{name}");
                continue;
            }
            assert!(!source.lines().any(|line| matches!(line.trim(), "# resolve_prefix" | "# resolve_last_page")), "unresolved script {name}");
            let actions = parse_script(&source).unwrap_or_else(|error| panic!("{name}: {error}"));
            let steps = if kind == "zhixu" {
                let mut observed = crate::zhixu::decoder::ZhixuDecoder::new();
                let mut observed_steps = 0;
                let mut observe = |prefix: &[(usize, Action)], page: &DecoderSnapshot| {
                    for (index, (_, action)) in prefix.iter().enumerate().skip(observed_steps) {
                        let _ = apply(&mut observed, action, (1 << 63) | (index as u64 + 1));
                    }
                    observed_steps = prefix.len();
                    let spans = (0..page.candidates.len()).map(|slot| observed.candidate_covered_letters(slot)).collect();
                    SpanObservation { spans, snapshot_matches: same_snapshot(page, &observed.snapshot()),
                        forks: Some((0..page.candidates.len()).map(|_| Box::new(observed.fork_unlearned_composition()) as Box<dyn Decoder>).collect()) }
                };
                trace_script_with_spans(&mut factory, &actions, script_index as u64 + 1, &mut next_id, Some(&mut observe))
            } else { trace_script(&mut factory, &actions, script_index as u64 + 1, &mut next_id) };
            println!("Y0_SCRIPT\t{kind}\t{name}\tsteps={}", steps.len());
            traces.push(ScriptTrace { name, steps });
        }
        if kind != "resolve" { fs::write(output, serde_json::to_vec(&traces).unwrap()).unwrap(); }
    }

    #[test]
    #[ignore]
    fn decoder_contract_compare() {
        let suite = std::env::var_os("Y0_SCRIPTS").expect("set Y0_SCRIPTS to a .keys file or directory");
        let shared = std::env::var_os("Y0_SHARED").expect("set Y0_SHARED to isolated public Rime resources");
        let tested = std::env::var("Y0_TESTED").unwrap_or_else(|_| "rime".into());
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let output = std::env::temp_dir().join(format!("zhixu-y0-{}-{stamp}", std::process::id()));
        fs::create_dir(&output).unwrap();
        println!("Y0_OUTPUT\t{}", output.display());
        let mut sides = Vec::new();
        let resolved = output.join("scripts");
        for (label, kind) in [("resolve", "resolve"), ("reference", "rime"), ("tested", tested.as_str())] {
            let trace = if kind == "resolve" { resolved.clone() } else { output.join(format!("{label}.json")) };
            let log = fs::File::create(output.join(format!("{label}.log"))).unwrap();
            let result = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "mixer::decoder_contract::real::decoder_contract_worker", "--ignored", "--nocapture", "--test-threads=1"])
                .env("Y0_WORKER_KIND", kind).env("Y0_WORKER_OUTPUT", &trace)
                .env("Y0_SCRIPTS", if kind == "resolve" { suite.as_os_str() } else { resolved.as_os_str() }).env("Y0_SHARED", &shared)
                .env("ZHIZI_SINGLE_LEARN", "0")
                .stdout(Stdio::from(log.try_clone().unwrap())).stderr(Stdio::from(log))
                .status().unwrap();
            assert!(result.success(), "{label} worker failed: {}", output.join(format!("{label}.log")).display());
            if kind != "resolve" {
                sides.push(serde_json::from_slice::<Vec<ScriptTrace>>(&fs::read(trace).unwrap()).unwrap());
            }
        }
        let summary = if tested == "zhixu" { compare_zhixu(&sides[0], &sides[1]) }
            else { compare(&sides[0], &sides[1]) };
        fs::write(output.join("summary.json"), serde_json::to_vec_pretty(&summary).unwrap()).unwrap();
        for first in &summary.first_differences { println!("Y0_FIRST\t{first}"); }
        println!("Y0_SUMMARY\ttested={tested}\tscripts={}\tsteps={}\tmismatched_scripts={}\tmismatched_steps={}\tby_kind={:?}",
            summary.scripts, summary.steps, summary.mismatched_scripts, summary.mismatched_steps, summary.by_kind);
        println!("Y1_SUMMARY\ttotal={}\tskipped={}\tstate_mismatched_steps={}\tfinal_input_structure={}/{}",
            summary.total_scripts, summary.skipped_scripts.len(), summary.state_mismatched_steps,
            summary.final_input_structure_matches, summary.final_input_scripts);
        println!("Y2_SUMMARY\tintentional_sogou_steps={}\tresidual_mismatched_steps={}\tintentional_by_kind={:?}",
            summary.intentional_sogou_steps, summary.residual_mismatched_steps, summary.intentional_sogou_by_kind);
        if tested == "mock" {
            assert!(summary.by_kind.contains_key("candidate_structure"), "negative control must detect structural differences");
            assert!(summary.by_kind.contains_key("preedit"), "negative control must detect state differences");
        } else {
            assert!(summary.passed(), "Decoder contract differences or no executed scripts; see summary.json");
        }
    }

    // Faster repair aid only: it deliberately omits candidate-selection shadows
    // and cannot replace decoder_contract_compare's complete acceptance run.
    #[test]
    #[ignore]
    fn decoder_contract_state_diagnostics() {
        let shared = std::env::var("Y0_SHARED").expect("isolated public-only fixture");
        let suite = PathBuf::from(std::env::var_os("Y0_SCRIPTS").unwrap());
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let output = std::env::temp_dir().join(format!("zhixu-y2-state-diagnostic-{}-{stamp}.json", std::process::id()));
        let user = output.with_extension("user");
        fs::create_dir(&user).unwrap();
        fs::write(user.join("agent_pinyin.custom.yaml"), "patch:\n  translator/enable_user_dict: false\n  translator/enable_encoder: false\n  translator/encode_commit_history: false\n").unwrap();
        let dictionary = crate::zhixu::loader::load(Path::new(&shared), &user).unwrap();
        let _dictionary = crate::zhixu::pin_snapshot(Some(std::sync::Arc::new(crate::zhixu::Index::from_data(dictionary))));
        let _learning = crate::zhixu::pin_learning(None);
        let mut traces = [Vec::new(), Vec::new()];
        for (script_index, path) in script_paths(&suite).iter().enumerate() {
            let name = path.strip_prefix(&suite).unwrap_or(path).display().to_string();
            let actions = parse_script(&fs::read_to_string(path).unwrap()).unwrap();
            for (side, kind) in ["rime", "zhixu"].iter().enumerate() {
                let mut decoder = decoder(kind, &shared, user.to_str().unwrap());
                let mut steps = Vec::new();
                for (index, (line, action)) in actions.iter().enumerate() {
                    let delivery_id = ((script_index as u64 + 1) << 32) | (index as u64 + 1);
                    let result = apply(decoder.as_mut(), action, delivery_id);
                    let snapshot = decoder.snapshot();
                    let boundary = observed_boundary(decoder.as_ref(), &snapshot);
                    let mut observed = state(&snapshot);
                    observed.confirmed = boundary.as_ref().map(|(prefix, _)| prefix.clone());
                    observed.remaining_letters = boundary.map(|(_, count)| count);
                    let (queries, observation_errors) = query_contract(decoder.as_ref(), &snapshot);
                    steps.push(Step { line: *line, delivery_id, action: format!("{action:?}"), state: observed,
                        result, shapes: Vec::new(), candidate_texts: snapshot.candidates.iter().map(|item| item.text.clone()).collect(),
                        secondary_count: snapshot.secondary_candidates.len(), queries, observation_errors,
                        legacy_boundary_checks: if decoder.native_editing() { 0 } else { 2 } });
                }
                traces[side].push(ScriptTrace { name: name.clone(), steps });
            }
        }
        let mut mismatches = Vec::new();
        for (reference, tested) in traces[0].iter().zip(&traces[1]) {
            for (index, (left, right)) in reference.steps.iter().zip(&tested.steps).enumerate() {
                let kinds = differences(left, right);
                if !kinds.is_empty() { mismatches.push(serde_json::json!({
                    "script": reference.name, "step": index + 1, "kinds": kinds, "reference": left, "tested": right,
                })); }
            }
        }
        let report = serde_json::json!({"diagnostic_only": true, "omits": "candidate-selection coverage shadows",
            "summary": compare_zhixu(&traces[0], &traces[1]), "differences": mismatches});
        fs::write(&output, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
        println!("Y2_STATE_DIAGNOSTIC_ONLY\t{}", output.display());
    }


    #[test]
    #[ignore]
    fn decoder_contract_native_menu_matches_fresh_complete_sequence() {
        let shared = std::env::var("Y0_SHARED").expect("isolated public-only fixture");
        let suite = PathBuf::from(std::env::var_os("Y0_SCRIPTS").unwrap());
        let comparison = PathBuf::from(std::env::var_os("Y2_COMPARISON").expect("completed formal comparison directory"));
        let reference: Vec<ScriptTrace> = serde_json::from_slice(&fs::read(comparison.join("reference.json")).unwrap()).unwrap();
        let tested: Vec<ScriptTrace> = serde_json::from_slice(&fs::read(comparison.join("tested.json")).unwrap()).unwrap();
        let mut wanted = BTreeMap::new();
        for (left, right) in reference.iter().zip(&tested) {
            assert_eq!(left.name, right.name);
            assert_eq!(left.steps.len(), right.steps.len());
            for (index, (left, step)) in left.steps.iter().zip(&right.steps).enumerate() {
                if !differences(left, step).is_empty() { wanted.insert((right.name.clone(), index + 1), step.clone()); }
            }
        }
        assert!(!wanted.is_empty(), "no reviewed differences");
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let user = std::env::temp_dir().join(format!("zhixu-y2-menu-proof-{}-{stamp}", std::process::id()));
        fs::create_dir(&user).unwrap();
        let dictionary = crate::zhixu::loader::load(Path::new(&shared), &user).unwrap();
        let _dictionary = crate::zhixu::pin_snapshot(Some(std::sync::Arc::new(crate::zhixu::Index::from_data(dictionary))));
        let _learning = crate::zhixu::pin_learning(None);
        let mut cache = BTreeMap::new();
        let mut proof = Vec::new();
        for (script_index, path) in script_paths(&suite).iter().enumerate() {
            let name = path.strip_prefix(&suite).unwrap_or(path).display().to_string();
            let actions = parse_script(&fs::read_to_string(path).unwrap()).unwrap();
            let mut decoder = crate::zhixu::decoder::ZhixuDecoder::new();
            for (index, (_, action)) in actions.iter().enumerate() {
                let result = apply(&mut decoder, action, ((script_index as u64 + 1) << 32) | (index as u64 + 1));
                let Some(expected) = wanted.get(&(name.clone(), index + 1)) else { continue; };
                let page = decoder.snapshot();
                let boundary = decoder.composition_boundary(BoundaryPurpose::Editing).unwrap();
                let mut actual = state(&page);
                actual.confirmed = Some(boundary.confirmed.clone());
                actual.remaining_letters = Some(boundary.unconfirmed_raw.bytes().filter(u8::is_ascii_alphabetic).count());
                assert_eq!(actual, expected.state, "formal trace changed: {name}:{}", index + 1);
                assert_eq!(result, expected.result);
                assert_eq!(page.candidates.iter().map(|item| item.text.clone()).collect::<Vec<_>>(), expected.candidate_texts);
                let fresh = cache.entry(boundary.unconfirmed_raw.clone()).or_insert_with(|| {
                    let mut fresh = crate::zhixu::decoder::ZhixuDecoder::new();
                    fresh.input(&boundary.unconfirmed_raw).unwrap();
                    fresh
                });
                let complete = decoder.snapshot_all_candidates().unwrap();
                assert_eq!(complete.candidates, fresh.snapshot_all_candidates().unwrap().candidates,
                    "whole sequence differs: {name}:{}", index + 1);
                let mut paged = fresh.fork_unlearned_composition();
                for _ in 0..page.page_no { paged.page_down().unwrap(); }
                for _ in 0..page.highlighted_index { paged.select_next().unwrap(); }
                let fresh_page = paged.snapshot();
                assert_eq!(page.candidates, fresh_page.candidates);
                assert_eq!((page.page_no, page.highlighted_index, page.is_last_page),
                    (fresh_page.page_no, fresh_page.highlighted_index, fresh_page.is_last_page));
                let live_queries = query_contract(&decoder, &page);
                assert!(live_queries.1.is_empty());
                assert_eq!(live_queries.0, expected.queries);
                assert_eq!(live_queries, query_contract(&paged, &fresh_page));
                proof.push(serde_json::json!({"script": name, "step": index + 1,
                    "raw": page.raw_input, "confirmed": boundary.confirmed, "unconfirmed_raw": boundary.unconfirmed_raw,
                    "page_no": page.page_no, "highlighted_index": page.highlighted_index,
                    "complete_candidates": complete.candidates.len(), "matches_formal_trace": true,
                    "fresh_end_complete_sequence_and_paged_queries_match": true}));
            }
        }
        assert_eq!(proof.len(), wanted.len(), "every formal difference needs evidence");
        let output = comparison.join("native-menu-proof.json");
        fs::write(&output, serde_json::to_vec_pretty(&serde_json::json!({
            "checked_steps": proof.len(), "fresh_raw_compositions": cache.len(), "steps": proof,
            "does_not_waive_strict_differences": true,
        })).unwrap()).unwrap();
        println!("Y2_NATIVE_MENU_PROOF\tsteps={}\tfresh_raw_compositions={}\t{}", proof.len(), cache.len(), output.display());
    }

}

#[test]
fn decoder_contract_parse_script() {
    let actions = parse_script("# fixture\n\ntype n\r\ntype '\nselect 2\nselect 0 no_learn\nselect 1 required\nleft\nright\nbackspace\npagedown\npageup\nnext\nprev\nnext_linear\nprev_linear\ncommit_raw\nclear\nclear_after_selection\n").unwrap();
    assert_eq!(actions.len(), 17);
    assert_eq!(actions[0], (3, Action::Type("n".into())));
    assert_eq!(actions[2].1, Action::Select(2, LearningIntent::DeliberateSelection));
    assert_eq!(actions[3].1, Action::Select(0, LearningIntent::NoLearn));
    assert_eq!(actions[4].1, Action::Select(1, LearningIntent::RequiredSegmentCommit));
    for bad in ["type ni", "type ü", "type A", "select -1", "select", "select 0 learn", "left 2", "unknown", "# empty\n"] {
        assert!(parse_script(bad).is_err(), "{bad}");
    }
    assert!(parse_script("type n\nnope").unwrap_err().contains("line 2"));
}

#[test]
fn decoder_contract_structure_is_an_unordered_multiset() {
    let whole = Shape { covered_letters: Some(5), whole: true, single_character: false };
    let partial = Shape { covered_letters: Some(2), whole: false, single_character: true };
    assert!(structure_equal(&[whole.clone(), partial.clone()], &[partial.clone(), whole.clone()]));
    assert!(!structure_equal(&[whole.clone(), partial.clone()], &[whole.clone(), whole]));
    assert!(!structure_equal(&[partial.clone()], &[partial.clone(), partial.clone()]));
    assert!(!structure_equal(&[partial], &[Shape { covered_letters: Some(3), whole: false, single_character: true }]));
}

#[test]
fn decoder_contract_confirmed_boundary_uses_rendered_suffix() {
    let snapshot = |raw: &str, preedit: &str| DecoderSnapshot { raw_input: raw.into(), preedit: preedit.into(), ..Default::default() };
    assert_eq!(confirmed_and_remaining(&snapshot("nihao", "ni hao")), Some(("".into(), 5)));
    assert_eq!(confirmed_and_remaining(&snapshot("nihao", "你hao")), Some(("你".into(), 3)));
    assert_eq!(confirmed_and_remaining(&snapshot("nvhao", "女hao")), Some(("女".into(), 3)));
    assert_eq!(confirmed_and_remaining(&snapshot("nvhao", "nü hao")), Some(("".into(), 5)));
    assert_eq!(confirmed_and_remaining(&snapshot("jvhao", "ju hao")), Some(("".into(), 5)));
    assert_eq!(confirmed_and_remaining(&snapshot("ni'hao", "你hao")), Some(("你".into(), 3)));
    assert_eq!(confirmed_and_remaining(&snapshot("'", "'")), Some(("".into(), 0)));
    assert_eq!(confirmed_and_remaining(&snapshot("'ni", "'ni")), Some(("".into(), 2)));
    assert_eq!(confirmed_and_remaining(&snapshot("abc", "你xyz")), None);
}

#[test]
fn decoder_contract_rejects_silent_loss_on_selection() {
    let before = DecoderSnapshot { raw_input: "nihao".into(), preedit: "ni hao".into(), ..Default::default() };
    let empty = DecoderSnapshot::default();
    assert_eq!(coverage(&before, &empty, &Ok(String::new())), None);
    assert_eq!(coverage(&before, &empty, &Ok("你好".into())), Some(5));
    assert_eq!(coverage(&before, &empty, &Err(-104)), None);
}

#[test]
fn decoder_contract_reports_first_difference_and_counts_all_steps() {
    let actions = parse_script("type n\ntype i\nselect 0\nclear").unwrap();
    let mut factory = || -> Box<dyn Decoder> { Box::<crate::state::MockDecoder>::default() };
    let baseline = trace_script(&mut factory, &actions, 1, &mut 0);
    assert!(baseline.iter().all(|step| step.observation_errors.is_empty()));
    let mut changed = baseline.clone();
    changed[0].candidate_texts.reverse();
    changed[0].candidate_texts[0] = "不同的文字".into();
    changed[0].shapes.reverse();
    assert!(differences(&baseline[0], &changed[0]).is_empty());
    changed[1].state.highlighted_index += 1;
    changed[2].result = Ok("different commit".into());
    let summary = compare(&[ScriptTrace { name: "fixture".into(), steps: baseline }], &[ScriptTrace { name: "fixture".into(), steps: changed }]);
    assert_eq!((summary.scripts, summary.steps, summary.mismatched_scripts, summary.mismatched_steps), (1, 4, 1, 2));
    assert_eq!(summary.first_differences[0]["step"], 2);
    assert_eq!(summary.by_kind.get("highlighted_index"), Some(&1));
    assert_eq!(summary.by_kind.get("commit_text"), Some(&1));
}

#[test]
fn decoder_contract_unknown_and_nonrepeatable_observations_never_pass() {
    let actions = parse_script("type n").unwrap();
    let mut creations = 0;
    let mut factory = || -> Box<dyn Decoder> {
        let mut decoder = crate::state::MockDecoder::default();
        if creations > 0 { decoder.input("z").unwrap(); }
        creations += 1;
        Box::new(decoder)
    };
    let trace = trace_script(&mut factory, &actions, 1, &mut 0);
    assert!(trace[0].observation_errors.iter().any(|error| error.ends_with("replay_changed_state")));
    assert!(differences(&trace[0], &trace[0]).contains(&"observation_error".to_string()));
}

#[test]
fn decoder_contract_parser_covers_all_fixed_scripts() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../host/probes/decoder_contract_scripts/fixed");
    let mut count = 0;
    for entry in std::fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|extension| extension == "keys") {
            parse_script(&std::fs::read_to_string(&path).unwrap()).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
            count += 1;
        }
    }
    assert_eq!(count, 20);
}

#[test]
fn decoder_contract_main_ids_do_not_depend_on_shadow_candidate_count() {
    let mut factory = || -> Box<dyn Decoder> { Box::<crate::state::MockDecoder>::default() };
    let empty = trace_script(&mut factory, &parse_script("clear\nclear").unwrap(), 1, &mut 0);
    let candidates = trace_script(&mut factory, &parse_script("type n\nclear").unwrap(), 1, &mut 0);
    assert_eq!(empty[1].delivery_id, candidates[1].delivery_id);
    let another_script = trace_script(&mut factory, &parse_script("clear\nclear").unwrap(), 2, &mut 0);
    assert_ne!(empty[1].delivery_id, another_script[1].delivery_id);
}

#[test]
fn decoder_contract_no_executed_scripts_is_not_a_pass() {
    let mut summary = compare(&[], &[]);
    summary.total_scripts = 1;
    summary.skipped_script_count = 1;
    summary.skipped_scripts.push(serde_json::json!({"script": "y2.keys", "reasons": ["Y2: pinyin cursor movement"]}));
    assert_eq!(summary.scripts, 0);
    assert!(!summary.passed());
    let json = serde_json::to_value(summary).unwrap();
    assert_eq!(json["skipped_script_count"], 1);
    assert_eq!(json["total_scripts"], 1);
    assert_eq!(json["scripts"], 0);
}

#[test]
fn decoder_contract_y2_explicit_spans_match_real_partial_selection() {
    use crate::zhixu::{loader::{DictionaryData, Entry}, Index};
    let dictionary = Index::from_data(DictionaryData { entries: [
        ("你", "ni", 100), ("好", "hao", 100), ("你好", "ni hao", 1000),
    ].into_iter().map(|(text, reading, weight)| Entry {
        text: text.into(), reading: reading.into(), weight, layer: "public_fixture".into(),
    }).collect() });
    crate::zhixu::with_index(std::sync::Arc::new(dictionary), || {
        let actions = parse_script("type n\ntype i\ntype h\ntype a\ntype o").unwrap();
        let mut factory = || -> Box<dyn Decoder> { Box::new(crate::zhixu::decoder::ZhixuDecoder::new()) };
        let mut observe = |prefix: &[(usize, Action)], page: &DecoderSnapshot| {
            let mut decoder = crate::zhixu::decoder::ZhixuDecoder::new();
            for (index, (_, action)) in prefix.iter().enumerate() { apply(&mut decoder, action, index as u64 + 1).unwrap(); }
            let spans = (0..page.candidates.len()).map(|slot| decoder.candidate_covered_letters(slot)).collect();
            SpanObservation { spans, snapshot_matches: same_snapshot(page, &decoder.snapshot()),
                forks: Some((0..page.candidates.len()).map(|_| Box::new(decoder.fork_unlearned_composition()) as Box<dyn Decoder>).collect()) }
        };
        let trace = trace_script_with_spans(&mut factory, &actions, 1, &mut 0, Some(&mut observe));
        let replayed = trace_script(&mut factory, &actions, 1, &mut 0);
        assert!(trace.iter().zip(&replayed).all(|(forked, replayed)| differences(forked, replayed).is_empty()));
        let last = trace.last().unwrap();
        assert!(last.observation_errors.is_empty(), "{:?}", last.observation_errors);
        assert!(last.shapes.contains(&Shape { covered_letters: Some(5), whole: true, single_character: false }));
        assert!(last.shapes.contains(&Shape { covered_letters: Some(2), whole: false, single_character: true }));
    });
}

#[test]
fn decoder_contract_separates_state_from_candidate_counts_and_final_input_rate() {
    let mut factory = || -> Box<dyn Decoder> { Box::<crate::state::MockDecoder>::default() };
    let baseline = trace_script(&mut factory, &parse_script("type n\ntype i").unwrap(), 1, &mut 0);
    let mut changed = baseline.clone();
    changed[0].shapes.clear();
    changed[0].queries.all_count = Some(999);
    let summary = compare(&[ScriptTrace { name: "typing".into(), steps: baseline }], &[ScriptTrace { name: "typing".into(), steps: changed }]);
    assert_eq!(summary.state_mismatched_steps, 0);
    assert_eq!((summary.candidate_mismatched_steps, summary.query_mismatched_steps), (1, 1));
    assert_eq!((summary.final_input_scripts, summary.final_input_structure_matches), (1, 1));
}

#[test]
fn decoder_contract_y2_verifies_partial_spans_by_selecting() {
    let mut factory = || -> Box<dyn Decoder> { Box::<crate::state::MockDecoder>::default() };
    let mut observe = |_: &[(usize, Action)], page: &DecoderSnapshot| {
        SpanObservation { spans: vec![Some(1); page.candidates.len()], snapshot_matches: true, forks: None }
    };
    let actions = parse_script("type n\ntype i").unwrap();
    let trace = trace_script_with_spans(&mut factory, &actions, 1, &mut 0, Some(&mut observe));
    assert!(trace[1].observation_errors.iter().any(|error| error.ends_with("span_disagrees_with_selection")));
}

#[test]
fn decoder_contract_sogou_classification_requires_retained_menu_and_never_hides_state_errors() {
    let mut factory = || -> Box<dyn Decoder> { Box::<crate::state::MockDecoder>::default() };
    let tested = trace_script(&mut factory, &parse_script("type n\ntype i\nleft").unwrap(), 1, &mut 0);
    let mut reference = tested.clone();
    reference[2].shapes.truncate(1);
    reference[2].queries.all_count = Some(1);
    let compare_traces = |reference: Vec<Step>, tested: Vec<Step>| {
        compare_zhixu(&[ScriptTrace { name: "cursor".into(), steps: reference }],
            &[ScriptTrace { name: "cursor".into(), steps: tested }])
    };
    let summary = compare_traces(reference.clone(), tested.clone());
    assert_eq!(summary.intentional_sogou_steps, 1);
    assert_eq!(summary.residual_mismatched_steps, 0);
    assert!(!summary.passed(), "classification must not change strict equality's exit status");
    assert_eq!(summary.intentional_sogou_differences[0]["step"], 3);
    reference[2].state.raw.push('a');
    let summary = compare_traces(reference, tested.clone());
    assert_eq!(summary.intentional_sogou_steps, 0);
    assert_eq!(summary.residual_mismatched_steps, 1);
    let mut changed_menu = tested.clone();
    changed_menu[2].state.highlighted_index += 1;
    let summary = compare_traces(tested, changed_menu);
    assert_eq!(summary.intentional_sogou_steps, 0);
    assert_eq!(summary.residual_mismatched_steps, 1);
}

#[test]
fn decoder_contract_checks_both_legacy_boundary_views_independently() {
    for (raw, preedit, prefix, mixing) in [
        ("nihao", "ni hao", Some(""), None),
        ("nihao", "你hao", Some("你"), Some(("你", "hao"))),
        ("nvlvse", "女lü se", Some("女"), Some(("女", "lvse"))),
        ("nihaoma", "你hao'ma", Some("你"), Some(("你", "haoma"))),
        ("jvhao", "ju hao", None, None),
        ("nvhao", "API女hao", Some("API女"), None),
        ("nihao", "你好", None, None),
    ] {
        let snapshot = DecoderSnapshot { raw_input: raw.into(), preedit: preedit.into(), ..Default::default() };
        assert_eq!(legacy_editing_prefix(&snapshot), prefix);
        let editing = prefix.map(|confirmed| CompositionBoundary { confirmed: confirmed.into(), unconfirmed_raw: String::new() });
        let mixing = mixing.map(|(confirmed, unconfirmed_raw)| CompositionBoundary { confirmed: confirmed.into(), unconfirmed_raw: unconfirmed_raw.into() });
        assert!(legacy_boundary_errors(&snapshot, editing.clone(), mixing.clone()).is_empty());
        let wrong = Some(CompositionBoundary { confirmed: "wrong".into(), unconfirmed_raw: "wrong".into() });
        assert_eq!(legacy_boundary_errors(&snapshot, wrong.clone(), mixing).len(), 1);
        assert_eq!(legacy_boundary_errors(&snapshot, editing, wrong).len(), 1);
    }
}

#[test]
fn decoder_contract_observes_native_confirmed_only_state_without_guessing() {
    struct BoundaryFixture;
    impl Decoder for BoundaryFixture {
        fn native_editing(&self) -> bool { true }
        fn composition_boundary(&self, _purpose: BoundaryPurpose) -> Option<CompositionBoundary> {
            Some(CompositionBoundary { confirmed: "API你".into(), unconfirmed_raw: String::new() })
        }
        fn snapshot(&self) -> DecoderSnapshot {
            DecoderSnapshot { raw_input: "apini".into(), preedit: "API你".into(), cursor_utf8: 6,
                ..Default::default() }
        }
        fn input(&mut self, _: &str) -> Result<String, i32> { unreachable!() }
        fn backspace(&mut self) -> Result<String, i32> { unreachable!() }
        fn move_left(&mut self) -> Result<String, i32> { unreachable!() }
        fn move_right(&mut self) -> Result<String, i32> { unreachable!() }
        fn page_up(&mut self) -> Result<String, i32> { unreachable!() }
        fn page_down(&mut self) -> Result<String, i32> { unreachable!() }
        fn select_previous(&mut self) -> Result<String, i32> { unreachable!() }
        fn select_next(&mut self) -> Result<String, i32> { unreachable!() }
        fn select_candidate(&mut self, _: usize, _: u64, _: LearningIntent) -> Result<String, i32> { unreachable!() }
        fn delete_candidate(&mut self, _: usize) -> Result<(), i32> { unreachable!() }
        fn commit_raw(&mut self) -> String { unreachable!() }
        fn clear(&mut self) { unreachable!() }
    }
    let decoder = BoundaryFixture;
    // Protocol fixture, not a claim that current Rime can produce this state.
    assert_eq!(confirmed_and_remaining(&decoder.snapshot()), None);
    assert_eq!(observed_boundary(&decoder, &decoder.snapshot()), Some(("API你".into(), 0)));
    assert!(query_contract(&decoder, &decoder.snapshot()).1.is_empty());
}

#[test]
#[ignore]
fn decoder_contract_native_middle_edits_match_complete_end_caret_menu() {
    let shared = std::env::var_os("Y0_SHARED").expect("isolated public-only fixture");
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let user = std::env::temp_dir().join(format!("zhixu-y2-menu-{}-{stamp}", std::process::id()));
    std::fs::create_dir(&user).unwrap();
    let dictionary = crate::zhixu::loader::load(std::path::Path::new(&shared), &user).unwrap();
    let _dictionary = crate::zhixu::pin_snapshot(Some(std::sync::Arc::new(crate::zhixu::Index::from_data(dictionary))));
    let _learning = crate::zhixu::pin_learning(None);
    let actions = parse_script(include_str!("decoder_contract_scripts/fixed/11-middle-edit.keys")).unwrap();
    let mut decoder = crate::zhixu::decoder::ZhixuDecoder::new();
    let mut checked = 0;
    for (index, (_, action)) in actions.iter().enumerate() {
        apply(&mut decoder, action, index as u64 + 1).unwrap();
        let page = decoder.snapshot();
        if page.cursor_utf8 >= page.preedit.len() || !matches!(action, Action::Type(_) | Action::Backspace) { continue; }
        let boundary = decoder.composition_boundary(BoundaryPurpose::Editing).unwrap();
        let mut fresh = crate::zhixu::decoder::ZhixuDecoder::new();
        fresh.input(&boundary.unconfirmed_raw).unwrap();
        let complete = decoder.snapshot_all_candidates().unwrap();
        let fresh_complete = fresh.snapshot_all_candidates().unwrap();
        assert!(!complete.candidates.is_empty(), "a vacuous menu proves nothing");
        assert_eq!(complete.candidates, fresh_complete.candidates);
        assert_eq!(page.candidates, fresh.snapshot().candidates);
        assert_eq!(query_contract(&decoder, &page), query_contract(&fresh, &fresh.snapshot()));
        println!("Y2_WHOLE_MENU\tstep={}\taction={action:?}\traw={}\tcomplete_candidates={}\tvisible={:?}",
            index + 1, boundary.unconfirmed_raw, complete.candidates.len(), page.candidates.iter().map(|item| &item.text).collect::<Vec<_>>());
        checked += 1;
    }
    assert_eq!(checked, 2);
}

#[test]
fn decoder_contract_rejects_a_bad_copied_shadow_state() {
    let mut factory = || -> Box<dyn Decoder> { Box::<crate::state::MockDecoder>::default() };
    let mut observe = |_: &[(usize, Action)], page: &DecoderSnapshot| SpanObservation {
        spans: vec![Some(1); page.candidates.len()], snapshot_matches: true,
        forks: Some((0..page.candidates.len()).map(|_| Box::<crate::state::MockDecoder>::default() as Box<dyn Decoder>).collect()),
    };
    let trace = trace_script_with_spans(&mut factory, &parse_script("type n").unwrap(), 1, &mut 0, Some(&mut observe));
    assert!(trace[0].observation_errors.iter().any(|error| error.ends_with("replay_changed_state")));
    assert!(trace[0].observation_errors.iter().any(|error| error.ends_with("unknown_coverage")));
}

#[test]
fn decoder_contract_native_shadow_commit_cannot_lose_or_deduplicate_confirmed_text() {
    assert!(selected_text_matches("你", "你", "你你", "", true));
    assert!(selected_text_matches("你", "好", "", "你好", false));
    assert!(!selected_text_matches("你", "你", "你", "", true));
    assert!(!selected_text_matches("你", "好", "", "", true));
    assert!(!selected_text_matches("你", "好", "", "好", false));
}
