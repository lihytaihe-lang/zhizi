use crate::custom_config::{CustomConfigStore, fuzzy_config};
use crate::exact_readings::{ExactReadingIndex, canonical_dictionary_reading, load_exact_readings};
use crate::secure_directory::physical_runtime_path;
use crate::selection_ledger::{SelectionClaim, SelectionDeliveryLedger};
use crate::state::{Candidate, Decoder, DecoderSnapshot, LearningIntent};
use std::ffi::{CStr, CString, c_char, c_int};
use std::path::PathBuf;
use std::ptr;
use std::slice;
use std::collections::HashSet;
use std::sync::{Arc, Mutex, OnceLock};

#[repr(C)]
struct AimRimeString {
    data: *mut c_char,
    len: usize,
}

#[repr(C)]
struct AimRimeCandidate {
    text: AimRimeString,
    comment: AimRimeString,
}

#[repr(C)]
struct AimRimeSnapshot {
    raw_input: AimRimeString,
    preedit: AimRimeString,
    cursor_utf8: usize,
    candidates: *mut AimRimeCandidate,
    candidate_count: usize,
    highlighted_index: usize,
    page_no: usize,
    is_last_page: c_int,
}

unsafe extern "C" {
    fn aim_rime_initialize(shared_dir: *const c_char, user_dir: *const c_char) -> c_int;
    fn aim_rime_session_create() -> usize;
    fn aim_rime_session_destroy(session: usize);
    fn aim_rime_process_key(
        session: usize,
        keycode: c_int,
        mask: c_int,
        commit: *mut AimRimeString,
    ) -> c_int;
    fn aim_rime_before_select_candidate(session: usize) -> c_int;
    fn aim_rime_candidate_requires_regular_selection(
        session: usize,
        index: usize,
        global_index: c_int,
    ) -> c_int;
    fn aim_rime_record_selection_probe(
        session: usize,
        reinforce: c_int,
        requires_regular_selection: c_int,
    );
    fn aim_rime_select_candidate(session: usize, index: usize, commit: *mut AimRimeString)
    -> c_int;
    fn aim_rime_select_candidate_global(
        session: usize,
        index: usize,
        commit: *mut AimRimeString,
    ) -> c_int;
    fn aim_rime_commit_candidate_without_learning(
        session: usize,
        index: usize,
        commit: *mut AimRimeString,
    ) -> c_int;
    fn aim_rime_commit_candidate_without_learning_global(
        session: usize,
        index: usize,
        commit: *mut AimRimeString,
    ) -> c_int;
    fn aim_rime_commit_preview_without_learning(
        session: usize,
        index: usize,
        commit: *mut AimRimeString,
    ) -> c_int;
    fn aim_rime_commit_preview_without_learning_global(
        session: usize,
        index: usize,
        commit: *mut AimRimeString,
    ) -> c_int;
    fn aim_rime_delete_candidate(session: usize, index: usize) -> c_int;
    fn aim_rime_delete_candidate_global(session: usize, index: usize) -> c_int;
    fn aim_rime_clear(session: usize);
    fn aim_rime_snapshot(
        session: usize,
        maximum_candidates: usize,
        snapshot: *mut AimRimeSnapshot,
    ) -> c_int;
    fn aim_rime_string_free(string: *mut AimRimeString);
    fn aim_rime_snapshot_free(snapshot: *mut AimRimeSnapshot);
    fn aim_rime_redeploy() -> c_int;
}

const XK_BACKSPACE: i32 = 0xff08;
const XK_LEFT: i32 = 0xff51;
const XK_UP: i32 = 0xff52;
const XK_RIGHT: i32 = 0xff53;
const XK_DOWN: i32 = 0xff54;
const XK_PAGE_UP: i32 = 0xff55;
const XK_PAGE_DOWN: i32 = 0xff56;

pub struct RimeDecoder {
    session: usize,
    user_dir: PathBuf,
    selection_ledger: SelectionDeliveryLedger,
    fuzzy_mask: Option<u32>,
    exact_readings: Arc<ExactReadingIndex>,
}

const FUZZY_CANDIDATE_POOL: usize = 256;

/// A single character the owner picks over the first is taught to Rime from the second
/// pick on (same letters, same character), not the first: Rime's user dictionary orders
/// by recency, so one pick of 时 put it ahead of 是 (194 picks) for every later shi, and
/// one 捞 ahead of 了. Chronological replay of the owner's October cases (2,972, learning
/// as typed): +9 -6 against learning every pick, and learning itself +41 -21 against
/// none (it, 接手, 候选字 stay learned). Words and phrases are taught on the first pick.
/// ZHIZI_SINGLE_LEARN overrides it for local evaluation only: 0 = every pick, 1 = never.
fn single_learn_policy() -> u8 {
    static POLICY: OnceLock<u8> = OnceLock::new();
    *POLICY.get_or_init(|| std::env::var("ZHIZI_SINGLE_LEARN").ok().and_then(|v| v.parse().ok()).unwrap_or(2))
}

/// First deliberate picks of a single character, by (typed letters, character), in this
/// process: the pick is not taught to Rime until it is repeated.
fn pending_single_picks() -> &'static Mutex<HashSet<(String, char)>> {
    static PENDING: OnceLock<Mutex<HashSet<(String, char)>>> = OnceLock::new();
    PENDING.get_or_init(|| Mutex::new(HashSet::new()))
}

impl RimeDecoder {
    /// A deliberate pick of one Chinese character that covers the whole input.
    fn single_character_intent(&self, index: usize) -> LearningIntent {
        let snapshot = self.snapshot();
        let Some(text) = snapshot.candidates.get(index).map(|candidate| candidate.text.as_str()) else {
            return LearningIntent::DeliberateSelection;
        };
        let mut characters = text.chars();
        let (Some(character), None) = (characters.next(), characters.next()) else {
            return LearningIntent::DeliberateSelection;
        };
        if !('\u{4e00}'..='\u{9fff}').contains(&character) {
            return LearningIntent::DeliberateSelection;
        }
        if single_learn_policy() == 1 {
            return LearningIntent::NoLearn;
        }
        let Ok(mut pending) = pending_single_picks().lock() else {
            return LearningIntent::DeliberateSelection;
        };
        if pending.remove(&(snapshot.raw_input.clone(), character)) {
            return LearningIntent::DeliberateSelection;
        }
        if pending.len() >= 1024 {
            pending.clear();
        }
        pending.insert((snapshot.raw_input, character));
        LearningIntent::NoLearn
    }

    pub fn new(shared_dir: &str, user_dir: &str) -> Result<Self, i32> {
        let user_path = physical_runtime_path(&PathBuf::from(user_dir));
        let shared_path = PathBuf::from(shared_dir);
        let exact_readings = load_exact_readings(&shared_path);
        let shared_dir = CString::new(shared_dir).map_err(|_| -100)?;
        let user_dir = CString::new(user_path.to_str().ok_or(-100)?).map_err(|_| -100)?;
        if unsafe { aim_rime_initialize(shared_dir.as_ptr(), user_dir.as_ptr()) } == 0 {
            return Err(-101);
        }
        let session = unsafe { aim_rime_session_create() };
        if session == 0 {
            return Err(-102);
        }
        Ok(Self {
            session,
            selection_ledger: SelectionDeliveryLedger::new(&user_path),
            user_dir: user_path,
            fuzzy_mask: None,
            exact_readings,
        })
    }

    fn owned_string(mut value: AimRimeString) -> String {
        let string = if value.data.is_null() || value.len == 0 {
            String::new()
        } else {
            let bytes = unsafe { slice::from_raw_parts(value.data.cast::<u8>(), value.len) };
            String::from_utf8_lossy(bytes).into_owned()
        };
        unsafe { aim_rime_string_free(&mut value) };
        string
    }

    fn key(&mut self, keycode: i32) -> Result<String, i32> {
        let mut commit = AimRimeString {
            data: ptr::null_mut(),
            len: 0,
        };
        let status = unsafe { aim_rime_process_key(self.session, keycode, 0, &mut commit) };
        let commit = Self::owned_string(commit);
        if status < 0 { Err(status) } else { Ok(commit) }
    }

    fn convert_string(value: &AimRimeString) -> String {
        if value.data.is_null() || value.len == 0 {
            return String::new();
        }
        let bytes = unsafe { slice::from_raw_parts(value.data.cast::<u8>(), value.len) };
        String::from_utf8_lossy(bytes).into_owned()
    }

    fn candidate_reading(&self, text: &str, raw_input: &str) -> String {
        let raw = canonical_dictionary_reading(raw_input).unwrap_or_default();
        // With fuzzy pinyin on, the decoder's text may spell another reading of the input
        // (zongguo -> 中国); the character check would then call it partial, so keep the
        // input as before.
        let fuzzy = self.fuzzy_mask.is_some_and(|mask| mask != 0);
        self.exact_readings
            .lookup(text, &raw)
            .or_else(|| (!fuzzy).then(|| self.exact_readings.composed_reading(text, &raw)).flatten())
            .unwrap_or(raw)
    }

    fn snapshot_with_maximum(&self, maximum_candidates: usize) -> DecoderSnapshot {
        let mut snapshot = AimRimeSnapshot {
            raw_input: AimRimeString {
                data: ptr::null_mut(),
                len: 0,
            },
            preedit: AimRimeString {
                data: ptr::null_mut(),
                len: 0,
            },
            cursor_utf8: 0,
            candidates: ptr::null_mut(),
            candidate_count: 0,
            highlighted_index: 0,
            page_no: 0,
            is_last_page: 1,
        };
        if unsafe { aim_rime_snapshot(self.session, maximum_candidates, &mut snapshot) } == 0 {
            return DecoderSnapshot::default();
        }
        let raw_input = Self::convert_string(&snapshot.raw_input);
        let preedit = Self::convert_string(&snapshot.preedit);
        let candidate_slice = if snapshot.candidates.is_null() {
            &[][..]
        } else {
            unsafe { slice::from_raw_parts(snapshot.candidates, snapshot.candidate_count) }
        };
        let candidates = candidate_slice
            .iter()
            .enumerate()
            .map(|(index, candidate)| {
                let text = Self::convert_string(&candidate.text);
                Candidate {
                    reading_key: self.candidate_reading(&text, &raw_input),
                    text,
                    comment: Self::convert_string(&candidate.comment),
                    source_index: (maximum_candidates != 0).then_some(index),
                }
            })
            .collect();
        let result = DecoderSnapshot {
            raw_input,
            preedit,
            cursor_utf8: snapshot.cursor_utf8,
            candidates,
            secondary_candidates: Vec::new(),
            highlighted_index: snapshot.highlighted_index,
            page_no: snapshot.page_no,
            is_last_page: snapshot.is_last_page != 0,
        };
        unsafe { aim_rime_snapshot_free(&mut snapshot) };
        result
    }
}

impl Drop for RimeDecoder {
    fn drop(&mut self) {
        unsafe { aim_rime_session_destroy(self.session) };
    }
}

impl Decoder for RimeDecoder {
    fn snapshot(&self) -> DecoderSnapshot {
        let maximum_candidates = if self.fuzzy_mask.unwrap_or(0) == 0 {
            0
        } else {
            FUZZY_CANDIDATE_POOL
        };
        self.snapshot_with_maximum(maximum_candidates)
    }

    fn snapshot_all_candidates(&self) -> Option<DecoderSnapshot> {
        let snapshot = self.snapshot_with_maximum(usize::MAX);
        snapshot.is_last_page.then_some(snapshot)
    }

    fn input(&mut self, text: &str) -> Result<String, i32> {
        let mut commit = String::new();
        for byte in text.bytes() {
            if !byte.is_ascii() {
                return Err(-103);
            }
            commit.push_str(&self.key(byte as i32)?);
        }
        Ok(commit)
    }

    fn backspace(&mut self) -> Result<String, i32> {
        self.key(XK_BACKSPACE)
    }
    fn move_left(&mut self) -> Result<String, i32> {
        self.key(XK_LEFT)
    }
    fn move_right(&mut self) -> Result<String, i32> {
        self.key(XK_RIGHT)
    }
    fn page_up(&mut self) -> Result<String, i32> {
        self.key(XK_PAGE_UP)
    }
    fn page_down(&mut self) -> Result<String, i32> {
        self.key(XK_PAGE_DOWN)
    }
    fn select_previous(&mut self) -> Result<String, i32> {
        self.key(XK_UP)
    }
    fn select_next(&mut self) -> Result<String, i32> {
        self.key(XK_DOWN)
    }

    fn select_candidate(
        &mut self,
        index: usize,
        delivery_id: u64,
        learning_intent: LearningIntent,
    ) -> Result<String, i32> {
        let mut commit = AimRimeString {
            data: ptr::null_mut(),
            len: 0,
        };
        let global_index = self.fuzzy_mask.unwrap_or(0) != 0;
        let requires_regular_selection = unsafe {
            aim_rime_candidate_requires_regular_selection(
                self.session,
                index,
                c_int::from(global_index),
            ) != 0
        };
        let effective_intent = match (learning_intent, requires_regular_selection) {
            // Rime's own first candidate that covers only part of the input: Space takes its whole
            // preview. A first candidate that is not Rime's (a pick moved up, the model's order)
            // and covers only part of the input is picked like any other: the rest stays to be
            // typed (owner 2026-10-12: shou kou, 收 first, Space gave 收扣).
            (LearningIntent::NoLearn, true) if index != 0 || global_index => LearningIntent::DeliberateSelection,
            (LearningIntent::NoLearn, true) => LearningIntent::RequiredSegmentCommit,
            (LearningIntent::DeliberateSelection, false) if single_learn_policy() != 0 => {
                self.single_character_intent(index)
            }
            (intent, _) => intent,
        };
        unsafe {
            aim_rime_record_selection_probe(
                self.session,
                c_int::from(learning_intent == LearningIntent::DeliberateSelection),
                c_int::from(requires_regular_selection),
            );
        }
        let commit_candidate_without_learning = |commit: &mut AimRimeString| {
            if global_index {
                unsafe {
                    aim_rime_commit_candidate_without_learning_global(self.session, index, commit)
                }
            } else {
                unsafe { aim_rime_commit_candidate_without_learning(self.session, index, commit) }
            }
        };
        let status = match effective_intent {
            LearningIntent::NoLearn => commit_candidate_without_learning(&mut commit),
            LearningIntent::RequiredSegmentCommit => {
                if global_index {
                    unsafe {
                        aim_rime_commit_preview_without_learning_global(
                            self.session,
                            index,
                            &mut commit,
                        )
                    }
                } else {
                    unsafe {
                        aim_rime_commit_preview_without_learning(self.session, index, &mut commit)
                    }
                }
            }
            LearningIntent::DeliberateSelection => match self.selection_ledger.claim(delivery_id) {
                SelectionClaim::Fresh => {
                    let ready = unsafe { aim_rime_before_select_candidate(self.session) };
                    if ready == 0 {
                        commit_candidate_without_learning(&mut commit)
                    } else if global_index {
                        unsafe {
                            aim_rime_select_candidate_global(self.session, index, &mut commit)
                        }
                    } else {
                        unsafe { aim_rime_select_candidate(self.session, index, &mut commit) }
                    }
                }
                SelectionClaim::AlreadyClaimed | SelectionClaim::Unavailable => {
                    commit_candidate_without_learning(&mut commit)
                }
            },
        };
        let commit = Self::owned_string(commit);
        if status == 0 { Err(-104) } else { Ok(commit) }
    }

    fn select_candidate_global(
        &mut self,
        index: usize,
        delivery_id: u64,
        learning_intent: LearningIntent,
    ) -> Result<String, i32> {
        let previous = self.fuzzy_mask;
        self.fuzzy_mask = Some(1);
        let result = self.select_candidate(index, delivery_id, learning_intent);
        self.fuzzy_mask = previous;
        result
    }

    fn delete_candidate(&mut self, index: usize) -> Result<(), i32> {
        let deleted = if self.fuzzy_mask.unwrap_or(0) != 0 {
            unsafe { aim_rime_delete_candidate_global(self.session, index) }
        } else {
            unsafe { aim_rime_delete_candidate(self.session, index) }
        };
        if deleted == 0 { Err(-105) } else { Ok(()) }
    }

    fn delete_candidate_global(&mut self, index: usize) -> Result<(), i32> {
        let deleted = unsafe { aim_rime_delete_candidate_global(self.session, index) };
        if deleted == 0 { Err(-105) } else { Ok(()) }
    }

    fn commit_raw(&mut self) -> String {
        let raw = self.snapshot().raw_input;
        unsafe { aim_rime_clear(self.session) };
        raw
    }

    fn clear(&mut self) {
        unsafe { aim_rime_clear(self.session) };
    }

    fn configure_fuzzy(&mut self, mask: u32) -> Result<(), i32> {
        if self.fuzzy_mask == Some(mask) {
            return Ok(());
        }
        let store = CustomConfigStore::open_or_create(&self.user_dir).map_err(|_| -106)?;
        let previous = store.read_managed().map_err(|_| -107)?;
        let yaml = fuzzy_config(mask);
        let next = (mask != 0).then_some(yaml.as_bytes());
        let desired_is_current = previous.as_deref() == next;
        if desired_is_current {
            // The deployed schema is process-global, while engines belong to
            // individual sessions. Recreate even when this decoder has not yet
            // observed a mask: it may have been created just before another
            // controller completed the global redeploy.
            unsafe { aim_rime_session_destroy(self.session) };
            self.session = unsafe { aim_rime_session_create() };
            if self.session == 0 {
                return Err(-109);
            }
            self.fuzzy_mask = Some(mask);
            return Ok(());
        }
        match next {
            Some(contents) => store
                .replace(contents, previous.as_deref())
                .map_err(|_| -107)?,
            None => store
                .remove(previous.as_deref().ok_or(-107)?)
                .map_err(|_| -107)?,
        }
        unsafe { aim_rime_session_destroy(self.session) };
        self.session = 0;
        let deployed = unsafe { aim_rime_redeploy() } != 0;
        if deployed {
            self.session = unsafe { aim_rime_session_create() };
        }
        if deployed && self.session != 0 {
            self.fuzzy_mask = Some(mask);
            return Ok(());
        }

        let rollback = match previous.as_deref() {
            Some(contents) => store.replace(contents, next),
            None => match next {
                Some(contents) => store.remove(contents),
                None => Ok(()),
            },
        };
        if rollback.is_err() {
            self.session = unsafe { aim_rime_session_create() };
            return if self.session == 0 {
                Err(-109)
            } else {
                Err(-107)
            };
        }
        let rollback_deployed = unsafe { aim_rime_redeploy() } != 0;
        if rollback_deployed {
            self.session = unsafe { aim_rime_session_create() };
        }
        if self.session == 0 {
            Err(-109)
        } else {
            Err(-108)
        }
    }
}

#[allow(dead_code)]
fn _verify_c_string(value: &CStr) -> &CStr {
    value
}
