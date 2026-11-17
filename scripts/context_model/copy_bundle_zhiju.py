"""Copy the checksum-pinned 顺手·知句 model into the bundle; unavailable is a build success.

The shell loads <Resources>/ZhijuModel lazily the first time a sentence-level
abbreviation is typed; without it the feature is off. A build without ZHIZI_WORK
(public export, CI) therefore still works.
"""
import hashlib
import os
import shutil
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
WORK_DIR = Path(os.environ.get("ZHIZI_WORK", ROOT / "ime_context_model"))
SOURCE = WORK_DIR / "release/zhiju-041-general-cache"
MANIFEST = ROOT / "native/resources/ZhijuModel/SHA256SUMS"


def copy_model(destination, source=SOURCE, manifest=MANIFEST):
    destination = Path(destination)
    shutil.rmtree(destination, ignore_errors=True)
    staged = []
    for line in Path(manifest).read_text().splitlines():
        if not line.strip():
            continue
        digest, relative = line.split(None, 1)
        path = Path(relative.strip())
        if path.is_absolute() or ".." in path.parts or not path.parts:
            raise ValueError("invalid checksum manifest")
        original = Path(source) / path
        inside = [Path(source).joinpath(*path.parts[:i]) for i in range(1, len(path.parts) + 1)]
        if not original.is_file() or any(p.is_symlink() for p in inside):
            return f"zhiju model unavailable: {path} missing"
        data = original.read_bytes()
        if hashlib.sha256(data).hexdigest() != digest:
            return f"zhiju model unavailable: {path} checksum mismatch"
        staged.append((path, data))
    for path, data in staged:
        target = destination / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(data)
    return f"zhiju model available: {len(staged)} files"


if __name__ == "__main__":
    print(copy_model(sys.argv[1]))
