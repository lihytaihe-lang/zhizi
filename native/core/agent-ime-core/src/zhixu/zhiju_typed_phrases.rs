//! Common chat and office phrases whose entire reading was typed. The caller
//! owns placement in the selection table, including any protected personal head.

use super::decoder;
use crate::state::Candidate;
use std::sync::OnceLock;

const TABLE: &str = include_str!("../../resources/zhiju_typed_phrases.tsv");
const LIMIT: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TypedPhrase {
    pub(crate) text: &'static str,
    pub(crate) reading: &'static str,
    pub(crate) consumed: usize,
}

struct Entry {
    text: &'static str,
    reading: &'static str,
    compact: String,
    syllables: usize,
    proof: Candidate,
}

fn index() -> &'static [Vec<Entry>; 26] {
    static INDEX: OnceLock<[Vec<Entry>; 26]> = OnceLock::new();
    INDEX.get_or_init(|| {
        let mut buckets: [Vec<Entry>; 26] = std::array::from_fn(|_| Vec::new());
        for line in TABLE
            .lines()
            .filter(|line| !line.starts_with('#') && !line.is_empty())
        {
            let mut columns = line.split('\t');
            let (Some(text), Some(reading)) = (columns.next(), columns.next()) else {
                continue;
            };
            let count = text.chars().count();
            if !(3..=7).contains(&count)
                || reading.split(' ').count() != count
                || !reading.split(' ').all(decoder::full_syllable)
            {
                continue;
            }
            let Some(&first) = reading
                .as_bytes()
                .first()
                .filter(|byte| byte.is_ascii_lowercase())
            else {
                continue;
            };
            buckets[usize::from(first - b'a')].push(Entry {
                text,
                reading,
                compact: reading.replace(' ', ""),
                syllables: count,
                proof: Candidate {
                    text: text.into(),
                    reading_key: reading.into(),
                    ..Candidate::default()
                },
            });
        }
        buckets
    })
}

pub(crate) fn candidates(raw: &str) -> Vec<TypedPhrase> {
    if !(3..=49).contains(&raw.len())
        || !raw.as_bytes()[0].is_ascii_lowercase()
        || !raw.bytes().all(|b| b.is_ascii_lowercase() || b == b'\'')
        || raw.ends_with('\'')
        || raw.contains("''")
        || raw.len() <= 6 && decoder::full_syllable(raw)
    {
        return Vec::new();
    }
    let letters = raw.bytes().filter(|&b| b != b'\'').count();
    if letters < 3 {
        return Vec::new();
    }
    let bucket = &index()[usize::from(raw.as_bytes()[0] - b'a')];
    let mut found: Vec<TypedPhrase> = Vec::new();
    // Exact full spelling precedes ambiguous abbreviations. Source order breaks ties.
    for need_exact in [true, false] {
        for row in bucket {
            if row.syllables > letters || found.iter().any(|word| word.text == row.text) {
                continue;
            }
            let exact = letters == row.compact.len()
                && raw.bytes().filter(|&b| b != b'\'').eq(row.compact.bytes());
            if exact == need_exact
                && decoder::candidate_coverage(raw, &row.proof) == Some(raw.len())
            {
                found.push(TypedPhrase {
                    text: row.text,
                    reading: row.reading,
                    consumed: raw.len(),
                });
                if found.len() == LIMIT {
                    return found;
                }
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn source_has_documented_distinct_valid_whole_readings() {
        let mut seen = HashSet::new();
        for line in TABLE
            .lines()
            .filter(|line| !line.starts_with('#') && !line.is_empty())
        {
            let fields: Vec<_> = line.split('\t').collect();
            assert_eq!(fields.len(), 4, "{line}");
            let (text, reading) = (fields[0], fields[1]);
            assert!(matches!(fields[2], "chat" | "office") && !fields[3].is_empty());
            assert!(seen.insert((text, reading)), "duplicate {line}");
            assert!(
                (3..=7).contains(&text.chars().count())
                    && text
                        .chars()
                        .all(|c| c == '〇' || ('\u{3400}'..='\u{9fff}').contains(&c))
            );
            assert_eq!(reading.split(' ').count(), text.chars().count());
            assert!(reading.split(' ').all(decoder::full_syllable), "{line}");
        }
        assert_eq!(index().iter().map(Vec::len).sum::<usize>(), seen.len());
    }

    #[test]
    fn full_initial_mixed_and_explicit_boundaries_recall_the_same_phrase() {
        for raw in [
            "qingchashoufujian",
            "qcsfj",
            "qingcsfj",
            "qing'cha'shou'fu'jian",
        ] {
            assert!(
                candidates(raw).iter().any(|row| row.text == "请查收附件"
                    && row.reading == "qing cha shou fu jian"
                    && row.consumed == raw.len()),
                "{raw}"
            );
        }
        for raw in [
            "qcsf",
            "qcsfjx",
            "qingchashoufurian",
            "qi'ngchashoufujian",
            "qcsfj'",
            "q''csfj",
        ] {
            assert!(
                candidates(raw).iter().all(|row| row.text != "请查收附件"),
                "{raw}"
            );
        }
    }

    #[test]
    fn chat_and_work_offers_preserve_the_whole_phrase_reading() {
        for (text, raw, other_reading) in [
            ("还得再等等", "haideizaidengdeng", "haidezaidengdeng"),
            ("重发一遍给你", "chongfayibiangeini", "zhongfayibiangeini"),
            ("忙完就回你", "mwjhn", "mwjn"),
            ("会议改到下午了", "hui'y'g'd'x'w'l", "hui'y'g'd'x'w'l'x"),
        ] {
            assert!(
                candidates(raw)
                    .iter()
                    .any(|offer| offer.text == text && offer.consumed == raw.len()),
                "{raw}"
            );
            assert!(
                candidates(other_reading)
                    .iter()
                    .all(|offer| offer.text != text),
                "{other_reading}"
            );
        }
    }

    #[test]
    fn polyphonic_forms_use_only_in_phrase_pronunciations() {
        for (raw, want) in [
            ("huangeinile", "还给你了"),
            ("haishimingtianba", "还是明天吧"),
            ("xingdetong", "行得通"),
            ("yinhangzhanghao", "银行账号"),
            ("chongxinfayixia", "重新发一下"),
            ("zhongdianshuoyixia", "重点说一下"),
            ("wodeizoule", "我得走了"),
            ("aisheishei", "爱谁谁"),
            ("aishuishui", "爱谁谁"),
        ] {
            assert!(candidates(raw).iter().any(|row| row.text == want), "{raw}");
        }
        for (raw, excluded) in [
            ("haigeinile", "还给你了"),
            ("hangdetong", "行得通"),
            ("yinxingzhanghao", "银行账号"),
            ("zhongxinfayixia", "重新发一下"),
            ("wodezoule", "我得走了"),
            ("aishuishei", "爱谁谁"),
        ] {
            assert!(
                candidates(raw).iter().all(|row| row.text != excluded),
                "{raw}"
            );
        }
        assert_eq!(
            candidates("ass")
                .iter()
                .filter(|row| row.text == "爱谁谁")
                .count(),
            1
        );
    }

    #[test]
    fn ordinary_single_syllables_and_non_phonetic_keys_do_not_trigger_phrases() {
        for raw in [
            "", "q", "han", "shang", "q5fj", "QCSFJ", "'mtj", "q-csfj", "数字",
        ] {
            assert!(candidates(raw).is_empty(), "{raw}");
        }
        assert!(candidates("mingtianjian")
            .iter()
            .any(|row| row.text == "明天见"));
    }
}
