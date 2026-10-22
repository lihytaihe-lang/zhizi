use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

#[derive(Serialize)]
pub struct Availability {
    pub available: bool,
    pub reason: Option<String>,
}

pub fn verify(directory: &Path) -> bool {
    // The embedded list, never a mutable bundle manifest, is the trust root.
    include_str!("../../native/resources/ContextModel/SHA256SUMS")
        .lines()
        .all(|line| {
            let Some((hash, name)) = line.split_once("  ") else {
                return false;
            };
            let file = directory.join(name);
            let Ok(metadata) = std::fs::symlink_metadata(&file) else {
                return false;
            };
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return false;
            }
            std::fs::read(file).is_ok_and(|bytes| format!("{:x}", Sha256::digest(bytes)) == hash)
        })
}

pub fn availability() -> Result<Availability, String> {
    let home = std::env::var_os("HOME").ok_or("无法确定用户目录。")?;
    let installed = [
        PathBuf::from(&home).join("Library/Input Methods/Agent Input Method.app"),
        PathBuf::from(home).join("Library/Input Methods/Agent Input Method.inputmethod"),
        PathBuf::from("/Library/Input Methods/Agent Input Method.app"),
        PathBuf::from("/Library/Input Methods/Agent Input Method.inputmethod"),
    ];
    // In a development checkout, inspect the built bundle only if no installed
    // bundle exists. External training artifacts alone never enable the UI.
    let bundle = installed
        .into_iter()
        .find(|path| path.exists())
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../build/native/release/Agent Input Method.inputmethod")
        });
    let available = verify(&bundle.join("Contents/Resources/ContextModel"));
    Ok(Availability {
        available,
        reason: (!available).then(|| "本机模型文件缺失或校验失败；上下文模型排序不可用。".into()),
    })
}
