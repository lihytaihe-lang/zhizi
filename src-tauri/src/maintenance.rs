use crate::hotword::{DynamicSource, DynamicSourceInput};
use crate::knowledge::KnowledgeSourceInput;
use crate::replay_gate::{CompositionOutcome, RecordedQualityEvent};
use crate::storage::{
    insert_lexicon_entry_in_transaction, validate_lexicon_input, AppStorage, LexiconEntry,
    LexiconEntryInput, ProjectProfileInput, ProjectRuntimeConfiguration,
};
use crate::userdb_maintenance::UserDbBackupItem;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs::File,
    io::{ErrorKind, Read},
    path::Path,
    time::Instant,
};

const BACKUP_FORMAT_VERSION: u32 = 1;
const MAX_BACKUP_BYTES: usize = 16 * 1024 * 1024;
const MAX_MODEL_AUDIT_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LearningPreferences {
    pub app_learning: bool,
    pub project_learning: bool,
    pub secure_input_learning: bool,
    pub updated_at_ms: i64,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LearningCleanupPreview {
    pub entry_count: usize,
    pub entry_ids: Vec<String>,
    pub texts: Vec<String>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LearningCleanupResult {
    pub batch_id: String,
    pub deleted_count: usize,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LearningCleanupItem {
    pub id: String,
    pub text: String,
    pub pinyin: String,
    pub source: String,
    pub domain: Option<String>,
    pub learned_at_ms: Option<i64>,
    pub last_hit_at_ms: Option<i64>,
    pub factory_top1: Option<String>,
    pub displaced_factory_top1: bool,
    pub attribution: Option<String>,
    pub batch_cleanup_eligible: bool,
    pub friction_event_count: usize,
    pub abandoned_count: usize,
    pub deep_page_count: usize,
    pub backspace_count: usize,
    pub suspected: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_audit: Option<ModelAuditEvidence>,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ModelAuditEvidence {
    pub suggested: String,
    pub flagged: usize,
    pub occurrences: usize,
    pub avg_margin: f64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ModelAuditPair {
    pub pinyin: String,
    pub committed: String,
    pub suggested: String,
    pub occurrences: usize,
    pub flagged: usize,
    #[serde(rename(deserialize = "avg_margin", serialize = "avgMargin"))]
    pub avg_margin: f64,
}

#[derive(Debug, Deserialize)]
struct ModelAuditDocument {
    schema: u32,
    pairs: Vec<ModelAuditPair>,
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LearningCleanupList {
    pub items: Vec<LearningCleanupItem>,
    pub unmatched_model_audit_pairs: Vec<ModelAuditPair>,
}

fn read_model_audit_pairs(quality_directory: &Path) -> Vec<ModelAuditPair> {
    let read = || -> Result<Vec<ModelAuditPair>, &'static str> {
        let file = match File::open(quality_directory.join("model_audit/silent_errors.v1.json")) {
            Ok(file) => file,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
            Err(_) => return Err("cannot open file"),
        };
        if file.metadata().map_err(|_| "cannot inspect file")?.len() > MAX_MODEL_AUDIT_BYTES {
            return Err("file exceeds 1 MiB");
        }
        // Also bound the read if the offline producer replaces/grows the file.
        let mut bytes = Vec::new();
        file.take(MAX_MODEL_AUDIT_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "cannot read file")?;
        if bytes.len() as u64 > MAX_MODEL_AUDIT_BYTES {
            return Err("file exceeds 1 MiB");
        }
        // Deliberately ignore examples and model metadata: preceding text never
        // leaves this file through the maintenance command or its diagnostics.
        let document: ModelAuditDocument =
            serde_json::from_slice(&bytes).map_err(|_| "invalid JSON")?;
        if document.schema != 1 {
            return Err("unsupported schema");
        }
        if document.pairs.iter().any(|pair| {
            pair.pinyin.is_empty()
                || !pair.pinyin.bytes().all(|byte| byte.is_ascii_lowercase())
                || pair.committed.is_empty()
                || pair.suggested.is_empty()
                || pair.flagged > pair.occurrences
                || !pair.avg_margin.is_finite()
        }) {
            return Err("invalid pair");
        }
        Ok(document.pairs)
    };
    read().unwrap_or_else(|reason| {
        eprintln!("model audit ignored: {reason}");
        Vec::new()
    })
}

fn model_audit_reading_matches(reading: &str, typed: &str) -> bool {
    let syllables = reading.split_whitespace().collect::<Vec<_>>();
    if syllables.is_empty()
        || typed.is_empty()
        || !typed.bytes().all(|byte| byte.is_ascii_lowercase())
        || syllables
            .iter()
            .any(|syllable| !syllable.bytes().all(|byte| byte.is_ascii_lowercase()))
    {
        return false;
    }
    // Each syllable must contribute a nonempty prefix. This accepts mixed full
    // spelling/initials and a truncated final syllable without substring matches.
    let mut positions = vec![false; typed.len() + 1];
    positions[0] = true;
    for syllable in syllables {
        let mut next = vec![false; typed.len() + 1];
        for (position, reachable) in positions.iter().enumerate() {
            if !reachable {
                continue;
            }
            for length in 1..=syllable.len().min(typed.len() - position) {
                if typed[position..].starts_with(&syllable[..length]) {
                    next[position + length] = true;
                }
            }
        }
        positions = next;
    }
    positions[typed.len()]
}

pub(crate) fn learning_cleanup_list_at(
    mut items: Vec<LearningCleanupItem>,
    events: &[RecordedQualityEvent],
    quality_directory: &Path,
) -> LearningCleanupList {
    let pairs = read_model_audit_pairs(quality_directory);
    let mut matched = vec![false; pairs.len()];
    for item in &mut items {
        item.model_audit = None;
        for (index, pair) in pairs.iter().enumerate() {
            if pair.committed != item.text
                || !model_audit_reading_matches(&item.pinyin, &pair.pinyin)
            {
                continue;
            }
            matched[index] = true;
            // Multiple typed readings may identify the same learning record.
            // Keep the strongest pair, never sum possibly overlapping counts.
            if item
                .model_audit
                .as_ref()
                .is_none_or(|audit| pair.flagged > audit.flagged)
            {
                item.model_audit = Some(ModelAuditEvidence {
                    suggested: pair.suggested.clone(),
                    flagged: pair.flagged,
                    occurrences: pair.occurrences,
                    avg_margin: pair.avg_margin,
                });
            }
        }
    }
    LearningCleanupList {
        items: rank_learning_cleanup_items(items, events),
        unmatched_model_audit_pairs: pairs
            .into_iter()
            .zip(matched)
            .filter_map(|(pair, matched)| (!matched).then_some(pair))
            .collect(),
    }
}

#[cfg(feature = "personal-seed")]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LearningCleanupEvidenceCorpus {
    cases: Vec<LearningCleanupEvidenceCase>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LearningCleanupEvidenceCase {
    query: String,
    clean_top1: String,
    observed_owner_top1: String,
    attribution: Option<String>,
    batch_cleanup_eligible: bool,
}

fn canonical_reading(value: &str) -> String {
    value
        .bytes()
        .filter(|byte| byte.is_ascii_lowercase())
        .map(char::from)
        .collect()
}

#[cfg(feature = "personal-seed")]
fn frozen_learning_cleanup_cases() -> Vec<LearningCleanupEvidenceCase> {
    serde_json::from_str::<LearningCleanupEvidenceCorpus>(include_str!(
        "../../quality/corpus/learning-cleanup-evidence-v3.json"
    ))
    .expect("learning cleanup corpus must keep its frozen evidence fields")
    .cases
}

#[cfg(not(feature = "personal-seed"))]
fn frozen_learning_cleanup_cases() -> Vec<LearningCleanupEvidenceCase> {
    Vec::new()
}

pub(crate) fn rank_learning_cleanup_items(
    mut items: Vec<LearningCleanupItem>,
    events: &[RecordedQualityEvent],
) -> Vec<LearningCleanupItem> {
    let frozen_cases = frozen_learning_cleanup_cases()
        .into_iter()
        .map(|case| (canonical_reading(&case.query), case))
        .collect::<HashMap<_, _>>();
    let mut events_by_reading = HashMap::<String, Vec<&RecordedQualityEvent>>::new();
    for event in events {
        events_by_reading
            .entry(canonical_reading(&event.pinyin))
            .or_default()
            .push(event);
    }
    for item in &mut items {
        let reading = canonical_reading(&item.pinyin);
        if let Some(case) = frozen_cases.get(&reading) {
            item.factory_top1 = Some(case.clean_top1.clone());
            item.attribution = case.attribution.clone();
            // This is deliberately an observed, frozen comparison rather than
            // a weighted guess. Batch eligibility additionally requires the
            // corpus attribution to say learned-pollution; ambiguous homophones
            // and single-initial observations remain evidence but cannot enter
            // the grouped cleanup.
            item.displaced_factory_top1 = case.batch_cleanup_eligible
                && case.clean_top1 != item.text
                && case.observed_owner_top1 == item.text;
            item.batch_cleanup_eligible = item.displaced_factory_top1;
        }

        for event in events_by_reading
            .get(&reading)
            .into_iter()
            .flat_map(|events| events.iter().copied())
        {
            let item_was_visible = event.selected_text == item.text
                || event
                    .candidates
                    .iter()
                    .any(|candidate| candidate == &item.text);
            if !item_was_visible {
                continue;
            }
            if event.selected_text == item.text && event.outcome != CompositionOutcome::Abandoned {
                item.last_hit_at_ms =
                    Some(item.last_hit_at_ms.unwrap_or(0).max(event.recorded_at_ms));
            }
            let suffered = event.outcome == CompositionOutcome::Abandoned
                || event.max_page > 0
                || event.backspaces > 0;
            item.friction_event_count += usize::from(suffered);
            item.abandoned_count += usize::from(event.outcome == CompositionOutcome::Abandoned);
            item.deep_page_count += usize::from(event.max_page > 0);
            item.backspace_count += event.backspaces;
        }
        item.suspected = item.displaced_factory_top1
            || item.model_audit.is_some()
            || item.friction_event_count > 0;
    }

    // Explainable and exactly recalculable ordering, with no invented weights:
    // suspected entries first; among them the frozen A-arm displacement is the
    // strongest signal, followed by model flags and distinct suffering events,
    // recent evidence and actual learning time. The rest is a stable tie-break.
    items.sort_by(|left, right| {
        right
            .suspected
            .cmp(&left.suspected)
            .then_with(|| {
                right
                    .displaced_factory_top1
                    .cmp(&left.displaced_factory_top1)
            })
            .then_with(|| {
                right
                    .model_audit
                    .as_ref()
                    .map(|audit| audit.flagged)
                    .cmp(&left.model_audit.as_ref().map(|audit| audit.flagged))
            })
            .then_with(|| right.friction_event_count.cmp(&left.friction_event_count))
            .then_with(|| right.last_hit_at_ms.cmp(&left.last_hit_at_ms))
            .then_with(|| right.learned_at_ms.cmp(&left.learned_at_ms))
            .then_with(|| {
                (&left.pinyin, &left.text, &left.source, &left.id).cmp(&(
                    &right.pinyin,
                    &right.text,
                    &right.source,
                    &right.id,
                ))
            })
    });
    items
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LearningCleanupBatchDocument {
    format_version: u32,
    sqlite_entries: Vec<LexiconEntryInput>,
    userdb_entries: Vec<UserDbBackupItem>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MaintenanceBackup {
    format_version: u32,
    created_at_ms: i64,
    secret_policy: String,
    lexicon_entries: Vec<LexiconEntryInput>,
    project_profiles: Vec<ProjectProfileInput>,
    runtime: ProjectRuntimeConfiguration,
    dynamic_sources: Vec<DynamicSource>,
    knowledge_sources: Vec<KnowledgeSourceInput>,
    safe_settings: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BackupPreview {
    pub format_version: u32,
    pub created_at_ms: i64,
    pub lexicon_count: usize,
    pub project_count: usize,
    pub dynamic_source_count: usize,
    pub knowledge_source_count: usize,
    pub contains_secrets: bool,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct IntegrityReport {
    pub quick_check: String,
    pub foreign_key_errors: usize,
    pub schema_version: i64,
    pub expected_schema_version: i64,
    pub lexicon_count: usize,
    pub knowledge_chunk_count: usize,
    pub recovery_contract: String,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ScaleProbeReport {
    pub personal_entry_count: usize,
    pub knowledge_chunk_count: usize,
    pub personal_query_milliseconds: f64,
    pub knowledge_query_milliseconds: f64,
    pub passed: bool,
}

fn entry_input(entry: LexiconEntry) -> LexiconEntryInput {
    LexiconEntryInput {
        id: entry.id,
        text: entry.text,
        pinyin: entry.pinyin,
        aliases: entry.aliases,
        professional_packs: entry.professional_packs,
        project_domains: entry.project_domains,
        sources: entry.sources,
        base_weight: entry.base_weight,
        fixed: entry.fixed,
        disabled: entry.disabled,
        demoted: entry.demoted,
        updated_at_ms: entry.updated_at_ms,
    }
}

fn parse_backup(document: &str) -> Result<MaintenanceBackup, String> {
    if document.len() > MAX_BACKUP_BYTES {
        return Err("备份超过 16 MiB 上限。".into());
    }
    let backup: MaintenanceBackup =
        serde_json::from_str(document).map_err(|error| format!("备份格式无效：{error}"))?;
    if backup.format_version != BACKUP_FORMAT_VERSION {
        return Err(format!("不支持的备份版本：{}。", backup.format_version));
    }
    if backup.secret_policy != "keychain-and-provider-secrets-excluded" {
        return Err("备份缺少无密钥策略声明。".into());
    }
    if backup.created_at_ms < 0 {
        return Err("备份时间无效。".into());
    }
    let mut ids = HashSet::new();
    for entry in &backup.lexicon_entries {
        validate_lexicon_input(entry.clone())?;
        if !ids.insert(entry.id.clone()) {
            return Err("备份包含重复个人词 ID。".into());
        }
    }
    for profile in &backup.project_profiles {
        if profile.id.trim().is_empty()
            || profile.name.trim().is_empty()
            || profile.project_domain.trim().is_empty()
        {
            return Err("备份包含无效项目 Profile。".into());
        }
    }
    Ok(backup)
}

impl AppStorage {
    pub fn learning_preferences(&self) -> Result<LearningPreferences, String> {
        let connection = self.lock_connection()?;
        connection.query_row("SELECT app_learning,project_learning,secure_input_learning,updated_at_ms FROM learning_preferences WHERE singleton=1", [], |row| Ok(LearningPreferences { app_learning:row.get::<_,i64>(0)? != 0, project_learning:row.get::<_,i64>(1)? != 0, secure_input_learning:row.get::<_,i64>(2)? != 0, updated_at_ms:row.get(3)? })).map_err(|error| format!("无法读取学习控制：{error}"))
    }

    pub fn save_learning_preferences(
        &self,
        mut preferences: LearningPreferences,
    ) -> Result<LearningPreferences, String> {
        preferences.secure_input_learning = false;
        if preferences.updated_at_ms < 0 {
            return Err("学习控制时间无效。".into());
        }
        let connection = self.lock_connection()?;
        connection.execute("UPDATE learning_preferences SET app_learning=?1,project_learning=?2,secure_input_learning=0,updated_at_ms=?3 WHERE singleton=1", params![preferences.app_learning as i64,preferences.project_learning as i64,preferences.updated_at_ms]).map_err(|error| format!("无法保存学习控制：{error}"))?;
        Ok(preferences)
    }

    pub(crate) fn learned_entries(&self) -> Result<Vec<LexiconEntry>, String> {
        Ok(self
            .list_lexicon_entries()?
            .into_iter()
            .filter(|entry| {
                entry
                    .sources
                    .iter()
                    .any(|source| source.source_kind == "learned")
            })
            .collect())
    }

    pub fn preview_learning_cleanup(
        &self,
        ids: Vec<String>,
        domain: Option<String>,
    ) -> Result<LearningCleanupPreview, String> {
        let requested = ids.into_iter().collect::<HashSet<_>>();
        let entries = self
            .learned_entries()?
            .into_iter()
            .filter(|entry| {
                (requested.is_empty() || requested.contains(&entry.id))
                    && domain
                        .as_ref()
                        .is_none_or(|domain| entry.project_domains.contains(domain))
            })
            .collect::<Vec<_>>();
        Ok(LearningCleanupPreview {
            entry_count: entries.len(),
            entry_ids: entries.iter().map(|entry| entry.id.clone()).collect(),
            texts: entries
                .iter()
                .take(20)
                .map(|entry| entry.text.clone())
                .collect(),
        })
    }

    pub fn apply_learning_cleanup_with_userdb(
        &self,
        ids: Vec<String>,
        domain: Option<String>,
        userdb_entries: Vec<UserDbBackupItem>,
        now_ms: i64,
    ) -> Result<LearningCleanupResult, String> {
        let preview = self.preview_learning_cleanup(ids, domain)?;
        if preview.entry_ids.is_empty() && userdb_entries.is_empty() {
            return Err("没有匹配的已学习词。".into());
        }
        let entries = self
            .learned_entries()?
            .into_iter()
            .filter(|entry| preview.entry_ids.contains(&entry.id))
            .map(entry_input)
            .collect::<Vec<_>>();
        let batch_id = format!("learning-cleanup-{now_ms}-{}", std::process::id());
        let document = serde_json::to_string(&LearningCleanupBatchDocument {
            format_version: 2,
            sqlite_entries: entries,
            userdb_entries: userdb_entries.clone(),
        })
        .map_err(|error| format!("无法编码学习撤销包：{error}"))?;
        let mut connection = self.lock_connection()?;
        let transaction = connection
            .transaction()
            .map_err(|error| format!("无法开始学习纠错事务：{error}"))?;
        transaction.execute("INSERT INTO learning_cleanup_batches (id,entries_json,created_at_ms) VALUES (?1,?2,?3)", params![&batch_id,document,now_ms]).map_err(|error| format!("无法记录学习撤销包：{error}"))?;
        for id in &preview.entry_ids {
            transaction
                .execute("DELETE FROM lexicon_entries WHERE id=?1", params![id])
                .map_err(|error| format!("无法删除误学词：{error}"))?;
        }
        transaction
            .commit()
            .map_err(|error| format!("无法提交学习纠错：{error}"))?;
        Ok(LearningCleanupResult {
            batch_id,
            deleted_count: preview.entry_count + userdb_entries.len(),
        })
    }

    pub fn learning_cleanup_userdb_entries(
        &self,
        batch_id: &str,
    ) -> Result<Vec<UserDbBackupItem>, String> {
        let connection = self.lock_connection()?;
        let document: String = connection
            .query_row(
                "SELECT entries_json FROM learning_cleanup_batches WHERE id=?1 AND undone_at_ms IS NULL",
                params![batch_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| format!("无法读取学习撤销包：{error}"))?
            .ok_or_else(|| "学习撤销包不存在或已使用。".to_string())?;
        if let Ok(batch) = serde_json::from_str::<LearningCleanupBatchDocument>(&document) {
            if batch.format_version != 2 {
                return Err("学习撤销包版本无效。".into());
            }
            return Ok(batch.userdb_entries);
        }
        serde_json::from_str::<Vec<LexiconEntryInput>>(&document)
            .map(|_| Vec::new())
            .map_err(|_| "学习撤销包损坏。".to_string())
    }

    pub fn undo_learning_cleanup(&self, batch_id: &str, now_ms: i64) -> Result<usize, String> {
        let mut connection = self.lock_connection()?;
        let transaction = connection
            .transaction()
            .map_err(|error| format!("无法开始学习撤销事务：{error}"))?;
        let document: String = transaction.query_row("SELECT entries_json FROM learning_cleanup_batches WHERE id=?1 AND undone_at_ms IS NULL", params![batch_id], |row| row.get(0)).optional().map_err(|error| format!("无法读取学习撤销包：{error}"))?.ok_or_else(|| "学习撤销包不存在或已使用。".to_string())?;
        let entries: Vec<LexiconEntryInput> =
            if let Ok(batch) = serde_json::from_str::<LearningCleanupBatchDocument>(&document) {
                if batch.format_version != 2 {
                    return Err("学习撤销包版本无效。".into());
                }
                batch.sqlite_entries
            } else {
                serde_json::from_str(&document).map_err(|_| "学习撤销包损坏。".to_string())?
            };
        for entry in &entries {
            insert_lexicon_entry_in_transaction(
                &transaction,
                &validate_lexicon_input(entry.clone())?,
            )?;
        }
        transaction
            .execute(
                "UPDATE learning_cleanup_batches SET undone_at_ms=?2 WHERE id=?1",
                params![batch_id, now_ms],
            )
            .map_err(|error| format!("无法完成学习撤销：{error}"))?;
        transaction
            .commit()
            .map_err(|error| format!("无法提交学习撤销：{error}"))?;
        Ok(entries.len())
    }

    pub fn export_maintenance_backup(&self, now_ms: i64) -> Result<String, String> {
        let connection = self.lock_connection()?;
        let safe_settings = {
            let mut statement = connection.prepare("SELECT key,value FROM settings WHERE key IN ('selectedModeId','shortcutSettings') ORDER BY key").map_err(|error| format!("无法准备安全设置备份：{error}"))?;
            let rows = statement
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(|error| format!("无法读取安全设置：{error}"))?;
            rows.collect::<Result<BTreeMap<_, _>, _>>()
                .map_err(|error| format!("无法解析安全设置：{error}"))?
        };
        drop(connection);
        let backup = MaintenanceBackup {
            format_version: BACKUP_FORMAT_VERSION,
            created_at_ms: now_ms,
            secret_policy: "keychain-and-provider-secrets-excluded".into(),
            lexicon_entries: self
                .list_lexicon_entries()?
                .into_iter()
                .map(entry_input)
                .collect(),
            project_profiles: self
                .list_project_profiles()?
                .into_iter()
                .map(|profile| ProjectProfileInput {
                    id: profile.id,
                    name: profile.name,
                    project_domain: profile.project_domain,
                    professional_packs: profile.professional_packs,
                    agent_context: profile.agent_context,
                    updated_at_ms: profile.updated_at_ms,
                })
                .collect(),
            runtime: self.project_runtime_configuration()?,
            dynamic_sources: self.list_dynamic_sources()?,
            knowledge_sources: self
                .list_knowledge_sources()?
                .into_iter()
                .map(|source| KnowledgeSourceInput {
                    id: source.id,
                    name: source.name,
                    source_type: source.source_type,
                    authorized_scope: source.authorized_scope,
                    authorization: source.authorization,
                    project_id: source.project_id,
                    sensitive_enabled: source.sensitive_enabled,
                    updated_at_ms: source.updated_at_ms,
                })
                .collect(),
            safe_settings,
        };
        serde_json::to_string_pretty(&backup).map_err(|error| format!("无法编码备份：{error}"))
    }

    pub fn preview_maintenance_backup(&self, document: &str) -> Result<BackupPreview, String> {
        let backup = parse_backup(document)?;
        Ok(BackupPreview {
            format_version: backup.format_version,
            created_at_ms: backup.created_at_ms,
            lexicon_count: backup.lexicon_entries.len(),
            project_count: backup.project_profiles.len(),
            dynamic_source_count: backup.dynamic_sources.len(),
            knowledge_source_count: backup.knowledge_sources.len(),
            contains_secrets: false,
        })
    }

    pub fn restore_maintenance_backup(&self, document: &str) -> Result<BackupPreview, String> {
        let backup = parse_backup(document)?;
        let preview = self.preview_maintenance_backup(document)?;
        let mut connection = self.lock_connection()?;
        let transaction = connection
            .transaction()
            .map_err(|error| format!("无法开始原子恢复：{error}"))?;
        transaction
            .execute("DELETE FROM lexicon_entries", [])
            .map_err(|error| format!("无法清理旧词库：{error}"))?;
        for entry in &backup.lexicon_entries {
            insert_lexicon_entry_in_transaction(
                &transaction,
                &validate_lexicon_input(entry.clone())?,
            )?;
        }
        transaction
            .execute("DELETE FROM project_profiles", [])
            .map_err(|error| format!("无法清理旧项目：{error}"))?;
        for profile in &backup.project_profiles {
            transaction.execute("INSERT INTO project_profiles (id,name,project_domain,agent_context,created_at_ms,updated_at_ms) VALUES (?1,?2,?3,?4,?5,?5)", params![profile.id.trim(),profile.name.trim(),profile.project_domain.trim(),profile.agent_context,profile.updated_at_ms]).map_err(|error| format!("无法恢复项目：{error}"))?;
            for pack in &profile.professional_packs {
                transaction.execute("INSERT INTO professional_packs (id,name) VALUES (?1,?1) ON CONFLICT(id) DO NOTHING", params![pack]).map_err(|error| format!("无法恢复词包：{error}"))?;
                transaction
                    .execute(
                        "INSERT INTO project_profile_packs (profile_id,pack_id) VALUES (?1,?2)",
                        params![profile.id, pack],
                    )
                    .map_err(|error| format!("无法恢复项目词包：{error}"))?;
            }
        }
        transaction
            .execute("DELETE FROM dynamic_sources", [])
            .map_err(|error| format!("无法清理旧动态源：{error}"))?;
        for source in &backup.dynamic_sources {
            restore_dynamic_source(&transaction, source)?;
        }
        transaction
            .execute("DELETE FROM knowledge_sources", [])
            .map_err(|error| format!("无法清理旧知识源：{error}"))?;
        for source in &backup.knowledge_sources {
            restore_knowledge_source(&transaction, source)?;
        }
        let active = backup.runtime.active_profile_id.as_ref().filter(|id| {
            backup
                .project_profiles
                .iter()
                .any(|profile| &profile.id == *id)
        });
        transaction.execute("UPDATE project_runtime_settings SET active_profile_id=?1,secondary_layer_enabled=?2,prediction_enabled=?3,candidate_theme=?4,context_model_ranking_enabled=?5 WHERE singleton=1", params![active,backup.runtime.secondary_layer_enabled as i64,backup.runtime.prediction_enabled as i64,backup.runtime.candidate_theme,backup.runtime.context_model_ranking_enabled as i64]).map_err(|error| format!("无法恢复运行配置：{error}"))?;
        for (key, value) in &backup.safe_settings {
            serde_json::from_str::<serde_json::Value>(value)
                .map_err(|_| "备份安全设置损坏。".to_string())?;
            transaction.execute("INSERT INTO settings (key,value,updated_at) VALUES (?1,?2,CURRENT_TIMESTAMP) ON CONFLICT(key) DO UPDATE SET value=excluded.value,updated_at=CURRENT_TIMESTAMP", params![key,value]).map_err(|error| format!("无法恢复安全设置：{error}"))?;
        }
        transaction
            .commit()
            .map_err(|error| format!("无法提交原子恢复：{error}"))?;
        Ok(preview)
    }

    pub fn integrity_report(&self) -> Result<IntegrityReport, String> {
        let connection = self.lock_connection()?;
        let quick_check: String = connection
            .query_row("PRAGMA quick_check", [], |row| row.get(0))
            .map_err(|error| format!("无法运行 SQLite quick_check：{error}"))?;
        let foreign_key_errors = {
            let mut statement = connection
                .prepare("PRAGMA foreign_key_check")
                .map_err(|error| format!("无法准备外键检查：{error}"))?;
            let rows = statement
                .query_map([], |_| Ok(()))
                .map_err(|error| format!("无法运行外键检查：{error}"))?;
            rows.count()
        };
        let schema_version: i64 = connection
            .query_row(
                "SELECT COALESCE(MAX(version),0) FROM schema_migrations",
                [],
                |row| row.get(0),
            )
            .map_err(|error| format!("无法读取 schema 版本：{error}"))?;
        let lexicon_count: usize = connection
            .query_row("SELECT COUNT(*) FROM lexicon_entries", [], |row| row.get(0))
            .map_err(|error| format!("无法统计词库：{error}"))?;
        let knowledge_chunk_count: usize = connection
            .query_row("SELECT COUNT(*) FROM knowledge_chunks", [], |row| {
                row.get(0)
            })
            .map_err(|error| format!("无法统计知识块：{error}"))?;
        Ok(IntegrityReport { quick_check,foreign_key_errors,schema_version,expected_schema_version:14,lexicon_count,knowledge_chunk_count,recovery_contract:"transactional migrations + validated backup restore + last-known-good snapshot/UserDB".into() })
    }
}

fn restore_dynamic_source(
    transaction: &rusqlite::Transaction<'_>,
    source: &DynamicSource,
) -> Result<(), String> {
    let input = DynamicSourceInput {
        id: source.id.clone(),
        display_name: source.display_name.clone(),
        source_type: source.source_type.clone(),
        refresh_interval_minutes: source.refresh_interval_minutes,
        authorization: source.authorization.clone(),
        terms_url: source.terms_url.clone(),
        license: source.license.clone(),
        cache_policy: source.cache_policy.clone(),
        ownership: source.ownership.clone(),
        distribution: source.distribution.clone(),
        disable_policy: source.disable_policy.clone(),
        endpoint: source.endpoint.clone(),
        opt_in: source.opt_in,
        updated_at_ms: source.updated_at_ms,
    };
    if input.id.trim().is_empty()
        || input.display_name.trim().is_empty()
        || input.authorization.trim().is_empty()
        || input.license.trim().is_empty()
    {
        return Err("备份包含未登记授权或许可证的动态源。".into());
    }
    transaction.execute("INSERT INTO dynamic_sources (id,display_name,source_type,refresh_interval_minutes,authorization,terms_url,license,cache_policy,ownership,distribution,disable_policy,endpoint,enabled,opt_in,last_refresh_ms,last_error,created_at_ms,updated_at_ms) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18)", params![source.id,source.display_name,source.source_type,source.refresh_interval_minutes,source.authorization,source.terms_url,source.license,source.cache_policy,source.ownership,source.distribution,source.disable_policy,source.endpoint,source.enabled as i64,source.opt_in as i64,source.last_refresh_ms,source.last_error,source.created_at_ms,source.updated_at_ms]).map_err(|error| format!("无法恢复动态源：{error}"))?;
    Ok(())
}

fn restore_knowledge_source(
    transaction: &rusqlite::Transaction<'_>,
    source: &KnowledgeSourceInput,
) -> Result<(), String> {
    if source.id.trim().is_empty()
        || source.name.trim().is_empty()
        || source.authorization.trim().is_empty()
    {
        return Err("备份包含无效知识源。".into());
    }
    let status = if source.source_type == "notion" {
        "deferred"
    } else {
        "authorized"
    };
    let project_id = if source.project_id.trim().is_empty() {
        source.id.trim()
    } else {
        source.project_id.trim()
    };
    transaction.execute("INSERT INTO knowledge_sources (id,name,source_type,authorized_scope,authorization,status,last_error,created_at_ms,updated_at_ms,project_id,retrieval_status,sensitive_enabled) VALUES (?1,?2,?3,?4,?5,?6,NULL,?7,?7,?8,'pending',?9)", params![source.id,source.name,source.source_type,source.authorized_scope,source.authorization,status,source.updated_at_ms,project_id,if source.sensitive_enabled { 1 } else { 0 }]).map_err(|error| format!("无法恢复知识源：{error}"))?;
    Ok(())
}

pub fn run_scale_probe() -> Result<ScaleProbeReport, String> {
    let mut connection =
        Connection::open_in_memory().map_err(|error| format!("无法创建隔离规模探针：{error}"))?;
    connection.execute_batch("CREATE TABLE personal(id INTEGER PRIMARY KEY,text TEXT,pinyin TEXT); CREATE INDEX personal_pinyin ON personal(pinyin); CREATE TABLE chunks(id INTEGER PRIMARY KEY,hash TEXT,content TEXT); CREATE INDEX chunks_hash ON chunks(hash);").map_err(|error| format!("无法初始化规模探针：{error}"))?;
    let transaction = connection
        .transaction()
        .map_err(|error| format!("无法开始规模探针：{error}"))?;
    {
        let mut statement = transaction
            .prepare("INSERT INTO personal (id,text,pinyin) VALUES (?1,?2,?3)")
            .map_err(|error| error.to_string())?;
        for index in 0..50_000 {
            statement
                .execute(params![
                    index,
                    format!("词{index}"),
                    format!("pin{:05}", index)
                ])
                .map_err(|error| error.to_string())?;
        }
    }
    {
        let mut statement = transaction
            .prepare("INSERT INTO chunks (id,hash,content) VALUES (?1,?2,?3)")
            .map_err(|error| error.to_string())?;
        for index in 0..100_000 {
            statement
                .execute(params![
                    index,
                    format!("hash{:06}", index),
                    format!("knowledge block {index}")
                ])
                .map_err(|error| error.to_string())?;
        }
    }
    transaction.commit().map_err(|error| error.to_string())?;
    let started = Instant::now();
    for index in 0..1_000 {
        let _: String = connection
            .query_row(
                "SELECT text FROM personal WHERE pinyin=?1",
                params![format!("pin{:05}", index * 49)],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
    }
    let personal_ms = started.elapsed().as_secs_f64() * 1000.0 / 1_000.0;
    let started = Instant::now();
    for index in 0..1_000 {
        let _: String = connection
            .query_row(
                "SELECT content FROM chunks WHERE hash=?1",
                params![format!("hash{:06}", index * 99)],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
    }
    let knowledge_ms = started.elapsed().as_secs_f64() * 1000.0 / 1_000.0;
    Ok(ScaleProbeReport {
        personal_entry_count: 50_000,
        knowledge_chunk_count: 100_000,
        personal_query_milliseconds: personal_ms,
        knowledge_query_milliseconds: knowledge_ms,
        passed: personal_ms <= 5.0 && knowledge_ms <= 5.0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::replay_gate::CompositionOutcome;
    use crate::storage::{LexiconSource, ProjectProfileInput};
    use std::{fs, path::PathBuf};
    fn path(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("agent-maintenance-tests-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        root.join(format!("{name}.sqlite3"))
    }
    fn cleanup(path: &PathBuf) {
        let _ = fs::remove_file(path);
        let _ = fs::remove_file(path.with_extension("sqlite3-wal"));
        let _ = fs::remove_file(path.with_extension("sqlite3-shm"));
    }
    fn learned(id: &str) -> LexiconEntryInput {
        LexiconEntryInput {
            id: id.into(),
            text: format!("学习{id}"),
            pinyin: format!("xuexi{id}"),
            aliases: vec![],
            professional_packs: vec![],
            project_domains: vec!["project-a".into()],
            sources: vec![LexiconSource {
                source_kind: "learned".into(),
                source: "local-selection".into(),
                license: "private-user-data".into(),
                confidence_millis: 1000,
            }],
            base_weight: 10,
            fixed: false,
            disabled: false,
            demoted: false,
            updated_at_ms: 1,
        }
    }

    fn cleanup_item(text: &str, pinyin: &str) -> LearningCleanupItem {
        LearningCleanupItem {
            id: format!("item-{text}"),
            text: text.into(),
            pinyin: pinyin.into(),
            source: "rime_userdb".into(),
            domain: None,
            learned_at_ms: None,
            last_hit_at_ms: None,
            factory_top1: None,
            displaced_factory_top1: false,
            attribution: None,
            batch_cleanup_eligible: false,
            friction_event_count: 0,
            abandoned_count: 0,
            deep_page_count: 0,
            backspace_count: 0,
            suspected: false,
            model_audit: None,
        }
    }

    fn audit_listing(name: &str, document: Option<&[u8]>) -> LearningCleanupList {
        let directory = path(name).with_extension("quality");
        let _ = fs::remove_dir_all(&directory);
        if let Some(document) = document {
            fs::create_dir_all(directory.join("model_audit")).unwrap();
            fs::write(
                directory.join("model_audit/silent_errors.v1.json"),
                document,
            )
            .unwrap();
        }
        let listing = learning_cleanup_list_at(
            vec![
                cleanup_item("清茶", "qing cha"),
                cleanup_item("普通词", "pu tong ci"),
            ],
            &[],
            &directory,
        );
        let _ = fs::remove_dir_all(directory);
        listing
    }

    fn assert_no_model_evidence(listing: LearningCleanupList) {
        assert_eq!(listing.items.len(), 2);
        assert!(listing
            .items
            .iter()
            .all(|item| item.model_audit.is_none() && !item.suspected));
        assert!(listing.unmatched_model_audit_pairs.is_empty());
    }

    #[test]
    fn model_audit_missing_file_does_not_affect_learning_list() {
        assert_no_model_evidence(audit_listing("audit-missing", None));
    }

    #[test]
    fn model_audit_bad_json_does_not_affect_learning_list() {
        assert_no_model_evidence(audit_listing("audit-bad-json", Some(b"{broken")));
    }

    #[test]
    fn model_audit_unknown_schema_does_not_affect_learning_list() {
        assert_no_model_evidence(audit_listing(
            "audit-schema",
            Some(br#"{"schema":2,"pairs":[]}"#),
        ));
    }

    #[test]
    fn model_audit_oversized_file_does_not_affect_learning_list() {
        let mut document = br#"{"schema":1,"pairs":[]}"#.to_vec();
        document.resize(MAX_MODEL_AUDIT_BYTES as usize + 1, b' ');
        assert_no_model_evidence(audit_listing("audit-oversized", Some(&document)));
    }

    #[test]
    fn model_audit_accepts_a_valid_file_at_the_size_limit() {
        let mut document = r#"{"schema":1,"pairs":[{"pinyin":"qc","committed":"清茶","suggested":"晴茶","occurrences":4,"flagged":3,"avg_margin":4.5}]}"#.as_bytes().to_vec();
        document.resize(MAX_MODEL_AUDIT_BYTES as usize, b' ');
        let listing = audit_listing("audit-size-limit", Some(&document));
        assert_eq!(listing.items[0].text, "清茶");
        assert_eq!(listing.items[0].model_audit.as_ref().unwrap().flagged, 3);
        assert!(!listing.items[0].batch_cleanup_eligible);
    }

    #[test]
    fn model_audit_invalid_pair_does_not_affect_learning_list() {
        assert_no_model_evidence(audit_listing("audit-invalid-pair", Some(br#"{"schema":1,"pairs":[
            {"pinyin":"qc","committed":"x","suggested":"y","occurrences":1,"flagged":2,"avg_margin":3.5}
        ]}"#)));
    }

    #[test]
    fn model_audit_accepts_full_spelling_and_syllable_prefix_abbreviations() {
        for (reading, typed) in [
            ("ye jiu shi", "yejiushi"),
            ("wo gan jue", "woganj"),
            ("wo gan jue", "wgj"),
            ("xian zai", "xianz"),
            ("ni shuo de", "nishuod"),
            ("yejiushi", "yejiushi"),
            ("  ye  jiu\tshi  ", "yejiushi"),
        ] {
            assert!(
                model_audit_reading_matches(reading, typed),
                "{reading} vs {typed}"
            );
        }
    }

    #[test]
    fn model_audit_rejects_substrings_wrong_syllables_and_empty_readings() {
        for (reading, typed) in [
            ("wo gan jue", "wo"),
            ("wo gan jue", "ganj"),
            ("wo gan jue", "wogank"),
            ("wo gan jue", "woganjuea"),
            ("wo gan jue", "wo-gan-jue"),
            ("wo gan jue", ""),
            ("", "w"),
            ("wo 123", "wo"),
            ("你", "n"),
        ] {
            assert!(
                !model_audit_reading_matches(reading, typed),
                "{reading} vs {typed}"
            );
        }
    }

    fn audit_pair(
        pinyin: &str,
        committed: &str,
        suggested: &str,
        flagged: usize,
    ) -> ModelAuditPair {
        ModelAuditPair {
            pinyin: pinyin.into(),
            committed: committed.into(),
            suggested: suggested.into(),
            flagged,
            occurrences: flagged + 1,
            avg_margin: 4.5,
        }
    }

    fn list_with_pairs(
        name: &str,
        items: Vec<LearningCleanupItem>,
        pairs: Vec<ModelAuditPair>,
    ) -> LearningCleanupList {
        let directory = path(name).with_extension("quality");
        fs::create_dir_all(directory.join("model_audit")).unwrap();
        // The producer uses snake_case avg_margin; examples are synthetic and
        // must never be returned to the frontend.
        let mut pairs = serde_json::to_value(pairs).unwrap();
        for pair in pairs.as_array_mut().unwrap() {
            let pair = pair.as_object_mut().unwrap();
            let margin = pair.remove("avgMargin").unwrap();
            pair.insert("avg_margin".into(), margin);
            pair.insert(
                "examples".into(),
                serde_json::json!(["synthetic context only"]),
            );
        }
        let document = serde_json::json!({"schema":1,"model":"synthetic","pairs":pairs});
        fs::write(
            directory.join("model_audit/silent_errors.v1.json"),
            document.to_string(),
        )
        .unwrap();
        let listing = learning_cleanup_list_at(items, &[], &directory);
        fs::remove_dir_all(directory).unwrap();
        listing
    }

    #[test]
    fn model_audit_marks_suspected_orders_flags_and_never_grants_batch_eligibility() {
        let listing = list_with_pairs(
            "audit-ranking",
            vec![
                cleanup_item("清茶", "qing cha"),
                cleanup_item("名天", "ming tian"),
                cleanup_item("普通词", "pu tong ci"),
            ],
            vec![
                audit_pair("qingcha", "清茶", "晴茶", 3),
                audit_pair("mingt", "名天", "明天", 8),
            ],
        );
        assert_eq!(
            listing
                .items
                .iter()
                .map(|item| item.text.as_str())
                .collect::<Vec<_>>(),
            ["名天", "清茶", "普通词"]
        );
        for item in &listing.items[..2] {
            assert!(item.suspected);
            assert!(!item.batch_cleanup_eligible);
            assert!(!item.displaced_factory_top1);
            assert!(item.model_audit.is_some());
        }
        assert!(listing.unmatched_model_audit_pairs.is_empty());
        let json = serde_json::to_value(&listing).unwrap();
        assert_eq!(json["items"][0]["modelAudit"]["avgMargin"], 4.5);
        assert!(json["items"][2].get("modelAudit").is_none());
        assert!(!json.to_string().contains("synthetic context"));
    }

    #[cfg(feature = "personal-seed")]
    #[test]
    fn model_audit_preserves_confirmed_pollution_eligibility_and_priority() {
        let listing = list_with_pairs(
            "audit-confirmed",
            vec![
                cleanup_item("只得", "zhi de"),
                cleanup_item("清茶", "qing cha"),
            ],
            vec![
                audit_pair("zhide", "只得", "值得", 2),
                audit_pair("qc", "清茶", "晴茶", 20),
            ],
        );
        assert_eq!(listing.items[0].text, "只得");
        assert!(listing.items[0].batch_cleanup_eligible);
        assert!(listing.items[0].model_audit.is_some());
        assert!(!listing.items[1].batch_cleanup_eligible);
    }

    #[test]
    fn model_audit_matches_all_sources_keeps_strongest_pair_and_reports_only_unmatched() {
        let mut personal = cleanup_item("清茶", "qing cha");
        personal.source = "sqlite_learned".into();
        personal.id = "personal".into();
        let listing = list_with_pairs(
            "audit-unmatched",
            vec![personal, cleanup_item("清茶", "qing cha")],
            vec![
                audit_pair("qingcha", "清茶", "晴茶", 3),
                audit_pair("qc", "清茶", "晴茶", 5),
                audit_pair("mingt", "清茶", "明天", 2),
                audit_pair("qc", "晴茶", "清茶", 2),
            ],
        );
        assert!(listing
            .items
            .iter()
            .all(|item| item.model_audit.as_ref().unwrap().flagged == 5));
        assert_eq!(listing.unmatched_model_audit_pairs.len(), 2);
        assert_eq!(listing.unmatched_model_audit_pairs[0].pinyin, "mingt");
        assert_eq!(listing.unmatched_model_audit_pairs[1].committed, "晴茶");
        let json = serde_json::to_value(listing).unwrap();
        assert_eq!(json["unmatchedModelAuditPairs"][0]["avgMargin"], 4.5);
        assert!(!json.to_string().contains("examples"));
    }

    fn quality_event(
        id: &str,
        pinyin: &str,
        candidates: &[&str],
        selected_text: &str,
        outcome: CompositionOutcome,
        recorded_at_ms: i64,
        max_page: usize,
        backspaces: usize,
    ) -> RecordedQualityEvent {
        let selected_index = candidates
            .iter()
            .position(|candidate| *candidate == selected_text)
            .unwrap_or(0);
        RecordedQualityEvent {
            id: id.into(),
            recorded_at_ms,
            pinyin: pinyin.into(),
            candidates: candidates
                .iter()
                .map(|candidate| (*candidate).into())
                .collect(),
            candidate_comments: Vec::new(),
            selected_index,
            selected_text: selected_text.into(),
            selected_candidate_comment: None,
            app_hash: "a".repeat(64),
            preceding_text: String::new(),
            fuzzy_mask: 0,
            typo_correction_enabled: false,
            active_project: None,
            active_packs: Vec::new(),
            secondary_enabled: true,
            max_page,
            outcome,
            duration_ms: 100,
            backspaces,
            redo_after_commit: false,
            candidate_count: candidates.len(),
            context_model: None,
        }
    }

    #[cfg(not(feature = "personal-seed"))]
    #[test]
    fn public_cleanup_uses_local_events_without_frozen_owner_evidence() {
        assert!(frozen_learning_cleanup_cases().is_empty());
        let events = vec![quality_event(
            "synthetic",
            "qingcha",
            &["清茶"],
            "",
            CompositionOutcome::Abandoned,
            300,
            1,
            2,
        )];
        let ranked =
            rank_learning_cleanup_items(vec![cleanup_item("清茶", "qingcha")], &events);
        let item = &ranked[0];
        assert!(item.suspected);
        assert_eq!(item.friction_event_count, 1);
        assert_eq!(item.factory_top1, None);
        assert_eq!(item.attribution, None);
        assert!(!item.displaced_factory_top1);
        assert!(!item.batch_cleanup_eligible);
    }

    #[cfg(feature = "personal-seed")]
    #[test]
    fn round5_cleanup_evidence_prioritizes_never_selected_owner_displacement() {
        let events = vec![
            quality_event(
                "hit",
                "zhide",
                &["只得", "值得"],
                "值得",
                CompositionOutcome::CommittedCandidate,
                200,
                0,
                0,
            ),
            quality_event(
                "friction",
                "zhide",
                &["只得", "值得"],
                "",
                CompositionOutcome::Abandoned,
                300,
                2,
                3,
            ),
        ];
        let ranked = rank_learning_cleanup_items(
            vec![
                cleanup_item("值得", "zhi de"),
                cleanup_item("只得", "zhi de"),
                cleanup_item("普通词", "putongci"),
            ],
            &events,
        );

        assert_eq!(ranked[0].text, "只得");
        assert_eq!(ranked[0].factory_top1.as_deref(), Some("值得"));
        assert!(ranked[0].displaced_factory_top1);
        assert_eq!(ranked[0].attribution.as_deref(), Some("learned-pollution"));
        assert!(ranked[0].batch_cleanup_eligible);
        assert!(ranked[0].suspected);
        assert_eq!(ranked[0].last_hit_at_ms, None);
        assert_eq!(ranked[0].friction_event_count, 1);
        assert_eq!(ranked[0].abandoned_count, 1);
        assert_eq!(ranked[0].deep_page_count, 1);
        assert_eq!(ranked[0].backspace_count, 3);
        assert!(!ranked[2].suspected);
    }

    #[cfg(feature = "personal-seed")]
    #[test]
    fn cleanup_evidence_keeps_round1_context_rows_out_of_the_round5_group() {
        let ranked = rank_learning_cleanup_items(
            vec![
                cleanup_item("圆", "yuan"),
                cleanup_item("嘛", "ma"),
                cleanup_item("项目", "xiangm"),
            ],
            &[],
        );
        let yuan = ranked.iter().find(|item| item.text == "圆").unwrap();
        assert_eq!(yuan.factory_top1.as_deref(), Some("元"));
        assert_eq!(yuan.attribution.as_deref(), Some("learned-pollution"));
        assert!(yuan.displaced_factory_top1);
        assert!(yuan.batch_cleanup_eligible);

        let ma = ranked.iter().find(|item| item.text == "嘛").unwrap();
        assert_eq!(ma.factory_top1.as_deref(), Some("吗"));
        assert_eq!(ma.attribution.as_deref(), Some("needs-context"));
        assert!(!ma.displaced_factory_top1);
        assert!(!ma.batch_cleanup_eligible);

        let project = ranked.iter().find(|item| item.text == "项目").unwrap();
        assert_eq!(project.factory_top1.as_deref(), Some("项目"));
        assert_eq!(project.attribution.as_deref(), Some("abbrev-competition"));
        assert!(!project.batch_cleanup_eligible);
    }

    #[test]
    fn cleanup_preview_delete_and_undo_are_transactional_and_context_free() {
        let p = path("learning");
        cleanup(&p);
        let storage = AppStorage::open(p.clone()).unwrap();
        storage.create_lexicon_entry(learned("one")).unwrap();
        let preview = storage
            .preview_learning_cleanup(vec![], Some("project-a".into()))
            .unwrap();
        assert_eq!(preview.entry_count, 1);
        assert_eq!(preview.texts, ["学习one"]);
        let result = storage
            .apply_learning_cleanup_with_userdb(preview.entry_ids, None, Vec::new(), 2)
            .unwrap();
        assert!(storage.list_lexicon_entries().unwrap().is_empty());
        assert_eq!(
            storage.undo_learning_cleanup(&result.batch_id, 3).unwrap(),
            1
        );
        assert_eq!(storage.list_lexicon_entries().unwrap().len(), 1);
        drop(storage);
        cleanup(&p);
    }

    #[test]
    fn mixed_cleanup_batch_keeps_userdb_snapshot_and_sqlite_undo_together() {
        let p = path("mixed-learning");
        cleanup(&p);
        let storage = AppStorage::open(p.clone()).unwrap();
        storage.create_lexicon_entry(learned("mixed")).unwrap();
        let userdb = UserDbBackupItem {
            id: "userdb:fixture".into(),
            text: "所义".into(),
            pinyin: "suo yi".into(),
            entry_line:
                "# Rime user dictionary\n#@/db_name\tpinyin_simp\nsuo yi \t所义\tc=3 d=2 t=7\n"
                    .into(),
        };
        let result = storage
            .apply_learning_cleanup_with_userdb(
                vec!["mixed".into()],
                None,
                vec![userdb.clone()],
                20,
            )
            .unwrap();
        assert_eq!(result.deleted_count, 2);
        assert_eq!(
            storage
                .learning_cleanup_userdb_entries(&result.batch_id)
                .unwrap(),
            [userdb]
        );
        assert!(storage.list_lexicon_entries().unwrap().is_empty());
        assert_eq!(
            storage.undo_learning_cleanup(&result.batch_id, 21).unwrap(),
            1
        );
        assert_eq!(storage.list_lexicon_entries().unwrap().len(), 1);
        assert!(storage
            .learning_cleanup_userdb_entries(&result.batch_id)
            .is_err());
        drop(storage);
        cleanup(&p);
    }

    #[test]
    fn secret_free_backup_restores_fresh_database_and_bad_restore_is_non_destructive() {
        let a = path("backup-a");
        let b = path("backup-b");
        cleanup(&a);
        cleanup(&b);
        let source = AppStorage::open(a.clone()).unwrap();
        source.create_lexicon_entry(learned("backup")).unwrap();
        source
            .save_project_profile(ProjectProfileInput {
                id: "project".into(),
                name: "Project".into(),
                project_domain: "project-a".into(),
                professional_packs: vec![],
                agent_context: "local only".into(),
                updated_at_ms: 2,
            })
            .unwrap();
        let backup_secret = "synthetic-backup-secret-never-export";
        source
            .lock_connection()
            .unwrap()
            .execute(
                "INSERT INTO settings(key,value) VALUES('providerSettings',?1)
                 ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                params![format!(r#"{{"apiKey":"{backup_secret}"}}"#)],
            )
            .unwrap();
        let document = source.export_maintenance_backup(3).unwrap();
        println!("BACKUP-REVERSE document={document}");
        assert!(!document.contains("apiKey"));
        assert!(!document.contains("providerSettings"));
        assert!(!document.contains(backup_secret));
        let target = AppStorage::open(b.clone()).unwrap();
        let preview = target.preview_maintenance_backup(&document).unwrap();
        assert_eq!(preview.lexicon_count, 1);
        target.restore_maintenance_backup(&document).unwrap();
        assert_eq!(target.list_lexicon_entries().unwrap().len(), 1);
        let before = target.list_lexicon_entries().unwrap();
        assert!(target
            .restore_maintenance_backup("{\"formatVersion\":99}")
            .is_err());
        assert_eq!(target.list_lexicon_entries().unwrap(), before);
        drop(source);
        drop(target);
        cleanup(&a);
        cleanup(&b);
    }

    #[test]
    fn integrity_and_isolated_scale_contracts_pass() {
        let p = path("integrity");
        cleanup(&p);
        let storage = AppStorage::open(p.clone()).unwrap();
        let report = storage.integrity_report().unwrap();
        assert_eq!(report.quick_check, "ok");
        assert_eq!(report.foreign_key_errors, 0);
        assert_eq!(report.schema_version, 14);
        drop(storage);
        cleanup(&p);
        let scale = run_scale_probe().unwrap();
        println!(
            "scale_probe personal={} personal_ms={:.6} knowledge={} knowledge_ms={:.6}",
            scale.personal_entry_count,
            scale.personal_query_milliseconds,
            scale.knowledge_chunk_count,
            scale.knowledge_query_milliseconds
        );
        assert_eq!(scale.personal_entry_count, 50_000);
        assert_eq!(scale.knowledge_chunk_count, 100_000);
        assert!(scale.passed);
    }
}
