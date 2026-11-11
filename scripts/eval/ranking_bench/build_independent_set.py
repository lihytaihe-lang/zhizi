"""Build S3 cases from ASAP/Wikipedia validation text and dictionary protections."""
import argparse
from collections import Counter, defaultdict
from functools import lru_cache
import hashlib
from pathlib import Path
import random
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import build_external_set as external
import margin_gate as gate

ROOT = Path("tmp/delegation/rank-s3-independent-protection-sets")
QUOTAS = dict(full=1250, short=1000, abbrev=750, mixed=500, typo=500, erhua=100)
SOURCES = (("asap", "asap.valid.txt"), ("wiki", "wikipedia.valid.txt"))
ENGLISH_LAYERS = ("agent_english", "agent_tech_terms", "agent_ai_terms",
                  "agent_pack_terms", "agent_project_terms")
SURNAMES = set("王李张刘陈杨黄赵吴周徐孙马朱胡郭何林高罗郑梁谢宋唐许韩冯邓曹彭曾肖田董袁潘于蒋蔡余杜叶程苏魏吕丁任沈姚卢姜崔钟谭陆汪范金石廖贾夏韦付方白邹孟熊秦邱江尹薛阎段雷侯龙史陶黎贺顾毛郝龚邵万钱严覃武戴莫孔向汤")
NONNAME_ENDINGS = set("区县市镇村庄寨省国州府堡桥路街馆院寺店厂矿团队站所局厅部处科房派宫教湾岛洋洲港郡王帝公氏爷婆妈爸宗祖潭仓湖岭里性量值度叹听问骂喊")
SINGLE_GIVEN = set("明华伟强军勇杰涛斌超波辉刚健平峰鹏飞龙磊洋宇浩凯俊志亮建文博天成东林清海山松柏荣光旭晨阳春秋冬雪梅兰竹菊芳丽娟敏静秀英慧燕婷萍娜霞玲莉红艳佳欣颖洁丹玉珍琼云月青宁安彬鑫锐宏达维新进良国忠信义礼智仁毅诚谦志远翔帆航力雄虎威武锋剑兵雄奇凡乐毅卓晖昊睿涵雨琪琳璐瑞德恩福康祥庆富贵永昌兴盛元章涛渊泽淳尧舜禹白甫轼迅布备操羽备权策逊统肃瑜蒙晃忠褚温斯禅牧适询奕逵雄铮达祎祺隽延骏")
NONNAME_PARTS = ("家庄", "家寨", "家村", "家湾", "家坪", "家沟", "家屯", "家店", "家口", "家岗", "家冲", "果树",
                 "先生", "夫人", "教授", "博士", "皇帝", "太后", "小姐", "公子", "主任", "书记", "将军", "元帅", "大头", "大夫", "员外", "二世",
                 "大人", "大哥", "师傅", "铁匠", "长老", "帮主", "姑娘", "妹妹", "师弟", "娘子", "进士", "庄主", "督师", "贵妃", "大侠", "太太", "大嫂", "观主", "中堂", "司令", "政委", "皇后", "香主")
NONNAMES = set("高尔夫 高加索 白骨精 王八蛋 孔雀石 王老吉 王老五 陈设 顾问 高兴 高明 高峰 高原 高低 高贵 高尚 高手 高亮 高空 高级 高强 高大 高速 高山 高雄 金黄 金融 金刚 金牌 金钱 金星 金属 金鱼 金华 白色 白天 白菜 白云 白马 白糖 白银 白雪 白杨 白脸 白话 白吃 白搭 白刃 龙虎 田赋 马力 马达 马虎 马甲 马上 马车 马克 马尾 马路 马列 马夫 马头 马蹄 马术 方圆 方正 方向 方言 方才 方案 方针 方块 方丈 石头 石英 石灰 石油 石材 石板 夏天 夏季 夏日 罗汉 罗马 田野 江南 江北 江边 江水 江河 万一 万能 万岁 万年 万里 何时 何处 何在 何必 何以 何如 何故 何况 何人 何谓 雷达 雷同 龙王 龙门 龙骨 龙舟 毛病 毛衣 毛发 毛皮 毛利 毛笔 毛线 武功 武术 严重 严格 严肃 汤圆 汤水 杜鹃 林木 林业 熊猫 魏征听 宋江听 孔明视".split())


NONNAMES.update((
    "万寿羹 严重者 于城邦 于步廊 刘郎浦 卢布 卢旺达 叶尔羌 叶绿体 叶绿素 叶莲子 吴县人 吴淞口 唐再加 唐武德 唐贞观 夏令营 夏威夷 夏布 孙家拐 孟加拉 "
    "张家界 徐家汇 朱家渡 李家墩 杜里调 杜鹃花 杨二人 杨同嘴 杨家台 杨梅 武昌鱼 武林 武林中 武清伯 武英殿 段时间 毛冠鹿 毛利人 毛竹 汤尤杯 潘家园 "
    "王家墩 王府井 田园诗 白云山 白云岩 白云石 白云观 白布 白帝城 白玉兰 白玉石 白皮书 白花花 白蛋白 白话文 白马山 白鹿原 石狮子 石英砂 "
    "罗家墩 罗布泊 罗汉堂 罗马式 胡同口 胡杨林 胡萝卜 苏伊士 苏州人 苏美尔 苏黎世 许可证 谢皇恩 贺兰山 赵家条 金丝燕 金元宝 金刀绣 金刚石 金刚经 "
    "金字塔 金毛狮 金水河 金灿灿 金瓶梅 金红石 金蛇剑 金銮殿 金钱豹 金银花 金面佛 钱塘江 陆海军 陆海空 陈列室 陈总舵 陈瞎子 雷阵雨 "
    "马六甲 马兰峪 马其顿 马家棚 马尼拉 马尾松 马拉松 马拉维 马林工 马步军 马王堆 高中生 高基庙 高岭土 高水平 黄古蒿 黄封殿 黄州菜 "
    "黄橙子 黄瓦朱 黄瓦飞 黄绿色 黄金周 黄鹤楼 黎巴嫩 龙井茶 龙卷风 龙王庙 魏巍史"
).split())
SINGLE_GIVEN.add("蓉")


def exclusion_keys(paths):
    keys = set()
    for path in paths:
        for row in gate.read_jsonl(path):
            raw = row.get("raw", row.get("keys"))
            if not isinstance(raw, str):
                raise ValueError("exclusion case has no raw/keys")
            keys.add((raw, row["context"], row["target"]))
    return keys


def name_words(path):
    accepted, rejected = [], Counter()
    entries = [line.split() for line in Path(path).read_text(encoding="utf-8").splitlines()]
    entries = [parts for parts in entries if len(parts) == 3 and parts[2] == "nr"]
    nr_words = {parts[0] for parts in entries}
    for parts in entries:
        word, frequency = parts[0], int(parts[1])
        if not external.HAN.fullmatch(word) or not 2 <= len(word) <= 3 or word[0] not in SURNAMES:
            rejected["length_han_surname"] += 1
        elif frequency < 50:
            rejected["frequency_below_50"] += 1
        elif (word in NONNAMES or word[-1] in NONNAME_ENDINGS
              or (len(word) == 2 and word[1] not in SINGLE_GIVEN)
              or (len(word) == 3 and word[:2] in nr_words and word[-1] in "来见笑心便可")
              or any(part in word for part in NONNAME_PARTS) or len(set(word)) == 1):
            rejected["obvious_dictionary_noise"] += 1
        else:
            accepted.append(dict(target=word, frequency=frequency))
    return accepted, dict(rejected)


def english_words(paths):
    result, seen = [], set()
    for path in paths:
        in_body = False
        for line in Path(path).read_text(encoding="utf-8").splitlines():
            if line.strip() == "...":
                in_body = True
                continue
            if not in_body or not line or line.startswith("#"):
                continue
            parts = line.split("\t")
            if len(parts) < 2:
                continue
            word, raw = parts[:2]
            if (not 2 <= len(word) <= 32 or not word.isascii() or not word.isalpha()
                    or raw != word.lower() or raw in seen):
                continue
            seen.add(raw)
            result.append(dict(target=word, raw=raw, english_layer=Path(path).stem))
    return result


def pinyin_reader():
    from pypinyin import Style, lazy_pinyin

    @lru_cache(maxsize=100000)
    def read(text):
        syllables = external.syllables(text, lazy_pinyin, Style)
        if len(syllables) != len(text) or not all(s.isascii() and s.isalpha() for s in syllables):
            return ()
        return tuple(syllables)
    return read


def sample_independent(corpus, words_path, excluded, seed, quotas=None):
    quotas = QUOTAS if quotas is None else quotas
    segmenter, read = external.Segmenter(words_path), pinyin_reader()
    homophones = defaultdict(set)
    for word in segmenter.freq:
        if 1 <= len(word) <= 2 and external.HAN.fullmatch(word):
            syllables = read(word)
            if syllables:
                homophones["".join(syllables)].add(word)
    short_keys = {raw for raw, words in homophones.items() if len(words) >= 2}
    seen, rows, contexts, stats = set(excluded), [], {}, {}
    for source_index, (src, filename) in enumerate(SOURCES):
        sentences = [(i + 1, s.strip()) for i, s in enumerate((Path(corpus) / filename).read_text(encoding="utf-8").splitlines()) if s.strip()]
        rng = random.Random(seed + 10 * source_index)
        rng.shuffle(sentences)
        counts, removed, context_pool, context_seen = Counter(), Counter(), [], set()

        def add(sub, raw, context, target, line_number, sentence):
            context = context[-32:]
            key = raw, context, target
            if key in seen:
                removed["excluded" if key in excluded else "duplicate"] += 1
                return
            seen.add(key)
            rows.append(dict(id=f"s3-{seed}-{src}-{sub}-{counts[sub]:05d}",
                             bucket="full" if sub in ("short", "erhua") else sub,
                             sub=sub, src=src, source="ext", raw=raw, context=context, target=target,
                             source_line=line_number, sentence_sha256=hashlib.sha256(sentence.encode()).hexdigest()))
            counts[sub] += 1

        for line_number, sentence in sentences:
            for context, text, words in external.pieces(sentence, segmenter, rng):
                snippet = context[-32:]
                if sum("一" <= c <= "鿿" for c in snippet) >= 4 and snippet not in context_seen:
                    context_seen.add(snippet)
                    context_pool.append(dict(context=snippet, source_line=line_number,
                                             sentence_sha256=hashlib.sha256(sentence.encode()).hexdigest()))
                sy = read(text)
                if not sy:
                    continue
                full, choice, case = "".join(sy), rng.random(), None
                if choice < .25 and counts["full"] < quotas["full"]:
                    case = "full", full
                elif choice < .4 and counts["abbrev"] < quotas["abbrev"] and len(words) == 1 and 2 <= len(text) <= 4:
                    case = "abbrev", "".join(external.initial(s) for s in sy)
                elif choice < .65 and counts["mixed"] < quotas["mixed"] and len(text) >= 3:
                    if len(words) >= 2 and rng.random() < .35:
                        last = len(words[-1])
                        keys = "".join(sy[:-last]) + "".join(external.initial(s) for s in sy[-last:])
                    else:
                        keys = "".join(sy[:-1]) + external.initial(sy[-1])
                    case = "mixed", keys
                elif choice < .9 and counts["typo"] < quotas["typo"] and len(full) >= 4:
                    keys = external.slip(full, rng)
                    if keys:
                        case = "typo", keys
                if case is None and counts["short"] < quotas["short"] and len(words[-1]) <= 2 and context:
                    tail = words[-1]
                    tail_sy = read(tail)
                    if tail_sy and "".join(tail_sy) in short_keys:
                        context += text[:-len(tail)]
                        text = tail
                        case = "short", "".join(tail_sy)
                if case:
                    add(*case, context, text, line_number, sentence)
            if all(counts[b] >= quotas[b] for b in quotas if b != "erhua"):
                break
        erhua_rng = random.Random(seed + 10 * source_index + 1)
        for line_number, sentence in sentences:
            if counts["erhua"] >= quotas["erhua"]:
                break
            for context, text, _ in external.pieces(sentence, segmenter, erhua_rng):
                if counts["erhua"] >= quotas["erhua"]:
                    break
                sy = read(text)
                if len(text) >= 2 and text.endswith("儿") and text.count("儿") == 1 and sy and sy[-1] == "er":
                    add("erhua", "".join(sy), context, text, line_number, sentence)
        contexts[src] = context_pool
        stats[src] = dict(counts={b: counts[b] for b in quotas}, removed=dict(removed),
                          source_lines=len(sentences), protection_contexts=len(context_pool))
    return rows, contexts, stats


def protection_cases(words, contexts, excluded, seed, sub, count):
    rng, seen = random.Random(seed), set(excluded)
    words = list(words)
    rng.shuffle(words)
    pools = {src: list(rows) for src, rows in contexts.items()}
    for pool in pools.values():
        rng.shuffle(pool)
        if not pool:
            raise ValueError("no real Chinese context for protection set")
    read, result = pinyin_reader(), []
    for word in words:
        if len(result) == count:
            break
        raw = word.get("raw") or "".join(read(word["target"]))
        if not raw:
            continue
        src = SOURCES[len(result) % len(SOURCES)][0]
        for snippet in pools[src]:
            key = raw, snippet["context"], word["target"]
            if key not in seen:
                seen.add(key)
                result.append(dict(word, **snippet, id=f"s3-{seed}-{sub}-{len(result):05d}",
                                   bucket="full", sub=sub, src=src, source="ext", raw=raw))
                pools[src].append(pools[src].pop(0))
                break
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--corpus", type=Path, required=True)
    parser.add_argument("--words", type=Path, required=True)
    parser.add_argument("--rime", type=Path, default=Path("native/resources/Rime"))
    parser.add_argument("--s1", type=Path, required=True)
    parser.add_argument("--old", type=Path, required=True)
    parser.add_argument("--out", type=Path, default=ROOT / "sets")
    parser.add_argument("--seed", type=int, default=20261110)
    args = parser.parse_args()
    args.out.mkdir(parents=True)
    exclusion_paths = [args.s1 / "tune.jsonl", args.s1 / "test.jsonl", args.old]
    excluded = exclusion_keys(exclusion_paths)
    independent, contexts, stats = sample_independent(args.corpus, args.words, excluded, args.seed)
    names, rejected = name_words(args.words)
    english_paths = [args.rime / f"{name}.dict.yaml" for name in ENGLISH_LAYERS]
    english = english_words(english_paths)
    used = excluded | {gate.case_key(r) for r in independent}
    name_cases = protection_cases(names, contexts, used, args.seed + 100, "name", 800)
    used.update(gate.case_key(r) for r in name_cases)
    english_cases = protection_cases(english, contexts, used, args.seed + 200, "english", 500)
    groups = dict(independent=independent, names=name_cases, english=english_cases,
                  protections=name_cases + english_cases)
    all_rows = independent + groups["protections"]
    if (len({gate.case_key(r) for r in all_rows}) != len(all_rows)
            or excluded & {gate.case_key(r) for r in all_rows}):
        raise ValueError("dataset overlap")
    for name, rows in groups.items():
        gate.write_jsonl(args.out / f"{name}.jsonl", rows)
    audit = random.Random(args.seed + 300).sample(name_cases, min(30, len(name_cases)))
    gate.write_jsonl(args.out / "name-audit-30.jsonl", audit)
    inputs = [*(args.corpus / filename for _, filename in SOURCES), args.words,
              *english_paths, *exclusion_paths]
    manifest = dict(seed=args.seed, target_per_source=QUOTAS, independent=stats,
                    totals={k: len(v) for k, v in groups.items()},
                    strata=dict(Counter(f"{r['bucket']}/{r['sub']}/{r['src']}" for r in all_rows)),
                    excluded_unique_triples=len(excluded), overlap=0,
                    names=dict(eligible=len(names), rejected=rejected, minimum_frequency=50,
                               surname_count=len(SURNAMES), lengths=dict(Counter(len(r["target"]) for r in name_cases))),
                    english=dict(eligible=len(english), layers=dict(Counter(r["english_layer"] for r in english_cases))),
                    inputs={str(p): gate.digest(p) for p in inputs},
                    files={f"{name}.jsonl": gate.digest(args.out / f"{name}.jsonl") for name in groups})
    gate.write_json(args.out / "dataset.json", manifest)
    print(manifest["totals"], flush=True)


if __name__ == "__main__":
    main()
