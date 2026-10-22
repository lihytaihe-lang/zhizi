import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { beforeEach, test } from "node:test";
import {
  DEFAULT_PROVIDER_SETTINGS,
  loadAppState,
  loadLocalAppState,
  saveAppState,
  saveLocalAppState,
} from "../lib/storage";

beforeEach(() => window.localStorage.clear());

test("v0.3.0 loads and updates raw localStorage emitted by v0.1.0", async () => {
  const fixture = JSON.parse(
    readFileSync(new URL("./fixtures/v0.1.0/local-storage.json", import.meta.url), "utf8"),
  ) as {
    generatorCommit: string;
    packageVersion: string;
    entries: Record<string, string>;
  };

  assert.equal(fixture.generatorCommit, "69dc2280420c1f52df22bd593e83f32687e76bb5");
  assert.equal(fixture.packageVersion, "0.1.0");
  for (const [key, value] of Object.entries(fixture.entries)) {
    window.localStorage.setItem(key, value);
  }

  const loaded = loadLocalAppState();
  assert.equal(loaded.history.length, 2);
  assert.equal(loaded.history[0].id, "v010-history-favorite");
  assert.equal(loaded.history[0].targetTool, "Codex");
  assert.equal(loaded.history[0].favorite, true);
  assert.equal(loaded.history[1].modeId, "bug-fix");
  assert.equal(loaded.userRules.length, 2);
  assert.equal(loaded.userRules[0].enabled, true);
  assert.equal(loaded.userRules[1].enabled, false);
  assert.equal(loaded.selectedModeId, "bug-fix");
  assert.equal(loaded.providerSettings.baseUrl, "https://provider.invalid/v1");
  assert.equal(loaded.providerSettings.model, "synthetic-v010-model");
  assert.equal(loaded.providerSettings.apiKey, "synthetic-v010-key-never-contacted");
  assert.equal(loaded.shortcutSettings.shortcut, "control+space");

  await saveAppState({
    ...loaded,
    selectedModeId: "product-planning",
    shortcutSettings: { shortcut: "alt+space" },
  });
  const restarted = loadLocalAppState();
  assert.equal(restarted.history.length, 2);
  assert.equal(restarted.userRules.length, 2);
  assert.equal(restarted.selectedModeId, "product-planning");
  // Browser persistence must not carry Provider secrets, even when they
  // arrived as v0.1-era plaintext.
  assert.equal(restarted.providerSettings.apiKey, "");
  assert.equal(restarted.providerSettings.anthropicApiKey, "");
  const persisted = JSON.parse(
    window.localStorage.getItem("agent-input-layer.provider-settings") ?? "{}",
  );
  assert.equal(persisted.apiKey, "");
  assert.equal(persisted.anthropicApiKey, "");
  assert.equal(restarted.shortcutSettings.shortcut, "alt+space");
});

test("local storage preserves legacy Prompt history, rules, mode, provider and shortcut", () => {
  const state = {
    history: Array.from({ length: 55 }, (_, index) => ({
      id: `history-${index}`,
      rawInput: `raw-${index}`,
      modeId: "codex-execution" as const,
      targetTool: "Codex",
      output: `output-${index}`,
      favorite: index === 0,
      createdAt: `2026-07-12T00:00:${String(index).padStart(2, "0")}.000Z`,
    })),
    userRules: [{ id: "rule", text: "保留规则", enabled: true }],
    selectedModeId: "bug-fix" as const,
    providerSettings: {
      enabled: true,
      baseUrl: "https://example.invalid/v1",
      model: "test-model",
      apiKey: "local-only-test-key",
    },
    shortcutSettings: { shortcut: "control+space" },
  };

  saveLocalAppState(state);
  const loaded = loadLocalAppState();

  assert.equal(loaded.history.length, 50);
  assert.equal(loaded.history[0].favorite, true);
  assert.equal(loaded.userRules[0].text, "保留规则");
  assert.equal(loaded.selectedModeId, "bug-fix");
  assert.equal(loaded.providerSettings.model, "test-model");
  assert.equal(loaded.shortcutSettings.shortcut, "control+space");
});

test("corrupt local values fall back without throwing", () => {
  window.localStorage.setItem("agent-input-layer.history", "not-json");
  window.localStorage.setItem("agent-input-layer.selected-mode", "unknown-mode");
  window.localStorage.setItem("agent-input-layer.provider-settings", "null");

  const loaded = loadLocalAppState();

  assert.deepEqual(loaded.history, []);
  assert.equal(loaded.selectedModeId, "codex-execution");
  assert.deepEqual(loaded.providerSettings, DEFAULT_PROVIDER_SETTINGS);
});

test("SQLite lock error falls back to local state and remains recoverable", async () => {
  const runtimeWindow = window as unknown as {
    __TAURI_INTERNALS__?: {
      invoke: (command: string) => Promise<unknown>;
    };
  };
  const warnings: unknown[][] = [];
  const originalWarn = console.warn;
  const fallback = {
    history: [],
    userRules: [{ id: "fallback-rule", text: "本地回退", enabled: true }],
    selectedModeId: "bug-fix" as const,
    providerSettings: DEFAULT_PROVIDER_SETTINGS,
    shortcutSettings: { shortcut: "control+space" },
  };

  runtimeWindow.__TAURI_INTERNALS__ = {
    invoke: async () => {
      throw new Error("database is locked");
    },
  };
  console.warn = (...args: unknown[]) => warnings.push(args);

  try {
    const loaded = await loadAppState(fallback);
    assert.deepEqual(loaded, fallback);

    await saveAppState(fallback);
    assert.deepEqual(loadLocalAppState(), {
      ...fallback,
      shortcutSettings: { shortcut: "control+space", mcpEnabled: false },
    });
    assert.equal(warnings.length, 2);
    assert.match(String(warnings[0][0]), /SQLite state/);
    assert.match(String(warnings[1][0]), /SQLite state/);
  } finally {
    console.warn = originalWarn;
    delete runtimeWindow.__TAURI_INTERNALS__;
  }
});

test("desktop fallback never persists Provider secrets and Keychain errors stay readable", async () => {
  const runtimeWindow = window as unknown as {
    __TAURI_INTERNALS__?: {
      invoke: (command: string) => Promise<unknown>;
    };
  };
  const secret = "synthetic-desktop-fallback-secret";
  const fallback = {
    history: [],
    userRules: [],
    selectedModeId: "bug-fix" as const,
    providerSettings: {
      ...DEFAULT_PROVIDER_SETTINGS,
      enabled: true,
      baseUrl: "https://provider.invalid/v1",
      model: "test",
      apiKey: secret,
    },
    shortcutSettings: { shortcut: "alt+space" },
  };
  const originalWarn = console.warn;
  console.warn = () => {};

  try {
    runtimeWindow.__TAURI_INTERNALS__ = {
      invoke: async () => {
        throw new Error("database is locked");
      },
    };
    const loaded = await loadAppState(fallback);
    assert.equal(loaded.providerSettings.enabled, false);
    assert.equal(loaded.providerSettings.apiKey, "");
    await saveAppState(fallback);
    assert.doesNotMatch(JSON.stringify(loadLocalAppState()), new RegExp(secret));

    runtimeWindow.__TAURI_INTERNALS__ = {
      invoke: async () => {
        throw new Error(`无法读取 macOS 钥匙串：${secret}`);
      },
    };
    await assert.rejects(
      loadAppState(fallback),
      (error: Error) =>
        error.message.includes("无法读取 macOS 钥匙串") &&
        !error.message.includes(secret),
    );
  } finally {
    console.warn = originalWarn;
    delete runtimeWindow.__TAURI_INTERNALS__;
  }
});

test("successful Tauri load and save keep the normalized frontend-Rust state contract", async () => {
  const runtimeWindow = window as unknown as {
    __TAURI_INTERNALS__?: {
      invoke: (command: string, args?: Record<string, unknown>) => Promise<unknown>;
    };
  };
  const invocations: Array<{ command: string; args?: Record<string, unknown> }> = [];
  const fallback = {
    history: [],
    userRules: [{ id: "fallback-rule", text: "fallback", enabled: true }],
    selectedModeId: "codex-execution" as const,
    providerSettings: DEFAULT_PROVIDER_SETTINGS,
    shortcutSettings: { shortcut: "alt+space" },
  };
  const nativeState = {
    history: Array.from({ length: 55 }, (_, index) => ({
      id: `native-history-${index}`,
      rawInput: `native-raw-${index}`,
      modeId: "bug-fix" as const,
      output: `native-output-${index}`,
      favorite: index === 0,
      createdAt: `2026-07-13T00:00:${String(index).padStart(2, "0")}.000Z`,
    })),
    userRules: [{ id: "native-rule", text: "native state", enabled: true }],
    selectedModeId: "bug-fix" as const,
    providerSettings: {
      enabled: true,
      baseUrl: "https://provider.invalid/v1",
      model: "native-model",
      apiKey: "synthetic-native-key",
    },
    shortcutSettings: { shortcut: "control+space" },
  };

  runtimeWindow.__TAURI_INTERNALS__ = {
    invoke: async (command, args) => {
      invocations.push({ command, args });
      if (command === "load_app_state") return nativeState;
      if (command === "save_app_state") return undefined;
      throw new Error(`unexpected command: ${command}`);
    },
  };

  try {
    const loaded = await loadAppState(fallback);
    assert.equal(loaded.history.length, 50);
    assert.equal(loaded.history[0].id, "native-history-0");
    assert.equal(loaded.userRules[0].id, "native-rule");
    assert.equal(loaded.selectedModeId, "bug-fix");
    assert.equal(loaded.providerSettings.model, "native-model");
    assert.equal(loaded.shortcutSettings.shortcut, "control+space");

    await saveAppState(loaded);
    assert.equal(invocations.length, 2);
    assert.equal(invocations[0].command, "load_app_state");
    assert.deepEqual(invocations[0].args, { fallback });
    assert.equal(invocations[1].command, "save_app_state");
    assert.deepEqual(invocations[1].args, { state: loaded });
    assert.equal(window.localStorage.length, 0);
  } finally {
    delete runtimeWindow.__TAURI_INTERNALS__;
  }
});
