//! Baseline candidate scorers for the paper's ablation (local evaluation only).
//!
//! Both plug into the host through the same `Scorer` trait as 顺手·知字, so the candidate
//! lists, the rules that blend the scores with the decoder's order, and the budget are all
//! identical; only the scores differ.
//!   unigram: sum of log character frequencies (no context).
//!   ngram:   character 4-gram, interpolated absolute discounting (D = 0.75), reads the context.
//! Training text: the same clean corpus the model was pre-trained on, capped per source
//! (see `train`). Characters are numbered in order of first sight; the 14-bit ids are packed
//! into one u64 key per n-gram.
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::Path;
use zhizi_host::ranking::Scorer;

const BOS: u64 = 1;
const UNK: u64 = 0;
const MAX_ID: u64 = (1 << 14) - 1;
const DISCOUNT: f64 = 0.75;

pub struct Counts {
    ids: HashMap<char, u64>,
    /// grams[n-1]: count of each n-gram (packed).
    grams: [HashMap<u64, u32>; 4],
    /// For each history of length n (packed): total count and number of distinct continuations.
    histories: [HashMap<u64, (u32, u32)>; 3],
    total: f64,
    vocabulary: f64,
}

fn pack(ids: &[u64]) -> u64 {
    ids.iter().fold(0u64, |key, &id| (key << 14) | id)
}

impl Counts {
    fn id(&mut self, c: char) -> u64 {
        let next = self.ids.len() as u64 + 2;
        if let Some(&id) = self.ids.get(&c) {
            return id;
        }
        if next > MAX_ID {
            return UNK;
        }
        self.ids.insert(c, next);
        next
    }

    fn lookup(&self, c: char) -> u64 {
        self.ids.get(&c).copied().unwrap_or(UNK)
    }
}

/// `sources`: (file, character cap) pairs read in order.
pub fn train(sources: &[(&Path, usize)]) -> Counts {
    let mut counts = Counts {
        ids: HashMap::new(),
        grams: Default::default(),
        histories: Default::default(),
        total: 0.0,
        vocabulary: 0.0,
    };
    for (path, cap) in sources {
        let mut seen = 0usize;
        let reader = BufReader::new(std::fs::File::open(path).unwrap());
        for line in reader.lines() {
            let Ok(line) = line else { continue };
            if seen >= *cap {
                break;
            }
            seen += line.chars().count();
            let ids: Vec<u64> = std::iter::repeat(BOS).take(3).chain(line.chars().map(|c| counts.id(c))).collect();
            for end in 3..ids.len() {
                for n in 1..=4 {
                    *counts.grams[n - 1].entry(pack(&ids[end + 1 - n..=end])).or_insert(0) += 1;
                }
                counts.total += 1.0;
            }
        }
    }
    // History statistics from the n-gram tables: history = gram without its last id.
    for n in 2..=4 {
        let mut stats: HashMap<u64, (u32, u32)> = HashMap::new();
        for (&gram, &count) in &counts.grams[n - 1] {
            let entry = stats.entry(gram >> 14).or_insert((0, 0));
            entry.0 += count;
            entry.1 += 1;
        }
        counts.histories[n - 2] = stats;
    }
    counts.vocabulary = counts.ids.len() as f64 + 2.0;
    counts
}

impl Counts {
    fn unigram(&self, id: u64) -> f64 {
        let count = f64::from(self.grams[0].get(&id).copied().unwrap_or(0));
        (count + 1.0) / (self.total + self.vocabulary)
    }

    /// P(next | history), history = up to the last three ids.
    fn probability(&self, history: &[u64], next: u64) -> f64 {
        let mut p = self.unigram(next);
        for n in 2..=4 {
            if history.len() + 1 < n {
                break;
            }
            let h = &history[history.len() + 1 - n..];
            let Some(&(total, kinds)) = self.histories[n - 2].get(&pack(h)) else { break };
            let mut gram = h.to_vec();
            gram.push(next);
            let count = f64::from(self.grams[n - 1].get(&pack(&gram)).copied().unwrap_or(0));
            let total = f64::from(total);
            p = ((count - DISCOUNT).max(0.0) + DISCOUNT * f64::from(kinds) * p) / total;
        }
        p
    }
}

pub struct NgramScorer(pub Counts);

impl Scorer for NgramScorer {
    fn score(&mut self, context: &str, candidates: &[String]) -> Result<Vec<f64>, String> {
        let tail: Vec<char> = context.chars().rev().take(3).collect::<Vec<_>>().into_iter().rev().collect();
        let mut base: Vec<u64> = std::iter::repeat(BOS).take(3 - tail.len()).collect();
        base.extend(tail.iter().map(|&c| self.0.lookup(c)));
        Ok(candidates
            .iter()
            .map(|text| {
                let mut history = base.clone();
                let mut score = 0.0;
                for c in text.chars() {
                    let id = self.0.lookup(c);
                    score += self.0.probability(&history[history.len() - 3..], id).ln();
                    history.push(id);
                }
                score
            })
            .collect())
    }
}

pub struct UnigramScorer(pub Counts);

impl Scorer for UnigramScorer {
    fn score(&mut self, _context: &str, candidates: &[String]) -> Result<Vec<f64>, String> {
        Ok(candidates.iter().map(|text| text.chars().map(|c| self.0.unigram(self.0.lookup(c)).ln()).sum()).collect())
    }
}

/// Default training mix: all of the colloquial sources, capped written ones (characters).
pub fn default_sources(dir: &Path) -> Vec<(std::path::PathBuf, usize)> {
    vec![
        (dir.join("asap.train.txt"), usize::MAX),
        (dir.join("tatoeba.train.txt"), usize::MAX),
        (dir.join("synth.train.txt"), usize::MAX),
        (dir.join("un.train.txt"), 15_000_000),
        (dir.join("wikipedia.train.txt"), 40_000_000),
    ]
}
