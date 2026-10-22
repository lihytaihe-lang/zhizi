import assert from "node:assert/strict";
import { test } from "node:test";
import { BUILT_IN_MODES } from "../domain/modes";
import {
  buildBrowserPayloadPreview,
  externalTextAccessStatus,
  performExternalTextAction,
} from "../lib/layer2";
import { composePrompt } from "../lib/promptComposer";

test("browser fallback payload contains only explicit text, rules and non-unknown AppClass", () => {
  const mode = BUILT_IN_MODES.find((candidate) => candidate.id === "rewrite")!;
  const localPrompt = composePrompt({ rawInput: "改写 AgentInputMethod", mode, userRules: [] });
  const preview = buildBrowserPayloadPreview({
    rawInput: "改写 AgentInputMethod",
    mode,
    userRules: [{ id: "one", text: "保持简洁", enabled: true }, { id: "off", text: "不能出现", enabled: false }],
    localPrompt,
    compact: true,
    appClass: "Code",
    contextAllowed: true,
  });
  assert.match(preview.payload, /改写 AgentInputMethod/);
  assert.match(preview.payload, /保持简洁/);
  assert.match(preview.payload, /【AppClass】\nCode/);
  assert.doesNotMatch(preview.payload, /不能出现|bundle|events\.jsonl|前文片段/);
  assert.equal(preview.characterCount, [...preview.payload].length);
  assert.equal(preview.profileEntryCount, 0);
});

test("unknown context contributes no AppClass or profile entries", () => {
  const mode = BUILT_IN_MODES[0];
  const preview = buildBrowserPayloadPreview({
    rawInput: "无项目术语原文",
    mode,
    userRules: [],
    localPrompt: composePrompt({ rawInput: "无项目术语原文", mode, userRules: [] }),
    compact: false,
    appClass: "Unknown",
    contextAllowed: true,
  });
  assert.doesNotMatch(preview.payload, /【AppClass】/);
  assert.equal(preview.projectTermCount, 0);
  assert.equal(preview.profileEntryCount, 0);
});

test("browser fallback keeps replace and insert disabled while preserving explicit clipboard fallback", async () => {
  const access = await externalTextAccessStatus();
  assert.equal(access.actionsEnabled, false);
  assert.match(access.message, /复制仍可用/);

  const action = await performExternalTextAction("replace", "预览结果");
  assert.equal(action.status, "unsupported");
  assert.equal(action.fallbackToClipboard, true);
  assert.match(action.message, /退回剪贴板/);
});
