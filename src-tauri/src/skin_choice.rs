//! The macOS input menu/gallery override has priority over runtime.json.
pub fn current() -> Option<String> {
    if isolated() { return None; }
    #[cfg(target_os = "macos")]
    { crate::ime_preferences::read("candidateSkinOverride") }
    #[cfg(not(target_os = "macos"))]
    { None }
}

fn isolated() -> bool {
    if cfg!(test) { return true; }
    #[cfg(debug_assertions)]
    if std::env::var_os("ZHIZI_SKIN_QA_ROOT").is_some() { return true; }
    false
}
