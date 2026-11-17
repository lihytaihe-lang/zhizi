//! Shared bounded-line snapshot reader for manager records and the replay helper.
use crate::replay_gate::RecordedQualityEvent;
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::{
    fs::{self, File},
    io::{BufRead, BufReader, Read, Take},
    path::Path,
};
const MAXIMUM_LINE_BYTES: u64 = 1024 * 1024;

pub(crate) struct Record {
    pub event: RecordedQualityEvent,
    pub(crate) raw: Vec<u8>,
    pub(crate) start: u64,
    pub repaired_candidate_count: bool,
}

pub(crate) struct Records {
    pub(crate) input: Option<BufReader<Take<File>>>,
    pub(crate) identity: String,
    offset: u64,
    strict_tail: bool,
    done: bool,
}

pub(crate) fn identity(metadata: &fs::Metadata) -> String {
    #[cfg(unix)]
    {
        format!("{}:{}", metadata.dev(), metadata.ino())
    }
    #[cfg(not(unix))]
    {
        format!("{:?}", metadata.created().ok())
    }
}

pub(crate) fn records(
    directory: &Path,
    cursor: Option<&str>,
    strict_tail: bool,
) -> Result<Records, String> {
    let path = directory.join("events.jsonl");
    let before = match fs::symlink_metadata(&path) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && cursor.is_none() => {
            return Ok(Records {
                input: None,
                identity: String::new(),
                offset: 0,
                strict_tail,
                done: true,
            });
        }
        Err(error) => return Err(format!("无法读取记录，请刷新后重试：{error}")),
    };
    if !before.is_file() || before.file_type().is_symlink() {
        return Err("质量记录不是安全的本地普通文件。".into());
    }
    let file = File::open(&path).map_err(|error| format!("无法打开质量记录：{error}"))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("无法检查质量记录：{error}"))?;
    if !metadata.is_file() || identity(&metadata) != identity(&before) {
        return Err("质量记录已被替换，请刷新后重试。".into());
    }
    let file_identity = identity(&metadata);
    let length = if let Some(cursor) = cursor {
        let (expected, offset) = cursor.rsplit_once(':').ok_or("记录页游标无效。")?;
        let offset = offset.parse::<u64>().map_err(|_| "记录页游标无效。")?;
        if expected != file_identity || offset > metadata.len() {
            return Err("记录已被删除或替换，请返回最近记录。".into());
        }
        offset
    } else {
        metadata.len()
    };
    Ok(Records {
        input: Some(BufReader::new(file.take(length))),
        identity: file_identity,
        offset: 0,
        strict_tail,
        done: false,
    })
}

impl Iterator for Records {
    type Item = Result<Record, String>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        let mut raw = Vec::new();
        let read = self
            .input
            .as_mut()?
            .by_ref()
            .take(MAXIMUM_LINE_BYTES + 1)
            .read_until(b'\n', &mut raw);
        match read {
            Err(error) => {
                self.done = true;
                Some(Err(format!("无法读取质量记录：{error}")))
            }
            Ok(0) => {
                self.done = true;
                None
            }
            Ok(_) if raw.len() as u64 > MAXIMUM_LINE_BYTES => {
                self.done = true;
                Some(Err("单条质量记录超过 1 MiB 上限。".into()))
            }
            Ok(_) => {
                let start = self.offset;
                self.offset += raw.len() as u64;
                match serde_json::from_slice::<RecordedQualityEvent>(&raw) {
                    Ok(mut event) => {
                        // Older writers appended a selected candidate without updating
                        // this diagnostic total. Infer its lower bound from the same
                        // unchanged page; retain raw bytes and all answer fields.
                        let repaired_candidate_count = event.candidate_count > 0
                            && event.candidate_count < event.candidates.len();
                        if repaired_candidate_count {
                            event.candidate_count = event.candidates.len();
                        }
                        Some(Ok(Record {
                            event,
                            raw,
                            start,
                            repaired_candidate_count,
                        }))
                    }
                    Err(error) if !self.strict_tail && !raw.ends_with(b"\n") && error.is_eof() => {
                        // A concurrent native append may be partway through its last line.
                        self.done = true;
                        None
                    }
                    Err(_) => {
                        self.done = true;
                        Some(Err("质量记录包含无效行，原文件未修改。".into()))
                    }
                }
            }
        }
    }
}
