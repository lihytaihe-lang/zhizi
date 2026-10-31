//! 管理台的词库包下载与安装：输入法核心进程不联网，下载只发生在这里。
//! 内容只允许来自钉死的 GitHub Release 地址，sha256 校验通过后原子装入 Rime 用户目录。

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs;
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    LazyLock, Mutex,
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const DEFAULT_LEXICON_TAG: &str = "v2026.10";
const PRODUCTION_BASE_URL: &str =
    "https://github.com/lihytaihe-lang/zhizi-lexicon/releases/download";
// GitHub 的 Release 下载会 302 到后两个资产主机。
const ALLOWED_HTTPS_HOSTS: [&str; 3] = [
    "github.com",
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
];
const MAX_REDIRECTS: usize = 5;
const INDEX_BYTES_LIMIT: u64 = 256 * 1024;
const DICT_BYTES_LIMIT: u64 = 64 * 1024 * 1024;
const MEMBERSHIP_BYTES_LIMIT: u64 = 32 * 1024 * 1024;
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(600);
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackIndex {
    pub format_version: i64,
    pub lexicon_version: String,
    pub packs: Vec<PackEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackEntry {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub rows: u64,
    pub dict: PackFile,
    pub membership: PackFile,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackFile {
    pub file: String,
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledPack {
    pub id: String,
    pub membership_bytes: u64,
    pub dict_present: bool,
}

/// 下载来源。生产入口只用 `production_source()`；`allow_http_loopback`
/// 只允许测试构造的来源访问本机回环地址。
pub struct Source {
    base_url: String,
    allow_http_loopback: bool,
}

fn production_source() -> Source {
    Source {
        base_url: PRODUCTION_BASE_URL.into(),
        allow_http_loopback: false,
    }
}

struct DownloadedAsset {
    bytes: u64,
    sha256: String,
}

fn valid_pack_id(id: &str) -> bool {
    (1..=48).contains(&id.len())
        && id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

fn validate_index(index: &PackIndex) -> Result<(), String> {
    if index.format_version != 1 {
        return Err("包索引 formatVersion 必须是 1。".into());
    }
    for pack in &index.packs {
        if !valid_pack_id(&pack.id) {
            return Err("包索引里的包 ID 无效。".into());
        }
        if pack.dict.file != format!("{}.dict.yaml", pack.id)
            || pack.membership.file != format!("{}.membership.tsv", pack.id)
        {
            return Err(format!("包 {} 的文件名不符合约定。", pack.id));
        }
        if !valid_sha256(&pack.dict.sha256) || !valid_sha256(&pack.membership.sha256) {
            return Err(format!("包 {} 的 sha256 无效。", pack.id));
        }
    }
    Ok(())
}

fn asset_url(source: &Source, tag: &str, file: &str) -> Result<String, String> {
    let tag = tag.trim();
    if tag.is_empty()
        || tag.contains("..")
        || !tag.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || byte == b'.' || byte == b'_' || byte == b'-'
        })
    {
        return Err("词库 Release 标签无效。".into());
    }
    Ok(format!(
        "{}/{tag}/{file}",
        source.base_url.trim_end_matches('/')
    ))
}

fn url_is_allowed(url: &reqwest::Url, allow_http_loopback: bool) -> bool {
    let host = url.host_str().unwrap_or_default();
    if url.scheme() == "https" {
        return ALLOWED_HTTPS_HOSTS.contains(&host);
    }
    if allow_http_loopback && url.scheme() == "http" {
        let host = host.trim_matches(|c| c == '[' || c == ']');
        return host.eq_ignore_ascii_case("localhost")
            || host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback());
    }
    false
}

fn redirect_policy(allow_http_loopback: bool) -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(move |attempt| {
        if attempt.previous().len() > MAX_REDIRECTS {
            return attempt.error("词库下载重定向次数过多。");
        }
        if !url_is_allowed(attempt.url(), allow_http_loopback) {
            return attempt.error("词库下载重定向到了不允许的地址。");
        }
        attempt.follow()
    })
}

fn describe_transport_error(label: &str, error: &reqwest::Error) -> String {
    let mut message = error.to_string();
    let mut source = std::error::Error::source(error);
    while let Some(cause) = source {
        message.push_str("：");
        message.push_str(&cause.to_string());
        source = cause.source();
    }
    format!("{label}网络失败：{message}")
}

async fn get_verified_response(
    source: &Source,
    url: &str,
    label: &str,
) -> Result<reqwest::Response, String> {
    let url = reqwest::Url::parse(url).map_err(|_| format!("{label}地址无效。"))?;
    if !url_is_allowed(&url, source.allow_http_loopback) {
        return Err(format!("{label}地址不在允许的来源内。"));
    }
    let client = reqwest::Client::builder()
        .timeout(DOWNLOAD_TIMEOUT)
        .redirect(redirect_policy(source.allow_http_loopback))
        .build()
        .map_err(|error| format!("无法初始化词库下载客户端：{error}"))?;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|error| describe_transport_error(label, &error))?;
    if !url_is_allowed(response.url(), source.allow_http_loopback) {
        return Err(format!("{label}最终地址不在允许的来源内。"));
    }
    if !response.status().is_success() {
        return Err(format!("{label}返回 HTTP {}。", response.status()));
    }
    Ok(response)
}

async fn download_bytes(
    source: &Source,
    url: &str,
    limit: u64,
    label: &str,
) -> Result<Vec<u8>, String> {
    let mut response = get_verified_response(source, url, label).await?;
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| describe_transport_error(label, &error))?
    {
        if body.len() as u64 + chunk.len() as u64 > limit {
            return Err(format!("{label}超过大小上限。"));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

async fn download_to_file(
    source: &Source,
    url: &str,
    limit: u64,
    path: &Path,
    label: &str,
) -> Result<DownloadedAsset, String> {
    let result = async {
        let mut response = get_verified_response(source, url, label).await?;
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options
            .open(path)
            .map_err(|error| format!("无法创建{label}临时文件：{error}"))?;
        let mut hasher = Sha256::new();
        let mut total: u64 = 0;
        loop {
            let Some(chunk) = response
                .chunk()
                .await
                .map_err(|error| describe_transport_error(label, &error))?
            else {
                break;
            };
            total += chunk.len() as u64;
            if total > limit {
                return Err(format!("{label}超过大小上限。"));
            }
            hasher.update(&chunk);
            file.write_all(&chunk)
                .map_err(|error| format!("无法写入{label}：{error}"))?;
        }
        file.sync_all()
            .map_err(|error| format!("无法持久化{label}：{error}"))?;
        Ok(DownloadedAsset {
            bytes: total,
            sha256: format!("{:x}", hasher.finalize()),
        })
    }
    .await;
    if result.is_err() {
        let _ = fs::remove_file(path);
    }
    result
}

fn temp_path(directory: &Path, file_name: &str) -> PathBuf {
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    directory.join(format!(
        ".{file_name}.{}-{sequence}.tmp",
        std::process::id()
    ))
}

#[cfg(unix)]
extern "C" {
    fn getuid() -> u32;
}

fn validate_rime_user_dir(path: &Path) -> Result<(), String> {
    let metadata =
        fs::symlink_metadata(path).map_err(|error| format!("无法检查 Rime 用户目录：{error}"))?;
    if metadata.file_type().is_symlink() {
        return Err("Rime 用户目录不能是符号链接。".into());
    }
    if !metadata.is_dir() {
        return Err("Rime 用户目录不是普通目录。".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != unsafe { getuid() } {
            return Err("Rime 用户目录不属于当前用户。".into());
        }
    }
    Ok(())
}

/// 返回 true 表示目录是本次创建的，安装失败时应连同清理。
fn ensure_packs_directory(directory: &Path) -> Result<bool, String> {
    let mut created = false;
    match fs::symlink_metadata(directory) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(directory).map_err(|error| format!("无法创建词库包目录：{error}"))?;
            created = true;
        }
        Err(error) => return Err(format!("无法检查词库包目录：{error}")),
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                return Err("词库包目录不能是符号链接。".into());
            }
            if !metadata.is_dir() {
                return Err("词库包目录不是普通目录。".into());
            }
        }
    }
    #[cfg(unix)]
    fs::set_permissions(directory, fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("无法设置词库包目录权限：{error}"))?;
    Ok(created)
}

fn unix_seconds() -> Result<u64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| "系统时间无效。".to_string())
}

/// 写临时文件已在下载阶段完成；这里只负责备份旧文件并原子换入。成功时
/// 返回被挪走的备份路径（如有），供失败时回滚。
fn replace_file(temp: &Path, target: &Path, label: &str) -> Result<Option<PathBuf>, String> {
    let backup = match fs::symlink_metadata(target) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                return Err(format!("{label}目标位置是符号链接，已拒绝安装。"));
            }
            if !metadata.is_file() {
                return Err(format!("{label}目标位置不是普通文件，已拒绝安装。"));
            }
            let name = target
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .ok_or_else(|| format!("{label}目标文件名无效。"))?;
            let backup = target.with_file_name(format!("{}.bak-{}", name, unix_seconds()?));
            fs::rename(target, &backup).map_err(|error| format!("无法备份旧{label}：{error}"))?;
            Some(backup)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(format!("无法检查旧{label}：{error}")),
    };
    match fs::rename(temp, target) {
        Ok(()) => Ok(backup),
        Err(error) => {
            if let Some(backup) = &backup {
                let _ = fs::rename(backup, target);
            }
            Err(format!("无法安装{label}：{error}"))
        }
    }
}

fn undo_install(target: &Path, backup: Option<PathBuf>) -> Result<(), String> {
    match backup {
        Some(backup) => {
            fs::rename(&backup, target).map_err(|error| format!("无法恢复旧成员表：{error}"))
        }
        None => match fs::remove_file(target) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!("无法撤销成员表安装：{error}")),
        },
    }
}

fn prune_old_backups(directory: &Path, name: &str) {
    let prefix = format!("{name}.bak-");
    let mut backups = Vec::<(u64, PathBuf)>::new();
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let Some(stamp) = entry
            .file_name()
            .to_str()
            .and_then(|file| file.strip_prefix(&prefix))
            .and_then(|stamp| stamp.parse::<u64>().ok())
        else {
            continue;
        };
        backups.push((stamp, entry.path()));
    }
    if backups.len() < 2 {
        return;
    }
    backups.sort_by_key(|(stamp, _)| *stamp);
    for (_, path) in &backups[..backups.len() - 1] {
        let _ = fs::remove_file(path);
    }
}

fn sync_directory(directory: &Path) {
    if let Ok(handle) = fs::File::open(directory) {
        let _ = handle.sync_all();
    }
}

fn verify_asset(asset: &DownloadedAsset, expected: &PackFile, label: &str) -> Result<(), String> {
    if asset.bytes != expected.bytes || asset.sha256 != expected.sha256 {
        return Err(format!("{label}校验失败：sha256 或大小与索引不符。"));
    }
    Ok(())
}

pub async fn fetch_index(source: &Source, tag: &str) -> Result<PackIndex, String> {
    let url = asset_url(source, tag, "packs.json")?;
    let bytes = download_bytes(source, &url, INDEX_BYTES_LIMIT, "包索引").await?;
    let index: PackIndex =
        serde_json::from_slice(&bytes).map_err(|_| "包索引不是有效的 packs.json。".to_string())?;
    validate_index(&index)?;
    Ok(index)
}

pub async fn install_pack(
    source: &Source,
    tag: &str,
    index: &PackIndex,
    id: &str,
    rime_user_dir: &Path,
) -> Result<InstalledPack, String> {
    validate_index(index)?;
    if !valid_pack_id(id) {
        return Err("词库包 ID 无效。".into());
    }
    let Some(pack) = index.packs.iter().find(|pack| pack.id == id) else {
        return Err("词库索引里没有这个包。".into());
    };
    validate_rime_user_dir(rime_user_dir)?;
    let membership_url = asset_url(source, tag, &pack.membership.file)?;
    let dict_url = asset_url(source, tag, &pack.dict.file)?;
    let packs_directory = rime_user_dir.join("lexicon_packs");
    let membership_temp = temp_path(rime_user_dir, &pack.membership.file);
    let dict_temp = temp_path(rime_user_dir, &pack.dict.file);
    let dict_target = rime_user_dir.join(&pack.dict.file);
    let mut created_packs_directory = false;
    let installed = async {
        let membership = download_to_file(
            source,
            &membership_url,
            MEMBERSHIP_BYTES_LIMIT,
            &membership_temp,
            "成员表",
        )
        .await?;
        let dict =
            download_to_file(source, &dict_url, DICT_BYTES_LIMIT, &dict_temp, "词典").await?;
        verify_asset(&membership, &pack.membership, "成员表")?;
        verify_asset(&dict, &pack.dict, "词典")?;
        created_packs_directory = ensure_packs_directory(&packs_directory)?;
        let membership_target = packs_directory.join(&pack.membership.file);
        let membership_backup = replace_file(&membership_temp, &membership_target, "成员表")?;
        // 成员表必须先于词典就位：词典出现时核心才能把它算进包层。
        if let Err(error) = replace_file(&dict_temp, &dict_target, "词典") {
            if let Err(undo_error) = undo_install(&membership_target, membership_backup) {
                return Err(format!("{error}；且撤销成员表失败：{undo_error}"));
            }
            return Err(error);
        }
        prune_old_backups(&packs_directory, &pack.membership.file);
        prune_old_backups(rime_user_dir, &pack.dict.file);
        sync_directory(&packs_directory);
        sync_directory(rime_user_dir);
        Ok(InstalledPack {
            id: id.to_string(),
            membership_bytes: membership.bytes,
            dict_present: true,
        })
    }
    .await;
    if installed.is_err() {
        let _ = fs::remove_file(&membership_temp);
        let _ = fs::remove_file(&dict_temp);
        if created_packs_directory {
            let _ = fs::remove_dir(&packs_directory);
        }
    }
    installed
}

pub fn remove_pack(id: &str, rime_user_dir: &Path) -> Result<(), String> {
    if !valid_pack_id(id) {
        return Err("词库包 ID 无效。".into());
    }
    let paths = [
        rime_user_dir.join(format!("{id}.dict.yaml")),
        rime_user_dir
            .join("lexicon_packs")
            .join(format!("{id}.membership.tsv")),
    ];
    for path in paths {
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("无法删除词库包文件：{error}")),
        }
    }
    Ok(())
}

pub fn installed_packs(rime_user_dir: &Path) -> Vec<InstalledPack> {
    let Ok(entries) = fs::read_dir(rime_user_dir.join("lexicon_packs")) else {
        return Vec::new();
    };
    let mut packs = Vec::new();
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let Some(id) = file_name
            .to_str()
            .and_then(|name| name.strip_suffix(".membership.tsv"))
        else {
            continue;
        };
        if !valid_pack_id(id) {
            continue;
        }
        let Ok(metadata) = fs::symlink_metadata(entry.path()) else {
            continue;
        };
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            continue;
        }
        let dict_present = fs::symlink_metadata(rime_user_dir.join(format!("{id}.dict.yaml")))
            .is_ok_and(|metadata| metadata.is_file());
        packs.push(InstalledPack {
            id: id.to_string(),
            membership_bytes: metadata.len(),
            dict_present,
        });
    }
    packs.sort_by(|left, right| left.id.cmp(&right.id));
    packs
}

#[tauri::command]
pub async fn lexicon_packs_available(tag: Option<String>) -> Result<PackIndex, String> {
    fetch_index(
        &production_source(),
        tag.as_deref().unwrap_or(DEFAULT_LEXICON_TAG),
    )
    .await
}

/// 正在安装或移除的词库包：同一包的请求互斥，不同包互不阻塞。
static IN_FLIGHT_PACKS: LazyLock<Mutex<HashSet<String>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

struct PackOperationGuard(String);

impl Drop for PackOperationGuard {
    fn drop(&mut self) {
        if let Ok(mut in_flight) = IN_FLIGHT_PACKS.lock() {
            in_flight.remove(&self.0);
        }
    }
}

fn reserve_pack_operation(id: &str) -> Result<PackOperationGuard, String> {
    let mut in_flight = IN_FLIGHT_PACKS
        .lock()
        .map_err(|_| "词库包状态锁已损坏。".to_string())?;
    if in_flight.contains(id) {
        return Err("该词库包正在处理中".to_string());
    }
    in_flight.insert(id.to_string());
    Ok(PackOperationGuard(id.to_string()))
}

/// 安装全程（含取索引）持有同一包的占用，避免同包并发安装或边装边删。
async fn install_pack_tracked(
    source: &Source,
    tag: &str,
    id: &str,
    rime_user_dir: &Path,
) -> Result<InstalledPack, String> {
    let _guard = reserve_pack_operation(id)?;
    let index = fetch_index(source, tag).await?;
    install_pack(source, tag, &index, id, rime_user_dir).await
}

fn remove_pack_tracked(id: &str, rime_user_dir: &Path) -> Result<Vec<InstalledPack>, String> {
    let _guard = reserve_pack_operation(id)?;
    remove_pack(id, rime_user_dir)?;
    Ok(installed_packs(rime_user_dir))
}

#[tauri::command]
pub async fn lexicon_pack_install(
    tag: Option<String>,
    id: String,
) -> Result<InstalledPack, String> {
    // The index carries the sha256 of every file, so it is fetched here and never taken from the caller.
    let source = production_source();
    let tag = tag.as_deref().unwrap_or(DEFAULT_LEXICON_TAG);
    let directory = crate::user_rime_directory()?;
    install_pack_tracked(&source, tag, &id, &directory).await
}

#[tauri::command]
pub fn lexicon_packs_installed() -> Result<Vec<InstalledPack>, String> {
    Ok(installed_packs(&crate::user_rime_directory()?))
}

#[tauri::command]
pub async fn lexicon_pack_remove(id: String) -> Result<Vec<InstalledPack>, String> {
    let directory = crate::user_rime_directory()?;
    remove_pack_tracked(&id, &directory)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Read;
    use std::net::{TcpListener, TcpStream};
    use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    enum Served {
        Bytes(Vec<u8>),
        Redirect(&'static str),
        Delayed(u64, Vec<u8>),
    }

    struct TestServer {
        base_url: String,
        shutdown: Arc<AtomicBool>,
    }

    impl Drop for TestServer {
        fn drop(&mut self) {
            self.shutdown.store(true, AtomicOrdering::Relaxed);
        }
    }

    fn serve_request(stream: &mut TcpStream, routes: &[(&'static str, Served)]) {
        let mut buffer = [0_u8; 8192];
        let mut seen = Vec::new();
        loop {
            match stream.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => {
                    seen.extend_from_slice(&buffer[..count]);
                    if seen.windows(4).any(|part| part == b"\r\n\r\n") {
                        break;
                    }
                }
                Err(_) => return,
            }
        }
        let head = String::from_utf8_lossy(&seen);
        let path = head
            .split_whitespace()
            .nth(1)
            .unwrap_or_default()
            .split('?')
            .next()
            .unwrap_or_default();
        let Some((_, served)) = routes.iter().find(|(route, _)| *route == path) else {
            let _ = stream.write_all(
                b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
            return;
        };
        let response = match served {
            Served::Bytes(body) => http_200_response(body),
            Served::Delayed(millis, body) => {
                thread::sleep(Duration::from_millis(*millis));
                http_200_response(body)
            }
            Served::Redirect(location) => format!(
                "HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            )
            .into_bytes(),
        };
        let _ = stream.write_all(&response);
    }

    fn http_200_response(body: &[u8]) -> Vec<u8> {
        let mut response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .into_bytes();
        response.extend_from_slice(body);
        response
    }

    fn spawn_server(routes: Vec<(&'static str, Served)>) -> TestServer {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind loopback lexicon server");
        listener
            .set_nonblocking(true)
            .expect("nonblocking listener");
        let address = listener.local_addr().expect("loopback address");
        let shutdown = Arc::new(AtomicBool::new(false));
        let flag = shutdown.clone();
        thread::spawn(move || {
            while !flag.load(AtomicOrdering::Relaxed) {
                let Ok((mut stream, _)) = listener.accept() else {
                    thread::sleep(Duration::from_millis(5));
                    continue;
                };
                let _ = stream.set_nonblocking(false);
                let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));
                serve_request(&mut stream, &routes);
            }
        });
        TestServer {
            base_url: format!("http://{address}"),
            shutdown,
        }
    }

    fn temporary_user_dir(name: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "agent-ime-lexicon-download-tests-{}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir(&directory).expect("create temporary user dir");
        directory
    }

    fn list_all(directory: &Path) -> Vec<String> {
        fn walk(directory: &Path, prefix: String, out: &mut Vec<String>) {
            for entry in fs::read_dir(directory).expect("read directory").flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
                    out.push(format!("{prefix}{name}/"));
                    walk(&entry.path(), format!("{prefix}{name}/"), out);
                } else {
                    out.push(format!("{prefix}{name}"));
                }
            }
        }
        let mut out = Vec::new();
        walk(directory, String::new(), &mut out);
        out.sort();
        out
    }

    fn backups_of(directory: &Path, name: &str) -> Vec<PathBuf> {
        let prefix = format!("{name}.bak-");
        let mut out: Vec<PathBuf> = fs::read_dir(directory)
            .expect("read directory")
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(&prefix))
            })
            .collect();
        out.sort();
        out
    }

    fn sha256_hex(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    fn pack_entry(id: &str, dict: &[u8], membership: &[u8]) -> PackEntry {
        PackEntry {
            id: id.into(),
            name: "测试包".into(),
            description: String::new(),
            rows: 1,
            dict: PackFile {
                file: format!("{id}.dict.yaml"),
                sha256: sha256_hex(dict),
                bytes: dict.len() as u64,
            },
            membership: PackFile {
                file: format!("{id}.membership.tsv"),
                sha256: sha256_hex(membership),
                bytes: membership.len() as u64,
            },
        }
    }

    fn index_for(pack: PackEntry) -> PackIndex {
        PackIndex {
            format_version: 1,
            lexicon_version: "2026.10".into(),
            packs: vec![pack],
        }
    }

    fn loopback_source(base_url: &str) -> Source {
        Source {
            base_url: base_url.into(),
            allow_http_loopback: true,
        }
    }

    #[cfg(unix)]
    fn mode_of(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    const ID: &str = "agent_pack_poetry";
    const DICT: &[u8] = b"# Rime dictionary\n...\n";
    const MEMBERSHIP: &[u8] = b"\xe5\x93\x80\xe6\xb1\x9f\xe5\xa4\xb4\tagent_pack_poetry\n";

    fn standard_routes() -> Vec<(&'static str, Served)> {
        vec![
            (
                "/vTEST/agent_pack_poetry.dict.yaml",
                Served::Bytes(DICT.to_vec()),
            ),
            (
                "/vTEST/agent_pack_poetry.membership.tsv",
                Served::Bytes(MEMBERSHIP.to_vec()),
            ),
        ]
    }

    fn install(
        server: &TestServer,
        directory: &Path,
        index: &PackIndex,
    ) -> Result<InstalledPack, String> {
        tauri::async_runtime::block_on(install_pack(
            &loopback_source(&server.base_url),
            "vTEST",
            index,
            ID,
            directory,
        ))
    }

    #[test]
    fn install_pack_writes_files_with_private_modes_and_keeps_one_backup() {
        let server = spawn_server(standard_routes());
        let directory = temporary_user_dir("install-ok");
        let index = index_for(pack_entry(ID, DICT, MEMBERSHIP));

        let installed = install(&server, &directory, &index).unwrap();
        assert_eq!(installed.id, ID);
        assert!(installed.dict_present);
        assert_eq!(
            fs::read(directory.join(format!("{ID}.dict.yaml"))).unwrap(),
            DICT
        );
        assert_eq!(
            fs::read(
                directory
                    .join("lexicon_packs")
                    .join(format!("{ID}.membership.tsv"))
            )
            .unwrap(),
            MEMBERSHIP
        );
        #[cfg(unix)]
        {
            assert_eq!(mode_of(&directory.join(format!("{ID}.dict.yaml"))), 0o600);
            assert_eq!(
                mode_of(
                    &directory
                        .join("lexicon_packs")
                        .join(format!("{ID}.membership.tsv"))
                ),
                0o600
            );
            assert_eq!(mode_of(&directory.join("lexicon_packs")), 0o700);
        }
        assert_eq!(
            installed_packs(&directory),
            vec![InstalledPack {
                id: ID.into(),
                membership_bytes: MEMBERSHIP.len() as u64,
                dict_present: true,
            }]
        );
        assert!(backups_of(&directory, &format!("{ID}.dict.yaml")).is_empty());
        assert!(backups_of(
            &directory.join("lexicon_packs"),
            &format!("{ID}.membership.tsv")
        )
        .is_empty());

        // Second install over existing files: old files become the single kept backup.
        fs::write(directory.join(format!("{ID}.dict.yaml.bak-1")), b"stale").unwrap();
        fs::write(
            directory
                .join("lexicon_packs")
                .join(format!("{ID}.membership.tsv.bak-1")),
            b"stale",
        )
        .unwrap();
        install(&server, &directory, &index).unwrap();
        let dict_backups = backups_of(&directory, &format!("{ID}.dict.yaml"));
        assert_eq!(dict_backups.len(), 1);
        assert_eq!(fs::read(&dict_backups[0]).unwrap(), DICT);
        let membership_backups = backups_of(
            &directory.join("lexicon_packs"),
            &format!("{ID}.membership.tsv"),
        );
        assert_eq!(membership_backups.len(), 1);
        assert_eq!(fs::read(&membership_backups[0]).unwrap(), MEMBERSHIP);

        let _ = fs::remove_dir_all(&directory);
    }

    #[test]
    fn checksum_or_size_mismatch_installs_nothing() {
        let server = spawn_server(standard_routes());
        let directory = temporary_user_dir("install-mismatch");

        let mut index = index_for(pack_entry(ID, DICT, MEMBERSHIP));
        index.packs[0].membership.sha256 = sha256_hex(b"tampered");
        let error = install(&server, &directory, &index).unwrap_err();
        assert!(error.contains("校验失败"), "{error}");
        assert!(
            list_all(&directory).is_empty(),
            "{:?}",
            list_all(&directory)
        );

        let mut index = index_for(pack_entry(ID, DICT, MEMBERSHIP));
        index.packs[0].dict.bytes += 1;
        let error = install(&server, &directory, &index).unwrap_err();
        assert!(error.contains("校验失败"), "{error}");
        assert!(
            list_all(&directory).is_empty(),
            "{:?}",
            list_all(&directory)
        );

        let _ = fs::remove_dir_all(&directory);
    }

    #[test]
    fn oversized_membership_aborts_without_residue() {
        let oversized = vec![b'x'; MEMBERSHIP_BYTES_LIMIT as usize + 1];
        let mut index = index_for(pack_entry(ID, DICT, &oversized));
        index.packs[0].membership.sha256 = sha256_hex(&oversized);
        index.packs[0].membership.bytes = oversized.len() as u64;
        let server = spawn_server(vec![
            (
                "/vTEST/agent_pack_poetry.dict.yaml",
                Served::Bytes(DICT.to_vec()),
            ),
            (
                "/vTEST/agent_pack_poetry.membership.tsv",
                Served::Bytes(oversized),
            ),
        ]);
        let directory = temporary_user_dir("install-oversize");
        let error = install(&server, &directory, &index).unwrap_err();
        assert!(error.contains("超过大小上限"), "{error}");
        assert!(
            list_all(&directory).is_empty(),
            "{:?}",
            list_all(&directory)
        );

        let _ = fs::remove_dir_all(&directory);
    }

    #[test]
    fn dict_failure_rolls_back_membership_install() {
        let server = spawn_server(standard_routes());
        let index = index_for(pack_entry(ID, DICT, MEMBERSHIP));

        // A directory squatting on the dict target makes the second step fail.
        let directory = temporary_user_dir("install-rollback");
        fs::create_dir(directory.join(format!("{ID}.dict.yaml"))).unwrap();
        let error = install(&server, &directory, &index).unwrap_err();
        assert!(error.contains("不是普通文件"), "{error}");
        assert_eq!(
            list_all(&directory),
            vec![format!("{ID}.dict.yaml/")],
            "{:?}",
            list_all(&directory)
        );

        // With a pre-existing membership the rollback restores the old content.
        let directory = temporary_user_dir("install-rollback-restore");
        fs::create_dir(directory.join("lexicon_packs")).unwrap();
        fs::write(
            directory
                .join("lexicon_packs")
                .join(format!("{ID}.membership.tsv")),
            b"old members",
        )
        .unwrap();
        fs::create_dir(directory.join(format!("{ID}.dict.yaml"))).unwrap();
        let error = install(&server, &directory, &index).unwrap_err();
        assert!(error.contains("不是普通文件"), "{error}");
        assert_eq!(
            fs::read(
                directory
                    .join("lexicon_packs")
                    .join(format!("{ID}.membership.tsv"))
            )
            .unwrap(),
            b"old members"
        );
        assert!(backups_of(
            &directory.join("lexicon_packs"),
            &format!("{ID}.membership.tsv")
        )
        .is_empty());

        let _ = fs::remove_dir_all(&directory);
        let _ = fs::remove_dir_all(std::env::temp_dir().join(format!(
            "agent-ime-lexicon-download-tests-{}-install-rollback",
            std::process::id()
        )));
    }

    #[test]
    fn remove_pack_clears_both_files_and_installed_packs_reflects_state() {
        let server = spawn_server(standard_routes());
        let directory = temporary_user_dir("remove");
        let index = index_for(pack_entry(ID, DICT, MEMBERSHIP));
        install(&server, &directory, &index).unwrap();

        fs::remove_file(directory.join(format!("{ID}.dict.yaml"))).unwrap();
        assert_eq!(
            installed_packs(&directory),
            vec![InstalledPack {
                id: ID.into(),
                membership_bytes: MEMBERSHIP.len() as u64,
                dict_present: false,
            }]
        );

        remove_pack(ID, &directory).unwrap();
        assert!(!directory.join(format!("{ID}.dict.yaml")).exists());
        assert!(!directory
            .join("lexicon_packs")
            .join(format!("{ID}.membership.tsv"))
            .exists());
        assert!(installed_packs(&directory).is_empty());

        // Removing again is not an error; unknown ids are fine too.
        remove_pack(ID, &directory).unwrap();
        remove_pack("agent_pack_other", &directory).unwrap();
        assert!(remove_pack("bad/id", &directory).is_err());

        let _ = fs::remove_dir_all(&directory);
    }

    #[test]
    fn fetch_index_validates_format_ids_file_names_and_hashes() {
        let sha = sha256_hex(b"abc");
        let valid = json!({
            "id": ID, "name": "诗词", "rows": 14880,
            "dict": { "file": format!("{ID}.dict.yaml"), "sha256": sha, "bytes": 3 },
            "membership": { "file": format!("{ID}.membership.tsv"), "sha256": sha, "bytes": 3 },
        });
        let index_with = |pack: &serde_json::Value, format_version: i64| {
            json!({
                "formatVersion": format_version,
                "lexiconVersion": "2026.10",
                "packs": [pack],
            })
            .to_string()
        };
        let bad_version =
            json!({ "formatVersion": 2, "lexiconVersion": "2026.10", "packs": [valid.clone()] })
                .to_string();
        let mut traversal = valid.clone();
        traversal["dict"]["file"] = json!("../../evil.dict.yaml");
        let traversal = index_with(&traversal, 1);
        let mut bad_id = valid.clone();
        bad_id["id"] = json!("Agent-Pack");
        let bad_id = index_with(&bad_id, 1);
        let mut bad_sha = valid.clone();
        bad_sha["membership"]["sha256"] = json!("A".repeat(64));
        let bad_sha = index_with(&bad_sha, 1);
        let server = spawn_server(vec![
            (
                "/ok/packs.json",
                Served::Bytes(index_with(&valid, 1).into_bytes()),
            ),
            (
                "/version/packs.json",
                Served::Bytes(bad_version.into_bytes()),
            ),
            (
                "/traversal/packs.json",
                Served::Bytes(traversal.into_bytes()),
            ),
            ("/badid/packs.json", Served::Bytes(bad_id.into_bytes())),
            ("/badsha/packs.json", Served::Bytes(bad_sha.into_bytes())),
            (
                "/toolarge/packs.json",
                Served::Bytes(vec![b' '; INDEX_BYTES_LIMIT as usize + 1]),
            ),
        ]);
        let source = loopback_source(&server.base_url);

        let index = tauri::async_runtime::block_on(fetch_index(&source, "ok")).unwrap();
        assert_eq!(index.format_version, 1);
        assert_eq!(index.lexicon_version, "2026.10");
        assert_eq!(index.packs.len(), 1);
        assert_eq!(index.packs[0].id, ID);

        for (tag, expected) in [
            ("version", "formatVersion"),
            ("traversal", "文件名"),
            ("badid", "ID"),
            ("badsha", "sha256"),
            ("toolarge", "超过大小上限"),
        ] {
            let error = tauri::async_runtime::block_on(fetch_index(&source, tag)).unwrap_err();
            assert!(error.contains(expected), "{tag}: {error}");
        }

        let error = tauri::async_runtime::block_on(fetch_index(&source, "../evil")).unwrap_err();
        assert!(error.contains("标签"), "{error}");
    }

    #[test]
    fn redirects_are_confined_to_allowed_hosts() {
        let sha = sha256_hex(b"abc");
        let valid = json!({
            "formatVersion": 1, "lexiconVersion": "2026.10",
            "packs": [{
                "id": ID, "name": "诗词", "rows": 1,
                "dict": { "file": format!("{ID}.dict.yaml"), "sha256": sha, "bytes": 3 },
                "membership": { "file": format!("{ID}.membership.tsv"), "sha256": sha, "bytes": 3 },
            }]
        })
        .to_string();
        let server = spawn_server(vec![
            ("/hop/packs.json", Served::Redirect("/landing/packs.json")),
            ("/landing/packs.json", Served::Bytes(valid.into_bytes())),
            (
                "/away/packs.json",
                Served::Redirect("http://example.invalid/x"),
            ),
        ]);
        let source = loopback_source(&server.base_url);

        let index = tauri::async_runtime::block_on(fetch_index(&source, "hop")).unwrap();
        assert_eq!(index.packs.len(), 1);

        let error = tauri::async_runtime::block_on(fetch_index(&source, "away")).unwrap_err();
        assert!(error.contains("不允许"), "{error}");

        // A production source never accepts loopback http, even before connecting.
        let production_like = Source {
            base_url: format!("{}/x", server.base_url),
            allow_http_loopback: false,
        };
        let error =
            tauri::async_runtime::block_on(fetch_index(&production_like, "ok")).unwrap_err();
        assert!(error.contains("不在允许"), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_user_directory_is_rejected() {
        let server = spawn_server(standard_routes());
        let real = temporary_user_dir("symlink-real");
        let link = std::env::temp_dir().join(format!(
            "agent-ime-lexicon-download-tests-{}-symlink-link",
            std::process::id()
        ));
        let _ = fs::remove_file(&link);
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let index = index_for(pack_entry(ID, DICT, MEMBERSHIP));
        let error = install(&server, &link, &index).unwrap_err();
        assert!(error.contains("符号链接"), "{error}");
        assert!(list_all(&real).is_empty(), "{:?}", list_all(&real));
        let _ = fs::remove_file(&link);
        let _ = fs::remove_dir_all(&real);
    }

    /// The in-flight set is process-wide and tests run in parallel: the two tests that hold `ID` take turns.
    static IN_FLIGHT_TESTS: Mutex<()> = Mutex::new(());

    #[test]
    fn same_pack_install_and_remove_are_mutually_exclusive() {
        let _turn = IN_FLIGHT_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let guard = reserve_pack_operation(ID).unwrap();
        let server = spawn_server(standard_routes());
        let directory = temporary_user_dir("mutex-guard");
        let index = index_for(pack_entry(ID, DICT, MEMBERSHIP));

        let error = tauri::async_runtime::block_on(install_pack_tracked(
            &loopback_source(&server.base_url),
            "vTEST",
            ID,
            &directory,
        ))
        .unwrap_err();
        assert!(error.contains("该词库包正在处理中"), "{error}");
        let error = remove_pack_tracked(ID, &directory).unwrap_err();
        assert!(error.contains("该词库包正在处理中"), "{error}");

        // 不同包之间互不阻塞。
        let other_guard = reserve_pack_operation("agent_pack_other").unwrap();
        drop(other_guard);
        drop(guard);

        // 占用结束后，同一包可以正常安装。
        install(&server, &directory, &index).unwrap();
        let _ = fs::remove_dir_all(&directory);
    }

    #[test]
    fn second_install_of_a_pack_in_flight_is_rejected() {
        let _turn = IN_FLIGHT_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let index_body = serde_json::to_vec(&index_for(pack_entry(ID, DICT, MEMBERSHIP))).unwrap();
        let server = spawn_server(vec![
            ("/vTEST/packs.json", Served::Bytes(index_body)),
            (
                "/vTEST/agent_pack_poetry.dict.yaml",
                Served::Delayed(800, DICT.to_vec()),
            ),
            (
                "/vTEST/agent_pack_poetry.membership.tsv",
                Served::Bytes(MEMBERSHIP.to_vec()),
            ),
        ]);
        let worker_directory = temporary_user_dir("mutex-concurrent");
        let base_url = server.base_url.clone();
        let worker = thread::spawn(move || {
            tauri::async_runtime::block_on(install_pack_tracked(
                &loopback_source(&base_url),
                "vTEST",
                ID,
                &worker_directory,
            ))
        });
        // 等第一个安装确实占住这个包（而不是睡一个固定时长：整轮回归时机器很忙，200 ms 不够）。
        let started = std::time::Instant::now();
        while !IN_FLIGHT_PACKS.lock().unwrap().contains(ID) {
            assert!(started.elapsed() < Duration::from_secs(10), "first install never started");
            thread::sleep(Duration::from_millis(5));
        }
        let error = tauri::async_runtime::block_on(install_pack_tracked(
            &loopback_source(&server.base_url),
            "vTEST",
            ID,
            &temporary_user_dir("mutex-concurrent-b"),
        ))
        .unwrap_err();
        assert!(error.contains("该词库包正在处理中"), "{error}");
        worker.join().unwrap().unwrap();
        let _ = fs::remove_dir_all(std::env::temp_dir().join(format!(
            "agent-ime-lexicon-download-tests-{}-mutex-concurrent",
            std::process::id()
        )));
        let _ = fs::remove_dir_all(std::env::temp_dir().join(format!(
            "agent-ime-lexicon-download-tests-{}-mutex-concurrent-b",
            std::process::id()
        )));
    }
}
