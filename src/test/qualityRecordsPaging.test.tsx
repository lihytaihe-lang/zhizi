import assert from "node:assert/strict";
import { test } from "node:test";
import { act } from "react";
import { QualityRecordingPanel } from "../components/QualityRecordingPanel";
import type { QualityRecordingEvent } from "../lib/lexicon";
import { findButton, installMiniDom, MiniMouseEvent, type MiniElement } from "./support/miniDom";

function record(id: string): QualityRecordingEvent {
  return { id, recordedAtMs: 1000, pinyin: "ni", candidates: ["你"], candidateComments: [""],
    selectedIndex: 0, selectedText: id, appHash: "a".repeat(64), precedingText: "", fuzzyMask: 0,
    typoCorrectionEnabled: false, activeProject: null, activePacks: [], secondaryEnabled: true,
    maxPage: 0, outcome: "committed_candidate", durationMs: 1, backspaces: 0,
    redoAfterCommit: false, candidateCount: 1, selectedCandidateComment: null };
}

async function click(container: MiniElement, label: string) {
  const button = findButton(container, label); assert.ok(button, label);
  await act(async () => { button.dispatchEvent(new MiniMouseEvent("click", { bubbles: true, cancelable: true })); });
}

test("records page stays usable after profile errors and uses whole-log counts and file export", async () => {
  const dom = installMiniDom();
  const runtime = window as unknown as { __TAURI_INTERNALS__?: {
    invoke: (command: string, args?: Record<string, unknown>) => Promise<unknown>;
  }; confirm: (message: string) => boolean };
  (globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  const calls: { command: string; args?: Record<string, unknown> }[] = [];
  const confirmations: string[] = [];
  const previousConfirm = runtime.confirm;
  runtime.confirm = (message) => { confirmations.push(message); return false; };
  let exports = 0;
  runtime.__TAURI_INTERNALS__ = { invoke: async (command, args) => {
    calls.push({ command, args });
    switch (command) {
      case "quality_recording_status": return { enabled: false, expiresAtMs: 0, continuous: false };
      case "quality_recording_consent_status": return { granted: true, grantedAtMs: 1, noticeVersion: "w7-v1" };
      case "quality_records_overview": return { totalCount: 100000, commits: 90000, firstChoiceRate: 0.8, redoRate: 0.02 };
      case "quality_records_page": return args?.cursor
        ? { records: [record("较早记录")], nextCursor: null }
        : { records: [record("最近记录")], nextCursor: "synthetic-cursor" };
      case "count_quality_records": return 100000;
      case "personal_profile_introspection": throw new Error("synthetic profile failure");
      case "list_personal_profile_auto_updates": return [];
      case "export_quality_records_file": return ++exports === 1 ? null : { path: "/tmp/synthetic-records.json", count: 100000 };
      default: throw new Error(`Unexpected command: ${command}`);
    }
  } };
  const { createRoot } = await import("react-dom/client");
  const root = createRoot(dom.container as unknown as Element);
  try {
    await act(async () => { root.render(<QualityRecordingPanel />); });
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 0)); });
    assert.match(dom.container.textContent, /共 100000 条，本页 1 条/);
    assert.match(dom.container.textContent, /最近记录/);
    await click(dom.container, "更早的记录");
    assert.match(dom.container.textContent, /ni → 较早记录/);
    assert.doesNotMatch(dom.container.textContent, /ni → 最近记录/);
    assert.equal(calls.find((call) => call.command === "quality_records_page" && call.args?.cursor)?.args?.limit, 100);
    await click(dom.container, "全部删除");
    assert.deepEqual(confirmations, ["将永久删除 100000 条记录。确定继续吗？"]);
    assert.ok(!calls.some((call) => call.command === "delete_quality_records"));
    await click(dom.container, "导出记录");
    assert.doesNotMatch(dom.container.textContent, /已导出/);
    await click(dom.container, "导出记录");
    assert.match(dom.container.textContent, /已导出 100000 条记录到 \/tmp\/synthetic-records.json/);
    assert.ok(!calls.some((call) => ["list_quality_records", "export_quality_records"].includes(call.command)));
  } finally {
    await act(async () => { root.unmount(); });
    runtime.confirm = previousConfirm; delete runtime.__TAURI_INTERNALS__; dom.cleanup();
  }
});
