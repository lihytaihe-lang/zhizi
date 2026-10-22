import assert from "node:assert/strict";
import { test } from "node:test";
import { act, isValidElement, type ReactElement, type ReactNode } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import App from "../App";
import { SettingsPanel } from "../components/SettingsPanel";
import { HistoryList } from "../components/HistoryList";
import { ResultPanel } from "../components/ResultPanel";
import { CompactInputPanel } from "../components/compact/CompactInputPanel";
import { BUILT_IN_MODES, DEFAULT_USER_RULES } from "../domain/modes";
import { loadLocalAppState, saveLocalAppState } from "../lib/storage";
import {
  findButton,
  installMiniDom,
  MiniEvent,
  MiniMouseEvent,
  type MiniElement,
} from "./support/miniDom";

type ElementProps = Record<string, unknown> & { children?: ReactNode };

function findElement(
  node: ReactNode,
  predicate: (element: ReactElement<ElementProps>) => boolean,
): ReactElement<ElementProps> | undefined {
  if (Array.isArray(node)) {
    for (const child of node) {
      const match = findElement(child, predicate);
      if (match) return match;
    }
    return undefined;
  }

  if (!isValidElement<ElementProps>(node)) return undefined;
  if (predicate(node)) return node;
  return findElement(node.props.children, predicate);
}

function keyboardEvent(key: string, options: { metaKey?: boolean; shiftKey?: boolean } = {}) {
  let prevented = false;
  let stopped = false;

  return {
    key,
    metaKey: options.metaKey ?? false,
    shiftKey: options.shiftKey ?? false,
    preventDefault: () => {
      prevented = true;
    },
    stopPropagation: () => {
      stopped = true;
    },
    get prevented() {
      return prevented;
    },
    get stopped() {
      return stopped;
    },
  };
}

async function click(element: MiniElement) {
  await act(async () => {
    element.dispatchEvent(new MiniMouseEvent("click", { bubbles: true, cancelable: true }));
  });
}

async function enterText(element: MiniElement, value: string) {
  await act(async () => {
    element.value = value;
    element.dispatchEvent(new MiniEvent("input", { bubbles: true, cancelable: true }));
  });
}

test("main workspace result and history surfaces retain visible legacy Prompt actions", () => {
  const copied: string[] = [];
  const removed: string[] = [];
  const favorited: string[] = [];
  const history = [{
    id: "history-1",
    rawInput: "保留 v0.1 工作台",
    modeId: "codex-execution" as const,
    targetTool: "Codex",
    output: "结构化任务指令",
    favorite: false,
    createdAt: "2026-07-12T00:00:00.000Z",
  }];

  const historyTree = HistoryList({
    history,
    onCopy: (output) => copied.push(output),
    onRemove: (id) => removed.push(id),
    onToggleFavorite: (id) => favorited.push(id),
  });
  const resultTree = ResultPanel({
    output: history[0].output,
    copyState: "idle",
    onCopy: () => copied.push(history[0].output),
  });
  const markup = renderToStaticMarkup(<>{resultTree}{historyTree}</>);

  assert.match(markup, /生成结果/);
  assert.match(markup, /结构化任务指令/);
  assert.match(markup, /最近生成/);
  assert.match(markup, /保留 v0\.1 工作台/);

  const historyCopy = findElement(historyTree, (element) => element.props["aria-label"] === "复制历史输出");
  const historyRemove = findElement(historyTree, (element) => element.props["aria-label"] === "删除历史");
  const historyFavorite = findElement(historyTree, (element) => element.props["aria-label"] === "收藏");
  const resultCopy = findElement(resultTree, (element) => element.type === "button");

  assert.ok(historyCopy && historyRemove && historyFavorite && resultCopy);
  (historyCopy.props.onClick as () => void)();
  (historyRemove.props.onClick as () => void)();
  (historyFavorite.props.onClick as () => void)();
  (resultCopy.props.onClick as () => void)();

  assert.deepEqual(copied, ["结构化任务指令", "结构化任务指令"]);
  assert.deepEqual(removed, ["history-1"]);
  assert.deepEqual(favorited, ["history-1"]);

  const failedResultMarkup = renderToStaticMarkup(
    <ResultPanel output={history[0].output} copyState="failed" onCopy={() => {}} />,
  );
  assert.match(failedResultMarkup, /复制失败/);
});

test("compact panel exposes explicit replace insert copy actions while existing shortcuts keep priority", () => {
  const actions: string[] = [];
  const baseProps = {
    copyState: "idle" as const,
    externalTextAccess: {
      authorized: true,
      targetAvailable: true,
      actionsEnabled: true,
      status: "ready" as const,
      message: "只有点击动作后才会写回。",
    },
    generateState: "idle" as const,
    generationNotice: "使用本地模板。",
    modes: BUILT_IN_MODES,
    onClose: () => actions.push("close"),
    onCopy: () => actions.push("copy"),
    onCopyAndClose: () => actions.push("copy-close"),
    onExternalTextAction: (action: "replace" | "insert") => actions.push(action),
    onGenerate: () => actions.push("generate"),
    onTogglePreview: () => actions.push("preview"),
    onRawInputChange: (value: string) => actions.push(`input:${value}`),
    onSelectMode: (modeId: (typeof BUILT_IN_MODES)[number]["id"]) => actions.push(`mode:${modeId}`),
    rawInput: "实现离线输入法",
    lastResultAction: "copy" as const,
    payloadPreview: { payload: "完整 payload", characterCount: 10, profileEntryCount: 0, projectTermCount: 0, preferenceCount: 0 },
    previewExpanded: false,
    rules: DEFAULT_USER_RULES,
    selectionMessage: "可手动粘贴",
    selectedMode: BUILT_IN_MODES[0],
    validationMessage: "",
  };
  const inputTree = CompactInputPanel({ ...baseProps, output: "" });
  const inputMarkup = renderToStaticMarkup(inputTree);

  assert.match(inputMarkup, /Agent Input Method 输入面板/);
  assert.match(inputMarkup, /实现离线输入法/);
  assert.match(inputMarkup, /Enter 生成/);
  assert.match(inputMarkup, /aria-label="关闭输入面板"/);

  const closeButton = findElement(inputTree, (element) => element.props["aria-label"] === "关闭输入面板");
  assert.ok(closeButton);
  (closeButton.props.onClick as () => void)();

  const textarea = findElement(inputTree, (element) => element.props.id === "compact-raw-input");
  assert.ok(textarea);
  const handleKeyDown = textarea.props.onKeyDown as (event: ReturnType<typeof keyboardEvent>) => void;

  const generateEvent = keyboardEvent("Enter");
  handleKeyDown(generateEvent);
  const copyEvent = keyboardEvent("Enter", { metaKey: true });
  handleKeyDown(copyEvent);
  const copyCloseEvent = keyboardEvent("Enter", { metaKey: true, shiftKey: true });
  handleKeyDown(copyCloseEvent);
  const closeEvent = keyboardEvent("Escape");
  handleKeyDown(closeEvent);

  assert.deepEqual(actions, ["close", "generate", "copy", "copy-close", "close"]);
  assert.ok(generateEvent.prevented && copyEvent.prevented && copyCloseEvent.prevented && closeEvent.prevented);

  const outputTree = CompactInputPanel({
    ...baseProps,
    copyState: "copied",
    output: "离线日用结果",
  });
  const outputMarkup = renderToStaticMarkup(outputTree);
  assert.match(outputMarkup, /离线日用结果/);
  assert.match(outputMarkup, /已复制/);
  assert.match(outputMarkup, /替换/);
  assert.match(outputMarkup, /插入/);
  assert.match(outputMarkup, /⌘↵ 复制（既有）/);

  const replace = findElement(outputTree, (element) => element.type === "button" && element.props.children === "替换");
  const insert = findElement(outputTree, (element) => element.type === "button" && element.props.children === "插入");
  const panel = findElement(outputTree, (element) => element.type === "main");
  assert.ok(replace && insert && panel);
  (replace.props.onClick as () => void)();
  (insert.props.onClick as () => void)();
  const panelKeyDown = panel.props.onKeyDown as (event: ReturnType<typeof keyboardEvent>) => void;
  panelKeyDown(keyboardEvent("c", { metaKey: true }));
  panelKeyDown(keyboardEvent("Enter", { metaKey: true }));
  panelKeyDown(keyboardEvent("Enter", { metaKey: true, shiftKey: true }));
  assert.deepEqual(actions.slice(-5), ["replace", "insert", "copy", "copy", "copy-close"]);

  const deniedMarkup = renderToStaticMarkup(
    <CompactInputPanel
      {...baseProps}
      externalTextAccess={{
        authorized: false,
        targetAvailable: true,
        actionsEnabled: false,
        status: "permission-denied",
        message: "请在系统设置授权；复制始终可用。",
      }}
      output="离线日用结果"
    />,
  );
  assert.match(deniedMarkup, /替换[^]*disabled|disabled[^]*替换/);
  assert.match(deniedMarkup, /插入[^]*disabled|disabled[^]*插入/);
  assert.match(deniedMarkup, /复制始终可用/);

  const failedOutputMarkup = renderToStaticMarkup(
    <CompactInputPanel {...baseProps} copyState="failed" output="离线日用结果" />,
  );
  assert.match(failedOutputMarkup, /复制失败，请手动选择结果复制/);
});

test("mounted App keeps main generation, copy, history, favorite, delete, storage, and Provider fallback", async () => {
  const dom = installMiniDom();
  const { createRoot } = await import("react-dom/client");
  const root = createRoot(dom.container as unknown as Element);
  (globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  const clipboardWrites: string[] = [];
  const warnings: unknown[][] = [];
  const originalWarn = console.warn;

  window.localStorage.clear();
  saveLocalAppState({
    history: [{
      id: "persisted-history",
      rawInput: "重启前保存的任务",
      modeId: "bug-fix",
      targetTool: "Codex",
      output: "重启前的结构化结果",
      favorite: false,
      createdAt: "2026-07-12T00:00:00.000Z",
    }],
    userRules: [{ id: "persisted-rule", text: "必须保留验证步骤", enabled: true }],
    selectedModeId: "codex-execution",
    providerSettings: {
      enabled: true,
      baseUrl: "https://provider.invalid/v1",
      model: "offline-test-model",
      apiKey: "provider-secret-must-not-render",
    },
    shortcutSettings: { shortcut: "control+space" },
  });
  Object.defineProperty(navigator, "clipboard", {
    configurable: true,
    value: {
      writeText: async (text: string) => clipboardWrites.push(text),
    },
  });
  console.warn = (...args: unknown[]) => warnings.push(args);

  try {
    await act(async () => {
      root.render(<App />);
    });

    assert.match(dom.container.textContent, /知字/);
    assert.match(dom.container.textContent, /常用开关/);
    const advancedTab = findButton(dom.container, "高级");
    assert.ok(advancedTab);
    await click(advancedTab);
    assert.match(dom.container.textContent, /重启前保存的任务/);
    assert.match(dom.container.textContent, /模型增强已启用/);

    const input = dom.document.getElementById("raw-input");
    assert.ok(input);
    await enterText(input, "实现一个可离线工作的输入法");

    const generate = findButton(dom.container, "生成开发任务");
    assert.ok(generate);
    await click(generate);

    assert.match(dom.container.textContent, /实现一个可离线工作的输入法/);
    assert.match(dom.container.textContent, /模型调用失败，已使用本地模板/);
    assert.doesNotMatch(dom.container.textContent, /provider-secret-must-not-render/);
    assert.doesNotMatch(JSON.stringify(warnings), /provider-secret-must-not-render/);

    const resultCopy = findButton(dom.container, "复制");
    assert.ok(resultCopy);
    await click(resultCopy);
    assert.equal(clipboardWrites.length, 1);
    assert.match(clipboardWrites[0], /实现一个可离线工作的输入法/);
    assert.ok(findButton(dom.container, "已复制"));

    const historyItems = dom.container.querySelectorAll("article");
    assert.equal(historyItems.length, 2);
    const newestHistory = historyItems.find((item) =>
      item.textContent.includes("实现一个可离线工作的输入法"));
    const persistedHistory = historyItems.find((item) =>
      item.textContent.includes("重启前保存的任务"));
    assert.ok(newestHistory && persistedHistory);

    const favorite = newestHistory.querySelector('[aria-label="收藏"]');
    const historyCopy = persistedHistory.querySelector('[aria-label="复制历史输出"]');
    const remove = persistedHistory.querySelector('[aria-label="删除历史"]');
    assert.ok(favorite && historyCopy && remove);

    await click(favorite);
    assert.match(favorite.className, /is-active/);
    await click(historyCopy);
    assert.equal(clipboardWrites.at(-1), "重启前的结构化结果");
    await click(remove);
    assert.doesNotMatch(dom.container.textContent, /重启前保存的任务/);

    const stored = loadLocalAppState();
    assert.equal(stored.history.length, 1);
    assert.equal(stored.history[0].rawInput, "实现一个可离线工作的输入法");
    assert.equal(stored.history[0].favorite, true);
    assert.equal(stored.userRules[0].text, "必须保留验证步骤");
  } finally {
    await act(async () => root.unmount());
    console.warn = originalWarn;
    dom.cleanup();
  }
});

test("mounted App keeps compact generation preview-only until an explicit copy and persists history", async () => {
  const dom = installMiniDom("?surface=compact");
  const { createRoot } = await import("react-dom/client");
  const root = createRoot(dom.container as unknown as Element);
  (globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  const clipboardWrites: string[] = [];

  window.localStorage.clear();
  Object.defineProperty(navigator, "clipboard", {
    configurable: true,
    value: {
      writeText: async (text: string) => clipboardWrites.push(text),
    },
  });

  try {
    await act(async () => {
      root.render(<App />);
    });

    assert.match(dom.container.textContent, /Option \+ Space/);
    const input = dom.document.getElementById("compact-raw-input");
    assert.ok(input);
    await enterText(input, "把现有功能整理成验收任务");

    const generate = findButton(dom.container, "生成任务指令");
    assert.ok(generate);
    await click(generate);

    assert.match(dom.container.textContent, /把现有功能整理成验收任务/);
    assert.match(dom.container.textContent, /已使用本地模板生成/);
    assert.match(dom.container.textContent, /尚未写入外部 App 或剪贴板/);
    assert.equal(clipboardWrites.length, 0);

    const copy = findButton(dom.container, "复制");
    assert.ok(copy);
    await click(copy);
    assert.equal(clipboardWrites.length, 1);
    assert.match(clipboardWrites[0], /把现有功能整理成验收任务/);

    const replace = findButton(dom.container, "替换");
    const insert = findButton(dom.container, "插入");
    assert.ok(replace && insert);
    assert.equal(replace.disabled, true);
    assert.equal(insert.disabled, true);
    assert.match(dom.container.textContent, /复制仍可用/);

    const stored = loadLocalAppState();
    assert.equal(stored.history.length, 1);
    assert.equal(stored.history[0].rawInput, "把现有功能整理成验收任务");
    assert.match(stored.history[0].output, /把现有功能整理成验收任务/);
  } finally {
    await act(async () => root.unmount());
    dom.cleanup();
  }
});

test("context model setting is off and disabled until a verified local bundle is available", () => {
  const html = renderToStaticMarkup(<SettingsPanel
    providerSettings={{ enabled: false, baseUrl: "", model: "", apiKey: "" }}
    shortcutSettings={{ shortcut: "control+space" }} settingsMessage=""
    onProviderChange={() => {}} onShortcutChange={() => {}} onApplyShortcut={() => {}}
    mcpStatus={{ enabled: false, endpoint: null }}
  />);
  assert.match(html, /上下文模型排序（实验）/);
  assert.match(html, /实验功能：用本机小模型按上下文调整同音字顺序，不联网/);
  assert.match(html, /type="checkbox" disabled=""/);
  assert.match(html, /正在检查本机模型文件/);
});
