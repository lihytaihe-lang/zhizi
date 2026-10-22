import { isTauriRuntime } from "./tauriRuntime";

export type ImePreferences = { typoCorrectionEnabled: boolean; fuzzyMask: number };

/** The menu's fuzzy-pinyin pairs, bit by bit (AgentInputController.fuzzyLabels). */
export const FUZZY_PAIRS = ["zh/z", "ch/c", "sh/s", "n/l", "f/h", "an/ang", "en/eng", "in/ing"] as const;

async function invokeIme<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  if (!isTauriRuntime()) {
    throw new Error("此功能需要在知字桌面管理台中使用。");
  }
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<T>(command, args);
}

export const imePreferences = () => invokeIme<ImePreferences>("ime_preferences");
export const setImeTypoCorrection = (enabled: boolean) =>
  invokeIme<ImePreferences>("set_ime_typo_correction", { enabled });
export const setImeFuzzyMask = (mask: number) => invokeIme<ImePreferences>("set_ime_fuzzy_mask", { mask });
