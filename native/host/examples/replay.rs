//! Owner replay (local only): types every recorded composition through the host with
//! 顺手·知字 and prints, per input, the first candidate the owner saw, the one this
//! build shows, and what the owner committed. Aggregates only go to stdout by default.
//!
//!   cargo run --release --example replay -- <shared> <user copy> <events.jsonl> <out.tsv>
//! with ZHIZI_ORT / ZHIZI_MODEL / ZHIZI_ITOS as for `type`.
use std::io::Write;
use zhizi_host::{Host, HostConfig, Key};

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let (shared, user, events, out) = (&arguments[0], &arguments[1], &arguments[2], &arguments[3]);
    let mut host = Host::open(&HostConfig { shared_dir: shared.into(), user_dir: user.into(), app_id: "zhizi.replay".into() })
        .expect("open");
    zhizi_host::ranking::onnx::init_runtime(std::env::var("ZHIZI_ORT").unwrap().as_ref()).unwrap();
    let scorer = zhizi_host::ranking::onnx::OnnxScorer::load(
        std::env::var("ZHIZI_MODEL").unwrap().as_ref(),
        std::env::var("ZHIZI_ITOS").unwrap().as_ref(),
        4,
    )
    .unwrap();
    host.enable_model(Box::new(scorer), None);
    let mut output = std::fs::File::create(out).unwrap();
    let mut seen = std::collections::HashSet::new();
    for line in std::fs::read_to_string(events).unwrap().lines() {
        let Ok(event) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        let raw = event["pinyin"].as_str().unwrap_or_default();
        let chosen = event["selectedText"].as_str().unwrap_or_default();
        let context = event["precedingText"].as_str().unwrap_or_default();
        if raw.len() < 4 || chosen.is_empty() || !raw.bytes().all(|b| b.is_ascii_lowercase()) {
            continue;
        }
        if !seen.insert((raw.to_string(), context.to_string(), chosen.to_string())) {
            continue;
        }
        let shown = event["candidates"].get(0).and_then(|v| v.as_str()).unwrap_or_default();
        host.clear_context();
        host.note_committed(context);
        let mut view = Default::default();
        for c in raw.chars() {
            view = host.process(Key::Text(c.to_string()), 0, false);
        }
        for _ in 0..40 {
            if host.model_observation().outcome != "pending" {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
            if let Some(late) = host.poll_ranking() {
                view = late;
            }
        }
        let view: zhizi_host::View = view;
        // A composition trace belongs to this input only.
        let now = view.candidates.first().map(|c| c.text.clone()).unwrap_or_default();
        let outcome = host.model_observation().outcome;
        let trace = host.last_composition().clone();
        let page: Vec<String> = view.candidates.iter().take(5).map(|c| format!("{}|{}", c.text, c.comment)).collect();
        host.process(Key::Escape, 0, false);
        let record = serde_json::json!({
            "raw": raw, "context": context, "shown": shown, "now": now, "chosen": chosen, "outcome": outcome,
            "page": page, "texts": trace.texts, "edits": trace.edits, "scores": trace.scores,
        });
        writeln!(output, "{record}").unwrap();
    }
}
