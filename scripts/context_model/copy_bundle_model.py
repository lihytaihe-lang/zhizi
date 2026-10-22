"""Copy only checksum-attested local model files; unavailable is a build success."""
import hashlib
import json
from pathlib import Path
import shutil
import sys
import os

ROOT = Path(__file__).resolve().parents[2]
WORK_DIR = Path(os.environ.get("ZHIZI_WORK", ROOT / "ime_context_model"))

SOURCE = WORK_DIR / 'release/zhizi-30m-v3'
PUBLIC_MANIFEST = ROOT / 'native/resources/ContextModel/SHA256SUMS.public'
MODEL_CARD = ROOT / 'native/resources/ContextModel/model_card.json'
MAX_CUSTOM_BYTES = 512 * 1024 * 1024
# Observed 372 B in three local compiles and in the maintainer's pinned copy;
# bounds are loose sanity limits, not an integrity check (integrity of weights
# and graph is pinned by hash).
PUBLIC_COREMLDATA_MIN_BYTES = 256
PUBLIC_COREMLDATA_MAX_BYTES = 64 * 1024

MANIFEST = Path(__file__).resolve().parents[2] / 'native/resources/ContextModel/SHA256SUMS'


def copy_model(destination, source=SOURCE, manifest=MANIFEST):
    destination = Path(destination)
    shutil.rmtree(destination, ignore_errors=True)
    destination.mkdir(parents=True)
    stage = destination / '.staging'
    stage.mkdir()
    tier = os.environ.get('ZHIZI_MODEL_TIER', 'official')
    try:
        if tier not in ('official', 'public', 'custom'):
            raise ValueError('invalid ZHIZI_MODEL_TIER; expected official/public/custom')
        if tier == 'custom':
            if not os.environ.get('ZHIZI_MODEL_DIR'):
                raise ValueError('custom tier requires ZHIZI_MODEL_DIR')
            source = Path(os.environ['ZHIZI_MODEL_DIR'])
        else:
            source = Path(source)
        if tier != 'official':
            # Check every ancestor and descendant before following or copying files.
            if any(p.is_symlink() for p in (source, *source.parents)):
                raise ValueError('symlink in model source')
            artifacts = list(source.rglob('*'))
            if any(p.is_symlink() for p in artifacts):
                raise ValueError('symlink in model artifact')
            if sum(p.stat().st_size for p in artifacts if p.is_file()) > MAX_CUSTOM_BYTES:
                raise ValueError('model exceeds 512 MB size limit')
            required = ['itos.json', 'zhizi30m_v2.mlmodelc/coremldata.bin',
                        'zhizi30m_v2.mlmodelc/metadata.json', 'zhizi30m_v2.mlmodelc/model.mil',
                        'zhizi30m_v2.mlmodelc/weights/weight.bin',
                        'zhizi30m_v2.mlmodelc/analytics/coremldata.bin']
            if not all((source / n).is_file() for n in required):
                raise ValueError('required model file missing')
            vocab = json.loads((source / 'itos.json').read_text())
            if (not isinstance(vocab, list) or not all(isinstance(v, str) for v in vocab)
                    or vocab[:3] != ['<pad>', '<bos>', '<unk>'] or len(set(vocab)) != len(vocab)):
                raise ValueError('invalid itos vocabulary')
            card = json.loads(MODEL_CARD.read_text())
            if (not isinstance(card, dict) or not {'checkpoint', 'input', 'output', 'pad', 'bos', 'unk'} <= card.keys()
                    or card['input'] != 'tokens Int32[8,40]'
                    or card['output'] not in ('logprobs Float16[8,39]', 'logprobs Float32[8,39]')
                    or [card['pad'], card['bos'], card['unk']] != [0, 1, 2]):
                raise ValueError('invalid repository model card interface')
            if tier == 'public':
                size = (source / required[1]).stat().st_size
                if not PUBLIC_COREMLDATA_MIN_BYTES <= size <= PUBLIC_COREMLDATA_MAX_BYTES:
                    raise ValueError('coremldata.bin size outside [256 B, 64 KiB]')
                manifest = PUBLIC_MANIFEST
        entries = []
        lines = (manifest.read_text().splitlines() if tier != 'custom' else
                 [f'-  {n}' for n in required])
        for line in lines:
            digest, relative = line.split(None, 1)
            relative = relative.strip()
            path = Path(relative)
            if path.is_absolute() or '..' in path.parts or not path.parts:
                raise ValueError('invalid checksum manifest')
            original = source / path
            if any((source / Path(*path.parts[:i])).is_symlink() for i in range(1, len(path.parts)+1)):
                raise ValueError('symlink in model artifact')
            data = original.read_bytes()
            if tier != 'custom' and hashlib.sha256(data).hexdigest() != digest:
                raise ValueError('model checksum mismatch')
            target = stage / path
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(data)
            if tier != 'custom' and hashlib.sha256(target.read_bytes()).hexdigest() != digest:
                raise ValueError('copied model checksum mismatch')
            entries.append(relative)
        if tier == 'public' and set(entries) != set(required) - {required[1]}:
            raise ValueError('incomplete public model manifest')
        if tier == 'official' and not {'itos.json', 'model_card.json', 'zhizi30m_v2.mlmodelc/coremldata.bin',
                'zhizi30m_v2.mlmodelc/model.mil', 'zhizi30m_v2.mlmodelc/weights/weight.bin'} <= set(entries):
            raise ValueError('incomplete model manifest')
        if tier != 'official':
            (stage / 'model_card.json').write_bytes(MODEL_CARD.read_bytes())
            if tier == 'public':
                shutil.copy2(source / 'zhizi30m_v2.mlmodelc/coremldata.bin',
                             stage / 'zhizi30m_v2.mlmodelc/coremldata.bin')
        if tier != 'custom':
            (stage / 'SHA256SUMS').write_bytes(manifest.read_bytes())
        for item in stage.iterdir():
            item.rename(destination / item.name)
        status = {'available': True, 'reason': None}
        if tier != 'official':
            status.update(tier=tier, official=tier == 'public')
    except (OSError, ValueError) as error:
        for item in destination.iterdir():
            if item.is_dir():
                shutil.rmtree(item)
            else:
                item.unlink()
        status = {'available': False, 'reason': '本机模型文件缺失或校验失败；上下文模型排序不可用。'}
        print(f'context_model_bundle=SKIP reason={type(error).__name__}: {error}')
    finally:
        shutil.rmtree(stage, ignore_errors=True)
    (destination / 'availability.json').write_text(json.dumps(status, ensure_ascii=False) + '\n')
    if status['available']:
        if tier == 'official':
            print('context_model_bundle=PASS sha256=verified')
        elif tier == 'public':
            print('context_model_bundle=PASS tier=public sha256=verified-partial')
        else:
            print('context_model_bundle=WARN tier=custom unofficial-model')
    return status


if __name__ == '__main__':
    copy_model(sys.argv[1])
