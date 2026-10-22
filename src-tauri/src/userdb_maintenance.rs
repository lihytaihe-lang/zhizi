use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::Duration,
};

const HOST_STOP_ATTEMPTS: usize = 20;
const BLACKLIST_FILE_NAME: &str = "agent_personal_blacklist_v1.json";
const BLACKLIST_FORMAT_VERSION: u32 = 1;
const MAX_BLACKLIST_ENTRIES: usize = 4096;
const MAX_BLACKLIST_BYTES: u64 = 1024 * 1024;

unsafe extern "C" {
    fn geteuid() -> u32;
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub id: String,
    pub reading: String,
    pub text: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct BlacklistEntry {
    reading: String,
    text: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct BlacklistDocument {
    format_version: u32,
    entries: Vec<BlacklistEntry>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UserDbBackupItem {
    pub id: String,
    pub text: String,
    pub pinyin: String,
    pub entry_line: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HelperItem {
    id: Option<String>,
    text: String,
    pinyin: String,
    entry_line: String,
}

#[derive(Clone, Debug, Deserialize)]
struct HelperResponse {
    items: Vec<HelperItem>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HelperRequest<'a> {
    id: &'a str,
    text: &'a str,
    pinyin: &'a str,
    entry_line: Option<&'a str>,
}

fn stable_id(text: &str, pinyin: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(b"agent-ime-userdb-item-v1\0");
    digest.update(text.as_bytes());
    digest.update(b"\0");
    digest.update(pinyin.as_bytes());
    format!("userdb:{:x}", digest.finalize())
}

fn blacklist_id(reading: &str, text: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(b"agent-ime-blacklist-item-v1\0");
    digest.update(reading.as_bytes());
    digest.update(b"\0");
    digest.update(text.as_bytes());
    format!("blacklist:{:x}", digest.finalize())
}

fn blacklist_path() -> Result<PathBuf, String> {
    let helper = helper_path()?;
    let (_, user_directory) = runtime_paths(&helper)?;
    Ok(user_directory.join(BLACKLIST_FILE_NAME))
}

fn parse_blacklist(bytes: &[u8]) -> Result<BlacklistDocument, String> {
    let document: BlacklistDocument =
        serde_json::from_slice(bytes).map_err(|error| format!("永久屏蔽列表格式无效：{error}"))?;
    if document.format_version != BLACKLIST_FORMAT_VERSION
        || document.entries.len() > MAX_BLACKLIST_ENTRIES
        || document.entries.iter().any(|entry| {
            entry.reading.is_empty()
                || entry.reading.len() > 256
                || !entry.reading.bytes().all(|byte| byte.is_ascii_lowercase())
                || entry.text.is_empty()
                || entry.text.len() > 1024
        })
    {
        return Err("永久屏蔽列表版本、数量或条目无效。".into());
    }
    Ok(document)
}

fn read_blacklist_document(path: &Path) -> Result<BlacklistDocument, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(BlacklistDocument {
                format_version: BLACKLIST_FORMAT_VERSION,
                entries: Vec::new(),
            });
        }
        Err(error) => return Err(format!("无法读取永久屏蔽列表：{error}")),
    };
    let uid = unsafe { geteuid() };
    if !metadata.file_type().is_file()
        || metadata.uid() != uid
        || metadata.nlink() != 1
        || metadata.mode() & 0o077 != 0
        || metadata.len() > MAX_BLACKLIST_BYTES
    {
        return Err("永久屏蔽列表未通过文件类型、所有者、链接、权限或大小校验。".into());
    }
    let bytes = fs::read(path).map_err(|error| format!("无法读取永久屏蔽列表：{error}"))?;
    parse_blacklist(&bytes)
}

fn write_blacklist_document(path: &Path, document: &BlacklistDocument) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(document)
        .map_err(|error| format!("无法编码永久屏蔽列表：{error}"))?;
    if bytes.len() as u64 > MAX_BLACKLIST_BYTES {
        return Err("永久屏蔽列表超过大小上限。".into());
    }
    let parent = path
        .parent()
        .ok_or_else(|| "永久屏蔽列表路径无效。".to_string())?;
    let metadata = fs::metadata(parent).map_err(|error| format!("永久屏蔽目录不可用：{error}"))?;
    if !metadata.is_dir() || metadata.uid() != unsafe { geteuid() } {
        return Err("永久屏蔽目录不是当前用户拥有的真实目录。".into());
    }
    let temporary = parent.join(format!(
        ".{BLACKLIST_FILE_NAME}.{}.manager.tmp",
        std::process::id()
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)
        .map_err(|error| format!("无法创建永久屏蔽临时文件：{error}"))?;
    let result = (|| -> Result<(), String> {
        file.write_all(&bytes)
            .map_err(|error| format!("无法写入永久屏蔽列表：{error}"))?;
        file.sync_all()
            .map_err(|error| format!("无法同步永久屏蔽列表：{error}"))?;
        fs::rename(&temporary, path)
            .map_err(|error| format!("无法原子替换永久屏蔽列表：{error}"))?;
        fs::File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| format!("无法同步永久屏蔽目录：{error}"))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

pub fn list_blacklist() -> Result<Vec<Item>, String> {
    let document = read_blacklist_document(&blacklist_path()?)?;
    Ok(document
        .entries
        .into_iter()
        .map(|entry| Item {
            id: blacklist_id(&entry.reading, &entry.text),
            reading: entry.reading,
            text: entry.text,
        })
        .collect())
}

pub fn remove_blacklist(ids: &[String]) -> Result<usize, String> {
    if ids.is_empty() {
        return Ok(0);
    }
    let helper = helper_path()?;
    stop_matching_host(&helper)?;
    let path = blacklist_path()?;
    let mut document = read_blacklist_document(&path)?;
    let before = document.entries.len();
    document
        .entries
        .retain(|entry| !ids.contains(&blacklist_id(&entry.reading, &entry.text)));
    let removed = before.saturating_sub(document.entries.len());
    if removed != 0 {
        write_blacklist_document(&path, &document)?;
    }
    Ok(removed)
}

fn helper_path() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("AGENT_IME_USERDB_HELPER") {
        let path = PathBuf::from(path);
        return path
            .is_file()
            .then_some(path)
            .ok_or_else(|| "AGENT_IME_USERDB_HELPER 指向的文件不存在。".into());
    }
    let home = std::env::var_os("HOME").ok_or_else(|| "无法确定用户目录。".to_string())?;
    let names = ["Agent Input Method.app", "Agent Input Method.inputmethod"];
    let mut candidates = Vec::new();
    for name in names {
        candidates.push(
            PathBuf::from(&home)
                .join("Library/Input Methods")
                .join(name)
                .join("Contents/MacOS/AgentIMEUserDBCtl"),
        );
        candidates.push(
            PathBuf::from("/Library/Input Methods")
                .join(name)
                .join("Contents/MacOS/AgentIMEUserDBCtl"),
        );
    }
    if cfg!(debug_assertions) {
        for configuration in ["debug", "release"] {
            candidates.push(
                std::env::current_dir()
                    .unwrap_or_default()
                    .join(format!(
                        "build/native/{configuration}/Agent Input Method.inputmethod/Contents/MacOS/AgentIMEUserDBCtl"
                    )),
            );
        }
    }
    candidates
        .into_iter()
        .find(|path| path.is_file())
        .ok_or_else(|| "未找到随输入法安装的 AgentIMEUserDBCtl；UserDB 维护已阻止。".into())
}

fn runtime_paths(helper: &Path) -> Result<(PathBuf, PathBuf), String> {
    let contents = helper
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| "UserDB helper bundle 路径无效。".to_string())?;
    let shared = contents.join("Resources/Rime");
    if !shared.is_dir() {
        return Err("UserDB helper 缺少同包 Rime 资源。".into());
    }
    let home = std::env::var_os("HOME").ok_or_else(|| "无法确定用户目录。".to_string())?;
    Ok((
        shared,
        PathBuf::from(home).join("Library/Application Support/AgentInputMethod/Rime"),
    ))
}

fn running_host_pids() -> Result<Vec<u32>, String> {
    let uid = Command::new("/usr/bin/id")
        .arg("-u")
        .output()
        .map_err(|error| format!("无法读取当前用户 ID：{error}"))?;
    if !uid.status.success() {
        return Err("无法读取当前用户 ID。".into());
    }
    let uid = String::from_utf8_lossy(&uid.stdout).trim().to_string();
    let output = Command::new("/usr/bin/pgrep")
        .args(["-U", &uid, "-x", "AgentInputMethod"])
        .output()
        .map_err(|error| format!("无法查询输入法进程：{error}"))?;
    if output.status.code() == Some(1) {
        return Ok(Vec::new());
    }
    if !output.status.success() {
        return Err("无法查询输入法进程。".into());
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| {
            line.trim()
                .parse::<u32>()
                .map_err(|_| "输入法进程 ID 无效。".to_string())
        })
        .collect()
}

fn stop_matching_host(helper: &Path) -> Result<(), String> {
    if std::env::var_os("AGENT_IME_USERDB_HELPER").is_some() {
        return Ok(());
    }
    let expected = helper
        .parent()
        .ok_or_else(|| "UserDB helper 路径无效。".to_string())?
        .join("AgentInputMethod")
        .canonicalize()
        .map_err(|error| format!("无法确认输入法宿主路径：{error}"))?;
    let pids = running_host_pids()?;
    for pid in &pids {
        let output = Command::new("/bin/ps")
            .args(["-p", &pid.to_string(), "-o", "comm="])
            .output()
            .map_err(|error| format!("无法确认输入法进程 {pid}：{error}"))?;
        let actual = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
        let actual = actual
            .canonicalize()
            .map_err(|error| format!("无法确认输入法进程 {pid} 路径：{error}"))?;
        if actual != expected {
            return Err(format!("拒绝停止非同包输入法进程：{pid}。"));
        }
    }
    for pid in pids {
        let status = Command::new("/bin/kill")
            .args(["-TERM", &pid.to_string()])
            .status()
            .map_err(|error| format!("无法停止输入法进程 {pid}：{error}"))?;
        if !status.success() {
            return Err(format!("输入法进程 {pid} 未接受维护停止请求。"));
        }
    }
    for _ in 0..HOST_STOP_ATTEMPTS {
        if running_host_pids()?.is_empty() {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(50));
    }
    Err("输入法宿主未在 1 秒内退出，UserDB 维护已阻止。".into())
}

fn invoke(operation: &str, request: Option<String>) -> Result<Vec<HelperItem>, String> {
    let helper = helper_path()?;
    let (shared, user) = runtime_paths(&helper)?;
    stop_matching_host(&helper)?;
    let mut command = Command::new(&helper);
    command.arg(operation).arg(shared).arg(user);
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    if request.is_some() {
        command.stdin(Stdio::piped());
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("无法运行 UserDB helper：{error}"))?;
    if let Some(request) = request {
        let write_result = child
            .stdin
            .take()
            .ok_or_else(|| "无法打开 UserDB helper 请求管道。".to_string())
            .and_then(|mut stdin| {
                stdin
                    .write_all(request.as_bytes())
                    .map_err(|error| format!("无法发送 UserDB helper 请求：{error}"))
            });
        if let Err(error) = write_result {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
    }
    let output = child
        .wait_with_output()
        .map_err(|error| format!("无法等待 UserDB helper：{error}"))?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if detail.is_empty() {
            "UserDB helper 执行失败。".into()
        } else {
            format!("UserDB helper 执行失败：{detail}")
        });
    }
    serde_json::from_slice::<HelperResponse>(&output.stdout)
        .map(|response| response.items)
        .map_err(|error| format!("UserDB helper 返回无效：{error}"))
}

pub fn list() -> Result<Vec<UserDbBackupItem>, String> {
    invoke("list", None)?
        .into_iter()
        .map(|item| {
            let id = stable_id(&item.text, &item.pinyin);
            Ok(UserDbBackupItem {
                id,
                text: item.text,
                pinyin: item.pinyin,
                entry_line: item.entry_line,
            })
        })
        .collect()
}

pub fn delete(items: &[UserDbBackupItem]) -> Result<Vec<UserDbBackupItem>, String> {
    let request = items
        .iter()
        .map(|item| HelperRequest {
            id: &item.id,
            text: &item.text,
            pinyin: &item.pinyin,
            entry_line: None,
        })
        .collect::<Vec<_>>();
    let document = serde_json::to_string(&request)
        .map_err(|error| format!("无法编码 UserDB 删除请求：{error}"))?;
    invoke("delete", Some(document))?
        .into_iter()
        .map(|item| {
            let id = item
                .id
                .ok_or_else(|| "UserDB helper 未返回条目 ID。".to_string())?;
            Ok(UserDbBackupItem {
                id,
                text: item.text,
                pinyin: item.pinyin,
                entry_line: item.entry_line,
            })
        })
        .collect()
}

pub fn restore(items: &[UserDbBackupItem]) -> Result<(), String> {
    if items.is_empty() {
        return Ok(());
    }
    let request = items
        .iter()
        .map(|item| HelperRequest {
            id: &item.id,
            text: &item.text,
            pinyin: &item.pinyin,
            entry_line: Some(&item.entry_line),
        })
        .collect::<Vec<_>>();
    let document = serde_json::to_string(&request)
        .map_err(|error| format!("无法编码 UserDB 恢复请求：{error}"))?;
    let restored = invoke("restore", Some(document))?;
    if restored.len() != items.len()
        || restored.iter().zip(items).any(|(actual, expected)| {
            actual.id.as_deref() != Some(expected.id.as_str())
                || actual.text != expected.text
                || actual.pinyin != expected.pinyin
        })
    {
        return Err("UserDB helper 恢复结果与请求不一致。".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_ids_bind_text_and_pinyin_without_learning_counts() {
        assert_eq!(stable_id("所义", "suo yi"), stable_id("所义", "suo yi"));
        assert_ne!(stable_id("所义", "suo yi"), stable_id("所以", "suo yi"));
        assert_ne!(stable_id("所义", "suo yi"), stable_id("所义", "suo yì"));
        assert!(stable_id("所义", "suo yi").starts_with("userdb:"));
    }

    #[test]
    fn blacklist_document_is_strict_and_ids_bind_reading_plus_text() {
        let document = parse_blacklist(
            r#"{"formatVersion":1,"entries":[{"reading":"zhge","text":"栅格"}]}"#.as_bytes(),
        )
        .unwrap();
        assert_eq!(document.entries.len(), 1);
        assert_eq!(blacklist_id("zhge", "栅格"), blacklist_id("zhge", "栅格"));
        assert_ne!(blacklist_id("zhge", "栅格"), blacklist_id("shange", "栅格"));
        assert!(parse_blacklist(
            r#"{"formatVersion":1,"entries":[{"reading":"zhge","text":"栅格","count":32}]}"#
                .as_bytes()
        )
        .is_err());
    }
}
