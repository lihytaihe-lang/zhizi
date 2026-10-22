#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PRODUCT_VERSION="$(tr -d '[:space:]' < "$ROOT_DIR/VERSION")"
BUNDLE="${1:-$ROOT_DIR/build/native/release/Agent Input Method.inputmethod}"
HARNESS="${2:-}"

[[ -d "$BUNDLE" && ! -L "$BUNDLE" ]] || { echo "missing release input method bundle: $BUNDLE" >&2; exit 2; }
if [[ -z "$HARNESS" ]]; then
  AIM_CORE_LIBRARY_DIR="$ROOT_DIR/native/core/target/release" swift build \
    --package-path "$ROOT_DIR/native/macos" -c release \
    --disable-automatic-resolution --product AgentIMEPerformanceHarness
  BIN_DIR="$(AIM_CORE_LIBRARY_DIR="$ROOT_DIR/native/core/target/release" swift build \
    --package-path "$ROOT_DIR/native/macos" -c release --disable-automatic-resolution --show-bin-path)"
  HARNESS="$BIN_DIR/AgentIMEPerformanceHarness"
fi
[[ -x "$HARNESS" ]] || { echo "missing release performance harness: $HARNESS" >&2; exit 2; }

TEMP_ROOT="$(mktemp -d /tmp/agent-ime-controller-performance-test.XXXXXX)"
trap 'chmod -R u+rwX "$TEMP_ROOT" 2>/dev/null || true; rm -rf "$TEMP_ROOT"' EXIT
echo "temporary_cleanup_scope=$TEMP_ROOT"

REPORT="$TEMP_ROOT/report.json"
USER_DIR="$TEMP_ROOT/user"
ARTIFACT_SHA="$($ROOT_DIR/script/artifact_checksum.sh "$BUNDLE")"
HARNESS_EXIT=0
DYLD_LIBRARY_PATH="$BUNDLE/Contents/Frameworks" "$HARNESS" \
  "$BUNDLE/Contents/Resources/Rime" "$USER_DIR" \
  "$ROOT_DIR/quality/corpus/base-500.json" "$REPORT" "$ARTIFACT_SHA" \
  "$BUNDLE/Contents/MacOS/AgentInputMethod" 1 --smoke >/dev/null || HARNESS_EXIT=$?

if [[ "$HARNESS_EXIT" -ne 0 ]]; then
  if [[ -f "$REPORT" ]]; then
    if [[ -n "${AIM_PERFORMANCE_REPORT_OUT:-}" ]]; then
      if mkdir -p "$(dirname "$AIM_PERFORMANCE_REPORT_OUT")" && cp "$REPORT" "$AIM_PERFORMANCE_REPORT_OUT"; then
        echo "harness exit=$HARNESS_EXIT; report saved to $AIM_PERFORMANCE_REPORT_OUT" >&2
      else
        echo "harness exit=$HARNESS_EXIT; report persistence failed to $AIM_PERFORMANCE_REPORT_OUT" >&2
      fi
    else
      echo "harness exit=$HARNESS_EXIT; report not persisted (temporary dir is cleaned on exit); set AIM_PERFORMANCE_REPORT_OUT to save future reports" >&2
    fi
  else
    echo "harness exit=$HARNESS_EXIT; no report produced at $REPORT" >&2
  fi
  exit "$HARNESS_EXIT"
fi

if [[ -n "${AIM_PERFORMANCE_REPORT_OUT:-}" ]]; then
  mkdir -p "$(dirname "$AIM_PERFORMANCE_REPORT_OUT")"
  cp "$REPORT" "$AIM_PERFORMANCE_REPORT_OUT"
fi

jq -e --arg version "$PRODUCT_VERSION" --arg checksum "$ARTIFACT_SHA" '
  .artifactChecksum == $checksum and .artifactVersion == $version and
  .buildType == "release" and .measurementMode == "smoke" and .official == false and
  .measurementPath == "IMKInputController.handle receipt before EventMapper through main-thread production CandidatePanel ViewModel application" and
  .mainThreadViewModelApplication == true and .windowPresentationDisabled == true and
  .windowLayoutExcludedByRegisteredEndpoint == true and
  .mainRunLoopDrainedBetweenSequences == true and
  .mainRunLoopAutoreleasePoolPerSequence == true and
  .resourceWindowSamplesAfterMainRunLoopDrain == true and
  (.resourceWindowSamplingRule | type == "string" and length > 0) and
  .resourceWindowRawSampleIntervalSeconds == 60 and
  .resourceWindowRawSamplesPerWindow == 5 and
  (.resourceWindows | length) >= 2 and
  .resourceWindows[-1].elapsedSeconds >= 1 and
  .resourceWindows[-2].elapsedSeconds < 1 and
  .warmupEvents == 1000 and .warmupElapsedSeconds >= 35 and
  .warmupMainRunLoopDrainedBetweenSequences == true and
  (.warmupResourceSamples | length) == 3 and
  .warmupResourceStabilizationSeconds >= 10 and
  (.warmBaselineRule | type == "string" and length > 0) and
  .resourceWindows[0].sample.residentKiB == (.warmupResourceSamples | map(.residentKiB) | sort | .[1]) and
  .resourceWindows[0].sample.fileDescriptors == (.warmupResourceSamples | map(.fileDescriptors) | sort | .[1]) and
  .resourceWindows[0].sample.threads == (.warmupResourceSamples | map(.threads) | sort | .[1]) and
  .sequences > 0 and .keyEvents > 0 and
  .latencySamples == .keyEvents and .p50Milliseconds <= 0.5 and
  .p95Milliseconds <= 2.5 and .p99Milliseconds <= 5 and
  .insertTextLatencySamples > 0 and
  (.maximumInsertTextLatencyMilliseconds | type) == "number" and
  (.insertTextP50Milliseconds | type) == "number" and
  (.insertTextP95Milliseconds | type) == "number" and
  (.insertTextP99Milliseconds | type) == "number" and
  .insertTextP50Milliseconds >= 0 and
  .insertTextP50Milliseconds <= .insertTextP95Milliseconds and
  .insertTextP95Milliseconds <= .insertTextP99Milliseconds and
  .insertTextP99Milliseconds <= .maximumInsertTextLatencyMilliseconds and
  (.localCandidateQueryRule | type == "string" and length > 0) and
  .localCandidateQueryLatencySamples == .keyEvents and
  .localCandidateQueryP50Milliseconds <= .localCandidateQueryP95Milliseconds and
  .localCandidateQueryP95Milliseconds <= .localCandidateQueryP99Milliseconds and
  .localCandidateQueryP95Milliseconds <= 2.5 and
  .localCandidateQueryP99Milliseconds <= 5 and
  .candidateMixerEnabled == true and
  (.currentCandidatePipelineRule | type == "string" and length > 0) and
  .currentCandidatePipelineLatencySamples == .keyEvents and
  .currentCandidatePipelineP50Milliseconds <= .currentCandidatePipelineP95Milliseconds and
  .currentCandidatePipelineP95Milliseconds <= .currentCandidatePipelineP99Milliseconds and
  .currentCandidatePipelineP95Milliseconds <= 20 and
  .composingBackspaceLatencySamples == .sequences and
  .composingBackspaceP95Milliseconds <= .composingBackspaceP99Milliseconds and
  .composingBackspaceP95Milliseconds <= 2.5 and
  .composingBackspaceP99Milliseconds <= 5 and
  .coldStartExecutionPath == "process-entry/non-nil-IMKServer/production-AgentInputController/activate/handle/marked+candidates/close" and
  .coldStartProductionController == true and .coldStartServerNonNil == true and
  .coldStartFirstEventConsumed == true and .coldStartMarkedTextApplied == true and
  .coldStartCandidateCountPositive == true and .coldStartLiveHandlesAfterZero == true and
  .coldStartTISConnectionRegistrationClaimed == false and
  .coldStarts == 2 and .coldStartP95Milliseconds <= 300 and
  .lifecycleSessions == 20 and .lifecycleLiveHandlesBefore == 0 and
  .lifecycleLiveHandlesAfter == 0 and .resourceGatePassed == true and
  .runPassed == true and .failures == 0 and
  (.contextModel.status == "PASS" or (.contextModel.status == "SKIP" and .contextModel.reason == "model-files-missing-or-sha256-mismatch")) and
  (.contextModel.status != "PASS" or (.contextModel.enabled == true and .contextModel.rerankCalls > 0 and .contextModel.rerankP95Milliseconds <= 8 and (.contextModel.coldStartMilliseconds | length) == 2))
' "$REPORT" >/dev/null

jq -r '
  "controller_performance_smoke=PASS samples=\(.latencySamples) p50_ms=\(.p50Milliseconds) p95_ms=\(.p95Milliseconds) p99_ms=\(.p99Milliseconds) local_query_p95_ms=\(.localCandidateQueryP95Milliseconds) candidate_pipeline_p95_ms=\(.currentCandidatePipelineP95Milliseconds) composing_backspace_p95_ms=\(.composingBackspaceP95Milliseconds) composing_backspace_p99_ms=\(.composingBackspaceP99Milliseconds) cold_p95_ms=\(.coldStartP95Milliseconds) lifecycle=\(.lifecycleSessions)"
' "$REPORT"


jq -r '.contextModel | "context_model_controller_performance=\(.status) key_p50_ms=\(.keyP50Milliseconds) key_p95_ms=\(.keyP95Milliseconds) key_p99_ms=\(.keyP99Milliseconds) rerank_p50_ms=\(.rerankP50Milliseconds) rerank_p95_ms=\(.rerankP95Milliseconds) cold_p95_ms=\(.coldStartP95Milliseconds) reason=\(.reason // "none")"' "$REPORT"
