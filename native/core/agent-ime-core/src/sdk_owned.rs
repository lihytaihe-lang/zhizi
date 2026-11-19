//! Copies trusted borrowed ABI outputs; the caller retains and frees originals.
use crate::{AimCandidate, AimResult};
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub text: String,
    pub comment: String,
    pub reading_key: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub status: i32,
    pub consumed: bool,
    pub mode: u8,
    pub secure_degraded: bool,
    pub marked_text: String,
    pub marked_cursor_utf8: usize,
    pub commit_text: String,
    pub candidates: Vec<Candidate>,
    pub secondary_candidates: Vec<Candidate>,
    pub highlighted_index: usize,
    pub page_no: usize,
    pub is_last_page: bool,
    pub selection_event_id: u64,
    pub core_elapsed_ns: u64,
}
unsafe fn text(pointer: *const u8, length: usize) -> Result<String, &'static str> {
    if length == 0 {
        return Ok(String::new());
    }
    if pointer.is_null() {
        return Err("missing text buffer");
    }
    // SAFETY: inherited from copy_result's readable-buffer contract; zero and
    // null were handled before forming the borrowed slice.
    let bytes = unsafe { std::slice::from_raw_parts(pointer, length) };
    std::str::from_utf8(bytes)
        .map(str::to_owned)
        .map_err(|_| "invalid UTF-8 text")
}
unsafe fn candidates(
    pointer: *const AimCandidate,
    count: usize,
) -> Result<Vec<Candidate>, &'static str> {
    if count == 0 {
        return Ok(Vec::new());
    }
    if pointer.is_null() {
        return Err("missing candidate buffer");
    }
    // SAFETY: inherited from copy_result; a nonempty array is initialized and
    // readable for the call, including each candidate's byte buffers.
    let entries = unsafe { std::slice::from_raw_parts(pointer, count) };
    entries
        .iter()
        .map(|entry| {
            // SAFETY: each candidate slice obeys the enclosing ABI buffer contract.
            unsafe {
                Ok(Candidate {
                    text: text(entry.text, entry.text_len)?,
                    comment: text(entry.comment, entry.comment_len)?,
                    reading_key: text(entry.reading_key, entry.reading_key_len)?,
                })
            }
        })
        .collect()
}
/// # Safety
/// Every nonnull nonempty native pointer must reference initialized readable
/// memory for its declared count/length until this call returns. Null+nonzero
/// and malformed UTF-8 are rejected. This function neither retains nor frees it.
pub(super) unsafe fn copy_result(raw: &AimResult) -> Result<Outcome, &'static str> {
    // SAFETY: the caller guarantees all native slices remain valid throughout
    // these checked, owned copies. Empty slices never borrow null pointers.
    unsafe {
        Ok(Outcome {
            status: raw.status,
            consumed: raw.consumed != 0,
            mode: raw.mode,
            secure_degraded: raw.secure_degraded != 0,
            marked_text: text(raw.marked_text, raw.marked_text_len)?,
            marked_cursor_utf8: raw.marked_cursor_utf8,
            commit_text: text(raw.commit_text, raw.commit_text_len)?,
            candidates: candidates(raw.candidates, raw.candidate_count)?,
            secondary_candidates: candidates(
                raw.secondary_candidates,
                raw.secondary_candidate_count,
            )?,
            highlighted_index: raw.highlighted_index,
            page_no: raw.page_no,
            is_last_page: raw.is_last_page != 0,
            selection_event_id: raw.selection_event_id,
            core_elapsed_ns: raw.core_elapsed_ns,
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn empty_native_buffers_are_valid_without_null_slices() {
        // SAFETY: all default buffers have zero length.
        let out = unsafe { copy_result(&AimResult::default()) }.unwrap();
        assert!(
            out.marked_text.is_empty()
                && out.candidates.is_empty()
                && out.secondary_candidates.is_empty()
        );
    }
    #[test]
    fn unicode_and_both_pages_are_owned_and_keep_scalar_fields() {
        let mut bytes = "你好".as_bytes().to_vec();
        let mut reading = b"ni hao".to_vec();
        let mut comment = "词".as_bytes().to_vec();
        let mut entries = [AimCandidate {
            text: bytes.as_mut_ptr(),
            text_len: bytes.len(),
            comment: comment.as_mut_ptr(),
            comment_len: comment.len(),
            reading_key: reading.as_mut_ptr(),
            reading_key_len: reading.len(),
        }];
        let raw = AimResult {
            status: -10,
            consumed: 1,
            mode: 1,
            secure_degraded: 1,
            marked_text: bytes.as_mut_ptr(),
            marked_text_len: bytes.len(),
            marked_cursor_utf8: bytes.len(),
            commit_text: bytes.as_mut_ptr(),
            commit_text_len: bytes.len(),
            candidates: entries.as_mut_ptr(),
            candidate_count: 1,
            secondary_candidates: entries.as_mut_ptr(),
            secondary_candidate_count: 1,
            highlighted_index: 2,
            page_no: 3,
            is_last_page: 1,
            selection_event_id: 9,
            core_elapsed_ns: 11,
            ..AimResult::default()
        };
        // SAFETY: all buffers are stack-owned here and remain alive until copy completes.
        let out = unsafe { copy_result(&raw) }.unwrap();
        bytes.fill(0);
        reading.fill(0);
        comment.fill(0);
        assert_eq!(out.marked_text, "你好");
        assert_eq!(out.commit_text, "你好");
        assert_eq!(
            out.candidates[0],
            Candidate {
                text: "你好".into(),
                comment: "词".into(),
                reading_key: "ni hao".into()
            }
        );
        assert_eq!(out.candidates, out.secondary_candidates);
        assert_eq!(
            (
                out.status,
                out.mode,
                out.marked_cursor_utf8,
                out.highlighted_index,
                out.page_no,
                out.selection_event_id,
                out.core_elapsed_ns
            ),
            (-10, 1, 6, 2, 3, 9, 11)
        );
        assert!(out.consumed && out.secure_degraded && out.is_last_page);
        // No native free: these test buffers were never native-owned allocations.
    }
    #[test]
    fn null_nonempty_text_or_candidates_are_rejected() {
        for raw in [
            AimResult {
                marked_text_len: 1,
                ..AimResult::default()
            },
            AimResult {
                candidate_count: 1,
                ..AimResult::default()
            },
        ] {
            // SAFETY: null+nonzero is a supported rejection; no such pointer is dereferenced.
            assert!(unsafe { copy_result(&raw) }.is_err());
        }
    }
    #[test]
    fn invalid_utf8_is_rejected_without_returning_private_bytes() {
        let mut bytes = [0xff];
        let raw = AimResult {
            marked_text: bytes.as_mut_ptr(),
            marked_text_len: 1,
            ..AimResult::default()
        };
        // SAFETY: the one-byte buffer is initialized and live; UTF-8 is validated.
        assert_eq!(unsafe { copy_result(&raw) }, Err("invalid UTF-8 text"));
    }
}
