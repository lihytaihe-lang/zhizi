//! The few switches a shell keeps in `<data>/settings.json`, and the Chinese / English mode it remembers per application
//! (`<data>/modes.json`, as the Mac remembers it in its preferences). Both files are optional: a missing or unreadable file means
//! the defaults. Writes go through a temporary file so a reader never sees half a file.
use std::path::Path;

/// Switches changed from the Start menu (Windows) or a settings page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    /// Typing slips (swapped or doubled letters) are corrected. On unless switched off.
    pub typo_correction: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self { typo_correction: true }
    }
}

fn read_json(path: &Path) -> Option<serde_json::Value> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

fn write_json(path: &Path, value: &serde_json::Value) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&temporary, value.to_string())?;
    std::fs::rename(temporary, path)
}

pub fn load(data_dir: &Path) -> Settings {
    let value = read_json(&data_dir.join("settings.json"));
    Settings { typo_correction: value.and_then(|v| v["typoCorrection"].as_bool()).unwrap_or(true) }
}

pub fn save(data_dir: &Path, settings: Settings) -> std::io::Result<()> {
    write_json(&data_dir.join("settings.json"), &serde_json::json!({ "typoCorrection": settings.typo_correction }))
}

/// Whether the person left `app` (a process file name) in English mode the last time.
pub fn english_mode(data_dir: &Path, app: &str) -> bool {
    read_json(&data_dir.join("modes.json")).and_then(|v| v[app.to_ascii_lowercase()].as_bool()).unwrap_or(false)
}

pub fn save_english_mode(data_dir: &Path, app: &str, english: bool) -> std::io::Result<()> {
    let path = data_dir.join("modes.json");
    let mut value = read_json(&path).filter(|v| v.is_object()).unwrap_or_else(|| serde_json::json!({}));
    let key = app.to_ascii_lowercase();
    if english {
        value[key] = serde_json::Value::Bool(true);
    } else if let Some(map) = value.as_object_mut() {
        map.remove(&key); // Chinese is the default: keep the file small
    }
    write_json(&path, &value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("zhizi-settings-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn typo_correction_is_on_unless_switched_off() {
        let dir = scratch("typo");
        assert!(load(&dir).typo_correction, "no file: default on");
        save(&dir, Settings { typo_correction: false }).unwrap();
        assert!(!load(&dir).typo_correction);
        save(&dir, Settings { typo_correction: true }).unwrap();
        assert!(load(&dir).typo_correction);
        std::fs::write(dir.join("settings.json"), "not json").unwrap();
        assert!(load(&dir).typo_correction, "unreadable file: default on");
    }

    #[test]
    fn english_mode_is_remembered_per_application() {
        let dir = scratch("modes");
        assert!(!english_mode(&dir, "Code.exe"));
        save_english_mode(&dir, "Code.exe", true).unwrap();
        save_english_mode(&dir, "notepad.exe", true).unwrap();
        assert!(english_mode(&dir, "code.exe"), "file names compare without case");
        save_english_mode(&dir, "CODE.EXE", false).unwrap();
        assert!(!english_mode(&dir, "Code.exe"));
        assert!(english_mode(&dir, "notepad.exe"));
    }
}
