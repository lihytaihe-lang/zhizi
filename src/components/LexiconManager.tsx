import { useEffect, useMemo, useState } from "react";
import {
  ArchiveRestore, Check, Download, FileUp, Pin, Plus, RefreshCw, Save,
  ShieldOff, Trash2, Undo2,
} from "lucide-react";
import {
  clearLearnedLexiconEntries, compileLexiconSnapshot, createLexiconEntry,
  deleteLexiconEntries, downloadJSON, exportLexicon, importLexicon,
  listLexiconEntries, previewLexiconImport, splitLexiconList, undoLexiconImport,
  updateLexiconEntry, nextSnapshotVersion,
  type LexiconEntry, type LexiconEntryInput, type LexiconImportPreview,
} from "../lib/lexicon";
import { ProjectProfileManager } from "./ProjectProfileManager";

type Draft = {
  id: string; text: string; pinyin: string; aliases: string; packs: string;
  domains: string; source: string; license: string; fixed: boolean;
  demoted: boolean; disabled: boolean;
};

const emptyDraft = (): Draft => ({
  id: "", text: "", pinyin: "", aliases: "", packs: "", domains: "",
  source: "owner-entry", license: "private-user-data", fixed: false,
  demoted: false, disabled: false,
});

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function toInput(draft: Draft): LexiconEntryInput {
  const now = Date.now();
  return {
    id: draft.id.trim() || `lexicon-${now}`,
    text: draft.text.trim(),
    pinyin: draft.pinyin.trim(),
    aliases: splitLexiconList(draft.aliases),
    professionalPacks: splitLexiconList(draft.packs),
    projectDomains: splitLexiconList(draft.domains),
    sources: [{
      sourceKind: "manual", source: draft.source.trim(), license: draft.license.trim(),
      confidenceMillis: 1000,
    }],
    baseWeight: draft.demoted ? -100 : 100,
    fixed: draft.fixed,
    disabled: draft.disabled,
    demoted: draft.demoted,
    updatedAtMs: now,
  };
}

function fromEntry(entry: LexiconEntry): Draft {
  return {
    id: entry.id, text: entry.text, pinyin: entry.pinyin,
    aliases: entry.aliases.join(", "), packs: entry.professionalPacks.join(", "),
    domains: entry.projectDomains.join(", "), source: entry.sources[0]?.source ?? "owner-entry",
    license: entry.sources[0]?.license ?? "private-user-data", fixed: entry.fixed,
    demoted: entry.demoted, disabled: entry.disabled,
  };
}

export function LexiconManager() {
  const [entries, setEntries] = useState<LexiconEntry[]>([]);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [draft, setDraft] = useState<Draft>(emptyDraft);
  const [editing, setEditing] = useState(false);
  const [message, setMessage] = useState("");
  const [importDocument, setImportDocument] = useState("");
  const [preview, setPreview] = useState<LexiconImportPreview | null>(null);
  const [undoBatch, setUndoBatch] = useState("");
  const [undoAction, setUndoAction] = useState<LexiconEntry[] | null>(null);
  const selectedCount = selected.size;
  const allSelected = entries.length > 0 && selectedCount === entries.length;
  const sourceCount = useMemo(() => new Set(entries.flatMap((entry) => entry.sources.map((source) => source.source))).size, [entries]);

  async function refresh() {
    try { setEntries(await listLexiconEntries()); }
    catch (error) { setMessage(errorMessage(error)); }
  }

  useEffect(() => { void refresh(); }, []);

  async function publishAfterMutation(success: string) {
    try {
      const snapshot = await compileLexiconSnapshot(nextSnapshotVersion());
      setMessage(`${success} 快照 v${snapshot.version} 已通过 Replay Gate 并发布。`);
    } catch (error) {
      setMessage(`${success} SQLite 已保存；快照未发布：${errorMessage(error)}`);
    }
  }

  async function saveDraft() {
    try {
      const input = toInput(draft);
      if (!input.text || !input.pinyin || !input.sources[0].source || !input.sources[0].license) {
        setMessage("词条、拼音、来源和许可证均为必填。"); return;
      }
      if (editing) await updateLexiconEntry(input); else await createLexiconEntry(input);
      const success = editing ? "词条已更新。" : "词条已新增。";
      setDraft(emptyDraft()); setEditing(false);
      await publishAfterMutation(success);
      await refresh();
    } catch (error) { setMessage(`保存失败：${errorMessage(error)}`); }
  }

  async function patchEntry(entry: LexiconEntry, patch: Partial<LexiconEntryInput>) {
    try {
      await updateLexiconEntry({ ...entry, ...patch, updatedAtMs: Date.now() });
      setUndoAction([entry]);
      await publishAfterMutation("管理动作已保存，可撤销。");
      await refresh();
    } catch (error) { setMessage(`更新失败：${errorMessage(error)}`); }
  }

  async function removeEntry(entry: LexiconEntry) {
    try {
      await deleteLexiconEntries([entry.id]);
      setUndoAction([entry]);
      await publishAfterMutation("词条已删除，可撤销。");
      await refresh();
    } catch (error) { setMessage(`删除失败：${errorMessage(error)}`); }
  }

  async function undoManagementAction() {
    if (!undoAction) return;
    try {
      const existingIds = new Set(entries.map((entry) => entry.id));
      for (const entry of undoAction) {
        const input: LexiconEntryInput = { ...entry, updatedAtMs: Date.now() };
        if (existingIds.has(input.id)) await updateLexiconEntry(input);
        else await createLexiconEntry(input);
      }
      setUndoAction(null); await publishAfterMutation("已撤销管理动作。"); await refresh();
    } catch (error) { setMessage(`撤销失败：${errorMessage(error)}`); }
  }

  async function removeSelected() {
    try {
      const removed = entries.filter((entry) => selected.has(entry.id));
      const count = await deleteLexiconEntries(removed.map((entry) => entry.id));
      setUndoAction(removed);
      setSelected(new Set()); await publishAfterMutation(`已删除 ${count} 条，可撤销。`); await refresh();
    } catch (error) { setMessage(`批量删除失败：${errorMessage(error)}`); }
  }

  async function readImport(file: File | undefined) {
    if (!file) return;
    try {
      const document = await file.text();
      const result = await previewLexiconImport(document);
      setImportDocument(document); setPreview(result);
      setMessage(`预览完成：${result.entryCount} 条，确认后才会写入。`);
    } catch (error) {
      setImportDocument(""); setPreview(null); setMessage(`导入预览失败：${errorMessage(error)}`);
    }
  }

  async function confirmImport() {
    try {
      const result = await importLexicon(importDocument);
      setUndoBatch(result.batchId); setImportDocument(""); setPreview(null);
      setMessage(`已导入 ${result.importedCount} 条，可立即撤销。`); await refresh();
    } catch (error) { setMessage(`导入失败：${errorMessage(error)}`); }
  }

  async function undoImport() {
    try {
      const count = await undoLexiconImport(undoBatch);
      setUndoBatch(""); setMessage(`已撤销 ${count} 条导入。`); await refresh();
    } catch (error) { setMessage(`撤销失败：${errorMessage(error)}`); }
  }

  async function exportEntries() {
    try { downloadJSON(`agent-lexicon-${Date.now()}.json`, await exportLexicon()); setMessage("已导出可恢复词库。"); }
    catch (error) { setMessage(`导出失败：${errorMessage(error)}`); }
  }

  async function publishSnapshot() {
    try {
      const result = await compileLexiconSnapshot(nextSnapshotVersion());
      setMessage(`快照 v${result.version} 已发布，${result.entryCount} 条。重新激活输入法后生效。`);
    } catch (error) { setMessage(`快照发布失败：${errorMessage(error)}`); }
  }

  return (
    <section className="panel lexicon-panel" aria-labelledby="lexicon-title">
      <ProjectProfileManager />
      <div className="panel-heading lexicon-heading">
        <div><h2 id="lexicon-title">个人与专业词库</h2><p>{entries.length} 条 · {sourceCount} 个来源 · SQLite 权威库</p></div>
        <div className="lexicon-toolbar">
          <label className="secondary-button file-button"><FileUp size={16} />导入<input accept="application/json,.json" onChange={(event) => void readImport(event.currentTarget.files?.[0])} type="file" /></label>
          <button className="secondary-button" onClick={() => void exportEntries()} type="button"><Download size={16} />导出</button>
          <button className="primary-button" onClick={() => void publishSnapshot()} type="button"><RefreshCw size={16} />发布快照</button>
        </div>
      </div>

      {preview ? <div className="import-preview"><strong>待导入 {preview.entryCount} 条</strong><span>{preview.texts.join("、")}</span><div><button className="primary-button" onClick={() => void confirmImport()} type="button"><Check size={16} />确认导入</button><button className="secondary-button" onClick={() => { setPreview(null); setImportDocument(""); }} type="button">取消</button></div></div> : null}
      {undoBatch ? <button className="secondary-button undo-import" onClick={() => void undoImport()} type="button"><Undo2 size={16} />撤销上次导入</button> : null}
      {undoAction ? <button className="secondary-button undo-import" onClick={() => void undoManagementAction()} type="button"><Undo2 size={16} />撤销上次固定/降权/删除</button> : null}

      <div className="lexicon-layout">
        <form className="lexicon-editor" onSubmit={(event) => { event.preventDefault(); void saveDraft(); }}>
          <h3>{editing ? "编辑词条" : "新增词条"}</h3>
          <div className="lexicon-form-grid">
            <label>词条<input onChange={(event) => setDraft({ ...draft, text: event.currentTarget.value })} value={draft.text} /></label>
            <label>拼音<input onChange={(event) => setDraft({ ...draft, pinyin: event.currentTarget.value })} placeholder="例如 ge ren ci" value={draft.pinyin} /></label>
            <label>别名<input onChange={(event) => setDraft({ ...draft, aliases: event.currentTarget.value })} placeholder="逗号分隔" value={draft.aliases} /></label>
            <label>专业词包<input onChange={(event) => setDraft({ ...draft, packs: event.currentTarget.value })} placeholder="可留空" value={draft.packs} /></label>
            <label>项目域<input onChange={(event) => setDraft({ ...draft, domains: event.currentTarget.value })} placeholder="与 Project Profile 项目域精确匹配" value={draft.domains} /></label>
            <label>来源<input onChange={(event) => setDraft({ ...draft, source: event.currentTarget.value })} value={draft.source} /></label>
            <label>许可证<input onChange={(event) => setDraft({ ...draft, license: event.currentTarget.value })} value={draft.license} /></label>
          </div>
          <div className="lexicon-flags">
            <label><input checked={draft.fixed} onChange={(event) => setDraft({ ...draft, fixed: event.currentTarget.checked })} type="checkbox" />固定</label>
            <label><input checked={draft.demoted} onChange={(event) => setDraft({ ...draft, demoted: event.currentTarget.checked })} type="checkbox" />降权</label>
            <label><input checked={draft.disabled} onChange={(event) => setDraft({ ...draft, disabled: event.currentTarget.checked })} type="checkbox" />禁用</label>
          </div>
          <div className="lexicon-editor-actions"><button className="primary-button" type="submit">{editing ? <Save size={16} /> : <Plus size={16} />}{editing ? "保存修改" : "新增词条"}</button>{editing ? <button className="secondary-button" onClick={() => { setEditing(false); setDraft(emptyDraft()); }} type="button">取消</button> : null}</div>
        </form>

        <div className="lexicon-table-wrap">
          <div className="lexicon-batch-bar">
            <label><input checked={allSelected} onChange={(event) => setSelected(event.currentTarget.checked ? new Set(entries.map((entry) => entry.id)) : new Set())} type="checkbox" />全选</label>
            <span>已选 {selectedCount}</span>
            <button className="secondary-button" disabled={!selectedCount} onClick={() => void removeSelected()} type="button"><Trash2 size={15} />批量删除</button>
            <button className="secondary-button" onClick={async () => { try { const count = await clearLearnedLexiconEntries(); setMessage(`已清理 ${count} 条误学词。`); await refresh(); } catch (error) { setMessage(errorMessage(error)); } }} type="button"><ArchiveRestore size={15} />清理误学</button>
          </div>
          <div className="lexicon-table" role="table" aria-label="个人词列表">
            {entries.length ? entries.map((entry) => <article className="lexicon-row" key={entry.id}>
              <input aria-label={`选择 ${entry.text}`} checked={selected.has(entry.id)} onChange={(event) => setSelected((current) => { const next = new Set(current); if (event.currentTarget.checked) next.add(entry.id); else next.delete(entry.id); return next; })} type="checkbox" />
              <button className="lexicon-main" onClick={() => { setEditing(true); setDraft(fromEntry(entry)); }} type="button"><strong>{entry.text}</strong><span>{entry.pinyin}</span><small>来源：{entry.sources.map((source) => `${source.source}（${source.license}）`).join("、")}</small></button>
              <div className="lexicon-row-actions">
                <button aria-label="固定词条" className={`icon-button ${entry.fixed ? "is-active" : ""}`} onClick={() => void patchEntry(entry, { fixed: !entry.fixed })} title="固定" type="button"><Pin size={15} /></button>
                <button aria-label="降权词条" className={`icon-button ${entry.demoted ? "is-active" : ""}`} onClick={() => void patchEntry(entry, { demoted: !entry.demoted, baseWeight: entry.demoted ? 100 : -100 })} title="降权" type="button"><ArchiveRestore size={15} /></button>
                <button aria-label="禁用词条" className={`icon-button ${entry.disabled ? "is-active is-danger" : ""}`} onClick={() => void patchEntry(entry, { disabled: !entry.disabled })} title="禁用" type="button"><ShieldOff size={15} /></button>
                <button aria-label="删除词条" className="icon-button is-danger" onClick={() => void removeEntry(entry)} title="删除并可撤销" type="button"><Trash2 size={15} /></button>
              </div>
            </article>) : <div className="empty-state">还没有个人词。可单条新增或导入 JSON 词库。</div>}
          </div>
        </div>
      </div>
      {message ? <p className="settings-message" role="status">{message}</p> : null}
    </section>
  );
}
