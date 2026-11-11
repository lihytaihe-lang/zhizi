import assert from "node:assert/strict";
import { test } from "node:test";
import { renderToStaticMarkup } from "react-dom/server";
import { SettingsPanel } from "../components/SettingsPanel";

test("settings keeps local skin controls beside the opt-in store entry", () => {
  const html = renderToStaticMarkup(<SettingsPanel
    providerSettings={{ enabled: false, baseUrl: "", model: "", apiKey: "" }}
    shortcutSettings={{ shortcut: "control+space" }} settingsMessage=""
    onProviderChange={() => {}} onShortcutChange={() => {}} onApplyShortcut={() => {}}
    mcpStatus={{ enabled: false, endpoint: null }}
  />);
  for (const label of ["我的皮肤", "皮肤商店", "安装皮肤目录", "安装 .json", "已安装候选皮肤"]) {
    assert.ok(html.includes(label), `missing ${label}`);
  }
});
