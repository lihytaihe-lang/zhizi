//! Counts runs of 2..=6 Han characters in open text, the data of an octagram grammar
//! (librime-octagram stores the natural log of each string's count; the decoder asks it
//! how often the end of the preceding text runs on into a candidate word).
//!
//!   zhizi-gram-count --min-count N [--shards 16] FILE... > counts.txt
//!
//! Prints `string count` lines, sorted by string, for every string seen at least N times.
//! Strings are counted shard by shard (by hash) so memory stays bounded; each line of
//! input is cut at every non-Han character, so no string spans punctuation or Latin text.
use std::collections::HashMap;
use std::io::{BufRead, BufReader, BufWriter, Write};

const MAX_LEN: usize = 6;

fn han(c: char) -> bool {
    ('\u{4e00}'..='\u{9fff}').contains(&c) || ('\u{3400}'..='\u{4dbf}').contains(&c)
}

/// Up to six code points packed into one u128 (21 bits each, length in the top bits).
fn pack(chars: &[char]) -> u128 {
    let mut key = chars.len() as u128;
    for &c in chars {
        key = (key << 21) | c as u128;
    }
    key
}

fn unpack(mut key: u128) -> String {
    let mut chars = Vec::new();
    while key >= 1 << 21 {
        chars.push(char::from_u32((key & 0x1f_ffff) as u32).unwrap_or('?'));
        key >>= 21;
    }
    chars.into_iter().rev().collect()
}

fn shard_of(key: u128, shards: u64) -> u64 {
    // splitmix-style mix so neighbouring keys spread evenly
    let mut x = (key as u64) ^ ((key >> 64) as u64);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d049bb133111eb);
    (x ^ (x >> 31)) % shards
}

fn main() {
    let mut arguments = std::env::args().skip(1);
    let mut min_count = 2u64;
    let mut shards = 16u64;
    let mut files = Vec::new();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--min-count" => min_count = arguments.next().and_then(|v| v.parse().ok()).expect("--min-count N"),
            "--shards" => shards = arguments.next().and_then(|v| v.parse().ok()).expect("--shards N"),
            _ => files.push(argument),
        }
    }
    assert!(!files.is_empty(), "usage: zhizi-gram-count --min-count N [--shards S] FILE...");
    let mut kept: Vec<(String, u64)> = Vec::new();
    for shard in 0..shards {
        let mut counts: HashMap<u128, u64> = HashMap::new();
        for file in &files {
            let reader = BufReader::with_capacity(1 << 20, std::fs::File::open(file).expect("open"));
            let mut run: Vec<char> = Vec::new();
            for line in reader.lines() {
                let Ok(line) = line else { continue };
                for c in line.chars().chain(std::iter::once('\n')) {
                    if han(c) {
                        run.push(c);
                        continue;
                    }
                    for start in 0..run.len() {
                        for len in 2..=MAX_LEN.min(run.len() - start) {
                            let key = pack(&run[start..start + len]);
                            if shard_of(key, shards) == shard {
                                *counts.entry(key).or_default() += 1;
                            }
                        }
                    }
                    run.clear();
                }
            }
        }
        let before = counts.len();
        kept.extend(counts.into_iter().filter(|&(_, n)| n >= min_count).map(|(k, n)| (unpack(k), n)));
        eprintln!("shard {shard}: {before} strings, kept so far {}", kept.len());
    }
    kept.sort_unstable();
    let mut out = BufWriter::new(std::io::stdout().lock());
    for (string, count) in kept {
        writeln!(out, "{string} {count}").unwrap();
    }
}
