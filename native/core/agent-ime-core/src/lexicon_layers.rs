//! Which optional dictionary layers the user switched off (`lexicon_layers_v1.json` in the Rime user directory).
//!
//! The dictionaries are merged into one Rime table, so a word cannot be asked which layer it came from. The build writes
//! `resources/lexicon_membership.tsv` instead: every word that sits in at least one optional layer (extension or professional pack)
//! and in no core layer, with the layers it sits in. A candidate is hidden only when every layer of its word is switched off; a
//! word that is also in a core layer or in an enabled layer stays. Nothing here touches the Rime table, so a switch takes effect
//! on the next keystroke without redeploying (a redeploy leaves the input method unusable for seconds and has taken the user
//! database lock before). The control file is only read, never blocks typing, and a damaged or foreign file means "nothing hidden".
//!
//! Professional packs the user downloads later add membership tables at runtime: `lexicon_packs/<id>.membership.tsv` in the
//! same user directory, in the same shape as the built-in one. A word of either table hides only when every layer the tables
//! list for it is off. The pack directory is fingerprinted at the same throttled pace, the files are parsed outside the state
//! lock, and a missing, damaged, foreign or oversized pack file means "not installed".

use crate::secure_directory::{
    open_or_create_owned_leaf_directory, physical_runtime_path, private_create, private_regular_file, sync_directory,
    validate_owned_directory,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

const FILE_NAME: &str = "lexicon_layers_v1.json";
const FORMAT_VERSION: u32 = 1;
const MAX_BYTES: u64 = 16 * 1024;
const MAX_LAYERS: usize = 64;
/// The control file is looked at again at most this often; the window that writes it is in the same process or a settings tool.
const RECHECK: Duration = Duration::from_millis(750);
/// Layers that are off before the user has chosen anything: the professional packs (owner 2026-10-17; measured: no effect on everyday
/// typing, one displaced candidate on the colloquial set). Must equal the `defaultEnabled: false` layers of `lexicon_catalog.json`
/// (a test checks it).
const DEFAULT_DISABLED: &[&str] = &["agent_pack_poetry", "agent_pack_cad", "agent_pack_industry"];
/// Membership tables of downloaded packs live here, next to the control file; the management console installs them.
const PACK_DIRECTORY: &str = "lexicon_packs";
const PACK_FILE_SUFFIX: &str = ".membership.tsv";
/// A pack membership file that is bigger than this, or not a private regular file, means "not installed".
const MAX_PACK_BYTES: u64 = 16 * 1024 * 1024;
/// How many pack membership files are read at all; files beyond the first eight by name are ignored.
const MAX_PACK_FILES: usize = 8;
/// How many entries all pack membership files may add together; the excess is dropped without an error.
const MAX_PACK_ENTRIES: usize = 400_000;

#[derive(Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Document {
    format_version: u32,
    /// Layer ids the user switched off.
    disabled: Vec<String>,
    /// Layer ids the user switched on although they are off by default.
    #[serde(default)]
    enabled: Vec<String>,
}

type Membership = HashMap<&'static str, Vec<&'static str>>;
/// The same table shape, but read at runtime from installed packs, so the strings are owned.
type ExtraMembership = HashMap<String, Vec<String>>;
/// File name, modification time and size of every pack membership file, sorted by name: any change means reread.
type PacksFingerprint = Vec<(String, SystemTime, u64)>;

/// Metadata for a background dictionary reader; membership bodies are validated only when rebuilding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DictionaryLayers {
    pub disabled: HashSet<String>,
    pub packs: Vec<(String, PathBuf)>,
    fingerprint: Option<PacksFingerprint>,
    dictionaries: PacksFingerprint,
}

pub(crate) fn dictionary_layers(user_dir: &Path) -> DictionaryLayers {
    let user_dir = physical_runtime_path(user_dir);
    let fingerprint = packs_fingerprint(&user_dir);
    let installed = installed_ids(&fingerprint);
    let document = read(&user_dir).map(|(document, _)| document).unwrap_or_default();
    let disabled = effective(&document, &installed);
    let mut packs = Vec::new();
    let mut dictionaries = Vec::new();
    for (name, _, _) in fingerprint.iter().flatten().take(MAX_PACK_FILES) {
        let Some(id) = pack_id(name) else { continue };
        let membership = user_dir.join(PACK_DIRECTORY).join(name);
        if !fs::symlink_metadata(&membership).is_ok_and(|metadata| private_regular_file(&metadata) && metadata.len() <= MAX_PACK_BYTES) {
            continue;
        }
        let path = user_dir.join(format!("{id}.dict.yaml"));
        let Ok(metadata) = fs::symlink_metadata(&path) else { continue };
        if !private_regular_file(&metadata) || metadata.len() > 64 * 1024 * 1024 {
            continue;
        }
        dictionaries.push((id.to_string(), metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH), metadata.len()));
        packs.push((id.to_string(), path));
    }
    DictionaryLayers { disabled, packs, fingerprint, dictionaries }
}

fn parse_membership(text: &'static str) -> Membership {
    text.lines()
        .filter_map(|line| {
            let (word, layers) = line.split_once('\t')?;
            let layers: Vec<&'static str> = layers.split(',').filter(|layer| !layer.is_empty()).collect();
            (!word.is_empty() && !layers.is_empty()).then_some((word, layers))
        })
        .collect()
}

fn membership() -> &'static Membership {
    static MEMBERSHIP: OnceLock<Membership> = OnceLock::new();
    MEMBERSHIP.get_or_init(|| parse_membership(include_str!("../resources/lexicon_membership.tsv")))
}

fn valid_id(id: &str) -> bool {
    (1..=48).contains(&id.len()) && id.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

/// The pack id of a membership file name (`<id>.membership.tsv`), when the id is well formed.
fn pack_id(file_name: &str) -> Option<&str> {
    let id = file_name.strip_suffix(PACK_FILE_SUFFIX)?;
    valid_id(id).then_some(id)
}

/// Ids of the packs installed in the pack directory (a well-named membership file each).
fn installed_ids(fingerprint: &Option<PacksFingerprint>) -> HashSet<String> {
    fingerprint.iter().flatten().filter_map(|(name, _, _)| pack_id(name)).map(str::to_string).collect()
}

/// The layers switched off. A pack that is off by default but was downloaded counts as on until the user switches it
/// off: downloading it is asking for it, and the lexicon window may never be opened.
fn effective(document: &Document, installed: &HashSet<String>) -> HashSet<String> {
    let mut disabled: HashSet<String> =
        DEFAULT_DISABLED.iter().filter(|id| !installed.contains(**id)).map(|id| id.to_string()).collect();
    for id in &document.enabled {
        disabled.remove(id);
    }
    disabled.extend(document.disabled.iter().cloned());
    disabled
}

fn read(user_dir: &Path) -> Option<(Document, SystemTime)> {
    let path = physical_runtime_path(user_dir).join(FILE_NAME);
    let metadata = fs::symlink_metadata(&path).ok()?;
    if !private_regular_file(&metadata) || metadata.len() > MAX_BYTES {
        return None;
    }
    let document: Document = serde_json::from_slice(&fs::read(&path).ok()?).ok()?;
    let usable = document.format_version == FORMAT_VERSION
        && document.disabled.len() <= MAX_LAYERS
        && document.enabled.len() <= MAX_LAYERS
        && document.disabled.iter().chain(&document.enabled).all(|id| valid_id(id));
    usable.then(|| (document, metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH)))
}

/// Fingerprint of the installed pack tables: name, modification time and size of every well-named file in the pack
/// directory, sorted. `None` is "no packs" (no directory, or nothing well named in it).
fn packs_fingerprint(user_dir: &Path) -> Option<PacksFingerprint> {
    let directory = physical_runtime_path(user_dir).join(PACK_DIRECTORY);
    if !fs::symlink_metadata(&directory).is_ok_and(|metadata| metadata.is_dir()) {
        return None;
    }
    let mut fingerprint: PacksFingerprint = fs::read_dir(&directory)
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            pack_id(&name)?;
            let metadata = entry.metadata().ok()?;
            Some((name, metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH), metadata.len()))
        })
        .collect();
    if fingerprint.is_empty() {
        return None;
    }
    fingerprint.sort();
    Some(fingerprint)
}

/// One line of a pack membership table: `word<TAB>layer[,layer]`. `#` lines and blank lines carry no word; a layer id
/// that fails `valid_id` drops the whole line.
fn pack_line(line: &str) -> Option<(String, Vec<String>)> {
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let (word, layers) = line.split_once('\t')?;
    let mut ids = Vec::new();
    for layer in layers.split(',') {
        if layer.is_empty() {
            continue;
        }
        if !valid_id(layer) {
            return None;
        }
        ids.push(layer.to_string());
    }
    (!word.is_empty() && !ids.is_empty()).then_some((word.to_string(), ids))
}

struct PackMembership {
    id: String,
    rows: Vec<(String, Vec<String>)>,
    complete: bool,
}

/// Reads every installed pack table, at most the first `MAX_PACK_FILES` by name, outside any lock. A file that cannot
/// be read, is not a private regular file, is too large or is not UTF-8 counts as absent, and the entry cap drops the
/// excess without an error.
fn load_pack_memberships(user_dir: &Path) -> Vec<PackMembership> {
    let directory = physical_runtime_path(user_dir).join(PACK_DIRECTORY);
    let mut names: Vec<String> = match fs::read_dir(&directory) {
        Ok(entries) => entries
            .flatten()
            .filter_map(|entry| entry.file_name().into_string().ok().filter(|name| pack_id(name).is_some()))
            .collect(),
        Err(_) => return Vec::new(),
    };
    names.sort();
    names.truncate(MAX_PACK_FILES);
    let mut tables = Vec::new();
    let mut entries = 0;
    for name in names {
        let path = directory.join(&name);
        let Ok(metadata) = fs::symlink_metadata(&path) else { continue };
        if !private_regular_file(&metadata) || metadata.len() > MAX_PACK_BYTES {
            continue;
        }
        let Some(text) = fs::read(&path).ok().and_then(|bytes| String::from_utf8(bytes).ok()) else { continue };
        let mut table = PackMembership { id: pack_id(&name).unwrap().to_string(), rows: Vec::new(), complete: true };
        for line in text.lines() {
            let Some(row) = pack_line(line) else { continue };
            if entries == MAX_PACK_ENTRIES {
                table.complete = false;
                break;
            }
            entries += 1;
            table.rows.push(row);
        }
        tables.push(table);
    }
    tables
}

fn load_extra_memberships(user_dir: &Path) -> ExtraMembership {
    let mut extra = ExtraMembership::new();
    for table in load_pack_memberships(user_dir) {
        for (word, layers) in table.rows {
            extra.entry(word).or_default().extend(layers);
        }
    }
    extra
}

/// Downloaded dictionaries have one source layer. Foreign/multi-layer membership or a truncated table cannot
/// establish that source, so the runtime dictionary treats that pack as unavailable and keeps any shared table.
pub(crate) fn dictionary_pack_ids(user_dir: &Path) -> HashSet<String> {
    load_pack_memberships(user_dir).into_iter().filter(|table| {
        table.complete && !table.rows.is_empty()
            && table.rows.iter().all(|(_, layers)| layers.len() == 1 && layers[0] == table.id)
    }).map(|table| table.id).collect()
}

/// Records the choice of one layer (`enabled` false = switched off) and keeps what else was chosen.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn set_layer(user_dir: &Path, id: &str, enabled: bool) -> io::Result<()> {
    if !valid_id(id) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid layer id"));
    }
    let mut document = read(user_dir).map(|(document, _)| document).unwrap_or(Document { format_version: FORMAT_VERSION, ..Default::default() });
    let mut off: BTreeSet<String> = document.disabled.into_iter().collect();
    let mut on: BTreeSet<String> = document.enabled.into_iter().collect();
    if enabled {
        off.remove(id);
        if DEFAULT_DISABLED.contains(&id) {
            on.insert(id.to_string());
        }
    } else {
        on.remove(id);
        off.insert(id.to_string());
    }
    document = Document { format_version: FORMAT_VERSION, disabled: off.into_iter().collect(), enabled: on.into_iter().collect() };
    let user_dir = physical_runtime_path(user_dir);
    let directory = open_or_create_owned_leaf_directory(&user_dir, 0o700)?;
    validate_owned_directory(&directory, "Lexicon layers parent")?;
    let bytes = serde_json::to_vec(&document).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let temporary = user_dir.join(format!(".{FILE_NAME}.{}.tmp", std::process::id()));
    let mut file = private_create(OpenOptions::new().write(true).create_new(true)).open(&temporary)?;
    let result = (|| {
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, user_dir.join(FILE_NAME))?;
        sync_directory(&user_dir)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

struct State {
    directory: Option<PathBuf>,
    disabled: HashSet<String>,
    modified: Option<SystemTime>,
    checked: Option<Instant>,
    /// Membership tables of installed packs, swapped in whole after reading them outside the lock.
    extra: Arc<ExtraMembership>,
    /// The pack directory fingerprint that `extra` was read from.
    packs: Option<PacksFingerprint>,
}

fn state() -> &'static Mutex<State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE.get_or_init(|| {
        Mutex::new(State {
            directory: None,
            disabled: DEFAULT_DISABLED.iter().map(|id| id.to_string()).collect(),
            modified: None,
            checked: None,
            extra: Arc::new(HashMap::new()),
            packs: None,
        })
    })
}

/// Points the layer switches at the user directory (the same one the blacklist lives in).
pub(crate) fn attach(user_dir: &Path) {
    if let Ok(mut state) = state().lock() {
        if state.directory.as_deref() != Some(user_dir) {
            state.directory = Some(user_dir.to_path_buf());
            state.checked = None;
            state.modified = None;
            // the pack tables of the previous directory must not leak into this one
            state.extra = Arc::new(HashMap::new());
            state.packs = None;
        }
    }
}

/// Refreshes the layer switches and notices pack membership changes, all under the state lock and at most once per
/// `RECHECK`. Returns the pack directory and fingerprint to reread outside the lock when the installed tables changed;
/// the caller parses them and swaps the result back in.
fn refresh(state: &mut State, now: Instant) -> Option<(PathBuf, PacksFingerprint)> {
    let Some(directory) = state.directory.clone() else { return None };
    if state.checked.is_some_and(|at| now.duration_since(at) < RECHECK) {
        return None;
    }
    state.checked = Some(now);
    let fingerprint = packs_fingerprint(&directory);
    // an install or removal changes which default-off packs count as on, so the packs reread the switches too
    let packs_changed = fingerprint != state.packs;
    match read(&directory) {
        Some((document, modified)) if packs_changed || state.modified != Some(modified) => {
            state.disabled = effective(&document, &installed_ids(&fingerprint));
            state.modified = Some(modified);
        }
        Some(_) => {}
        None if packs_changed || state.modified.is_some() => {
            // no file, or it was removed or damaged: back to the defaults
            state.disabled = effective(&Document::default(), &installed_ids(&fingerprint));
            state.modified = None;
        }
        None => {}
    }
    if fingerprint == state.packs {
        return None;
    }
    match fingerprint {
        None => {
            // the pack directory went away: back to the built-in membership alone
            state.packs = None;
            state.extra = Arc::new(HashMap::new());
            None
        }
        Some(fingerprint) => Some((directory, fingerprint)),
    }
}

#[cfg_attr(not(test), allow(dead_code))]
fn hidden_in(membership: &Membership, disabled: &HashSet<String>, text: &str) -> bool {
    !disabled.is_empty()
        && membership.get(text).is_some_and(|layers| layers.iter().all(|layer| disabled.contains(*layer)))
}

/// The built-in table and the installed pack tables count together: a word hides only when every layer of the union of
/// their listings is off, so a word both tables list stays visible while either side keeps an enabled layer.
fn hidden_in_combined(membership: &Membership, extra: &ExtraMembership, disabled: &HashSet<String>, text: &str) -> bool {
    if disabled.is_empty() {
        return false;
    }
    match (membership.get(text), extra.get(text)) {
        (None, None) => false,
        (Some(builtin), None) => builtin.iter().all(|layer| disabled.contains(*layer)),
        (None, Some(pack)) => pack.iter().all(|layer| disabled.contains(layer)),
        (Some(builtin), Some(pack)) => {
            builtin.iter().all(|layer| disabled.contains(*layer)) && pack.iter().all(|layer| disabled.contains(layer))
        }
    }
}

/// True when `text` is a run of two or more known words of at least two characters each, none of them hidden. Rime assembles such a
/// phrase from words that are on (在线模型 from 在线 + 模型) and it stays even if a switched-off layer also lists the same phrase.
#[cfg_attr(not(test), allow(dead_code))]
fn assembled_from_visible_words(
    membership: &Membership,
    disabled: &HashSet<String>,
    known: &dyn Fn(&str) -> bool,
    text: &str,
) -> bool {
    assembled_from_visible(&|part| hidden_in(membership, disabled, part), known, text)
}

/// The same cut with "is this part hidden" already decided, so the built-in table and the pack tables can join in.
fn assembled_from_visible(hidden: &dyn Fn(&str) -> bool, known: &dyn Fn(&str) -> bool, text: &str) -> bool {
    let cuts: Vec<usize> = text.char_indices().map(|(index, _)| index).chain(std::iter::once(text.len())).collect();
    let characters = cuts.len() - 1;
    if characters < 4 {
        return false;
    }
    // reachable[i]: the first i characters split into visible known words; at least two words, so the whole text itself does not count
    let mut reachable = vec![false; characters + 1];
    reachable[0] = true;
    for end in 2..=characters {
        for start in 0..=end - 2 {
            if !reachable[start] || (start == 0 && end == characters) {
                continue;
            }
            let part = &text[cuts[start]..cuts[end]];
            if known(part) && !hidden(part) {
                reachable[end] = true;
                break;
            }
        }
    }
    reachable[characters]
}

/// True when `text` exists only in optional layers that the user switched off. `known` tells whether a text is a word
/// of any dictionary. The pack tables are reread outside the lock so typing never waits on file reads.
pub(crate) fn is_hidden(text: &str, known: &dyn Fn(&str) -> bool) -> bool {
    let reload = match state().lock() {
        Ok(mut state) => refresh(&mut state, Instant::now()),
        Err(_) => return false,
    };
    if let Some((directory, fingerprint)) = reload {
        let tables = Arc::new(load_extra_memberships(&directory));
        if let Ok(mut state) = state().lock() {
            state.extra = tables;
            state.packs = Some(fingerprint);
        }
    }
    let Ok(state) = state().lock() else { return false };
    let hidden = |word: &str| hidden_in_combined(membership(), &state.extra, &state.disabled, word);
    hidden(text) && !assembled_from_visible(&hidden, known, text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::UNIX_EPOCH;

    fn scratch(label: &str) -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let directory = std::env::temp_dir().join(format!("aim-layers-{label}-{}-{nanos}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::SeqCst)));
        fs::create_dir(&directory).unwrap();
        fs::canonicalize(&directory).unwrap()
    }

    fn fixture() -> Membership {
        parse_membership("春眠\tagent_pack_poetry\n正火\tagent_pack_industry\n淬火\tagent_pack_cad,agent_pack_industry\n成语一\tagent_idioms\n")
    }

    /// Writes a pack membership table the way the management console leaves it: a private regular file.
    fn write_pack(user_dir: &Path, id: &str, contents: &str) {
        let directory = user_dir.join(PACK_DIRECTORY);
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join(format!("{id}{PACK_FILE_SUFFIX}"));
        fs::write(&path, contents).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        }
    }

    #[test]
    fn a_word_is_hidden_only_when_every_layer_it_sits_in_is_off() {
        let membership = fixture();
        let off = |layers: &[&str]| layers.iter().map(|layer| layer.to_string()).collect::<HashSet<_>>();
        assert!(!hidden_in(&membership, &off(&[]), "春眠"), "nothing switched off hides nothing");
        assert!(hidden_in(&membership, &off(&["agent_pack_poetry"]), "春眠"));
        assert!(!hidden_in(&membership, &off(&["agent_pack_poetry"]), "正火"), "other layers stay");
        assert!(!hidden_in(&membership, &off(&["agent_pack_cad"]), "淬火"), "one enabled layer keeps the word");
        assert!(hidden_in(&membership, &off(&["agent_pack_cad", "agent_pack_industry"]), "淬火"));
        assert!(!hidden_in(&membership, &off(&["agent_pack_poetry"]), "你好"), "a word in no optional layer is never hidden");
    }

    #[test]
    fn choices_are_kept_between_calls_and_bad_files_hide_nothing() {
        let directory = scratch("file");
        assert!(read(&directory).is_none());
        set_layer(&directory, "agent_pack_poetry", false).unwrap();
        set_layer(&directory, "agent_idioms", false).unwrap();
        set_layer(&directory, "agent_idioms", true).unwrap();
        let (document, _) = read(&directory).unwrap();
        assert_eq!(document.disabled, vec!["agent_pack_poetry".to_string()]);
        assert!(set_layer(&directory, "../escape", false).is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            assert_eq!(fs::metadata(directory.join(FILE_NAME)).unwrap().mode() & 0o077, 0, "private file");
        }
        fs::write(directory.join(FILE_NAME), b"{\"formatVersion\":9,\"disabled\":[]}").unwrap();
        assert!(read(&directory).is_none(), "a foreign format version reads as no choice");
        fs::write(directory.join(FILE_NAME), b"not json").unwrap();
        assert!(read(&directory).is_none());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn the_effective_set_follows_the_defaults_and_the_choices() {
        let document = Document { format_version: 1, disabled: vec!["agent_idioms".into()], enabled: vec!["agent_pack_cad".into()] };
        let disabled = effective(&document, &HashSet::new());
        assert!(disabled.contains("agent_idioms"));
        assert!(!disabled.contains("agent_pack_cad"));
        for default in DEFAULT_DISABLED {
            assert!(effective(&Document::default(), &HashSet::new()).contains(*default));
        }
    }

    #[test]
    fn professional_packs_are_off_until_chosen_and_the_catalog_agrees() {
        let off: BTreeSet<&str> = DEFAULT_DISABLED.iter().copied().collect();
        let catalog: serde_json::Value = serde_json::from_str(include_str!("../../../resources/Lexicon/lexicon_catalog.json")).unwrap();
        let catalog_off: BTreeSet<&str> = catalog["layers"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|layer| layer["defaultEnabled"] == false)
            .map(|layer| layer["id"].as_str().unwrap())
            .collect();
        let catalog_ids: BTreeSet<&str> = catalog["layers"].as_array().unwrap().iter().map(|layer| layer["id"].as_str().unwrap()).collect();
        // A pack missing from the catalog (the public source tree ships the packs as empty shells) cannot disagree.
        let off_in_catalog: BTreeSet<&str> = off.iter().copied().filter(|id| catalog_ids.contains(id)).collect();
        assert_eq!(off_in_catalog, catalog_off, "core defaults and catalog defaults must change together");
        let directory = scratch("defaults");
        assert!(effective(&Document::default(), &HashSet::new()).contains("agent_pack_poetry"));
        set_layer(&directory, "agent_pack_poetry", true).unwrap();
        let (document, _) = read(&directory).unwrap();
        assert_eq!(document.enabled, vec!["agent_pack_poetry".to_string()]);
        assert!(!effective(&document, &HashSet::new()).contains("agent_pack_poetry"), "switched on by the user");
        set_layer(&directory, "agent_pack_poetry", false).unwrap();
        let (document, _) = read(&directory).unwrap();
        assert!(document.enabled.is_empty() && effective(&document, &HashSet::new()).contains("agent_pack_poetry"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn a_phrase_rime_assembles_from_visible_words_stays_even_when_a_pack_lists_it_too() {
        let membership = parse_membership("在线模型\tagent_pack_industry\n正火\tagent_pack_industry\n春眠\tagent_pack_poetry\n");
        let off: HashSet<String> = ["agent_pack_industry".to_string(), "agent_pack_poetry".to_string()].into();
        let known = |word: &str| matches!(word, "在线" | "模型" | "在线模型" | "正火" | "春眠");
        assert!(assembled_from_visible_words(&membership, &off, &known, "在线模型"), "在线 + 模型, both visible");
        assert!(!assembled_from_visible_words(&membership, &off, &known, "正火"), "two characters are never assembled");
        let only_pack_parts = |word: &str| matches!(word, "春眠" | "春眠春眠" | "正火");
        assert!(!assembled_from_visible_words(&membership, &off, &only_pack_parts, "春眠春眠"), "parts that are hidden do not count");
    }

    #[test]
    fn a_pack_word_hides_while_its_layer_is_off_until_the_user_opens_it() {
        let directory = scratch("packs-open");
        write_pack(&directory, "agent_pack_poetry", "哀江头\tagent_pack_poetry\n");
        let membership = fixture();
        let extra = load_extra_memberships(&directory);
        assert_eq!(extra.get("哀江头").map(|layers| layers.as_slice()), Some(&["agent_pack_poetry".to_string()][..]));
        let off = effective(&Document::default(), &HashSet::new());
        assert!(hidden_in_combined(&membership, &extra, &off, "哀江头"), "a pack stays off before the user has chosen");
        let opened = effective(&Document { format_version: FORMAT_VERSION, disabled: vec![], enabled: vec!["agent_pack_poetry".into()] }, &HashSet::new());
        assert!(!hidden_in_combined(&membership, &extra, &opened, "哀江头"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn the_builtin_and_the_pack_layers_of_one_word_union() {
        let directory = scratch("packs-union");
        // the builtin table puts 淬火 in cad + industry, the pack adds idioms: only all three off hide it
        write_pack(&directory, "agent_idioms_more", "淬火\tagent_idioms\n");
        let membership = fixture();
        let extra = load_extra_memberships(&directory);
        let off = |layers: &[&str]| layers.iter().map(|layer| layer.to_string()).collect::<HashSet<_>>();
        assert!(!hidden_in_combined(&membership, &extra, &off(&["agent_pack_cad", "agent_pack_industry"]), "淬火"), "the pack layer is still on");
        assert!(hidden_in_combined(&membership, &extra, &off(&["agent_pack_cad", "agent_pack_industry", "agent_idioms"]), "淬火"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn broken_pack_files_count_as_not_installed() {
        let directory = scratch("packs-broken");
        let packs = directory.join(PACK_DIRECTORY);
        fs::create_dir_all(&packs).unwrap();
        write_pack(&directory, "BAD", "词\tagent_idioms\n");
        fs::write(packs.join("notes.txt"), "词\tagent_idioms\n").unwrap();
        fs::create_dir(packs.join(format!("agent_pack_dir{PACK_FILE_SUFFIX}"))).unwrap();
        write_pack(
            &directory,
            "agent_pack_ok",
            "# 注释\n\n好词\tagent_idioms\n空层\t\n坏层\tagent_idioms,BAD\n逃逸\t../escape\n无表\t词\n",
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let target = directory.join("elsewhere.tsv");
            fs::write(&target, "链词\tagent_idioms\n").unwrap();
            std::os::unix::fs::symlink(&target, packs.join(format!("agent_pack_link{PACK_FILE_SUFFIX}"))).unwrap();
            // a file others may read is not private
            let open = packs.join(format!("agent_pack_open{PACK_FILE_SUFFIX}"));
            fs::write(&open, "开词\tagent_idioms\n").unwrap();
            fs::set_permissions(&open, fs::Permissions::from_mode(0o644)).unwrap();
        }
        let big = packs.join(format!("agent_pack_big{PACK_FILE_SUFFIX}"));
        fs::write(&big, vec![b'x'; MAX_PACK_BYTES as usize + 1]).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&big, fs::Permissions::from_mode(0o600)).unwrap();
        }
        let extra = load_extra_memberships(&directory);
        assert_eq!(extra.len(), 1, "only the intact private file counts: {extra:?}");
        assert_eq!(extra.get("好词").map(|layers| layers.as_slice()), Some(&["agent_idioms".to_string()][..]));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn pack_tables_are_reread_only_when_the_fingerprint_changes_and_outside_the_window() {
        let directory = scratch("packs-fingerprint");
        let mut state =
            State { directory: Some(directory.clone()), disabled: HashSet::new(), modified: None, checked: None, extra: Arc::new(HashMap::new()), packs: None };
        // no pack directory: the fingerprint already says "no packs", nothing to reread
        let start = Instant::now();
        assert!(refresh(&mut state, start).is_none());

        write_pack(&directory, "agent_pack_poetry", "哀江头\tagent_pack_poetry\n");
        assert!(refresh(&mut state, start + Duration::from_millis(100)).is_none(), "not scanned inside the window");
        assert_eq!(state.checked, Some(start), "the directory was not scanned again");
        let (_, fingerprint) = refresh(&mut state, start + RECHECK).expect("a changed directory must be reread");
        state.packs = Some(fingerprint);
        state.extra = Arc::new(load_extra_memberships(&directory));
        assert!(state.extra.contains_key("哀江头"));

        write_pack(&directory, "agent_pack_poetry", "哀江头\tagent_pack_poetry\n更漏子\tagent_pack_poetry\n");
        assert!(refresh(&mut state, start + RECHECK + Duration::from_millis(600)).is_none(), "still inside the next window");
        let _ = refresh(&mut state, start + RECHECK * 2).expect("a changed file must be reread");
        state.extra = Arc::new(load_extra_memberships(&directory));
        assert!(state.extra.contains_key("更漏子"));

        fs::remove_dir_all(directory.join(PACK_DIRECTORY)).unwrap();
        assert!(refresh(&mut state, start + RECHECK * 3).is_none());
        assert!(state.extra.is_empty() && state.packs.is_none(), "a removed directory leaves the builtin table alone");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn only_the_first_eight_pack_files_are_read() {
        let directory = scratch("packs-files");
        for index in 1..=9 {
            write_pack(&directory, &format!("a_pack_{index}"), &format!("词{index}\tagent_idioms\n"));
        }
        let extra = load_extra_memberships(&directory);
        for index in 1..=8 {
            assert!(extra.contains_key(&format!("词{index}")), "a_pack_{index} is among the first eight by name");
        }
        assert!(!extra.contains_key("词9"), "the ninth file is ignored");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn pack_entries_are_capped_at_four_hundred_thousand() {
        let directory = scratch("packs-entries");
        let mut contents = String::new();
        for index in 0..=MAX_PACK_ENTRIES {
            contents.push_str(&format!("词条{index}\tagent_idioms\n"));
        }
        write_pack(&directory, "a_pack_many", &contents);
        let extra = load_extra_memberships(&directory);
        assert_eq!(extra.len(), MAX_PACK_ENTRIES);
        assert!(extra.contains_key(&format!("词条{}", MAX_PACK_ENTRIES - 1)));
        assert!(!extra.contains_key(&format!("词条{}", MAX_PACK_ENTRIES)));
        let tables = load_pack_memberships(&directory);
        assert_eq!(tables[0].rows.len(), MAX_PACK_ENTRIES);
        assert!(!tables[0].complete, "a truncated table cannot establish all dictionary entries' source layer");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn a_downloaded_default_off_pack_counts_as_on_until_switched_off() {
        let installed: HashSet<String> = ["agent_pack_poetry".to_string()].into();
        assert!(!effective(&Document::default(), &installed).contains("agent_pack_poetry"), "downloaded = wanted");
        assert!(effective(&Document::default(), &installed).contains("agent_pack_cad"), "not downloaded stays off");
        let off = Document { format_version: FORMAT_VERSION, disabled: vec!["agent_pack_poetry".into()], enabled: vec![] };
        assert!(effective(&off, &installed).contains("agent_pack_poetry"), "the user's switch wins");
    }
}
