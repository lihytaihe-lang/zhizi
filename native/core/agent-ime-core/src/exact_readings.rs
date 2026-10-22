//! Readings of candidate texts: the derived `agent_exact_readings.tsv` index (every
//! dictionary entry with its syllables) and, for phrases the decoder puts together,
//! readings composed from their characters.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

pub(crate) const EXACT_READING_INDEX_NAME: &str = "agent_exact_readings.tsv";
const MAX_INDEX_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Default)]
pub(crate) struct ExactReadingIndex {
    bytes: Vec<u8>,
    line_offsets: Vec<usize>,
}

impl ExactReadingIndex {
    fn parse(bytes: Vec<u8>) -> Self {
        let mut line_offsets = Vec::new();
        if !bytes.is_empty() {
            line_offsets.push(0);
            for (index, byte) in bytes.iter().enumerate() {
                if *byte == b'\n' && index + 1 < bytes.len() {
                    line_offsets.push(index + 1);
                }
            }
        }
        Self {
            bytes,
            line_offsets,
        }
    }

    fn line(&self, index: usize) -> &[u8] {
        let start = self.line_offsets[index];
        let end = self
            .line_offsets
            .get(index + 1)
            .copied()
            .unwrap_or(self.bytes.len());
        self.bytes[start..end]
            .strip_suffix(b"\n")
            .unwrap_or(&self.bytes[start..end])
    }

    /// Comma-separated readings of `text`, if the index has it.
    fn values(&self, text: &str) -> Option<&[u8]> {
        let target = text.as_bytes();
        let mut lower = 0;
        let mut upper = self.line_offsets.len();
        let index = loop {
            if lower >= upper {
                return None;
            }
            let middle = lower + (upper - lower) / 2;
            let line = self.line(middle);
            let key = line.split(|byte| *byte == b'\t').next().unwrap_or_default();
            match key.cmp(target) {
                std::cmp::Ordering::Less => lower = middle + 1,
                std::cmp::Ordering::Greater => upper = middle,
                std::cmp::Ordering::Equal => break middle,
            }
        };
        let line = self.line(index);
        let tab = line.iter().position(|byte| *byte == b'\t')?;
        Some(&line[tab + 1..])
    }

    pub(crate) fn lookup(&self, text: &str, raw: &str) -> Option<String> {
        let values = self.values(text)?;
        let selected = values
            .split(|byte| *byte == b',')
            .find(|value| {
                std::str::from_utf8(value)
                    .ok()
                    .and_then(canonical_dictionary_reading)
                    .as_deref()
                    == Some(raw)
            })
            .or_else(|| values.split(|byte| *byte == b',').next())?;
        String::from_utf8(selected.to_vec()).ok()
    }

    /// Reading of a candidate the index lacks (a phrase the decoder put together, such
    /// as 这就是 or 你先): the whole input when its characters can spell all of it (each
    /// one a full syllable or, abbreviated, the leading letters of one), else the
    /// characters' own syllables. A candidate covering only the start of the input
    /// (这就是 for zhejiushiwoshuo) then no longer passes for one covering all of it,
    /// so the context model cannot lift it over the whole sentence. None when a
    /// character is not in the index (the caller keeps the input).
    pub(crate) fn composed_reading(&self, text: &str, raw: &str) -> Option<String> {
        let mut syllables: Vec<Vec<String>> = Vec::new();
        for character in text.chars() {
            let values = self.values(character.encode_utf8(&mut [0; 4]))?;
            let readings: Vec<String> = values
                .split(|byte| *byte == b',')
                .filter_map(|value| std::str::from_utf8(value).ok().and_then(canonical_dictionary_reading))
                .collect();
            if readings.is_empty() {
                return None;
            }
            syllables.push(readings);
        }
        let mut reachable = vec![false; raw.len() + 1];
        reachable[0] = true;
        for readings in &syllables {
            let mut next = vec![false; raw.len() + 1];
            for start in (0..=raw.len()).filter(|&start| reachable[start]) {
                for reading in readings {
                    for length in 1..=reading.len().min(raw.len() - start) {
                        if raw[start..start + length] == reading[..length] {
                            next[start + length] = true;
                        }
                    }
                }
            }
            reachable = next;
        }
        Some(if reachable[raw.len()] {
            raw.to_string()
        } else {
            syllables.iter().map(|readings| readings[0].as_str()).collect::<Vec<_>>().join(" ")
        })
    }
}

fn reading_cache() -> &'static Mutex<HashMap<PathBuf, Arc<ExactReadingIndex>>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, Arc<ExactReadingIndex>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) fn canonical_dictionary_reading(value: &str) -> Option<String> {
    let reading = value
        .trim()
        .to_ascii_lowercase()
        .chars()
        .filter(|character| !character.is_whitespace() && *character != '\'')
        .collect::<String>();
    (!reading.is_empty() && reading.bytes().all(|byte| byte.is_ascii_lowercase()))
        .then_some(reading)
}

#[cfg(test)]
mod exact_reading_tests {
    use super::*;

    #[test]
    fn exact_index_preserves_syllable_boundaries_and_selects_the_matching_reading() {
        let index =
            ExactReadingIndex::parse(b"idiom\tluan qi ba zao\nterm\tle qi,yue qi\n".to_vec());
        assert_eq!(
            index.lookup("idiom", "luanqibazao").as_deref(),
            Some("luan qi ba zao")
        );
        assert_eq!(index.lookup("term", "yueqi").as_deref(), Some("yue qi"));
        assert_eq!(index.lookup("term", "leqi").as_deref(), Some("le qi"));
    }

    #[test]
    fn a_phrase_covering_only_the_start_of_the_input_gets_its_own_syllables() {
        // Sorted by UTF-8 bytes, as the generated index is.
        let index = ExactReadingIndex::parse(
            "你\tni\n先\txian\n友\tyou\n找\tzhao\n朋\tpeng\n给\tgei,ji\n".as_bytes().to_vec(),
        );
        // Not in the index: 你先 for nixianzhaozhao covers only the start.
        assert_eq!(index.composed_reading("你先", "nixianzhaozhao").as_deref(), Some("ni xian"));
        // Characters that spell the whole input, abbreviations included, keep it.
        assert_eq!(index.composed_reading("你先找找", "nixianzhaozhao").as_deref(), Some("nixianzhaozhao"));
        assert_eq!(index.composed_reading("给朋友", "geipy").as_deref(), Some("geipy"));
        assert_eq!(index.composed_reading("你吗", "nima"), None);
    }
}

pub(crate) fn load_exact_readings(shared_dir: &Path) -> Arc<ExactReadingIndex> {
    if let Ok(cache) = reading_cache().lock()
        && let Some(index) = cache.get(shared_dir)
    {
        return index.clone();
    }
    let path = shared_dir.join(EXACT_READING_INDEX_NAME);
    let bytes = match fs::metadata(&path) {
        Ok(metadata) if metadata.len() <= MAX_INDEX_BYTES => {
            fs::read(path).unwrap_or_default()
        }
        _ => Vec::new(),
    };
    let index = Arc::new(ExactReadingIndex::parse(bytes));
    if let Ok(mut cache) = reading_cache().lock() {
        cache.insert(shared_dir.to_path_buf(), index.clone());
    }
    index
}
