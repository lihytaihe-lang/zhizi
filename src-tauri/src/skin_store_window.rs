use std::sync::atomic::{AtomicU64, Ordering};

static OPEN_REQUEST: AtomicU64 = AtomicU64::new(0);

#[tauri::command]
pub fn skin_store_open_request() -> u64 {
    OPEN_REQUEST.load(Ordering::Relaxed)
}

#[cfg(target_os = "macos")]
pub fn handle_open(app: &tauri::AppHandle, event: &tauri::RunEvent) {
    use tauri::Emitter;

    // The URL opens a page. It carries no purchase, login or download action.
    if let tauri::RunEvent::Opened { urls } = event {
        if urls.iter().any(|url| url.as_str() == "zhizi-skins://store/") {
            let request = OPEN_REQUEST.fetch_add(1, Ordering::Relaxed) + 1;
            super::show_main_window(app);
            let _ = app.emit_to(super::MAIN_WINDOW_LABEL, "zhizi://open-skin-store", request);
        }
    }
}

#[cfg(target_os = "macos")]
pub fn notify_packages_changed() {
    #[cfg(debug_assertions)]
    if std::env::var_os("ZHIZI_SKIN_QA_ROOT").is_some() { return; }

    unsafe extern "C" {
        fn notify_post(name: *const std::ffi::c_char) -> u32;
    }

    // The offline gallery rereads the shared directory when it regains focus.
    unsafe { notify_post(c"com.hongye.facai.inputmethod.zhizi.skin-packages-changed".as_ptr()) };
}
