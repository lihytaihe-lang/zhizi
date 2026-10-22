"""Public synthetic parity gate through Swift's actual production scorer.

Only aggregate owner held-out results leave the private temporary directory.
The Python reference is eval_owner.ZhiziScorer with its inference window capped
at the exported 40 positions, keeping long-candidate truncation identical.
Cases run through the harness in batches (AIM_ALIGNMENT_BATCH, default 8000):
one Core ML process sustained only ~16k consecutive inferences before its
IOSurface pool was exhausted, so each batch gets a fresh process and the merged
agreement/latency results keep the single-pass semantics.
"""
import argparse
import datetime
import hashlib
import json
import math
import os
from pathlib import Path
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[3]
WORK_DIR = Path(os.environ.get("ZHIZI_WORK", ROOT / "ime_context_model"))
ALIGNMENT_BATCH = 8000

import torch
from eval_owner import ZhiziScorer

CHECKPOINT = WORK_DIR / 'clean-v2/soup/best.pt'  # the checkpoint the bundled Core ML model was exported from (2026-10-16)


def synthetic_cases():
    contexts = ['', '我现在', '明天我们', '下次我会', '这件事情', '这个项目需要', '文档已经',
                '天气很好，我们准备', '演示时请仔细', '这是一个公开的合成用例', '今天完成代码审查，之后',
                '写好说明后，我将', '这是公开文本。' * 8, '测试🙂组合e\u0301字符', '未知字符\u9fff']
    groups = [['在', '再', '载', '栽'], ['是', '事', '市', '式', '时'], ['吧', '把', '八', '巴'],
              ['知道', '指导'], ['做出', '作出'], ['实际', '世纪', '时机'],
              ['在' * 7, '再' * 7], ['在' * 8, '再' * 8]]
    result = []
    for context in contexts:
        for group in groups:
            # Rotations exercise beta=2 and group-relative ranks, not just LM argmax.
            for shift in range(len(group)):
                result.append({'context': context, 'candidates': group[shift:] + group[:shift]})
    return result


def heldout_cases():
    source = Path(os.environ.get('AIM_OWNER_QUALITY_EVENTS',
                  str(Path.home() / 'Library/Application Support/AgentInputMethod/Quality/events.jsonl')))
    if not source.is_file() or source.is_symlink():
        return None
    from pypinyin import pinyin, Style
    def readings(text):
        return [set(row) for row in pinyin(text, style=Style.NORMAL, heteronym=True)]
    def han(text):
        return bool(text) and all('\u3400' <= c <= '\u9fff' for c in text)
    cases = []
    for line in source.read_text().splitlines():
        event = json.loads(line)
        if datetime.datetime.fromtimestamp(event['recordedAtMs'] / 1000).strftime('%m-%d') < '07-22':
            continue
        gold = event.get('selectedText', '')
        if not han(gold) or 'precedingText' not in event:
            continue
        target = readings(gold)
        group = []
        for text in event['candidates']:
            if (han(text) and len(text) == len(gold) and text not in group
                    and all(a & b for a, b in zip(target, readings(text)))):
                group.append(text)
        # An exported inference batch has eight slots. No silent top-eight cut.
        if gold in group and 2 <= len(group) <= 8 and max(map(len, group)) <= 39:
            cases.append({'context': event['precedingText'], 'candidates': group})
    return cases


def evaluate(cases, scorer, harness, directory, batch_size=None):
    if batch_size is None:
        batch_size = max(1, int(os.environ.get('AIM_ALIGNMENT_BATCH', ALIGNMENT_BATCH)))
    with tempfile.TemporaryDirectory(prefix='agent-ime-context-alignment-') as temporary:
        root = Path(temporary)
        rows = []
        load_milliseconds = None
        for index, start in enumerate(range(0, len(cases), batch_size)):
            batch = cases[start:start + batch_size]
            request = root / f'cases-{index}.json'
            output = root / f'swift-{index}.json'
            request.write_text(json.dumps(batch, ensure_ascii=False))
            request.chmod(0o600)
            subprocess.run([harness, str(directory), str(request), str(output)], check=True, timeout=120)
            measured = json.loads(output.read_text())
            if len(measured['rows']) != len(batch):
                raise AssertionError(f'harness returned {len(measured["rows"])} rows for {len(batch)} cases')
            rows.extend(measured['rows'])
            load = measured['loadMilliseconds']
            load_milliseconds = load if load_milliseconds is None else max(load_milliseconds, load)
        if len(rows) != len(cases):
            raise AssertionError(f'harness returned {len(rows)} rows for {len(cases)} cases')
    same = 0
    for case, row in zip(cases, rows, strict=True):
        scores = scorer.score(case['context'], case['candidates'])
        reference = max(range(len(scores)), key=lambda i: scores[i] - 2 * i)
        if not all(math.isfinite(x) for x in row['scores']):
            raise AssertionError('non-finite Swift score')
        same += reference == row['order'][0]
    latencies = sorted(row['elapsedNanoseconds'] / 1e6 for row in rows)
    def percentile(fraction):
        return latencies[min(len(latencies) - 1, math.ceil(len(latencies) * fraction) - 1)]
    return {'cases': len(cases), 'agree': same, 'agreement': same / len(cases),
            'rerankP50Milliseconds': percentile(.5), 'rerankP95Milliseconds': percentile(.95),
            'rerankP99Milliseconds': percentile(.99), 'modelLoadAndWarmupMilliseconds': load_milliseconds}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--harness', required=True)
    ap.add_argument('--directory', type=Path, required=True)
    ap.add_argument('--out', type=Path, required=True)
    args = ap.parse_args()
    if not CHECKPOINT.is_file():
        raise FileNotFoundError('present Core ML model needs the matching PyTorch reference checkpoint')
    torch.set_num_threads(4)
    scorer = ZhiziScorer(str(CHECKPOINT), 'cpu')
    exported = json.loads((args.directory / 'itos.json').read_text())
    if scorer.vocab.itos != exported:
        raise AssertionError('PyTorch and Core ML vocabularies differ')
    scorer.model.config.block_size = 40
    result = evaluate(synthetic_cases(), scorer, args.harness, args.directory)
    result['publicSynthetic'] = True
    result['computeUnits'] = 'all'
    result['checkpointSHA256'] = hashlib.sha256(CHECKPOINT.read_bytes()).hexdigest()
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(result, indent=2) + '\n')
    print(f"context_model_alignment={'PASS' if result['agreement'] >= .99 else 'FAIL'} "
          f"agreement={result['agree']}/{result['cases']} percent={100*result['agreement']:.4f}")
    print(f"context_model_rerank_performance={'PASS' if result['rerankP95Milliseconds'] <= 8 else 'FAIL'} "
          f"p50_ms={result['rerankP50Milliseconds']:.4f} p95_ms={result['rerankP95Milliseconds']:.4f} "
          f"p99_ms={result['rerankP99Milliseconds']:.4f} load_ms={result['modelLoadAndWarmupMilliseconds']:.4f}")
    private = heldout_cases()
    if private:
        # Do not persist the private inputs, individual outputs, or aggregate
        # held-out report in the repository. Temporary files are mode 0600.
        result_private = evaluate(private, scorer, args.harness, args.directory)
        print(f"context_model_heldout_alignment agreement={result_private['agree']}/{result_private['cases']} "
              f"percent={100*result_private['agreement']:.4f} local_only=true")
    else:
        print('context_model_heldout_alignment=SKIP reason=no-local-eligible-heldout-records')
    if result['agreement'] < .99 or result['rerankP95Milliseconds'] > 8:
        raise SystemExit(1)


if __name__ == '__main__':
    try:
        main()
    except Exception as error:
        print(f'context_model_alignment=FAIL reason={type(error).__name__}', file=sys.stderr)
        print('context_model_rerank_performance=FAIL reason=no-valid-host-measurement', file=sys.stderr)
        raise
