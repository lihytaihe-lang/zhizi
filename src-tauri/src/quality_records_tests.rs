use super::*;
use serde_json::json;
use std::io::{BufReader, BufWriter};

struct Fixture(std::path::PathBuf);
impl Fixture {
    fn new() -> Self {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("quality-stream-{}-{unique}", std::process::id()));
        fs::create_dir_all(root.join("Quality")).unwrap();
        Self(root)
    }
    fn quality(&self) -> std::path::PathBuf {
        self.0.join("Quality")
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn line(id: usize) -> Vec<u8> {
    let value = json!({"id":id.to_string(),"recordedAtMs":1000,"pinyin":"abc",
        "candidates":["甲","abc"],"selectedIndex":if id%2==0 {0}else{1},
        "selectedText":if id%2==0 {"甲"}else{"abc"},"appHash":"a".repeat(64),
        "outcome":if id%2==0 {"committed_candidate"}else{"committed_raw"},
        "redoAfterCommit":id%3==0,
        "contextModel":{"enabled":true,"outcome":"composed","zhijuOutcome":"offered","futureModelField":{"x":1}},
        "futureTopLevel":{"unknown":"kept"},"segments":[{"text":"中文"}]});
    let mut raw = serde_json::to_vec(&value).unwrap();
    raw.push(b'\n');
    raw
}

#[test]
fn large_modern_log_pages_counts_and_complete_file_export() {
    let fixture = Fixture::new();
    let directory = fixture.quality();
    let mut output = BufWriter::new(File::create(directory.join("events.jsonl")).unwrap());
    for id in 0..9000 {
        let mut value: serde_json::Value = serde_json::from_slice(&line(id)).unwrap();
        value["futurePadding"] = json!("x".repeat(8192));
        serde_json::to_writer(&mut output, &value).unwrap();
        output.write_all(b"\n").unwrap();
    }
    output.flush().unwrap();
    drop(output);
    assert!(fs::metadata(directory.join("events.jsonl")).unwrap().len() > 64 * 1024 * 1024);
    let first = page(&directory, None, 50, &Filter::default()).unwrap();
    assert_eq!(first.records.len(), 50);
    assert_eq!(first.records[0].id, "8999");
    assert_eq!(first.records[49].id, "8950");
    let second = page(
        &directory,
        first.next_cursor.as_deref(),
        50,
        &Filter::default(),
    )
    .unwrap();
    assert_eq!(second.records[0].id, "8949");
    assert_eq!(second.records[49].id, "8900");
    assert_eq!(
        count(
            &directory,
            &Filter {
                raw_only: Some(true),
                ..Default::default()
            }
        )
        .unwrap(),
        4500
    );
    let stats = overview(&directory, 2000).unwrap();
    assert_eq!(stats.total_count, 9000);
    assert_eq!(stats.commits, 4500);
    assert_eq!(stats.first_choice_rate, Some(1.0));
    let exported = export_file(&directory, &fixture.0.join("export.json")).unwrap();
    assert_eq!(exported.count, 9000);
    // Deserialize one element at a time, so the verification also avoids a full array allocation.
    struct Count;
    impl<'de> serde::de::Visitor<'de> for Count {
        type Value = usize;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("record array")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<usize, A::Error> {
            let mut n = 0;
            while seq.next_element::<serde::de::IgnoredAny>()?.is_some() {
                n += 1;
            }
            Ok(n)
        }
    }
    let mut decoder =
        serde_json::Deserializer::from_reader(BufReader::new(File::open(&exported.path).unwrap()));
    assert_eq!(
        serde::de::Deserializer::deserialize_seq(&mut decoder, Count).unwrap(),
        9000
    );
    assert!(export_string(&directory).unwrap_err().contains("文件导出"));
}

#[test]
fn delete_preserves_retained_raw_json_unknown_fields_and_new_outcomes() {
    let fixture = Fixture::new();
    let directory = fixture.quality();
    let mut retained = line(2);
    retained.insert(0, b' ');
    retained.insert(retained.len() - 1, b'\r');
    fs::write(
        directory.join("events.jsonl"),
        [line(1), retained.clone(), line(3)].concat(),
    )
    .unwrap();
    let _lock = crate::acquire_quality_lock(&directory).unwrap();
    assert_eq!(
        rewrite_locked(&directory, |event| event.id != "2").unwrap(),
        2
    );
    assert_eq!(fs::read(directory.join("events.jsonl")).unwrap(), retained);
    let mut exported = Vec::new();
    assert_eq!(export_json(&directory, &mut exported).unwrap(), 1);
    let values: Vec<serde_json::Value> = serde_json::from_slice(&exported).unwrap();
    assert_eq!(values[0]["futureTopLevel"]["unknown"], "kept");
    assert_eq!(values[0]["contextModel"]["zhijuOutcome"], "offered");
    assert_eq!(values[0]["segments"][0]["text"], "中文");
}

#[test]
fn legacy_count_repair_keeps_raw_answers_and_strict_replay_validation() {
    let fixture = Fixture::new();
    let directory = fixture.quality();
    let mut value: serde_json::Value = serde_json::from_slice(&line(2)).unwrap();
    value["candidateCount"] = json!(1);
    let mut raw = serde_json::to_vec(&value).unwrap();
    raw.push(b'\n');
    fs::write(directory.join("events.jsonl"), &raw).unwrap();
    let record = records(&directory, None, false)
        .unwrap()
        .next()
        .unwrap()
        .unwrap();
    assert!(record.repaired_candidate_count);
    assert_eq!(record.event.candidate_count, 2);
    assert_eq!(record.raw, raw);
    let events = crate::replay_gate::read_quality_events(&directory).unwrap();
    assert_eq!(events[0].candidates, vec!["甲", "abc"]);
    assert_eq!(events[0].selected_text, "甲");
    assert_eq!(fs::read(directory.join("events.jsonl")).unwrap(), raw);
    value["selectedIndex"] = json!(9);
    fs::write(
        directory.join("events.jsonl"),
        serde_json::to_vec(&value).unwrap(),
    )
    .unwrap();
    assert!(
        crate::replay_gate::read_quality_events(&directory).is_err(),
        "count repair must not make an invalid selection eligible"
    );
}

#[test]
fn append_between_pages_is_not_duplicated_and_delete_invalidates_cursor() {
    let fixture = Fixture::new();
    let directory = fixture.quality();
    fs::write(
        directory.join("events.jsonl"),
        [line(0), line(1), line(2)].concat(),
    )
    .unwrap();
    let first = page(&directory, None, 1, &Filter::default()).unwrap();
    assert_eq!(first.records[0].id, "2");
    OpenOptions::new()
        .append(true)
        .open(directory.join("events.jsonl"))
        .unwrap()
        .write_all(&line(3))
        .unwrap();
    let next = page(
        &directory,
        first.next_cursor.as_deref(),
        2,
        &Filter::default(),
    )
    .unwrap();
    assert_eq!(
        next.records
            .iter()
            .map(|r| r.id.as_str())
            .collect::<Vec<_>>(),
        ["1", "0"]
    );
    let _lock = crate::acquire_quality_lock(&directory).unwrap();
    rewrite_locked(&directory, |r| r.id == "3").unwrap();
    assert!(page(
        &directory,
        first.next_cursor.as_deref(),
        1,
        &Filter::default()
    )
    .is_err());
}

#[test]
fn malformed_rewrite_keeps_source_and_failed_export_keeps_destination() {
    let fixture = Fixture::new();
    let directory = fixture.quality();
    let bytes = [line(1), b"{\"unfinished\":".to_vec()].concat();
    fs::write(directory.join("events.jsonl"), &bytes).unwrap();
    assert_eq!(
        page(&directory, None, 100, &Filter::default())
            .unwrap()
            .records
            .len(),
        1
    );
    let _lock = crate::acquire_quality_lock(&directory).unwrap();
    assert!(rewrite_locked(&directory, |_| true).is_err());
    assert_eq!(fs::read(directory.join("events.jsonl")).unwrap(), bytes);
    fs::write(directory.join("events.jsonl"), b"invalid\n").unwrap();
    let destination = fixture.0.join("existing.json");
    fs::write(&destination, b"keep").unwrap();
    assert!(export_file(&directory, &destination).is_err());
    assert_eq!(fs::read(destination).unwrap(), b"keep");
    assert!(fs::read_dir(&fixture.0).unwrap().all(|entry| !entry
        .unwrap()
        .file_name()
        .to_string_lossy()
        .contains(".quality-export")));
}

#[test]
fn shared_native_append_lock_preserves_an_append_after_replacement() {
    let fixture = Fixture::new();
    let directory = fixture.quality();
    fs::write(directory.join("events.jsonl"), [line(1), line(2)].concat()).unwrap();
    let guard = crate::acquire_quality_lock(&directory).unwrap();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let (locked_tx, locked_rx) = std::sync::mpsc::channel();
    let append_directory = directory.clone();
    let writer = std::thread::spawn(move || {
        ready_tx.send(()).unwrap();
        let _guard = crate::acquire_quality_lock(&append_directory).unwrap();
        locked_tx.send(()).unwrap();
        OpenOptions::new()
            .append(true)
            .open(append_directory.join("events.jsonl"))
            .unwrap()
            .write_all(&line(3))
            .unwrap();
    });
    ready_rx.recv().unwrap();
    assert!(locked_rx
        .recv_timeout(std::time::Duration::from_millis(50))
        .is_err());
    assert_eq!(
        rewrite_locked(&directory, |event| event.id == "1").unwrap(),
        1
    );
    drop(guard);
    writer.join().unwrap();
    assert_eq!(
        fs::read(directory.join("events.jsonl")).unwrap(),
        [line(2), line(3)].concat()
    );
}

#[cfg(unix)]
#[test]
fn links_and_export_over_source_are_rejected() {
    let fixture = Fixture::new();
    let directory = fixture.quality();
    let original = fixture.0.join("source");
    fs::write(&original, line(1)).unwrap();
    std::os::unix::fs::symlink(&original, directory.join("events.jsonl")).unwrap();
    assert!(page(&directory, None, 100, &Filter::default()).is_err());
    fs::remove_file(directory.join("events.jsonl")).unwrap();
    fs::write(directory.join("events.jsonl"), line(1)).unwrap();
    assert!(export_file(&directory, &directory.join("events.jsonl")).is_err());
    assert_eq!(fs::read(directory.join("events.jsonl")).unwrap(), line(1));
}
