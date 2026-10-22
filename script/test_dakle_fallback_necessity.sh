#!/usr/bin/env bash
# Build a verification-only core with owner raw-hit injection disabled, then
# exhaust every dakle candidate page in clean and owner-copy arms.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MODE="${1:---check}"
BUNDLE="${2:-$ROOT_DIR/build/native/release/Agent Input Method.inputmethod}"
SHARED_RIME="$BUNDLE/Contents/Resources/Rime"
LIVE_USER_RIME="$HOME/Library/Application Support/AgentInputMethod/Rime"
FROZEN_REPORT="$ROOT_DIR/docs/current/evidence/dakle-round4-fallback-necessity-v1.json"
FACTORY_PRIORITIES="$ROOT_DIR/native/resources/Rime/agent_factory_reading_priorities.tsv"
OWNER_READINGS="$ROOT_DIR/native/resources/Rime/agent_owner_verified_readings.tsv"
MIXER_SOURCE="$ROOT_DIR/native/core/agent-ime-core/src/mixer.rs"

[[ "$MODE" == "--check" || "$MODE" == "--write" ]] || {
  echo "usage: $0 [--check|--write] [release-bundle]" >&2
  exit 2
}
[[ -d "$BUNDLE" && ! -L "$BUNDLE" ]] || {
  echo "missing release input bundle: $BUNDLE" >&2
  exit 2
}
[[ -d "$LIVE_USER_RIME" && ! -L "$LIVE_USER_RIME" ]] || {
  echo "missing owner Rime directory: $LIVE_USER_RIME" >&2
  exit 2
}

TEMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/agent-ime-dakle-fallback-necessity.XXXXXX")"
trap 'chmod -R u+rwX "$TEMP_ROOT" 2>/dev/null || true; rm -rf "$TEMP_ROOT"' EXIT
CORE_TARGET="$TEMP_ROOT/core-target"
SWIFT_SCRATCH="$TEMP_ROOT/swift-build"
mkdir -p "$CORE_TARGET" "$SWIFT_SCRATCH" "$TEMP_ROOT/clean-user" "$TEMP_ROOT/owner-user"
cp -R "$LIVE_USER_RIME"/. "$TEMP_ROOT/owner-user/"
chmod -R u+rwX "$TEMP_ROOT/owner-user"

LIBRARY_PATH="$ROOT_DIR/.build-deps/librime/1.16.1/lib" \
  CARGO_TARGET_DIR="$CORE_TARGET" cargo build --locked --offline \
  --manifest-path "$ROOT_DIR/native/core/agent-ime-core/Cargo.toml" \
  --release --features "librime,diagnostic-no-owner-raw-hits" >/dev/null

if [[ "${AIM_SWIFTPM_SANDBOX_WORKAROUND:-0}" == "1" ]]; then
  SWIFT_MODULE_CACHE="$TEMP_ROOT/swift-module-cache"
  mkdir -p "$SWIFT_MODULE_CACHE"
  CLANG_MODULE_CACHE_PATH="$SWIFT_MODULE_CACHE" \
    SWIFTPM_MODULECACHE_OVERRIDE="$SWIFT_MODULE_CACHE" \
    AIM_CORE_LIBRARY_DIR="$CORE_TARGET/release" swift build \
    --package-path "$ROOT_DIR/native/macos" -c debug --disable-automatic-resolution \
    --disable-sandbox --scratch-path "$SWIFT_SCRATCH" \
    --product AgentIMEUserDBHarness
  BIN_DIR="$(CLANG_MODULE_CACHE_PATH="$SWIFT_MODULE_CACHE" \
    SWIFTPM_MODULECACHE_OVERRIDE="$SWIFT_MODULE_CACHE" swift build \
    --package-path "$ROOT_DIR/native/macos" -c debug --disable-automatic-resolution \
    --disable-sandbox --scratch-path "$SWIFT_SCRATCH" --show-bin-path)"
else
  AIM_CORE_LIBRARY_DIR="$CORE_TARGET/release" swift build \
    --package-path "$ROOT_DIR/native/macos" -c debug --disable-automatic-resolution \
    --scratch-path "$SWIFT_SCRATCH" --product AgentIMEUserDBHarness
  BIN_DIR="$(swift build \
    --package-path "$ROOT_DIR/native/macos" -c debug --disable-automatic-resolution \
    --scratch-path "$SWIFT_SCRATCH" --show-bin-path)"
fi
HARNESS="$BIN_DIR/AgentIMEUserDBHarness"
USERDB_CTL="$BUNDLE/Contents/MacOS/AgentIMEUserDBCtl"

entry_digest() {
  DYLD_LIBRARY_PATH="$BUNDLE/Contents/Frameworks" "$USERDB_CTL" list \
    "$SHARED_RIME" "$1" |
    jq -S -c '[.items[] | {pinyin, text, entryLine}] | sort_by(.pinyin, .text)' |
    shasum -a 256 | awk '{print $1}'
}

OWNER_DIGEST_BEFORE="$(entry_digest "$TEMP_ROOT/owner-user")"
for arm in clean owner; do
  user_directory="$TEMP_ROOT/$arm-user"
  for repetition in 1 2 3 4 5; do
    output="$TEMP_ROOT/$arm-$repetition.json"
    DYLD_LIBRARY_PATH="$BUNDLE/Contents/Frameworks:$CORE_TARGET/release" \
      "$HARNESS" --process-deep-candidate-probe \
      "$SHARED_RIME" "$user_directory" dakle "$output" 4096 >/dev/null
    jq -e '
      .query == "dakle" and .complete == true and
      (.pages | length) >= 1 and
      (.allCandidates | length) == ([.pages[].candidates[]] | length)
    ' "$output" >/dev/null || {
      jq '{query, complete, pageCount:(.pages | length),
        candidateCount:(.allCandidates | length), lastPage:(.pages[-1] // null)}' \
        "$output" >&2
      exit 1
    }
    if [[ "$repetition" -gt 1 ]]; then
      cmp -s "$TEMP_ROOT/$arm-1.json" "$output" || {
        echo "dakle fallback diagnostic is unstable in $arm arm" >&2
        exit 1
      }
    fi
  done
done
OWNER_DIGEST_AFTER="$(entry_digest "$TEMP_ROOT/owner-user")"
[[ "$OWNER_DIGEST_BEFORE" == "$OWNER_DIGEST_AFTER" ]] || {
  echo "dakle fallback diagnostic changed owner-copy logical entries" >&2
  exit 1
}

OPEN_PRIORITY="$(awk -F '\t' '$1 == "打开" && $2 == "da kai" {print $3}' \
  "$FACTORY_PRIORITIES" | sort -nr | head -1)"
LE_PRIORITY="$(awk -F '\t' '$1 == "了" && $2 == "le" {print $3}' \
  "$FACTORY_PRIORITIES" | sort -nr | head -1)"
[[ "$OPEN_PRIORITY" =~ ^[0-9]+$ && "$LE_PRIORITY" =~ ^[0-9]+$ ]] || {
  echo "missing factory priority path for 打开 / 了" >&2
  exit 1
}
if [[ "$OPEN_PRIORITY" -lt "$LE_PRIORITY" ]]; then
  FACTORY_PRIORITY="$OPEN_PRIORITY"
else
  FACTORY_PRIORITY="$LE_PRIORITY"
fi
grep -Fx $'打开了\tda kai le\tdakle' "$OWNER_READINGS" >/dev/null

clean_rank="$(jq '(.allCandidates | index("打开了")) as $rank |
  if $rank == null then null else $rank + 1 end' "$TEMP_ROOT/clean-1.json")"
owner_rank="$(jq '(.allCandidates | index("打开了")) as $rank |
  if $rank == null then null else $rank + 1 end' "$TEMP_ROOT/owner-1.json")"
clean_list_sha="$(jq -S -c '.allCandidates' "$TEMP_ROOT/clean-1.json" |
  shasum -a 256 | awk '{print $1}')"
owner_list_sha="$(jq -S -c '.allCandidates' "$TEMP_ROOT/owner-1.json" |
  shasum -a 256 | awk '{print $1}')"

jq -n \
  --slurpfile clean "$TEMP_ROOT/clean-1.json" \
  --slurpfile owner "$TEMP_ROOT/owner-1.json" \
  --arg tableSHA "$(shasum -a 256 "$OWNER_READINGS" | awk '{print $1}')" \
  --arg factorySHA "$(shasum -a 256 "$FACTORY_PRIORITIES" | awk '{print $1}')" \
  --arg sourceSHA "$(shasum -a 256 "$MIXER_SOURCE" | awk '{print $1}')" \
  --arg cleanListSHA "$clean_list_sha" \
  --arg ownerListSHA "$owner_list_sha" \
  --argjson factoryPriority "$FACTORY_PRIORITY" \
  --argjson openPriority "$OPEN_PRIORITY" \
  --argjson lePriority "$LE_PRIORITY" \
  --argjson cleanRank "$clean_rank" \
  --argjson ownerRank "$owner_rank" '
  {
    kind: "dakle-round4-fallback-necessity",
    formatVersion: 1,
    diagnosticFeature: "diagnostic-no-owner-raw-hits",
    query: "dakle",
    target: "打开了",
    fullReading: "da kai le",
    ownerReadingsSHA256: $tableSHA,
    factoryPrioritiesSHA256: $factorySHA,
    runtimeSourceSHA256: $sourceSHA,
    rawFullySyllabifiable: false,
    exactPrefixCoverage: true,
    coveredSyllables: 3,
    targetFactoryPath: [
      {text: "打开", reading: "da kai", weight: $openPriority},
      {text: "了", reading: "le", weight: $lePriority}
    ],
    targetFactoryPriority: $factoryPriority,
    mechanismCandidateScope: "current-first-page-existing-candidates-only",
    clean: {
      top5: $clean[0].pages[0].candidates,
      exhaustiveCandidateCount: ($clean[0].allCandidates | length),
      exhaustiveListSHA256: $cleanListSHA,
      exhaustiveSearchComplete: $clean[0].complete,
      targetRank: $cleanRank
    },
    owner: {
      top5: $owner[0].pages[0].candidates,
      exhaustiveCandidateCount: ($owner[0].allCandidates | length),
      exhaustiveListSHA256: $ownerListSHA,
      exhaustiveSearchComplete: $owner[0].complete,
      targetRank: $ownerRank
    },
    stableRepetitionsPerArm: 5,
    ownerEntriesUnchanged: true
  }' > "$TEMP_ROOT/generated.json"

if [[ "$MODE" == "--write" ]]; then
  cp "$TEMP_ROOT/generated.json" "$FROZEN_REPORT"
else
  cmp -s "$TEMP_ROOT/generated.json" "$FROZEN_REPORT" || {
    echo "dakle fallback necessity report is stale" >&2
    diff -u "$FROZEN_REPORT" "$TEMP_ROOT/generated.json" || true
    exit 1
  }
fi

jq -r '
  "dakle_fallback_necessity=PASS clean_top5=\(.clean.top5 | join("/")) owner_top5=\(.owner.top5 | join("/")) clean_rank=\(.clean.targetRank // "absent") owner_rank=\(.owner.targetRank // "absent") owner_exhaustive=\(.owner.exhaustiveSearchComplete) owner_entries_unchanged=\(.ownerEntriesUnchanged)"
' "$TEMP_ROOT/generated.json"
