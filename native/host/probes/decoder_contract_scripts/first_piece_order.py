#!/usr/bin/env python3
"""用 Y0 的真实选择观测核对前半截与整串候选的第一页结构。

只使用 --dictionary 指定的已有公开词典。裸 Rime 的整串合成句常在第一，
不能把它与 Mixer 处理后的首选混为一谈。记录文字和目标词位置，结构门沿用
Y0 的覆盖形状无序多重集；有序形状差另外列出；B 类可见前半截的槽位和覆盖必须相同。三条完整简拼
另列目标文字差异，不用本门宣称完整简拼首选与 Rime 相同。
"""
import argparse
from collections import Counter
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile


CASES = {
    'leiss': ('类似', 4), 'read': ('热爱', 3), 'wozenmy': ('我怎么', 6),
    'ganje': ('感觉', 4), 'renkd': ('认可', 4), 'houmm': ('后妈', 4),
    'yigng': ('义工', 3), 'app': ('阿婆', 2), 'shangjai': ('上架', 6),
    'zgr': ('中国人', 3), 'shengc': ('生成', 6), 'henkyi': ('很可以', 6),
}
COMPLETE_ABBREVIATIONS = {'zgr', 'shengc', 'henkyi'}


def write_scripts(destination):
    destination.mkdir(parents=True, exist_ok=False)
    for raw in CASES:
        (destination / f'{raw}.keys').write_text(
            '# Y3e 公开输入形状；仅打字，最后一步保留第一页。\nclear\n'
            + ''.join(f'type {key}\n' for key in raw), encoding='utf-8')


def first_pages(traces):
    result = {}
    for trace in traces:
        raw = Path(trace['name']).stem
        target, covered = CASES[raw]
        step = trace['steps'][-1]
        state = step['state']
        if state['raw'] != raw or state['page_no'] != 0:
            raise ValueError(f'{raw}: 最后一步不是完整输入第一页')
        errors = [error for event in trace['steps'] for error in event['observation_errors']]
        successful = all(event['result'] == {'Ok': ''} for event in trace['steps'])
        if len(step['candidate_texts']) != len(step['shapes']):
            raise ValueError(f'{raw}: 文字与形状数量不同')
        entries = [{'text': text, **shape} for text, shape
                   in zip(step['candidate_texts'], step['shapes'])]
        target_slot = next((index for index, entry in enumerate(entries) if entry['text'] == target), None)
        target_shape = step['shapes'][target_slot] if target_slot is not None else None
        expected = {'covered_letters': covered, 'whole': covered == len(raw), 'single_character': False}
        result[raw] = {'target': target, 'target_slot': target_slot, 'target_shape': target_shape,
                       'expected_shape': expected, 'target_matches': target_shape == expected and not errors and successful,
                       'whole_slots': [index for index, shape in enumerate(step['shapes']) if shape['whole']],
                       'preedit': state['preedit'], 'entries': entries,
                       'successful_steps': successful, 'observation_errors': errors}
    return result


def compare_pages(reference, tested):
    complete = set(reference) == set(tested) == set(CASES)
    differences, ordered, targets, complete_targets, newly_visible = [], [], [], [], []
    for raw in sorted(set(reference) & set(tested)):
        left, right = reference[raw], tested[raw]
        shape = lambda page: [(entry['covered_letters'], entry['whole'], entry['single_character'])
                              for entry in page['entries']]
        if shape(left) != shape(right):
            ordered.append(raw)
        if left['target_slot'] is not None:
            if (left['target_slot'], left['target_shape']) != (right['target_slot'], right['target_shape']) \
                    or not right['target_matches']:
                (complete_targets if raw in COMPLETE_ABBREVIATIONS else targets).append(raw)
        elif right['target_slot'] is not None:
            newly_visible.append(raw)
        if Counter(shape(left)) != Counter(shape(right)) or left['observation_errors'] or right['observation_errors'] \
                or not left['successful_steps'] or not right['successful_steps']:
            differences.append(raw)
    return {'complete_cases': complete, 'structure_differences': differences,
            'ordered_structure_differences': ordered, 'target_differences': targets,
            'complete_abbreviation_target_differences': complete_targets,
            'targets_newly_visible': newly_visible, 'passed': complete and not differences and not targets}


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--rime-root', required=True, type=Path)
    parser.add_argument('--dictionary', required=True, type=Path, help='已有本地公开 pinyin_simp.dict.yaml')
    parser.add_argument('--tested', choices=['rime', 'zhixu'], default='zhixu')
    parser.add_argument('--output', required=True, type=Path, help='结构报告 JSON 路径')
    args = parser.parse_args(argv)
    repository = Path(__file__).resolve().parents[4]
    temporary = Path(tempfile.mkdtemp(prefix='zhixu-y3e-first-piece-'))
    scripts = temporary / 'scripts'
    write_scripts(scripts)
    dictionary = args.dictionary
    command = [sys.executable, '-B', str(Path(__file__).with_name('run.py')),
               '--rime-root', str(args.rime_root), '--dictionary', str(dictionary),
               '--scripts', str(scripts), '--tested', args.tested]
    execution = subprocess.run(command, cwd=repository, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    log = temporary / 'run.log'
    log.write_text(execution.stdout, encoding='utf-8')
    outputs = [line.split('\t', 1)[1] for line in execution.stdout.splitlines() if line.startswith('Y0_OUTPUT\t')]
    if len(outputs) != 1:
        raise RuntimeError(f'Y0 没有给出唯一输出目录；退出 {execution.returncode}；见 {log}')
    output = Path(outputs[0])
    sides = {side: first_pages(json.loads((output / f'{side}.json').read_text()))
             for side in ['reference', 'tested']}
    comparison = compare_pages(sides['reference'], sides['tested'])
    targets = all(case['target_matches'] for side in sides.values() for case in side.values())
    summary = json.loads((output / 'summary.json').read_text())
    # 公共词典不保证含有每个目标词；仍报告缺失，不把未见词写成已验证。
    passed = comparison['passed']
    if args.tested == 'rime':
        passed = passed and execution.returncode == 0 and summary['mismatched_steps'] == 0
    report = {'fixture': 'public',
              'scope': 'bare decoder first page; not Mixer top1', 'command': command,
              'y0_exit': execution.returncode, 'strict_summary': summary,
              'first_page_comparison': comparison, 'all_targets_on_first_page': targets,
              'passed': passed, 'sides': sides,
              'paths': {'dictionary': str(dictionary), 'scripts': str(scripts),
                        'run_log': str(log), 'trace_directory': str(output)},
              'sha256': {str(path): digest(path) for path in [dictionary, log,
                         output / 'reference.json', output / 'tested.json', output / 'summary.json']}}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
    print(f"第一页结构 {'通过' if comparison['passed'] else '失败'}；"
          f"B类可见前半截 {'通过' if not comparison['target_differences'] else '失败'}；"
          f"完整简拼文字差 {comparison['complete_abbreviation_target_differences']}；"
          f"全部目标在第一页 {'是' if targets else '否'}；Y0 严格退出 {execution.returncode}；{args.output}")
    return 0 if passed else 1


if __name__ == '__main__':
    sys.exit(main())
