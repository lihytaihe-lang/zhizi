import type { QualityRecordingEvent } from "./lexicon";

export type WeeklyStats = { commits: number; firstChoiceRate: number | null; redoRate: number | null };

const WEEK_MS = 7 * 24 * 60 * 60 * 1000;

/** First-choice and redo rates over the last seven days of the owner's own records. */
export function weeklyStats(events: QualityRecordingEvent[], nowMs: number): WeeklyStats {
  const recent = events.filter(
    (event) => event.recordedAtMs >= nowMs - WEEK_MS && event.outcome === "committed_candidate" && event.selectedText,
  );
  if (recent.length === 0) return { commits: 0, firstChoiceRate: null, redoRate: null };
  const first = recent.filter((event) => event.selectedIndex === 0).length;
  const redo = recent.filter((event) => event.redoAfterCommit).length;
  return { commits: recent.length, firstChoiceRate: first / recent.length, redoRate: redo / recent.length };
}

export function percent(value: number | null): string {
  return value === null ? "—" : `${(value * 100).toFixed(1)}%`;
}
