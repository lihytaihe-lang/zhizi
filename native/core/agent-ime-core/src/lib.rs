#![deny(unsafe_op_in_unsafe_fn)]

mod blacklist;
mod context_model;
#[cfg(any(test, feature = "librime"))]
mod custom_config;
mod english_choices;
#[cfg(any(test, feature = "librime"))]
mod exact_readings;
mod mixer;
mod lexicon_layers;
mod personal_picks;
pub mod ranking;
#[cfg(feature = "librime")]
mod rime;
mod secure_directory;
#[cfg(any(test, feature = "librime"))]
mod selection_ledger;
mod state;

use context_model::ContextModel;
use mixer::MixerLexicon;
#[cfg(feature = "librime")]
use rime::RimeDecoder;
#[cfg(not(feature = "librime"))]
use state::MockDecoder;
use state::{Candidate, Decoder, KeyEvent, KeyKind, ResultModel, SecurityState, Session};
use std::ffi::c_void;
use std::ptr;
use std::slice;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

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
/// Creates one isolated IME session.
///
/// # Safety
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
    #[cfg(not(feature = "librime"))]
    let _ = (&shared_dir, &user_dir);

    #[cfg(feature = "librime")]
    let decoder: Box<dyn Decoder> = match RimeDecoder::new(&shared_dir, &user_dir) {
        Ok(decoder) => Box::new(decoder),
        Err(_) => return ptr::null_mut(),
    };
    #[cfg(feature = "librime")]
    let blacklist = match blacklist::shared_blacklist(std::path::Path::new(&user_dir)) {
        Ok(blacklist) => blacklist,
        Err(_) => return ptr::null_mut(),
    };
    #[cfg(not(feature = "librime"))]
    let decoder: Box<dyn Decoder> = Box::new(MockDecoder::default());

    let session = Box::new(AimSession {
        app_id: app_id.clone(),
        #[cfg(feature = "librime")]
        inner: Session::new_with_blacklist(decoder, app_id, blacklist),
        #[cfg(not(feature = "librime"))]
        inner: Session::new(decoder, app_id),
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
    i32::from(mixer::withdraw_selection(&unsafe { &*session }.app_id, &raw, &text))
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
/// C3: letters changed to reach the first candidate and then each alternative of
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
        .map(u8::to_string)
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
/// C3: puts one of the current alternatives first for this input (0 on success).
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
/// Shows one of the current alternatives in the last slot of the first page when none
/// leads (0 on success).
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
/// by [`aim_session_process`] and not already freed. Its owned pointer/count
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
    6
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

    #[test]
    fn ffi_create_process_destroy_balances_live_handles() {
        let _serialized = serialize_live_session_tests();
        let baseline = aim_live_session_count();
        let shared = b"/tmp/shared";
        let user = b"/tmp/user";
        let app = b"test.app";
        let session = unsafe {
            aim_session_create(
                shared.as_ptr(),
                shared.len(),
                user.as_ptr(),
                user.len(),
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
    }

    #[test]
    fn ffi_context_model_snapshot_is_validated_applied_and_released() {
        let _serialized = serialize_live_session_tests();
        let bytes =
            r#"{"rules":[{"pinyin":"ceshi","senses":[{"word":"测试","triggers":["运行"]}]}]}"#
                .as_bytes();
        let model = unsafe { aim_context_model_snapshot_prepare(bytes.as_ptr(), bytes.len()) };
        assert!(!model.is_null());
        assert!(unsafe { aim_context_model_snapshot_prepare(std::ptr::null(), 0) }.is_null());

        let shared = b"/tmp/shared";
        let user = b"/tmp/user";
        let app = b"test.context.ffi";
        let session = unsafe {
            aim_session_create(
                shared.as_ptr(),
                shared.len(),
                user.as_ptr(),
                user.len(),
                app.as_ptr(),
                app.len(),
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
    }
}
