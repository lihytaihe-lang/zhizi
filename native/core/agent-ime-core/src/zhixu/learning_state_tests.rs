use crate::zhixu::{self, Index, learning::LearningMode, loader::{DictionaryData, Entry}};
use std::sync::atomic::AtomicBool;

#[derive(Default)]
struct LearningSegmentDecoder {
    raw: String,
    prefix: String,
    picks: Arc<Mutex<Vec<(usize, String)>>>,
    expose_whole: Arc<AtomicBool>,
}

impl LearningSegmentDecoder {
    fn page(&self) -> Vec<Candidate> {
        if self.raw.is_empty() { return Vec::new(); }
        let mut words = if self.prefix.is_empty() {
            vec![("再", "zai"), ("在", "zai")]
        } else {
            vec![("高", "gao"), ("搞", "gao")]
        };
        if self.prefix.is_empty() && self.raw == "zaigao" && self.expose_whole.load(Ordering::Relaxed) {
            words.push(("再搞", "zai gao"));
        }
        words.into_iter().map(|(text, reading)| Candidate {
            text: text.into(), reading_key: reading.into(), ..Candidate::default()
        }).collect()
    }
}

impl Decoder for LearningSegmentDecoder {
    fn snapshot(&self) -> DecoderSnapshot {
        let preedit = if self.prefix.is_empty() { self.raw.clone() } else { format!("{}gao", self.prefix) };
        DecoderSnapshot { raw_input: self.raw.clone(), cursor_utf8: preedit.len(), preedit,
            candidates: self.page(), is_last_page: true, ..DecoderSnapshot::default() }
    }
    fn input(&mut self, text: &str) -> Result<String, i32> { self.raw.push_str(text); Ok(String::new()) }
    fn backspace(&mut self) -> Result<String, i32> { self.raw.pop(); Ok(String::new()) }
    fn move_left(&mut self) -> Result<String, i32> { Ok(String::new()) }
    fn move_right(&mut self) -> Result<String, i32> { Ok(String::new()) }
    fn page_up(&mut self) -> Result<String, i32> { Ok(String::new()) }
    fn page_down(&mut self) -> Result<String, i32> { Ok(String::new()) }
    fn select_previous(&mut self) -> Result<String, i32> { Ok(String::new()) }
    fn select_next(&mut self) -> Result<String, i32> { Ok(String::new()) }
    fn select_candidate(&mut self, index: usize, _: u64, _: LearningIntent) -> Result<String, i32> {
        let text = self.page().get(index).ok_or(-10)?.text.clone();
        self.picks.lock().unwrap().push((index, text.clone()));
        if self.raw == "zaigao" && self.prefix.is_empty() && text.chars().count() == 1 {
            self.prefix = text;
            return Ok(String::new());
        }
        let text = format!("{}{text}", self.prefix);
        self.clear();
        Ok(text)
    }
    fn delete_candidate(&mut self, _: usize) -> Result<(), i32> { Err(-10) }
    fn commit_raw(&mut self) -> String { let text = self.raw.clone(); self.clear(); text }
    fn clear(&mut self) { self.raw.clear(); self.prefix.clear(); }
}

fn learning_state_index() -> Arc<Index> {
    Arc::new(Index::from_data(DictionaryData {
        entries: [("再", "zai"), ("在", "zai"), ("高", "gao"), ("搞", "gao")]
            .into_iter().map(|(text, reading)| Entry {
                text: text.into(), reading: reading.into(), weight: 800, layer: "fixture".into(),
            }).collect(),
    }))
}

fn learning_state_session() -> (Session, Arc<Mutex<Vec<(usize, String)>>>, Arc<AtomicBool>) {
    let decoder = LearningSegmentDecoder::default();
    let picks = decoder.picks.clone();
    let expose_whole = decoder.expose_whole.clone();
    let mut session = Session::new(Box::new(decoder), format!("learning-state-{}", NEXT_APP.fetch_add(1, Ordering::Relaxed)));
    session.configure_learning(LearningMode::Zhixu).unwrap();
    (session, picks, expose_whole)
}

fn learning_state_pick(session: &mut Session, digit: &str, id: u64) -> ResultModel {
    let mut pick = event(KeyKind::Text, digit);
    pick.delivery_id = id;
    session.process(pick)
}

fn learning_state_commit_parts(session: &mut Session) {
    let before = type_text(session, "zaigao");
    assert!(!before.candidates.iter().any(|candidate| candidate.text == "再搞"));
    let prefix = learning_state_pick(session, "1", 7101);
    assert_eq!(prefix.marked_text, "再gao");
    assert!(prefix.commit_text.is_empty());
    assert_eq!(prefix.candidates.iter().map(|candidate| candidate.text.as_str()).collect::<Vec<_>>(), vec!["高", "搞"]);
    let whole = learning_state_pick(session, "2", 7102);
    assert_eq!(whole.commit_text, "再搞");
    assert!(whole.marked_text.is_empty());
}

fn learning_state_mature(session: &mut Session) {
    session.decoder.learning_key(&event(KeyKind::Unsupported, ""), crate::personal_picks::now_ms() + 5_001);
}

#[test]
fn learning_state_retained_segments_appear_as_one_word_next_time() {
    zhixu::with_index(learning_state_index(), || {
        let (mut session, picks, _) = learning_state_session();
        learning_state_commit_parts(&mut session);
        assert_eq!(*picks.lock().unwrap(), vec![(0, "再".into()), (1, "搞".into())]);
        learning_state_mature(&mut session);
        let next = type_text(&mut session, "zaigao");
        assert_eq!(next.candidates[0].text, "再搞");
        assert_eq!(learning_state_pick(&mut session, "1", 7103).commit_text, "再搞");
        assert_eq!(picks.lock().unwrap().len(), 2, "new local word commits directly");
    });
}

#[test]
fn learning_state_deleted_segments_are_not_added() {
    zhixu::with_index(learning_state_index(), || {
        let (mut session, _, _) = learning_state_session();
        learning_state_commit_parts(&mut session);
        assert!(!session.process(event(KeyKind::Backspace, "")).consumed);
        learning_state_mature(&mut session);
        let next = type_text(&mut session, "zaigao");
        assert!(!next.candidates.iter().any(|candidate| candidate.text == "再搞"));
        assert_eq!(next.candidates[0].text, "再");
    });
}

#[test]
fn learning_state_reordered_word_keeps_the_native_selection_index() {
    zhixu::with_index(learning_state_index(), || {
        let (mut session, picks, expose_whole) = learning_state_session();
        learning_state_commit_parts(&mut session);
        learning_state_mature(&mut session);
        expose_whole.store(true, Ordering::Relaxed);
        let next = type_text(&mut session, "zaigao");
        assert_eq!(next.candidates[0].text, "再搞");
        assert_eq!(learning_state_pick(&mut session, "1", 7104).commit_text, "再搞");
        assert_eq!(picks.lock().unwrap().last(), Some(&(2, "再搞".into())));
    });
}

#[test]
fn learning_state_rime_switch_keeps_default_key_commit_and_candidate_results() {
    zhixu::with_index(learning_state_index(), || {
        let mut baseline = Session::new(Box::<LearningSegmentDecoder>::default(), format!("learning-default-{}", NEXT_APP.fetch_add(1, Ordering::Relaxed)));
        let mut explicit = Session::new(Box::<LearningSegmentDecoder>::default(), format!("learning-rime-{}", NEXT_APP.fetch_add(1, Ordering::Relaxed)));
        explicit.configure_learning(LearningMode::Rime).unwrap();
        let keys = "zaigao12".chars().map(|c| (KeyKind::Text, c.to_string()))
            .chain([(KeyKind::Backspace, String::new())])
            .chain("zaigao".chars().map(|c| (KeyKind::Text, c.to_string())))
            .chain([(KeyKind::Escape, String::new())]);
        for (id, (kind, text)) in keys.enumerate() {
            let mut key = event(kind, &text);
            key.delivery_id = 7200 + id as u64;
            let a = baseline.process(key.clone());
            let b = explicit.process(key);
            assert_eq!(a.status, b.status);
            assert_eq!(a.consumed, b.consumed);
            assert_eq!(a.mode, b.mode);
            assert_eq!(a.secure_degraded, b.secure_degraded);
            assert_eq!(a.marked_text, b.marked_text);
            assert_eq!(a.marked_cursor_utf8, b.marked_cursor_utf8);
            assert_eq!(a.commit_text, b.commit_text);
            assert_eq!(a.candidates, b.candidates);
            assert_eq!(a.secondary_candidates, b.secondary_candidates);
            assert_eq!(a.highlighted_index, b.highlighted_index);
            assert_eq!(a.page_no, b.page_no);
            assert_eq!(a.is_last_page, b.is_last_page);
            assert_eq!(a.selection_event_id, b.selection_event_id);
        }
    });
}
