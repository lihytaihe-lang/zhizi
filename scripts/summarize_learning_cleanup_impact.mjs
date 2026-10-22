import { createHash } from "node:crypto";
import { readFile, writeFile } from "node:fs/promises";

function argument(name) {
  const index = process.argv.indexOf(name);
  if (index < 0 || !process.argv[index + 1]) throw new Error(`missing ${name}`);
  return process.argv[index + 1];
}

const eventBytes = await readFile(argument("--events"));
const events = eventBytes.toString("utf8")
  .split(/\r?\n/u)
  .filter(Boolean)
  .map(JSON.parse);
const before = JSON.parse(await readFile(argument("--before"), "utf8")).observations;
const after = JSON.parse(await readFile(argument("--after"), "utf8")).observations;
const beforeTop1 = new Map(before.map((item) => [item.query, item.actual[0] ?? ""]));
const afterTop1 = new Map(after.map((item) => [item.query, item.actual[0] ?? ""]));
if (beforeTop1.size !== afterTop1.size) throw new Error("cleanup replay arms differ in query count");

const changedReadings = [...beforeTop1.keys()]
  .filter((reading) => beforeTop1.get(reading) !== afterTop1.get(reading))
  .sort((left, right) => left.localeCompare(right, "en"));
const rich = (event) => Object.hasOwn(event, "outcome")
  && Object.hasOwn(event, "backspaces")
  && Object.hasOwn(event, "redoAfterCommit")
  && Object.hasOwn(event, "maxPage");

function projectTier(metric, rows, wantedText) {
  const known = rows.filter((event) => wantedText(event) !== null);
  const improved = known.filter((event) =>
    beforeTop1.get(event.pinyin) !== wantedText(event)
    && afterTop1.get(event.pinyin) === wantedText(event)).length;
  const regressed = known.filter((event) =>
    beforeTop1.get(event.pinyin) === wantedText(event)
    && afterTop1.get(event.pinyin) !== wantedText(event)).length;
  return {
    metric,
    before: rows.length,
    after: rows.length - improved + regressed,
    knownDesiredEvents: known.length,
    improved,
    regressed,
  };
}

const tiers = [
  // A redo and an abandoned composition do not carry the intended replacement
  // text. Replaying candidates cannot invent it, so these remain unchanged.
  projectTier(
    "redo-after-commit",
    events.filter((event) => rich(event) && event.redoAfterCommit),
    () => null,
  ),
  projectTier(
    "abandoned",
    events.filter((event) => rich(event) && event.outcome === "abandoned"),
    () => null,
  ),
  projectTier(
    "backspace-rework",
    events.filter((event) => rich(event) && event.backspaces > 0),
    (event) => event.selectedText || null,
  ),
  projectTier(
    "non-first-selection",
    events.filter((event) => event.selectedIndex > 0),
    (event) => event.selectedText || null,
  ),
];

const affectedEvents = events.filter((event) => changedReadings.includes(event.pinyin)
  && event.selectedText);
const desiredTop1Before = affectedEvents.filter((event) =>
  beforeTop1.get(event.pinyin) === event.selectedText).length;
const desiredTop1After = affectedEvents.filter((event) =>
  afterTop1.get(event.pinyin) === event.selectedText).length;
const nonFirst = events.filter((event) => event.selectedIndex > 0);
const originalTop1Correct = events.filter(
  (event) => (event.candidates?.[0] ?? "") === event.selectedText,
);
if (nonFirst.length !== 875 || originalTop1Correct.length !== 11_331) {
  throw new Error(
    `owner denominators drifted: non-first=${nonFirst.length} original-correct=${originalTop1Correct.length}`,
  );
}
const preventedEvents = nonFirst.filter((event) =>
  beforeTop1.get(event.pinyin) !== event.selectedText
  && afterTop1.get(event.pinyin) === event.selectedText);
const regressedEvents = originalTop1Correct.filter((event) =>
  beforeTop1.get(event.pinyin) === event.selectedText
  && afterTop1.get(event.pinyin) !== event.selectedText);
const stableByReading = (rows) => {
  const counts = new Map();
  for (const event of rows) counts.set(event.pinyin, (counts.get(event.pinyin) ?? 0) + 1);
  return [...counts.entries()]
    .map(([reading, incidents]) => ({ reading, incidents }))
    .sort((left, right) => right.incidents - left.incidents
      || left.reading.localeCompare(right.reading, "en"));
};

const summary = {
  kind: "learning-cleanup-impact",
  formatVersion: 2,
  sourceKind: "owner-private-quality-events-evaluation-only",
  redactionState: "aggregate-only-no-owner-body-context-app-or-event-identity",
  sourceEventsSHA256: createHash("sha256").update(eventBytes).digest("hex"),
  evidenceSHA256: argument("--evidence-sha256"),
  sourceEvents: events.length,
  uniqueReadingsReplayed: beforeTop1.size,
  eligibleCohortReadings: Number(argument("--eligible-readings")),
  eligibleCohortIncidents: Number(argument("--eligible-incidents")),
  cleanupEntriesDeletedOnPrivateCopy: Number(argument("--deleted")),
  logicalEntriesBefore: Number(argument("--entries-before")),
  logicalEntriesAfterCleanup: Number(argument("--entries-after-cleanup")),
  undoRestoredLogicalDigest: argument("--undo-restored") === "true",
  ownerEntryDigestBefore: argument("--owner-entry-digest-before"),
  ownerEntryDigestAfterUndo: argument("--owner-entry-digest-after-undo"),
  changedTop1Readings: changedReadings.length,
  changedReadingEvidence: changedReadings.map((reading) => ({
    reading,
    beforeTop1: beforeTop1.get(reading),
    afterTop1: afterTop1.get(reading),
    eventCount: events.filter((event) => event.pinyin === reading).length,
  })),
  affectedCommittedEvents: affectedEvents.length,
  desiredTop1Before,
  desiredTop1After,
  benefit: {
    denominator: nonFirst.length,
    prevented: preventedEvents.length,
    preventedPercent: Number((preventedEvents.length / nonFirst.length * 100).toFixed(4)),
    currentBeforeAlreadyCorrect: nonFirst.filter(
      (event) => beforeTop1.get(event.pinyin) === event.selectedText,
    ).length,
    byReading: stableByReading(preventedEvents),
  },
  cost: {
    denominator: originalTop1Correct.length,
    regressed: regressedEvents.length,
    regressedPercent: Number(
      (regressedEvents.length / originalTop1Correct.length * 100).toFixed(4),
    ),
    currentBeforeAlreadyWrong: originalTop1Correct.filter(
      (event) => beforeTop1.get(event.pinyin) !== event.selectedText,
    ).length,
    byReading: stableByReading(regressedEvents),
  },
  netPreventedMinusRegressed: preventedEvents.length - regressedEvents.length,
  tiers,
};

if (!summary.undoRestoredLogicalDigest
    || summary.ownerEntryDigestBefore !== summary.ownerEntryDigestAfterUndo
    || summary.logicalEntriesBefore - summary.logicalEntriesAfterCleanup
      !== summary.cleanupEntriesDeletedOnPrivateCopy
    || summary.eligibleCohortReadings < summary.cleanupEntriesDeletedOnPrivateCopy
    || summary.eligibleCohortIncidents < summary.eligibleCohortReadings
    || summary.benefit.denominator !== 875
    || summary.cost.denominator !== 11_331
    || !/^[0-9a-f]{64}$/u.test(summary.sourceEventsSHA256)
    || !/^[0-9a-f]{64}$/u.test(summary.evidenceSHA256)
    || !/^[0-9a-f]{64}$/u.test(summary.ownerEntryDigestBefore)) {
  throw new Error("learning cleanup impact integrity contract failed");
}
const generated = `${JSON.stringify(summary, null, 2)}\n`;
const reportPath = process.argv.includes("--report") ? argument("--report") : null;
if (process.argv.includes("--write")) {
  if (!reportPath) throw new Error("--write requires --report");
  await writeFile(reportPath, generated, { mode: 0o644 });
} else if (process.argv.includes("--check")) {
  if (!reportPath) throw new Error("--check requires --report");
  if (await readFile(reportPath, "utf8") !== generated) {
    throw new Error("learning cleanup impact report is stale");
  }
} else {
  process.stdout.write(generated);
}
console.log(
  `learning_cleanup_round5_impact=PASS deleted=${summary.cleanupEntriesDeletedOnPrivateCopy}`
    + ` benefit=${summary.benefit.prevented}/${summary.benefit.denominator}`
    + ` regressions=${summary.cost.regressed}/${summary.cost.denominator}`
    + ` net=${summary.netPreventedMinusRegressed}`
    + ` tiers=${summary.tiers.map((tier) => `${tier.metric}:${tier.before}->${tier.after}`).join(",")}`
    + " privacy=PASS",
);
