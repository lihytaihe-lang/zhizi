"""Pinyin-constrained character search with the shipped 知字 ONNX model.

The host scores a candidate by gathering the log-probability of each of its
characters (native/host/src/ranking.rs). This search asks for the next
character under the keys still left. The exported graph already computes the
full head logits and then gathers one label; the last position of those logits
is the next-character distribution. A startup check compares it with the
gathered output.

Input window, for one new character: [bos] plus the last min(len, 38)
characters of context + text already generated. That is ranking.rs with
longest = 1 (keep = min(context, 39 - longest)).

Key spans follow the product: a token is a full syllable or an initial
(zh/ch/sh are one initial; a/o/e and y/w-initial syllables abbreviate to the
first letter). An apostrophe ends a token and is consumed with it.
"""
import argparse
import json
import math
import random
import re
import time
from collections import defaultdict
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parents[3]
MIXER = ROOT / "native/core/agent-ime-core/src/mixer.rs"
INITIAL_LETTERS = frozenset("aeobpmfdtnlgkhjqxrzcsyw")
COREML_MS_PER_8 = 2.9
BEAMS = (4, 8, 16)
MUS = (0.0, 0.1, 0.3)
SEED = 20261111


def _load_syllables():
    text = MIXER.read_text(encoding="utf-8")
    match = re.search(r'const PINYIN_SYLLABLES: &str = "(.*?)";', text, re.S)
    if not match:
        raise RuntimeError("PINYIN_SYLLABLES not found in mixer.rs")
    return frozenset(match.group(1).split())


SYLLABLES = _load_syllables()


def canonical_syllable(token):
    """Same spelling fold as zhixu::decoder::canonical_syllable."""
    if ("uen" not in token and "uei" not in token and "iou" not in token
            and token not in ("nve", "lve")
            and not (token[:1] in "jqxy" and len(token) > 1 and token[1] == "v")):
        return token
    spelling = token
    if spelling[:1] in "jqxy" and len(spelling) > 1 and spelling[1] == "v":
        spelling = spelling[0] + "u" + spelling[2:]
    if spelling in ("nve", "lve"):
        spelling = spelling[0] + "u" + spelling[2:]
    return spelling.replace("uen", "un").replace("uei", "ui").replace("iou", "iu")


def full_syllable(token):
    canonical = canonical_syllable(token)
    spelling = {"nue": "nve", "lue": "lve"}.get(canonical, canonical)
    return spelling in SYLLABLES and spelling not in {"m", "n", "ng", "hm", "hng"}


def initial(token):
    return token in ("zh", "ch", "sh") or (len(token) == 1 and token in INITIAL_LETTERS)


def syllable_matches(token, syllable):
    return (token == syllable
            or (initial(token) and syllable.startswith(token))
            or canonical_syllable(token) == canonical_syllable(syllable))


def next_spans(keys, pos):
    """Legal next tokens and the key index after each, including trailing apostrophes."""
    found = []
    limit = min(len(keys), pos + 6)
    end = pos + 1
    while end <= limit:
        token = keys[pos:end]
        if "'" in token:
            break
        if initial(token) or full_syllable(token):
            nxt = end
            while nxt < len(keys) and keys[nxt] == "'":
                nxt += 1
            found.append((token, nxt))
        end += 1
    return found


def segment_edges(keys):
    """Preferred spelling edges from the product decoder.

    Among segmentations that reach the furthest point, keep the ones with the
    fewest abbreviated initials, so a complete syllable is not split into its
    interior (deng, not d+eng). A retained zh/ch/sh initial is also opened as
    its two letters, which the schema keeps beside the two-letter initial.
    """
    size = len(keys)
    raw = [[] for _ in range(size + 1)]
    reachable = [False] * (size + 1)
    reachable[0] = True
    for start in range(size):
        if not reachable[start]:
            continue
        for token, nxt in next_spans(keys, start):
            raw[start].append((token, nxt))
            reachable[nxt] = True
    last = 0
    for index, ok in enumerate(reachable):
        if ok:
            last = index
    huge = size + 2
    abbr = [huge] * (size + 1)
    abbr[last] = 0
    for start in range(last - 1, -1, -1):
        best = huge
        for token, nxt in raw[start]:
            if abbr[nxt] >= huge:
                continue
            cost = abbr[nxt] + (0 if full_syllable(token) else 1)
            if cost < best:
                best = cost
        abbr[start] = best
    edges = [[] for _ in range(size + 1)]
    for start, options in enumerate(raw):
        for token, nxt in options:
            if abbr[nxt] >= huge:
                continue
            extra = 0 if full_syllable(token) else 1
            if abbr[start] != abbr[nxt] + extra:
                continue
            edges[start].append((token, nxt, nxt))
            if token not in ("zh", "ch", "sh"):
                continue
            # The second letter must not see other edges at that key position.
            mid = len(edges)
            edges.append([(token[1], nxt, nxt)])
            edges[start].append((token[0], start + 1, mid))
    return edges


class Vocab:
    def __init__(self, itos):
        if list(itos[:3]) != ["<pad>", "<bos>", "<unk>"]:
            raise ValueError("itos.json is not the 知字 vocabulary")
        self.itos = list(itos)
        self.stoi = {symbol: index for index, symbol in enumerate(self.itos)}


def prefix_ids(context, generated, vocab):
    """[bos] + tail of context and generated text. One new character still fits in 40."""
    preceding = list(context) + list(generated)
    keep = min(len(preceding), 38)
    tail = preceding[-keep:] if keep else []
    return [1] + [vocab.stoi.get(char, 2) for char in tail]


class ReadingIndex:
    def __init__(self, pairs, vocab):
        self.by_char = defaultdict(set)
        exact = defaultdict(set)
        canon = defaultdict(set)
        initials = defaultdict(set)
        self.dropped = 0
        for char, reading in pairs:
            if len(char) != 1 or " " in reading or not full_syllable(reading):
                continue
            cid = vocab.stoi.get(char)
            if cid is None or cid < 3:
                self.dropped += 1
                continue
            self.by_char[char].add(reading)
            exact[reading].add(cid)
            canon[canonical_syllable(reading)].add(cid)
            if reading[:2] in ("zh", "ch", "sh"):
                initials[reading[:2]].add(cid)
            if reading[:1] in INITIAL_LETTERS:
                initials[reading[:1]].add(cid)
        self._exact = exact
        self._canon = canon
        self._initials = initials
        self._cache = {}

    def ids(self, token):
        cached = self._cache.get(token)
        if cached is not None:
            return cached
        acc = set()
        if initial(token):
            acc |= self._initials.get(token, set())
        if full_syllable(token):
            acc |= self._exact.get(token, set())
            acc |= self._canon.get(canonical_syllable(token), set())
        arr = np.fromiter(sorted(acc), dtype=np.int32, count=len(acc))
        self._cache[token] = arr
        return arr

    def readings_of(self, char):
        return self.by_char.get(char, ())


class Lexicon:
    """Multi-character entries. Bonus is μ * ln(weight) when a suffix is completed."""

    def __init__(self, weights):
        self.weights = {}
        self.by_stem = defaultdict(list)
        for word, weight in weights.items():
            if len(word) < 2 or weight <= 0:
                continue
            if word in self.weights and self.weights[word] >= weight:
                continue
            self.weights[word] = weight
        for word, weight in self.weights.items():
            self.by_stem[word[:-1]].append((word[-1], weight))

    def bonus_vector(self, text, mu, nvocab, stoi):
        vec = np.zeros(nvocab, dtype=np.float64)
        if mu == 0.0:
            return vec
        suffix = text
        while True:
            for char, weight in self.by_stem.get(suffix, ()):
                cid = stoi.get(char)
                if cid is None or cid < 3:
                    continue
                vec[cid] += mu * math.log(weight)
            if not suffix:
                break
            suffix = suffix[1:]
        return vec


class Hyp:
    __slots__ = ("score", "text", "pos", "tokens", "node")

    def __init__(self, score, text, pos, tokens, node=None):
        self.score = float(score)
        self.text = text
        self.pos = pos
        self.tokens = tokens
        self.node = pos if node is None else node


def _rank_key(hyp):
    # Equal model scores: the same character is often legal for an initial and
    # for the full syllable. Keep the span that consumed more keys, or the beam
    # fills with prefixes that cannot finish the input.
    return (-hyp.score, -hyp.pos, hyp.text)


def expand(states, logp, edges, vocab, readings, lexicon, beam, mu):
    prelim = []
    cand_rows = 0
    nvocab = len(vocab.itos)
    for row, state in enumerate(states):
        spans = edges[state.node]
        bonus = None
        if mu != 0.0:
            bonus = lexicon.bonus_vector(state.text, mu, nvocab, vocab.stoi)
        seen = set()
        for token, end, node in spans:
            ids = readings.ids(token)
            if len(ids) == 0:
                continue
            seen.update(int(item) for item in ids)
            scores = np.asarray(logp[row], dtype=np.float64)[ids]
            if bonus is not None:
                scores = scores + bonus[ids]
            order = np.argsort(-scores, kind="stable")
            cutoff = float(scores[order[min(beam, len(order)) - 1]])
            for index in order:
                score = float(scores[index])
                if score < cutoff:
                    break
                cid = int(ids[index])
                char = vocab.itos[cid]
                prelim.append(Hyp(state.score + score, state.text + char, end, state.tokens + (token,), node))
        cand_rows += len(seen)
    prelim.sort(key=_rank_key)
    chosen = []
    seen_state = set()
    for hyp in prelim:
        key = (hyp.text, hyp.node)
        if key in seen_state:
            continue
        seen_state.add(key)
        chosen.append(hyp)
        if len(chosen) == beam:
            break
    return chosen, cand_rows


def _empty_stats():
    return {"forwards": 0, "run_rows": 0, "cand_rows": 0, "onnx_ms": 0.0, "coreml_ms": 0.0}


def _note_forward(stats, model_ms, nrows, cand):
    stats["forwards"] += 1
    stats["run_rows"] += nrows
    stats["cand_rows"] += cand
    stats["onnx_ms"] += model_ms
    if cand:
        stats["coreml_ms"] += math.ceil(cand / 8) * COREML_MS_PER_8


def beam_search(keys, context, model, vocab, readings, lexicon, beam=8, mu=0.0):
    stats = _empty_stats()
    if not keys:
        return [], stats
    edges = segment_edges(keys)
    beam_states = [Hyp(0.0, "", 0, ())]
    done = []
    for _ in range(len(keys) + 1):
        active = []
        for hyp in beam_states:
            if hyp.pos >= len(keys):
                done.append(hyp)
            elif hyp.pos < len(keys) and edges[hyp.node]:
                active.append(hyp)
        if not active:
            break
        prefixes = [prefix_ids(context, hyp.text, vocab) for hyp in active]
        started = time.perf_counter()
        logp = model.logprobs(prefixes)
        elapsed = (time.perf_counter() - started) * 1000.0
        nxt, cand = expand(active, logp, edges, vocab, readings, lexicon, beam, mu)
        _note_forward(stats, elapsed, len(active), cand)
        if not nxt:
            break
        beam_states = nxt
    best = {}
    for hyp in done:
        old = best.get(hyp.text)
        if old is None or hyp.score > old.score:
            best[hyp.text] = hyp
    return sorted(best.values(), key=_rank_key), stats


def greedy_search(keys, context, model, vocab, readings, lexicon, mu=0.0):
    """One hypothesis: at every step keep the single best extension."""
    stats = _empty_stats()
    if not keys:
        return [], stats
    edges = segment_edges(keys)
    state = Hyp(0.0, "", 0, ())
    for _ in range(len(keys) + 1):
        if state.pos >= len(keys):
            return [state], stats
        if not edges[state.node]:
            return [], stats
        started = time.perf_counter()
        logp = model.logprobs([prefix_ids(context, state.text, vocab)])
        elapsed = (time.perf_counter() - started) * 1000.0
        nxt, cand = expand([state], logp, edges, vocab, readings, lexicon, 1, mu)
        _note_forward(stats, elapsed, 1, cand)
        if not nxt:
            return [], stats
        state = nxt[0]
    return [], stats


def aligned(keys, text, tokens, readings):
    if len(text) != len(tokens):
        return False
    pos = 0
    for char, token in zip(text, tokens):
        ends = [end for tok, end in next_spans(keys, pos) if tok == token]
        if not ends:
            return False
        if not any(syllable_matches(token, reading) for reading in readings.readings_of(char)):
            return False
        pos = ends[0]
    return pos == len(keys)


def iter_dict_rows(path):
    body = False
    with path.open(encoding="utf-8") as handle:
        for line in handle:
            if not body:
                if line.strip() == "...":
                    body = True
                continue
            line = line.strip()
            if not line or line.startswith("#"):
                continue
            parts = line.split("\t")
            if len(parts) < 2:
                continue
            weight = int(parts[2]) if len(parts) >= 3 and parts[2].strip().isdigit() else 0
            yield parts[0], parts[1].strip().lower(), weight


def dictionary_paths(root=ROOT):
    paths = sorted((root / "native/resources/Rime").glob("*.dict.yaml"))
    paths += sorted((root / ".build-deps/rime-pinyin-simp").glob("*/pinyin_simp.dict.yaml"))
    return paths


def load_tables(vocab, root=ROOT):
    pairs = []
    weights = {}
    files = 0
    for path in dictionary_paths(root):
        files += 1
        for text, reading, weight in iter_dict_rows(path):
            if len(text) == 1 and " " not in reading:
                pairs.append((text, reading))
            elif len(text) >= 2 and weight > 0:
                if text not in weights or weight > weights[text]:
                    weights[text] = weight
    readings = ReadingIndex(pairs, vocab)
    return readings, Lexicon(weights), {
        "files": files,
        "reading_rows": len(pairs),
        "chars": len(readings.by_char),
        "dropped_oov": readings.dropped,
        "words": len(weights),
    }


class OnnxNext:
    def __init__(self, session, distribution=False):
        self.session = session
        self.distribution = distribution

    def logprobs(self, prefixes):
        rows = np.asarray(prefixes, dtype=np.int32)
        if self.distribution:
            return self.session.run(["logprobs"], {"tokens": rows})[0]
        logits = self.session.run(["/head/MatMul_output_0"], {"tokens": rows})[0]
        last = np.asarray(logits[:, -1, :], dtype=np.float64)
        last -= last.max(axis=1, keepdims=True)
        return last - np.log(np.exp(last).sum(axis=1, keepdims=True))


def open_session(threads, dist_path=None):
    import onnx
    import onnxruntime as ort
    from onnx import TensorProto, helper

    if dist_path is not None:
        options = ort.SessionOptions()
        options.intra_op_num_threads = threads
        options.inter_op_num_threads = 1
        session = ort.InferenceSession(str(dist_path), options, providers=["CPUExecutionProvider"])
        return session, str(dist_path)

    dyn = ROOT / "ime_context_model/release/zhizi-30m-v2-onnx/zhizi30m_v2.dyn.fp32.onnx"
    fixed = ROOT / "ime_context_model/release/zhizi-30m-v2-onnx/zhizi30m_v2.onnx"
    path = dyn if dyn.is_file() else fixed
    if not path.is_file():
        raise FileNotFoundError(path)
    model = onnx.load(str(path))
    model.graph.output.append(helper.make_tensor_value_info(
        "/head/MatMul_output_0", TensorProto.FLOAT, None))
    options = ort.SessionOptions()
    options.intra_op_num_threads = threads
    options.inter_op_num_threads = 1
    options.graph_optimization_level = ort.GraphOptimizationLevel.ORT_ENABLE_ALL
    session = ort.InferenceSession(model.SerializeToString(), options, providers=["CPUExecutionProvider"])
    return session, path.name


def check_matches_gather(session):
    """Last-position log-softmax versus appending the character and gathering."""
    rng = np.random.default_rng(SEED)
    worst = 0.0
    for length in (1, 4, 15, 39):
        prefix = np.zeros((1, length), np.int32)
        prefix[0, 0] = 1
        if length > 1:
            prefix[0, 1:] = rng.integers(3, 7000, size=length - 1)
        chars = rng.integers(3, 7000, size=6)
        rows = np.zeros((6, length + 1), np.int32)
        rows[:, :length] = prefix
        rows[:, length] = chars
        gathered = session.run(["logprobs"], {"tokens": rows})[0][:, length - 1]
        logits = session.run(["/head/MatMul_output_0"], {"tokens": prefix})[0][0, length - 1]
        shifted = logits - logits.max()
        dist = shifted - np.log(np.exp(shifted).sum())
        worst = max(worst, float(np.max(np.abs(dist[chars] - gathered))))
    return worst


def calibrate(session):
    """Median milliseconds for an 8-row forward, distribution and gathered output."""
    out = {}
    specs = (("dist", "/head/MatMul_output_0", 8, 16), ("dist40", "/head/MatMul_output_0", 8, 40),
             ("gather", "logprobs", 8, 17))
    for name, output, rows, length in specs:
        tokens = np.zeros((rows, length), np.int32)
        tokens[:, 0] = 1
        tokens[:, 1:] = 3
        session.run([output], {"tokens": tokens})
        times = []
        for _ in range(5):
            started = time.perf_counter()
            session.run([output], {"tokens": tokens})
            times.append((time.perf_counter() - started) * 1000.0)
        out[name] = sorted(times)[len(times) // 2]
    return out


def read_jsonl(path):
    rows = []
    with path.open(encoding="utf-8") as handle:
        for line in handle:
            line = line.strip()
            if line:
                rows.append(json.loads(line))
    return rows


def case_from(row, group):
    return {
        "group": group,
        "id": row["id"],
        "sub": row.get("sub") or "",
        "raw": row["raw"],
        "context": row.get("context") or "",
        "target": row["target"],
    }


def load_cases(root=ROOT, limit=0):
    tune = read_jsonl(root / "tmp/sent-abbrev/tune.jsonl")
    indep = read_jsonl(root / "tmp/rank-indep/independent.jsonl")
    probes = read_jsonl(Path(__file__).with_name("suffix_probes.jsonl"))
    sent = [case_from(row, "sent") for row in tune]
    buckets = {name: [] for name in ("full", "abbrev", "mixed")}
    for row in indep:
        if row.get("bucket") in buckets:
            buckets[row["bucket"]].append(case_from(row, row["bucket"]))
    sampled = []
    for name in ("full", "abbrev", "mixed"):
        picked = random.Random(SEED).sample(buckets[name], 500)
        picked.sort(key=lambda item: item["id"])
        sampled.extend(picked)
    suffix = [case_from(row, "suffix") for row in probes]
    if limit:
        sent = sent[:limit]
        trimmed = []
        for name in ("full", "abbrev", "mixed"):
            trimmed.extend([item for item in sampled if item["group"] == name][:limit])
        sampled = trimmed
    return sent + sampled + suffix


def rank_of(target, ranked):
    for index, hyp in enumerate(ranked):
        if hyp.text == target:
            return index
    return -1


def run_cases(cases, model, vocab, readings, lexicon, out_path):
    out_path.parent.mkdir(parents=True, exist_ok=True)
    started = time.perf_counter()
    with out_path.open("w", encoding="utf-8") as handle:
        for index, case in enumerate(cases, 1):
            for beam in BEAMS:
                for mu in MUS:
                    ranked, stats = beam_search(
                        case["raw"], case["context"], model, vocab, readings, lexicon, beam=beam, mu=mu)
                    rank = rank_of(case["target"], ranked)
                    top = [hyp.text for hyp in ranked[:5]]
                    record = {
                        "group": case["group"],
                        "id": case["id"],
                        "sub": case["sub"],
                        "n": len(case["target"]),
                        "raw": case["raw"],
                        "target": case["target"],
                        "b": beam,
                        "mu": mu,
                        "rank": rank,
                        "ok1": int(rank == 0),
                        "ok5": int(0 <= rank < 5),
                        "top": top,
                        "top_n": len(top[0]) if top else 0,
                        "forwards": stats["forwards"],
                        "run_rows": stats["run_rows"],
                        "cand_rows": stats["cand_rows"],
                        "onnx_ms": round(stats["onnx_ms"], 3),
                        "coreml_ms": round(stats["coreml_ms"], 3),
                    }
                    handle.write(json.dumps(record, ensure_ascii=False) + "\n")
            if index % 25 == 0 or index == len(cases):
                elapsed = time.perf_counter() - started
                print(f"{index}/{len(cases)} {elapsed:.0f}s", flush=True)
            handle.flush()


def percentile(values, fraction):
    if not values:
        return None
    ordered = sorted(values)
    if len(ordered) == 1:
        return ordered[0]
    position = (len(ordered) - 1) * fraction
    low = int(position)
    high = min(low + 1, len(ordered) - 1)
    return ordered[low] + (ordered[high] - ordered[low]) * (position - low)


def length_bucket(group, size):
    if group == "sent":
        if size <= 5:
            return "4-5"
        if size <= 7:
            return "6-7"
        return "8-10"
    if size <= 2:
        return "1-2"
    if size <= 4:
        return "3-4"
    return "5+"


def summarize(paths, out_path):
    rows = []
    for path in paths:
        rows.extend(read_jsonl(path))
    groups = {}
    timing = {}
    for row in rows:
        key = (row["group"], row["b"], row["mu"])
        bucket = groups.setdefault(key, {
            "n": 0, "ok1": 0, "ok5": 0, "len_match": 0, "subs": {}, "lengths": {},
        })
        bucket["n"] += 1
        bucket["ok1"] += row["ok1"]
        bucket["ok5"] += row["ok5"]
        if row["top"] and row["top_n"] == row["n"]:
            bucket["len_match"] += 1
        sub = row["sub"] or row["group"]
        sub_bucket = bucket["subs"].setdefault(sub, {"n": 0, "ok1": 0, "ok5": 0})
        sub_bucket["n"] += 1
        sub_bucket["ok1"] += row["ok1"]
        sub_bucket["ok5"] += row["ok5"]
        length = length_bucket(row["group"], row["n"])
        length_bucket_stats = bucket["lengths"].setdefault(length, {"n": 0, "ok1": 0, "ok5": 0})
        length_bucket_stats["n"] += 1
        length_bucket_stats["ok1"] += row["ok1"]
        length_bucket_stats["ok5"] += row["ok5"]
        slot = timing.setdefault(key, {"forwards": [], "cand_rows": [], "run_rows": [], "onnx_ms": [], "coreml_ms": []})
        for field in slot:
            slot[field].append(row[field])

    def pack(bucket):
        packed = {}
        for key, values in bucket.items():
            packed[key] = {
                "p50": percentile(values, 0.50),
                "p95": percentile(values, 0.95),
                "mean": sum(values) / len(values) if values else None,
            }
        return packed

    summary = {"groups": {}, "timing": {}}
    for (group, beam, mu), bucket in groups.items():
        summary["groups"].setdefault(group, {}).setdefault(str(beam), {})[str(mu)] = bucket
        summary["timing"].setdefault(group, {}).setdefault(str(beam), {})[str(mu)] = pack(timing[(group, beam, mu)])

    default = [row for row in rows if row["b"] == 8 and row["mu"] == 0.0 and row["group"] != "suffix"]
    success = [row for row in default if row["ok1"]]
    failure = [row for row in default if not row["ok5"]]
    summary["examples"] = {
        "success": pick_examples(success, 20),
        "failure": pick_examples(failure, 20),
    }
    summary["suffix"] = [row for row in rows if row["group"] == "suffix"]
    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(json.dumps(summary, ensure_ascii=False), encoding="utf-8")
    print(f"wrote {out_path} from {len(rows)} rows", flush=True)


def pick_examples(rows, count):
    """Round-robin groups, and within a group round-robin subs, in id order."""
    by_group = defaultdict(lambda: defaultdict(list))
    for row in rows:
        by_group[row["group"]][row.get("sub") or ""].append(row)
    queues = {}
    for group, by_sub in by_group.items():
        for items in by_sub.values():
            items.sort(key=lambda item: item["id"])
        queues[group] = (sorted(by_sub), by_sub, 0)
    picked = []
    order = ("sent", "full", "abbrev", "mixed")
    while len(picked) < count and any(
            any(queues[group][1].values()) for group in order if group in queues):
        for group in order:
            if group not in queues or len(picked) >= count:
                continue
            subs, by_sub, cursor = queues[group]
            for _ in range(len(subs)):
                sub = subs[cursor % len(subs)]
                cursor += 1
                bucket = by_sub[sub]
                if bucket:
                    picked.append(example_view(bucket.pop(0)))
                    break
            queues[group] = (subs, by_sub, cursor)
    return picked


def example_view(row):
    return {
        "group": row["group"],
        "sub": row["sub"],
        "id": row["id"],
        "raw": row["raw"],
        "target": row["target"],
        "top": row["top"],
        "rank": row["rank"],
    }


def main():
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest="cmd", required=True)
    run = sub.add_parser("run")
    run.add_argument("--shard", type=int, default=0)
    run.add_argument("--shards", type=int, default=1)
    run.add_argument("--threads", type=int, default=4)
    run.add_argument("--limit", type=int, default=0)
    run.add_argument("--out", type=Path, default=None)
    run.add_argument("--v3-dist", type=Path, default=None,
                     help="v3 full.onnx distribution export; itos.json must be beside it")
    summary = sub.add_parser("summarize")
    summary.add_argument("paths", nargs="+", type=Path)
    summary.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    if args.cmd == "summarize":
        summarize(args.paths, args.out)
        return
    print("loading cases", flush=True)
    all_cases = load_cases(limit=args.limit)
    cases = [case for index, case in enumerate(all_cases) if index % args.shards == args.shard]
    print(f"loading tables, shard cases {len(cases)}", flush=True)
    itos_path = (args.v3_dist.with_name("itos.json") if args.v3_dist is not None
                 else ROOT / "ime_context_model/release/zhizi-30m-v2/itos.json")
    itos = json.loads(itos_path.read_text(encoding="utf-8"))
    vocab = Vocab(itos)
    readings, lexicon, table_info = load_tables(vocab)
    print(json.dumps(table_info, ensure_ascii=False), flush=True)
    print("opening session", flush=True)
    session, model_name = open_session(args.threads, args.v3_dist)
    # The separate zhiju verifier compares the v3 distribution to its shipped
    # gather model. The original v2 startup checks remain unchanged.
    worst = check_matches_gather(session) if args.v3_dist is None else None
    if worst is not None and worst > 1e-4:
        raise RuntimeError(f"distribution does not match gather (max abs {worst})")
    calibration = calibrate(session) if args.v3_dist is None else {}
    out = args.out
    if out is None:
        out = ROOT / "docs/current/evidence/zhizi-generate-probe" / f"shard-{args.shard}.jsonl"
    if args.shard == 0:
        meta = {
            "model": model_name,
            "gather_max_abs": worst,
            "calibration_ms": calibration,
            "tables": table_info,
            "cases": len(all_cases),
        }
        meta_path = out.parent / "check.json"
        meta_path.parent.mkdir(parents=True, exist_ok=True)
        meta_path.write_text(json.dumps(meta, ensure_ascii=False, indent=2), encoding="utf-8")
        print(json.dumps(meta, ensure_ascii=False), flush=True)
    print(f"shard {args.shard}/{args.shards} cases {len(cases)}", flush=True)
    run_cases(cases, OnnxNext(session, distribution=args.v3_dist is not None), vocab, readings, lexicon, out)


if __name__ == "__main__":
    main()
