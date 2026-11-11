//! S1 offline candidate snapshots; no engine or installed-profile changes.
//! slot_rules <shared> <new output dir> <cases.jsonl> <current|pool> <bucket>
//! Only ZHIZI_ORT / ZHIZI_MODEL / ZHIZI_ITOS supply model files.
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::time::{Duration, Instant};
use agent_ime_core::{AimKeyEvent, AimResult, AimSession, aim_result_free, aim_session_create,
    aim_session_destroy, aim_session_process, aim_session_set_candidate_context,
    aim_session_set_preceding_text, aim_session_set_typo_correction};
use zhizi_host::ranking::{Scorer, onnx::init_runtime};
use zhizi_host::{Ablation, Host, HostConfig, Key, View};

#[derive(Clone, Deserialize, Serialize)]
struct Case {
    id: String,
    bucket: String,
    raw: String,
    context: String,
    target: String,
    source: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Candidate {
    text: String,
    reading: String,
}

fn han(text: &str) -> bool {
    !text.is_empty() && text.chars().all(|c| ('\u{3400}'..='\u{9fff}').contains(&c))
}

fn one_slip(a: &[u8], b: &[u8]) -> bool {
    if a == b { return true; }
    if a.len() == b.len() {
        let different: Vec<usize> = (0..a.len()).filter(|&i| a[i] != b[i]).collect();
        return different.len() == 1 || (different.len() == 2 && different[1] == different[0] + 1
            && a[different[0]] == b[different[1]] && a[different[1]] == b[different[0]]);
    }
    let (short, long) = if a.len() < b.len() { (a, b) } else { (b, a) };
    long.len() == short.len() + 1 && (0..long.len()).any(|i| short[..i] == long[..i] && short[i..] == long[i + 1..])
}

fn covers(candidate: &Candidate, raw: &str, bucket: &str) -> bool {
    let length = candidate.text.chars().count();
    if !han(&candidate.text) || length > 39 || raw.is_empty() || !raw.bytes().all(|c| c.is_ascii_lowercase()) {
        return false;
    }
    let reading = candidate.reading.trim().to_ascii_lowercase();
    let syllables: Vec<&str> = reading.split(|c: char| c.is_whitespace() || c == '\'')
        .filter(|s| !s.is_empty()).collect();
    if syllables.is_empty() || syllables.iter().any(|s| !s.bytes().all(|c| c.is_ascii_lowercase())) {
        return false;
    }
    let compact = syllables.concat();
    match bucket {
        "full" => compact == raw,
        "abbrev" => syllables.len() == length && length == raw.len()
            && syllables.iter().map(|s| s.as_bytes()[0] as char).collect::<String>() == raw,
        "typo" => one_slip(compact.as_bytes(), raw.as_bytes()),
        "mixed" => {
            if compact == raw { return true; }
            if syllables.len() != length { return false; }
            // A nonempty prefix of each syllable must consume exactly all input.
            let mut ends = HashSet::from([0usize]);
            for syllable in syllables {
                let mut next = HashSet::new();
                for start in ends {
                    for count in 1..=syllable.len().min(raw.len() - start) {
                        if raw[start..].starts_with(&syllable[..count]) { next.insert(start + count); }
                    }
                }
                ends = next;
            }
            ends.contains(&raw.len())
        }
        _ => false,
    }
}

fn comparable(candidates: &[Candidate], raw: &str, bucket: &str, first: &str, n: usize) -> Vec<Candidate> {
    if first.is_empty() { return Vec::new(); }
    let length = if han(first) { first.chars().count() } else {
        candidates.iter().find(|c| covers(c, raw, bucket)).map(|c| c.text.chars().count()).unwrap_or(0)
    };
    let mut seen = HashSet::new();
    candidates.iter().filter(|c| covers(c, raw, bucket) && c.text.chars().count() == length
        && seen.insert(c.text.clone())).take(n).cloned().collect()
}

fn check_scores(scores: &[f64], expected: usize) -> Result<(), String> {
    if scores.len() != expected || scores.iter().any(|s| !s.is_finite()) {
        return Err("invalid model scores".into());
    }
    Ok(())
}

fn scoring_context(context: &str, texts: &[String]) -> Result<String, String> {
    let longest = texts.iter().map(|t| t.chars().count()).max().unwrap_or(0);
    if texts.is_empty() || longest > 39 || texts.iter().any(|t| t.is_empty()) { return Err("candidate outside model window".into()); }
    let context: Vec<char> = context.chars().collect();
    let keep = context.len().min(39 - longest);
    Ok(context[context.len() - keep..].iter().collect())
}

fn extract_scores(values: &[f32], width: usize, prefix: usize, texts: &[String]) -> Result<Vec<f64>, String> {
    if prefix == 0 || values.len() != width * texts.len()
        || texts.iter().any(|t| t.is_empty() || prefix - 1 + t.chars().count() > width) {
        return Err("invalid score positions".into());
    }
    let scores: Vec<f64> = texts.iter().enumerate().map(|(row, text)| {
        (prefix - 1..prefix - 1 + text.chars().count()).map(|i| values[row * width + i] as f64).sum()
    }).collect();
    check_scores(&scores, texts.len())?;
    Ok(scores)
}

// OnnxScorer's protocol with an explicit CPU policy; ORT's default can compile Core ML on macOS.
struct CpuScorer {
    session: ort::session::Session,
    vocabulary: HashMap<String, i32>,
}

impl CpuScorer {
    fn load(model: &Path, itos: &Path, output: &Path) -> Result<Self, String> {
        let itos: Vec<String> = serde_json::from_slice(&fs::read(itos).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        if itos.get(..3) != Some(&["<pad>".to_string(), "<bos>".to_string(), "<unk>".to_string()][..]) {
            return Err("invalid model vocabulary".into());
        }
        let session = ort::session::Session::builder().map_err(|e| e.to_string())?
            .with_auto_device(ort::session::builder::AutoDevicePolicy::PreferCPU).map_err(|e| e.to_string())?
            .with_parallel_execution(false).map_err(|e| e.to_string())?
            .with_intra_threads(1).map_err(|e| e.to_string())?
            .with_inter_threads(1).map_err(|e| e.to_string())?
            .with_profiling(output.join("cpu-provider")).map_err(|e| e.to_string())?
            .commit_from_file(model).map_err(|e| e.to_string())?;
        let mut scorer = Self { session, vocabulary: itos.into_iter().enumerate().map(|(i, text)| (text, i as i32)).collect() };
        scorer.score("这是上文", &["候选".into(), "测试".into()])?;
        let profile = scorer.session.end_profiling().map_err(|e| e.to_string())?;
        let events: Vec<serde_json::Value> = serde_json::from_slice(&fs::read(profile).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        let providers: HashSet<String> = events.iter().filter_map(|e| e["args"]["provider"].as_str().map(str::to_string)).collect();
        if providers != HashSet::from(["CPUExecutionProvider".into()]) { return Err("CPU provider verification failed".into()); }
        fs::write(output.join("provider.json"), serde_json::to_vec(&providers).unwrap()).map_err(|e| e.to_string())?;
        Ok(scorer)
    }
}

impl Scorer for CpuScorer {
    fn score(&mut self, context: &str, texts: &[String]) -> Result<Vec<f64>, String> {
        let context = scoring_context(context, texts)?;
        let token = |c: char| self.vocabulary.get(c.encode_utf8(&mut [0; 4]) as &str).copied().unwrap_or(2);
        let prefix: Vec<i32> = std::iter::once(1).chain(context.chars().map(token)).collect();
        let mut scores = Vec::new();
        for chunk in texts.chunks(8) {
            let length = prefix.len() + chunk.iter().map(|t| t.chars().count()).max().unwrap();
            let mut tokens = vec![0i32; chunk.len() * length];
            for (row, text) in chunk.iter().enumerate() {
                for (column, id) in prefix.iter().copied().chain(text.chars().map(token)).enumerate() {
                    tokens[row * length + column] = id;
                }
            }
            let input = ort::value::Tensor::from_array(([chunk.len(), length], tokens)).map_err(|e| e.to_string())?;
            let outputs = self.session.run(ort::inputs!["tokens" => input]).map_err(|e| e.to_string())?;
            let (shape, probabilities) = outputs["logprobs"].try_extract_tensor::<f32>().map_err(|e| e.to_string())?;
            if shape.len() != 2 || shape[0] as usize != chunk.len() || shape[1] as usize != length - 1 {
                return Err("unexpected model output shape".into());
            }
            scores.extend(extract_scores(probabilities, length - 1, prefix.len(), chunk)?);
        }
        check_scores(&scores, texts.len())?;
        Ok(scores)
    }
}

fn write_row(stream: &mut impl Write, row: &impl Serialize) -> Result<(), String> {
    serde_json::to_writer(&mut *stream, row).map_err(|e| e.to_string())?;
    writeln!(stream).map_err(|e| e.to_string())
}

fn valid_case(case: &Case, bucket: &str) -> bool {
    !case.id.is_empty() && case.bucket == bucket && case.source == "ext" && !case.raw.is_empty()
        && case.raw.bytes().all(|c| c.is_ascii_lowercase()) && !case.target.is_empty()
        && case.context.chars().count() <= 32
}

fn load_cases(path: &Path, bucket: &str) -> Result<Vec<Case>, String> {
    let mut seen = HashSet::new();
    let mut cases = Vec::new();
    for line in fs::read_to_string(path).map_err(|e| e.to_string())?.lines().filter(|l| !l.trim().is_empty()) {
        let case: Case = serde_json::from_str(line).map_err(|e| e.to_string())?;
        if !valid_case(&case, bucket) || !seen.insert(case.id.clone()) {
            return Err("invalid case (no silent filtering)".into());
        }
        cases.push(case);
    }
    if cases.is_empty() { return Err("no cases".into()); }
    Ok(cases)
}

struct AuditedScorer {
    inner: CpuScorer,
    failed: Arc<AtomicBool>,
    tail: Arc<AtomicBool>,
}

impl Scorer for AuditedScorer {
    fn score(&mut self, context: &str, texts: &[String]) -> Result<Vec<f64>, String> {
        if self.tail.load(Ordering::SeqCst) { return Ok(vec![0.0; texts.len()]); }
        let result = self.inner.score(context, texts).and_then(|scores| {
            check_scores(&scores, texts.len())?;
            Ok(scores)
        });
        if result.is_err() { self.failed.store(true, Ordering::SeqCst); }
        result
    }
}

fn ready(host: &mut Host, mut view: View) -> Result<View, String> {
    let started = Instant::now();
    while host.model_observation().outcome == "pending" {
        if started.elapsed() > Duration::from_secs(120) { return Err("model timed out".into()); }
        std::thread::sleep(Duration::from_millis(1));
        if let Some(late) = host.poll_ranking() { view = late; }
    }
    Ok(view)
}

fn current(shared: &Path, user: &Path, cases: &[Case], scorer: CpuScorer, stream: &mut impl Write) -> Result<(), String> {
    let failed = Arc::new(AtomicBool::new(false));
    let tail = Arc::new(AtomicBool::new(false));
    let mut host = Host::open(&HostConfig { shared_dir: shared.into(), user_dir: user.into(), app_id: "zhizi.slot-rules".into() })?;
    host.enable_model(Box::new(AuditedScorer { inner: scorer, failed: Arc::clone(&failed), tail: Arc::clone(&tail) }), None);
    for (i, case) in cases.iter().enumerate() {
        tail.store(false, Ordering::SeqCst);
        host.set_ablation(Ablation::None);
        host.process(Key::Escape, 0, false);
        host.clear_context();
        host.note_committed(&case.context);
        let mut view = View::default();
        for c in case.raw.chars() { view = host.process(Key::Text(c.to_string()), 0, false); }
        view = ready(&mut host, view)?;
        let observation = host.model_observation().clone();
        let model_ready = !host.last_group().scores.is_empty() || !host.last_composition().scores.is_empty();
        if failed.load(Ordering::SeqCst) || (!model_ready && observation.outcome != "no_group") {
            return Err(format!("current model failed at case {}", i + 1));
        }
        let mut all: Vec<String> = view.candidates.iter().map(|c| c.text.clone()).collect();
        let first_page = all.clone();
        let expected_tail: Vec<String> = host.peek_following_pages(3).iter()
            .flat_map(|p| p.text.split('\t').map(str::to_string)).collect();
        // Enumerate native tail pages without changing any generation knob. C3 is page-zero only;
        // JoinOnly suppresses C1/C2 application, and dummy scores avoid wasted tail inference.
        host.set_ablation(Ablation::JoinOnly);
        tail.store(true, Ordering::SeqCst);
        for _ in 0..60 {
            if view.is_last_page || view.candidates.is_empty() { break; }
            let before = view.page;
            let next = host.process(Key::PageDown, 0, false);
            view = ready(&mut host, next)?;
            if view.page <= before { return Err("tail page did not advance".into()); }
            if !host.last_composition().scores.is_empty() || !host.last_group().scores.is_empty() {
                return Err("model modified a native tail page".into());
            }
            all.extend(view.candidates.iter().map(|c| c.text.clone()));
        }
        if all[first_page.len()..].get(..expected_tail.len()) != Some(expected_tail.as_slice()) {
            return Err("tail enumeration disagrees with native peek".into());
        }
        let mut row = serde_json::to_value(case).unwrap();
        row["candidates"] = serde_json::json!(all);
        row["first_page"] = serde_json::json!(first_page);
        row["model_ready"] = serde_json::json!(model_ready);
        row["model_ms"] = serde_json::json!(observation.latency_ms);
        row["outcome"] = serde_json::json!(observation.outcome);
        row["rank_all"] = serde_json::json!(all.iter().position(|t| t == &case.target).map(|j| j as i64).unwrap_or(-1));
        write_row(stream, &row)?;
        if (i + 1) % 50 == 0 { eprintln!("current {}/{}", i + 1, cases.len()); }
    }
    Ok(())
}

// Small owning wrapper around the existing C ABI: preserve the reading metadata lost by Host::View.
struct Core(*mut AimSession);

impl Drop for Core {
    fn drop(&mut self) { unsafe { aim_session_destroy(self.0) }; }
}

impl Core {
    fn open(shared: &Path, user: &Path) -> Result<Self, String> {
        let shared = shared.to_str().ok_or("non-UTF8 shared path")?;
        let user = user.to_str().ok_or("non-UTF8 user path")?;
        let app = "zhizi.slot-rules";
        let session = unsafe { aim_session_create(shared.as_ptr(), shared.len(), user.as_ptr(), user.len(), app.as_ptr(), app.len()) };
        if session.is_null() { return Err("core failed to open".into()); }
        let core = Self(session);
        if unsafe { aim_session_set_typo_correction(session, 1) } != 0
            || unsafe { aim_session_set_candidate_context(session, std::ptr::null(), 0, b"[]".as_ptr(), 2, 0, 0) } != 0 {
            return Err("core configuration failed".into());
        }
        Ok(core)
    }

    fn key(&mut self, kind: u32, text: &str) -> Result<(Vec<Candidate>, usize, bool), String> {
        let event = AimKeyEvent { kind, text: text.as_ptr(), text_len: text.len(), modifiers: 0,
            security_state: 0, delivery_id: 0 };
        let mut result = AimResult::default();
        if unsafe { aim_session_process(self.0, &event, &mut result) } != 0 { return Err("core key failed".into()); }
        let copy = |pointer: *const u8, length: usize| {
            if length == 0 { String::new() } else {
                String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(pointer, length) }).into_owned()
            }
        };
        let candidates = if result.candidate_count == 0 { Vec::new() } else {
            unsafe { std::slice::from_raw_parts(result.candidates, result.candidate_count) }.iter()
                .map(|c| Candidate { text: copy(c.text, c.text_len), reading: copy(c.reading_key, c.reading_key_len) }).collect()
        };
        let page = result.page_no;
        let last = result.is_last_page != 0;
        unsafe { aim_result_free(&mut result) };
        Ok((candidates, page, last))
    }

    fn pool(&mut self, case: &Case) -> Result<Vec<Candidate>, String> {
        self.key(6, "")?;
        if unsafe { aim_session_set_preceding_text(self.0, case.context.as_ptr(), case.context.len()) } != 0 {
            return Err("core context failed".into());
        }
        let mut view = (Vec::new(), 0, true);
        for c in case.raw.chars() { view = self.key(0, &c.to_string())?; }
        let mut all = view.0.clone();
        for _ in 0..60 {
            if view.2 || view.0.is_empty() { break; }
            let before = view.1;
            view = self.key(8, "")?;
            if view.1 <= before { return Err("pool page did not advance".into()); }
            all.extend(view.0.iter().cloned());
        }
        Ok(all)
    }
}

fn pool(shared: &Path, user: &Path, cases: &[Case], mut scorer: CpuScorer, current_path: &Path,
        stream: &mut impl Write) -> Result<(), String> {
    let mut baseline = HashMap::new();
    for line in fs::read_to_string(current_path).map_err(|e| e.to_string())?.lines() {
        let row: serde_json::Value = serde_json::from_str(line).map_err(|e| e.to_string())?;
        let case: Case = serde_json::from_value(row.clone()).map_err(|e| e.to_string())?;
        if baseline.insert(case.id.clone(), row).is_some() { return Err("duplicate baseline ID".into()); }
    }
    if baseline.len() != cases.len() { return Err("baseline case count mismatch".into()); }
    let mut core = Core::open(shared, user)?;
    for (i, case) in cases.iter().enumerate() {
        let old = baseline.get(&case.id).ok_or("missing baseline ID")?;
        for field in ["bucket", "raw", "context", "target", "source"] {
            if old[field] != serde_json::to_value(case).unwrap()[field] { return Err("baseline fields changed".into()); }
        }
        let first = old["candidates"][0].as_str().unwrap_or("");
        let candidates = core.pool(case)?;
        let texts_with_readings = comparable(&candidates, &case.raw, &case.bucket, first, 40);
        let texts: Vec<String> = texts_with_readings.iter().map(|c| c.text.clone()).collect();
        let started = Instant::now();
        let scores = if texts.is_empty() { Vec::new() } else { scorer.score(&case.context, &texts)? };
        check_scores(&scores, texts.len())?;
        let mut row = serde_json::to_value(case).unwrap();
        row["pool_size"] = serde_json::json!(candidates.len());
        row["complete_count"] = serde_json::json!(candidates.iter().filter(|c| covers(c, &case.raw, &case.bucket)).count());
        row["comparable_count"] = serde_json::json!(comparable(&candidates, &case.raw, &case.bucket, first, usize::MAX).len());
        row["pool"] = serde_json::json!(candidates);
        row["scored"] = serde_json::json!(texts_with_readings);
        row["texts"] = serde_json::json!(texts);
        row["scores"] = serde_json::json!(scores);
        row["model_ms"] = serde_json::json!(started.elapsed().as_secs_f64() * 1000.0);
        write_row(stream, &row)?;
        if (i + 1) % 50 == 0 { eprintln!("pool {}/{}", i + 1, cases.len()); }
    }
    Ok(())
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if !(args.len() == 5 && args[3] == "current" || args.len() == 6 && args[3] == "pool")
        || !["full", "abbrev", "mixed", "typo"].contains(&args[4].as_str()) {
        return Err("usage: slot_rules <shared> <new output dir> <cases> <current|pool> <bucket> [current.jsonl]".into());
    }
    // No worker exists yet. The only generation override is the requested abbreviation pool.
    unsafe {
        for name in ["ZX_ABBREV_WORD_PAGES", "ZX_PEEK_PAGES", "ZX_ABBREV_SINGLE_INTERLEAVE"] {
            std::env::remove_var(name);
        }
        if args[3] == "pool" && args[4] == "abbrev" {
            std::env::set_var("ZX_ABBREV_WORD_PAGES", "20");
            std::env::set_var("ZX_PEEK_PAGES", "60");
        }
    }
    let cases = load_cases(Path::new(&args[2]), &args[4])?;
    let variable = |name| std::env::var(name).map_err(|_| format!("missing {name}"));
    init_runtime(Path::new(&variable("ZHIZI_ORT")?))?;
    let output = Path::new(&args[1]);
    fs::create_dir(output).map_err(|e| e.to_string())?;
    let user = output.join("user");
    fs::create_dir(&user).map_err(|e| e.to_string())?;
    let scorer = CpuScorer::load(Path::new(&variable("ZHIZI_MODEL")?), Path::new(&variable("ZHIZI_ITOS")?), output)?;
    let mut stream = BufWriter::new(File::options().write(true).create_new(true).open(output.join("rows.jsonl")).map_err(|e| e.to_string())?);
    if args[3] == "current" { current(Path::new(&args[0]), &user, &cases, scorer, &mut stream)?; }
    else { pool(Path::new(&args[0]), &user, &cases, scorer, Path::new(&args[5]), &mut stream)?; }
    stream.flush().map_err(|e| e.to_string())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("slot_rules: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(text: &str, reading: &str) -> Candidate {
        Candidate { text: text.into(), reading: reading.into() }
    }

    #[test]
    fn complete_readings_reject_partial_and_missing_keys() {
        assert!(covers(&candidate("你好", "ni hao"), "nihao", "full"));
        assert!(!covers(&candidate("你", "ni"), "nihao", "full"));
        assert!(!covers(&candidate("你好", ""), "nihao", "full"));
        assert!(covers(&candidate("你好", "ni'hao"), "nih", "mixed"));
        assert!(!covers(&candidate("你好", "ni hao"), "nihaox", "mixed"));
    }

    #[test]
    fn abbreviations_consume_one_initial_per_syllable() {
        assert!(covers(&candidate("测试", "ce shi"), "cs", "abbrev"));
        assert!(!covers(&candidate("测试", "cs"), "cs", "abbrev"));
        assert!(!covers(&candidate("测试", "ce shi"), "c", "abbrev"));
        assert!(!covers(&candidate("API", "a pi ai"), "apa", "abbrev"));
    }

    #[test]
    fn whole_typo_reading_accepts_one_slip_not_a_prefix_or_two_slips() {
        for raw in ["niho", "nihhao", "nigao", "niaho"] {
            assert!(covers(&candidate("你好", "ni hao"), raw, "typo"), "{raw}");
        }
        assert!(!covers(&candidate("你", "ni"), "nihao", "typo"));
        assert!(!covers(&candidate("你好", "ni hao"), "nihox", "typo"));
        assert!(one_slip(b"nihao", b"nihao"));
    }

    #[test]
    fn filter_before_limit_and_keep_only_head_length_without_labels() {
        let candidates = [candidate("你", "ni"), candidate("你好", "ni hao"),
            candidate("尼豪", "ni hao"), candidate("你好", "ni hao"), candidate("拟好", "ni hao")];
        let group = comparable(&candidates, "nihao", "full", "你好", 2);
        assert_eq!(group.iter().map(|c| c.text.as_str()).collect::<Vec<_>>(), ["你好", "尼豪"]);
        assert!(comparable(&candidates, "nihao", "full", "你", 40).is_empty());
        assert_eq!(comparable(&candidates, "nihao", "full", "OpenAI", 40).len(), 3);
    }

    #[test]
    fn bad_scores_fail_closed() {
        assert!(check_scores(&[f64::NAN], 1).is_err());
        assert!(check_scores(&[0.0], 2).is_err());
        assert!(check_scores(&[-1.0, -2.0], 2).is_ok());
    }

    #[test]
    fn scoring_window_uses_unicode_and_one_shared_suffix() {
        let texts = ["测试".into(), "长".repeat(10)];
        let context = "甲".repeat(35) + "乙丙丁戊己";
        let clipped = scoring_context(&context, &texts).unwrap();
        assert_eq!(clipped.chars().count(), 29);
        assert!(clipped.ends_with("乙丙丁戊己"));
        assert!(scoring_context("上文", &["".into()]).is_err());
        assert!(scoring_context("上文", &["长".repeat(40)]).is_err());
    }

    #[test]
    fn scores_gather_only_candidate_positions_per_padded_row() {
        let texts = ["两字".into(), "三个字".into()];
        let values = [10., 20., -1., -2., 99., 10., 20., -3., -4., -5.];
        assert_eq!(extract_scores(&values, 5, 3, &texts).unwrap(), [-3., -12.]);
        assert!(extract_scores(&values[..9], 5, 3, &texts).is_err());
        assert!(extract_scores(&values, 5, 0, &texts).is_err());
        assert!(extract_scores(&values, 5, 5, &texts).is_err());
        assert!(extract_scores(&[f32::NAN; 10], 5, 3, &texts).is_err());
    }

    #[test]
    fn one_key_short_cases_are_preserved_in_full_bucket() {
        let mut case = Case { id: "fixture".into(), bucket: "full".into(), raw: "a".into(),
            context: "上文".into(), target: "啊".into(), source: "ext".into() };
        assert!(valid_case(&case, "full"));
        assert!(covers(&candidate("啊", "a"), "a", "full"));
        case.raw.clear();
        assert!(!valid_case(&case, "full"));
        case.raw = "API".into();
        assert!(!valid_case(&case, "full"));
    }
}
