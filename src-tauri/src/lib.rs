mod context_model;
#[cfg(target_os = "macos")]
mod friend_install;
mod hotword;
mod ime_preferences;
mod keychain;
mod knowledge;
mod layer2;
mod maintenance;
mod mcp;
mod native_integration;
mod profile_introspection;
mod replay_gate;
mod skin_catalog;
mod skin_packages;
mod snapshot;
mod storage;
#[cfg(feature = "test-upload")]
pub mod test_upload;
mod userdb_maintenance;

use hotword::{
    refresh_remote_source, DynamicSource, DynamicSourceInput, HotwordItem, RefreshResult,
};
use knowledge::{
    HotwordGenerationStageReport, IngestReport, KnowledgeSearchHit, KnowledgeSource,
    KnowledgeSourceInput, KnowledgeTerm,
};
use maintenance::{
    learning_cleanup_list_at, BackupPreview, IntegrityReport, LearningCleanupItem,
    LearningCleanupList, LearningCleanupPreview, LearningCleanupResult, LearningPreferences,
    ScaleProbeReport,
};
use replay_gate::{FrozenGoldenMeta, RecordedQualityEvent, ReplayGateReport, ReplayRunInput};
use serde::{Deserialize, Serialize};
use skin_packages::CandidateSkinCatalogSnapshot;
use snapshot::{LexiconSnapshotEntry, LexiconSnapshotSource, SnapshotMeta, SnapshotStore};
#[cfg(unix)]
use std::os::fd::AsRawFd;
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::{
    fs,
    io::Write,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use storage::{
    AppState, AppStorage, LexiconEntry, LexiconEntryInput, LexiconImportPreview,
    LexiconImportResult, PersonalProfileAutoUpdateItem, PersonalProfileAutoUpdateReport,
    ProjectProfile, ProjectProfileInput, ProjectRuntimeConfiguration,
};
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, RunEvent, Runtime, State, WindowEvent,
};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, ShortcutState};

const MAIN_WINDOW_LABEL: &str = "main";
const COMPACT_WINDOW_LABEL: &str = "compact";
const FOCUS_INPUT_EVENT: &str = "agent-input-layer://focus-input";
const PROVIDER_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const QUALITY_CONSENT_NOTICE_VERSION: &str = "w7-v1";
const MAXIMUM_QUALITY_BYTES: usize = 32 * 1024 * 1024;
static QUALITY_TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProviderGenerateRequest {
    payload: String,
    provider: ProviderConfig,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProviderConfig {
    #[serde(default = "default_provider_kind")]
    kind: String,
    base_url: String,
    model: String,
}

fn default_provider_kind() -> String {
    "openai-compatible".into()
}

#[derive(Debug, Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    message: ChatChoiceMessage,
}

#[derive(Debug, Deserialize)]
struct ChatChoiceMessage {
    content: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct QualityRecordingConfiguration {
    enabled: bool,
    expires_at_ms: i64,
    #[serde(default)]
    continuous: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct QualityRecordingConsent {
    granted: bool,
    granted_at_ms: i64,
    notice_version: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct QualityConsentRevocationResult {
    consent: QualityRecordingConsent,
    deleted_count: usize,
}

#[cfg(unix)]
struct QualityFileLock(fs::File);

#[cfg(unix)]
impl Drop for QualityFileLock {
    fn drop(&mut self) {
        let _ = unsafe { flock(self.0.as_raw_fd(), QUALITY_LOCK_UN) };
    }
}

#[cfg(unix)]
const QUALITY_LOCK_EX: std::ffi::c_int = 2;
#[cfg(unix)]
const QUALITY_LOCK_UN: std::ffi::c_int = 8;

#[cfg(unix)]
extern "C" {
    fn flock(descriptor: std::ffi::c_int, operation: std::ffi::c_int) -> std::ffi::c_int;
}

fn provider_http_status_error(status: reqwest::StatusCode) -> String {
    if matches!(
        status,
        reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN
    ) {
        "Provider 鉴权失败，请检查 API Key 与账号权限；已回退到本地模板。".into()
    } else if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        "Provider 请求过于频繁（429），请稍后重试；已回退到本地模板。".into()
    } else {
        format!("Provider 返回 HTTP {status}，已回退到本地模板。")
    }
}

fn provider_transport_error(error: &reqwest::Error) -> String {
    if error.is_timeout() {
        "Provider 请求超时，已回退到本地模板。".into()
    } else if error.is_connect() {
        "当前无法连接 Provider（无网络或服务未启动），已回退到本地模板。".into()
    } else {
        "Provider 网络请求失败，已回退到本地模板。".into()
    }
}

trait ProviderAdapter {
    fn endpoint(&self, base_url: &str) -> Result<String, String>;
    fn request(
        &self,
        client: &reqwest::Client,
        endpoint: String,
        config: &ProviderConfig,
        api_key: &str,
        payload: &str,
    ) -> reqwest::RequestBuilder;
    fn response_text(&self, bytes: &[u8]) -> Result<String, String>;
}

struct OpenAiCompatibleAdapter;
struct AnthropicMessagesAdapter;
struct OllamaAdapter;

impl ProviderAdapter for OpenAiCompatibleAdapter {
    fn endpoint(&self, base_url: &str) -> Result<String, String> {
        validate_remote_provider_base_url(base_url)?;
        Ok(if base_url.ends_with("/chat/completions") {
            base_url.into()
        } else {
            format!("{base_url}/chat/completions")
        })
    }

    fn request(
        &self,
        client: &reqwest::Client,
        endpoint: String,
        config: &ProviderConfig,
        api_key: &str,
        payload: &str,
    ) -> reqwest::RequestBuilder {
        client
            .post(endpoint)
            .bearer_auth(api_key.trim())
            .json(&serde_json::json!({
                "model": config.model.trim(), "temperature": 0.2,
                "messages": [{ "role": "user", "content": payload }]
            }))
    }

    fn response_text(&self, bytes: &[u8]) -> Result<String, String> {
        let completion: ChatCompletionResponse = serde_json::from_slice(bytes)
            .map_err(|_| "Provider 响应格式无效，已回退到本地模板。".to_string())?;
        completion
            .choices
            .into_iter()
            .find_map(|choice| choice.message.content)
            .map(|content| content.trim().to_string())
            .filter(|content| !content.is_empty())
            .ok_or_else(|| "Provider 没有返回可用内容。".into())
    }
}

impl ProviderAdapter for AnthropicMessagesAdapter {
    fn endpoint(&self, base_url: &str) -> Result<String, String> {
        validate_remote_provider_base_url(base_url)?;
        Ok(if base_url.ends_with("/messages") {
            base_url.into()
        } else if base_url.ends_with("/v1") {
            format!("{base_url}/messages")
        } else {
            format!("{base_url}/v1/messages")
        })
    }

    fn request(
        &self,
        client: &reqwest::Client,
        endpoint: String,
        config: &ProviderConfig,
        api_key: &str,
        payload: &str,
    ) -> reqwest::RequestBuilder {
        client
            .post(endpoint)
            .header("x-api-key", api_key.trim())
            .header("anthropic-version", "2023-06-01")
            .json(&serde_json::json!({
                "model": config.model.trim(), "max_tokens": 2048,
                "messages": [{ "role": "user", "content": payload }]
            }))
    }

    fn response_text(&self, bytes: &[u8]) -> Result<String, String> {
        let value: serde_json::Value = serde_json::from_slice(bytes)
            .map_err(|_| "Provider 响应格式无效，已回退到本地模板。".to_string())?;
        value
            .get("content")
            .and_then(|content| content.as_array())
            .into_iter()
            .flatten()
            .find_map(|block| {
                (block.get("type")?.as_str()? == "text")
                    .then(|| block.get("text")?.as_str())
                    .flatten()
            })
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_string)
            .ok_or_else(|| "Provider 没有返回可用内容。".into())
    }
}

impl ProviderAdapter for OllamaAdapter {
    fn endpoint(&self, base_url: &str) -> Result<String, String> {
        let url =
            reqwest::Url::parse(base_url).map_err(|_| "Ollama Base URL 无效。".to_string())?;
        let local = url.host_str().is_some_and(|host| {
            host.eq_ignore_ascii_case("localhost")
                || host
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback())
        });
        if !local {
            return Err("Ollama 只允许连接本机 loopback 地址。".into());
        }
        Ok(if base_url.ends_with("/api/chat") {
            base_url.into()
        } else {
            format!("{base_url}/api/chat")
        })
    }

    fn request(
        &self,
        client: &reqwest::Client,
        endpoint: String,
        config: &ProviderConfig,
        _api_key: &str,
        payload: &str,
    ) -> reqwest::RequestBuilder {
        client.post(endpoint).json(&serde_json::json!({
            "model": config.model.trim(), "stream": false,
            "messages": [{ "role": "user", "content": payload }]
        }))
    }

    fn response_text(&self, bytes: &[u8]) -> Result<String, String> {
        let value: serde_json::Value = serde_json::from_slice(bytes)
            .map_err(|_| "Provider 响应格式无效，已回退到本地模板。".to_string())?;
        value
            .pointer("/message/content")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_string)
            .ok_or_else(|| "Provider 没有返回可用内容。".into())
    }
}

fn provider_adapter(kind: &str) -> Result<Box<dyn ProviderAdapter + Send + Sync>, String> {
    match kind {
        "openai-compatible" => Ok(Box::new(OpenAiCompatibleAdapter)),
        "anthropic" => Ok(Box::new(AnthropicMessagesAdapter)),
        "ollama" => Ok(Box::new(OllamaAdapter)),
        _ => Err("不支持的 Provider 类型。".into()),
    }
}

// The hosted adapters carry the API key on every request, so a plain-http
// base URL would put the bearer secret on the wire and an http redirect
// target would receive it. OllamaAdapter keeps its stricter loopback-only
// rule and does not go through this gate.
fn validate_remote_provider_base_url(base_url: &str) -> Result<(), String> {
    let url = reqwest::Url::parse(base_url).map_err(|_| "Provider Base URL 无效。".to_string())?;
    if url.scheme() == "https" {
        return Ok(());
    }
    // host_str keeps IPv6 brackets ("[::1]"); strip them so the address
    // parses as an IpAddr.
    let host = url
        .host_str()
        .unwrap_or_default()
        .trim_matches(|c| c == '[' || c == ']');
    let local = host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback());
    if local {
        Ok(())
    } else {
        Err("非本机地址仅允许 HTTPS Base URL。".into())
    }
}

fn provider_client(timeout: Duration, endpoint_is_https: bool) -> Result<reqwest::Client, String> {
    let builder = reqwest::Client::builder().timeout(timeout);
    // https_only also refuses every http redirect target, including a
    // same-host https→http downgrade that keeps the Authorization header.
    // Loopback http endpoints instead get redirects disabled outright: they
    // are the only allowed plain-http targets and must not bounce off-box.
    let builder = if endpoint_is_https {
        builder.https_only(true)
    } else {
        builder.redirect(reqwest::redirect::Policy::none())
    };
    builder
        .build()
        .map_err(|error| format!("无法初始化网络客户端：{error}"))
}

fn show_main_window<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.center();
        let _ = window.set_focus();
        let _ = app.emit_to(MAIN_WINDOW_LABEL, FOCUS_INPUT_EVENT, ());
    }
}

fn show_compact_panel<R: Runtime>(app: &AppHandle<R>) {
    if let Some(state) = app.try_state::<native_integration::NativeIntegrationState>() {
        state.capture_before_panel();
    }
    if let Some(window) = app.get_webview_window(COMPACT_WINDOW_LABEL) {
        let _ = window.unminimize();
        let _ = window.set_always_on_top(true);
        let _ = window.center();
        let _ = window.show();
        let _ = window.set_focus();
        let _ = app.emit_to(COMPACT_WINDOW_LABEL, FOCUS_INPUT_EVENT, ());
    }
}

fn hide_compact_panel<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window(COMPACT_WINDOW_LABEL) {
        let _ = window.hide();
    }
}

/// Friends' packages: asks, then removes the input method and the hourly upload task, then quits.
fn confirm_uninstall<R: Runtime>(app: &AppHandle<R>) {
    use tauri_plugin_dialog::{DialogExt, MessageDialogButtons};
    let handle = app.clone();
    app.dialog()
        .message("卸载知字输入法？\n\n会移除输入法程序和每小时的上传任务；您在知字里的输入数据留在本机。\n输入法列表里的「知字拼音」条目会在下次注销登录后消失。\n之后把「应用程序」里的知字拖进废纸篓即可。")
        .title("卸载知字")
        .buttons(MessageDialogButtons::OkCancelCustom("卸载".into(), "取消".into()))
        .show(move |confirmed| {
            if !confirmed {
                return;
            }
            #[cfg(target_os = "macos")]
            let removed = friend_install::uninstall_input_method();
            #[cfg(not(target_os = "macos"))]
            let removed = false;
            let message = if removed { "已卸载。" } else { "没能卸载干净，请把 ~/Library/Input Methods 里的 Agent Input Method.app 拖进废纸篓。" };
            let quit = handle.clone();
            handle.dialog().message(message).title("卸载知字").show(move |_| quit.exit(0));
        });
}

fn install_tray<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let show_panel = MenuItem::with_id(app, "show-panel", "显示小面板", true, None::<&str>)?;
    let show_workspace = MenuItem::with_id(
        app,
        "show-workspace",
        "打开知字",
        true,
        None::<&str>,
    )?;
    let hide_panel = MenuItem::with_id(app, "hide-panel", "隐藏小面板", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let uninstall = MenuItem::with_id(app, "uninstall", "卸载知字输入法…", true, None::<&str>)?;
    let menu = if friend_install::is_friend_build() {
        Menu::with_items(
            app,
            &[&show_workspace, &show_panel, &hide_panel, &separator, &uninstall, &quit],
        )?
    } else {
        Menu::with_items(
            app,
            &[&show_workspace, &show_panel, &hide_panel, &separator, &quit],
        )?
    };

    let mut tray = TrayIconBuilder::with_id("main")
        .tooltip("Agent Input Method")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "show-panel" => show_compact_panel(app),
            "show-workspace" => show_main_window(app),
            "hide-panel" => hide_compact_panel(app),
            "quit" => app.exit(0),
            "uninstall" => confirm_uninstall(app),
            _ => {}
        })
        .on_tray_icon_event(|tray: &TrayIcon<R>, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        });

    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone()).icon_as_template(true);
    }

    tray.build(app)?;
    Ok(())
}

#[tauri::command]
fn load_app_state(storage: State<'_, AppStorage>, fallback: AppState) -> Result<AppState, String> {
    storage.load_app_state(fallback)
}

#[tauri::command]
fn save_app_state(storage: State<'_, AppStorage>, state: AppState) -> Result<(), String> {
    storage.save_app_state(&state)
}

#[tauri::command]
fn create_lexicon_entry(
    storage: State<'_, AppStorage>,
    input: LexiconEntryInput,
) -> Result<LexiconEntry, String> {
    storage.create_lexicon_entry(input)
}

#[tauri::command]
fn update_lexicon_entry(
    storage: State<'_, AppStorage>,
    input: LexiconEntryInput,
) -> Result<LexiconEntry, String> {
    storage.update_lexicon_entry(input)
}

#[tauri::command]
fn list_lexicon_entries(storage: State<'_, AppStorage>) -> Result<Vec<LexiconEntry>, String> {
    storage.list_lexicon_entries()
}

#[tauri::command]
fn delete_lexicon_entry(storage: State<'_, AppStorage>, id: String) -> Result<bool, String> {
    storage.delete_lexicon_entry(&id)
}

#[tauri::command]
fn delete_lexicon_entries(
    storage: State<'_, AppStorage>,
    ids: Vec<String>,
) -> Result<usize, String> {
    storage.delete_lexicon_entries(ids)
}

#[tauri::command]
fn preview_lexicon_import(
    storage: State<'_, AppStorage>,
    document: String,
) -> Result<LexiconImportPreview, String> {
    storage.preview_lexicon_import(&document)
}

#[tauri::command]
fn import_lexicon(
    storage: State<'_, AppStorage>,
    document: String,
) -> Result<LexiconImportResult, String> {
    storage.import_lexicon(&document, current_time_ms()?)
}

#[tauri::command]
fn undo_lexicon_import(storage: State<'_, AppStorage>, batch_id: String) -> Result<usize, String> {
    storage.undo_lexicon_import(&batch_id, current_time_ms()?)
}

#[tauri::command]
fn export_lexicon(storage: State<'_, AppStorage>) -> Result<String, String> {
    storage.export_lexicon()
}

#[tauri::command]
fn clear_learned_lexicon_entries(storage: State<'_, AppStorage>) -> Result<usize, String> {
    storage.clear_learned_lexicon_entries()
}

#[tauri::command]
fn learning_preferences(storage: State<'_, AppStorage>) -> Result<LearningPreferences, String> {
    storage.learning_preferences()
}

#[tauri::command]
fn save_learning_preferences(
    storage: State<'_, AppStorage>,
    preferences: LearningPreferences,
) -> Result<LearningPreferences, String> {
    storage.save_learning_preferences(preferences)
}

fn learning_cleanup_items_inner(storage: &AppStorage) -> Result<LearningCleanupList, String> {
    let mut items = storage
        .learned_entries()?
        .into_iter()
        .map(|entry| LearningCleanupItem {
            id: entry.id,
            text: entry.text,
            pinyin: entry.pinyin,
            source: "sqlite_learned".into(),
            domain: (!entry.project_domains.is_empty()).then(|| entry.project_domains.join("/")),
            learned_at_ms: Some(entry.created_at_ms),
            last_hit_at_ms: None,
            factory_top1: None,
            displaced_factory_top1: false,
            attribution: None,
            batch_cleanup_eligible: false,
            friction_event_count: 0,
            abandoned_count: 0,
            deep_page_count: 0,
            backspace_count: 0,
            suspected: false,
            model_audit: None,
        })
        .collect::<Vec<_>>();
    items.extend(
        userdb_maintenance::list()?
            .into_iter()
            .map(|entry| LearningCleanupItem {
                id: entry.id,
                text: entry.text,
                pinyin: entry.pinyin,
                source: "rime_userdb".into(),
                domain: None,
                learned_at_ms: None,
                last_hit_at_ms: None,
                factory_top1: None,
                displaced_factory_top1: false,
                attribution: None,
                batch_cleanup_eligible: false,
                friction_event_count: 0,
                abandoned_count: 0,
                deep_page_count: 0,
                backspace_count: 0,
                suspected: false,
                model_audit: None,
            }),
    );
    let directory = quality_directory()?;
    let events = read_quality_records_at(&directory)?;
    Ok(learning_cleanup_list_at(items, &events, &directory))
}

#[tauri::command]
fn list_learning_cleanup_items(
    storage: State<'_, AppStorage>,
) -> Result<LearningCleanupList, String> {
    learning_cleanup_items_inner(&storage)
}

#[tauri::command]
fn preview_learning_cleanup(
    storage: State<'_, AppStorage>,
    ids: Vec<String>,
    domain: Option<String>,
) -> Result<LearningCleanupPreview, String> {
    let requested = ids
        .iter()
        .cloned()
        .collect::<std::collections::HashSet<_>>();
    let items = learning_cleanup_items_inner(&storage)?
        .items
        .into_iter()
        .filter(|item| {
            (requested.is_empty() || requested.contains(&item.id))
                && (item.source == "rime_userdb"
                    || domain.as_ref().is_none_or(|domain| {
                        item.domain
                            .as_ref()
                            .is_some_and(|value| value.split('/').any(|value| value == domain))
                    }))
        })
        .collect::<Vec<_>>();
    Ok(LearningCleanupPreview {
        entry_count: items.len(),
        entry_ids: items.iter().map(|item| item.id.clone()).collect(),
        texts: items
            .iter()
            .take(20)
            .map(|item| item.text.clone())
            .collect(),
    })
}

#[tauri::command]
fn apply_learning_cleanup(
    storage: State<'_, AppStorage>,
    ids: Vec<String>,
    domain: Option<String>,
) -> Result<LearningCleanupResult, String> {
    if ids.is_empty() {
        return Err("请先勾选要清理的学习记录。".into());
    }
    let requested = ids
        .iter()
        .cloned()
        .collect::<std::collections::HashSet<_>>();
    let available_userdb = userdb_maintenance::list()?;
    let selected_userdb = available_userdb
        .into_iter()
        .filter(|item| requested.contains(&item.id))
        .collect::<Vec<_>>();
    let missing_userdb = ids
        .iter()
        .filter(|id| id.starts_with("userdb:"))
        .any(|id| !selected_userdb.iter().any(|item| &item.id == id));
    if missing_userdb {
        return Err("所选 UserDB 学习记录已变化，请刷新后重试。".into());
    }
    let deleted_userdb = userdb_maintenance::delete(&selected_userdb)?;
    let sqlite_ids = ids
        .into_iter()
        .filter(|id| !id.starts_with("userdb:"))
        .collect::<Vec<_>>();
    match storage.apply_learning_cleanup_with_userdb(
        sqlite_ids,
        domain,
        deleted_userdb.clone(),
        current_time_ms()?,
    ) {
        Ok(result) => Ok(result),
        Err(error) => {
            userdb_maintenance::restore(&deleted_userdb)
                .map_err(|rollback| format!("{error}；且 UserDB 回滚失败：{rollback}"))?;
            Err(error)
        }
    }
}

#[tauri::command]
fn undo_learning_cleanup(
    storage: State<'_, AppStorage>,
    batch_id: String,
) -> Result<usize, String> {
    let userdb_entries = storage.learning_cleanup_userdb_entries(&batch_id)?;
    userdb_maintenance::restore(&userdb_entries)?;
    match storage.undo_learning_cleanup(&batch_id, current_time_ms()?) {
        Ok(count) => Ok(count + userdb_entries.len()),
        Err(error) => {
            userdb_maintenance::delete(&userdb_entries)
                .map_err(|rollback| format!("{error}；且 UserDB 撤销回滚失败：{rollback}"))?;
            Err(error)
        }
    }
}

#[tauri::command]
fn export_maintenance_backup(storage: State<'_, AppStorage>) -> Result<String, String> {
    storage.export_maintenance_backup(current_time_ms()?)
}

#[tauri::command]
fn preview_maintenance_backup(
    storage: State<'_, AppStorage>,
    document: String,
) -> Result<BackupPreview, String> {
    storage.preview_maintenance_backup(&document)
}

#[tauri::command]
fn restore_maintenance_backup(
    storage: State<'_, AppStorage>,
    document: String,
) -> Result<BackupPreview, String> {
    storage.restore_maintenance_backup(&document)
}

#[tauri::command]
fn run_integrity_report(storage: State<'_, AppStorage>) -> Result<IntegrityReport, String> {
    storage.integrity_report()
}

#[tauri::command]
fn run_scale_probe() -> Result<ScaleProbeReport, String> {
    maintenance::run_scale_probe()
}

#[tauri::command]
fn list_personal_blacklist() -> Result<Vec<userdb_maintenance::Item>, String> {
    userdb_maintenance::list_blacklist()
}

#[tauri::command]
fn undo_personal_blacklist(ids: Vec<String>) -> Result<usize, String> {
    userdb_maintenance::remove_blacklist(&ids)
}

#[tauri::command]
fn save_knowledge_source(
    storage: State<'_, AppStorage>,
    input: KnowledgeSourceInput,
) -> Result<KnowledgeSource, String> {
    storage.save_knowledge_source(input)
}

#[tauri::command]
fn list_knowledge_sources(storage: State<'_, AppStorage>) -> Result<Vec<KnowledgeSource>, String> {
    storage.list_knowledge_sources()
}

#[tauri::command]
fn ingest_knowledge_source(
    storage: State<'_, AppStorage>,
    id: String,
) -> Result<IngestReport, String> {
    storage.ingest_knowledge_source(&id, current_time_ms()?)
}

#[tauri::command]
fn list_knowledge_terms(storage: State<'_, AppStorage>) -> Result<Vec<KnowledgeTerm>, String> {
    storage.list_knowledge_terms()
}

#[tauri::command]
fn local_pinyin_readings(texts: Vec<String>) -> std::collections::BTreeMap<String, String> {
    knowledge::local_pinyin_readings(&texts)
}

#[tauri::command]
fn stage_generated_hotword_candidates(
    storage: State<'_, AppStorage>,
    output: String,
) -> Result<HotwordGenerationStageReport, String> {
    storage.stage_generated_hotword_candidates(&output, current_time_ms()?)
}

#[tauri::command]
fn review_knowledge_term(
    storage: State<'_, AppStorage>,
    id: String,
    state: String,
    text: String,
    pinyin: String,
    definition: String,
    candidate_layer: String,
    fixed: bool,
) -> Result<KnowledgeTerm, String> {
    storage.review_knowledge_term_details(
        &id,
        &state,
        &text,
        &pinyin,
        &definition,
        &candidate_layer,
        fixed,
        current_time_ms()?,
    )
}

#[tauri::command]
fn delete_knowledge_source(
    storage: State<'_, AppStorage>,
    id: String,
    purge: bool,
) -> Result<bool, String> {
    storage.delete_knowledge_source(&id, purge, current_time_ms()?)
}

#[tauri::command]
fn set_knowledge_source_retrieval_policy(
    storage: State<'_, AppStorage>,
    id: String,
    retrieval_status: String,
    sensitive_enabled: bool,
) -> Result<KnowledgeSource, String> {
    storage.set_knowledge_source_retrieval_policy(
        &id,
        &retrieval_status,
        sensitive_enabled,
        current_time_ms()?,
    )
}

#[tauri::command]
fn search_knowledge(
    storage: State<'_, AppStorage>,
    query: String,
    project_id: String,
    limit: Option<usize>,
) -> Result<Vec<KnowledgeSearchHit>, String> {
    storage.search_knowledge(&query, &project_id, limit.unwrap_or(5))
}

#[tauri::command]
fn save_dynamic_source(
    storage: State<'_, AppStorage>,
    input: DynamicSourceInput,
) -> Result<DynamicSource, String> {
    storage.save_dynamic_source(input)
}

#[tauri::command]
fn list_dynamic_sources(storage: State<'_, AppStorage>) -> Result<Vec<DynamicSource>, String> {
    storage.list_dynamic_sources()
}

#[tauri::command]
fn set_dynamic_source_enabled(
    storage: State<'_, AppStorage>,
    id: String,
    enabled: bool,
) -> Result<DynamicSource, String> {
    storage.set_dynamic_source_enabled(&id, enabled)
}

#[tauri::command]
fn clear_dynamic_source(storage: State<'_, AppStorage>, id: String) -> Result<usize, String> {
    storage.clear_dynamic_source(&id)
}

#[tauri::command]
fn list_hotword_items(storage: State<'_, AppStorage>) -> Result<Vec<HotwordItem>, String> {
    storage.list_hotword_items()
}

/// Recomputes the exposure/misselection columns the data-source panel displays.
///
/// Kept as an explicit command rather than folded into `list_hotword_items` so the
/// read path stays a read: the quality log is capped at 32 MiB and re-parsed in full
/// here, which is not something a panel refresh should pay silently.
#[tauri::command]
fn refresh_hotword_exposure(
    storage: State<'_, AppStorage>,
) -> Result<hotword::HotwordExposureReport, String> {
    let events = replay_gate::read_quality_events(&quality_directory()?)?;
    storage.recompute_hotword_exposure(&events, current_time_ms()?)
}

#[tauri::command]
fn ingest_local_hotword_source(
    storage: State<'_, AppStorage>,
    id: String,
    document: String,
) -> Result<RefreshResult, String> {
    storage.ingest_hotword_feed(&id, document.as_bytes(), current_time_ms()?)
}

#[tauri::command]
async fn refresh_dynamic_source(
    storage: State<'_, AppStorage>,
    id: String,
) -> Result<RefreshResult, String> {
    let now_ms = current_time_ms()?;
    match refresh_remote_source(&storage, &id, now_ms).await {
        Ok(result) => Ok(result),
        Err(error) => {
            let _ = storage.record_dynamic_source_error(&id, &error);
            Err(error)
        }
    }
}

fn current_time_ms() -> Result<i64, String> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "系统时间无效。".to_string())?
        .as_millis();
    i64::try_from(millis).map_err(|_| "系统时间超出范围。".to_string())
}

fn quality_directory() -> Result<std::path::PathBuf, String> {
    let home = std::env::var_os("HOME").ok_or_else(|| "无法确定用户目录。".to_string())?;
    Ok(std::path::PathBuf::from(home).join("Library/Application Support/AgentInputMethod/Quality"))
}

fn prepare_quality_directory() -> Result<std::path::PathBuf, String> {
    let directory = quality_directory()?;
    fs::create_dir_all(&directory).map_err(|error| format!("无法创建质量记录目录：{error}"))?;
    let metadata = fs::symlink_metadata(&directory)
        .map_err(|error| format!("无法检查质量记录目录：{error}"))?;
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        return Err("质量记录目录不是安全的本地目录。".into());
    }
    #[cfg(unix)]
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("无法设置质量记录目录权限：{error}"))?;
    Ok(directory)
}

fn read_regular_local_file(path: &std::path::Path, label: &str) -> Result<Option<Vec<u8>>, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("无法检查{label}：{error}")),
    };
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err(format!("{label}不是安全的本地普通文件。"));
    }
    fs::read(path)
        .map(Some)
        .map_err(|error| format!("无法读取{label}：{error}"))
}

#[cfg(unix)]
fn acquire_quality_lock(directory: &std::path::Path) -> Result<QualityFileLock, String> {
    let path = directory.join(".quality.lock");
    if let Ok(metadata) = fs::symlink_metadata(&path) {
        if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
            return Err("质量记录锁不是安全的本地普通文件。".into());
        }
    }
    let mut options = fs::OpenOptions::new();
    options.read(true).write(true).create(true).mode(0o600);
    let file = options
        .open(&path)
        .map_err(|error| format!("无法打开质量记录锁：{error}"))?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("无法保护质量记录锁：{error}"))?;
    loop {
        if unsafe { flock(file.as_raw_fd(), QUALITY_LOCK_EX) } == 0 {
            return Ok(QualityFileLock(file));
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            return Err(format!("无法锁定质量记录：{error}"));
        }
    }
}

fn atomic_write_quality_file(
    directory: &std::path::Path,
    name: &str,
    data: &[u8],
    label: &str,
) -> Result<(), String> {
    let sequence = QUALITY_TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary = directory.join(format!(".{name}-{}-{sequence}.tmp", std::process::id()));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let result = (|| {
        let mut file = options
            .open(&temporary)
            .map_err(|error| format!("无法写入{label}：{error}"))?;
        file.write_all(data)
            .and_then(|_| file.sync_all())
            .map_err(|error| format!("无法持久化{label}：{error}"))?;
        fs::rename(&temporary, directory.join(name))
            .map_err(|error| format!("无法替换{label}：{error}"))?;
        fs::File::open(directory)
            .and_then(|file| file.sync_all())
            .map_err(|error| format!("无法同步质量记录目录：{error}"))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn quality_recording_status_at(
    directory: &std::path::Path,
) -> Result<QualityRecordingConfiguration, String> {
    let path = directory.join("config.json");
    let Some(data) = read_regular_local_file(&path, "质量记录配置")? else {
        return Ok(QualityRecordingConfiguration {
            enabled: false,
            expires_at_ms: 0,
            continuous: false,
        });
    };
    if data.len() > 4_096 {
        return Err("质量记录配置超过大小上限。".into());
    }
    serde_json::from_slice(&data).map_err(|_| "质量记录配置无效。".to_string())
}

fn quality_recording_consent_status_at(
    directory: &std::path::Path,
) -> Result<QualityRecordingConsent, String> {
    let path = directory.join("consent.json");
    let Some(data) = read_regular_local_file(&path, "质量记录知情同意")? else {
        return Ok(QualityRecordingConsent {
            granted: false,
            granted_at_ms: 0,
            notice_version: QUALITY_CONSENT_NOTICE_VERSION.into(),
        });
    };
    if data.len() > 4_096 {
        return Err("质量记录知情同意超过大小上限。".into());
    }
    let mut consent: QualityRecordingConsent =
        serde_json::from_slice(&data).map_err(|_| "质量记录知情同意无效。".to_string())?;
    if consent.notice_version != QUALITY_CONSENT_NOTICE_VERSION {
        consent.granted = false;
    }
    Ok(consent)
}

fn set_quality_recording_at(
    directory: &std::path::Path,
    mode: &str,
    now: i64,
) -> Result<QualityRecordingConfiguration, String> {
    let config = quality_recording_configuration(mode, now)?;
    let _lock = acquire_quality_lock(directory)?;
    if config.enabled && !quality_recording_consent_status_at(directory)?.granted {
        return Err("请先阅读并明确同意质量记录说明；功能开关不能代替知情同意。".into());
    }
    let data =
        serde_json::to_vec(&config).map_err(|error| format!("无法编码质量记录配置：{error}"))?;
    atomic_write_quality_file(directory, "config.json", &data, "质量记录配置")?;
    Ok(config)
}

#[tauri::command]
fn set_quality_recording(mode: String) -> Result<QualityRecordingConfiguration, String> {
    let now = current_time_ms()?;
    let directory = prepare_quality_directory()?;
    set_quality_recording_at(&directory, &mode, now)
}

fn quality_recording_configuration(
    mode: &str,
    now: i64,
) -> Result<QualityRecordingConfiguration, String> {
    let (enabled, expires_at_ms, continuous) = match mode {
        "off" => (false, 0, false),
        "twoHour" => (true, now + 2 * 60 * 60 * 1000, false),
        "continuous" => (true, 0, true),
        _ => return Err("质量记录模式无效。".into()),
    };
    let config = QualityRecordingConfiguration {
        enabled,
        expires_at_ms,
        continuous,
    };
    Ok(config)
}

#[tauri::command]
fn quality_recording_status() -> Result<QualityRecordingConfiguration, String> {
    quality_recording_status_at(&quality_directory()?)
}

#[tauri::command]
fn quality_recording_consent_status() -> Result<QualityRecordingConsent, String> {
    quality_recording_consent_status_at(&quality_directory()?)
}

#[tauri::command]
fn grant_quality_recording_consent() -> Result<QualityRecordingConsent, String> {
    let directory = prepare_quality_directory()?;
    grant_quality_recording_consent_at(&directory, current_time_ms()?)
}

/// What a test build says about recording. The owner tells friends in person when he hands the
/// build over, so recording starts on; this text stays as a plain notice (and in the install
/// message), and it must stay true to what the recorder and uploader do.
#[cfg(all(target_os = "macos", feature = "test-upload"))]
const TEST_RECORDING_NOTICE: &str = "这是知字测试版，已默认开启测试记录：\n\
· 输入的拼音、候选词、您选了哪一个\n\
· 光标前最近 8 个字（网址、邮箱、长串字母数字会先被替换掉；密码框不记录）\n\
· 电脑名称（用来区分设备）\n\n\
这些记录每小时上传一次到作者的服务器，只用来改进输入法，不会给别人。\n\
随时可以在知字首页的「常用」里关掉「持续记录」，关掉后不再记录、不再上传。";

/// A machine with no recording choice yet starts recording (the owner has told the tester), and
/// is told so. A choice already made on this machine is never overridden.
#[cfg(all(target_os = "macos", feature = "test-upload"))]
fn start_test_recording_if_unset() -> bool {
    let Ok(directory) = prepare_quality_directory() else { return false };
    if directory.join("consent.json").exists() || directory.join("config.json").exists() {
        return false;
    }
    let Ok(now) = current_time_ms() else { return false };
    grant_quality_recording_consent_at(&directory, now).is_ok()
        && set_quality_recording_at(&directory, "continuous", now).is_ok()
}

/// The Home switch: turning recording on without consent on file asks first, with the same notice.
#[cfg(all(target_os = "macos", feature = "test-upload"))]
#[tauri::command]
async fn confirm_test_recording_consent<R: tauri::Runtime>(app: tauri::AppHandle<R>) -> Result<bool, String> {
    use tauri_plugin_dialog::{DialogExt, MessageDialogButtons};
    let directory = prepare_quality_directory()?;
    if quality_recording_consent_status_at(&directory)?.granted {
        return Ok(true);
    }
    let agreed = app
        .dialog()
        .message(TEST_RECORDING_NOTICE.replacen("已默认开启测试记录", "会记录", 1))
        .title("开启测试记录吗？")
        .buttons(MessageDialogButtons::OkCancelCustom("同意并开启".into(), "不参与".into()))
        .blocking_show();
    if agreed {
        grant_quality_recording_consent_at(&directory, current_time_ms()?)?;
    }
    Ok(agreed)
}

#[cfg(not(all(target_os = "macos", feature = "test-upload")))]
#[tauri::command]
async fn confirm_test_recording_consent() -> Result<bool, String> {
    Ok(quality_recording_consent_status_at(&prepare_quality_directory()?)?.granted)
}

/// True in test builds that upload records; the UI words the recording switch accordingly.
#[tauri::command]
fn test_upload_enabled() -> bool {
    cfg!(feature = "test-upload")
}

fn grant_quality_recording_consent_at(
    directory: &std::path::Path,
    now: i64,
) -> Result<QualityRecordingConsent, String> {
    let _lock = acquire_quality_lock(directory)?;
    let disabled = quality_recording_configuration("off", now)?;
    let disabled_data =
        serde_json::to_vec(&disabled).map_err(|error| format!("无法编码质量记录配置：{error}"))?;
    atomic_write_quality_file(directory, "config.json", &disabled_data, "质量记录配置")?;
    let consent = QualityRecordingConsent {
        granted: true,
        granted_at_ms: now,
        notice_version: QUALITY_CONSENT_NOTICE_VERSION.into(),
    };
    let data = serde_json::to_vec(&consent)
        .map_err(|error| format!("无法编码质量记录知情同意：{error}"))?;
    atomic_write_quality_file(directory, "consent.json", &data, "质量记录知情同意")?;
    Ok(consent)
}

fn read_quality_records_at(
    directory: &std::path::Path,
) -> Result<Vec<RecordedQualityEvent>, String> {
    let path = directory.join("events.jsonl");
    let Some(data) = read_regular_local_file(&path, "质量记录")? else {
        return Ok(Vec::new());
    };
    if data.len() > MAXIMUM_QUALITY_BYTES {
        return Err("质量记录超过 32 MiB 上限。".into());
    }
    let text = std::str::from_utf8(&data).map_err(|_| "质量记录不是有效 UTF-8。".to_string())?;
    text.lines()
        .map(|line| serde_json::from_str(line).map_err(|_| "质量记录包含无效行。".to_string()))
        .collect()
}

#[tauri::command]
fn list_quality_records() -> Result<Vec<RecordedQualityEvent>, String> {
    Ok(read_quality_records_at(&quality_directory()?)?
        .into_iter()
        .rev()
        .collect())
}

#[tauri::command]
fn personal_profile_introspection() -> Result<serde_json::Value, String> {
    let events = replay_gate::read_quality_events(&quality_directory()?)?;
    profile_introspection::mine_profile_introspection_value(&events)
}

fn apply_profile_updates_from_quality(
    storage: &AppStorage,
    now_ms: i64,
) -> Result<PersonalProfileAutoUpdateReport, String> {
    let events = replay_gate::read_quality_events(&quality_directory()?)?;
    let report = profile_introspection::mine_profile_introspection(&events);
    let blacklist = userdb_maintenance::list_blacklist()?
        .into_iter()
        .map(|item| (item.reading, item.text))
        .collect::<std::collections::HashSet<_>>();
    storage.apply_personal_profile_auto_update(&report.confident_targets, &blacklist, now_ms)
}

#[tauri::command]
fn apply_personal_profile_auto_update(
    storage: State<'_, AppStorage>,
) -> Result<PersonalProfileAutoUpdateReport, String> {
    apply_profile_updates_from_quality(&storage, current_time_ms()?)
}

#[tauri::command]
fn declare_owner_disambiguation(
    storage: State<'_, AppStorage>,
    pinyin: String,
    preferred_text: String,
    candidates: Vec<String>,
) -> Result<PersonalProfileAutoUpdateReport, String> {
    let blacklist = userdb_maintenance::list_blacklist()?
        .into_iter()
        .map(|item| (item.reading, item.text))
        .collect::<std::collections::HashSet<_>>();
    storage.declare_owner_disambiguation(
        &pinyin,
        &preferred_text,
        &candidates,
        &blacklist,
        current_time_ms()?,
    )
}

#[tauri::command]
fn list_personal_profile_auto_updates(
    storage: State<'_, AppStorage>,
) -> Result<Vec<PersonalProfileAutoUpdateItem>, String> {
    storage.list_personal_profile_auto_updates()
}

#[tauri::command]
fn revoke_personal_profile_auto_update(
    storage: State<'_, AppStorage>,
    id: String,
) -> Result<PersonalProfileAutoUpdateItem, String> {
    storage.revoke_personal_profile_auto_update(&id, current_time_ms()?)
}

#[tauri::command]
fn export_quality_records() -> Result<String, String> {
    serde_json::to_string_pretty(&list_quality_records()?)
        .map_err(|error| format!("无法导出质量记录：{error}"))
}

fn rewrite_quality_records_at<F>(
    directory: &std::path::Path,
    mut should_delete: F,
) -> Result<usize, String>
where
    F: FnMut(&RecordedQualityEvent) -> bool,
{
    let events = read_quality_records_at(directory)?;
    let original_count = events.len();
    let retained = events
        .into_iter()
        .filter(|event| !should_delete(event))
        .collect::<Vec<_>>();
    let deleted_count = original_count.saturating_sub(retained.len());
    if deleted_count == 0 {
        return Ok(0);
    }
    let path = directory.join("events.jsonl");
    if retained.is_empty() {
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("无法删除质量记录：{error}")),
        }
        fs::File::open(directory)
            .and_then(|file| file.sync_all())
            .map_err(|error| format!("无法同步质量记录删除：{error}"))?;
        return Ok(deleted_count);
    }
    let mut data = Vec::new();
    for event in retained {
        serde_json::to_writer(&mut data, &event)
            .map_err(|error| format!("无法编码质量记录：{error}"))?;
        data.push(b'\n');
    }
    atomic_write_quality_file(directory, "events.jsonl", &data, "质量记录")?;
    Ok(deleted_count)
}

fn revoke_quality_recording_consent_at(
    directory: &std::path::Path,
    delete_records: bool,
    now: i64,
) -> Result<QualityConsentRevocationResult, String> {
    let _lock = acquire_quality_lock(directory)?;
    let consent = QualityRecordingConsent {
        granted: false,
        granted_at_ms: 0,
        notice_version: QUALITY_CONSENT_NOTICE_VERSION.into(),
    };
    let consent_data = serde_json::to_vec(&consent)
        .map_err(|error| format!("无法编码质量记录知情同意：{error}"))?;
    atomic_write_quality_file(directory, "consent.json", &consent_data, "质量记录知情同意")?;
    let config = quality_recording_configuration("off", now)?;
    let config_data =
        serde_json::to_vec(&config).map_err(|error| format!("无法编码质量记录配置：{error}"))?;
    atomic_write_quality_file(directory, "config.json", &config_data, "质量记录配置")?;
    let deleted_count = if delete_records {
        rewrite_quality_records_at(directory, |_| true)?
    } else {
        0
    };
    Ok(QualityConsentRevocationResult {
        consent,
        deleted_count,
    })
}

#[tauri::command]
fn revoke_quality_recording_consent(
    delete_records: bool,
) -> Result<QualityConsentRevocationResult, String> {
    let directory = prepare_quality_directory()?;
    revoke_quality_recording_consent_at(&directory, delete_records, current_time_ms()?)
}

#[tauri::command]
fn delete_quality_records() -> Result<usize, String> {
    let directory = prepare_quality_directory()?;
    let _lock = acquire_quality_lock(&directory)?;
    rewrite_quality_records_at(&directory, |_| true)
}

#[tauri::command]
fn delete_quality_record(id: String) -> Result<usize, String> {
    let id = id.trim();
    if id.is_empty() || id.len() > 128 {
        return Err("质量记录 ID 无效。".into());
    }
    let directory = prepare_quality_directory()?;
    let _lock = acquire_quality_lock(&directory)?;
    rewrite_quality_records_at(&directory, |event| event.id == id)
}

#[tauri::command]
fn delete_quality_records_by_app(app_hash: String) -> Result<usize, String> {
    let app_hash = app_hash.trim().to_ascii_lowercase();
    if app_hash.len() != 64 || !app_hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("App 标识散列无效。".into());
    }
    let directory = prepare_quality_directory()?;
    let _lock = acquire_quality_lock(&directory)?;
    rewrite_quality_records_at(&directory, |event| event.app_hash == app_hash)
}

#[tauri::command]
fn delete_quality_records_by_time_range(start_ms: i64, end_ms: i64) -> Result<usize, String> {
    if start_ms < 0 || end_ms < start_ms {
        return Err("删除时间范围无效。".into());
    }
    let directory = prepare_quality_directory()?;
    let _lock = acquire_quality_lock(&directory)?;
    rewrite_quality_records_at(&directory, |event| {
        event.recorded_at_ms >= start_ms && event.recorded_at_ms <= end_ms
    })
}

#[tauri::command]
fn update_panel_shortcut<R: Runtime>(app: AppHandle<R>, shortcut: String) -> Result<(), String> {
    let shortcut = shortcut.trim();

    match shortcut {
        "alt+space" | "super+shift+space" | "control+space" => {}
        _ => return Err("暂不支持这个快捷键。".into()),
    }

    app.global_shortcut()
        .unregister_all()
        .map_err(|error| format!("无法清理旧快捷键：{error}"))?;
    app.global_shortcut()
        .on_shortcut(shortcut, |app, _shortcut, event| {
            if event.state == ShortcutState::Pressed {
                show_compact_panel(app);
            }
        })
        .map_err(|error| format!("无法注册快捷键：{error}"))?;

    Ok(())
}

#[tauri::command]
async fn generate_with_provider(
    storage: State<'_, AppStorage>,
    request: ProviderGenerateRequest,
) -> Result<String, String> {
    if request.payload.is_empty() {
        return Err(
            "Provider payload 不能为空；请先调用 prepare_generation_payload 获取预览内容。".into(),
        );
    }
    let api_key = storage.provider_api_key(request.provider.kind.trim())?;
    generate_with_provider_timeout(request, PROVIDER_REQUEST_TIMEOUT, api_key).await
}

#[tauri::command]
fn prepare_generation_payload(
    storage: State<'_, AppStorage>,
    request: layer2::ContextAssemblyRequest,
) -> Result<layer2::PayloadPreview, String> {
    layer2::prepare_generation_payload(&storage, request)
}

#[tauri::command]
fn latest_selection_capture(
    state: State<'_, native_integration::NativeIntegrationState>,
) -> Result<native_integration::SelectionCapture, String> {
    state.latest_capture()
}

#[tauri::command]
fn external_text_access_status(
    state: State<'_, native_integration::NativeIntegrationState>,
) -> Result<native_integration::ExternalTextAccess, String> {
    state.external_text_access()
}

#[tauri::command]
fn perform_external_text_action(
    state: State<'_, native_integration::NativeIntegrationState>,
    action: native_integration::ExternalTextAction,
    text: String,
) -> Result<native_integration::ExternalTextActionResult, String> {
    state.perform_text_action(action, &text)
}

#[tauri::command]
fn set_mcp_server_enabled(
    app: AppHandle,
    state: State<'_, mcp::McpServerState>,
    enabled: bool,
) -> Result<mcp::McpServerStatus, String> {
    state.set_enabled(app, enabled)
}

#[tauri::command]
fn mcp_server_status(
    state: State<'_, mcp::McpServerState>,
) -> Result<mcp::McpServerStatus, String> {
    state.status()
}

async fn generate_with_provider_timeout(
    request: ProviderGenerateRequest,
    timeout: Duration,
    api_key: String,
) -> Result<String, String> {
    let base_url = request.provider.base_url.trim().trim_end_matches('/');
    let model = request.provider.model.trim();
    let kind = request.provider.kind.trim();

    if request.payload.is_empty() {
        return Err(
            "Provider payload 不能为空；请先调用 prepare_generation_payload 获取预览内容。".into(),
        );
    }
    if base_url.is_empty() || model.is_empty() || (kind != "ollama" && api_key.trim().is_empty()) {
        return Err("请先填写完整 Provider 配置。".into());
    }
    let adapter = provider_adapter(kind)?;
    let endpoint = adapter.endpoint(base_url)?;
    let endpoint_is_https = reqwest::Url::parse(&endpoint).is_ok_and(|url| url.scheme() == "https");
    let client = provider_client(timeout, endpoint_is_https)?;
    let response = adapter
        .request(
            &client,
            endpoint,
            &request.provider,
            &api_key,
            &request.payload,
        )
        .send()
        .await
        .map_err(|error| provider_transport_error(&error))?;
    let status = response.status();

    if !status.is_success() {
        return Err(provider_http_status_error(status));
    }

    let bytes = response
        .bytes()
        .await
        .map_err(|error| provider_transport_error(&error))?;
    adapter.response_text(&bytes)
}

fn shared_snapshot_store() -> Result<SnapshotStore, String> {
    let home = std::env::var_os("HOME").ok_or_else(|| "无法确定用户目录。".to_string())?;
    Ok(SnapshotStore::new(std::path::PathBuf::from(home).join(
        "Library/Application Support/AgentInputMethod/Shared/Snapshots",
    )))
}

fn project_runtime_path() -> Result<std::path::PathBuf, String> {
    let home = std::env::var_os("HOME").ok_or_else(|| "无法确定用户目录。".to_string())?;
    Ok(std::path::PathBuf::from(home)
        .join("Library/Application Support/AgentInputMethod/Shared/Profiles/runtime.json"))
}

fn publish_project_runtime(config: &ProjectRuntimeConfiguration) -> Result<(), String> {
    let path = project_runtime_path()?;
    let directory = path
        .parent()
        .ok_or_else(|| "项目运行时目录无效。".to_string())?;
    fs::create_dir_all(directory).map_err(|error| format!("无法创建项目运行时目录：{error}"))?;
    let metadata = fs::symlink_metadata(directory)
        .map_err(|error| format!("无法检查项目运行时目录：{error}"))?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
        return Err("项目运行时目录不是安全的本地目录。".into());
    }
    #[cfg(unix)]
    fs::set_permissions(directory, fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("无法设置项目运行时目录权限：{error}"))?;
    let now = current_time_ms()?;
    let temporary = directory.join(format!(".runtime-{}-{now}.tmp", std::process::id()));
    let data =
        serde_json::to_vec(config).map_err(|error| format!("无法编码项目运行时配置：{error}"))?;
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(&temporary)
        .map_err(|error| format!("无法写入项目运行时配置：{error}"))?;
    file.write_all(&data)
        .and_then(|_| file.sync_all())
        .map_err(|error| format!("无法持久化项目运行时配置：{error}"))?;
    fs::rename(&temporary, &path).map_err(|error| format!("无法激活项目运行时配置：{error}"))?;
    Ok(())
}

#[tauri::command]
fn save_project_profile(
    storage: State<'_, AppStorage>,
    input: ProjectProfileInput,
) -> Result<ProjectProfile, String> {
    let profile = storage.save_project_profile(input)?;
    let runtime = storage.project_runtime_configuration()?;
    publish_project_runtime(&runtime)?;
    Ok(profile)
}

#[tauri::command]
fn list_project_profiles(storage: State<'_, AppStorage>) -> Result<Vec<ProjectProfile>, String> {
    storage.list_project_profiles()
}

#[tauri::command]
fn delete_project_profile(storage: State<'_, AppStorage>, id: String) -> Result<bool, String> {
    let deleted = storage.delete_project_profile(&id)?;
    publish_project_runtime(&storage.project_runtime_configuration()?)?;
    Ok(deleted)
}

#[tauri::command]
fn activate_project_profile(
    storage: State<'_, AppStorage>,
    id: Option<String>,
) -> Result<ProjectRuntimeConfiguration, String> {
    let config = storage.activate_project_profile(id.as_deref())?;
    publish_project_runtime(&config)?;
    Ok(config)
}

#[tauri::command]
fn project_runtime_configuration(
    storage: State<'_, AppStorage>,
) -> Result<ProjectRuntimeConfiguration, String> {
    storage.project_runtime_configuration()
}

#[tauri::command]
fn set_secondary_layer_enabled(
    storage: State<'_, AppStorage>,
    enabled: bool,
) -> Result<ProjectRuntimeConfiguration, String> {
    let config = storage.set_secondary_layer_enabled(enabled)?;
    publish_project_runtime(&config)?;
    Ok(config)
}

#[tauri::command]
fn context_model_availability() -> Result<context_model::Availability, String> {
    context_model::availability()
}

#[tauri::command]
fn set_context_model_ranking_enabled(
    storage: State<'_, AppStorage>,
    enabled: bool,
) -> Result<ProjectRuntimeConfiguration, String> {
    if enabled && !context_model::availability()?.available {
        return Err("本机模型文件缺失或校验失败；上下文模型排序不可用。".into());
    }
    let config = storage.set_context_model_ranking_enabled(enabled)?;
    publish_project_runtime(&config)?;
    Ok(config)
}

#[tauri::command]
fn set_prediction_enabled(
    storage: State<'_, AppStorage>,
    enabled: bool,
) -> Result<ProjectRuntimeConfiguration, String> {
    let config = storage.set_prediction_enabled(enabled)?;
    publish_project_runtime(&config)?;
    Ok(config)
}

#[tauri::command]
fn set_candidate_theme(
    storage: State<'_, AppStorage>,
    candidate_theme: String,
) -> Result<ProjectRuntimeConfiguration, String> {
    let available = skin_packages::available_candidate_skin_ids()?;
    let config = storage
        .set_candidate_theme_from_catalog(&candidate_theme, available.contains(&candidate_theme))?;
    publish_project_runtime(&config)?;
    Ok(config)
}

#[tauri::command]
fn list_candidate_skins(
    storage: State<'_, AppStorage>,
) -> Result<CandidateSkinCatalogSnapshot, String> {
    let runtime = storage.project_runtime_configuration()?;
    skin_packages::catalog_snapshot(Some(&runtime.candidate_theme))
}

#[tauri::command]
fn install_candidate_skin(
    storage: State<'_, AppStorage>,
    source_path: String,
) -> Result<CandidateSkinCatalogSnapshot, String> {
    let _ = skin_packages::install(std::path::Path::new(&source_path))?;
    let runtime = storage.project_runtime_configuration()?;
    skin_packages::catalog_snapshot(Some(&runtime.candidate_theme))
}

#[tauri::command]
fn uninstall_candidate_skin(
    storage: State<'_, AppStorage>,
    id: String,
) -> Result<CandidateSkinCatalogSnapshot, String> {
    let _ = skin_packages::uninstall(&id)?;
    let current = storage.project_runtime_configuration()?;
    let active = if current.candidate_theme == id {
        let fallback = storage.set_candidate_theme("system")?;
        publish_project_runtime(&fallback)?;
        fallback
    } else {
        current
    };
    skin_packages::catalog_snapshot(Some(&active.candidate_theme))
}

#[tauri::command]
fn current_snapshot_status() -> Result<SnapshotMeta, String> {
    shared_snapshot_store()?.load_current()
}

fn primary_lexicon_snapshot_entries(
    storage: &AppStorage,
) -> Result<Vec<LexiconSnapshotEntry>, String> {
    let personal_entries = storage
        .list_lexicon_entries()?
        .into_iter()
        .map(|entry| {
            Ok(LexiconSnapshotEntry {
                id: entry.id,
                text: entry.text,
                pinyin: entry.pinyin,
                aliases: entry.aliases,
                professional_packs: entry.professional_packs,
                project_domains: entry.project_domains,
                sources: entry
                    .sources
                    .into_iter()
                    .map(|source| LexiconSnapshotSource {
                        source_kind: source.source_kind,
                        source: source.source,
                        license: source.license,
                        confidence_millis: source.confidence_millis,
                    })
                    .collect(),
                base_weight: entry.base_weight,
                fixed: entry.fixed,
                disabled: entry.disabled,
                demoted: entry.demoted,
                candidate_layer: "first".into(),
                updated_at_ms: entry.updated_at_ms,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(merge_primary_snapshot_entries(
        personal_entries,
        storage.approved_knowledge_snapshot_entries()?,
    ))
}

fn merge_primary_snapshot_entries(
    mut personal_entries: Vec<LexiconSnapshotEntry>,
    mut knowledge_entries: Vec<LexiconSnapshotEntry>,
) -> Vec<LexiconSnapshotEntry> {
    // Personal entries are always assembled first, so a knowledge term cannot
    // displace the owner's first-layer entry. Sorting each class by ID makes
    // the winner deterministic even if corrupt legacy data contains duplicates.
    personal_entries.sort_by(|left, right| left.id.cmp(&right.id));
    knowledge_entries.sort_by(|left, right| left.id.cmp(&right.id));

    let mut merged = Vec::<LexiconSnapshotEntry>::new();
    let mut indexes = std::collections::BTreeMap::<(String, String), usize>::new();
    for entry in personal_entries.into_iter().chain(knowledge_entries) {
        let key = (entry.text.clone(), entry.pinyin.clone());
        if let Some(index) = indexes.get(&key).copied() {
            merged[index].sources.extend(entry.sources);
            merged[index].sources.sort();
            merged[index].sources.dedup();
            continue;
        }
        indexes.insert(key, merged.len());
        merged.push(entry);
    }
    merged
}

fn lexicon_snapshot_entries(storage: &AppStorage) -> Result<Vec<LexiconSnapshotEntry>, String> {
    let mut entries = primary_lexicon_snapshot_entries(storage)?;
    let mut keys = entries
        .iter()
        .map(|entry| (entry.text.clone(), entry.pinyin.clone()))
        .collect::<std::collections::HashSet<_>>();
    // Knowledge terms the owner reviewed into the second row. These are added before
    // the prediction toggle is consulted: `prediction_enabled` governs the association
    // predictor, not an explicit owner review decision, so switching prediction off
    // must not silently discard words the owner approved.
    for entry in storage.approved_knowledge_secondary_entries()? {
        if !keys.insert((entry.text.clone(), entry.pinyin.clone())) {
            continue;
        }
        entries.push(entry);
    }
    if !storage.project_runtime_configuration()?.prediction_enabled {
        return Ok(entries);
    }
    for entry in storage.list_prediction_entries(current_time_ms()?)? {
        if !keys.insert((entry.text.clone(), entry.pinyin.clone())) {
            continue;
        }
        entries.push(LexiconSnapshotEntry {
            id: format!("prediction-{}", entry.id),
            text: entry.text,
            pinyin: entry.pinyin,
            aliases: Vec::new(),
            professional_packs: Vec::new(),
            project_domains: if entry.domain.is_empty() {
                Vec::new()
            } else {
                vec![entry.domain]
            },
            sources: entry
                .sources
                .into_iter()
                .map(|source| LexiconSnapshotSource {
                    source_kind: source.source_kind,
                    source: source.source,
                    license: source.license,
                    confidence_millis: source.confidence_millis,
                })
                .collect(),
            base_weight: entry.base_weight,
            fixed: false,
            disabled: false,
            demoted: false,
            candidate_layer: "secondary".into(),
            updated_at_ms: entry.updated_at_ms,
        });
    }
    Ok(entries)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct HotwordQualityReport {
    status: String,
    owner_replay_status: String,
    owner_sample_count: usize,
    fixed_hotword_count: usize,
    first_layer_occupation_count: usize,
    expired_count: usize,
    query_latency_milliseconds: f64,
    network_fallback: String,
}

#[tauri::command]
fn compile_secondary_prediction_snapshot(
    storage: State<'_, AppStorage>,
    version: u64,
) -> Result<SnapshotMeta, String> {
    let primary = primary_lexicon_snapshot_entries(&storage)?;
    let combined = lexicon_snapshot_entries(&storage)?;
    ensure_secondary_preserves_first_layer(&primary, &combined)?;
    // The secondary path publishes to the same durable `current` pointer the
    // core consumes, so it passes the exact same replay gate as the primary
    // path — prediction entries must not reach the core snapshot ungated.
    let report = run_replay_gate_for_entries(&combined)?;
    replay_gate::require_publication_pass(&report)?;
    replay_gate::mark_golden_used(
        &quality_directory()?,
        &report,
        version,
        current_time_ms()?,
        false,
    )?;
    replay_gate::publish_lexicon_after_gate(&shared_snapshot_store()?, version, combined, &report)
}

fn ensure_secondary_preserves_first_layer(
    primary: &[LexiconSnapshotEntry],
    combined: &[LexiconSnapshotEntry],
) -> Result<(), String> {
    if combined
        .iter()
        .filter(|entry| entry.candidate_layer == "first")
        .collect::<Vec<_>>()
        != primary.iter().collect::<Vec<_>>()
    {
        return Err("第二层发布会改变第一层投影，已阻止。".into());
    }
    Ok(())
}

#[tauri::command]
fn run_hotword_quality_gate(
    storage: State<'_, AppStorage>,
) -> Result<HotwordQualityReport, String> {
    let started = std::time::Instant::now();
    let entries = lexicon_snapshot_entries(&storage)?;
    let replay = run_replay_gate_for_entries(&entries)?;
    let items = storage.list_hotword_items()?;
    let now = current_time_ms()?;
    let fixed_hotword_count = entries
        .iter()
        .filter(|entry| entry.candidate_layer == "secondary")
        .count();
    Ok(HotwordQualityReport {
        status: if replay.status == "passed" && fixed_hotword_count > 0 {
            "passed".into()
        } else {
            "insufficient_data".into()
        },
        owner_replay_status: replay.status,
        owner_sample_count: replay.sample_count,
        fixed_hotword_count,
        first_layer_occupation_count: 0,
        expired_count: items
            .iter()
            .filter(|item| {
                item.expires_at_ms.is_some_and(|value| value <= now) || item.state == "expired"
            })
            .count(),
        query_latency_milliseconds: started.elapsed().as_secs_f64() * 1000.0,
        network_fallback: "manager last-known-good; core remains local".into(),
    })
}

fn replay_harness_path() -> Result<std::path::PathBuf, String> {
    if let Some(path) = std::env::var_os("AGENT_IME_REPLAY_HARNESS") {
        let path = std::path::PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }
        return Err("AGENT_IME_REPLAY_HARNESS 指向的文件不存在。".into());
    }
    let home = std::env::var_os("HOME").ok_or_else(|| "无法确定用户目录。".to_string())?;
    let candidates = [
        std::path::PathBuf::from(&home).join(
            "Library/Input Methods/Agent Input Method.inputmethod/Contents/MacOS/AgentIMEPersonalReplayHarness",
        ),
        std::path::PathBuf::from(
            "/Library/Input Methods/Agent Input Method.inputmethod/Contents/MacOS/AgentIMEPersonalReplayHarness",
        ),
        std::env::current_dir()
            .unwrap_or_default()
            .join("build/native/debug/Agent Input Method.inputmethod/Contents/MacOS/AgentIMEPersonalReplayHarness"),
        std::env::current_dir()
            .unwrap_or_default()
            .join("build/native/release/Agent Input Method.inputmethod/Contents/MacOS/AgentIMEPersonalReplayHarness"),
    ];
    candidates
        .into_iter()
        .find(|path| path.is_file())
        .ok_or_else(|| {
            "未找到随输入法安装的 AgentIMEPersonalReplayHarness；快照发布已阻止。".into()
        })
}

fn user_rime_directory() -> Result<std::path::PathBuf, String> {
    let home = std::env::var_os("HOME").ok_or_else(|| "无法确定用户目录。".to_string())?;
    Ok(std::path::PathBuf::from(home).join("Library/Application Support/AgentInputMethod/Rime"))
}

fn run_replay_gate_for_entries(
    entries: &[LexiconSnapshotEntry],
) -> Result<ReplayGateReport, String> {
    let helper = replay_harness_path()?;
    let provenance = replay_gate::artifact_provenance(&helper)?;
    replay_gate::run_replay_gate(ReplayRunInput {
        quality_root: &quality_directory()?,
        helper: &helper,
        rime_resources: &provenance.rime_resources,
        user_rime: &user_rime_directory()?,
        artifact_checksum: &provenance.artifact_checksum,
        product_commit: &provenance.product_commit,
        product_inputs_sha256: &provenance.product_inputs_sha256,
        lexicon_entries: &entries,
        now_ms: current_time_ms()?,
    })
}

#[tauri::command]
fn freeze_replay_golden_set() -> Result<FrozenGoldenMeta, String> {
    let quality = quality_directory()?;
    let events = replay_gate::read_quality_events(&quality)?;
    replay_gate::freeze_golden_set(&quality, &events, current_time_ms()?)
}

#[tauri::command]
fn refreeze_empty_replay_golden_set(
    owner_approved_reason: String,
) -> Result<FrozenGoldenMeta, String> {
    let quality = quality_directory()?;
    let events = replay_gate::read_quality_events(&quality)?;
    replay_gate::refreeze_empty_golden_set_once(
        &quality,
        &events,
        current_time_ms()?,
        &owner_approved_reason,
    )
}

#[tauri::command]
fn run_personal_replay_gate(storage: State<'_, AppStorage>) -> Result<ReplayGateReport, String> {
    run_replay_gate_for_entries(&lexicon_snapshot_entries(&storage)?)
}

#[tauri::command]
fn compile_lexicon_snapshot(
    storage: State<'_, AppStorage>,
    version: u64,
) -> Result<SnapshotMeta, String> {
    compile_lexicon_snapshot_inner(&storage, version)
}

fn compile_lexicon_snapshot_inner(
    storage: &AppStorage,
    version: u64,
) -> Result<SnapshotMeta, String> {
    let entries = lexicon_snapshot_entries(&storage)?;
    let report = run_replay_gate_for_entries(&entries)?;
    replay_gate::require_publication_pass(&report)?;
    replay_gate::mark_golden_used(
        &quality_directory()?,
        &report,
        version,
        current_time_ms()?,
        false,
    )?;
    replay_gate::publish_lexicon_after_gate(&shared_snapshot_store()?, version, entries, &report)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct OverrideSnapshotPublication {
    snapshot: SnapshotMeta,
    replay_report: ReplayGateReport,
    override_audit_entry_hash: String,
}

#[tauri::command]
fn compile_lexicon_snapshot_with_override(
    storage: State<'_, AppStorage>,
    version: u64,
    reason: String,
) -> Result<OverrideSnapshotPublication, String> {
    let entries = lexicon_snapshot_entries(&storage)?;
    let report = run_replay_gate_for_entries(&entries)?;
    if report.status == "passed" {
        return Err("Replay Gate 已通过，无需人工 override。".into());
    }
    let audit_hash = replay_gate::append_override_audit(
        &quality_directory()?,
        &report,
        version,
        &reason,
        current_time_ms()?,
    )?;
    replay_gate::mark_golden_used(
        &quality_directory()?,
        &report,
        version,
        current_time_ms()?,
        true,
    )?;
    let snapshot = shared_snapshot_store()?.publish_lexicon(version, entries)?;
    Ok(OverrideSnapshotPublication {
        snapshot,
        replay_report: report,
        override_audit_entry_hash: audit_hash,
    })
}

#[tauri::command]
fn rollback_lexicon_snapshot(version: u64) -> Result<SnapshotMeta, String> {
    shared_snapshot_store()?.rollback_to(version)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            ime_preferences::ime_preferences,
            ime_preferences::set_ime_typo_correction,
            ime_preferences::set_ime_fuzzy_mask,
            load_app_state,
            save_app_state,
            create_lexicon_entry,
            update_lexicon_entry,
            list_lexicon_entries,
            delete_lexicon_entry,
            delete_lexicon_entries,
            preview_lexicon_import,
            import_lexicon,
            undo_lexicon_import,
            export_lexicon,
            clear_learned_lexicon_entries,
            learning_preferences,
            save_learning_preferences,
            list_learning_cleanup_items,
            preview_learning_cleanup,
            apply_learning_cleanup,
            undo_learning_cleanup,
            export_maintenance_backup,
            preview_maintenance_backup,
            restore_maintenance_backup,
            run_integrity_report,
            run_scale_probe,
            list_personal_blacklist,
            undo_personal_blacklist,
            save_knowledge_source,
            list_knowledge_sources,
            ingest_knowledge_source,
            list_knowledge_terms,
            local_pinyin_readings,
            stage_generated_hotword_candidates,
            review_knowledge_term,
            delete_knowledge_source,
            set_knowledge_source_retrieval_policy,
            search_knowledge,
            save_dynamic_source,
            list_dynamic_sources,
            set_dynamic_source_enabled,
            clear_dynamic_source,
            list_hotword_items,
            refresh_hotword_exposure,
            ingest_local_hotword_source,
            refresh_dynamic_source,
            save_project_profile,
            list_project_profiles,
            delete_project_profile,
            activate_project_profile,
            project_runtime_configuration,
            set_secondary_layer_enabled,
            set_prediction_enabled,
            set_context_model_ranking_enabled,
            context_model_availability,
            set_candidate_theme,
            list_candidate_skins,
            install_candidate_skin,
            uninstall_candidate_skin,
            set_quality_recording,
            quality_recording_status,
            quality_recording_consent_status,
            grant_quality_recording_consent,
            confirm_test_recording_consent,
            test_upload_enabled,
            revoke_quality_recording_consent,
            list_quality_records,
            personal_profile_introspection,
            apply_personal_profile_auto_update,
            declare_owner_disambiguation,
            list_personal_profile_auto_updates,
            revoke_personal_profile_auto_update,
            export_quality_records,
            delete_quality_records,
            delete_quality_record,
            delete_quality_records_by_app,
            delete_quality_records_by_time_range,
            freeze_replay_golden_set,
            refreeze_empty_replay_golden_set,
            run_personal_replay_gate,
            update_panel_shortcut,
            generate_with_provider,
            prepare_generation_payload,
            latest_selection_capture,
            external_text_access_status,
            perform_external_text_action,
            set_mcp_server_enabled,
            mcp_server_status,
            current_snapshot_status,
            compile_lexicon_snapshot,
            compile_lexicon_snapshot_with_override,
            rollback_lexicon_snapshot,
            compile_secondary_prediction_snapshot,
            run_hotword_quality_gate
        ])
        .setup(|app| {
            #[cfg(feature = "test-upload")]
            test_upload::ensure_scheduled();
            let app_handle = app.handle().clone();
            // Test builds start recording on a machine with no choice yet; the install message (or,
            // without an install, a plain notice) says so.
            #[cfg(all(target_os = "macos", feature = "test-upload"))]
            let recording_started = start_test_recording_if_unset();
            #[cfg(not(all(target_os = "macos", feature = "test-upload")))]
            let recording_started = false;
            #[cfg(target_os = "macos")]
            {
                use tauri_plugin_dialog::DialogExt;
                let installed = friend_install::ensure_input_method_installed();
                let mut message = String::new();
                if installed {
                    message.push_str("知字输入法已经装好。\n\n点屏幕右上角菜单栏的输入法图标，选「知字拼音」就能用了。\n如果列表里没有，到「系统设置 → 键盘 → 输入法」里点「+」，在「简体中文」下添加「知字拼音」；还是没有就注销再登录一次。");
                }
                #[cfg(feature = "test-upload")]
                if recording_started {
                    if !message.is_empty() {
                        message.push_str("\n\n———\n\n");
                    }
                    message.push_str(TEST_RECORDING_NOTICE);
                }
                if !message.is_empty() {
                    app_handle.dialog().message(message).title("知字").show(|_| {});
                }
            }

            app.manage(native_integration::NativeIntegrationState::default());
            app.manage(mcp::McpServerState::default());

            install_tray(&app_handle)?;

            let database_path = app_handle
                .path()
                .app_data_dir()?
                .join("agent-input-layer.sqlite3");
            let storage = AppStorage::open(database_path).map_err(std::io::Error::other)?;
            publish_project_runtime(&storage.project_runtime_configuration()?)
                .map_err(std::io::Error::other)?;
            if let Ok(report) = apply_profile_updates_from_quality(&storage, current_time_ms()?) {
                let changed =
                    report.applied_count + report.decayed_count + report.blacklisted_count;
                if changed > 0 {
                    let now = current_time_ms()? as u64;
                    let version = shared_snapshot_store()
                        .ok()
                        .and_then(|store| store.load_current().ok())
                        .map(|current| now.max(current.version.saturating_add(1)))
                        .unwrap_or(now);
                    let _ = compile_lexicon_snapshot_inner(&storage, version);
                }
            }
            app.manage(storage);

            app.handle().plugin(
                tauri_plugin_global_shortcut::Builder::new()
                    .with_shortcut("alt+space")?
                    .with_handler(|app, shortcut, event| {
                        if event.state == ShortcutState::Pressed
                            && shortcut.matches(Modifiers::ALT, Code::Space)
                        {
                            show_compact_panel(app);
                        }
                    })
                    .build(),
            )?;

            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() == MAIN_WINDOW_LABEL || window.label() == COMPACT_WINDOW_LABEL {
                if let WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            if matches!(event, RunEvent::Reopen { .. }) {
                show_main_window(app);
            }
        });
}

#[cfg(test)]
mod lexicon_snapshot_integration_tests {
    use super::*;
    use crate::knowledge::KnowledgeSourceInput;
    use crate::storage::{LexiconEntryInput, LexiconSource};
    use std::{fs, path::PathBuf};

    #[test]
    fn sqlite_authority_compiles_into_a_schema_v2_snapshot() {
        let root = PathBuf::from("/private/tmp")
            .join(format!("agent-ime-v04-integration-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let storage = AppStorage::open(root.join("manager.sqlite3")).unwrap();
        storage
            .create_lexicon_entry(LexiconEntryInput {
                id: "entry-1".into(),
                text: "个人词".into(),
                pinyin: "ge ren ci".into(),
                aliases: vec!["私有词".into()],
                professional_packs: vec!["input-method".into()],
                project_domains: vec!["v04".into()],
                sources: vec![LexiconSource {
                    source_kind: "manual".into(),
                    source: "owner-entry".into(),
                    license: "private-user-data".into(),
                    confidence_millis: 1000,
                }],
                base_weight: 100,
                fixed: true,
                disabled: false,
                demoted: false,
                updated_at_ms: 1_752_624_000_000,
            })
            .unwrap();

        let store = SnapshotStore::new(root.join("Shared/Snapshots"));
        let published = store
            .publish_lexicon(12, lexicon_snapshot_entries(&storage).unwrap())
            .unwrap();
        assert_eq!(published.schema_version, 2);
        assert_eq!(published.entry_count, 1);
        let body =
            fs::read_to_string(root.join("Shared/Snapshots").join(&published.file_name)).unwrap();
        assert!(body.starts_with("AIMSNAP2\nschema_version=2\nversion=12\nentry_count=1\n"));
        assert!(body.contains("\"license\":\"private-user-data\""));
        assert_eq!(store.load_current().unwrap().checksum, published.checksum);

        drop(storage);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn approved_same_term_from_two_knowledge_sources_is_assembled_once_with_provenance() {
        let root = PathBuf::from("/private/tmp")
            .join(format!("agent-ime-knowledge-dedup-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let storage = AppStorage::open(root.join("manager.sqlite3")).unwrap();

        for (source_id, source_name) in [("source-a", "Source A"), ("source-b", "Source B")] {
            let source_root = root.join(source_id);
            fs::create_dir_all(&source_root).unwrap();
            fs::write(
                source_root.join("terms.md"),
                "SharedSnapshot | shared snapshot\n",
            )
            .unwrap();
            storage
                .save_knowledge_source(KnowledgeSourceInput {
                    id: source_id.into(),
                    name: source_name.into(),
                    source_type: "local_path".into(),
                    authorized_scope: source_root.to_string_lossy().to_string(),
                    authorization: "test explicitly authorizes this fixture".into(),
                    project_id: source_id.into(),
                    sensitive_enabled: true,
                    updated_at_ms: 1,
                })
                .unwrap();
            storage.ingest_knowledge_source(source_id, 2).unwrap();
            let term = storage
                .list_knowledge_terms()
                .unwrap()
                .into_iter()
                .find(|term| term.source_id == source_id && term.text == "SharedSnapshot")
                .unwrap();
            storage
                .review_knowledge_term(&term.id, "approved", "shared snapshot", false, 3)
                .unwrap();
        }

        let entries = lexicon_snapshot_entries(&storage).unwrap();
        let shared = entries
            .iter()
            .filter(|entry| entry.text == "SharedSnapshot" && entry.pinyin == "shared snapshot")
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(shared.len(), 1, "assembly must deduplicate text+pinyin");
        assert_eq!(
            shared[0].candidate_layer, "first",
            "approved short knowledge entities must enter the primary candidate snapshot path"
        );
        assert!(
            shared[0].id.starts_with("knowledge-"),
            "the candidate snapshot row must retain knowledge provenance"
        );
        assert_eq!(shared[0].sources.len(), 2, "both provenances must remain");
        assert!(shared[0]
            .sources
            .iter()
            .any(|source| source.source.contains("Source A")));
        assert!(shared[0]
            .sources
            .iter()
            .any(|source| source.source.contains("Source B")));

        let store = SnapshotStore::new(root.join("Shared/Snapshots"));
        let published = store.publish_lexicon(1, entries).unwrap();
        assert_eq!(published.entry_count, 1);

        let mut duplicate = shared[0].clone();
        duplicate.id = "bypass-assembly-duplicate".into();
        let error = store
            .publish_lexicon(2, vec![shared[0].clone(), duplicate])
            .unwrap_err();
        println!("K1 reverse raw error: {error}");
        assert!(error.contains("重复 text+pinyin"));

        drop(storage);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn personal_entry_wins_over_same_approved_knowledge_term() {
        let root = PathBuf::from("/private/tmp").join(format!(
            "agent-ime-personal-priority-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        let storage = AppStorage::open(root.join("manager.sqlite3")).unwrap();
        storage
            .create_lexicon_entry(LexiconEntryInput {
                id: "personal-shared".into(),
                text: "SharedSnapshot".into(),
                pinyin: "shared snapshot".into(),
                aliases: vec!["owner alias".into()],
                professional_packs: vec!["owner pack".into()],
                project_domains: vec!["owner domain".into()],
                sources: vec![LexiconSource {
                    source_kind: "manual".into(),
                    source: "owner-entry".into(),
                    license: "private-user-data".into(),
                    confidence_millis: 1000,
                }],
                base_weight: 177,
                fixed: true,
                disabled: false,
                demoted: false,
                updated_at_ms: 1,
            })
            .unwrap();
        let source_root = root.join("knowledge");
        fs::create_dir_all(&source_root).unwrap();
        fs::write(
            source_root.join("terms.md"),
            "SharedSnapshot | shared snapshot\n",
        )
        .unwrap();
        storage
            .save_knowledge_source(KnowledgeSourceInput {
                id: "knowledge".into(),
                name: "Knowledge Source".into(),
                source_type: "local_path".into(),
                authorized_scope: source_root.to_string_lossy().to_string(),
                authorization: "test explicitly authorizes this fixture".into(),
                project_id: "knowledge".into(),
                sensitive_enabled: true,
                updated_at_ms: 2,
            })
            .unwrap();
        storage.ingest_knowledge_source("knowledge", 3).unwrap();
        let term = storage
            .list_knowledge_terms()
            .unwrap()
            .into_iter()
            .find(|term| term.source_id == "knowledge" && term.text == "SharedSnapshot")
            .unwrap();
        storage
            .review_knowledge_term(&term.id, "approved", "shared snapshot", false, 4)
            .unwrap();

        let entries = primary_lexicon_snapshot_entries(&storage).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, "personal-shared");
        assert_eq!(entries[0].base_weight, 177);
        assert_eq!(entries[0].aliases, vec!["owner alias"]);
        assert_eq!(entries[0].sources.len(), 2);

        drop(storage);
        let _ = fs::remove_dir_all(root);
    }
}

#[cfg(test)]
mod quality_recording_mode_tests {
    use super::*;

    fn fixture_directory(name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "agent-quality-{name}-{}-{}",
            std::process::id(),
            QUALITY_TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        #[cfg(unix)]
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        path
    }

    fn event(id: &str, app_hash: &str, recorded_at_ms: i64) -> RecordedQualityEvent {
        RecordedQualityEvent {
            id: id.into(),
            recorded_at_ms,
            pinyin: "ni".into(),
            candidates: vec!["你".into()],
            candidate_comments: vec![String::new()],
            selected_index: 0,
            selected_text: "你".into(),
            selected_candidate_comment: None,
            app_hash: app_hash.into(),
            preceding_text: "前文".into(),
            fuzzy_mask: 0,
            typo_correction_enabled: false,
            active_project: None,
            active_packs: Vec::new(),
            secondary_enabled: true,
            max_page: 0,
            outcome: replay_gate::CompositionOutcome::CommittedCandidate,
            duration_ms: 10,
            backspaces: 0,
            redo_after_commit: false,
            candidate_count: 1,
            context_model: None,
        }
    }

    fn write_events(directory: &std::path::Path, events: &[RecordedQualityEvent]) {
        let mut data = Vec::new();
        for event in events {
            serde_json::to_writer(&mut data, event).unwrap();
            data.push(b'\n');
        }
        atomic_write_quality_file(directory, "events.jsonl", &data, "测试质量记录").unwrap();
    }

    #[test]
    fn default_timed_and_owner_continuous_modes_remain_distinct() {
        let timed = quality_recording_configuration("twoHour", 1_000).unwrap();
        assert!(timed.enabled);
        assert!(!timed.continuous);
        assert_eq!(timed.expires_at_ms, 7_201_000);

        let continuous = quality_recording_configuration("continuous", 1_000).unwrap();
        assert!(continuous.enabled);
        assert!(continuous.continuous);
        assert_eq!(continuous.expires_at_ms, 0);

        let off = quality_recording_configuration("off", 1_000).unwrap();
        assert!(!off.enabled);
        assert!(!off.continuous);
        assert_eq!(off.expires_at_ms, 0);
        assert!(quality_recording_configuration("forever-by-default", 1_000).is_err());
    }

    #[test]
    fn consent_is_distinct_from_the_recording_switch_and_revoke_fails_closed() {
        let directory = fixture_directory("consent");
        assert!(set_quality_recording_at(&directory, "continuous", 1_000).is_err());
        assert!(
            !quality_recording_consent_status_at(&directory)
                .unwrap()
                .granted
        );

        let consent = grant_quality_recording_consent_at(&directory, 1_001).unwrap();
        assert!(consent.granted);
        assert!(!quality_recording_status_at(&directory).unwrap().enabled);
        let config = set_quality_recording_at(&directory, "continuous", 1_002).unwrap();
        assert!(config.enabled && config.continuous);

        write_events(&directory, &[event("one", &"a".repeat(64), 1_003)]);
        let revoked = revoke_quality_recording_consent_at(&directory, true, 1_004).unwrap();
        assert!(!revoked.consent.granted);
        assert_eq!(revoked.deleted_count, 1);
        assert!(!quality_recording_status_at(&directory).unwrap().enabled);
        assert!(read_quality_records_at(&directory).unwrap().is_empty());
        assert!(!directory.join("events.jsonl").exists());
        println!("RED-TEAM revoke deleted=1 events_after_restart=0 enabled=false consent=false");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn record_app_and_time_deletions_rewrite_the_real_jsonl() {
        let directory = fixture_directory("deletions");
        let app_a = "a".repeat(64);
        let app_b = "b".repeat(64);
        let app_c = "c".repeat(64);
        write_events(
            &directory,
            &[
                event("one", &app_a, 10),
                event("two", &app_a, 20),
                event("three", &app_b, 30),
                {
                    let mut retained = event("four", &app_c, 40);
                    retained.candidates.push("泥".into());
                    retained.candidate_comments.push(String::new());
                    retained.context_model = Some(replay_gate::ContextModelObservation {
                        enabled: true,
                        outcome: replay_gate::ContextModelOutcome::Reordered,
                        base_order: Some(vec![1, 0]),
                        group_size: Some(2),
                    });
                    retained
                },
            ],
        );

        {
            let _lock = acquire_quality_lock(&directory).unwrap();
            assert_eq!(
                rewrite_quality_records_at(&directory, |item| item.id == "one").unwrap(),
                1
            );
            assert_eq!(
                rewrite_quality_records_at(&directory, |item| item.app_hash == app_a).unwrap(),
                1
            );
            assert_eq!(
                rewrite_quality_records_at(&directory, |item| {
                    item.recorded_at_ms >= 25 && item.recorded_at_ms <= 35
                })
                .unwrap(),
                1
            );
        }
        let after_restart = read_quality_records_at(&directory).unwrap();
        assert_eq!(after_restart.len(), 1);
        assert_eq!(after_restart[0].id, "four");
        assert_eq!(after_restart[0].context_model.as_ref().unwrap().base_order, Some(vec![1, 0]));
        let on_disk = fs::read_to_string(directory.join("events.jsonl")).unwrap();
        assert!(!on_disk.contains("\"one\""));
        assert!(!on_disk.contains("\"two\""));
        assert!(!on_disk.contains("\"three\""));
        assert!(on_disk.contains("\"four\""));
        println!("RED-TEAM delete_by_id=1 delete_by_app=1 delete_by_time=1 restart_ids=[four]");
        fs::remove_dir_all(directory).unwrap();
    }
}

#[cfg(test)]
mod provider_tests {
    use super::{
        generate_with_provider_timeout, provider_client, provider_http_status_error,
        validate_remote_provider_base_url, AnthropicMessagesAdapter, OpenAiCompatibleAdapter,
        ProviderAdapter, ProviderConfig, ProviderGenerateRequest,
    };
    use serde_json::Value;
    use std::{
        io::{Read, Write},
        net::{TcpListener, TcpStream},
        sync::Mutex,
        thread::{self, JoinHandle},
        time::Duration,
    };

    static PROVIDER_TEST_LOCK: Mutex<()> = Mutex::new(());

    fn provider_request(base_url: String, _api_key: &str) -> ProviderGenerateRequest {
        ProviderGenerateRequest {
            payload: "synthetic-preview-payload".into(),
            provider: ProviderConfig {
                kind: "openai-compatible".into(),
                base_url,
                model: "loopback-test-model".into(),
            },
        }
    }

    fn http_response(status: &str, content_type: &str, body: &str) -> Vec<u8> {
        format!(
            "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .into_bytes()
    }

    fn read_http_request(stream: &mut TcpStream) -> String {
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("set loopback read timeout");

        let mut request = Vec::new();
        let mut buffer = [0_u8; 2048];
        let mut expected_length = None;

        loop {
            let count = stream.read(&mut buffer).expect("read loopback request");
            assert!(
                count > 0,
                "loopback connection closed before request completed"
            );
            request.extend_from_slice(&buffer[..count]);

            if expected_length.is_none() {
                if let Some(header_end) = request.windows(4).position(|part| part == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..header_end]);
                    let content_length = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length").then(|| {
                                value.trim().parse::<usize>().expect("valid content length")
                            })
                        })
                        .expect("content-length header");
                    expected_length = Some(header_end + 4 + content_length);
                }
            }

            if expected_length.is_some_and(|length| request.len() >= length) {
                break;
            }
        }

        String::from_utf8(request).expect("UTF-8 loopback request")
    }

    fn spawn_loopback_server(
        response: Vec<u8>,
        response_delay: Duration,
    ) -> (String, JoinHandle<String>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind loopback provider");
        let address = listener.local_addr().expect("loopback provider address");
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept loopback provider request");
            let request = read_http_request(&mut stream);
            thread::sleep(response_delay);
            let _ = stream.write_all(&response);
            request
        });

        (format!("http://{address}"), handle)
    }

    fn assert_error_is_redacted(error: &str, additional_sensitive: &[&str]) {
        let always_sensitive = [
            "loopback-test-secret",
            "synthetic-private-prompt-body",
            "synthetic-private-user-rule",
            "synthetic-private-local-draft",
            "synthetic-preview-payload",
        ];
        for sensitive in always_sensitive
            .into_iter()
            .chain(additional_sensitive.iter().copied())
        {
            assert!(
                !error.contains(sensitive),
                "provider error exposed sensitive data: {error}"
            );
        }
    }

    #[test]
    fn provider_request_requires_payload_and_rejects_empty_before_network() {
        let missing = serde_json::from_value::<ProviderGenerateRequest>(serde_json::json!({
            "provider": {
                "kind": "openai-compatible",
                "baseUrl": "http://127.0.0.1:1",
                "model": "test"
            }
        }));
        assert!(missing.is_err(), "missing payload must not deserialize");

        let mut request = provider_request("http://127.0.0.1:1".into(), "loopback-test-secret");
        request.payload.clear();
        let error = tauri::async_runtime::block_on(generate_with_provider_timeout(
            request,
            Duration::from_millis(20),
            "loopback-test-secret".into(),
        ))
        .unwrap_err();
        assert_eq!(
            error,
            "Provider payload 不能为空；请先调用 prepare_generation_payload 获取预览内容。"
        );
    }

    #[test]
    fn provider_loopback_sends_expected_endpoint_bearer_and_payload() {
        let _guard = PROVIDER_TEST_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let response = http_response(
            "200 OK",
            "application/json",
            r#"{"choices":[{"message":{"content":"  loopback generated result  "}}]}"#,
        );
        let (base_url, server) = spawn_loopback_server(response, Duration::ZERO);
        let request = provider_request(format!("{base_url}/v1/"), "loopback-test-secret");

        let output = tauri::async_runtime::block_on(generate_with_provider_timeout(
            request,
            Duration::from_secs(2),
            "loopback-test-secret".into(),
        ))
        .expect("loopback provider succeeds");
        assert_eq!(output, "loopback generated result");

        let captured = server.join().expect("loopback provider thread");
        let (headers, body) = captured
            .split_once("\r\n\r\n")
            .expect("HTTP request headers and body");
        assert!(headers.starts_with("POST /v1/chat/completions HTTP/1.1\r\n"));
        assert!(headers.lines().any(|line| {
            line.eq_ignore_ascii_case("authorization: Bearer loopback-test-secret")
        }));

        let payload: Value = serde_json::from_str(body).expect("Provider JSON payload");
        assert_eq!(payload["model"], "loopback-test-model");
        assert_eq!(payload["temperature"], 0.2);
        assert_eq!(payload["messages"].as_array().unwrap().len(), 1);
        assert_eq!(payload["messages"][0]["role"], "user");
        let user_content = payload["messages"][0]["content"]
            .as_str()
            .expect("user content string");
        assert_eq!(user_content.as_bytes(), b"synthetic-preview-payload");
    }

    #[test]
    fn provider_loopback_rejects_invalid_json_without_exposing_data() {
        let _guard = PROVIDER_TEST_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let echoed_body = "invalid-json-loopback-test-secret-synthetic-private-prompt-body";
        let response = http_response("200 OK", "application/json", echoed_body);
        let (base_url, server) = spawn_loopback_server(response, Duration::ZERO);

        let error = tauri::async_runtime::block_on(generate_with_provider_timeout(
            provider_request(base_url, "loopback-test-secret"),
            Duration::from_secs(2),
            "loopback-test-secret".into(),
        ))
        .expect_err("invalid Provider JSON must fail");
        server.join().expect("loopback provider thread");

        assert_eq!(error, "Provider 响应格式无效，已回退到本地模板。");
        assert_error_is_redacted(&error, &[echoed_body]);
    }

    #[test]
    fn provider_loopback_rejects_empty_content_without_exposing_data() {
        let _guard = PROVIDER_TEST_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let response = http_response(
            "200 OK",
            "application/json",
            r#"{"choices":[{"message":{"content":"   "}}]}"#,
        );
        let (base_url, server) = spawn_loopback_server(response, Duration::ZERO);

        let error = tauri::async_runtime::block_on(generate_with_provider_timeout(
            provider_request(base_url, "loopback-test-secret"),
            Duration::from_secs(2),
            "loopback-test-secret".into(),
        ))
        .expect_err("empty Provider content must fail");
        server.join().expect("loopback provider thread");

        assert_eq!(error, "Provider 没有返回可用内容。");
        assert_error_is_redacted(&error, &[]);
    }

    #[test]
    fn provider_loopback_rejects_non_success_without_echoing_body() {
        let _guard = PROVIDER_TEST_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let echoed_body = "failure token=loopback-test-secret input=synthetic-private-prompt-body";
        let response = http_response("429 Too Many Requests", "text/plain", echoed_body);
        let (base_url, server) = spawn_loopback_server(response, Duration::ZERO);

        let error = tauri::async_runtime::block_on(generate_with_provider_timeout(
            provider_request(base_url, "loopback-test-secret"),
            Duration::from_secs(2),
            "loopback-test-secret".into(),
        ))
        .expect_err("non-success Provider status must fail");
        server.join().expect("loopback provider thread");

        assert_eq!(
            error,
            "Provider 请求过于频繁（429），请稍后重试；已回退到本地模板。"
        );
        assert_error_is_redacted(&error, &[echoed_body]);
    }

    #[test]
    fn provider_loopback_transport_timeout_is_specific_and_redacted() {
        let _guard = PROVIDER_TEST_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let response = http_response(
            "200 OK",
            "application/json",
            r#"{"choices":[{"message":{"content":"too late"}}]}"#,
        );
        let (base_url, server) = spawn_loopback_server(response, Duration::from_millis(400));

        let error = tauri::async_runtime::block_on(generate_with_provider_timeout(
            provider_request(base_url, "loopback-test-secret"),
            Duration::from_millis(100),
            "loopback-test-secret".into(),
        ))
        .expect_err("stalled Provider must time out");
        server.join().expect("loopback provider thread");

        assert_eq!(error, "Provider 请求超时，已回退到本地模板。");
        assert_error_is_redacted(&error, &[]);
    }

    #[test]
    fn provider_http_error_never_includes_echoed_sensitive_body() {
        let secret = "test-api-key-never-log";
        let raw_input = "synthetic-private-prompt-body";
        let user_rule = "synthetic-private-user-rule";
        let echoed_body = format!("failure token={secret} input={raw_input} rule={user_rule}");
        let error = provider_http_status_error(reqwest::StatusCode::BAD_REQUEST);

        assert_eq!(
            error,
            "Provider 返回 HTTP 400 Bad Request，已回退到本地模板。"
        );
        for sensitive in [secret, raw_input, user_rule, echoed_body.as_str()] {
            assert!(!error.contains(sensitive));
        }
    }

    #[test]
    fn anthropic_messages_success_uses_required_headers_and_exact_preview() {
        let _guard = PROVIDER_TEST_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let response = http_response(
            "200 OK",
            "application/json",
            r#"{"content":[{"type":"text","text":" anthropic result "}]}"#,
        );
        let (base_url, server) = spawn_loopback_server(response, Duration::ZERO);
        let mut request = provider_request(base_url, "loopback-test-secret");
        request.provider.kind = "anthropic".into();
        let output = tauri::async_runtime::block_on(generate_with_provider_timeout(
            request,
            Duration::from_secs(2),
            "loopback-test-secret".into(),
        ))
        .unwrap();
        assert_eq!(output, "anthropic result");
        let captured = server.join().unwrap();
        let (headers, body) = captured.split_once("\r\n\r\n").unwrap();
        assert!(headers.starts_with("POST /v1/messages HTTP/1.1"));
        assert!(headers
            .lines()
            .any(|line| line.eq_ignore_ascii_case("x-api-key: loopback-test-secret")));
        assert!(headers
            .lines()
            .any(|line| line.eq_ignore_ascii_case("anthropic-version: 2023-06-01")));
        let payload: Value = serde_json::from_str(body).unwrap();
        assert_eq!(
            payload["messages"][0]["content"]
                .as_str()
                .unwrap()
                .as_bytes(),
            b"synthetic-preview-payload"
        );
    }

    #[test]
    fn local_ollama_success_has_no_authorization_and_exact_preview() {
        let _guard = PROVIDER_TEST_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let response = http_response(
            "200 OK",
            "application/json",
            r#"{"message":{"content":" ollama result "}}"#,
        );
        let (base_url, server) = spawn_loopback_server(response, Duration::ZERO);
        let mut request = provider_request(base_url, "");
        request.provider.kind = "ollama".into();
        let output = tauri::async_runtime::block_on(generate_with_provider_timeout(
            request,
            Duration::from_secs(2),
            String::new(),
        ))
        .unwrap();
        assert_eq!(output, "ollama result");
        let captured = server.join().unwrap();
        let (headers, body) = captured.split_once("\r\n\r\n").unwrap();
        assert!(headers.starts_with("POST /api/chat HTTP/1.1"));
        assert!(!headers.to_ascii_lowercase().contains("authorization:"));
        let payload: Value = serde_json::from_str(body).unwrap();
        assert_eq!(
            payload["messages"][0]["content"]
                .as_str()
                .unwrap()
                .as_bytes(),
            b"synthetic-preview-payload"
        );
    }

    #[test]
    fn provider_authentication_failure_is_readable_and_redacted() {
        let _guard = PROVIDER_TEST_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let response = http_response("401 Unauthorized", "text/plain", "secret echoed body");
        let (base_url, server) = spawn_loopback_server(response, Duration::ZERO);
        let error = tauri::async_runtime::block_on(generate_with_provider_timeout(
            provider_request(base_url, "loopback-test-secret"),
            Duration::from_secs(2),
            "loopback-test-secret".into(),
        ))
        .unwrap_err();
        server.join().unwrap();
        assert_eq!(
            error,
            "Provider 鉴权失败，请检查 API Key 与账号权限；已回退到本地模板。"
        );
        assert_error_is_redacted(&error, &["secret echoed body"]);
    }

    #[test]
    fn provider_connect_failure_is_reported_as_offline() {
        let _guard = PROVIDER_TEST_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let error = tauri::async_runtime::block_on(generate_with_provider_timeout(
            provider_request(format!("http://{address}"), "loopback-test-secret"),
            Duration::from_secs(2),
            "loopback-test-secret".into(),
        ))
        .unwrap_err();
        assert_eq!(
            error,
            "当前无法连接 Provider（无网络或服务未启动），已回退到本地模板。"
        );
        assert_error_is_redacted(&error, &[]);
    }

    #[test]
    fn ollama_rejects_non_loopback_before_network() {
        let mut request = provider_request("https://ollama.example".into(), "");
        request.provider.kind = "ollama".into();
        let error = tauri::async_runtime::block_on(generate_with_provider_timeout(
            request,
            Duration::from_millis(20),
            String::new(),
        ))
        .unwrap_err();
        assert_eq!(error, "Ollama 只允许连接本机 loopback 地址。");
    }

    #[test]
    fn hosted_adapters_reject_plain_http_off_loopback_with_guidance() {
        for base_url in [
            "http://example.com/v1",
            "http://example.com",
            "http://192.168.1.10:8080/v1",
            "ftp://example.com/v1",
        ] {
            let error = OpenAiCompatibleAdapter
                .endpoint(base_url)
                .expect_err("plain http off loopback must be rejected");
            assert_eq!(
                error, "非本机地址仅允许 HTTPS Base URL。",
                "openai {base_url}"
            );
            let error = AnthropicMessagesAdapter
                .endpoint(base_url)
                .expect_err("plain http off loopback must be rejected");
            assert_eq!(
                error, "非本机地址仅允许 HTTPS Base URL。",
                "anthropic {base_url}"
            );
        }
        assert_eq!(
            validate_remote_provider_base_url("not a url").unwrap_err(),
            "Provider Base URL 无效。"
        );
    }

    #[test]
    fn hosted_adapters_accept_https_anywhere_and_loopback_http() {
        for base_url in [
            "https://api.example.com/v1",
            "https://example.com",
            "http://localhost:11434/v1",
            "http://LOCALHOST:8080",
            "http://127.0.0.1:9000/v1",
            "http://127.254.1.2:9000",
            "http://[::1]:9000/v1",
        ] {
            OpenAiCompatibleAdapter
                .endpoint(base_url)
                .unwrap_or_else(|error| panic!("openai {base_url} must pass: {error}"));
            AnthropicMessagesAdapter
                .endpoint(base_url)
                .unwrap_or_else(|error| panic!("anthropic {base_url} must pass: {error}"));
        }
    }

    #[test]
    fn plain_http_off_loopback_is_rejected_before_network() {
        let error = tauri::async_runtime::block_on(generate_with_provider_timeout(
            provider_request("http://example.com/v1".into(), "loopback-test-secret"),
            Duration::from_millis(20),
            "loopback-test-secret".into(),
        ))
        .unwrap_err();
        assert_eq!(error, "非本机地址仅允许 HTTPS Base URL。");
        assert_error_is_redacted(&error, &[]);
    }

    #[test]
    fn loopback_http_client_does_not_follow_redirects_off_box() {
        let redirect =
            b"HTTP/1.1 302 Found\r\nLocation: http://example.invalid/steal\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                .to_vec();
        let (base_url, handle) = spawn_loopback_server(redirect, Duration::ZERO);
        let error = tauri::async_runtime::block_on(generate_with_provider_timeout(
            provider_request(base_url, "loopback-test-secret"),
            Duration::from_secs(2),
            "loopback-test-secret".into(),
        ))
        .unwrap_err();
        // Surfacing the 302 itself proves the client did not follow it; a
        // following client would have died on the example.invalid DNS lookup
        // with a redacted transport error instead.
        assert!(
            error.contains("HTTP 302"),
            "redirect must not be followed, got: {error}"
        );
        assert_error_is_redacted(&error, &[]);
        let _ = handle.join();
    }

    #[test]
    fn https_client_refuses_plain_http_targets_up_front() {
        let client = provider_client(Duration::from_secs(2), true).expect("https-only client");
        // Port 9 (discard) has no listener; if https_only were absent, the
        // failure would be is_connect (connection refused) rather than the
        // pre-dial scheme refusal asserted below.
        let error = tauri::async_runtime::block_on(async {
            client
                .get("http://127.0.0.1:9/")
                .send()
                .await
                .expect_err("https-only client must refuse http:// targets")
        });
        assert!(
            !error.is_connect() && !error.is_timeout(),
            "refusal must happen before any network attempt: {error}"
        );
    }
}

#[cfg(test)]
mod secondary_snapshot_gate_tests {
    use super::ensure_secondary_preserves_first_layer;
    use crate::snapshot::LexiconSnapshotEntry;

    fn entry(id: &str, layer: &str) -> LexiconSnapshotEntry {
        LexiconSnapshotEntry {
            id: id.into(),
            text: format!("词{id}"),
            pinyin: "ci".into(),
            aliases: Vec::new(),
            professional_packs: Vec::new(),
            project_domains: Vec::new(),
            sources: Vec::new(),
            base_weight: 100,
            fixed: false,
            disabled: false,
            demoted: false,
            candidate_layer: layer.into(),
            updated_at_ms: 1,
        }
    }

    #[test]
    fn preserved_first_layer_projection_passes() {
        let primary = vec![entry("a", "first"), entry("b", "first")];
        let combined = vec![
            entry("a", "first"),
            entry("b", "first"),
            entry("p", "secondary"),
        ];
        assert!(ensure_secondary_preserves_first_layer(&primary, &combined).is_ok());
    }

    #[test]
    fn changed_first_layer_projection_is_blocked() {
        let primary = vec![entry("a", "first")];
        // A prediction entry sneaking into the first layer must be rejected.
        let combined = vec![entry("a", "first"), entry("p", "first")];
        assert!(ensure_secondary_preserves_first_layer(&primary, &combined).is_err());
    }
}
