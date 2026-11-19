#!/usr/bin/env bash
set -euo pipefail
SDK_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SDK_TARGET="${CARGO_TARGET_DIR:-$SDK_ROOT/target/sdk-c-build}"
SDK_DEMO_OUT="$SDK_ROOT/target/sdk-demo"
mkdir -p "$SDK_TARGET" "$SDK_DEMO_OUT"
CARGO_TARGET_DIR="$SDK_TARGET" cargo rustc --manifest-path "$SDK_ROOT/Cargo.toml" \
  --locked --release --lib -- --print native-static-libs 2>&1 | tee "$SDK_TARGET/native-link.log"
SDK_LIB_DIR="$SDK_TARGET/${CARGO_BUILD_TARGET:+$CARGO_BUILD_TARGET/}release"
SDK_LINK_TEXT="$(sed -n 's/^note: native-static-libs: //p' "$SDK_TARGET/native-link.log" | tail -1)"
[[ -n "$SDK_LINK_TEXT" ]] || { echo 'Rust did not report native static link libraries' >&2; exit 1; }
read -r -a SDK_NATIVE_LIBS <<< "$SDK_LINK_TEXT"
"${CC:-cc}" -std=c11 -Wall -Wextra -Werror -I"$SDK_ROOT/include" \
  "$SDK_ROOT/examples/c/zhixu_demo.c" "$SDK_LIB_DIR/libagent_ime_core.a" \
  "${SDK_NATIVE_LIBS[@]}" -o "$SDK_DEMO_OUT/zhixu-demo"
printf '%s\n' "$SDK_DEMO_OUT/zhixu-demo"
