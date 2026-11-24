//! Pinyin-constrained next-character search. The platform owns the model; the
//! core owns spellings, dictionary readings, the context window and the beam.

use super::Index;
use super::decoder::{canonical_syllable, full_syllable, initial};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::{Duration, Instant};

#[path = "zhiju_cutoff.rs"]
mod cutoff;

#[path = "zhiju_lexical.rs"]
pub(crate) mod lexical;

#[derive(Debug, PartialEq)]
pub(crate) enum SearchError {
    NotApplicable,
    Scorer,
    Invalid,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Candidate {
    pub text: String,
    pub score: f64,
    pub consumed: usize,
    pub reading: String,
}

pub(crate) struct Vocabulary {
    symbols: Vec<String>,
    ids: HashMap<char, i32>,
}

impl Vocabulary {
    pub(crate) fn load(path: &Path) -> Result<Self, String> {
        let symbols = serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        Self::new(symbols)
    }

    fn new(symbols: Vec<String>) -> Result<Self, String> {
        if symbols.get(..3) != Some(&["<pad>".into(), "<bos>".into(), "<unk>".into()][..])
            || symbols.len() > i32::MAX as usize
            || symbols.iter().collect::<HashSet<_>>().len() != symbols.len() {
            return Err("itos.json is not the 顺手·知字 vocabulary".into());
        }
        let mut ids = HashMap::new();
        for (id, symbol) in symbols.iter().enumerate() {
            if symbol.chars().count() == 1 { ids.insert(symbol.chars().next().unwrap(), id as i32); }
        }
        Ok(Self { symbols, ids })
    }

    fn prefix(&self, context: &str, generated: &str) -> Vec<i32> {
        let preceding: Vec<_> = context.chars().chain(generated.chars()).collect();
        let keep = preceding.len().min(38);
        std::iter::once(1).chain(preceding[preceding.len() - keep..].iter()
            .map(|ch| self.ids.get(ch).copied().unwrap_or(2))).collect()
    }
}

pub(crate) struct Search {
    vocab: Vocabulary,
    generation: Option<u64>,
    allowed: HashMap<String, Vec<usize>>,
    word_bonuses: Option<HashMap<String, Vec<(usize, f64)>>>,
    base: Weights,
}

#[derive(Clone, Copy, Default)]
struct Weights {
    initial_penalty: f64,
    word_mu: f64,
}

impl Weights {
    // Chosen 2026-11-12 on the owner's real abbreviated sentences (net +8 with the
    // lexicon-word lead guard) and the public tune subset (256 -> 357 of 700 top-1):
    // docs/current/evidence/zhiju-owner-findings/REPORT.md, "Claude 扫参".
    // Default (all zero) is the neutral search the mechanism tests pin.
    const PRODUCTION: Self = Self { initial_penalty: 2.0, word_mu: 0.1 };

    // Offline evaluation only; invalid overrides retain the given defaults.
    fn evaluation(defaults: Self) -> Self {
        Self {
            initial_penalty: Self::override_value(std::env::var("ZHIZI_EVAL_ZHIJU_INITIAL_PENALTY").ok().as_deref(), defaults.initial_penalty),
            word_mu: Self::override_value(std::env::var("ZHIZI_EVAL_ZHIJU_WORD_MU").ok().as_deref(), defaults.word_mu),
        }
    }

    fn override_value(value: Option<&str>, default: f64) -> f64 {
        value.and_then(|value| value.parse::<f64>().ok()).filter(|value| value.is_finite() && *value >= 0.0).unwrap_or(default)
    }
}

impl Search {
    pub(crate) fn new(vocab: Vocabulary) -> Self {
        Self { vocab, generation: None, allowed: HashMap::new(), word_bonuses: None, base: Weights::PRODUCTION }
    }

    fn readings(&mut self, index: &Index) {
        if self.generation == Some(index.generation) { return; }
        self.allowed.clear();
        self.word_bonuses = None;
        for (id, symbol) in self.vocab.symbols.iter().enumerate().skip(3) {
            if symbol.chars().count() != 1 || symbol.chars().any(char::is_control) { continue; }
            for reading in index.all_readings_for_text(symbol).filter(|reading| full_syllable(reading)) {
                self.allowed.entry(canonical_syllable(reading).into_owned()).or_default().push(id);
                if matches!(reading.get(..2), Some("zh" | "ch" | "sh")) {
                    self.allowed.entry(reading[..2].to_string()).or_default().push(id);
                }
                if reading.get(..1).is_some_and(initial) {
                    self.allowed.entry(reading[..1].to_string()).or_default().push(id);
                }
            }
        }
        for ids in self.allowed.values_mut() { ids.sort_unstable(); ids.dedup(); }
        self.generation = Some(index.generation);
    }

    fn ids(&self, token: &str) -> &[usize] {
        self.allowed.get(canonical_syllable(token).as_ref()).map_or(&[], Vec::as_slice)
    }

    fn words(&mut self, index: &Index) {
        if self.word_bonuses.is_some() { return; }
        let mut weights = HashMap::new();
        for word in &index.entries {
            let text = index.texts.get(word.text);
            if text.chars().count() >= 2 && word.weight > 0 {
                let weight = weights.entry(text).or_insert(0);
                *weight = (*weight).max(word.weight);
            }
        }
        let mut bonuses: HashMap<String, Vec<(usize, f64)>> = HashMap::new();
        for (word, weight) in weights {
            let (end, ch) = word.char_indices().next_back().unwrap();
            if let Some(&id) = self.vocab.ids.get(&ch) {
                bonuses.entry(word[..end].into()).or_default().push((id as usize, f64::from(weight).ln()));
            }
        }
        self.word_bonuses = Some(bonuses);
    }

    fn bonuses(&self, text: &str, mu: f64) -> HashMap<usize, f64> {
        let mut bonuses = HashMap::new();
        if mu != 0.0 && let Some(words) = &self.word_bonuses {
            for (start, _) in text.char_indices() {
                if let Some(completions) = words.get(&text[start..]) {
                    for &(id, bonus) in completions { *bonuses.entry(id).or_insert(0.0) += mu * bonus; }
                }
            }
        }
        bonuses
    }

    pub(crate) fn run(&mut self, index: &Index, keys: &str, context: &str, beam: usize,
        budget: Duration, scorer: impl FnMut(&[i32], usize, usize, &mut [f32], usize) -> Result<(), ()>)
        -> Result<Vec<Candidate>, SearchError> {
        let started = Instant::now();
        self.run_with_clock(index, keys, context, beam, budget, Weights::evaluation(self.base), scorer, || started.elapsed())
    }

    pub(crate) fn prepare(&mut self, index: &Index) {
        self.readings(index);
        if Weights::evaluation(self.base).word_mu != 0.0 { self.words(index); }
    }

    pub(crate) fn vocabulary_count(&self) -> usize { self.vocab.symbols.len() }

    pub(crate) fn begin(&mut self, index: &Index, keys: &str, context: &str, beam: usize)
        -> Result<Progress, SearchError> {
        self.begin_with_weights(index, keys, context, beam, Weights::evaluation(self.base))
    }

    fn begin_with_weights(&mut self, index: &Index, keys: &str, context: &str, beam: usize, weights: Weights)
        -> Result<Progress, SearchError> {
        if !crate::mixer::sentence_abbreviation(keys) { return Err(SearchError::NotApplicable); }
        let beam = if beam == 0 { 8 } else { beam };
        if beam.checked_mul(self.vocab.symbols.len()).is_none() { return Err(SearchError::Invalid); }
        self.readings(index);
        if weights.word_mu != 0.0 { self.words(index); }
        let edges = segment_edges(keys).into_iter().map(|edges| edges.into_iter().map(|edge| Edge {
            token: edge.token.to_owned(), consumed: edge.consumed, node: edge.node,
        }).collect()).collect();
        Ok(Progress { keys_len: keys.len(), context: context.into(), beam, weights, edges,
            active: vec![State { candidate: Candidate { text: String::new(), score: 0.0, consumed: 0,
                reading: String::new() }, node: 0 }], done: Vec::new(), tokens: Vec::new(), steps: 0 })
    }

    pub(crate) fn advance(&self, index: &Index, progress: &mut Progress, logprobs: &[f32]) -> Result<(), SearchError> {
        let vocab = self.vocab.symbols.len();
        if progress.tokens.is_empty() || logprobs.len() != progress.rows() * vocab { return Err(SearchError::Invalid); }
        if logprobs.iter().any(|value| !value.is_finite()) { return Err(SearchError::Scorer); }
        let beam = progress.beam;
        let weights = progress.weights;
        let mut prelim = Vec::new();
        for (row, state) in progress.active.iter().enumerate() {
            let logp = &logprobs[row * vocab..(row + 1) * vocab];
            let bonuses = self.bonuses(&state.candidate.text, weights.word_mu);
            let score = |id| match bonuses.get(&id) {
                Some(bonus) => f64::from(logp[id]) + bonus,
                None => f64::from(logp[id]),
            };
            for edge in &progress.edges[state.node] {
                let token = edge.token.as_str();
                // A span can contain hundreds of legal characters. Compute each
                // score once, select the cutoff, then sort only retained ties.
                // The frozen legacy oracle pins the same scores and request order.
                let scored = self.ids(token).iter().map(|&id| (id, score(id))).collect();
                for (id, token_score) in cutoff::keep_cutoff_ties(scored, beam) {
                    let reading = index.all_readings_for_text(&self.vocab.symbols[id])
                        .filter(|reading| full_syllable(reading)
                            && (canonical_syllable(reading) == canonical_syllable(token)
                                || initial(token) && reading.starts_with(token)))
                        .min().expect("allowed character has a matching dictionary reading");
                    let penalty = if initial(token) && !full_syllable(token) { weights.initial_penalty } else { 0.0 };
                    let increment = if penalty == 0.0 { token_score } else { token_score - penalty };
                    let score = state.candidate.score + increment;
                    if !score.is_finite() { return Err(SearchError::Invalid); }
                    prelim.push(State { candidate: Candidate {
                        text: state.candidate.text.clone() + &self.vocab.symbols[id],
                        score, consumed: edge.consumed,
                        reading: if state.candidate.reading.is_empty() { reading.into() }
                            else { format!("{} {reading}", state.candidate.reading) },
                    }, node: edge.node });
                }
            }
        }
        prelim.sort_by(|a, b| rank(&a.candidate, &b.candidate));
        let mut seen = HashSet::new();
        let active = prelim.into_iter().filter(|state| seen.insert((state.candidate.text.clone(), state.node)))
            .take(beam);
        progress.active.clear();
        // Retain completions even when the prediction crossed the caller's deadline.
        for state in active {
            if state.candidate.consumed == progress.keys_len { progress.done.push(state.candidate); }
            else { progress.active.push(state); }
        }
        progress.steps += 1;
        progress.tokens.clear();
        Ok(())
    }

    fn run_with_clock(&mut self, index: &Index, keys: &str, context: &str, beam: usize,
        budget: Duration, weights: Weights,
        mut scorer: impl FnMut(&[i32], usize, usize, &mut [f32], usize) -> Result<(), ()>,
        mut elapsed: impl FnMut() -> Duration) -> Result<Vec<Candidate>, SearchError> {
        if !crate::mixer::sentence_abbreviation(keys) { return Err(SearchError::NotApplicable); }
        let beam = if beam == 0 { 8 } else { beam };
        let vocab = self.vocab.symbols.len();
        if beam.checked_mul(vocab).is_none() { return Err(SearchError::Invalid); }
        if elapsed() >= budget { return Ok(Vec::new()); }
        let mut progress = self.begin_with_weights(index, keys, context, beam, weights)?;
        loop {
            if elapsed() >= budget || progress.next_tokens(self).is_none() { break; }
            let rows = progress.rows();
            let mut logprobs = Vec::new();
            logprobs.try_reserve_exact(rows * vocab).map_err(|_| SearchError::Invalid)?;
            logprobs.resize(rows * vocab, f32::NAN);
            scorer(&progress.tokens, rows, 40, &mut logprobs, vocab).map_err(|_| SearchError::Scorer)?;
            self.advance(index, &mut progress, &logprobs)?;
            if elapsed() >= budget { break; }
        }
        Ok(progress.finish())
    }
}

struct State {
    candidate: Candidate,
    node: usize,
}

fn rank(a: &Candidate, b: &Candidate) -> std::cmp::Ordering {
    b.score.total_cmp(&a.score).then(b.consumed.cmp(&a.consumed)).then(a.text.cmp(&b.text))
}

#[derive(Clone, Debug)]
struct Edge<T> {
    token: T,
    consumed: usize,
    node: usize,
}

fn segment_edges(keys: &str) -> Vec<Vec<Edge<&str>>> {
    let (spellings, _) = super::decoder::spelling_edges(keys, true);
    let mut edges = vec![Vec::new(); keys.len() + 1];
    for (start, options) in spellings.iter().enumerate() {
        for &(end, next) in options {
            let token = &keys[start..end];
            edges[start].push(Edge { token, consumed: next, node: next });
            if matches!(token, "zh" | "ch" | "sh") {
                let mid = edges.len();
                edges.push(vec![Edge { token: &keys[start + 1..end], consumed: next, node: next }]);
                edges[start].push(Edge { token: &keys[start..start + 1], consumed: start + 1, node: mid });
            }
        }
    }
    edges
}

pub(crate) struct Progress {
    keys_len: usize,
    context: String,
    beam: usize,
    weights: Weights,
    edges: Vec<Vec<Edge<String>>>,
    active: Vec<State>,
    done: Vec<Candidate>,
    tokens: Vec<i32>,
    steps: usize,
}

impl Progress {
    pub(crate) fn rows(&self) -> usize { self.tokens.len() / 40 }

    pub(crate) fn next_tokens(&mut self, search: &Search) -> Option<&[i32]> {
        if self.steps == 16 { return None; }
        if self.tokens.is_empty() {
            self.active.retain(|state| !self.edges[state.node].is_empty());
            if self.active.is_empty() { return None; }
            self.tokens.resize(self.active.len() * 40, 0);
            for (row, state) in self.active.iter().enumerate() {
                let prefix = search.vocab.prefix(&self.context, &state.candidate.text);
                self.tokens[row * 40..row * 40 + prefix.len()].copy_from_slice(&prefix);
            }
        }
        Some(&self.tokens)
    }

    pub(crate) fn finish(&mut self) -> Vec<Candidate> {
        self.steps = 16;
        self.tokens.clear();
        self.active.clear();
        self.done.sort_by(rank);
        let mut seen = HashSet::new();
        self.done.retain(|candidate| seen.insert(candidate.text.clone()));
        self.done.truncate(self.beam);
        self.done.clone()
    }
}

#[cfg(test)]
impl Search {
    // Frozen pre-continuation algorithm: independent result and request-order oracle.
    fn legacy_run_with_clock(&mut self, index: &Index, keys: &str, context: &str, beam: usize,
        budget: Duration, weights: Weights,
        mut scorer: impl FnMut(&[i32], usize, usize, &mut [f32], usize) -> Result<(), ()>,
        mut elapsed: impl FnMut() -> Duration) -> Result<Vec<Candidate>, SearchError> {
        if !crate::mixer::sentence_abbreviation(keys) { return Err(SearchError::NotApplicable); }
        let beam = if beam == 0 { 8 } else { beam };
        let vocab = self.vocab.symbols.len();
        if beam.checked_mul(vocab).is_none() { return Err(SearchError::Invalid); }
        if elapsed() >= budget { return Ok(Vec::new()); }
        self.readings(index);
        if weights.word_mu != 0.0 { self.words(index); }
        let edges = segment_edges(keys);
        let mut active = vec![State { candidate: Candidate { text: String::new(), score: 0.0, consumed: 0,
            reading: String::new() }, node: 0 }];
        let mut done = Vec::new();
        for _ in 0..16 {
            active.retain(|state| !edges[state.node].is_empty());
            if active.is_empty() || elapsed() >= budget { break; }
            // Fixed [B,40] ABI for both platform backends. Every active state
            // has generated the same number of characters, hence equal lengths.
            let rows = active.len();
            let mut tokens = vec![0; rows * 40];
            for (row, state) in active.iter().enumerate() {
                let prefix = self.vocab.prefix(context, &state.candidate.text);
                tokens[row * 40..row * 40 + prefix.len()].copy_from_slice(&prefix);
            }
            let mut logprobs = Vec::new();
            logprobs.try_reserve_exact(rows * vocab).map_err(|_| SearchError::Invalid)?;
            logprobs.resize(rows * vocab, f32::NAN);
            scorer(&tokens, rows, 40, &mut logprobs, vocab).map_err(|_| SearchError::Scorer)?;
            if logprobs.iter().any(|value| !value.is_finite()) { return Err(SearchError::Scorer); }
            let mut prelim = Vec::new();
            for (row, state) in active.iter().enumerate() {
                let logp = &logprobs[row * vocab..(row + 1) * vocab];
                let bonuses = self.bonuses(&state.candidate.text, weights.word_mu);
                let score = |id| match bonuses.get(&id) {
                    Some(bonus) => f64::from(logp[id]) + bonus,
                    None => f64::from(logp[id]),
                };
                for edge in &edges[state.node] {
                    let mut ids = self.ids(edge.token).to_vec();
                    if ids.is_empty() { continue; }
                    ids.sort_by(|&a, &b| score(b).total_cmp(&score(a)).then(a.cmp(&b)));
                    // Keep all ties at the per-span cutoff, as Python's stable
                    // argsort does, before the global text/consumption tie break.
                    let cutoff = score(ids[beam.min(ids.len()) - 1]);
                    for id in ids.into_iter().take_while(|&id| score(id) >= cutoff) {
                        let reading = index.all_readings_for_text(&self.vocab.symbols[id])
                            .filter(|reading| full_syllable(reading)
                                && (canonical_syllable(reading) == canonical_syllable(edge.token)
                                    || initial(edge.token) && reading.starts_with(edge.token)))
                            .min().expect("allowed character has a matching dictionary reading");
                        let penalty = if initial(edge.token) && !full_syllable(edge.token) { weights.initial_penalty } else { 0.0 };
                        let increment = if penalty == 0.0 { score(id) } else { score(id) - penalty };
                        let score = state.candidate.score + increment;
                        if !score.is_finite() { return Err(SearchError::Invalid); }
                        prelim.push(State { candidate: Candidate {
                            text: state.candidate.text.clone() + &self.vocab.symbols[id],
                            score, consumed: edge.consumed,
                            reading: if state.candidate.reading.is_empty() { reading.into() }
                                else { format!("{} {reading}", state.candidate.reading) },
                        }, node: edge.node });
                    }
                }
            }
            prelim.sort_by(|a, b| rank(&a.candidate, &b.candidate));
            let mut seen = HashSet::new();
            active = prelim.into_iter().filter(|state| seen.insert((state.candidate.text.clone(), state.node)))
                .take(beam).collect();
            // Collect before checking the deadline or the 16-step cap, including
            // completions from the forward that crossed the wall-clock budget.
            let mut unfinished = Vec::new();
            for state in active {
                if state.candidate.consumed == keys.len() { done.push(state.candidate); }
                else { unfinished.push(state); }
            }
            active = unfinished;
            if elapsed() >= budget { break; }
        }
        done.sort_by(rank);
        let mut seen = HashSet::new();
        done.retain(|candidate| seen.insert(candidate.text.clone()));
        done.truncate(beam);
        Ok(done)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::loader::{DictionaryData, Entry};
    use std::cell::Cell;

    fn fixture() -> (Search, Index) {
        let pairs = [("我", "wo"), ("外", "wai"), ("们", "men"), ("想", "xiang"), ("星", "xing"),
            ("吃", "chi"), ("菜", "cai"), ("张", "zhang"), ("常", "chang"), ("是", "shi"),
            ("和", "he"), ("阿", "a"), ("爱", "ai"), ("哦", "o"), ("偶", "ou"),
            ("居", "ju"), ("略", "lve"), ("对", "dui"), ("短语", "duan yu"), ("无", "w"), ("我们", "wo men")];
        let symbols = ["<pad>", "<bos>", "<unk>", "<chat>"].into_iter().map(str::to_string)
            .chain(pairs.iter().filter(|(ch, _)| ch.chars().count() == 1).map(|(ch, _)| ch.to_string())).collect();
        let index = Index::from_data(DictionaryData { entries: pairs.into_iter().map(|(text, reading)| Entry {
            text: text.into(), reading: reading.into(), weight: 10, layer: "fixture".into(),
        }).collect() });
        let mut search = Search::new(Vocabulary::new(symbols).unwrap());
        search.base = Weights::default();
        (search, index)
    }

    fn uniform(_: &[i32], _: usize, _: usize, out: &mut [f32], _: usize) -> Result<(), ()> {
        out.fill(-1.0);
        Ok(())
    }

    #[test]
    fn zhiju_continuation_matches_frozen_legacy_scores_order_readings_and_requests() {
        let (mut search, index) = fixture();
        let score = |tokens: &[i32], out: &mut [f32], vocab: usize, ties| {
            for (prefix, row) in tokens.chunks(40).zip(out.chunks_mut(vocab)) {
                let sum: i32 = prefix.iter().sum();
                for (id, value) in row.iter_mut().enumerate() {
                    *value = if ties { -1.0 } else { -((sum + id as i32 * 17) % 257) as f32 / 16.0 };
                }
            }
        };
        for keys in ["wmxc", "zhchshw", "wo'men'x'c", "a'o'w'm", "ju'lve'd'w", "w'w'w'w'w'w'w'w'w'w'w'w'w'w'w'w'w"] {
            for context in ["", "外我外我外我"] {
                for beam in [1, 8, 32] {
                    for weights in [Weights::default(), Weights::PRODUCTION] {
                        for ties in [false, true] {
                            let mut expected_requests = Vec::new();
                            let expected = search.legacy_run_with_clock(&index, keys, context, beam, Duration::from_secs(10), weights,
                                |tokens, _, _, out, vocab| {
                                    expected_requests.push(tokens.to_vec()); score(tokens, out, vocab, ties); Ok(())
                                }, || Duration::ZERO).unwrap();
                            let mut progress = search.begin_with_weights(&index, keys, context, beam, weights).unwrap();
                            let mut actual_requests = Vec::new();
                            while let Some(tokens) = progress.next_tokens(&search) {
                                actual_requests.push(tokens.to_vec());
                                assert_eq!(progress.next_tokens(&search).unwrap(), actual_requests.last().unwrap());
                                let mut scores = vec![f32::NAN; progress.rows() * search.vocabulary_count()];
                                score(&progress.tokens, &mut scores, search.vocabulary_count(), ties);
                                search.advance(&index, &mut progress, &scores).unwrap();
                                assert_eq!(progress.steps, actual_requests.len());
                            }
                            let actual = progress.finish();
                            assert_eq!(actual_requests, expected_requests, "{keys} / {context} / {beam}");
                            assert_eq!(actual, expected, "{keys} / {context} / {beam}");
                            assert_eq!(actual.iter().map(|c| c.score.to_bits()).collect::<Vec<_>>(),
                                expected.iter().map(|c| c.score.to_bits()).collect::<Vec<_>>());
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn zhiju_large_spans_match_legacy_at_cutoff_ties_and_signed_zero() {
        let symbols: Vec<String> = ["<pad>", "<bos>", "<unk>"].into_iter().map(str::to_string)
            .chain((0x4e00..0x5000).map(|value| char::from_u32(value).unwrap().to_string())).collect();
        let index = Index::from_data(DictionaryData { entries: symbols[3..].iter().map(|text| Entry {
            text: text.clone(), reading: "wo".into(), weight: 10, layer: "fixture".into(),
        }).collect() });
        let mut search = Search::new(Vocabulary::new(symbols).unwrap());
        for beam in [1, 8, 32] {
            for mode in 0..4 {
                let fill = |tokens: &[i32], out: &mut [f32], vocab: usize| {
                    for (prefix, row) in tokens.chunks(40).zip(out.chunks_mut(vocab)) {
                        let sum: usize = prefix.iter().map(|&value| value as usize).sum();
                        for (id, score) in row.iter_mut().enumerate() {
                            *score = match mode {
                                0 => -(((sum + id * 17) % 509) as f32) / 16.0,
                                1 => -(((sum + id * 17) % 11) as f32),
                                2 => if id % 2 == 0 { -0.0 } else { 0.0 },
                                _ => -1.0,
                            };
                        }
                    }
                };
                let mut expected_requests = Vec::new();
                let expected = search.legacy_run_with_clock(&index, "wwww", "", beam,
                    Duration::from_secs(10), Weights::PRODUCTION, |tokens, _, _, out, vocab| {
                        expected_requests.push(tokens.to_vec()); fill(tokens, out, vocab); Ok(())
                    }, || Duration::ZERO).unwrap();
                let mut actual_requests = Vec::new();
                let actual = search.run_with_clock(&index, "wwww", "", beam,
                    Duration::from_secs(10), Weights::PRODUCTION, |tokens, _, _, out, vocab| {
                        actual_requests.push(tokens.to_vec()); fill(tokens, out, vocab); Ok(())
                    }, || Duration::ZERO).unwrap();
                assert_eq!(actual_requests, expected_requests, "beam={beam}, mode={mode}");
                assert_eq!(actual, expected, "beam={beam}, mode={mode}");
                assert_eq!(actual.iter().map(|c| c.score.to_bits()).collect::<Vec<_>>(),
                    expected.iter().map(|c| c.score.to_bits()).collect::<Vec<_>>());
            }
        }
    }

    #[test]
    fn zhiju_evaluation_weight_overrides_are_finite_and_nonnegative() {
        for value in [None, Some(""), Some("bad"), Some("NaN"), Some("inf"), Some("-0.1")] {
            assert_eq!(Weights::override_value(value, 0.75), 0.75);
        }
        for value in ["0", "0.1", "2.5"] {
            assert_eq!(Weights::override_value(Some(value), 0.75), value.parse::<f64>().unwrap());
        }
    }

    #[test]
    fn zhiju_evaluation_overrides_reach_the_search() {
        const CHILD: &str = "ZHIZI_ZHIJU_WEIGHTS_TEST";
        if std::env::var_os(CHILD).is_none() {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "zhixu::zhiju::tests::zhiju_evaluation_overrides_reach_the_search", "--nocapture"])
                .env(CHILD, "1").env("ZHIZI_EVAL_ZHIJU_INITIAL_PENALTY", "2")
                .env("ZHIZI_EVAL_ZHIJU_WORD_MU", "0.1").output().unwrap();
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stdout));
            return;
        }
        let (mut search, index) = fixture();
        let result = search.run(&index, "wo'men'x'c", "", 8, Duration::from_secs(10), uniform).unwrap();
        assert!((result[0].score - (-8.0 + 0.1 * 10_f64.ln())).abs() < 1e-12);
        assert!(search.word_bonuses.is_some());
    }

    #[test]
    fn zhiju_zero_weights_match_the_legacy_search() {
        let (mut search, index) = fixture();
        let result = search.run_with_clock(&index, "wo'men'x'c", "", 8, Duration::from_secs(10),
            Weights::default(), uniform, || Duration::ZERO).unwrap();
        assert_eq!(result.iter().map(|candidate| (candidate.text.as_str(), candidate.score, candidate.consumed, candidate.reading.as_str()))
            .collect::<Vec<_>>(), [
                ("我们想吃", -4.0, 10, "wo men xiang chi"),
                ("我们想常", -4.0, 10, "wo men xiang chang"),
                ("我们想菜", -4.0, 10, "wo men xiang cai"),
                ("我们星吃", -4.0, 10, "wo men xing chi"),
                ("我们星常", -4.0, 10, "wo men xing chang"),
                ("我们星菜", -4.0, 10, "wo men xing cai"),
            ]);
        assert!(search.word_bonuses.is_none(), "zero mu skips the word index entirely");
    }

    #[test]
    fn zhiju_initial_penalty_counts_each_initial_including_digraph_splits() {
        let (mut search, index) = fixture();
        let run = |search: &mut Search, keys, penalty| search.run_with_clock(&index, keys, "", 64,
            Duration::from_secs(10), Weights { initial_penalty: penalty, word_mu: 0.0 }, uniform, || Duration::ZERO).unwrap();
        let plain = run(&mut search, "zh'ch'sh'w", 0.0);
        let penalized = run(&mut search, "zh'ch'sh'w", 0.75);
        for (text, initials) in [("张常是我", 4), ("张和常是我", 5), ("张常和是我", 5)] {
            let before = plain.iter().find(|candidate| candidate.text == text).unwrap();
            let after = penalized.iter().find(|candidate| candidate.text == text).unwrap();
            assert_eq!(after.score, before.score - 0.75 * f64::from(initials), "{text}");
        }
        let full = run(&mut search, "a'wo'm'zh", 0.75);
        assert_eq!(full.iter().find(|candidate| candidate.text == "阿我们张").unwrap().score, -4.0 - 1.5);
    }

    #[test]
    fn zhiju_word_mu_rewards_completed_suffixes_before_the_span_cutoff() {
        let pairs = [("我", "wo", 1), ("们", "men", 1), ("项", "xiang", 1), ("目", "mu", 1),
            ("买", "mai", 1), ("我们", "wo men", 100), ("项目", "xiang mu", 100),
            ("项目", "xiang mu", 10000), ("们项目", "men xiang mu", 1000),
            ("我们项目", "wo men xiang mu", 100000)];
        let index = |scale: u32| Index::from_data(DictionaryData { entries: pairs.into_iter().map(|(text, reading, weight)| Entry {
            text: text.into(), reading: reading.into(), weight: weight * scale, layer: "fixture".into(),
        }).collect() });
        let symbols = ["<pad>", "<bos>", "<unk>", "我", "们", "项", "目", "买"].into_iter().map(str::to_string).collect();
        let mut search = Search::new(Vocabulary::new(symbols).unwrap());
        let scorer = |tokens: &[i32], _: usize, cols: usize, out: &mut [f32], vocab| {
            for (prefix, row) in tokens.chunks(cols).zip(out.chunks_mut(vocab)) {
                let length = prefix.iter().take_while(|&&id| id != 0).count();
                row.fill(-10.0);
                if length < 4 { row[length + 2] = -1.0; }
                else { row[6] = -0.5; row[7] = -0.25; }
            }
            Ok(())
        };
        let run = |search: &mut Search, index: &Index, mu| search.run_with_clock(index, "womxiangm", "", 1,
            Duration::from_secs(10), Weights { initial_penalty: 0.0, word_mu: mu }, scorer, || Duration::ZERO).unwrap();
        let first = index(1);
        assert_eq!(run(&mut search, &first, 0.0)[0].text, "我们项买");
        let rewarded = run(&mut search, &first, 0.1);
        assert_eq!(rewarded[0].text, "我们项目");
        let expected = -3.5 + 0.1 * (100_f64.ln() + 10000_f64.ln() + 1000_f64.ln() + 100000_f64.ln());
        assert!((rewarded[0].score - expected).abs() < 1e-12);
        let second = index(2);
        let refreshed = run(&mut search, &second, 0.1);
        assert!((refreshed[0].score - rewarded[0].score - 0.4 * 2_f64.ln()).abs() < 1e-12);
    }

    fn public_search(keys: &str, context: &str, target: &str, rival: &str, weights: Weights) -> Vec<Candidate> {
        let pairs = [("这", "zhe"), ("个", "ge"), ("我", "wo"), ("是", "shi"), ("上", "shang"),
            ("们", "men"), ("在", "zai"), ("换", "huan"), ("喝", "he"), ("项", "xiang"), ("目", "mu"),
            ("想", "xiang"), ("买", "mai"), ("什", "shen"), ("么", "me"), ("香", "xiang"), ("米", "mi")];
        let mut entries: Vec<_> = pairs.iter().map(|(text, reading)| Entry {
            text: (*text).into(), reading: (*reading).into(), weight: 1, layer: "fixture".into(),
        }).collect();
        entries.push(Entry { text: "项目".into(), reading: "xiang mu".into(), weight: 10000, layer: "fixture".into() });
        let index = Index::from_data(DictionaryData { entries });
        let symbols: Vec<String> = ["<pad>", "<bos>", "<unk>"].into_iter().chain(pairs.iter().map(|(ch, _)| *ch)).map(str::to_string).collect();
        let mut search = Search::new(Vocabulary::new(symbols.clone()).unwrap());
        search.run_with_clock(&index, keys, context, 8, Duration::from_secs(10), weights,
            |tokens, _, cols, out, vocab| {
                for (prefix, row) in tokens.chunks(cols).zip(out.chunks_mut(vocab)) {
                    let generated: String = prefix.iter().skip(1 + context.chars().count()).take_while(|&&id| id != 0)
                        .map(|&id| symbols[id as usize].as_str()).collect();
                    row.fill(-30.0);
                    for (sentence, score) in [(target, -0.4_f32), (rival, -0.1)] {
                        if let Some(next) = sentence.strip_prefix(&generated).and_then(|suffix| suffix.chars().next()) {
                            let id = symbols.iter().position(|symbol| symbol == &next.to_string()).unwrap();
                            row[id] = row[id].max(score);
                        }
                    }
                }
                Ok(())
            }, || Duration::ZERO).unwrap()
    }

    #[test]
    fn zhiju_public_initial_regressions_with_and_without_context() {
        for context in ["", "我"] {
            for (keys, target, rival) in [("zhgewos", "这个我是", "在换个我上"), ("womzhge", "我们这个", "我们在喝个")] {
                assert_eq!(public_search(keys, context, target, rival, Weights::default())[0].text, rival);
                let corrected = public_search(keys, context, target, rival, Weights { initial_penalty: 2.0, word_mu: 0.0 });
                assert_eq!(corrected[0].text, target, "{keys} / {context}");
            }
        }
    }

    #[test]
    fn zhiju_public_project_words_are_in_the_top_five_with_mu() {
        for context in ["", "我"] {
            for (keys, target, rival) in [("womxiangm", "我们项目", "我们想买"), ("shenmxiangm", "什么项目", "什么香米")] {
                let corrected = public_search(keys, context, target, rival, Weights { initial_penalty: 0.0, word_mu: 0.1 });
                assert!(corrected.iter().take(5).any(|candidate| candidate.text == target), "{keys} / {context}: {corrected:?}");
            }
        }
    }

    fn paths<'a>(keys: &'a str, edges: &[Vec<Edge<&'a str>>], node: usize, path: Vec<&'a str>, out: &mut Vec<Vec<&'a str>>) {
        for edge in &edges[node] {
            let mut next = path.clone();
            next.push(edge.token);
            if edge.consumed == keys.len() { out.push(next); }
            else { paths(keys, edges, edge.node, next, out); }
        }
    }

    #[test]
    fn zhiju_segmentation_matches_preferred_product_spellings() {
        for keys in ["dengluc", "zhchshw", "wo'men'x'c", "a'o'w'm", "jv'lv'duei'w", "wo''menxc"] {
            let mut expected = Vec::new();
            super::super::decoder::visit_spellings(keys, |end, tokens| {
                if end == keys.len() { expected.push(tokens.to_vec()); }
            });
            let mut actual = Vec::new();
            paths(keys, &segment_edges(keys), 0, Vec::new(), &mut actual);
            expected.sort(); expected.dedup(); actual.sort(); actual.dedup();
            assert_eq!(actual, expected, "{keys}");
        }
    }

    #[test]
    fn zhiju_readings_include_zero_initials_aliases_and_only_single_char_entries() {
        let (mut search, index) = fixture();
        search.readings(&index);
        let chars = |token: &str| -> Vec<_> { search.ids(token).iter()
            .map(|&id| search.vocab.symbols[id].as_str()).collect() };
        assert_eq!(chars("a"), ["阿", "爱"]);
        assert_eq!(chars("o"), ["哦", "偶"]);
        assert_eq!(chars("jv"), ["居"]);
        assert_eq!(chars("lue"), ["略"]);
        assert_eq!(chars("duei"), ["对"]);
        assert!(!chars("w").contains(&"无"));
        assert!(!chars("d").contains(&"短语"));
    }

    #[test]
    fn zhiju_empty_generated_context_and_38_char_tail() {
        let (search, _) = fixture();
        assert_eq!(search.vocab.prefix("", ""), [1]);
        assert_eq!(search.vocab.prefix("不在字表", ""), [1, 2, 2, 2, 2]);
        let prefix = search.vocab.prefix(&"外".repeat(45), "我");
        assert_eq!(prefix.len(), 39);
        assert_eq!(*prefix.last().unwrap(), search.vocab.ids[&'我']);
        assert!(prefix[1..38].iter().all(|id| *id == search.vocab.ids[&'外']));
    }

    #[test]
    fn zhiju_beam_one_is_independent_greedy() {
        let (mut search, index) = fixture();
        search.readings(&index);
        for keys in ["wmxc", "zhchshw", "a'o'w'm"] {
            let edges = segment_edges(keys);
            let (mut text, mut score, mut node, mut consumed) = (String::new(), 0.0, 0, 0);
            while consumed < keys.len() {
                let mut options = Vec::new();
                for edge in &edges[node] {
                    for &id in search.allowed.get(edge.token).into_iter().flatten() {
                        options.push((-(id as f64), edge.consumed, text.clone() + &search.vocab.symbols[id], edge.node));
                    }
                }
                options.sort_by(|a, b| b.0.total_cmp(&a.0).then(b.1.cmp(&a.1)).then(a.2.cmp(&b.2)));
                let best = options.first().unwrap();
                score += best.0; consumed = best.1; text = best.2.clone(); node = best.3;
            }
            let got = search.run(&index, keys, "", 1, Duration::from_secs(10), |tokens, rows, cols, out, vocab| {
                assert_eq!(rows, 1);
                assert_eq!(tokens.len(), rows * cols);
                for row in out.chunks_mut(vocab) { for (id, value) in row.iter_mut().enumerate() { *value = -(id as f32); } }
                Ok(())
            }).unwrap();
            assert_eq!(got.iter().map(|candidate| (candidate.text.clone(), candidate.score, candidate.consumed))
                .collect::<Vec<_>>(), [(text, score, consumed)], "{keys}");
            assert_eq!(super::super::decoder::candidate_coverage(keys, &crate::state::Candidate {
                text: got[0].text.clone(), reading_key: got[0].reading.clone(), comment: String::new(), source_index: None,
            }), Some(keys.len()));
        }
    }

    #[test]
    fn zhiju_callback_errors_and_nonfinite_scores_are_rejected() {
        let (mut search, index) = fixture();
        assert_eq!(search.run(&index, "wmxc", "", 8, Duration::from_secs(10), |_, _, _, _, _| Err(())), Err(SearchError::Scorer));
        for value in [f32::NAN, f32::INFINITY] {
            assert_eq!(search.run(&index, "wmxc", "", 8, Duration::from_secs(10), |_, _, _, out, _| { out.fill(value); Ok(()) }), Err(SearchError::Scorer));
        }
    }

    #[test]
    fn zhiju_result_readings_cover_apostrophes_initials_and_canonical_syllables() {
        let (mut search, index) = fixture();
        for keys in ["wmxc", "wo'men'x'c", "zhchshw", "a'o'w'm", "ju'lve'd'w"] {
            let results = search.run(&index, keys, "", 8, Duration::from_secs(10), uniform)
                .unwrap_or_else(|error| panic!("{keys}: {error:?}"));
            assert!(!results.is_empty(), "{keys}");
            for result in results {
                assert_eq!(super::super::decoder::candidate_coverage(keys, &crate::state::Candidate {
                    text: result.text, reading_key: result.reading, comment: String::new(), source_index: None,
                }), Some(result.consumed), "{keys}");
                assert_eq!(result.consumed, keys.len());
            }
        }
    }

    #[test]
    fn zhiju_full_syllable_does_not_choose_a_longer_polyphonic_reading() {
        let index = Index::from_data(DictionaryData { entries: [
            ("居", "jv"), ("居", "jue"), ("们", "men"), ("我", "wo"), ("吃", "chi"),
        ].into_iter().map(|(text, reading)| Entry {
            text: text.into(), reading: reading.into(), weight: 10, layer: "fixture".into(),
        }).collect() });
        let symbols = ["<pad>", "<bos>", "<unk>", "居", "们", "我", "吃"].into_iter().map(str::to_string).collect();
        let mut search = Search::new(Vocabulary::new(symbols).unwrap());
        let result = search.run(&index, "ju'm'w'c", "", 1, Duration::from_secs(10), uniform).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].text, "居们我吃");
        assert_eq!(result[0].reading, "jv men wo chi");
    }

    #[test]
    fn zhiju_budget_keeps_completed_sentences_and_stops_callbacks() {
        let (mut search, index) = fixture();
        let calls = Cell::new(0);
        let result = search.run_with_clock(&index, "zhchshw", "", 8, Duration::from_millis(1), Weights::default(),
            |tokens, rows, cols, out, vocab| { calls.set(calls.get() + 1); uniform(tokens, rows, cols, out, vocab) },
            || if calls.get() >= 5 { Duration::from_millis(2) } else { Duration::ZERO }).unwrap();
        assert_eq!(calls.get(), 5);
        assert!(!result.is_empty());
        assert!(result.iter().all(|candidate| candidate.consumed == 7));
        assert!(result.iter().any(|candidate| candidate.text.chars().count() == 4));
        let result = search.run(&index, "wmxc", "", 8, Duration::ZERO, |_, _, _, _, _| panic!("budget already exhausted")).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn zhiju_not_applicable_never_calls_the_model() {
        let (mut search, index) = fixture();
        for raw in ["", "nihao", "nh", "wmx", "wo''menxc", "wo1menxc", "我wmxc", "gognneng", "woxianzaishiyongsougou"] {
            assert_eq!(search.run(&index, raw, "", 8, Duration::from_secs(1), |_, _, _, _, _| panic!("not applicable")), Err(SearchError::NotApplicable), "{raw}");
        }
    }

    #[test]
    fn zhiju_maximum_sixteen_steps_includes_the_last_completion() {
        let (mut search, index) = fixture();
        let calls = Cell::new(0);
        for length in [16, 17] {
            calls.set(0);
            let keys = vec!["w"; length].join("'");
            let got = search.run(&index, &keys, "", 1, Duration::from_secs(10), |tokens, rows, cols, out, vocab| {
                calls.set(calls.get() + 1); uniform(tokens, rows, cols, out, vocab)
            }).unwrap();
            assert_eq!(calls.get(), 16);
            assert_eq!(got.is_empty(), length == 17);
        }
    }

    #[test]
    fn zhiju_vocabulary_validation() {
        for symbols in [vec![], vec!["<bos>", "<pad>", "<unk>"], vec!["<pad>", "<bos>", "<unk>", "我", "我"]] {
            assert!(Vocabulary::new(symbols.into_iter().map(str::to_string).collect()).is_err());
        }
    }

    #[test]
    fn zhiju_uses_unprojected_readings_and_refreshes_dictionary_generations() {
        let (mut search, _) = fixture();
        let index = |reading: &str| Index::from_data(DictionaryData { entries: vec![Entry {
            text: "我".into(), reading: reading.into(), weight: 1, layer: "pinyin_simp".into(),
        }] });
        let first = index("wo");
        assert!(first.readings_for_text("我").next().is_none());
        search.readings(&first);
        assert_eq!(search.ids("w"), [search.vocab.ids[&'我'] as usize]);
        let second = index("ai");
        search.readings(&second);
        assert!(search.ids("w").is_empty());
        assert_eq!(search.ids("a"), [search.vocab.ids[&'我'] as usize]);
    }

    #[derive(Default)]
    struct Callback {
        calls: usize,
        first_prefix: Vec<i32>,
        fail: bool,
    }

    unsafe extern "C" fn callback(user_data: *mut std::ffi::c_void, tokens: *const i32,
        rows: usize, cols: usize, out: *mut f32, vocab: usize) -> i32 {
        let state = unsafe { &mut *user_data.cast::<Callback>() };
        state.calls += 1;
        if state.fail { return 77; }
        assert_eq!(cols, 40);
        assert!((1..=8).contains(&rows));
        let tokens = unsafe { std::slice::from_raw_parts(tokens, rows * cols) };
        if state.first_prefix.is_empty() {
            state.first_prefix = tokens[..40].iter().take_while(|&&id| id != 0).copied().collect();
        }
        unsafe { std::slice::from_raw_parts_mut(out, rows * vocab) }.fill(-1.0);
        0
    }

    #[test]
    fn zhiju_ffi_contract_and_page_are_stable_on_success_errors_and_short_buffers() {
        use crate::{AimSession, aim_session_set_zhiju_vocab, aim_session_zhiju_search};
        use crate::state::{KeyEvent, KeyKind, SecurityState, Session};
        let (search, index) = fixture();
        let path = std::env::temp_dir().join(format!("zhiju-ffi-{}-{}.json", std::process::id(), index.generation));
        std::fs::write(&path, serde_json::to_vec(&search.vocab.symbols).unwrap()).unwrap();
        let path_bytes = path.to_str().unwrap().as_bytes();
        let decoder = super::super::decoder::ZhixuDecoder::from_ready_dictionary(std::sync::Arc::new(index));
        let app_id = "zhiju.ffi-test".to_string();
        let mut session = AimSession { inner: Session::new(Box::new(decoder), app_id.clone()), app_id };
        session.inner.set_host_preceding_text("外");
        let key = |text: &str| KeyEvent { kind: KeyKind::Text, text: text.into(), modifiers: 0,
            security: SecurityState::NonSecure, delivery_id: 0 };
        for ch in "wmxc".chars() { session.inner.process(key(&ch.to_string())); }
        let before = session.inner.current_result();
        let mut state = Callback::default();
        let mut output = [0xa5; 2048];
        let run = |session: &mut AimSession, state: &mut Callback, output: &mut [u8], length: &mut usize| unsafe {
            aim_session_zhiju_search(session, Some(callback), (state as *mut Callback).cast(), 0, 10000, output.as_mut_ptr(), length)
        };
        let mut length = output.len();
        assert_eq!(run(&mut session, &mut state, &mut output, &mut length), -4);
        assert_eq!(length, 0);
        assert_eq!(state.calls, 0);
        assert_eq!(unsafe { aim_session_set_zhiju_vocab(&mut session, path_bytes.as_ptr(), path_bytes.len()) }, 0);
        std::fs::write(&path, b"[]").unwrap();
        assert_eq!(unsafe { aim_session_set_zhiju_vocab(&mut session, path_bytes.as_ptr(), path_bytes.len()) }, -2);
        length = output.len();
        assert_eq!(run(&mut session, &mut state, &mut output, &mut length), 0);
        assert_eq!(state.first_prefix, [1, search.vocab.ids[&'外']]);
        let encoded = std::str::from_utf8(&output[..length]).unwrap();
        assert!(!encoded.is_empty());
        assert!(encoded.lines().all(|line| {
            let fields: Vec<_> = line.split('\t').collect();
            fields.len() == 3 && fields[0].chars().count() == 4
                && fields[1].parse::<f64>().is_ok_and(|score| score.is_finite() && score <= -4.0) && fields[2] == "4"
        }));
        let after = session.inner.current_result();
        assert_eq!(before.marked_text, after.marked_text);
        assert_eq!(before.candidates, after.candidates);
        output.fill(0xa5); length = 1;
        assert_eq!(run(&mut session, &mut state, &mut output, &mut length), -6);
        assert!(length > 1);
        assert!(output.iter().all(|&byte| byte == 0xa5));
        state.fail = true; length = output.len();
        assert_eq!(run(&mut session, &mut state, &mut output, &mut length), -5);
        assert_eq!(length, 0);
        assert!(output.iter().all(|&byte| byte == 0xa5));
        state.calls = 0;
        session.inner.process(KeyEvent { kind: KeyKind::Escape, ..key("") });
        for ch in "nihao".chars() { session.inner.process(key(&ch.to_string())); }
        length = output.len();
        assert_eq!(run(&mut session, &mut state, &mut output, &mut length), -3);
        assert_eq!(state.calls, 0);
        session.inner.process(KeyEvent { security: SecurityState::Secure, ..key("wmxc") });
        length = output.len();
        assert_eq!(run(&mut session, &mut state, &mut output, &mut length), -3);
        assert_eq!(state.calls, 0);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn zhiju_ffi_rejects_null_arguments() {
        use crate::{aim_session_set_zhiju_vocab, aim_session_zhiju_search};
        use std::ptr::null_mut;
        let mut length = 10;
        assert_eq!(unsafe { aim_session_zhiju_search(null_mut(), Some(callback), null_mut(), 8, 10, null_mut(), &mut length) }, -1);
        assert_eq!(length, 0);
        assert_eq!(unsafe { aim_session_zhiju_search(null_mut(), None, null_mut(), 8, 10, null_mut(), null_mut()) }, -1);
        assert_eq!(unsafe { aim_session_set_zhiju_vocab(null_mut(), std::ptr::null(), 0) }, -1);
    }

    #[derive(Default)]
    struct Requests {
        tokens: Vec<Vec<i32>>,
        rows: usize,
        vocab: usize,
    }

    unsafe extern "C" fn request(user_data: *mut std::ffi::c_void, tokens: *const i32,
        rows: usize, cols: usize, vocab: usize) {
        let state = unsafe { &mut *user_data.cast::<Requests>() };
        assert_eq!(cols, 40);
        state.tokens.push(unsafe { std::slice::from_raw_parts(tokens, rows * cols) }.to_vec());
        state.rows = rows;
        state.vocab = vocab;
    }

    unsafe extern "C" fn record_scores(user_data: *mut std::ffi::c_void, tokens: *const i32,
        rows: usize, cols: usize, out: *mut f32, vocab: usize) -> i32 {
        unsafe { request(user_data, tokens, rows, cols, vocab); }
        unsafe { std::slice::from_raw_parts_mut(out, rows * vocab) }.fill(-1.0);
        0
    }

    fn step_session(keys: &str) -> crate::AimSession {
        use crate::state::{KeyEvent, KeyKind, SecurityState, Session};
        let (search, index) = fixture();
        let path = std::env::temp_dir().join(format!("zhiju-step-{}-{}.json", std::process::id(), index.generation));
        std::fs::write(&path, serde_json::to_vec(&search.vocab.symbols).unwrap()).unwrap();
        let app_id = format!("zhiju.step-test.{}", index.generation);
        let decoder = super::super::decoder::ZhixuDecoder::from_ready_dictionary(std::sync::Arc::new(index));
        let mut session = crate::AimSession { inner: Session::new(Box::new(decoder), app_id.clone()), app_id };
        session.inner.set_host_preceding_text("外");
        session.inner.set_zhiju_vocab(&path).unwrap();
        std::fs::remove_file(path).unwrap();
        for ch in keys.chars() {
            session.inner.process(KeyEvent { kind: KeyKind::Text, text: ch.to_string(), modifiers: 0,
                security: SecurityState::NonSecure, delivery_id: 0 });
        }
        session
    }

    fn step(session: &mut crate::AimSession, generation: &mut u64, scores: &[f32],
        requests: &mut Requests, budget: u64, output: &mut [u8], length: &mut usize) -> i32 {
        *length = output.len();
        unsafe { crate::aim_session_zhiju_step(session, generation, Some(request),
            (requests as *mut Requests).cast(), 8, budget, scores.as_ptr(), scores.len(), output.as_mut_ptr(), length) }
    }

    #[test]
    fn zhiju_steps_match_synchronous_results_and_request_each_distribution_once() {
        for keys in ["wmxc", "zhchshw", "wo'men'x'c", "a'o'w'm", "ju'lve'd'w"] {
            let mut session = step_session(keys);
            let mut expected = [0; 2048];
            let mut length = expected.len();
            let mut sync = Requests::default();
            assert_eq!(unsafe { crate::aim_session_zhiju_search(&mut session, Some(record_scores),
                (&mut sync as *mut Requests).cast(), 8, 10000, expected.as_mut_ptr(), &mut length) }, 0);
            let expected = expected[..length].to_vec();
            assert!(!expected.is_empty(), "{keys}");
            let before = session.inner.current_result();
            let mut requests = Requests::default();
            let mut generation = 0;
            let mut scores = Vec::new();
            let mut output = [0; 2048];
            for pass in 0..=16 {
                let status = step(&mut session, &mut generation, &scores, &mut requests, 10000, &mut output, &mut length);
                if status == 0 {
                    assert_eq!(&output[..length], expected, "{keys}");
                    assert_eq!(pass, sync.tokens.len());
                    break;
                }
                assert_eq!(status, 1, "{keys}");
                assert_eq!(length, 0);
                assert_eq!(requests.tokens.len(), pass + 1);
                scores = vec![-1.0; requests.rows * requests.vocab];
                assert!(pass < 16);
            }
            assert_ne!(generation, 0);
            assert_eq!(requests.tokens, sync.tokens, "{keys}");
            assert_eq!(session.inner.current_result().candidates, before.candidates);
            assert_eq!(session.inner.current_result().marked_text, before.marked_text);
        }
    }

    #[test]
    fn zhiju_detached_steps_match_every_candidate_score_reading_and_request() {
        for keys in ["wmxc", "zhchshw", "wo'men'x'c", "a'o'w'm", "ju'lve'd'w"] {
            let mut session = step_session(keys);
            let mut expected_requests = Vec::new();
            let expected = session.inner.zhiju_search(8, Duration::from_secs(10), |tokens, _, _, scores, _| {
                expected_requests.push(tokens.to_vec());
                scores.fill(-1.0);
                Ok(())
            }).unwrap();
            let mut snapshot = session.inner.zhiju_snapshot().unwrap();
            let mut search = fixture().0;
            // The session uses production weights, even in the neutral fixture.
            search.base = Weights::PRODUCTION;
            snapshot.prepare(&mut search);
            let mut requests = Vec::new();
            let mut scores = Vec::new();
            loop {
                let mut count = 0;
                let result = snapshot.step(&mut search, 8, Duration::from_secs(10), &scores, |tokens, rows, vocab| {
                    requests.push(tokens.to_vec());
                    count = rows * vocab;
                }).unwrap();
                if let Some(actual) = result {
                    assert_eq!(actual, expected, "{keys}");
                    assert_eq!(snapshot.results.as_ref(), Some(&expected));
                    break;
                }
                scores = vec![-1.0; count];
            }
            assert_eq!(requests, expected_requests, "{keys}");
        }
    }

    #[test]
    fn zhiju_detached_snapshot_outlives_keys_and_session_but_cannot_apply_stale_or_foreign_results() {
        use crate::state::{KeyEvent, KeyKind, SecurityState};
        let mut session = step_session("wmxc");
        let mut snapshot = session.inner.zhiju_snapshot().unwrap();
        let mut foreign = step_session("wmxc");
        assert!(matches!(foreign.inner.apply_zhiju_snapshot(&snapshot, false), Err(-8)));
        for kind in [KeyKind::Backspace, KeyKind::Space, KeyKind::PageDown, KeyKind::Escape] {
            let snapshot = session.inner.zhiju_snapshot().unwrap();
            session.inner.process(KeyEvent { kind, text: String::new(), modifiers: 0,
                security: SecurityState::NonSecure, delivery_id: 0 });
            assert!(matches!(session.inner.apply_zhiju_snapshot(&snapshot, false), Err(-8)));
            session = step_session("wmxc");
        }
        let before_mode_change = session.inner.zhiju_snapshot().unwrap();
        session.inner.set_mode(crate::state::InputMode::DirectEnglish);
        assert!(matches!(session.inner.apply_zhiju_snapshot(&before_mode_change, false), Err(-8)));
        drop(session);
        let mut search = fixture().0;
        snapshot.prepare(&mut search);
        let mut scores = Vec::new();
        loop {
            let mut count = 0;
            if snapshot.step(&mut search, 8, Duration::from_secs(10), &scores, |_, rows, vocab| count = rows * vocab)
                .unwrap().is_some() { break; }
            scores = vec![-1.0; count];
        }
        assert!(!snapshot.results.unwrap().is_empty());
    }

    #[test]
    fn zhiju_detached_ffi_validates_buffers_and_rereads_results() {
        use crate::*;
        let mut session = step_session("wmxc");
        let (search, _) = fixture();
        let mut search = AimZhijuSearch { inner: search };
        let mut snapshot = ptr::null_mut();
        assert_eq!(unsafe { aim_session_zhiju_snapshot(&session, &mut snapshot) }, 0);
        assert_eq!(unsafe { aim_zhiju_search_prepare(&mut search, snapshot) }, 0);
        let mut requests = Requests::default();
        let mut scores = Vec::new();
        let mut length = 0;
        let run = |snapshot, search: &mut AimZhijuSearch, scores: &[f32], output: &mut [u8], length: &mut usize, requests: &mut Requests| {
            *length = output.len();
            unsafe { aim_zhiju_snapshot_step(snapshot, search, Some(request), (requests as *mut Requests).cast(),
                8, 10000, scores.as_ptr(), scores.len(), output.as_mut_ptr(), length) }
        };
        loop {
            let status = run(snapshot, &mut search, &scores, &mut [], &mut length, &mut requests);
            if status == -6 { break; }
            assert_eq!(status, 1);
            scores = vec![-1.0; requests.rows * requests.vocab];
        }
        assert!(length > 0);
        let mut output = vec![0; length];
        assert_eq!(run(snapshot, &mut search, &[], &mut output, &mut length, &mut requests), 0);
        let mut page = AimResult::default();
        let status = unsafe { aim_session_apply_zhiju_snapshot(&mut session, snapshot, 1, &mut page) };
        assert!(status == 0 || status <= -10);
        assert!(page.candidate_count > 0);
        unsafe { aim_result_free(&mut page); aim_zhiju_snapshot_destroy(snapshot); }
        let mut invalid = 1usize as *mut AimZhijuSnapshot;
        assert_eq!(unsafe { aim_session_zhiju_snapshot(ptr::null(), &mut invalid) }, -1);
        assert!(invalid.is_null());
        assert_eq!(unsafe { aim_zhiju_search_prepare(ptr::null_mut(), ptr::null()) }, -1);
        assert!(unsafe { aim_zhiju_search_create(ptr::null(), 1) }.is_null());
        let mut empty = std::mem::MaybeUninit::uninit();
        assert_eq!(unsafe { aim_session_apply_zhiju_snapshot(ptr::null_mut(), ptr::null(), 0, empty.as_mut_ptr()) }, -1);
        let mut empty = unsafe { empty.assume_init() };
        assert!(empty.candidates.is_null());
        unsafe { aim_result_free(&mut empty); }
        assert_eq!(unsafe { aim_zhiju_snapshot_step(ptr::null_mut(), &mut search, Some(request), ptr::null_mut(),
            8, 0, ptr::null(), 0, ptr::null_mut(), &mut length) }, -1);
        unsafe { aim_zhiju_snapshot_destroy(ptr::null_mut()); aim_zhiju_search_destroy(ptr::null_mut()); }
    }

    #[test]
    fn zhiju_steps_invalidate_old_scores_after_keys_commit_paging_and_security_changes() {
        use crate::state::{KeyEvent, KeyKind, SecurityState};
        for kind in [KeyKind::Text, KeyKind::Backspace, KeyKind::Space, KeyKind::PageDown, KeyKind::Escape] {
            let mut session = step_session("wmxc");
            let mut generation = 0;
            let mut requests = Requests::default();
            let mut output = [0xa5; 2048];
            let mut length = output.len();
            assert_eq!(step(&mut session, &mut generation, &[], &mut requests, 10000, &mut output, &mut length), 1);
            session.inner.process(KeyEvent { kind, text: "w".into(), modifiers: 0,
                security: SecurityState::NonSecure, delivery_id: 0 });
            let scores = vec![-1.0; requests.rows * requests.vocab];
            assert_eq!(step(&mut session, &mut generation, &scores, &mut requests, 10000, &mut output, &mut length), -8);
            assert_eq!(length, 0);
            assert_eq!(requests.tokens.len(), 1);
            assert!(output.iter().all(|&byte| byte == 0xa5));
        }
        for security in [SecurityState::Secure, SecurityState::Unknown] {
            let mut session = step_session("wmxc");
            let mut generation = 0;
            let mut requests = Requests::default();
            let mut output = [0; 2048];
            let mut length = output.len();
            assert_eq!(step(&mut session, &mut generation, &[], &mut requests, 10000, &mut output, &mut length), 1);
            session.inner.process(KeyEvent { kind: KeyKind::Text, text: "w".into(), modifiers: 0, security, delivery_id: 0 });
            let scores = vec![-1.0; requests.rows * requests.vocab];
            assert_eq!(step(&mut session, &mut generation, &scores, &mut requests, 10000, &mut output, &mut length), -8);
            assert_eq!(requests.tokens.len(), 1);
        }
    }

    #[test]
    fn zhiju_step_validates_scores_and_can_reread_completed_results_after_a_short_buffer() {
        let mut session = step_session("wmxc");
        let mut generation = 0;
        let mut requests = Requests::default();
        let mut output = [0xa5; 2048];
        let mut length = output.len();
        assert_eq!(step(&mut session, &mut generation, &[], &mut requests, 0, &mut output, &mut length), 0);
        assert_eq!(generation, 0);
        assert!(requests.tokens.is_empty());
        assert_eq!(length, 0);
        assert_eq!(step(&mut session, &mut generation, &[], &mut requests, 10000, &mut output, &mut length), 1);
        assert_eq!(step(&mut session, &mut generation, &[], &mut requests, 10000, &mut output, &mut length), -2);
        let count = requests.rows * requests.vocab;
        assert_eq!(step(&mut session, &mut generation, &vec![-1.0; count - 1], &mut requests, 10000, &mut output, &mut length), -2);
        assert_eq!(step(&mut session, &mut generation, &vec![f32::NAN; count], &mut requests, 10000, &mut output, &mut length), -5);
        assert_eq!(requests.tokens.len(), 1);
        assert!(output.iter().all(|&byte| byte == 0xa5));
        for _ in 0..16 {
            let scores = vec![-1.0; requests.rows * requests.vocab];
            let status = step(&mut session, &mut generation, &scores, &mut requests, 10000, &mut output, &mut length);
            if status == 0 { break; }
            assert_eq!(status, 1);
        }
        assert!(length > 1);
        let expected = output[..length].to_vec();
        let calls = requests.tokens.len();
        output.fill(0xa5);
        assert_eq!(step(&mut session, &mut generation, &[], &mut requests, 10000, &mut output[..1], &mut length), -6);
        assert_eq!(length, expected.len());
        assert!(output.iter().all(|&byte| byte == 0xa5));
        assert_eq!(step(&mut session, &mut generation, &[], &mut requests, 10000, &mut output, &mut length), 0);
        assert_eq!(&output[..length], expected);
        assert_eq!(requests.tokens.len(), calls);
        let old = generation;
        generation = 0;
        assert_eq!(step(&mut session, &mut generation, &[], &mut requests, 10000, &mut output, &mut length), 1);
        assert_ne!(generation, old);
        let mut old = old;
        assert_eq!(step(&mut session, &mut old, &[], &mut requests, 10000, &mut output, &mut length), -8);
    }

    #[test]
    fn zhiju_step_ffi_rejects_null_arguments() {
        use std::ptr::{null, null_mut};
        let mut session = step_session("wmxc");
        let mut length = 0;
        let mut generation = 0;
        let call = |session, generation, callback, scores, count, length| unsafe {
            crate::aim_session_zhiju_step(session, generation, callback, null_mut(), 8, 1000, scores, count, null_mut(), length)
        };
        assert_eq!(call(null_mut(), &mut generation, Some(request), null(), 0, &mut length), -1);
        assert_eq!(call(&mut session, null_mut(), Some(request), null(), 0, &mut length), -1);
        assert_eq!(call(&mut session, &mut generation, None, null(), 0, &mut length), -1);
        assert_eq!(call(&mut session, &mut generation, Some(request), null(), 1, &mut length), -1);
        assert_eq!(call(&mut session, &mut generation, Some(request), null(), 0, null_mut()), -1);
    }

    #[test]
    fn zhiju_step_deadline_keeps_completed_expansions_and_issues_no_more_requests() {
        let mut session = step_session("zhchshw");
        let mut generation = 0;
        let mut requests = Requests::default();
        let mut output = [0; 2048];
        let mut length = output.len();
        let mut scores = Vec::new();
        for _ in 0..5 {
            assert_eq!(step(&mut session, &mut generation, &scores, &mut requests, 10000, &mut output, &mut length), 1);
            scores = vec![-1.0; requests.rows * requests.vocab];
        }
        assert_eq!(step(&mut session, &mut generation, &scores, &mut requests, 0, &mut output, &mut length), 0);
        assert_eq!(requests.tokens.len(), 5);
        let (mut search, index) = fixture();
        let calls = Cell::new(0);
        let expected = search.run_with_clock(&index, "zhchshw", "外", 8, Duration::from_millis(1), Weights::PRODUCTION,
            |tokens, rows, cols, out, vocab| { calls.set(calls.get() + 1); uniform(tokens, rows, cols, out, vocab) },
            || if calls.get() >= 5 { Duration::from_millis(2) } else { Duration::ZERO }).unwrap();
        let expected: String = expected.iter().map(|c| format!("{}\t{}\t{}\n", c.text, c.score, c.consumed)).collect();
        assert!(!expected.is_empty());
        assert_eq!(std::str::from_utf8(&output[..length]).unwrap(), expected);
    }
}
