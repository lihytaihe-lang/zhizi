import { isTauriRuntime } from "./tauriRuntime";

export async function getCurrentWindowLabel(): Promise<string> {
  if (!isTauriRuntime()) {
    return new URLSearchParams(window.location.search).get("surface") === "compact"
      ? "compact"
      : "main";
  }

  try {
    const { getCurrentWindow } = await import("@tauri-apps/api/window");
    return getCurrentWindow().label;
  } catch {
    return "main";
  }
}

export async function hideCurrentWindow(): Promise<void> {
  if (!isTauriRuntime()) {
    return;
  }

  try {
    const { getCurrentWindow } = await import("@tauri-apps/api/window");
    await getCurrentWindow().hide();
  } catch {
    // Browser preview and restricted environments can ignore native window control.
  }
}

export async function resizeCurrentWindow(width: number, height: number): Promise<void> {
  if (!isTauriRuntime()) {
    return;
  }

  try {
    const { getCurrentWindow, LogicalSize } = await import("@tauri-apps/api/window");
    const currentWindow = getCurrentWindow();

    await currentWindow.setSize(new LogicalSize(width, height));
    await currentWindow.center();
  } catch {
    // Window sizing is only available in the native shell.
  }
}

export async function listenForPanelFocus(onFocus: () => void): Promise<() => void> {
  if (!isTauriRuntime()) {
    return () => {};
  }

  try {
    const { listen } = await import("@tauri-apps/api/event");
    return await listen("agent-input-layer://focus-input", onFocus);
  } catch {
    return () => {};
  }
}
