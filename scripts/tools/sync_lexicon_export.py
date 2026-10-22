#!/usr/bin/env python3
"""Re-sync a lexicon export directory with the files listed in its FILES.sha256.

Usage: python3 scripts/tools/sync_lexicon_export.py releases/zhizi-lexicon-YYYYMMDD [--dry-run]

FILES.sha256 lines read "<sha256>  <destination>  <-  <source path in this repository>". Every listed file is
copied from the repository again and the hashes are rewritten; comment lines (starting with "#") are kept.
A destination named in a "# <path> is the ONLY file intentionally modified" comment is left alone.
"""
import argparse
import hashlib
from pathlib import Path
import re
import shutil
import sys

REPO = Path(__file__).resolve().parents[2]
LINE = re.compile(r"^([0-9a-f]{64})  (\S+)  <-  (\S+)$")


def sha256(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def intentionally_modified(lines):
    return {match.group(1) for line in lines for match in [re.match(r"^# (\S+) is the ONLY file intentionally modified", line)] if match}


def sync(export, dry_run):
    sums = export / "FILES.sha256"
    lines = sums.read_text(encoding="utf-8").splitlines()
    keep = intentionally_modified(lines)
    out, changed = [], []
    for line in lines:
        match = LINE.match(line)
        if not match:
            out.append(line)
            continue
        _, destination, source = match.groups()
        target, origin = export / destination, REPO / source
        if destination in keep:
            out.append(f"{sha256(target)}  {destination}  <-  {source}")
            continue
        if not origin.is_file():
            raise ValueError(f"source missing: {source}")
        if not target.is_file() or sha256(target) != sha256(origin):
            changed.append(destination)
            if not dry_run:
                target.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(origin, target)
        out.append(f"{sha256(origin if destination not in keep else target)}  {destination}  <-  {source}")
    if not dry_run:
        sums.write_text("\n".join(out) + "\n", encoding="utf-8")
    return changed


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("export", type=Path)
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args()
    try:
        changed = sync(args.export.resolve(strict=True), args.dry_run)
    except (OSError, ValueError) as error:
        print(f"sync failed: {error}", file=sys.stderr)
        return 2
    for name in changed:
        print(("would update " if args.dry_run else "updated ") + name)
    print(f"{len(changed)} file(s) {'would change' if args.dry_run else 'updated'}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
