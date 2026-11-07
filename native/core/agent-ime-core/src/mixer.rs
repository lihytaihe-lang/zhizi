use crate::blacklist::Store;
use crate::english_choices;
use crate::personal_picks;
use crate::ranking::{self, EditKind};
use crate::zhixu::{self, Index};
use crate::zhixu::learning::{Choice, LearningMode};
use crate::zhixu::learning_session::LearningSession;
use crate::zhixu::trie::HeavyTerminal;

use crate::context_model::ContextModel;
use crate::state::{BoundaryPurpose, Candidate, CompositionBoundary, Decoder, DecoderSnapshot, LearningIntent};
use serde::Deserialize;
use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex, OnceLock};
use unicode_normalization::UnicodeNormalization;
#[cfg(test)]
#[path = "zhixu/benchmarks.rs"]
mod runtime_dictionary_tests;

#[cfg(test)]
#[path = "zhixu/mixer_tests.rs"]
mod runtime_dictionary_unit_tests;
#[cfg(test)]
#[path = "../../../host/probes/decoder_contract.rs"]
mod decoder_contract;
include!("zhixu/learning_readings.rs");


pub const PAGE_SIZE: usize = 5;
pub const MAX_SELECTION_PROMOTION: usize = 3;
#[cfg(test)]
thread_local! {
    static WHOLE_TIMING: std::cell::RefCell<[(usize, u128); 6]> = const { std::cell::RefCell::new([(0, 0); 6]) };
    static WHOLE_MIXER_TIMING: std::cell::RefCell<[u128; 13]> = const { std::cell::RefCell::new([0; 13]) };
    static WHOLE_REUSED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static WHOLE_GENERATION_CONTINUATIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static WHOLE_LEARNING_TIMING: std::cell::Cell<u128> = const { std::cell::Cell::new(0) };
    static WHOLE_LEARNING_DEPTH: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) struct WholeLearningTiming(Option<std::time::Instant>);

#[cfg(test)]
impl WholeLearningTiming {
    pub(crate) fn start() -> Self {
        Self(WHOLE_LEARNING_DEPTH.with(|depth| {
            let outer = depth.get() == 0;
            depth.set(depth.get() + 1);
            outer.then(std::time::Instant::now)
        }))
    }
}

#[cfg(test)]
impl Drop for WholeLearningTiming {
    fn drop(&mut self) {
        WHOLE_LEARNING_DEPTH.with(|depth| depth.set(depth.get() - 1));
        if let Some(start) = self.0 {
            WHOLE_LEARNING_TIMING.with(|timing| timing.set(timing.get() + start.elapsed().as_nanos()));
        }
    }
}

#[cfg(test)]
struct WholeTiming(usize, std::time::Instant);

#[cfg(test)]
impl WholeTiming {
    fn start(slot: usize) -> Self { Self(slot, std::time::Instant::now()) }
}

#[cfg(test)]
impl Drop for WholeTiming {
    fn drop(&mut self) {
        WHOLE_TIMING.with(|timing| {
            let mut timing = timing.borrow_mut();
            timing[self.0].0 += 1;
            timing[self.0].1 += self.1.elapsed().as_nanos();
        });
    }
}

#[cfg(test)]
struct WholeMixerTiming(usize, std::time::Instant);

#[cfg(test)]
impl WholeMixerTiming {
    fn start(slot: usize) -> Self { Self(slot, std::time::Instant::now()) }
}

#[cfg(test)]
impl Drop for WholeMixerTiming {
    fn drop(&mut self) {
        WHOLE_MIXER_TIMING.with(|timing| timing.borrow_mut()[self.0] += self.1.elapsed().as_nanos());
    }
}

const MAX_FUZZY_EXACT_PROTECTION: usize = 4;
const LONG_PHRASE_MIN_SYLLABLES: usize = 3;
const LONG_PHRASE_MAX_NONEXACT_SYLLABLES: usize = 1;
const HEAVY_GARBLE_BEAM_PER_POSITION: usize = 8;
const HEAVY_GARBLE_MAX_WORDS: u8 = 5;
const HEAVY_GARBLE_MAX_TOTAL_EDITS: u8 = 2;
const HEAVY_GARBLE_MAX_WORD_EDITS: u8 = 1;
const HEAVY_GARBLE_MAX_CANDIDATES: usize = 2;
pub(crate) const PINYIN_SYLLABLES: &str = "
a ai an ang ao
ba bai ban bang bao bei ben beng bi bian biao bie bin bing bo bu
ca cai can cang cao ce cen ceng cha chai chan chang chao che chen cheng chi chong chou chu chua chuai chuan chuang chui chun chuo ci cong cou cu cuan cui cun cuo
da dai dan dang dao de dei den deng di dia dian diao die ding diu dong dou du duan dui dun duo
e ei en eng er
fa fan fang fei fen feng fo fou fu
ga gai gan gang gao ge gei gen geng gong gou gu gua guai guan guang gui gun guo
ha hai han hang hao he hei hen heng hong hou hu hua huai huan huang hui hun huo
ji jia jian jiang jiao jie jin jing jiong jiu ju juan jue jun
ka kai kan kang kao ke kei ken keng kong kou ku kua kuai kuan kuang kui kun kuo
la lai lan lang lao le lei leng li lia lian liang liao lie lin ling liu long lou lu luan lun luo lv lve
ma mai man mang mao me mei men meng mi mian miao mie min ming miu mo mou mu
na nai nan nang nao ne nei nen neng ni nian niang niao nie nin ning niu nong nou nu nuan nuo nv nve
o ou
pa pai pan pang pao pei pen peng pi pian piao pie pin ping po pou pu
qi qia qian qiang qiao qie qin qing qiong qiu qu quan que qun
ran rang rao re ren reng ri rong rou ru ruan rui run ruo
sa sai san sang sao se sen seng sha shai shan shang shao she shei shen sheng shi shou shu shua shuai shuan shuang shui shun shuo si song sou su suan sui sun suo
ta tai tan tang tao te teng ti tian tiao tie ting tong tou tu tuan tui tun tuo
wa wai wan wang wei wen weng wo wu
xi xia xian xiang xiao xie xin xing xiong xiu xu xuan xue xun
ya yan yang yao ye yi yin ying yo yong you yu yuan yue yun
za zai zan zang zao ze zei zen zeng zha zhai zhan zhang zhao zhe zhei zhen zheng zhi zhong zhou zhu zhua zhuai zhuan zhuang zhui zhun zhuo zi zong zou zu zuan zui zun zuo
";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LexiconSource {
    pub source_kind: String,
    pub source: String,
    pub license: String,
    pub confidence_millis: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LexiconEntry {
    pub id: String,
    pub text: String,
    pub pinyin: String,
    pub aliases: Vec<String>,
    pub professional_packs: Vec<String>,
    pub project_domains: Vec<String>,
    pub sources: Vec<LexiconSource>,
    pub base_weight: i64,
    pub fixed: bool,
    pub disabled: bool,
    #[serde(default)]
    pub demoted: bool,
    #[serde(default = "default_candidate_layer")]
    pub candidate_layer: String,
    pub updated_at_ms: i64,
}

fn default_candidate_layer() -> String {
    "first".into()
}

fn is_personal_auto_prediction_entry(entry: &LexiconEntry) -> bool {
    entry.sources.iter().any(|source| {
        matches!(
            source.source_kind.as_str(),
            "personal-friction-auto" | "owner-declared-disambiguation"
        )
    })
}

fn is_owner_declared_disambiguation(entry: &LexiconEntry) -> bool {
    entry
        .sources
        .iter()
        .any(|source| source.source_kind == "owner-declared-disambiguation")
}

#[derive(Clone, Debug, Default)]
pub struct MixerLexicon {
    entries: Vec<LexiconEntry>,
    blocked_texts: HashSet<String>,
    /// Entry indices (ascending) keyed by canonical pinyin — replaces the
    /// per-keystroke linear scan over the whole lexicon.
    indices_by_pinyin: HashMap<String, Vec<usize>>,
    /// Ascending indices of `fixed` entries; fixed entries also match by
    /// reading prefix, which a hash lookup cannot serve. Bounded by how many
    /// entries a user pins, so a linear pass here stays cheap.
    fixed_indices: Vec<usize>,
    /// Ascending indices of `candidate_layer == "secondary"` entries; the
    /// prediction layer prefix-matches over these only.
    secondary_indices: Vec<usize>,
    /// Owner choices share the existing personal-profile lexicon and are only
    /// consulted when the current reading contains the declared reading.
    owner_declared_indices: Vec<usize>,
}

impl MixerLexicon {
    pub fn from_json(bytes: &[u8]) -> Result<Self, i32> {
        const MAXIMUM_BYTES: usize = 16 * 1024 * 1024;
        const MAXIMUM_ENTRIES: usize = 100_000;
        if bytes.len() > MAXIMUM_BYTES {
            return Err(-201);
        }
        let mut entries: Vec<LexiconEntry> = serde_json::from_slice(bytes).map_err(|_| -202)?;
        if entries.len() > MAXIMUM_ENTRIES {
            return Err(-201);
        }
        let mut ids = HashSet::new();
        let mut keys = HashSet::new();
        for entry in &mut entries {
            entry.id = canonical_nonempty(&entry.id).ok_or(-203)?;
            entry.text = canonical_nonempty(&entry.text).ok_or(-203)?;
            entry.pinyin = canonical_reading(&entry.pinyin).ok_or(-203)?;
            if !ids.insert(entry.id.clone())
                || !keys.insert((normalize_text(&entry.text), entry.pinyin.clone()))
                || entry.sources.is_empty()
                || !matches!(entry.candidate_layer.as_str(), "first" | "secondary")
                || entry.updated_at_ms < 0
            {
                return Err(-203);
            }
            for source in &entry.sources {
                if canonical_nonempty(&source.source_kind).is_none()
                    || canonical_nonempty(&source.source).is_none()
                    || canonical_nonempty(&source.license).is_none()
                    || !(0..=1000).contains(&source.confidence_millis)
                {
                    return Err(-203);
                }
            }
        }
        entries.sort_by(|left, right| {
            (&left.text, &left.pinyin, &left.id).cmp(&(&right.text, &right.pinyin, &right.id))
        });
        let blocked_texts = entries
            .iter()
            .filter(|entry| entry.disabled)
            .map(|entry| normalize_text(&entry.text))
            .collect();
        let mut indices_by_pinyin: HashMap<String, Vec<usize>> = HashMap::new();
        let mut fixed_indices = Vec::new();
        let mut secondary_indices = Vec::new();
        let mut owner_declared_indices = Vec::new();
        for (index, entry) in entries.iter().enumerate() {
            indices_by_pinyin
                .entry(entry.pinyin.clone())
                .or_default()
                .push(index);
            if entry.fixed {
                fixed_indices.push(index);
            }
            if entry.candidate_layer == "secondary" || is_personal_auto_prediction_entry(entry) {
                secondary_indices.push(index);
            }
            if is_owner_declared_disambiguation(entry) {
                owner_declared_indices.push(index);
            }
        }
        Ok(Self {
            entries,
            blocked_texts,
            indices_by_pinyin,
            fixed_indices,
            secondary_indices,
            owner_declared_indices,
        })
    }

    /// Ascending entry indices matching the original full-scan visit order for
    /// the first layer: exact pinyin hits plus fixed-entry prefix hits.
    fn first_layer_indices(&self, reading: &str) -> Vec<usize> {
        let mut matched = self
            .indices_by_pinyin
            .get(reading)
            .cloned()
            .unwrap_or_default();
        if !reading.is_empty() {
            for &index in &self.fixed_indices {
                let entry = &self.entries[index];
                if entry.pinyin != reading && entry.pinyin.starts_with(reading) {
                    matched.push(index);
                }
            }
        }
        matched.sort_unstable();
        matched
    }
}

#[derive(Clone, Debug)]
enum Selection {
    Base { index: usize, text: String },
    GlobalBase { index: usize, text: String },
    Local { text: String },
}

#[derive(Clone, Debug)]
struct MixedCandidate {
    candidate: Candidate,
    selection: Selection,
    tier: u8,
    score: i64,
}

#[derive(Clone, Copy, Debug)]
struct HeavyWordMatch {
    raw_end: usize,
    node: usize,
    terminal: HeavyTerminal,
    edits: u8,
    omitted_letters: u8,
    deletions: u8,
}

#[derive(Clone, Debug, PartialEq)]
struct HeavyPath {
    score: i64,
    text: String,
    words: u8,
    edits: u8,
    edit_units: f64,
    nodes: [usize; HEAVY_GARBLE_MAX_WORDS as usize],
}

/// Upper bound on learned promotions. Eviction is deterministic FIFO so ranking stays reproducible.
const MAX_LEARNED_PROMOTIONS: usize = personal_picks::MAX_ENTRIES;
/// A first pick moves a candidate up at most MAX_SELECTION_PROMOTION places; the same pick made this many times puts it first.
const CONFIRMED_PICKS: u32 = 3;

type LearnedKey = (String, String, String);

/// The deliberate picks, per (app, input, text). They are kept in the Rime user directory once the mixer
/// knows it (`attach`), so they survive restarts, and fade after `personal_picks::MAX_AGE_MS` without a repeat
/// (they used to be forgotten after 128 other picks: a day's typing).
#[derive(Clone, Debug)]
struct SelectionUndo {
    key: LearnedKey,
    written_ms: u64,
    // Include expiry/FIFO removals so a record/withdraw pair preserves the entry set.
    previous: Vec<(LearnedKey, Option<(usize, u32, u64)>)>,
    insertion_order: VecDeque<LearnedKey>,
}

#[derive(Clone, Debug, Default)]
struct LearningState {
    target_positions: HashMap<LearnedKey, usize>,
    counts: HashMap<LearnedKey, u32>,
    last_ms: HashMap<LearnedKey, u64>,
    insertion_order: VecDeque<LearnedKey>,
    directory: Option<std::path::PathBuf>,
    undo: Option<SelectionUndo>,
}

impl LearningState {
    fn record(&mut self, key: LearnedKey, target: usize) {
        self.record_at(key, target, personal_picks::now_ms());
    }

    fn record_at(&mut self, key: LearnedKey, target: usize, now_ms: u64) {
        self.undo = None;
        let previous = self.entry(&key);
        self.undo = Some(SelectionUndo {
            key: key.clone(), written_ms: now_ms,
            previous: vec![(key.clone(), previous)],
            insertion_order: self.insertion_order.clone(),
        });
        self.expire(now_ms);
        let count = self.counts.get(&key).copied().unwrap_or(0).saturating_add(1);
        let target = if count >= CONFIRMED_PICKS { 0 } else { target };
        self.counts.insert(key.clone(), count);
        self.last_ms.insert(key.clone(), now_ms);
        if let Some(prior) = self.target_positions.get_mut(&key) {
            *prior = (*prior).min(target);
        } else {
            self.target_positions.insert(key.clone(), target);
            self.insertion_order.push_back(key.clone());
            while self.insertion_order.len() > MAX_LEARNED_PROMOTIONS {
                if let Some(evicted) = self.insertion_order.pop_front() {
                    self.drop_key(&evicted);
                }
            }
        }
        if !self.persist(&[]) { self.undo = None; }
    }

    fn forget(&mut self, key: &LearnedKey) {
        self.undo = None;
        self.drop_key(key);
        self.insertion_order.retain(|item| item != key);
        self.persist(std::slice::from_ref(key));
    }

    fn entry(&self, key: &LearnedKey) -> Option<(usize, u32, u64)> {
        Some((*self.target_positions.get(key)?, *self.counts.get(key)?, *self.last_ms.get(key)?))
    }

    fn withdraw(&mut self, key: &LearnedKey) -> bool {
        let Some(undo) = &self.undo else { return false };
        if &undo.key != key || self.last_ms.get(key) != Some(&undo.written_ms) {
            return false;
        }
        // Do not overwrite a later update made by another process.
        if self.directory.as_ref().is_some_and(|directory| personal_picks::load(directory)
            .iter().any(|e| (&e.app, &e.reading, &e.text) == (&key.0, &key.1, &key.2)
                && e.last_ms > undo.written_ms)) {
            self.undo = None;
            return false;
        }
        let undo = self.undo.take().unwrap();
        let changed: Vec<_> = undo.previous.iter().map(|(key, _)| key.clone()).collect();
        for (key, previous) in undo.previous {
            self.drop_key(&key);
            if let Some((target, count, last)) = previous {
                self.target_positions.insert(key.clone(), target);
                self.counts.insert(key.clone(), count);
                self.last_ms.insert(key, last);
            }
        }
        self.insertion_order = undo.insertion_order;
        // Remove the just-written version before merging the restored older snapshot.
        self.persist(&changed)
    }

    fn drop_key(&mut self, key: &LearnedKey) {
        let previous = self.entry(key);
        if let Some(undo) = &mut self.undo {
            if !undo.previous.iter().any(|(saved, _)| saved == key) {
                undo.previous.push((key.clone(), previous));
            }
        }
        self.target_positions.remove(key);
        self.counts.remove(key);
        self.last_ms.remove(key);
    }

    /// Picks that were not repeated for 45 days are forgotten.
    fn expire(&mut self, now_ms: u64) {
        let stale = self
            .last_ms
            .iter()
            .filter(|(_, last)| now_ms.saturating_sub(**last) > personal_picks::MAX_AGE_MS)
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        for key in stale {
            self.drop_key(&key);
            self.insertion_order.retain(|item| item != &key);
        }
    }

    /// Reads the picks kept in `directory` (once per process and directory) and keeps writing there.
    fn attach(&mut self, directory: &std::path::Path) {
        if self.directory.as_deref() == Some(directory) {
            return;
        }
        self.undo = None;
        self.directory = Some(directory.to_path_buf());
        let now = personal_picks::now_ms();
        for entry in personal_picks::load(directory) {
            if now.saturating_sub(entry.last_ms) > personal_picks::MAX_AGE_MS {
                continue;
            }
            let key = (entry.app, entry.reading, entry.text);
            let newer = self.last_ms.get(&key).is_none_or(|last| *last < entry.last_ms);
            if newer {
                self.counts.insert(key.clone(), entry.count);
                self.last_ms.insert(key.clone(), entry.last_ms);
                if self.target_positions.insert(key.clone(), entry.target).is_none() {
                    self.insertion_order.push_back(key);
                }
            }
        }
    }

    /// Writes the file: what other processes added is kept (the file is read again and merged), `forgotten` is not.
    fn persist(&self, forgotten: &[LearnedKey]) -> bool {
        let Some(directory) = &self.directory else {
            return true;
        };
        let now = personal_picks::now_ms();
        let mut merged: HashMap<LearnedKey, personal_picks::Entry> = HashMap::new();
        for entry in personal_picks::load(directory) {
            if now.saturating_sub(entry.last_ms) <= personal_picks::MAX_AGE_MS {
                merged.insert((entry.app.clone(), entry.reading.clone(), entry.text.clone()), entry);
            }
        }
        for key in forgotten {
            merged.remove(key);
        }
        for (key, target) in &self.target_positions {
            let (count, last_ms) = (self.counts.get(key).copied().unwrap_or(1), self.last_ms.get(key).copied().unwrap_or(now));
            let fresher = merged.get(key).is_none_or(|existing| existing.last_ms <= last_ms);
            if fresher {
                merged.insert(
                    key.clone(),
                    personal_picks::Entry {
                        app: key.0.clone(),
                        reading: key.1.clone(),
                        text: key.2.clone(),
                        target: *target,
                        count,
                        last_ms,
                    },
                );
            }
        }
        let mut entries = merged.into_values().collect::<Vec<_>>();
        entries.sort_by(|a, b| b.last_ms.cmp(&a.last_ms).then_with(|| a.text.cmp(&b.text)));
        entries.truncate(MAX_LEARNED_PROMOTIONS);
        personal_picks::save(directory, &entries).is_ok()
    }
}

#[derive(Clone, Debug)]
struct ProjectAbbreviation {
    text: String,
    reading: String,
    scope: String,
}

pub(crate) fn withdraw_selection(app: &str, raw: &str, text: &str) -> bool {
    let withdrawn = shared_learning().lock().is_ok_and(|mut learning|
        learning.withdraw(&(app.to_owned(), raw.to_owned(), text.to_owned())));
    if withdrawn { whole_preferences_changed(); }
    withdrawn
}

fn whole_preferences_revision() -> &'static std::sync::atomic::AtomicU64 {
    static REVISION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    &REVISION
}

fn whole_preferences_changed() {
    whole_preferences_revision().fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

fn shared_learning() -> &'static Mutex<LearningState> {
    static STATE: OnceLock<Mutex<LearningState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(LearningState::default()))
}

/// Candidates pushed off a page by something inserted into it (the composition offer, a short
/// English word, a lead) are carried to the start of the next page instead of being lost.
#[derive(Default)]
struct CarryState {
    raw: String,
    /// What page p pushed off, waiting for page p + 1.
    out: HashMap<usize, Vec<MixedCandidate>>,
    /// What the base ranking itself left beyond PAGE_SIZE on page p.
    native: HashMap<usize, Vec<MixedCandidate>>,
}

#[derive(PartialEq)]
struct WholeKey {
    raw: String,
    preedit: String,
    cursor: usize,
    native_page: usize,
    native_last: bool,
    native_ascii: bool,
    dictionary: u64,
    learning: u64,
    preferences: u64,
    mode: LearningMode,
    enabled: bool,
    corrections: ranking::CorrectionSettings,
}

struct WholeRanked {
    lead: Option<(String, String)>,
    offered: Option<(String, String)>,
    candidates: Arc<Vec<MixedCandidate>>,
    confirmed_first: Option<Option<String>>,
}

struct WholeCache {
    key: WholeKey,
    base: Option<Arc<DecoderSnapshot>>,
    generated: Option<Vec<MixedCandidate>>,
    generated_complete: bool,
    generated_certified: bool,
    ranked: Option<WholeRanked>,
    confirmed_first: Option<Option<String>>,
}

type WholePath = (i64, String, u64, usize);

struct WholePathRank<'a>(&'a WholePath, usize);

impl Ord for WholePathRank<'_> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.0.cmp(&other.0.0).then_with(|| other.0.1.cmp(&self.0.1))
            .then_with(|| other.1.cmp(&self.1))
    }
}

impl PartialOrd for WholePathRank<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> { Some(self.cmp(other)) }
}

impl PartialEq for WholePathRank<'_> {
    fn eq(&self, other: &Self) -> bool { self.cmp(other) == Ordering::Equal }
}

impl Eq for WholePathRank<'_> {}

pub(crate) struct WholeLattice {
    reading: String,
    fuzzy: u32,
    dictionary: u64,
    personal: Option<u64>,
    learning: u64,
    tick: u64,
    bonuses: Vec<i64>,
    cap: usize,
    beam: usize,
    paths: Vec<Vec<WholePath>>,
}

pub struct MixerDecoder {
    inner: Box<dyn Decoder>,
    learning: LearningSession,
    lexicon: Arc<MixerLexicon>,
    app_id: String,
    typo_enabled: bool,
    active_project: Option<String>,
    active_packs: HashSet<String>,
    secondary_enabled: bool,
    prediction_enabled: bool,
    code_ascii_bias: bool,
    fuzzy_mask: u32,
    fuzzy_page_no: usize,
    fuzzy_highlighted: usize,
    whole_page_no: usize,
    whole_highlighted: usize,
    #[cfg(test)]
    whole_override: Option<bool>,
    #[cfg(test)]
    whole_eager: bool,
    deep_prefix_keys: HashSet<(String, String)>,
    deep_candidates: Vec<MixedCandidate>,
    deep_page_no: Option<usize>,
    /// The page number the first page of `deep_candidates` is shown as (2 after two pages of words).
    deep_base: usize,
    deep_highlighted: usize,
    typed_ascii: String,
    raw_override: Option<String>,
    context_model: Arc<ContextModel>,
    preceding_text: Option<String>,
    host_order: Option<(String, usize, Vec<Candidate>, Vec<MixedCandidate>)>,
    /// C3: a whole-reading composition the context model chose over the decoder's
    /// first, for this raw input only.
    lead_composition: Option<(String, String)>,
    /// A whole-reading composition offered in the last slot of the first page when none
    /// leads (`ranking::composition_offer`), for this raw input only.
    offered_composition: Option<(String, String)>,
    blacklist: Arc<Mutex<Store>>,
    english_choices: Arc<Mutex<english_choices::Store>>,
    /// The last key's composition alternatives (with edits, and the first's own edits),
    /// keyed by what produced them: the shells ask for them two or three times a key.
    composition_cache: Mutex<Option<(CompositionKey, Vec<(String, f64)>, f64)>>,
    /// Keep a displayed candidate page and its selection mapping on one dictionary
    /// generation. A new edit resets this; background swaps cannot change a digit's target.
    dictionary_snapshot: Mutex<Option<Option<Arc<Index>>>>,
    /// One sequence per edit/dictionary/learning state, independent of the visible page.
    whole_cache: Mutex<Option<WholeCache>>,
    whole_lattice: Mutex<Option<WholeLattice>>,
    carry: Mutex<CarryState>,
}


/// Why `lead_with_composition` refuses (returned through `aim_session_lead_with_composition`; a shell only needs
/// "non-zero means the sentence does not lead", but diagnostics and tests name the reason).
pub const LEAD_NOT_AN_ALTERNATIVE: i32 = -10;
/// The owner's learned whole word stays first (`protect_learned_whole`): a model sentence never displaces what
/// the owner has taught for this reading.
pub const LEAD_OUTRANKED_BY_LEARNED_WHOLE: i32 = -11;
/// Something else is first on the real page.
pub const LEAD_OUTRANKED_OTHER: i32 = -12;

impl MixerDecoder {
    fn pin_dictionary(&self) -> (zhixu::SnapshotGuard, zhixu::LearningSnapshotGuard) {
        #[cfg(test)]
        let _timing = WholeTiming::start(0);
        let snapshot = self.dictionary_snapshot.lock().ok().map(|mut cached| {
            cached.get_or_insert_with(|| self.inner.dictionary_snapshot().or_else(zhixu::ready)).clone()
        }).unwrap_or_else(|| self.inner.dictionary_snapshot().or_else(zhixu::ready));
        if self.learning.mode == LearningMode::Zhixu {
            self.learning.seed_if_needed(&snapshot.clone().unwrap_or_else(zhixu::fallback));
            ranking::set_learning_style(if self.use_rime_order(&self.inner.snapshot().raw_input) {
                None
            } else { self.learning.style() });
        } else {
            ranking::set_learning_style(None);
        }
        (zhixu::pin_snapshot(snapshot), zhixu::pin_learning(
            if self.whole_enabled() { self.learning.whole_overlay() }
            else if self.use_rime_order(&self.inner.snapshot().raw_input) { None } else { self.learning.overlay() }))
    }

    fn has_preceding_context(&self) -> bool {
        self.preceding_text
            .as_deref()
            .is_some_and(|text| !text.trim().is_empty())
    }

    pub fn new(inner: Box<dyn Decoder>, app_id: String) -> Self {
        Self {
            inner,
            learning: LearningSession::default(),
            lexicon: Arc::new(MixerLexicon::default()),
            app_id,
            typo_enabled: false,
            active_project: None,
            active_packs: HashSet::new(),
            secondary_enabled: true,
            prediction_enabled: false,
            code_ascii_bias: false,
            fuzzy_mask: 0,
            fuzzy_page_no: 0,
            fuzzy_highlighted: 0,
            whole_page_no: 0,
            whole_highlighted: 0,
            #[cfg(test)]
            whole_override: None,
            #[cfg(test)]
            whole_eager: false,
            deep_prefix_keys: HashSet::new(),
            deep_candidates: Vec::new(),
            deep_page_no: None,
            deep_base: 2,
            deep_highlighted: 0,
            typed_ascii: String::new(),
            raw_override: None,
            context_model: Arc::new(ContextModel::default()),
            preceding_text: None,
            host_order: None,
            lead_composition: None,
            offered_composition: None,
            blacklist: Arc::new(Mutex::new(Store::default())),
            english_choices: Arc::new(Mutex::new(english_choices::Store::default())),
            composition_cache: Mutex::new(None),
            dictionary_snapshot: Mutex::new(None),
            whole_cache: Mutex::new(None),
            whole_lattice: Mutex::new(None),
            carry: Mutex::new(CarryState::default()),
        }
    }

    /// The product session's permanent inner: the native decoder over one fully
    /// deployed dictionary generation.
    pub(crate) fn new_zhixu_inner(
        dictionary: Arc<Index>,
        blacklist: Option<&Arc<Mutex<Store>>>,
    ) -> Result<Box<dyn Decoder>, i32> {
        let mut decoder = crate::zhixu::decoder::ZhixuDecoder::from_ready_dictionary(dictionary);
        if let Some(directory) = blacklist.and_then(|store| store.lock().ok().and_then(|store| store.directory())) {
            decoder.attach_learning(&directory);
        }
        decoder.configure_learning(LearningMode::Zhixu)?;
        Ok(Box::new(decoder))
    }

    /// The product constructor: the native decoder is the inner for the session's
    /// whole life and learning runs in Zhixu, exactly as the trial switch's on
    /// state behaved.
    pub(crate) fn new_native(
        app_id: String,
        blacklist: Arc<Mutex<Store>>,
        dictionary: Arc<Index>,
    ) -> Result<Self, i32> {
        let inner = Self::new_zhixu_inner(dictionary.clone(), Some(&blacklist))?;
        let mut mixer = Self::new(inner, app_id);
        mixer.apply_blacklist(blacklist);
        mixer.learning.set_mode(LearningMode::Zhixu, &dictionary);
        Ok(mixer)
    }

    pub(crate) fn apply_blacklist(&mut self, blacklist: Arc<Mutex<Store>>) {
        // The owner's English picks and deliberate picks live next to the blacklist in the user directory.
        let directory = blacklist.lock().ok().and_then(|blacklist| blacklist.directory());
        if let Some(store) = directory.as_deref().and_then(|directory| english_choices::shared(directory).ok()) {
            self.english_choices = store;
        }
        if let Some(directory) = &directory
            && let Ok(mut learning) = shared_learning().lock()
        {
            learning.attach(directory);
        }
        if let Some(directory) = &directory {
            crate::lexicon_layers::attach(directory);
            self.learning.attach(directory);
        }
        self.blacklist = blacklist;
        whole_preferences_changed();
        self.invalidate_whole();
    }

    fn apply_owner_declared_ranking(&self, reading: &str, candidates: &mut [MixedCandidate]) {
        let preferences = self
            .lexicon
            .owner_declared_indices
            .iter()
            .map(|&index| &self.lexicon.entries[index])
            .filter(|entry| {
                !entry.disabled
                    && entry.pinyin != reading
                    && reading.ends_with(&entry.pinyin)
                    && entry_applies(entry, self.active_project.as_deref(), &self.active_packs)
            })
            .collect::<Vec<_>>();
        if preferences.is_empty() {
            return;
        }
        candidates.sort_by_key(|candidate| {
            std::cmp::Reverse(
                preferences
                    .iter()
                    .filter(|entry| candidate.candidate.text.contains(&entry.text))
                    .count(),
            )
        });
    }

    fn add_learned_words(&self, raw: &str, candidates: &mut Vec<MixedCandidate>) {
        if self.learning.mode != LearningMode::Zhixu || self.use_rime_order(raw) { return; }
        let all = self.learning.entries();
        let mut entries: Vec<_> = all.iter().filter(|entry| entry.is_learned()).collect();
        let tick = self.learning.rime_tick();
        entries.sort_by(|a, b| b.score(tick).total_cmp(&a.score(tick)).then(b.last_ms.cmp(&a.last_ms)).then(a.text.cmp(&b.text)));
        let mut inserted = 0;
        for entry in entries.into_iter().filter(|entry| learning_covers(raw, &entry.reading)) {
            if candidates.iter().any(|item| item.candidate.text == entry.text) { continue; }
            let reading = entry.reading.replace(' ', "");
            if self.lexicon.blocked_texts.contains(&normalize_text(&entry.text))
                || self.blacklist.lock().ok().is_none_or(|store| store.contains(&reading, &entry.text)
                    || store.contains(raw, &entry.text)) { continue; }
            // One new whole word is enough to make the next occurrence directly selectable.
            // Keep the native selection mapping whenever Rime already has it.
            let slot = (PAGE_SIZE - 1).min(candidates.len());
            candidates.insert(slot, MixedCandidate {
                candidate: Candidate { text: entry.text.clone(), comment: "个人 · 已学习".into(), reading_key: reading, source_index: None },
                selection: Selection::Local { text: entry.text.clone() }, tier: 2, score: 0,
            });
            inserted += 1;
            if inserted == 1 { break; }
        }
    }

    fn use_rime_order(&self, raw: &str) -> bool {
        self.learning.use_rime_order(raw)
            && (!self.inner.native_editing()
                || !self.learning.entries().iter().any(|entry| self.learned_abbreviation_matches(raw, entry)))
    }

    fn learned_abbreviation_matches(&self, raw: &str, entry: &zhixu::learning::Entry) -> bool {
        self.inner.native_editing() && entry.is_learned() && entry.text.chars().count() >= 2
            && learning_covers(raw, &entry.reading)
    }

    fn apply_zhixu_promotions(&self, raw: &str, candidates: &mut Vec<MixedCandidate>) {
        if self.use_rime_order(raw) { return; }
        let choices = self.learning.choices();
        let entries = self.learning.entries();
        if self.whole_enabled() && entries.is_empty() && choices.is_empty() {
            self.set_confirmed_first(None);
            return;
        }
        let tick = self.learning.rime_tick();
        let compact = raw.replace([' ', '\''], "");
        let mut targets = HashMap::new();
        for (position, item) in candidates.iter().enumerate() {
            // Native multi-character abbreviations carry a full reading; bare d and
            // the Rime inner path still require complete syllables.
            if let Some(entry) = entries.iter().filter(|e| e.text == item.candidate.text
                && (e.reading.replace(' ', "") == compact
                    || (self.learned_abbreviation_matches(raw, e) && item.candidate.reading_key == e.reading)))
                .max_by(|a, b| a.score(tick).total_cmp(&b.score(tick))) {
                targets.insert(entry.text.clone(), (if entry.count >= CONFIRMED_PICKS { 0 } else { position.saturating_sub(MAX_SELECTION_PROMOTION) }, entry.count, entry.last_ms, entry.score(tick)));
            }
        }
        for choice in choices.iter().filter(|c| c.app == self.app_id && c.raw == raw) {
            // A per-app choice must not replace migrated recency with lifetime counts.
            let score = targets.get(&choice.text).map_or(choice.count as f64, |value| value.3);
            targets.insert(choice.text.clone(), (choice.target, choice.count, choice.last_ms, score));
        }
        if targets.is_empty() && self.whole_enabled() {
            self.set_confirmed_first(None);
            return;
        }
        for target in 0..candidates.len() {
            let selected = (target..candidates.len()).filter(|&i| targets.get(&candidates[i].candidate.text)
                .is_some_and(|&(position, _, _, _)| position == target))
                .max_by(|&a, &b| {
                    let (_, _, last_a, score_a) = targets[&candidates[a].candidate.text];
                    let (_, _, last_b, score_b) = targets[&candidates[b].candidate.text];
                    score_a.total_cmp(&score_b).then(last_a.cmp(&last_b)).then(b.cmp(&a))
                });
            if let Some(index) = selected { let item = candidates.remove(index); candidates.insert(target, item); }
        }
        self.set_confirmed_first(candidates.first().and_then(|c| targets.get(&c.candidate.text)
            .filter(|&&(target, count, _, _)| target == 0 && count >= CONFIRMED_PICKS).map(|_| c.candidate.text.clone())));
    }

    fn protect_learned_whole(&self, raw: &str, candidates: &mut Vec<MixedCandidate>) {
        if self.learning.mode != LearningMode::Zhixu || self.use_rime_order(raw) { return; }
        let entries = self.learning.entries();
        if self.whole_enabled() && entries.is_empty() { return; }
        for index in 0..candidates.len() {
            let text = &candidates[index].candidate.text;
            if text.chars().count() < 2 || !entries.iter().any(|e| e.is_learned() && e.text == *text && learning_covers(raw, &e.reading)) { continue; }
            if let Some(target) = (0..index).find(|&i| learning_partial(raw, &candidates[i].candidate.text)) {
                let item = candidates.remove(index);
                candidates.insert(target, item);
            }
        }
    }

    fn learning_choice(&self, raw: &str, candidate: &Candidate, position: usize, page: &[Candidate]) -> Option<Choice> {
        let compact = raw.replace([' ', '\''], "");
        let (reading, consumed) = resolve_learning_reading(raw, &candidate.text)
            .filter(|(reading, consumed)| !whole_candidate(candidate)
                || (*consumed == compact.len() && reading.replace(' ', "") == compact
                    && whole_spelling_matches(raw, reading)))
            .or_else(|| {
            // An imported whole word need not be in the shipped dictionary. Its
            // stored syllables are evidence; the candidate's raw echo is not.
            if !whole_candidate(candidate) { return None; }
            let mut readings: Vec<_> = self.learning.entries().iter()
                .filter(|entry| entry.text == candidate.text && entry.reading.replace(' ', "") == compact
                    && whole_spelling_matches(raw, &entry.reading))
                .map(|entry| entry.reading.clone()).collect();
            readings.sort();
            readings.dedup();
            (readings.len() == 1).then(|| (readings.remove(0), compact.len()))
        })?;
        let alternatives = page.iter().filter(|c| c.text != candidate.text
            && c.text.chars().count() == candidate.text.chars().count()
            && c.text.chars().all(|c| ('\u{3400}'..='\u{9fff}').contains(&c)))
            .take(9).map(|c| c.text.clone()).collect();
        Some(Choice { app: self.app_id.clone(), raw: compact[..consumed].into(), reading,
            text: candidate.text.clone(), target: position.saturating_sub(MAX_SELECTION_PROMOTION).min(8),
            count: 1, last_ms: personal_picks::now_ms(), alternatives })
    }

    fn whole_enabled(&self) -> bool {
        // The native decoder already generated its complete input. The retained
        // block-3 path is only for explicit legacy probes, never product sessions.
        if self.inner.native_editing() { return false; }
        #[cfg(test)]
        if let Some(enabled) = self.whole_override { return enabled; }
        false
    }

    fn confirmed_boundary(&self) -> Option<CompositionBoundary> {
        self.inner.composition_boundary(BoundaryPurpose::Mixing)
            .filter(|boundary| !boundary.confirmed.is_empty())
    }

    /// The letters of the input still to be converted: the decoder's own boundary,
    /// or the whole raw input when it reports none.
    fn letters_left(&self) -> String {
        self.inner.composition_boundary(BoundaryPurpose::Mixing)
            .map_or_else(|| self.inner.snapshot().raw_input, |boundary| boundary.unconfirmed_raw)
    }

    fn at_native_edit_boundary(&self) -> bool {
        if !self.inner.native_editing() { return false; }
        let snapshot = self.inner.snapshot();
        !snapshot.raw_input.is_empty() && self.inner.composition_boundary(BoundaryPurpose::Mixing)
            .is_some_and(|boundary| snapshot.cursor_utf8 == boundary.confirmed.len())
    }

    fn keep_native_page_column(&mut self, column: Option<usize>) -> Result<String, i32> {
        let Some(column) = column else { return Ok(String::new()); };
        let page = self.snapshot();
        let column = column.min(page.candidates.len().saturating_sub(1));
        if self.whole_base().is_some() { self.whole_highlighted = column; }
        else if self.deep_page_no.is_some() { self.deep_highlighted = column; }
        else if self.fuzzy_mask != 0 { self.fuzzy_highlighted = column; }
        else {
            // Returning from Mixer's deep pages can restore a different column
            // than the inner pager last held. Move only within the arrived row.
            let arrived = self.inner.snapshot().highlighted_index;
            for _ in column..arrived { self.inner.select_previous()?; }
            for _ in arrived..column { self.inner.select_next()?; }
        }
        Ok(String::new())
    }

    fn whole_reading(&self, base: &DecoderSnapshot) -> Option<String> {
        if !self.whole_enabled() || self.raw_override.is_some() || self.confirmed_boundary().is_some() {
            return None;
        }
        let reading = canonical_reading(&base.raw_input)?;
        if whole_syllable_count(&reading).is_none()
            || has_registered_ascii_exact(&reading, self.code_ascii_bias)
            || base.candidates.iter().any(|c| c.text.is_ascii() && c.text.eq_ignore_ascii_case(&base.raw_input))
        {
            return None;
        }
        Some(reading)
    }

    fn whole_base(&self) -> Option<Arc<DecoderSnapshot>> {
        #[cfg(test)]
        let _timing = WholeTiming::start(1);
        #[cfg(test)]
        let _mixer_timing = WholeMixerTiming::start(7);
        if !self.whole_enabled() { return None; }
        let base = self.inner.snapshot();
        let _dictionary = self.pin_dictionary();
        let key = WholeKey {
            raw: base.raw_input.clone(), preedit: base.preedit.clone(), cursor: base.cursor_utf8,
            native_page: base.page_no, native_last: base.is_last_page,
            native_ascii: base.candidates.iter().any(|c| c.text.is_ascii() && c.text.eq_ignore_ascii_case(&base.raw_input)),
            dictionary: zhixu::generation(), learning: self.learning.revision(), mode: self.learning.mode,
            preferences: whole_preferences_revision().load(std::sync::atomic::Ordering::Relaxed),
            enabled: self.whole_enabled(), corrections: ranking::settings(),
        };
        if let Ok(cache) = self.whole_cache.lock()
            && let Some(cache) = cache.as_ref().filter(|cache| cache.key == key)
        {
            return cache.base.clone();
        }
        let all = self.load_whole_base(base).map(Arc::new);
        if let Ok(mut cache) = self.whole_cache.lock() {
            *cache = Some(WholeCache { key, base: all.clone(), generated: None, generated_complete: true, generated_certified: false, ranked: None, confirmed_first: None });
        }
        all
    }

    fn whole_prefix_safe(&self, raw: &str) -> bool {
        #[cfg(test)]
        if self.whole_eager { return false; }
        let Some(count) = whole_syllable_count(raw) else { return false; };
        let compact = raw.replace('\'', "");
        let index = zhixu::current();
        let exact_word = index.whole_words_for_compact_reading(&compact).any(|word|
            word.syllables == count && word.text.chars().count() >= 2 && whole_spelling_matches(raw, word.reading));
        exact_word && raw.len() > 1 && self.fuzzy_mask == 0 && !self.typo_enabled && !self.code_ascii_bias
            && self.preceding_text.is_none() && self.lexicon.entries.is_empty()
            && self.lead_composition.is_none() && self.offered_composition.is_none()
            && owner_verified_hit_for_raw(raw).is_none() && daily_abbreviation_for(raw).is_none()
            && project_abbreviation_for(raw, self.active_project.as_deref(), &self.active_packs).is_none()
            && self.english_choices.lock().ok().is_some_and(|choices| choices.get(raw).is_none())
            && self.learning.mode == LearningMode::Zhixu && !self.use_rime_order(raw)
            && self.learning.entries().is_empty() && self.learning.choices().is_empty()
            && embedded_ascii_match(raw).is_none()
    }

    fn load_whole_base(&self, base: DecoderSnapshot) -> Option<DecoderSnapshot> {
        self.whole_reading(&base)?;
        let maximum = if whole_syllable_count(&base.raw_input).is_some_and(|count| count < LONG_PHRASE_MIN_SYLLABLES)
            && self.whole_prefix_safe(&base.raw_input) && !self.inner.whole_prefix_has_raw_echo() { 2 * PAGE_SIZE } else { usize::MAX };
        self.load_whole_prefix(base, maximum)
    }

    fn load_whole_prefix(&self, base: DecoderSnapshot, maximum: usize) -> Option<DecoderSnapshot> {
        let native_all = {
            #[cfg(test)]
            let _timing = WholeTiming::start(2);
            self.inner.snapshot_candidate_prefix(maximum)
        };
        let mut all = match native_all {
            Some(all) => all,
            None if base.is_last_page && base.page_no == 0 => base,
            None => return None,
        };
        // The native pager stays put; retained prefix candidates must use their global index.
        for (index, candidate) in all.candidates.iter_mut().enumerate() {
            candidate.source_index = Some(candidate.source_index.unwrap_or(index));
        }
        all.page_no = 0;
        Some(all)
    }

    fn extend_whole_prefix(&self, base: &DecoderSnapshot, maximum: usize) -> Option<Arc<DecoderSnapshot>> {
        let next = Arc::new(self.load_whole_prefix(self.inner.snapshot(), maximum)?);
        if !next.is_last_page && next.candidates.len() <= base.candidates.len() { return None; }
        if let Ok(mut cache) = self.whole_cache.lock() && let Some(cache) = cache.as_mut() {
            cache.base = Some(next.clone());
            cache.ranked = None;
        }
        Some(next)
    }

    fn complete_whole_generation(&self, base: &DecoderSnapshot) -> bool {
        let Some((generated, complete)) = self.whole_cache.lock().ok().and_then(|cache|
            cache.as_ref().map(|cache| (cache.generated.is_some(), cache.generated_complete))) else { return false; };
        if generated && complete { return false; }
        let Some(reading) = self.whole_reading(base) else { return false; };
        #[cfg(test)]
        if generated { WHOLE_GENERATION_CONTINUATIONS.with(|count| count.set(count.get() + 1)); }
        let entries = self.learning.entries();
        let mut lattice = self.whole_lattice.lock().ok();
        let generated = whole_reading_candidates_with(&reading, &entries, self.learning.rime_tick(),
            self.use_rime_order(&base.raw_input),
            lattice.as_deref_mut().map(|cache| (cache, self.learning.revision())));
        if let Ok(mut cache) = self.whole_cache.lock() && let Some(cache) = cache.as_mut() {
            cache.generated = Some(generated);
            cache.generated_complete = true;
            cache.generated_certified = false;
            cache.ranked = None;
        }
        true
    }

    fn whole_page_ranked(&self, mut base: Arc<DecoderSnapshot>) -> (Arc<DecoderSnapshot>, Arc<Vec<MixedCandidate>>) {
        loop {
            let ranked = self.whole_ranked(&base);
            let (generated_complete, certified) = self.whole_cache.lock().ok().and_then(|cache|
                cache.as_ref().map(|cache| (cache.generated_complete, cache.generated_certified)))
                .unwrap_or((true, false));
            let needed = self.whole_page_no.saturating_add(2).saturating_mul(PAGE_SIZE);
            // Generated tails follow every native prefix. Keep a full next page
            // before declaring that their as-yet-unknown count cannot affect last.
            let stable = ranked.iter().filter(|item| item.candidate.comment != ZHIXU_WHOLE_TAIL_COMMENT).count();
            if !generated_complete && (!certified || base.is_last_page && stable < needed) {
                if self.complete_whole_generation(&base) {
                    if !certified && !base.is_last_page {
                        if let Some(next) = self.extend_whole_prefix(&base, usize::MAX) { base = next; }
                    }
                    continue;
                }
            }
            if base.is_last_page { return (base, ranked); }
            // The certificate is recorded before complete-candidate promotion:
            // an exact word already fixes has_exact_reading, and that promotion
            // must already have an unconditional early return on this prefix.
            if certified && stable >= needed { return (base, ranked); }
            let maximum = if certified { base.candidates.len().saturating_add(2 * PAGE_SIZE).max(needed) }
                else { usize::MAX };
            let Some(next) = self.extend_whole_prefix(&base, maximum) else { return (base, ranked) };
            base = next;
        }
    }

    fn whole_ranked(&self, base: &DecoderSnapshot) -> Arc<Vec<MixedCandidate>> {
        if let Ok(cache) = self.whole_cache.lock()
            && let Some(ranked) = cache.as_ref().and_then(|cache| cache.ranked.as_ref())
            && ranked.lead == self.lead_composition && ranked.offered == self.offered_composition
        {
            // Another session may have updated the process-wide protection since our last query.
            if let Some(confirmed) = &ranked.confirmed_first { ranking::set_confirmed_first(confirmed.clone()); }
            return ranked.candidates.clone();
        }
        #[cfg(test)]
        let _timing = WholeTiming::start(3);
        let mut ranked = self.first_layer_with_whole(base, true);
        #[cfg(test)]
        let _mixer_timing = WholeMixerTiming::start(6);
        for item in &mut ranked {
            if matches!(item.selection, Selection::Base { .. }) {
                let Selection::Base { index, text } = std::mem::replace(&mut item.selection,
                    Selection::Local { text: String::new() }) else { unreachable!() };
                item.selection = Selection::GlobalBase { index, text };
            }
        }
        self.add_learned_words(&base.raw_input, &mut ranked);
        if self.learning.mode == LearningMode::Zhixu {
            self.apply_zhixu_promotions(&base.raw_input, &mut ranked);
            if !self.use_rime_order(&base.raw_input) { lead_with_owner_letter(&base.raw_input, &mut ranked); }
        }
        if self.fuzzy_mask != 0 {
            self.prioritize_first_syllable_single_characters(&base.raw_input, &mut ranked);
        }
        self.place_offered_composition(base, &mut ranked);
        self.protect_learned_whole(&base.raw_input, &mut ranked);
        order_whole_candidates(&mut ranked);
        let ranked = Arc::new(ranked);
        if let Ok(mut cache) = self.whole_cache.lock()
            && let Some(cache) = cache.as_mut()
        {
            cache.ranked = Some(WholeRanked { lead: self.lead_composition.clone(), offered: self.offered_composition.clone(),
                candidates: ranked.clone(), confirmed_first: cache.confirmed_first.clone() });
        }
        ranked
    }

    fn set_confirmed_first(&self, text: Option<String>) {
        if self.whole_enabled()
            && let Ok(mut cache) = self.whole_cache.lock()
            && let Some(cache) = cache.as_mut()
        {
            cache.confirmed_first = Some(text.clone());
        }
        ranking::set_confirmed_first(text);
    }

    fn first_layer(&self, base: &DecoderSnapshot) -> Vec<MixedCandidate> {
        self.first_layer_with_whole(base, false)
    }

    fn first_layer_with_whole(&self, base: &DecoderSnapshot, whole: bool) -> Vec<MixedCandidate> {
        let _dictionary = self.pin_dictionary();
        if self.raw_override.is_some() {
            if let Some(text) = project_term_for_reading(&base.raw_input) {
                return vec![MixedCandidate {
                    candidate: Candidate {
                        text: text.to_string(),
                        comment: "英文 · 项目词".into(),
                        reading_key: base.raw_input.to_ascii_lowercase(),
                        source_index: None,
                    },
                    selection: Selection::Local {
                        text: text.to_string(),
                    },
                    tier: 0,
                    score: 520,
                }];
            }
        }
        let reading = canonical_reading(&base.raw_input).unwrap_or_default();
        let allowed_text = |text: &str| {
            let normalized = if whole && self.lexicon.blocked_texts.is_empty() {
                Cow::Borrowed(text.trim())
            } else { Cow::Owned(normalize_text(text)) };
            !normalized.is_empty()
                && !self.lexicon.blocked_texts.contains(normalized.as_ref())
                && !crate::lexicon_layers::is_hidden(text, &|word| {
                    chinese_syllabified_readings_by_text().contains_key(word)
                })
        };
        #[cfg(test)]
        let mixer_timing = WholeMixerTiming::start(0);
        let whole_reading = whole.then(|| self.whole_reading(base)).flatten();
        let whole_compact = whole_reading.as_ref().map(|reading| reading.replace('\'', ""));
        let mut base_candidates: Vec<MixedCandidate> = base
            .candidates
            .iter()
            .enumerate()
            .filter(|(_, candidate)| whole_compact.as_ref().is_none_or(|compact|
                candidate.text.chars().count() < 2 || !(exact_spelling(compact, candidate)
                    || dictionary_phrase_matches_raw(compact, &candidate.text))) && allowed_text(&candidate.text))
            .map(|(index, candidate)| MixedCandidate {
                candidate: candidate.clone(),
                selection: Selection::Base {
                    index: candidate.source_index.unwrap_or(index),
                    text: candidate.text.clone(),
                },
                tier: 2,
                score: -(index as i64),
            })
            .collect();
        #[cfg(test)]
        drop(mixer_timing);
        #[cfg(test)]
        let mixer_timing = WholeMixerTiming::start(1);
        if let Some(reading) = whole_reading {
            // Single characters and prefix selections retain the native decoder mapping.
            let entries = self.learning.entries();
            let cached = self.whole_cache.lock().ok().and_then(|cache| cache.as_ref().and_then(|cache| cache.generated.clone()));
            let mut whole = cached.unwrap_or_else(|| {
                let mut lattice = self.whole_lattice.lock().ok();
                let (generated, complete) = if self.whole_prefix_safe(&base.raw_input) {
                    whole_reading_candidates_prefix_with(&reading, &entries, self.learning.rime_tick(),
                        self.use_rime_order(&base.raw_input),
                        lattice.as_deref_mut().map(|cache| (cache, self.learning.revision())),
                        &|text| allowed_text(text) && self.blacklist.lock().ok()
                            .is_some_and(|store| store.is_empty() || !store.contains(&reading, text)))
                } else {
                    (whole_reading_candidates_with(&reading, &entries, self.learning.rime_tick(),
                        self.use_rime_order(&base.raw_input),
                        lattice.as_deref_mut().map(|cache| (cache, self.learning.revision()))), true)
                };
                if let Ok(mut cache) = self.whole_cache.lock() && let Some(cache) = cache.as_mut() {
                    cache.generated = Some(generated.clone());
                    cache.generated_complete = complete;
                }
                generated
            });
            {
                let blacklist = self.blacklist.lock().ok();
                whole.retain(|item| allowed_text(&item.candidate.text) && blacklist.as_ref()
                    .is_some_and(|store| store.is_empty() || !store.contains(&reading, &item.candidate.text)));
            }
            let mut sentence = false;
            for item in &mut whole {
                if item.candidate.comment == ZHIXU_WHOLE_SENTENCE_COMMENT {
                    if sentence { item.candidate.comment = ZHIXU_WHOLE_TAIL_COMMENT.into(); }
                    sentence = true;
                }
            }
            whole.append(&mut base_candidates);
            order_whole_candidates(&mut whole);
            base_candidates = whole;
        }
        #[cfg(test)]
        drop(mixer_timing);
        #[cfg(test)]
        let mixer_timing = WholeMixerTiming::start(2);
        let has_exact_reading = base_candidates.iter().any(|item| {
            dictionary_readings(&item.candidate.text)
                .is_some_and(|readings| readings.iter().any(|(compact, _)| compact == &reading))
        });
        if !reading.is_empty() && !has_exact_reading {
            // When librime accepted only abbreviation/mixed-spelling parses,
            // protect candidates whose dictionary reading is a real completion
            // or a single adjacent typo of the raw input. This keeps initials
            // support enabled while preventing high-frequency one-character
            // sentence assembly from outranking a coherent multi-letter word.
            // Exact queries deliberately retain librime's original order.
            let (plausible, assembled): (Vec<_>, Vec<_>) =
                base_candidates.into_iter().partition(|item| {
                    dictionary_readings(&item.candidate.text)
                        .is_some_and(|readings| {
                            readings.iter().any(|(_, syllabified)| {
                                plausible_nonexact_reading(&reading, syllabified)
                            })
                        })
                });
            base_candidates = plausible.into_iter().chain(assembled).collect();
        }
        if self.fuzzy_mask != 0 {
            // A fuzzy derivation retains its language-model weight. Stable
            // partitioning here prevents frequent fuzzy spellings from hiding
            // exact spellings without weakening fuzzy candidate generation.
            // Four protected exact slots keep the fuzzy corpus reachable in
            // Top-5 even for readings (such as ci) with many exact homophones.
            let mut protected = Vec::new();
            let mut remainder = Vec::new();
            for item in base_candidates {
                // Only dictionary words earn the protection: an assembled sentence carries the
                // raw letters as its reading and would otherwise always look exact (你门 over
                // 你们 for limen with n/l on).
                let exact = dictionary_readings(&item.candidate.text)
                    .is_some_and(|readings| readings.iter().any(|(compact, _)| compact == &reading));
                if exact && protected.len() < MAX_FUZZY_EXACT_PROTECTION {
                    protected.push(item);
                } else {
                    remainder.push(item);
                }
            }
            protected.extend(remainder);
            base_candidates = protected;
        }
        self.apply_bounded_promotions(&reading, &mut base_candidates);
        self.apply_context_ranking(&reading, &mut base_candidates);
        self.apply_owner_declared_ranking(&reading, &mut base_candidates);
        let english_leads = base.page_no == 0 && english_word_leads(&base.raw_input, &base_candidates);

        #[cfg(test)]
        drop(mixer_timing);
        #[cfg(test)]
        let mixer_timing = WholeMixerTiming::start(3);
        let mut personalized = Vec::new();
        let mut demoted = Vec::new();
        // Personalized candidates are a first-page overlay. Repeating them on
        // every Rime page would both hide later base candidates and violate the
        // deterministic no-duplicate contract across pagination.
        if base.page_no == 0 {
            if let Some((text, _full_reading)) = owner_verified_hit_for_raw(&base.raw_input) {
                personalized.push(MixedCandidate {
                    candidate: Candidate {
                        text: text.to_string(),
                        comment: "常用短串 · 已确认".into(),
                        reading_key: base.raw_input.to_ascii_lowercase(),
                        source_index: None,
                    },
                    // An exact owner-confirmed raw hit is a local, complete
                    // commit. It never uses librime selection and therefore
                    // cannot teach UserDB, regardless of candidate position.
                    selection: Selection::Local {
                        text: text.to_string(),
                    },
                    tier: 0,
                    score: 526,
                });
            }
            if let Some((text, reading, weight)) = daily_abbreviation_for(&base.raw_input) {
                personalized.push(MixedCandidate {
                    candidate: Candidate {
                        text: text.to_string(),
                        comment: "日用词 · 简拼".into(),
                        reading_key: reading.to_string(),
                        source_index: None,
                    },
                    selection: Selection::Local {
                        text: text.to_string(),
                    },
                    tier: 1,
                    score: *weight,
                });
            }
            #[cfg(test)]
            let local_timing = WholeMixerTiming::start(9);
            // Native prefixes carry their full pronunciation even when they
            // consume fewer keys (clo -> 丑陋 consumes cl). Only whole-input
            // candidates provide Chinese evidence for the existing ASCII gate.
            let ascii = if self.inner.native_editing() {
                let evidence = base_candidates.iter().filter(|item|
                    zhixu::decoder::candidate_coverage(&base.raw_input, &item.candidate) == Some(base.raw_input.len()))
                    .cloned().collect::<Vec<_>>();
                local_ascii_candidates_with_prefix_evidence(&base.raw_input, &base_candidates,
                    &evidence, self.code_ascii_bias, self.typo_enabled)
            } else {
                local_ascii_candidates(&base.raw_input, &base_candidates, self.code_ascii_bias, self.typo_enabled)
            };
            personalized.extend(ascii);
            #[cfg(test)]
            drop(local_timing);
            #[cfg(test)]
            let local_timing = WholeMixerTiming::start(10);
            if should_promote_raw_ascii(&base.raw_input, &base_candidates) {
                personalized.extend(heavy_garble_candidates(
                    &base.raw_input,
                    &base_candidates,
                    self.code_ascii_bias,
                ));
            }
            if self.typo_enabled
                && !english_leads
                && !should_promote_raw_ascii(&base.raw_input, &base_candidates)
            {
                personalized.extend(slip_candidates(
                    &base.raw_input,
                    &base_candidates,
                    self.code_ascii_bias,
                ));
            }
            #[cfg(test)]
            drop(local_timing);
            #[cfg(test)]
            let local_timing = WholeMixerTiming::start(11);
            if embedded_ascii_match(&base.raw_input).is_some() {
                // Existing local decoder candidates are the primary evidence
                // for the Chinese spans. Deeper Rime pages are only a bounded
                // fallback when one side is not present in that leading set.
                let mut embedded_sources = personalized
                    .iter()
                    .chain(base_candidates.iter())
                    .map(|item| item.candidate.clone())
                    .collect::<Vec<_>>();
                if let Some(all) = self.inner.snapshot_all_candidates() {
                    for candidate in all.candidates {
                        if embedded_sources.len() >= 256 {
                            break;
                        }
                        if !embedded_sources
                            .iter()
                            .any(|existing| existing.text == candidate.text)
                        {
                            embedded_sources.push(candidate);
                        }
                    }
                }
                if let Some(candidate) =
                    embedded_ascii_candidate(&base.raw_input, &embedded_sources)
                {
                    personalized.push(candidate);
                }
            }
            #[cfg(test)]
            drop(local_timing);
            #[cfg(test)]
            let _local_timing = WholeMixerTiming::start(12);
            if let Some(expansion) = project_abbreviation_for(
                &base.raw_input,
                self.active_project.as_deref(),
                &self.active_packs,
            ) {
                personalized.push(MixedCandidate {
                    candidate: Candidate {
                        text: expansion.text.clone(),
                        comment: "英文 · 项目缩写".into(),
                        reading_key: expansion.reading.clone(),
                        source_index: None,
                    },
                    selection: Selection::Local {
                        text: expansion.text.clone(),
                    },
                    tier: 0,
                    score: 519,
                });
            }
            for index in self.lexicon.first_layer_indices(&reading) {
                let entry = &self.lexicon.entries[index];
                if entry.disabled
                    || entry.candidate_layer != "first"
                    || !entry_applies(entry, self.active_project.as_deref(), &self.active_packs)
                {
                    continue;
                }
                let exact = entry.pinyin == reading;
                let fixed_prefix =
                    entry.fixed && !reading.is_empty() && entry.pinyin.starts_with(&reading);
                if !exact && !fixed_prefix {
                    continue;
                }
                let source_label = source_label(entry);
                let mixed = MixedCandidate {
                    candidate: Candidate {
                        text: entry.text.clone(),
                        comment: source_label,
                        reading_key: entry.pinyin.clone(),
                        source_index: None,
                    },
                    selection: Selection::Local {
                        text: entry.text.clone(),
                    },
                    tier: if entry.fixed {
                        0
                    } else if entry.demoted {
                        2
                    } else {
                        1
                    },
                    score: entry.base_weight,
                };
                if entry.demoted && !entry.fixed {
                    demoted.push(mixed)
                } else {
                    personalized.push(mixed)
                }
            }
        }
        personalized.sort_by(mixed_order);
        demoted.sort_by(mixed_order);

        // A local duplicate inherits the strongest tier but keeps the real
        // decoder selection path so librime's at-most-once learning stays intact.
        if !whole || !personalized.is_empty() || !demoted.is_empty() {
            let base_by_key: HashMap<(String, String), &MixedCandidate> = base_candidates
                .iter()
                .map(|item| {
                    (
                        candidate_key(&item.candidate, &reading),
                        item,
                    )
                })
                .collect();
            for item in personalized.iter_mut().chain(demoted.iter_mut()) {
                if let Some(base) = base_by_key.get(&candidate_key(&item.candidate, &reading)) {
                    item.selection = base.selection.clone();
                    if whole && whole_candidate(&base.candidate) {
                        item.candidate.comment = base.candidate.comment.clone();
                    }
                }
            }
        }

        #[cfg(test)]
        drop(mixer_timing);
        #[cfg(test)]
        let mixer_timing = WholeMixerTiming::start(4);
        let mut output;
        if whole {
            output = personalized.into_iter().chain(base_candidates).chain(demoted).collect();
            let blacklist = self.blacklist.lock().ok();
            retain_unique_whole(&mut output, &reading, blacklist.as_deref());
        } else {
            output = Vec::new();
            let mut seen = HashSet::new();
            for item in personalized
                .into_iter()
                .chain(base_candidates)
                .chain(demoted)
            {
                let key = candidate_key(&item.candidate, &reading);
                let scoped_reading = canonical_reading(&item.candidate.reading_key)
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| reading.clone());
                let blacklisted =
                    self.blacklist.lock().ok().is_none_or(|blacklist| {
                        blacklist.contains(&scoped_reading, &item.candidate.text)
                    });
                if !blacklisted && seen.insert(key) {
                    output.push(item);
                }
            }
        }
        #[cfg(test)]
        drop(mixer_timing);
        #[cfg(test)]
        let _mixer_timing = WholeMixerTiming::start(5);
        if whole {
            let short_reading = whole_has_complete_short_reading(&reading, &output);
            let certified = self.whole_prefix_safe(&base.raw_input)
                && output.iter().any(|item| item.candidate.comment == ZHIXU_WHOLE_COMMENT
                    && dictionary_readings(&item.candidate.text).is_some_and(|readings|
                        readings.iter().any(|(compact, _)| compact == &reading)))
                && !output.iter().any(|item| matches!(item.selection, Selection::Local { .. })
                    && !whole_candidate(&item.candidate))
                && (short_reading || output.first().is_some_and(|item|
                    dictionary_phrase_match_syllable_count(&reading, &item.candidate.text).is_some()));
            if let Ok(mut cache) = self.whole_cache.lock() && let Some(cache) = cache.as_mut() {
                cache.generated_certified = certified;
            }
            promote_complete_dictionary_candidate_to_top_with_short_reading(
                &base.raw_input, self.code_ascii_bias, &mut output, Some(short_reading), false);
        } else {
            promote_complete_dictionary_candidate_to_top_with_short_reading(
                &base.raw_input, self.code_ascii_bias, &mut output, None, self.inner.native_editing());
        }
        demote_raw_ascii_to_last_first_page_slot(&base.raw_input, &mut output);
        place_slip(&mut output);
        if base.page_no == 0 && !base.candidates.is_empty() && !output.is_empty() && !english_leads {
            for tail in trailing_letter_candidates(&base.raw_input, &output, self.code_ascii_bias).into_iter().rev() {
                if self.blacklist.lock().ok().is_some_and(|blacklist| !blacklist.contains(&reading, &tail.candidate.text)) {
                    output.insert(3.min(output.len()), tail);
                }
            }
        }
        if base.page_no == 0 {
            place_short_english(&base.raw_input, &reading, &mut output);
            lead_with_owner_letter(&base.raw_input, &mut output);
        }
        if english_leads {
            lead_with_english_word(&base.raw_input, &reading, &mut output);
        }
        if base.page_no == 0
            && let Some(text) = self
                .english_choices
                .lock()
                .ok()
                .and_then(|choices| choices.get(&base.raw_input).map(str::to_string))
        {
            lead_with_learned_english(&text, &reading, &mut output);
        }
        if base.page_no == 0
            && let Some((raw, text)) = &self.lead_composition
            && *raw == base.raw_input
        {
            // A decoder candidate already on the page moves up and keeps its own
            // selection path, so committing it still teaches the user dictionary.
            if let Some(index) = output
                .iter()
                .take(PAGE_SIZE)
                .position(|item| item.candidate.text == *text && !matches!(item.selection, Selection::Local { .. }))
            {
                let item = output.remove(index);
                output.insert(0, item);
                return output;
            }
            output.retain(|item| item.candidate.text != *text);
            output.insert(
                0,
                MixedCandidate {
                    candidate: Candidate {
                        text: text.clone(),
                        comment: COMPOSITION_COMMENT.into(),
                        reading_key: reading.clone(),
                        source_index: None,
                    },
                    selection: Selection::Local { text: text.clone() },
                    tier: 0,
                    score: 0,
                },
            );
            // The lead pushes the short English word off the first page: put it back.
            place_short_english(&base.raw_input, &reading, &mut output);
        }
        output
    }

    fn prediction_layer(&self, base: &DecoderSnapshot) -> Vec<MixedCandidate> {
        if !self.secondary_enabled || !self.prediction_enabled || base.raw_input.is_empty() {
            return Vec::new();
        }
        let reading = canonical_reading(&base.raw_input).unwrap_or_default();
        let mut entries = self
            .lexicon
            .secondary_indices
            .iter()
            .map(|&index| &self.lexicon.entries[index])
            .filter(|entry| {
                let personal_completion = is_personal_auto_prediction_entry(entry)
                    && entry.candidate_layer == "first"
                    && entry.pinyin != reading;
                !entry.disabled
                    && (entry.candidate_layer == "secondary" || personal_completion)
                    && (entry.pinyin == reading || entry.pinyin.starts_with(&reading))
                    && entry_applies(entry, self.active_project.as_deref(), &self.active_packs)
            })
            .collect::<Vec<_>>();
        entries.sort_by(|left, right| {
            right
                .base_weight
                .cmp(&left.base_weight)
                .then_with(|| left.text.cmp(&right.text))
                .then_with(|| left.id.cmp(&right.id))
        });
        entries
            .into_iter()
            .take(PAGE_SIZE)
            .map(|entry| MixedCandidate {
                candidate: Candidate {
                    text: entry.text.clone(),
                    comment: format!("关联 · {}", source_label(entry)),
                    reading_key: entry.pinyin.clone(),
                    source_index: None,
                },
                selection: Selection::Local {
                    text: entry.text.clone(),
                },
                tier: 3,
                score: entry.base_weight,
            })
            .collect()
    }

    fn secondary_layer(&self, base: &DecoderSnapshot) -> Vec<MixedCandidate> {
        if !self.secondary_enabled {
            return Vec::new();
        }
        let mut seen = HashSet::new();
        self.prediction_layer(base)
            .into_iter()
            .filter(|item| {
                seen.insert((
                    normalize_text(&item.candidate.text),
                    item.candidate.reading_key.clone(),
                ))
            })
            .take(PAGE_SIZE)
            .collect()
    }

    fn visible(&self) -> (DecoderSnapshot, Vec<MixedCandidate>) {
        #[cfg(test)]
        let _mixer_timing = WholeMixerTiming::start(8);
        let _dictionary = self.pin_dictionary();
        let (mut snapshot, mut mixed) = self.visible_unranked();
        if let Some((raw, page, candidates, ordered)) = &self.host_order {
            if raw == &snapshot.raw_input
                && *page == snapshot.page_no
                && candidates == &snapshot.candidates
            {
                mixed = ordered.clone();
            }
        }
        one_row_per_text(&mut mixed);
        self.place_offered_composition(&snapshot, &mut mixed);
        if self.fuzzy_mask == 0 && self.deep_page_no.is_none() && self.whole_base().is_none() {
            self.carry_pages(&snapshot, &mut mixed);
        }
        // The whole sequence already applied learning before paging. Reapplying
        // it to a slice can pull a deferred split ahead of native prefixes.
        if self.whole_base().is_none() {
            self.protect_learned_whole(&snapshot.raw_input, &mut mixed);
        }
        snapshot.candidates = mixed.iter().map(|item| item.candidate.clone()).collect();
        (snapshot, mixed)
    }

    fn note_native_overflow(&self, raw: &str, page: usize, beyond: Vec<MixedCandidate>) {
        let Ok(mut carry) = self.carry.lock() else { return };
        if carry.raw != raw {
            *carry = CarryState { raw: raw.to_string(), ..CarryState::default() };
        }
        carry.native.insert(page, beyond);
    }

    /// A page shows PAGE_SIZE candidates. What something inserted into it pushes off (and what
    /// the previous page pushed off) is carried to the start of the next page, so a common single
    /// character the composition offer displaced (我 for wo xuan yi xuan) is still there to pick.
    fn carry_pages(&self, snapshot: &DecoderSnapshot, mixed: &mut Vec<MixedCandidate>) {
        let Ok(mut carry) = self.carry.lock() else { return };
        let page = snapshot.page_no;
        if carry.raw != snapshot.raw_input {
            *carry = CarryState { raw: snapshot.raw_input.clone(), ..CarryState::default() };
        }
        let mut over: Vec<MixedCandidate> = if mixed.len() > PAGE_SIZE { mixed.split_off(PAGE_SIZE) } else { Vec::new() };
        over.extend(carry.native.get(&page).cloned().unwrap_or_default());
        // These came from this page: a page-local base index becomes a global one.
        let globalise = |items: &mut Vec<MixedCandidate>| {
            for item in items.iter_mut() {
                if let Selection::Base { index, text } = &item.selection {
                    let global = if item.candidate.source_index.is_some() { *index } else { page * PAGE_SIZE + *index };
                    item.selection = Selection::GlobalBase { index: global, text: text.clone() };
                }
            }
        };
        globalise(&mut over);
        if page >= 1
            && let Some(incoming) = carry.out.get(&(page - 1)).filter(|incoming| !incoming.is_empty())
        {
            let mut joined: Vec<MixedCandidate> = incoming
                .iter()
                .filter(|item| !mixed.iter().any(|shown| shown.candidate.text == item.candidate.text))
                .cloned()
                .collect();
            joined.append(mixed);
            *mixed = joined;
            if mixed.len() > PAGE_SIZE {
                // The incoming items are global already; this page's own are not yet.
                let mut tail = mixed.split_off(PAGE_SIZE);
                globalise(&mut tail);
                over = tail.into_iter().chain(over).collect();
            }
        }
        carry.out.insert(page, over);
    }

    /// The offered composition takes the last slot of the first page (slots 1-4 keep
    /// their digits); the candidate it displaces moves after it.
    fn place_offered_composition(&self, snapshot: &DecoderSnapshot, mixed: &mut Vec<MixedCandidate>) {
        let Some((raw, text)) = &self.offered_composition else {
            return;
        };
        if *raw != snapshot.raw_input
            || snapshot.page_no != 0
            || self.deep_page_no.is_some()
            || mixed.iter().take(PAGE_SIZE).any(|item| item.candidate.text == *text)
        {
            return;
        }
        let reading = canonical_reading(raw).unwrap_or_default();
        mixed.insert(
            (PAGE_SIZE - 1).min(mixed.len()),
            MixedCandidate {
                candidate: Candidate {
                    text: text.clone(),
                    comment: crate::ranking::COMPOSITION_OFFER_COMMENT.into(),
                    reading_key: reading,
                    source_index: None,
                },
                selection: Selection::Local { text: text.clone() },
                tier: 0,
                score: 0,
            },
        );
    }

    fn visible_unranked(&self) -> (DecoderSnapshot, Vec<MixedCandidate>) {
        let _dictionary = self.pin_dictionary();
        if let Some(base) = self.whole_base() {
            let (base, ranked) = self.whole_page_ranked(base);
            let native_last = base.is_last_page;
            let mut base = DecoderSnapshot { raw_input: base.raw_input.clone(), preedit: base.preedit.clone(),
                cursor_utf8: base.cursor_utf8, ..DecoderSnapshot::default() };
            let start = self.whole_page_no.saturating_mul(PAGE_SIZE);
            base.is_last_page = native_last && ranked.len().saturating_sub(start) <= PAGE_SIZE;
            let mixed: Vec<_> = ranked.iter().skip(start).take(PAGE_SIZE).cloned().collect();
            base.secondary_candidates = self.secondary_layer(&base).into_iter().map(|item| item.candidate).collect();
            base.candidates = mixed.iter().map(|item| item.candidate.clone()).collect();
            base.page_no = self.whole_page_no;
            base.highlighted_index = self.whole_highlighted.min(mixed.len().saturating_sub(1));
            return (base, mixed);
        }
        let mut base = self.inner.snapshot();
        if self.raw_override.is_none()
            && let Some(boundary) = self.confirmed_boundary()
        {
            base.raw_input = boundary.unconfirmed_raw;
        }
        if let Some(raw) = &self.raw_override {
            base.raw_input = raw.clone();
            base.preedit = raw.clone();
            base.cursor_utf8 = raw.len();
            base.candidates.clear();
            base.highlighted_index = 0;
            base.page_no = 0;
            base.is_last_page = true;
        } else if let Some((text, _)) = owner_verified_hit_for_raw(&base.raw_input) {
            base.preedit = text.to_string();
            base.cursor_utf8 = base.preedit.len();
        }
        if let Some(page_no) = self.deep_page_no {
            let page_start = page_no.saturating_sub(self.deep_base).saturating_mul(PAGE_SIZE);
            let mixed = self
                .deep_candidates
                .iter()
                .skip(page_start)
                .take(PAGE_SIZE)
                .cloned()
                .collect::<Vec<_>>();
            let secondary_candidates = self
                .secondary_layer(&base)
                .into_iter()
                .map(|item| item.candidate)
                .collect();
            let snapshot = DecoderSnapshot {
                raw_input: base.raw_input,
                preedit: base.preedit,
                cursor_utf8: base.cursor_utf8,
                candidates: mixed.iter().map(|item| item.candidate.clone()).collect(),
                secondary_candidates,
                highlighted_index: self.deep_highlighted.min(mixed.len().saturating_sub(1)),
                page_no,
                is_last_page: page_start.saturating_add(PAGE_SIZE) >= self.deep_candidates.len(),
            };
            return (snapshot, mixed);
        }
        let mut ranked = self.first_layer(&base);
        if base.page_no == 0 { self.add_learned_words(&base.raw_input, &mut ranked); }
        if self.learning.mode == LearningMode::Zhixu {
            self.apply_zhixu_promotions(&base.raw_input, &mut ranked);
            if base.page_no == 0 && !self.use_rime_order(&base.raw_input) { lead_with_owner_letter(&base.raw_input, &mut ranked); }
        }
        if self.fuzzy_mask != 0 {
            self.prioritize_first_syllable_single_characters(&base.raw_input, &mut ranked);
        }
        self.protect_learned_whole(&base.raw_input, &mut ranked);
        let page_no = if self.fuzzy_mask == 0 {
            base.page_no
        } else {
            self.fuzzy_page_no
        };
        let page_start = if self.fuzzy_mask == 0 {
            0
        } else {
            page_no.saturating_mul(PAGE_SIZE)
        };
        let mixed = ranked
            .iter()
            .skip(page_start)
            .take(PAGE_SIZE)
            .cloned()
            .collect::<Vec<_>>();
        if self.fuzzy_mask == 0 {
            // Only what our own insertions pushed off: a decoder that hands over more than a
            // page at once pages through the rest itself.
            let beyond = if base.candidates.len() <= PAGE_SIZE {
                ranked.iter().skip(PAGE_SIZE).cloned().collect::<Vec<_>>()
            } else {
                Vec::new()
            };
            self.note_native_overflow(&base.raw_input, page_no, beyond);
        }
        let secondary_candidates = self
            .secondary_layer(&base)
            .into_iter()
            .map(|item| item.candidate)
            .collect();
        let snapshot = DecoderSnapshot {
            raw_input: base.raw_input,
            preedit: base.preedit,
            cursor_utf8: base.cursor_utf8,
            candidates: mixed.iter().map(|item| item.candidate.clone()).collect(),
            secondary_candidates,
            highlighted_index: if self.fuzzy_mask == 0 {
                base.highlighted_index
            } else {
                self.fuzzy_highlighted
            }
            .min(mixed.len().saturating_sub(1)),
            page_no,
            is_last_page: if self.fuzzy_mask == 0 {
                base.is_last_page
            } else {
                page_start.saturating_add(PAGE_SIZE) >= ranked.len()
            },
        };
        (snapshot, mixed)
    }

    fn apply_bounded_promotions(&self, reading: &str, candidates: &mut Vec<MixedCandidate>) {
        if self.learning.mode == LearningMode::Zhixu && !self.use_rime_order(reading) {
            return;
        }
        // Snapshot the (small) learned set for this app+reading up front so the
        // O(n²) placement scan below probes a local map with borrowed keys —
        // no per-probe String clones and no global lock held across the scan.
        // text -> (place, picks, last pick): of two picks that want the same place the one made more often (then more recently) takes it.
        let learned: HashMap<String, (usize, u32, u64)> = {
            let Ok(learning) = shared_learning().lock() else {
                return;
            };
            learning
                .target_positions
                .iter()
                .filter(|((app, learned_reading, _), _)| {
                    app == &self.app_id && learned_reading == reading
                })
                .map(|(key, position)| {
                    (
                        key.2.clone(),
                        (*position, learning.counts.get(key).copied().unwrap_or(1), learning.last_ms.get(key).copied().unwrap_or(0)),
                    )
                })
                .collect()
        };
        if learned.is_empty() {
            self.set_confirmed_first(None);
            return;
        }
        for target in 0..candidates.len() {
            let Some(current) = (target..candidates.len())
                .filter(|index| {
                    learned
                        .get(candidates[*index].candidate.text.as_str())
                        .is_some_and(|(position, _, _)| *position == target)
                })
                .max_by_key(|index| {
                    let (_, picks, last) = learned[candidates[*index].candidate.text.as_str()];
                    (picks, last, std::cmp::Reverse(*index))
                })
            else {
                continue;
            };
            if current != target {
                let item = candidates.remove(current);
                candidates.insert(target, item);
            }
        }
        let confirmed = candidates.first().and_then(|item| {
            let (position, picks, _) = learned.get(item.candidate.text.as_str())?;
            (*position == 0 && *picks >= CONFIRMED_PICKS).then(|| item.candidate.text.clone())
        });
        self.set_confirmed_first(confirmed);
    }

    fn apply_context_ranking(&self, reading: &str, candidates: &mut [MixedCandidate]) {
        let Some(preceding_text) = self.preceding_text.as_deref() else {
            return;
        };
        // Stable sorting is deliberate: candidates without table evidence, or
        // with identical evidence, retain the exact base order. A longer
        // matching suffix is more specific. Context only reorders candidates
        // that the base decoder produced.
        candidates.sort_by(|left, right| {
            let left_score =
                self.context_model
                    .score(reading, preceding_text, &left.candidate.text);
            let right_score =
                self.context_model
                    .score(reading, preceding_text, &right.candidate.text);
            right_score.cmp(&left_score)
        });
    }

    fn record_selection(&self, raw: &str, position: usize, text: &str) {
        if self.learning.mode == LearningMode::Zhixu { return; }
        let Ok(mut learning) = shared_learning().lock() else {
            return;
        };
        learning.undo = None;
        if position == 0 { return; }
        let key = (self.app_id.clone(), raw.to_string(), text.to_string());
        let target = position.saturating_sub(MAX_SELECTION_PROMOTION);
        learning.record(key, target);
        whole_preferences_changed();
    }

    /// A deliberate pick of English for this input is remembered; a deliberate pick
    /// of Chinese forgets an earlier English pick. Raw pinyin committed verbatim
    /// (`woxikanx`, `zhuangixnban`) is not English: only English words, registered
    /// spellings (Windows, ChatGPT) and short tokens (glm, mbp) are remembered.
    fn learn_english_choice(&self, raw: &str, text: &str) {
        let Ok(mut choices) = self.english_choices.lock() else {
            return;
        };
        let english = text != raw || raw.len() <= 5 || english_word_parts(raw);
        let _ = if english && english_choices::valid(raw, text) {
            choices.remember(raw, text)
        } else if !text.is_ascii() {
            choices.forget(raw)
        } else {
            Ok(())
        };
        whole_preferences_changed();
    }

    fn forget_selection(&self, raw: &str, text: &str) {
        let Ok(mut learning) = shared_learning().lock() else {
            return;
        };
        learning.forget(&(self.app_id.clone(), raw.to_string(), text.to_string()));
        whole_preferences_changed();
    }

    fn reset_layer(&mut self) {
        self.invalidate_whole();
        if let Ok(mut dictionary) = self.dictionary_snapshot.lock() {
            *dictionary = None;
        }
        self.host_order = None;
        self.lead_composition = None;
        self.offered_composition = None;
        // The secondary layer is parallel to base pagination and has no
        // numbered page state to reset.
        self.fuzzy_page_no = 0;
        self.fuzzy_highlighted = 0;
        self.whole_page_no = 0;
        self.whole_highlighted = 0;
        self.deep_prefix_keys.clear();
        self.deep_candidates.clear();
        self.deep_page_no = None;
        self.deep_base = 2;
        self.deep_highlighted = 0;
        if let Ok(mut carry) = self.carry.lock() {
            *carry = CarryState::default();
        }
    }

    fn invalidate_whole(&self) {
        if let Ok(mut cache) = self.whole_cache.lock() { *cache = None; }
        if self.whole_enabled() && let Ok(mut cache) = self.whole_lattice.lock() { *cache = None; }
    }

    fn remember_visible_page(&mut self) {
        let (snapshot, _) = self.visible();
        let reading = canonical_reading(&snapshot.raw_input).unwrap_or_default();
        self.deep_prefix_keys.extend(
            snapshot
                .candidates
                .iter()
                .map(|candidate| candidate_key(candidate, &reading)),
        );
    }

    fn prepare_deep_fallback(&mut self) -> bool {
        let Some(all) = self.inner.snapshot_all_candidates() else {
            return false;
        };
        // The letters still to be converted: after a segment was picked mid-composition the
        // decoder's boundary reports them (分析 picked, d toutoude left: characters that start
        // with d, not with fen).
        let rest_raw = self.letters_left();
        let Some(start) = deep_single_start(&rest_raw) else {
            return false;
        };
        let reading = canonical_reading(&rest_raw).unwrap_or_default();
        let mut seen = self.deep_prefix_keys.clone();
        let mut ordered = Vec::new();
        // What the first two pages pushed off (never shown) leads the rest.
        let pushed_off = self.carry.lock().map(|carry| carry.out.get(&1).cloned().unwrap_or_default()).unwrap_or_default();
        for item in pushed_off {
            if seen.insert(candidate_key(&item.candidate, &reading)) {
                ordered.push(item);
            }
        }
        for (page_no, page) in all.candidates.chunks(PAGE_SIZE).enumerate().skip(2) {
            let base = DecoderSnapshot {
                raw_input: rest_raw.clone(),
                preedit: all.preedit.clone(),
                cursor_utf8: all.cursor_utf8,
                candidates: page.to_vec(),
                secondary_candidates: Vec::new(),
                highlighted_index: 0,
                page_no,
                is_last_page: (page_no + 1) * PAGE_SIZE >= all.candidates.len(),
            };
            for mut item in self.first_layer(&base) {
                let key = candidate_key(&item.candidate, &reading);
                if !seen.insert(key) {
                    continue;
                }
                if let Selection::Base { index, text } = item.selection {
                    item.selection = Selection::GlobalBase { index, text };
                }
                ordered.push(item);
            }
        }
        // `ordered` is the native librime/UserDB frequency sequence. A stable
        // partition keeps that frequency order inside both groups.
        let (single, remainder): (Vec<_>, Vec<_>) =
            ordered.into_iter().partition(|item| start.matches(&item.candidate));
        self.deep_candidates = single.into_iter().chain(remainder).collect();
        if self.deep_candidates.is_empty() {
            return false;
        }
        self.deep_base = 2;
        self.deep_page_no = Some(2);
        self.deep_highlighted = 0;
        true
    }

    /// The decoder is on its last page but our own insertions pushed candidates off it: show them
    /// as one more page instead of dropping them.
    fn prepare_overflow_page(&mut self) -> bool {
        let page = self.inner.snapshot().page_no;
        let over = self.carry.lock().map(|carry| carry.out.get(&page).cloned().unwrap_or_default()).unwrap_or_default();
        if over.is_empty() {
            return false;
        }
        self.deep_candidates = over;
        self.deep_base = page + 1;
        self.deep_page_no = Some(page + 1);
        self.deep_highlighted = 0;
        true
    }

    // Only Linear navigation looks beyond the native last-page flag. Keep the
    // shared snapshot/peek semantics used by Windows expansion unchanged.
    // Call after snapshot() has populated the carry for the visible page.
    fn has_linear_overflow(&self, snapshot: &DecoderSnapshot) -> bool {
        self.fuzzy_mask == 0 && self.deep_page_no.is_none()
            && self.carry.lock().map(|carry| carry.raw == snapshot.raw_input
                && carry.out.get(&snapshot.page_no).is_some_and(|items| !items.is_empty()))
                .unwrap_or(false)
    }

    fn prioritize_first_syllable_single_characters(
        &self,
        raw_input: &str,
        candidates: &mut Vec<MixedCandidate>,
    ) {
        let Some(start) = deep_single_start(raw_input) else {
            return;
        };
        if candidates.len() <= PAGE_SIZE * 2 {
            return;
        }
        let deep = candidates.split_off(PAGE_SIZE * 2);
        // The original librime/UserDB order is the frequency order. Stable
        // partitioning promotes matching single characters without changing
        // their relative frequency or either frozen leading page.
        let (single, remainder): (Vec<_>, Vec<_>) =
            deep.into_iter().partition(|item| start.matches(&item.candidate));
        candidates.extend(single);
        candidates.extend(remainder);
    }
}

/// How the single characters that follow the first two pages must be read. Two pages
/// of words, then the characters the input starts with: owner 2026-10-11 「词组短语最好
/// 只给两页……前面没有可用词组，我就希望直接用单字」; after paging the owner picked a
/// single character 29 times of 37, and from the third page 11 of 13 (wenzhge 吻,
/// tnabian 挺, shenxiazhgexiangm 什).
enum DeepSingleStart {
    /// The first syllable of three or more whole syllables (woxianzaiqukan: wo).
    Syllable(String),
    /// Letters that are not whole pinyin: the longest syllable they start with
    /// (wenzhge: wen), else their initial (zhgexiangm: zh, tnabian: t).
    Prefix(String),
}

impl DeepSingleStart {
    fn matches(&self, candidate: &Candidate) -> bool {
        if candidate.text.chars().count() != 1 {
            return false;
        }
        match self {
            DeepSingleStart::Syllable(syllable) => pinyin_segmentation(&candidate.reading_key)
                .and_then(|value| value.into_iter().next())
                .is_some_and(|value| &value == syllable),
            DeepSingleStart::Prefix(prefix) => {
                let whole = pinyin_syllables().contains(prefix.as_str());
                chinese_syllabified_readings_by_text()
                    .get(&candidate.text)
                    .into_iter()
                    .flatten()
                    .any(|(compact, _)| if whole { compact == prefix } else { compact.starts_with(prefix.as_str()) })
            }
        }
    }
}

fn deep_single_start(raw: &str) -> Option<DeepSingleStart> {
    if let Some(syllables) = pinyin_segmentation(raw) {
        return (syllables.len() >= LONG_PHRASE_MIN_SYLLABLES)
            .then(|| DeepSingleStart::Syllable(syllables[0].clone()));
    }
    let reading = canonical_reading(raw)?;
    if reading.is_empty() || !reading.bytes().all(|byte| byte.is_ascii_lowercase()) {
        return None;
    }
    let syllables = pinyin_syllables();
    let longest = (1..=reading.len().min(6)).rev().find(|&end| syllables.contains(&reading[..end]));
    let prefix = match longest {
        Some(end) => reading[..end].to_string(),
        None if ["zh", "ch", "sh"].iter().any(|initial| reading.starts_with(initial)) => reading[..2].to_string(),
        None => reading[..1].to_string(),
    };
    Some(DeepSingleStart::Prefix(prefix))
}

// Only verified, complete dictionary readings qualify. Raw-input fallback
// readings are not evidence of pronunciation (especially for abbreviations).
const HEAVY_GARBLE_COMMENT: &str = "重度乱拼 · 本地索引";
/// A one-letter slip (yoyisi -> 有意思, jieidan -> 节点), offered third; the context
/// model decides whether it leads.
const SLIP_COMMENT: &str = "补字母 · 本地索引";
const COMPOSITION_COMMENT: &str = "整句 · 顺手·知字";
const ZHIXU_WHOLE_COMMENT: &str = "整串 · 顺手·知序";
const ZHIXU_WHOLE_SENTENCE_COMMENT: &str = "整句 · 顺手·知序";
const ZHIXU_WHOLE_TAIL_COMMENT: &str = "备选 · 顺手·知序";

fn whole_candidate(candidate: &Candidate) -> bool {
    matches!(candidate.comment.as_str(), ZHIXU_WHOLE_COMMENT | ZHIXU_WHOLE_SENTENCE_COMMENT | ZHIXU_WHOLE_TAIL_COMMENT)
}

fn order_whole_candidates(candidates: &mut Vec<MixedCandidate>) {
    // Learning and legacy page overlays may reorder the source. Keep the page
    // boundary after those steps too: words, one sentence, native prefixes, tail.
    let mut previous = 0;
    let ordered = candidates.iter().all(|item| {
        let group = match item.candidate.comment.as_str() {
            ZHIXU_WHOLE_COMMENT => 1,
            ZHIXU_WHOLE_SENTENCE_COMMENT => 2,
            ZHIXU_WHOLE_TAIL_COMMENT => 4,
            _ if previous == 0 && matches!(item.selection, Selection::Local { .. }) => 0,
            _ => 3,
        };
        let monotonic = group >= previous;
        previous = group;
        monotonic
    });
    if ordered { return; }
    let mut lead = Vec::new();
    let mut words = Vec::new();
    let mut sentences = Vec::new();
    let mut prefixes = Vec::new();
    let mut tail = Vec::new();
    for item in std::mem::take(candidates) {
        match item.candidate.comment.as_str() {
            ZHIXU_WHOLE_COMMENT => words.push(item),
            ZHIXU_WHOLE_SENTENCE_COMMENT => sentences.push(item),
            ZHIXU_WHOLE_TAIL_COMMENT => tail.push(item),
            // Only personal overlays already ahead of every whole and native candidate stay ahead; a
            // local insertion further down (a slip at slot five, 用哪个 after 用) must not be lifted.
            _ if words.is_empty() && sentences.is_empty() && tail.is_empty() && prefixes.is_empty()
                && matches!(item.selection, Selection::Local { .. }) => lead.push(item),
            _ => prefixes.push(item),
        }
    }
    lead.extend(words);
    lead.extend(sentences);
    lead.extend(prefixes);
    lead.extend(tail);
    *candidates = lead;
}

fn decoder_candidate(item: &MixedCandidate) -> bool {
    !matches!(item.selection, Selection::Local { .. })
        || matches!(item.candidate.comment.as_str(), ZHIXU_WHOLE_COMMENT | ZHIXU_WHOLE_SENTENCE_COMMENT)
}

/// Model-first group (C2): every visible Han decoder candidate whose reading covers the
/// whole raw input, in full syllables or the owner's syllable-prefix abbreviations
/// (hais -> 还是 / 还剩, jiexialzuoshenm -> 接下来做什么 / 解析阿里做什么). It contains the
/// C1 homophone group. Whole-sentence compositions from the heavy-garble decoder join;
/// other local entries (project, English, owner-confirmed or daily abbreviations) never
/// do, and the first slot must be eligible so the model only re-decides among complete
/// readings of the same input.
fn context_complete_group(
    snapshot: &DecoderSnapshot,
    mixed: &[MixedCandidate],
    has_context: bool,
) -> u32 {
    let han = |text: &str| {
        !text.is_empty() && text.chars().all(|c| ('\u{3400}'..='\u{9fff}').contains(&c))
    };
    let raw = canonical_reading(&snapshot.raw_input).unwrap_or_default();
    if raw.is_empty() {
        return 0;
    }
    let covers = |item: &MixedCandidate| {
        let c = &item.candidate;
        (decoder_candidate(item)
            || c.comment == HEAVY_GARBLE_COMMENT
            || c.comment == SLIP_COMMENT)
            && han(&c.text)
            && (canonical_reading(&c.reading_key).as_deref() == Some(raw.as_str())
                || syllable_prefix_abbreviation(&raw, &c.reading_key))
    };
    if !mixed.first().is_some_and(covers) {
        return 0;
    }
    let mask = mixed.iter().enumerate().fold(0u32, |mask, (index, item)| {
        if index < 8 && covers(item) && (index == 0 || common_simplified(&item.candidate.text)) {
            mask | (1 << index)
        } else {
            mask
        }
    });
    mask | context_group(snapshot, mixed, has_context)
}

fn context_group(snapshot: &DecoderSnapshot, mixed: &[MixedCandidate], has_context: bool) -> u32 {
    let Some(first) = snapshot.candidates.first() else {
        return 0;
    };
    let han = |text: &str| {
        !text.is_empty() && text.chars().all(|c| ('\u{3400}'..='\u{9fff}').contains(&c))
    };
    if !han(&first.text)
        || mixed
            .first()
            .is_none_or(|item| !decoder_candidate(item))
    {
        return 0;
    }
    let readings = chinese_syllabified_readings_by_text();
    let Some(first_readings) = readings.get(&first.text) else {
        return 0;
    };
    let raw = canonical_reading(&snapshot.raw_input).unwrap_or_default();
    let full = first_readings
        .iter()
        .filter(|(compact, syllables)| {
            compact == &raw && syllables.split_whitespace().count() == first.text.chars().count()
        })
        .collect::<Vec<_>>();
    if full.is_empty() {
        if raw.len() > 2 && first.text.chars().count() >= 2 {
            return abbreviation_homophone_group(&raw, &first.text, first_readings, mixed, has_context);
        }
        return initial_homophone_group(&raw, &first.text, first_readings, mixed, has_context);
    }
    mixed.iter().enumerate().fold(0, |mask, (index, item)| {
        let c = &item.candidate;
        if index < 8
            && decoder_candidate(item)
            && han(&c.text)
            && (index == 0 || common_simplified(&c.text))
            && c.text.chars().count() == first.text.chars().count()
            && canonical_reading(&c.reading_key).as_deref() == Some(raw.as_str())
            && readings
                .get(&c.text)
                .is_some_and(|values| values.iter().any(|value| full.contains(&value)))
        {
            mask | (1 << index)
        } else {
            mask
        }
    })
}

/// A bare initial leads with the character the owner settled on for it in their own
/// records (`ranking::OwnerStyle`), not with whatever was picked last: one pick of 捞
/// made Rime's recency put it ahead of 了 for every later l (owner, 2026-10-10).
fn lead_with_owner_letter(raw: &str, output: &mut Vec<MixedCandidate>) {
    if raw.len() != 1 || !raw.bytes().all(|b| b.is_ascii_lowercase()) {
        return;
    }
    let Some(text) = crate::ranking::owner_letter(raw) else { return };
    if let Some(index) = output.iter().take(PAGE_SIZE).position(|item| item.candidate.text == text)
        && index > 0
    {
        let item = output.remove(index);
        output.insert(0, item);
    }
}

/// Every character is one of the 6,763 GB2312 simplified characters. Traditional and
/// variant forms (喲, 來) stay visible where the decoder put them, but the context
/// model never lifts them over the first (owner record: yo -> 喲 retyped).
fn common_simplified(text: &str) -> bool {
    static CHARS: OnceLock<HashSet<char>> = OnceLock::new();
    let chars = CHARS.get_or_init(|| include_str!("../resources/gb2312_han.txt").trim().chars().collect());
    text.chars()
        .all(|c| !('\u{3400}'..='\u{9fff}').contains(&c) || chars.contains(&c))
}

/// A bare initial (`z`, `b`) whose first candidate is one character: the visible
/// single characters sharing that character's syllable (在/再 for zai, 吧/把 for
/// ba) form a homophone group, so the model rather than the last pick decides.
/// Owner records: Rime's recency order flipped 在/再 and the owner committed the
/// wrong one silently (「一直再调」「还是再这个」). Never without preceding text,
/// and other syllables under the same initial (做 for z) keep their order.
/// Letters that are not whole pinyin spell several words equally (hais: 还是 hai shi,
/// 还剩 hai sheng, 还说 hai shuo; each typed syllable whole or each a beginning in the
/// same places): Rime's order among them is only its recency, so they form the
/// homophone group the context decides with the light rank penalty (owner 2026-10-11:
/// hais 还剩 first after one recent pick, 还是 is meant; the model preferred 还是 by 3-8
/// in every recorded context, short of the 6 a different reading needs). A first whose
/// letters spell it differently (che ji: 车机 whole, 趁机 chen begun) stays apart.
fn abbreviation_homophone_group(
    raw: &str,
    first_text: &str,
    first_readings: &[(String, String)],
    mixed: &[MixedCandidate],
    has_context: bool,
) -> u32 {
    let han = |text: &str| !text.is_empty() && text.chars().all(|c| ('\u{3400}'..='\u{9fff}').contains(&c));
    if !has_context || !han(first_text) {
        return 0;
    }
    let characters = first_text.chars().count();
    let spellings = |readings: &[(String, String)]| {
        readings
            .iter()
            .filter(|(_, spaced)| spaced.split_whitespace().count() == characters)
            .flat_map(|(_, spaced)| typed_spellings(raw, &spaced.split_whitespace().collect::<Vec<_>>()))
            .collect::<HashSet<Vec<bool>>>()
    };
    let first = spellings(first_readings);
    if first.is_empty() {
        return 0;
    }
    let readings = chinese_syllabified_readings_by_text();
    let mask = mixed.iter().enumerate().fold(0u32, |mask, (index, item)| {
        let c = &item.candidate;
        if index < 8
            && !matches!(item.selection, Selection::Local { .. })
            && han(&c.text)
            && c.text.chars().count() == characters
            && (index == 0 || common_simplified(&c.text))
            && readings.get(&c.text).is_some_and(|values| !spellings(values).is_disjoint(&first))
        {
            mask | (1 << index)
        } else {
            mask
        }
    });
    if mask & 1 == 0 || mask.count_ones() < 2 { 0 } else { mask }
}

/// Every way `raw` spells these syllables, each syllable by a non-empty beginning of it:
/// true where the whole syllable was typed.
fn typed_spellings(raw: &str, syllables: &[&str]) -> Vec<Vec<bool>> {
    fn walk(raw: &str, syllables: &[&str], typed: &mut Vec<bool>, out: &mut Vec<Vec<bool>>) {
        let Some((syllable, rest)) = syllables.split_first() else {
            if raw.is_empty() {
                out.push(typed.clone());
            }
            return;
        };
        for end in 1..=syllable.len().min(raw.len()) {
            if syllable.starts_with(&raw[..end]) {
                typed.push(end == syllable.len());
                walk(&raw[end..], rest, typed, out);
                typed.pop();
            }
        }
    }
    let mut out = Vec::new();
    walk(raw, syllables, &mut Vec::new(), &mut out);
    out
}

fn initial_homophone_group(
    raw: &str,
    first_text: &str,
    first_readings: &[(String, String)],
    mixed: &[MixedCandidate],
    has_context: bool,
) -> u32 {
    let han = |text: &str| {
        text.chars().count() == 1 && text.chars().all(|c| ('\u{3400}'..='\u{9fff}').contains(&c))
    };
    if !has_context || raw.is_empty() || raw.len() > 2 || !han(first_text) {
        return 0;
    }
    let syllables = first_readings
        .iter()
        .filter(|(compact, spaced)| {
            compact != raw && compact.starts_with(raw) && spaced.split_whitespace().count() == 1
        })
        .map(|(compact, _)| compact.as_str())
        .collect::<Vec<_>>();
    if syllables.is_empty() {
        return 0;
    }
    let readings = chinese_syllabified_readings_by_text();
    let mask = mixed.iter().enumerate().fold(0u32, |mask, (index, item)| {
        let c = &item.candidate;
        if index < 8
            && !matches!(item.selection, Selection::Local { .. })
            && han(&c.text)
            && (index == 0 || common_simplified(&c.text))
            && readings.get(&c.text).is_some_and(|values| {
                values.iter().any(|(compact, _)| syllables.contains(&compact.as_str()))
            })
        {
            mask | (1 << index)
        } else {
            mask
        }
    });
    if mask & 1 == 0 { 0 } else { mask }
}

impl Decoder for MixerDecoder {
    fn native_editing(&self) -> bool { self.inner.native_editing() }

    fn composition_boundary(&self, purpose: BoundaryPurpose) -> Option<CompositionBoundary> {
        if self.inner.native_editing() {
            self.inner.composition_boundary(purpose)
        } else {
            crate::state::default_composition_boundary(&self.snapshot())
        }
    }

    fn configure_zhixu_whole(&mut self, _on: bool) -> Result<(), i32> {
        // The shells still call this knob, but the inner decoder is native for
        // the product session's whole life; there is nothing left to switch.
        // A native session rejects the call mid-composition, as the switched-on
        // product always did; legacy probes keep their lenient path.
        if self.inner.native_editing() && self.is_composing() { return Err(-10); }
        if let Ok(cache) = self.whole_lattice.get_mut() { *cache = None; }
        self.reset_layer();
        if let Ok(mut cache) = self.composition_cache.lock() { *cache = None; }
        Ok(())
    }

    fn configure_learning(&mut self, mode: LearningMode) -> Result<(), i32> {
        // A native inner only ever learns in Zhixu; no Rime decoder remains to
        // keep a legacy mode for.
        let mode = if self.inner.native_editing() { LearningMode::Zhixu } else { mode };
        self.learning.set_mode(mode, &zhixu::current());
        if mode == LearningMode::Zhixu {
            let directory = self.blacklist.lock().ok().and_then(|store| store.directory());
            if let Some(directory) = directory {
                let now = personal_picks::now_ms();
                let choices = personal_picks::load(&directory).into_iter()
                    .filter(|entry| now.saturating_sub(entry.last_ms) <= personal_picks::MAX_AGE_MS)
                    .filter_map(|entry| {
                        let (reading, consumed) = resolve_learning_reading(&entry.reading, &entry.text)?;
                        let raw = entry.reading.replace([' ', '\''], "");
                        if consumed != raw.len() { return None; }
                        Some(Choice { app: entry.app, raw, reading, text: entry.text, target: entry.target,
                            count: entry.count, last_ms: entry.last_ms, alternatives: Vec::new() })
                    }).collect();
                self.learning.import_choices(choices);
            }
        }
        self.reset_layer();
        if let Ok(mut cache) = self.composition_cache.lock() { *cache = None; }
        Ok(())
    }

    fn learning_key(&mut self, event: &crate::state::KeyEvent, now: u64) {
        self.learning.key(event, self.is_composing(), now);
    }

    fn learning_commit(&mut self, text: &str, delivery_id: u64, now: u64) {
        self.learning.commit(text, delivery_id, now);
    }

    fn withdraw_learning(&mut self, raw: &str, text: &str) -> bool {
        self.learning.withdraw(raw, text)
    }

    fn corrections_changed(&mut self) {
        self.reset_layer();
        if let Ok(mut cache) = self.composition_cache.lock() { *cache = None; }
    }

    fn composition_alternatives(&self) -> Vec<String> {
        self.compositions().0.into_iter().map(|(text, _)| text).collect()
    }

    fn composition_edits(&self) -> Vec<f64> {
        let (alternatives, first_edits) = self.compositions();
        if alternatives.is_empty() {
            return Vec::new();
        }
        std::iter::once(first_edits).chain(alternatives.into_iter().map(|(_, edits)| edits)).collect()
    }

    /// Not whole pinyin, and the first is not one word the letters abbreviate: a first
    /// such as 知识库 (zhishik) or 这个 (zhge) keeps the full margin (owner replay at 6:
    /// 只是看, 找个), while a guessed sentence (之前的 over 支球队 for zhiqd) still leads.
    fn composition_guessed(&self) -> bool {
        let _dictionary = self.pin_dictionary();
        let _ = self.compositions();
        let Ok(cache) = self.composition_cache.lock() else { return false };
        cache
            .as_ref()
            .and_then(|((_, raw, _, first, _), _, _)| {
                // An apostrophe only marks a syllable boundary (xi'anj, zhongguo').
                let reading = canonical_reading(raw)?.replace('\'', "");
                let (first, _, _) = first.as_ref()?;
                Some(
                    !valid_pinyin_spelling(&reading)
                        && (all_initials(&reading) || !abbreviated_vocabulary_word(first, &reading)),
                )
            })
            .unwrap_or(false)
    }

    /// C3: lets a model-composed sentence lead the page. **The rule**: it leads only when it really is first on
    /// the page the digit keys resolve against (`visible()`). The owner's learned whole word outranks it
    /// (`protect_learned_whole`), and then the call fails with `LEAD_OUTRANKED_BY_LEARNED_WHOLE` and the page stays
    /// as the core has it; the shell may still offer the sentence in the last slot. Never report success for a lead
    /// the page does not show (11-07: the shell drew the sentence first while digit 2 committed it).
    fn lead_with_composition(&mut self, text: &str) -> Result<(), i32> {
        if self.learning.mode == LearningMode::Zhixu {
            let (snapshot, mut page) = self.visible();
            page.insert(0, MixedCandidate { candidate: Candidate { text: text.into(), comment: String::new(),
                reading_key: String::new(), source_index: None }, selection: Selection::Local { text: text.into() }, tier: 0, score: 0 });
            self.protect_learned_whole(&snapshot.raw_input, &mut page);
            if let Some(first) = page.first().filter(|item| item.candidate.text != text) {
                return Err(self.lead_refusal(&snapshot.raw_input, &first.candidate.text));
            }
        }
        if !self.composition_alternatives().iter().any(|value| value == text) {
            return Err(LEAD_NOT_AN_ALTERNATIVE);
        }
        let raw = self.letters_left();
        let (previous_order, previous_lead) = (self.host_order.take(), self.lead_composition.take());
        self.lead_composition = Some((raw.clone(), text.to_string()));
        if self.learning.mode == LearningMode::Zhixu {
            // The simulation above does not see everything `visible()` does. Only report success when the page
            // really starts with the sentence, or the shell shows one order and the digit keys act on another.
            let (snapshot, page) = self.visible();
            if let Some(first) = page.first().filter(|item| item.candidate.text != text) {
                let code = self.lead_refusal(&snapshot.raw_input, &first.candidate.text);
                self.host_order = previous_order;
                self.lead_composition = previous_lead;
                return Err(code);
            }
        }
        Ok(())
    }

    fn offer_composition(&mut self, text: &str) -> Result<(), i32> {
        if !self.composition_alternatives().iter().any(|value| value == text) {
            return Err(-10);
        }
        if self.learning.mode == LearningMode::Zhixu {
            let (snapshot, mut page) = self.visible();
            if !page.iter().any(|item| item.candidate.text == text) {
                page.insert((PAGE_SIZE - 1).min(page.len()), MixedCandidate {
                    candidate: Candidate { text: text.into(), comment: ranking::COMPOSITION_OFFER_COMMENT.into(), reading_key: snapshot.raw_input.clone(), source_index: None },
                    selection: Selection::Local { text: text.into() }, tier: 0, score: 0,
                });
                page.truncate(PAGE_SIZE);
                let requested: Vec<_> = page.iter().map(|item| item.candidate.clone()).collect();
                self.protect_learned_whole(&snapshot.raw_input, &mut page);
                if page.iter().map(|item| &item.candidate).ne(requested.iter()) { return Err(-10); }
            }
        }
        self.offered_composition = Some((self.letters_left(), text.to_string()));
        Ok(())
    }

    fn peek_following_pages(&mut self, count: usize) -> Vec<Vec<Candidate>> {
        // Real paging is the only definition of a page (fuzzy pages, the deep
        // fallback after page 2, local insertions), so walk it and walk back.
        let saved_order = self.host_order.clone();
        let saved_highlight = self.snapshot().highlighted_index;
        let mut pages = Vec::new();
        let mut moved = 0;
        while pages.len() < count && !self.snapshot().is_last_page {
            let before = self.snapshot().candidates;
            if self.page_down().is_err() {
                break;
            }
            moved += 1;
            let page = self.snapshot().candidates;
            if page.is_empty() || page == before {
                break;
            }
            pages.push(page);
        }
        for _ in 0..moved {
            let _ = self.page_up();
        }
        self.host_order = saved_order;
        // Paging keeps the highlighted column (librime) and Down on the last
        // candidate rolls a page: restore the delta instead of assuming 0.
        let arrived = self.snapshot().highlighted_index;
        if arrived > saved_highlight {
            for _ in 0..(arrived - saved_highlight) {
                let _ = self.select_previous();
            }
        } else {
            for _ in 0..(saved_highlight - arrived) {
                let _ = self.select_next();
            }
        }
        pages
    }

    fn context_group_mask(&self) -> u32 {
        let (snapshot, mixed) = self.visible_unranked();
        context_group(&snapshot, &mixed, self.has_preceding_context())
    }

    fn context_complete_mask(&self) -> u32 {
        let (snapshot, mixed) = self.visible_unranked();
        context_complete_group(&snapshot, &mixed, self.has_preceding_context())
    }

    fn reorder_context_candidates(&mut self, order: &[usize]) -> Result<(), i32> {
        let (snapshot, mixed) = self.visible_unranked();
        // The complete-coverage group contains the homophone group, so one check
        // admits C1 swaps and C2 promotions while every other slot stays fixed.
        let has_context = self.has_preceding_context();
        let mask = context_complete_group(&snapshot, &mixed, has_context)
            | context_group(&snapshot, &mixed, has_context);
        if mask.count_ones() < 2 || order.len() != mixed.len() {
            return Err(-10);
        }
        let mut seen = 0u32;
        for (slot, &source) in order.iter().enumerate() {
            if source >= mixed.len()
                || seen & (1 << source) != 0
                || (mask & (1 << slot) == 0 && source != slot)
                || (mask & (1 << slot) != 0 && mask & (1 << source) == 0)
            {
                return Err(-10);
            }
            seen |= 1 << source;
        }
        let mut ordered: Vec<_> = order.iter().map(|&index| mixed[index].clone()).collect();
        let requested: Vec<_> = ordered.iter().map(|item| item.candidate.clone()).collect();
        self.protect_learned_whole(&snapshot.raw_input, &mut ordered);
        if ordered.iter().map(|item| &item.candidate).ne(requested.iter()) { return Err(-10); }
        self.host_order = Some((
            snapshot.raw_input,
            snapshot.page_no,
            snapshot.candidates,
            ordered,
        ));
        Ok(())
    }

    fn is_composing(&self) -> bool {
        // Mirrors visible(): a raw override replaces the inner raw input
        // wholesale, so composition state follows it exclusively. Never runs
        // the ranking pipeline.
        match &self.raw_override {
            Some(raw) => !raw.is_empty(),
            None => self.inner.is_composing(),
        }
    }

    fn snapshot(&self) -> DecoderSnapshot {
        self.visible().0
    }

    fn snapshot_all_candidates(&self) -> Option<DecoderSnapshot> {
        if !self.whole_enabled() { return None; }
        let _dictionary = self.pin_dictionary();
        let mut base = self.whole_base()?;
        if !base.is_last_page { base = self.extend_whole_prefix(&base, usize::MAX)?; }
        self.complete_whole_generation(&base);
        let mut all = (*base).clone();
        all.candidates = self.whole_ranked(&base).iter().map(|item| item.candidate.clone()).collect();
        let page = self.visible().0;
        let start = page.page_no * PAGE_SIZE;
        if start < all.candidates.len() {
            let end = (start + PAGE_SIZE).min(all.candidates.len());
            all.candidates.splice(start..end, page.candidates);
        }
        all.highlighted_index = 0;
        all.secondary_candidates = self.secondary_layer(&all).into_iter().map(|item| item.candidate).collect();
        Some(all)
    }

    fn input(&mut self, text: &str) -> Result<String, i32> {
        let prefix = if self.whole_enabled() && text.len() == 1 && text.as_bytes()[0].is_ascii_alphabetic()
            && self.raw_override.is_none()
        {
            self.whole_lattice.lock().ok().and_then(|mut cache| cache.take()).filter(|_| {
                let base = self.inner.snapshot();
                base.cursor_utf8 == base.preedit.len()
            })
        } else { None };
        self.reset_layer();
        if text.len() == 1 && text.as_bytes()[0].is_ascii_digit() {
            let prefix = self
                .raw_override
                .clone()
                .unwrap_or_else(|| self.typed_ascii.clone());
            let proposed = format!("{prefix}{text}").to_ascii_lowercase();
            self.typed_ascii = proposed.clone();
            self.raw_override = Some(proposed);
            return Ok(String::new());
        }
        let result = self.inner.input(text);
        if result.is_ok() && text.len() == 1 && text.as_bytes()[0].is_ascii_alphabetic() {
            self.typed_ascii
                .push(text.as_bytes()[0].to_ascii_lowercase() as char);
            if prefix.is_some() && let Ok(mut cache) = self.whole_lattice.lock() { *cache = prefix; }
        }
        result
    }
    fn backspace(&mut self) -> Result<String, i32> {
        self.reset_layer();
        if let Some(mut raw) = self.raw_override.take() {
            // Pop by char, not by byte slice: raw_override is ASCII today, but
            // a byte slice would panic across a future multi-byte boundary and
            // abort the host mid-keystroke.
            raw.pop();
            self.typed_ascii = raw.clone();
            if raw != self.inner.snapshot().raw_input {
                self.raw_override = Some(raw);
            }
            return Ok(String::new());
        }
        let result = self.inner.backspace();
        if result.is_ok() {
            self.typed_ascii.pop();
        }
        result
    }
    fn move_left(&mut self) -> Result<String, i32> {
        if !self.inner.native_editing() { self.host_order = None; }
        self.inner.move_left()
    }
    fn move_right(&mut self) -> Result<String, i32> {
        if !self.inner.native_editing() { self.host_order = None; }
        self.inner.move_right()
    }
    fn page_up(&mut self) -> Result<String, i32> {
        if self.at_native_edit_boundary() { return Ok(String::new()); }
        let column = self.inner.native_editing().then(|| self.snapshot())
            .filter(|page| page.page_no > 0).map(|page| page.highlighted_index);
        self.host_order = None;
        if self.whole_base().is_some() {
            self.whole_page_no = self.whole_page_no.saturating_sub(1);
            self.whole_highlighted = 0;
            return self.keep_native_page_column(column);
        }
        if let Some(page_no) = self.deep_page_no {
            if page_no > self.deep_base {
                self.deep_page_no = Some(page_no - 1);
            } else {
                self.deep_page_no = None;
            }
            self.deep_highlighted = 0;
            return self.keep_native_page_column(column);
        }
        let result = if self.fuzzy_mask == 0 {
            self.inner.page_up()
        } else {
            self.fuzzy_page_no = self.fuzzy_page_no.saturating_sub(1);
            self.fuzzy_highlighted = 0;
            Ok(String::new())
        };
        if column.is_some() { result?; self.keep_native_page_column(column) } else { result }
    }
    fn page_down(&mut self) -> Result<String, i32> {
        if self.at_native_edit_boundary() { return Ok(String::new()); }
        let column = self.inner.native_editing().then(|| self.snapshot().highlighted_index);
        self.host_order = None;
        if self.whole_base().is_some() {
            if !self.snapshot().is_last_page { self.whole_page_no += 1; }
            self.whole_highlighted = 0;
            return self.keep_native_page_column(column);
        }
        if let Some(page_no) = self.deep_page_no {
            if !self.snapshot().is_last_page {
                self.deep_page_no = Some(page_no + 1);
            }
            self.deep_highlighted = 0;
            return self.keep_native_page_column(column);
        }
        let result = if self.fuzzy_mask == 0 {
            let page_no = self.inner.snapshot().page_no;
            if page_no <= 1 {
                self.remember_visible_page();
            }
            if page_no == 1 && self.prepare_deep_fallback() {
                Ok(String::new())
            } else if self.inner.snapshot().is_last_page && self.prepare_overflow_page() {
                Ok(String::new())
            } else {
                self.inner.page_down()
            }
        } else {
            if !self.snapshot().is_last_page {
                self.fuzzy_page_no += 1;
            }
            self.fuzzy_highlighted = 0;
            Ok(String::new())
        };
        if column.is_some() { result?; self.keep_native_page_column(column) } else { result }
    }
    fn select_previous(&mut self) -> Result<String, i32> {
        if self.at_native_edit_boundary() { return Ok(String::new()); }
        if self.whole_base().is_some() {
            self.whole_highlighted = self.whole_highlighted.saturating_sub(1);
            return Ok(String::new());
        }
        if self.deep_page_no.is_some() {
            self.deep_highlighted = self.deep_highlighted.saturating_sub(1);
            return Ok(String::new());
        }
        if self.fuzzy_mask == 0 {
            self.inner.select_previous()
        } else {
            self.fuzzy_highlighted = self.fuzzy_highlighted.saturating_sub(1);
            Ok(String::new())
        }
    }
    fn select_next(&mut self) -> Result<String, i32> {
        if self.at_native_edit_boundary() { return Ok(String::new()); }
        if self.whole_base().is_some() {
            let last = self.snapshot().candidates.len().saturating_sub(1);
            self.whole_highlighted = (self.whole_highlighted + 1).min(last);
            return Ok(String::new());
        }
        if self.deep_page_no.is_some() {
            let last = self.snapshot().candidates.len().saturating_sub(1);
            self.deep_highlighted = (self.deep_highlighted + 1).min(last);
            return Ok(String::new());
        }
        if self.fuzzy_mask == 0 {
            self.inner.select_next()
        } else {
            let last = self.snapshot().candidates.len().saturating_sub(1);
            self.fuzzy_highlighted = (self.fuzzy_highlighted + 1).min(last);
            Ok(String::new())
        }
    }
    fn select_previous_linear(&mut self) -> Result<String, i32> {
        if self.at_native_edit_boundary() { return Ok(String::new()); }
        if self.deep_page_no.is_none() && self.fuzzy_mask == 0 && self.whole_base().is_none() {
            return self.inner.select_previous();
        }
        let before = self.snapshot();
        if before.highlighted_index > 0 {
            if self.whole_base().is_some() { self.whole_highlighted -= 1; }
            else if self.deep_page_no.is_some() { self.deep_highlighted -= 1; }
            else { self.fuzzy_highlighted -= 1; }
        } else if before.page_no > 0 {
            // Use the existing pager, including the deep -> native transition.
            self.page_up()?;
            let last = self.snapshot().candidates.len().saturating_sub(1);
            let arrived = self.snapshot().highlighted_index;
            for _ in arrived..last { self.select_next()?; }
        }
        Ok(String::new())
    }
    fn select_next_linear(&mut self) -> Result<String, i32> {
        if self.at_native_edit_boundary() { return Ok(String::new()); }
        let before = self.snapshot();
        if self.deep_page_no.is_none() && self.fuzzy_mask == 0 && self.whole_base().is_none() {
            // The native Down key bypasses Mixer's deep/overflow pager. Use it
            // only inside a row, and let page_down own every forward boundary.
            if before.highlighted_index + 1 < before.candidates.len() {
                return self.inner.select_next();
            }
            if before.is_last_page && !self.has_linear_overflow(&before) {
                return Ok(String::new());
            }
        } else if before.highlighted_index + 1 < before.candidates.len() {
            if self.whole_base().is_some() { self.whole_highlighted += 1; }
            else if self.deep_page_no.is_some() { self.deep_highlighted += 1; }
            else { self.fuzzy_highlighted += 1; }
            return Ok(String::new());
        } else if before.is_last_page {
            return Ok(String::new());
        }
        self.page_down()?;
        if self.snapshot().page_no != before.page_no {
            // Native paging keeps its column; fuzzy/deep paging resets it.
            let arrived = self.snapshot().highlighted_index;
            for _ in 0..arrived { self.select_previous()?; }
        }
        Ok(String::new())
    }
    fn select_candidate(
        &mut self,
        index: usize,
        delivery_id: u64,
        learning_intent: LearningIntent,
    ) -> Result<String, i32> {
        let (snapshot, mixed) = self.visible();
        let selected = mixed.get(index).cloned().ok_or(-10)?;
        let choice = if self.learning.mode == LearningMode::Zhixu {
            let _dictionary = self.pin_dictionary();
            self.learning_choice(&snapshot.raw_input, &selected.candidate, snapshot.page_no * PAGE_SIZE + index, &snapshot.candidates)
        } else { None };
        if let Ok(mut learning) = shared_learning().lock() { learning.undo = None; }
        if learning_intent == LearningIntent::DeliberateSelection {
            self.learn_english_choice(&snapshot.raw_input, &selected.candidate.text);
        }
        let result = match selected.selection {
            Selection::Base {
                index: base_index,
                text,
            } => {
                if learning_intent == LearningIntent::DeliberateSelection {
                    self.record_selection(&snapshot.raw_input, index, &text);
                }
                self.reset_layer();
                self.typed_ascii.clear();
                self.inner
                    .select_candidate(base_index, delivery_id, learning_intent)
            }
            Selection::GlobalBase {
                index: base_index,
                text,
            } => {
                let position = snapshot.page_no.saturating_mul(PAGE_SIZE) + index;
                if learning_intent == LearningIntent::DeliberateSelection {
                    self.record_selection(&snapshot.raw_input, position, &text);
                }
                self.reset_layer();
                self.typed_ascii.clear();
                self.inner
                    .select_candidate_global(base_index, delivery_id, learning_intent)
            }
            Selection::Local { text } => {
                // Rime's `clear` drops the segment picked earlier in this composition: keep it.
                let confirmed = self.confirmed_boundary()
                    .map(|boundary| boundary.confirmed)
                    .unwrap_or_default();
                self.inner.clear();
                self.typed_ascii.clear();
                self.raw_override = None;
                self.reset_layer();
                Ok(format!("{confirmed}{text}"))
            }
        };
        if result.is_ok() && self.learning.mode == LearningMode::Zhixu {
            self.learning.stage(choice, delivery_id);
        } else if result.is_err() {
            self.learning.clear_segments();
        }
        result
    }
    fn select_secondary_candidate(
        &mut self,
        index: usize,
        _delivery_id: u64,
    ) -> Result<String, i32> {
        let mut base = self.inner.snapshot();
        // The page shows these for the letters left after a segment picked earlier; look them up the same way.
        if self.raw_override.is_none()
            && let Some(boundary) = self.confirmed_boundary()
        {
            base.raw_input = boundary.unconfirmed_raw;
        }
        let selected = self.secondary_layer(&base).get(index).cloned().ok_or(-10)?;
        match selected.selection {
            // Same full reset as the Local branch of select_candidate: a stale
            // raw_override from digit extension would otherwise shadow new
            // input and make a later commit_raw replay the abandoned string.
            Selection::Local { text } => {
                let confirmed = self.confirmed_boundary()
                    .map(|boundary| boundary.confirmed)
                    .unwrap_or_default();
                self.inner.clear();
                self.typed_ascii.clear();
                self.raw_override = None;
                self.reset_layer();
                Ok(format!("{confirmed}{text}"))
            }
            Selection::Base { .. } | Selection::GlobalBase { .. } => Err(-206),
        }
    }
    fn delete_candidate(&mut self, index: usize) -> Result<(), i32> {
        let (snapshot, mixed) = self.visible();
        if self.inner.native_editing()
            && let Some(item) = mixed.get(index)
            && !matches!(item.selection, Selection::Local { .. })
        {
            self.learning.forget_pending(&item.candidate.reading_key, &item.candidate.text);
        }
        match mixed.get(index).map(|item| &item.selection) {
            Some(Selection::Base { index, text }) => {
                self.inner.delete_candidate(*index)?;
                self.forget_selection(&snapshot.raw_input, text);
                self.invalidate_whole();
                Ok(())
            }
            Some(Selection::GlobalBase { index, text }) => {
                self.inner.delete_candidate_global(*index)?;
                self.forget_selection(&snapshot.raw_input, text);
                self.invalidate_whole();
                Ok(())
            }
            Some(Selection::Local { .. }) => Err(-206),
            None => Err(-10),
        }
    }
    fn blacklist_candidate(&mut self, index: usize) -> Result<(), i32> {
        let (snapshot, mixed) = self.visible();
        let selected = mixed.get(index).ok_or(-10)?;
        let text = selected.candidate.text.clone();
        let reading = canonical_reading(&selected.candidate.reading_key)
            .filter(|value| !value.is_empty())
            .or_else(|| canonical_reading(&snapshot.raw_input))
            .ok_or(-10)?;
        let mut blacklist = self.blacklist.lock().map_err(|_| -207)?;
        blacklist.insert(&reading, &text).map_err(|_| -207)?;
        drop(blacklist);
        self.forget_selection(&snapshot.raw_input, &text);
        self.invalidate_whole();
        Ok(())
    }
    fn commit_raw(&mut self) -> String {
        self.learning.clear_segments();
        if let Ok(mut learning) = shared_learning().lock() { learning.undo = None; }
        self.reset_layer();
        if let Some(raw) = self.raw_override.take() {
            self.inner.clear();
            self.typed_ascii.clear();
            return raw;
        }
        self.typed_ascii.clear();
        let committed = self.inner.commit_raw();
        // Enter on an English word is as deliberate as picking it (owner: mini, agent
        // committed with Enter three times and never learned). Only real English words:
        // raw pinyin committed this way is usually a typo about to be retyped. A word
        // that is also whole pinyin (mini, long, hang) leads over its Chinese from then
        // on, so it is remembered on the second Enter, not the first.
        if committed.len() >= 4
            && committed.bytes().all(|byte| byte.is_ascii_lowercase())
            && english_word_parts(&committed)
            && let Ok(mut choices) = self.english_choices.lock()
            && (!valid_pinyin_spelling(&committed) || choices.note_enter(&committed))
        {
            let _ = choices.remember(&committed, &committed);
            whole_preferences_changed();
        }
        committed
    }
    fn clear(&mut self) {
        self.learning.clear_segments();
        if let Ok(mut learning) = shared_learning().lock() {
            learning.undo = None;
        }
        self.clear_after_selection();
    }
    fn clear_after_selection(&mut self) {
        self.reset_layer();
        self.typed_ascii.clear();
        self.raw_override = None;
        self.inner.clear();
    }
    fn configure_fuzzy(&mut self, mask: u32) -> Result<(), i32> {
        // The native decoder reads fuzzy spellings itself; a live menu must not change.
        if self.inner.native_editing() && self.is_composing() { return Err(-10); }
        self.reset_layer();
        self.inner.configure_fuzzy(mask)?;
        self.fuzzy_mask = mask;
        Ok(())
    }
    fn reload_dictionaries(&mut self) -> Result<(), i32> {
        self.reset_layer();
        // A native inner rebuilds and validates its own generation synchronously.
        self.inner.reload_dictionaries()
    }
    fn apply_lexicon(&mut self, lexicon: Arc<MixerLexicon>) {
        self.lexicon = lexicon;
        self.reset_layer();
    }
    fn apply_context_model(&mut self, model: Arc<ContextModel>) {
        self.context_model = model;
        self.reset_layer();
    }
    fn configure_preceding_text(&mut self, preceding_text: Option<String>) {
        self.preceding_text = preceding_text;
        self.reset_layer();
    }
    fn configure_typo(&mut self, enabled: bool) {
        self.typo_enabled = enabled;
        self.reset_layer();
    }
    fn configure_code_ascii_bias(&mut self, enabled: bool) {
        self.code_ascii_bias = enabled;
        self.reset_layer();
    }
    fn configure_candidate_context(
        &mut self,
        active_project: Option<String>,
        active_packs: Vec<String>,
        secondary_enabled: bool,
        prediction_enabled: bool,
    ) {
        self.active_project = active_project;
        self.active_packs = active_packs.into_iter().collect();
        self.secondary_enabled = secondary_enabled;
        self.prediction_enabled = prediction_enabled;
        self.reset_layer();
    }
}

fn canonical_nonempty(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty() && trimmed == value).then(|| value.to_string())
}

fn project_term_readings() -> Arc<HashMap<String, String>> {
    zhixu::terms().project.clone()
}

fn project_term_for_reading(reading: &str) -> Option<String> {
    project_term_readings().get(&reading.to_ascii_lowercase()).cloned()
}

fn english_term_readings() -> Arc<HashMap<String, Vec<String>>> {
    zhixu::terms().english.clone()
}

fn tech_term_readings() -> Arc<HashMap<String, Vec<String>>> {
    zhixu::terms().tech.clone()
}

fn daily_abbreviations() -> &'static HashMap<String, (String, String, i64)> {
    static TERMS: OnceLock<HashMap<String, (String, String, i64)>> = OnceLock::new();
    TERMS.get_or_init(|| {
        include_str!("../../../resources/Rime/agent_daily.dict.yaml")
            .lines()
            .filter(|line| !line.starts_with(['#', '-', '.']) && line.contains('\t'))
            .filter_map(|line| {
                let mut columns = line.split('\t');
                let text = columns.next()?.trim();
                let raw_reading = columns.next()?.trim();
                let weight = columns.next()?.trim().parse::<i64>().ok()?;
                let syllables = raw_reading.split_whitespace().collect::<Vec<_>>();
                (syllables.len() > 1).then(|| {
                    let abbreviation = syllables
                        .iter()
                        .filter_map(|syllable| syllable.chars().next())
                        .collect::<String>();
                    (abbreviation, (text.to_string(), syllables.concat(), weight))
                })
            })
            .collect()
    })
}

fn daily_abbreviation_for(raw: &str) -> Option<&'static (String, String, i64)> {
    daily_abbreviations().get(&raw.to_ascii_lowercase())
}

/// Keep the existing C3 ABI's multiplier: fractional edits carry cost / multiplier.
/// With a zero override the C3 multiplier is zero; retain old heavy-path units.
fn edit_units(kind: EditKind) -> f64 {
    let penalty = ranking::edit_penalty();
    if penalty == 0.0 { 1.0 } else { ranking::typed_edit_cost(kind) / penalty }
}

fn repair_edit_units(typed: &str, corrected: &str) -> f64 {
    let penalty = ranking::edit_penalty();
    match ranking::typed_repair_cost(typed, corrected) {
        Some(_) if penalty == 0.0 => 1.0,
        Some(cost) => cost / penalty,
        None => 1.0,
    }
}

struct CorrectionReading {
    reading: String,
    texts: Vec<String>,
    edits: f64,
}

fn trailing_letter_reading(reading: &str) -> Option<&str> {
    let settings = ranking::settings();
    if !settings.enabled || !settings.trailing_letter || reading.len() < 6
        || reading.len() > 32 || !reading.bytes().all(|b| b.is_ascii_lowercase())
        || valid_pinyin_spelling(reading) || ends_with_bare_initial(reading)
        || all_initials(reading)
    {
        return None;
    }
    let shorter = &reading[..reading.len() - 1];
    // xianzo -> xianz: the remaining z is the intended abbreviated 做, not a
    // second missing-letter correction. Full syllables + a bare initial are also
    // complete mixed-pinyin input. Never strip a raw bare initial (woganj).
    (valid_pinyin_spelling(shorter) || ends_with_bare_initial(shorter)).then_some(shorter)
}

/// At most four distinct readings: a tail/literal repair plus the best three heavy
/// readings. Offer each reading's first sentence before its homophones.
fn correction_readings(reading: &str) -> Vec<CorrectionReading> {
    let settings = ranking::settings();
    if !settings.enabled || reading.len() < 6 || reading.len() > 32
        || !reading.bytes().all(|b| b.is_ascii_lowercase())
        || all_initials(reading) || ends_with_bare_initial(reading)
        || has_registered_ascii_exact(reading, false)
        || !tech_terms_for_reading_prefix(reading).is_empty()
    {
        return Vec::new();
    }
    const READINGS: usize = 3;
    let mut picked: Vec<(String, f64)> = Vec::new();
    if let Some(shorter) = trailing_letter_reading(reading) {
        picked.push((shorter.to_string(), edit_units(EditKind::ExtraLetter)));
    }
    if settings.multiple_readings && !valid_pinyin_spelling(reading) {
        let repairs = spelling_repairs(reading);
        let mut kinds = Vec::new();
        for spelling in &repairs {
            let Some(kind) = ranking::single_edit_kind(reading, spelling) else { continue };
            if kinds.contains(&kind) || picked.iter().any(|(old, _)| old == spelling) { continue; }
            kinds.push(kind);
            picked.push((spelling.clone(), repair_edit_units(reading, spelling)));
            if picked.len() == READINGS { break; }
        }
        for spelling in repairs {
            if picked.len() == READINGS { break; }
            if !picked.iter().any(|(old, _)| *old == spelling) {
                let edits = repair_edit_units(reading, &spelling);
                picked.push((spelling, edits));
            }
        }
    }
    picked.sort_by(|a, b| a.1.total_cmp(&b.1));
    let mut corrections: Vec<_> = picked.into_iter().map(|(spelling, edits)| {
        let texts = if valid_pinyin_spelling(&spelling) { decode_compositions(&spelling, 2) }
            else { tail_compositions(reading, &spelling) };
        CorrectionReading { reading: spelling, texts, edits }
    }).filter(|repair| !repair.texts.is_empty()).collect();
    if settings.multiple_readings && !valid_pinyin_spelling(reading) {
        let mut heavy = heavy_correction_readings(reading);
        // Keep one literal repair (a swap when present), then every one of the
        // heavy decoder's best three distinct readings, including two-edit paths.
        let preferred = corrections.iter().position(|repair| trailing_letter_reading(reading) == Some(repair.reading.as_str()))
            .or_else(|| corrections.iter().position(|repair| {
                ranking::single_edit_kind(reading, &repair.reading) == Some(EditKind::Transposition)
            })).unwrap_or(0);
        if !corrections.is_empty() {
            let repair = corrections.remove(preferred);
            heavy.retain(|other| other.reading != repair.reading);
            corrections.insert(0, repair);
        }
        let mut kept: Vec<_> = corrections.drain(..corrections.len().min(1)).collect();
        kept.extend(heavy);
        for repair in corrections {
            if kept.len() == 4 { break; }
            if !kept.iter().any(|other| other.reading == repair.reading) { kept.push(repair); }
        }
        corrections = kept;
    }
    corrections
}

fn trailing_letter_candidates(raw: &str, base: &[MixedCandidate], project_terms_enabled: bool) -> Vec<MixedCandidate> {
    let Some(reading) = canonical_reading(raw) else { return Vec::new() };
    if base.is_empty() || has_registered_ascii_exact(&reading, project_terms_enabled)
        || !tech_terms_for_reading_prefix(&reading).is_empty()
    { return Vec::new(); }
    let Some(shorter) = trailing_letter_reading(&reading) else { return Vec::new() };
    let texts = tail_compositions(&reading, shorter);
    texts.into_iter().filter(|text| !base.iter().any(|candidate| candidate.candidate.text == *text))
        .take(2)
        .map(|text| MixedCandidate {
            candidate: Candidate { text: text.clone(), comment: "尾字母 · 本地索引".into(),
                reading_key: reading.clone(), source_index: None },
            selection: Selection::Local { text }, tier: 2, score: 0,
        }).collect()
}

fn tail_compositions(typed: &str, shorter: &str) -> Vec<String> {
    if valid_pinyin_spelling(shorter) { return decode_compositions(shorter, 2); }
    let head = &shorter[..shorter.len() - 1];
    // xianzo cannot be repaired into complete pinyin by deleting o alone. The
    // surviving z is an abbreviation; retain the vowel as evidence when choosing
    // its completion (zuo), without changing the complete head's segmentation.
    let mut texts = Vec::new();
    for spelling in spelling_repairs(typed) {
        if spelling.starts_with(shorter) && pinyin_syllables().contains(&spelling[head.len()..]) {
            texts.extend(decode_compositions(&spelling, 2));
            if texts.len() >= 2 { break; }
        }
    }
    if texts.is_empty() { texts = initial_tail_completions(shorter).into_iter().take(2).collect(); }
    texts
}

fn push_heavy_walk_state(
    states: &mut Vec<(usize, usize, u8, u8)>,
    seen: &mut HashSet<(usize, usize, u8, u8)>,
    state: (usize, usize, u8, u8),
) {
    if seen.insert(state) {
        states.push(state);
    }
}

fn heavy_words_from(index: &Index, raw: &str, start: usize, allow_edits: bool) -> Vec<HeavyWordMatch> {
    let trie = &index.trie;
    let bytes = raw.as_bytes();
    let mut matches = Vec::new();
    let mut frontier = vec![(0usize, start, 0u8, 0u8)];
    while !frontier.is_empty() {
        let mut next = Vec::new();
        let mut next_seen = HashSet::new();
        for (node, raw_index, edits, deletions) in frontier {
            if raw_index > start {
                if let Some(terminal) = trie.terminal(node) {
                    matches.push(HeavyWordMatch {
                        raw_end: raw_index,
                        node,
                        terminal,
                        edits,
                        deletions,
                        omitted_letters: terminal.full_letter_count.saturating_sub(
                            raw_index
                                .saturating_sub(start)
                                .saturating_sub(deletions as usize)
                                as u8,
                        ),
                    });
                }
            }
            if raw_index >= bytes.len() {
                continue;
            }

            let direct = trie.letter_child(node, bytes[raw_index]);
            if let Some(child) = direct {
                push_heavy_walk_state(
                    &mut next,
                    &mut next_seen,
                    (child, raw_index + 1, edits, deletions),
                );
            }
            for boundary_node in trie.boundary_children(node) {
                if let Some(child) = trie.letter_child(boundary_node, bytes[raw_index]) {
                    push_heavy_walk_state(
                        &mut next,
                        &mut next_seen,
                        (child, raw_index + 1, edits, deletions),
                    );
                }
            }

            if allow_edits && edits < HEAVY_GARBLE_MAX_WORD_EDITS {
                // Deletion consumes one stray raw letter while staying at the
                // same trie node. Crossing a boundary at the same time covers
                // a stray letter immediately before the next syllable.
                push_heavy_walk_state(
                    &mut next,
                    &mut next_seen,
                    (node, raw_index + 1, edits + 1, deletions + 1),
                );
                for boundary_node in trie.boundary_children(node) {
                    push_heavy_walk_state(
                        &mut next,
                        &mut next_seen,
                        (boundary_node, raw_index + 1, edits + 1, deletions + 1),
                    );
                }

                // The only other edit is an adjacent transposition. There is
                // deliberately no substitution or insertion transition.
                if raw_index + 1 < bytes.len() {
                    for base in std::iter::once(node).chain(trie.boundary_children(node)) {
                        if let Some(first) = trie.letter_child(base, bytes[raw_index + 1])
                            && let Some(second) = trie.letter_child(first, bytes[raw_index])
                        {
                            push_heavy_walk_state(
                                &mut next,
                                &mut next_seen,
                                (second, raw_index + 2, edits + 1, deletions),
                            );
                        }
                    }
                }
            }
        }
        frontier = next;
    }
    matches.sort_by(|left, right| {
        left.raw_end
            .cmp(&right.raw_end)
            .then_with(|| left.edits.cmp(&right.edits))
            .then_with(|| left.omitted_letters.cmp(&right.omitted_letters))
            .then_with(|| {
                right
                    .terminal
                    .lexical_score
                    .cmp(&left.terminal.lexical_score)
            })
            .then_with(|| left.terminal.text.cmp(&right.terminal.text))
    });
    matches.dedup_by(|left, right| {
        left.raw_end == right.raw_end
            && left.edits == right.edits
            && left.omitted_letters == right.omitted_letters
            && left.terminal.text == right.terminal.text
    });
    matches
}

fn heavy_path_order(left: &HeavyPath, right: &HeavyPath) -> Ordering {
    right
        .score
        .cmp(&left.score)
        .then_with(|| left.edits.cmp(&right.edits))
        .then_with(|| left.words.cmp(&right.words))
        .then_with(|| left.text.cmp(&right.text))
}

fn prune_heavy_paths(paths: &mut Vec<HeavyPath>) {
    paths.sort_by(heavy_path_order);
    let mut seen = HashSet::new();
    paths.retain(|path| seen.insert(normalize_text(&path.text)));
    paths.truncate(HEAVY_GARBLE_BEAM_PER_POSITION);
}

fn decode_heavy_garble(reading: &str) -> Vec<String> {
    decode_heavy_garble_with_edits(reading).into_iter().map(|(text, _)| text).collect()
}

/// Heavy-garble decodes with normalized typing costs; abbreviation omissions stay separate.
fn decode_heavy_garble_with_edits(reading: &str) -> Vec<(String, f64)> {
    decode_heavy_paths(reading).into_iter().map(|path| (path.text, path.edit_units)).collect()
}

fn decode_heavy_paths(reading: &str) -> Vec<HeavyPath> {
    let index = zhixu::current();
    let mut paths = vec![Vec::new(); reading.len() + 1];
    paths[0].push(HeavyPath {
        score: 0,
        text: String::new(),
        words: 0,
        edits: 0,
        edit_units: 0.0,
        nodes: [0; HEAVY_GARBLE_MAX_WORDS as usize],
    });
    for start in 0..reading.len() {
        if paths[start].is_empty() {
            continue;
        }
        prune_heavy_paths(&mut paths[start]);
        let current = paths[start].clone();
        let matches = heavy_words_from(&index, reading, start, true);
        let mut touched = HashSet::new();
        for matched in matches {
            for path in &current {
                if path.words >= HEAVY_GARBLE_MAX_WORDS
                    || path.edits + matched.edits > HEAVY_GARBLE_MAX_TOTAL_EDITS
                {
                    continue;
                }
                let character_count = index.text(matched.terminal).chars().count();
                // the heavy decoder does not keep which letters it changed: price a changed letter as
                // an average substitution, not as the cheap transposition
                let edit_units = f64::from(matched.edits - matched.deletions) * edit_units(EditKind::OtherSubstitution)
                    + f64::from(matched.deletions) * edit_units(EditKind::ExtraLetter);
                // Omitted finals are deliberate abbreviations, not missing-letter edits.
                let mut added_score = matched.terminal.lexical_score
                    - (25_000.0 * edit_units).round() as i64
                    - 25_000 * i64::from(matched.omitted_letters);
                if character_count == 1 {
                    // Single-character fillers must cost more than their
                    // dictionary-frequency term can earn.
                    added_score -= 30_000;
                } else {
                    // Reward a coherent multi-character dictionary word once,
                    // rather than rewarding every extra output character. The
                    // latter made long, highly abbreviated junk dominate.
                    added_score += 40_000;
                }
                if path.words != 0 {
                    // Prefer a compact lexical parse when two parses cover the
                    // same raw span; every extra boundary pays a fixed cost.
                    added_score -= 30_000;
                }
                let mut nodes = path.nodes;
                nodes[path.words as usize] = matched.node;
                paths[matched.raw_end].push(HeavyPath {
                    score: path.score + added_score,
                    text: format!("{}{}", path.text, index.text(matched.terminal)),
                    words: path.words + 1,
                    edits: path.edits + matched.edits,
                    edit_units: path.edit_units + edit_units,
                    nodes,
                });
                touched.insert(matched.raw_end);
            }
        }
        for raw_end in touched {
            prune_heavy_paths(&mut paths[raw_end]);
        }
    }
    let mut finals = std::mem::take(&mut paths[reading.len()]);
    prune_heavy_paths(&mut finals);
    finals
        .into_iter()
        .filter(|path| path.text.chars().count() >= 2)
        .collect()
}

fn heavy_correction_readings(reading: &str) -> Vec<CorrectionReading> {
    let index = zhixu::current();
    let mut seen = HashSet::new();
    decode_heavy_paths(reading).into_iter().filter_map(|path| {
        let mut pronunciation = String::new();
        for &node in &path.nodes[..path.words as usize] {
            let terminal = index.trie.terminal(node)?;
            let text = index.text(terminal);
            let full = index.readings_for_text(text).find(|full| {
                if full.bytes().filter(u8::is_ascii_lowercase).count() != usize::from(terminal.full_letter_count)
                    || !index.words_for_reading(full).any(|(word, weight)| word == text
                        && ((weight.max(1) as f64).powf(0.2) * 1_000.0).round() as i64 == terminal.lexical_score)
                { return false; }
                let mut cursor = 0;
                for syllable in full.split(' ') {
                    if cursor != 0 {
                        let Some(boundary) = index.trie.boundary_children(cursor).next() else { return false };
                        cursor = boundary;
                    }
                    for letter in syllable.bytes() {
                        let Some(child) = index.trie.letter_child(cursor, letter) else { return false };
                        cursor = child;
                        if cursor == node { return true; }
                    }
                }
                false
            })?;
            pronunciation.extend(full.bytes().filter(|b| *b != b' ').map(char::from));
        }
        seen.insert(pronunciation.clone()).then_some(CorrectionReading {
            reading: pronunciation, texts: vec![path.text], edits: path.edit_units,
        })
    }).take(3).collect()
}

/// C3 (model-composed sentences): when the decoder's first is a composition or covers
/// only part of a long valid spelling (taifuzale -> 太傅 / 台服砸了), offer other whole-reading
/// segmentations for the context model; a complete dictionary word first is left to
/// the model-first group. Owner records: margins over 8 replaced silent garbage
/// (你系按看下 -> 你先看下, 真立下 -> 整理下) about nine times in ten.
/// Everything C3's alternatives depend on: the reading, the page and the first
/// candidate (its text, its source label and whether it is a local candidate).
type CompositionKey = ((u64, u64), String, usize, Option<(String, String, bool)>, ranking::CorrectionSettings);

impl MixerDecoder {
    /// The refusal code for a page whose first candidate is `first` instead of the sentence.
    fn lead_refusal(&self, raw: &str, first: &str) -> i32 {
        let learned = self.learning.entries().iter()
            .any(|entry| entry.is_learned() && entry.text == first && learning_covers(raw, &entry.reading));
        if learned { LEAD_OUTRANKED_BY_LEARNED_WHOLE } else { LEAD_OUTRANKED_OTHER }
    }

    /// C3 alternatives for the current page and the first's own edits, computed once per
    /// key: the shells ask for alternatives and edits two or three times a key.
    fn compositions(&self) -> (Vec<(String, f64)>, f64) {
        let _dictionary = self.pin_dictionary();
        let (snapshot, mixed) = self.visible_unranked();
        let key: CompositionKey = (
            (zhixu::generation(), zhixu::learning_index().map_or(0, |index| index.generation)),
            snapshot.raw_input.clone(),
            snapshot.page_no,
            mixed.first().map(|first| {
                (
                    first.candidate.text.clone(),
                    first.candidate.comment.clone(),
                    matches!(first.selection, Selection::Local { .. }),
                )
            }),
            ranking::settings(),
        );
        let visible = |mut alternatives: Vec<(String, f64)>| {
            if ranking::settings().enabled {
                let reading = canonical_reading(&snapshot.raw_input).unwrap_or_default();
                if let Ok(blacklist) = self.blacklist.lock() {
                    alternatives.retain(|(text, _)| !blacklist.contains(&reading, text));
                } else { alternatives.clear(); }
            }
            alternatives
        };
        if let Ok(cache) = self.composition_cache.lock()
            && let Some((cached, alternatives, first_edits)) = cache.as_ref()
            && *cached == key
        {
            return (visible(alternatives.clone()), *first_edits);
        }
        let first_edits = first_composition_edits(&snapshot, &mixed);
        let alternatives = composition_candidates_for(&snapshot, &mixed, first_edits);
        if let Ok(mut cache) = self.composition_cache.lock() {
            *cache = Some((key, alternatives.clone(), first_edits));
        }
        (visible(alternatives), first_edits)
    }
}

#[cfg(test)]
fn composition_alternatives_for(snapshot: &DecoderSnapshot, mixed: &[MixedCandidate]) -> Vec<String> {
    composition_candidates_for(snapshot, mixed, first_composition_edits(snapshot, mixed))
        .into_iter()
        .map(|(text, _)| text)
        .collect()
}

/// Normalized typing cost of a heavy-garble first; zero for an untouched decoder first.
fn first_composition_edits(snapshot: &DecoderSnapshot, mixed: &[MixedCandidate]) -> f64 {
    mixed
        .first()
        .filter(|first| first.candidate.comment == HEAVY_GARBLE_COMMENT)
        .and_then(|first| {
            let reading = canonical_reading(&snapshot.raw_input)?;
            decode_heavy_garble_with_edits(&reading)
                .into_iter()
                .find(|(text, _)| *text == first.candidate.text)
                .map(|(_, edits)| edits)
        })
        .unwrap_or(0.0)
}

fn composition_candidates_for(snapshot: &DecoderSnapshot, mixed: &[MixedCandidate], first_edits: f64) -> Vec<(String, f64)> {
    // An apostrophe only marks a syllable boundary; the sentences are built from the letters.
    let Some(reading) = canonical_reading(&snapshot.raw_input).map(|reading| reading.replace('\'', "")).filter(|reading| !reading.is_empty()) else {
        return Vec::new();
    };
    let Some(first) = mixed.first() else {
        return Vec::new();
    };
    // A whole word of any layer the decoders know: the exact-reading index leaves out
    // the everyday layers (general, newwords, modern), whose words must stay too.
    let complete_word = chinese_syllabified_readings_by_text()
        .get(&first.candidate.text)
        .is_some_and(|readings| readings.iter().any(|(compact, _)| *compact == reading))
        || vocabulary_word(&first.candidate.text, &reading);
    let valid = valid_pinyin_spelling(&reading);
    // An invalid full spelling (Rime read it as abbreviations, or the heavy-garble layer
    // guessed) may have a one-slip repair that competes. A local first other than a
    // heavy-garble guess (English, project terms) stays.
    let garbled_first = !valid && first.candidate.comment == HEAVY_GARBLE_COMMENT;
    // Step 2.3 (NEXT-ORDER-2026-10-10): the model chooses well when the answer is
    // offered (86-93% on cases the decoder got wrong), so offer it more: any reading of
    // four letters or more, or two initials or more (hd 核对, cwfx 财务风险: all-initial
    // words 29.3% -> 37.0% first, nothing broken), up to 23 sentences. A whole dictionary word first stays:
    // without that guard the owner's 分词, 加权, 知识库 lost to composed rivals.
    if snapshot.page_no != 0
        || (reading.len() < 4 && !all_initials(&reading))
        // Past 32 letters the decoders slow every key and the model's 39-character
        // window no longer holds the sentence.
        || reading.len() > 32
        || (matches!(first.selection, Selection::Local { .. }) && !decoder_candidate(first) && !garbled_first)
        || complete_word
        // A registered English or tech term first (agent from the English layer) is
        // what was typed; the model scores Latin letters as unknown characters.
        || first.candidate.text.chars().any(|c| c.is_ascii_alphabetic())
    {
        return Vec::new();
    }
    let mut decoded: Vec<String> = if valid {
        decode_compositions(&reading, 24)
    } else {
        // The owner abbreviates syllables to their initials (zaifageipy = 再发给朋友):
        // that is a style, not a slip, so these cost no edits.
        decode_compositions_with(&reading, 12, true)
    };
    // The homophone lattice only fills the places left after the decoder's sentences and
    // (for an invalid spelling) the slip repairs, and leaves a guessed (heavy-garble) first alone: that is the
    // garble layer's. Earlier in the list it crowded the repairs out (现场 xiancang).
    let mut lattice: Vec<String> = if lattice_enabled() && !garbled_first {
        decode_lattice(&reading, lattice_params().limit, !valid)
    } else {
        Vec::new()
    };
    // A full valid spelling has no slip repairs to crowd out: its lattice sentences simply
    // follow the decoder's own.
    if valid {
        decoded.append(&mut lattice);
    }
    // Twenty-one sentences: up to six homophone words in the decoder's own segmentation
    // (打字办 -> 大字版, 偷偷该 -> 偷偷改: the commonest remaining full-pinyin miss, and
    // the choice the model makes best), then other segmentations, then slip repairs
    // (one edit each) in what is left. Then at most two function-character variants
    // (在发给朋友 -> 再发给朋友): with the first, 24 rows, three batches of the model.
    let sentences = if lattice_enabled() { lattice_params().sentences } else { 21 };
    const HOMOPHONE_WORDS: usize = 6;
    const VARIANTS: usize = 2;
    let first_text = &first.candidate.text;
    let mut seen = HashSet::from([first_text.clone()]);
    let homophones: Vec<(String, f64)> = homophone_word_variants(&reading, first_text)
        .into_iter()
        .filter(|text| seen.insert(text.clone()))
        .take(HOMOPHONE_WORDS)
        .map(|text| (text, first_edits))
        .collect();
    let corrections = correction_readings(&reading);
    let mut correction_rows = Vec::new();
    for rank in 0..2 {
        for repair in &corrections {
            if let Some(text) = repair.texts.get(rank) && text != first_text
                && !correction_rows.iter().any(|(old, _)| old == text) {
                correction_rows.push((text.clone(), repair.edits));
            } else if let Some(text) = repair.texts.get(rank)
                && let Some((_, edits)) = correction_rows.iter_mut().find(|(old, _)| old == text)
            {
                *edits = edits.min(repair.edits);
            }
        }
    }
    let repair_spellings = spelling_repairs(&reading);
    let repaired = repair_spellings
        .iter()
        .flat_map(|spelling| decode_compositions(spelling, 2).into_iter().map(|text| (text, repair_edit_units(&reading, spelling))));
    // The homophones of each repaired spelling come after every repair's own two sentences.
    let repaired_lattice: Vec<(String, f64)> = if lattice_enabled() && !garbled_first {
        repair_spellings
            .iter()
            .flat_map(|spelling| decode_lattice(spelling, 3, false).into_iter().map(|text| (text, repair_edit_units(&reading, spelling))))
            .collect()
    } else {
        Vec::new()
    };
    // Trailing initials completed (zhedifangy -> 这地方也) lead the typed spelling's
    // sentences; three places stay for slip repairs, which those would crowd out.
    const REPAIRS: usize = 3;
    let tails = if valid { Vec::new() } else { initial_tail_completions(&reading) };
    let sentences = sentences.saturating_sub(correction_rows.len());
    let typed: Vec<(String, f64)> = tails
        .into_iter()
        .chain(decoded)
        .map(|text| (text, 0.0))
        .filter(|(text, _)| seen.insert(text.clone()))
        .take(sentences.saturating_sub(REPAIRS).saturating_sub(homophones.len()))
        .collect();
    let mut primary: Vec<(String, f64)> = typed
        .into_iter()
        .chain(repaired.filter(|(text, _)| seen.insert(text.clone())))
        .take(sentences.saturating_sub(homophones.len()))
        .collect();
    let room = sentences.saturating_sub(homophones.len()).saturating_sub(primary.len());
    // The typed spelling's lattice sentences first, then the homophones of the repairs.
    primary.extend(
        lattice
            .into_iter()
            .map(|text| (text, 0.0))
            .chain(repaired_lattice)
            .filter(|(text, _)| seen.insert(text.clone()))
            .take(room),
    );
    primary.extend(homophones);
    for (text, edits) in &mut correction_rows {
        if let Some(at) = primary.iter().position(|(old, _)| old == text) {
            *edits = edits.min(primary.remove(at).1);
        }
        seen.insert(text.clone());
    }
    correction_rows.extend(primary);
    let primary = correction_rows;
    // The first's variants come from the same keys, so they carry its edits.
    let variants: Vec<(String, f64)> = std::iter::once((first_text.clone(), first_edits))
        .chain(primary.iter().cloned())
        .take(3)
        .flat_map(|(text, edits)| homophone_function_variants(&text).into_iter().map(move |variant| (variant, edits)))
        .filter(|(text, _)| seen.insert(text.clone()))
        .take(VARIANTS)
        .collect();
    // The shells read the sentences through a 4096-byte buffer; past that the whole list
    // would be reported as missing. Keep the front of the list and drop the tail.
    const MAX_SENTENCE_BYTES: usize = 4000;
    let mut bytes = 0;
    let mut edit_bytes = ranking::format_edit_units(first_edits).len() + 1;
    primary
        .into_iter()
        .chain(variants)
        .take_while(|(text, edits)| {
            edit_bytes += ranking::format_edit_units(*edits).len() + 1;
            bytes += text.len() + 1;
            bytes <= MAX_SENTENCE_BYTES && edit_bytes <= 250
        })
        .collect()
}

/// Two or more bare initials and nothing else (hd, jdfs): an all-initials abbreviation,
/// not the start of a syllable still being typed (zh, sh).
fn all_initials(reading: &str) -> bool {
    let bytes = reading.as_bytes();
    let (mut index, mut count) = (0, 0);
    while index < bytes.len() {
        let two = bytes.get(index + 1) == Some(&b'h') && matches!(bytes[index], b'z' | b'c' | b's');
        if !two && !b"bpmfdtnlgkhjqxrzcsyw".contains(&bytes[index]) {
            return false;
        }
        index += if two { 2 } else { 1 };
        count += 1;
    }
    count >= 2
}

/// Words of the decoders' vocabulary by syllables ("jiu kuai" -> 就快, 九块 ...), most
/// frequent first.
fn words_by_reading() -> Arc<Index> {
    zhixu::current()
}

/// True when `text` is a word of the decoders' vocabulary read as `reading` (compact).
fn vocabulary_word(text: &str, reading: &str) -> bool {
    zhixu::current().vocabulary_word(text, reading)
}

/// True when `text` is one word of the decoders' vocabulary or the exact-reading index
/// whose syllables `reading` spells in order, each in full or by a prefix (zhishik
/// 知识库, zhge 这个): the owner abbreviating a word they know, which a composed
/// sentence must not displace. Owner replay 2026-10-11 at margin 6: zhishik -> 只是看.
fn abbreviated_vocabulary_word(text: &str, reading: &str) -> bool {
    let index = zhixu::current();
    fn spells(reading: &str, syllables: &[&str]) -> bool {
        match syllables.split_first() {
            None => reading.is_empty(),
            Some((syllable, rest)) => (1..=syllable.len().min(reading.len()))
                .any(|length| syllable.starts_with(&reading[..length]) && spells(&reading[length..], rest)),
        }
    }
    text.chars().count() >= 2
        && (index.readings_for_text(text).any(|spaced| spells(reading, &spaced.split(' ').collect::<Vec<_>>()))
            || chinese_syllabified_readings_by_text().get(text).is_some_and(|readings| {
                readings.iter().any(|(_, spaced)| spells(reading, &spaced.split(' ').collect::<Vec<_>>()))
            }))
}

/// `first` with one span of one to three syllables replaced by another word of the same
/// syllables, most promising first (the replacement's frequency against the replaced
/// text's). Only for a full spelling whose syllables match the first one to one.
fn homophone_word_variants(reading: &str, first: &str) -> Vec<String> {
    let Some(syllables) = pinyin_segmentation(reading) else { return Vec::new() };
    let characters: Vec<char> = first.chars().collect();
    if syllables.len() != characters.len()
        || syllables.len() < 2
        || !characters.iter().all(|c| ('\u{4e00}'..='\u{9fff}').contains(c))
    {
        return Vec::new();
    }
    let dictionary = words_by_reading();
    let mut scored: Vec<(f64, String)> = Vec::new();
    for length in 1..=3.min(syllables.len()) {
        for start in 0..=syllables.len() - length {
            let spaced = syllables[start..start + length].join(" ");
            let replaced: String = characters[start..start + length].iter().collect();
            let replaced_weight = dictionary.words_for_reading(&spaced).find(|(text, _)| *text == replaced).map_or(1, |(_, weight)| weight);
            for (text, weight) in dictionary.words_for_reading(&spaced).filter(|(text, _)| *text != replaced).take(3) {
                let variant: String = characters[..start]
                    .iter()
                    .chain(text.chars().collect::<Vec<_>>().iter())
                    .chain(characters[start + length..].iter())
                    .collect();
                scored.push(((weight as f64).ln() - (replaced_weight as f64).ln(), variant));
            }
        }
    }
    scored.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    let mut seen = HashSet::new();
    scored.into_iter().map(|(_, text)| text).filter(|text| seen.insert(text.clone())).collect()
}

/// Readings that end in bare initials after full syllables (zhedifangy 这地方也,
/// haishid 还是等, pinlvdt 频率动态, zhoujiab 周加班): the head decoded in full syllables
/// with the commonest words the initials abbreviate, alone or taking the head's last
/// syllable. The decoder keeps one word per typed prefix (d -> 的 only), so in the
/// external set 144 of 171 such misses were never offered (2026-10-11).
fn initial_tail_completions(reading: &str) -> Vec<String> {
    const AFTER_FIRST_HEAD: usize = 6;
    const AFTER_SECOND_HEAD: usize = 2;
    const TAKING_LAST_SYLLABLE: usize = 3;
    let Some((head, tokens)) = split_initial_tail(reading) else { return Vec::new() };
    let Some(syllables) = pinyin_segmentation(head) else { return Vec::new() };
    let mut out = Vec::new();
    for (rank, head_text) in decode_compositions(head, 2).iter().enumerate() {
        let share = if rank == 0 { AFTER_FIRST_HEAD } else { AFTER_SECOND_HEAD };
        out.extend(words_with_initials(None, &tokens).take(share).map(|word| format!("{head_text}{word}")));
    }
    if let Some((last, rest)) = syllables.split_last() {
        let rest_text = if rest.is_empty() { Some(String::new()) } else { decode_compositions(&rest.concat(), 1).into_iter().next() };
        if let Some(rest_text) = rest_text {
            out.extend(
                words_with_initials(Some(last), &tokens)
                    .take(TAKING_LAST_SYLLABLE)
                    .map(|word| format!("{rest_text}{word}")),
            );
        }
    }
    let mut seen = HashSet::new();
    out.retain(|text| seen.insert(text.clone()));
    out
}

/// `reading` as full syllables then one to four initials (zh/ch/sh count as one), the
/// shortest such tail: zaifageipy -> (zaifagei, [p, y]).
fn split_initial_tail(reading: &str) -> Option<(&str, Vec<&str>)> {
    if valid_pinyin_spelling(reading) {
        return None;
    }
    (1..reading.len().min(9)).find_map(|length| {
        let (head, tail) = reading.split_at(reading.len() - length);
        let tokens = initial_tokens(tail)?;
        (tokens.len() <= 4 && valid_pinyin_spelling(head)).then_some((head, tokens))
    })
}

fn initial_tokens(tail: &str) -> Option<Vec<&str>> {
    let bytes = tail.as_bytes();
    let mut tokens = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        let two = bytes.get(index + 1) == Some(&b'h') && matches!(bytes[index], b'z' | b'c' | b's');
        if !two && !b"bpmfdtnlgkhjqxrzcsyw".contains(&bytes[index]) {
            return None;
        }
        let end = index + if two { 2 } else { 1 };
        tokens.push(&tail[index..end]);
        index = end;
    }
    (!tokens.is_empty()).then_some(tokens)
}

/// Words whose syllables start with `tokens` in order, after `leading` when given (the
/// whole first syllable), most frequent first.
fn words_with_initials(leading: Option<&String>, tokens: &[&str]) -> std::vec::IntoIter<String> {
    let index = zhixu::current();
    let key: String = leading
        .and_then(|syllable| syllable.chars().next())
        .into_iter()
        .chain(tokens.iter().filter_map(|token| token.chars().next()))
        .collect();
    let skip = usize::from(leading.is_some());
    index.words_for_initials(&key).filter_map(|(word, reading, _)| {
        let mut syllables = reading.split(' ');
        let leads = leading.is_none_or(|syllable| syllables.next() == Some(syllable.as_str()));
        let follows = reading.split(' ').skip(skip).zip(tokens).all(|(syllable, token)| syllable.starts_with(token));
        (leads && follows).then(|| word.to_string())
    }).collect::<Vec<_>>().into_iter()
}

/// Each sentence with one confusable function character swapped (在/再, 的/地/得,
/// 他/她/它, 做/作, 那/哪): a decoder without context picks one by frequency.
fn homophone_function_variants(text: &str) -> Vec<String> {
    const GROUPS: [&[char]; 5] = [&['在', '再'], &['的', '地', '得'], &['他', '她', '它'], &['做', '作'], &['那', '哪']];
    let characters: Vec<char> = text.chars().collect();
    let mut variants = Vec::new();
    for (index, character) in characters.iter().enumerate() {
        let Some(group) = GROUPS.iter().find(|group| group.contains(character)) else { continue };
        for &other in group.iter().filter(|&&other| other != *character) {
            let mut changed = characters.clone();
            changed[index] = other;
            variants.push(changed.into_iter().collect());
        }
    }
    variants
}

/// One-edit repairs of a fast typist's slips that give a valid full spelling: the
/// shared letter typed once (zong+gan as `zongan`, jin+nian as `jinian`), two letters
/// swapped (zhuangixnban, zhenmeixinagdao), one extra vowel, a letter left out
/// (xianza, yingai) and a neighbouring key. The model decides among them (C3), each
/// repair costing an edit. A valid spelling is repaired too: a slip can land on
/// another valid one (shangle -> shanle); at the composition margin no correctly typed
/// case in either evaluation set changed (2026-10-10).
/// Owner records (10-09..10): wozongan committed 我总按 then retyped 我总感觉;
/// zhuangixnban -> 装新版 failed three times.
fn spelling_repairs(reading: &str) -> Vec<String> {
    // Each kind of slip gets its own share, so on a long reading the many dropped-letter
    // positions cannot crowd out swaps and neighbouring keys.
    const SWAPS: usize = 4;
    const EXTRA_VOWELS: usize = 3;
    const DROPPED_LETTERS: usize = 6;
    const NEIGHBOUR_KEYS: usize = 3;
    const REPEATED_LETTERS: usize = 2;
    const ROWS: [&str; 3] = ["qwertyuiop", "asdfghjkl", "zxcvbnm"];
    let bytes = reading.as_bytes();
    let mut repairs: Vec<String> = Vec::new();
    let mut add = |candidates: Vec<String>, share: usize, repairs: &mut Vec<String>| {
        let mut taken = 0;
        for candidate in candidates {
            if taken == share {
                break;
            }
            if candidate != reading && valid_pinyin_spelling(&candidate) && !repairs.contains(&candidate) {
                repairs.push(candidate);
                taken += 1;
            }
        }
    };
    let doubled = (1..bytes.len())
        .filter(|&index| matches!(bytes[index - 1], b'n' | b'g'))
        .map(|index| {
            let mut doubled = reading.to_string();
            doubled.insert(index, bytes[index - 1] as char);
            doubled
        })
        .collect();
    add(doubled, usize::MAX, &mut repairs);
    let swaps = (0..bytes.len().saturating_sub(1))
        .map(|index| {
            let mut swapped = bytes.to_vec();
            swapped.swap(index, index + 1);
            String::from_utf8(swapped).unwrap_or_default()
        })
        .collect();
    add(swaps, SWAPS, &mut repairs);
    // Only an extra vowel is a slip (the slip layer's rule): a typed consonant is the
    // owner abbreviating a character (dengtawanc = 等他完成, zhenweis = 甄维斯), and a
    // final a/e/o is a particle. Owner replay: consonant deletions led to 等他玩, 真为.
    let extra_vowels = reading
        .char_indices()
        .filter(|&(index, letter)| "aeiouv".contains(letter) && !(index + 1 == bytes.len() && "aeo".contains(letter)))
        .map(|(index, _)| {
            let mut shorter = reading.to_string();
            shorter.remove(index);
            shorter
        })
        .collect();
    add(extra_vowels, EXTRA_VOWELS, &mut repairs);
    // The same letter struck twice (zhhuyaoshi, rriqi) is a slip, not an abbreviation.
    let repeated = (1..bytes.len())
        .filter(|&index| bytes[index] == bytes[index - 1])
        .map(|index| {
            let mut shorter = reading.to_string();
            shorter.remove(index);
            shorter
        })
        .collect();
    add(repeated, REPEATED_LETTERS, &mut repairs);
    // The commonest slip is a letter left out (47% of corrections in Sogou's logs,
    // mostly i/g/h/u/e: xianza, yingai, senme), then a neighbouring key (19%, same
    // QWERTY row).
    let dropped = "ighuenoa"
        .chars()
        .flat_map(|letter| {
            (0..=bytes.len()).map(move |index| {
                let mut longer = reading.to_string();
                longer.insert(index, letter);
                longer
            })
        })
        .collect();
    add(dropped, DROPPED_LETTERS, &mut repairs);
    let mut neighbours = Vec::new();
    for (index, letter) in reading.char_indices() {
        for row in ROWS {
            let Some(at) = row.find(letter) else { continue };
            for neighbour in [at.checked_sub(1), Some(at + 1)].into_iter().flatten() {
                if let Some(other) = row[neighbour..].chars().next() {
                    let mut replaced = reading.to_string();
                    replaced.replace_range(index..index + 1, &other.to_string());
                    neighbours.push(replaced);
                }
            }
        }
    }
    add(neighbours, NEIGHBOUR_KEYS, &mut repairs);
    repairs
}

#[cfg(test)]
#[path = "zhixu/learning_index_tests.rs"]
mod learning_index_tests;

/// Whole-reading segmentations into dictionary words, exact syllables only, scored
/// without the heavy-garble layer's single-character penalty: 太|复杂|了 must be
/// able to compete with 台服|砸了. Diversity matters more than this order; the
/// context model chooses among them (C3, model-composed sentences).
fn decode_compositions(reading: &str, limit: usize) -> Vec<String> {
    decode_compositions_with(reading, limit, false)
}

/// `abbreviated`: a syllable may be typed as its prefix (the owner's style:
/// woxikanx = 我先看下, jiexialzuoshenm = 接下来做什么), each omitted letter costing a
/// little so full syllables still win when they fit.
fn decode_compositions_with(reading: &str, limit: usize, abbreviated: bool) -> Vec<String> {
    let index = zhixu::current();
    let personal = zhixu::learning_index();
    const BEAM: usize = 32;
    const WORD_COST: i64 = 15000;
    const OMITTED_LETTER_COST: i64 = 1500;
    let mut paths: Vec<Vec<(i64, String)>> = vec![Vec::new(); reading.len() + 1];
    paths[0].push((0, String::new()));
    for start in 0..reading.len() {
        if paths[start].is_empty() {
            continue;
        }
        let mut current = std::mem::take(&mut paths[start]);
        current.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
        current.truncate(BEAM);
        let mut base_words = HashSet::new();
        for (source, dictionary) in std::iter::once(index.as_ref()).chain(personal.as_deref()).enumerate() {
            for matched in heavy_words_from(dictionary, reading, start, false) {
                // Interjection syllables (n 嗯, m 呣, ng, hm) only split real syllables.
                if matched.edits != 0
                    || (!abbreviated && matched.omitted_letters != 0)
                    || matches!(&reading[start..matched.raw_end], "n" | "m" | "ng" | "hm" | "hng")
                {
                    continue;
                }
                let word = dictionary.text(matched.terminal);
                if personal.is_some() {
                    let key = (matched.raw_end, word, matched.omitted_letters);
                    if source == 0 { base_words.insert(key); }
                    else if base_words.contains(&key) { continue; }
                }
                for (score, text) in &current {
                    paths[matched.raw_end].push((
                        score + matched.terminal.lexical_score
                            - WORD_COST
                            - OMITTED_LETTER_COST * i64::from(matched.omitted_letters),
                        format!("{text}{word}"),
                    ));
                }
            }
        }
        paths[start] = current;
    }
    let mut finals = std::mem::take(&mut paths[reading.len()]);
    finals.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
    let mut seen = HashSet::new();
    finals
        .into_iter()
        .filter(|(_, text)| seen.insert(text.clone()))
        .map(|(_, text)| text)
        .take(limit)
        .collect()
}

fn whole_spelling_matches(raw: &str, spaced: &str) -> bool {
    if !raw.contains('\'') { return true; }
    let mut boundaries = HashSet::new();
    let mut position = 0;
    for byte in spaced.bytes() {
        if byte == b' ' { boundaries.insert(position); }
        else { position += 1; }
    }
    position = 0;
    raw.bytes().all(|byte| {
        if byte == b'\'' { boundaries.contains(&position) }
        else { position += 1; true }
    })
}

fn whole_syllable_count(reading: &str) -> Option<usize> {
    let syllables = pinyin_syllables();
    reading.split('\'').try_fold(0, |count, segment| {
        let segment = if !segment.is_empty() && segment.bytes().all(|byte| byte.is_ascii_lowercase()) {
            std::borrow::Cow::Borrowed(segment)
        } else { std::borrow::Cow::Owned(canonical_reading(segment)?) };
        if segment.len() > 120 { return None; }
        // Only the minimum count is used here; the default segmenter still returns
        // its original spellings and tie order for all other callers.
        let mut best = vec![usize::MAX; segment.len() + 1];
        best[segment.len()] = 0;
        let repair_jqx = zhixu::fuzzy::has_jqx_spelling_error(&segment);
        for start in (0..segment.len()).rev() {
            for end in start + 1..=segment.len().min(start + 6) {
                if best[end] != usize::MAX && (syllables.contains(&segment[start..end])
                    || repair_jqx && !zhixu::fuzzy::jqx_token_repairs(&segment[start..end]).is_empty()) {
                    best[start] = best[start].min(best[end] + 1);
                }
            }
        }
        (best[0] != usize::MAX).then(|| count + best[0])
    })
}

pub(crate) fn whole_learning_bonus(entry: &zhixu::learning::Entry, tick: u64, now: u64) -> i64 {
    // librime's poet also uses the user's single-character frequencies (搞 over 高 for an owner who
    // writes 搞). The owner replay, 10-29: same cap as words, 20 -> 16 breaks, no hot break.
    // Rime scores are log probabilities; twice its dynamic scale turns one
    // recent selection into approximately one unit, like the legacy count score.
    let score = entry.score(tick);
    let evidence = if entry.rime.is_some() { score.min(0.0).exp() * 401.00083333298664 } else { score };
    let freshness = (-(now.saturating_sub(entry.last_ms) as f64) / (30.0 * 24.0 * 60.0 * 60.0 * 1000.0)).exp2();
    // Bounded below a word's segmentation cost: frequency helps without erasing it.
    (2000.0 * (evidence * freshness).ln_1p()).min(3000.0).round() as i64
}

#[cfg(test)]
fn whole_reading_candidates(reading: &str, entries: &[zhixu::learning::Entry], tick: u64, inherited_order: bool) -> Vec<MixedCandidate> {
    whole_reading_candidates_with(reading, entries, tick, inherited_order, None)
}

#[cfg(test)]
thread_local! {
    // Disjoint sub-stages of whole_reading_candidates_with: lookup, homophones,
    // learned entries, lattice expansion, sorting/deduplication, output cloning.
    static WHOLE_GENERATION_TIMING: std::cell::RefCell<[u128; 6]> = const { std::cell::RefCell::new([0; 6]) };
}

#[cfg(test)]
struct WholeGenerationTiming(usize, std::time::Instant);

#[cfg(test)]
impl WholeGenerationTiming {
    fn start(slot: usize) -> Self { Self(slot, std::time::Instant::now()) }
    fn next(&mut self, slot: usize) {
        self.record();
        self.0 = slot;
        self.1 = std::time::Instant::now();
    }
    fn record(&self) {
        WHOLE_GENERATION_TIMING.with(|timing| timing.borrow_mut()[self.0] += self.1.elapsed().as_nanos());
    }
}

#[cfg(test)]
impl Drop for WholeGenerationTiming {
    fn drop(&mut self) { self.record(); }
}

fn whole_reading_candidates_prefix_with(reading: &str, entries: &[zhixu::learning::Entry], tick: u64, inherited_order: bool,
    cache: Option<(&mut Option<WholeLattice>, u64)>, accept: &dyn Fn(&str) -> bool) -> (Vec<MixedCandidate>, bool) {
    whole_reading_candidates_demand(reading, entries, tick, inherited_order, cache, Some(accept))
}

fn whole_reading_candidates_with(reading: &str, entries: &[zhixu::learning::Entry], tick: u64, inherited_order: bool,
    cache: Option<(&mut Option<WholeLattice>, u64)>) -> Vec<MixedCandidate> {
    whole_reading_candidates_demand(reading, entries, tick, inherited_order, cache, None).0
}

#[cfg(test)]
pub(crate) fn decoder_whole_candidates(reading: &str, entries: &[zhixu::learning::Entry], tick: u64, inherited_order: bool) -> Vec<Candidate> {
    decoder_whole_candidates_cached(reading, entries, tick, inherited_order, None)
}

pub(crate) fn decoder_whole_candidates_cached(reading: &str, entries: &[zhixu::learning::Entry], tick: u64, inherited_order: bool,
    cache: Option<(&mut Option<WholeLattice>, u64)>) -> Vec<Candidate> {
    whole_reading_candidates_with(reading, entries, tick, inherited_order, cache)
        .into_iter().map(|item| item.candidate).collect()
}

fn whole_reading_candidates_demand(reading: &str, entries: &[zhixu::learning::Entry], tick: u64, inherited_order: bool,
    cache: Option<(&mut Option<WholeLattice>, u64)>, accept: Option<&dyn Fn(&str) -> bool>) -> (Vec<MixedCandidate>, bool) {
    #[cfg(test)]
    let _timing = WholeTiming::start(4);
    #[cfg(test)]
    let mut stage = WholeGenerationTiming::start(0);
    let Some(normal_count) = whole_syllable_count(reading) else { return (Vec::new(), true) };
    let compact = reading.replace('\'', "");
    let index = zhixu::current();
    let personal = zhixu::learning_index();
    #[cfg(test)]
    stage.next(1);
    let eligible = |word: &zhixu::WholeWord<'_>| word.text.chars().count() >= 2
        && whole_spelling_matches(reading, word.reading);
    let mut words: Vec<_> = index.ranked_whole_words_for_compact_reading(&compact, normal_count)
        .filter(eligible).map(|word| (word.text, word.weight, word.syllables)).collect();
    let mut additions: Vec<_> = personal.iter().flat_map(|dictionary|
        dictionary.whole_words_for_compact_reading(&compact)).filter(eligible)
        .map(|word| (word.text, word.weight, word.syllables)).collect();
    let repairs = zhixu::fuzzy::jqx_spelling_repairs(reading);
    // A repaired whole dictionary word keeps the same front-of-prefix placement
    // as an exact word. Every repair here costs one edit, so their weight order
    // is unchanged by the common JQX_REPAIR_COST; assembled paths pay it below.
    for repair in &repairs {
        let compact = repair.spelling.replace('\'', "");
        for dictionary in std::iter::once(index.as_ref()).chain(personal.as_deref()) {
            additions.extend(dictionary.whole_words_for_compact_reading(&compact)
                .filter(|word| whole_spelling_matches(&repair.spelling, word.reading))
                .map(|word| (word.text, word.weight, word.syllables)));
        }
    }
    #[cfg(test)]
    stage.next(2);
    let mut learned: HashMap<&str, f64> = HashMap::new();
    for entry in entries.iter().filter(|entry| entry.reading.replace(' ', "") == compact
        && whole_spelling_matches(reading, &entry.reading)
        && entry.text.chars().count() >= 2
        && entry.text.chars().all(|c| ('\u{3400}'..='\u{9fff}').contains(&c)))
    {
        additions.push((entry.text.as_str(), 800, entry.reading.split_whitespace().count()));
        if inherited_order {
            let score = entry.score(tick);
            learned.entry(&entry.text).and_modify(|kept| *kept = kept.max(score)).or_insert(score);
        }
    }
    #[cfg(test)]
    stage.next(4);
    // Only learned scores disturb the index's static weight/text order. Preserve
    // source order for exact ties, including repeated text with another reading.
    if !learned.is_empty() {
        let mut promoted = Vec::new();
        words.retain(|word| {
            if learned.contains_key(word.0) { promoted.push(*word); false } else { true }
        });
        promoted.append(&mut additions);
        additions = promoted;
    }
    let compare = |a: &(&str, u32, usize), b: &(&str, u32, usize)|
        (b.2 == normal_count).cmp(&(a.2 == normal_count))
            .then_with(|| learned.get(b.0).partial_cmp(&learned.get(a.0)).unwrap_or(Ordering::Equal))
            .then(b.1.cmp(&a.1)).then(a.0.cmp(&b.0));
    if !additions.is_empty() {
        additions.sort_by(compare);
        let mut merged = Vec::with_capacity(words.len() + additions.len());
        let mut original = words.into_iter().peekable();
        let mut additions = additions.into_iter().peekable();
        while let (Some(a), Some(b)) = (original.peek(), additions.peek()) {
            if compare(a, b) != Ordering::Greater { merged.push(original.next().unwrap()); }
            else { merged.push(additions.next().unwrap()); }
        }
        merged.extend(original);
        merged.extend(additions);
        words = merged;
    }
    let mut seen = HashSet::new();
    words.retain(|(text, _, _)| seen.insert(*text));
    let cap = words.len().max(lattice_params().per_reading).max(6);
    // Rime shows no sentence when a whole word reads the input (Z2's librime probe): the sentence
    // then stays reachable at the tail, outside the model's group (shangxi: 上系 must not face 赏析).
    let has_word = words.iter().any(|(_, _, count)| *count == normal_count);
    #[cfg(test)]
    drop(stage);
    let mut texts: Vec<_> = words.into_iter().map(|(text, _, count)|
        (text, if count == normal_count { ZHIXU_WHOLE_COMMENT } else { ZHIXU_WHOLE_TAIL_COMMENT })).collect();
    let (lattice, complete) = match accept {
        Some(_) if has_word => (Vec::new(), false),
        Some(accept) => {
            let word_texts: HashSet<_> = texts.iter().map(|(text, _)| *text).collect();
            let stop = |text: &str, count: usize| count == normal_count && text.chars().count() >= 2
                && !word_texts.contains(text) && accept(text);
            decode_whole_lattice_demand(reading, lattice_params().limit.max(cap), cap, 96, entries, tick, cache,
                personal_picks::now_ms(), Some(&stop))
        }
        None => decode_whole_lattice_demand(reading, lattice_params().limit.max(cap), cap, 96, entries, tick, cache,
            personal_picks::now_ms(), None),
    };
    texts.extend(lattice.iter().map(|(text, count)|
        (text.as_str(), if *count == normal_count && !has_word { ZHIXU_WHOLE_SENTENCE_COMMENT } else { ZHIXU_WHOLE_TAIL_COMMENT })));
    #[cfg(test)]
    let _stage = WholeGenerationTiming::start(5);
    let mut seen = HashSet::new();
    let single_repair = normal_count == 1 && zhixu::fuzzy::has_jqx_spelling_error(reading);
    let candidates = texts.into_iter()
        .filter(|(text, _)| (text.chars().count() >= 2 || single_repair) && seen.insert(*text))
        .map(|(text, comment)| MixedCandidate {
            candidate: Candidate { text: text.into(), comment: comment.into(),
                reading_key: reading.into(), source_index: None },
            selection: Selection::Local { text: text.into() }, tier: 2, score: 0,
        }).collect();
    (candidates, complete)
}

/// Score cost of a lone zero-initial syllable inside a whole sentence (about
/// e^2 in weight). Owner replay 10-30: top1 4,082 -> 4,083, none lost; 4000
/// starts to flip kanao. `ZHIZI_ZERO_INITIAL_PENALTY` overrides it for
/// local evaluation only.
fn zero_initial_penalty() -> i64 {
    static PENALTY: OnceLock<i64> = OnceLock::new();
    *PENALTY.get_or_init(|| std::env::var("ZHIZI_ZERO_INITIAL_PENALTY").ok()
        .and_then(|value| value.parse().ok()).unwrap_or(2000))
}

/// The lattice's size limits. Defaults are the shipped values; `ZHIZI_LAT_HOMO`, `ZHIZI_LAT_PER`,
/// `ZHIZI_LAT_LIMIT` and `ZHIZI_LAT_SENT` override them for local evaluation only (A2 of NEXT-ORDER).
struct LatticeParams {
    per_reading: usize,
    limit: usize,
    sentences: usize,
}

fn lattice_params() -> &'static LatticeParams {
    static PARAMS: OnceLock<LatticeParams> = OnceLock::new();
    PARAMS.get_or_init(|| {
        let read = |name: &str, default: usize| {
            std::env::var(name).ok().and_then(|value| value.parse().ok()).filter(|value| *value > 0).unwrap_or(default)
        };
        LatticeParams {
            per_reading: read("ZHIZI_LAT_PER", 3),
            limit: read("ZHIZI_LAT_LIMIT", 48),
            sentences: read("ZHIZI_LAT_SENT", 40),
        }
    })
}

/// P1 of LATTICE-NBEST-PLAN. The whole-reading decoder above keeps one word per reading,
/// so a homophone (搞了 for gaole: 高了 is the best `gao`) can never be generated. This
/// lattice keeps the best few words per reading, prunes by the same lexical score and
/// returns at most `PER_READING` homophone variants of each way of reading the input
/// (which words, by reading), so twelve sentences are twelve different readings rather
/// than twelve spellings of one.
fn decode_lattice(reading: &str, limit: usize, abbreviated: bool) -> Vec<String> {
    decode_lattice_with(reading, limit, abbreviated, None).into_iter().map(|(text, _)| text).collect()
}

fn prune_whole_paths(paths: &mut Vec<WholePath>, cap: usize, beam: usize) {
    #[cfg(test)]
    if let Some((old, new, calls)) = WHOLE_PRUNING_TIMING.with(|timing| timing.take()) {
        let mut reference = paths.clone();
        let original = |paths: &mut Vec<WholePath>| {
            let start = std::time::Instant::now();
            paths.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
            let mut counts = HashMap::new();
            paths.retain(|path| {
                let count = counts.entry(path.2).or_insert(0);
                *count += 1;
                *count <= cap
            });
            paths.truncate(beam.max(cap));
            start.elapsed().as_nanos()
        };
        let optimized = |paths: &mut Vec<WholePath>| {
            let start = std::time::Instant::now();
            prune_whole_paths(paths, cap, beam);
            start.elapsed().as_nanos()
        };
        let (reference_time, optimized_time) = if calls % 2 == 0 {
            (original(&mut reference), optimized(paths))
        } else {
            let optimized_time = optimized(paths);
            (original(&mut reference), optimized_time)
        };
        assert_eq!(*paths, reference);
        WHOLE_PRUNING_TIMING.with(|timing| timing.set(Some((old + reference_time, new + optimized_time, calls + 1))));
        return;
    }
    let maximum = beam.max(cap);
    if paths.len() <= maximum.saturating_mul(2) {
        paths.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
        let mut counts = HashMap::new();
        paths.retain(|path| {
            let count = counts.entry(path.2).or_insert(0);
            *count += 1;
            *count <= cap
        });
        paths.truncate(maximum);
        return;
    }
    // Keep each reading's best cap paths before choosing the global beam.
    // A rejected path already has cap better paths with the same reading, so
    // it cannot survive the original stable sort/filter at any later position.
    let mut readings: HashMap<u64, BinaryHeap<std::cmp::Reverse<WholePathRank<'_>>>> = HashMap::new();
    for (position, path) in paths.iter().enumerate() {
        let ranked = WholePathRank(path, position);
        let kept = readings.entry(path.2).or_default();
        if kept.len() < cap {
            kept.push(std::cmp::Reverse(ranked));
        } else if kept.peek().is_some_and(|worst| ranked > worst.0) {
            kept.pop();
            kept.push(std::cmp::Reverse(ranked));
        }
    }
    let mut indices: Vec<_> = readings.into_values().flatten().map(|ranked| ranked.0.1).collect();
    let compare = |&left: &usize, &right: &usize|
        paths[right].0.cmp(&paths[left].0).then_with(|| paths[left].1.cmp(&paths[right].1))
            .then(left.cmp(&right));
    if indices.len() > maximum {
        indices.select_nth_unstable_by(maximum, compare);
        indices.truncate(maximum);
    }
    indices.sort_unstable_by(compare);
    let mut selected: Vec<_> = indices.into_iter().enumerate().map(|(rank, position)| (position, rank)).collect();
    selected.sort_unstable_by_key(|&(position, _)| position);
    let ranks: Vec<_> = selected.iter().map(|&(_, rank)| rank).collect();
    let mut selected = selected.into_iter().peekable();
    let mut position = 0;
    paths.retain(|_| {
        let keep = selected.peek().is_some_and(|&(wanted, _)| wanted == position);
        if keep { selected.next(); }
        position += 1;
        keep
    });
    let mut ranks = ranks;
    for position in 0..ranks.len() {
        while ranks[position] != position {
            let target = ranks[position];
            paths.swap(position, target);
            ranks.swap(position, target);
        }
    }
}

struct WholeFinalOrder<'a> {
    prefix: std::vec::IntoIter<WholePathRank<'a>>,
    tail: Vec<WholePathRank<'a>>,
    tail_heap: Option<BinaryHeap<WholePathRank<'a>>>,
}

impl<'a> WholeFinalOrder<'a> {
    fn new(paths: &'a [WholePath], maximum: usize) -> Self {
        let mut ranked: Vec<_> = paths.iter().enumerate()
            .map(|(position, path)| WholePathRank(path, position)).collect();
        let tail = if maximum == 0 {
            return Self { prefix: Vec::new().into_iter(), tail: ranked, tail_heap: None };
        } else if ranked.len() > maximum {
            ranked.select_nth_unstable_by(maximum, |a, b| b.cmp(a));
            ranked.split_off(maximum)
        } else { Vec::new() };
        ranked.sort_unstable_by(|a, b| b.cmp(a));
        Self { prefix: ranked.into_iter(), tail, tail_heap: None }
    }
}

impl<'a> Iterator for WholeFinalOrder<'a> {
    type Item = WholePathRank<'a>;
    fn next(&mut self) -> Option<Self::Item> {
        self.prefix.next().or_else(|| self.tail_heap.get_or_insert_with(||
            std::mem::take(&mut self.tail).into()).pop())
    }
}

#[cfg(test)]
thread_local! {
    static WHOLE_PRUNING_TIMING: std::cell::Cell<Option<(u128, u128, usize)>> = const { std::cell::Cell::new(None) };
    static WHOLE_FINAL_POPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static WHOLE_LATTICE_EXPANSIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[test]
fn whole_lattice_saturated_readings_do_not_pop_unused_homophones() {
    let index = Arc::new(Index::from_data(zhixu::loader::DictionaryData {
        entries: (0..1000).map(|position| zhixu::loader::Entry {
            text: char::from_u32(0x4e00 + position).unwrap().to_string(), reading: "a".into(),
            weight: 1000 - position, layer: "fixture".into(),
        }).collect(),
    }));
    zhixu::with_index(index, || {
        WHOLE_FINAL_POPS.with(|pops| pops.set(0));
        assert_eq!(decode_whole_lattice("a", 48, 6, 96, &[], 0, None, 0),
            decode_whole_reference("a", 48, 6, 96, &[], 0, 0));
        WHOLE_FINAL_POPS.with(|pops| assert_eq!(pops.get(), 6));
    });
}

#[test]
fn whole_lattice_prefers_the_split_without_a_lone_interior_zero_initial() {
    let entry = |text: &str, reading: &str, weight| zhixu::loader::Entry {
        text: text.into(), reading: reading.into(), weight, layer: "fixture".into(),
    };
    let index = Arc::new(Index::from_data(zhixu::loader::DictionaryData { entries: vec![
        entry("忙", "mang", 1000), entry("偶", "ou", 1000), entry("慢", "man", 500), entry("够", "gou", 500),
        entry("方", "fang", 1000), entry("按", "an", 1000), entry("方案", "fang an", 10),
    ] }));
    zhixu::with_index(index, || {
        let first = |reading: &str| decode_whole_lattice(reading, 48, 6, 96, &[], 0, None, 0)[0].0.clone();
        // 忙偶 scores higher by weight alone; the lone interior 偶 costs it the lead.
        assert_eq!(first("mangou"), "慢够");
        // An explicit apostrophe is the user's split.
        assert_eq!(first("mang'ou"), "忙偶");
        // A whole word with a zero-initial syllable is not penalized; split
        // characters are (方按 would otherwise outscore the rare word).
        assert_eq!(first("fangan"), "方案");
    });
}

#[test]
#[ignore]
fn whole_lattice_real_dictionary_pruning_paired_timing() {
    let shared = std::env::var("ZHIXU_Z8_SHARED").expect("set ZHIXU_Z8_SHARED to repository Rime resources");
    let user = std::env::temp_dir().join(format!("zhixu-z9-pruning-profile-{}", std::process::id()));
    std::fs::create_dir(&user).unwrap();
    let index = Arc::new(Index::from_data(zhixu::loader::load(std::path::Path::new(&shared), &user).unwrap()));
    zhixu::with_index(index, || {
        // Warm exactly the same dictionary-derived metadata as the measured runs.
        whole_reading_candidates_with("yijian", &[], 0, false, None);
        for keys in ["yijian", "gaole", "womengaichengxiawuchaliao", "duoxianchengdeyilaijiao", "jiexialaizuoshenme", "zhongjianbuyaoting"] {
            WHOLE_PRUNING_TIMING.with(|timing| timing.set(Some((0, 0, 0))));
            for _ in 0..15 {
                let mut cache = None;
                for end in 1..=keys.len() {
                    if whole_syllable_count(&keys[..end]).is_none() { continue; }
                    whole_reading_candidates_with(&keys[..end], &[], 0, false, Some((&mut cache, 0)));
                }
            }
            let (old, new, calls) = WHOLE_PRUNING_TIMING.with(|timing| timing.take().unwrap());
            println!("Z9_PRUNING_PAIRED\t{keys}\tcalls={calls}\told_us={:.3}\tnew_us={:.3}\tratio={:.3}",
                old as f64 / 1e3, new as f64 / 1e3, new as f64 / old as f64);
        }
    });
    std::fs::remove_dir_all(user).unwrap();
}

#[test]
fn whole_lattice_final_order_sorts_only_requested_prefix() {
    let paths: Vec<WholePath> = (0..2000).map(|position|
        ((position * 17 % 23) as i64, format!("词{}", position % 7),
            (position * 31 % 9) as u64, position)).collect();
    let mut expected: Vec<_> = paths.iter().enumerate().map(|(position, path)| WholePathRank(path, position)).collect();
    expected.sort_by(|a, b| b.cmp(a));
    let mut order = WholeFinalOrder::new(&paths, 6);
    let prefix: Vec<_> = order.by_ref().take(6).map(|rank| rank.1).collect();
    assert_eq!(prefix, expected[..6].iter().map(|rank| rank.1).collect::<Vec<_>>());
    assert!(order.tail_heap.is_none(), "unused final paths must remain unheaped and unsorted");
    let tail: Vec<_> = order.map(|rank| rank.1).collect();
    assert_eq!(tail, expected[6..].iter().map(|rank| rank.1).collect::<Vec<_>>());
}

#[test]
fn whole_lattice_prefix_pruning_matches_stable_complete_sort() {
    for cap in [1, 3, 6, 96, 120] {
        for beam in [1, 40, 96] {
            let mut original: Vec<WholePath> = (0..2000).map(|position|
                ((position * 17 % 23) as i64, format!("词{}", position % 7),
                    (position * 31 % 9) as u64, position)).collect();
            // Exact score/text ties retain insertion order, including distinct
            // readings and syllable counts which affect subsequent expansion.
            let mut expected = original.clone();
            expected.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
            let mut counts = HashMap::new();
            expected.retain(|path| {
                let count = counts.entry(path.2).or_insert(0);
                *count += 1;
                *count <= cap
            });
            expected.truncate(beam.max(cap));
            prune_whole_paths(&mut original, cap, beam);
            assert_eq!(original, expected, "cap={cap} beam={beam}");
        }
    }
}

#[test]
#[ignore]
fn whole_lattice_real_dictionary_matches_stable_reference() {
    let shared = std::env::var("ZHIXU_Z8_SHARED").expect("set ZHIXU_Z8_SHARED to repository Rime resources");
    let user = std::env::temp_dir().join(format!("zhixu-z9-lattice-oracle-{}-{}", std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    std::fs::create_dir(&user).unwrap();
    let index = Arc::new(Index::from_data(zhixu::loader::load(std::path::Path::new(&shared), &user).unwrap()));
    let now = 1_000_000;
    let entries = vec![
        zhixu::learning::Entry { reading: "yi jian".into(), text: "乙键".into(), count: 2, last_ms: now, rime: None },
        zhixu::learning::Entry { reading: "gao".into(), text: "搞".into(), count: 1, last_ms: now, rime: None },
    ];
    zhixu::with_index(index, || {
        for learning in [false, true] {
            let _personal = zhixu::pin_learning(learning.then(|| Arc::new(Index::from_learning(&entries))));
            let entries = if learning { entries.as_slice() } else { &[] };
            for cap in [6, 96] {
                for keys in ["yijian", "gaole", "womengaichengxiawuchaliao", "duoxianchengdeyilaijiao", "jiexialaizuoshenme", "zhongjianbuyaoting"] {
                    let mut cache = None;
                    for end in 1..=keys.len() {
                        let reading = &keys[..end];
                        if whole_syllable_count(reading).is_none() { continue; }
                        assert_eq!(decode_whole_lattice(reading, 120, cap, 96, entries, 0, Some((&mut cache, 1)), now),
                            decode_whole_reference(reading, 120, cap, 96, entries, 0, now),
                            "{reading} cap={cap} learning={learning}");
                    }
                    println!("Z9_LATTICE_ORACLE\t{keys}\tcap={cap}\tlearning={learning}\tequal=true");
                }
            }
        }
    });
    std::fs::remove_dir_all(user).unwrap();
}

fn decode_whole_lattice(reading: &str, limit: usize, cap: usize, beam: usize,
    entries: &[zhixu::learning::Entry], tick: u64, cache: Option<(&mut Option<WholeLattice>, u64)>, now: u64) -> Vec<(String, usize)> {
    decode_whole_lattice_demand(reading, limit, cap, beam, entries, tick, cache, now, None).0
}

fn decode_whole_lattice_demand(reading: &str, limit: usize, cap: usize, beam: usize,
    entries: &[zhixu::learning::Entry], tick: u64, mut cache: Option<(&mut Option<WholeLattice>, u64)>, now: u64,
    stop: Option<&dyn Fn(&str, usize) -> bool>) -> (Vec<(String, usize)>, bool) {
    #[cfg(test)]
    let _timing = WholeTiming::start(5);
    #[cfg(test)]
    let mut stage = WholeGenerationTiming::start(0);
    if reading.is_empty() { return (Vec::new(), true); }
    let repair_jqx = zhixu::fuzzy::has_jqx_spelling_error(reading);
    let index = zhixu::current();
    let personal = zhixu::learning_index();
    #[cfg(test)]
    stage.next(2);
    let bonuses: Vec<_> = entries.iter().map(|entry| whole_learning_bonus(entry, tick, now)).collect();
    let mut learning_scores = HashMap::new();
    for (entry, &bonus) in entries.iter().zip(&bonuses) {
        learning_scores.entry((entry.text.as_str(), entry.reading.as_str()))
            .and_modify(|kept: &mut i64| *kept = (*kept).max(bonus)).or_insert(bonus);
    }
    #[cfg(test)]
    stage.next(3);
    let revision = cache.as_ref().map_or(0, |(_, revision)| *revision);
    let fuzzy_mask = zhixu::fuzzy::mask();
    let previous = cache.as_mut().and_then(|(cache, _)| cache.take()).filter(|state|
        state.fuzzy == fuzzy_mask && state.dictionary == index.generation && state.personal == personal.as_ref().map(|index| index.generation)
        && state.learning == revision && state.tick == tick && state.bonuses == bonuses && state.cap == cap && state.beam == beam
        && reading.len() >= state.reading.len() && reading.starts_with(&state.reading)
        && reading.as_bytes()[state.reading.len()..].iter().all(u8::is_ascii_lowercase));
    let reused = previous.as_ref().map_or(0, |state| state.reading.len());
    #[cfg(test)]
    if reused > 0 { WHOLE_REUSED.with(|count| count.set(count.get() + 1)); }
    let mut state = previous.unwrap_or_else(|| {
        let mut paths = vec![Vec::new(); reading.len() + 1];
        paths[0].push((0, String::new(), 0, 0));
        WholeLattice { reading: String::new(), fuzzy: fuzzy_mask, dictionary: index.generation,
            personal: personal.as_ref().map(|index| index.generation), learning: revision, tick,
            bonuses, cap, beam, paths }
    });
    state.paths.resize_with(reading.len() + 1, Vec::new);
    let paths = &mut state.paths;
    if reused < reading.len() {
        #[cfg(test)]
        WHOLE_LATTICE_EXPANSIONS.with(|count| count.set(count.get() + 1));
        for start in 0..reading.len() {
            if paths[start].is_empty() { continue; }
            let mut current = std::mem::take(&mut paths[start]);
            // Interior prefix states are already pruned. The previous final is deliberately
            // kept unpruned until it becomes an interior position on the next append.
            if start >= reused {
                #[cfg(test)]
                stage.next(4);
                prune_whole_paths(&mut current, cap, beam);
                #[cfg(test)]
                stage.next(3);
            }
            if reading.as_bytes()[start] == b'\'' {
                paths[start + 1].extend(current);
                continue;
            }
            let mut seen = HashSet::new();
            let repairs = if repair_jqx { zhixu::fuzzy::jqx_spelling_repairs(&reading[start..]) } else { Vec::new() };
            for dictionary in std::iter::once(index.as_ref()).chain(personal.as_deref()) {
                let mut extend = |offset: usize, word, spelling: &str, repair_cost: i64| {
                    let word: zhixu::WholeWord<'_> = word;
                    let end = start + offset;
                    let spelled = match fuzzy_mask {
                        0 => whole_spelling_matches(spelling, word.reading),
                        mask => zhixu::fuzzy::spelling_matches(spelling, word.reading, mask),
                    };
                    if !spelled
                        || (personal.is_some() && !seen.insert((end, word.text, word.reading)))
                    { return; }
                    let lexical_score = word.lexical_score
                        + learning_scores.get(&(word.text, word.reading)).copied().unwrap_or(0);
                    // A lone zero-initial syllable inside a sentence (忙偶 for mangou) is
                    // usually the other split of the letters (慢够). Whole words that
                    // contain one (方案, 西安) and an explicit apostrophe are unaffected.
                    let interior_zero_initial = start > 0 && word.syllables == 1
                        && reading.as_bytes()[start - 1] != b'\''
                        && matches!(word.reading.as_bytes().first(), Some(b'a' | b'o' | b'e'));
                    let penalty = if interior_zero_initial { zero_initial_penalty() } else { 0 };
                    for (score, text, readings_used, count) in &current {
                        let mut joined = String::with_capacity(text.len() + word.text.len());
                        joined.push_str(text);
                        joined.push_str(word.text);
                        paths[end].push((score + lexical_score - 32236 - penalty - repair_cost, joined,
                            (readings_used ^ word.reading_hash).wrapping_mul(0x0000_0100_0000_01b3), count + word.syllables));
                    }
                };
                dictionary.visit_whole_words_for_prefixes(&reading[start..], reused.saturating_sub(start), |offset, word|
                    extend(offset, word, &reading[start..start + offset], 0));
                for repair in &repairs {
                    dictionary.visit_whole_words_for_prefixes(&repair.spelling, 0, |offset, word| {
                        if let Some(raw_end) = repair.raw_end(offset).filter(|&end| end > reused.saturating_sub(start)) {
                            extend(raw_end, word, &repair.spelling[..offset], zhixu::fuzzy::JQX_REPAIR_COST);
                        }
                    });
                }
            }
            paths[start] = current;
        }
    }
    #[cfg(test)]
    stage.next(4);
    // Pop only the requested prefix of the stable score/text order. Insertion position
    // breaks ties exactly as the original stable sort, without sorting unused tails.
    let final_paths = &paths[reading.len()];
    let single_reading = cap < limit && final_paths.len() > cap && final_paths.first()
        .is_some_and(|first| final_paths.iter().all(|path| path.2 == first.2));
    let mut finals = WholeFinalOrder::new(final_paths, usize::from(stop.is_some()));
    let mut seen = HashSet::new();
    let mut per_reading = HashMap::new();
    let mut result = Vec::new();
    let mut complete = true;
    while result.len() < limit {
        let Some(WholePathRank((_, text, readings_used, count), _)) = finals.next() else { break; };
        #[cfg(test)]
        WHOLE_FINAL_POPS.with(|pops| pops.set(pops.get() + 1));
        if !seen.insert(text.as_str()) { continue; }
        let kept = per_reading.entry(*readings_used).or_insert(0);
        *kept += 1;
        if *kept <= cap {
            #[cfg(test)]
            stage.next(5);
            result.push((text.clone(), *count));
            #[cfg(test)]
            stage.next(4);
            // Every remaining path shares the saturated reading. Keep the
            // unpruned final states for later appends.
            if single_reading && *kept == cap { break; }
            if result.len() < limit && stop.is_some_and(|stop| stop(text, *count)) {
                complete = false;
                break;
            }
        }
    }
    if let Some((cache, _)) = cache {
        #[cfg(test)]
        stage.next(5);
        state.reading = reading.into();
        *cache = Some(state);
    }
    (result, complete)
}

fn decode_lattice_with(reading: &str, limit: usize, abbreviated: bool,
    whole: Option<(usize, &[zhixu::learning::Entry], u64)>) -> Vec<(String, usize)> {
    if let Some((cap, entries, tick)) = whole {
        return decode_whole_lattice(reading, limit, cap, if abbreviated { 40 } else { 96 }, entries, tick, None, personal_picks::now_ms());
    }
    #[cfg(test)]
    let _timing = WholeTiming::start(5);
    let index = zhixu::current();
    let personal = zhixu::learning_index();
    // Abbreviated input opens many more readings per position; a narrower beam keeps a
    // 23-letter abbreviation under the keystroke budget.
    let beam: usize = if abbreviated { 40 } else { 96 };
    let beam_per_reading = 2;
    let beam = beam.max(beam_per_reading);
    const WORD_COST: i64 = 15000;
    const OMITTED_LETTER_COST: i64 = 1500;
    let per_reading_cap = lattice_params().per_reading;
    if reading.is_empty() {
        return Vec::new();
    }
    // The third field hashes the readings used so far (a trie node per word), so two paths
    // are homophone variants of each other exactly when it is the same.
    type LatticePath = (i64, String, u64, usize);
    let mut paths: Vec<Vec<LatticePath>> = vec![Vec::new(); reading.len() + 1];
    paths[0].push((0, String::new(), 0, 0));
    for start in 0..reading.len() {
        if paths[start].is_empty() {
            continue;
        }
        let mut current = std::mem::take(&mut paths[start]);
        current.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
        // Prune by score but keep the readings_used diverse: at most `BEAM_PER_READING` homophone
        // variants of one way of reading the input so far.
        let mut kept_per_reading: HashMap<u64, usize> = HashMap::new();
        current.retain(|(_, _, readings_used, _)| {
            let count = kept_per_reading.entry(*readings_used).or_insert(0);
            *count += 1;
            *count <= beam_per_reading
        });
        current.truncate(beam);
        let mut base_words = HashSet::new();
        for (source, dictionary) in std::iter::once(index.as_ref()).chain(personal.as_deref()).enumerate() {
            for matched in heavy_words_from(dictionary, reading, start, false) {
                if matched.edits != 0
                    || matches!(&reading[start..matched.raw_end], "n" | "m" | "ng" | "hm" | "hng")
                {
                    continue;
                }
                for word in dictionary.trie.terminals(matched.node) {
                    let omitted = i64::from(word.full_letter_count) - (matched.raw_end - start) as i64;
                    if omitted < 0 || (!abbreviated && omitted != 0) {
                        continue;
                    }
                    let word_text = dictionary.text(*word);
                    if personal.is_some() {
                        let key = (matched.raw_end, word_text, omitted);
                        if source == 0 { base_words.insert(key); }
                        else if base_words.contains(&key) { continue; }
                    }
                    // Personal trie nodes have their own namespace; base-only
                    // decoding retains the same reading hashes and tie order.
                    let node = matched.node as u64 + 1 | (source as u64) << 63;
                    let interior_zero_initial = start > 0 && word_text.chars().count() == 1
                        && reading.as_bytes()[start - 1] != b'\''
                        && matches!(reading.as_bytes()[start], b'a' | b'o' | b'e');
                    let penalty = if interior_zero_initial { zero_initial_penalty() } else { 0 };
                    for (score, text, readings_used, count) in &current {
                        let readings_used = (readings_used ^ node).wrapping_mul(0x0000_0100_0000_01b3);
                        paths[matched.raw_end].push((
                            score + word.lexical_score - WORD_COST - OMITTED_LETTER_COST * omitted - penalty,
                            format!("{text}{word_text}"),
                            readings_used,
                            *count,
                        ));
                    }
                }
            }
        }
        paths[start] = current;
    }
    let mut finals = std::mem::take(&mut paths[reading.len()]);
    finals.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
    let mut seen = HashSet::new();
    let mut per_reading: HashMap<u64, usize> = HashMap::new();
    finals
        .into_iter()
        .filter(|(_, text, _, _)| seen.insert(text.clone()))
        .filter(|(_, _, readings_used, _)| {
            let count = per_reading.entry(*readings_used).or_insert(0);
            *count += 1;
            *count <= per_reading_cap
        })
        .map(|(_, text, _, count)| (text, count))
        .take(limit)
        .collect()
}

// Frozen pre-Z7 whole decoder, kept as an independent order oracle.
#[cfg(test)]
fn decode_whole_reference(reading: &str, limit: usize, cap: usize, beam: usize,
    entries: &[zhixu::learning::Entry], tick: u64, now: u64) -> Vec<(String, usize)> {
    let index = zhixu::current();
    let personal = zhixu::learning_index();
    let mut learning_scores: HashMap<(&str, &str), i64> = HashMap::new();
    for entry in entries.iter() {
        let bonus = whole_learning_bonus(entry, tick, now);
        learning_scores.entry((&entry.text, &entry.reading))
            .and_modify(|kept| *kept = (*kept).max(bonus)).or_insert(bonus);
    }
    let beam_per_reading = cap;
    let beam = beam.max(beam_per_reading);
    // librime 1.16.1 without grammar: ln(weight) + ln(1e-8) + ln(1e-6).
    const WHOLE_WORD_COST: i64 = 32236;
    let per_reading_cap = cap;
    if reading.is_empty() {
        return Vec::new();
    }
    // The third field hashes the readings used so far (a trie node per word), so two paths
    // are homophone variants of each other exactly when it is the same.
    type LatticePath = (i64, String, u64, usize);
    let mut paths: Vec<Vec<LatticePath>> = vec![Vec::new(); reading.len() + 1];
    paths[0].push((0, String::new(), 0, 0));
    for start in 0..reading.len() {
        if paths[start].is_empty() {
            continue;
        }
        let mut current = std::mem::take(&mut paths[start]);
        current.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
        // Prune by score but keep the readings_used diverse: at most `BEAM_PER_READING` homophone
        // variants of one way of reading the input so far.
        let mut kept_per_reading: HashMap<u64, usize> = HashMap::new();
        current.retain(|(_, _, readings_used, _)| {
            let count = kept_per_reading.entry(*readings_used).or_insert(0);
            *count += 1;
            *count <= beam_per_reading
        });
        current.truncate(beam);
        if reading.as_bytes()[start] == b'\'' {
            paths[start + 1].extend(current);
            continue;
        }
        // Query the complete index: the correction trie's six-word shortlist is
        // intentionally unchanged for the shipped decoder.
        for end in start + 1..=reading.len() {
            if reading.as_bytes()[end - 1] == b'\'' { continue; }
            let raw_segment = &reading[start..end];
            let segment = raw_segment.replace('\'', "");
            let mut seen = HashSet::new();
            for dictionary in std::iter::once(index.as_ref()).chain(personal.as_deref()) {
                for (word, spaced, weight) in dictionary.all_words_for_compact_reading(&segment) {
                    if !whole_spelling_matches(raw_segment, spaced)
                        || !seen.insert((word, spaced))
                        || crate::zhixu::learning::normalize_reading(spaced).is_none()
                        || !word.chars().all(|c| ('\u{3400}'..='\u{9fff}').contains(&c))
                    { continue; }
                    let node = spaced.bytes().fold(0xcbf2_9ce4_8422_2325u64,
                        |hash, byte| (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3));
                    // Use the same probability scale for words and single characters;
                    // the per-word cost supplies the penalty for additional splits.
                    let lexical_score = ((weight.max(1) as f64).ln() * 1_000.0).round() as i64
                        + learning_scores.get(&(word, spaced)).copied().unwrap_or(0);
                    let penalty = if start > 0 && !spaced.contains(' ') && reading.as_bytes()[start - 1] != b'\''
                        && matches!(spaced.as_bytes().first(), Some(b'a' | b'o' | b'e')) { zero_initial_penalty() } else { 0 };
                    for (score, text, readings_used, count) in &current {
                        paths[end].push((score + lexical_score - WHOLE_WORD_COST - penalty, format!("{text}{word}"),
                            (readings_used ^ node).wrapping_mul(0x0000_0100_0000_01b3), count + spaced.split_whitespace().count()));
                    }
                }
            }
        }
        paths[start] = current;
    }
    let mut finals = std::mem::take(&mut paths[reading.len()]);
    finals.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
    let mut seen = HashSet::new();
    let mut per_reading: HashMap<u64, usize> = HashMap::new();
    finals
        .into_iter()
        .filter(|(_, text, _, _)| seen.insert(text.clone()))
        .filter(|(_, _, readings_used, _)| {
            let count = per_reading.entry(*readings_used).or_insert(0);
            *count += 1;
            *count <= per_reading_cap
        })
        .map(|(_, text, _, count)| (text, count))
        .take(limit)
        .collect()
}

/// On by default; `ZHIZI_LATTICE=0` turns it off, for before/after evaluation only.
fn lattice_enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| !std::env::var("ZHIZI_LATTICE").is_ok_and(|value| value == "0"))
}

/// One row per text on a page. The same text can arrive twice with different sources: a heavy-garble index candidate
/// (a bare `Local` string) and the decoder's ordinary candidate (about 0.3% of pages in the 11-07 replay: 但是皮肤, 搞混了,
/// 指针那个). The row stays where the earlier one was; when the earlier is a bare `Local` and the later is the decoder's
/// own candidate, the decoder's candidate (its selection path teaches the dictionary) takes that row.
fn one_row_per_text(items: &mut Vec<MixedCandidate>) {
    let mut kept: Vec<MixedCandidate> = Vec::with_capacity(items.len());
    for item in items.drain(..) {
        match kept.iter_mut().find(|existing| existing.candidate.text == item.candidate.text) {
            Some(existing) => {
                if matches!(existing.selection, Selection::Local { .. }) && !matches!(item.selection, Selection::Local { .. }) {
                    *existing = item;
                }
            }
            None => kept.push(item),
        }
    }
    *items = kept;
}

fn heavy_garble_candidates(
    raw: &str,
    base: &[MixedCandidate],
    project_terms_enabled: bool,
) -> Vec<MixedCandidate> {
    let Some(reading) = canonical_reading(raw) else {
        return Vec::new();
    };
    if reading.len() < 6
        || reading.len() > 32
        // Preserve the frozen no-candidate raw-commit behavior: this layer is
        // an injection into a real decoder page, not a second decoder.
        || base.is_empty()
        || valid_pinyin_spelling(&reading)
        // Q3-B1b's exact-ASCII predicate is the single English gate for all
        // generated heavy-garble candidates, including project terms.
        || has_registered_ascii_exact(&reading, project_terms_enabled)
        // Q3-B1/Q2 also protect a registered technical prefix (for example
        // `window` -> Windows) from generated Chinese displacement.
        || !tech_terms_for_reading_prefix(&reading).is_empty()
    {
        return Vec::new();
    }
    decode_heavy_garble(&reading)
        .into_iter()
        .take(HEAVY_GARBLE_MAX_CANDIDATES)
        .enumerate()
        .map(|(index, text)| MixedCandidate {
            candidate: Candidate {
                text: text.clone(),
                comment: HEAVY_GARBLE_COMMENT.into(),
                reading_key: reading.clone(),
                source_index: None,
            },
            selection: Selection::Local { text },
            tier: 1,
            score: 510 - index as i64,
        })
        .collect()
}

/// The owner abbreviates a syllable to its initial consonant (kehud = 客户端,
/// buxyao = 不需要), so a typed consonant is never "one letter too many": only an
/// extra vowel inside a syllable (houmioan -> 后面) counts as a slip.
fn drops_a_consonant(typed: &str, word: &str) -> bool {
    if typed.len() != word.len() + 1 {
        return false;
    }
    !typed.char_indices().any(|(index, letter)| {
        let last = index + letter.len_utf8() == typed.len();
        "aeiouv".contains(letter)
            // A final a/e/o is the particle 啊/呃/哦 (maidia = 麦蒂啊), not a slip.
            && !(last && "aeo".contains(letter))
            && typed[..index].to_string() + &typed[index + letter.len_utf8()..] == word
    })
}

/// `woganj` (我感觉), `kehud` (客户端): whole syllables plus one trailing initial is
/// the owner abbreviating the next character, never a typo to correct.
fn ends_with_bare_initial(reading: &str) -> bool {
    let Some(last) = reading.chars().last() else {
        return false;
    };
    let head = &reading[..reading.len() - last.len_utf8()];
    !"aeiouv".contains(last)
        && !head.is_empty()
        && !valid_pinyin_spelling(reading)
        && valid_pinyin_spelling(head)
}

/// One slipped letter the heavy-garble layer does not handle, often because the
/// slip still spells pinyin (`yoyisi` yo-yi-si, `jieidan` ji-ei-dan). When the
/// decoder can only compose the reading from pieces (哟意思, 机诶但 — not a
/// dictionary word), offer the one dictionary word the indexed edit decoder finds.
/// On the owner's records the composed first was meant far more often than not
/// (都做, 改了), so the slip never leads by itself: the context model lifts it.
fn slip_candidates(
    raw: &str,
    base: &[MixedCandidate],
    project_terms_enabled: bool,
) -> Vec<MixedCandidate> {
    let Some(reading) = canonical_reading(raw) else {
        return Vec::new();
    };
    let Some(first) = base.first() else {
        return Vec::new();
    };
    let words = chinese_syllabified_readings_by_text();
    if reading.len() < 4
        || reading.len() > 32
        || !raw.bytes().all(|byte| byte.is_ascii_alphabetic())
        || chinese_exact_readings().contains(&reading)
        || protected_base_readings().contains(&reading)
        || has_registered_ascii_exact(&reading, project_terms_enabled)
        || !tech_terms_for_reading_prefix(&reading).is_empty()
        || ends_with_bare_initial(&reading)
        || words.contains_key(&first.candidate.text)
        || base.iter().any(|item| {
            item.candidate.text.chars().count() >= 2 && words.contains_key(&item.candidate.text)
                && canonical_reading(&item.candidate.reading_key).as_deref() == Some(reading.as_str())
        })
    {
        return Vec::new();
    }
    // One slipped letter (missing, swapped, wrong, or an extra vowel), and the
    // raw is not just the start of the word's reading (`niha` is still being
    // typed toward nihao).
    let one_slip = |text: &String| {
        words.get(text).is_some_and(|readings| {
            readings.iter().any(|(compact, _)| {
                damerau_levenshtein_one(&reading, compact) == Some(1)
                    && !drops_a_consonant(&reading, compact)
                    && !compact.starts_with(reading.as_str())
            })
        })
    };
    decode_heavy_garble(&reading)
        .into_iter()
        .find(|text| text.chars().count() >= 2 && one_slip(text))
        .filter(|text| !base.iter().any(|item| item.candidate.text == *text))
        .map(|text| MixedCandidate {
            candidate: Candidate {
                text: text.clone(),
                comment: SLIP_COMMENT.into(),
                reading_key: reading.clone(),
                source_index: None,
            },
            selection: Selection::Local { text },
            tier: 2,
            score: 0,
        })
        .into_iter()
        .collect()
}

fn chinese_exact_readings() -> &'static HashSet<String> {
    static READINGS: OnceLock<HashSet<String>> = OnceLock::new();
    READINGS.get_or_init(|| {
        include_str!("../../../resources/Rime/agent_exact_readings.tsv")
            .lines()
            .filter_map(|line| {
                let (text, readings) = line.split_once('\t')?;
                text.chars()
                    .any(|character| !character.is_ascii())
                    .then_some(readings)
            })
            .flat_map(|readings| readings.split(','))
            .filter_map(canonical_reading)
            .collect()
    })
}

fn chinese_syllabified_readings_by_text() -> &'static HashMap<String, Vec<(String, String)>> {
    static READINGS: OnceLock<HashMap<String, Vec<(String, String)>>> = OnceLock::new();
    READINGS.get_or_init(|| {
        include_str!("../../../resources/Rime/agent_exact_readings.tsv")
            .lines()
            .filter_map(|line| {
                let (text, readings) = line.split_once('\t')?;
                text.chars()
                    .any(|character| !character.is_ascii())
                    .then(|| {
                        (
                            text.to_string(),
                            readings
                                .split(',')
                                .filter_map(|reading| {
                                    canonical_reading(reading)
                                        .map(|compact| (compact, reading.to_string()))
                                })
                                .collect::<Vec<_>>(),
                        )
                    })
            })
            .collect()
    })
}

fn parse_readings(text: &'static str) -> HashMap<String, Vec<(String, String)>> {
    text.lines()
        .filter_map(|line| {
            let (text, readings) = line.split_once('\t')?;
            text.chars().any(|character| !character.is_ascii()).then(|| {
                (
                    text.to_string(),
                    readings
                        .split(',')
                        .filter_map(|reading| canonical_reading(reading).map(|compact| (compact, reading.to_string())))
                        .collect::<Vec<_>>(),
                )
            })
        })
        .collect()
}

/// Words of the layers added after July (general, new words, spoken, modern, THUOCL IT/finance, AI names, packs) that the frozen
/// index does not know; see generate_exact_reading_index.mjs.
fn later_layer_readings_by_text() -> &'static HashMap<String, Vec<(String, String)>> {
    static READINGS: OnceLock<HashMap<String, Vec<(String, String)>>> = OnceLock::new();
    READINGS.get_or_init(|| parse_readings(include_str!("../../../resources/Rime/agent_extra_readings.tsv")))
}

/// Readings of a dictionary word from any layer the input method loads, for telling a dictionary word from an assembled one.
fn dictionary_readings(text: &str) -> Option<&'static Vec<(String, String)>> {
    chinese_syllabified_readings_by_text().get(text).or_else(|| later_layer_readings_by_text().get(text))
}

fn owner_verified_readings_by_text() -> &'static HashMap<String, Vec<String>> {
    static READINGS: OnceLock<HashMap<String, Vec<String>>> = OnceLock::new();
    READINGS.get_or_init(|| {
        include_str!("../../../resources/Rime/agent_owner_verified_readings.tsv")
            .lines()
            .filter(|line| !line.starts_with('#'))
            .filter_map(|line| {
                let mut columns = line.split('\t');
                let text = columns.next()?;
                let readings = columns.next()?;
                Some((
                    text.to_string(),
                    readings
                        .split(',')
                        .filter(|reading| canonical_reading(reading).is_some())
                        .map(str::to_string)
                        .collect::<Vec<_>>(),
                ))
            })
            .collect()
    })
}

fn owner_verified_hits_by_raw() -> &'static HashMap<String, (String, String)> {
    static HITS: OnceLock<HashMap<String, (String, String)>> = OnceLock::new();
    HITS.get_or_init(|| {
        #[cfg(feature = "diagnostic-no-owner-raw-hits")]
        let rows = "";
        #[cfg(not(feature = "diagnostic-no-owner-raw-hits"))]
        let rows = include_str!("../../../resources/Rime/agent_owner_verified_readings.tsv");
        rows.lines()
            .filter(|line| !line.starts_with('#'))
            .filter_map(|line| {
                let mut columns = line.split('\t');
                let text = columns.next()?.trim();
                let full_reading = columns.next()?.trim();
                let raw_hits = columns.next()?;
                Some(
                    raw_hits
                        .split(',')
                        .filter_map(canonical_reading)
                        .map(|raw| (raw, (text.to_string(), full_reading.to_string())))
                        .collect::<Vec<_>>(),
                )
            })
            .flatten()
            .collect()
    })
}

fn owner_verified_hit_for_raw(raw: &str) -> Option<(&'static str, &'static str)> {
    owner_verified_hits_by_raw()
        .get(&canonical_reading(raw)?)
        .map(|(text, reading)| (text.as_str(), reading.as_str()))
}

fn project_abbreviations() -> &'static HashMap<String, ProjectAbbreviation> {
    static ABBREVIATIONS: OnceLock<HashMap<String, ProjectAbbreviation>> = OnceLock::new();
    ABBREVIATIONS.get_or_init(|| {
        include_str!("../resources/project_abbreviations.tsv")
            .lines()
            .filter(|line| !line.starts_with('#') && line.contains('\t'))
            .filter_map(|line| {
                let mut columns = line.split('\t');
                let abbreviation = columns.next()?.trim().to_ascii_lowercase();
                let text = columns.next()?.trim().to_string();
                let reading = columns.next()?.trim().to_ascii_lowercase();
                let scope = columns.next()?.trim().to_string();
                (!abbreviation.is_empty()
                    && !text.is_empty()
                    && !reading.is_empty()
                    && !scope.is_empty())
                .then(|| {
                    (
                        abbreviation,
                        ProjectAbbreviation {
                            text,
                            reading,
                            scope,
                        },
                    )
                })
            })
            .collect()
    })
}

fn project_abbreviation_for(
    raw: &str,
    active_project: Option<&str>,
    active_packs: &HashSet<String>,
) -> Option<&'static ProjectAbbreviation> {
    let expansion = project_abbreviations().get(&raw.to_ascii_lowercase())?;
    (active_project == Some(expansion.scope.as_str()) || active_packs.contains(&expansion.scope))
        .then_some(expansion)
}

fn canonical_reading(value: &str) -> Option<String> {
    let trimmed = value.trim().to_ascii_lowercase();
    let compact = trimmed
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>();
    (!compact.is_empty()
        && compact
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte == b'\''))
    .then_some(compact)
}

fn normalize_text(value: &str) -> String {
    value.trim().nfkc().collect::<String>()
}

fn candidate_key(candidate: &Candidate, fallback_reading: &str) -> (String, String) {
    let reading =
        canonical_reading(&candidate.reading_key).unwrap_or_else(|| fallback_reading.to_string());
    (normalize_text(&candidate.text), reading)
}

// The whole sequence borrows normalized keys until the retain mask is built.
fn whole_candidate_key<'a>(candidate: &'a Candidate, fallback: &'a str) -> (Cow<'a, str>, Cow<'a, str>) {
    let text = candidate.text.trim();
    let text = if unicode_normalization::is_nfkc_quick(text.chars()) == unicode_normalization::IsNormalized::Yes {
        Cow::Borrowed(text)
    } else { Cow::Owned(normalize_text(text)) };
    let key = &candidate.reading_key;
    let reading = if !key.is_empty() && key.bytes().all(|byte| byte.is_ascii_lowercase() || byte == b'\'') {
        Cow::Borrowed(key.as_str())
    } else {
        canonical_reading(key).map_or(Cow::Borrowed(fallback), Cow::Owned)
    };
    (text, reading)
}

fn retain_unique_whole(candidates: &mut Vec<MixedCandidate>, reading: &str, blacklist: Option<&Store>) {
    let keep: Vec<_> = {
        let mut seen = HashSet::with_capacity(candidates.len());
        candidates.iter().map(|item| {
            let key = whole_candidate_key(&item.candidate, reading);
            !blacklist.is_none_or(|store| !store.is_empty() && store.contains(&key.1, &item.candidate.text))
                && seen.insert(key)
        }).collect()
    };
    let mut keep = keep.into_iter();
    candidates.retain(|_| keep.next().unwrap());
}

fn whole_has_complete_short_reading(reading: &str, candidates: &[MixedCandidate]) -> bool {
    // This guard depends only on the reading key. Hundreds of homophones may
    // share it, so segment each distinct key once while retaining the same any.
    let mut checked = HashMap::new();
    candidates.iter().any(|item| *checked.entry(item.candidate.reading_key.as_str()).or_insert_with(|| {
        canonical_reading(&item.candidate.reading_key).is_some_and(|candidate_reading| {
            candidate_reading.starts_with(reading)
                || syllable_prefix_abbreviation(reading, &item.candidate.reading_key)
                || adjacent_transposition(reading, &candidate_reading)
        }) && pinyin_segmentation(&item.candidate.reading_key)
            .is_some_and(|syllables| syllables.len() < LONG_PHRASE_MIN_SYLLABLES)
    }))
}

fn entry_applies(
    entry: &LexiconEntry,
    active_project: Option<&str>,
    active_packs: &HashSet<String>,
) -> bool {
    let project_matches = entry.project_domains.is_empty()
        || active_project
            .is_some_and(|project| entry.project_domains.iter().any(|domain| domain == project));
    let pack_matches = entry.professional_packs.is_empty()
        || entry
            .professional_packs
            .iter()
            .any(|pack| active_packs.contains(pack));
    project_matches && pack_matches
}

fn source_label(entry: &LexiconEntry) -> String {
    if is_owner_declared_disambiguation(entry) {
        return "个人词 · 你已指定".into();
    }
    let kind = if !entry.project_domains.is_empty() {
        "项目词"
    } else if !entry.professional_packs.is_empty() {
        "专业词"
    } else {
        "个人词"
    };
    let sources = entry
        .sources
        .iter()
        .map(|source| source.source.as_str())
        .collect::<Vec<_>>()
        .join("、");
    format!("{kind} · {sources}")
}

/// The verbatim ASCII fallback ("英文 · 原样") stays reachable on the first page but
/// takes its last slot. In the owner's records it sat third 113 times and was chosen
/// 12 times, 11 of them real English words; otherwise it pushed the intended phrase
/// (做这个项目, 也有可能) from third to fourth. Registered English, tech and project
/// terms carry their own labels and are not moved; nor is the raw spelling of a
/// registered term (mariadb next to MariaDB), which is that term's lowercase variant.
/// The slip sits third, so the model lifts it only past a 12-point margin (6 per
/// slot). On the owner's records lifts above 12 were nearly all real (后面, 节点,
/// 视角, 有意思) while those between 6 and 12 were mostly other intents
/// (词源 -> 裁员, 待选 -> 大选, 你先 -> 内线).
fn place_slip(output: &mut Vec<MixedCandidate>) {
    let Some(position) = output
        .iter()
        .position(|item| item.candidate.comment == SLIP_COMMENT)
    else {
        return;
    };
    let target = 2.min(output.len().saturating_sub(1));
    if position != target {
        let item = output.remove(position);
        output.insert(target, item);
    }
}

/// Common English words (the CLIP vocabulary's whole-word tokens), most frequent first.
fn english_word_ranks() -> &'static HashMap<&'static str, usize> {
    static RANKS: OnceLock<HashMap<&'static str, usize>> = OnceLock::new();
    RANKS.get_or_init(|| {
        include_str!("../resources/english_words.txt")
            .lines()
            .enumerate()
            .map(|(rank, word)| (word, rank))
            .collect()
    })
}

/// One common English word of four or more letters, or two or three of at least three
/// letters each (open+code, deep+seek, draw+things). A part that also spells pinyin
/// counts only when it is a frequent English word (open, mini): the vocabulary holds
/// romanised names (jian, xia, shu) that would turn hurried Chinese such as
/// `nikaolxia` (你考虑下) into English. Rare fragments (aun, hii, jinx past rank
/// 20,000) never join a compound: they spelled slips such as `budaun` (不大).
fn english_word_parts(raw: &str) -> bool {
    const PINYIN_PART_MAX_RANK: usize = 5000;
    const PART_MAX_RANK: usize = 20_000;
    let ranks = english_word_ranks();
    if raw.len() >= 4 && ranks.contains_key(raw) {
        return true;
    }
    // reachable[end] = fewest parts that spell raw[..end]
    let mut reachable = vec![usize::MAX; raw.len() + 1];
    reachable[0] = 0;
    for start in 0..raw.len() {
        if reachable[start] >= 3 {
            continue;
        }
        for end in (start + 3)..=raw.len() {
            let part = &raw[start..end];
            if ranks.get(part).is_some_and(|&rank| {
                rank < PART_MAX_RANK && (rank < PINYIN_PART_MAX_RANK || !valid_pinyin_spelling(part))
            }) {
                reachable[end] = reachable[end].min(reachable[start] + 1);
            }
        }
    }
    (2..=3).contains(&reachable[raw.len()])
}

/// The candidate is ordinary Chinese for this input: dictionary words, at least one of
/// several characters and at most one single character, whose syllables the raw
/// spells in full or abbreviates by prefix, with no slips and at most half of them
/// abbreviated. `went` -> 问题, `meiwent` -> 没问题, `tishic` -> 提示词 qualify;
/// `flash` -> 发拉升 and `freelance` -> 发热而来内测 (four of six abbreviated) do not.
fn clean_chinese_reading(raw: &str, text: &str) -> bool {
    let boundaries = text
        .char_indices()
        .map(|(index, _)| index)
        .chain(std::iter::once(text.len()))
        .collect::<Vec<_>>();
    let readings_by_text = chinese_syllabified_readings_by_text();
    // states[char boundary] = (raw position, single characters, has a longer word,
    // syllables, abbreviated syllables)
    let mut states = vec![HashSet::new(); boundaries.len()];
    states[0].insert((0usize, 0usize, false, 0usize, 0usize));
    for start in 0..boundaries.len() - 1 {
        let current = states[start].iter().copied().collect::<Vec<_>>();
        for end in (start + 1)..boundaries.len() {
            let Some(readings) = readings_by_text.get(&text[boundaries[start]..boundaries[end]]) else {
                continue;
            };
            let single = end - start == 1;
            for (_, syllabified) in readings {
                for &(raw_start, singles, has_word, syllables, abbreviated) in &current {
                    let singles = singles + usize::from(single);
                    if singles > 1 {
                        continue;
                    }
                    let mut positions = HashSet::from([(raw_start, abbreviated)]);
                    for syllable in syllabified.split_whitespace() {
                        positions = positions
                            .into_iter()
                            .flat_map(|(position, abbreviated)| {
                                (position + 1..=raw.len().min(position + syllable.len()))
                                    .filter(move |&next| syllable.starts_with(&raw[position..next]))
                                    .map(move |next| {
                                        (next, abbreviated + usize::from(next - position < syllable.len()))
                                    })
                            })
                            .collect();
                    }
                    let count = syllables + syllabified.split_whitespace().count();
                    for (position, abbreviated) in positions {
                        states[end].insert((position, singles, has_word || !single, count, abbreviated));
                    }
                }
            }
        }
    }
    states[boundaries.len() - 1].iter().any(|&(position, _, has_word, syllables, abbreviated)| {
        position == raw.len() && has_word && abbreviated * 2 <= syllables
    })
}

/// A typed English word that pinyin cannot spell leads verbatim, ahead of the
/// heavy-garble decodes Rime makes of it. Owner records (October): opencode, deepseek,
/// freelance, flash, harness, photoshop sat fifth behind 偶尔弄得 / 第二饿死嗯嗯 and were
/// picked there every time. Valid pinyin spellings (mini, bug) and inputs that also
/// read as ordinary Chinese (went -> 问题, ship -> 视频, dans -> 但是) never qualify.
fn english_word_leads(raw: &str, candidates: &[MixedCandidate]) -> bool {
    raw.len() >= 4
        && raw.bytes().all(|byte| byte.is_ascii_lowercase())
        && !valid_pinyin_spelling(raw)
        && !pinyin_still_being_typed(raw)
        && english_word_parts(raw)
        && !candidates.iter().any(|item| {
            let text = &item.candidate.text;
            text.chars().any(|character| !character.is_ascii()) && clean_chinese_reading(raw, text)
        })
}

/// Full syllables followed by the start of one more (`zai` + `n`, `o pu` + `s`): the
/// owner is still typing pinyin.
fn pinyin_still_being_typed(raw: &str) -> bool {
    let syllables = pinyin_syllables();
    (1..raw.len()).any(|split| {
        let tail = &raw[split..];
        valid_pinyin_spelling(&raw[..split])
            && syllables
                .iter()
                .any(|syllable| syllable.len() > tail.len() && syllable.starts_with(tail))
    })
}

fn lead_with_english_word(raw: &str, reading: &str, output: &mut Vec<MixedCandidate>) {
    // Registered English (Claude, or Windows completing `wind`) already leads.
    if output.first().is_some_and(|item| item.candidate.text.is_ascii()) {
        return;
    }
    let item = match output.iter().position(|item| item.candidate.text == raw) {
        Some(position) => output.remove(position),
        None => MixedCandidate {
            candidate: Candidate {
                text: raw.to_string(),
                comment: "英文 · 原样".into(),
                reading_key: reading.to_string(),
                source_index: None,
            },
            selection: Selection::Local { text: raw.to_string() },
            tier: 2,
            score: 519,
        },
    };
    output.insert(0, item);
}

/// The English the owner last picked deliberately for this input leads.
fn lead_with_learned_english(text: &str, reading: &str, output: &mut Vec<MixedCandidate>) {
    let item = match output.iter().position(|item| item.candidate.text == text) {
        Some(0) => return,
        Some(position) => output.remove(position),
        None => MixedCandidate {
            candidate: Candidate {
                text: text.to_string(),
                comment: "英文 · 你用过".into(),
                reading_key: reading.to_string(),
                source_index: None,
            },
            selection: Selection::Local { text: text.to_string() },
            tier: 2,
            score: 519,
        },
    };
    output.insert(0, item);
}

/// Three-letter English words (mac, key, env) are not pinyin, but the same three letters
/// are also the initials of Chinese words, so they never lead (the frozen digit-selection
/// feel of slots 1-4 stays): the typed word takes the last slot of the first page.
/// Owner decision 2026-10-11.
fn place_short_english(raw: &str, reading: &str, output: &mut Vec<MixedCandidate>) {
    const LAST_SLOT: usize = 4;
    const MAX_RANK: usize = 20_000;
    // Short tech words the general English list ranks too low or lacks.
    const TECH: &[&str] = &[
        "env", "cpu", "gpu", "ssh", "sql", "css", "pdf", "url", "ide", "usb", "ssd", "tcp", "udp", "dns", "jwt",
        "xml", "yml", "cdn", "orm", "vim", "npm", "pip", "cli", "sdk", "api",
    ];
    if raw.len() != 3 || !raw.bytes().all(|byte| byte.is_ascii_lowercase()) {
        return;
    }
    // env spells e-n-v in pinyin; a listed tech word still counts.
    if !TECH.contains(&raw)
        && (valid_pinyin_spelling(raw) || !english_word_ranks().get(raw).is_some_and(|&rank| rank < MAX_RANK))
    {
        return;
    }
    if output.iter().take(LAST_SLOT + 1).any(|item| item.candidate.text.eq_ignore_ascii_case(raw)) {
        return;
    }
    let item = match output.iter().position(|item| item.candidate.text == raw) {
        Some(position) => output.remove(position),
        None => MixedCandidate {
            candidate: Candidate {
                text: raw.to_string(),
                comment: "英文 · 原样".into(),
                reading_key: reading.to_string(),
                source_index: None,
            },
            selection: Selection::Local { text: raw.to_string() },
            tier: 2,
            score: 519,
        },
    };
    output.insert(LAST_SLOT.min(output.len()), item);
}

fn demote_raw_ascii_to_last_first_page_slot(raw: &str, output: &mut Vec<MixedCandidate>) {
    const FIRST_PAGE_LAST_SLOT: usize = 4;
    if output
        .iter()
        .any(|item| item.candidate.text != raw && item.candidate.text.eq_ignore_ascii_case(raw))
    {
        return;
    }
    let Some(position) = output.iter().position(|item| {
        item.candidate.text == raw && item.candidate.comment == "英文 · 原样"
    }) else {
        return;
    };
    let target = FIRST_PAGE_LAST_SLOT.min(output.len() - 1);
    if position < target {
        let item = output.remove(position);
        output.insert(target, item);
    }
}

fn mixed_order(left: &MixedCandidate, right: &MixedCandidate) -> Ordering {
    left.tier
        .cmp(&right.tier)
        .then_with(|| right.score.cmp(&left.score))
        .then_with(|| left.candidate.reading_key.cmp(&right.candidate.reading_key))
        .then_with(|| {
            normalize_text(&left.candidate.text).cmp(&normalize_text(&right.candidate.text))
        })
}

fn plausible_nonexact_reading(raw: &str, candidate_reading: &str) -> bool {
    let Some(candidate) = canonical_reading(candidate_reading) else {
        return false;
    };
    // One letter typed past the end of a word is the next syllable or a particle (huiliaoa 回聊啊, ...juedingx 决定先), not a slip:
    // a word that stops before it does not cover the input.
    let ends_early = raw.len() == candidate.len() + 1 && raw.starts_with(&candidate);
    candidate.starts_with(raw)
        || (!ends_early && damerau_levenshtein_one(raw, &candidate) == Some(1))
        || syllable_prefix_abbreviation(raw, candidate_reading)
}

/// Whether `candidate` spells `reading` exactly. A word any dictionary layer knows is
/// judged by its true readings: with fuzzy pinyin on, the reading key of a word the
/// runtime exact-readings index lacks (the post-July layers: zenm -> 榛蘑) is the raw
/// input echoed back, and trusting it would protect a fuzzy derivation as an exact
/// spelling. A word no layer knows is an assembled parse whose echoed key is the only
/// evidence that it covers the whole input.
fn exact_spelling(reading: &str, candidate: &Candidate) -> bool {
    match dictionary_readings(&candidate.text) {
        Some(readings) => readings.iter().any(|(compact, _)| compact == reading),
        None => canonical_reading(&candidate.reading_key).as_deref() == Some(reading),
    }
}

fn syllable_prefix_abbreviation(raw: &str, candidate_reading: &str) -> bool {
    let normalized = candidate_reading.trim().to_ascii_lowercase();
    let syllables = normalized
        .split(|character: char| character.is_whitespace() || character == '\'')
        .filter(|syllable| !syllable.is_empty())
        .collect::<Vec<_>>();
    if syllables.len() < 2 || raw.len() < syllables.len() {
        return false;
    }

    fn consumes(raw: &str, syllables: &[&str]) -> bool {
        let Some((syllable, remaining)) = syllables.split_first() else {
            return raw.is_empty();
        };
        let remaining_minimum = remaining.len();
        let maximum = syllable
            .len()
            .min(raw.len().saturating_sub(remaining_minimum));
        (1..=maximum).any(|length| {
            syllable.starts_with(&raw[..length]) && consumes(&raw[length..], remaining)
        })
    }

    consumes(raw, &syllables)
}

fn consume_dictionary_syllables(
    raw: &str,
    raw_start: usize,
    candidate_reading: &str,
    nonexact_so_far: usize,
) -> Vec<(usize, usize, usize)> {
    let normalized = candidate_reading.trim().to_ascii_lowercase();
    let syllables = normalized
        .split(|character: char| character.is_whitespace() || character == '\'')
        .filter(|syllable| !syllable.is_empty())
        .collect::<Vec<_>>();
    if syllables.is_empty() {
        return Vec::new();
    }

    let mut states = HashSet::from([(raw_start, nonexact_so_far)]);
    for (index, syllable) in syllables.iter().enumerate() {
        let remaining_minimum = syllables.len() - index - 1;
        let mut next = HashSet::new();
        for (start, nonexact) in states {
            let maximum = raw
                .len()
                .saturating_sub(remaining_minimum)
                .min(start.saturating_add(syllable.len() + 1));
            for end in (start + 1)..=maximum {
                let raw_syllable = &raw[start..end];
                let added = if raw_syllable == *syllable {
                    0
                } else if syllable.starts_with(raw_syllable)
                    || damerau_levenshtein_one(raw_syllable, syllable) == Some(1)
                {
                    1
                } else {
                    continue;
                };
                if nonexact + added <= LONG_PHRASE_MAX_NONEXACT_SYLLABLES {
                    next.insert((end, nonexact + added));
                }
            }
        }
        states = next;
        if states.is_empty() {
            return Vec::new();
        }
    }

    states
        .into_iter()
        .map(|(raw_end, nonexact)| (raw_end, nonexact, syllables.len()))
        .collect()
}

fn dictionary_phrase_match_syllable_count(raw: &str, text: &str) -> Option<usize> {
    // Full dictionary entries are handled by plausible_nonexact_reading. This
    // path proves that a Rime-composed candidate is itself a phrase made from
    // dictionary entries, then aligns the raw input to their preserved pinyin
    // syllables. It only inspects an existing candidate; it never creates one.
    let boundaries = text
        .char_indices()
        .map(|(index, _)| index)
        .chain(std::iter::once(text.len()))
        .collect::<Vec<_>>();
    if boundaries.len() < 3 {
        return None;
    }

    // (raw byte, syllable count, non-exact syllables, capped word count,
    // contains a multi-character dictionary word)
    let mut states = vec![HashSet::new(); boundaries.len()];
    states[0].insert((0usize, 0usize, 0usize, 0usize, false));
    let readings_by_text = chinese_syllabified_readings_by_text();

    for start in 0..boundaries.len() - 1 {
        let current = states[start].iter().copied().collect::<Vec<_>>();
        if current.is_empty() {
            continue;
        }
        for end in (start + 1)..boundaries.len() {
            let word = &text[boundaries[start]..boundaries[end]];
            let Some(readings) = readings_by_text.get(word) else {
                continue;
            };
            for (_, syllabified) in readings {
                for &(raw_start, syllable_count, nonexact, word_count, has_word) in &current {
                    for (raw_end, new_nonexact, added_syllables) in
                        consume_dictionary_syllables(raw, raw_start, syllabified, nonexact)
                    {
                        states[end].insert((
                            raw_end,
                            syllable_count + added_syllables,
                            new_nonexact,
                            (word_count + 1).min(2),
                            has_word || end - start > 1,
                        ));
                    }
                }
            }
        }
    }

    states.last().and_then(|final_states| {
        final_states
            .iter()
            .filter_map(
                |&(raw_end, syllable_count, nonexact, word_count, has_word)| {
                    (raw_end == raw.len()
                        && syllable_count >= LONG_PHRASE_MIN_SYLLABLES
                        && nonexact <= LONG_PHRASE_MAX_NONEXACT_SYLLABLES
                        && word_count >= 2
                        && has_word)
                        .then_some(syllable_count)
                },
            )
            .max()
    })
}

fn dictionary_phrase_matches_raw(raw: &str, text: &str) -> bool {
    dictionary_phrase_match_syllable_count(raw, text).is_some()
}

fn promote_complete_dictionary_candidate_to_top(
    raw: &str,
    project_terms_enabled: bool,
    candidates: &mut [MixedCandidate],
) {
    promote_complete_dictionary_candidate_to_top_with_short_reading(raw, project_terms_enabled, candidates, None, false);
}

fn promote_complete_dictionary_candidate_to_top_with_short_reading(
    raw: &str,
    project_terms_enabled: bool,
    candidates: &mut [MixedCandidate],
    short_reading: Option<bool>,
    native_readings: bool,
) {
    if candidates.len() < 2 {
        return;
    }
    let Some(reading) = canonical_reading(raw) else {
        return;
    };
    // The ordinary plausible-word pass already chose a native first piece.
    // Keep it while the remaining input is incomplete; a complete spelling or
    // a whole abbreviated word still goes through the original whole-word rule.
    if native_readings && !valid_pinyin_spelling(&reading)
        && candidates.first().is_some_and(|item| item.candidate.text.chars().count() >= 2
            && zhixu::decoder::candidate_coverage(raw, &item.candidate).is_some_and(|end| end < raw.len())) {
        return;
    }
    let has_complete_short_reading = short_reading.unwrap_or_else(|| candidates.iter().any(|item| {
        canonical_reading(&item.candidate.reading_key).is_some_and(|candidate_reading| {
            (candidate_reading.starts_with(&reading)
                || syllable_prefix_abbreviation(&reading, &item.candidate.reading_key))
                || adjacent_transposition(&reading, &candidate_reading)
        }) && pinyin_segmentation(&item.candidate.reading_key)
            .is_some_and(|syllables| syllables.len() < LONG_PHRASE_MIN_SYLLABLES)
    }));
    if has_complete_short_reading {
        return;
    }
    if has_registered_ascii_exact(&reading, project_terms_enabled) {
        return;
    }
    let complete_syllable_count = |item: &MixedCandidate| {
        let candidate_reading =
            canonical_reading(&item.candidate.reading_key).unwrap_or_else(|| reading.clone());
        dictionary_phrase_match_syllable_count(&reading, &item.candidate.text)
            .or_else(|| {
                // Native candidates always carry the completed pronunciation.
                // It is not evidence that the typed input covers that sentence:
                // leiss -> lei si shi must not displace the first piece 类似.
                (!native_readings && candidate_reading != reading
                    && plausible_nonexact_reading(&reading, &item.candidate.reading_key))
                .then(|| {
                    dictionary_phrase_match_syllable_count(&candidate_reading, &item.candidate.text)
                })?
            })
            .or_else(|| {
                owner_verified_readings_by_text()
                    .get(&item.candidate.text)?
                    .iter()
                    .filter(|verified| plausible_nonexact_reading(&reading, verified))
                    .filter_map(|verified| {
                        let verified = canonical_reading(verified)?;
                        dictionary_phrase_match_syllable_count(&verified, &item.candidate.text)
                    })
                    .max()
            })
    };
    if complete_syllable_count(&candidates[0]).is_some() {
        return;
    }
    let mut best_full: Option<(usize, usize)> = None;
    for (index, item) in candidates.iter().enumerate() {
        if item.candidate.comment == SLIP_COMMENT {
            continue;
        }
        let Some(syllable_count) = complete_syllable_count(item) else {
            continue;
        };
        if best_full.is_none_or(|(_, best_count)| syllable_count > best_count) {
            best_full = Some((index, syllable_count));
        }
    }
    let Some((full_index, full_syllable_count)) = best_full else {
        return;
    };
    if full_index == 0 {
        return;
    }
    if full_syllable_count >= LONG_PHRASE_MIN_SYLLABLES {
        candidates.swap(0, full_index);
    }
}

fn should_promote_raw_ascii(raw: &str, candidates: &[MixedCandidate]) -> bool {
    let Some(reading) = canonical_reading(raw) else {
        return false;
    };
    if !raw.bytes().all(|byte| byte.is_ascii_alphabetic()) {
        return false;
    }
    if candidates.is_empty() {
        return false;
    }
    // Short inputs are overwhelmingly Chinese initials and also carry frozen
    // digit-selection muscle memory. Exact local terms (for example API) are
    // still inserted by local_ascii_candidates.
    if reading.len() < 4
        || valid_pinyin_spelling(&reading)
        || chinese_exact_readings().contains(&reading)
        || protected_base_readings().contains(&reading)
    {
        return false;
    }
    if has_exact_or_plausible_chinese(raw, candidates) {
        return false;
    }
    true
}

fn has_exact_or_plausible_chinese(raw: &str, candidates: &[MixedCandidate]) -> bool {
    let Some(reading) = canonical_reading(raw) else {
        return false;
    };
    candidates.iter().any(|item| {
        let text = &item.candidate.text;
        text.chars().any(|character| !character.is_ascii())
            && (chinese_syllabified_readings_by_text()
                .get(text)
                .is_some_and(|readings| {
                    readings.iter().any(|(compact, syllabified)| {
                        compact == &reading || plausible_nonexact_reading(&reading, syllabified)
                    })
                })
                || dictionary_phrase_matches_raw(&reading, text))
    })
}

fn has_exact_chinese(reading: &str, candidates: &[MixedCandidate]) -> bool {
    candidates.iter().any(|item| {
        item.candidate
            .text
            .chars()
            .any(|character| !character.is_ascii())
            && chinese_syllabified_readings_by_text()
                .get(&item.candidate.text)
                .is_some_and(|readings| readings.iter().any(|(compact, _)| compact == reading))
    })
}

fn has_registered_ascii_exact(reading: &str, project_terms_enabled: bool) -> bool {
    (project_terms_enabled && project_term_for_reading(reading).is_some())
        || english_term_readings().contains_key(reading)
        || tech_term_readings().contains_key(reading)
}

/// ASCII terms whose reading starts with the typed letters. The shortest reading comes first (`so` -> Sora before SolidWorks), and a
/// very short prefix may fill at most one place of the page: with the vocabulary packs a two-letter prefix matches dozens of names,
/// and a page of five brand names would push every Chinese candidate off it (`so`, found by the double-letter corpus 2026-10-14).
fn tech_terms_for_reading_prefix(reading: &str) -> Vec<String> {
    let terms = tech_term_readings();
    let mut matches = terms
        .iter()
        .filter(|(term_reading, _)| term_reading.starts_with(reading))
        .flat_map(|(term_reading, texts)| texts.iter().map(move |text| (term_reading.len(), text.as_str())))
        .collect::<Vec<_>>();
    matches.sort_unstable();
    let mut seen = std::collections::HashSet::new();
    matches.retain(|(_, text)| seen.insert(*text));
    let cap = match reading.len() {
        0..=2 => 1,
        3 => 3,
        _ => usize::MAX,
    };
    matches.into_iter().map(|(_, text)| text.to_string()).take(cap).collect()
}

fn canonical_embedded_ascii_terms() -> Arc<Vec<(String, String)>> {
    zhixu::terms().embedded.clone()
}

fn embedded_ascii_match(raw: &str) -> Option<(usize, usize, String, String)> {
    let reading = canonical_reading(raw)?;
    if !raw.bytes().all(|byte| byte.is_ascii_alphabetic()) || valid_pinyin_spelling(&reading) {
        return None;
    }

    let mut matches = Vec::new();
    for (term_reading, text) in canonical_embedded_ascii_terms().iter() {
        // The narrow fallback only owns a segment that cannot itself form a
        // pinyin syllable chain. Ambiguous registered terms such as API
        // retain the existing pure-pinyin behavior.
        if term_reading.len() < 3 || valid_pinyin_spelling(term_reading) {
            continue;
        }
        for (start, _) in reading.match_indices(term_reading) {
            let end = start + term_reading.len();
            if start == 0 || end == reading.len() {
                continue;
            }
            let Some(prefix) = pinyin_segmentation(&reading[..start]) else {
                continue;
            };
            let Some(suffix) = pinyin_segmentation(&reading[end..]) else {
                continue;
            };
            if prefix.len() + suffix.len() < 2 {
                continue;
            }
            matches.push((start, end, term_reading.clone(), text.clone()));
        }
    }
    (matches.len() == 1).then(|| matches.remove(0))
}

fn exact_dictionary_composition_matches(raw: &str, text: &str) -> bool {
    let Some(reading) = canonical_reading(raw) else {
        return false;
    };
    if text.is_empty()
        || !text.chars().all(|character| !character.is_ascii())
        || !valid_pinyin_spelling(&reading)
    {
        return false;
    }
    let boundaries = text
        .char_indices()
        .map(|(index, _)| index)
        .chain(std::iter::once(text.len()))
        .collect::<Vec<_>>();
    let readings_by_text = chinese_syllabified_readings_by_text();
    let mut states = vec![HashSet::new(); boundaries.len()];
    states[0].insert(0usize);
    for start in 0..boundaries.len().saturating_sub(1) {
        let raw_starts = states[start].iter().copied().collect::<Vec<_>>();
        if raw_starts.is_empty() {
            continue;
        }
        for end in (start + 1)..boundaries.len() {
            let word = &text[boundaries[start]..boundaries[end]];
            let Some(word_readings) = readings_by_text.get(word) else {
                continue;
            };
            for (compact, _) in word_readings {
                for &raw_start in &raw_starts {
                    let raw_end = raw_start + compact.len();
                    if reading
                        .get(raw_start..raw_end)
                        .is_some_and(|segment| segment == compact)
                    {
                        states[end].insert(raw_end);
                    }
                }
            }
        }
    }
    states
        .last()
        .is_some_and(|raw_ends| raw_ends.contains(&reading.len()))
}

fn chinese_side_from_candidates(
    raw: &str,
    candidates: &[Candidate],
    prefix: bool,
) -> Option<String> {
    for candidate in candidates.iter().take(256) {
        let text = candidate.text.as_str();
        if text.chars().count() > 64 || !text.chars().all(|character| !character.is_ascii()) {
            continue;
        }
        let boundaries = text
            .char_indices()
            .map(|(index, _)| index)
            .chain(std::iter::once(text.len()))
            .collect::<Vec<_>>();
        if prefix {
            for end in (1..boundaries.len()).rev() {
                let side = &text[..boundaries[end]];
                if exact_dictionary_composition_matches(raw, side) {
                    return Some(side.to_string());
                }
            }
        } else {
            for start in 0..boundaries.len().saturating_sub(1) {
                let side = &text[boundaries[start]..];
                if exact_dictionary_composition_matches(raw, side) {
                    return Some(side.to_string());
                }
            }
        }
    }
    None
}

fn embedded_ascii_candidate(raw: &str, candidates: &[Candidate]) -> Option<MixedCandidate> {
    let (start, end, _, canonical_text) = embedded_ascii_match(raw)?;
    let reading = canonical_reading(raw)?;
    let prefix_raw = &reading[..start];
    let suffix_raw = &reading[end..];
    // A side the page does not spell (the page's whole-reading guesses may end in another word: 当前这台盘后女儿
    // carries no 啊) is decoded on its own, still held to an exact dictionary composition of that side.
    let side = |raw: &str, prefix: bool| {
        chinese_side_from_candidates(raw, candidates, prefix).or_else(|| {
            decode_lattice(raw, 3, false)
                .into_iter()
                .find(|text| exact_dictionary_composition_matches(raw, text))
        })
    };
    let sides = (side(prefix_raw, true)?, side(suffix_raw, false)?);
    let text = format!("{}{canonical_text}{}", sides.0, sides.1);
    Some(MixedCandidate {
        candidate: Candidate {
            text: text.clone(),
            comment: "中英混输 · 规范词典".into(),
            reading_key: reading,
            source_index: None,
        },
        selection: Selection::Local { text },
        tier: 0,
        score: 525,
    })
}

fn local_ascii_candidates(raw: &str, base_candidates: &[MixedCandidate], code_ascii_bias: bool,
    typo_enabled: bool) -> Vec<MixedCandidate> {
    local_ascii_candidates_with_prefix_evidence(raw, base_candidates, base_candidates, code_ascii_bias, typo_enabled)
}

fn local_ascii_candidates_with_prefix_evidence(
    raw: &str,
    base_candidates: &[MixedCandidate],
    prefix_evidence: &[MixedCandidate],
    code_ascii_bias: bool,
    typo_enabled: bool,
) -> Vec<MixedCandidate> {
    let Some(reading) = canonical_reading(raw) else {
        return Vec::new();
    };
    if !raw.bytes().all(|byte| byte.is_ascii_alphabetic()) {
        return Vec::new();
    }

    let project = project_term_for_reading(&reading);
    let english = english_term_readings();
    let tech = tech_term_readings();
    let exact_english = english.get(&reading);
    let exact_tech = tech.get(&reading);
    let allow_ascii_prefix = !has_exact_or_plausible_chinese(raw, prefix_evidence);
    let prefix_tech = (exact_tech.is_none() && allow_ascii_prefix)
        .then(|| tech_terms_for_reading_prefix(&reading))
        .unwrap_or_default();
    let has_tech = exact_tech.is_some() || !prefix_tech.is_empty();
    // A registered ASCII exact reading remains ahead of a merely plausible
    // (K=1) Chinese phrase. An exact Chinese reading still owns the normal
    // Chinese order, so short collisions such as `ci` are unchanged.
    let protect_registered_exact = has_registered_ascii_exact(&reading, code_ascii_bias)
        && !has_exact_chinese(&reading, base_candidates);
    let promote_raw = !prefix_tech.is_empty()
        || protect_registered_exact
        || should_promote_raw_ascii(raw, base_candidates)
            && !(typo_enabled && !typo_candidates(raw).is_empty());
    if project.is_none() && exact_english.is_none() && !has_tech && !promote_raw {
        return Vec::new();
    }
    if !promote_raw && !code_ascii_bias {
        return Vec::new();
    }

    let mut ordered = Vec::new();
    let mut push_unique = |text: &str, label: &'static str, score: i64| {
        if !ordered.iter().any(|(existing, _, _)| existing == text) {
            ordered.push((text.to_string(), label, score));
        }
    };
    if let Some(values) = exact_tech {
        for (index, text) in values.iter().enumerate() {
            push_unique(text, "英文 · 行业词", 523 - index as i64);
        }
    } else {
        for (index, text) in prefix_tech.iter().enumerate() {
            push_unique(text, "英文 · 行业词前缀", 523 - index as i64);
        }
    }
    if let Some(values) = exact_english {
        for (index, text) in values.iter().enumerate() {
            push_unique(text, "英文 · 英文词", 521 - index as i64);
        }
    }
    if !code_ascii_bias && !has_tech && project.is_some() && promote_raw {
        push_unique(raw, "英文 · 原样", 523);
    }
    if code_ascii_bias {
        if let Some(text) = project {
            push_unique(&text, "英文 · 项目词", 520);
        }
    } else {
        if let Some(text) = project {
            push_unique(&text, "英文 · 项目词", 522);
        }
    }
    if promote_raw {
        push_unique(raw, "英文 · 原样", 519);
    }

    ordered
        .into_iter()
        .map(|(text, comment, score)| MixedCandidate {
            candidate: Candidate {
                text: text.clone(),
                comment: comment.into(),
                reading_key: reading.clone(),
                source_index: None,
            },
            selection: Selection::Local { text },
            // Personalized/fixed entries keep their existing priority. This
            // overlay is chained before the base tier, so tier 2 still places
            // exact local ASCII and raw fallback ahead of Rime garbage.
            tier: 2,
            score,
        })
        .collect()
}

fn valid_pinyin_spelling(reading: &str) -> bool {
    if reading.is_empty() {
        return false;
    }
    if reading.contains('\'') {
        return reading.split('\'').all(valid_pinyin_spelling);
    }
    let syllables = pinyin_syllables();
    let mut reachable = vec![false; reading.len() + 1];
    reachable[0] = true;
    for start in 0..reading.len() {
        if !reachable[start] {
            continue;
        }
        for end in (start + 1)..=reading.len().min(start + 6) {
            if syllables.contains(&reading[start..end]) {
                reachable[end] = true;
            }
        }
    }
    reachable[reading.len()]
}

fn pinyin_segmentation(value: &str) -> Option<Vec<String>> {
    let reading = canonical_reading(value)?;
    // The table below stores every suffix: quadratic, and nobody types a hundred letters at once.
    if reading.len() > 120 {
        return None;
    }
    if reading.contains('\'') {
        let segments = reading
            .split('\'')
            .filter(|segment| !segment.is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>();
        return (!segments.is_empty()
            && segments
                .iter()
                .all(|segment| pinyin_syllables().contains(segment.as_str())))
        .then_some(segments);
    }
    let syllables = pinyin_syllables();
    let mut best: Vec<Option<Vec<String>>> = vec![None; reading.len() + 1];
    best[reading.len()] = Some(Vec::new());
    for start in (0..reading.len()).rev() {
        let mut winner: Option<Vec<String>> = None;
        for end in ((start + 1)..=reading.len().min(start + 6)).rev() {
            let segment = &reading[start..end];
            if !syllables.contains(segment) {
                continue;
            }
            let Some(suffix) = best[end].as_ref() else {
                continue;
            };
            let mut candidate = Vec::with_capacity(suffix.len() + 1);
            candidate.push(segment.to_string());
            candidate.extend(suffix.iter().cloned());
            if winner
                .as_ref()
                .is_none_or(|current| candidate.len() < current.len())
            {
                winner = Some(candidate);
            }
        }
        best[start] = winner;
    }
    best[0].clone()
}

pub(crate) fn pinyin_syllables() -> &'static HashSet<&'static str> {
    static SYLLABLES: OnceLock<HashSet<&'static str>> = OnceLock::new();
    SYLLABLES.get_or_init(|| PINYIN_SYLLABLES.split_whitespace().collect())
}

#[derive(Deserialize)]
struct BaseCorpus {
    cases: Vec<BaseCase>,
}

#[derive(Deserialize)]
struct BaseCase {
    query: String,
    allowed: Vec<String>,
}

fn typo_dictionary() -> &'static Vec<(String, String)> {
    static DICTIONARY: OnceLock<Vec<(String, String)>> = OnceLock::new();
    DICTIONARY.get_or_init(|| {
        // Reachable lazily from the typing path: a corrupt embedded corpus must
        // degrade to "no typo correction", never abort the host. Validity is
        // separately enforced by frozen_behavior tests and the base-500 gate.
        let Ok(parsed) = serde_json::from_str::<BaseCorpus>(include_str!(
            "../../../../quality/corpus/base-500.json"
        )) else {
            return Vec::new();
        };
        parsed
            .cases
            .into_iter()
            .filter_map(|case| {
                case.allowed
                    .into_iter()
                    .next()
                    .map(|text| (case.query, text))
            })
            .collect()
    })
}

fn protected_base_readings() -> &'static HashSet<String> {
    static READINGS: OnceLock<HashSet<String>> = OnceLock::new();
    READINGS.get_or_init(|| {
        typo_dictionary()
            .iter()
            .map(|(reading, _)| reading.clone())
            .collect()
    })
}

fn typo_candidates(raw: &str) -> Vec<Candidate> {
    let raw = raw.to_ascii_lowercase();
    let mut matches = typo_dictionary()
        .iter()
        .filter_map(|(reading, text)| {
            let distance = damerau_levenshtein_one(&raw, reading)?;
            (distance == 1).then(|| Candidate {
                text: text.clone(),
                comment: format!("纠错 · {reading}"),
                reading_key: reading.clone(),
                source_index: None,
            })
        })
        .collect::<Vec<_>>();
    matches.sort_by(|left, right| {
        left.reading_key
            .cmp(&right.reading_key)
            .then_with(|| left.text.cmp(&right.text))
    });
    matches
        .dedup_by(|left, right| left.text == right.text && left.reading_key == right.reading_key);
    matches
}

fn damerau_levenshtein_one(left: &str, right: &str) -> Option<usize> {
    if left == right {
        return Some(0);
    }
    let a = left.as_bytes();
    let b = right.as_bytes();
    let length_delta = a.len().abs_diff(b.len());
    if length_delta > 1 {
        return None;
    }
    if a.len() == b.len() {
        let mismatches = a
            .iter()
            .zip(b)
            .enumerate()
            .filter(|(_, (x, y))| x != y)
            .collect::<Vec<_>>();
        if mismatches.len() == 1 {
            return Some(1);
        }
        if mismatches.len() == 2 {
            let first = mismatches[0].0;
            let second = mismatches[1].0;
            if second == first + 1 && a[first] == b[second] && a[second] == b[first] {
                return Some(1);
            }
        }
        return None;
    }
    let (short, long) = if a.len() < b.len() { (a, b) } else { (b, a) };
    let mut skipped = false;
    let mut short_index = 0;
    for byte in long {
        if short_index < short.len() && *byte == short[short_index] {
            short_index += 1;
        } else if skipped {
            return None;
        } else {
            skipped = true;
        }
    }
    Some(1)
}

fn adjacent_transposition(left: &str, right: &str) -> bool {
    let left = left.as_bytes();
    let right = right.as_bytes();
    if left.len() != right.len() {
        return false;
    }
    let mismatches = left
        .iter()
        .zip(right)
        .enumerate()
        .filter_map(|(index, (left, right))| (left != right).then_some(index))
        .collect::<Vec<_>>();
    mismatches.len() == 2
        && mismatches[1] == mismatches[0] + 1
        && left[mismatches[0]] == right[mismatches[1]]
        && left[mismatches[1]] == right[mismatches[0]]
}

#[cfg(test)]
mod tests {
    include!("zhixu/learning_mixer_tests.rs");
    include!("zhixu/regression.rs");
    use super::*;
    use serde::Deserialize;

    #[derive(Default)]
    struct ListDecoder {
        raw: String,
        exact_boundary: Option<crate::state::CompositionBoundary>,
        /// Text picked earlier in the composition and the letters left, as Rime shows them
        /// in the preedit while `raw` still holds the whole input.
        confirmed: String,
        remainder: String,
        candidates: Vec<Candidate>,
        highlighted: usize,
        page_no: usize,
        is_last_page: bool,
        delivery_ids: Arc<Mutex<Vec<(u64, LearningIntent)>>>,
    }

    impl ListDecoder {
        fn with(raw: &str, values: &[(&str, &str)]) -> Self {
            Self {
                raw: raw.into(),
                exact_boundary: None,
                confirmed: String::new(),
                remainder: String::new(),
                candidates: values
                    .iter()
                    .map(|(text, reading)| Candidate {
                        text: (*text).into(),
                        comment: String::new(),
                        reading_key: (*reading).into(),
                        source_index: None,
                    })
                    .collect(),
                highlighted: 0,
                page_no: 0,
                is_last_page: true,
                delivery_ids: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn with_more_pages(raw: &str, values: &[(&str, &str)]) -> Self {
            let mut decoder = Self::with(raw, values);
            decoder.is_last_page = false;
            decoder
        }
    }

    impl Decoder for ListDecoder {
        fn native_editing(&self) -> bool { self.exact_boundary.is_some() }
        fn composition_boundary(&self, _purpose: crate::state::BoundaryPurpose) -> Option<crate::state::CompositionBoundary> {
            self.exact_boundary.clone().or_else(|| crate::state::default_composition_boundary(&self.snapshot()))
        }
        fn snapshot(&self) -> DecoderSnapshot {
            let preedit = if self.confirmed.is_empty() {
                self.raw.clone()
            } else {
                format!("{}{}", self.confirmed, self.remainder)
            };
            DecoderSnapshot {
                raw_input: self.raw.clone(),
                cursor_utf8: preedit.len(),
                preedit,
                candidates: self.candidates.clone(),
                secondary_candidates: Vec::new(),
                highlighted_index: self.highlighted,
                page_no: self.page_no,
                is_last_page: self.is_last_page,
            }
        }
        fn input(&mut self, text: &str) -> Result<String, i32> {
            self.raw.push_str(text);
            Ok(String::new())
        }
        fn backspace(&mut self) -> Result<String, i32> {
            self.raw.pop();
            Ok(String::new())
        }
        fn move_left(&mut self) -> Result<String, i32> {
            Ok(String::new())
        }
        fn move_right(&mut self) -> Result<String, i32> {
            Ok(String::new())
        }
        fn page_up(&mut self) -> Result<String, i32> {
            Ok(String::new())
        }
        fn page_down(&mut self) -> Result<String, i32> {
            self.page_no += 1;
            self.is_last_page = true;
            Ok(String::new())
        }
        fn select_previous(&mut self) -> Result<String, i32> {
            self.highlighted = self.highlighted.saturating_sub(1);
            Ok(String::new())
        }
        fn select_next(&mut self) -> Result<String, i32> {
            self.highlighted = (self.highlighted + 1).min(self.candidates.len().saturating_sub(1));
            Ok(String::new())
        }
        fn select_candidate(
            &mut self,
            index: usize,
            delivery_id: u64,
            learning_intent: LearningIntent,
        ) -> Result<String, i32> {
            self.delivery_ids
                .lock()
                .unwrap()
                .push((delivery_id, learning_intent));
            let text = self.candidates.get(index).ok_or(-10)?.text.clone();
            self.clear();
            Ok(text)
        }
        fn delete_candidate(&mut self, index: usize) -> Result<(), i32> {
            if index < self.candidates.len() {
                Ok(())
            } else {
                Err(-10)
            }
        }
        fn commit_raw(&mut self) -> String {
            std::mem::take(&mut self.raw)
        }
        fn clear(&mut self) {
            self.raw.clear();
        }
    }

    struct DeepPagedDecoder {
        raw: String,
        candidates: Vec<Candidate>,
        page_no: usize,
        global_selections: Arc<Mutex<Vec<usize>>>,
    }

    impl DeepPagedDecoder {
        fn new(raw: &str, values: &[(&str, &str)]) -> (Self, Arc<Mutex<Vec<usize>>>) {
            let global_selections = Arc::new(Mutex::new(Vec::new()));
            (
                Self {
                    raw: raw.into(),
                    candidates: values
                        .iter()
                        .enumerate()
                        .map(|(index, (text, reading))| Candidate {
                            text: (*text).into(),
                            comment: String::new(),
                            reading_key: (*reading).into(),
                            source_index: Some(index),
                        })
                        .collect(),
                    page_no: 0,
                    global_selections: global_selections.clone(),
                },
                global_selections,
            )
        }

        fn finish(&mut self, text: String) -> String {
            self.raw.clear();
            self.page_no = 0;
            text
        }
    }

    impl Decoder for DeepPagedDecoder {
        fn whole_prefix_has_raw_echo(&self) -> bool {
            self.candidates.iter().any(|c| c.text.is_ascii() && c.text.eq_ignore_ascii_case(&self.raw))
        }

        fn snapshot(&self) -> DecoderSnapshot {
            let start = self.page_no * PAGE_SIZE;
            DecoderSnapshot {
                raw_input: self.raw.clone(),
                preedit: self.raw.clone(),
                cursor_utf8: self.raw.len(),
                candidates: self
                    .candidates
                    .iter()
                    .skip(start)
                    .take(PAGE_SIZE)
                    .cloned()
                    .collect(),
                secondary_candidates: Vec::new(),
                highlighted_index: 0,
                page_no: self.page_no,
                is_last_page: start + PAGE_SIZE >= self.candidates.len(),
            }
        }

        fn snapshot_all_candidates(&self) -> Option<DecoderSnapshot> {
            let mut snapshot = self.snapshot();
            snapshot.candidates = self.candidates.clone();
            snapshot.page_no = 0;
            snapshot.is_last_page = true;
            Some(snapshot)
        }

        fn input(&mut self, text: &str) -> Result<String, i32> {
            self.raw.push_str(text);
            self.page_no = 0;
            Ok(String::new())
        }
        fn backspace(&mut self) -> Result<String, i32> {
            self.raw.pop();
            self.page_no = 0;
            Ok(String::new())
        }
        fn move_left(&mut self) -> Result<String, i32> {
            Ok(String::new())
        }
        fn move_right(&mut self) -> Result<String, i32> {
            Ok(String::new())
        }
        fn page_up(&mut self) -> Result<String, i32> {
            self.page_no = self.page_no.saturating_sub(1);
            Ok(String::new())
        }
        fn page_down(&mut self) -> Result<String, i32> {
            if (self.page_no + 1) * PAGE_SIZE < self.candidates.len() {
                self.page_no += 1;
            }
            Ok(String::new())
        }
        fn select_previous(&mut self) -> Result<String, i32> {
            Ok(String::new())
        }
        fn select_next(&mut self) -> Result<String, i32> {
            Ok(String::new())
        }
        fn select_candidate(
            &mut self,
            index: usize,
            _delivery_id: u64,
            _learning_intent: LearningIntent,
        ) -> Result<String, i32> {
            let global = self.page_no * PAGE_SIZE + index;
            let text = self.candidates.get(global).ok_or(-10)?.text.clone();
            Ok(self.finish(text))
        }
        fn select_candidate_global(
            &mut self,
            index: usize,
            _delivery_id: u64,
            _learning_intent: LearningIntent,
        ) -> Result<String, i32> {
            let text = self.candidates.get(index).ok_or(-10)?.text.clone();
            self.global_selections.lock().unwrap().push(index);
            Ok(self.finish(text))
        }
        fn delete_candidate(&mut self, index: usize) -> Result<(), i32> {
            ((self.page_no * PAGE_SIZE + index) < self.candidates.len())
                .then_some(())
                .ok_or(-10)
        }
        fn delete_candidate_global(&mut self, index: usize) -> Result<(), i32> {
            (index < self.candidates.len()).then_some(()).ok_or(-10)
        }
        fn commit_raw(&mut self) -> String {
            let raw = self.raw.clone();
            self.finish(raw)
        }
        fn clear(&mut self) {
            self.raw.clear();
            self.page_no = 0;
        }
    }

    struct WholePartialDecoder(ListDecoder);

    impl Decoder for WholePartialDecoder {
        fn snapshot(&self) -> DecoderSnapshot { self.0.snapshot() }
        fn composition_boundary(&self, _purpose: BoundaryPurpose) -> Option<CompositionBoundary> {
            // A native-style partial pick: the picked text plus the letters left.
            if self.0.confirmed.is_empty() { return None; }
            let letters = self.0.remainder.chars()
                .filter(|c| c.is_ascii_alphabetic()).collect::<String>();
            (!letters.is_empty()).then(|| CompositionBoundary {
                confirmed: self.0.confirmed.clone(), unconfirmed_raw: letters,
            })
        }
        fn snapshot_all_candidates(&self) -> Option<DecoderSnapshot> { Some(self.snapshot()) }
        fn input(&mut self, text: &str) -> Result<String, i32> { self.0.input(text) }
        fn backspace(&mut self) -> Result<String, i32> { self.0.backspace() }
        fn move_left(&mut self) -> Result<String, i32> { self.0.move_left() }
        fn move_right(&mut self) -> Result<String, i32> { self.0.move_right() }
        fn page_up(&mut self) -> Result<String, i32> { self.0.page_up() }
        fn page_down(&mut self) -> Result<String, i32> { self.0.page_down() }
        fn select_previous(&mut self) -> Result<String, i32> { self.0.select_previous() }
        fn select_next(&mut self) -> Result<String, i32> { self.0.select_next() }
        fn select_candidate(&mut self, index: usize, id: u64, intent: LearningIntent) -> Result<String, i32> {
            if self.0.confirmed.is_empty() && self.0.candidates.get(index).is_some_and(|c| c.text == "一键") {
                self.0.confirmed = "一键".into();
                self.0.remainder = "le".into();
                self.0.candidates = ListDecoder::with("le", &[("了", "le")]).candidates;
                return Ok(String::new());
            }
            let prefix = self.0.confirmed.clone();
            self.0.select_candidate(index, id, intent).map(|text| format!("{prefix}{text}"))
        }
        fn select_candidate_global(&mut self, index: usize, id: u64, intent: LearningIntent) -> Result<String, i32> {
            self.select_candidate(index, id, intent)
        }
        fn delete_candidate(&mut self, index: usize) -> Result<(), i32> { self.0.delete_candidate(index) }
        fn commit_raw(&mut self) -> String { self.0.commit_raw() }
        fn clear(&mut self) { self.0.clear(); }
    }

    #[test]
    fn whole_shadow_preserves_native_prefix_and_segment_selection() {
        zhixu::with_index(whole_fixture(), || {
            let decoder = WholePartialDecoder(ListDecoder::with("yijianle", &[("一键", "yi jian"), ("一", "yi")]));
            let mut mixer = MixerDecoder::new(Box::new(decoder), "whole-segments".into());
            mixer.whole_override = Some(true);
            let all = mixer.snapshot_all_candidates().unwrap().candidates;
            let index = all.iter().position(|c| c.text == "一键").unwrap();
            assert_eq!(all[index].source_index, Some(0));
            for _ in 0..index / PAGE_SIZE { mixer.page_down().unwrap(); }
            assert_eq!(mixer.select_candidate(index % PAGE_SIZE, 704, LearningIntent::NoLearn).unwrap(), "");
            let after = mixer.snapshot();
            assert_eq!(after.preedit, "一键le");
            assert_eq!(after.raw_input, "le");
            assert!(mixer.snapshot_all_candidates().is_none(), "confirmed segments keep the native path");
            mixer.whole_override = Some(false);
            assert_eq!(mixer.snapshot().candidates, after.candidates);
            assert_eq!(mixer.select_candidate(0, 705, LearningIntent::NoLearn).unwrap(), "一键了");
        });
    }

    #[test]
    fn whole_shadow_preserves_migrated_learning_scores() {
        let directory = migration_fixture("#@/tick\t1000\nyi jian \t意见\tc=37 d=0.1 t=10\nyi jian \t一键\tc=7 d=7 t=1000\nyi jian \t乙键\tc=1 d=0.01 t=10\n");
        zhixu::with_index(whole_fixture(), || {
            let mut mixer = whole_mixer("yijian", &[("意见", "yi jian")], false);
            mixer.learning.attach(&directory);
            mixer.configure_learning(LearningMode::Zhixu).unwrap();
            assert!(mixer.learning.use_rime_order("yijian"));
            assert_eq!(mixer.snapshot().candidates[0].text, "意见");
            mixer.whole_override = Some(true);
            assert_eq!(mixer.snapshot().candidates[0].text, "一键");
            assert!(mixer.snapshot_all_candidates().unwrap().candidates.iter().any(|c| c.text == "乙键"));
            assert!(mixer.learning.use_rime_order("yijian"), "source replacement does not change learning state");
            let all = mixer.snapshot_all_candidates().unwrap().candidates;
            let position = all.iter().position(|c| c.text == "乙键").unwrap();
            for _ in 0..position / PAGE_SIZE { mixer.page_down().unwrap(); }
            assert_eq!(mixer.select_candidate(position % PAGE_SIZE, 707, LearningIntent::DeliberateSelection).unwrap(), "乙键");
            let now = personal_picks::now_ms();
            mixer.learning_commit("乙键", 707, now);
            mixer.learning.key(&crate::state::KeyEvent { kind: crate::state::KeyKind::Text, text: "a".into(), modifiers: 0,
                security: crate::state::SecurityState::NonSecure, delivery_id: 708 }, false, now + 6000);
            assert!(mixer.learning.entries().iter().any(|e| e.text == "乙键" && e.reading == "yi jian"
                && e.rime.as_ref().is_some_and(|weight| weight.updates == 1)));
        });
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn whole_shadow_imported_reading_does_not_learn_a_partial_pronunciation() {
        let directory = migration_fixture("#@/tick\t1000\ngao lan \t高了\tc=7 d=7 t=1000\n");
        zhixu::with_index(whole_fixture(), || {
            let mut mixer = whole_mixer("gaolan", &[("高", "gao")], true);
            mixer.learning.attach(&directory);
            mixer.configure_learning(LearningMode::Zhixu).unwrap();
            let all = mixer.snapshot_all_candidates().unwrap().candidates;
            let position = all.iter().position(|c| c.text == "高了").unwrap();
            for _ in 0..position / PAGE_SIZE { mixer.page_down().unwrap(); }
            assert_eq!(mixer.select_candidate(position % PAGE_SIZE, 709, LearningIntent::DeliberateSelection).unwrap(), "高了");
            let now = personal_picks::now_ms();
            mixer.learning_commit("高了", 709, now);
            mixer.learning.key(&crate::state::KeyEvent { kind: crate::state::KeyKind::Text, text: "a".into(), modifiers: 0,
                security: crate::state::SecurityState::NonSecure, delivery_id: 710 }, false, now + 6000);
            let entries = mixer.learning.entries();
            assert!(entries.iter().all(|entry| entry.reading != "gao le"));
            assert!(entries.iter().any(|entry| entry.text == "高了" && entry.reading == "gao lan"
                && entry.rime.as_ref().is_some_and(|weight| weight.updates == 1)));
        });
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn whole_shadow_learning_promotions_and_context_reordering_remain_available() {
        zhixu::with_index(whole_fixture(), || {
            let mut mixer = whole_mixer("yijian", &[("意见", "yi jian")], true);
            mixer.configure_learning(LearningMode::Zhixu).unwrap();
            let before = mixer.snapshot_all_candidates().unwrap().candidates;
            let position = before.iter().position(|c| c.text == "一键").unwrap();
            mixer.learning.remember_for_test("yi jian", "一键", personal_picks::now_ms());
            let learned = mixer.snapshot_all_candidates().unwrap().candidates;
            assert_eq!(learned.iter().position(|c| c.text == "一键"), Some(position.saturating_sub(MAX_SELECTION_PROMOTION)));
            let mut order: Vec<_> = (0..PAGE_SIZE).collect();
            order.swap(0, 1);
            mixer.reorder_context_candidates(&order).unwrap();
            assert_eq!(mixer.snapshot().candidates[0], learned[1]);
            assert_eq!(&mixer.snapshot_all_candidates().unwrap().candidates[..PAGE_SIZE], &mixer.snapshot().candidates);
        });
    }

    fn whole_fixture() -> Arc<Index> {
        use crate::zhixu::loader::{DictionaryData, Entry};
        Arc::new(Index::from_data(DictionaryData { entries: [
            ("意见", "yi jian", 1000), ("一件", "yi jian", 990),
            ("一见", "yi jian", 980), ("已建", "yi jian", 970),
            ("易见", "yi jian", 960), ("一键", "yi jian", 950),
            ("异见", "yi jian", 940), ("以剑", "yi jian", 930),
            ("义剑", "yi jian", 920), ("衣件", "yi jian", 910),
            ("一箭", "yi jian", 900), ("一剪", "yi jian", 890),
            ("高", "gao", 1000), ("搞", "gao", 900), ("了", "le", 1000),
            ("一", "yi", 1000), ("见", "jian", 1000),
        ].into_iter().map(|(text, reading, weight)| Entry {
            text: text.into(), reading: reading.into(), weight, layer: "fixture".into(),
        }).collect() }))
    }

    fn whole_mixer(raw: &str, values: &[(&str, &str)], enabled: bool) -> MixerDecoder {
        let mut mixer = MixerDecoder::new(Box::new(ListDecoder::with(raw, values)), "whole-fixture".into());
        mixer.whole_override = Some(enabled);
        mixer
    }

    #[test]
    fn q9_public_jqx_repairs_reach_first_page() {
        let dictionary = zhixu::fallback();
        zhixu::with_index(dictionary.clone(), || {
            let mut missing = Vec::new();
            for (raw, expected) in [
                ("qanmian", "前面"), ("xanzai", "现在"), ("jandan", "简单"),
                ("xang", "想"), ("haoxangshi", "好像是"),
            ] {
                let decoder = crate::zhixu::decoder::ZhixuDecoder::from_ready_dictionary(dictionary.clone());
                let mut mixer = MixerDecoder::new(Box::new(decoder), format!("q9-{raw}"));
                mixer.configure_zhixu_whole(true).unwrap();
                mixer.input(raw).unwrap();
                let page = mixer.snapshot();
                println!("Q9_PAGE\t{raw}\t{:?}", page.candidates.iter().map(|c| c.text.as_str()).collect::<Vec<_>>());
                if !page.candidates.iter().any(|candidate| candidate.text == expected) {
                    missing.push((raw, expected));
                }
                if let Some(position) = page.candidates.iter().position(|candidate| candidate.text == expected) {
                    assert_eq!(mixer.select_candidate(position, 909, LearningIntent::NoLearn).unwrap(), expected);
                    assert!(mixer.snapshot().raw_input.is_empty(), "{raw}: repaired selection consumes all keys");
                }
            }
            assert!(missing.is_empty(), "missing first-page repairs: {missing:?}");
        });
    }

    #[test]
    fn q9_legal_public_candidates_match_every_baseline_row() {
        let dictionary = zhixu::fallback();
        zhixu::with_index(dictionary.clone(), || {
            // Captured before the Q9 repair: all pages and all Candidate fields.
            for (raw, groups) in [
                ("qian", vec![
                    ("钱 前 千 签 欠 浅 牵 迁 潜 倩 铅 谦 乾", "", "qian"),
                    ("奇案", "", "qi an"),
                    ("其 起 期 气 器 七 齐 奇 骑 妻 启 旗 企 棋 漆 弃 琪 琦 岂", "", "qi"),
                    ("其按 其俺 起按 其安 起俺 起安", ZHIXU_WHOLE_TAIL_COMMENT, "qian"),
                ]),
                ("xia", vec![
                    ("下 夏 吓 瞎 虾 霞 侠", "", "xia"),
                    ("系 西 洗 戏 喜 係 细 吸 希 溪 锡 息 兮 熙 席 惜 稀 习 夕", "", "xi"),
                    ("系啊 西啊 洗啊 系阿 戏啊 喜啊", ZHIXU_WHOLE_TAIL_COMMENT, "xia"),
                ]),
                ("jiao", vec![
                    ("叫 交 教 较 脚 角 胶 焦 娇 觉 浇 搅 缴 嚼", "", "jiao"),
                    ("家 加 假 价 架 甲 佳 夹 嘉 嫁 贾 驾", "", "jia"),
                    ("及 级 即 几 机 既 急 集 记 极 鸡 寄 基 继 吉 计 挤 季 剂 暨 纪 己 积 籍 技 击 姬 济 肌 忌 祭 疾 激 迹 辑", "", "ji"),
                    ("家哦 加哦 及奥 家噢 加噢 及熬 假哦 家喔 级奥 即奥 几奥 机奥 及啊哦 及啊噢 级啊哦 即啊哦 几啊哦 及阿哦", ZHIXU_WHOLE_TAIL_COMMENT, "jiao"),
                ]),
            ] {
                let decoder = crate::zhixu::decoder::ZhixuDecoder::from_ready_dictionary(dictionary.clone());
                let mut mixer = MixerDecoder::new(Box::new(decoder), format!("q9-legal-{raw}"));
                mixer.configure_zhixu_whole(true).unwrap();
                mixer.input(raw).unwrap();
                let mut candidates = Vec::new();
                loop {
                    let page = mixer.snapshot();
                    candidates.extend(page.candidates);
                    if page.is_last_page { break; }
                    mixer.page_down().unwrap();
                }
                let expected: Vec<_> = groups.into_iter().flat_map(|(texts, comment, reading)|
                    texts.split_whitespace().map(move |text| Candidate { text: text.into(), comment: comment.into(),
                        reading_key: reading.into(), source_index: None })).collect();
                assert_eq!(candidates, expected, "{raw}: every baseline row stays unchanged");
            }
        });
    }

    #[test]
    fn q9_jqx_sentence_paths_keep_incremental_and_fresh_results_equal() {
        use crate::zhixu::loader::{DictionaryData, Entry};
        // No multi-character dictionary entries: every result must be assembled.
        let dictionary = Arc::new(Index::from_data(DictionaryData { entries: [
            ("前", "qian", 1000), ("全", "quan", 900), ("面", "mian", 1000),
            ("现", "xian", 1000), ("在", "zai", 1000), ("简", "jian", 1000),
            ("单", "dan", 1000), ("好", "hao", 1000), ("想", "xiang", 1000),
            ("是", "shi", 1000), ("接", "jie", 1000), ("家", "jia", 1000),
        ].into_iter().map(|(text, reading, weight)| Entry {
            text: text.into(), reading: reading.into(), weight, layer: "fixture".into(),
        }).collect() }));
        zhixu::with_index(dictionary, || {
            for (keys, expected) in [("qanmian", "前面"), ("xanzai", "现在"), ("jandan", "简单"),
                ("haoxangshi", "好想是"), ("jei", "接"), ("jai", "家"), ("ixang", "想"),
                ("qanhaoxang", "前好想")] {
                let mut cache = None;
                for end in 1..=keys.len() {
                    let raw = &keys[..end];
                    if whole_syllable_count(raw).is_none() { continue; }
                    let incremental = decode_whole_lattice(raw, 120, 6, 96, &[], 0, Some((&mut cache, 0)), 0);
                    let fresh = decode_whole_lattice(raw, 120, 6, 96, &[], 0, None, 0);
                    assert_eq!(incremental, fresh, "{keys}: {raw}");
                    if end == keys.len() {
                        assert_eq!(fresh.first().map(|(text, _)| text.as_str()), Some(expected), "{keys}");
                    }
                }
            }
            for raw in ["q'an", "x'ang", "j'ai"] {
                assert!(decode_whole_lattice(raw, 120, 6, 96, &[], 0, None, 0).is_empty(), "{raw}");
            }
            let mut exact = None;
            let mut repaired = None;
            decode_whole_lattice("qian", 120, 6, 96, &[], 0, Some((&mut exact, 0)), 0);
            decode_whole_lattice("qan", 120, 6, 96, &[], 0, Some((&mut repaired, 0)), 0);
            let score = |state: WholeLattice| state.paths.last().unwrap().iter()
                .find(|path| path.1 == "前").unwrap().0;
            assert_eq!(score(exact.unwrap()) - score(repaired.unwrap()), zhixu::fuzzy::JQX_REPAIR_COST);
            let _fuzzy = zhixu::fuzzy::pin_mask(1 << 5);
            assert_eq!(decode_whole_lattice("qanmian", 120, 6, 96, &[], 0, None, 0)[0].0, "前面");
        });
    }

    #[test]
    fn native_editing_preserves_column_through_whole_and_deep_pages() {
        use crate::zhixu::loader::{DictionaryData, Entry};
        let entries = ["意见", "一件", "一见", "已建", "易见", "一键", "异见", "以剑", "义剑", "衣件", "一箭", "一剪"]
            .into_iter().enumerate().map(|(at, text)| Entry {
                text: text.into(), reading: "yi jian".into(), weight: 1000 - at as u32, layer: "fixture".into(),
            }).chain("一乙以已衣医伊依宜易亦义亿益异艺议译移仪疑遗".chars().enumerate().map(|(at, text)| Entry {
                text: text.to_string(), reading: "yi".into(), weight: 500 - at as u32, layer: "fixture".into(),
            })).collect();
        zhixu::with_index(Arc::new(Index::from_data(DictionaryData { entries })), || {
            for whole in [false, true] {
                let mut decoder = crate::zhixu::decoder::ZhixuDecoder::new();
                decoder.input("yijianyi").unwrap();
                let mut mixer = MixerDecoder::new(Box::new(decoder), format!("native-page-column-{whole}"));
                mixer.whole_override = Some(whole);
                for _ in 0..3 { mixer.select_next().unwrap(); }
                mixer.page_up().unwrap();
                assert_eq!(mixer.snapshot().highlighted_index, 0, "first-page PageUp resets the column");
                for _ in 0..3 { mixer.select_next().unwrap(); }
                let first = mixer.snapshot();
                assert_eq!(first.highlighted_index, 3);
                mixer.page_down().unwrap();
                assert_eq!(mixer.snapshot().highlighted_index, 3, "whole={whole}");
                let second = mixer.snapshot();
                mixer.page_down().unwrap();
                let third = mixer.snapshot();
                if !whole { assert!(mixer.deep_page_no.is_some(), "exercise the native-to-deep transition"); }
                assert_eq!(third.highlighted_index, 3.min(third.candidates.len() - 1), "whole={whole}");
                let column = third.highlighted_index;
                mixer.page_up().unwrap();
                assert_eq!(mixer.snapshot().highlighted_index, column, "whole={whole}");
                assert_eq!(mixer.snapshot().candidates, second.candidates);
                mixer.page_up().unwrap();
                assert_eq!(mixer.snapshot().highlighted_index, column, "whole={whole}");
                assert_eq!(mixer.snapshot().candidates, first.candidates);
                let before_peek = mixer.snapshot();
                let _ = mixer.peek_following_pages(3);
                assert_whole_snapshot(&before_peek, &mixer.snapshot());
                for _ in column..PAGE_SIZE { mixer.select_next_linear().unwrap(); }
                assert_eq!((mixer.snapshot().page_no, mixer.snapshot().highlighted_index), (1, 0));
                mixer.select_previous_linear().unwrap();
                assert_eq!((mixer.snapshot().page_no, mixer.snapshot().highlighted_index), (0, 4));
                mixer.page_down().unwrap();
                mixer.page_down().unwrap();
                for _ in 0..10 {
                    if mixer.inner.snapshot().cursor_utf8 == 0 { break; }
                    mixer.move_left().unwrap();
                }
                assert_eq!(mixer.inner.snapshot().cursor_utf8, 0);
                let at_start = mixer.snapshot();
                mixer.page_down().unwrap();
                assert_whole_snapshot(&at_start, &mixer.snapshot());
                mixer.page_up().unwrap();
                mixer.select_next().unwrap();
                mixer.select_previous().unwrap();
                mixer.select_next_linear().unwrap();
                mixer.select_previous_linear().unwrap();
                assert_whole_snapshot(&at_start, &mixer.snapshot());
            }
        });
    }

    pub(super) fn assert_whole_snapshot(a: &DecoderSnapshot, b: &DecoderSnapshot) {
        assert_eq!((&a.raw_input, &a.preedit, a.cursor_utf8, &a.candidates, &a.secondary_candidates,
            a.page_no, a.highlighted_index, a.is_last_page),
            (&b.raw_input, &b.preedit, b.cursor_utf8, &b.candidates, &b.secondary_candidates,
            b.page_no, b.highlighted_index, b.is_last_page));
    }

    #[test]
    fn whole_generated_tail_waits_for_native_eof_and_full_query_completes_it() {
        if !whole_test_child("whole_generated_tail_waits_for_native_eof_and_full_query_completes_it") { return; }
        zhixu::with_index(whole_fixture(), || {
            let strings: Vec<_> = (0..60).map(|i| char::from_u32(0x4e00 + i).unwrap().to_string()).collect();
            let values: Vec<_> = strings.iter().map(|text| (text.as_str(), "yi")).collect();
            let (decoder, _) = DeepPagedDecoder::new("yijian", &values);
            let mut lazy = MixerDecoder::new(Box::new(decoder), "whole-tail-lazy".into());
            lazy.configure_learning(LearningMode::Zhixu).unwrap();
            lazy.whole_override = Some(true);
            let first = lazy.snapshot();
            assert!(!lazy.whole_cache.lock().unwrap().as_ref().unwrap().generated_complete,
                "the first two pages of whole words must not enumerate their lattice tail");
            let (decoder, _) = DeepPagedDecoder::new("yijian", &values);
            let mut eager = MixerDecoder::new(Box::new(decoder), "whole-tail-lazy".into());
            eager.configure_learning(LearningMode::Zhixu).unwrap();
            eager.whole_override = Some(true);
            eager.whole_eager = true;
            assert_whole_snapshot(&first, &eager.snapshot());
            whole_lazy_assert_navigation(&mut lazy, &mut eager);
            let all = lazy.snapshot_all_candidates().unwrap();
            assert!(lazy.whole_cache.lock().unwrap().as_ref().unwrap().generated_complete);
            assert_whole_snapshot(&all, &eager.snapshot_all_candidates().unwrap());
            let (decoder, _) = DeepPagedDecoder::new("yijian", &values);
            let mut full_query = MixerDecoder::new(Box::new(decoder), "whole-tail-lazy".into());
            full_query.configure_learning(LearningMode::Zhixu).unwrap();
            full_query.whole_override = Some(true);
            assert_whole_snapshot(&full_query.snapshot_all_candidates().unwrap(), &eager.snapshot_all_candidates().unwrap());
            assert!(full_query.whole_cache.lock().unwrap().as_ref().unwrap().generated_complete);
        });
    }

    #[test]
    fn whole_long_word_tail_requires_positive_unfiltered_promotion_evidence() {
        if !whole_test_child("whole_long_word_tail_requires_positive_unfiltered_promotion_evidence") { return; }
        use crate::zhixu::loader::{DictionaryData, Entry};
        let index = Arc::new(Index::from_data(DictionaryData { entries: [
            ("多线程", "duo xian cheng", 1000), ("多", "duo", 900), ("夺", "duo", 800),
            ("线程", "xian cheng", 900), ("闲程", "xian cheng", 800),
        ].into_iter().map(|(text, reading, weight)| Entry {
            text: text.into(), reading: reading.into(), weight, layer: "fixture".into(),
        }).collect() }));
        zhixu::with_index(index, || {
            let strings: Vec<_> = (0..60).map(|i| char::from_u32(0x4e00 + i).unwrap().to_string()).collect();
            let values: Vec<_> = strings.iter().map(|text| (text.as_str(), "yi")).collect();
            for blocked in [false, true] {
                let make = |eager| {
                    let (decoder, _) = DeepPagedDecoder::new("duoxiancheng", &values);
                    let mut mixer = MixerDecoder::new(Box::new(decoder), "whole-long-prefix".into());
                    mixer.configure_learning(LearningMode::Zhixu).unwrap();
                    mixer.whole_override = Some(true);
                    mixer.whole_eager = eager;
                    if blocked { mixer.blacklist.lock().unwrap().insert("duoxiancheng", "多线程").unwrap(); }
                    mixer
                };
                let mut lazy = make(false);
                let mut eager = make(true);
                WHOLE_TIMING.with(|timing| *timing.borrow_mut() = [(0, 0); 6]);
                let first = lazy.snapshot();
                WHOLE_TIMING.with(|timing| {
                    let timing = timing.borrow();
                    assert!(timing[2].0 <= 2 && timing[3].0 <= 2,
                        "a failed tail certificate must complete native and generated together: {timing:?}");
                });
                assert_whole_snapshot(&first, &eager.snapshot());
                {
                    let cache = lazy.whole_cache.lock().unwrap();
                    let cache = cache.as_ref().unwrap();
                    if blocked {
                        assert!(cache.generated_complete, "a hidden exact word cannot certify an omitted tail");
                        assert!(cache.base.as_ref().unwrap().is_last_page, "failed certificate must fetch every native");
                    } else {
                        assert!(!cache.generated_complete);
                        assert!(cache.generated_certified);
                        assert!(cache.base.as_ref().unwrap().is_last_page, "long native inputs retain the Z9 full-fetch boundary");
                    }
                }
                whole_lazy_assert_navigation(&mut lazy, &mut eager);
            }
        });
    }

    #[test]
    fn whole_extra_readings_do_not_allow_an_unknown_exact_negative_certificate() {
        if !whole_test_child("whole_extra_readings_do_not_allow_an_unknown_exact_negative_certificate") { return; }
        use crate::zhixu::loader::{DictionaryData, Entry};
        let raw = "yiyijieshao";
        assert!(!chinese_exact_readings().contains(raw));
        assert!(dictionary_readings("一一介绍").unwrap().iter().any(|(key, _)| key == raw));
        let index = Arc::new(Index::from_data(DictionaryData { entries: [
            ("一", "yi"), ("介绍", "jie shao"),
        ].into_iter().map(|(text, reading)| Entry {
            text: text.into(), reading: reading.into(), weight: 1000, layer: "fixture".into(),
        }).collect() }));
        zhixu::with_index(index, || {
            let values = [("一", "yi"), ("已", "yi"), ("以", "yi")];
            let mut lazy = whole_mixer(raw, &values, true);
            lazy.configure_learning(LearningMode::Zhixu).unwrap();
            let mut eager = whole_mixer(raw, &values, true);
            eager.configure_learning(LearningMode::Zhixu).unwrap();
            eager.whole_eager = true;
            assert_whole_snapshot(&lazy.snapshot(), &eager.snapshot());
            assert!(lazy.whole_cache.lock().unwrap().as_ref().unwrap().generated_complete,
                "a reading absent only from the primary map does not prove that omitted tails lack an exact hit");
            assert_whole_snapshot(&lazy.snapshot_all_candidates().unwrap(), &eager.snapshot_all_candidates().unwrap());
        });
    }

    #[test]
    fn whole_order_keeps_ordered_allocation_and_matches_stable_partition() {
        let item = |kind: usize, index: usize| MixedCandidate {
            candidate: Candidate { text: index.to_string(), comment: match kind {
                1 => ZHIXU_WHOLE_COMMENT, 2 => ZHIXU_WHOLE_SENTENCE_COMMENT,
                4 => ZHIXU_WHOLE_TAIL_COMMENT, _ => "native",
            }.into(), reading_key: "yijian".into(), source_index: Some(index) },
            selection: if kind == 0 { Selection::Local { text: index.to_string() } }
                else { Selection::GlobalBase { text: index.to_string(), index } }, tier: 2, score: 0,
        };
        let mut ordered = Vec::with_capacity(1024);
        ordered.extend((0..5).map(|kind| item(kind, kind)));
        let capacity = ordered.capacity();
        order_whole_candidates(&mut ordered);
        assert_eq!(ordered.capacity(), capacity, "an already ordered whole sequence keeps its allocation");
        for bits in 0..625 {
            let values: Vec<_> = (0..4).map(|slot| item((bits / 5usize.pow(slot as u32)) % 5, slot)).collect();
            let mut buckets: [Vec<_>; 5] = std::array::from_fn(|_| Vec::new());
            for value in values.iter().cloned() {
                let bucket = match value.candidate.comment.as_str() {
                    ZHIXU_WHOLE_COMMENT => 1, ZHIXU_WHOLE_SENTENCE_COMMENT => 2,
                    ZHIXU_WHOLE_TAIL_COMMENT => 4,
                    _ if buckets[1..].iter().all(Vec::is_empty) && matches!(value.selection, Selection::Local { .. }) => 0,
                    _ => 3,
                };
                buckets[bucket].push(value);
            }
            let expected: Vec<_> = buckets.into_iter().flatten().collect();
            let mut actual = values;
            order_whole_candidates(&mut actual);
            assert_eq!(format!("{actual:?}"), format!("{expected:?}"), "category combination {bits}");
        }
    }

    #[test]
    fn whole_borrowed_keys_match_legacy_normalization() {
        for text in ["意见", " ＡＢＣ ", "a\u{301}", "\u{212b}", "\u{f900}", " ", "①"] {
            for reading in ["yijian", " Yi Jian ", "yi'jian", "yi\u{2003}jian", "", "yi1", "é"] {
                let candidate = Candidate { text: text.into(), reading_key: reading.into(), ..Candidate::default() };
                let expected = candidate_key(&candidate, "fallback");
                let actual = whole_candidate_key(&candidate, "fallback");
                assert_eq!((actual.0.as_ref(), actual.1.as_ref()), (expected.0.as_str(), expected.1.as_str()));
            }
        }
        let candidate = Candidate { text: "意见".into(), reading_key: "yijian".into(), ..Candidate::default() };
        let key = whole_candidate_key(&candidate, "fallback");
        assert!(matches!(key.0, Cow::Borrowed(_)), "normalized Han text needs no owned key");
        assert!(matches!(key.1, Cow::Borrowed(_)), "canonical reading needs no owned key");
    }

    #[test]
    fn whole_borrowed_dedup_and_blacklist_match_legacy_fields() {
        let mut blacklist = Store::default();
        blacklist.insert("yijian", "黑名单").unwrap();
        let input: Vec<_> = [("Ａ", " Yi Jian "), ("A", "yijian"), ("A", "gaole"),
            ("黑名单", "yijian"), ("意见", ""), ("意见", "yijian"), ("①", "yijian"), ("1", "yijian")]
            .into_iter().enumerate().map(|(index, (text, reading))| MixedCandidate {
                candidate: Candidate { text: text.into(), reading_key: reading.into(), comment: index.to_string(), source_index: Some(index) },
                selection: Selection::GlobalBase { text: text.into(), index }, tier: 2, score: -(index as i64),
            }).collect();
        let mut expected = input.clone();
        let mut seen = HashSet::new();
        expected.retain(|item| {
            let key = candidate_key(&item.candidate, "yijian");
            !blacklist.contains(&key.1, &item.candidate.text) && seen.insert(key)
        });
        let mut actual = input;
        retain_unique_whole(&mut actual, "yijian", Some(&blacklist));
        assert_eq!(format!("{actual:?}"), format!("{expected:?}"));
        retain_unique_whole(&mut actual, "yijian", None);
        assert!(actual.is_empty(), "a poisoned blacklist retains the existing fail-closed behavior");
    }

    #[test]
    fn whole_short_reading_memo_matches_legacy_with_repeated_keys() {
        let candidate = |reading: &str| MixedCandidate {
            candidate: Candidate { text: "候选".into(), reading_key: reading.into(), ..Candidate::default() },
            selection: Selection::Local { text: "候选".into() }, tier: 2, score: 0,
        };
        for raw in ["yijian", "duoxiancheng", "yij", "gaole"] {
            let values: Vec<_> = ["duo xian cheng", " Yi Jian ", "gao le", "yi'jian", "yi", ""]
                .into_iter().cycle().take(500).map(candidate).collect();
            let expected = values.iter().any(|item| {
                canonical_reading(&item.candidate.reading_key).is_some_and(|key| {
                    key.starts_with(raw) || syllable_prefix_abbreviation(raw, &item.candidate.reading_key)
                        || adjacent_transposition(raw, &key)
                }) && pinyin_segmentation(&item.candidate.reading_key).is_some_and(|parts| parts.len() < LONG_PHRASE_MIN_SYLLABLES)
            });
            assert_eq!(whole_has_complete_short_reading(raw, &values), expected, "{raw}");
        }
    }

    #[test]
    fn whole_lazy_prefix_expands_pages_and_preserves_native_selection() {
        zhixu::with_index(whole_fixture(), || {
            let strings: Vec<_> = (0..60).map(|i| char::from_u32(0x4e00 + i).unwrap().to_string()).collect();
            let values: Vec<_> = strings.iter().map(|s| (s.as_str(), "yi")).collect();
            let (decoder, _) = DeepPagedDecoder::new("yijian", &values);
            let mut lazy = MixerDecoder::new(Box::new(decoder), "whole-lazy".into());
            lazy.configure_learning(LearningMode::Zhixu).unwrap();
            lazy.whole_override = Some(true);
            let first = lazy.snapshot();
            assert!(!lazy.whole_cache.lock().unwrap().as_ref().unwrap().base.as_ref().unwrap().is_last_page,
                "first page must retain an expandable native prefix");
            assert_eq!(lazy.whole_cache.lock().unwrap().as_ref().unwrap().base.as_ref().unwrap().candidates.len(), 2 * PAGE_SIZE);
            let (decoder, _) = DeepPagedDecoder::new("yijian", &values);
            let mut eager = MixerDecoder::new(Box::new(decoder), "whole-lazy".into());
            eager.configure_learning(LearningMode::Zhixu).unwrap();
            eager.whole_override = Some(true);
            eager.whole_eager = true;
            loop {
                let a = lazy.snapshot();
                let b = eager.snapshot();
                assert_whole_snapshot(&a, &b);
                if a.is_last_page { break; }
                lazy.page_down().unwrap();
                eager.page_down().unwrap();
            }
            while lazy.whole_page_no > 0 {
                lazy.page_up().unwrap(); eager.page_up().unwrap();
                assert_whole_snapshot(&lazy.snapshot(), &eager.snapshot());
            }
            assert_whole_snapshot(&lazy.snapshot(), &first);
        });
    }

    fn whole_lazy_assert_navigation(lazy: &mut MixerDecoder, eager: &mut MixerDecoder) {
        let first = lazy.snapshot();
        loop {
            let page = lazy.snapshot();
            assert_whole_snapshot(&page, &eager.snapshot());
            if page.is_last_page { break; }
            lazy.page_down().unwrap(); eager.page_down().unwrap();
        }
        while lazy.snapshot().page_no > 0 {
            lazy.page_up().unwrap(); eager.page_up().unwrap();
            assert_whole_snapshot(&lazy.snapshot(), &eager.snapshot());
        }
        assert_whole_snapshot(&lazy.snapshot(), &first);
        let mut moves = 0;
        loop {
            let page = lazy.snapshot();
            assert_whole_snapshot(&page, &eager.snapshot());
            if page.is_last_page && page.highlighted_index + 1 == page.candidates.len() { break; }
            lazy.select_next_linear().unwrap(); eager.select_next_linear().unwrap();
            moves += 1;
        }
        assert!(moves > 2 * PAGE_SIZE, "linear movement must cross fetched and page boundaries");
        for _ in 0..moves {
            lazy.select_previous_linear().unwrap(); eager.select_previous_linear().unwrap();
            assert_whole_snapshot(&lazy.snapshot(), &eager.snapshot());
        }
        assert_whole_snapshot(&lazy.snapshot(), &first);
    }

    #[test]
    fn whole_lazy_linear_expands_unfetched_native_and_selects_global() {
        zhixu::with_index(whole_fixture(), || {
            let strings: Vec<_> = (0..80).map(|i| char::from_u32(0x4e80 + i).unwrap().to_string()).collect();
            let values: Vec<_> = strings.iter().map(|s| (s.as_str(), "yi")).collect();
            let make = |eager| {
                let (decoder, selections) = DeepPagedDecoder::new("yijian", &values);
                let mut mixer = MixerDecoder::new(Box::new(decoder), "whole-lazy-linear".into());
                mixer.configure_learning(LearningMode::Zhixu).unwrap();
                mixer.whole_override = Some(true);
                mixer.whole_eager = eager;
                (mixer, selections)
            };
            let (mut lazy, lazy_selections) = make(false);
            let (mut eager, eager_selections) = make(true);
            lazy.snapshot();
            let initial = lazy.whole_cache.lock().unwrap().as_ref().unwrap().base.as_ref().unwrap().candidates.len();
            assert_eq!(initial, 2 * PAGE_SIZE);
            for step in 0..200 {
                let page = lazy.snapshot();
                assert_whole_snapshot(&page, &eager.snapshot());
                if page.candidates[page.highlighted_index].source_index == Some(27) { break; }
                assert!(step < 199, "the deep native target must remain reachable");
                lazy.select_next_linear().unwrap(); eager.select_next_linear().unwrap();
            }
            let page = lazy.snapshot();
            assert_eq!(page.candidates[page.highlighted_index].text, strings[27]);
            let fetched = lazy.whole_cache.lock().unwrap().as_ref().unwrap().base.as_ref().unwrap().candidates.len();
            assert!(fetched > initial && fetched < values.len(), "linear movement expands only the needed native prefix");
            for _ in 0..2 * PAGE_SIZE {
                lazy.select_previous_linear().unwrap(); eager.select_previous_linear().unwrap();
                assert_whole_snapshot(&lazy.snapshot(), &eager.snapshot());
            }
            for _ in 0..2 * PAGE_SIZE {
                lazy.select_next_linear().unwrap(); eager.select_next_linear().unwrap();
                assert_whole_snapshot(&lazy.snapshot(), &eager.snapshot());
            }
            let index = lazy.snapshot().highlighted_index;
            assert_eq!(lazy.select_candidate(index, 900, LearningIntent::NoLearn).unwrap(), strings[27]);
            assert_eq!(eager.select_candidate(index, 900, LearningIntent::NoLearn).unwrap(), strings[27]);
            assert_eq!(*lazy_selections.lock().unwrap(), vec![27]);
            assert_eq!(*eager_selections.lock().unwrap(), vec![27]);
        });
    }

    #[test]
    fn whole_lazy_deep_raw_echo_keeps_the_full_native_result() {
        zhixu::with_index(whole_fixture(), || {
            let mut strings: Vec<_> = (0..40).map(|i| char::from_u32(0x4e80 + i).unwrap().to_string()).collect();
            strings[27] = "YIJIAN".into();
            let values: Vec<_> = strings.iter().map(|s| (s.as_str(), "yi")).collect();
            let make = |eager| {
                let (decoder, _) = DeepPagedDecoder::new("yijian", &values);
                let mut mixer = MixerDecoder::new(Box::new(decoder), "whole-lazy-raw-echo".into());
                mixer.configure_learning(LearningMode::Zhixu).unwrap();
                mixer.whole_override = Some(true);
                mixer.whole_eager = eager;
                mixer
            };
            let mut lazy = make(false);
            let mut eager = make(true);
            let first = lazy.snapshot();
            assert!(lazy.whole_cache.lock().unwrap().as_ref().unwrap().base.as_ref().unwrap().is_last_page);
            assert!(lazy.whole_cache.lock().unwrap().as_ref().unwrap().generated.is_none(),
                "a deep case-insensitive raw echo disables whole generation too");
            assert_eq!(first.candidates, eager.snapshot().candidates);
            whole_lazy_assert_navigation(&mut lazy, &mut eager);
            let all = lazy.snapshot_all_candidates().unwrap();
            assert!(all.candidates.iter().all(|candidate| !whole_candidate(candidate)));
            assert!(all.candidates.iter().any(|candidate| candidate.text == "YIJIAN"));
        });
    }

    #[test]
    fn whole_lazy_deep_learning_promotion_keeps_native_deduplication() {
        zhixu::with_index(whole_fixture(), || {
            let mut strings: Vec<_> = (0..45).map(|i| char::from_u32(0x4e80 + i).unwrap().to_string()).collect();
            strings[37] = strings[22].clone();
            let selected = strings[22].clone();
            let values: Vec<_> = strings.iter().map(|s| (s.as_str(), "yi")).collect();
            let now = personal_picks::now_ms();
            let make = |eager| {
                let (decoder, selections) = DeepPagedDecoder::new("yijian", &values);
                let mut mixer = MixerDecoder::new(Box::new(decoder), "whole-lazy-learned".into());
                mixer.configure_learning(LearningMode::Zhixu).unwrap();
                mixer.learning.remember_for_test("yi", &selected, now);
                mixer.learning.import_choices(vec![Choice { app: "whole-lazy-learned".into(), raw: "yijian".into(),
                    reading: "yi".into(), text: selected.clone(), target: 0, count: 4,
                    last_ms: now, alternatives: Vec::new() }]);
                mixer.whole_override = Some(true);
                mixer.whole_eager = eager;
                (mixer, selections)
            };
            let (mut lazy, lazy_selections) = make(false);
            let (mut eager, eager_selections) = make(true);
            lazy.snapshot();
            assert!(lazy.whole_cache.lock().unwrap().as_ref().unwrap().base.as_ref().unwrap().is_last_page,
                "learned deep candidates must be available before ranking");
            whole_lazy_assert_navigation(&mut lazy, &mut eager);
            let all = lazy.snapshot_all_candidates().unwrap();
            assert_eq!(all.candidates.iter().filter(|candidate| candidate.text == selected).count(), 1);
            let position = all.candidates.iter().position(|candidate| candidate.text == selected).unwrap();
            assert!(position < 22, "the deep native choice is promoted before pagination");
            for _ in 0..position / PAGE_SIZE { lazy.page_down().unwrap(); eager.page_down().unwrap(); }
            assert_eq!(lazy.select_candidate(position % PAGE_SIZE, 901, LearningIntent::NoLearn).unwrap(), selected);
            assert_eq!(eager.select_candidate(position % PAGE_SIZE, 901, LearningIntent::NoLearn).unwrap(), selected);
            assert_eq!(*lazy_selections.lock().unwrap(), vec![22]);
            assert_eq!(*eager_selections.lock().unwrap(), vec![22]);
        });
    }

    #[test]
    fn whole_lazy_local_overlay_keeps_the_last_deep_duplicate_mapping() {
        zhixu::with_index(whole_fixture(), || {
            let mut strings: Vec<_> = (0..45).map(|i| char::from_u32(0x4e80 + i).unwrap().to_string()).collect();
            strings[37] = strings[22].clone();
            let selected = strings[22].clone();
            let values: Vec<_> = strings.iter().map(|s| (s.as_str(), "yijian")).collect();
            let overlay = lexicon(vec![entry_json("deep-duplicate", &selected, "yijian", false, false, false, &[])]);
            let make = |eager| {
                let (decoder, selections) = DeepPagedDecoder::new("yijian", &values);
                let mut mixer = MixerDecoder::new(Box::new(decoder), "whole-lazy-local-overlay".into());
                mixer.configure_learning(LearningMode::Zhixu).unwrap();
                mixer.apply_lexicon(overlay.clone());
                mixer.whole_override = Some(true);
                mixer.whole_eager = eager;
                (mixer, selections)
            };
            let (mut lazy, lazy_selections) = make(false);
            let (mut eager, eager_selections) = make(true);
            lazy.snapshot();
            assert!(lazy.whole_cache.lock().unwrap().as_ref().unwrap().base.as_ref().unwrap().is_last_page,
                "local/native duplicate mapping requires the complete native sequence");
            whole_lazy_assert_navigation(&mut lazy, &mut eager);
            let all = lazy.snapshot_all_candidates().unwrap();
            assert_eq!(all.candidates.iter().filter(|candidate| candidate.text == selected).count(), 1);
            let position = all.candidates.iter().position(|candidate| candidate.text == selected).unwrap();
            for _ in 0..position / PAGE_SIZE { lazy.page_down().unwrap(); eager.page_down().unwrap(); }
            assert_eq!(lazy.select_candidate(position % PAGE_SIZE, 902, LearningIntent::NoLearn).unwrap(), selected);
            assert_eq!(eager.select_candidate(position % PAGE_SIZE, 902, LearningIntent::NoLearn).unwrap(), selected);
            assert_eq!(*lazy_selections.lock().unwrap(), vec![37]);
            assert_eq!(*eager_selections.lock().unwrap(), vec![37]);
        });
    }

    fn whole_query_pipeline(mixer: &MixerDecoder) {
        let _ = mixer.snapshot();
        let _ = mixer.context_group_mask();
        let _ = mixer.context_complete_mask();
        let _ = mixer.composition_alternatives();
        let _ = mixer.composition_edits();
        let _ = mixer.snapshot_all_candidates();
        let _ = mixer.snapshot();
    }

    fn whole_test_child(name: &str) -> bool {
        const CHILD: &str = "ZHIXU_WHOLE_CACHE_TEST";
        if std::env::var(CHILD).as_deref() == Ok(name) { return true; }
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &format!("mixer::tests::{name}"), "--nocapture"])
            .env(CHILD, name).env_remove("ZHIZI_ZHIXU_WHOLE").output().unwrap();
        assert!(output.status.success(), "{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        false
    }

    /// The product session shape: a native inner built exactly as
    /// `Session::new_native` constructs it, over an empty in-memory user store.
    fn native_mixer(app: &str, dictionary: Arc<Index>) -> MixerDecoder {
        MixerDecoder::new_native(app.into(), Arc::new(Mutex::new(Store::default())), dictionary).unwrap()
    }

    fn native_mixer_with(app: &str, blacklist: Arc<Mutex<Store>>, dictionary: Arc<Index>) -> MixerDecoder {
        MixerDecoder::new_native(app.into(), blacklist, dictionary).unwrap()
    }

    fn y3e_terms_fixture() -> Arc<Index> {
        use crate::zhixu::loader::{DictionaryData, Entry};
        let mut entries: Vec<_> = [
            ("收起", "shou qi", 5000), ("来", "lai", 5000),
            ("处理", "chu li", 5000), ("哦", "o", 5000),
            ("丑陋", "chou lou", 5000), ("的", "de", 5000), ("法", "fa", 5000),
        ].into_iter().map(|(text, reading, weight)| Entry {
            text: text.into(), reading: reading.into(), weight, layer: "fixture".into(),
        }).collect();
        entries.extend([("SQLite", "sqlite"), ("Cloudflare", "cloudflare")]
            .into_iter().map(|(text, reading)| Entry {
                text: text.into(), reading: reading.into(), weight: 1000, layer: "agent_tech_terms".into(),
            }));
        Arc::new(Index::from_data(DictionaryData { entries }))
    }

    #[test]
    fn fuzzy_protection_keeps_assembled_sentences_off_the_exact_slots() {
        // An assembled sentence carries the raw letters as its reading; it must not take the
        // exact-spelling slots from a real word that only fuzzy spelling reaches (fuzzy-56 n-l).
        use crate::zhixu::loader::{DictionaryData, Entry};
        let dictionary = Arc::new(Index::from_data(DictionaryData { entries: [
            ("你们", "ni men", 9000), ("你", "ni", 9000), ("门", "men", 5000),
            ("梦", "meng", 3000), ("里", "li", 6000), ("们", "men", 4000),
        ].into_iter().map(|(text, reading, weight)| Entry {
            text: text.into(), reading: reading.into(), weight, layer: "fixture".into(),
        }).collect() }));
        zhixu::with_index(dictionary.clone(), || {
            let mut mixer = native_mixer("fuzzy.protection", dictionary.clone());
            mixer.configure_fuzzy(1 << 3).unwrap();
            mixer.input("limen").unwrap();
            let page: Vec<_> = mixer.snapshot().candidates.into_iter().map(|c| c.text).collect();
            assert_eq!(page.first().map(String::as_str), Some("你们"), "{page:?}");
        });
    }

    #[test]
    fn y3e_native_technical_prefixes_keep_existing_ascii_priority() {
        let mut failures = Vec::new();
        let dictionary = y3e_terms_fixture();
        zhixu::with_index(dictionary.clone(), || {
            for (raw, expected) in [("sql", "SQLite"), ("clo", "Cloudflare"), ("cloudf", "Cloudflare")] {
                let mut mixer = native_mixer("y3e.terms", dictionary.clone());
                mixer.input(raw).unwrap();
                let inner = mixer.inner.snapshot();
                assert!(!inner.candidates.is_empty(), "{raw}: Chinese competition must exist");
                let page = mixer.snapshot();
                let committed = mixer.select_candidate(0, 501, LearningIntent::NoLearn).unwrap();
                if page.candidates[0].text != expected || committed != expected {
                    failures.push(format!("{raw}: inner={:?}, mixed={:?}, committed={committed:?}", inner.candidates, page.candidates));
                }
            }
        });
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    fn y3e_native_first_pieces_precede_generated_completions_and_repairs() {
        use crate::zhixu::loader::{DictionaryData, Entry};
        let mut failures = Vec::new();
        for (raw, text, reading, rest, extra) in [
            ("leiss", "类似", "lei si", "s", vec![("是", "shi")]),
            ("read", "热爱", "re ai", "d", vec![("的", "de")]),
            ("wozenmy", "我怎么", "wo zen me", "y", vec![("有", "you")]),
            ("ganje", "感觉", "gan jue", "e", vec![("二", "er"), ("鹅", "e")]),
            ("renkd", "认可", "ren ke", "d", vec![("人口", "ren kou"), ("的", "de")]),
            ("houmm", "后妈", "hou ma", "m", vec![("后面", "hou mian"), ("没", "mei")]),
            ("yigng", "义工", "yi gong", "ng", vec![("应该", "ying gai"), ("能", "neng"), ("跟", "gen")]),
            ("app", "阿婆", "a po", "p", vec![("安排", "an pai"), ("怕", "pa")]),
            ("shangjai", "上架", "shang jia", "ai", vec![("商家", "shang jia"), ("爱", "ai")]),
        ] {
            let mut entries = vec![Entry { text: text.into(), reading: reading.into(), weight: 9000, layer: "fixture".into() }];
            entries.extend(extra.into_iter().map(|(text, reading)| Entry {
                text: text.into(), reading: reading.into(), weight: 5000, layer: "fixture".into(),
            }));
            let dictionary = Arc::new(Index::from_data(DictionaryData { entries }));
            zhixu::with_index(dictionary.clone(), || {
                let mut mixer = native_mixer("y3e.first-piece", dictionary.clone());
                mixer.configure_typo(true);
                mixer.input(raw).unwrap();
                let inner = mixer.inner.snapshot();
                let page = mixer.snapshot();
                let committed = mixer.select_candidate(0, 502, LearningIntent::NoLearn).unwrap();
                let remaining = mixer.confirmed_boundary().map(|boundary| boundary.unconfirmed_raw);
                if page.candidates[0].text != text || !committed.is_empty() || remaining.as_deref() != Some(rest) {
                    failures.push(format!("{raw}: inner={:?}, mixed={:?}, committed={committed:?}, rest={remaining:?}", inner.candidates, page.candidates));
                }
            });
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    fn y3e_native_complete_abbreviations_and_syllables_still_lead() {
        use crate::zhixu::loader::{DictionaryData, Entry};
        let mut entries: Vec<_> = [
            ("中国人", "zhong guo ren"), ("中国", "zhong guo"),
            ("生成", "sheng cheng"), ("生", "sheng"),
            ("很可以", "hen ke yi"), ("很可", "hen ke"),
            ("类似", "lei si"), ("是", "shi"),
        ].into_iter().map(|(text, reading)| Entry {
            text: text.into(), reading: reading.into(), weight: 5000, layer: "fixture".into(),
        }).collect();
        entries.push(Entry { text: "ShengCloud".into(), reading: "shengcloud".into(),
            weight: 5000, layer: "agent_tech_terms".into() });
        let dictionary = Arc::new(Index::from_data(DictionaryData { entries }));
        zhixu::with_index(dictionary.clone(), || {
            for (raw, text) in [("zgr", "中国人"), ("shengc", "生成"), ("henkyi", "很可以"), ("leisishi", "类似是"), ("leisshi", "类似是")] {
                let mut mixer = native_mixer("y3e.complete", dictionary.clone());
                mixer.configure_typo(true);
                mixer.input(raw).unwrap();
                assert_eq!(mixer.snapshot().candidates[0].text, text, "{raw}");
                assert_eq!(mixer.select_candidate(0, 503, LearningIntent::NoLearn).unwrap(), text, "{raw}");
            }
        });
    }

    #[test]
    fn y3e_native_ascii_evidence_keeps_the_nonempty_page_raw_fallback() {
        let raw = "qzxvjk";
        let mut decoder = ListDecoder::with(raw, &[("甲乙丙丁", raw)]);
        decoder.exact_boundary = Some(crate::state::CompositionBoundary {
            confirmed: String::new(), unconfirmed_raw: raw.into(),
        });
        let mixer = MixerDecoder::new(Box::new(decoder), "y3e.raw-fallback".into());
        assert!(mixer.native_editing());
        assert!(mixer.snapshot().candidates.iter().any(|candidate| candidate.text == raw));
    }

    #[test]
    fn y3e_rime_technical_prefixes_keep_existing_ascii_priority() {
        zhixu::with_index(y3e_terms_fixture(), || {
            for (raw, expected, values) in [
                ("sql", "SQLite", vec![("收起", "shou qi"), ("收", "shou")]),
                ("clo", "Cloudflare", vec![("处理", "chu li"), ("出", "chu")]),
                ("cloudf", "Cloudflare", vec![("丑陋", "chou lou"), ("丑", "chou")]),
            ] {
                let mixer = MixerDecoder::new(Box::new(ListDecoder::with(raw, &values)), "y3e.rime-terms".into());
                assert!(!mixer.native_editing());
                assert_eq!(mixer.snapshot().candidates[0].text, expected, "{raw}");
            }
        });
    }

    #[test]
    fn native_inner_never_runs_legacy_whole_generation() {
        if !whole_test_child("native_inner_never_runs_legacy_whole_generation") { return; }
        let mut mixer = MixerDecoder::new(Box::new(crate::zhixu::decoder::ZhixuDecoder::new()), "y3.native".into());
        mixer.configure_zhixu_whole(true).unwrap();
        mixer.input("nihao").unwrap();
        assert!(!mixer.whole_enabled());
        assert!(mixer.whole_base().is_none());
        assert!(mixer.whole_cache.lock().unwrap().is_none());
        assert!(mixer.whole_lattice.lock().unwrap().is_none());
        assert!(!mixer.snapshot().candidates.is_empty());
    }

    #[test]
    fn product_inner_stays_native_for_its_whole_life() {
        if !whole_test_child("product_inner_stays_native_for_its_whole_life") { return; }
        let dictionary = y3e_terms_fixture();
        zhixu::with_index(dictionary.clone(), || {
            let mut mixer = native_mixer("y3.switch", dictionary.clone());
            assert!(mixer.native_editing());
            assert_eq!(mixer.learning.mode, LearningMode::Zhixu);
            assert!(!mixer.whole_enabled());
            mixer.input("nihao").unwrap();
            assert_eq!(mixer.configure_zhixu_whole(false), Err(-10));
            assert_eq!(mixer.configure_fuzzy(1), Err(-10));
            assert_eq!(mixer.inner.snapshot().raw_input, "nihao");
            mixer.clear();
            // F2: Zhixu reads fuzzy spellings itself, so fuzzy input stays native.
            mixer.configure_fuzzy(1).unwrap();
            assert!(mixer.native_editing());
            assert!(!mixer.whole_enabled());
            mixer.configure_fuzzy(0).unwrap();
            assert!(mixer.native_editing());
            // The shell knob no longer changes the inner or the learning mode.
            mixer.configure_zhixu_whole(false).unwrap();
            assert!(mixer.native_editing());
            assert_eq!(mixer.learning.mode, LearningMode::Zhixu);
            mixer.configure_zhixu_whole(true).unwrap();
            assert!(mixer.native_editing());
            assert_eq!(mixer.learning.mode, LearningMode::Zhixu);
        });
    }

    #[test]
    fn y5b_ready_dictionary_does_not_construct_fallback_for_learning() {
        let dictionary = Arc::new(Index::from_data(zhixu::loader::DictionaryData { entries: Vec::new() }));
        zhixu::with_index(dictionary, || {
            let mut mixer = MixerDecoder::new(Box::new(crate::state::MockDecoder::default()), "y5b.seed".into());
            mixer.configure_learning(LearningMode::Zhixu).unwrap();
            zhixu::FALLBACK_CALLS.with(|calls| calls.set(0));
            let _pin = mixer.pin_dictionary();
            assert_eq!(zhixu::FALLBACK_CALLS.with(|calls| calls.get()), 0);
        });
    }

    #[test]
    fn y5b_explicit_reload_refreshes_pending_handoff() {
        if !whole_test_child("y5b_explicit_reload_refreshes_pending_handoff") { return; }
        let root = std::env::temp_dir().join(format!("y5b-reload-{}", std::process::id()));
        let shared = root.join("shared");
        let user = root.join("user");
        std::fs::create_dir_all(&shared).unwrap();
        std::fs::create_dir_all(&user).unwrap();
        zhixu::loader::attach(&shared, &user);
        let file = shared.join("agent_pinyin.dict.yaml");
        std::fs::write(&file, "---\nname: agent_pinyin\n...\n旧词\tjiu ci\t100\n").unwrap();
        let dictionary = zhixu::loader::deployed_snapshot().unwrap().unwrap();
        dictionary.warm_whole();
        let mut mixer = native_mixer("y5b.reload", dictionary);
        assert!(mixer.native_editing());
        std::fs::write(&file, "---\nname: agent_pinyin\n...\n新词\txin ci\t100\n").unwrap();
        mixer.reload_dictionaries().unwrap();
        assert!(mixer.inner.dictionary_snapshot().unwrap().all_readings_for_text("新词").next().is_some(),
            "the inner decoder reads the new generation");
        if let Ok(Some(ready)) = zhixu::loader::ready_deployed_snapshot() {
            assert!(ready.all_readings_for_text("旧词").next().is_none(), "reload must retire the pending old deployment");
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn native_reload_keeps_composition_and_the_next_input_uses_the_new_generation() {
        if !whole_test_child("native_reload_keeps_composition_and_the_next_input_uses_the_new_generation") { return; }
        let root = std::env::temp_dir().join(format!("y5b-handoff-{}", std::process::id()));
        let shared = root.join("shared");
        let user = root.join("user");
        std::fs::create_dir_all(&shared).unwrap();
        std::fs::create_dir_all(&user).unwrap();
        zhixu::loader::attach(&shared, &user);
        std::fs::write(shared.join("agent_pinyin.dict.yaml"), "---\nname: agent_pinyin\n...\n你好\tni hao\t100\n你\tni\t90\n").unwrap();
        let dictionary = zhixu::loader::deployed_snapshot().unwrap().unwrap();
        dictionary.warm_whole();
        let mut mixer = native_mixer_with("y5b", crate::blacklist::shared_blacklist(&user).unwrap(), dictionary);
        assert!(mixer.native_editing());
        assert_eq!(mixer.input("ni").unwrap(), "");
        let before = mixer.inner.snapshot();
        std::fs::write(shared.join("agent_pinyin.dict.yaml"), "---\nname: agent_pinyin\n...\n你好\tni hao\t100\n你\tni\t90\n泥蒿\tni hao\t80\n").unwrap();
        mixer.reload_dictionaries().unwrap();
        assert_eq!(mixer.inner.snapshot().preedit, before.preedit);
        assert_eq!(mixer.inner.snapshot().cursor_utf8, before.cursor_utf8);
        let committed = mixer.commit_raw();
        assert_eq!(committed, "ni");
        assert_eq!(mixer.input("nihao").unwrap(), "", "reload must not replay a commit");
        assert!(mixer.native_editing());
        assert_eq!(mixer.inner.snapshot().raw_input, "nihao");
        assert_eq!(mixer.learning.mode, LearningMode::Zhixu);
        assert!(mixer.inner.snapshot_all_candidates().unwrap().candidates.iter().any(|c| c.text == "你好"));
        assert!(mixer.inner.snapshot_all_candidates().unwrap().candidates.iter().any(|c| c.text == "泥蒿"),
            "the next input adopts the new generation");
        let now = personal_picks::now_ms();
        let at = mixer.snapshot().candidates.iter().position(|c| c.text == "你好").unwrap();
        let committed = mixer.select_candidate(at, 9001, LearningIntent::DeliberateSelection).unwrap();
        assert_eq!(committed, "你好");
        mixer.learning_commit(&committed, 9001, now);
        mixer.learning_key(&crate::state::KeyEvent { kind: crate::state::KeyKind::Text,
            text: "n".into(), modifiers: 0, security: crate::state::SecurityState::NonSecure,
            delivery_id: 9002 }, now + 6_000);
        let store = crate::zhixu::learning::LearningStore::load(&user, now + 6_000);
        assert!(store.entries().iter().any(|entry| entry.text == "你好" && entry.is_learned()));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn product_native_learning_persists_and_is_read_by_the_inner_decoder() {
        if !whole_test_child("product_native_learning_persists_and_is_read_by_the_inner_decoder") { return; }
        let directory = migration_fixture("拟好\tni hao\t7\n");
        let blacklist = crate::blacklist::shared_blacklist(&directory).unwrap();
        let mut mixer = native_mixer_with("y3.learning", blacklist, zhixu::fallback());
        let now = personal_picks::now_ms();
        mixer.input("nihao").unwrap();
        assert!(mixer.inner.snapshot_all_candidates().unwrap().candidates.iter().any(|candidate| candidate.text == "拟好"));
        let page = mixer.snapshot();
        let at = page.candidates.iter().position(|candidate| candidate.text == "你好").unwrap();
        let committed = mixer.select_candidate(at, 9001, LearningIntent::DeliberateSelection).unwrap();
        assert_eq!(committed, "你好");
        mixer.learning_commit(&committed, 9001, now);
        mixer.learning_key(&crate::state::KeyEvent { kind: crate::state::KeyKind::Text,
            text: "n".into(), modifiers: 0, security: crate::state::SecurityState::NonSecure,
            delivery_id: 9002 }, now + 6_000);
        let store = crate::zhixu::learning::LearningStore::load(&directory, now + 6_000);
        assert!(store.entries().iter().any(|entry| entry.text == "你好" && entry.is_learned()));
        assert!(mixer.inner.snapshot().raw_input.is_empty());
        mixer.learning.remember_for_test("ni hao", "泥蒿", now + 6_000);
        mixer.input("nihao").unwrap();
        assert!(mixer.inner.snapshot_all_candidates().unwrap().candidates.iter().any(|candidate| candidate.text == "泥蒿"));
        drop(mixer);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn y3d_native_full_picks_promote_abbreviations_with_existing_bounds() {
        if !whole_test_child("y3d_native_full_picks_promote_abbreviations_with_existing_bounds") { return; }
        let dictionary = Arc::new(Index::from_data(crate::zhixu::loader::DictionaryData { entries: [
            ("生产", "sheng chan", 9000), ("声称", "sheng cheng", 8000),
            ("生辰", "sheng chen", 7000), ("生抽", "sheng chou", 6000),
            ("生成", "sheng cheng", 1000),
        ].into_iter().map(|(text, reading, weight)| crate::zhixu::loader::Entry {
            text: text.into(), reading: reading.into(), weight, layer: "fixture".into(),
        }).collect() }));
        zhixu::with_index(dictionary.clone(), || {
            for (case, export) in [None,
                Some("#@/tick\t1000\nsheng chan \t生产\tc=7 d=8 t=999\n"),
                Some("#@/tick\t1000\nsheng chan \t生产\tc=7 d=8 t=999\nsheng cheng \t生成\tc=1 d=1 t=999\n"),
            ].into_iter().enumerate() {
                let directory = std::env::temp_dir().join(format!("aim-y3d-full-pick-{}-{case}", std::process::id()));
                std::fs::create_dir(&directory).unwrap();
                if let Some(export) = export {
                    let mut store = crate::zhixu::learning::LearningStore::default();
                    store.import_rime_export(export, personal_picks::now_ms()).unwrap();
                    store.save(&directory).unwrap();
                }
                let mut mixer = native_mixer_with("y3d.learning",
                    crate::blacklist::shared_blacklist(&directory).unwrap(), dictionary.clone());
                mixer.input("shengc").unwrap();
                let before = mixer.snapshot();
                let position = |page: &DecoderSnapshot| page.candidates.iter().position(|c| c.text == "生成").unwrap();
                assert_eq!(position(&before), if case == 2 { 1 } else { 4 }, "pure migration keeps its order: {:?}", before.candidates);
                mixer.clear();
                let now = personal_picks::now_ms();
                for pick in 1..=CONFIRMED_PICKS {
                    mixer.input("shengcheng").unwrap();
                    migration_pick_and_confirm(&mut mixer, "生成", 9300 + u64::from(pick), now + u64::from(pick) * 6_000);
                    let entry = mixer.learning.entries().iter().find(|e| e.text == "生成").cloned().unwrap();
                    assert!(entry.is_learned());
                    assert_eq!(entry.reading, "sheng cheng");
                    assert_eq!(entry.count, pick + u32::from(case == 2));
                    for raw in ["shengc", "sc", "shengch", "sheng'c"] {
                        mixer.input(raw).unwrap();
                        let native = mixer.inner.snapshot();
                        assert_eq!(position(&native), 4, "new learning is not an inner unbounded boost");
                        let expected = if entry.count >= CONFIRMED_PICKS { 0 } else { 1 };
                        for _ in 0..2 {
                            let page = mixer.snapshot();
                            assert_eq!(position(&page), expected, "case={case} pick={pick} raw={raw}");
                            assert_eq!((&page.preedit, page.cursor_utf8), (&native.preedit, native.cursor_utf8));
                        }
                        if entry.count >= CONFIRMED_PICKS {
                            assert!(ranking::owner_keeps("生成", "生产"), "confirmed first remains protected");
                        }
                        assert_eq!(mixer.select_candidate(expected, 9400, LearningIntent::NoLearn).unwrap(), "生成");
                        mixer.clear();
                    }
                }
                drop(mixer);
                std::fs::remove_dir_all(directory).unwrap();
            }
        });
    }

    #[test]
    fn y3d_abbreviation_promotions_require_native_learned_matching_readings() {
        if !whole_test_child("y3d_abbreviation_promotions_require_native_learned_matching_readings") { return; }
        for (native, raw, text, reading, candidate_reading, expected) in [
            (true, "shengc", "生成", "sheng cheng", "sheng cheng", 1),
            (false, "shengc", "生成", "sheng cheng", "sheng cheng", 4),
            (true, "cc", "重传", "zhong chuan", "chong chuan", 4),
            (true, "zc", "重传", "zhong chuan", "chong chuan", 4),
            (true, "shengcx", "生成", "sheng cheng", "sheng cheng", 4),
            (true, "sh", "声", "sheng", "sheng", 4),
            (false, "shengcheng", "生成", "sheng cheng", "sheng cheng", 1),
        ] {
            let mut inner = ListDecoder::with(raw, &[]);
            if native { inner.exact_boundary = Some(CompositionBoundary { confirmed: String::new(), unconfirmed_raw: raw.into() }); }
            let mut mixer = MixerDecoder::new(Box::new(inner), "y3d.scope".into());
            mixer.configure_learning(LearningMode::Zhixu).unwrap();
            mixer.learning.remember_for_test(reading, text, personal_picks::now_ms());
            let mut candidates: Vec<_> = ["甲甲", "乙乙", "丙丙", "丁丁", text].into_iter().map(|text| MixedCandidate {
                candidate: Candidate { text: text.into(), reading_key: candidate_reading.into(), ..Candidate::default() },
                selection: Selection::Local { text: text.into() }, tier: 2, score: 0,
            }).collect();
            mixer.apply_zhixu_promotions(raw, &mut candidates);
            assert_eq!(candidates.iter().position(|c| c.candidate.text == text), Some(expected), "native={native} raw={raw} reading={reading}");
        }
    }

    #[test]
    fn y3d_abbreviation_choices_keep_app_targets_and_confirmation_protection() {
        if !whole_test_child("y3d_abbreviation_choices_keep_app_targets_and_confirmation_protection") { return; }
        for (app, expected) in [("y3d.choice", 2), ("y3d.other", 0)] {
            let mut inner = ListDecoder::with("shengc", &[]);
            inner.exact_boundary = Some(CompositionBoundary { confirmed: String::new(), unconfirmed_raw: "shengc".into() });
            let mut mixer = MixerDecoder::new(Box::new(inner), app.into());
            mixer.configure_learning(LearningMode::Zhixu).unwrap();
            let now = personal_picks::now_ms();
            for _ in 0..CONFIRMED_PICKS { mixer.learning.remember_for_test("sheng cheng", "生成", now); }
            mixer.learning.import_choices(vec![Choice { app: "y3d.choice".into(), raw: "shengc".into(),
                reading: "sheng cheng".into(), text: "生成".into(), target: 2, count: 1, last_ms: now,
                alternatives: vec!["生产".into()] }]);
            let mut candidates: Vec<_> = ["生产", "生存", "牲畜", "生财", "生成"].into_iter().map(|text| MixedCandidate {
                candidate: Candidate { text: text.into(), reading_key: "sheng cheng".into(), ..Candidate::default() },
                selection: Selection::Local { text: text.into() }, tier: 2, score: 0,
            }).collect();
            mixer.apply_zhixu_promotions("shengc", &mut candidates);
            assert_eq!(candidates.iter().position(|c| c.candidate.text == "生成"), Some(expected));
            assert_eq!(ranking::owner_keeps("生成", "生产"), expected == 0);
        }
    }

    #[test]
    fn product_native_deletion_removes_promotions_and_pending_learning() {
        if !whole_test_child("product_native_deletion_removes_promotions_and_pending_learning") { return; }
        let dictionary = Arc::new(Index::from_data(crate::zhixu::loader::DictionaryData { entries: [
            ("你好", 10000), ("拟好", 9000),
        ].into_iter().map(|(text, weight)| crate::zhixu::loader::Entry {
            text: text.into(), reading: "ni hao".into(), weight, layer: "fixture".into(),
        }).collect() }));
        zhixu::with_index(dictionary.clone(), || {
            let directory = std::env::temp_dir().join(format!("zhixu-y3b-mixer-delete-{}", std::process::id()));
            std::fs::create_dir_all(&directory).unwrap();
            let now = personal_picks::now_ms();
            let mut store = crate::zhixu::learning::LearningStore::load(&directory, now);
            for _ in 0..10 { store.record("ni hao", "拟好", now); }
            store.record_choice(Choice { app: "y3b.delete".into(), raw: "nihao".into(),
                reading: "ni hao".into(), text: "拟好".into(), target: 0, count: 10,
                last_ms: now, alternatives: vec!["你好".into()] });
            store.save(&directory).unwrap();
            let mut mixer = native_mixer_with("y3b.delete",
                crate::blacklist::shared_blacklist(&directory).unwrap(), dictionary.clone());
            mixer.input("nihao").unwrap();
            assert_eq!(mixer.snapshot().candidates[0].text, "拟好");
            let committed = mixer.select_candidate(0, 9201, LearningIntent::DeliberateSelection).unwrap();
            assert_eq!(committed, "拟好");
            mixer.learning_commit(&committed, 9201, now);
            mixer.input("nihao").unwrap();
            assert_eq!(mixer.snapshot().candidates[0].text, "拟好");
            assert_eq!(mixer.delete_candidate(0), Ok(()));
            assert_eq!(mixer.snapshot().candidates.iter().map(|item| item.text.as_str()).collect::<Vec<_>>(), ["你好", "拟好"]);
            mixer.clear();
            mixer.learning_key(&crate::state::KeyEvent { kind: crate::state::KeyKind::Text,
                text: "n".into(), modifiers: 0, security: crate::state::SecurityState::NonSecure,
                delivery_id: 9202 }, now + 6_000);
            let saved = crate::zhixu::learning::LearningStore::load(&directory, now + 6_000);
            assert!(saved.entries().is_empty(), "deleted candidate must not be relearned by the outer pending queue");
            assert!(saved.choices().is_empty());
            drop(mixer);
            std::fs::remove_dir_all(directory).unwrap();
        });
    }

    #[test]
    fn product_native_learning_uses_the_deployed_reading_snapshot() {
        if !whole_test_child("product_native_learning_uses_the_deployed_reading_snapshot") { return; }
        let deployed = Arc::new(Index::from_data(crate::zhixu::loader::DictionaryData {
            entries: vec![crate::zhixu::loader::Entry { text: "甲乙".into(), reading: "ni hao".into(),
                weight: 900, layer: "fixture".into() }],
        }));
        zhixu::with_index(deployed.clone(), || {
            let mut mixer = native_mixer("y3.deployed-learning", deployed.clone());
            mixer.input("nihao").unwrap();
            let page = mixer.snapshot();
            let at = page.candidates.iter().position(|candidate| candidate.text == "甲乙").unwrap();
            let committed = mixer.select_candidate(at, 9101, LearningIntent::DeliberateSelection).unwrap();
            assert_eq!(committed, "甲乙");
            let now = personal_picks::now_ms();
            mixer.learning_commit(&committed, 9101, now);
            mixer.learning_key(&crate::state::KeyEvent { kind: crate::state::KeyKind::Text,
                text: "n".into(), modifiers: 0, security: crate::state::SecurityState::NonSecure,
                delivery_id: 9102 }, now + 6_000);
            assert!(mixer.learning.entries().iter().any(|entry| entry.text == "甲乙" && entry.reading == "ni hao"));
        });
    }

    #[test]
    fn whole_session_cache_reuses_sequence_for_queries_pages_and_highlights() {
        if !whole_test_child("whole_session_cache_reuses_sequence_for_queries_pages_and_highlights") { return; }
        zhixu::with_index(whole_fixture(), || {
            let mut mixer = whole_mixer("yijian", &[("意见", "yi jian")], true);
            WHOLE_TIMING.with(|timing| *timing.borrow_mut() = [(0, 0); 6]);
            let first = mixer.snapshot().candidates;
            whole_query_pipeline(&mixer);
            assert_eq!(mixer.snapshot().candidates, first);
            mixer.select_next().unwrap();
            mixer.page_down().unwrap();
            let _ = mixer.snapshot_all_candidates();
            mixer.page_up().unwrap();
            assert_eq!(mixer.snapshot().candidates, first);
            WHOLE_TIMING.with(|timing| {
                let timing = timing.borrow();
                assert_eq!(timing[2].0, 1, "native full sequence queried once");
                assert_eq!(timing[4].0, 1, "whole candidate sequence generated once");
            });
        });
    }

    #[test]
    fn whole_session_cache_refreshes_after_blacklist_changes_and_native_deletion() {
        if !whole_test_child("whole_session_cache_refreshes_after_blacklist_changes_and_native_deletion") { return; }
        zhixu::with_index(whole_fixture(), || {
            let (decoder, _) = DeepPagedDecoder::new("yijian", &[("一", "yi")]);
            let mut mixer = MixerDecoder::new(Box::new(decoder), "whole-fixture".into());
            mixer.whole_override = Some(true);
            let first = mixer.snapshot().candidates[0].text.clone();
            mixer.blacklist_candidate(0).unwrap();
            assert!(!mixer.snapshot_all_candidates().unwrap().candidates.iter().any(|c| c.text == first));
            let next = mixer.snapshot().candidates[0].text.clone();
            let mut blacklist = Store::default();
            blacklist.insert("yijian", &next).unwrap();
            mixer.apply_blacklist(Arc::new(Mutex::new(blacklist)));
            assert!(!mixer.snapshot_all_candidates().unwrap().candidates.iter().any(|c| c.text == next));
            let all = mixer.snapshot_all_candidates().unwrap().candidates;
            let position = all.iter().position(|c| c.source_index == Some(0)).unwrap();
            for _ in 0..position / PAGE_SIZE { mixer.page_down().unwrap(); }
            WHOLE_TIMING.with(|timing| *timing.borrow_mut() = [(0, 0); 6]);
            mixer.delete_candidate(position % PAGE_SIZE).unwrap();
            let _ = mixer.snapshot();
            WHOLE_TIMING.with(|timing| assert_eq!(timing.borrow()[2].0, 1, "native deletion invalidates full sequence"));
        });
    }

    #[test]
    fn whole_session_cache_sees_a_peer_blacklist_write() {
        zhixu::with_index(whole_fixture(), || {
            let blacklist = Arc::new(Mutex::new(Store::default()));
            let mut first = whole_mixer("yijian", &[("一", "yi")], true);
            let mut peer = whole_mixer("yijian", &[("一", "yi")], true);
            first.apply_blacklist(blacklist.clone());
            peer.apply_blacklist(blacklist);
            let hidden = first.snapshot().candidates[0].text.clone();
            peer.blacklist_candidate(0).unwrap();
            assert!(!first.snapshot_all_candidates().unwrap().candidates.iter().any(|c| c.text == hidden));
        });
    }

    #[test]
    fn whole_session_cache_tracks_learning_modes_switches_and_dictionary_generations() {
        if !whole_test_child("whole_session_cache_tracks_learning_modes_switches_and_dictionary_generations") { return; }
        zhixu::with_index(whole_fixture(), || {
            let mut mixer = whole_mixer("yijian", &[("一", "yi")], true);
            mixer.whole_override = None;
            mixer.whole_override = Some(true);
            mixer.configure_learning(LearningMode::Zhixu).unwrap();
            let original = mixer.snapshot_all_candidates().unwrap().candidates;
            let now = personal_picks::now_ms();
            mixer.learning.remember_for_test("yi jian", "乙键", now);
            assert!(mixer.snapshot_all_candidates().unwrap().candidates.iter().any(|c| c.text == "乙键"));
            for _ in 0..CONFIRMED_PICKS { mixer.learning.remember_for_test("yi jian", "一键", now); }
            assert_eq!(mixer.snapshot().candidates[0].text, "一键", "count-only updates invalidate ranking");
            ranking::set_confirmed_first(Some("cache-unrelated".into()));
            let _ = mixer.snapshot();
            assert!(!ranking::owner_keeps("cache-unrelated", "一键"), "cache hits restore confirmed-first protection");
            mixer.configure_learning(LearningMode::Rime).unwrap();
            assert_eq!(mixer.snapshot_all_candidates().unwrap().candidates, original);
            ranking::set_confirmed_first(Some("cache-unrelated".into()));
            let _ = mixer.snapshot();
            assert!(!ranking::owner_keeps("cache-unrelated", "意见"), "an unconfirmed cached page also clears protection");
            mixer.configure_learning(LearningMode::Zhixu).unwrap();
            assert_eq!(mixer.snapshot().candidates[0].text, "一键");
            mixer.whole_override = Some(false);
            assert!(mixer.snapshot_all_candidates().is_none());
            mixer.whole_override = Some(true);
            assert_eq!(mixer.snapshot().candidates[0].text, "一键");
            let replacement = Arc::new(Index::from_data(zhixu::loader::DictionaryData {
                entries: vec![zhixu::loader::Entry { text: "异简".into(), reading: "yi jian".into(), weight: 1000, layer: "fixture".into() }],
            }));
            zhixu::with_index(replacement.clone(), || {
                assert!(!mixer.snapshot_all_candidates().unwrap().candidates.iter().any(|c| c.text == "异简"), "displayed dictionary stays pinned");
                mixer.reload_dictionaries().unwrap();
                assert!(mixer.snapshot_all_candidates().unwrap().candidates.iter().any(|c| c.text == "异简"));
            });
            mixer.input("l").unwrap();
            mixer.backspace().unwrap();
            assert!(!mixer.snapshot_all_candidates().unwrap().candidates.iter().any(|c| c.text == "异简"), "next edit repins the current dictionary");
        });
    }

    #[test]
    fn whole_session_cache_does_not_keep_native_page_ineligibility() {
        zhixu::with_index(whole_fixture(), || {
            let (decoder, _) = DeepPagedDecoder::new("yijian", &[
                ("yijian", "yijian"), ("一", "yi"), ("以", "yi"), ("已", "yi"), ("易", "yi"), ("见", "jian"),
            ]);
            let mut mixer = MixerDecoder::new(Box::new(decoder), "whole-eligibility".into());
            mixer.whole_override = Some(true);
            assert!(mixer.snapshot_all_candidates().is_none());
            mixer.page_down().unwrap();
            assert!(mixer.snapshot_all_candidates().is_some());
        });
    }

    #[test]
    fn whole_session_cached_pages_match_fresh_ranking_after_context_changes() {
        if !whole_test_child("whole_session_cached_pages_match_fresh_ranking_after_context_changes") { return; }
        zhixu::with_index(whole_fixture(), || {
            for raw in ["yijian", "yijianle", "gaole", "yi'jian", "xian", "yij"] {
                let mut mixer = whole_mixer(raw, &[("一", "yi"), ("意见", "yi jian")], true);
                mixer.configure_learning(LearningMode::Zhixu).unwrap();
                mixer.learning.remember_for_test("yi jian", "一键", personal_picks::now_ms());
                if let Some(text) = mixer.composition_alternatives().first().cloned() {
                    let _ = mixer.lead_with_composition(&text);
                    let _ = mixer.offer_composition(&text);
                }
                loop {
                    let cached = mixer.snapshot();
                    let all = mixer.snapshot_all_candidates().map(|all| all.candidates);
                    mixer.invalidate_whole();
                    let fresh = mixer.snapshot();
                    assert_eq!(cached.candidates, fresh.candidates, "{raw} page {}", cached.page_no);
                    assert_eq!(cached.secondary_candidates, fresh.secondary_candidates);
                    assert_eq!(cached.is_last_page, fresh.is_last_page);
                    assert_eq!(all, mixer.snapshot_all_candidates().map(|all| all.candidates));
                    if fresh.is_last_page { break; }
                    mixer.page_down().unwrap();
                }
            }
        });
    }

    /// Synthetic core-only query workload; excludes Rime, the host and model inference.
    #[test]
    fn whole_session_incremental_pages_match_fresh_and_reuse_prefixes() {
        if !whole_test_child("whole_session_incremental_pages_match_fresh_and_reuse_prefixes") { return; }
        zhixu::with_index(zhixu::fallback(), || {
            WHOLE_REUSED.with(|count| count.set(0));
            for keys in ["yijian", "gaole", "womengaichengxiawuchaliao", "duoxianchengdeyilaijiao", "jiexialaizuoshenme", "yi'jian", "xi'an"] {
                let (decoder, _) = DeepPagedDecoder::new("", &[("一", "yi")]);
                let mut mixer = MixerDecoder::new(Box::new(decoder), "whole-incremental".into());
                mixer.whole_override = Some(true);
                for ch in keys.chars() {
                    mixer.input(&ch.to_string()).unwrap();
                    let cached = mixer.snapshot_all_candidates().map(|s| s.candidates);
                    let (decoder, _) = DeepPagedDecoder::new(&mixer.inner.snapshot().raw_input, &[("一", "yi")]);
                    let mut fresh = MixerDecoder::new(Box::new(decoder), "whole-incremental".into());
                    fresh.whole_override = Some(true);
                    assert_eq!(cached, fresh.snapshot_all_candidates().map(|s| s.candidates), "{keys}: {}", mixer.inner.snapshot().raw_input);
                    if cached.is_some() {
                        loop {
                            let a = mixer.snapshot();
                            let b = fresh.snapshot();
                            assert_eq!(a.candidates, b.candidates);
                            assert_eq!(a.is_last_page, b.is_last_page);
                            if a.is_last_page { break; }
                            mixer.page_down().unwrap();
                            fresh.page_down().unwrap();
                        }
                    }
                }
                for _ in 0..3 {
                    mixer.backspace().unwrap();
                    let cached = mixer.snapshot_all_candidates().map(|s| s.candidates);
                    mixer.invalidate_whole();
                    assert_eq!(cached, mixer.snapshot_all_candidates().map(|s| s.candidates));
                }
                mixer.input("a").unwrap();
                let cached = mixer.snapshot_all_candidates().map(|s| s.candidates);
                mixer.invalidate_whole();
                assert_eq!(cached, mixer.snapshot_all_candidates().map(|s| s.candidates));
            }
            WHOLE_REUSED.with(|count| assert!(count.get() > 0, "append should reuse lattice prefixes"));
        });
    }

    #[test]
    fn whole_session_incremental_edit_boundaries_reset_the_lattice() {
        struct EditDecoder { inner: DeepPagedDecoder, cursor: usize }
        impl Decoder for EditDecoder {
            fn snapshot(&self) -> DecoderSnapshot {
                let mut snapshot = self.inner.snapshot();
                let boundary = self.inner.raw.len().min(2);
                snapshot.preedit.insert(boundary, ' ');
                snapshot.cursor_utf8 = self.cursor + usize::from(self.cursor > boundary);
                snapshot
            }
            fn snapshot_all_candidates(&self) -> Option<DecoderSnapshot> { self.inner.snapshot_all_candidates() }
            fn input(&mut self, text: &str) -> Result<String, i32> {
                self.inner.raw.insert_str(self.cursor, text);
                self.cursor += text.len();
                Ok(String::new())
            }
            fn backspace(&mut self) -> Result<String, i32> {
                if self.cursor > 0 { self.cursor -= 1; self.inner.raw.remove(self.cursor); }
                Ok(String::new())
            }
            fn move_left(&mut self) -> Result<String, i32> { self.cursor = self.cursor.saturating_sub(1); Ok(String::new()) }
            fn move_right(&mut self) -> Result<String, i32> { self.cursor = (self.cursor + 1).min(self.inner.raw.len()); Ok(String::new()) }
            fn page_up(&mut self) -> Result<String, i32> { self.inner.page_up() }
            fn page_down(&mut self) -> Result<String, i32> { self.inner.page_down() }
            fn select_previous(&mut self) -> Result<String, i32> { self.inner.select_previous() }
            fn select_next(&mut self) -> Result<String, i32> { self.inner.select_next() }
            fn select_candidate(&mut self, index: usize, delivery: u64, intent: LearningIntent) -> Result<String, i32> {
                self.inner.select_candidate(index, delivery, intent)
            }
            fn delete_candidate(&mut self, index: usize) -> Result<(), i32> { self.inner.delete_candidate(index) }
            fn commit_raw(&mut self) -> String { self.cursor = 0; self.inner.commit_raw() }
            fn clear(&mut self) { self.cursor = 0; self.inner.clear(); }
        }
        zhixu::with_index(whole_fixture(), || {
            let (inner, _) = DeepPagedDecoder::new("yijia", &[("一", "yi")]);
            let mut mixer = MixerDecoder::new(Box::new(EditDecoder { inner, cursor: 5 }), "whole-edit".into());
            mixer.whole_override = Some(true);
            let _ = mixer.snapshot();
            assert!(mixer.whole_lattice.lock().unwrap().is_some());
            mixer.input("n").unwrap();
            assert!(mixer.whole_lattice.lock().unwrap().is_some(), "end cursor is measured in spaced preedit bytes");
            mixer.backspace().unwrap();
            assert!(mixer.whole_lattice.lock().unwrap().is_none());
            let _ = mixer.snapshot();
            mixer.move_left().unwrap();
            // Inserting the same letter in the middle still gives a string-prefix
            // match; the cursor check must force a rebuild before the next query.
            mixer.input("a").unwrap();
            assert_eq!(mixer.inner.snapshot().raw_input, "yijiaa");
            assert!(mixer.whole_lattice.lock().unwrap().is_none());
            let actual = mixer.snapshot_all_candidates().map(|s| s.candidates);
            let (decoder, _) = DeepPagedDecoder::new("yijiaa", &[("一", "yi")]);
            let mut fresh = MixerDecoder::new(Box::new(decoder), "whole-edit".into());
            fresh.whole_override = Some(true);
            assert_eq!(actual, fresh.snapshot_all_candidates().map(|s| s.candidates));
            mixer.reload_dictionaries().unwrap();
            assert!(mixer.whole_lattice.lock().unwrap().is_none());
            let _ = mixer.snapshot();
            mixer.configure_learning(LearningMode::Zhixu).unwrap();
            assert!(mixer.whole_lattice.lock().unwrap().is_none());
        });
    }

    #[test]
    fn whole_session_syllable_count_matches_original_segmentation() {
        let check = |reading: &str| {
            let original = reading.split('\'').try_fold(0, |count, segment|
                pinyin_segmentation(segment).map(|syllables| count + syllables.len()));
            assert_eq!(whole_syllable_count(reading), original, "{reading}");
        };
        let syllables: Vec<_> = PINYIN_SYLLABLES.split_whitespace().collect();
        for (position, syllable) in syllables.iter().enumerate() {
            let reading = format!("{syllable}{}{}", syllables[(position * 7) % syllables.len()], syllables[(position * 13) % syllables.len()]);
            for end in 1..=reading.len() { check(&reading[..end]); }
            check(&reading.replace('a', "'a"));
        }
        for reading in ["", "'", "yi''jian", "xi'an", "YI JIAN", "中", "ni'  hao", "yi3", &"a".repeat(120), &"a".repeat(121)] { check(reading); }
    }

    #[test]
    fn whole_session_incremental_lattice_matches_original_with_learning_and_edits() {
        let now = personal_picks::now_ms();
        let entries = vec![
            zhixu::learning::Entry { reading: "yi jian".into(), text: "乙键".into(), count: 2, last_ms: now, rime: None },
            zhixu::learning::Entry { reading: "gao".into(), text: "搞".into(), count: 1, last_ms: now, rime: None },
        ];
        zhixu::with_index(zhixu::fallback(), || {
            let _personal = zhixu::pin_learning(Some(Arc::new(Index::from_learning(&entries))));
            for (cap, beam) in [1, 6, 12, 100].into_iter().flat_map(|cap| [40, 96].map(|beam| (cap, beam))) {
                let mut cache = None;
                for keys in ["yijian", "gaole", "womengaichengxiawuchaliao", "duoxianchengdeyilaijiao", "jiexialaizuoshenme", "yi'jian", "xi'an"] {
                    for end in 1..=keys.len() {
                        let reading = &keys[..end];
                        let actual = decode_whole_lattice(reading, 120, cap, beam, &entries, 0, Some((&mut cache, 1)), now);
                        assert_eq!(actual, decode_whole_reference(reading, 120, cap, beam, &entries, 0, now), "{reading} cap={cap}");
                    }
                    // Backspace, replacing a middle letter, and another append all recompute
                    // the changed prefix, even if the edited input has no complete spelling.
                    for reading in [&keys[..keys.len() - 1], &keys.replacen('a', "o", 1), keys] {
                        let actual = decode_whole_lattice(reading, 120, cap, beam, &entries, 0, Some((&mut cache, 1)), now);
                        assert_eq!(actual, decode_whole_reference(reading, 120, cap, beam, &entries, 0, now), "edit {reading} cap={cap}");
                    }
                }
            }
        });
    }

    #[test]
    fn whole_session_incremental_lattice_invalidates_versions_caps_and_decayed_scores() {
        let now = 1_000_000;
        let entries = vec![zhixu::learning::Entry { reading: "yi jian".into(), text: "一键".into(),
            count: 1, last_ms: now, rime: None }];
        zhixu::with_index(whole_fixture(), || {
            for change in 0..8 {
                let mut cache = None;
                decode_whole_lattice("yijia", 48, 6, 96, &entries, 0, Some((&mut cache, 1)), now);
                WHOLE_REUSED.with(|count| count.set(0));
                let revision = if change == 1 { 2 } else { 1 };
                let tick = if change == 2 { 1 } else { 0 };
                let beam = if change == 7 { 40 } else { 96 };
                let cap = if change == 3 { 7 } else { 6 };
                let time = if change == 4 { now + 30 * 24 * 60 * 60 * 1000 } else { now };
                let dictionary = if change == 5 { whole_fixture() } else { zhixu::current() };
                let _personal = zhixu::pin_learning(if change == 6 { Some(whole_fixture()) } else { None });
                zhixu::with_index(dictionary, || {
                    let actual = decode_whole_lattice("yijian", 48, cap, beam, &entries, tick, Some((&mut cache, revision)), time);
                    assert_eq!(actual, decode_whole_reference("yijian", 48, cap, beam, &entries, tick, time));
                });
                WHOLE_REUSED.with(|count| assert_eq!(count.get(), usize::from(change == 0), "change={change}"));
            }
            // A smaller cap also changes pruning, and the old final must remain unpruned.
            let mut cache = None;
            decode_whole_lattice("yijian", 1, 12, 96, &entries, 0, Some((&mut cache, 1)), now);
            assert!(cache.as_ref().unwrap().paths[6].len() > 1);
            WHOLE_REUSED.with(|count| count.set(0));
            assert_eq!(decode_whole_lattice("yijianle", 48, 6, 96, &entries, 0, Some((&mut cache, 1)), now),
                decode_whole_reference("yijianle", 48, 6, 96, &entries, 0, now));
            WHOLE_REUSED.with(|count| assert_eq!(count.get(), 0));
        });
    }

    #[test]
    fn whole_generator_homophone_merge_matches_original_learning_sort() {
        let index = Arc::new(Index::from_data(zhixu::loader::DictionaryData { entries: [
            ("西安", "xi an", 1000), ("先安", "xian", 800), ("西安", "xian", 800),
            ("相安", "xian", 800), ("先案", "xi an", 1200), ("西案", "xi an", 800),
        ].into_iter().map(|(text, reading, weight)| zhixu::loader::Entry {
            text: text.into(), reading: reading.into(), weight, layer: "fixture".into(),
        }).collect() }));
        let entries: Vec<_> = [("xi an", "西安", 7), ("xian", "西安", 2),
            ("xi an", "习安", 3), ("xian", "先安", 9), ("xian", "新案", 1)]
            .into_iter().map(|(reading, text, count)| zhixu::learning::Entry {
                reading: reading.into(), text: text.into(), count, last_ms: 1_000_000, rime: None,
            }).collect();
        zhixu::with_index(index.clone(), || {
            let personal = Arc::new(Index::from_learning(&entries));
            let _personal = zhixu::pin_learning(Some(personal.clone()));
            for reading in ["xian", "xi'an"] {
                let compact = reading.replace('\'', "");
                let count = whole_syllable_count(reading).unwrap();
                for inherited in [false, true] {
                    for tick in [0, 100] {
                        let mut original: Vec<_> = [&*index, &*personal].into_iter().flat_map(|dictionary|
                            dictionary.whole_words_for_compact_reading(&compact))
                            .filter(|word| word.text.chars().count() >= 2 && whole_spelling_matches(reading, word.reading))
                            .map(|word| (word.text, word.weight, word.syllables)).collect();
                        let mut learned: HashMap<&str, f64> = HashMap::new();
                        for entry in entries.iter().filter(|entry| whole_spelling_matches(reading, &entry.reading)) {
                            original.push((entry.text.as_str(), 800, entry.reading.split_whitespace().count()));
                            if inherited {
                                let score = entry.score(tick);
                                learned.entry(&entry.text).and_modify(|kept| *kept = kept.max(score)).or_insert(score);
                            }
                        }
                        original.sort_by(|a, b| (b.2 == count).cmp(&(a.2 == count))
                            .then_with(|| learned.get(b.0).partial_cmp(&learned.get(a.0)).unwrap_or(Ordering::Equal))
                            .then(b.1.cmp(&a.1)).then(a.0.cmp(&b.0)));
                        let mut seen = HashSet::new();
                        original.retain(|(text, _, _)| seen.insert(*text));
                        let expected: Vec<_> = original.into_iter().map(|(text, _, syllables)| MixedCandidate {
                            candidate: Candidate { text: text.into(),
                                comment: if syllables == count { ZHIXU_WHOLE_COMMENT } else { ZHIXU_WHOLE_TAIL_COMMENT }.into(),
                                reading_key: reading.into(), source_index: None },
                            selection: Selection::Local { text: text.into() }, tier: 2, score: 0,
                        }).collect();
                        let (actual, complete) = whole_reading_candidates_prefix_with(reading, &entries, tick, inherited, None, &|_| true);
                        assert!(!complete);
                        assert_eq!(format!("{actual:?}"), format!("{expected:?}"), "{reading} inherited={inherited} tick={tick}");
                    }
                }
            }
        });
    }

    #[test]
    fn whole_generator_defers_word_lattice_and_completes_exact_reading() {
        zhixu::with_index(whole_fixture(), || {
            let mut cache = None;
            WHOLE_TIMING.with(|timing| *timing.borrow_mut() = [(0, 0); 6]);
            WHOLE_LATTICE_EXPANSIONS.with(|count| count.set(0));
            let (prefix, complete) = whole_reading_candidates_prefix_with("yijian", &[], 0, false,
                Some((&mut cache, 0)), &|_| true);
            assert!(!complete, "word candidates do not require the lattice tail");
            assert!(prefix.iter().any(|item| item.candidate.comment == ZHIXU_WHOLE_COMMENT));
            WHOLE_TIMING.with(|timing| assert_eq!(timing.borrow()[5].0, 0));
            let full = whole_reading_candidates_with("yijian", &[], 0, false, Some((&mut cache, 0)));
            assert!(prefix.len() <= full.len());
            assert_eq!(format!("{prefix:?}"), format!("{:?}", &full[..prefix.len()]));
            let original = full.clone();
            WHOLE_REUSED.with(|count| count.set(0));
            assert_eq!(format!("{:?}", whole_reading_candidates_with("yijian", &[], 0, false, Some((&mut cache, 0)))), format!("{original:?}"));
            WHOLE_REUSED.with(|count| assert_eq!(count.get(), 1, "same-reading tail completion reuses final states"));
            WHOLE_LATTICE_EXPANSIONS.with(|count| assert_eq!(count.get(), 1, "complete queries must not re-expand final states"));
        });
    }

    #[test]
    fn whole_generator_prefix_and_completed_tail_match_full_on_fallback() {
        zhixu::with_index(zhixu::fallback(), || {
            for keys in ["yijian", "gaole", "womengaichengxiawuchaliao", "duoxianchengdeyilaijiao", "jiexialaizuoshenme", "zhongjianbuyaoting"] {
                let mut cache = None;
                for end in 1..=keys.len() {
                    let reading = &keys[..end];
                    let full = whole_reading_candidates_with(reading, &[], 0, false, None);
                    let reject = full.iter().find(|item| item.candidate.comment == ZHIXU_WHOLE_SENTENCE_COMMENT)
                        .map(|item| item.candidate.text.as_str());
                    let accept = |text: &str| Some(text) != reject;
                    WHOLE_LATTICE_EXPANSIONS.with(|count| count.set(0));
                    let (prefix, complete) = whole_reading_candidates_prefix_with(reading, &[], 0, false,
                        Some((&mut cache, 0)), &accept);
                    assert!(prefix.len() <= full.len(), "{reading}");
                    assert_eq!(format!("{prefix:?}"), format!("{:?}", &full[..prefix.len()]), "{reading}");
                    if complete { assert_eq!(format!("{prefix:?}"), format!("{full:?}"), "{reading}"); }
                    let eligible_sentence = |items: &[MixedCandidate]| items.iter().find(|item|
                        item.candidate.comment == ZHIXU_WHOLE_SENTENCE_COMMENT && accept(&item.candidate.text))
                        .map(|item| item.candidate.text.clone());
                    assert_eq!(eligible_sentence(&prefix), eligible_sentence(&full), "filtered sentence {reading}");
                    assert_eq!(format!("{:?}", whole_reading_candidates_with(reading, &[], 0, false, Some((&mut cache, 0)))), format!("{full:?}"),
                        "complete {reading}");
                    WHOLE_LATTICE_EXPANSIONS.with(|count| assert!(count.get() <= 1, "duplicate expansion {reading}"));
                }
            }
        });
    }

    /// Generator-only timings include every typed prefix; hashes pin the original order.
    #[test]
    #[ignore]
    fn whole_generator_timing() {
        zhixu::with_index(zhixu::fallback(), || {
            let cold = std::time::Instant::now();
            let _ = whole_reading_candidates("yijian", &[], 0, false);
            println!("WHOLE_GENERATOR_COLD\tus={:.3}", cold.elapsed().as_secs_f64() * 1e6);
            for keys in ["yijian", "gaole", "womengaichengxiawuchaliao", "duoxianchengdeyilaijiao", "jiexialaizuoshenme"] {
                let mut times = Vec::new();
                let mut active = Vec::new();
                let mut hash = 0xcbf2_9ce4_8422_2325u64;
                for round in 0..28 {
                    let (decoder, _) = DeepPagedDecoder::new("", &[("一", "yi")]);
                    let mut mixer = MixerDecoder::new(Box::new(decoder), "whole-generator-timing".into());
                    mixer.whole_override = Some(true);
                    for ch in keys.chars() {
                        WHOLE_TIMING.with(|timing| *timing.borrow_mut() = [(0, 0); 6]);
                        mixer.input(&ch.to_string()).unwrap();
                        whole_query_pipeline(&mixer);
                        WHOLE_TIMING.with(|timing| {
                            let (calls, nanos) = timing.borrow()[4];
                            if round >= 3 {
                                times.push(nanos as f64 / 1e3);
                                if calls > 0 { active.push(nanos as f64 / 1e3); }
                            }
                        });
                        if round == 3 {
                            let nanos = WHOLE_TIMING.with(|timing| timing.borrow()[4].1);
                            let lattice = WHOLE_TIMING.with(|timing| timing.borrow()[5].1);
                            if let Ok(cache) = mixer.whole_lattice.lock() && let Some(cache) = cache.as_ref() {
                                let bytes = cache.paths.capacity() * std::mem::size_of::<Vec<WholePath>>()
                                    + cache.paths.iter().map(|paths| paths.capacity() * std::mem::size_of::<WholePath>()
                                        + paths.iter().map(|path| path.1.capacity()).sum::<usize>()).sum::<usize>();
                                println!("WHOLE_GENERATOR_KEY\t{keys}\t{}\tus={:.3}\tlattice_us={:.3}\tcap={}\tfinal_paths={}\tpath_bytes={bytes}",
                                    mixer.inner.snapshot().raw_input, nanos as f64 / 1e3, lattice as f64 / 1e3, cache.cap,
                                    cache.paths.last().map_or(0, Vec::len));
                            }
                            if let Some(all) = mixer.snapshot_all_candidates() {
                                for c in all.candidates {
                                    for value in [&c.text, &c.comment, &c.reading_key] {
                                        for byte in value.bytes().chain([0]) {
                                            hash = (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                for (label, samples) in [("all", &mut times), ("active", &mut active)] {
                    samples.sort_by(f64::total_cmp);
                    println!("WHOLE_GENERATOR_TIME\t{keys}\t{label}\tsamples={}\tp50_us={:.3}\tp95_us={:.3}\thash={hash:016x}",
                        samples.len(), samples[samples.len() / 2], samples[samples.len() * 95 / 100]);
                }
            }
        });
    }

    /// Synthetic core-only query workload; excludes Rime, the host and model inference.
    #[test]
    #[ignore]
    fn whole_session_timing() {
        zhixu::with_index(zhixu::fallback(), || {
            for keys in ["yijian", "gaole", "womengaichengxiawuchaliao", "duoxianchengdeyilaijiao", "jiexialaizuoshenme"] {
                let mut times = Vec::new();
                for round in 0..28 {
                    let (decoder, _) = DeepPagedDecoder::new("", &[("一", "yi")]);
                    let mut mixer = MixerDecoder::new(Box::new(decoder), "whole-timing".into());
                    mixer.whole_override = Some(true);
                    for ch in keys.chars() {
                        let start = std::time::Instant::now();
                        mixer.input(&ch.to_string()).unwrap();
                        whole_query_pipeline(&mixer);
                        if round >= 3 { times.push(start.elapsed().as_secs_f64() * 1e3); }
                    }
                }
                times.sort_by(f64::total_cmp);
                println!("WHOLE_SESSION_TIME\t{keys}\tsamples={}\tp50_ms={:.3}\tp95_ms={:.3}\tp99_ms={:.3}",
                    times.len(), times[times.len() / 2], times[(times.len() * 95 / 100).min(times.len() - 1)],
                    times[(times.len() * 99 / 100).min(times.len() - 1)]);
            }
            let mut mixer = whole_mixer("yijian", &[("一", "yi")], true);
            WHOLE_TIMING.with(|timing| *timing.borrow_mut() = [(0, 0); 6]);
            whole_query_pipeline(&mixer);
            WHOLE_TIMING.with(|timing| {
                for (name, (calls, nanos)) in ["pin_dictionary", "whole_base", "native_all", "whole_ranked", "whole_reading", "lattice"]
                    .into_iter().zip(*timing.borrow()) {
                    println!("WHOLE_SESSION_CALL\t{name}\tcalls={calls}\tinclusive_us={:.3}", nanos as f64 / 1e3);
                }
            });
            mixer.select_next().unwrap();
        });
    }

    fn whole_learning_fixture() -> Arc<Index> {
        use crate::zhixu::loader::{DictionaryData, Entry};
        Arc::new(Index::from_data(DictionaryData { entries: [
            ("中间", "zhong jian", 1000), ("不要听", "bu yao ting", 1500),
            ("听", "ting", 1180957), ("这种", "zhe zhong", 51161),
            ("标识", "biao shi", 2175), ("表", "biao", 18141), ("是", "shi", 1180957),
            ("承载", "cheng zai", 870), ("成", "cheng", 33117), ("在", "zai", 974740),
            ("不了", "bu liao", 1000), ("我", "wo", 1000),
            ("让它", "rang ta", 1519), ("让他", "rang ta", 7960),
        ].into_iter().map(|(text, reading, weight)| Entry {
            text: text.into(), reading: reading.into(), weight, layer: "fixture".into(),
        }).collect() }))
    }

    #[test]
    fn whole_shadow_learning_composes_new_words_without_changing_native_order() {
        zhixu::with_index(whole_learning_fixture(), || {
            let mut mixer = whole_mixer("zhongjianbuyaoting", &[("中间不要听", "zhong jian bu yao ting")], false);
            mixer.configure_learning(LearningMode::Zhixu).unwrap();
            let native = mixer.snapshot().candidates;
            for _ in 0..2 { mixer.learning.remember_for_test("bu yao ting", "不要停", personal_picks::now_ms()); }
            assert_eq!(mixer.snapshot().candidates, native);
            mixer.whole_override = Some(true);
            assert_eq!(mixer.snapshot().candidates[0].text, "中间不要停");
            mixer.whole_override = Some(false);
            assert_eq!(mixer.snapshot().candidates, native);
        });
    }

    #[test]
    fn whole_shadow_migrated_learning_composes_without_new_selections() {
        let directory = migration_fixture("#@/tick\t1000\nbu yao ting \t不要停\tc=2 d=2 t=1000\n");
        zhixu::with_index(whole_learning_fixture(), || {
            let mut mixer = whole_mixer("zhongjianbuyaoting", &[("中间不要听", "zhong jian bu yao ting")], false);
            mixer.learning.attach(&directory);
            mixer.configure_learning(LearningMode::Zhixu).unwrap();
            assert!(mixer.learning.use_rime_order("zhongjianbuyaoting"));
            assert!(!mixer.learning.entries()[0].is_learned());
            let native = mixer.snapshot().candidates;
            mixer.whole_override = Some(true);
            assert_eq!(mixer.snapshot().candidates[0].text, "中间不要停");
            assert!(mixer.learning.use_rime_order("zhongjianbuyaoting"));
            assert!(mixer.learning.whole_overlay().unwrap().vocabulary_word("不要停", "buyaoting"));
            assert!(mixer.learning.overlay().is_none());
            assert!(mixer.learning.whole_overlay().unwrap().vocabulary_word("不要停", "buyaoting"));
            mixer.whole_override = Some(false);
            assert_eq!(mixer.snapshot().candidates, native);
        });
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn whole_shadow_public_sentences_follow_poet_scores() {
        use crate::zhixu::loader::{DictionaryData, Entry};
        let index = Arc::new(Index::from_data(DictionaryData { entries: [
            ("祝", "zhu", 4031), ("祝你", "zhu ni", 1706), ("住", "zhu", 21924),
            ("你", "ni", 973526), ("你好", "ni hao", 20728), ("好", "hao", 378543),
            ("请", "qing", 55964), ("再", "zai", 120734), ("在", "zai", 974740),
            ("确认", "que ren", 13268), ("再确认", "zai que ren", 510), ("请在", "qing zai", 1338),
            ("天", "tian", 44055), ("提案", "ti an", 1071), ("晚上", "wan shang", 43945),
            ("吃", "chi", 65315), ("保持", "bao chi", 16708), ("安静", "an jing", 3689),
            ("现在", "xian zai", 246263), ("出发", "chu fa", 5678), ("今天", "jin tian", 109961),
            ("很", "hen", 160208), ("冷", "leng", 6135), ("很冷", "hen leng", 354),
            ("我", "wo", 1192789), ("我想", "wo xiang", 15660), ("想", "xiang", 141790),
            ("想去", "xiang qu", 2787), ("去", "qu", 330876), ("去北京", "qu bei jing", 417),
            ("北京", "bei jing", 53176), ("北京旅游", "bei jing lv you", 104), ("旅游", "lv you", 11088),
            ("发送", "fa song", 6966), ("成功", "cheng gong", 27437),
        ].into_iter().map(|(text, reading, weight)| Entry {
            text: text.into(), reading: reading.into(), weight, layer: "fixture".into(),
        }).collect() }));
        zhixu::with_index(index, || {
            // Public-corpus sentences, with dictionary weights frozen in this fixture.
            for (reading, expected) in [
                ("zhunihao", "祝你好"), ("qingzaiqueren", "请再确认"),
                ("tianwanshangchi", "天晚上吃"), ("baochianjing", "保持安静"),
                ("xianzaichufa", "现在出发"), ("jintianhenleng", "今天很冷"),
                ("woxiangqubeijinglvyou", "我想去北京旅游"), ("fasongchenggong", "发送成功"),
            ] {
                let sentences = decode_lattice_with(reading, 12, false, Some((6, &[], 0)));
                assert_eq!(sentences[0].0, expected, "{reading}: {sentences:?}");
                assert_eq!(whole_reading_candidates(reading, &[], 0, false)[0].candidate.text, expected);
            }
        });
    }

    #[test]
    fn whole_shadow_poet_boundary_uses_log_probabilities() {
        use crate::zhixu::loader::{DictionaryData, Entry};
        for (weight, expected) in [(99, "上下系"), (101, "甲乙系")] {
            let index = Arc::new(Index::from_data(DictionaryData { entries: [
                ("上", "shang", 100_000_000), ("下", "xia", 100_000_000),
                ("系", "xi", 1), ("甲乙", "shang xia", weight),
            ].into_iter().map(|(text, reading, weight)| Entry {
                text: text.into(), reading: reading.into(), weight, layer: "fixture".into(),
            }).collect() }));
            zhixu::with_index(index, || {
                let sentences = decode_lattice_with("shangxiaxi", 10, false, Some((6, &[], 0)));
                assert_eq!(sentences[0].0, expected, "weight={weight}: {sentences:?}");
                assert_eq!(decode_lattice("shangxiaxi", 10, false)[0], "上下系",
                    "default scoring stays unchanged");
            });
        }
    }

    #[test]
    fn whole_shadow_dictionary_words_beat_high_frequency_single_characters() {
        zhixu::with_index(whole_learning_fixture(), || {
            for (raw, expected, split) in [("zhezhongbiaoshi", "这种标识", "这种表是"),
                ("chengzaibuliao", "承载不了", "成在不了")]
            {
                let mixer = whole_mixer(raw, &[(split, raw)], true);
                let all = mixer.snapshot_all_candidates().unwrap().candidates;
                assert_eq!(all[0].text, expected);
                assert!(all.iter().any(|candidate| candidate.text == split));
                assert_eq!(decode_lattice(raw, 12, false)[0], split, "default scoring stays unchanged");
            }
        });
    }

    #[test]
    fn whole_shadow_single_characters_keep_their_frequency_order_inside_sentences() {
        use crate::zhixu::loader::{DictionaryData, Entry};
        let index = Arc::new(Index::from_data(DictionaryData { entries: [
            ("是", "shi", 1180957), ("事", "shi", 300000), ("得", "dei", 5000), ("随着", "sui zhe", 9000),
        ].into_iter().map(|(text, reading, weight)| Entry {
            text: text.into(), reading: reading.into(), weight, layer: "fixture".into(),
        }).collect() }));
        zhixu::with_index(index, || {
            // Both are above the old clamp; a clamp tied them and 事随着 could lead.
            let sentences = super::decode_lattice_with("shisuizhe", 10, false, Some((6, &[], 0)));
            assert_eq!(sentences.first().map(|(text, _)| text.as_str()), Some("是随着"), "{sentences:?}");
            let sentences = super::decode_lattice_with("shidei", 10, false, Some((6, &[], 0)));
            assert_eq!(sentences.first().map(|(text, _)| text.as_str()), Some("是得"), "{sentences:?}");
        });
    }

    #[test]
    fn whole_shadow_single_character_learning_never_changes_sentences() {
        use crate::zhixu::learning::Entry;
        zhixu::with_index(whole_learning_fixture(), || {
            let before = whole_reading_candidates("zhongjianbuyaoting", &[], 0, false);
            let entries = [Entry { reading: "ting".into(), text: "听".into(), count: 1_000_000,
                last_ms: personal_picks::now_ms(), rime: None }];
            let _learning = zhixu::pin_learning(Some(Arc::new(Index::from_learning(&entries))));
            assert_eq!(whole_reading_candidates("zhongjianbuyaoting", &entries, 0, false)
                .iter().map(|item| &item.candidate).collect::<Vec<_>>(),
                before.iter().map(|item| &item.candidate).collect::<Vec<_>>());
        });
    }

    #[test]
    fn whole_shadow_learning_bonus_is_bounded_and_respects_both_clocks() {
        use crate::zhixu::learning::{Entry, RimeWeight};
        let now = personal_picks::now_ms();
        let mut entry = Entry { reading: "rang ta".into(), text: "让它".into(), count: 1,
            last_ms: now, rime: None };
        let once = whole_learning_bonus(&entry, 1000, now);
        assert_eq!(once, 1386);
        entry.count = 2;
        assert_eq!(whole_learning_bonus(&entry, 1000, now), 2197);
        assert_eq!(whole_learning_bonus(&entry, 1000, now + 30 * 24 * 60 * 60 * 1000), once);
        entry.count = 1_000_000;
        assert_eq!(whole_learning_bonus(&entry, 1000, now), 3000);
        entry.rime = Some(RimeWeight { commits: 2, dynamic: 2.0, tick: 1000, updates: 0 });
        let recent = whole_learning_bonus(&entry, 1000, now);
        assert!((2190..=2197).contains(&recent));
        assert!(whole_learning_bonus(&entry, 2000, now) < recent, "Rime selection-clock decay is retained");
        entry.rime.as_mut().unwrap().dynamic = 1e100;
        assert_eq!(whole_learning_bonus(&entry, 1000, now), 3000);
        // Like librime's poet, a learned single character takes part too, under the same cap (搞 over 高).
        entry.text = "它".into();
        entry.reading = "ta".into();
        assert_eq!(whole_learning_bonus(&entry, 1000, now), 3000);
    }

    #[test]
    fn whole_shadow_learned_spelling_composes_with_owner_style() {
        zhixu::with_index(whole_learning_fixture(), || {
            let mut mixer = whole_mixer("worangta", &[("我让他", "wo rang ta")], true);
            mixer.configure_learning(LearningMode::Zhixu).unwrap();
            assert_eq!(mixer.snapshot().candidates[0].text, "我让他");
            let now = personal_picks::now_ms();
            mixer.learning.import_choices(vec![Choice { app: "whole-fixture".into(), raw: "rangta".into(),
                reading: "rang ta".into(), text: "让它".into(), target: 0, count: 10, last_ms: now,
                alternatives: vec!["让他".into()] }]);
            mixer.learning.remember_for_test("rang ta", "让它", now);
            assert!(mixer.learning.style().unwrap().keeps("让它", "让他"));
            assert_eq!(mixer.snapshot().candidates[0].text, "我让它");
        });
    }

    fn whole_order_fixture() -> Arc<Index> {
        use crate::zhixu::loader::{DictionaryData, Entry};
        Arc::new(Index::from_data(DictionaryData { entries: [
            ("要", "yao", 100), ("呀", "ya", 1000), ("哦", "o", 1000),
            ("天", "tian", 100), ("提案", "ti an", 1000),
            ("先", "xian", 100), ("西安", "xi an", 1000),
            ("西", "xi", 1000), ("安", "an", 1000),
            ("赏析", "shang xi", 10), ("上", "shang", 1000),
            ("赏", "shang", 900), ("系", "xi", 2000), ("戏", "xi", 900),
            ("高", "gao", 1000), ("搞", "gao", 900), ("了", "le", 1000),
        ].into_iter().map(|(text, reading, weight)| Entry {
            text: text.into(), reading: reading.into(), weight, layer: "fixture".into(),
        }).collect() }))
    }

    #[test]
    fn whole_shadow_normal_syllables_precede_extra_splits_and_remain_selectable() {
        zhixu::with_index(whole_order_fixture(), || {
            for (raw, first, split) in [("yao", "要", "呀哦"), ("tian", "天", "提案"), ("xian", "先", "西安")] {
                let values = [(first, raw), ("字", raw), ("词", raw), ("页", raw), ("选", raw)];
                let mut mixer = whole_mixer(raw, &values, true);
                let all = mixer.snapshot_all_candidates().unwrap().candidates;
                assert_eq!(all[0].text, first, "normal syllable {raw}");
                let position = all.iter().position(|candidate| candidate.text == split).unwrap();
                assert!(position >= values.len(), "{split} follows every native candidate");
                for _ in 0..position / PAGE_SIZE { mixer.page_down().unwrap(); }
                let mask = mixer.context_complete_mask() | mixer.context_group_mask();
                assert_eq!(mask & (1 << (position % PAGE_SIZE)), 0, "extra split {split} cannot be reranked");
                assert_eq!(mixer.select_candidate(position % PAGE_SIZE, 711, LearningIntent::NoLearn).unwrap(), split);
            }
        });
    }

    #[test]
    fn whole_shadow_explicit_split_is_the_normal_reading() {
        zhixu::with_index(whole_order_fixture(), || {
            let mixer = whole_mixer("xi'an", &[("西安", "xi an"), ("西", "xi")], true);
            assert_eq!(mixer.snapshot().candidates[0].text, "西安");
            assert_ne!(mixer.context_complete_mask() & 1, 0);
        });
    }

    #[test]
    fn whole_shadow_dictionary_words_send_sentences_to_the_tail_outside_the_mask() {
        zhixu::with_index(whole_order_fixture(), || {
            let mut mixer = whole_mixer("shangxi", &[("上", "shang"), ("赏", "shang")], true);
            let all = mixer.snapshot_all_candidates().unwrap().candidates;
            // Like librime: a whole word present, no sentence ahead of the native prefixes.
            assert_eq!(all.iter().take(3).map(|c| c.text.as_str()).collect::<Vec<_>>(), ["赏析", "上", "赏"]);
            let tail = all.iter().position(|c| c.text == "上系").unwrap();
            assert!(tail >= 3 && tail < 5, "{all:?}");
            let mask = mixer.context_complete_mask() | mixer.context_group_mask();
            assert_eq!(mask & (1 << tail), 0, "a sentence behind a whole word does not qualify");
            let mut order: Vec<_> = (0..mixer.snapshot().candidates.len()).collect();
            order.swap(0, tail);
            assert_eq!(mixer.reorder_context_candidates(&order), Err(-10));
        });
    }

    #[test]
    fn whole_shadow_without_whole_words_leads_with_only_the_best_sentence() {
        zhixu::with_index(whole_order_fixture(), || {
            let mixer = whole_mixer("gaole", &[("高", "gao"), ("搞", "gao")], true);
            let all = mixer.snapshot_all_candidates().unwrap().candidates;
            assert_eq!(all.iter().map(|c| c.text.as_str()).collect::<Vec<_>>(), ["高了", "高", "搞", "搞了"]);
            assert_eq!(mixer.context_complete_mask() & 0b1001, 1);
            assert_eq!(mixer.context_group_mask() & 0b1000, 0);
        });
    }

    #[test]
    fn whole_shadow_ordering_disabled_preserves_every_native_page() {
        zhixu::with_index(whole_order_fixture(), || {
            for raw in ["yao", "tian", "xian", "xi'an", "shangxi", "gaole"] {
                let values = [("上系", raw), ("赏析", raw), ("西安", raw), ("要", raw), ("天", raw), ("先", raw)];
                let (decoder, _) = DeepPagedDecoder::new(raw, &values);
                let (control, _) = DeepPagedDecoder::new(raw, &values);
                let mut native = MixerDecoder::new(Box::new(control), "whole-disabled-order".into());
                let mut mixer = MixerDecoder::new(Box::new(decoder), "whole-disabled-order".into());
                mixer.whole_override = Some(false);
                loop {
                    assert_eq!(mixer.snapshot().candidates, native.snapshot().candidates, "disabled page {raw}");
                    if native.snapshot().is_last_page { break; }
                    native.page_down().unwrap();
                    mixer.page_down().unwrap();
                }
            }
        });
    }

    #[test]
    fn whole_shadow_partial_apostrophe_boundaries_keep_whole_candidates() {
        zhixu::with_index(whole_fixture(), || {
            let mixer = whole_mixer("yi'jianle", &[("一键了", "yi jian le"), ("一", "yi")], true);
            assert_eq!(mixer.snapshot().candidates[0].text, "意见了");
            assert!(mixer.snapshot_all_candidates().unwrap().candidates.iter().any(|c| c.text == "一键了"));
            assert_ne!(mixer.context_complete_mask() & 1, 0);
        });
    }

    #[test]
    fn whole_shadow_page_groups_survive_personal_overlays_and_prefix_promotions() {
        zhixu::with_index(whole_order_fixture(), || {
            let mut split = whole_mixer("xian", &[("先", "xian")], true);
            split.apply_lexicon(lexicon(vec![entry_json("split", "西安", "xian", true, false, false, &[])]));
            assert_eq!(split.snapshot().candidates[0].text, "先");
            assert_eq!(split.context_complete_mask() & 0b10, 0);

            let mut mixer = whole_mixer("shangxi", &[("上", "shang"), ("赏", "shang")], true);
            mixer.context_model = Arc::new(ContextModel::from_json(r#"{"rules":[{"pinyin":"shangxi","senses":[{"word":"上","triggers":["test"]}]}]}"#.as_bytes()).unwrap());
            mixer.preceding_text = Some("test".into());
            assert_eq!(mixer.snapshot().candidates[0].text, "赏析", "native prefix cannot split the leading group");
        });
    }

    #[test]
    fn whole_shadow_learned_extra_split_keeps_visible_and_full_order_identical() {
        zhixu::with_index(whole_order_fixture(), || {
            let mut mixer = whole_mixer("xian", &[("先", "xian"), ("西", "xi")], true);
            mixer.configure_learning(LearningMode::Zhixu).unwrap();
            mixer.learning.remember_for_test("xi an", "西安", personal_picks::now_ms());
            let all = mixer.snapshot_all_candidates().unwrap().candidates;
            assert_eq!(mixer.snapshot().candidates, all[..all.len().min(PAGE_SIZE)]);
            assert_eq!(all.iter().take(3).map(|c| c.text.as_str()).collect::<Vec<_>>(), ["先", "西", "西安"]);
            assert_eq!(mixer.context_complete_mask() & 0b100, 0);
        });
    }

    #[test]
    fn whole_shadow_unsupported_segmentation_keeps_the_native_page() {
        zhixu::with_index(whole_order_fixture(), || {
            let raw = "yao".repeat(41);
            let text = "要".repeat(41);
            let values = [(text.as_str(), raw.as_str()), ("要", "yao")];
            let mut mixer = whole_mixer(&raw, &values, false);
            let before = mixer.snapshot();
            mixer.whole_override = Some(true);
            assert!(mixer.whole_base().is_none(), "unsupported segmentation stays native");
            assert_eq!(mixer.snapshot().candidates, before.candidates);
        });
    }

    #[test]
    fn whole_shadow_preserves_explicit_syllable_boundaries_and_context_mask() {
        assert!(whole_spelling_matches("xi'an", "xi an"));
        assert!(!whole_spelling_matches("xi'an", "xian"));
        zhixu::with_index(whole_fixture(), || {
            let mixer = whole_mixer("yi'jian", &[("意见", "yi jian"), ("一", "yi")], true);
            let all = mixer.snapshot_all_candidates().unwrap();
            assert!(all.candidates.iter().any(|c| c.text == "一键" && c.reading_key == "yi'jian" && c.source_index.is_none()));
            assert!(mixer.context_complete_mask().count_ones() >= 2);
        });
    }

    #[test]
    fn whole_shadow_respects_disabled_words() {
        zhixu::with_index(whole_fixture(), || {
            let mut mixer = whole_mixer("yijian", &[("意见", "yi jian")], true);
            mixer.apply_lexicon(lexicon(vec![entry_json("blocked-whole", "一键", "yijian", false, true, false, &[])]));
            assert!(mixer.snapshot_all_candidates().unwrap().candidates.iter().all(|c| c.text != "一键"));
        });
    }

    #[test]
    fn whole_shadow_includes_all_homophones_and_lattice_sentences() {
        zhixu::with_index(whole_fixture(), || {
            let mixer = whole_mixer("yijian", &[("意见", "yi jian"), ("一", "yi")], true);
            let all = mixer.snapshot_all_candidates().expect("shadow exposes the complete sequence");
            for text in ["一键", "一剪", "一箭", "异见"] {
                assert!(all.candidates.iter().any(|c| c.text == text), "missing {text}");
            }
            assert!(all.candidates.iter().filter(|c| c.text == "一键").all(|c| c.reading_key == "yijian" && c.source_index.is_none()));
            let mixer = whole_mixer("gaole", &[("高了", "gaole"), ("高", "gao")], true);
            let all = mixer.snapshot_all_candidates().unwrap();
            for text in ["高了", "搞了"] { assert!(all.candidates.iter().any(|c| c.text == text), "missing {text}"); }
        });
    }

    #[test]
    fn whole_shadow_disabled_and_ineligible_pages_are_unchanged() {
        for (raw, values) in [
            ("yijian", vec![("意见", "yi jian"), ("一件", "yi jian"), ("一", "yi")]),
            ("gaole", vec![("高了", "gao le"), ("搞了", "gao le")]),
            ("nihao", vec![("你好", "ni hao"), ("拟好", "ni hao")]),
        ] {
            let mut mixer = whole_mixer(raw, &values, false);
            let before = mixer.snapshot();
            assert_eq!(before.candidates, ListDecoder::with(raw, &values).snapshot().candidates, "native page {raw}");
            mixer.whole_override = None;
            assert_eq!(mixer.snapshot().candidates, before.candidates, "default {raw}");
        }
        for (raw, values) in [
            ("wmyiqi", vec![("我们一起", "wo men yi qi")]),
            ("sz", vec![("设置", "she zhi")]),
            ("yijianx", vec![("意见", "yi jian")]),
            ("mini", vec![("mini", "mini"), ("迷你", "mi ni")]),
            ("api", vec![("API", "api")]),
        ] {
            let mut mixer = whole_mixer(raw, &values, false);
            let before = mixer.snapshot();
            mixer.whole_override = Some(true);
            assert_eq!(mixer.snapshot().candidates, before.candidates, "ineligible {raw}");
            assert_eq!(mixer.snapshot().is_last_page, before.is_last_page);
        }
    }

    #[test]
    fn whole_shadow_pages_and_global_partial_selection_share_one_sequence() {
        zhixu::with_index(whole_fixture(), || {
            let (decoder, selections) = DeepPagedDecoder::new("yijian", &[
                ("意见", "yi jian"), ("一", "yi"), ("衣", "yi"), ("依", "yi"),
                ("已", "yi"), ("意", "yi"), ("亿", "yi"),
            ]);
            let mut mixer = MixerDecoder::new(Box::new(decoder), "whole-pages".into());
            mixer.whole_override = Some(true);
            let all = mixer.snapshot_all_candidates().unwrap().candidates;
            let mut walked = Vec::new();
            loop {
                let page = mixer.snapshot();
                assert_eq!(page.candidates, all[page.page_no * PAGE_SIZE..all.len().min((page.page_no + 1) * PAGE_SIZE)]);
                walked.extend(page.candidates);
                if page.is_last_page { break; }
                mixer.page_down().unwrap();
                assert!(mixer.snapshot().page_no < 10);
            }
            assert_eq!(walked, all);
            while mixer.snapshot().page_no > 1 { mixer.page_up().unwrap(); }
            let selected = mixer.snapshot().candidates[0].text.clone();
            assert_eq!(mixer.select_candidate(0, 701, LearningIntent::NoLearn).unwrap(), selected);
            assert!(selections.lock().unwrap().is_empty(), "whole selection is Local");
            let (decoder, _) = DeepPagedDecoder::new("yijian", &[("一", "yi")]);
            let mut third = MixerDecoder::new(Box::new(decoder), "whole-third-page".into());
            third.whole_override = Some(true);
            for _ in 0..PAGE_SIZE * 2 { third.select_next_linear().unwrap(); }
            assert_eq!(third.snapshot().page_no, 2);
            assert_eq!(third.snapshot().highlighted_index, 0);
            let selected = third.snapshot().candidates[0].text.clone();
            assert_eq!(third.select_candidate(0, 706, LearningIntent::NoLearn).unwrap(), selected);
        });
    }

    #[test]
    fn whole_shadow_local_selection_learns_and_joins_context_groups() {
        zhixu::with_index(whole_fixture(), || {
            let mut mixer = whole_mixer("yijian", &[("意见", "yi jian")], true);
            mixer.configure_learning(LearningMode::Zhixu).unwrap();
            assert!(mixer.context_complete_mask().count_ones() >= 2);
            assert!(mixer.context_group_mask().count_ones() >= 2);
            let position = mixer.snapshot_all_candidates().unwrap().candidates.iter().position(|c| c.text == "一键").unwrap();
            for _ in 0..position / PAGE_SIZE { mixer.page_down().unwrap(); }
            let text = mixer.select_candidate(position % PAGE_SIZE, 702, LearningIntent::DeliberateSelection).unwrap();
            assert_eq!(text, "一键");
            let now = personal_picks::now_ms();
            mixer.learning_commit(&text, 702, now);
            mixer.learning.key(&crate::state::KeyEvent { kind: crate::state::KeyKind::Text, text: "a".into(), modifiers: 0, security: crate::state::SecurityState::NonSecure, delivery_id: 703 }, false, now + 6000);
            assert!(mixer.learning.entries().iter().any(|e| e.text == "一键" && e.reading == "yi jian" && e.count == 1));
        });
    }

    fn entry_json(
        id: &str,
        text: &str,
        pinyin: &str,
        fixed: bool,
        disabled: bool,
        demoted: bool,
        domains: &[&str],
    ) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "text": text,
            "pinyin": pinyin,
            "aliases": [],
            "professional_packs": [],
            "project_domains": domains,
            "sources": [{"source_kind":"manual","source":"owner","license":"private","confidence_millis":1000}],
            "base_weight": 100,
            "fixed": fixed,
            "disabled": disabled,
            "demoted": demoted,
            "updated_at_ms": 1
        })
    }

    fn lexicon(entries: Vec<serde_json::Value>) -> Arc<MixerLexicon> {
        Arc::new(MixerLexicon::from_json(&serde_json::to_vec(&entries).unwrap()).unwrap())
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct MixerReplaySet {
        schema_version: u32,
        cases: Vec<MixerReplayCase>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct MixerReplayCase {
        id: String,
        query: String,
        app: String,
        base: Vec<MixerReplayBase>,
        entries: Vec<MixerReplayEntry>,
        expected_first: Vec<String>,
        #[serde(default)]
        absent: Vec<String>,
        #[serde(default)]
        repeat: usize,
        select_index: Option<usize>,
        expected_reopened_index: Option<usize>,
        #[serde(default)]
        typo_enabled: bool,
        #[serde(default)]
        expected_second_contains: Vec<String>,
    }

    #[derive(Deserialize)]
    struct MixerReplayBase {
        text: String,
        reading: String,
    }

    #[derive(Deserialize)]
    struct MixerReplayEntry {
        id: String,
        text: String,
        pinyin: String,
        #[serde(default)]
        fixed: bool,
        #[serde(default)]
        disabled: bool,
        #[serde(default)]
        demoted: bool,
        #[serde(default)]
        domains: Vec<String>,
    }

    #[test]
    fn frozen_hr1_through_hr8_replay_is_green() {
        let replay: MixerReplaySet = serde_json::from_str(include_str!(
            "../../../../quality/corpus/mixer-hr-replay-v1.json"
        ))
        .unwrap();
        assert_eq!(replay.schema_version, 1);
        assert_eq!(replay.cases.len(), 9);

        for case in replay.cases {
            let values = case
                .base
                .iter()
                .map(|item| (item.text.as_str(), item.reading.as_str()))
                .collect::<Vec<_>>();
            let entry_values = case
                .entries
                .iter()
                .map(|entry| {
                    let domains = entry.domains.iter().map(String::as_str).collect::<Vec<_>>();
                    entry_json(
                        &entry.id,
                        &entry.text,
                        &entry.pinyin,
                        entry.fixed,
                        entry.disabled,
                        entry.demoted,
                        &domains,
                    )
                })
                .collect::<Vec<_>>();
            let shared = lexicon(entry_values);
            let mut mixer = MixerDecoder::new(
                Box::new(ListDecoder::with(&case.query, &values)),
                case.app.clone(),
            );
            mixer.apply_lexicon(shared.clone());
            mixer.configure_typo(case.typo_enabled);
            mixer.configure_candidate_context(Some(case.app.clone()), Vec::new(), true, false);

            let first = mixer.snapshot().candidates;
            let first_text = first
                .iter()
                .map(|item| item.text.clone())
                .collect::<Vec<_>>();
            assert_eq!(first_text, case.expected_first, "{} first layer", case.id);
            for absent in &case.absent {
                assert!(
                    first.iter().all(|item| &item.text != absent),
                    "{} leaked {absent}",
                    case.id
                );
            }
            for _ in 0..case.repeat {
                assert_eq!(
                    mixer.snapshot().candidates,
                    first,
                    "{} changed order",
                    case.id
                );
            }

            if let Some(index) = case.select_index {
                mixer
                    .select_candidate(index, 1, LearningIntent::DeliberateSelection)
                    .unwrap();
                let mut reopened = MixerDecoder::new(
                    Box::new(ListDecoder::with(&case.query, &values)),
                    case.app.clone(),
                );
                reopened.apply_lexicon(shared);
                reopened.configure_candidate_context(
                    Some(case.app.clone()),
                    Vec::new(),
                    true,
                    false,
                );
                let reopened_index = reopened
                    .snapshot()
                    .candidates
                    .iter()
                    .position(|item| item.text == case.base[index].text);
                assert_eq!(
                    reopened_index, case.expected_reopened_index,
                    "{} reopen",
                    case.id
                );
            }

            if !case.expected_second_contains.is_empty() {
                let second = mixer.snapshot();
                for expected in &case.expected_second_contains {
                    assert!(
                        second
                            .secondary_candidates
                            .iter()
                            .any(|item| &item.text == expected),
                        "{} second layer missing {expected}",
                        case.id
                    );
                }
            }
        }
    }

    #[test]
    fn typo_distance_covers_transpose_missing_extra_and_substitution() {
        assert_eq!(damerau_levenshtein_one("nhi", "nih"), Some(1));
        assert_eq!(damerau_levenshtein_one("niha", "nihao"), Some(1));
        assert_eq!(damerau_levenshtein_one("nihhao", "nihao"), Some(1));
        assert_eq!(damerau_levenshtein_one("nihap", "nihao"), Some(1));
        assert_eq!(damerau_levenshtein_one("wrong", "nihao"), None);
    }

    #[test]
    fn hr1_blocked_text_is_removed_from_every_source() {
        let base = ListDecoder::with("ni", &[("你", "ni"), ("泥", "ni")]);
        let mut mixer = MixerDecoder::new(Box::new(base), "app".into());
        mixer.apply_lexicon(lexicon(vec![entry_json(
            "x",
            "你",
            "ni",
            false,
            true,
            false,
            &[],
        )]));
        assert_eq!(
            mixer
                .snapshot()
                .candidates
                .iter()
                .map(|c| c.text.as_str())
                .collect::<Vec<_>>(),
            ["泥"]
        );
    }

    #[test]
    fn hr2_fixed_prefix_is_tier_zero_and_hr3_deduplicates_sources() {
        let base = ListDecoder::with("ni", &[("你", "ni"), ("泥", "ni")]);
        let mut mixer = MixerDecoder::new(Box::new(base), "app".into());
        mixer.apply_lexicon(lexicon(vec![
            entry_json("fixed", "你好", "nihao", true, false, false, &[]),
            entry_json("dup", "你", "ni", false, false, false, &[]),
        ]));
        let values = mixer.snapshot().candidates;
        assert_eq!(values[0].text, "你好");
        assert_eq!(values.iter().filter(|c| c.text == "你").count(), 1);
    }

    #[test]
    fn hr4_empty_personalization_is_byte_for_byte_base_and_hr5_is_deterministic() {
        let values = [("你", "ni"), ("呢", "ni"), ("泥", "ni")];
        let mixer = MixerDecoder::new(Box::new(ListDecoder::with("ni", &values)), "hr4-app".into());
        let expected = values.iter().map(|(text, _)| *text).collect::<Vec<_>>();
        for _ in 0..20 {
            assert_eq!(
                mixer
                    .snapshot()
                    .candidates
                    .iter()
                    .map(|c| c.text.as_str())
                    .collect::<Vec<_>>(),
                expected
            );
        }
    }

    #[test]
    fn host_context_order_preserves_slots_selection_and_learning_text() {
        for (slot, expected, intent) in [
            (0, "再", LearningIntent::NoLearn),
            (2, "在", LearningIntent::DeliberateSelection),
        ] {
            let base = ListDecoder::with(
                "zai",
                &[
                    ("在", "zai"),
                    ("ASCII", "zai"),
                    ("再", "zai"),
                    ("才", "cai"),
                ],
            );
            let delivery_ids = base.delivery_ids.clone();
            let mut mixer = MixerDecoder::new(Box::new(base), format!("host-context-{slot}"));
            assert_eq!(mixer.context_group_mask(), 0b0101);
            mixer.reorder_context_candidates(&[2, 1, 0, 3]).unwrap();
            assert_eq!(
                mixer
                    .snapshot()
                    .candidates
                    .iter()
                    .map(|c| c.text.as_str())
                    .collect::<Vec<_>>(),
                ["再", "ASCII", "在", "才"]
            );
            assert_eq!(mixer.select_candidate(slot, 912, intent).unwrap(), expected);
            assert_eq!(*delivery_ids.lock().unwrap(), vec![(912, intent)]);
            if intent == LearningIntent::DeliberateSelection {
                assert_eq!(
                    shared_learning().lock().unwrap().target_positions.get(&(
                        format!("host-context-{slot}"),
                        "zai".into(),
                        expected.into()
                    )),
                    Some(&slot.saturating_sub(MAX_SELECTION_PROMOTION))
                );
                assert!(
                    !shared_learning()
                        .lock()
                        .unwrap()
                        .target_positions
                        .contains_key(&(format!("host-context-{slot}"), "zai".into(), "再".into()))
                );
            }
        }
    }

    #[test]
    fn complete_group_admits_abbreviated_full_readings_and_promotion_only() {
        let mut mixer = MixerDecoder::new(
            Box::new(ListDecoder::with(
                "hais",
                &[("还剩", "hai sheng"), ("还是", "hai shi"), ("海上", "hai shang"), ("还", "hai")],
            )),
            "host-complete".into(),
        );
        // 还 covers only "hai", so it stays outside; the homophone group is empty here
        // because hais is not a full reading.
        assert_eq!(mixer.context_complete_mask(), 0b0111);
        assert_eq!(mixer.context_group_mask(), 0);
        mixer.reorder_context_candidates(&[1, 0, 2, 3]).unwrap();
        assert_eq!(
            mixer.snapshot().candidates.iter().map(|c| c.text.as_str()).collect::<Vec<_>>(),
            ["还是", "还剩", "海上", "还"]
        );
        assert!(mixer.reorder_context_candidates(&[3, 1, 2, 0]).is_err());
    }

    #[test]
    fn host_context_rejects_invalid_permutations_atomically() {
        let mut mixer = MixerDecoder::new(
            Box::new(ListDecoder::with(
                "zai",
                &[("在", "zai"), ("ASCII", "zai"), ("再", "zai")],
            )),
            "host-atomic".into(),
        );
        mixer.reorder_context_candidates(&[2, 1, 0]).unwrap();
        for order in [vec![1, 0, 2], vec![2, 1, 2], vec![2, 1], vec![9, 1, 0]] {
            assert!(mixer.reorder_context_candidates(&order).is_err());
            assert_eq!(mixer.snapshot().candidates[0].text, "再");
        }
    }

    #[test]
    fn host_context_order_expires_on_input_and_paging() {
        let mut mixer = MixerDecoder::new(
            Box::new(ListDecoder::with("zai", &[("在", "zai"), ("再", "zai")])),
            "host-invalidate".into(),
        );
        mixer.reorder_context_candidates(&[1, 0]).unwrap();
        mixer.select_next().unwrap();
        assert_eq!(mixer.snapshot().candidates[1].text, "在");
        mixer.page_up().unwrap();
        assert_eq!(mixer.snapshot().candidates[0].text, "在");
        mixer.reorder_context_candidates(&[1, 0]).unwrap();
        mixer.input("n").unwrap();
        assert_eq!(mixer.snapshot().candidates[0].text, "在");
    }

    #[test]
    fn native_editing_keeps_host_candidate_order_during_caret_movement() {
        let mut decoder = ListDecoder::with("zai", &[("在", "zai"), ("再", "zai")]);
        decoder.exact_boundary = Some(CompositionBoundary { confirmed: String::new(), unconfirmed_raw: "zai".into() });
        let mut mixer = MixerDecoder::new(Box::new(decoder), "native-host-edit".into());
        mixer.reorder_context_candidates(&[1, 0]).unwrap();
        let shown = mixer.snapshot().candidates;
        mixer.move_left().unwrap();
        assert_eq!(mixer.snapshot().candidates, shown);
        mixer.move_right().unwrap();
        assert_eq!(mixer.snapshot().candidates, shown);
        assert_eq!(mixer.select_candidate(0, 4, LearningIntent::NoLearn).unwrap(), "再");
    }

    #[test]
    fn native_page_column_restoration_uses_inner_highlight_after_filtering() {
        let mut decoder = ListDecoder::with("ni", &[("你", "ni"), ("你", "ni"), ("尼", "ni"), ("拟", "ni"), ("你", "ni")]);
        decoder.exact_boundary = Some(CompositionBoundary { confirmed: String::new(), unconfirmed_raw: "ni".into() });
        decoder.highlighted = 4;
        let mut mixer = MixerDecoder::new(Box::new(decoder), "native-filtered-column".into());
        assert_eq!(mixer.snapshot().candidates.len(), 3);
        assert_eq!(mixer.snapshot().highlighted_index, 2);
        mixer.keep_native_page_column(Some(1)).unwrap();
        assert_eq!(mixer.snapshot().highlighted_index, 1);
        assert_eq!(mixer.inner.snapshot().highlighted_index, 1);
    }

    #[test]
    fn host_context_excludes_abbreviations_partial_english_and_unverified_readings() {
        for (raw, values) in [
            ("z", vec![("在", "zai"), ("再", "zai")]),
            ("zai", vec![("ASCII", "zai"), ("在", "zai")]),
            (
                "zai",
                vec![("在", "zai"), ("在家", "zai jia"), ("才", "zai")],
            ),
        ] {
            let mut mixer = MixerDecoder::new(
                Box::new(ListDecoder::with(raw, &values)),
                "host-exclude".into(),
            );
            assert!(mixer.context_group_mask().count_ones() < 2);
            assert!(mixer.reorder_context_candidates(&[1, 0]).is_err());
        }
    }

    #[test]
    fn context_model_reorders_only_existing_candidates_and_preserves_selection_mapping() {
        let values = [
            ("候选一", "ceshi"),
            ("候选二", "ceshi"),
            ("候选三", "ceshi"),
        ];
        let mut mixer = MixerDecoder::new(
            Box::new(ListDecoder::with("ceshi", &values)),
            "context-model".into(),
        );
        let model = ContextModel::from_json(
            r#"{"rules":[{"pinyin":"ceshi","senses":[{"word":"候选二","triggers":["前文"]}]}]}"#
                .as_bytes(),
        )
        .unwrap();
        mixer.apply_context_model(Arc::new(model));

        assert_eq!(
            mixer
                .snapshot()
                .candidates
                .iter()
                .map(|candidate| candidate.text.as_str())
                .collect::<Vec<_>>(),
            ["候选一", "候选二", "候选三"]
        );
        mixer.configure_preceding_text(Some("无关前缀前文".into()));
        assert_eq!(
            mixer
                .snapshot()
                .candidates
                .iter()
                .map(|candidate| candidate.text.as_str())
                .collect::<Vec<_>>(),
            ["候选二", "候选一", "候选三"]
        );
        assert_eq!(
            mixer
                .select_candidate(0, 0, LearningIntent::NoLearn)
                .unwrap(),
            "候选二"
        );
    }

    #[test]
    fn unmatched_or_removed_context_model_degrades_to_exact_base_order() {
        let values = [("候选一", "ceshi"), ("候选二", "ceshi")];
        let mut mixer = MixerDecoder::new(
            Box::new(ListDecoder::with("ceshi", &values)),
            "context-fallback".into(),
        );
        mixer.configure_preceding_text(Some("没有模型证据".into()));
        assert_eq!(
            mixer
                .snapshot()
                .candidates
                .iter()
                .map(|candidate| candidate.text.as_str())
                .collect::<Vec<_>>(),
            ["候选一", "候选二"]
        );
    }

    #[test]
    fn nonempty_input_never_exposes_an_empty_first_candidate() {
        let mixer = MixerDecoder::new(
            Box::new(ListDecoder::with(
                "zan",
                &[("", "zan"), ("   ", "zan"), ("咱", "zan"), ("攒", "zan")],
            )),
            "visible-slot-invariant".into(),
        );
        let snapshot = mixer.snapshot();
        assert_eq!(
            snapshot
                .candidates
                .iter()
                .map(|candidate| candidate.text.as_str())
                .collect::<Vec<_>>(),
            ["咱", "攒"]
        );
        assert_eq!(snapshot.highlighted_index, 0);
        assert!(
            !snapshot.candidates[snapshot.highlighted_index]
                .text
                .trim()
                .is_empty()
        );
    }

    #[test]
    fn fuzzy_pool_stably_prioritizes_exact_readings_without_hiding_expansions() {
        let base = ListDecoder::with(
            "zan",
            &[
                ("张", "zhang"),
                ("站", "zhan"),
                ("咱", "zan"),
                ("占", "zhan"),
                ("攒", "zan"),
                ("涨", "zhang"),
                ("暂", "zan"),
            ],
        );
        let mut mixer = MixerDecoder::new(Box::new(base), "fuzzy-exact-priority".into());
        mixer.configure_fuzzy(1).unwrap();
        let first = mixer.snapshot();
        assert_eq!(
            first
                .candidates
                .iter()
                .map(|candidate| candidate.text.as_str())
                .collect::<Vec<_>>(),
            ["咱", "攒", "暂", "张", "站"]
        );
        assert!(!first.is_last_page);
        assert_eq!(
            mixer
                .select_candidate(0, 1, LearningIntent::DeliberateSelection)
                .unwrap(),
            "咱"
        );
    }

    #[test]
    fn coherent_nonexact_readings_beat_abbreviation_assembly_without_disabling_initials() {
        for (raw, desired, reading, garbage) in [
            (
                "dansh",
                "但是",
                "danshi",
                [
                    ("到奥你说有", "daoaonishuoyou"),
                    ("到奥你说我该", "daoaonishuowogai"),
                ],
            ),
            (
                "gengjai",
                "更加",
                "gengjia",
                [("根宫颈癌", "gengongjingai"), ("更国家爱", "gengguojiaai")],
            ),
            (
                "zhem",
                "这么",
                "zheme",
                [("这和没", "zhehemei"), ("这会吗", "zhehuima")],
            ),
            (
                "zenm",
                "怎么",
                "zenme",
                [("怎你们", "zennimen"), ("在那吗", "zainanma")],
            ),
        ] {
            let values = [garbage[0], garbage[1], (desired, reading)];
            let mixer = MixerDecoder::new(Box::new(ListDecoder::with(raw, &values)), raw.into());
            let snapshot = mixer.snapshot();
            assert_eq!(snapshot.candidates[0].text, desired, "raw={raw}");
        }

        let exact = MixerDecoder::new(
            Box::new(ListDecoder::with(
                "zan",
                &[("咱", "zan"), ("赞", "zan"), ("站", "zhan")],
            )),
            "exact-order-stays-rime".into(),
        );
        assert_eq!(
            exact
                .snapshot()
                .candidates
                .iter()
                .map(|candidate| candidate.text.as_str())
                .collect::<Vec<_>>(),
            ["咱", "赞", "站"]
        );

        for (raw, reading) in [
            ("luanqbz", "luan qi ba zao"),
            ("lqbz", "luan qi ba zao"),
            ("bcgt", "bai chi gan tou"),
            ("wangybl", "wang yang bu lao"),
            ("zhonggr", "zhong guo ren"),
            ("ra", "re ai"),
        ] {
            assert!(
                plausible_nonexact_reading(raw, reading),
                "syllable abbreviation should be plausible: raw={raw} reading={reading}"
            );
        }
        assert!(!syllable_prefix_abbreviation("mariadb", "mariadb"));
    }

    #[test]
    fn owner_verified_raw_hit_commits_complete_text_locally_without_generalizing() {
        assert_eq!(
            owner_verified_hit_for_raw("dakle"),
            Some(("打开了", "da kai le"))
        );
        assert_eq!(owner_verified_hit_for_raw("dkl"), None);

        let base = ListDecoder::with(
            "dakle",
            &[
                ("dakle", "dakle"),
                ("大快了", "da kuai le"),
                ("打开", "da kai"),
            ],
        );
        let mut mixer = MixerDecoder::new(Box::new(base), "owner-dakle".into());
        assert_eq!(mixer.snapshot().candidates[0].text, "打开了");
        assert_eq!(
            mixer
                .select_candidate(0, 7, LearningIntent::NoLearn)
                .unwrap(),
            "打开了"
        );
        assert!(mixer.snapshot().raw_input.is_empty());
    }

    #[test]
    fn long_dictionary_phrases_allow_one_shortened_syllable_without_promoting_garble() {
        for (raw, text) in [
            ("worenwi", "我认为"),
            ("womxianchulizhezhong", "我们先处理这种"),
            ("zhgetiyan", "这个体验"),
            ("danshwojide", "但是我记得"),
            ("womdouzhidao", "我们都知道"),
            ("womyiqi", "我们一起"),
            ("womxuyao", "我们需要"),
            ("womkeyi", "我们可以"),
            ("rizhiqingk", "日志情况"),
        ] {
            assert!(
                dictionary_phrase_matches_raw(raw, text),
                "dictionary phrase should match: raw={raw} text={text}"
            );
            let candidate = MixedCandidate {
                candidate: Candidate {
                    text: text.into(),
                    comment: String::new(),
                    reading_key: raw.into(),
                    source_index: Some(0),
                },
                selection: Selection::Base {
                    index: 0,
                    text: text.into(),
                },
                tier: 2,
                score: 0,
            };
            assert!(!should_promote_raw_ascii(raw, &[candidate]), "raw={raw}");
        }

        assert!(!dictionary_phrase_matches_raw("wmyiqi", "我们一起"));
        assert!(!dictionary_phrase_matches_raw("wom", "我们"));
        for (raw, text) in [("mariadb", "吗日啊代表"), ("react", "热爱传统")] {
            assert!(
                !dictionary_phrase_matches_raw(raw, text),
                "assembled Chinese should not match: raw={raw} text={text}"
            );
            let candidate = MixedCandidate {
                candidate: Candidate {
                    text: text.into(),
                    comment: String::new(),
                    reading_key: raw.into(),
                    source_index: Some(0),
                },
                selection: Selection::Base {
                    index: 0,
                    text: text.into(),
                },
                tier: 2,
                score: 0,
            };
            assert!(should_promote_raw_ascii(raw, &[candidate]), "raw={raw}");
        }
    }

    #[test]
    fn complete_dictionary_phrase_owns_only_top1_over_partial_prefixes() {
        fn mixed(text: &str, reading: &str, index: usize) -> MixedCandidate {
            MixedCandidate {
                candidate: Candidate {
                    text: text.into(),
                    comment: String::new(),
                    reading_key: reading.into(),
                    source_index: Some(index),
                },
                selection: Selection::Base {
                    index,
                    text: text.into(),
                },
                tier: 2,
                score: -(index as i64),
            }
        }
        for (raw, desired, values) in [
            (
                "jiukyi",
                "就可以",
                vec![
                    mixed("就开", "jiu kai", 0),
                    mixed("就可以", "jiu ke yi", 1),
                    mixed("就可", "jiu ke", 2),
                ],
            ),
            (
                "jiukyizuow",
                "就可以做完",
                vec![
                    mixed("就可一座", "jiu ke yi zuo", 0),
                    mixed("即可一座", "ji ke yi zuo", 1),
                    mixed("就可以做完", "jiu ke yi zuo wan", 2),
                    mixed("就可以", "jiu ke yi", 3),
                ],
            ),
        ] {
            let before = values
                .iter()
                .map(|item| item.candidate.text.clone())
                .collect::<HashSet<_>>();
            let mut after = values;
            promote_complete_dictionary_candidate_to_top(raw, false, &mut after);
            assert_eq!(after[0].candidate.text, desired, "raw={raw}");
            assert_eq!(
                after
                    .iter()
                    .map(|item| item.candidate.text.clone())
                    .collect::<HashSet<_>>(),
                before,
                "candidate set changed for raw={raw}"
            );
        }

        let mut full_spelling = vec![
            mixed("就可以做完", "jiu ke yi zuo wan", 0),
            mixed("就可以", "jiu ke yi", 1),
        ];
        let unchanged = full_spelling
            .iter()
            .map(|item| item.candidate.text.clone())
            .collect::<Vec<_>>();
        promote_complete_dictionary_candidate_to_top("jiukeyizuowan", false, &mut full_spelling);
        assert_eq!(
            full_spelling
                .iter()
                .map(|item| item.candidate.text.clone())
                .collect::<Vec<_>>(),
            unchanged
        );

        let mut already_complete = vec![
            mixed("百尺竿头", "bai chi gan tou", 0),
            mixed("百尺竿头更进一步", "bai chi gan tou geng jin yi bu", 1),
        ];
        promote_complete_dictionary_candidate_to_top("bcgt", false, &mut already_complete);
        assert_eq!(already_complete[0].candidate.text, "百尺竿头");

        let mut short = vec![mixed("就开", "jiu kai", 0), mixed("就可", "jiu ke", 1)];
        promote_complete_dictionary_candidate_to_top("jiuk", false, &mut short);
        assert_eq!(short[0].candidate.text, "就开");

        let mut garbage_only = vec![mixed("就开", "jiu kai", 0), mixed("我们一起", "wmyiqi", 1)];
        promote_complete_dictionary_candidate_to_top("wmyiqi", false, &mut garbage_only);
        assert_eq!(garbage_only[0].candidate.text, "就开");

        for raw in ["redis", "npm", "docker"] {
            let mut registered = vec![mixed(raw, raw, 0), mixed("我们一起", "wo men yi qi", 1)];
            promote_complete_dictionary_candidate_to_top(raw, false, &mut registered);
            assert_eq!(registered[0].candidate.text, raw);
        }
    }

    #[test]
    fn registered_ascii_exact_blocks_only_nonexact_chinese_promotion() {
        assert_eq!(LONG_PHRASE_MAX_NONEXACT_SYLLABLES, 1);
        assert!(has_registered_ascii_exact("token", false));
        assert!(has_registered_ascii_exact("redis", false));
        assert!(!has_registered_ascii_exact("key", false));
        assert!(has_registered_ascii_exact("key", true));

        let worenwi = MixerDecoder::new(
            Box::new(ListDecoder::with("worenwi", &[("我认为", "worenwi")])),
            "three-syllable-abbreviation".into(),
        );
        assert_eq!(worenwi.snapshot().candidates[0].text, "我认为");

        let redis = MixerDecoder::new(
            Box::new(ListDecoder::with("redis", &[("热第三", "redis")])),
            "registered-english-exact".into(),
        );
        assert_eq!(redis.snapshot().candidates[0].text, "Redis");

        let exact_chinese = MixerDecoder::new(
            Box::new(ListDecoder::with("ci", &[("次", "ci")])),
            "exact-chinese-control".into(),
        );
        assert_eq!(exact_chinese.snapshot().candidates[0].text, "次");
    }

    #[test]
    fn correction_group_keeps_the_transposed_reading_without_a_model() {
        let mixer = MixerDecoder::new(
            Box::new(ListDecoder::with("yijignshile", &[("以及功能湿了", "yi ji gong neng shi le")])),
            "correction-group".into(),
        );
        let first = mixer.snapshot().candidates[0].text.clone();
        let alternatives = mixer.composition_alternatives();
        let texts: Vec<_> = std::iter::once(first).chain(alternatives).collect();
        assert!(texts.iter().any(|text| text == "已经是了"), "{texts:?}");
        let desired = texts.iter().position(|text| text == "已经是了").unwrap();
        assert!(desired < 5, "{texts:?}");
    }

    #[test]
    fn trailing_letter_correction_is_on_the_first_page() {
        let mixer = MixerDecoder::new(
            Box::new(ListDecoder::with("xianzo", &[("先走", "xian zou"), ("西安", "xi an")])),
            "trailing-correction".into(),
        );
        let page = mixer.snapshot();
        assert!(page.candidates.iter().take(5).any(|candidate| candidate.text == "先做"), "{:?}", page.candidates);
        assert_eq!(page.candidates[0].text, "先走");
    }

    fn mixed(text: &str, comment: &str, local: bool) -> MixedCandidate {
        let selection = if local { Selection::Local { text: text.into() } } else { Selection::Base { index: 0, text: text.into() } };
        MixedCandidate { candidate: Candidate { text: text.into(), comment: comment.into(), reading_key: String::new(), source_index: None },
            selection, tier: 0, score: 0 }
    }

    /// 11-07 replay (0.3% of pages): the heavy-garble copy and the decoder's own copy of the same text.
    #[test]
    fn a_page_shows_each_text_once_and_the_decoders_candidate_keeps_the_row() {
        let mut page = vec![
            mixed("执政", "重度乱拼 · 本地索引", true),
            mixed("指针那个", "重度乱拼 · 本地索引", true),
            mixed("指针那个", "", false),
            mixed("指针你", "尾字母 · 本地索引", true),
        ];
        one_row_per_text(&mut page);
        let texts: Vec<_> = page.iter().map(|m| m.candidate.text.as_str()).collect();
        assert_eq!(texts, ["执政", "指针那个", "指针你"]);
        assert!(matches!(page[1].selection, Selection::Base { .. }), "the decoder's selection path (it teaches the dictionary) is kept");
        assert_eq!(page[1].candidate.comment, "");
        // The decoder's copy first, or two ordinary copies: the first row stays.
        let mut page = vec![mixed("甲", "", false), mixed("甲", "重度乱拼 · 本地索引", true), mixed("乙", "", false), mixed("乙", "", false)];
        one_row_per_text(&mut page);
        assert_eq!(page.iter().map(|m| m.candidate.text.as_str()).collect::<Vec<_>>(), ["甲", "乙"]);
        assert!(matches!(page[0].selection, Selection::Base { .. }));
    }

    /// A composition that `lead_with_composition` accepts must really lead the page: the owner's
    /// learned whole (八九本 for bajiuben) once outranked the model's 把剧本 in `visible()` while the
    /// call still said Ok, so the shell showed 把剧本 first and digit 2 committed it (11-07).
    #[test]
    fn an_accepted_composition_lead_really_leads_when_a_learned_whole_competes() {
        zhixu::with_index(zhixu::fallback(), || {
            let mut mixer = MixerDecoder::new(
                Box::new(ListDecoder::with("bajiuben", &[("八九本", "ba jiu ben"), ("八九", "ba jiu"), ("把酒", "ba jiu")])),
                "lead-vs-learned-whole".into());
            mixer.typo_enabled = true;
            mixer.configure_learning(LearningMode::Zhixu).unwrap();
            mixer.learning.remember_for_test("ba jiu ben", "八九本", personal_picks::now_ms());
            let alternatives = mixer.composition_alternatives();
            let mut refused = 0;
            for text in alternatives {
                let outcome = mixer.lead_with_composition(&text);
                let page = mixer.snapshot().candidates;
                if outcome.is_ok() {
                    assert_eq!(page[0].text, text, "accepted lead must be first");
                } else {
                    refused += 1;
                    assert_eq!(outcome, Err(LEAD_OUTRANKED_BY_LEARNED_WHOLE), "the refusal names the learned whole");
                    assert_eq!(page[0].text, "八九本", "a refused lead leaves the learned whole first");
                }
            }
            assert!(refused > 0, "the learned whole must have refused at least one lead");
        });
    }

    #[test]
    fn correction_switches_restore_their_baseline_paths_and_protect_abbreviations() {
        use ranking::{CorrectionSettings, with_correction_settings};
        let off = CorrectionSettings { enabled: false, ..Default::default() };
        let capture = |raw: &str, text: &str, full: &str| {
            let mut mixer = MixerDecoder::new(Box::new(ListDecoder::with(raw, &[(text, full)])), "correction-switches".into());
            mixer.typo_enabled = true;
            let page = mixer.snapshot();
            (page.candidates, mixer.composition_alternatives(), mixer.composition_edits())
        };
        for (raw, text, full) in [
            ("shenm", "什么", "shen me"), ("kehud", "客户端", "ke hu duan"),
            ("woganj", "我感觉", "wo gan jue"), ("maidia", "麦蒂啊", "mai di a"),
            ("sd", "是的", "shi de"), ("nikk", "你看看", "ni kan kan"),
        ] {
            let before = with_correction_settings(off, || capture(raw, text, full));
            let after = with_correction_settings(CorrectionSettings::default(), || capture(raw, text, full));
            assert_eq!(after.0[0], before.0[0], "{raw}");
            assert!(correction_readings(raw).is_empty(), "{raw} must keep its original interpretation");
            eprintln!("CORRECTION_PROTECTED raw={raw} before={} after={}", before.0[0].text, after.0[0].text);
        }
        // Each feature alone, then switched off, yields the baseline of its path.
        let disabled = CorrectionSettings { enabled: true, typed_costs: false, multiple_readings: false, trailing_letter: false };
        for raw in ["yijignshile", "xianzo", "gengixn", "houmioan"] {
            let before = with_correction_settings(off, || capture(raw, "测试", raw));
            assert_eq!(with_correction_settings(disabled, || capture(raw, "测试", raw)), before, "{raw}");
            with_correction_settings(off, || assert!(correction_readings(raw).is_empty()));
        }
        let flat = CorrectionSettings { typed_costs: false, ..Default::default() };
        with_correction_settings(flat, || {
            for kind in [EditKind::Transposition, EditKind::AdjacentKey, EditKind::MissingLetter,
                EditKind::ExtraLetter, EditKind::OtherSubstitution] { assert_eq!(edit_units(kind), 1.0); }
            assert_eq!(decode_heavy_garble_with_edits("gengixn"),
                with_correction_settings(off, || decode_heavy_garble_with_edits("gengixn")));
        });
        with_correction_settings(CorrectionSettings { multiple_readings: false, ..Default::default() }, || {
            assert!(correction_readings("yijignshile").is_empty());
            assert!(!correction_readings("xianzo").is_empty(), "tail switch is independent");
        });
        with_correction_settings(CorrectionSettings { trailing_letter: false, ..Default::default() }, || {
            assert!(trailing_letter_reading("xianzo").is_none());
            assert_eq!(capture("xianzo", "先走", "xian zou").0,
                with_correction_settings(off, || capture("xianzo", "先走", "xian zou").0));
            assert!(!correction_readings("yijignshile").is_empty());
        });
    }

    #[test]
    fn corrections_keep_heavy_readings_costs_and_the_empty_page_contract() {
        let raw = "yijignshile";
        let heavy = heavy_correction_readings(raw);
        assert!(!heavy.is_empty());
        assert_eq!(heavy.len(), heavy.iter().map(|repair| &repair.reading).collect::<HashSet<_>>().len());
        let repairs = correction_readings(raw);
        for heavy in &heavy {
            assert!(repairs.iter().any(|repair| repair.reading == heavy.reading), "{} missing", heavy.reading);
        }
        let correct = repairs.iter().find(|repair| repair.texts.iter().any(|text| text == "已经是了")).unwrap();
        // Which heavy repairs appear is the lexicon's call (cap three: 已经死了 once, 已经是了 once 是了 became a word);
        // what must hold is that a heavier spelling than the literal swap is among them.
        assert!(heavy.iter().any(|repair| correct.edits < repair.edits), "literal swap must cost less than swap + stray h");
        let swap = ranking::typed_repair_cost("yijignshile", "yijingshile").unwrap();
        assert!((correct.edits * ranking::edit_penalty() - swap).abs() < 1e-9);
        for raw in ["yijignshile", "xianzo"] {
            let mixer = MixerDecoder::new(Box::new(ListDecoder::with(raw, &[])), "correction-empty".into());
            assert!(mixer.snapshot().candidates.is_empty());
            assert!(mixer.composition_alternatives().is_empty());
        }
    }

    #[test]
    fn trailing_corrections_never_replace_an_existing_slip_or_particle_first() {
        let mut mixer = MixerDecoder::new(Box::new(ListDecoder::with("xianzo", &[
            ("先走", "xian zou"), ("西安", "xi an"), ("县", "xian"), ("先", "xian"), ("鲜", "xian"),
        ])), "tail-slip-order".into());
        mixer.typo_enabled = true;
        assert_eq!(mixer.snapshot().candidates[0].text, "先走");
        assert!(mixer.snapshot().candidates.iter().take(5).any(|candidate| candidate.text == "先做"));
        assert_eq!(trailing_letter_reading("woganj"), None);
        assert_eq!(trailing_letter_reading("maidia"), None);
        // A genuine extra final vowel after an otherwise complete spelling.
        assert_eq!(trailing_letter_reading("xianzuov"), Some("xianzuo"));
        assert!(tail_compositions("xianzuov", "xianzuo").contains(&"先做".to_string()));
    }

    #[test]
    fn trailing_corrections_obey_the_reading_blacklist() {
        let mut mixer = MixerDecoder::new(Box::new(ListDecoder::with("xianzo", &[("先走", "xian zou")])), "tail-blacklist".into());
        let blacklist = Arc::new(Mutex::new(Store::default()));
        blacklist.lock().unwrap().insert("xianzo", "先做").unwrap();
        mixer.apply_blacklist(blacklist);
        assert!(!mixer.snapshot().candidates.iter().any(|candidate| candidate.text == "先做"));
        assert!(!mixer.composition_alternatives().iter().any(|text| text == "先做"));
    }

    #[test]
    fn heavy_correction_reading_matches_the_selected_polyphonic_terminal() {
        use zhixu::loader::{DictionaryData, Entry};
        for (text, low, high) in [("熟", "shou", "shu"), ("数", "sha", "shu")] {
            let index = Arc::new(Index::from_data(DictionaryData { entries: vec![
                Entry { text: text.into(), reading: low.into(), weight: 10_000, layer: "test".into() },
                Entry { text: text.into(), reading: high.into(), weight: 100_000, layer: "test".into() },
                Entry { text: "了".into(), reading: "le".into(), weight: 100_000, layer: "test".into() },
            ] }));
            zhixu::with_index(index, || {
                let repairs = heavy_correction_readings("shl");
                let target = format!("{text}了");
                let repair = repairs.iter().find(|repair| repair.texts.contains(&target)).unwrap();
                assert_eq!(repair.reading, format!("{high}le"));
            });
        }
    }

    #[test]
    fn heavy_garble_uses_bounded_indexed_edits_and_the_registered_ascii_gate() {
        assert_eq!(HEAVY_GARBLE_BEAM_PER_POSITION, 8);
        assert_eq!(HEAVY_GARBLE_MAX_WORDS, 5);
        assert_eq!(HEAVY_GARBLE_MAX_TOTAL_EDITS, 2);
        assert_eq!(HEAVY_GARBLE_MAX_WORD_EDITS, 1);
        assert_eq!(HEAVY_GARBLE_MAX_CANDIDATES, 2);

        let transposed = decode_heavy_garble("gengixn");
        assert!(transposed.iter().any(|text| text == "更新"));
        let deleted = decode_heavy_garble("gengxxin");
        assert!(deleted.iter().any(|text| text == "更新"));

        for raw in [
            "npm", "kafka", "safari", "bun", "deno", "ansible", "nginx", "grafana", "webpack",
            "vite", "mariadb", "docker",
        ] {
            assert!(has_registered_ascii_exact(raw, false), "raw={raw}");
            assert!(
                heavy_garble_candidates(raw, &[], false).is_empty(),
                "registered ASCII leaked generated Chinese: raw={raw}"
            );
        }
    }

    #[test]
    fn traditional_and_variant_characters_never_join_a_model_group_below_the_first() {
        assert!(common_simplified("哟") && common_simplified("有意思") && common_simplified("Codex"));
        assert!(!common_simplified("喲") && !common_simplified("來"));
        let mut mixer = MixerDecoder::new(
            Box::new(ListDecoder::with("yo", &[("哟", "yo"), ("喲", "yo"), ("唷", "yo")])),
            "variants".into(),
        );
        mixer.configure_preceding_text(Some("那".into()));
        let mask = mixer.context_group_mask();
        let texts = mixer.snapshot().candidates.iter().map(|c| c.text.clone()).collect::<Vec<_>>();
        let variant = texts.iter().position(|t| t == "喲").unwrap();
        assert_eq!(mask & (1 << variant), 0, "{texts:?} mask={mask:b}");
        assert_ne!(mask & 1, 0);
    }

    #[test]
    fn letters_that_spell_several_words_alike_group_them() {
        assert_eq!(typed_spellings("hais", &["hai", "shi"]), vec![vec![true, false]]);
        assert_eq!(typed_spellings("hais", &["hai", "sheng"]), vec![vec![true, false]]);
        assert_eq!(typed_spellings("cheji", &["che", "ji"]), vec![vec![true, true]]);
        assert_eq!(typed_spellings("cheji", &["chen", "ji"]), vec![vec![false, true]]);
        assert!(typed_spellings("hais", &["hao", "shi"]).is_empty());
        let item = |text: &str, reading: &str| MixedCandidate {
            candidate: Candidate { text: text.into(), comment: String::new(), reading_key: reading.into(), source_index: None },
            selection: Selection::Base { index: 0, text: text.into() },
            tier: 0,
            score: 0,
        };
        let page = [item("还剩", "hai sheng"), item("还是", "hai shi"), item("还说", "hai shuo"), item("海上", "hai shang"), item("还", "hai")];
        let readings = chinese_syllabified_readings_by_text();
        let first = readings.get("还剩").unwrap();
        let mask = abbreviation_homophone_group("hais", "还剩", first, &page, true);
        assert_eq!(mask & 0b111, 0b111, "还剩 还是 还说: {mask:b}");
        assert_eq!(mask & (1 << 4), 0, "one character is not the same word length");
        assert_eq!(abbreviation_homophone_group("hais", "还剩", first, &page, false), 0, "only with context");
    }

    #[test]
    fn an_offered_composition_takes_the_last_first_page_slot_and_commits() {
        let mut mixer = MixerDecoder::new(
            Box::new(ListDecoder::with(
                "taifuzale",
                &[("太傅", "tai fu"), ("台服", "tai fu"), ("台服砸了", "tai fu za le"), ("泰复", "tai fu"), ("太", "tai")],
            )),
            "c3-offer".into(),
        );
        let before = mixer.snapshot().candidates.iter().map(|c| c.text.clone()).collect::<Vec<_>>();
        assert!(mixer.offer_composition("不在备选里").is_err());
        mixer.offer_composition("太复杂了").unwrap();
        let page = mixer.snapshot();
        let texts = page.candidates.iter().map(|c| c.text.clone()).collect::<Vec<_>>();
        assert_eq!(texts[..4], before[..4], "slots 1-4 keep their candidates");
        assert_eq!(texts[4], "太复杂了");
        assert_eq!(page.candidates[4].comment, crate::ranking::COMPOSITION_OFFER_COMMENT);
        assert_eq!(mixer.select_candidate(4, 9, LearningIntent::DeliberateSelection).unwrap(), "太复杂了");
        assert!(mixer.snapshot().raw_input.is_empty());
    }

    #[test]
    fn the_single_characters_after_two_pages_follow_the_letters_left_after_a_picked_segment() {
        // 分析 picked, d toutoude left: the decoder's boundary reports the
        // letters left, and the single characters must start with them.
        let mut decoder = ListDecoder::with("fenxidtoutoude", &[]);
        decoder.exact_boundary = Some(CompositionBoundary {
            confirmed: "分析".into(), unconfirmed_raw: "dtoutoude".into(),
        });
        let rest = decoder.composition_boundary(BoundaryPurpose::Mixing).unwrap().unconfirmed_raw;
        assert_eq!(rest, "dtoutoude");
        let start = deep_single_start(&rest).expect("the letters left start the single characters");
        let candidate = |text: &str, reading: &str| Candidate {
            text: text.into(),
            comment: String::new(),
            reading_key: reading.into(),
            source_index: None,
        };
        assert!(start.matches(&candidate("的", "de")), "a single character starting with d");
        assert!(!start.matches(&candidate("分", "fen")), "not the picked part's first syllable");
        // The whole input would have chosen the picked part's syllable instead.
        let whole = deep_single_start("fenxidtoutoude").expect("whole input start");
        assert!(whole.matches(&candidate("分", "fen")) && !whole.matches(&candidate("的", "de")));
    }

    #[test]
    fn what_the_composition_offer_pushes_off_the_first_page_leads_the_second() {
        // wo xuan yi xuan: the offer takes slot 5 of the first page; 我, the common single
        // character that was there, must still be on the next page (owner 2026-10-12).
        let values = [
            ("我悬疑选", "wo xuan yi xuan"), ("斡旋", "wo xuan"), ("我选", "wo xuan"), ("涡旋", "wo xuan"), ("我", "wo"),
            ("喔", "wo"), ("窝", "wo"), ("握", "wo"), ("卧", "wo"), ("沃", "wo"),
            ("硪", "wo"), ("倭", "wo"), ("渥", "wo"), ("蜗", "wo"), ("肟", "wo"),
        ];
        let (decoder, selections) = DeepPagedDecoder::new("woxuanyixuan", &values);
        let mut mixer = MixerDecoder::new(Box::new(decoder), "carry-pages".into());
        mixer.offer_composition("我选一选").unwrap();
        let texts = |mixer: &MixerDecoder| mixer.snapshot().candidates.iter().map(|c| c.text.clone()).collect::<Vec<_>>();
        assert_eq!(texts(&mixer), ["我悬疑选", "斡旋", "我选", "涡旋", "我选一选"]);
        mixer.page_down().unwrap();
        let second = texts(&mixer);
        assert_eq!(second[0], "我", "{second:?}");
        assert_eq!(second.len(), PAGE_SIZE);
        // Nothing is lost and nothing repeats across the pages.
        let mut all = vec!["我悬疑选".to_string(), "斡旋".into(), "我选".into(), "涡旋".into(), "我选一选".into()];
        all.extend(second);
        for _ in 0..3 {
            if mixer.snapshot().is_last_page {
                break;
            }
            mixer.page_down().unwrap();
            all.extend(texts(&mixer));
        }
        for (text, _) in values {
            assert!(all.iter().any(|shown| shown == text), "{text} was lost: {all:?}");
        }
        assert_eq!(all.len(), all.iter().collect::<HashSet<_>>().len(), "{all:?}");
        // The carried character commits through the decoder's own index.
        let mut again = MixerDecoder::new(Box::new(DeepPagedDecoder::new("woxuanyixuan", &values).0), "carry-pages-2".into());
        again.offer_composition("我选一选").unwrap();
        again.page_down().unwrap();
        assert_eq!(again.select_candidate(0, 5, LearningIntent::DeliberateSelection).unwrap(), "我");
        let _ = selections;
    }

    #[test]
    fn what_the_offer_pushes_off_a_last_page_is_one_more_page() {
        let values = [("甲", "jia"), ("乙", "yi"), ("丙", "bing"), ("丁", "ding"), ("戊", "wu")];
        let (decoder, _selections) = DeepPagedDecoder::new("woxuanyixuan", &values);
        let mut mixer = MixerDecoder::new(Box::new(decoder), "overflow-page".into());
        mixer.offer_composition("我选一选").unwrap();
        let first = mixer.snapshot();
        assert_eq!(first.candidates.len(), PAGE_SIZE);
        assert!(first.candidates.iter().any(|c| c.text == "我选一选"));
        let shown = first.candidates.iter().map(|c| c.text.clone()).collect::<Vec<_>>();
        let lost = values.iter().map(|(t, _)| t.to_string()).find(|t| !shown.contains(t)).expect("one native candidate was pushed off");
        mixer.page_down().unwrap();
        let second = mixer.snapshot();
        assert_eq!(second.candidates.iter().map(|c| c.text.clone()).collect::<Vec<_>>(), [lost.clone()]);
        assert!(second.is_last_page);
        assert_eq!(second.page_no, 1);
        mixer.page_up().unwrap();
        assert_eq!(mixer.snapshot().page_no, 0);
        mixer.page_down().unwrap();
        assert_eq!(mixer.select_candidate(0, 9, LearningIntent::DeliberateSelection).unwrap(), lost);
    }

    #[test]
    fn ai_era_names_are_registered_with_their_official_spelling() {
        for (reading, text) in [("workbuddy", "WorkBuddy"), ("deepseek", "DeepSeek"), ("claudecode", "Claude Code"), ("kimi", "Kimi"), ("obsidian", "Obsidian")] {
            assert!(tech_term_readings().get(reading).is_some_and(|texts| texts.iter().any(|value| value == text)), "{reading}");
        }
        // Ordinary words stay words.
        assert!(!tech_term_readings().contains_key("make") && !tech_term_readings().contains_key("comet"));
    }

    #[test]
    fn apostrophes_and_umlauts_do_not_lose_letters_of_the_composition() {
        let mixer = MixerDecoder::new(
            Box::new(ListDecoder::with(
                "tai'fu'za'le",
                &[("太傅", "tai fu"), ("台服", "tai fu"), ("台服砸了", "tai fu za le"), ("泰复", "tai fu"), ("太", "tai")],
            )),
            "apostrophe".into(),
        );
        let alternatives = mixer.composition_alternatives();
        assert!(alternatives.iter().any(|text| text == "太复杂了"), "{alternatives:?}");
        assert!(pinyin_segmentation(&"a".repeat(500)).is_none());
    }

    #[test]
    fn a_segment_picked_mid_composition_is_kept_and_not_spelled_again() {
        // The decoder's boundary reports the picked text and the letters left.
        let mut decoder = ListDecoder::with("daoshihwoxuanyixuan", &[("我悬疑选", "wo xuan yi xuan"), ("斡旋", "wo xuan")]);
        decoder.exact_boundary = Some(CompositionBoundary {
            confirmed: "到时候".into(), unconfirmed_raw: "woxuanyixuan".into(),
        });
        let mut mixer = MixerDecoder::new(Box::new(decoder), "partial-pick".into());
        let (snapshot, _) = mixer.visible_unranked();
        assert_eq!(snapshot.raw_input, "woxuanyixuan");
        // A sentence for the letters left, committed with the picked text in front.
        mixer.offer_composition("我选一选").unwrap();
        let texts = mixer.snapshot().candidates.iter().map(|c| c.text.clone()).collect::<Vec<_>>();
        let at = texts.iter().position(|text| text == "我选一选").expect("offered");
        assert!(texts.iter().all(|text| !text.contains('是') || text == "我选一选" || !text.contains("到是")), "{texts:?}");
        assert_eq!(mixer.select_candidate(at, 3, LearningIntent::DeliberateSelection).unwrap(), "到时候我选一选");
    }

    /// F3 2.4 under the real native decoder: pick 到时候 off
    /// `daoshihwoxuanyixuan`, then the remaining letters keep composing.
    #[test]
    fn native_partial_pick_leaves_the_remaining_letters_to_compose() {
        use crate::zhixu::loader::{DictionaryData, Entry};
        let dictionary = Arc::new(Index::from_data(DictionaryData { entries: [
            ("到时候", "dao shi hou", 9000), ("倒", "dao", 5000), ("到", "dao", 4000),
            ("我", "wo", 9000), ("选", "xuan", 8000), ("一", "yi", 7000), ("悬疑", "xuan yi", 6000),
        ].into_iter().map(|(text, reading, weight)| Entry {
            text: text.into(), reading: reading.into(), weight, layer: "fixture".into(),
        }).collect() }));
        zhixu::with_index(dictionary.clone(), || {
            let mut mixer = native_mixer("native-partial-pick", dictionary.clone());
            for letter in "daoshihwoxuanyixuan".chars() {
                mixer.input(&letter.to_string()).unwrap();
            }
            // The 到时候 prefix piece keeps composing when picked.
            assert_eq!(select_displayed_text(&mut mixer, "到时候", 3), "");
            let boundary = mixer.confirmed_boundary().expect("segment picked");
            assert_eq!(boundary.confirmed, "到时候");
            assert_eq!(boundary.unconfirmed_raw, "woxuanyixuan");
            let (snapshot, _) = mixer.visible_unranked();
            assert_eq!(snapshot.raw_input, "woxuanyixuan");
            assert!(snapshot.candidates.iter().any(|c| c.text == "我选一选"), "{snapshot:?}");
            // A sentence for the letters left commits with the picked text in front.
            mixer.offer_composition("我选一选").unwrap();
            let at = mixer.snapshot().candidates.iter().position(|c| c.text == "我选一选").unwrap();
            assert_eq!(mixer.select_candidate(at, 4, LearningIntent::DeliberateSelection).unwrap(), "到时候我选一选");
        });
    }

    /// F3 2.4 under the real native decoder: after 分析 is picked off
    /// `fenxidtoutoude`, the single characters after two pages start with d, not fen.
    #[test]
    fn native_deep_single_characters_follow_the_letters_left_after_a_picked_segment() {
        use crate::zhixu::loader::{DictionaryData, Entry};
        let dictionary = Arc::new(Index::from_data(DictionaryData { entries: [
            ("分析", "fen xi", 9000), ("分", "fen", 8000), ("坟", "fen", 7000), ("芬", "fen", 6000), ("纷", "fen", 5000),
            ("的", "de", 9000), ("都", "dou", 8000), ("道", "dao", 7000), ("到", "dao", 6000), ("东", "dong", 5000),
            ("洞", "dong", 4000), ("动", "dong", 3000),
        ].into_iter().map(|(text, reading, weight)| Entry {
            text: text.into(), reading: reading.into(), weight, layer: "fixture".into(),
        }).collect() }));
        zhixu::with_index(dictionary.clone(), || {
            let mut mixer = native_mixer("native-deep-letters-left", dictionary.clone());
            for letter in "fenxidtoutoude".chars() {
                mixer.input(&letter.to_string()).unwrap();
            }
            assert_eq!(select_displayed_text(&mut mixer, "分析", 5), "");
            assert_eq!(mixer.confirmed_boundary().unwrap().unconfirmed_raw, "dtoutoude");
            mixer.page_down().unwrap();
            mixer.page_down().unwrap();
            let deep = mixer.snapshot();
            let singles: Vec<_> = deep.candidates.iter().map(|c| c.text.clone()).collect();
            assert!(singles.iter().take(3).all(|text| ["的", "都", "道", "到", "东", "洞", "动"].contains(&text.as_str())),
                "single characters start with the letters left: {singles:?}");
            assert!(!singles.iter().take(3).any(|text| ["分", "坟", "芬", "纷"].contains(&text.as_str())),
                "not the picked part's first syllable: {singles:?}");
        });
    }

    /// F3 2.5: a Local commit carries this composition's already-picked segments.
    #[test]
    fn local_candidate_commit_carries_this_compositions_confirmed_segments() {
        use crate::zhixu::loader::{DictionaryData, Entry};
        let dictionary = Arc::new(Index::from_data(DictionaryData { entries: [
            ("到时候", "dao shi hou", 9000), ("倒", "dao", 5000),
            ("我", "wo", 9000), ("选", "xuan", 8000), ("一", "yi", 7000),
        ].into_iter().map(|(text, reading, weight)| Entry {
            text: text.into(), reading: reading.into(), weight, layer: "fixture".into(),
        }).collect() }));
        zhixu::with_index(dictionary.clone(), || {
            let mut mixer = native_mixer("local-confirmed-prefix", dictionary.clone());
            for letter in "daoshihwoxuanyixuan".chars() {
                mixer.input(&letter.to_string()).unwrap();
            }
            assert_eq!(select_displayed_text(&mut mixer, "到时候", 6), "");
            assert_eq!(mixer.confirmed_boundary().unwrap().confirmed, "到时候");
            // The composed sentence is a Local selection: committing it must not
            // drop the segment picked earlier in this same composition.
            let at = mixer.snapshot().candidates.iter().position(|c| c.text == "我选一选").unwrap();
            assert_eq!(mixer.select_candidate(at, 7, LearningIntent::DeliberateSelection).unwrap(), "到时候我选一选");
        });
    }

    /// Selects the first displayed candidate with exactly `text`, paging as needed.
    fn select_displayed_text(mixer: &mut MixerDecoder, text: &str, id: u64) -> String {
        loop {
            let page = mixer.snapshot();
            if let Some(at) = page.candidates.iter().position(|candidate| candidate.text == text) {
                return mixer.select_candidate(at, id, LearningIntent::DeliberateSelection).unwrap();
            }
            assert!(!page.is_last_page, "{text} was not offered: {:?}", page.candidates);
            mixer.page_down().unwrap();
        }
    }

    #[test]
    fn native_boundary_keeps_ascii_confirmed_text_on_local_commit() {
        let mut decoder = ListDecoder::with("adaoshihwoxuanyixuan", &[("我悬疑选", "wo xuan yi xuan"), ("斡旋", "wo xuan")]);
        decoder.confirmed = "A到时候".into();
        decoder.remainder = "wo xuan yi xuan".into();
        decoder.exact_boundary = Some(crate::state::CompositionBoundary {
            confirmed: decoder.confirmed.clone(), unconfirmed_raw: "woxuanyixuan".into(),
        });
        let mut mixer = MixerDecoder::new(Box::new(decoder), "native-partial-pick".into());
        assert!(mixer.native_editing());
        assert_eq!(mixer.visible_unranked().0.raw_input, "woxuanyixuan");
        assert_eq!(mixer.composition_boundary(crate::state::BoundaryPurpose::Editing).unwrap().confirmed, "A到时候");
        mixer.offer_composition("我选一选").unwrap();
        let at = mixer.snapshot().candidates.iter().position(|c| c.text == "我选一选").unwrap();
        assert_eq!(mixer.select_candidate(at, 3, LearningIntent::DeliberateSelection).unwrap(), "A到时候我选一选");
    }

    #[test]
    fn native_boundary_keeps_ascii_confirmed_text_on_secondary_commit() {
        let mut prediction = entry_json("native-predict-after-pick", "世界和平", "shijie", false, false, false, &[]);
        prediction["candidate_layer"] = serde_json::json!("secondary");
        prediction["sources"][0]["source_kind"] = serde_json::json!("prediction:fixture");
        let mut decoder = ListDecoder::with("anihaoshijie", &[("世界", "shi jie")]);
        decoder.confirmed = "A你好".into();
        decoder.remainder = "shi jie".into();
        decoder.exact_boundary = Some(crate::state::CompositionBoundary {
            confirmed: decoder.confirmed.clone(), unconfirmed_raw: "shijie".into(),
        });
        let mut mixer = MixerDecoder::new(Box::new(decoder), "native-assoc-after-pick".into());
        mixer.apply_lexicon(lexicon(vec![prediction]));
        mixer.configure_candidate_context(None, Vec::new(), true, true);
        assert_eq!(mixer.snapshot().secondary_candidates[0].text, "世界和平");
        assert_eq!(mixer.select_secondary_candidate(0, 1).unwrap(), "A你好世界和平");
    }

    #[test]
    fn model_composed_sentences_are_offered_and_lead_only_when_asked() {
        let mut mixer = MixerDecoder::new(
            Box::new(ListDecoder::with(
                "taifuzale",
                &[("太傅", "tai fu"), ("台服", "tai fu"), ("台服砸了", "tai fu za le"), ("泰复", "tai fu"), ("太", "tai")],
            )),
            "c3".into(),
        );
        let alternatives = mixer.composition_alternatives();
        assert!(alternatives.iter().any(|text| text == "太复杂了"), "{alternatives:?}");
        assert!(!alternatives.iter().any(|text| text == "台服砸了"), "already on the page: {alternatives:?}");
        // The complete composition already outranks the partial 太傅; it is still
        // the decoder's own sentence until the host asks.
        assert_eq!(mixer.snapshot().candidates[0].text, "台服砸了");
        assert!(mixer.lead_with_composition("不在备选里").is_err());
        mixer.lead_with_composition("太复杂了").unwrap();
        let page = mixer.snapshot();
        assert_eq!(page.candidates[0].text, "太复杂了");
        assert_eq!(page.candidates[0].comment, COMPOSITION_COMMENT);
        assert_eq!(page.candidates.len(), 5);
        assert_eq!(mixer.select_candidate(0, 7, LearningIntent::NoLearn).unwrap(), "太复杂了");
        assert!(mixer.snapshot().raw_input.is_empty());

        // A complete dictionary word first is left to the model-first group.
        let word = MixerDecoder::new(
            Box::new(ListDecoder::with("xianzaizhege", &[("现在这个", "xian zai zhe ge")])),
            "c3-word".into(),
        );
        assert!(word.composition_alternatives().is_empty() || word.snapshot().candidates[0].text == "现在这个");
    }

    #[test]
    fn loose_slips_fix_invalid_spellings_third_and_spare_abbreviations_and_particles() {
        let mut mixer = MixerDecoder::new(
            Box::new(ListDecoder::with("jieidan", &[("机诶但", "ji ei dan"), ("机", "ji"), ("记", "ji"), ("及", "ji")])),
            "loose".into(),
        );
        mixer.configure_typo(true);
        let texts = mixer.snapshot().candidates.iter().map(|c| c.text.clone()).collect::<Vec<_>>();
        assert_eq!(texts.get(2).map(String::as_str), Some("节点"), "{texts:?}");
        assert_ne!(mixer.context_complete_mask() & 0b101, 0);

        assert!(ends_with_bare_initial("woganj") && ends_with_bare_initial("kehud"));
        assert!(!ends_with_bare_initial("xian") && !ends_with_bare_initial("jieidan"));
        assert!(drops_a_consonant("buxyao", "buyao"), "x abbreviates 需");
        assert!(!drops_a_consonant("houmioan", "houmian"));
        assert!(drops_a_consonant("maidia", "maidi"), "final a is 啊");
        for (raw, first) in [("woganj", "我敢觉"), ("kehud", "客户但"), ("buxyao", "不许要")] {
            let mut mixer = MixerDecoder::new(Box::new(ListDecoder::with(raw, &[(first, raw)])), format!("loose-{raw}"));
            mixer.configure_typo(true);
            assert!(
                !mixer.snapshot().candidates.iter().any(|c| c.comment == SLIP_COMMENT),
                "raw={raw}"
            );
        }
    }

    #[test]
    fn bare_initial_groups_same_syllable_characters_only_with_context() {
        let base = [("再", "zai"), ("在", "zai"), ("做", "zuo"), ("字", "zi"), ("仔", "zai")];
        let mut mixer = MixerDecoder::new(Box::new(ListDecoder::with("z", &base)), "initial".into());
        assert_eq!(mixer.context_group_mask(), 0, "no preceding text, no group");
        mixer.configure_preceding_text(Some("搜狗最近一直".into()));
        let mask = mixer.context_group_mask();
        let texts = mixer.snapshot().candidates.iter().map(|c| c.text.clone()).collect::<Vec<_>>();
        let members = texts
            .iter()
            .enumerate()
            .filter(|(index, _)| mask & (1 << index) != 0)
            .map(|(_, text)| text.as_str())
            .collect::<Vec<_>>();
        assert!(members.contains(&"再") && members.contains(&"在"), "{texts:?} {members:?}");
        assert!(!members.contains(&"做") && !members.contains(&"字"), "{members:?}");
        // The model may then swap 在 to the front.
        let first = texts.iter().position(|t| t == "再").unwrap();
        let second = texts.iter().position(|t| t == "在").unwrap();
        let mut order = (0..texts.len()).collect::<Vec<_>>();
        order.swap(first, second);
        assert!(mixer.reorder_context_candidates(&order).is_ok());
        assert_eq!(mixer.snapshot().candidates[first].text, "在");

        // A typed full syllable keeps the existing exact-reading group only.
        let mut full = MixerDecoder::new(Box::new(ListDecoder::with("zai", &[("再", "zai"), ("在", "zai")])), "initial-full".into());
        full.configure_preceding_text(Some("一直".into()));
        assert_eq!(full.context_group_mask() & 0b11, 0b11);
    }

    #[test]
    fn valid_spelling_slip_offers_the_dictionary_word_only_for_composed_firsts() {
        let mut mixer = MixerDecoder::new(
            Box::new(ListDecoder::with(
                "yoyisi",
                &[("哟意思", "yo yi si"), ("哟", "yo"), ("唷", "yo")],
            )),
            "slip-yoyisi".into(),
        );
        mixer.configure_typo(true);
        let texts = mixer
            .snapshot()
            .candidates
            .iter()
            .map(|candidate| candidate.text.clone())
            .collect::<Vec<_>>();
        eprintln!("yoyisi -> {texts:?}");
        // Third slot; the context model alone may lift it.
        assert_eq!(texts[0], "哟意思", "{texts:?}");
        assert_eq!(texts[2], "有意思", "{texts:?}");
        assert_eq!(mixer.context_complete_mask() & 0b101, 0b101, "slip joins the model group");

        let mut off = MixerDecoder::new(
            Box::new(ListDecoder::with("yoyisi", &[("哟意思", "yo yi si")])),
            "slip-off".into(),
        );
        off.configure_typo(false);
        assert!(!off.snapshot().candidates.iter().any(|candidate| candidate.text == "有意思"));

        // A dictionary-word first means the spelling was meant.
        for (raw, base) in [
            ("nihao", [("你好", "ni hao"), ("你号", "ni hao")]),
            ("youyisi", [("有意思", "you yi si"), ("有", "you")]),
            ("women", [("我们", "wo men"), ("我", "wo")]),
            // Still typing toward nihao: the last syllable is not complete.
            ("niha", [("拟哈", "ni ha"), ("你", "ni")]),
        ] {
            let mut mixer = MixerDecoder::new(Box::new(ListDecoder::with(raw, &base)), format!("slip-{raw}"));
            mixer.configure_typo(true);
            assert!(
                !mixer.snapshot().candidates.iter().any(|candidate| candidate.comment == SLIP_COMMENT),
                "raw={raw}"
            );
        }
    }

    #[test]
    fn english_committed_with_enter_is_remembered() {
        let choices = Arc::new(Mutex::new(english_choices::Store::default()));
        let mut first = MixerDecoder::new(
            Box::new(ListDecoder::with("mini", &[("迷你", "mi ni"), ("米", "mi")])),
            "enter-english".into(),
        );
        first.english_choices = choices.clone();
        assert_eq!(first.commit_raw(), "mini");
        // mini is also whole pinyin (迷你): one Enter is not enough.
        assert_eq!(choices.lock().unwrap().get("mini"), None);
        let mut again = MixerDecoder::new(
            Box::new(ListDecoder::with("mini", &[("迷你", "mi ni"), ("米", "mi")])),
            "enter-english".into(),
        );
        again.english_choices = choices.clone();
        assert_eq!(again.commit_raw(), "mini");
        let mut second = MixerDecoder::new(
            Box::new(ListDecoder::with("mini", &[("迷你", "mi ni"), ("米", "mi")])),
            "enter-english".into(),
        );
        second.english_choices = choices.clone();
        assert_eq!(second.snapshot().candidates[0].text, "mini");
        // Raw pinyin committed with Enter is not English.
        let mut typo = MixerDecoder::new(
            Box::new(ListDecoder::with("woxikanx", &[("我吸看下", "wo xi kan xia")])),
            "enter-english".into(),
        );
        typo.english_choices = choices.clone();
        typo.commit_raw();
        assert_eq!(choices.lock().unwrap().get("woxikanx"), None);
        // agent also abbreviates 阿根廷 (a gen t), so it never leads on its own; it is
        // not whole pinyin, so one Enter is enough.
        let mut agent = MixerDecoder::new(
            Box::new(ListDecoder::with("agent", &[("阿根廷", "a gen ting")])),
            "enter-english".into(),
        );
        agent.english_choices = choices.clone();
        assert_eq!(agent.snapshot().candidates[0].text, "阿根廷");
        assert_eq!(agent.commit_raw(), "agent");
        let mut later = MixerDecoder::new(
            Box::new(ListDecoder::with("agent", &[("阿根廷", "a gen ting")])),
            "enter-english".into(),
        );
        later.english_choices = choices.clone();
        assert_eq!(later.snapshot().candidates[0].text, "agent");
    }

    #[test]
    fn spelling_repairs_swap_and_double_but_never_drop_a_consonant() {
        assert!(spelling_repairs("zhuangixnban").contains(&"zhuangxinban".to_string()));
        assert!(spelling_repairs("zhenmeixinagdao").contains(&"zhenmeixiangdao".to_string()));
        assert!(spelling_repairs("wozongan").contains(&"wozonggan".to_string()));
        assert!(all_initials("hd") && all_initials("cwfx") && all_initials("zhg"));
        assert!(homophone_word_variants("xuetiao", "学条").contains(&"血条".to_string()));
        assert!(vocabulary_word("顺手知字", "shunshouzhizi") && !vocabulary_word("顺手之子", "shunshouzhizi"));
        assert!(abbreviated_vocabulary_word("知识库", "zhishik") && abbreviated_vocabulary_word("这个", "zhge"));
        assert!(!abbreviated_vocabulary_word("知识库", "zhishikuai") && !abbreviated_vocabulary_word("这地方有", "zhedifangy"));
        assert!(!all_initials("zh") && !all_initials("ni") && !all_initials("hao"));
        // The same letter struck twice.
        assert!(spelling_repairs("zhhuyaoshi").contains(&"zhuyaoshi".to_string()));
        // An unrelated extra consonant is the owner abbreviating, not a slip.
        assert!(!spelling_repairs("fenfxiang").contains(&"fenxiang".to_string()));
        // A letter left out, and a neighbouring key.
        assert!(spelling_repairs("xianza").contains(&"xianzai".to_string()));
        assert!(spelling_repairs("yingai").contains(&"yinggai".to_string()));
        assert!(spelling_repairs("nihap").contains(&"nihao".to_string()));
        // A typed consonant abbreviates a character (等他完成, 甄维斯).
        assert!(!spelling_repairs("dengtawanc").contains(&"dengtawan".to_string()));
        assert!(!spelling_repairs("zhenweis").contains(&"zhenwei".to_string()));
        assert!(decode_compositions("zhuangxinban", 2).contains(&"装新版".to_string()));
    }

    #[test]
    fn english_words_pinyin_cannot_spell_lead_verbatim() {
        let first = |raw: &str, base: &[(&str, &str)]| {
            let mut mixer = MixerDecoder::new(Box::new(ListDecoder::with(raw, base)), format!("en-{raw}"));
            mixer.configure_typo(true);
            mixer.snapshot().candidates.into_iter().map(|item| item.text).collect::<Vec<_>>()
        };
        for (raw, base) in [
            ("opencode", [("偶尔弄得", "ou er nong de"), ("藕片", "ou pian")]),
            ("deepseek", [("第二饿死嗯嗯", "di er e si en en"), ("德尔", "de er")]),
            ("freelance", [("发热而来内测", "fa re er lai nei ce"), ("发热", "fa re")]),
            ("flash", [("发拉升", "fa la sheng"), ("法拉", "fa la")]),
        ] {
            let texts = first(raw, &base);
            // The English leads; a registered name leads with its official spelling (OpenCode, DeepSeek).
            assert!(texts[0].eq_ignore_ascii_case(raw), "{texts:?}");
            assert!(texts.contains(&base[0].0.to_string()), "Chinese stays reachable: {texts:?}");
        }
        // Ordinary Chinese abbreviations, hurried Chinese and pinyin spellings keep their order.
        for (raw, base) in [
            ("went", [("问题", "wen ti"), ("文体", "wen ti")]),
            ("meiwent", [("没问题", "mei wen ti"), ("没", "mei")]),
            ("nikaolxia", [("你考虑下", "ni kao lü xia"), ("你考虑", "ni kao lü")]),
            ("budaun", [("不大", "bu da"), ("不", "bu")]),
            ("mini", [("迷你", "mi ni"), ("米", "mi")]),
        ] {
            let texts = first(raw, &base);
            assert_ne!(texts[0], raw, "{texts:?}");
            assert!(!texts[0].is_ascii(), "{texts:?}");
        }
        assert!(english_word_parts("drawthings") && english_word_parts("openrouter"));
        assert!(!english_word_parts("jiandau") && !english_word_parts("shudoeshi"));
    }

    #[test]
    fn deliberate_english_picks_lead_until_chinese_is_picked() {
        let choices = Arc::new(Mutex::new(english_choices::Store::default()));
        let mixer = |choices: &Arc<Mutex<english_choices::Store>>| {
            let mut mixer = MixerDecoder::new(
                Box::new(ListDecoder::with("mini", &[("迷你", "mi ni"), ("米", "mi"), ("mini", "mini")])),
                "english-choices".into(),
            );
            mixer.english_choices = choices.clone();
            mixer
        };
        let texts = |mixer: &MixerDecoder| {
            mixer.snapshot().candidates.into_iter().map(|item| item.text).collect::<Vec<_>>()
        };
        let mut first = mixer(&choices);
        assert_eq!(texts(&first)[0], "迷你");
        let index = texts(&first).iter().position(|text| text == "mini").unwrap();
        first.select_candidate(index, 1, LearningIntent::DeliberateSelection).unwrap();
        let mut second = mixer(&choices);
        assert_eq!(texts(&second)[0], "mini");
        let index = texts(&second).iter().position(|text| text == "迷你").unwrap();
        second.select_candidate(index, 2, LearningIntent::DeliberateSelection).unwrap();
        assert_eq!(texts(&mixer(&choices))[0], "迷你");
        // Raw pinyin committed verbatim is not English.
        let mut raw = MixerDecoder::new(
            Box::new(ListDecoder::with("woxikanx", &[("我吸看下", "wo xi kan xia"), ("woxikanx", "woxikanx")])),
            "english-choices".into(),
        );
        raw.english_choices = choices.clone();
        raw.select_candidate(1, 3, LearningIntent::DeliberateSelection).unwrap();
        assert_eq!(choices.lock().unwrap().get("woxikanx"), None);
    }

    /// Local-only owner probe: `AIM_ENGLISH_EVENTS=<events.jsonl> cargo test --release
    /// owner_english_probe -- --ignored --nocapture`. Lists recorded inputs where the
    /// English word would now lead, with what the owner committed.
    #[test]
    #[ignore]
    fn owner_english_probe() {
        let path = std::env::var("AIM_ENGLISH_EVENTS").expect("AIM_ENGLISH_EVENTS");
        let mut tally = std::collections::BTreeMap::<String, usize>::new();
        for line in std::fs::read_to_string(path).unwrap().lines() {
            let Ok(event) = serde_json::from_str::<serde_json::Value>(line) else { continue };
            let raw = event["pinyin"].as_str().unwrap_or_default().to_string();
            let chosen = event["selectedText"].as_str().unwrap_or_default().to_string();
            if chosen.is_empty() {
                continue;
            }
            let base = event["candidates"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|value| value.as_str())
                .enumerate()
                .map(|(index, text)| MixedCandidate {
                    candidate: Candidate { text: text.into(), comment: String::new(), reading_key: raw.clone(), source_index: Some(index) },
                    selection: Selection::Base { index, text: text.into() },
                    tier: 2,
                    score: 0,
                })
                .collect::<Vec<_>>();
            if !english_word_leads(&raw, &base) {
                continue;
            }
            let kind = if chosen.eq_ignore_ascii_case(&raw) { "EN" } else if chosen.is_ascii() { "ASCII" } else { "ZH" };
            *tally.entry(format!("{kind}\t{raw}\t{chosen}")).or_default() += 1;
        }
        for (key, count) in tally {
            eprintln!("{count}\t{key}");
        }
    }

    /// Local-only owner probe: `AIM_SLIP_EVENTS=<events.jsonl> cargo test --release
    /// owner_slip_probe -- --ignored --nocapture`. Prints counts and a few examples.
    #[test]
    #[ignore]
    fn owner_slip_probe() {
        let path = std::env::var("AIM_SLIP_EVENTS").expect("AIM_SLIP_EVENTS");
        let (mut fired, mut fixed, mut kept_first, mut other) = (0, 0, 0, 0);
        let mut examples = Vec::new();
        for line in std::fs::read_to_string(path).unwrap().lines() {
            let Ok(event) = serde_json::from_str::<serde_json::Value>(line) else { continue };
            let raw = event["pinyin"].as_str().unwrap_or_default().to_ascii_lowercase();
            let chosen = event["selectedText"].as_str().unwrap_or_default().to_string();
            let first = event["candidates"][0].as_str().unwrap_or_default().to_string();
            if chosen.is_empty() || first.is_empty() || !raw.bytes().all(|b| b.is_ascii_lowercase()) {
                continue;
            }
            let base = vec![MixedCandidate {
                candidate: Candidate { text: first.clone(), comment: String::new(), reading_key: raw.clone(), source_index: Some(0) },
                selection: Selection::Base { index: 0, text: first.clone() },
                tier: 2,
                score: 0,
            }];
            if should_promote_raw_ascii(&raw, &base) { continue }
            let Some(slip) = slip_candidates(&raw, &base, false).into_iter().next() else { continue };
            fired += 1;
            let text = slip.candidate.text;
            if chosen == text { fixed += 1 } else if chosen == first { kept_first += 1 } else { other += 1 }
            examples.push(format!("{}\t{raw}\t{first}\t{text}\t{chosen}", event["id"].as_str().unwrap_or_default()));
        }
        eprintln!("fired={fired} owner_chose_slip={fixed} owner_kept_first={kept_first} other={other}");
        for example in examples { eprintln!("  {example}"); }
    }

    #[test]
    fn mixed_language_promotes_bad_pinyin_and_keeps_normal_chinese_order() {
        for raw in ["codex", "claude", "github", "api", "token", "python"] {
            let mixer = MixerDecoder::new(
                Box::new(ListDecoder::with(
                    raw,
                    &[("垃圾解析", raw), ("组句", "zujv")],
                )),
                format!("mixed-{raw}"),
            );
            let terms = tech_term_readings();
            let expected = terms
                .get(raw)
                .and_then(|values| values.first())
                .map(String::as_str)
                .unwrap_or(raw);
            assert_eq!(mixer.snapshot().candidates[0].text, expected, "raw={raw}");
        }

        for (raw, base) in [
            ("nihao", [("你好", "nihao"), ("你号", "nihao")]),
            ("zhongguo", [("中国", "zhongguo"), ("中过", "zhongguo")]),
        ] {
            let mixer = MixerDecoder::new(
                Box::new(ListDecoder::with(raw, &base)),
                format!("pinyin-{raw}"),
            );
            let snapshot = mixer.snapshot();
            assert_eq!(
                snapshot
                    .candidates
                    .iter()
                    .map(|candidate| candidate.text.as_str())
                    .collect::<Vec<_>>(),
                base.iter().map(|(text, _)| *text).collect::<Vec<_>>(),
                "normal pinyin first layer changed for {raw}"
            );
            assert!(snapshot.secondary_candidates.is_empty());
        }
    }

    #[test]
    fn mixed_language_preserves_coherent_abbreviation_repairs_and_base_learning_path() {
        let repaired = MixerDecoder::new(
            Box::new(ListDecoder::with(
                "zenm",
                &[("怎你们", "zennimen"), ("怎么", "zenme")],
            )),
            "mixed-zenm".into(),
        );
        assert_eq!(repaired.snapshot().candidates[0].text, "怎么");
        assert!(
            repaired
                .snapshot()
                .candidates
                .iter()
                .all(|item| item.text != "zenm")
        );
        assert!(repaired.snapshot().secondary_candidates.is_empty());

        let mut learned = MixerDecoder::new(
            Box::new(ListDecoder::with(
                "codex",
                &[("垃圾解析", "codex"), ("codex", "codex")],
            )),
            "mixed-learning".into(),
        );
        assert_eq!(learned.snapshot().candidates[0].text, "Codex");
        assert_eq!(
            learned
                .select_candidate(0, 41, LearningIntent::DeliberateSelection)
                .unwrap(),
            "Codex"
        );
    }

    #[test]
    fn singles_after_two_pages_follow_how_the_input_starts() {
        let single = |text: &str, reading: &str| Candidate {
            text: text.into(),
            comment: String::new(),
            reading_key: reading.into(),
            source_index: None,
        };
        let start = |raw: &str| match deep_single_start(raw) {
            Some(DeepSingleStart::Syllable(value)) => format!("syllable {value}"),
            Some(DeepSingleStart::Prefix(value)) => format!("prefix {value}"),
            None => "none".into(),
        };
        assert_eq!(start("woxianzaiqukan"), "syllable wo");
        assert_eq!(start("jiandan"), "none", "two whole syllables: Rime already gives singles");
        assert_eq!(start("wenzhge"), "prefix wen");
        assert_eq!(start("zhgexiangm"), "prefix zh");
        assert_eq!(start("tnabian"), "prefix t");
        let wen = deep_single_start("wenzhge").unwrap();
        assert!(wen.matches(&single("吻", "wen")));
        assert!(!wen.matches(&single("我", "wo")), "a whole syllable matches exactly");
        assert!(!wen.matches(&single("问诊", "wen zhen")), "only single characters");
        let t = deep_single_start("tnabian").unwrap();
        assert!(t.matches(&single("挺", "ting")) && t.matches(&single("他", "ta")));
        assert!(!t.matches(&single("你", "ni")));
    }

    #[test]
    fn three_letter_english_takes_the_last_first_page_slot_and_never_leads() {
        let item = |text: &str| MixedCandidate {
            candidate: Candidate {
                text: text.into(),
                comment: String::new(),
                reading_key: "mac".into(),
                source_index: None,
            },
            selection: Selection::Local { text: text.into() },
            tier: 2,
            score: 0,
        };
        let page = |words: &[&str]| words.iter().map(|text| item(text)).collect::<Vec<_>>();
        let texts = |output: &[MixedCandidate]| output.iter().map(|item| item.candidate.text.clone()).collect::<Vec<_>>();
        // A common word, and a tech word that also spells pinyin (env = e + n + v).
        for word in ["mac", "key", "env"] {
            let mut output = page(&["一", "二", "三", "四", "五", "六"]);
            place_short_english(word, word, &mut output);
            assert_eq!(texts(&output)[..4], ["一", "二", "三", "四"], "slots 1-4 stay: {word}");
            assert_eq!(output[4].candidate.text, word);
            assert_eq!(output[4].candidate.comment, "英文 · 原样");
            assert_eq!(output.len(), 7);
        }
        // Already on the first page (case-insensitively), or further down: no duplicate.
        let mut output = page(&["API", "一", "二", "三", "四", "api"]);
        place_short_english("api", "api", &mut output);
        assert_eq!(texts(&output), ["API", "一", "二", "三", "四", "api"]);
        let mut output = page(&["一", "二", "三", "四", "五", "mac"]);
        place_short_english("mac", "mac", &mut output);
        assert_eq!(texts(&output), ["一", "二", "三", "四", "mac", "五"]);
        // Plain pinyin (you, run), other lengths and rare fragments are left alone.
        for word in ["you", "run", "macs", "ma", "aun", "Mac"] {
            let mut output = page(&["一", "二", "三", "四", "五"]);
            place_short_english(word, word, &mut output);
            assert_eq!(output.len(), 5, "{word}");
        }
    }

    #[test]
    fn raw_ascii_fallback_takes_the_last_first_page_slot_and_terms_stay() {
        let item = |text: &str, comment: &str| MixedCandidate {
            candidate: Candidate {
                text: text.into(),
                comment: comment.into(),
                reading_key: "zuozhgexiangm".into(),
                source_index: None,
            },
            selection: Selection::Local { text: text.into() },
            tier: 2,
            score: 0,
        };
        let mut output = vec![
            item("作者项目", ""), item("作者各项", ""), item("zuozhgexiangm", "英文 · 原样"),
            item("做这个项目", ""), item("作者", ""), item("做", ""),
        ];
        demote_raw_ascii_to_last_first_page_slot("zuozhgexiangm", &mut output);
        let texts = output.iter().map(|item| item.candidate.text.as_str()).collect::<Vec<_>>();
        assert_eq!(texts, ["作者项目", "作者各项", "做这个项目", "作者", "zuozhgexiangm", "做"]);

        // Short pages keep it last; registered English terms are not touched.
        let mut short = vec![item("sonnet", "英文 · 原样"), item("送你", "")];
        demote_raw_ascii_to_last_first_page_slot("sonnet", &mut short);
        assert_eq!(short[1].candidate.text, "sonnet");
        let mut registered = vec![item("Docker", "英文 · 行业词"), item("多课", "")];
        demote_raw_ascii_to_last_first_page_slot("docker", &mut registered);
        assert_eq!(registered[0].candidate.text, "Docker");
        // The raw spelling of a registered term keeps its slot (frozen canonical-mariadb).
        let mut variant = vec![
            item("MariaDB", "英文 · 行业词"), item("mariadb", "英文 · 原样"),
            item("吗日啊代表", ""), item("吗", ""), item("嘛", ""),
        ];
        demote_raw_ascii_to_last_first_page_slot("mariadb", &mut variant);
        assert_eq!(variant[1].candidate.text, "mariadb");
    }

    #[test]
    fn mixed_language_quality_rule_is_deterministic() {
        for valid in ["nihao", "zhongguo", "xiexie", "renmin", "xi'an"] {
            assert!(valid_pinyin_spelling(valid), "valid={valid}");
        }
        for invalid in ["codex", "claude", "github", "hello", "world", "python"] {
            assert!(!valid_pinyin_spelling(invalid), "invalid={invalid}");
        }
    }

    #[test]
    fn narrow_mixed_input_embeds_one_unambiguous_canonical_ascii_term() {
        for (raw, base, expected) in [
            (
                "dangqianzhetaimacbookproa",
                vec![("当前这台吗传播哦恐怕人哦啊", "dangqianzhetaimacbookproa")],
                "当前这台MacBookPro啊",
            ),
            (
                "dangqianzhetaiiphonea",
                vec![("当前这台好哦女儿啊", "dangqianzhetaiiphonea")],
                "当前这台iPhone啊",
            ),
            (
                // the page's own guess no longer ends in 啊: the suffix is decoded on its own
                "dangqianzhetaiiphonea",
                vec![("当前这台盘后女儿", "dangqianzhetaiiphonea")],
                "当前这台iPhone啊",
            ),
            (
                "dangqianzhetaigithuba",
                vec![
                    ("当前这台个体会把", "dangqianzhetaigithuba"),
                    ("当前这台固体会不会啊", "dangqianzhetaigithuba"),
                ],
                "当前这台GitHub啊",
            ),
        ] {
            let mut mixer = MixerDecoder::new(
                Box::new(ListDecoder::with(raw, &base)),
                format!("narrow-mixed-{raw}"),
            );
            let snapshot = mixer.snapshot();
            assert_eq!(snapshot.candidates[0].text, expected, "raw={raw}");
            assert_eq!(
                snapshot.candidates[0].comment, "中英混输 · 规范词典",
                "raw={raw}"
            );
            assert_eq!(
                mixer
                    .select_candidate(0, 1, LearningIntent::DeliberateSelection)
                    .unwrap(),
                expected,
                "raw={raw}"
            );
            assert!(mixer.snapshot().raw_input.is_empty(), "raw={raw}");
        }

        assert!(embedded_ascii_match("macbookpro").is_none());
        assert!(embedded_ascii_match("dangqianapia").is_none());
        assert!(exact_dictionary_composition_matches(
            "dangqianzhetai",
            "当前这台"
        ));
        assert!(exact_dictionary_composition_matches("a", "啊"));
    }

    #[test]
    fn offcorpus_ascii_is_generalized_and_chinese_initials_stay_first() {
        for (raw, canonical) in [
            ("mariadb", "MariaDB"),
            ("docker", "Docker"),
            ("react", "React"),
            ("postgres", "PostgreSQL"),
            ("nginx", "nginx"),
            ("windows", "Windows"),
            ("kubernetes", "Kubernetes"),
            ("redis", "Redis"),
        ] {
            let mut mixer = MixerDecoder::new(
                Box::new(ListDecoder::with(raw, &[("缩写组句垃圾", raw)])),
                format!("offcorpus-{raw}"),
            );
            let unknown_candidates = mixer.snapshot().candidates;
            assert_eq!(unknown_candidates[0].text, canonical, "raw={raw}");
            assert!(
                unknown_candidates
                    .iter()
                    .take(PAGE_SIZE)
                    .any(|item| item.text == canonical),
                "canonical form missing for {raw}: {unknown_candidates:?}"
            );
            assert!(
                unknown_candidates.iter().any(|item| item.text == raw),
                "raw fallback missing for {raw}: {unknown_candidates:?}"
            );
            mixer.configure_code_ascii_bias(true);
            assert_eq!(
                mixer.snapshot().candidates[0].text,
                canonical,
                "code raw={raw}"
            );
        }

        for (raw, canonical) in [
            ("windo", "Windows"),
            ("window", "Windows"),
            ("maria", "MariaDB"),
            ("clau", "Claude"),
            ("dock", "Docker"),
            ("typescr", "TypeScript"),
        ] {
            let mixer = MixerDecoder::new(
                Box::new(ListDecoder::with(raw, &[("组句垃圾", raw)])),
                format!("ascii-prefix-{raw}"),
            );
            let candidates = mixer.snapshot().candidates;
            assert_eq!(
                candidates[0].text, canonical,
                "raw={raw} candidates={candidates:?}"
            );
        }

        for (raw, chinese, reading) in [
            ("w", "我", "wo"),
            ("wo", "我", "wo"),
            ("ni", "你", "ni"),
            ("ma", "吗", "ma"),
            ("da", "大", "da"),
            ("ke", "可", "ke"),
            ("ra", "热爱", "re ai"),
            ("d", "的", "de"),
        ] {
            let mixer = MixerDecoder::new(
                Box::new(ListDecoder::with(raw, &[(chinese, reading)])),
                format!("ascii-prefix-control-{raw}"),
            );
            assert_eq!(mixer.snapshot().candidates[0].text, chinese, "raw={raw}");
        }

        for (raw, values) in [
            ("ra", [("热爱", "reai"), ("人啊", "rena")]),
            ("d", [("的", "de"), ("地", "di")]),
        ] {
            let mixer = MixerDecoder::new(
                Box::new(ListDecoder::with(raw, &values)),
                format!("initial-{raw}"),
            );
            assert_eq!(mixer.snapshot().candidates[0].text, values[0].0);
        }
    }

    #[test]
    fn short_tech_terms_never_leak_into_the_prediction_only_second_row() {
        let values = [
            ("中文", "zhong wen"),
            ("候选", "hou xuan"),
            ("保持", "bao chi"),
        ];
        let mut checked = 0usize;
        for (raw, expected_values) in tech_term_readings()
            .iter()
            .filter(|(reading, _)| reading.len() <= 3)
        {
            let mixer = MixerDecoder::new(
                Box::new(ListDecoder::with(raw, &values)),
                format!("q2-{raw}"),
            );
            let snapshot = mixer.snapshot();
            let _ = expected_values;
            assert!(
                snapshot.secondary_candidates.is_empty(),
                "raw={raw} secondary={:?}",
                snapshot.secondary_candidates
            );
            checked += 1;
        }
        assert!(checked >= 10);
    }

    #[test]
    fn hr6_one_selection_moves_at_most_three_and_never_drops_without_action() {
        let values = [
            ("一", "ce"),
            ("二", "ce"),
            ("三", "ce"),
            ("四", "ce"),
            ("五", "ce"),
        ];
        let app = "hr6-bounded";
        let mut first = MixerDecoder::new(Box::new(ListDecoder::with("ce", &values)), app.into());
        assert_eq!(
            first
                .select_candidate(4, 1, LearningIntent::DeliberateSelection)
                .unwrap(),
            "五"
        );
        let reopened = MixerDecoder::new(Box::new(ListDecoder::with("ce", &values)), app.into());
        let next = reopened.snapshot().candidates;
        assert_eq!(next.iter().position(|c| c.text == "五"), Some(1));
        assert_eq!(reopened.snapshot().candidates, next);
    }

    #[test]
    fn learned_promotions_remain_bounded_and_fade_only_with_time() {
        let mut state = LearningState::default();
        for index in 0..=MAX_LEARNED_PROMOTIONS {
            state.record_at(("app".into(), format!("r{index}"), "词".into()), 0, 1_000 + index as u64);
        }
        assert_eq!(state.target_positions.len(), MAX_LEARNED_PROMOTIONS);
        assert_eq!(state.insertion_order.len(), MAX_LEARNED_PROMOTIONS);
        // The oldest is evicted first; the newest survives.
        assert!(!state.target_positions.contains_key(&("app".into(), "r0".into(), "词".into())));
        let newest = ("app".to_string(), format!("r{MAX_LEARNED_PROMOTIONS}"), "词".to_string());
        assert!(state.target_positions.contains_key(&newest));
        // 128 other picks no longer make a pick fade (they did until 2026-10-17); 46 days without it do.
        let key = ("app".to_string(), "nikk".to_string(), "你看看".to_string());
        state.record_at(key.clone(), 2, 10_000);
        for index in 0..300u64 {
            state.record_at(("app".into(), format!("x{index}"), "字".into()), 1, 10_001 + index);
        }
        assert_eq!(state.target_positions.get(&key), Some(&2), "still there after 300 other picks");
        state.record_at(("app".into(), "later".into(), "字".into()), 1, 10_000 + personal_picks::MAX_AGE_MS + 1);
        assert!(!state.target_positions.contains_key(&key), "forgotten after 45 days");
    }

    #[test]
    fn withdraw_selection_restores_persisted_snapshot_and_is_idempotent() {
        let directory = std::env::temp_dir().join(format!("aim-i1-{}-{}", std::process::id(), personal_picks::now_ms()));
        std::fs::create_dir(&directory).unwrap();
        let mut state = LearningState::default();
        state.attach(&directory);
        let key = ("i1".into(), "ce".into(), "测试".into());
        state.persist(&[]);
        let before = personal_picks::load(&directory);
        state.record(key.clone(), 0); // shallow numeric selection
        assert!(!state.withdraw(&("other".into(), "ce".into(), "测试".into())));
        assert!(!state.withdraw(&("i1".into(), "cuo".into(), "测试".into())));
        assert!(!state.withdraw(&("i1".into(), "ce".into(), "别的".into())));
        assert!(state.withdraw(&key));
        assert_eq!(personal_picks::load(&directory), before);
        assert!(!state.withdraw(&key));
        state.record(key.clone(), 5);
        state.record(key.clone(), 5);
        let before = personal_picks::load(&directory);
        state.record(key.clone(), 5);
        assert_eq!(state.counts[&key], 3);
        assert_eq!(state.target_positions[&key], 0);
        assert!(state.withdraw(&key));
        assert_eq!(state.counts[&key], 2);
        assert_eq!(state.target_positions[&key], 5);
        assert_eq!(personal_picks::load(&directory), before);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn withdraw_selection_restores_fifo_and_expired_entries() {
        let mut state = LearningState::default();
        for i in 0..MAX_LEARNED_PROMOTIONS {
            let key = ("i1-capacity".into(), "ce".into(), format!("词{i}"));
            state.target_positions.insert(key.clone(), 1);
            state.counts.insert(key.clone(), 2);
            state.last_ms.insert(key.clone(), 100);
            state.insertion_order.push_back(key);
        }
        let before = state.clone();
        let key = ("i1-capacity".into(), "ce".into(), "新增".into());
        for now in [101, 101 + personal_picks::MAX_AGE_MS] {
            state.record_at(key.clone(), 0, now);
            assert!(state.withdraw(&key));
            assert_eq!(state.target_positions, before.target_positions);
            assert_eq!(state.counts, before.counts);
            assert_eq!(state.last_ms, before.last_ms);
            assert_eq!(state.insertion_order, before.insertion_order);
        }
    }

    #[test]
    fn withdraw_selection_only_latest_and_forget_invalidates() {
        let mut state = LearningState::default();
        let first = ("i1".into(), "ce".into(), "甲".into());
        let last = ("i1".into(), "ce".into(), "乙".into());
        state.record(first.clone(), 1);
        state.record(last.clone(), 1);
        assert!(!state.withdraw(&first));
        assert!(state.withdraw(&last));
        assert!(!state.withdraw(&first));
        assert_eq!(state.counts[&first], 1);
        state.record(last.clone(), 1);
        state.forget(&first);
        assert!(!state.withdraw(&last));
        state.record(last.clone(), 1);
        state = LearningState::default();
        assert!(!state.withdraw(&last));
    }

    #[test]
    fn a_pick_made_three_times_goes_first_whatever_its_slot() {
        let mut state = LearningState::default();
        let key = ("app".to_string(), "nikk".to_string(), "你看看".to_string());
        state.record_at(key.clone(), 5, 1);
        assert_eq!(state.target_positions.get(&key), Some(&5), "the first pick moves it up three places from slot 8");
        state.record_at(key.clone(), 5, 2);
        assert_eq!(state.target_positions.get(&key), Some(&5));
        state.record_at(key.clone(), 5, 3);
        assert_eq!(state.target_positions.get(&key), Some(&0), "the third pick puts it first");
        assert_eq!(state.counts.get(&key), Some(&3));
    }

    #[test]
    fn learned_promotions_survive_a_restart_and_forgetting_reaches_the_file() {
        let directory = std::env::temp_dir().join(format!("aim-learned-{}-{}", std::process::id(), personal_picks::now_ms()));
        std::fs::create_dir(&directory).unwrap();
        let directory = std::fs::canonicalize(&directory).unwrap();
        let key = ("app".to_string(), "nikk".to_string(), "你看看".to_string());
        let other = ("app".to_string(), "zhge".to_string(), "这个".to_string());
        let mut first = LearningState::default();
        first.attach(&directory);
        first.record(key.clone(), 0);
        first.record(key.clone(), 0);
        first.record(key.clone(), 0);
        first.record(other.clone(), 3);
        // a new process reads it back, with the counts
        let mut second = LearningState::default();
        second.attach(&directory);
        assert_eq!(second.target_positions.get(&key), Some(&0));
        assert_eq!(second.counts.get(&key), Some(&3));
        assert_eq!(second.target_positions.get(&other), Some(&3));
        // two processes at once: each keeps what the other wrote
        second.record(("app".into(), "nam".into(), "那么".into()), 1);
        first.record(("app".into(), "wans".into(), "晚上".into()), 2);
        let mut third = LearningState::default();
        third.attach(&directory);
        for reading in ["nikk", "zhge", "nam", "wans"] {
            assert!(third.target_positions.keys().any(|k| k.1 == reading), "{reading} survived both writers");
        }
        third.forget(&key);
        let mut fourth = LearningState::default();
        fourth.attach(&directory);
        assert!(!fourth.target_positions.contains_key(&key), "forgotten in the file too");
        assert!(fourth.target_positions.contains_key(&other));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn top_one_is_committed_without_learning_but_deliberate_selection_learns() {
        let values = [("一", "ce"), ("二", "ce"), ("三", "ce")];
        let app = "top-one-no-learning";
        let first_decoder = ListDecoder::with("ce", &values);
        let first_deliveries = first_decoder.delivery_ids.clone();
        let mut first = MixerDecoder::new(Box::new(first_decoder), app.into());
        assert_eq!(
            first
                .select_candidate(0, 77, LearningIntent::NoLearn)
                .unwrap(),
            "一"
        );
        assert_eq!(
            *first_deliveries.lock().unwrap(),
            vec![(77, LearningIntent::NoLearn)]
        );
        assert!(
            shared_learning()
                .lock()
                .unwrap()
                .target_positions
                .keys()
                .all(|(learned_app, _, _)| learned_app != app)
        );

        let required_decoder = ListDecoder::with("ce", &values);
        let required_deliveries = required_decoder.delivery_ids.clone();
        let mut required = MixerDecoder::new(Box::new(required_decoder), app.into());
        assert_eq!(
            required
                .select_candidate(0, 78, LearningIntent::RequiredSegmentCommit)
                .unwrap(),
            "一"
        );
        assert_eq!(
            *required_deliveries.lock().unwrap(),
            vec![(78, LearningIntent::RequiredSegmentCommit)]
        );

        let deliberate_decoder = ListDecoder::with("ce", &values);
        let deliberate_deliveries = deliberate_decoder.delivery_ids.clone();
        let mut deliberate = MixerDecoder::new(Box::new(deliberate_decoder), app.into());
        assert_eq!(
            deliberate
                .select_candidate(2, 88, LearningIntent::DeliberateSelection)
                .unwrap(),
            "三"
        );
        assert_eq!(
            *deliberate_deliveries.lock().unwrap(),
            vec![(88, LearningIntent::DeliberateSelection)]
        );
        let reopened = MixerDecoder::new(Box::new(ListDecoder::with("ce", &values)), app.into());
        assert_eq!(reopened.snapshot().candidates[0].text, "三");
    }

    #[test]
    fn blacklist_is_reading_scoped_filters_every_source_and_forgets_learning() {
        let values = [("栅格", "zha ge"), ("这个", "zhe ge")];
        let mut mixer = MixerDecoder::new(
            Box::new(ListDecoder::with("zhge", &values)),
            "blacklist-scope".into(),
        );
        mixer
            .select_candidate(1, 1, LearningIntent::DeliberateSelection)
            .unwrap();
        let mut mixer = MixerDecoder::new(
            Box::new(ListDecoder::with("zhge", &values)),
            "blacklist-scope".into(),
        );
        let target = mixer
            .snapshot()
            .candidates
            .iter()
            .position(|candidate| candidate.text == "栅格")
            .unwrap();
        mixer.blacklist_candidate(target).unwrap();
        let blacklist = mixer.blacklist.clone();
        assert!(
            mixer
                .snapshot()
                .candidates
                .iter()
                .all(|candidate| candidate.text != "栅格")
        );

        let mut abbreviated = MixerDecoder::new(
            Box::new(ListDecoder::with(
                "zhg",
                &[("栅格", "zha ge"), ("这个", "zhe ge")],
            )),
            "blacklist-scope".into(),
        );
        abbreviated.apply_blacklist(blacklist.clone());
        assert!(
            abbreviated
                .snapshot()
                .candidates
                .iter()
                .all(|candidate| candidate.text != "栅格")
        );

        let mut other_reading = MixerDecoder::new(
            Box::new(ListDecoder::with("shange", &[("栅格", "shan ge")])),
            "blacklist-scope".into(),
        );
        other_reading.apply_blacklist(blacklist.clone());
        assert_eq!(other_reading.snapshot().candidates[0].text, "栅格");
        let mut english = MixerDecoder::new(
            Box::new(ListDecoder::with("mariadb", &[("MariaDB", "mariadb")])),
            "blacklist-scope".into(),
        );
        english.apply_blacklist(blacklist);
        assert_eq!(english.snapshot().candidates[0].text, "MariaDB");
    }

    #[test]
    fn backspace_on_non_ascii_override_shortens_by_char_without_panicking() {
        let mut mixer = MixerDecoder::new(
            Box::new(ListDecoder::with("ka", &[("咖", "ka")])),
            "backspace-utf8".into(),
        );
        mixer.raw_override = Some("caf例".to_string());
        assert_eq!(mixer.backspace(), Ok(String::new()));
        assert_eq!(mixer.raw_override.as_deref(), Some("caf"));
        assert_eq!(mixer.typed_ascii, "caf");
    }

    #[test]
    fn typo_dictionary_is_loaded_and_nonempty_from_the_frozen_corpus() {
        // The graceful-degradation path must not mask a broken embedded corpus:
        // with the real corpus the dictionary must parse and be non-empty.
        assert!(!typo_dictionary().is_empty());
    }

    #[test]
    fn typo_corrections_remain_internal_and_never_enter_candidate_rows() {
        let mut mixer = MixerDecoder::new(
            Box::new(ListDecoder::with("niha", &[("拟哈", "niha")])),
            "typo".into(),
        );
        mixer.configure_typo(true);
        assert!(
            mixer
                .snapshot()
                .candidates
                .iter()
                .all(|c| !c.comment.starts_with("纠错"))
        );
        assert!(typo_candidates("niha").iter().any(|candidate| {
            candidate.text == "你好" && candidate.comment == "纠错 · nihao"
        }));
        assert!(mixer.snapshot().secondary_candidates.is_empty());
    }

    #[test]
    fn hr8_project_terms_only_apply_to_the_exact_active_project() {
        let project = lexicon(vec![entry_json(
            "p",
            "项目专词",
            "ni",
            false,
            false,
            false,
            &["com.example.project"],
        )]);
        let mut active = MixerDecoder::new(
            Box::new(ListDecoder::with("ni", &[("你", "ni")])),
            "com.example.project".into(),
        );
        active.apply_lexicon(project.clone());
        active.configure_candidate_context(
            Some("com.example.project".into()),
            Vec::new(),
            true,
            false,
        );
        assert_eq!(active.snapshot().candidates[0].text, "项目专词");
        let mut unrelated = MixerDecoder::new(
            Box::new(ListDecoder::with("ni", &[("你", "ni")])),
            "com.example.other".into(),
        );
        unrelated.apply_lexicon(project);
        unrelated.configure_candidate_context(
            Some("com.example.other".into()),
            Vec::new(),
            true,
            false,
        );
        assert_eq!(
            unrelated
                .snapshot()
                .candidates
                .iter()
                .map(|c| c.text.as_str())
                .collect::<Vec<_>>(),
            ["你"]
        );
    }

    #[test]
    fn generated_project_abbreviation_is_scoped_and_does_not_change_existing_repairs() {
        let base_pi = &[("皮", "pi"), ("批", "pi"), ("屁", "pi")];
        let mut active = MixerDecoder::new(
            Box::new(ListDecoder::with("pi", base_pi)),
            "abbreviation-active".into(),
        );
        active.configure_candidate_context(
            Some("agent_input_method".into()),
            Vec::new(),
            true,
            false,
        );
        assert_eq!(active.snapshot().candidates[0].text, "ProductInputs");

        let mut unrelated = MixerDecoder::new(
            Box::new(ListDecoder::with("pi", base_pi)),
            "abbreviation-unrelated".into(),
        );
        unrelated.configure_candidate_context(
            Some("another_project".into()),
            Vec::new(),
            true,
            false,
        );
        assert_eq!(
            unrelated
                .snapshot()
                .candidates
                .iter()
                .map(|candidate| candidate.text.as_str())
                .collect::<Vec<_>>(),
            ["皮", "批", "屁"]
        );

        for (raw, base, expected) in [
            ("zhem", &[("这么", "zheme"), ("折磨", "zhemo")][..], "这么"),
            (
                "zenm",
                &[("怎么", "zenme"), ("怎你们", "zennimen")][..],
                "怎么",
            ),
            (
                "dansh",
                &[("但是", "danshi"), ("单身", "danshen")][..],
                "但是",
            ),
        ] {
            let mut mixer = MixerDecoder::new(
                Box::new(ListDecoder::with(raw, base)),
                format!("abbreviation-control-{raw}"),
            );
            mixer.configure_candidate_context(
                Some("agent_input_method".into()),
                Vec::new(),
                true,
                false,
            );
            assert_eq!(mixer.snapshot().candidates[0].text, expected, "raw={raw}");
        }
    }

    #[test]
    fn project_profile_activates_only_its_packs_and_global_order_is_stable() {
        let mut rust = entry_json("rust", "所有权", "suo you quan", false, false, false, &[]);
        rust["professional_packs"] = serde_json::json!(["rust"]);
        let mut design = entry_json(
            "design",
            "所有权设计",
            "suo you quan",
            false,
            false,
            false,
            &[],
        );
        design["professional_packs"] = serde_json::json!(["design"]);
        let shared = lexicon(vec![rust, design]);
        let base = [("所有权人", "suoyouquan")];

        let mut global = MixerDecoder::new(
            Box::new(ListDecoder::with("suoyouquan", &base)),
            "app".into(),
        );
        global.apply_lexicon(shared.clone());
        assert_eq!(global.snapshot().candidates[0].text, "所有权人");

        let mut active = MixerDecoder::new(
            Box::new(ListDecoder::with("suoyouquan", &base)),
            "app".into(),
        );
        active.apply_lexicon(shared);
        active.configure_candidate_context(
            Some("project-a".into()),
            vec!["rust".into()],
            true,
            false,
        );
        let texts = active
            .snapshot()
            .candidates
            .into_iter()
            .map(|candidate| candidate.text)
            .collect::<Vec<_>>();
        assert!(texts.contains(&"所有权".to_string()));
        assert!(!texts.contains(&"所有权设计".to_string()));
    }

    #[test]
    fn secondary_layer_never_uses_typo_or_raw_ascii_as_filler() {
        let mut mixer = MixerDecoder::new(
            Box::new(ListDecoder::with("niha", &[("拟哈", "niha")])),
            "app".into(),
        );
        mixer.configure_typo(true);
        let enabled = mixer.snapshot();
        assert_eq!(enabled.candidates[0].text, "拟哈");
        assert!(enabled.secondary_candidates.is_empty());
        mixer.configure_candidate_context(None, Vec::new(), false, false);
        let disabled = mixer.snapshot();
        assert_eq!(disabled.candidates[0].text, "拟哈");
        assert!(disabled.secondary_candidates.is_empty());
    }

    #[test]
    fn local_prediction_is_opt_in_secondary_only_and_never_takes_numbered_slots() {
        let mut prediction = entry_json("predict-1", "你好世界", "nihao", false, false, false, &[]);
        prediction["candidate_layer"] = serde_json::json!("secondary");
        prediction["sources"][0]["source_kind"] = serde_json::json!("prediction:fixture");
        let mut mixer = MixerDecoder::new(
            Box::new(ListDecoder::with("ni", &[("你", "ni"), ("尼", "ni")])),
            "prediction".into(),
        );
        mixer.apply_lexicon(lexicon(vec![prediction]));

        mixer.configure_candidate_context(None, Vec::new(), true, false);
        let disabled = mixer.snapshot();
        assert_eq!(disabled.candidates[0].text, "你");
        assert!(disabled.secondary_candidates.is_empty());

        mixer.configure_candidate_context(None, Vec::new(), true, true);
        let enabled = mixer.snapshot();
        assert_eq!(enabled.candidates[0].text, "你");
        assert_eq!(enabled.secondary_candidates[0].text, "你好世界");
        assert!(enabled.secondary_candidates[0].comment.starts_with("关联"));
        assert_eq!(enabled.secondary_candidates.len(), 1);
        let secondary_before_page = enabled.secondary_candidates.clone();
        mixer.page_down().unwrap();
        assert_eq!(mixer.snapshot().secondary_candidates, secondary_before_page);
        assert_eq!(mixer.select_secondary_candidate(0, 1).unwrap(), "你好世界");
    }

    #[test]
    fn an_associated_phrase_after_a_picked_segment_is_selected_by_the_letters_left() {
        let mut prediction = entry_json("predict-after-pick", "世界和平", "shijie", false, false, false, &[]);
        prediction["candidate_layer"] = serde_json::json!("secondary");
        prediction["sources"][0]["source_kind"] = serde_json::json!("prediction:fixture");
        let mut decoder = ListDecoder::with("nihaoshijie", &[("世界", "shi jie")]);
        decoder.confirmed = "你好".into();
        decoder.remainder = "shi jie".into();
        let mut mixer = MixerDecoder::new(Box::new(decoder), "assoc-after-pick".into());
        mixer.apply_lexicon(lexicon(vec![prediction]));
        mixer.configure_candidate_context(None, Vec::new(), true, true);
        assert_eq!(mixer.snapshot().secondary_candidates[0].text, "世界和平");
        assert_eq!(mixer.select_secondary_candidate(0, 1).unwrap(), "你好世界和平");
    }

    #[test]
    fn secondary_local_selection_clears_digit_extension_raw_override() {
        let mut prediction = entry_json(
            "predict-override",
            "你好世界",
            "nihao",
            false,
            false,
            false,
            &[],
        );
        prediction["candidate_layer"] = serde_json::json!("secondary");
        prediction["sources"][0]["source_kind"] = serde_json::json!("prediction:fixture");
        let mut mixer = MixerDecoder::new(
            Box::new(ListDecoder::with("", &[("你", "ni")])),
            "secondary-override".into(),
        );
        mixer.apply_lexicon(lexicon(vec![prediction]));
        mixer.configure_candidate_context(None, Vec::new(), true, true);
        mixer.input("n").unwrap();
        mixer.input("i").unwrap();
        // Same digit-extension entry point as the base64 golden test at the
        // session level: digits after typing build raw_override.
        mixer.input("6").unwrap();
        mixer.input("4").unwrap();
        assert_eq!(mixer.raw_override.as_deref(), Some("ni64"));

        assert_eq!(mixer.select_secondary_candidate(0, 1).unwrap(), "你好世界");
        assert!(mixer.raw_override.is_none());

        mixer.input("h").unwrap();
        let snapshot = mixer.snapshot();
        assert_eq!(snapshot.preedit, "h");
        assert_eq!(snapshot.raw_input, "h");
        assert_eq!(mixer.commit_raw(), "h");
    }

    #[test]
    fn confident_auto_profile_promotes_exact_and_can_complete_in_prediction_layer() {
        let mut auto = entry_json(
            "profile-auto",
            "我怎么知道",
            "wozenmzhidao",
            false,
            false,
            false,
            &[],
        );
        auto["sources"][0]["source_kind"] = serde_json::json!("personal-friction-auto");
        auto["base_weight"] = serde_json::json!(520);
        let lexicon_bytes = lexicon(vec![auto]);

        let mut exact = MixerDecoder::new(
            Box::new(ListDecoder::with(
                "wozenmzhidao",
                &[
                    ("我怎么指导", "wozenmzhidao"),
                    ("我怎么知道", "wozenmzhidao"),
                ],
            )),
            "profile-auto-exact".into(),
        );
        exact.apply_lexicon(lexicon_bytes.clone());
        exact.configure_candidate_context(None, Vec::new(), true, true);
        let exact_snapshot = exact.snapshot();
        assert_eq!(exact_snapshot.candidates[0].text, "我怎么知道");
        assert!(exact_snapshot.secondary_candidates.is_empty());

        let mut prefix = MixerDecoder::new(
            Box::new(ListDecoder::with("wozenm", &[("我怎么", "wozenm")])),
            "profile-auto-prefix".into(),
        );
        prefix.apply_lexicon(lexicon_bytes);
        prefix.configure_candidate_context(None, Vec::new(), true, true);
        let prefix_snapshot = prefix.snapshot();
        assert_eq!(prefix_snapshot.candidates[0].text, "我怎么");
        assert_eq!(prefix_snapshot.secondary_candidates[0].text, "我怎么知道");
    }

    #[test]
    fn owner_declared_disambiguation_promotes_the_matching_long_phrase() {
        // 他怎么知道 is in no dictionary (我怎么知道 is in the spoken layer and would lead on its own).
        let base = || {
            Box::new(ListDecoder::with(
                "tazenmzhidao",
                &[
                    ("他怎么指导", "tazenmzhidao"),
                    ("他怎么知道", "tazenmzhidao"),
                ],
            )) as Box<dyn Decoder>
        };
        let baseline = MixerDecoder::new(base(), "owner-declared-baseline".into()).snapshot();
        assert_eq!(baseline.candidates[0].text, "他怎么指导");

        let mut preference = entry_json(
            "profile-auto:zhidao:知道",
            "知道",
            "zhidao",
            false,
            false,
            false,
            &[],
        );
        preference["sources"][0]["source_kind"] =
            serde_json::json!("owner-declared-disambiguation");
        preference["sources"][0]["source"] =
            serde_json::json!("owner_declared_disambiguation#manager-panel");
        preference["base_weight"] = serde_json::json!(520);

        let mut declared = MixerDecoder::new(base(), "owner-declared-active".into());
        declared.apply_lexicon(lexicon(vec![preference]));
        assert_eq!(declared.snapshot().candidates[0].text, "他怎么知道");

        declared.apply_lexicon(lexicon(Vec::new()));
        assert_eq!(declared.snapshot().candidates[0].text, "他怎么指导");
    }

    #[test]
    fn personalized_overlay_is_not_repeated_on_later_base_pages() {
        let base = ListDecoder::with_more_pages("ni", &[("你", "ni"), ("泥", "ni")]);
        let mut mixer = MixerDecoder::new(Box::new(base), "paging".into());
        mixer.apply_lexicon(lexicon(vec![entry_json(
            "personal",
            "拟定词",
            "ni",
            false,
            false,
            false,
            &[],
        )]));
        assert_eq!(mixer.snapshot().candidates[0].text, "拟定词");
        mixer.page_down().unwrap();
        assert!(
            mixer
                .snapshot()
                .candidates
                .iter()
                .all(|item| item.text != "拟定词")
        );
    }

    #[test]
    fn deep_page_fallback_preserves_first_two_pages_and_full_set_without_duplicates() {
        let values = [
            ("整句00", "wo xiang chi fan"),
            ("整句01", "wo xiang chi fan"),
            ("整句02", "wo xiang chi fan"),
            ("整句03", "wo xiang chi fan"),
            ("整句04", "wo xiang chi fan"),
            ("整句05", "wo xiang chi fan"),
            ("整句06", "wo xiang chi fan"),
            ("整句07", "wo xiang chi fan"),
            ("整句08", "wo xiang chi fan"),
            ("整句09", "wo xiang chi fan"),
            ("组合10", "wo xiang chi fan"),
            ("我", "wo"),
            ("组合12", "wo xiang chi fan"),
            ("窝", "wo"),
            ("组合14", "wo xiang chi fan"),
            ("组合15", "wo xiang chi fan"),
            ("握", "wo"),
            ("组合17", "wo xiang chi fan"),
            ("组合18", "wo xiang chi fan"),
            ("组合19", "wo xiang chi fan"),
        ];
        let original = values
            .iter()
            .map(|value| value.0.to_string())
            .collect::<Vec<_>>();
        let (decoder, selections) = DeepPagedDecoder::new("woxiangchifan", &values);
        let mut mixer = MixerDecoder::new(Box::new(decoder), "deep-pages".into());
        let mut prediction = entry_json(
            "deep-secondary",
            "我想吃饭以后",
            "woxiangchifanyihou",
            false,
            false,
            false,
            &[],
        );
        prediction["candidate_layer"] = serde_json::json!("secondary");
        prediction["sources"][0]["source_kind"] = serde_json::json!("prediction:fixture");
        mixer.apply_lexicon(lexicon(vec![prediction]));
        mixer.configure_candidate_context(None, Vec::new(), true, true);

        let first_snapshot = mixer.snapshot();
        let secondary = first_snapshot.secondary_candidates.clone();
        let first = first_snapshot.candidates;
        assert_eq!(
            first
                .iter()
                .map(|item| item.text.as_str())
                .collect::<Vec<_>>(),
            values[..5].iter().map(|item| item.0).collect::<Vec<_>>()
        );
        mixer.page_down().unwrap();
        let second = mixer.snapshot().candidates;
        assert_eq!(
            second
                .iter()
                .map(|item| item.text.as_str())
                .collect::<Vec<_>>(),
            values[5..10].iter().map(|item| item.0).collect::<Vec<_>>()
        );
        mixer.page_down().unwrap();
        let third = mixer.snapshot();
        assert_eq!(third.page_no, 2);
        assert_eq!(third.secondary_candidates, secondary);
        assert!(
            third
                .candidates
                .iter()
                .take(3)
                .all(|item| item.text.chars().count() == 1),
            "third page should prioritize first-syllable single characters: {:?}",
            third.candidates
        );

        let mut after = first
            .into_iter()
            .chain(second)
            .map(|item| item.text)
            .collect::<Vec<_>>();
        loop {
            let page = mixer.snapshot();
            after.extend(page.candidates.iter().map(|item| item.text.clone()));
            if page.is_last_page {
                break;
            }
            mixer.page_down().unwrap();
        }
        assert_eq!(
            after.iter().cloned().collect::<HashSet<_>>(),
            original.iter().cloned().collect::<HashSet<_>>()
        );
        assert_eq!(after.len(), after.iter().collect::<HashSet<_>>().len());

        while mixer.snapshot().page_no > 2 {
            mixer.page_up().unwrap();
        }
        assert_eq!(
            mixer
                .select_candidate(0, 41, LearningIntent::DeliberateSelection)
                .unwrap(),
            "我"
        );
        assert_eq!(*selections.lock().unwrap(), vec![11]);
    }

    #[test]
    fn short_reading_keeps_every_native_page_unchanged() {
        let values = [
            ("甲0", "ni hao"),
            ("甲1", "ni hao"),
            ("甲2", "ni hao"),
            ("甲3", "ni hao"),
            ("甲4", "ni hao"),
            ("甲5", "ni hao"),
            ("甲6", "ni hao"),
            ("甲7", "ni hao"),
            ("甲8", "ni hao"),
            ("甲9", "ni hao"),
            ("你", "ni"),
            ("甲11", "ni hao"),
        ];
        let (decoder, _) = DeepPagedDecoder::new("nihao", &values);
        let mut mixer = MixerDecoder::new(Box::new(decoder), "short-pages".into());
        for expected_page in 0..3 {
            let page = mixer.snapshot();
            assert_eq!(page.page_no, expected_page);
            assert_eq!(
                page.candidates
                    .iter()
                    .map(|item| item.text.as_str())
                    .collect::<Vec<_>>(),
                values
                    .iter()
                    .skip(expected_page * PAGE_SIZE)
                    .take(PAGE_SIZE)
                    .map(|item| item.0)
                    .collect::<Vec<_>>()
            );
            mixer.page_down().unwrap();
        }
    }

    #[test]
    fn peeking_following_pages_matches_paging_and_leaves_the_page_alone() {
        let values = (0..17)
            .map(|index| (format!("甲{index}"), "ni hao"))
            .collect::<Vec<_>>();
        let borrowed = values.iter().map(|(t, r)| (t.as_str(), *r)).collect::<Vec<_>>();
        let (decoder, _) = DeepPagedDecoder::new("nihao", &borrowed);
        let mut mixer = MixerDecoder::new(Box::new(decoder), "peek".into());
        let before = mixer.snapshot();
        let pages = mixer.peek_following_pages(3);
        let texts = |page: &Vec<Candidate>| page.iter().map(|c| c.text.clone()).collect::<Vec<_>>();
        assert_eq!(pages.len(), 3);
        for (offset, page) in pages.iter().enumerate() {
            let expected = values
                .iter()
                .skip((offset + 1) * PAGE_SIZE)
                .take(PAGE_SIZE)
                .map(|(t, _)| t.clone())
                .collect::<Vec<_>>();
            assert_eq!(texts(page), expected, "page {}", offset + 1);
        }
        let after = mixer.snapshot();
        assert_eq!(after.page_no, before.page_no);
        assert_eq!(texts(&after.candidates), texts(&before.candidates));
        assert_eq!(after.highlighted_index, before.highlighted_index);
        // Fewer pages than asked: stops at the last page.
        assert_eq!(mixer.peek_following_pages(3).len(), 3);
    }

    /// A decoder with real librime menu semantics: Page_Up/Page_Down keep the
    /// highlighted column, Down on the last candidate rolls into the next page
    /// (highlight back to 0), Up on the first candidate rolls into the previous
    /// one (highlight on its last column).
    struct RimeMenuDecoder {
        raw: String,
        candidates: Vec<Candidate>,
        page_no: usize,
        highlighted: usize,
    }

    impl RimeMenuDecoder {
        fn new(raw: &str, values: &[(&str, &str)]) -> Self {
            Self {
                raw: raw.into(),
                candidates: values
                    .iter()
                    .enumerate()
                    .map(|(index, (text, reading))| Candidate {
                        text: (*text).into(),
                        comment: String::new(),
                        reading_key: (*reading).into(),
                        source_index: Some(index),
                    })
                    .collect(),
                page_no: 0,
                highlighted: 0,
            }
        }

        fn page_len(&self) -> usize {
            self.snapshot().candidates.len()
        }
    }

    impl Decoder for RimeMenuDecoder {
        fn snapshot(&self) -> DecoderSnapshot {
            let start = self.page_no * PAGE_SIZE;
            let end = (start + PAGE_SIZE).min(self.candidates.len());
            DecoderSnapshot {
                raw_input: self.raw.clone(),
                preedit: self.raw.clone(),
                cursor_utf8: self.raw.len(),
                candidates: self.candidates[start..end].to_vec(),
                secondary_candidates: Vec::new(),
                highlighted_index: self.highlighted.min(end.saturating_sub(start + 1)),
                page_no: self.page_no,
                is_last_page: end >= self.candidates.len(),
            }
        }

        fn snapshot_all_candidates(&self) -> Option<DecoderSnapshot> {
            let mut snapshot = self.snapshot();
            snapshot.candidates = self.candidates.clone();
            snapshot.page_no = 0;
            snapshot.is_last_page = true;
            Some(snapshot)
        }

        fn input(&mut self, text: &str) -> Result<String, i32> {
            self.raw.push_str(text);
            self.page_no = 0;
            self.highlighted = 0;
            Ok(String::new())
        }
        fn backspace(&mut self) -> Result<String, i32> {
            self.raw.pop();
            self.page_no = 0;
            self.highlighted = 0;
            Ok(String::new())
        }
        fn move_left(&mut self) -> Result<String, i32> {
            Ok(String::new())
        }
        fn move_right(&mut self) -> Result<String, i32> {
            Ok(String::new())
        }
        fn page_up(&mut self) -> Result<String, i32> {
            self.page_no = self.page_no.saturating_sub(1);
            Ok(String::new())
        }
        fn page_down(&mut self) -> Result<String, i32> {
            if (self.page_no + 1) * PAGE_SIZE < self.candidates.len() {
                self.page_no += 1;
            }
            Ok(String::new())
        }
        fn select_previous(&mut self) -> Result<String, i32> {
            if self.highlighted > 0 {
                self.highlighted -= 1;
            } else if self.page_no > 0 {
                self.page_no -= 1;
                self.highlighted = self.page_len().saturating_sub(1);
            }
            Ok(String::new())
        }
        fn select_next(&mut self) -> Result<String, i32> {
            if self.highlighted + 1 < self.page_len() {
                self.highlighted += 1;
            } else if (self.page_no + 1) * PAGE_SIZE < self.candidates.len() {
                self.page_no += 1;
                self.highlighted = 0;
            }
            Ok(String::new())
        }
        fn select_candidate(
            &mut self,
            index: usize,
            _delivery_id: u64,
            _learning_intent: LearningIntent,
        ) -> Result<String, i32> {
            let global = self.page_no * PAGE_SIZE + index;
            let text = self.candidates.get(global).ok_or(-10)?.text.clone();
            self.raw.clear();
            self.page_no = 0;
            self.highlighted = 0;
            Ok(text)
        }
        fn delete_candidate(&mut self, index: usize) -> Result<(), i32> {
            ((self.page_no * PAGE_SIZE + index) < self.candidates.len())
                .then_some(())
                .ok_or(-10)
        }
        fn commit_raw(&mut self) -> String {
            self.raw.clone()
        }
        fn clear(&mut self) {
            self.raw.clear();
            self.page_no = 0;
            self.highlighted = 0;
        }
    }

    #[test]
    fn peek_following_pages_from_a_kept_column_returns_to_the_same_page_and_highlight() {
        // 55 candidates = 11 full pages; the session sits on page 7 with the
        // highlighted column kept at 4 (the state the expanded panel reaches by
        // Down + Page_Down keys). Peeking must not move it to another page.
        let values = (0..55)
            .map(|index| (format!("词{index:02}"), "ni hao"))
            .collect::<Vec<_>>();
        let borrowed = values
            .iter()
            .map(|(text, reading)| (text.as_str(), *reading))
            .collect::<Vec<_>>();
        let mut mixer = MixerDecoder::new(
            Box::new(RimeMenuDecoder::new("nihao", &borrowed)),
            "peek-kept-column".into(),
        );
        for _ in 0..4 {
            mixer.select_next().unwrap();
        }
        for _ in 0..7 {
            mixer.page_down().unwrap();
        }
        let before = mixer.snapshot();
        assert_eq!(before.page_no, 7);
        assert_eq!(before.highlighted_index, 4);
        let pages = mixer.peek_following_pages(3);
        let texts = |page: &Vec<Candidate>| page.iter().map(|c| c.text.clone()).collect::<Vec<_>>();
        assert_eq!(pages.len(), 3);
        for (offset, page) in pages.iter().enumerate() {
            let expected = values
                .iter()
                .skip((offset + 8) * PAGE_SIZE)
                .take(PAGE_SIZE)
                .map(|(text, _)| text.clone())
                .collect::<Vec<_>>();
            assert_eq!(texts(page), expected, "page {}", offset + 8);
        }
        let after = mixer.snapshot();
        assert_eq!(after.page_no, before.page_no, "peek must return to page 7");
        assert_eq!(
            after.highlighted_index, before.highlighted_index,
            "peek must restore the kept highlight column"
        );
        // The peeked pages are exactly the pages continuing down from the state
        // the session was left in.
        for page in &pages {
            mixer.page_down().unwrap();
            assert_eq!(texts(&mixer.snapshot().candidates), texts(page));
        }
    }

    #[test]
    fn fuzzy_and_deep_selection_clamps_within_current_page() {
        let values = (0..25).map(|index| (format!("整句{index:02}"), "wo xiang chi fan")).collect::<Vec<_>>();
        let borrowed = values.iter().map(|(text, reading)| (text.as_str(), *reading)).collect::<Vec<_>>();
        for deep in [false, true] {
            let decoder: Box<dyn Decoder> = if deep {
                Box::new(RimeMenuDecoder::new("woxiangchifan", &borrowed))
            } else {
                Box::new(ListDecoder::with("woxiangchifan", &borrowed))
            };
            let mut mixer = MixerDecoder::new(decoder, format!("clamped-selection-{deep}"));
            if deep {
                mixer.page_down().unwrap();
                mixer.page_down().unwrap();
                assert_eq!(mixer.deep_page_no, Some(2));
            } else {
                mixer.configure_fuzzy(1).unwrap();
                mixer.page_down().unwrap();
                assert_ne!(mixer.fuzzy_mask, 0);
            }
            let before = mixer.snapshot();
            assert!(before.page_no > 0);
            assert!(!before.is_last_page);
            for _ in 0..10 { mixer.select_previous().unwrap(); }
            assert_eq!(mixer.snapshot().highlighted_index, 0);
            assert_eq!(mixer.snapshot().page_no, before.page_no);
            assert_eq!(mixer.snapshot().candidates, before.candidates);
            for _ in 0..10 { mixer.select_next().unwrap(); }
            assert_eq!(mixer.snapshot().highlighted_index, before.candidates.len() - 1);
            assert_eq!(mixer.snapshot().page_no, before.page_no);
            assert_eq!(mixer.snapshot().candidates, before.candidates);
        }
    }

    #[test]
    fn fuzzy_linear_selection_crosses_pages_in_both_directions() {
        let values = (0..14).map(|index| (format!("词{index:02}"), "shi")).collect::<Vec<_>>();
        let borrowed = values.iter().map(|(text, reading)| (text.as_str(), *reading)).collect::<Vec<_>>();
        let mut mixer = MixerDecoder::new(Box::new(ListDecoder::with("shi", &borrowed)), "fuzzy-linear".into());
        mixer.configure_fuzzy(1).unwrap();
        assert_linear_selection_pages(&mut mixer, 0);
    }

    #[test]
    fn deep_linear_selection_crosses_pages_and_returns_to_native_page() {
        let values = (0..25).map(|index| (format!("整句{index:02}"), "wo xiang chi fan")).collect::<Vec<_>>();
        let borrowed = values.iter().map(|(text, reading)| (text.as_str(), *reading)).collect::<Vec<_>>();
        let mut mixer = MixerDecoder::new(Box::new(RimeMenuDecoder::new("woxiangchifan", &borrowed)), "deep-linear".into());
        mixer.page_down().unwrap();
        mixer.page_down().unwrap();
        assert_eq!(mixer.deep_page_no, Some(2));
        assert_linear_selection_pages(&mut mixer, 2);
        // Crossing the beginning of the deep list restores the native second page.
        for _ in 0..5 { mixer.select_previous_linear().unwrap(); }
        assert_eq!(mixer.deep_page_no, None);
        assert_eq!(mixer.snapshot().page_no, 1);
        assert_eq!(mixer.snapshot().highlighted_index, 4);
        // The forward transition must enter Mixer paging, not native page 2.
        mixer.select_next_linear().unwrap();
        assert_eq!(mixer.deep_page_no, Some(2));
        assert_eq!(mixer.snapshot().highlighted_index, 0);
        mixer.select_previous_linear().unwrap();
        assert_eq!(mixer.deep_page_no, None);
        assert_eq!(mixer.snapshot().page_no, 1);
        assert_eq!(mixer.snapshot().highlighted_index, 4);
    }

    fn assert_linear_selection_pages(mixer: &mut MixerDecoder, first_page: usize) {
        let first = mixer.snapshot().candidates;
        let expected = mixer.peek_following_pages(1);
        assert_eq!(expected.len(), 1);
        for _ in 0..5 { mixer.select_next_linear().unwrap(); }
        assert_eq!(mixer.snapshot().page_no, first_page + 1);
        assert_eq!(mixer.snapshot().highlighted_index, 0);
        assert_eq!(mixer.snapshot().candidates, expected[0]);
        mixer.select_previous_linear().unwrap();
        assert_eq!(mixer.snapshot().page_no, first_page);
        assert_eq!(mixer.snapshot().highlighted_index, 4);
        assert_eq!(mixer.snapshot().candidates, first);
    }

    #[test]
    fn linear_return_does_not_reapply_host_reordering_to_landing_page() {
        let values = [("时", "shi"), ("十", "shi"), ("石", "shi"), ("试", "shi"), ("式", "shi"),
                      ("室", "shi"), ("是", "shi"), ("事", "shi"), ("市", "shi"), ("诗", "shi"),
                      ("师", "shi"), ("史", "shi"), ("识", "shi"), ("世", "shi"), ("实", "shi")];
        for fuzzy in [false, true] {
            let decoder: Box<dyn Decoder> = if fuzzy { Box::new(ListDecoder::with("shi", &values)) }
                else { Box::new(RimeMenuDecoder::new("shi", &values)) };
            let mut mixer = MixerDecoder::new(decoder, format!("linear-return-order-{fuzzy}"));
            if fuzzy { mixer.configure_fuzzy(1).unwrap(); }
            let first = mixer.snapshot().candidates;
            mixer.reorder_context_candidates(&[4, 3, 2, 1, 0]).unwrap();
            assert_ne!(mixer.snapshot().candidates, first);
            for _ in 0..20 {
                if mixer.snapshot().page_no == 2 { break; }
                mixer.select_next_linear().unwrap();
            }
            assert_eq!(mixer.snapshot().page_no, 2);
            assert_eq!(mixer.snapshot().highlighted_index, 0);
            for _ in 0..20 {
                if mixer.snapshot().page_no == 0 { break; }
                mixer.select_previous_linear().unwrap();
            }
            let landed = mixer.snapshot();
            assert_eq!(landed.page_no, 0);
            assert_eq!(landed.highlighted_index, 4);
            assert_eq!(landed.candidates, first);
            assert_eq!(mixer.select_candidate(4, 0, LearningIntent::NoLearn).unwrap(), first[4].text);
        }
    }

    #[test]
    fn fuzzy_explicit_paging_crosses_pages_in_both_directions() {
        let values = (0..14).map(|index| (format!("词{index:02}"), "shi")).collect::<Vec<_>>();
        let borrowed = values.iter().map(|(text, reading)| (text.as_str(), *reading)).collect::<Vec<_>>();
        let mut mixer = MixerDecoder::new(Box::new(ListDecoder::with("shi", &borrowed)), "fuzzy-linear".into());
        mixer.configure_fuzzy(1).unwrap();
        assert_linear_pages(&mut mixer, 0);
    }

    #[test]
    fn deep_explicit_paging_crosses_pages_and_returns_to_native_page() {
        let values = (0..25).map(|index| (format!("整句{index:02}"), "wo xiang chi fan")).collect::<Vec<_>>();
        let borrowed = values.iter().map(|(text, reading)| (text.as_str(), *reading)).collect::<Vec<_>>();
        let mut mixer = MixerDecoder::new(Box::new(RimeMenuDecoder::new("woxiangchifan", &borrowed)), "deep-linear".into());
        mixer.page_down().unwrap();
        mixer.page_down().unwrap();
        assert_eq!(mixer.deep_page_no, Some(2));
        assert_linear_pages(&mut mixer, 2);
        // Crossing the beginning of the deep list restores the native second page.
        for _ in 0..5 { step_linear_page(&mut mixer, false); }
        assert_eq!(mixer.deep_page_no, None);
        assert_eq!(mixer.snapshot().page_no, 1);
        assert_eq!(mixer.snapshot().highlighted_index, 4);
        // The forward transition must enter Mixer paging, not native page 2.
        step_linear_page(&mut mixer, true);
        assert_eq!(mixer.deep_page_no, Some(2));
        assert_eq!(mixer.snapshot().highlighted_index, 0);
        step_linear_page(&mut mixer, false);
        assert_eq!(mixer.deep_page_no, None);
        assert_eq!(mixer.snapshot().page_no, 1);
        assert_eq!(mixer.snapshot().highlighted_index, 4);
    }

    // Keep covering the existing explicit pager independently of linear keys.
    // Ordinary selection remains clamped in fuzzy/deep rows.
    fn step_linear_page(mixer: &mut MixerDecoder, forward: bool) {
        let before = mixer.snapshot();
        let target = if forward && before.highlighted_index + 1 == before.candidates.len() {
            mixer.page_down().unwrap();
            0
        } else if !forward && before.highlighted_index == 0 {
            mixer.page_up().unwrap();
            mixer.snapshot().candidates.len().saturating_sub(1)
        } else {
            if forward { mixer.select_next().unwrap(); } else { mixer.select_previous().unwrap(); }
            return;
        };
        let arrived = mixer.snapshot().highlighted_index;
        for _ in target..arrived { mixer.select_previous().unwrap(); }
        for _ in arrived..target { mixer.select_next().unwrap(); }
    }

    fn assert_linear_pages(mixer: &mut MixerDecoder, first_page: usize) {
        let first = mixer.snapshot().candidates;
        let expected = mixer.peek_following_pages(1);
        assert_eq!(expected.len(), 1);
        for _ in 0..5 { step_linear_page(mixer, true); }
        assert_eq!(mixer.snapshot().page_no, first_page + 1);
        assert_eq!(mixer.snapshot().highlighted_index, 0);
        assert_eq!(mixer.snapshot().candidates, expected[0]);
        step_linear_page(mixer, false);
        assert_eq!(mixer.snapshot().page_no, first_page);
        assert_eq!(mixer.snapshot().highlighted_index, 4);
        assert_eq!(mixer.snapshot().candidates, first);
    }

    #[test]
    fn fuzzy_long_reading_keeps_first_two_pages_and_prioritizes_deep_single_characters() {
        let values = [
            ("整0", "wo xiang chi fan"),
            ("整1", "wo xiang chi fan"),
            ("整2", "wo xiang chi fan"),
            ("整3", "wo xiang chi fan"),
            ("整4", "wo xiang chi fan"),
            ("整5", "wo xiang chi fan"),
            ("整6", "wo xiang chi fan"),
            ("整7", "wo xiang chi fan"),
            ("整8", "wo xiang chi fan"),
            ("整9", "wo xiang chi fan"),
            ("组合", "wo xiang chi fan"),
            ("我", "wo"),
            ("窝", "wo"),
            ("后段", "wo xiang chi fan"),
            ("握", "wo"),
        ];
        let mut mixer = MixerDecoder::new(
            Box::new(ListDecoder::with("woxiangchifan", &values)),
            "fuzzy-deep".into(),
        );
        mixer.fuzzy_mask = 1;
        let first = mixer.snapshot();
        assert_eq!(
            first
                .candidates
                .iter()
                .map(|item| item.text.as_str())
                .collect::<Vec<_>>(),
            values[..5].iter().map(|item| item.0).collect::<Vec<_>>()
        );
        mixer.page_down().unwrap();
        let second = mixer.snapshot();
        assert_eq!(
            second
                .candidates
                .iter()
                .map(|item| item.text.as_str())
                .collect::<Vec<_>>(),
            values[5..10].iter().map(|item| item.0).collect::<Vec<_>>()
        );
        mixer.page_down().unwrap();
        assert!(
            mixer
                .snapshot()
                .candidates
                .iter()
                .take(3)
                .all(|item| item.text.chars().count() == 1)
        );
    }

    #[test]
    fn fuzzy_abbreviation_keeps_the_exact_spelling_word_ahead_of_the_fuzzy_derivation() {
        // fuzzy-exact-priority-v1.json abbreviation-control-zenm, build 1743: with fuzzy
        // pinyin on, 榛蘑 (zhen mo, a z->zh derivation the post-July layers added) leads
        // over 怎么 because its reading key is the raw input echoed back — the runtime
        // index lacks it — and the exact protection took the echo for an exact spelling.
        let values = [
            ("榛蘑", "zenm"),
            ("怎么", "zen me"),
            ("曾", "zeng"),
            ("增", "zeng"),
            ("赠", "zeng"),
        ];
        let mut mixer = MixerDecoder::new(
            Box::new(ListDecoder::with("zenm", &values)),
            "fuzzy-abbrev".into(),
        );
        mixer.fuzzy_mask = 231;
        let page = mixer.snapshot();
        let texts: Vec<&str> = page.candidates.iter().map(|item| item.text.as_str()).collect();
        assert_eq!(texts.first(), Some(&"怎么"));
        // The fuzzy derivation stays reachable on the page, ranked by weight.
        assert!(texts.contains(&"榛蘑"));
    }

    #[test]
    fn fuzzy_exact_protection_keeps_a_post_july_exact_word_first() {
        // fuzzy-exact-priority-v1.json exact-over-fuzzy-congzheli: 从这里 is only in the
        // post-July layers, so its reading key is the raw input echoed back; its true
        // reading is the exact spelling and must keep the protected first slot.
        let values = [("从这里", "congzheli"), ("总这里", "cong zheli"), ("充这里", "cong zheli")];
        let mut mixer = MixerDecoder::new(
            Box::new(ListDecoder::with("congzheli", &values)),
            "fuzzy-exact".into(),
        );
        mixer.fuzzy_mask = 231;
        assert_eq!(mixer.snapshot().candidates.first().map(|item| item.text.clone()), Some("从这里".into()));
    }

    #[test]
    fn one_hundred_personal_queries_meet_the_frozen_top_one_threshold() {
        let query_for = |index: usize| {
            let first = char::from(b'a' + ((index / 26) as u8));
            let second = char::from(b'a' + ((index % 26) as u8));
            format!("p{first}{second}")
        };
        let entries = (0..100)
            .map(|index| {
                entry_json(
                    &format!("entry-{index}"),
                    &format!("个人词{index}"),
                    &query_for(index),
                    false,
                    false,
                    false,
                    &[],
                )
            })
            .collect::<Vec<_>>();
        let shared = lexicon(entries);
        let hits = (0..100)
            .filter(|index| {
                let query = query_for(*index);
                let base_text = format!("基础词{index}");
                let values = [(base_text.as_str(), query.as_str())];
                let mut mixer = MixerDecoder::new(
                    Box::new(ListDecoder::with(&query, &values)),
                    "personal".into(),
                );
                mixer.apply_lexicon(shared.clone());
                mixer
                    .snapshot()
                    .candidates
                    .first()
                    .map(|item| item.text.clone())
                    == Some(format!("个人词{index}"))
            })
            .count();
        assert_eq!(hits, 100, "personal top-1 hits={hits}/100");
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct TypoSet {
        cases: Vec<TypoCase>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct TypoCase {
        typo_query: String,
        allowed: Vec<String>,
    }

    #[test]
    fn frozen_typo_set_meets_eighty_percent_hit_rate_deterministically() {
        let corpus: TypoSet = serde_json::from_str(include_str!(
            "../../../../quality/corpus/typo-correction-v1.json"
        ))
        .unwrap();
        let hits = corpus
            .cases
            .iter()
            .filter(|case| {
                let first = typo_candidates(&case.typo_query);
                let second = typo_candidates(&case.typo_query);
                first == second
                    && first
                        .iter()
                        .any(|candidate| case.allowed.contains(&candidate.text))
            })
            .count();
        eprintln!("frozen typo hit rate: {hits}/{}", corpus.cases.len());
        assert!(
            hits * 100 / corpus.cases.len() >= 80,
            "hits={hits}/{}",
            corpus.cases.len()
        );
    }

    #[test]
    fn typo_enabled_has_zero_base_first_layer_regressions() {
        let corpus: BaseCorpus =
            serde_json::from_str(include_str!("../../../../quality/corpus/base-500.json")).unwrap();
        let regressions = corpus
            .cases
            .iter()
            .filter_map(|case| {
                let values = case
                    .allowed
                    .iter()
                    .map(|text| (text.as_str(), case.query.as_str()))
                    .collect::<Vec<_>>();
                let expected = values.first().map(|(text, _)| (*text).to_string());
                let mut mixer = MixerDecoder::new(
                    Box::new(ListDecoder::with(&case.query, &values)),
                    "typo-base".into(),
                );
                mixer.configure_typo(true);
                (mixer
                    .snapshot()
                    .candidates
                    .first()
                    .map(|item| item.text.clone())
                    != expected)
                    .then_some(case.query.as_str())
            })
            .collect::<Vec<_>>();
        assert!(
            regressions.is_empty(),
            "typo first-layer regressions={}/500: {regressions:?}",
            regressions.len()
        );
    }

    #[test]
    fn large_lexicon_keystroke_pipeline_meets_frozen_latency_budget() {
        // Synthetic LOAD corpus (scale only, not a quality golden set — §7.3
        // applies to acceptance replay corpora): 50,000 personal entries at the
        // product's supported scale. Five warm measurement rounds use the
        // median round P95/P99 so a single scheduler spike cannot decide the
        // gate. Release enforces the frozen P95 ≤ 2.5 ms / P99 ≤ 5 ms budget.
        // Debug only enforces a deliberately non-frozen gross-regression bound;
        // test_all.sh separately runs this exact test in release mode.
        fn synthetic_reading(index: usize) -> String {
            let mut value = index;
            let mut reading = String::from("zz");
            for _ in 0..6 {
                reading.push(char::from(b'a' + (value % 26) as u8));
                value /= 26;
            }
            reading
        }
        const ENTRY_COUNT: usize = 50_000;
        let entries: Vec<serde_json::Value> = (0..ENTRY_COUNT)
            .map(|index| {
                serde_json::json!({
                    "id": format!("scale-{index}"),
                    "text": format!("规模词{index}"),
                    "pinyin": synthetic_reading(index),
                    "aliases": [],
                    "professional_packs": [],
                    "project_domains": [],
                    "sources": [{
                        "source_kind": "manual",
                        "source": "owner",
                        "license": "private",
                        "confidence_millis": 1000
                    }],
                    "base_weight": 100 + (index % 100) as i64,
                    "fixed": index % 1000 == 0,
                    "disabled": false,
                    "demoted": false,
                    "candidate_layer": if index % 10 == 0 { "secondary" } else { "first" },
                    "updated_at_ms": 1
                })
            })
            .collect();
        let shared = lexicon(entries);
        let hit_reading = synthetic_reading(4_321);
        let miss_reading = "zzzzzzzq";
        // 64 distinct base candidates model a large decoding pool, and 50
        // learned promotions keep apply_bounded_promotions on the timed path.
        let base_texts: Vec<String> = (0..64).map(|i| format!("基候选{i}")).collect();
        let mut mixers = Vec::new();
        for reading in [hit_reading.as_str(), miss_reading] {
            let base_values: Vec<(&str, &str)> = base_texts
                .iter()
                .map(|text| (text.as_str(), reading))
                .collect();
            let mut mixer = MixerDecoder::new(
                Box::new(ListDecoder::with(reading, &base_values)),
                "latency-gate".into(),
            );
            mixer.apply_lexicon(shared.clone());
            for index in 0..50 {
                mixer.record_selection(reading, 4 + index % 3, &base_texts[index % 64]);
            }
            // Warm the lazy statics once so the gate times the steady state.
            let _ = mixer.snapshot();
            mixers.push(mixer);
        }
        let mut round_p95_ns = Vec::new();
        let mut round_p99_ns = Vec::new();
        for _ in 0..5 {
            let mut timings_ns: Vec<u128> = Vec::new();
            for mixer in &mixers {
                for _ in 0..100 {
                    let started = std::time::Instant::now();
                    let snapshot = mixer.snapshot();
                    timings_ns.push(started.elapsed().as_nanos());
                    assert!(!snapshot.raw_input.is_empty());
                }
            }
            timings_ns.sort_unstable();
            round_p95_ns.push(timings_ns[(timings_ns.len() * 95 / 100).min(timings_ns.len() - 1)]);
            round_p99_ns.push(timings_ns[(timings_ns.len() * 99 / 100).min(timings_ns.len() - 1)]);
        }
        round_p95_ns.sort_unstable();
        round_p99_ns.sort_unstable();
        let p95 = round_p95_ns[round_p95_ns.len() / 2];
        let p99 = round_p99_ns[round_p99_ns.len() / 2];
        eprintln!(
            "latency-gate: rounds_p95={round_p95_ns:?} rounds_p99={round_p99_ns:?} median_p95={p95}ns median_p99={p99}ns release={}",
            !cfg!(debug_assertions)
        );
        #[cfg(not(debug_assertions))]
        assert!(
            p95 <= 2_500_000,
            "50k-entry pipeline P95 {p95}ns exceeds the frozen 2.5ms budget"
        );
        #[cfg(not(debug_assertions))]
        assert!(
            p99 <= 5_000_000,
            "50k-entry pipeline P99 {p99}ns exceeds the frozen 5ms budget"
        );
        #[cfg(debug_assertions)]
        assert!(
            p95 <= 25_000_000 && p99 <= 50_000_000,
            "50k-entry debug pipeline gross regression: P95={p95}ns P99={p99}ns; this 10x debug-only ceiling is not the frozen release budget"
        );
    }
    fn offered_last_page_mixer() -> MixerDecoder {
        let values = [("甲", "jia"), ("乙", "yi"), ("丙", "bing"), ("丁", "ding"), ("戊", "wu")];
        let mut mixer = MixerDecoder::new(Box::new(RimeMenuDecoder::new("woxuanyixuan", &values)),
            "linear-overflow-round5".into());
        mixer.offer_composition("我选一选").unwrap();
        mixer
    }

    #[test]
    fn linear_selection_reaches_offered_last_page_overflow_and_returns() {
        let mut mixer = offered_last_page_mixer();
        let first = mixer.snapshot();
        assert_eq!(first.candidates.len(), PAGE_SIZE);
        assert!(first.candidates.iter().any(|c| c.text == "我选一选"));
        // Reach the boundary even on the broken implementation so the test
        // independently checks actual navigation as well as the host's last flag.
        for _ in 0..5 { mixer.select_next_linear().unwrap(); }
        let overflow = mixer.snapshot();
        assert_eq!(overflow.page_no, 1);
        assert_eq!(overflow.highlighted_index, 0);
        assert_eq!(overflow.candidates.iter().map(|c| c.text.as_str()).collect::<Vec<_>>(), ["戊"]);
        assert!(overflow.is_last_page);
        assert!(first.is_last_page, "Linear navigation must not change the native last-page flag");
        mixer.select_next_linear().unwrap();
        assert_eq!(mixer.snapshot().page_no, 1);
        assert_eq!(mixer.snapshot().highlighted_index, 0);
        mixer.select_previous_linear().unwrap();
        let returned = mixer.snapshot();
        assert_eq!(returned.page_no, 0);
        assert_eq!(returned.highlighted_index, 4);
        assert_eq!(returned.candidates, first.candidates);
        assert!(returned.is_last_page);
        mixer.select_next_linear().unwrap();
        assert_eq!(mixer.snapshot().candidates, overflow.candidates);
        assert_eq!(mixer.snapshot().highlighted_index, 0);
    }

    #[test]
    fn ordinary_selection_on_offered_last_page_keeps_native_behavior() {
        let mut mixer = offered_last_page_mixer();
        let first = mixer.snapshot().candidates;
        for _ in 0..6 { mixer.select_next().unwrap(); }
        assert_eq!(mixer.snapshot().page_no, 0);
        assert_eq!(mixer.snapshot().highlighted_index, 4);
        assert_eq!(mixer.snapshot().candidates, first);
        for _ in 0..6 { mixer.select_previous().unwrap(); }
        assert_eq!(mixer.snapshot().page_no, 0);
        assert_eq!(mixer.snapshot().highlighted_index, 0);
        assert_eq!(mixer.snapshot().candidates, first);
    }

    #[test]
    fn offered_last_page_snapshot_and_peek_keep_non_linear_semantics() {
        let mut mixer = offered_last_page_mixer();
        let before = mixer.snapshot();
        assert!(before.is_last_page, "native last-page flag must keep its old meaning");
        assert_eq!(before.page_no, 0);
        assert_eq!(before.candidates.len(), PAGE_SIZE);
        assert!(mixer.peek_following_pages(3).is_empty(), "Windows regrid must not add an overflow row");
        let after = mixer.snapshot();
        assert!(after.is_last_page);
        assert_eq!(after.page_no, before.page_no);
        assert_eq!(after.highlighted_index, before.highlighted_index);
        assert_eq!(after.candidates, before.candidates);
    }

    #[test]
    fn linear_overflow_remains_reachable_with_native_last_page_flag() {
        let mut mixer = offered_last_page_mixer();
        let first = mixer.snapshot();
        assert!(first.is_last_page);
        for _ in 0..5 { mixer.select_next_linear().unwrap(); }
        let overflow = mixer.snapshot();
        assert_eq!(overflow.page_no, 1);
        assert_eq!(overflow.highlighted_index, 0);
        assert_eq!(overflow.candidates.iter().map(|c| c.text.as_str()).collect::<Vec<_>>(), ["戊"]);
        mixer.select_previous_linear().unwrap();
        let returned = mixer.snapshot();
        assert_eq!(returned.page_no, 0);
        assert_eq!(returned.highlighted_index, PAGE_SIZE - 1);
        assert_eq!(returned.candidates, first.candidates);
        assert!(returned.is_last_page);
        assert!(mixer.peek_following_pages(3).is_empty());
        mixer.select_next_linear().unwrap();
        assert_eq!(mixer.snapshot().candidates, overflow.candidates);
    }

    #[test]
    fn windows_peek_does_not_expand_the_offered_native_last_page() {
        let mut mixer = offered_last_page_mixer();
        for _ in 0..4 { mixer.select_next().unwrap(); }
        let before = mixer.snapshot();
        assert!(mixer.peek_following_pages(3).is_empty());
        let after = mixer.snapshot();
        assert_eq!(after.page_no, before.page_no);
        assert_eq!(after.highlighted_index, before.highlighted_index);
        assert_eq!(after.candidates, before.candidates);
    }

}


#[cfg(test)]
mod composition_probe {
    /// Local-only owner probe: `AIM_COMPOSE_EVENTS=<events.jsonl> cargo test --release
    /// owner_composition_probe -- --ignored --nocapture` prints `id\traw\tcomps|...`.
    #[test]
    #[ignore]
    fn owner_composition_probe() {
        let path = std::env::var("AIM_COMPOSE_EVENTS").expect("AIM_COMPOSE_EVENTS");
        for line in std::fs::read_to_string(path).unwrap().lines() {
            let Ok(event) = serde_json::from_str::<serde_json::Value>(line) else { continue };
            let raw = event["pinyin"].as_str().unwrap_or_default().to_ascii_lowercase();
            if raw.len() < 6 || !raw.bytes().all(|b| b.is_ascii_lowercase()) {
                continue;
            }
            let abbreviated = !super::valid_pinyin_spelling(&raw);
            if abbreviated != std::env::var_os("AIM_COMPOSE_ABBREV").is_some() {
                continue;
            }
            let comps = super::decode_compositions_with(&raw, 10, abbreviated);
            if comps.is_empty() {
                continue;
            }
            println!("COMP\t{}\t{raw}\t{}", event["id"].as_str().unwrap_or_default(), comps.join("|"));
        }
    }

    /// P1 of LATTICE-NBEST-PLAN, local only: `AIM_P1_CASES=<tsv: bucket id keys target ..> cargo test
    /// --release p1_decoder_rank_probe -- --ignored --nocapture` prints, for each case, the rank of the
    /// target among every segmentation the current decoder yields (or -1) and how many it yields.
    #[test]
    #[ignore]
    fn p1_decoder_rank_probe() {
        let path = std::env::var("AIM_P1_CASES").expect("AIM_P1_CASES");
        // AIM_ZHIXU_SHARED=<Rime shared dir> decodes over the full unified dictionary instead of the fallback.
        let _full = std::env::var_os("AIM_ZHIXU_SHARED").map(|shared| {
            let user = std::env::temp_dir().join("aim-p1-empty-user");
            std::fs::create_dir_all(&user).unwrap();
            let data = crate::zhixu::loader::load(std::path::Path::new(&shared), &user).unwrap();
            crate::zhixu::pin_snapshot(Some(std::sync::Arc::new(crate::zhixu::Index::from_data(data))))
        });
        for line in std::fs::read_to_string(path).unwrap().lines() {
            let cols: Vec<&str> = line.split('\t').collect();
            if cols.len() < 4 || !cols[2].bytes().all(|b| b.is_ascii_lowercase()) {
                continue;
            }
            let (keys, target) = (cols[2], cols[3]);
            let valid = super::valid_pinyin_spelling(keys);
            let all = super::decode_compositions_with(keys, 100_000, !valid);
            let rank = all.iter().position(|text| text == target).map_or(-1, |i| i as i64);
            let lattice = super::decode_lattice(keys, 100_000, !valid);
            let lattice_rank = lattice.iter().position(|text| text == target).map_or(-1, |i| i as i64);
            println!(
                "RANK\t{}\t{}\t{keys}\t{target}\tvalid={valid}\ttotal={}\trank={rank}\tlattice_total={}\tlattice_rank={lattice_rank}",
                cols[0], cols[1], all.len(), lattice.len()
            );
        }
    }

    /// Local timing of both decoders: `cargo test --release lattice_timing -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn lattice_timing() {
        for (keys, abbreviated) in [
            ("gaole", false),
            ("womengaichengxiawuchaliao", false),
            ("duoxianchengdeyilaijiao", false),
            ("jiexialaizuoshenme", false),
            ("zaifageipy", true),
            ("shijuerengyongls", true),
            ("jxlzuoshenmzhegexiangmu", true),
        ] {
            let start = std::time::Instant::now();
            let old = super::decode_compositions_with(keys, 24, abbreviated);
            let old_time = start.elapsed();
            let start = std::time::Instant::now();
            let new = super::decode_lattice(keys, 24, abbreviated);
            println!("TIME\t{keys}\told {:.2}ms ({})\tlattice {:.2}ms ({})", old_time.as_secs_f64() * 1e3, old.len(), start.elapsed().as_secs_f64() * 1e3, new.len());
            if !abbreviated {
                let generate = || super::decode_lattice(keys, super::lattice_params().limit, false).len();
                let start = std::time::Instant::now();
                let count = generate();
                let cold = start.elapsed().as_secs_f64() * 1e3;
                let mut times = Vec::new();
                for _ in 0..25 {
                    let start = std::time::Instant::now();
                    assert_eq!(generate(), count);
                    times.push(start.elapsed().as_secs_f64() * 1e3);
                }
                times.sort_by(f64::total_cmp);
                println!("WHOLE_TIME\t{keys}\tfirst={cold:.3}ms\tmedian={:.3}ms\tp95={:.3}ms\tcount={count}",
                    times[12], times[23]);
            }
        }
    }

    /// A1 of NEXT-ORDER (error edges), local only: `AIM_P1_CASES=<tsv: bucket id keys target ..> cargo test
    /// --release a1_repair_probe -- --ignored --nocapture`. For each case: how many slip repairs the mixer
    /// offers, and whether the target is among the sentences of any of them (old decoder / lattice).
    #[test]
    #[ignore]
    fn a1_repair_probe() {
        let path = std::env::var("AIM_P1_CASES").expect("AIM_P1_CASES");
        for line in std::fs::read_to_string(path).unwrap().lines() {
            let cols: Vec<&str> = line.split('\t').collect();
            if cols.len() < 6 || !cols[2].bytes().all(|b| b.is_ascii_lowercase()) || cols[5] != "tier2" {
                continue;
            }
            let (keys, target) = (cols[2], cols[3]);
            let repairs = super::spelling_repairs(keys);
            let old = repairs.iter().any(|r| super::decode_compositions(r, 100_000).iter().any(|t| t == target));
            let top2 = repairs.iter().any(|r| super::decode_compositions(r, 2).iter().any(|t| t == target));
            let lattice = repairs.iter().any(|r| super::decode_lattice(r, 100_000, false).iter().any(|t| t == target));
            let lattice3 = repairs.iter().any(|r| super::decode_lattice(r, 3, false).iter().any(|t| t == target));
            println!("REPAIR\t{}\t{keys}\t{target}\trepairs={}\told_any={old}\told_top2={top2}\tlat_any={lattice}\tlat_top3={lattice3}", cols[0], repairs.len());
        }
    }

    #[test]
    pub(super) fn the_lattice_keeps_homophones_and_diverse_readings_the_decoder_cannot() {
        // One word per reading: the whole-reading decoder can only ever say 高了.
        let plain = super::decode_compositions("gaole", 100);
        assert!(!plain.iter().any(|text| text == "搞了"), "{plain:?}");
        let lattice = super::decode_lattice("gaole", 100, false);
        assert!(lattice.iter().any(|text| text == "搞了"), "{lattice:?}");
        // Abbreviated: every two-letter word is its own reading, none is cut as a homophone.
        let abbreviated = super::decode_lattice("sz", 100, true);
        assert!(abbreviated.len() > 6, "{abbreviated:?}");
        // Nothing is invented: every output is built from dictionary words.
        assert!(super::decode_lattice("", 10, false).is_empty());
    }

    #[test]
    pub(super) fn compositions_include_the_natural_reading() {
        let comps = super::decode_compositions("taifuzale", 10);
        assert!(comps.iter().any(|c| c == "太复杂了"), "{comps:?}");
        let comps = super::decode_compositions("jiexialaizuoshenme", 10);
        assert!(comps.iter().any(|c| c == "接下来做什么"), "{comps:?}");
        for (raw, expected) in [("woxikanx", "我先看下"), ("jiexialzuoshenm", "接下来做什么"), ("nikkyoushenmwent", "你看看有什么问题")] {
            let comps = super::decode_compositions_with(raw, 10, true);
            assert!(comps.iter().any(|c| c == expected), "{raw}: {comps:?}");
        }
    }

}
