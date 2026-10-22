#include "AgentIMECoreBridge.h"

uint32_t agent_ime_core_bridge_compile_check(void) {
  return aim_core_abi_version();
}
