use super::*;
use crate::zhixu::{learning, loader::{DictionaryData, Entry}};

fn base() -> Arc<Index> {
    Arc::new(Index::from_data(DictionaryData {
        entries: [
            ("我", "wo", 800, "pinyin_simp"),
            ("了", "le", 800, "pinyin_simp"),
            ("要", "yao", 900, "pinyin_simp"),
            ("药", "yao", 500, "pinyin_simp"),
            ("记得", "ji de", 800, "pinyin_simp"),
            ("药剂", "yao ji", 100, "agent_spoken"),
            ("的", "de", 800, "pinyin_simp"),
            ("再搞", "zai gao", 800, "agent_owner_words"),
            ("一二三四五六七八", "yi er san si wu liu qi ba", 800, "agent_owner_words"),
            ("冷门", "leng men", 1, "pinyin_simp"),
        ].into_iter().map(|(text, reading, weight, layer)| Entry {
            text: text.into(), reading: reading.into(), weight, layer: layer.into(),
        }).collect(),
    }))
}

fn learned(reading: &str, text: &str, count: u32) -> learning::Entry {
    learning::Entry { reading: reading.into(), text: text.into(), count, last_ms: 1, rime: None }
}

#[test]
fn learning_index_seeds_and_all_readings_preserve_the_existing_projection() {
    let index = base();
    assert_eq!(index.owner_words(), vec![
        ("再搞".into(), "zai gao".into(), 800),
        ("一二三四五六七八".into(), "yi er san si wu liu qi ba".into(), 800),
    ]);
    assert_eq!(index.all_readings_for_text("冷门").collect::<Vec<_>>(), vec!["leng men"]);
    assert_eq!(index.all_readings_for_text("一二三四五六七八").collect::<Vec<_>>(), vec!["yi er san si wu liu qi ba"]);
    assert!(index.readings_for_text("冷门").next().is_none());
    assert!(index.readings_for_text("一二三四五六七八").next().is_none());
}

#[test]
fn learning_index_composes_new_words_between_base_words() {
    let personal = Arc::new(Index::from_learning(&[learned("cai wo", "裁我", 1)]));
    assert_eq!(personal.words_for_initials("cw").next().unwrap().0, "裁我");
    zhixu::with_index(base(), || {
        assert!(decode_compositions("wocaiwole", 12).is_empty());
        let _learning = zhixu::pin_learning(Some(personal));
        assert!(decode_compositions("wocaiwole", 12).contains(&"我裁我了".into()));
        assert!(decode_lattice("wocaiwole", 12, false).contains(&"我裁我了".into()));
        assert!(decode_compositions_with("wocwle", 12, true).contains(&"我裁我了".into()));
        assert!(decode_lattice("wocwle", 12, true).contains(&"我裁我了".into()));
    });
}

#[test]
fn learning_index_single_character_counts_never_change_composition() {
    zhixu::with_index(base(), || {
        let original = decode_compositions("yaojide", 12);
        let lattice = decode_lattice("yaojide", 12, false);
        let personal = Arc::new(Index::from_learning(&[learned("yao", "药", 1_000_000)]));
        assert_eq!(personal.entry_count(), 0);
        let _learning = zhixu::pin_learning(Some(personal));
        assert_eq!(decode_compositions("yaojide", 12), original);
        assert_eq!(decode_lattice("yaojide", 12, false), lattice);
        assert_eq!(original.first().map(String::as_str), Some("要记得"));
    });
}

#[test]
fn learning_index_word_frequency_is_not_a_composition_weight() {
    let low = Index::from_learning(&[learned("cai wo", "裁我", 1)]);
    let high = Index::from_learning(&[learned("cai wo", "裁我", 1_000_000)]);
    assert_eq!(low.words_for_reading("cai wo").collect::<Vec<_>>(), high.words_for_reading("cai wo").collect::<Vec<_>>());
    assert_eq!(high.words_for_reading("cai wo").next(), Some(("裁我", 800)));
}

#[test]
fn learning_index_seed_duplicates_keep_existing_compositions() {
    zhixu::with_index(base(), || {
        let original = decode_compositions("wozaigaole", 12);
        let lattice = decode_lattice("wozaigaole", 12, false);
        let personal = Arc::new(Index::from_learning(&[learned("zai gao", "再搞", 1_000_000)]));
        let _learning = zhixu::pin_learning(Some(personal));
        assert_eq!(decode_compositions("wozaigaole", 12), original);
        assert_eq!(decode_lattice("wozaigaole", 12, false), lattice);
    });
}

#[test]
fn learning_index_long_words_and_nested_pin_restore() {
    let text = "一二三四五六七八";
    let personal = Arc::new(Index::from_learning(&[learned("yi er san si wu liu qi ba", text, 1)]));
    assert_eq!(personal.words_for_reading("yi er san si wu liu qi ba").next().unwrap().0, text);
    assert!(zhixu::learning_index().is_none());
    {
        let _personal = zhixu::pin_learning(Some(personal.clone()));
        {
            let _disabled = zhixu::pin_learning(None);
            assert!(zhixu::learning_index().is_none());
        }
        assert!(Arc::ptr_eq(&zhixu::learning_index().unwrap(), &personal));
    }
    assert!(zhixu::learning_index().is_none());
}
