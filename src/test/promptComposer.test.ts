import assert from "node:assert/strict";
import { test } from "node:test";
import { BUILT_IN_MODES } from "../domain/modes";
import { composePrompt } from "../lib/promptComposer";

test("all ten action modes generate non-empty main and compact local prompts", () => {
  for (const mode of BUILT_IN_MODES) {
    const main = composePrompt({ rawInput: "  做一个可靠的测试  ", mode, userRules: [] });
    const compact = composePrompt({
      rawInput: "做一个可靠的测试",
      mode,
      userRules: [],
      compact: true,
    });

    assert.ok(main.includes("做一个可靠的测试"), mode.id);
    assert.ok(compact.includes("做一个可靠的测试"), mode.id);
    assert.notEqual(main, compact, mode.id);
  }
  assert.equal(BUILT_IN_MODES.length, 10);
});

test("only enabled non-empty rules enter generated prompts", () => {
  const mode = BUILT_IN_MODES.find((item) => item.id === "codex-execution")!;
  const output = composePrompt({
    rawInput: "修复问题",
    mode,
    userRules: [
      { id: "enabled", text: "保留这个规则", enabled: true },
      { id: "disabled", text: "不能出现的规则", enabled: false },
      { id: "empty", text: "   ", enabled: true },
    ],
  });

  assert.match(output, /保留这个规则/);
  assert.doesNotMatch(output, /不能出现的规则/);
  assert.match(output, /执行边界/);
});

test("hotword generation local fallback includes the exact TSV contract and pinyin", () => {
  const mode = BUILT_IN_MODES.find((item) => item.id === "hotword-candidates")!;
  const output = composePrompt({ rawInput: "近期产品讨论", mode, userRules: [] });
  assert.match(output, /词<TAB>拼音<TAB>简短释义/);
  assert.match(output, /<词>\t<拼音>\t<简短释义>/);
  assert.match(output, /拼音由本次生成一并给出/);
});
