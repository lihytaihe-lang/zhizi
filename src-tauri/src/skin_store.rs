use crate::skin_packages::{self, CandidateSkinCatalogSnapshot};
use crate::storage::AppStorage;
use protocol::{IndexSkin, SkinIndex};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

static SEQUENCE: AtomicU64 = AtomicU64::new(0);
const PUBLIC_KEY: &str = include_str!("../resources/skin_index_pubkey.hex");
const MAX_CACHE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SkinStoreSettings {
    pub index_url: String,
    pub last_refreshed_at_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StoreSkin {
    pub id: String,
    pub name: protocol::SkinName,
    pub author: String,
    pub description: String,
    pub version: String,
    pub tier: String,
    pub platforms: Vec<String>,
    pub available_for_sale: bool,
    pub product_id: String,
    pub animated: bool,
    pub bundle_bytes: u64,
    pub min_app_version: String,
    pub updated_at: String,
    pub license: String,
    pub changelog: String,
    pub state: String,
    pub installed_version: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StoreSnapshot {
    pub skins: Vec<StoreSkin>,
    pub refreshed_at_ms: Option<i64>,
    pub stale: bool,
    pub error: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CachedIndex {
    source_url: String,
    refreshed_at_ms: i64,
    document: String,
    signature: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InstalledVersion {
    version: String,
    manifest_sha256: String,
}

pub struct StoreBackend {
    data_dir: PathBuf,
    skins_root: PathBuf,
    key_hex: &'static str,
    last_error: Mutex<Option<(String, String)>>,
    mutation: Mutex<()>,
}

impl StoreBackend {
    pub fn enrich_catalog(&self, mut catalog: CandidateSkinCatalogSnapshot) -> CandidateSkinCatalogSnapshot {
        let versions: BTreeMap<String, InstalledVersion> = read_bounded(&self.versions_path(), 128 * 1024)
            .ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default();
        for entry in &mut catalog.skins {
            if entry.origin != "external" || entry.status != "ready" { continue; }
            if let Some(record) = versions.get(&entry.id).filter(|record| {
                protocol::compare_versions(&record.version, &record.version).is_ok()
                    && read_bounded(&self.skins_root.join(&entry.id).join("skin.json"), skin_packages::MAXIMUM_SKIN_JSON_BYTES as usize)
                        .is_ok_and(|bytes| protocol::sha256_hex(&bytes) == record.manifest_sha256)
            }) { entry.version = Some(record.version.clone()); }
        }
        catalog
    }

    pub fn new(data_dir: PathBuf, skins_root: PathBuf) -> Self {
        Self {
            data_dir,
            skins_root,
            key_hex: PUBLIC_KEY,
            last_error: Mutex::new(None),
            mutation: Mutex::new(()),
        }
    }

    fn configured_url(storage: &AppStorage) -> Result<String, String> {
        let connection = storage.lock_connection()?;
        let raw: Option<String> = connection
            .query_row(
                "SELECT value FROM settings WHERE key = 'skinStore'",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| "无法读取皮肤商店设置。".to_string())?;
        let settings: SkinStoreSettings = raw
            .map(|raw| {
                serde_json::from_str(&raw)
                    .map_err(|_| "皮肤商店设置已损坏，请重新设置地址。".to_string())
            })
            .transpose()?
            .unwrap_or_default();
        if !settings.index_url.is_empty() {
            protocol::validate_download_url(&settings.index_url)?;
        }
        Ok(settings.index_url)
    }

    pub fn settings(&self, storage: &AppStorage) -> Result<SkinStoreSettings, String> {
        let index_url = Self::configured_url(storage)?;
        let last_refreshed_at_ms = self.cached(&index_url).map(|(_, time)| time);
        Ok(SkinStoreSettings {
            index_url,
            last_refreshed_at_ms,
        })
    }

    pub fn set_url(&self, storage: &AppStorage, url: &str) -> Result<SkinStoreSettings, String> {
        let url = url.trim();
        if !url.is_empty() {
            protocol::validate_download_url(url)?;
        }
        let _guard = self
            .mutation
            .lock()
            .map_err(|_| "皮肤商店正在忙，请重试。".to_string())?;
        let settings = SkinStoreSettings {
            index_url: url.into(),
            last_refreshed_at_ms: None,
        };
        let value =
            serde_json::to_string(&settings).map_err(|_| "无法保存皮肤商店设置。".to_string())?;
        storage.lock_connection()?.execute(
            "INSERT INTO settings (key, value, updated_at) VALUES ('skinStore', ?1, CURRENT_TIMESTAMP) ON CONFLICT(key) DO UPDATE SET value=excluded.value, updated_at=CURRENT_TIMESTAMP",
            params![value],
        ).map_err(|_| "无法保存皮肤商店设置。".to_string())?;
        *self
            .last_error
            .lock()
            .map_err(|_| "无法更新皮肤商店状态。".to_string())? = None;
        self.settings(storage)
    }

    fn cache_path(&self) -> PathBuf {
        self.data_dir.join("skins-index-cache.json")
    }

    fn verify_index(&self, bytes: &[u8], signature: &str) -> Result<SkinIndex, String> {
        if self.key_hex == PUBLIC_KEY {
            protocol::verify_index(bytes, signature)
        } else {
            protocol::verify_index_with_key(bytes, signature, self.key_hex)
        }
    }

    fn cached(&self, url: &str) -> Option<(SkinIndex, i64)> {
        if url.is_empty() {
            return None;
        }
        let bytes = read_bounded(&self.cache_path(), MAX_CACHE_BYTES).ok()?;
        let cached: CachedIndex = serde_json::from_slice(&bytes).ok()?;
        if cached.source_url != url || cached.refreshed_at_ms < 0 {
            return None;
        }
        let index = self
            .verify_index(cached.document.as_bytes(), &cached.signature)
            .ok()?;
        Some((index, cached.refreshed_at_ms))
    }

    pub fn list(
        &self,
        storage: &AppStorage,
        catalog: CandidateSkinCatalogSnapshot,
    ) -> Result<StoreSnapshot, String> {
        let url = Self::configured_url(storage)?;
        let error = self
            .last_error
            .lock()
            .map_err(|_| "无法读取皮肤商店状态。".to_string())?
            .as_ref()
            .filter(|(source, _)| source == &url)
            .map(|(_, error)| error.clone());
        let Some((index, refreshed)) = self.cached(&url) else {
            return Ok(StoreSnapshot {
                skins: vec![],
                refreshed_at_ms: None,
                stale: true,
                error,
            });
        };
        Ok(self.snapshot(index, refreshed, error, catalog))
    }

    fn snapshot(
        &self,
        index: SkinIndex,
        refreshed: i64,
        error: Option<String>,
        catalog: CandidateSkinCatalogSnapshot,
    ) -> StoreSnapshot {
        let versions: BTreeMap<String, InstalledVersion> =
            read_bounded(&self.versions_path(), 128 * 1024)
                .ok()
                .and_then(|bytes| serde_json::from_slice(&bytes).ok())
                .unwrap_or_default();
        let skins = index
            .skins
            .into_iter()
            .filter(|skin| skin.tier == "free")
            .map(|skin| {
                let installed = catalog.skins.iter().any(|entry| {
                    entry.id == skin.id && entry.origin == "external" && entry.status == "ready"
                });
                let installed_version = if installed {
                    versions
                        .get(&skin.id)
                        .filter(|record| {
                            protocol::compare_versions(&record.version, &record.version).is_ok()
                                && read_bounded(
                                    &self.skins_root.join(&skin.id).join("skin.json"),
                                    skin_packages::MAXIMUM_SKIN_JSON_BYTES as usize,
                                )
                                .map(|bytes| protocol::sha256_hex(&bytes) == record.manifest_sha256)
                                .unwrap_or(false)
                        })
                        .map(|record| record.version.clone())
                } else {
                    None
                };
                let state = skin_state(
                    &skin,
                    installed,
                    installed_version.as_deref(),
                    env!("CARGO_PKG_VERSION"),
                );
                StoreSkin {
                    id: skin.id,
                    name: skin.name,
                    author: skin.author,
                    description: skin.description,
                    version: skin.version,
                    tier: skin.tier,
                    platforms: skin.platforms,
                    available_for_sale: skin.available_for_sale,
                    product_id: skin.product_id,
                    animated: skin.animated,
                    bundle_bytes: skin.bundle_bytes,
                    min_app_version: skin.min_app_version,
                    updated_at: skin.updated_at,
                    license: skin.license,
                    changelog: skin.changelog,
                    state: state.into(),
                    installed_version,
                }
            })
            .collect();
        StoreSnapshot {
            skins,
            refreshed_at_ms: Some(refreshed),
            stale: error.is_some(),
            error,
        }
    }

    pub async fn refresh(
        &self,
        storage: &AppStorage,
        now_ms: i64,
        catalog: CandidateSkinCatalogSnapshot,
    ) -> Result<StoreSnapshot, String> {
        let client = fetch_client();
        self.refresh_with(storage, now_ms, catalog, |url, limit| {
            let client = client.clone();
            async move { fetch_document(client?, url, limit).await }
        })
        .await
    }

    async fn refresh_with<F, Fut>(
        &self,
        storage: &AppStorage,
        now_ms: i64,
        catalog: CandidateSkinCatalogSnapshot,
        mut fetch: F,
    ) -> Result<StoreSnapshot, String>
    where
        F: FnMut(String, usize) -> Fut,
        Fut: std::future::Future<Output = Result<Vec<u8>, String>>,
    {
        let url = Self::configured_url(storage)?;
        let result = async {
            if url.is_empty() {
                return Err("还没有连接皮肤商店，请先填写 HTTPS 商店地址。".into());
            }
            if now_ms < 0 {
                return Err("系统时间无效。".into());
            }
            let index_url = format!("{}/skins-index.json", url.trim_end_matches('/'));
            protocol::validate_download_url(&index_url)?;
            let document = fetch(index_url.clone(), protocol::MAX_INDEX_BYTES).await?;
            let signature = fetch(format!("{index_url}.sig"), 256).await?;
            if document.len() > protocol::MAX_INDEX_BYTES || signature.len() > 256 {
                return Err("皮肤索引或签名超过大小上限。".into());
            }
            let signature = std::str::from_utf8(&signature)
                .map_err(|_| "皮肤商店签名格式无效。".to_string())?;
            self.verify_index(&document, signature)?;
            let cached = CachedIndex {
                source_url: url.clone(),
                refreshed_at_ms: now_ms,
                document: String::from_utf8(document)
                    .map_err(|_| "皮肤索引编码无效。".to_string())?,
                signature: signature.into(),
            };
            let _guard = self
                .mutation
                .lock()
                .map_err(|_| "皮肤商店正在忙，请重试。".to_string())?;
            if Self::configured_url(storage)? != url {
                return Err("皮肤商店地址已改变，请重新刷新。".into());
            }
            write_json(&self.cache_path(), &cached)?;
            Ok(())
        }
        .await;
        let error = result.err();
        if Self::configured_url(storage)? == url {
            *self
                .last_error
                .lock()
                .map_err(|_| "无法更新皮肤商店状态。".to_string())? =
                error.map(|error| (url, error));
        }
        self.list(storage, catalog)
    }

    fn entry(&self, storage: &AppStorage, id: &str) -> Result<IndexSkin, String> {
        let url = Self::configured_url(storage)?;
        self.cached(&url)
            .and_then(|(index, _)| index.skins.into_iter().find(|skin| skin.tier == "free" && skin.id == id))
            .ok_or_else(|| "没有找到有效的商店皮肤，请先刷新皮肤商店。".into())
    }

    pub async fn preview(&self, storage: &AppStorage, id: &str) -> Result<String, String> {
        let client = fetch_client()?;
        self.preview_with(storage, id, |url, limit| {
            fetch_document(client.clone(), url, limit)
        })
        .await
    }

    async fn preview_with<F, Fut>(
        &self,
        storage: &AppStorage,
        id: &str,
        mut fetch: F,
    ) -> Result<String, String>
    where
        F: FnMut(String, usize) -> Fut,
        Fut: std::future::Future<Output = Result<Vec<u8>, String>>,
    {
        let skin = self.entry(storage, id)?;
        let path = self
            .data_dir
            .join("skin-previews")
            .join(format!("{}.png", skin.preview_sha256.to_ascii_lowercase()));
        let cached = read_bounded(&path, protocol::MAX_PREVIEW_BYTES)
            .ok()
            .filter(|bytes| valid_preview(bytes, &skin.preview_sha256));
        let bytes = match cached {
            Some(bytes) => bytes,
            None => {
                let bytes = fetch(skin.preview_url.clone(), protocol::MAX_PREVIEW_BYTES).await?;
                if !valid_preview(&bytes, &skin.preview_sha256) {
                    return Err("皮肤预览校验失败，已丢弃。".into());
                }
                atomic_write(&path, &bytes)?;
                bytes
            }
        };
        Ok(format!(
            "data:image/png;base64,{}",
            protocol::base64_encode(&bytes)
        ))
    }

    pub async fn install(
        &self,
        storage: &AppStorage,
        id: &str,
    ) -> Result<CandidateSkinCatalogSnapshot, String> {
        let client = fetch_client()?;
        self.install_with(
            storage,
            id,
            skin_packages::catalog_snapshot(None)?,
            |url, limit| fetch_document(client.clone(), url, limit),
            skin_packages::install,
        )
        .await
    }

    async fn install_with<F, Fut, I>(
        &self,
        storage: &AppStorage,
        id: &str,
        catalog: CandidateSkinCatalogSnapshot,
        fetch: F,
        install: I,
    ) -> Result<CandidateSkinCatalogSnapshot, String>
    where
        F: FnMut(String, usize) -> Fut,
        Fut: std::future::Future<Output = Result<Vec<u8>, String>>,
        I: FnOnce(&Path) -> Result<CandidateSkinCatalogSnapshot, String>,
    {
        let skin = self.entry(storage, id)?;
        self.install_entry_with(&skin, catalog, fetch, install)
            .await
    }

    async fn install_entry_with<F, Fut, I>(
        &self,
        skin: &IndexSkin,
        catalog: CandidateSkinCatalogSnapshot,
        mut fetch: F,
        install: I,
    ) -> Result<CandidateSkinCatalogSnapshot, String>
    where
        F: FnMut(String, usize) -> Fut,
        Fut: std::future::Future<Output = Result<Vec<u8>, String>>,
        I: FnOnce(&Path) -> Result<CandidateSkinCatalogSnapshot, String>,
    {
        if protocol::compare_versions(&skin.min_app_version, env!("CARGO_PKG_VERSION"))?
            == std::cmp::Ordering::Greater
        {
            return Err("需要升级知字后才能安装此皮肤。".into());
        }
        if !skin.platforms.iter().any(|platform| platform == std::env::consts::OS) {
            return Err("这款皮肤暂不支持当前系统。".into());
        }
        if skin.tier != "free" {
            return Err("当前只提供免费皮肤。".into());
        }
        let url = skin.bundle_url.clone();
        protocol::validate_download_url(&url)?;
        if catalog
            .skins
            .iter()
            .any(|entry| entry.id == skin.id && entry.origin == "builtIn")
        {
            return Err("内置皮肤不能覆盖更新。".into());
        }
        let bytes = fetch(url, protocol::MAX_BUNDLE_BYTES).await?;
        let files = protocol::decode_bundle(&bytes, skin)?;
        let _guard = self
            .mutation
            .lock()
            .map_err(|_| "皮肤商店正在忙，请重试。".to_string())?;
        let temporary = TemporaryDirectory::new()?;
        let package = temporary.0.join(&skin.id);
        fs::create_dir(&package).map_err(|_| "无法创建皮肤解包目录。".to_string())?;
        for (path, bytes) in &files {
            write_new(&package.join(path), bytes)?;
        }
        let snapshot = install(&package)?;
        // The installed manifest stays unchanged; store provenance lives in manager data.
        let manifest = files
            .iter()
            .find(|(path, _)| path == "skin.json")
            .ok_or_else(|| "皮肤包缺少 skin.json。".to_string())?;
        let mut versions: BTreeMap<String, InstalledVersion> =
            read_bounded(&self.versions_path(), 128 * 1024)
                .ok()
                .and_then(|bytes| serde_json::from_slice(&bytes).ok())
                .unwrap_or_default();
        versions.insert(
            skin.id.clone(),
            InstalledVersion {
                version: skin.version.clone(),
                manifest_sha256: protocol::sha256_hex(&manifest.1),
            },
        );
        // Metadata loss must not turn a completed atomic installation into a reported failure.
        let _ = write_json(&self.versions_path(), &versions);
        Ok(snapshot)
    }

    fn versions_path(&self) -> PathBuf {
        self.data_dir.join("skin-store-versions.json")
    }
}

fn skin_state(
    skin: &IndexSkin,
    installed: bool,
    version: Option<&str>,
    app_version: &str,
) -> &'static str {
    if !skin.platforms.iter().any(|platform| platform == std::env::consts::OS) { "unsupportedPlatform" }
    else if protocol::compare_versions(&skin.min_app_version, app_version).ok()
        == Some(std::cmp::Ordering::Greater)
    {
        "needsAppUpgrade"
    } else if !installed {
        "notInstalled"
    } else if version.and_then(|version| protocol::compare_versions(&skin.version, version).ok())
        == Some(std::cmp::Ordering::Greater)
    {
        "updateAvailable"
    } else {
        "installed"
    }
}

fn valid_preview(bytes: &[u8], sha: &str) -> bool {
    bytes.len() <= protocol::MAX_PREVIEW_BYTES
        && protocol::is_png(bytes)
        && protocol::sha256_hex(bytes).eq_ignore_ascii_case(sha)
}

fn fetch_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(12))
        .build()
        .map_err(|_| "无法创建皮肤商店下载器。".into())
}

async fn fetch_document(
    client: reqwest::Client,
    url: String,
    maximum: usize,
) -> Result<Vec<u8>, String> {
    protocol::validate_download_url(&url)?;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|_| "当前无法连接皮肤商店，请稍后重试。".to_string())?;
    bounded_response(response, maximum).await
}

async fn bounded_response(response: reqwest::Response, maximum: usize) -> Result<Vec<u8>, String> {
    bounded_download(
        response.status().as_u16(),
        response.content_length(),
        maximum,
        response,
        |mut response| async move {
            let chunk = response
                .chunk()
                .await
                .map_err(|_| "皮肤商店下载中断，请稍后重试。".to_string())?;
            Ok((response, chunk))
        },
    )
    .await
}

async fn bounded_download<S, T: AsRef<[u8]>, F, Fut>(
    status: u16,
    content_length: Option<u64>,
    maximum: usize,
    mut stream: S,
    mut next_chunk: F,
) -> Result<Vec<u8>, String>
where
    F: FnMut(S) -> Fut,
    Fut: std::future::Future<Output = Result<(S, Option<T>), String>>,
{
    if !(200..300).contains(&status) {
        return Err("皮肤商店返回错误或重定向，请稍后重试。".into());
    }
    if content_length
        .map(|length| length > maximum as u64)
        .unwrap_or(false)
    {
        return Err("皮肤商店文件超过大小上限。".into());
    }
    let mut bytes = Vec::new();
    loop {
        let (returned, chunk) = next_chunk(stream).await?;
        stream = returned;
        let Some(chunk) = chunk else {
            break;
        };
        let chunk = chunk.as_ref();
        if chunk.len() > maximum.saturating_sub(bytes.len()) {
            return Err("皮肤商店文件超过大小上限。".into());
        }
        bytes.extend_from_slice(chunk);
    }
    Ok(bytes)
}

fn read_bounded(path: &Path, maximum: usize) -> Result<Vec<u8>, String> {
    let metadata = fs::symlink_metadata(path).map_err(|_| "无法读取皮肤商店缓存。".to_string())?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > maximum as u64 {
        return Err("皮肤商店缓存无效。".into());
    }
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|_| "无法读取皮肤商店缓存。".to_string())?
        .take(maximum as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "无法读取皮肤商店缓存。".to_string())?;
    if bytes.len() > maximum {
        return Err("皮肤商店缓存超过大小上限。".into());
    }
    Ok(bytes)
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|_| "无法创建皮肤商店暂存文件。".to_string())?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|_| "无法写入皮肤商店暂存文件。".into())
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let bytes = serde_json::to_vec(value).map_err(|_| "无法保存皮肤商店缓存。".to_string())?;
    atomic_write(path, &bytes)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "皮肤商店缓存目录无效。".to_string())?;
    fs::create_dir_all(parent).map_err(|_| "无法创建皮肤商店缓存目录。".to_string())?;
    let metadata =
        fs::symlink_metadata(parent).map_err(|_| "无法检查皮肤商店缓存目录。".to_string())?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err("皮肤商店缓存目录无效。".into());
    }
    let temporary = parent.join(format!(
        ".skin-store-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let result = write_new(&temporary, bytes)
        .and_then(|_| fs::rename(&temporary, path).map_err(|_| "无法保存皮肤商店缓存。".into()));
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

struct TemporaryDirectory(PathBuf);
impl TemporaryDirectory {
    fn new() -> Result<Self, String> {
        let path = std::env::temp_dir().join(format!(
            "zhizi-skin-store-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder
            .create(&path)
            .map_err(|_| "无法创建皮肤解包暂存目录。".to_string())?;
        Ok(Self(path))
    }
}
impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static SEQUENCE: AtomicUsize = AtomicUsize::new(0);
    const FIXTURE_INDEX: &[u8] = include_bytes!("../tests/fixtures/skin-store/skins-index.json");
    const FIXTURE_SIG: &str = include_str!("../tests/fixtures/skin-store/skins-index.json.sig");
    const FIXTURE_KEY: &str = include_str!("../tests/fixtures/skin-store/public-key.hex");
    const FIXTURE_BUNDLE: &[u8] =
        include_bytes!("../tests/fixtures/skin-store/teststore-1.0.0.zhskin.json");

    fn setup() -> (StoreBackend, AppStorage) {
        let root = std::env::temp_dir().join(format!(
            "skin-store-tests-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        let storage = AppStorage::open(root.join("settings.sqlite3")).unwrap();
        let mut backend = StoreBackend::new(root.join("manager"), root.join("Skins"));
        backend.key_hex = FIXTURE_KEY;
        (backend, storage)
    }

    fn seed(backend: &StoreBackend, storage: &AppStorage) {
        backend
            .set_url(storage, "https://example.invalid/store")
            .unwrap();
        let result = tauri::async_runtime::block_on(backend.refresh_with(
            storage,
            1234,
            empty_catalog(),
            |url, _| async move {
                Ok(if url.ends_with(".sig") {
                    FIXTURE_SIG.as_bytes().to_vec()
                } else {
                    FIXTURE_INDEX.to_vec()
                })
            },
        ))
        .unwrap();
        assert!(!result.stale, "{:?}", result.error);
    }

    fn empty_catalog() -> CandidateSkinCatalogSnapshot {
        CandidateSkinCatalogSnapshot {
            skins: vec![],
            issues: vec![],
        }
    }

    #[test]
    fn settings_are_opt_in_persistent_and_reject_unsafe_urls() {
        let (backend, storage) = setup();
        assert_eq!(backend.settings(&storage).unwrap().index_url, "");
        for url in [
            "http://example.invalid",
            "https://example.invalid/?",
            "https://user@example.invalid",
            "https://example.invalid/#x",
        ] {
            assert!(backend.set_url(&storage, url).is_err(), "{url}");
        }
        backend
            .set_url(&storage, "https://example.invalid/store")
            .unwrap();
        assert_eq!(
            backend.settings(&storage).unwrap().index_url,
            "https://example.invalid/store"
        );
    }

    #[test]
    fn failed_refresh_retains_verified_cache_and_marks_stale() {
        let (backend, storage) = setup();
        seed(&backend, &storage);
        let result = tauri::async_runtime::block_on(backend.refresh_with(
            &storage,
            5678,
            empty_catalog(),
            |_, _| async { Err("当前无法连接皮肤商店。".into()) },
        ))
        .unwrap();
        assert!(result.stale);
        assert_eq!(result.refreshed_at_ms, Some(1234));
        assert_eq!(result.skins.len(), 1);
        assert!(result.error.is_some());
        assert!(
            backend
                .list(&storage, empty_catalog())
                .unwrap()
                .stale
        );
        backend
            .set_url(&storage, "https://example.invalid/other")
            .unwrap();
        assert!(backend
            .list(&storage, empty_catalog())
            .unwrap()
            .skins
            .is_empty());
    }

    #[test]
    fn corrupted_cache_is_absent() {
        let (backend, storage) = setup();
        seed(&backend, &storage);
        fs::write(backend.cache_path(), b"broken").unwrap();
        assert!(backend
            .list(&storage, empty_catalog())
            .unwrap()
            .skins
            .is_empty());
        assert_eq!(
            backend.settings(&storage).unwrap().last_refreshed_at_ms,
            None
        );
    }

    #[test]
    fn python_fixture_installs_through_existing_atomic_installer() {
        let (backend, storage) = setup();
        seed(&backend, &storage);
        let calls = AtomicUsize::new(0);
        let catalog = tauri::async_runtime::block_on(backend.install_with(
            &storage,
            "teststore",
            empty_catalog(),
            |_, _| {
                calls.fetch_add(1, Ordering::Relaxed);
                async { Ok(FIXTURE_BUNDLE.to_vec()) }
            },
            |source| crate::skin_packages::install_for_store_test(source, &backend.skins_root),
        ))
        .unwrap();
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        assert!(backend.skins_root.join("teststore/skin.json").is_file());
        let snapshot = backend
            .list(&storage, catalog)
            .unwrap();
        assert_eq!(snapshot.skins[0].state, "installed");
        assert_eq!(
            snapshot.skins[0].installed_version.as_deref(),
            Some("1.0.0")
        );
    }

    #[test]
    fn signed_mixed_catalog_exposes_and_downloads_only_free_skins() {
        use ring::signature::{Ed25519KeyPair, KeyPair};
        use serde_json::json;

        let (mut backend, storage) = setup();
        let mut index: serde_json::Value = serde_json::from_slice(FIXTURE_INDEX).unwrap();
        let skins = index["skins"].as_array_mut().unwrap();
        let mut paid = skins[0].clone();
        paid["id"] = json!("testpaid");
        paid["productId"] = json!("skin.testpaid");
        paid["tier"] = json!("paid");
        paid["priceCents"] = json!(1200);
        skins.push(paid);
        let document = protocol::canonical_bytes(&index).unwrap();
        let pair = Ed25519KeyPair::from_seed_unchecked(&[7; 32]).unwrap();
        let encode_hex = |bytes: &[u8]| bytes.iter().map(|byte| format!("{byte:02x}")).collect::<String>();
        let signature = encode_hex(pair.sign(&document).as_ref());
        backend.key_hex = Box::leak(encode_hex(pair.public_key().as_ref()).into_boxed_str());
        backend.set_url(&storage, "https://example.invalid/store").unwrap();

        let refreshed = tauri::async_runtime::block_on(backend.refresh_with(
            &storage,
            1234,
            empty_catalog(),
            |url, _| {
                let bytes = if url.ends_with(".sig") { signature.as_bytes().to_vec() } else { document.clone() };
                async { Ok(bytes) }
            },
        )).unwrap();
        assert!(!refreshed.stale);
        assert_eq!(refreshed.skins.len(), 1);
        assert_eq!(refreshed.skins[0].id, "teststore");
        assert!(tauri::async_runtime::block_on(backend.preview_with(
            &storage, "testpaid", |_, _| async { panic!("paid preview must not download") },
        )).is_err());
        assert!(tauri::async_runtime::block_on(backend.install_with(
            &storage,
            "testpaid",
            empty_catalog(),
            |_, _| async { panic!("paid package must not download") },
            |_| panic!("paid package must not install"),
        )).is_err());
        assert!(!backend.skins_root.exists());
    }

    #[test]
    fn invalid_bundles_leave_existing_skin_directory_unchanged() {
        use serde_json::json;
        let (backend, _) = setup();
        fs::create_dir_all(&backend.skins_root).unwrap();
        fs::write(backend.skins_root.join("sentinel"), b"unchanged").unwrap();
        let original_skin =
            protocol::verify_index_with_key(FIXTURE_INDEX, FIXTURE_SIG, FIXTURE_KEY)
                .unwrap()
                .skins
                .remove(0);
        let bundle: serde_json::Value = serde_json::from_slice(FIXTURE_BUNDLE).unwrap();
        let mut bad = Vec::new();
        let mut value = bundle.clone();
        value["id"] = json!("different");
        bad.push(("id", value));
        let mut value = bundle.clone();
        value["version"] = json!("9.0.0");
        bad.push(("version", value));
        let mut value = bundle.clone();
        value["files"][0]["path"] = json!("../skin.json");
        bad.push(("traversal", value));
        let mut value = bundle.clone();
        value["files"] = json!((0..9)
            .map(|i| json!({"path": format!("{i}.png"), "base64": ""}))
            .collect::<Vec<_>>());
        bad.push(("file count", value));
        let mut value = bundle.clone();
        value["files"]
            .as_array_mut()
            .unwrap()
            .push(json!({"path":"bad.png", "base64":protocol::base64_encode(b"not png")}));
        bad.push(("not PNG", value));
        let mut value = bundle.clone();
        value["files"].as_array_mut().unwrap().push(
            json!({"path":"large.png", "base64":protocol::base64_encode(&vec![0u8; 1536*1024+1])}),
        );
        bad.push(("decoded size", value));
        for (name, value) in bad {
            let bytes = serde_json::to_vec(&value).unwrap();
            let mut skin = original_skin.clone();
            skin.bundle_sha256 = protocol::sha256_hex(&bytes);
            skin.bundle_bytes = bytes.len() as u64;
            let result = tauri::async_runtime::block_on(backend.install_entry_with(
                &skin,
                empty_catalog(),
                |_, _| async { Ok(bytes.clone()) },
                |source| crate::skin_packages::install_for_store_test(source, &backend.skins_root),
            ));
            assert!(result.is_err(), "{name}");
            assert_eq!(
                fs::read(backend.skins_root.join("sentinel")).unwrap(),
                b"unchanged"
            );
            assert_eq!(
                fs::read_dir(&backend.skins_root).unwrap().count(),
                1,
                "{name}"
            );
        }
        let mut skin = original_skin;
        skin.bundle_sha256 = "00".repeat(32);
        assert!(tauri::async_runtime::block_on(backend.install_entry_with(
            &skin,
            empty_catalog(),
            |_, _| async { Ok(FIXTURE_BUNDLE.to_vec()) },
            |_| panic!("bad SHA reached installer")
        ))
        .is_err());
        assert_eq!(fs::read_dir(&backend.skins_root).unwrap().count(), 1);
    }

    #[test]
    fn full_builtin_derived_bundle_installs_assets() {
        let (backend, _) = setup();
        let bytes = include_bytes!("../tests/fixtures/skin-store/full-system-1.0.0.zhskin.json");
        let mut skin = protocol::verify_index_with_key(FIXTURE_INDEX, FIXTURE_SIG, FIXTURE_KEY)
            .unwrap()
            .skins
            .remove(0);
        skin.id = "testfull".into();
        skin.bundle_sha256 = protocol::sha256_hex(bytes);
        skin.bundle_bytes = bytes.len() as u64;
        let catalog = tauri::async_runtime::block_on(backend.install_entry_with(
            &skin,
            empty_catalog(),
            |_, _| async { Ok(bytes.to_vec()) },
            |source| crate::skin_packages::install_for_store_test(source, &backend.skins_root),
        ))
        .unwrap();
        assert!(catalog
            .skins
            .iter()
            .any(|entry| entry.id == "testfull" && entry.status == "ready"));
        assert!(backend.skins_root.join("testfull/mark.png").is_file());
        assert!(backend.skins_root.join("testfull/skin.json").is_file());
        assert_eq!(fs::read_dir(&backend.skins_root).unwrap().count(), 1);
    }

    #[test]
    fn available_update_installs_and_persists_new_version() {
        use serde_json::json;
        let (backend, storage) = setup();
        seed(&backend, &storage);
        let catalog = tauri::async_runtime::block_on(backend.install_with(
            &storage,
            "teststore",
            empty_catalog(),
            |_, _| async { Ok(FIXTURE_BUNDLE.to_vec()) },
            |source| crate::skin_packages::install_for_store_test(source, &backend.skins_root),
        ))
        .unwrap();
        let mut index =
            protocol::verify_index_with_key(FIXTURE_INDEX, FIXTURE_SIG, FIXTURE_KEY).unwrap();
        let mut bundle: serde_json::Value = serde_json::from_slice(FIXTURE_BUNDLE).unwrap();
        let encoded = bundle["files"][0]["base64"].as_str().unwrap();
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&protocol::base64_decode(encoded).unwrap()).unwrap();
        manifest["description"] = json!("updated package");
        let manifest = serde_json::to_vec(&manifest).unwrap();
        bundle["files"][0]["base64"] = json!(protocol::base64_encode(&manifest));
        bundle["version"] = json!("1.1.0");
        let bytes = serde_json::to_vec(&bundle).unwrap();
        let skin = &mut index.skins[0];
        skin.version = "1.1.0".into();
        skin.bundle_sha256 = protocol::sha256_hex(&bytes);
        skin.bundle_bytes = bytes.len() as u64;
        let before = backend.snapshot(
            index.clone(),
            1234,
            None,
            catalog.clone(),
        );
        assert_eq!(before.skins[0].state, "updateAvailable");
        assert_eq!(before.skins[0].installed_version.as_deref(), Some("1.0.0"));
        let catalog = tauri::async_runtime::block_on(backend.install_entry_with(
            &index.skins[0],
            catalog,
            |_, _| async { Ok(bytes.clone()) },
            |source| crate::skin_packages::install_for_store_test(source, &backend.skins_root),
        ))
        .unwrap();
        assert_eq!(
            fs::read(backend.skins_root.join("teststore/skin.json")).unwrap(),
            manifest
        );
        let reopened = StoreBackend::new(backend.data_dir.clone(), backend.skins_root.clone());
        let after = reopened.snapshot(index, 1234, None, catalog);
        assert_eq!(after.skins[0].state, "installed");
        assert_eq!(after.skins[0].installed_version.as_deref(), Some("1.1.0"));
        assert_eq!(fs::read_dir(&backend.skins_root).unwrap().count(), 1);
    }

    #[test]
    fn preview_is_sha_checked_cached_and_requires_real_png() {
        let (backend, storage) = setup();
        seed(&backend, &storage);
        let bytes = include_bytes!("../tests/fixtures/skin-store/preview.png");
        let calls = AtomicUsize::new(0);
        for _ in 0..2 {
            let data = tauri::async_runtime::block_on(backend.preview_with(
                &storage,
                "teststore",
                |_, limit| {
                    assert_eq!(limit, protocol::MAX_PREVIEW_BYTES);
                    calls.fetch_add(1, Ordering::Relaxed);
                    async { Ok(bytes.to_vec()) }
                },
            ))
            .unwrap();
            assert!(data.starts_with("data:image/png;base64,"));
        }
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        let path = backend
            .data_dir
            .join("skin-previews")
            .join(format!("{}.png", protocol::sha256_hex(bytes)));
        fs::write(&path, b"corrupt cache").unwrap();
        assert!(tauri::async_runtime::block_on(backend.preview_with(
            &storage,
            "teststore",
            |_, _| async { Ok(b"wrong hash".to_vec()) }
        ))
        .is_err());
        let fake_png = b"\x89PNG\r\n\x1a\n";
        assert!(!valid_preview(fake_png, &protocol::sha256_hex(fake_png)));
    }

    #[test]
    fn upgrade_and_update_states_match_frontend_contract() {
        let mut skin = protocol::verify_index_with_key(FIXTURE_INDEX, FIXTURE_SIG, FIXTURE_KEY)
            .unwrap()
            .skins
            .remove(0);
        assert_eq!(skin_state(&skin, false, None, "0.3.24"), "notInstalled");
        assert_eq!(skin_state(&skin, true, Some("1.0"), "0.3.24"), "installed");
        assert_eq!(
            skin_state(&skin, true, Some("0.9.9"), "0.3.24"),
            "updateAvailable"
        );
        skin.min_app_version = "99.0.0".into();
        assert_eq!(
            skin_state(&skin, true, Some("1.0.0"), "0.3.24"),
            "needsAppUpgrade"
        );
        let (backend, _) = setup();
        let snapshot = backend.snapshot(
            SkinIndex {
                schema: "agent-ime-skin-index/1".into(),
                generated_at: "2026-11-08T04:00:00Z".into(),
                producer: "test".into(),
                skins: vec![skin],
            },
            1234,
            None,
            empty_catalog(),
        );
        let value = serde_json::to_value(&snapshot).unwrap();
        assert!(value["skins"][0]["installedVersion"].is_null());
        assert_eq!(value["skins"][0]["state"], "needsAppUpgrade");
        assert!(value["skins"][0].get("bundleUrl").is_none());
        assert_eq!(value["refreshedAtMs"], 1234);
    }

    #[test]
    fn production_client_rejects_http_before_network() {
        let client = fetch_client().unwrap();
        let result = tauri::async_runtime::block_on(client.get("http://127.0.0.1:1/").send());
        let error = result.unwrap_err();
        assert!(!error.is_connect() && !error.is_timeout());
    }

    #[test]
    fn response_limits_apply_to_headers_and_streamed_chunks() {
        for (name, status, length, chunks, maximum, expected) in [
            (
                "bounded",
                200,
                Some(4),
                vec![b"te".to_vec(), b"st".to_vec()],
                4,
                true,
            ),
            ("length", 200, Some(5), vec![], 4, false),
            (
                "chunks",
                200,
                None,
                vec![b"abc".to_vec(), b"def".to_vec()],
                4,
                false,
            ),
            ("redirect", 302, Some(0), vec![], 4, false),
            ("server error", 500, Some(0), vec![], 4, false),
        ] {
            let result = tauri::async_runtime::block_on(bounded_download(
                status,
                length,
                maximum,
                chunks.into_iter(),
                |mut chunks| async move {
                    let chunk = chunks.next();
                    Ok((chunks, chunk))
                },
            ));
            assert_eq!(result.is_ok(), expected, "{name}");
        }
    }

    #[test]
    fn invalid_refresh_never_replaces_old_signed_cache() {
        let (backend, storage) = setup();
        seed(&backend, &storage);
        let before = fs::read(backend.cache_path()).unwrap();
        let bad_documents = [
            b"not JSON".to_vec(),
            FIXTURE_INDEX.iter().copied().chain([b'x']).collect(),
            vec![b' '; protocol::MAX_INDEX_BYTES + 1],
        ];
        for document in bad_documents {
            let result = tauri::async_runtime::block_on(backend.refresh_with(
                &storage,
                5000,
                empty_catalog(),
                |url, _| {
                    let bytes = if url.ends_with(".sig") {
                        FIXTURE_SIG.as_bytes().to_vec()
                    } else {
                        document.clone()
                    };
                    async { Ok(bytes) }
                },
            ))
            .unwrap();
            assert!(result.stale);
            assert_eq!(result.refreshed_at_ms, Some(1234));
            assert_eq!(result.skins.len(), 1);
            assert_eq!(fs::read(backend.cache_path()).unwrap(), before);
        }
    }

}

mod protocol {
    use crate::skin_catalog::is_candidate_skin_id;
    use chrono::{DateTime, NaiveDate};
    use ring::signature::{UnparsedPublicKey, ED25519};
    use serde::de::{self, MapAccess, SeqAccess, Visitor};
    use serde::{Deserialize, Deserializer, Serialize};
    use serde_json::{Map, Value};
    use sha2::{Digest, Sha256};
    use std::cmp::Ordering;
    use std::collections::HashSet;
    use std::fmt;

    pub(crate) const MAX_INDEX_BYTES: usize = 512 * 1024;
    pub(crate) const MAX_PREVIEW_BYTES: usize = 300 * 1024;
    pub(crate) const MAX_BUNDLE_BYTES: usize = 22 * 1024 * 1024 / 10;
    const MAX_FILES: usize = 8;
    const MAX_DECODED_BYTES: usize = 3 * 1024 * 1024 / 2;
    const MAX_SKIN_JSON_BYTES: usize = 256 * 1024;
    const PUBLIC_KEY_HEX: &str = include_str!("../resources/skin_index_pubkey.hex");

    #[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    pub(crate) struct SkinIndex {
        pub schema: String,
        pub generated_at: String,
        pub producer: String,
        pub skins: Vec<IndexSkin>,
    }

    #[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    pub(crate) struct SkinName {
        pub zh: String,
        pub en: String,
    }

    #[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    pub(crate) struct IndexSkin {
        pub id: String,
        pub name: SkinName,
        pub author: String,
        pub description: String,
        pub version: String,
        pub tier: String,
        pub price_cents: u32,
        #[serde(default = "default_currency")]
        pub currency: String,
        #[serde(default = "default_platforms")]
        pub platforms: Vec<String>,
        #[serde(default = "default_for_sale")]
        pub available_for_sale: bool,
        pub product_id: String,
        pub animated: bool,
        pub preview_url: String,
        pub preview_sha256: String,
        pub bundle_url: String,
        pub bundle_sha256: String,
        pub bundle_bytes: u64,
        pub min_app_version: String,
        pub updated_at: String,
        pub license: String,
        pub changelog: String,
    }

    fn default_currency() -> String { "CNY".into() }
    fn default_for_sale() -> bool { true }
    fn default_platforms() -> Vec<String> { vec!["macos".into()] }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Bundle {
        schema: String,
        id: String,
        version: String,
        files: Vec<BundleFile>,
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct BundleFile {
        path: String,
        base64: String,
    }

    // Reject duplicate keys before interpreting either a signature or package metadata.
    struct UniqueValue(Value);
    impl<'de> Deserialize<'de> for UniqueValue {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            struct ValueVisitor;
            impl<'de> Visitor<'de> for ValueVisitor {
                type Value = UniqueValue;
                fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                    formatter.write_str("JSON with unique keys")
                }
                fn visit_bool<E: de::Error>(self, value: bool) -> Result<Self::Value, E> {
                    Ok(UniqueValue(Value::Bool(value)))
                }
                fn visit_i64<E: de::Error>(self, value: i64) -> Result<Self::Value, E> {
                    Ok(UniqueValue(Value::from(value)))
                }
                fn visit_u64<E: de::Error>(self, value: u64) -> Result<Self::Value, E> {
                    Ok(UniqueValue(Value::from(value)))
                }
                fn visit_f64<E: de::Error>(self, value: f64) -> Result<Self::Value, E> {
                    serde_json::Number::from_f64(value)
                        .map(|number| UniqueValue(Value::Number(number)))
                        .ok_or_else(|| E::custom("无效数值"))
                }
                fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                    Ok(UniqueValue(Value::String(value.into())))
                }
                fn visit_string<E: de::Error>(self, value: String) -> Result<Self::Value, E> {
                    Ok(UniqueValue(Value::String(value)))
                }
                fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
                    Ok(UniqueValue(Value::Null))
                }
                fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
                    Ok(UniqueValue(Value::Null))
                }
                fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                    let mut values = Vec::new();
                    while let Some(value) = seq.next_element::<UniqueValue>()? {
                        values.push(value.0);
                    }
                    Ok(UniqueValue(Value::Array(values)))
                }
                fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                    let mut values = Map::new();
                    while let Some((key, value)) = map.next_entry::<String, UniqueValue>()? {
                        if values.insert(key, value.0).is_some() {
                            return Err(de::Error::custom("重复字段"));
                        }
                    }
                    Ok(UniqueValue(Value::Object(values)))
                }
            }
            deserializer.deserialize_any(ValueVisitor)
        }
    }

    pub(crate) fn canonical_bytes(value: &Value) -> Result<Vec<u8>, String> {
        fn ordered(value: &Value) -> Result<Value, String> {
            match value {
                Value::Object(map) => {
                    let mut keys = map.keys().collect::<Vec<_>>();
                    keys.sort();
                    let mut result = Map::new();
                    for key in keys {
                        result.insert(key.clone(), ordered(&map[key])?);
                    }
                    Ok(Value::Object(result))
                }
                Value::Array(values) => values
                    .iter()
                    .map(ordered)
                    .collect::<Result<Vec<_>, _>>()
                    .map(Value::Array),
                Value::Number(number) if !number.is_i64() && !number.is_u64() => {
                    Err("规范化索引不接受小数。".into())
                }
                _ => Ok(value.clone()),
            }
        }
        let mut bytes =
            serde_json::to_vec(&ordered(value)?).map_err(|_| "无法规范化皮肤索引。".to_string())?;
        bytes.push(b'\n');
        Ok(bytes)
    }

    fn decode_hex(value: &str, byte_count: usize) -> Result<Vec<u8>, String> {
        let value = value.trim();
        if value.len() != byte_count * 2 || !value.bytes().all(|ch| ch.is_ascii_hexdigit()) {
            return Err("签名或公钥格式无效。".into());
        }
        (0..value.len())
            .step_by(2)
            .map(|index| {
                u8::from_str_radix(&value[index..index + 2], 16)
                    .map_err(|_| "签名或公钥格式无效。".to_string())
            })
            .collect()
    }

    pub(crate) fn verify_index(bytes: &[u8], sig: &str) -> Result<SkinIndex, String> {
        verify_index_with_key(bytes, sig, PUBLIC_KEY_HEX)
    }

    pub(crate) fn verify_index_with_key(
        bytes: &[u8],
        sig: &str,
        key_hex: &str,
    ) -> Result<SkinIndex, String> {
        if bytes.len() > MAX_INDEX_BYTES {
            return Err("皮肤索引超过 512 KiB，整份已丢弃。".into());
        }
        let value = serde_json::from_slice::<UniqueValue>(bytes)
            .map_err(|_| "皮肤索引 JSON 无效或字段重复，整份已丢弃。".to_string())?
            .0;
        let normalized = canonical_bytes(&value)?;
        UnparsedPublicKey::new(&ED25519, decode_hex(key_hex, 32)?)
            .verify(&normalized, &decode_hex(sig, 64)?)
            .map_err(|_| "皮肤索引签名校验失败，整份已丢弃。".to_string())?;
        if value.get("schema").and_then(Value::as_str) != Some("agent-ime-skin-index/1") {
            return Err("不支持的皮肤索引 schema。".into());
        }
        let skins = value
            .get("skins")
            .and_then(Value::as_array)
            .ok_or_else(|| "皮肤索引 skins 必须为数组。".to_string())?;
        if skins.len() > 200 {
            return Err("皮肤索引超过 200 条，整份已丢弃。".into());
        }
        let index: SkinIndex = serde_json::from_value(value)
            .map_err(|_| "皮肤索引字段缺失或类型无效，整份已丢弃。".to_string())?;
        DateTime::parse_from_rfc3339(&index.generated_at)
            .map_err(|_| "皮肤索引生成时间必须是带时区的 RFC3339 时间。".to_string())?;
        validate_text(&index.producer, "生成器", 120, false)?;
        let mut ids = HashSet::new();
        for skin in &index.skins {
            validate_id(&skin.id)?;
            if is_candidate_skin_id(&skin.id) {
                return Err("商店皮肤 id 与内置皮肤冲突，整份索引已丢弃。".into());
            }
            if !ids.insert(&skin.id) {
                return Err("皮肤索引存在重复 id，整份已丢弃。".into());
            }
            validate_text(&skin.name.zh, "中文名称", 80, false)?;
            validate_text(&skin.name.en, "英文名称", 80, false)?;
            validate_text(&skin.author, "作者", 120, false)?;
            validate_text(&skin.description, "描述", 1000, true)?;
            validate_text(&skin.product_id, "商品 id", 128, false)?;
            validate_text(&skin.license, "许可", 1000, false)?;
            validate_text(&skin.changelog, "更新说明", 2000, true)?;
            compare_versions(&skin.version, &skin.min_app_version)?;
            validate_download_url(&skin.preview_url)?;
            validate_download_url(&skin.bundle_url)?;
            if !is_sha256(&skin.preview_sha256) || !is_sha256(&skin.bundle_sha256) {
                return Err("皮肤 SHA-256 必须为 64 位十六进制。".into());
            }
            if skin.price_cents > 99_900
                || !["CNY", "USD", "HKD", "EUR", "GBP", "JPY"].contains(&skin.currency.as_str())
                || skin.platforms.is_empty() || skin.platforms.len() > 3
                || skin.platforms.iter().any(|platform| !["macos", "windows", "linux"].contains(&platform.as_str()))
                || !matches!(skin.tier.as_str(), "free" | "paid")
                || ((skin.tier == "free") != (skin.price_cents == 0))
            {
                return Err("皮肤价格必须为 0–99900，且仅免费皮肤价格为 0。".into());
            }
            if skin.bundle_bytes == 0 || skin.bundle_bytes > MAX_BUNDLE_BYTES as u64 {
                return Err("皮肤包大小必须在 1 字节至 2.2 MiB 以内。".into());
            }
            if skin.updated_at.len() != 10
                || NaiveDate::parse_from_str(&skin.updated_at, "%Y-%m-%d").is_err()
            {
                return Err("皮肤更新日期必须为 YYYY-MM-DD。".into());
            }
        }
        Ok(index)
    }

    fn validate_text(value: &str, label: &str, maximum: usize, empty: bool) -> Result<(), String> {
        if (!empty && value.trim().is_empty())
            || value.chars().count() > maximum
            || value
                .chars()
                .any(|ch| ch.is_control() && !matches!(ch, '\n' | '\r' | '\t'))
        {
            Err(format!(
                "{label}为空、含控制字符或超过 {maximum} 字符上限。"
            ))
        } else {
            Ok(())
        }
    }

    fn validate_id(id: &str) -> Result<(), String> {
        let bytes = id.as_bytes();
        if bytes.is_empty()
            || bytes.len() > 64
            || !bytes[0].is_ascii_lowercase()
            || id.contains("..")
            || !bytes.iter().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(byte)
            })
        {
            return Err("皮肤 id 必须匹配 ^[a-z][a-z0-9._-]{0,63}$，不允许路径穿越。".into());
        }
        Ok(())
    }

    pub(crate) fn validate_download_url(value: &str) -> Result<(), String> {
        let url = reqwest::Url::parse(value)
            .map_err(|_| "下载地址必须是无参数、无身份信息的 HTTPS URL。".to_string())?;
        if !value.starts_with("https://")
            || value[8..].starts_with('/')
            || value[8..]
                .split('/')
                .next()
                .is_some_and(|authority| authority.contains('@'))
            || value.chars().count() > 2048
            || value
                .chars()
                .any(|ch| ch.is_whitespace() || ch.is_control())
            || value.contains(['?', '#', '\\'])
            || url.scheme() != "https"
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err("下载地址必须是无参数、无身份信息的 HTTPS URL。".into());
        }
        Ok(())
    }

    fn is_sha256(value: &str) -> bool {
        value.len() == 64 && value.bytes().all(|ch| ch.is_ascii_hexdigit())
    }

    pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
        Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    pub(crate) fn compare_versions(left: &str, right: &str) -> Result<Ordering, String> {
        fn components(value: &str) -> Result<Vec<u64>, String> {
            if value.len() > 128 {
                return Err("版本不能超过 128 字符。".into());
            }
            value
                .split('.')
                .map(|part| {
                    if part.is_empty() || !part.bytes().all(|ch| ch.is_ascii_digit()) {
                        return Err("版本必须为数字点分格式。".into());
                    }
                    part.parse().map_err(|_| "版本数字超出范围。".into())
                })
                .collect()
        }
        let left = components(left)?;
        let right = components(right)?;
        for index in 0..left.len().max(right.len()) {
            let order = left
                .get(index)
                .unwrap_or(&0)
                .cmp(right.get(index).unwrap_or(&0));
            if order != Ordering::Equal {
                return Ok(order);
            }
        }
        Ok(Ordering::Equal)
    }

    pub(crate) fn base64_encode(bytes: &[u8]) -> String {
        const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut result = String::with_capacity((bytes.len() + 2) / 3 * 4);
        for chunk in bytes.chunks(3) {
            let first = chunk[0];
            let second = *chunk.get(1).unwrap_or(&0);
            let third = *chunk.get(2).unwrap_or(&0);
            result.push(ALPHABET[(first >> 2) as usize] as char);
            result.push(ALPHABET[(((first & 3) << 4) | (second >> 4)) as usize] as char);
            result.push(if chunk.len() > 1 {
                ALPHABET[(((second & 15) << 2) | (third >> 6)) as usize] as char
            } else {
                '='
            });
            result.push(if chunk.len() > 2 {
                ALPHABET[(third & 63) as usize] as char
            } else {
                '='
            });
        }
        result
    }

    pub(crate) fn base64_decode(value: &str) -> Result<Vec<u8>, String> {
        fn digit(value: u8) -> Option<u8> {
            match value {
                b'A'..=b'Z' => Some(value - b'A'),
                b'a'..=b'z' => Some(value - b'a' + 26),
                b'0'..=b'9' => Some(value - b'0' + 52),
                b'+' => Some(62),
                b'/' => Some(63),
                _ => None,
            }
        }
        if value.len() % 4 != 0 {
            return Err("皮肤文件 base64 无效。".into());
        }
        let mut bytes = Vec::with_capacity(value.len() / 4 * 3);
        let groups = value.len() / 4;
        for (index, chunk) in value.as_bytes().chunks(4).enumerate() {
            let first = digit(chunk[0]).ok_or_else(|| "皮肤文件 base64 无效。".to_string())?;
            let second = digit(chunk[1]).ok_or_else(|| "皮肤文件 base64 无效。".to_string())?;
            let third = if chunk[2] == b'=' {
                0
            } else {
                digit(chunk[2]).ok_or_else(|| "皮肤文件 base64 无效。".to_string())?
            };
            let fourth = if chunk[3] == b'=' {
                0
            } else {
                digit(chunk[3]).ok_or_else(|| "皮肤文件 base64 无效。".to_string())?
            };
            if (chunk[2] == b'=' && chunk[3] != b'=')
                || ((chunk[2] == b'=' || chunk[3] == b'=') && index + 1 != groups)
            {
                return Err("皮肤文件 base64 无效。".into());
            }
            bytes.push((first << 2) | (second >> 4));
            if chunk[2] != b'=' {
                bytes.push((second << 4) | (third >> 2));
            }
            if chunk[3] != b'=' {
                bytes.push((third << 6) | fourth);
            }
        }
        if base64_encode(&bytes) != value {
            return Err("皮肤文件 base64 无效。".into());
        }
        Ok(bytes)
    }

    pub(crate) fn is_png(bytes: &[u8]) -> bool {
        if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            return false;
        }
        let mut offset = 8;
        let mut has_header = false;
        let mut has_image = false;
        while offset + 12 <= bytes.len() {
            let length = u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
            let Some(end) = offset
                .checked_add(12)
                .and_then(|value| value.checked_add(length))
            else {
                return false;
            };
            if end > bytes.len() {
                return false;
            }
            let kind = &bytes[offset + 4..offset + 8];
            let data = &bytes[offset + 8..end - 4];
            let mut crc = !0u32;
            for byte in &bytes[offset + 4..end - 4] {
                crc ^= u32::from(*byte);
                for _ in 0..8 {
                    crc = if crc & 1 == 1 {
                        (crc >> 1) ^ 0xedb88320
                    } else {
                        crc >> 1
                    };
                }
            }
            if !crc != u32::from_be_bytes(bytes[end - 4..end].try_into().unwrap()) {
                return false;
            }
            if !has_header {
                if kind != b"IHDR" || length != 13 || data[..4] == [0; 4] || data[4..8] == [0; 4] {
                    return false;
                }
                has_header = true;
            } else if kind == b"IHDR" {
                return false;
            }
            if kind == b"IDAT" {
                has_image = true;
            }
            if kind == b"IEND" {
                return length == 0 && has_image && end == bytes.len();
            }
            offset = end;
        }
        false
    }

    pub(crate) fn decode_bundle(
        bytes: &[u8],
        skin: &IndexSkin,
    ) -> Result<Vec<(String, Vec<u8>)>, String> {
        if bytes.len() > MAX_BUNDLE_BYTES || bytes.len() as u64 != skin.bundle_bytes {
            return Err("皮肤包体积与索引不符或超过 2.2 MiB。".into());
        }
        if !sha256_hex(bytes).eq_ignore_ascii_case(&skin.bundle_sha256) {
            return Err("皮肤包 SHA-256 校验失败。".into());
        }
        let value = serde_json::from_slice::<UniqueValue>(bytes)
            .map_err(|_| "皮肤包 JSON 无效或字段重复。".to_string())?
            .0;
        let bundle: Bundle =
            serde_json::from_value(value).map_err(|_| "皮肤包字段缺失或类型无效。".to_string())?;
        if bundle.schema != "agent-ime-skin-bundle/1"
            || bundle.id != skin.id
            || bundle.version != skin.version
        {
            return Err("皮肤包 schema、id 或版本与索引不符。".into());
        }
        if bundle.files.is_empty() || bundle.files.len() > MAX_FILES {
            return Err("皮肤包必须包含 skin.json，且最多 8 个文件。".into());
        }
        let mut paths = HashSet::new();
        let mut files = Vec::new();
        let mut total = 0usize;
        let mut has_skin = false;
        for file in bundle.files {
            if file.path.is_empty()
                || file.path.contains("..")
                || file.path.contains(['/', '\\', ':'])
                || file.path.chars().any(|ch| ch.is_control())
                || !paths.insert(file.path.clone())
                || !(file.path.ends_with(".json") || file.path.ends_with(".png"))
            {
                return Err("皮肤包包含不安全、重复或不支持的文件路径。".into());
            }
            let data = base64_decode(&file.base64)?;
            total = total
                .checked_add(data.len())
                .ok_or_else(|| "皮肤包体积溢出。".to_string())?;
            if total > MAX_DECODED_BYTES {
                return Err("皮肤包解开后超过 1.5 MiB。".into());
            }
            if file.path.ends_with(".png") && !is_png(&data) {
                return Err("皮肤素材必须为有效 PNG 文件。".into());
            }
            if file.path.ends_with(".json") {
                if file.path == "skin.json" && data.len() > MAX_SKIN_JSON_BYTES {
                    return Err("skin.json 超过 256 KiB。".into());
                }
                let value = serde_json::from_slice::<UniqueValue>(&data)
                    .map_err(|_| "皮肤 JSON 文件无效或字段重复。".to_string())?
                    .0;
                if file.path == "skin.json" {
                    if value.get("id").and_then(Value::as_str) != Some(skin.id.as_str())
                        || value
                            .get("version")
                            .is_some_and(|version| version.as_str() != Some(skin.version.as_str()))
                    {
                        return Err("skin.json id 或版本与索引不符。".into());
                    }
                    has_skin = true;
                }
            }
            files.push((file.path, data));
        }
        if !has_skin {
            return Err("皮肤包缺少 skin.json。".into());
        }
        Ok(files)
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use ring::signature::{Ed25519KeyPair, KeyPair};
        use serde_json::json;

        fn index_value() -> Value {
            json!({
                "schema":"agent-ime-skin-index/1", "generatedAt":"2026-11-08T04:00:00Z",
                "producer":"agent-ime-skin-publisher/1.0.0", "skins":[{
                    "id":"teststore", "name":{"zh":"测试皮肤","en":"Test skin"},
                    "author":"Test publisher", "description":"Test only", "version":"1.0.0",
                    "tier":"free", "priceCents":0, "productId":"skin.teststore", "animated":false,
                    "previewUrl":"https://example.invalid/preview.png", "previewSha256":"00".repeat(32),
                    "bundleUrl":"https://example.invalid/teststore-1.0.0.zhskin.json", "bundleSha256":"00".repeat(32),
                    "bundleBytes":500, "minAppVersion":"0.3.24", "updatedAt":"2026-11-08",
                    "license":"Test only", "changelog":"Initial test"
                }]
            })
        }

        fn sign(value: &Value) -> (Vec<u8>, String, String) {
            let pair = Ed25519KeyPair::from_seed_unchecked(&[7; 32]).unwrap();
            let data = canonical_bytes(value).unwrap();
            let signature = pair
                .sign(&data)
                .as_ref()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            let key = pair
                .public_key()
                .as_ref()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            (data, signature, key)
        }

        fn verify(value: &Value) -> Result<SkinIndex, String> {
            let (bytes, sig, key) = sign(value);
            verify_index_with_key(&bytes, &sig, &key)
        }

        #[test]
        fn valid_index_signature_and_reordered_keys_pass() {
            let value = index_value();
            let (bytes, sig, key) = sign(&value);
            assert_eq!(
                verify_index_with_key(&bytes, &sig, &key).unwrap().skins[0].id,
                "teststore"
            );
            let pretty = serde_json::to_vec_pretty(&value).unwrap();
            assert!(verify_index_with_key(&pretty, &sig, &key).is_ok());
            let reordered_index = format!(
                "{{\"skins\":{},\"producer\":{},\"generatedAt\":{},\"schema\":{}}}",
                value["skins"], value["producer"], value["generatedAt"], value["schema"]
            );
            assert!(verify_index_with_key(reordered_index.as_bytes(), &sig, &key).is_ok());
            let reordered = b"{\"z\":1,\"a\":{\"y\":2,\"b\":3}}";
            assert_eq!(
                canonical_bytes(&serde_json::from_slice(reordered).unwrap()).unwrap(),
                b"{\"a\":{\"b\":3,\"y\":2},\"z\":1}\n"
            );
        }

        #[test]
        fn tamper_and_wrong_signing_key_fail() {
            let (mut bytes, sig, key) = sign(&index_value());
            let position = bytes
                .windows(9)
                .position(|window| window == b"teststore")
                .unwrap();
            bytes[position] = b'x';
            assert!(verify_index_with_key(&bytes, &sig, &key).is_err());
            let (bytes, sig, _) = sign(&index_value());
            assert!(verify_index_with_key(&bytes, &sig, &"00".repeat(32)).is_err());
        }

        #[test]
        fn invalid_schema_and_too_many_entries_fail() {
            let mut value = index_value();
            value["schema"] = json!("agent-ime-skin-index/2");
            assert!(verify(&value).is_err());
            let mut value = index_value();
            value["skins"] = json!(vec![value["skins"][0].clone(); 201]);
            assert!(verify(&value).is_err());
        }

        #[test]
        fn invalid_index_fields_fail_as_whole_document() {
            let cases = [
                ("id", json!("../escape")),
                ("id", json!("A")),
                ("id", json!("a".repeat(65))),
                ("id", json!("system")),
                ("previewUrl", json!("http://example.invalid/a")),
                ("bundleUrl", json!("https://example.invalid/a?token=value")),
                ("previewSha256", json!("0".repeat(63))),
                ("bundleSha256", json!("g".repeat(64))),
                ("priceCents", json!(-1)),
                ("priceCents", json!(99901)),
                ("priceCents", json!(1)),
                ("tier", json!("paid")),
                ("tier", json!("unknown")),
                ("version", json!("1.a")),
                ("minAppVersion", json!("1..2")),
                ("bundleBytes", json!(0)),
                ("bundleBytes", json!(2_306_868)),
                ("updatedAt", json!("2026-02-30")),
                ("productId", json!("")),
            ];
            for (field, invalid) in cases {
                let mut value = index_value();
                value["skins"][0][field] = invalid;
                assert!(verify(&value).is_err(), "{field}");
            }
            let mut paid = index_value();
            paid["skins"][0]["tier"] = json!("paid");
            paid["skins"][0]["priceCents"] = json!(99900);
            assert!(verify(&paid).is_ok());
            let mut duplicate = index_value();
            duplicate["skins"] =
                json!([duplicate["skins"][0].clone(), duplicate["skins"][0].clone()]);
            assert!(verify(&duplicate).is_err());
        }

        #[test]
        fn duplicate_json_keys_and_fractional_numbers_are_rejected() {
            let (_, sig, key) = sign(&index_value());
            assert!(verify_index_with_key(b"{\"schema\":1,\"schema\":2}", &sig, &key).is_err());
            assert!(canonical_bytes(&json!({"float":1.5})).is_err());
        }

        #[test]
        fn index_text_limits_match_publisher() {
            for (field, maximum) in [
                ("author", 120),
                ("description", 1000),
                ("productId", 128),
                ("license", 1000),
                ("changelog", 2000),
            ] {
                let mut value = index_value();
                value["skins"][0][field] = json!("字".repeat(maximum + 1));
                assert!(verify(&value).is_err(), "{field}");
            }
            let mut value = index_value();
            value["skins"][0]["name"]["zh"] = json!("字".repeat(81));
            assert!(verify(&value).is_err());
            let mut value = index_value();
            value["producer"] = json!("x".repeat(121));
            assert!(verify(&value).is_err());
            let mut value = index_value();
            value["skins"][0]["description"] = json!("a\u{0085}b");
            assert!(verify(&value).is_err());
        }

        #[test]
        fn python_fixture_signature_bundle_and_preview_interoperate() {
            let bytes = include_bytes!("../tests/fixtures/skin-store/skins-index.json");
            let sig = include_str!("../tests/fixtures/skin-store/skins-index.json.sig");
            let key = include_str!("../tests/fixtures/skin-store/public-key.hex");
            let index = verify_index_with_key(bytes, sig, key).unwrap();
            assert!(verify_index(bytes, sig).is_err());
            let skin = index
                .skins
                .iter()
                .find(|skin| skin.id == "teststore")
                .unwrap();
            let bundle = include_bytes!("../tests/fixtures/skin-store/teststore-1.0.0.zhskin.json");
            let files = decode_bundle(bundle, skin).unwrap();
            assert_eq!(files.len(), 1);
            assert_eq!(files[0].0, "skin.json");
            let preview = include_bytes!("../tests/fixtures/skin-store/preview.png");
            assert!(is_png(preview));
            assert!(sha256_hex(preview).eq_ignore_ascii_case(&skin.preview_sha256));
            let mut tampered = preview.to_vec();
            tampered[16] ^= 1;
            assert!(!is_png(&tampered));
        }

        fn bundle_value() -> Value {
            json!({"schema":"agent-ime-skin-bundle/1", "id":"teststore", "version":"1.0.0",
                "files":[{"path":"skin.json", "base64":base64_encode(b"{\"id\":\"teststore\"}")}]})
        }

        fn decode(value: &Value) -> Result<Vec<(String, Vec<u8>)>, String> {
            let bytes = serde_json::to_vec(value).unwrap();
            let mut skin: IndexSkin =
                serde_json::from_value(index_value()["skins"][0].clone()).unwrap();
            skin.bundle_sha256 = sha256_hex(&bytes);
            skin.bundle_bytes = bytes.len() as u64;
            decode_bundle(&bytes, &skin)
        }

        #[test]
        fn valid_bundle_is_decoded_and_sha_is_checked() {
            assert_eq!(
                decode(&bundle_value()).unwrap(),
                vec![("skin.json".into(), b"{\"id\":\"teststore\"}".to_vec())]
            );
            let bytes = serde_json::to_vec(&bundle_value()).unwrap();
            let mut skin: IndexSkin =
                serde_json::from_value(index_value()["skins"][0].clone()).unwrap();
            skin.bundle_bytes = bytes.len() as u64;
            assert!(decode_bundle(&bytes, &skin).is_err());
        }

        #[test]
        fn bundle_identity_schema_and_paths_are_checked() {
            for (field, invalid) in [
                ("id", json!("other")),
                ("version", json!("1.1.0")),
                ("schema", json!("wrong")),
            ] {
                let mut value = bundle_value();
                value[field] = invalid;
                assert!(decode(&value).is_err(), "{field}");
            }
            for invalid in [
                "../skin.json",
                "/skin.json",
                "dir/skin.json",
                "a\\skin.json",
                "a..json",
                "x.exe",
                "C:skin.json",
            ] {
                let mut value = bundle_value();
                value["files"][0]["path"] = json!(invalid);
                assert!(decode(&value).is_err(), "{invalid}");
            }
            let mut value = bundle_value();
            value["files"][0]["base64"] = json!(base64_encode(b"{\"id\":\"other\"}"));
            assert!(decode(&value).is_err());
            let mut value = bundle_value();
            value["files"][0]["base64"] =
                json!(base64_encode(b"{\"id\":\"teststore\",\"version\":\"9\"}"));
            assert!(decode(&value).is_err());
        }

        #[test]
        fn bundle_file_count_total_size_skin_json_size_and_png_are_checked() {
            let mut value = bundle_value();
            value["files"] = json!(vec![value["files"][0].clone(); 9]);
            assert!(decode(&value).is_err());
            let mut value = bundle_value();
            value["files"]
                .as_array_mut()
                .unwrap()
                .push(json!({"path":"x.png", "base64":base64_encode(b"not a PNG")}));
            assert!(decode(&value).is_err());
            let mut value = bundle_value();
            let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
            png.resize(1_572_864, 0);
            value["files"]
                .as_array_mut()
                .unwrap()
                .push(json!({"path":"x.png", "base64":base64_encode(&png)}));
            assert!(decode(&value).is_err());
            let mut value = bundle_value();
            value["files"][0]["base64"] = json!(base64_encode(&vec![b' '; 262_145]));
            assert!(decode(&value).is_err());
            let mut value = bundle_value();
            let duplicate = value["files"][0].clone();
            value["files"].as_array_mut().unwrap().push(duplicate);
            assert!(decode(&value).is_err());
        }

        #[test]
        fn numeric_versions_compare_with_zero_padding() {
            assert_eq!(compare_versions("1.10", "1.9").unwrap(), Ordering::Greater);
            assert_eq!(compare_versions("1.2", "1.2.0").unwrap(), Ordering::Equal);
            assert_eq!(compare_versions("0.3.24", "0.4").unwrap(), Ordering::Less);
            for invalid in ["", "1.", ".1", "1..2", "1.2-beta", "-1", "1 2"] {
                assert!(compare_versions(invalid, "1").is_err(), "{invalid}");
            }
        }

        #[test]
        fn download_urls_only_allow_clean_https() {
            assert!(validate_download_url("https://example.invalid/skins/test.png").is_ok());
            for invalid in [
                "http://example.invalid/a",
                "https://example.invalid/a?x=1",
                "https://example.invalid/a?",
                "https://example.invalid/a#",
                "https://user@example.invalid/a",
                "https://@example.invalid/a",
                "https://example.invalid/a\n",
                "https:///a",
                "https://example.invalid:bad/a",
                &format!("https://example.invalid/{}", "x".repeat(2048)),
            ] {
                assert!(validate_download_url(invalid).is_err(), "{invalid}");
            }
        }

        #[test]
        fn base64_is_strict_and_round_trips() {
            for bytes in [
                &b""[..],
                &b"a"[..],
                &b"ab"[..],
                &b"abc"[..],
                &[0, 255, 1][..],
            ] {
                assert_eq!(base64_decode(&base64_encode(bytes)).unwrap(), bytes);
            }
            for invalid in [
                "a", "====", "a===", "YQ", "YQ=", "YQ==\n", "YR==", "YWJ=", "YQ==AAAA",
            ] {
                assert!(base64_decode(invalid).is_err(), "{invalid}");
            }
        }
    }
}
