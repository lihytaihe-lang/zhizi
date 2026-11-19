import { useEffect, useState } from "react";
import { KeyRound, Save, Zap } from "lucide-react";
import type { ProviderKind, ProviderSettings, ShortcutSettings } from "../domain/types";
import type { McpServerStatus } from "../lib/layer2";
import {
  contextModelAvailability,
  setContextModelRankingEnabled,
  projectRuntimeConfiguration,
} from "../lib/lexicon";

type SettingsPanelProps = {
  providerSettings: ProviderSettings;
  shortcutSettings: ShortcutSettings;
  settingsMessage: string;
  onProviderChange: (settings: ProviderSettings) => void;
  onShortcutChange: (settings: ShortcutSettings) => void;
  onApplyShortcut: () => void;
  mcpStatus: McpServerStatus;
};

const shortcutOptions = [
  { label: "Option + Space", value: "alt+space" },
  { label: "Command + Shift + Space", value: "super+shift+space" },
  { label: "Control + Space", value: "control+space" },
];

export function SettingsPanel({
  providerSettings,
  shortcutSettings,
  settingsMessage,
  onProviderChange,
  onShortcutChange,
  onApplyShortcut,
  mcpStatus,
}: SettingsPanelProps) {
  const [contextRankingEnabled, setContextRankingEnabled] = useState(false);
  const [modelAvailability, setModelAvailability] = useState({ available: false, reason: "正在检查本机模型文件。" as string | null });
  const [modelMessage, setModelMessage] = useState("");
  const [modelBusy, setModelBusy] = useState(false);
  useEffect(() => {
    void Promise.all([projectRuntimeConfiguration(), contextModelAvailability()])
      .then(([runtime, availability]) => {
        setContextRankingEnabled(runtime.contextModelRankingEnabled ?? false);
        setModelAvailability(availability);
      }).catch(() => setModelAvailability({ available: false, reason: "本机模型文件不可用；请使用包含模型的输入法构建。" }));
  }, []);
  async function changeContextRanking(enabled: boolean) {
    setModelBusy(true);
    try {
      const runtime = await setContextModelRankingEnabled(enabled);
      setContextRankingEnabled(runtime.contextModelRankingEnabled);
      setModelMessage("已保存，下次开始输入时生效。");
    } catch (error) { setModelMessage(String(error)); }
    finally { setModelBusy(false); }
  }
  const providerKind = providerSettings.providerKind ?? "openai-compatible";
  return (
    <section className="panel settings-panel" aria-labelledby="settings-title">
      <div className="panel-heading">
        <div>
          <h2 id="settings-title">设置</h2>
          <p>本地模板默认可用，模型增强为可选项</p>
        </div>
      </div>

      <div className="settings-section">
        <h3>上下文模型排序（实验）</h3>
        <p>实验功能：用本机小模型按上下文调整同音字顺序，不联网</p>
        <label className="toggle-row">
          <input type="checkbox" checked={contextRankingEnabled && modelAvailability.available}
            disabled={!modelAvailability.available || modelBusy}
            onChange={(event) => void changeContextRanking(event.currentTarget.checked)} />
          <span>启用上下文模型排序（默认关闭）</span>
        </label>
        {modelAvailability.reason ? <p role="status">{modelAvailability.reason}</p> : null}
        {modelMessage ? <p role="status">{modelMessage}</p> : null}
      </div>

      <div className="settings-section">
        <div className="settings-section-heading">
          <Zap size={17} />
          <div>
            <h3>悬浮小面板</h3>
            <p>生成后只预览；替换或插入必须由你点击，并通过辅助功能 AXSelectedText 写回</p>
          </div>
        </div>
        <p className="settings-message">不合成 Cmd+V；密码框与安全输入目标一律拒绝写入，复制始终可用。</p>
      </div>

      <div className="settings-section">
        <div className="settings-section-heading">
          <Zap size={17} />
          <div>
            <h3>本地只读 MCP</h3>
            <p>仅监听 127.0.0.1，暴露词汇、项目术语、动作列表与本地模板渲染</p>
          </div>
        </div>
        <label className="toggle-row">
          <input
            checked={Boolean(shortcutSettings.mcpEnabled)}
            onChange={(event) => onShortcutChange({ ...shortcutSettings, mcpEnabled: event.currentTarget.checked })}
            type="checkbox"
          />
          <span>启用本地 MCP server（默认关闭）</span>
        </label>
        {mcpStatus.enabled ? (
          <>
            <p className="settings-message">已开启：{mcpStatus.endpoint}</p>
            <p className="settings-message">地址里含访问令牌，只给你信任的本机程序。</p>
          </>
        ) : (
          <p className="settings-message">当前关闭；无监听端口。</p>
        )}
      </div>

      <div className="settings-section">
        <div className="settings-section-heading">
          <Zap size={17} />
          <div>
            <h3>快捷键</h3>
            <p>用于唤起桌面面板</p>
          </div>
        </div>

        <div className="field-grid">
          <label className="field-label" htmlFor="shortcut-select">
            全局快捷键
          </label>
          <select
            id="shortcut-select"
            onChange={(event) => onShortcutChange({ shortcut: event.currentTarget.value })}
            value={shortcutSettings.shortcut}
          >
            {shortcutOptions.map((option) => (
              <option key={option.value} value={option.value}>
                {option.label}
              </option>
            ))}
          </select>
        </div>

        <button className="secondary-button settings-action" onClick={onApplyShortcut} type="button">
          <Save size={16} />
          应用快捷键
        </button>
      </div>


      <div className="settings-section">
        <div className="settings-section-heading">
          <KeyRound size={17} />
          <div>
            <h3>模型增强</h3>
            <p>OpenAI 兼容 / Anthropic Messages / 本地 Ollama；全部 BYO key，不代付、不计量</p>
          </div>
        </div>

        <label className="toggle-row">
          <input
            checked={providerSettings.enabled}
            onChange={(event) =>
              onProviderChange({
                ...providerSettings,
                enabled: event.currentTarget.checked,
              })
            }
            type="checkbox"
          />
          <span>启用模型增强</span>
        </label>

        <div className="field-grid">
          <label className="field-label" htmlFor="provider-kind">
            Provider
          </label>
          <select
            id="provider-kind"
            onChange={(event) => onProviderChange({ ...providerSettings, providerKind: event.currentTarget.value as ProviderKind })}
            value={providerKind}
          >
            <option value="openai-compatible">OpenAI 兼容</option>
            <option value="anthropic">Anthropic Messages</option>
            <option value="ollama">本地 Ollama</option>
          </select>

          {providerKind === "openai-compatible" ? <>
          <label className="field-label" htmlFor="provider-base-url">
            Base URL
          </label>
          <input
            id="provider-base-url"
            onChange={(event) =>
              onProviderChange({
                ...providerSettings,
                baseUrl: event.currentTarget.value,
              })
            }
            placeholder="例如：https://api.example.com/v1"
            value={providerSettings.baseUrl}
          />

          <label className="field-label" htmlFor="provider-model">
            Model
          </label>
          <input
            id="provider-model"
            onChange={(event) =>
              onProviderChange({
                ...providerSettings,
                model: event.currentTarget.value,
              })
            }
            placeholder="填写兼容接口的模型名"
            value={providerSettings.model}
          />

          <label className="field-label" htmlFor="provider-api-key">
            API Key
          </label>
          <input
            id="provider-api-key"
            onChange={(event) =>
              onProviderChange({
                ...providerSettings,
                apiKey: event.currentTarget.value,
              })
            }
            placeholder="不填写时只使用本地模板"
            type="password"
            value={providerSettings.apiKey}
          />
          </> : null}

          {providerKind === "anthropic" ? <>
          <label className="field-label" htmlFor="anthropic-base-url">Base URL</label>
          <input id="anthropic-base-url" value={providerSettings.anthropicBaseUrl ?? "https://api.anthropic.com"} onChange={(event) => onProviderChange({ ...providerSettings, anthropicBaseUrl: event.currentTarget.value })} />
          <label className="field-label" htmlFor="anthropic-model">Model</label>
          <input id="anthropic-model" value={providerSettings.anthropicModel ?? ""} onChange={(event) => onProviderChange({ ...providerSettings, anthropicModel: event.currentTarget.value })} placeholder="例如：claude-sonnet-4-5" />
          <label className="field-label" htmlFor="anthropic-api-key">API Key</label>
          <input id="anthropic-api-key" type="password" value={providerSettings.anthropicApiKey ?? ""} onChange={(event) => onProviderChange({ ...providerSettings, anthropicApiKey: event.currentTarget.value })} placeholder="BYO Anthropic key" />
          </> : null}

          {providerKind === "ollama" ? <>
          <label className="field-label" htmlFor="ollama-base-url">Loopback URL</label>
          <input id="ollama-base-url" value={providerSettings.ollamaBaseUrl ?? "http://127.0.0.1:11434"} onChange={(event) => onProviderChange({ ...providerSettings, ollamaBaseUrl: event.currentTarget.value })} />
          <label className="field-label" htmlFor="ollama-model">Model</label>
          <input id="ollama-model" value={providerSettings.ollamaModel ?? ""} onChange={(event) => onProviderChange({ ...providerSettings, ollamaModel: event.currentTarget.value })} placeholder="例如：qwen3:8b" />
          </> : null}
        </div>
      </div>

      {settingsMessage ? <p className="settings-message">{settingsMessage}</p> : null}
    </section>
  );
}
