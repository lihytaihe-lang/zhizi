"""Leakage-safe train/valid/test splitting for the zhiju context-model dataset (stdlib only)."""

import argparse
import hashlib
import json
import math
import os
import sys
import unicodedata

OUTPUT_FILES = ("train.jsonl", "valid.jsonl", "test.jsonl", "manifest.json")
JSONL_FILES = (("train.jsonl", "train"), ("valid.jsonl", "valid"), ("test.jsonl", "test"))
SPLIT_NAMES = ("train", "valid", "test")
DEFAULT_SEED = "20261115"
DEFAULT_VALID_FRACTION = 0.1
DEFAULT_TEST_FRACTION = 0.1
_NEAR_MIN_LENGTH = 6
_RAW_ALPHABET = frozenset("abcdefghijklmnopqrstuvwxyz'")
_REQUIRED_FIELDS = ("id", "context", "target", "raw", "source", "license")
NORMALIZATION_DESCRIPTION = (
    "Unicode NFKC normalization plus casefold, then removal of Unicode whitespace "
    "and punctuation (categories P*, isspace); applied for grouping only, stored data unchanged"
)
NEAR_GROUPING_DESCRIPTION = (
    "rows unioned when normalized targets are equal, when explicit nonempty group strings "
    "are equal, or when both normalized targets have length >= 6 and Unicode edit distance "
    "<= 1 (insertion/deletion/substitution); near matches found via exact keys and "
    "delete-one-character signature buckets with a final edit-distance check, never "
    "all-pairs comparisons; unions are transitive via union-find"
)


def normalize_text(text):
    decomposed = unicodedata.normalize("NFKC", text).casefold()
    return "".join(
        ch
        for ch in decomposed
        if not ch.isspace() and not unicodedata.category(ch).startswith("P")
    )


class _UnionFind:
    def __init__(self):
        self._parent = {}

    def add(self, item):
        self._parent.setdefault(item, item)

    def find(self, item):
        self.add(item)
        root = item
        while self._parent[root] != root:
            root = self._parent[root]
        while self._parent[item] != root:
            self._parent[item], item = root, self._parent[item]
        return root

    def union(self, a, b):
        root_a = self.find(a)
        root_b = self.find(b)
        if root_a != root_b:
            self._parent[root_b] = root_a


def _edit_distance_at_most_one(a, b):
    len_a, len_b = len(a), len(b)
    if abs(len_a - len_b) > 1:
        return False
    if len_a > len_b:
        a, b, len_a, len_b = b, a, len_b, len_a
    edits = 0
    i = j = 0
    while i < len_a and j < len_b:
        if a[i] == b[j]:
            i += 1
            j += 1
            continue
        edits += 1
        if edits > 1:
            return False
        if len_a == len_b:
            i += 1
        j += 1
    if j < len_b:
        edits += 1
    return edits <= 1


def _require_nonempty_string(index, field, value):
    if not isinstance(value, str):
        raise ValueError("row %d: field %r must be a string" % (index, field))
    if not value:
        raise ValueError("row %d: field %r must be nonempty" % (index, field))
    return value


def _validate_row(index, row, seen_ids):
    if not isinstance(row, dict):
        raise ValueError("row %d must be a JSON object" % index)
    if "splitGroup" in row:
        raise ValueError("row %d: preexisting 'splitGroup' field is not allowed" % index)
    for field in _REQUIRED_FIELDS:
        if field not in row:
            raise ValueError("row %d: missing required field %r" % (index, field))
    row_id = _require_nonempty_string(index, "id", row["id"])
    if row_id in seen_ids:
        raise ValueError("row %d: duplicate id %r" % (index, row_id))
    seen_ids.add(row_id)
    if not isinstance(row["context"], str):
        raise ValueError("row %d: field 'context' must be a string (may be empty)" % index)
    target = _require_nonempty_string(index, "target", row["target"])
    raw = _require_nonempty_string(index, "raw", row["raw"])
    _require_nonempty_string(index, "source", row["source"])
    _require_nonempty_string(index, "license", row["license"])
    if "group" in row and not isinstance(row["group"], str):
        raise ValueError("row %d: optional field 'group' must be a string" % index)
    if "negatives" in row:
        negatives = row["negatives"]
        if not isinstance(negatives, list):
            raise ValueError("row %d: optional field 'negatives' must be a list" % index)
        seen_negatives = set()
        for item in negatives:
            if not isinstance(item, str) or not item:
                raise ValueError("row %d: negatives must be nonempty strings" % index)
            if item in seen_negatives:
                raise ValueError("row %d: negatives must be unique (duplicate %r)" % (index, item))
            if item == target:
                raise ValueError("row %d: negative %r must not equal target" % (index, item))
            seen_negatives.add(item)
    if any(ch not in _RAW_ALPHABET for ch in raw):
        raise ValueError(
            "row %d: 'raw' must contain only lowercase ASCII letters and apostrophes" % index
        )
    if all(ch == "'" for ch in raw):
        raise ValueError("row %d: 'raw' must not consist only of apostrophes" % index)
    normalized_target = normalize_text(target)
    if not normalized_target:
        raise ValueError("row %d: 'target' must have a nonempty normalized form" % index)
    return {"id": row_id, "row": row, "norm_target": normalized_target}


def _validate_seed(seed):
    if not isinstance(seed, str) or not seed:
        raise ValueError("seed must be a nonempty string")


def _validate_fraction(name, value):
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise ValueError("%s must be a number" % name)
    number = float(value)
    if not math.isfinite(number):
        raise ValueError("%s must be finite" % name)
    if number <= 0:
        raise ValueError("%s must be positive" % name)
    return number


def _validate_fractions(valid_fraction, test_fraction):
    valid = _validate_fraction("valid_fraction", valid_fraction)
    test = _validate_fraction("test_fraction", test_fraction)
    if valid + test >= 1:
        raise ValueError("valid_fraction + test_fraction must be < 1")
    return valid, test


def _hash_unit(seed, identity):
    digest = hashlib.sha256((seed + identity).encode("utf-8")).digest()
    return int.from_bytes(digest[:16], "big") / float(1 << 128)


def _split_for(unit, test_fraction, valid_fraction):
    if unit < test_fraction:
        return "test"
    if unit < test_fraction + valid_fraction:
        return "valid"
    return "train"


def prepare(rows, seed=DEFAULT_SEED, valid_fraction=DEFAULT_VALID_FRACTION, test_fraction=DEFAULT_TEST_FRACTION):
    """Validate rows, group leakage-connected rows, and split them.

    Returns (splits, manifest) where splits maps "train"/"valid"/"test" to lists of
    rows (each with a 'splitGroup' field added, sorted by id). Performs no filesystem IO.
    """
    _validate_seed(seed)
    valid_fraction, test_fraction = _validate_fractions(valid_fraction, test_fraction)
    checked = []
    seen_ids = set()
    for index, row in enumerate(rows):
        checked.append(_validate_row(index, row, seen_ids))
    if not checked:
        raise ValueError("input must contain at least one row")
    forest = _UnionFind()
    for item in checked:
        forest.add(item["id"])
    exact_buckets = {}
    for item in checked:
        exact_buckets.setdefault(item["norm_target"], []).append(item["id"])
    for ids in exact_buckets.values():
        for other_id in ids[1:]:
            forest.union(ids[0], other_id)
    group_buckets = {}
    for item in checked:
        group = item["row"].get("group")
        if group:
            group_buckets.setdefault(group, []).append(item["id"])
    for ids in group_buckets.values():
        for other_id in ids[1:]:
            forest.union(ids[0], other_id)
    signatures = {}
    canonical_by_norm = {}
    for norm_target, ids in exact_buckets.items():
        if len(norm_target) < _NEAR_MIN_LENGTH:
            continue
        canonical_by_norm[norm_target] = min(ids)
        signatures.setdefault(norm_target, []).append(norm_target)
        for pos in range(len(norm_target)):
            deleted = norm_target[:pos] + norm_target[pos + 1:]
            signatures.setdefault(deleted, []).append(norm_target)
    for members in signatures.values():
        if len(members) < 2:
            continue
        ordered = sorted(set(members))
        for x in range(len(ordered)):
            for y in range(x + 1, len(ordered)):
                left, right = ordered[x], ordered[y]
                left_id = canonical_by_norm[left]
                right_id = canonical_by_norm[right]
                if forest.find(left_id) == forest.find(right_id):
                    continue
                if _edit_distance_at_most_one(left, right):
                    forest.union(left_id, right_id)
    components = {}
    for item in checked:
        components.setdefault(forest.find(item["id"]), []).append(item["id"])
    identity_by_root = {}
    for root, member_ids in components.items():
        joined = json.dumps(sorted(member_ids), ensure_ascii=False, separators=(",", ":"))
        identity_by_root[root] = hashlib.sha256(joined.encode("utf-8")).hexdigest()
    component_split = {}
    for root, identity in identity_by_root.items():
        unit = _hash_unit(seed, identity)
        component_split[identity] = _split_for(unit, test_fraction, valid_fraction)
    splits = {name: [] for name in SPLIT_NAMES}
    for item in checked:
        identity = identity_by_root[forest.find(item["id"])]
        out_row = dict(item["row"])
        out_row["splitGroup"] = identity
        splits[component_split[identity]].append(out_row)
    for name in SPLIT_NAMES:
        splits[name].sort(key=lambda entry: entry["id"])
    by_source = {}
    by_license = {}
    for item in checked:
        source = item["row"]["source"]
        license_name = item["row"]["license"]
        by_source[source] = by_source.get(source, 0) + 1
        by_license[license_name] = by_license.get(license_name, 0) + 1
    manifest = {
        "input_rows": len(checked),
        "groups": len(components),
        "splits": {
            name: {
                "rows": len(splits[name]),
                "groups": len({entry["splitGroup"] for entry in splits[name]}),
            }
            for name in SPLIT_NAMES
        },
        "by_source": {key: by_source[key] for key in sorted(by_source)},
        "by_license": {key: by_license[key] for key in sorted(by_license)},
        "normalization": NORMALIZATION_DESCRIPTION,
        "near_grouping": NEAR_GROUPING_DESCRIPTION,
        "seed": seed,
        "valid_fraction": valid_fraction,
        "test_fraction": test_fraction,
    }
    return splits, manifest


def _parse_jsonl(text):
    rows = []
    for line_number, line in enumerate(text.splitlines(), start=1):
        if not line.strip():
            continue
        try:
            rows.append(json.loads(line))
        except json.JSONDecodeError as error:
            raise ValueError("line %d: invalid JSON: %s" % (line_number, error.msg)) from None
    return rows


def _serialize_split(rows):
    return "".join(json.dumps(row, ensure_ascii=False) + "\n" for row in rows)


def _write_bytes(path, data):
    with open(path, "wb") as handle:
        handle.write(data)


def main(argv=None):
    parser = argparse.ArgumentParser(
        description="Split a JSONL dataset into leakage-safe train/valid/test files."
    )
    parser.add_argument("--input", required=True, help="input JSONL file")
    parser.add_argument("--out", required=True, help="output directory")
    parser.add_argument("--seed", default=DEFAULT_SEED, help="split seed string")
    parser.add_argument("--valid-fraction", type=float, default=DEFAULT_VALID_FRACTION)
    parser.add_argument("--test-fraction", type=float, default=DEFAULT_TEST_FRACTION)
    args = parser.parse_args(argv)
    out_dir = args.out
    if os.path.exists(out_dir):
        if not os.path.isdir(out_dir):
            raise ValueError("--out must be a directory: %s" % out_dir)
        for name in OUTPUT_FILES:
            path = os.path.join(out_dir, name)
            if os.path.exists(path):
                raise ValueError("refusing to overwrite existing output file: %s" % path)
    with open(args.input, "rb") as handle:
        raw_input = handle.read()
    input_sha256 = hashlib.sha256(raw_input).hexdigest()
    try:
        text = raw_input.decode("utf-8")
    except UnicodeDecodeError as error:
        raise ValueError("input is not valid UTF-8: %s" % error) from None
    splits, manifest = prepare(
        _parse_jsonl(text), args.seed, args.valid_fraction, args.test_fraction
    )
    os.makedirs(out_dir, exist_ok=True)
    output_sha256 = {}
    for file_name, split_name in JSONL_FILES:
        data = _serialize_split(splits[split_name]).encode("utf-8")
        _write_bytes(os.path.join(out_dir, file_name), data)
        output_sha256[file_name] = hashlib.sha256(data).hexdigest()
    manifest = dict(manifest)
    manifest["input_sha256"] = input_sha256
    manifest["output_sha256"] = output_sha256
    manifest_data = (
        json.dumps(manifest, ensure_ascii=False, sort_keys=True, indent=2) + "\n"
    ).encode("utf-8")
    _write_bytes(os.path.join(out_dir, "manifest.json"), manifest_data)
    return manifest


if __name__ == "__main__":
    try:
        main()
    except ValueError as error:
        print("error: %s" % error, file=sys.stderr)
        sys.exit(2)
