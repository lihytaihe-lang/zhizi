use super::*;
use crate::zhixu::loader::{DictionaryData, Entry};

fn tiny_index() -> Arc<Index> {
    Arc::new(Index::from_data(DictionaryData {
        entries: vec![
            Entry {
                text: "量子猫".into(),
                reading: "liang zi mao".into(),
                weight: 900,
                layer: "downloaded".into(),
            },
            Entry {
                text: "QuantumCat".into(),
                reading: "quantumcat".into(),
                weight: 800,
                layer: "downloaded".into(),
            },
        ],
    }))
}

#[test]
fn runtime_snapshot_reaches_every_dictionary_consumer() {
    let index = tiny_index();
    zhixu::with_index(index, || {
        assert!(decode_lattice("liangzimao", 24, false).contains(&"量子猫".to_string()));
        assert_eq!(decode_compositions("liangzimao", 2), ["量子猫"]);
        assert!(decode_heavy_garble("liangizmao").contains(&"量子猫".to_string()));
        assert!(vocabulary_word("量子猫", "liangzimao"));
        assert!(abbreviated_vocabulary_word("量子猫", "lzm"));
        assert_eq!(
            words_with_initials(None, &["l", "z", "m"]).collect::<Vec<_>>(),
            ["量子猫"]
        );
        assert_eq!(tech_terms_for_reading_prefix("quant"), ["QuantumCat"]);
        assert!(has_registered_ascii_exact("quantumcat", false));
        assert_eq!(
            local_ascii_candidates("quantumcat", &[], false, false)[0]
                .candidate
                .text,
            "QuantumCat"
        );
    });
}

#[test]
fn fallback_snapshot_keeps_the_original_word_weights_and_first_results() {
    let fallback = zhixu::fallback();
    let original: HashMap<_, _> = include_str!("../../resources/heavy_garble_words.tsv")
        .lines()
        .filter(|line| !line.starts_with('#'))
        .filter_map(|line| {
            let mut columns = line.split('\t');
            Some((
                (columns.next()?.to_string(), columns.next()?.to_string()),
                columns.next()?.parse::<u32>().ok()?,
            ))
        })
        .collect();
    let actual: HashMap<_, _> = fallback.projection_words()
        .map(|(text, reading, weight)| ((text.to_string(), reading.to_string()), weight))
        .collect();
    assert_eq!(actual, original);
    let before = (
        decode_lattice("taifuzale", 24, false),
        decode_heavy_garble("gengixn"),
    );
    let loaded = zhixu::with_index(tiny_index(), || decode_lattice("liangzimao", 24, false));
    assert_eq!(loaded.first().map(String::as_str), Some("量子猫"));
    let after = (
        decode_lattice("taifuzale", 24, false),
        decode_heavy_garble("gengixn"),
    );
    assert_eq!(before, after);
    assert!(before.0.contains(&"太复杂了".to_string()));
    assert!(before.1.contains(&"更新".to_string()));
}

#[test]
fn empty_runtime_ascii_snapshot_does_not_reuse_the_previous_terms() {
    zhixu::with_index(tiny_index(), || {
        assert!(has_registered_ascii_exact("quantumcat", false))
    });
    zhixu::with_index(
        Arc::new(Index::from_data(DictionaryData::default())),
        || {
            assert!(english_term_readings().is_empty());
            assert!(tech_term_readings().is_empty());
            assert!(project_term_readings().is_empty());
            assert!(canonical_embedded_ascii_terms().is_empty());
            assert!(!has_registered_ascii_exact("quantumcat", false));
        },
    );
}
