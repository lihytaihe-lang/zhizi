//! Synthetic Q12 replay. All timers and fixture generation are test-only.

use super::{Index, learning, loader};
use crate::state::{KeyEvent, KeyKind, SecurityState, Session};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(usize)]
pub(crate) enum Stage {
    Dictionary, Generation, Learning, SortDedup, PagingClone, Visible, Ffi,
}

const STAGES: [Stage; 7] = [Stage::Dictionary, Stage::Generation, Stage::Learning,
    Stage::SortDedup, Stage::PagingClone, Stage::Visible, Stage::Ffi];

#[derive(Default)]
struct Timing {
    enabled: bool,
    current: Option<Stage>,
    checkpoint: Option<Instant>,
    elapsed: [u128; 7],
}

impl Timing {
    fn switch(&mut self, next: Option<Stage>) {
        let now = Instant::now();
        if let (Some(stage), Some(start)) = (self.current, self.checkpoint) {
            self.elapsed[stage as usize] += now.duration_since(start).as_nanos();
        }
        self.current = next;
        self.checkpoint = Some(now);
    }
}

thread_local! {
    static TIMING: RefCell<Timing> = RefCell::new(Timing::default());
}

pub(crate) struct Span {
    previous: Option<Stage>,
    enabled: bool,
}

pub(crate) fn span(stage: Stage) -> Span {
    TIMING.with(|slot| {
        let mut timing = slot.borrow_mut();
        let previous = timing.current;
        let enabled = timing.enabled;
        if enabled { timing.switch(Some(stage)); }
        Span { previous, enabled }
    })
}

impl Drop for Span {
    fn drop(&mut self) {
        if self.enabled {
            TIMING.with(|slot| slot.borrow_mut().switch(self.previous));
        }
    }
}

fn start_timing() {
    TIMING.with(|slot| *slot.borrow_mut() = Timing { enabled: true, ..Timing::default() });
}

fn finish_timing() -> [u128; 7] {
    TIMING.with(|slot| {
        let mut timing = slot.borrow_mut();
        timing.switch(None);
        timing.enabled = false;
        timing.elapsed
    })
}

#[test]
fn nested_profile_spans_restore_the_outer_stage_and_disable_cleanly() {
    start_timing();
    let started = Instant::now();
    let outer = span(Stage::Learning);
    std::thread::sleep(std::time::Duration::from_millis(1));
    let inner = span(Stage::Dictionary);
    TIMING.with(|slot| assert_eq!(slot.borrow().current, Some(Stage::Dictionary)));
    std::thread::sleep(std::time::Duration::from_millis(1));
    drop(inner);
    TIMING.with(|slot| assert_eq!(slot.borrow().current, Some(Stage::Learning)));
    drop(outer);
    TIMING.with(|slot| assert_eq!(slot.borrow().current, None));
    let elapsed = finish_timing();
    assert!(elapsed[Stage::Learning as usize] > 0);
    assert!(elapsed[Stage::Dictionary as usize] > 0);
    assert_eq!(elapsed[Stage::Generation as usize], 0);
    assert!(elapsed.iter().sum::<u128>() <= started.elapsed().as_nanos());
    drop(span(Stage::Ffi));
    TIMING.with(|slot| {
        assert!(!slot.borrow().enabled);
        assert_eq!(slot.borrow().elapsed, elapsed);
    });
}

// Ten short initials, fourteen sentences, eight apostrophe inputs and eight
// single-letter substitutions; these are invented examples, not owner records.
const INPUTS: [&str; 40] = [
    "nh", "sj", "xm", "kf", "cs", "jxl", "wms", "zgr", "srfa", "xzxm",
    "jiexialaizuoshenme", "womengaichengxiawuliao", "woxiangxiankanxiaxiaoguo",
    "mingtianzaishiyixiazhege", "zhegexiangmuxuyaoceshi", "qingbangwokanxiawentia",
    "womengkeyixianzuoyiban", "jintianxiabaohouzaishuo", "xianzaiyinggaimeiwentile",
    "wozhunbeimingtiankaishi", "zheyangxiegengrongyidong", "dengdaoceshiwanchengyihou",
    "nengbunengzaifayibian", "jiangwanhuiyihouzaijixu",
    "xi'an", "chang'an", "tian'anmen", "wo'ai'ni", "ni'hao'shijie", "xian'zai",
    "ming'tian'jian", "xiang'mu'ceshi",
    "nihaoshijir", "xiangmukaifx", "woxiangchifab", "jiexialaizuoshenmr",
    "zhegexiangmuxuyaoceshj", "mingtianzaishiyixiazhegr", "womengaichengxiawulian",
    "dengdaoceshiwanchengyihov",
];

const SENTENCES: [(&str, &str); 12] = [
    ("jie xia lai zuo shen me", "接下来做什么"),
    ("wo men gai cheng xia wu liao", "我们改成下午聊"),
    ("wo xiang xian kan xia xiao guo", "我想先看下效果"),
    ("ming tian zai shi yi xia zhe ge", "明天再试一下这个"),
    ("zhe ge xiang mu xu yao ce shi", "这个项目需要测试"),
    ("wo men ke yi xian zuo yi ban", "我们可以先做一半"),
    ("xian zai ying gai mei wen ti le", "现在应该没问题了"),
    ("wo zhun bei ming tian kai shi", "我准备明天开始"),
    ("zhe yang xie geng rong yi dong", "这样写更容易懂"),
    ("deng dao ce shi wan cheng yi hou", "等到测试完成以后"),
    ("neng bu neng zai fa yi bian", "能不能再发一遍"),
    ("jiang wan hui yi hou zai ji xu", "讲完会议后再继续"),
];

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let stamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let root = std::env::temp_dir().join(format!("q12-key-profile-{}-{stamp}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(root.join("shared")).unwrap();
        std::fs::create_dir(root.join("user")).unwrap();
        #[cfg(unix)] {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(root.join("user"), std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        Self(root)
    }

    fn dictionary(&self) -> Arc<Index> {
        let checkout = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        for entry in std::fs::read_dir(checkout.join("native/resources/Rime")).unwrap() {
            let entry = entry.unwrap();
            if entry.file_name().to_string_lossy().ends_with(".dict.yaml") {
                std::fs::copy(entry.path(), self.0.join("shared").join(entry.file_name())).unwrap();
            }
        }
        let relative = ".build-deps/rime-pinyin-simp/0c6861ef7420ee780270ca6d993d18d4101049d0/pinyin_simp.dict.yaml";
        let checkout = checkout.canonicalize().unwrap();
        let source = std::env::var_os("Q12_PINYIN_SOURCE").map(PathBuf::from).unwrap_or_else(|| {
            let local = checkout.join(relative);
            if local.is_file() { local } else {
                // Delegated worktrees live in <repository>/tmp/worktrees/<task>.
                checkout.ancestors().nth(3).unwrap_or(&checkout).join(relative)
            }
        });
        assert!(source.is_file(), "Q12 needs the pinned offline pinyin_simp source; set Q12_PINYIN_SOURCE to that static file");
        std::fs::copy(source, self.0.join("shared/pinyin_simp.dict.yaml")).unwrap();
        // The checked-in owner-word layer is deliberately an empty shell.
        // Populate only its scratch copy with invented full readings; the native
        // initials lookup exercises these words without importing owner data.
        let owner = self.0.join("shared/agent_owner_words.dict.yaml");
        let mut owner_rows = std::fs::read_to_string(&owner).unwrap();
        owner_rows.push_str("\n青禾平台\tqing he ping tai\t670\n星舟项目\txing zhou xiang mu\t660\n");
        std::fs::write(owner, owner_rows).unwrap();
        let data = loader::load(&self.0.join("shared"), &self.0.join("user")).unwrap();
        assert!(data.entries.iter().any(|entry| entry.layer == "agent_project_terms"));
        assert!(data.entries.iter().any(|entry| entry.layer == "agent_owner_words"));
        let index = Arc::new(Index::from_data(data));
        index.warm_whole();
        assert!(index.entry_count() > 100_000, "Q12 must use the deployed dictionary scale");
        index
    }

    fn seed_learning(&self) {
        let now = crate::personal_picks::now_ms();
        let syllables = ["ni", "hao", "wo", "men", "shi", "jie", "xiang", "mu", "ce", "shi",
            "ming", "tian", "xian", "zai", "zhong", "guo", "ren", "gong", "kai", "fa"];
        let mut entries = Vec::new();
        for i in 0..4_000usize {
            let reading = format!("{} {} {}", syllables[i / 400], syllables[i / 20 % 20], syllables[i % 20]);
            let text = format!("合{}{}", char::from_u32(0x4e00 + (i / 80) as u32).unwrap(),
                char::from_u32(0x5000 + (i % 80) as u32).unwrap());
            entries.push(learning::Entry { reading, text, count: (i % 9 + 1) as u32,
                last_ms: now - 2 * 24 * 60 * 60 * 1000,
                rime: Some(learning::RimeWeight { commits: (i % 9 + 1) as u32,
                    dynamic: (i % 17 + 1) as f64, tick: 10_000, updates: u32::from(i % 4 == 0) }) });
        }
        for (reading, text) in SENTENCES {
            entries.push(learning::Entry { reading: reading.into(), text: text.into(), count: 3,
                last_ms: now - 2 * 24 * 60 * 60 * 1000, rime: Some(learning::RimeWeight {
                    commits: 3, dynamic: 3.0, tick: 10_000, updates: 3,
                }) });
        }
        let document = serde_json::json!({ "formatVersion": 1, "migrated": true, "rimeTick": 10_000,
            "ownerSeeded": true, "legacyChoicesImported": true, "entries": entries, "choices": [], "deliveryIds": [] });
        let path = self.0.join("user").join(learning::FILE_NAME);
        std::fs::write(&path, serde_json::to_vec(&document).unwrap()).unwrap();
        #[cfg(unix)] {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        assert_eq!(learning::LearningStore::load(&self.0.join("user"), now).entries().len(), 4_012);
    }

    fn session(&self, dictionary: Arc<Index>) -> crate::AimSession {
        let app_id = "q12.synthetic.replay".to_string();
        let blacklist = crate::blacklist::shared_blacklist(&self.0.join("user")).unwrap();
        let mut inner = Session::new_native(app_id.clone(), blacklist, dictionary).unwrap();
        inner.configure_zhixu_whole(true).unwrap();
        inner.configure_learning(learning::LearningMode::Zhixu).unwrap();
        inner.configure_fuzzy(0).unwrap();
        crate::AimSession { app_id, inner }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
}

fn event(kind: KeyKind, text: &str) -> KeyEvent {
    KeyEvent { kind, text: text.into(), modifiers: 0, security: SecurityState::NonSecure, delivery_id: 0 }
}

fn page_texts(session: &mut crate::AimSession) -> String {
    let needed = unsafe { crate::aim_session_page_texts(session, std::ptr::null_mut(), 0) };
    let mut bytes = vec![0u8; needed.unsigned_abs()];
    let size = unsafe { crate::aim_session_page_texts(session, bytes.as_mut_ptr(), bytes.len()) };
    assert!(size >= 0);
    bytes.truncate(size as usize);
    String::from_utf8(bytes).unwrap()
}

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
struct Baseline {
    // Interning stores every text/label exactly, without lossy fingerprints.
    inputs: Vec<String>,
    strings: Vec<String>,
    candidates: Vec<(usize, usize)>,
    pages: Vec<Vec<usize>>,
    // Input index, byte prefix length, three (native page number, page id) pairs.
    rows: Vec<(usize, usize, Vec<(usize, usize)>)>,
}

fn intern<T: Clone + Eq + std::hash::Hash>(value: T, values: &mut Vec<T>, ids: &mut HashMap<T, usize>) -> usize {
    if let Some(&id) = ids.get(&value) { return id; }
    let id = values.len();
    values.push(value.clone());
    ids.insert(value, id);
    id
}

fn capture_pages(fixture: &Fixture, dictionary: Arc<Index>) -> Baseline {
    let mut session = fixture.session(dictionary);
    let mut baseline = Baseline { inputs: INPUTS.iter().map(|raw| raw.to_string()).collect(), strings: Vec::new(), candidates: Vec::new(), pages: Vec::new(), rows: Vec::new() };
    let (mut strings, mut candidates, mut pages) = (HashMap::new(), HashMap::new(), HashMap::new());
    for (input_index, raw) in INPUTS.iter().enumerate() {
        session.inner.process(event(KeyKind::Escape, ""));
        for (offset, ch) in raw.char_indices() {
            session.inner.process(event(KeyKind::Text, &ch.to_string()));
            let mut row = Vec::new();
            for page in 0..3 {
                if page > 0 { session.inner.process(event(KeyKind::PageDown, "")); }
                let result = session.inner.current_result();
                let ffi_texts = page_texts(&mut session);
                assert_eq!(ffi_texts, result.candidates.iter().map(|c| c.text.as_str()).collect::<Vec<_>>().join("\t"));
                let ids: Vec<_> = result.candidates.iter().map(|candidate| {
                    let text = intern(candidate.text.clone(), &mut baseline.strings, &mut strings);
                    let label = intern(candidate.comment.clone(), &mut baseline.strings, &mut strings);
                    intern((text, label), &mut baseline.candidates, &mut candidates)
                }).collect();
                let id = intern(ids, &mut baseline.pages, &mut pages);
                row.push((result.page_no, id));
            }
            while session.inner.current_result().page_no > 0 {
                session.inner.process(event(KeyKind::PageUp, ""));
            }
            baseline.rows.push((input_index, offset + ch.len_utf8(), row));
        }
    }
    baseline
}

#[test]
fn core_key_latency_pages_match_frozen_baseline() {
    let fixture = Fixture::new();
    fixture.seed_learning();
    let dictionary = fixture.dictionary();
    crate::ranking::with_correction_settings(crate::ranking::CorrectionSettings::default(), ||
    super::with_index(dictionary.clone(), || {
        let actual = capture_pages(&fixture, dictionary);
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/core_key_latency_pages.json");
        if std::env::var_os("Q12_WRITE_BASELINE").as_deref() == Some(std::ffi::OsStr::new("1")) {
            let bytes = serde_json::to_vec(&actual).unwrap();
            assert!(bytes.len() < 200_000, "baseline must stay below 200 KB");
            std::fs::write(&path, bytes).unwrap();
            println!("Q12_BASELINE rows={} bytes={}", actual.rows.len(), std::fs::metadata(path).unwrap().len());
            return;
        }
        let bytes = std::fs::read(&path).expect("generate Q12 baseline on unoptimized code using Q12_WRITE_BASELINE=1");
        assert!(bytes.len() < 200_000);
        let expected: Baseline = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(actual, expected, "Q12 candidate texts, labels, pages or input prefixes changed");
        println!("Q12_EQUIVALENCE inputs={} prefixes={} page_checks={}", INPUTS.len(), actual.rows.len(), actual.rows.len() * 3);
    }));
}

fn percentile(samples: &[u128], percent: usize) -> f64 {
    let mut values = samples.to_vec();
    values.sort_unstable();
    values[(values.len() * percent).div_ceil(100) - 1] as f64 / 1_000_000.0
}

/// `cargo test --offline --release core_key_latency_profile -- --ignored --nocapture --test-threads=1`
#[test]
#[ignore]
fn core_key_latency_profile() {
    assert!(!cfg!(debug_assertions), "Q12 timings require --release");
    let fixture = Fixture::new();
    fixture.seed_learning();
    let dictionary = fixture.dictionary();
    let entries = dictionary.entry_count();
    crate::ranking::with_correction_settings(crate::ranking::CorrectionSettings::default(), ||
    super::with_index(dictionary.clone(), || {
        let mut stages: [Vec<u128>; 7] = std::array::from_fn(|_| Vec::new());
        let mut process = Vec::new();
        let mut total = Vec::new();
        let mut unassigned = Vec::new();
        let mut control_process = Vec::new();
        let mut control_total = Vec::new();
        // Warm static/lazy structures separately; measured passes each start a
        // fresh session, so a replay cannot collapse into prefix-cache hits.
        for round in 0..7 {
            let profile = round > 0 && round % 2 == 1;
            let control = round > 0 && !profile;
            let mut session = fixture.session(dictionary.clone());
            for raw in INPUTS {
                session.inner.process(event(KeyKind::Escape, ""));
                for ch in raw.chars() {
                    let key = event(KeyKind::Text, &ch.to_string());
                    if profile { start_timing(); }
                    let started = Instant::now();
                    let model = session.inner.process(key);
                    let core_ns = started.elapsed().as_nanos();
                    let mut result = crate::AimResult::default();
                    {
                        let _span = span(Stage::Ffi);
                        crate::fill_result(&mut result, model, core_ns.min(u64::MAX as u128) as u64);
                    }
                    let total_ns = started.elapsed().as_nanos();
                    if profile {
                        let elapsed = finish_timing();
                        assert!(elapsed.iter().sum::<u128>() <= total_ns, "profile stages must be disjoint");
                        for (samples, time) in stages.iter_mut().zip(elapsed) { samples.push(time); }
                        process.push(core_ns);
                        total.push(total_ns);
                        unassigned.push(total_ns.saturating_sub(elapsed.iter().sum()));
                    } else if control {
                        control_process.push(core_ns);
                        control_total.push(total_ns);
                    }
                    unsafe { crate::aim_result_free(&mut result) };
                }
            }
        }
        println!("Q12_PROFILE dictionary_entries={entries} learned_words=4000 learned_sentences=12 inputs={} text_keys={} measured_passes=3 control_passes=3 migrated_unchanged=3000 updated=1012", INPUTS.len(), process.len());
        println!("Q12_STAGE\tp50_ms\tp95_ms\tp99_ms");
        for (stage, samples) in STAGES.iter().zip(&stages) {
            println!("Q12_STAGE\t{stage:?}\t{:.6}\t{:.6}\t{:.6}", percentile(samples, 50), percentile(samples, 95), percentile(samples, 99));
        }
        for (name, samples) in [("SessionProcess", &process), ("ProcessAndFfi", &total), ("Unassigned", &unassigned), ("ControlSessionProcess", &control_process), ("ControlProcessAndFfi", &control_total)] {
            println!("Q12_STAGE\t{name}\t{:.6}\t{:.6}\t{:.6}", percentile(samples, 50), percentile(samples, 95), percentile(samples, 99));
        }
    }));
}

/// Numeral x measure-word words (几十秒, 十几分钟, ...) must be typable with full pinyin
/// (scripts/lexicon/gen_numeral_unit_words.py). `jishimiao` used to offer 及时秒/即使秒 only.
#[test]
fn numeral_unit_words_reach_the_first_page() {
    let fixture = Fixture::new();
    let dictionary = fixture.dictionary();
    crate::ranking::with_correction_settings(crate::ranking::CorrectionSettings::default(), ||
    super::with_index(dictionary.clone(), || {
        let mut session = fixture.session(dictionary.clone());
        for (raw, want, top) in [("jishimiao", "几十秒", true), ("shijifenzhong", "十几分钟", false),
            ("jishixiaoshi", "几十小时", false), ("jishihaomiao", "几十毫秒", false),
            ("jibaitian", "几百天", false), ("jishitiaorizhi", "几十条日志", false)] {
            session.inner.process(event(KeyKind::Escape, ""));
            for ch in raw.chars() { session.inner.process(event(KeyKind::Text, &ch.to_string())); }
            let page = page_texts(&mut session);
            let texts: Vec<&str> = page.split('\t').collect();
            assert!(texts.contains(&want), "{raw}: {want} missing from the first page {texts:?}");
            if top { assert_eq!(texts[0], want, "{raw}: {texts:?}"); }
        }
    }));
}
