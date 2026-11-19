use agent_ime_core::sdk::{Engine, KeyEvent, KeyKind, Security};
use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 4
        || args[3].is_empty()
        || args[3].len() > 256
        || !args[3].bytes().all(|c| c.is_ascii_lowercase())
    {
        return Err("usage: zhixu <absolute-dictionary-dir> <absolute-private-profile-dir> <lowercase-pinyin>".into());
    }
    // This console example deliberately declares its own ordinary input as
    // non-secure. Real hosts must provide their actual security state.
    let mut engine = Engine::open(
        Path::new(&args[1]),
        Path::new(&args[2]),
        "org.zhixu.sdk.rust-demo",
    )?;
    println!(
        "Zhixu {} (ABI {})",
        env!("CARGO_PKG_VERSION"),
        agent_ime_core::ABI_VERSION
    );
    let mut id = 0;
    for c in args[3].chars() {
        id += 1;
        let text = c.to_string();
        let out = engine.process(KeyEvent {
            kind: KeyKind::Text,
            text: &text,
            modifiers: 0,
            security: Security::NonSecure,
            delivery_id: id,
        })?;
        if out.status != 0 {
            return Err(format!("input rejected: {}", out.status).into());
        }
    }
    let page = engine.snapshot()?;
    println!("marked: {}", page.marked_text);
    for (i, candidate) in page.candidates.iter().enumerate() {
        println!(
            "{}. {} [{}] {}",
            i + 1,
            candidate.text,
            candidate.reading_key,
            candidate.comment
        );
    }
    let committed = engine.process(KeyEvent {
        kind: KeyKind::Space,
        text: "",
        modifiers: 0,
        security: Security::NonSecure,
        delivery_id: id + 1,
    })?;
    if committed.status != 0 || committed.commit_text.is_empty() {
        return Err("no complete commit".into());
    }
    println!("committed: {}", committed.commit_text);
    Ok(())
}
