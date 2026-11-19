import assert from "node:assert/strict";
import { test } from "node:test";
import { renderToStaticMarkup } from "react-dom/server";
import { SettingsPanel } from "../components/SettingsPanel";

test("settings keeps settings controls without a second skin gallery", () => {
  const html = renderToStaticMarkup(<SettingsPanel
    providerSettings={{ enabled: false, baseUrl: "", model: "", apiKey: "" }}
    shortcutSettings={{ shortcut: "control+space" }} settingsMessage=""
    onProviderChange={() => {}} onShortcutChange={() => {}} onApplyShortcut={() => {}}
    mcpStatus={{ enabled: false, endpoint: null }}
  />);
  assert.match(html, /上下文模型排序/);
  assert.doesNotMatch(html, /我的皮肤|已安装候选皮肤|安装皮肤目录/);
});
