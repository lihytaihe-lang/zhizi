import assert from "node:assert/strict";
import { test } from "node:test";
import { act } from "react";
import { LexiconPacksPanel } from "../components/LexiconPacksPanel";
import type { InstalledPack, PackIndex } from "../lib/lexiconPacks";
import { findButton, installMiniDom, MiniMouseEvent, type MiniElement } from "./support/miniDom";

type InvokeCall = { command: string; args?: Record<string, unknown> };

function sampleIndex(): PackIndex {
  return {
    formatVersion: 1,
    lexiconVersion: "2026.10",
    packs: [
      {
        id: "poetry", name: "诗词", description: "唐诗宋词常见词", rows: 12000,
        dict: { file: "poetry.dict.yaml", sha256: "a".repeat(64), bytes: 400000 },
        membership: { file: "poetry.membership.tsv", sha256: "a".repeat(64), bytes: 5000 },
      },
      {
        id: "law", name: "法律", description: "法言法语", rows: 8000,
        dict: { file: "law.dict.yaml", sha256: "b".repeat(64), bytes: 300000 },
        membership: { file: "law.membership.tsv", sha256: "b".repeat(64), bytes: 4000 },
      },
    ],
  };
}

const installedLaw: InstalledPack = { id: "law", membershipBytes: 5000, dictPresent: true };

async function mountPacks(
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
      return handler(command, args);
    },
  };
  const { createRoot } = await import("react-dom/client");
  const root = createRoot(dom.container as unknown as Element);
  try {
    await act(async () => { root.render(<LexiconPacksPanel />); });
    // Flush the lazy Tauri module import used by the initial load.
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

function rowFor(container: MiniElement, text: string): MiniElement | null {
  return container.querySelectorAll("article").find((row) => row.textContent.includes(text)) ?? null;
}

test("lists index packs with names, descriptions, row counts and installed state", async () => {
  await mountPacks((command) => {
    switch (command) {
      case "lexicon_packs_available": return sampleIndex();
      case "lexicon_packs_installed": return [installedLaw];
      default: throw new Error(`Unexpected command: ${command}`);
    }
  }, async (container, calls) => {
    assert.deepEqual(calls.map((call) => call.command).sort(), ["lexicon_packs_available", "lexicon_packs_installed"]);
    const poetry = rowFor(container, "诗词");
    const law = rowFor(container, "法律");
    assert.ok(poetry && law);
    assert.match(poetry.textContent, /唐诗宋词常见词/);
    assert.match(poetry.textContent, /12000 条/);
    assert.match(poetry.textContent, /下载并安装/);
    assert.doesNotMatch(poetry.textContent, /已安装/);
    assert.match(law.textContent, /法言法语/);
    assert.match(law.textContent, /已安装/);
    assert.match(law.textContent, /移除/);
    assert.doesNotMatch(law.textContent, /下载并安装/);
    assert.match(container.textContent, /在输入法菜单的『词库…』里打开或关闭对应词库；输入法会在下次切换到它时自动载入新词库。/);
  });
});

test("install button invokes lexicon_pack_install with { id }, stays disabled while downloading, then flips to installed", async () => {
  let resolveInstall: (pack: InstalledPack) => void;
  await mountPacks((command) => {
    switch (command) {
      case "lexicon_packs_available": return sampleIndex();
      case "lexicon_packs_installed": return [installedLaw];
      case "lexicon_pack_install":
        return new Promise<InstalledPack>((resolve) => { resolveInstall = resolve; });
      default: throw new Error(`Unexpected command: ${command}`);
    }
  }, async (container, calls) => {
    const installButton = findButton(rowFor(container, "诗词") ?? container, "下载并安装");
    assert.ok(installButton);
    await click(installButton);
    assert.match(rowFor(container, "诗词")?.textContent ?? "", /正在下载…/);
    assert.equal(installButton.disabled, true);
    assert.deepEqual(calls.find((call) => call.command === "lexicon_pack_install")?.args, { id: "poetry" });

    await act(async () => {
      resolveInstall({ id: "poetry", membershipBytes: 9000, dictPresent: true });
      await new Promise((resolve) => setTimeout(resolve, 0));
    });
    const poetry = rowFor(container, "诗词");
    assert.ok(poetry);
    assert.match(poetry.textContent, /已安装/);
    assert.doesNotMatch(poetry.textContent, /正在下载/);
  });
});

test("concurrent installs keep each pack busy on its own and a repeat click never resubmits (A, B, A)", async () => {
  const resolvers: Record<string, (pack: InstalledPack) => void> = {};
  await mountPacks((command, args) => {
    switch (command) {
      case "lexicon_packs_available": return sampleIndex();
      case "lexicon_packs_installed": return [];
      case "lexicon_pack_install":
        return new Promise<InstalledPack>((resolve) => { resolvers[args?.id as string] = resolve; });
      default: throw new Error(`Unexpected command: ${command}`);
    }
  }, async (container, calls) => {
    const poetryButton = findButton(rowFor(container, "诗词") ?? container, "下载并安装");
    const lawButton = findButton(rowFor(container, "法律") ?? container, "下载并安装");
    assert.ok(poetryButton && lawButton);
    await click(poetryButton);
    await click(lawButton);
    assert.match(rowFor(container, "诗词")?.textContent ?? "", /正在下载…/);
    assert.match(rowFor(container, "法律")?.textContent ?? "", /正在下载…/);
    assert.equal(poetryButton.disabled, true);
    assert.equal(lawButton.disabled, true);

    // 第三次点击 A（即便绕过 disabled 属性）也不会重复提交。
    await click(poetryButton);
    assert.equal(calls.filter((call) => call.command === "lexicon_pack_install" && call.args?.id === "poetry").length, 1);
    assert.equal(calls.filter((call) => call.command === "lexicon_pack_install" && call.args?.id === "law").length, 1);

    // B 先完成也不会解除 A 的忙碌。
    await act(async () => {
      resolvers["law"]({ id: "law", membershipBytes: 8000, dictPresent: true });
      await new Promise((resolve) => setTimeout(resolve, 0));
    });
    assert.match(rowFor(container, "诗词")?.textContent ?? "", /正在下载…/);
    assert.equal(poetryButton.disabled, true);

    await act(async () => {
      resolvers["poetry"]({ id: "poetry", membershipBytes: 9000, dictPresent: true });
      await new Promise((resolve) => setTimeout(resolve, 0));
    });
    assert.match(rowFor(container, "诗词")?.textContent ?? "", /已安装/);
    assert.match(rowFor(container, "法律")?.textContent ?? "", /已安装/);
  });
});

test("failed install shows the error and re-enables the download button", async () => {
  await mountPacks((command) => {
    switch (command) {
      case "lexicon_packs_available": return sampleIndex();
      case "lexicon_packs_installed": return [installedLaw];
      case "lexicon_pack_install": throw new Error("下载超时");
      default: throw new Error(`Unexpected command: ${command}`);
    }
  }, async (container) => {
    const installButton = findButton(rowFor(container, "诗词") ?? container, "下载并安装");
    assert.ok(installButton);
    await click(installButton);
    assert.match(container.textContent, /诗词 安装失败：下载超时/);
    const freshButton = findButton(rowFor(container, "诗词") ?? container, "下载并安装");
    assert.ok(freshButton);
    assert.equal(freshButton.disabled, false);
    assert.doesNotMatch(rowFor(container, "诗词")?.textContent ?? "", /已安装/);
  });
});

test("index failure still lists installed packs and offers a working retry", async () => {
  let indexReachable = false;
  await mountPacks((command) => {
    switch (command) {
      case "lexicon_packs_available":
        if (!indexReachable) throw new Error("GitHub 不可达");
        return sampleIndex();
      case "lexicon_packs_installed": return [installedLaw];
      default: throw new Error(`Unexpected command: ${command}`);
    }
  }, async (container, calls) => {
    assert.match(container.textContent, /取词库包列表失败：GitHub 不可达/);
    const law = rowFor(container, "law");
    assert.ok(law);
    assert.match(law.textContent, /已安装/);
    assert.match(law.textContent, /移除/);
    assert.doesNotMatch(container.textContent, /诗词/);

    indexReachable = true;
    const retry = findButton(container, "重试");
    assert.ok(retry);
    await click(retry);
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 0)); });
    assert.equal(calls.filter((call) => call.command === "lexicon_packs_available").length, 2);
    assert.doesNotMatch(container.textContent, /取词库包列表失败/);
    assert.match(rowFor(container, "诗词")?.textContent ?? "", /下载并安装/);
    assert.match(rowFor(container, "法律")?.textContent ?? "", /已安装/);
  });
});

test("remove confirms through window.confirm and updates the list to the returned packs", async () => {
  const confirmations: string[] = [];
  const runtimeWindow = window as unknown as { confirm?: (message: string) => boolean };
  const originalConfirm = runtimeWindow.confirm;
  runtimeWindow.confirm = (message: string) => {
    confirmations.push(message);
    return true;
  };
  try {
    let installed: InstalledPack[] = [installedLaw];
    await mountPacks((command) => {
      switch (command) {
        case "lexicon_packs_available": return sampleIndex();
        case "lexicon_packs_installed": return installed;
        case "lexicon_pack_remove": return (installed = []);
        default: throw new Error(`Unexpected command: ${command}`);
      }
    }, async (container, calls) => {
      const removeButton = findButton(rowFor(container, "法律") ?? container, "移除");
      assert.ok(removeButton);
      await click(removeButton);
      assert.deepEqual(confirmations, ["移除词库包「法律」？"]);
      assert.deepEqual(calls.find((call) => call.command === "lexicon_pack_remove")?.args, { id: "law" });
      const law = rowFor(container, "法律");
      assert.ok(law);
      assert.match(law.textContent, /下载并安装/);
      assert.doesNotMatch(law.textContent, /已安装/);
    });
  } finally {
    if (originalConfirm === undefined) delete runtimeWindow.confirm;
    else runtimeWindow.confirm = originalConfirm;
  }
});
