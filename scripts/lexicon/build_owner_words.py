"""从本机输入记录生成「我的常用词」层（只用标准库）。

用法：python3 scripts/lexicon/build_owner_words.py --events <events.jsonl> \
    --dict-dir <Rime 词典目录> [--before YYYY-MM-DD] --out <用户 Rime 目录> [--dry-run]
"""
import argparse
from collections import Counter, defaultdict
from datetime import datetime
from functools import lru_cache
import json
import os
from pathlib import Path
import re
import stat
import tempfile


LAYER = "agent_owner_words"
# 与 lexicon_layers.rs / zhixu/loader.rs 的运行时限制保持一致。
MAX_PACK_BYTES = 16 * 1024 * 1024
MAX_DICTIONARY_BYTES = 64 * 1024 * 1024
MAX_PACK_ENTRIES = 400_000
HANZI = re.compile(r"[\u3400-\u4dbf\u4e00-\u9fff\uf900-\ufaff\U00020000-\U0002fa1f\U00030000-\U000323af]+")
SYLLABLE = re.compile(r"[a-z]+")


def before_ms(text):
    """日期按运行脚本机器的本地零点解释，边界当天不收。"""
    if not re.fullmatch(r"\d{4}-\d{2}-\d{2}", text):
        raise ValueError("--before must be YYYY-MM-DD")
    return int(datetime.strptime(text, "%Y-%m-%d").timestamp() * 1000)


def read_dictionaries(directory):
    """所有层都参与查重；逐字对齐的词条为每个字提供完整读音。"""
    paths = sorted(Path(directory).glob("*.dict.yaml"))
    if not paths:
        raise ValueError("--dict-dir contains no *.dict.yaml files")
    if not (Path(directory) / "pinyin_simp.dict.yaml").is_file():
        # Without the base table every common word (一两万) looks missing: point at an installed bundle's Rime directory.
        raise ValueError("--dict-dir has no pinyin_simp.dict.yaml; use the input method bundle's Rime directory")
    known = set()
    readings = defaultdict(set)
    for path in paths:
        body = False
        with path.open(encoding="utf-8-sig") as source:
            for line in source:
                if not body:
                    if line.strip() == "...":
                        body = True
                    continue
                if not line.strip() or line.lstrip().startswith("#"):
                    continue
                cells = line.rstrip("\r\n").split("\t")
                word = cells[0]
                known.add(word)
                syllables = cells[1].split() if len(cells) > 1 else []
                if len(word) != len(syllables) or not all(SYLLABLE.fullmatch(s) for s in syllables):
                    continue
                for char, syllable in zip(word, syllables):
                    if HANZI.fullmatch(char):
                        readings[char].add(syllable)
        if not body:
            raise ValueError("dictionary is missing the '...' data separator")
    return known, readings


def unique_reading(word, pinyin, readings):
    """只保留与按键完整相等的逐字读音；找到两种就停止。"""
    if not isinstance(pinyin, str) or not SYLLABLE.fullmatch(pinyin):
        return None

    @lru_cache(maxsize=None)
    def matches(index, offset):
        if index == len(word):
            return [()] if offset == len(pinyin) else []
        result = []
        for syllable in sorted(readings.get(word[index], ())):
            if not pinyin.startswith(syllable, offset):
                continue
            for rest in matches(index + 1, offset + len(syllable)):
                result.append((syllable,) + rest)
                if len(result) == 2:
                    return result
        return result

    choices = matches(0, 0)
    return " ".join(choices[0]) if len(choices) == 1 else None


def select_words(events, known, readings, before=None):
    counts = Counter()
    entries = set()
    for event in events:
        if not isinstance(event, dict) or event.get("outcome") != "committed_candidate":
            continue
        word = event.get("selectedText")
        if (not isinstance(word, str) or not 2 <= len(word) <= 6
                or not HANZI.fullmatch(word) or word in known or word[-1] in "的地得"):
            continue
        index = event.get("selectedIndex")
        segments = event.get("segments")
        if not ((isinstance(segments, list) and segments) or (type(index) is int and index >= 1)):
            continue
        deleted = event.get("deletedAfterCommit")
        if deleted is not None and (type(deleted) is not int or deleted > 0):
            continue
        if event.get("redoAfterCommit"):
            continue
        if before is not None:
            recorded = event.get("recordedAtMs")
            if type(recorded) is not int or recorded >= before:
                continue
        reading = unique_reading(word, event.get("pinyin"), readings)
        if reading is not None:
            counts[word] += 1
            entries.add((word, reading))
    return [(word, reading, 500 + 500 * counts[word])
            for word, reading in sorted(entries)]


def read_events(path):
    with Path(path).open(encoding="utf-8") as source:
        for number, line in enumerate(source, 1):
            if not line.strip():
                continue
            try:
                yield json.loads(line)
            except json.JSONDecodeError:
                # 记录是一直追加的文件，进程被杀时最后一行可能写了一半：跳过，不带出正文。
                continue


def owned_directory(path, create=False, private=False):
    if create:
        path.mkdir(mode=0o700, parents=True, exist_ok=True)
    metadata = path.lstat()
    if not stat.S_ISDIR(metadata.st_mode) or metadata.st_uid != os.geteuid():
        raise ValueError("output directory must be owned by the current user and not a symlink")
    if private:
        path.chmod(0o700)


def check_target(path):
    try:
        metadata = path.lstat()
    except FileNotFoundError:
        return
    if (not stat.S_ISREG(metadata.st_mode) or metadata.st_uid != os.geteuid()
            or metadata.st_nlink != 1):
        raise ValueError("existing output must be an owned regular file with one link")


def stage_file(path, data):
    descriptor, name = tempfile.mkstemp(prefix="." + path.name + ".", suffix=".tmp", dir=path.parent)
    temporary = Path(name)
    try:
        with os.fdopen(descriptor, "wb") as output:
            os.fchmod(output.fileno(), 0o600)
            output.write(data)
            output.flush()
            os.fsync(output.fileno())
    except BaseException:
        temporary.unlink()
        raise
    return temporary


def sync_directory(path):
    descriptor = os.open(path, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def write_outputs(out, rows):
    out = Path(out)
    packs = out / "lexicon_packs"
    dictionary = out / (LAYER + ".dict.yaml")
    membership = packs / (LAYER + ".membership.tsv")
    words = sorted({word for word, _, _ in rows})
    generated = datetime.now().astimezone().isoformat(timespec="seconds")
    dictionary_bytes = ("# 本机个人词层，由 build_owner_words.py 生成。\n---\n"
                        f'name: {LAYER}\nversion: "{generated}"\nsort: by_weight\n...\n'
                        + "".join(f"{word}\t{reading}\t{weight}\n" for word, reading, weight in rows)).encode("utf-8")
    membership_bytes = "".join(f"{word}\t{LAYER}\n" for word in words).encode("utf-8")
    if (len(words) > MAX_PACK_ENTRIES or len(dictionary_bytes) > MAX_DICTIONARY_BYTES
            or len(membership_bytes) > MAX_PACK_BYTES):
        raise ValueError("owner words exceed runtime dictionary or membership limits")
    if not rows and not out.exists() and not out.is_symlink():
        return
    owned_directory(out, create=bool(rows))
    if rows or packs.exists() or packs.is_symlink():
        owned_directory(packs, create=bool(rows), private=True)
    for path in (dictionary, membership):
        check_target(path)
    if not rows:
        for path in (dictionary, membership):
            path.unlink(missing_ok=True)
        sync_directory(out)
        if packs.exists():
            sync_directory(packs)
        return
    staged = []
    try:
        # 两个文件都完成落盘才开始替换；membership 先于词典，与安装器一致。
        for path, data in ((membership, membership_bytes), (dictionary, dictionary_bytes)):
            staged.append((stage_file(path, data), path))
        for temporary, path in staged:
            os.replace(temporary, path)
            sync_directory(path.parent)
    finally:
        for temporary, _ in staged:
            temporary.unlink(missing_ok=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--events", required=True, type=Path)
    parser.add_argument("--dict-dir", required=True, type=Path)
    parser.add_argument("--before")
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args()
    try:
        cutoff = before_ms(args.before) if args.before is not None else None
        known, readings = read_dictionaries(args.dict_dir)
        rows = select_words(read_events(args.events), known, readings, cutoff)
        words = sorted({word for word, _, _ in rows})
        if not args.dry_run:
            write_outputs(args.out, rows)
    except (OSError, ValueError) as error:
        parser.exit(1, f"error: {error}\n")
    print(f"我的常用词：{len(words)} 个词")
    if args.dry_run:
        for word in words[:50]:
            print(word)


if __name__ == "__main__":
    main()
