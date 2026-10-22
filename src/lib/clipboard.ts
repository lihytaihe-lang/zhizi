import { isTauriRuntime } from "./tauriRuntime";

export async function copyText(text: string): Promise<boolean> {
  if (!text.trim()) {
    return false;
  }

  if (isTauriRuntime()) {
    try {
      const { writeText } = await import("@tauri-apps/plugin-clipboard-manager");
      await writeText(text);
      return true;
    } catch {
      // Fall through to browser clipboard APIs for local web preview.
    }
  }

  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    const textarea = document.createElement("textarea");
    textarea.value = text;
    textarea.setAttribute("readonly", "true");
    textarea.style.position = "fixed";
    textarea.style.opacity = "0";
    document.body.appendChild(textarea);
    textarea.select();

    try {
      return document.execCommand("copy");
    } finally {
      document.body.removeChild(textarea);
    }
  }
}
