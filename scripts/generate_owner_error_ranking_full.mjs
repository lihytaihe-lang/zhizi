#!/usr/bin/env node

import { createHash } from "node:crypto";
import { readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(fileURLToPath(new URL("..", import.meta.url)));
const outputPath = path.join(root, "quality/corpus/owner-error-ranking-full-v1.json");
const probePath = path.join(root, "quality/corpus/owner-error-ranking-full-probe-v1.json");
const round1Path = path.join(root, "quality/corpus/owner-error-ranking-v1.json");
const exactReadingsPath = path.join(root, "native/resources/Rime/agent_exact_readings.tsv");
const forbiddenRepositoryFields = new Set([
  "id",
  "recordedAtMs",
  "precedingText",
  "appHash",
  "activeProject",
  "activePacks",
]);

function argument(name) {
  const index = process.argv.indexOf(name);
  return index >= 0 ? process.argv[index + 1] : null;
}

function fail(message) {
  throw new Error(message);
}

function checksum(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

function round(value) {
  return Number(value.toFixed(4));
}

function stableCounts(values) {
  const counts = new Map();
  for (const value of values) counts.set(value, (counts.get(value) ?? 0) + 1);
  return [...counts.entries()]
    .map(([text, count]) => ({ text, count }))
    .sort((left, right) => right.count - left.count
      || left.text.localeCompare(right.text, "zh-Hans-CN"));
}

function transitionCounts(events) {
  const counts = new Map();
  for (const event of events) {
    const wrongTop1 = event.candidates[0] ?? "";
    const key = JSON.stringify([wrongTop1, event.selectedText]);
    const current = counts.get(key) ?? {
      wrongTop1,
      ownerWanted: event.selectedText,
      count: 0,
    };
    current.count += 1;
    counts.set(key, current);
  }
  return [...counts.values()].sort((left, right) => right.count - left.count
    || left.wrongTop1.localeCompare(right.wrongTop1, "zh-Hans-CN")
    || left.ownerWanted.localeCompare(right.ownerWanted, "zh-Hans-CN"));
}

function hasBidirectionalCorrection(transitions) {
  const pairs = new Set(transitions.map(({ wrongTop1, ownerWanted }) =>
    JSON.stringify([wrongTop1, ownerWanted])));
  return transitions.some(({ wrongTop1, ownerWanted }) =>
    wrongTop1 !== ownerWanted && pairs.has(JSON.stringify([ownerWanted, wrongTop1])));
}

function validateEvent(event, lineNumber) {
  if (typeof event.pinyin !== "string" || typeof event.selectedText !== "string") {
    fail(`event line ${lineNumber} lacks pinyin or selectedText`);
  }
  if (!Array.isArray(event.candidates)
      || event.candidates.some((item) => typeof item !== "string")) {
    fail(`event line ${lineNumber} has invalid candidates`);
  }
  if (!Number.isInteger(event.selectedIndex) || event.selectedIndex < 0) {
    fail(`event line ${lineNumber} has invalid selectedIndex`);
  }
  if (!Number.isFinite(event.recordedAtMs) || typeof event.id !== "string") {
    fail(`event line ${lineNumber} lacks frozen identity fields`);
  }
}

async function readEvents(file) {
  const bytes = await readFile(file);
  const events = bytes.toString("utf8").split(/\r?\n/u).filter(Boolean).map((line, index) => {
    const event = JSON.parse(line);
    validateEvent(event, index + 1);
    return event;
  });
  if (events.length === 0) fail("quality event source is empty");
  if (new Set(events.map(({ id }) => id)).size !== events.length) {
    fail("quality event source contains duplicate IDs");
  }
  return { bytes, events };
}

function rankedNonFirstReadings(events) {
  const byReading = new Map();
  for (const event of events) {
    if (event.selectedIndex <= 0) continue;
    const values = byReading.get(event.pinyin) ?? [];
    values.push(event);
    byReading.set(event.pinyin, values);
  }
  return [...byReading.entries()]
    .map(([reading, errorEvents]) => ({ reading, count: errorEvents.length, errorEvents }))
    .sort((left, right) => right.count - left.count
      || left.reading.localeCompare(right.reading, "en"));
}

async function writeProbe(eventsFile) {
  const { events } = await readEvents(eventsFile);
  const ranked = rankedNonFirstReadings(events);
  const probe = {
    kind: "short-input-learning-probe",
    formatVersion: 1,
    cases: ranked.map(({ reading }, index) => ({
      id: `full-rank-${String(index + 1).padStart(3, "0")}`,
      category: "owner-error-ranking-full",
      query: reading,
      allowed: [],
      mustPassTop1: false,
    })),
  };
  await writeFile(probePath, `${JSON.stringify(probe, null, 2)}\n`, { mode: 0o644 });
  console.log(`owner_error_ranking_full_probe=WRITTEN readings=${ranked.length}`);
}

async function readExactReadingIndex() {
  const exactTexts = new Set();
  const syllables = new Set();
  for (const line of (await readFile(exactReadingsPath, "utf8")).split(/\r?\n/u)) {
    if (!line) continue;
    const [text, rawReadings] = line.split("\t");
    if (!text || !rawReadings) continue;
    exactTexts.add(text);
    if (!/\p{Script=Han}/u.test(text)) continue;
    for (const reading of rawReadings.split(",")) {
      for (const syllable of reading.trim().split(/\s+/u)) {
        if (/^[a-z]+$/u.test(syllable)
            && (syllable.length > 1 || ["a", "e", "o"].includes(syllable))) {
          syllables.add(syllable);
        }
      }
    }
  }
  return { exactTexts, syllables };
}

function canFullySyllabify(raw, syllables) {
  const reachable = new Array(raw.length + 1).fill(false);
  reachable[0] = true;
  for (let start = 0; start < raw.length; start += 1) {
    if (!reachable[start]) continue;
    for (let end = start + 1; end <= raw.length; end += 1) {
      if (syllables.has(raw.slice(start, end))) reachable[end] = true;
    }
  }
  return reachable[raw.length];
}

function selectionCommentIsRawEcho(events, wanted) {
  const matching = events.filter((event) => event.selectedText === wanted);
  return matching.length > 0 && matching.every((event) => {
    const direct = typeof event.selectedCandidateComment === "string"
      ? event.selectedCandidateComment : "";
    const indexed = Array.isArray(event.candidateComments)
      ? event.candidateComments[event.selectedIndex] ?? "" : "";
    return `${direct} ${indexed}`.includes("原样");
  });
}

function classify(item, exactIndex) {
  const dominantWanted = item.desiredCounts[0]?.text ?? "";
  const ownerTop1Count = item.selectionCounts
    .find(({ text }) => text === item.ownerTop1)?.count ?? 0;
  const rawEcho = selectionCommentIsRawEcho(item.errorEvents, dominantWanted)
    && !exactIndex.exactTexts.has(dominantWanted);
  if (rawEcho) {
    return {
      attribution: "raw-echo-competition",
      criterion: `“${dominantWanted}”是屏幕可选的“原样”回显，不是精确词典候选；四类枚举不能覆盖这类竞争。`,
    };
  }

  if (item.cleanTop1 !== item.ownerTop1 && ownerTop1Count === 0) {
    return {
      attribution: "learned-pollution",
      criterion: `clean 首选“${item.cleanTop1}”被 owner 臂“${item.ownerTop1}”挤掉，且“${item.ownerTop1}”在冻结记录中被选择 0 次。`,
    };
  }

  if (hasBidirectionalCorrection(item.transitions)) {
    return {
      attribution: "needs-context",
      criterion: "同一读音存在至少一组双向改选 A→B 与 B→A，直接证明两种表达都真实需要。",
    };
  }

  const partialReading = !canFullySyllabify(item.reading, exactIndex.syllables);
  const wantedIsChinese = /\p{Script=Han}/u.test(dominantWanted);
  const wantedIsAvailable = item.cleanCandidates.includes(dominantWanted);
  if (partialReading && wantedIsChinese && wantedIsAvailable) {
    return {
      attribution: "abbrev-competition",
      criterion: `“${item.reading}”不能完整切成全拼音节，正确解“${dominantWanted}”在 clean Top-5 中但被同前缀展开压住。`,
    };
  }

  if (item.selectionCounts.length >= 2 && item.cleanTop1 === item.ownerTop1) {
    return {
      attribution: "needs-context",
      criterion: `记录中实际选择 ${item.selectionCounts.length} 种文本，且 clean/owner 首选同为“${item.cleanTop1}”。`,
    };
  }

  if (item.selectionCounts.length >= 2 && ownerTop1Count > 0) {
    return {
      attribution: "needs-context",
      criterion: `记录中实际选择 ${item.selectionCounts.length} 种文本；owner 首选“${item.ownerTop1}”也被真实选择 ${ownerTop1Count} 次，不是单向垃圾词。`,
    };
  }

  if (!wantedIsAvailable) {
    return {
      attribution: "dictionary-gap",
      criterion: `正确解“${dominantWanted}”不在 clean Top-5 中。`,
    };
  }

  if (!partialReading && item.cleanTop1 === item.ownerTop1) {
    return {
      attribution: "full-pinyin-ranking",
      criterion: `完整全拼“${item.reading}”的正确解“${dominantWanted}”在 clean Top-5，但无双向或多选择上下文证据；它不是缩写位竞争。`,
    };
  }

  if (item.cleanTop1 !== item.ownerTop1 && ownerTop1Count > 0) {
    return {
      attribution: "ranking-drift",
      criterion: `冻结事件发生后 owner 当前首选已变为真实选择过的“${item.ownerTop1}”；历史错误在当前 owner 臂不再以同一首选复现。`,
    };
  }

  fail(`classification rules do not cover reading ${item.reading}`);
}

function assertPrivacySafe(value, location = "root") {
  if (Array.isArray(value)) {
    value.forEach((item, index) => assertPrivacySafe(item, `${location}[${index}]`));
    return;
  }
  if (value === null || typeof value !== "object") return;
  for (const [key, nested] of Object.entries(value)) {
    if (forbiddenRepositoryFields.has(key)) {
      fail(`privacy-forbidden field ${key} at ${location}`);
    }
    assertPrivacySafe(nested, `${location}.${key}`);
  }
}

function verifyFrozen(document) {
  assertPrivacySafe(document);
  if (document.kind !== "owner-error-ranking-full"
      || document.formatVersion !== 1
      || document.statistics.totalEvents !== 12_252
      || document.statistics.nonFirstSelections !== 875
      || document.statistics.uniqueNonFirstReadings !== 632
      || document.comparison.cases !== document.statistics.uniqueNonFirstReadings
      || document.comparison.ownerEntriesUnchanged !== true
      || !/^[0-9a-f]{64}$/u.test(document.statistics.sourceEventsSHA256)
      || !/^[0-9a-f]{64}$/u.test(document.comparison.comparisonSHA256)
      || !Array.isArray(document.allReadings)
      || document.allReadings.length !== document.statistics.uniqueNonFirstReadings) {
    fail("owner full-error ranking corpus has an unsupported or drifted format");
  }
  const readings = new Set(document.allReadings.map(({ reading }) => reading));
  if (readings.size !== document.allReadings.length) {
    fail("owner full-error ranking contains duplicate readings");
  }
  for (let index = 1; index < document.allReadings.length; index += 1) {
    const previous = document.allReadings[index - 1];
    const current = document.allReadings[index];
    if (previous.count < current.count
        || (previous.count === current.count
          && previous.reading.localeCompare(current.reading, "en") > 0)) {
      fail("owner full-error ranking order is not deterministic");
    }
  }
  const attributionReadings = Object.values(document.attributionShares)
    .reduce((total, item) => total + item.readings, 0);
  const attributionIncidents = Object.values(document.attributionShares)
    .reduce((total, item) => total + item.incidents, 0);
  if (attributionReadings !== document.statistics.uniqueNonFirstReadings
      || attributionIncidents !== document.statistics.nonFirstSelections
      || document.allReadings.reduce((total, item) => total + item.count, 0)
        !== document.statistics.nonFirstSelections) {
    fail("owner full-error attribution does not cover the frozen source");
  }
}

async function verifyPrivateSource(eventsFile, document) {
  verifyFrozen(document);
  const source = await readEvents(eventsFile);
  const sourceSHA = checksum(source.bytes);
  // A documented re-serialization of the same frozen records is accepted (see the alias file).
  const aliases = JSON.parse(await readFile(
    path.join(path.dirname(outputPath), "owner-source-reserialization-v1.json"), "utf8",
  )).aliases;
  if (sourceSHA !== document.statistics.sourceEventsSHA256
      && sourceSHA !== aliases[document.statistics.sourceEventsSHA256]) {
    fail("owner full-error private source checksum changed");
  }
  const ranked = rankedNonFirstReadings(source.events);
  const selectedByReading = new Map();
  for (const event of source.events) {
    if (!event.selectedText) continue;
    const values = selectedByReading.get(event.pinyin) ?? [];
    values.push(event.selectedText);
    selectedByReading.set(event.pinyin, values);
  }
  if (ranked.length !== document.allReadings.length) {
    fail("owner full-error private source reading count changed");
  }
  ranked.forEach((item, index) => {
    const frozen = document.allReadings[index];
    if (item.reading !== frozen.reading
        || item.count !== frozen.count
        || JSON.stringify(transitionCounts(item.errorEvents))
          !== JSON.stringify(frozen.errorTransitions)
        || JSON.stringify(stableCounts(selectedByReading.get(item.reading) ?? []))
          !== JSON.stringify(frozen.selectionCounts)) {
      fail(`owner full-error private aggregate drift for reading rank ${index + 1}`);
    }
  });
}

async function generate(eventsFile, comparisonFile) {
  const [
    eventSource,
    comparisonBytes,
    exactIndex,
    probe,
    round1,
  ] = await Promise.all([
    readEvents(eventsFile),
    readFile(comparisonFile),
    readExactReadingIndex(),
    readFile(probePath, "utf8").then(JSON.parse),
    readFile(round1Path, "utf8").then(JSON.parse),
  ]);
  const comparison = JSON.parse(comparisonBytes);
  const ranked = rankedNonFirstReadings(eventSource.events);
  const probeReadings = probe.cases.map(({ query }) => query);
  const rankedReadings = ranked.map(({ reading }) => reading);
  if (JSON.stringify(probeReadings) !== JSON.stringify(rankedReadings)) {
    fail("owner full-error probe readings are stale");
  }
  if (comparison.cases !== ranked.length || comparison.ownerEntriesUnchanged !== true) {
    fail("clean/owner full comparison is incomplete or changed owner-copy entries");
  }

  const comparisons = new Map(comparison.comparisons.map((item) => [item.query, item]));
  const selectedByReading = new Map();
  for (const event of eventSource.events) {
    if (!event.selectedText) continue;
    const values = selectedByReading.get(event.pinyin) ?? [];
    values.push(event.selectedText);
    selectedByReading.set(event.pinyin, values);
  }
  const allReadings = ranked.map((rankedItem, index) => {
    const arm = comparisons.get(rankedItem.reading);
    if (!arm || !arm.clean?.[0] || !arm.owner?.[0]) {
      fail(`clean/owner comparison lacks reading ${rankedItem.reading}`);
    }
    const internal = {
      ...rankedItem,
      transitions: transitionCounts(rankedItem.errorEvents),
      desiredCounts: stableCounts(rankedItem.errorEvents.map(({ selectedText }) => selectedText)),
      selectionCounts: stableCounts(selectedByReading.get(rankedItem.reading) ?? []),
      cleanTop1: arm.clean[0],
      ownerTop1: arm.owner[0],
      cleanCandidates: arm.clean,
      ownerCandidates: arm.owner,
    };
    return {
      rank: index + 1,
      reading: internal.reading,
      count: internal.count,
      errorTransitions: internal.transitions,
      selectionCounts: internal.selectionCounts,
      cleanCandidates: internal.cleanCandidates,
      ownerCandidates: internal.ownerCandidates,
      ...classify(internal, exactIndex),
    };
  });

  const attributions = [...new Set(allReadings.map(({ attribution }) => attribution))].sort();
  const attributionShares = Object.fromEntries(attributions.map((attribution) => {
    const items = allReadings.filter((item) => item.attribution === attribution);
    const incidents = items.reduce((total, item) => total + item.count, 0);
    return [attribution, {
      readings: items.length,
      readingPercent: round(items.length / allReadings.length * 100),
      incidents,
      incidentPercent: round(incidents / 875 * 100),
    }];
  }));

  const document = {
    kind: "owner-error-ranking-full",
    formatVersion: 1,
    sourceKind: "owner-private-quality-events-evaluation-only",
    redactionState: "aggregate-only-no-owner-body-context-app-or-event-identity",
    classificationOrder: [
      "raw-echo-competition",
      "learned-pollution",
      "needs-context-bidirectional",
      "abbrev-competition",
      "needs-context-multiple-selections",
      "dictionary-gap",
      "full-pinyin-ranking",
      "ranking-drift",
    ],
    statistics: {
      totalEvents: eventSource.events.length,
      nonFirstSelections: ranked.reduce((total, item) => total + item.count, 0),
      uniqueNonFirstReadings: ranked.length,
      sourceEventsSHA256: checksum(eventSource.bytes),
    },
    comparison: {
      cases: comparison.cases,
      artifactChecksum: comparison.artifactChecksum,
      productCommit: comparison.productCommit,
      ownerEntryCount: comparison.ownerEntryCount,
      ownerEntriesUnchanged: comparison.ownerEntriesUnchanged,
      top1Differences: comparison.top1Differences,
      orderDifferences: comparison.orderDifferences,
      comparisonSHA256: checksum(comparisonBytes),
    },
    round1Top30AttributionShares: round1.attributionShares,
    attributionShares,
    allReadings,
  };
  verifyFrozen(document);
  return `${JSON.stringify(document, null, 2)}\n`;
}

if (process.argv.includes("--write-probe")) {
  const eventsFile = argument("--events");
  if (!eventsFile) fail("usage: --events FILE --write-probe");
  await writeProbe(eventsFile);
} else if (process.argv.includes("--verify-source")) {
  const eventsFile = argument("--events");
  if (!eventsFile) fail("usage: --events FILE --verify-source");
  const current = JSON.parse(await readFile(outputPath, "utf8"));
  await verifyPrivateSource(eventsFile, current);
  console.log(
    `owner_error_ranking_full_source=PASS incidents=${current.statistics.nonFirstSelections}`
      + ` readings=${current.statistics.uniqueNonFirstReadings} privacy=PASS`,
  );
} else if (process.argv.includes("--verify-frozen")) {
  const current = JSON.parse(await readFile(outputPath, "utf8"));
  verifyFrozen(current);
  console.log(
    `owner_error_ranking_full=PASS incidents=${current.statistics.nonFirstSelections}`
      + ` readings=${current.statistics.uniqueNonFirstReadings} privacy=PASS`
      + ` checksum=${checksum(await readFile(outputPath))}`,
  );
} else {
  const eventsFile = argument("--events");
  const comparisonFile = argument("--comparison");
  if (!eventsFile || !comparisonFile) {
    fail("usage: --events FILE --comparison FILE (--write|--check)");
  }
  const generated = await generate(eventsFile, comparisonFile);
  if (process.argv.includes("--check")) {
    if (await readFile(outputPath, "utf8") !== generated) {
      fail("owner full-error ranking corpus is stale");
    }
  } else if (process.argv.includes("--write")) {
    await writeFile(outputPath, generated, { mode: 0o644 });
  } else {
    fail("choose --write or --check");
  }
  console.log(
    `owner_error_ranking_full=PASS incidents=875 readings=632 privacy=PASS`
      + ` checksum=${checksum(generated)}`,
  );
}
