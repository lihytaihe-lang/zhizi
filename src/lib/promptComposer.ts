import type { ComposeInput } from "../domain/types";

const devGuardrails = [
  "请先阅读项目结构和相关文件，确认现有技术栈、路由方式、组件规范和代码风格。",
  "先给出简短实现计划，再开始修改。",
  "只修改完成任务所必需的文件，不要重构无关模块。",
  "不要引入新的重型依赖；如果必须引入，请先说明理由和替代方案。",
  "保持现有代码风格和交互习惯。",
  "完成后请列出修改文件、运行方式、测试方式、风险和待确认项。",
];

const compactDevGuardrails = [
  "先读相关文件并说明关键假设。",
  "只做完成目标所需的最小改动。",
  "不引入重型依赖，除非先说明理由。",
  "完成后给出修改文件、运行方式和验证结果。",
];

function numbered(items: string[]): string {
  return items.map((item, index) => `${index + 1}. ${item}`).join("\n");
}

function cleanLine(text: string): string {
  return text.trim().replace(/[。；;,\s]+$/u, "");
}

function userRulesBlock(userRules: ComposeInput["userRules"]): string {
  const enabledRules = userRules.filter((rule) => rule.enabled && rule.text.trim());

  if (enabledRules.length === 0) {
    return "";
  }

  return `\n【用户长期偏好】\n${numbered(enabledRules.map((rule) => rule.text.trim()))}\n`;
}

function compactRulesLine(userRules: ComposeInput["userRules"]): string {
  const enabledRules = userRules
    .filter((rule) => rule.enabled && rule.text.trim())
    .slice(0, 3)
    .map((rule) => cleanLine(rule.text));

  return enabledRules.length > 0 ? `\n长期偏好：${enabledRules.join("；")}。` : "";
}

function devGuardrailsBlock(appliesDevGuardrails: boolean): string {
  if (!appliesDevGuardrails) {
    return "";
  }

  return `\n【执行边界】\n${numbered(devGuardrails)}\n`;
}

function compactDevGuardrailsBlock(appliesDevGuardrails: boolean): string {
  if (!appliesDevGuardrails) {
    return "";
  }

  return `\n执行边界：\n${numbered(compactDevGuardrails)}`;
}

function composeCompactPrompt({ rawInput, mode, userRules }: ComposeInput): string {
  const idea = rawInput.trim();
  const rules = compactRulesLine(userRules);
  const guardrails = compactDevGuardrailsBlock(mode.appliesDevGuardrails);

  switch (mode.id) {
    case "chatgpt-discussion":
      return `请围绕这个想法做一次高质量讨论：${idea}${rules}\n\n请输出：\n1. 核心用户和最痛场景；\n2. MVP 应做/不做；\n3. 关键假设、风险和反例；\n4. 下一步最小验证行动。`;

    case "codex-execution":
      return `请在当前项目完成：${idea}${rules}${guardrails}\n\n验收：\n1. 功能可本地运行并验证；\n2. 不破坏现有功能；\n3. 回复修改文件、验证方式、结果和遗留风险。`;

    case "product-planning":
      return `请把这个产品想法整理成可执行规划：${idea}${rules}\n\n请输出：\n1. 定位和目标用户；\n2. 核心场景；\n3. MVP 范围和明确不做；\n4. 功能优先级；\n5. 风险、验证方式和下一步开发任务。`;

    case "bug-fix":
      return `请排查并修复：${idea}${rules}${guardrails}\n\n请先说明最可能原因和相关文件，再修根因。完成后回复：根因、修改点、验证方式、结果和遗留风险。`;

    case "code-review":
      return `请按风险优先审查当前代码：${idea}${rules}\n\n只列真实问题，按严重程度排序。每条包含：文件/位置、原因、影响、建议和是否需要测试。没有问题请直接说明。`;

    case "ui-design-prompt":
      return `请生成高保真 UI 设计 brief：${idea}${rules}\n\n要求：真实工具界面，桌面端优先，信息层级清晰，包含核心工作流、导航、主操作区、状态反馈和空/成功状态；视觉克制专业，不做营销页。`;

    case "rewrite":
      return `请改写以下原文，保留事实、语气和原意，改善表达、结构与可读性：\n\n${idea}${rules}\n\n只输出改写后的正文，不补充原文没有的信息。`;

    case "summarize":
      return `请总结以下原文：\n\n${idea}${rules}\n\n按“核心结论、关键事实、行动项、未决问题”组织；没有的部分省略，不推测。`;

    case "translate":
      return `请翻译以下原文：中文自动译为英文，其他语言自动译为中文；保留专有名词、代码、路径、数字和原有格式：\n\n${idea}${rules}\n\n只输出译文。`;

    case "hotword-candidates":
      return `请围绕这个主题生成待审热词：${idea}${rules}\n\n每行严格使用“词<TAB>拼音<TAB>简短释义”；拼音使用空格分隔的无声调小写音节。不要标题、序号、Markdown 或解释。\n\n本地手填模板：\n<词>\t<拼音>\t<简短释义>`;
  }
}

export function composePrompt({ rawInput, mode, userRules, compact = false }: ComposeInput): string {
  if (compact) {
    return composeCompactPrompt({ rawInput, mode, userRules });
  }

  const idea = rawInput.trim();
  const rules = userRulesBlock(userRules);
  const guardrails = devGuardrailsBlock(mode.appliesDevGuardrails);

  switch (mode.id) {
    case "chatgpt-discussion":
      return `我想讨论一个问题/方向，请帮我把它分析清楚。\n\n【我的初步想法】\n${idea}\n${rules}\n【希望你帮我分析】\n1. 这个方向的核心用户是谁；\n2. 用户最痛的 3 个场景是什么；\n3. MVP 应该做什么，不应该做什么；\n4. 可选方案之间的取舍；\n5. 如何验证这个方向是否成立；\n6. 下一步最小行动是什么。\n\n【回答要求】\n- 不要泛泛而谈；\n- 明确哪些是假设，哪些需要验证；\n- 给出可落地建议；\n- 必要时指出风险和反例。`;

    case "codex-execution":
      return `请在当前项目中完成以下任务。\n\n【任务目标】\n${idea}\n${rules}${guardrails}\n【需求范围】\n1. 基于用户目标补齐必要但不过度扩张的功能细节；\n2. 优先实现最小可用闭环；\n3. 保持现有项目视觉、架构和代码风格；\n4. 如需求不明确，请先列出你的假设。\n\n【验收标准】\n1. 功能可以在本地运行并被验证；\n2. 不破坏现有功能；\n3. 关键路径有必要测试或清晰的手动验证步骤；\n4. 输出中说明修改文件、运行方式、验证结果和遗留风险。`;

    case "product-planning":
      return `请把下面这个产品想法整理成可执行的产品规划。\n\n【产品想法】\n${idea}\n${rules}\n【请输出】\n1. 产品定位；\n2. 目标用户；\n3. 核心使用场景；\n4. MVP 范围；\n5. 明确不做什么；\n6. 核心功能列表；\n7. 数据结构或模块划分；\n8. 版本路线图；\n9. 主要风险和规避方式；\n10. 下一步可以交给 Codex 执行的开发任务。\n\n【要求】\n- 以可落地为主；\n- 明确 MVP 边界；\n- 不要写成空泛商业分析；\n- 对不确定点标注假设。`;

    case "bug-fix":
      return `请排查并修复以下问题。\n\n【问题描述】\n${idea}\n${rules}${guardrails}\n【排查重点】\n1. 复现路径和实际现象；\n2. 相关状态、路由、接口、缓存或异步逻辑；\n3. 最近可能影响该问题的代码路径；\n4. 是否存在错误处理、边界状态或时序问题；\n5. 是否需要添加回归测试。\n\n【执行要求】\n1. 先列出可能原因；\n2. 再定位最可能的文件；\n3. 只修复根因，不做大范围重构；\n4. 完成后说明根因、修改点、验证方式和遗留风险。`;

    case "code-review":
      return `请对当前代码进行审查。\n\n【审查目标】\n${idea}\n${rules}\n【重点关注】\n1. 明显 bug 或行为回归；\n2. 安全和隐私风险；\n3. 性能问题；\n4. 过度复杂或难维护的实现；\n5. 与现有架构、代码风格不一致的地方；\n6. 缺少测试或验收路径。\n\n【输出要求】\n- 按严重程度排序；\n- 每个问题给出文件/位置、原因和建议；\n- 只输出真实风险，不要为了凑数泛泛建议；\n- 不要直接改代码，除非我明确要求。`;

    case "ui-design-prompt":
      return `请生成一张高保真 UI 设计图/原型图。\n\n【页面或产品想法】\n${idea}\n${rules}\n【设计要求】\n1. 做真实可用的产品界面，不要做营销落地页；\n2. 信息层级清晰，优先展示核心工作流；\n3. 使用克制、专业、适合工具型产品的视觉风格；\n4. 包含必要的导航、主操作区、状态反馈和空/成功状态；\n5. 信息密度高但不拥挤；\n6. 不要大面积装饰性渐变，不要赛博朋克，不要游戏风；\n7. 控件和文字必须清晰可读。\n\n【输出格式】\n- 桌面端优先；\n- 16:9；\n- 高保真 UI mockup；\n- 如果需要补充假设，请让假设服务于 MVP。`;

    case "rewrite":
      return `请改写下面的原文。\n\n【原文】\n${idea}\n${rules}\n【要求】\n1. 保留事实、原意和语气；\n2. 改善结构、准确性和可读性；\n3. 不添加原文没有的信息；\n4. 只输出改写后的正文。`;

    case "summarize":
      return `请总结下面的原文。\n\n【原文】\n${idea}\n${rules}\n【输出】\n1. 核心结论；\n2. 关键事实；\n3. 行动项；\n4. 未决问题。\n\n没有的部分省略，不推测、不添加原文之外的信息。`;

    case "translate":
      return `请翻译下面的原文。中文自动译为英文，其他语言自动译为中文。\n\n【原文】\n${idea}\n${rules}\n【要求】\n1. 保留专有名词、代码、路径、数字和原有格式；\n2. 语义准确、表达自然；\n3. 不添加解释；\n4. 只输出译文。`;

    case "hotword-candidates":
      return `请围绕 owner 明确给出的主题生成一组可供人工审核的中文热词候选。\n\n【主题】\n${idea}\n${rules}\n【唯一输出格式】\n每行严格输出：词<TAB>拼音<TAB>简短释义\n\n【要求】\n1. 拼音由本次生成一并给出，使用空格分隔的无声调小写音节；\n2. 不抓取、引用或声称来自任何站点；\n3. 不输出标题、序号、Markdown 表格、代码围栏或解释；\n4. 一行只放一个候选。\n\n【无 Provider 时的本地手填模板】\n<词>\t<拼音>\t<简短释义>`;
  }
}
