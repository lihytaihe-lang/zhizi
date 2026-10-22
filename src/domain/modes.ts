import type { Mode, UserRule } from "./types";

export const BUILT_IN_MODES: Mode[] = [
  {
    id: "codex-execution",
    name: "Codex 执行",
    description: "把功能想法改写成边界清晰的代码实现任务。",
    category: "development",
    actionLabel: "生成开发任务",
    appliesDevGuardrails: true,
  },
  {
    id: "bug-fix",
    name: "Bug 修复",
    description: "把一句故障描述改写成可定位、可验证的排查任务。",
    category: "development",
    actionLabel: "生成排查任务",
    appliesDevGuardrails: true,
  },
  {
    id: "code-review",
    name: "代码审查",
    description: "让 AI 按风险优先级审查代码，而不是泛泛建议。",
    category: "development",
    actionLabel: "生成审查任务",
    appliesDevGuardrails: false,
  },
  {
    id: "chatgpt-discussion",
    name: "ChatGPT 讨论",
    description: "把方向或想法整理成高质量讨论问题。",
    category: "discussion",
    actionLabel: "生成讨论提示",
    appliesDevGuardrails: false,
  },
  {
    id: "product-planning",
    name: "产品规划",
    description: "把产品想法拆成定位、用户、MVP、路线图和风险。",
    category: "planning",
    actionLabel: "生成产品规划",
    appliesDevGuardrails: false,
  },
  {
    id: "ui-design-prompt",
    name: "UI 设计提示词",
    description: "把页面想法转成可给图像或设计工具的 UI brief。",
    category: "design",
    actionLabel: "生成设计提示",
    appliesDevGuardrails: false,
  },
  {
    id: "rewrite",
    name: "改写",
    description: "保留原意，改善表达、结构与可读性。",
    category: "writing",
    actionLabel: "改写原文",
    appliesDevGuardrails: false,
  },
  {
    id: "summarize",
    name: "总结",
    description: "提炼原文事实、结论、行动项与未决问题。",
    category: "writing",
    actionLabel: "总结原文",
    appliesDevGuardrails: false,
  },
  {
    id: "translate",
    name: "翻译",
    description: "按原文语言自动中英互译，保留术语和格式。",
    category: "writing",
    actionLabel: "翻译原文",
    appliesDevGuardrails: false,
  },
  {
    id: "hotword-candidates",
    // 名称从「热词候选生成」改为本名（2026-07-25）：这个动作产出的是围绕 owner 给定
    // 主题生成的个人词/术语，不是互联网热词热梗——后者需要在线词源，见
    // docs/current/tasks/HOTWORD-SOURCE-SERVER.md。id 保持不变以免影响已落库的记录。
    name: "个人词/术语生成",
    description: "围绕 owner 明确给出的主题生成带拼音和简短释义的个人词/术语，进审核队列。不是互联网热词热梗。",
    category: "lexicon",
    actionLabel: "生成待审词条",
    appliesDevGuardrails: false,
  },
];

export const DEFAULT_USER_RULES: UserRule[] = [
  {
    id: "mvp-first",
    text: "偏好 MVP 先行，先做可验证闭环，再扩展复杂功能。",
    enabled: true,
  },
  {
    id: "no-heavy-deps",
    text: "不要随意引入重型依赖；如确有必要，先说明原因。",
    enabled: true,
  },
  {
    id: "tests-required",
    text: "开发任务需要包含运行方式和验证方式。",
    enabled: true,
  },
];
