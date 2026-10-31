use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

const OFFICIAL_MANIFEST: &str = include_str!("../../native/resources/ContextModel/SHA256SUMS");

#[derive(Clone, Copy, Default, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
enum ModelTier {
    #[default]
    Official,
    Public,
    Custom,
}

#[derive(Deserialize)]
struct BundleMetadata {
    #[serde(default)]
    tier: ModelTier,
}

#[derive(Serialize)]
pub struct Availability {
    pub available: bool,
    pub reason: Option<String>,
}

fn read_bundle_file(directory: &Path, name: &str) -> Option<Vec<u8>> {
    if name.is_empty()
        || name.contains('\\')
        || name.split('/').any(|part| part.is_empty() || part == ".")
    {
        return None;
    }
    let mut path = directory.to_path_buf();
    for component in Path::new(name).components() {
        if !matches!(component, Component::Normal(_)) {
            return None;
        }
        path.push(component);
        if std::fs::symlink_metadata(&path)
            .ok()?
            .file_type()
            .is_symlink()
        {
            return None;
        }
    }
    if !std::fs::symlink_metadata(&path).ok()?.is_file() {
        return None;
    }
    std::fs::read(path).ok()
}

fn model_tier(directory: &Path) -> Option<ModelTier> {
    match std::fs::symlink_metadata(directory.join("availability.json")) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(ModelTier::Official),
        Err(_) => None,
        Ok(_) => serde_json::from_slice::<BundleMetadata>(&read_bundle_file(
            directory,
            "availability.json",
        )?)
        .ok()
        .map(|metadata| metadata.tier),
    }
}

fn verify_manifest(directory: &Path, manifest: &str) -> bool {
    let mut names = HashSet::new();
    for line in manifest.lines() {
        let Some((hash, name)) = line.split_once("  ") else {
            return false;
        };
        if hash.len() != 64 || !names.insert(name) {
            return false;
        }
        let Some(bytes) = read_bundle_file(directory, name) else {
            return false;
        };
        if format!("{:x}", Sha256::digest(bytes)) != hash {
            return false;
        }
    }
    // A mutable list must not enable an empty or partially attested model.
    OFFICIAL_MANIFEST.lines().all(|line| {
        line.split_once("  ")
            .is_some_and(|(_, name)| names.contains(name))
    })
}

fn inspect(directory: &Path, official_manifest: &str) -> Availability {
    let tier = model_tier(directory);
    let available = std::fs::symlink_metadata(directory).is_ok_and(|metadata| metadata.is_dir())
        && match tier {
            // Official and legacy bundles always use the embedded trust root.
            Some(ModelTier::Official) => verify_manifest(directory, official_manifest),
            Some(ModelTier::Public | ModelTier::Custom) => {
                read_bundle_file(directory, "SHA256SUMS")
                    .and_then(|bytes| String::from_utf8(bytes).ok())
                    .is_some_and(|manifest| verify_manifest(directory, &manifest))
            }
            None => false,
        };
    Availability {
        available,
        reason: if !available {
            Some("本机模型文件缺失或校验失败；上下文模型排序不可用。".into())
        } else if tier != Some(ModelTier::Official) {
            // Bundle hashes prove integrity, not the identity of official weights.
            Some("本机构建的模型（未与官方权重比对）".into())
        } else {
            None
        },
    }
}

pub fn verify(directory: &Path) -> Availability {
    inspect(directory, OFFICIAL_MANIFEST)
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
    Ok(verify(&bundle.join("Contents/Resources/ContextModel")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    const OFFICIAL: &str = include_str!("../../native/resources/ContextModel/SHA256SUMS");

    struct Bundle(PathBuf);

    impl Bundle {
        fn new(tier: Option<&str>) -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "zhizi-context-model-tests-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            let bundle = Self(path);
            let mut manifest = String::new();
            for line in OFFICIAL.lines() {
                let (_, name) = line.split_once("  ").unwrap();
                bundle.write(name, name);
                manifest.push_str(&format!("{:x}  {name}\n", Sha256::digest(name.as_bytes())));
            }
            bundle.write("SHA256SUMS", &manifest);
            if let Some(tier) = tier {
                bundle.write(
                    "availability.json",
                    &format!(r#"{{"available":true,"tier":"{tier}"}}"#),
                );
            }
            bundle
        }

        fn write(&self, name: &str, data: &str) {
            let path = self.0.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, data).unwrap();
        }
    }

    impl Drop for Bundle {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn public_and_custom_accept_complete_bundle_and_reject_any_changed_file() {
        for tier in ["public", "custom"] {
            let bundle = Bundle::new(Some(tier));
            let status = verify(&bundle.0);
            assert!(status.available, "{tier} bundle should be usable");
            assert_eq!(
                status.reason.as_deref(),
                Some("本机构建的模型（未与官方权重比对）")
            );
            for line in OFFICIAL.lines() {
                let (_, name) = line.split_once("  ").unwrap();
                bundle.write(name, "changed");
                assert!(!verify(&bundle.0).available, "{tier}: changed {name}");
                fs::remove_file(bundle.0.join(name)).unwrap();
                assert!(!verify(&bundle.0).available, "{tier}: missing {name}");
                bundle.write(name, name);
                assert!(verify(&bundle.0).available, "{tier}: restored {name}");
            }
        }
    }

    #[test]
    fn official_and_legacy_ignore_mutable_bundle_manifest() {
        for tier in [Some("official"), None] {
            let bundle = Bundle::new(tier);
            assert!(!verify(&bundle.0).available);
            bundle.write(
                "availability.json",
                r#"{"available":true,"official":false}"#,
            );
            assert!(
                !verify(&bundle.0).available,
                "no tier must still use official hashes"
            );
        }
    }

    #[test]
    fn official_and_legacy_verify_pinned_files_without_bundle_manifest() {
        for metadata in [
            None,
            Some(r#"{"available":true}"#),
            Some(r#"{"tier":"official"}"#),
        ] {
            let bundle = Bundle::new(None);
            if let Some(metadata) = metadata {
                bundle.write("availability.json", metadata);
            }
            // Synthetic pinned bytes avoid reading any private model artifacts.
            let pinned = fs::read_to_string(bundle.0.join("SHA256SUMS")).unwrap();
            fs::remove_file(bundle.0.join("SHA256SUMS")).unwrap();
            let status = inspect(&bundle.0, &pinned);
            assert!(status.available);
            assert!(status.reason.is_none());
            bundle.write("itos.json", "changed");
            assert!(!inspect(&bundle.0, &pinned).available);
        }
    }

    #[test]
    fn local_bundle_rejects_missing_incomplete_or_malformed_manifests() {
        let bundle = Bundle::new(Some("public"));
        let manifest = fs::read_to_string(bundle.0.join("SHA256SUMS")).unwrap();
        fs::remove_file(bundle.0.join("SHA256SUMS")).unwrap();
        assert!(!verify(&bundle.0).available);
        for omitted in 0..OFFICIAL.lines().count() {
            let incomplete = manifest
                .lines()
                .enumerate()
                .filter(|(index, _)| *index != omitted)
                .map(|(_, line)| line)
                .collect::<Vec<_>>()
                .join("\n");
            bundle.write("SHA256SUMS", &incomplete);
            assert!(!verify(&bundle.0).available);
        }
        for malformed in [
            String::new(),
            "invalid".into(),
            format!("{manifest}{manifest}"),
        ] {
            bundle.write("SHA256SUMS", &malformed);
            assert!(!verify(&bundle.0).available);
        }
        for name in [
            "../itos.json",
            "/itos.json",
            "./itos.json",
            "zhizi30m_v2.mlmodelc/../itos.json",
            "\\itos.json",
        ] {
            bundle.write(
                "SHA256SUMS",
                &format!("{manifest}{:x}  {name}\n", Sha256::digest(b"itos.json")),
            );
            assert!(!verify(&bundle.0).available);
        }
    }

    #[test]
    fn invalid_tier_metadata_cannot_enable_a_local_model() {
        let bundle = Bundle::new(Some("public"));
        for metadata in ["{broken", "[]", r#"{"tier":"other"}"#, r#"{"tier":null}"#] {
            bundle.write("availability.json", metadata);
            assert!(!verify(&bundle.0).available);
        }
    }

    #[cfg(unix)]
    #[test]
    fn local_bundle_rejects_symlinked_files_and_model_directories() {
        use std::os::unix::fs::symlink;
        let bundle = Bundle::new(Some("public"));
        for name in [
            "availability.json",
            "SHA256SUMS",
            "itos.json",
            "zhizi30m_v2.mlmodelc",
        ] {
            let path = bundle.0.join(name);
            let original = bundle.0.join("original");
            fs::rename(&path, &original).unwrap();
            symlink(&original, &path).unwrap();
            assert!(!verify(&bundle.0).available, "symlinked {name}");
            fs::remove_file(&path).unwrap();
            fs::rename(&original, &path).unwrap();
            assert!(verify(&bundle.0).available);
        }
    }
}
