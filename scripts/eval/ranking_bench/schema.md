# 排序评测回放格式

产物为 UTF-8 JSONL，每行一个用例；相同用例在不同版本中的 `id` 必须保持不变。
字段可扩展，以下字段必需。排序、目标与模型观测均对应最后一个输入键之后的状态。

| 字段 | 类型 | 语义 |
|---|---|---|
| `id` | 非空字符串 | 数据集内唯一的稳定 ID；对比按 ID 配对，不按行号 |
| `bucket` | 字符串 | `full` 全拼、`abbrev` 简拼、`mixed` 混拼、`typo` 手误、`english` 英文、`name` 姓名；由数据集标注，不按键长猜测 |
| `raw` | 非空字符串 | 原始输入键序；当前两个示例只支持至少两位小写 ASCII 字母，编排层拒绝其余输入，避免示例静默过滤 |
| `context` | 字符串 | 上文，允许空串；编排层不截断、不改写 |
| `target` | 非空字符串 | 清洗后的目标文本 |
| `candidates` | 字符串数组 | 最终排序前 60 个候选的纯文本（不足 60 则全列）；保持原顺序，不去重 |
| `rank_all` | 整数 | 目标在展平候选池中的首次位置，**从 0 开始**；`-1` 表示池内未找到。大于等于 60 仍算池内，不得以 `candidates` 截断后的查找结果覆盖它 |
| `model_ready` | 布尔值 | 最终输入有至少一组有效模型评分返回；pending、不可用、没有评分任务、所有评分失败均为 false。不能用“outcome 不是 pending”代替 |
| `model_ms` | 非负数或 null | 最终输入模型 worker 的评分耗时（毫秒，包含 composition/group 两组评分），不是整例墙钟耗时，也不是等待窗；未返回为 null，无模型为 0 |
| `source` | 字符串 | `ext` 外部集、`owner_pre` 事件前口径、`friend` 朋友验证集 |

`rank_all` 覆盖当前页及 `peek_following_pages(60)` 能返回的后续页，沿用原 pool 工具的观测上限；
不是对无界生成空间的声明。池内率为 `rank_all >= 0` 的比例；首位为 `rank_all == 0`；
Top5 为 `0 <= rank_all < 5`；MRR 对池内例取 `1 / (rank_all + 1)`，池外例计 0。
保留示例旧字段（如 `texts/scores/edits/gtexts/gscores/ghomo/outcome`），方便在已有评分上离线分析；
这些字段只覆盖示例原来实际评分的集合，不是每个候选的完整特征或统一打分器。

合成示例：

```json
{"id":"synth-abbrev-1","bucket":"abbrev","raw":"nh","context":"见面先说","target":"你好","candidates":["你好","你会"],"rank_all":0,"model_ready":true,"model_ms":8.5,"source":"ext"}
```

## 编排输入和运行

输入也是 JSONL，只需提供 `id,bucket,raw,context,target,source`，同一批输入可同时跑两种模式。
两个示例原有去重行为不变；编排层按 `(raw, context, target)` 生成唯一事件，
再把结果映射回所有原始 ID，既不丢重复事件，也不随并发数改变样本数量。输入顺序决定输出顺序。
短输入、不支持的键、缺失结果、重复结果或坏 JSON 都直接报错，不静默跳过。

```sh
python3 scripts/eval/ranking_bench/run_bench.py \
  --cases /path/to/cases.jsonl --shared-dir /path/to/shared \
  --blank-user-dir /path/to/blank-profile --work-dir /path/to/new-run \
  --mode both --jobs 3 \
  --pool-bin /path/to/examples/pool --replay-bin /path/to/examples/replay
```

`--work-dir` 必须不存在，防止混入旧结果。每个模式、每个分片启动一个进程，
各自从 `--blank-user-dir` 复制画像并仅写自己的副本；模板应由调用方准备并确认无个人学习记录。
两个模式顺序执行，总并发限制为 1–3，默认 1。不编译、不安装依赖、不自动找数据或模型。
进程数不同可能带来不同调度时延，是否逐条一致仍需在真实环境复跑验收。

`ZHIZI_MIN_KEYS` 固定为 2；等待窗优先级为 `--wait-ms` > `ZHIZI_WAIT_MS` > 3000 毫秒。
模型路径只取 `--ort/--model/--itos` 或 `ZHIZI_ORT/ZHIZI_MODEL/ZHIZI_ITOS` 环境变量，
编排脚本不读这些文件、不把路径写入报告。只有 replay 子进程加载模型；pool 子进程不带这三个变量。
诊断输出保存在运行目录，不复制到提交证据中。原始用例和回放产物可能包含私有上文，应留在调用方指定的本地目录。

输出为 `pool.jsonl`、`replay.jsonl`（依模式选择）及对应 `.report.txt`。
replay 报告第一行显示最终输入的模型就绪率（分子/分母）与「无评分任务」条数及占比：
低于 98% 时该行 ANSI 红色并返回非零退出码；产物仍保留供排查。空集不可作为通过证据。
pool 明示不适用模型门，不能混入 replay 的分母。

门的分母是「应该有模型评分的用例」：`outcome` 为 `composed/unchanged/reordered`，或 `model_ready=true`。
`outcome=no_group` 且 `model_ready=false` 表示该输入没有需要模型打分的组，单列为「无评分任务」，
报告给出数量与占比，但不进分母、也不拉低就绪率；`model_ready=true` 的 `no_group` 仍按就绪计入分母与分子。
`outcome` 为 `pending/unavailable` 等其余取值一律算未就绪，照常进分母。未评分的英文/姓名同样计入未就绪。
没有 `outcome` 字段的旧产物保持旧口径：全部用例计入分母，未就绪照常拉低就绪率，不因缺字段而放宽。

`owner_pre` 仅为来源标签，本工具按每进程空白画像回放，**不恢复事件前学习快照**；
需要该口径时应由 Claude 使用 `learnreplay.rs` 另行验收，不能把空白画像结果当事件前快照结果。

## 离线报告与对比

```sh
python3 scripts/eval/ranking_bench/report.py /path/to/replay.jsonl
python3 scripts/eval/ranking_bench/report.py /path/to/new.jsonl --compare /path/to/old.jsonl
python3 scripts/eval/ranking_bench/report.py /path/to/pool.jsonl --no-model
```

可用 `--weights /path/to/weights.json` 替换同目录默认权重。默认全拼 50%、简拼 25%、混拼 10%、
手误 10%，英文/姓名合计 5%，暂各分 2.5%。每桶先求均值，再按桶权重计算四项指标的加权总分，
不是按日志条数加权。缺桶会列出缺失桶及权重覆盖率，并只在有样本桶间归一化；
这种分数仅适用于该子集，不能当作六桶完整评测。

对比要求两份产物的 ID 集及每例 `bucket/raw/context/target/source` 一致。
“修好”指旧首位错、新首位对；“弄坏”反之，同时分桶列出；排名变好但未到首位不算修好。
对比报告必须同时检查新旧产物的模型门，不能用未就绪的基线证明收益。

本任务仅用合成数据验证工具，不提供真实基线、逐条复跑一致性、事件前快照、位置稳定性、
会话聚类区间、逐键 p95 或 15ms 出结果率；这些仍由后续工作验收。
