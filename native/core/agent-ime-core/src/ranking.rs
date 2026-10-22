//! How 顺手·知字's scores change the page, shared by every host (macOS calls these
//! through the C ABI, the Rust host directly), so the two platforms cannot drift:
//!
//! - C3: a whole-reading composition leads when P(text | context) x P(keys | text), the
//!   model's log probability minus `EDIT_PENALTY` per letter changed to reach it, beats
//!   the decoder's first by more than `COMPOSITION_LEAD_MARGIN` (taifuzale -> 太复杂了).
//! - C2: among candidates that cover the whole input, another reading or length leads
//!   when its score - 6 x rank beats the first; C1: homophones of the same reading are
//!   reordered by score - 2 x rank among their own slots.
//! - The owner's settled habits (a pair chosen at least 10 times at 93 percent or more
//!   in their own records) keep their first candidate in both.
//!
//! The hosts keep only what is theirs: running the model, its time budget, and the
//! preceding text they can see.

use std::collections::{HashMap, HashSet};
use std::sync::{OnceLock, RwLock};

/// Owner records: above 8 the lead replaced silent garbage (你系按看下 -> 你先看下)
/// about nine times in ten; between 4 and 8 it misread intent more often.
pub const COMPOSITION_LEAD_MARGIN: f64 = 8.0;
/// When the typed letters do not spell whole pinyin (zhedifangy, pinlvdt, nicshiyong)
/// the decoder's first is itself a guess at abbreviations or a slip, so less is needed to
/// replace it. External set 2026-10-11 at 6: 58.4% -> 59.8% first, the owner's October
/// set 89.7% -> 89.9% (你测试用, 都点了, 能发了); at 4 full-pinyin words broke (制定).
pub const GUESSED_COMPOSITION_LEAD_MARGIN: f64 = 6.0;
/// Log-probability cost of one letter changed to reach a candidate (P(keys | text)).
/// Owner replay 2026-10-10: at 4, one-slip repairs led with 装新版 over the heavy-garble
/// 换个新版, and 真没想到, 更不用说, 清单列出来 over garbage.
pub const EDIT_PENALTY: f64 = 4.0;
pub const COMPOSITION_COMMENT: &str = "整句 · 顺手·知字";
pub const HEAVY_GARBLE_COMMENT: &str = "重度乱拼 · 本地索引";

fn overridden(name: &str, default: f64) -> f64 {
    std::env::var(name).ok().and_then(|value| value.parse().ok()).unwrap_or(default)
}

/// ZHIZI_EDIT_PENALTY overrides the constant for local evaluation only.
pub fn edit_penalty() -> f64 {
    static VALUE: OnceLock<f64> = OnceLock::new();
    *VALUE.get_or_init(|| overridden("ZHIZI_EDIT_PENALTY", EDIT_PENALTY))
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
/// the alternatives; `edits[i]` the letters changed to reach `texts[i]`; `guessed` when
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
}

fn owner_style() -> &'static RwLock<OwnerStyle> {
    static STYLE: OnceLock<RwLock<OwnerStyle>> = OnceLock::new();
    STYLE.get_or_init(|| RwLock::new(OwnerStyle::default()))
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
    owner_style().read().map(|style| style.keeps(first, challenger)).unwrap_or(false)
        || confirmed_first().read().map(|text| text.as_deref() == Some(first)).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

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
