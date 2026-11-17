"""Copy the checksum-pinned word bigram table into the bundle; unavailable is a build success.

The core reads <shared Rime>/word-bigram/<tier>/ for sentence-level abbreviation.
Without the table it keeps the no-table lattice parameters, so a build without
ZHIZI_WORK (public export, CI) still works.
"""
import hashlib
import os
import shutil
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
WORK_DIR = Path(os.environ.get("ZHIZI_WORK", ROOT / "ime_context_model"))
SOURCE = WORK_DIR / "release/word-bigram-v1"
MANIFEST = ROOT / "native/resources/WordBigram/SHA256SUMS"


def copy_table(destination, source=SOURCE, manifest=MANIFEST):
    destination = Path(destination)
    shutil.rmtree(destination, ignore_errors=True)
    entries = []
    for line in Path(manifest).read_text().splitlines():
        if not line.strip():
            continue
        digest, relative = line.split(None, 1)
        path = Path(relative.strip())
        if path.is_absolute() or ".." in path.parts or len(path.parts) != 2:
            raise ValueError("invalid checksum manifest")
        entries.append((digest, path))
    staged = []
    for digest, path in entries:
        original = Path(source) / path
        if not original.is_file() or original.is_symlink():
            return f"word-bigram unavailable: {path} missing"
        data = original.read_bytes()
        if hashlib.sha256(data).hexdigest() != digest:
            return f"word-bigram unavailable: {path} checksum mismatch"
        staged.append((path, data))
    for path, data in staged:
        target = destination / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(data)
    return f"word-bigram available: {len(staged)} files"


if __name__ == "__main__":
    print(copy_table(sys.argv[1]))
