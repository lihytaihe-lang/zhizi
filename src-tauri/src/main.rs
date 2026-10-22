// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    #[cfg(feature = "test-upload")]
    if std::env::args().any(|argument| argument == agent_input_layer_lib::test_upload::FLAG) {
        std::process::exit(agent_input_layer_lib::test_upload::run_headless());
    }
    agent_input_layer_lib::run()
}
