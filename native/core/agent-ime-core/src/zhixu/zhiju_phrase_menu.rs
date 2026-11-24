//! Reading-backed phrases for the native candidate menu. The table is ordered by
//! the bundled Zhiju next-character model and accessed without loading a model on a key.
use super::decoder::candidate_coverage;
use crate::state::Candidate;

pub(crate) const CONTINUATION: &str = "短语 · 知句联想";
pub(crate) const TYPED_PHRASE: &str = "短语 · 知句";
const TABLE: &str = include_str!("../../resources/zhiju_phrase_continuations.tsv");
include!(concat!(env!("OUT_DIR"), "/zhiju_phrase_offsets.rs"));

pub(super) fn continuations<'a>(raw: &'a str, head: &'a Candidate)
    -> impl Iterator<Item = (&'static str, &'static str)> + 'a {
    let rows = HEADS.binary_search_by(|entry| entry.0.cmp(&head.text)).ok()
        .map(|pos| &TABLE[HEADS[pos].1..HEADS[pos].2]).unwrap_or("");
    // A prediction can follow a fully typed word. An initial, a homophone
    // with a different reading, or the beginning of a longer input cannot unlock it.
    rows.lines().filter_map(move |line| {
        let mut fields = line.split('\t');
        let (_, text, reading) = (fields.next()?, fields.next()?, fields.next()?);
        let mut spelling = reading.split_ascii_whitespace();
        let (first, second) = (spelling.next()?, spelling.next()?);
        if !first.bytes().chain(second.bytes()).eq(raw.bytes().filter(|&b| b != b'\'')) {
            return None;
        }
        if head.reading_key.split_ascii_whitespace().ne([first, second])
            || (raw.contains('\'') && candidate_coverage(raw, head) != Some(raw.len())) {
            return None;
        }
        Some((text, reading))
    })
}
