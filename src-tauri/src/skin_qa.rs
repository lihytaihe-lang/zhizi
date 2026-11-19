//! Debug-only native UI runner. It never starts the normal application's installer, recorder, or profile migration.
use std::path::PathBuf;
use tauri::Manager;

pub fn root() -> Result<Option<PathBuf>, String> {
    let Some(raw) = std::env::var_os("ZHIZI_SKIN_QA_ROOT") else { return Ok(None); };
    let root = std::fs::canonicalize(raw).map_err(|_| "QA profile must already exist")?;
    let within = |parent: PathBuf| std::fs::canonicalize(parent)
        .is_ok_and(|parent| root != parent && root.starts_with(parent));
    let local_qa = std::env::temp_dir().join("zhizi-skin-qa");
    let project_tmp = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../tmp");
    if !root.is_dir() || !(within(local_qa) || within(project_tmp)) {
        return Err("QA profile must be below the project's tmp or the local zhizi-skin-qa directory".into());
    }
    Ok(Some(root))
}

#[tauri::command]
fn skin_qa_report(report: serde_json::Value) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(&report).map_err(|_| "invalid report")?;
    if bytes.len() > 128 * 1024 { return Err("report too large".into()); }
    std::fs::write(root()?.ok_or("QA profile missing")?.join("report.json"), bytes).map_err(|error| error.to_string())
}

pub fn run(context: tauri::Context<tauri::Wry>) {
    let directory = root().expect("invalid isolated skin QA profile").expect("missing skin QA profile");
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .invoke_handler(tauri::generate_handler![
            super::project_runtime_configuration, super::skin_store_window::skin_store_open_request,
            super::skin_store_get_settings, super::skin_store_set_url, super::skin_store_list,
            super::skin_store_refresh, super::skin_store_preview, super::skin_store_install, super::skin_store_purchase,
            super::skin_account_cached, super::skin_account_set_service, super::skin_account_request_code,
            super::skin_account_login, super::skin_account_refresh, super::skin_account_logout,
            super::skin_account_resume,
            super::skin_account_more_orders,
            super::skin_account_cancel,
            skin_qa_report,
        ])
        .on_page_load(|window, payload| {
            eprintln!("skin QA page: {:?} {}", payload.event(), payload.url());
            if payload.event() == tauri::webview::PageLoadEvent::Finished {
                if let Ok(Some(root)) = root() {
                    if let Ok(script) = std::fs::read_to_string(root.join("steps.js")) {
                        if script.len() < 64 * 1024 {
                            if let Err(error) = window.eval(&script) { eprintln!("skin QA script: {error}"); }
                        }
                    }
                }
            }
        })
        .setup(move |app| {
            app.manage(super::storage::AppStorage::open(directory.join("manager.sqlite3")).map_err(std::io::Error::other)?);
            app.manage(super::skin_store::StoreBackend::new(directory.join("Store"), directory.join("Skins")));
            let commerce: super::skin_commerce::Client = std::sync::Arc::new(super::skin_commerce::CommerceClient::new(directory.join("Store")));
            let entitlements: super::skin_store::Entitlements = commerce.clone();
            app.manage(commerce);
            app.manage(entitlements);
            let window = app.get_webview_window("main").ok_or("QA main window missing")?;
            let url = "http://localhost:1420/?skin-qa".parse()?;
            window.navigate(url)?;
            window.set_title("知字 · 商店连接验证")?;
            window.set_position(tauri::LogicalPosition::new(80.0, 80.0))?;
            window.show()?;
            window.set_focus()?;
            Ok(())
        })
        .build(context)
        .expect("skin QA failed")
        .run(|app, event| {
            #[cfg(target_os = "macos")]
            super::skin_store_window::handle_open(app, &event);
        });
}
