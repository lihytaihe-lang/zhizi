#!/usr/bin/env python3
"""Generate the project-authored modern Chinese dictionary for DICT-D2."""

from __future__ import annotations

import argparse
import hashlib
from pathlib import Path

from pypinyin import Style, lazy_pinyin


ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "quality/corpus/agent-modern-source.tsv"
OUTPUT = ROOT / "native/resources/Rime/agent_modern.dict.yaml"
PROVENANCE = ROOT / "quality/corpus/agent-modern-provenance.tsv"
PINYIN_DICTIONARY = (
    ROOT
    / ".build-deps/rime-pinyin-simp/0c6861ef7420ee780270ca6d993d18d4101049d0/pinyin_simp.dict.yaml"
)
DEDUP_INPUTS = (
    PINYIN_DICTIONARY,
    ROOT / "native/resources/Rime/agent_daily.dict.yaml",
    ROOT / "native/resources/Rime/agent_idioms.dict.yaml",
    ROOT / "native/resources/Rime/agent_thuocl_it.dict.yaml",
    ROOT / "native/resources/Rime/agent_thuocl_caijing.dict.yaml",
    ROOT / "native/resources/Rime/agent_english.dict.yaml",
    ROOT / "native/resources/Rime/agent_project_terms.dict.yaml",
    ROOT / "native/resources/Rime/agent_tech_terms.dict.yaml",
)
PYPINYIN_VERSION = "0.55.0"
WEIGHTS = {
    "owner-project": 520,
    "owner-gap": 520,
    "internet": 520,
    "ai-tech": 520,
    "modern": 470,
    "work": 460,
}
REQUIRED_AGGREGATE = {
    "微信": "wei xin",
    "微博": "wei bo",
    "抖音": "dou yin",
    "小红书": "xiao hong shu",
    "拼多多": "pin duo duo",
    "特斯拉": "te si la",
    "大数据": "da shu ju",
    "区块链": "qu kuai lian",
    "比特币": "bi te bi",
    "私有云": "si you yun",
    "埋点": "mai dian",
    "蒸汽机": "zheng qi ji",
    "瓦特": "wa te",
    "举例子": "ju li zi",
    "熟读": "shu du",
    "甄维斯": "zhen wei si",
    "顺手知字": "shun shou zhi zi",
}
REVIEWED_POLYPHONIC = {
    "收藏": "shou cang",
    "收藏量": "shou cang liang",
    "曝光量": "bao guang liang",
    "播放量": "bo fang liang",
    "增长点": "zeng zhang dian",
    "增长率": "zeng zhang lv",
    "增长曲线": "zeng zhang qu xian",
    "熟读": "shu du",
}
EXPECTED_SOURCE_ROWS = 288
EXPECTED_OVERLAP = 136
EXPECTED_SKIPPED = 1
EXPECTED_OUTPUT = 151


def data_rows(path: Path):
    for line in path.read_text(encoding="utf-8").splitlines():
        if not line or line.startswith(("#", "---", "...")) or ":" in line:
            continue
        columns = line.split("\t")
        if len(columns) >= 2:
            yield [column.strip() for column in columns]


def schema_syllables() -> set[str]:
    syllables: set[str] = set()
    for _, reading, *_ in data_rows(PINYIN_DICTIONARY):
        syllables.update(reading.lower().split())
    if len(syllables) != 415:
        raise RuntimeError(f"pinned schema syllable inventory changed: {len(syllables)}")
    return syllables


def reading_for(text: str) -> list[str]:
    values = lazy_pinyin(text, style=Style.NORMAL, strict=True, errors="default")
    return [{"lve": "lue", "nve": "nue"}.get(value, value) for value in values]


def read_source() -> list[tuple[str, str, int]]:
    rows: list[tuple[str, str, int]] = []
    seen: set[str] = set()
    for line_number, line in enumerate(SOURCE.read_text(encoding="utf-8").splitlines(), 1):
        if not line or line.startswith("#"):
            continue
        columns = [column.strip() for column in line.split("\t")]
        if len(columns) != 2 or not all(columns):
            raise RuntimeError(f"invalid modern source row {line_number}: {line!r}")
        text, category = columns
        if category not in WEIGHTS:
            raise RuntimeError(f"invalid modern category at row {line_number}: {category!r}")
        if text in seen:
            raise RuntimeError(f"duplicate modern source text at row {line_number}: {text}")
        seen.add(text)
        rows.append((text, category, line_number))
    if len(rows) > 500:
        raise RuntimeError(f"modern source exceeds 500-row cap: {len(rows)}")
    return rows


def generated_assets() -> tuple[str, str, dict[str, object]]:
    valid_syllables = schema_syllables()
    existing = {columns[0] for path in DEDUP_INPUTS for columns in data_rows(path)}
    source_rows = read_source()
    output_rows: list[str] = []
    provenance_rows: list[str] = []
    generated_readings: dict[str, str] = {}
    overlap: list[str] = []
    skipped: list[tuple[str, list[str], int]] = []
    for text, category, line_number in source_rows:
        if text in existing:
            overlap.append(text)
            continue
        syllables = reading_for(text)
        bad = [syllable for syllable in syllables if syllable not in valid_syllables]
        if bad:
            skipped.append((text, bad, line_number))
            continue
        reading = " ".join(syllables)
        weight = WEIGHTS[category]
        output_rows.append(f"{text}\t{reading}\t{weight}")
        provenance_rows.append(
            f"{text}\t{reading}\t{weight}\t{category}\tDICT-EXPANSION D2"
            "\tproject-authored\thuman-curated modern daily written-language term"
        )
        generated_readings[text] = reading

    for text, expected in REVIEWED_POLYPHONIC.items():
        if text in generated_readings and generated_readings[text] != expected:
            raise RuntimeError(
                f"reviewed polyphonic reading changed for {text}: {generated_readings[text]} expected={expected}"
            )
    aggregate_readings = {
        columns[0]: columns[1]
        for path in DEDUP_INPUTS
        for columns in data_rows(path)
        if len(columns) >= 2
    }
    aggregate_readings.update(generated_readings)
    for text, expected in REQUIRED_AGGREGATE.items():
        if aggregate_readings.get(text) != expected:
            raise RuntimeError(
                f"required modern coverage missing or reading changed for {text}: "
                f"{aggregate_readings.get(text)!r} expected={expected!r}"
            )
    if len(output_rows) != len({row.split("\t", 1)[0] for row in output_rows}):
        raise RuntimeError("modern generated output contains duplicate text")

    stats: dict[str, object] = {
        "source": len(source_rows),
        "overlap": len(overlap),
        "overlap_samples": overlap[:12],
        "skipped": len(skipped),
        "skip_samples": skipped[:12],
        "output": len(output_rows),
    }
    for key, expected in (
        ("source", EXPECTED_SOURCE_ROWS),
        ("overlap", EXPECTED_OVERLAP),
        ("skipped", EXPECTED_SKIPPED),
        ("output", EXPECTED_OUTPUT),
    ):
        if expected is not None and stats[key] != expected:
            raise RuntimeError(
                f"modern generation contract changed: {key}={stats[key]} expected={expected}"
            )

    dictionary = (
        "# Generated from the project-authored DICT-D2 curation; do not edit by hand.\n"
        f"# Readings: pypinyin {PYPINYIN_VERSION}; weights are category-based and capped at 520.\n"
        "---\nname: agent_modern\nversion: \"0.4.0\"\nsort: by_weight\n...\n\n"
        + "\n".join(output_rows)
        + "\n"
    )
    provenance = (
        "text\treading\tweight\tcategory\tsource\tlicense\treason\n"
        + "\n".join(provenance_rows)
        + "\n"
    )
    return dictionary, provenance, stats


def update_or_check(path: Path, generated: str, check: bool) -> None:
    if check:
        if path.read_text(encoding="utf-8") != generated:
            raise RuntimeError(f"{path.relative_to(ROOT)} is stale; regenerate it")
    else:
        path.write_text(generated, encoding="utf-8")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    dictionary, provenance, stats = generated_assets()
    update_or_check(OUTPUT, dictionary, args.check)
    update_or_check(PROVENANCE, provenance, args.check)
    skip_samples = ",".join(
        f"{text}@{line}:{'/'.join(bad)}" for text, bad, line in stats["skip_samples"]
    ) or "none"
    overlap_samples = ",".join(stats["overlap_samples"]) or "none"
    print(
        f"modern_dictionary=PASS source={stats['source']} overlap={stats['overlap']}"
        f" skipped={stats['skipped']} output={stats['output']} weights=460-520"
        f" overlap_samples={overlap_samples} skip_samples={skip_samples}"
        f" dictionary_sha256={hashlib.sha256(dictionary.encode()).hexdigest()}"
    )


if __name__ == "__main__":
    main()
