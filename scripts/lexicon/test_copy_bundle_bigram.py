import hashlib
import importlib.util
from pathlib import Path

spec = importlib.util.spec_from_file_location("copy_bundle_bigram", Path(__file__).with_name("copy_bundle_bigram.py"))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


def write_source(tmp_path):
    source = tmp_path / "src"
    (source / "10mb").mkdir(parents=True)
    lines = []
    for name, body in (("bigrams.tsv", b"a\tb\t-1\n"), ("unigrams.tsv", b"a\t-2\t0\n")):
        (source / "10mb" / name).write_bytes(body)
        lines.append(f"{hashlib.sha256(body).hexdigest()}  10mb/{name}")
    manifest = tmp_path / "SHA256SUMS"
    manifest.write_text("\n".join(lines) + "\n")
    return source, manifest


def test_copies_pinned_files(tmp_path):
    source, manifest = write_source(tmp_path)
    result = module.copy_table(tmp_path / "out", source, manifest)
    assert result.startswith("word-bigram available")
    assert (tmp_path / "out/10mb/bigrams.tsv").read_bytes() == b"a\tb\t-1\n"


def test_missing_or_changed_table_is_unavailable_and_copies_nothing(tmp_path):
    source, manifest = write_source(tmp_path)
    (source / "10mb/unigrams.tsv").write_bytes(b"changed\n")
    assert "checksum mismatch" in module.copy_table(tmp_path / "out", source, manifest)
    assert not (tmp_path / "out").exists()
    assert "missing" in module.copy_table(tmp_path / "out2", tmp_path / "nowhere", manifest)


def test_rejects_paths_outside_the_tier(tmp_path):
    source, manifest = write_source(tmp_path)
    manifest.write_text("00  ../evil\n")
    try:
        module.copy_table(tmp_path / "out", source, manifest)
    except ValueError:
        return
    raise AssertionError("expected ValueError")
