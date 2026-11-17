#[path = "../quality_log.rs"]
#[allow(dead_code)]
mod quality_log;
#[path = "../replay_gate.rs"]
#[allow(dead_code)]
mod replay_gate;
#[path = "../snapshot.rs"]
#[allow(dead_code)]
mod snapshot;

use snapshot::SnapshotStore;
use std::{
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

fn main() {
    let arguments = std::env::args().collect::<Vec<_>>();
    if arguments.get(1).map(String::as_str) == Some("quality-readonly") {
        if arguments.len() != 3 { eprintln!("usage: snapshot-fixture-ctl quality-readonly <quality-root>"); std::process::exit(2); }
        let directory = std::path::Path::new(&arguments[2]);
        let mut total = 0usize;
        let mut composed = 0usize;
        let mut repaired_candidate_count = 0usize;
        let mut issues = std::collections::BTreeMap::<&str,usize>::new();
        let read = quality_log::records(directory, None, false).and_then(|records| {
            for record in records {
                let record = record?;
                repaired_candidate_count += usize::from(record.repaired_candidate_count);
                let event = record.event;
                total += 1;
                if let Some(reason) = replay_gate::event_validation_issue(&event) { *issues.entry(reason).or_default() += 1; }
                composed += usize::from(event.context_model.is_some_and(|observation|
                    observation.outcome == replay_gate::ContextModelOutcome::Composed));
            }
            Ok(())
        });
        let validation = replay_gate::stream_quality_events(directory)
            .and_then(|mut records| records.try_fold(0usize, |count, record| record.map(|_| count + 1)));
        let profile = agent_input_layer_lib::quality_profile_readonly_summary(directory);
        println!("{}", serde_json::json!({"readOnly":true,"recordCount":total,
            "composedCount":composed,"legacyCandidateCountRepaired":repaired_candidate_count,"validationIssueCounts":issues,"readPassed":read.is_ok(),"readError":read.err(),
            "replayValidationPassed":validation.is_ok(),"replayValidationError":validation.err(),
            "profilePassed":profile.is_ok(),"profile":profile.as_ref().ok(),"profileError":profile.err()}));
        return;
    }
    if arguments.get(1).map(String::as_str) == Some("personal-replay-empty") {
        run_personal_replay_empty(&arguments);
        return;
    }
    if arguments.get(1).map(String::as_str) == Some("personal-replay-refreeze-empty") {
        run_personal_replay_refreeze_empty(&arguments);
        return;
    }
    if arguments.len() != 3 {
        eprintln!("usage: snapshot-fixture-ctl <snapshot-root> <version>\n       snapshot-fixture-ctl personal-replay-empty <quality-root> <helper> <rime-resources> <user-rime>\n       snapshot-fixture-ctl personal-replay-refreeze-empty <quality-root> <owner-approved-reason>");
        std::process::exit(2);
    }
    let version = arguments[2].parse::<u64>().unwrap_or_else(|_| {
        eprintln!("invalid version");
        std::process::exit(2);
    });
    let store = SnapshotStore::new(PathBuf::from(&arguments[1]));
    let result = store.publish_fixture(version).unwrap_or_else(|error| {
        eprintln!("{error}");
        std::process::exit(3);
    });
    println!(
        "version={} checksum={} file={} entries={}",
        result.version, result.checksum, result.file_name, result.entry_count
    );
}

fn run_personal_replay_refreeze_empty(arguments: &[String]) {
    if arguments.len() != 4 {
        eprintln!("usage: snapshot-fixture-ctl personal-replay-refreeze-empty <quality-root> <owner-approved-reason>");
        std::process::exit(2);
    }
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or_default();
    let quality_root = std::path::Path::new(&arguments[2]);
    let events = replay_gate::read_quality_events(quality_root).unwrap_or_else(|error| {
        eprintln!("{error}");
        std::process::exit(3);
    });
    let frozen =
        replay_gate::refreeze_empty_golden_set_once(quality_root, &events, now_ms, &arguments[3])
            .unwrap_or_else(|error| {
                eprintln!("{error}");
                std::process::exit(3);
            });
    println!(
        "batch={} samples={} checksum={} frozen_at_ms={}",
        frozen.batch_id, frozen.sample_count, frozen.checksum, frozen.frozen_at_ms
    );
}

fn run_personal_replay_empty(arguments: &[String]) {
    if arguments.len() != 6 {
        eprintln!("usage: snapshot-fixture-ctl personal-replay-empty <quality-root> <helper> <rime-resources> <user-rime>");
        std::process::exit(2);
    }
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or_default();
    let lexicon: Vec<serde_json::Value> = Vec::new();
    let helper = std::path::Path::new(&arguments[3]);
    let provenance = replay_gate::artifact_provenance(helper).unwrap_or_else(|error| {
        eprintln!("{error}");
        std::process::exit(3);
    });
    let report = replay_gate::run_replay_gate(replay_gate::ReplayRunInput {
        quality_root: std::path::Path::new(&arguments[2]),
        helper,
        rime_resources: std::path::Path::new(&arguments[4]),
        user_rime: std::path::Path::new(&arguments[5]),
        artifact_checksum: &provenance.artifact_checksum,
        product_commit: &provenance.product_commit,
        product_inputs_sha256: &provenance.product_inputs_sha256,
        lexicon_entries: &lexicon,
        now_ms,
    })
    .unwrap_or_else(|error| {
        eprintln!("{error}");
        std::process::exit(3);
    });
    println!("{}", serde_json::to_string_pretty(&report).unwrap());
    std::process::exit(match report.status.as_str() {
        "passed" => 0,
        "insufficient_data" => 4,
        "failed" => 5,
        _ => 3,
    });
}
