#![cfg_attr(not(any(test, feature = "librime")), allow(dead_code))]

//! Which optional dictionary layers the user switched off (`lexicon_layers_v1.json` in the Rime user directory).
//!
//! The dictionaries are merged into one Rime table, so a word cannot be asked which layer it came from. The build writes
//! `resources/lexicon_membership.tsv` instead: every word that sits in at least one optional layer (extension or professional pack)
//! and in no core layer, with the layers it sits in. A candidate is hidden only when every layer of its word is switched off; a
//! word that is also in a core layer or in an enabled layer stays. Nothing here touches the Rime table, so a switch takes effect
//! on the next keystroke without redeploying (a redeploy leaves the input method unusable for seconds and has taken the user
//! database lock before). The control file is only read, never blocks typing, and a damaged or foreign file means "nothing hidden".

use crate::secure_directory::{
    open_or_create_owned_leaf_directory, physical_runtime_path, private_create, private_regular_file, sync_directory,
    validate_owned_directory,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
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

fn effective(document: &Document) -> HashSet<String> {
    let mut disabled: HashSet<String> = DEFAULT_DISABLED.iter().map(|id| id.to_string()).collect();
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

/// Records the choice of one layer (`enabled` false = switched off) and keeps what else was chosen.
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
}

fn state() -> &'static Mutex<State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE.get_or_init(|| {
        Mutex::new(State { directory: None, disabled: DEFAULT_DISABLED.iter().map(|id| id.to_string()).collect(), modified: None, checked: None })
    })
}

/// Points the layer switches at the user directory (the same one the blacklist lives in).
pub(crate) fn attach(user_dir: &Path) {
    if let Ok(mut state) = state().lock() {
        if state.directory.as_deref() != Some(user_dir) {
            state.directory = Some(user_dir.to_path_buf());
            state.checked = None;
            state.modified = None;
        }
    }
}

fn refresh(state: &mut State) {
    let Some(directory) = state.directory.clone() else { return };
    if state.checked.is_some_and(|at| at.elapsed() < RECHECK) {
        return;
    }
    state.checked = Some(Instant::now());
    match read(&directory) {
        Some((document, modified)) if state.modified != Some(modified) => {
            state.disabled = effective(&document);
            state.modified = Some(modified);
        }
        Some(_) => {}
        None if state.modified.is_some() => {
            // the file was removed or damaged: back to the defaults
            state.disabled = effective(&Document::default());
            state.modified = None;
        }
        None => {}
    }
}

fn hidden_in(membership: &Membership, disabled: &HashSet<String>, text: &str) -> bool {
    !disabled.is_empty()
        && membership.get(text).is_some_and(|layers| layers.iter().all(|layer| disabled.contains(*layer)))
}

/// True when `text` is a run of two or more known words of at least two characters each, none of them hidden. Rime assembles such a
/// phrase from words that are on (在线模型 from 在线 + 模型) and it stays even if a switched-off layer also lists the same phrase.
fn assembled_from_visible_words(
    membership: &Membership,
    disabled: &HashSet<String>,
    known: &dyn Fn(&str) -> bool,
    text: &str,
) -> bool {
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
            if known(part) && !hidden_in(membership, disabled, part) {
                reachable[end] = true;
                break;
            }
        }
    }
    reachable[characters]
}

/// True when `text` exists only in optional layers that the user switched off. `known` tells whether a text is a word of any dictionary.
pub(crate) fn is_hidden(text: &str, known: &dyn Fn(&str) -> bool) -> bool {
    let Ok(mut state) = state().lock() else { return false };
    refresh(&mut state);
    hidden_in(membership(), &state.disabled, text)
        && !assembled_from_visible_words(membership(), &state.disabled, known, text)
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
        let disabled = effective(&document);
        assert!(disabled.contains("agent_idioms"));
        assert!(!disabled.contains("agent_pack_cad"));
        for default in DEFAULT_DISABLED {
            assert!(effective(&Document::default()).contains(*default));
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
        assert!(effective(&Document::default()).contains("agent_pack_poetry"));
        set_layer(&directory, "agent_pack_poetry", true).unwrap();
        let (document, _) = read(&directory).unwrap();
        assert_eq!(document.enabled, vec!["agent_pack_poetry".to_string()]);
        assert!(!effective(&document).contains("agent_pack_poetry"), "switched on by the user");
        set_layer(&directory, "agent_pack_poetry", false).unwrap();
        let (document, _) = read(&directory).unwrap();
        assert!(document.enabled.is_empty() && effective(&document).contains("agent_pack_poetry"));
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
}
