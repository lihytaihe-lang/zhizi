import { isTauriRuntime } from "./tauriRuntime";

export async function applyPanelShortcut(shortcut: string): Promise<string> {
  if (!isTauriRuntime()) {
    return "已保存。浏览器预览中不会注册系统快捷键。";
  }

  const { invoke } = await import("@tauri-apps/api/core");
  await invoke("update_panel_shortcut", { shortcut });
  return "快捷键已更新。";
}
