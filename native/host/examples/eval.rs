//! Bucketed evaluation (local): types every case through the host and reports, per
//! bucket, how often the target is the first candidate and how often it is on the first
//! page. Cases are JSON lines `{"id","bucket","context","keys","target"}` (see
//! scripts/eval/build_external_set.py and the owner set built locally).
//!
//!   cargo run --release --example eval -- <shared> <fresh user dir> <cases.jsonl> <out.jsonl>
//! with ZHIZI_ORT / ZHIZI_MODEL / ZHIZI_ITOS for 顺手·知字 (omit them to measure the
//! decoder alone), and ZHIZI_OWNER_RECORDS for the owner's settled choices. Nothing is
//! committed, so the user dictionary learns nothing.
use std::collections::BTreeMap;
use std::io::Write;
use zhizi_host::{Host, HostConfig, Key};

#[derive(Default)]
struct Tally {
    cases: usize,
    first: usize,
    page: usize,
}

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let (shared, user, cases, out) = (&arguments[0], &arguments[1], &arguments[2], &arguments[3]);
    let mut host = Host::open(&HostConfig { shared_dir: shared.into(), user_dir: user.into(), app_id: "zhizi.eval".into() })
        .expect("open");
    let model = std::env::var("ZHIZI_MODEL").is_ok();
    #[cfg(feature = "model")]
    if model {
        zhizi_host::ranking::onnx::init_runtime(std::env::var("ZHIZI_ORT").unwrap().as_ref()).unwrap();
        let scorer = zhizi_host::ranking::onnx::OnnxScorer::load(
            std::env::var("ZHIZI_MODEL").unwrap().as_ref(),
            std::env::var("ZHIZI_ITOS").unwrap().as_ref(),
            4,
        )
        .unwrap();
        host.enable_model(Box::new(scorer), None);
    }
    // Candidate export for training data: the model's masks exist only while a model is enabled, and
    // which candidates it would score does not depend on its scores, so a scorer that rates everything
    // alike is enough (ZHIZI_EVAL_DUMMY_SCORER).
    if !model && std::env::var("ZHIZI_EVAL_DUMMY_SCORER").is_ok() {
        struct Flat;
        impl zhizi_host::ranking::Scorer for Flat {
            fn score(&mut self, _context: &str, candidates: &[String]) -> Result<Vec<f64>, String> {
                Ok(vec![0.0; candidates.len()])
            }
        }
        host.enable_model(Box::new(Flat), None);
    }
    // The owner's settled choices (OwnerStyle), as the macOS shell loads them at start.
    if let Ok(records) = std::env::var("ZHIZI_OWNER_RECORDS") {
        host.set_owner_style(&std::fs::read_to_string(records).unwrap());
    }
    let trace = std::env::var("ZHIZI_EVAL_TRACE").is_ok();
    let wait_ticks: usize = std::env::var("ZHIZI_EVAL_WAIT_TICKS").ok().and_then(|value| value.parse().ok()).unwrap_or(40);
    let mut output = std::fs::File::create(out).unwrap();
    let mut tallies: BTreeMap<String, Tally> = BTreeMap::new();
    for line in std::fs::read_to_string(cases).unwrap().lines() {
        let Ok(case) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        let keys = case["keys"].as_str().unwrap_or_default();
        let target = case["target"].as_str().unwrap_or_default();
        let bucket = case["bucket"].as_str().unwrap_or("all").to_string();
        if keys.is_empty() || target.is_empty() {
            continue;
        }
        host.clear_context();
        host.note_committed(case["context"].as_str().unwrap_or_default());
        let mut view = Default::default();
        for c in keys.chars() {
            view = host.process(Key::Text(c.to_string()), 0, false);
        }
        // Accuracy, not latency: a busy machine must not turn a slow model into a different answer.
        for _ in 0..wait_ticks {
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
        let on_page = page.iter().any(|text| text == target);
        let outcome = host.model_observation().outcome;
        // ZHIZI_EVAL_TRACE (local diagnosis): the whole-reading compositions the core
        // generated for this input and whether the target is on the next three pages.
        let trace = trace.then(|| {
            let alternatives = host.composition_alternatives();
            // The model's totals for [first] + alternatives, when it scored this input.
            let scored = host.last_composition();
            let totals: Vec<f64> = if scored.texts.len() == alternatives.len() + 1 && scored.texts[1..] == alternatives[..] {
                (0..scored.scores.len())
                    .map(|i| scored.scores[i] - zhizi_host::ranking::edit_penalty() * f64::from(scored.edits.get(i).copied().unwrap_or(0)))
                    .collect()
            } else {
                Vec::new()
            };
            // Read the masks before anything moves the page or clears the composition.
            let slots: Vec<String> = host
                .scored_page_slots()
                .into_iter()
                .filter_map(|i| view.candidates.get(i).map(|c| c.text.clone()))
                .collect();
            let deeper = host.peek_following_pages(3).iter().any(|page| page.text.split('\t').any(|text| text == target));
            (alternatives, deeper, totals, slots)
        });
        host.process(Key::Escape, 0, false);
        let tally = tallies.entry(bucket.clone()).or_default();
        tally.cases += 1;
        tally.first += usize::from(first);
        tally.page += usize::from(on_page);
        let mut record = serde_json::json!({
            "id": case["id"], "bucket": bucket, "keys": keys, "target": target, "context": case["context"],
            "first": first, "onPage": on_page, "page": page, "model": outcome,
        });
        if let Some((alternatives, deeper, totals, slots)) = trace {
            record["alternatives"] = serde_json::json!(alternatives);
            // What the model scores: the page candidates in its masks, and the alternatives.
            record["scored"] = serde_json::json!(slots);
            record["totals"] = serde_json::json!(totals);
            record["onNextPages"] = serde_json::json!(deeper);
        }
        writeln!(output, "{record}").unwrap();
    }
    let mut all = Tally::default();
    println!("bucket\tcases\ttop1\ttop5\tmodel={model}");
    for (bucket, tally) in &tallies {
        all.cases += tally.cases;
        all.first += tally.first;
        all.page += tally.page;
        println!("{bucket}\t{}\t{}", tally.cases, percentages(tally));
    }
    println!("all\t{}\t{}", all.cases, percentages(&all));
}

fn percentages(tally: &Tally) -> String {
    let share = |count: usize| 100.0 * count as f64 / tally.cases.max(1) as f64;
    format!("{:.1}%\t{:.1}%", share(tally.first), share(tally.page))
}
