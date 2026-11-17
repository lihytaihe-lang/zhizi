#!/usr/bin/env python3
"""Build a pruned interpolated Kneser-Ney word bigram table.

The product lexicon is every entry under native/resources/Rime/*.dict.yaml
(maximum weight) plus the pinned pinyin_simp table that agent_pinyin imports.
Common words such as 我们 / 吃饭 live only in that base table; without it the
segmentation would fall through to single characters and the keys would not
match the decoder.

Smoothing is modified interpolated Kneser-Ney (Chen & Goodman). A kept bigram
stores log10 P(w2|w1), which already includes the interpolation term. A missing
bigram is log10 α(w1) + log10 P_cont(w2). After size pruning, α is recomputed
so the kept probabilities plus that backoff still sum to one. An unknown
history uses α = 1, i.e. the continuation distribution itself, so an unseen
pair is not given one flat penalty.

Pruning keeps the highest-count bigrams that fit the byte budget (ties break
on entropy gain, then on the two words). Pure entropy gain was tried and
rejected: it filled the budget with near-deterministic rare pairs (encyclopedia
fragments whose backoff probability is tiny) and left ordinary collocations
such as 吃饭|吧 on the backoff path.

Usage:
  python3 scripts/lexicon/build_word_bigram.py --out tmp/word-bigram
"""
from __future__ import annotations

import argparse
import hashlib
import json
import math
import re
import subprocess
import sys
import time
from pathlib import Path

BOS = "<s>"
EOS = "</s>"
REPO = Path(__file__).resolve().parents[2]
NO_CONT = -99.0
TIERS_MB = (5, 10, 20)
# format_log never writes more than 12 characters inside the [-99, 99] clamp.
_ALPHA_WIDTH = 12

_CLAUSE_RE = re.compile(r"(?:\\n|\\r|[。！？!?；;…\n\r])+")
# Chinese runs (middle dot may sit inside a name), latin words, digit runs.
_PIECE_RE = re.compile(
    r"[A-Za-z][A-Za-z0-9'_.+-]*|[0-9]+(?:\.[0-9]+)?|[\u3400-\u4dbf\u4e00-\u9fff\uf900-\ufaff·・]+"
)

CONTRASTS = (
    ("吃饭|吧", ("pair", "吃饭", "吧"), "吃|方便", ("pair", "吃", "方便")),
    ("已经|解决", ("pair", "已经", "解决"), "夜间经济整句", ("sent", "夜间经济")),
    ("已经解决整句", ("sent", "已经解决"), "夜间经济整句", ("sent", "夜间经济")),
    ("使用|搜狗", ("pair", "使用", "搜狗"), "是|也", ("pair", "是", "也")),
    ("天气|不错", ("pair", "天气", "不错"), "天气|把", ("pair", "天气", "把")),
    ("我们下次去吃饭整句", ("sent", "我们下次去吃饭"), "我们下次去吃发布整句", ("sent", "我们下次去吃发布")),
    ("我们|明天", ("pair", "我们", "明天"), "我们|发布", ("pair", "我们", "发布")),
    ("今天|天气", ("pair", "今天", "天气"), "今天|发布", ("pair", "今天", "发布")),
    ("没有|问题", ("pair", "没有", "问题"), "没有|把", ("pair", "没有", "把")),
    ("一个|人", ("pair", "一个", "人"), "一个|把", ("pair", "一个", "把")),
    ("怎么|样", ("pair", "怎么", "样"), "怎么|把", ("pair", "怎么", "把")),
    ("可以|吗", ("pair", "可以", "吗"), "可以|把", ("pair", "可以", "把")),
    ("已经|完成", ("pair", "已经", "完成"), "已经|方便", ("pair", "已经", "方便")),
    ("感觉|不错", ("pair", "感觉", "不错"), "感觉|把", ("pair", "感觉", "把")),
    ("一起|去", ("pair", "一起", "去"), "一起|把", ("pair", "一起", "把")),
    ("非常|好", ("pair", "非常", "好"), "非常|把", ("pair", "非常", "把")),
    ("开始|工作", ("pair", "开始", "工作"), "开始|把", ("pair", "开始", "把")),
    ("谢谢|你", ("pair", "谢谢", "你"), "谢谢|发布", ("pair", "谢谢", "发布")),
    ("希望|你", ("pair", "希望", "你"), "希望|把", ("pair", "希望", "把")),
    ("下次|见", ("pair", "下次", "见"), "下次|发布", ("pair", "下次", "发布")),
)


def format_log(value):
    """Fixed 8-decimal log10 text. Values at or below -99 mean 'no mass'."""
    if value != value or value <= NO_CONT:
        return "-99.00000000"
    if value >= 99.0:
        return "99.00000000"
    return f"{value:.8f}"


def log10_stored(probability):
    """log10 rounded the same way the TSV writes it."""
    if probability <= 0.0:
        return NO_CONT
    return float(format_log(math.log10(probability)))


def modified_kn_discounts(n1, n2, n3, n4):
    """Chen & Goodman modified discounts (D1, D2, D3+).

    Falls back when a count-of-counts is zero, and clamps so c - D(c) stays
    positive (the interpolation identity needs that).
    """
    if n1 <= 0 or n2 <= 0:
        return (0.5, 0.5, 0.5)
    y = n1 / (n1 + 2.0 * n2)
    d1 = 1.0 - 2.0 * y * (n2 / n1)
    d2 = 2.0 - 3.0 * y * (n3 / n2) if n3 > 0 else d1
    d3 = 3.0 - 4.0 * y * (n4 / n3) if n3 > 0 and n4 > 0 else d2

    def clamp(value, hi):
        if value != value or value < 0.0:
            return 0.0
        if value > hi:
            return hi
        return value

    return (clamp(d1, 0.999999), clamp(d2, 1.999999), clamp(d3, 2.999999))


def _discount(count, d1, d2, d3):
    if count <= 1:
        return d1
    if count == 2:
        return d2
    return d3


def load_weights(paths):
    """word -> max positive integer weight. Comments, blanks, weight <= 0 are skipped."""
    weights = {}
    for path in paths:
        in_body = False
        with Path(path).open(encoding="utf-8-sig") as handle:
            for line in handle:
                line = line.rstrip("\n").rstrip("\r")
                if not in_body:
                    if line.strip() == "...":
                        in_body = True
                    continue
                if not line or line.startswith("#"):
                    continue
                parts = line.split("\t")
                if len(parts) < 3 or not parts[0]:
                    continue
                try:
                    weight = int(float(parts[-1]))
                except ValueError:
                    continue
                if weight <= 0:
                    continue
                word = parts[0]
                previous = weights.get(word)
                if previous is None or weight > previous:
                    weights[word] = weight
    return weights


class _Node:
    __slots__ = ("child", "logw")

    def __init__(self):
        self.child = {}
        self.logw = None


class Lexicon:
    """Maximum-probability segmentation over a weighted word set.

    P(word) = weight / sum(weights). A character that is not itself a dictionary
    word scores as weight 1, including a character that is only a prefix of a
    longer word. Ties keep the segmentation already recorded, which is the
    longer span from the left.
    """

    def __init__(self, weights):
        if not weights:
            raise ValueError("lexicon is empty")
        self.words = set(weights)
        total = float(sum(weights.values()))
        log_total = math.log(total)
        self.oov_logw = -log_total
        self.root = _Node()
        self.max_len = 1
        for word, weight in weights.items():
            node = self.root
            for char in word:
                nxt = node.child.get(char)
                if nxt is None:
                    nxt = _Node()
                    node.child[char] = nxt
                node = nxt
            node.logw = math.log(weight) - log_total
            if len(word) > self.max_len:
                self.max_len = len(word)

    def __contains__(self, word):
        return word in self.words

    def segment(self, text):
        n = len(text)
        if n == 0:
            return []
        neg = -1e300
        best = [neg] * (n + 1)
        back = list(range(-1, n))
        best[0] = 0.0
        root = self.root
        oov = self.oov_logw
        for i in range(n):
            score = best[i]
            if score < -1e299:
                continue
            node = root
            j = i
            while j < n:
                node = node.child.get(text[j])
                if node is None:
                    if j == i:
                        cand = score + oov
                        if cand > best[i + 1]:
                            best[i + 1] = cand
                            back[i + 1] = i
                    break
                j += 1
                if node.logw is not None:
                    cand = score + node.logw
                    if cand > best[j]:
                        best[j] = cand
                        back[j] = i
                elif j == i + 1:
                    cand = score + oov
                    if cand > best[i + 1]:
                        best[i + 1] = cand
                        back[i + 1] = i
        spans = []
        i = n
        while i > 0:
            j = back[i]
            if j < 0 or j >= i:
                j = i - 1
            spans.append(text[j:i])
            i = j
        spans.reverse()
        return spans


def _latin_token(lexicon, span):
    if span in lexicon.words:
        return span
    lowered = span.lower()
    if lowered in lexicon.words:
        return lowered
    return span


def tokenize_line(lexicon, line):
    """Split one physical line into sentences of word tokens.

    。！？ and a stored '\\n' end a sentence. Commas and quotes are skipped, so
    the words on either side become neighbors. Latin and digit runs are one
    token each; other out-of-dictionary characters stay single characters.
    """
    text = line.replace("\\n", "\n").replace("\\r", "\n")
    sentences = []
    for clause in _CLAUSE_RE.split(text):
        tokens = []
        for match in _PIECE_RE.finditer(clause):
            span = match.group(0)
            first = span[0]
            if first.isascii() and first.isalpha():
                tokens.append(_latin_token(lexicon, span))
            elif first.isdigit():
                tokens.append(span)
            else:
                tokens.extend(lexicon.segment(span))
        if tokens:
            sentences.append(tokens)
    return sentences


def _add(bigram, left, right):
    left = sys.intern(left)
    right = sys.intern(right)
    inner = bigram.get(left)
    if inner is None:
        inner = {}
        bigram[left] = inner
    inner[right] = inner.get(right, 0) + 1


def _continuation(bigram, dictionary_words, delta):
    """P_cont(w) = (N1+(·, w) + δ) / Z over observed successors and dictionary words.

    δ is added only to dictionary words and </s>, so a product word that never
    occurred still has a small continuation instead of probability zero.
    """
    counts = {}
    for _left, succs in bigram.items():
        for right, count in succs.items():
            if count > 0:
                counts[right] = counts.get(right, 0) + 1
    extra = set(dictionary_words)
    extra.discard(BOS)
    if delta > 0.0:
        extra.add(EOS)
    support = {word for word, count in counts.items() if count > 0}
    if delta > 0.0:
        support |= extra
    support.discard(BOS)
    raw_sum = 0.0
    raw = {}
    for word in support:
        added = delta if delta > 0.0 and word in extra else 0.0
        value = counts.get(word, 0) + added
        if value <= 0.0:
            continue
        raw[word] = value
        raw_sum += value
    if raw_sum <= 0.0:
        raise ValueError("continuation distribution is empty")
    return {word: value / raw_sum for word, value in raw.items()}, counts


class FullModel:
    """Unpruned modified interpolated KN, plus an entropy ranking of bigrams."""

    def __init__(self, bigram, pcont, gamma, prob, ranked, discounts, delta):
        self.bigram = bigram
        self._pcont = pcont
        self._gamma = gamma
        self._prob = prob
        self.ranked = ranked
        self.discounts = discounts
        self.delta = delta

    def contexts(self):
        return list(self.bigram.keys())

    def successors(self, left):
        return self.bigram.get(left, {})

    def gamma(self, left):
        return self._gamma[left]

    def pcont(self, word):
        return self._pcont[word]

    def raw_mass(self, left):
        """Exact probability mass of one history, before log rounding."""
        seen = self.bigram[left]
        mass = 0.0
        for right in seen:
            mass += self._prob[(left, right)]
        backed = self._gamma[left]
        for word, cont in self._pcont.items():
            if word not in seen:
                mass += backed * cont
        return mass

    def materialize(self, target_bytes=None):
        if target_bytes is None:
            return self._materialize_all()
        return self._materialize_budget(target_bytes)

    def _materialize_all(self):
        lm = WordBigramLM()
        lm.omitted_count = 0
        floor = min(self._pcont.values()) * 0.1 if self._pcont else 1e-12
        lm.pcont_floor = floor if floor > 0.0 else 1e-12
        lm.floor_log = log10_stored(lm.pcont_floor)
        for (left, right), prob in self._prob.items():
            lm.bigram[(left, right)] = log10_stored(prob)
        for word, cont in self._pcont.items():
            lm.log_pcont[word] = log10_stored(cont)
        for left, backed in self._gamma.items():
            lm.log_alpha[left] = log10_stored(backed) if backed > 0.0 else NO_CONT
        return lm

    def _materialize_budget(self, target_bytes):
        """Keep a count-prefix of bigrams, then fill the leftover with P_cont rows.

        Alpha text is budgeted at its maximum width, so the written tables stay
        within `target_bytes` unless a single bigram is larger than the budget.
        α is then recomputed from the rounded probabilities that will be stored.
        """
        log_pc = {word: format_log(math.log10(cont)) for word, cont in self._pcont.items()}
        width = {word: len(word.encode("utf-8")) for word in self._pcont}
        kept = []
        seen = set()
        used = 0
        for _count, _score, left, right, prob, cont in self.ranked:
            if prob <= 0.0 or cont <= 0.0 or right not in log_pc:
                continue
            if left not in width:
                width[left] = len(left.encode("utf-8"))
            logp = format_log(math.log10(prob))
            add = width[left] + width[right] + len(logp) + 3
            fresh = []
            for word in (left, right):
                if word not in seen and word not in fresh:
                    fresh.append(word)
                    pc_text = log_pc.get(word, "-99.00000000")
                    add += width[word] + len(pc_text) + _ALPHA_WIDTH + 3
            if kept and used + add > target_bytes:
                break
            kept.append((left, right, logp, log_pc[right]))
            used += add
            seen.update(fresh)

        sum_p = {}
        sum_pc = {}
        for left, _right, logp, logpc in kept:
            sum_p[left] = sum_p.get(left, 0.0) + (10 ** float(logp))
            sum_pc[left] = sum_pc.get(left, 0.0) + (10 ** float(logpc))
        alpha_text = {}
        for left, mass in sum_p.items():
            denom = 1.0 - sum_pc[left]
            numer = 1.0 - mass
            if denom <= 1e-12 or numer <= 0.0:
                alpha = 1e-12
            else:
                alpha = min(max(numer / denom, 1e-12), 1e6)
            alpha_text[left] = format_log(math.log10(alpha))

        used = 0
        seen = set()
        for left, right, logp, _logpc in kept:
            used += width[left] + width[right] + len(logp) + 3
            seen.add(left)
            seen.add(right)
        for word in seen:
            pc_text = log_pc.get(word, "-99.00000000")
            used += width[word] + len(pc_text) + len(alpha_text.get(word, "0.00000000")) + 3

        for word, _cont in sorted(self._pcont.items(), key=lambda item: (-item[1], item[0])):
            if word in seen:
                continue
            pc_text = log_pc[word]
            add = width[word] + len(pc_text) + len("0.00000000") + 3
            if used + add > target_bytes:
                continue
            seen.add(word)
            used += add

        lm = WordBigramLM()
        for left, right, logp, _logpc in kept:
            lm.bigram[(left, right)] = float(logp)
        for word in seen:
            if word in log_pc:
                lm.log_pcont[word] = float(log_pc[word])
            lm.log_alpha[word] = float(alpha_text.get(word, "0.00000000"))
        stored_mass = 0.0
        for value in lm.log_pcont.values():
            stored_mass += 10 ** value
        omitted = [word for word in self._pcont if word not in lm.log_pcont]
        lm.omitted_count = len(omitted)
        if lm.omitted_count:
            lm.pcont_floor = max(0.0, 1.0 - stored_mass) / lm.omitted_count
        else:
            smallest = min(self._pcont.values()) * 0.1 if self._pcont else 1e-12
            lm.pcont_floor = smallest if smallest > 0.0 else 1e-12
        lm.floor_log = log10_stored(lm.pcont_floor) if lm.pcont_floor > 0.0 else NO_CONT
        lm.payload_bytes = used
        return lm


def model_from_counts(bigram, dictionary_words, delta):
    pcont, _counts = _continuation(bigram, dictionary_words, delta)
    n1 = n2 = n3 = n4 = 0
    ctx_total = {}
    ctx_buckets = {}
    for left, succs in bigram.items():
        total = 0
        b1 = b2 = b3 = 0
        for _right, count in succs.items():
            total += count
            if count == 1:
                n1 += 1
                b1 += 1
            elif count == 2:
                n2 += 1
                b2 += 1
            elif count == 3:
                n3 += 1
                b3 += 1
            else:
                b3 += 1
                if count == 4:
                    n4 += 1
        ctx_total[left] = total
        ctx_buckets[left] = (b1, b2, b3)
    d1, d2, d3 = modified_kn_discounts(n1, n2, n3, n4)
    gamma = {}
    for left, total in ctx_total.items():
        b1, b2, b3 = ctx_buckets[left]
        gamma[left] = (d1 * b1 + d2 * b2 + d3 * b3) / total if total else 0.0
    prob = {}
    ranked = []
    for left, succs in bigram.items():
        total = ctx_total[left]
        backed = gamma[left]
        for right, count in succs.items():
            cont = pcont.get(right, 0.0)
            if cont <= 0.0 or total <= 0:
                continue
            discounted = _discount(count, d1, d2, d3)
            value = (count - discounted) / total + backed * cont
            if value <= 0.0:
                continue
            if value > 1.0:
                value = 1.0
            prob[(left, right)] = value
            backoff = backed * cont
            if backoff <= 0.0:
                score = count * value
            elif value <= backoff:
                score = 0.0
            else:
                score = count * value * math.log(value / backoff)
            ranked.append((count, score, left, right, value, cont))
    # Count first. Entropy gain only breaks ties: ranking by entropy alone
    # keeps rare, nearly certain pairs and drops ordinary collocations.
    ranked.sort(key=lambda item: (-item[0], -item[1], item[2], item[3]))
    return FullModel(bigram, pcont, gamma, prob, ranked, (d1, d2, d3, n1, n2, n3, n4), delta)


def fit_kn(sentences, dictionary_words, delta):
    bigram = {}
    for sentence in sentences:
        if not sentence:
            continue
        previous = BOS
        for token in sentence:
            _add(bigram, previous, token)
            previous = token
        _add(bigram, previous, EOS)
    if not bigram:
        raise ValueError("no sentences")
    return model_from_counts(bigram, dictionary_words, delta)


class WordBigramLM:
    """Queryable pruned table. logprob(w1, w2) is log10 P(w2|w1) with backoff."""

    def __init__(self):
        self.bigram = {}
        self.log_pcont = {}
        self.log_alpha = {}
        self.default_log_alpha = 0.0  # unknown history: α = 1, pure continuation
        self.omitted_count = 0
        self.pcont_floor = 1e-12
        self.floor_log = -12.0
        self.payload_bytes = 0

    def backoff_logprob(self, left, right):
        alpha = self.log_alpha.get(left, self.default_log_alpha)
        cont = self.log_pcont.get(right)
        if cont is None:
            cont = self.floor_log
        return alpha + cont

    def logprob(self, left, right):
        hit = self.bigram.get((left, right))
        if hit is not None:
            return hit
        return self.backoff_logprob(left, right)


def context_mass(lm, left):
    """Closed-vocabulary mass: explicit bigrams + α · remaining continuation.

    Omitted unigrams share `pcont_floor` (`omitted_count` of them). Strings
    outside that set are not part of the sum.
    """
    mass = 0.0
    seen = set()
    for (history, word), logp in lm.bigram.items():
        if history == left:
            mass += 10 ** logp
            seen.add(word)
    alpha = 10 ** lm.log_alpha.get(left, lm.default_log_alpha)
    for word, logp in lm.log_pcont.items():
        if word not in seen:
            mass += alpha * (10 ** logp)
    if lm.omitted_count:
        mass += alpha * lm.omitted_count * lm.pcont_floor
    return mass


def _sha256(payload):
    return hashlib.sha256(payload).hexdigest()


def write_tier(directory, lm, meta):
    directory.mkdir(parents=True, exist_ok=True)
    bigram_lines = [
        f"{left}\t{right}\t{format_log(logp)}\n"
        for (left, right), logp in lm.bigram.items()
    ]
    bigram_lines.sort()
    bigram_payload = "".join(bigram_lines).encode("utf-8")
    words = set(lm.log_pcont) | set(lm.log_alpha)
    unigram_lines = []
    for word in words:
        cont = lm.log_pcont.get(word)
        cont_text = format_log(cont) if cont is not None else "-99.00000000"
        alpha_text = format_log(lm.log_alpha.get(word, lm.default_log_alpha))
        unigram_lines.append(f"{word}\t{cont_text}\t{alpha_text}\n")
    unigram_lines.sort()
    unigram_payload = "".join(unigram_lines).encode("utf-8")
    (directory / "bigrams.tsv").write_bytes(bigram_payload)
    (directory / "unigrams.tsv").write_bytes(unigram_payload)
    manifest = dict(meta)
    manifest["bigram_count"] = len(lm.bigram)
    manifest["unigram_count"] = len(words)
    manifest["bigrams_sha256"] = _sha256(bigram_payload)
    manifest["unigrams_sha256"] = _sha256(unigram_payload)
    manifest["bigrams_bytes"] = len(bigram_payload)
    manifest["unigrams_bytes"] = len(unigram_payload)
    manifest["table_bytes"] = len(bigram_payload) + len(unigram_payload)
    manifest["pcont_floor"] = lm.pcont_floor
    manifest["omitted_unigram_count"] = lm.omitted_count
    manifest["default_log_alpha"] = lm.default_log_alpha
    text = json.dumps(manifest, ensure_ascii=False, indent=2, sort_keys=True) + "\n"
    (directory / "manifest.json").write_text(text, encoding="utf-8")
    return manifest


def load_tier(directory):
    directory = Path(directory)
    manifest = json.loads((directory / "manifest.json").read_text(encoding="utf-8"))
    lm = WordBigramLM()
    lm.pcont_floor = float(manifest["pcont_floor"])
    lm.omitted_count = int(manifest["omitted_unigram_count"])
    lm.default_log_alpha = float(manifest.get("default_log_alpha", 0.0))
    lm.floor_log = log10_stored(lm.pcont_floor) if lm.pcont_floor > 0.0 else NO_CONT
    for line in (directory / "bigrams.tsv").read_text(encoding="utf-8").splitlines():
        if not line:
            continue
        left, right, logp = line.split("\t")
        lm.bigram[(left, right)] = float(logp)
    for line in (directory / "unigrams.tsv").read_text(encoding="utf-8").splitlines():
        if not line:
            continue
        word, cont, alpha = line.split("\t")
        lm.log_alpha[word] = float(alpha)
        cont_value = float(cont)
        if cont_value > -98.0:
            lm.log_pcont[word] = cont_value
    return lm


def iter_capped_lines(path, cap_chars, seed):
    """Yield every line, or a seeded stride when the file is larger than cap_chars.

    `cap_chars <= 0` means no cap. The stride phase is `seed % period`, so the
    same seed reads the same lines and a different seed shifts the residue class.
    """
    path = Path(path)
    if ".valid." in path.name:
        raise ValueError(f"refusing to read a held-out split: {path}")
    if cap_chars is None or cap_chars <= 0:
        with path.open("rb") as handle:
            for raw in handle:
                yield raw.decode("utf-8")
        return
    total = 0
    count = 0
    with path.open("rb") as handle:
        for raw in handle:
            total += len(raw.decode("utf-8"))
            count += 1
    if count == 0 or total <= cap_chars:
        with path.open("rb") as handle:
            for raw in handle:
                yield raw.decode("utf-8")
        return
    period = max(1, int(round(total / float(cap_chars))))
    offset = seed % period
    with path.open("rb") as handle:
        for index, raw in enumerate(handle):
            if index % period == offset:
                yield raw.decode("utf-8")


def iter_train_lines(path, holdout_ratio, holdout):
    """Yield the training prefix. The last `holdout_ratio` of bytes, cut on a
    line boundary, is appended to `holdout` and not yielded."""
    path = Path(path)
    if ".valid." in path.name:
        raise ValueError(f"refusing to read a held-out split: {path}")
    size = path.stat().st_size
    cut = int(size * (1.0 - holdout_ratio)) if holdout_ratio > 0.0 else size + 1
    with path.open("rb") as handle:
        while True:
            pos = handle.tell()
            raw = handle.readline()
            if not raw:
                break
            line = raw.decode("utf-8")
            if pos >= cut:
                holdout.append(line)
            else:
                yield line


def _consume(lexicon, lines, bigram, progress_name=""):
    chars = 0
    tokens = 0
    sentences = 0
    next_report = 5000000
    for line in lines:
        chars += len(line.rstrip("\r\n"))
        for sentence in tokenize_line(lexicon, line):
            sentences += 1
            previous = BOS
            for token in sentence:
                tokens += 1
                _add(bigram, previous, token)
                previous = token
            _add(bigram, previous, EOS)
        if progress_name and chars >= next_report:
            print(f"{progress_name}: {chars} chars, {tokens} tokens", file=sys.stderr, flush=True)
            next_report += 5000000
    return chars, tokens, sentences


def build_from_files(sources, lexicon, out_dir, tiers_mb, wiki_chars, seed,
                     synth_holdout, delta, dictionary_words=None):
    """Count, fit KN, and write one directory per tier (`5mb`, `10mb`, ...).

    `sources` is `(name, path, license)`. The wikipedia/wiki source is stride-
    capped when `wiki_chars > 0`. synth keeps its final `synth_holdout` fraction
    out of the counts and returns those lines for perplexity.
    """
    if dictionary_words is None:
        dictionary_words = set(lexicon.words)
    out_dir = Path(out_dir)
    bigram = {}
    source_stats = {}
    holdout = []
    started = time.perf_counter()
    for name, path, license_name in sources:
        path = Path(path)
        if name == "synth" and synth_holdout > 0.0:
            lines = iter_train_lines(path, synth_holdout, holdout)
            cap_note = None
        elif name in ("wiki", "wikipedia") and wiki_chars and wiki_chars > 0:
            lines = iter_capped_lines(path, wiki_chars, seed)
            cap_note = wiki_chars
        else:
            lines = iter_capped_lines(path, 0, seed)
            cap_note = None
        chars, tokens, sentences = _consume(lexicon, lines, bigram, progress_name=name)
        source_stats[name] = {
            "path": str(path),
            "license": license_name,
            "chars": chars,
            "tokens": tokens,
            "sentences": sentences,
            "char_cap": cap_note,
        }
        print(f"{name}: chars={chars} tokens={tokens} sentences={sentences}", file=sys.stderr)
    counted = time.perf_counter()
    if not bigram:
        raise ValueError("corpus produced no tokens")
    full = model_from_counts(bigram, dictionary_words, delta)
    # Counts and the explicit probability map are only needed to build `ranked`.
    # Drop them before the three tier writes so the prune step doesn't hold
    # a second copy of every bigram.
    full.bigram = {}
    full._prob = {}
    bigram.clear()
    fitted = time.perf_counter()
    holdout_chars = sum(len(line.rstrip("\r\n")) for line in holdout)
    models = {}
    manifests = {}
    base_meta = {
        "smoothing": "modified-interpolated-kneser-ney",
        "delta": delta,
        "seed": seed,
        "wiki_chars": wiki_chars,
        "synth_holdout_ratio": synth_holdout,
        "synth_holdout_chars": holdout_chars,
        "sources": source_stats,
        "unique_bigrams": len(full.ranked),
        "discounts": {
            "D1": full.discounts[0],
            "D2": full.discounts[1],
            "D3": full.discounts[2],
            "n1": full.discounts[3],
            "n2": full.discounts[4],
            "n3": full.discounts[5],
            "n4": full.discounts[6],
        },
        "lexicon_words": len(dictionary_words),
    }
    for mb in tiers_mb:
        target = int(mb) * 1024 * 1024
        lm = full.materialize(target_bytes=target)
        meta = dict(base_meta)
        meta["tier"] = f"{int(mb)}mb"
        meta["target_bytes"] = target
        manifests[f"{int(mb)}mb"] = write_tier(out_dir / f"{int(mb)}mb", lm, meta)
        models[f"{int(mb)}mb"] = lm
        print(
            f"tier {int(mb)}mb: bigrams={len(lm.bigram)} unigrams={len(lm.log_pcont)} "
            f"bytes={manifests[f'{int(mb)}mb']['table_bytes']}",
            file=sys.stderr,
        )
    done = time.perf_counter()
    return {
        "sources": source_stats,
        "holdout": holdout,
        "holdout_chars": holdout_chars,
        "models": models,
        "manifests": manifests,
        "seconds": {"count": counted - started, "kn": fitted - counted, "write": done - fitted},
        "unique_bigrams": len(full.ranked),
        "discounts": base_meta["discounts"],
        "delta": delta,
        "seed": seed,
        "continuation_types": len(full._pcont),
    }


def sentence_log10(lm, lexicon, text):
    """Sum of log10 P along the segmented sentence, including <s> and </s>."""
    sentences = tokenize_line(lexicon, text)
    total = 0.0
    tokens = []
    predictions = 0
    for sentence in sentences:
        tokens.extend(sentence)
        previous = BOS
        for token in sentence:
            total += lm.logprob(previous, token)
            previous = token
            predictions += 1
        total += lm.logprob(previous, EOS)
        predictions += 1
    return {"tokens": tokens, "log10": total, "predictions": predictions}


def _contrast_value(lm, lexicon, spec):
    kind = spec[0]
    if kind == "pair":
        return {"log10": lm.logprob(spec[1], spec[2]), "tokens": [spec[1], spec[2]]}
    return sentence_log10(lm, lexicon, spec[1])


def contrast_report(models, lexicon):
    rows = []
    for left_name, left_spec, right_name, right_spec in CONTRASTS:
        row = {"left": left_name, "right": right_name, "tiers": {}}
        for name, lm in models.items():
            left = _contrast_value(lm, lexicon, left_spec)
            right = _contrast_value(lm, lexicon, right_spec)
            row["tiers"][name] = {
                "left_log10": left["log10"],
                "right_log10": right["log10"],
                "left_tokens": left["tokens"],
                "right_tokens": right["tokens"],
            }
        rows.append(row)
    return rows


def perplexity(lm, lexicon, lines):
    total = 0.0
    predictions = 0
    sentences = 0
    for line in lines:
        for sentence in tokenize_line(lexicon, line):
            sentences += 1
            previous = BOS
            for token in sentence:
                total += lm.logprob(previous, token)
                previous = token
                predictions += 1
            total += lm.logprob(previous, EOS)
            predictions += 1
    if predictions == 0:
        return {"predictions": 0, "sentences": 0, "log10": 0.0, "perplexity": None}
    return {
        "predictions": predictions,
        "sentences": sentences,
        "mean_log10": total / predictions,
        "perplexity": 10 ** (-total / predictions),
    }


def resolve_base(repo):
    """Pinned pinyin_simp. A linked worktree keeps it in the main checkout."""
    repo = Path(repo)
    matches = sorted((repo / ".build-deps/rime-pinyin-simp").glob("*/pinyin_simp.dict.yaml"))
    if matches:
        return matches[0]
    try:
        common = subprocess.check_output(
            ["git", "-C", str(repo), "rev-parse", "--git-common-dir"], text=True
        ).strip()
    except (OSError, subprocess.CalledProcessError):
        return None
    main = Path(common).resolve().parent
    matches = sorted((main / ".build-deps/rime-pinyin-simp").glob("*/pinyin_simp.dict.yaml"))
    return matches[0] if matches else None


def default_sources(corpus_dir):
    corpus_dir = Path(corpus_dir)
    return [
        ("asap", corpus_dir / "asap.train.txt", "Apache-2.0"),
        ("wikipedia", corpus_dir / "wikipedia.train.txt", "CC BY-SA 4.0"),
        ("synth", corpus_dir / "synth.train.txt",
         "generated (DeepSeek allowed; GLM disallowed for release; Qwen unconfirmed)"),
        ("tatoeba", corpus_dir / "tatoeba.train.txt", "CC BY 2.0 FR"),
    ]


def run(argv):
    parser = argparse.ArgumentParser(description="Build pruned word-bigram tables.")
    parser.add_argument("--repo", type=Path, default=REPO)
    parser.add_argument("--dict-dir", type=Path, default=None)
    parser.add_argument("--base", type=Path, default=None,
                        help="pinyin_simp dictionary. Default: the pinned .build-deps copy.")
    parser.add_argument("--no-base", action="store_true",
                        help="Do not load pinyin_simp. Segmentation then misses ordinary words.")
    parser.add_argument("--corpus-dir", type=Path, default=None)
    parser.add_argument("--out", type=Path, default=None)
    parser.add_argument("--wiki-chars", type=int, default=40000000,
                        help="Stride cap for wikipedia (seed selects the residue class). "
                             "0 reads the whole train file. The default keeps written text "
                             "from crowding spoken collocations out of a count-pruned table.")
    parser.add_argument("--synth-holdout", type=float, default=0.02)
    parser.add_argument("--seed", type=int, default=0)
    parser.add_argument("--delta", type=float, default=0.1)
    parser.add_argument("--tiers", default="5,10,20",
                        help="Comma-separated size targets in MB.")
    args = parser.parse_args(argv)
    repo = args.repo
    dict_dir = args.dict_dir or (repo / "native/resources/Rime")
    corpus_dir = args.corpus_dir or (repo / "ime_context_model/corpus/v2")
    out_dir = args.out or (repo / "tmp/word-bigram")
    paths = sorted(dict_dir.glob("*.dict.yaml"))
    if not paths:
        raise SystemExit(f"no dictionaries in {dict_dir}")
    base = None
    if not args.no_base:
        base = args.base or resolve_base(repo)
        if base is None:
            raise SystemExit("pinned pinyin_simp not found; pass --base or --no-base")
        paths.append(Path(base))
    print(f"loading {len(paths)} dictionaries", file=sys.stderr)
    weights = load_weights(paths)
    lexicon = Lexicon(weights)
    print(f"lexicon words={len(weights)} max_len={lexicon.max_len}", file=sys.stderr)
    tiers = tuple(int(part) for part in args.tiers.split(",") if part.strip())
    sources = default_sources(corpus_dir)
    for _name, path, _license in sources:
        if not path.is_file():
            raise SystemExit(f"missing corpus {path}")
    result = build_from_files(
        sources, lexicon, out_dir, tiers, args.wiki_chars, args.seed,
        args.synth_holdout, args.delta, dictionary_words=set(weights),
    )
    report = {
        "sources": result["sources"],
        "holdout_chars": result["holdout_chars"],
        "seconds": result["seconds"],
        "unique_bigrams": result["unique_bigrams"],
        "continuation_types": result["continuation_types"],
        "discounts": result["discounts"],
        "delta": result["delta"],
        "seed": result["seed"],
        "wiki_chars": args.wiki_chars,
        "base": None if base is None else str(base),
        "lexicon_words": len(weights),
        "tiers": result["manifests"],
        "perplexity": {},
        "contrasts": contrast_report(result["models"], lexicon),
    }
    for name, lm in result["models"].items():
        report["perplexity"][name] = perplexity(lm, lexicon, result["holdout"])
        print(
            f"perplexity {name}: {report['perplexity'][name].get('perplexity')}",
            file=sys.stderr,
        )
    out_dir.mkdir(parents=True, exist_ok=True)
    (out_dir / "selfcheck.json").write_text(
        json.dumps(report, ensure_ascii=False, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    print(f"wrote {out_dir}", file=sys.stderr)
    return report


def main():
    run(sys.argv[1:])


if __name__ == "__main__":
    main()
