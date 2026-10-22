import type { Mode, ProviderSettings, UserRule } from "../domain/types";
import { composePrompt } from "./promptComposer";
import { generateWithProvider, isProviderReady } from "./providerClient";

export type GenerationSource = "local" | "model" | "fallback";

type GeneratePromptInput = {
  rawInput: string;
  mode: Mode;
  userRules: UserRule[];
  provider: ProviderSettings;
  compact?: boolean;
  payload: string;
};

type ProviderGenerator = typeof generateWithProvider;

export type GenerationResult = {
  output: string;
  source: GenerationSource;
  notice: string;
};

function shortError(error: unknown): string {
  const message = error instanceof Error ? error.message : String(error || "未知错误");
  const trimmed = message.trim();
  return trimmed.length <= 120 ? trimmed : `${trimmed.slice(0, 120)}...`;
}

export async function generatePromptWithFallback(
  input: GeneratePromptInput,
  providerGenerator: ProviderGenerator = generateWithProvider,
): Promise<GenerationResult> {
  const compact = input.compact ?? false;
  const localOutput = composePrompt({
    rawInput: input.rawInput,
    mode: input.mode,
    userRules: input.userRules,
    compact,
  });

  if (!isProviderReady(input.provider)) {
    return {
      output: localOutput,
      source: "local",
      notice: input.provider.enabled
        ? "Provider 配置不完整，已使用本地模板。"
        : "已使用本地模板生成。",
    };
  }

  try {
    const providerOutput = (
      await providerGenerator({
        payload: input.payload,
        provider: input.provider,
      })
    ).trim();

    if (providerOutput) {
      return {
        output: providerOutput,
        source: "model",
        notice: "已使用模型增强优化。",
      };
    }

    return {
      output: localOutput,
      source: "fallback",
      notice: "模型返回为空，已使用本地模板。",
    };
  } catch (error) {
    return {
      output: localOutput,
      source: "fallback",
      notice: `模型调用失败，已使用本地模板：${shortError(error)}`,
    };
  }
}
