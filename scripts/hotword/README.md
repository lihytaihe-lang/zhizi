# 人工热词包工具（Phase 1）

`build_pack.py` 只读取人工整理的本地 TSV/JSON，生成本地签名文件；不采集、不联网、不上传。输入示例都明确标为测试，`example.invalid` 不是实际来源或托管。

输入 TSV 首行为字段名，JSON 为对象数组。必填 `text`（词）、`pinyin`（无声调小写拼音）、`sourceName`（来源名）、`sourceUrl`（HTTPS 来源 URL）、`license`（许可）；可选 `definition`（≤120 字）、`domain`（默认 hotword）、`expiresDays`（默认 30 天）、`confidenceMillis`（默认 600）。每条词保留来源与许可，客户端收包后仍要人工审核。

从仓库根目录运行测试打包：

```sh
ZHIZI_HOTWORD_KEY_FILE=src-tauri/tests/fixtures/hotword_test_seed.hex python3 scripts/hotword/build_pack.py build scripts/hotword/sample/test-input.tsv --output-dir /private/tmp/hotword-test-pack --base-url https://example.invalid/hotword-test --generated-at 2026-11-08T00:00:00Z --public-key-file src-tauri/tests/fixtures/hotword_test_pubkey.hex
python3 scripts/hotword/build_pack.py --verify /private/tmp/hotword-test-pack/hotword-20261108.json --public-key-file src-tauri/tests/fixtures/hotword_test_pubkey.hex --now 2026-11-08T00:00:00Z
python3 scripts/hotword/build_pack.py verify /private/tmp/hotword-test-pack/latest.json --public-key-file src-tauri/tests/fixtures/hotword_test_pubkey.hex
python3 -m unittest scripts.hotword.test_build_pack -v
```

产物为 `hotword-YYYYMMDD.json`、同名 `.json.sig`、`latest.json`、`latest.json.sig`。`--base-url` 必填，用来写入指针的包 URL；它只生成文本，不将文件发往该地址。URL 必须为 HTTPS，无 query、fragment、身份信息。`--generated-at` 可固定重建日期，`--valid-days` 默认 30。`--verify` 与 `verify` 等价，只在本地验签并校验格式和有效期。

生产公钥来自 `src-tauri/resources/hotword_pack_pubkey.hex`，客户端以 `include_str!` 编译进程序，不包含私钥。生产打包时省略 `--public-key-file`；脚本从规定环境变量指向的仓库外密钥文件读取种子，并先核对派生公钥与内置生产公钥一致。`--public-key-file` 是本地工具的公钥核验入口，这里仅用于公开测试夹具；管理台生产代码不支持替换公钥。此仓库中的测试密钥与生产公钥不同，不能用于发布。

规范化规则：递归 key 排序、无多余空白、UTF-8、末尾恰好一个换行；只接受整数数值，拒绝重复 key。Ed25519 分离签名为 128 个十六进制字符，签名文件末尾带换行。指针 SHA-256 对正文文件的原始字节计算。
