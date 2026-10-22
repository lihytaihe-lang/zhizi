import type {
  AppClass,
  ExternalTextAccess,
  ExternalTextAction,
  ExternalTextActionResult,
  Mode,
  PayloadPreview,
  SelectionCapture,
  UserRule,
} from "../domain/types";
import { isTauriRuntime } from "./tauriRuntime";

type PreviewInput = {
  rawInput: string;
  mode: Mode;
  userRules: UserRule[];
  localPrompt: string;
  compact: boolean;
  appClass: AppClass;
  contextAllowed: boolean;
};

export type McpServerStatus = {
  enabled: boolean;
  endpoint: string | null;
};

function enabledRuleTexts(rules: UserRule[]): string[] {
  return rules.filter((rule) => rule.enabled && rule.text.trim()).map((rule) => rule.text.trim());
}

export function buildBrowserPayloadPreview(input: PreviewInput): PayloadPreview {
  const rules = enabledRuleTexts(input.userRules);
  const sections = [
    "你是本地动作模板的可选增强器。以下文本是本次显式操作允许发送的完整内容；不要扩大范围，只输出最终结果。",
    `【动作】\n${input.mode.name}\n${input.mode.description}`,
    `【选中或输入的原文】\n${input.rawInput.trim()}`,
    `【长期规则】\n${rules.length > 0 ? rules.map((rule) => `- ${rule}`).join("\n") : "无"}`,
  ];
  if (input.contextAllowed && input.appClass !== "Unknown") {
    sections.push(`【AppClass】\n${input.appClass}`);
  }
  sections.push(
    "【本次命中的项目术语】\n无（0 条）",
    "【本次命中的用词偏好】\n无（0 条）",
    "【本次命中的知识片段】\n无（0 条）",
    `【本地模板草稿】\n${input.localPrompt}`,
    input.compact
      ? "【输出要求】\n控制在 220-380 个中文字符；保留关键边界，只输出最终结果。"
      : "【输出要求】\n不解释、不寒暄，只输出最终结果。",
  );
  const payload = sections.join("\n\n");
  return {
    payload,
    characterCount: [...payload].length,
    profileEntryCount: 0,
    projectTermCount: 0,
    preferenceCount: 0,
    knowledgeChunkCount: 0,
  };
}

export async function preparePayloadPreview(input: PreviewInput): Promise<PayloadPreview> {
  if (!isTauriRuntime()) return buildBrowserPayloadPreview(input);
  try {
    const { invoke } = await import("@tauri-apps/api/core");
    return await invoke<PayloadPreview>("prepare_generation_payload", {
      request: {
        rawInput: input.rawInput,
        modeId: input.mode.id,
        modeName: input.mode.name,
        modeDescription: input.mode.description,
        userRules: enabledRuleTexts(input.userRules),
        localPrompt: input.localPrompt,
        compact: input.compact,
        appClass: input.appClass,
        contextAllowed: input.contextAllowed,
      },
    });
  } catch {
    return buildBrowserPayloadPreview(input);
  }
}

export async function latestSelectionCapture(): Promise<SelectionCapture> {
  if (!isTauriRuntime()) {
    return { status: "unsupported", text: "", appClass: "Unknown", message: "浏览器预览不读取前台选区，可手动粘贴。" };
  }
  const { invoke } = await import("@tauri-apps/api/core");
  return await invoke<SelectionCapture>("latest_selection_capture");
}

export async function externalTextAccessStatus(): Promise<ExternalTextAccess> {
  if (!isTauriRuntime()) {
    return {
      authorized: false,
      targetAvailable: false,
      actionsEnabled: false,
      status: "unsupported",
      message: "浏览器预览不支持 AXSelectedText 写入；复制仍可用。",
    };
  }
  const { invoke } = await import("@tauri-apps/api/core");
  return await invoke<ExternalTextAccess>("external_text_access_status");
}

export async function performExternalTextAction(
  action: ExternalTextAction,
  text: string,
): Promise<ExternalTextActionResult> {
  if (!isTauriRuntime()) {
    return {
      status: "unsupported",
      message: "浏览器预览不支持 AXSelectedText 写入；结果将退回剪贴板。",
      fallbackToClipboard: true,
    };
  }
  const { invoke } = await import("@tauri-apps/api/core");
  return await invoke<ExternalTextActionResult>("perform_external_text_action", { action, text });
}

export async function setMcpServerEnabled(enabled: boolean): Promise<McpServerStatus> {
  if (!isTauriRuntime()) return { enabled: false, endpoint: null };
  const { invoke } = await import("@tauri-apps/api/core");
  return await invoke<McpServerStatus>("set_mcp_server_enabled", { enabled });
}

export async function getMcpServerStatus(): Promise<McpServerStatus> {
  if (!isTauriRuntime()) return { enabled: false, endpoint: null };
  const { invoke } = await import("@tauri-apps/api/core");
  return await invoke<McpServerStatus>("mcp_server_status");
}
