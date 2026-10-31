//! How 顺手·知字's scores change the page, shared by every host (macOS calls these
//! through the C ABI, the Rust host directly), so the two platforms cannot drift:
//!
//! - C3: a whole-reading composition leads when P(text | context) x P(keys | text), the
//!   model's log probability minus the typed edit cost of reaching it, beats
//!   the decoder's first by more than `COMPOSITION_LEAD_MARGIN` (taifuzale -> 太复杂了).
//! - C2: among candidates that cover the whole input, another reading or length leads
//!   when its score - 6 x rank beats the first; C1: homophones of the same reading are
//!   reordered by score - 2 x rank among their own slots.
//! - The owner's settled habits (a pair chosen at least 10 times at 93 percent or more
//!   in their own records) keep their first candidate in both.
//!
//! The hosts keep only what is theirs: running the model, its time budget, and the
//! preceding text they can see.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, OnceLock, RwLock};

/// Owner records: above 8 the lead replaced silent garbage (你系按看下 -> 你先看下)
/// about nine times in ten; between 4 and 8 it misread intent more often.
pub const COMPOSITION_LEAD_MARGIN: f64 = 8.0;
/// When the typed letters do not spell whole pinyin (zhedifangy, pinlvdt, nicshiyong)
/// the decoder's first is itself a guess at abbreviations or a slip, so less is needed to
/// replace it. External set 2026-10-11 at 6: 58.4% -> 59.8% first, the owner's October
/// set 89.7% -> 89.9% (你测试用, 都点了, 能发了); at 4 full-pinyin words broke (制定).
pub const GUESSED_COMPOSITION_LEAD_MARGIN: f64 = 6.0;
/// Legacy mean edit cost and multiplier for normalized C3 costs (P(keys | text)).
/// Owner replay 2026-10-10: at 4, one-slip repairs led with 装新版 over the heavy-garble
/// 换个新版, and 真没想到, 更不用说, 清单列出来 over garbage.
pub const EDIT_PENALTY: f64 = 4.0;
pub const COMPOSITION_COMMENT: &str = "整句 · 顺手·知字";
pub const HEAVY_GARBLE_COMMENT: &str = "重度乱拼 · 本地索引";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CorrectionSettings {
    pub enabled: bool,
    pub typed_costs: bool,
    pub multiple_readings: bool,
    pub trailing_letter: bool,
}

impl Default for CorrectionSettings {
    fn default() -> Self {
        Self { enabled: true, typed_costs: true, multiple_readings: true, trailing_letter: true }
    }
}

fn correction_settings_from(value: impl Fn(&str) -> Option<String>) -> CorrectionSettings {
    CorrectionSettings {
        enabled: value("ZHIZI_CORRECTIONS").as_deref() != Some("0"),
        typed_costs: value("ZHIZI_EDIT_COSTS").as_deref() != Some("flat"),
        multiple_readings: value("ZHIZI_CORRECTION_READINGS").as_deref() != Some("0"),
        trailing_letter: value("ZHIZI_TRAILING_LETTER").as_deref() != Some("0"),
    }
}

// The high bit distinguishes a shell's explicit choice from startup environment defaults.
static CORRECTION_SETTINGS: AtomicU8 = AtomicU8::new(0);

pub fn settings() -> CorrectionSettings {
    #[cfg(test)]
    if let Some(settings) = TEST_CORRECTION_SETTINGS.with(|value| value.get()) {
        return settings;
    }
    let bits = CORRECTION_SETTINGS.load(Ordering::Acquire);
    if bits & 0x80 != 0 {
        return CorrectionSettings {
            enabled: bits & 1 != 0,
            typed_costs: bits & 2 != 0,
            multiple_readings: bits & 4 != 0,
            trailing_letter: bits & 8 != 0,
        };
    }
    static DEFAULTS: OnceLock<CorrectionSettings> = OnceLock::new();
    *DEFAULTS.get_or_init(|| correction_settings_from(|name| std::env::var(name).ok()))
}

/// Process-wide core settings; callers invalidate any page/composition cache after changing them.
pub fn set_correction_settings(settings: CorrectionSettings) {
    let bits = 0x80 | u8::from(settings.enabled) | (u8::from(settings.typed_costs) << 1)
        | (u8::from(settings.multiple_readings) << 2) | (u8::from(settings.trailing_letter) << 3);
    CORRECTION_SETTINGS.store(bits, Ordering::Release);
}

#[cfg(test)]
thread_local! {
    static TEST_CORRECTION_SETTINGS: std::cell::Cell<Option<CorrectionSettings>> = const { std::cell::Cell::new(None) };
}

#[cfg(test)]
pub(crate) fn with_correction_settings<T>(settings: CorrectionSettings, run: impl FnOnce() -> T) -> T {
    struct Restore(Option<CorrectionSettings>);
    impl Drop for Restore {
        fn drop(&mut self) {
            TEST_CORRECTION_SETTINGS.with(|value| value.set(self.0));
        }
    }
    let _restore = Restore(TEST_CORRECTION_SETTINGS.with(|value| value.replace(Some(settings))));
    run()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditKind {
    Transposition,
    AdjacentKey,
    MissingLetter,
    ExtraLetter,
    OtherSubstitution,
}

// QWERTY horizontal/diagonal neighbors, symmetric. Include z/d as supplied by the
// owner's s/d/z neighborhood; d/l and n/l remain pronunciation substitutions.
pub const QWERTY_NEIGHBORS: &[(u8, &[u8])] = &[
    (b'q', b"wa"), (b'w', b"qesa"), (b'e', b"wrsd"), (b'r', b"etdf"),
    (b't', b"ryfg"), (b'y', b"tugh"), (b'u', b"yihj"), (b'i', b"uojk"),
    (b'o', b"ipkl"), (b'p', b"ol"), (b'a', b"qwsz"), (b's', b"wedxza"),
    (b'd', b"erfcxsz"), (b'f', b"rtgvcd"), (b'g', b"tyhbvf"), (b'h', b"yujnbg"),
    (b'j', b"uikmnh"), (b'k', b"iolmj"), (b'l', b"opk"), (b'z', b"asxd"),
    (b'x', b"zsdc"), (b'c', b"xdfv"), (b'v', b"cfgb"), (b'b', b"vghn"),
    (b'n', b"bhjm"), (b'm', b"njk"),
];

fn adjacent_keys(left: u8, right: u8) -> bool {
    QWERTY_NEIGHBORS.iter().any(|&(key, neighbors)| key == left && neighbors.contains(&right))
}

/// Classifies exactly one edit, from typed keys to corrected spelling. Eligibility
/// (in particular intentional abbreviations) remains the caller's responsibility.
pub fn single_edit_kind(typed: &str, corrected: &str) -> Option<EditKind> {
    if !typed.bytes().chain(corrected.bytes()).all(|letter| letter.is_ascii_lowercase()) {
        return None;
    }
    let (typed, corrected) = (typed.as_bytes(), corrected.as_bytes());
    let skip_one = |long: &[u8], short: &[u8]| {
        let first = long.iter().zip(short).position(|(a, b)| a != b).unwrap_or(short.len());
        long[first + 1..] == short[first..]
    };
    if typed.len() + 1 == corrected.len() {
        return skip_one(corrected, typed).then_some(EditKind::MissingLetter);
    }
    if typed.len() == corrected.len() + 1 {
        return skip_one(typed, corrected).then_some(EditKind::ExtraLetter);
    }
    if typed.len() != corrected.len() {
        return None;
    }
    let mut differences = typed.iter().zip(corrected).enumerate().filter_map(|(i, (a, b))| (a != b).then_some(i));
    let first = differences.next()?;
    match (differences.next(), differences.next()) {
        (None, None) => Some(if adjacent_keys(typed[first], corrected[first]) {
            EditKind::AdjacentKey
        } else {
            EditKind::OtherSubstitution
        }),
        (Some(second), None) if second == first + 1
            && typed[first] == corrected[second] && typed[second] == corrected[first] => Some(EditKind::Transposition),
        _ => None,
    }
}

// Priced per edit, not per kind (Claude 2026-10-28, owner records before 2026-10-19 only, so the
// frozen replay sample from 10-19 on stays out): 113 "typed, deleted within 20 s, retyped one edit
// away" pairs, with completions at the end left out because they are the owner finishing an
// abbreviation (shenm -> shenme), not a typo. Kinds: swap 24, substitution 51, missing 23, extra 15.
// P(edit) = P(kind) * (n + 0.5) / (n_kind + 0.5 * V_kind), V = 325 swaps, 650 substitutions,
// 26 missing, 26 extra; cost = -ln P(edit) * 0.7457, the scale that keeps the count-weighted mean
// over the observed pairs at the old flat 4.0, so the C3 lead margins keep their calibration.
// A frequent slip is cheap (gn/ng 3.66), an edit never seen costs more than the old 4 (about 5.5).
const OBSERVED_EDITS: &[(EditKind, u8, u8, f64)] = &[
    (EditKind::Transposition, b'g', b'n', 3.658), (EditKind::Transposition, b'h', b'z', 3.658),
    (EditKind::Transposition, b'a', b'i', 4.371), (EditKind::Transposition, b'e', b'i', 4.371),
    // substitutions: typed letter, meant letter
    (EditKind::AdjacentKey, b'z', b's', 3.743), (EditKind::AdjacentKey, b's', b'd', 3.893),
    (EditKind::OtherSubstitution, b'd', b'l', 4.080), (EditKind::OtherSubstitution, b'n', b'l', 4.331),
    (EditKind::OtherSubstitution, b'n', b'y', 4.331), (EditKind::AdjacentKey, b'w', b'z', 4.331),
    (EditKind::OtherSubstitution, b'y', b'd', 4.331), (EditKind::AdjacentKey, b'z', b'd', 4.331),
    // missing / extra: the letter left out or typed in addition
    (EditKind::MissingLetter, b'c', 0, 2.925), (EditKind::MissingLetter, b'i', 0, 2.925),
    (EditKind::MissingLetter, b'e', 0, 3.176), (EditKind::MissingLetter, b'k', 0, 3.176),
    (EditKind::MissingLetter, b'm', 0, 3.176), (EditKind::MissingLetter, b'n', 0, 3.176),
    (EditKind::MissingLetter, b'o', 0, 3.176),
    (EditKind::ExtraLetter, b'h', 0, 3.056), (EditKind::ExtraLetter, b'i', 0, 3.307),
    (EditKind::ExtraLetter, b'w', 0, 3.307),
];

/// An edit never seen in the pairs; an unseen adjacent-key substitution sits between a seen one and
/// an unseen non-adjacent one (keyboard prior, not measured).
fn unseen_cost(kind: EditKind) -> f64 {
    match kind {
        EditKind::Transposition => 5.571,
        EditKind::AdjacentKey => 5.0,
        EditKind::OtherSubstitution => 5.531,
        EditKind::MissingLetter => 4.376,
        EditKind::ExtraLetter => 4.507,
    }
}

/// Where only the kind is known (the heavy decoder counts edits without letters): the count-weighted
/// mean over that kind's observed pairs.
fn kind_cost(kind: EditKind) -> f64 {
    match kind {
        EditKind::Transposition => 4.141,
        EditKind::AdjacentKey | EditKind::OtherSubstitution => 4.441,
        EditKind::MissingLetter => 3.226,
        EditKind::ExtraLetter => 3.460,
    }
}

fn cost_for(kind: EditKind, settings: CorrectionSettings, override_cost: Option<f64>) -> f64 {
    if let Some(cost) = override_cost {
        return cost;
    }
    if !settings.enabled || !settings.typed_costs {
        return EDIT_PENALTY;
    }
    kind_cost(kind)
}

/// The edit itself, letters included: (kind, first letter, second letter). A transposition keeps its
/// two letters sorted; a substitution is (typed, meant); missing/extra carry the one letter.
pub fn single_edit(typed: &str, corrected: &str) -> Option<(EditKind, u8, u8)> {
    let kind = single_edit_kind(typed, corrected)?;
    let (t, c) = (typed.as_bytes(), corrected.as_bytes());
    let first = t.iter().zip(c).position(|(a, b)| a != b).unwrap_or(t.len().min(c.len()));
    Some(match kind {
        EditKind::Transposition => (kind, t[first].min(t[first + 1]), t[first].max(t[first + 1])),
        EditKind::AdjacentKey | EditKind::OtherSubstitution => (kind, t[first], c[first]),
        EditKind::MissingLetter => (kind, c[first], 0),
        EditKind::ExtraLetter => (kind, t[first], 0),
    })
}

/// The price of turning `typed` into `corrected` by one edit, letters included.
pub fn typed_repair_cost(typed: &str, corrected: &str) -> Option<f64> {
    let (kind, a, b) = single_edit(typed, corrected)?;
    let settings = settings();
    if let Some(cost) = edit_penalty_override() {
        return Some(cost);
    }
    if !settings.enabled || !settings.typed_costs {
        return Some(EDIT_PENALTY);
    }
    Some(
        OBSERVED_EDITS
            .iter()
            .find(|&&(k, x, y, _)| k == kind && x == a && y == b)
            .map(|&(_, _, _, cost)| cost)
            .unwrap_or_else(|| unseen_cost(kind)),
    )
}

pub fn typed_edit_cost(kind: EditKind) -> f64 {
    cost_for(kind, settings(), edit_penalty_override())
}

fn penalty_override(value: Option<&str>) -> Option<f64> {
    value?.parse::<f64>().ok().filter(|value| value.is_finite() && *value >= 0.0)
}

fn edit_penalty_override() -> Option<f64> {
    static VALUE: OnceLock<Option<f64>> = OnceLock::new();
    *VALUE.get_or_init(|| penalty_override(std::env::var("ZHIZI_EDIT_PENALTY").ok().as_deref()))
}

/// Compact decimal wire values fit the shells' existing 256-byte C3 buffer.
pub(crate) fn format_edit_units(value: f64) -> String {
    format!("{value:.6}").trim_end_matches('0').trim_end_matches('.').to_string()
}

fn overridden(name: &str, default: f64) -> f64 {
    std::env::var(name).ok().and_then(|value| value.parse().ok()).unwrap_or(default)
}

/// ZHIZI_EDIT_PENALTY overrides the constant for local evaluation only.
pub fn edit_penalty() -> f64 {
    edit_penalty_override().unwrap_or(EDIT_PENALTY)
}

/// ZHIZI_COMPOSITION_MARGIN and ZHIZI_GUESSED_COMPOSITION_MARGIN override the constants
/// for local evaluation only.
pub fn composition_margin(guessed: bool) -> f64 {
    static VALUE: OnceLock<f64> = OnceLock::new();
    static GUESSED: OnceLock<f64> = OnceLock::new();
    if guessed {
        *GUESSED.get_or_init(|| overridden("ZHIZI_GUESSED_COMPOSITION_MARGIN", GUESSED_COMPOSITION_LEAD_MARGIN))
    } else {
        *VALUE.get_or_init(|| overridden("ZHIZI_COMPOSITION_MARGIN", COMPOSITION_LEAD_MARGIN))
    }
}

/// C2: what each slot of the decoder's order is worth against the model.
pub const GROUP_RANK_PENALTY: f64 = 6.0;

/// ZHIZI_GROUP_RANK_PENALTY overrides the constant for local evaluation only.
fn group_rank_penalty() -> f64 {
    static VALUE: OnceLock<f64> = OnceLock::new();
    *VALUE.get_or_init(|| overridden("ZHIZI_GROUP_RANK_PENALTY", GROUP_RANK_PENALTY))
}

/// C1: within one reading, score - 2 x rank.
pub fn blended_order(scores: &[f64]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..scores.len()).collect();
    order.sort_by(|&left, &right| {
        let a = scores[left] - 2.0 * left as f64;
        let b = scores[right] - 2.0 * right as f64;
        b.partial_cmp(&a).unwrap_or(std::cmp::Ordering::Equal).then(left.cmp(&right))
    });
    order
}

/// C2 over the complete-coverage group (`homophone[i]`: same reading as the first).
pub fn model_first_order(scores: &[f64], homophone: &[bool]) -> Vec<usize> {
    // Never panic here (it runs behind the C ABI): mismatched input keeps the order.
    if scores.len() != homophone.len() {
        return (0..scores.len()).collect();
    }
    let mut order: Vec<usize> = (0..scores.len()).collect();
    let same_reading: Vec<usize> = (0..scores.len()).filter(|&i| homophone[i]).collect();
    if same_reading.len() >= 2 {
        let ranked = blended_order(&same_reading.iter().map(|&i| scores[i]).collect::<Vec<_>>());
        for (rank, &slot) in same_reading.iter().enumerate() {
            order[slot] = same_reading[ranked[rank]];
        }
    }
    let penalty = group_rank_penalty();
    let key = |index: usize| scores[index] - penalty * index as f64;
    // Highest key; ties go to the earlier index.
    let Some(winner) = (0..scores.len()).fold(None, |best: Option<usize>, index| match best {
        Some(current) if key(current) >= key(index) => Some(current),
        _ => Some(index),
    }) else {
        return order;
    };
    if (homophone[winner] && same_reading.len() >= 2) || winner == order[0] || key(winner) <= key(order[0]) {
        return order;
    }
    order.retain(|&index| index != winner);
    order.insert(0, winner);
    order
}

/// C2/C1 order of a group (`texts[0]` is the page's first), the owner's habits allowing.
pub fn group_order(texts: &[String], scores: &[f64], homophone: &[bool]) -> Vec<usize> {
    if texts.len() != scores.len() || scores.len() != homophone.len() || scores.is_empty() {
        return (0..scores.len()).collect();
    }
    let mut order = model_first_order(scores, homophone);
    if order[0] != 0 && owner_keeps(&texts[0], &texts[order[0]]) {
        order.retain(|&index| index != 0);
        order.insert(0, 0);
    }
    order
}

/// C3: which composition leads, if any. `texts[0]` is the decoder's first and the rest
/// the alternatives; `edits[i]` the typed cost divided by `edit_penalty()`; `guessed` when
/// the typed letters are not whole pinyin (the first is a guess too).
pub fn composition_lead(texts: &[String], scores: &[f64], edits: &[f64], guessed: bool) -> Option<usize> {
    if texts.len() < 2 || scores.len() != texts.len() || !scores.iter().all(|s| s.is_finite()) {
        return None;
    }
    let penalty = edit_penalty();
    let total = |i: usize| scores[i] - penalty * edits.get(i).copied().unwrap_or(0.0);
    // Highest total; ties go to the earlier alternative.
    let best = (1..texts.len()).fold(1, |best, index| if total(index) > total(best) { index } else { best });
    (total(best) - total(0) > composition_margin(guessed) && !owner_keeps(&texts[0], &texts[best])).then_some(best)
}

/// A whole-reading composition that does not lead is still offered in the last slot of
/// the first page when its total is within this of the decoder's first. Eval 2026-10-11
/// (both sets, best composition not on the page): first page +53 -5 external, +27 -4
/// owner (一是我想, 好用点, 装新版, 让它们 were generated and never shown); first
/// candidates unchanged.
pub const COMPOSITION_OFFER_MARGIN: f64 = 8.0;
pub const COMPOSITION_OFFER_COMMENT: &str = "整句 · 备选";

/// Which composition is offered when none leads: the best alternative (ties go to the
/// earlier one) within `COMPOSITION_OFFER_MARGIN` of the first. Same inputs as
/// `composition_lead`.
pub fn composition_offer(texts: &[String], scores: &[f64], edits: &[f64]) -> Option<usize> {
    if texts.len() < 2 || scores.len() != texts.len() || !scores.iter().all(|s| s.is_finite()) {
        return None;
    }
    let penalty = edit_penalty();
    let total = |i: usize| scores[i] - penalty * edits.get(i).copied().unwrap_or(0.0);
    let best = (1..texts.len()).fold(1, |best, index| if total(index) > total(best) { index } else { best });
    (total(best) >= total(0) - COMPOSITION_OFFER_MARGIN).then_some(best)
}

/// Pairs the owner settled on in their own records: `first` keeps the first slot
/// against `challenger`.
#[derive(Default)]
pub struct OwnerStyle {
    protected: HashSet<(String, String)>,
    /// A bare initial and the character the owner settled on for it (l 了 312 of 316).
    letters: HashMap<String, String>,
}

impl OwnerStyle {
    pub const MINIMUM_CHOICES: usize = 10;
    pub const MINIMUM_SHARE: f64 = 0.93;
    /// A bare initial is settled at 90 percent: one pick of 捞 for l must not outrank
    /// 了, but 做/在/再 for z (64:48:29) stay with the decoder and the model.
    pub const MINIMUM_LETTER_SHARE: f64 = 0.9;

    /// `records`: quality records, one JSON object per line. Same-length pairs on the
    /// first page count (做 over 作 89:1, 的 over 地 147:0); pairs the owner splits by
    /// context (嘛/吗 27:31, 在/再 72:48) stay with the model, and so does a silent typo
    /// the owner kept committing (不时 over 不是 24:3, below the share).
    pub fn from_records(records: &str) -> Self {
        let mut chosen: HashMap<(String, String), usize> = HashMap::new();
        let mut letters: HashMap<String, HashMap<String, usize>> = HashMap::new();
        for line in records.lines() {
            let Ok(object) = serde_json::from_str::<serde_json::Value>(line) else { continue };
            let Some(selected) = object["selectedText"].as_str().filter(|text| !text.is_empty()) else { continue };
            // A redone commit is no habit; a reading picked in parts lists a whole sentence
            // the owner never saw on the page.
            if object["redoAfterCommit"].as_bool() == Some(true) || object.get("segments").is_some_and(|v| !v.is_null()) {
                continue;
            }
            if let Some(letter) = object["pinyin"].as_str().filter(|p| p.len() == 1 && p.bytes().all(|b| b.is_ascii_lowercase()))
            {
                let committed = object["committedText"].as_str().unwrap_or(selected);
                if committed.chars().count() == 1 && committed.chars().all(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)) {
                    *letters.entry(letter.to_string()).or_default().entry(committed.to_string()).or_default() += 1;
                }
            }
            let Some(candidates) = object["candidates"].as_array() else { continue };
            let page: Vec<&str> = candidates.iter().take(5).filter_map(|value| value.as_str()).collect();
            if !page.contains(&selected) {
                continue;
            }
            let length = selected.chars().count();
            for other in page {
                if other != selected && other.chars().count() == length {
                    *chosen.entry((selected.to_string(), other.to_string())).or_default() += 1;
                }
            }
        }
        let mut protected = HashSet::new();
        for ((first, other), &count) in &chosen {
            if count < Self::MINIMUM_CHOICES {
                continue;
            }
            let reverse = chosen.get(&(other.clone(), first.clone())).copied().unwrap_or(0);
            if count as f64 / (count + reverse) as f64 >= Self::MINIMUM_SHARE {
                protected.insert((first.clone(), other.clone()));
            }
        }
        let letters = letters
            .into_iter()
            .filter_map(|(letter, counts)| {
                let total: usize = counts.values().sum();
                let (text, count) = counts.into_iter().max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0)))?;
                (count >= Self::MINIMUM_CHOICES && count as f64 / total as f64 >= Self::MINIMUM_LETTER_SHARE)
                    .then_some((letter, text))
            })
            .collect();
        Self { protected, letters }
    }

    /// Counts come from the same confirmed choices as candidate promotions.
    /// Alternatives are the intersection actually seen for this choice; absent
    /// alternatives never manufacture pairwise preference evidence.
    pub fn from_learning(choices: &[crate::zhixu::learning::Choice]) -> Self {
        let mut chosen: HashMap<(String, String), u64> = HashMap::new();
        let mut letters: HashMap<String, HashMap<String, u64>> = HashMap::new();
        for choice in choices {
            let count = u64::from(choice.count);
            let length = choice.text.chars().count();
            if choice.raw.len() == 1 && choice.raw.bytes().all(|b| b.is_ascii_lowercase())
                && length == 1 && choice.text.chars().all(|c| ('\u{4e00}'..='\u{9fff}').contains(&c))
            {
                let total = letters.entry(choice.raw.clone()).or_default().entry(choice.text.clone()).or_default();
                *total = total.saturating_add(count);
            }
            let mut seen = HashSet::new();
            for other in &choice.alternatives {
                if other != &choice.text && other.chars().count() == length && seen.insert(other) {
                    let total = chosen.entry((choice.text.clone(), other.clone())).or_default();
                    *total = total.saturating_add(count);
                }
            }
        }
        let protected = chosen.iter().filter_map(|((first, other), &count)| {
            let reverse = chosen.get(&(other.clone(), first.clone())).copied().unwrap_or(0);
            (count >= Self::MINIMUM_CHOICES as u64
                && count as f64 / count.saturating_add(reverse) as f64 >= Self::MINIMUM_SHARE)
                .then(|| (first.clone(), other.clone()))
        }).collect();
        let letters = letters.into_iter().filter_map(|(letter, counts)| {
            let total = counts.values().fold(0u64, |total, count| total.saturating_add(*count));
            let (text, count) = counts.into_iter().max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0)))?;
            (count >= Self::MINIMUM_CHOICES as u64 && count as f64 / total as f64 >= Self::MINIMUM_LETTER_SHARE)
                .then_some((letter, text))
        }).collect();
        Self { protected, letters }
    }

    pub fn keeps(&self, first: &str, challenger: &str) -> bool {
        self.protected.contains(&(first.to_string(), challenger.to_string()))
    }

    pub fn letter(&self, letter: &str) -> Option<&str> {
        self.letters.get(letter).map(String::as_str)
    }
}

/// The character the owner settled on for a bare initial, if any.
pub fn owner_letter(letter: &str) -> Option<String> {
    owner_style().read().ok().and_then(|style| style.letter(letter).map(str::to_string))
        .or_else(|| LEARNING_STYLE.with(|slot| slot.borrow().as_ref().and_then(|style| style.letter(letter).map(str::to_string))))
}

fn owner_style() -> &'static RwLock<OwnerStyle> {
    static STYLE: OnceLock<RwLock<OwnerStyle>> = OnceLock::new();
    STYLE.get_or_init(|| RwLock::new(OwnerStyle::default()))
}

// The shell reads ranking on the same thread as the session snapshot. Setting
// None on a Rime snapshot restores the legacy rules without changing its seed.
thread_local! { static LEARNING_STYLE: RefCell<Option<Arc<OwnerStyle>>> = const { RefCell::new(None) }; }

pub(crate) fn set_learning_style(style: Option<Arc<OwnerStyle>>) {
    LEARNING_STYLE.with(|slot| { *slot.borrow_mut() = style; });
}

/// The process-wide table, built by the host from the owner's local records once.
pub fn load_owner_style(records: &str) {
    let style = OwnerStyle::from_records(records);
    if let Ok(mut current) = owner_style().write() {
        *current = style;
    }
}

/// The candidate a deliberate pick made three times has put first on the page now showing (personal picks in the mixer): the model
/// does not take the first place from it, as it does not from a settled habit.
fn confirmed_first() -> &'static RwLock<Option<String>> {
    static FIRST: OnceLock<RwLock<Option<String>>> = OnceLock::new();
    FIRST.get_or_init(|| RwLock::new(None))
}

pub(crate) fn set_confirmed_first(text: Option<String>) {
    if let Ok(mut current) = confirmed_first().write() {
        *current = text;
    }
}

pub fn owner_keeps(first: &str, challenger: &str) -> bool {
    let (seed_keeps, seed_reverse) = owner_style().read()
        .map(|style| (style.keeps(first, challenger), style.keeps(challenger, first)))
        .unwrap_or((false, false));
    seed_keeps
        || (!seed_reverse && LEARNING_STYLE.with(|slot| slot.borrow().as_ref()
            .is_some_and(|style| style.keeps(first, challenger))))
        || confirmed_first().read().map(|text| text.as_deref() == Some(first)).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn correction_costs_price_the_edit_and_keep_the_old_mean() {
        // the owner's frequent slips are cheaper than the old flat 4, an edit never seen is dearer
        let cost = |typed: &str, meant: &str| typed_repair_cost(typed, meant).unwrap();
        assert_eq!(cost("yijign", "yijing"), 3.658, "ng typed as gn");
        assert_eq!(cost("hzong", "zhong"), 3.658);
        assert_eq!(cost("zi", "si"), 3.743, "z for s: adjacent and observed");
        assert_eq!(cost("pi", "bi"), 5.531, "unseen non-adjacent substitution");
        assert_eq!(cost("ji", "ki"), 5.0, "unseen adjacent substitution");
        // an extra h is a common slip of this owner (zh/z), so shi -> si is cheap on its own; 已经死了
        // loses to 已经是了 because it needs the swap and the dropped h, two edits against one
        assert!(cost("yijign", "yijing") < cost("yijign", "yijing") + cost("shi", "si"));
        assert_eq!(cost("shi", "si"), 3.056);
        // the kind-only prices are the count-weighted means of each kind, and their overall mean is 4
        let (kinds, counts) = ([EditKind::Transposition, EditKind::OtherSubstitution, EditKind::MissingLetter, EditKind::ExtraLetter], [24.0, 51.0, 23.0, 15.0]);
        let mean = kinds.iter().zip(counts).map(|(kind, n)| cost_for(*kind, CorrectionSettings::default(), None) * n).sum::<f64>() / 113.0;
        assert!((mean - 4.0).abs() < 0.01, "{mean}");
        let flat = correction_settings_from(|name| (name == "ZHIZI_EDIT_COSTS").then(|| "flat".into()));
        for kind in kinds {
            assert_eq!(cost_for(kind, flat, None), 4.0);
            assert_eq!(cost_for(kind, CorrectionSettings::default(), Some(7.0)), 7.0);
            assert_eq!(cost_for(kind, CorrectionSettings { enabled: false, ..Default::default() }, None), 4.0);
        }
    }

    #[test]
    fn single_edit_classification_distinguishes_keys_swaps_and_direction() {
        for (typed, corrected, expected) in [
            ("jign", "jing", EditKind::Transposition),
            ("hzong", "zhong", EditKind::Transposition),
            ("z", "s", EditKind::AdjacentKey),
            ("z", "d", EditKind::AdjacentKey),
            ("d", "s", EditKind::AdjacentKey),
            ("shenme", "shenmei", EditKind::MissingLetter),
            ("houmioan", "houmian", EditKind::ExtraLetter),
            ("n", "l", EditKind::OtherSubstitution),
        ] {
            assert_eq!(single_edit_kind(typed, corrected), Some(expected));
        }
        assert_eq!(single_edit_kind("jing", "jing"), None);
        assert_eq!(single_edit_kind("abcd", "dcba"), None);
        assert_eq!(single_edit_kind("今", "京"), None);
        for &(key, neighbors) in QWERTY_NEIGHBORS {
            for &neighbor in neighbors {
                assert!(adjacent_keys(neighbor, key), "adjacency must be symmetric");
                assert!(!adjacent_keys(key, key));
            }
        }
    }

    #[test]
    fn correction_switches_are_independent_and_test_overrides_restore() {
        for name in ["ZHIZI_CORRECTIONS", "ZHIZI_CORRECTION_READINGS", "ZHIZI_TRAILING_LETTER"] {
            let config = correction_settings_from(|key| (key == name).then(|| "0".into()));
            assert_eq!(config.enabled, name != "ZHIZI_CORRECTIONS");
            assert!(config.typed_costs);
            assert_eq!(config.multiple_readings, name != "ZHIZI_CORRECTION_READINGS");
            assert_eq!(config.trailing_letter, name != "ZHIZI_TRAILING_LETTER");
        }
        let original = settings();
        let off = CorrectionSettings { enabled: false, ..Default::default() };
        with_correction_settings(off, || {
            assert_eq!(settings(), off);
            let _ = std::panic::catch_unwind(|| with_correction_settings(CorrectionSettings::default(), || {
                assert_eq!(settings(), CorrectionSettings::default());
                panic!("exercise restoration");
            }));
            assert_eq!(settings(), off);
        });
        assert_eq!(settings(), original);
        assert_eq!(penalty_override(Some("7.0")), Some(7.0));
        for invalid in ["", "invalid", "NaN", "inf", "-1"] {
            assert_eq!(penalty_override(Some(invalid)), None);
        }
    }

    fn texts(values: &[&str]) -> Vec<String> {
        values.iter().map(|v| v.to_string()).collect()
    }

    #[test]
    fn the_best_composition_is_offered_when_close_enough() {
        let texts: Vec<String> = ["打字办", "大字版", "打字版"].iter().map(|t| t.to_string()).collect();
        assert_eq!(composition_offer(&texts, &[-20.0, -24.0, -30.0], &[0.0, 0.0, 0.0]), Some(1));
        assert_eq!(composition_offer(&texts, &[-20.0, -29.0, -30.0], &[0.0, 0.0, 0.0]), None, "more than 8 behind");
        assert_eq!(composition_offer(&texts, &[-20.0, -22.0, -21.0], &[0.0, 1.0, 0.0]), Some(2), "edits count");
        assert_eq!(composition_offer(&texts[..1], &[-20.0], &[0.0]), None);
    }

    #[test]
    fn blended_keeps_rank_unless_two_points_per_slot_are_beaten() {
        assert_eq!(blended_order(&[-5.0, -4.0]), vec![0, 1]);
        assert_eq!(blended_order(&[-5.0, -2.5]), vec![1, 0]);
        assert_eq!(blended_order(&[0.0, 2.0, 4.0]), vec![0, 1, 2]);
    }

    #[test]
    fn another_reading_leads_only_past_six_points_per_slot() {
        // slot 2 (different reading) must beat the first by more than 12.
        assert_eq!(model_first_order(&[-20.0, -30.0, -9.0], &[true, false, false]), vec![0, 1, 2]);
        assert_eq!(model_first_order(&[-20.0, -30.0, -7.0], &[true, false, false]), vec![2, 0, 1]);
        // Homophones swap among themselves by the C1 rule.
        assert_eq!(model_first_order(&[-10.0, -5.0, -40.0], &[true, true, false]), vec![1, 0, 2]);
        let none = [false, false, false];
        assert_eq!(model_first_order(&[-12.0, -5.0, -14.0], &none), vec![1, 0, 2]);
        assert_eq!(model_first_order(&[-8.0, -3.0, -14.0], &none), vec![0, 1, 2]);
        assert_eq!(model_first_order(&[-11.0, -2.0, -12.0], &[true, true, true]), vec![1, 0, 2]);
        assert_eq!(model_first_order(&[-20.0, -21.0, -1.0], &[true, true, false]), vec![2, 0, 1]);
    }

    #[test]
    fn empty_or_mismatched_groups_keep_their_order() {
        assert_eq!(group_order(&[], &[], &[]), Vec::<usize>::new());
        assert_eq!(group_order(&texts(&["一", "二"]), &[0.0, 1.0], &[true]), vec![0, 1]);
        assert_eq!(model_first_order(&[0.0, 9.0], &[true]), vec![0, 1]);
    }

    #[test]
    fn composition_leads_past_the_margin_after_the_edit_penalty() {
        let page = texts(&["台服砸了", "太复杂了", "太傅砸了"]);
        assert_eq!(composition_lead(&page, &[-30.0, -21.0, -40.0], &[0.0, 0.0, 0.0], false), Some(1));
        assert_eq!(composition_lead(&page, &[-30.0, -23.0, -40.0], &[0.0, 0.0, 0.0], false), None);
        // One changed letter costs 4: a 10-point win becomes 6.
        assert_eq!(composition_lead(&page, &[-30.0, -20.0, -40.0], &[0.0, 1.0, 0.0], false), None);
        assert_eq!(composition_lead(&page, &[-30.0, f64::NAN, -40.0], &[0.0, 0.0, 0.0], false), None);
        // A first guessed from abbreviations needs only 6.
        assert_eq!(composition_lead(&page, &[-30.0, -23.0, -40.0], &[0.0, 0.0, 0.0], true), Some(1));
    }

    #[test]
    fn owner_style_needs_ten_choices_and_ninety_three_percent() {
        let mut records = String::new();
        for _ in 0..10 {
            records.push_str(r#"{"selectedText":"做","candidates":["作","做"]}"#);
            records.push('\n');
        }
        let style = OwnerStyle::from_records(&records);
        assert!(style.keeps("做", "作"));
        assert!(!style.keeps("作", "做"));
        records.push_str(r#"{"selectedText":"作","candidates":["作","做"]}"#);
        assert!(!OwnerStyle::from_records(&records).keeps("做", "作"));
    }

    fn learned_choice(raw: &str, reading: &str, text: &str, count: u32, alternatives: &[&str]) -> crate::zhixu::learning::Choice {
        crate::zhixu::learning::Choice {
            app: "fixture".into(), raw: raw.into(), reading: reading.into(), text: text.into(),
            target: 0, count, last_ms: 1,
            alternatives: alternatives.iter().map(|text| text.to_string()).collect(),
        }
    }

    #[test]
    fn learned_owner_style_uses_weighted_choices_without_relaxing_thresholds() {
        let chosen = learned_choice("zuo", "zuo", "做", 10, &["作"]);
        assert!(OwnerStyle::from_learning(std::slice::from_ref(&chosen)).keeps("做", "作"));
        let reverse = learned_choice("zuo", "zuo", "作", 1, &["做"]);
        assert!(!OwnerStyle::from_learning(&[chosen, reverse]).keeps("做", "作"));
        assert!(!OwnerStyle::from_learning(&[learned_choice("zuo", "zuo", "做", 9, &["作"])]).keeps("做", "作"));
        assert!(!OwnerStyle::from_learning(&[learned_choice("zuo", "zuo", "做", 5, &["作", "作"])]).keeps("做", "作"));
        assert!(!OwnerStyle::from_learning(&[learned_choice("zuo", "zuo", "做", 100, &[])]).keeps("做", "作"));
    }

    #[test]
    fn learned_owner_style_full_syllable_picks_do_not_change_a_bare_initial() {
        let choices = [
            learned_choice("d", "de", "的", 10, &["地"]),
            learned_choice("dao", "dao", "到", 2, &["道"]),
        ];
        let style = OwnerStyle::from_learning(&choices);
        assert_eq!(style.letter("d"), Some("的"));
        assert!(style.keeps("的", "地"));
        assert_eq!(OwnerStyle::from_learning(&choices[1..]).letter("d"), None);
        let records = [r#"{"pinyin":"d","selectedText":"的","candidates":["的","地"]}"#; 10].join("\n");
        assert_eq!(OwnerStyle::from_records(&records).letter("d"), Some("的"));
    }

    #[test]
    fn a_bare_initial_settles_at_ninety_percent_of_ten_choices() {
        let mut records = String::new();
        for (text, times) in [("了", 18), ("捞", 2), ("做", 6), ("在", 5)] {
            let letter = if text == "了" || text == "捞" { "l" } else { "z" };
            for _ in 0..times {
                records.push_str(&format!(r#"{{"pinyin":"{letter}","selectedText":"{text}","candidates":[]}}"#));
                records.push('\n');
            }
        }
        let style = OwnerStyle::from_records(&records);
        assert_eq!(style.letter("l"), Some("了"));
        assert_eq!(style.letter("z"), None, "split by context: the model decides");
    }
}
