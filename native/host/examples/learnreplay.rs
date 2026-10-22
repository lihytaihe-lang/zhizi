//! Learning replay (local only): the owner's cases in time order, each typed, scored
//! (first candidate and first page) and then committed the way the owner did, so what
//! Rime learns shapes the later cases. Use a fresh user directory per policy.
//!
//!   cargo run --release --example learnreplay -- <shared> <user dir> <cases.jsonl> <out.jsonl>
//! with ZHIZI_ORT / ZHIZI_MODEL / ZHIZI_ITOS as for `eval`; ZHIZI_SINGLE_LEARN picks the policy.
use std::io::Write;
use zhizi_host::{Host, HostConfig, Key};

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let (shared, user, cases, out) = (&arguments[0], &arguments[1], &arguments[2], &arguments[3]);
    let mut host = Host::open(&HostConfig { shared_dir: shared.into(), user_dir: user.into(), app_id: "zhizi.learnreplay".into() })
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
    let (mut total, mut first_hits, mut page_hits) = (0usize, 0usize, 0usize);
    for line in std::fs::read_to_string(cases).unwrap().lines() {
        let Ok(case) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        let keys = case["keys"].as_str().unwrap_or_default();
        let target = case["target"].as_str().unwrap_or_default();
        if keys.is_empty() || target.is_empty() {
            continue;
        }
        host.clear_context();
        host.note_committed(case["context"].as_str().unwrap_or_default());
        let mut view = Default::default();
        for c in keys.chars() {
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
        let page: Vec<String> = view.candidates.iter().take(5).map(|c| c.text.clone()).collect();
        let first = page.first().is_some_and(|text| text == target);
        let position = page.iter().position(|text| text == target);
        // The owner's pick: space for the first, a digit for another candidate on the page.
        let after = match position {
            Some(0) => host.process(Key::Space, 0, false),
            Some(index) => host.process(Key::Text((index + 1).to_string()), 0, false),
            None => host.process(Key::Escape, 0, false),
        };
        // A pick that covered only part of the letters (有人 for yourenz) leaves the rest
        // composing: drop it so the next case starts clean.
        if !after.preedit.is_empty() {
            host.process(Key::Escape, 0, false);
        }
        total += 1;
        first_hits += usize::from(first);
        page_hits += usize::from(position.is_some());
        let record = serde_json::json!({
            "id": case["id"], "keys": keys, "target": target, "first": first, "onPage": position.is_some(), "page": page,
        });
        writeln!(output, "{record}").unwrap();
    }
    println!("cases {total} top1 {:.1}% top5 {:.1}%", 100.0 * first_hits as f64 / total as f64, 100.0 * page_hits as f64 / total as f64);
}
