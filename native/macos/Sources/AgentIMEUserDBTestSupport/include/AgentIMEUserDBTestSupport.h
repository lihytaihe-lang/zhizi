#ifndef AGENT_IME_USERDB_TEST_SUPPORT_H
#define AGENT_IME_USERDB_TEST_SUPPORT_H

#include <stdint.h>

int aim_userdb_test_sync_user_data(void);
int aim_userdb_test_control_reset(const char *path);
int aim_userdb_test_control_read(const char *path,
                                 uint64_t *select_calls,
                                 uint64_t *hook_state,
                                 int64_t *process_id,
                                 int64_t *selected_result);
int aim_userdb_test_stop_after_sync(const char *path);
void aim_userdb_test_selection_probe_reset(void);
int aim_userdb_test_selection_probe_read(uint64_t *count, int *reinforce,
                                         int *requires_regular_selection);

#endif
