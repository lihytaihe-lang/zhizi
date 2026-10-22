//! Local diagnosis: type keys, press Space (or a digit), show what is committed and what stays.
//!   spacepick <shared Rime dir> <user dir> <keys> [key]      key: space (default) or a digit
use zhizi_host::{Host, HostConfig, Key};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let mut host = Host::open(&HostConfig { shared_dir: a[0].clone().into(), user_dir: a[1].clone().into(), app_id: "zhizi.space".into() })
        .expect("open");
    #[cfg(feature = "model")]
    if std::env::var("ZHIZI_MODEL").is_ok() {
        zhizi_host::ranking::onnx::init_runtime(std::env::var("ZHIZI_ORT").unwrap().as_ref()).unwrap();
        let scorer = zhizi_host::ranking::onnx::OnnxScorer::load(
            std::env::var("ZHIZI_MODEL").unwrap().as_ref(),
            std::env::var("ZHIZI_ITOS").unwrap().as_ref(),
            4,
        )
        .unwrap();
        host.enable_model(Box::new(scorer), None);
    }
    host.clear_context();
    if let Ok(context) = std::env::var("SPACE_CONTEXT") {
        host.note_committed(&context);
    }
    let mut view: zhizi_host::View = Default::default();
    for c in a[2].chars() {
        view = host.process(Key::Text(c.to_string()), 0, false);
    }
    for _ in 0..400 {
        if host.model_observation().outcome != "pending" {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
        if let Some(late) = host.poll_ranking() {
            view = late;
        }
    }
    // PRE_PICKS="2,1": pick as the owner did (a candidate, then the next), then type the keys again.
    if let Ok(picks) = std::env::var("PRE_PICKS") {
        for pick in picks.split(',') {
            let done = host.process(Key::Text(pick.to_string()), 0, false);
            println!("pre-pick {pick:?}: commit={:?} preedit={:?}", done.commit, done.preedit);
        }
        for c in a[2].chars() {
            view = host.process(Key::Text(c.to_string()), 0, false);
        }
        for _ in 0..400 {
            if host.model_observation().outcome != "pending" { break; }
            std::thread::sleep(std::time::Duration::from_millis(5));
            if let Some(late) = host.poll_ranking() { view = late; }
        }
    }
    println!("typed {:?}: preedit={:?}", a[2], view.preedit);
    println!("   page: {}", view.candidates.iter().map(|c| format!("{}[{}]", c.text, c.comment)).collect::<Vec<_>>().join(" | "));
    if a.get(3).map(String::as_str) == Some("down-space") {
        let moved = host.process(Key::Down, 0, false);
        println!("after Down: highlighted={} page: {}", moved.highlighted, moved.candidates.iter().map(|c| c.text.clone()).collect::<Vec<_>>().join(" | "));
    }
    let key = match a.get(3).map(String::as_str) {
        Some("comma") => Key::Text(",".to_string()),
        Some(digit) if digit.chars().all(|c| c.is_ascii_digit()) => Key::Text(digit.to_string()),
        _ => Key::Space,
    };
    let done = host.process(key, 0, false);
    println!("after the key: commit={:?} preedit={:?}", done.commit, done.preedit);
    println!("   page: {}", done.candidates.iter().map(|c| format!("{}[{}]", c.text, c.comment)).collect::<Vec<_>>().join(" | "));
}
