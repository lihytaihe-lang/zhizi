import { BUILT_IN_MODES, DEFAULT_USER_RULES } from "../domain/modes";
import type {
  HistoryItem,
  ModeId,
  ProviderSettings,
  ShortcutSettings,
  UserRule,
} from "../domain/types";
import { isTauriRuntime } from "./tauriRuntime";

const historyKey = "agent-input-layer.history";
const rulesKey = "agent-input-layer.user-rules";
const selectedModeKey = "agent-input-layer.selected-mode";
const providerSettingsKey = "agent-input-layer.provider-settings";
const shortcutSettingsKey = "agent-input-layer.shortcut-settings";

export type AppStorageState = {
  history: HistoryItem[];
  userRules: UserRule[];
  selectedModeId: ModeId;
  providerSettings: ProviderSettings;
  shortcutSettings: ShortcutSettings;
};

export const DEFAULT_PROVIDER_SETTINGS: ProviderSettings = {
  enabled: false,
  providerKind: "openai-compatible",
  baseUrl: "",
  model: "",
  apiKey: "",
  anthropicBaseUrl: "https://api.anthropic.com",
  anthropicModel: "",
  anthropicApiKey: "",
  ollamaBaseUrl: "http://127.0.0.1:11434",
  ollamaModel: "",
};

export const DEFAULT_SHORTCUT_SETTINGS: ShortcutSettings = {
  shortcut: "alt+space",
  mcpEnabled: false,
};

function readJson<T>(key: string, fallback: T): T {
  try {
    const raw = window.localStorage.getItem(key);
    return raw ? (JSON.parse(raw) as T) : fallback;
  } catch {
    return fallback;
  }
}

function writeJson<T>(key: string, value: T): void {
  window.localStorage.setItem(key, JSON.stringify(value));
}

function cloneDefaultRules(): UserRule[] {
  return DEFAULT_USER_RULES.map((rule) => ({ ...rule }));
}

function getDefaultSelectedMode(): ModeId {
  return BUILT_IN_MODES[0].id;
}

function normalizeSelectedMode(modeId: unknown, fallback = getDefaultSelectedMode()): ModeId {
  const exists = BUILT_IN_MODES.some((mode) => mode.id === modeId);
  return exists ? (modeId as ModeId) : fallback;
}

function normalizeHistory(history: unknown): HistoryItem[] {
  return Array.isArray(history) ? (history as HistoryItem[]).slice(0, 50) : [];
}

function normalizeRules(rules: unknown, fallback: UserRule[]): UserRule[] {
  return Array.isArray(rules) ? (rules as UserRule[]) : fallback;
}

function normalizeProviderSettings(settings: unknown): ProviderSettings {
  return {
    ...DEFAULT_PROVIDER_SETTINGS,
    ...(settings && typeof settings === "object" ? (settings as Partial<ProviderSettings>) : {}),
  };
}

function normalizeShortcutSettings(settings: unknown): ShortcutSettings {
  return {
    ...DEFAULT_SHORTCUT_SETTINGS,
    ...(settings && typeof settings === "object" ? (settings as Partial<ShortcutSettings>) : {}),
  };
}

function withoutProviderSecrets(state: AppStorageState): AppStorageState {
  return {
    ...state,
    providerSettings: {
      ...state.providerSettings,
      enabled: false,
      apiKey: "",
      anthropicApiKey: "",
    },
  };
}

function isProviderCredentialError(error: unknown): boolean {
  return /钥匙串|Provider 密钥/.test(String(error));
}

function getDefaultAppState(): AppStorageState {
  return {
    history: [],
    userRules: cloneDefaultRules(),
    selectedModeId: getDefaultSelectedMode(),
    providerSettings: DEFAULT_PROVIDER_SETTINGS,
    shortcutSettings: DEFAULT_SHORTCUT_SETTINGS,
  };
}

function normalizeAppState(
  state: Partial<AppStorageState>,
  fallback = getDefaultAppState(),
): AppStorageState {
  return {
    history: normalizeHistory(state.history),
    userRules: normalizeRules(state.userRules, fallback.userRules),
    selectedModeId: normalizeSelectedMode(state.selectedModeId, fallback.selectedModeId),
    providerSettings: normalizeProviderSettings(
      state.providerSettings ?? fallback.providerSettings,
    ),
    shortcutSettings: normalizeShortcutSettings(
      state.shortcutSettings ?? fallback.shortcutSettings,
    ),
  };
}

export function loadLocalAppState(): AppStorageState {
  const fallback = getDefaultAppState();
  const storedSelectedMode = window.localStorage.getItem(selectedModeKey);

  return normalizeAppState(
    {
      history: readJson<HistoryItem[]>(historyKey, fallback.history),
      userRules: readJson<UserRule[]>(rulesKey, fallback.userRules),
      selectedModeId: storedSelectedMode ? (storedSelectedMode as ModeId) : undefined,
      providerSettings: readJson<Partial<ProviderSettings>>(
        providerSettingsKey,
        fallback.providerSettings,
      ) as ProviderSettings,
      shortcutSettings: readJson<Partial<ShortcutSettings>>(
        shortcutSettingsKey,
        fallback.shortcutSettings,
      ) as ShortcutSettings,
    },
    fallback,
  );
}

export function saveLocalAppState(state: AppStorageState): void {
  const normalized = normalizeAppState(state);

  writeJson(historyKey, normalized.history);
  writeJson(rulesKey, normalized.userRules);
  window.localStorage.setItem(selectedModeKey, normalized.selectedModeId);
  writeJson(providerSettingsKey, normalized.providerSettings);
  writeJson(shortcutSettingsKey, normalized.shortcutSettings);
}

export async function loadAppState(fallback = loadLocalAppState()): Promise<AppStorageState> {
  if (!isTauriRuntime()) {
    return fallback;
  }

  try {
    const { invoke } = await import("@tauri-apps/api/core");
    const state = await invoke<AppStorageState>("load_app_state", { fallback });
    window.localStorage.removeItem(providerSettingsKey);
    return normalizeAppState(state, fallback);
  } catch (error) {
    if (isProviderCredentialError(error)) {
      throw new Error("无法载入本地设置：无法读取 macOS 钥匙串中的 Provider 密钥，请解锁钥匙串或重新填写。");
    }
    console.warn("Failed to load SQLite state; using secret-free localStorage fallback.", error);
    // Also drop any v0.1-era plaintext provider settings so the fallback
    // never leaves a readable key behind in localStorage.
    window.localStorage.removeItem(providerSettingsKey);
    return withoutProviderSecrets(fallback);
  }
}

export async function saveAppState(state: AppStorageState): Promise<void> {
  const normalized = normalizeAppState(state);

  if (!isTauriRuntime()) {
    // Browser previews have no Keychain; persisting raw secrets there would
    // put Provider keys in plaintext storage.
    saveLocalAppState(withoutProviderSecrets(normalized));
    return;
  }

  try {
    const { invoke } = await import("@tauri-apps/api/core");
    await invoke("save_app_state", { state: normalized });
  } catch (error) {
    if (isProviderCredentialError(error)) {
      throw new Error("无法保存本地设置：无法写入 macOS 钥匙串中的 Provider 密钥，请解锁钥匙串后重试。");
    }
    console.warn("Failed to save SQLite state; using secret-free localStorage fallback.", error);
    saveLocalAppState(withoutProviderSecrets(normalized));
  }
}

