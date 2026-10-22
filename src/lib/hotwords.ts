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
