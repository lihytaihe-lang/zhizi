//! Local diagnosis: type, pick a candidate by its number mid-composition, keep typing, show every page.
//!   partial <shared Rime dir> <user dir> <first keys> <pick text> <more keys>
use zhizi_host::{Host, HostConfig, Key};

fn show(label: &str, view: &zhizi_host::View) {
    println!("{label}: preedit={:?} commit={:?}", view.preedit, view.commit);
    println!("   candidates: {}", view.candidates.iter().map(|c| format!("{}[{}]", c.text, c.comment)).collect::<Vec<_>>().join(" | "));
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let mut host = Host::open(&HostConfig { shared_dir: a[0].clone().into(), user_dir: a[1].clone().into(), app_id: "zhizi.partial".into() })
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
    let mut view: zhizi_host::View = Default::default();
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
    show("typed", &view);
    println!("   core alternatives: {:?}", host.composition_alternatives().iter().take(8).collect::<Vec<_>>());
    let Some(index) = view.candidates.iter().position(|c| c.text == a[3]) else {
        println!("{:?} is not on the page", a[3]);
        return;
    };
    view = host.process(Key::Text((index + 1).to_string()), 0, false);
    for _ in 0..40 {
        if host.model_observation().outcome != "pending" {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
        if let Some(late) = host.poll_ranking() {
            view = late;
        }
    }
    show("after pick", &view);
    println!("   core alternatives: {:?}", host.composition_alternatives().iter().take(8).collect::<Vec<_>>());
    if std::env::var("PARTIAL_CONTEXT").is_ok() {
        host.note_committed(&a[3]);
        view = host.process(Key::Text("x".into()), 0, false);
        view = host.process(Key::Backspace, 0, false);
        for _ in 0..40 {
            if host.model_observation().outcome != "pending" { break; }
            std::thread::sleep(std::time::Duration::from_millis(5));
            if let Some(late) = host.poll_ranking() { view = late; }
        }
    }
    for c in a[4].chars() {
        view = host.process(Key::Text(c.to_string()), 0, false);
    }
    show("then typed more", &view);
    if let Some(second) = a.get(5) {
        match view.candidates.iter().position(|c| &c.text == second) {
            Some(index) => {
                let done = host.process(Key::Text((index + 1).to_string()), 0, false);
                println!("second pick {second:?} -> commit={:?} preedit={:?}", done.commit, done.preedit);
            }
            None => println!("second pick {second:?} is not on the page"),
        }
        return;
    }
    for page in host.peek_following_pages(2) {
        println!("   next page: {}", page.text.replace('\t', " | "));
    }
}
