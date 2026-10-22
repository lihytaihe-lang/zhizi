import type { ResultAction } from "../domain/types";

const LAST_RESULT_ACTION_KEY = "agent-input-layer.last-result-action";
const DEFAULT_RESULT_ACTION: ResultAction = "copy";

export function loadLastResultAction(): ResultAction {
  try {
    const value = window.localStorage.getItem(LAST_RESULT_ACTION_KEY);
    return value === "replace" || value === "insert" || value === "copy"
      ? value
      : DEFAULT_RESULT_ACTION;
  } catch {
    return DEFAULT_RESULT_ACTION;
  }
}

export function saveLastResultAction(action: ResultAction): void {
  try {
    window.localStorage.setItem(LAST_RESULT_ACTION_KEY, action);
  } catch {
    // Highlight persistence is a convenience only; actions remain explicit.
  }
}
