//! Real model + Host acceptance, using a new synthetic user directory.
//! product_acceptance <release-dir> <blank-user> <cases.json> <new-output.json>
use std::{path::Path, time::{Duration, Instant}};
use serde::Deserialize;
use zhizi_host::{Host, HostConfig, Key, View};

#[derive(Deserialize)]
struct Case {
    id: String, raw: String, context: String, target: String,
    #[serde(default = "yes")] context_ranking: bool,
    #[serde(default = "yes")] zhiju: bool,
    #[serde(default = "yes")] typo_correction: bool,
    #[serde(default)] inter_key_ms: u64,
}
fn yes() -> bool { true }
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if !matches!(args.len(), 4 | 5) || args.get(4).is_some_and(|flag| flag != "--defer-ranking") {
        return Err("usage: product_acceptance release-dir blank-user cases.json new-output.json [--defer-ranking]".into());
    }
    let root = Path::new(&args[0]);
    let user = Path::new(&args[1]);
    if user.exists() && std::fs::read_dir(user)?.next().is_some() { return Err("requires a blank synthetic user directory".into()); }
    std::fs::create_dir_all(user)?;
    let mut host = Host::open(&HostConfig { shared_dir: root.join("Rime"), user_dir: user.into(), app_id: "zhizi.product-acceptance".into() })?;
    zhizi_host::ranking::onnx::init_runtime(&root.join(if cfg!(windows) { "onnxruntime.dll" } else { "libonnxruntime.so" }))?;
    let scorer = zhizi_host::ranking::onnx::OnnxScorer::load(&root.join("Model/zhizi30m_v2.onnx"), &root.join("Model/itos.json"), 4)?;
    if args.len() == 5 {
        host.enable_deferred_model(Box::new(scorer), Box::new(|| {}));
    } else {
        host.enable_model(Box::new(scorer), None);
    }
    let model = zhizi_host::zhiju::onnx::OnnxDistribution::load(&root.join("ZhijuModel/zhiju30m_v3.onnx"), 4)?;
    host.enable_zhiju(&root.join("ZhijuModel/itos.json"), Box::new(model), None)?;
    let cases: Vec<Case> = serde_json::from_slice(&std::fs::read(&args[2])?)?;
    let mut records = Vec::new();
    for case in cases {
        host.process(Key::Escape, 0, false);
        host.clear_context();
        host.set_context_ranking_enabled(case.context_ranking);
        host.set_zhiju_enabled(case.zhiju);
        host.set_typo_correction(case.typo_correction);
        host.set_context(&case.context);
        let mut view = View::default();
        let mut key_ms = Vec::new();
        for character in case.raw.chars() {
            let started = Instant::now();
            view = host.process(Key::Text(character.to_string()), 0, false);
            key_ms.push(started.elapsed().as_secs_f64()*1000.0);
            let until = Instant::now()+Duration::from_millis(case.inter_key_ms);
            while Instant::now()<until {
                if let Some(next) = host.poll_ranking() { view = next; }
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        let deadline = Instant::now()+Duration::from_millis(500);
        let mut updated = Instant::now();
        while Instant::now() < deadline {
            if let Some(next) = host.poll_ranking() { view = next; updated = Instant::now(); }
            if host.model_observation().outcome != "pending" && host.generation_observation().outcome != "pending"
                && updated.elapsed() >= Duration::from_millis(12) { break; }
            std::thread::sleep(Duration::from_millis(1));
        }
        let texts: Vec<_> = view.candidates.iter().map(|c| c.text.clone()).collect();
        if texts != host.core_page_texts() { return Err(format!("{}: displayed page differs from core digit mapping", case.id).into()); }
        let observation = host.model_observation().clone();
        let generation = host.generation_observation().clone();
        let comments: Vec<_> = view.candidates.iter().map(|c| c.comment.clone()).collect();
        let mut selected = None;
        if comments.first().is_some_and(|s| s == "整句 · 知句") {
            let commit = host.process(Key::Text("1".into()), 0, false).commit;
            if Some(&commit) != texts.first() { return Err(format!("{}: generated selection differs from the shown first", case.id).into()); }
            selected = Some(commit);
        }
        records.push(serde_json::json!({"id":case.id,"raw":case.raw,"context":case.context,"target":case.target,
            "contextRanking":case.context_ranking,"zhiju":case.zhiju,"candidates":texts,"comments":comments,
            "targetRank":texts.iter().position(|s| s==&case.target),"selectedGenerated":selected,
            "rankingOutcome":observation.outcome,"rankingMs":observation.latency_ms,
            "generationOutcome":generation.outcome,"generationMs":generation.latency_ms,"generationCallbacks":generation.callbacks,
            "generationMemoRows":generation.reused_prefix_rows,"interKeyMs":case.inter_key_ms,
            "keyMs":key_ms,"displayMatchesCore":true}));
    }
    let output=std::fs::OpenOptions::new().write(true).create_new(true).open(&args[3])?;
    serde_json::to_writer_pretty(output, &records)?;
    println!("{} real-model cases completed; all displayed pages matched core digit mapping", records.len());
    Ok(())
}
