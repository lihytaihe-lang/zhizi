//! Bounded dictionary segment expansions for the same immutable Zhiju snapshot.
//! These candidates have reading proofs, but no neural score or promotion order.

use super::super::{Index, decoder};
use std::collections::{HashSet, VecDeque};
use std::time::{Duration, Instant};

const MAX_SYLLABLES: usize = 8;
const MAX_PATHS: usize = 12;
const MAX_PREFIX_WORDS: usize = 2;
const MAX_SUFFIX_WORDS: usize = 6;
const MAX_RESULTS: usize = 12;

#[derive(Clone)]
pub(crate) struct LexicalCandidate {
    pub text: String,
    pub reading: String,
    pub consumed: usize,
}

struct Word<'a> {
    text: &'a str,
    reading: &'a str,
    weight: u32,
}

fn words<'a>(index: &'a Index, tokens: &[&str], limit: usize, within_budget: &impl Fn() -> bool)
    -> Option<Vec<Word<'a>>> {
    let initials = index.complete_initials.get()?;
    let key: String = tokens.iter().map(|token| token.as_bytes()[0] as char).collect();
    let mut matches = Vec::new();
    for &id in initials.get(&key).into_iter().flatten() {
        if !within_budget() { return None; }
        let word = &index.entries[id as usize];
        let text = index.texts.get(word.text);
        let reading = index.readings.get(word.reading);
        if word.weight == 0 || text.chars().count() != tokens.len()
            || reading.split_whitespace().count() != tokens.len()
            || !text.chars().all(|ch| ch == '〇' || ('\u{3400}'..='\u{9fff}').contains(&ch))
            || !reading.split_whitespace().zip(tokens).all(|(syllable, token)|
                decoder::full_syllable(syllable) && decoder::syllable_matches(token, syllable)) {
            continue;
        }
        matches.push(Word { text, reading, weight: word.weight });
    }
    matches.sort_unstable_by(|a, b| b.weight.cmp(&a.weight).then(a.text.cmp(b.text)).then(a.reading.cmp(b.reading)));
    let mut seen = HashSet::new();
    matches.retain(|word| seen.insert(word.text));
    matches.truncate(limit);
    Some(matches)
}

pub(crate) fn candidates(index: &Index, raw: &str, budget: Duration) -> Option<Vec<LexicalCandidate>> {
    let started = Instant::now();
    let within_budget = || started.elapsed() < budget;
    if !within_budget() || index.complete_initials.get().is_none() { return None; }
    if raw.len() > MAX_SYLLABLES * 6 { return Some(Vec::new()); }
    let (edges, _) = decoder::spelling_edges(raw, true);
    let mut pending = VecDeque::from([(0, Vec::new())]);
    let mut paths = Vec::new();
    while let Some((position, tokens)) = pending.pop_front() {
        if !within_budget() { return None; }
        if position == raw.len() {
            if tokens.len() >= 4 { paths.push(tokens); }
            if paths.len() == MAX_PATHS { break; }
            continue;
        }
        if tokens.len() == MAX_SYLLABLES { continue; }
        for &(end, next) in edges[position].iter().rev() {
            let mut extended = tokens.clone();
            extended.push(&raw[position..end]);
            pending.push_back((next, extended));
        }
    }

    let mut complete = Vec::new();
    let mut partitions = Vec::new();
    for path in paths {
        for word in words(index, &path, MAX_PREFIX_WORDS, &within_budget)? {
            complete.push(LexicalCandidate {
                text: word.text.into(), reading: word.reading.into(), consumed: raw.len(),
            });
        }
        // Every retained boundary has two real dictionary words. No sound match
        // can bridge a partial word or spend any key byte more than once.
        for split in 2..=path.len()-2 {
            let prefix = words(index, &path[..split], MAX_PREFIX_WORDS, &within_budget)?;
            if prefix.is_empty() { continue; }
            let suffix = words(index, &path[split..], MAX_SUFFIX_WORDS, &within_budget)?;
            if !suffix.is_empty() { partitions.push((prefix, suffix)); }
        }
    }
    let mut seen = HashSet::new();
    let mut complete: Vec<_> = complete.into_iter()
        .filter(|word| seen.insert(word.text.clone())).take(MAX_PREFIX_WORDS).collect();
    // Rotate across boundaries and across distinct leading words before moving
    // down the suffix ranking. A common first word cannot use every slot.
    'results: for tail_index in 0..MAX_SUFFIX_WORDS {
        for (prefix, suffix) in &partitions {
            if !within_budget() { return None; }
            let Some(tail) = suffix.get(tail_index) else { continue; };
            for head in prefix {
                let text = format!("{}{}", head.text, tail.text);
                if seen.insert(text.clone()) {
                    complete.push(LexicalCandidate {
                        text, reading: format!("{} {}", head.reading, tail.reading), consumed: raw.len(),
                    });
                    if complete.len() == MAX_RESULTS { break 'results; }
                }
            }
        }
    }
    complete.retain(|word| decoder::candidate_coverage(raw, &crate::state::Candidate {
        text: word.text.clone(), reading_key: word.reading.clone(),
        comment: String::new(), source_index: None,
    }) == Some(raw.len()));
    within_budget().then_some(complete)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zhixu::loader::{DictionaryData, Entry};

    #[test]
    fn lexical_coverage_keeps_alternative_words_without_weakening_mixed_spelling() {
        let dictionary = Index::from_data(DictionaryData { entries: [
            ("今天", "jin tian", 8000), ("集团", "ji tuan", 3000),
            ("休息", "xiu xi", 800), ("学习", "xue xi", 400),
            ("相信", "xiang xin", 200), ("修改", "xiu gai", 4000),
            ("坏数据", "huai shu", 9000),
        ].into_iter().map(|(text, reading, weight)| Entry {
            text: text.into(), reading: reading.into(), weight, layer: "fixture".into(),
        }).collect() });
        let budget = Duration::from_secs(1);
        assert!(candidates(&dictionary, "jtxx", budget).is_none(), "skip cold initials index");
        dictionary.complete_initials();
        assert!(candidates(&dictionary, "jtxx", Duration::ZERO).is_none());
        let initials = candidates(&dictionary, "jtxx", budget).unwrap();
        let texts: Vec<_> = initials.iter().map(|candidate| candidate.text.as_str()).collect();
        assert_eq!(texts, ["今天休息", "集团休息", "今天学习", "集团学习", "今天相信", "集团相信"]);
        let mixed = candidates(&dictionary, "jintianxx", budget).unwrap();
        assert_eq!(mixed[1].text, "今天学习");
        assert_eq!(mixed[1].reading, "jin tian xue xi");
        assert!(mixed.iter().all(|candidate| candidate.consumed == "jintianxx".len()
            && candidate.text.starts_with("今天")));
        assert!(candidates(&dictionary, "jintianxxx", budget).unwrap().is_empty());
    }
}
