#!/usr/bin/env python3
"""离线生成并签名皮肤索引；--verify 仅验签，不下载、不发布、不输出私钥。"""
import argparse
from datetime import date, datetime
import hashlib
import os
from pathlib import Path
import re
import sys
from urllib.parse import urlsplit

from cryptography.exceptions import InvalidSignature
from cryptography.hazmat.primitives import serialization
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey, Ed25519PublicKey

from build_skin_bundle import (MAX_BUNDLE_BYTES, canonical_bytes, is_png, load_json,
                               read_regular, skin_id, validate_bundle, version)

ROOT = Path(__file__).resolve().parents[2]
PUBLIC_KEY_HEX = "cc1ce96bbaefc8d7f33dc9b32fc9e0faa95ef235db2eb0701c54aadc079da7e5"
SCHEMA = "agent-ime-skin-index/1"
MAX_INDEX_BYTES = 512 * 1024
MAX_PREVIEW_BYTES = 300 * 1024
MAX_SKINS = 200
SKIN_FIELDS = {"id", "name", "author", "description", "version", "tier", "priceCents", "productId",
               "animated", "previewUrl", "previewSha256", "bundleUrl", "bundleSha256", "bundleBytes",
               "minAppVersion", "updatedAt", "license", "changelog"}


def download_url(value):
    try:
        if not isinstance(value, str) or len(value) > 2048 or not value.startswith("https://"):
            raise ValueError()
        parsed = urlsplit(value)
        if (parsed.scheme != "https" or not parsed.hostname or parsed.username is not None
                or parsed.password is not None or "?" in value or "#" in value
                or "\\" in value or any(c.isspace() or ord(c) < 32 or 127 <= ord(c) <= 159 for c in value)):
            raise ValueError()
        parsed.port
    except (ValueError, TypeError):
        raise ValueError("下载地址必须是无参数、无身份信息的 HTTPS URL。") from None
    return value


def text(value, label, maximum, empty=False):
    if (not isinstance(value, str) or len(value) > maximum or (not empty and not value.strip())
            or any(ord(c) < 32 and c not in "\n\r\t" or 127 <= ord(c) <= 159 for c in value)):
        raise ValueError(label + "为空、类型无效、含控制字符或过长。")


def validate_index(value):
    if (not isinstance(value, dict) or set(value) != {"schema", "generatedAt", "producer", "skins"}
            or value.get("schema") != SCHEMA):
        raise ValueError("不支持的皮肤索引 schema 或字段。")
    generated = value.get("generatedAt")
    try:
        if (not isinstance(generated, str) or not re.fullmatch(
                r"[0-9]{4}-[0-9]{2}-[0-9]{2}[Tt][0-9]{2}:[0-9]{2}:[0-9]{2}(?:\.[0-9]+)?(?:[Zz]|[+-][0-9]{2}:[0-9]{2})", generated)
                or datetime.fromisoformat(generated.upper().replace("Z", "+00:00")).tzinfo is None):
            raise ValueError()
    except (ValueError, TypeError):
        raise ValueError("生成日期必须为带时区的 RFC3339 时间。") from None
    text(value.get("producer"), "生成器", 120)
    skins = value.get("skins")
    if not isinstance(skins, list) or len(skins) > MAX_SKINS:
        raise ValueError("索引必须包含最多 200 条皮肤。")
    ids = set()
    for item in skins:
        if not isinstance(item, dict) or not SKIN_FIELDS.issubset(item) or bool(set(item) - SKIN_FIELDS - {"currency", "platforms", "availableForSale"}):
            raise ValueError("皮肤索引条目字段不完整或包含未知字段。")
        identifier = skin_id(item.get("id"))
        if (ROOT / "native/resources/Skins/Builtin" / identifier).is_dir():
            raise ValueError("商店皮肤 id 不能与内置皮肤冲突。")
        if identifier in ids:
            raise ValueError("索引皮肤 id 重复。")
        ids.add(identifier)
        name = item.get("name")
        if not isinstance(name, dict) or set(name) != {"zh", "en"}:
            raise ValueError("皮肤名称必须包含 zh/en。")
        text(name.get("zh"), "中文名", 80)
        text(name.get("en"), "英文名", 80)
        for key, maximum, empty in [("author", 120, False), ("description", 1000, True),
                                    ("productId", 128, False), ("license", 1000, False), ("changelog", 2000, True)]:
            text(item.get(key), key, maximum, empty)
        version(item.get("version"))
        version(item.get("minAppVersion"))
        price = item.get("priceCents")
        tier = item.get("tier")
        if type(price) is not int or not 0 <= price <= 99900 or tier not in ("free", "paid") or (tier == "free") != (price == 0):
            raise ValueError("价格必须为 0–99900 的整数，且免费当且仅当价格为 0。")
        if item.get("currency", "CNY") not in ("CNY", "USD", "HKD", "EUR", "GBP", "JPY"):
            raise ValueError("不支持的价格币种。")
        if type(item.get("availableForSale", True)) is not bool:
            raise ValueError("availableForSale 必须是布尔值。")
        platforms = item.get("platforms", ["macos"])
        if not isinstance(platforms, list) or not 1 <= len(platforms) <= 3 or any(p not in ("macos", "windows", "linux") for p in platforms) or len(set(platforms)) != len(platforms):
            raise ValueError("适用系统必须是 macos、windows、linux 中不重复的项目。")
        if type(item.get("animated")) is not bool:
            raise ValueError("animated 必须为布尔值。")
        for key in ("previewUrl", "bundleUrl"):
            download_url(item.get(key))
        for key in ("previewSha256", "bundleSha256"):
            if not isinstance(item.get(key), str) or not re.fullmatch(r"[0-9a-fA-F]{64}", item[key]):
                raise ValueError("SHA-256 必须为 64 位十六进制。")
        size = item.get("bundleBytes")
        if type(size) is not int or not 1 <= size <= MAX_BUNDLE_BYTES:
            raise ValueError("皮肤包大小必须为 1–2.2 MiB 的整数字节数。")
        try:
            if not isinstance(item.get("updatedAt"), str) or not re.fullmatch(r"[0-9]{4}-[0-9]{2}-[0-9]{2}", item["updatedAt"]):
                raise ValueError()
            date.fromisoformat(item["updatedAt"])
        except (ValueError, TypeError):
            raise ValueError("更新日期必须为 YYYY-MM-DD。") from None
    return value


def public_key(test_file):
    try:
        value = read_regular(test_file, 66).decode("ascii").strip() if test_file is not None else PUBLIC_KEY_HEX
        if not re.fullmatch(r"[0-9a-fA-F]{64}", value):
            raise ValueError()
        return Ed25519PublicKey.from_public_bytes(bytes.fromhex(value))
    except (ValueError, OSError, UnicodeError):
        raise ValueError("无法读取有效的验签公钥。") from None


def signing_key(expected):
    try:
        path = Path(os.environ.get("ZHIZI_SKIN_KEY_FILE", "~/.zhizi-secrets/skin-signing.key")).expanduser()
        value = read_regular(path, 66).decode("ascii").strip()
        if not re.fullmatch(r"[0-9a-fA-F]{64}", value):
            raise ValueError()
        private = Ed25519PrivateKey.from_private_bytes(bytes.fromhex(value))
    except (ValueError, OSError, UnicodeError):
        raise ValueError("无法读取有效的签名私钥。") from None
    derived = private.public_key().public_bytes(serialization.Encoding.Raw, serialization.PublicFormat.Raw)
    target = expected.public_bytes(serialization.Encoding.Raw, serialization.PublicFormat.Raw)
    if derived != target:
        raise ValueError("签名私钥与内置公钥（或显式测试公钥）不匹配，未生成索引。")
    return private


def build(args):
    config = load_json(read_regular(args.document, MAX_INDEX_BYTES))
    if not isinstance(config, dict) or set(config) - {"schema", "generatedAt", "producer", "skins"}:
        raise ValueError("商品清单必须为 JSON 对象，且不能有未知字段。")
    if config.get("schema", SCHEMA) != SCHEMA:
        raise ValueError("不支持的皮肤索引 schema。")
    rows = config.get("skins")
    if not isinstance(rows, list) or len(rows) > MAX_SKINS:
        raise ValueError("索引必须包含最多 200 条皮肤。")
    result = {"schema": SCHEMA, "generatedAt": config.get("generatedAt"),
              "producer": config.get("producer", "agent-ime-skin-publisher/1.0.0"), "skins": []}
    for row in rows:
        if not isinstance(row, dict):
            raise ValueError("每条商品必须为 JSON 对象。")
        item = dict(row)
        bundle_path = item.pop("bundlePath", None)
        preview_path = item.pop("previewPath", None)
        if not isinstance(bundle_path, str) or not isinstance(preview_path, str):
            raise ValueError("商品清单必须指定本地 bundlePath 与 previewPath。")
        data = read_regular(args.document.parent / bundle_path, MAX_BUNDLE_BYTES)
        bundle = validate_bundle(load_json(data))
        if item.get("id") != bundle["id"] or item.get("version") != bundle["version"]:
            raise ValueError("商品清单与皮肤包 id/version 不一致。")
        preview = read_regular(args.document.parent / preview_path, MAX_PREVIEW_BYTES)
        if not is_png(preview):
            raise ValueError("预览文件必须为 PNG。")
        item.update({"bundleSha256": hashlib.sha256(data).hexdigest(), "bundleBytes": len(data),
                     "previewSha256": hashlib.sha256(preview).hexdigest()})
        result["skins"].append(item)
    validate_index(result)
    data = canonical_bytes(result)
    if len(data) > MAX_INDEX_BYTES:
        raise ValueError("皮肤索引超过 512 KiB。")
    private = signing_key(public_key(args.test_public_key_file))
    signature = (private.sign(data).hex() + "\n").encode("ascii")
    args.out.mkdir(parents=True, exist_ok=True)
    (args.out / "skins-index.json").write_bytes(data)
    (args.out / "skins-index.json.sig").write_bytes(signature)
    print("已生成皮肤索引及分离签名；共 %d 条。" % len(rows))


def verify(args):
    data = read_regular(args.document, MAX_INDEX_BYTES)
    signature = read_regular(Path(str(args.document) + ".sig"), 130).decode("ascii").strip()
    if not re.fullmatch(r"[0-9a-fA-F]{128}", signature):
        raise ValueError("分离签名格式无效。")
    value = load_json(data)
    public_key(args.test_public_key_file).verify(bytes.fromhex(signature), canonical_bytes(value))
    validate_index(value)
    print("皮肤索引签名与字段校验通过。")


def main(argv=None):
    argv = list(sys.argv[1:] if argv is None else argv)
    verify_only = bool(argv and argv[0] == "--verify")
    if verify_only:
        argv.pop(0)
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("document", type=Path, help="人工商品清单；--verify 模式为已签名索引")
    parser.add_argument("--out", type=Path, required=not verify_only)
    parser.add_argument("--test-public-key-file", type=Path,
                        help="仅供测试夹具使用；省略时严格核对内置生产公钥")
    args = parser.parse_args(argv)
    try:
        (verify if verify_only else build)(args)
    except (InvalidSignature, ValueError, OSError, UnicodeError, OverflowError, TypeError, KeyError):
        # 不打印底层异常：文件路径、输入内容均可能携带私钥。
        print("索引或签名校验失败、字段无效或文件读写失败；未完成操作。", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
