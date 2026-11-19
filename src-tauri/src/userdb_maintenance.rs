//! Current native learning management; all edits share the core's file lock.
//! The legacy UserDbBackupItem name is retained for existing SQLite undo batches.
use agent_ime_core::{blacklist, zhixu::learning::{self, LearningWordBackup, ManagementKey}};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{io, path::{Path, PathBuf}};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Item { pub id: String, pub reading: String, pub text: String }

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UserDbBackupItem { pub id: String, pub text: String, pub pinyin: String, pub entry_line: String }

fn stable_id(text: &str, pinyin: &str) -> String {
    let mut digest = Sha256::new(); digest.update(b"agent-ime-userdb-item-v1\0");
    digest.update(text.as_bytes()); digest.update(b"\0"); digest.update(pinyin.as_bytes());
    format!("userdb:{:x}", digest.finalize())
}
fn blacklist_id(reading: &str, text: &str) -> String {
    let mut digest = Sha256::new(); digest.update(b"agent-ime-blacklist-item-v1\0");
    digest.update(reading.as_bytes()); digest.update(b"\0"); digest.update(text.as_bytes());
    format!("blacklist:{:x}", digest.finalize())
}
pub fn runtime_directory() -> Result<PathBuf, String> {
    #[cfg(target_os = "macos")]
    { Ok(PathBuf::from(std::env::var_os("HOME").ok_or("无法确定本地用户目录。")?)
        .join("Library/Application Support/AgentInputMethod/Rime")) }
    #[cfg(not(target_os = "macos"))]
    { Err("此管理台尚未配置当前系统的输入法学习目录。".into()) }
}
fn explain(error: io::Error) -> String {
    let detail = match error.kind() {
        io::ErrorKind::WouldBlock => "输入法正在保存，请稍后重试",
        io::ErrorKind::StorageFull => "合并将超过学习容量，现有记录未改动",
        io::ErrorKind::NotFound => "所选记录已变化，请刷新后重试",
        io::ErrorKind::InvalidData | io::ErrorKind::PermissionDenied => "学习文件、权限或备份校验失败，现有记录未改动",
        _ => "本地学习操作未完成",
    };
    format!("{detail}：{error}")
}
fn backup_item(word: LearningWordBackup) -> Result<UserDbBackupItem, String> {
    Ok(UserDbBackupItem { id: stable_id(&word.key.text, &word.key.reading),
        text: word.key.text.clone(), pinyin: word.key.reading.clone(),
        entry_line: serde_json::to_string(&word).map_err(|e| e.to_string())? })
}
fn keys(items: &[UserDbBackupItem]) -> Result<Vec<ManagementKey>, String> {
    items.iter().map(|item| {
        if item.id != stable_id(&item.text, &item.pinyin) { return Err("学习条目 ID 与内容不一致。".into()); }
        Ok(ManagementKey { reading: item.pinyin.clone(), text: item.text.clone() })
    }).collect()
}
fn list_at(directory: &Path) -> Result<Vec<UserDbBackupItem>, String> {
    learning::management_list(directory).map_err(explain)?.items.into_iter().map(backup_item).collect()
}
fn delete_at(directory: &Path, items: &[UserDbBackupItem]) -> Result<Vec<UserDbBackupItem>, String> {
    if items.is_empty() { return Ok(Vec::new()); }
    learning::management_delete(directory, &keys(items)?).map_err(explain)?
        .into_iter().map(backup_item).collect()
}
fn restore_at(directory: &Path, items: &[UserDbBackupItem]) -> Result<(), String> {
    if items.is_empty() { return Ok(()); }
    let expected = keys(items)?;
    let words = items.iter().zip(expected).map(|(item, expected)| {
        let word: LearningWordBackup = serde_json::from_str(&item.entry_line)
            .map_err(|_| "该撤销批次属于旧版 Rime 格式，不能写入当前学习文件。".to_string())?;
        if word.key != expected { return Err("学习恢复内容与所选条目不一致。".into()); }
        Ok(word)
    }).collect::<Result<Vec<_>, String>>()?;
    learning::management_restore(directory, &words).map(|_| ()).map_err(explain)
}
pub fn list() -> Result<Vec<UserDbBackupItem>, String> { list_at(&runtime_directory()?) }
pub fn delete(items: &[UserDbBackupItem]) -> Result<Vec<UserDbBackupItem>, String> { delete_at(&runtime_directory()?, items) }
pub fn delete_journaled(items: &[UserDbBackupItem], journal: impl FnOnce(Vec<UserDbBackupItem>) -> Result<(), String>) -> Result<(), String> {
    learning::management_delete_journaled(&runtime_directory()?, &keys(items)?, |words| {
        let payload = words.iter().cloned().map(backup_item).collect::<Result<Vec<_>, _>>().map_err(io::Error::other)?;
        journal(payload).map_err(io::Error::other)
    }).map(|_| ()).map_err(explain)
}
pub fn restore(items: &[UserDbBackupItem]) -> Result<(), String> { restore_at(&runtime_directory()?, items) }
pub fn status() -> Result<learning::LearningStatus, String> {
    Ok(learning::management_list(&runtime_directory()?).map_err(explain)?.status)
}
pub fn export_learning() -> Result<String, String> { learning::management_export(&runtime_directory()?).map_err(explain) }
pub fn preview_learning(document: &str) -> Result<learning::LearningImportPreview, String> {
    learning::management_preview(&runtime_directory()?, document).map_err(explain)
}
pub fn import_learning(document: &str) -> Result<learning::LearningImportPreview, String> {
    learning::management_import(&runtime_directory()?, document).map_err(explain)
}
fn list_blacklist_at(directory: &Path) -> Result<Vec<Item>, String> {
    Ok(blacklist::managed_list(directory).map_err(explain)?.into_iter().map(|e|
        Item { id: blacklist_id(&e.reading, &e.text), reading: e.reading, text: e.text }).collect())
}
fn remove_blacklist_at(directory: &Path, ids: &[String]) -> Result<usize, String> {
    if ids.is_empty() { return Ok(0); }
    let all = list_blacklist_at(directory)?;
    let selected = ids.iter().map(|id| all.iter().find(|item| &item.id == id)
        .map(|e| blacklist::BlacklistEntry { reading: e.reading.clone(), text: e.text.clone() })
        .ok_or_else(|| "永久屏蔽记录已变化，请刷新后重试。".to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    blacklist::managed_remove(directory, &selected).map_err(explain)
}
pub fn list_blacklist() -> Result<Vec<Item>, String> { list_blacklist_at(&runtime_directory()?) }
pub fn remove_blacklist(ids: &[String]) -> Result<usize, String> { remove_blacklist_at(&runtime_directory()?, ids) }

#[cfg(test)]
mod tests {
    use super::*;
    use agent_ime_core::zhixu::learning::LearningStore;
    use std::{fs, os::unix::fs::PermissionsExt, sync::atomic::{AtomicU64, Ordering}};
    struct Scratch(PathBuf);
    impl Scratch {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!("native-management-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            fs::create_dir(&path).unwrap(); fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap(); Self(path)
        }
        fn seed(&self) {
            let mut store = LearningStore::load(&self.0, 100);
            store.record("ni hao", "你好", 100); store.record("zhong guo", "中国", 101);
            store.save(&self.0).unwrap();
        }
    }
    impl Drop for Scratch { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); } }
    #[test]
    fn current_store_list_delete_undo_keeps_peer_learning() {
        let dir = Scratch::new(); dir.seed(); let all = list_at(&dir.0).unwrap(); assert_eq!(all.len(), 2);
        let deleted = delete_at(&dir.0, &all[..1]).unwrap(); assert_eq!(list_at(&dir.0).unwrap().len(), 1);
        let mut peer = LearningStore::load(&dir.0, 102); peer.record("wo", "我", 102); peer.save(&dir.0).unwrap();
        restore_at(&dir.0, &deleted).unwrap(); restore_at(&dir.0, &deleted).unwrap();
        assert_eq!(list_at(&dir.0).unwrap().len(), 3);
        let bytes = fs::read(dir.0.join(learning::FILE_NAME)).unwrap();
        let mut bad = deleted; bad[0].entry_line = "legacy Rime row".into();
        assert!(restore_at(&dir.0, &bad).is_err()); assert_eq!(bytes, fs::read(dir.0.join(learning::FILE_NAME)).unwrap());
    }
    #[test]
    fn stale_or_tampered_selection_is_rejected_without_data_loss() {
        let dir = Scratch::new(); dir.seed(); let mut items = list_at(&dir.0).unwrap();
        let bytes = fs::read(dir.0.join(learning::FILE_NAME)).unwrap(); items[0].text = "替换".into();
        assert!(delete_at(&dir.0, &items).is_err()); assert_eq!(bytes, fs::read(dir.0.join(learning::FILE_NAME)).unwrap());
        assert!(list_blacklist_at(&dir.0).unwrap().is_empty()); assert_eq!(delete_at(&dir.0, &[]).unwrap().len(), 0);
    }
    #[test]
    fn current_blacklist_lists_and_removes_without_retired_helper() {
        let dir = Scratch::new(); let path = dir.0.join("agent_personal_blacklist_v1.json");
        fs::write(&path, r#"{"formatVersion":1,"entries":[{"reading":"nihao","text":"你好"},{"reading":"wo","text":"我"}]}"#).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let listed = list_blacklist_at(&dir.0).unwrap(); assert_eq!(listed.len(), 2);
        assert_eq!(remove_blacklist_at(&dir.0, &[listed[0].id.clone()]).unwrap(), 1);
        assert_eq!(list_blacklist_at(&dir.0).unwrap().len(), 1);
        assert!(remove_blacklist_at(&dir.0, &[listed[0].id.clone()]).is_err());
        assert_eq!(list_blacklist_at(&dir.0).unwrap().len(), 1);
    }
}
