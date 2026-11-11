import assert from "node:assert/strict";
import { test } from "node:test";
import {
  formatSkinPrice,
  getSkinStorePreview,
  getSkinStoreSettings,
  installStoreSkin,
  listStoreSkins,
  purchaseStoreSkin,
  refreshSkinStore,
  setSkinStoreUrl,
  type StoreSnapshot,
} from "../lib/skinStore";

test("skin store browser fallback reads empty local state and rejects unavailable actions", async () => {
  assert.deepEqual(await getSkinStoreSettings(), { indexUrl: "", lastRefreshedAtMs: null });
  assert.deepEqual(await listStoreSkins(), { skins: [], refreshedAtMs: null, stale: false, error: null });
  for (const action of [
    () => setSkinStoreUrl("https://example.invalid/skins"),
    refreshSkinStore,
    () => getSkinStorePreview("sample"),
    () => installStoreSkin("sample"),
    () => purchaseStoreSkin("sample"),
  ]) {
    await assert.rejects(action, /请在管理台 App 中/);
  }
});

test("skin store wrappers invoke the seven design commands with exact argument names", async () => {
  const calls: { command: string; args?: Record<string, unknown> }[] = [];
  const settings = { indexUrl: "https://example.invalid/skins", lastRefreshedAtMs: 1000 };
  const snapshot: StoreSnapshot = { skins: [], refreshedAtMs: 1000, stale: true, error: "当前无法刷新" };
  const catalog = { skins: [], issues: [] };
  const purchase = { ok: false, message: "购买功能尚未开放" };
  const runtime = window as unknown as { __TAURI_INTERNALS__?: { invoke: (command: string, args?: Record<string, unknown>) => Promise<unknown> } };
  runtime.__TAURI_INTERNALS__ = {
    invoke: async (command, args) => {
      calls.push({ command, args });
      switch (command) {
        case "skin_store_get_settings":
        case "skin_store_set_url": return settings;
        case "skin_store_list":
        case "skin_store_refresh": return snapshot;
        case "skin_store_preview": return "data:image/png;base64,dGVzdA==";
        case "skin_store_install": return catalog;
        case "skin_store_purchase": return purchase;
        default: throw new Error(`Unexpected command: ${command}`);
      }
    },
  };
  try {
    assert.deepEqual(await getSkinStoreSettings(), settings);
    assert.deepEqual(await setSkinStoreUrl(settings.indexUrl), settings);
    assert.deepEqual(await listStoreSkins(), snapshot);
    assert.deepEqual(await refreshSkinStore(), snapshot);
    assert.equal(await getSkinStorePreview("sample"), "data:image/png;base64,dGVzdA==");
    assert.deepEqual(await installStoreSkin("sample"), catalog);
    assert.deepEqual(await purchaseStoreSkin("sample"), purchase);
    assert.deepEqual(calls, [
      { command: "skin_store_get_settings", args: {} },
      { command: "skin_store_set_url", args: { url: settings.indexUrl } },
      { command: "skin_store_list", args: {} },
      { command: "skin_store_refresh", args: {} },
      { command: "skin_store_preview", args: { id: "sample" } },
      { command: "skin_store_install", args: { id: "sample" } },
      { command: "skin_store_purchase", args: { id: "sample" } },
    ]);
  } finally {
    delete runtime.__TAURI_INTERNALS__;
  }
});

test("skin store wrappers preserve backend human-readable errors", async () => {
  const runtime = window as unknown as { __TAURI_INTERNALS__?: { invoke: () => Promise<unknown> } };
  runtime.__TAURI_INTERNALS__ = { invoke: async () => { throw "签名校验失败，请重试"; } };
  try {
    await assert.rejects(refreshSkinStore(), (error) => error === "签名校验失败，请重试");
  } finally {
    delete runtime.__TAURI_INTERNALS__;
  }
});

test("skin prices show free, whole yuan and exactly two decimal places for cents", () => {
  assert.equal(formatSkinPrice(0), "免费");
  assert.equal(formatSkinPrice(1200), "¥12");
  assert.equal(formatSkinPrice(1250), "¥12.50");
  assert.equal(formatSkinPrice(1201), "¥12.01");
  assert.equal(formatSkinPrice(1), "¥0.01");
});
