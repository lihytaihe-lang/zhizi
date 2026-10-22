//! Start-menu switches for the Windows build (no console window):
//!
//!   zhizi-settings --typo-correction on|off
//!
//! Writes `%APPDATA%\Zhizi\settings.json`; running applications pick the change up the next time they gain focus.
#![cfg_attr(windows, windows_subsystem = "windows")]
use std::path::PathBuf;

fn data_directory() -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(std::env::var_os("APPDATA").unwrap_or_default()).join("Zhizi")
    } else {
        let base = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share"));
        base.join("zhizi")
    }
}

#[cfg(windows)]
fn notice(text: &str) {
    #[link(name = "user32")]
    unsafe extern "system" {
        fn MessageBoxW(window: isize, text: *const u16, caption: *const u16, kind: u32) -> i32;
    }
    let wide = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
    unsafe { MessageBoxW(0, wide(text).as_ptr(), wide("知字").as_ptr(), 0x40) };
}

#[cfg(not(windows))]
fn notice(text: &str) {
    println!("{text}");
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let on = match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["--typo-correction", "on"] => true,
        ["--typo-correction", "off"] => false,
        _ => {
            notice("用法：zhizi-settings --typo-correction on|off");
            std::process::exit(2);
        }
    };
    match zhizi_host::settings::save(&data_directory(), zhizi_host::settings::Settings { typo_correction: on }) {
        Ok(()) => notice(if on {
            "已开启键位纠错（打错、打反的字母会自动纠正）。正在使用的程序在下次切换窗口后生效。"
        } else {
            "已关闭键位纠错。正在使用的程序在下次切换窗口后生效。"
        }),
        Err(error) => {
            notice(&format!("没能保存设置：{error}"));
            std::process::exit(1);
        }
    }
}
