//! Candidate-pool coverage (no model): for each recorded composition, is the string the
//! owner committed generated at all, and where?
//!
//!   pool <shared> <user dir> <events.jsonl> <out.jsonl>      (events: owner log, local only)
//!   pool <shared> <user dir> --probe <context> <keys…>        (prints the pool for each keys)
//!
//! Per case it records `p1` (first page as the core ranks it, no model), `rank_all` (index in
//! every page flattened, -1 = never generated), `in_comp` (in the whole-reading composition
//! list), `pages`. A string absent from both is a generation miss (G); the model cannot fix it.
use std::io::Write;
use zhizi_host::{Host, HostConfig, Key};

fn pool(host: &mut Host, context: &str, raw: &str) -> (Vec<String>, Vec<String>, Vec<String>) {
    host.clear_context();
    host.note_committed(context);
    let mut view = Default::default();
    for c in raw.chars() {
        view = host.process(Key::Text(c.to_string()), 0, false);
    }
    let view: zhizi_host::View = view;
    let first: Vec<String> = view.candidates.iter().map(|c| c.text.clone()).collect();
    let mut all = first.clone();
    for page in host.peek_following_pages(60) {
        all.extend(page.text.split('\t').filter(|t| !t.is_empty()).map(str::to_string));
    }
    let comps = host.composition_alternatives();
    host.process(Key::Escape, 0, false);
    (first, all, comps)
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let mut host = Host::open(&HostConfig { shared_dir: a[0].clone().into(), user_dir: a[1].clone().into(), app_id: "zhizi.pool".into() })
        .expect("open");
    if a[2] == "--probe" {
        for keys in &a[4..] {
            let (first, all, comps) = pool(&mut host, &a[3], keys);
            println!("== {keys}\n page1: {}\n all({}): {}\n comps({}): {}", first.join(" "), all.len(), all.join(" "), comps.len(), comps.join(" "));
        }
        return;
    }
    let mut out = std::fs::File::create(&a[3]).unwrap();
    let mut seen = std::collections::HashSet::new();
    for line in std::fs::read_to_string(&a[2]).unwrap().lines() {
        let Ok(e) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        let raw = e["pinyin"].as_str().unwrap_or_default();
        let chosen = e["selectedText"].as_str().unwrap_or_default();
        let context = e["precedingText"].as_str().unwrap_or_default();
        if raw.len() < std::env::var("ZHIZI_MIN_KEYS").ok().and_then(|v| v.parse().ok()).unwrap_or(4) || chosen.is_empty() || !raw.bytes().all(|b| b.is_ascii_lowercase()) {
            continue;
        }
        if !seen.insert((raw.to_string(), context.to_string(), chosen.to_string())) {
            continue;
        }
        let (first, all, comps) = pool(&mut host, context, raw);
        let rank_all = all.iter().position(|t| t == chosen).map(|i| i as i64).unwrap_or(-1);
        let rank_p1 = first.iter().position(|t| t == chosen).map(|i| i as i64).unwrap_or(-1);
        let in_comp = comps.iter().any(|t| t == chosen);
        let record = serde_json::json!({
            "raw": raw, "context": context, "chosen": chosen,
            "rank_p1": rank_p1, "rank_all": rank_all, "in_comp": in_comp,
            "n_all": all.len(), "n_comp": comps.len(), "top": first.first(),
            "candidates": all.iter().take(60).collect::<Vec<_>>(),
            "model_ready": false, "model_ms": 0,
        });
        writeln!(out, "{record}").unwrap();
    }
}
