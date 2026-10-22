//! The input method's own toggles (typo correction, fuzzy pinyin) live in its
//! user defaults domain and are switched from its input menu. The manager home
//! page reads and writes the same keys; the input method applies them the next
//! time a client activates it.

use serde::{Deserialize, Serialize};
use std::process::Command;

const DOMAIN: &str = "com.hongye.facai.inputmethod.zhizi";
const DEFAULTS: &str = "/usr/bin/defaults";
/// The fuzzy-pinyin bits the input menu offers (see AgentInputController.fuzzyLabels).
pub const FUZZY_MASK_LIMIT: u32 = 0xff;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImePreferences {
    pub typo_correction_enabled: bool,
    pub fuzzy_mask: u32,
}

fn read(key: &str) -> Option<String> {
    let output = Command::new(DEFAULTS).args(["read", DOMAIN, key]).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn write(key: &str, kind: &str, value: &str) -> Result<(), String> {
    let status = Command::new(DEFAULTS)
        .args(["write", DOMAIN, key, kind, value])
        .status()
        .map_err(|error| format!("无法写入输入法设置：{error}"))?;
    status
        .success()
        .then_some(())
        .ok_or_else(|| "无法写入输入法设置".to_string())
}

pub fn parse(typo: Option<&str>, fuzzy: Option<&str>) -> ImePreferences {
    ImePreferences {
        typo_correction_enabled: matches!(typo, Some("1") | Some("YES") | Some("true")),
        fuzzy_mask: fuzzy
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(0)
            & FUZZY_MASK_LIMIT,
    }
}

#[tauri::command]
pub fn ime_preferences() -> ImePreferences {
    parse(
        read("typoCorrectionEnabled").as_deref(),
        read("fuzzyMask").as_deref(),
    )
}

#[tauri::command]
pub fn set_ime_typo_correction(enabled: bool) -> Result<ImePreferences, String> {
    write("typoCorrectionEnabled", "-bool", if enabled { "YES" } else { "NO" })?;
    Ok(ime_preferences())
}

#[tauri::command]
pub fn set_ime_fuzzy_mask(mask: u32) -> Result<ImePreferences, String> {
    if mask > FUZZY_MASK_LIMIT {
        return Err("模糊音选项超出范围".to_string());
    }
    write("fuzzyMask", "-int", &mask.to_string())?;
    Ok(ime_preferences())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_defaults_output_and_clamps_the_fuzzy_mask() {
        assert_eq!(parse(Some("1"), Some("231")), ImePreferences { typo_correction_enabled: true, fuzzy_mask: 231 });
        assert_eq!(parse(Some("0"), None), ImePreferences { typo_correction_enabled: false, fuzzy_mask: 0 });
        assert_eq!(parse(None, Some("garbage")).fuzzy_mask, 0);
        assert_eq!(parse(None, Some("65535")).fuzzy_mask, 65535 & FUZZY_MASK_LIMIT);
        assert!(set_ime_fuzzy_mask(FUZZY_MASK_LIMIT + 1).is_err());
    }
}
