import assert from "node:assert/strict";
import { test } from "node:test";
import { act } from "react";
import { SkinStore } from "../components/SkinStore";
import type { CandidateSkinCatalogSnapshot } from "../lib/lexicon";
import type { StoreSkin, StoreSnapshot } from "../lib/skinStore";
import { findButton, installMiniDom, MiniEvent, MiniKeyboardEvent, MiniMouseEvent, type MiniElement } from "./support/miniDom";

type InvokeCall = { command: string; args?: Record<string, unknown> };
const storeUrl = "https://example.invalid/skins";
const updatedAt = Date.UTC(2026, 10, 8, 4);

function skin(id: string, overrides: Partial<StoreSkin> = {}): StoreSkin {
  return {
    id, name: { zh: id, en: `English ${id}` }, author: "测试作者", description: `${id} 的皮肤描述`,
    version: "1.2.0", tier: "free", productId: `skin.${id}`, animated: false,
    bundleBytes: 812345, minAppVersion: "0.3.24", updatedAt: "2026-11-08", license: "测试许可",
    changelog: "改善预览", state: "notInstalled", installedVersion: null,
    ...overrides,
  };
}

function snapshot(skins: StoreSkin[] = [], overrides: Partial<StoreSnapshot> = {}): StoreSnapshot {
  return { skins, refreshedAtMs: updatedAt, stale: false, error: null, ...overrides };
}

function catalog(id: string): CandidateSkinCatalogSnapshot {
  return { skins: [{ id, displayName: { zh: id, en: id }, author: "测试作者", description: "测试皮肤",
    origin: "external", status: "ready", reason: null, canUninstall: true }], issues: [] };
}

async function click(element: MiniElement | null | undefined) {
  assert.ok(element, "Expected an interactive element");
  await act(async () => {
    element.dispatchEvent(new MiniMouseEvent("click", { bubbles: true, cancelable: true }));
    await new Promise((resolve) => setTimeout(resolve, 0));
  });
}

function row(container: MiniElement, id: string) {
  const result = container.querySelectorAll("article").find((entry) => entry.textContent.includes(id));
  assert.ok(result, `Missing card for ${id}`);
  return result;
}

async function enterText(element: MiniElement | null, value: string) {
  assert.ok(element);
  await act(async () => {
    element.value = value;
    element.dispatchEvent(new MiniEvent("input", { bubbles: true, cancelable: true }));
  });
}

async function mountStore(
  options: {
    skins?: StoreSkin[]; initial?: StoreSnapshot; url?: string;
    handler?: (command: string, args?: Record<string, unknown>) => unknown;
    beforeCheck?: boolean;
  },
  check: (container: MiniElement, calls: InvokeCall[], focus: () => void) => Promise<void>,
) {
  const dom = installMiniDom();
  const runtime = window as unknown as { __TAURI_INTERNALS__?: { invoke: (command: string, args?: Record<string, unknown>) => Promise<unknown> } };
  const calls: InvokeCall[] = [];
  const previousAdd = window.addEventListener;
  const previousRemove = window.removeEventListener;
  let onFocus: EventListenerOrEventListenerObject | null = null;
  window.addEventListener = (type: string, listener: EventListenerOrEventListenerObject) => {
    if (type === "focus") onFocus = listener;
  };
  window.removeEventListener = (type: string, listener: EventListenerOrEventListenerObject) => {
    if (type === "focus" && listener === onFocus) onFocus = null;
  };
  (globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  runtime.__TAURI_INTERNALS__ = { invoke: async (command, args) => {
    calls.push({ command, args });
    const custom = options.handler?.(command, args);
    if (custom !== undefined) return custom;
    switch (command) {
      case "skin_store_get_settings":
      case "skin_store_set_url": return { indexUrl: options.url ?? storeUrl, lastRefreshedAtMs: options.url === "" ? null : updatedAt };
      case "skin_store_list":
      case "skin_store_refresh": return options.initial ?? snapshot(options.skins);
      case "skin_store_preview": return "data:image/png;base64,dGVzdA==";
      default: throw new Error(`Unexpected command: ${command}`);
    }
  } };
  const { createRoot } = await import("react-dom/client");
  const root = createRoot(dom.container as unknown as Element);
  try {
    await act(async () => {
      root.render(<SkinStore />);
    });
    if (options.beforeCheck !== false) {
      await act(async () => { await new Promise((resolve) => setTimeout(resolve, 0)); });
    }
    await check(dom.container, calls, () => {
      if (typeof onFocus === "function") onFocus(new Event("focus"));
    });
  } finally {
    await act(async () => { root.unmount(); });
    delete runtime.__TAURI_INTERNALS__;
    if (previousAdd) window.addEventListener = previousAdd;
    else Reflect.deleteProperty(window, "addEventListener");
    if (previousRemove) window.removeEventListener = previousRemove;
    else Reflect.deleteProperty(window, "removeEventListener");
    dom.cleanup();
  }
}

test("store starts with skeletons until local reads finish and never starts a network request on open", async () => {
  let resolveList!: (value: StoreSnapshot) => void;
  await mountStore({ handler: (command) => command === "skin_store_list"
    ? new Promise<StoreSnapshot>((resolve) => { resolveList = resolve; }) : undefined }, async (container, calls) => {
    assert.ok(container.querySelector(".skin-store-skeleton"));
    assert.deepEqual(calls.map((call) => call.command).sort(), ["skin_store_get_settings", "skin_store_list"]);
    await act(async () => { resolveList(snapshot([skin("云朵")])); });
    assert.equal(container.querySelector(".skin-store-skeleton"), null);
    assert.ok(row(container, "云朵"));
    assert.ok(!calls.some((call) => /refresh|preview/.test(call.command)));
  });
});

test("unconnected empty state explains the address, accepts a URL and connects explicitly", async () => {
  await mountStore({ url: "", initial: snapshot([], { refreshedAtMs: null }), handler: (command, args) => {
    if (command === "skin_store_set_url") return { indexUrl: args?.url, lastRefreshedAtMs: null };
    if (command === "skin_store_refresh") return snapshot([skin("云朵")]);
  } }, async (container, calls) => {
    assert.match(container.textContent, /还没有连接皮肤商店/);
    assert.ok(container.querySelector("#skin-store-url"));
    assert.equal(findButton(container, "连接")?.disabled, true);
    await enterText(container.querySelector("#skin-store-url"), storeUrl);
    assert.equal(findButton(container, "连接")?.disabled, false);
    await act(async () => {
      container.querySelector("form")!.dispatchEvent(new MiniEvent("submit", { bubbles: true, cancelable: true }));
      await new Promise((resolve) => setTimeout(resolve, 0));
    });
    assert.deepEqual(calls.find((call) => call.command === "skin_store_set_url")?.args, { url: storeUrl });
    assert.equal(calls.filter((call) => call.command === "skin_store_refresh").length, 1);
    assert.ok(row(container, "云朵"));
  });
});

test("cached failure preserves cards and last update, and retry refreshes explicitly", async () => {
  await mountStore({ initial: snapshot([skin("旧缓存")], { stale: true, error: "网络暂时不可用" }),
    handler: (command) => command === "skin_store_refresh" ? snapshot([skin("新索引")]) : undefined,
  }, async (container, calls) => {
    assert.ok(row(container, "旧缓存"));
    assert.match(container.textContent, /上次更新于/);
    assert.match(container.textContent, /当前无法刷新/);
    assert.match(container.textContent, /网络暂时不可用/);
    await click(findButton(container, "重试"));
    assert.ok(row(container, "新索引"));
    assert.doesNotMatch(container.textContent, /网络暂时不可用/);
    assert.equal(calls.filter((call) => call.command === "skin_store_refresh").length, 1);
  });
});

test("refresh rejection retains cached cards and permits retry without duplicate refreshes", async () => {
  let rejectRefresh!: (reason: unknown) => void;
  await mountStore({ skins: [skin("保留缓存")], handler: (command) => command === "skin_store_refresh"
    ? new Promise((_, reject) => { rejectRefresh = reject; }) : undefined }, async (container, calls) => {
    await click(findButton(container, "刷新"));
    const pending = container.querySelectorAll("button").find((button) => button.textContent.includes("刷新"));
    assert.ok(pending?.disabled);
    await click(pending);
    assert.equal(calls.filter((call) => call.command === "skin_store_refresh").length, 1);
    await act(async () => { rejectRefresh("连接失败，请稍后重试"); });
    assert.ok(row(container, "保留缓存"));
    assert.match(container.textContent, /连接失败，请稍后重试/);
    assert.ok(findButton(container, "重试"));
  });
});

test("free cards show state labels and an upgrade explanation", async () => {
  await mountStore({ skins: [
    skin("免费皮肤"),
    skin("已装皮肤", { state: "installed", installedVersion: "1.2.0" }),
    skin("当前皮肤", { state: "installed", installedVersion: "1.2.0" }),
    skin("新版皮肤", { state: "updateAvailable", installedVersion: "1.0.0" }),
    skin("升级皮肤", { state: "needsAppUpgrade", minAppVersion: "99.0.0" }),
  ] }, async (container) => {
    assert.ok(findButton(row(container, "免费皮肤"), "获取"));
    assert.equal(findButton(row(container, "已装皮肤"), "已安装")?.disabled, true);
    assert.equal(findButton(row(container, "当前皮肤"), "已安装")?.disabled, true);
    assert.match(row(container, "新版皮肤").textContent, /新版/);
    assert.ok(findButton(row(container, "新版皮肤"), "更新"));
    assert.equal(findButton(row(container, "升级皮肤"), "需要升级知字")?.disabled, true);
    assert.match(row(container, "升级皮肤").textContent, /99.0.0/);
    assert.match(row(container, "免费皮肤").textContent, /免费/);
    assert.match(row(container, "免费皮肤").textContent, /English 免费皮肤/);
    assert.match(row(container, "免费皮肤").textContent, /测试作者/);
    assert.equal(findButton(row(container, "已装皮肤"), "设为当前"), null);
    assert.equal(findButton(row(container, "已装皮肤"), "卸载"), null);
  });
});

test("free installs and updates invoke install once, show pending progress and mark the installed product", async () => {
  for (const entry of [skin("下载皮肤"),
    skin("下载皮肤", { state: "updateAvailable", installedVersion: "1.0.0" })]) {
    let resolveInstall!: (value: CandidateSkinCatalogSnapshot) => void;
    await mountStore({ skins: [entry], handler: (command) => command === "skin_store_install"
      ? new Promise((resolve) => { resolveInstall = resolve; }) : undefined }, async (container, calls) => {
      const button = findButton(row(container, "下载皮肤"), entry.state === "updateAvailable" ? "更新" : "获取");
      await click(button);
      assert.equal(button?.disabled, true);
      assert.ok(row(container, "下载皮肤").textContent.includes(entry.state === "updateAvailable" ? "正在更新…" : "正在获取…"));
      assert.ok(row(container, "下载皮肤").querySelector('[role="progressbar"]'));
      await click(button);
      assert.deepEqual(calls.filter((call) => call.command === "skin_store_install"), [
        { command: "skin_store_install", args: { id: "下载皮肤" } },
      ]);
      const installed = catalog("下载皮肤");
      await act(async () => { resolveInstall(installed); });
      assert.equal(findButton(row(container, "下载皮肤"), "已安装")?.disabled, true);
      assert.match(container.textContent, /皮肤管理/);
    });
  }
});

test("failed installation shows the backend message and re-enables acquisition", async () => {
  await mountStore({ skins: [skin("失败皮肤")], handler: (command) => {
    if (command === "skin_store_install") throw "皮肤包校验失败";
  } }, async (container) => {
    await click(findButton(row(container, "失败皮肤"), "获取"));
    assert.match(container.textContent, /皮肤包校验失败/);
    assert.equal(findButton(row(container, "失败皮肤"), "获取")?.disabled, false);
  });
});

test("paid entries and account actions are absent even if cached data contains a paid skin", async () => {
  await mountStore({ skins: [skin("免费皮肤"), skin("付费样例", { tier: "paid" })] }, async (container, calls) => {
    assert.ok(row(container, "免费皮肤"));
    assert.doesNotMatch(container.textContent, /付费样例|账号与订单|登录|购买/);
    assert.deepEqual(calls.map(({ command }) => command).sort(), ["skin_store_get_settings", "skin_store_list"]);
  });
});

test("filters combine with case-insensitive search across names and authors", async () => {
  await mountStore({ skins: [skin("云朵", { name: { zh: "云朵", en: "Cloud" }, author: "Alice", animated: true }),
    skin("墨色", { name: { zh: "墨色", en: "Ink" }, tier: "paid" }),
    skin("雪色", { state: "installed", installedVersion: "1.2.0" }),
    skin("新版", { state: "updateAvailable", installedVersion: "1.0.0" })] }, async (container) => {
    assert.equal(container.querySelectorAll("article").length, 3);
    await click(findButton(container, "动画"));
    assert.equal(container.querySelectorAll("article").length, 1);
    assert.ok(row(container, "云朵"));
    await click(findButton(container, "已安装"));
    assert.equal(container.querySelectorAll("article").length, 2);
    await click(findButton(container, "全部"));
    await enterText(container.querySelector('[aria-label="搜索皮肤"]'), "cLoUd");
    assert.equal(container.querySelectorAll("article").length, 1);
    assert.ok(row(container, "云朵"));
    await enterText(container.querySelector('[aria-label="搜索皮肤"]'), "alice");
    assert.equal(container.querySelectorAll("article").length, 1);
    await enterText(container.querySelector('[aria-label="搜索皮肤"]'), "墨色");
    assert.equal(container.querySelectorAll("article").length, 0);
    assert.match(container.textContent, /没有|未找到/);
  });
});

test("details expose metadata, close by button and Escape, and return keyboard focus", async () => {
  await mountStore({ skins: [skin("详情皮肤", { animated: true })] }, async (container) => {
    const open = container.querySelector('[aria-label="查看 详情皮肤 详情"]');
    assert.ok(open);
    open.focus();
    await click(open);
    const drawer = container.querySelector('[role="dialog"]');
    assert.ok(drawer);
    assert.equal(drawer.getAttribute("aria-modal"), "true");
    for (const value of ["详情皮肤 的皮肤描述", "1.2.0", "2026-11-08", "测试作者", "测试许可", "改善预览"]) {
      assert.ok(drawer.textContent.includes(value), value);
    }
    assert.match(drawer.textContent, /KB|KiB|MB|MiB|字节/);
    await click(container.querySelector('[aria-label="关闭皮肤详情"]'));
    assert.equal(container.querySelector('[role="dialog"]'), null);
    assert.equal(document.activeElement, open);
    await click(open);
    await act(async () => {
      document.dispatchEvent(new MiniKeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
    });
    assert.equal(container.querySelector('[role="dialog"]'), null);
    assert.equal(document.activeElement, open);
  });
});

test("previews wait for explicit refresh and intersection and failures retain a placeholder", async () => {
  type Observed = { callback: (entries: { isIntersecting: boolean; target: Element }[]) => void; target?: Element };
  const observed: Observed[] = [];
  const descriptor = Object.getOwnPropertyDescriptor(globalThis, "IntersectionObserver");
  Object.defineProperty(globalThis, "IntersectionObserver", { configurable: true, value: class {
    entry: Observed;
    constructor(callback: Observed["callback"]) { this.entry = { callback }; observed.push(this.entry); }
    observe(target: Element) { this.entry.target = target; }
    unobserve() {}
    disconnect() {}
  } });
  try {
    await mountStore({ skins: [skin("预览皮肤")], handler: (command) => {
      if (command === "skin_store_preview") throw "预览暂不可用";
    } }, async (container, calls) => {
      assert.ok(!calls.some((call) => call.command === "skin_store_preview"));
      await click(findButton(container, "刷新"));
      assert.ok(!calls.some((call) => call.command === "skin_store_preview"));
      const preview = observed.find((entry) => entry.target);
      assert.ok(preview?.target);
      await act(async () => { preview.callback([{ isIntersecting: false, target: preview.target! }]); });
      assert.ok(!calls.some((call) => call.command === "skin_store_preview"));
      await act(async () => {
        preview.callback([{ isIntersecting: true, target: preview.target! }]);
        await new Promise((resolve) => setTimeout(resolve, 0));
      });
      assert.deepEqual(calls.find((call) => call.command === "skin_store_preview")?.args, { id: "预览皮肤" });
      assert.ok(row(container, "预览皮肤").querySelector(".skin-store-preview-placeholder"));
    });
  } finally {
    if (descriptor) Object.defineProperty(globalThis, "IntersectionObserver", descriptor);
    else delete (globalThis as unknown as { IntersectionObserver?: unknown }).IntersectionObserver;
  }
});

test("an installed skin with an available update remains actionable", async () => {
  await mountStore({ skins: [skin("当前新版", {
    state: "updateAvailable", installedVersion: "1.0.0",
  })] }, async (container) => {
    assert.equal(findButton(row(container, "当前新版"), "更新")?.disabled, false);
    assert.equal(findButton(row(container, "当前新版"), "使用中"), null);
  });
});

test("details contain Tab and Shift+Tab focus and main actions do not open the drawer", async () => {
  await mountStore({ skins: [skin("键盘皮肤", { state: "installed", installedVersion: "1.2.0" })] }, async (container) => {
    await click(findButton(row(container, "键盘皮肤"), "已安装"));
    assert.equal(container.querySelector('[role="dialog"]'), null);
    await click(container.querySelector('[aria-label="查看 键盘皮肤 详情"]'));
    const drawer = container.querySelector('[role="dialog"]');
    assert.ok(drawer);
    const buttons = drawer.querySelectorAll("button").filter((button) => !button.disabled);
    const first = buttons[0];
    const last = buttons[buttons.length - 1];
    assert.equal(document.activeElement, first);
    const backwards = new MiniKeyboardEvent("keydown", { key: "Tab", shiftKey: true, cancelable: true, bubbles: true });
    await act(async () => { document.dispatchEvent(backwards); });
    assert.equal(backwards.defaultPrevented, true);
    assert.equal(document.activeElement, last);
    const forwards = new MiniKeyboardEvent("keydown", { key: "Tab", cancelable: true, bubbles: true });
    await act(async () => { document.dispatchEvent(forwards); });
    assert.equal(forwards.defaultPrevented, true);
    assert.equal(document.activeElement, first);
  });
});

test("built-in skin IDs are excluded from store cards even when present in the index", async () => {
  await mountStore({ skins: [skin("system", { name: { zh: "内置误入", en: "Built in" } }), skin("商店外部")] }, async (container) => {
    assert.equal(container.querySelectorAll("article").length, 1);
    assert.ok(row(container, "商店外部"));
    assert.doesNotMatch(container.textContent, /内置误入/);
  });
});

test("missing IntersectionObserver keeps placeholders and never fetches previews eagerly", async () => {
  const descriptor = Object.getOwnPropertyDescriptor(globalThis, "IntersectionObserver");
  delete (globalThis as unknown as { IntersectionObserver?: unknown }).IntersectionObserver;
  try {
    await mountStore({ skins: [skin("兼容预览")] }, async (container, calls) => {
      await click(findButton(container, "刷新"));
      await click(container.querySelector('[aria-label="查看 兼容预览 详情"]'));
      assert.ok(container.querySelector(".skin-store-preview-placeholder"));
      assert.ok(!calls.some((call) => call.command === "skin_store_preview"));
    });
  } finally {
    if (descriptor) Object.defineProperty(globalThis, "IntersectionObserver", descriptor);
  }
});

test("installation serializes free downloads until the catalog update completes", async () => {
  let resolveInstall!: (value: CandidateSkinCatalogSnapshot) => void;
  await mountStore({ skins: [skin("first-install"), skin("second-install")],
    handler: (command) => command === "skin_store_install"
      ? new Promise((resolve) => { resolveInstall = resolve; }) : undefined,
  }, async (container, calls) => {
    await click(findButton(row(container, "first-install"), "获取"));
    const second = findButton(row(container, "second-install"), "获取");
    assert.equal(second?.disabled, true);
    await click(second);
    assert.deepEqual(calls.filter(({ command }) => command === "skin_store_install").map(({ args }) => args?.id), ["first-install"]);
    await act(async () => { resolveInstall(catalog("first-install")); });
    assert.equal(second?.disabled, false);
    await click(second);
    await act(async () => { resolveInstall(catalog("second-install")); });
    assert.deepEqual(calls.filter(({ command }) => command === "skin_store_install").map(({ args }) => args?.id), ["first-install", "second-install"]);
  });
});

test("returning from the gallery rereads the local store cache without reaching the network", async () => {
  let installed = false;
  await mountStore({ handler: (command) => command === "skin_store_list"
    ? snapshot([skin("native-gallery", installed ? { state: "installed", installedVersion: "1.2.0" } : {})]) : undefined,
  }, async (container, calls, focus) => {
    assert.ok(findButton(row(container, "native-gallery"), "获取"));
    installed = true;
    await act(async () => focus());
    assert.equal(findButton(row(container, "native-gallery"), "已安装")?.disabled, true);
    assert.equal(calls.filter(({ command }) => command === "skin_store_list").length, 2);
    assert.ok(!calls.some(({ command }) => command === "skin_store_refresh"));
  });
});

test("verified preview data becomes an image and card and details share one preview request", async () => {
  type Observed = { callback: (entries: { isIntersecting: boolean; target: Element }[]) => void; target?: Element };
  const observed: Observed[] = [];
  const descriptor = Object.getOwnPropertyDescriptor(globalThis, "IntersectionObserver");
  Object.defineProperty(globalThis, "IntersectionObserver", { configurable: true, value: class {
    entry: Observed;
    constructor(callback: Observed["callback"]) { this.entry = { callback }; observed.push(this.entry); }
    observe(target: Element) { this.entry.target = target; }
    disconnect() {}
  } });
  const image = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+/rWQAAAAASUVORK5CYII=";
  try {
    await mountStore({ skins: [skin("verified-preview")], handler: (command) => command === "skin_store_preview" ? image : undefined },
      async (container, calls) => {
        await click(findButton(container, "刷新"));
        const cardPreview = observed.find((entry) => entry.target);
        assert.ok(cardPreview?.target);
        await act(async () => {
          cardPreview.callback([{ isIntersecting: true, target: cardPreview.target! }]);
          await new Promise((resolve) => setTimeout(resolve, 0));
        });
        const cardImage = row(container, "verified-preview").querySelector("img");
        assert.equal(cardImage?.getAttribute("src"), image);
        assert.equal(cardImage?.getAttribute("alt"), "verified-preview 皮肤预览");
        await click(container.querySelector('[aria-label="查看 verified-preview 详情"]'));
        const detailPreview = observed[observed.length - 1];
        assert.ok(detailPreview.target);
        await act(async () => {
          detailPreview.callback([{ isIntersecting: true, target: detailPreview.target! }]);
          await new Promise((resolve) => setTimeout(resolve, 0));
        });
        assert.equal(container.querySelector('[role="dialog"]')?.querySelector("img")?.getAttribute("src"), image);
        assert.equal(calls.filter((call) => call.command === "skin_store_preview").length, 1);
      });
  } finally {
    if (descriptor) Object.defineProperty(globalThis, "IntersectionObserver", descriptor);
    else delete (globalThis as unknown as { IntersectionObserver?: unknown }).IntersectionObserver;
  }
});

test("a failed settings read still renders cached cards and never starts a request", async () => {
  await mountStore({ skins: [skin("settings-failure-cache")], handler: (command) => {
    if (command === "skin_store_get_settings") throw "无法读取商店设置";
  } }, async (container, calls) => {
    assert.ok(row(container, "settings-failure-cache"));
    assert.match(container.textContent, /无法读取商店设置/);
    assert.equal(container.querySelector(".skin-store-skeleton"), null);
    assert.deepEqual(calls.map((call) => call.command).sort(), ["skin_store_get_settings", "skin_store_list"]);
  });
});

test("a rejected cache read exits loading and offers an explicit successful retry", async () => {
  await mountStore({ skins: [skin("recovered-index")], handler: (command) => {
    if (command === "skin_store_list") throw "无法读取商店缓存";
  } }, async (container, calls) => {
    assert.match(container.textContent, /无法读取商店缓存/);
    assert.equal(container.querySelector(".skin-store-skeleton"), null);
    assert.equal(calls.filter((call) => call.command === "skin_store_refresh").length, 0);
    await click(findButton(container, "重试"));
    assert.ok(row(container, "recovered-index"));
    assert.doesNotMatch(container.textContent, /无法读取商店缓存/);
    assert.equal(calls.filter((call) => call.command === "skin_store_refresh").length, 1);
  });
});

test("installation errors remain readable inside the open details dialog", async () => {
  const notice = "皮肤下载失败，请重试";
  await mountStore({ skins: [skin("dialog-feedback")],
    handler: (command) => { if (command === "skin_store_install") throw notice; },
  }, async (container) => {
    await click(container.querySelector('[aria-label="查看 dialog-feedback 详情"]'));
    const dialog = container.querySelector('[role="dialog"]');
    assert.ok(dialog);
    await click(findButton(dialog, "获取"));
    assert.ok(dialog.textContent.includes(notice));
    assert.equal(container.querySelector('[role="dialog"]'), dialog);
  });
});

test("details retain Tab focus when its pending action becomes disabled or focus falls to the body", async () => {
  let resolveInstall!: (value: CandidateSkinCatalogSnapshot) => void;
  await mountStore({ skins: [skin("dialog-pending")], handler: (command) => command === "skin_store_install"
    ? new Promise((resolve) => { resolveInstall = resolve; }) : undefined,
  }, async (container) => {
    await click(container.querySelector('[aria-label="查看 dialog-pending 详情"]'));
    const dialog = container.querySelector('[role="dialog"]');
    assert.ok(dialog);
    const action = findButton(dialog, "获取");
    const close = dialog.querySelector('[aria-label="关闭皮肤详情"]');
    assert.ok(action && close);
    action.focus();
    await click(action);
    assert.equal(action.disabled, true);
    const fromDisabled = new MiniKeyboardEvent("keydown", { key: "Tab", cancelable: true, bubbles: true });
    await act(async () => { document.dispatchEvent(fromDisabled); });
    assert.equal(fromDisabled.defaultPrevented, true);
    assert.ok(document.activeElement === close, "focus should stay on the dialog close action");
    document.body.focus();
    const fromBody = new MiniKeyboardEvent("keydown", { key: "Tab", cancelable: true, bubbles: true });
    await act(async () => { document.dispatchEvent(fromBody); });
    assert.equal(fromBody.defaultPrevented, true);
    assert.ok(document.activeElement === close, "focus should stay on the dialog close action");
    await act(async () => { resolveInstall(catalog("dialog-pending")); });
  });
});
