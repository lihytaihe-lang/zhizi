//! Lexicon coverage probe: `coverage <shared> <user> <words.tsv> <out.tsv>`; each input
//! line is `word<TAB>pinyin` (pinyin without spaces). Types the pinyin with no context
//! and writes the word's rank on the first page (0-based, or -1) and the first candidate.
use std::io::Write;
use zhizi_host::{Host, HostConfig, Key};

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let mut host = Host::open(&HostConfig {
        shared_dir: arguments[0].clone().into(),
        user_dir: arguments[1].clone().into(),
        app_id: "zhizi.coverage".into(),
    })
    .expect("open");
    let mut output = std::fs::File::create(&arguments[3]).unwrap();
    for line in std::fs::read_to_string(&arguments[2]).unwrap().lines() {
        let Some((word, pinyin)) = line.split_once('\t') else { continue };
        let mut view = zhizi_host::View::default();
        for c in pinyin.chars() {
            view = host.process(Key::Text(c.to_string()), 0, false);
        }
        let rank = view.candidates.iter().position(|c| c.text == word).map(|i| i as i64).unwrap_or(-1);
        let first = view.candidates.first().map(|c| c.text.as_str()).unwrap_or("");
        writeln!(output, "{word}\t{pinyin}\t{rank}\t{first}").unwrap();
        host.process(Key::Escape, 0, false);
    }
}
