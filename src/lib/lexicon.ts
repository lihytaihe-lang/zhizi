import { isTauriRuntime } from "./tauriRuntime";
import type { CandidateSkinId } from "../domain/skinCatalog";

export type LexiconSource = {
  sourceKind: string;
  source: string;
  license: string;
  confidenceMillis: number;
};

export type LexiconEntryInput = {
  id: string;
  text: string;
  pinyin: string;
  aliases: string[];
  professionalPacks: string[];
  projectDomains: string[];
  sources: LexiconSource[];
  baseWeight: number;
  fixed: boolean;
  disabled: boolean;
  demoted: boolean;
  updatedAtMs: number;
};

export type LexiconEntry = LexiconEntryInput & { createdAtMs: number };
export type LexiconImportPreview = { entryCount: number; texts: string[] };
export type LexiconImportResult = { batchId: string; importedCount: number };
export type SnapshotMeta = {
  version: number;
  schemaVersion: number;
  checksum: string;
  fileName: string;
  entryCount: number;
};

export type QualityRecordingStatus = { enabled: boolean; expiresAtMs: number; continuous: boolean };
export type QualityRecordingConsent = { granted: boolean; grantedAtMs: number; noticeVersion: string };
export type QualityConsentRevocationResult = {
  consent: QualityRecordingConsent;
  deletedCount: number;
};
export type QualityRecordingMode = "off" | "twoHour" | "continuous";
export type ProjectProfileInput = {
  id: string; name: string; projectDomain: string; professionalPacks: string[];
  agentContext: string; updatedAtMs: number;
};
export type ProjectProfile = ProjectProfileInput & {
  active: boolean; createdAtMs: number;
};
export type ProjectRuntimeConfiguration = {
  activeProfileId: string | null;
  activeProjectDomain: string | null;
  activeProfessionalPacks: string[];
  agentContext: string;
  secondaryLayerEnabled: boolean;
  predictionEnabled: boolean;
  contextModelRankingEnabled: boolean;
  candidateTheme: CandidateSkinId;
};
export type CandidateSkinEntry = {
  id: string;
  displayName: { zh: string; en: string };
  author: string;
  description: string;
  origin: "builtIn" | "external";
  status: "ready" | "conflict";
  reason: string | null;
  canUninstall: boolean;
};
export type CandidateSkinIssue = { package: string; reason: string };
export type CandidateSkinCatalogSnapshot = {
  skins: CandidateSkinEntry[];
  issues: CandidateSkinIssue[];
};
export type HotwordQualityReport = {
  status: "passed" | "insufficient_data";
  ownerReplayStatus: "passed" | "failed" | "insufficient_data";
  ownerSampleCount: number;
  fixedHotwordCount: number;
  firstLayerOccupationCount: number;
  expiredCount: number;
  queryLatencyMilliseconds: number;
  networkFallback: string;
};
export type QualityRecordingEvent = {
  id: string;
  recordedAtMs: number;
  pinyin: string;
  candidates: string[];
  candidateComments: string[];
  selectedIndex: number;
  selectedText: string;
  selectedCandidateComment: string | null;
  appHash: string;
  precedingText: string;
  fuzzyMask: number;
  typoCorrectionEnabled: boolean;
  activeProject: string | null;
  activePacks: string[];
  secondaryEnabled: boolean;
  maxPage: number;
  outcome: "committed_candidate" | "committed_raw" | "abandoned";
  durationMs: number;
  backspaces: number;
  redoAfterCommit: boolean;
  candidateCount: number;
  contextModel?: {
    enabled: boolean;
    outcome: "disabled" | "unavailable" | "no_group" | "timeout" | "suspended" | "error" | "unchanged" | "reordered";
    baseOrder?: number[];
    groupSize?: number;
  };
};

export type PersonalProfileIntrospection = {
  sourceKind: "quality-events-jsonl-v1";
  readOnly: true;
  totalEvents: number;
  frictionEvents: number;
  rawCommitEvents: number;
  homophoneSplitCount: number;
  deepPageEvents: number;
  abandonedEvents: number;
  redoEvents: number;
  latestRecordedAtMs: number | null;
  topNTargets: Array<{
    pinyin: string;
    selectedText: string;
    frictionCount: number;
    totalCount: number;
    deepestRank: number;
    latestRecordedAtMs: number;
    confident: boolean;
    runnerUpFrictionCount: number;
  }>;
  wholeStringTargets: Array<{
    pinyin: string;
    rawCommitCount: number;
    latestRecordedAtMs: number;
    chineseCandidatePreview: string[];
  }>;
  homophoneSplits: Array<{
    pinyin: string;
    frictionCount: number;
    latestRecordedAtMs: number;
    choices: Array<{ text: string; frictionCount: number }>;
  }>;
};

export type PersonalProfileAutoUpdateItem = {
  id: string;
  entryId: string;
  pinyin: string;
  text: string;
  frictionCount: number;
  runnerUpFrictionCount: number;
  latestFrictionAtMs: number;
  appliedAtMs: number;
  lastRefreshedAtMs: number;
  status: "active" | "decayed" | "revoked" | "blacklisted";
  statusChangedAtMs: number;
  predictionEligible: boolean;
  decayAfterMs: number;
  evidenceKind: "friction" | "owner_declared_disambiguation";
  ownerDeclaredCandidates: string[];
};
export type PersonalProfileAutoUpdateReport = {
  appliedCount: number;
  refreshedCount: number;
  decayedCount: number;
  blacklistedCount: number;
  skippedRevokedCount: number;
  items: PersonalProfileAutoUpdateItem[];
};

export type FrozenGoldenMeta = {
  batchId: string;
  checksum: string;
  sampleCount: number;
  sourceKind: string;
  frozenAtMs: number;
};
export type ConfidenceInterval = { lower: number; upper: number };
export type ReplayGateReport = {
  schemaVersion: number;
  status: "passed" | "failed" | "insufficient_data";
  sourceKind: string;
  goldenChecksum: string;
  candidateChecksum: string;
  reportChecksum: string;
  sampleCount: number;
  repetitions: number;
  baselineTop1Hits: number;
  baselineTop5Hits: number;
  candidateTop1Hits: number;
  candidateTop5Hits: number;
  baselineTop1Cases: number;
  firstChoiceRegressions: number;
  jitterCases: number;
  latencySamples: number;
  baselineTop1Rate: number;
  baselineTop5Rate: number;
  candidateTop1Rate: number;
  candidateTop5Rate: number;
  firstChoiceRegressionRate: number;
  candidateTop1Confidence: ConfidenceInterval;
  candidateTop5Confidence: ConfidenceInterval;
  firstChoiceRegressionConfidence: ConfidenceInterval;
  latencyP95Milliseconds: number;
  latencyP99Milliseconds: number;
  confidenceSufficient: boolean;
};
export type OverrideSnapshotPublication = {
  snapshot: SnapshotMeta;
  replayReport: ReplayGateReport;
  overrideAuditEntryHash: string;
};

async function invokeLocal<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  if (!isTauriRuntime()) {
    throw new Error("此功能需要在 Agent Input Method 桌面管理台中使用。");
  }
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<T>(command, args);
}

export const listLexiconEntries = () => invokeLocal<LexiconEntry[]>("list_lexicon_entries");
export const createLexiconEntry = (input: LexiconEntryInput) =>
  invokeLocal<LexiconEntry>("create_lexicon_entry", { input });
export const updateLexiconEntry = (input: LexiconEntryInput) =>
  invokeLocal<LexiconEntry>("update_lexicon_entry", { input });
export const deleteLexiconEntries = (ids: string[]) =>
  invokeLocal<number>("delete_lexicon_entries", { ids });
export const previewLexiconImport = (document: string) =>
  invokeLocal<LexiconImportPreview>("preview_lexicon_import", { document });
export const importLexicon = (document: string) =>
  invokeLocal<LexiconImportResult>("import_lexicon", { document });
export const undoLexiconImport = (batchId: string) =>
  invokeLocal<number>("undo_lexicon_import", { batchId });
export const exportLexicon = () => invokeLocal<string>("export_lexicon");
export const clearLearnedLexiconEntries = () =>
  invokeLocal<number>("clear_learned_lexicon_entries");
export const compileLexiconSnapshot = (version: number) =>
  invokeLocal<SnapshotMeta>("compile_lexicon_snapshot", { version });

export const qualityRecordingStatus = () =>
  invokeLocal<QualityRecordingStatus>("quality_recording_status");
export const setQualityRecording = (mode: QualityRecordingMode) =>
  invokeLocal<QualityRecordingStatus>("set_quality_recording", { mode });
export const qualityRecordingConsentStatus = () =>
  invokeLocal<QualityRecordingConsent>("quality_recording_consent_status");
export const grantQualityRecordingConsent = () =>
  invokeLocal<QualityRecordingConsent>("grant_quality_recording_consent");
export const confirmTestRecordingConsent = () => invokeLocal<boolean>("confirm_test_recording_consent");
export const testUploadEnabled = () => invokeLocal<boolean>("test_upload_enabled");
export const revokeQualityRecordingConsent = (deleteRecords: boolean) =>
  invokeLocal<QualityConsentRevocationResult>("revoke_quality_recording_consent", { deleteRecords });
export const listQualityRecords = () =>
  invokeLocal<QualityRecordingEvent[]>("list_quality_records");
export const personalProfileIntrospection = () =>
  invokeLocal<PersonalProfileIntrospection>("personal_profile_introspection");
export const applyPersonalProfileAutoUpdate = () =>
  invokeLocal<PersonalProfileAutoUpdateReport>("apply_personal_profile_auto_update");
export const declareOwnerDisambiguation = (
  pinyin: string, preferredText: string, candidates: string[],
) => invokeLocal<PersonalProfileAutoUpdateReport>("declare_owner_disambiguation", {
  pinyin, preferredText, candidates,
});
export const listPersonalProfileAutoUpdates = () =>
  invokeLocal<PersonalProfileAutoUpdateItem[]>("list_personal_profile_auto_updates");
export const revokePersonalProfileAutoUpdate = (id: string) =>
  invokeLocal<PersonalProfileAutoUpdateItem>("revoke_personal_profile_auto_update", { id });
export const exportQualityRecords = () => invokeLocal<string>("export_quality_records");
export const deleteQualityRecords = () => invokeLocal<number>("delete_quality_records");
export const deleteQualityRecord = (id: string) =>
  invokeLocal<number>("delete_quality_record", { id });
export const deleteQualityRecordsByApp = (appHash: string) =>
  invokeLocal<number>("delete_quality_records_by_app", { appHash });
export const deleteQualityRecordsByTimeRange = (startMs: number, endMs: number) =>
  invokeLocal<number>("delete_quality_records_by_time_range", { startMs, endMs });
export const freezeReplayGoldenSet = () =>
  invokeLocal<FrozenGoldenMeta>("freeze_replay_golden_set");
export const runPersonalReplayGate = () =>
  invokeLocal<ReplayGateReport>("run_personal_replay_gate");
export const compileLexiconSnapshotWithOverride = (version: number, reason: string) =>
  invokeLocal<OverrideSnapshotPublication>("compile_lexicon_snapshot_with_override", {
    version, reason,
  });

export const saveProjectProfile = (input: ProjectProfileInput) =>
  invokeLocal<ProjectProfile>("save_project_profile", { input });
export const listProjectProfiles = () =>
  invokeLocal<ProjectProfile[]>("list_project_profiles");
export const deleteProjectProfile = (id: string) =>
  invokeLocal<boolean>("delete_project_profile", { id });
export const activateProjectProfile = (id: string | null) =>
  invokeLocal<ProjectRuntimeConfiguration>("activate_project_profile", { id });
export const projectRuntimeConfiguration = () =>
  invokeLocal<ProjectRuntimeConfiguration>("project_runtime_configuration");
export const setSecondaryLayerEnabled = (enabled: boolean) =>
  invokeLocal<ProjectRuntimeConfiguration>("set_secondary_layer_enabled", { enabled });
export const setPredictionEnabled = (enabled: boolean) =>
  invokeLocal<ProjectRuntimeConfiguration>("set_prediction_enabled", { enabled });
export const setCandidateTheme = (candidateTheme: CandidateSkinId) =>
  invokeLocal<ProjectRuntimeConfiguration>("set_candidate_theme", { candidateTheme });
export const listCandidateSkins = () =>
  invokeLocal<CandidateSkinCatalogSnapshot>("list_candidate_skins");
export const installCandidateSkin = (sourcePath: string) =>
  invokeLocal<CandidateSkinCatalogSnapshot>("install_candidate_skin", { sourcePath });
export const uninstallCandidateSkin = (id: string) =>
  invokeLocal<CandidateSkinCatalogSnapshot>("uninstall_candidate_skin", { id });
export const compileSecondaryPredictionSnapshot = (version: number) =>
  invokeLocal<SnapshotMeta>("compile_secondary_prediction_snapshot", { version });
export const runHotwordQualityGate = () =>
  invokeLocal<HotwordQualityReport>("run_hotword_quality_gate");
export const rollbackLexiconSnapshot = (version: number) =>
  invokeLocal<SnapshotMeta>("rollback_lexicon_snapshot", { version });

export function splitLexiconList(value: string): string[] {
  return [...new Set(value.split(/[,，\n]/).map((item) => item.trim()).filter(Boolean))].sort();
}

export function downloadJSON(fileName: string, contents: string): void {
  const url = URL.createObjectURL(new Blob([contents], { type: "application/json;charset=utf-8" }));
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = fileName;
  anchor.click();
  URL.revokeObjectURL(url);
}

let lastSnapshotVersion = 0;
export function nextSnapshotVersion(): number {
  lastSnapshotVersion = Math.max(Date.now(), lastSnapshotVersion + 1);
  return lastSnapshotVersion;
}

export const contextModelAvailability = () =>
  invokeLocal<{ available: boolean; reason: string | null }>("context_model_availability");
export const setContextModelRankingEnabled = (enabled: boolean) =>
  invokeLocal<ProjectRuntimeConfiguration>("set_context_model_ranking_enabled", { enabled });
