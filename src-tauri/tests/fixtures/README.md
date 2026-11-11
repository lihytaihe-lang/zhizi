# 热词签名测试夹具

此目录 `hotword_test_seed.hex` 与 `hotword_test_pubkey.hex` 是公开、可重复生成、仅供自动测试的 Ed25519 密钥对，与客户端生产公钥不同，禁止用于发布。种子字节为递增的 0 到 31，不是生产秘密。

四份 JSON/签名产物由 `scripts/hotword/build_pack.py` 用 `scripts/hotword/sample/test-input.tsv` 和上述测试密钥生成，日期固定为 2026-11-08。Python 测试重建并逐字节比较，Rust 测试读取同一份产物验签，验证两端规范化与签名互通。
