import assert from "node:assert/strict";
import { test } from "node:test";
import { act } from "react";
import { NativeLearningBackup } from "../components/NativeLearningBackup";
import { findButton, installMiniDom, MiniEvent, MiniMouseEvent, type MiniElement } from "./support/miniDom";

async function mount(check: (container: MiniElement, calls: { command: string; args?: Record<string, unknown> }[]) => Promise<void>, rejectMerge = false) {
  const dom = installMiniDom();
  const runtime = window as unknown as { __TAURI_INTERNALS__?: { invoke: (command: string, args?: Record<string, unknown>) => Promise<unknown> } };
  const calls: { command: string; args?: Record<string, unknown> }[] = [];
  (globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  runtime.__TAURI_INTERNALS__ = { invoke: async (command, args) => {
    calls.push({ command, args });
    if (command === "native_learning_status") return { entries: 2, choices: 3, actualBytes: 400, estimatedBytes: 800, maxBytes: 1048576, maxEntries: 4096, maxAgeDays: 45 };
    if (command === "preview_native_learning_import") return { entries: 4, choices: 5, estimatedBytes: 2000 };
    if (command === "merge_native_learning_import") {
      if (rejectMerge) throw "合并将超过学习容量，现有记录未改动";
      return { words: 4, choices: 5 };
    }
    throw new Error(`Unexpected command: ${command}`);
  } };
  const { createRoot } = await import("react-dom/client"); const root = createRoot(dom.container as unknown as Element);
  try {
    await act(async () => { root.render(<NativeLearningBackup />); });
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 0)); });
    await check(dom.container, calls);
  } finally { await act(async () => root.unmount()); delete runtime.__TAURI_INTERNALS__; dom.cleanup(); }
}
async function click(button: MiniElement | null) {
  assert.ok(button); await act(async () => { button.dispatchEvent(new MiniMouseEvent("click", { bubbles: true, cancelable: true })); });
}
async function enter(container: MiniElement, text: string) {
  const area = container.querySelector("textarea"); assert.ok(area);
  await act(async () => { area.value = text; area.dispatchEvent(new MiniEvent("input", { bubbles: true, cancelable: true })); });
}
test("backup requires a fresh preview after every edit and sends only the explicitly approved document", async () => {
  await mount(async (container, calls) => {
    assert.match(container.textContent, /个人数据/); assert.match(container.textContent, /各最多 4096/);
    assert.equal(findButton(container, "确认合并"), null);
    await enter(container, "backup-A"); await click(findButton(container, "预览校验"));
    assert.ok(findButton(container, "确认合并"));
    assert.ok(!calls.some((c) => c.command === "merge_native_learning_import"));
    await enter(container, "backup-B"); await enter(container, "backup-A");
    assert.equal(findButton(container, "确认合并"), null, "editing away and back cannot reuse an old approval preview");
    await click(findButton(container, "预览校验")); await click(findButton(container, "确认合并"));
    assert.deepEqual(calls.find((c) => c.command === "merge_native_learning_import")?.args, { document: "backup-A" });
    assert.equal(container.querySelector("textarea")?.value, ""); assert.match(container.textContent, /合并完成/);
  });
});
test("capacity rejection remains visible and preserves the pasted backup for correction", async () => {
  await mount(async (container, calls) => {
    await enter(container, "backup-over-capacity"); await click(findButton(container, "预览校验"));
    await click(findButton(container, "确认合并"));
    assert.match(container.textContent, /合并将超过学习容量，现有记录未改动/);
    assert.equal(container.querySelector("textarea")?.value, "backup-over-capacity");
    assert.equal(calls.filter((c) => c.command === "merge_native_learning_import").length, 1);
  }, true);
});
