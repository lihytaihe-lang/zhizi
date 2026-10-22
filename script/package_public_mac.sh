#!/usr/bin/env bash
# Public Mac package: 知字-<VERSION>.dmg holding the manager with the input method
# inside (the manager installs it on first launch). Unlike the friend package, the
# manager must come from a --no-default-features build (no test-upload, no
# personal-seed), nothing here talks to Apple: notarization is left to the
# maintainer; this script only prints the command template.
#
#   SIGN_IDENTITY              required; Developer ID identity, or "-" for an ad-hoc dry run
#   ZHIZI_MANAGER_CARGO_FLAGS  must contain --no-default-features (guards the build context)
#   NOTARIZE                   1 = staple + assess the freshly packaged dmg (only after the
#                              maintainer has notarized that exact dmg); default prints instead
#   --staple-only <dmg>        only staple + assess an already-notarized dmg
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

staple_and_assess() {
  xcrun stapler staple "$1"
  spctl --assess --type open --context context:primary-signature --verbose=2 "$1"
}

print_notary_instructions() {
  local dmg="$1"
  cat <<EOF
公证由维护者执行：本脚本不提交公证、不访问任何密钥。占位符均为环境变量名，不含任何值。
  xcrun notarytool submit '$dmg' --key <NOTARY_KEY_PATH> --key-id <NOTARY_KEY_ID> --issuer <NOTARY_ISSUER> --wait
  xcrun stapler staple '$dmg'
  spctl --assess --type open --context context:primary-signature --verbose=2 '$dmg'
公证完成后如需对已公证的 dmg 单独补票据：$0 --staple-only '$dmg'
EOF
}

if [[ "${1:-}" == "--staple-only" ]]; then
  [[ $# -eq 2 && -f "$2" ]] || { echo "usage: $0 --staple-only <notarized-dmg>" >&2; exit 2; }
  staple_and_assess "$2"
  echo "public_mac_staple=$2"
  exit 0
fi
[[ $# -eq 0 ]] || { echo "usage: $0 [--staple-only <notarized-dmg>]" >&2; exit 2; }

: "${SIGN_IDENTITY:?SIGN_IDENTITY is required (Developer ID identity, or - for an ad-hoc dry run)}"
case "${ZHIZI_MANAGER_CARGO_FLAGS:-}" in
  *--no-default-features*) ;;
  *) echo "ZHIZI_MANAGER_CARGO_FLAGS must contain --no-default-features; build the manager with:" >&2
     echo "  ZHIZI_MANAGER_CARGO_FLAGS=--no-default-features script/build_manager.sh" >&2
     exit 2 ;;
esac

IME="$ROOT_DIR/build/native/release/Agent Input Method.inputmethod"
MANAGER="$ROOT_DIR/src-tauri/target/release/bundle/macos/Agent Input Method.app"
[[ -d "$IME" && -d "$MANAGER" ]] || { echo "build the input method and manager first" >&2; exit 2; }
RELEASE_VERSION="$(tr -d '[:space:]' < "$ROOT_DIR/VERSION")"
[[ "$RELEASE_VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "bad VERSION file: $RELEASE_VERSION" >&2; exit 2; }
OUT="$ROOT_DIR/releases/public-mac/$RELEASE_VERSION"
STAGE="$OUT/stage"
APP="$STAGE/知字.app"
NESTED="$APP/Contents/Resources/InputMethod/Agent Input Method.app"
DMG="$OUT/知字-$RELEASE_VERSION.dmg"

rm -rf "$OUT"
mkdir -p "$STAGE"
trap 'rm -rf "$OUT"' EXIT
ditto "$MANAGER" "$APP"
mkdir -p "$APP/Contents/Resources/InputMethod"
ditto "$IME" "$APP/Contents/Resources/InputMethod/Agent Input Method.app"
cat > "$STAGE/README.txt" <<'EOF'
知字输入法（Mac）/ Zhizi Input Method (macOS)
=============================================

【这是什么】知字是开源的中文拼音输入法，本安装包里是「知字」应用（管理台），
它内含输入法本体，第一次打开时会自动安装输入法。

【安装】
1. 把「知字」拖到右边的「应用程序」文件夹。
2. 打开「应用程序」里的「知字」，按提示完成输入法安装。
   如 macOS 提示无法验证开发者，请在「应用程序」里右键（或按住 Control 点）「知字」→「打开」。
3. 点屏幕右上角菜单栏的输入法图标，选「知字拼音」。如果列表里没有：
   「系统设置 → 键盘 → 输入法（编辑）」→「+」→ 简体中文 → 添加「知字拼音」；
   仍没有就注销再登录一次。

【卸载】点菜单栏的「知字」图标 →「卸载知字输入法…」，然后把「应用程序」里的
「知字」拖进废纸篓；输入法列表里的条目在下次注销登录后消失。

【授权】本软件按项目仓库中的 LICENSE 文件授权；所含第三方组件的许可见应用包内
THIRD_PARTY_NOTICES 与 Licenses 目录。

【模型】可选的本地候选排序模型（默认不启用，不影响使用）见项目仓库
docs/public/MODEL.md。

English
-------
Zhizi is an open-source Chinese pinyin input method. This disk image holds the
Zhizi app, which bundles and installs the input method on first launch.

Install: drag 知字 to Applications, open it and follow the prompts, then pick
"知字拼音" from the input-method menu. If macOS blocks an unidentified
developer, right-click (or Control-click) the app and choose Open.

Uninstall: use "卸载知字输入法…" in the Zhizi menu-bar icon, then move the app
to the Trash; the input-method entry disappears after the next logout.

Licensing: see the LICENSE file in the project repository; third-party notices
ship inside the app bundle. The optional local ranking model is documented in
docs/public/MODEL.md in the repository.
EOF
ln -s /Applications "$STAGE/Applications"

# --- Self-checks: fail before any signing or dmg creation. ---
plist_value() { /usr/libexec/PlistBuddy -c "Print :$2" "$1"; }
IME_BUILD="$(plist_value "$IME/Contents/Info.plist" CFBundleVersion)"
MGR_BUILD="$(plist_value "$APP/Contents/Info.plist" CFBundleVersion)"
IME_COMMIT="$(plist_value "$IME/Contents/Info.plist" AgentIMEProductCommit)"
MGR_COMMIT="$(plist_value "$APP/Contents/Info.plist" AgentIMEProductCommit)"
echo "selfcheck ime build=$IME_BUILD manager build=$MGR_BUILD"
echo "selfcheck ime commit=$IME_COMMIT manager commit=$MGR_COMMIT"
[[ "$IME_BUILD" == "$MGR_BUILD" ]] || { echo "CFBundleVersion mismatch between input method and manager" >&2; exit 1; }
[[ "$IME_COMMIT" == "$MGR_COMMIT" ]] || { echo "AgentIMEProductCommit mismatch between input method and manager" >&2; exit 1; }

AVAILABILITY="$NESTED/Contents/Resources/ContextModel/availability.json"
[[ -f "$AVAILABILITY" ]] || { echo "missing $AVAILABILITY" >&2; exit 1; }
python3 - "$AVAILABILITY" <<'PY'
import json, sys
with open(sys.argv[1], encoding="utf-8") as handle:
    record = json.load(handle)
tier = record.get("tier", "<absent: official-format record>")
print(f"selfcheck model available={record.get('available')} tier={tier}")
PY

# Manager binaries must carry no test-upload / personal-seed code. These three
# markers are verified by inspection: present in default-features builds, absent
# from --no-default-features builds. The invite env-var name is NOT a marker:
# layer2.rs embeds the project-terms dictionary, which carries that name as an
# ordinary term row in every build.
FORBIDDEN_STRINGS=('zhizi.ihongui' 'test-upload' 'personalProfileV1Seeded')
while IFS= read -r executable; do
  for pattern in "${FORBIDDEN_STRINGS[@]}"; do
    # No grep -q here: with pipefail, grep -q's early exit SIGPIPEs strings and
    # turns a hit into a non-zero pipeline status.
    if strings "$executable" | grep -F -- "$pattern" >/dev/null; then
      echo "selfcheck FAILED: '$pattern' found in $executable" >&2
      echo "the manager looks like a default-features (test-upload) build; rebuild with:" >&2
      echo "  ZHIZI_MANAGER_CARGO_FLAGS=--no-default-features script/build_manager.sh" >&2
      exit 1
    fi
  done
done < <(find "$APP/Contents/MacOS" -type f | LC_ALL=C sort)
echo "selfcheck manager binaries clean: no ${FORBIDDEN_STRINGS[*]}"

sign() {
  if [[ "$SIGN_IDENTITY" == "-" ]]; then
    codesign --force --sign - --timestamp=none "$@"
  else
    codesign --force --options runtime --timestamp --sign "$SIGN_IDENTITY" "$@"
  fi
}

# Inside out: libraries, then executables, then each bundle.
while IFS= read -r library; do sign "$library"; done \
  < <(find "$NESTED" -type f -name '*.dylib' | LC_ALL=C sort)
while IFS= read -r executable; do sign "$executable"; done \
  < <(find "$NESTED/Contents/MacOS" -type f -perm -111 | LC_ALL=C sort)
sign "$NESTED"
while IFS= read -r executable; do sign "$executable"; done \
  < <(find "$APP/Contents/MacOS" -type f -perm -111 | LC_ALL=C sort)
sign "$APP"
codesign --verify --deep --strict --verbose=1 "$APP"

hdiutil create -quiet -volname "知字" -srcfolder "$STAGE" -ov -format UDZO "$DMG"
[[ "$SIGN_IDENTITY" == "-" ]] || codesign --force --timestamp --sign "$SIGN_IDENTITY" "$DMG"
shasum -a 256 "$DMG" | tee "$DMG.sha256"
rm -rf "$STAGE"
trap - EXIT

if [[ "${NOTARIZE:-0}" == "1" ]]; then
  staple_and_assess "$DMG"
else
  print_notary_instructions "$DMG"
fi
echo "public_mac_package=$DMG"
