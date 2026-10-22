#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BUNDLE="${1:-$ROOT_DIR/build/native/release/Agent Input Method.inputmethod}"
CORPUS="$ROOT_DIR/quality/corpus/offcorpus-probe-v1.json"
ASCII_PREFIX_CORPUS="$ROOT_DIR/quality/corpus/ascii-prefix-position-v1.json"
PROJECT_TERMS="$ROOT_DIR/native/resources/Rime/agent_project_terms.dict.yaml"
FROZEN_CORPUS_SHA="8bb635b0a6e12659c9fdef77ab6898f810e8ebc47d1d3f32f8227a106f0516c7"
FROZEN_ASCII_PREFIX_SHA="d43b7446bb213db3ed6674d9982211b27c0c9827ac42202c59a8fdfea298f0cd"
[[ -d "$BUNDLE" && ! -L "$BUNDLE" ]] || { echo "missing release input bundle: $BUNDLE" >&2; exit 2; }

ACTUAL_CORPUS_SHA="$(shasum -a 256 "$CORPUS" | awk '{print $1}')"
[[ "$ACTUAL_CORPUS_SHA" == "$FROZEN_CORPUS_SHA" ]] || {
  echo "offcorpus probe checksum mismatch: expected $FROZEN_CORPUS_SHA, got $ACTUAL_CORPUS_SHA" >&2
  exit 6
}
ACTUAL_ASCII_PREFIX_SHA="$(shasum -a 256 "$ASCII_PREFIX_CORPUS" | awk '{print $1}')"
[[ "$ACTUAL_ASCII_PREFIX_SHA" == "$FROZEN_ASCII_PREFIX_SHA" ]] || {
  echo "ASCII prefix position checksum mismatch: expected $FROZEN_ASCII_PREFIX_SHA, got $ACTUAL_ASCII_PREFIX_SHA" >&2
  exit 6
}
node "$ROOT_DIR/scripts/generate_ascii_prefix_probe.mjs" --check >/dev/null

touch "$ROOT_DIR/native/macos/Sources/AgentIMECoreBridge/bridge.c"
AIM_CORE_LIBRARY_DIR="$ROOT_DIR/native/core/target/release" swift build \
  --package-path "$ROOT_DIR/native/macos" -c debug --disable-automatic-resolution \
  --product AgentIMEReplayHarness >/dev/null
BIN_DIR="$(swift build \
  --package-path "$ROOT_DIR/native/macos" -c debug --disable-automatic-resolution --show-bin-path)"
HARNESS="$BIN_DIR/AgentIMEReplayHarness"
TEMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/agent-ime-offcorpus.XXXXXX")"
trap 'chmod -R u+rwX "$TEMP_ROOT" 2>/dev/null || true; rm -rf "$TEMP_ROOT"' EXIT
REPORT="$TEMP_ROOT/report.json"
ASCII_PREFIX_REPORT="$TEMP_ROOT/ascii-prefix-report.json"
ARTIFACT_SHA="$($ROOT_DIR/script/artifact_checksum.sh "$BUNDLE")"

set +e
DYLD_LIBRARY_PATH="$BUNDLE/Contents/Frameworks:$ROOT_DIR/native/core/target/release" "$HARNESS" \
  "$BUNDLE/Contents/Resources/Rime" "$TEMP_ROOT/user" "$CORPUS" "$REPORT" 5 "$ARTIFACT_SHA" \
  >/dev/null
HARNESS_STATUS=$?
set -e
if [[ "$HARNESS_STATUS" -ne 0 ]]; then
  jq -c '.observations[] | {id, query, candidates: .actual}' "$REPORT" >&2
  jq -c '.failures[]' "$REPORT" >&2
  exit "$HARNESS_STATUS"
fi

set +e
DYLD_LIBRARY_PATH="$BUNDLE/Contents/Frameworks:$ROOT_DIR/native/core/target/release" "$HARNESS" \
  "$BUNDLE/Contents/Resources/Rime" "$TEMP_ROOT/ascii-prefix-user" \
  "$ASCII_PREFIX_CORPUS" "$ASCII_PREFIX_REPORT" 1 "$ARTIFACT_SHA" >/dev/null
ASCII_PREFIX_STATUS=$?
set -e
if [[ "$ASCII_PREFIX_STATUS" -ne 0 ]]; then
  jq -c '.failures[]' "$ASCII_PREFIX_REPORT" >&2
  exit "$ASCII_PREFIX_STATUS"
fi

PROJECT_TERMS_JSON="$(awk -F '\t' \
  'NF >= 3 && $0 !~ /^[#.-]/ { print tolower($1) }' "$PROJECT_TERMS" \
  | jq -Rsc 'split("\n") | map(select(length > 0))')"

jq -e --arg checksum "$ARTIFACT_SHA" --argjson projectTerms "$PROJECT_TERMS_JSON" '
  .artifactChecksum == $checksum and .cases == 76 and .repetitions == 5 and
  .attemptedSessions == 380 and .createdSessions == 380 and .executedSessions == 380 and
  .candidatesMissingReadingKey == 0 and .top5Hits == 76 and
  .mustPassTop1Hits == 58 and .mustPassTop1Cases == 58 and .stableCases == 76 and
  (.failures | length) == 0 and
  (.observations | map({key: .id, value: .actual}) | from_entries) as $actual |
  ($actual["ascii-mariadb"] | index("mariadb")) != null and
  ($actual["ascii-docker"] | index("docker")) != null and
  ($actual["ascii-react"] | index("react")) != null and
  ($actual["ascii-postgres"] | index("postgres")) != null and
  ($actual["ascii-nginx"] | index("nginx")) != null and
  ($actual["ascii-windows"] | index("windows")) != null and
  ($actual["ascii-kubernetes"] | index("kubernetes")) != null and
  ($actual["ascii-redis"] | index("redis")) != null and
  (["MariaDB", "mariadb"] | index($actual["ascii-mariadb"][0])) != null and
  (["Docker", "docker"] | index($actual["ascii-docker"][0])) != null and
  (["React", "react"] | index($actual["ascii-react"][0])) != null and
  (["PostgreSQL", "Postgres", "postgres"] | index($actual["ascii-postgres"][0])) != null and
  (["Windows", "windows"] | index($actual["ascii-windows"][0])) != null and
  (["Kubernetes", "kubernetes"] | index($actual["ascii-kubernetes"][0])) != null and
  (["Redis", "redis"] | index($actual["ascii-redis"][0])) != null and
  $actual["pinyin-nihao"][0] == "你好" and
  $actual["pinyin-zhongguo"][0] == "中国" and
  $actual["pinyin-wanshang"][0] == "晚上" and
  $actual["pinyin-gongzuo"][0] == "工作" and
  $actual["control-dansh"][0] == "但是" and
  $actual["control-zhem"][0] == "这么" and
  $actual["control-idiom"][0] == "百尺竿头更进一步" and
  $actual["pollution-ra"][0] == "热爱" and
  (["的", "地", "得"] | index($actual["pollution-d"][0])) != null and
  all(($actual["ascii-windows"] + $actual["ascii-nginx"] + $actual["pollution-ra"] + $actual["pollution-d"])[];
    . as $candidate |
    ($candidate | ascii_downcase | contains("inputmethod") | not) and
    (($projectTerms | index($candidate | ascii_downcase)) == null)) and
  $actual["abbrev-luanqbz"][0] == "乱七八糟" and
  $actual["abbrev-lqbz"][0] == "乱七八糟" and
  $actual["abbrev-bcgt"][0] == "百尺竿头" and
  $actual["abbrev-wangybl"][0] == "亡羊补牢" and
  $actual["abbrev-zhonggr"][0] == "中国人" and
  $actual["abbrev-zhem"][0] == "这么" and $actual["abbrev-dansh"][0] == "但是" and
  $actual["abbrev-gongz"][0] == "工作" and $actual["abbrev-shij"][0] == "时间" and
  ($actual["ascii-prefix-windo"] | index("Windows")) != null and
  ($actual["ascii-prefix-window"] | index("Windows")) != null and
  ($actual["ascii-prefix-maria"] | index("MariaDB")) != null and
  ($actual["ascii-prefix-clau"] | index("Claude")) != null and
  ($actual["ascii-prefix-dock"] | index("Docker")) != null and
  ($actual["ascii-prefix-typescr"] | index("TypeScript")) != null and
  $actual["prefix-control-ni"][0] == "你" and $actual["prefix-control-wo"][0] == "我" and
  $actual["prefix-control-ma"][0] == "吗" and $actual["prefix-control-da"][0] == "大" and
  $actual["prefix-control-ke"][0] == "可" and
  $actual["canonical-mariadb"][0:2] == ["MariaDB", "mariadb"] and
  $actual["canonical-github"][0:2] == ["GitHub", "github"] and
  $actual["canonical-macos"][0] == "macOS" and
  $actual["canonical-typescript"][0] == "TypeScript" and
  $actual["canonical-nginx"][0] == "nginx" and
  (($actual["canonical-nginx"] | map(select(. == "nginx")) | length) == 1) and
  (($actual["ascii-prefix-homepage-sv"] | map(select(. == "是" or . == "说")) | length) >= 1)
  and $actual["d1-positive-fulei"][0] == "父类"
  and $actual["d1-positive-jiasuo"][0] == "加锁"
  and $actual["d1-positive-leiming"][0] == "类名"
  and $actual["d1-owner-siyouyun"][0] == "私有云"
  and $actual["d1-control-shijian"][0] == "时间"
  and $actual["d1-control-wenti"][0] == "问题"
  and $actual["d2-owner-weixin"][0] == "微信"
  and ($actual["d2-owner-weixin"] | index("威信")) != null
  and ($actual["d2-owner-weibo"] | index("微博")) != null
  and $actual["d2-owner-douyin"][0] == "抖音"
  and $actual["d2-owner-xiaohongshu"][0] == "小红书"
  and $actual["d2-owner-pinduoduo"][0] == "拼多多"
  and $actual["d2-owner-tesila"][0] == "特斯拉"
  and ($actual["d2-owner-maidian"] | index("埋点")) != null
  and $actual["d2-owner-dashuju"][0] == "大数据"
  and $actual["d2-owner-qukuailian"][0] == "区块链"
  and $actual["d2-owner-bitebi"][0] == "比特币"
  and $actual["d2-owner-zhengqiji"][0] == "蒸汽机"
  and $actual["d2-owner-wate"][0] == "瓦特"
  and $actual["d2-owner-julizi"][0] == "举例子"
  and $actual["d2-owner-shudu"][0] == "熟读"
  and $actual["d2-owner-zhenweisi"][0] == "甄维斯"
  and $actual["q1-frozen-xiangm"][0] == "项目"
  and $actual["q1-frozen-shouji"][0] == "手机"
  and $actual["q1-frozen-pinyin"][0] == "拼音"
  and $actual["q1-frozen-zuoshenm"][0] == "做什么"
  and $actual["q1-frozen-wancl"][0] == "完成了"
  and $actual["q1-named-nanyong"][0] == "难用"
  and $actual["q1-named-chaojifan"][0] == "超级烦"
  and $actual["q2-cursor"][0] == "Cursor"
  and $actual["q2-copilot"][0] == "Copilot"
  and $actual["q2-prompt"][0] == "prompt"
  and $actual["q2-token"][0] == "token"
  and $actual["q2-embedding"][0] == "embedding"
' "$REPORT" >/dev/null

jq -e --arg checksum "$ARTIFACT_SHA" '
  .artifactChecksum == $checksum and .cases == 702 and .repetitions == 1 and
  .attemptedSessions == 702 and .createdSessions == 702 and .executedSessions == 702 and
  .candidatesMissingReadingKey == 0 and .top5Hits == 702 and
  .mustPassTop1Cases == 0 and .stableCases == 702 and (.failures | length) == 0
' "$ASCII_PREFIX_REPORT" >/dev/null

jq -c '.observations[] | {id, query, candidates: .actual}' "$REPORT"
jq -c '.observations[] | select(.query == "i" or .query == "u" or .query == "v" or .query == "in" or .query == "io" or .query == "sv" or .query == "ub" or .query == "ve" or .query == "vi" or .query == "vs" or .query == "vu") | {id, query, candidates: .actual}' "$ASCII_PREFIX_REPORT"
echo "ascii_prefix_position=PASS cases=702 sessions=702 chinese_required=621 no_chinese=81 stable=702/702 artifact_checksum=$ARTIFACT_SHA corpus_checksum=$ACTUAL_ASCII_PREFIX_SHA"
echo "offcorpus_probe=PASS cases=76 sessions=380 top5=76/76 must_top1=58/58 stable=76/76 artifact_checksum=$ARTIFACT_SHA corpus_checksum=$ACTUAL_CORPUS_SHA"
