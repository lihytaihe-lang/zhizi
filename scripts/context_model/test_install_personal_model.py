import hashlib
import json
from pathlib import Path
import stat
import tempfile
import unittest
from unittest import mock

from scripts.context_model import install_personal_model as installer


class InstallPersonalModelTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.model = self.root / 'model'
        self.base = self.root / 'Base.app/Contents/Resources/ContextModel'
        self.destination = self.root / 'user/PersonalModel'
        self.files = {
            'itos.json': b'["a", "b"]\n',
            'zhizi30m_v2.mlmodelc/model.mil': b'fake compiled model',
            'zhizi30m_v2.mlmodelc/weights/weight.bin': b'personal weights',
            'zhizi30m_v2.mlmodelc/analytics/coremldata.bin': b'fake analytics',
        }
        for name, data in self.files.items():
            path = self.model / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
        (self.base / 'zhizi30m_v2.mlmodelc/weights').mkdir(parents=True)
        (self.base / 'itos.json').write_bytes(self.files['itos.json'])
        (self.base / 'zhizi30m_v2.mlmodelc/weights/weight.bin').write_bytes(b'base weights')

    def install(self, base=None):
        return installer.install_model(self.model, base or self.base, 0.3, self.destination)

    def test_replaces_existing_directory_in_one_swap_after_staging(self):
        self.install()
        (self.destination / 'old-only').write_bytes(b'old')
        self.files['zhizi30m_v2.mlmodelc/model.mil'] = b'new model'
        (self.model / 'zhizi30m_v2.mlmodelc/model.mil').write_bytes(b'new model')
        swap = installer._swap_directories

        def observe_swap(stage, destination):
            self.assertEqual((destination / 'old-only').read_bytes(), b'old')
            self.assertEqual((destination / 'zhizi30m_v2.mlmodelc/model.mil').read_bytes(), b'fake compiled model')
            for name, data in self.files.items():
                self.assertEqual((stage / name).read_bytes(), data)
            self.assertTrue((stage / 'personal_model.json').is_file())
            self.assertEqual(stage.parent, destination.parent)
            swap(stage, destination)

        with mock.patch.object(installer, '_swap_directories', side_effect=observe_swap) as exchange:
            self.install()
        exchange.assert_called_once()
        self.assertFalse((self.destination / 'old-only').exists())
        self.assertEqual((self.destination / 'zhizi30m_v2.mlmodelc/model.mil').read_bytes(), b'new model')
        self.assertEqual(list(self.destination.parent.iterdir()), [self.destination])

    def test_swap_failure_keeps_installed_model_and_removes_stage(self):
        self.install()
        before = (self.destination / 'personal_model.json').read_bytes()
        with mock.patch.object(installer, '_swap_directories', side_effect=OSError('swap failed')):
            with self.assertRaises(OSError):
                self.install()
        self.assertEqual((self.destination / 'personal_model.json').read_bytes(), before)
        self.assertEqual(list(self.destination.parent.iterdir()), [self.destination])

    def test_traversal_failure_does_not_replace_installed_model(self):
        self.install()
        before = (self.destination / 'personal_model.json').read_bytes()

        def unreadable_subdirectory(top, followlinks=False, onerror=None):
            if onerror is not None:
                onerror(PermissionError('unreadable model subtree'))
            return iter([
                (str(top), [], ['model.mil']),
                (str(Path(top) / 'weights'), [], ['weight.bin']),
            ])

        with mock.patch.object(installer.os, 'walk', side_effect=unreadable_subdirectory):
            with self.assertRaises(PermissionError):
                self.install()
        self.assertEqual((self.destination / 'personal_model.json').read_bytes(), before)
        self.assertEqual(list(self.destination.parent.iterdir()), [self.destination])

    def test_private_permissions_for_all_files_and_directories(self):
        self.install()
        for path in [self.destination, *self.destination.rglob('*')]:
            self.assertEqual(stat.S_IMODE(path.stat().st_mode), 0o700 if path.is_dir() else 0o600, path.name)

    def test_mismatched_itos_is_rejected_without_replacing(self):
        self.install()
        before = (self.destination / 'personal_model.json').read_bytes()
        (self.model / 'itos.json').write_bytes(b'["b", "a"]\n')
        with self.assertRaisesRegex(ValueError, 'itos'):
            self.install()
        self.assertEqual((self.destination / 'personal_model.json').read_bytes(), before)

    def test_manifest_contains_base_digest_strength_date_and_every_file(self):
        manifest = self.install(self.root / 'Base.app')
        self.assertEqual(manifest, json.loads((self.destination / 'personal_model.json').read_text()))
        self.assertEqual(manifest['baseWeightsSha256'], hashlib.sha256(b'base weights').hexdigest())
        self.assertEqual(manifest['strength'], 0.3)
        self.assertRegex(manifest['createdAt'], r'^\d{4}-\d{2}-\d{2}T.*Z$')
        self.assertEqual(manifest['files'], {name: hashlib.sha256(data).hexdigest() for name, data in self.files.items()})

    def test_symlink_is_rejected_before_installation(self):
        path = self.model / 'zhizi30m_v2.mlmodelc/model.mil'
        path.unlink()
        path.symlink_to(self.base / 'itos.json')
        with self.assertRaisesRegex(ValueError, 'symlink'):
            self.install()
        self.assertFalse(self.destination.exists())


if __name__ == '__main__':
    unittest.main()
