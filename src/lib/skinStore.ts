import type { CandidateSkinCatalogSnapshot } from "./lexicon";
import { isTauriRuntime } from "./tauriRuntime";

export type SkinStoreSettings = {
  indexUrl: string;
  lastRefreshedAtMs: number | null;
};

export type StoreSkin = {
  id: string;
  name: { zh: string; en: string };
  author: string;
  description: string;
  version: string;
  tier: "free" | "paid";
  platforms?: string[];
  availableForSale?: boolean;
  productId: string;
  animated: boolean;
  bundleBytes: number;
  minAppVersion: string;
  updatedAt: string;
  license: string;
  changelog: string;
  state: "notInstalled" | "installed" | "updateAvailable" | "needsAppUpgrade" | "unsupportedPlatform";
  installedVersion: string | null;
};

export type StoreSnapshot = {
  skins: StoreSkin[];
  refreshedAtMs: number | null;
  stale: boolean;
  error: string | null;
};

async function invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  const api = await import("@tauri-apps/api/core");
  return api.invoke<T>(command, args);
}

export async function getSkinStoreSettings(): Promise<SkinStoreSettings> {
  return isTauriRuntime()
    ? invoke("skin_store_get_settings")
    : { indexUrl: "", lastRefreshedAtMs: null };
}

export async function setSkinStoreUrl(url: string): Promise<SkinStoreSettings> {
  if (!isTauriRuntime()) throw new Error("请在管理台 App 中配置皮肤商店地址。");
  return invoke("skin_store_set_url", { url });
}

// 读取缓存不联网；仅在用户点击「连接 / 刷新 / 重试」后刷新索引。
export async function listStoreSkins(): Promise<StoreSnapshot> {
  return isTauriRuntime()
    ? invoke("skin_store_list")
    : { skins: [], refreshedAtMs: null, stale: false, error: null };
}

export async function refreshSkinStore(): Promise<StoreSnapshot> {
  if (!isTauriRuntime()) throw new Error("请在管理台 App 中刷新皮肤商店。");
  return invoke("skin_store_refresh");
}

export async function getSkinStorePreview(id: string): Promise<string> {
  if (!isTauriRuntime()) throw new Error("请在管理台 App 中查看皮肤预览。");
  return invoke("skin_store_preview", { id });
}

export async function installStoreSkin(id: string): Promise<CandidateSkinCatalogSnapshot> {
  if (!isTauriRuntime()) throw new Error("请在管理台 App 中获取皮肤。");
  return invoke("skin_store_install", { id });
}
