//! Offline wide-pool experiment; never installs a model in the candidate-pool host.
//! wide_rerank <shared> <new empty user> <cases.jsonl> <output dir> <wide|current>
//! Model files come only from ZHIZI_ORT / ZHIZI_MODEL / ZHIZI_ITOS.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::time::{Duration, Instant};
use zhizi_host::ranking::Scorer;
use zhizi_host::ranking::onnx::{OnnxScorer, init_runtime};
use zhizi_host::{Host, HostConfig, Key, View};

const LIMITS: [usize; 3] = [20, 40, 100];
const WEIGHTS: [f64; 4] = [0.0, 0.1, 0.5, 2.0];

#[derive(Clone, Deserialize, Serialize)]
struct Case {
    id: String,
    bucket: String,
    raw: String,
    context: String,
    target: String,
    source: String,
}

fn words(all: &[String], limit: usize) -> Vec<String> {
    all.iter().filter(|text| text.chars().count() >= 2).take(limit).cloned().collect()
}

fn scoring_context(context: &str, texts: &[String]) -> Result<String, String> {
    let longest = texts.iter().map(|text| text.chars().count()).max().unwrap_or(0);
    if longest > 39 || texts.iter().any(|text| text.is_empty()) {
        return Err("candidate outside scorer's 1..39 character range".into());
    }
    let context: Vec<char> = context.chars().collect();
    let keep = context.len().min(39 - longest);
    Ok(context[context.len() - keep..].iter().collect())
}

fn order(scores: &[f64], weight: f64) -> Vec<usize> {
    let mut indices: Vec<usize> = (0..scores.len()).collect();
    indices.sort_by(|&a, &b| {
        let score = |i: usize| scores[i] - weight * i as f64;
        score(b).total_cmp(&score(a)).then(a.cmp(&b))
    });
    indices
}

fn record(case: &Case, candidates: &[String], ms: f64, ready: bool) -> Value {
    let mut row = serde_json::to_value(case).unwrap();
    row["candidates"] = json!(candidates.iter().take(60).collect::<Vec<_>>());
    row["rank_all"] = json!(candidates.iter().position(|text| text == &case.target).map(|i| i as i64).unwrap_or(-1));
    row["model_ms"] = json!(ms);
    row["model_ready"] = json!(ready);
    row
}

#[derive(Serialize)]
struct Batch {
    offset: usize,
    rows: usize,
    ms: f64,
}

#[derive(Serialize)]
struct Measurement {
    batch_size: usize,
    total_ms: f64,
    batches: Vec<Batch>,
    // Exact individual calls for batch=1; amortized batch wall time otherwise.
    candidate_ms: Vec<f64>,
    scores: Vec<f64>,
}

// Experimental batch sizes only. The scoring protocol mirrors OnnxScorer; no engine changes.
struct BatchScorer {
    session: ort::session::Session,
    vocabulary: HashMap<String, i32>,
}

impl BatchScorer {
    fn load(model: &Path, itos: &Path, profile: Option<&Path>) -> Result<Self, String> {
        let itos: Vec<String> = serde_json::from_slice(&fs::read(itos).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        if itos.get(..3) != Some(&["<pad>".to_string(), "<bos>".to_string(), "<unk>".to_string()][..]) {
            return Err("invalid model vocabulary".into());
        }
        let mut builder = ort::session::Session::builder().map_err(|e| e.to_string())?
            .with_auto_device(ort::session::builder::AutoDevicePolicy::PreferCPU).map_err(|e| e.to_string())?
            .with_parallel_execution(false).map_err(|e| e.to_string())?
            .with_intra_threads(1).map_err(|e| e.to_string())?
            .with_inter_threads(1).map_err(|e| e.to_string())?;
        if let Some(path) = profile { builder = builder.with_profiling(path).map_err(|e| e.to_string())?; }
        let session = builder.commit_from_file(model).map_err(|e| e.to_string())?;
        Ok(Self { session, vocabulary: itos.into_iter().enumerate().map(|(i, text)| (text, i as i32)).collect() })
    }

    fn token(&self, c: char) -> i32 {
        self.vocabulary.get(c.encode_utf8(&mut [0; 4]) as &str).copied().unwrap_or(2)
    }

    fn score(&mut self, context: &str, texts: &[String], batch_size: usize) -> Result<Measurement, String> {
        let started = Instant::now();
        let context = scoring_context(context, texts)?;
        let prefix: Vec<i32> = std::iter::once(1).chain(context.chars().map(|c| self.token(c))).collect();
        let mut result = Measurement { batch_size, total_ms: 0.0, batches: Vec::new(), candidate_ms: Vec::new(), scores: Vec::new() };
        for chunk in texts.chunks(batch_size) {
            let before = Instant::now();
            let length = prefix.len() + chunk.iter().map(|text| text.chars().count()).max().unwrap();
            let mut tokens = vec![0i32; chunk.len() * length];
            for (row, text) in chunk.iter().enumerate() {
                for (column, token) in prefix.iter().copied().chain(text.chars().map(|c| self.token(c))).enumerate() {
                    tokens[row * length + column] = token;
                }
            }
            let input = ort::value::Tensor::from_array(([chunk.len(), length], tokens)).map_err(|e| e.to_string())?;
            let outputs = self.session.run(ort::inputs!["tokens" => input]).map_err(|e| e.to_string())?;
            let (_, logprobs) = outputs["logprobs"].try_extract_tensor::<f32>().map_err(|e| e.to_string())?;
            let scores: Vec<f64> = chunk.iter().enumerate().map(|(row, text)| {
                (prefix.len() - 1..prefix.len() - 1 + text.chars().count())
                    .map(|i| logprobs[row * (length - 1) + i] as f64).sum()
            }).collect();
            check_scores(&scores, chunk.len())?;
            let ms = before.elapsed().as_secs_f64() * 1000.0;
            result.batches.push(Batch { offset: result.scores.len(), rows: chunk.len(), ms });
            result.candidate_ms.extend(std::iter::repeat_n(ms / chunk.len() as f64, chunk.len()));
            result.scores.extend(scores);
        }
        result.total_ms = started.elapsed().as_secs_f64() * 1000.0;
        Ok(result)
    }
}

fn check_scores(scores: &[f64], expected: usize) -> Result<(), String> {
    if scores.len() != expected || scores.iter().any(|s| !s.is_finite()) {
        return Err("invalid model scores".into());
    }
    Ok(())
}

struct AuditedScorer {
    inner: OnnxScorer,
    failed: Arc<AtomicBool>,
}

impl Scorer for AuditedScorer {
    fn score(&mut self, context: &str, texts: &[String]) -> Result<Vec<f64>, String> {
        let result = self.inner.score(context, texts).and_then(|scores| {
            check_scores(&scores, texts.len())?;
            Ok(scores)
        });
        if result.is_err() { self.failed.store(true, Ordering::SeqCst); }
        result
    }
}

fn output(path: &Path) -> Result<BufWriter<File>, String> {
    File::options().write(true).create_new(true).open(path).map(BufWriter::new).map_err(|e| e.to_string())
}

fn write_row(stream: &mut impl Write, row: &impl Serialize) -> Result<(), String> {
    serde_json::to_writer(&mut *stream, row).map_err(|e| e.to_string())?;
    writeln!(stream).map_err(|e| e.to_string())
}

fn type_case(host: &mut Host, case: &Case) -> View {
    host.process(Key::Escape, 0, false);
    host.clear_context();
    host.note_committed(&case.context);
    let mut view = View::default();
    for c in case.raw.chars() {
        view = host.process(Key::Text(c.to_string()), 0, false);
    }
    view
}

fn pool(host: &mut Host, view: &View) -> Vec<String> {
    let mut all: Vec<String> = view.candidates.iter().map(|c| c.text.clone()).collect();
    for page in host.peek_following_pages(60) {
        all.extend(page.text.split('\t').filter(|t| !t.is_empty()).map(str::to_string));
    }
    all
}

fn load_cases(path: &Path) -> Result<Vec<Case>, String> {
    let mut cases = Vec::new();
    let mut ids = HashSet::new();
    for (i, line) in fs::read_to_string(path).map_err(|e| e.to_string())?.lines().enumerate() {
        if line.trim().is_empty() { continue; }
        let case: Case = serde_json::from_str(line).map_err(|e| format!("case {}: {e}", i + 1))?;
        if case.id.is_empty() || !ids.insert(case.id.clone()) || case.bucket != "abbrev" || case.raw.len() < 2
            || !case.raw.bytes().all(|c| c.is_ascii_lowercase()) || case.target.is_empty()
            || !["ext", "owner_pre", "friend"].contains(&case.source.as_str()) || case.context.chars().count() > 32 {
            return Err(format!("invalid abbreviation case {}", i + 1));
        }
        cases.push(case);
    }
    if cases.is_empty() { return Err("no cases".into()); }
    Ok(cases)
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() != 5 || !["wide", "current"].contains(&args[4].as_str()) {
        return Err("usage: wide_rerank <shared> <new empty user> <cases.jsonl> <output dir> <wide|current>".into());
    }
    let wide = args[4] == "wide";
    let pages = if wide { "20" } else { "2" };
    // Set process-local experiment controls before any host/model threads exist.
    unsafe {
        std::env::set_var("ZX_PEEK_PAGES", "60");
        std::env::set_var("ZX_ABBREV_WORD_PAGES", pages);
        std::env::set_var("ZX_ABBREV_SINGLE_INTERLEAVE", "0");
    }
    let cases = load_cases(Path::new(&args[2]))?;
    let variable = |name| std::env::var(name).map_err(|_| format!("missing {name}"));
    let library = variable("ZHIZI_ORT")?;
    let model = variable("ZHIZI_MODEL")?;
    let itos = variable("ZHIZI_ITOS")?;
    init_runtime(library.as_ref())?;
    let mut scorer = OnnxScorer::load(model.as_ref(), itos.as_ref(), 1)?;
    let user = Path::new(&args[1]);
    fs::create_dir_all(user).map_err(|e| e.to_string())?;
    if fs::read_dir(user).map_err(|e| e.to_string())?.next().is_some() {
        return Err("user directory must be empty".into());
    }
    let out = Path::new(&args[3]);
    fs::create_dir_all(out).map_err(|e| e.to_string())?;
    let mut host = Host::open(&HostConfig { shared_dir: args[0].clone().into(), user_dir: user.into(), app_id: "zhizi.wide-rerank".into() })?;
    if !wide {
        let failed = Arc::new(AtomicBool::new(false));
        host.enable_model(Box::new(AuditedScorer { inner: scorer, failed: Arc::clone(&failed) }), None);
        let mut stream = output(&out.join("current.jsonl"))?;
        for (i, case) in cases.iter().enumerate() {
            let mut view = type_case(&mut host, case);
            let started = Instant::now();
            while host.model_observation().outcome == "pending" {
                if started.elapsed() > Duration::from_secs(120) { return Err("current model timed out".into()); }
                std::thread::sleep(Duration::from_millis(1));
                if let Some(late) = host.poll_ranking() { view = late; }
            }
            let observation = host.model_observation();
            let ready = !host.last_group().scores.is_empty() || !host.last_composition().scores.is_empty();
            let ms = observation.latency_ms.unwrap_or(0.0);
            let outcome = observation.outcome;
            if failed.load(Ordering::SeqCst) || (!ready && outcome != "no_group") {
                return Err(format!("current scoring failed on case {}", i + 1));
            }
            let all = pool(&mut host, &view);
            let mut row = record(case, &all, ms, ready);
            row["outcome"] = json!(outcome);
            write_row(&mut stream, &row)?;
            if (i + 1) % 25 == 0 { eprintln!("current: {}/{}", i + 1, cases.len()); }
        }
        stream.flush().map_err(|e| e.to_string())?;
        return Ok(());
    }
    let warm: Vec<String> = (0..32).map(|_| "候选".into()).collect();
    let mut probe = BatchScorer::load(model.as_ref(), itos.as_ref(), Some(&out.join("cpu-probe")))?;
    probe.score("这是预热上文", &warm, 8)?;
    let profile = probe.session.end_profiling().map_err(|e| e.to_string())?;
    let events: Vec<Value> = serde_json::from_slice(&fs::read(&profile).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let providers: HashSet<String> = events.iter().filter_map(|event| event["args"]["provider"].as_str().map(str::to_string)).collect();
    if providers != HashSet::from(["CPUExecutionProvider".into()]) {
        return Err(format!("CPU provider verification failed: {providers:?}"));
    }
    write_row(&mut output(&out.join("cpu-provider.json"))?, &json!({"providers": providers, "profile": profile, "threads": 1}))?;
    drop(probe);
    let mut batch_scorer = BatchScorer::load(model.as_ref(), itos.as_ref(), None)?;
    for _ in 0..3 {
        scorer.score("这是预热上文", &warm)?;
        for batch in [1, 8, 16, 32] { batch_scorer.score("这是预热上文", &warm, batch)?; }
    }
    let mut detail = output(&out.join("wide.jsonl"))?;
    let mut streams = HashMap::new();
    for n in LIMITS {
        for weight in WEIGHTS {
            let name = format!("n{n}-w{weight}");
            streams.insert(name.clone(), output(&out.join(format!("{name}.jsonl")))?);
        }
    }
    for (i, case) in cases.iter().enumerate() {
        let view = type_case(&mut host, case);
        let all = pool(&mut host, &view);
        let mut experiments = Vec::new();
        for (j, n) in LIMITS.into_iter().enumerate() {
            let texts = words(&all, n);
            let mut batches = [8, 16, 32, 1];
            batches.rotate_left((i + j) % 4);
            let mut measurements = Vec::new();
            for batch in batches {
                measurements.push(batch_scorer.score(&case.context, &texts, batch)?);
            }
            let reference = &measurements.iter().find(|m| m.batch_size == 8).unwrap();
            let existing_scores = if texts.is_empty() { Vec::new() } else { scorer.score(&case.context, &texts)? };
            check_scores(&existing_scores, texts.len())?;
            let existing_delta = existing_scores.iter().zip(&reference.scores).map(|(a, b)| (a - b).abs()).fold(0.0, f64::max);
            let existing_order_changed: Vec<bool> = WEIGHTS.iter().map(|&w| order(&existing_scores, w) != order(&reference.scores, w)).collect();
            let mut max_delta = 0.0f64;
            let mut rank_changes = Vec::new();
            for measurement in &measurements {
                check_scores(&measurement.scores, texts.len())?;
                let delta = measurement.scores.iter().zip(&reference.scores).map(|(a, b)| (a - b).abs()).fold(0.0, f64::max);
                max_delta = max_delta.max(delta);
                if delta > 0.001 { return Err(format!("batch score mismatch on case {}: {delta}", i + 1)); }
                rank_changes.push(json!({ "batch_size": measurement.batch_size, "order_changed": WEIGHTS.iter()
                    .map(|&weight| order(&measurement.scores, weight) != order(&reference.scores, weight)).collect::<Vec<_>>() }));
            }
            let mut ranks = Vec::new();
            for weight in WEIGHTS {
                let sorted: Vec<String> = order(&reference.scores, weight).iter().map(|&index| texts[index].clone()).collect();
                let row = record(case, &sorted, reference.total_ms, !texts.is_empty());
                ranks.push(json!({ "weight": weight, "rank_all": row["rank_all"] }));
                write_row(streams.get_mut(&format!("n{n}-w{weight}")).unwrap(), &row)?;
            }
            experiments.push(json!({ "n": n, "actual_n": texts.len(), "texts": texts,
                "measurements": measurements, "max_score_delta": max_delta, "batch_order_changes": rank_changes, "ranks": ranks,
                "existing_scores": existing_scores, "existing_max_delta": existing_delta, "existing_order_changed": existing_order_changed }));
        }
        let mut row = serde_json::to_value(case).unwrap();
        row["pool_size"] = json!(all.len());
        row["pool_word_count"] = json!(words(&all, usize::MAX).len());
        row["pool_rank_all"] = json!(all.iter().position(|text| text == &case.target).map(|i| i as i64).unwrap_or(-1));
        row["experiments"] = json!(experiments);
        write_row(&mut detail, &row)?;
        if (i + 1) % 10 == 0 { eprintln!("wide: {}/{}", i + 1, cases.len()); }
    }
    detail.flush().map_err(|e| e.to_string())?;
    for stream in streams.values_mut() { stream.flush().map_err(|e| e.to_string())?; }
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("wide_rerank: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_words_before_truncating_without_changing_prior_order() {
        let all = ["是", "时候", "上", "上海", "时候", "生活"].map(String::from);
        assert_eq!(words(&all, 3), ["时候", "上海", "时候"]);
        assert!(words(&all, 0).is_empty());
    }

    #[test]
    fn pure_model_and_three_prior_strengths_keep_stable_ties() {
        let scores = [-5.0, -4.9, -4.0, -4.0];
        assert_eq!(order(&scores, 0.0), [2, 3, 1, 0]);
        assert_eq!(order(&scores, 0.1), [2, 3, 0, 1]);
        assert_eq!(order(&scores, 0.5), [0, 2, 1, 3]);
        assert_eq!(order(&scores, 2.0), [0, 1, 2, 3]);
    }

    #[test]
    fn batches_share_one_unicode_context_window() {
        let context = "甲".repeat(35) + "乙丙丁戊己";
        let texts = ["测试".into(), "长".repeat(10)];
        let clipped = scoring_context(&context, &texts).unwrap();
        assert_eq!(clipped.chars().count(), 29);
        assert!(clipped.ends_with("乙丙丁戊己"));
        assert_eq!(scoring_context(&clipped, &texts[..1]).unwrap(), clipped);
        assert!(scoring_context("", &["长".repeat(40)]).is_err());
        assert_eq!(scoring_context("上文", &[]).unwrap(), "上文");
    }

    #[test]
    fn full_rank_survives_report_candidate_cutoff_and_misses() {
        let case = Case { id: "test".into(), bucket: "abbrev".into(), raw: "cs".into(),
            context: "".into(), target: "目标".into(), source: "ext".into() };
        let mut candidates: Vec<String> = (0..70).map(|i| format!("词{i}")).collect();
        candidates.push(case.target.clone());
        let row = record(&case, &candidates, 12.5, true);
        assert_eq!(row["rank_all"], 70);
        assert_eq!(row["candidates"].as_array().unwrap().len(), 60);
        assert_eq!(row["model_ms"], 12.5);
        assert_eq!(record(&case, &[], 0.0, false)["rank_all"], -1);
    }

    #[test]
    fn reject_missing_or_nonfinite_scores() {
        assert!(check_scores(&[1.0], 2).is_err());
        assert!(check_scores(&[f64::NAN], 1).is_err());
        assert!(check_scores(&[f64::INFINITY], 1).is_err());
        assert!(check_scores(&[-2.0, -3.0], 2).is_ok());
    }
}
