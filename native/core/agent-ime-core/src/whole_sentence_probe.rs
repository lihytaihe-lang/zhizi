//! Public-corpus comparison with a fresh librime user directory; opt-in local probe only.

use crate::zhixu::{self, Index};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;

fn unique_reading(index: &Index, text: &str) -> Result<String, &'static str> {
    let mut boundaries: Vec<_> = text.char_indices().map(|(at, _)| at).collect();
    if !(2..=8).contains(&boundaries.len())
        || !text.chars().all(|c| ('\u{3400}'..='\u{9fff}').contains(&c))
    { return Err("not_2_to_8_hanzi"); }
    boundaries.push(text.len());
    let mut paths = vec![BTreeSet::new(); boundaries.len()];
    paths[0].insert(String::new());
    for start in 0..boundaries.len() - 1 {
        let prefixes: Vec<_> = paths[start].iter().cloned().collect();
        for end in start + 1..boundaries.len() {
            for reading in index.all_readings_for_text(&text[boundaries[start]..boundaries[end]]) {
                let Some(reading) = zhixu::learning::normalize_reading(reading) else { continue };
                if reading.split_whitespace().count() != end - start { continue; }
                for prefix in &prefixes {
                    paths[end].insert(if prefix.is_empty() { reading.clone() } else { format!("{prefix} {reading}") });
                    // Two distinct prefixes at the same text boundary already prove ambiguity;
                    // appending the same suffix cannot make them equal again.
                    while paths[end].len() > 2 { paths[end].pop_last(); }
                }
            }
        }
    }
    let readings = paths.last().unwrap();
    match readings.len() {
        0 => Err("missing_reading"),
        1 => Ok(readings.first().unwrap().clone()),
        _ => Err("ambiguous_reading"),
    }
}

const RIME_PROBE: &str = r#"
#include "aim_rime_shim.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

int main(int argc, char **argv) {
  if (argc != 3 || !aim_rime_initialize(argv[1], argv[2])) return 2;
  uintptr_t session = aim_rime_session_create();
  if (!session) return 3;
  char *line = NULL;
  size_t capacity = 0;
  while (getline(&line, &capacity, stdin) >= 0) {
    line[strcspn(line, "\r\n")] = 0;
    aim_rime_clear(session);
    for (const unsigned char *key = (const unsigned char *)line; *key; ++key) {
      AimRimeString commit = {0};
      int consumed = aim_rime_process_key(session, *key, 0, &commit);
      int committed = commit.len != 0;
      aim_rime_string_free(&commit);
      if (consumed <= 0 || committed) return 4;
    }
    AimRimeSnapshot snapshot = {0};
    if (!aim_rime_snapshot(session, 1, &snapshot) || !snapshot.candidate_count) return 5;
    printf("%s\t%.*s\n", line, (int)snapshot.candidates[0].text.len,
           snapshot.candidates[0].text.data);
    aim_rime_snapshot_free(&snapshot);
  }
  free(line);
  aim_rime_session_destroy(session);
  return ferror(stdin) ? 6 : 0;
}
"#;

fn fresh_directory() -> PathBuf {
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let path = std::env::temp_dir().join(format!("zhixu-z4-{}-{stamp}", std::process::id()));
    fs::create_dir(&path).unwrap();
    path
}

fn probe_dictionary(shared: &Path, user: &Path) -> Arc<Index> {
    // Direct librime sees every table imported by agent_pinyin, without the
    // Mixer's optional-layer filter. Match that set in this isolated dictionary user.
    for layer in ["agent_pack_poetry", "agent_pack_cad", "agent_pack_industry"] {
        crate::lexicon_layers::set_layer(user, layer, true).unwrap();
    }
    fn imports(shared: &Path, name: &str, tables: &mut BTreeSet<String>) {
        if !tables.insert(name.to_string()) { return; }
        let text = fs::read_to_string(shared.join(format!("{name}.dict.yaml"))).unwrap();
        let mut in_imports = false;
        for line in text.lines().take_while(|line| line.trim() != "...") {
            let line = line.trim();
            if let Some(value) = line.strip_prefix("import_tables:") {
                in_imports = value.trim().is_empty();
            } else if in_imports && line.starts_with("- ") {
                let name = line[2..].split(" #").next().unwrap().trim().trim_matches(['\'', '"']);
                imports(shared, name, tables);
            } else if !line.is_empty() && !line.starts_with('#') {
                in_imports = false;
            }
        }
    }
    let mut tables = BTreeSet::new();
    imports(shared, "agent_pinyin", &mut tables);
    let mut data = zhixu::loader::load(shared, user).unwrap();
    // loader also reads ASCII layers for the product; they are outside Rime's
    // aggregate and must not supply additional Chinese readings to this comparison.
    data.entries.retain(|entry| tables.contains(&entry.layer));
    println!("Z4_DICTIONARY\tentries={}\ttables={}", data.entries.len(), tables.into_iter().collect::<Vec<_>>().join(","));
    Arc::new(Index::from_data(data))
}

fn rime_firsts(shared: &Path, temporary: &Path, readings: &[String]) -> Vec<String> {
    let rime = PathBuf::from(std::env::var_os("RIME_ROOT").expect("RIME_ROOT (existing librime include/ and lib/)"));
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let shim = repository.join("native/macos/Sources/RimeShim");
    let source = temporary.join("probe.c");
    let executable = temporary.join("rime-probe");
    fs::write(&source, RIME_PROBE).unwrap();
    let compile = Command::new("cc").arg("-std=gnu11")
        .arg("-I").arg(shim.join("include"))
        .arg("-I").arg(rime.join("include"))
        .arg(&source).arg(shim.join("aim_rime_shim.c"))
        .arg("-L").arg(rime.join("lib")).arg("-lrime").arg("-lpthread")
        .arg("-o").arg(&executable).output().expect("compile existing Rime shim");
    assert!(compile.status.success(), "C probe compilation failed: {}", String::from_utf8_lossy(&compile.stderr));
    let user = temporary.join("rime-user");
    fs::create_dir(&user).unwrap();
    let keys = temporary.join("keys.txt");
    fs::write(&keys, readings.join("\n") + "\n").unwrap();
    let output = Command::new(&executable).arg(shared).arg(&user)
        .env("DYLD_LIBRARY_PATH", rime.join("lib"))
        .env("LD_LIBRARY_PATH", rime.join("lib"))
        .stdin(Stdio::from(fs::File::open(keys).unwrap()))
        .output().expect("run isolated librime probe");
    assert!(output.status.success(), "Rime probe failed ({:?}): {}", output.status.code(), String::from_utf8_lossy(&output.stderr));
    let lines = String::from_utf8(output.stdout).expect("Rime UTF-8 output");
    let rows: Vec<_> = lines.lines().collect();
    assert_eq!(rows.len(), readings.len(), "Rime probe must not silently drop inputs");
    rows.into_iter().zip(readings).map(|(row, expected)| {
        let (reading, first) = row.split_once('\t').expect("Rime TSV output");
        assert_eq!(reading, expected);
        assert!(!first.is_empty());
        first.to_string()
    }).collect()
}

/// `AIM_Z4_SENTENCES=<one Hanzi sentence per line> AIM_ZHIXU_SHARED=<bundled Rime>
/// RIME_ROOT=<existing librime> cargo test --offline z4_sentence_comparison -- --ignored --nocapture`.
#[test]
#[ignore]
fn z4_sentence_comparison() {
    let input = std::env::var_os("AIM_Z4_SENTENCES").expect("AIM_Z4_SENTENCES");
    let shared = PathBuf::from(std::env::var_os("AIM_ZHIXU_SHARED").expect("AIM_ZHIXU_SHARED"));
    let temporary = fresh_directory();
    println!("Z4_TEMP\t{}", temporary.display());
    let dictionary_user = temporary.join("dictionary-user");
    fs::create_dir(&dictionary_user).unwrap();
    let index = probe_dictionary(&shared, &dictionary_user);
    let _dictionary = zhixu::pin_snapshot(Some(index.clone()));
    let _learning = zhixu::pin_learning(None);
    let source = fs::read_to_string(input).unwrap();
    let mut cases = Vec::new();
    for (line, text) in source.lines().enumerate() {
        let text = text.trim();
        if text.is_empty() { continue; }
        match unique_reading(&index, text) {
            Ok(reading) => cases.push((line + 1, text, reading.replace(' ', ""))),
            Err(reason) => println!("Z4_SKIP\t{}\t{text}\t{reason}", line + 1),
        }
    }
    assert!(cases.len() >= 2, "need at least two unambiguous sentences");
    let split = cases.len() / 2;
    let readings: Vec<_> = cases.iter().map(|(_, _, reading)| reading.clone()).collect();
    let rime = rime_firsts(&shared, &temporary, &readings);
    let mut counts = [(0usize, 0usize, 0usize); 2];
    println!("Z4_COLUMNS\tbucket\tline\ttarget\treading\trime\twhole\tlattice\twhole_word\tmatch\tsame_syllable_count");
    for (at, ((line, target, reading), native)) in cases.iter().zip(&rime).enumerate() {
        let bucket = usize::from(at >= split);
        let label = ["tune", "holdout"][bucket];
        let whole = super::whole_reading_candidates(reading, &[], 0, false);
        let first = whole.first().expect("whole decoder returned no candidates");
        let lattice = super::decode_lattice_with(reading, super::lattice_params().limit, false,
            Some((super::lattice_params().per_reading.max(6), &[], 0)));
        let lattice = &lattice.first().expect("whole lattice returned no candidates").0;
        let matched = first.candidate.text == *native;
        let whole_word = first.candidate.comment == super::ZHIXU_WHOLE_COMMENT;
        counts[bucket].0 += 1;
        counts[bucket].1 += usize::from(matched);
        counts[bucket].2 += usize::from(whole_word);
        // Compact pinyin can admit another syllabification (xi an / xian), so
        // this is diagnostic only, not a candidate-span coverage assertion.
        let same_syllable_count = native.chars().count() == target.chars().count();
        println!("Z4_CASE\t{label}\t{line}\t{target}\t{reading}\t{native}\t{}\t{lattice}\t{whole_word}\t{matched}\t{same_syllable_count}", first.candidate.text);
        if !matched {
            println!("Z4_DIFF\t{label}\t{line}\t{target}\t{reading}\t{native}\t{}", first.candidate.text);
        }
    }
    for (label, (total, matches, words)) in ["tune", "holdout"].into_iter().zip(counts) {
        println!("Z4_SUMMARY\t{label}\ttotal={total}\tmatches={matches}\tagreement={:.4}%\twhole_words={words}",
            matches as f64 * 100.0 / total as f64);
    }
}

#[test]
fn whole_sentence_probe_annotation_rejects_ambiguous_segmentations() {
    use crate::zhixu::loader::{DictionaryData, Entry};
    let index = Index::from_data(DictionaryData { entries: [
        ("一", "yi"), ("行", "xing"), ("一行", "yi hang"),
        ("天", "tian"), ("气", "qi"), ("天气", "tian qi"),
    ].into_iter().map(|(text, reading)| Entry {
        text: text.into(), reading: reading.into(), weight: 100, layer: "test".into(),
    }).collect() });
    assert_eq!(unique_reading(&index, "天气"), Ok("tian qi".into()));
    assert_eq!(unique_reading(&index, "一行"), Err("ambiguous_reading"));
}

#[test]
fn whole_sentence_probe_annotation_rejects_missing_and_out_of_scope_text() {
    let index = Index::from_data(crate::zhixu::loader::DictionaryData::default());
    assert_eq!(unique_reading(&index, "天气"), Err("missing_reading"));
    assert_eq!(unique_reading(&index, "天气!"), Err("not_2_to_8_hanzi"));
    assert_eq!(unique_reading(&index, "天"), Err("not_2_to_8_hanzi"));
}

#[test]
fn whole_sentence_probe_dictionary_matches_only_aggregate_imports() {
    let temporary = fresh_directory();
    let shared = temporary.join("shared");
    let user = temporary.join("dictionary-user");
    fs::create_dir(&shared).unwrap();
    fs::create_dir(&user).unwrap();
    fs::write(shared.join("agent_pinyin.dict.yaml"),
        "---\nname: agent_pinyin\nsort: by_weight\nimport_tables:\n  - agent_pack_poetry\n...\n天气\ttian qi\t100\n").unwrap();
    fs::write(shared.join("agent_pack_poetry.dict.yaml"),
        "---\nname: agent_pack_poetry\nsort: by_weight\n...\n天际\ttian ji\t100\n").unwrap();
    fs::write(shared.join("agent_tech_terms.dict.yaml"),
        "---\nname: agent_tech_terms\nsort: by_weight\n...\n终端\tzhong duan\t100\n").unwrap();
    let index = probe_dictionary(&shared, &user);
    assert_eq!(unique_reading(&index, "天际"), Ok("tian ji".into()));
    assert_eq!(unique_reading(&index, "终端"), Err("missing_reading"));
    fs::remove_dir_all(temporary).unwrap();
}
