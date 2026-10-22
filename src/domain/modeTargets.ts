import type { ModeId } from "./types";

export function getModeTargetTool(modeId: ModeId): string {
  switch (modeId) {
    case "codex-execution":
    case "bug-fix":
    case "code-review":
      return "Codex";
    case "ui-design-prompt":
      return "ChatGPT / 设计工具";
    case "chatgpt-discussion":
    case "product-planning":
      return "ChatGPT";
    case "rewrite":
    case "summarize":
    case "translate":
    case "hotword-candidates":
      return "当前 App";
  }
}
