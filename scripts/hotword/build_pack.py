#!/usr/bin/env python3
"""离线整理、签名与验签；不采集、不发布，不输出私钥内容。"""
import argparse
import csv
from datetime import date, datetime, timedelta, timezone
import hashlib
import json
import os
from pathlib import Path
import re
import sys
from urllib.parse import urlsplit

from cryptography.exceptions import InvalidSignature
from cryptography.hazmat.primitives import serialization
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey, Ed25519PublicKey

ROOT = Path(__file__).resolve().parents[2]
PUBLIC_KEY_FILE = ROOT / "src-tauri/resources/hotword_pack_pubkey.hex"
PACK_SCHEMA = "agent-ime-hotword-pack/1"
LATEST_SCHEMA = "agent-ime-hotword-latest/1"
MAX_BYTES = 2 * 1024 * 1024


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


def parse_time(value):
    try:
        if not isinstance(value, str) or not re.fullmatch(r"[0-9]{4}-[0-9]{2}-[0-9]{2}[Tt][0-9]{2}:[0-9]{2}:[0-9]{2}(?:\.[0-9]+)?(?:[Zz]|[+-][0-9]{2}:[0-9]{2})", value):
            raise ValueError()
        parsed = datetime.fromisoformat(value.upper().replace("Z", "+00:00"))
        if parsed.tzinfo is None:
            raise ValueError()
        return parsed.astimezone(timezone.utc)
    except (ValueError, TypeError, AttributeError):
        raise ValueError("日期必须是带时区的 RFC3339 时间。") from None


def timestamp(value):
    return value.astimezone(timezone.utc).isoformat(timespec="seconds").replace("+00:00", "Z")


def download_url(value):
    try:
        if not isinstance(value, str):
            raise ValueError()
        parsed = urlsplit(value)
        if (parsed.scheme != "https" or not parsed.hostname or parsed.username is not None
                or parsed.password is not None or parsed.query or parsed.fragment
                or "?" in value or "#" in value or any(ord(c) <= 32 or ord(c) == 127 for c in value)):
            raise ValueError()
        parsed.port
    except (ValueError, TypeError):
        raise ValueError("下载地址必须是无参数、无身份信息的 HTTPS URL。") from None
    return value


def source_url(value):
    try:
        if not isinstance(value, str) or any(c.isspace() or ord(c) < 32 or 127 <= ord(c) <= 159 for c in value):
            raise ValueError()
        parsed = urlsplit(value)
        if parsed.scheme != "https" or not parsed.hostname or parsed.username is not None or parsed.password is not None:
            raise ValueError()
        parsed.port
    except (ValueError, TypeError):
        raise ValueError("来源 URL 必须是有效 HTTPS 地址。") from None
    return value


def nonempty(value, label):
    if not isinstance(value, str) or not value.strip():
        raise ValueError(label + "不能为空。")


def validate_pack(pack, now):
    if not isinstance(pack, dict):
        raise ValueError("词包必须为 JSON 对象。")
    if pack.get("schema") != PACK_SCHEMA:
        raise ValueError("不支持的词包 schema。")
    if parse_time(pack.get("validUntil")) <= now:
        raise ValueError("词包已过期。")
    entries = pack.get("entries")
    count = pack.get("entryCount")
    if not isinstance(entries, list) or type(count) is not int or count != len(entries) or count > 2000:
        raise ValueError("词包条数不一致或超过 2,000 条。")
    nonempty(pack.get("packId"), "词包 ID")
    nonempty(pack.get("producer"), "生成器")
    parse_time(pack.get("generatedAt"))
    for item in entries:
        if not isinstance(item, dict):
            raise ValueError("词条必须为 JSON 对象。")
        text = item.get("text")
        if not isinstance(text, str) or not text.strip() or not 1 <= len(text) <= 16 or any(ord(c) < 32 or 127 <= ord(c) <= 159 for c in text):
            raise ValueError("词条必须为 1–16 个字符，且不能含控制字符。")
        pinyin = item.get("pinyin")
        if not isinstance(pinyin, str) or not re.fullmatch("[a-z ']+", pinyin) or not any("a" <= c <= "z" for c in pinyin):
            raise ValueError("拼音只能使用小写字母、空格和撇号。")
        definition = item.get("definition")
        if not isinstance(definition, str) or len(definition) > 120:
            raise ValueError("释义不能超过 120 字。")
        if not isinstance(item.get("domain"), str):
            raise ValueError("领域必须为字符串。")
        confidence = item.get("confidenceMillis")
        if type(confidence) is not int or not 0 <= confidence <= 1000:
            raise ValueError("可信度必须为 0–1000 的整数。")
        expiry = item.get("expiresAtMs")
        if expiry is not None and (type(expiry) is not int or not 0 <= expiry <= 9223372036854775807):
            raise ValueError("词条过期时间必须为非负毫秒整数。")
        try:
            if not isinstance(item["firstSeenAt"], str) or not re.fullmatch(r"[0-9]{4}-[0-9]{2}-[0-9]{2}", item["firstSeenAt"]):
                raise ValueError()
            date.fromisoformat(item["firstSeenAt"])
        except (KeyError, TypeError, ValueError):
            raise ValueError("首次收录日期必须为 YYYY-MM-DD。") from None
        sources = item.get("sources")
        if not isinstance(sources, list) or not sources:
            raise ValueError("每条词至少需要一个来源。")
        for source in sources:
            if not isinstance(source, dict):
                raise ValueError("来源必须为 JSON 对象。")
            nonempty(source.get("name"), "来源名")
            nonempty(source.get("license"), "许可")
            source_url(source.get("url"))
            parse_time(source.get("collectedAt"))
    return pack


def read_key(path, private=False):
    try:
        value = path.read_text(encoding="ascii").strip()
        if not re.fullmatch("[0-9a-fA-F]{64}", value):
            raise ValueError()
        raw = bytes.fromhex(value)
        return Ed25519PrivateKey.from_private_bytes(raw) if private else Ed25519PublicKey.from_public_bytes(raw)
    except (OSError, UnicodeError, ValueError):
        raise ValueError("无法读取有效的签名私钥。" if private else "无法读取有效的验签公钥。") from None


def load_rows(path):
    try:
        if path.suffix.lower() == ".json":
            rows = load_json(path.read_bytes())
        else:
            with path.open(encoding="utf-8-sig", newline="") as stream:
                rows = list(csv.DictReader(stream, delimiter="\t"))
    except (OSError, UnicodeError, json.JSONDecodeError):
        raise ValueError("无法读取人工整理的 TSV/JSON 输入。") from None
    if not isinstance(rows, list):
        raise ValueError("JSON 输入必须是词条数组。")
    return rows


def build(args):
    generated = parse_time(args.generated_at) if args.generated_at else datetime.now(timezone.utc).replace(microsecond=0)
    if args.valid_days <= 0:
        raise ValueError("词包有效天数必须大于 0。")
    name = "hotword-" + generated.strftime("%Y%m%d")
    base = download_url(args.base_url).rstrip("/")
    pack_url = download_url(base + "/" + name + ".json")
    entries = []
    def integer(value):
        if type(value) is int:
            return value
        if isinstance(value, str) and re.fullmatch(r"-?[0-9]+", value):
            return int(value)
        raise ValueError("过期天数与可信度必须为整数。")
    for row in load_rows(args.input):
        if not isinstance(row, dict):
            raise ValueError("每一行必须为一个词条对象。")
        try:
            days = integer(row.get("expiresDays", 30))
            confidence = integer(row.get("confidenceMillis", 600))
        except (TypeError, ValueError):
            raise ValueError("过期天数与可信度必须为整数。") from None
        if days <= 0:
            raise ValueError("词条过期天数必须大于 0。")
        entries.append({
            "text": row.get("text"), "pinyin": row.get("pinyin"), "definition": row.get("definition", ""),
            "domain": row.get("domain", "hotword"), "confidenceMillis": confidence,
            "expiresAtMs": int((generated + timedelta(days=days)).timestamp() * 1000),
            "firstSeenAt": generated.date().isoformat(),
            "sources": [{"name": row.get("sourceName"), "url": row.get("sourceUrl"), "license": row.get("license"), "collectedAt": timestamp(generated)}],
        })
    pack = {"schema": PACK_SCHEMA, "packId": name, "generatedAt": timestamp(generated),
            "validUntil": timestamp(generated + timedelta(days=args.valid_days)),
            "producer": "agent-ime-hotword-manual/1", "entryCount": len(entries), "entries": entries}
    validate_pack(pack, generated)
    pack_bytes = canonical_bytes(pack)
    if len(pack_bytes) > MAX_BYTES:
        raise ValueError("词包超过 2 MiB。")
    latest_bytes = canonical_bytes({"schema": LATEST_SCHEMA, "packId": name, "url": pack_url,
                                    "sha256": hashlib.sha256(pack_bytes).hexdigest(), "generatedAt": timestamp(generated)})
    # 只从环境变量/约定路径读取，不将路径或内容写入输出。
    private = read_key(Path(os.environ.get("ZHIZI_HOTWORD_KEY_FILE", "~/.zhizi-secrets/hotword-signing.key")).expanduser(), private=True)
    public = read_key(args.public_key_file)
    raw_public = private.public_key().public_bytes(serialization.Encoding.Raw, serialization.PublicFormat.Raw)
    expected = public.public_bytes(serialization.Encoding.Raw, serialization.PublicFormat.Raw)
    if raw_public != expected:
        raise ValueError("签名私钥与指定公钥不匹配，未生成词包。")
    outputs = {name + ".json": pack_bytes, name + ".json.sig": (private.sign(pack_bytes).hex() + "\n").encode(),
               "latest.json": latest_bytes, "latest.json.sig": (private.sign(latest_bytes).hex() + "\n").encode()}
    args.output_dir.mkdir(parents=True, exist_ok=True)
    for filename, data in outputs.items():
        (args.output_dir / filename).write_bytes(data)
    print("已生成词包、指针及两份分离签名；共 %d 条。" % len(entries))


def verify(args):
    try:
        data = args.document.read_bytes()
        sig = Path(str(args.document) + ".sig").read_text(encoding="ascii").strip()
    except (OSError, UnicodeError):
        raise ValueError("无法读取待验文件或分离签名。") from None
    if len(data) > MAX_BYTES:
        raise ValueError("待验文件超过 2 MiB。")
    try:
        value = load_json(data)
        normalized = canonical_bytes(value)
        if not re.fullmatch("[0-9a-fA-F]{128}", sig):
            raise ValueError()
        read_key(args.public_key_file).verify(bytes.fromhex(sig), normalized)
    except (InvalidSignature, ValueError, UnicodeError, json.JSONDecodeError):
        raise ValueError("签名校验失败，整包已丢弃。") from None
    if not isinstance(value, dict):
        raise ValueError("签名文件必须是 JSON 对象。")
    if value.get("schema") == PACK_SCHEMA:
        validate_pack(value, parse_time(args.now) if args.now else datetime.now(timezone.utc))
    elif value.get("schema") == LATEST_SCHEMA:
        nonempty(value.get("packId"), "词包 ID")
        download_url(value.get("url"))
        parse_time(value.get("generatedAt"))
        if not isinstance(value.get("sha256"), str) or not re.fullmatch("[0-9a-fA-F]{64}", value["sha256"]):
            raise ValueError("指针 SHA-256 无效。")
    else:
        raise ValueError("不支持的词包 schema。")
    print("签名与词包校验通过。")


def main(argv=None):
    argv = list(sys.argv[1:] if argv is None else argv)
    if argv and argv[0] == "--verify":
        argv[0] = "verify"
    if argv and argv[0] not in ["build", "verify", "-h", "--help"]:
        argv.insert(0, "build")
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    builder = sub.add_parser("build", help="离线打包并签名")
    builder.add_argument("input", type=Path)
    builder.add_argument("--output-dir", type=Path, required=True)
    builder.add_argument("--base-url", required=True)
    builder.add_argument("--generated-at")
    builder.add_argument("--valid-days", type=int, default=30)
    builder.add_argument("--public-key-file", type=Path, default=PUBLIC_KEY_FILE)
    verifier = sub.add_parser("verify", help="仅本地验签，也可写 --verify")
    verifier.add_argument("document", type=Path)
    verifier.add_argument("--public-key-file", type=Path, default=PUBLIC_KEY_FILE)
    verifier.add_argument("--now")
    args = parser.parse_args(argv)
    try:
        (build if args.command == "build" else verify)(args)
    except (ValueError, OverflowError, TypeError, OSError, KeyError) as error:
        # OSError 往往包含路径，统一隐藏；其余可预期错误不含密钥或路径。
        message = "文件读写失败，未完成操作。" if isinstance(error, OSError) else str(error)
        if not isinstance(error, ValueError):
            message = "输入字段无效，未完成操作。" if not isinstance(error, OSError) else message
        print(message, file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
