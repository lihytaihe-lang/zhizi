import type { ProviderKind, ProviderSettings } from "../domain/types";
import { isTauriRuntime } from "./tauriRuntime";

type GenerateWithProviderInput = {
  payload: string;
  provider: ProviderSettings;
};

export function isProviderReady(provider: ProviderSettings): boolean {
  if (!provider.enabled) return false;
  const selected = selectedProviderConfiguration(provider);
  return Boolean(
    selected.baseUrl.trim() &&
      selected.model.trim() &&
      (selected.kind === "ollama" || selected.apiKey.trim()),
  );
}

export function selectedProviderConfiguration(provider: ProviderSettings): {
  kind: ProviderKind;
  baseUrl: string;
  model: string;
  apiKey: string;
} {
  const kind = provider.providerKind ?? "openai-compatible";
  if (kind === "anthropic") {
    return {
      kind,
      baseUrl: provider.anthropicBaseUrl ?? "https://api.anthropic.com",
      model: provider.anthropicModel ?? "",
      apiKey: provider.anthropicApiKey ?? "",
    };
  }
  if (kind === "ollama") {
    return {
      kind,
      baseUrl: provider.ollamaBaseUrl ?? "http://127.0.0.1:11434",
      model: provider.ollamaModel ?? "",
      apiKey: "",
    };
  }
  return {
    kind,
    baseUrl: provider.baseUrl,
    model: provider.model,
    apiKey: provider.apiKey,
  };
}

export async function generateWithProvider({
  payload,
  provider,
}: GenerateWithProviderInput): Promise<string> {
  if (!isTauriRuntime()) {
    throw new Error("模型调用需要在桌面 App 中运行。");
  }

  const { invoke } = await import("@tauri-apps/api/core");
  const selected = selectedProviderConfiguration(provider);

  return await invoke<string>("generate_with_provider", {
    request: {
      payload,
      provider: {
        kind: selected.kind,
        baseUrl: selected.baseUrl,
        model: selected.model,
      },
    },
  });
}
