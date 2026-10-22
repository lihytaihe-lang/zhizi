#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "${BASH_SOURCE[0]%/*}/.." && pwd)"
SOURCE_ROOTS=(
  "$ROOT_DIR/native/core"
  "$ROOT_DIR/native/macos/Sources/AgentInputMethodHost"
  "$ROOT_DIR/native/macos/Sources/AgentInputMethodHostKit"
  "$ROOT_DIR/native/macos/Package.swift"
)

SEARCH_TOOL=""
for candidate in rg grep; do
  if ! command -v "$candidate" >/dev/null 2>&1; then
    continue
  fi
  set +e
  if [[ "$candidate" == "rg" ]]; then
    "$candidate" 'agent-ime-search-tool-probe' </dev/null >/dev/null 2>&1
  else
    "$candidate" -E 'agent-ime-search-tool-probe' </dev/null >/dev/null 2>&1
  fi
  probe_status=$?
  set -e
  if [[ $probe_status -eq 0 || $probe_status -eq 1 ]]; then
    SEARCH_TOOL="$candidate"
    break
  fi
done
if [[ -z "$SEARCH_TOOL" ]]; then
  echo "core_network_boundary=ERROR: no search tool (ripgrep/grep) available" >&2
  exit 2
fi

for required_tool in otool nm; do
  if ! command -v "$required_tool" >/dev/null 2>&1; then
    echo "core_network_boundary=ERROR: required binary inspection tool '$required_tool' unavailable" >&2
    exit 2
  fi
done

count_nonempty_lines() {
  local content="$1"
  local count=0
  local line
  while IFS= read -r line; do
    [[ -n "$line" ]] && ((count += 1))
  done <<< "$content"
  printf '%s' "$count"
}

run_search_files() {
  local pattern="$1"
  shift
  if [[ "$SEARCH_TOOL" == "rg" ]]; then
    "$SEARCH_TOOL" -n -i "$pattern" "$@"
  else
    "$SEARCH_TOOL" -n -i -E "$pattern" "$@"
  fi
}

run_search_stdin() {
  local pattern="$1"
  if [[ "$SEARCH_TOOL" == "rg" ]]; then
    "$SEARCH_TOOL" -i "$pattern"
  else
    "$SEARCH_TOOL" -i -E "$pattern"
  fi
}

SOURCE_FILES=()
while IFS= read -r -d '' source_file; do
  SOURCE_FILES+=("$source_file")
done < <(find "${SOURCE_ROOTS[@]}" \
  \( -type d \( -name target -o -name .build -o -name vendor -o -name node_modules \) -prune \) -o \
  \( -type f ! -name Cargo.lock -print0 \))
if [[ ${#SOURCE_FILES[@]} -eq 0 ]]; then
  echo "core_network_boundary=ERROR: no core/host source files found" >&2
  exit 2
fi

# Substring-matched, case-insensitive. `URLSession` deliberately replaces the old
# `NSURLSession`: the modern Swift spelling does not contain the `NS` prefix, so the
# previous pattern let `URLSession.shared.dataTask(...)` through the gate untouched.
# Every pattern below was probed individually against the current source roots and
# matches zero lines, so a hit is a real new network reference rather than
# pre-existing noise. Bare crate names that are also common English substrings
# (`surf` in "surface", `hyper` in "hyperlink") are deliberately kept out of the
# source pattern and covered word-bounded by the dependency pattern instead.
SOURCE_PATTERN='reqwest|ureq|isahc|attohttpc|minreq|URLSession|URLRequest|dataTask|CFNetwork|CFSocket|CFHTTP|Network\.framework|NWConnection|NWListener|NWBrowser|NWPathMonitor|SCNetwork|MultipeerConnectivity|WebSocket|TcpStream|TcpListener|UdpSocket|std::net|socket2|getaddrinfo|inet_addr|inet_pton|https?://'
set +e
source_output="$(run_search_files "$SOURCE_PATTERN" "${SOURCE_FILES[@]}" 2>&1)"
source_status=$?
set -e
if [[ $source_status -gt 1 ]]; then
  echo "$source_output" >&2
  echo "core_network_boundary=ERROR: source search failed with exit $source_status" >&2
  exit 2
fi
source_clients="$(count_nonempty_lines "$source_output")"
if [[ $source_status -eq 0 ]]; then
  echo "$source_output" >&2
  echo "core/host network boundary violated by a prohibited source reference" >&2
  exit 1
fi

dependency_tree="$(cargo tree --manifest-path "$ROOT_DIR/native/core/Cargo.toml" --offline)"
set +e
dependency_output="$(run_search_stdin '(^|[[:space:]])(reqwest|hyper|h2|rustls|native-tls|openssl|tokio|async-std|smol|mio|socket2|ureq|curl|isahc|attohttpc|minreq|surf|tungstenite|trust-dns|hickory|url)([[:space:]]|v|$)' <<< "$dependency_tree" 2>&1)"
dependency_status=$?
set -e
if [[ $dependency_status -gt 1 ]]; then
  echo "$dependency_output" >&2
  echo "core_network_boundary=ERROR: dependency search failed with exit $dependency_status" >&2
  exit 2
fi
dependency_clients="$(count_nonempty_lines "$dependency_output")"
if [[ $dependency_status -eq 0 ]]; then
  echo "$dependency_output" >&2
  echo "core dependency graph contains a network client/runtime" >&2
  exit 1
fi

# Binary inspection is the only layer that can catch a network client reached
# through a path the source/dependency greps do not model. It used to be wrapped in
# `if [[ -x "$EXECUTABLE" ]]`, so a missing bundle silently skipped it and the script
# still printed PASS with counters that had never been populated. That made a PASS
# provable without any binary evidence. It is now fail-hard: no inspectable host
# binary means exit 2, never PASS.
BUNDLE="${1:-}"
if [[ -z "$BUNDLE" ]]; then
  for candidate_bundle in \
    "$ROOT_DIR/build/native/release/Agent Input Method.inputmethod" \
    "$ROOT_DIR/build/native/debug/Agent Input Method.inputmethod"; do
    if [[ -x "$candidate_bundle/Contents/MacOS/AgentInputMethod" ]]; then
      BUNDLE="$candidate_bundle"
      break
    fi
  done
fi
if [[ -z "$BUNDLE" ]]; then
  echo "core_network_boundary=ERROR: no input method bundle to inspect; build one first (./script/build_native.sh release) or pass a bundle path" >&2
  exit 2
fi
EXECUTABLE="$BUNDLE/Contents/MacOS/AgentInputMethod"
if [[ ! -x "$EXECUTABLE" ]]; then
  echo "core_network_boundary=ERROR: host executable missing or not executable: $EXECUTABLE" >&2
  exit 2
fi

linked_libraries="$(otool -L "$EXECUTABLE")"
set +e
linked_output="$(run_search_stdin 'CFNetwork|Network\.framework|WebKit' <<< "$linked_libraries" 2>&1)"
linked_status=$?
set -e
if [[ $linked_status -gt 1 ]]; then
  echo "$linked_output" >&2
  echo "core_network_boundary=ERROR: linked-framework search failed with exit $linked_status" >&2
  exit 2
fi
linked_network_frameworks="$(count_nonempty_lines "$linked_output")"
if [[ $linked_status -eq 0 ]]; then
  echo "$linked_output" >&2
  echo "input method host links a prohibited network framework" >&2
  exit 1
fi
undefined_symbols="$(nm -u "$EXECUTABLE")"
set +e
symbol_output="$(run_search_stdin 'URLSession|CFHTTP|CFNetwork|NWConnection|NWPathMonitor|WebSocket|getaddrinfo' <<< "$undefined_symbols" 2>&1)"
symbol_status=$?
set -e
if [[ $symbol_status -gt 1 ]]; then
  echo "$symbol_output" >&2
  echo "core_network_boundary=ERROR: client-symbol search failed with exit $symbol_status" >&2
  exit 2
fi
client_symbols="$(count_nonempty_lines "$symbol_output")"
if [[ $symbol_status -eq 0 ]]; then
  echo "$symbol_output" >&2
  echo "input method host imports a prohibited network client symbol" >&2
  exit 1
fi

if [[ $source_clients -ne 0 || $dependency_clients -ne 0 || $linked_network_frameworks -ne 0 || $client_symbols -ne 0 ]]; then
  echo "core_network_boundary=ERROR: nonzero prohibited-reference counters" >&2
  exit 1
fi
# The inspected bundle is named in the evidence line so a recorded PASS states which
# artifact it covers instead of leaving that to the reader's assumption.
echo "core_network_boundary=PASS source_clients=$source_clients dependency_clients=$dependency_clients linked_network_frameworks=$linked_network_frameworks client_symbols=$client_symbols source_files=${#SOURCE_FILES[@]} inspected_bundle=${BUNDLE#$ROOT_DIR/}"
