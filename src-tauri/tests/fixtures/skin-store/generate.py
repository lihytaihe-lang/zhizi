#!/usr/bin/env python3
"""仅重建公开测试夹具；永远不读取生产签名私钥。"""
import json
import os
from pathlib import Path
import shutil
import sys

from cryptography.hazmat.primitives import serialization
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[3]
sys.path.insert(0, str(ROOT / "scripts/skins"))
import build_skin_bundle
import build_skin_index


def main():
    source = ROOT / "native/resources/Skins/Builtin/system"
    skin = json.loads((source / "skin.json").read_bytes())
    compatible = HERE / "teststore"
    compatible.mkdir(exist_ok=True)
    # 该基线的 P1 安装器仅接收 skin.json；显式裁成其可接受的样式子集。
    p1 = {key: value for key, value in skin.items() if key != "decorations" and not (key == "panelBorderWidth" and value is None)}
    p1["id"] = "teststore"
    (compatible / "skin.json").write_text(json.dumps(p1, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    full = HERE / "testfull"
    full.mkdir(exist_ok=True)
    skin["id"] = "testfull"
    (full / "skin.json").write_text(json.dumps(skin, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    shutil.copyfile(source / "mark.png", full / "mark.png")
    shutil.copyfile(source / "mark.png", HERE / "preview.png")
    for directory, filename in [(compatible, "teststore-1.0.0.zhskin.json"), (full, "full-system-1.0.0.zhskin.json")]:
        if build_skin_bundle.main([str(directory), "--out", str(HERE / filename), "--version", "1.0.0"]):
            return 1
    # 公开且可预测的测试种子，不具备任何生产授权。
    test_seed = bytes(range(32))
    private = Ed25519PrivateKey.from_private_bytes(test_seed)
    (HERE / "test-only-signing.key").write_text(test_seed.hex() + "\n", encoding="ascii")
    public = private.public_key().public_bytes(serialization.Encoding.Raw, serialization.PublicFormat.Raw)
    (HERE / "public-key.hex").write_text(public.hex() + "\n", encoding="ascii")
    config = {
        "schema": "agent-ime-skin-index/1", "generatedAt": "2026-11-08T04:00:00Z",
        "producer": "agent-ime-skin-publisher/1.0.0", "skins": [{
            "id": "teststore", "name": {"zh": "测试简洁", "en": "Test Clean"},
            "author": skin["author"], "description": "从内置简洁皮肤派生的公开测试夹具，不用于上架。",
            "version": "1.0.0", "tier": "free", "priceCents": 0, "productId": "skin.teststore", "animated": False,
            "previewUrl": "https://example.invalid/skins/preview.png", "previewPath": "preview.png",
            "bundleUrl": "https://example.invalid/skins/teststore-1.0.0.zhskin.json",
            "bundlePath": "teststore-1.0.0.zhskin.json", "minAppVersion": "0.3.24", "updatedAt": "2026-11-08",
            "license": "Agent Input Method 自制，仅供测试", "changelog": "首次测试夹具。",
        }],
    }
    (HERE / "publisher-config.json").write_text(json.dumps(config, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    previous = os.environ.get("ZHIZI_SKIN_KEY_FILE")
    try:
        os.environ["ZHIZI_SKIN_KEY_FILE"] = str(HERE / "test-only-signing.key")
        return build_skin_index.main([str(HERE / "publisher-config.json"), "--out", str(HERE),
                                      "--test-public-key-file", str(HERE / "public-key.hex")])
    finally:
        if previous is None:
            os.environ.pop("ZHIZI_SKIN_KEY_FILE", None)
        else:
            os.environ["ZHIZI_SKIN_KEY_FILE"] = previous


if __name__ == "__main__":
    sys.exit(main())
