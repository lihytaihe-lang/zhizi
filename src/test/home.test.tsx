import assert from "node:assert/strict";
import test from "node:test";
import { renderToStaticMarkup } from "react-dom/server";
import { HomePanel } from "../components/HomePanel";
import { percent, weeklyStats } from "../lib/weeklyStats";
import type { QualityRecordingEvent } from "../lib/lexicon";

const now = Date.UTC(2026, 9, 10);
function event(partial: Partial<QualityRecordingEvent>): QualityRecordingEvent {
  return {
    id: "e", recordedAtMs: now - 1000, pinyin: "shi", candidates: ["是"], candidateComments: [""],
    selectedIndex: 0, selectedText: "是", selectedCandidateComment: null, appHash: "a", precedingText: "",
    fuzzyMask: 0, typoCorrectionEnabled: true, activeProject: null, activePacks: [], secondaryEnabled: true,
    maxPage: 0, outcome: "committed_candidate", durationMs: 10, backspaces: 0, redoAfterCommit: false,
    candidateCount: 5, ...partial,
  };
}

test("weekly stats count only this week's committed candidates", () => {
  const stats = weeklyStats([
    event({}), event({ selectedIndex: 2 }), event({ redoAfterCommit: true }),
    event({ outcome: "abandoned", selectedText: "" }),
    event({ recordedAtMs: now - 8 * 24 * 60 * 60 * 1000, selectedIndex: 3 }),
  ], now);
  assert.equal(stats.commits, 3);
  assert.equal(percent(stats.firstChoiceRate), "66.7%");
  assert.equal(percent(stats.redoRate), "33.3%");
  assert.equal(percent(weeklyStats([], now).firstChoiceRate), "—");
});

test("home shows the daily toggles, where to change the skin and the ways out", () => {
  const markup = renderToStaticMarkup(<HomePanel onNavigate={() => {}} />);
  for (const label of ["顺手·知字排序", "持续记录", "手误纠错", "模糊音", "皮肤管理…", "这周的输入", "词库", "记录明细", "高级"]) {
    assert.match(markup, new RegExp(label.replace(/[()（）]/g, ".")), label);
  }
  assert.doesNotMatch(markup, /home-skins/, "one skin management only: the input method menu's gallery");
});
