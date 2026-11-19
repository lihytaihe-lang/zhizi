//! Private JSON-lines bridge for IBus. The main thread alone owns the core;
//! workers may announce completion but never write stdout or touch a live session.
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    io::{self, BufRead, Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, SyncSender},
    },
    thread,
};
use zhizi_host::{Host, HostConfig, Key, View, settings};

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum Request {
    Key {
        id: u64,
        epoch: u64,
        key: String,
        #[serde(default)]
        text: String,
        #[serde(default)]
        modifiers: u32,
    },
    Focus {
        id: u64,
        epoch: u64,
        app: String,
    },
    Reset {
        id: u64,
        epoch: u64,
    },
    Security {
        id: u64,
        epoch: u64,
        secure: bool,
    },
    Context {
        id: u64,
        epoch: u64,
        text: String,
    },
    Reload {
        id: u64,
        epoch: u64,
    },
    Poll {
        id: u64,
        epoch: u64,
    },
    Quit {
        id: u64,
        epoch: u64,
    },
}
impl Request {
    fn identity(&self) -> (u64, u64) {
        match self {
            Self::Key { id, epoch, .. }
            | Self::Focus { id, epoch, .. }
            | Self::Reset { id, epoch }
            | Self::Security { id, epoch, .. }
            | Self::Context { id, epoch, .. }
            | Self::Reload { id, epoch }
            | Self::Poll { id, epoch }
            | Self::Quit { id, epoch } => (*id, *epoch),
        }
    }
}
struct Models {
    ranking: Option<zhizi_host::ranking::onnx::OnnxScorer>,
    generation: Option<zhizi_host::zhiju::onnx::OnnxDistribution>,
}
enum Event {
    Command(Request),
    Poll,
    Loaded(Models),
    End,
    Invalid,
}
fn write_json(value: Value) -> io::Result<()> {
    let stdout = io::stdout();
    let mut out = stdout.lock();
    serde_json::to_writer(&mut out, &value)?;
    out.write_all(b"\n")?;
    out.flush()
}
fn view_json(v: &View) -> Value {
    json!({"consumed":v.consumed,"preedit":v.preedit,"cursor_utf8":v.cursor_utf8,"commit":v.commit,
    "candidates":v.candidates.iter().map(|c|json!({"text":c.text,"comment":c.comment})).collect::<Vec<_>>(),
    "highlighted":v.highlighted,"page":v.page,"is_last_page":v.is_last_page,"english_mode":v.english_mode})
}
fn key(name: &str, text: &str) -> Option<Key> {
    Some(match name {
        "Text" if text.chars().count() == 1 && !text.chars().next()?.is_control() => {
            Key::Text(text.into())
        }
        "Backspace" => Key::Backspace,
        "Left" => Key::Left,
        "Right" => Key::Right,
        "Up" => Key::Up,
        "Down" => Key::Down,
        "Space" => Key::Space,
        "Enter" => Key::Enter,
        "Escape" => Key::Escape,
        "PageUp" => Key::PageUp,
        "PageDown" => Key::PageDown,
        "ShiftTap" => Key::ShiftTap,
        "CapsLock" => Key::CapsLock,
        "DeleteCandidate" => Key::DeleteCandidate,
        "BlacklistCandidate" => Key::BlacklistCandidate,
        _ => return None,
    })
}
fn verified(root: &Path, group: &str) -> bool {
    let manifest: Value =
        match serde_json::from_str(include_str!("../../../resources/WindowsModels.json")) {
            Ok(v) => v,
            Err(_) => return false,
        };
    let Some(files) = manifest[group]["files"].as_object() else {
        return false;
    };
    let directory = root.join(if group == "ranking" {
        "Model"
    } else {
        "ZhijuModel"
    });
    files.iter().all(|(name, expected)| {
        let Ok(mut file) = std::fs::File::open(directory.join(name)) else {
            return false;
        };
        let mut hash = Sha256::new();
        let mut buffer = [0u8; 65536];
        loop {
            match file.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => hash.update(&buffer[..n]),
                Err(_) => return false,
            }
        }
        expected.as_str() == Some(format!("{:x}", hash.finalize()).as_str())
    })
}
fn notifier(tx: SyncSender<Event>, pending: Arc<AtomicBool>) -> Box<dyn Fn() + Send> {
    Box::new(move || {
        if !pending.swap(true, Ordering::AcqRel) && tx.try_send(Event::Poll).is_err() {
            pending.store(false, Ordering::Release);
        }
    })
}
fn apply_settings(host: &mut Host, data: &Path, app: &str) {
    let s = settings::load(data);
    host.set_typo_correction(s.typo_correction);
    host.set_context_ranking_enabled(s.context_ranking);
    host.set_zhiju_enabled(s.zhiju);
    host.set_english_mode(settings::english_mode(data, app));
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 3 {
        return Err("usage: zhizi-ibus-host resources user-data onnxruntime-library".into());
    }
    let resources = PathBuf::from(&args[0]);
    let data = PathBuf::from(&args[1]);
    let runtime = PathBuf::from(&args[2]);
    std::fs::create_dir_all(&data)?;
    let mut app = "ibus.unknown".to_string();
    let config = |name: &str| HostConfig {
        shared_dir: resources.join("Rime"),
        user_dir: data.join("Rime"),
        app_id: name.into(),
    };
    let mut host = Host::open(&config(&app))?;
    apply_settings(&mut host, &data, &app);
    let (tx, rx) = mpsc::sync_channel(256);
    let poll_pending = Arc::new(AtomicBool::new(false));
    let reader = tx.clone();
    thread::spawn(move || {
        let stdin = io::stdin();
        let mut input = stdin.lock();
        loop {
            let mut line = String::new();
            let count = match input.by_ref().take(65537).read_line(&mut line) {
                Ok(n) => n,
                Err(_) => break,
            };
            if count == 0 {
                break;
            }
            if count > 65536 || !line.ends_with('\n') {
                let _ = reader.send(Event::Invalid);
                break;
            }
            match serde_json::from_str(&line) {
                Ok(request) => {
                    if reader.send(Event::Command(request)).is_err() {
                        return;
                    }
                }
                Err(_) => {
                    let _ = reader.send(Event::Invalid);
                    break;
                }
            }
        }
        let _ = reader.send(Event::End);
    });
    let loader = tx.clone();
    let model_root = resources.clone();
    thread::spawn(move || {
        let threads = (thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(2)
            / 2)
        .clamp(1, 4);
        let mut models = Models {
            ranking: None,
            generation: None,
        };
        if zhizi_host::ranking::onnx::init_runtime(&runtime).is_ok() {
            if verified(&model_root, "ranking") {
                models.ranking = zhizi_host::ranking::onnx::OnnxScorer::load(
                    &model_root.join("Model/zhizi30m_v2.onnx"),
                    &model_root.join("Model/itos.json"),
                    threads,
                )
                .ok();
            }
            if verified(&model_root, "generation") {
                models.generation = zhizi_host::zhiju::onnx::OnnxDistribution::load(
                    &model_root.join("ZhijuModel/zhiju30m_v3.onnx"),
                    threads,
                )
                .ok();
            }
        }
        let _ = loader.send(Event::Loaded(models));
    });
    let mut epoch = 0;
    let mut secure = true;
    let mut focus_valid = true;
    let mut english_mode = settings::english_mode(&data, &app);
    let mut ranking_available = false;
    let mut generation_available = false;
    write_json(
        json!({"event":"ready","version":include_str!("../../../../VERSION").trim(),"abi":6}),
    )?;
    while let Ok(event) = rx.recv() {
        match event {
            Event::End => {
                // Closing the private pipe is a normal shell shutdown. Confirm
                // ordinary committed observations before destroying the session.
                host.process(Key::FocusLost, 0, secure);
                host.clear_context();
                break;
            }
            Event::Invalid => {
                write_json(json!({"event":"error","error":"invalid_protocol"}))?;
                break;
            }
            Event::Loaded(models) => {
                ranking_available = models.ranking.is_some();
                if let Some(model) = models.ranking {
                    host.enable_model(
                        Box::new(model),
                        Some(notifier(tx.clone(), poll_pending.clone())),
                    );
                }
                if let Some(model) = models.generation {
                    generation_available = host
                        .enable_zhiju(
                            &resources.join("ZhijuModel/itos.json"),
                            Box::new(model),
                            Some(notifier(tx.clone(), poll_pending.clone())),
                        )
                        .is_ok();
                }
                write_json(
                    json!({"event":"models","ranking":ranking_available,"generation":generation_available}),
                )?;
            }
            Event::Poll => {
                poll_pending.store(false, Ordering::Release);
                // A notification does not change the page. Only the IBus UI
                // event loop may request a poll, then render its reply before
                // handling another selection key. This preserves slot binding.
                write_json(json!({"event":"poll"}))?;
            }
            Event::Command(request) => {
                let (id, next_epoch) = request.identity();
                if next_epoch < epoch {
                    write_json(json!({"id":id,"epoch":next_epoch,"error":"stale_request"}))?;
                    continue;
                }
                epoch = next_epoch;
                let mut view = None;
                let mut error = None;
                let mut quit = false;
                match request {
                    Request::Key {
                        key: name,
                        text,
                        modifiers,
                        ..
                    } => {
                        if !secure && focus_valid {
                            if let Some(key) = key(&name, &text) {
                                let v = host.process(key, modifiers & 31, false);
                                if v.english_mode != english_mode {
                                    english_mode = v.english_mode;
                                    if settings::save_english_mode(&data, &app, v.english_mode)
                                        .is_err()
                                    {
                                        error = Some("settings_write_failed");
                                    }
                                }
                                view = Some(v);
                            } else {
                                error = Some("unknown_key");
                            }
                        } else {
                            view = Some(View::default());
                        }
                    }
                    Request::Focus { app: new_app, .. } => {
                        host.process(Key::FocusLost, 0, secure);
                        host.clear_context();
                        focus_valid = true;
                        if new_app.is_empty()
                            || new_app.len() > 256
                            || new_app.chars().any(char::is_control)
                        {
                            error = Some("invalid_app");
                            focus_valid = false;
                        } else if new_app != app {
                            if host.change_application(&config(&new_app)).is_ok() {
                                app = new_app;
                            } else {
                                error = Some("application_session_failed");
                                focus_valid = false;
                            }
                        }
                        apply_settings(&mut host, &data, &app);
                        english_mode = settings::english_mode(&data, &app);
                        view = Some(View::default());
                    }
                    Request::Security { secure: next, .. } => {
                        if !secure && next {
                            // These observations belong to the ordinary field
                            // that just lost focus, before entering private input.
                            host.process(Key::FocusLost, 0, false);
                        }
                        secure = next;
                        if secure {
                            view = Some(host.process(Key::FocusLost, 0, true));
                            host.clear_context();
                        }
                    }
                    Request::Context { text, .. } => {
                        if !secure {
                            let bounded: String = text
                                .chars()
                                .rev()
                                .take(32)
                                .collect::<Vec<_>>()
                                .into_iter()
                                .rev()
                                .collect();
                            host.set_context(&bounded);
                        }
                    }
                    Request::Reset { .. } => {
                        view = Some(host.process(Key::FocusLost, 0, secure));
                        host.clear_context();
                    }
                    Request::Reload { .. } => {
                        host.process(Key::FocusLost, 0, secure);
                        apply_settings(&mut host, &data, &app);
                        english_mode = settings::english_mode(&data, &app);
                        view = Some(View::default());
                    }
                    Request::Poll { .. } => {
                        if !secure && focus_valid {
                            view = host.poll_ranking();
                        }
                    }
                    Request::Quit { .. } => {
                        host.process(Key::FocusLost, 0, secure);
                        host.clear_context();
                        quit = true;
                    }
                }
                let mut reply = json!({"id":id,"epoch":epoch,"ranking_available":ranking_available,"generation_available":generation_available});
                if let Some(v) = view {
                    let mut packet = view_json(&v);
                    if !v.preedit.is_empty() && !v.is_last_page {
                        packet["following_candidates"] = host
                            .peek_following_pages(1)
                            .iter()
                            .map(|c| json!({"text":c.text,"comment":c.comment}))
                            .collect();
                    }
                    reply["view"] = packet
                }
                if let Some(e) = error {
                    reply["error"] = e.into()
                }
                write_json(reply)?;
                if quit {
                    break;
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn protocol_rejects_multi_scalar_text_and_unknown_keys() {
        assert!(key("Text", "你好").is_none());
        assert!(key("Text", "你").is_some());
        assert!(key("Text", "\n").is_none());
        assert!(key("Delete", "x").is_none());
    }
    #[test]
    fn protocol_identity_and_page_are_owned_by_core() {
        let r: Request =
            serde_json::from_str(r#"{"op":"key","id":7,"epoch":9,"key":"Text","text":"n"}"#)
                .unwrap();
        assert_eq!(r.identity(), (7, 9));
        let v = View {
            cursor_utf8: 3,
            preedit: "你".into(),
            ..Default::default()
        };
        assert_eq!(view_json(&v)["cursor_utf8"], 3);
    }
}
