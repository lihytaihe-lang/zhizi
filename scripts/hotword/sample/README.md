# 仅测试：人工词包输入

这 5 条是管道测试数据，不代表真实热词，不代表真实来源或授权。来源名统一为「测试样例」，URL 统一使用不可访问的 `example.invalid`。

TSV 与 JSON 两份内容相同；字段为 `text`、`pinyin`、`definition`、`sourceName`、`sourceUrl`、`license`、`domain`、`expiresDays`，可选 `confidenceMillis`（默认 600）。拼音仅用无声调小写字母、空格和撇号，词包及词条默认有效 30 天。

```sh
ZHIZI_HOTWORD_KEY_FILE=src-tauri/tests/fixtures/hotword_test_seed.hex python3 scripts/hotword/build_pack.py build scripts/hotword/sample/test-input.tsv --output-dir /private/tmp/hotword-test-pack --base-url https://example.invalid/hotword-test --generated-at 2026-11-08T00:00:00Z --public-key-file src-tauri/tests/fixtures/hotword_test_pubkey.hex
python3 scripts/hotword/build_pack.py --verify /private/tmp/hotword-test-pack/hotword-20261108.json --public-key-file src-tauri/tests/fixtures/hotword_test_pubkey.hex --now 2026-11-08T00:00:00Z
python3 -m unittest scripts.hotword.test_build_pack -v
```

脚本只写本地文件，不下载、不上传。签名对象为递归 key 排序、无多余空白、直接 UTF-8、末尾恰好一个换行的 JSON；不接受重复 key 或小数。签名是 128 个十六进制字符，文件末尾带一个换行。SHA-256 对正文文件的原始字节计算。

生产打包时不传 `--public-key-file`，脚本使用编译进客户端的生产公钥核对私钥是否匹配。私钥仅从规定环境变量或默认的仓库外位置读取，错误消息不回显内容或路径。此目录及 fixtures 中的密钥仅用于测试，客户端生产路径不接受测试公钥。不要用测试密钥发布词包。
