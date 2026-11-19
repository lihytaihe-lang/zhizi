//! Explicit learning-store primitives, using only a caller-prepared empty profile.
use agent_ime_core::zhixu::learning::{self, LearningStore};
use std::path::PathBuf;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 2 {
        return Err("usage: learning <absolute-empty-private-profile-dir>".into());
    }
    let directory = PathBuf::from(&args[1]);
    if !directory.is_absolute()
        || !directory.is_dir()
        || std::fs::read_dir(&directory)?.next().is_some()
    {
        return Err(
            "prepare an absolute empty private directory for this explicit write demonstration"
                .into(),
        );
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis() as u64;
    let mut store = LearningStore::load(&directory, now);
    store.record("ni hao", "你好", now);
    store.save(&directory)?;
    let backup = learning::management_export(&directory)?;
    let before = learning::management_list(&directory)?;
    let key = before.items[0].key.clone();
    let removed = learning::management_delete(&directory, &[key])?;
    assert!(learning::management_list(&directory)?.items.is_empty());
    learning::management_restore(&directory, &removed)?;
    learning::management_import(&directory, &backup)?;
    learning::management_import(&directory, &backup)?;
    let after = learning::management_list(&directory)?;
    assert_eq!(after.items[0].entries[0].count, 1);
    println!(
        "learning round trip: words={}, choices={}, estimate={} bytes; duplicate import did not add counts",
        after.status.entries, after.status.choices, after.status.estimated_bytes
    );
    // The JSON is personal data. The example intentionally does not print it.
    Ok(())
}
