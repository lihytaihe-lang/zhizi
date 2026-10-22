import hashlib
from pathlib import Path
import tempfile
import unittest
from copy_bundle_model import copy_model


class CopyModelTests(unittest.TestCase):
    def test_missing_and_mismatched_files_disable_without_stale_model(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source, target = root / 'source', root / 'bundle'
            source.mkdir()
            target.mkdir()
            (target / 'itos.json').write_text('stale')
            manifest = root / 'SHA256SUMS'
            manifest.write_text('0' * 64 + '  itos.json\n')
            self.assertFalse(copy_model(target, source, manifest)['available'])
            self.assertFalse((target / 'itos.json').exists())
            (source / 'itos.json').write_text('corrupt')
            self.assertFalse(copy_model(target, source, manifest)['available'])

    def test_complete_checked_copy_and_symlink_rejection(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source, target = root / 'source', root / 'bundle'
            names = ['itos.json', 'model_card.json', 'zhizi30m_v2.mlmodelc/coremldata.bin',
                     'zhizi30m_v2.mlmodelc/model.mil', 'zhizi30m_v2.mlmodelc/weights/weight.bin']
            for name in names:
                path = source / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(name.encode())
            manifest = root / 'SHA256SUMS'
            manifest.write_text(''.join(f'{hashlib.sha256((source/n).read_bytes()).hexdigest()}  {n}\n' for n in names))
            self.assertTrue(copy_model(target, source, manifest)['available'])
            for name in names:
                self.assertEqual((source / name).read_bytes(), (target / name).read_bytes())
            (source / 'itos.json').unlink()
            (source / 'itos.json').symlink_to(source / 'model_card.json')
            self.assertFalse(copy_model(target, source, manifest)['available'])
            self.assertFalse((target / 'zhizi30m_v2.mlmodelc').exists())


if __name__ == '__main__':
    unittest.main()
