# 上下文排序模型

知字可选的 30M Core ML 模型在本地重排候选。模型单独发布于 https://huggingface.co/DabbleCove/shunshou-zhizi-30m ，按 CC BY-SA 4.0 授权。不获取模型也能使用输入法，候选按词库排序。运行时推理不联网。

## 构建时三档

`ZHIZI_MODEL_TIER` 在构建时选择模型，设置界面不能选择模型。缺省为 `official`。

| 档位 | 适用人群 | 校验 |
| --- | --- | --- |
| `official` | 拥有精确发布产物的维护者 | 核对 `native/resources/ContextModel/SHA256SUMS` 全部七行哈希；缺省行为不变。 |
| `public` | 下载公开模型后从源码构建的用户 | `SHA256SUMS.public` 钉住 itos.json 及编译目录内 metadata.json、model.mil、weights/weight.bin、analytics/coremldata.bin。仓库模型卡做结构检查并拷入。顶层 coremldata.bin 只查存在、非符号链接、大小 256 B–64 KiB，不钉哈希。 |
| `custom` | 提供自己模型的开发者 | 不校验哈希。检查必需文件、源路径及目录树全程无符号链接、词表为无重复字符串数组且保留前三项、总大小不超过 512 MB。检查并拷入仓库模型卡。Core ML 接口由运行时检查，Python 不加载模型。 |

公开编译会改变顶层 coremldata.bin 内的转换日期和 UUID 元数据；权重与图结构仍逐字节校验。仓库 model_card.json 与正式私有发布副本也不同，public/custom 使用仓库副本。public 成功的 availability 记录含 `tier: public, official: true`，表示钉住的核心产物一致；custom 含 `official: false`。official 记录保持原格式。管理台把 public 档构建显示为非官方构建的独立标识**暂未提供**。

实测顶层 coremldata.bin：三次本机编译与维护者钉住副本均为 372 字节。public 档边界 256 B–64 KiB 是宽松的合理性界限，并非完整性校验；权重与图结构仍由哈希钉住。

## 命令（macOS）

`script/build_native.sh` 第 100 行调用拷贝脚本，并继承这些环境变量。默认源为 `$ZHIZI_WORK/release/zhizi-30m-v3`；未设置 `ZHIZI_WORK` 时为仓库内 `ime_context_model/release/zhizi-30m-v3`。下载需要网络及 Core ML 编译器；运行时不需要网络。

```sh
# 维护者提供的精确正式发布源
ZHIZI_MODEL_TIER=official ZHIZI_WORK=/path/to/work bash script/build_native.sh debug

# 公开源码构建；fetch 拒绝覆盖已有非空目标
python3 scripts/context_model/fetch_public_model.py
ZHIZI_MODEL_TIER=public bash script/build_native.sh debug

# 自定义编译模型
ZHIZI_MODEL_TIER=custom ZHIZI_MODEL_DIR=/path/to/custom-model bash script/build_native.sh debug
```

以下是构建说明；本文档没有声称某个产品构建已按此测试。缺失或不合格模型会打印原因与 `SKIP`、写入 `available: false`、退出码仍为 0，输入法可不带模型排序继续构建。custom 成功打印 `WARN ... unofficial-model`，非法档位也跳过。

## 自定义接口

目录内必须有 `itos.json` 与 Core ML 编译目录 `zhizi30m_v2.mlmodelc/`（不是 mlpackage）。编译目录必需文件为 metadata.json、model.mil、coremldata.bin、weights/weight.bin、analytics/coremldata.bin。词表为无重复字符串数组，前三项依次 `<pad>`、`<bos>`、`<unk>`，索引 pad=0、bos=1、unk=2。运行时要求输入 `tokens` Int32[8,40]，输出 `logprobs` [8,39]，分数必须有限。不符合时该会话回退词库排序，不会崩溃。Python 不加载 Core ML 检查形状。拷入的仓库模型卡描述预期接口，不是自定义模型来源的独立证明。
