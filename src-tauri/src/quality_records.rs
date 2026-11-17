//! Streaming access to the append-only quality log. Pages retain at most 200 rows;
//! rewrite/export preserve the original JSON rather than reserializing old schemas.
use crate::replay_gate::{CompositionOutcome, RecordedQualityEvent};
use serde::{Deserialize, Serialize};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::{
    collections::VecDeque,
    fs::{self, File, OpenOptions},
    io::Write,
    path::Path,
    sync::atomic::Ordering,
};

const WEEK_MS: i64 = 7 * 24 * 60 * 60 * 1000;

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Filter {
    pub raw_only: Option<bool>,
    pub app_hash: Option<String>,
    pub start_ms: Option<i64>,
    pub end_ms: Option<i64>,
}

impl Filter {
    fn matches(&self, event: &RecordedQualityEvent) -> bool {
        (!self.raw_only.unwrap_or(false) || event.selected_text == event.pinyin)
            && self
                .app_hash
                .as_ref()
                .is_none_or(|hash| *hash == event.app_hash)
            && self
                .start_ms
                .is_none_or(|start| event.recorded_at_ms >= start)
            && self.end_ms.is_none_or(|end| event.recorded_at_ms <= end)
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Page {
    pub records: Vec<RecordedQualityEvent>,
    pub next_cursor: Option<String>,
}

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Overview {
    pub total_count: usize,
    pub commits: usize,
    pub first_choice_rate: Option<f64>,
    pub redo_rate: Option<f64>,
}

pub(crate) use crate::quality_log::{identity, records};

pub(crate) fn page(
    directory: &Path,
    cursor: Option<&str>,
    limit: usize,
    filter: &Filter,
) -> Result<Page, String> {
    let limit = limit.clamp(1, 200);
    let mut reader = records(directory, cursor, false)?;
    let mut tail = VecDeque::with_capacity(limit);
    let mut matched = 0;
    for record in reader.by_ref() {
        let record = record?;
        if filter.matches(&record.event) {
            matched += 1;
            if tail.len() == limit {
                tail.pop_front();
            }
            tail.push_back(record);
        }
    }
    let next_cursor =
        (matched > tail.len()).then(|| format!("{}:{}", reader.identity, tail[0].start));
    Ok(Page {
        records: tail.into_iter().rev().map(|record| record.event).collect(),
        next_cursor,
    })
}

pub(crate) fn count(directory: &Path, filter: &Filter) -> Result<usize, String> {
    records(directory, None, false)?.try_fold(0, |count, record| {
        Ok(count + usize::from(filter.matches(&record?.event)))
    })
}

pub(crate) fn overview(directory: &Path, now_ms: i64) -> Result<Overview, String> {
    let mut result = Overview::default();
    let (mut first, mut redo) = (0, 0);
    for record in records(directory, None, false)? {
        let event = record?.event;
        result.total_count += 1;
        if event.recorded_at_ms >= now_ms.saturating_sub(WEEK_MS)
            && event.outcome == CompositionOutcome::CommittedCandidate
            && !event.selected_text.is_empty()
        {
            result.commits += 1;
            first += usize::from(event.selected_index == 0);
            redo += usize::from(event.redo_after_commit);
        }
    }
    if result.commits > 0 {
        result.first_choice_rate = Some(first as f64 / result.commits as f64);
        result.redo_rate = Some(redo as f64 / result.commits as f64);
    }
    Ok(result)
}

/// Caller holds the same .quality.lock used by the native append path.
pub(crate) fn rewrite_locked(
    directory: &Path,
    mut delete: impl FnMut(&RecordedQualityEvent) -> bool,
) -> Result<usize, String> {
    let reader = records(directory, None, true)?;
    if reader.input.is_none() {
        return Ok(0);
    }
    let source_identity = reader.identity.clone();
    let sequence = crate::QUALITY_TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary = directory.join(format!(
        ".events-rewrite-{}-{sequence}.tmp",
        std::process::id()
    ));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut output = options
            .open(&temporary)
            .map_err(|error| format!("无法创建记录临时文件：{error}"))?;
        let (mut deleted, mut retained) = (0, 0);
        for record in reader {
            let record = record?;
            if delete(&record.event) {
                deleted += 1;
            } else {
                output
                    .write_all(&record.raw)
                    .map_err(|error| error.to_string())?;
                if !record.raw.ends_with(b"\n") {
                    output.write_all(b"\n").map_err(|error| error.to_string())?;
                }
                retained += 1;
            }
        }
        if deleted == 0 {
            return Ok(0);
        }
        output.sync_all().map_err(|error| error.to_string())?;
        drop(output);
        let source = directory.join("events.jsonl");
        let current = fs::symlink_metadata(&source).map_err(|error| error.to_string())?;
        if !current.is_file()
            || current.file_type().is_symlink()
            || identity(&current) != source_identity
        {
            return Err("质量记录在删除期间已被替换，未写回。".into());
        }
        if retained == 0 {
            fs::remove_file(source).map_err(|error| error.to_string())?;
        } else {
            fs::rename(&temporary, source).map_err(|error| error.to_string())?;
        }
        File::open(directory)
            .and_then(|file| file.sync_all())
            .map_err(|error| error.to_string())?;
        Ok(deleted)
    })();
    let _ = fs::remove_file(temporary);
    result
}

pub(crate) fn export_json(directory: &Path, output: &mut impl Write) -> Result<usize, String> {
    output
        .write_all(b"[\n")
        .map_err(|error| error.to_string())?;
    let mut count = 0;
    for record in records(directory, None, false)? {
        let record = record?;
        if count > 0 {
            output
                .write_all(b",\n")
                .map_err(|error| error.to_string())?;
        }
        output
            .write_all(&record.raw)
            .map_err(|error| error.to_string())?;
        count += 1;
    }
    output
        .write_all(b"\n]\n")
        .map_err(|error| error.to_string())?;
    Ok(count)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExportResult {
    pub path: String,
    pub count: usize,
}

pub(crate) fn export_file(directory: &Path, destination: &Path) -> Result<ExportResult, String> {
    let parent = destination.parent().ok_or("导出位置无效。")?;
    let source = directory.join("events.jsonl");
    if destination == source || parent.canonicalize().ok() == directory.canonicalize().ok() {
        return Err("请选择记录目录以外的导出位置。".into());
    }
    if let Ok(metadata) = fs::symlink_metadata(destination) {
        if !metadata.is_file()
            || metadata.file_type().is_symlink()
            || fs::metadata(&source).is_ok_and(|source| identity(&source) == identity(&metadata))
        {
            return Err("导出目标不能是原始记录或链接。".into());
        }
    }
    let sequence = crate::QUALITY_TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary = parent.join(format!(
        ".quality-export-{}-{sequence}.tmp",
        std::process::id()
    ));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut output = options
            .open(&temporary)
            .map_err(|error| format!("无法创建导出文件：{error}"))?;
        let count = export_json(directory, &mut output)?;
        output.sync_all().map_err(|error| error.to_string())?;
        drop(output);
        fs::rename(&temporary, destination)
            .map_err(|error| format!("无法保存导出文件：{error}"))?;
        File::open(parent)
            .and_then(|file| file.sync_all())
            .map_err(|error| error.to_string())?;
        Ok(ExportResult {
            path: destination.to_string_lossy().into_owned(),
            count,
        })
    })();
    let _ = fs::remove_file(temporary);
    result
}

/// Compatibility for older small-log callers; the UI uses export_file.
pub(crate) fn export_string(directory: &Path) -> Result<String, String> {
    struct Limited(Vec<u8>);
    impl Write for Limited {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if self.0.len().saturating_add(bytes.len()) > 32 * 1024 * 1024 {
                return Err(std::io::Error::other("记录较大，请使用文件导出。"));
            }
            self.0.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut output = Limited(Vec::new());
    export_json(directory, &mut output)?;
    String::from_utf8(output.0).map_err(|_| "记录不是有效 UTF-8。".into())
}

#[cfg(test)]
#[path = "quality_records_tests.rs"]
mod tests;
