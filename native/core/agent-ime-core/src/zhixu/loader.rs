//! Reads Rime text dictionaries and publishes complete immutable indexes off the typing thread.

use super::Index;
use crate::lexicon_layers::{self, DictionaryLayers};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime};

const ASCII_LAYERS: &[&str] = &[
    "agent_project_terms",
    "agent_english",
    "agent_tech_terms",
    "agent_ai_terms",
    "agent_pack_terms",
];
const RECHECK: Duration = Duration::from_millis(750);
const MAX_DICTIONARY_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Entry {
    pub text: String,
    pub reading: String,
    pub weight: u32,
    pub layer: String,
}

#[derive(Default)]
pub(crate) struct DictionaryData {
    pub entries: Vec<Entry>,
}

fn valid_table(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

/// The shipped tables use the Rime text format: a YAML header, `...`, then tab-separated text/code/weight.
/// Unsupported header column layouts and weights fail the load instead of silently changing their meaning.
fn parse(text: &str, layer: &str) -> io::Result<(Vec<String>, Vec<Entry>)> {
    let mut imports = Vec::new();
    let mut entries = Vec::new();
    let mut in_imports = false;
    let mut body = false;
    for line in text.lines() {
        let line = line.trim_start_matches('\u{feff}');
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if !body {
            if trimmed == "..." {
                body = true;
                continue;
            }
            if let Some(value) = trimmed.strip_prefix("import_tables:") {
                if !value.trim().is_empty() && value.trim() != "[]" {
                    return Err(invalid("unsupported import_tables layout"));
                }
                in_imports = value.trim().is_empty();
            } else if in_imports && trimmed.starts_with("- ") {
                let name = trimmed[2..]
                    .split(" #")
                    .next()
                    .unwrap_or("")
                    .trim()
                    .trim_matches(['\'', '"']);
                if !valid_table(name) {
                    return Err(invalid("invalid dictionary import name"));
                }
                imports.push(name.to_string());
            } else {
                in_imports = false;
                if trimmed.starts_with("columns:") || trimmed == "use_preset_vocabulary: true" {
                    return Err(invalid(
                        "unsupported dictionary columns or preset vocabulary",
                    ));
                }
                if let Some(sort) = trimmed.strip_prefix("sort:") {
                    if sort
                        .split(" #")
                        .next()
                        .unwrap_or("")
                        .trim()
                        .trim_matches(['\'', '"'])
                        != "by_weight"
                    {
                        return Err(invalid("unsupported dictionary sort order"));
                    }
                }
            }
            continue;
        }
        let mut columns = line.split('\t');
        let word = columns.next().unwrap_or("").trim();
        let Some(code) = columns.next() else {
            return Err(invalid("dictionary row has no reading"));
        };
        let reading = code
            .split_whitespace()
            .map(str::to_ascii_lowercase)
            .collect::<Vec<_>>()
            .join(" ");
        let Some(weight) = columns
            .next()
            .and_then(|weight| weight.trim().parse::<u32>().ok())
        else {
            return Err(invalid("dictionary row has no integer weight"));
        };
        if word.is_empty() || reading.is_empty() {
            return Err(invalid("empty dictionary word or reading"));
        }
        entries.push(Entry {
            text: word.to_string(),
            reading,
            weight,
            layer: layer.to_string(),
        });
    }
    if !body {
        return Err(invalid("dictionary header is incomplete"));
    }
    Ok((imports, entries))
}

fn read_dictionary(path: &Path) -> io::Result<String> {
    let metadata = fs::metadata(path)?;
    if !metadata.is_file() || metadata.len() > MAX_DICTIONARY_BYTES {
        return Err(invalid("dictionary is not a bounded regular file"));
    }
    fs::read_to_string(path)
}

fn safe_deployed_override(metadata: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        metadata.is_file() && metadata.nlink() == 1
            && metadata.uid() == crate::secure_directory::effective_user_id()
            && metadata.mode() & 0o022 == 0
    }
    #[cfg(windows)]
    { metadata.is_file() }
}

fn load_with_layers(
    shared: &Path,
    user: &Path,
    layers: &DictionaryLayers,
    ascii_only: bool,
    deployed: bool,
) -> io::Result<DictionaryData> {
    struct Reader<'a> {
        shared: &'a Path,
        user: &'a Path,
        deployed: bool,
        installed: &'a HashSet<String>,
        layers: &'a DictionaryLayers,
        overrides: HashMap<&'a str, &'a Path>,
        visited: HashSet<String>,
        data: DictionaryData,
    }
    impl Reader<'_> {
        fn table(&mut self, name: &str, optional: bool) -> io::Result<()> {
            if (!self.deployed && self.layers.disabled.contains(name)) || !self.visited.insert(name.to_string()) {
                return Ok(());
            }
            if self.visited.len() > 128 {
                return Err(invalid("too many dictionary imports"));
            }
            let shared_path = self.shared.join(format!("{name}.dict.yaml"));
            let user_path = self.user.join(format!("{name}.dict.yaml"));
            // Rime resolves user copies of a shared import even without a pack
            // membership file. Unlisted user tables are never discovered here.
            let imported_override = self.deployed && name != "agent_pinyin"
                && (self.installed.contains(name)
                    || matches!(fs::symlink_metadata(self.user.join("lexicon_packs").join(format!("{name}.membership.tsv"))),
                        Err(error) if error.kind() == io::ErrorKind::NotFound))
                && fs::symlink_metadata(&shared_path).is_ok_and(|metadata| metadata.is_file())
                && match fs::symlink_metadata(&user_path) {
                    Ok(metadata) => {
                        if !safe_deployed_override(&metadata) || metadata.len() > MAX_DICTIONARY_BYTES {
                            return Err(invalid("unsafe deployed dictionary override"));
                        }
                        true
                    }
                    Err(error) if error.kind() == io::ErrorKind::NotFound => false,
                    Err(error) => return Err(error),
                };
            let path = if imported_override { &user_path }
                else { self.overrides.get(name).copied().unwrap_or(&shared_path) };
            let text = match read_dictionary(path) {
                Ok(text) => text,
                Err(error) if optional && error.kind() == io::ErrorKind::NotFound => return Ok(()),
                Err(error) => return Err(error),
            };
            let (imports, entries) = parse(&text, name)?;
            self.data.entries.extend(entries);
            for import in imports {
                self.table(&import, false)?;
            }
            Ok(())
        }
    }
    let installed = lexicon_layers::dictionary_pack_ids(user);
    let mut reader = Reader {
        shared,
        user,
        deployed,
        installed: &installed,
        layers,
        overrides: layers
            .packs
            .iter()
            .filter(|(name, _)| installed.contains(name))
            .map(|(name, path)| (name.as_str(), path.as_path()))
            .collect(),
        visited: HashSet::new(),
        data: DictionaryData::default(),
    };
    if !ascii_only {
        reader.table("agent_pinyin", false)?;
    }
    for layer in ASCII_LAYERS {
        reader.table(layer, true)?;
    }
    for (layer, _) in &layers.packs {
        if installed.contains(layer) {
            reader.table(layer, false)?;
        }
    }
    Ok(reader.data)
}

pub(crate) fn load(shared: &Path, user: &Path) -> io::Result<DictionaryData> {
    load_with_layers(shared, user, &lexicon_layers::dictionary_layers(user), false, false)
}

/// The ASCII term layers alone (about a thousand lines), read on the attaching thread: an English or product term
/// typed in the first few hundred milliseconds after start (npm, Redis) must not wait for the full dictionary.
fn load_terms(shared: &Path, user: &Path) -> Arc<Index> {
    let data = load_with_layers(shared, user, &lexicon_layers::dictionary_layers(user), true, false).unwrap_or_default();
    Arc::new(Index::from_data(data))
}

type SharedFingerprint = Vec<(String, SystemTime, u64, [u64; 6])>;

fn shared_fingerprint(shared: &Path) -> SharedFingerprint {
    let mut files: Vec<_> = fs::read_dir(shared)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            name.strip_suffix(".dict.yaml")?;
            let metadata = fs::symlink_metadata(entry.path()).ok()?;
            #[cfg(unix)]
            let identity = {
                use std::os::unix::fs::MetadataExt;
                [metadata.ino(), metadata.mode() as u64, metadata.nlink(), metadata.uid() as u64,
                    metadata.ctime() as u64, metadata.ctime_nsec() as u64]
            };
            #[cfg(not(unix))]
            let identity = [0; 6];
            Some((
                name,
                metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                metadata.len(),
                identity,
            ))
        })
        .collect();
    files.sort();
    files
}

fn reap_indexes(generations: &mut Vec<Arc<Index>>) {
    let mut seen = HashSet::new();
    generations.retain(|index| seen.insert(Arc::as_ptr(index)));
    generations.retain(|index| Arc::strong_count(index) > 1);
}

/// Correction indexes load on the worker; explicit idle native reloads build their own generation.
/// Typing only reads immutable snapshots, and the worker reclaims retired generations.
pub(crate) struct RuntimeDictionary {
    shared: PathBuf,
    user: PathBuf,
    index: Mutex<Option<Arc<Index>>>,
    /// The ASCII layers read at attach, used until the first full generation is published.
    terms: Arc<Index>,
    /// Idle reloads hand new native generations to the same background reclaimer.
    deployed_generations: Arc<Mutex<Vec<Arc<Index>>>>,
    deployed: Mutex<Option<(DictionaryLayers, SharedFingerprint, SharedFingerprint, Arc<Index>)>>,
}

impl RuntimeDictionary {
    pub(crate) fn new(shared: PathBuf, user: PathBuf) -> Arc<Self> {
        let terms = load_terms(&shared, &user);
        let runtime = Arc::new(Self {
            shared,
            user,
            index: Mutex::new(None),
            terms,
            deployed_generations: Arc::new(Mutex::new(Vec::new())),
            deployed: Mutex::new(None),
        });
        let weak = Arc::downgrade(&runtime);
        let deployed_generations = runtime.deployed_generations.clone();
        let _ = std::thread::Builder::new()
            .name("zhixu-dictionary".into())
            .spawn(move || {
                let mut previous = None;
                // Keep even the current generation worker-owned: replacing the global runtime may leave a short-lived
                // reader as its last owner, and that reader must not inherit the index's eventual deallocation.
                let mut generations = Vec::new();
                loop {
                    let alive = if let Some(runtime) = weak.upgrade() {
                        runtime.refresh(&mut previous, &mut generations);
                        if super::WHOLE_PREWARM_REQUESTED.load(std::sync::atomic::Ordering::Relaxed)
                            || std::env::var("ZHIZI_ZHIXU_WHOLE").is_ok_and(|value| value == "1") {
                            let _ = runtime.deployed_snapshot();
                        }
                        if let Some(index) = runtime.snapshot() { super::prewarm_loaded_whole(index); }
                        true
                    } else {
                        false
                    };
                    if let Ok(mut pending) = deployed_generations.lock() {
                        generations.append(&mut pending);
                    }
                    reap_indexes(&mut generations);
                    if !alive && generations.is_empty() {
                        break;
                    }
                    std::thread::sleep(RECHECK);
                }
            });
        runtime
    }

    fn refresh(
        &self,
        previous: &mut Option<(DictionaryLayers, SharedFingerprint)>,
        generations: &mut Vec<Arc<Index>>,
    ) {
        let layers = lexicon_layers::dictionary_layers(&self.user);
        let shared = shared_fingerprint(&self.shared);
        if previous
            .as_ref()
            .is_some_and(|last| last.0 == layers && last.1 == shared)
        {
            return;
        }
        let user_before = shared_fingerprint(&self.user);
        let Ok(data) = load_with_layers(&self.shared, &self.user, &layers, false, false) else {
            return;
        };
        // Share the immutable deployment with the correction path whenever
        // both readers select the same tables. Disabled layers and safe user
        // overrides still require distinct views; never publish those to Rime's
        // unchanged correction path by treating them as equivalent.
        let index = match load_with_layers(&self.shared, &self.user, &layers, false, true) {
            Ok(deployed) if deployed.entries == data.entries => match self.deployed_snapshot() {
                Ok(index) => index,
                Err(_) => return,
            },
            _ => Arc::new(Index::from_data(data)),
        };
        // An install writes membership before its dictionary. Do not publish a generation built across a change.
        if layers != lexicon_layers::dictionary_layers(&self.user)
            || shared != shared_fingerprint(&self.shared)
            || user_before != shared_fingerprint(&self.user)
        {
            return;
        }
        if let Ok(mut pending) = self.deployed_generations.lock() {
            generations.append(&mut pending);
        }
        generations.push(index.clone());
        let retired = if let Ok(mut current) = self.index.lock() {
            let retired = current.replace(index);
            *previous = Some((layers, shared));
            retired
        } else {
            None
        };
        // Release the replaced publication outside the lock; generations retains ownership until worker reaping.
        drop(retired);
    }

    fn deployed_snapshot(&self) -> io::Result<Arc<Index>> {
        // Serialize builders, not typing. An unchanged idle switch reuses the
        // generation already built by the background loader.
        let mut deployed = self.deployed.lock().map_err(|_| invalid("dictionary deployment unavailable"))?;
        let layers = lexicon_layers::dictionary_layers(&self.user);
        let before = (shared_fingerprint(&self.shared), shared_fingerprint(&self.user));
        if let Some((previous_layers, shared, user, index)) = deployed.as_ref() {
            if *previous_layers == layers && shared == &before.0 && user == &before.1 {
                return Ok(index.clone());
            }
        }
        // The native decoder replaces Rime, so it needs the entire deployed
        // candidate table. Mixer still applies the existing per-word layer filter.
        let data = load_with_layers(&self.shared, &self.user, &layers, false, true)?;
        let index = Arc::new(Index::from_data(data));
        if layers != lexicon_layers::dictionary_layers(&self.user)
            || before != (shared_fingerprint(&self.shared), shared_fingerprint(&self.user)) {
            return Err(invalid("dictionaries changed during native reload"));
        }
        self.deployed_generations.lock().map_err(|_| invalid("dictionary reclaimer unavailable"))?
            .push(index.clone());
        *deployed = Some((layers, before.0, before.1, index.clone()));
        super::prewarm_whole_index(index.clone());
        Ok(index)
    }

    fn ready_deployed(&self) -> Option<Arc<Index>> {
        self.deployed.try_lock().ok()?.as_ref().map(|(_, _, _, index)| index)
            .filter(|index| index.whole_ready()).cloned()
    }

    pub(crate) fn snapshot(&self) -> Option<Arc<Index>> {
        self.index.lock().ok().and_then(|index| index.clone())
    }
}

fn runtime() -> &'static Mutex<Option<Arc<RuntimeDictionary>>> {
    static RUNTIME: OnceLock<Mutex<Option<Arc<RuntimeDictionary>>>> = OnceLock::new();
    RUNTIME.get_or_init(|| Mutex::new(None))
}

pub(crate) fn attach(shared: &Path, user: &Path) {
    let retired = {
        let Ok(mut current) = runtime().lock() else {
            return;
        };
        if current
            .as_ref()
            .is_some_and(|runtime| runtime.shared == shared && runtime.user == user)
        {
            return;
        }
        current.replace(RuntimeDictionary::new(
            shared.to_path_buf(),
            user.to_path_buf(),
        ))
    };
    drop(retired);
}

/// Builds a native decoder's immutable generation at an idle deployment point.
/// The caller replaces its held generation only after this succeeds; ordinary
/// background correction dictionaries retain their existing loading rules.
pub(crate) fn deployed_snapshot() -> io::Result<Option<Arc<Index>>> {
    let attached = runtime().lock().map_err(|_| invalid("dictionary runtime unavailable"))?.clone();
    attached.map(|runtime| runtime.deployed_snapshot()).transpose()
}

/// A key must never join a dictionary builder or initialize its reading indexes.
/// None inside Ok preserves the standalone decoder path without an attached runtime.
pub(crate) fn ready_deployed_snapshot() -> Result<Option<Arc<Index>>, ()> {
    #[cfg(test)]
    if let Some(dictionary) = super::PINNED.with(|slot| slot.borrow().clone()).flatten() {
        return Ok(Some(dictionary));
    }
    let attached = runtime().try_lock().map_err(|_| ())?.clone();
    let Some(runtime) = attached else { return Ok(None); };
    runtime.ready_deployed().map(Some).ok_or(())
}

/// The attach-time ASCII terms of the attached directories, if any are attached.
pub(crate) fn early_terms() -> Option<Arc<Index>> {
    let runtime = runtime().lock().ok().and_then(|runtime| runtime.clone())?;
    Some(runtime.terms.clone())
}

pub(crate) fn current() -> Option<Arc<Index>> {
    let runtime = runtime().lock().ok().and_then(|runtime| runtime.clone())?;
    runtime.snapshot()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "zhixu-loader-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(path.join("shared")).unwrap();
            fs::create_dir_all(path.join("user")).unwrap();
            let fixture = Self(fs::canonicalize(path).unwrap());
            fixture.write("shared/agent_pinyin.dict.yaml", "---\nname: agent_pinyin\nsort: by_weight\nimport_tables:\n  - first\n  - second\n...\n");
            fixture.write(
                "shared/first.dict.yaml",
                "---\nname: first\nsort: by_weight\n...\n你好\t NI   HAO \t100\n你\tni\t500\n",
            );
            fixture.write(
                "shared/second.dict.yaml",
                "---\nname: second\nsort: by_weight\n...\n你好\tni hao\t200\n好啊\thao a\t300\n",
            );
            fixture
        }

        fn shared(&self) -> PathBuf {
            self.0.join("shared")
        }
        fn user(&self) -> PathBuf {
            self.0.join("user")
        }

        fn write(&self, path: &str, text: &str) {
            let path = self.0.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, text).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn imports_preserve_layer_order_and_normalize_readings() {
        let fixture = Fixture::new();
        let data = load(&fixture.shared(), &fixture.user()).unwrap();
        let rows: Vec<_> = data
            .entries
            .iter()
            .map(|entry| {
                (
                    entry.text.as_str(),
                    entry.reading.as_str(),
                    entry.weight,
                    entry.layer.as_str(),
                )
            })
            .collect();
        assert_eq!(
            rows,
            [
                ("你好", "ni hao", 100, "first"),
                ("你", "ni", 500, "first"),
                ("你好", "ni hao", 200, "second"),
                ("好啊", "hao a", 300, "second")
            ]
        );
        let index = Index::from_data(data);
        assert_eq!(index.entry_count(), 3);
        let words = index.entry_words("ni hao");
        assert_eq!(words.len(), 1);
        assert_eq!(words[0].weight, 200);
        assert_eq!(words[0].layers, ["first", "second"]);
    }

    #[test]
    fn absent_ascii_dictionaries_are_empty() {
        let fixture = Fixture::new();
        let data = load(&fixture.shared(), &fixture.user()).unwrap();
        assert_eq!(data.entries.len(), 4);
        assert!(
            data.entries
                .iter()
                .all(|entry| !entry.layer.starts_with("agent_"))
        );
        let index = Index::from_data(data);
        assert!(index.project.is_empty());
        assert!(index.english.is_empty());
        assert!(index.tech.is_empty());
    }

    #[test]
    fn ascii_terms_are_read_at_attach_without_the_full_dictionary() {
        let fixture = Fixture::new();
        fixture.write(
            "shared/agent_tech_terms.dict.yaml",
            "---\nname: agent_tech_terms\n...\nRedis\tredis\t500\n",
        );
        let terms = load_terms(&fixture.shared(), &fixture.user());
        assert!(!terms.tech.is_empty(), "npm/Redis typed right after start must not wait for the worker");
        assert!(terms.entry_words("ni hao").is_empty(), "only the ASCII layers are read on the attaching thread");
    }

    #[test]
    fn missing_import_does_not_publish_a_partial_dictionary() {
        let fixture = Fixture::new();
        fs::remove_file(fixture.shared().join("second.dict.yaml")).unwrap();
        assert!(load(&fixture.shared(), &fixture.user()).is_err());
    }

    #[test]
    fn recursive_imports_are_visited_once_and_cannot_leave_the_shared_directory() {
        let fixture = Fixture::new();
        fixture.write("shared/second.dict.yaml", "---\nname: second\nimport_tables:\n  - first\n  - agent_pinyin\n...\n好啊\thao a\t300\n");
        assert_eq!(
            load(&fixture.shared(), &fixture.user())
                .unwrap()
                .entries
                .len(),
            3
        );
        fixture.write(
            "shared/second.dict.yaml",
            "---\nname: second\nimport_tables:\n  - ../outside\n...\n",
        );
        assert!(load(&fixture.shared(), &fixture.user()).is_err());
    }

    #[test]
    fn unsupported_dictionary_sort_does_not_change_the_runtime_order() {
        let fixture = Fixture::new();
        fixture.write(
            "shared/second.dict.yaml",
            "---\nname: second\nsort: original\n...\n好啊\thao a\t300\n",
        );
        assert!(load(&fixture.shared(), &fixture.user()).is_err());
    }

    #[test]
    fn unusable_pack_membership_never_adds_words_or_overrides_a_shared_layer() {
        let fixture = Fixture::new();
        fixture.write(
            "user/second.dict.yaml",
            "---\nname: second\n...\n下载词\txia zai ci\t900\n",
        );
        for membership in [
            "",
            "# empty\n",
            "invalid row\n",
            "下载词\t../foreign\n",
            "下载词\tforeign\n",
            "下载词\tsecond,foreign\n",
        ] {
            fixture.write("user/lexicon_packs/second.membership.tsv", membership);
            let data = load(&fixture.shared(), &fixture.user()).unwrap();
            assert!(
                !data.entries.iter().any(|entry| entry.text == "下载词"),
                "membership {membership:?}"
            );
            assert!(data.entries.iter().any(|entry| entry.text == "好啊"));
        }
        fs::write(
            fixture.user().join("lexicon_packs/second.membership.tsv"),
            [0xff, 0xfe],
        )
        .unwrap();
        assert!(
            !load(&fixture.shared(), &fixture.user())
                .unwrap()
                .entries
                .iter()
                .any(|entry| entry.text == "下载词")
        );
        fixture.write(
            "user/lexicon_packs/second.membership.tsv",
            "下载词\tsecond\n",
        );
        assert!(
            load(&fixture.shared(), &fixture.user())
                .unwrap()
                .entries
                .iter()
                .any(|entry| entry.text == "下载词")
        );
    }

    #[test]
    fn owner_words_override_the_shared_shell_and_are_enabled_by_default() {
        let fixture = Fixture::new();
        fixture.write("shared/agent_pinyin.dict.yaml", "---\nname: agent_pinyin\nsort: by_weight\nimport_tables:\n  - first\n  - second\n  - agent_owner_words\n...\n");
        fixture.write(
            "shared/agent_owner_words.dict.yaml",
            "---\nname: agent_owner_words\nsort: by_weight\n...\n",
        );
        fixture.write(
            "user/agent_owner_words.dict.yaml",
            "---\nname: agent_owner_words\nsort: by_weight\n...\n甲乙丙\tjia yi bing\t1500\n乙丙甲\tyi bing jia\t1000\n",
        );
        fixture.write(
            "user/lexicon_packs/agent_owner_words.membership.tsv",
            "甲乙丙\tagent_owner_words\n乙丙甲\tagent_owner_words\n",
        );
        assert!(!fixture.user().join("lexicon_layers_v1.json").exists());
        assert!(
            !lexicon_layers::dictionary_layers(&fixture.user())
                .disabled
                .contains("agent_owner_words")
        );
        let runtime = RuntimeDictionary {
            shared: fixture.shared(),
            user: fixture.user(),
            index: Mutex::new(None),
            terms: Arc::new(Index::from_data(DictionaryData::default())),
            deployed_generations: Arc::new(Mutex::new(Vec::new())),
            deployed: Mutex::new(None),
        };
        let mut fingerprint = None;
        let mut generations = Vec::new();
        runtime.refresh(&mut fingerprint, &mut generations);
        let index = runtime.snapshot().unwrap();
        for (reading, text, weight) in [
            ("jia yi bing", "甲乙丙", 1500),
            ("yi bing jia", "乙丙甲", 1000),
        ] {
            let words = index.entry_words(reading);
            assert_eq!(words.len(), 1);
            assert_eq!(words[0].text, text);
            assert_eq!(words[0].weight, weight);
            assert_eq!(words[0].layers, ["agent_owner_words"]);
        }
        lexicon_layers::set_layer(&fixture.user(), "agent_owner_words", false).unwrap();
        runtime.refresh(&mut fingerprint, &mut generations);
        let disabled = runtime.snapshot().unwrap();
        assert!(disabled.entry_words("jia yi bing").is_empty());
        assert!(disabled.entry_words("yi bing jia").is_empty());
        assert_eq!(disabled.entry_count(), index.entry_count() - 2);
        lexicon_layers::set_layer(&fixture.user(), "agent_owner_words", true).unwrap();
        runtime.refresh(&mut fingerprint, &mut generations);
        assert_eq!(runtime.snapshot().unwrap().entry_count(), index.entry_count());
    }

    #[test]
    fn disabled_layers_follow_existing_control_file_and_default_rules() {
        let fixture = Fixture::new();
        fixture.write(
            "user/lexicon_layers_v1.json",
            r#"{"formatVersion":1,"disabled":["second"],"enabled":[]}"#,
        );
        let data = load(&fixture.shared(), &fixture.user()).unwrap();
        assert_eq!(data.entries.len(), 2);
        assert!(data.entries.iter().all(|entry| entry.layer == "first"));
        fixture.write("user/lexicon_layers_v1.json", "damaged");
        assert_eq!(
            load(&fixture.shared(), &fixture.user())
                .unwrap()
                .entries
                .len(),
            4
        );
        fixture.write(
            "user/lexicon_packs/agent_pack_poetry.membership.tsv",
            "春眠\tagent_pack_poetry\n",
        );
        fixture.write(
            "user/agent_pack_poetry.dict.yaml",
            "---\nname: agent_pack_poetry\n...\n春眠\tchun mian\t100\n",
        );
        // a downloaded pack that is off by default counts as on until the user switches it off
        assert_eq!(
            load(&fixture.shared(), &fixture.user())
                .unwrap()
                .entries
                .len(),
            5
        );
        fixture.write(
            "user/lexicon_layers_v1.json",
            r#"{"formatVersion":1,"disabled":["agent_pack_poetry"],"enabled":[]}"#,
        );
        assert_eq!(
            load(&fixture.shared(), &fixture.user())
                .unwrap()
                .entries
                .len(),
            4
        );
        fixture.write(
            "user/lexicon_layers_v1.json",
            r#"{"formatVersion":1,"disabled":[],"enabled":["agent_pack_poetry"]}"#,
        );
        assert!(
            load(&fixture.shared(), &fixture.user())
                .unwrap()
                .entries
                .iter()
                .any(|entry| entry.text == "春眠")
        );
    }

    #[test]
    fn downloaded_dictionary_changes_and_removal_rebuild_the_published_index() {
        let fixture = Fixture::new();
        let runtime = RuntimeDictionary {
            shared: fixture.shared(),
            user: fixture.user(),
            index: Mutex::new(None),
            terms: Arc::new(Index::from_data(DictionaryData::default())),
            deployed_generations: Arc::new(Mutex::new(Vec::new())),
            deployed: Mutex::new(None),
        };
        assert!(runtime.snapshot().is_none());
        let mut fingerprint = None;
        let mut generations = Vec::new();
        runtime.refresh(&mut fingerprint, &mut generations);
        let original = runtime.snapshot().unwrap();
        runtime.refresh(&mut fingerprint, &mut generations);
        assert!(Arc::ptr_eq(&original, &runtime.snapshot().unwrap()));
        fixture.write(
            "user/lexicon_packs/downloaded.membership.tsv",
            "新词\tdownloaded\n",
        );
        // This mirrors the installer's two separate renames: membership alone must not add a word.
        runtime.refresh(&mut fingerprint, &mut generations);
        assert!(
            !load(&fixture.shared(), &fixture.user())
                .unwrap()
                .entries
                .iter()
                .any(|entry| entry.text == "新词")
        );
        fixture.write(
            "user/downloaded.dict.yaml",
            "---\nname: downloaded\n...\n新词\txin ci\t800\nNewTerm\tnewterm\t500\n",
        );
        runtime.refresh(&mut fingerprint, &mut generations);
        let installed = runtime.snapshot().unwrap();
        assert!(!Arc::ptr_eq(&original, &installed));
        assert_eq!(installed.entry_count(), original.entry_count() + 2);
        let data = load(&fixture.shared(), &fixture.user()).unwrap();
        assert!(data.entries.iter().any(|entry| entry.text == "新词"));
        assert!(data.entries.iter().any(|entry| entry.text == "NewTerm"));
        // Dictionary bytes changed without touching membership: the dictionary fingerprint still rebuilds.
        fixture.write(
            "user/downloaded.dict.yaml",
            "---\nname: downloaded\n...\n替换新词\tti huan xin ci\t800\n",
        );
        runtime.refresh(&mut fingerprint, &mut generations);
        assert!(!Arc::ptr_eq(&installed, &runtime.snapshot().unwrap()));
        fixture.write(
            "user/lexicon_layers_v1.json",
            r#"{"formatVersion":1,"disabled":["downloaded"]}"#,
        );
        runtime.refresh(&mut fingerprint, &mut generations);
        assert_eq!(
            runtime.snapshot().unwrap().entry_count(),
            original.entry_count()
        );
        fs::remove_file(fixture.user().join("lexicon_layers_v1.json")).unwrap();
        fs::remove_file(fixture.user().join("downloaded.dict.yaml")).unwrap();
        runtime.refresh(&mut fingerprint, &mut generations);
        assert_eq!(
            runtime.snapshot().unwrap().entry_count(),
            original.entry_count()
        );
    }

    #[test]
    fn downloaded_import_overrides_shared_table_and_failed_reload_keeps_previous_generation() {
        let fixture = Fixture::new();
        fixture.write("user/lexicon_packs/second.membership.tsv", "新词\tsecond\n");
        fixture.write(
            "user/second.dict.yaml",
            "---\nname: second\n...\n新词\txin ci\t800\n",
        );
        let data = load(&fixture.shared(), &fixture.user()).unwrap();
        assert_eq!(data.entries.len(), 3);
        assert!(!data.entries.iter().any(|entry| entry.text == "好啊"));
        let runtime = RuntimeDictionary {
            shared: fixture.shared(),
            user: fixture.user(),
            index: Mutex::new(None),
            terms: Arc::new(Index::from_data(DictionaryData::default())),
            deployed_generations: Arc::new(Mutex::new(Vec::new())),
            deployed: Mutex::new(None),
        };
        let mut fingerprint = None;
        let mut generations = Vec::new();
        runtime.refresh(&mut fingerprint, &mut generations);
        let original = runtime.snapshot().unwrap();
        fixture.write("shared/first.dict.yaml", "broken header");
        runtime.refresh(&mut fingerprint, &mut generations);
        assert!(Arc::ptr_eq(&original, &runtime.snapshot().unwrap()));
    }

    #[test]
    fn y5b_ready_deployment_never_waits_for_builder_or_cold_indexes() {
        let fixture = Fixture::new();
        let index = Arc::new(Index::from_data(load(&fixture.shared(), &fixture.user()).unwrap()));
        let runtime = RuntimeDictionary {
            shared: fixture.shared(), user: fixture.user(), index: Mutex::new(None), terms: index.clone(),
            deployed_generations: Arc::new(Mutex::new(Vec::new())), deployed: Mutex::new(None),
        };
        let mut builder = runtime.deployed.lock().unwrap();
        assert!(runtime.ready_deployed().is_none(), "must not wait on the builder lock");
        *builder = Some((lexicon_layers::dictionary_layers(&fixture.user()), Vec::new(), Vec::new(), index.clone()));
        drop(builder);
        assert!(runtime.ready_deployed().is_none(), "dictionary alone is not a warm reading index");
        index.warm_whole();
        assert!(Arc::ptr_eq(&runtime.ready_deployed().unwrap(), &index));
    }

    #[test]
    fn y4_deployment_reuses_background_generation_and_unchanged_reload() {
        let fixture = Fixture::new();
        let runtime = RuntimeDictionary::new(fixture.shared(), fixture.user());
        let first = runtime.deployed_snapshot().unwrap();
        let second = runtime.deployed_snapshot().unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while runtime.snapshot().is_none() {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(Arc::ptr_eq(&first, &runtime.snapshot().unwrap()));
    }

    #[cfg(unix)]
    #[test]
    fn y4_cached_deployment_revalidates_override_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let fixture = Fixture::new();
        fixture.write("user/second.dict.yaml", "---\nname: second\n...\n新词\txin ci\t800\n");
        let runtime = RuntimeDictionary::new(fixture.shared(), fixture.user());
        let held = runtime.deployed_snapshot().unwrap();
        fs::set_permissions(fixture.user().join("second.dict.yaml"), fs::Permissions::from_mode(0o666)).unwrap();
        assert!(runtime.deployed_snapshot().is_err());
        assert!(held.all_readings_for_text("新词").next().is_some());
    }

    #[test]
    fn deployed_snapshot_loads_shared_import_overrides_without_membership() {
        let fixture = Fixture::new();
        fixture.write("shared/agent_pinyin.dict.yaml", "---\nname: agent_pinyin\nimport_tables:\n  - agent_pack_cad\n...\n");
        fixture.write("shared/agent_pack_cad.dict.yaml", "---\nname: agent_pack_cad\n...\n原词\tyuan ci\t100\n");
        fixture.write("user/agent_pack_cad.dict.yaml", "---\nname: agent_pack_cad\n...\n重载词\tchong zai ci\t200\n");
        fixture.write("user/unlisted.dict.yaml", "---\nname: unlisted\n...\n不应导入\tbu ying dao ru\t900\n");
        let runtime = RuntimeDictionary::new(fixture.shared(), fixture.user());
        let deployed = runtime.deployed_snapshot().unwrap();
        assert_eq!(deployed.all_readings_for_text("重载词").collect::<Vec<_>>(), vec!["chong zai ci"]);
        assert!(deployed.all_readings_for_text("原词").next().is_none());
        assert!(deployed.all_readings_for_text("不应导入").next().is_none());
        // The old correction loader still honors its layer/membership contract.
        assert!(load(&fixture.shared(), &fixture.user()).unwrap().entries.is_empty());
    }

    #[test]
    fn deployed_override_does_not_bypass_invalid_pack_membership() {
        let fixture = Fixture::new();
        fixture.write("user/second.dict.yaml", "---\nname: second\n...\n非法包词\tfei fa bao ci\t800\n");
        fixture.write("user/lexicon_packs/second.membership.tsv", "非法包词\tforeign_layer\n");
        let runtime = RuntimeDictionary::new(fixture.shared(), fixture.user());
        let deployed = runtime.deployed_snapshot().unwrap();
        assert!(deployed.all_readings_for_text("非法包词").next().is_none());
        assert!(deployed.all_readings_for_text("好啊").next().is_some());
    }

    #[test]
    fn deployed_reload_is_immediate_and_failed_reload_keeps_the_held_generation() {
        let fixture = Fixture::new();
        let runtime = RuntimeDictionary::new(fixture.shared(), fixture.user());
        let old = runtime.deployed_snapshot().unwrap();
        fixture.write("user/second.dict.yaml", "---\nname: second\n...\n新词\txin ci\t800\n");
        let next = runtime.deployed_snapshot().unwrap();
        assert!(next.all_readings_for_text("新词").next().is_some());
        assert!(old.all_readings_for_text("新词").next().is_none());
        assert!(old.all_readings_for_text("好啊").next().is_some());
        fixture.write("user/second.dict.yaml", "broken header");
        assert!(runtime.deployed_snapshot().is_err());
        assert!(next.all_readings_for_text("新词").next().is_some());
        fs::remove_file(fixture.user().join("second.dict.yaml")).unwrap();
        let removed = runtime.deployed_snapshot().unwrap();
        assert!(removed.all_readings_for_text("新词").next().is_none());
        assert!(removed.all_readings_for_text("好啊").next().is_some());
    }

    #[cfg(unix)]
    #[test]
    fn deployed_import_override_rejects_symlinks_shared_links_and_foreign_writes() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let fixture = Fixture::new();
        let runtime = RuntimeDictionary::new(fixture.shared(), fixture.user());
        let path = fixture.user().join("second.dict.yaml");
        symlink(fixture.shared().join("second.dict.yaml"), &path).unwrap();
        assert!(runtime.deployed_snapshot().is_err());
        fs::remove_file(&path).unwrap();
        fs::hard_link(fixture.shared().join("second.dict.yaml"), &path).unwrap();
        assert!(runtime.deployed_snapshot().is_err());
        fs::remove_file(&path).unwrap();
        fixture.write("user/second.dict.yaml", "---\nname: second\n...\n新词\txin ci\t800\n");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o666)).unwrap();
        assert!(runtime.deployed_snapshot().is_err());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(runtime.deployed_snapshot().unwrap().all_readings_for_text("新词").next().is_some());
    }

    #[test]
    fn deployed_generations_remain_worker_owned_after_runtime_replacement() {
        let fixture = Fixture::new();
        let runtime = RuntimeDictionary::new(fixture.shared(), fixture.user());
        let held = runtime.deployed_snapshot().unwrap();
        let retired = Arc::downgrade(&held);
        drop(runtime);
        assert!(Arc::strong_count(&held) >= 2, "native generation must have a background owner");
        drop(held);
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while retired.upgrade().is_some() {
            assert!(std::time::Instant::now() < deadline, "retired native generation was not reclaimed");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn background_worker_publishes_without_a_reader_refresh() {
        let fixture = Fixture::new();
        let runtime = RuntimeDictionary::new(fixture.shared(), fixture.user());
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            if runtime.snapshot().is_some() {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "background dictionary load timed out"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        let held = runtime.snapshot().unwrap();
        let index = Arc::downgrade(&held);
        drop(runtime);
        drop(held);
        while index.upgrade().is_some() {
            assert!(
                std::time::Instant::now() < deadline,
                "retired dictionary worker did not release its indexes"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn replaced_indexes_stay_worker_owned_until_readers_release_them() {
        let fixture = Fixture::new();
        let runtime = RuntimeDictionary {
            shared: fixture.shared(),
            user: fixture.user(),
            index: Mutex::new(None),
            terms: Arc::new(Index::from_data(DictionaryData::default())),
            deployed_generations: Arc::new(Mutex::new(Vec::new())),
            deployed: Mutex::new(None),
        };
        let mut fingerprint = None;
        let mut generations = Vec::new();
        runtime.refresh(&mut fingerprint, &mut generations);
        let held = runtime.snapshot().unwrap();
        let previous = Arc::downgrade(&held);
        fixture.write(
            "shared/first.dict.yaml",
            "---\nname: first\n...\n更新\tgeng xin\t900\n",
        );
        runtime.refresh(&mut fingerprint, &mut generations);
        reap_indexes(&mut generations);
        assert!(
            previous.upgrade().is_some(),
            "a reader still holds the replaced index"
        );
        drop(held);
        assert!(
            previous.upgrade().is_some(),
            "dropping the last reader must leave the worker owning the old index"
        );
        reap_indexes(&mut generations);
        assert!(
            previous.upgrade().is_none(),
            "the worker reclaims a generation after its readers finish"
        );
        let current = Arc::downgrade(&runtime.snapshot().unwrap());
        drop(runtime);
        assert!(
            current.upgrade().is_some(),
            "the active generation also remains worker-owned when its runtime closes"
        );
        reap_indexes(&mut generations);
        assert!(current.upgrade().is_none());
        assert!(generations.is_empty());
    }
}
