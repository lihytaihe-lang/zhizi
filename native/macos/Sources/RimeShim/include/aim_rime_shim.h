#ifndef AIM_RIME_SHIM_H
#define AIM_RIME_SHIM_H

#include <stddef.h>
#include <stdint.h>

typedef struct {
  char *data;
  size_t len;
} AimRimeString;

typedef struct {
  AimRimeString text;
  AimRimeString comment;
} AimRimeCandidate;

typedef struct {
  AimRimeString raw_input;
  AimRimeString preedit;
  size_t cursor_utf8;
  AimRimeCandidate *candidates;
  size_t candidate_count;
  size_t highlighted_index;
  size_t page_no;
  int is_last_page;
} AimRimeSnapshot;

int aim_rime_initialize(const char *shared_dir, const char *user_dir);
uintptr_t aim_rime_session_create(void);
void aim_rime_session_destroy(uintptr_t session);
int aim_rime_process_key(uintptr_t session, int keycode, int mask, AimRimeString *commit);
int aim_rime_before_select_candidate(uintptr_t session);
int aim_rime_candidate_requires_regular_selection(uintptr_t session, size_t index,
                                                  int global_index);
void aim_rime_record_selection_probe(uintptr_t session, int reinforce,
                                     int requires_regular_selection);
int aim_rime_select_candidate(uintptr_t session, size_t index, AimRimeString *commit);
int aim_rime_select_candidate_global(uintptr_t session, size_t index,
                                     AimRimeString *commit);
int aim_rime_commit_candidate_without_learning(uintptr_t session, size_t index,
                                               AimRimeString *commit);
int aim_rime_commit_candidate_without_learning_global(uintptr_t session, size_t index,
                                                      AimRimeString *commit);
int aim_rime_commit_preview_without_learning(uintptr_t session, size_t index,
                                             AimRimeString *commit);
int aim_rime_commit_preview_without_learning_global(uintptr_t session, size_t index,
                                                    AimRimeString *commit);
int aim_rime_delete_candidate(uintptr_t session, size_t index);
int aim_rime_delete_candidate_global(uintptr_t session, size_t index);
void aim_rime_clear(uintptr_t session);
int aim_rime_snapshot(uintptr_t session, size_t maximum_candidates,
                      AimRimeSnapshot *snapshot);
void aim_rime_string_free(AimRimeString *string);
void aim_rime_snapshot_free(AimRimeSnapshot *snapshot);
int aim_rime_redeploy(void);
int aim_rime_export_user_dict(const char *dict_name, const char *text_file);
int aim_rime_import_user_dict(const char *dict_name, const char *text_file);
int aim_rime_backup_user_dict(const char *dict_name, const char *snapshot_file);
int aim_rime_restore_user_dict(const char *snapshot_file);

#endif
