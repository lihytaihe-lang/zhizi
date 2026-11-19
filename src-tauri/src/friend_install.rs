//! Installs the input method carried inside a packaged manager (friends' test builds:
//! drag 知字 to Applications, open it once). The packaging script places the input
//! method bundle at `Contents/Resources/InputMethod/Agent Input Method.app`; builds
//! without it (the owner's development installs) skip this entirely.

use std::ffi::{c_char, c_int, c_uint, CString};
use std::fs::OpenOptions;
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

const BUNDLE_NAME: &str = "Agent Input Method.app";
const STAGING_NAME: &str = ".Agent Input Method.app.installing";
const RENAME_SWAP: c_uint = 0x0000_0002;
const EXCLUSIVE_NONBLOCKING: c_int = 2 | 4;

extern "C" {
    fn renamex_np(source: *const c_char, destination: *const c_char, flags: c_uint) -> c_int;
}

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

fn publish_bundle(staging: &Path, destination: &Path) -> std::io::Result<()> {
    if !destination.exists() {
        return std::fs::rename(staging, destination);
    }
    let from = CString::new(staging.as_os_str().as_bytes())?;
    let to = CString::new(destination.as_os_str().as_bytes())?;
    // Both app directories are on the same volume. An interruption leaves a
    // complete bundle at the installed path, with the previous one in staging.
    if unsafe { renamex_np(from.as_ptr(), to.as_ptr(), RENAME_SWAP) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

fn install_lock(directory: &Path) -> Option<std::fs::File> {
    let lock = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(directory.join(".zhizi-install.lock"))
        .ok()?;
    // Closing this descriptor releases the lease, including after a crash.
    (unsafe { crate::flock(lock.as_raw_fd(), EXCLUSIVE_NONBLOCKING) } == 0).then_some(lock)
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
    let Some(wanted) = bundle_version(&source) else {
        return false;
    };
    if std::fs::create_dir_all(&directory).is_err() {
        return false;
    }
    let Some(_lock) = install_lock(&directory) else {
        return false;
    };
    if bundle_version(&destination).is_some_and(|installed| installed >= wanted) {
        return false;
    }
    let staging = directory.join(STAGING_NAME);
    let _ = std::fs::remove_dir_all(&staging);
    let (Some(source_text), Some(staging_text), Some(destination_text)) =
        (source.to_str(), staging.to_str(), destination.to_str())
    else {
        return false;
    };
    if !run("/usr/bin/ditto", &[source_text, staging_text])
        || bundle_version(&staging) != Some(wanted)
        || !run(
            "/usr/bin/codesign",
            &["--verify", "--deep", "--strict", staging_text],
        )
    {
        let _ = std::fs::remove_dir_all(&staging);
        return false;
    }
    if publish_bundle(&staging, &destination).is_err() {
        return false;
    }
    // The running image can retain the old build until the next activation.
    let _ = run(
        "/usr/bin/pkill",
        &[
            "-f",
            "Input Methods/Agent Input Method.app/Contents/MacOS/AgentInputMethod$",
        ],
    );
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
    let directory = home.join("Library/Input Methods");
    if !directory.exists() {
        return true;
    }
    let Some(_lock) = install_lock(&directory) else {
        return false;
    };
    #[cfg(feature = "test-upload")]
    {
        let label = crate::test_upload::LAUNCH_AGENT_LABEL;
        let agent = home.join(format!("Library/LaunchAgents/{label}.plist"));
        let uid = Command::new("/usr/bin/id")
            .arg("-u")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok());
        if let Some(uid) = uid {
            let _ = run(
                "/bin/launchctl",
                &["bootout", &format!("gui/{}/{label}", uid.trim())],
            );
        }
        let _ = std::fs::remove_file(&agent);
    }
    let _ = run(
        "/usr/bin/pkill",
        &[
            "-f",
            "Input Methods/Agent Input Method.app/Contents/MacOS/AgentInputMethod",
        ],
    );
    let destination = directory.join(BUNDLE_NAME);
    let removed = !destination.exists() || std::fs::remove_dir_all(&destination).is_ok();
    if removed {
        let _ = std::fs::remove_dir_all(directory.join(STAGING_NAME));
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incomplete_staging_leaves_the_installed_bundle_and_success_preserves_the_old_copy() {
        let key = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("zhizi-install-test-{}-{key}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        let installed = root.join(BUNDLE_NAME);
        let staging = root.join("staging");
        let lease = install_lock(&root).unwrap();
        let concurrent_install_or_uninstall = install_lock(&root);
        drop(lease);
        std::fs::create_dir(&installed).unwrap();
        std::fs::write(installed.join("build"), "2573").unwrap();
        let failed = publish_bundle(&staging, &installed);
        let version_after_failure = std::fs::read_to_string(installed.join("build")).unwrap();
        std::fs::create_dir(&staging).unwrap();
        std::fs::write(staging.join("build"), "2627").unwrap();
        let published = publish_bundle(&staging, &installed);
        let current = std::fs::read_to_string(installed.join("build")).unwrap();
        let previous = std::fs::read_to_string(staging.join("build")).unwrap();
        std::fs::remove_dir_all(root).unwrap();
        assert!(failed.is_err());
        assert!(concurrent_install_or_uninstall.is_none());
        assert_eq!(version_after_failure, "2573");
        assert!(published.is_ok());
        assert_eq!(current, "2627");
        assert_eq!(previous, "2573");
    }
}
