use crate::skin_catalog::{is_candidate_skin_id, CANDIDATE_SKIN_CATALOG};
use serde::Serialize;
use serde_json::{Map, Value};
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

pub const MAXIMUM_PACKAGES: usize = 128;
pub const MAXIMUM_FILES_PER_PACKAGE: usize = 8;
pub const MAXIMUM_PACKAGE_BYTES: u64 = 512 * 1024;
pub const MAXIMUM_SKIN_JSON_BYTES: u64 = 256 * 1024;

static INSTALL_SEQUENCE: AtomicU64 = AtomicU64::new(0);

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
    let metadata =
        fs::symlink_metadata(source).map_err(|error| format!("无法读取所选皮肤包：{error}"))?;
    if metadata.file_type().is_symlink() {
        return Err("拒绝安装符号链接皮肤包。".into());
    }
    let parsed = if metadata.is_dir() {
        read_package(source, None)?
    } else if metadata.is_file()
        && source.extension().and_then(|value| value.to_str()) == Some("json")
    {
        let json = read_bounded_regular(source, MAXIMUM_SKIN_JSON_BYTES)?;
        parse_skin_json(json)?
    } else {
        return Err("请选择一个皮肤包目录或 .json 文件。".into());
    };
    ensure_directory_path(root)?;
    let destination = root.join(&parsed.id);
    if fs::symlink_metadata(&destination).is_ok() {
        return Err(format!("外部皮肤 {} 已安装；请先卸载。", parsed.id));
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "系统时间无效。".to_string())?
        .as_nanos();
    let sequence = INSTALL_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let staging = root.join(format!(".install-{}-{now}-{sequence}", std::process::id()));
    fs::create_dir(&staging).map_err(|error| format!("无法创建皮肤安装暂存目录：{error}"))?;
    #[cfg(unix)]
    fs::set_permissions(&staging, fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("无法设置皮肤安装目录权限：{error}"))?;
    let result = (|| {
        let target = staging.join("skin.json");
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options
            .open(&target)
            .map_err(|error| format!("无法创建 skin.json：{error}"))?;
        file.write_all(&parsed.json)
            .and_then(|_| file.sync_all())
            .map_err(|error| format!("无法持久化 skin.json：{error}"))?;
        fs::rename(&staging, &destination).map_err(|error| format!("无法激活皮肤包：{error}"))?;
        Ok(parsed.id.clone())
    })();
    if result.is_err() {
        let _ = fs::remove_file(staging.join("skin.json"));
        let _ = fs::remove_dir(&staging);
    }
    result
}

fn uninstall_at(id: &str, root: &Path) -> Result<(), String> {
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
    let _ = read_package(&package, Some(id))?;
    fs::remove_file(package.join("skin.json"))
        .map_err(|error| format!("无法删除 skin.json：{error}"))?;
    fs::remove_dir(&package).map_err(|error| format!("无法删除皮肤包目录：{error}"))
}

fn read_package(path: &Path, expected_directory: Option<&str>) -> Result<ParsedSkin, String> {
    let metadata =
        fs::symlink_metadata(path).map_err(|error| format!("无法检查皮肤包：{error}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err("包路径必须是普通目录，不能是符号链接。".into());
    }
    let mut entries = fs::read_dir(path)
        .map_err(|error| format!("无法读取包目录：{error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("无法读取包目录：{error}"))?;
    if entries.len() > MAXIMUM_FILES_PER_PACKAGE {
        return Err(format!("单包文件数超过 {MAXIMUM_FILES_PER_PACKAGE} 上限。"));
    }
    entries.sort_by_key(|entry| entry.file_name());
    let mut total_bytes = 0u64;
    for entry in &entries {
        let entry_metadata = fs::symlink_metadata(entry.path())
            .map_err(|error| format!("无法检查包内文件：{error}"))?;
        if entry_metadata.file_type().is_symlink() {
            return Err(format!(
                "包内 {} 是指向目录外的符号链接，已拒绝。",
                entry.file_name().to_string_lossy()
            ));
        }
        if !entry_metadata.is_file() {
            return Err("P1 皮肤包只允许普通数据文件。".into());
        }
        total_bytes = total_bytes
            .checked_add(entry_metadata.len())
            .ok_or_else(|| "单包体积溢出。".to_string())?;
        if total_bytes > MAXIMUM_PACKAGE_BYTES {
            return Err(format!("单包体积超过 {MAXIMUM_PACKAGE_BYTES} 字节上限。"));
        }
    }
    if entries.len() != 1 || entries[0].file_name() != "skin.json" {
        return Err("P1 皮肤包必须只包含 skin.json，不接受素材、字体或可执行文件。".into());
    }
    let json = read_bounded_regular(&entries[0].path(), MAXIMUM_SKIN_JSON_BYTES)?;
    let parsed = parse_skin_json(json)?;
    if let Some(expected) = expected_directory {
        if parsed.id != expected {
            return Err(format!(
                "skin.json id '{}' 与包目录 '{}' 不一致。",
                parsed.id, expected
            ));
        }
    }
    Ok(parsed)
}

fn read_bounded_regular(path: &Path, maximum: u64) -> Result<Vec<u8>, String> {
    let metadata =
        fs::symlink_metadata(path).map_err(|error| format!("无法检查 skin.json：{error}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err("skin.json 必须是普通文件，不能是符号链接。".into());
    }
    if metadata.len() > maximum {
        return Err(format!("skin.json 超过 {maximum} 字节上限。"));
    }
    let file = File::open(path).map_err(|error| format!("无法打开 skin.json：{error}"))?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(maximum + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("无法读取 skin.json：{error}"))?;
    if bytes.len() as u64 > maximum {
        return Err(format!("skin.json 超过 {maximum} 字节上限。"));
    }
    let after =
        fs::symlink_metadata(path).map_err(|error| format!("无法复核 skin.json：{error}"))?;
    if after.file_type().is_symlink() || after.len() != metadata.len() {
        return Err("skin.json 在读取过程中发生变化。".into());
    }
    Ok(bytes)
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
        .and_then(Value::as_u64)
        .ok_or_else(|| "缺少必填字段 formatVersion。".to_string())?;
    if format_version != 1 {
        return Err("formatVersion 必须为 1。".into());
    }
    let display = object
        .get("displayName")
        .and_then(Value::as_object)
        .ok_or_else(|| "缺少必填字段 displayName。".to_string())?;
    if display.keys().any(|key| key != "zh" && key != "en") {
        return Err("displayName 只允许 zh/en。".into());
    }
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
        if let Some(value) = object.get(field) {
            let font = value
                .as_object()
                .ok_or_else(|| format!("{field} 必须是对象。"))?;
            let family = required_string(font, "family", 40)?;
            const SYSTEM_FAMILIES: [&str; 13] = ["PingFang SC", "systemMonospacedDigit", "Kaiti SC", "Songti SC", "Yuanti SC", "Xingkai SC", "Hannotate SC", "Hanzipen SC", "Wawati SC", "Weibei SC", "Libian SC", "Baoli SC", "Yuppy SC"];
            if !SYSTEM_FAMILIES.contains(&family.as_str()) {
                return Err(format!("P1 不支持字体 {family}。"));
            }
            let weight = required_string(font, "weight", 20)?;
            if !["regular", "medium", "semibold", "bold"].contains(&weight.as_str()) {
                return Err(format!("{field}.weight 无效。"));
            }
            validate_number(font.get("size"), &format!("{field}.size"), 0.0, 96.0, false)?;
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
        if let Some(value) = object.get(field) {
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
            validate_number(object.get(field), field, 0.0, 1.0, false)?;
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
            validate_number(object.get(field), field, 0.0, 4096.0, false)?;
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
    if let Some(value) = object.get("material") {
        let material = value
            .as_str()
            .ok_or_else(|| "material 必须是字符串。".to_string())?;
        if material != "popover" && material != "hudWindow" && material != "glass" {
            return Err("material 只支持 popover/hudWindow/glass。".into());
        }
    }
    if let Some(value) = object.get("shadowEnabled") {
        if !value.is_boolean() {
            return Err("shadowEnabled 必须是布尔值。".into());
        }
    }
    if let Some(value) = object.get("preeditRow") {
        if !value.is_boolean() {
            return Err("preeditRow 必须是布尔值。".into());
        }
    }
    if let Some(value) = object.get("indexSuffix") {
        if value.as_str() != Some("") && value.as_str() != Some(".") {
            return Err("indexSuffix 只能是空或 \".\"。".into());
        }
    }
    if let Some(value) = object.get("edgeInsets") {
        validate_numeric_object(value, "edgeInsets", &["top", "left", "bottom", "right"])?;
    }
    if let Some(value) = object.get("minimumPanelSize") {
        validate_numeric_object(value, "minimumPanelSize", &["width", "height"])?;
    }
    let border_color_null = object.get("panelBorderColor").is_some_and(Value::is_null);
    let border_width_null = object.get("panelBorderWidth").is_some_and(Value::is_null);
    if object.contains_key("panelBorderColor")
        && object.contains_key("panelBorderWidth")
        && border_color_null != border_width_null
    {
        return Err("panelBorderColor 与 panelBorderWidth 必须同时为空或同时有值。".into());
    }
    Ok(())
}

fn validate_numeric_object(value: &Value, label: &str, fields: &[&str]) -> Result<(), String> {
    let object = value
        .as_object()
        .ok_or_else(|| format!("{label} 必须是对象。"))?;
    if object.len() != fields.len() || object.keys().any(|key| !fields.contains(&key.as_str())) {
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
    if colors.len() != 2 || !colors.contains_key("light") || !colors.contains_key("dark") {
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
    if value.trim().is_empty() {
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
    if value.chars().count() > maximum {
        return Err(format!("{field} 超过 {maximum} 字符上限。"));
    }
    Ok(value.into())
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
        fs::create_dir_all(parent).map_err(|error| format!("无法创建应用支持目录：{error}"))?;
    }
    verify_directory_path(parent)?;
    fs::create_dir(path).map_err(|error| format!("无法创建皮肤目录：{error}"))?;
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("无法设置皮肤目录权限：{error}"))?;
    verify_directory_path(path)
}

fn verify_directory_path(path: &Path) -> Result<(), String> {
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err("皮肤目录包含路径穿越。".into());
    }
    let metadata =
        fs::symlink_metadata(path).map_err(|error| format!("无法检查皮肤目录：{error}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err("皮肤目录不是安全的普通目录。".into());
    }
    Ok(())
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
}
