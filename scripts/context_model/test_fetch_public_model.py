import hashlib
import http.server
import io
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import threading
import unittest
from unittest import mock

import fetch_public_model as fetch_model


FIXTURE_DOWNLOADS = {
    'itos.json': b'{"i": "tos"}',
    'zhizi30m_v2.mlpackage/Manifest.json': b'{"manifest": true}',
    'zhizi30m_v2.mlpackage/Data/com.apple.CoreML/model.mlmodel': b'MLMODEL',
    'zhizi30m_v2.mlpackage/Data/com.apple.CoreML/weights/weight.bin': b'W' * 4096,
}

FIXTURE_MLMODELC = {
    'metadata.json': b'{"meta": 1}',
    'model.mil': b'MIL',
    'coremldata.bin': b'conversion-date-and-uuid',
    'weights/weight.bin': b'w' * 64,
    'analytics/coremldata.bin': b'analytics',
}

FIXTURE_MODEL_CARD = b'{"card": "fixture"}'

COMPILER_SCRIPT = """import pathlib
import sys

assert sys.argv[1:2] == ['compile']
source = pathlib.Path(sys.argv[2])
output = pathlib.Path(sys.argv[3]) / 'zhizi30m_v2.mlmodelc'
assert (source / 'Manifest.json').is_file()
files = {name: bytes(value, 'utf-8') for name, value in %r.items()}
for name, data in files.items():
    path = output / name
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)
"""

FAILING_COMPILER_SCRIPT = """import sys
sys.exit(3)
"""


class FakeDownloader:
    def __init__(self, files, tamper=None):
        self.files = {name: bytes(data) for name, data in files.items()}
        self.tamper = tamper
        self.calls = []

    def __call__(self, url, dest_path, expected_sha256, max_bytes, timeout):
        relative = None
        for name in self.files:
            if url.endswith('/' + name):
                relative = name
                break
        if relative is None:
            raise fetch_model.FetchError(f'no fixture for url: {url}')
        self.calls.append(relative)
        data = self.files[relative]
        if self.tamper is not None:
            data = self.tamper(relative, data)
        Path(dest_path).parent.mkdir(parents=True, exist_ok=True)
        Path(dest_path).write_bytes(data)
        fetch_model.verify_file(dest_path, expected_sha256)


def sha256(data):
    return hashlib.sha256(bytes(data)).hexdigest()


def write_manifest(path, entries):
    lines = []
    for relative, digest in entries.items():
        lines.append(f'{digest}  {relative}')
    path.write_text('\n'.join(lines) + '\n')


class Fixture:
    def __init__(self, root):
        self.root = root
        self.downloads = root / 'downloads'
        for name, data in FIXTURE_DOWNLOADS.items():
            path = self.downloads / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
        self.hf_manifest = root / 'HF_SHA256SUMS'
        write_manifest(self.hf_manifest, {name: sha256(data) for name, data in FIXTURE_DOWNLOADS.items()})
        self.model_card = root / 'model_card.json'
        self.model_card.write_bytes(FIXTURE_MODEL_CARD)
        self.compiler = root / 'fake_compiler.py'
        self.compiler.write_text(COMPILER_SCRIPT % {k: v.decode() for k, v in FIXTURE_MLMODELC.items()})
        self.compiler.chmod(0o755)

    def official_manifest(self, overrides=None):
        entries = {
            'itos.json': sha256(FIXTURE_DOWNLOADS['itos.json']),
            'model_card.json': sha256(FIXTURE_MODEL_CARD),
        }
        for name, data in FIXTURE_MLMODELC.items():
            entries[f'zhizi30m_v2.mlmodelc/{name}'] = sha256(data)
        for relative, digest in (overrides or {}).items():
            entries[relative] = digest
        path = self.root / 'official_SHA256SUMS'
        write_manifest(path, entries)
        return path

    def run(self, dest, **kwargs):
        parameters = {
            'hf_manifest': self.hf_manifest,
            'model_card': self.model_card,
            'compiler': [sys.executable, str(self.compiler)],
        }
        parameters.update(kwargs)
        if 'official_manifest' not in parameters:
            parameters['official_manifest'] = self.official_manifest()
        return fetch_model.fetch_public_model(dest, **parameters)


class FetchPublicModelTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.fixture = Fixture(self.root)
        self.work = self.root / 'work'
        self.work.mkdir()
        self.dest = self.work / 'release' / 'zhizi-30m-v3'

    def tearDown(self):
        self.temporary.cleanup()

    def staging_leftovers(self):
        return sorted(self.work.glob(f'.{self.dest.name}.fetch-tmp-*'))

    def test_success_produces_layout_and_second_run_refuses(self):
        downloader = FakeDownloader(FIXTURE_DOWNLOADS)
        with mock.patch('sys.stdout', new=io.StringIO()) as stdout:
            self.fixture.run(self.dest, downloader=downloader)
        expected = {
            'itos.json': FIXTURE_DOWNLOADS['itos.json'],
            'model_card.json': FIXTURE_MODEL_CARD,
        }
        for name, data in FIXTURE_MLMODELC.items():
            expected[f'zhizi30m_v2.mlmodelc/{name}'] = data
        for relative, data in expected.items():
            self.assertEqual((self.dest / relative).read_bytes(), data, relative)
        self.assertIn(f'context_model_fetch=OK revision={fetch_model.REVISION}', stdout.getvalue())
        self.assertIn('Build next with ZHIZI_MODEL_TIER=public', stdout.getvalue())
        self.assertEqual(sorted(downloader.calls), sorted(FIXTURE_DOWNLOADS))
        self.assertEqual(self.staging_leftovers(), [])
        with self.assertRaises(fetch_model.FetchError) as refused:
            self.fixture.run(self.dest, downloader=downloader)
        self.assertIn('already exists', str(refused.exception))
        self.assertEqual(len(downloader.calls), len(FIXTURE_DOWNLOADS))

    def test_any_download_hash_mismatch_fails_without_residue(self):
        downloader = FakeDownloader(FIXTURE_DOWNLOADS, tamper=lambda name, data: data[:-1] + b'X' if name.endswith('weight.bin') else data)
        with self.assertRaises(fetch_model.FetchError) as failed:
            self.fixture.run(self.dest, downloader=downloader)
        self.assertIn('checksum mismatch', str(failed.exception))
        self.assertFalse(self.dest.exists())
        self.assertFalse(self.dest.parent.exists())
        self.assertEqual(self.staging_leftovers(), [])

    def test_dest_exists_nonempty_refuses_before_downloading(self):
        self.dest.parent.mkdir(parents=True)
        self.dest.mkdir()
        (self.dest / 'precious.bin').write_bytes(b'owner data')
        downloader = FakeDownloader(FIXTURE_DOWNLOADS)
        with self.assertRaises(fetch_model.FetchError) as refused:
            self.fixture.run(self.dest, downloader=downloader)
        self.assertIn('already exists', str(refused.exception))
        self.assertEqual(downloader.calls, [])
        self.assertEqual((self.dest / 'precious.bin').read_bytes(), b'owner data')

    def test_compiler_failure_fails_without_residue(self):
        failing = self.root / 'failing_compiler.py'
        failing.write_text(FAILING_COMPILER_SCRIPT)
        downloader = FakeDownloader(FIXTURE_DOWNLOADS)
        with self.assertRaises(fetch_model.FetchError) as failed:
            self.fixture.run(self.dest, downloader=downloader, compiler=[sys.executable, str(failing)])
        self.assertIn('coremlcompiler failed', str(failed.exception))
        self.assertFalse(self.dest.exists())
        self.assertEqual(self.staging_leftovers(), [])

    def test_relative_dest_escape_rejected(self):
        downloader = FakeDownloader(FIXTURE_DOWNLOADS)
        with self.assertRaises(fetch_model.FetchError) as refused:
            self.fixture.run(self.work / 'a' / '..' / 'escaped', downloader=downloader)
        self.assertIn("'..'", str(refused.exception))
        self.assertEqual(downloader.calls, [])

    def test_coremldata_mismatch_warns_but_succeeds(self):
        manifest = self.fixture.official_manifest(overrides={
            'zhizi30m_v2.mlmodelc/coremldata.bin': '0' * 64,
        })
        downloader = FakeDownloader(FIXTURE_DOWNLOADS)
        with mock.patch('sys.stdout', new=io.StringIO()) as stdout:
            self.fixture.run(self.dest, downloader=downloader, official_manifest=manifest)
        output = stdout.getvalue()
        self.assertIn('context_model_fetch=WARN', output)
        self.assertIn('coremldata.bin', output)
        self.assertIn(f'context_model_fetch=OK revision={fetch_model.REVISION}', output)

    def test_model_card_mismatch_warns_but_succeeds(self):
        manifest = self.fixture.official_manifest(overrides={
            'model_card.json': '0' * 64,
        })
        downloader = FakeDownloader(FIXTURE_DOWNLOADS)
        with mock.patch('sys.stdout', new=io.StringIO()) as stdout:
            self.fixture.run(self.dest, downloader=downloader, official_manifest=manifest)
        output = stdout.getvalue()
        self.assertIn('context_model_fetch=WARN model_card.json', output)
        self.assertIn(f'context_model_fetch=OK revision={fetch_model.REVISION}', output)

    def test_other_compile_output_mismatch_fails(self):
        manifest = self.fixture.official_manifest(overrides={
            'zhizi30m_v2.mlmodelc/model.mil': '0' * 64,
        })
        downloader = FakeDownloader(FIXTURE_DOWNLOADS)
        with self.assertRaises(fetch_model.FetchError) as failed:
            self.fixture.run(self.dest, downloader=downloader, official_manifest=manifest)
        self.assertIn('model.mil', str(failed.exception))
        self.assertFalse(self.dest.exists())
        self.assertEqual(self.staging_leftovers(), [])


class UrlValidationTests(unittest.TestCase):
    def test_allowed_hosts(self):
        for url in (
            'https://huggingface.co/DabbleCove/shunshou-zhizi-30m/resolve/x/itos.json',
            'https://us.aws.cdn.hf.co/xet-bridge-us/payload?X=1',
            'https://cdn-lfs.huggingface.co/a/b',
            'https://cdn-lfs-us-1.hf.co/a/b',
        ):
            fetch_model.validate_url(url)

    def test_refused_hosts_and_schemes(self):
        for url in (
            'http://huggingface.co/x',
            'ftp://huggingface.co/x',
            'https://evil.example.com/x',
            'https://huggingface.co.evil.com/x',
            'https://not-hf.co/x',
            'https://us.aws.cdn.hf.co.evil.com/x',
            'https:///path',
            'file:///etc/passwd',
        ):
            with self.assertRaises(fetch_model.FetchError):
                fetch_model.validate_url(url)


class RedirectRefusalTests(unittest.TestCase):
    def test_redirect_to_non_whitelist_host_refused(self):
        class Redirect(http.server.BaseHTTPRequestHandler):
            def do_GET(self):
                self.send_response(302)
                self.send_header('Location', 'https://evil.example.com/payload')
                self.end_headers()

            def log_message(self, *args):
                pass

        server = http.server.HTTPServer(('127.0.0.1', 0), Redirect)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            url = f'http://127.0.0.1:{server.server_port}/itos.json'
            opener = urllib_request_opener()
            with self.assertRaises(fetch_model.FetchError) as refused:
                opener.open(url, timeout=10)
            self.assertIn('host not in allowlist', str(refused.exception))
        finally:
            server.shutdown()
            server.server_close()


class StreamCapTests(unittest.TestCase):
    def test_receive_stream_enforces_size_cap(self):
        response = io.BytesIO(b'x' * 100)
        with tempfile.TemporaryDirectory() as temporary:
            target = Path(temporary) / 'too-big'
            with self.assertRaises(fetch_model.FetchError) as failed:
                fetch_model.receive_stream(response, target, max_bytes=10)
            self.assertIn('size cap', str(failed.exception))


class CliTests(unittest.TestCase):
    def test_main_rejects_relative_escape_dest(self):
        with tempfile.TemporaryDirectory() as temporary:
            escape = str(Path(temporary) / 'a' / '..' / 'escaped')
            with mock.patch('sys.stderr', new=io.StringIO()):
                status = fetch_model.main(['--dest', escape])
            self.assertEqual(status, 1)


def urllib_request_opener():
    import urllib.request
    return urllib.request.build_opener(fetch_model.SafeRedirectHandler)


if __name__ == '__main__':
    unittest.main()
