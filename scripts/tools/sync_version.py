#!/usr/bin/env python3
"""Write VERSION into every file that carries the user-visible release version, or check that they agree.

  python3 scripts/tools/sync_version.py --root <repo>            write
  python3 scripts/tools/sync_version.py --root <repo> --check    exit 1 and list the files that differ; writes nothing

Called through script/sync_version.sh.  Internal library crates keep their own versions on purpose.
"""
import argparse
import re
import sys
from pathlib import Path

# (file, pattern with the old version between group 1 and group 2, how many times it must match)
TARGETS = [
    ("native/macos/Sources/AgentInputMethodHostKit/ReleaseVersion.swift", r'(static let current = ")[^"]+(")', 1),
    ("native/macos/Resources/InputMethod-Info.plist", r'(<key>CFBundleShortVersionString</key>\s*<string>)[^<]+(</string>)', 1),
    ("package.json", r'("version": ")[^"]+(")', 1),
    ("package-lock.json", r'(\A\{\s*"name": "agent-input-layer",\s*"version": ")[^"]+(")', 1),
    ("package-lock.json", r'("packages": \{\s*"": \{\s*"name": "agent-input-layer",\s*"version": ")[^"]+(")', 1),
    ("src-tauri/tauri.conf.json", r'(\A\{[^}]*?"version": ")[^"]+(")', 1),
    ("src-tauri/Cargo.toml", r'(\A\[package\]\s*name = "agent-input-layer"\s*version = ")[^"]+(")', 1),
    ("src-tauri/Cargo.lock", r'(\[\[package\]\]\s*name = "agent-input-layer"\s*version = ")[^"]+(")', 1),
]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", required=True)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    root = Path(args.root)
    version = (root / "VERSION").read_text(encoding="utf-8").strip()
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", version):
        sys.exit(f"bad VERSION file: {version!r}")
    stale = []
    for rel, pattern, count in TARGETS:
        path = root / rel
        text = path.read_text(encoding="utf-8")
        new, n = re.subn(pattern, lambda m: m.group(1) + version + m.group(2), text, count=count)
        if n != count:
            sys.exit(f"{rel}: the version field was not found ({pattern[:40]}...)")
        if new != text:
            stale.append(rel)
            if not args.check:
                path.write_text(new, encoding="utf-8")
    if args.check and stale:
        print(f"version differs from VERSION ({version}): " + ", ".join(sorted(set(stale))), file=sys.stderr)
        sys.exit(1)
    changed = " (" + ", ".join(sorted(set(stale))) + ")" if stale else ""
    print(("in sync: " if args.check else "synced ") + version + changed)


if __name__ == "__main__":
    main()
