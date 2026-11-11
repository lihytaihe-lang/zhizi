# 公开测试夹具

`test-only-signing.key` 是固定、公开、可预测的 **TEST ONLY** Ed25519 种子；
`public-key.hex` 与生产公钥不同，绝不能用于生产。生成程序从不读取生产私钥。
所有 HTTPS 地址均使用不提供真实托管的 `example.invalid`。

在仓库根目录运行以下命令即可通过实际发布脚本重建全部产物：

```sh
python3 src-tauri/tests/fixtures/skin-store/generate.py
python3 scripts/skins/build_skin_index.py --verify src-tauri/tests/fixtures/skin-store/skins-index.json --test-public-key-file src-tauri/tests/fixtures/skin-store/public-key.hex
```

`teststore/skin.json` 来自 `native/resources/Skins/Builtin/system/skin.json`，
只在测试夹具中修改 id，并显式删去当前 P1 安装器拒绝的 `decorations` 与
值为 null 的 `panelBorderWidth`。它通过 `build_skin_bundle.py` 实际打包，
用于 Rust 验签、mock 下载和现有安装器的互通测试。

`testfull/` 保留内置目录的 PNG 与完整样式，只把 id 改成 `testfull`，
由同一脚本产出 `full-system-1.0.0.zhskin.json`。该包符合商店传输格式，
但当前基线的 P1 安装器不支持素材文件及 decorations，必须拒绝安装。
生产打包脚本不会静默删减皮肤字段或素材。

人工商品清单采用索引字段，额外填写相对配置文件的 `bundlePath` 和
`previewPath`；发布脚本据此计算 `bundleBytes`、`bundleSha256` 和
`previewSha256`，输出中去除本地路径。签名规范化字节按 key 排序、紧凑
UTF-8 JSON、末尾单换行；私钥仅从约定环境变量或默认外部路径读取。

内置皮肤没有发布版本，打包命令支持显式 `--version`；省略时优先使用
`skin.json.version`，没有该字段则默认 `1.0.0`，只写入包信封。
