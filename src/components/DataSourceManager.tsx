import { useEffect, useMemo, useState } from "react";
import { Database, Loader2, Pause, Play, RefreshCw, Trash2 } from "lucide-react";
import {
  clearDynamicSource,
  ingestLocalHotwordSource,
  listDynamicSources,
  listHotwordItems,
  refreshHotwordExposure,
  refreshDynamicSource,
  saveDynamicSource,
  setDynamicSourceEnabled,
  type DynamicSource,
  type DynamicSourceInput,
  type HotwordItem,
} from "../lib/hotwords";
import {
  compileSecondaryPredictionSnapshot,
  nextSnapshotVersion,
  rollbackLexiconSnapshot,
  runHotwordQualityGate,
  type HotwordQualityReport,
} from "../lib/lexicon";

const emptySource = (): DynamicSourceInput => ({
  id: "",
  displayName: "",
  sourceType: "official_json",
  refreshIntervalMinutes: 60,
  authorization: "",
  termsUrl: "",
  license: "",
  cachePolicy: "last-known-good",
  ownership: "remote_cached",
  distribution: "snapshot-only",
  disablePolicy: "pause-and-clear",
  endpoint: "",
  optIn: false,
  updatedAtMs: Date.now(),
});

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export function DataSourceManager() {
  const [sources, setSources] = useState<DynamicSource[]>([]);
  const [items, setItems] = useState<HotwordItem[]>([]);
  const [draft, setDraft] = useState<DynamicSourceInput>(emptySource);
  const [localText, setLocalText] = useState("");
  const [busy, setBusy] = useState("");
  const [message, setMessage] = useState("");
  const [quality, setQuality] = useState<HotwordQualityReport | null>(null);
  const [rollbackVersion, setRollbackVersion] = useState("");
  const reviewCount = useMemo(() => items.filter((item) => item.reviewRequired).length, [items]);

  async function reload() {
    // Recompute 曝光/误选 before reading, otherwise the panel renders the stale stored
    // values. A failure here must not blank the panel — the counters are diagnostics,
    // the word list is the point — so it degrades to showing the last stored numbers.
    try {
      await refreshHotwordExposure();
    } catch {
      // deliberately ignored; the list below still loads
    }
    const [nextSources, nextItems] = await Promise.all([listDynamicSources(), listHotwordItems()]);
    setSources(nextSources);
    setItems(nextItems);
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

  return (
    <section className="panel data-source-manager" aria-labelledby="data-source-title">
      <div className="panel-heading">
        <div>
          <h2 id="data-source-title">热词数据源</h2>
          <p>管理 App 侧取网；输入法核心无网络。所有来源默认关闭且必须登记授权、条款与许可证。</p>
        </div>
        <span className="status-pill">{sources.length} 个来源 · {items.length} 个暂存词 · {reviewCount} 个待审</span>
      </div>

      <div className="data-source-grid">
        <form
          className="data-source-form"
          onSubmit={(event) => {
            event.preventDefault();
            void run("save", async () => {
              await saveDynamicSource({ ...draft, updatedAtMs: Date.now() });
              setDraft(emptySource());
            });
          }}
        >
          <h3>登记来源</h3>
          <label>来源 ID<input value={draft.id} onChange={(event) => setDraft({ ...draft, id: event.currentTarget.value })} /></label>
          <label>名称<input value={draft.displayName} onChange={(event) => setDraft({ ...draft, displayName: event.currentTarget.value })} /></label>
          <div className="two-field-row">
            <label>类型<select value={draft.sourceType} onChange={(event) => setDraft({ ...draft, sourceType: event.currentTarget.value as DynamicSourceInput["sourceType"] })}>
              <option value="official_json">官方 JSON</option><option value="official_xml">官方 XML</option><option value="local_text">本地文本</option>
            </select></label>
            <label>归属<select value={draft.ownership} onChange={(event) => setDraft({ ...draft, ownership: event.currentTarget.value as DynamicSourceInput["ownership"] })}>
              <option value="remote_cached">远端缓存</option><option value="user_local">用户本地</option><option value="self-authored">Owner 自著</option><option value="bundled">正式包</option>
            </select></label>
          </div>
          <label>HTTPS endpoint<input value={draft.endpoint} onChange={(event) => setDraft({ ...draft, endpoint: event.currentTarget.value })} /></label>
          <label>授权依据<input value={draft.authorization} onChange={(event) => setDraft({ ...draft, authorization: event.currentTarget.value })} /></label>
          <label>条款 URL<input value={draft.termsUrl} onChange={(event) => setDraft({ ...draft, termsUrl: event.currentTarget.value })} /></label>
          <label>许可证<input value={draft.license} onChange={(event) => setDraft({ ...draft, license: event.currentTarget.value })} /></label>
          <label className="toggle-row"><input type="checkbox" checked={draft.optIn} onChange={(event) => setDraft({ ...draft, optIn: event.currentTarget.checked })} />owner 显式 opt-in</label>
          <button className="primary-button" disabled={busy === "save"} type="submit"><Database size={16} />登记（保持关闭）</button>
          <small>本地文本格式：词\t拼音[\t释义]；旧纯文本行会标为缺拼音且不进候选。</small>
        </form>

        <div className="data-source-list">
          <h3>来源状态</h3>
          {sources.length === 0 ? <p className="empty-state">尚未登记动态来源。</p> : sources.map((source) => (
            <article className="data-source-card" key={source.id}>
              <div><strong>{source.displayName}</strong><span>{source.sourceType} · {source.license || "许可证未登记"}</span></div>
              <small>{source.enabled ? "已启用" : "已暂停"} · {source.lastRefreshMs ? new Date(source.lastRefreshMs).toLocaleString() : "从未刷新"}</small>
              {source.lastError ? <p className="validation">{source.lastError}</p> : null}
              <div className="inline-actions">
                <button type="button" onClick={() => void run(`toggle-${source.id}`, async () => { await setDynamicSourceEnabled(source.id, !source.enabled); })}>
                  {source.enabled ? <Pause size={14} /> : <Play size={14} />}{source.enabled ? "暂停" : "启用"}
                </button>
                <button disabled={!source.enabled || source.sourceType === "local_text"} type="button" onClick={() => void run(`refresh-${source.id}`, async () => { await refreshDynamicSource(source.id); })}><RefreshCw size={14} />刷新</button>
                <button type="button" onClick={() => void run(`clear-${source.id}`, async () => { await clearDynamicSource(source.id); })}><Trash2 size={14} />清除派生数据</button>
              </div>
              {source.sourceType === "local_text" && source.enabled ? (
                <div className="local-source-import">
                  <textarea placeholder={"每行：词\\t拼音[\\t释义]；旧纯文本行会标为缺拼音且不进候选。"} value={localText} onChange={(event) => setLocalText(event.currentTarget.value)} />
                  <button type="button" onClick={() => void run(`local-${source.id}`, async () => { await ingestLocalHotwordSource(source.id, localText); setLocalText(""); })}>本地抽取</button>
                </div>
              ) : null}
            </article>
          ))}
          {busy ? <p className="inline-progress"><Loader2 className="spin" size={15} />处理中</p> : null}
          {message ? <p className="validation">{message}</p> : null}
        </div>
      </div>

      <div className="hotword-staging">
        <div className="hotword-toolbar">
          <div><h3>规范化与第二层关联</h3><small>只有已启用、读音确定、未过期的词可发布；第一层投影必须逐位不变。</small></div>
          <div className="inline-actions">
            <button type="button" onClick={() => void run("publish-prediction", async () => {
              const snapshot = await compileSecondaryPredictionSnapshot(nextSnapshotVersion());
              setMessage(`第二层快照 ${snapshot.version} 已发布。`);
            })}>发布第二层快照</button>
            <button type="button" onClick={() => void run("quality-gate", async () => {
              setQuality(await runHotwordQualityGate());
            })}>运行热词质量 Gate</button>
          </div>
        </div>
        <div className="rollback-row">
          <input inputMode="numeric" placeholder="快照版本" value={rollbackVersion} onChange={(event) => setRollbackVersion(event.currentTarget.value)} />
          <button type="button" onClick={() => void run("rollback", async () => {
            await rollbackLexiconSnapshot(Number(rollbackVersion));
            setMessage(`已回滚到快照 ${rollbackVersion}。`);
          })}>回滚快照</button>
        </div>
        {quality ? <div className={`replay-report is-${quality.status}`}>
          <strong>{quality.status === "passed" ? "热词 Gate 通过" : "热词 Gate：真实语料不足"}</strong>
          <span>固定热词 {quality.fixedHotwordCount} · owner 样本 {quality.ownerSampleCount} · 首层误占 {quality.firstLayerOccupationCount}</span>
          <span>过期 {quality.expiredCount} · 查询 {quality.queryLatencyMilliseconds.toFixed(2)} ms · {quality.networkFallback}</span>
        </div> : null}
        {items.length === 0 ? <p className="empty-state">暂无热词。</p> : items.map((item) => (
          <div className="hotword-row" key={item.id}>
            <strong>{item.text}</strong><span>{item.pinyin || "读音待审核"}</span>
            <small>{item.sourceIds.join(" + ")} · 可信 {item.confidenceMillis / 10}% · {item.state} · 曝光 {item.exposureCount} · 误选 {item.misselectionCount}</small>
          </div>
        ))}
      </div>
    </section>
  );
}
