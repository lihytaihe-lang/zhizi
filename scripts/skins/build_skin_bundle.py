#!/usr/bin/env python3
"""离线打包皮肤目录；不修改源文件、不下载、不发布。"""
import argparse
import base64
import binascii
import json
from pathlib import Path
import re
import sys
import zlib

SCHEMA = "agent-ime-skin-bundle/1"
MAX_FILES = 8
MAX_PACKAGE_BYTES = 1536 * 1024
MAX_SKIN_JSON_BYTES = 256 * 1024
MAX_BUNDLE_BYTES = 22 * 1024 * 1024 // 10
PNG_HEADER = b"\x89PNG\r\n\x1a\n"


def is_png(data):
    if not data.startswith(PNG_HEADER):
        return False
    offset = len(PNG_HEADER)
    has_header = False
    has_image = False
    while offset + 12 <= len(data):
        length = int.from_bytes(data[offset:offset + 4], "big")
        end = offset + 12 + length
        if end > len(data):
            return False
        kind = data[offset + 4:offset + 8]
        chunk = data[offset + 8:end - 4]
        if zlib.crc32(data[offset + 4:end - 4]) != int.from_bytes(data[end - 4:end], "big"):
            return False
        if not has_header:
            if kind != b"IHDR" or length != 13 or not int.from_bytes(chunk[:4], "big") or not int.from_bytes(chunk[4:8], "big"):
                return False
            has_header = True
        elif kind == b"IHDR":
            return False
        if kind == b"IDAT":
            has_image = True
        if kind == b"IEND":
            return length == 0 and has_image and end == len(data)
        offset = end
    return False


def canonical_bytes(value):
    def check(item):
        if isinstance(item, float):
            raise ValueError("规范化 JSON 不接受小数。")
        if isinstance(item, dict):
            for child in item.values():
                check(child)
        elif isinstance(item, list):
            for child in item:
                check(child)
    check(value)
    return (json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":"), allow_nan=False) + "\n").encode("utf-8")


def load_json(data):
    def pairs(items):
        result = {}
        for key, value in items:
            if key in result:
                raise ValueError("JSON 存在重复字段。")
            result[key] = value
        return result
    return json.loads(data, object_pairs_hook=pairs, parse_constant=lambda _: (_ for _ in ()).throw(ValueError("JSON 数值无效。")))


def skin_id(value):
    if not isinstance(value, str) or not re.fullmatch(r"[a-z][a-z0-9._-]{0,63}", value) or ".." in value:
        raise ValueError("皮肤 id 必须是 1–64 位小写字母、数字、点、短横线或下划线，且以字母开头、不能含 ..。")
    return value


def version(value):
    if (not isinstance(value, str) or not re.fullmatch(r"[0-9]+(?:\.[0-9]+)*", value)
            or len(value) > 128 or any(int(part) > 18446744073709551615 for part in value.split("."))):
        raise ValueError("版本必须是数字点分格式。")
    return value


def file_path(value):
    if (not isinstance(value, str) or not value or value in (".", "..") or ".." in value
            or "/" in value or "\\" in value or ":" in value
            or any(ord(char) < 32 or 127 <= ord(char) <= 159 for char in value)
            or Path(value).suffix not in (".json", ".png")):
        raise ValueError("包内只允许安全文件名的 JSON/PNG 普通文件，不允许子目录或路径穿越。")
    return value


def read_regular(path, maximum):
    if path.is_symlink() or not path.is_file() or path.stat().st_size > maximum:
        raise ValueError("文件不是普通文件、是符号链接或超过大小上限。")
    with path.open("rb") as stream:
        data = stream.read(maximum + 1)
    if len(data) > maximum:
        raise ValueError("文件超过大小上限。")
    return data


def validate_files(files, expected_id, expected_version):
    if not isinstance(files, list) or not 1 <= len(files) <= MAX_FILES:
        raise ValueError("单包必须包含 1–8 个文件。")
    paths = set()
    total = 0
    found_skin = False
    for item in files:
        if not isinstance(item, dict) or set(item) != {"path", "base64"}:
            raise ValueError("包内文件字段必须是 path/base64。")
        path = file_path(item["path"])
        if path in paths:
            raise ValueError("包内文件名重复。")
        paths.add(path)
        encoded = item["base64"]
        if not isinstance(encoded, str) or len(encoded) > (MAX_PACKAGE_BYTES + 2) // 3 * 4:
            raise ValueError("包内 Base64 无效或过大。")
        try:
            data = base64.b64decode(encoded, validate=True)
            if base64.b64encode(data).decode("ascii") != encoded:
                raise ValueError()
        except (ValueError, binascii.Error):
            raise ValueError("包内 Base64 无效。") from None
        total += len(data)
        if total > MAX_PACKAGE_BYTES:
            raise ValueError("单包解开后超过 1.5 MiB。")
        if path.endswith(".png"):
            if not is_png(data):
                raise ValueError("PNG 文件内容无效。")
        else:
            try:
                value = load_json(data)
            except (ValueError, UnicodeError):
                raise ValueError("包内 JSON 无效。") from None
            if path == "skin.json":
                found_skin = True
                if len(data) > MAX_SKIN_JSON_BYTES:
                    raise ValueError("skin.json 超过 256 KiB。")
                if (not isinstance(value, dict) or value.get("id") != expected_id
                        or type(value.get("formatVersion")) is not int or value["formatVersion"] != 1):
                    raise ValueError("skin.json 的 id 或 formatVersion 无效。")
                if "version" in value and value["version"] != expected_version:
                    raise ValueError("skin.json 与皮肤包版本不一致。")
    if not found_skin:
        raise ValueError("皮肤包缺少 skin.json。")


def validate_bundle(value):
    if not isinstance(value, dict) or set(value) != {"schema", "id", "version", "files"} or value.get("schema") != SCHEMA:
        raise ValueError("不支持的皮肤包 schema 或字段。")
    skin_id(value.get("id"))
    version(value.get("version"))
    validate_files(value.get("files"), value["id"], value["version"])
    return value


def build(source, output, bundle_version):
    if source.is_symlink() or not source.is_dir():
        raise ValueError("皮肤源必须是普通目录，不能是符号链接。")
    entries = sorted(source.iterdir(), key=lambda path: path.name)
    if not 1 <= len(entries) <= MAX_FILES:
        raise ValueError("单包必须包含 1–8 个文件。")
    files = []
    total = 0
    for path in entries:
        file_path(path.name)
        data = read_regular(path, MAX_SKIN_JSON_BYTES if path.name == "skin.json" else MAX_PACKAGE_BYTES)
        total += len(data)
        if total > MAX_PACKAGE_BYTES:
            raise ValueError("单包解开后超过 1.5 MiB。")
        files.append({"path": path.name, "base64": base64.b64encode(data).decode("ascii")})
    skin = load_json(read_regular(source / "skin.json", MAX_SKIN_JSON_BYTES))
    if not isinstance(skin, dict):
        raise ValueError("skin.json 必须为 JSON 对象。")
    selected_version = bundle_version if bundle_version is not None else skin.get("version", "1.0.0")
    if "version" in skin and skin["version"] != selected_version:
        raise ValueError("skin.json 与皮肤包版本不一致。")
    value = {"schema": SCHEMA, "id": skin.get("id"), "version": selected_version, "files": files}
    validate_bundle(value)
    data = canonical_bytes(value)
    if len(data) > MAX_BUNDLE_BYTES:
        raise ValueError("皮肤包超过 2.2 MiB。")
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_bytes(data)
    print("已生成皮肤包；共 %d 个文件。" % len(files))


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--version", help="数字点分版本；默认采用 skin.json 的 version，缺省为 1.0.0")
    args = parser.parse_args(argv)
    try:
        build(args.source, args.out, args.version)
    except (ValueError, OSError, UnicodeError, OverflowError, TypeError, KeyError):
        print("皮肤目录或包字段无效、超限，或文件读写失败；未完成打包。", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
