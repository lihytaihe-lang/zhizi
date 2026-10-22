#!/usr/bin/env python3
"""Export tracked public files and scan them without recording matched text.

Usage:
    python3 scripts/tools/public_export.py --src REPO --out EMPTY_DIR \
        [--config CONFIG.json] [--report REPORT.json] [--dry-run]

Patterns are root-relative, slash-separated globs; ** matches zero or more
segments and a trailing / includes descendants. Bare exclude patterns match
at any depth. A segment !(a|b) matches every name except a and b (skin allowlist).
Scan rules contain name, regex pattern, optional target (text or filename), and
optional allow (exact matches ignored). Filename findings use line 0.
UTF BOM text and non-NUL text decoded as UTF-8 with replacement are scanned.
Binary NUL-containing files are copied without text scanning. Symlinks and
submodules are refused. Tracked files use current working-copy content.
Reports describe selected files, sizes in bytes, and rule locations only.
Blockers exit 1 before copying; errors exit 2. Dry runs print the JSON report
but write nothing, even when --report is provided. Normal runs also print JSON.
"""

import argparse
import codecs
import fnmatch
from functools import lru_cache
import json
from pathlib import Path, PurePosixPath
import re
import shutil
import subprocess
import sys


def matches(path, pattern, exclude=False):
    directory = pattern.endswith("/")
    pattern = pattern.rstrip("/")
    if exclude and "/" not in pattern:
        pattern = "**/" + pattern
    if directory:
        pattern += "/**"
    parts = tuple(path.split("/"))
    globs = tuple(pattern.split("/"))

    @lru_cache(None)
    def walk(i, j):
        if j == len(globs):
            return i == len(parts)
        glob = globs[j]
        if glob == "**":
            return walk(i, j + 1) or (i < len(parts) and walk(i + 1, j))
        if i == len(parts):
            return False
        if glob.startswith("!(") and glob.endswith(")"):
            hit = parts[i] not in glob[2:-1].split("|")
        else:
            hit = fnmatch.fnmatchcase(parts[i], glob)
        return hit and walk(i + 1, j + 1)

    return walk(0, 0)


def load_config(path):
    config = json.loads(path.read_text(encoding="utf-8"))
    for key in ("include", "exclude"):
        if not isinstance(config[key], list) or not all(
                isinstance(item, str) and item for item in config[key]):
            raise ValueError("invalid path patterns")
    rules = {}
    for severity in ("blocker", "warning"):
        rules[severity] = []
        for rule in config["scan"][severity]:
            if not isinstance(rule["name"], str) or not rule["name"]:
                raise ValueError("invalid rule name")
            target = rule.get("target", "text")
            allow = rule.get("allow", [])
            if target not in ("text", "filename") or not isinstance(allow, list):
                raise ValueError("invalid scan rule")
            rules[severity].append((rule["name"], re.compile(rule["pattern"]),
                                    target, allow))
    return config, rules


def text_lines(data):
    if data.startswith((codecs.BOM_UTF32_LE, codecs.BOM_UTF32_BE)):
        return data.decode("utf-32", errors="replace").splitlines()
    if data.startswith((codecs.BOM_UTF16_LE, codecs.BOM_UTF16_BE)):
        return data.decode("utf-16", errors="replace").splitlines()
    if b"\0" in data:
        return []
    return data.decode("utf-8-sig", errors="replace").splitlines()


STUB_NOTE = ("# Rows for this layer are distributed through the zhizi-lexicon repository,\n"
             "# not this source tree. The file stays as an empty shell so build lists and imports resolve.\n")


def stub_dictionary(data):
    """Keep a Rime dictionary's header (comments, YAML front matter, the '...' line); drop every data row."""
    text = data.decode("utf-8-sig")
    lines = text.splitlines(keepends=True)
    for index, line in enumerate(lines):
        if line.strip() == "...":
            head = lines[:index + 1]
            break
    else:
        raise ValueError("stub dictionary has no '...' separator")
    # Header comments come first; the note goes right after them, before the YAML front matter.
    split = next((i for i, line in enumerate(head) if not line.startswith("#")), len(head))
    return ("".join(head[:split]) + STUB_NOTE + "".join(head[split:])).encode("utf-8")


def export(args):
    src = args.src.resolve(strict=True)
    out = args.out.absolute()
    if out.is_symlink() or (out.exists() and
                            (not out.is_dir() or any(out.iterdir()))):
        raise ValueError("output directory must be absent or empty")
    config, rules = load_config(args.config)
    root = subprocess.run(["git", "-C", str(src), "rev-parse", "--show-toplevel"],
                          capture_output=True, check=True).stdout
    if Path(root.decode().strip()).resolve() != src:
        raise ValueError("source must be the repository root")
    tracked = subprocess.run(["git", "-C", str(src), "ls-files", "-z"],
                             capture_output=True, check=True).stdout
    names = sorted(set(name.decode("utf-8") for name in tracked.split(b"\0") if name))
    selected = [name for name in names
                if any(matches(name, pattern) for pattern in config["include"])
                and not any(matches(name, pattern, exclude=True)
                            for pattern in config["exclude"])]
    rename = config.get("rename", {})
    if not isinstance(rename, dict):
        raise ValueError("invalid rename mapping")
    for source, target in rename.items():
        for name in (source, target):
            if not isinstance(name, str) or not name or "\\" in name:
                raise ValueError("invalid rename path")
            path = PurePosixPath(name)
            if path.is_absolute() or ".." in path.parts or str(path) != name:
                raise ValueError("unsafe rename path")
        if source not in selected:
            raise ValueError("rename source missing or excluded")
    if len(set(rename.values())) != len(rename):
        raise ValueError("duplicate rename target")
    # Explicit mappings replace an existing destination, before scanning/copying.
    selected = [name for name in selected if name not in rename.values() or name in rename]
    destinations = {name: rename.get(name, name) for name in selected}
    stub_dicts = set(config.get("stub_dicts", []))
    if not all(isinstance(item, str) and item for item in stub_dicts):
        raise ValueError("invalid stub_dicts")
    if not stub_dicts <= set(selected):
        raise ValueError("stub_dicts entry missing or excluded")
    if args.report:
        report_path = args.report.resolve()
        if report_path == src or src in report_path.parents:
            raise ValueError("report must be outside the source repository")
        if any(report_path == (out / destinations[name]).resolve() for name in selected):
            raise ValueError("report would overwrite an exported file")
    report = {"file_count": len(selected), "total_size": 0,
              "files": sorted(destinations.values()), "rename": rename, "stubbed": sorted(stub_dicts), "blocker": [], "warning": []}
    for name in selected:
        relative = PurePosixPath(name)
        if relative.is_absolute() or ".." in relative.parts:
            raise ValueError("unsafe tracked path")
        path = src
        for part in relative.parts:
            path /= part
            if path.is_symlink():
                raise ValueError("selected path contains a symlink")
        if not path.is_file():
            raise ValueError("selected tracked entry is not a regular file")
        data = path.read_bytes()
        if name in stub_dicts:
            data = stub_dictionary(data)
        report["total_size"] += len(data)
        lines = text_lines(data)
        for severity, entries in rules.items():
            for rule_name, pattern, target, allow in entries:
                candidates = [(0, relative.name)] if target == "filename" else enumerate(lines, 1)
                for number, line in candidates:
                    if any(match.group() not in allow for match in pattern.finditer(line)):
                        report[severity].append({"file": name, "line": number,
                                                 "rule": rule_name})
    if not args.dry_run:
        if not report["blocker"]:
            out.mkdir(parents=True, exist_ok=True)
            for name in selected:
                destination = out / destinations[name]
                destination.parent.mkdir(parents=True, exist_ok=True)
                if name in stub_dicts:
                    destination.write_bytes(stub_dictionary((src / name).read_bytes()))
                    shutil.copystat(src / name, destination)
                else:
                    shutil.copy2(src / name, destination)
        if args.report:
            args.report.write_text(json.dumps(report, ensure_ascii=True, indent=2) + "\n",
                                   encoding="utf-8")
    print(json.dumps(report, ensure_ascii=True, indent=2))
    return 1 if report["blocker"] else 0


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--src", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--config", type=Path, default=Path(__file__).with_suffix(".json"))
    parser.add_argument("--report", type=Path)
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args()
    try:
        return export(args)
    except (OSError, ValueError, KeyError, TypeError, re.error, subprocess.SubprocessError):
        # Exception messages may contain private paths, config text, or Git output.
        print("public export failed: check repository, config, paths and permissions", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
