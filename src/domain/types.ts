export type ModeCategory = "discussion" | "development" | "design" | "planning" | "writing" | "lexicon";

export type ModeId =
  | "chatgpt-discussion"
  | "codex-execution"
  | "product-planning"
  | "bug-fix"
  | "code-review"
  | "ui-design-prompt"
  | "rewrite"
  | "summarize"
  | "translate"
  | "hotword-candidates";

export type Mode = {
  id: ModeId;
  name: string;
  description: string;
  category: ModeCategory;
  actionLabel: string;
  appliesDevGuardrails: boolean;
};

export type UserRule = {
  id: string;
  text: string;
  enabled: boolean;
};

export type HistoryItem = {
  id: string;
  rawInput: string;
  modeId: ModeId;
  targetTool?: string;
  output: string;
  favorite: boolean;
  createdAt: string;
};

export type ProviderSettings = {
  enabled: boolean;
  baseUrl: string;
  model: string;
  apiKey: string;
  providerKind?: ProviderKind;
  anthropicBaseUrl?: string;
  anthropicModel?: string;
  anthropicApiKey?: string;
  ollamaBaseUrl?: string;
  ollamaModel?: string;
};

export type ProviderKind = "openai-compatible" | "anthropic" | "ollama";

export type ShortcutSettings = {
  shortcut: string;
  mcpEnabled?: boolean;
};

export type AppClass = "Code" | "Prose" | "Browser" | "Unknown";

export type PayloadPreview = {
  payload: string;
  characterCount: number;
  profileEntryCount: number;
  projectTermCount: number;
  preferenceCount: number;
  knowledgeChunkCount: number;
};

export type SelectionCapture = {
  status: "captured" | "empty" | "permission-denied" | "secure" | "unsupported";
  text: string;
  appClass: AppClass;
  message: string;
};

export type ExternalTextAction = "replace" | "insert";

export type ResultAction = ExternalTextAction | "copy";

export type ExternalTextAccess = {
  authorized: boolean;
  targetAvailable: boolean;
  actionsEnabled: boolean;
  status: "ready" | "permission-denied" | "target-missing" | "secure-refused" | "unsupported";
  message: string;
};

export type ExternalTextActionResult = {
  status: string;
  message: string;
  fallbackToClipboard: boolean;
};

export type ComposeInput = {
  rawInput: string;
  mode: Mode;
  userRules: UserRule[];
  compact?: boolean;
};

export type CopyState = "idle" | "copied" | "failed";

export type GenerateState = "idle" | "generating" | "local" | "model" | "fallback";
