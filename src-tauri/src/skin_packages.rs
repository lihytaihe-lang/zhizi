use crate::skin_catalog::{is_candidate_skin_id, CANDIDATE_SKIN_CATALOG};
use serde::Serialize;
use serde_json::{Map, Value};
use std::collections::HashSet;
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
use std::fs::OpenOptions;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, PermissionsExt};

pub const MAXIMUM_PACKAGES: usize = 128;
pub const MAXIMUM_FILES_PER_PACKAGE: usize = 8;
// SkinGalleryPackages.maximumExternalBytes is stricter than the built-in loader's 1.5 MiB.
pub const MAXIMUM_PACKAGE_BYTES: u64 = 1_536 * 1024;
pub const MAXIMUM_SKIN_JSON_BYTES: u64 = 256 * 1024;

static INSTALL_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static INSTALL_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CandidateSkinDisplayName {
    pub zh: String,
    pub en: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CandidateSkinEntry {
    pub id: String,
    pub display_name: CandidateSkinDisplayName,
    pub author: String,
    pub description: String,
    pub origin: String,
    pub status: String,
    pub reason: Option<String>,
    pub can_uninstall: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CandidateSkinIssue {
    pub package: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CandidateSkinCatalogSnapshot {
    pub skins: Vec<CandidateSkinEntry>,
    pub issues: Vec<CandidateSkinIssue>,
}

#[derive(Debug, Clone)]
struct ParsedSkin {
    id: String,
    display_name: CandidateSkinDisplayName,
    author: String,
    description: String,
    json: Vec<u8>,
    assets: Vec<(String, Vec<u8>)>,
}

pub fn default_skins_root() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME").ok_or_else(|| "无法确定用户目录。".to_string())?;
    Ok(PathBuf::from(home).join("Library/Application Support/AgentInputMethod/Skins"))
}

pub fn catalog_snapshot(
    current_theme: Option<&str>,
) -> Result<CandidateSkinCatalogSnapshot, String> {
    catalog_snapshot_at(&default_skins_root()?, current_theme)
}

pub fn available_candidate_skin_ids() -> Result<HashSet<String>, String> {
    let snapshot = catalog_snapshot(None)?;
    Ok(snapshot
        .skins
        .into_iter()
        .filter(|skin| skin.origin == "builtIn" || skin.status == "ready")
        .map(|skin| skin.id)
        .collect())
}

pub fn install(source: &Path) -> Result<CandidateSkinCatalogSnapshot, String> {
    let root = default_skins_root()?;
    install_at(source, &root)?;
    catalog_snapshot_at(&root, None)
}

pub fn uninstall(id: &str) -> Result<CandidateSkinCatalogSnapshot, String> {
    let root = default_skins_root()?;
    uninstall_at(id, &root)?;
    catalog_snapshot_at(&root, None)
}

fn built_in_entries() -> Vec<CandidateSkinEntry> {
    CANDIDATE_SKIN_CATALOG
        .iter()
        .map(|skin| CandidateSkinEntry {
            id: skin.id.into(),
            display_name: CandidateSkinDisplayName {
                zh: skin.display_name_zh.into(),
                en: skin.display_name_en.into(),
            },
            author: skin.author.into(),
            description: skin.description.into(),
            origin: "builtIn".into(),
            status: "ready".into(),
            reason: None,
            can_uninstall: false,
        })
        .collect()
}

fn catalog_snapshot_at(
    root: &Path,
    current_theme: Option<&str>,
) -> Result<CandidateSkinCatalogSnapshot, String> {
    let mut skins = built_in_entries();
    let mut issues = Vec::new();
    if root.exists() {
        verify_directory_path(root)?;
        let mut entries = fs::read_dir(root)
            .map_err(|error| format!("无法扫描皮肤目录：{error}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("无法扫描皮肤目录：{error}"))?;
        entries.sort_by_key(|entry| entry.file_name());
        if entries.len() > MAXIMUM_PACKAGES {
            return Err(format!("皮肤目录超过 {MAXIMUM_PACKAGES} 个包上限。"));
        }
        for entry in entries {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with(".install-") {
                continue;
            }
            match read_package(&entry.path(), Some(&name)) {
                Ok(parsed) => {
                    if is_candidate_skin_id(&parsed.id) {
                        let reason = "与内置皮肤 id 冲突；内置包优先。".to_string();
                        skins.push(CandidateSkinEntry {
                            id: parsed.id,
                            display_name: parsed.display_name,
                            author: parsed.author,
                            description: parsed.description,
                            origin: "external".into(),
                            status: "conflict".into(),
                            reason: Some(reason.clone()),
                            can_uninstall: true,
                        });
                        issues.push(CandidateSkinIssue {
                            package: name,
                            reason,
                        });
                    } else {
                        skins.push(CandidateSkinEntry {
                            id: parsed.id,
                            display_name: parsed.display_name,
                            author: parsed.author,
                            description: parsed.description,
                            origin: "external".into(),
                            status: "ready".into(),
                            reason: None,
                            can_uninstall: true,
                        });
                    }
                }
                Err(reason) => issues.push(CandidateSkinIssue {
                    package: name,
                    reason,
                }),
            }
        }
    }
    if let Some(current) = current_theme {
        let available = skins
            .iter()
            .any(|skin| skin.id == current && (skin.origin == "builtIn" || skin.status == "ready"));
        if !available && current != "system" {
            issues.push(CandidateSkinIssue {
                package: current.into(),
                reason: "当前皮肤不存在或校验失败；输入法已静默回落内置 system。".into(),
            });
        }
    }
    Ok(CandidateSkinCatalogSnapshot { skins, issues })
}

fn install_at(source: &Path, root: &Path) -> Result<String, String> {
    install_at_with(source, root, |_| Ok(()))
}

fn install_at_with(
    source: &Path,
    root: &Path,
    mut checkpoint: impl FnMut(&str) -> Result<(), String>,
) -> Result<String, String> {
    let _guard = INSTALL_LOCK
        .lock()
        .map_err(|_| "皮肤安装器正在忙。".to_string())?;
    verify_directory_path(source.parent().ok_or("皮肤包没有父目录。")?)?;
    let metadata =
        fs::symlink_metadata(source).map_err(|error| format!("无法读取所选皮肤包：{error}"))?;
    if metadata.file_type().is_symlink() {
        return Err("拒绝安装符号链接皮肤包。".into());
    }
    let parsed = if metadata.is_dir() {
        let name = source
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or("皮肤目录名称无效。")?;
        read_package(source, Some(name))?
    } else if metadata.is_file()
        && source.extension().and_then(|value| value.to_str()) == Some("json")
    {
        let json = read_bounded_regular(source, MAXIMUM_SKIN_JSON_BYTES)?;
        let parsed = parse_skin_json(json)?;
        let object: Value =
            serde_json::from_slice(&parsed.json).map_err(|error| error.to_string())?;
        validate_decorations(object.as_object().unwrap(), &[])?;
        parsed
    } else {
        return Err("请选择一个皮肤包目录或 .json 文件。".into());
    };
    if is_candidate_skin_id(&parsed.id) {
        return Err("不能与内置皮肤同名。".into());
    }
    ensure_directory_path(root)?;
    let destination = root.join(&parsed.id);
    let previous = match fs::symlink_metadata(&destination) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => Some(metadata),
        Ok(_) => return Err("拒绝覆盖不安全的皮肤包路径。".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(format!("无法检查已装皮肤：{error}")),
    };
    let previous_package = if previous.is_some() {
        Some(read_package(&destination, Some(&parsed.id))?)
    } else {
        None
    };
    let root_directory = SecurePackageDirectory::open(root)?;
    let package_count = root_directory.names(MAXIMUM_PACKAGES)?.len();
    if package_count > MAXIMUM_PACKAGES || (previous.is_none() && package_count == MAXIMUM_PACKAGES)
    {
        return Err(format!("皮肤目录超过 {MAXIMUM_PACKAGES} 个包上限。"));
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "系统时间无效。".to_string())?
        .as_nanos();
    let sequence = INSTALL_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    // Keep transient entries outside Skins, including when it already contains 128 packages.
    let staging = root
        .parent()
        .ok_or("皮肤目录没有父目录。")?
        .join(format!(".install-{}-{now}-{sequence}", std::process::id()));
    fs::create_dir(&staging).map_err(|error| format!("无法创建皮肤安装暂存目录：{error}"))?;
    let staging_identity = fs::symlink_metadata(&staging).map_err(|error| error.to_string())?;
    let mut created_files = Vec::new();
    let result = (|| {
        let staging_directory = SecurePackageDirectory::open(&staging)?;
        #[cfg(unix)]
        staging_directory
            .file
            .set_permissions(fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("无法设置皮肤安装目录权限：{error}"))?;
        for (name, bytes) in std::iter::once(("skin.json", &parsed.json)).chain(
            parsed
                .assets
                .iter()
                .map(|(name, bytes)| (name.as_str(), bytes)),
        ) {
            staging_directory.verify()?;
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            let mut file = secure_fs::create_at(&staging_directory.file, name)?;
            #[cfg(not(any(target_os = "macos", target_os = "linux")))]
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(staging.join(name))
                .map_err(|error| error.to_string())?;
            created_files.push((
                name.to_string(),
                file.metadata().map_err(|error| error.to_string())?,
            ));
            checkpoint("created")?;
            file.write_all(bytes)
                .and_then(|_| file.sync_all())
                .map_err(|error| format!("无法持久化 {name}：{error}"))?;
            checkpoint(name)?;
        }
        checkpoint("validate")?;
        checkpoint("activate")?;
        staging_directory.verify()?;
        let staged = read_package(&staging, Some(&parsed.id))?;
        if staged.json != parsed.json || staged.assets != parsed.assets {
            return Err("暂存皮肤在校验过程中发生变化。".into());
        }
        #[cfg(unix)]
        File::open(&staging)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| format!("无法持久化暂存目录：{error}"))?;
        staging_directory.verify()?;
        root_directory.verify()?;
        match (&previous, fs::symlink_metadata(&destination)) {
            (Some(before), Ok(after)) if same_file(before, &after) && after.is_dir() => {}
            (None, Err(error)) if error.kind() == std::io::ErrorKind::NotFound => {}
            _ => return Err("已装皮肤在更新过程中发生变化。".into()),
        }
        activate_package(&staging, &root_directory, &parsed.id, previous.is_some())?;
        // After the single atomic activation the new package is committed. Cleanup/fsync
        // errors cannot be reported as an installation failure with the old package gone.
        #[cfg(unix)]
        let _ = File::open(root).and_then(|directory| directory.sync_all());
        Ok(parsed.id.clone())
    })();
    // Delete only the bounded, validated files belonging to the expected directory.
    // A substituted path or newly added payload is never recursively removed.
    if result.is_ok() {
        if let (Some(identity), Some(package)) = (&previous, &previous_package) {
            let _ = remove_package_files(&staging, identity, package);
        }
    } else {
        let _ = remove_created_files(&staging, &staging_identity, &created_files);
    }
    result
}

fn activate_package(
    staging: &Path,
    root: &SecurePackageDirectory,
    id: &str,
    replacing: bool,
) -> Result<(), String> {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        use std::ffi::CString;
        use std::os::fd::AsRawFd;
        use std::os::unix::ffi::OsStrExt;
        let parent = SecurePackageDirectory::open(staging.parent().ok_or("暂存目录没有父目录。")?)?;
        parent.verify()?;
        root.verify()?;
        let from = CString::new(staging.file_name().ok_or("暂存目录名称无效。")?.as_bytes())
            .map_err(|_| "路径含空字节。")?;
        let to = CString::new(id).map_err(|_| "路径含空字节。")?;
        #[cfg(target_os = "macos")]
        let result = {
            extern "C" {
                fn renameatx_np(
                    from_fd: i32,
                    from: *const std::ffi::c_char,
                    to_fd: i32,
                    to: *const std::ffi::c_char,
                    flags: u32,
                ) -> i32;
            }
            // RENAME_SWAP / RENAME_EXCL: never expose a missing target or overwrite a new arrival.
            unsafe {
                renameatx_np(
                    parent.file.as_raw_fd(),
                    from.as_ptr(),
                    root.file.as_raw_fd(),
                    to.as_ptr(),
                    if replacing { 2 } else { 4 },
                )
            }
        };
        #[cfg(target_os = "linux")]
        let result = {
            extern "C" {
                fn renameat2(
                    from_fd: i32,
                    from: *const std::ffi::c_char,
                    to_fd: i32,
                    to: *const std::ffi::c_char,
                    flags: u32,
                ) -> i32;
            }
            unsafe {
                renameat2(
                    parent.file.as_raw_fd(),
                    from.as_ptr(),
                    root.file.as_raw_fd(),
                    to.as_ptr(),
                    if replacing { 2 } else { 1 },
                )
            }
        };
        if result != 0 {
            return Err(format!(
                "无法原子激活皮肤包：{}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(())
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        if replacing {
            return Err("此系统尚不支持原子替换皮肤目录。".into());
        }
        fs::rename(staging, root.path.join(id)).map_err(|error| format!("无法激活皮肤包：{error}"))
    }
}

fn uninstall_at(id: &str, root: &Path) -> Result<(), String> {
    let _guard = INSTALL_LOCK
        .lock()
        .map_err(|_| "皮肤安装器正在忙。".to_string())?;
    validate_id(id)?;
    if is_candidate_skin_id(id) {
        return Err("内置皮肤不能卸载。".into());
    }
    verify_directory_path(root)?;
    let package = root.join(id);
    let metadata = fs::symlink_metadata(&package).map_err(|_| format!("未安装外部皮肤 {id}。"))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err("拒绝卸载已被替换的皮肤包路径。".into());
    }
    let parsed = read_package(&package, Some(id))?;
    remove_package_files(&package, &metadata, &parsed)
}

fn remove_created_files(
    path: &Path,
    identity: &fs::Metadata,
    created: &[(String, fs::Metadata)],
) -> Result<(), String> {
    let directory = SecurePackageDirectory::open(path)?;
    if !same_file(
        identity,
        &directory
            .file
            .metadata()
            .map_err(|error| error.to_string())?,
    ) {
        return Err("拒绝清理已被替换的暂存目录。".into());
    }
    let names = directory.names(MAXIMUM_FILES_PER_PACKAGE)?;
    for name in &names {
        let expected = created
            .iter()
            .find(|(expected, _)| expected == name)
            .ok_or("暂存目录中出现未知文件。")?;
        let actual = fs::symlink_metadata(path.join(name)).map_err(|error| error.to_string())?;
        if !same_file(&expected.1, &actual) {
            return Err("暂存文件已被替换。".into());
        }
    }
    remove_known_names(&directory, names)
}

fn remove_package_files(
    path: &Path,
    identity: &fs::Metadata,
    package: &ParsedSkin,
) -> Result<(), String> {
    let directory = SecurePackageDirectory::open(path)?;
    if !same_file(
        identity,
        &directory
            .file
            .metadata()
            .map_err(|error| error.to_string())?,
    ) {
        return Err("拒绝删除已被替换的皮肤包目录。".into());
    }
    let names = directory.names(MAXIMUM_FILES_PER_PACKAGE)?;
    let files = std::iter::once(("skin.json", &package.json))
        .chain(
            package
                .assets
                .iter()
                .map(|(name, bytes)| (name.as_str(), bytes)),
        )
        .collect::<Vec<_>>();
    if names
        .iter()
        .any(|name| !files.iter().any(|(expected, _)| name == expected))
    {
        return Err("拒绝删除皮肤包中新出现的文件。".into());
    }
    // Finish every check before deleting the first file.
    for (name, bytes) in &files {
        if directory.read(name, MAXIMUM_PACKAGE_BYTES)? != **bytes {
            return Err("拒绝删除读取后发生变化的皮肤文件。".into());
        }
    }
    remove_known_names(&directory, names)
}

fn remove_known_names(
    directory: &SecurePackageDirectory,
    names: Vec<String>,
) -> Result<(), String> {
    let path = &directory.path;
    for name in names {
        directory.verify()?;
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        secure_fs::unlink_at(&directory.file, &name, false)?;
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        fs::remove_file(path.join(&name)).map_err(|error| error.to_string())?;
    }
    directory.verify()?;
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        let parent = SecurePackageDirectory::open(path.parent().ok_or("目录没有父目录。")?)?;
        parent.verify()?;
        secure_fs::unlink_at(
            &parent.file,
            path.file_name()
                .and_then(|name| name.to_str())
                .ok_or("目录名无效。")?,
            true,
        )
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        fs::remove_dir(path).map_err(|error| error.to_string())
    }
}

fn read_package(path: &Path, expected_directory: Option<&str>) -> Result<ParsedSkin, String> {
    verify_directory_path(path)?;
    let metadata =
        fs::symlink_metadata(path).map_err(|error| format!("无法检查皮肤包：{error}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err("包路径必须是普通目录，不能是符号链接。".into());
    }
    let directory = SecurePackageDirectory::open(path)?;
    let names = directory.names(MAXIMUM_FILES_PER_PACKAGE)?;
    let mut total_bytes = 0u64;
    let mut json = None;
    let mut assets = Vec::new();
    for name in &names {
        if name != "skin.json" && !decoration_asset_name(name) {
            return Err("只能包含 skin.json 和 PNG 图片。".into());
        }
        let limit = if name == "skin.json" {
            MAXIMUM_SKIN_JSON_BYTES
        } else {
            MAXIMUM_PACKAGE_BYTES
        };
        let bytes = directory.read(name, limit)?;
        total_bytes = total_bytes
            .checked_add(bytes.len() as u64)
            .ok_or_else(|| "单包体积溢出。".to_string())?;
        if total_bytes > MAXIMUM_PACKAGE_BYTES {
            return Err(format!("单包体积超过 {MAXIMUM_PACKAGE_BYTES} 字节上限。"));
        }
        if name == "skin.json" {
            json = Some(bytes);
        } else {
            assets.push((name.clone(), bytes));
        }
    }
    let json = json.ok_or("缺少 skin.json。")?;
    let mut parsed = parse_skin_json(json)?;
    if let Some(expected) = expected_directory {
        if parsed.id != expected {
            return Err(format!(
                "skin.json id '{}' 与包目录 '{}' 不一致。",
                parsed.id, expected
            ));
        }
    }
    let value: Value = serde_json::from_slice(&parsed.json).map_err(|error| error.to_string())?;
    validate_decorations(value.as_object().unwrap(), &assets)?;
    directory.verify()?;
    parsed.assets = assets;
    Ok(parsed)
}

fn read_bounded_regular(path: &Path, maximum: u64) -> Result<Vec<u8>, String> {
    let directory = SecurePackageDirectory::open(path.parent().ok_or("文件没有父目录。")?)?;
    directory.read(
        path.file_name()
            .and_then(|name| name.to_str())
            .ok_or("文件名无效。")?,
        maximum,
    )
}

fn parse_skin_json(json: Vec<u8>) -> Result<ParsedSkin, String> {
    let value: Value =
        serde_json::from_slice(&json).map_err(|error| format!("JSON 语法错误：{error}"))?;
    let object = value
        .as_object()
        .ok_or_else(|| "skin.json 根节点必须是对象。".to_string())?;
    validate_allowed_fields(object)?;
    let id = required_string(object, "id", 64)?;
    validate_id(&id)?;
    let format_version = object
        .get("formatVersion")
        .and_then(Value::as_f64)
        .ok_or_else(|| "缺少必填字段 formatVersion。".to_string())?;
    if format_version != 1.0 {
        return Err("formatVersion 必须为 1。".into());
    }
    let display = object
        .get("displayName")
        .and_then(Value::as_object)
        .ok_or_else(|| "缺少必填字段 displayName。".to_string())?;
    let display_name = CandidateSkinDisplayName {
        zh: required_string(display, "zh", 80)?,
        en: required_string(display, "en", 80)?,
    };
    let author = required_string(object, "author", 120)?;
    let description = required_string_allow_empty(object, "description", 1000)?;
    validate_optional_style(object)?;
    Ok(ParsedSkin {
        id,
        display_name,
        author,
        description,
        json,
        assets: Vec::new(),
    })
}

fn validate_allowed_fields(object: &Map<String, Value>) -> Result<(), String> {
    const ALLOWED: &[&str] = &[
        "id",
        "displayName",
        "formatVersion",
        "author",
        "description",
        "candidateFont",
        "indexFont",
        "candidateColor",
        "secondaryCandidateColor",
        "indexColor",
        "highlightedCandidateColor",
        "highlightFillColor",
        "highlightFillOpacity",
        "highlightBorderColor",
        "highlightBorderOpacity",
        "rowHeight",
        "horizontalPadding",
        "verticalPadding",
        "indexCandidateSpacing",
        "candidateSpacing",
        "rowSpacing",
        "capsuleCornerRadius",
        "panelCornerRadius",
        "edgeInsets",
        "material",
        "minimumPanelSize",
        "maximumPanelWidth",
        "widthQuantization",
        "panelBackground",
        "panelBorderColor",
        "panelBorderWidth",
        "shadowEnabled",
        "preeditRow",
        "indexSuffix",
        "decorations",
        "overflow",
        "slotColors",
    ];
    let unknown = object
        .keys()
        .filter(|key| !ALLOWED.contains(&key.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    if unknown.is_empty() {
        Ok(())
    } else {
        Err(format!("P1 不支持字段 {}。", unknown.join(", ")))
    }
}

fn validate_optional_style(object: &Map<String, Value>) -> Result<(), String> {
    for field in ["candidateFont", "indexFont"] {
        if let Some(value) = object.get(field).filter(|value| !value.is_null()) {
            let font = value
                .as_object()
                .ok_or_else(|| format!("{field} 必须是对象。"))?;
            let family = required_string(font, "family", 40)?;
            const SYSTEM_FAMILIES: [&str; 13] = [
                "PingFang SC",
                "systemMonospacedDigit",
                "Kaiti SC",
                "Songti SC",
                "Yuanti SC",
                "Xingkai SC",
                "Hannotate SC",
                "Hanzipen SC",
                "Wawati SC",
                "Weibei SC",
                "Libian SC",
                "Baoli SC",
                "Yuppy SC",
            ];
            if !SYSTEM_FAMILIES.contains(&family.as_str()) {
                return Err(format!("P1 不支持字体 {family}。"));
            }
            let weight = required_string(font, "weight", 20)?;
            if !["regular", "medium", "semibold", "bold"].contains(&weight.as_str()) {
                return Err(format!("{field}.weight 无效。"));
            }
            validate_number(
                font.get("size"),
                &format!("{field}.size"),
                f64::from_bits(1),
                96.0,
                false,
            )?;
        }
    }
    for field in [
        "candidateColor",
        "secondaryCandidateColor",
        "indexColor",
        "highlightedCandidateColor",
        "highlightFillColor",
        "highlightBorderColor",
    ] {
        if let Some(value) = object.get(field).filter(|value| !value.is_null()) {
            validate_color(value, field, false)?;
        }
    }
    for field in ["panelBackground", "panelBorderColor"] {
        if let Some(value) = object.get(field) {
            validate_color(value, field, true)?;
        }
    }
    for field in ["highlightFillOpacity", "highlightBorderOpacity"] {
        if object.contains_key(field) {
            validate_number(object.get(field), field, 0.0, 1.0, true)?;
        }
    }
    for field in [
        "rowHeight",
        "horizontalPadding",
        "verticalPadding",
        "indexCandidateSpacing",
        "candidateSpacing",
        "rowSpacing",
        "capsuleCornerRadius",
        "panelCornerRadius",
        "maximumPanelWidth",
        "widthQuantization",
    ] {
        if object.contains_key(field) {
            validate_number(object.get(field), field, 0.0, 4096.0, true)?;
        }
    }
    if object.contains_key("panelBorderWidth") {
        validate_number(
            object.get("panelBorderWidth"),
            "panelBorderWidth",
            0.0,
            4096.0,
            true,
        )?;
    }
    if let Some(value) = object.get("material").filter(|value| !value.is_null()) {
        let material = value
            .as_str()
            .ok_or_else(|| "material 必须是字符串。".to_string())?;
        if !["popover", "hudWindow", "glass", "none"].contains(&material) {
            return Err("material 只支持 popover/hudWindow/glass/none。".into());
        }
    }
    if let Some(value) = object.get("shadowEnabled").filter(|value| !value.is_null()) {
        if !value.is_boolean() {
            return Err("shadowEnabled 必须是布尔值。".into());
        }
    }
    if let Some(value) = object.get("preeditRow").filter(|value| !value.is_null()) {
        if !value.is_boolean() {
            return Err("preeditRow 必须是布尔值。".into());
        }
    }
    if let Some(value) = object.get("indexSuffix").filter(|value| !value.is_null()) {
        if value.as_str() != Some("") && value.as_str() != Some(".") {
            return Err("indexSuffix 只能是空或 \".\"。".into());
        }
    }
    if let Some(value) = object.get("edgeInsets").filter(|value| !value.is_null()) {
        validate_numeric_object(value, "edgeInsets", &["top", "left", "bottom", "right"])?;
    }
    if let Some(value) = object
        .get("minimumPanelSize")
        .filter(|value| !value.is_null())
    {
        validate_numeric_object(value, "minimumPanelSize", &["width", "height"])?;
    }
    let border_color = object
        .get("panelBorderColor")
        .is_some_and(|value| !value.is_null());
    let border_width = object
        .get("panelBorderWidth")
        .is_some_and(|value| !value.is_null());
    if border_color != border_width {
        return Err("panelBorderColor 与 panelBorderWidth 必须同时为空或同时有值。".into());
    }
    if let Some(value) = object.get("overflow") {
        let insets = value.as_object().ok_or("overflow 必须是对象。")?;
        if insets.len() != 4 {
            return Err("overflow 必须含 top/left/bottom/right。".into());
        }
        for field in ["top", "left", "bottom", "right"] {
            validate_number(insets.get(field), field, 0.0, 120.0, false)?;
        }
    }
    if let Some(value) = object.get("slotColors") {
        let colors = value
            .as_array()
            .filter(|colors| colors.len() == 5)
            .ok_or("slotColors 必须是 5 个 light/dark 颜色。")?;
        for color in colors {
            if !color.as_object().is_some_and(|color| {
                color.len() == 2 && color.contains_key("light") && color.contains_key("dark")
            }) {
                return Err("slotColors 必须是 light/dark 颜色。".into());
            }
            validate_color(color, "slotColors", false)?;
        }
    }
    Ok(())
}

fn validate_numeric_object(value: &Value, label: &str, fields: &[&str]) -> Result<(), String> {
    let object = value
        .as_object()
        .ok_or_else(|| format!("{label} 必须是对象。"))?;
    if fields.iter().any(|key| !object.contains_key(*key)) {
        return Err(format!("{label} 字段不完整。"));
    }
    for field in fields {
        validate_number(
            object.get(*field),
            &format!("{label}.{field}"),
            0.0,
            4096.0,
            false,
        )?;
    }
    Ok(())
}

fn validate_color(value: &Value, label: &str, nullable: bool) -> Result<(), String> {
    if nullable && value.is_null() {
        return Ok(());
    }
    if let Some(semantic) = value.as_str() {
        if [
            "system:clear",
            "system:labelColor",
            "system:secondaryLabelColor",
            "system:tertiaryLabelColor",
            "system:controlAccentColor",
            "system:systemIndigo",
            "system:systemTeal",
        ]
        .contains(&semantic)
        {
            return Ok(());
        }
        return Err(format!("{label} 使用了不支持的系统语义色。"));
    }
    let colors = value
        .as_object()
        .ok_or_else(|| format!("{label} 必须是 light/dark 颜色对象。"))?;
    if !colors.contains_key("light") || !colors.contains_key("dark") {
        return Err(format!("{label} 必须同时包含 light/dark。"));
    }
    for appearance in ["light", "dark"] {
        let color = colors
            .get(appearance)
            .and_then(Value::as_str)
            .ok_or_else(|| format!("{label}.{appearance} 必须是字符串。"))?;
        let valid_length = color.len() == 7 || color.len() == 9;
        if !valid_length
            || !color.starts_with('#')
            || !color[1..]
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'A'..=b'F').contains(&byte))
        {
            return Err(format!("{label}.{appearance} 必须为大写 #RRGGBB[AA]。"));
        }
    }
    Ok(())
}

fn validate_number(
    value: Option<&Value>,
    label: &str,
    minimum: f64,
    maximum: f64,
    nullable: bool,
) -> Result<(), String> {
    if nullable && value.is_some_and(Value::is_null) {
        return Ok(());
    }
    let number = value
        .and_then(Value::as_f64)
        .filter(|number| number.is_finite() && *number >= minimum && *number <= maximum)
        .ok_or_else(|| format!("{label} 必须在 {minimum}..{maximum} 范围内。"))?;
    let _ = number;
    Ok(())
}

fn required_string(
    object: &Map<String, Value>,
    field: &str,
    maximum: usize,
) -> Result<String, String> {
    let value = required_string_allow_empty(object, field, maximum)?;
    if value
        .trim_matches(|character: char| character.is_whitespace() || character == '\u{200b}')
        .is_empty()
    {
        Err(format!("必填字段 {field} 不能为空。"))
    } else {
        Ok(value)
    }
}

fn required_string_allow_empty(
    object: &Map<String, Value>,
    field: &str,
    maximum: usize,
) -> Result<String, String> {
    let value = object
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("缺少必填字段 {field}。"))?;
    if skin_character_count(value)? > maximum {
        return Err(format!("{field} 超过 {maximum} 字符上限。"));
    }
    Ok(value.into())
}

fn skin_character_count(value: &str) -> Result<usize, String> {
    #[cfg(target_os = "macos")]
    {
        use std::ffi::{c_char, c_void};
        #[link(name = "icucore")]
        extern "C" {
            fn ubrk_open(
                kind: i32,
                locale: *const c_char,
                text: *const u16,
                length: i32,
                status: *mut i32,
            ) -> *mut c_void;
            fn ubrk_first(iterator: *mut c_void) -> i32;
            fn ubrk_next(iterator: *mut c_void) -> i32;
            fn ubrk_close(iterator: *mut c_void);
        }
        let text: Vec<u16> = value.encode_utf16().collect();
        let length = i32::try_from(text.len()).map_err(|_| "皮肤文字长度超过限制。".to_string())?;
        unsafe {
            // ICU character boundaries, unlike CF composed ranges, follow extended graphemes.
            let mut status = 0;
            let iterator = ubrk_open(
                0,
                b"root\0".as_ptr().cast(),
                text.as_ptr(),
                length,
                &mut status,
            );
            if iterator.is_null() || status > 0 {
                if !iterator.is_null() {
                    ubrk_close(iterator);
                }
                return Err("无法检查皮肤文字长度。".into());
            }
            if ubrk_first(iterator) != 0 {
                ubrk_close(iterator);
                return Err("无法检查皮肤文字长度。".into());
            }
            let mut count = 0;
            while ubrk_next(iterator) != -1 {
                count += 1;
            }
            ubrk_close(iterator);
            Ok(count)
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(value.chars().count())
    }
}

fn validate_id(id: &str) -> Result<(), String> {
    let bytes = id.as_bytes();
    let valid = !bytes.is_empty()
        && bytes.len() <= 64
        && bytes[0].is_ascii_lowercase()
        && bytes.iter().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(byte)
        });
    if valid {
        Ok(())
    } else {
        Err("皮肤 id 必须匹配 ^[a-z][a-z0-9._-]{0,63}$，不允许路径穿越。".into())
    }
}

fn ensure_directory_path(path: &Path) -> Result<(), String> {
    if path.exists() {
        return verify_directory_path(path);
    }
    let parent = path
        .parent()
        .ok_or_else(|| "皮肤目录没有父目录。".to_string())?;
    if !parent.exists() {
        return Err("皮肤目录的父目录不存在。".into());
    }
    verify_directory_path(parent)?;
    fs::create_dir(path).map_err(|error| format!("无法创建皮肤目录：{error}"))?;
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("无法设置皮肤目录权限：{error}"))?;
    verify_directory_path(path)
}

fn verify_directory_path(path: &Path) -> Result<(), String> {
    let path = package_absolute_path(path)?;
    let mut current = path
        .ancestors()
        .last()
        .ok_or("目录没有根路径。")?
        .to_path_buf();
    for component in path
        .components()
        .filter(|part| matches!(part, Component::Normal(_)))
    {
        current.push(component);
        let metadata =
            fs::symlink_metadata(&current).map_err(|error| format!("无法检查皮肤目录：{error}"))?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err("皮肤目录不是安全的普通目录。".into());
        }
    }
    Ok(())
}

fn package_absolute_path(path: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute()
        || path.as_os_str().as_encoded_bytes().contains(&0)
        || path
            .as_os_str()
            .as_encoded_bytes()
            .split(|byte| *byte == b'/')
            .any(|part| part == b"." || part == b"..")
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
    {
        return Err("皮肤目录包含路径穿越或不是绝对路径。".into());
    }
    #[cfg(target_os = "macos")]
    for alias in ["/tmp", "/var"] {
        if let Ok(tail) = path.strip_prefix(alias) {
            return Ok(Path::new("/private")
                .join(alias.trim_start_matches('/'))
                .join(tail));
        }
    }
    Ok(path.into())
}

fn same_file(before: &fs::Metadata, after: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        before.dev() == after.dev()
            && before.ino() == after.ino()
            && before.uid() == after.uid()
            && before.file_type() == after.file_type()
    }
    #[cfg(not(unix))]
    {
        before.file_type() == after.file_type()
            && before.len() == after.len()
            && before.created().ok() == after.created().ok()
    }
}

struct SecurePackageDirectory {
    path: PathBuf,
    file: File,
    bindings: Vec<(PathBuf, fs::Metadata)>,
}

fn open_directory_handle(path: &Path) -> Result<File, String> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        OpenOptions::new()
            .read(true)
            .custom_flags(0x0200_0000 | 0x0020_0000)
            .open(path)
            .map_err(|error| error.to_string())
    }
    #[cfg(not(windows))]
    {
        File::open(path).map_err(|error| error.to_string())
    }
}

impl SecurePackageDirectory {
    fn open(path: &Path) -> Result<Self, String> {
        let path = package_absolute_path(path)?;
        verify_directory_path(&path)?;
        let mut spelling = path
            .ancestors()
            .last()
            .ok_or("目录没有根路径。")?
            .to_path_buf();
        let mut current = open_directory_handle(&spelling)?;
        let mut bindings = Vec::new();
        for component in path.components().filter_map(|part| match part {
            Component::Normal(name) => Some(name),
            _ => None,
        }) {
            spelling.push(component);
            let before = fs::symlink_metadata(&spelling).map_err(|error| error.to_string())?;
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            let next = secure_fs::open_at(&current, component, secure_fs::DIRECTORY)?;
            #[cfg(not(any(target_os = "macos", target_os = "linux")))]
            let next = open_directory_handle(&spelling)?;
            let opened = next.metadata().map_err(|error| error.to_string())?;
            if !opened.is_dir() || !same_file(&before, &opened) {
                return Err("皮肤目录在打开过程中发生变化。".into());
            }
            bindings.push((spelling.clone(), opened));
            current = next;
        }
        #[cfg(unix)]
        {
            extern "C" {
                fn geteuid() -> u32;
            }
            if path != Path::new("/")
                && current.metadata().map_err(|error| error.to_string())?.uid()
                    != unsafe { geteuid() }
            {
                return Err("皮肤目录不属于当前用户。".into());
            }
        }
        let directory = Self {
            path,
            file: current,
            bindings,
        };
        directory.verify()?;
        Ok(directory)
    }

    fn verify(&self) -> Result<(), String> {
        for (path, before) in &self.bindings {
            let after = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
            if !after.is_dir() || !same_file(before, &after) {
                return Err("皮肤目录在读取过程中发生变化。".into());
            }
        }
        Ok(())
    }

    fn names(&self, maximum: usize) -> Result<Vec<String>, String> {
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        let mut names = secure_fs::names(&self.file, maximum)?;
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        let mut names = fs::read_dir(&self.path)
            .map_err(|error| error.to_string())?
            .take(maximum + 1)
            .map(|entry| {
                entry
                    .map_err(|error| error.to_string())?
                    .file_name()
                    .into_string()
                    .map_err(|_| "文件名无效。".to_string())
            })
            .collect::<Result<Vec<_>, _>>()?;
        if names.len() > maximum {
            return Err(format!("单包文件数超过 {maximum} 上限。"));
        }
        names.sort();
        if names.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err("皮肤包含重复文件名。".into());
        }
        self.verify()?;
        Ok(names)
    }

    fn read(&self, name: &str, maximum: u64) -> Result<Vec<u8>, String> {
        if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\0']) {
            return Err("文件名包含路径穿越。".into());
        }
        self.verify()?;
        let path = self.path.join(name);
        let before = fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
        if before.file_type().is_symlink() {
            return Err(format!("包内 {name} 是指向目录外的符号链接，已拒绝。"));
        }
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        let file = secure_fs::open_at(&self.file, name.as_ref(), secure_fs::NONBLOCK)?;
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        let file = File::open(&path).map_err(|error| error.to_string())?;
        let opened = file.metadata().map_err(|error| error.to_string())?;
        if !opened.is_file() || !same_file(&before, &opened) {
            return Err("包内文件必须是稳定的普通文件。".into());
        }
        #[cfg(unix)]
        {
            extern "C" {
                fn geteuid() -> u32;
            }
            if opened.nlink() != 1 || opened.uid() != unsafe { geteuid() } {
                return Err("包内文件必须属于当前用户且不能是硬链接。".into());
            }
        }
        if opened.len() > maximum {
            return Err(format!("{name} 超过 {maximum} 字节上限。"));
        }
        let mut bytes = Vec::with_capacity(opened.len() as usize);
        (&file)
            .take(maximum + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| error.to_string())?;
        let after = file.metadata().map_err(|error| error.to_string())?;
        let bound = fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
        let stable = same_file(&opened, &after)
            && same_file(&after, &bound)
            && opened.len() == after.len()
            && after.len() == bytes.len() as u64
            && bytes.len() as u64 <= maximum;
        #[cfg(unix)]
        let stable = stable
            && opened.mode() == after.mode()
            && after.mode() == bound.mode()
            && after.nlink() == 1
            && bound.nlink() == 1;
        if !stable {
            return Err(format!("{name} 在读取过程中发生变化。"));
        }
        self.verify()?;
        Ok(bytes)
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
mod secure_fs {
    use super::*;
    use std::ffi::{c_char, CStr, CString, OsStr};
    use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd};
    use std::os::unix::ffi::OsStrExt;
    #[cfg(target_os = "macos")]
    pub const DIRECTORY: i32 = 0x100000;
    #[cfg(target_os = "linux")]
    pub const DIRECTORY: i32 = 0x10000;
    #[cfg(target_os = "macos")]
    pub const NONBLOCK: i32 = 4;
    #[cfg(target_os = "linux")]
    pub const NONBLOCK: i32 = 0x800;
    #[cfg(target_os = "macos")]
    const SAFE: i32 = 0x100 | 0x1000000; // O_NOFOLLOW | O_CLOEXEC
    #[cfg(target_os = "linux")]
    const SAFE: i32 = 0x20000 | 0x80000;
    #[cfg_attr(target_os = "macos", repr(C, packed(4)))]
    #[cfg_attr(target_os = "linux", repr(C))]
    struct Dirent {
        inode: u64,
        offset: u64,
        length: u16,
        #[cfg(target_os = "macos")]
        name_length: u16,
        kind: u8,
        #[cfg(target_os = "macos")]
        name: [c_char; 1024],
        #[cfg(target_os = "linux")]
        name: [c_char; 256],
    }
    #[repr(C)]
    struct DirectoryStream {
        _private: [u8; 0],
    }
    extern "C" {
        fn openat(fd: i32, path: *const c_char, flags: i32, ...) -> i32;
        fn unlinkat(fd: i32, path: *const c_char, flags: i32) -> i32;
        fn fdopendir(fd: i32) -> *mut DirectoryStream;
        #[cfg_attr(
            all(target_os = "macos", target_arch = "x86_64"),
            link_name = "readdir$INODE64"
        )]
        fn readdir(directory: *mut DirectoryStream) -> *mut Dirent;
        fn closedir(directory: *mut DirectoryStream) -> i32;
        #[cfg(target_os = "macos")]
        fn __error() -> *mut i32;
        #[cfg(target_os = "linux")]
        fn __errno_location() -> *mut i32;
    }
    pub fn open_at(parent: &File, name: &OsStr, flags: i32) -> Result<File, String> {
        let name = CString::new(name.as_bytes()).map_err(|_| "文件名含空字节。")?;
        let fd = unsafe { openat(parent.as_raw_fd(), name.as_ptr(), SAFE | flags) };
        if fd < 0 {
            return Err(format!(
                "无法安全打开包内文件：{}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(unsafe { File::from_raw_fd(fd) })
    }
    pub fn create_at(parent: &File, name: &str) -> Result<File, String> {
        let name = CString::new(name).map_err(|_| "文件名含空字节。")?;
        #[cfg(target_os = "macos")]
        let create_flags = 0x200 | 0x800; // O_CREAT | O_EXCL
        #[cfg(target_os = "linux")]
        let create_flags = 0x40 | 0x80;
        let fd = unsafe {
            openat(
                parent.as_raw_fd(),
                name.as_ptr(),
                SAFE | create_flags | 1,
                0o600u32,
            )
        };
        if fd < 0 {
            return Err(format!(
                "无法创建皮肤素材：{}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(unsafe { File::from_raw_fd(fd) })
    }
    pub fn unlink_at(parent: &File, name: &str, directory: bool) -> Result<(), String> {
        let name = CString::new(name).map_err(|_| "文件名含空字节。")?;
        #[cfg(target_os = "macos")]
        let remove_directory = 0x80;
        #[cfg(target_os = "linux")]
        let remove_directory = 0x200;
        let result = unsafe {
            unlinkat(
                parent.as_raw_fd(),
                name.as_ptr(),
                if directory { remove_directory } else { 0 },
            )
        };
        if result != 0 {
            return Err(format!(
                "无法删除皮肤文件：{}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(())
    }
    pub fn names(parent: &File, maximum: usize) -> Result<Vec<String>, String> {
        let enumeration = open_at(parent, OsStr::new("."), DIRECTORY)?;
        let fd = enumeration.into_raw_fd();
        let stream = unsafe { fdopendir(fd) };
        if stream.is_null() {
            drop(unsafe { File::from_raw_fd(fd) });
            return Err("无法扫描皮肤包目录。".into());
        }
        struct Stream(*mut DirectoryStream);
        impl Drop for Stream {
            fn drop(&mut self) {
                unsafe {
                    closedir(self.0);
                }
            }
        }
        let stream = Stream(stream);
        let mut names = Vec::new();
        loop {
            #[cfg(target_os = "macos")]
            let errno = unsafe { __error() };
            #[cfg(target_os = "linux")]
            let errno = unsafe { __errno_location() };
            unsafe {
                *errno = 0;
            }
            let entry = unsafe { readdir(stream.0) };
            if entry.is_null() {
                if unsafe { *errno } != 0 {
                    return Err("扫描皮肤包目录失败。".into());
                }
                break;
            }
            let name = unsafe { CStr::from_ptr((*entry).name.as_ptr()) }
                .to_str()
                .map_err(|_| "包内文件名无效。")?;
            if name == "." || name == ".." {
                continue;
            }
            if names.len() == maximum {
                return Err(format!("单包文件数超过 {maximum} 上限。"));
            }
            names.push(name.to_string());
        }
        Ok(names)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn temporary_root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "agent-ime-skins-{label}-{}-{}",
            std::process::id(),
            INSTALL_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn minimal(id: &str) -> Vec<u8> {
        format!(
            r##"{{"id":"{id}","displayName":{{"zh":"测试","en":"Test"}},"formatVersion":1,"author":"Owner","description":"minimal","highlightFillColor":{{"light":"#FF0000","dark":"#00FF00"}}}}"##
        )
        .into_bytes()
    }

    fn write_package(root: &Path, id: &str, bytes: &[u8]) {
        let package = root.join(id);
        fs::create_dir_all(&package).unwrap();
        fs::write(package.join("skin.json"), bytes).unwrap();
    }

    #[test]
    fn minimal_package_uses_optional_style_and_is_available_without_rebuild() {
        let root = temporary_root("minimal");
        fs::create_dir_all(&root).unwrap();
        write_package(&root, "owner-red", &minimal("owner-red"));
        let snapshot = catalog_snapshot_at(&root, Some("owner-red")).unwrap();
        assert!(snapshot
            .skins
            .iter()
            .any(|skin| skin.id == "owner-red" && skin.status == "ready"));
        assert!(snapshot.issues.is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn four_bad_package_classes_return_readable_reasons() {
        let root = temporary_root("bad");
        fs::create_dir_all(&root).unwrap();
        write_package(&root, "bad-json", b"{not-json");
        write_package(
            &root,
            "missing-required",
            br#"{"id":"missing-required","formatVersion":1}"#,
        );
        let oversized = root.join("oversized");
        fs::create_dir_all(&oversized).unwrap();
        fs::write(
            oversized.join("skin.json"),
            vec![b' '; MAXIMUM_SKIN_JSON_BYTES as usize + 1],
        )
        .unwrap();
        #[cfg(unix)]
        {
            let linked = root.join("linked");
            fs::create_dir_all(&linked).unwrap();
            std::os::unix::fs::symlink("/private/tmp/outside-skin.json", linked.join("skin.json"))
                .unwrap();
        }
        let snapshot = catalog_snapshot_at(&root, None).unwrap();
        let joined = snapshot
            .issues
            .iter()
            .map(|issue| format!("{}:{}", issue.package, issue.reason))
            .collect::<Vec<_>>()
            .join("\n");
        println!("SKIN-PACKAGE-REVERSE\n{joined}");
        assert!(joined.contains("bad-json:JSON 语法错误"));
        assert!(joined.contains("missing-required:缺少必填字段 displayName"));
        assert!(joined.contains("oversized:skin.json 超过"));
        #[cfg(unix)]
        assert!(joined.contains("linked:包内 skin.json 是指向目录外的符号链接"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn built_in_conflict_is_marked_and_built_in_remains_first() {
        let root = temporary_root("conflict");
        fs::create_dir_all(&root).unwrap();
        write_package(&root, "system", &minimal("system"));
        let snapshot = catalog_snapshot_at(&root, None).unwrap();
        let system = snapshot
            .skins
            .iter()
            .filter(|skin| skin.id == "system")
            .collect::<Vec<_>>();
        assert_eq!(system.len(), 2);
        assert_eq!(system[0].origin, "builtIn");
        assert_eq!(system[1].status, "conflict");
        assert!(system[1].reason.as_deref().unwrap().contains("内置包优先"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn install_json_and_uninstall_are_bounded() {
        let parent = temporary_root("install");
        fs::create_dir_all(&parent).unwrap();
        let source = parent.join("owner-blue.json");
        fs::write(&source, minimal("owner-blue")).unwrap();
        let root = parent.join("Skins");
        assert_eq!(install_at(&source, &root).unwrap(), "owner-blue");
        assert!(root.join("owner-blue/skin.json").is_file());
        uninstall_at("owner-blue", &root).unwrap();
        assert!(!root.join("owner-blue").exists());
        fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn every_builtin_package_parses_and_installs_as_an_external_copy() {
        let parent = temporary_root("builtin-assets");
        fs::create_dir_all(&parent).unwrap();
        let builtin = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../native/resources/Skins/Builtin")
            .canonicalize()
            .unwrap();
        let mut count = 0;
        let mut rejected = Vec::new();
        for entry in fs::read_dir(builtin).unwrap() {
            let entry = entry.unwrap();
            let name = entry.file_name().into_string().unwrap();
            count += 1;
            if let Err(reason) = read_package(&entry.path(), Some(&name)) {
                rejected.push(format!("{name}: {reason}"));
                continue;
            }
            let external_id = format!("test-{name}");
            let source = parent.join(&external_id);
            fs::create_dir(&source).unwrap();
            for file in fs::read_dir(entry.path()).unwrap() {
                let file = file.unwrap();
                let mut bytes = fs::read(file.path()).unwrap();
                if file.file_name() == "skin.json" {
                    let mut value: Value = serde_json::from_slice(&bytes).unwrap();
                    value["id"] = Value::String(external_id.clone());
                    bytes = serde_json::to_vec(&value).unwrap();
                }
                fs::write(source.join(file.file_name()), bytes).unwrap();
            }
            install_at(&source, &parent.join("Skins")).unwrap();
            for file in fs::read_dir(&source).unwrap() {
                let file = file.unwrap();
                assert_eq!(
                    fs::read(file.path()).unwrap(),
                    fs::read(
                        parent
                            .join("Skins")
                            .join(&external_id)
                            .join(file.file_name())
                    )
                    .unwrap()
                );
            }
            uninstall_at(&external_id, &parent.join("Skins")).unwrap();
        }
        assert_eq!(count, 13);
        fs::remove_dir_all(parent).unwrap();
        assert!(
            rejected.is_empty(),
            "内置包未全部通过外置规则：{}",
            rejected.join("; ")
        );
    }

    #[test]
    fn same_id_install_replaces_the_complete_package() {
        let parent = temporary_root("update");
        fs::create_dir_all(&parent).unwrap();
        let source = parent.join("owner.json");
        let root = parent.join("Skins");
        fs::write(&source, minimal("owner-blue")).unwrap();
        install_at(&source, &root).unwrap();
        let mut value: Value = serde_json::from_slice(&minimal("owner-blue")).unwrap();
        value["description"] = Value::String("updated".into());
        let updated = serde_json::to_vec(&value).unwrap();
        fs::write(&source, &updated).unwrap();
        install_at(&source, &root).unwrap();
        assert_eq!(
            fs::read(root.join("owner-blue/skin.json")).unwrap(),
            updated
        );
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
        fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn overflow_limits_match_swift() {
        let with_overflow = |top: f64| {
            let mut value: Value = serde_json::from_slice(&minimal("owner-blue")).unwrap();
            value["overflow"] = serde_json::json!({"top": top, "left": 120.0, "bottom": 0.0, "right": 0.0});
            serde_json::to_vec(&value).unwrap()
        };
        parse_skin_json(with_overflow(120.0)).unwrap();
        parse_skin_json(with_overflow(64.0)).unwrap();
        assert!(parse_skin_json(with_overflow(121.0))
            .unwrap_err()
            .contains("top 必须在 0..120"));
    }

    #[test]
    fn swift_optional_nulls_and_nested_unknown_fields_are_accepted() {
        let mut value: Value = serde_json::from_slice(&minimal("owner-blue")).unwrap();
        value["candidateFont"] = Value::Null;
        value["rowHeight"] = Value::Null;
        value["shadowEnabled"] = Value::Null;
        value["material"] = Value::String("none".into());
        value["displayName"]["extra"] = Value::String("ignored".into());
        parse_skin_json(serde_json::to_vec(&value).unwrap()).unwrap();
    }

    #[test]
    fn metadata_uses_swift_character_counts_and_integral_json_numbers() {
        let mut value: Value = serde_json::from_slice(&minimal("owner-blue")).unwrap();
        value["displayName"]["zh"] = Value::String("e\u{301}".repeat(80));
        value["formatVersion"] = serde_json::json!(1.0);
        parse_skin_json(serde_json::to_vec(&value).unwrap()).unwrap();
        value["displayName"]["zh"] = Value::String("e\u{301}".repeat(81));
        assert!(parse_skin_json(serde_json::to_vec(&value).unwrap()).is_err());
    }

    #[test]
    fn metadata_matches_swift_crlf_and_zero_width_whitespace() {
        let mut value: Value = serde_json::from_slice(&minimal("owner-blue")).unwrap();
        value["description"] = Value::String("\r\n".repeat(1000));
        parse_skin_json(serde_json::to_vec(&value).unwrap()).unwrap();
        value["author"] = Value::String("\u{200b}".into());
        assert!(parse_skin_json(serde_json::to_vec(&value).unwrap()).is_err());
    }

    #[test]
    fn metadata_counts_extended_graphemes_like_swift() {
        for (text, count) in [
            ("\r\n", 1),
            ("A\u{200c}", 1),
            ("\u{0600}A", 1),
            ("\n\u{0301}", 2),
            ("\u{feff}\u{0301}", 2),
            ("👨‍👩‍👧‍👦", 1),
        ] {
            assert_eq!(
                skin_character_count(text).unwrap(),
                count,
                "wrong count for {text:?}"
            );
        }
    }

    fn asset_package(parent: &Path, id: &str) -> PathBuf {
        let package = parent.join(id);
        let mut value: Value = serde_json::from_slice(&minimal(id)).unwrap();
        value["decorations"] = serde_json::json!([{"type":"sprite","sheet":"mark.png","frames":1,"size":{"width":32,"height":32}}]);
        write_package(parent, id, &serde_json::to_vec(&value).unwrap());
        fs::write(
            package.join("mark.png"),
            include_bytes!("../../native/resources/Skins/Builtin/xuanmo/mark.png"),
        )
        .unwrap();
        package
    }

    #[test]
    fn rejected_packages_cover_each_filesystem_and_size_boundary() {
        let parent = temporary_root("rejected");
        fs::create_dir_all(&parent).unwrap();
        let source = asset_package(&parent, "owner-blue");
        let original = fs::read(source.join("mark.png")).unwrap();
        fs::write(
            source.join("mark.png"),
            vec![0; MAXIMUM_PACKAGE_BYTES as usize],
        )
        .unwrap();
        assert!(read_package(&source, Some("owner-blue"))
            .unwrap_err()
            .contains("单包体积"));
        fs::write(source.join("mark.png"), &original).unwrap();
        for index in 0..7 {
            fs::write(source.join(format!("{index}.png")), b"x").unwrap();
        }
        assert!(read_package(&source, None).unwrap_err().contains("文件数"));
        for index in 0..7 {
            fs::remove_file(source.join(format!("{index}.png"))).unwrap();
        }
        for name in ["extra.json", "script.js", "font.ttf", "MARK.PNG"] {
            fs::write(source.join(name), b"x").unwrap();
            assert!(read_package(&source, None).is_err(), "accepted {name}");
            fs::remove_file(source.join(name)).unwrap();
        }
        assert!(read_package(&source, Some("other-id"))
            .unwrap_err()
            .contains("不一致"));
        assert!(read_package(&source.join("../owner-blue"), None).is_err());
        let mut huge = original;
        huge[16..20].copy_from_slice(&4097u32.to_be_bytes());
        fs::write(source.join("mark.png"), huge).unwrap();
        assert!(read_package(&source, None).unwrap_err().contains("尺寸"));
        fs::remove_dir_all(parent).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_hardlinks_and_special_files_are_rejected_without_following() {
        use std::os::unix::fs::symlink;
        let parent = temporary_root("links");
        fs::create_dir_all(&parent).unwrap();
        write_package(&parent, "owner-blue", &minimal("owner-blue"));
        let package = parent.join("owner-blue");
        symlink(&package, parent.join("linked")).unwrap();
        assert!(read_package(&parent.join("linked"), None).is_err());
        assert!(read_package(&parent.join("linked/child"), None).is_err());
        fs::hard_link(package.join("skin.json"), parent.join("hard.json")).unwrap();
        assert!(read_package(&package, None).is_err());
        fs::remove_file(parent.join("hard.json")).unwrap();
        symlink("skin.json", package.join("mark.png")).unwrap();
        assert!(read_package(&package, None).is_err());
        fs::remove_file(package.join("mark.png")).unwrap();
        fs::create_dir(package.join("nested.png")).unwrap();
        assert!(read_package(&package, None).is_err());
        fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn failures_during_update_preserve_old_bytes_and_remove_staging() {
        let parent = temporary_root("update-failure");
        fs::create_dir_all(&parent).unwrap();
        let source = asset_package(&parent, "owner-blue");
        let root = parent.join("Skins");
        fs::create_dir(&root).unwrap();
        let old = minimal("owner-blue");
        write_package(&root, "owner-blue", &old);
        for failure in ["created", "skin.json", "mark.png", "validate", "activate"] {
            assert!(install_at_with(&source, &root, |step| {
                if step == failure {
                    Err(format!("injected at {failure}"))
                } else {
                    Ok(())
                }
            })
            .unwrap_err()
            .contains("injected"));
            assert_eq!(fs::read(root.join("owner-blue/skin.json")).unwrap(), old);
            assert_eq!(fs::read_dir(root.join("owner-blue")).unwrap().count(), 1);
            assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
            assert!(!fs::read_dir(&parent).unwrap().any(|entry| entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".install-")));
        }
        let corrupted = install_at_with(&source, &root, |step| {
            if step == "activate" {
                let staging = fs::read_dir(&parent)
                    .unwrap()
                    .map(|entry| entry.unwrap().path())
                    .find(|path| {
                        path.file_name()
                            .unwrap()
                            .to_string_lossy()
                            .starts_with(".install-")
                    })
                    .unwrap();
                fs::write(staging.join("skin.json"), b"{bad-json").unwrap();
            }
            Ok(())
        });
        assert!(corrupted.is_err());
        assert_eq!(fs::read(root.join("owner-blue/skin.json")).unwrap(), old);
        assert!(!fs::read_dir(&parent).unwrap().any(|entry| entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".install-")));
        install_at(&source, &root).unwrap();
        assert!(root.join("owner-blue/mark.png").is_file());
        let legacy = parent.join("legacy.json");
        fs::write(&legacy, &old).unwrap();
        install_at(&legacy, &root).unwrap();
        assert!(!root.join("owner-blue/mark.png").exists());
        fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn package_limit_allows_update_and_rejects_builtin_and_new_packages() {
        let parent = temporary_root("package-limit");
        let root = parent.join("Skins");
        fs::create_dir_all(&root).unwrap();
        for index in 0..MAXIMUM_PACKAGES {
            let id = format!("owner-{index}");
            write_package(&root, &id, &minimal(&id));
        }
        let source = parent.join("source.json");
        fs::write(&source, minimal("owner-0")).unwrap();
        install_at_with(&source, &root, |_| {
            assert_eq!(fs::read_dir(&root).unwrap().count(), MAXIMUM_PACKAGES);
            Ok(())
        })
        .unwrap();
        fs::write(&source, minimal("owner-new")).unwrap();
        assert!(install_at(&source, &root).is_err());
        fs::write(&source, minimal("system")).unwrap();
        assert!(install_at(&source, &root).unwrap_err().contains("内置"));
        assert!(!root.join("system").exists());
        fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn version_is_rejected_like_current_swift_and_malformed_old_targets_are_preserved() {
        let mut value: Value = serde_json::from_slice(&minimal("owner-blue")).unwrap();
        value["version"] = serde_json::json!("1.0.0");
        assert!(parse_skin_json(serde_json::to_vec(&value).unwrap()).is_err());
        let parent = temporary_root("invalid-old");
        let root = parent.join("Skins");
        fs::create_dir_all(root.join("owner-blue")).unwrap();
        fs::write(root.join("owner-blue/private.txt"), b"preserve").unwrap();
        let source = parent.join("source.json");
        fs::write(&source, minimal("owner-blue")).unwrap();
        assert!(install_at(&source, &root).is_err());
        assert_eq!(
            fs::read(root.join("owner-blue/private.txt")).unwrap(),
            b"preserve"
        );
        fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn substituted_staging_is_never_activated_or_deleted() {
        let parent = temporary_root("substituted-stage");
        let root = parent.join("Skins");
        fs::create_dir_all(&root).unwrap();
        let source = asset_package(&parent, "owner-blue");
        let old = minimal("owner-blue");
        write_package(&root, "owner-blue", &old);
        let result = install_at_with(&source, &root, |step| {
            if step == "activate" {
                let staging = fs::read_dir(&parent)
                    .unwrap()
                    .map(|entry| entry.unwrap().path())
                    .find(|path| {
                        path.file_name()
                            .unwrap()
                            .to_string_lossy()
                            .starts_with(".install-")
                    })
                    .unwrap();
                fs::rename(&staging, parent.join("original-stage")).unwrap();
                fs::create_dir(&staging).unwrap();
                fs::write(staging.join("private.txt"), b"preserve replacement").unwrap();
            }
            Ok(())
        });
        assert!(result.is_err());
        assert_eq!(fs::read(root.join("owner-blue/skin.json")).unwrap(), old);
        assert!(parent.join("original-stage/skin.json").is_file());
        assert!(fs::read_dir(&parent)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .any(|path| path.join("private.txt").is_file()));
        fs::remove_dir_all(parent).unwrap();
    }
}

fn decoration_object<'a>(
    value: Option<&'a Value>,
    label: &str,
) -> Result<&'a Map<String, Value>, String> {
    value
        .and_then(Value::as_object)
        .ok_or_else(|| format!("{label} 必须是对象。"))
}

fn decoration_keys(
    object: &Map<String, Value>,
    allowed: &[&str],
    label: &str,
) -> Result<(), String> {
    if object.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err(format!("{label} 含不支持的字段。"));
    }
    Ok(())
}

fn decoration_number(
    value: Option<&Value>,
    default: Option<f64>,
    label: &str,
    minimum: f64,
    maximum: f64,
) -> Result<f64, String> {
    let number = match value {
        Some(value) => value.as_f64(),
        None => default,
    }
    .filter(|number| number.is_finite() && *number >= minimum && *number <= maximum)
    .ok_or_else(|| format!("{label} 必须在 {minimum}..{maximum} 范围内。"))?;
    Ok(number)
}

fn decoration_integer(
    value: Option<&Value>,
    default: Option<f64>,
    label: &str,
    minimum: usize,
    maximum: usize,
) -> Result<usize, String> {
    let value = decoration_number(value, default, label, minimum as f64, maximum as f64)?;
    if value.fract() != 0.0 {
        return Err(format!("{label} 必须是整数。"));
    }
    Ok(value as usize)
}

fn decoration_point(
    value: Option<&Value>,
    keys: &[&str; 2],
    label: &str,
    minimum: f64,
    maximum: f64,
) -> Result<(f64, f64), String> {
    let object = decoration_object(value, label)?;
    decoration_keys(object, keys, label)?;
    Ok((
        decoration_number(object.get(keys[0]), None, label, minimum, maximum)?,
        decoration_number(object.get(keys[1]), None, label, minimum, maximum)?,
    ))
}

fn decoration_color(value: Option<&Value>, label: &str) -> Result<(), String> {
    let value = value.ok_or_else(|| format!("{label} 缺失。"))?;
    if !value.is_object() {
        return Err(format!("{label} 必须是 light/dark 颜色。"));
    }
    validate_color(value, label, false)
}

fn decoration_region(value: Option<&Value>, label: &str) -> Result<(), String> {
    let Some(value) = value else {
        return Ok(());
    };
    let object = decoration_object(Some(value), label)?;
    decoration_keys(object, &["x", "y", "width", "height"], label)?;
    let x = decoration_number(object.get("x"), None, label, 0.0, 1.0)?;
    let y = decoration_number(object.get("y"), None, label, 0.0, 1.0)?;
    let width = decoration_number(object.get("width"), None, label, 0.05, 1.0)?;
    let height = decoration_number(object.get("height"), None, label, 0.05, 1.0)?;
    if x + width > 1.0001 || y + height > 1.0001 {
        return Err(format!("{label}.region 超出面板。"));
    }
    Ok(())
}

fn decoration_motions(value: Option<&Value>, label: &str) -> Result<(), String> {
    let Some(value) = value else {
        return Ok(());
    };
    let list = value
        .as_array()
        .filter(|list| list.len() <= 4)
        .ok_or_else(|| format!("{label}.motion 必须是最多 4 项的数组。"))?;
    for value in list {
        let object = decoration_object(Some(value), label)?;
        decoration_number(object.get("period"), None, label, 1.0, 20.0)?;
        let (field, minimum, maximum, keys): (&str, f64, f64, &[&str]) =
            match object.get("kind").and_then(Value::as_str) {
                Some("float") => ("amplitude", 0.5, 12.0, &["kind", "amplitude", "period"]),
                Some("breathe") => ("scale", 0.005, 0.12, &["kind", "scale", "period"]),
                Some("sway") => ("degrees", 0.3, 12.0, &["kind", "degrees", "period"]),
                Some("twitch") => ("degrees", 0.3, 12.0, &["kind", "degrees", "period", "delay", "duration"]),
                Some("glow") => ("radius", 2.0, 30.0, &["kind", "color", "radius", "period"]),
                Some("hop") => ("height", 1.0, 16.0, &["kind", "height", "squash", "period"]),
                Some("fall") => ("height", 1.0, 120.0, &["kind", "height", "drift", "period"]),
                Some("fade") => ("minimum", 0.1, 0.95, &["kind", "minimum", "period"]),
                _ => return Err(format!("{label}.motion 不支持的 kind。")),
            };
        decoration_keys(object, keys, label)?;
        decoration_number(object.get(field), None, label, minimum, maximum)?;
        if object.get("kind").and_then(Value::as_str) == Some("twitch") {
            let delay = decoration_number(object.get("delay"), None, label, 0.0, 20.0)?;
            let duration = decoration_number(object.get("duration"), None, label, 0.3, 0.5)?;
            let period = decoration_number(object.get("period"), None, label, 1.0, 20.0)?;
            if delay + duration > period {
                return Err(format!("{label}.motion 抖动必须在周期内结束。"));
            }
        }
        if field == "radius" {
            decoration_color(object.get("color"), label)?;
        }
        if object.get("kind").and_then(Value::as_str) == Some("hop") {
            decoration_number(object.get("squash"), None, label, 0.0, 0.25)?;
        }
        if object.get("kind").and_then(Value::as_str) == Some("fall") {
            decoration_number(object.get("drift"), Some(0.0), label, -40.0, 40.0)?;
        }
    }
    Ok(())
}

fn decoration_clips(
    value: Option<&Value>,
    columns: usize,
    label: &str,
) -> Result<Vec<usize>, String> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let list = value
        .as_array()
        .filter(|list| !list.is_empty() && list.len() <= 16)
        .ok_or_else(|| format!("{label}.clips 必须是 1 到 16 项的数组。"))?;
    let mut rows = Vec::new();
    let mut steps = 0.0;
    for (index, value) in list.iter().enumerate() {
        let object = decoration_object(Some(value), label)?;
        decoration_keys(
            object,
            &["row", "frames", "fps", "seconds", "move", "flip"],
            label,
        )?;
        decoration_integer(object.get("frames"), None, label, 1, columns)?;
        let fps = decoration_number(object.get("fps"), Some(8.0), label, 1.0, 30.0)?;
        let seconds = decoration_number(object.get("seconds"), None, label, 0.2, 30.0)?;
        steps += fps * seconds;
        if let Some(value) = object.get("move") {
            decoration_point(Some(value), &["from", "to"], label, -0.5, 1.5)?;
        }
        rows.push(decoration_integer(
            object.get("row"),
            Some(index as f64),
            label,
            0,
            15,
        )?);
    }
    if steps > 1200.0 {
        return Err(format!("{label}.clips 帧步数超过 1200。"));
    }
    Ok(rows)
}

fn decoration_asset_name(name: &str) -> bool {
    let Some(stem) = name.strip_suffix(".png") else {
        return false;
    };
    !stem.is_empty()
        && stem.len() <= 63
        && (stem.as_bytes()[0].is_ascii_lowercase() || stem.as_bytes()[0].is_ascii_digit())
        && stem.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(&byte)
        })
}

fn validate_decorations(
    object: &Map<String, Value>,
    assets: &[(String, Vec<u8>)],
) -> Result<(), String> {
    let Some(value) = object.get("decorations") else {
        return if assets.is_empty() {
            Ok(())
        } else {
            Err("图片未被 decorations 引用。".into())
        };
    };
    let list = value
        .as_array()
        .filter(|list| !list.is_empty() && list.len() <= 8)
        .ok_or_else(|| "decorations 必须是 1 到 8 项的数组。".to_string())?;
    let mut used = HashSet::new();
    for (index, value) in list.iter().enumerate() {
        let label = format!("decorations[{index}]");
        let object = decoration_object(Some(value), &label)?;
        if let Some(layer) = object.get("layer") {
            if layer.as_str() != Some("below") && layer.as_str() != Some("above") {
                return Err(format!("{label}.layer 只能是 below 或 above。"));
            }
        }
        match object.get("type").and_then(Value::as_str) {
            Some("sprite") | Some("frame") => {
                let sprite = object.get("type").and_then(Value::as_str) == Some("sprite");
                decoration_keys(
                    object,
                    if sprite {
                        &[
                            "type",
                            "sheet",
                            "frames",
                            "fps",
                            "size",
                            "anchor",
                            "offset",
                            "travel",
                            "layer",
                            "overflow",
                            "opacity",
                            "pixelated",
                            "staticFrame",
                            "pivot",
                            "motion",
                            "clips",
                            "zone",
                        ]
                    } else {
                        &["type", "sheet", "slice", "opacity"]
                    },
                    &label,
                )?;
                let name = object
                    .get("sheet")
                    .and_then(Value::as_str)
                    .filter(|name| decoration_asset_name(name))
                    .ok_or_else(|| format!("{label}.sheet 必须是包内的 png 文件名。"))?;
                let data = assets
                    .iter()
                    .find(|(asset, _)| asset == name)
                    .map(|(_, data)| data)
                    .ok_or_else(|| format!("{label}.sheet 缺少图片。"))?;
                used.insert(name);
                let frames = if sprite {
                    decoration_integer(object.get("frames"), None, &label, 1, 24)?
                } else {
                    1
                };
                let (width, height) = png_sheet_dimensions(data, frames, &label)?;
                decoration_number(object.get("opacity"), Some(1.0), &label, 0.0, 1.0)?;
                if !sprite {
                    let slice = decoration_object(object.get("slice"), &label)?;
                    decoration_keys(slice, &["top", "left", "bottom", "right"], &label)?;
                    let mut edges = Vec::new();
                    for field in ["top", "left", "bottom", "right"] {
                        edges.push(decoration_number(
                            slice.get(field),
                            None,
                            &label,
                            0.0,
                            2048.0,
                        )?);
                    }
                    if edges[1] + edges[3] >= width as f64 || edges[0] + edges[2] >= height as f64 {
                        return Err(format!("{label}.slice 必须保留可拉伸的中间部分。"));
                    }
                    continue;
                }
                decoration_number(object.get("fps"), Some(8.0), &label, 1.0, 30.0)?;
                let (size_x, size_y) =
                    decoration_point(object.get("size"), &["width", "height"], &label, 1.0, 300.0)?;
                if size_y > 256.0 {
                    return Err(format!("{label}.size.height 必须在 1..256 范围内。"));
                }
                for (field, minimum, maximum) in [
                    ("anchor", 0.0, 1.0),
                    ("offset", -256.0, 256.0),
                    ("pivot", 0.0, 1.0),
                ] {
                    if let Some(value) = object.get(field) {
                        decoration_point(Some(value), &["x", "y"], &label, minimum, maximum)?;
                    }
                }
                decoration_number(
                    object.get("staticFrame"),
                    Some(0.0),
                    &label,
                    0.0,
                    (frames - 1) as f64,
                )?;
                decoration_motions(object.get("motion"), &label)?;
                let clip_rows = decoration_clips(object.get("clips"), frames, &label)?;
                if let Some(travel) = object.get("travel") {
                    if !clip_rows.is_empty() {
                        return Err(format!("{label} 有 clips 时不能再写 travel。"));
                    }
                    let travel = decoration_object(Some(travel), &label)?;
                    decoration_keys(travel, &["from", "to", "duration"], &label)?;
                    decoration_number(travel.get("from"), None, &label, -1.0, 2.0)?;
                    decoration_number(travel.get("to"), None, &label, -1.0, 2.0)?;
                    decoration_number(travel.get("duration"), None, &label, 1.0, 120.0)?;
                }
                if !clip_rows.is_empty() {
                    let cell_height = width as f64 / frames as f64 * size_y / size_x;
                    let rows = (height as f64 / cell_height).round();
                    if rows < 1.0
                        || (rows * cell_height - height as f64).abs() > height as f64 * 0.03
                        || clip_rows.iter().any(|row| *row as f64 >= rows)
                    {
                        return Err(format!("{label}.clips 与精灵表的行数或格子形状不符。"));
                    }
                }
                if let Some(zone) = object.get("zone") {
                    if clip_rows.is_empty() {
                        return Err(format!("{label}.zone 只能与 clips 一起用。"));
                    }
                    let zone = decoration_object(Some(zone), &label)?;
                    decoration_keys(zone, &["width"], &label)?;
                    decoration_number(zone.get("width"), None, &label, 8.0, 1024.0)?;
                }
            }
            Some("twinkle") => {
                decoration_keys(
                    object,
                    &[
                        "type", "count", "size", "color", "period", "seed", "layer", "overflow",
                        "region",
                    ],
                    &label,
                )?;
                let (minimum, maximum) =
                    decoration_point(object.get("size"), &["min", "max"], &label, 1.0, 32.0)?;
                let (short, long) =
                    decoration_point(object.get("period"), &["min", "max"], &label, 0.4, 12.0)?;
                if minimum > maximum || short > long {
                    return Err(format!("{label} 的 min 不能大于 max。"));
                }
                decoration_integer(object.get("count"), None, &label, 1, 24)?;
                decoration_integer(object.get("seed"), Some(1.0), &label, 0, 1_000_000)?;
                decoration_color(object.get("color"), &label)?;
                decoration_region(object.get("region"), &label)?;
            }
            Some("petals") => {
                decoration_keys(
                    object,
                    &[
                        "type",
                        "rate",
                        "size",
                        "color",
                        "lifetime",
                        "fallSpeed",
                        "drift",
                        "layer",
                        "overflow",
                        "region",
                    ],
                    &label,
                )?;
                let (minimum, maximum) =
                    decoration_point(object.get("size"), &["min", "max"], &label, 2.0, 32.0)?;
                if minimum > maximum {
                    return Err(format!("{label}.size 的 min 不能大于 max。"));
                }
                decoration_number(object.get("rate"), None, &label, 0.2, 12.0)?;
                decoration_number(object.get("lifetime"), None, &label, 1.0, 20.0)?;
                decoration_number(object.get("fallSpeed"), Some(14.0), &label, 2.0, 80.0)?;
                decoration_number(object.get("drift"), Some(0.0), &label, -40.0, 40.0)?;
                decoration_color(object.get("color"), &label)?;
                decoration_region(object.get("region"), &label)?;
            }
            Some("water") => {
                decoration_keys(
                    object,
                    &["type", "depth", "poolWidth", "from", "to", "highlight", "period", "layer", "overflow"],
                    &label,
                )?;
                decoration_number(object.get("depth"), None, &label, 8.0, 120.0)?;
                decoration_number(object.get("poolWidth"), None, &label, 32.0, 300.0)?;
                decoration_number(object.get("period"), Some(5.0), &label, 1.0, 20.0)?;
                for field in ["from", "to", "highlight"] {
                    decoration_color(object.get(field), &label)?;
                }
            }
            Some("gradient") => {
                decoration_keys(
                    object,
                    &["type", "from", "to", "angle", "opacity", "layer"],
                    &label,
                )?;
                decoration_color(object.get("from"), &label)?;
                decoration_color(object.get("to"), &label)?;
                decoration_number(object.get("angle"), Some(90.0), &label, 0.0, 360.0)?;
                decoration_number(object.get("opacity"), Some(1.0), &label, 0.0, 1.0)?;
            }
            Some("shimmer") => {
                decoration_keys(
                    object,
                    &["type", "color", "period", "width", "angle", "layer"],
                    &label,
                )?;
                decoration_color(object.get("color"), &label)?;
                decoration_number(object.get("period"), Some(8.0), &label, 3.0, 30.0)?;
                decoration_number(object.get("width"), Some(0.22), &label, 0.05, 0.8)?;
                decoration_number(object.get("angle"), Some(18.0), &label, -60.0, 60.0)?;
            }
            Some("shootingStar") => {
                decoration_keys(
                    object,
                    &[
                        "type", "color", "period", "length", "angle", "seed", "layer", "overflow",
                        "region",
                    ],
                    &label,
                )?;
                decoration_color(object.get("color"), &label)?;
                decoration_number(object.get("period"), Some(7.0), &label, 3.0, 40.0)?;
                decoration_number(object.get("length"), Some(60.0), &label, 16.0, 200.0)?;
                decoration_number(object.get("angle"), Some(205.0), &label, 0.0, 360.0)?;
                decoration_integer(object.get("seed"), Some(1.0), &label, 0, 1_000_000)?;
                decoration_region(object.get("region"), &label)?;
            }
            _ => return Err(format!("{label} 不支持的 type。")),
        }
    }
    if assets.iter().any(|(name, _)| !used.contains(name.as_str())) {
        return Err("包内有未被引用的图片。".into());
    }
    Ok(())
}

fn png_sheet_dimensions(data: &[u8], frames: usize, label: &str) -> Result<(usize, usize), String> {
    // Bound dimensions before handing compressed data to ImageIO, which is also the Swift decoder.
    if data.len() > 1024 * 1024
        || data.len() < 33
        || !data.starts_with(b"\x89PNG\r\n\x1a\n")
        || &data[8..16] != b"\0\0\0\rIHDR"
    {
        return Err(format!("{label}.sheet 不是合规的 png。"));
    }
    let width = u32::from_be_bytes(data[16..20].try_into().unwrap()) as usize;
    let height = u32::from_be_bytes(data[20..24].try_into().unwrap()) as usize;
    if width == 0
        || width > 4096
        || height == 0
        || height > 1536
        || width % frames != 0
        || width / frames < 1
    {
        return Err(format!("{label}.sheet 尺寸与 frames 不符。"));
    }
    png_decode(data, width, height).map_err(|_| format!("{label}.sheet 不是合规的 png。"))?;
    Ok((width, height))
}

#[cfg(target_os = "macos")]
fn png_decode(data: &[u8], width: usize, height: usize) -> Result<(), ()> {
    use std::ffi::c_void;
    type Reference = *const c_void;
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFDataCreate(
            allocator: Reference,
            bytes: *const u8,
            length: std::ffi::c_long,
        ) -> Reference;
        fn CFStringCreateWithCString(
            allocator: Reference,
            value: *const i8,
            encoding: u32,
        ) -> Reference;
        fn CFEqual(left: Reference, right: Reference) -> u8;
        fn CFRelease(reference: Reference);
    }
    #[link(name = "ImageIO", kind = "framework")]
    extern "C" {
        fn CGImageSourceCreateWithData(data: Reference, options: Reference) -> Reference;
        fn CGImageSourceGetCount(source: Reference) -> usize;
        fn CGImageSourceGetType(source: Reference) -> Reference;
        fn CGImageSourceCreateImageAtIndex(
            source: Reference,
            index: usize,
            options: Reference,
        ) -> Reference;
    }
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGImageGetWidth(image: Reference) -> usize;
        fn CGImageGetHeight(image: Reference) -> usize;
        fn CGImageRelease(image: Reference);
    }
    unsafe {
        let null = std::ptr::null();
        let data_ref = CFDataCreate(null, data.as_ptr(), data.len() as std::ffi::c_long);
        if data_ref.is_null() {
            return Err(());
        }
        let source = CGImageSourceCreateWithData(data_ref, null);
        CFRelease(data_ref);
        if source.is_null() {
            return Err(());
        }
        let png_type =
            CFStringCreateWithCString(null, b"public.png\0".as_ptr().cast(), 0x0800_0100);
        let source_type = CGImageSourceGetType(source);
        let compliant = !png_type.is_null()
            && !source_type.is_null()
            && CFEqual(source_type, png_type) != 0
            && CGImageSourceGetCount(source) == 1;
        if !png_type.is_null() {
            CFRelease(png_type);
        }
        if !compliant {
            CFRelease(source);
            return Err(());
        }
        let image = CGImageSourceCreateImageAtIndex(source, 0, null);
        CFRelease(source);
        if image.is_null() {
            return Err(());
        }
        let matches = CGImageGetWidth(image) == width && CGImageGetHeight(image) == height;
        CGImageRelease(image);
        if matches {
            Ok(())
        } else {
            Err(())
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn png_decode(_data: &[u8], _width: usize, _height: usize) -> Result<(), ()> {
    // ImageIO parity is required; platforms without that decoder must fail closed.
    Err(())
}

#[cfg(test)]
mod decoration_tests {
    use super::*;
    use serde_json::json;

    fn validate(list: Value) -> Result<(), String> {
        validate_decorations(json!({"decorations": list}).as_object().unwrap(), &[])
    }

    fn color() -> Value {
        json!({"light": "#FFFFFF", "dark": "#000000"})
    }

    #[test]
    fn twitch_ranges_and_rest_match_swift() {
        let motion = json!({"kind":"twitch","degrees":8,"period":6,"delay":1.2,"duration":0.4});
        decoration_motions(Some(&json!([motion.clone()])), "sprite").unwrap();
        for delay in [0.0, 0.5] {
            decoration_motions(Some(&json!([{"kind":"twitch","degrees":12,"period":1,"delay":delay,"duration":0.5}])), "sprite").unwrap();
        }
        for (field, value) in [
            ("degrees", json!(0.2)), ("degrees", json!(12.1)),
            ("delay", json!(-0.1)), ("delay", json!(5.7)),
            ("duration", json!(0.29)), ("duration", json!(0.51)),
            ("period", json!(0.9)), ("period", json!(20.1)), ("unknown", json!(1)),
        ] {
            let mut bad = motion.clone();
            bad[field] = value;
            assert!(decoration_motions(Some(&json!([bad])), "sprite").is_err(), "{field}");
        }
        for field in ["degrees", "delay", "duration"] {
            let mut bad = motion.clone();
            bad.as_object_mut().unwrap().remove(field);
            assert!(decoration_motions(Some(&json!([bad])), "sprite").is_err(), "{field}");
        }
    }

    #[test]
    fn procedural_decoration_types_and_swift_defaults_are_accepted() {
        for decoration in [
            json!({"type":"twinkle","count":24,"size":{"min":1,"max":32},"period":{"min":0.4,"max":12},"color":color(),"overflow":"defaults to false"}),
            json!({"type":"petals","rate":0.2,"size":{"min":2,"max":32},"lifetime":20,"color":color()}),
            json!({"type":"gradient","from":color(),"to":color()}),
            json!({"type":"shimmer","color":color()}),
            json!({"type":"shootingStar","color":color()}),
        ] {
            validate(json!([decoration])).unwrap();
        }
        validate(json!([{"type":"gradient","from":{"light":"#FFFFFF","dark":"#000000","ignored":1},"to":color()}])).unwrap();
        validate_decorations(json!({}).as_object().unwrap(), &[]).unwrap();
    }

    #[test]
    fn decoration_count_limit_is_eight_like_swift() {
        let gradients = |count: usize| {
            (0..count)
                .map(|_| json!({"type":"gradient","from":color(),"to":color()}))
                .collect::<Vec<Value>>()
        };
        validate(Value::Array(gradients(8))).unwrap();
        assert!(validate(Value::Array(gradients(9)))
            .unwrap_err()
            .contains("1 到 8"));
    }

    #[test]
    fn water_and_falling_leaf_ranges_match_swift() {
        let water = json!({"type":"water","depth":44,"poolWidth":128,"from":color(),"to":color(),"highlight":color(),"overflow":true});
        validate(json!([water.clone()])).unwrap();
        for (field, value) in [
            ("depth", json!(7.9)),
            ("depth", json!(120.1)),
            ("poolWidth", json!(31.9)),
            ("poolWidth", json!(300.1)),
            ("period", json!(0.9)),
            ("period", json!(20.1)),
            ("unknown", json!(1)),
        ] {
            let mut bad = water.clone();
            bad[field] = value;
            assert!(validate(json!([bad])).is_err(), "accepted {field}");
        }
        decoration_motions(Some(&json!([{"kind":"fall","height":34,"drift":12,"period":12}])), "sprite").unwrap();
        decoration_motions(Some(&json!([{"kind":"fall","height":120,"period":1}])), "sprite").unwrap();
        for motion in [
            json!({"kind":"fall","height":121,"period":12}),
            json!({"kind":"fall","height":34,"drift":41,"period":12}),
            json!({"kind":"fall","height":34,"squash":0.1,"period":12}),
        ] {
            assert!(decoration_motions(Some(&json!([motion])), "sprite").is_err());
        }
    }

    #[test]
    fn decoration_arrays_unknown_fields_and_numeric_bounds_are_rejected() {
        for list in [
            Value::Null,
            json!([]),
            json!([{}, {}, {}, {}, {}, {}, {}, {}, {}]),
            json!([{"type":"script"}]),
            json!([{"type":"gradient","from":color(),"to":color(),"unknown":1}]),
            json!([{"type":"gradient","from":color(),"to":color(),"layer":null}]),
            json!([{"type":"gradient","from":color(),"to":color(),"angle":true}]),
            json!([{"type":"gradient","from":color(),"to":color(),"angle":361}]),
            json!([{"type":"shimmer","color":color(),"width":0.01}]),
            json!([{"type":"shootingStar","color":color(),"seed":0.5}]),
            json!([{"type":"shootingStar","color":color(),"region":{"x":0.9,"y":0,"width":0.2,"height":1}}]),
            json!([{"type":"shootingStar","color":color(),"region":null}]),
            json!([{"type":"petals","rate":1,"size":{"min":3,"max":2},"lifetime":1,"color":color()}]),
            json!([{"type":"twinkle","count":25,"size":{"min":1,"max":2},"period":{"min":1,"max":2},"color":color()}]),
            json!([{"type":"twinkle","count":1,"size":{"min":1,"max":2},"period":{"min":3,"max":2},"color":color()}]),
            json!([{"type":"gradient","from":"system:clear","to":color()}]),
            json!([{"type":"gradient","from":{"light":"#ffffff","dark":"#000000"},"to":color()}]),
        ] {
            assert!(validate(list.clone()).is_err(), "accepted {list}");
        }
    }

    #[test]
    fn action_clip_and_motion_limits_match_swift() {
        decoration_clips(
            Some(&json!([{"frames":1,"seconds":0.2,"flip":"ignored"}])),
            1,
            "sprite",
        )
        .unwrap();
        for clips in [
            json!([]),
            Value::Null,
            json!([{"frames":2,"seconds":1}]),
            json!([{"frames":1,"seconds":1,"row":16}]),
            json!([{"frames":1,"seconds":1,"move":{"from":-0.6,"to":1}}]),
            json!([{"frames":1,"seconds":30,"fps":30},{"frames":1,"seconds":30,"fps":30}]),
        ] {
            assert!(
                decoration_clips(Some(&clips), 1, "sprite").is_err(),
                "accepted {clips}"
            );
        }
        let motions = json!([
            {"kind":"float","amplitude":0.5,"period":1},
            {"kind":"breathe","scale":0.12,"period":20},
            {"kind":"sway","degrees":12,"period":1},
            {"kind":"glow","radius":30,"color":color(),"period":1}
        ]);
        decoration_motions(Some(&motions), "sprite").unwrap();
        decoration_motions(
            Some(&json!([
                {"kind":"hop","height":16,"squash":0.25,"period":1},
                {"kind":"fade","minimum":0.95,"period":20}
            ])),
            "sprite",
        )
        .unwrap();
        for motions in [
            Value::Null,
            json!([{}, {}, {}, {}, {}]),
            json!([{"kind":"float","amplitude":0,"period":1}]),
            json!([{"kind":"hop","height":1,"squash":0.3,"period":1}]),
            json!([{"kind":"fade","minimum":0.5,"period":0}]),
            json!([{"kind":"fade","minimum":0.5,"period":1,"script":"bad"}]),
        ] {
            assert!(
                decoration_motions(Some(&motions), "sprite").is_err(),
                "accepted {motions}"
            );
        }
    }

    #[test]
    fn asset_names_and_png_dimensions_are_checked_before_decode() {
        for name in ["a.png", "0-_.png", &format!("{}.png", "a".repeat(63))] {
            assert!(decoration_asset_name(name));
        }
        for name in [
            "../a.png",
            "a/../b.png",
            "A.png",
            ".png",
            "a.jpg",
            &format!("{}.png", "a".repeat(64)),
        ] {
            assert!(!decoration_asset_name(name));
        }
        let original = include_bytes!("../../native/resources/Skins/Builtin/xuanmo/mark.png");
        let mut huge = original.to_vec();
        huge[16..20].copy_from_slice(&4097u32.to_be_bytes());
        assert!(png_sheet_dimensions(&huge, 1, "sprite")
            .unwrap_err()
            .contains("尺寸"));
        huge[16..20].copy_from_slice(&4096u32.to_be_bytes());
        huge[20..24].copy_from_slice(&1537u32.to_be_bytes());
        assert!(png_sheet_dimensions(&huge, 1, "sprite")
            .unwrap_err()
            .contains("尺寸"));
        assert!(png_sheet_dimensions(&original[..24], 1, "sprite").is_err());
        assert!(validate_decorations(
            json!({}).as_object().unwrap(),
            &[("a.png".into(), original.to_vec())]
        )
        .is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn pilot_sprite_width_boundary_matches_swift() {
        let bytes = include_bytes!("../../docs/current/evidence/skin-m2-pilot-art/keyed/sakura-top-a.png")
            .to_vec();
        let assets = vec![("branch.png".into(), bytes)];
        let sprite = json!({"type":"sprite","sheet":"branch.png","frames":1,
            "size":{"width":300,"height":96},
            "clips":[{"row":0,"frames":1,"seconds":3.6,"flip":true}]});
        validate_decorations(
            json!({"decorations":[sprite.clone()]}).as_object().unwrap(),
            &assets,
        )
        .unwrap();
        for size in [
            json!({"width":300.01,"height":96}),
            json!({"width":300,"height":256.01}),
        ] {
            let mut invalid = sprite.clone();
            invalid["size"] = size;
            assert!(validate_decorations(
                json!({"decorations":[invalid]}).as_object().unwrap(),
                &assets,
            )
            .is_err());
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn sprite_image_references_frames_and_clips_are_validated() {
        let bytes = include_bytes!("../../native/resources/Skins/Builtin/xuanmo/mark.png").to_vec();
        let assets = vec![("a.png".into(), bytes)];
        let sprite = json!({"type":"sprite","sheet":"a.png","frames":1,"size":{"width":32,"height":32},"pixelated":"ignored"});
        validate_decorations(
            json!({"decorations":[sprite.clone()]}).as_object().unwrap(),
            &assets,
        )
        .unwrap();
        for (key, value) in [
            ("sheet", json!("../a.png")),
            ("frames", json!(25)),
            ("staticFrame", json!(1)),
            ("anchor", json!({"x":1.01,"y":0})),
            ("zone", json!({"width":8})),
            ("clips", json!([{"row":15,"frames":1,"seconds":1}])),
        ] {
            let mut invalid = sprite.clone();
            invalid[key] = value;
            assert!(
                validate_decorations(
                    json!({"decorations":[invalid]}).as_object().unwrap(),
                    &assets
                )
                .is_err(),
                "accepted {key}"
            );
        }
        let frame =
            json!({"type":"frame","sheet":"a.png","slice":{"top":0,"left":0,"bottom":0,"right":0}});
        validate_decorations(json!({"decorations":[frame]}).as_object().unwrap(), &assets).unwrap();
        let mut fractional = sprite;
        fractional["frames"] = json!(2);
        fractional["staticFrame"] = json!(0.5);
        validate_decorations(
            json!({"decorations":[fractional]}).as_object().unwrap(),
            &assets,
        )
        .unwrap();
    }
}
