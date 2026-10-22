#ifndef AGENT_IME_CORE_H
#define AGENT_IME_CORE_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct AimSession AimSession;
typedef struct AimLexiconSnapshot AimLexiconSnapshot;
typedef struct AimContextModelSnapshot AimContextModelSnapshot;

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
  AIM_KEY_UNSUPPORTED = 18,
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
/* Returns 1 for a matching withdrawal, 0 for no match, negative on invalid arguments. */
int32_t aim_session_withdraw_selection(AimSession *session, const uint8_t *raw, size_t raw_len,
                                     const uint8_t *text, size_t text_len);
int32_t aim_session_set_fuzzy(AimSession *session, uint32_t fuzzy_mask);
int32_t aim_session_set_context_profile(AimSession *session,
                                        uint32_t app_class_override,
                                        uint32_t punctuation_policy);
uint32_t aim_session_context_group_mask(AimSession *session);
uint32_t aim_session_context_complete_mask(AimSession *session);
intptr_t aim_session_peek_following_pages(AimSession *session, size_t count, uint8_t *buffer, size_t capacity);
intptr_t aim_session_composition_alternatives(AimSession *session, uint8_t *buffer, size_t capacity);
intptr_t aim_session_composition_edits(AimSession *session, uint8_t *buffer, size_t capacity);
int32_t aim_session_lead_with_composition(AimSession *session, const uint8_t *text, size_t length);
int32_t aim_session_offer_composition(AimSession *session, const uint8_t *text, size_t length);
int32_t aim_session_composition_guessed(AimSession *session);
int32_t aim_ranking_composition_lead(const uint8_t *texts, size_t length, const double *scores, const double *edits, size_t count, int32_t guessed);
int32_t aim_ranking_composition_offer(const uint8_t *texts, size_t length, const double *scores, const double *edits, size_t count);
int32_t aim_ranking_group_order(const uint8_t *texts, size_t length, const double *scores, const uint8_t *homophone, size_t count, size_t *order);
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

#ifdef __cplusplus
}
#endif

#endif
