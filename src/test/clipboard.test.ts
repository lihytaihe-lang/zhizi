import assert from "node:assert/strict";
import { afterEach, test } from "node:test";
import { copyText } from "../lib/clipboard";

const originalClipboard = navigator.clipboard;
const originalExecCommand = document.execCommand;
const runtimeWindow = window as unknown as {
  __TAURI_INTERNALS__?: {
    invoke: (
      command: string,
      args?: Record<string, unknown>,
      options?: unknown,
    ) => Promise<unknown>;
  };
};
const originalTauriInternals = runtimeWindow.__TAURI_INTERNALS__;

afterEach(() => {
  Object.defineProperty(navigator, "clipboard", {
    configurable: true,
    value: originalClipboard,
  });
  Object.defineProperty(document, "execCommand", {
    configurable: true,
    value: originalExecCommand,
  });

  if (originalTauriInternals === undefined) {
    delete runtimeWindow.__TAURI_INTERNALS__;
  } else {
    runtimeWindow.__TAURI_INTERNALS__ = originalTauriInternals;
  }
});

test("browser clipboard path copies non-empty legacy Prompt output", async () => {
  const writes: string[] = [];
  Object.defineProperty(navigator, "clipboard", {
    configurable: true,
    value: {
      writeText: async (text: string) => {
        writes.push(text);
      },
    },
  });

  assert.equal(await copyText("结构化任务指令"), true);
  assert.deepEqual(writes, ["结构化任务指令"]);
  assert.equal(await copyText("   "), false);
  assert.deepEqual(writes, ["结构化任务指令"]);
});

test("Tauri clipboard plugin copies legacy Prompt output without using browser fallback", async () => {
  const invocations: Array<{
    command: string;
    args?: Record<string, unknown>;
  }> = [];
  const browserWrites: string[] = [];

  runtimeWindow.__TAURI_INTERNALS__ = {
    invoke: async (command, args) => {
      invocations.push({ command, args });
    },
  };
  Object.defineProperty(navigator, "clipboard", {
    configurable: true,
    value: {
      writeText: async (text: string) => {
        browserWrites.push(text);
      },
    },
  });

  assert.equal(await copyText("Tauri 原生复制"), true);
  assert.deepEqual(invocations, [
    {
      command: "plugin:clipboard-manager|write_text",
      args: { label: undefined, text: "Tauri 原生复制" },
    },
  ]);
  assert.deepEqual(browserWrites, []);
});

test("Tauri clipboard plugin failure falls back to the browser clipboard", async () => {
  const invocations: Array<{
    command: string;
    args?: Record<string, unknown>;
  }> = [];
  const browserWrites: string[] = [];

  runtimeWindow.__TAURI_INTERNALS__ = {
    invoke: async (command, args) => {
      invocations.push({ command, args });
      throw new Error("clipboard plugin unavailable");
    },
  };
  Object.defineProperty(navigator, "clipboard", {
    configurable: true,
    value: {
      writeText: async (text: string) => {
        browserWrites.push(text);
      },
    },
  });

  assert.equal(await copyText("回退复制"), true);
  assert.deepEqual(invocations, [
    {
      command: "plugin:clipboard-manager|write_text",
      args: { label: undefined, text: "回退复制" },
    },
  ]);
  assert.deepEqual(browserWrites, ["回退复制"]);
});

test("all clipboard paths failing returns false for manual-copy UI", async () => {
  runtimeWindow.__TAURI_INTERNALS__ = {
    invoke: async () => {
      throw new Error("synthetic Tauri clipboard failure");
    },
  };
  Object.defineProperty(navigator, "clipboard", {
    configurable: true,
    value: {
      writeText: async () => {
        throw new Error("synthetic browser clipboard failure");
      },
    },
  });
  Object.defineProperty(document, "execCommand", {
    configurable: true,
    value: () => false,
  });

  assert.equal(await copyText("需要手动复制的结果"), false);
});
