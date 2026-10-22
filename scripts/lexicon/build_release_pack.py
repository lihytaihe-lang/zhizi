"""Build a reproducible monthly lexicon directory and tar.gz using only stdlib.

Usage: python3 scripts/lexicon/build_release_pack.py --rime native/resources/Rime \
    --provenance quality/corpus --version 2026.10 --out /tmp/lexicon-output \
    [--layers agent_general,agent_newwords]

The default layer set is every agent_*.dict.yaml in --rime. Dictionary bytes
are preserved; plain TSVs contain exactly three columns after the ... marker.
MANIFEST sha256 values refer to the original dictionaries. Source and license
are cells from the layer audit's summary table; a "同上" (ditto) cell is replaced by the row above it.
Both are TODO if the layer has no provenance file; unavailable audit cells are
also TODO. This records evidence availability, not release approval. Provenance
is checked by filename, not certified or included in the package.

The audit path is relative to this script, independent of the working directory.
Existing release directories/archives are refused. Tar metadata, member order,
and gzip metadata are fixed; identical inputs produce identical archive bytes
with the same Python/zlib implementation.
"""
import argparse
import gzip
import hashlib
import io
import json
from pathlib import Path
import re
import tarfile
import tempfile


AUDIT_PATH = (Path(__file__).resolve().parents[2] / "docs/current/licenses" /
              "LEXICON-LAYER-AUDIT-2026-10-16.md")


# Layers whose per-row provenance file does not carry the layer's own name (the THUOCL tables are tracked by their upstream names).
# A layer with no data rows (agent_pinyin only imports the others) has nothing to document.
PROVENANCE_ALIASES = {
    "agent_idioms": ("thuocl-idiom-provenance.tsv",),
    "agent_thuocl_it": ("thuocl-it-provenance.tsv",),
    "agent_thuocl_caijing": ("thuocl-caijing-provenance.tsv",),
}


def audit_descriptions(path):
    """Read exact source/license cells from the audit's summary table."""
    if not path.is_file():
        return {}
    descriptions = {}
    columns = None
    previous = ("TODO", "TODO")
    for line in path.read_text(encoding="utf-8").splitlines():
        if not line.strip().startswith("|"):
            columns = None
            continue
        cells = [cell.strip() for cell in line.strip().strip("|").split("|")]
        if all(label in cells for label in ("层", "内容来源", "许可状态")):
            columns = {label: cells.index(label) for label in ("层", "内容来源", "许可状态")}
            continue
        if columns is None or len(cells) <= max(columns.values()):
            continue
        match = re.match(r"`(agent_[A-Za-z0-9_]+)`", cells[columns["层"]])
        if match:
            # "同上" ("ditto") points at the row above; a public MANIFEST must say what it refers to.
            resolved = []
            for label, above in (("内容来源", previous[0]), ("许可状态", previous[1])):
                cell = cells[columns[label]] or "TODO"
                resolved.append(above + cell[len("同上"):] if cell.startswith("同上") else cell)
            previous = tuple(resolved)
            descriptions[match.group(1)] = previous
    return descriptions


def plain_rows(data, path):
    """Extract data lines, rejecting malformed columns with file/line context."""
    rows = []
    in_body = False
    for number, line in enumerate(data.decode("utf-8-sig").splitlines(), 1):
        if not in_body:
            if line.strip() == "...":
                in_body = True
            continue
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        cells = line.split("\t")
        if len(cells) != 3:
            raise ValueError(f"{path}:{number}: expected 3 tab-separated columns, got {len(cells)}")
        rows.append(line)
    if not in_body:
        raise ValueError(f"{path}: missing '...' data separator")
    return rows


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def readme(version, layers):
    lines = [f"# 知字词库 {version}", "", "| 层 | 条数 |", "|---|---:|"]
    lines.extend(f"| {layer['name']} | {layer['rows']} |" for layer in layers)
    lines.extend([
        "", "## 安装与使用", "",
        "Rime 用户：将 layers/ 中需要的 .dict.yaml 文件复制到 Rime 用户目录，"
        "在方案所用词典的 import_tables 列表中加入相应层名（不含 .dict.yaml 后缀），然后重新部署。"
        "含聚合导入表时，其引用的其他词典也须安装；外部基础表需另行准备。",
        "", "（尚未提供 plum 配方；暂请按上面的方式手动安装。）", "",
        "纯文本用户：plain/ 下每层一个 UTF-8 TSV 文件，无表头，每行依次为词、拼音、权重，"
        "以制表符分隔，可按此顺序导入自己的工具。", "", "## 许可与来源", "",
        "各层许可与来源逐字取自仓库许可审计总表，详情见 MANIFEST.json。"
        "TODO 表示缺少 provenance 或审计说明，需补齐后再决定分发；"
        "审计中的“同上”等引用亦保留原文，需回查审计文档。"
        "本包不会将许可状态描述转换为统一开源许可证，也不代表发布审批已完成。", ""])
    for layer in layers:
        lines.extend([f"- {layer['name']}：{layer['license']}",
                      f"  来源：{layer['source']}"])
    return "\n".join(lines) + "\n"


def write_archive(directory, archive):
    """Write sorted tar members with normalized tar and gzip metadata."""
    with archive.open("wb") as raw:
        with gzip.GzipFile(filename="", mode="wb", fileobj=raw, mtime=0) as compressed:
            with tarfile.open(fileobj=compressed, mode="w", format=tarfile.USTAR_FORMAT) as tar:
                for path in [directory] + sorted(directory.rglob("*")):
                    info = tarfile.TarInfo(path.relative_to(directory.parent).as_posix())
                    info.mtime = 0
                    info.mode = 0o755 if path.is_dir() else 0o644
                    if path.is_dir():
                        info.type = tarfile.DIRTYPE
                        tar.addfile(info)
                    else:
                        data = path.read_bytes()
                        info.size = len(data)
                        tar.addfile(info, io.BytesIO(data))


def build_release_pack(rime, provenance, version, out, layers=None, audit_path=AUDIT_PATH):
    """Build a release and return its directory; audit_path supports fixture audits."""
    rime, provenance, out, audit_path = map(Path, (rime, provenance, out, audit_path))
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]*", version):
        raise ValueError("version must contain only letters, digits, dots, underscores or hyphens")
    if not rime.is_dir():
        raise ValueError(f"Rime directory does not exist: {rime}")
    if layers is None:
        layers = [path.name[:-len(".dict.yaml")] for path in rime.glob("agent_*.dict.yaml")]
    layers = sorted(set(layers))
    if not layers:
        raise ValueError("no agent layers selected")
    for name in layers:
        if not re.fullmatch(r"agent_[A-Za-z0-9_]+", name):
            raise ValueError(f"invalid layer name: {name!r}")
    target = out / f"zhizi-lexicon-{version}"
    archive = out / f"{target.name}.tar.gz"
    for path in (target, archive):
        if path.exists():
            raise FileExistsError(f"output already exists: {path}")
    descriptions = audit_descriptions(audit_path)
    manifest = []
    files = {}
    for name in layers:
        path = rime / f"{name}.dict.yaml"
        data = path.read_bytes()
        rows = plain_rows(data, path)
        has_provenance = not rows or any((provenance / filename).is_file() for filename in (
            f"{name.replace('_', '-')}-provenance.tsv", f"{name}-provenance.tsv",
            *PROVENANCE_ALIASES.get(name, ())))
        source, license_note = descriptions.get(name, ("TODO", "TODO")) if has_provenance else ("TODO", "TODO")
        manifest.append({"name": name, "rows": len(rows), "sha256": sha256(data),
                         "license": license_note, "source": source})
        files[f"layers/{name}.dict.yaml"] = data
        files[f"plain/{name}.tsv"] = ("".join(row + "\n" for row in rows)).encode("utf-8")
    files["MANIFEST.json"] = (json.dumps({"version": version, "layers": manifest},
                                         ensure_ascii=False, indent=2) + "\n").encode("utf-8")
    files["README.md"] = readme(version, manifest).encode("utf-8")
    files["SHA256SUMS"] = "".join(f"{sha256(data)}  {name}\n"
                                   for name, data in sorted(files.items())).encode("utf-8")
    out.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".lexicon-pack-", dir=out) as staging:
        directory = Path(staging) / target.name
        directory.mkdir()
        for name, data in sorted(files.items()):
            path = directory / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
        staged_archive = Path(staging) / archive.name
        write_archive(directory, staged_archive)
        directory.rename(target)
        staged_archive.rename(archive)
    return target


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--rime", required=True, type=Path)
    parser.add_argument("--provenance", required=True, type=Path)
    parser.add_argument("--version", required=True)
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("--layers", help="comma-separated layer names; default: all agent_*.dict.yaml")
    args = parser.parse_args()
    layers = None if args.layers is None else [name.strip() for name in args.layers.split(",")]
    try:
        directory = build_release_pack(args.rime, args.provenance, args.version, args.out, layers)
    except (OSError, ValueError) as error:
        parser.exit(1, f"error: {error}\n")
    print(directory)
    print(directory.with_name(directory.name + ".tar.gz"))


if __name__ == "__main__":
    main()
