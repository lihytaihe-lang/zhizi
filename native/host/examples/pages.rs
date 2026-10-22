//! Local diagnosis: how many candidate pages an input has, as the grid panel would page through them.
//!   pages <shared Rime dir> <user dir> <keys> [pages]
use zhizi_host::{Host, HostConfig, Key};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let mut host = Host::open(&HostConfig { shared_dir: a[0].clone().into(), user_dir: a[1].clone().into(), app_id: "zhizi.pages".into() })
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
    if let Ok(context) = std::env::var("PAGES_CONTEXT") {
        host.note_committed(&context);
    }
    let mut view = Default::default();
    for c in a[2].chars() {
        view = host.process(Key::Text(c.to_string()), 0, false);
    }
    for _ in 0..40 {
        if host.model_observation().outcome != "pending" {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
        if let Some(late) = host.poll_ranking() {
            view = late;
        }
    }
    let view: zhizi_host::View = view;
    println!("page 1: {}", view.candidates.iter().map(|c| c.text.clone()).collect::<Vec<_>>().join(" "));
    let more = host.peek_following_pages(a.get(3).and_then(|n| n.parse().ok()).unwrap_or(12));
    for (i, page) in more.iter().enumerate() {
        println!("page {}: {}", i + 2, page.text.replace('\t', " "));
    }
    println!("total pages: {}", more.len() + 1);
    if let Ok(number) = std::env::var("PICK_ON_PAGE2") {
        let second = host.process(Key::PageDown, 0, false);
        println!("page 2 again: {}", second.candidates.iter().map(|c| c.text.clone()).collect::<Vec<_>>().join(" "));
        let done = host.process(Key::Text(number), 0, false);
        println!("picked slot on page 2 -> commit={:?} preedit={:?} next: {}", done.commit, done.preedit, done.candidates.iter().map(|c| c.text.clone()).collect::<Vec<_>>().join(" "));
    }
}
