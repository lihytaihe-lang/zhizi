//! Cleanup for the hourly LaunchAgent that friends' test builds installed:
//! `~/Library/LaunchAgents/com.agentinputlayer.zhizi.test-upload.plist` starts this
//! binary headless with `--upload-test-logs` every hour. The open-source release is
//! built without the `test-upload` feature and knows nothing of that flag, so without
//! this cleanup the stale task would open the full management console in the
//! background every hour, forever. The public build boots the agent out and moves
//! the plist into `~/Library/Application Support/AgentInputMethod/DevelopmentInstalls/`
//! (it never deletes it). A plist whose `ProgramArguments` points at some other
//! program is never touched.

use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// Same value as `test_upload::LAUNCH_AGENT_LABEL`; that constant sits behind the
/// `test-upload` feature, which the public build does not carry.
pub const LABEL: &str = "com.agentinputlayer.zhizi.test-upload";
const FLAG: &str = "--upload-test-logs";

#[derive(Debug, PartialEq, Eq)]
pub enum RetireOutcome {
    /// No plist at the expected path; nothing to do.
    NotPresent,
    /// The plist exists but starts some other program; we leave it alone.
    NotOurs,
    /// The agent was booted out (best effort) and the plist moved to the backup directory.
    Retired,
    /// The plist is ours but the backup failed; it is still in place.
    Failed(String),
}

/// True when the arguments look like the stale hourly task waking us up.
pub fn is_legacy_upload_invocation(arguments: &[String]) -> bool {
    arguments.iter().any(|argument| argument == FLAG)
}

/// Parses plist text and returns true only when the first `ProgramArguments` element
/// equals `executable` exactly. No plist dependency: take the first `<string>…</string>`
/// after `<key>ProgramArguments</key>` and XML-unescape its text.
fn plist_points_to(plist_text: &str, executable: &Path) -> bool {
    let Some(key) = plist_text.find("<key>ProgramArguments</key>") else {
        return false;
    };
    let rest = &plist_text[key..];
    let Some(open) = rest.find("<string>") else {
        return false;
    };
    let inner = &rest[open + "<string>".len()..];
    let Some(close) = inner.find("</string>") else {
        return false;
    };
    Path::new(&xml_unescape(&inner[..close])) == executable
}

/// Inverse of `test_upload::xml_escape` for the three characters it escapes;
/// `&amp;` goes last so `&amp;lt;` unescapes to `&lt;`, not `<`.
fn xml_unescape(value: &str) -> String {
    value
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// When the stale LaunchAgent's plist exists and points at `executable`: move the plist to the
/// backup directory under `home`, then boot the agent out. The runner's result is ignored — once the
/// plist has moved, launchd cannot load the task again, which is the part that matters.
pub fn retire_if_present(
    home: &Path,
    executable: &Path,
    uid: u32,
    runner: &dyn Fn(&str, &[&str]) -> bool,
) -> RetireOutcome {
    let plist_path = home.join(format!("Library/LaunchAgents/{LABEL}.plist"));
    let Ok(plist_text) = fs::read_to_string(&plist_path) else {
        return RetireOutcome::NotPresent;
    };
    if !plist_points_to(&plist_text, executable) {
        return RetireOutcome::NotOurs;
    }
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let backup_directory =
        home.join("Library/Application Support/AgentInputMethod/DevelopmentInstalls");
    if let Err(error) = fs::create_dir_all(&backup_directory) {
        return RetireOutcome::Failed(error.to_string());
    }
    let backup_path = backup_directory.join(format!("legacy-upload-agent-retired-{seconds}.plist"));
    // The plist moves FIRST: when launchd started this very process from that plist, `bootout` ends it
    // (launchd stops the job it boots out), so anything after the bootout may never run.
    let moved = fs::rename(&plist_path, &backup_path).is_ok()
        // rename can refuse to cross filesystems; fall back to copy and delete.
        || fs::copy(&plist_path, &backup_path).and_then(|_| fs::remove_file(&plist_path)).is_ok();
    if !moved {
        return RetireOutcome::Failed("could not move the plist to the backup directory".to_string());
    }
    runner("/bin/launchctl", &["bootout", &format!("gui/{uid}/{LABEL}")]);
    RetireOutcome::Retired
}

/// Retires the stale task with this process's real home, executable and uid.
/// Callers treat failures as non-fatal: a leftover agent is annoying, a broken
/// startup is worse.
#[cfg(target_os = "macos")]
pub fn retire_from_environment() -> RetireOutcome {
    use std::path::PathBuf;
    let Some(home) = std::env::var_os("HOME") else {
        return RetireOutcome::Failed("HOME is not set".to_string());
    };
    let Ok(executable) = std::env::current_exe() else {
        return RetireOutcome::Failed("current_exe failed".to_string());
    };
    retire_if_present(
        &PathBuf::from(home),
        &executable,
        unsafe { getuid() },
        &|program, arguments| {
            std::process::Command::new(program)
                .args(arguments)
                .output()
                .is_ok_and(|output| output.status.success())
        },
    )
}

#[cfg(target_os = "macos")]
unsafe fn getuid() -> u32 {
    unsafe extern "C" {
        fn getuid() -> u32;
    }
    unsafe { getuid() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::path::PathBuf;

    /// A throwaway HOME with `Library/LaunchAgents` already in place; the real
    /// `~/Library` is never touched.
    fn temp_home(name: &str) -> PathBuf {
        let home = std::env::temp_dir().join(format!("aim-legacy-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        fs::create_dir_all(home.join("Library/LaunchAgents")).unwrap();
        home
    }

    /// The plist `test_upload::ensure_scheduled` writes, with the given first
    /// `ProgramArguments` element already XML-escaped by the caller when needed.
    fn plist_text(first_argument: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{LABEL}</string>
  <key>ProgramArguments</key>
  <array><string>{first_argument}</string><string>{FLAG}</string></array>
  <key>StartInterval</key><integer>3600</integer>
  <key>RunAtLoad</key><true/>
</dict>
</plist>
"#
        )
    }

    fn write_plist(home: &Path, first_argument: &str) {
        fs::write(
            home.join(format!("Library/LaunchAgents/{LABEL}.plist")),
            plist_text(first_argument),
        )
        .unwrap();
    }

    /// The single file backed up under `DevelopmentInstalls`, by name.
    fn backup_names(home: &Path) -> Vec<String> {
        fs::read_dir(home.join("Library/Application Support/AgentInputMethod/DevelopmentInstalls"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect()
    }

    fn recording_runner() -> (
        impl Fn(&str, &[&str]) -> bool,
        std::rc::Rc<RefCell<Vec<(String, Vec<String>)>>>,
    ) {
        let calls = std::rc::Rc::new(RefCell::new(Vec::new()));
        let sink = std::rc::Rc::clone(&calls);
        let runner = move |program: &str, arguments: &[&str]| {
            sink.borrow_mut().push((
                program.to_string(),
                arguments.iter().map(|argument| argument.to_string()).collect(),
            ));
            true
        };
        (runner, calls)
    }

    #[test]
    fn plist_is_already_gone_when_bootout_runs() {
        // launchd ends the process it boots out, so the move has to happen before the bootout.
        let home = temp_home("order");
        let executable = home.join("current/agent-input-layer");
        write_plist(&home, &executable.to_string_lossy());
        let plist = home.join(format!("Library/LaunchAgents/{LABEL}.plist"));
        let seen_at_bootout = std::cell::Cell::new(None);
        let runner = |_program: &str, _arguments: &[&str]| {
            seen_at_bootout.set(Some(plist.exists()));
            true
        };
        assert_eq!(retire_if_present(&home, &executable, 501, &runner), RetireOutcome::Retired);
        assert_eq!(seen_at_bootout.get(), Some(false), "the plist must already be moved when bootout runs");
    }

    #[test]
    fn retires_plist_pointing_at_this_binary() {
        let home = temp_home("retire");
        let executable = home.join("current/agent-input-layer");
        write_plist(&home, &executable.to_string_lossy());

        let (runner, calls) = recording_runner();
        assert_eq!(
            retire_if_present(&home, &executable, 501, &runner),
            RetireOutcome::Retired
        );
        assert_eq!(
            calls.borrow().as_slice(),
            [(
                "/bin/launchctl".to_string(),
                vec![
                    "bootout".to_string(),
                    "gui/501/com.agentinputlayer.zhizi.test-upload".to_string()
                ]
            )]
        );
        assert_eq!(
            backup_names(&home),
            vec![format!(
                "legacy-upload-agent-retired-{}.plist",
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_secs()
            )]
        );
        assert!(!home.join(format!("Library/LaunchAgents/{LABEL}.plist")).exists());
        let backup_text = fs::read_dir(
            home.join("Library/Application Support/AgentInputMethod/DevelopmentInstalls"),
        )
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
        assert_eq!(fs::read_to_string(backup_text).unwrap(), plist_text(&executable.to_string_lossy()));
        fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn leaves_foreign_plist_alone() {
        let home = temp_home("foreign");
        let executable = home.join("current/agent-input-layer");
        let foreign_text = plist_text("/somewhere/else/entirely");
        fs::write(
            home.join(format!("Library/LaunchAgents/{LABEL}.plist")),
            &foreign_text,
        )
        .unwrap();

        let (runner, calls) = recording_runner();
        assert_eq!(
            retire_if_present(&home, &executable, 501, &runner),
            RetireOutcome::NotOurs
        );
        assert!(calls.borrow().is_empty(), "bootout must not run for a foreign plist");
        assert_eq!(
            fs::read_to_string(home.join(format!("Library/LaunchAgents/{LABEL}.plist"))).unwrap(),
            foreign_text
        );
        assert!(backup_dir_absent(&home));
        fs::remove_dir_all(&home).unwrap();
    }

    fn backup_dir_absent(home: &Path) -> bool {
        !home
            .join("Library/Application Support/AgentInputMethod/DevelopmentInstalls")
            .exists()
    }

    #[test]
    fn nothing_to_do_without_plist() {
        let home = temp_home("absent");
        let (runner, calls) = recording_runner();
        assert_eq!(
            retire_if_present(&home, &home.join("current/agent-input-layer"), 501, &runner),
            RetireOutcome::NotPresent
        );
        assert!(calls.borrow().is_empty());
        fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn moves_plist_even_when_bootout_fails() {
        let home = temp_home("bootout-fails");
        let executable = home.join("current/agent-input-layer");
        write_plist(&home, &executable.to_string_lossy());

        let runner = |_program: &str, _arguments: &[&str]| false;
        assert_eq!(
            retire_if_present(&home, &executable, 501, &runner),
            RetireOutcome::Retired
        );
        assert!(!home.join(format!("Library/LaunchAgents/{LABEL}.plist")).exists());
        assert_eq!(backup_names(&home).len(), 1);
        fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn reports_failure_when_backup_directory_cannot_be_created() {
        let home = temp_home("blocked");
        let executable = home.join("current/agent-input-layer");
        write_plist(&home, &executable.to_string_lossy());
        // A file squatting on the backup directory path makes create_dir_all fail.
        let squatter = home.join("Library/Application Support/AgentInputMethod/DevelopmentInstalls");
        fs::create_dir_all(squatter.parent().unwrap()).unwrap();
        fs::write(&squatter, b"").unwrap();

        let (runner, _calls) = recording_runner();
        assert!(matches!(
            retire_if_present(&home, &executable, 501, &runner),
            RetireOutcome::Failed(_)
        ));
        assert!(home.join(format!("Library/LaunchAgents/{LABEL}.plist")).exists());
        fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn matches_xml_escaped_paths() {
        let home = temp_home("escaped");
        let executable = home.join("Apps/A & B/agent-input-layer");
        let escaped_executable = executable.to_string_lossy().replace('&', "&amp;");
        write_plist(&home, &escaped_executable);

        let (runner, _calls) = recording_runner();
        assert_eq!(
            retire_if_present(&home, &executable, 501, &runner),
            RetireOutcome::Retired
        );
        assert_eq!(backup_names(&home).len(), 1);
        fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn recognises_legacy_invocations() {
        let arguments = |slice: &[&str]| -> Vec<String> {
            slice.iter().map(|argument| argument.to_string()).collect()
        };
        assert!(is_legacy_upload_invocation(&arguments(&[
            "agent-input-layer",
            "--upload-test-logs"
        ])));
        assert!(!is_legacy_upload_invocation(&arguments(&["agent-input-layer"])));
        assert!(!is_legacy_upload_invocation(&[]));
    }
}
