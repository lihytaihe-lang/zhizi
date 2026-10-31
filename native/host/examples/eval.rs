//! Bucketed evaluation (local): types every case through the host and reports, per
//! bucket, how often the target is the first candidate and how often it is on the first
//! page. Cases are JSON lines `{"id","bucket","context","keys","target"}` (see
//! scripts/eval/build_external_set.py and the owner set built locally).
//!
//!   cargo run --release --example eval -- <shared> <fresh user dir> <cases.jsonl> <out.jsonl>
//! with ZHIZI_ORT / ZHIZI_MODEL / ZHIZI_ITOS for 顺手·知字 (omit them to measure the
//! decoder alone), and ZHIZI_OWNER_RECORDS for the owner's settled choices. Nothing is
//! committed by default, so the user dictionary learns nothing.
//! ZHIZI_EVAL_SEQUENTIAL=1 scores each case before selecting its target and committing;
//! ZHIZI_LEARNING=zhixu|rime chooses the learning provider. This mode requires an empty
//! user directory and no ZHIZI_OWNER_RECORDS. Cases stay in file order; optional
//! `segments: [{"text": ...}]` restore segment picks when the whole target is absent.
//! `selectable` reports `selected`, a 1-based `page` (null for unavailable/segment picks),
//! and `segmentPages` for successful segment picks. After every successful commit both
//! providers wait 5 seconds for Zhixu's retained-text learning window before continuing.
//! ZHIZI_ZHIXU_WHOLE=1 uses the product trial switch: Zhixu handles input and
//! learning inside the Mixer. Without it the default inner decoder remains Rime.
//! ZHIZI_EVAL_MEMORY=1 reports macOS RSS/peak bytes and samples again after two worker checks.
use std::collections::BTreeMap;
use std::io::Write;
use zhizi_host::{Ablation, Host, HostConfig, Key, View};

#[path = "eval_baselines/mod.rs"]
mod baselines;

#[derive(Default)]
struct Tally {
    cases: usize,
    first: usize,
    page: usize,
    unselectable: usize,
}

fn empty_user_directory(path: &std::path::Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput,
                "sequential replay requires a real user directory"));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => std::fs::create_dir_all(path)?,
        Err(error) => return Err(error),
        _ => {}
    }
    if !std::fs::symlink_metadata(path)?.is_dir()
        || std::fs::read_dir(path)?.next().is_some()
    {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput,
            "sequential replay requires an empty, real user directory; use a new path"));
    }
    Ok(())
}

fn wait_for_ranking(host: &mut Host, mut view: View, wait_ticks: usize) -> View {
    for _ in 0..wait_ticks {
        if host.model_observation().outcome != "pending" {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
        if let Some(late) = host.poll_ranking() {
            view = late;
        }
    }
    view
}

fn find_candidate(host: &mut Host, view: &mut View, text: &str, wait_ticks: usize) -> Option<usize> {
    loop {
        if let Some(index) = view.candidates.iter().take(5).position(|c| c.text == text) {
            return Some(index);
        }
        if view.candidates.is_empty() {
            return None;
        }
        // The native last-page flag can coexist with a reachable Mixer overflow page.
        let previous = view.page;
        let next = host.process(Key::PageDown, 0, false);
        *view = wait_for_ranking(host, next, wait_ticks);
        if view.page <= previous {
            return None;
        }
    }
}

fn select_target(host: &mut Host, mut view: View, case: &serde_json::Value, wait_ticks: usize) -> serde_json::Value {
    let target = case["target"].as_str().unwrap_or_default();
    let unavailable = serde_json::json!({"selected":false,"page":null});
    if let Some(index) = find_candidate(host, &mut view, target, wait_ticks) {
        let page = view.page + 1;
        // Digits select the displayed slot even if paging kept a different slot highlighted.
        let after = host.process(Key::Text((index + 1).to_string()), 0, false);
        return if after.commit == target && after.preedit.is_empty() {
            serde_json::json!({"selected":true,"page":page})
        } else { unavailable };
    }
    let Some(segments) = case["segments"].as_array() else { return unavailable };
    let texts: Option<Vec<&str>> = segments.iter().map(|segment| segment["text"].as_str().filter(|text| !text.is_empty())).collect();
    let Some(texts) = texts else { return unavailable };
    if texts.is_empty() || texts.concat() != target {
        return unavailable;
    }
    while view.page > 0 {
        let previous = view.page;
        let next = host.process(Key::PageUp, 0, false);
        view = wait_for_ranking(host, next, wait_ticks);
        if view.page >= previous { return unavailable; }
    }
    let mut pages = Vec::new();
    for (part, text) in texts.iter().enumerate() {
        let Some(index) = find_candidate(host, &mut view, text, wait_ticks) else { return unavailable };
        pages.push(view.page + 1);
        let next = host.process(Key::Text((index + 1).to_string()), 0, false);
        view = wait_for_ranking(host, next, wait_ticks);
        if part + 1 == texts.len() {
            if view.commit != target || !view.preedit.is_empty() { return unavailable; }
        } else if !view.commit.is_empty() || view.preedit.is_empty() {
            return unavailable;
        }
    }
    serde_json::json!({"selected":true,"page":null,"segmentPages":pages})
}

#[cfg(target_os = "macos")]
fn memory_snapshot() -> (u64, u64, u64) {
    #[repr(C)]
    #[derive(Default)]
    struct TaskInfo {
        virtual_size: u64,
        resident_size: u64,
        resident_size_max: u64,
        times: [i32; 4],
        policy: i32,
        suspend_count: i32,
    }
    #[repr(C)]
    #[derive(Default)]
    struct Timeval {
        seconds: i64,
        microseconds: i32,
    }
    #[repr(C)]
    #[derive(Default)]
    struct Rusage {
        user: Timeval,
        system: Timeval,
        maxrss: i64,
        other: [i64; 13],
    }
    unsafe extern "C" {
        static mach_task_self_: u32;
        fn task_info(task: u32, flavor: u32, info: *mut i32, count: *mut u32) -> i32;
        fn getrusage(who: i32, usage: *mut Rusage) -> i32;
    }
    let mut info = TaskInfo::default();
    let mut count = (std::mem::size_of::<TaskInfo>() / 4) as u32;
    let result = unsafe {
        task_info(
            mach_task_self_,
            20,
            (&mut info as *mut TaskInfo).cast(),
            &mut count,
        )
    };
    assert_eq!(result, 0);
    let mut usage = Rusage::default();
    assert_eq!(unsafe { getrusage(0, &mut usage) }, 0);
    (
        info.resident_size,
        info.resident_size_max,
        usage.maxrss as u64,
    )
}

fn memory_sample(phase: &str) {
    if std::env::var_os("ZHIZI_EVAL_MEMORY").is_none() {
        return;
    }
    #[cfg(target_os = "macos")]
    {
        let (rss, peak, rusage_peak) = memory_snapshot();
        eprintln!(
            "ZHIXU_EVAL_MEMORY phase={phase} rss_bytes={rss} peak_bytes={peak} rusage_peak_bytes={rusage_peak}"
        );
    }
    #[cfg(not(target_os = "macos"))]
    eprintln!("ZHIXU_EVAL_MEMORY phase={phase} unsupported platform");
}

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    run(&arguments);
}

fn run(arguments: &[String]) {
    memory_sample("start");
    assert_eq!(arguments.len(), 4, "eval <shared> <user dir> <cases.jsonl> <out.jsonl>");
    let (shared, user, cases, out) = (&arguments[0], &arguments[1], &arguments[2], &arguments[3]);
    let sequential = std::env::var("ZHIZI_EVAL_SEQUENTIAL").as_deref() == Ok("1");
    if sequential {
        assert!(std::env::var_os("ZHIZI_OWNER_RECORDS").is_none(), "sequential replay must start without OwnerStyle records");
        empty_user_directory(std::path::Path::new(user)).expect("fresh sequential user directory");
        assert!(matches!(std::env::var("ZHIZI_LEARNING").as_deref(), Ok("zhixu" | "rime") | Err(_)), "ZHIZI_LEARNING must be zhixu or rime");
    }
    // Name the inner decoder this run uses, and refuse a run that asked for another one (Q16: a
    // comparison "against Zhixu" once ran Rime against Rime because Zhixu was never switched on).
    let decoder = if std::env::var("ZHIZI_ZHIXU_WHOLE").as_deref() == Ok("1") { "zhixu" } else { "rime" };
    if let Ok(required) = std::env::var("ZHIZI_EVAL_REQUIRE_DECODER") {
        assert_eq!(required, decoder, "ZHIZI_EVAL_REQUIRE_DECODER={required} but this run uses {decoder}; set ZHIZI_ZHIXU_WHOLE=1 for zhixu");
    }
    eprintln!("eval decoder={decoder}");
    let mut host = Host::open(&HostConfig { shared_dir: shared.into(), user_dir: user.into(), app_id: "zhizi.eval".into() })
        .expect("open");
    // Host::open applies ZHIZI_ZHIXU_WHOLE through the same core entry point as the product.
    // ZHIZI_LEARNING=zhixu also enables its learning store with the default Rime inner decoder.
    if std::env::var("ZHIZI_LEARNING").as_deref() == Ok("zhixu") {
        assert!(host.set_learning(zhizi_host::LearningMode::Zhixu), "switch to zhixu learning");
    }
    memory_sample("host_open");
    // ZHIZI_FUZZY_MASK=<0..255> enables fuzzy-pinyin groups as the menu does.
    if let Some(mask) = std::env::var("ZHIZI_FUZZY_MASK").ok().and_then(|value| value.parse::<u32>().ok()) {
        assert!(host.set_fuzzy(mask), "ZHIZI_FUZZY_MASK rejected");
    }
    let model = std::env::var("ZHIZI_MODEL").is_ok();
    // Paper ablation (local): ZHIZI_EVAL_ABLATION=rerank|join applies only that part of the scores;
    // ZHIZI_EVAL_BASELINE=ngram|unigram swaps the model for a baseline scorer on the same candidates
    // (corpus: ZHIZI_BASELINE_CORPUS, the clean pre-training text).
    match std::env::var("ZHIZI_EVAL_ABLATION").as_deref() {
        Ok("rerank") => host.set_ablation(Ablation::RerankOnly),
        Ok("join") => host.set_ablation(Ablation::JoinOnly),
        _ => {}
    }
    let baseline = std::env::var("ZHIZI_EVAL_BASELINE").ok();
    if let Some(kind) = &baseline {
        let dir = std::env::var("ZHIZI_BASELINE_CORPUS").expect("ZHIZI_BASELINE_CORPUS");
        let sources = baselines::default_sources(std::path::Path::new(&dir));
        let refs: Vec<(&std::path::Path, usize)> = sources.iter().map(|(p, n)| (p.as_path(), *n)).collect();
        eprintln!("training {kind} baseline ...");
        let counts = baselines::train(&refs);
        match kind.as_str() {
            "ngram" => host.enable_model(Box::new(baselines::NgramScorer(counts)), None),
            "unigram" => host.enable_model(Box::new(baselines::UnigramScorer(counts)), None),
            other => panic!("unknown baseline {other}"),
        }
    }
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
    if !model && baseline.is_none() && std::env::var("ZHIZI_EVAL_DUMMY_SCORER").is_ok() {
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
    let mut key_ms = std::env::var("ZHIZI_EVAL_KEY_MS").ok()
        .map(|path| std::fs::File::create(path).expect("ZHIZI_EVAL_KEY_MS file"));
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
            let started = std::time::Instant::now();
            view = host.process(Key::Text(c.to_string()), 0, false);
            // ZHIZI_EVAL_KEY_MS=<file> (local diagnosis): wall time of each typed key.
            if let Some(file) = key_ms.as_mut() {
                use std::io::Write;
                let _ = writeln!(file, "{}\t{}\t{:.3}", keys, c, started.elapsed().as_secs_f64() * 1e3);
            }
        }
        // Accuracy, not latency: a busy machine must not turn a slow model into a different answer.
        let view = wait_for_ranking(&mut host, view, wait_ticks);
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
                    .map(|i| scored.scores[i] - zhizi_host::ranking::edit_penalty() * scored.edits.get(i).copied().unwrap_or(0.0))
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
        let selectable = sequential.then(|| select_target(&mut host, view, &case, wait_ticks));
        if selectable.as_ref().is_some_and(|s| s["selected"] == true) {
            // Learning is retained only after the production deletion window, at an idle key.
            // Apply the same pause to both providers; do not bypass their normal commit path.
            // ZHIZI_EVAL_VIRTUAL_CLOCK=1 moves the clock instead of sleeping (same window, same commit path)
            if !agent_ime_core::personal_picks::advance_eval_clock(5_001) {
                std::thread::sleep(std::time::Duration::from_millis(5_001));
            }
        }
        host.process(Key::Escape, 0, false);
        let tally = tallies.entry(bucket.clone()).or_default();
        tally.cases += 1;
        tally.first += usize::from(first);
        tally.page += usize::from(on_page);
        tally.unselectable += usize::from(selectable.as_ref().is_some_and(|s| s["selected"] == false));
        let mut record = serde_json::json!({
            "id": case["id"], "bucket": bucket, "keys": keys, "target": target, "context": case["context"],
            "first": first, "onPage": on_page, "page": page, "model": outcome,
        });
        if let Some(selectable) = selectable {
            record["selectable"] = selectable;
        }
        if let Some((alternatives, deeper, totals, slots)) = trace {
            record["alternatives"] = serde_json::json!(alternatives);
            // What the model scores: the page candidates in its masks, and the alternatives.
            record["scored"] = serde_json::json!(slots);
            record["totals"] = serde_json::json!(totals);
            record["onNextPages"] = serde_json::json!(deeper);
        }
        writeln!(output, "{record}").unwrap();
    }
    memory_sample("workload_complete");
    if std::env::var_os("ZHIZI_EVAL_MEMORY").is_some() {
        // Two worker checks after Escape releases the last displayed generation.
        std::thread::sleep(std::time::Duration::from_millis(1600));
        memory_sample("quiescent");
    }
    let mut all = Tally::default();
    println!("bucket\tcases\ttop1\ttop5\tmodel={model}\tdecoder={decoder}");
    for (bucket, tally) in &tallies {
        all.cases += tally.cases;
        all.first += tally.first;
        all.page += tally.page;
        all.unselectable += tally.unselectable;
        println!("{bucket}\t{}\t{}", tally.cases, percentages(tally));
    }
    println!("all\t{}\t{}", all.cases, percentages(&all));
    if sequential {
        println!("unselectable {}", all.unselectable);
    }
}

fn percentages(tally: &Tally) -> String {
    let share = |count: usize| 100.0 * count as f64 / tally.cases.max(1) as f64;
    format!("{:.1}%\t{:.1}%", share(tally.first), share(tally.page))
}

#[cfg(all(test, feature = "librime"))]
mod sequential_tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn synthetic_replay_child() {
        let Some(root) = std::env::var_os("ZHIZI_EVAL_TEST_DIR") else { return };
        let root = Path::new(&root);
        run(&[
            root.join("shared").to_str().unwrap().into(),
            root.join("user").to_str().unwrap().into(),
            root.join("cases.jsonl").to_str().unwrap().into(),
            root.join("out.jsonl").to_str().unwrap().into(),
        ]);
        let mut host = Host::open(&HostConfig {
            shared_dir: root.join("shared"), user_dir: root.join("user"), app_id: "zhizi.eval".into(),
        }).unwrap();
        let mut view = View::default();
        for c in "zai".chars() {
            view = host.process(Key::Text(c.to_string()), 0, false);
        }
        // Mixer may preserve a native last-page flag while another page remains reachable.
        view.is_last_page = true;
        assert!(find_candidate(&mut host, &mut view, "仔", 40).is_some());
        assert_eq!(view.page, 1);
        host.process(Key::Escape, 0, false);
    }

    #[test]
    fn sequential_user_directory_must_be_new_or_empty() {
        let root = std::env::temp_dir().canonicalize().unwrap().join(format!("zhixu-l4-empty-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        let user = root.join("user");
        empty_user_directory(&user).unwrap();
        empty_user_directory(&user).unwrap();
        std::fs::write(user.join("sentinel"), "keep").unwrap();
        assert!(empty_user_directory(&user).is_err());
        assert_eq!(std::fs::read_to_string(user.join("sentinel")).unwrap(), "keep");
        #[cfg(unix)]
        {
            let link = root.join("link");
            std::os::unix::fs::symlink(&user, &link).unwrap();
            assert!(empty_user_directory(&link).is_err());
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn sequential_replay_learns_segments_and_counts_unselectable_cases() {
        for mode in ["zhixu", "rime"] {
            let root = std::env::temp_dir().canonicalize().unwrap().join(format!(
                "zhixu-l4-{mode}-{}-{}", std::process::id(),
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos(),
            ));
            let shared = root.join("shared");
            std::fs::create_dir_all(&shared).unwrap();
            std::fs::write(shared.join("default.yaml"),
                "config_version: '1.0'\nschema_list:\n  - schema: agent_pinyin\nmenu:\n  page_size: 5\n").unwrap();
            std::fs::write(shared.join("agent_pinyin.schema.yaml"), r#"schema:
  schema_id: agent_pinyin
  name: Synthetic replay
  version: '1.0'
engine:
  processors: [speller, selector, navigator, express_editor]
  segmentors: [abc_segmentor, fallback_segmentor]
  translators: [script_translator]
speller:
  alphabet: abcdefghijklmnopqrstuvwxyz
  delimiter: " '"
translator:
  dictionary: agent_pinyin
  enable_sentence: false
  enable_completion: false
  enable_user_dict: true
  enable_encoder: true
menu:
  page_size: 5
"#).unwrap();
            std::fs::write(shared.join("agent_pinyin.dict.yaml"),
                "---\nname: agent_pinyin\nversion: '1.0'\nsort: by_weight\n...\n再\tzai\t900\n在\tzai\t800\n载\tzai\t700\n灾\tzai\t600\n栽\tzai\t500\n宰\tzai\t400\n仔\tzai\t300\n高\tgao\t900\n搞\tgao\t800\n").unwrap();
            std::fs::write(shared.join("agent_exact_readings.tsv"),
                "仔\tzai\n再\tzai\n在\tzai\n宰\tzai\n搞\tgao\n栽\tzai\n灾\tzai\n载\tzai\n高\tgao\n").unwrap();
            let cases = [
                serde_json::json!({"id":"learn", "keys":"zaigao", "target":"再搞", "context":"停了，", "segments":[{"text":"再","index":1,"page":0},{"text":"搞","index":2,"page":0}]}),
                serde_json::json!({"id":"repeat", "keys":"zaigao", "target":"再搞", "context":"停了，"}),
                serde_json::json!({"id":"missing", "keys":"zai", "target":"座"}),
                serde_json::json!({"id":"bad-segment", "keys":"zai", "target":"座", "segments":[{"text":"座"}]}),
                serde_json::json!({"id":"page-two", "keys":"zai", "target":"宰"}),
                serde_json::json!({"id":"page-one", "keys":"gao", "target":"搞"}),
                serde_json::json!({"id":"mismatched-segments", "keys":"zai", "target":"座", "segments":[{"text":"再"}]}),
                serde_json::json!({"id":"incomplete-segments", "keys":"zaigao", "target":"再座", "segments":[{"text":"再"},{"text":"座"}]}),
            ];
            std::fs::write(root.join("cases.jsonl"), cases.iter().map(|c| format!("{c}\n")).collect::<String>()).unwrap();
            let mut command = std::process::Command::new(std::env::current_exe().unwrap());
            command.args(["--exact", "sequential_tests::synthetic_replay_child", "--nocapture"])
                .env("ZHIZI_EVAL_TEST_DIR", &root)
                .env("ZHIZI_EVAL_SEQUENTIAL", "1")
                .env("ZHIZI_LEARNING", mode)
                .env_remove("ZHIZI_OWNER_RECORDS")
                .env_remove("ZHIZI_MODEL")
                .env_remove("ZHIZI_EVAL_BASELINE")
                .env_remove("ZHIZI_EVAL_DUMMY_SCORER");
            let output = command.output().unwrap();
            assert!(output.status.success(), "{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
            let rows: Vec<serde_json::Value> = std::fs::read_to_string(root.join("out.jsonl")).unwrap()
                .lines().map(|line| serde_json::from_str(line).unwrap()).collect();
            assert_eq!(rows.len(), 8);
            // This valid Rime fixture composes 再高 first; 再 remains selectable as a segment.
            assert_eq!(rows[0]["page"][0], "再高");
            assert_eq!(rows[0]["first"], false);
            assert_eq!(rows[0]["onPage"], false);
            assert_eq!(rows[0]["selectable"], serde_json::json!({"selected":true,"page":null,"segmentPages":[1,1]}));
            if mode == "zhixu" {
                assert_eq!(rows[1]["page"][0], "再搞");
                assert_eq!(rows[1]["first"], true);
                assert_eq!(rows[1]["selectable"]["page"], 1);
            } else {
                println!("synthetic Rime repeat: {}", rows[1]);
            }
            for row in &rows[2..4] {
                assert_eq!(row["selectable"]["selected"], false);
            }
            assert_eq!(rows[4]["onPage"], false);
            assert_eq!(rows[4]["selectable"], serde_json::json!({"selected":true,"page":2}));
            assert_eq!(rows[5]["selectable"]["selected"], true);
            assert!(rows[6..].iter().all(|row| row["selectable"]["selected"] == false));
            let unavailable = rows.iter().filter(|r| r["selectable"]["selected"] == false).count();
            assert!(String::from_utf8_lossy(&output.stdout).contains(&format!("unselectable {unavailable}")));
            let before = std::fs::read(root.join("out.jsonl")).unwrap();
            let refused = command.output().unwrap();
            assert!(!refused.status.success());
            assert!(String::from_utf8_lossy(&refused.stderr).contains("fresh sequential user directory"));
            assert_eq!(std::fs::read(root.join("out.jsonl")).unwrap(), before);
            command.env("ZHIZI_OWNER_RECORDS", root.join("absent-records.jsonl"));
            let refused = command.output().unwrap();
            assert!(!refused.status.success());
            assert!(String::from_utf8_lossy(&refused.stderr).contains("without OwnerStyle records"));
            command.env_remove("ZHIZI_OWNER_RECORDS").env("ZHIZI_EVAL_SEQUENTIAL", "0");
            // A separate process still uses this synthetic schema, with its own empty user directory.
            let independent = root.join("independent");
            std::fs::create_dir(&independent).unwrap();
            std::fs::rename(root.join("user"), independent.join("sequential-user")).unwrap();
            std::fs::create_dir(root.join("user")).unwrap();
            let output = command.output().unwrap();
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
            let independent_rows: Vec<serde_json::Value> = std::fs::read_to_string(root.join("out.jsonl")).unwrap()
                .lines().map(|line| serde_json::from_str(line).unwrap()).collect();
            assert!(independent_rows.iter().all(|row| row.get("selectable").is_none()));
            assert_eq!(independent_rows[0]["page"], independent_rows[1]["page"]);
            std::fs::remove_dir_all(root).unwrap();
        }
    }
}

#[cfg(all(test, feature = "librime"))]
mod decoder_switch_tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn decoder_switch_child() {
        let Some(root) = std::env::var_os("ZHIXU_Y3_EVAL_TEST_DIR") else { return };
        let root = Path::new(&root);
        // The trial now hands off at an idle boundary after background loading.
        // Keep this assertion about the ready eval route, not cold-start timing.
        if std::env::var("ZHIZI_ZHIXU_WHOLE").as_deref() == Ok("1") {
            let mut host = Host::open(&HostConfig { shared_dir: root.join("shared"),
                user_dir: root.join("user"), app_id: "zhizi.eval".into() }).unwrap();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                host.process(Key::Text("n".into()), 0, false);
                let view = host.process(Key::Text("i".into()), 0, false);
                host.process(Key::Escape, 0, false);
                if view.candidates.first().is_some_and(|c| c.text == "你") { break; }
                assert!(std::time::Instant::now() < deadline, "background decoder never became ready");
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
        run(&[
            root.join("shared").to_str().unwrap().into(),
            root.join("user").to_str().unwrap().into(),
            root.join("cases.jsonl").to_str().unwrap().into(),
            root.join("out.jsonl").to_str().unwrap().into(),
        ]);
    }

    #[test]
    fn whole_switch_replaces_rime_input_in_eval() {
        for whole in [None, Some("0"), Some("true"), Some("1")] {
            let root = std::env::temp_dir().canonicalize().unwrap().join(format!(
                "zhixu-y3-eval-{}-{}", std::process::id(),
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos(),
            ));
            let shared = root.join("shared");
            std::fs::create_dir_all(&shared).unwrap();
            std::fs::create_dir(root.join("user")).unwrap();
            std::fs::write(shared.join("default.yaml"),
                "config_version: '1.0'\nschema_list:\n  - schema: agent_pinyin\nmenu:\n  page_size: 5\n").unwrap();
            // Rime initializes normally but has no spelling processor. A whole
            // candidate overlay cannot type ni; only replacing the inner decoder can.
            std::fs::write(shared.join("agent_pinyin.schema.yaml"), r#"schema:
  schema_id: agent_pinyin
  name: Decoder switch fixture
  version: '1.0'
engine:
  processors: [ascii_composer]
  segmentors: [abc_segmentor, fallback_segmentor]
  translators: [script_translator]
speller:
  alphabet: abcdefghijklmnopqrstuvwxyz
translator:
  dictionary: agent_pinyin
  enable_user_dict: false
menu:
  page_size: 5
"#).unwrap();
            std::fs::write(shared.join("agent_pinyin.dict.yaml"),
                "---\nname: agent_pinyin\nversion: '1.0'\nsort: by_weight\n...\n你\tni\t900\n").unwrap();
            std::fs::write(shared.join("agent_exact_readings.tsv"), "你\tni\n").unwrap();
            std::fs::write(root.join("cases.jsonl"),
                "{\"id\":\"decoder-switch\",\"keys\":\"ni\",\"target\":\"你\"}\n").unwrap();
            let mut command = std::process::Command::new(std::env::current_exe().unwrap());
            command.args(["--exact", "decoder_switch_tests::decoder_switch_child", "--nocapture"])
                .env("ZHIXU_Y3_EVAL_TEST_DIR", &root)
                .env_remove("ZHIZI_ZHIXU_WHOLE")
                .env_remove("ZHIZI_LEARNING")
                .env_remove("ZHIZI_EVAL_SEQUENTIAL")
                .env_remove("ZHIZI_OWNER_RECORDS")
                .env_remove("ZHIZI_MODEL")
                .env_remove("ZHIZI_EVAL_BASELINE")
                .env_remove("ZHIZI_EVAL_DUMMY_SCORER");
            if let Some(whole) = whole { command.env("ZHIZI_ZHIXU_WHOLE", whole); }
            let output = command.output().unwrap();
            assert!(output.status.success(), "{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
            let row: serde_json::Value = serde_json::from_str(
                std::fs::read_to_string(root.join("out.jsonl")).unwrap().trim()).unwrap();
            if whole == Some("1") {
                assert_eq!(row["page"][0], "你", "the trial flag must replace Rime's input path");
            } else {
                assert_eq!(row["page"], serde_json::json!([]), "the default Rime path stays unchanged");
            }
            std::fs::remove_dir_all(root).unwrap();
        }
    }
}
