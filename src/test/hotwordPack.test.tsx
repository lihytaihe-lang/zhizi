import assert from "node:assert/strict";
import { test } from "node:test";
import { act } from "react";
import { DataSourceManager } from "../components/DataSourceManager";
import type { DynamicSource, HotwordItem } from "../lib/hotwords";
import { findButton, installMiniDom, MiniEvent, MiniMouseEvent, type MiniElement } from "./support/miniDom";

type InvokeCall = { command: string; args?: Record<string, unknown> };
const latestUrl = "https://example.invalid/test-only/latest.json";
const dayMs = 24 * 60 * 60 * 1000;

async function mountManager(
  handler: (command: string, args?: Record<string, unknown>) => unknown,
  check: (container: MiniElement, calls: InvokeCall[]) => Promise<void>,
) {
  const dom = installMiniDom();
  const runtime = window as unknown as {
    __TAURI_INTERNALS__?: { invoke: (command: string, args?: Record<string, unknown>) => Promise<unknown> };
  };
  const calls: InvokeCall[] = [];
  (globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  runtime.__TAURI_INTERNALS__ = {
    invoke: async (command, args) => {
      calls.push({ command, args });
      switch (command) {
        case "refresh_hotword_exposure": return null;
        case "list_dynamic_sources": return handler(command, args) ?? [];
        case "list_hotword_items": return handler(command, args) ?? [];
        case "get_hotword_pack_settings": return handler(command, args) ?? { url: "", lastPackId: null, lastUpdatedAtMs: null };
        default: return handler(command, args);
      }
    },
  };
  const { createRoot } = await import("react-dom/client");
  const root = createRoot(dom.container as unknown as Element);
  try {
    await act(async () => { root.render(<DataSourceManager />); });
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 0)); });
    await check(dom.container, calls);
  } finally {
    await act(async () => { root.unmount(); });
    delete runtime.__TAURI_INTERNALS__;
    dom.cleanup();
  }
}

async function click(element: MiniElement) {
  await act(async () => {
    element.dispatchEvent(new MiniMouseEvent("click", { bubbles: true, cancelable: true }));
  });
}

test("blank pack address disables updates and opening the panel only reads local settings", async () => {
  await mountManager(() => undefined, async (container, calls) => {
    const update = findButton(container, "更新热词");
    assert.ok(update);
    assert.equal(update.disabled, true);
    assert.match(container.textContent, /未配置热词包地址/);
    assert.deepEqual(calls.map((call) => call.command).sort(), [
      "get_hotword_pack_settings", "list_dynamic_sources", "list_hotword_items", "refresh_hotword_exposure",
    ]);
  });
});

test("pack age prompts an update only after fourteen days", async () => {
  for (const days of [14, 15]) {
    await mountManager((command) => command === "get_hotword_pack_settings"
      ? { url: latestUrl, lastPackId: "test-pack", lastUpdatedAtMs: Date.now() - days * dayMs }
      : undefined, async (container, calls) => {
      assert.match(container.textContent, new RegExp(`热词更新于 ${days} 天前`));
      assert.equal(container.textContent.includes("超过 14 天，建议更新热词"), days > 14);
      assert.ok(!calls.some((call) => call.command === "update_hotword_pack"));
    });
  }
});

test("saving a changed address is local and never starts a download", async () => {
  let storedUrl = "";
  await mountManager((command, args) => {
    if (command === "get_hotword_pack_settings") return { url: storedUrl, lastPackId: null, lastUpdatedAtMs: null };
    if (command === "save_hotword_pack_url") {
      storedUrl = args?.url as string;
      return { url: storedUrl, lastPackId: null, lastUpdatedAtMs: null };
    }
  }, async (container, calls) => {
    const address = container.querySelector("#hotword-pack-url");
    assert.ok(address);
    await act(async () => {
      address.value = latestUrl;
      address.dispatchEvent(new MiniEvent("input", { bubbles: true, cancelable: true }));
    });
    const save = findButton(container, "保存热词包地址");
    assert.ok(save);
    await click(save);
    assert.deepEqual(calls.find((call) => call.command === "save_hotword_pack_url")?.args, { url: latestUrl });
    assert.ok(!calls.some((call) => call.command === "update_hotword_pack"));
    assert.match(container.textContent, /热词包地址已保存/);
  });
});

test("manual update saves the address then fetches once and keeps the button disabled while pending", async () => {
  let resolveUpdate: (result: unknown) => void;
  await mountManager((command) => {
    if (command === "get_hotword_pack_settings" || command === "save_hotword_pack_url") {
      return { url: latestUrl, lastPackId: null, lastUpdatedAtMs: null };
    }
    if (command === "update_hotword_pack") return new Promise((resolve) => { resolveUpdate = resolve; });
  }, async (container, calls) => {
    const update = findButton(container, "更新热词");
    assert.ok(update);
    assert.equal(update.disabled, false);
    await click(update);
    assert.equal(update.disabled, true);
    assert.deepEqual(calls.filter((call) => /save_hotword_pack_url|update_hotword_pack/.test(call.command)), [
      { command: "save_hotword_pack_url", args: { url: latestUrl } },
      { command: "update_hotword_pack", args: {} },
    ]);
    await act(async () => {
      resolveUpdate({ alreadyLatest: true, packId: "test-pack", receivedCount: 0, pinyinConflictCount: 0, message: "已是最新" });
    });
    assert.match(container.textContent, /已是最新/);
    assert.equal(update.disabled, false);
  });
});

test("failed manual verification displays the reason and allows retry", async () => {
  await mountManager((command) => {
    if (command === "get_hotword_pack_settings" || command === "save_hotword_pack_url") {
      return { url: latestUrl, lastPackId: null, lastUpdatedAtMs: null };
    }
    if (command === "update_hotword_pack") throw new Error("热词包签名验证失败，已丢弃整包。");
  }, async (container) => {
    const update = findButton(container, "更新热词");
    assert.ok(update);
    await click(update);
    assert.match(container.textContent, /热词包签名验证失败，已丢弃整包。/);
    assert.equal(update.disabled, false);
  });
});

test("pack sources cannot use unsigned refresh and conflicting pinyin remains visible for review", async () => {
  const source: DynamicSource = {
    id: "pack:test-only", displayName: "测试词包", sourceType: "official_json", refreshIntervalMinutes: 60,
    authorization: "测试授权", termsUrl: latestUrl, license: "测试许可", cachePolicy: "last-known-good",
    ownership: "remote_cached", distribution: "snapshot-only", disablePolicy: "pause-and-clear", endpoint: latestUrl,
    optIn: true, enabled: true, lastRefreshMs: null, lastError: null, createdAtMs: 0, updatedAtMs: 0,
  };
  const item: HotwordItem = {
    id: "test-only", text: "测试词", pinyin: "ce shi ci", confidenceMillis: 600, freshnessMillis: 1000,
    domain: "test", expiresAtMs: null, reviewRequired: true, state: "review", sourceIds: [source.id],
    exposureCount: 0, misselectionCount: 0, updatedAtMs: 0, pinyinWarning: "拼音与冻结索引冲突，请审核。",
  };
  await mountManager((command) => {
    if (command === "list_dynamic_sources") return [source];
    if (command === "list_hotword_items") return [item];
  }, async (container) => {
    const refresh = findButton(container, "刷新");
    assert.ok(refresh);
    assert.equal(refresh.disabled, true);
    assert.match(container.textContent, /ce shi ci/);
    assert.match(container.textContent, /拼音与冻结索引冲突，请审核。/);
  });
});
