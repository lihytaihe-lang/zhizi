#ifndef AGENT_IME_CORE_H
#define AGENT_IME_CORE_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* ABI generation 6; version string is static, borrowed, and never freed. */
#define AIM_CORE_ABI_VERSION 6u
const char *aim_core_version(void);

typedef struct AimSession AimSession;
typedef struct AimLexiconSnapshot AimLexiconSnapshot;
typedef struct AimContextModelSnapshot AimContextModelSnapshot;
typedef struct AimZhijuSearch AimZhijuSearch;
typedef struct AimZhijuSnapshot AimZhijuSnapshot;

typedef enum {
  AIM_KEY_TEXT = 0,
  AIM_KEY_BACKSPACE = 1,
  AIM_KEY_LEFT = 2,
  AIM_KEY_RIGHT = 3,
  AIM_KEY_SPACE = 4,
  AIM_KEY_ENTER = 5,
  AIM_KEY_ESCAPE = 6,
  AIM_KEY_PAGE_UP = 7,
  AIM_KEY_PAGE_DOWN = 8,
  AIM_KEY_UP = 9,
  AIM_KEY_DOWN = 10,
  AIM_KEY_SHIFT_TAP = 11,
  AIM_KEY_CAPS_LOCK = 12,
  AIM_KEY_FOCUS_LOST = 13,
  AIM_KEY_DEACTIVATE = 14,
  AIM_KEY_SLEEP = 15,
  AIM_KEY_RESUME = 16,
  AIM_KEY_DELETE_CANDIDATE = 17,
  AIM_KEY_BLACKLIST_CANDIDATE = 18,
  AIM_KEY_UNSUPPORTED = 19,
  AIM_KEY_LINEAR_NEXT = 20,
  AIM_KEY_LINEAR_PREVIOUS = 21
} AimKeyKind;

typedef enum {
  AIM_SECURITY_NON_SECURE = 0,
  AIM_SECURITY_SECURE = 1,
  AIM_SECURITY_UNKNOWN = 2
} AimSecurityState;

enum {
  AIM_MOD_SHIFT = 1u << 0,
  AIM_MOD_CONTROL = 1u << 1,
  AIM_MOD_OPTION = 1u << 2,
  AIM_MOD_COMMAND = 1u << 3,
  AIM_MOD_CAPS_LOCK = 1u << 4
};

typedef struct {
  uint32_t kind;
  const uint8_t *text;
  size_t text_len;
  uint32_t modifiers;
  uint32_t security_state;
  uint64_t delivery_id;
} AimKeyEvent;

typedef struct {
  uint8_t *text;
  size_t text_len;
  uint8_t *comment;
  size_t comment_len;
  uint8_t *reading_key;
  size_t reading_key_len;
} AimCandidate;

typedef struct {
  int32_t status;
  uint8_t consumed;
  uint8_t mode;
  uint8_t secure_degraded;
  uint8_t reserved;
  uint8_t *marked_text;
  size_t marked_text_len;
  size_t marked_cursor_utf8;
  uint8_t *commit_text;
  size_t commit_text_len;
  AimCandidate *candidates;
  size_t candidate_count;
  AimCandidate *secondary_candidates;
  size_t secondary_candidate_count;
  size_t highlighted_index;
  size_t page_no;
  uint8_t is_last_page;
  uint64_t selection_event_id;
  uint64_t core_elapsed_ns;
} AimResult;

/* Prepare explicit UTF-8 dictionary and private profile directories. Serialize
 * creation and keep one (shared,user) deployment while any sessions are active.
 * Each handle is sequential, non-reentrant, and released exactly once. */
AimSession *aim_session_create(const uint8_t *shared_dir,
                               size_t shared_dir_len,
                               const uint8_t *user_dir,
                               size_t user_dir_len,
                               const uint8_t *app_id,
                               size_t app_id_len);
void aim_session_destroy(AimSession *session);
int32_t aim_session_process(AimSession *session,
                            const AimKeyEvent *event,
                            AimResult *result);
/*
 * `result` is output-only and may point to uninitialized storage. After a
 * successful call, release its owned buffers with aim_result_free before the
 * storage is reused or discarded.
 */
/* Reads the complete current page without changing state; output-only result is released with aim_result_free. */
int32_t aim_session_current_page(AimSession *session, AimResult *result);
/* Returns 1 for a matching withdrawal, 0 for no match, negative on invalid arguments. */
int32_t aim_session_withdraw_selection(AimSession *session, const uint8_t *raw, size_t raw_len,
                                     const uint8_t *text, size_t text_len);
/* 1 = Zhixu, the only learning rule (sessions always learn in Zhixu); idle
 * session only. The retired 0 = Rime rule is rejected as an invalid value:
 * null=-1, invalid=-2 (including 0), composing=-10. */
int32_t aim_session_set_learning(AimSession *session, uint32_t mode);
/* Retained decoder knob: 0/1 are accepted for shell compatibility, but the inner
 * decoder is native Zhixu for the session's whole life; this call no longer changes it.
 * Null=-1, invalid=-2, composing=-10. */
int32_t aim_session_set_zhixu_whole(AimSession *session, uint32_t on);
/* Applies to newly generated menus. An already displayed digit keeps its selection. */
int32_t aim_session_set_zhiju_phrases(AimSession *session, uint8_t enabled);
/* Retired: the Rime-userdb migration is complete. Always -50 (capability
 * removed); never reads or writes the filesystem. */
int32_t aim_learning_available(const uint8_t *user, size_t user_len);
/* Retired: the Rime-userdb migration is complete. Always -50 (capability
 * removed); never reads or writes the filesystem. Symbol kept until the
 * shells drop their calls. */
int32_t aim_learning_migrate(const uint8_t *shared, size_t shared_len,
                             const uint8_t *user, size_t user_len);
int32_t aim_session_set_fuzzy(AimSession *session, uint32_t fuzzy_mask);
/* Reloads the native dictionaries so tables added to the user directory since the
 * session started (downloaded professional packs) take effect. Drops any composition;
 * call it only at a safe point. Returns 0 on success. */
int32_t aim_session_reload_dictionaries(AimSession *session);
int32_t aim_session_set_context_profile(AimSession *session,
                                        uint32_t app_class_override,
                                        uint32_t punctuation_policy);
/* Independent of apply_context_model (the latter is a context-bigram snapshot).
 * UTF-8 path to itos.json. Success=0; null session=-1; invalid path/vocab=-2.
 * A failed load preserves the previously loaded vocabulary. */
int32_t aim_session_set_zhiju_vocab(AimSession *session, const uint8_t *path, size_t path_len);
/* One synchronous call per beam step: tokens is Int32[rows,cols], cols=40,
 * BOS=1 followed by the last min(context+generated length,38) characters,
 * UNK=2, right-padded with PAD=0. Each row has the same non-padding length.
 * Fill every Float32 out[rows,vocab] with finite next-character logprobs.
 * Return 0 on success, nonzero on failure. Do not reenter the session, unwind,
 * retain pointers, or use these borrowed buffers after returning. */
typedef int32_t (*AimZhijuScorer)(void *user_data, const int32_t *tokens,
                                size_t rows, size_t cols, float *out, size_t vocab);
/* Search only: never changes the page, ranking or composition.
 * beam=0 means 8; otherwise use the requested width. At most 16 model steps.
 * budget_ms is a wall-clock budget, including core preparation. Zero performs
 * no forwards. A synchronous callback cannot be interrupted: its completed
 * expansions are retained, then the deadline is checked before another call.
 * Returns at most beam completed sentences (no unfinished text), sorted by
 * cumulative logprob plus word rewards minus initial penalties descending,
 * consumed keys descending, UTF-8 text ascending.
 * *out_len is byte capacity on entry; bytes written/required on return.
 * UTF-8 encoding, no NUL terminator, one record per line:
 *     text<TAB>score (decimal f64)<TAB>consumed ASCII keys<LF>
 * Apostrophes count as keys. Output may be empty, including on timeout.
 * 0=success (also timeout/step cap with partial completed results), -1=null
 * argument, -2=invalid size, -3=not a sentence abbreviation (also secure/ASCII
 * mode), -4=vocabulary not set, -5=callback failure/non-finite output,
 * -6=buffer too small (*out_len is required bytes), -7=no loaded dictionary.
 * On errors no bytes are written; *out_len=0 except -6. out may be NULL only
 * with zero capacity. Session access must remain exclusive throughout. */
int32_t aim_session_zhiju_search(AimSession *session, AimZhijuScorer scorer, void *user_data,
                               size_t beam, uint64_t budget_ms, uint8_t *out, size_t *out_len);
/* A request-only callback. Copy tokens before returning; predict after releasing
 * session access. The tokens/rows/cols/vocab layout matches AimZhijuScorer. */
typedef void (*AimZhijuRowsReady)(void *user_data, const int32_t *tokens,
                                size_t rows, size_t cols, size_t vocab);
/* Worker-owned search. Create loads only vocabulary; prepare builds reusable
 * dictionary caches. Neither call takes or accesses a session. Null on load error. */
AimZhijuSearch *aim_zhiju_search_create(const uint8_t *path, size_t path_len);
void aim_zhiju_search_destroy(AimZhijuSearch *search);
/* Capture under exclusive session access; only copies input/context and retains
 * an immutable dictionary Arc. No vocabulary loading, expansion or sorting.
 * 0=success; -1=null; -3=not applicable; -7=no dictionary. Clears *out on error.
 * The snapshot outlives keys, dictionary reloads and session destruction. */
int32_t aim_session_zhiju_snapshot(const AimSession *session, AimZhijuSnapshot **out);
/* Capture a preview of one lower-case ASCII append to a native first-page
 * composition at its unconfirmed end. The capture only copies state.
 * Use aim_zhiju_snapshot_lexical on a worker. Preview snapshots cannot start
 * neural search or use the normal apply APIs. -2=not lower-case ASCII;
 * other capture statuses and snapshot ownership match the normal capture. */
int32_t aim_session_zhiju_preview(const AimSession *session, uint8_t next_letter, AimZhijuSnapshot **out);
void aim_zhiju_snapshot_destroy(AimZhijuSnapshot *snapshot);
int32_t aim_zhiju_search_prepare(AimZhijuSearch *search, const AimZhijuSnapshot *snapshot);
/* Same requests, step/budget/buffer rules and encoded results as the session
 * continuation below, but operates entirely on worker-owned state. Use the same
 * search for every step and do not concurrently use either handle. No session
 * access or global runtime lock is required. A stale snapshot may finish safely;
 * the host should cancel it before starting another expansion or prediction. */
int32_t aim_zhiju_snapshot_step(AimZhijuSnapshot *snapshot, AimZhijuSearch *search,
                              AimZhijuRowsReady request, void *user_data,
                              size_t beam, uint64_t budget_ms,
                              const float *scores, size_t score_count,
                              uint8_t *out, size_t *out_len);
/* Bounded dictionary-only expansion for this snapshot, independent of GEN.
 * Output is text<TAB>full spaced reading<TAB>consumed bytes<LF> per candidate;
 * no neural scores. out_len uses the same capacity and -6 rules as above.
 * -9 means the warmed lexical index or time budget was unavailable. */
int32_t aim_zhiju_snapshot_lexical(AimZhijuSnapshot *snapshot, uint64_t budget_ms,
                                 uint8_t *out, size_t *out_len);
/* One exclusive session call: validate originating session, key/dictionary
 * generation, optionally remember/promote the best completed result, then read
 * the current page. promote=0 only reads the page (including timeout fallback).
 * -8=stale/foreign snapshot (empty result); other lead statuses match
 * aim_session_lead_with_composition and return the unchanged current page.
 * Free out with aim_result_free on every status. */
int32_t aim_session_apply_zhiju_snapshot(AimSession *session, const AimZhijuSnapshot *snapshot,
                                      int32_t promote, AimResult *out);
/* Atomically selects one full-input candidate proved by this same snapshot's
 * neural or lexical expansion. Otherwise -2 (unproved) or -8 (stale/foreign).
 * Lead statuses and AimResult ownership match aim_session_apply_zhiju_snapshot. */
int32_t aim_session_apply_zhiju_snapshot_selected(AimSession *session, const AimZhijuSnapshot *snapshot,
                                               const uint8_t *text, size_t text_length, AimResult *out);
/* After processing exactly the previewed following unmodified lower-case key,
 * apply a lexical proof from the preview. Key generation, input, context, security,
 * session, dictionary, and end caret must still agree. -2=unproved; -8=stale,
 * wrong key, wrong snapshot kind or already applied. AimResult ownership and
 * lead statuses match the normal selected apply API. */
int32_t aim_session_apply_zhiju_preview(AimSession *session, const AimZhijuSnapshot *snapshot,
                                      const uint8_t *text, size_t text_length, AimResult *out);
/* Resumable search with the same results/encoding as aim_session_zhiju_search.
 * *generation=0 starts a search with no scores. A prepared search receives a
 * nonzero identity; an already exhausted start budget leaves it zero.
 * 1=request emitted, *out_len=0; supply exactly rows*vocab finite scores on the
 * next call with that identity. Each call expands at most one completed step,
 * preserving its beam and completions; prior steps are never replayed.
 * 0=complete (possibly empty/partial). budget_ms is the REMAINING wall-clock
 * budget for this call, including core preparation; the host must subtract
 * time spent predicting outside the core. Zero applies any supplied scores and
 * returns completed results without requesting another batch. Zero with no
 * scores also finishes a pending search using its completed steps only.
 * beam is used only on start. Every key (including commit/paging/security),
 * vocabulary/lexicon replacement, or new search invalidates the old identity;
 * resuming it returns -8 without a request or output. A dictionary generation
 * change also invalidates it. Completed results can be reread with no scores,
 * including after -6 (short buffer). Other statuses match the synchronous ABI.
 * All pointers are borrowed for this call only; session access is exclusive
 * during the call, and is free between calls, including during prediction. */
int32_t aim_session_zhiju_step(AimSession *session, uint64_t *generation,
                             AimZhijuRowsReady request, void *user_data,
                             size_t beam, uint64_t budget_ms,
                             const float *scores, size_t score_count,
                             uint8_t *out, size_t *out_len);
uint32_t aim_session_context_group_mask(AimSession *session);
uint32_t aim_session_context_complete_mask(AimSession *session);
int32_t aim_session_contextual_prior_allowed(AimSession *session);
typedef struct {
  uint32_t homophone_mask;
  uint32_t complete_mask;
  int32_t contextual_prior_allowed;
} AimContextRankingInputs;
/* Reads all ranking inputs from one original page. NULL returns zeros.
 * Session access is exclusive during the call. No candidates are reordered. */
AimContextRankingInputs aim_session_context_ranking_inputs(AimSession *session);
intptr_t aim_session_peek_following_pages(AimSession *session, size_t count, uint8_t *buffer, size_t capacity);
intptr_t aim_session_composition_alternatives(AimSession *session, uint8_t *buffer, size_t capacity);
/* Texts of the current page exactly as digit keys resolve against it, tab-separated. A shell that draws a page it
 * built itself must match this. Returns bytes written, or the negated size when the buffer is too small. */
intptr_t aim_session_page_texts(AimSession *session, uint8_t *buffer, size_t capacity);
/* Process-wide correction switches; refreshes this session without clearing keys.
 * Call between model jobs and refresh each other live session before presenting it. */
int32_t aim_session_set_corrections(AimSession *session, int32_t enabled,
                                  int32_t typed_costs, int32_t multiple_readings,
                                  int32_t trailing_letter);
intptr_t aim_session_composition_edits(AimSession *session, uint8_t *buffer, size_t capacity);
/* 0 = leads; -1 invalid; -10 not a current alternative; -11 the owner's learned whole stays first; -12 something else is first;
 * -13 a dictionary entry covering all keys stays first (Zhiju results only).
 * Success means the page the digit keys resolve against really starts with the text. */
int32_t aim_session_lead_with_composition(AimSession *session, const uint8_t *text, size_t length);
/* Offers a current C3 alternative or a remembered, full-reading-validated Zhiju
 * result in slot five. Raw/full-reading blacklists and learned-word protection
 * still apply; key/page edits expire generated proofs. Never changes slots 1–4. */
int32_t aim_session_offer_composition(AimSession *session, const uint8_t *text, size_t length);
/* Read-only, bounded off-page dictionary matches for 2–3 initials. UTF-8 TSV;
 * returns bytes written or negated required size. Same buffer contract as page_texts. */
intptr_t aim_session_abbreviation_alternatives(AimSession *session, uint8_t *buffer, size_t capacity);
/* Read-only lexical forecast for one final letter. At most eight raw/text pairs,
 * separated by TAB and newline. Same buffer contract as page_texts. */
intptr_t aim_session_exact_lookahead(AimSession *session, uint8_t *buffer, size_t capacity);
/* Offer a current short dictionary alternative in slot five; never changes slots 1–4. */
int32_t aim_session_offer_abbreviation(AimSession *session, const uint8_t *text, size_t length);
int32_t aim_session_composition_guessed(AimSession *session);
int32_t aim_ranking_composition_lead(const uint8_t *texts, size_t length, const double *scores, const double *edits, size_t count, int32_t guessed);
int32_t aim_ranking_composition_offer(const uint8_t *texts, size_t length, const double *scores, const double *edits, size_t count);
int32_t aim_ranking_group_order(const uint8_t *texts, size_t length, const double *scores, const uint8_t *homophone, size_t count, size_t *order);
// has_context is 0 or 1; the host sets 1 when the preceding text contains Han.
int32_t aim_ranking_contextual_group_order(const uint8_t *texts, size_t length, const double *scores, const uint8_t *homophone, size_t count, int32_t has_context, size_t *order);
int32_t aim_ranking_load_owner_style(const uint8_t *records, size_t length);
int32_t aim_ranking_owner_keeps(const uint8_t *first, size_t first_length, const uint8_t *challenger, size_t challenger_length);
int32_t aim_session_reorder_context_candidates(AimSession *session, const size_t *order, size_t count);
int32_t aim_session_set_preceding_text(AimSession *session,
                                       const uint8_t *text,
                                       size_t text_len);
AimLexiconSnapshot *aim_lexicon_snapshot_prepare(const uint8_t *json,
                                                 size_t json_len);
void aim_lexicon_snapshot_destroy(AimLexiconSnapshot *snapshot);
int32_t aim_session_apply_lexicon(AimSession *session,
                                  const AimLexiconSnapshot *snapshot);
AimContextModelSnapshot *aim_context_model_snapshot_prepare(
    const uint8_t *model, size_t model_len);
void aim_context_model_snapshot_destroy(AimContextModelSnapshot *snapshot);
int32_t aim_session_apply_context_model(
    AimSession *session, const AimContextModelSnapshot *snapshot);
int32_t aim_session_set_typo_correction(AimSession *session, uint8_t enabled);
int32_t aim_session_set_candidate_context(AimSession *session,
                                          const uint8_t *active_project,
                                          size_t active_project_len,
                                          const uint8_t *active_packs_json,
                                          size_t active_packs_json_len,
                                          uint8_t secondary_enabled,
                                          uint8_t prediction_enabled);
int32_t aim_session_select_secondary_candidate(AimSession *session,
                                               size_t index,
                                               uint64_t delivery_id,
                                               uint32_t security_state,
                                               AimResult *result);
int32_t aim_session_set_mode(AimSession *session, uint32_t mode);
void aim_result_free(AimResult *result);
uint32_t aim_core_abi_version(void);
size_t aim_live_session_count(void);
/* Waits for the background write of the persisted Zhixu index cache: 0 when
 * settled (also when the write was dropped), 1 when timeout_ms (capped at
 * 10000) elapsed. Short-lived processes call it after their last key so a
 * cache built during the session reaches the disk before exit. */
int32_t aim_index_cache_flush(uint32_t timeout_ms);

#ifdef __cplusplus
}
#endif

#endif
