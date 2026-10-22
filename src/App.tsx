import { useEffect, useMemo, useState } from "react";
import { BookMarked, BookOpen, Database, Home, ListChecks, Loader2, MessageSquare, Settings, SlidersHorizontal, WandSparkles, Wrench } from "lucide-react";
import "./App.css";
import { CompactInputPanel } from "./components/compact/CompactInputPanel";
import { HistoryList } from "./components/HistoryList";
import { ModeSelector } from "./components/ModeSelector";
import { ResultPanel } from "./components/ResultPanel";
import { RuleList } from "./components/RuleList";
import { SettingsPanel } from "./components/SettingsPanel";
import { HomePanel } from "./components/HomePanel";
import { QualityRecordingPanel } from "./components/QualityRecordingPanel";
import { LexiconManager } from "./components/LexiconManager";
import { DataSourceManager } from "./components/DataSourceManager";
import { KnowledgeManager } from "./components/KnowledgeManager";
import { MaintenanceManager } from "./components/MaintenanceManager";
import { BUILT_IN_MODES } from "./domain/modes";
import { getModeTargetTool } from "./domain/modeTargets";
import type {
  CopyState,
  ExternalTextAccess,
  ExternalTextAction,
  GenerateState,
  HistoryItem,
  ModeId,
  PayloadPreview,
  ProviderSettings,
  ResultAction,
  ShortcutSettings,
  UserRule,
} from "./domain/types";
import { copyText } from "./lib/clipboard";
import { applyPanelShortcut } from "./lib/nativeSettings";
import {
  getCurrentWindowLabel,
  hideCurrentWindow,
  listenForPanelFocus,
  resizeCurrentWindow,
} from "./lib/nativeWindow";
import { isProviderReady } from "./lib/providerClient";
import { generatePromptWithFallback } from "./lib/generation";
import { composePrompt } from "./lib/promptComposer";
import {
  buildBrowserPayloadPreview,
  externalTextAccessStatus,
  getMcpServerStatus,
  latestSelectionCapture,
  performExternalTextAction,
  preparePayloadPreview,
  setMcpServerEnabled,
  type McpServerStatus,
} from "./lib/layer2";
import { loadAppState, loadLocalAppState, saveAppState } from "./lib/storage";
import { stageGeneratedHotwordCandidates } from "./lib/knowledge";
import { loadLastResultAction, saveLastResultAction } from "./lib/resultAction";

type ViewId = "home" | "records" | "compose" | "lexicon" | "sources" | "knowledge" | "maintenance" | "settings";
/** Everything outside the input method's daily use sits under 高级 (owner 2026-10-10). */
const ADVANCED_VIEWS: ViewId[] = ["compose", "knowledge", "sources", "maintenance", "settings"];
type WindowSurface = "main" | "compact";

const UNAVAILABLE_EXTERNAL_TEXT_ACCESS: ExternalTextAccess = {
  authorized: false,
  targetAvailable: false,
  actionsEnabled: false,
  status: "unsupported",
  message: "正在检查辅助功能权限；复制始终可用。",
};

function createId(): string {
  return typeof crypto.randomUUID === "function"
    ? crypto.randomUUID()
    : `${Date.now()}-${Math.random().toString(16).slice(2)}`;
}

function getErrorMessage(error: unknown): string {
  if (error instanceof Error) {
    return error.message;
  }

  if (typeof error === "string") {
    return error;
  }

  return "未知错误";
}

function getShortcutLabel(shortcut: string): string {
  switch (shortcut) {
    case "super+shift+space":
      return "Command + Shift + Space";
    case "control+space":
      return "Control + Space";
    case "alt+space":
    default:
      return "Option + Space";
  }
}

function App() {
  const [initialAppState] = useState(() => loadLocalAppState());
  const [windowSurface, setWindowSurface] = useState<WindowSurface | null>(null);
  const [view, setView] = useState<ViewId>("home");
  const advanced = ADVANCED_VIEWS.includes(view);
  const [rawInput, setRawInput] = useState("");
  const [selectedModeId, setSelectedModeId] = useState<ModeId>(
    initialAppState.selectedModeId,
  );
  const [output, setOutput] = useState("");
  const [history, setHistory] = useState<HistoryItem[]>(initialAppState.history);
  const [rules, setRules] = useState<UserRule[]>(initialAppState.userRules);
  const [providerSettings, setProviderSettings] = useState<ProviderSettings>(
    initialAppState.providerSettings,
  );
  const [shortcutSettings, setShortcutSettings] = useState<ShortcutSettings>(
    initialAppState.shortcutSettings,
  );
  const [draftRule, setDraftRule] = useState("");
  const [copyState, setCopyState] = useState<CopyState>("idle");
  const [generateState, setGenerateState] = useState<GenerateState>("idle");
  const [generationNotice, setGenerationNotice] = useState("");
  const [settingsMessage, setSettingsMessage] = useState("");
  const [validationMessage, setValidationMessage] = useState("");
  const [appClass, setAppClass] = useState<"Code" | "Prose" | "Browser" | "Unknown">("Unknown");
  const [contextAllowed, setContextAllowed] = useState(false);
  const [selectionMessage, setSelectionMessage] = useState("");
  const [externalTextAccess, setExternalTextAccess] = useState<ExternalTextAccess>(
    UNAVAILABLE_EXTERNAL_TEXT_ACCESS,
  );
  const [lastResultAction, setLastResultAction] = useState<ResultAction>(
    () => loadLastResultAction(),
  );
  const [previewExpanded, setPreviewExpanded] = useState(false);
  const [mcpStatus, setMcpStatus] = useState<McpServerStatus>({ enabled: false, endpoint: null });

  const selectedMode = useMemo(
    () => BUILT_IN_MODES.find((mode) => mode.id === selectedModeId) ?? BUILT_IN_MODES[0],
    [selectedModeId],
  );
  const providerReady = useMemo(() => isProviderReady(providerSettings), [providerSettings]);
  const [storageReady, setStorageReady] = useState(false);
  const localPrompt = useMemo(() => composePrompt({
    rawInput,
    mode: selectedMode,
    userRules: rules,
    compact: windowSurface === "compact",
  }), [rawInput, rules, selectedMode, windowSurface]);
  const [payloadPreview, setPayloadPreview] = useState<PayloadPreview>(() => buildBrowserPayloadPreview({
    rawInput: "",
    mode: BUILT_IN_MODES[0],
    userRules: initialAppState.userRules,
    localPrompt: "",
    compact: false,
    appClass: "Unknown",
    contextAllowed: false,
  }));

  function resetCompactSession() {
    setRawInput("");
    setOutput("");
    setValidationMessage("");
    setGenerationNotice("");
    setGenerateState("idle");
    setCopyState("idle");
  }

  useEffect(() => {
    let cancelled = false;

    void getCurrentWindowLabel().then((label) => {
      if (cancelled) {
        return;
      }

      const surface: WindowSurface = label === "compact" ? "compact" : "main";
      document.documentElement.dataset.windowSurface = surface;
      setWindowSurface(surface);
    });

    return () => {
      cancelled = true;
      delete document.documentElement.dataset.windowSurface;
    };
  }, []);

  useEffect(() => {
    let cancelled = false;

    void loadAppState(initialAppState).then((state) => {
      if (cancelled) {
        return;
      }

      setHistory(state.history);
      setRules(state.userRules);
      setSelectedModeId(state.selectedModeId);
      setProviderSettings(state.providerSettings);
      setShortcutSettings(state.shortcutSettings);
      setStorageReady(true);
    }).catch((error) => {
      if (cancelled) {
        return;
      }
      setProviderSettings({
        ...initialAppState.providerSettings,
        enabled: false,
        apiKey: "",
        anthropicApiKey: "",
      });
      setSettingsMessage(getErrorMessage(error));
    });

    return () => {
      cancelled = true;
    };
  }, [initialAppState]);

  useEffect(() => {
    if (!storageReady) return;
    void setMcpServerEnabled(Boolean(shortcutSettings.mcpEnabled))
      .then(setMcpStatus)
      .catch(() => getMcpServerStatus().then(setMcpStatus).catch(() => {}));
  }, [shortcutSettings.mcpEnabled, storageReady]);

  useEffect(() => {
    if (!storageReady) {
      return;
    }

    void saveAppState({
      history,
      userRules: rules,
      selectedModeId,
      providerSettings,
      shortcutSettings,
    }).catch((error) => setSettingsMessage(getErrorMessage(error)));
  }, [history, providerSettings, rules, selectedModeId, shortcutSettings, storageReady]);

  useEffect(() => {
    if (copyState === "idle") {
      return;
    }

    const timer = window.setTimeout(() => setCopyState("idle"), 1600);
    return () => window.clearTimeout(timer);
  }, [copyState]);

  useEffect(() => {
    if (!windowSurface) {
      return;
    }

    let unlisten = () => {};
    const inputId = windowSurface === "compact" ? "compact-raw-input" : "raw-input";

    void listenForPanelFocus(() => {
      window.setTimeout(() => document.getElementById(inputId)?.focus(), 40);
      if (windowSurface === "compact") {
        void latestSelectionCapture().then((capture) => {
          setAppClass(capture.appClass);
          setContextAllowed(capture.status !== "secure" && capture.status !== "permission-denied" && capture.status !== "unsupported" && capture.appClass !== "Unknown");
          setSelectionMessage(capture.message);
          if (capture.status === "captured" && capture.text.trim()) {
            setRawInput(capture.text);
            setOutput("");
            setGenerateState("idle");
            setGenerationNotice("");
          }
        }).catch(() => {
          setAppClass("Unknown");
          setContextAllowed(false);
          setSelectionMessage("未能读取前台选区，小面板仍可手动粘贴。");
        });
        void externalTextAccessStatus()
          .then(setExternalTextAccess)
          .catch(() => setExternalTextAccess(UNAVAILABLE_EXTERNAL_TEXT_ACCESS));
      }
    }).then((nextUnlisten) => {
      unlisten = nextUnlisten;
    });

    return () => unlisten();
  }, [windowSurface]);

  useEffect(() => {
    if (windowSurface !== "compact") {
      return;
    }
    let cancelled = false;
    const refresh = () => {
      void externalTextAccessStatus()
        .then((status) => {
          if (!cancelled) setExternalTextAccess(status);
        })
        .catch(() => {
          if (!cancelled) setExternalTextAccess(UNAVAILABLE_EXTERNAL_TEXT_ACCESS);
        });
    };
    refresh();
    const timer = window.setInterval(refresh, 1000);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [windowSurface]);

  useEffect(() => {
    let cancelled = false;
    void preparePayloadPreview({
      rawInput,
      mode: selectedMode,
      userRules: rules,
      localPrompt,
      compact: windowSurface === "compact",
      appClass,
      contextAllowed,
    }).then((preview) => { if (!cancelled) setPayloadPreview(preview); });
    return () => { cancelled = true; };
  }, [appClass, contextAllowed, localPrompt, rawInput, rules, selectedMode, windowSurface]);

  useEffect(() => {
    if (windowSurface !== "compact") {
      return;
    }

    const hasOutput = output.trim().length > 0;
    void resizeCurrentWindow(600, hasOutput ? 428 : previewExpanded ? 500 : 340);
  }, [output, previewExpanded, windowSurface]);

  async function handleGenerate() {
    if (generateState === "generating") {
      return;
    }

    if (!rawInput.trim()) {
      setValidationMessage("先输入一句想法，再生成任务指令。");
      return;
    }

    setValidationMessage("");
    setGenerationNotice("");
    if (providerReady) {
      setGenerateState("generating");
    }

    const preview = await preparePayloadPreview({
      rawInput,
      mode: selectedMode,
      userRules: rules,
      localPrompt,
      compact: windowSurface === "compact",
      appClass,
      contextAllowed,
    });
    setPayloadPreview(preview);
    const result = await generatePromptWithFallback({
      rawInput,
      mode: selectedMode,
      userRules: rules,
      provider: providerSettings,
      compact: windowSurface === "compact",
      payload: preview.payload,
    });
    const nextOutput = result.output;
    const nextGenerateState: GenerateState = result.source;
    let notice = result.notice;
    if (selectedMode.id === "hotword-candidates") {
      try {
        const staged = await stageGeneratedHotwordCandidates(nextOutput);
        notice = staged.stagedCount > 0
          ? `${notice} 已送入知识页同一审核队列 ${staged.stagedCount} 条；缺拼音 ${staged.missingPinyinCount} 条、格式无效 ${staged.invalidCount} 条。`
          : `${notice} 当前是本地手填模板；填好“词、拼音、释义”后再由该动作生成并送审。`;
      } catch (error) {
        notice = `${notice} 生成结果未入审核队列：${getErrorMessage(error)}`;
      }
    }

    const nextItem: HistoryItem = {
      id: createId(),
      rawInput: rawInput.trim(),
      modeId: selectedMode.id,
      targetTool: getModeTargetTool(selectedMode.id),
      output: nextOutput,
      favorite: false,
      createdAt: new Date().toISOString(),
    };

    setOutput(nextOutput);
    setHistory((items) => [nextItem, ...items].slice(0, 50));
    setGenerateState(nextGenerateState);
    if (windowSurface === "compact") {
      setCopyState("idle");
      notice = `${notice} 请预览后显式选择替换、插入或复制；尚未写入外部 App 或剪贴板。`;
    } else {
      setCopyState("idle");
    }
    setGenerationNotice(notice);
  }

  async function handleCopy(text = output) {
    if (!text.trim()) {
      return;
    }

    setLastResultAction("copy");
    saveLastResultAction("copy");
    const copied = await copyText(text);
    setCopyState(copied ? "copied" : "failed");
    if (windowSurface === "compact") {
      setGenerationNotice(copied ? "已复制结果。" : "复制失败，请手动选择结果复制。");
    }
  }

  async function handleCopyAndClose() {
    if (!output.trim()) {
      return;
    }

    setLastResultAction("copy");
    saveLastResultAction("copy");
    const copied = await copyText(output);
    setCopyState(copied ? "copied" : "failed");

    if (copied) {
      resetCompactSession();
      await hideCurrentWindow();
    }
  }

  async function handleExternalTextAction(action: ExternalTextAction) {
    if (!output.trim()) {
      return;
    }

    setLastResultAction(action);
    saveLastResultAction(action);
    try {
      const result = await performExternalTextAction(action, output);
      let notice = result.message;
      if (result.fallbackToClipboard) {
        const copied = await copyText(output);
        setCopyState(copied ? "copied" : "failed");
        notice = copied
          ? `${notice} 已复制结果，可手动粘贴。`
          : `${notice} 剪贴板回退也失败，请手动选择结果复制。`;
      } else {
        setCopyState("idle");
      }
      setGenerationNotice(notice);
      setExternalTextAccess(await externalTextAccessStatus());
    } catch (error) {
      const copied = await copyText(output);
      setCopyState(copied ? "copied" : "failed");
      setGenerationNotice(
        `${getErrorMessage(error)} ${copied ? "已退回剪贴板。" : "剪贴板回退也失败。"}`,
      );
    }
  }

  function handleSelectMode(modeId: ModeId) {
    setSelectedModeId(modeId);
    setValidationMessage("");
  }

  function handleCompactRawInputChange(value: string) {
    setRawInput(value);
    setValidationMessage("");

    if (output) {
      setOutput("");
      setGenerateState("idle");
      setGenerationNotice("");
    }
  }

  function handleCompactSelectMode(modeId: ModeId) {
    setSelectedModeId(modeId);
    setValidationMessage("");
    setOutput("");
    setGenerateState("idle");
    setGenerationNotice("");
  }

  async function handleApplyShortcut() {
    setSettingsMessage("正在应用快捷键...");

    try {
      const message = await applyPanelShortcut(shortcutSettings.shortcut);
      setSettingsMessage(message);
    } catch (error) {
      setSettingsMessage(`快捷键应用失败：${getErrorMessage(error)}`);
    }
  }

  function handleAddRule() {
    const text = draftRule.trim();

    if (!text) {
      return;
    }

    setRules((currentRules) => [
      ...currentRules,
      {
        id: createId(),
        text,
        enabled: true,
      },
    ]);
    setDraftRule("");
  }

  function handleToggleRule(id: string) {
    setRules((currentRules) =>
      currentRules.map((rule) =>
        rule.id === id
          ? {
              ...rule,
              enabled: !rule.enabled,
            }
          : rule,
      ),
    );
  }

  function handleRemoveRule(id: string) {
    setRules((currentRules) => currentRules.filter((rule) => rule.id !== id));
  }

  function handleRemoveHistory(id: string) {
    setHistory((items) => items.filter((item) => item.id !== id));
  }

  function handleToggleFavorite(id: string) {
    setHistory((items) =>
      items.map((item) =>
        item.id === id
          ? {
              ...item,
              favorite: !item.favorite,
            }
          : item,
      ),
    );
  }

  if (!windowSurface) {
    return null;
  }

  if (windowSurface === "compact") {
    return (
      <CompactInputPanel
        copyState={copyState}
        externalTextAccess={externalTextAccess}
        generateState={generateState}
        generationNotice={generationNotice}
        modes={BUILT_IN_MODES}
        onClose={() => void hideCurrentWindow()}
        onCopy={() => void handleCopy()}
        onCopyAndClose={() => void handleCopyAndClose()}
        onExternalTextAction={(action) => void handleExternalTextAction(action)}
        onGenerate={() => void handleGenerate()}
        onRawInputChange={handleCompactRawInputChange}
        onSelectMode={handleCompactSelectMode}
        output={output}
        lastResultAction={lastResultAction}
        payloadPreview={payloadPreview}
        previewExpanded={previewExpanded}
        rawInput={rawInput}
        rules={rules}
        selectionMessage={selectionMessage}
        selectedMode={selectedMode}
        validationMessage={validationMessage}
        onTogglePreview={() => setPreviewExpanded((expanded) => !expanded)}
      />
    );
  }

  return (
    <main className="app-shell">
      <header className="app-header">
        <div>
          <p className="product-kicker">知字输入法</p>
          <h1>知字</h1>
        </div>
        <div className="header-actions">
          <div className={`status-pill ${providerReady ? "is-model" : ""}`}>
            {providerReady
              ? "模型增强已启用"
              : `${getShortcutLabel(shortcutSettings.shortcut)} 唤起小面板`}
          </div>
          <div className="view-tabs" role="tablist" aria-label="主视图">
            {([
              ["home", "首页", <Home size={16} key="i" />],
              ["lexicon", "词库", <BookOpen size={16} key="i" />],
              ["records", "记录", <ListChecks size={16} key="i" />],
            ] as const).map(([id, label, icon]) => (
              <button aria-pressed={view === id} className={`view-tab ${view === id ? "is-active" : ""}`} key={id} onClick={() => setView(id)} type="button">{icon}{label}</button>
            ))}
            <button aria-pressed={advanced} className={`view-tab ${advanced ? "is-active" : ""}`} onClick={() => setView(advanced ? view : "compose")} type="button"><SlidersHorizontal size={16} />高级</button>
          </div>
          {advanced ? (
            <div className="view-tabs view-subtabs" role="tablist" aria-label="高级">
              {([
                ["compose", "生成", <MessageSquare size={15} key="i" />],
                ["knowledge", "知识", <BookMarked size={15} key="i" />],
                ["sources", "数据源", <Database size={15} key="i" />],
                ["maintenance", "维护", <Wrench size={15} key="i" />],
                ["settings", "更多设置", <Settings size={15} key="i" />],
              ] as const).map(([id, label, icon]) => (
                <button aria-pressed={view === id} className={`view-tab ${view === id ? "is-active" : ""}`} key={id} onClick={() => setView(id)} type="button">{icon}{label}</button>
              ))}
            </div>
          ) : null}
        </div>
      </header>

      <section className={`workspace ${view !== "compose" && view !== "settings" ? "is-lexicon" : ""}`}>
        <div className="primary-column">
          {view === "home" ? (
            <HomePanel onNavigate={(destination) => setView(destination === "advanced" ? "compose" : destination)} />
          ) : view === "records" ? (
            <QualityRecordingPanel />
          ) : view === "compose" ? (
            <>
              <section className="panel composer-panel" aria-labelledby="composer-title">
                <div className="panel-heading">
                  <div>
                    <h2 id="composer-title">生成工作台</h2>
                    <p>个人词/术语生成、改写、总结等动作都在这里；日常输入请用快捷键唤起 Compact Input Panel</p>
                  </div>
                </div>

                <ModeSelector
                  modes={BUILT_IN_MODES}
                  onSelect={handleSelectMode}
                  selectedModeId={selectedModeId}
                />

                <label className="input-label" htmlFor="raw-input">
                  原始想法
                </label>
                <textarea
                  autoFocus
                  id="raw-input"
                  onChange={(event) => {
                    setRawInput(event.currentTarget.value);
                    setValidationMessage("");
                  }}
                  onKeyDown={(event) => {
                    if (event.key === "Enter" && event.metaKey) {
                      event.preventDefault();
                      void handleCopy();
                      return;
                    }

                    if (event.key === "Enter" && !event.shiftKey) {
                      event.preventDefault();
                      void handleGenerate();
                      return;
                    }

                    if (event.key === "Escape") {
                      event.preventDefault();
                      void hideCurrentWindow();
                    }
                  }}
                  placeholder="例如：帮我做一个登录页；登录后跳首页失败；我想做一个输入法产品"
                  value={rawInput}
                />

                <div className="payload-preview-summary">
                  <button className="text-button" onClick={() => setPreviewExpanded((expanded) => !expanded)} type="button">
                    {previewExpanded ? "收起 payload" : "看看要发什么"}
                  </button>
                  <span>本次将发送 {payloadPreview.characterCount} 字符 / {payloadPreview.profileEntryCount} 条档案条目</span>
                </div>
                {previewExpanded ? <pre className="payload-preview">{payloadPreview.payload}</pre> : null}

                <div className="composer-footer">
                  <div>
                    <strong>{selectedMode.name}</strong>
                    <span>{selectedMode.description}</span>
                  </div>
                  <button
                    className="primary-button"
                    disabled={generateState === "generating"}
                    onClick={() => void handleGenerate()}
                    type="button"
                  >
                    {generateState === "generating" ? (
                      <Loader2 className="spin" size={18} />
                    ) : (
                      <WandSparkles size={18} />
                    )}
                    {generateState === "generating" ? "生成中" : selectedMode.actionLabel}
                  </button>
                </div>
                {validationMessage ? <p className="validation">{validationMessage}</p> : null}
                {generationNotice ? (
                  <p className={`generation-notice ${generateState === "fallback" ? "is-warning" : ""}`}>
                    {generationNotice}
                  </p>
                ) : null}
              </section>

              <ResultPanel copyState={copyState} onCopy={() => void handleCopy()} output={output} />
            </>
          ) : view === "lexicon" ? (
            <LexiconManager />
          ) : view === "sources" ? (
            <DataSourceManager />
          ) : view === "knowledge" ? (
            <KnowledgeManager />
          ) : view === "maintenance" ? (
            <MaintenanceManager />
          ) : (
            <SettingsPanel
              onApplyShortcut={() => void handleApplyShortcut()}
              onProviderChange={setProviderSettings}
              onShortcutChange={setShortcutSettings}
              providerSettings={providerSettings}
              settingsMessage={settingsMessage}
              shortcutSettings={shortcutSettings}
              mcpStatus={mcpStatus}
            />
          )}
        </div>

        {view === "compose" || view === "settings" ? <aside className="side-column">
          <RuleList
            draftRule={draftRule}
            onAddRule={handleAddRule}
            onDraftRuleChange={setDraftRule}
            onRemoveRule={handleRemoveRule}
            onToggleRule={handleToggleRule}
            rules={rules}
          />
          <HistoryList
            history={history}
            onCopy={(text) => void handleCopy(text)}
            onRemove={handleRemoveHistory}
            onToggleFavorite={handleToggleFavorite}
          />
        </aside> : null}
      </section>
    </main>
  );
}

export default App;
