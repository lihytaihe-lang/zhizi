import { useEffect, useState } from "react";
import {
  ArchiveRestore,
  Download,
  Gauge,
  Loader2,
  ShieldCheck,
  Trash2,
  Undo2,
} from "lucide-react";
import { downloadJSON } from "../lib/lexicon";
import {
  applyLearningCleanup,
  exportMaintenanceBackup,
  learningPreferences,
  listLearningCleanupItems,
  listPersonalBlacklist,
  previewLearningCleanup,
  previewMaintenanceBackup,
  restoreMaintenanceBackup,
  runIntegrityReport,
  runScaleProbe,
  saveLearningPreferences,
  undoLearningCleanup,
  undoPersonalBlacklist,
  type BackupPreview,
  type IntegrityReport,
  type LearningCleanupItem,
  type LearningCleanupPreview,
  type LearningPreferences,
  type ModelAuditPair,
  type PersonalBlacklistItem,
  type ScaleProbeReport,
} from "../lib/maintenance";

const errorText = (error: unknown) => error instanceof Error ? error.message : String(error);
const evidenceTime = (value: number | null, unavailable: string) =>
  value === null
    ? unavailable
    : new Intl.DateTimeFormat("zh-CN", {
      year: "numeric",
      month: "2-digit",
      day: "2-digit",
      hour: "2-digit",
      minute: "2-digit",
    }).format(new Date(value));

export function MaintenanceManager() {
  const [preferences, setPreferences] = useState<LearningPreferences | null>(null);
  const [learned, setLearned] = useState<LearningCleanupItem[]>([]);
  const [unmatchedModelAuditPairs, setUnmatchedModelAuditPairs] = useState<ModelAuditPair[]>([]);
  const [learningQuery, setLearningQuery] = useState("");
  const [selectedIds, setSelectedIds] = useState<Set<string>>(new Set());
  const [blacklist, setBlacklist] = useState<PersonalBlacklistItem[]>([]);
  const [selectedBlacklistIds, setSelectedBlacklistIds] = useState<Set<string>>(new Set());
  const [domain, setDomain] = useState("");
  const [cleanup, setCleanup] = useState<LearningCleanupPreview | null>(null);
  const [undoBatch, setUndoBatch] = useState("");
  const [backupDocument, setBackupDocument] = useState("");
  const [backupPreview, setBackupPreview] = useState<BackupPreview | null>(null);
  const [integrity, setIntegrity] = useState<IntegrityReport | null>(null);
  const [scale, setScale] = useState<ScaleProbeReport | null>(null);
  const [busy, setBusy] = useState("");
  const [message, setMessage] = useState("");

  async function reload() {
    const [nextPreferences, listing, blocked] = await Promise.all([
      learningPreferences(),
      listLearningCleanupItems(),
      listPersonalBlacklist(),
    ]);
    const entries = listing.items;
    setPreferences(nextPreferences);
    setLearned(entries);
    setUnmatchedModelAuditPairs(listing.unmatchedModelAuditPairs);
    setBlacklist(blocked);
    setSelectedIds((current) =>
      new Set([...current].filter((id) => entries.some((entry) => entry.id === id))));
    setSelectedBlacklistIds((current) =>
      new Set([...current].filter((id) => blocked.some((entry) => entry.id === id))));
  }

  useEffect(() => {
    void reload();
  }, []);

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

  const normalizedQuery = learningQuery.trim().toLocaleLowerCase();
  const visibleLearning = learned.filter((entry) =>
    normalizedQuery.length === 0
    || entry.text.toLocaleLowerCase().includes(normalizedQuery)
    || entry.pinyin.toLocaleLowerCase().includes(normalizedQuery)
    || entry.domain?.toLocaleLowerCase().includes(normalizedQuery));
  const suspectedLearning = visibleLearning.filter((entry) => entry.suspected).slice(0, 100);
  const otherLearning = visibleLearning.filter((entry) => !entry.suspected).slice(0, 50);
  const batchCleanupLearning = visibleLearning.filter((entry) => entry.batchCleanupEligible);

  function toggleLearningEntry(id: string, checked: boolean) {
    setSelectedIds((current) => {
      const next = new Set(current);
      if (checked) next.add(id);
      else next.delete(id);
      setCleanup(null);
      return next;
    });
  }

  function learningEntry(entry: LearningCleanupItem) {
    const sourceLabel = entry.source === "rime_userdb" ? "输入习惯" : "个人词";
    const excludedAttribution = entry.attribution === "needs-context"
      ? "需要上下文，本组不处理"
      : entry.attribution === "abbrev-competition"
        ? "缩写竞争，本组不处理"
        : entry.attribution === "dictionary-gap"
          ? "词典缺词，本组不处理"
          : null;
    return <label className="learned-item learned-item-evidence" key={entry.id}>
      <input
        aria-label={`选择 ${entry.text}`}
        checked={selectedIds.has(entry.id)}
        onChange={(event) => toggleLearningEntry(entry.id, event.currentTarget.checked)}
        type="checkbox"
      />
      <span>
        <strong>{entry.text}</strong>
        <small>{entry.pinyin} · {entry.domain || "全局"} · {sourceLabel}</small>
        <small className="learning-evidence">
          <span>学到：{evidenceTime(entry.learnedAtMs, "历史时间不可得")}</span>
          <span>最近命中：{evidenceTime(entry.lastHitAtMs, "暂无记录")}</span>
          <span>
            受苦记录 {entry.frictionEventCount} 次
            （放弃 {entry.abandonedCount} · 翻页 {entry.deepPageCount} · 回删 {entry.backspaceCount}）
          </span>
        </small>
        {entry.modelAudit ? <small className="learning-evidence">
          模型复核：按上下文更像「{entry.modelAudit.suggested}」（{entry.modelAudit.flagged}/{entry.modelAudit.occurrences} 次）
        </small> : null}
      </span>
      <em className={`source-tag ${entry.suspected ? "suspected" : "ordinary"}`}>
        {entry.displacedFactoryTop1
          ? `曾挤掉出厂首选「${entry.factoryTop1}」`
          : excludedAttribution || (entry.modelAudit ? "待人工复核" : entry.suspected ? "有受苦记录" : "普通记录")}
      </em>
    </label>;
  }

  return <section className="panel maintenance-manager" aria-labelledby="maintenance-title">
    <div className="panel-heading">
      <div>
        <h2 id="maintenance-title">维护与恢复</h2>
        <p>学习纠错、无密钥备份、数据完整性和规模探针集中在同一工作台。</p>
      </div>
      <span className="status-pill">本地数据 · 危险动作先预览</span>
    </div>
    <div className="maintenance-grid">
      <article className="maintenance-card">
        <h3>学习透明度</h3>
        <p>学习记录按可复算证据排序；清理只取消学习提升，基础词仍可选择并重新学习。安全输入学习永久关闭。</p>
        {preferences ? <>
          <label className="toggle-row">
            <input
              checked={preferences.appLearning}
              onChange={(event) =>
                setPreferences({ ...preferences, appLearning: event.currentTarget.checked })}
              type="checkbox"
            />
            允许普通 App 本地学习
          </label>
          <label className="toggle-row">
            <input
              checked={preferences.projectLearning}
              onChange={(event) =>
                setPreferences({ ...preferences, projectLearning: event.currentTarget.checked })}
              type="checkbox"
            />
            允许项目域本地学习
          </label>
          <label className="toggle-row">
            <input checked={false} disabled type="checkbox" />
            安全输入学习（强制关闭）
          </label>
          <button
            onClick={() => void run("preferences", async () => {
              await saveLearningPreferences({
                ...preferences,
                secureInputLearning: false,
                updatedAtMs: Date.now(),
              });
              setMessage("学习控制已保存。");
            })}
            type="button"
          >
            保存学习控制
          </button>
        </> : null}
        <input
          aria-label="搜索学习记录"
          placeholder="搜索词或拼音，例如：法语眼"
          value={learningQuery}
          onChange={(event) => setLearningQuery(event.currentTarget.value)}
        />
        <h4>疑似学歪</h4>
        <p>成组撤销只包含对照确认、曾挤掉出厂首选的学歪污染；上下文、缩写竞争和词典缺词不会混入本组。</p>
        <button
          disabled={batchCleanupLearning.length === 0}
          onClick={() => void run("preview-cleanup-group", async () => {
            const ids = batchCleanupLearning.map((entry) => entry.id);
            setSelectedIds(new Set(ids));
            setCleanup(await previewLearningCleanup(ids, domain.trim() || null));
          })}
          type="button"
        >
          <Trash2 size={14} />预览本组 {batchCleanupLearning.length} 条
        </button>
        <div className="learned-list" aria-label="疑似学歪列表">
          {suspectedLearning.length === 0
            ? <small>当前筛选下没有疑似学歪记录。</small>
            : suspectedLearning.map(learningEntry)}
        </div>
        <details className="learning-other-records">
          <summary>其余学习记录（{visibleLearning.length - suspectedLearning.length}）</summary>
          <div className="learned-list" aria-label="其余学习记录列表">
            {otherLearning.map(learningEntry)}
          </div>
        </details>
        <input
          placeholder="可选：只清理指定项目域的个人词"
          value={domain}
          onChange={(event) => {
            setDomain(event.currentTarget.value);
            setCleanup(null);
          }}
        />
        <div className="inline-actions">
          <button
            disabled={selectedIds.size === 0}
            onClick={() => void run("preview-cleanup", async () => {
              setCleanup(await previewLearningCleanup([...selectedIds], domain.trim() || null));
            })}
            type="button"
          >
            <Trash2 size={14} />预览所选 {selectedIds.size} 条
          </button>
          {cleanup ? <button
            disabled={cleanup.entryCount === 0}
            onClick={() => void run("apply-cleanup", async () => {
              const result = await applyLearningCleanup(
                cleanup.entryIds,
                domain.trim() || null,
              );
              setUndoBatch(result.batchId);
              setSelectedIds(new Set());
              setCleanup(null);
              setMessage(`已清理 ${result.deletedCount} 条误学；重开输入 session 后生效，可撤销。`);
            })}
            type="button"
          >
            确认清理 {cleanup.entryCount} 条
          </button> : null}
          {undoBatch ? <button
            onClick={() => void run("undo-cleanup", async () => {
              const count = await undoLearningCleanup(undoBatch);
              setUndoBatch("");
              setMessage(`已恢复 ${count} 条学习状态。`);
            })}
            type="button"
          >
            <Undo2 size={14} />撤销
          </button> : null}
        </div>
        {cleanup ? <small>预览：{cleanup.texts.join("、") || "无匹配"}</small> : null}
        {unmatchedModelAuditPairs.length > 0 ? <details className="learning-model-unmatched">
          <summary>模型还发现 {unmatchedModelAuditPairs.length} 组可能的错字，但它们不是学习造成的，暂时无法在这里修</summary>
          <ul aria-label="未匹配学习记录的模型错字">
            {unmatchedModelAuditPairs.map((pair, index) => <li key={index}>
              {pair.committed}→{pair.suggested}（{pair.pinyin}）
            </li>)}
          </ul>
        </details> : null}
      </article>

      <article className="maintenance-card">
        <h3>永久屏蔽</h3>
        <p>候选高亮时按 Shift+Control+Delete，可只屏蔽“这个拼音 → 这个词”；安全输入不会新增。撤销后重开输入 session 生效。</p>
        <div className="learned-list" aria-label="永久屏蔽列表">
          {blacklist.length === 0 ? <small>暂无永久屏蔽项。</small> : blacklist.map((entry) =>
            <label className="learned-item" key={entry.id}>
              <input
                aria-label={`选择撤销 ${entry.text}`}
                checked={selectedBlacklistIds.has(entry.id)}
                onChange={(event) => {
                  const checked = event.currentTarget.checked;
                  setSelectedBlacklistIds((current) => {
                    const next = new Set(current);
                    if (checked) next.add(entry.id);
                    else next.delete(entry.id);
                    return next;
                  });
                }}
                type="checkbox"
              />
              <span><strong>{entry.text}</strong><small>{entry.reading}</small></span>
              <em className="source-tag ordinary">永久屏蔽</em>
            </label>)}
        </div>
        <button
          disabled={selectedBlacklistIds.size === 0}
          onClick={() => void run("undo-blacklist", async () => {
            const count = await undoPersonalBlacklist([...selectedBlacklistIds]);
            setSelectedBlacklistIds(new Set());
            setMessage(`已撤销 ${count} 条永久屏蔽；重开输入 session 后恢复候选。`);
          })}
          type="button"
        >
          <Undo2 size={14} />撤销所选 {selectedBlacklistIds.size} 条
        </button>
      </article>

      <article className="maintenance-card">
        <h3>备份 / 恢复</h3>
        <p>导出个人词、项目、设置、来源配置和知识元数据；访问凭据永不进入备份。</p>
        <button
          onClick={() => void run("export", async () => {
            const document = await exportMaintenanceBackup();
            downloadJSON(`agent-input-backup-${Date.now()}.json`, document);
            setMessage("无密钥备份已导出。");
          })}
          type="button"
        >
          <Download size={14} />导出完整备份
        </button>
        <label className="secondary-button file-button">
          <ArchiveRestore size={14} />选择备份预览
          <input
            accept="application/json,.json"
            onChange={(event) => {
              const file = event.currentTarget.files?.[0];
              if (file) void run("preview-backup", async () => {
                const document = await file.text();
                setBackupDocument(document);
                setBackupPreview(await previewMaintenanceBackup(document));
              });
            }}
            type="file"
          />
        </label>
        {backupPreview ? <div className="backup-preview">
          <strong>格式 v{backupPreview.formatVersion} · 无密钥</strong>
          <span>
            词 {backupPreview.lexiconCount} · 项目 {backupPreview.projectCount}
            · 动态源 {backupPreview.dynamicSourceCount} · 知识源 {backupPreview.knowledgeSourceCount}
          </span>
          <button
            onClick={() => void run("restore", async () => {
              await restoreMaintenanceBackup(backupDocument);
              setBackupDocument("");
              setBackupPreview(null);
              setMessage("备份已原子恢复。");
            })}
            type="button"
          >
            确认原子恢复
          </button>
        </div> : null}
      </article>

      <article className="maintenance-card">
        <h3>诊断与规模</h3>
        <p>检查本地数据完整性与版本兼容，并在隔离内存库上测量 5 万个人词与 10 万知识块查询。</p>
        <div className="inline-actions">
          <button
            onClick={() => void run(
              "integrity",
              async () => setIntegrity(await runIntegrityReport()),
            )}
            type="button"
          >
            <ShieldCheck size={14} />完整性检查
          </button>
          <button
            onClick={() => void run("scale", async () => setScale(await runScaleProbe()))}
            type="button"
          >
            <Gauge size={14} />规模探针
          </button>
        </div>
        {integrity ? <div className="diagnostic-report">
          <strong>
            {integrity.quickCheck === "ok" && integrity.foreignKeyErrors === 0
              ? "完整性通过" : "完整性异常"}
          </strong>
          <span>
            数据版本 {integrity.schemaVersion}/{integrity.expectedSchemaVersion}
            · 关系错误 {integrity.foreignKeyErrors}
            · 词 {integrity.lexiconCount} · 知识块 {integrity.knowledgeChunkCount}
          </span>
          <small>{integrity.recoveryContract}</small>
        </div> : null}
        {scale ? <div className="diagnostic-report">
          <strong>{scale.passed ? "规模查询通过" : "规模查询超限"}</strong>
          <span>
            个人词 {scale.personalEntryCount}：{scale.personalQueryMilliseconds.toFixed(3)} ms/query
          </span>
          <span>
            知识块 {scale.knowledgeChunkCount}：{scale.knowledgeQueryMilliseconds.toFixed(3)} ms/query
          </span>
        </div> : null}
      </article>
    </div>
    {busy ? <p className="inline-progress"><Loader2 className="spin" size={15} />处理中</p> : null}
    {message ? <p className="settings-message" role="status">{message}</p> : null}
  </section>;
}
