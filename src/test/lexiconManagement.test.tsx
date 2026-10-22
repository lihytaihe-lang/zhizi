import assert from "node:assert/strict";
import { test } from "node:test";
import { renderToStaticMarkup } from "react-dom/server";
import { DataSourceManager } from "../components/DataSourceManager";
import { folderNameFromPath, KnowledgeManager, sourceIdForPath } from "../components/KnowledgeManager";
import { LexiconManager } from "../components/LexiconManager";
import { MaintenanceManager } from "../components/MaintenanceManager";
import {
  countQualityRecordsInRange, permanentDeletionConfirmation, qualityStatusPresentation,
  QualityRecordingPanel,
} from "../components/QualityRecordingPanel";
import { SettingsPanel } from "../components/SettingsPanel";
import { applyLocalPinyinReadings, type KnowledgeTerm } from "../lib/knowledge";
import { splitLexiconList } from "../lib/lexicon";
import type { QualityRecordingConsent, QualityRecordingEvent, QualityRecordingStatus } from "../lib/lexicon";

test("lexicon management exposes single, batch, import preview, undo, export and learned cleanup actions", () => {
  const markup = renderToStaticMarkup(<LexiconManager />);
  for (const label of [
    "新增词条", "批量删除", "导入", "导出", "发布快照", "清理误学",
    "固定", "降权", "禁用", "来源", "许可证",
    "Project Profile", "无项目", "Agent 上下文", "第二层扩展候选",
  ]) {
    assert.match(markup, new RegExp(label));
  }
});

test("learning cleanup surfaces suspicious evidence in owner language", () => {
  const markup = renderToStaticMarkup(<MaintenanceManager />);
  assert.match(markup, /疑似学歪/);
  assert.match(markup, /搜索词或拼音，例如：法语眼/);
  assert.match(markup, /挤掉出厂首选/);
  assert.match(markup, /上下文、缩写竞争和词典缺词不会混入本组/);
  assert.match(markup, /预览本组 0 条/);
  assert.match(markup, /预览所选 0 条/);
  assert.doesNotMatch(markup, /Rime UserDB|SQLite learned|schema/);
});

test("hotword review and candidate theme controls expose the owner-facing contracts", () => {
  const knowledge = renderToStaticMarkup(<KnowledgeManager />);
  const sources = renderToStaticMarkup(<DataSourceManager />);
  const settings = renderToStaticMarkup(<SettingsPanel
    providerSettings={{
      enabled: false,
      baseUrl: "",
      model: "",
      apiKey: "",
    }}
    shortcutSettings={{ shortcut: "alt+space" }}
    settingsMessage=""
    onProviderChange={() => {}}
    onShortcutChange={() => {}}
    onApplyShortcut={() => {}}
    mcpStatus={{ enabled: false, endpoint: null }}
  />);

  assert.match(knowledge, /同一审核队列/);
  assert.match(knowledge, /未审核项不进候选/);
  assert.match(knowledge, />添加文件夹</);
  assert.match(knowledge, /<details class="knowledge-advanced">/);
  assert.doesNotMatch(knowledge, /<details class="knowledge-advanced" open/);
  assert.match(knowledge, /绝对路径/);
  assert.match(knowledge, /本地文件夹（用户选择）/);
  assert.match(knowledge, /批量通过到第一层/);
  assert.match(knowledge, /批量拒绝/);
  assert.ok(sources.includes(String.raw`词\t拼音[\t释义]`));
  assert.match(sources, /缺拼音且不进候选/);
  assert.match(sources, /Owner 自著/);
  assert.match(settings, /候选皮肤/);
  assert.match(settings, /皮肤外观来自本地目录/);
  for (const displayName of ["简洁", "玄墨", "水豚", "落樱"]) {
    assert.match(settings, new RegExp(displayName));
  }
  assert.match(settings, /安装皮肤目录/);
  assert.match(settings, /安装 \.json/);
  assert.match(settings, /设为当前/);
  assert.match(settings, /当前/);
  assert.match(settings, /切换只在下一次 composition 生效/);
});

test("folder defaults are deterministic and local pinyin only fills missing readings", async () => {
  assert.equal(folderNameFromPath("/Users/owner/Documents/current/"), "current");
  assert.equal(folderNameFromPath(String.raw`C:\Projects\AgentIME`), "AgentIME");
  const firstId = await sourceIdForPath("/Users/owner/Documents/current");
  const secondId = await sourceIdForPath("/Users/owner/Documents/current");
  assert.match(firstId, /^local-[0-9a-f]{12}$/);
  assert.equal(firstId, secondId);

  const term = (text: string, pinyin: string): KnowledgeTerm => ({
    id: text,
    sourceId: "source",
    sourceName: "Source",
    sourcePath: "/tmp/source",
    text,
    pinyin,
    kind: "glossary",
    relation: "",
    definition: "",
    candidateLayer: "first",
    confidenceMillis: 950,
    reviewState: "pending",
    fixed: false,
    lastVerifiedMs: 1,
  });
  const filled = applyLocalPinyinReadings(
    [term("拼音", ""), term("知识", "owner verified")],
    { "拼音": "pin yin", "知识": "zhi shi" },
  );
  assert.equal(filled[0].pinyin, "pin yin");
  assert.equal(filled[1].pinyin, "owner verified");
});

test("quality recording UI uses owner language and keeps implementation words folded", () => {
  const markup = renderToStaticMarkup(<QualityRecordingPanel />);
  const mainMarkup = markup.replace(/<details class="technical-details">[\s\S]*?<\/details>/, "");
  for (const forbidden of ["friction", "层 0", "首位反射提交", "events.jsonl", "Top-N"]) {
    assert.doesNotMatch(mainMarkup, new RegExp(forbidden), `${forbidden} leaked outside 技术细节`);
  }
  assert.match(markup, /<details class="technical-details">/);
  assert.doesNotMatch(markup, /<details class="technical-details" open/);
  assert.match(markup, /Quality\/events.jsonl/);
  assert.match(mainMarkup, /导出记录/);
  assert.match(mainMarkup, /已自动应用 0 条 · 最近一次 暂无/);
  assert.match(mainMarkup, /立即重算/);
  assert.match(mainMarkup, /有 0 次你翻过 3 页以上/);
  assert.match(mainMarkup, /有 0 次你打到一半放弃了/);
  assert.match(mainMarkup, /有 0 次你上屏后又立刻删掉重打/);
  for (const implementationWord of [
    "max_page", "outcome", "duration_ms", "backspaces", "redo_after_commit", "candidate_count",
  ]) {
    assert.doesNotMatch(mainMarkup, new RegExp(implementationWord));
  }

  for (const destructiveLabel of ["全部删除", "撤回并删除全部记录", "按时间范围删除"]) {
    assert.match(mainMarkup, new RegExp(`secondary-button danger-button[^>]*[\\s\\S]*?${destructiveLabel}`));
  }
  assert.match(mainMarkup, /aria-label="删除开始时间" type="datetime-local" value=""/);
  assert.match(mainMarkup, /aria-label="删除结束时间" type="datetime-local" value=""/);
  assert.match(mainMarkup, /请先选择完整时间/);
  assert.match(mainMarkup, /secondary-button danger-button" disabled="" type="button"/);
});

test("combined recording status exposes exactly one correct action in all four states", () => {
  const now = 2_000_000_000_000;
  const notConsented: QualityRecordingConsent = { granted: false, grantedAtMs: 0, noticeVersion: "w7-v1" };
  const consented: QualityRecordingConsent = { granted: true, grantedAtMs: now - 1, noticeVersion: "w7-v1" };
  const off: QualityRecordingStatus = { enabled: false, expiresAtMs: 0, continuous: false };
  assert.deepEqual(qualityStatusPresentation(notConsented, off, now), {
    kind: "not-consented", label: "未记录 —— 需要先同意", actionLabel: "同意并开始",
  });
  assert.deepEqual(qualityStatusPresentation(consented, off, now), {
    kind: "ready", label: "未记录 —— 已同意，但开关未开", actionLabel: "开始记录",
  });
  assert.deepEqual(qualityStatusPresentation(consented, { enabled: true, expiresAtMs: 0, continuous: true }, now), {
    kind: "continuous", label: "正在记录（持续）", actionLabel: "停止",
  });
  assert.deepEqual(qualityStatusPresentation(consented, { enabled: true, expiresAtMs: now + 83 * 60_000, continuous: false }, now), {
    kind: "timed", label: "正在记录（还剩 1 小时 23 分）", actionLabel: "停止",
  });
});

test("time-range preview and all destructive confirmations use exact record counts", () => {
  const event = (id: string, recordedAtMs: number): QualityRecordingEvent => ({
    id, recordedAtMs, pinyin: "ni", candidates: ["你"], candidateComments: [],
    selectedIndex: 0, selectedText: "你", selectedCandidateComment: null,
    appHash: "hash", precedingText: "", fuzzyMask: 0, typoCorrectionEnabled: false,
    activeProject: null, activePacks: [], secondaryEnabled: false,
    maxPage: 0, outcome: "committed_candidate", durationMs: 10, backspaces: 0,
    redoAfterCommit: false, candidateCount: 1,
  });
  const first = new Date("2026-01-01T00:00:01").getTime();
  const second = new Date("2026-01-01T00:00:02").getTime();
  const third = new Date("2026-01-01T00:00:03").getTime();
  const recordedWithModel = event("b", second);
  recordedWithModel.contextModel = { enabled: true, outcome: "reordered", baseOrder: [0], groupSize: 2 };
  const records = [event("a", first), recordedWithModel, event("c", third)];
  assert.equal(countQualityRecordsInRange(records, "", ""), null);
  assert.equal(
    countQualityRecordsInRange(records, "2026-01-01T00:00:01", "2026-01-01T00:00:02"),
    2,
  );
  assert.equal(permanentDeletionConfirmation(9413), "将永久删除 9413 条记录。确定继续吗？");
  assert.equal(
    permanentDeletionConfirmation(9413, true),
    "将永久删除 9413 条记录，并撤回同意。确定继续吗？",
  );
});

test("lexicon list parsing is deterministic and removes duplicate import metadata", () => {
  assert.deepEqual(splitLexiconList("beta, alpha，beta\n gamma "), ["alpha", "beta", "gamma"]);
});
