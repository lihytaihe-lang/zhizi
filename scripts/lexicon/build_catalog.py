"""Build the lexicon layer catalog and the optional-layer-only word membership.

Usage: python3 scripts/lexicon/build_catalog.py \
    [--rime native/resources/Rime] [--base <pinyin_simp.dict.yaml>] --out <dir>

Reads the fixed layer table below against a Rime dictionary directory plus the
mandatory base dictionary (pinyin_simp; when --base is omitted it is searched
under .build-deps/rime-pinyin-simp/*/ at the repository root and a missing base
is an error, because base words must participate in the membership check).
Writes two build resources used by the runtime candidate filter:

  lexicon_catalog.json    layer registry: id, Chinese name, one-line
                          description, group (core/extension/pack), default
                          enabled, row count; plus "unlisted" (agent_*.dict.yaml
                          files present but not in the table; not an error, the
                          core handles them separately) and "missing" (table
                          layers whose file is absent).
  lexicon_membership.tsv  "word<TAB>layer,layer" for every word that appears in
                          at least one optional (extension/pack) layer and in
                          no core layer; layer ids are sorted and lines are
                          ordered by word codepoints. The core drops a
                          candidate only when every layer listing its word is
                          switched off, so core and base words never appear.

Rows count the non-empty, non-comment lines after the "..." marker. Repeated
words (same or different readings) collapse to one membership entry. Identical
inputs produce byte-identical output.
"""
import argparse
import json
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
DEFAULT_RIME = REPO / "native/resources/Rime"
BASE_DIRNAME = ".build-deps/rime-pinyin-simp"
SUFFIX = ".dict.yaml"

# Fixed layer table from the lexicon manager spec; deliberately not a config file.
# The packs are off by default (owner 2026-10-17); change here and in DEFAULT_DISABLED in agent-ime-core/src/lexicon_layers.rs together
# (a core test compares them).
# (id, Chinese name, one-line description, group, default enabled)
LAYERS = (
    ("pinyin_simp", "基础词库", "通用汉语词汇", "core", True),
    ("agent_common_fix", "常用修正", "少量常见排序修正", "core", True),
    ("agent_daily", "日常修正", "少量日常用词修正", "core", True),
    ("agent_general", "通用词库", "常用词与短语", "core", True),
    ("agent_modern", "现代词", "微信、直播、外卖等现代日常词", "core", True),
    ("agent_spoken", "口语短语", "日常说话的整句与短语", "core", True),
    ("agent_newwords", "日常名词", "动物、食物、物品等日常名称", "core", True),
    ("agent_idioms", "成语", "常用成语", "extension", True),
    ("agent_thuocl_it", "IT 词汇", "编程与互联网常用词", "extension", True),
    ("agent_thuocl_caijing", "财经词汇", "财经金融常用词", "extension", True),
    ("agent_ai_names", "AI 产品名", "AI 工具与模型的中文名", "extension", True),
    ("agent_pack_poetry", "诗词", "古诗词名句与诗人", "pack", False),
    ("agent_pack_cad", "CAD 术语", "设计与绘图软件术语", "pack", False),
    ("agent_pack_industry", "行业术语", "工业与工程领域词条", "pack", False),
)


def find_default_base(repo=REPO):
    """Return the pinned pinyin_simp base dictionary below repo; sorted glob
    keeps the choice deterministic if several checkouts are present."""
    matches = sorted((repo / BASE_DIRNAME).glob("*/pinyin_simp.dict.yaml"))
    if not matches:
        raise FileNotFoundError(
            f"base dictionary not found under {repo}/{BASE_DIRNAME}/*/pinyin_simp.dict.yaml; pass --base")
    return matches[0]


def read_layer(path):
    """Return (data rows, word set) for one dictionary; the word is the first
    tab-separated column; comments and blank lines are skipped."""
    rows = 0
    words = set()
    in_body = False
    for line in path.read_text(encoding="utf-8-sig").splitlines():
        if not in_body:
            if line.strip() == "...":
                in_body = True
            continue
        stripped = line.strip()
        if not stripped or stripped.startswith("#"):
            continue
        rows += 1
        words.add(line.split("\t", 1)[0])
    if not in_body:
        raise ValueError(f"{path}: missing '...' data separator")
    return rows, words


def build_catalog(rime, base, out):
    """Write both outputs into out; return (catalog dict, membership word count)."""
    rime, base, out = Path(rime), Path(base), Path(out)
    if not rime.is_dir():
        raise ValueError(f"Rime directory does not exist: {rime}")
    if not base.is_file():
        raise ValueError(f"base dictionary not found: {base}")
    catalog_layers = []
    missing = []
    core_words = set()
    optional = {}
    for layer_id, name, description, group, default_enabled in LAYERS:
        path = base if layer_id == "pinyin_simp" else rime / (layer_id + SUFFIX)
        if not path.is_file():
            missing.append(layer_id)
            continue
        rows, words = read_layer(path)
        if rows == 0 and group != "core":
            # An optional layer shipped as an empty shell (public source tree: the rows come from the
            # lexicon repository) is not offered in the manager.
            missing.append(layer_id)
            continue
        catalog_layers.append({"id": layer_id, "name": name, "description": description,
                               "group": group, "defaultEnabled": default_enabled, "rows": rows})
        if group == "core":
            core_words |= words
        else:
            for word in words:
                optional.setdefault(word, set()).add(layer_id)
    membership = {word: layer_ids for word, layer_ids in optional.items()
                  if word not in core_words}
    listed = {layer[0] for layer in LAYERS}
    unlisted = sorted(path.name[:-len(SUFFIX)] for path in rime.glob("agent_*" + SUFFIX)
                      if path.name[:-len(SUFFIX)] not in listed)
    catalog = {"formatVersion": 1, "layers": catalog_layers,
               "unlisted": unlisted, "missing": missing}
    out.mkdir(parents=True, exist_ok=True)
    (out / "lexicon_catalog.json").write_bytes(
        (json.dumps(catalog, ensure_ascii=False, indent=2) + "\n").encode("utf-8"))
    lines = "".join(word + "\t" + ",".join(sorted(layer_ids)) + "\n"
                    for word, layer_ids in sorted(membership.items()))
    (out / "lexicon_membership.tsv").write_bytes(lines.encode("utf-8"))
    return catalog, len(membership)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--rime", type=Path, default=DEFAULT_RIME,
                        help="Rime dictionary directory (default: %(default)s)")
    parser.add_argument("--base", type=Path,
                        help="path to pinyin_simp.dict.yaml (default: search "
                             + BASE_DIRNAME + "/*/pinyin_simp.dict.yaml at the repository root)")
    parser.add_argument("--out", required=True, type=Path)
    args = parser.parse_args()
    try:
        base = args.base if args.base is not None else find_default_base()
        catalog, membership_words = build_catalog(args.rime, base, args.out)
    except (OSError, ValueError) as error:
        parser.exit(1, f"error: {error}\n")
    total_rows = sum(layer["rows"] for layer in catalog["layers"])
    print(f"{args.out / 'lexicon_catalog.json'}: {len(catalog['layers'])} layers, "
          f"{total_rows} rows, unlisted={catalog['unlisted']}, missing={catalog['missing']}")
    print(f"{args.out / 'lexicon_membership.tsv'}: {membership_words} words")


if __name__ == "__main__":
    main()
