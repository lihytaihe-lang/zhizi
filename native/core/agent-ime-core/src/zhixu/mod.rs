//! Zhixu is a platform-independent runtime dictionary for local Chinese composition
//! and ASCII terms. Immutable snapshots share strings and entry IDs across lookups;
//! background loading replaces a whole snapshot while readers retain the previous one.

pub(crate) mod index_cache;
pub(crate) mod loader;
pub(crate) mod trie;
pub mod learning;
pub(crate) mod learning_session;
pub mod decoder;
pub(crate) mod fuzzy;

use index_cache::{Reader, Writer};
use loader::{DictionaryData, Entry};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use trie::{CompactReadingTrie, HeavyTrie};

static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);

fn fresh_generation() -> u64 {
    NEXT_GENERATION.fetch_add(1, Ordering::Relaxed)
}

/// Sorted strings in one allocation. IDs retain lexicographic order for trie ties.
struct Strings {
    bytes: String,
    spans: Vec<(usize, usize)>,
}

impl Strings {
    fn new<'a>(values: impl Iterator<Item = &'a str>) -> Self {
        let mut values: Vec<_> = values.collect();
        values.sort_unstable();
        values.dedup();
        let mut bytes = String::with_capacity(values.iter().map(|s| s.len()).sum());
        let mut spans = Vec::with_capacity(values.len());
        for value in values {
            spans.push((bytes.len(), value.len()));
            bytes.push_str(value);
        }
        Self { bytes, spans }
    }

    fn get(&self, id: u32) -> &str {
        let (start, len) = self.spans[id as usize];
        &self.bytes[start..start + len]
    }

    fn find(&self, value: &str) -> Option<u32> {
        self.spans
            .binary_search_by(|&(start, len)| self.bytes[start..start + len].cmp(value))
            .ok()
            .map(|id| id as u32)
    }

    fn cache_write(&self, out: &mut Writer) {
        out.u64(self.bytes.len() as u64);
        out.bytes(self.bytes.as_bytes());
        out.u64(self.spans.len() as u64);
        for &(start, len) in &self.spans {
            out.u32(start as u32);
            out.u32(len as u32);
        }
    }

    fn cache_read(from: &mut Reader) -> std::io::Result<Self> {
        let byte_count = from.count(from.remaining())?;
        let bytes = from.bytes(byte_count)?;
        let bytes = String::from_utf8(bytes.to_vec())
            .map_err(|_| from.invalid("cached index strings are not UTF-8"))?;
        let span_count = from.count(from.remaining() / 8)?;
        let mut spans = Vec::with_capacity(span_count);
        for _ in 0..span_count {
            let start = from.u32()? as usize;
            let len = from.u32()? as usize;
            if start.saturating_add(len) > bytes.len() {
                return Err(from.invalid("cached index string span is out of bounds"));
            }
            spans.push((start, len));
        }
        Ok(Self { bytes, spans })
    }

    #[cfg_attr(not(test), allow(dead_code))]
    fn estimated_bytes(&self) -> usize {
        self.bytes.capacity() + self.spans.capacity() * std::mem::size_of::<(usize, usize)>()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct StoredWord {
    text: u32,
    reading: u32,
    weight: u32,
    layers_start: u32,
    layers_len: u32,
}

#[cfg(test)]
pub(crate) struct Word {
    pub text: String,
    pub weight: u32,
    pub layers: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct WholeWord<'a> {
    pub(crate) text: &'a str,
    pub(crate) reading: &'a str,
    pub(crate) weight: u32,
    pub(crate) syllables: usize,
    pub(crate) reading_hash: u64,
    pub(crate) lexical_score: i64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct WholeWordMetadata {
    // Zero marks an invalid word; every normalized reading has a syllable.
    syllables: usize,
    reading_hash: u64,
    lexical_score: i64,
}

struct WholeReadingIndex {
    readings: Strings,
    entries: Vec<(u32, u32)>,
    trie: OnceLock<CompactReadingTrie>,
    metadata: OnceLock<Vec<WholeWordMetadata>>,
}

impl WholeReadingIndex {
    fn trie(&self) -> &CompactReadingTrie {
        self.trie.get_or_init(|| {
            let mut start = 0;
            CompactReadingTrie::new((0..self.readings.spans.len()).map(|reading| {
                let end = start + self.entries[start..].partition_point(|&(key, _)| key == reading as u32);
                let item = (self.readings.get(reading as u32), start as u32, (end - start) as u32);
                start = end;
                item
            }))
        })
    }
}

pub(crate) struct Index {
    pub generation: u64,
    entries: Vec<StoredWord>,
    texts: Strings,
    readings: Strings,
    layers: Strings,
    sources: Vec<u32>,
    by_reading: Vec<u32>,
    whole_readings: OnceLock<WholeReadingIndex>,
    whole_prewarm_started: AtomicBool,
    projection: Vec<u32>,
    by_text: Vec<u32>,
    all_by_text: Vec<u32>,
    initials: HashMap<String, Vec<u32>>,
    complete_initials: OnceLock<HashMap<String, Vec<u32>>>,
    pub trie: HeavyTrie,
    pub project: Arc<HashMap<String, String>>,
    pub english: Arc<HashMap<String, Vec<String>>>,
    pub tech: Arc<HashMap<String, Vec<String>>>,
    pub embedded: Arc<Vec<(String, String)>>,
}

impl Index {
    fn complete_initials(&self) -> &HashMap<String, Vec<u32>> {
        self.complete_initials.get_or_init(|| self.build_complete_initials())
    }

    fn build_complete_initials(&self) -> HashMap<String, Vec<u32>> {
        let mut result: HashMap<String, Vec<u32>> = HashMap::new();
        // Retain complete-reading order, including rare words outside the
        // correction projection. Each key also fixes the syllable count.
        for &id in &self.by_reading {
            let word = &self.entries[id as usize];
            let key: String = self.readings.get(word.reading).split_whitespace()
                .filter_map(|syllable| syllable.chars().next()).collect();
            result.entry(key).or_default().push(id);
        }
        result
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn whole_ready(&self) -> bool {
        self.complete_initials.get().is_some()
            && self.whole_readings.get().is_some_and(|whole| whole.trie.get().is_some() && whole.metadata.get().is_some())
    }

    pub(crate) fn warm_whole(&self) {
        if self.complete_initials.get().is_none() {
            let prepared = self.build_complete_initials();
            let _ = self.complete_initials.set(prepared);
        }
        if self.whole_readings.get().is_some_and(|whole| whole.trie.get().is_some() && whole.metadata.get().is_some()) {
            return;
        }
        // Build outside the live OnceLocks: an early key may initialize its own
        // snapshot instead of waiting for a background initializer to finish.
        let prepared = self.build_whole_readings();
        prepared.trie();
        let _ = prepared.metadata.set(self.build_whole_metadata());
        if let Err(prepared) = self.whole_readings.set(prepared) {
            let whole = self.whole_readings.get().unwrap();
            let _ = whole.trie.set(prepared.trie.into_inner().unwrap());
            let _ = whole.metadata.set(prepared.metadata.into_inner().unwrap());
        }
    }

    /// Counts rank the standalone candidate only. Learned words get a fixed
    /// composition weight, and learned single characters never enter this trie.
    pub(crate) fn from_learning(entries: &[learning::Entry]) -> Self {
        Self::from_data(DictionaryData {
            entries: entries.iter().filter(|entry| entry.text.chars().count() >= 2)
                .map(|entry| Entry {
                    text: entry.text.clone(),
                    reading: entry.reading.clone(),
                    weight: 800,
                    layer: "agent_zhixu_learning".into(),
                }).collect(),
        })
    }

    pub(crate) fn owner_words(&self) -> Vec<(String, String, u32)> {
        if self.layers.find("agent_owner_words").is_none() { return Vec::new(); }
        self.entries.iter().filter(|word| {
            self.sources[word.layers_start as usize..(word.layers_start + word.layers_len) as usize]
                .iter().any(|&layer| self.layers.get(layer) == "agent_owner_words")
        }).map(|word| (
            self.texts.get(word.text).to_string(),
            self.readings.get(word.reading).to_string(),
            word.weight,
        )).collect()
    }

    /// Learning needs readings even for rare or long words that the correction
    /// decoder deliberately leaves out of its projection.
    pub(crate) fn all_readings_for_text(&self, text: &str) -> impl Iterator<Item = &str> {
        let ids = self.texts.find(text).map_or(&[][..], |text| {
            let start = self.all_by_text.partition_point(|&id| self.entries[id as usize].text < text);
            let end = self.all_by_text.partition_point(|&id| self.entries[id as usize].text <= text);
            &self.all_by_text[start..end]
        });
        ids.iter().map(|&id| self.readings.get(self.entries[id as usize].reading))
    }

    pub fn from_data(data: DictionaryData) -> Self {
        let texts = Strings::new(data.entries.iter().map(|entry| entry.text.as_str()));
        let readings = Strings::new(data.entries.iter().map(|entry| entry.reading.as_str()));
        let layers = Strings::new(data.entries.iter().map(|entry| entry.layer.as_str()));
        let mut entries: Vec<StoredWord> = Vec::with_capacity(data.entries.len());
        let mut sources: Vec<Vec<u32>> = Vec::with_capacity(data.entries.len());
        let mut pairs: HashMap<(u32, u32), usize> = HashMap::with_capacity(data.entries.len());
        let mut project = HashMap::new();
        let mut english: HashMap<String, Vec<String>> = HashMap::new();
        let mut tech: HashMap<String, Vec<String>> = HashMap::new();
        for entry in data.entries {
            let Entry {
                text,
                reading,
                weight,
                layer,
            } = entry;
            match layer.as_str() {
                "agent_project_terms" => {
                    project.insert(reading.clone(), text.clone());
                }
                "agent_english" => {
                    english
                        .entry(reading.clone())
                        .or_default()
                        .push(text.clone());
                }
                "agent_tech_terms" | "agent_ai_terms" | "agent_pack_terms" => {
                    tech.entry(reading.clone()).or_default().push(text.clone());
                }
                _ if text.is_ascii() => {
                    tech.entry(reading.clone()).or_default().push(text.clone());
                }
                _ => {}
            }
            let text = texts.find(&text).unwrap();
            let reading = readings.find(&reading).unwrap();
            let layer = layers.find(&layer).unwrap();
            if let Some(&id) = pairs.get(&(text, reading)) {
                entries[id].weight = entries[id].weight.max(weight);
                if !sources[id].contains(&layer) {
                    sources[id].push(layer);
                }
            } else {
                pairs.insert((text, reading), entries.len());
                entries.push(StoredWord {
                    text,
                    reading,
                    weight,
                    layers_start: 0,
                    layers_len: 0,
                });
                sources.push(vec![layer]);
            }
        }
        drop(pairs);
        let mut source_ids = Vec::with_capacity(sources.iter().map(Vec::len).sum());
        for (entry, sources) in entries.iter_mut().zip(sources) {
            entry.layers_start = source_ids.len() as u32;
            entry.layers_len = sources.len() as u32;
            source_ids.extend(sources);
        }
        entries.shrink_to_fit();
        let mut by_reading: Vec<u32> = (0..entries.len() as u32).collect();
        by_reading.sort_by(|&a, &b| {
            let (a, b) = (&entries[a as usize], &entries[b as usize]);
            a.reading
                .cmp(&b.reading)
                .then_with(|| b.weight.cmp(&a.weight))
                .then_with(|| a.text.cmp(&b.text))
        });
        let valid = |word: &StoredWord| {
            let reading = readings.get(word.reading);
            let text = texts.get(word.text);
            !reading.is_empty()
                && reading
                    .split(' ')
                    .all(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_lowercase()))
                && text.chars().all(|c| {
                    ('\u{3400}'..='\u{9fff}').contains(&c) || ('\u{f900}'..='\u{faff}').contains(&c)
                })
                && text.chars().count() == reading.split(' ').count()
                && (text.chars().count() <= 7
                    || (text.chars().count() <= 64
                        && source_ids[word.layers_start as usize..(word.layers_start + word.layers_len) as usize]
                            .iter().any(|&layer| layers.get(layer) == "agent_zhixu_learning")))
                && (word.weight >= 500
                    || source_ids[word.layers_start as usize
                        ..(word.layers_start + word.layers_len) as usize]
                        .iter()
                        .any(|&layer| layers.get(layer) != "pinyin_simp"))
                && reading.len() < 256
        };
        let mut projection: Vec<u32> = by_reading
            .iter()
            .copied()
            .filter(|&id| valid(&entries[id as usize]))
            .collect();
        projection.shrink_to_fit();
        let mut all_by_text: Vec<u32> = (0..entries.len() as u32).collect();
        all_by_text.sort_by_key(|&id| (entries[id as usize].text, entries[id as usize].reading));
        let mut by_text = projection.clone();
        by_text.sort_by_key(|&id| (entries[id as usize].text, entries[id as usize].reading));
        let mut initials: HashMap<String, Vec<u32>> = HashMap::new();
        for &id in &projection {
            let word = &entries[id as usize];
            let key: String = readings
                .get(word.reading)
                .split(' ')
                .filter_map(|part| part.chars().next())
                .collect();
            initials.entry(key).or_default().push(id);
        }
        for ids in initials.values_mut() {
            ids.sort_by(|&a, &b| {
                let (a, b) = (&entries[a as usize], &entries[b as usize]);
                b.weight.cmp(&a.weight).then_with(|| a.text.cmp(&b.text))
            });
            ids.shrink_to_fit();
        }
        let mut trie = HeavyTrie::new();
        // Entry order is first import order, so trie node IDs and lattice beam grouping stay unchanged.
        for word in &entries {
            if valid(word) {
                trie.insert(word.text, readings.get(word.reading), word.weight);
            }
        }
        trie.finish();
        let mut ascii_readings: Vec<_> = tech.keys().chain(english.keys()).cloned().collect();
        ascii_readings.sort();
        ascii_readings.dedup();
        let mut embedded: Vec<_> = ascii_readings
            .into_iter()
            .filter_map(|reading| {
                tech.get(&reading)
                    .or_else(|| english.get(&reading))
                    .and_then(|values| values.first())
                    .map(|text| (reading, text.clone()))
            })
            .collect();
        embedded.sort_by(|a, b| b.0.len().cmp(&a.0.len()).then_with(|| a.0.cmp(&b.0)));
        Self {
            generation: fresh_generation(),
            entries,
            texts,
            readings,
            layers,
            sources: source_ids,
            by_reading,
            whole_readings: OnceLock::new(),
            whole_prewarm_started: AtomicBool::new(false),
            projection,
            by_text,
            all_by_text,
            initials,
            complete_initials: OnceLock::new(),
            trie,
            project: Arc::new(project),
            english: Arc::new(english),
            tech: Arc::new(tech),
            embedded: Arc::new(embedded),
        }
    }

    /// Serializes every `from_data` result plus the whole-reading structures
    /// `warm_whole` prepared. The runtime generation counter stays out; the
    /// reader assigns a fresh one.
    pub(crate) fn cache_write(&self, out: &mut Writer) {
        let warmed = self.whole_ready();
        out.u8(u8::from(warmed));
        out.u64(self.entries.len() as u64);
        for word in &self.entries {
            out.u32(word.text);
            out.u32(word.reading);
            out.u32(word.weight);
            out.u32(word.layers_start);
            out.u32(word.layers_len);
        }
        out.u64(self.sources.len() as u64);
        out.slice_u32(&self.sources);
        self.texts.cache_write(out);
        self.readings.cache_write(out);
        self.layers.cache_write(out);
        out.u64(self.by_reading.len() as u64);
        out.slice_u32(&self.by_reading);
        out.u64(self.projection.len() as u64);
        out.slice_u32(&self.projection);
        out.u64(self.by_text.len() as u64);
        out.slice_u32(&self.by_text);
        out.u64(self.all_by_text.len() as u64);
        out.slice_u32(&self.all_by_text);
        out.map_u32_ids(&self.initials);
        self.trie.cache_write(out);
        out.string_map(&self.project);
        out.string_map_vec(&self.english);
        out.string_map_vec(&self.tech);
        out.u64(self.embedded.len() as u64);
        for (reading, text) in self.embedded.iter() {
            out.str(reading);
            out.str(text);
        }
        if warmed {
            let whole = self.whole_readings.get().unwrap();
            whole.readings.cache_write(out);
            out.u64(whole.entries.len() as u64);
            for &(reading, id) in &whole.entries {
                out.u32(reading);
                out.u32(id);
            }
            whole.trie.get().unwrap().cache_write(out);
            let metadata = whole.metadata.get().unwrap();
            out.u64(metadata.len() as u64);
            for item in metadata {
                out.u32(item.syllables as u32);
                out.u64(item.reading_hash);
                out.i64(item.lexical_score);
            }
            out.map_u32_ids(self.complete_initials.get().unwrap());
        }
    }

    /// Rebuilds the index from cache bytes, leaving lazy structures empty when
    /// the cache predates warming. Every id is bounds-checked so a damaged file
    /// fails the read instead of producing an unsafe index.
    pub(crate) fn cache_read(from: &mut Reader) -> std::io::Result<Self> {
        let warmed = from.u8()? != 0;
        let entry_count = from.count(from.remaining() / 20)?;
        let mut entries = Vec::with_capacity(entry_count);
        for _ in 0..entry_count {
            entries.push(StoredWord {
                text: from.u32()?,
                reading: from.u32()?,
                weight: from.u32()?,
                layers_start: from.u32()?,
                layers_len: from.u32()?,
            });
        }
        let source_count = from.count(from.remaining() / 4)?;
        let sources = from.slice_u32(source_count)?;
        let texts = Strings::cache_read(from)?;
        let readings = Strings::cache_read(from)?;
        let layers = Strings::cache_read(from)?;
        let id_list = |from: &mut Reader| -> std::io::Result<Vec<u32>> {
            let count = from.count(from.remaining() / 4)?;
            Ok(from.slice_u32(count)?.to_vec())
        };
        let by_reading = id_list(from)?;
        let projection = id_list(from)?;
        let by_text = id_list(from)?;
        let all_by_text = id_list(from)?;
        for id in by_reading.iter().chain(&projection).chain(&by_text).chain(&all_by_text) {
            if *id as usize >= entry_count {
                return Err(from.invalid("cached index id is not an entry"));
            }
        }
        for word in &entries {
            if word.text as usize >= texts.spans.len()
                || word.reading as usize >= readings.spans.len()
                || word.layers_start as usize + word.layers_len as usize > sources.len()
                || word.layers_start.checked_add(word.layers_len).is_none()
            {
                return Err(from.invalid("cached index entry is out of bounds"));
            }
        }
        let initials = from.map_u32_ids(entry_count)?;
        let trie = HeavyTrie::cache_read(from, texts.spans.len())?;
        let project = from.string_map()?;
        let english = from.string_map_vec()?;
        let tech = from.string_map_vec()?;
        let embedded_count = from.count(from.remaining())?;
        let mut embedded = Vec::with_capacity(embedded_count);
        for _ in 0..embedded_count {
            embedded.push((from.str()?.to_string(), from.str()?.to_string()));
        }
        let (whole_readings, complete_initials, prewarm_started) = if warmed {
            let whole_readings_strings = Strings::cache_read(from)?;
            let whole_entry_count = from.count(from.remaining() / 8)?;
            let mut whole_entries = Vec::with_capacity(whole_entry_count);
            for _ in 0..whole_entry_count {
                let reading = from.u32()?;
                let id = from.u32()?;
                if reading as usize >= whole_readings_strings.spans.len() || id as usize >= entry_count {
                    return Err(from.invalid("cached whole entry is out of bounds"));
                }
                whole_entries.push((reading, id));
            }
            let trie = CompactReadingTrie::cache_read(from, whole_entries.len())?;
            let metadata_count = from.count(from.remaining() / 20)?;
            let mut metadata = Vec::with_capacity(metadata_count);
            for _ in 0..metadata_count {
                metadata.push(WholeWordMetadata {
                    syllables: from.u32()? as usize,
                    reading_hash: from.u64()?,
                    lexical_score: from.i64()?,
                });
            }
            if metadata.len() != entry_count {
                return Err(from.invalid("cached whole metadata size mismatch"));
            }
            let complete = from.map_u32_ids(entry_count)?;
            let whole = WholeReadingIndex {
                readings: whole_readings_strings,
                entries: whole_entries,
                trie: OnceLock::from(trie),
                metadata: OnceLock::from(metadata),
            };
            (OnceLock::from(whole), OnceLock::from(complete), true)
        } else {
            (OnceLock::new(), OnceLock::new(), false)
        };
        Ok(Self {
            generation: fresh_generation(),
            entries,
            texts,
            readings,
            layers,
            sources,
            by_reading,
            whole_readings,
            whole_prewarm_started: AtomicBool::new(prewarm_started),
            projection,
            by_text,
            all_by_text,
            initials,
            complete_initials,
            trie,
            project: Arc::new(project),
            english: Arc::new(english),
            tech: Arc::new(tech),
            embedded: Arc::new(embedded),
        })
    }

    /// Structural equality of every persisted field (generation and lazy
    /// OnceLock wrappers excluded) for the cache round-trip checks.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn cache_equals(&self, other: &Self) -> bool {
        self.entries == other.entries
            && self.texts.bytes == other.texts.bytes
            && self.texts.spans == other.texts.spans
            && self.readings.bytes == other.readings.bytes
            && self.readings.spans == other.readings.spans
            && self.layers.bytes == other.layers.bytes
            && self.layers.spans == other.layers.spans
            && self.sources == other.sources
            && self.by_reading == other.by_reading
            && self.projection == other.projection
            && self.by_text == other.by_text
            && self.all_by_text == other.all_by_text
            && self.initials == other.initials
            && self.trie.cache_equals(&other.trie)
            && *self.project == *other.project
            && *self.english == *other.english
            && *self.tech == *other.tech
            && *self.embedded == *other.embedded
            && match (self.complete_initials.get(), other.complete_initials.get()) {
                (None, None) => true,
                (Some(mine), Some(theirs)) => mine == theirs,
                _ => false,
            }
            && match (self.whole_readings.get(), other.whole_readings.get()) {
                (None, None) => true,
                (Some(mine), Some(theirs)) => {
                    mine.readings.bytes == theirs.readings.bytes
                        && mine.readings.spans == theirs.readings.spans
                        && mine.entries == theirs.entries
                        && match (mine.trie.get(), theirs.trie.get()) {
                            (None, None) => true,
                            (Some(mine), Some(theirs)) => mine.cache_equals(theirs),
                            _ => false,
                        }
                        && mine.metadata.get() == theirs.metadata.get()
                }
                _ => false,
            }
    }

    fn reading_ids<'a>(&'a self, ids: &'a [u32], reading: &str) -> &'a [u32] {
        let Some(reading) = self.readings.find(reading) else {
            return &[];
        };
        let start = ids.partition_point(|&id| self.entries[id as usize].reading < reading);
        let end = ids.partition_point(|&id| self.entries[id as usize].reading <= reading);
        &ids[start..end]
    }

    pub fn words_for_reading(&self, reading: &str) -> impl Iterator<Item = (&str, u32)> {
        self.reading_ids(&self.projection, reading)
            .iter()
            .map(|&id| {
                let word = &self.entries[id as usize];
                (self.texts.get(word.text), word.weight)
            })
    }

    fn whole_readings(&self) -> &WholeReadingIndex {
        self.whole_readings.get_or_init(|| self.build_whole_readings())
    }

    fn build_whole_readings(&self) -> WholeReadingIndex {
        let compact: Vec<_> = (0..self.readings.spans.len())
            .map(|id| self.readings.get(id as u32).replace(' ', ""))
            .collect();
        let readings = Strings::new(compact.iter().map(String::as_str));
        let mut entries: Vec<_> = self.entries.iter().enumerate().map(|(id, word)| {
            (readings.find(&compact[word.reading as usize]).unwrap(), id as u32)
        }).collect();
        entries.sort_by(|&(reading_a, id_a), &(reading_b, id_b)| {
            let (a, b) = (&self.entries[id_a as usize], &self.entries[id_b as usize]);
            reading_a.cmp(&reading_b).then_with(|| b.weight.cmp(&a.weight))
                .then_with(|| a.text.cmp(&b.text)).then_with(|| a.reading.cmp(&b.reading))
        });
        WholeReadingIndex { readings, entries, trie: OnceLock::new(), metadata: OnceLock::new() }
    }

    /// Complete homophones, including rare and long entries outside the correction
    /// projection. Build this index only when the whole-reading experiment uses it.
    pub(crate) fn all_words_for_compact_reading(&self, reading: &str) -> impl Iterator<Item = (&str, &str, u32)> {
        let whole = self.whole_readings();
        let entries = whole.readings.find(reading).map_or(&[][..], |reading| {
            let start = whole.entries.partition_point(|&(key, _)| key < reading);
            let end = whole.entries.partition_point(|&(key, _)| key <= reading);
            &whole.entries[start..end]
        });
        entries.iter().map(|&(_, id)| {
            let word = &self.entries[id as usize];
            (self.texts.get(word.text), self.readings.get(word.reading), word.weight)
        })
    }

    /// Complete words whose compact reading joins one spelling from each part,
    /// walking the trie so dead prefixes stop early. The visitor gets the chosen
    /// index into each part.
    pub(crate) fn visit_compact_paths<'a>(&'a self, parts: &[Vec<String>],
        mut visitor: impl FnMut(&[usize], &'a str, &'a str, u32)) {
        fn walk<'a>(index: &'a Index, trie: &CompactReadingTrie, node: usize, parts: &[Vec<String>],
            path: &mut Vec<usize>, visitor: &mut dyn FnMut(&[usize], &'a str, &'a str, u32)) {
            let Some(part) = parts.get(path.len()) else {
                for &(_, id) in &index.whole_readings().entries[trie.terminal(node)] {
                    let word = &index.entries[id as usize];
                    visitor(path, index.texts.get(word.text), index.readings.get(word.reading), word.weight);
                }
                return;
            };
            for (choice, spelling) in part.iter().enumerate() {
                let Some(next) = trie.walk(node, spelling) else { continue };
                path.push(choice);
                walk(index, trie, next, parts, path, visitor);
                path.pop();
            }
        }
        if parts.is_empty() { return; }
        let trie = self.whole_readings().trie();
        walk(self, trie, 0, parts, &mut Vec::with_capacity(parts.len()), &mut visitor);
    }

    pub(crate) fn visit_words_for_prefixes<'a>(&'a self, reading: &str,
        mut visitor: impl FnMut(usize, &'a str, &'a str, u32)) {
        let whole = self.whole_readings();
        whole.trie().visit_prefixes(reading, |end, range| {
            for &(_, id) in &whole.entries[range] {
                let word = &self.entries[id as usize];
                visitor(end, self.texts.get(word.text), self.readings.get(word.reading), word.weight);
            }
        });
    }

    fn whole_metadata(&self) -> &[WholeWordMetadata] {
        self.whole_readings().metadata.get_or_init(|| self.build_whole_metadata())
    }

    fn build_whole_metadata(&self) -> Vec<WholeWordMetadata> {
        self.entries.iter().map(|word| {
            let text = self.texts.get(word.text);
            let reading = self.readings.get(word.reading);
            if learning::normalize_reading(reading).is_none()
                || !text.chars().all(|c| ('\u{3400}'..='\u{9fff}').contains(&c)) {
                return WholeWordMetadata::default();
            }
            WholeWordMetadata {
                syllables: reading.split_whitespace().count(),
                reading_hash: reading.bytes().fold(0xcbf2_9ce4_8422_2325u64,
                    |hash, byte| (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)),
                lexical_score: ((word.weight.max(1) as f64).ln() * 1_000.0).round() as i64,
            }
        }).collect()
    }

    fn whole_word(&self, id: u32, metadata: &[WholeWordMetadata]) -> Option<WholeWord<'_>> {
        let metadata = &metadata[id as usize];
        if metadata.syllables == 0 { return None; }
        let word = &self.entries[id as usize];
        Some(WholeWord {
            text: self.texts.get(word.text), reading: self.readings.get(word.reading), weight: word.weight,
            syllables: metadata.syllables, reading_hash: metadata.reading_hash, lexical_score: metadata.lexical_score,
        })
    }

    pub(crate) fn whole_words_for_compact_reading(&self, reading: &str) -> impl Iterator<Item = WholeWord<'_>> + Clone {
        let whole = self.whole_readings();
        let metadata = self.whole_metadata();
        let entries = whole.readings.find(reading).map_or(&[][..], |reading| {
            let start = whole.entries.partition_point(|&(key, _)| key < reading);
            let end = whole.entries.partition_point(|&(key, _)| key <= reading);
            &whole.entries[start..end]
        });
        entries.iter().filter_map(|&(_, id)| self.whole_word(id, metadata))
    }

    /// Both syllable groups retain the complete index's static weight/text order.
    pub(crate) fn ranked_whole_words_for_compact_reading(&self, reading: &str, count: usize) -> impl Iterator<Item = WholeWord<'_>> {
        let words = self.whole_words_for_compact_reading(reading);
        words.clone().filter(move |word| word.syllables == count)
            .chain(words.filter(move |word| word.syllables != count))
    }

    pub(crate) fn visit_whole_words_for_prefixes<'a>(&'a self, reading: &str, after: usize,
        mut visitor: impl FnMut(usize, WholeWord<'a>)) {
        let whole = self.whole_readings();
        let metadata = self.whole_metadata();
        let mut visit = |end: usize, range: std::ops::Range<usize>| {
            if end <= after { return; }
            for &(_, id) in &whole.entries[range] {
                if let Some(word) = self.whole_word(id, metadata) { visitor(end, word); }
            }
        };
        // Fuzzy matches are verified syllable by syllable by the lattice.
        match fuzzy::mask() {
            0 => whole.trie().visit_prefixes(reading, visit),
            mask => whole.trie().visit_prefixes_fuzzy(reading, mask, &mut visit),
        }
    }

    pub fn readings_for_text(&self, text: &str) -> impl Iterator<Item = &str> {
        let ids = self.texts.find(text).map_or(&[][..], |text| {
            let start = self
                .by_text
                .partition_point(|&id| self.entries[id as usize].text < text);
            let end = self
                .by_text
                .partition_point(|&id| self.entries[id as usize].text <= text);
            &self.by_text[start..end]
        });
        ids.iter()
            .map(|&id| self.readings.get(self.entries[id as usize].reading))
    }

    pub fn vocabulary_word(&self, text: &str, reading: &str) -> bool {
        self.readings_for_text(text)
            .any(|spaced| spaced.bytes().filter(|&b| b != b' ').eq(reading.bytes()))
    }

    pub fn words_for_initials(&self, key: &str) -> impl Iterator<Item = (&str, &str, u32)> {
        self.initials.get(key).into_iter().flatten().map(|&id| {
            let word = &self.entries[id as usize];
            (
                self.texts.get(word.text),
                self.readings.get(word.reading),
                word.weight,
            )
        })
    }

    #[cfg(test)]
    pub fn entry_words(&self, reading: &str) -> Vec<Word> {
        self.reading_ids(&self.by_reading, reading)
            .iter()
            .map(|&id| {
                let word = &self.entries[id as usize];
                Word {
                    text: self.texts.get(word.text).into(),
                    weight: word.weight,
                    layers: self.sources[word.layers_start as usize
                        ..(word.layers_start + word.layers_len) as usize]
                        .iter()
                        .map(|&id| self.layers.get(id).into())
                        .collect(),
                }
            })
            .collect()
    }

    #[cfg(test)]
    pub fn projection_words(&self) -> impl Iterator<Item = (&str, &str, u32)> {
        self.projection.iter().map(|&id| {
            let word = &self.entries[id as usize];
            (
                self.texts.get(word.text),
                self.readings.get(word.reading),
                word.weight,
            )
        })
    }

    pub fn text(&self, terminal: trie::HeavyTerminal) -> &str {
        self.texts.get(terminal.text)
    }
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn projection_count(&self) -> usize {
        self.projection.len()
    }

    /// Owned capacity, excluding allocator metadata and temporary construction buffers.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn estimated_bytes(&self) -> usize {
        self.estimated_breakdown()
            .iter()
            .map(|(_, bytes)| bytes)
            .sum()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn estimated_breakdown(&self) -> Vec<(&'static str, usize)> {
        use std::mem::size_of;
        let ascii = self.project.capacity() * size_of::<(String, String)>()
            + self
                .project
                .iter()
                .map(|(k, v)| k.capacity() + v.capacity())
                .sum::<usize>()
            + [&self.english, &self.tech]
                .iter()
                .map(|map| {
                    map.capacity() * size_of::<(String, Vec<String>)>()
                        + map
                            .iter()
                            .map(|(k, v)| {
                                k.capacity()
                                    + v.capacity() * size_of::<String>()
                                    + v.iter().map(String::capacity).sum::<usize>()
                            })
                            .sum::<usize>()
                })
                .sum::<usize>()
            + self.embedded.capacity() * size_of::<(String, String)>()
            + self
                .embedded
                .iter()
                .map(|(k, v)| k.capacity() + v.capacity())
                .sum::<usize>();
        vec![
            ("index", size_of::<Self>()),
            (
                "entries",
                self.entries.capacity() * size_of::<StoredWord>()
                    + self.sources.capacity() * size_of::<u32>(),
            ),
            (
                "strings",
                self.texts.estimated_bytes()
                    + self.readings.estimated_bytes()
                    + self.layers.estimated_bytes(),
            ),
            ("reading_ids", self.by_reading.capacity() * size_of::<u32>()),
            ("complete_initials", self.complete_initials.get().map_or(0, |keys|
                keys.capacity() * size_of::<(String, Vec<u32>)>()
                    + keys.iter().map(|(key, ids)| key.capacity() + ids.capacity() * size_of::<u32>()).sum::<usize>())),
            ("whole_readings", self.whole_readings.get().map_or(0, |index| {
                index.readings.estimated_bytes() + index.entries.capacity() * size_of::<(u32, u32)>()
                    + index.trie.get().map_or(0, CompactReadingTrie::estimated_bytes)
                    + index.metadata.get().map_or(0, |metadata| metadata.capacity() * size_of::<WholeWordMetadata>())
            })),
            (
                "projection_ids",
                self.projection.capacity() * size_of::<u32>(),
            ),
            ("text_ids", (self.by_text.capacity() + self.all_by_text.capacity()) * size_of::<u32>()),
            (
                "initials",
                self.initials.capacity() * size_of::<(String, Vec<u32>)>()
                    + self
                        .initials
                        .iter()
                        .map(|(key, ids)| key.capacity() + ids.capacity() * size_of::<u32>())
                        .sum::<usize>(),
            ),
            ("trie", self.trie.estimated_bytes() - size_of::<HeavyTrie>()),
            ("ascii", ascii),
        ]
    }
}

#[cfg(test)]
thread_local! { pub(crate) static FALLBACK_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }

pub(crate) fn fallback() -> Arc<Index> {
    #[cfg(test)]
    FALLBACK_CALLS.with(|calls| calls.set(calls.get() + 1));
    static FALLBACK: OnceLock<Arc<Index>> = OnceLock::new();
    FALLBACK
        .get_or_init(|| {
            let entries = include_str!("../../resources/heavy_garble_words.tsv")
                .lines()
                .filter_map(|line| {
                    if line.starts_with('#') {
                        return None;
                    }
                    let mut columns = line.split('\t');
                    Some(Entry {
                        text: columns.next()?.into(),
                        reading: columns.next()?.into(),
                        weight: columns.next()?.parse().ok()?,
                        layer: "fallback".into(),
                    })
                })
                .collect();
            #[allow(unused_mut)]
            let mut data = DictionaryData { entries };
            #[cfg(test)]
            add_ascii_fixture(&mut data);
            Arc::new(Index::from_data(data))
        })
        .clone()
}

thread_local! { static PINNED: RefCell<Option<Option<Arc<Index>>>> = const { RefCell::new(None) }; }

thread_local! { static LEARNING_PINNED: RefCell<Option<Arc<Index>>> = const { RefCell::new(None) }; }

pub(crate) struct LearningSnapshotGuard(Option<Arc<Index>>);
impl Drop for LearningSnapshotGuard {
    fn drop(&mut self) {
        LEARNING_PINNED.with(|slot| { *slot.borrow_mut() = self.0.take(); });
    }
}

pub(crate) fn pin_learning(index: Option<Arc<Index>>) -> LearningSnapshotGuard {
    LearningSnapshotGuard(LEARNING_PINNED.with(|slot| slot.replace(index)))
}

pub(crate) fn learning_index() -> Option<Arc<Index>> {
    LEARNING_PINNED.with(|slot| slot.borrow().clone())
}

pub(crate) fn ready() -> Option<Arc<Index>> {
    PINNED
        .with(|slot| slot.borrow().clone())
        .unwrap_or_else(loader::current)
}

pub(crate) fn current() -> Arc<Index> {
    ready().unwrap_or_else(fallback)
}

static WHOLE_PREWARM_REQUESTED: AtomicBool = AtomicBool::new(false);

/// Enabling whole-reading composition warms the current dictionary and each later generation.
pub(crate) fn prewarm_whole() {
    WHOLE_PREWARM_REQUESTED.store(true, Ordering::Relaxed);
    if let Some(index) = ready() { prewarm_whole_index(index); }
}

pub(super) fn prewarm_loaded_whole(index: Arc<Index>) {
    if WHOLE_PREWARM_REQUESTED.load(Ordering::Relaxed)
        || std::env::var("ZHIZI_ZHIXU_WHOLE").is_ok_and(|value| value == "1") {
        prewarm_whole_index(index);
    }
}

fn prewarm_whole_index(index: Arc<Index>) {
    if index.whole_prewarm_started.swap(true, Ordering::Relaxed) { return; }
    let worker_index = index.clone();
    if std::thread::Builder::new().name("zhixu-whole-prewarm".into())
        .spawn(move || {
            worker_index.warm_whole();
            // A fresh build publishes its cache file only after the warm
            // structures it must contain are ready.
            index_cache::run_scheduled(&worker_index);
        }).is_err() {
        index.whole_prewarm_started.store(false, Ordering::Relaxed);
    }
}

/// ASCII checks must not initialize the 69k-word fallback on an ordinary first key.
pub(crate) fn terms() -> Arc<Index> {
    ready().or_else(loader::early_terms).unwrap_or_else(|| {
        static EMPTY: OnceLock<Arc<Index>> = OnceLock::new();
        EMPTY
            .get_or_init(|| {
                #[allow(unused_mut)]
                let mut data = DictionaryData::default();
                #[cfg(test)]
                add_ascii_fixture(&mut data);
                Arc::new(Index::from_data(data))
            })
            .clone()
    })
}

pub(crate) fn generation() -> u64 {
    ready().map_or(0, |index| index.generation)
}

pub(crate) struct SnapshotGuard(Option<Option<Arc<Index>>>);
impl Drop for SnapshotGuard {
    fn drop(&mut self) {
        PINNED.with(|slot| {
            *slot.borrow_mut() = self.0.take();
        });
    }
}

/// Pins either the ready generation or the fallback state without building it.
pub(crate) fn pin_snapshot(index: Option<Arc<Index>>) -> SnapshotGuard {
    SnapshotGuard(PINNED.with(|slot| slot.replace(Some(index))))
}

#[cfg(test)]
pub(crate) fn with_index<T>(index: Arc<Index>, f: impl FnOnce() -> T) -> T {
    let _guard = SnapshotGuard(PINNED.with(|slot| slot.replace(Some(Some(index)))));
    f()
}

#[cfg(test)]
fn add_ascii_fixture(data: &mut DictionaryData) {
    for line in include_str!("../../tests/fixtures/zhixu_ascii_fixture.tsv")
        .lines()
        .filter(|line| !line.starts_with('#'))
    {
        let mut c = line.split('\t');
        if let (Some(layer), Some(text), Some(reading), Some(weight)) =
            (c.next(), c.next(), c.next(), c.next())
        {
            data.entries.push(Entry {
                layer: layer.into(),
                text: text.into(),
                reading: reading.into(),
                weight: weight.parse().unwrap(),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prewarm_fixture(text: &str) -> Arc<Index> {
        Arc::new(Index::from_data(DictionaryData { entries: vec![Entry {
            text: text.into(), reading: "ni hao".into(), weight: 100, layer: "test".into(),
        }] }))
    }

    #[test]
    fn whole_prewarm_finishes_every_index_before_publication() {
        let index = prewarm_fixture("你好");
        assert!(index.whole_readings.get().is_none());
        index.warm_whole();
        let whole = index.whole_readings.get().expect("prewarm builds complete readings");
        assert!(whole.trie.get().is_some());
        assert!(whole.metadata.get().is_some());
        assert_eq!(index.whole_words_for_compact_reading("nihao").next().unwrap().text, "你好");
    }

    #[test]
    fn whole_prewarm_old_generation_never_populates_the_replacement() {
        let old = prewarm_fixture("你好");
        let replacement = prewarm_fixture("拟好");
        let (started, ready) = std::sync::mpsc::channel();
        let (resume, wait) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            started.send(()).unwrap();
            wait.recv().unwrap();
            old.warm_whole();
            old
        });
        ready.recv().unwrap();
        // A key in the replacement generation is correct while the previous worker is pending.
        assert!(replacement.whole_readings.get().is_none());
        assert_eq!(replacement.whole_words_for_compact_reading("nihao").next().unwrap().text, "拟好");
        resume.send(()).unwrap();
        let old = worker.join().unwrap();
        assert_ne!(old.generation, replacement.generation);
        assert!(old.whole_readings.get().unwrap().trie.get().is_some());
        assert_eq!(replacement.whole_words_for_compact_reading("nihao").next().unwrap().text, "拟好");
    }

    #[test]
    fn whole_prewarm_pending_keeps_lookups_correct_without_waiting() {
        let index = prewarm_fixture("你好");
        let worker_index = index.clone();
        let (started, ready) = std::sync::mpsc::channel();
        let (resume, wait) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            worker_index.whole_prewarm_started.store(true, Ordering::Relaxed);
            started.send(()).unwrap();
            wait.recv().unwrap();
            worker_index.warm_whole();
        });
        ready.recv().unwrap();
        assert!(index.whole_readings.get().is_none());
        let words = index.whole_words_for_compact_reading("nihao").collect::<Vec<_>>();
        assert_eq!(words[0].text, "你好");
        let mut prefixes = Vec::new();
        index.visit_whole_words_for_prefixes("nihao", 0, |end, word| prefixes.push((end, word)));
        assert_eq!(prefixes, [(5, words[0])]);
        resume.send(()).unwrap();
        worker.join().unwrap();
        assert_eq!(index.whole_words_for_compact_reading("nihao").collect::<Vec<_>>(), words);
    }

    #[test]
    fn whole_prewarm_background_thread_completes_once_per_generation() {
        let index = prewarm_fixture("你好");
        prewarm_whole_index(index.clone());
        prewarm_whole_index(index.clone());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !index.whole_readings.get().is_some_and(|whole| whole.trie.get().is_some() && whole.metadata.get().is_some()) {
            assert!(std::time::Instant::now() < deadline, "background prewarm timed out");
            std::thread::yield_now();
        }
        assert!(index.whole_prewarm_started.load(Ordering::Relaxed));
        assert_eq!(index.whole_words_for_compact_reading("nihao").next().unwrap().text, "你好");
    }

    fn reference_whole_word<'a>((text, reading, weight): (&'a str, &'a str, u32)) -> Option<WholeWord<'a>> {
        (learning::normalize_reading(reading).is_some()
            && text.chars().all(|c| ('\u{3400}'..='\u{9fff}').contains(&c))).then(|| WholeWord {
                text, reading, weight, syllables: reading.split_whitespace().count(),
                reading_hash: reading.bytes().fold(0xcbf2_9ce4_8422_2325u64,
                    |hash, byte| (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)),
                lexical_score: ((weight.max(1) as f64).ln() * 1_000.0).round() as i64,
            })
    }

    #[test]
    fn whole_presorted_homophones_match_original_sort_on_fallback() {
        let index = fallback();
        let whole = index.whole_readings();
        for id in 0..whole.readings.spans.len() {
            let reading = whole.readings.get(id as u32);
            let words: Vec<_> = index.whole_words_for_compact_reading(reading).collect();
            let maximum = words.iter().map(|word| word.syllables).max().unwrap_or(0);
            for count in 0..=maximum + 1 {
                let mut expected = words.clone();
                expected.sort_by(|a, b| (b.syllables == count).cmp(&(a.syllables == count))
                    .then(b.weight.cmp(&a.weight)).then(a.text.cmp(b.text)));
                assert_eq!(index.ranked_whole_words_for_compact_reading(reading, count).collect::<Vec<_>>(), expected,
                    "reading={reading} count={count}");
            }
        }
    }

    #[test]
    fn whole_metadata_matches_filtered_complete_lookup_on_fallback() {
        let index = fallback();
        let mut readings: Vec<_> = (0..index.readings.spans.len())
            .map(|id| index.readings.get(id as u32).replace(' ', ""))
            .collect();
        readings.sort_unstable();
        readings.dedup();
        for reading in &readings {
            let expected = index.all_words_for_compact_reading(reading)
                .filter_map(reference_whole_word).collect::<Vec<_>>();
            assert_eq!(index.whole_words_for_compact_reading(reading).collect::<Vec<_>>(), expected, "{reading}");
            let mut actual = Vec::new();
            index.visit_whole_words_for_prefixes(reading, 0, |end, word| actual.push((end, word)));
            let mut expected = Vec::new();
            for (offset, letter) in reading.char_indices() {
                if !letter.is_ascii_lowercase() { break; }
                let end = offset + 1;
                expected.extend(index.all_words_for_compact_reading(&reading[..end])
                    .filter_map(reference_whole_word).map(|word| (end, word)));
            }
            assert_eq!(actual, expected, "{reading}");
        }
        let metadata = index.whole_readings().metadata.get().unwrap();
        println!("WHOLE_METADATA\tentries={}\tbytes={}", metadata.len(),
            metadata.capacity() * std::mem::size_of::<WholeWordMetadata>());
    }

    #[test]
    fn whole_metadata_is_lazy_and_keeps_original_fields_and_filters() {
        let index = Index::from_data(DictionaryData { entries: [
            ("甲", "jia", 0), ("你", "ni", 10), ("你好", "ni hao", 10),
            ("\u{3400}", "ni", 1), ("\u{9fff}", "hao", 2), ("\u{f900}", "hao", 3),
            ("ASCII", "ni", 4), ("错", "notpinyin", 5), ("空", "", 6),
            ("组", " XI AN ", 7), ("先", "xi'an", 8),
        ].into_iter().map(|(text, reading, weight)| Entry {
            text: text.into(), reading: reading.into(), weight, layer: "test".into(),
        }).collect() });
        let _ = index.words_for_reading("jia").count();
        assert!(index.whole_readings.get().is_none());
        let _ = index.all_words_for_compact_reading("jia").count();
        index.visit_words_for_prefixes("nihao", |_, _, _, _| {});
        assert!(index.whole_readings().metadata.get().is_none());
        let before = index.estimated_bytes();
        for reading in ["jia", "ni", "hao", "nihao", "notpinyin", "", "XIAN", "xi'an"] {
            assert_eq!(index.whole_words_for_compact_reading(reading).collect::<Vec<_>>(),
                index.all_words_for_compact_reading(reading).filter_map(reference_whole_word).collect::<Vec<_>>());
        }
        assert_eq!(index.whole_words_for_compact_reading("jia").next().unwrap().lexical_score, 0);
        assert_eq!(index.whole_words_for_compact_reading("xi'an").next().unwrap().syllables, 1);
        let mut actual = Vec::new();
        index.visit_whole_words_for_prefixes("ni'hao", 0, |end, word| actual.push((end, word)));
        let mut expected = Vec::new();
        for end in (1..=6).filter(|&end| end != 3) {
            expected.extend(index.all_words_for_compact_reading(&"ni'hao"[..end].replace('\'', ""))
                .filter_map(reference_whole_word).map(|word| (end, word)));
        }
        assert_eq!(actual, expected);
        let metadata = index.whole_readings().metadata.get().unwrap();
        assert_eq!(index.estimated_bytes() - before,
            metadata.capacity() * std::mem::size_of::<WholeWordMetadata>());
    }

    fn check_whole_prefixes(index: &Index, reading: &str) {
        let mut expected = Vec::new();
        for (end, letter) in reading.char_indices() {
            if letter == '\'' { continue; }
            if !letter.is_ascii_lowercase() { break; }
            let end = end + 1;
            let compact = reading[..end].replace('\'', "");
            expected.extend(index.all_words_for_compact_reading(&compact)
                .map(|(text, spaced, weight)| (end, text, spaced, weight)));
        }
        let mut actual = Vec::new();
        index.visit_words_for_prefixes(reading,
            |end, text, spaced, weight| actual.push((end, text, spaced, weight)));
        assert_eq!(actual, expected, "{reading}");
    }

    #[test]
    fn whole_prefix_trie_matches_complete_lookup_on_fallback() {
        let index = fallback();
        let mut readings: Vec<_> = (0..index.readings.spans.len())
            .map(|id| index.readings.get(id as u32).replace(' ', ""))
            .collect();
        readings.sort_unstable();
        readings.dedup();
        for (position, reading) in readings.iter().enumerate() {
            check_whole_prefixes(&index, reading);
            if position % 997 == 0 {
                let explicit = reading.chars().map(|letter| letter.to_string())
                    .collect::<Vec<_>>().join("'");
                check_whole_prefixes(&index, &explicit);
            }
        }
        for reading in ["", "'", "''", "xian", "xi'an", "yi'jian'", "yijian!", "yijian中", "YIJIAN", "zzzzzz"] {
            check_whole_prefixes(&index, reading);
        }
        println!("WHOLE_PREFIX_TRIE\treadings={}\tbytes={}", readings.len(),
            index.whole_readings().trie.get().unwrap().estimated_bytes());
    }

    #[test]
    fn whole_prefix_trie_is_lazy_and_keeps_rare_long_and_ambiguous_entries() {
        let index = Index::from_data(DictionaryData { entries: [
            ("冷门", "leng men", 499),
            ("一二三四五六七八", "yi er san si wu liu qi ba", 999),
            ("西安", "xi an", 900), ("先", "xian", 900),
            ("鲜", "xian", 900), ("先", "xi an", 901),
            ("非字母", "bad!", 999), ("空", "", 999),
        ].into_iter().map(|(text, reading, weight)| Entry {
            text: text.into(), reading: reading.into(), weight, layer: "pinyin_simp".into(),
        }).collect() });
        assert!(index.whole_readings.get().is_none());
        let _ = index.all_words_for_compact_reading("xian").count();
        assert!(index.whole_readings().trie.get().is_none());
        let before = index.estimated_bytes();
        for reading in ["xian", "xi'an", "lengmen", "yiersansiwuliuqiba", "bad!", "", "空"] {
            check_whole_prefixes(&index, reading);
        }
        assert!(index.whole_readings().trie.get().is_some());
        assert!(index.estimated_bytes() > before);
    }

    #[test]
    fn whole_reading_lookup_keeps_all_homophones_and_ambiguous_boundaries() {
        let mut entries = [
            ("冷门", "leng men", 499, "pinyin_simp"),
            ("一二三四五六七八", "yi er san si wu liu qi ba", 999, "agent_spoken"),
            ("西安", "xi an", 900, "test"),
            ("先", "xian", 1_000, "test"),
        ].into_iter().map(|(text, reading, weight, layer)| Entry {
            text: text.into(), reading: reading.into(), weight, layer: layer.into(),
        }).collect::<Vec<_>>();
        for (position, text) in ["意见", "一件", "一见", "异见", "衣件", "仪检", "一键"].into_iter().enumerate() {
            entries.push(Entry { text: text.into(), reading: "yi jian".into(),
                weight: 1_000 - position as u32, layer: "test".into() });
        }
        let index = Index::from_data(DictionaryData { entries });
        assert!(index.whole_readings.get().is_none());
        let bytes_before = index.estimated_bytes();
        assert_eq!(index.all_words_for_compact_reading("lengmen").collect::<Vec<_>>(), [("冷门", "leng men", 499)]);
        assert!(index.whole_readings.get().is_some());
        assert!(index.estimated_bytes() > bytes_before);
        assert_eq!(index.all_words_for_compact_reading("yiersansiwuliuqiba").collect::<Vec<_>>(),
            [("一二三四五六七八", "yi er san si wu liu qi ba", 999)]);
        assert_eq!(index.all_words_for_compact_reading("xian").collect::<Vec<_>>(),
            [("先", "xian", 1_000), ("西安", "xi an", 900)]);
        assert_eq!(index.all_words_for_compact_reading("yijian").map(|word| word.0).collect::<Vec<_>>(),
            ["意见", "一件", "一见", "异见", "衣件", "仪检", "一键"]);
        assert!(index.all_words_for_compact_reading("yij").next().is_none());
        assert!(index.all_words_for_compact_reading("").next().is_none());
        assert!(index.words_for_reading("leng men").next().is_none());
    }

    #[test]
    fn homophones_and_initials_share_the_reading_storage() {
        let index = Index::from_data(DictionaryData {
            entries: ["你", "泥"]
                .into_iter()
                .map(|text| Entry {
                    text: text.into(),
                    reading: "ni".into(),
                    weight: 800,
                    layer: "test".into(),
                })
                .collect(),
        });
        let reading = index.readings_for_text("你").next().unwrap();
        assert!(std::ptr::eq(
            reading.as_ptr(),
            index.readings_for_text("泥").next().unwrap().as_ptr()
        ));
        for (_, initial_reading, _) in index.words_for_initials("n") {
            assert!(std::ptr::eq(reading.as_ptr(), initial_reading.as_ptr()));
        }
    }

    #[test]
    fn complete_index_retains_low_weight_and_long_words_outside_decoder_projection() {
        let entries = [
            ("冷门", "leng men", 499, "pinyin_simp"),
            ("日常", "ri chang", 1, "agent_spoken"),
            (
                "一二三四五六七八",
                "yi er san si wu liu qi ba",
                999,
                "agent_spoken",
            ),
        ]
        .into_iter()
        .map(|(text, reading, weight, layer)| Entry {
            text: text.into(),
            reading: reading.into(),
            weight,
            layer: layer.into(),
        })
        .collect();
        let index = Index::from_data(DictionaryData { entries });
        assert_eq!(index.entry_count(), 3);
        assert_eq!(index.entry_words("leng men")[0].weight, 499);
        assert!(index.words_for_reading("leng men").next().is_none());
        assert!(
            index
                .words_for_reading("yi er san si wu liu qi ba")
                .next()
                .is_none()
        );
        assert_eq!(
            index.words_for_reading("ri chang").next().unwrap().0,
            "日常"
        );
    }
}
