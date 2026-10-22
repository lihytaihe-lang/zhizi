import { useEffect, useState } from "react";
import {
  AlertTriangle, Download, PlayCircle, RefreshCw, ScanSearch, ShieldCheck,
  Sparkles, Trash2, Undo2,
} from "lucide-react";
import {
  applyPersonalProfileAutoUpdate, compileLexiconSnapshot,
  compileLexiconSnapshotWithOverride, declareOwnerDisambiguation,
  deleteQualityRecord, deleteQualityRecords, deleteQualityRecordsByApp,
  deleteQualityRecordsByTimeRange, downloadJSON, exportQualityRecords,
  freezeReplayGoldenSet, grantQualityRecordingConsent, listPersonalProfileAutoUpdates,
  listQualityRecords, nextSnapshotVersion, personalProfileIntrospection,
  qualityRecordingConsentStatus, qualityRecordingStatus, revokePersonalProfileAutoUpdate,
  revokeQualityRecordingConsent, runPersonalReplayGate, setQualityRecording,
  type FrozenGoldenMeta, type PersonalProfileAutoUpdateItem,
  type PersonalProfileIntrospection, type QualityRecordingConsent,
  type QualityRecordingEvent, type QualityRecordingStatus, type ReplayGateReport,
} from "../lib/lexicon";

const message = (error: unknown) => error instanceof Error ? error.message : String(error);

export type QualityStatusPresentation = {
  kind: "not-consented" | "ready" | "continuous" | "timed";
  label: string;
  actionLabel: "同意并开始" | "开始记录" | "停止";
};

export function qualityStatusPresentation(
  consent: QualityRecordingConsent,
  status: QualityRecordingStatus,
  nowMs: number,
): QualityStatusPresentation {
  if (!consent.granted) {
    return { kind: "not-consented", label: "未记录 —— 需要先同意", actionLabel: "同意并开始" };
  }
  if (status.enabled && status.continuous) {
    return { kind: "continuous", label: "正在记录（持续）", actionLabel: "停止" };
  }
  if (status.enabled && !status.continuous && status.expiresAtMs > nowMs) {
    const remainingMinutes = Math.max(1, Math.ceil((status.expiresAtMs - nowMs) / 60_000));
    const hours = Math.floor(remainingMinutes / 60);
    const minutes = remainingMinutes % 60;
    const remaining = hours > 0 ? `${hours} 小时${minutes ? ` ${minutes} 分` : ""}` : `${minutes} 分`;
    return { kind: "timed", label: `正在记录（还剩 ${remaining}）`, actionLabel: "停止" };
  }
  return { kind: "ready", label: "未记录 —— 已同意，但开关未开", actionLabel: "开始记录" };
}

function rangeMilliseconds(start: string, end: string): { startMs: number; endMs: number } | null {
  if (!start || !end) return null;
  const startMs = new Date(start).getTime();
  const endMs = new Date(end).getTime();
  return Number.isFinite(startMs) && Number.isFinite(endMs) && endMs >= startMs
    ? { startMs, endMs }
    : null;
}

export function countQualityRecordsInRange(
  records: QualityRecordingEvent[], start: string, end: string,
): number | null {
  const range = rangeMilliseconds(start, end);
  if (!range) return null;
  return records.filter((record) =>
    record.recordedAtMs >= range.startMs && record.recordedAtMs <= range.endMs
  ).length;
}

export function permanentDeletionConfirmation(count: number, revokeConsent = false): string {
  return `将永久删除 ${count} 条记录${revokeConsent ? "，并撤回同意" : ""}。确定继续吗？`;
}

function recentLabel(value: number | null): string {
  if (!value) return "暂无";
  const date = new Date(value);
  const now = new Date();
  const time = date.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  const startToday = new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime();
  const startYesterday = startToday - 86_400_000;
  if (value >= startToday) return `今天 ${time}`;
  if (value >= startYesterday) return `昨天 ${time}`;
  return date.toLocaleString();
}

function ownerStatus(item: PersonalProfileAutoUpdateItem): string {
  if (item.status === "active") return item.evidenceKind === "owner_declared_disambiguation" ? "你已指定" : "已经记住";
  if (item.status === "revoked") return "已撤销";
  if (item.status === "blacklisted") return "已被你的屏蔽设置挡住";
  return "长时间没再用，已放回原位";
}

export function QualityRecordingPanel() {
  const [status, setStatus] = useState<QualityRecordingStatus>({ enabled: false, expiresAtMs: 0, continuous: false });
  const [consent, setConsent] = useState<QualityRecordingConsent>({ granted: false, grantedAtMs: 0, noticeVersion: "w7-v1" });
  const [records, setRecords] = useState<QualityRecordingEvent[]>([]);
  const [rangeStart, setRangeStart] = useState("");
  const [rangeEnd, setRangeEnd] = useState("");
  const [notice, setNotice] = useState("");
  const [technicalNotice, setTechnicalNotice] = useState("");
  const [golden, setGolden] = useState<FrozenGoldenMeta | null>(null);
  const [report, setReport] = useState<ReplayGateReport | null>(null);
  const [overrideReason, setOverrideReason] = useState("");
  const [introspection, setIntrospection] = useState<PersonalProfileIntrospection | null>(null);
  const [autoUpdates, setAutoUpdates] = useState<PersonalProfileAutoUpdateItem[]>([]);
  const [introspectionLoading, setIntrospectionLoading] = useState(false);
  const [showChanges, setShowChanges] = useState(false);
  const [declaringPinyin, setDeclaringPinyin] = useState<string | null>(null);
  const [showRawOnly, setShowRawOnly] = useState(false);
  const [nowMs, setNowMs] = useState(() => Date.now());

  const reportError = (error: unknown) => {
    setNotice("操作没有完成，请展开“技术细节”查看原因。");
    setTechnicalNotice(message(error));
  };

  async function refresh() {
    try {
      const [nextStatus, nextConsent, nextRecords, nextIntrospection, nextAutoUpdates] = await Promise.all([
        qualityRecordingStatus(), qualityRecordingConsentStatus(), listQualityRecords(),
        personalProfileIntrospection(), listPersonalProfileAutoUpdates(),
      ]);
      setStatus(nextStatus);
      setConsent(nextConsent);
      setRecords(nextRecords);
      setIntrospection(nextIntrospection);
      setAutoUpdates(nextAutoUpdates);
    } catch (error) { reportError(error); }
  }

  async function publishProfileChange(success: string) {
    try {
      const snapshot = await compileLexiconSnapshot(nextSnapshotVersion());
      setNotice(`${success}，新选择已生效。`);
      setTechnicalNotice(`已发布本地快照 v${snapshot.version}。`);
    } catch (error) {
      setNotice(`${success}，但安全检查暂未允许它进入输入法。`);
      setTechnicalNotice(message(error));
    }
  }

  async function applyAutoUpdate() {
    setIntrospectionLoading(true);
    try {
      const value = await applyPersonalProfileAutoUpdate();
      setAutoUpdates(value.items);
      setIntrospection(await personalProfileIntrospection());
      await publishProfileChange("已经重新检查并更新记住的选择");
    } catch (error) { reportError(error); }
    finally { setIntrospectionLoading(false); }
  }

  async function declarePreference(pinyin: string, text: string, candidates: string[]) {
    setIntrospectionLoading(true);
    try {
      const value = await declareOwnerDisambiguation(pinyin, text, candidates);
      setAutoUpdates(value.items);
      setDeclaringPinyin(null);
      setShowChanges(true);
      await publishProfileChange(`已按你的指定记住 ${pinyin} → ${text}`);
    } catch (error) { reportError(error); }
    finally { setIntrospectionLoading(false); }
  }

  async function revokeAutoUpdate(id: string) {
    setIntrospectionLoading(true);
    try {
      const item = await revokePersonalProfileAutoUpdate(id);
      setAutoUpdates(await listPersonalProfileAutoUpdates());
      await publishProfileChange(`已撤销 ${item.pinyin} → ${item.text}，以后也不会自己加回来`);
    } catch (error) { reportError(error); }
    finally { setIntrospectionLoading(false); }
  }

  async function refreshRecords() {
    setRecords(await listQualityRecords());
    setIntrospection(await personalProfileIntrospection());
  }

  async function runStatusAction(presentation: QualityStatusPresentation) {
    try {
      if (presentation.kind === "not-consented") {
        setConsent(await grantQualityRecordingConsent());
        setStatus(await setQualityRecording("twoHour"));
        setNotice("已同意并开始记录 2 小时。");
      } else if (presentation.kind === "ready") {
        setStatus(await setQualityRecording("twoHour"));
        setNotice("已开始记录 2 小时。");
      } else {
        setStatus(await setQualityRecording("off"));
        setNotice("已停止记录。");
      }
    } catch (error) { reportError(error); }
  }

  async function revokeConsent(deleteRecords: boolean) {
    try {
      if (deleteRecords) {
        const current = await listQualityRecords();
        setRecords(current);
        if (!window.confirm(permanentDeletionConfirmation(current.length, true))) return;
      }
      const result = await revokeQualityRecordingConsent(deleteRecords);
      setConsent(result.consent);
      setStatus({ enabled: false, expiresAtMs: 0, continuous: false });
      if (deleteRecords) setRecords([]);
      setNotice(deleteRecords
        ? `已撤回同意并永久删除 ${result.deletedCount} 条记录。`
        : "已撤回同意并停止记录；以前的记录仍保留在这台机器上。");
    } catch (error) { reportError(error); }
  }

  async function deleteAllRecords() {
    try {
      const current = await listQualityRecords();
      setRecords(current);
      if (!window.confirm(permanentDeletionConfirmation(current.length))) return;
      const count = await deleteQualityRecords();
      setRecords([]);
      setIntrospection(await personalProfileIntrospection());
      setNotice(`已永久删除 ${count} 条记录。`);
    } catch (error) { reportError(error); }
  }

  async function deleteTimeRange() {
    const range = rangeMilliseconds(rangeStart, rangeEnd);
    if (!range) {
      setNotice("请先完整选择开始和结束时间。");
      return;
    }
    try {
      const current = await listQualityRecords();
      setRecords(current);
      const count = current.filter((record) =>
        record.recordedAtMs >= range.startMs && record.recordedAtMs <= range.endMs
      ).length;
      if (!window.confirm(permanentDeletionConfirmation(count))) return;
      const deleted = await deleteQualityRecordsByTimeRange(range.startMs, range.endMs);
      await refreshRecords();
      setNotice(`已永久删除所选时间内的 ${deleted} 条记录。`);
    } catch (error) { reportError(error); }
  }

  useEffect(() => { void refresh(); }, []);
  useEffect(() => {
    const timer = window.setInterval(() => setNowMs(Date.now()), 30_000);
    return () => window.clearInterval(timer);
  }, []);

  const presentation = qualityStatusPresentation(consent, status, nowMs);
  const continuousActive = consent.granted && status.enabled && status.continuous;
  const timedActive = consent.granted && status.enabled && !status.continuous && status.expiresAtMs > nowMs;
  const activeLearned = autoUpdates.filter((item) => item.status === "active" && item.evidenceKind === "friction");
  const lastAutomaticUpdate = activeLearned.reduce<number | null>(
    (latest, item) => Math.max(latest ?? 0, item.lastRefreshedAtMs), null,
  );
  const rangeCount = countQualityRecordsInRange(records, rangeStart, rangeEnd);
  const displayedRecords = showRawOnly
    ? records.filter((record) => record.selectedText === record.pinyin)
    : records;
  const percent = (value: number) => `${(value * 100).toFixed(2)}%`;

  return <div className="settings-section quality-section">
    <div className="settings-section-heading">
      <ShieldCheck size={17} />
      <div><h3>打字记录</h3><p>帮输入法记住你的选择；密码和其他敏感输入永远不记录</p></div>
    </div>

    <section className={`recording-status is-${presentation.kind}`} aria-labelledby="recording-status-title">
      <span aria-live="polite" id="recording-status-title" role="status">
        <b aria-hidden="true">{presentation.kind === "continuous" || presentation.kind === "timed" ? "●" : "○"}</b>
        {presentation.label}
      </span>
      <button className="primary-button" onClick={() => void runStatusAction(presentation)} type="button">
        {presentation.actionLabel}
      </button>
    </section>

    <div className="quality-primary-actions">
      <button className="primary-button" onClick={async () => {
        try {
          downloadJSON(`quality-golden-${Date.now()}.json`, await exportQualityRecords());
          setNotice("记录已导出。");
        } catch (error) { reportError(error); }
      }} type="button"><Download size={15} />导出记录</button>
    </div>

    <div className="profile-introspection" aria-labelledby="profile-introspection-title">
      <div className="profile-introspection-heading">
        <ScanSearch aria-hidden="true" size={18} />
        <div><strong id="profile-introspection-title">输入法正在学什么</strong><p>只看这台机器上的打字记录；顺手按下第一个的不算你的偏好。</p></div>
        <button className="secondary-button" disabled={introspectionLoading} onClick={() => void applyAutoUpdate()} type="button">
          <RefreshCw size={14} />{introspectionLoading ? "正在检查" : "立即重算"}
        </button>
      </div>

      <div className="auto-update-lead" aria-live="polite">
        <Sparkles aria-hidden="true" size={17} />
        <strong>已自动应用 {activeLearned.length} 条 · 最近一次 {recentLabel(lastAutomaticUpdate)}</strong>
      </div>

      <div className="owner-insight-grid composition-insights" aria-label="最近的输入经历">
        <section className="owner-insight"><h4>有 {introspection?.deepPageEvents ?? 0} 次你翻过 3 页以上</h4></section>
        <section className="owner-insight"><h4>有 {introspection?.abandonedEvents ?? 0} 次你打到一半放弃了</h4></section>
        <section className="owner-insight"><h4>有 {introspection?.redoEvents ?? 0} 次你上屏后又立刻删掉重打</h4></section>
      </div>

      {introspection ? <div className="owner-insight-grid">
        <section className="owner-insight">
          <h4>有 {introspection.frictionEvents} 次你得往后翻才找到想要的字</h4>
          <p>已自动记住其中 {activeLearned.length} 个。</p>
          <button className="text-button" onClick={() => setShowChanges((value) => !value)} type="button">
            {showChanges ? "收起变化" : "看看改了什么"}
          </button>
          {showChanges ? <div className="profile-signal-list" aria-label="已经记住的选择">
            {autoUpdates.map((item) => <div className="profile-signal" key={item.id}>
              <strong>{item.pinyin} → {item.text}</strong>
              <span>{ownerStatus(item)} · {recentLabel(item.statusChangedAtMs)}</span>
              {item.status !== "revoked" ? <button className="secondary-button compact-button" disabled={introspectionLoading} onClick={() => void revokeAutoUpdate(item.id)} type="button"><Undo2 size={13} />撤销</button> : null}
            </div>)}
            {!autoUpdates.length ? <p className="empty-state">还没有记住新的选择。</p> : null}
          </div> : null}
        </section>

        <section className="owner-insight">
          <h4>有 {introspection.rawCommitEvents} 次整串没打出来</h4>
          <p>这类现在多数已经能打出来；下面是最近常见的几条。</p>
          <div className="plain-example-list">
            {introspection.wholeStringTargets.slice(0, 4).map((target) => <span key={target.pinyin}>{target.pinyin}</span>)}
            {!introspection.wholeStringTargets.length ? <span>暂时没有</span> : null}
          </div>
          <button className="text-button" onClick={() => {
            setShowRawOnly(true);
            document.getElementById("quality-records")?.scrollIntoView({ behavior: "smooth" });
          }} type="button">反馈还打不出的</button>
        </section>

        <section className="owner-insight">
          <h4>有 {introspection.homophoneSplitCount} 个读音你两个字都用</h4>
          <p>这类不会自动替你选默认，否则很容易一半对、一半错。</p>
          <div className="profile-signal-list">
            {introspection.homophoneSplits.slice(0, 6).map((split) => <div className="profile-signal" key={split.pinyin}>
              <strong>{split.pinyin}</strong>
              <span>{split.choices.map((choice) => choice.text).join(" / ")}</span>
              {declaringPinyin === split.pinyin ? <div className="preference-choices" aria-label={`为 ${split.pinyin} 指定常用词`}>
                {split.choices.map((choice) => <button className="secondary-button" disabled={introspectionLoading} key={choice.text} onClick={() => void declarePreference(split.pinyin, choice.text, split.choices.map((item) => item.text))} type="button">选“{choice.text}”</button>)}
                <button className="text-button" onClick={() => setDeclaringPinyin(null)} type="button">取消</button>
              </div> : <button className="text-button" onClick={() => setDeclaringPinyin(split.pinyin)} type="button">我来指定</button>}
            </div>)}
            {!introspection.homophoneSplits.length ? <p className="empty-state">暂时没有需要指定的读音。</p> : null}
          </div>
        </section>
      </div> : <p className="empty-state">打开桌面管理台后，会在这里说明输入法记住了什么，以及你下一步能做什么。</p>}
    </div>

    <details className="danger-zone">
      <summary><AlertTriangle aria-hidden="true" size={15} />更多记录操作</summary>
      <p>删除后无法恢复。每次删除前都会再次显示准确条数，请你确认。</p>
      <div className="danger-actions">
        <button className="secondary-button danger-button" disabled={!records.length} onClick={() => void deleteAllRecords()} type="button"><Trash2 size={14} />全部删除</button>
        <button className="secondary-button danger-button" disabled={!consent.granted || !records.length} onClick={() => void revokeConsent(true)} type="button"><Trash2 size={14} />撤回并删除全部记录</button>
      </div>
      <div className="quality-time-delete" aria-label="按时间删除记录">
        <label>开始时间<input aria-label="删除开始时间" onChange={(event) => setRangeStart(event.currentTarget.value)} type="datetime-local" value={rangeStart} /></label>
        <label>结束时间<input aria-label="删除结束时间" onChange={(event) => setRangeEnd(event.currentTarget.value)} type="datetime-local" value={rangeEnd} /></label>
        <div className="range-delete-submit">
          <span aria-live="polite">{rangeCount === null ? "请先选择完整时间" : `将删除 ${rangeCount} 条`}</span>
          <button className="secondary-button danger-button" disabled={rangeCount === null || rangeCount === 0} onClick={() => void deleteTimeRange()} type="button"><Trash2 size={14} />按时间范围删除</button>
        </div>
      </div>
    </details>

    <div className="quality-records" id="quality-records" aria-label="逐条打字记录">
      <div className="records-heading">
        <strong>{showRawOnly ? "还没打出来的记录" : "最近的打字记录"}</strong>
        {showRawOnly ? <button className="text-button" onClick={() => setShowRawOnly(false)} type="button">显示全部</button> : null}
      </div>
      {displayedRecords.map((record) => <article className="quality-record" key={record.id}>
        <strong>{record.pinyin} → {record.selectedText}</strong>
        <span>时间：{new Date(record.recordedAtMs).toLocaleString()}</span>
        <span title={record.appHash}>使用位置：{record.appHash.slice(0, 12)}…</span>
        <span>你输入：{record.pinyin} · 你选了：{record.selectedText}</span>
        <span title={record.candidates.join(" · ")}>当时看到：{record.candidates.join(" · ")}</span>
        <span title={record.precedingText}>前面文字（已遮住敏感内容）：{record.precedingText || "（空）"}</span>
        <div className="quality-record-actions">
          <button className="secondary-button" onClick={async () => { try { const count = await deleteQualityRecord(record.id); await refreshRecords(); setNotice(`已删除 ${count} 条记录。`); } catch (error) { reportError(error); } }} type="button"><Trash2 size={13} />删除此条</button>
          <button className="secondary-button" onClick={async () => { try { const count = await deleteQualityRecordsByApp(record.appHash); await refreshRecords(); setNotice(`已删除这个使用位置的 ${count} 条记录。`); } catch (error) { reportError(error); } }} type="button"><Trash2 size={13} />删除这个位置的全部记录</button>
        </div>
      </article>)}
      {!displayedRecords.length ? <p className="empty-state">暂无记录。</p> : null}
    </div>

    <details className="technical-details">
      <summary>技术细节</summary>
      <section className={`quality-consent ${consent.granted ? "is-granted" : ""}`} aria-labelledby="quality-consent-title">
        <strong id="quality-consent-title">本地质量记录知情同意与独立开关</strong>
        <ul>
          <li>记录什么：拼音、候选与提交词、最多 8 个 UTF-16 单元的脱敏前文片段、App 标识的 SHA-256 散列及本地候选配置。</li>
          <li>存在哪里：仅本机 <code>Quality/events.jsonl</code>，不因本同意产生任何网络上传。</li>
          <li>保留多久：2 小时只限制采集时段；已记录内容保留到手动删除，持续采集本身不自动过期。</li>
          <li>怎么撤回：撤回后立即停止新记录，可保留既有记录。</li>
        </ul>
        <div className="quality-actions">
          {!consent.granted ? <button className="secondary-button" onClick={async () => { try { setConsent(await grantQualityRecordingConsent()); setNotice("已同意，但还没有开始记录。"); } catch (error) { reportError(error); } }} type="button">只同意，暂不开始</button> : <>
            <span className="consent-state">已同意 · {new Date(consent.grantedAtMs).toLocaleString()}</span>
            <button className="secondary-button" onClick={() => void revokeConsent(false)} type="button">撤回同意（保留记录）</button>
          </>}
        </div>
      </section>
      <label className="toggle-row"><input checked={timedActive} disabled={!consent.granted} onChange={async (event) => { try { setStatus(await setQualityRecording(event.currentTarget.checked ? "twoHour" : "off")); } catch (error) { reportError(error); } }} type="checkbox" /><span>开启 2 小时本地质量记录</span></label>
      <label className="toggle-row owner-toggle"><input checked={continuousActive} disabled={!consent.granted} onChange={async (event) => { try { setStatus(await setQualityRecording(event.currentTarget.checked ? "continuous" : "off")); } catch (error) { reportError(error); } }} type="checkbox" /><span>持续采集（owner/dev，不自动过期）</span></label>
      <p>自省只读 Quality/events.jsonl；friction 必须至少 2 次且领先 runner-up 2 倍才能写入层 0，首位反射提交不计信号。Top-N、整串 raw commit 与 owner_declared_disambiguation 共用现有自更新审计通道。</p>
      <section className="profile-auto-updates"><h4>自更新审计</h4><div className="profile-signal-list">{autoUpdates.map((item) => <div className="profile-signal" key={`technical:${item.id}`}><strong>{item.pinyin} → {item.text}</strong><span>{item.evidenceKind} · friction {item.frictionCount}:{item.runnerUpFrictionCount} · {item.status} · latest {item.latestFrictionAtMs}</span></div>)}</div></section>
      <div className="replay-gate-card">
        <div><strong>V04-07 Replay Gate</strong><p>只冻结 owner 真实质量记录；报告不包含拼音、候选或上屏正文。</p></div>
        <div className="quality-actions">
          <button className="secondary-button" onClick={async () => { try { const value = await freezeReplayGoldenSet(); setGolden(value); } catch (error) { reportError(error); } }} type="button">冻结真实 golden set</button>
          <button className="primary-button" onClick={async () => { try { setReport(await runPersonalReplayGate()); } catch (error) { reportError(error); } }} type="button"><PlayCircle size={15} />运行 Replay Gate</button>
        </div>
        {golden ? <small>Golden：{golden.sampleCount} 条 · {golden.checksum}</small> : null}
        {report ? <div className={`replay-report is-${report.status}`}><strong>{report.status === "passed" ? "PASS" : report.status === "failed" ? "BLOCKED" : "INSUFFICIENT DATA"}</strong><span>样本 {report.sampleCount} · Top-1 {percent(report.candidateTop1Rate)} · Top-5 {percent(report.candidateTop5Rate)}</span><span>首选退化 {percent(report.firstChoiceRegressionRate)} · 抖动 {report.jitterCases} · P95/P99 {report.latencyP95Milliseconds.toFixed(3)}/{report.latencyP99Milliseconds.toFixed(3)} ms</span></div> : null}
        {report && report.status !== "passed" ? <div className="override-row"><input aria-label="人工 override 原因" onChange={(event) => setOverrideReason(event.currentTarget.value)} placeholder="人工 override 原因（写入本地审计）" value={overrideReason} /><button className="secondary-button" disabled={!overrideReason.trim()} onClick={async () => { try { const value = await compileLexiconSnapshotWithOverride(nextSnapshotVersion(), overrideReason); setTechnicalNotice(`已人工 override 发布快照 v${value.snapshot.version}`); setOverrideReason(""); } catch (error) { reportError(error); } }} type="button">审计后 override 发布</button></div> : null}
      </div>
      {technicalNotice ? <pre className="technical-notice">{technicalNotice}</pre> : null}
    </details>

    {notice ? <p className="settings-message" role="status">{notice}</p> : null}
  </div>;
}
