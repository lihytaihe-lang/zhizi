"""Check community TSV entries without changing the Rime dictionaries.

Usage: python3 scripts/lexicon/check_contribution.py contrib/*.tsv \
    --rime native/resources/Rime [--report report.json]

Rows contain word<TAB>pinyin[<TAB>weight]; blank lines and # comments are ignored.
JSON is a list of file/line/level/rule objects, with no submitted row contents.
Errors exit 1; warnings alone exit 0. Only Python's standard library is required.
"""

import argparse
import json
from pathlib import Path
import re
import sys
import unicodedata


# Hanyu Pinyin syllable inventory, transcribed from the initials/finals tables
# in the Scheme for the Chinese Phonetic Alphabet (汉语拼音方案), with ü as v.
# Includes rare interjections and dialect syllables used in Mandarin dictionaries.
# The shipped schema defines transformations, not an inventory; its single-Han
# entries currently expose only da and guan, so they cannot validate submissions.
SYLLABLES = set('''
a ai an ang ao
ba bai ban bang bao bei ben beng bi bian biao bie bin bing bo bu
ca cai can cang cao ce cen ceng cha chai chan chang chao che chen cheng chi chong chou chu chua chuai chuan chuang chui chun chuo ci cong cou cu cuan cui cun cuo
da dai dan dang dao de dei den deng di dia dian diao die ding diu dong dou du duan dui dun duo
e ei en eng er
fa fan fang fei fen feng fo fou fu
ga gai gan gang gao ge gei gen geng gong gou gu gua guai guan guang gui gun guo
ha hai han hang hao he hei hen heng hong hou hu hua huai huan huang hui hun huo
ji jia jian jiang jiao jie jin jing jiong jiu ju juan jue jun
ka kai kan kang kao ke kei ken keng kong kou ku kua kuai kuan kuang kui kun kuo
la lai lan lang lao le lei len leng li lia lian liang liao lie lin ling liu lo long lou lu luan lun luo lv lve
ma mai man mang mao me mei men meng mi mian miao mie min ming miu mo mou mu
na nai nan nang nao ne nei nen neng ni nian niang niao nie nin ning niu nong nou nu nuan nuo nv nve
o ou
pa pai pan pang pao pei pen peng pi pian piao pie pin ping po pou pu
qi qia qian qiang qiao qie qin qing qiong qiu qu quan que qun
ran rang rao re ren reng ri rong rou ru ruan rui run ruo
sa sai san sang sao se sen seng sha shai shan shang shao she shei shen sheng shi shou shu shua shuai shuan shuang shui shun shuo si song sou su suan sui sun suo
ta tai tan tang tao te teng ti tian tiao tie ting tong tou tu tuan tui tun tuo
wa wai wan wang wei wen weng wo wu
xi xia xian xiang xiao xie xin xing xiong xiu xu xuan xue xun
ya yan yang yao ye yi yin ying yo yong you yu yuan yue yun
za zai zan zang zao ze zei zen zeng zha zhai zhan zhang zhao zhe zhei zhen zheng zhi zhong zhou zhu zhua zhuai zhuan zhuang zhui zhun zhuo zi zong zou zu zuan zui zun zuo
hm hng m n ng
'''.split())

PRIVACY = re.compile(
    r'(?<![0-9])(?:\+?86[- ]?)?1[3-9][0-9]{9}(?![0-9])'
    r'|[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}'
    r'|https?://\S+|www\.\S+'
    r'|(?<![A-Za-z0-9])[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)*\.[A-Za-z]{2,}(?:/\S*)?',
    re.IGNORECASE)


def is_han(word):
    """Accept assigned Han ideographs, including compatibility and extension Han."""
    return bool(word) and all(
        unicodedata.name(char, '').startswith(('CJK UNIFIED IDEOGRAPH-',
                                               'CJK COMPATIBILITY IDEOGRAPH-'))
        for char in word)


def add_issue(issues, path, line, level, rule):
    """Record locations and fixed rule identifiers, never submitted contents."""
    issues.append({'file': str(path), 'line': line, 'level': level, 'rule': rule})


def load_rime(directory, issues):
    """Read dictionary bodies and reading TSVs, including escaped-tab assets."""
    entries = {}
    if not directory.is_dir():
        add_issue(issues, directory, 0, 'error', 'rime_read')
        return entries
    paths = sorted(directory.rglob('*.dict.yaml')) + sorted(directory.rglob('*.tsv'))
    if not paths:
        add_issue(issues, directory, 0, 'error', 'rime_read')
    for path in paths:
        try:
            with path.open(encoding='utf-8-sig') as source:
                body = path.suffix == '.tsv'
                for line in source:
                    if line.strip() == '...':
                        body = True
                        continue
                    if not body or not line.strip() or line.startswith('#'):
                        continue
                    columns = line.rstrip('\r\n').replace('\\t', '\t').split('\t')
                    if len(columns) >= 2 and is_han(columns[0]):
                        entries.setdefault(columns[0], set()).add(' '.join(columns[1].split()))
        except (OSError, UnicodeError):
            add_issue(issues, path, 0, 'error', 'rime_read')
    return entries


def check_file(path, entries, issues):
    """Check one UTF-8 TSV; return the number of non-comment submission rows."""
    count = 0
    seen = set()
    try:
        with path.open(encoding='utf-8-sig') as source:
            for number, raw in enumerate(source, 1):
                line = raw.rstrip('\r\n')
                if not line.strip() or line.startswith('#'):
                    continue
                count += 1
                columns = line.split('\t')
                if PRIVACY.search(line):
                    add_issue(issues, path, number, 'warning', 'privacy_shape')
                if len(columns) not in (2, 3):
                    add_issue(issues, path, number, 'error', 'column_count')
                word = columns[0]
                if not (2 <= len(word) <= 8 and is_han(word)):
                    add_issue(issues, path, number, 'error', 'word_format')
                elif len(set(word)) == 1:
                    add_issue(issues, path, number, 'warning', 'repeated_characters')
                if len(columns) < 2:
                    continue
                reading = columns[1]
                syllables = reading.split(' ')
                if any(syllable not in SYLLABLES for syllable in syllables):
                    add_issue(issues, path, number, 'error', 'pinyin_format')
                if len(syllables) != len(word):
                    add_issue(issues, path, number, 'error', 'syllable_count')
                pair = (word, reading)
                if pair in seen:
                    add_issue(issues, path, number, 'error', 'duplicate_entry')
                seen.add(pair)
                if reading in entries.get(word, set()):
                    add_issue(issues, path, number, 'error', 'existing_entry')
                elif word in entries:
                    add_issue(issues, path, number, 'warning', 'new_reading')
                if len(columns) >= 3:
                    weight = columns[2]
                    if not re.fullmatch(r'[0-9]{1,4}', weight) or not 1 <= int(weight) <= 1000:
                        add_issue(issues, path, number, 'error', 'weight_range')
    except (OSError, UnicodeError):
        add_issue(issues, path, 0, 'error', 'contribution_read')
    return count


def main(argv=None):
    """Validate files, optionally write JSON, and return a CI-compatible exit code."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('files', nargs='+', type=Path)
    parser.add_argument('--rime', required=True, type=Path)
    parser.add_argument('--report', type=Path)
    args = parser.parse_args(argv)
    issues = []
    entries = load_rime(args.rime, issues)
    total = sum(check_file(path, entries, issues) for path in args.files)
    if total > 500:
        add_issue(issues, args.files[0], 0, 'warning', 'submission_size')
    output = json.dumps(issues, ensure_ascii=True, indent=2) + '\n'
    if args.report:
        try:
            args.report.write_text(output, encoding='utf-8')
        except OSError:
            print('error: report_write', file=sys.stderr)
            return 1
    # ASCII JSON also prevents submitted filenames from injecting terminal/CI commands.
    print(output, end='')
    return int(any(issue['level'] == 'error' for issue in issues))


if __name__ == '__main__':
    sys.exit(main())
