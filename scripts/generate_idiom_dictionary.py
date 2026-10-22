#!/usr/bin/env python3
"""Generate the pinned THUOCL dictionaries used by the offline Rime bundle."""

from __future__ import annotations

import argparse
import hashlib
import math
from dataclasses import dataclass
from pathlib import Path

from pypinyin import Style, lazy_pinyin


ROOT = Path(__file__).resolve().parents[1]
PINYIN_DICTIONARY = (
    ROOT
    / ".build-deps/rime-pinyin-simp/0c6861ef7420ee780270ca6d993d18d4101049d0/pinyin_simp.dict.yaml"
)
BASE_DEDUP_INPUTS = (
    PINYIN_DICTIONARY,
    ROOT / "native/resources/Rime/agent_daily.dict.yaml",
    ROOT / "native/resources/Rime/agent_english.dict.yaml",
    ROOT / "native/resources/Rime/agent_project_terms.dict.yaml",
    ROOT / "native/resources/Rime/agent_tech_terms.dict.yaml",
)
THUOCL_COMMIT = "a30ce79d895d01ab5132a5c74c29703ff7efb4cc"
PYPINYIN_VERSION = "0.55.0"
WEIGHT_CAP = 520
SKIP_SAMPLE_LIMIT = 12


@dataclass(frozen=True)
class TableSpec:
    key: str
    source: Path
    output: Path
    provenance: Path
    dictionary_name: str
    expected_source_rows: int
    expected_unique_rows: int
    expected_base_overlap: int
    expected_prior_overlap: int
    expected_skipped: int
    expected_output: int
    expected_skipped_ascii: int = 0
    expected_long_source: int | None = None
    expected_long_output: int | None = None


# SOURCE is table-configurable. The order is also part of the dedup contract:
# caijing gives way to six words already emitted by the higher-value IT table.
TABLES = (
    TableSpec(
        key="chengyu",
        source=ROOT / "third_party/THUOCL/data/THUOCL_chengyu.txt",
        output=ROOT / "native/resources/Rime/agent_idioms.dict.yaml",
        provenance=ROOT / "quality/corpus/thuocl-idiom-provenance.tsv",
        dictionary_name="agent_idioms",
        expected_source_rows=8519,
        expected_unique_rows=8519,
        expected_base_overlap=884,
        expected_prior_overlap=0,
        expected_skipped=0,
        expected_output=7635,
        expected_long_source=645,
        expected_long_output=645,
    ),
    TableSpec(
        key="it",
        source=ROOT / "third_party/THUOCL/data/THUOCL_IT.txt",
        output=ROOT / "native/resources/Rime/agent_thuocl_it.dict.yaml",
        provenance=ROOT / "quality/corpus/thuocl-it-provenance.tsv",
        dictionary_name="agent_thuocl_it",
        expected_source_rows=16000,
        expected_unique_rows=15999,
        expected_base_overlap=640,
        expected_prior_overlap=2,
        expected_skipped=4,
        expected_skipped_ascii=3141,
        expected_output=12212,
    ),
    TableSpec(
        key="caijing",
        source=ROOT / "third_party/THUOCL/data/THUOCL_caijing.txt",
        output=ROOT / "native/resources/Rime/agent_thuocl_caijing.dict.yaml",
        provenance=ROOT / "quality/corpus/thuocl-caijing-provenance.tsv",
        dictionary_name="agent_thuocl_caijing",
        expected_source_rows=3830,
        expected_unique_rows=3830,
        expected_base_overlap=1355,
        expected_prior_overlap=11,
        expected_skipped=0,
        expected_output=2464,
    ),
)


def data_rows(path: Path):
    for line in path.read_text(encoding="utf-8").splitlines():
        if not line or line.startswith(("#", "---", "...")) or ":" in line:
            continue
        columns = line.split("\t")
        if len(columns) >= 2:
            yield [column.strip() for column in columns]


def read_source_rows(path: Path) -> list[tuple[str, int, int]]:
    """Parse THUOCL rows, accepting BOM/CR/spacing variants and rejecting bad DF."""
    contents = path.read_text(encoding="utf-8-sig")
    rows: list[tuple[str, int, int]] = []
    for line_number, line in enumerate(contents.splitlines(), 1):
        if not line.strip():
            continue
        columns = line.strip().split()
        if len(columns) != 2:
            raise RuntimeError(
                f"invalid THUOCL row {path.name}:{line_number}: expected word and DF, got {line!r}"
            )
        text, raw_df = columns
        if not raw_df.isdigit():
            raise RuntimeError(
                f"invalid THUOCL DF {path.name}:{line_number}: {raw_df!r} in {line!r}"
            )
        rows.append((text, int(raw_df), line_number))
    return rows


def schema_syllables() -> set[str]:
    syllables: set[str] = set()
    for _, reading, *_ in data_rows(PINYIN_DICTIONARY):
        syllables.update(reading.lower().split())
    if len(syllables) != 415:
        raise RuntimeError(f"pinned schema syllable inventory changed: {len(syllables)}")
    return syllables


def input_texts(paths: tuple[Path, ...]) -> set[str]:
    return {columns[0] for path in paths for columns in data_rows(path)}


def dictionary_reading(text: str) -> list[str]:
    syllables = lazy_pinyin(text, style=Style.NORMAL, strict=True, errors="default")
    return [{"lve": "lue", "nve": "nue"}.get(value, value) for value in syllables]


def weight_for(df: int) -> int:
    # Chunk 10 froze the calibrated output band at 104..240. The independent
    # 520 ceiling remains a fail-safe for any future formula revision.
    return min(WEIGHT_CAP, max(104, min(240, round(math.exp(3.68) * (df**0.181)))))


def generate_table(
    spec: TableSpec,
    valid_syllables: set[str],
    base_existing: set[str],
    prior_generated: set[str],
) -> tuple[str, str, dict[str, object], set[str]]:
    source_rows = read_source_rows(spec.source)
    seen: set[str] = set()
    unique_rows: list[tuple[str, int, int]] = []
    duplicates: list[tuple[str, int]] = []
    for text, df, line_number in source_rows:
        if text in seen:
            duplicates.append((text, line_number))
            continue
        seen.add(text)
        unique_rows.append((text, df, line_number))

    output_rows: list[str] = []
    provenance_rows: list[str] = []
    skipped: list[tuple[str, list[str], int]] = []
    skipped_ascii: list[tuple[str, list[str], int]] = []
    base_overlap = 0
    prior_overlap = 0
    long_source = sum(len(text) != 4 for text, _, _ in unique_rows)
    long_output = 0
    output_texts: set[str] = set()

    for text, df, line_number in unique_rows:
        if text in base_existing:
            base_overlap += 1
            continue
        if text in prior_generated:
            prior_overlap += 1
            continue
        ascii_characters = [character for character in text if character.isascii()]
        if ascii_characters:
            # A pure-pinyin schema must not ship entries whose *text* contains
            # ASCII: readings that happen to be valid syllables (`pa`, `sun公司`)
            # would leak Latin candidates into the pinyin candidate flow.
            skipped_ascii.append((text, ascii_characters, line_number))
            continue
        syllables = dictionary_reading(text)
        bad = [syllable for syllable in syllables if syllable not in valid_syllables]
        if bad:
            skipped.append((text, bad, line_number))
            continue
        reading = " ".join(syllables)
        weight = weight_for(df)
        output_rows.append(f"{text}\t{reading}\t{weight}")
        provenance_rows.append(
            f"{text}\t{reading}\t{weight}\t{df}\t{spec.source.name}:{line_number}"
            f"\t{THUOCL_COMMIT}\tMIT\tpypinyin-{PYPINYIN_VERSION}"
        )
        output_texts.add(text)
        long_output += len(text) != 4

    actual = {
        "source": len(source_rows),
        "unique": len(unique_rows),
        "duplicates": len(duplicates),
        "duplicate_samples": duplicates[:SKIP_SAMPLE_LIMIT],
        "base_overlap": base_overlap,
        "prior_overlap": prior_overlap,
        "skipped": len(skipped),
        "skip_samples": skipped[:SKIP_SAMPLE_LIMIT],
        "skipped_ascii": len(skipped_ascii),
        "skipped_ascii_samples": skipped_ascii[:SKIP_SAMPLE_LIMIT],
        "output": len(output_rows),
        "long_source": long_source,
        "long_output": long_output,
    }
    expected = {
        "source": spec.expected_source_rows,
        "unique": spec.expected_unique_rows,
        "base_overlap": spec.expected_base_overlap,
        "prior_overlap": spec.expected_prior_overlap,
        "skipped": spec.expected_skipped,
        "skipped_ascii": spec.expected_skipped_ascii,
        "output": spec.expected_output,
    }
    for key, value in expected.items():
        if actual[key] != value:
            raise RuntimeError(
                f"{spec.key} generation contract changed: {key}={actual[key]} expected={value}"
            )
    if spec.expected_long_source is not None and long_source != spec.expected_long_source:
        raise RuntimeError(
            f"{spec.key} long-source contract changed: {long_source} expected={spec.expected_long_source}"
        )
    if spec.expected_long_output is not None and long_output != spec.expected_long_output:
        raise RuntimeError(
            f"{spec.key} long-output contract changed: {long_output} expected={spec.expected_long_output}"
        )
    if len(output_rows) != len(output_texts):
        raise RuntimeError(f"{spec.key} generated duplicate text rows")

    min_weight = min((int(row.rsplit("\t", 1)[1]) for row in output_rows), default=0)
    max_weight = max((int(row.rsplit("\t", 1)[1]) for row in output_rows), default=0)
    actual["min_weight"] = min_weight
    actual["max_weight"] = max_weight
    header = (
        f"# Generated from pinned {spec.source.name} (MIT); do not edit by hand.\n"
        f"# Source commit: {THUOCL_COMMIT}; readings: pypinyin {PYPINYIN_VERSION}.\n"
        "# Weight = round(exp(3.68) * DF^0.181), calibrated to 104..240; hard cap 520.\n"
        "---\n"
        f"name: {spec.dictionary_name}\n"
        "version: \"0.4.0\"\n"
        "sort: by_weight\n"
        "...\n\n"
    )
    dictionary = header + "\n".join(output_rows) + "\n"
    provenance = (
        "text\treading\tweight\tdf\tsource\tcommit\tlicense\treading_tool\n"
        + "\n".join(provenance_rows)
        + "\n"
    )
    return dictionary, provenance, actual, output_texts


def update_or_check(path: Path, generated: str, check: bool) -> None:
    if check:
        if path.read_text(encoding="utf-8") != generated:
            raise RuntimeError(f"{path.relative_to(ROOT)} is stale; regenerate it")
    else:
        path.write_text(generated, encoding="utf-8")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    parser.add_argument(
        "--table",
        choices=[spec.key for spec in TABLES] + ["all"],
        default="all",
        help="generate one configured SOURCE or the complete ordered set",
    )
    args = parser.parse_args()

    valid_syllables = schema_syllables()
    base_existing = input_texts(BASE_DEDUP_INPUTS)
    prior_generated: set[str] = set()
    selected = {spec.key for spec in TABLES} if args.table == "all" else {args.table}
    for spec in TABLES:
        dictionary, provenance, stats, output_texts = generate_table(
            spec, valid_syllables, base_existing, prior_generated
        )
        if spec.key in selected:
            update_or_check(spec.output, dictionary, args.check)
            update_or_check(spec.provenance, provenance, args.check)
        prior_generated.update(output_texts)
        if spec.key in selected:
            skip_samples = ",".join(
                f"{text}@{line}:{'/'.join(bad)}"
                for text, bad, line in stats["skip_samples"]
            ) or "none"
            print(
                f"thuocl_{spec.key}=PASS source={stats['source']} unique={stats['unique']}"
                f" duplicates={stats['duplicates']} base_overlap={stats['base_overlap']}"
                f" prior_overlap={stats['prior_overlap']} skipped={stats['skipped']}"
                f" output={stats['output']} weights={stats['min_weight']}-{stats['max_weight']}"
                f" skip_samples={skip_samples}"
                f" dictionary_sha256={hashlib.sha256(dictionary.encode()).hexdigest()}"
            )


if __name__ == "__main__":
    main()
