"""Segmentation, pinyin constraints, and beam width 1 for zhizi_generate.

Run: python3 -m unittest scripts/eval/zhizi_generate/test_generate.py
"""
import importlib.util
import unittest
from pathlib import Path

import numpy as np

SCRIPT = Path(__file__).with_name("generate.py")
SPEC = importlib.util.spec_from_file_location("zhizi_generate", SCRIPT)
gen = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(gen)


class TableModel:
    """log P(next char | generated text). Missing chars stay at the default."""

    def __init__(self, vocab, table, default=-30.0):
        self.vocab = vocab
        self.table = table
        self.default = default

    def logprobs(self, prefixes):
        out = np.full((len(prefixes), len(self.vocab.itos)), self.default, dtype=np.float32)
        for row, ids in enumerate(prefixes):
            text = "".join(self.vocab.itos[item] for item in ids if item >= 3)
            for char, value in self.table.get(text, {}).items():
                out[row, self.vocab.stoi[char]] = value
        return out


def vocab_of(*chars):
    return gen.Vocab(["<pad>", "<bos>", "<unk>", *chars])


def readings_of(vocab, pairs):
    return gen.ReadingIndex(pairs, vocab)


class SegmentationTests(unittest.TestCase):
    def spans(self, keys, pos=0):
        return gen.next_spans(keys, pos)

    def test_zh_ch_sh_are_one_initial_and_full_syllables_stay(self):
        zhong = self.spans("zhong")
        self.assertIn(("z", 1), zhong)
        self.assertIn(("zh", 2), zhong)
        self.assertIn(("zhong", 5), zhong)
        self.assertNotIn(("zho", 3), zhong)
        self.assertNotIn(("zhon", 4), zhong)
        self.assertIn(("ch", 2), self.spans("chang"))
        self.assertIn(("c", 1), self.spans("chang"))
        self.assertIn(("chang", 5), self.spans("chang"))
        self.assertIn(("sh", 2), self.spans("shi"))
        self.assertIn(("s", 1), self.spans("shi"))
        self.assertIn(("shi", 3), self.spans("shi"))
        # n/m/ng are initials or impossible, not syllables.
        self.assertEqual(self.spans("n"), [("n", 1)])
        self.assertEqual([token for token, _ in self.spans("ng")], ["n"])

    def test_apostrophe_is_a_hard_boundary(self):
        self.assertIn(("xi", 3), self.spans("xi'an"))
        self.assertIn(("x", 1), self.spans("xi'an"))
        self.assertFalse(any(token == "xian" for token, _ in self.spans("xi'an")))
        self.assertIn(("an", 5), self.spans("xi'an", 3))
        self.assertIn(("a", 4), self.spans("xi'an", 3))
        # que'shi: q u e ' ...
        self.assertIn(("que", 4), self.spans("que'shi"))
        self.assertNotIn(("ques", 4), self.spans("que'shi"))
        self.assertEqual(self.spans("'an"), [])
        doubled = self.spans("ni''hao")
        self.assertIn(("ni", 4), doubled)

    def test_zero_initial_uses_the_first_letter(self):
        self.assertIn(("a", 1), self.spans("a"))
        self.assertIn(("a", 1), self.spans("ang"))
        self.assertIn(("an", 2), self.spans("ang"))
        self.assertIn(("ang", 3), self.spans("ang"))
        self.assertIn(("o", 1), self.spans("ou"))
        self.assertIn(("ou", 2), self.spans("ou"))
        self.assertIn(("e", 1), self.spans("er"))
        self.assertIn(("er", 2), self.spans("er"))
        self.assertIn(("w", 1), self.spans("wo"))
        self.assertIn(("wo", 2), self.spans("wo"))
        self.assertIn(("y", 1), self.spans("yi"))
        self.assertIn(("yi", 2), self.spans("yi"))
        self.assertNotIn(("i", 1), self.spans("yi"))

    def test_canonical_u_and_v_spellings_match(self):
        self.assertTrue(gen.syllable_matches("lue", "lve"))
        self.assertTrue(gen.syllable_matches("lve", "lue"))
        self.assertTrue(gen.syllable_matches("jve", "jue"))
        self.assertTrue(gen.full_syllable("nv"))
        self.assertTrue(gen.full_syllable("nve"))
        self.assertFalse(gen.full_syllable("n"))
        self.assertFalse(gen.full_syllable("ng"))
        self.assertTrue(gen.syllable_matches("zh", "zhong"))
        self.assertFalse(gen.syllable_matches("zh", "zou"))
        self.assertTrue(gen.syllable_matches("a", "ang"))
        self.assertFalse(gen.syllable_matches("ai", "ang"))


class IndexTests(unittest.TestCase):
    def test_allowed_characters_follow_the_match_predicate(self):
        vocab = vocab_of("你", "泥", "中", "重", "啊", "安", "略", "女", "大")
        pairs = [
            ("你", "ni"), ("泥", "ni"), ("中", "zhong"), ("重", "chong"),
            ("啊", "a"), ("安", "an"), ("略", "lve"), ("女", "nv"), ("大", "da"),
        ]
        index = readings_of(vocab, pairs)
        tokens = ["n", "ni", "z", "zh", "zhong", "ch", "c", "a", "an", "ang", "lve", "lue", "nv", "da"]
        for token in tokens:
            if not (gen.initial(token) or gen.full_syllable(token)):
                continue
            got = [vocab.itos[item] for item in index.ids(token)]
            brute = []
            for char, reading in pairs:
                if gen.syllable_matches(token, reading) and char not in brute:
                    brute.append(char)
            self.assertEqual(set(got), set(brute), token)


class WindowTests(unittest.TestCase):
    def test_bos_and_39_minus_one_window(self):
        vocab = vocab_of("你", "好", "世")
        self.assertEqual(gen.prefix_ids("", "", vocab), [1])
        self.assertEqual(gen.prefix_ids("世", "", vocab), [1, vocab.stoi["世"]])
        self.assertEqual(gen.prefix_ids("世", "你", vocab), [1, vocab.stoi["世"], vocab.stoi["你"]])
        context = "你" * 50
        ids = gen.prefix_ids(context, "好", vocab)
        self.assertEqual(ids[0], 1)
        self.assertEqual(len(ids), 39)
        self.assertEqual(ids[-1], vocab.stoi["好"])
        self.assertTrue(all(item == vocab.stoi["你"] for item in ids[1:-1]))
        unknown = gen.prefix_ids("Q", "", vocab)
        self.assertEqual(unknown, [1, 2])


class SearchTests(unittest.TestCase):
    def world(self):
        vocab = vocab_of("你", "泥", "好", "号", "大")
        index = readings_of(vocab, [
            ("你", "ni"), ("泥", "ni"), ("好", "hao"), ("号", "hao"), ("大", "da"),
        ])
        lexicon = gen.Lexicon({})
        return vocab, index, lexicon

    def test_every_generated_character_matches_the_keys(self):
        vocab, index, lexicon = self.world()
        flat = {"": {"你": -1.0, "泥": -1.0, "大": 5.0},
                "你": {"好": -1.0, "号": -1.0, "大": 5.0},
                "泥": {"好": -1.0, "号": -1.0, "大": 5.0}}
        model = TableModel(vocab, flat)
        ranked, _ = gen.beam_search("nihao", "", model, vocab, index, lexicon, beam=8, mu=0.0)
        self.assertTrue(ranked)
        for hyp in ranked:
            self.assertTrue(gen.aligned("nihao", hyp.text, hyp.tokens, index), hyp.text)
            self.assertNotIn("大", hyp.text)

    def test_beam_width_one_matches_greedy_and_wider_beam_can_differ(self):
        vocab, index, lexicon = self.world()
        # 泥 is better as a first step; 你好 is better once both characters are scored.
        table = {
            "": {"你": -1.0, "泥": -0.5},
            "你": {"好": -0.2, "号": -4.0},
            "泥": {"好": -4.0, "号": -2.0},
        }
        model = TableModel(vocab, table)
        wide, _ = gen.beam_search("nihao", "", model, vocab, index, lexicon, beam=8, mu=0.0)
        narrow, _ = gen.beam_search("nihao", "", model, vocab, index, lexicon, beam=1, mu=0.0)
        greedy, _ = gen.greedy_search("nihao", "", model, vocab, index, lexicon, mu=0.0)
        self.assertEqual(wide[0].text, "你好")
        self.assertEqual(narrow[0].text, greedy[0].text)
        self.assertEqual(narrow[0].tokens, greedy[0].tokens)
        self.assertEqual(narrow[0].score, greedy[0].score)
        self.assertEqual(greedy[0].text, "泥号")
        self.assertNotEqual(wide[0].text, greedy[0].text)
        for hyp in wide + narrow + greedy:
            self.assertTrue(gen.aligned("nihao", hyp.text, hyp.tokens, index))

    def test_equal_scores_use_the_same_tie_break(self):
        vocab, index, lexicon = self.world()
        table = {
            "": {"你": -1.0, "泥": -1.0},
            "你": {"好": -1.0, "号": -1.0},
            "泥": {"好": -1.0, "号": -1.0},
        }
        model = TableModel(vocab, table)
        narrow, _ = gen.beam_search("nihao", "", model, vocab, index, lexicon, beam=1, mu=0.0)
        greedy, _ = gen.greedy_search("nihao", "", model, vocab, index, lexicon, mu=0.0)
        self.assertEqual(narrow[0].text, greedy[0].text)
        self.assertEqual(narrow[0].score, greedy[0].score)

    def test_tied_logprob_keeps_the_longer_span(self):
        vocab, index, lexicon = self.world()
        # 好 and 号 are equally likely, and each is legal as h, ha, or hao.
        table = {"": {"你": -1.0}, "你": {"好": -1.0, "号": -1.0}}
        model = TableModel(vocab, table)
        ranked, _ = gen.greedy_search("nihao", "", model, vocab, index, lexicon, mu=0.0)
        self.assertEqual(ranked[0].tokens, ("ni", "hao"))
        self.assertTrue(gen.aligned("nihao", ranked[0].text, ranked[0].tokens, index))

    def test_initial_that_cannot_finish_the_keys_is_dropped(self):
        # "s" + "hi" cannot be segmented; only the full syllable reaches the end.
        vocab = vocab_of("是", "三")
        index = readings_of(vocab, [("是", "shi"), ("三", "san")])
        lexicon = gen.Lexicon({})
        model = TableModel(vocab, {"": {"三": 0.0, "是": -5.0}})
        ranked, _ = gen.greedy_search("shi", "", model, vocab, index, lexicon, mu=0.0)
        self.assertEqual(ranked[0].text, "是")
        self.assertEqual(ranked[0].tokens, ("shi",))
        self.assertEqual([(tok, end) for tok, end, _ in gen.segment_edges("shi")[0]], [("shi", 3)])

    def test_preferred_spellings_keep_complete_syllables(self):
        self.assertEqual([tok for tok, _, _ in gen.segment_edges("deng")[0]], ["deng"])
        self.assertEqual([tok for tok, _, _ in gen.segment_edges("lunzhizhi")[0]], ["lun"])
        first = [tok for tok, _, _ in gen.segment_edges("yaoqingzhi")[0]]
        self.assertIn("yao", first)
        self.assertNotIn("y", first)
        # The schema also opens a retained zh/ch/sh initial into its two letters.
        opened = [tok for tok, _, _ in gen.segment_edges("zh")[0]]
        self.assertEqual(opened, ["zh", "z"])
        z_node = next(node for tok, _, node in gen.segment_edges("zh")[0] if tok == "z")
        self.assertEqual([(tok, end) for tok, end, _ in gen.segment_edges("zh")[z_node]], [("h", 2)])
        # "he" is a real edge at the h, but the opened z must not take it.
        mixed = gen.segment_edges("llzher")
        z_node = next(node for tok, _, node in mixed[2] if tok == "z")
        self.assertEqual([tok for tok, _, _ in mixed[z_node]], ["h"])

    def test_lexicon_bonus_is_mu_times_log_frequency_of_a_completed_word(self):
        vocab, index, _ = self.world()
        lexicon = gen.Lexicon({"你好": 1000, "你": 99999})
        table = {
            "": {"你": -1.0, "泥": -0.9},
            "你": {"好": -1.0, "号": -3.0},
            "泥": {"好": -3.0, "号": -1.0},
        }
        model = TableModel(vocab, table)
        plain, _ = gen.beam_search("nihao", "", model, vocab, index, lexicon, beam=4, mu=0.0)
        rewarded, _ = gen.beam_search("nihao", "", model, vocab, index, lexicon, beam=4, mu=0.3)
        self.assertEqual(plain[0].text, "泥号")
        self.assertEqual(rewarded[0].text, "你好")
        plain_nihao = next(item for item in plain if item.text == "你好")
        rewarded_nihao = next(item for item in rewarded if item.text == "你好")
        import math
        self.assertAlmostEqual(
            rewarded_nihao.score - plain_nihao.score, 0.3 * math.log(1000), places=5)


if __name__ == "__main__":
    unittest.main()
