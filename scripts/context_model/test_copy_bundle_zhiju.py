import hashlib
import importlib.util
from pathlib import Path

spec = importlib.util.spec_from_file_location("copy_bundle_zhiju", Path(__file__).with_name("copy_bundle_zhiju.py"))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


def write_source(tmp_path):
    source = tmp_path / "src"
    files = {"itos.json": b'["<pad>","<bos>","<unk>"]', "m.mlmodelc/weights/weight.bin": b"w", "m.mlmodelc/model.mil": b"mil"}
    lines = []
    for name, body in files.items():
        (source / name).parent.mkdir(parents=True, exist_ok=True)
        (source / name).write_bytes(body)
        lines.append(f"{hashlib.sha256(body).hexdigest()}  {name}")
    manifest = tmp_path / "SHA256SUMS"
    manifest.write_text("\n".join(lines) + "\n")
    return source, manifest


def test_copies_nested_pinned_files(tmp_path):
    source, manifest = write_source(tmp_path)
    assert module.copy_model(tmp_path / "out", source, manifest).startswith("zhiju model available: 3")
    assert (tmp_path / "out/m.mlmodelc/weights/weight.bin").read_bytes() == b"w"


def test_changed_or_missing_file_copies_nothing(tmp_path):
    source, manifest = write_source(tmp_path)
    (source / "m.mlmodelc/model.mil").write_bytes(b"changed")
    assert "checksum mismatch" in module.copy_model(tmp_path / "out", source, manifest)
    assert not (tmp_path / "out").exists()
    assert "missing" in module.copy_model(tmp_path / "out2", tmp_path / "none", manifest)


def test_rejects_escaping_paths(tmp_path):
    source, manifest = write_source(tmp_path)
    manifest.write_text("00  ../evil\n")
    try:
        module.copy_model(tmp_path / "out", source, manifest)
    except ValueError:
        return
    raise AssertionError("expected ValueError")
