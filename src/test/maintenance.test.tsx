import assert from "node:assert/strict";
import { test } from "node:test";
import { act } from "react";
import { MaintenanceManager } from "../components/MaintenanceManager";
import type { LearningCleanupItem, LearningCleanupList } from "../lib/maintenance";
import { findButton, installMiniDom, MiniMouseEvent, type MiniElement } from "./support/miniDom";

const modelItem: LearningCleanupItem = {
  id: "synthetic-learned", text: "清茶", pinyin: "qing cha", source: "rime_userdb",
  domain: null, learnedAtMs: null, lastHitAtMs: null, factoryTop1: null,
  displacedFactoryTop1: false, attribution: null, batchCleanupEligible: false,
  frictionEventCount: 0, abandonedCount: 0, deepPageCount: 0, backspaceCount: 0,
  suspected: true, modelAudit: { suggested: "晴茶", flagged: 3, occurrences: 4, avgMargin: 4.5 },
};

async function mountMaintenance(
  listing: LearningCleanupList,
  check: (container: MiniElement, calls: { command: string; args?: Record<string, unknown> }[]) => Promise<void>,
) {
  const dom = installMiniDom();
  const runtime = window as unknown as {
    __TAURI_INTERNALS__?: { invoke: (command: string, args?: Record<string, unknown>) => Promise<unknown> };
  };
  const calls: { command: string; args?: Record<string, unknown> }[] = [];
  (globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  runtime.__TAURI_INTERNALS__ = {
    invoke: async (command, args) => {
      calls.push({ command, args });
      switch (command) {
        case "learning_preferences":
          return { appLearning: true, projectLearning: true, secureInputLearning: false, updatedAtMs: 0 };
        case "list_learning_cleanup_items": return listing;
        case "list_personal_blacklist": return [];
        case "preview_learning_cleanup":
          return { entryCount: 1, entryIds: [modelItem.id], texts: [modelItem.text] };
        case "apply_learning_cleanup": return { batchId: "synthetic-batch", deletedCount: 1 };
        case "undo_learning_cleanup": return 1;
        default: throw new Error(`Unexpected command: ${command}`);
      }
    },
  };
  const { createRoot } = await import("react-dom/client");
  const root = createRoot(dom.container as unknown as Element);
  try {
    await act(async () => { root.render(<MaintenanceManager />); });
    // Flush the lazy Tauri module import used by the first reload.
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

test("model evidence renders review wording and stays out of grouped cleanup", async () => {
  await mountMaintenance({ items: [modelItem], unmatchedModelAuditPairs: [] }, async (container, calls) => {
    assert.match(container.textContent, /模型复核：按上下文更像「晴茶」（3\/4 次）/);
    assert.match(container.textContent, /待人工复核/);
    const group = findButton(container, "预览本组 0 条");
    assert.ok(group);
    assert.equal(group.disabled, true);
    const checkbox = container.querySelector('[aria-label="选择 清茶"]');
    assert.ok(checkbox);
    assert.equal(checkbox.checked, false);
    assert.ok(calls.every((call) => !/preview_|apply_|undo_/.test(call.command)));
    assert.equal(container.querySelector(".learning-model-unmatched"), null);
  });
});

test("unmatched model pairs render in a folded read-only list without selection controls", async () => {
  await mountMaintenance({ items: [], unmatchedModelAuditPairs: [
    { pinyin: "mingt", committed: "名天", suggested: "明天", flagged: 2, occurrences: 3, avgMargin: 4.5 },
    { pinyin: "hua", committed: "花", suggested: "画", flagged: 3, occurrences: 4, avgMargin: 5 },
  ] }, async (container, calls) => {
    const details = container.querySelector(".learning-model-unmatched");
    assert.ok(details);
    assert.equal(details.getAttribute("open"), null);
    assert.match(details.textContent, /模型还发现 2 组可能的错字，但它们不是学习造成的，暂时无法在这里修/);
    assert.match(details.textContent, /名天→明天（mingt）/);
    assert.match(details.textContent, /花→画（hua）/);
    assert.equal(details.querySelectorAll("li").length, 2);
    assert.equal(details.querySelectorAll("input").length, 0);
    assert.equal(details.querySelectorAll("button").length, 0);
    assert.ok(calls.every((call) => !/preview_|apply_|undo_/.test(call.command)));
  });
});

test("model evidence uses the existing explicit selection preview apply and undo flow", async () => {
  await mountMaintenance({ items: [modelItem], unmatchedModelAuditPairs: [] }, async (container, calls) => {
    const checkbox = container.querySelector('[aria-label="选择 清茶"]');
    assert.ok(checkbox);
    checkbox.checked = true;
    await click(checkbox);
    const preview = findButton(container, "预览所选 1 条");
    assert.ok(preview);
    await click(preview);
    assert.deepEqual(calls.find((call) => call.command === "preview_learning_cleanup")?.args,
      { ids: [modelItem.id], domain: null });
    assert.ok(!calls.some((call) => call.command === "apply_learning_cleanup"));
    const apply = findButton(container, "确认清理 1 条");
    assert.ok(apply);
    await click(apply);
    assert.deepEqual(calls.find((call) => call.command === "apply_learning_cleanup")?.args,
      { ids: [modelItem.id], domain: null });
    const undo = findButton(container, "撤销");
    assert.ok(undo);
    await click(undo);
    assert.deepEqual(calls.find((call) => call.command === "undo_learning_cleanup")?.args,
      { batchId: "synthetic-batch" });
    assert.match(container.textContent, /已恢复 1 条学习状态/);
  });
});
