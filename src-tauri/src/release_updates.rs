use serde::{Deserialize, Serialize};
use std::time::Duration;

const ENDPOINT: &str = "https://zhizi.dabblecove.com/updates/stable.json";
const MAX_MANIFEST_BYTES: usize = 32 * 1024;

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub(crate) struct ReleaseIdentity {
    version: String,
    build: String,
}

impl ReleaseIdentity {
    fn number(&self) -> Option<([u32; 3], u32)> {
        let version = version_number(&self.version, false)?;
        let build = self.build.parse::<u32>().ok()?;
        (build > 0 && self.build == build.to_string()).then_some((version, build))
    }
}

fn version_number(text: &str, allow_two_components: bool) -> Option<[u32; 3]> {
    let mut version = [0; 3];
    let mut parts = text.split('.');
    for (index, component) in version.iter_mut().enumerate() {
        if index == 2 && allow_two_components && parts.clone().next().is_none() {
            break;
        }
        let text = parts.next()?;
        *component = text.parse().ok()?;
        if text != component.to_string() {
            return None;
        }
    }
    parts.next().is_none().then_some(version)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReleaseInfo {
    manager: ReleaseIdentity,
    installed: Option<ReleaseIdentity>,
    loaded: Option<ReleaseIdentity>,
    bundled: Option<ReleaseIdentity>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StableManifest {
    schema_version: u32,
    channel: String,
    version: String,
    build: String,
    platform: String,
    #[serde(rename = "minimumMacOS")]
    minimum_mac_os: String,
    #[serde(rename = "downloadURL")]
    download_url: String,
    #[serde(rename = "notesURL")]
    notes_url: String,
    sha256: String,
    developer_id_signed: bool,
    apple_notarized: bool,
}

#[derive(Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub(crate) enum UpdateCheck {
    Current,
    Incompatible,
    Unavailable,
    Available {
        release: ReleaseIdentity,
        #[serde(rename = "downloadUrl")]
        download_url: String,
        #[serde(rename = "notesUrl")]
        notes_url: String,
    },
}

fn evaluate(
    payload: &[u8],
    current: &ReleaseIdentity,
    platform: &str,
    os: &str,
) -> Option<UpdateCheck> {
    let manifest: StableManifest = serde_json::from_slice(payload).ok()?;
    if manifest.schema_version != 1 || manifest.channel != "stable" {
        return None;
    }
    if manifest.platform != platform {
        return Some(UpdateCheck::Incompatible);
    }
    if version_number(os, true)? < version_number(&manifest.minimum_mac_os, true)? {
        return Some(UpdateCheck::Incompatible);
    }
    let release = ReleaseIdentity {
        version: manifest.version,
        build: manifest.build,
    };
    let latest = release.number()?;
    let tag = format!("v{}", release.version);
    let asset = format!("Zhizi-{}-macos-arm64.dmg", release.version);
    let releases = "https://github.com/lihytaihe-lang/zhizi/releases";
    if manifest.download_url != format!("{releases}/download/{tag}/{asset}")
        || manifest.notes_url != format!("{releases}/tag/{tag}")
        || !manifest.developer_id_signed
        || !manifest.apple_notarized
        || manifest.sha256.len() != 64
        || !manifest
            .sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return None;
    }
    if latest <= current.number()? {
        return Some(UpdateCheck::Current);
    }
    Some(UpdateCheck::Available {
        release,
        download_url: manifest.download_url,
        notes_url: manifest.notes_url,
    })
}

async fn fetch_manifest() -> Option<Vec<u8>> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(10))
        .build()
        .ok()?;
    let mut response = client
        .get(ENDPOINT)
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?;
    if response.status() != reqwest::StatusCode::OK
        || response
            .content_length()
            .is_some_and(|length| length > MAX_MANIFEST_BYTES as u64)
    {
        return None;
    }
    let mut payload = Vec::new();
    while let Some(chunk) = response.chunk().await.ok()? {
        if payload.len() + chunk.len() > MAX_MANIFEST_BYTES {
            return None;
        }
        payload.extend_from_slice(&chunk);
    }
    Some(payload)
}

#[tauri::command]
pub(crate) async fn check_stable_update() -> UpdateCheck {
    if !cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        return UpdateCheck::Incompatible;
    }
    let Ok((info, os)) =
        tauri::async_runtime::spawn_blocking(|| (local_release_info(), platform::os_version()))
            .await
    else {
        return UpdateCheck::Unavailable;
    };
    let Some(os) = os else {
        return UpdateCheck::Unavailable;
    };
    let Some(payload) = fetch_manifest().await else {
        return UpdateCheck::Unavailable;
    };
    evaluate(&payload, &info.manager, "macos-arm64", &os).unwrap_or(UpdateCheck::Unavailable)
}

#[tauri::command]
pub(crate) async fn get_release_info() -> Result<ReleaseInfo, String> {
    tauri::async_runtime::spawn_blocking(local_release_info)
        .await
        .map_err(|error| error.to_string())
}

fn local_release_info() -> ReleaseInfo {
    ReleaseInfo {
        manager: platform::manager_identity().unwrap_or_else(|| ReleaseIdentity {
            version: env!("CARGO_PKG_VERSION").into(),
            build: String::new(),
        }),
        installed: platform::installed_identity(),
        loaded: platform::loaded_identity(),
        bundled: platform::bundled_identity(),
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::*;
    use std::{
        fs::File,
        io::Read,
        path::{Path, PathBuf},
        process::Command,
    };

    const BUNDLE_NAME: &str = "Agent Input Method.app";
    const IME_ID: &str = "com.hongye.facai.inputmethod.zhizi";

    fn home() -> Option<PathBuf> {
        std::env::var_os("HOME").map(PathBuf::from)
    }

    fn manager_contents() -> Option<PathBuf> {
        Some(
            std::env::current_exe()
                .ok()?
                .parent()?
                .parent()?
                .to_path_buf(),
        )
    }

    fn installed_bundle() -> Option<PathBuf> {
        Some(home()?.join("Library/Input Methods").join(BUNDLE_NAME))
    }

    fn identity(plist: &Path) -> Option<ReleaseIdentity> {
        let value = |key: &str| {
            let output = Command::new("/usr/bin/plutil")
                .args(["-extract", key, "raw", "-o", "-"])
                .arg(plist)
                .output()
                .ok()?;
            output
                .status
                .success()
                .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        };
        let identity = ReleaseIdentity {
            version: value("CFBundleShortVersionString")?,
            build: value("CFBundleVersion")?,
        };
        identity.number().map(|_| identity)
    }

    pub(super) fn manager_identity() -> Option<ReleaseIdentity> {
        identity(&manager_contents()?.join("Info.plist"))
    }

    pub(super) fn installed_identity() -> Option<ReleaseIdentity> {
        identity(&installed_bundle()?.join("Contents/Info.plist"))
    }

    pub(super) fn bundled_identity() -> Option<ReleaseIdentity> {
        identity(
            &manager_contents()?
                .join("Resources/InputMethod")
                .join(BUNDLE_NAME)
                .join("Contents/Info.plist"),
        )
    }

    pub(super) fn os_version() -> Option<String> {
        let output = Command::new("/usr/bin/sw_vers")
            .arg("-productVersion")
            .output()
            .ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct RunningMarker {
        format_version: u32,
        bundle_identifier: String,
        #[serde(rename = "processID")]
        process_id: u32,
        launched_at: String,
        version: String,
        build: String,
    }

    pub(super) fn loaded_identity() -> Option<ReleaseIdentity> {
        let path =
            home()?.join("Library/Application Support/AgentInputMethod/Runtime/running.marker");
        if !path.symlink_metadata().ok()?.is_file() {
            return None;
        }
        let mut bytes = Vec::new();
        File::open(path)
            .ok()?
            .take(16_385)
            .read_to_end(&mut bytes)
            .ok()?;
        if bytes.len() > 16_384 {
            return None;
        }
        let marker: RunningMarker = serde_json::from_slice(&bytes).ok()?;
        if marker.format_version != 1
            || marker.bundle_identifier != IME_ID
            || marker.process_id == 0
        {
            return None;
        }
        let executable = installed_bundle()?.join("Contents/MacOS/AgentInputMethod");
        if crate::native_integration::process_executable(i32::try_from(marker.process_id).ok()?)?
            != executable
        {
            return None;
        }
        // Match the live process and its launch time so a stale marker, even
        // with a reused PID, does not report the bundle on disk as running.
        let output = Command::new("/bin/ps")
            .env("LC_ALL", "C")
            .env("TZ", "UTC")
            .args(["-p", &marker.process_id.to_string(), "-o", "lstart="])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let line = String::from_utf8_lossy(&output.stdout);
        let started = chrono::NaiveDateTime::parse_from_str(line.trim(), "%a %b %e %H:%M:%S %Y")
            .ok()?
            .and_utc();
        let launched = chrono::DateTime::parse_from_rfc3339(&marker.launched_at).ok()?;
        if !(0..=60).contains(&(launched.to_utc() - started).num_seconds()) {
            return None;
        }
        let identity = ReleaseIdentity {
            version: marker.version,
            build: marker.build,
        };
        identity.number().map(|_| identity)
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use super::ReleaseIdentity;

    pub(super) fn manager_identity() -> Option<ReleaseIdentity> {
        None
    }
    pub(super) fn installed_identity() -> Option<ReleaseIdentity> {
        None
    }
    pub(super) fn loaded_identity() -> Option<ReleaseIdentity> {
        None
    }
    pub(super) fn bundled_identity() -> Option<ReleaseIdentity> {
        None
    }
    pub(super) fn os_version() -> Option<String> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> serde_json::Value {
        serde_json::json!({
            "schemaVersion": 1, "channel": "stable", "version": "0.3.50", "build": "2627",
            "platform": "macos-arm64", "minimumMacOS": "14.0",
            "downloadURL": "https://github.com/lihytaihe-lang/zhizi/releases/download/v0.3.50/Zhizi-0.3.50-macos-arm64.dmg",
            "notesURL": "https://github.com/lihytaihe-lang/zhizi/releases/tag/v0.3.50",
            "sha256": "a".repeat(64), "developerIdSigned": true, "appleNotarized": true,
        })
    }

    fn check(
        payload: &serde_json::Value,
        version: &str,
        build: &str,
        os: &str,
    ) -> Option<UpdateCheck> {
        let current = ReleaseIdentity {
            version: version.into(),
            build: build.into(),
        };
        evaluate(
            &serde_json::to_vec(payload).unwrap(),
            &current,
            "macos-arm64",
            os,
        )
    }

    #[test]
    fn stable_check_orders_versions_then_builds_and_rejects_incompatible_systems() {
        let manifest = manifest();
        assert!(matches!(
            check(&manifest, "0.3.49", "2690", "14.0"),
            Some(UpdateCheck::Available { .. })
        ));
        assert!(matches!(
            check(&manifest, "0.3.50", "2626", "14.0.0"),
            Some(UpdateCheck::Available { .. })
        ));
        assert!(matches!(
            check(&manifest, "0.3.50", "2627", "26.0"),
            Some(UpdateCheck::Current)
        ));
        assert!(matches!(
            check(&manifest, "0.3.50", "2628", "26.0"),
            Some(UpdateCheck::Current)
        ));
        assert!(matches!(
            check(&manifest, "0.3.49", "2621", "13.9"),
            Some(UpdateCheck::Incompatible)
        ));
    }

    #[test]
    fn invalid_manifest_cannot_supply_update_links() {
        for (field, value) in [
            ("schemaVersion", serde_json::json!(2)),
            (
                "downloadURL",
                serde_json::json!("https://github.com@example.test/patch.dmg"),
            ),
            (
                "notesURL",
                serde_json::json!("https://github.com/lihytaihe-lang/zhizi/releases/tag/v0.3.49"),
            ),
            ("appleNotarized", serde_json::json!(false)),
        ] {
            let mut manifest = manifest();
            manifest[field] = value;
            assert!(
                check(&manifest, "0.3.49", "2621", "14.0").is_none(),
                "{field}"
            );
        }
    }
}
