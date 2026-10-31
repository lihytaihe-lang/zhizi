#!/bin/sh
# 仅投影公开分支，不替代 PowerShell 执行或 Inno Setup 编译。
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
iss=${1:-"$root/native/windows/installer/zhizi.iss"}
build=${2:-"$root/script/build_windows.ps1"}
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT HUP INT TERM
project() {
 python3 - "$1" "$2" <<'PY'
import re, sys
from pathlib import Path
iss, ps = (Path(x).read_text() for x in sys.argv[1:])
stack = [True]
for line in iss.splitlines():
    directive = re.match(r'^#(ifdef|ifndef)\s+(\w+)', line)
    if directive:
        kind, name = directive.groups()
        enabled = (kind == 'ifdef') if name == 'PUBLIC' else True
        stack.append(stack[-1] and enabled)
    elif line == '#else':
        previous = stack.pop()
        stack.append(stack[-1] and not previous)
    elif line == '#endif':
        stack.pop()
    elif stack[-1]:
        print(line)
assert len(stack) == 1, 'Unbalanced installer directives'
# 此拒绝检查必须存在；它读取环境以拒绝公开构建，不能当作包内邀请码。
guard = "if($Public -and (Test-Path Env:ZHIZI_TEST_INVITE)){throw 'Public build rejects the test invite environment variable'}"
assert ps.count(guard) == 1, 'Missing public invite rejection'
skip = None
for line in ps.splitlines():
    if line == '# public-check: invite guard begin (rejects environment input; never packaged)':
        assert skip is None
        skip = 'guard'
    elif line == '# public-check: invite guard end':
        assert skip == 'guard'
        skip = None
    elif line == 'if(!$Public){ # public-check: friend begin':
        assert skip is None
        skip = 'friend'
    elif line == '} # public-check: friend end':
        assert skip == 'friend'
        skip = None
    elif skip == 'guard':
        assert line == guard, 'Unexpected code in invite guard'
    elif skip is None:
        print(line)
assert skip is None, 'Unbalanced build projection markers'
PY
}
check() {
 project "$1" "$2" > "$work/public.txt" || return 1
 python3 - "$work/public.txt" <<'PY_CHECK'
import re, sys
from pathlib import Path
content = Path(sys.argv[1]).read_text()
content = re.sub(r'ZHIZI_TEST_INVITE\tzhizi_test_invite(?=\t|\r|\n|$)', '', content, flags=re.IGNORECASE)
# 源码投影检查打包文件引用；实际包由 PowerShell 按文件名检查。
for pattern in (
    r'zhizi-upload\.exe',
    r"[\"'\\/]consent\.json(?=[\"'\s;]|$)",
    r'zhizi\.ihongui|--set-consent',
    r'ZHIZI_TEST_INVITE',
):
    if re.search(pattern, content, re.IGNORECASE):
        print('Public source check failed: forbidden file or marker', file=sys.stderr)
        sys.exit(1)
PY_CHECK
}
check "$iss" "$build"
# 正负例只写临时副本。
cp "$iss" "$work/terms.iss"
printf '\nZHIZI_TEST_INVITE\tzhizi_test_invite\t421\n' >> "$work/terms.iss"
if ! check "$work/terms.iss" "$build"; then
 echo 'Self-test failed: project term was rejected' >&2
 exit 1
fi
for marker in 'Source: "zhizi-upload.exe"' 'zhizi.ihongui' 'ZHIZI_TEST_INVITE=abc123' 'Source: "consent.json"' '--set-consent'; do
 cp "$work/terms.iss" "$work/injected.iss"
 printf '\n%s\n' "$marker" >> "$work/injected.iss"
 if check "$work/injected.iss" "$build" > "$work/negative.log" 2>&1; then
  echo "Self-test failed: injected marker was accepted: $marker" >&2
  exit 1
 fi
done
echo 'Public source check passed; project term accepted and injected leaks rejected.'
