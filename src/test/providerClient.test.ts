import assert from "node:assert/strict";
import { test } from "node:test";
import { isProviderReady, selectedProviderConfiguration } from "../lib/providerClient";

test("provider readiness requires opt-in and every credential field", () => {
  assert.equal(
    isProviderReady({ enabled: true, baseUrl: " https://example.com/v1 ", model: "m", apiKey: "k" }),
    true,
  );
  assert.equal(
    isProviderReady({ enabled: false, baseUrl: "https://example.com", model: "m", apiKey: "k" }),
    false,
  );
  assert.equal(
    isProviderReady({ enabled: true, baseUrl: "https://example.com", model: " ", apiKey: "k" }),
    false,
  );
});

test("provider selection preserves independent OpenAI, Anthropic and Ollama settings", () => {
  const settings = {
    enabled: true,
    providerKind: "anthropic" as const,
    baseUrl: "https://openai.example/v1",
    model: "openai-model",
    apiKey: "openai-key",
    anthropicBaseUrl: "https://anthropic.example",
    anthropicModel: "claude-fixture",
    anthropicApiKey: "anthropic-key",
    ollamaBaseUrl: "http://127.0.0.1:11434",
    ollamaModel: "qwen-fixture",
  };
  assert.deepEqual(selectedProviderConfiguration(settings), {
    kind: "anthropic", baseUrl: "https://anthropic.example", model: "claude-fixture", apiKey: "anthropic-key",
  });
  assert.equal(isProviderReady(settings), true);
  assert.equal(isProviderReady({ ...settings, providerKind: "ollama", ollamaModel: "" }), false);
  assert.equal(isProviderReady({ ...settings, providerKind: "ollama" }), true);
});
