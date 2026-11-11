struct RimeOrderFixture(std::path::PathBuf);

impl RimeOrderFixture {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let directory = std::env::temp_dir().join(format!("q12-rime-order-cache-{}-{}", std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
        std::fs::create_dir(&directory).unwrap();
        let mut store = zhixu::learning::LearningStore::default();
        store.import_rime_export("你好\tni hao\t7\n项目\txiang mu\t3\n", personal_picks::now_ms()).unwrap();
        store.save(&directory).unwrap();
        Self(directory)
    }

    fn mixer(&self, native: bool) -> MixerDecoder {
        let mut mixer = MixerDecoder::new(Box::new(ListDecoder::default()), "q12-rime-order-cache".into());
        self.set_native(&mut mixer, native);
        mixer.learning.attach(&self.0);
        mixer.learning.set_mode(LearningMode::Zhixu, &zhixu::fallback());
        mixer
    }

    fn set_native(&self, mixer: &mut MixerDecoder, native: bool) {
        let mut decoder = ListDecoder::with("nihao", &[("你好", "ni hao"), ("你", "ni")]);
        decoder.exact_boundary = native.then(|| CompositionBoundary { confirmed: String::new(), unconfirmed_raw: "nihao".into() });
        mixer.inner = Box::new(decoder);
    }
}

impl Drop for RimeOrderFixture {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
}

fn original_rime_order(mixer: &MixerDecoder, raw: &str) -> bool {
    mixer.learning.use_rime_order(raw)
        && (!mixer.inner.native_editing()
            || !mixer.learning.entries().iter().any(|entry| mixer.learned_abbreviation_matches(raw, entry)))
}

#[test]
fn key_latency_cache_reuses_same_raw_revision_evaluation() {
    let fixture = RimeOrderFixture::new();
    let mixer = fixture.mixer(true);
    let expected = original_rime_order(&mixer, "nh");
    mixer.rime_order_evaluations.set(0);
    for _ in 0..20 { assert_eq!(mixer.use_rime_order("nh"), expected); }
    assert_eq!(mixer.rime_order_evaluations.get(), 1, "same raw and learning facts need only one scan");
    assert_eq!(mixer.use_rime_order("xm"), original_rime_order(&mixer, "xm"));
    assert_eq!(mixer.rime_order_evaluations.get(), 2, "a different raw input must evaluate independently");
    assert_eq!(mixer.use_rime_order("nh"), expected);
    assert_eq!(mixer.rime_order_evaluations.get(), 3, "only one raw input is retained");
}

#[test]
fn key_latency_cache_matches_original_across_modes_native_and_learning_revisions() {
    let fixture = RimeOrderFixture::new();
    let mut mixer = fixture.mixer(true);
    let mut peer = fixture.mixer(true);
    let raws = ["nihao", "nh", "nhao", "ni'hao", "ni hao", "xiangmu", "xm", "xmu", "wode", ""];
    let compare = |mixer: &MixerDecoder| {
        for raw in raws {
            let expected = original_rime_order(mixer, raw);
            for _ in 0..3 { assert_eq!(mixer.use_rime_order(raw), expected, "{raw}"); }
        }
    };
    compare(&mixer);
    assert!(mixer.use_rime_order("nh"));
    let before = mixer.learning.revision();
    peer.learning.remember_for_test("ni hao", "你好", personal_picks::now_ms());
    assert_ne!(mixer.learning.revision(), before, "peer learning invalidates the shared revision");
    assert!(!mixer.use_rime_order("nh"));
    compare(&mixer);

    assert!(mixer.use_rime_order("wode"));
    let revision = mixer.learning.revision();
    mixer.learning.mode = LearningMode::Rime;
    assert_eq!(mixer.learning.revision(), revision, "mode toggles can leave learning facts unchanged");
    assert!(!mixer.use_rime_order("wode"));
    compare(&mixer);
    assert!(!mixer.use_rime_order("wode"));
    mixer.learning.mode = LearningMode::Zhixu;
    assert!(mixer.use_rime_order("wode"));
    compare(&mixer);

    assert!(!mixer.use_rime_order("nh"));
    fixture.set_native(&mut mixer, false);
    assert_eq!(mixer.learning.revision(), revision);
    assert!(mixer.use_rime_order("nh"), "legacy editing keeps the original abbreviation behavior");
    compare(&mixer);
    fixture.set_native(&mut mixer, true);
    assert!(!mixer.use_rime_order("nh"));

    let revision = mixer.learning.revision();
    peer.learning.forget("ni hao", "你好").unwrap();
    assert_ne!(mixer.learning.revision(), revision, "peer forgetting invalidates the cached result");
    assert!(mixer.use_rime_order("nh"));
    compare(&mixer);

    let expected = mixer.snapshot().candidates;
    for _ in 0..4 { assert_eq!(mixer.snapshot().candidates, expected, "cache hits preserve every candidate field"); }
}

#[test]
fn key_latency_cache_attach_changes_knowledge_and_poison_falls_back() {
    let first = RimeOrderFixture::new();
    let second = RimeOrderFixture::new();
    let mut mixer = first.mixer(true);
    mixer.learning.remember_for_test("ni hao", "你好", personal_picks::now_ms());
    assert!(!mixer.use_rime_order("nh"));
    let revision = mixer.learning.revision();
    mixer.learning.attach(&second.0);
    mixer.learning.set_mode(LearningMode::Zhixu, &zhixu::fallback());
    assert_ne!(mixer.learning.revision(), revision, "new Knowledge has its own revision");
    assert!(mixer.use_rime_order("nh"));
    mixer.learning.attach(&first.0);
    mixer.learning.set_mode(LearningMode::Zhixu, &zhixu::fallback());
    // The first store no longer has another session retaining its in-memory
    // facts, so attaching reloads exactly what its synthetic file contains.
    assert_eq!(mixer.use_rime_order("nh"), original_rime_order(&mixer, "nh"));

    let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _cache = mixer.rime_order_cache.lock().unwrap();
        panic!("synthetic cache poison");
    }));
    assert!(poisoned.is_err());
    let expected = original_rime_order(&mixer, "nh");
    mixer.rime_order_evaluations.set(0);
    for _ in 0..3 { assert_eq!(mixer.use_rime_order("nh"), expected); }
    assert_eq!(mixer.rime_order_evaluations.get(), 3, "poisoned caches always use the original scan");
}
