import contextlib
import hashlib
import io
import json
import os
import subprocess
import sys
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import copy_bundle_model as model

# Frozen pre-tier implementation: execute only against synthetic fixtures.
OLD_SOURCE = '"""Copy only checksum-attested local model files; unavailable is a build success."""\nimport hashlib\nimport json\nfrom pathlib import Path\nimport shutil\nimport sys\nimport os\n\nROOT = Path(__file__).resolve().parents[2]\nWORK_DIR = Path(os.environ.get("ZHIZI_WORK", ROOT / "ime_context_model"))\n\nSOURCE = WORK_DIR / \'release/zhizi-30m-v3\'\nMANIFEST = Path(__file__).resolve().parents[2] / \'native/resources/ContextModel/SHA256SUMS\'\n\n\ndef copy_model(destination, source=SOURCE, manifest=MANIFEST):\n    destination = Path(destination)\n    shutil.rmtree(destination, ignore_errors=True)\n    destination.mkdir(parents=True)\n    stage = destination / \'.staging\'\n    stage.mkdir()\n    try:\n        entries = []\n        for line in manifest.read_text().splitlines():\n            digest, relative = line.split(None, 1)\n            relative = relative.strip()\n            path = Path(relative)\n            if path.is_absolute() or \'..\' in path.parts or not path.parts:\n                raise ValueError(\'invalid checksum manifest\')\n            original = source / path\n            if any((source / Path(*path.parts[:i])).is_symlink() for i in range(1, len(path.parts)+1)):\n                raise ValueError(\'symlink in model artifact\')\n            data = original.read_bytes()\n            if hashlib.sha256(data).hexdigest() != digest:\n                raise ValueError(\'model checksum mismatch\')\n            target = stage / path\n            target.parent.mkdir(parents=True, exist_ok=True)\n            target.write_bytes(data)\n            if hashlib.sha256(target.read_bytes()).hexdigest() != digest:\n                raise ValueError(\'copied model checksum mismatch\')\n            entries.append(relative)\n        if not {\'itos.json\', \'model_card.json\', \'zhizi30m_v2.mlmodelc/coremldata.bin\',\n                \'zhizi30m_v2.mlmodelc/model.mil\', \'zhizi30m_v2.mlmodelc/weights/weight.bin\'} <= set(entries):\n            raise ValueError(\'incomplete model manifest\')\n        (stage / \'SHA256SUMS\').write_bytes(manifest.read_bytes())\n        for item in stage.iterdir():\n            item.rename(destination / item.name)\n        status = {\'available\': True, \'reason\': None}\n    except (OSError, ValueError) as error:\n        for item in destination.iterdir():\n            if item.is_dir():\n                shutil.rmtree(item)\n            else:\n                item.unlink()\n        status = {\'available\': False, \'reason\': \'本机模型文件缺失或校验失败；上下文模型排序不可用。\'}\n        print(f\'context_model_bundle=SKIP reason={type(error).__name__}: {error}\')\n    finally:\n        shutil.rmtree(stage, ignore_errors=True)\n    (destination / \'availability.json\').write_text(json.dumps(status, ensure_ascii=False) + \'\\n\')\n    if status[\'available\']:\n        print(\'context_model_bundle=PASS sha256=verified\')\n    return status\n\n\nif __name__ == \'__main__\':\n    copy_model(sys.argv[1])\n'


class TierTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        self.source = self.root / 'source'
        self.dest = self.root / 'bundle'
        self.names = ['itos.json', 'model_card.json', 'zhizi30m_v2.mlmodelc/coremldata.bin',
                      'zhizi30m_v2.mlmodelc/metadata.json', 'zhizi30m_v2.mlmodelc/model.mil',
                      'zhizi30m_v2.mlmodelc/weights/weight.bin',
                      'zhizi30m_v2.mlmodelc/analytics/coremldata.bin']
        for n in self.names:
            p = self.source / n
            p.parent.mkdir(parents=True, exist_ok=True)
            p.write_bytes(b'x' * 2048)
        (self.source / 'itos.json').write_text(json.dumps(['<pad>', '<bos>', '<unk>', '字']))
        self.manifest = self.root / 'official'
        self.public = self.root / 'public'
        lines = {n: f'{hashlib.sha256((self.source/n).read_bytes()).hexdigest()}  {n}\n' for n in self.names}
        self.manifest.write_text(''.join(lines.values()))
        self.public.write_text(''.join(v for n, v in lines.items() if n not in
                                     ('model_card.json', 'zhizi30m_v2.mlmodelc/coremldata.bin')))
        self.env = patch.dict(os.environ, {}, clear=True)
        self.env.start()
        self.addCleanup(self.env.stop)
        self.patches = patch.multiple(model, PUBLIC_MANIFEST=self.public)
        self.patches.start()
        self.addCleanup(self.patches.stop)

    def run_model(self, tier=None):
        if tier is not None:
            os.environ['ZHIZI_MODEL_TIER'] = tier
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            status = model.copy_model(self.dest, self.source, self.manifest)
        return status, output.getvalue()

    def test_default_byte_identical_to_frozen_implementation(self):
        namespace = {'__file__': str(Path(model.__file__)), '__name__': 'frozen_model'}
        exec(OLD_SOURCE, namespace)
        for case in ('valid', 'corrupt', 'missing', 'symlink'):
            with self.subTest(case=case):
                vocab = self.source / 'itos.json'
                if case == 'corrupt':
                    vocab.write_text('corrupt')
                elif case == 'missing':
                    vocab.unlink()
                elif case == 'symlink':
                    vocab.symlink_to(self.source / 'model_card.json')
                old_dest = self.root / 'old'
                output = io.StringIO()
                with contextlib.redirect_stdout(output):
                    old_status = namespace['copy_model'](old_dest, self.source, self.manifest)
                status, stdout = self.run_model()
                self.assertEqual(status, old_status)
                # Normalize destination only for missing-file diagnostics.
                self.assertEqual(stdout, output.getvalue())
                old_files = {p.relative_to(old_dest): p.read_bytes() for p in old_dest.rglob('*') if p.is_file()}
                new_files = {p.relative_to(self.dest): p.read_bytes() for p in self.dest.rglob('*') if p.is_file()}
                self.assertEqual(new_files, old_files)
                explicit_status, explicit_stdout = self.run_model('official')
                self.assertEqual(explicit_status, old_status)
                self.assertEqual(explicit_stdout, output.getvalue())
                os.environ.pop('ZHIZI_MODEL_TIER')

    def test_default_cli_exit_and_complete_output_match_old(self):
        old_script = self.root / 'old_cli.py'
        old_script.write_text("namespace = {'__file__': " + repr(str(Path(model.__file__))) +
                              ", '__name__': 'frozen'}\nexec(" + repr(OLD_SOURCE) +
                              ", namespace)\nnamespace['copy_model'](" + repr(str(self.dest)) +
                              ", __import__('pathlib').Path(" + repr(str(self.source)) +
                              "), __import__('pathlib').Path(" + repr(str(self.manifest)) + "))\n")
        current = ("import sys; sys.path.insert(0, " + repr(str(Path(model.__file__).parent)) +
                   "); from copy_bundle_model import copy_model; from pathlib import Path; copy_model(" +
                   repr(str(self.dest)) + ", Path(" + repr(str(self.source)) + "), Path(" +
                   repr(str(self.manifest)) + "))")
        for missing in (False, True):
            if missing:
                (self.source / 'itos.json').unlink()
            old = subprocess.run([sys.executable, str(old_script)], capture_output=True, env={'PYTHONDONTWRITEBYTECODE': '1'})
            new = subprocess.run([sys.executable, '-c', current], capture_output=True, env={'PYTHONDONTWRITEBYTECODE': '1'})
            self.assertEqual(new.returncode, 0)
            self.assertEqual((new.returncode, new.stdout, new.stderr),
                             (old.returncode, old.stdout, old.stderr))

    def test_public_variable_metadata_and_every_pinned_file(self):
        (self.source / 'zhizi30m_v2.mlmodelc/coremldata.bin').write_bytes(b'y' * 2048)
        status, output = self.run_model('public')
        self.assertTrue(status['available'])
        self.assertTrue(status['official'])
        self.assertIn('PASS tier=public sha256=verified-partial', output)
        self.assertEqual((self.dest / 'model_card.json').read_bytes(), model.MODEL_CARD.read_bytes())
        for line in self.public.read_text().splitlines():
            n = line.split(None, 1)[1]
            p = self.source / n
            original = p.read_bytes()
            p.write_bytes(original[:-1] + bytes([original[-1] ^ 1]))
            self.assertFalse(self.run_model('public')[0]['available'], n)
            p.write_bytes(original)

    def test_public_missing_metadata(self):
        (self.source / 'zhizi30m_v2.mlmodelc/coremldata.bin').unlink()
        self.assertFalse(self.run_model('public')[0]['available'])

    def test_custom_valid_and_required_directory(self):
        self.assertFalse(self.run_model('custom')[0]['available'])
        os.environ['ZHIZI_MODEL_DIR'] = str(self.source)
        status, output = self.run_model('custom')
        self.assertTrue(status['available'])
        self.assertFalse(status['official'])
        self.assertIn('WARN tier=custom unofficial-model', output)

    def test_custom_invalid_vocab_symlink_size(self):
        os.environ['ZHIZI_MODEL_DIR'] = str(self.source)
        p = self.source / 'itos.json'
        for vocab in (['wrong', '<bos>', '<unk>'], ['<pad>', '<bos>', '<unk>', '<unk>'], [0, 1, 2]):
            p.write_text(json.dumps(vocab))
            self.assertIn('invalid itos vocabulary', self.run_model('custom')[1])
        p.write_text(json.dumps(['<pad>', '<bos>', '<unk>']))
        link = self.source / 'link'
        link.symlink_to(p)
        self.assertIn('symlink', self.run_model('custom')[1])
        link.unlink()
        huge = self.source / 'huge'
        with huge.open('wb') as f:
            f.truncate(512 * 1024 * 1024 + 1)
        self.assertIn('512 MB', self.run_model('custom')[1])

    def test_public_metadata_size_bounds(self):
        metadata = self.source / 'zhizi30m_v2.mlmodelc/coremldata.bin'
        for size in (372, 256, 64 * 1024):
            with self.subTest(size=size):
                metadata.write_bytes(b'x' * size)
                status, output = self.run_model('public')
                self.assertTrue(status['available'], output)
                self.assertIn('PASS tier=public sha256=verified-partial', output)
        for size in (255, 64 * 1024 + 1, 0):
            with self.subTest(size=size):
                metadata.write_bytes(b'x' * size)
                status, output = self.run_model('public')
                self.assertFalse(status['available'])
                self.assertIn('size outside', output)
        metadata.unlink()
        status, output = self.run_model('public')
        self.assertFalse(status['available'])
        self.assertIn('required model file missing', output)
        metadata.symlink_to(self.source / 'zhizi30m_v2.mlmodelc/metadata.json')
        status, output = self.run_model('public')
        self.assertFalse(status['available'])
        self.assertIn('symlink', output)

    def test_public_invalid_card(self):
        card = self.root / 'bad-card.json'
        card.write_text('{}')
        with patch.object(model, 'MODEL_CARD', card):
            self.assertIn('invalid repository model card', self.run_model('public')[1])

    def test_custom_missing_compiled_file(self):
        os.environ['ZHIZI_MODEL_DIR'] = str(self.source)
        (self.source / 'zhizi30m_v2.mlmodelc/model.mil').unlink()
        self.assertIn('required model file missing', self.run_model('custom')[1])

    def test_invalid_tier(self):
        status, output = self.run_model('other')
        self.assertFalse(status['available'])
        self.assertIn('invalid ZHIZI_MODEL_TIER', output)

    def test_tiers_do_not_leak_into_later_official(self):
        for tier in ('public', 'custom'):
            with patch.dict(os.environ, {'ZHIZI_MODEL_TIER': tier, 'ZHIZI_MODEL_DIR': str(self.source)}):
                self.assertTrue(self.run_model()[0]['available'])
            status, output = self.run_model()
            self.assertEqual(status, {'available': True, 'reason': None})
            self.assertEqual(output, 'context_model_bundle=PASS sha256=verified\n')

    def test_public_manifest_matches_official(self):
        root = Path(model.__file__).resolve().parents[2]
        official = (root / 'native/resources/ContextModel/SHA256SUMS').read_text().splitlines()
        public = (root / 'native/resources/ContextModel/SHA256SUMS.public').read_text().splitlines()
        self.assertEqual(len(public), 5)
        self.assertTrue(set(public) <= set(official))


if __name__ == '__main__':
    unittest.main()
