"""Tests for the word-bigram table builder.

Run: cd scripts/lexicon && python3 -m unittest test_build_word_bigram -v
"""
import importlib.util
import math
from pathlib import Path
import tempfile
import unittest


SCRIPT = Path(__file__).with_name("build_word_bigram.py")
SPEC = importlib.util.spec_from_file_location("build_word_bigram", SCRIPT)
builder = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(builder)


def write_dict(path, rows):
    """rows are (word, reading, weight). A comment and a blank sit in the body."""
    body = ["---", "name: " + path.stem, 'version: "0.0.1"', "sort: by_weight", "...", ""]
    for index, (word, reading, weight) in enumerate(rows):
        if index == 1:
            body.extend(["# comment inside the body", ""])
        body.append("\t".join((word, reading, str(weight))))
    path.write_text("\n".join(body) + "\n", encoding="utf-8")


class DictionaryAndSegmentTests(unittest.TestCase):
    def test_max_weight_skips_comments_and_nonpositive(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            write_dict(root / "a.dict.yaml", [("你好", "ni hao", 10), ("我们", "wo men", 0)])
            write_dict(root / "b.dict.yaml", [("你好", "ni hao", 40), ("吃饭", "chi fan", 5)])
            weights = builder.load_weights([root / "a.dict.yaml", root / "b.dict.yaml"])
        self.assertEqual(weights["你好"], 40)
        self.assertEqual(weights["吃饭"], 5)
        self.assertNotIn("我们", weights)

    def test_segment_oov_character_and_single_character_fallback(self):
        # 我 is only a prefix of 我们, not its own entry. 吃饭 beats 吃+饭.
        lex = builder.Lexicon({"我们": 100, "吃饭": 80, "吃": 8, "饭": 8, "去": 20})
        self.assertEqual(lex.segment("我们去吃饭"), ["我们", "去", "吃饭"])
        self.assertEqual(lex.segment("我"), ["我"])
        self.assertEqual(lex.segment("我吃饭"), ["我", "吃饭"])
        self.assertEqual(lex.segment("我们麒吃饭"), ["我们", "麒", "吃饭"])
        self.assertEqual(lex.segment("龘龘"), ["龘", "龘"])
        self.assertEqual(lex.segment(""), [])

    def test_tokenize_splits_sentences_and_keeps_ascii_and_digits(self):
        lex = builder.Lexicon({"我们": 50, "吃饭": 40, "github": 30})
        sentences = builder.tokenize_line(lex, "我们去github吃饭。买了20个\\n你吃饭")
        self.assertEqual(sentences[0], ["我们", "去", "github", "吃饭"])
        self.assertEqual(sentences[1], ["买", "了", "20", "个"])
        self.assertEqual(sentences[2], ["你", "吃饭"])
        # Case fold only when the dictionary has the lowercase word.
        folded = builder.tokenize_line(lex, "用GitHub吧")
        self.assertIn("github", folded[0])


class KneserNeyTests(unittest.TestCase):
    def test_modified_kn_discounts_match_chen_goodman(self):
        d1, d2, d3 = builder.modified_kn_discounts(10, 5, 2, 1)
        self.assertAlmostEqual(d1, 0.5, places=9)
        self.assertAlmostEqual(d2, 1.4, places=9)
        self.assertAlmostEqual(d3, 2.0, places=9)

    def _sentences(self):
        return [
            ["我", "去", "吃饭"],
            ["我", "去", "吃饭"],
            ["我", "去", "吃饭"],
            ["我", "想", "吃饭"],
            ["你", "去", "玩"],
            ["你", "想", "玩"],
            ["我", "想", "玩"],
            ["你", "去", "吃饭"],
        ]

    def test_probability_mass_and_unseen_backoff(self):
        full = builder.fit_kn(self._sentences(), dictionary_words=set(), delta=0.0)
        for context in full.contexts():
            self.assertAlmostEqual(full.raw_mass(context), 1.0, places=8)
        # (我, 玩) never occurs; 玩 does occur elsewhere, so continuation is nonzero.
        self.assertNotIn("玩", full.successors("我"))
        backoff = math.log10(full.gamma("我")) + math.log10(full.pcont("玩"))
        lm = full.materialize(target_bytes=None)
        self.assertAlmostEqual(lm.logprob("我", "玩"), backoff, places=5)
        self.assertGreater(lm.logprob("我", "去"), lm.logprob("我", "玩"))
        for context in {w1 for w1, _w2 in lm.bigram}:
            self.assertAlmostEqual(builder.context_mass(lm, context), 1.0, places=3)

    def test_dictionary_word_absent_from_counts_still_has_backoff(self):
        full = builder.fit_kn(self._sentences(), dictionary_words={"发布"}, delta=0.1)
        self.assertGreater(full.pcont("发布"), 0.0)
        lm = full.materialize(target_bytes=None)
        self.assertTrue(math.isfinite(lm.logprob("我", "发布")))
        self.assertAlmostEqual(builder.context_mass(lm, "我"), 1.0, places=3)


class PruneAndStabilityTests(unittest.TestCase):
    def _sentences(self):
        base = [
            ["我", "去", "吃饭"],
            ["我", "去", "吃饭"],
            ["我", "想", "吃饭"],
            ["你", "去", "玩"],
            ["你", "想", "玩"],
            ["今天", "天气", "不错"],
            ["今天", "天气", "不错"],
            ["今天", "天气", "不错"],
            ["天气", "不错"],
        ]
        return base

    def test_pruned_query_still_backs_off_and_normalizes(self):
        sentences = self._sentences()
        full = builder.fit_kn(sentences, dictionary_words=set(), delta=0.0)
        lm_full = full.materialize(target_bytes=None)
        lm = full.materialize(target_bytes=220)
        self.assertLess(len(lm.bigram), len(lm_full.bigram))
        self.assertLessEqual(lm.payload_bytes, 220)
        self.assertGreater(lm.payload_bytes, 0)
        removed = next(pair for pair in lm_full.bigram if pair not in lm.bigram)
        w1, w2 = removed
        got = lm.logprob(w1, w2)
        self.assertTrue(math.isfinite(got))
        self.assertAlmostEqual(got, lm.backoff_logprob(w1, w2), places=6)
        self.assertAlmostEqual(builder.context_mass(lm, w1), 1.0, places=3)
        kept = next(iter(lm.bigram))
        self.assertEqual(lm.logprob(*kept), lm.bigram[kept])

    def test_count_prune_keeps_frequent_pair_not_rare_certain_pair(self):
        # 稀|有 is the only continuation of 稀, so its entropy gain is large
        # and its count is 1. The budget must prefer the repeated pair.
        sentences = [["常", "见"]] * 40 + [["稀", "有"]]
        full = builder.fit_kn(sentences, dictionary_words=set(), delta=0.0)
        lm = full.materialize(target_bytes=200)
        self.assertIn(("常", "见"), lm.bigram)
        self.assertNotIn(("稀", "有"), lm.bigram)
        self.assertLessEqual(lm.payload_bytes, 200)

    def test_same_seed_writes_identical_tables(self):
        lines = ["第%d行我们去吃饭吧\n" % i for i in range(40)]
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            corpus = root / "wiki.txt"
            corpus.write_text("".join(lines), encoding="utf-8")
            lex = builder.Lexicon({"我们": 30, "去": 10, "吃饭": 20, "吧": 8})
            out_a = root / "a"
            out_b = root / "b"
            out_c = root / "c"
            builder.build_from_files(
                [("wiki", corpus, "test")], lex, out_a, tiers_mb=(1,),
                wiki_chars=80, seed=1, synth_holdout=0.0, delta=0.0)
            builder.build_from_files(
                [("wiki", corpus, "test")], lex, out_b, tiers_mb=(1,),
                wiki_chars=80, seed=1, synth_holdout=0.0, delta=0.0)
            builder.build_from_files(
                [("wiki", corpus, "test")], lex, out_c, tiers_mb=(1,),
                wiki_chars=80, seed=2, synth_holdout=0.0, delta=0.0)
            for name in ("bigrams.tsv", "unigrams.tsv"):
                self.assertEqual((out_a / "1mb" / name).read_bytes(),
                                 (out_b / "1mb" / name).read_bytes())
            # Different residue class, so the sampled lines (and the table) differ.
            self.assertNotEqual((out_a / "1mb" / "bigrams.tsv").read_bytes(),
                                (out_c / "1mb" / "bigrams.tsv").read_bytes())


if __name__ == "__main__":
    unittest.main()
