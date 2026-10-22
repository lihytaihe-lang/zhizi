#include "AgentIMEUserDBTestSupport.h"

#include <fcntl.h>
#include <signal.h>
#include <stdatomic.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <unistd.h>
#include <rime_api.h>

#define AIM_USERDB_CONTROL_MAGIC UINT64_C(0x41494d5544423031)

typedef struct {
  uint64_t magic;
  _Atomic uint64_t select_calls;
  _Atomic uint64_t hook_state;
  _Atomic int64_t process_id;
  _Atomic int64_t selected_result;
  uint8_t reserved[4096 - 40];
} AimUserDBControlPage;

static AimUserDBControlPage *map_control(const char *path, int create,
                                         int *descriptor) {
  if (path == NULL || path[0] == '\0' || descriptor == NULL) return NULL;
  int flags = create ? (O_CREAT | O_RDWR) : O_RDONLY;
  int fd = open(path, flags, S_IRUSR | S_IWUSR);
  if (fd < 0) return NULL;
  if (create && ftruncate(fd, sizeof(AimUserDBControlPage)) != 0) {
    close(fd);
    return NULL;
  }
  int protection = create ? (PROT_READ | PROT_WRITE) : PROT_READ;
  void *mapping = mmap(NULL, sizeof(AimUserDBControlPage), protection,
                       MAP_SHARED, fd, 0);
  if (mapping == MAP_FAILED) {
    close(fd);
    return NULL;
  }
  *descriptor = fd;
  return mapping;
}

static _Atomic uint64_t g_selection_probe_count = 0;
static _Atomic int g_selection_probe_reinforce = -1;
static _Atomic int g_selection_probe_requires_regular_selection = -1;

void aim_userdb_test_selection_probe_reset(void) {
  atomic_store(&g_selection_probe_count, 0);
  atomic_store(&g_selection_probe_reinforce, -1);
  atomic_store(&g_selection_probe_requires_regular_selection, -1);
}

int aim_userdb_test_selection_probe_read(uint64_t *count, int *reinforce,
                                         int *requires_regular_selection) {
  if (count != NULL) *count = atomic_load(&g_selection_probe_count);
  if (reinforce != NULL) *reinforce = atomic_load(&g_selection_probe_reinforce);
  if (requires_regular_selection != NULL) {
    *requires_regular_selection =
        atomic_load(&g_selection_probe_requires_regular_selection);
  }
  return atomic_load(&g_selection_probe_count) > 0;
}

__attribute__((used, visibility("default")))
void aim_rime_test_record_selection_probe(uintptr_t session, int reinforce,
                                          int requires_regular_selection) {
  (void)session;
  atomic_fetch_add(&g_selection_probe_count, 1);
  atomic_store(&g_selection_probe_reinforce, reinforce);
  atomic_store(&g_selection_probe_requires_regular_selection,
               requires_regular_selection);
}

static void unmap_control(AimUserDBControlPage *control, int descriptor) {
  if (control != NULL) munmap(control, sizeof(*control));
  if (descriptor >= 0) close(descriptor);
}

int aim_userdb_test_sync_user_data(void) {
  RimeApi *api = rime_get_api();
  if (api == NULL || api->sync_user_data == NULL) return 0;
  int started = api->sync_user_data();
  // sync_user_data only queues UserDictSync on librime's maintenance thread.
  // Returning before it finishes let the harness tear librime down under a
  // running sync (SIGSEGV in UserDictManager::GetUserDictList).
  if (started && api->join_maintenance_thread != NULL) api->join_maintenance_thread();
  return started;
}

int aim_userdb_test_control_reset(const char *path) {
  int descriptor = -1;
  AimUserDBControlPage *control = map_control(path, 1, &descriptor);
  if (control == NULL) return 0;
  memset(control, 0, sizeof(*control));
  control->magic = AIM_USERDB_CONTROL_MAGIC;
  int result = msync(control, sizeof(*control), MS_SYNC) == 0;
  unmap_control(control, descriptor);
  return result;
}

int aim_userdb_test_control_read(const char *path,
                                 uint64_t *select_calls,
                                 uint64_t *hook_state,
                                 int64_t *process_id,
                                 int64_t *selected_result) {
  int descriptor = -1;
  AimUserDBControlPage *control = map_control(path, 0, &descriptor);
  if (control == NULL || control->magic != AIM_USERDB_CONTROL_MAGIC) {
    unmap_control(control, descriptor);
    return 0;
  }
  if (select_calls != NULL) *select_calls = atomic_load(&control->select_calls);
  if (hook_state != NULL) *hook_state = atomic_load(&control->hook_state);
  if (process_id != NULL) *process_id = atomic_load(&control->process_id);
  if (selected_result != NULL) *selected_result = atomic_load(&control->selected_result);
  unmap_control(control, descriptor);
  return 1;
}

int aim_userdb_test_stop_after_sync(const char *path) {
  int descriptor = -1;
  AimUserDBControlPage *control = map_control(path, 1, &descriptor);
  if (control == NULL) return 0;
  if (control->magic != AIM_USERDB_CONTROL_MAGIC) {
    memset(control, 0, sizeof(*control));
    control->magic = AIM_USERDB_CONTROL_MAGIC;
  }
  atomic_store(&control->process_id, (int64_t)getpid());
  atomic_store(&control->hook_state, UINT64_C(3));
  if (msync(control, sizeof(*control), MS_SYNC) != 0) {
    unmap_control(control, descriptor);
    return 0;
  }
  raise(SIGSTOP);
  atomic_store(&control->hook_state, UINT64_C(4));
  msync(control, sizeof(*control), MS_SYNC);
  unmap_control(control, descriptor);
  return 1;
}

__attribute__((used, visibility("default")))
void aim_rime_test_after_claim_before_select(uintptr_t session) {
  (void)session;
  const char *path = getenv("AIM_USERDB_TEST_PRESELECT_CONTROL_PATH");
  if (path == NULL || path[0] == '\0') return;
  int descriptor = -1;
  AimUserDBControlPage *control = map_control(path, 1, &descriptor);
  if (control == NULL) return;
  if (control->magic != AIM_USERDB_CONTROL_MAGIC) {
    memset(control, 0, sizeof(*control));
    control->magic = AIM_USERDB_CONTROL_MAGIC;
  }
  atomic_fetch_add(&control->select_calls, 1);
  atomic_store(&control->process_id, (int64_t)getpid());
  atomic_store(&control->hook_state, UINT64_C(5));
  msync(control, sizeof(*control), MS_SYNC);
  raise(SIGSTOP);
  atomic_store(&control->hook_state, UINT64_C(6));
  msync(control, sizeof(*control), MS_SYNC);
  unmap_control(control, descriptor);
}

__attribute__((used, visibility("default")))
void aim_rime_test_after_select_return(uintptr_t session, int selected) {
  (void)session;
  const char *path = getenv("AIM_USERDB_TEST_CONTROL_PATH");
  if (path == NULL || path[0] == '\0') return;
  int descriptor = -1;
  AimUserDBControlPage *control = map_control(path, 1, &descriptor);
  if (control == NULL) return;
  if (control->magic != AIM_USERDB_CONTROL_MAGIC) {
    memset(control, 0, sizeof(*control));
    control->magic = AIM_USERDB_CONTROL_MAGIC;
  }
  atomic_fetch_add(&control->select_calls, 1);
  atomic_store(&control->selected_result, selected);
  atomic_store(&control->process_id, (int64_t)getpid());
  atomic_store(&control->hook_state, UINT64_C(1));
  msync(control, sizeof(*control), MS_SYNC);
  raise(SIGSTOP);
  atomic_store(&control->hook_state, UINT64_C(2));
  msync(control, sizeof(*control), MS_SYNC);
  unmap_control(control, descriptor);
}
