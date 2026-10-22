import { createHash } from "node:crypto";
import { readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(fileURLToPath(new URL("..", import.meta.url)));
const outputPath = path.join(root, "quality/corpus/owner-error-ranking-v1.json");
const probePath = path.join(root, "quality/corpus/owner-error-ranking-probe-v1.json");
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

function round(value) {
  return Number(value.toFixed(4));
}

function localDate(recordedAtMs) {
  return new Date(recordedAtMs + 8 * 60 * 60 * 1_000).toISOString().slice(0, 10);
}

function inclusiveCalendarDays(start, end) {
  const startMs = Date.parse(`${start}T00:00:00Z`);
  const endMs = Date.parse(`${end}T00:00:00Z`);
  return Math.floor((endMs - startMs) / 86_400_000) + 1;
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
  const requiredStrings = ["pinyin", "selectedText"];
  for (const field of requiredStrings) {
    if (typeof event[field] !== "string") fail(`event line ${lineNumber} lacks ${field}`);
  }
  if (!Array.isArray(event.candidates) || event.candidates.some((item) => typeof item !== "string")) {
    fail(`event line ${lineNumber} has invalid candidates`);
  }
  if (!Number.isInteger(event.selectedIndex) || event.selectedIndex < 0) {
    fail(`event line ${lineNumber} has invalid selectedIndex`);
  }
  if (!Number.isFinite(event.recordedAtMs)) {
    fail(`event line ${lineNumber} has invalid recordedAtMs`);
  }
}

async function readEvents(file) {
  const lines = (await readFile(file, "utf8")).split(/\r?\n/u).filter(Boolean);
  return lines.map((line, index) => {
    const event = JSON.parse(line);
    validateEvent(event, index + 1);
    return event;
  });
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
        // A lone consonant may exist as an interjection reading in the base
        // dictionary, but in a multi-syllable owner raw such as xiang+m or
        // na+m it is exactly the abbreviated syllable being diagnosed.
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
  const dictionaryGap = selectionCommentIsRawEcho(item.errorEvents, dominantWanted)
    && !exactIndex.exactTexts.has(dominantWanted);
  if (dictionaryGap) {
    return {
      attribution: "dictionary-gap",
      criterion: `“${dominantWanted}”仅以“英文·原样”回退出现，精确词典索引无该词；中文解码候选压在其前。`,
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
      criterion: `“${item.reading}”不能完整切成全拼音节，正确解“${dominantWanted}”在 clean 候选中但被同前缀展开压住。`,
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
      criterion: `正确解“${dominantWanted}”不在 clean 臂候选中。`,
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
  if (document.kind !== "owner-error-ranking" || document.formatVersion !== 1) {
    fail("owner-error ranking corpus has an unsupported format");
  }
  if (document.statistics.totalEvents !== 12_252
      || document.statistics.richSchemaEvents !== 1_338
      || document.statistics.inclusiveCalendarDays !== 9) {
    fail("owner-error ranking frozen source totals changed");
  }
  if (!Array.isArray(document.top30) || document.top30.length !== 30) {
    fail("owner-error ranking must contain exactly 30 readings");
  }
  const readings = new Set(document.top30.map(({ reading }) => reading));
  if (readings.size !== 30) fail("owner-error ranking contains duplicate readings");
  for (let index = 1; index < document.top30.length; index += 1) {
    const previous = document.top30[index - 1];
    const current = document.top30[index];
    if (previous.count < current.count
        || (previous.count === current.count
          && previous.reading.localeCompare(current.reading, "en") > 0)) {
      fail("owner-error ranking order is not deterministic");
    }
  }
  const attributed = Object.values(document.attributionShares)
    .reduce((total, item) => total + item.readings, 0);
  if (attributed !== 30) fail("attribution shares do not cover Top-30");
}

async function generate(eventsFile, comparisonFile) {
  const [events, comparison, exactIndex, probe] = await Promise.all([
    readEvents(eventsFile),
    readFile(comparisonFile, "utf8").then(JSON.parse),
    readExactReadingIndex(),
    readFile(probePath, "utf8").then(JSON.parse),
  ]);
  if (events.length === 0) fail("quality event source is empty");
  if (new Set(events.map(({ id }) => id)).size !== events.length) {
    fail("quality event source contains duplicate IDs");
  }

  const dates = [...new Set(events.map(({ recordedAtMs }) => localDate(recordedAtMs)))].sort();
  const startDate = dates[0];
  const endDate = dates.at(-1);
  const calendarDays = inclusiveCalendarDays(startDate, endDate);
  const rich = events.filter((event) =>
    Object.hasOwn(event, "outcome")
    && Object.hasOwn(event, "backspaces")
    && Object.hasOwn(event, "redoAfterCommit")
    && Object.hasOwn(event, "maxPage"));
  const tier = (metric, count, denominator) => ({
    metric,
    count,
    denominator,
    percent: round(count / denominator * 100),
    perCalendarDay: round(count / calendarDays),
  });
  const tiers = [
    tier("redo-after-commit", rich.filter(({ redoAfterCommit }) => redoAfterCommit).length, rich.length),
    tier("abandoned", rich.filter(({ outcome }) => outcome === "abandoned").length, rich.length),
    tier("backspace-rework", rich.filter(({ backspaces }) => backspaces > 0).length, rich.length),
    tier("non-first-selection", events.filter(({ selectedIndex }) => selectedIndex > 0).length, events.length),
  ];

  const comparisons = new Map(comparison.comparisons.map((item) => [item.query, item]));
  const nonFirst = events.filter(({ selectedIndex }) => selectedIndex > 0);
  const byReading = new Map();
  for (const event of nonFirst) {
    const values = byReading.get(event.pinyin) ?? [];
    values.push(event);
    byReading.set(event.pinyin, values);
  }
  const selectedByReading = new Map();
  for (const event of events) {
    if (!event.selectedText) continue;
    const values = selectedByReading.get(event.pinyin) ?? [];
    values.push(event.selectedText);
    selectedByReading.set(event.pinyin, values);
  }
  const ranked = [...byReading.entries()]
    .map(([reading, errorEvents]) => ({ reading, count: errorEvents.length, errorEvents }))
    .sort((left, right) => right.count - left.count
      || left.reading.localeCompare(right.reading, "en"))
    .slice(0, 30);

  const expectedProbeReadings = probe.cases.map(({ query }) => query);
  const actualProbeReadings = ranked.map(({ reading }) => reading);
  if (JSON.stringify(expectedProbeReadings) !== JSON.stringify(actualProbeReadings)) {
    fail("owner-error ranking probe readings are stale; regenerate the probe before A/B");
  }
  if (comparison.cases !== 30 || comparison.ownerEntriesUnchanged !== true) {
    fail("clean/owner comparison is incomplete or changed the owner-copy entries");
  }

  const top30 = ranked.map((rankedItem, index) => {
    const arm = comparisons.get(rankedItem.reading);
    if (!arm || !arm.clean?.[0] || !arm.owner?.[0]) {
      fail(`clean/owner comparison lacks reading ${rankedItem.reading}`);
    }
    const internal = {
      ...rankedItem,
      transitions: transitionCounts(rankedItem.errorEvents),
      errorTop1Counts: stableCounts(rankedItem.errorEvents.map((event) => event.candidates[0] ?? "")),
      desiredCounts: stableCounts(rankedItem.errorEvents.map(({ selectedText }) => selectedText)),
      selectionCounts: stableCounts(selectedByReading.get(rankedItem.reading) ?? []),
      cleanTop1: arm.clean[0],
      ownerTop1: arm.owner[0],
      cleanCandidates: arm.clean,
      ownerCandidates: arm.owner,
    };
    const classification = classify(internal, exactIndex);
    return {
      rank: index + 1,
      reading: internal.reading,
      count: internal.count,
      errorTransitions: internal.transitions,
      selectionCounts: internal.selectionCounts,
      cleanCandidates: internal.cleanCandidates,
      ownerCandidates: internal.ownerCandidates,
      ...classification,
    };
  });

  const attributionShares = Object.fromEntries([
    "learned-pollution",
    "needs-context",
    "abbrev-competition",
    "dictionary-gap",
  ].map((attribution) => {
    const items = top30.filter((item) => item.attribution === attribution);
    const incidents = items.reduce((total, item) => total + item.count, 0);
    return [attribution, {
      readings: items.length,
      readingPercent: round(items.length / top30.length * 100),
      incidents,
      incidentPercent: round(incidents
        / top30.reduce((total, item) => total + item.count, 0) * 100),
    }];
  }));

  const document = {
    kind: "owner-error-ranking",
    formatVersion: 1,
    statistics: {
      totalEvents: events.length,
      uniqueEvents: new Set(events.map(({ id }) => id)).size,
      richSchemaEvents: rich.length,
      startDate,
      endDate,
      inclusiveCalendarDays: calendarDays,
      activeRecordDays: dates.length,
      tiers,
    },
    top30,
    attributionShares,
  };
  verifyFrozen(document);
  return `${JSON.stringify(document, null, 2)}\n`;
}

if (process.argv.includes("--verify-frozen")) {
  const current = JSON.parse(await readFile(outputPath, "utf8"));
  verifyFrozen(current);
  const checksum = createHash("sha256").update(await readFile(outputPath)).digest("hex");
  console.log(`owner_error_ranking=PASS top30=30 privacy=PASS checksum=${checksum}`);
} else {
  const eventsFile = argument("--events");
  const comparisonFile = argument("--comparison");
  if (!eventsFile || !comparisonFile) {
    fail("usage: generate_owner_error_ranking.mjs --events FILE --comparison FILE (--write|--check)");
  }
  const generated = await generate(eventsFile, comparisonFile);
  if (process.argv.includes("--check")) {
    if (await readFile(outputPath, "utf8") !== generated) fail("owner-error ranking corpus is stale");
  } else if (process.argv.includes("--write")) {
    await writeFile(outputPath, generated, { mode: 0o644 });
  } else {
    fail("choose --write or --check");
  }
  const checksum = createHash("sha256").update(generated).digest("hex");
  console.log(`owner_error_ranking=PASS events=12252 rich=1338 top30=30 checksum=${checksum}`);
}
