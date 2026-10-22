use crate::snapshot::{LexiconSnapshotEntry, SnapshotMeta, SnapshotStore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::Command,
};

pub const REPLAY_BATCH_ID: &str = "v0.4";
pub const REPLAY_SOURCE_KIND: &str = "owner-quality-recording-v1";
/// Version of the frozen golden set, the active pointer, the used marker and the
/// override audit. Owner corpora stay on this version; it is never bumped to make
/// a helper report parse.
const GOLDEN_SCHEMA_VERSION: u64 = 1;
/// Version of the report emitted by `AgentIMEPersonalReplayHarness`. It is an
/// independent contract from the golden schema: the helper gained provenance
/// binding and moved to 2 while the corpus stayed at 1.
const REPORT_SCHEMA_VERSION: u64 = 2;
const MAXIMUM_GOLDEN_BYTES: usize = 32 * 1024 * 1024;
const MAXIMUM_REPORT_BYTES: usize = 8 * 1024 * 1024;
const MAXIMUM_AUDIT_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CompositionOutcome {
    #[default]
    CommittedCandidate,
    CommittedRaw,
    Abandoned,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextModelOutcome {
    Disabled,
    Unavailable,
    NoGroup,
    Timeout,
    Suspended,
    Error,
    Unchanged,
    Reordered,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextModelObservation {
    pub enabled: bool,
    pub outcome: ContextModelOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_order: Option<Vec<usize>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_size: Option<usize>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordedQualityEvent {
    pub id: String,
    pub recorded_at_ms: i64,
    pub pinyin: String,
    pub candidates: Vec<String>,
    #[serde(default)]
    pub candidate_comments: Vec<String>,
    pub selected_index: usize,
    pub selected_text: String,
    #[serde(default)]
    pub selected_candidate_comment: Option<String>,
    pub app_hash: String,
    #[serde(default)]
    pub preceding_text: String,
    #[serde(default)]
    pub fuzzy_mask: u32,
    #[serde(default)]
    pub typo_correction_enabled: bool,
    #[serde(default)]
    pub active_project: Option<String>,
    #[serde(default)]
    pub active_packs: Vec<String>,
    #[serde(default = "default_secondary_enabled")]
    pub secondary_enabled: bool,
    #[serde(default)]
    pub max_page: usize,
    #[serde(default)]
    pub outcome: CompositionOutcome,
    #[serde(default)]
    pub duration_ms: u64,
    #[serde(default)]
    pub backspaces: usize,
    #[serde(default)]
    pub redo_after_commit: bool,
    #[serde(default)]
    pub candidate_count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_model: Option<ContextModelObservation>,
}

fn default_secondary_enabled() -> bool {
    true
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct FrozenGoldenSet {
    schema_version: u64,
    source_kind: String,
    batch_id: String,
    frozen_at_ms: i64,
    source_events_checksum: String,
    cases: Vec<FrozenGoldenCase>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct FrozenGoldenCase {
    id: String,
    recorded_at_ms: i64,
    pinyin: String,
    baseline_candidates: Vec<String>,
    selected_index: usize,
    selected_text: String,
    app_hash: String,
    #[serde(default)]
    preceding_text: String,
    fuzzy_mask: u32,
    typo_correction_enabled: bool,
    active_project: Option<String>,
    active_packs: Vec<String>,
    secondary_enabled: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ActiveGoldenPointer {
    schema_version: u64,
    batch_id: String,
    checksum: String,
    file_name: String,
    sample_count: usize,
    source_kind: String,
    frozen_at_ms: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct UsedGoldenMarker {
    schema_version: u64,
    batch_id: String,
    golden_checksum: String,
    report_checksum: String,
    snapshot_version: u64,
    used_at_ms: i64,
    override_used: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FrozenGoldenMeta {
    pub batch_id: String,
    pub checksum: String,
    pub sample_count: usize,
    pub source_kind: String,
    pub frozen_at_ms: i64,
    #[serde(skip)]
    pub path: PathBuf,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfidenceInterval {
    pub lower: f64,
    pub upper: f64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplayThresholds {
    pub candidate_top1_minimum: f64,
    pub first_choice_regression_maximum: f64,
    pub latency_p95_maximum_milliseconds: f64,
    pub latency_p99_maximum_milliseconds: f64,
    pub candidate_jitter_maximum_cases: usize,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplayDifference {
    pub case_id: String,
    pub baseline_rank: Option<usize>,
    pub candidate_rank: Option<usize>,
    pub top1_changed: bool,
    pub top5_changed: bool,
    pub jitter: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplayGateReport {
    pub schema_version: u64,
    pub status: String,
    pub source_kind: String,
    pub artifact_checksum: String,
    pub product_commit: String,
    pub product_inputs_sha256: String,
    pub user_db_checksum: String,
    pub generated_at: String,
    pub redaction_state: String,
    pub golden_checksum: String,
    pub candidate_checksum: String,
    pub sample_count: usize,
    pub repetitions: usize,
    pub baseline_top1_hits: usize,
    pub baseline_top5_hits: usize,
    pub candidate_top1_hits: usize,
    pub candidate_top5_hits: usize,
    pub baseline_top1_cases: usize,
    pub first_choice_regressions: usize,
    pub jitter_cases: usize,
    pub latency_samples: usize,
    pub baseline_top1_rate: f64,
    pub baseline_top5_rate: f64,
    pub candidate_top1_rate: f64,
    pub candidate_top5_rate: f64,
    pub first_choice_regression_rate: f64,
    pub candidate_top1_confidence: ConfidenceInterval,
    pub candidate_top5_confidence: ConfidenceInterval,
    pub first_choice_regression_confidence: ConfidenceInterval,
    pub latency_p95_milliseconds: f64,
    pub latency_p99_milliseconds: f64,
    pub thresholds: ReplayThresholds,
    pub confidence_sufficient: bool,
    pub measurement_rule: String,
    pub confidence_rule: String,
    pub privacy_rule: String,
    pub differences: Vec<ReplayDifference>,
    #[serde(default)]
    pub report_checksum: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct OverrideAuditPayload {
    schema_version: u64,
    recorded_at_ms: i64,
    batch_id: String,
    snapshot_version: u64,
    golden_checksum: String,
    candidate_checksum: String,
    report_checksum: String,
    gate_status: String,
    reason: String,
    previous_entry_hash: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct OverrideAuditEntry {
    #[serde(flatten)]
    payload: OverrideAuditPayload,
    entry_hash: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct EmptyGoldenRefreezeAudit {
    schema_version: u64,
    batch_id: String,
    recorded_at_ms: i64,
    reason: String,
    previous_checksum: String,
    previous_sample_count: usize,
    new_checksum: String,
    new_file_name: String,
    new_sample_count: usize,
    new_frozen_at_ms: i64,
    source_events_checksum: String,
}

pub struct ReplayRunInput<'a, T: Serialize> {
    pub quality_root: &'a Path,
    pub helper: &'a Path,
    pub rime_resources: &'a Path,
    /// Live user Rime directory. It is never handed to the helper directly: the
    /// gate copies it to a private snapshot first, so a report cannot be
    /// invalidated by the owner typing while the replay runs.
    pub user_rime: &'a Path,
    pub artifact_checksum: &'a str,
    pub product_commit: &'a str,
    pub product_inputs_sha256: &'a str,
    pub lexicon_entries: &'a T,
    pub now_ms: i64,
}

fn build_frozen_golden(
    events: &[RecordedQualityEvent],
    now_ms: i64,
) -> Result<(FrozenGoldenSet, Vec<u8>, ActiveGoldenPointer), String> {
    let mut ordered = events.to_vec();
    ordered.sort_by(|left, right| {
        (left.recorded_at_ms, &left.id).cmp(&(right.recorded_at_ms, &right.id))
    });
    let mut ids = HashSet::new();
    let mut cases = Vec::with_capacity(ordered.len());
    for event in &ordered {
        validate_event(event)?;
        if !ids.insert(event.id.clone()) {
            return Err("质量记录包含重复事件 ID，不能冻结 golden set。".into());
        }
        if event.outcome == CompositionOutcome::Abandoned {
            continue;
        }
        let canonical =
            serde_json::to_vec(event).map_err(|error| format!("无法规范化质量记录：{error}"))?;
        cases.push(FrozenGoldenCase {
            id: sha256(&canonical),
            recorded_at_ms: event.recorded_at_ms,
            pinyin: event.pinyin.clone(),
            baseline_candidates: event.candidates.clone(),
            selected_index: event.selected_index,
            selected_text: event.selected_text.clone(),
            app_hash: event.app_hash.clone(),
            preceding_text: event.preceding_text.clone(),
            fuzzy_mask: event.fuzzy_mask,
            typo_correction_enabled: event.typo_correction_enabled,
            active_project: event.active_project.clone(),
            active_packs: event.active_packs.clone(),
            secondary_enabled: event.secondary_enabled,
        });
    }
    let source_bytes =
        serde_json::to_vec(&ordered).map_err(|error| format!("无法规范化质量记录集合：{error}"))?;
    let frozen = FrozenGoldenSet {
        schema_version: GOLDEN_SCHEMA_VERSION,
        source_kind: REPLAY_SOURCE_KIND.into(),
        batch_id: REPLAY_BATCH_ID.into(),
        frozen_at_ms: now_ms,
        source_events_checksum: sha256(&source_bytes),
        cases,
    };
    let bytes = canonical_pretty_json(&frozen)?;
    if bytes.len() > MAXIMUM_GOLDEN_BYTES {
        return Err("冻结 golden set 超过 32 MiB 上限。".into());
    }
    let checksum = sha256(&bytes);
    let pointer = ActiveGoldenPointer {
        schema_version: GOLDEN_SCHEMA_VERSION,
        batch_id: REPLAY_BATCH_ID.into(),
        checksum: checksum.clone(),
        file_name: format!("golden-{REPLAY_BATCH_ID}-{checksum}.json"),
        sample_count: frozen.cases.len(),
        source_kind: REPLAY_SOURCE_KIND.into(),
        frozen_at_ms: now_ms,
    };
    Ok((frozen, bytes, pointer))
}

pub fn freeze_golden_set(
    quality_root: &Path,
    events: &[RecordedQualityEvent],
    now_ms: i64,
) -> Result<FrozenGoldenMeta, String> {
    let replay_root = prepare_replay_root(quality_root)?;
    let pointer_path = replay_root.join(format!("active-{REPLAY_BATCH_ID}.json"));
    let used_path = replay_root.join(format!("used-{REPLAY_BATCH_ID}.json"));
    let empty_refreeze_audit = replay_root.join(format!("empty-refreeze-{REPLAY_BATCH_ID}.json"));
    if empty_refreeze_audit.exists() && pointer_path.exists() {
        return load_active_golden(&replay_root, &pointer_path);
    }
    if used_path.exists() && pointer_path.exists() {
        return load_active_golden(&replay_root, &pointer_path);
    }

    let (frozen, bytes, pointer) = build_frozen_golden(events, now_ms)?;
    if pointer_path.exists() {
        let active = load_active_golden(&replay_root, &pointer_path)?;
        let active_bytes =
            read_bounded_regular(&active.path, MAXIMUM_GOLDEN_BYTES, "冻结 golden set")?
                .ok_or_else(|| "冻结 golden set 不存在。".to_string())?;
        let active_set: FrozenGoldenSet = serde_json::from_slice(&active_bytes)
            .map_err(|_| "冻结 golden set 格式无效。".to_string())?;
        if active_set.source_events_checksum == frozen.source_events_checksum {
            return Ok(active);
        }
    }
    atomic_private_write(&replay_root, &pointer.file_name, &bytes)?;
    atomic_private_replace(
        &replay_root,
        &format!("active-{REPLAY_BATCH_ID}.json"),
        &canonical_pretty_json(&pointer)?,
    )?;
    load_active_golden(&replay_root, &pointer_path)
}

/// Replaces only the never-used empty v0.4 golden set. The immutable audit is
/// written before the active pointer, so a retry can complete an interrupted
/// pointer switch without permitting a second replacement.
pub fn refreeze_empty_golden_set_once(
    quality_root: &Path,
    events: &[RecordedQualityEvent],
    now_ms: i64,
    reason: &str,
) -> Result<FrozenGoldenMeta, String> {
    let reason = reason.trim();
    if reason.is_empty() || reason.len() > 500 || reason.chars().any(char::is_control) {
        return Err("空 golden set 解冻必须提供 1–500 字且不含控制字符的 owner 批准理由。".into());
    }
    let replay_root = prepare_replay_root(quality_root)?;
    let pointer_path = replay_root.join(format!("active-{REPLAY_BATCH_ID}.json"));
    let used_path = replay_root.join(format!("used-{REPLAY_BATCH_ID}.json"));
    let audit_name = format!("empty-refreeze-{REPLAY_BATCH_ID}.json");
    let audit_path = replay_root.join(&audit_name);
    let active = load_active_golden(&replay_root, &pointer_path)?;

    if let Some(audit_bytes) = read_bounded_regular(&audit_path, 16_384, "空 golden set 解冻审计")?
    {
        let audit: EmptyGoldenRefreezeAudit = serde_json::from_slice(&audit_bytes)
            .map_err(|_| "空 golden set 解冻审计无效。".to_string())?;
        if audit.schema_version != GOLDEN_SCHEMA_VERSION
            || audit.batch_id != REPLAY_BATCH_ID
            || !is_checksum(&audit.previous_checksum)
            || !is_checksum(&audit.new_checksum)
            || !is_checksum(&audit.source_events_checksum)
            || !safe_leaf_name(&audit.new_file_name)
            || audit.previous_sample_count != 0
            || audit.new_sample_count == 0
        {
            return Err("空 golden set 解冻审计未通过校验。".into());
        }
        if active.checksum == audit.new_checksum {
            return Ok(active);
        }
        if active.checksum != audit.previous_checksum || active.sample_count != 0 {
            return Err("active golden set 与一次性解冻审计不一致。".into());
        }
        if used_path.exists() {
            return Err("空 golden set 已绑定验收结果，禁止解冻。".into());
        }
        let golden_path = replay_root.join(&audit.new_file_name);
        let golden_bytes =
            read_bounded_regular(&golden_path, MAXIMUM_GOLDEN_BYTES, "一次性解冻 golden set")?
                .ok_or_else(|| "一次性解冻 golden set 不存在。".to_string())?;
        if sha256(&golden_bytes) != audit.new_checksum {
            return Err("一次性解冻 golden set checksum 不匹配。".into());
        }
        let pointer = ActiveGoldenPointer {
            schema_version: GOLDEN_SCHEMA_VERSION,
            batch_id: REPLAY_BATCH_ID.into(),
            checksum: audit.new_checksum,
            file_name: audit.new_file_name,
            sample_count: audit.new_sample_count,
            source_kind: REPLAY_SOURCE_KIND.into(),
            frozen_at_ms: audit.new_frozen_at_ms,
        };
        atomic_private_replace(
            &replay_root,
            &format!("active-{REPLAY_BATCH_ID}.json"),
            &canonical_pretty_json(&pointer)?,
        )?;
        return load_active_golden(&replay_root, &pointer_path);
    }

    if used_path.exists() {
        return Err("空 golden set 已绑定验收结果，禁止解冻。".into());
    }
    if active.sample_count != 0 {
        return Err("仅允许一次性替换从未用于验收的 0 样本 golden set。".into());
    }
    let (frozen, bytes, pointer) = build_frozen_golden(events, now_ms)?;
    if pointer.sample_count == 0 {
        return Err("没有真实质量记录，不能解冻为空集。".into());
    }
    atomic_private_write(&replay_root, &pointer.file_name, &bytes)?;
    let audit = EmptyGoldenRefreezeAudit {
        schema_version: GOLDEN_SCHEMA_VERSION,
        batch_id: REPLAY_BATCH_ID.into(),
        recorded_at_ms: now_ms,
        reason: reason.into(),
        previous_checksum: active.checksum,
        previous_sample_count: active.sample_count,
        new_checksum: pointer.checksum.clone(),
        new_file_name: pointer.file_name.clone(),
        new_sample_count: pointer.sample_count,
        new_frozen_at_ms: pointer.frozen_at_ms,
        source_events_checksum: frozen.source_events_checksum,
    };
    atomic_private_write(&replay_root, &audit_name, &canonical_pretty_json(&audit)?)?;
    atomic_private_replace(
        &replay_root,
        &format!("active-{REPLAY_BATCH_ID}.json"),
        &canonical_pretty_json(&pointer)?,
    )?;
    load_active_golden(&replay_root, &pointer_path)
}

pub fn mark_golden_used(
    quality_root: &Path,
    report: &ReplayGateReport,
    snapshot_version: u64,
    now_ms: i64,
    override_used: bool,
) -> Result<(), String> {
    if report.status != "passed" && !override_used {
        return Err("只有通过的 Replay Gate 才能冻结为批次验收 golden set。".into());
    }
    let replay_root = prepare_replay_root(quality_root)?;
    let active = load_active_golden(
        &replay_root,
        &replay_root.join(format!("active-{REPLAY_BATCH_ID}.json")),
    )?;
    if active.checksum != report.golden_checksum {
        return Err("Replay Gate 报告不对应当前 active golden set。".into());
    }
    let marker_path = replay_root.join(format!("used-{REPLAY_BATCH_ID}.json"));
    if let Some(bytes) = read_bounded_regular(&marker_path, 4_096, "used golden 标记")? {
        let existing: UsedGoldenMarker =
            serde_json::from_slice(&bytes).map_err(|_| "used golden 标记无效。".to_string())?;
        return if existing.schema_version == GOLDEN_SCHEMA_VERSION
            && existing.batch_id == REPLAY_BATCH_ID
            && existing.golden_checksum == report.golden_checksum
        {
            Ok(())
        } else {
            Err("批次已绑定另一份 golden set，不能改动。".into())
        };
    }
    let marker = UsedGoldenMarker {
        schema_version: GOLDEN_SCHEMA_VERSION,
        batch_id: REPLAY_BATCH_ID.into(),
        golden_checksum: report.golden_checksum.clone(),
        report_checksum: report.report_checksum.clone(),
        snapshot_version,
        used_at_ms: now_ms,
        override_used,
    };
    atomic_private_write(
        &replay_root,
        &format!("used-{REPLAY_BATCH_ID}.json"),
        &canonical_pretty_json(&marker)?,
    )
}

pub fn run_replay_gate<T: Serialize>(
    input: ReplayRunInput<'_, T>,
) -> Result<ReplayGateReport, String> {
    let events = read_quality_events(input.quality_root)?;
    let golden = freeze_golden_set(input.quality_root, &events, input.now_ms)?;
    let replay_root = prepare_replay_root(input.quality_root)?;
    let candidate_bytes = serde_json::to_vec(input.lexicon_entries)
        .map_err(|error| format!("无法编码 Replay Gate 候选词库：{error}"))?;
    let candidate_checksum = sha256(&candidate_bytes);
    let candidate_name = format!("candidate-{candidate_checksum}.json");
    atomic_private_write(&replay_root, &candidate_name, &candidate_bytes)?;
    let reports = replay_root.join("Reports");
    prepare_private_directory(&reports)?;
    let report_name = format!(
        "report-{}-{}-{}.json",
        input.now_ms, golden.checksum, candidate_checksum
    );
    let report_path = reports.join(&report_name);

    if !is_checksum(input.artifact_checksum)
        || !is_commit(input.product_commit)
        || !is_checksum(input.product_inputs_sha256)
    {
        return Err("Replay Gate 制品 provenance 无效，快照发布已阻止。".into());
    }
    let (user_db_snapshot, user_db_checksum) = snapshot_user_rime(&replay_root, input.user_rime)?;

    let output = Command::new(input.helper)
        .arg(input.rime_resources)
        .arg(&user_db_snapshot)
        .arg(&golden.path)
        .arg(replay_root.join(&candidate_name))
        .arg(&report_path)
        .arg("5")
        .arg(input.artifact_checksum)
        .arg(input.product_commit)
        .arg(input.product_inputs_sha256)
        .arg(&user_db_checksum)
        .output()
        .map_err(|error| format!("无法启动个人语料 Replay Gate：{error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let redacted = stderr.lines().next().unwrap_or("helper failed");
        return Err(format!("个人语料 Replay Gate 执行失败：{redacted}"));
    }
    let report_bytes =
        read_bounded_regular(&report_path, MAXIMUM_REPORT_BYTES, "Replay Gate 报告")?
            .ok_or_else(|| "Replay Gate 未生成报告。".to_string())?;
    let report_checksum = sha256(&report_bytes);
    let mut report: ReplayGateReport = serde_json::from_slice(&report_bytes)
        .map_err(|_| "Replay Gate 报告格式无效。".to_string())?;
    validate_report(
        &report,
        &golden,
        &candidate_checksum,
        &ReportProvenance {
            artifact_checksum: input.artifact_checksum,
            product_commit: input.product_commit,
            product_inputs_sha256: input.product_inputs_sha256,
            user_db_checksum: &user_db_checksum,
        },
    )?;
    report.report_checksum = report_checksum;
    Ok(report)
}

pub fn require_publication_pass(report: &ReplayGateReport) -> Result<(), String> {
    match report.status.as_str() {
        "passed" => Ok(()),
        "insufficient_data" => Err(format!(
            "Replay Gate 数据不足：当前冻结真实样本 {} 条，快照发布已阻止。",
            report.sample_count
        )),
        "failed" => Err(format!(
            "Replay Gate 未达冻结阈值：首选 {:.2}%、首选退化 {:.2}%、抖动 {} 条、P95/P99 {:.3}/{:.3} ms；快照发布已阻止。",
            report.candidate_top1_rate * 100.0,
            report.first_choice_regression_rate * 100.0,
            report.jitter_cases,
            report.latency_p95_milliseconds,
            report.latency_p99_milliseconds
        )),
        _ => Err("Replay Gate 返回未知状态，快照发布已阻止。".into()),
    }
}

pub fn publish_lexicon_after_gate(
    store: &SnapshotStore,
    version: u64,
    entries: Vec<LexiconSnapshotEntry>,
    report: &ReplayGateReport,
) -> Result<SnapshotMeta, String> {
    require_publication_pass(report)?;
    store.publish_lexicon(version, entries)
}

pub fn append_override_audit(
    quality_root: &Path,
    report: &ReplayGateReport,
    snapshot_version: u64,
    reason: &str,
    now_ms: i64,
) -> Result<String, String> {
    let reason = reason.trim();
    if reason.is_empty() || reason.len() > 500 || reason.chars().any(char::is_control) {
        return Err("人工 override 必须提供 1–500 字且不含控制字符的原因。".into());
    }
    let replay_root = prepare_replay_root(quality_root)?;
    let path = replay_root.join("override-audit.jsonl");
    let existing = read_bounded_regular(&path, MAXIMUM_AUDIT_BYTES, "Replay Gate override 审计")?
        .unwrap_or_default();
    let previous_entry_hash = if existing.is_empty() {
        "0".repeat(64)
    } else {
        let existing_text = String::from_utf8(existing.clone())
            .map_err(|_| "Replay Gate override 审计不是 UTF-8。".to_string())?;
        let last = existing_text
            .lines()
            .last()
            .ok_or_else(|| "Replay Gate override 审计为空行。".to_string())?;
        serde_json::from_str::<OverrideAuditEntry>(last)
            .map_err(|_| "Replay Gate override 审计包含无效行。".to_string())?
            .entry_hash
    };
    let payload = OverrideAuditPayload {
        schema_version: GOLDEN_SCHEMA_VERSION,
        recorded_at_ms: now_ms,
        batch_id: REPLAY_BATCH_ID.into(),
        snapshot_version,
        golden_checksum: report.golden_checksum.clone(),
        candidate_checksum: report.candidate_checksum.clone(),
        report_checksum: report.report_checksum.clone(),
        gate_status: report.status.clone(),
        reason: reason.into(),
        previous_entry_hash,
    };
    let entry_hash = sha256(
        &serde_json::to_vec(&payload)
            .map_err(|error| format!("无法编码 override 审计：{error}"))?,
    );
    let entry = OverrideAuditEntry {
        payload,
        entry_hash: entry_hash.clone(),
    };
    let mut line =
        serde_json::to_vec(&entry).map_err(|error| format!("无法编码 override 审计：{error}"))?;
    line.push(b'\n');
    let mut options = OpenOptions::new();
    options.create(true).append(true).mode(0o600);
    let mut file = options
        .open(&path)
        .map_err(|error| format!("无法打开 override 审计：{error}"))?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("无法保护 override 审计：{error}"))?;
    file.write_all(&line)
        .and_then(|_| file.sync_all())
        .map_err(|error| format!("无法持久化 override 审计：{error}"))?;
    Ok(entry_hash)
}

pub fn read_quality_events(quality_root: &Path) -> Result<Vec<RecordedQualityEvent>, String> {
    let path = quality_root.join("events.jsonl");
    let Some(data) = read_bounded_regular(&path, MAXIMUM_GOLDEN_BYTES, "质量记录")? else {
        return Ok(Vec::new());
    };
    let text = std::str::from_utf8(&data).map_err(|_| "质量记录不是有效 UTF-8。".to_string())?;
    text.lines()
        .map(|line| {
            let event: RecordedQualityEvent =
                serde_json::from_str(line).map_err(|_| "质量记录包含无效行。".to_string())?;
            validate_event(&event)?;
            Ok(event)
        })
        .collect()
}

fn validate_event(event: &RecordedQualityEvent) -> Result<(), String> {
    let valid_pinyin = !event.pinyin.is_empty()
        && event.pinyin.len() <= 128
        && event
            .pinyin
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte == b'\'');
    let valid_candidates = (event.outcome == CompositionOutcome::Abandoned
        || !event.candidates.is_empty())
        && event.candidates.len() <= 12
        && event.candidates.iter().all(|candidate| {
            !candidate.trim().is_empty()
                && candidate.len() <= 256
                && !candidate.chars().any(char::is_control)
        });
    let valid_selected = if event.outcome == CompositionOutcome::Abandoned {
        event.selected_text.is_empty()
    } else {
        event.selected_index < event.candidates.len()
            && event.candidates[event.selected_index] == event.selected_text
    };
    let valid_hash = is_checksum(&event.app_hash);
    let valid_context = event.fuzzy_mask <= 0xff
        && event.active_packs.len() <= 64
        && event
            .active_packs
            .iter()
            .all(|pack| !pack.trim().is_empty() && pack.len() <= 256)
        && event
            .active_project
            .as_ref()
            .is_none_or(|project| !project.trim().is_empty() && project.len() <= 256);
    if event.id.trim().is_empty()
        || event.recorded_at_ms < 0
        || !valid_pinyin
        || !valid_candidates
        || !valid_selected
        || !valid_hash
        || !valid_context
        || event.max_page > 10_000
        || (event.candidate_count != 0 && event.candidate_count < event.candidates.len())
    {
        return Err("质量记录未通过 Replay Gate 脱敏/结构校验。".into());
    }
    Ok(())
}

pub(crate) struct ReportProvenance<'a> {
    pub(crate) artifact_checksum: &'a str,
    pub(crate) product_commit: &'a str,
    pub(crate) product_inputs_sha256: &'a str,
    pub(crate) user_db_checksum: &'a str,
}

fn validate_report(
    report: &ReplayGateReport,
    golden: &FrozenGoldenMeta,
    candidate_checksum: &str,
    provenance: &ReportProvenance<'_>,
) -> Result<(), String> {
    // The helper echoes back what it was bound to. A report that measured a
    // different bundle, commit, input set or UserDB snapshot than the one we
    // asked for is not evidence about this candidate, so it is rejected rather
    // than reinterpreted.
    if report.artifact_checksum != provenance.artifact_checksum
        || report.product_commit != provenance.product_commit
        || report.product_inputs_sha256 != provenance.product_inputs_sha256
        || report.user_db_checksum != provenance.user_db_checksum
        || report.redaction_state != "redacted-no-input-or-candidate-text"
    {
        return Err("Replay Gate 报告未绑定本次制品、提交或 UserDB 快照。".into());
    }
    let thresholds_match = (report.thresholds.candidate_top1_minimum - 0.95).abs() < f64::EPSILON
        && (report.thresholds.first_choice_regression_maximum - 0.01).abs() < f64::EPSILON
        && (report.thresholds.latency_p95_maximum_milliseconds - 2.5).abs() < f64::EPSILON
        && (report.thresholds.latency_p99_maximum_milliseconds - 5.0).abs() < f64::EPSILON
        && report.thresholds.candidate_jitter_maximum_cases == 0;
    if report.schema_version != REPORT_SCHEMA_VERSION
        || report.source_kind != REPLAY_SOURCE_KIND
        || report.golden_checksum != golden.checksum
        || report.candidate_checksum != candidate_checksum
        || report.sample_count != golden.sample_count
        || report.repetitions != 5
        || !matches!(
            report.status.as_str(),
            "passed" | "failed" | "insufficient_data"
        )
        || !thresholds_match
        || report
            .differences
            .iter()
            .any(|difference| !is_checksum(&difference.case_id))
    {
        return Err("Replay Gate 报告与冻结输入、阈值或隐私格式不一致。".into());
    }
    Ok(())
}

fn load_active_golden(replay_root: &Path, pointer_path: &Path) -> Result<FrozenGoldenMeta, String> {
    let bytes = read_bounded_regular(pointer_path, 4_096, "active golden 指针")?
        .ok_or_else(|| "active golden 指针不存在。".to_string())?;
    let pointer: ActiveGoldenPointer =
        serde_json::from_slice(&bytes).map_err(|_| "active golden 指针无效。".to_string())?;
    if pointer.schema_version != GOLDEN_SCHEMA_VERSION
        || pointer.batch_id != REPLAY_BATCH_ID
        || pointer.source_kind != REPLAY_SOURCE_KIND
        || !is_checksum(&pointer.checksum)
        || !safe_leaf_name(&pointer.file_name)
    {
        return Err("active golden 指针未通过校验。".into());
    }
    let path = replay_root.join(&pointer.file_name);
    let golden_bytes = read_bounded_regular(&path, MAXIMUM_GOLDEN_BYTES, "冻结 golden set")?
        .ok_or_else(|| "冻结 golden set 不存在。".to_string())?;
    if sha256(&golden_bytes) != pointer.checksum {
        return Err("冻结 golden set checksum 不匹配。".into());
    }
    let golden: FrozenGoldenSet = serde_json::from_slice(&golden_bytes)
        .map_err(|_| "冻结 golden set 格式无效。".to_string())?;
    if golden.schema_version != GOLDEN_SCHEMA_VERSION
        || golden.batch_id != REPLAY_BATCH_ID
        || golden.source_kind != REPLAY_SOURCE_KIND
        || golden.cases.len() != pointer.sample_count
        || golden.frozen_at_ms != pointer.frozen_at_ms
    {
        return Err("冻结 golden set 与 active 指针不一致。".into());
    }
    Ok(FrozenGoldenMeta {
        batch_id: pointer.batch_id,
        checksum: pointer.checksum,
        sample_count: pointer.sample_count,
        source_kind: pointer.source_kind,
        frozen_at_ms: pointer.frozen_at_ms,
        path,
    })
}

fn prepare_replay_root(quality_root: &Path) -> Result<PathBuf, String> {
    prepare_private_directory(quality_root)?;
    let replay_root = quality_root.join("ReplayGate");
    prepare_private_directory(&replay_root)?;
    Ok(replay_root)
}

fn prepare_private_directory(path: &Path) -> Result<(), String> {
    fs::create_dir_all(path).map_err(|error| format!("无法创建本地 Replay Gate 目录：{error}"))?;
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("无法检查本地 Replay Gate 目录：{error}"))?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
        return Err("Replay Gate 路径不是安全的本地目录。".into());
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("无法保护本地 Replay Gate 目录：{error}"))
}

fn atomic_private_write(directory: &Path, name: &str, bytes: &[u8]) -> Result<(), String> {
    if !safe_leaf_name(name) {
        return Err("Replay Gate 文件名无效。".into());
    }
    let destination = directory.join(name);
    if destination.exists() {
        let existing = read_bounded_regular(&destination, bytes.len().max(1), "Replay Gate 文件")?
            .ok_or_else(|| "Replay Gate 文件消失。".to_string())?;
        return if existing == bytes {
            Ok(())
        } else {
            Err("Replay Gate 不允许覆盖同名冻结文件。".into())
        };
    }
    let temporary_name = format!(".{name}.{}-{}.tmp", std::process::id(), randless_nonce());
    let temporary = directory.join(&temporary_name);
    let mut options = OpenOptions::new();
    options.write(true).create_new(true).mode(0o600);
    let mut file = options
        .open(&temporary)
        .map_err(|error| format!("无法暂存 Replay Gate 文件：{error}"))?;
    if let Err(error) = file.write_all(bytes).and_then(|_| file.sync_all()) {
        let _ = fs::remove_file(&temporary);
        return Err(format!("无法持久化 Replay Gate 文件：{error}"));
    }
    if let Err(error) = fs::rename(&temporary, &destination) {
        let _ = fs::remove_file(&temporary);
        return Err(format!("无法冻结 Replay Gate 文件：{error}"));
    }
    fs::set_permissions(&destination, fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("无法保护 Replay Gate 文件：{error}"))
}

fn atomic_private_replace(directory: &Path, name: &str, bytes: &[u8]) -> Result<(), String> {
    if !safe_leaf_name(name) {
        return Err("Replay Gate 文件名无效。".into());
    }
    let temporary_name = format!(".{name}.{}-{}.tmp", std::process::id(), randless_nonce());
    let temporary = directory.join(&temporary_name);
    let mut options = OpenOptions::new();
    options.write(true).create_new(true).mode(0o600);
    let mut file = options
        .open(&temporary)
        .map_err(|error| format!("无法暂存 Replay Gate 指针：{error}"))?;
    if let Err(error) = file.write_all(bytes).and_then(|_| file.sync_all()) {
        let _ = fs::remove_file(&temporary);
        return Err(format!("无法持久化 Replay Gate 指针：{error}"));
    }
    if let Err(error) = fs::rename(&temporary, directory.join(name)) {
        let _ = fs::remove_file(&temporary);
        return Err(format!("无法切换 Replay Gate 指针：{error}"));
    }
    fs::set_permissions(directory.join(name), fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("无法保护 Replay Gate 指针：{error}"))
}

/// Everything the helper must be bound to that can be derived from the shipped
/// input-method bundle alone.
pub struct ArtifactProvenance {
    pub rime_resources: PathBuf,
    pub artifact_checksum: String,
    pub product_commit: String,
    pub product_inputs_sha256: String,
}

/// Derives the bundle provenance from the helper path. Both the Manager and the
/// fixture CLI go through this, so the two callers cannot disagree about what a
/// report is bound to.
pub fn artifact_provenance(helper: &Path) -> Result<ArtifactProvenance, String> {
    let contents = helper
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| "Replay Gate helper bundle 路径无效。".to_string())?;
    let bundle = contents
        .parent()
        .ok_or_else(|| "Replay Gate helper bundle 路径无效。".to_string())?;
    let rime_resources = contents.join("Resources/Rime");
    if !rime_resources.is_dir() {
        return Err("Replay Gate helper 缺少冻结 Rime 资源。".into());
    }
    let plist = read_bounded_regular(
        &contents.join("Info.plist"),
        1024 * 1024,
        "输入法 Info.plist",
    )?
    .ok_or_else(|| "输入法 Info.plist 不存在。".to_string())?;
    let plist =
        String::from_utf8(plist).map_err(|_| "输入法 Info.plist 不是 UTF-8。".to_string())?;
    Ok(ArtifactProvenance {
        rime_resources,
        artifact_checksum: directory_checksum(bundle, "输入法制品")?,
        product_commit: info_plist_string(&plist, "AgentIMEProductCommit")?,
        product_inputs_sha256: info_plist_string(&plist, "AgentIMEProductInputsSHA256")?,
    })
}

/// Reads a top-level `<key>`/`<string>` pair out of a bundle Info.plist. The
/// bundles this crate drives are always written as XML plists by
/// `script/build_native.sh`, so a bounded scan is enough and avoids adding a
/// plist dependency for two fields.
fn info_plist_string(plist: &str, key: &str) -> Result<String, String> {
    let marker = format!("<key>{key}</key>");
    let after = plist
        .split_once(&marker)
        .ok_or_else(|| format!("输入法 Info.plist 缺少 {key}。"))?
        .1;
    let opening = after
        .find("<string>")
        .ok_or_else(|| format!("输入法 Info.plist 的 {key} 不是字符串。"))?;
    let value = &after[opening + "<string>".len()..];
    let closing = value
        .find("</string>")
        .ok_or_else(|| format!("输入法 Info.plist 的 {key} 未闭合。"))?;
    Ok(value[..closing].trim().to_string())
}

/// Byte-for-byte equivalent of `script/artifact_checksum.sh`: every regular file
/// under `root`, listed as `./relative/path`, sorted by raw byte order, rendered
/// as `{sha256}  {path}\n`, and hashed as a whole. Any symlink anywhere in the
/// tree is rejected, exactly as the script does.
pub fn directory_checksum(root: &Path, label: &str) -> Result<String, String> {
    let mut entries = Vec::new();
    collect_regular_files(root, &mut String::from("."), &mut entries, label)?;
    entries.sort();
    let mut manifest = String::new();
    for relative in entries {
        let bytes = fs::read(root.join(relative.trim_start_matches("./")))
            .map_err(|error| format!("无法读取{label}内容：{error}"))?;
        manifest.push_str(&sha256(&bytes));
        manifest.push_str("  ");
        manifest.push_str(&relative);
        manifest.push('\n');
    }
    Ok(sha256(manifest.as_bytes()))
}

fn collect_regular_files(
    directory: &Path,
    prefix: &mut String,
    entries: &mut Vec<String>,
    label: &str,
) -> Result<(), String> {
    let listing =
        fs::read_dir(directory).map_err(|error| format!("无法遍历{label}目录：{error}"))?;
    for entry in listing {
        let entry = entry.map_err(|error| format!("无法遍历{label}目录：{error}"))?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| format!("{label}包含非 UTF-8 文件名。"))?;
        let file_type = entry
            .file_type()
            .map_err(|error| format!("无法检查{label}条目：{error}"))?;
        if file_type.is_symlink() {
            return Err(format!("{label}包含符号链接，拒绝校验。"));
        }
        let previous = prefix.len();
        prefix.push('/');
        prefix.push_str(&name);
        if file_type.is_dir() {
            collect_regular_files(&entry.path(), prefix, entries, label)?;
        } else if file_type.is_file() {
            entries.push(prefix.clone());
        }
        prefix.truncate(previous);
    }
    Ok(())
}

/// Copies the live user Rime directory into a private snapshot and checksums it.
/// The replay must not read a directory librime is still writing to, or the
/// report describes a state that no longer exists.
fn snapshot_user_rime(replay_root: &Path, user_rime: &Path) -> Result<(PathBuf, String), String> {
    let snapshot = replay_root.join("UserDBSnapshot");
    if snapshot.exists() {
        let metadata = fs::symlink_metadata(&snapshot)
            .map_err(|error| format!("无法检查 UserDB 快照目录：{error}"))?;
        if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
            return Err("UserDB 快照路径不是安全的本地目录。".into());
        }
        fs::remove_dir_all(&snapshot)
            .map_err(|error| format!("无法清理上一次 UserDB 快照：{error}"))?;
    }
    prepare_private_directory(&snapshot)?;
    copy_regular_tree(user_rime, &snapshot)?;
    let checksum = directory_checksum(&snapshot, "UserDB 快照")?;
    Ok((snapshot, checksum))
}

fn copy_regular_tree(source: &Path, destination: &Path) -> Result<(), String> {
    let listing =
        fs::read_dir(source).map_err(|error| format!("无法读取用户 Rime 目录：{error}"))?;
    for entry in listing {
        let entry = entry.map_err(|error| format!("无法读取用户 Rime 目录：{error}"))?;
        let file_type = entry
            .file_type()
            .map_err(|error| format!("无法检查用户 Rime 条目：{error}"))?;
        if file_type.is_symlink() {
            return Err("用户 Rime 目录包含符号链接，Replay Gate 已阻止。".into());
        }
        let target = destination.join(entry.file_name());
        if file_type.is_dir() {
            prepare_private_directory(&target)?;
            copy_regular_tree(&entry.path(), &target)?;
        } else if file_type.is_file() {
            fs::copy(entry.path(), &target)
                .map_err(|error| format!("无法快照用户 Rime 文件：{error}"))?;
            fs::set_permissions(&target, fs::Permissions::from_mode(0o600))
                .map_err(|error| format!("无法保护 UserDB 快照文件：{error}"))?;
        }
    }
    Ok(())
}

fn is_commit(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn read_bounded_regular(
    path: &Path,
    maximum: usize,
    label: &str,
) -> Result<Option<Vec<u8>>, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("无法检查{label}：{error}")),
    };
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err(format!("{label}不是安全的普通文件。"));
    }
    if metadata.len() > maximum as u64 {
        return Err(format!("{label}超过大小上限。"));
    }
    fs::read(path)
        .map(Some)
        .map_err(|error| format!("无法读取{label}：{error}"))
}

fn canonical_pretty_json<T: Serialize>(value: &T) -> Result<Vec<u8>, String> {
    let mut bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| format!("无法编码 Replay Gate 文件：{error}"))?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn is_checksum(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn safe_leaf_name(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && !value.contains('/')
        && !value.contains("..")
        && !value.as_bytes().contains(&0)
}

fn randless_nonce() -> u128 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "agent-ime-replay-{name}-{}-{}",
            std::process::id(),
            randless_nonce()
        ))
    }

    fn event(index: usize) -> RecordedQualityEvent {
        RecordedQualityEvent {
            id: format!("event-{index}"),
            recorded_at_ms: 1_000 + index as i64,
            pinyin: "ni".into(),
            candidates: vec!["你".into(), "泥".into()],
            candidate_comments: vec![String::new(), String::new()],
            selected_index: 0,
            selected_text: "你".into(),
            selected_candidate_comment: None,
            app_hash: "a".repeat(64),
            preceding_text: "刚才说到这里".into(),
            fuzzy_mask: 7,
            typo_correction_enabled: true,
            active_project: Some("local-project".into()),
            active_packs: vec!["input-method".into()],
            secondary_enabled: true,
            max_page: 0,
            outcome: CompositionOutcome::CommittedCandidate,
            duration_ms: 10,
            backspaces: 0,
            redo_after_commit: false,
            candidate_count: 2,
            context_model: None,
        }
    }

    #[test]
    fn legacy_quality_lines_default_new_composition_metadata_without_text_schema_changes() {
        let legacy = serde_json::json!({
            "id": "legacy-1",
            "recordedAtMs": 1000,
            "pinyin": "ni",
            "candidates": ["你"],
            "candidateComments": [""],
            "selectedIndex": 0,
            "selectedText": "你",
            "selectedCandidateComment": null,
            "appHash": "a".repeat(64),
            "precedingText": "",
            "fuzzyMask": 0,
            "typoCorrectionEnabled": false,
            "activeProject": null,
            "activePacks": [],
            "secondaryEnabled": true
        });
        let decoded: RecordedQualityEvent = serde_json::from_value(legacy).unwrap();
        assert_eq!(decoded.max_page, 0);
        assert_eq!(decoded.outcome, CompositionOutcome::CommittedCandidate);
        assert_eq!(decoded.duration_ms, 0);
        assert_eq!(decoded.backspaces, 0);
        assert!(!decoded.redo_after_commit);
        assert_eq!(decoded.candidate_count, 0);
        validate_event(&decoded).unwrap();
    }

    #[test]
    fn mixed_legacy_and_context_model_records_work_for_replay() {
        let legacy = serde_json::to_value(event(1)).unwrap();
        let mut reordered = serde_json::to_value(event(2)).unwrap();
        let observation = serde_json::json!({
            "enabled": true, "outcome": "reordered", "baseOrder": [1, 0], "groupSize": 2
        });
        reordered["contextModel"] = observation.clone();
        let root = root("context-model-records");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("events.jsonl"), format!("{legacy}\n{reordered}\n")).unwrap();
        let events = read_quality_events(&root).unwrap();
        assert!(events[0].context_model.is_none());
        assert_eq!(serde_json::to_value(&events[1]).unwrap()["contextModel"], observation);
        assert!(serde_json::to_value(&events[0]).unwrap().get("contextModel").is_none());
        let (golden, _, _) = build_frozen_golden(&events, 2_000).unwrap();
        assert_eq!(golden.cases.len(), 2);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn context_model_outcomes_round_trip_without_extra_fields() {
        for outcome in ["disabled", "unavailable", "no_group", "timeout", "suspended", "error", "unchanged", "reordered"] {
            let mut observation = serde_json::json!({"enabled": outcome != "disabled", "outcome": outcome});
            if outcome != "disabled" { observation["groupSize"] = serde_json::json!(2); }
            if outcome == "reordered" { observation["baseOrder"] = serde_json::json!([1, 0]); }
            let decoded: ContextModelObservation = serde_json::from_value(observation.clone()).unwrap();
            assert_eq!(serde_json::to_value(decoded).unwrap(), observation);
        }
    }

    #[test]
    fn abandoned_compositions_remain_visible_locally_but_never_enter_replay_golden_cases() {
        let mut abandoned = event(2);
        abandoned.outcome = CompositionOutcome::Abandoned;
        abandoned.selected_text.clear();
        let (golden, _, _) = build_frozen_golden(&[event(1), abandoned], 2_000).unwrap();
        assert_eq!(golden.cases.len(), 1);
        assert_eq!(golden.cases[0].selected_text, "你");
    }

    fn report(status: &str, sample_count: usize) -> ReplayGateReport {
        ReplayGateReport {
            schema_version: REPORT_SCHEMA_VERSION,
            status: status.into(),
            source_kind: REPLAY_SOURCE_KIND.into(),
            artifact_checksum: "1".repeat(64),
            product_commit: "a".repeat(40),
            product_inputs_sha256: "2".repeat(64),
            user_db_checksum: "3".repeat(64),
            generated_at: "2026-07-28T00:00:00Z".into(),
            redaction_state: "redacted-no-input-or-candidate-text".into(),
            golden_checksum: "b".repeat(64),
            candidate_checksum: "c".repeat(64),
            sample_count,
            repetitions: 5,
            baseline_top1_hits: sample_count,
            baseline_top5_hits: sample_count,
            candidate_top1_hits: sample_count,
            candidate_top5_hits: sample_count,
            baseline_top1_cases: sample_count,
            first_choice_regressions: 0,
            jitter_cases: 0,
            latency_samples: sample_count,
            baseline_top1_rate: 1.0,
            baseline_top5_rate: 1.0,
            candidate_top1_rate: 1.0,
            candidate_top5_rate: 1.0,
            first_choice_regression_rate: 0.0,
            candidate_top1_confidence: ConfidenceInterval {
                lower: 0.99,
                upper: 1.0,
            },
            candidate_top5_confidence: ConfidenceInterval {
                lower: 0.99,
                upper: 1.0,
            },
            first_choice_regression_confidence: ConfidenceInterval {
                lower: 0.0,
                upper: 0.009,
            },
            latency_p95_milliseconds: 1.0,
            latency_p99_milliseconds: 2.0,
            thresholds: ReplayThresholds {
                candidate_top1_minimum: 0.95,
                first_choice_regression_maximum: 0.01,
                latency_p95_maximum_milliseconds: 2.5,
                latency_p99_maximum_milliseconds: 5.0,
                candidate_jitter_maximum_cases: 0,
            },
            confidence_sufficient: true,
            measurement_rule: "test".into(),
            confidence_rule: "test".into(),
            privacy_rule: "test".into(),
            differences: vec![],
            report_checksum: "d".repeat(64),
        }
    }

    #[test]
    fn frozen_owner_records_are_immutable_and_checksum_bound() {
        let quality = root("freeze");
        let first = freeze_golden_set(&quality, &[event(1), event(2)], 2_000).unwrap();
        assert_eq!(first.sample_count, 2);
        assert!(is_checksum(&first.checksum));
        let refreshed = freeze_golden_set(&quality, &[event(3)], 3_000).unwrap();
        assert_ne!(refreshed.checksum, first.checksum);
        assert_eq!(refreshed.sample_count, 1);
        let mut accepted = report("passed", 1);
        accepted.golden_checksum = refreshed.checksum.clone();
        accepted.report_checksum = "d".repeat(64);
        mark_golden_used(&quality, &accepted, 44, 3_500, false).unwrap();
        let second = freeze_golden_set(&quality, &[event(4)], 4_000).unwrap();
        assert_eq!(second.checksum, refreshed.checksum);
        assert_eq!(second.sample_count, 1);
        let bytes = fs::read(&refreshed.path).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.contains(REPLAY_SOURCE_KIND));
        assert!(!text.contains("event-1"));
        fs::remove_dir_all(quality).unwrap();
    }

    #[test]
    fn empty_real_collection_freezes_but_is_insufficient_and_blocks() {
        let quality = root("empty");
        let frozen = freeze_golden_set(&quality, &[], 1_000).unwrap();
        assert_eq!(frozen.sample_count, 0);
        let insufficient = report("insufficient_data", 0);
        let error = require_publication_pass(&insufficient).unwrap_err();
        assert!(error.contains("数据不足"));
        fs::remove_dir_all(quality).unwrap();
    }

    #[test]
    fn empty_golden_refreeze_is_one_time_explicit_and_checksum_audited() {
        let quality = root("empty-refreeze");
        let empty = freeze_golden_set(&quality, &[], 1_000).unwrap();
        assert_eq!(empty.sample_count, 0);

        let replaced = refreeze_empty_golden_set_once(
            &quality,
            &[event(1), event(2)],
            2_000,
            "owner approved replacing the never-used empty v0.4 set",
        )
        .unwrap();
        assert_eq!(replaced.sample_count, 2);
        assert_ne!(replaced.checksum, empty.checksum);

        let audit =
            fs::read_to_string(quality.join("ReplayGate/empty-refreeze-v0.4.json")).unwrap();
        assert!(audit.contains(&empty.checksum));
        assert!(audit.contains(&replaced.checksum));
        assert!(audit.contains("owner approved"));

        let repeated = refreeze_empty_golden_set_once(
            &quality,
            &[event(3)],
            3_000,
            "a different request cannot replace it again",
        )
        .unwrap();
        assert_eq!(repeated.checksum, replaced.checksum);
        assert_eq!(repeated.sample_count, 2);
        let ordinary_freeze = freeze_golden_set(&quality, &[event(4)], 4_000).unwrap();
        assert_eq!(ordinary_freeze.checksum, replaced.checksum);
        assert_eq!(ordinary_freeze.sample_count, 2);
        fs::remove_dir_all(quality).unwrap();
    }

    #[test]
    fn failed_gate_blocks_and_explicit_override_is_hash_chained() {
        let quality = root("override");
        let failed = report("failed", 400);
        assert!(require_publication_pass(&failed).is_err());
        let first =
            append_override_audit(&quality, &failed, 44, "owner reviewed regression", 1_000)
                .unwrap();
        let second =
            append_override_audit(&quality, &failed, 45, "temporary dev unblock", 2_000).unwrap();
        assert_ne!(first, second);
        let audit = fs::read_to_string(quality.join("ReplayGate/override-audit.jsonl")).unwrap();
        assert_eq!(audit.lines().count(), 2);
        assert!(audit.contains("owner reviewed regression"));
        assert!(audit.contains(&first));
        assert!(append_override_audit(&quality, &failed, 46, "", 3_000).is_err());
        fs::remove_dir_all(quality).unwrap();
    }

    #[test]
    fn failed_gate_cannot_advance_the_durable_snapshot_pointer() {
        let snapshot_root = root("publish-block");
        let store = SnapshotStore::new(snapshot_root.clone());
        store.publish_fixture(1).unwrap();
        let failed = report("failed", 400);
        assert!(publish_lexicon_after_gate(&store, 2, Vec::new(), &failed).is_err());
        assert_eq!(store.load_current().unwrap().version, 1);

        let passed = report("passed", 400);
        publish_lexicon_after_gate(&store, 2, Vec::new(), &passed).unwrap();
        assert_eq!(store.load_current().unwrap().version, 2);
        fs::remove_dir_all(snapshot_root).unwrap();
    }

    fn golden_meta() -> FrozenGoldenMeta {
        FrozenGoldenMeta {
            batch_id: REPLAY_BATCH_ID.into(),
            checksum: "b".repeat(64),
            sample_count: 400,
            source_kind: REPLAY_SOURCE_KIND.into(),
            frozen_at_ms: 1_000,
            path: PathBuf::from("/nonexistent"),
        }
    }

    fn bound_provenance() -> ReportProvenance<'static> {
        ReportProvenance {
            artifact_checksum: "1111111111111111111111111111111111111111111111111111111111111111",
            product_commit: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            product_inputs_sha256:
                "2222222222222222222222222222222222222222222222222222222222222222",
            user_db_checksum: "3333333333333333333333333333333333333333333333333333333333333333",
        }
    }

    /// The golden corpus stays at schema 1 while the helper reports schema 2.
    /// Sharing one constant is what silently disabled the Manager's gate.
    #[test]
    fn golden_and_report_schema_versions_are_independent_contracts() {
        assert_eq!(GOLDEN_SCHEMA_VERSION, 1);
        assert_eq!(REPORT_SCHEMA_VERSION, 2);

        let candidate = "c".repeat(64);
        let accepted = report("passed", 400);
        validate_report(&accepted, &golden_meta(), &candidate, &bound_provenance()).unwrap();

        let mut stale = report("passed", 400);
        stale.schema_version = GOLDEN_SCHEMA_VERSION;
        assert!(validate_report(&stale, &golden_meta(), &candidate, &bound_provenance()).is_err());
    }

    #[test]
    fn a_report_measuring_another_artifact_commit_or_userdb_is_rejected() {
        let candidate = "c".repeat(64);
        for mutate in [
            (|report: &mut ReplayGateReport| report.artifact_checksum = "9".repeat(64))
                as fn(&mut ReplayGateReport),
            |report: &mut ReplayGateReport| report.product_commit = "9".repeat(40),
            |report: &mut ReplayGateReport| report.product_inputs_sha256 = "9".repeat(64),
            |report: &mut ReplayGateReport| report.user_db_checksum = "9".repeat(64),
            |report: &mut ReplayGateReport| report.redaction_state = "raw".into(),
        ] {
            let mut tampered = report("passed", 400);
            mutate(&mut tampered);
            assert!(
                validate_report(&tampered, &golden_meta(), &candidate, &bound_provenance())
                    .is_err()
            );
        }
    }

    /// The Rust checksum must agree with `script/artifact_checksum.sh`, because
    /// the CLI runner binds reports with the script and the Manager binds them
    /// with this function; a divergence would make the two paths disagree about
    /// the same bundle.
    #[test]
    fn directory_checksum_agrees_with_the_shell_implementation() {
        let fixture = root("artifact-checksum");
        fs::create_dir_all(fixture.join("Contents/MacOS")).unwrap();
        fs::create_dir_all(fixture.join("Contents/Resources/Rime")).unwrap();
        fs::write(fixture.join("Contents/Info.plist"), b"plist").unwrap();
        fs::write(fixture.join("Contents/MacOS/Binary"), b"binary\n").unwrap();
        fs::write(fixture.join("Contents/MacOS/Alpha"), b"").unwrap();
        fs::write(fixture.join("Contents/Resources/Rime/a.yaml"), b"a: 1\n").unwrap();
        fs::write(fixture.join("Contents/Resources/Rime/B.yaml"), b"b: 2\n").unwrap();
        fs::write(fixture.join("top-level"), b"top\n").unwrap();

        let script = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("script/artifact_checksum.sh");
        let output = Command::new(&script).arg(&fixture).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let expected = String::from_utf8(output.stdout).unwrap().trim().to_string();

        assert_eq!(directory_checksum(&fixture, "fixture").unwrap(), expected);
        fs::remove_dir_all(fixture).unwrap();
    }

    #[test]
    fn directory_checksum_refuses_a_tree_containing_symlinks() {
        let fixture = root("artifact-symlink");
        fs::create_dir_all(fixture.join("Contents")).unwrap();
        fs::write(fixture.join("Contents/real"), b"real").unwrap();
        std::os::unix::fs::symlink("real", fixture.join("Contents/link")).unwrap();
        assert!(directory_checksum(&fixture, "fixture").is_err());
        fs::remove_dir_all(fixture).unwrap();
    }

    #[test]
    fn the_userdb_snapshot_is_private_independent_and_checksum_bound() {
        let quality = root("userdb-snapshot");
        let replay_root = prepare_replay_root(&quality).unwrap();
        let live = quality.join("live-rime");
        fs::create_dir_all(live.join("nested")).unwrap();
        fs::write(live.join("pinyin_simp.userdb"), b"one\n").unwrap();
        fs::write(live.join("nested/log"), b"two\n").unwrap();

        let (snapshot, checksum) = snapshot_user_rime(&replay_root, &live).unwrap();
        assert!(is_checksum(&checksum));
        assert_eq!(
            fs::read(snapshot.join("pinyin_simp.userdb")).unwrap(),
            b"one\n"
        );
        assert_eq!(
            fs::metadata(&snapshot).unwrap().permissions().mode() & 0o777,
            0o700
        );

        // The owner keeps typing: the snapshot must not move with the live tree.
        fs::write(live.join("pinyin_simp.userdb"), b"three\n").unwrap();
        assert_eq!(
            directory_checksum(&snapshot, "UserDB 快照").unwrap(),
            checksum
        );

        let (_, second) = snapshot_user_rime(&replay_root, &live).unwrap();
        assert_ne!(second, checksum);
        fs::remove_dir_all(quality).unwrap();
    }
}
