#!/bin/bash
# Regenerate everything derived from the repository after ANY file change, in the order it depends on itself, and keep the project-terms
# hash in the two licence registries equal to the file (the project-term layer is extracted from the repo's own docs, so editing a registry
# can change it).  Run before a regression and before a commit that you want to leave the tree clean.
set -eu
cd "$(dirname "$0")/../.."
EVENTS="${ZHIZI_FROZEN_EVENTS:-$HOME/Library/Application Support/AgentInputMethod/Quality/Frozen/events-2026-07-24.jsonl}"
chain() {
  node scripts/generate_project_terms.mjs | tail -1 | cut -c1-120
  node scripts/generate_heavy_garble_words.mjs | tail -1 | cut -c1-120
  node scripts/generate_exact_reading_index.mjs | tail -1 | cut -c1-120
  node scripts/generate_ascii_prefix_probe.mjs | tail -1 | cut -c1-120
  for s in evaluate_owner_abbrev_competition evaluate_owner_abbrev_impact_full evaluate_owner_context_disambig; do
    node scripts/$s.mjs --events "$EVENTS" --write | tail -1 | cut -c1-140
  done
  node scripts/generate_candidate_skins.mjs | tail -1 | cut -c1-100
  lexicon_catalog
}
lexicon_catalog() {  # layer catalog for the lexicon window + the optional-layer word list the core compiles in
  local tmp; tmp="$(mktemp -d)"
  python3 scripts/lexicon/build_catalog.py --base "$(ls .build-deps/rime-pinyin-simp/*/pinyin_simp.dict.yaml | head -1)" --out "$tmp" | tail -1 | cut -c1-100
  cp "$tmp/lexicon_catalog.json" native/resources/Lexicon/lexicon_catalog.json
  cp "$tmp/lexicon_membership.tsv" native/core/agent-ime-core/resources/lexicon_membership.tsv
  rm -rf "$tmp"
}
sync_registry() {  # prints "changed" when it edited the registries
  python3 - <<'P'
import hashlib, re
file = "native/resources/Rime/agent_project_terms.dict.yaml"
new = hashlib.sha256(open(file, "rb").read()).hexdigest()
changed = False
for f in ("THIRD_PARTY_NOTICES.md", "docs/current/licenses/DEPENDENCY_REGISTRY.md"):
    text = open(f, encoding="utf-8").read()
    hit = re.search(r"agent_project_terms[^|]*?SHA-256 `([0-9a-f]{64})`|Project term dictionary \| [0-9.]+ / SHA-256 `([0-9a-f]{64})`", text)
    for old in {g for g in (hit.groups() if hit else ()) if g}:
        if old != new:
            open(f, "w", encoding="utf-8").write(text.replace(old, new)); changed = True
print("changed" if changed else "same")
P
}
for round in 1 2 3; do
  chain
  [ "$(sync_registry)" = "same" ] && { echo "derived chain stable after round $round"; exit 0; }
done
echo "derived chain did not settle in 3 rounds" >&2; exit 1
