import type { KeyboardEvent } from "react";
import { Clipboard, Loader2, RotateCcw, WandSparkles, X } from "lucide-react";
import type {
  CopyState,
  ExternalTextAccess,
  ExternalTextAction,
  GenerateState,
  Mode,
  ModeId,
  PayloadPreview,
  ResultAction,
  UserRule,
} from "../../domain/types";
import { getModeTargetTool } from "../../domain/modeTargets";

type CompactInputPanelProps = {
  copyState: CopyState;
  externalTextAccess: ExternalTextAccess;
  generateState: GenerateState;
  generationNotice: string;
  modes: Mode[];
  onClose: () => void;
  onCopy: () => void;
  onCopyAndClose: () => void;
  onExternalTextAction: (action: ExternalTextAction) => void;
  onGenerate: () => void;
  onRawInputChange: (value: string) => void;
  onSelectMode: (modeId: ModeId) => void;
  output: string;
  lastResultAction: ResultAction;
  payloadPreview: PayloadPreview;
  previewExpanded: boolean;
  rawInput: string;
  rules: UserRule[];
  selectionMessage: string;
  selectedMode: Mode;
  validationMessage: string;
  onTogglePreview: () => void;
};

function getRuleSummary(rules: UserRule[]): string {
  const enabledCount = rules.filter((rule) => rule.enabled && rule.text.trim()).length;
  return enabledCount > 0 ? `已应用 ${enabledCount} 条规则` : "未应用长期规则";
}

function getNextModeId(modes: Mode[], currentModeId: ModeId, direction: 1 | -1): ModeId {
  const currentIndex = modes.findIndex((mode) => mode.id === currentModeId);
  const safeIndex = currentIndex >= 0 ? currentIndex : 0;
  const nextIndex = (safeIndex + direction + modes.length) % modes.length;

  return modes[nextIndex].id;
}

export function CompactInputPanel({
  copyState,
  externalTextAccess,
  generateState,
  generationNotice,
  modes,
  onClose,
  onCopy,
  onCopyAndClose,
  onExternalTextAction,
  onGenerate,
  onRawInputChange,
  onSelectMode,
  output,
  lastResultAction,
  payloadPreview,
  previewExpanded,
  rawInput,
  rules,
  selectionMessage,
  selectedMode,
  validationMessage,
  onTogglePreview,
}: CompactInputPanelProps) {
  const hasOutput = output.trim().length > 0;
  const isGenerating = generateState === "generating";
  const targetTool = getModeTargetTool(selectedMode.id);
  const shortcutHint = hasOutput
    ? "⌘↵ 复制（既有）· ⌘⇧↵ 复制并关闭（既有）· ⌘C 复制"
    : "Enter 生成 · Shift+Enter 换行 · Esc 关闭";

  function handleInputKeyDown(event: KeyboardEvent<HTMLTextAreaElement>) {
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      onClose();
      return;
    }

    if (event.key === "Enter" && event.metaKey && event.shiftKey) {
      event.preventDefault();
      event.stopPropagation();
      onCopyAndClose();
      return;
    }

    if (event.key === "Enter" && event.metaKey) {
      event.preventDefault();
      event.stopPropagation();
      onCopy();
      return;
    }

    if (event.key === "Enter" && !event.shiftKey) {
      event.preventDefault();
      event.stopPropagation();
      onGenerate();
    }
  }

  function handlePanelKeyDown(event: KeyboardEvent<HTMLElement>) {
    if (event.key === "Escape") {
      event.preventDefault();
      onClose();
      return;
    }

    if (event.key === "Enter" && event.metaKey && event.shiftKey) {
      event.preventDefault();
      onCopyAndClose();
      return;
    }

    if (event.key === "Enter" && event.metaKey) {
      event.preventDefault();
      onCopy();
      return;
    }

    if (hasOutput && event.key.toLowerCase() === "c" && event.metaKey && !event.shiftKey) {
      event.preventDefault();
      event.stopPropagation();
      onCopy();
    }
  }

  function handleModeKeyDown(event: KeyboardEvent<HTMLSelectElement>) {
    if (event.key !== "ArrowDown" && event.key !== "ArrowUp") {
      return;
    }

    event.preventDefault();
    onSelectMode(getNextModeId(modes, selectedMode.id, event.key === "ArrowDown" ? 1 : -1));
  }

  return (
    <main className="compact-surface" onKeyDown={handlePanelKeyDown}>
      <section
        aria-busy={isGenerating}
        aria-label="Agent Input Method 输入面板"
        className={`compact-panel ${hasOutput ? "has-output" : ""}`}
      >
        <header className="compact-header">
          {hasOutput ? (
            <div className="compact-mode-title">
              <strong>{selectedMode.name}</strong>
              <span>适合复制到 {targetTool}</span>
            </div>
          ) : (
            <>
              <label className="compact-mode-select-wrap" htmlFor="compact-mode-select">
                <select
                  id="compact-mode-select"
                  onChange={(event) => onSelectMode(event.currentTarget.value as ModeId)}
                  onKeyDown={handleModeKeyDown}
                  value={selectedMode.id}
                >
                  {modes.map((mode) => (
                    <option key={mode.id} value={mode.id}>
                      {mode.name}
                    </option>
                  ))}
                </select>
              </label>
              <span className="compact-rule-summary">{getRuleSummary(rules)}</span>
              <kbd>Option + Space</kbd>
            </>
          )}
          <button
            aria-label="关闭输入面板"
            className="compact-close-button"
            onClick={onClose}
            title="关闭（Esc）"
            type="button"
          >
            <X aria-hidden="true" size={16} />
          </button>
        </header>

        {hasOutput ? (
          <pre className="compact-result">{output}</pre>
        ) : (
          <div className="compact-input-block">
            <label htmlFor="compact-raw-input">原始想法</label>
            <textarea
              autoFocus
              id="compact-raw-input"
              onChange={(event) => onRawInputChange(event.currentTarget.value)}
              onKeyDown={handleInputKeyDown}
              placeholder="例如：帮我做一个设置页面"
              value={rawInput}
            />
            <div className="payload-preview-summary compact-payload-summary">
              <button className="text-button" onClick={onTogglePreview} type="button">
                {previewExpanded ? "收起 payload" : "看看要发什么"}
              </button>
              <span>本次将发送 {payloadPreview.characterCount} 字符 / {payloadPreview.profileEntryCount} 条档案条目</span>
            </div>
            {previewExpanded ? <pre className="payload-preview compact-payload-preview">{payloadPreview.payload}</pre> : null}
          </div>
        )}

        <footer className="compact-footer">
          <div className="compact-status">
            {validationMessage ? <span className="is-error">{validationMessage}</span> : null}
            {!validationMessage && copyState === "failed" ? (
              <span className="is-error">复制失败，请手动选择结果复制。</span>
            ) : null}
            {!validationMessage && copyState !== "failed" && generationNotice ? (
              <span>{generationNotice}</span>
            ) : null}
            {!validationMessage && !generationNotice && selectionMessage ? <span>{selectionMessage}</span> : null}
            {hasOutput ? (
              <span className={externalTextAccess.actionsEnabled ? "" : "is-error"}>
                {externalTextAccess.message}
              </span>
            ) : null}
            {!validationMessage ? <span className="compact-shortcut-hint">{shortcutHint}</span> : null}
          </div>

          {hasOutput ? (
            <div className="compact-actions">
              <button
                aria-pressed={lastResultAction === "replace"}
                className={lastResultAction === "replace" ? "primary-button" : "secondary-button"}
                disabled={!externalTextAccess.actionsEnabled}
                onClick={() => onExternalTextAction("replace")}
                title="替换原选区；⌘↵ 已被既有复制动作占用"
                type="button"
              >
                替换
              </button>
              <button
                aria-pressed={lastResultAction === "insert"}
                className={lastResultAction === "insert" ? "primary-button" : "secondary-button"}
                disabled={!externalTextAccess.actionsEnabled}
                onClick={() => onExternalTextAction("insert")}
                title="保留原文并在选区后插入；⌘⇧↵ 已被既有复制并关闭占用"
                type="button"
              >
                插入
              </button>
              <button
                aria-pressed={lastResultAction === "copy"}
                className={lastResultAction === "copy" ? "primary-button" : "secondary-button"}
                onClick={onCopy}
                type="button"
              >
                <Clipboard size={16} />
                {copyState === "copied" ? "已复制" : "复制"}
              </button>
              <button
                className="secondary-button"
                disabled={isGenerating}
                onClick={onGenerate}
                type="button"
              >
                <RotateCcw size={16} />
                重新生成
              </button>
            </div>
          ) : (
            <div className="compact-actions">
              <button
                className="primary-button"
                disabled={isGenerating}
                onClick={onGenerate}
                type="button"
              >
                {isGenerating ? <Loader2 className="spin" size={16} /> : <WandSparkles size={16} />}
                {isGenerating ? "生成中" : "生成任务指令"}
              </button>
              <button
                className="secondary-button"
                disabled={!hasOutput}
                onClick={onCopyAndClose}
                type="button"
              >
                <X size={16} />
                复制并关闭
              </button>
            </div>
          )}
        </footer>
      </section>
    </main>
  );
}
