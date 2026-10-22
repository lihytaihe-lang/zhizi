import { useEffect, useState } from "react";
import { BookMarked, Check, FolderPlus, Loader2, Pencil, RefreshCw, Trash2, X } from "lucide-react";
import {
  deleteKnowledgeSource,
  ingestKnowledgeSource,
  listKnowledgeSources,
  listKnowledgeTerms,
  reviewKnowledgeTerm,
  saveKnowledgeSource,
  setKnowledgeSourceRetrievalPolicy,
  type KnowledgeSource,
  type KnowledgeSourceInput,
  type KnowledgeTerm,
} from "../lib/knowledge";

const DEFAULT_LOCAL_AUTHORIZATION = "本地文件夹（用户选择）";
const emptyDraft = (): KnowledgeSourceInput => ({
  id: "",
  name: "",
  sourceType: "local_path",
  authorizedScope: "",
  authorization: DEFAULT_LOCAL_AUTHORIZATION,
  projectId: "",
  sensitiveEnabled: true,
  updatedAtMs: Date.now(),
});
const errorText = (error: unknown) => error instanceof Error ? error.message : String(error);

export function folderNameFromPath(path: string): string {
  const withoutTrailingSeparator = path.replace(/[\\/]+$/, "");
  return withoutTrailingSeparator.split(/[\\/]/).pop()?.trim() || "本地文件夹";
}

export async function sourceIdForPath(path: string): Promise<string> {
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(path));
  const shortHash = Array.from(new Uint8Array(digest).slice(0, 6))
    .map((byte) => byte.toString(16).padStart(2, "0"))
    .join("");
  return `local-${shortHash}`;
}

export function KnowledgeManager() {
  const [sources, setSources] = useState<KnowledgeSource[]>([]);
  const [terms, setTerms] = useState<KnowledgeTerm[]>([]);
  const [draft, setDraft] = useState(emptyDraft);
  const [busy, setBusy] = useState("");
  const [message, setMessage] = useState("");
  const pendingTerms = terms.filter((term) => term.reviewState === "pending");

  async function reload() {
    const [nextSources, nextTerms] = await Promise.all([listKnowledgeSources(), listKnowledgeTerms()]);
    setSources(nextSources);
    setTerms(nextTerms);
  }

  useEffect(() => { void reload(); }, []);

  async function run(key: string, action: () => Promise<void>) {
    setBusy(key);
    setMessage("");
    try {
      await action();
      await reload();
    } catch (error) {
      setMessage(errorText(error));
    } finally {
      setBusy("");
    }
  }

  function reviewDraft(term: KnowledgeTerm) {
    const text = (document.getElementById(`text-${term.id}`) as HTMLInputElement).value;
    const pinyin = (document.getElementById(`pinyin-${term.id}`) as HTMLInputElement).value;
    const definition = (document.getElementById(`definition-${term.id}`) as HTMLInputElement).value;
    const candidateLayer = (document.getElementById(`layer-${term.id}`) as HTMLSelectElement).value as "first" | "secondary";
    return { text, pinyin, definition, candidateLayer };
  }

  async function registerAndIngest(input: KnowledgeSourceInput) {
    const source = await saveKnowledgeSource({ ...input, updatedAtMs: Date.now() });
    if (source.sourceType !== "local_path") {
      setMessage(`已登记 ${source.name}；该来源按计划仅登记，不发起网络读取。`);
      return;
    }
    const report = await ingestKnowledgeSource(source.id);
    const refreshedTerms = await listKnowledgeTerms();
    const pendingCount = refreshedTerms.filter(
      (term) => term.sourceId === source.id && term.reviewState === "pending",
    ).length;
    setMessage(
      `已添加并摄取 ${report.documentCount} 文档、${report.chunkCount} 块、${report.extractedTermCount} 术语`
      + `${pendingCount > 0 ? `；有 ${pendingCount} 个词待审核。` : "。"}`
      + `${report.skippedCount > 0 ? ` 跳过 ${report.skippedCount} 项。` : ""}`,
    );
  }

  async function addFolder() {
    await run("add-folder", async () => {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const selected = await open({
        title: "添加本地知识文件夹",
        directory: true,
        multiple: false,
        recursive: true,
        canCreateDirectories: false,
      });
      if (!selected || Array.isArray(selected)) return;
      const nextDraft: KnowledgeSourceInput = {
        id: await sourceIdForPath(selected),
        name: folderNameFromPath(selected),
        sourceType: "local_path",
        authorizedScope: selected,
        authorization: DEFAULT_LOCAL_AUTHORIZATION,
        projectId: "",
        sensitiveEnabled: true,
        updatedAtMs: Date.now(),
      };
      setDraft(nextDraft);
      await registerAndIngest(nextDraft);
    });
  }

  function jumpToReview() {
    const review = document.getElementById("knowledge-term-review");
    review?.scrollIntoView({ behavior: "smooth", block: "start" });
    review?.focus();
  }

  async function reviewPendingBatch(state: "approved" | "rejected") {
    let changedCount = 0;
    let missingPinyinCount = 0;
    for (const term of pendingTerms) {
      const value = reviewDraft(term);
      if (state === "approved" && !value.pinyin.trim()) {
        missingPinyinCount += 1;
        continue;
      }
      const candidateLayer = state === "approved" ? "first" : value.candidateLayer;
      await reviewKnowledgeTerm(
        term.id,
        state,
        value.text,
        value.pinyin,
        value.definition,
        candidateLayer,
        state === "approved",
      );
      changedCount += 1;
    }
    setMessage(state === "approved"
      ? `已批量通过 ${changedCount} 个词并设为第一层${missingPinyinCount > 0 ? `；${missingPinyinCount} 个缺拼音，仍保持待审核。` : "。"}`
      : `已批量拒绝 ${changedCount} 个词。`);
  }

  return <section className="panel data-source-manager" aria-labelledby="knowledge-title">
    <div className="panel-heading">
      <div>
        <h2 id="knowledge-title">个人知识</h2>
        <p>只读取逐项授权的本地路径；纯本地切块与规则抽取。默认无来源，未审核项不进候选。</p>
      </div>
      <span className="status-pill">{sources.length} 个来源 · {pendingTerms.length} 个待审</span>
    </div>

    <div className="data-source-grid">
      <div className="data-source-form">
        <h3>添加本地知识</h3>
        <p>选择一个文件夹后会自动登记并立即本地摄取；无需填写路径或 ID。</p>
        <button
          className="primary-button add-folder-button"
          disabled={Boolean(busy)}
          onClick={() => void addFolder()}
          type="button"
        >
          {busy === "add-folder" ? <Loader2 className="spin" size={16} /> : <FolderPlus size={16} />}
          添加文件夹
        </button>
        <small>使用系统原生目录选择器；不联网，只读取你这次明确选择的文件夹。</small>

        <details className="knowledge-advanced">
          <summary>高级</summary>
          <form onSubmit={(event) => {
            event.preventDefault();
            void run("save-advanced", async () => registerAndIngest(draft));
          }}>
            <small>无 GUI 或需要手填远程挂载路径时使用。选过文件夹后，这里也会预填名称、路径和来源 ID，方便改名。</small>
            <label>来源 ID<input value={draft.id} onChange={(event) => setDraft({ ...draft, id: event.currentTarget.value })} /></label>
            <label>项目 ID（匹配 Project Profile 域）<input value={draft.projectId} onChange={(event) => setDraft({ ...draft, projectId: event.currentTarget.value })} placeholder="留空时使用来源 ID" /></label>
            <label>名称<input value={draft.name} onChange={(event) => setDraft({ ...draft, name: event.currentTarget.value })} /></label>
            <label>类型<select value={draft.sourceType} onChange={(event) => setDraft({ ...draft, sourceType: event.currentTarget.value as KnowledgeSourceInput["sourceType"] })}><option value="local_path">本地文件 / 项目目录</option><option value="notion">Notion 页面（后置）</option></select></label>
            <label>{draft.sourceType === "local_path" ? "绝对路径" : "明确页面 ID"}<input value={draft.authorizedScope} onChange={(event) => setDraft({ ...draft, authorizedScope: event.currentTarget.value })} /></label>
            <label>授权说明<input value={draft.authorization} onChange={(event) => setDraft({ ...draft, authorization: event.currentTarget.value })} /></label>
            <button className="secondary-button" disabled={Boolean(busy)} type="submit"><BookMarked size={16} />登记并摄取</button>
            <small>Notion 只登记范围，不发起网络请求；V07-03 按工单标记后置。</small>
          </form>
        </details>
      </div>

      <div className="data-source-list">
        <h3>来源状态</h3>
        {sources.length === 0 ? <p className="empty-state">尚未授权知识源。</p> : sources.map((source) => {
          const pendingCount = terms.filter(
            (term) => term.sourceId === source.id && term.reviewState === "pending",
          ).length;
          return <article className="data-source-card" key={source.id}>
            <div><strong>{source.name}</strong><span>{source.status} · 检索 {source.retrievalStatus} · 敏感 {source.sensitiveEnabled ? "开" : "关"}</span></div>
            <small>{source.documentCount} 文档 · {source.chunkCount} 块 · {source.termCount} 术语 · 项目 {source.projectId}</small>
            <code>{source.authorizedScope || "授权已撤销"}</code>
            {pendingCount > 0 ? <div className="source-review-notice">
              <strong>有 {pendingCount} 个词待审核</strong>
              <button onClick={jumpToReview} type="button">去审核</button>
            </div> : null}
            <div className="inline-actions">
              <button disabled={source.id === "generated-hotword-review" || source.sourceType === "notion" || source.status === "revoked"} onClick={() => void run(`ingest-${source.id}`, async () => {
                const report = await ingestKnowledgeSource(source.id);
                setMessage(`摄取 ${report.documentCount} 文档、${report.chunkCount} 块、${report.extractedTermCount} 术语；跳过 ${report.skippedCount}。`);
              })} type="button"><RefreshCw size={14} />本地摄取</button>
              <button disabled={source.status === "revoked"} onClick={() => void run(`approval-${source.id}`, async () => {
                await setKnowledgeSourceRetrievalPolicy(source.id, source.retrievalStatus === "approved" ? "pending" : "approved", source.sensitiveEnabled);
              })} type="button">{source.retrievalStatus === "approved" ? "暂停检索" : "批准检索"}</button>
              <button disabled={source.status === "revoked"} onClick={() => void run(`sensitive-${source.id}`, async () => {
                await setKnowledgeSourceRetrievalPolicy(source.id, source.retrievalStatus, !source.sensitiveEnabled);
              })} type="button">{source.sensitiveEnabled ? "关闭敏感源" : "开启敏感源"}</button>
              <button onClick={() => void run(`revoke-${source.id}`, async () => { await deleteKnowledgeSource(source.id, false); })} type="button">撤权并保留</button>
              <button onClick={() => void run(`purge-${source.id}`, async () => { await deleteKnowledgeSource(source.id, true); })} type="button"><Trash2 size={14} />删除及清除</button>
            </div>
          </article>;
        })}
      </div>
    </div>

    <div className="hotword-staging">
      <div className="hotword-toolbar">
        <div>
          <h3 id="knowledge-term-review" tabIndex={-1}>本地术语审核</h3>
          <small>复用同一审核队列：精确批准项可进第一行；相关或低置信项只进第二行；未批准项不进词库。</small>
        </div>
        <div className="inline-actions">
          <button disabled={pendingTerms.length === 0 || Boolean(busy)} type="button" onClick={() => void run("approve-all", async () => reviewPendingBatch("approved"))}><Check size={14} />批量通过到第一层</button>
          <button disabled={pendingTerms.length === 0 || Boolean(busy)} type="button" onClick={() => void run("reject-all", async () => reviewPendingBatch("rejected"))}><X size={14} />批量拒绝</button>
        </div>
      </div>
      {terms.length === 0 ? <p className="empty-state">暂无抽取术语。</p> : terms.map((term) => <div className={`hotword-row knowledge-term-row ${term.pinyin.trim() ? "" : "is-missing-pinyin"}`} key={term.id}>
        <input aria-label={`${term.text} 词条`} defaultValue={term.text} id={`text-${term.id}`} />
        <input aria-invalid={!term.pinyin.trim()} aria-label={`${term.text} 拼音`} defaultValue={term.pinyin} id={`pinyin-${term.id}`} placeholder="必填：空格分隔拼音" />
        <input aria-label={`${term.text} 简短释义`} defaultValue={term.definition} id={`definition-${term.id}`} placeholder="简短释义" />
        <select aria-label={`${term.text} 候选层`} defaultValue={term.candidateLayer} id={`layer-${term.id}`}><option value="first">精确匹配 · 第一行</option><option value="secondary">相关/低置信 · 仅第二行</option></select>
        <small>{term.sourceName} · {term.kind} · 可信 {term.confidenceMillis / 10}% · {term.reviewState}{term.pinyin.trim() ? "" : " · 缺拼音，不得通过"}</small>
        <div className="inline-actions">
          <button type="button" onClick={() => void run(`modify-${term.id}`, async () => {
            const value = reviewDraft(term);
            await reviewKnowledgeTerm(term.id, "pending", value.text, value.pinyin, value.definition, value.candidateLayer, false);
          })}><Pencil size={14} />保存修改</button>
          <button type="button" onClick={() => void run(`approve-${term.id}`, async () => {
            const value = reviewDraft(term);
            await reviewKnowledgeTerm(term.id, "approved", value.text, value.pinyin, value.definition, value.candidateLayer, value.candidateLayer === "first");
          })}><Check size={14} />通过</button>
          <button type="button" onClick={() => void run(`reject-${term.id}`, async () => {
            const value = reviewDraft(term);
            await reviewKnowledgeTerm(term.id, "rejected", value.text, value.pinyin, value.definition, value.candidateLayer, false);
          })}><X size={14} />拒绝</button>
        </div>
      </div>)}
      {busy ? <p className="inline-progress"><Loader2 className="spin" size={15} />处理中</p> : null}
      {message ? <p className="generation-notice">{message}</p> : null}
    </div>
  </section>;
}
