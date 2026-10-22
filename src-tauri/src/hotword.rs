use crate::storage::{AppStorage, LexiconSource};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

const MAX_FEED_BYTES: usize = 2 * 1024 * 1024;
const FETCH_TIMEOUT: Duration = Duration::from_secs(12);
pub(crate) const SELF_AUTHORED_HOTWORD_SOURCE_ID: &str = "owner-self-authored-hotwords";

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DynamicSourceInput {
    pub id: String,
    pub display_name: String,
    pub source_type: String,
    pub refresh_interval_minutes: i64,
    pub authorization: String,
    pub terms_url: String,
    pub license: String,
    pub cache_policy: String,
    pub ownership: String,
    pub distribution: String,
    pub disable_policy: String,
    pub endpoint: String,
    pub opt_in: bool,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DynamicSource {
    pub id: String,
    pub display_name: String,
    pub source_type: String,
    pub refresh_interval_minutes: i64,
    pub authorization: String,
    pub terms_url: String,
    pub license: String,
    pub cache_policy: String,
    pub ownership: String,
    pub distribution: String,
    pub disable_policy: String,
    pub endpoint: String,
    pub enabled: bool,
    pub opt_in: bool,
    pub last_refresh_ms: Option<i64>,
    pub last_error: Option<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HotwordItem {
    pub id: String,
    pub text: String,
    pub pinyin: String,
    pub confidence_millis: i64,
    pub freshness_millis: i64,
    pub domain: String,
    pub expires_at_ms: Option<i64>,
    pub review_required: bool,
    pub state: String,
    pub source_ids: Vec<String>,
    pub exposure_count: i64,
    pub misselection_count: i64,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PredictionEntry {
    pub id: String,
    pub text: String,
    pub pinyin: String,
    pub domain: String,
    pub sources: Vec<LexiconSource>,
    pub base_weight: i64,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HotwordExposureReport {
    pub considered_events: usize,
    pub tracked_items: usize,
    pub changed_items: usize,
    pub total_exposures: i64,
    pub total_misselections: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RefreshResult {
    pub source_id: String,
    pub received_count: usize,
    pub staged_count: usize,
    pub review_count: usize,
    pub retained_previous_on_failure: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct JsonFeed {
    items: Vec<FeedRecord>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FeedRecord {
    text: String,
    #[serde(default)]
    pinyin: String,
    #[serde(default = "default_confidence")]
    confidence_millis: i64,
    #[serde(default = "default_freshness")]
    freshness_millis: i64,
    #[serde(default)]
    domain: String,
    expires_at_ms: Option<i64>,
    #[serde(default)]
    source_record_id: String,
}

fn default_confidence() -> i64 {
    700
}
fn default_freshness() -> i64 {
    1000
}

impl AppStorage {
    pub(crate) fn sync_approved_generated_hotwords(
        &self,
        now_ms: i64,
    ) -> Result<RefreshResult, String> {
        let source = DynamicSourceInput {
            id: SELF_AUTHORED_HOTWORD_SOURCE_ID.into(),
            display_name: "Owner 自著热词".into(),
            source_type: "local_text".into(),
            refresh_interval_minutes: 60,
            authorization: "self-authored: owner explicitly approved each entry".into(),
            terms_url: "self-authored://owner-review".into(),
            license: "self-authored".into(),
            cache_policy: "local-reviewed-only".into(),
            ownership: "self-authored".into(),
            distribution: "local-snapshot-only".into(),
            disable_policy: "pause-and-clear".into(),
            endpoint: String::new(),
            opt_in: true,
            updated_at_ms: now_ms,
        };
        self.save_dynamic_source(source)?;
        self.set_dynamic_source_enabled(SELF_AUTHORED_HOTWORD_SOURCE_ID, true)?;

        let document = {
            let connection = self.lock_connection()?;
            let mut statement = connection
                .prepare(
                    "SELECT text,pinyin,definition
                     FROM knowledge_terms
                     WHERE kind='generated_hotword' AND review_state='approved' AND pinyin<>''
                     ORDER BY text,pinyin,id",
                )
                .map_err(|error| format!("无法准备已审热词本地源：{error}"))?;
            let rows = statement
                .query_map([], |row| {
                    Ok(format!(
                        "{}\t{}\t{}",
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?
                    ))
                })
                .map_err(|error| format!("无法读取已审热词本地源：{error}"))?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|error| format!("无法解析已审热词本地源：{error}"))?
                .join("\n")
        };
        self.ingest_hotword_feed(SELF_AUTHORED_HOTWORD_SOURCE_ID, document.as_bytes(), now_ms)
    }

    pub fn save_dynamic_source(&self, input: DynamicSourceInput) -> Result<DynamicSource, String> {
        let input = validate_source_input(input)?;
        let mut connection = self.lock_connection()?;
        let transaction = connection
            .transaction()
            .map_err(|error| format!("无法开始数据源保存事务：{error}"))?;
        let created_at = transaction
            .query_row(
                "SELECT created_at_ms FROM dynamic_sources WHERE id = ?1",
                params![&input.id],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(|error| format!("无法读取数据源：{error}"))?
            .unwrap_or(input.updated_at_ms);
        transaction
            .execute(
                "INSERT INTO dynamic_sources
                 (id, display_name, source_type, refresh_interval_minutes, authorization,
                  terms_url, license, cache_policy, ownership, distribution, disable_policy,
                  endpoint, enabled, opt_in, created_at_ms, updated_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 0, ?13, ?14, ?15)
                 ON CONFLICT(id) DO UPDATE SET
                   display_name=excluded.display_name, source_type=excluded.source_type,
                   refresh_interval_minutes=excluded.refresh_interval_minutes,
                   authorization=excluded.authorization, terms_url=excluded.terms_url,
                   license=excluded.license, cache_policy=excluded.cache_policy,
                   ownership=excluded.ownership, distribution=excluded.distribution,
                   disable_policy=excluded.disable_policy, endpoint=excluded.endpoint,
                   opt_in=excluded.opt_in, updated_at_ms=excluded.updated_at_ms",
                params![
                    &input.id,
                    &input.display_name,
                    &input.source_type,
                    input.refresh_interval_minutes,
                    &input.authorization,
                    &input.terms_url,
                    &input.license,
                    &input.cache_policy,
                    &input.ownership,
                    &input.distribution,
                    &input.disable_policy,
                    &input.endpoint,
                    input.opt_in as i64,
                    created_at,
                    input.updated_at_ms,
                ],
            )
            .map_err(|error| format!("无法保存数据源：{error}"))?;
        let mut saved = transaction
            .query_row(
                "SELECT id, display_name, source_type, refresh_interval_minutes, authorization,
                        terms_url, license, cache_policy, ownership, distribution, disable_policy,
                        endpoint, enabled, opt_in, last_refresh_ms, last_error, created_at_ms, updated_at_ms
                 FROM dynamic_sources WHERE id=?1",
                params![&input.id],
                map_source,
            )
            .map_err(|error| format!("无法读取保存后的数据源：{error}"))?;
        if saved.enabled && enforce_enable_policy(&saved).is_err() {
            transaction
                .execute(
                    "UPDATE dynamic_sources SET enabled=0 WHERE id=?1",
                    params![&input.id],
                )
                .map_err(|error| format!("无法按启用策略停用数据源：{error}"))?;
            saved.enabled = false;
        }
        transaction
            .commit()
            .map_err(|error| format!("无法提交数据源保存事务：{error}"))?;
        Ok(saved)
    }

    pub fn list_dynamic_sources(&self) -> Result<Vec<DynamicSource>, String> {
        let connection = self.lock_connection()?;
        let mut statement = connection.prepare(
            "SELECT id, display_name, source_type, refresh_interval_minutes, authorization,
                    terms_url, license, cache_policy, ownership, distribution, disable_policy,
                    endpoint, enabled, opt_in, last_refresh_ms, last_error, created_at_ms, updated_at_ms
             FROM dynamic_sources ORDER BY display_name, id"
        ).map_err(|error| format!("无法读取数据源：{error}"))?;
        let rows = statement
            .query_map([], map_source)
            .map_err(|error| format!("无法读取数据源：{error}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("无法解析数据源：{error}"))
    }

    pub fn set_dynamic_source_enabled(
        &self,
        id: &str,
        enabled: bool,
    ) -> Result<DynamicSource, String> {
        let source = self
            .get_dynamic_source(id)?
            .ok_or_else(|| "数据源不存在。".to_string())?;
        if enabled {
            enforce_enable_policy(&source)?;
        }
        let connection = self.lock_connection()?;
        connection
            .execute(
                "UPDATE dynamic_sources SET enabled = ?2 WHERE id = ?1",
                params![id, enabled as i64],
            )
            .map_err(|error| format!("无法更新数据源状态：{error}"))?;
        drop(connection);
        self.get_dynamic_source(id)?
            .ok_or_else(|| "数据源更新后不可见。".into())
    }

    pub fn clear_dynamic_source(&self, id: &str) -> Result<usize, String> {
        let mut connection = self.lock_connection()?;
        let transaction = connection
            .transaction()
            .map_err(|error| format!("无法开始清除事务：{error}"))?;
        let item_ids = {
            let mut statement = transaction
                .prepare("SELECT DISTINCT item_id FROM hotword_item_sources WHERE source_id = ?1")
                .map_err(|error| format!("无法读取来源派生词：{error}"))?;
            let rows = statement
                .query_map(params![id], |row| row.get::<_, String>(0))
                .map_err(|error| format!("无法读取来源派生词：{error}"))?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|error| format!("无法解析来源派生词：{error}"))?
        };
        transaction
            .execute(
                "DELETE FROM hotword_item_sources WHERE source_id = ?1",
                params![id],
            )
            .map_err(|error| format!("无法清除来源关联：{error}"))?;
        let mut removed = 0;
        for item_id in item_ids {
            removed += transaction
                .execute(
                    "DELETE FROM hotword_items WHERE id = ?1 AND NOT EXISTS
                 (SELECT 1 FROM hotword_item_sources WHERE item_id = ?1)",
                    params![item_id],
                )
                .map_err(|error| format!("无法清除派生热词：{error}"))?;
        }
        transaction
            .commit()
            .map_err(|error| format!("无法提交清除事务：{error}"))?;
        Ok(removed)
    }

    pub fn list_hotword_items(&self) -> Result<Vec<HotwordItem>, String> {
        let connection = self.lock_connection()?;
        let mut statement = connection
            .prepare(
                "SELECT id, text, pinyin, confidence_millis, freshness_millis, domain,
                    expires_at_ms, review_required, state, exposure_count,
                    misselection_count, updated_at_ms
             FROM hotword_items ORDER BY state, updated_at_ms DESC, text",
            )
            .map_err(|error| format!("无法读取热词：{error}"))?;
        let bases = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, Option<i64>>(6)?,
                    row.get::<_, i64>(7)? != 0,
                    row.get::<_, String>(8)?,
                    row.get::<_, i64>(9)?,
                    row.get::<_, i64>(10)?,
                    row.get::<_, i64>(11)?,
                ))
            })
            .map_err(|error| format!("无法读取热词：{error}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("无法解析热词：{error}"))?;
        let mut items = Vec::with_capacity(bases.len());
        for (
            id,
            text,
            pinyin,
            confidence_millis,
            freshness_millis,
            domain,
            expires_at_ms,
            review_required,
            state,
            exposure_count,
            misselection_count,
            updated_at_ms,
        ) in bases
        {
            let mut source_statement = connection.prepare(
                "SELECT DISTINCT source_id FROM hotword_item_sources WHERE item_id = ?1 ORDER BY source_id"
            ).map_err(|error| format!("无法读取热词来源：{error}"))?;
            let source_ids = source_statement
                .query_map(params![&id], |row| row.get::<_, String>(0))
                .map_err(|error| format!("无法读取热词来源：{error}"))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| format!("无法解析热词来源：{error}"))?;
            items.push(HotwordItem {
                id,
                text,
                pinyin,
                confidence_millis,
                freshness_millis,
                domain,
                expires_at_ms,
                review_required,
                state,
                source_ids,
                exposure_count,
                misselection_count,
                updated_at_ms,
            });
        }
        Ok(items)
    }

    /// Recomputes `exposure_count` / `misselection_count` for every hotword item from
    /// the recorded quality events.
    ///
    /// Both columns shipped in migration 006 and were read by `list_hotword_items` and
    /// rendered by the data-source panel, but nothing ever wrote them — the panel
    /// therefore always showed `曝光 0 · 误选 0`, which made V06-06 ("面板显示候选曝光
    /// 和误选") and V06-04 ("误选可降权和屏蔽") structurally unreachable.
    ///
    /// Definitions, kept deliberately narrow so the numbers mean exactly one thing:
    /// - **exposure**: the item's text was present in the candidate list the owner was
    ///   actually shown for that composition.
    /// - **misselection**: it was shown and the owner committed something else. An
    ///   abandoned composition counts as neither — nothing was chosen, so calling it a
    ///   misselection would blame the hotword for the owner giving up.
    ///
    /// This recomputes absolute totals rather than incrementing, because the event log
    /// is append-only and re-read in full on every call; incrementing would inflate the
    /// counts on the second run. Recomputation is idempotent for a given log.
    pub fn recompute_hotword_exposure(
        &self,
        events: &[crate::replay_gate::RecordedQualityEvent],
        now_ms: i64,
    ) -> Result<HotwordExposureReport, String> {
        use crate::replay_gate::CompositionOutcome;

        let mut connection = self.lock_connection()?;
        let mut statement = connection
            .prepare("SELECT id, text FROM hotword_items")
            .map_err(|error| format!("无法读取热词：{error}"))?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|error| format!("无法读取热词：{error}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("无法解析热词：{error}"))?;
        drop(statement);

        // Several items can carry the same text (same word staged from two sources);
        // each of them is credited with the exposure, so the panel does not silently
        // attribute a shared word to whichever row happened to be first.
        let mut by_text = BTreeMap::<&str, Vec<&str>>::new();
        for (id, text) in &rows {
            by_text.entry(text.as_str()).or_default().push(id.as_str());
        }
        let mut exposures = BTreeMap::<&str, (i64, i64)>::new();
        for (id, _) in &rows {
            exposures.insert(id.as_str(), (0, 0));
        }

        let mut counted_events = 0usize;
        for event in events {
            if event.outcome == CompositionOutcome::Abandoned {
                continue;
            }
            counted_events += 1;
            // A candidate list can repeat a text across pages; dedupe so one shown
            // composition is one exposure.
            let shown = event
                .candidates
                .iter()
                .map(String::as_str)
                .collect::<BTreeSet<_>>();
            for text in shown {
                let Some(item_ids) = by_text.get(text) else {
                    continue;
                };
                let missed = event.selected_text != text;
                for item_id in item_ids {
                    let entry = exposures.entry(item_id).or_insert((0, 0));
                    entry.0 += 1;
                    if missed {
                        entry.1 += 1;
                    }
                }
            }
        }

        let transaction = connection
            .transaction()
            .map_err(|error| format!("无法开始曝光统计事务：{error}"))?;
        let mut changed_items = 0usize;
        for (item_id, (exposure, misselection)) in &exposures {
            let updated = transaction
                .execute(
                    "UPDATE hotword_items
                     SET exposure_count = ?2, misselection_count = ?3, updated_at_ms = ?4
                     WHERE id = ?1 AND (exposure_count <> ?2 OR misselection_count <> ?3)",
                    params![item_id, exposure, misselection, now_ms],
                )
                .map_err(|error| format!("无法写入曝光统计：{error}"))?;
            changed_items += updated;
        }
        transaction
            .commit()
            .map_err(|error| format!("无法提交曝光统计：{error}"))?;

        Ok(HotwordExposureReport {
            considered_events: counted_events,
            tracked_items: rows.len(),
            changed_items,
            total_exposures: exposures.values().map(|(exposure, _)| *exposure).sum(),
            total_misselections: exposures
                .values()
                .map(|(_, misselection)| *misselection)
                .sum(),
        })
    }

    pub fn list_prediction_entries(&self, now_ms: i64) -> Result<Vec<PredictionEntry>, String> {
        let connection = self.lock_connection()?;
        let mut statement = connection
            .prepare(
                "SELECT id, text, pinyin, domain, confidence_millis, freshness_millis, updated_at_ms
                 FROM hotword_items
                 WHERE state='staged' AND review_required=0 AND pinyin<>''
                   AND (expires_at_ms IS NULL OR expires_at_ms > ?1)
                 ORDER BY text, pinyin, id",
            )
            .map_err(|error| format!("无法读取关联预测词：{error}"))?;
        let rows = statement
            .query_map(params![now_ms], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, i64>(6)?,
                ))
            })
            .map_err(|error| format!("无法读取关联预测词：{error}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("无法解析关联预测词：{error}"))?;
        let mut output = Vec::new();
        for (id, text, pinyin, domain, confidence, freshness, updated_at_ms) in rows {
            let mut source_statement = connection
                .prepare(
                    "SELECT s.id, s.source_type, s.display_name, s.license
                     FROM hotword_item_sources hs
                     JOIN dynamic_sources s ON s.id=hs.source_id
                     WHERE hs.item_id=?1 AND s.enabled=1 AND s.license<>''
                     ORDER BY s.id, s.source_type, s.display_name, s.license",
                )
                .map_err(|error| format!("无法读取预测词来源：{error}"))?;
            let sources = source_statement
                .query_map(params![&id], |row| {
                    Ok(LexiconSource {
                        source_kind: format!("prediction:{}", row.get::<_, String>(1)?),
                        source: format!(
                            "{}:{}",
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(2)?
                        ),
                        license: row.get(3)?,
                        confidence_millis: confidence.min(freshness),
                    })
                })
                .map_err(|error| format!("无法读取预测词来源：{error}"))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| format!("无法解析预测词来源：{error}"))?;
            if !sources.is_empty() {
                output.push(PredictionEntry {
                    id,
                    text,
                    pinyin,
                    domain,
                    sources,
                    base_weight: confidence.saturating_mul(freshness) / 1000,
                    updated_at_ms,
                });
            }
        }
        Ok(output)
    }

    pub fn ingest_hotword_feed(
        &self,
        source_id: &str,
        body: &[u8],
        now_ms: i64,
    ) -> Result<RefreshResult, String> {
        let source = self
            .get_dynamic_source(source_id)?
            .ok_or_else(|| "数据源不存在。".to_string())?;
        enforce_active_source(&source)?;
        if body.len() > MAX_FEED_BYTES {
            return Err("数据源响应超过 2 MiB 上限；已保留上一有效数据。".into());
        }
        let records = parse_feed(&source.source_type, body)?;
        let normalized = normalize_records(records, now_ms)?;
        let mut connection = self.lock_connection()?;
        let transaction = connection
            .transaction()
            .map_err(|error| format!("无法开始热词更新事务：{error}"))?;
        transaction
            .execute(
                "DELETE FROM hotword_item_sources WHERE source_id = ?1",
                params![source_id],
            )
            .map_err(|error| format!("无法替换来源热词：{error}"))?;
        for record in &normalized {
            transaction.execute(
                "INSERT INTO hotword_items
                 (id, text, pinyin, confidence_millis, freshness_millis, domain, expires_at_ms,
                  review_required, state, created_at_ms, updated_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10)
                 ON CONFLICT(text, pinyin) DO UPDATE SET
                   confidence_millis=max(hotword_items.confidence_millis, excluded.confidence_millis),
                   freshness_millis=max(hotword_items.freshness_millis, excluded.freshness_millis),
                   domain=CASE WHEN hotword_items.domain='' THEN excluded.domain ELSE hotword_items.domain END,
                   expires_at_ms=excluded.expires_at_ms,
                   review_required=max(hotword_items.review_required, excluded.review_required),
                   state=CASE WHEN max(hotword_items.review_required, excluded.review_required)=1 THEN 'review' ELSE 'staged' END,
                   updated_at_ms=excluded.updated_at_ms",
                params![record.id, record.text, record.pinyin, record.confidence_millis,
                    record.freshness_millis, record.domain, record.expires_at_ms,
                    record.review_required as i64, record.state, now_ms]
            ).map_err(|error| format!("无法保存规范化热词：{error}"))?;
            let item_id: String = transaction
                .query_row(
                    "SELECT id FROM hotword_items WHERE text=?1 AND pinyin=?2",
                    params![record.text, record.pinyin],
                    |row| row.get(0),
                )
                .map_err(|error| format!("无法定位规范化热词：{error}"))?;
            transaction.execute(
                "INSERT INTO hotword_item_sources (item_id, source_id, source_record_id, fetched_at_ms)
                 VALUES (?1, ?2, ?3, ?4)", params![item_id, source_id, record.source_record_id, now_ms]
            ).map_err(|error| format!("无法保存热词来源：{error}"))?;
        }
        transaction
            .execute(
                "DELETE FROM hotword_items WHERE NOT EXISTS
             (SELECT 1 FROM hotword_item_sources WHERE item_id = hotword_items.id)",
                [],
            )
            .map_err(|error| format!("无法清理失效热词：{error}"))?;
        transaction
            .execute(
                "UPDATE dynamic_sources SET last_refresh_ms=?2, last_error=NULL WHERE id=?1",
                params![source_id, now_ms],
            )
            .map_err(|error| format!("无法记录刷新状态：{error}"))?;
        transaction
            .commit()
            .map_err(|error| format!("无法提交热词更新：{error}"))?;
        Ok(RefreshResult {
            source_id: source_id.into(),
            received_count: normalized.len(),
            staged_count: normalized
                .iter()
                .filter(|item| !item.review_required)
                .count(),
            review_count: normalized
                .iter()
                .filter(|item| item.review_required)
                .count(),
            retained_previous_on_failure: false,
        })
    }

    pub fn record_dynamic_source_error(&self, id: &str, message: &str) -> Result<(), String> {
        let connection = self.lock_connection()?;
        connection
            .execute(
                "UPDATE dynamic_sources SET last_error=?2 WHERE id=?1",
                params![id, message],
            )
            .map_err(|error| format!("无法记录数据源错误：{error}"))?;
        Ok(())
    }

    fn get_dynamic_source(&self, id: &str) -> Result<Option<DynamicSource>, String> {
        let connection = self.lock_connection()?;
        connection.query_row(
            "SELECT id, display_name, source_type, refresh_interval_minutes, authorization,
                    terms_url, license, cache_policy, ownership, distribution, disable_policy,
                    endpoint, enabled, opt_in, last_refresh_ms, last_error, created_at_ms, updated_at_ms
             FROM dynamic_sources WHERE id=?1", params![id], map_source
        ).optional().map_err(|error| format!("无法读取数据源：{error}"))
    }
}

pub async fn refresh_remote_source(
    storage: &AppStorage,
    source_id: &str,
    now_ms: i64,
) -> Result<RefreshResult, String> {
    let source = storage
        .get_dynamic_source(source_id)?
        .ok_or_else(|| "数据源不存在。".to_string())?;
    enforce_active_source(&source)?;
    if source.source_type == "local_text" {
        return Err("本地文本来源必须使用本地导入，不允许取网。".into());
    }
    if !source.endpoint.starts_with("https://") {
        return Err("官方来源只允许 HTTPS endpoint。".into());
    }
    let client = hotword_fetch_client()?;
    let response = client
        .get(&source.endpoint)
        .send()
        .await
        .map_err(|error| classify_fetch_error(&error))?;
    // https_only already refuses http targets; re-check the final URL so a
    // redirect that somehow lands off-https is rejected before the body is
    // trusted.
    if response.url().scheme() != "https" {
        return Err("官方源重定向离开了 HTTPS；已保留上一有效数据。".into());
    }
    let status = response.status();
    if status.as_u16() == 429 {
        return Err("官方源限流（HTTP 429）；已保留上一有效数据。".into());
    }
    if !status.is_success() {
        return Err(format!("官方源返回 HTTP {status}；已保留上一有效数据。"));
    }
    let body = response
        .bytes()
        .await
        .map_err(|error| format!("读取官方源失败：{error}；已保留上一有效数据。"))?;
    storage.ingest_hotword_feed(source_id, &body, now_ms)
}

fn map_source(row: &rusqlite::Row<'_>) -> rusqlite::Result<DynamicSource> {
    Ok(DynamicSource {
        id: row.get(0)?,
        display_name: row.get(1)?,
        source_type: row.get(2)?,
        refresh_interval_minutes: row.get(3)?,
        authorization: row.get(4)?,
        terms_url: row.get(5)?,
        license: row.get(6)?,
        cache_policy: row.get(7)?,
        ownership: row.get(8)?,
        distribution: row.get(9)?,
        disable_policy: row.get(10)?,
        endpoint: row.get(11)?,
        enabled: row.get::<_, i64>(12)? != 0,
        opt_in: row.get::<_, i64>(13)? != 0,
        last_refresh_ms: row.get(14)?,
        last_error: row.get(15)?,
        created_at_ms: row.get(16)?,
        updated_at_ms: row.get(17)?,
    })
}

fn validate_source_input(mut input: DynamicSourceInput) -> Result<DynamicSourceInput, String> {
    input.id = required("数据源 ID", input.id)?;
    input.display_name = required("数据源名称", input.display_name)?;
    input.cache_policy = required("缓存策略", input.cache_policy)?;
    input.distribution = required("分发策略", input.distribution)?;
    input.disable_policy = required("禁用策略", input.disable_policy)?;
    input.authorization = input.authorization.trim().to_string();
    input.terms_url = input.terms_url.trim().to_string();
    input.license = input.license.trim().to_string();
    input.endpoint = input.endpoint.trim().to_string();
    if !matches!(
        input.source_type.as_str(),
        "official_json" | "official_xml" | "local_text"
    ) {
        return Err("数据源类型不受支持。".into());
    }
    if !matches!(
        input.ownership.as_str(),
        "bundled" | "user_local" | "remote_cached" | "self-authored"
    ) {
        return Err("数据归属不受支持。".into());
    }
    if input.refresh_interval_minutes < 5 {
        return Err("刷新间隔不得短于 5 分钟。".into());
    }
    if input.updated_at_ms < 0 {
        return Err("数据源更新时间不能为负数。".into());
    }
    Ok(input)
}

fn enforce_enable_policy(source: &DynamicSource) -> Result<(), String> {
    if source.license.trim().is_empty()
        || source.terms_url.trim().is_empty()
        || source.authorization.trim().is_empty()
    {
        return Err("未登记许可证、条款或授权的数据源不得启用。".into());
    }
    if !source.opt_in {
        return Err("数据源默认关闭，必须由 owner 显式 opt-in。".into());
    }
    Ok(())
}

fn enforce_active_source(source: &DynamicSource) -> Result<(), String> {
    if !source.enabled {
        return Err("数据源已停用；不得取网或入库。".into());
    }
    enforce_enable_policy(source)
}

fn required(label: &str, value: String) -> Result<String, String> {
    let value = value.trim().to_string();
    if value.is_empty() {
        Err(format!("{label}不可为空。"))
    } else {
        Ok(value)
    }
}

fn parse_feed(source_type: &str, body: &[u8]) -> Result<Vec<FeedRecord>, String> {
    let text = std::str::from_utf8(body)
        .map_err(|_| "数据源不是有效 UTF-8；已保留上一有效数据。".to_string())?;
    match source_type {
        "official_json" => serde_json::from_str::<JsonFeed>(text)
            .map(|feed| feed.items)
            .map_err(|_| "官方 JSON 结构无效；已保留上一有效数据。".into()),
        "official_xml" => parse_xml_feed(text),
        "local_text" => Ok(text
            .lines()
            .enumerate()
            .filter_map(|(index, line)| {
                let mut columns = line.splitn(3, '\t');
                let text = columns.next().unwrap_or_default().trim();
                let pinyin = columns.next().unwrap_or_default().trim();
                (!text.is_empty()).then(|| FeedRecord {
                    text: text.into(),
                    pinyin: pinyin.into(),
                    confidence_millis: 600,
                    freshness_millis: 1000,
                    domain: "local".into(),
                    expires_at_ms: None,
                    source_record_id: format!("line-{}", index + 1),
                })
            })
            .collect()),
        _ => Err("数据源类型不受支持。".into()),
    }
}

fn parse_xml_feed(text: &str) -> Result<Vec<FeedRecord>, String> {
    let mut records = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("<item") {
        rest = &rest[start..];
        let Some(open_end) = rest.find('>') else {
            return Err("官方 XML 起始标签损坏；已保留上一有效数据。".into());
        };
        rest = &rest[open_end + 1..];
        let Some(end) = rest.find("</item>") else {
            return Err("官方 XML 结束标签损坏；已保留上一有效数据。".into());
        };
        let item = &rest[..end];
        let text_value = xml_value(item, "text")
            .ok_or_else(|| "官方 XML item 缺少 text；已保留上一有效数据。".to_string())?;
        records.push(FeedRecord {
            text: decode_xml(text_value),
            pinyin: xml_value(item, "pinyin")
                .map(decode_xml)
                .unwrap_or_default(),
            confidence_millis: xml_value(item, "confidenceMillis")
                .and_then(|v| v.parse().ok())
                .unwrap_or(700),
            freshness_millis: xml_value(item, "freshnessMillis")
                .and_then(|v| v.parse().ok())
                .unwrap_or(1000),
            domain: xml_value(item, "domain")
                .map(decode_xml)
                .unwrap_or_default(),
            expires_at_ms: xml_value(item, "expiresAtMs").and_then(|v| v.parse().ok()),
            source_record_id: xml_value(item, "id").map(decode_xml).unwrap_or_default(),
        });
        rest = &rest[end + 7..];
    }
    if records.is_empty() {
        return Err("官方 XML 不含 item；已保留上一有效数据。".into());
    }
    Ok(records)
}

fn xml_value<'a>(item: &'a str, tag: &str) -> Option<&'a str> {
    let start_tag = format!("<{tag}>");
    let end_tag = format!("</{tag}>");
    let start = item.find(&start_tag)? + start_tag.len();
    let end = item[start..].find(&end_tag)? + start;
    Some(item[start..end].trim())
}

fn decode_xml(value: &str) -> String {
    value
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

fn normalize_records(
    records: Vec<FeedRecord>,
    now_ms: i64,
) -> Result<Vec<NormalizedRecord>, String> {
    if records.len() > 10_000 {
        return Err("单次热词更新超过 10000 条；已保留上一有效数据。".into());
    }
    let mut merged: BTreeMap<(String, String), NormalizedRecord> = BTreeMap::new();
    for (index, record) in records.into_iter().enumerate() {
        let text = record.text.trim().to_string();
        if text.is_empty() || text.chars().count() > 64 {
            return Err("热词文本为空或超过 64 字；已保留上一有效数据。".into());
        }
        if !(0..=1000).contains(&record.confidence_millis)
            || !(0..=1000).contains(&record.freshness_millis)
        {
            return Err("热词可信度或时效超出范围；已保留上一有效数据。".into());
        }
        let pinyin = normalize_pinyin(&record.pinyin);
        let review_required = pinyin.is_empty();
        let key = (text.clone(), pinyin.clone());
        let source_record_id = if record.source_record_id.trim().is_empty() {
            format!("record-{}", index + 1)
        } else {
            record.source_record_id.trim().into()
        };
        let id = stable_id(&text, &pinyin);
        let candidate = NormalizedRecord {
            id,
            text,
            pinyin,
            confidence_millis: record.confidence_millis,
            freshness_millis: record.freshness_millis,
            domain: record.domain.trim().to_string(),
            expires_at_ms: record.expires_at_ms,
            review_required,
            state: if review_required {
                "review".into()
            } else if record.expires_at_ms.is_some_and(|value| value <= now_ms) {
                "expired".into()
            } else {
                "staged".into()
            },
            source_record_id,
        };
        merged
            .entry(key)
            .and_modify(|existing| {
                existing.confidence_millis =
                    existing.confidence_millis.max(candidate.confidence_millis);
                existing.freshness_millis =
                    existing.freshness_millis.max(candidate.freshness_millis);
            })
            .or_insert(candidate);
    }
    Ok(merged.into_values().collect())
}

#[derive(Debug, Clone)]
struct NormalizedRecord {
    id: String,
    text: String,
    pinyin: String,
    confidence_millis: i64,
    freshness_millis: i64,
    domain: String,
    expires_at_ms: Option<i64>,
    review_required: bool,
    state: String,
    source_record_id: String,
}

fn normalize_pinyin(value: &str) -> String {
    let mut syllables = BTreeSet::new();
    let normalized = value
        .to_lowercase()
        .split_whitespace()
        .filter(|part| {
            !part.is_empty() && part.chars().all(|ch| ch.is_ascii_lowercase() || ch == '\'')
        })
        .collect::<Vec<_>>()
        .join(" ");
    if !normalized.is_empty() {
        syllables.insert(normalized);
    }
    syllables.into_iter().next().unwrap_or_default()
}

fn stable_id(text: &str, pinyin: &str) -> String {
    let digest = Sha256::digest(format!("hotword\0{text}\0{pinyin}").as_bytes());
    format!("hot-{:x}", digest)[..28].to_string()
}

fn hotword_fetch_client() -> Result<reqwest::Client, String> {
    // Hotword feeds carry no credentials, but the fetch must still not be
    // downgradeable: https_only refuses every http request and redirect
    // target up front.
    reqwest::Client::builder()
        .timeout(FETCH_TIMEOUT)
        .https_only(true)
        .build()
        .map_err(|error| format!("无法创建官方源适配器：{error}"))
}

fn classify_fetch_error(error: &reqwest::Error) -> String {
    if error.is_timeout() {
        "官方源请求超时；已保留上一有效数据。".into()
    } else {
        format!("官方源网络失败：{error}；已保留上一有效数据。")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn path(name: &str) -> PathBuf {
        std::env::temp_dir()
            .join(format!("agent-ime-hotword-tests-{}", std::process::id()))
            .join(format!("{name}.sqlite3"))
    }

    #[test]
    fn hotword_fetch_client_refuses_plain_http_targets_up_front() {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let client = hotword_fetch_client().unwrap();
        let error = tauri::async_runtime::block_on(async {
            client
                .get(format!("http://{address}/feed.json"))
                .send()
                .await
                .expect_err("https-only client must refuse http:// targets")
        });
        assert!(
            !error.is_connect() && !error.is_timeout(),
            "refusal must happen before any network attempt: {error}"
        );
    }

    fn source(id: &str, source_type: &str) -> DynamicSourceInput {
        DynamicSourceInput {
            id: id.into(),
            display_name: "官方测试源".into(),
            source_type: source_type.into(),
            refresh_interval_minutes: 30,
            authorization: "owner opt-in".into(),
            terms_url: "https://example.invalid/terms".into(),
            license: "CC-BY-4.0".into(),
            cache_policy: "last-known-good".into(),
            ownership: "remote_cached".into(),
            distribution: "snapshot-only".into(),
            disable_policy: "pause-and-clear".into(),
            endpoint: "https://example.invalid/feed".into(),
            opt_in: true,
            updated_at_ms: 100,
        }
    }

    #[test]
    fn unlicensed_or_not_opted_in_sources_cannot_be_enabled() {
        let db = path("license");
        let _ = std::fs::remove_file(&db);
        let storage = AppStorage::open(db.clone()).unwrap();
        let mut input = source("blocked", "official_json");
        input.license.clear();
        storage.save_dynamic_source(input).unwrap();
        assert!(storage
            .set_dynamic_source_enabled("blocked", true)
            .unwrap_err()
            .contains("许可证"));
        let mut input = source("opt", "official_json");
        input.opt_in = false;
        storage.save_dynamic_source(input).unwrap();
        assert!(storage
            .set_dynamic_source_enabled("opt", true)
            .unwrap_err()
            .contains("opt-in"));
        drop(storage);
        let _ = std::fs::remove_file(db);
    }

    #[test]
    fn json_xml_encoding_and_bad_payloads_are_deterministic_and_keep_last_good() {
        let db = path("fixtures");
        let _ = std::fs::remove_file(&db);
        let storage = AppStorage::open(db.clone()).unwrap();
        storage
            .save_dynamic_source(source("json", "official_json"))
            .unwrap();
        storage.set_dynamic_source_enabled("json", true).unwrap();
        let good = r#"{"items":[{"text":"Agent输入法","pinyin":"agent shu ru fa","confidenceMillis":920,"sourceRecordId":"a1"},{"text":"待审核","sourceRecordId":"a2"}]}"#.as_bytes();
        let result = storage.ingest_hotword_feed("json", good, 200).unwrap();
        assert_eq!((result.staged_count, result.review_count), (1, 1));
        assert_eq!(storage.list_hotword_items().unwrap().len(), 2);
        assert!(storage.ingest_hotword_feed("json", b"{bad", 300).is_err());
        assert_eq!(
            storage.list_hotword_items().unwrap().len(),
            2,
            "bad refresh must retain last-known-good rows"
        );

        storage
            .save_dynamic_source(source("xml", "official_xml"))
            .unwrap();
        storage.set_dynamic_source_enabled("xml", true).unwrap();
        let xml = br#"<?xml version="1.0"?><feed><item><id>x1</id><text>A&amp;B</text><pinyin>a b</pinyin></item></feed>"#;
        assert_eq!(
            storage
                .ingest_hotword_feed("xml", xml, 400)
                .unwrap()
                .staged_count,
            1
        );
        assert!(storage
            .ingest_hotword_feed("xml", b"<feed><item>", 500)
            .is_err());
        drop(storage);
        let _ = std::fs::remove_file(db);
    }

    #[test]
    fn multi_source_merge_review_expiry_and_clear_are_traceable() {
        let db = path("merge");
        let _ = std::fs::remove_file(&db);
        let storage = AppStorage::open(db.clone()).unwrap();
        for id in ["one", "two"] {
            storage
                .save_dynamic_source(source(id, "official_json"))
                .unwrap();
            storage.set_dynamic_source_enabled(id, true).unwrap();
        }
        let first = r#"{"items":[{"text":"同词","pinyin":"tong ci","confidenceMillis":600,"expiresAtMs":9999,"sourceRecordId":"one"}]}"#.as_bytes();
        let second = r#"{"items":[{"text":"同词","pinyin":"tong ci","confidenceMillis":950,"expiresAtMs":50,"sourceRecordId":"two"}]}"#.as_bytes();
        storage.ingest_hotword_feed("one", first, 100).unwrap();
        storage.ingest_hotword_feed("two", second, 100).unwrap();
        let items = storage.list_hotword_items().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].confidence_millis, 950);
        assert_eq!(items[0].source_ids, ["one", "two"]);
        let predictions = storage.list_prediction_entries(40).unwrap();
        assert_eq!(predictions.len(), 1);
        assert_eq!(predictions[0].text, "同词");
        assert_eq!(predictions[0].sources.len(), 2);
        assert!(predictions[0]
            .sources
            .iter()
            .all(|source| source.source_kind.starts_with("prediction:")));
        assert_eq!(storage.clear_dynamic_source("one").unwrap(), 0);
        assert_eq!(storage.clear_dynamic_source("two").unwrap(), 1);
        drop(storage);
        let _ = std::fs::remove_file(db);
    }

    #[test]
    fn updating_an_enabled_source_with_missing_authorization_disables_it() {
        let db = path("update-policy");
        let _ = std::fs::remove_file(&db);
        let storage = AppStorage::open(db.clone()).unwrap();
        storage
            .save_dynamic_source(source("updated", "official_json"))
            .unwrap();
        assert!(
            storage
                .set_dynamic_source_enabled("updated", true)
                .unwrap()
                .enabled
        );

        let mut stripped = source("updated", "official_json");
        stripped.authorization.clear();
        stripped.updated_at_ms = 200;
        let saved = storage.save_dynamic_source(stripped).unwrap();
        assert!(
            !saved.enabled,
            "an enabled source must be disabled when an update removes authorization"
        );
        assert!(
            !storage
                .list_dynamic_sources()
                .unwrap()
                .into_iter()
                .find(|source| source.id == "updated")
                .unwrap()
                .enabled
        );

        drop(storage);
        let _ = std::fs::remove_file(db);
    }

    #[test]
    fn never_enabled_source_cannot_ingest_or_refresh_and_has_zero_side_effects() {
        let db = path("paused-api");
        let _ = std::fs::remove_file(&db);
        let storage = AppStorage::open(db.clone()).unwrap();
        let mut input = source("paused", "official_json");
        input.endpoint = "https://127.0.0.1:9/should-never-connect".into();
        let saved = storage.save_dynamic_source(input).unwrap();
        assert!(!saved.enabled);

        let feed = br#"{"items":[{"text":"MustNotIngest","pinyin":"must not ingest","sourceRecordId":"paused"}]}"#;
        let ingest_error = storage
            .ingest_hotword_feed("paused", feed, 200)
            .unwrap_err();
        assert!(ingest_error.contains("停用"), "{ingest_error}");
        assert!(storage.list_hotword_items().unwrap().is_empty());

        let refresh_error =
            tauri::async_runtime::block_on(refresh_remote_source(&storage, "paused", 300))
                .unwrap_err();
        assert!(refresh_error.contains("停用"), "{refresh_error}");
        assert!(storage.list_hotword_items().unwrap().is_empty());
        let paused = storage
            .list_dynamic_sources()
            .unwrap()
            .into_iter()
            .find(|source| source.id == "paused")
            .unwrap();
        assert_eq!(paused.last_refresh_ms, None);
        assert_eq!(paused.last_error, None);

        drop(storage);
        let _ = std::fs::remove_file(db);
    }

    fn quality_event(
        id: &str,
        candidates: &[&str],
        selected_text: &str,
        outcome: &str,
    ) -> crate::replay_gate::RecordedQualityEvent {
        // Built through serde rather than a struct literal so the many
        // `#[serde(default)]` fields stay out of the fixture and the test keeps
        // stating only what it actually depends on.
        serde_json::from_value(serde_json::json!({
            "id": id,
            "recordedAtMs": 1_000,
            "pinyin": "ceshi",
            "candidates": candidates,
            "selectedIndex": 0,
            "selectedText": selected_text,
            "appHash": "test-app",
            "outcome": outcome,
        }))
        .unwrap()
    }

    #[test]
    fn exposure_and_misselection_counters_are_recomputed_from_quality_events_and_are_idempotent() {
        let db = path("exposure-counters");
        let _ = std::fs::remove_file(&db);
        let storage = AppStorage::open(db.clone()).unwrap();
        let mut local = source("local", "local_text");
        local.ownership = "user_local".into();
        local.endpoint.clear();
        local.license = "self-authored".into();
        local.terms_url = "self-authored://fixture".into();
        storage.save_dynamic_source(local).unwrap();
        storage.set_dynamic_source_enabled("local", true).unwrap();
        storage
            .ingest_hotword_feed(
                "local",
                "甲词\tjia ci\t释义一\n乙词\tyi ci\t释义二\n".as_bytes(),
                200,
            )
            .unwrap();

        // Before this fix nothing ever wrote these columns, so the panel showed 0 forever.
        let before = storage.list_hotword_items().unwrap();
        assert!(before
            .iter()
            .all(|item| item.exposure_count == 0 && item.misselection_count == 0));

        let events = vec![
            // Both shown, 甲词 chosen: 乙词 was shown and passed over.
            quality_event("e1", &["甲词", "乙词"], "甲词", "committed_candidate"),
            // Only 乙词 shown and chosen.
            quality_event("e2", &["乙词"], "乙词", "committed_candidate"),
            // Abandoned: nothing was chosen, so this is neither exposure nor misselection.
            quality_event("e3", &["甲词", "乙词"], "", "abandoned"),
            // Repeated text inside one candidate list still counts as a single exposure.
            quality_event("e4", &["甲词", "甲词"], "别的词", "committed_candidate"),
        ];

        let report = storage.recompute_hotword_exposure(&events, 400).unwrap();
        assert_eq!(report.considered_events, 3);
        assert_eq!(report.tracked_items, 2);
        assert_eq!(report.changed_items, 2);

        let items = storage.list_hotword_items().unwrap();
        let first = items.iter().find(|item| item.text == "甲词").unwrap();
        let second = items.iter().find(|item| item.text == "乙词").unwrap();
        assert_eq!((first.exposure_count, first.misselection_count), (2, 1));
        assert_eq!((second.exposure_count, second.misselection_count), (2, 1));
        assert_eq!(report.total_exposures, 4);
        assert_eq!(report.total_misselections, 2);

        // Re-running over the same append-only log must not inflate the totals.
        let again = storage.recompute_hotword_exposure(&events, 500).unwrap();
        assert_eq!(again.changed_items, 0);
        assert_eq!(again.total_exposures, 4);
        assert_eq!(again.total_misselections, 2);
        let repeated = storage.list_hotword_items().unwrap();
        let first_again = repeated.iter().find(|item| item.text == "甲词").unwrap();
        assert_eq!(
            (first_again.exposure_count, first_again.misselection_count),
            (2, 1)
        );

        drop(storage);
        let _ = std::fs::remove_file(db);
    }

    #[test]
    fn local_text_parses_tsv_and_legacy_plain_text_but_only_tsv_enters_candidates() {
        let db = path("local-tsv-and-legacy");
        let _ = std::fs::remove_file(&db);
        let storage = AppStorage::open(db.clone()).unwrap();
        let mut local = source("local", "local_text");
        local.ownership = "user_local".into();
        local.endpoint.clear();
        local.license = "self-authored".into();
        local.terms_url = "self-authored://fixture".into();
        storage.save_dynamic_source(local).unwrap();
        storage.set_dynamic_source_enabled("local", true).unwrap();

        let result = storage
            .ingest_hotword_feed(
                "local",
                "表格热词\tbiao ge re ci\t有拼音\n旧格式热词\n".as_bytes(),
                200,
            )
            .unwrap();
        assert_eq!(
            (
                result.received_count,
                result.staged_count,
                result.review_count
            ),
            (2, 1, 1)
        );
        let items = storage.list_hotword_items().unwrap();
        assert_eq!(items.len(), 2);
        let legacy = items.iter().find(|item| item.text == "旧格式热词").unwrap();
        assert!(legacy.pinyin.is_empty());
        assert!(legacy.review_required);
        let candidates = storage.list_prediction_entries(201).unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].text, "表格热词");
        assert_eq!(candidates[0].pinyin, "biao ge re ci");

        let raw = tauri::async_runtime::block_on(refresh_remote_source(&storage, "local", 300))
            .unwrap_err();
        println!("LOCAL-TEXT-NETWORK-REVERSE {raw}");
        assert_eq!(raw, "本地文本来源必须使用本地导入，不允许取网。");
        drop(storage);
        let _ = std::fs::remove_file(db);
    }
}
