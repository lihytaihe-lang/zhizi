#![deny(unsafe_op_in_unsafe_fn)]

pub mod blacklist;
mod context_model;
mod exact_context;
mod english_choices;
mod exact_offpage;
mod mixer;
mod lexicon_layers;
pub mod personal_picks;
pub mod ranking;
mod secure_directory;
mod state;
pub mod zhixu;
pub mod sdk;

use context_model::ContextModel;
use mixer::MixerLexicon;
use state::{Candidate, KeyEvent, KeyKind, ResultModel, SecurityState, Session};
use std::ffi::c_void;
use std::ptr;
use std::slice;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// Stable C struct/key ABI generation; increment for incompatible ABI changes.
pub const ABI_VERSION: u32 = 6;
/// Static NUL-terminated crate version. Borrow only; never free or mutate it.
#[unsafe(no_mangle)]
pub extern "C" fn aim_core_version() -> *const std::ffi::c_char {
    concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr().cast()
}

static LIVE_SESSIONS: AtomicUsize = AtomicUsize::new(0);

#[repr(C)]
pub struct AimKeyEvent {
    pub kind: u32,
    pub text: *const u8,
    pub text_len: usize,
    pub modifiers: u32,
    pub security_state: u32,
    pub delivery_id: u64,
}

#[repr(C)]
pub struct AimCandidate {
    pub text: *mut u8,
    pub text_len: usize,
    pub comment: *mut u8,
    pub comment_len: usize,
    pub reading_key: *mut u8,
    pub reading_key_len: usize,
}

#[repr(C)]
pub struct AimResult {
    pub status: i32,
    pub consumed: u8,
    pub mode: u8,
    pub secure_degraded: u8,
    pub reserved: u8,
    pub marked_text: *mut u8,
    pub marked_text_len: usize,
    pub marked_cursor_utf8: usize,
    pub commit_text: *mut u8,
    pub commit_text_len: usize,
    pub candidates: *mut AimCandidate,
    pub candidate_count: usize,
    pub secondary_candidates: *mut AimCandidate,
    pub secondary_candidate_count: usize,
    pub highlighted_index: usize,
    pub page_no: usize,
    pub is_last_page: u8,
    pub selection_event_id: u64,
    pub core_elapsed_ns: u64,
}

impl Default for AimResult {
    fn default() -> Self {
        Self {
            status: 0,
            consumed: 0,
            mode: 0,
            secure_degraded: 0,
            reserved: 0,
            marked_text: ptr::null_mut(),
            marked_text_len: 0,
            marked_cursor_utf8: 0,
            commit_text: ptr::null_mut(),
            commit_text_len: 0,
            candidates: ptr::null_mut(),
            candidate_count: 0,
            secondary_candidates: ptr::null_mut(),
            secondary_candidate_count: 0,
            highlighted_index: 0,
            page_no: 0,
            is_last_page: 1,
            selection_event_id: 0,
            core_elapsed_ns: 0,
        }
    }
}

pub struct AimSession {
    app_id: String,
    inner: Session,
}

pub struct AimLexiconSnapshot {
    inner: std::sync::Arc<MixerLexicon>,
}

pub struct AimContextModelSnapshot {
    inner: std::sync::Arc<ContextModel>,
}

pub struct AimZhijuSearch {
    inner: zhixu::zhiju::Search,
}

pub struct AimZhijuSnapshot {
    inner: state::ZhijuSnapshot,
}

fn copy_bytes(bytes: &[u8]) -> (*mut u8, usize) {
    if bytes.is_empty() {
        return (ptr::null_mut(), 0);
    }
    let mut owned = bytes.to_vec().into_boxed_slice();
    let pointer = owned.as_mut_ptr();
    let length = owned.len();
    std::mem::forget(owned);
    (pointer, length)
}

unsafe fn free_bytes(pointer: *mut u8, length: usize) {
    if !pointer.is_null() && length > 0 {
        let slice_pointer = ptr::slice_from_raw_parts_mut(pointer, length);
        unsafe { drop(Box::from_raw(slice_pointer)) };
    }
}

fn candidate_to_ffi(candidate: Candidate) -> AimCandidate {
    let (text, text_len) = copy_bytes(candidate.text.as_bytes());
    let (comment, comment_len) = copy_bytes(candidate.comment.as_bytes());
    let (reading_key, reading_key_len) = copy_bytes(candidate.reading_key.as_bytes());
    AimCandidate {
        text,
        text_len,
        comment,
        comment_len,
        reading_key,
        reading_key_len,
    }
}

fn fill_result(result: &mut AimResult, model: ResultModel, core_elapsed_ns: u64) {
    let (marked_text, marked_text_len) = copy_bytes(model.marked_text.as_bytes());
    let (commit_text, commit_text_len) = copy_bytes(model.commit_text.as_bytes());
    let candidates: Vec<AimCandidate> =
        model.candidates.into_iter().map(candidate_to_ffi).collect();
    let (candidate_pointer, candidate_count) = if candidates.is_empty() {
        (ptr::null_mut(), 0)
    } else {
        let mut owned = candidates.into_boxed_slice();
        let pointer = owned.as_mut_ptr();
        let count = owned.len();
        std::mem::forget(owned);
        (pointer, count)
    };
    let secondary_candidates: Vec<AimCandidate> = model
        .secondary_candidates
        .into_iter()
        .map(candidate_to_ffi)
        .collect();
    let (secondary_candidate_pointer, secondary_candidate_count) =
        if secondary_candidates.is_empty() {
            (ptr::null_mut(), 0)
        } else {
            let mut owned = secondary_candidates.into_boxed_slice();
            let pointer = owned.as_mut_ptr();
            let count = owned.len();
            std::mem::forget(owned);
            (pointer, count)
        };

    *result = AimResult {
        status: model.status,
        consumed: u8::from(model.consumed),
        mode: model.mode as u8,
        secure_degraded: u8::from(model.secure_degraded),
        reserved: 0,
        marked_text,
        marked_text_len,
        marked_cursor_utf8: model.marked_cursor_utf8,
        commit_text,
        commit_text_len,
        candidates: candidate_pointer,
        candidate_count,
        secondary_candidates: secondary_candidate_pointer,
        secondary_candidate_count,
        highlighted_index: model.highlighted_index,
        page_no: model.page_no,
        is_last_page: u8::from(model.is_last_page),
        selection_event_id: model.selection_event_id,
        core_elapsed_ns,
    };
}

unsafe fn utf8_argument(pointer: *const u8, length: usize) -> Result<String, ()> {
    if pointer.is_null() {
        return if length == 0 {
            Ok(String::new())
        } else {
            Err(())
        };
    }
    let bytes = unsafe { slice::from_raw_parts(pointer, length) };
    std::str::from_utf8(bytes)
        .map(str::to_owned)
        .map_err(|_| ())
}

#[unsafe(no_mangle)]
/// Creates one composition session over the process dictionary deployment.
///
/// # Safety
///
/// Serialize creation and keep the same shared/user deployment while any
/// sessions are alive, including sessions owned by the Rust SDK facade.
///
/// Each non-null pointer must reference a readable buffer of its corresponding
/// length for the duration of this call. A null pointer is permitted only when
/// its length is zero. Every buffer must contain valid UTF-8. A non-null return
/// value is owned by the caller and must be released exactly once with
/// [`aim_session_destroy`].
pub unsafe extern "C" fn aim_session_create(
    shared_dir: *const u8,
    shared_dir_len: usize,
    user_dir: *const u8,
    user_dir_len: usize,
    app_id: *const u8,
    app_id_len: usize,
) -> *mut AimSession {
    let Ok(shared_dir) = (unsafe { utf8_argument(shared_dir, shared_dir_len) }) else {
        return ptr::null_mut();
    };
    let Ok(user_dir) = (unsafe { utf8_argument(user_dir, user_dir_len) }) else {
        return ptr::null_mut();
    };
    let Ok(app_id) = (unsafe { utf8_argument(app_id, app_id_len) }) else {
        return ptr::null_mut();
    };
    // The native dictionaries load with the session; a failed load fails the
    // session, exactly as a failed Rime deployment used to.
    let shared_path = std::path::PathBuf::from(&shared_dir);
    let user_path = secure_directory::physical_runtime_path(&std::path::PathBuf::from(&user_dir));
    zhixu::loader::attach(&shared_path, &user_path);
    let Ok(dictionary) = zhixu::loader::deployed_snapshot() else {
        return ptr::null_mut();
    };
    let Some(dictionary) = dictionary else {
        return ptr::null_mut();
    };
    let Ok(blacklist) = blacklist::shared_blacklist(&user_path) else {
        return ptr::null_mut();
    };
    let session = Box::new(AimSession {
        app_id: app_id.clone(),
        inner: match Session::new_native(app_id, blacklist, dictionary) {
            Ok(session) => session,
            Err(_) => return ptr::null_mut(),
        },
    });
    LIVE_SESSIONS.fetch_add(1, Ordering::SeqCst);
    Box::into_raw(session)
}

#[unsafe(no_mangle)]
/// Destroys a session created by [`aim_session_create`].
///
/// # Safety
///
/// `session` must be null or a live pointer returned by
/// [`aim_session_create`] that has not already been destroyed. After this call,
/// the pointer must not be used again.
pub unsafe extern "C" fn aim_session_destroy(session: *mut AimSession) {
    if !session.is_null() {
        unsafe { drop(Box::from_raw(session)) };
        LIVE_SESSIONS.fetch_sub(1, Ordering::SeqCst);
    }
}

#[unsafe(no_mangle)]
/// Processes one key event and writes an owned result.
///
/// # Safety
///
/// `session` must reference a live session, `event` must reference a readable
/// [`AimKeyEvent`], and `result` must reference writable storage for one
/// [`AimResult`]. `event.text` follows the same pointer/length and UTF-8 rules
/// as [`aim_session_create`]. On success, the caller must eventually pass the
/// written result to [`aim_result_free`].
pub unsafe extern "C" fn aim_session_process(
    session: *mut AimSession,
    event: *const AimKeyEvent,
    result: *mut AimResult,
) -> i32 {
    if result.is_null() {
        return -1;
    }
    // `result` is output-only storage. Never inspect or free its prior bytes:
    // a C caller must be able to pass an uninitialized stack AimResult on the
    // first call. Successful outputs remain caller-owned via aim_result_free.
    unsafe { result.write(AimResult::default()) };
    if session.is_null() || event.is_null() {
        return -1;
    }
    let session = unsafe { &mut *session };
    let event = unsafe { &*event };
    let text = match unsafe { utf8_argument(event.text, event.text_len) } {
        Ok(text) => text,
        Err(_) => return -2,
    };
    let key_kind = KeyKind::from_u32(event.kind).unwrap_or(KeyKind::Unsupported);
    let security = SecurityState::from_u32(event.security_state);
    let started = Instant::now();
    let model = session.inner.process(KeyEvent {
        kind: key_kind,
        text,
        modifiers: event.modifiers,
        security,
        delivery_id: event.delivery_id,
    });
    let elapsed = started.elapsed().as_nanos().min(u64::MAX as u128) as u64;
    unsafe { fill_result(&mut *result, model, elapsed) };
    0
}

#[unsafe(no_mangle)]
/// Reads the complete current page without changing session state.
///
/// # Safety
///
/// `session` must reference a live session and `result` must reference writable,
/// output-only storage for one [`AimResult`], which may be uninitialized. On
/// success, the caller must release the written result with [`aim_result_free`].
pub unsafe extern "C" fn aim_session_current_page(
    session: *mut AimSession,
    result: *mut AimResult,
) -> i32 {
    if result.is_null() {
        return -1;
    }
    unsafe { result.write(AimResult::default()) };
    if session.is_null() {
        return -1;
    }
    let session = unsafe { &*session };
    let started = Instant::now();
    let model = session.inner.current_result();
    let elapsed = started.elapsed().as_nanos().min(u64::MAX as u128) as u64;
    unsafe { fill_result(&mut *result, model, elapsed) };
    0
}

#[unsafe(no_mangle)]
/// Withdraws the immediately preceding matching personal pick. Returns 1 if withdrawn, 0 otherwise.
///
/// # Safety
/// `session` must be live; raw/text must be valid UTF-8 pointer/length pairs.
pub unsafe extern "C" fn aim_session_withdraw_selection(
    session: *mut AimSession, raw: *const u8, raw_len: usize,
    text: *const u8, text_len: usize,
) -> i32 {
    if session.is_null() { return -1; }
    let (Ok(raw), Ok(text)) = (unsafe { utf8_argument(raw, raw_len) }, unsafe { utf8_argument(text, text_len) }) else {
        return -2;
    };
    let session = unsafe { &mut *session };
    let withdrawn = session.inner.withdraw_learning(&raw, &text);
    i32::from(mixer::withdraw_selection(&session.app_id, &raw, &text) || withdrawn)
}

#[unsafe(no_mangle)]
/// Selects the learning rule: 1 = Zhixu (the only rule; sessions always learn
/// in Zhixu). The former 0 = Rime rule no longer exists and returns -2 like
/// any other invalid mode. Call only with no composition; busy sessions are
/// rejected with -10.
/// # Safety
/// `session` must be null or a live, exclusively accessed session.
pub unsafe extern "C" fn aim_session_set_learning(session: *mut AimSession, mode: u32) -> i32 {
    let Some(session) = (unsafe { session.as_mut() }) else { return -1 };
    let mode = match mode {
        1 => zhixu::learning::LearningMode::Zhixu,
        _ => return -2,
    };
    session.inner.configure_learning(mode).err().unwrap_or(0)
}

#[unsafe(no_mangle)]
/// Retained decoder knob: 0/1 are accepted for shell compatibility, but the
/// session's inner decoder is native Zhixu for its whole life and this call no
/// longer changes it. Returns -1 for null, -2 for invalid values, and -10
/// during composition.
/// # Safety
/// `session` must be null or a live, exclusively accessed session.
pub unsafe extern "C" fn aim_session_set_zhixu_whole(session: *mut AimSession, on: u32) -> i32 {
    let Some(session) = (unsafe { session.as_mut() }) else { return -1 };
    let on = match on {
        0 => false,
        1 => true,
        _ => return -2,
    };
    session.inner.configure_zhixu_whole(on).err().unwrap_or(0)
}

#[unsafe(no_mangle)]
/// Retired migration entry point (Rime user dictionaries are gone): always
/// returns -50 (capability removed) and never reads or writes `user`.
/// # Safety
/// `user` must be a valid UTF-8 pointer/length pair.
pub unsafe extern "C" fn aim_learning_available(user: *const u8, user_len: usize) -> i32 {
    let _ = (user, user_len);
    -50
}

#[unsafe(no_mangle)]
/// Retired migration entry point (the one-time Rime userdb migration is
/// complete): always returns -50 (capability removed) and never touches the
/// filesystem. The symbol stays for the shells until they drop their calls.
/// # Safety
/// Paths must be valid UTF-8 pointer/length pairs.
pub unsafe extern "C" fn aim_learning_migrate(
    shared: *const u8, shared_len: usize, user: *const u8, user_len: usize,
) -> i32 {
    let _ = (shared, shared_len, user, user_len);
    -50
}

#[unsafe(no_mangle)]
/// Reconfigures the supported fuzzy-pinyin groups for a session.
///
/// # Safety
///
/// `session` must reference a live session returned by
/// [`aim_session_create`].
pub unsafe extern "C" fn aim_session_set_fuzzy(session: *mut AimSession, fuzzy_mask: u32) -> i32 {
    if session.is_null() {
        return -1;
    }
    match unsafe { &mut *session }.inner.configure_fuzzy(fuzzy_mask) {
        Ok(()) => 0,
        Err(status) => status,
    }
}

#[unsafe(no_mangle)]
/// Reloads the native dictionaries so tables added to the user directory since
/// the session started (downloaded professional packs) take effect. Drops any
/// composition; call it only at a safe point. Returns 0 on success.
///
/// # Safety
///
/// `session` must reference a live session returned by [`aim_session_create`].
pub unsafe extern "C" fn aim_session_reload_dictionaries(session: *mut AimSession) -> i32 {
    if session.is_null() {
        return -1;
    }
    match unsafe { &mut *session }.inner.reload_dictionaries() {
        Ok(()) => 0,
        Err(status) => status,
    }
}

#[unsafe(no_mangle)]
/// Sets the per-session context routing override and punctuation lock.
///
/// App class values are 0 (automatic), 1 (code), 2 (prose), 3 (browser), and
/// 4 (unknown). Punctuation values are 0 (automatic), 1 (fullwidth), and
/// 2 (ASCII). A change requested during composition is applied at the next
/// safe session boundary.
///
/// # Safety
///
/// `session` must reference a live session returned by [`aim_session_create`].
pub unsafe extern "C" fn aim_session_set_context_profile(
    session: *mut AimSession,
    app_class_override: u32,
    punctuation_policy: u32,
) -> i32 {
    if session.is_null() {
        return -1;
    }
    match unsafe { &mut *session }
        .inner
        .configure_context_profile(app_class_override, punctuation_policy)
    {
        Ok(()) => 0,
        Err(status) => status,
    }
}

#[unsafe(no_mangle)]
/// Supplies the immediately preceding host text for the next composition.
/// The value stays in session memory only for that composition. Secure and
/// unknown-security events discard it before any input is processed.
///
/// # Safety
///
/// `session` must reference a live session. `text` follows the pointer/length
/// and UTF-8 rules of [`aim_session_create`].
pub unsafe extern "C" fn aim_session_set_preceding_text(
    session: *mut AimSession,
    text: *const u8,
    text_len: usize,
) -> i32 {
    if session.is_null() {
        return -1;
    }
    let Ok(text) = (unsafe { utf8_argument(text, text_len) }) else {
        return -2;
    };
    unsafe { &mut *session }
        .inner
        .set_host_preceding_text(&text);
    0
}

/// A synchronous batch of right-padded tokens [rows,40] to next-character
/// log-probabilities [rows,vocab]. Zero means success. See the C header.
pub type AimZhijuScorer = unsafe extern "C" fn(
    user_data: *mut c_void, tokens: *const i32, rows: usize, cols: usize, out: *mut f32, vocab: usize,
) -> i32;

#[unsafe(no_mangle)]
/// Loads the vocabulary independently of the optional context-bigram model.
/// A failed load preserves the previous vocabulary.
/// # Safety
/// `session` is live and exclusively accessed; `path` is UTF-8 bytes of `path_len`.
pub unsafe extern "C" fn aim_session_set_zhiju_vocab(
    session: *mut AimSession, path: *const u8, path_len: usize,
) -> i32 {
    if session.is_null() { return -1; }
    let Ok(path) = (unsafe { utf8_argument(path, path_len) }) else { return -2; };
    match unsafe { &mut *session }.inner.set_zhiju_vocab(std::path::Path::new(&path)) {
        Ok(()) => 0,
        Err(_) => -2,
    }
}

#[unsafe(no_mangle)]
/// Runs a sentence-abbreviation search without changing the composition/page.
/// `*out_len` is capacity on input and bytes written/required on output.
/// The compact UTF-8 records and status codes are documented in the C header.
/// # Safety
/// Session access is exclusive for the entire synchronous search. `out_len`
/// points to writable storage; `out` has its input capacity. The callback must
/// fill [rows,vocab], must not unwind or reenter the session, and may only use
/// its borrowed buffers for the duration of the callback.
pub unsafe extern "C" fn aim_session_zhiju_search(
    session: *mut AimSession, scorer: Option<AimZhijuScorer>, user_data: *mut c_void,
    beam: usize, budget_ms: u64, out: *mut u8, out_len: *mut usize,
) -> i32 {
    if out_len.is_null() { return -1; }
    let capacity = unsafe { *out_len };
    unsafe { *out_len = 0; }
    if session.is_null() || scorer.is_none() || (out.is_null() && capacity != 0) { return -1; }
    let scorer = scorer.unwrap();
    let result = unsafe { &mut *session }.inner.zhiju_search(beam, Duration::from_millis(budget_ms),
        |tokens, rows, cols, logprobs, vocab| {
            match unsafe { scorer(user_data, tokens.as_ptr(), rows, cols, logprobs.as_mut_ptr(), vocab) } {
                0 => Ok(()), _ => Err(()),
            }
        });
    let candidates = match result { Ok(candidates) => candidates, Err(status) => return status };
    let mut encoded = String::new();
    for candidate in &candidates {
        use std::fmt::Write;
        let _ = writeln!(encoded, "{}\t{}\t{}", candidate.text, candidate.score, candidate.consumed);
    }
    unsafe { *out_len = encoded.len(); }
    if encoded.len() > capacity { return -6; }
    if !encoded.is_empty() { unsafe { ptr::copy_nonoverlapping(encoded.as_ptr(), out, encoded.len()); } }
    unsafe { &mut *session }.inner.remember_zhiju_results(&candidates);
    0
}

/// Copies one borrowed batch request to the host; no inference runs in this callback.
pub type AimZhijuRowsReady = unsafe extern "C" fn(
    user_data: *mut c_void, tokens: *const i32, rows: usize, cols: usize, vocab: usize,
);

#[unsafe(no_mangle)]
/// Loads a worker-owned vocabulary, without accessing a session or the runtime.
/// # Safety
/// `path` is valid UTF-8 storage of `path_len` bytes.
pub unsafe extern "C" fn aim_zhiju_search_create(path: *const u8, path_len: usize) -> *mut AimZhijuSearch {
    let Ok(path) = (unsafe { utf8_argument(path, path_len) }) else { return ptr::null_mut(); };
    let Ok(vocab) = zhixu::zhiju::Vocabulary::load(std::path::Path::new(&path)) else { return ptr::null_mut(); };
    Box::into_raw(Box::new(AimZhijuSearch { inner: zhixu::zhiju::Search::new(vocab) }))
}

#[unsafe(no_mangle)]
/// # Safety
/// `search` is null or a live, exclusively owned worker search, destroyed once.
pub unsafe extern "C" fn aim_zhiju_search_destroy(search: *mut AimZhijuSearch) {
    if !search.is_null() { unsafe { drop(Box::from_raw(search)); } }
}

#[unsafe(no_mangle)]
/// Captures only immutable dictionary ownership, input, context and identity.
/// # Safety
/// Session access is exclusive for this call; `out` is writable pointer storage.
pub unsafe extern "C" fn aim_session_zhiju_snapshot(session: *const AimSession, out: *mut *mut AimZhijuSnapshot) -> i32 {
    if out.is_null() { return -1; }
    unsafe { *out = ptr::null_mut(); }
    if session.is_null() { return -1; }
    match unsafe { &*session }.inner.zhiju_snapshot() {
        Ok(inner) => { unsafe { *out = Box::into_raw(Box::new(AimZhijuSnapshot { inner })); } 0 },
        Err(status) => status,
    }
}

#[unsafe(no_mangle)]
/// # Safety
/// `snapshot` is null or a live snapshot, destroyed once after its last use.
pub unsafe extern "C" fn aim_zhiju_snapshot_destroy(snapshot: *mut AimZhijuSnapshot) {
    if !snapshot.is_null() { unsafe { drop(Box::from_raw(snapshot)); } }
}

#[unsafe(no_mangle)]
/// Prepares reusable dictionary caches on the worker, outside the search deadline.
/// # Safety
/// Both handles are live and exclusive. No session access is required.
pub unsafe extern "C" fn aim_zhiju_search_prepare(search: *mut AimZhijuSearch, snapshot: *const AimZhijuSnapshot) -> i32 {
    if search.is_null() || snapshot.is_null() { return -1; }
    unsafe { &*snapshot }.inner.prepare(&mut unsafe { &mut *search }.inner);
    0
}

#[unsafe(no_mangle)]
/// Advances a worker-owned snapshot; it never accesses the originating session.
/// # Safety
/// Handles are live, exclusively accessed and use the same search for every step.
/// Buffer and callback rules match `aim_session_zhiju_step`.
pub unsafe extern "C" fn aim_zhiju_snapshot_step(
    snapshot: *mut AimZhijuSnapshot, search: *mut AimZhijuSearch, request: Option<AimZhijuRowsReady>, user_data: *mut c_void,
    beam: usize, budget_ms: u64, scores: *const f32, score_count: usize, out: *mut u8, out_len: *mut usize,
) -> i32 {
    if out_len.is_null() { return -1; }
    let capacity = unsafe { *out_len };
    unsafe { *out_len = 0; }
    if snapshot.is_null() || search.is_null() || request.is_none()
        || (scores.is_null() && score_count != 0) || (out.is_null() && capacity != 0) { return -1; }
    if score_count > isize::MAX as usize / std::mem::size_of::<f32>() { return -2; }
    let scores = if score_count == 0 { &[] } else { unsafe { slice::from_raw_parts(scores, score_count) } };
    let request = request.unwrap();
    let result = unsafe { &mut *snapshot }.inner.step(&mut unsafe { &mut *search }.inner, beam,
        Duration::from_millis(budget_ms), scores, |tokens, rows, vocab| {
            unsafe { request(user_data, tokens.as_ptr(), rows, 40, vocab); }
        });
    let candidates = match result { Ok(Some(candidates)) => candidates, Ok(None) => return 1, Err(status) => return status };
    let mut encoded = String::new();
    for candidate in &candidates {
        use std::fmt::Write;
        let _ = writeln!(encoded, "{}\t{}\t{}", candidate.text, candidate.score, candidate.consumed);
    }
    unsafe { *out_len = encoded.len(); }
    if encoded.len() > capacity { return -6; }
    if !encoded.is_empty() { unsafe { ptr::copy_nonoverlapping(encoded.as_ptr(), out, encoded.len()); } }
    0
}

#[unsafe(no_mangle)]
/// Validates identity, optionally promotes the best result, and reads the page once.
/// # Safety
/// Session access is exclusive; snapshot is live and no longer being advanced.
/// `out` is writable, uninitialized AimResult storage, freed with aim_result_free.
pub unsafe extern "C" fn aim_session_apply_zhiju_snapshot(
    session: *mut AimSession, snapshot: *const AimZhijuSnapshot, promote: i32, out: *mut AimResult,
) -> i32 {
    if out.is_null() { return -1; }
    unsafe { ptr::write(out, AimResult::default()); }
    if session.is_null() || snapshot.is_null() { return -1; }
    let session = unsafe { &mut *session };
    let (status, page) = match session.inner.apply_zhiju_snapshot(&unsafe { &*snapshot }.inner, promote != 0) {
        Ok(result) => result, Err(status) => return status,
    };
    fill_result(unsafe { &mut *out }, page, 0);
    status
}

#[unsafe(no_mangle)]
/// Advances at most one saved beam step, then requests its successor or returns results.
/// See the C header for generation, budget and buffer ownership rules.
/// # Safety
/// Session access is exclusive only for this call. `generation` and `out_len`
/// are writable; `scores` has `score_count` floats, `out` has its input capacity.
/// The request callback must not unwind, reenter the session or retain pointers.
pub unsafe extern "C" fn aim_session_zhiju_step(
    session: *mut AimSession, generation: *mut u64, request: Option<AimZhijuRowsReady>, user_data: *mut c_void,
    beam: usize, budget_ms: u64, scores: *const f32, score_count: usize, out: *mut u8, out_len: *mut usize,
) -> i32 {
    if out_len.is_null() { return -1; }
    let capacity = unsafe { *out_len };
    unsafe { *out_len = 0; }
    if session.is_null() || generation.is_null() || request.is_none()
        || (scores.is_null() && score_count != 0) || (out.is_null() && capacity != 0) { return -1; }
    if score_count > isize::MAX as usize / std::mem::size_of::<f32>() { return -2; }
    let scores = if score_count == 0 { &[] } else { unsafe { slice::from_raw_parts(scores, score_count) } };
    let request = request.unwrap();
    let result = unsafe { &mut *session }.inner.zhiju_step(unsafe { &mut *generation }, beam,
        Duration::from_millis(budget_ms), scores, |tokens, rows, vocab| {
            unsafe { request(user_data, tokens.as_ptr(), rows, 40, vocab); }
        });
    let candidates = match result { Ok(Some(candidates)) => candidates, Ok(None) => return 1, Err(status) => return status };
    let mut encoded = String::new();
    for candidate in &candidates {
        use std::fmt::Write;
        let _ = writeln!(encoded, "{}\t{}\t{}", candidate.text, candidate.score, candidate.consumed);
    }
    unsafe { *out_len = encoded.len(); }
    if encoded.len() > capacity { return -6; }
    if !encoded.is_empty() { unsafe { ptr::copy_nonoverlapping(encoded.as_ptr(), out, encoded.len()); } }
    unsafe { &mut *session }.inner.remember_zhiju_results(&candidates);
    0
}

#[unsafe(no_mangle)]
/// Returns eligible slots in the original visible page. No text crosses this ABI.
/// # Safety
/// `session` must be null or a live, exclusively accessed session.
pub unsafe extern "C" fn aim_session_context_group_mask(session: *mut AimSession) -> u32 {
    if session.is_null() {
        return 0;
    }
    unsafe { &*session }.inner.context_group_mask()
}

#[unsafe(no_mangle)]
/// Returns slots whose reading covers the whole raw input (model-first group). No text
/// crosses this ABI.
/// # Safety
/// `session` must be null or a live, exclusively accessed session.
pub unsafe extern "C" fn aim_session_context_complete_mask(session: *mut AimSession) -> u32 {
    if session.is_null() {
        return 0;
    }
    unsafe { &*session }.inner.context_complete_mask()
}

#[unsafe(no_mangle)]
/// Allows the contextual soft prior only when the current first has no matching pick.
/// # Safety
/// `session` must be null or a live, exclusively accessed session.
pub unsafe extern "C" fn aim_session_contextual_prior_allowed(session: *mut AimSession) -> i32 {
    if session.is_null() { return 0; }
    i32::from(unsafe { &*session }.inner.contextual_prior_allowed())
}

#[unsafe(no_mangle)]
/// C3: writes the whole-reading compositions the context model may lead with, as UTF-8
/// separated by `\t`. Returns the bytes written, or the negated size when too small.
/// # Safety
/// `session` must be live and exclusively accessed; `buffer` writable for `capacity`.
pub unsafe extern "C" fn aim_session_composition_alternatives(
    session: *mut AimSession,
    buffer: *mut u8,
    capacity: usize,
) -> isize {
    if session.is_null() {
        return 0;
    }
    let text = unsafe { &*session }
        .inner
        .composition_alternatives()
        .iter()
        .map(|value| value.replace(['\t', '\n'], " "))
        .collect::<Vec<_>>()
        .join("\t");
    let bytes = text.as_bytes();
    if bytes.len() > capacity || buffer.is_null() {
        return -(bytes.len() as isize);
    }
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), buffer, bytes.len()) };
    bytes.len() as isize
}

#[unsafe(no_mangle)]
/// The texts of the current page as the core resolves digit keys against it, separated by `\t`.
/// A shell that draws a page it built itself (model reorder, composed lead, offered composition)
/// compares it with this before showing it. Returns the bytes written, or the negated size when too small.
/// # Safety
/// `session` must be live and exclusively accessed; `buffer` writable for `capacity`.
pub unsafe extern "C" fn aim_session_page_texts(
    session: *mut AimSession,
    buffer: *mut u8,
    capacity: usize,
) -> isize {
    if session.is_null() {
        return 0;
    }
    let text = unsafe { &*session }
        .inner
        .page_texts()
        .iter()
        .map(|value| value.replace(['\t', '\n'], " "))
        .collect::<Vec<_>>()
        .join("\t");
    let bytes = text.as_bytes();
    if bytes.len() > capacity || buffer.is_null() {
        return -(bytes.len() as isize);
    }
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), buffer, bytes.len()) };
    bytes.len() as isize
}

#[unsafe(no_mangle)]
/// Set core correction switches (nonzero = on). Process-wide; refreshes this session
/// without losing typed keys. Other live sessions pick up settings on their next key.
/// # Safety
/// `session` must be live and exclusively accessed.
pub unsafe extern "C" fn aim_session_set_corrections(
    session: *mut AimSession, enabled: i32, typed_costs: i32,
    multiple_readings: i32, trailing_letter: i32,
) -> i32 {
    if session.is_null() { return -1; }
    unsafe { &mut *session }.inner.set_correction_settings(ranking::CorrectionSettings {
        enabled: enabled != 0, typed_costs: typed_costs != 0,
        multiple_readings: multiple_readings != 0, trailing_letter: trailing_letter != 0,
    });
    0
}

#[unsafe(no_mangle)]
/// C3: normalized edit cost (cost / edit_penalty), first candidate then alternatives of
/// `aim_session_composition_alternatives`, tab-separated decimals; length or -needed.
/// # Safety
/// `session` is a live session; `buffer` holds `capacity` bytes.
pub unsafe extern "C" fn aim_session_composition_edits(
    session: *mut AimSession,
    buffer: *mut u8,
    capacity: usize,
) -> isize {
    if session.is_null() {
        return 0;
    }
    let text = unsafe { &*session }
        .inner
        .composition_edits()
        .iter()
        .map(|value| ranking::format_edit_units(*value))
        .collect::<Vec<_>>()
        .join("\t");
    let bytes = text.as_bytes();
    if bytes.len() > capacity || buffer.is_null() {
        return -(bytes.len() as isize);
    }
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), buffer, bytes.len()) };
    bytes.len() as isize
}

#[unsafe(no_mangle)]
/// C3: 1 when the typed letters are not whole pinyin (the decoder's first is a guess at
/// abbreviations or a slip, `ranking::GUESSED_COMPOSITION_LEAD_MARGIN`), else 0.
/// # Safety
/// `session` is a live session.
pub unsafe extern "C" fn aim_session_composition_guessed(session: *mut AimSession) -> i32 {
    if session.is_null() {
        return 0;
    }
    i32::from(unsafe { &*session }.inner.composition_guessed())
}

/// Tab-separated texts from a C buffer (None when not UTF-8 or the count differs).
unsafe fn tab_separated(texts: *const u8, length: usize, count: usize) -> Option<Vec<String>> {
    if texts.is_null() {
        return None;
    }
    let bytes = unsafe { std::slice::from_raw_parts(texts, length) };
    let parts: Vec<String> = std::str::from_utf8(bytes).ok()?.split('\t').map(str::to_string).collect();
    (parts.len() == count).then_some(parts)
}

#[unsafe(no_mangle)]
/// C3 decision (`ranking::composition_lead`): `texts` holds the decoder's first and the
/// alternatives, tab-separated, with one score and one edit count each; `guessed` from
/// `aim_session_composition_guessed`. Returns the index of the composition that leads,
/// or -1.
/// # Safety
/// `texts` readable for `length` bytes; `scores` and `edits` hold `count` values.
pub unsafe extern "C" fn aim_ranking_composition_lead(
    texts: *const u8,
    length: usize,
    scores: *const f64,
    edits: *const f64,
    count: usize,
    guessed: i32,
) -> i32 {
    if scores.is_null() || edits.is_null() {
        return -1;
    }
    let Some(texts) = (unsafe { tab_separated(texts, length, count) }) else { return -1 };
    let scores = unsafe { std::slice::from_raw_parts(scores, count) };
    let edits = unsafe { std::slice::from_raw_parts(edits, count) };
    ranking::composition_lead(&texts, scores, edits, guessed != 0).map_or(-1, |index| index as i32)
}

#[unsafe(no_mangle)]
/// Offer decision (`ranking::composition_offer`), same arguments as
/// `aim_ranking_composition_lead` without `guessed`. Returns the index of the
/// composition offered in the last first-page slot, or -1.
/// # Safety
/// `texts` readable for `length` bytes; `scores` and `edits` hold `count` values.
pub unsafe extern "C" fn aim_ranking_composition_offer(
    texts: *const u8,
    length: usize,
    scores: *const f64,
    edits: *const f64,
    count: usize,
) -> i32 {
    if scores.is_null() || edits.is_null() {
        return -1;
    }
    let Some(texts) = (unsafe { tab_separated(texts, length, count) }) else { return -1 };
    let scores = unsafe { std::slice::from_raw_parts(scores, count) };
    let edits = unsafe { std::slice::from_raw_parts(edits, count) };
    ranking::composition_offer(&texts, scores, edits).map_or(-1, |index| index as i32)
}

#[unsafe(no_mangle)]
/// C2/C1 order of a group (`ranking::group_order`): writes `count` indices into `order`
/// (0 on success). `homophone[i]` is nonzero when `texts[i]` reads like the first.
/// # Safety
/// `texts` readable for `length` bytes; `scores`, `homophone` and `order` hold `count`.
pub unsafe extern "C" fn aim_ranking_group_order(
    texts: *const u8,
    length: usize,
    scores: *const f64,
    homophone: *const u8,
    count: usize,
    order: *mut usize,
) -> i32 {
    if scores.is_null() || homophone.is_null() || order.is_null() || count == 0 {
        return -1;
    }
    let Some(texts) = (unsafe { tab_separated(texts, length, count) }) else { return -1 };
    let scores = unsafe { std::slice::from_raw_parts(scores, count) };
    let homophone: Vec<bool> = unsafe { std::slice::from_raw_parts(homophone, count) }.iter().map(|&b| b != 0).collect();
    let ranked = ranking::group_order(&texts, scores, &homophone);
    unsafe { std::ptr::copy_nonoverlapping(ranked.as_ptr(), order, count) };
    0
}

#[unsafe(no_mangle)]
/// Context-aware C1 order; C2 and settled-owner protections retain their rules.
/// # Safety
/// `texts` readable for `length` bytes; the other arrays hold `count` entries.
pub unsafe extern "C" fn aim_ranking_contextual_group_order(
    texts: *const u8, length: usize, scores: *const f64, homophone: *const u8,
    count: usize, has_context: i32, order: *mut usize,
) -> i32 {
    if scores.is_null() || homophone.is_null() || order.is_null() || count == 0
        || !(0..=1).contains(&has_context) { return -1; }
    let Some(texts) = (unsafe { tab_separated(texts, length, count) }) else { return -1 };
    let scores = unsafe { std::slice::from_raw_parts(scores, count) };
    if !scores.iter().all(|value| value.is_finite()) { return -1; }
    let homophone: Vec<bool> = unsafe { std::slice::from_raw_parts(homophone, count) }.iter().map(|&b| b != 0).collect();
    let ranked = ranking::contextual_group_order(&texts, scores, &homophone, has_context != 0);
    unsafe { std::ptr::copy_nonoverlapping(ranked.as_ptr(), order, count) };
    0
}

#[unsafe(no_mangle)]
/// Builds the owner's settled habits from their local quality records (JSON lines).
/// # Safety
/// `records` readable for `length` bytes.
pub unsafe extern "C" fn aim_ranking_load_owner_style(records: *const u8, length: usize) -> i32 {
    if records.is_null() {
        return -1;
    }
    let bytes = unsafe { std::slice::from_raw_parts(records, length) };
    ranking::load_owner_style(&String::from_utf8_lossy(bytes));
    0
}

#[unsafe(no_mangle)]
/// 1 when the owner's habits keep `first` ahead of `challenger`, else 0.
/// # Safety
/// `first` and `challenger` readable for their lengths.
pub unsafe extern "C" fn aim_ranking_owner_keeps(
    first: *const u8,
    first_length: usize,
    challenger: *const u8,
    challenger_length: usize,
) -> i32 {
    if first.is_null() || challenger.is_null() {
        return 0;
    }
    let first = unsafe { std::slice::from_raw_parts(first, first_length) };
    let challenger = unsafe { std::slice::from_raw_parts(challenger, challenger_length) };
    match (std::str::from_utf8(first), std::str::from_utf8(challenger)) {
        (Ok(first), Ok(challenger)) => i32::from(ranking::owner_keeps(first, challenger)),
        _ => 0,
    }
}

#[unsafe(no_mangle)]
/// C3: puts one of the current alternatives first for this input. 0 on success; -1 invalid arguments; -10 the text is
/// not a current alternative; -11 the owner's learned whole word stays first (a model sentence never displaces what the
/// owner has taught); -12 something else is first on the real page. Success means the page really starts with it.
/// # Safety
/// `session` must be live and exclusively accessed; `text` readable for `length` bytes.
pub unsafe extern "C" fn aim_session_lead_with_composition(
    session: *mut AimSession,
    text: *const u8,
    length: usize,
) -> i32 {
    if session.is_null() || text.is_null() {
        return -1;
    }
    let bytes = unsafe { std::slice::from_raw_parts(text, length) };
    let Ok(value) = std::str::from_utf8(bytes) else {
        return -1;
    };
    match unsafe { &mut *session }.inner.lead_with_composition(value) {
        Ok(()) => 0,
        Err(status) => status,
    }
}

#[unsafe(no_mangle)]
/// Offers a current C3 alternative or full-reading-validated Zhiju result in the
/// last slot of the first page (0 on success). The first four slots stay fixed.
/// # Safety
/// `session` must be live and exclusively accessed; `text` readable for `length` bytes.
pub unsafe extern "C" fn aim_session_offer_composition(
    session: *mut AimSession,
    text: *const u8,
    length: usize,
) -> i32 {
    if session.is_null() || text.is_null() {
        return -1;
    }
    let bytes = unsafe { std::slice::from_raw_parts(text, length) };
    let Ok(value) = std::str::from_utf8(bytes) else {
        return -1;
    };
    match unsafe { &mut *session }.inner.offer_composition(value) {
        Ok(()) => 0,
        Err(status) => status,
    }
}

#[unsafe(no_mangle)]
/// Bounded off-page dictionary alternatives for short initials, UTF-8 TSV.
/// # Safety
/// `session` must be live and exclusive; `buffer` writable for `capacity` bytes,
/// or null with capacity zero to query the negated required size.
pub unsafe extern "C" fn aim_session_abbreviation_alternatives(
    session: *mut AimSession, buffer: *mut u8, capacity: usize,
) -> isize {
    if session.is_null() { return 0; }
    let text = unsafe { &*session }.inner.abbreviation_alternatives().join("\t");
    if text.len() > capacity || buffer.is_null() { return -(text.len() as isize); }
    unsafe { ptr::copy_nonoverlapping(text.as_ptr(), buffer, text.len()) };
    text.len() as isize
}

#[unsafe(no_mangle)]
/// Offers a currently eligible dictionary word in the last first-page slot.
/// # Safety
/// `session` must be live and exclusive; `text` readable for `length` bytes.
pub unsafe extern "C" fn aim_session_offer_abbreviation(
    session: *mut AimSession, text: *const u8, length: usize,
) -> i32 {
    if session.is_null() || text.is_null() { return -1; }
    let Ok(text) = std::str::from_utf8(unsafe { slice::from_raw_parts(text, length) }) else { return -1; };
    unsafe { &mut *session }.inner.offer_abbreviation(text).err().unwrap_or(0)
}

#[unsafe(no_mangle)]
/// Writes the candidate texts of up to `count` following pages (expanded panel) into
/// `buffer` as UTF-8: pages separated by `\n`, candidates by `\t`. Returns the bytes
/// written, or the negated required size when `capacity` is too small. Page, highlight
/// and model order are unchanged.
/// # Safety
/// `session` must be live and exclusively accessed; `buffer` must be writable for
/// `capacity` bytes (or null with capacity 0 to query the size).
pub unsafe extern "C" fn aim_session_peek_following_pages(
    session: *mut AimSession,
    count: usize,
    buffer: *mut u8,
    capacity: usize,
) -> isize {
    if session.is_null() {
        return 0;
    }
    let pages = unsafe { &mut *session }.inner.peek_following_pages(count);
    let text = pages
        .iter()
        .map(|page| {
            page.iter()
                .map(|candidate| candidate.text.replace(['\t', '\n'], " "))
                .collect::<Vec<_>>()
                .join("\t")
        })
        .collect::<Vec<_>>()
        .join("\n");
    let bytes = text.as_bytes();
    if bytes.len() > capacity || buffer.is_null() {
        return -(bytes.len() as isize);
    }
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), buffer, bytes.len()) };
    bytes.len() as isize
}

#[unsafe(no_mangle)]
/// Atomically stores a validated permutation of the original visible page.
/// # Safety
/// `session` must be live and exclusively accessed. `order` must contain
/// `count` readable indices. The page has at most eight slots.
pub unsafe extern "C" fn aim_session_reorder_context_candidates(
    session: *mut AimSession,
    order: *const usize,
    count: usize,
) -> i32 {
    if session.is_null() || order.is_null() || count == 0 || count > 8 {
        return -1;
    }
    let order = unsafe { slice::from_raw_parts(order, count) };
    match unsafe { &mut *session }
        .inner
        .reorder_context_candidates(order)
    {
        Ok(()) => 0,
        Err(status) => status,
    }
}

#[unsafe(no_mangle)]
/// Parses an immutable Mixer lexicon snapshot from canonical JSON.
///
/// # Safety
///
/// When `json_len` is nonzero, `json` must point to `json_len` readable bytes.
/// The returned pointer, when non-null, must be released exactly once with
/// [`aim_lexicon_snapshot_destroy`].
pub unsafe extern "C" fn aim_lexicon_snapshot_prepare(
    json: *const u8,
    json_len: usize,
) -> *mut AimLexiconSnapshot {
    let bytes = if json.is_null() {
        if json_len == 0 {
            &[][..]
        } else {
            return ptr::null_mut();
        }
    } else {
        unsafe { slice::from_raw_parts(json, json_len) }
    };
    match MixerLexicon::from_json(bytes) {
        Ok(lexicon) => Box::into_raw(Box::new(AimLexiconSnapshot {
            inner: std::sync::Arc::new(lexicon),
        })),
        Err(_) => ptr::null_mut(),
    }
}

#[unsafe(no_mangle)]
/// Releases a prepared Mixer lexicon snapshot.
///
/// # Safety
///
/// `snapshot` must be null or a live pointer returned by
/// [`aim_lexicon_snapshot_prepare`] that has not already been released.
pub unsafe extern "C" fn aim_lexicon_snapshot_destroy(snapshot: *mut AimLexiconSnapshot) {
    if !snapshot.is_null() {
        unsafe { drop(Box::from_raw(snapshot)) };
    }
}

#[unsafe(no_mangle)]
/// Applies an immutable Mixer lexicon snapshot to a session.
///
/// # Safety
///
/// `session` and `snapshot` must reference live values returned by their
/// corresponding create/prepare functions for the duration of the call.
pub unsafe extern "C" fn aim_session_apply_lexicon(
    session: *mut AimSession,
    snapshot: *const AimLexiconSnapshot,
) -> i32 {
    if session.is_null() || snapshot.is_null() {
        return -1;
    }
    let snapshot = unsafe { &*snapshot };
    unsafe { &mut *session }
        .inner
        .apply_lexicon(snapshot.inner.clone());
    0
}

#[unsafe(no_mangle)]
/// Parses an immutable deterministic context-disambiguation table.
///
/// # Safety
///
/// When `model_len` is nonzero, `model` must point to `model_len` readable
/// bytes. The returned pointer must be released exactly once with
/// [`aim_context_model_snapshot_destroy`].
pub unsafe extern "C" fn aim_context_model_snapshot_prepare(
    model: *const u8,
    model_len: usize,
) -> *mut AimContextModelSnapshot {
    let bytes = if model.is_null() {
        if model_len == 0 {
            &[][..]
        } else {
            return ptr::null_mut();
        }
    } else {
        unsafe { slice::from_raw_parts(model, model_len) }
    };
    match ContextModel::from_json(bytes) {
        Ok(model) => Box::into_raw(Box::new(AimContextModelSnapshot {
            inner: std::sync::Arc::new(model),
        })),
        Err(_) => ptr::null_mut(),
    }
}

#[unsafe(no_mangle)]
/// Releases a prepared context model snapshot.
///
/// # Safety
///
/// `snapshot` must be null or a live pointer returned by
/// [`aim_context_model_snapshot_prepare`] that has not been released.
pub unsafe extern "C" fn aim_context_model_snapshot_destroy(
    snapshot: *mut AimContextModelSnapshot,
) {
    if !snapshot.is_null() {
        unsafe { drop(Box::from_raw(snapshot)) };
    }
}

#[unsafe(no_mangle)]
/// Applies one immutable context model to a live session.
///
/// # Safety
///
/// Both pointers must remain live for the duration of this call.
pub unsafe extern "C" fn aim_session_apply_context_model(
    session: *mut AimSession,
    snapshot: *const AimContextModelSnapshot,
) -> i32 {
    if session.is_null() || snapshot.is_null() {
        return -1;
    }
    let snapshot = unsafe { &*snapshot };
    unsafe { &mut *session }
        .inner
        .apply_context_model(snapshot.inner.clone());
    0
}

#[unsafe(no_mangle)]
/// Enables or disables typo-correction candidates for a session.
///
/// # Safety
///
/// `session` must reference a live session returned by
/// [`aim_session_create`].
pub unsafe extern "C" fn aim_session_set_typo_correction(
    session: *mut AimSession,
    enabled: u8,
) -> i32 {
    if session.is_null() || enabled > 1 {
        return -1;
    }
    unsafe { &mut *session }.inner.configure_typo(enabled != 0);
    0
}

#[unsafe(no_mangle)]
/// Configures the active project, professional packs, and second-layer state.
///
/// # Safety
///
/// `session` must reference a live session. Each non-null byte pointer must
/// remain readable for its associated length for the duration of the call.
pub unsafe extern "C" fn aim_session_set_candidate_context(
    session: *mut AimSession,
    active_project: *const u8,
    active_project_len: usize,
    active_packs_json: *const u8,
    active_packs_json_len: usize,
    secondary_enabled: u8,
    prediction_enabled: u8,
) -> i32 {
    if session.is_null()
        || secondary_enabled > 1
        || prediction_enabled > 1
        || active_packs_json_len > 64 * 1024
    {
        return -1;
    }
    let Ok(project) = (unsafe { utf8_argument(active_project, active_project_len) }) else {
        return -2;
    };
    if !project.is_empty() && (project.trim().is_empty() || project != project.trim()) {
        return -2;
    }
    let Ok(packs_json) = (unsafe { utf8_argument(active_packs_json, active_packs_json_len) })
    else {
        return -2;
    };
    let Ok(packs) = serde_json::from_str::<Vec<String>>(&packs_json) else {
        return -2;
    };
    if packs.len() > 256
        || packs
            .iter()
            .any(|pack| pack.trim().is_empty() || pack != pack.trim())
    {
        return -2;
    }
    unsafe { &mut *session }.inner.configure_candidate_context(
        (!project.is_empty()).then_some(project),
        packs,
        secondary_enabled != 0,
        prediction_enabled != 0,
    );
    0
}

#[unsafe(no_mangle)]
/// Selects an unnumbered candidate from the parallel second layer.
///
/// # Safety
///
/// `session` must reference a live session. `result` must point to writable
/// output storage; after success, release its owned buffers with
/// [`aim_result_free`].
pub unsafe extern "C" fn aim_session_select_secondary_candidate(
    session: *mut AimSession,
    index: usize,
    delivery_id: u64,
    security_state: u32,
    result: *mut AimResult,
) -> i32 {
    if session.is_null() || result.is_null() {
        return -1;
    }
    unsafe { result.write(AimResult::default()) };
    let started = Instant::now();
    let model = unsafe { &mut *session }.inner.select_secondary_candidate(
        index,
        delivery_id,
        SecurityState::from_u32(security_state),
    );
    let elapsed = started.elapsed().as_nanos().min(u64::MAX as u128) as u64;
    unsafe { fill_result(&mut *result, model, elapsed) };
    0
}

#[unsafe(no_mangle)]
/// Changes the direct-English/Chinese mode of a session.
///
/// # Safety
///
/// `session` must reference a live session returned by
/// [`aim_session_create`].
pub unsafe extern "C" fn aim_session_set_mode(session: *mut AimSession, mode: u32) -> i32 {
    if session.is_null() {
        return -1;
    }
    let mode = match mode {
        0 => crate::state::InputMode::Chinese,
        1 => crate::state::InputMode::DirectEnglish,
        _ => return -2,
    };
    unsafe { &mut *session }.inner.set_mode(mode);
    0
}

#[unsafe(no_mangle)]
/// Releases buffers owned by an [`AimResult`] and resets it to its default.
///
/// # Safety
///
/// `result` must be null or point to a valid [`AimResult`] previously written
/// by [`aim_session_process`] or [`aim_session_current_page`] and not already freed. Its owned pointer/count
/// fields must not have been modified by the caller.
pub unsafe extern "C" fn aim_result_free(result: *mut AimResult) {
    if result.is_null() {
        return;
    }
    let result = unsafe { &mut *result };
    unsafe {
        free_bytes(result.marked_text, result.marked_text_len);
        free_bytes(result.commit_text, result.commit_text_len);
    }
    if !result.candidates.is_null() && result.candidate_count > 0 {
        let candidates = unsafe {
            Box::from_raw(ptr::slice_from_raw_parts_mut(
                result.candidates,
                result.candidate_count,
            ))
        };
        for candidate in candidates.iter() {
            unsafe {
                free_bytes(candidate.text, candidate.text_len);
                free_bytes(candidate.comment, candidate.comment_len);
                free_bytes(candidate.reading_key, candidate.reading_key_len);
            }
        }
    }
    if !result.secondary_candidates.is_null() && result.secondary_candidate_count > 0 {
        let candidates = unsafe {
            Box::from_raw(ptr::slice_from_raw_parts_mut(
                result.secondary_candidates,
                result.secondary_candidate_count,
            ))
        };
        for candidate in candidates.iter() {
            unsafe {
                free_bytes(candidate.text, candidate.text_len);
                free_bytes(candidate.comment, candidate.comment_len);
                free_bytes(candidate.reading_key, candidate.reading_key_len);
            }
        }
    }
    *result = AimResult::default();
}

#[unsafe(no_mangle)]
pub extern "C" fn aim_core_abi_version() -> u32 {
    ABI_VERSION
}

#[unsafe(no_mangle)]
/// Waits for the background write of the persisted Zhixu index cache to settle.
/// Returns 0 when no write is pending or it finished (also when its target
/// rejected the file), 1 when `timeout_ms` (capped at 10 seconds) elapsed.
/// Short-lived processes call this after their last key so a cache built
/// during the session actually reaches the disk before exit.
pub extern "C" fn aim_index_cache_flush(timeout_ms: u32) -> i32 {
    let timeout = Duration::from_millis(u64::from(timeout_ms.min(10_000)));
    i32::from(!zhixu::index_cache::flush(timeout))
}

#[unsafe(no_mangle)]
pub extern "C" fn aim_live_session_count() -> usize {
    LIVE_SESSIONS.load(Ordering::SeqCst)
}

#[allow(dead_code)]
fn _opaque(_: *mut c_void) {}

#[cfg(test)]
mod ffi_tests {
    use super::*;

    /// `aim_live_session_count` is process-global, so a test that asserts a
    /// create/destroy pair leaves it unchanged cannot run while another test
    /// holds a session. Every test in this module that creates one takes this
    /// lock, which keeps the balance assertion exact rather than approximate.
    static LIVE_SESSION_TESTS: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn serialize_live_session_tests() -> std::sync::MutexGuard<'static, ()> {
        LIVE_SESSION_TESTS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn assert_same_ffi_page(actual: &AimResult, expected: &AimResult) {
        let text = |pointer, length| unsafe { utf8_argument(pointer, length) }.unwrap();
        let candidates = |pointer: *mut AimCandidate, count| {
            let values = if count == 0 { &[][..] }
                else { unsafe { slice::from_raw_parts(pointer, count) } };
            values.iter().map(|candidate| (
                text(candidate.text, candidate.text_len),
                text(candidate.comment, candidate.comment_len),
                text(candidate.reading_key, candidate.reading_key_len),
            )).collect::<Vec<_>>()
        };
        assert_eq!(text(actual.marked_text, actual.marked_text_len), text(expected.marked_text, expected.marked_text_len));
        assert_eq!(actual.marked_cursor_utf8, expected.marked_cursor_utf8);
        assert_eq!(candidates(actual.candidates, actual.candidate_count), candidates(expected.candidates, expected.candidate_count));
        assert_eq!(candidates(actual.secondary_candidates, actual.secondary_candidate_count),
            candidates(expected.secondary_candidates, expected.secondary_candidate_count));
        assert_eq!(actual.highlighted_index, expected.highlighted_index);
        assert_eq!(actual.page_no, expected.page_no);
        assert_eq!(actual.is_last_page, expected.is_last_page);
        assert_eq!(actual.mode, expected.mode);
        assert_eq!(actual.secure_degraded, expected.secure_degraded);
    }

    #[test]
    fn ffi_current_page_matches_process_and_accepts_uninitialized_output() {
        let app_id = "ffi.current-page".to_string();
        let mut session = AimSession { inner: Session::new(Box::new(crate::state::MockDecoder::default()), app_id.clone()), app_id };
        for (kind, text, page_no, highlight) in [(0, "a", 0, 0), (0, "b", 0, 0), (0, "c", 0, 0),
            (10, "", 0, 1), (8, "", 1, 0), (10, "", 1, 1)] {
            let event = AimKeyEvent { kind, text: text.as_ptr(), text_len: text.len(), modifiers: 0,
                security_state: 0, delivery_id: 941 };
            let mut expected = std::mem::MaybeUninit::<AimResult>::uninit();
            assert_eq!(unsafe { aim_session_process(&mut session, &event, expected.as_mut_ptr()) }, 0);
            let mut expected = unsafe { expected.assume_init() };
            assert_eq!((expected.page_no, expected.highlighted_index), (page_no, highlight));
            for _ in 0..2 {
                let mut current = std::mem::MaybeUninit::<AimResult>::uninit();
                assert_eq!(unsafe { aim_session_current_page(&mut session, current.as_mut_ptr()) }, 0);
                let mut current = unsafe { current.assume_init() };
                assert_same_ffi_page(&current, &expected);
                assert_eq!(current.status, 0);
                assert_eq!(current.consumed, 0);
                assert_eq!(current.commit_text_len, 0);
                assert!(current.commit_text.is_null());
                assert_eq!(current.selection_event_id, 0);
                unsafe { aim_result_free(&mut current) };
                assert!(current.marked_text.is_null());
                assert!(current.candidates.is_null());
                assert!(current.secondary_candidates.is_null());
            }
            unsafe { aim_result_free(&mut expected) };
        }
        assert!(include_str!("../include/agent_ime_core.h").contains("int32_t aim_session_current_page("));
    }

    #[test]
    fn ffi_current_page_reflects_accepted_and_refused_composition_leads() {
        crate::zhixu::with_index(crate::zhixu::fallback(), || {
            let app_id = "ffi.current-page-lead".to_string();
            let mut inner = Session::new(Box::new(crate::state::MockDecoder::default()), app_id.clone());
            for character in "taifuzale".chars() {
                inner.process(KeyEvent { kind: KeyKind::Text, text: character.to_string(), modifiers: 0,
                    security: SecurityState::NonSecure, delivery_id: 1 });
            }
            assert!(inner.composition_alternatives().iter().any(|text| text == "太复杂了"));
            let mut session = AimSession { inner, app_id };
            let mut before = AimResult::default();
            assert_eq!(unsafe { aim_session_current_page(&mut session, &mut before) }, 0);
            assert!(before.candidate_count > 0);
            let original_first = unsafe { utf8_argument((*before.candidates).text, (*before.candidates).text_len) }.unwrap();
            for (text, status) in [("不在备选里", -10), ("太复杂了", 0)] {
                assert_eq!(unsafe { aim_session_lead_with_composition(&mut session, text.as_ptr(), text.len()) }, status);
                let mut current = AimResult::default();
                assert_eq!(unsafe { aim_session_current_page(&mut session, &mut current) }, 0);
                assert!(current.candidate_count > 0);
                let first = unsafe { &*current.candidates };
                assert_eq!(unsafe { utf8_argument(first.text, first.text_len) }.unwrap(),
                    if status == 0 { text } else { &original_first });
                if status == 0 {
                    assert_eq!(unsafe { utf8_argument(first.comment, first.comment_len) }.unwrap(), ranking::COMPOSITION_COMMENT);
                    assert!(!unsafe { utf8_argument(first.reading_key, first.reading_key_len) }.unwrap().is_empty());
                } else {
                    assert_same_ffi_page(&current, &before);
                }
                unsafe { aim_result_free(&mut current) };
            }
            unsafe { aim_result_free(&mut before) };
        });
    }

    #[test]
    fn ffi_current_page_rejects_null_arguments() {
        assert_eq!(unsafe { aim_session_current_page(ptr::null_mut(), ptr::null_mut()) }, -1);
        let mut output = std::mem::MaybeUninit::<AimResult>::uninit();
        assert_eq!(unsafe { aim_session_current_page(ptr::null_mut(), output.as_mut_ptr()) }, -1);
        let mut output = unsafe { output.assume_init() };
        assert!(output.marked_text.is_null());
        assert!(output.candidates.is_null());
        assert!(output.secondary_candidates.is_null());
        unsafe { aim_result_free(&mut output) };
        let app_id = "ffi.current-page-null".to_string();
        let mut session = AimSession { inner: Session::new(Box::new(crate::state::MockDecoder::default()), app_id.clone()), app_id };
        assert_eq!(unsafe { aim_session_current_page(&mut session, ptr::null_mut()) }, -1);
    }

    #[test]
    fn ffi_zhiju_dictionary_whole_rejection_keeps_the_entire_page() {
        use crate::zhixu::loader::{DictionaryData, Entry};
        let dictionary = std::sync::Arc::new(crate::zhixu::Index::from_data(DictionaryData {
            entries: [("乱七八糟", "luan qi ba zao", 100000), ("老", "lao", 10), ("钱", "qian", 10),
                ("别", "bie", 10), ("追", "zhui", 10)].into_iter().map(|(text, reading, weight)| Entry {
                text: text.into(), reading: reading.into(), weight, layer: "fixture".into(),
            }).collect(),
        }));
        crate::zhixu::with_index(dictionary.clone(), || {
            let app_id = "ffi.zhiju-dictionary-whole".to_string();
            let mut session = AimSession { inner: Session::new_native(app_id.clone(),
                std::sync::Arc::new(std::sync::Mutex::new(crate::blacklist::Store::default())), dictionary).unwrap(), app_id };
            let path = std::env::temp_dir().join(format!("zhiju-word-guard-{}.json", std::process::id()));
            std::fs::write(&path, serde_json::to_vec(&["<pad>", "<bos>", "<unk>", "老", "钱", "别", "追"]).unwrap()).unwrap();
            session.inner.set_zhiju_vocab(&path).unwrap();
            std::fs::remove_file(path).unwrap();
            for ch in "lqbz".chars() {
                session.inner.process(KeyEvent { kind: KeyKind::Text, text: ch.to_string(), modifiers: 0,
                    security: SecurityState::NonSecure, delivery_id: 0 });
            }
            unsafe extern "C" fn scorer(_: *mut c_void, _: *const i32, rows: usize, _: usize,
                out: *mut f32, vocab: usize) -> i32 {
                unsafe { std::slice::from_raw_parts_mut(out, rows * vocab) }.fill(-1.0);
                0
            }
            let mut output = [0; 2048];
            let mut length = output.len();
            assert_eq!(unsafe { aim_session_zhiju_search(&mut session, Some(scorer), ptr::null_mut(),
                8, 10000, output.as_mut_ptr(), &mut length) }, 0);
            assert!(std::str::from_utf8(&output[..length]).unwrap().starts_with("老钱别追\t"));
            let mut before = AimResult::default();
            assert_eq!(unsafe { aim_session_current_page(&mut session, &mut before) }, 0);
            let first = unsafe { &*before.candidates };
            assert_eq!(unsafe { utf8_argument(first.text, first.text_len) }.unwrap(), "乱七八糟");
            let target = "老钱别追";
            assert_eq!(unsafe { aim_session_lead_with_composition(&mut session, target.as_ptr(), target.len()) },
                mixer::LEAD_OUTRANKED_BY_DICTIONARY_WHOLE);
            let mut after = AimResult::default();
            assert_eq!(unsafe { aim_session_current_page(&mut session, &mut after) }, 0);
            assert_same_ffi_page(&after, &before);
            unsafe { aim_result_free(&mut before); aim_result_free(&mut after); }
        });
    }

    #[test]
    fn ffi_zhiju_search_result_can_lead_and_commit_the_real_page() {
        use crate::zhixu::loader::{DictionaryData, Entry};
        let dictionary = std::sync::Arc::new(crate::zhixu::Index::from_data(DictionaryData {
            entries: [
                ("我们", "wo men"), ("我", "wo"), ("外", "wai"),
                ("为", "wei"), ("问", "wen"), ("晚", "wan"),
                ("们", "men"), ("没", "mei"), ("明", "ming"), ("面", "mian"), ("蛮", "man"),
                ("想", "xiang"), ("下", "xia"), ("先", "xian"), ("小", "xiao"), ("玄", "xuan"),
                ("吃", "chi"), ("次", "ci"), ("从", "cong"), ("才", "cai"), ("醋", "cu"),
            ].into_iter().enumerate().map(|(at, (text, reading))| Entry {
                text: text.into(), reading: reading.into(), weight: 100_000 / (at as u32 + 1), layer: "fixture".into(),
            }).collect(),
        }));
        crate::zhixu::with_index(dictionary.clone(), || {
            let app_id = "ffi.zhiju-lead".to_string();
            let mut session = AimSession { inner: Session::new_native(app_id.clone(),
                std::sync::Arc::new(std::sync::Mutex::new(crate::blacklist::Store::default())), dictionary.clone()).unwrap(), app_id };
            let path = std::env::temp_dir().join(format!("zhiju-accept-{}.json", std::process::id()));
            std::fs::write(&path, br#"["<pad>","<bos>","<unk>","\u665a","\u86ee","\u7384","\u918b","\u5916"]"#).unwrap();
            session.inner.set_zhiju_vocab(&path).unwrap();
            std::fs::remove_file(path).unwrap();
            for ch in "wmxc".chars() {
                session.inner.process(KeyEvent { kind: KeyKind::Text, text: ch.to_string(), modifiers: 0,
                    security: SecurityState::NonSecure, delivery_id: 0 });
            }
            let target = "晚蛮玄醋";
            assert!(!session.inner.composition_alternatives().iter().any(|text| text == target));
            assert_eq!(session.inner.lead_with_composition(target), Err(mixer::LEAD_NOT_AN_ALTERNATIVE));
            unsafe extern "C" fn scorer(_: *mut c_void, _: *const i32, rows: usize, _: usize,
                out: *mut f32, vocab: usize) -> i32 {
                unsafe { std::slice::from_raw_parts_mut(out, rows * vocab) }.fill(-1.0);
                0
            }
            let before = session.inner.current_result();
            let mut output = [0; 2048];
            let mut length = 1;
            assert_eq!(unsafe { aim_session_zhiju_search(&mut session, Some(scorer), ptr::null_mut(),
                8, 10000, output.as_mut_ptr(), &mut length) }, -6);
            assert_eq!(session.inner.lead_with_composition(target), Err(mixer::LEAD_NOT_AN_ALTERNATIVE),
                "an unsuccessful FFI return does not admit search results");
            length = output.len();
            assert_eq!(unsafe { aim_session_zhiju_search(&mut session, Some(scorer), ptr::null_mut(),
                8, 10000, output.as_mut_ptr(), &mut length) }, 0);
            assert!(std::str::from_utf8(&output[..length]).unwrap().lines().any(|line| line.starts_with(target)));
            assert_eq!(session.inner.current_result().candidates, before.candidates, "search does not change the page");
            assert_eq!(session.inner.lead_with_composition("外蛮玄醋"), Ok(()), "all returned top-K sentences are eligible");
            assert_eq!(session.inner.lead_with_composition(target), Ok(()));
            let page = session.inner.current_result();
            assert_eq!(page.candidates[0].text, target);
            let committed = session.inner.process(KeyEvent { kind: KeyKind::Text, text: "1".into(), modifiers: 0,
                security: SecurityState::NonSecure, delivery_id: 0xA1_9100 });
            assert_eq!(committed.commit_text, target);
            assert!(committed.marked_text.is_empty());
            assert!(committed.candidates.is_empty());
        });
    }

    #[test]
    fn ffi_correction_settings_and_fractional_costs_round_trip() {
        // The settings are process-wide; exercise the public setter in a child so
        // the rest of the parallel suite retains its own defaults.
        const CHILD: &str = "ZHIXU_CORRECTION_FFI_TEST";
        if std::env::var_os(CHILD).is_none() {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "ffi_tests::ffi_correction_settings_and_fractional_costs_round_trip", "--nocapture"])
                .env(CHILD, "1").output().unwrap();
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
            return;
        }
        use crate::state::{Decoder, MockDecoder};
        let mut decoder = MockDecoder::default();
        decoder.input("yijignshile").unwrap();
        let app_id = "ffi.corrections".to_string();
        let mut session = AimSession { inner: Session::new(Box::new(decoder), app_id.clone()), app_id };
        assert_eq!(unsafe { aim_session_set_corrections(ptr::null_mut(), 1, 1, 1, 1) }, -1);
        assert_eq!(unsafe { aim_session_set_corrections(&mut session, 1, 1, 1, 1) }, 0);
        let alternatives = session.inner.composition_alternatives();
        // The sentence-level alternatives depend on the lexicon (一经是了 / 以及是了 ...); the contract here is that
        // a typed-key repair yields alternatives with fractional costs.
        assert!(!alternatives.is_empty());
        let expected = session.inner.composition_edits();
        assert!(expected.iter().any(|value| value.fract() != 0.0));
        let mut buffer = [0u8; 256];
        let size = unsafe { aim_session_composition_edits(&mut session, buffer.as_mut_ptr(), buffer.len()) };
        assert!(size > 0 && size <= 256);
        let decoded: Vec<f64> = std::str::from_utf8(&buffer[..size as usize]).unwrap()
            .split('\t').map(|value| value.parse().unwrap()).collect();
        assert_eq!(decoded.len(), alternatives.len() + 1);
        for (value, expected) in decoded.iter().zip(expected) { assert!((value - expected).abs() <= 0.0000005); }
        assert_eq!(unsafe { aim_session_set_corrections(&mut session, 0, 1, 1, 1) }, 0);
        assert!(!ranking::settings().enabled);
        assert!(!session.inner.composition_alternatives().is_empty(), "settings must keep typed keys");
        assert!(session.inner.composition_edits().iter().all(|value| value.fract() == 0.0));
        assert!(include_str!("../include/agent_ime_core.h").contains("int32_t aim_session_set_corrections("));
    }

    #[test]
    fn ffi_learning_mode_rejects_invalid_or_busy_sessions() {
        let app_id = "ffi.learning-mode".to_string();
        let mut session = AimSession { inner: Session::new(Box::new(crate::state::MockDecoder::default()), app_id.clone()), app_id };
        assert_eq!(unsafe { aim_session_set_learning(ptr::null_mut(), 1) }, -1);
        // The retired Rime rule (0) is rejected like any other invalid mode.
        for invalid in [0, 2, u32::MAX] {
            assert_eq!(unsafe { aim_session_set_learning(&mut session, invalid) }, -2);
        }
        assert_eq!(unsafe { aim_session_set_learning(&mut session, 1) }, 0);
        session.inner.process(KeyEvent { kind: KeyKind::Text, text: "n".into(), modifiers: 0,
            security: SecurityState::NonSecure, delivery_id: 1 });
        assert_eq!(unsafe { aim_session_set_learning(&mut session, 1) }, -10);
        let cancelled = session.inner.process(KeyEvent { kind: KeyKind::Escape, text: String::new(), modifiers: 0,
            security: SecurityState::NonSecure, delivery_id: 2 });
        assert!(cancelled.marked_text.is_empty());
        assert_eq!(unsafe { aim_session_set_learning(&mut session, 1) }, 0);
        assert!(include_str!("../include/agent_ime_core.h").contains("int32_t aim_session_set_learning("));
    }

    #[test]
    fn ffi_retired_learning_migration_entries_return_unsupported() {
        let user = b"/nonexistent/zhixu/user";
        assert_eq!(unsafe { aim_learning_available(user.as_ptr(), user.len()) }, -50);
        assert_eq!(unsafe { aim_learning_available(ptr::null(), 0) }, -50);
        assert_eq!(
            unsafe { aim_learning_migrate(b"shared".as_ptr(), 6, user.as_ptr(), user.len()) },
            -50,
        );
        assert_eq!(unsafe { aim_learning_migrate(ptr::null(), 0, ptr::null(), 0) }, -50);
        assert!(!std::path::Path::new("/nonexistent/zhixu").exists());
        let header = include_str!("../include/agent_ime_core.h");
        assert!(header.contains("int32_t aim_learning_available("));
        assert!(header.contains("int32_t aim_learning_migrate("));
    }

    #[test]
    fn ffi_index_cache_flush_settles_a_scheduled_write() {
        assert_eq!(aim_index_cache_flush(u32::MAX), 0, "the timeout is capped but still settles");
        let root = std::env::temp_dir().join(format!("aim-ffi-index-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("user")).unwrap();
        let dictionary = std::sync::Arc::new(zhixu::Index::from_data(zhixu::loader::DictionaryData {
            entries: vec![zhixu::loader::Entry {
                text: "你".into(),
                reading: "ni".into(),
                weight: 100,
                layer: "ffi".into(),
            }],
        }));
        dictionary.warm_whole();
        assert!(!zhixu::index_cache::ffi_test_schedule(&root, &dictionary), "nothing is on disk yet");
        assert_eq!(aim_index_cache_flush(50), 0, "a short timeout still writes unclaimed work inline");
        assert!(
            root.join("user").join("agent_zhixu_index_cache_v1.bin").is_file(),
            "flush wrote the scheduled cache file"
        );
        std::fs::remove_dir_all(root).unwrap();
        assert!(include_str!("../include/agent_ime_core.h").contains("int32_t aim_index_cache_flush("));
    }

    #[test]
    fn ffi_zhixu_whole_rejects_invalid_or_busy_sessions() {
        let app_id = "ffi.zhixu-whole".to_string();
        let mut session = AimSession { inner: Session::new(Box::new(crate::state::MockDecoder::default()), app_id.clone()), app_id };
        assert_eq!(unsafe { aim_session_set_zhixu_whole(ptr::null_mut(), 1) }, -1);
        for invalid in [2, u32::MAX] {
            assert_eq!(unsafe { aim_session_set_zhixu_whole(&mut session, invalid) }, -2);
        }
        for on in [0, 1] {
            assert_eq!(unsafe { aim_session_set_zhixu_whole(&mut session, on) }, 0);
        }
        session.inner.process(KeyEvent { kind: KeyKind::Text, text: "n".into(), modifiers: 0,
            security: SecurityState::NonSecure, delivery_id: 1 });
        for on in [0, 1] {
            assert_eq!(unsafe { aim_session_set_zhixu_whole(&mut session, on) }, -10);
        }
        session.inner.process(KeyEvent { kind: KeyKind::Escape, text: String::new(), modifiers: 0,
            security: SecurityState::NonSecure, delivery_id: 2 });
        assert_eq!(unsafe { aim_session_set_zhixu_whole(&mut session, 0) }, 0);
        assert!(include_str!("../include/agent_ime_core.h").contains("int32_t aim_session_set_zhixu_whole("));
    }

    #[test]
    fn ffi_linear_key_numbers_refresh_the_menu_without_committing() {
        use crate::state::MockDecoder;
        let app_id = "ffi.linear-keys".to_string();
        let mut inner = Session::new(Box::new(MockDecoder::default()), app_id.clone());
        for text in ["n", "i"] {
            inner.process(KeyEvent { kind: KeyKind::Text, text: text.into(), modifiers: 0,
                security: SecurityState::NonSecure, delivery_id: 0 });
        }
        let mut session = AimSession { app_id, inner };
        for (kind, highlight) in [(20, 1), (21, 0)] {
            let event = AimKeyEvent { kind, text: ptr::null(), text_len: 0, modifiers: 0,
                security_state: 0, delivery_id: 940 };
            let mut result = AimResult::default();
            assert_eq!(unsafe { aim_session_process(&mut session, &event, &mut result) }, 0);
            assert_eq!(result.status, 0);
            assert_eq!(result.consumed, 1);
            assert_eq!(result.highlighted_index, highlight);
            assert!(result.candidate_count > 1);
            assert!(result.marked_text_len > 0);
            assert_eq!(result.commit_text_len, 0);
            assert_eq!(result.selection_event_id, 0);
            unsafe { aim_result_free(&mut result) };
        }
        let header = include_str!("../include/agent_ime_core.h");
        assert!(header.contains("AIM_KEY_LINEAR_NEXT = 20"));
        assert!(header.contains("AIM_KEY_LINEAR_PREVIOUS = 21"));
    }

    /// `aim_session_create` attaches the process-global dictionary runtime, so
    /// tests that create sessions run in a child to leave other tests' state alone.
    fn session_child(name: &str) -> bool {
        const CHILD: &str = "AIM_FFI_SESSION_TEST";
        if std::env::var(CHILD).as_deref() == Ok(name) { return true; }
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &format!("ffi_tests::{name}"), "--nocapture"])
            .env(CHILD, name).output().unwrap();
        assert!(output.status.success(), "{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        false
    }

    /// `aim_session_create` deploys the native dictionaries, so tests that
    /// create sessions need real (minimal) shared directories.
    fn session_directories(name: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("aim-ffi-session-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let shared = root.join("shared");
        let user = root.join("user");
        std::fs::create_dir_all(&shared).unwrap();
        std::fs::create_dir_all(&user).unwrap();
        std::fs::write(shared.join("agent_pinyin.dict.yaml"),
            "---\nname: agent_pinyin\nsort: by_weight\nimport_tables:\n  - pinyin_simp\n...\n").unwrap();
        std::fs::write(shared.join("pinyin_simp.dict.yaml"),
            "---\nname: pinyin_simp\nsort: by_weight\n...\n你\tni\t100\n你好\tni hao\t90\n好\thao\t80\n").unwrap();
        root
    }

    #[test]
    fn ffi_create_process_destroy_balances_live_handles() {
        if !session_child("ffi_create_process_destroy_balances_live_handles") { return; }
        let baseline = aim_live_session_count();
        let root = session_directories("balances");
        let shared = root.join("shared");
        let user = root.join("user");
        let app = b"test.app";
        let shared_dir = shared.to_str().unwrap().as_bytes();
        let user_dir = user.to_str().unwrap().as_bytes();
        let session = unsafe {
            aim_session_create(
                shared_dir.as_ptr(),
                shared_dir.len(),
                user_dir.as_ptr(),
                user_dir.len(),
                app.as_ptr(),
                app.len(),
            )
        };
        assert!(!session.is_null());
        let mut result = std::mem::MaybeUninit::<AimResult>::uninit();
        let text = b"n";
        let event = AimKeyEvent {
            kind: 0,
            text: text.as_ptr(),
            text_len: text.len(),
            modifiers: 0,
            security_state: 0,
            delivery_id: 0,
        };
        assert_eq!(
            unsafe { aim_session_process(session, &event, result.as_mut_ptr()) },
            0
        );
        let mut result = unsafe { result.assume_init() };
        assert_eq!(result.consumed, 1);
        unsafe { aim_result_free(&mut result) };
        unsafe { aim_session_destroy(session) };
        assert_eq!(aim_live_session_count(), baseline);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ffi_create_fails_when_the_dictionaries_cannot_load() {
        if !session_child("ffi_create_fails_when_the_dictionaries_cannot_load") { return; }
        let baseline = aim_live_session_count();
        let root = session_directories("missing");
        // A shared directory without dictionary tables fails the native deployment;
        // the session must fail exactly as a failed Rime deployment used to.
        std::fs::remove_file(root.join("shared/pinyin_simp.dict.yaml")).unwrap();
        let shared = root.join("shared");
        let user = root.join("user");
        let app = b"test.app";
        let shared_dir = shared.to_str().unwrap().as_bytes();
        let user_dir = user.to_str().unwrap().as_bytes();
        let session = unsafe {
            aim_session_create(
                shared_dir.as_ptr(),
                shared_dir.len(),
                user_dir.as_ptr(),
                user_dir.len(),
                app.as_ptr(),
                app.len(),
            )
        };
        assert!(session.is_null());
        assert_eq!(aim_live_session_count(), baseline);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ffi_context_model_snapshot_is_validated_applied_and_released() {
        if !session_child("ffi_context_model_snapshot_is_validated_applied_and_released") { return; }
        let _serialized = serialize_live_session_tests();
        let bytes =
            r#"{"rules":[{"pinyin":"ceshi","senses":[{"word":"测试","triggers":["运行"]}]}]}"#
                .as_bytes();
        let model = unsafe { aim_context_model_snapshot_prepare(bytes.as_ptr(), bytes.len()) };
        assert!(!model.is_null());
        assert!(unsafe { aim_context_model_snapshot_prepare(std::ptr::null(), 0) }.is_null());

        let root = session_directories("context");
        let shared_dir = root.join("shared");
        let user_dir = root.join("user");
        let app_id = b"test.context.ffi";
        let session = unsafe {
            aim_session_create(
                shared_dir.to_str().unwrap().as_ptr() as *const u8,
                shared_dir.to_str().unwrap().len(),
                user_dir.to_str().unwrap().as_ptr() as *const u8,
                user_dir.to_str().unwrap().len(),
                app_id.as_ptr(),
                app_id.len(),
            )
        };
        assert!(!session.is_null());
        assert_eq!(
            unsafe { aim_session_apply_context_model(session, model) },
            0
        );
        unsafe {
            aim_context_model_snapshot_destroy(model);
            aim_session_destroy(session);
        }
        assert!(zhixu::index_cache::flush(Duration::from_millis(2_000)),
            "the background index cache must settle before removing its user directory");
        remove_dir_all_settled(root);
    }
}

/// Test helper: background prewarm/index-cache threads may still be writing into a temp directory when a test ends,
/// which makes a single `remove_dir_all` fail with "Directory not empty" (Q23). Retry for a few seconds.
#[cfg(test)]
pub(crate) fn remove_dir_all_settled(path: impl AsRef<std::path::Path>) {
    let path = path.as_ref();
    let mut last = None;
    for _ in 0..250 {
        match std::fs::remove_dir_all(path) {
            Ok(()) => return,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
            Err(error) => last = Some(error),
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    panic!("{path:?} still not removable after retries: {last:?}");
}
