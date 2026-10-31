//! Test-only probe, included by the core's Mixer module with librime linked by native/host.
use super::*;
use crate::rime::RimeDecoder;
use crate::state::{KeyEvent, KeyKind, SecurityState};
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::time::Instant;

#[derive(Default)]
struct NativeTiming {
    input: AtomicU64,
    page: AtomicU64,
    all: AtomicU64,
    all_count: AtomicU64,
}

impl NativeTiming {
    fn reset(&self) {
        for counter in [&self.input, &self.page, &self.all, &self.all_count] {
            counter.store(0, AtomicOrdering::Relaxed);
        }
    }

    fn add(counter: &AtomicU64, start: Instant) {
        counter.fetch_add(start.elapsed().as_nanos() as u64, AtomicOrdering::Relaxed);
    }

    fn get(counter: &AtomicU64) -> u64 { counter.load(AtomicOrdering::Relaxed) }
}

struct TimedRime {
    inner: RimeDecoder,
    timing: Arc<NativeTiming>,
    // Diagnostic simulation only: fetch the real complete sequence before truncating it.
    native_limit: Option<usize>,
}

impl Decoder for TimedRime {
    fn whole_prefix_has_raw_echo(&self) -> bool {
        let start = Instant::now();
        let result = self.inner.whole_prefix_has_raw_echo();
        NativeTiming::add(&self.timing.all, start);
        result
    }

    fn snapshot_candidate_prefix(&self, maximum: usize) -> Option<DecoderSnapshot> {
        if self.native_limit.is_some() { return self.snapshot_all_candidates(); }
        let start = Instant::now();
        let result = self.inner.snapshot_candidate_prefix(maximum);
        NativeTiming::add(&self.timing.all, start);
        self.timing.all_count.fetch_add(result.as_ref().map_or(0, |s| s.candidates.len()) as u64, AtomicOrdering::Relaxed);
        result
    }

    fn snapshot(&self) -> DecoderSnapshot {
        let start = Instant::now();
        let result = self.inner.snapshot();
        NativeTiming::add(&self.timing.page, start);
        result
    }

    fn snapshot_all_candidates(&self) -> Option<DecoderSnapshot> {
        let start = Instant::now();
        let mut result = self.inner.snapshot_all_candidates();
        NativeTiming::add(&self.timing.all, start);
        self.timing.all_count.fetch_add(result.as_ref().map_or(0, |s| s.candidates.len()) as u64, AtomicOrdering::Relaxed);
        if let Some(limit) = self.native_limit && let Some(snapshot) = &mut result {
            snapshot.candidates.truncate(limit);
        }
        result
    }

    fn input(&mut self, text: &str) -> Result<String, i32> {
        let start = Instant::now();
        let result = self.inner.input(text);
        NativeTiming::add(&self.timing.input, start);
        result
    }

    fn backspace(&mut self) -> Result<String, i32> { self.inner.backspace() }
    fn move_left(&mut self) -> Result<String, i32> { self.inner.move_left() }
    fn move_right(&mut self) -> Result<String, i32> { self.inner.move_right() }
    fn page_up(&mut self) -> Result<String, i32> { self.inner.page_up() }
    fn page_down(&mut self) -> Result<String, i32> { self.inner.page_down() }
    fn select_previous(&mut self) -> Result<String, i32> { self.inner.select_previous() }
    fn select_next(&mut self) -> Result<String, i32> { self.inner.select_next() }
    fn select_candidate(&mut self, index: usize, id: u64, intent: LearningIntent) -> Result<String, i32> {
        self.inner.select_candidate(index, id, intent)
    }
    fn select_candidate_global(&mut self, index: usize, id: u64, intent: LearningIntent) -> Result<String, i32> {
        self.inner.select_candidate_global(index, id, intent)
    }
    fn delete_candidate(&mut self, index: usize) -> Result<(), i32> { self.inner.delete_candidate(index) }
    fn delete_candidate_global(&mut self, index: usize) -> Result<(), i32> { self.inner.delete_candidate_global(index) }
    fn commit_raw(&mut self) -> String { self.inner.commit_raw() }
    fn clear(&mut self) { self.inner.clear() }
    fn configure_fuzzy(&mut self, mask: u32) -> Result<(), i32> { self.inner.configure_fuzzy(mask) }
}

#[derive(Clone, Copy)]
struct Sample {
    total: u64,
    input: u64,
    native_all: u64,
    all_ffi: u64,
    all_reading: u64,
    native_page: u64,
    generation: u64,
    learning: u64,
    ranked: u64,
    native_candidates: u64,
    generation_parts: [u128; 6],
    mixer_parts: [u128; 13],
    native_builds: usize,
    generation_builds: usize,
    generation_continuations: usize,
    lattice_expansions: usize,
}

impl Sample {
    fn rest(self) -> u64 {
        self.total.saturating_sub(self.input + self.native_all + self.native_page + self.generation + self.learning)
    }
}

fn query_pipeline(mixer: &MixerDecoder) -> (DecoderSnapshot, Option<DecoderSnapshot>) {
    let _ = mixer.snapshot();
    let _ = mixer.context_group_mask();
    let _ = mixer.context_complete_mask();
    let _ = mixer.composition_alternatives();
    let _ = mixer.composition_edits();
    let all = mixer.snapshot_all_candidates();
    (mixer.snapshot(), all)
}

fn measure(mixer: &mut MixerDecoder, timing: &NativeTiming, letter: char, id: u64) -> (Sample, DecoderSnapshot, Option<DecoderSnapshot>) {
    timing.reset();
    WHOLE_TIMING.with(|timing| *timing.borrow_mut() = [(0, 0); 6]);
    WHOLE_LEARNING_TIMING.with(|timing| timing.set(0));
    WHOLE_GENERATION_CONTINUATIONS.with(|count| count.set(0));
    WHOLE_LATTICE_EXPANSIONS.with(|count| count.set(0));
    WHOLE_GENERATION_TIMING.with(|timing| *timing.borrow_mut() = [0; 6]);
    WHOLE_MIXER_TIMING.with(|timing| *timing.borrow_mut() = [0; 13]);
    crate::rime::RIME_ALL_TIMING.with(|timing| timing.set((0, 0)));
    let text = letter.to_string();
    let event = KeyEvent { kind: KeyKind::Text, text: text.clone(), modifiers: 0, security: SecurityState::NonSecure, delivery_id: id };
    let start = Instant::now();
    mixer.learning_key(&event, id);
    assert!(mixer.input(&text).unwrap().is_empty());
    let (page, all) = query_pipeline(mixer);
    let total = start.elapsed().as_nanos() as u64;
    let stages = WHOLE_TIMING.with(|timing| *timing.borrow());
    let continuations = WHOLE_GENERATION_CONTINUATIONS.with(|count| count.get());
    let expansions = WHOLE_LATTICE_EXPANSIONS.with(|count| count.get());
    assert!(stages[2].0 <= 2 && stages[3].0 <= 2 && stages[4].0 <= 1 + continuations && continuations <= 1 && expansions <= 1,
        "at most two native/ranked builds, one initial generation plus one tail continuation, and one lattice expansion per key: {stages:?}, continuations={continuations}, expansions={expansions}");
    let sample = Sample {
        total,
        input: NativeTiming::get(&timing.input),
        native_all: NativeTiming::get(&timing.all),
        all_ffi: crate::rime::RIME_ALL_TIMING.with(|timing| timing.get().0) as u64,
        all_reading: crate::rime::RIME_ALL_TIMING.with(|timing| timing.get().1) as u64,
        native_page: NativeTiming::get(&timing.page),
        generation: stages[4].1 as u64,
        learning: WHOLE_LEARNING_TIMING.with(|timing| timing.get()) as u64,
        ranked: stages[3].1 as u64,
        native_candidates: NativeTiming::get(&timing.all_count),
        generation_parts: WHOLE_GENERATION_TIMING.with(|timing| *timing.borrow()),
        mixer_parts: WHOLE_MIXER_TIMING.with(|timing| *timing.borrow()),
        native_builds: stages[2].0,
        generation_builds: stages[4].0,
        generation_continuations: continuations,
        lattice_expansions: expansions,
    };
    (sample, page, all)
}

fn fingerprint(hash: &mut u64, snapshot: &DecoderSnapshot) {
    for value in [&snapshot.raw_input, &snapshot.preedit] {
        hash_bytes(hash, value.as_bytes());
    }
    for candidate in snapshot.candidates.iter().chain(&snapshot.secondary_candidates) {
        for value in [&candidate.text, &candidate.comment, &candidate.reading_key] {
            hash_bytes(hash, value.as_bytes());
        }
        hash_bytes(hash, &(candidate.source_index.unwrap_or(usize::MAX) as u64).to_le_bytes());
    }
    for value in [snapshot.cursor_utf8, snapshot.page_no, snapshot.highlighted_index, usize::from(snapshot.is_last_page)] {
        hash_bytes(hash, &(value as u64).to_le_bytes());
    }
}

fn hash_bytes(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes.iter().copied().chain([0]) {
        *hash = (*hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3);
    }
}

fn percentile(samples: &[Sample], field: impl Fn(Sample) -> u64, percent: usize) -> f64 {
    let mut values: Vec<_> = samples.iter().copied().map(field).collect();
    values.sort_unstable();
    values[(values.len() * percent / 100).min(values.len() - 1)] as f64 / 1e3
}

fn summary(keys: &str, enabled: bool, samples: &[Sample], hash: u64) {
    summary_labeled("Z8_SUMMARY", keys, enabled, samples, hash);
}

fn summary_labeled(label: &str, keys: &str, enabled: bool, samples: &[Sample], hash: u64) {
    let mean = |field: fn(Sample) -> u64| samples.iter().copied().map(field).sum::<u64>() as f64 / samples.len() as f64 / 1e3;
    println!("{label}\t{keys}\ton={enabled}\tn={}\tp50_us={:.3}\tp95_us={:.3}\tinput_mean_us={:.3}\tall_mean_us={:.3}\tall_ffi_mean_us={:.3}\tall_reading_mean_us={:.3}\tpage_mean_us={:.3}\tgeneration_mean_us={:.3}\tlearning_mean_us={:.3}\trest_mean_us={:.3}\tranked_inclusive_mean_us={:.3}\thash={hash:016x}",
        samples.len(), percentile(samples, |s| s.total, 50), percentile(samples, |s| s.total, 95),
        mean(|s| s.input), mean(|s| s.native_all), mean(|s| s.all_ffi), mean(|s| s.all_reading), mean(|s| s.native_page), mean(|s| s.generation), mean(|s| s.learning), mean(Sample::rest), mean(|s| s.ranked));
    let parts: Vec<_> = (0..6).map(|i| samples.iter().map(|s| s.generation_parts[i]).sum::<u128>() as f64 / samples.len() as f64 / 1e3).collect();
    println!("Z9_GENERATION\t{label}\t{keys}\ton={enabled}\tlookup_homophones_learning_lattice_sort_output_us={parts:?}");
    let mixer_parts: Vec<_> = (0..13).map(|i| samples.iter().map(|s| s.mixer_parts[i]).sum::<u128>() as f64 / samples.len() as f64 / 1e3).collect();
    println!("Z10_MIXER\t{label}\t{keys}\ton={enabled}\tnative_filter_generated_promotions_overlays_dedup_placement_ranked_post_base_visible_local_ascii_garble_embedded_lexicon_us={mixer_parts:?}");
    println!("Z8_CACHE\t{label}\t{keys}\ton={enabled}\tkeys={}\tnative_builds={}\tgeneration_builds={}\tgeneration_continuations={}\tlattice_expansions={}\tmax_native_ranked_per_key=2",
        samples.len(), samples.iter().map(|s| s.native_builds).sum::<usize>(), samples.iter().map(|s| s.generation_builds).sum::<usize>(), samples.iter().map(|s| s.generation_continuations).sum::<usize>(), samples.iter().map(|s| s.lattice_expansions).sum::<usize>());
}

fn session(shared: &str, user: &str, enabled: bool) -> (MixerDecoder, Arc<NativeTiming>) {
    session_with_limit(shared, user, enabled, None)
}

fn session_with_limit(shared: &str, user: &str, enabled: bool, native_limit: Option<usize>) -> (MixerDecoder, Arc<NativeTiming>) {
    let timing = Arc::new(NativeTiming::default());
    let inner = TimedRime { inner: RimeDecoder::new(shared, user).expect("real librime session"), timing: timing.clone(), native_limit };
    let mut mixer = MixerDecoder::new(Box::new(inner), "zhixu-z8-profile".into());
    mixer.whole_override = Some(enabled);
    mixer.configure_learning(LearningMode::Zhixu).unwrap();
    (mixer, timing)
}

#[test]
#[ignore]
fn whole_real_rime_key_latency_profile() {
    let shared = std::env::var("ZHIXU_Z8_SHARED").expect("set ZHIXU_Z8_SHARED to repository Rime resources");
    let user = std::env::temp_dir().join(format!("zhixu-z8-key-latency-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    std::fs::create_dir(&user).unwrap();
    println!("Z8_ENV\tshared={shared}\tuser={}\tlearning=zhixu\tmodel=none\twarm_rounds=3\tmeasured_rounds=15", user.display());
    let index = Arc::new(Index::from_data(zhixu::loader::load(std::path::Path::new(&shared), &user).expect("repository runtime dictionary")));
    let user = user.to_str().unwrap();
    zhixu::with_index(index, || {
        let (mut cold, timing) = session(&shared, user, true);
        for (i, letter) in "yijian".chars().enumerate() {
            let (sample, _, _) = measure(&mut cold, &timing, letter, i as u64 + 1);
            println!("Z8_COLD\t{}\ttotal_us={:.3}\tnative_all_us={:.3}\tgeneration_us={:.3}\tlearning_us={:.3}", &"yijian"[..i + 1], sample.total as f64 / 1e3, sample.native_all as f64 / 1e3, sample.generation as f64 / 1e3, sample.learning as f64 / 1e3);
        }
        drop(cold);
        let mut aggregate = [Vec::new(), Vec::new()];
        for keys in ["yijian", "gaole", "womengaichengxiawuchaliao", "duoxianchengdeyilaijiao", "jiexialaizuoshenme", "zhongjianbuyaoting"] {
            let mut samples = [Vec::new(), Vec::new()];
            let mut hashes = [0xcbf2_9ce4_8422_2325; 2];
            for round in 0..18 {
                for enabled in if round % 2 == 0 { [false, true] } else { [true, false] } {
                    let slot = usize::from(enabled);
                    let (mut mixer, timing) = session(&shared, user, enabled);
                    for (i, letter) in keys.chars().enumerate() {
                        let (sample, page, all) = measure(&mut mixer, &timing, letter, (i + 1) as u64);
                        if round >= 3 { samples[slot].push(sample); }
                        if round == 3 {
                            fingerprint(&mut hashes[slot], &page);
                            if let Some(all) = all { fingerprint(&mut hashes[slot], &all); }
                            println!("Z8_KEY\t{keys}\t{}\ton={enabled}\ttotal_us={:.3}\tinput_us={:.3}\tall_us={:.3}\tall_ffi_us={:.3}\tall_reading_us={:.3}\tpage_us={:.3}\tgeneration_us={:.3}\tlearning_us={:.3}\trest_us={:.3}\tnative_candidates={}",
                                &keys[..i + 1], sample.total as f64 / 1e3, sample.input as f64 / 1e3, sample.native_all as f64 / 1e3, sample.all_ffi as f64 / 1e3, sample.all_reading as f64 / 1e3, sample.native_page as f64 / 1e3, sample.generation as f64 / 1e3, sample.learning as f64 / 1e3, sample.rest() as f64 / 1e3, sample.native_candidates);
                        }
                    }
                }
            }
            for enabled in [false, true] {
                let slot = usize::from(enabled);
                summary(keys, enabled, &samples[slot], hashes[slot]);
                aggregate[slot].extend_from_slice(&samples[slot]);
            }
        }
        for enabled in [false, true] { summary("ALL", enabled, &aggregate[usize::from(enabled)], 0); }
    });
    // librime keeps this user directory alive until process exit; retain it beside the raw probe log.
}

fn collect_pages(mixer: &mut MixerDecoder) -> Vec<Candidate> {
    let expected = mixer.snapshot_all_candidates().expect("whole sequence").candidates;
    let mut pages = Vec::new();
    let mut page_no = 0;
    loop {
        let page = mixer.snapshot();
        assert_eq!(page.page_no, page_no);
        assert_eq!(page.candidates, expected.iter().skip(page_no * PAGE_SIZE).take(PAGE_SIZE).cloned().collect::<Vec<_>>(),
            "visible page must match the complete sequence at page {}", page_no + 1);
        pages.extend(page.candidates);
        if page.is_last_page { break; }
        page_no += 1;
        assert!(page_no <= expected.len() / PAGE_SIZE + 1, "pager must terminate");
        mixer.page_down().unwrap();
    }
    assert_eq!(pages, expected, "every candidate and global source index must survive actual paging");
    pages
}

/// Tests the semantics of an eager fixed limit, not its speed. No native limit is shipped.
#[test]
#[ignore]
fn whole_real_rime_candidate_limit_equivalence_probe() {
    const LIMIT: usize = 20;
    let shared = std::env::var("ZHIXU_Z8_SHARED").expect("set ZHIXU_Z8_SHARED to repository Rime resources");
    let user = std::env::temp_dir().join(format!("zhixu-z8-limit-equivalence-{}-{}", std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    std::fs::create_dir(&user).unwrap();
    let index = Arc::new(Index::from_data(zhixu::loader::load(std::path::Path::new(&shared), &user).expect("repository runtime dictionary")));
    println!("Z8_LIMIT_ENV\tshared={shared}\tuser={}\tlimit={LIMIT}\tmode=diagnostic-full-fetch-then-truncate", user.display());
    zhixu::with_index(index, || {
        let mut over_limit = 0;
        let mut mismatches = 0;
        let mut first_difference = None;
        for keys in ["yijian", "gaole", "womengaichengxiawuchaliao", "duoxianchengdeyilaijiao", "jiexialaizuoshenme", "zhongjianbuyaoting"] {
            let (mut full, timing) = session(&shared, user.to_str().unwrap(), true);
            let (mut limited, _) = session_with_limit(&shared, user.to_str().unwrap(), true, Some(LIMIT));
            for letter in keys.chars() {
                assert!(full.input(&letter.to_string()).unwrap().is_empty());
                assert!(limited.input(&letter.to_string()).unwrap().is_empty());
            }
            let full_pages = collect_pages(&mut full);
            let limited_pages = collect_pages(&mut limited);
            let native_count = NativeTiming::get(&timing.all_count) as usize;
            if native_count > LIMIT { over_limit += 1; }
            let first = (0..full_pages.len().max(limited_pages.len())).find(|&i| full_pages.get(i) != limited_pages.get(i));
            if let Some(i) = first {
                mismatches += 1;
                println!("Z8_LIMIT_DIFFERENCE\t{keys}\tnative_count={native_count}\tfull_count={}\tlimited_count={}\tpage={}\tslot={}\tfull={:?}\tlimited={:?}",
                    full_pages.len(), limited_pages.len(), i / PAGE_SIZE + 1, i % PAGE_SIZE + 1,
                    full_pages.get(i).map(|c| (&c.text, c.source_index)), limited_pages.get(i).map(|c| (&c.text, c.source_index)));
                if first_difference.is_none() { first_difference = Some((keys.to_string(), i, full_pages.get(i).cloned(), limited_pages.get(i).cloned())); }
            } else {
                println!("Z8_LIMIT_EQUAL\t{keys}\tnative_count={native_count}\tcount={}", full_pages.len());
            }
        }
        assert!(over_limit > 0, "the fixture must exceed the proposed native limit");
        println!("Z8_LIMIT_RESULT\tover_limit={over_limit}\tmismatches={mismatches}");
        // The task rejects a fixed limit when any page changes. This diagnostic records that stop condition;
        // it does not approve or implement a product cap.
        assert!(first_difference.is_some(), "repository fixture must expose fixed-limit loss");
    });
}

fn core_session(shared: &str, user: &str, enabled: bool) -> (crate::state::Session, Arc<NativeTiming>) {
    let timing = Arc::new(NativeTiming::default());
    let inner = TimedRime { inner: RimeDecoder::new(shared, user).expect("real librime session"), timing: timing.clone(), native_limit: None };
    let mut session = crate::state::Session::new(Box::new(inner), "zhixu-z8-session-profile".into());
    session.configure_learning(LearningMode::Zhixu).unwrap();
    session.configure_zhixu_whole(enabled).unwrap();
    (session, timing)
}

fn measure_session(session: &mut crate::state::Session, timing: &NativeTiming, letter: char, id: u64) -> (Sample, crate::state::ResultModel) {
    timing.reset();
    WHOLE_TIMING.with(|timing| *timing.borrow_mut() = [(0, 0); 6]);
    WHOLE_LEARNING_TIMING.with(|timing| timing.set(0));
    WHOLE_GENERATION_CONTINUATIONS.with(|count| count.set(0));
    WHOLE_LATTICE_EXPANSIONS.with(|count| count.set(0));
    WHOLE_GENERATION_TIMING.with(|timing| *timing.borrow_mut() = [0; 6]);
    WHOLE_MIXER_TIMING.with(|timing| *timing.borrow_mut() = [0; 13]);
    crate::rime::RIME_ALL_TIMING.with(|timing| timing.set((0, 0)));
    let event = KeyEvent { kind: KeyKind::Text, text: letter.to_string(), modifiers: 0, security: SecurityState::NonSecure, delivery_id: id };
    let start = Instant::now();
    let result = session.process(event);
    let total = start.elapsed().as_nanos() as u64;
    let stages = WHOLE_TIMING.with(|timing| *timing.borrow());
    let continuations = WHOLE_GENERATION_CONTINUATIONS.with(|count| count.get());
    let expansions = WHOLE_LATTICE_EXPANSIONS.with(|count| count.get());
    assert!(stages[2].0 <= 2 && stages[3].0 <= 2 && stages[4].0 <= 1 + continuations && continuations <= 1 && expansions <= 1,
        "at most two native/ranked builds, one initial generation plus one tail continuation, and one lattice expansion per key: {stages:?}, continuations={continuations}, expansions={expansions}");
    let sample = Sample {
        total,
        input: NativeTiming::get(&timing.input),
        native_all: NativeTiming::get(&timing.all),
        all_ffi: crate::rime::RIME_ALL_TIMING.with(|timing| timing.get().0) as u64,
        all_reading: crate::rime::RIME_ALL_TIMING.with(|timing| timing.get().1) as u64,
        native_page: NativeTiming::get(&timing.page),
        generation: stages[4].1 as u64,
        learning: WHOLE_LEARNING_TIMING.with(|timing| timing.get()) as u64,
        ranked: stages[3].1 as u64,
        native_candidates: NativeTiming::get(&timing.all_count),
        generation_parts: WHOLE_GENERATION_TIMING.with(|timing| *timing.borrow()),
        mixer_parts: WHOLE_MIXER_TIMING.with(|timing| *timing.borrow()),
        native_builds: stages[2].0,
        generation_builds: stages[4].0,
        generation_continuations: continuations,
        lattice_expansions: expansions,
    };
    assert_eq!(result.status, 0);
    assert!(result.consumed);
    assert!(result.commit_text.is_empty());
    (sample, result)
}

/// Measures only the production core Session::process call for each key, without extra queries.
#[test]
#[ignore]
fn whole_real_rime_session_key_latency_profile() {
    assert!(!zhixu_whole_enabled(), "unset ZHIZI_ZHIXU_WHOLE so configure_zhixu_whole(false) can measure the disabled path");
    let shared = std::env::var("ZHIXU_Z8_SHARED").expect("set ZHIXU_Z8_SHARED to repository Rime resources");
    let user = std::env::temp_dir().join(format!("zhixu-z8-session-key-latency-{}-{}", std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    std::fs::create_dir(&user).unwrap();
    println!("Z8_SESSION_ENV\tshared={shared}\tuser={}\tlearning=zhixu\tmodel=none\twarm_rounds=3\tmeasured_rounds=15\tpath=Session::process-only", user.display());
    let index = Arc::new(Index::from_data(zhixu::loader::load(std::path::Path::new(&shared), &user).expect("repository runtime dictionary")));
    let user = user.to_str().unwrap();
    zhixu::with_index(index, || {
        let (mut cold, timing) = core_session(&shared, user, true);
        for (i, letter) in "yijian".chars().enumerate() {
            let (sample, _) = measure_session(&mut cold, &timing, letter, i as u64 + 1);
            println!("Z8_SESSION_COLD\t{}\ttotal_us={:.3}\tnative_all_us={:.3}\tgeneration_us={:.3}\tlearning_us={:.3}",
                &"yijian"[..i + 1], sample.total as f64 / 1e3, sample.native_all as f64 / 1e3, sample.generation as f64 / 1e3, sample.learning as f64 / 1e3);
        }
        drop(cold);
        let mut aggregate = [Vec::new(), Vec::new()];
        for keys in ["yijian", "gaole", "womengaichengxiawuchaliao", "duoxianchengdeyilaijiao", "jiexialaizuoshenme", "zhongjianbuyaoting"] {
            let mut samples = [Vec::new(), Vec::new()];
            let mut hashes = [0xcbf2_9ce4_8422_2325; 2];
            for round in 0..18 {
                for enabled in if round % 2 == 0 { [false, true] } else { [true, false] } {
                    let slot = usize::from(enabled);
                    let (mut session, timing) = core_session(&shared, user, enabled);
                    for (i, letter) in keys.chars().enumerate() {
                        let (sample, result) = measure_session(&mut session, &timing, letter, (i + 1) as u64);
                        if round >= 3 { samples[slot].push(sample); }
                        if round == 3 {
                            fingerprint(&mut hashes[slot], &DecoderSnapshot { raw_input: keys[..i + 1].to_string(), preedit: result.marked_text,
                                cursor_utf8: result.marked_cursor_utf8, candidates: result.candidates, secondary_candidates: result.secondary_candidates,
                                highlighted_index: result.highlighted_index, page_no: result.page_no, is_last_page: result.is_last_page });
                            println!("Z8_KEY_SESSION\t{keys}\t{}\ton={enabled}\ttotal_us={:.3}\tinput_us={:.3}\tall_us={:.3}\tall_ffi_us={:.3}\tall_reading_us={:.3}\tpage_us={:.3}\tgeneration_us={:.3}\tlearning_us={:.3}\trest_us={:.3}\tnative_candidates={}",
                                &keys[..i + 1], sample.total as f64 / 1e3, sample.input as f64 / 1e3, sample.native_all as f64 / 1e3, sample.all_ffi as f64 / 1e3,
                                sample.all_reading as f64 / 1e3, sample.native_page as f64 / 1e3, sample.generation as f64 / 1e3,
                                sample.learning as f64 / 1e3, sample.rest() as f64 / 1e3, sample.native_candidates);
                        }
                    }
                }
            }
            for enabled in [false, true] {
                let slot = usize::from(enabled);
                summary_labeled("Z8_SESSION_SUMMARY", keys, enabled, &samples[slot], hashes[slot]);
                aggregate[slot].extend_from_slice(&samples[slot]);
            }
        }
        for enabled in [false, true] {
            summary_labeled("Z8_SESSION_SUMMARY", "ALL", enabled, &aggregate[usize::from(enabled)], 0);
        }
    });
}

/// Compare actual lazy paging to the unchanged eager sequence without priming the lazy full query.
#[test]
#[ignore]
fn whole_real_rime_lazy_pages_equal_eager_to_last_and_back() {
    let shared = std::env::var("ZHIXU_Z8_SHARED").expect("static repository shared directory");
    let user = std::env::temp_dir().join(format!("zhixu-z9-lazy-pages-{}", std::process::id()));
    std::fs::create_dir(&user).unwrap();
    let index = Arc::new(Index::from_data(zhixu::loader::load(std::path::Path::new(&shared), &user).unwrap()));
    zhixu::with_index(index, || {
        for keys in ["yijian", "gaole", "womengaichengxiawuchaliao", "duoxianchengdeyilaijiao", "jiexialaizuoshenme", "zhongjianbuyaoting"] {
            let (mut lazy, timing) = session(&shared, user.to_str().unwrap(), true);
            let (mut eager, _) = session(&shared, user.to_str().unwrap(), true);
            eager.whole_eager = true;
            for letter in keys.chars() {
                lazy.input(&letter.to_string()).unwrap();
                eager.input(&letter.to_string()).unwrap();
            }
            let native = eager.inner.snapshot_all_candidates().unwrap();
            assert_eq!(eager.inner.whole_prefix_has_raw_echo(), native.candidates.iter()
                .any(|candidate| candidate.text.is_ascii() && candidate.text.eq_ignore_ascii_case(&native.raw_input)));
            for maximum in [0, 1, PAGE_SIZE, 2 * PAGE_SIZE, 20, native.candidates.len(), usize::MAX] {
                let prefix = eager.inner.snapshot_candidate_prefix(maximum).unwrap();
                assert_eq!(prefix.candidates, native.candidates[..maximum.min(native.candidates.len())]);
                assert_eq!(prefix.is_last_page, maximum >= native.candidates.len());
                assert_eq!((prefix.page_no, prefix.highlighted_index), (0, 0));
            }
            let mut pages = Vec::new();
            loop {
                let a = lazy.snapshot(); let b = eager.snapshot();
                super::tests::assert_whole_snapshot(&a, &b);
                let last = a.is_last_page;
                pages.push(a);
                if last { break; }
                assert!(pages.len() < 10000);
                lazy.page_down().unwrap(); eager.page_down().unwrap();
            }
            for expected in pages.iter().rev().skip(1) {
                lazy.page_up().unwrap(); eager.page_up().unwrap();
                super::tests::assert_whole_snapshot(&lazy.snapshot(), expected);
                super::tests::assert_whole_snapshot(&lazy.snapshot(), &eager.snapshot());
            }
            let all = lazy.snapshot_all_candidates().unwrap();
            assert_eq!(all.candidates, pages.iter().flat_map(|p| p.candidates.clone()).collect::<Vec<_>>());
            let mut hash = 0xcbf2_9ce4_8422_2325;
            for page in &pages { fingerprint(&mut hash, page); }
            fingerprint(&mut hash, &all);
            println!("Z10_PAGES_FINGERPRINT\t{keys}\thash={hash:016x}");
            println!("Z9_LAZY_EQUAL\t{keys}\tpages={}\tcandidates={}\tnative_converted={}", pages.len(), all.candidates.len(), NativeTiming::get(&timing.all_count));
        }
    });
}

/// Public-only fixture, actual product constructor, including native deployment.
#[test]
#[ignore]
fn whole_y4_inner_latency_profile() {
    let shared = std::env::var("ZHIXU_Z8_SHARED").unwrap();
    let user = std::env::temp_dir().join(format!("zhixu-y4-profile-{}", std::process::id()));
    std::fs::create_dir(&user).unwrap();
    let mut samples = [Vec::new(), Vec::new()];
    let mut backs = [Vec::new(), Vec::new()];
    let order = if std::env::var_os("ZHIXU_Y4_NATIVE_FIRST").is_some() { [true, false] } else { [false, true] };
    for round in 0..4 {
        for enabled in if round % 2 == 0 { order } else { [order[1], order[0]] } {
            let start = Instant::now();
            let rime = RimeDecoder::new(&shared, user.to_str().unwrap()).unwrap();
            let rime_us = start.elapsed().as_micros();
            let mut session = crate::state::Session::new_with_blacklist(Box::new(rime), "y4-public".into(),
                Arc::new(std::sync::Mutex::new(crate::blacklist::Store::default()))).unwrap();
            session.configure_learning(LearningMode::Zhixu).unwrap();
            session.configure_zhixu_whole(enabled).unwrap();
            println!("Y4_START on={enabled} round={round} rime_us={rime_us} total_us={}", start.elapsed().as_micros());
            let mut hash = 0xcbf2_9ce4_8422_2325;
            for keys in ["yijian", "gaole", "womengaichengxiawuchaliao", "duoxianchengdeyilaijiao", "jiexialaizuoshenme", "zhongjianbuyaoting"] {
                for (i, (kind, text)) in keys.chars().map(|ch| (KeyKind::Text, ch.to_string()))
                    .chain(keys.chars().map(|_| (KeyKind::Backspace, String::new()))).enumerate() {
                    crate::zhixu::decoder::INNER_TIMING.with(|t| *t.borrow_mut() = [0; 3]);
                    let start = Instant::now();
                    let result = session.process(KeyEvent { kind, text, modifiers: 0,
                        security: SecurityState::NonSecure, delivery_id: (i + 1) as u64 });
                    let us = start.elapsed().as_nanos() as u64;
                    assert_eq!(result.status, 0);
                    let parts = crate::zhixu::decoder::INNER_TIMING.with(|t| *t.borrow());
                    if round == 0 { println!("Y4_KEY on={enabled} keys={keys} step={i} back={} ns={us} generate_initials_whole_ns={parts:?}", i >= keys.len()); }
                    if round > 0 { if i < keys.len() { samples[enabled as usize].push(us); } else { backs[enabled as usize].push(us); } }
                    hash_bytes(&mut hash, format!("{:?}", result).as_bytes());
                }
            }
            println!("Y4_HASH on={enabled} round={round} hash={hash:016x}");
        }
    }
    for enabled in [false, true] {
        for (label, values) in [("keys", &mut samples[enabled as usize]), ("backspace", &mut backs[enabled as usize])] {
            values.sort_unstable();
            println!("Y4_SUMMARY on={enabled} kind={label} n={} p50_us={} p95_us={} p99_us={}", values.len(),
                values[values.len()/2] / 1000, values[values.len()*95/100] / 1000, values[values.len()*99/100] / 1000);
        }
    }
}

/// Full repository deployment, frozen base-500 inputs, production Session path.
#[test]
#[ignore]
fn whole_y5b_full_dictionary_latency_profile() {
    let shared = std::env::var("ZHIXU_Z8_SHARED").unwrap();
    let corpus: serde_json::Value = serde_json::from_str(include_str!("../../../quality/corpus/base-500.json")).unwrap();
    let user = std::env::temp_dir().join(format!("zhixu-y5b-{}", std::process::id()));
    std::fs::create_dir(&user).unwrap();
    for enabled in [false, true] {
        let start = Instant::now();
        let rime = RimeDecoder::new(&shared, user.to_str().unwrap()).unwrap();
        let mut session = crate::state::Session::new_with_blacklist(Box::new(rime), "y5b".into(),
            Arc::new(std::sync::Mutex::new(crate::blacklist::Store::default()))).unwrap();
        session.configure_zhixu_whole(enabled).unwrap();
        println!("Y5B_START on={enabled} us={}", start.elapsed().as_micros());
        // Separate warm typing from cold construction and the handoff tests.
        std::thread::sleep(std::time::Duration::from_secs(3));
        if enabled {
            let dictionary = zhixu::loader::ready_deployed_snapshot().expect("full deployment must be warm")
                .expect("real Rime attaches the runtime dictionary");
            println!("Y5B_DICTIONARY entries={}", dictionary.entry_count());
        }
        let mut samples = Vec::new();
        let mut id = 0;
        for case in corpus["cases"].as_array().unwrap() {
            let keys = case["query"].as_str().unwrap();
            for (step, ch) in keys.chars().enumerate() {
                crate::zhixu::decoder::INNER_TIMING.with(|t| *t.borrow_mut() = [0; 3]);
                id += 1;
                let start = Instant::now();
                let result = session.process(KeyEvent { kind: KeyKind::Text, text: ch.to_string(), modifiers: 0,
                    security: SecurityState::NonSecure, delivery_id: id });
                let ns = start.elapsed().as_nanos();
                assert_eq!(result.status, 0);
                let parts = crate::zhixu::decoder::INNER_TIMING.with(|t| *t.borrow());
                println!("Y5B_TRACE {}", serde_json::json!({"on": enabled, "case": case["id"], "step": step, "result": format!("{result:?}")}));
                samples.push((ns, keys.to_string(), step, parts));
            }
            id += 1;
            session.process(KeyEvent { kind: KeyKind::Escape, text: String::new(), modifiers: 0,
                security: SecurityState::NonSecure, delivery_id: id });
        }
        samples.sort_by_key(|s| s.0);
        println!("Y5B_SUMMARY on={enabled} n={} p50_ns={} p95_ns={} p99_ns={}", samples.len(),
            samples[samples.len()/2].0, samples[samples.len()*95/100].0, samples[samples.len()*99/100].0);
        for sample in samples.iter().rev().take(20) { println!("Y5B_TAIL on={enabled} {sample:?}"); }
    }
}

#[cfg(target_os = "macos")]
mod whole_y6_macos {
    use std::ffi::{c_int, c_long, c_uint, c_void};

    const CLOCK_THREAD_CPUTIME_ID: c_int = 16;

    #[repr(C)]
    struct Timespec { tv_sec: c_long, tv_nsec: c_long }

    unsafe extern "C" {
        fn pthread_self() -> *mut c_void;
        fn pthread_get_qos_class_np(thread: *mut c_void, class: *mut c_uint, relative: *mut c_int) -> c_int;
        fn pthread_set_qos_class_self_np(class: c_uint, relative: c_int) -> c_int;
        fn pthread_threadid_np(thread: *mut c_void, id: *mut u64) -> c_int;
        fn pthread_main_np() -> c_int;
        fn clock_gettime_nsec_np(clock: c_int) -> u64;
        fn clock_getres(clock: c_int, resolution: *mut Timespec) -> c_int;
    }

    pub(super) fn thread_state() -> serde_json::Value {
        let (mut class, mut relative, mut id) = (0, 0, 0);
        let qos_status = unsafe { pthread_get_qos_class_np(pthread_self(), &mut class, &mut relative) };
        let id_status = unsafe { pthread_threadid_np(std::ptr::null_mut(), &mut id) };
        assert_eq!(qos_status, 0);
        assert_eq!(id_status, 0);
        // These APIs report requested QoS; effective overrides and CPU placement are not exposed here.
        serde_json::json!({"supported": true, "requested_qos": class, "relative_priority": relative,
            "thread_id": id, "is_main": unsafe { pthread_main_np() } != 0,
            "get_qos_status": qos_status, "thread_id_status": id_status})
    }

    pub(super) fn set_qos(class: u32) -> i32 {
        let status = unsafe { pthread_set_qos_class_self_np(class, 0) };
        assert_eq!(status, 0, "requested QoS change failed");
        let state = thread_state();
        assert_eq!(state["requested_qos"].as_u64(), Some(class as u64));
        assert_eq!(state["relative_priority"].as_i64(), Some(0));
        status
    }

    pub(super) fn cpu_ns() -> u64 {
        let value = unsafe { clock_gettime_nsec_np(CLOCK_THREAD_CPUTIME_ID) };
        assert_ne!(value, 0, "thread CPU clock failed: {}", std::io::Error::last_os_error());
        value
    }

    pub(super) fn clock_resolution_ns() -> u64 {
        let mut resolution = Timespec { tv_sec: 0, tv_nsec: 0 };
        let status = unsafe { clock_getres(CLOCK_THREAD_CPUTIME_ID, &mut resolution) };
        assert_eq!(status, 0, "thread CPU clock resolution failed");
        (resolution.tv_sec as u64) * 1_000_000_000 + resolution.tv_nsec as u64
    }
}

fn whole_y6_qos_request(setting: &str) -> Option<u32> {
    match setting {
        "inherit" => None,
        "interactive" => Some(0x21),
        _ => panic!("ZHIXU_Y6_QOS must be inherit or interactive"),
    }
}

fn whole_y6_thread_state() -> serde_json::Value {
    #[cfg(target_os = "macos")]
    { whole_y6_macos::thread_state() }
    #[cfg(not(target_os = "macos"))]
    { serde_json::json!({"supported": false}) }
}

fn whole_y6_cpu_ns() -> u64 {
    #[cfg(target_os = "macos")]
    { whole_y6_macos::cpu_ns() }
    #[cfg(not(target_os = "macos"))]
    { 0 }
}

#[test]
fn whole_y6_qos_diagnostic_requires_explicit_interactive_request() {
    assert_eq!(whole_y6_qos_request("inherit"), None);
    assert_eq!(whole_y6_qos_request("interactive"), Some(0x21));
    assert!(std::panic::catch_unwind(|| whole_y6_qos_request("background")).is_err());
}

/// Optional Y6_FUZZY_MASK env value: absent keeps today's behavior, present must be decimal 0..=255.
fn whole_f1_fuzzy_mask(value: Option<&str>) -> Option<u32> {
    let value = value?;
    let mask: u32 = value.parse().expect("Y6_FUZZY_MASK must be a decimal 0..=255");
    assert!(mask <= 0xff, "Y6_FUZZY_MASK must be a decimal 0..=255");
    Some(mask)
}

#[test]
fn whole_f1_fuzzy_mask_accepts_documented_range_only() {
    assert_eq!(whole_f1_fuzzy_mask(None), None);
    assert_eq!(whole_f1_fuzzy_mask(Some("0")), Some(0));
    assert_eq!(whole_f1_fuzzy_mask(Some("1")), Some(1));
    assert_eq!(whole_f1_fuzzy_mask(Some("255")), Some(255));
    assert!(std::panic::catch_unwind(|| whole_f1_fuzzy_mask(Some("256"))).is_err());
    assert!(std::panic::catch_unwind(|| whole_f1_fuzzy_mask(Some("z/zh"))).is_err());
}

fn whole_y6_phases(runs: usize) -> std::ops::RangeInclusive<usize> {
    assert!(runs > 0, "at least one measured run is required");
    0..=runs
}

fn whole_y6_sequence(sequence: usize, keys: &str) -> Vec<(usize, KeyKind, String)> {
    let slot = sequence % 2;
    let mut events: Vec<_> = keys.chars().map(|ch| (slot, KeyKind::Text, ch.to_string())).collect();
    events.push((slot, KeyKind::Backspace, String::new()));
    if let Some(ch) = keys.chars().last() { events.push((slot, KeyKind::Text, ch.to_string())); }
    events.push((slot, if sequence % 5 == 0 { KeyKind::Enter } else { KeyKind::Escape }, String::new()));
    if sequence % 1000 == 0 { events.push((1 - slot, KeyKind::Escape, String::new())); }
    events
}

#[derive(Debug, PartialEq, Eq)]
struct WholeY6Ready {
    correction_generation: u64,
    correction_entries: usize,
    correction_whole_ready: bool,
    whole_generation: Option<u64>,
    whole_entries: Option<usize>,
}

fn whole_y6_ready(enabled: bool) -> WholeY6Ready {
    let deadline = Instant::now() + std::time::Duration::from_secs(30);
    loop {
        if let Some(correction) = zhixu::loader::current() {
            let whole = zhixu::loader::ready_deployed_snapshot().ok().flatten();
            if !enabled || (correction.whole_ready() && whole.is_some()) {
                return WholeY6Ready {
                    correction_generation: correction.generation,
                    correction_entries: correction.entry_count(),
                    correction_whole_ready: correction.whole_ready(),
                    whole_generation: whole.as_ref().map(|index| index.generation),
                    whole_entries: whole.as_ref().map(|index| index.entry_count()),
                };
            }
        }
        assert!(Instant::now() < deadline, "required dictionary prewarm timed out");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[test]
fn whole_y6_requested_runs_keep_one_warmup() {
    assert_eq!(whole_y6_phases(1).collect::<Vec<_>>(), vec![0, 1]);
    assert_eq!(whole_y6_phases(5).collect::<Vec<_>>(), vec![0, 1, 2, 3, 4, 5]);
}

#[test]
fn whole_y6_harness_sequence_shape() {
    let corpus: serde_json::Value = serde_json::from_str(include_str!("../../../quality/corpus/base-500.json")).unwrap();
    let cases = corpus["cases"].as_array().unwrap();
    let mut totals = [0; 4];
    for sequence in 0..1000 {
        let keys = cases[sequence % cases.len()]["query"].as_str().unwrap();
        let letters: Vec<_> = keys.chars().collect();
        assert!(!letters.is_empty());
        let events = whole_y6_sequence(sequence, keys);
        let slot = sequence % 2;
        assert_eq!(events.len(), letters.len() + 3 + usize::from(sequence % 1000 == 0));
        for (event, letter) in events.iter().zip(&letters) {
            assert_eq!(event, &(slot, KeyKind::Text, letter.to_string()));
        }
        assert_eq!(events[letters.len()], (slot, KeyKind::Backspace, String::new()));
        assert_eq!(events[letters.len() + 1], (slot, KeyKind::Text, letters.last().unwrap().to_string()));
        assert_eq!(events[letters.len() + 2],
            (slot, if sequence % 5 == 0 { KeyKind::Enter } else { KeyKind::Escape }, String::new()));
        if sequence % 1000 == 0 {
            assert_eq!(events.last().unwrap(), &(1 - slot, KeyKind::Escape, String::new()));
        }
        for (_, kind, _) in events {
            totals[match kind { KeyKind::Text => 0, KeyKind::Backspace => 1,
                KeyKind::Enter => 2, KeyKind::Escape => 3, _ => unreachable!() }] += 1;
        }
    }
    assert_eq!(totals, [8248, 1000, 200, 801]);
    assert_eq!(whole_y6_sequence(1000, "a").last().unwrap(), &(1, KeyKind::Escape, String::new()));
    assert_eq!(whole_y6_sequence(1, ""), vec![(1, KeyKind::Backspace, String::new()),
        (1, KeyKind::Escape, String::new())]);
}

/// Mirror the controller harness warm-up and measured key stream, with isolated public data.
#[test]
#[ignore]
fn whole_y5c_harness_latency_profile() {
    let shared = std::env::var("ZHIXU_Z8_SHARED").unwrap();
    let enabled = std::env::var("ZHIXU_Y5C_ON").unwrap() == "1";
    let runs = std::env::var("ZHIXU_Y6_RUNS").ok().map_or(1, |value| value.parse::<usize>().unwrap());
    assert!(std::env::var_os("ZHIXU_Y5C_SECONDS").is_none(), "stable comparisons require the complete fixed sequence");
    let corpus: serde_json::Value = serde_json::from_str(include_str!("../../../quality/corpus/base-500.json")).unwrap();
    let cases = corpus["cases"].as_array().unwrap();
    let user = std::env::temp_dir().join(format!("zhixu-y5c-{}", std::process::id()));
    std::fs::create_dir(&user).unwrap();
    let mut sessions: Vec<_> = ["hot", "hot-secondary"].iter().map(|suffix| {
        let rime = RimeDecoder::new(&shared, user.to_str().unwrap()).unwrap();
        let mut session = crate::state::Session::new_with_blacklist(Box::new(rime),
            format!("com.agentinputlayer.performance.{suffix}"),
            crate::blacklist::shared_blacklist(&user).unwrap()).unwrap();
        session.configure_zhixu_whole(enabled).unwrap();
        session
    }).collect();
    // F1 measurement parameter: configure both sessions right after construction, before any warm-up.
    // Without Y6_FUZZY_MASK nothing here runs and behavior is unchanged.
    if let Some(mask) = whole_f1_fuzzy_mask(std::env::var("Y6_FUZZY_MASK").ok().as_deref()) {
        for session in &mut sessions { session.configure_fuzzy(mask).unwrap(); }
        println!("Y6_FUZZY {}", serde_json::json!({"mask": mask}));
    }
    let qos_setting = std::env::var("ZHIXU_Y6_QOS").unwrap_or_else(|_| "inherit".into());
    let qos_request = whole_y6_qos_request(&qos_setting);
    let thread_before = whole_y6_thread_state();
    #[cfg(target_os = "macos")]
    let qos_set_status = qos_request.map(whole_y6_macos::set_qos);
    #[cfg(not(target_os = "macos"))]
    let qos_set_status: Option<i32> = {
        assert!(qos_request.is_none(), "interactive QoS diagnostic is only supported on macOS");
        None
    };
    #[cfg(target_os = "macos")]
    let clock_resolution_ns = whole_y6_macos::clock_resolution_ns();
    #[cfg(not(target_os = "macos"))]
    let clock_resolution_ns = 0u64;
    let mut empty_wall = Vec::with_capacity(1000);
    let mut empty_cpu = Vec::with_capacity(1000);
    for _ in 0..1000 {
        let start = Instant::now();
        let before = whole_y6_cpu_ns();
        let after = whole_y6_cpu_ns();
        empty_wall.push(start.elapsed().as_nanos());
        empty_cpu.push(after.checked_sub(before).expect("thread CPU clock went backwards"));
    }
    empty_wall.sort_unstable();
    empty_cpu.sort_unstable();
    println!("Y6_SCHEDULING {}", serde_json::json!({"qos_setting": qos_setting,
        "qos_set_status": qos_set_status, "thread_before": thread_before, "thread_after": whole_y6_thread_state(),
        "clock_thread_cputime_id": 16, "clock_resolution_ns": clock_resolution_ns, "empty_read_pairs": 1000,
        "empty_pair_wall_p50_ns": empty_wall[500], "empty_pair_wall_p95_ns": empty_wall[950],
        "empty_pair_cpu_p50_ns": empty_cpu[500], "empty_pair_cpu_p95_ns": empty_cpu[950]}));
    let events: Vec<_> = (0..1000).map(|sequence|
        whole_y6_sequence(sequence, cases[sequence % cases.len()]["query"].as_str().unwrap())).collect();
    let event_count = events.iter().map(Vec::len).sum();
    let mut documents = [String::with_capacity(1024), String::with_capacity(1024)];
    let mut composing = [false; 2];
    let mut id = 0;
    let mut warmed_native = [false; 2];
    for phase in whole_y6_phases(runs) {
        let measured = phase > 0;
        let ready_started = Instant::now();
        let before = whole_y6_ready(enabled);
        println!("Y6_READY {}", serde_json::json!({"on": enabled, "phase": phase, "measured": measured,
            "stage": "before", "wait_ns": ready_started.elapsed().as_nanos(),
            "correction_generation": before.correction_generation, "correction_entries": before.correction_entries,
            "whole_generation": before.whole_generation, "whole_entries": before.whole_entries,
            "correction_whole_ready": before.correction_whole_ready,
            "whole_ready": before.whole_generation.is_some(), "warmed_native": warmed_native,
            "qos_setting": qos_setting, "qos_set_status": qos_set_status, "thread": whole_y6_thread_state()}));
        if measured && enabled { assert!(warmed_native.iter().all(|ready| *ready)); }
        // Identical buffering during warm-up and measurement avoids per-key JSON,
        // Debug formatting and stdout work changing the next key's allocator state.
        let mut records = Vec::with_capacity(event_count);
        let record_capacity = records.capacity();
        let first_id = id + 1;
        let phase_started = Instant::now();
        for (sequence, events) in events.iter().enumerate() {
            let keys = cases[sequence % cases.len()]["query"].as_str().unwrap();
            for (step, (target, kind, text)) in events.iter().enumerate() {
                let (target, kind, text) = (*target, *kind, text.clone());
                if kind == KeyKind::Text && !composing[target] {
                    let document = &documents[target];
                    sessions[target].set_host_preceding_text(&document[document.len().saturating_sub(32)..]);
                }
                id += 1;
                crate::zhixu::decoder::INNER_TIMING.with(|t| *t.borrow_mut() = [0; 3]);
                let cpu_started = whole_y6_cpu_ns();
                let start = Instant::now();
                let result = sessions[target].process(KeyEvent { kind, text, modifiers: 0,
                    security: SecurityState::NonSecure, delivery_id: id });
                let ns = start.elapsed().as_nanos();
                let cpu_ns = whole_y6_cpu_ns().checked_sub(cpu_started).expect("thread CPU clock went backwards");
                let parts = crate::zhixu::decoder::INNER_TIMING.with(|t| *t.borrow());
                warmed_native[target] |= parts[0] > 0;
                composing[target] = !result.marked_text.is_empty();
                documents[target].push_str(&result.commit_text);
                let excess = documents[target].len().saturating_sub(512);
                documents[target].drain(..excess);
                assert_eq!(result.status, 0);
                if kind == KeyKind::Enter { assert_eq!(result.commit_text, keys); }
                records.push((sequence, step, target, kind, ns, parts, result, cpu_ns));
            }
            std::thread::sleep(std::time::Duration::from_millis(40));
        }
        let elapsed = phase_started.elapsed();
        let after = whole_y6_ready(enabled);
        assert_eq!(before.correction_generation, after.correction_generation, "correction generation changed within a phase");
        if enabled { assert_eq!(before, after, "deployed dictionary changed within a phase"); }
        assert_eq!(records.len(), event_count);
        assert_eq!(records.capacity(), record_capacity, "measurement buffer must never grow");
        assert!(composing.iter().all(|value| !value));
        if enabled { assert!(warmed_native.iter().all(|ready| *ready), "both sessions must have generated native candidates"); }
        println!("Y6_READY {}", serde_json::json!({"on": enabled, "phase": phase, "measured": measured,
            "stage": "after", "correction_generation": after.correction_generation, "correction_entries": after.correction_entries,
            "whole_generation": after.whole_generation, "whole_entries": after.whole_entries,
            "correction_whole_ready": after.correction_whole_ready,
            "whole_ready": after.whole_generation.is_some(), "warmed_native": warmed_native,
            "elapsed_ns": elapsed.as_nanos(), "first_delivery_id": first_id, "last_delivery_id": id,
            "buffered_results": records.len(), "qos_setting": qos_setting,
            "qos_set_status": qos_set_status, "thread": whole_y6_thread_state()}));
        if measured {
            for (sequence, step, target, kind, ns, parts, result, cpu_ns) in &records {
                let case = &cases[*sequence % cases.len()];
                println!("Y5C_TRACE {}", serde_json::json!({"on": enabled, "run": phase - 1,
                    "sequence": sequence, "step": step, "target": target, "case": case["id"], "query": case["query"],
                    "kind": format!("{kind:?}"), "ns": ns, "cpu_ns": cpu_ns,
                    "parts": parts, "result": format!("{result:?}")}));
            }
            let mut ordered: Vec<_> = records.iter().collect();
            ordered.sort_unstable_by_key(|record| record.4);
            println!("Y5C_SUMMARY on={enabled} run={} n={} p50_ns={} p95_ns={} p99_ns={}", phase - 1, ordered.len(),
                ordered[ordered.len()/2].4, ordered[ordered.len()*95/100].4, ordered[ordered.len()*99/100].4);
            for record in ordered.iter().rev().take(ordered.len().div_ceil(100)) {
                println!("Y5C_TAIL {}", serde_json::json!({"on": enabled, "run": phase - 1,
                    "sequence": record.0, "step": record.1, "target": record.2,
                    "query": cases[record.0 % cases.len()]["query"], "kind": format!("{:?}", record.3),
                    "ns": record.4, "cpu_ns": record.7, "parts": record.5}));
            }
        }
        drop(records);
        if !measured { std::thread::sleep(std::time::Duration::from_secs(10)); }
    }
    println!("Y5C_USER {}", user.display());
}

/// 知字 suspended on 10-29: the host timed the two context-mask calls together with the
/// model, so their cost counted against the 8 ms budget. Times exactly those two calls
/// after every composing letter, the way CoreSession asks for them.
#[test]
#[ignore]
fn context_mask_latency_profile() {
    let shared = std::env::var("ZHIXU_Z8_SHARED").unwrap();
    let enabled = std::env::var("ZHIXU_Y5C_ON").unwrap() == "1";
    let corpus: serde_json::Value = serde_json::from_str(include_str!("../../../quality/corpus/base-500.json")).unwrap();
    let cases = corpus["cases"].as_array().unwrap();
    // MASK_USER: a throwaway copy of a real user directory; MASK_OWNER_CONFIG=1 mirrors the
    // owner's switches (typo correction, second row, zhixu learning).
    let user = std::env::var("MASK_USER").map(std::path::PathBuf::from).unwrap_or_else(|_| {
        let user = std::env::temp_dir().join(format!("zhizi-mask-latency-{}", std::process::id()));
        std::fs::create_dir(&user).unwrap();
        user
    });
    let rime = RimeDecoder::new(&shared, user.to_str().unwrap()).unwrap();
    let mut session = crate::state::Session::new_with_blacklist(Box::new(rime),
        "com.agentinputlayer.performance.mask".into(), crate::blacklist::shared_blacklist(&user).unwrap()).unwrap();
    if std::env::var("MASK_OWNER_CONFIG").as_deref() == Ok("1") {
        session.configure_learning(crate::zhixu::learning::LearningMode::Zhixu).unwrap();
        session.configure_typo(true);
        session.configure_candidate_context(None, Vec::new(), true, false);
    }
    session.configure_zhixu_whole(enabled).unwrap();
    whole_y6_ready(enabled);
    let mut id = 0;
    for phase in 0..2 {
        let (mut masks, mut keys) = (Vec::new(), Vec::new());
        for case in cases.iter().take(300) {
            let query = case["query"].as_str().unwrap();
            session.set_host_preceding_text("我们今天先把这个");
            for letter in query.chars() {
                id += 1;
                let start = Instant::now();
                session.process(KeyEvent { kind: KeyKind::Text, text: letter.to_string(), modifiers: 0,
                    security: SecurityState::NonSecure, delivery_id: id });
                keys.push(start.elapsed().as_nanos());
                let start = Instant::now();
                let mask = session.context_group_mask() | session.context_complete_mask();
                masks.push(start.elapsed().as_nanos());
                std::hint::black_box(mask);
            }
            id += 1;
            session.process(KeyEvent { kind: KeyKind::Escape, text: String::new(), modifiers: 0,
                security: SecurityState::NonSecure, delivery_id: id });
        }
        if phase == 0 { continue; }
        for (name, values) in [("key", &mut keys), ("masks", &mut masks)] {
            values.sort_unstable();
            let at = |p: usize| values[values.len() * p / 100] as f64 / 1e6;
            println!("MASK_SUMMARY on={enabled} part={name} n={} p50_ms={:.3} p90_ms={:.3} p95_ms={:.3} p99_ms={:.3}",
                values.len(), at(50), at(90), at(95), at(99));
        }
    }
}
