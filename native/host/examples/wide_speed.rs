//! Offline G4 timing and score-equivalence experiment; no engine changes.
//! wide_speed <G3 wide.jsonl> <G3 cases.jsonl> <new output directory>
//! Models/runtime/vocabulary are loaded only from ZHIZI_MODEL/ORT/ITOS;
//! optional existing variants: ZHIZI_MODEL_INT8 and ZHIZI_MODEL_FP16.
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::Path;
use std::time::Instant;
use zhizi_host::ranking::onnx::init_runtime;

const WEIGHTS: [f64; 4] = [0.0, 0.1, 0.5, 2.0];

fn context_window(context: &str, texts: &[String]) -> Result<Vec<char>, String> {
    let longest = texts.iter().map(|t| t.chars().count()).max().unwrap_or(0);
    if longest > 39 || texts.iter().any(String::is_empty) { return Err("invalid candidate length".into()); }
    let chars: Vec<char> = context.chars().collect();
    let keep = chars.len().min(39 - longest);
    Ok(chars[chars.len() - keep..].to_vec())
}

fn batches(texts: &[String], size: usize, bucket: bool) -> Vec<Vec<usize>> {
    let mut indices: Vec<usize> = (0..texts.len()).collect();
    if bucket { indices.sort_by_key(|&i| texts[i].chars().count()); }
    let mut result = Vec::new();
    let mut start = 0;
    while start < indices.len() {
        let length = texts[indices[start]].chars().count();
        let mut end = start + 1;
        while end < indices.len() && end - start < size
            && (!bucket || texts[indices[end]].chars().count() == length) { end += 1; }
        result.push(indices[start..end].to_vec());
        start = end;
    }
    result
}

fn extract(logprobs: &[f32], rows: usize, length: usize, prefix: usize, counts: &[usize]) -> Result<Vec<f64>, String> {
    if length < 2 || prefix == 0 || counts.len() != rows || logprobs.len() != rows * (length - 1)
        || counts.iter().any(|&n| n == 0 || prefix + n > length) {
        return Err("invalid logprobs shape or gather range".into());
    }
    let scores: Vec<f64> = counts.iter().enumerate().map(|(row, &n)| {
        let start = row * (length - 1) + prefix - 1;
        logprobs[start..start + n].iter().map(|&v| v as f64).sum()
    }).collect();
    if scores.iter().any(|s| !s.is_finite()) { return Err("non-finite score".into()); }
    Ok(scores)
}

fn order(scores: &[f64], weight: f64) -> Vec<usize> {
    let mut indices: Vec<usize> = (0..scores.len()).collect();
    indices.sort_by(|&a, &b| (scores[b] - weight * b as f64).total_cmp(&(scores[a] - weight * a as f64)).then(a.cmp(&b)));
    indices
}

fn distribution(values: &[f64]) -> serde_json::Value {
    if values.is_empty() { return json!({"count": 0, "median_ms": null, "p95_ms": null}); }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let n = sorted.len();
    let median = if n % 2 == 0 { (sorted[n / 2 - 1] + sorted[n / 2]) / 2.0 } else { sorted[n / 2] };
    json!({"count": n, "median_ms": median, "p95_ms": sorted[(n * 95).div_ceil(100) - 1],
        "within_15ms": sorted.iter().filter(|&&ms| ms <= 15.0).count()})
}

#[derive(Clone, Serialize)]
struct Config {
    name: String,
    model: usize,
    threads: usize,
    batch: usize,
    bucket: bool,
}

#[derive(Serialize)]
struct Measurement {
    total_ms: f64,
    calls: usize,
    token_cells: usize,
    scores: Vec<f64>,
}

struct Scorer {
    session: ort::session::Session,
    vocabulary: HashMap<String, i32>,
}

impl Scorer {
    fn load(model: &Path, vocabulary: &[String], threads: usize, profile: Option<&Path>) -> Result<Self, String> {
        let mut builder = ort::session::Session::builder().map_err(|e| e.to_string())?
            .with_auto_device(ort::session::builder::AutoDevicePolicy::PreferCPU).map_err(|e| e.to_string())?
            .with_parallel_execution(false).map_err(|e| e.to_string())?
            .with_intra_threads(threads).map_err(|e| e.to_string())?
            .with_inter_threads(1).map_err(|e| e.to_string())?
            // Multiple sessions coexist; idle worker pools must not contend with the measured one.
            .with_config_entry("session.intra_op.allow_spinning", "0").map_err(|e| e.to_string())?;
        if let Some(path) = profile { builder = builder.with_profiling(path).map_err(|e| e.to_string())?; }
        let session = builder.commit_from_file(model).map_err(|e| e.to_string())?;
        if session.inputs().len() != 1 || session.inputs()[0].name() != "tokens"
            || session.outputs().len() != 1 || session.outputs()[0].name() != "logprobs" {
            return Err("this benchmark requires the existing tokens -> logprobs interface".into());
        }
        Ok(Self { session, vocabulary: vocabulary.iter().cloned().enumerate().map(|(i, t)| (t, i as i32)).collect() })
    }

    fn token(&self, c: char) -> i32 {
        self.vocabulary.get(c.encode_utf8(&mut [0; 4]) as &str).copied().unwrap_or(2)
    }

    fn score(&mut self, context: &str, texts: &[String], config: &Config) -> Result<Measurement, String> {
        let started = Instant::now();
        let mut result = Measurement { total_ms: 0.0, calls: 0, token_cells: 0, scores: vec![0.0; texts.len()] };
        // Match G3: zero candidates means no model call and zero latency in the 300-case denominator.
        if texts.is_empty() { return Ok(result); }
        let prefix: Vec<i32> = std::iter::once(1).chain(context_window(context, texts)?.into_iter().map(|c| self.token(c))).collect();
        for group in batches(texts, config.batch, config.bucket) {
            let counts: Vec<usize> = group.iter().map(|&i| texts[i].chars().count()).collect();
            let length = prefix.len() + counts.iter().max().unwrap();
            let mut tokens = vec![0i32; group.len() * length];
            for (row, &index) in group.iter().enumerate() {
                for (column, token) in prefix.iter().copied().chain(texts[index].chars().map(|c| self.token(c))).enumerate() {
                    tokens[row * length + column] = token;
                }
            }
            let input = ort::value::Tensor::from_array(([group.len(), length], tokens)).map_err(|e| e.to_string())?;
            let outputs = self.session.run(ort::inputs!["tokens" => input]).map_err(|e| e.to_string())?;
            let (shape, data) = outputs["logprobs"].try_extract_tensor::<f32>().map_err(|e| e.to_string())?;
            if shape.len() != 2 || shape[0] != group.len() as i64 || shape[1] != (length - 1) as i64 {
                return Err("unexpected model output dimensions".into());
            }
            let scores = extract(data, group.len(), length, prefix.len(), &counts)?;
            for (&index, score) in group.iter().zip(scores) { result.scores[index] = score; }
            result.calls += 1;
            result.token_cells += group.len() * length;
        }
        result.total_ms = started.elapsed().as_secs_f64() * 1000.0;
        Ok(result)
    }
}

fn fingerprint(path: &Path) -> Result<String, String> {
    Ok(format!("{:x}", Sha256::digest(fs::read(path).map_err(|e| e.to_string())?)))
}

fn output(path: &Path) -> Result<BufWriter<File>, String> {
    File::options().create_new(true).write(true).open(path).map(BufWriter::new).map_err(|e| e.to_string())
}

fn write_row(stream: &mut impl Write, value: &impl Serialize) -> Result<(), String> {
    serde_json::to_writer(&mut *stream, value).map_err(|e| e.to_string())?;
    writeln!(stream).map_err(|e| e.to_string())
}

fn read_rows(path: &Path) -> Result<Vec<Value>, String> {
    fs::read_to_string(path).map_err(|e| e.to_string())?.lines().filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).map_err(|e| e.to_string())).collect()
}

struct Case {
    id: String,
    context: String,
    texts: Vec<String>,
    g3_scores: Vec<f64>,
}

fn cases(wide: &[Value], original: &[Value]) -> Result<Vec<Case>, String> {
    if wide.len() != 300 || original.len() != 300 { return Err("expected the same 300 G3 cases".into()); }
    let mut ids = HashSet::new();
    wide.iter().zip(original).map(|(row, case)| {
        for field in ["id", "bucket", "raw", "context", "target", "source"] {
            if !case[field].is_string() || row[field] != case[field] { return Err(format!("G3 case mismatch: {field}")); }
        }
        let id = row["id"].as_str().unwrap().to_string();
        if id.is_empty() || !ids.insert(id.clone()) || case["bucket"] != "abbrev" || case["source"] != "ext"
            || case["context"].as_str().unwrap().chars().count() > 32 { return Err("invalid G3 case".into()); }
        let exp = row["experiments"].as_array().ok_or("missing G3 experiments")?.iter().find(|e| e["n"] == 40).ok_or("missing N=40")?;
        let texts: Vec<String> = serde_json::from_value(exp["texts"].clone()).map_err(|e| e.to_string())?;
        let measurement = exp["measurements"].as_array().ok_or("missing G3 measurements")?.iter()
            .find(|m| m["batch_size"] == 8).ok_or("missing G3 batch=8")?;
        let g3_scores: Vec<f64> = serde_json::from_value(measurement["scores"].clone()).map_err(|e| e.to_string())?;
        if texts.len() > 40 || exp["actual_n"].as_u64() != Some(texts.len() as u64)
            || texts.iter().any(|t| t.chars().count() < 2) || g3_scores.len() != texts.len()
            || g3_scores.iter().any(|s| !s.is_finite()) { return Err("invalid G3 candidates/scores".into()); }
        Ok(Case { id, context: row["context"].as_str().unwrap().into(), texts, g3_scores })
    }).collect()
}

fn comparison(reference: &[f64], scores: &[f64]) -> Value {
    json!({"max_delta": reference.iter().zip(scores).map(|(a,b)| (a-b).abs()).fold(0.0, f64::max),
        "order_changed": WEIGHTS.map(|w| order(reference, w) != order(scores, w)),
        "top1_changed": WEIGHTS.map(|w| order(reference, w).first() != order(scores, w).first())})
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() != 3 { return Err("usage: wide_speed <G3 wide.jsonl> <G3 cases.jsonl> <new output dir>".into()); }
    let data = cases(&read_rows(args[0].as_ref())?, &read_rows(args[1].as_ref())?)?;
    let out = Path::new(&args[2]);
    fs::create_dir(out).map_err(|e| e.to_string())?;
    let variable = |name| std::env::var(name).map_err(|_| format!("missing {name}"));
    let library = variable("ZHIZI_ORT")?;
    let itos = variable("ZHIZI_ITOS")?;
    init_runtime(library.as_ref())?;
    let vocabulary: Vec<String> = serde_json::from_slice(&fs::read(&itos).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    if vocabulary.get(..3) != Some(&["<pad>".into(), "<bos>".into(), "<unk>".into()][..]) { return Err("invalid vocabulary".into()); }
    let mut models = vec![("original", variable("ZHIZI_MODEL")?)];
    for (label, name) in [("int8", "ZHIZI_MODEL_INT8"), ("fp16", "ZHIZI_MODEL_FP16")] {
        if let Ok(path) = std::env::var(name) { models.push((label, path)); }
    }
    let mut metadata = Vec::new();
    let mut sessions = HashMap::new();
    let mut configs = Vec::new();
    let warm = ["候选".to_string(), "预热词".to_string()];
    for (model, (label, path)) in models.iter().enumerate() {
        let config = Config { name: format!("{label}-b8-t1"), model, threads: 1, batch: 8, bucket: false };
        let mut probe = Scorer::load(path.as_ref(), &vocabulary, 1, Some(&out.join(format!("probe-{label}"))))?;
        probe.score("这是预热上文", &warm, &config)?;
        let interface = json!({"inputs": probe.session.inputs().iter().map(|x| json!({"name": x.name(), "type": format!("{:?}", x.dtype())})).collect::<Vec<_>>(),
            "outputs": probe.session.outputs().iter().map(|x| json!({"name": x.name(), "type": format!("{:?}", x.dtype())})).collect::<Vec<_>>()});
        let profile = probe.session.end_profiling().map_err(|e| e.to_string())?;
        let events: Vec<Value> = serde_json::from_slice(&fs::read(profile).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        let providers: HashSet<String> = events.iter().filter_map(|e| e["args"]["provider"].as_str().map(str::to_string)).collect();
        if providers != HashSet::from(["CPUExecutionProvider".into()]) { return Err(format!("unexpected providers: {providers:?}")); }
        metadata.push(json!({"label": label, "sha256": fingerprint(path.as_ref())?, "interface": interface, "providers": providers}));
        drop(probe);
        let threads: &[usize] = if model == 0 { &[1, 2, 4, 8] } else { &[1] };
        for &thread_count in threads {
            sessions.insert((model, thread_count), Scorer::load(path.as_ref(), &vocabulary, thread_count, None)?);
            let options: &[(usize, bool)] = if model == 0 && thread_count == 1 {
                &[(8, false), (16, false), (32, false), (40, false), (8, true), (40, true)]
            } else { &[(40, false), (40, true)] };
            for &(batch, bucket) in options {
                configs.push(Config { name: format!("{label}-b{batch}-t{thread_count}{}", if bucket { "-bucket" } else { "" }),
                    model, threads: thread_count, batch, bucket });
            }
        }
    }
    // Warm actual shapes outside the measured region, without using target labels.
    for config in &configs {
        let scorer = sessions.get_mut(&(config.model, config.threads)).unwrap();
        for case in data.iter().take(3) { scorer.score(&case.context, &case.texts, config)?; }
    }
    let started = Instant::now();
    let mut detail = output(&out.join("measurements.jsonl"))?;
    let mut timings = vec![Vec::new(); configs.len()];
    let mut full_timings = timings.clone();
    let mut max_delta = vec![0.0f64; configs.len()];
    let mut order_changes = vec![[0usize; 4]; configs.len()];
    let mut top1_changes = order_changes.clone();
    let mut g3_max_delta = 0.0f64;
    let mut g3_order_changes = [0usize; 4];
    for (i, case) in data.iter().enumerate() {
        let mut measurements: Vec<Option<Measurement>> = (0..configs.len()).map(|_| None).collect();
        for offset in 0..configs.len() {
            let j = (i + offset) % configs.len();
            let config = &configs[j];
            measurements[j] = Some(sessions.get_mut(&(config.model, config.threads)).unwrap().score(&case.context, &case.texts, config)?);
        }
        let measurements: Vec<Measurement> = measurements.into_iter().map(Option::unwrap).collect();
        let reference = &measurements[0].scores;
        let g3 = comparison(&case.g3_scores, reference);
        g3_max_delta = g3_max_delta.max(g3["max_delta"].as_f64().unwrap());
        for k in 0..4 { g3_order_changes[k] += usize::from(g3["order_changed"][k].as_bool().unwrap()); }
        let mut comparisons = Vec::new();
        for (j, measurement) in measurements.iter().enumerate() {
            timings[j].push(measurement.total_ms);
            if case.texts.len() == 40 { full_timings[j].push(measurement.total_ms); }
            let cmp = comparison(reference, &measurement.scores);
            max_delta[j] = max_delta[j].max(cmp["max_delta"].as_f64().unwrap());
            for k in 0..4 {
                order_changes[j][k] += usize::from(cmp["order_changed"][k].as_bool().unwrap());
                top1_changes[j][k] += usize::from(cmp["top1_changed"][k].as_bool().unwrap());
            }
            comparisons.push(cmp);
        }
        write_row(&mut detail, &json!({"id": case.id, "actual_n": case.texts.len(), "measurements": measurements,
            "comparisons": comparisons, "g3_baseline_comparison": g3}))?;
        if (i + 1) % 10 == 0 { eprintln!("wide_speed: {}/{} ({:.1}s)", i + 1, data.len(), started.elapsed().as_secs_f64()); }
    }
    detail.flush().map_err(|e| e.to_string())?;
    let results: Vec<Value> = configs.iter().enumerate().map(|(i, config)| json!({"config": config,
        "all": distribution(&timings[i]), "full_n40": distribution(&full_timings[i]),
        "max_delta": max_delta[i], "order_changes": order_changes[i], "top1_changes": top1_changes[i]})).collect();
    let summary = json!({"cases": data.len(), "empty_cases": data.iter().filter(|c| c.texts.is_empty()).count(), "weights": WEIGHTS,
        "g3_detail_sha256": fingerprint(args[0].as_ref())?, "cases_sha256": fingerprint(args[1].as_ref())?,
        "runtime_sha256": fingerprint(library.as_ref())?, "vocabulary_sha256": fingerprint(itos.as_ref())?,
        "models": metadata, "g3_baseline_max_delta": g3_max_delta, "g3_baseline_order_changes": g3_order_changes,
        "elapsed_seconds": started.elapsed().as_secs_f64(), "results": results});
    write_row(&mut output(&out.join("summary.json"))?, &summary)?;
    println!("{}", serde_json::to_string_pretty(&summary).unwrap());
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("wide_speed: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_unicode_window_precedes_all_length_buckets() {
        let context = "甲".repeat(35) + "乙丙丁戊己";
        let texts = ["测试".into(), "长".repeat(10)];
        let window = context_window(&context, &texts).unwrap();
        assert_eq!(window.len(), 29);
        assert!(window.iter().collect::<String>().ends_with("乙丙丁戊己"));
        assert!(context_window("", &["长".repeat(40)]).is_err());
        assert!(context_window("", &[String::new()]).is_err());
    }

    #[test]
    fn buckets_preserve_duplicate_indices_and_partial_batches() {
        let texts = ["长长长", "短短", "长长长", "最最最最", "短短"].map(String::from);
        assert_eq!(batches(&texts, 2, false), [vec![0, 1], vec![2, 3], vec![4]]);
        let grouped = batches(&texts, 2, true);
        assert_eq!(grouped, [vec![1, 4], vec![0, 2], vec![3]]);
        let mut restored = vec![0; texts.len()];
        for group in grouped { for i in group { restored[i] = i + 10; } }
        assert_eq!(restored, [10, 11, 12, 13, 14]);
        assert!(batches(&[], 8, true).is_empty());
    }

    #[test]
    fn gather_excludes_context_and_padding_and_checks_shape() {
        let data = [-99.0, -98.0, -2.0, -3.0, -97.0, -96.0, -4.0, -95.0];
        assert_eq!(extract(&data, 2, 5, 3, &[2, 1]).unwrap(), [-5.0, -4.0]);
        assert!(extract(&data[..7], 2, 5, 3, &[2, 1]).is_err());
        assert!(extract(&data, 2, 5, 3, &[3, 1]).is_err());
        let mut invalid = data;
        invalid[2] = f32::NAN;
        assert!(extract(&invalid, 2, 5, 3, &[2, 1]).is_err());
    }

    #[test]
    fn order_uses_original_prior_and_detects_tiny_tail_changes() {
        assert_eq!(order(&[-5.0, -4.9, -4.0, -4.0], 0.5), [0, 2, 1, 3]);
        assert_eq!(order(&[-5.0, -4.9, -4.0, -4.0], 0.0), [2, 3, 1, 0]);
        assert_ne!(order(&[-1.0, -2.0, -2.000001], 0.0), order(&[-1.0, -2.000002, -2.0], 0.0));
    }

    #[test]
    fn sample_median_and_nearest_rank_p95() {
        assert_eq!(distribution(&[4.0, 1.0, 3.0, 2.0])["median_ms"], 2.5);
        let values: Vec<f64> = (1..=300).map(f64::from).collect();
        assert_eq!(distribution(&values)["p95_ms"], 285.0);
        assert_eq!(distribution(&[])["median_ms"], serde_json::Value::Null);
    }

    #[test]
    fn reject_changed_case_identity_candidate_counts_and_reference_scores() {
        let original: Vec<Value> = (0..300).map(|i| json!({"id": format!("case-{i}"), "bucket": "abbrev",
            "raw": "cs", "context": "上文", "target": "测试", "source": "ext"})).collect();
        let mut wide = original.clone();
        for row in &mut wide {
            row["experiments"] = json!([{"n": 40, "actual_n": 2, "texts": ["测试", "测试"],
                "measurements": [{"batch_size": 8, "scores": [-1.0, -1.0]}]}]);
        }
        assert_eq!(cases(&wide, &original).unwrap().len(), 300);
        assert!(cases(&wide[..299], &original).is_err());
        for (field, value) in [("id", json!("case-1")), ("context", json!("改变"))] {
            let mut changed = wide.clone();
            changed[0][field] = value;
            assert!(cases(&changed, &original).is_err());
        }
        let mut changed = wide.clone();
        changed[0]["experiments"][0]["actual_n"] = json!(3);
        assert!(cases(&changed, &original).is_err());
        wide[0]["experiments"][0]["measurements"][0]["scores"] = json!([-1.0]);
        assert!(cases(&wide, &original).is_err());
    }

    #[test]
    fn report_tail_swaps_even_when_top1_and_score_tolerance_pass() {
        let cmp = comparison(&[-1.0, -2.0, -2.000001], &[-1.0, -2.000002, -2.0]);
        assert!(cmp["max_delta"].as_f64().unwrap() < 0.001);
        assert_eq!(cmp["order_changed"][0], true);
        assert_eq!(cmp["top1_changed"][0], false);
        assert_eq!(comparison(&[], &[])["max_delta"], 0.0);
    }
}
