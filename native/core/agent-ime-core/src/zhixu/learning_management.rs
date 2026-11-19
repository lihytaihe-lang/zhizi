//! Explicit management of the current v1 learning store. No host process control.
use super::*;
use crate::secure_directory::open_existing_owned_leaf_directory;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, Hash, Ord, PartialOrd)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagementKey { pub reading: String, pub text: String }

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LearningWordBackup {
    pub format_version: u32,
    pub key: ManagementKey,
    pub rime_tick: u64,
    pub migrated: bool,
    pub owner_seeded: bool,
    pub legacy_choices_imported: bool,
    pub entries: Vec<Entry>,
    pub choices: Vec<Choice>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LearningStatus {
    pub entries: usize, pub choices: usize, pub actual_bytes: usize, pub estimated_bytes: usize,
    pub max_bytes: u64, pub max_entries: usize, pub max_age_days: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LearningManagementSnapshot { pub items: Vec<LearningWordBackup>, pub status: LearningStatus }

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LearningImportPreview { pub entries: usize, pub choices: usize, pub estimated_bytes: usize }

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Backup { format_version: u32, kind: String, contains_secrets: bool, document: Document }

fn invalid(message: &'static str) -> io::Error { io::Error::new(io::ErrorKind::InvalidData, message) }
fn key(reading: &str, text: &str) -> ManagementKey {
    ManagementKey { reading: reading.into(), text: text.into() }
}

fn validate(document: &Document) -> io::Result<()> {
    if document.format_version != FORMAT_VERSION || document.rime_tick == u64::MAX
        || document.entries.len() > MAX_ENTRIES || document.choices.len() > MAX_ENTRIES
        || document.delivery_ids.len() > MAX_DELIVERY_IDS
        || document.entries.iter().any(|e| !valid_entry(e) || e.rime.as_ref().is_some_and(|w| w.tick > document.rime_tick))
        || document.choices.iter().any(|e| !valid_choice(e)) {
        return Err(invalid("invalid bounded learning document"));
    }
    let mut entries = HashSet::new(); let mut choices = HashSet::new(); let mut ids = HashSet::new();
    if !document.entries.iter().all(|e| entries.insert(entry_key(e)))
        || !document.choices.iter().all(|e| choices.insert(choice_key(e)))
        || !document.delivery_ids.iter().all(|id| *id != 0 && ids.insert(*id)) {
        return Err(invalid("duplicate learning facts or delivery IDs"));
    }
    Ok(())
}

/// Reads one atomic file version without expiring, pruning, creating directories or saving.
pub(super) fn read_strict(directory: &Path) -> io::Result<(Document, usize)> {
    let directory = physical_runtime_path(directory);
    match open_existing_owned_leaf_directory(&directory) {
        Ok(root) => validate_owned_directory(&root, "learning management parent")?,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok((LearningStore::default().document(), 0)),
        Err(e) => return Err(e),
    }
    let path = directory.join(FILE_NAME);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok((LearningStore::default().document(), 0)),
        Err(e) => return Err(e),
    };
    if !private_regular_file(&metadata) || metadata.len() > MAX_BYTES { return Err(invalid("unsafe learning file")); }
    let mut options = OpenOptions::new(); options.read(true);
    #[cfg(unix)] {
        use std::os::unix::fs::OpenOptionsExt;
        #[cfg(target_os = "macos")] options.custom_flags(0x100);
        #[cfg(target_os = "linux")] options.custom_flags(0x0002_0000);
    }
    let file = options.open(path)?;
    if !private_regular_file(&file.metadata()?) { return Err(invalid("unsafe opened learning file")); }
    let mut bytes = Vec::new(); file.take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_BYTES { return Err(invalid("oversized learning file")); }
    let document: Document = serde_json::from_slice(&bytes).map_err(|_| invalid("damaged learning document"))?;
    validate(&document)?; Ok((document, bytes.len()))
}

fn status(document: &Document, actual: usize) -> LearningStatus {
    LearningStatus { entries: document.entries.len(), choices: document.choices.len(), actual_bytes: actual,
        estimated_bytes: estimated_bytes(document), max_bytes: MAX_BYTES, max_entries: MAX_ENTRIES,
        max_age_days: MAX_AGE_MS / (24 * 60 * 60 * 1000) }
}

fn words(document: &Document) -> Vec<LearningWordBackup> {
    let mut grouped: BTreeMap<ManagementKey, LearningWordBackup> = BTreeMap::new();
    let make = |key| LearningWordBackup { format_version: 1, key, rime_tick: document.rime_tick,
        migrated: document.migrated, owner_seeded: document.owner_seeded,
        legacy_choices_imported: document.legacy_choices_imported, entries: vec![], choices: vec![] };
    for entry in &document.entries {
        let k = key(&entry.reading, &entry.text); grouped.entry(k.clone()).or_insert_with(|| make(k)).entries.push(entry.clone());
    }
    for choice in &document.choices {
        let k = key(&choice.reading, &choice.text); grouped.entry(k.clone()).or_insert_with(|| make(k)).choices.push(choice.clone());
    }
    grouped.into_values().collect()
}

pub fn management_list(directory: &Path) -> io::Result<LearningManagementSnapshot> {
    let (document, bytes) = read_strict(directory)?;
    Ok(LearningManagementSnapshot { items: words(&document), status: status(&document, bytes) })
}

fn ensure_fits(document: &Document) -> io::Result<Vec<u8>> {
    validate(document)?;
    let bytes = serde_json::to_vec(document).map_err(|_| invalid("cannot encode learning document"))?;
    if bytes.len() as u64 > MAX_BYTES || estimated_bytes(document) > MAX_BYTES as usize {
        return Err(io::Error::new(io::ErrorKind::StorageFull, "learning capacity would be exceeded; no records changed"));
    }
    Ok(bytes)
}

fn transaction<T>(directory: &Path, edit: impl FnOnce(&mut Document) -> io::Result<T>) -> io::Result<T> {
    transaction_journaled(directory, edit, |_| Ok(()))
}

fn transaction_journaled<T>(directory: &Path, edit: impl FnOnce(&mut Document) -> io::Result<T>,
    journal: impl FnOnce(&T) -> io::Result<()>) -> io::Result<T> {
    let directory = physical_runtime_path(directory);
    let root = open_or_create_owned_leaf_directory(&directory, 0o700)?;
    validate_owned_directory(&root, "learning management parent")?;
    let _lock = learning_lock::LearningLock::acquire(&directory)?;
    let (mut document, _) = read_strict(&directory)?;
    let result = edit(&mut document)?; let bytes = ensure_fits(&document)?;
    // Persist the caller's undo journal before replacing any learning data.
    // The shared lock keeps the journal's facts equal to the actual deletion.
    journal(&result)?;
    // Publish the invalidation while holding the same nonblocking lock the
    // reader acquires before refreshing. A failed later data write causes only
    // a harmless reload; an early reader cannot acknowledge uncommitted data.
    mark_management_change(&directory)?;
    let temporary = directory.join(format!(".{FILE_NAME}.{}.management.tmp", std::process::id()));
    let mut file = private_create(OpenOptions::new().write(true).create_new(true)).open(&temporary)?;
    let saved = (|| { file.write_all(&bytes)?; file.sync_all()?; drop(file);
        validate_owned_directory(&root, "learning management parent")?;
        fs::rename(&temporary, directory.join(FILE_NAME))?; sync_directory(&directory) })();
    if saved.is_err() { let _ = fs::remove_file(temporary); }
    saved?; Ok(result)
}

pub fn management_delete(directory: &Path, selected: &[ManagementKey]) -> io::Result<Vec<LearningWordBackup>> {
    management_delete_journaled(directory, selected, |_| Ok(()))
}

/// The callback must durably save the undo payload and must not re-enter a
/// learning write: it runs while LearningLock protects the selected facts.
pub fn management_delete_journaled(directory: &Path, selected: &[ManagementKey],
    journal: impl FnOnce(&[LearningWordBackup]) -> io::Result<()>) -> io::Result<Vec<LearningWordBackup>> {
    if selected.is_empty() || selected.len() > MAX_ENTRIES { return Err(invalid("select a bounded set of learned words")); }
    let requested: HashSet<_> = selected.iter().cloned().collect();
    if requested.len() != selected.len() || selected.iter().any(|k| !valid_word(&k.reading, &k.text)) {
        return Err(invalid("invalid or duplicate learned-word selection"));
    }
    transaction_journaled(directory, |document| {
        let removed: Vec<_> = words(document).into_iter().filter(|item| requested.contains(&item.key)).collect();
        if removed.len() != requested.len() { return Err(io::Error::new(io::ErrorKind::NotFound, "learning selection changed; refresh first")); }
        document.entries.retain(|entry| !requested.contains(&key(&entry.reading, &entry.text)));
        document.choices.retain(|choice| !requested.contains(&key(&choice.reading, &choice.text)));
        Ok(removed)
    }, |removed| journal(removed))
}

fn merge(document: &mut Document, incoming: &[LearningWordBackup]) -> io::Result<()> {
    if incoming.len() > MAX_ENTRIES * 2 || incoming.iter().map(|w| w.entries.len()).sum::<usize>() > MAX_ENTRIES
        || incoming.iter().map(|w| w.choices.len()).sum::<usize>() > MAX_ENTRIES {
        return Err(invalid("unbounded learning restore payload"));
    }
    let mut keys = HashSet::new();
    for word in incoming {
        if word.format_version != 1 || !valid_word(&word.key.reading, &word.key.text) || !keys.insert(word.key.clone())
            || word.rime_tick == u64::MAX || word.entries.len() > 1 || word.choices.len() > MAX_ENTRIES
            || word.entries.iter().any(|e| key(&e.reading, &e.text) != word.key || !valid_entry(e)
                || e.rime.as_ref().is_some_and(|w| w.tick > word.rime_tick))
            || word.choices.iter().any(|e| key(&e.reading, &e.text) != word.key || !valid_choice(e)) {
            return Err(invalid("invalid learning restore payload"));
        }
        let mut choices = HashSet::new();
        if !word.choices.iter().all(|c| choices.insert(choice_key(c))) { return Err(invalid("duplicate restore choices")); }
    }
    for word in incoming {
        document.rime_tick = document.rime_tick.max(word.rime_tick);
        document.migrated |= word.migrated; document.owner_seeded |= word.owner_seeded;
        document.legacy_choices_imported |= word.legacy_choices_imported;
    }
    for word in incoming {
        for entry in &word.entries {
            if let Some(current) = document.entries.iter_mut().find(|current| same_entry(current, entry)) {
                current.count = current.count.max(entry.count); current.last_ms = current.last_ms.max(entry.last_ms);
                current.rime = match (&current.rime, &entry.rime) {
                    (Some(a), Some(b)) => Some(RimeWeight { commits: a.commits.max(b.commits),
                        dynamic: a.decayed(document.rime_tick).max(b.decayed(document.rime_tick)), tick: document.rime_tick,
                        updates: a.updates.max(b.updates) }),
                    (Some(a), None) => Some(a.clone()), (None, b) => b.clone(),
                };
            } else { document.entries.push(entry.clone()); }
        }
        for choice in &word.choices {
            if let Some(current) = document.choices.iter_mut().find(|current| same_choice(current, choice)) {
                let count = current.count.max(choice.count);
                if choice.last_ms > current.last_ms { *current = choice.clone(); }
                current.count = count;
            } else { document.choices.push(choice.clone()); }
        }
    }
    Ok(())
}

pub fn management_restore(directory: &Path, backup: &[LearningWordBackup]) -> io::Result<LearningImportPreview> {
    if backup.len() > MAX_ENTRIES * 2 { return Err(invalid("too many restore words")); }
    transaction(directory, |document| {
        merge(document, backup)?;
        Ok(LearningImportPreview { entries: backup.iter().map(|w| w.entries.len()).sum(),
            choices: backup.iter().map(|w| w.choices.len()).sum(), estimated_bytes: estimated_bytes(document) })
    })
}

pub fn management_export(directory: &Path) -> io::Result<String> {
    let (document, _) = read_strict(directory)?;
    serde_json::to_string_pretty(&Backup { format_version: 1, kind: "zhixu-learning-backup".into(), contains_secrets: false, document })
        .map_err(|_| invalid("cannot encode learning backup"))
}

fn parse_backup(text: &str) -> io::Result<Backup> {
    if text.len() > MAX_IMPORT_BYTES { return Err(invalid("oversized learning backup")); }
    let backup: Backup = serde_json::from_str(text).map_err(|_| invalid("invalid learning backup format"))?;
    if backup.format_version != 1 || backup.kind != "zhixu-learning-backup" || backup.contains_secrets {
        return Err(invalid("unsupported learning backup"));
    }
    validate(&backup.document)?; Ok(backup)
}

pub fn management_preview(directory: &Path, text: &str) -> io::Result<LearningImportPreview> {
    let incoming = parse_backup(text)?; let (mut current, _) = read_strict(directory)?;
    merge_document(&mut current, &incoming.document)?; ensure_fits(&current)?;
    Ok(LearningImportPreview { entries: incoming.document.entries.len(), choices: incoming.document.choices.len(),
        estimated_bytes: estimated_bytes(&current) })
}

pub fn management_import(directory: &Path, text: &str) -> io::Result<LearningImportPreview> {
    let incoming = parse_backup(text)?;
    transaction(directory, |current| {
        merge_document(current, &incoming.document)?;
        Ok(LearningImportPreview { entries: incoming.document.entries.len(), choices: incoming.document.choices.len(),
            estimated_bytes: estimated_bytes(current) })
    })
}

fn merge_document(current: &mut Document, incoming: &Document) -> io::Result<()> {
    merge(current, &words(incoming))?;
    current.rime_tick = current.rime_tick.max(incoming.rime_tick);
    current.migrated |= incoming.migrated; current.owner_seeded |= incoming.owner_seeded;
    current.legacy_choices_imported |= incoming.legacy_choices_imported;
    let mut ids = HashSet::new();
    let mut merged = VecDeque::new();
    for id in incoming.delivery_ids.iter().chain(&current.delivery_ids) {
        if ids.insert(*id) { merged.push_back(*id); }
    }
    while merged.len() > MAX_DELIVERY_IDS { merged.pop_front(); }
    current.delivery_ids = merged;
    Ok(())
}

pub(super) fn checked_store(directory: &Path, now: u64) -> io::Result<LearningStore> {
    let (document, _) = read_strict(directory)?;
    let mut store = LearningStore { entries: Arc::new(document.entries), choices: document.choices,
        migrated: document.migrated, rime_tick: document.rime_tick, owner_seeded: document.owner_seeded,
        legacy_choices_imported: document.legacy_choices_imported, delivery_ids: document.delivery_ids,
        ..LearningStore::default() };
    store.limit(); store.baseline = Some(Box::new(store.document())); store.expire(now); store.limit();
    Ok(store)
}

const REVISION_FILE: &str = ".agent_learning_management_revision";

pub(crate) fn management_revision(directory: &Path) -> io::Result<Option<String>> {
    let path = physical_runtime_path(directory).join(REVISION_FILE);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(m) => m, Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None), Err(e) => return Err(e),
    };
    if !private_regular_file(&metadata) || metadata.len() > 128 { return Err(invalid("unsafe management revision")); }
    let mut options = OpenOptions::new(); options.read(true);
    #[cfg(unix)] {
        use std::os::unix::fs::OpenOptionsExt;
        #[cfg(target_os = "macos")] options.custom_flags(0x100);
        #[cfg(target_os = "linux")] options.custom_flags(0x0002_0000);
    }
    let file = options.open(path)?;
    if !private_regular_file(&file.metadata()?) { return Err(invalid("unsafe opened management revision")); }
    let mut text = String::new(); file.take(129).read_to_string(&mut text)?;
    if text.is_empty() || text.len() > 128 || !text.bytes().all(|c| c.is_ascii_digit() || c == b'-') {
        return Err(invalid("invalid management revision"));
    }
    Ok(Some(text))
}

/// Caller holds LearningLock. This sidecar is not part of the learning format.
pub(crate) fn mark_management_change(directory: &Path) -> io::Result<()> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let _ = management_revision(directory)?;
    let text = format!("{}-{}-{}", std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos(),
        NEXT.fetch_add(1, Ordering::Relaxed));
    let temporary = directory.join(format!(".{REVISION_FILE}.{}.tmp", std::process::id()));
    let mut file = private_create(OpenOptions::new().write(true).create_new(true)).open(&temporary)?;
    let result = (|| { file.write_all(text.as_bytes())?; file.sync_all()?; drop(file);
        fs::rename(&temporary, directory.join(REVISION_FILE))?; sync_directory(directory) })();
    if result.is_err() { let _ = fs::remove_file(temporary); }
    result
}

pub(crate) fn refreshed_store(directory: &Path, now: u64) -> io::Result<(LearningStore, Option<String>)> {
    let _lock = learning_lock::LearningLock::acquire(directory)?;
    let revision = management_revision(directory)?;
    Ok((checked_store(directory, now)?, revision))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    #[test]
    fn durable_journal_precedes_deletion_and_failed_journal_preserves_bytes() {
        let dir = Scratch::new(); dir.seed();
        let path = dir.0.join(FILE_NAME); let before = fs::read(&path).unwrap();
        let selected = [ManagementKey { reading: "ni hao".into(), text: "你好".into() }];
        assert!(management_delete_journaled(&dir.0, &selected, |_| Err(io::Error::other("journal unavailable"))).is_err());
        assert_eq!(before, fs::read(&path).unwrap());
        assert!(management_revision(&dir.0).unwrap().is_none());
        let journal = dir.0.join("undo-test.json");
        let removed = management_delete_journaled(&dir.0, &selected, |facts| {
            assert_eq!(before, fs::read(&path)?);
            fs::write(&journal, serde_json::to_vec(facts).unwrap())?;
            OpenOptions::new().write(true).open(&journal)?.sync_all()
        }).unwrap();
        assert_eq!(serde_json::from_slice::<Vec<LearningWordBackup>>(&fs::read(journal).unwrap()).unwrap(), removed);
        assert_eq!(management_list(&dir.0).unwrap().items.len(), 1);
    }
    struct Scratch(std::path::PathBuf);
    impl Scratch {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(1);
            let path = physical_runtime_path(&std::env::temp_dir()).join(format!("learning-management-{}-{}",
                std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            open_or_create_owned_leaf_directory(&path, 0o700).unwrap(); Self(path)
        }
        fn seed(&self) {
            let mut store = LearningStore::load(&self.0, 100);
            store.record("ni hao", "你好", 100); store.record("ni hao", "你好", 101);
            store.record("zhong guo", "中国", 102); store.save(&self.0).unwrap();
        }
    }
    impl Drop for Scratch { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); } }

    #[test]
    fn list_is_read_only_and_missing_profile_stays_missing() {
        let dir = Scratch::new(); let missing = dir.0.join("missing");
        assert!(management_list(&missing).unwrap().items.is_empty()); assert!(!missing.exists());
        dir.seed(); let before = fs::read(dir.0.join(FILE_NAME)).unwrap();
        let listed = management_list(&dir.0).unwrap(); assert_eq!(listed.status.entries, 2);
        assert_eq!(listed.status.actual_bytes, before.len());
        assert_eq!(fs::read(dir.0.join(FILE_NAME)).unwrap(), before);
    }
    #[test]
    fn selected_delete_and_undo_keep_peer_facts_and_restore_idempotently() {
        let dir = Scratch::new(); dir.seed();
        let removed = management_delete(&dir.0, &[key("ni hao", "你好")]).unwrap();
        let mut peer = LearningStore::load(&dir.0, 103); peer.record("bei jing", "北京", 104); peer.save(&dir.0).unwrap();
        management_restore(&dir.0, &removed).unwrap();
        let once = fs::read(dir.0.join(FILE_NAME)).unwrap(); management_restore(&dir.0, &removed).unwrap();
        assert_eq!(fs::read(dir.0.join(FILE_NAME)).unwrap(), once);
        let listed = management_list(&dir.0).unwrap(); assert_eq!(listed.status.entries, 3);
        assert!(listed.items.iter().any(|w| w.key.text == "北京"));
        assert_eq!(listed.items.iter().find(|w| w.key.text == "你好").unwrap().entries[0].count, 2);
    }
    #[test]
    fn stale_active_session_does_not_resurrect_deleted_facts() {
        let dir = Scratch::new(); dir.seed(); let mut stale = LearningStore::load(&dir.0, 103);
        management_delete(&dir.0, &[key("ni hao", "你好")]).unwrap();
        stale.record("shang hai", "上海", 104); stale.save(&dir.0).unwrap();
        let listed = management_list(&dir.0).unwrap();
        assert!(!listed.items.iter().any(|w| w.key.text == "你好"));
        assert!(listed.items.iter().any(|w| w.key.text == "上海"));
    }
    #[test]
    fn backup_merges_without_lowering_current_counts_or_times() {
        let dir = Scratch::new(); dir.seed(); let backup = management_export(&dir.0).unwrap();
        let mut current = LearningStore::load(&dir.0, 103); current.record("ni hao", "你好", 104);
        current.record("bei jing", "北京", 105); current.save(&dir.0).unwrap();
        let preview = management_preview(&dir.0, &backup).unwrap(); assert_eq!(preview.entries, 2);
        let before = fs::read(dir.0.join(FILE_NAME)).unwrap();
        management_import(&dir.0, &backup).unwrap(); assert_eq!(fs::read(dir.0.join(FILE_NAME)).unwrap(), before);
        let store = LearningStore::load(&dir.0, 105);
        let word = store.entries().iter().find(|e| e.text == "你好").unwrap(); assert_eq!((word.count, word.last_ms), (3,104));
    }
    #[test]
    fn malformed_and_stale_requests_do_not_change_the_file() {
        let dir = Scratch::new(); dir.seed(); let before = fs::read(dir.0.join(FILE_NAME)).unwrap();
        assert!(management_import(&dir.0, "{}").is_err());
        assert!(management_delete(&dir.0, &[key("bei jing", "北京")]).is_err());
        let mut backup: Backup = serde_json::from_str(&management_export(&dir.0).unwrap()).unwrap();
        backup.document.entries[0].count = 0;
        assert!(management_import(&dir.0, &serde_json::to_string(&backup).unwrap()).is_err());
        assert_eq!(fs::read(dir.0.join(FILE_NAME)).unwrap(), before);
    }
    #[test]
    fn read_and_edit_fail_closed_on_a_damaged_existing_file() {
        let dir = Scratch::new(); dir.seed(); let exported = management_export(&dir.0).unwrap();
        fs::write(dir.0.join(FILE_NAME), b"broken").unwrap();
        assert!(management_list(&dir.0).is_err()); assert!(management_import(&dir.0, &exported).is_err());
        assert_eq!(fs::read(dir.0.join(FILE_NAME)).unwrap(), b"broken");
    }
    #[test]
    fn capacity_rejection_keeps_every_existing_record() {
        let dir = Scratch::new(); dir.seed(); let before = fs::read(dir.0.join(FILE_NAME)).unwrap();
        let mut incoming = LearningStore::default().document();
        for index in 0..4096u32 {
            let text = char::from_u32(0x4e00 + index).unwrap().to_string();
            incoming.entries.push(Entry { reading:"ta".into(),text,count:1,last_ms:100,rime:None });
        }
        let backup = serde_json::to_string(&Backup { format_version:1,kind:"zhixu-learning-backup".into(),contains_secrets:false,document:incoming }).unwrap();
        assert!(management_import(&dir.0, &backup).is_err()); assert_eq!(fs::read(dir.0.join(FILE_NAME)).unwrap(), before);
    }
    #[test]
    fn a_busy_writer_returns_without_mutating_or_waiting() {
        let dir = Scratch::new(); dir.seed(); let before = fs::read(dir.0.join(FILE_NAME)).unwrap();
        let _held = learning_lock::LearningLock::acquire(&dir.0).unwrap();
        assert!(management_delete(&dir.0, &[key("ni hao", "你好")]).is_err());
        assert_eq!(fs::read(dir.0.join(FILE_NAME)).unwrap(), before);
    }
    #[cfg(unix)]
    #[test]
    fn symlink_or_public_learning_file_is_never_modified() {
        use std::os::unix::fs::{symlink,PermissionsExt};
        let dir = Scratch::new(); dir.seed(); let original = dir.0.join("original");
        fs::rename(dir.0.join(FILE_NAME), &original).unwrap(); symlink(&original, dir.0.join(FILE_NAME)).unwrap();
        assert!(management_list(&dir.0).is_err()); assert!(management_delete(&dir.0, &[key("ni hao","你好")]).is_err());
        fs::remove_file(dir.0.join(FILE_NAME)).unwrap(); fs::rename(original,dir.0.join(FILE_NAME)).unwrap();
        fs::set_permissions(dir.0.join(FILE_NAME),fs::Permissions::from_mode(0o644)).unwrap(); assert!(management_list(&dir.0).is_err());
    }
}
