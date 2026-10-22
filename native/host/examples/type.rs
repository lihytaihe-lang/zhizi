//! Smoke test: `cargo run --example type -- <rime shared dir> <user dir> <keys...>`.
//! Each argument is a key: letters/digits/punctuation as text, or space/enter/
//! backspace/escape/up/down/left/right/pageup/pagedown.
use zhizi_host::{Host, HostConfig, Key};

fn main() {
    let mut arguments = std::env::args().skip(1);
    let shared = arguments.next().expect("shared dir");
    let user = arguments.next().expect("user dir");
    let mut host = Host::open(&HostConfig {
        shared_dir: shared.into(),
        user_dir: user.into(),
        app_id: "zhizi.example".into(),
    })
    .expect("open");
    // Optional quality records: ZHIZI_RECORDS=<events.jsonl>
    if let Ok(path) = std::env::var("ZHIZI_RECORDS") {
        host.enable_recording(zhizi_host::quality::RecorderConfig {
            path: path.into(),
            platform: std::env::consts::OS.into(),
            build: "example".into(),
            device: "example".into(),
        });
    }
    // Optional 顺手·知字: ZHIZI_ORT=<onnxruntime library> ZHIZI_MODEL=<model.onnx> ZHIZI_ITOS=<itos.json>
    #[cfg(feature = "model")]
    if let (Ok(library), Ok(model), Ok(itos)) =
        (std::env::var("ZHIZI_ORT"), std::env::var("ZHIZI_MODEL"), std::env::var("ZHIZI_ITOS"))
    {
        zhizi_host::ranking::onnx::init_runtime(library.as_ref()).expect("onnxruntime");
        let scorer = zhizi_host::ranking::onnx::OnnxScorer::load(model.as_ref(), itos.as_ref(), 4).expect("model");
        host.enable_model(Box::new(scorer), None);
    }
    for word in arguments {
        let keys: Vec<Key> = match word.as_str() {
            "space" => vec![Key::Space],
            "enter" => vec![Key::Enter],
            "backspace" => vec![Key::Backspace],
            "escape" => vec![Key::Escape],
            "up" => vec![Key::Up],
            "down" => vec![Key::Down],
            "left" => vec![Key::Left],
            "right" => vec![Key::Right],
            "pageup" => vec![Key::PageUp],
            "pagedown" => vec![Key::PageDown],
            text => text.chars().map(|c| Key::Text(c.to_string())).collect(),
        };
        let mut view = Default::default();
        for key in keys {
            view = host.process(key, 0, false);
        }
        let view: zhizi_host::View = view;
        let mut view = view;
        // Wait for a late ranking the way a shell would on its next poll.
        for _ in 0..50 {
            if host.model_observation().outcome != "pending" {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
            if let Some(late) = host.poll_ranking() {
                view = late;
            }
        }
        let row: Vec<String> = view
            .candidates
            .iter()
            .map(|c| if c.comment.is_empty() { c.text.clone() } else { format!("{}({})", c.text, c.comment) })
            .collect();
        if std::env::var("ZHIZI_ALTS").is_ok() && !view.preedit.is_empty() {
            println!("            alternatives: {:?}", host.composition_alternatives());
        }
        let o = host.model_observation();
        println!(
            "{word:>10} | preedit={:?} commit={:?} | {} | model={} group={} ms={:?} late={}",
            view.preedit, view.commit, row.join(" "), o.outcome, o.group_size, o.latency_ms, o.late
        );
    }
}
