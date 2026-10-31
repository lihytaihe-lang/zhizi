// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    #[cfg(feature = "test-upload")]
    if std::env::args().any(|argument| argument == agent_input_layer_lib::test_upload::FLAG) {
        std::process::exit(agent_input_layer_lib::test_upload::run_headless());
    }
    // The stale hourly LaunchAgent a test build left behind wakes the public build
    // with `--upload-test-logs`; retire that task and leave without opening the console.
    #[cfg(not(feature = "test-upload"))]
    if agent_input_layer_lib::legacy_upload_task::is_legacy_upload_invocation(
        &std::env::args().collect::<Vec<String>>(),
    ) {
        #[cfg(target_os = "macos")]
        if let agent_input_layer_lib::legacy_upload_task::RetireOutcome::Failed(error) =
            agent_input_layer_lib::legacy_upload_task::retire_from_environment()
        {
            eprintln!("retiring the stale hourly upload agent failed: {error}");
        }
        std::process::exit(0);
    }
    agent_input_layer_lib::run()
}
