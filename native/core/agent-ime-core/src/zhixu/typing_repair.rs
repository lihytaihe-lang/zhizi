//! Search the compact pronunciation trie with a shared edit budget across words.
//! Every returned path accounts for all typed bytes. The raw composition stays editable.
use super::{Index, trie::CompactReadingTrie};
use std::{
    cell::RefCell,
    sync::{Arc, Weak},
};

const BEAM: usize = 3;
pub(crate) const COMMENT: &str = "拼音纠错";

#[derive(Clone, Default)]
struct Path {
    text: String,
    score: i32,
    words: usize,
}

#[derive(Clone)]
pub(crate) struct Repair {
    pub text: String,
    pub whole_word: bool,
}

// Traverse exact edges directly after spending the budget. The visitor sees the number
// of input bytes covered, so an edit on either side of a word boundary is representable.
fn visit(
    trie: &CompactReadingTrie,
    node: usize,
    raw: &[u8],
    at: usize,
    remaining: usize,
    found: &mut impl FnMut(usize, usize, std::ops::Range<usize>),
) {
    let terminal = trie.terminal(node);
    if at > 0 && !terminal.is_empty() {
        found(at, remaining, terminal);
    }
    if let Some(&letter) = raw.get(at) {
        if let Some(child) = trie.child(node, letter) {
            visit(trie, child, raw, at + 1, remaining, found);
        }
        if remaining == 0 {
            return;
        }
        visit(trie, node, raw, at + 1, remaining - 1, found);
        if let Some(&next) = raw.get(at + 1)
            && next != letter
            && let Some(swapped) = trie.child(node, next).and_then(|n| trie.child(n, letter))
        {
            visit(trie, swapped, raw, at + 2, remaining - 1, found);
        }
    }
    if remaining > 0 {
        for (expected, child) in trie.children(node) {
            visit(trie, child, raw, at, remaining - 1, found);
            if raw.get(at).is_some_and(|&letter| letter != expected) {
                visit(trie, child, raw, at + 1, remaining - 1, found);
            }
        }
    }
}

fn search(index: &Index, raw: &str) -> Vec<Repair> {
    let budget = if raw.len() >= 6 { 2 } else { 1 };
    let whole = index.whole_readings();
    let mut paths: Vec<[Vec<Path>; 3]> = (0..=raw.len()).map(|_| Default::default()).collect();
    paths[0][0].push(Path::default());
    for start in 0..raw.len() {
        let previous = std::mem::take(&mut paths[start]);
        let Some(used) = previous.iter().position(|v| !v.is_empty()) else {
            continue;
        };
        let available = budget - used;
        // Different edit scripts often reach the same reading. Expand its homophones once.
        let mut matches = std::collections::BTreeMap::new();
        visit(
            whole.trie(),
            0,
            &raw.as_bytes()[start..],
            0,
            available,
            &mut |len, left, range| {
                let edits = available - left;
                if edits > 0 && len < 3 {
                    return;
                }
                matches
                    .entry((len, range.start, range.end))
                    .and_modify(|old: &mut usize| *old = (*old).min(edits))
                    .or_insert(edits);
            },
        );
        for ((len, first, end), edits) in matches {
            for &(_, id) in whole.entries[first..end].iter().take(BEAM) {
                let word = &index.entries[id as usize];
                let text = index.texts.get(word.text);
                if !text
                    .chars()
                    .all(|ch| ('\u{3400}'..='\u{9fff}').contains(&ch))
                {
                    continue;
                }
                let score = (word.weight.max(1) as f32).ln() * 100.0;
                for (spent, prefixes) in previous[..=budget - edits].iter().enumerate() {
                    let out = &mut paths[start + len][spent + edits];
                    for prefix in prefixes {
                        let score = prefix.score + score as i32 - 1600;
                        if out.len() == BEAM && out.last().is_some_and(|p| p.score >= score) {
                            continue;
                        }
                        let joined = format!("{}{text}", prefix.text);
                        if let Some(old) = out.iter().position(|p| p.text == joined) {
                            if out[old].score >= score {
                                continue;
                            }
                            out.remove(old);
                        }
                        let pos = out.partition_point(|p| p.score >= score);
                        out.insert(
                            pos,
                            Path {
                                text: joined,
                                score,
                                words: prefix.words + 1,
                            },
                        );
                        out.truncate(BEAM);
                    }
                }
            }
        }
    }
    let mut complete = Vec::new();
    for (edits, paths) in paths.pop().unwrap().into_iter().enumerate().skip(1) {
        for path in paths {
            if path.text.chars().count() >= 2 {
                complete.push((
                    path.score - 700 * edits as i32,
                    Repair {
                        text: path.text,
                        whole_word: path.words == 1,
                    },
                ));
            }
        }
    }
    complete.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
    complete.into_iter().map(|(_, repair)| repair).collect()
}

#[derive(Default)]
pub(crate) struct Cache(RefCell<Option<(Weak<Index>, String, Vec<Repair>)>>);

impl Cache {
    pub fn matches(&self, index: &Arc<Index>, raw: &str) -> Vec<Repair> {
        if let Some((dictionary, key, repairs)) = self.0.borrow().as_ref()
            && key == raw
            && dictionary.as_ptr() == Arc::as_ptr(index)
        {
            return repairs.clone();
        }
        let repairs = search(index, raw);
        self.0
            .replace(Some((Arc::downgrade(index), raw.into(), repairs.clone())));
        repairs
    }
}
