#!/usr/bin/env python3
"""Install a merged personal Core ML model in the current user's private storage."""

import argparse
import ctypes
from datetime import datetime, timezone
import hashlib
import json
import math
import os
from pathlib import Path
import shutil
import stat
import sys
import tempfile


MODEL_NAME = 'zhizi30m_v2.mlmodelc'
WEIGHTS_PATH = Path(MODEL_NAME) / 'weights/weight.bin'


def personal_model_directory():
    # Mirror AppDelegate's home-relative primary Application Support root.
    return Path.home() / 'Library' / 'Application Support' / 'AgentInputMethod' / 'PersonalModel'


def _require_regular(path):
    mode = path.lstat().st_mode
    if stat.S_ISLNK(mode):
        raise ValueError('symlink is not allowed')
    if not stat.S_ISREG(mode):
        raise ValueError('model entry is not a regular file')


def _model_files(source):
    model = source / MODEL_NAME
    if source.is_symlink() or model.is_symlink():
        raise ValueError('symlink is not allowed')
    if not model.is_dir():
        raise ValueError('compiled model directory is missing')
    _require_regular(source / 'itos.json')
    files = [source / 'itos.json']

    def traversal_failed(error):
        raise error

    for directory, directories, names in os.walk(model, followlinks=False, onerror=traversal_failed):
        for name in directories:
            child = Path(directory) / name
            if child.is_symlink():
                raise ValueError('symlink is not allowed')
        for name in names:
            child = Path(directory) / name
            _require_regular(child)
            files.append(child)
    if source / WEIGHTS_PATH not in files:
        raise ValueError('compiled model weights are missing')
    return sorted(files)


def _base_directory(bundle):
    bundle = Path(bundle)
    if bundle.suffix.lower() == '.app':
        for path in [bundle, bundle / 'Contents', bundle / 'Contents/Resources']:
            if path.is_symlink():
                raise ValueError('symlink is not allowed')
        return bundle / 'Contents/Resources/ContextModel'
    return bundle


def _sha256(path):
    digest = hashlib.sha256()
    with path.open('rb') as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b''):
            digest.update(chunk)
    return digest.hexdigest()


def _copy_regular(source, target):
    descriptor = os.open(source, os.O_RDONLY | os.O_NOFOLLOW)
    with os.fdopen(descriptor, 'rb') as incoming:
        if not stat.S_ISREG(os.fstat(incoming.fileno()).st_mode):
            raise ValueError('model entry is not a regular file')
        with target.open('xb') as outgoing:
            shutil.copyfileobj(incoming, outgoing)


def _swap_directories(stage, destination):
    library = ctypes.CDLL(None, use_errno=True)
    if sys.platform == 'darwin':
        exchange = library.renamex_np
        exchange.argtypes = [ctypes.c_char_p, ctypes.c_char_p, ctypes.c_uint]
        exchange.restype = ctypes.c_int
        result = exchange(os.fsencode(stage), os.fsencode(destination), 2)  # RENAME_SWAP
    elif sys.platform.startswith('linux'):
        exchange = library.renameat2
        exchange.argtypes = [ctypes.c_int, ctypes.c_char_p, ctypes.c_int, ctypes.c_char_p, ctypes.c_uint]
        exchange.restype = ctypes.c_int
        result = exchange(-100, os.fsencode(stage), -100, os.fsencode(destination), 2)  # RENAME_EXCHANGE
    else:
        raise OSError('atomic directory exchange is unavailable')
    if result != 0:
        error = ctypes.get_errno()
        raise OSError(error, 'atomic directory exchange failed')


def install_model(model_dir, base_bundle, strength, destination=None):
    if not math.isfinite(strength) or not 0 <= strength <= 1:
        raise ValueError('strength must be between 0 and 1')
    source = Path(model_dir)
    base = _base_directory(base_bundle)
    destination = Path(destination) if destination is not None else personal_model_directory()
    files = _model_files(source)
    for path in [base, base / MODEL_NAME, base / MODEL_NAME / 'weights']:
        if path.is_symlink():
            raise ValueError('symlink is not allowed')
    _require_regular(base / 'itos.json')
    _require_regular(base / WEIGHTS_PATH)
    if (source / 'itos.json').read_bytes() != (base / 'itos.json').read_bytes():
        raise ValueError('itos.json differs from the bundled base')
    if destination.is_symlink() or destination.parent.is_symlink():
        raise ValueError('symlink is not allowed')
    if destination.exists() and not destination.is_dir():
        raise ValueError('personal model destination is not a directory')
    destination.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    stage = Path(tempfile.mkdtemp(prefix='.personal-model-', dir=destination.parent))
    try:
        for path in files:
            target = stage / path.relative_to(source)
            target.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
            _copy_regular(path, target)
        # Empty model subdirectories are harmless; only files form the manifest.
        for path in [stage, *stage.rglob('*')]:
            path.chmod(0o700 if path.is_dir() else 0o600)
        if (stage / 'itos.json').read_bytes() != (base / 'itos.json').read_bytes():
            raise ValueError('itos.json changed while staging')
        manifest = {
            'baseWeightsSha256': _sha256(base / WEIGHTS_PATH),
            'strength': strength,
            'createdAt': datetime.now(timezone.utc).isoformat().replace('+00:00', 'Z'),
            'files': {path.relative_to(source).as_posix(): _sha256(stage / path.relative_to(source)) for path in files},
        }
        manifest_path = stage / 'personal_model.json'
        manifest_path.write_text(json.dumps(manifest, indent=2, sort_keys=True) + '\n', encoding='utf-8')
        manifest_path.chmod(0o600)
        if destination.exists():
            _swap_directories(stage, destination)
        else:
            os.replace(stage, destination)
        return manifest
    finally:
        if stage.exists():
            shutil.rmtree(stage)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--model-dir', type=Path, required=True)
    parser.add_argument('--base-bundle', type=Path, required=True)
    parser.add_argument('--strength', type=float, required=True)
    arguments = parser.parse_args()
    try:
        install_model(arguments.model_dir, arguments.base_bundle, arguments.strength)
    except (OSError, ValueError):
        parser.exit(1, 'personal_model_install=FAIL\n')
    print('personal_model_install=PASS')


if __name__ == '__main__':
    main()
