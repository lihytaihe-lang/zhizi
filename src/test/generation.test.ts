import assert from "node:assert/strict";
import { test } from "node:test";
import { BUILT_IN_MODES } from "../domain/modes";
import type { ProviderSettings } from "../domain/types";
import { generatePromptWithFallback } from "../lib/generation";

const disabledProvider: ProviderSettings = {
  enabled: false,
  baseUrl: "",
  model: "",
  apiKey: "",
};

const readyProvider: ProviderSettings = {
  enabled: true,
  baseUrl: "https://example.invalid/v1",
  model: "test-model",
  apiKey: "test-secret-never-log",
};

test("main and compact surfaces preserve local generation fallback", async () => {
  const mode = BUILT_IN_MODES[0];
  const main = await generatePromptWithFallback({
    rawInput: "实现登录页",
    mode,
    userRules: [],
    provider: disabledProvider,
    payload: "prepared main payload",
  });
  const compact = await generatePromptWithFallback({
    rawInput: "实现登录页",
    mode,
    userRules: [],
    provider: disabledProvider,
    compact: true,
    payload: "prepared compact payload",
  });

  assert.equal(main.source, "local");
  assert.equal(compact.source, "local");
  assert.notEqual(main.output, compact.output);
  assert.match(main.notice, /本地模板/);
});

test("provider success replaces local output", async () => {
  const result = await generatePromptWithFallback(
    {
      rawInput: "实现登录页",
      mode: BUILT_IN_MODES[0],
      userRules: [],
      provider: readyProvider,
      payload: "prepared provider payload",
    },
    async () => "模型结果",
  );

  assert.deepEqual(result, {
    output: "模型结果",
    source: "model",
    notice: "已使用模型增强优化。",
  });
});

test("empty or failed provider falls back without exposing API key", async () => {
  const input = {
    rawInput: "实现登录页",
    mode: BUILT_IN_MODES[0],
    userRules: [],
    provider: readyProvider,
    payload: "prepared provider payload",
  };
  const empty = await generatePromptWithFallback(input, async () => "  ");
  const failed = await generatePromptWithFallback(input, async () => {
    throw new Error("provider unavailable");
  });

  assert.equal(empty.source, "fallback");
  assert.equal(failed.source, "fallback");
  assert.match(failed.notice, /provider unavailable/);
  assert.doesNotMatch(JSON.stringify({ empty, failed }), /test-secret-never-log/);
});

test("provider timeout falls back without blocking local Prompt output", async () => {
  const result = await generatePromptWithFallback(
    {
      rawInput: "实现登录页",
      mode: BUILT_IN_MODES[0],
      userRules: [],
      provider: readyProvider,
      payload: "prepared provider payload",
    },
    async () => {
      throw new Error("Provider request timed out after 60 seconds");
    },
  );

  assert.equal(result.source, "fallback");
  assert.match(result.notice, /timed out/);
  assert.ok(result.output.length > 0);
  assert.doesNotMatch(JSON.stringify(result), /test-secret-never-log/);
});

test("provider unconfigured keeps all ten actions local and performs zero provider calls", async () => {
  let providerCalls = 0;
  for (const mode of BUILT_IN_MODES) {
    const result = await generatePromptWithFallback({
      rawInput: "本地动作原文",
      mode,
      userRules: [],
      provider: disabledProvider,
      payload: `prepared payload for ${mode.id}`,
    }, async () => {
      providerCalls += 1;
      return "不应调用";
    });
    assert.equal(result.source, "local", mode.id);
    assert.ok(result.output.includes("本地动作原文"), mode.id);
  }
  assert.equal(BUILT_IN_MODES.length, 10);
  assert.equal(providerCalls, 0);
});
