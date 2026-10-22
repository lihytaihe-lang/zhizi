import { isTauriRuntime } from "./tauriRuntime";

export type KnowledgeSourceInput = { id: string; name: string; sourceType: "local_path" | "notion"; authorizedScope: string; authorization: string; projectId: string; sensitiveEnabled: boolean; updatedAtMs: number };
export type KnowledgeSource = KnowledgeSourceInput & { status: "authorized" | "ready" | "error" | "revoked" | "deferred"; retrievalStatus: "pending" | "approved"; lastError: string | null; documentCount: number; chunkCount: number; termCount: number; createdAtMs: number };
export type KnowledgeTerm = { id: string; sourceId: string; sourceName: string; sourcePath: string; text: string; pinyin: string; kind: string; relation: string; definition: string; candidateLayer: "first" | "secondary"; confidenceMillis: number; reviewState: "pending" | "approved" | "rejected"; fixed: boolean; lastVerifiedMs: number };
export type IngestReport = { documentCount: number; chunkCount: number; extractedTermCount: number; skippedCount: number; unchangedCount: number };
export type HotwordGenerationStageReport = { stagedCount: number; invalidCount: number; missingPinyinCount: number };
export type KnowledgeSearchHit = { chunkId: string; content: string; rank: number; sourceId: string; sourceName: string; documentPath: string; ordinal: number };

async function invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> { const api = await import("@tauri-apps/api/core"); return api.invoke<T>(command, args); }
export async function listKnowledgeSources(): Promise<KnowledgeSource[]> { return isTauriRuntime() ? invoke("list_knowledge_sources") : []; }
export async function saveKnowledgeSource(input: KnowledgeSourceInput): Promise<KnowledgeSource> { return invoke("save_knowledge_source", { input }); }
export async function ingestKnowledgeSource(id: string): Promise<IngestReport> { return invoke("ingest_knowledge_source", { id }); }
export function applyLocalPinyinReadings(terms: KnowledgeTerm[], readings: Record<string, string>): KnowledgeTerm[] {
  return terms.map((term) => term.pinyin.trim() || !readings[term.text]
    ? term
    : { ...term, pinyin: readings[term.text] });
}
export async function listKnowledgeTerms(): Promise<KnowledgeTerm[]> {
  if (!isTauriRuntime()) return [];
  const terms = await invoke<KnowledgeTerm[]>("list_knowledge_terms");
  const texts = [...new Set(terms.filter((term) => !term.pinyin.trim()).map((term) => term.text))];
  if (texts.length === 0) return terms;
  const readings = await invoke<Record<string, string>>("local_pinyin_readings", { texts });
  return applyLocalPinyinReadings(terms, readings);
}
export async function stageGeneratedHotwordCandidates(output: string): Promise<HotwordGenerationStageReport> {
  return isTauriRuntime()
    ? invoke("stage_generated_hotword_candidates", { output })
    : { stagedCount: 0, invalidCount: 0, missingPinyinCount: 0 };
}
export async function reviewKnowledgeTerm(
  id: string,
  state: "pending" | "approved" | "rejected",
  text: string,
  pinyin: string,
  definition: string,
  candidateLayer: "first" | "secondary",
  fixed: boolean,
): Promise<KnowledgeTerm> {
  return invoke("review_knowledge_term", { id, state, text, pinyin, definition, candidateLayer, fixed });
}
export async function deleteKnowledgeSource(id: string, purge: boolean): Promise<boolean> { return invoke("delete_knowledge_source", { id, purge }); }
export async function setKnowledgeSourceRetrievalPolicy(id: string, retrievalStatus: "pending" | "approved", sensitiveEnabled: boolean): Promise<KnowledgeSource> {
  return invoke("set_knowledge_source_retrieval_policy", { id, retrievalStatus, sensitiveEnabled });
}
export async function searchKnowledge(query: string, projectId: string, limit = 5): Promise<KnowledgeSearchHit[]> {
  return invoke("search_knowledge", { query, projectId, limit });
}
