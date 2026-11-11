import { isTauriRuntime } from "./tauriRuntime";

export type DynamicSourceInput = {
  id: string;
  displayName: string;
  sourceType: "official_json" | "official_xml" | "local_text";
  refreshIntervalMinutes: number;
  authorization: string;
  termsUrl: string;
  license: string;
  cachePolicy: string;
  ownership: "bundled" | "user_local" | "remote_cached" | "self-authored";
  distribution: string;
  disablePolicy: string;
  endpoint: string;
  optIn: boolean;
  updatedAtMs: number;
};

export type DynamicSource = DynamicSourceInput & {
  enabled: boolean;
  lastRefreshMs: number | null;
  lastError: string | null;
  createdAtMs: number;
};

export type HotwordItem = {
  id: string;
  text: string;
  pinyin: string;
  confidenceMillis: number;
  freshnessMillis: number;
  domain: string;
  expiresAtMs: number | null;
  reviewRequired: boolean;
  state: "staged" | "review" | "expired" | "blocked";
  sourceIds: string[];
  exposureCount: number;
  misselectionCount: number;
  updatedAtMs: number;
  pinyinWarning?: string | null;
};

export type HotwordPackSettings = {
  url: string;
  lastPackId: string | null;
  lastUpdatedAtMs: number | null;
};

export type HotwordPackUpdateResult = {
  alreadyLatest: boolean;
  packId: string;
  receivedCount: number;
  pinyinConflictCount: number;
  message: string;
};

async function invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  const api = await import("@tauri-apps/api/core");
  return api.invoke<T>(command, args);
}

export async function listDynamicSources(): Promise<DynamicSource[]> {
  return isTauriRuntime() ? invoke("list_dynamic_sources") : [];
}

export async function saveDynamicSource(input: DynamicSourceInput): Promise<DynamicSource> {
  if (!isTauriRuntime()) {
    return { ...input, enabled: false, lastRefreshMs: null, lastError: null, createdAtMs: input.updatedAtMs };
  }
  return invoke("save_dynamic_source", { input });
}

export async function setDynamicSourceEnabled(id: string, enabled: boolean): Promise<DynamicSource> {
  return invoke("set_dynamic_source_enabled", { id, enabled });
}

export async function clearDynamicSource(id: string): Promise<number> {
  return invoke("clear_dynamic_source", { id });
}

export async function refreshDynamicSource(id: string): Promise<void> {
  await invoke("refresh_dynamic_source", { id });
}

export async function ingestLocalHotwordSource(id: string, document: string): Promise<void> {
  await invoke("ingest_local_hotword_source", { id, document });
}

export async function listHotwordItems(): Promise<HotwordItem[]> {
  return isTauriRuntime() ? invoke("list_hotword_items") : [];
}

export async function getHotwordPackSettings(): Promise<HotwordPackSettings> {
  return isTauriRuntime()
    ? invoke("get_hotword_pack_settings")
    : { url: "", lastPackId: null, lastUpdatedAtMs: null };
}

export async function saveHotwordPackUrl(url: string): Promise<HotwordPackSettings> {
  if (!isTauriRuntime()) throw new Error("请在管理台 App 中配置热词包地址。");
  return invoke("save_hotword_pack_url", { url });
}

// 仅由 owner 点击「更新热词」调用；读取设置和打开面板不会触发取包。
export async function updateHotwordPack(): Promise<HotwordPackUpdateResult> {
  if (!isTauriRuntime()) throw new Error("请在管理台 App 中更新热词。");
  return invoke("update_hotword_pack");
}

export type HotwordExposureReport = {
  consideredEvents: number;
  trackedItems: number;
  changedItems: number;
  totalExposures: number;
  totalMisselections: number;
};

/// Recomputes 曝光/误选 from the recorded quality events before the panel reads them.
/// Until 2026-07-25 nothing wrote those two columns, so the panel always showed 0.
export async function refreshHotwordExposure(): Promise<HotwordExposureReport | null> {
  return isTauriRuntime() ? invoke("refresh_hotword_exposure") : null;
}
