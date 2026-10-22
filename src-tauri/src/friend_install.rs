//! Installs the input method carried inside a packaged manager (friends' test builds:
//! drag 知字 to Applications, open it once). The packaging script places the input
//! method bundle at `Contents/Resources/InputMethod/Agent Input Method.app`; builds
//! without it (the owner's development installs) skip this entirely.

use std::path::{Path, PathBuf};
use std::process::Command;

const BUNDLE_NAME: &str = "Agent Input Method.app";

fn bundled_input_method() -> Option<PathBuf> {
    let executable = std::env::current_exe().ok()?;
    let contents = executable.parent()?.parent()?;
    let bundle = contents.join("Resources/InputMethod").join(BUNDLE_NAME);
    bundle.is_dir().then_some(bundle)
}

fn bundle_version(bundle: &Path) -> Option<u64> {
    let output = Command::new("/usr/libexec/PlistBuddy")
        .args(["-c", "Print :CFBundleVersion"])
        .arg(bundle.join("Contents/Info.plist"))
        .output()
        .ok()?;
    String::from_utf8(output.stdout).ok()?.trim().parse().ok()
}

fn run(program: &str, arguments: &[&str]) -> bool {
    Command::new(program)
        .args(arguments)
        .output()
        .is_ok_and(|output| output.status.success())
}

/// Returns true when it installed or upgraded the input method.
pub fn ensure_input_method_installed() -> bool {
    let Some(source) = bundled_input_method() else {
        return false;
    };
    let Some(home) = std::env::var_os("HOME") else {
        return false;
    };
    let directory = PathBuf::from(home).join("Library/Input Methods");
    let destination = directory.join(BUNDLE_NAME);
    let wanted = bundle_version(&source);
    if destination.is_dir() && bundle_version(&destination) >= wanted {
        return false;
    }
    if std::fs::create_dir_all(&directory).is_err() {
        return false;
    }
    // Stop a running older copy so the new one starts on the next keystroke.
    let _ = run("/usr/bin/pkill", &["-f", "Input Methods/Agent Input Method.app/Contents/MacOS/AgentInputMethod"]);
    let staging = directory.join(".Agent Input Method.app.installing");
    let _ = std::fs::remove_dir_all(&staging);
    let (Some(source_text), Some(staging_text), Some(destination_text)) =
        (source.to_str(), staging.to_str(), destination.to_str())
    else {
        return false;
    };
    if !run("/usr/bin/ditto", &[source_text, staging_text]) {
        let _ = std::fs::remove_dir_all(&staging);
        return false;
    }
    let _ = std::fs::remove_dir_all(&destination);
    if std::fs::rename(&staging, &destination).is_err() {
        return false;
    }
    let _ = run(
        "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister",
        &["-f", destination_text],
    );
    let control = destination.join("Contents/MacOS/AgentIMEInputSourceCtl");
    if let Some(control) = control.to_str() {
        let _ = run(control, &["register"]);
        let _ = run(control, &["enable"]);
    }
    true
}

/// True when this manager carries an input method (a friends' package), so it also offers the uninstall.
pub fn is_friend_build() -> bool {
    bundled_input_method().is_some()
}

/// Removes what the friends' package installed: the hourly upload task and the input method
/// bundle. Local typing data stays on the machine; the input source entry in System Settings
/// disappears after the next sign-out. The system's input source is deliberately not disabled.
pub fn uninstall_input_method() -> bool {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return false;
    };
    #[cfg(feature = "test-upload")]
    {
        let label = crate::test_upload::LAUNCH_AGENT_LABEL;
        let agent = home.join(format!("Library/LaunchAgents/{label}.plist"));
        let uid = Command::new("/usr/bin/id").arg("-u").output().ok().and_then(|o| String::from_utf8(o.stdout).ok());
        if let Some(uid) = uid {
            let _ = run("/bin/launchctl", &["bootout", &format!("gui/{}/{label}", uid.trim())]);
        }
        let _ = std::fs::remove_file(&agent);
    }
    let _ = run("/usr/bin/pkill", &["-f", "Input Methods/Agent Input Method.app/Contents/MacOS/AgentInputMethod"]);
    let destination = home.join("Library/Input Methods").join(BUNDLE_NAME);
    !destination.exists() || std::fs::remove_dir_all(&destination).is_ok()
}
