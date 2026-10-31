//! Fuzzy-pinyin spelling variants for the Zhixu decoder.
//!
//! Rime implements the eight groups as `xform`/`derive` rules over the schema's
//! spelling algebra. Here a group is a symmetric alternative applied to one
//! syllable token: `z` also reads as `zh` and `zh` as `z`, `an` as `ang` and
//! back. Initial groups (bits 0-4) and final groups (bits 5-7) are independent,
//! so a token has at most four spellings. Only alternatives that are themselves
//! valid syllables (or valid initials for an abbreviated token) are kept.

use super::decoder::{canonical_syllable, full_syllable, initial};

pub(crate) const FUZZY_MASK_ALL: u32 = 0xff;

thread_local! {
    static MASK: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// The groups enabled for the generation running on this thread.
pub(crate) fn mask() -> u32 { MASK.with(std::cell::Cell::get) }

pub(crate) struct MaskGuard(u32);

impl Drop for MaskGuard {
    fn drop(&mut self) { MASK.with(|mask| mask.set(self.0)); }
}

/// Enables `mask` for generation on this thread until the guard drops, like
/// the dictionary and learning snapshots pinned around the same calls.
pub(crate) fn pin_mask(mask: u32) -> MaskGuard {
    MaskGuard(MASK.with(|current| current.replace(mask & FUZZY_MASK_ALL)))
}

const INITIAL_PAIRS: [(u32, &str, &str); 5] = [
    (1 << 0, "z", "zh"),
    (1 << 1, "c", "ch"),
    (1 << 2, "s", "sh"),
    (1 << 3, "n", "l"),
    (1 << 4, "f", "h"),
];
const FINAL_PAIRS: [(u32, &str, &str); 3] = [
    (1 << 5, "an", "ang"),
    (1 << 6, "en", "eng"),
    (1 << 7, "in", "ing"),
];

/// The other spelling of the token's initial under the enabled groups, if any.
fn initial_alternate(token: &str, mask: u32) -> Option<String> {
    for &(bit, short, long) in &INITIAL_PAIRS {
        if mask & bit == 0 { continue; }
        // `zh` must be tried before `z`: both prefix the same token.
        if short.len() == 1 && long.len() == 2 {
            if let Some(rest) = token.strip_prefix(long) { return Some(format!("{short}{rest}")); }
            if let Some(rest) = token.strip_prefix(short) { return Some(format!("{long}{rest}")); }
        } else {
            if let Some(rest) = token.strip_prefix(short) { return Some(format!("{long}{rest}")); }
            if let Some(rest) = token.strip_prefix(long) { return Some(format!("{short}{rest}")); }
        }
    }
    None
}

/// The other spelling of the token's final under the enabled groups, if any.
fn final_alternate(token: &str, mask: u32) -> Option<String> {
    for &(bit, short, long) in &FINAL_PAIRS {
        if mask & bit == 0 { continue; }
        if let Some(stem) = token.strip_suffix(long) { return Some(format!("{stem}{short}")); }
        if let Some(stem) = token.strip_suffix(short) { return Some(format!("{stem}{long}")); }
    }
    None
}

/// Every spelling of `token` under `mask`, the token itself first. Variants that
/// are neither a full syllable nor (for an initial token) an initial are dropped.
pub(crate) fn token_variants(token: &str, mask: u32) -> Vec<String> {
    let mut variants = vec![token.to_string()];
    if mask & FUZZY_MASK_ALL == 0 { return variants; }
    let is_initial = initial(token) && !full_syllable(token);
    let valid = |spelling: &str| if is_initial { initial(spelling) } else { full_syllable(spelling) };
    let initials = initial_alternate(token, mask).filter(|spelling| valid(spelling));
    let finals = if is_initial { None } else { final_alternate(token, mask) };
    if let Some(spelling) = initials.as_ref() { variants.push(spelling.clone()); }
    if let Some(spelling) = finals.as_ref().filter(|spelling| valid(spelling)) {
        variants.push(spelling.clone());
        if let Some(both) = initial_alternate(spelling, mask).filter(|both| valid(both)) { variants.push(both); }
    }
    variants.dedup();
    variants
}

/// Whether `raw` (apostrophes mark required boundaries) spells the spaced
/// `reading` one syllable per token under `mask`, each token being the
/// syllable itself or one of its fuzzy spellings.
pub(crate) fn spelling_matches(raw: &str, reading: &str, mask: u32) -> bool {
    let syllables: Vec<_> = reading.split_whitespace().map(canonical_syllable).collect();
    let bytes = raw.as_bytes();
    // reachable[k] holds the raw positions after k matched syllables.
    let mut reachable = vec![false; raw.len() + 1];
    reachable[0] = true;
    for syllable in &syllables {
        let mut next = vec![false; raw.len() + 1];
        for start in (0..=raw.len()).filter(|&start| reachable[start]) {
            let start = (start..raw.len()).find(|&at| bytes[at] != b'\'').unwrap_or(raw.len());
            for end in start + 1..=raw.len().min(start + 6) {
                let token = &raw[start..end];
                if token.contains('\'') { break; }
                if token_variants(token, mask).iter().any(|spelling| canonical_syllable(spelling) == *syllable) {
                    next[end] = true;
                }
            }
        }
        reachable = next;
    }
    let tail = (0..=raw.len()).rev().take_while(|&at| at == raw.len() || bytes[at] == b'\'').collect::<Vec<_>>();
    tail.iter().any(|&at| reachable[at])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn variants(token: &str, mask: u32) -> Vec<String> {
        let mut found = token_variants(token, mask);
        let first = found.remove(0);
        assert_eq!(first, token, "the unchanged spelling is always first");
        found.sort();
        found
    }

    #[test]
    fn mask_zero_changes_nothing() {
        assert_eq!(token_variants("zhang", 0), vec!["zhang"]);
    }

    #[test]
    fn each_initial_group_is_symmetric() {
        assert_eq!(variants("zi", 1 << 0), vec!["zhi"]);
        assert_eq!(variants("zhi", 1 << 0), vec!["zi"]);
        assert_eq!(variants("ci", 1 << 1), vec!["chi"]);
        assert_eq!(variants("chi", 1 << 1), vec!["ci"]);
        assert_eq!(variants("si", 1 << 2), vec!["shi"]);
        assert_eq!(variants("shi", 1 << 2), vec!["si"]);
        assert_eq!(variants("na", 1 << 3), vec!["la"]);
        assert_eq!(variants("la", 1 << 3), vec!["na"]);
        assert_eq!(variants("fa", 1 << 4), vec!["ha"]);
        assert_eq!(variants("ha", 1 << 4), vec!["fa"]);
    }

    #[test]
    fn each_final_group_is_symmetric() {
        assert_eq!(variants("lan", 1 << 5), vec!["lang"]);
        assert_eq!(variants("lang", 1 << 5), vec!["lan"]);
        assert_eq!(variants("ben", 1 << 6), vec!["beng"]);
        assert_eq!(variants("beng", 1 << 6), vec!["ben"]);
        assert_eq!(variants("bin", 1 << 7), vec!["bing"]);
        assert_eq!(variants("bing", 1 << 7), vec!["bin"]);
    }

    #[test]
    fn initial_and_final_combine_independently() {
        assert_eq!(variants("zhang", (1 << 0) | (1 << 5)), vec!["zan", "zang", "zhan"]);
    }

    #[test]
    fn alternates_that_are_not_syllables_are_dropped() {
        // yuang, yeng and shing are not syllables; an/ang must not invent them.
        assert!(variants("yuan", 1 << 5).is_empty());
        assert!(variants("yan", 1 << 5).contains(&"yang".to_string()));
        assert!(variants("xin", 1 << 7).contains(&"xing".to_string()));
        assert!(!variants("zhi", 0xff).contains(&"zhing".to_string()));
    }

    #[test]
    fn abbreviated_initial_tokens_follow_the_initial_groups_only() {
        assert_eq!(variants("z", 1 << 0), vec!["zh"]);
        assert_eq!(variants("zh", 1 << 0), vec!["z"]);
        assert_eq!(variants("n", 1 << 3), vec!["l"]);
        assert!(variants("z", 1 << 5).is_empty());
    }

    #[test]
    fn spelling_matches_checks_each_syllable_under_the_mask() {
        assert!(spelling_matches("zongguo", "zhong guo", 1 << 0));
        assert!(!spelling_matches("zongguo", "zhong guo", 0));
        assert!(spelling_matches("zhongguo", "zhong guo", 0), "exact spellings always match");
        assert!(spelling_matches("zeyan", "zhe yang", (1 << 0) | (1 << 5)));
        assert!(!spelling_matches("zeyan", "zhe yang", 1 << 0), "an/ang is not enabled");
        assert!(spelling_matches("lihao", "ni hao", 1 << 3));
        assert!(!spelling_matches("lihao", "ni", 1 << 3), "every letter is consumed");
        assert!(spelling_matches("zong'guo", "zhong guo", 1 << 0));
        assert!(!spelling_matches("zon'gguo", "zhong guo", 1 << 0), "apostrophes stay boundaries");
    }
}
