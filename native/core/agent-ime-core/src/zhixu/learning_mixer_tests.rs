#[test]
fn learning_whole_words_beat_partial_only_after_learning() {
    use crate::zhixu::learning::LearningMode;
    for (raw, partial, whole, reading) in [("zaigao", "再", "再搞", "zai gao"), ("caiwo", "裁", "裁我", "cai wo")] {
        let mut mixer = MixerDecoder::new(Box::new(ListDecoder::with(raw, &[(partial, raw), (whole, raw)])), format!("learning-{raw}"));
        let before = mixer.snapshot();
        mixer.configure_learning(LearningMode::Zhixu).unwrap();
        mixer.learning.remember_for_test(reading, whole, personal_picks::now_ms());
        assert_eq!(mixer.snapshot().candidates[0].text, whole);
        mixer.configure_learning(LearningMode::Rime).unwrap();
        assert_eq!(mixer.snapshot().candidates, before.candidates);
    }
}

#[test]
fn learning_does_not_guess_unlearned_full_coverage() {
    let mut mixer = MixerDecoder::new(Box::new(ListDecoder::with("meikando", &[("没看到", "mei kan dao"), ("没看懂", "mei kan dong")])), "learning-pieces".into());
    let before = mixer.snapshot();
    mixer.configure_learning(crate::zhixu::learning::LearningMode::Zhixu).unwrap();
    assert_eq!(mixer.snapshot().candidates, before.candidates);
    assert_eq!(mixer.snapshot().candidates[0].text, "没看到");
}

#[test]
fn learning_one_pick_moves_at_most_three_places_and_host_cannot_restore_partial_top() {
    let mut mixer = MixerDecoder::new(Box::new(ListDecoder::with("zai", &[("在", "zai"), ("载", "zai"), ("宰", "zai"), ("栽", "zai"), ("再", "zai")])), "learning-bounded".into());
    mixer.configure_learning(crate::zhixu::learning::LearningMode::Zhixu).unwrap();
    mixer.learning.remember_for_test("zai", "再", personal_picks::now_ms());
    assert_eq!(mixer.snapshot().candidates.iter().position(|c| c.text == "再"), Some(1));

    let mut mixer = MixerDecoder::new(Box::new(ListDecoder::with("zaigao", &[("再", "zai"), ("再搞", "zai gao")])), "learning-host-order".into());
    mixer.configure_learning(crate::zhixu::learning::LearningMode::Zhixu).unwrap();
    mixer.learning.remember_for_test("zai gao", "再搞", personal_picks::now_ms());
    assert_eq!(mixer.snapshot().candidates[0].text, "再搞");
    mixer.configure_preceding_text(Some("停了，".into()));
    assert!(mixer.reorder_context_candidates(&[1, 0]).is_err());
    assert_eq!(mixer.select_candidate(0, 99, LearningIntent::NoLearn).unwrap(), "再搞");
}

#[test]
fn learning_reading_uses_dictionary_syllables_and_consumed_segment_only() {
    assert_eq!(resolve_learning_reading("zaigao", "再"), Some(("zai".into(), 3)));
    assert_eq!(resolve_learning_reading("caiwo", "裁我"), Some(("cai wo".into(), 5)));
    assert_eq!(resolve_learning_reading("yaojide", "要记得"), Some(("yao ji de".into(), 7)));
    assert!(resolve_learning_reading("abc", "不存在读音").is_none());
    assert!(resolve_learning_reading("nikk", "API").is_none());
}

#[test]
fn learning_dao_does_not_replace_the_settled_bare_d() {
    let mut mixer = MixerDecoder::new(Box::new(ListDecoder::with("d", &[("到", "dao"), ("的", "de")])), "learning-letter".into());
    mixer.configure_learning(crate::zhixu::learning::LearningMode::Zhixu).unwrap();
    mixer.learning.import_choices(vec![Choice { app: "learning-letter".into(), raw: "d".into(), reading: "de".into(), text: "的".into(),
        target: 0, count: 10, last_ms: personal_picks::now_ms(), alternatives: vec!["到".into()] }]);
    for _ in 0..2 { mixer.learning.remember_for_test("dao", "到", personal_picks::now_ms()); }
    assert_eq!(mixer.snapshot().candidates[0].text, "的");
}

#[test]
fn learning_migrated_whole_words_rank_without_guessed_coverage() {
    let mut store = crate::zhixu::learning::LearningStore::default();
    store.import_rime_export("再搞\tzai gao\t7\n裁我\tcai wo\t3\n", personal_picks::now_ms()).unwrap();
    for entry in store.entries() {
        let raw = entry.reading.replace(' ', "");
        let partial: String = entry.text.chars().take(1).collect();
        let mut mixer = MixerDecoder::new(Box::new(ListDecoder::with(&raw, &[(&partial, &raw)])), "learning-import".into());
        mixer.configure_learning(crate::zhixu::learning::LearningMode::Zhixu).unwrap();
        for _ in 0..entry.count { mixer.learning.remember_for_test(&entry.reading, &entry.text, entry.last_ms); }
        assert_eq!(mixer.snapshot().candidates[0].text, entry.text);
    }
}

#[test]
fn learning_single_characters_do_not_change_daluanjihua_composition() {
    use crate::zhixu::{learning::Entry as Learned, loader::{DictionaryData, Entry}};
    let base = Arc::new(Index::from_data(DictionaryData { entries: [
        ("打乱", "da luan", 1000), ("计划", "ji hua", 1000), ("大陆", "da lu", 800),
        ("按", "an", 500), ("大", "da", 500), ("陆", "lu", 500),
    ].into_iter().map(|(text, reading, weight)| Entry { text: text.into(), reading: reading.into(), weight, layer: "fixture".into() }).collect() }));
    zhixu::with_index(base, || {
        let original = decode_compositions("daluanjihua", 12);
        let lattice = decode_lattice("daluanjihua", 12, false);
        assert_eq!(original[0], "打乱计划");
        let personal = Arc::new(Index::from_learning(&[
            Learned { text: "大".into(), reading: "da".into(), count: 1_000_000, last_ms: 1, rime: None },
            Learned { text: "陆".into(), reading: "lu".into(), count: 1_000_000, last_ms: 1, rime: None },
        ]));
        let _guard = zhixu::pin_learning(Some(personal));
        assert_eq!(decode_compositions("daluanjihua", 12), original);
        assert_eq!(decode_lattice("daluanjihua", 12, false), lattice);
    });
}

#[test]
fn learning_new_word_invalidates_same_reading_composition_cache() {
    use crate::zhixu::loader::{DictionaryData, Entry};
    let base = Arc::new(Index::from_data(DictionaryData { entries: [("我", "wo"), ("了", "le")]
        .into_iter().map(|(text, reading)| Entry { text: text.into(), reading: reading.into(), weight: 800, layer: "fixture".into() }).collect() }));
    zhixu::with_index(base, || {
        let mut mixer = MixerDecoder::new(Box::new(ListDecoder::with("wocaiwole", &[("我", "wo")])), "learning-cache".into());
        mixer.configure_learning(crate::zhixu::learning::LearningMode::Zhixu).unwrap();
        assert!(!mixer.composition_alternatives().contains(&"我裁我了".into()));
        mixer.learning.remember_for_test("cai wo", "裁我", personal_picks::now_ms());
        assert!(mixer.composition_alternatives().contains(&"我裁我了".into()));
    });
}

fn migration_fixture(export: &str) -> std::path::PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let directory = std::env::temp_dir().join(format!("aim-migration-fidelity-{}-{}",
        std::process::id(), NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
    std::fs::create_dir(&directory).unwrap();
    let mut store = crate::zhixu::learning::LearningStore::default();
    assert!(store.import_rime_export(export, personal_picks::now_ms()).unwrap() > 0);
    store.save(&directory).unwrap();
    directory
}

#[test]
fn migration_keeps_recent_ta_first_and_the_entire_first_page() {
    let directory = migration_fixture("#@/tick\t1000\nta \t他\tc=37 d=0.1 t=10\nta \t它\tc=7 d=7 t=1000\n");
    let values = [("它", "ta"), ("他", "ta"), ("她", "ta"), ("塔", "ta"), ("塌", "ta")];
    let mut mixer = MixerDecoder::new(Box::new(ListDecoder::with("ta", &values)), "zhizi.eval".into());
    let before = mixer.snapshot().candidates;
    assert_eq!(before[0].text, "它");
    mixer.learning.attach(&directory);
    mixer.configure_learning(crate::zhixu::learning::LearningMode::Zhixu).unwrap();
    assert_eq!(mixer.snapshot().candidates, before);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn migration_does_not_add_whole_word_protection_or_composition_candidates() {
    use crate::zhixu::loader::{DictionaryData, Entry};
    let directory = migration_fixture("#@/tick\t1000\ncai wo \t裁我\tc=7 d=7 t=1000\n");
    for values in [
        vec![("裁", "cai"), ("裁我", "cai wo"), ("猜", "cai"), ("财", "cai"), ("采", "cai")],
        vec![("裁", "cai"), ("猜", "cai"), ("才", "cai"), ("财", "cai"), ("采", "cai")],
    ] {
        let mut mixer = MixerDecoder::new(Box::new(ListDecoder::with("caiwo", &values)), "migration-whole".into());
        let before = mixer.snapshot().candidates;
        mixer.learning.attach(&directory);
        mixer.configure_learning(crate::zhixu::learning::LearningMode::Zhixu).unwrap();
        assert_eq!(mixer.snapshot().candidates, before);
    }
    let base = Arc::new(Index::from_data(DictionaryData { entries: [("我", "wo"), ("了", "le")]
        .into_iter().map(|(text, reading)| Entry { text: text.into(), reading: reading.into(), weight: 800, layer: "fixture".into() }).collect() }));
    zhixu::with_index(base, || {
        let mut mixer = MixerDecoder::new(Box::new(ListDecoder::with("wocaiwole", &[("我", "wo")])), "migration-composition".into());
        let before = mixer.composition_alternatives();
        assert!(!before.contains(&"我裁我了".into()));
        mixer.learning.attach(&directory);
        mixer.configure_learning(crate::zhixu::learning::LearningMode::Zhixu).unwrap();
        assert_eq!(mixer.composition_alternatives(), before);
    });
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn migration_preserves_app_choices_before_context_ranking() {
    let directory = migration_fixture("#@/tick\t1000\nta \t他\tc=37 d=0.1 t=10\nta \t它\tc=7 d=7 t=1000\n");
    let app = "migration-legacy-app";
    let key = (app.to_string(), "ta".to_string(), "塌".to_string());
    let now = personal_picks::now_ms();
    {
        let mut legacy = shared_learning().lock().unwrap();
        legacy.target_positions.insert(key.clone(), 0);
        legacy.counts.insert(key.clone(), 3);
        legacy.last_ms.insert(key.clone(), now);
    }
    let model = Arc::new(ContextModel::from_json(
        r#"{"rules":[{"pinyin":"ta","senses":[{"word":"他","triggers":["朋友"]}]}]}"#.as_bytes()).unwrap());
    for current_app in [app, "zhizi.eval"] {
        let values = [("它", "ta"), ("他", "ta"), ("她", "ta"), ("塔", "ta"), ("塌", "ta")];
        let mut mixer = MixerDecoder::new(Box::new(ListDecoder::with("ta", &values)), current_app.into());
        mixer.apply_context_model(model.clone());
        mixer.configure_preceding_text(Some("朋友".into()));
        let before = mixer.snapshot().candidates;
        assert_eq!(before[0].text, "他");
        assert_eq!(before[1].text, if current_app == app { "塌" } else { "它" });
        mixer.learning.attach(&directory);
        mixer.configure_learning(crate::zhixu::learning::LearningMode::Zhixu).unwrap();
        mixer.learning.import_choices(vec![Choice { app: app.into(), raw: "ta".into(), reading: "ta".into(), text: "塌".into(),
            target: 0, count: 3, last_ms: now, alternatives: Vec::new() }]);
        assert_eq!(mixer.snapshot().candidates, before, "app {current_app}");
    }
    {
        let mut legacy = shared_learning().lock().unwrap();
        legacy.target_positions.remove(&key);
        legacy.counts.remove(&key);
        legacy.last_ms.remove(&key);
    }
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn migration_preserves_top_five_for_sixty_synthetic_readings() {
    let syllables = ["ta", "na", "ba", "ma", "da", "gen", "zai", "you", "shi", "bu",
        "cai", "wo", "yao", "ji", "de", "kan", "dao", "rang", "le", "hao"];
    let mut export = String::from("#@/tick\t1000\n");
    let mut cases = Vec::new();
    for (number, syllable) in syllables.iter().enumerate() {
        for length in [1, 2, 5] {
            let reading = std::iter::repeat_n(*syllable, length).collect::<Vec<_>>().join(" ");
            let texts: Vec<String> = (0..6).map(|position| {
                std::iter::repeat_n(char::from_u32(0x6000 + (number * 6 + position) as u32).unwrap(), length).collect()
            }).collect();
            for (position, text) in texts.iter().enumerate() {
                let (count, weight, tick) = if position == 0 { (7, 7.0, 1000) } else { (37 + position, 0.1, 10) };
                export.push_str(&format!("{reading} \t{text}\tc={count} d={weight} t={tick}\n"));
            }
            cases.push((reading, texts));
        }
    }
    let directory = migration_fixture(&export);
    assert_eq!(cases.len(), 60);
    for (reading, texts) in cases {
        let raw = reading.replace(' ', "");
        let values: Vec<_> = texts.iter().take(5).map(|text| (text.as_str(), reading.as_str())).collect();
        let mut mixer = MixerDecoder::new(Box::new(ListDecoder::with(&raw, &values)), "migration-property".into());
        let before = mixer.snapshot().candidates;
        mixer.learning.attach(&directory);
        mixer.configure_learning(crate::zhixu::learning::LearningMode::Zhixu).unwrap();
        assert_eq!(mixer.snapshot().candidates, before, "reading {reading}");
    }
    std::fs::remove_dir_all(directory).unwrap();
}

fn migration_pick_and_confirm(mixer: &mut MixerDecoder, text: &str, id: u64, now: u64) {
    let position = mixer.snapshot().candidates.iter().position(|candidate| candidate.text == text).unwrap();
    assert_eq!(mixer.select_candidate(position, id, LearningIntent::DeliberateSelection).unwrap(), text);
    mixer.learning_commit(text, id, now);
    mixer.learning_key(&crate::state::KeyEvent { kind: crate::state::KeyKind::Unsupported, text: String::new(),
        modifiers: 0, security: crate::state::SecurityState::NonSecure, delivery_id: 0 }, now + 5_001);
}

#[test]
fn migration_recent_ta_changes_only_after_new_confirmed_selections() {
    let directory = migration_fixture("#@/tick\t1000\nta \t他\tc=37 d=12 t=1\nta \t它\tc=7 d=8 t=1000\n");
    let values = [("它", "ta"), ("他", "ta"), ("她", "ta"), ("塔", "ta"), ("塌", "ta")];
    let mut mixer = MixerDecoder::new(Box::new(ListDecoder::with("ta", &values)), "migration-new-ta".into());
    mixer.learning.attach(&directory);
    mixer.configure_learning(crate::zhixu::learning::LearningMode::Zhixu).unwrap();
    assert_eq!(mixer.snapshot().candidates[0].text, "它");
    let now = personal_picks::now_ms();
    for selection in 1..=10 {
        migration_pick_and_confirm(&mut mixer, "他", 1000 + selection, now + selection * 6_000);
        mixer.input("ta").unwrap();
        if selection == 1 {
            assert_eq!(mixer.snapshot().candidates[0].text, "它");
            assert_eq!(mixer.learning.entries().iter().find(|entry| entry.text == "他").unwrap().rime.as_ref().unwrap().updates, 1);
        }
    }
    assert_eq!(mixer.snapshot().candidates[0].text, "他");
    assert_eq!(mixer.learning.choices().iter().find(|choice| choice.text == "他").unwrap().count, 10);
    drop(mixer);
    let mut restarted = MixerDecoder::new(Box::new(ListDecoder::with("ta", &values)), "migration-new-ta".into());
    restarted.learning.attach(&directory);
    restarted.configure_learning(crate::zhixu::learning::LearningMode::Zhixu).unwrap();
    assert_eq!(restarted.snapshot().candidates[0].text, "他");
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn migration_new_learning_in_another_reading_preserves_untouched_pages() {
    use crate::zhixu::loader::{DictionaryData, Entry};
    let directory = migration_fixture("#@/tick\t1000\nta \t他\tc=37 d=12 t=1\nta \t它\tc=7 d=8 t=1000\ncai wo \t裁我\tc=7 d=7 t=1000\n");
    let mut learner = MixerDecoder::new(Box::new(ListDecoder::with("caiwo", &[("裁", "cai"), ("裁我", "cai wo")])), "migration-other-reading".into());
    learner.learning.attach(&directory);
    learner.configure_learning(crate::zhixu::learning::LearningMode::Zhixu).unwrap();
    migration_pick_and_confirm(&mut learner, "裁我", 2001, personal_picks::now_ms());
    assert!(!learner.learning.use_rime_order("caiwo"));
    let values = [("它", "ta"), ("他", "ta"), ("她", "ta"), ("塔", "ta"), ("塌", "ta")];
    let mut untouched = MixerDecoder::new(Box::new(ListDecoder::with("ta", &values)), "migration-untouched".into());
    let before = untouched.snapshot().candidates;
    untouched.learning.attach(&directory);
    untouched.configure_learning(crate::zhixu::learning::LearningMode::Zhixu).unwrap();
    assert!(untouched.learning.use_rime_order("ta"));
    assert_eq!(untouched.snapshot().candidates, before);
    let base = Arc::new(Index::from_data(DictionaryData { entries: [("我", "wo"), ("了", "le")]
        .into_iter().map(|(text, reading)| Entry { text: text.into(), reading: reading.into(), weight: 800, layer: "fixture".into() }).collect() }));
    zhixu::with_index(base, || {
        let mut untouched = MixerDecoder::new(Box::new(ListDecoder::with("wocaiwole", &[("我", "wo")])), "migration-untouched-composition".into());
        let before = untouched.composition_alternatives();
        untouched.learning.attach(&directory);
        untouched.configure_learning(crate::zhixu::learning::LearningMode::Zhixu).unwrap();
        assert_eq!(untouched.composition_alternatives(), before);
    });
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn migration_owner_seeds_and_legacy_choices_do_not_open_new_learning() {
    use crate::zhixu::loader::{DictionaryData, Entry};
    let directory = migration_fixture("#@/tick\t1000\nta \t他\tc=37 d=12 t=1\nta \t它\tc=7 d=8 t=1000\n");
    let base = Arc::new(Index::from_data(DictionaryData { entries: vec![
        Entry { text: "塔".into(), reading: "ta".into(), weight: 800, layer: "agent_owner_words".into() },
    ] }));
    zhixu::with_index(base, || {
        let values = [("它", "ta"), ("他", "ta"), ("她", "ta"), ("塔", "ta"), ("塌", "ta")];
        let mut mixer = MixerDecoder::new(Box::new(ListDecoder::with("ta", &values)), "zhizi.eval".into());
        let before = mixer.snapshot().candidates;
        mixer.learning.attach(&directory);
        mixer.configure_learning(crate::zhixu::learning::LearningMode::Zhixu).unwrap();
        mixer.learning.import_choices(vec![Choice { app: "owner.app".into(), raw: "ta".into(), reading: "ta".into(),
            text: "塌".into(), count: 50, target: 0, last_ms: personal_picks::now_ms(), alternatives: Vec::new() }]);
        assert!(mixer.learning.use_rime_order("ta"));
        assert!(!mixer.learning.entries().iter().any(|entry| entry.text == "塔" || entry.text == "塌"));
        assert_eq!(mixer.snapshot().candidates, before);
    });
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn migration_homophone_characters_keep_recent_choices_until_relearned() {
    for (reading, recent, old) in [("ta", "它", "他"), ("na", "那", "哪"), ("ba", "把", "吧"), ("ma", "吗", "嘛")] {
        let export = format!("#@/tick\t1000\n{reading} \t{old}\tc=37 d=12 t=1\n{reading} \t{recent}\tc=7 d=8 t=999\n");
        let directory = migration_fixture(&export);
        let values = [(recent, reading), (old, reading)];
        let mut mixer = MixerDecoder::new(Box::new(ListDecoder::with(reading, &values)), format!("migration-homophone-{reading}"));
        let before = mixer.snapshot().candidates;
        assert_eq!(before[0].text, recent);
        mixer.learning.attach(&directory);
        mixer.configure_learning(crate::zhixu::learning::LearningMode::Zhixu).unwrap();
        assert_eq!(mixer.snapshot().candidates, before);
        let now = personal_picks::now_ms();
        for count in 1..=10 {
            mixer.learning.remember_for_test(reading, old, now + count);
            if count == 1 {
                assert_eq!(mixer.snapshot().candidates[0].text, recent, "reading {reading}");
            }
        }
        assert_eq!(mixer.snapshot().candidates[0].text, old, "reading {reading}");
        std::fs::remove_dir_all(directory).unwrap();
    }
}
