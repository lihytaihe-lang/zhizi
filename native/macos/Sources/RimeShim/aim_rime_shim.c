#include "aim_rime_shim.h"

#ifdef _WIN32
// Windows (the 知字 Windows host builds this file with MSVC): map the few POSIX
// facilities used here. The UserDB test hooks exist only in the macOS harness, so
// they are never looked up.
#include <windows.h>
#include <limits.h>
#include <stdint.h>
#include <rime_api.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#ifndef PATH_MAX
#define PATH_MAX 4096
#endif
typedef SRWLOCK pthread_mutex_t;
#define PTHREAD_MUTEX_INITIALIZER SRWLOCK_INIT
#define pthread_mutex_lock(mutex) AcquireSRWLockExclusive(mutex)
#define pthread_mutex_unlock(mutex) ReleaseSRWLockExclusive(mutex)
#define RTLD_DEFAULT NULL
#define dlsym(handle, name) NULL
static const char *getprogname(void) { return NULL; }
#else
#include <dlfcn.h>
#include <fcntl.h>
#include <pthread.h>
#include <limits.h>
#include <stdint.h>
#include <rime_api.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#endif

static pthread_mutex_t g_rime_mutex = PTHREAD_MUTEX_INITIALIZER;
static RimeApi *g_api = NULL;
static int g_initialized = 0;
static char *g_shared_dir = NULL;

// librime's public levers module owns UserDB text export/import. The packaged
// SDK does not install rime_levers_api.h, so keep the exact stable prefix of
// the 1.16.1 public ABI here instead of reaching into LevelDB directly.
typedef struct {
  int data_size;
  void *custom_settings_init;
  void *custom_settings_destroy;
  void *load_settings;
  void *save_settings;
  void *customize_bool;
  void *customize_int;
  void *customize_double;
  void *customize_string;
  void *is_first_run;
  void *settings_is_modified;
  void *settings_get_config;
  void *switcher_settings_init;
  void *get_available_schema_list;
  void *get_selected_schema_list;
  void *schema_list_destroy;
  void *get_schema_id;
  void *get_schema_name;
  void *get_schema_version;
  void *get_schema_author;
  void *get_schema_description;
  void *get_schema_file_path;
  void *select_schemas;
  void *get_hotkeys;
  void *set_hotkeys;
  void *user_dict_iterator_init;
  void *user_dict_iterator_destroy;
  void *next_user_dict;
  int (*backup_user_dict)(const char *dict_name);
  int (*restore_user_dict)(const char *snapshot_file);
  int (*export_user_dict)(const char *dict_name, const char *text_file);
  int (*import_user_dict)(const char *dict_name, const char *text_file);
  void *customize_item;
} AimRimeLeversApi;

typedef void (*AimRimeAfterSelectHook)(uintptr_t session, int selected);
typedef void (*AimRimeBeforeSelectHook)(uintptr_t session);
typedef void (*AimRimeSelectionProbeHook)(uintptr_t session, int reinforce,
                                          int requires_regular_selection);

static AimRimeString copy_string(const char *value) {
  AimRimeString result = {0};
  if (value == NULL) return result;
  result.len = strlen(value);
  if (result.len == 0) return result;
  result.data = malloc(result.len);
  if (result.data != NULL) memcpy(result.data, value, result.len);
  else result.len = 0;
  return result;
}

static void take_commit(uintptr_t session, AimRimeString *commit) {
  if (commit == NULL) return;
  *commit = (AimRimeString){0};
  RIME_STRUCT(RimeCommit, value);
  if (g_api->get_commit(session, &value)) {
    *commit = copy_string(value.text);
    g_api->free_commit(&value);
  }
}

int aim_rime_initialize(const char *shared_dir, const char *user_dir) {
  pthread_mutex_lock(&g_rime_mutex);
  if (!g_initialized) {
    g_api = rime_get_api();
    if (g_api == NULL) {
      pthread_mutex_unlock(&g_rime_mutex);
      return 0;
    }
    RIME_STRUCT(RimeTraits, traits);
    traits.shared_data_dir = shared_dir;
    traits.user_data_dir = user_dir;
    traits.distribution_name = "Agent Input Method";
    traits.distribution_code_name = "agent_input_method";
    traits.distribution_version = "0.3.0";
    traits.app_name = "rime.agent_input_method";
    traits.min_log_level = 2;
    traits.log_dir = "";
    g_api->setup(&traits);
    g_api->initialize(&traits);
    g_shared_dir = strdup(shared_dir);
    g_api->start_maintenance(True);
    g_api->join_maintenance_thread();
    g_initialized = 1;
  }
  pthread_mutex_unlock(&g_rime_mutex);
  return 1;
}

uintptr_t aim_rime_session_create(void) {
  if (!g_initialized || g_api == NULL) return 0;
  RimeSessionId session = g_api->create_session();
  if (session != 0) g_api->select_schema(session, "agent_pinyin");
  return session;
}

void aim_rime_session_destroy(uintptr_t session) {
  if (g_api != NULL && session != 0) g_api->destroy_session(session);
}

int aim_rime_process_key(uintptr_t session, int keycode, int mask, AimRimeString *commit) {
  if (g_api == NULL || session == 0) return -1;
  int consumed = g_api->process_key(session, keycode, mask);
  take_commit(session, commit);
  return consumed;
}

int aim_rime_before_select_candidate(uintptr_t session) {
  if (g_api == NULL || session == 0) return 0;
  const char *program = getprogname();
  if (program != NULL && strcmp(program, "AgentIMEUserDBHarness") == 0) {
    AimRimeBeforeSelectHook hook =
        (AimRimeBeforeSelectHook)dlsym(
            RTLD_DEFAULT, "aim_rime_test_after_claim_before_select");
    if (hook != NULL) hook(session);
  }
  return 1;
}

static int aim_string_ends_with(const char *value, const char *suffix) {
  if (value == NULL || suffix == NULL) return 0;
  size_t value_len = strlen(value);
  size_t suffix_len = strlen(suffix);
  return value_len >= suffix_len &&
         memcmp(value + value_len - suffix_len, suffix, suffix_len) == 0;
}

static const char *aim_candidate_at(uintptr_t session, size_t index, int global_index,
                                    RimeContext *context,
                                    RimeCandidateListIterator *iterator) {
  if (!global_index) {
    return index < (size_t)context->menu.num_candidates
               ? context->menu.candidates[index].text
               : NULL;
  }
  if (!g_api->candidate_list_from_index(session, iterator, (int)index) ||
      !g_api->candidate_list_next(iterator)) {
    return NULL;
  }
  return iterator->candidate.text;
}

int aim_rime_candidate_requires_regular_selection(uintptr_t session, size_t index,
                                                  int global_index) {
  if (g_api == NULL || session == 0) return 1;
  RIME_STRUCT(RimeContext, context);
  if (!g_api->get_context(session, &context)) return 1;
  RimeCandidateListIterator iterator = {0};
  const char *candidate = aim_candidate_at(session, index, global_index, &context, &iterator);
  const char *preview = context.commit_text_preview;
  // A candidate shorter than the current full commit preview either confirms
  // only one segment or must preserve an already-confirmed prefix. Equal-size
  // homophones can still be returned directly on a duplicate/unavailable
  // delivery without learning, even when they are not highlighted.
  int required = candidate == NULL || preview == NULL ||
                 strlen(candidate) < strlen(preview);
  if (iterator.ptr != NULL) g_api->candidate_list_end(&iterator);
  g_api->free_context(&context);
  return required;
}

void aim_rime_record_selection_probe(uintptr_t session, int reinforce,
                                     int requires_regular_selection) {
  const char *program = getprogname();
  if (program == NULL || strcmp(program, "AgentIMEUserDBHarness") != 0) return;
  AimRimeSelectionProbeHook hook =
      (AimRimeSelectionProbeHook)dlsym(RTLD_DEFAULT,
                                       "aim_rime_test_record_selection_probe");
  if (hook != NULL) hook(session, reinforce, requires_regular_selection);
}

int aim_rime_select_candidate(uintptr_t session, size_t index, AimRimeString *commit) {
  if (g_api == NULL || session == 0) return 0;
  int selected = g_api->select_candidate_on_current_page(session, index);
  // No product target exports this symbol. The process-test executable does,
  // and both the executable-name gate and dynamic lookup keep production
  // input-method processes entirely outside the test-hook path.
  const char *program = getprogname();
  if (program != NULL && strcmp(program, "AgentIMEUserDBHarness") == 0) {
    AimRimeAfterSelectHook hook =
        (AimRimeAfterSelectHook)dlsym(RTLD_DEFAULT,
                                      "aim_rime_test_after_select_return");
    if (hook != NULL) {
      hook(session, selected);
    }
  }
  take_commit(session, commit);
  return selected;
}

int aim_rime_select_candidate_global(uintptr_t session, size_t index,
                                     AimRimeString *commit) {
  if (g_api == NULL || session == 0) return 0;
  int selected = g_api->select_candidate(session, index);
  take_commit(session, commit);
  return selected;
}

int aim_rime_commit_candidate_without_learning(uintptr_t session, size_t index,
                                               AimRimeString *commit) {
  if (g_api == NULL || session == 0 || commit == NULL) return 0;
  *commit = (AimRimeString){0};
  RIME_STRUCT(RimeContext, context);
  if (!g_api->get_context(session, &context)) return 0;
  int available = index < (size_t)context.menu.num_candidates;
  const char *candidate = available ? context.menu.candidates[index].text : NULL;
  const char *preview = context.commit_text_preview;
  // A confirmed prefix is safe to preserve only when the requested candidate
  // is the suffix represented by the preview. A candidate that is merely a
  // prefix would confirm one segment and must go through regular selection.
  int has_confirmed_prefix = available && preview != NULL && candidate != NULL &&
                             strlen(candidate) < strlen(preview) &&
                             aim_string_ends_with(preview, candidate);
  int requires_segment_selection = available && preview != NULL && candidate != NULL &&
                                   strlen(candidate) < strlen(preview) &&
                                   !has_confirmed_prefix;
  if (available && !requires_segment_selection) {
    *commit = copy_string(has_confirmed_prefix ? preview : candidate);
  }
  g_api->free_context(&context);
  if (!available || requires_segment_selection) return 0;
  // Clearing composition returns the candidate text to the host without
  // traversing any librime selection/learning API.
  g_api->clear_composition(session);
  return 1;
}

int aim_rime_commit_candidate_without_learning_global(uintptr_t session, size_t index,
                                                      AimRimeString *commit) {
  if (g_api == NULL || session == 0 || commit == NULL) return 0;
  *commit = (AimRimeString){0};
  RimeCandidateListIterator iterator = {0};
  int available = g_api->candidate_list_from_index(session, &iterator, (int)index) &&
                  g_api->candidate_list_next(&iterator);
  RIME_STRUCT(RimeContext, context);
  int has_context = g_api->get_context(session, &context);
  const char *candidate = available ? iterator.candidate.text : NULL;
  const char *preview = has_context ? context.commit_text_preview : NULL;
  int has_confirmed_prefix = available && preview != NULL && candidate != NULL &&
                             strlen(candidate) < strlen(preview) &&
                             aim_string_ends_with(preview, candidate);
  int requires_segment_selection = available && preview != NULL && candidate != NULL &&
                                   strlen(candidate) < strlen(preview) &&
                                   !has_confirmed_prefix;
  if (available && !requires_segment_selection) {
    *commit = copy_string(has_confirmed_prefix ? preview : candidate);
  }
  if (has_context) g_api->free_context(&context);
  if (iterator.ptr != NULL) g_api->candidate_list_end(&iterator);
  if (!available || requires_segment_selection) return 0;
  g_api->clear_composition(session);
  return 1;
}

int aim_rime_commit_preview_without_learning(uintptr_t session, size_t index,
                                             AimRimeString *commit) {
  if (g_api == NULL || session == 0 || commit == NULL) return 0;
  *commit = (AimRimeString){0};
  RIME_STRUCT(RimeContext, context);
  if (!g_api->get_context(session, &context)) return 0;
  int available = index < (size_t)context.menu.num_candidates;
  const char *candidate = available ? context.menu.candidates[index].text : NULL;
  const char *preview = context.commit_text_preview;
  const char *complete = preview != NULL && preview[0] != '\0' ? preview : candidate;
  if (available && complete != NULL) *commit = copy_string(complete);
  g_api->free_context(&context);
  if (!available || complete == NULL) return 0;
  // RequiredSegmentCommit is a host commit action, not segment confirmation:
  // return the full preview and clear composition without traversing any
  // librime selection/learning API.
  g_api->clear_composition(session);
  return 1;
}

int aim_rime_commit_preview_without_learning_global(uintptr_t session, size_t index,
                                                    AimRimeString *commit) {
  if (g_api == NULL || session == 0 || commit == NULL) return 0;
  *commit = (AimRimeString){0};
  RimeCandidateListIterator iterator = {0};
  int available = g_api->candidate_list_from_index(session, &iterator, (int)index) &&
                  g_api->candidate_list_next(&iterator);
  RIME_STRUCT(RimeContext, context);
  int has_context = g_api->get_context(session, &context);
  const char *candidate = available ? iterator.candidate.text : NULL;
  const char *preview = has_context ? context.commit_text_preview : NULL;
  const char *complete = preview != NULL && preview[0] != '\0' ? preview : candidate;
  if (available && complete != NULL) *commit = copy_string(complete);
  if (has_context) g_api->free_context(&context);
  if (iterator.ptr != NULL) g_api->candidate_list_end(&iterator);
  if (!available || complete == NULL) return 0;
  g_api->clear_composition(session);
  return 1;
}

int aim_rime_delete_candidate(uintptr_t session, size_t index) {
  if (g_api == NULL || session == 0) return 0;
  return g_api->delete_candidate_on_current_page(session, index);
}

int aim_rime_delete_candidate_global(uintptr_t session, size_t index) {
  if (g_api == NULL || session == 0) return 0;
  return g_api->delete_candidate(session, index);
}

void aim_rime_clear(uintptr_t session) {
  if (g_api != NULL && session != 0) g_api->clear_composition(session);
}

int aim_rime_snapshot(uintptr_t session, size_t maximum_candidates,
                      AimRimeSnapshot *snapshot) {
  if (g_api == NULL || session == 0 || snapshot == NULL) return 0;
  *snapshot = (AimRimeSnapshot){0};
  const char *raw = g_api->get_input(session);
  snapshot->raw_input = copy_string(raw);
  RIME_STRUCT(RimeContext, context);
  if (!g_api->get_context(session, &context)) return 1;
  snapshot->preedit = copy_string(context.composition.preedit);
  snapshot->cursor_utf8 = (size_t)context.composition.cursor_pos;
  snapshot->highlighted_index = (size_t)context.menu.highlighted_candidate_index;
  snapshot->page_no = (size_t)context.menu.page_no;
  snapshot->is_last_page = context.menu.is_last_page;
  if (maximum_candidates > 0) {
    size_t capacity = maximum_candidates == SIZE_MAX ? 64 : maximum_candidates;
    snapshot->candidates = calloc(capacity, sizeof(AimRimeCandidate));
    if (snapshot->candidates != NULL) {
      RimeCandidateListIterator iterator = {0};
      int complete = 1;
      if (g_api->candidate_list_begin(session, &iterator)) {
        while ((maximum_candidates == SIZE_MAX ||
                snapshot->candidate_count < maximum_candidates) &&
               g_api->candidate_list_next(&iterator)) {
          if (snapshot->candidate_count == capacity) {
            if (capacity > SIZE_MAX / 2) {
              complete = 0;
              break;
            }
            size_t next_capacity = capacity * 2;
            AimRimeCandidate *grown =
                realloc(snapshot->candidates, next_capacity * sizeof(AimRimeCandidate));
            if (grown == NULL) {
              complete = 0;
              break;
            }
            memset(grown + capacity, 0,
                   (next_capacity - capacity) * sizeof(AimRimeCandidate));
            snapshot->candidates = grown;
            capacity = next_capacity;
          }
          size_t index = snapshot->candidate_count++;
          snapshot->candidates[index].text = copy_string(iterator.candidate.text);
          snapshot->candidates[index].comment = copy_string(iterator.candidate.comment);
        }
        snapshot->is_last_page =
            complete &&
            (maximum_candidates == SIZE_MAX ||
             snapshot->candidate_count < maximum_candidates ||
             !g_api->candidate_list_next(&iterator));
        g_api->candidate_list_end(&iterator);
      }
    }
    snapshot->highlighted_index = 0;
    snapshot->page_no = 0;
  } else if (context.menu.num_candidates > 0) {
    snapshot->candidate_count = (size_t)context.menu.num_candidates;
    snapshot->candidates = calloc(snapshot->candidate_count, sizeof(AimRimeCandidate));
    if (snapshot->candidates != NULL) {
      for (size_t index = 0; index < snapshot->candidate_count; ++index) {
        snapshot->candidates[index].text = copy_string(context.menu.candidates[index].text);
        snapshot->candidates[index].comment = copy_string(context.menu.candidates[index].comment);
      }
    } else {
      snapshot->candidate_count = 0;
    }
  }
  g_api->free_context(&context);
  return 1;
}

void aim_rime_string_free(AimRimeString *string) {
  if (string == NULL) return;
  free(string->data);
  *string = (AimRimeString){0};
}

void aim_rime_snapshot_free(AimRimeSnapshot *snapshot) {
  if (snapshot == NULL) return;
  aim_rime_string_free(&snapshot->raw_input);
  aim_rime_string_free(&snapshot->preedit);
  for (size_t index = 0; index < snapshot->candidate_count; ++index) {
    aim_rime_string_free(&snapshot->candidates[index].text);
    aim_rime_string_free(&snapshot->candidates[index].comment);
  }
  free(snapshot->candidates);
  *snapshot = (AimRimeSnapshot){0};
}

static AimRimeLeversApi *aim_rime_levers_api(void) {
  if (g_api == NULL || g_api->find_module == NULL) return NULL;
  RimeModule *module = g_api->find_module("levers");
  if (module == NULL || module->get_api == NULL) return NULL;
  return (AimRimeLeversApi *)module->get_api();
}

int aim_rime_export_user_dict(const char *dict_name, const char *text_file) {
  AimRimeLeversApi *levers = aim_rime_levers_api();
  if (levers == NULL || levers->export_user_dict == NULL || dict_name == NULL ||
      text_file == NULL) return -1;
  return levers->export_user_dict(dict_name, text_file);
}

int aim_rime_import_user_dict(const char *dict_name, const char *text_file) {
  AimRimeLeversApi *levers = aim_rime_levers_api();
  if (levers == NULL || levers->import_user_dict == NULL || dict_name == NULL ||
      text_file == NULL) return -1;
  return levers->import_user_dict(dict_name, text_file);
}

#ifdef _WIN32
static int copy_file(const char *source, const char *destination) {
  wchar_t wide_source[PATH_MAX];
  wchar_t wide_destination[PATH_MAX];
  if (MultiByteToWideChar(CP_UTF8, 0, source, -1, wide_source, PATH_MAX) == 0 ||
      MultiByteToWideChar(CP_UTF8, 0, destination, -1, wide_destination, PATH_MAX) == 0)
    return 0;
  return CopyFileW(wide_source, wide_destination, TRUE) ? 1 : 0;
}
#else
static int copy_file(const char *source, const char *destination) {
  int input_fd = open(source, O_RDONLY | O_NOFOLLOW);
  if (input_fd < 0) return 0;
  int output_fd = open(destination, O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW, 0600);
  if (output_fd < 0) {
    close(input_fd);
    return 0;
  }
  FILE *input = fdopen(input_fd, "rb");
  FILE *output = fdopen(output_fd, "wb");
  if (input == NULL || output == NULL) {
    if (input != NULL) fclose(input); else close(input_fd);
    if (output != NULL) fclose(output); else close(output_fd);
    unlink(destination);
    return 0;
  }
  char buffer[16384];
  size_t count;
  int success = 1;
  while ((count = fread(buffer, 1, sizeof(buffer), input)) > 0) {
    if (fwrite(buffer, 1, count, output) != count) {
      success = 0;
      break;
    }
  }
  if (ferror(input) || fflush(output) != 0) success = 0;
  if (fclose(output) != 0) success = 0;
  fclose(input);
  if (!success) unlink(destination);
  return success;
}
#endif

int aim_rime_backup_user_dict(const char *dict_name, const char *snapshot_file) {
  AimRimeLeversApi *levers = aim_rime_levers_api();
  if (levers == NULL || levers->backup_user_dict == NULL || dict_name == NULL ||
      snapshot_file == NULL || g_api == NULL || g_api->get_user_data_sync_dir == NULL)
    return 0;
  if (!levers->backup_user_dict(dict_name)) return 0;
  char directory[PATH_MAX] = {0};
  g_api->get_user_data_sync_dir(directory, sizeof(directory));
  if (directory[0] == '\0') return 0;
  char source[PATH_MAX] = {0};
  int length = snprintf(source, sizeof(source), "%s/%s.userdb.txt", directory,
                        dict_name);
  if (length <= 0 || (size_t)length >= sizeof(source)) return 0;
  return copy_file(source, snapshot_file);
}

int aim_rime_restore_user_dict(const char *snapshot_file) {
  AimRimeLeversApi *levers = aim_rime_levers_api();
  if (levers == NULL || levers->restore_user_dict == NULL || snapshot_file == NULL)
    return 0;
  return levers->restore_user_dict(snapshot_file);
}

int aim_rime_redeploy(void) {
  if (g_api == NULL || !g_initialized) return 0;
  if (g_shared_dir != NULL) {
    size_t length = strlen(g_shared_dir) + strlen("/agent_pinyin.schema.yaml") + 1;
    char *schema_path = malloc(length);
    if (schema_path == NULL) return 0;
    snprintf(schema_path, length, "%s/agent_pinyin.schema.yaml", g_shared_dir);
    int deployed = g_api->deploy_schema(schema_path);
    free(schema_path);
    if (!deployed) return 0;
  }
  g_api->start_maintenance(True);
  g_api->join_maintenance_thread();
  return 1;
}
