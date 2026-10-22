#!/usr/bin/env bash
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
# Records WHAT changes in the HIToolbox input-source preferences, and what was running, each time they change.
# (watch_input_source.sh only logs a hash; the hash flips between two values every few minutes and the cause is unknown.)
#   scripts/tools/watch_input_source_diff.sh [out dir]      stop it by its PID
set -u
EVAL_OUT="${ZHIZI_EVAL_OUT:-$ROOT_DIR/eval-runs}"
OUT="${1:-$EVAL_OUT/inputsource-diff}"
mkdir -p "$OUT"
snapshot() { defaults read com.apple.HIToolbox 2>/dev/null; }
previous="$(snapshot)"
printf '%s\n' "$previous" > "$OUT/state-initial.txt"
count=0
while sleep 1; do
  current="$(snapshot)"
  [[ "$current" == "$previous" ]] && continue
  count=$((count + 1))
  stamp="$(date '+%Y%m%d-%H%M%S')"
  {
    echo "== change $count at $stamp"
    diff <(printf '%s\n' "$previous") <(printf '%s\n' "$current")
    echo "-- processes started in the last 90 s (pid, start, command):"
    ps -ax -o pid=,lstart=,command= | awk -v now="$(date +%s)" '{print}' | while read -r pid rest; do
      started="$(ps -o lstart= -p "$pid" 2>/dev/null)"
      [[ -z "$started" ]] && continue
      epoch="$(date -j -f '%a %b %e %T %Y' "$started" +%s 2>/dev/null || echo 0)"
      (( $(date +%s) - epoch <= 90 )) && echo "$pid $started $(ps -o command= -p "$pid" | cut -c1-140)"
    done
    echo "-- log (last 25 s, HIToolbox / TextInput / cfprefsd for HIToolbox):"
    log show --last 25s --style compact --predicate 'subsystem CONTAINS "TextInput" OR process == "TextInputMenuAgent" OR process == "imklaunchagent" OR eventMessage CONTAINS "HIToolbox"' 2>/dev/null | tail -40 | cut -c1-220
  } >> "$OUT/changes.log"
  previous="$current"
done
