//! Reproducible local timings against the pinned, public dictionary sources.

use super::*;
use crate::ranking::{CorrectionSettings, with_correction_settings};
use crate::zhixu::{Index, fallback, loader, with_index};
use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

// Existing mixer regression/probe inputs, kept literal so the benchmark cannot
// quietly change its workload when a corpus or the runtime dictionaries change.
const INPUTS: &[&str] = &[
    "dangqianzhetaimacbookproa",
    "womengaichengxiawuchaliao",
    "duoxianchengdeyilaijiao",
    "jxlzuoshenmzhegexiangmu",
    "dangqianzhetaigithuba",
    "dangqianzhetaiiphonea",
    "womxianchulizhezhong",
    "daoshihwoxuanyixuan",
    "jiexialaizuoshenme",
    "woxiangchifanyihou",
    "daoaonishuowogai",
    "nikkyoushenmwent",
    "shijuerengyongls",
    "jiexialzuoshenm",
    "zhenmeixiangdao",
    "zhenmeixinagdao",
    "dangqianzhetai",
    "daoaonishuoyou",
    "fenxidtoutoude",
    "woxianzaiqukan",
    "gengongjingai",
    "jiukeyizuowan",
    "shunshouzhizi",
    "woxiangchifan",
    "zuozhgexiangm",
    "dangqianapia",
    "gengguojiaai",
    "tazenmzhidao",
    "womdouzhidao",
    "woxuanyixuan",
    "wozenmzhidao",
    "xianzaizhege",
    "zhuangixnban",
    "zhuangxinban",
    "danshwojide",
    "nihaoshijie",
    "dengtawanc",
    "jiukyizuow",
    "rizhiqingk",
    "suoyouquan",
    "zaifageipy",
    "zhedifangy",
    "zhgexiangm",
    "zhhuyaoshi",
    "zhishikuai",
    "congzheli",
    "dengtawan",
    "dtoutoude",
    "fenfxiang",
    "nikaolxia",
    "shudoeshi",
    "taifuzale",
    "wozonggan",
    "zhgetiyan",
    "zhuyaoshi",
    "fenxiang",
    "gengxxin",
    "houmioan",
    "womxuyao",
    "woxikanx",
    "wozongan",
    "zainanma",
    "zennimen",
    "zhehemei",
];

// Correction-card examples and existing slips, independent of owner records.
const CORRECTION_INPUTS: &[&str] = &[
    "yijignshile", "xianzo", "shenm", "kehud", "woganj", "maidia", "sd", "nikk",
    "gengixn", "gengxxin", "houmioan", "zhuangixnban", "zhenmeixinagdao",
    "zhhuyaoshi", "wozongan", "jieidan",
];

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory =
            std::env::temp_dir().join(format!("zhixu-benchmark-{}-{stamp}", std::process::id()));
        std::fs::create_dir_all(directory.join("shared")).unwrap();
        std::fs::create_dir(directory.join("user")).unwrap();
        Self(directory)
    }

    fn install_ascii_fixture(&self) {
        let mut layers: HashMap<&str, String> = HashMap::new();
        for line in include_str!("../../tests/fixtures/zhixu_ascii_fixture.tsv")
            .lines()
            .filter(|line| !line.starts_with('#'))
        {
            let Some((layer, row)) = line.split_once('\t') else {
                continue;
            };
            let text = layers
                .entry(layer)
                .or_insert_with(|| format!("---\nname: {layer}\nsort: by_weight\n...\n"));
            text.push_str(row);
            text.push('\n');
        }
        for (layer, text) in layers {
            std::fs::write(
                self.0.join("shared").join(format!("{layer}.dict.yaml")),
                text,
            )
            .unwrap();
        }
    }

    fn copy_dictionaries(&self) {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        for entry in std::fs::read_dir(root.join("native/resources/Rime")).unwrap() {
            let entry = entry.unwrap();
            if entry.file_name().to_string_lossy().ends_with(".dict.yaml") {
                std::fs::copy(entry.path(), self.0.join("shared").join(entry.file_name())).unwrap();
            }
        }
        // Same revision as scripts/generate_heavy_garble_words.mjs. Never fetch it.
        let upstream = root.join(".build-deps/rime-pinyin-simp/0c6861ef7420ee780270ca6d993d18d4101049d0/pinyin_simp.dict.yaml");
        assert!(
            upstream.is_file(),
            "the pinned offline pinyin_simp source is required: {}",
            upstream.display()
        );
        std::fs::copy(upstream, self.0.join("shared/pinyin_simp.dict.yaml")).unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn base_page(raw: &str) -> Vec<MixedCandidate> {
    vec![MixedCandidate {
        candidate: Candidate {
            text: "测试".into(),
            comment: String::new(),
            reading_key: raw.into(),
            source_index: Some(0),
        },
        selection: Selection::Base {
            index: 0,
            text: "测试".into(),
        },
        tier: 1,
        score: 0,
    }]
}

fn p95(samples: &[Duration]) -> f64 {
    let mut samples = samples.to_vec();
    samples.sort_unstable();
    samples[(samples.len() * 95).div_ceil(100) - 1].as_secs_f64() * 1_000.0
}

fn measure(index: &Arc<Index>, raw: &str, page: &[MixedCandidate]) -> (Duration, Duration) {
    with_index(index.clone(), || {
        let abbreviated = !valid_pinyin_spelling(raw);
        let start = Instant::now();
        black_box(decode_lattice(black_box(raw), 24, abbreviated));
        let lattice = start.elapsed();
        let start = Instant::now();
        black_box(heavy_garble_candidates(black_box(raw), page, false));
        (lattice, start.elapsed())
    })
}

fn correction_pipeline(index: &Arc<Index>, raw: &str, settings: CorrectionSettings) -> (Duration, Duration) {
    with_correction_settings(settings, || with_index(index.clone(), || {
        // A fresh mixer represents an uncached keystroke. The synthetic native page
        // is identical on both sides; no model, user dictionary or user data is read.
        let mut decoder = crate::state::MockDecoder::default();
        decoder.input(raw).unwrap();
        let mut mixer = MixerDecoder::new(Box::new(decoder), "correction-benchmark".into());
        mixer.configure_typo(true);
        let started = Instant::now();
        black_box(mixer.snapshot());
        let page = started.elapsed();
        black_box(mixer.composition_alternatives());
        black_box(mixer.composition_edits());
        black_box(mixer.composition_guessed());
        (page, started.elapsed())
    }))
}

fn benchmark_correction_paths(index: &Arc<Index>) {
    let baseline = CorrectionSettings { enabled: false, ..CorrectionSettings::default() };
    let enabled = CorrectionSettings::default();
    let mut inputs = INPUTS.to_vec();
    for raw in CORRECTION_INPUTS {
        if !inputs.contains(raw) {
            inputs.push(raw);
        }
    }
    // Warm every path and both settings before sampling. Alternating order in each
    // round avoids always charging the same configuration for cache/frequency ramp-up.
    for raw in &inputs {
        correction_pipeline(index, raw, baseline);
        correction_pipeline(index, raw, enabled);
    }
    let mut old_pages = Vec::new();
    let mut new_pages = Vec::new();
    let mut old_total = Vec::new();
    let mut new_total = Vec::new();
    let mut old_corrections = Vec::new();
    let mut new_corrections = Vec::new();
    for round in 0..3 {
        for (position, raw) in inputs.iter().enumerate() {
            let (before, after) = if (round + position) % 2 == 0 {
                (correction_pipeline(index, raw, baseline), correction_pipeline(index, raw, enabled))
            } else {
                let after = correction_pipeline(index, raw, enabled);
                (correction_pipeline(index, raw, baseline), after)
            };
            old_pages.push(before.0);
            new_pages.push(after.0);
            old_total.push(before.1);
            new_total.push(after.1);
            if CORRECTION_INPUTS.contains(raw) {
                old_corrections.push(before.1);
                new_corrections.push(after.1);
            }
        }
    }
    for (name, before, after) in [
        ("correction_page", &old_pages, &new_pages),
        ("correction_pipeline", &old_total, &new_total),
        ("correction_inputs_pipeline", &old_corrections, &new_corrections),
    ] {
        let old_p95 = p95(before);
        let new_p95 = p95(after);
        let ratio = new_p95 / old_p95;
        println!(
            "ZHIXU_CORRECTION_TIMING method={name} samples={} baseline_p95_ms={old_p95:.3} enabled_p95_ms={new_p95:.3} ratio={ratio:.3}",
            before.len()
        );
        assert!(ratio <= 1.3, "{name}: enabled p95 is {ratio:.3} times baseline, exceeding 1.3");
    }
}

/// `cargo test --offline --release -- --ignored zhixu_dictionary_benchmark --nocapture`
#[test]
#[ignore]
fn zhixu_dictionary_benchmark() {
    with_correction_settings(
        CorrectionSettings { enabled: false, ..CorrectionSettings::default() },
        dictionary_benchmark,
    );
}

fn dictionary_benchmark() {
    assert!(INPUTS.len() >= 50);
    assert_eq!(
        INPUTS.iter().copied().collect::<HashSet<_>>().len(),
        INPUTS.len()
    );
    assert!(
        INPUTS
            .iter()
            .any(|raw| raw.len() > 20 && !valid_pinyin_spelling(raw))
    );
    let fixture = Fixture::new();
    fixture.copy_dictionaries();
    let started = Instant::now();
    let old = fallback();
    let old_load = started.elapsed();
    let started = Instant::now();
    let data = loader::load(&fixture.0.join("shared"), &fixture.0.join("user")).unwrap();
    let parse_time = started.elapsed();
    let new = Arc::new(Index::from_data(data));
    let load_time = started.elapsed();
    println!(
        "ZHIXU_LOAD old_entries={} new_entries={} old_decode_entries={} new_decode_entries={} old_build_ms={:.3} new_parse_ms={:.3} new_load_and_build_ms={:.3} old_estimated_bytes={} new_estimated_bytes={}",
        old.entry_count(),
        new.entry_count(),
        old.projection_count(),
        new.projection_count(),
        old_load.as_secs_f64() * 1e3,
        parse_time.as_secs_f64() * 1e3,
        load_time.as_secs_f64() * 1e3,
        old.estimated_bytes(),
        new.estimated_bytes(),
    );

    let pages: Vec<_> = INPUTS.iter().map(|raw| base_page(raw)).collect();
    let active: Vec<_> = INPUTS
        .iter()
        .map(|raw| {
            with_index(old.clone(), || {
                !valid_pinyin_spelling(raw)
                    && !has_registered_ascii_exact(raw, false)
                    && tech_terms_for_reading_prefix(raw).is_empty()
            })
        })
        .collect();
    let mut behavior_hash = 0xcbf29ce484222325u64;
    let mut differences = (0, 0);
    let mut heavy_regressions = Vec::new();
    for (raw, page) in INPUTS.iter().zip(&pages) {
        let results = |index: &Arc<Index>| {
            with_index(index.clone(), || {
                let lattice = decode_lattice(raw, 24, !valid_pinyin_spelling(raw));
                let heavy = heavy_garble_candidates(raw, page, false)
                    .into_iter()
                    .map(|candidate| candidate.candidate.text)
                    .collect::<Vec<_>>();
                (lattice, heavy)
            })
        };
        let before = results(&old);
        let after = results(&new);
        let typos = with_index(new.clone(), || decode_heavy_garble(raw));
        for byte in format!("{raw}:{after:?}:{typos:?}\n").bytes() {
            behavior_hash = (behavior_hash ^ u64::from(byte)).wrapping_mul(0x100000001b3);
        }
        if ["jxlzuoshenmzhegexiangmu", "nikkyoushenmwent", "woxikanx"].contains(raw)
            && before.1.first() != after.1.first()
        {
            heavy_regressions.push((*raw, before.1.first().cloned(), after.1.first().cloned()));
        }
        differences.0 += usize::from(before.0 != after.0);
        differences.1 += usize::from(before.1 != after.1);
        if before.0.first() != after.0.first() || before.1.first() != after.1.first() {
            println!(
                "ZHIXU_TOP_CHANGE raw={raw} lattice_old={:?} lattice_new={:?} heavy_old={:?} heavy_new={:?}",
                before.0.first(),
                after.0.first(),
                before.1.first(),
                after.1.first()
            );
        }
    }

    println!("ZHIXU_BEHAVIOR hash={behavior_hash:016x}");
    // Captured from 7ae0440 before changing storage: complete lists, including typo paths.
    assert_eq!(behavior_hash, 0x8fe7b62dec51628c, "dictionary storage changed candidate lists");

    let mut per_input_lattice = vec![(Duration::ZERO, Duration::ZERO); INPUTS.len()];
    let mut old_lattice = Vec::new();
    let mut new_lattice = Vec::new();
    let mut old_heavy = Vec::new();
    let mut new_heavy = Vec::new();
    let mut old_active_heavy = Vec::new();
    let mut new_active_heavy = Vec::new();
    // The result comparison above warms both indexes. Alternate order to avoid
    // systematically charging one dictionary for cache or frequency ramp-up.
    for round in 0..3 {
        for (position, (raw, page)) in INPUTS.iter().zip(&pages).enumerate() {
            let (before, after) = if (round + position) % 2 == 0 {
                (measure(&old, raw, page), measure(&new, raw, page))
            } else {
                let after = measure(&new, raw, page);
                (measure(&old, raw, page), after)
            };
            per_input_lattice[position].0 += before.0;
            per_input_lattice[position].1 += after.0;
            old_lattice.push(before.0);
            new_lattice.push(after.0);
            old_heavy.push(before.1);
            new_heavy.push(after.1);
            if active[position] {
                old_active_heavy.push(before.1);
                new_active_heavy.push(after.1);
            }
        }
    }
    println!(
        "ZHIXU_WORKLOAD unique_inputs={} over_20_letters={} heavy_active_inputs={} samples_per_method={} changed_lattice_lists={} changed_heavy_lists={}",
        INPUTS.len(),
        INPUTS.iter().filter(|raw| raw.len() > 20).count(),
        active.iter().filter(|active| **active).count(),
        old_lattice.len(),
        differences.0,
        differences.1
    );
    let mut slowest: Vec<_> = INPUTS.iter().zip(per_input_lattice).collect();
    slowest.sort_by_key(|(_, (_, after))| std::cmp::Reverse(*after));
    for (raw, (before, after)) in slowest.into_iter().take(8) {
        println!(
            "ZHIXU_SLOW_INPUT raw={raw} lattice_old_mean_ms={:.3} lattice_new_mean_ms={:.3} ratio={:.3}",
            before.as_secs_f64() * 1e3 / 3.0,
            after.as_secs_f64() * 1e3 / 3.0,
            after.as_secs_f64() / before.as_secs_f64()
        );
    }
    let mut ratios = Vec::new();
    for (name, before, after) in [
        ("decode_lattice", &old_lattice, &new_lattice),
        ("heavy_garble_candidates", &old_heavy, &new_heavy),
        (
            "heavy_garble_candidates_active",
            &old_active_heavy,
            &new_active_heavy,
        ),
    ] {
        let old_p95 = p95(before);
        let new_p95 = p95(after);
        let ratio = new_p95 / old_p95;
        println!(
            "ZHIXU_TIMING method={name} old_p95_ms={old_p95:.3} new_p95_ms={new_p95:.3} ratio={ratio:.3}"
        );
        ratios.push((name, ratio));
    }
    // The original assertions use fixed ASCII examples, including public-shell
    // terms. Only this replay receives the fixture; timings above use disk data.
    fixture.install_ascii_fixture();
    let replay_data = loader::load(&fixture.0.join("shared"), &fixture.0.join("user")).unwrap();
    let replay = Arc::new(Index::from_data(replay_data));
    // These tests deliberately retain learning across mixer instances. Run them
    // once, with the shipping defaults; the legacy lists were checked above.
    with_correction_settings(CorrectionSettings::default(), || {
        super::tests::check_loaded_dictionary(replay);
    });
    assert!(
        heavy_regressions.is_empty(),
        "new low-weight words displaced protected heavy candidates: {heavy_regressions:?}"
    );
    for (name, ratio) in ratios {
        assert!(
            ratio <= 1.5,
            "{name}: new p95 is {ratio:.3} times the fallback, exceeding 1.5"
        );
    }
    benchmark_correction_paths(&new);
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

#[cfg(target_os = "macos")]
fn memory_sample(phase: &str) {
    let (rss, peak, rusage_peak) = memory_snapshot();
    println!(
        "ZHIXU_MEMORY phase={phase} rss_bytes={rss} peak_bytes={peak} rusage_peak_bytes={rusage_peak}"
    );
}

/// Run alone in a fresh process: `cargo test --offline --release -- --ignored zhixu_dictionary_memory --nocapture`.
#[test]
#[ignore]
#[cfg(target_os = "macos")]
fn zhixu_dictionary_memory() {
    let fixture = Fixture::new();
    fixture.copy_dictionaries();
    memory_sample("start");
    let old = fallback();
    memory_sample("fallback");
    println!("ZHIXU_PARTS fallback={:?}", old.estimated_breakdown());
    let data = loader::load(&fixture.0.join("shared"), &fixture.0.join("user")).unwrap();
    let parsed = data.entries.capacity() * std::mem::size_of::<loader::Entry>()
        + data
            .entries
            .iter()
            .map(|e| e.text.capacity() + e.reading.capacity() + e.layer.capacity())
            .sum::<usize>();
    println!(
        "ZHIXU_PARSED entries={} capacity_bytes={parsed}",
        data.entries.len()
    );
    memory_sample("parsed_with_fallback");
    let index = Arc::new(Index::from_data(data));
    memory_sample("fallback_and_runtime");
    println!("ZHIXU_PARTS runtime={:?}", index.estimated_breakdown());
    let estimated = index.estimated_bytes();
    let next = Arc::new(Index::from_data(
        loader::load(&fixture.0.join("shared"), &fixture.0.join("user")).unwrap(),
    ));
    memory_sample("fallback_and_two_generations");
    drop(index);
    memory_sample("old_generation_reclaimed");
    drop(next);
    drop(old);
    memory_sample("readers_released");
    assert!(
        estimated <= 40 * 1024 * 1024,
        "runtime index capacity {estimated} exceeds 40 MiB"
    );
}
