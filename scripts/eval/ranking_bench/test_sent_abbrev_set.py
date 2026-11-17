"""Rules for the sentence-abbreviation set. Synthetic dict and corpus only."""
import json
import random
import tempfile
import unittest
from pathlib import Path

from scripts.eval.ranking_bench import build_sent_abbrev_set as build


PROBES = (
    ("woxianzsysoug", "我现在使用搜狗"),
    ("woxianzaisysougou", "我现在使用搜狗"),
    ("woxianzaishiysougou", "我现在使用搜狗"),
    ("jintianqtbcw", "今天天气不错"),
    ("wmxcqcfb", "我们下次去吃饭吧"),
    ("zgwtyjjjl", "这个问题已经解决了"),
    ("que'shi's'shou", "确实顺手"),
    ("queshishunsh", "确实顺手"),
)

WORDS = ["中国", "是", "爱"]
SYLLABLES = ("zhong", "guo", "shi", "ai")


def mix_fits(raw):
    """True when raw abbreviates 30–60% of the three fixture words and spells the rest in full."""
    patterns = {
        0: "zhg" + "shi" + "ai",
        1: "zhongguo" + "sh" + "ai",
        2: "zhongguo" + "shi" + "a",
    }
    return raw in patterns.values()


class InitialTests(unittest.TestCase):
    def test_zh_ch_sh_are_two_letters_and_z_c_s_stay_one(self):
        self.assertEqual(build.syllable_initial("zhong"), "zh")
        self.assertEqual(build.syllable_initial("zhang"), "zh")
        self.assertEqual(build.syllable_initial("chi"), "ch")
        self.assertEqual(build.syllable_initial("chang"), "ch")
        self.assertEqual(build.syllable_initial("shi"), "sh")
        self.assertEqual(build.syllable_initial("shang"), "sh")
        self.assertEqual(build.syllable_initial("zi"), "z")
        self.assertEqual(build.syllable_initial("ci"), "c")
        self.assertEqual(build.syllable_initial("si"), "s")
        self.assertEqual(build.syllable_initial("ri"), "r")

    def test_zero_initial_uses_the_written_first_letter(self):
        # a/o/e 是韵母本身。y/w 不是二十一声母，注音以它们开头，书写上的韵母首字母就是 y/w。
        self.assertEqual(build.syllable_initial("a"), "a")
        self.assertEqual(build.syllable_initial("ai"), "a")
        self.assertEqual(build.syllable_initial("er"), "e")
        self.assertEqual(build.syllable_initial("ou"), "o")
        self.assertEqual(build.syllable_initial("o"), "o")
        self.assertEqual(build.syllable_initial("e"), "e")
        self.assertEqual(build.syllable_initial("wo"), "w")
        self.assertEqual(build.syllable_initial("yi"), "y")
        self.assertEqual(build.syllable_initial("yu"), "y")
        self.assertEqual(build.syllable_initial("nv"), "n")
        self.assertEqual(build.syllable_initial("lv"), "l")

    def test_four_styles_on_a_fixed_word_split(self):
        self.assertEqual(build.keys_for("all_initials", WORDS, SYLLABLES), "zhgsha")
        self.assertEqual(build.keys_for("head_full", WORDS, SYLLABLES), "zhongguosha")
        self.assertEqual(build.keys_for("tail_full", WORDS, SYLLABLES), "zhgshai")
        self.assertEqual(build.keys_for("all_initials", ["吃", "爱"], ("chi", "ai")), "cha")
        self.assertEqual(build.keys_for("head_full", ["吃", "爱"], ("chi", "ai")), "chia")
        self.assertEqual(build.keys_for("tail_full", ["吃", "爱"], ("chi", "ai")), "chai")

    def test_single_word_only_keeps_all_initials(self):
        self.assertEqual(build.keys_for("all_initials", ["中国"], ("zhong", "guo")), "zhg")
        self.assertIsNone(build.keys_for("head_full", ["中国"], ("zhong", "guo")))
        self.assertIsNone(build.keys_for("tail_full", ["中国"], ("zhong", "guo")))
        self.assertIsNone(build.keys_for("random_mix", ["中国"], ("zhong", "guo"), random.Random(0)))

    def test_mix_fraction_is_inclusive_and_stable(self):
        self.assertEqual(build.mix_options(2), [1])
        self.assertEqual(build.mix_options(3), [1])
        self.assertEqual(build.mix_options(4), [2])
        self.assertEqual(build.mix_options(5), [2, 3])
        self.assertEqual(build.mix_options(10), [3, 4, 5, 6])
        self.assertEqual(build.mix_options(1), [])
        first = build.keys_for("random_mix", WORDS, SYLLABLES, random.Random(1))
        second = build.keys_for("random_mix", WORDS, SYLLABLES, random.Random(1))
        self.assertEqual(first, second)
        self.assertTrue(mix_fits(first))

    def test_context_is_the_preceding_text_and_drops_the_target(self):
        target = "中国是爱"
        line = "甲" * 40 + target
        context = build.prefix_context(line, 40)
        self.assertEqual(context, "甲" * 32)
        self.assertNotIn(target, context)
        self.assertEqual(build.prefix_context(target, 0), "")
        self.assertLessEqual(len(context), 32)


class SetTests(unittest.TestCase):
    def corpus(self, root):
        root = Path(root)
        words = root / "dict.txt"
        words.write_text("\n".join([
            "中国 1000", "是 1000", "爱 800", "而 800", "我们 800",
            "今天 1000", "天气 1000", "不错 800", "吃饭 800", "非常 800", "好 800",
        ]) + "\n", encoding="utf-8")
        corpus = root / "corpus"
        corpus.mkdir()
        lines = []
        for i in range(40):
            prefix = "甲" * (i % 7)
            lines.append(f"{prefix}中国是爱而我们今天天气不错吃饭非常好")
            # The same target occurs in the preceding 32 characters and must not be kept.
            if i % 5 == 0:
                lines.append("天气不错之后今天天气不错吃饭非常好")
        text = "\n".join(lines) + "\n"
        (corpus / "asap.valid.txt").write_text(text, encoding="utf-8")
        (corpus / "wikipedia.valid.txt").write_text(text, encoding="utf-8")
        return corpus, words

    def test_seed_split_and_every_row_follows_its_style(self):
        with tempfile.TemporaryDirectory() as directory:
            corpus, words = self.corpus(directory)
            first = build.collect(corpus, words, seed=20261111, per_sub=6)
            second = build.collect(corpus, words, seed=20261111, per_sub=6)
            self.assertEqual(first, second)
            self.assertGreaterEqual(min(sum(row["sub"] == sub for row in first) for sub in build.SUBS), 1)
            tune, test = build.split_rows(first, 20261111)
            self.assertEqual(build.split_rows(second, 20261111), (tune, test))
            ids = [row["id"] for row in first]
            self.assertEqual(len(ids), len(set(ids)))
            self.assertEqual({row["id"] for row in tune + test}, set(ids))
            segmenter = build.external.Segmenter(words)
            read = build.pinyin_reader()
            for sub in build.SUBS:
                group = [row for row in first if row["sub"] == sub]
                cut = len(group) * 7 // 10
                self.assertEqual(sum(row["sub"] == sub for row in tune), cut)
                self.assertEqual(sum(row["sub"] == sub for row in test), len(group) - cut)
            for row in first:
                self.assertEqual(row["bucket"], "sent_abbrev")
                self.assertIn(row["src"], ("asap", "wiki"))
                self.assertEqual(row["source"], "ext")
                self.assertRegex(row["raw"], r"^[a-z]{2,}$")
                self.assertGreaterEqual(len(row["target"]), 4)
                self.assertLessEqual(len(row["target"]), 10)
                self.assertNotIn(row["target"], row["context"])
                self.assertLessEqual(len(row["context"]), 32)
                self.assertTrue(build.external.HAN.fullmatch(row["target"]))
                syllables = read(row["target"])
                pieces = segmenter.cut(row["target"], HMM=False)
                self.assertTrue(build.spelling_matches(row["sub"], row["raw"], pieces, syllables))


class ProbeTests(unittest.TestCase):
    def test_eight_plan_rows(self):
        path = Path(build.__file__).with_name("sent_abbrev_probes.jsonl")
        rows = [json.loads(line) for line in path.read_text(encoding="utf-8").splitlines() if line.strip()]
        self.assertEqual([(row["raw"], row["target"]) for row in rows], list(PROBES))
        self.assertEqual(len({row["id"] for row in rows}), 8)
        for row in rows:
            self.assertEqual(row["bucket"], "sent_abbrev")
            self.assertEqual(row["context"], "")
            self.assertEqual(row["source"], "ext")


if __name__ == "__main__":
    unittest.main()
