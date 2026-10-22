#!/usr/bin/env bash
# Keeps LaunchServices pointing at the installed input method only.
#
# Builds, fixtures and backups share the bundle identifier. LaunchServices launches the
# highest-version copy it knows, so a fresh build in build/native/release (whose number
# is higher than the installed one) became the input method the system started, and
# was then rebuilt or deleted under the owner's typing (2026-10-10: typing stopped from
# 15:26 while regressions ran). Run after building or testing; it only unregisters.
set -euo pipefail
LSREGISTER="/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister"
BUNDLE_ID="com.hongye.facai.inputmethod.zhizi"
INSTALLED="${AIM_INSTALL_DIR:-$HOME/Library/Input Methods}/Agent Input Method.app"
[[ -x "$LSREGISTER" ]] || exit 0
removed=0
while IFS= read -r path; do
  [[ -z "$path" || "$path" == "$INSTALLED" ]] && continue
  "$LSREGISTER" -u "$path" >/dev/null 2>&1 || true
  removed=$((removed + 1))
done < <("$LSREGISTER" -dump 2>/dev/null | awk -v id="$BUNDLE_ID" '
  /^-+$/ { path = "" }
  /^path:/ { sub(/^path: +/, ""); sub(/ \(0x[0-9a-f]+\)$/, ""); path = $0 }
  $1 == "identifier:" && $2 == id { print path }' | sort -u)
echo "dev_bundles_unregistered=$removed"
