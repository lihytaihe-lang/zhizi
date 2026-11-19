#[derive(Clone, Debug)]
pub(crate) struct Word {
    pub text: String,
    pub reading: String,
    pub dictionary_match: bool,
}

const RAW_MIN_CHARS: usize = 4;
const RAW_MAX_CHARS: usize = 24;
const TEXT_MIN_CHARS: usize = 2;
const TEXT_MAX_CHARS: usize = 4;
const SCAN_LIMIT: usize = 20;
const OUTPUT_LIMIT: usize = 7;

fn valid_raw(raw: &str) -> bool {
    let count = raw.chars().count();
    count >= RAW_MIN_CHARS
        && count <= RAW_MAX_CHARS
        && raw.chars().all(|c| c.is_ascii_lowercase())
}

fn valid_text(text: &str) -> bool {
    let count = text.chars().count();
    count >= TEXT_MIN_CHARS
        && count <= TEXT_MAX_CHARS
        && text.chars().all(|c| c >= '\u{4E00}' && c <= '\u{9FFF}')
}

fn syllables(reading: &str) -> Option<Vec<&str>> {
    let tokens: Vec<&str> = reading.split_whitespace().collect();
    if tokens.is_empty() {
        return None;
    }
    for token in &tokens {
        if !token.chars().all(|c| c.is_ascii_lowercase()) {
            return None;
        }
    }
    Some(tokens)
}

pub(crate) fn exact_words(raw: &str, visible: &[String], prefix: &[Word]) -> Vec<String> {
    let mut selected: Vec<String> = Vec::new();
    if !valid_raw(raw) {
        return selected;
    }
    let mut taken: Vec<&str> = Vec::new();
    for entry in prefix.iter().take(SCAN_LIMIT) {
        if selected.len() == OUTPUT_LIMIT {
            break;
        }
        if !entry.dictionary_match || !valid_text(&entry.text) {
            continue;
        }
        let tokens = match syllables(&entry.reading) {
            Some(tokens) => tokens,
            None => continue,
        };
        if tokens.len() != entry.text.chars().count() {
            continue;
        }
        if tokens.concat() != raw {
            continue;
        }
        if visible.iter().any(|hidden| hidden == &entry.text) {
            continue;
        }
        if taken.contains(&entry.text.as_str()) {
            continue;
        }
        taken.push(entry.text.as_str());
        selected.push(entry.text.clone());
    }
    selected
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(text: &str, reading: &str) -> Word {
        Word {
            text: text.to_string(),
            reading: reading.to_string(),
            dictionary_match: true,
        }
    }

    fn word_flag(text: &str, reading: &str, dictionary_match: bool) -> Word {
        Word {
            text: text.to_string(),
            reading: reading.to_string(),
            dictionary_match,
        }
    }

    #[test]
    fn accepts_exact_concatenations_only() {
        let prefix = vec![
            word("丨一", "ab  cd"),
            word("丨一三", "a bc\td"),
            word("\u{4E00}\u{9FFF}", "ab cd"),
            word("丨一三中", "a b c d"),
        ];
        assert_eq!(
            exact_words("abcd", &[], &prefix),
            vec!["丨一", "丨一三", "\u{4E00}\u{9FFF}", "丨一三中"]
        );
    }

    #[test]
    fn rejects_partial_initials_and_mismatched_counts() {
        let prefix = vec![
            word("丨一", "ab c"),
            word("丨一三中", "a b c"),
            word("丨一", "ab c d"),
            word("丨一", "abcd"),
            word("丨一三", "ab cd"),
        ];
        assert!(exact_words("abcd", &[], &prefix).is_empty());
    }

    #[test]
    fn excludes_visible_deduplicates_and_keeps_order() {
        let prefix = vec![
            word("丨一", "ab cd"),
            word("丨一", "a bcd"),
            word("丨三", "ab cd"),
        ];
        assert_eq!(
            exact_words("abcd", &[String::from("丨三")], &prefix),
            vec!["丨一"]
        );
    }

    #[test]
    fn preserves_prefix_order_with_interleaved_invalids() {
        let prefix = vec![
            word_flag("丨一", "ab cd", false),
            word("丨一三", "a bc d"),
            word("丨一", "ab! cd"),
            word("丨三中", "ab c d"),
        ];
        assert_eq!(exact_words("abcd", &[], &prefix), vec!["丨一三", "丨三中"]);
    }

    #[test]
    fn returns_at_most_seven_entries() {
        let chars = ['一', '丨', '三', '中', '五'];
        let combos = [(0usize, 1usize), (0, 2), (0, 3), (0, 4), (1, 0), (1, 2), (1, 3), (1, 4), (2, 0)];
        let readings = ["ab cd", "a bcd", "abc d"];
        let prefix: Vec<Word> = combos
            .iter()
            .enumerate()
            .map(|(i, &(a, b))| {
                let text: String = chars[a].to_string() + &chars[b].to_string();
                word(&text, readings[i % readings.len()])
            })
            .collect();
        assert_eq!(
            exact_words("abcd", &[], &prefix),
            vec!["一丨", "一三", "一中", "一五", "丨一", "丨三", "丨中"]
        );
    }

    #[test]
    fn scans_only_first_twenty_entries() {
        let mut prefix: Vec<Word> = (0..19).map(|_| word_flag("丨一", "ab cd", false)).collect();
        prefix.push(word("丨三", "ab cd"));
        prefix.push(word("丨中", "ab cd"));
        assert_eq!(exact_words("abcd", &[], &prefix), vec!["丨三"]);
    }

    #[test]
    fn rejects_invalid_raw() {
        let prefix = vec![word("丨一", "ab cd")];
        let long = "a".repeat(25);
        let upper = "abcd".to_uppercase();
        for raw in ["", "abc", long.as_str(), upper.as_str(), "ab1d", "ab d", "abcd一"] {
            assert!(exact_words(raw, &[], &prefix).is_empty());
        }
    }

    #[test]
    fn rejects_invalid_text_and_reading() {
        let prefix = vec![
            word("丨", "abcd"),
            word("丨一三中五", "a b c d e"),
            word("a一", "ab cd"),
            word("\u{4DFF}一", "ab cd"),
            word("\u{A000}一", "ab cd"),
            word("丨一", ""),
            word("丨一", "AB CD"),
            word("丨一", "a1 cd"),
            word("丨一", "ab cd e"),
        ];
        assert!(exact_words("abcd", &[], &prefix).is_empty());
    }

    #[test]
    fn handles_empty_inputs() {
        assert!(exact_words("abcd", &[], &[]).is_empty());
        let prefix = vec![word("丨一", "ab cd")];
        assert!(exact_words("", &[], &prefix).is_empty());
        assert!(exact_words("abcd", &[String::from("丨一")], &prefix).is_empty());
    }

    #[test]
    fn accepts_longest_raw_boundary() {
        let raw = "z".repeat(24);
        let prefix = vec![word("丨一三中", "zzzzzz zzzzzz zzzzzz zzzzzz")];
        assert_eq!(exact_words(&raw, &[], &prefix), vec!["丨一三中"]);
    }
}
