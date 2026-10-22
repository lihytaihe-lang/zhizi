"""Download the public Shunshou-Zhizi context model, verify it, compile it locally.

Fetches the pinned Hugging Face revision, checks every file against
native/resources/ContextModel/HF_SHA256SUMS, compiles the .mlpackage with
xcrun coremlcompiler, and installs the layout copy_bundle_model.py expects.
"""
import argparse
import hashlib
import os
from pathlib import Path
import shutil
import subprocess
import sys
import urllib.parse
import urllib.request

ROOT = Path(__file__).resolve().parents[2]
REVISION = 'b74f57068819380bce70ab3bec22696a81a95c95'
BASE_URL = f'https://huggingface.co/DabbleCove/shunshou-zhizi-30m/resolve/{REVISION}'
HF_MANIFEST = ROOT / 'native/resources/ContextModel/HF_SHA256SUMS'
OFFICIAL_MANIFEST = ROOT / 'native/resources/ContextModel/SHA256SUMS'
MODEL_CARD = ROOT / 'native/resources/ContextModel/model_card.json'
DEFAULT_COMPILER = ['xcrun', 'coremlcompiler']
ALLOWED_HOSTS = ('huggingface.co',)
ALLOWED_HOST_SUFFIXES = ('.huggingface.co', '.hf.co')
CHUNK_BYTES = 256 * 1024
MAX_DOWNLOAD_BYTES = 128 * 1024 * 1024
TIMEOUT_SECONDS = 60
NON_REPRODUCIBLE = {
    'zhizi30m_v2.mlmodelc/coremldata.bin':
        'coremlcompiler embeds a conversion timestamp and UUIDs in this file, '
        'so the official pin can never match a local compile',
    'model_card.json':
        'the official SHA256SUMS pins the private release copy of this file; '
        'the repository copy in native/resources/ContextModel/ differs from it',
}


class FetchError(Exception):
    pass


def validate_url(url):
    parsed = urllib.parse.urlsplit(url)
    if parsed.scheme != 'https':
        raise FetchError(f'only https is allowed: {url}')
    host = (parsed.hostname or '').lower()
    if host in ALLOWED_HOSTS or host.endswith(ALLOWED_HOST_SUFFIXES):
        return
    raise FetchError(f'host not in allowlist: {host or url}')


class SafeRedirectHandler(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        try:
            validate_url(newurl)
        except FetchError as error:
            raise FetchError(f'refusing redirect from {req.full_url}: {error}') from error
        return super().redirect_request(req, fp, code, msg, headers, newurl)


def receive_stream(response, dest_path, max_bytes):
    digest = hashlib.sha256()
    total = 0
    with open(dest_path, 'wb') as target:
        while True:
            chunk = response.read(CHUNK_BYTES)
            if not chunk:
                break
            total += len(chunk)
            if total > max_bytes:
                raise FetchError(f'download exceeds size cap ({max_bytes} bytes)')
            digest.update(chunk)
            target.write(chunk)
    return digest.hexdigest()


def download_file(url, dest_path, expected_sha256, max_bytes=MAX_DOWNLOAD_BYTES,
                  timeout=TIMEOUT_SECONDS):
    validate_url(url)
    request = urllib.request.Request(url, headers={'User-Agent': 'zhizi-fetch/1.0'})
    opener = urllib.request.build_opener(SafeRedirectHandler())
    try:
        with opener.open(request, timeout=timeout) as response:
            actual = receive_stream(response, dest_path, max_bytes)
    except FetchError:
        raise
    except OSError as error:
        raise FetchError(f'download failed: {url}: {error}') from error
    if actual != expected_sha256:
        raise FetchError(f'checksum mismatch for {Path(dest_path).name}: expected {expected_sha256}, got {actual}')


def verify_file(path, expected_sha256):
    actual = hashlib.sha256(Path(path).read_bytes()).hexdigest()
    if actual != expected_sha256:
        raise FetchError(f'checksum mismatch for {Path(path).name}: expected {expected_sha256}, got {actual}')


def parse_manifest(path):
    entries = {}
    for line in Path(path).read_text().splitlines():
        if not line.strip():
            continue
        digest, relative = line.split(None, 1)
        relative = relative.strip().lstrip('./')
        candidate = Path(relative)
        if len(digest) != 64 or any(c not in '0123456789abcdef' for c in digest):
            raise FetchError(f'invalid digest in manifest: {line}')
        if candidate.is_absolute() or '..' in candidate.parts or not candidate.parts:
            raise FetchError(f'invalid path in manifest: {relative}')
        entries[relative] = digest
    if not entries:
        raise FetchError(f'empty manifest: {path}')
    return entries


def compile_mlpackage(mlpackage_dir, output_dir, compiler=None):
    if not (Path(mlpackage_dir) / 'Manifest.json').is_file():
        raise FetchError(f'missing Manifest.json in {mlpackage_dir}')
    command = list(compiler or DEFAULT_COMPILER) + ['compile', str(mlpackage_dir), str(output_dir)]
    result = subprocess.run(command, capture_output=True, text=True)
    if result.returncode != 0:
        detail = (result.stderr or result.stdout or '').strip()[:500]
        raise FetchError(f'coremlcompiler failed ({result.returncode}): {detail}')
    compiled = Path(output_dir) / 'zhizi30m_v2.mlmodelc'
    if not compiled.is_dir():
        raise FetchError('compiler produced no zhizi30m_v2.mlmodelc')
    return compiled


def verify_tree(root, manifest_path):
    matches, mismatches, missing = [], [], []
    for relative, digest in parse_manifest(manifest_path).items():
        path = Path(root) / relative
        if not path.is_file():
            missing.append(relative)
        elif hashlib.sha256(path.read_bytes()).hexdigest() == digest:
            matches.append(relative)
        else:
            mismatches.append(relative)
    return {'matches': matches, 'mismatches': mismatches, 'missing': missing}


def default_dest():
    work = os.environ.get('ZHIZI_WORK') or str(ROOT / 'ime_context_model')
    return str(Path(work) / 'release' / 'zhizi-30m-v3')


def fetch_public_model(dest, hf_manifest=HF_MANIFEST, official_manifest=OFFICIAL_MANIFEST,
                       model_card=MODEL_CARD, downloader=download_file, compiler=None,
                       max_bytes=MAX_DOWNLOAD_BYTES, timeout=TIMEOUT_SECONDS,
                       revision=REVISION, base_url=BASE_URL):
    dest = Path(dest)
    if '..' in dest.parts:
        raise FetchError(f"destination must not contain '..' parts: {dest}")
    dest = dest.absolute()
    if dest.exists() and (not dest.is_dir() or any(dest.iterdir())):
        raise FetchError(f'destination already exists and is not empty: {dest} (refusing to touch it)')
    entries = parse_manifest(hf_manifest)
    if not (Path(model_card)).is_file():
        raise FetchError(f'missing model card: {model_card}')
    created_parents = []
    probe = dest.parent
    while not probe.exists():
        created_parents.append(probe)
        probe = probe.parent
    dest.parent.mkdir(parents=True, exist_ok=True)
    staging = dest.parent / f'.{dest.name}.fetch-tmp-{os.getpid()}'
    if staging.exists():
        raise FetchError(f'staging directory already exists: {staging}')
    staging.mkdir()
    succeeded = False
    try:
        print(f'context_model_fetch: revision={revision}')
        print(f'context_model_fetch: dest={dest}')
        downloads = staging / 'downloads'
        for relative in sorted(entries):
            print(f'context_model_fetch: download {relative}')
            target = downloads / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            downloader(f'{base_url}/{relative}', target, entries[relative],
                       max_bytes=max_bytes, timeout=timeout)
        print('context_model_fetch: compile (xcrun coremlcompiler compile)')
        compiled = compile_mlpackage(downloads / 'zhizi30m_v2.mlpackage', staging / 'compiled', compiler)
        release = staging / 'release'
        release.mkdir()
        shutil.copyfile(downloads / 'itos.json', release / 'itos.json')
        shutil.copyfile(model_card, release / 'model_card.json')
        shutil.copytree(compiled, release / 'zhizi30m_v2.mlmodelc')
        report = verify_tree(release, official_manifest)
        problems = report['missing'] + [name for name in report['mismatches'] if name not in NON_REPRODUCIBLE]
        if problems:
            raise FetchError(f'compiled artifacts do not match the official manifest: {", ".join(sorted(problems))}')
        for name in report['mismatches']:
            print(f'context_model_fetch=WARN {name} differs from the official SHA256SUMS: '
                  f'{NON_REPRODUCIBLE[name]}. All reproducible artifacts matched.')
        if dest.is_dir():
            dest.rmdir()
        os.rename(release, dest)
        print(f'context_model_fetch=OK revision={revision} dest={dest}')
        print('Build next with ZHIZI_MODEL_TIER=public (see docs/public/MODEL.md).')
        succeeded = True
        return dest
    finally:
        shutil.rmtree(staging, ignore_errors=True)
        if not succeeded:
            for directory in created_parents:
                try:
                    directory.rmdir()
                except OSError:
                    break


def main(argv=None):
    parser = argparse.ArgumentParser(description='Fetch and compile the public context model.')
    parser.add_argument('--dest', default=default_dest(), help='target release directory (default: copy_bundle_model.py source)')
    args = parser.parse_args(argv)
    try:
        fetch_public_model(args.dest)
    except FetchError as error:
        print(f'context_model_fetch=FAIL reason={error}', file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    sys.exit(main())
