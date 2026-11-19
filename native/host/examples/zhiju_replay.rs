//! Offline 知句 search; the page/ranking pipeline never receives the results.
//!   ZHIZI_ORT=... ZHIZI_MODEL=... ZHIZI_ITOS=... cargo run --offline --release
//!     --example zhiju_replay -- <shared> <blank-user> <cases.jsonl> <new-output.jsonl> [beam=8] [budget-ms=30000]
//! Output follows scripts/eval/ranking_bench/schema.md, with search-only candidates.

use serde::Deserialize;
use std::collections::HashSet;
use std::io::{BufRead, Write};
use std::path::Path;

#[derive(Deserialize)]
struct Case {
    id: String,
    bucket: String,
    raw: String,
    context: String,
    target: String,
    source: String,
    #[serde(default)]
    sub: String,
}

fn read_cases(path: &Path) -> Result<Vec<Case>, String> {
    let input = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut cases = Vec::new();
    let mut ids = HashSet::new();
    for (index, line) in std::io::BufReader::new(input).lines().enumerate() {
        let line = line.map_err(|e| e.to_string())?;
        if line.trim().is_empty() { continue; }
        let case: Case = serde_json::from_str(&line).map_err(|e| format!("line {}: {e}", index + 1))?;
        if case.id.is_empty() || case.bucket.is_empty() || case.target.is_empty() || case.source.is_empty()
            || case.raw.is_empty() || !case.raw.bytes().all(|b| b.is_ascii_lowercase() || b == b'\'') {
            return Err(format!("line {}: invalid evaluation case", index + 1));
        }
        if !ids.insert(case.id.clone()) { return Err(format!("duplicate id: {}", case.id)); }
        cases.push(case);
    }
    if cases.is_empty() { return Err("empty evaluation set".into()); }
    Ok(cases)
}

fn evaluation_weight(value: Option<String>) -> Option<f64> {
    value.and_then(|value| value.parse().ok()).filter(|value: &f64| value.is_finite() && *value >= 0.0)
}

#[cfg(feature = "model")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use zhizi_host::{Host, HostConfig, Key};
    use zhizi_host::zhiju::onnx::OnnxDistribution;

    let args: Vec<_> = std::env::args().skip(1).collect();
    if !(4..=7).contains(&args.len()) {
        return Err("usage: zhiju_replay <shared> <blank-user> <cases.jsonl> <new-output.jsonl> [beam=8] [budget-ms=30000] [threads=4]".into());
    }
    let beam: usize = args.get(4).map(|s| s.parse()).transpose()?.unwrap_or(8);
    let budget_ms: u64 = args.get(5).map(|s| s.parse()).transpose()?.unwrap_or(30_000);
    let threads: usize = args.get(6).map(|s| s.parse()).transpose()?.unwrap_or(4);
    if !(1..=4).contains(&threads) { return Err("threads must be 1 through 4".into()); }
    let cases = read_cases(Path::new(&args[2]))?;
    let user = Path::new(&args[1]);
    if user.exists() && std::fs::read_dir(user)?.next().is_some() {
        return Err("zhiju_replay requires an empty user directory".into());
    }
    std::fs::create_dir_all(user)?;
    let mut host = Host::open(&HostConfig {
        shared_dir: args[0].clone().into(), user_dir: user.into(), app_id: "zhizi.zhiju-replay".into(),
    })?;
    let required = |name: &str| std::env::var(name).map_err(|_| format!("missing {name}"));
    zhizi_host::ranking::onnx::init_runtime(Path::new(&required("ZHIZI_ORT")?))?;
    host.set_zhiju_vocab(Path::new(&required("ZHIZI_ITOS")?))?;
    let mut model = OnnxDistribution::load(Path::new(&required("ZHIZI_MODEL")?), threads)?;
    let mut output = std::io::BufWriter::new(std::fs::OpenOptions::new().write(true).create_new(true).open(&args[3])?);
    for (index, case) in cases.iter().enumerate() {
        host.process(Key::Escape, 0, false);
        host.clear_context();
        host.note_committed(&case.context);
        for ch in case.raw.chars() { host.process(Key::Text(ch.to_string()), 0, false); }
        let started = std::time::Instant::now();
        let (candidates, status) = match host.zhiju_search(&mut model, beam, budget_ms) {
            Ok(candidates) => (candidates, 0),
            Err(-3) => (Vec::new(), -3),
            Err(status) => return Err(format!("{}: 知句 search failed ({status})", case.id).into()),
        };
        let search_ms = started.elapsed().as_secs_f64() * 1000.0;
        let stats = model.stats();
        let rank = candidates.iter().position(|c| c.text == case.target).map(|i| i as i64).unwrap_or(-1);
        let texts: Vec<_> = candidates.iter().take(60).map(|c| &c.text).collect();
        let record = serde_json::json!({
            "id": case.id, "bucket": case.bucket, "sub": case.sub, "raw": case.raw,
            "context": case.context, "target": case.target, "source": case.source,
            "candidates": texts, "rank_all": rank, "model_ready": stats.successful_calls > 0,
            "model_ms": stats.onnx_ms, "outcome": if status == -3 { "not_applicable" } else { "searched" },
            "status": status, "beam": beam, "budget_ms": budget_ms,
            "initial_penalty_override": evaluation_weight(std::env::var("ZHIZI_EVAL_ZHIJU_INITIAL_PENALTY").ok()),
            "word_mu_override": evaluation_weight(std::env::var("ZHIZI_EVAL_ZHIJU_WORD_MU").ok()),
            "scores": candidates.iter().map(|c| c.score).collect::<Vec<_>>(),
            "consumed_keys": candidates.iter().map(|c| c.consumed).collect::<Vec<_>>(),
            "callback_calls": stats.callback_calls, "run_rows": stats.run_rows,
            "cached_calls": stats.cached_calls, "threads": threads,
            "search_ms": search_ms,
        });
        writeln!(output, "{record}")?;
        if (index + 1) % 25 == 0 { output.flush()?; eprintln!("{}/{}", index + 1, cases.len()); }
    }
    output.flush()?;
    Ok(())
}

#[cfg(not(feature = "model"))]
fn main() { eprintln!("zhiju_replay requires the model feature"); std::process::exit(2); }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zhiju_replay_records_only_valid_evaluation_weight_overrides() {
        assert_eq!(evaluation_weight(None), None);
        for value in ["bad", "NaN", "inf", "-0.1"] {
            assert_eq!(evaluation_weight(Some(value.into())), None);
        }
        assert_eq!(evaluation_weight(Some("0".into())), Some(0.0));
        assert_eq!(evaluation_weight(Some("0.1".into())), Some(0.1));
    }

    #[test]
    fn zhiju_replay_rejects_invalid_records_and_duplicate_ids() {
        let path = std::env::temp_dir().join(format!("zhiju-replay-cases-{}.jsonl", std::process::id()));
        let valid = r#"{"id":"a","bucket":"abbrev","raw":"wo'men'x'c","context":"","target":"我们想吃","source":"ext"}"#;
        std::fs::write(&path, valid).unwrap();
        assert_eq!(read_cases(&path).unwrap().len(), 1);
        for data in [format!("{valid}\n{valid}"), valid.replace("wo'men'x'c", "wo1men"), valid.replace("\"id\":\"a\"", "\"id\":\"\""), "bad json".into(), String::new()] {
            std::fs::write(&path, data).unwrap();
            assert!(read_cases(&path).is_err());
        }
        std::fs::remove_file(path).unwrap();
    }
}
