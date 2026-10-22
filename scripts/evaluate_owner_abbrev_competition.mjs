#!/usr/bin/env node

import { createHash } from "node:crypto";
import { readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(fileURLToPath(new URL("..", import.meta.url)));
const eventsPath = argument("--events");
const rankingPath = path.join(root, "quality/corpus/owner-error-ranking-v1.json");
const factoryPrioritiesPath = path.join(
  root,
  "native/resources/Rime/agent_factory_reading_priorities.tsv",
);
const mixerSourcePath = path.join(root, "native/core/agent-ime-core/src/mixer.rs");
const reportPath = path.join(
  root,
  "docs/current/evidence/abbrev-competition-round3-owner-gate-v1.json",
);
const forbiddenReportFields = new Set([
  "id",
  "recordedAtMs",
  "precedingText",
  "appHash",
  "selectedText",
  "candidates",
  "activeProject",
  "activePacks",
]);
if (!eventsPath) fail("missing required --events path");

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

function canonicalReading(value) {
  const syllables = value.trim().toLowerCase().split(/[\s']+/gu).filter(Boolean);
  return syllables.length > 0 && syllables.every((syllable) => /^[a-z]+$/u.test(syllable))
    ? syllables.join("") : null;
}

function parseFactoryPriorities(bytes) {
  const byText = new Map();
  for (const line of bytes.toString("utf8").split(/\r?\n/gu)) {
    if (!line) continue;
    const [text, reading, weightText] = line.split("\t", 3);
    const weight = Number.parseInt(weightText, 10);
    if (!text || !reading || !Number.isSafeInteger(weight) || weight < 0) {
      fail("factory reading priority index is invalid");
    }
    const values = byText.get(text) ?? [];
    values.push({ reading, weight });
    byText.set(text, values);
  }
  return byText;
}

function consumeExactPrefixes(raw, rawStart, candidateReading) {
  const syllables = candidateReading.trim().toLowerCase()
    .split(/[\s']+/gu).filter(Boolean);
  if (syllables.length === 0) return [];
  let positions = new Set([rawStart]);
  for (let index = 0; index < syllables.length; index += 1) {
    const syllable = syllables[index];
    const remainingMinimum = syllables.length - index - 1;
    const next = new Set();
    for (const start of positions) {
      const maximum = Math.min(
        syllable.length,
        Math.max(0, raw.length - start - remainingMinimum),
      );
      for (let length = 1; length <= maximum; length += 1) {
        const end = start + length;
        if (syllable.startsWith(raw.slice(start, end))) next.add(end);
      }
    }
    positions = next;
    if (positions.size === 0) return [];
  }
  return [...positions].map((rawEnd) => ({ rawEnd, syllableCount: syllables.length }));
}

function phrasePrefixMatchPriority(raw, text, factoryPriorities) {
  const direct = (factoryPriorities.get(text) ?? [])
    .flatMap(({ reading, weight }) =>
      consumeExactPrefixes(raw, 0, reading)
        .filter(({ rawEnd }) => rawEnd === raw.length)
        .map(({ syllableCount }) => ({ syllableCount, priority: weight })))
    .sort((left, right) =>
      right.priority - left.priority || right.syllableCount - left.syllableCount)[0];
  if (direct) return direct;

  const characters = [...text];
  if (characters.length === 0 || raw.length < characters.length) return null;
  const states = Array.from({ length: characters.length + 1 }, () => new Map());
  states[0].set("0:0:0", {
    rawEnd: 0,
    syllableCount: 0,
    wordCount: 0,
    priority: Number.MAX_SAFE_INTEGER,
  });
  for (let start = 0; start < characters.length; start += 1) {
    if (states[start].size === 0) continue;
    for (let end = start + 1; end <= characters.length; end += 1) {
      const word = characters.slice(start, end).join("");
      for (const factoryReading of factoryPriorities.get(word) ?? []) {
        for (const state of states[start].values()) {
          if (state.wordCount >= 2) continue;
          for (const consumed of consumeExactPrefixes(raw, state.rawEnd, factoryReading.reading)) {
            const syllableCount = state.syllableCount + consumed.syllableCount;
            const wordCount = state.wordCount + 1;
            const priority = Math.min(state.priority, factoryReading.weight);
            const key = `${consumed.rawEnd}:${syllableCount}:${wordCount}`;
            const existing = states[end].get(key);
            if (!existing || priority > existing.priority) {
              states[end].set(key, {
                rawEnd: consumed.rawEnd,
                syllableCount,
                wordCount,
                priority,
              });
            }
          }
        }
      }
    }
  }
  return [...states.at(-1).values()]
    .filter(({ rawEnd }) => rawEnd === raw.length)
    .sort((left, right) =>
      right.priority - left.priority || right.syllableCount - left.syllableCount)[0] ?? null;
}

function promotedTop1(event, factoryPriorities) {
  const raw = canonicalReading(event.pinyin);
  if (!raw) return event.candidates[0] ?? "";
  const coherentNearbyTop1Priority = Math.max(
    0,
    ...(factoryPriorities.get(event.candidates[0]) ?? []).map(({ reading, weight }) => {
      const exactPrefixAbbreviation = consumeExactPrefixes(raw, 0, reading)
        .some(({ rawEnd }) => rawEnd === raw.length);
      if (exactPrefixAbbreviation) return 0;
      const compact = canonicalReading(reading);
      return compact?.startsWith(raw) || damerauLevenshteinOne(raw, compact) ? weight : 0;
    }),
  );
  let bestIndex = null;
  let bestPriority = null;
  event.candidates.forEach((candidate, index) => {
    const evidence = phrasePrefixMatchPriority(
      raw,
      candidate,
      factoryPriorities,
    );
    if (evidence === null || evidence.syllableCount < 2) return;
    const priority = evidence.priority;
    if (bestIndex === null || priority > bestPriority) {
      bestIndex = index;
      bestPriority = priority;
    }
  });
  if ((bestPriority ?? 0) <= coherentNearbyTop1Priority) return event.candidates[0] ?? "";
  return event.candidates[bestIndex ?? 0] ?? "";
}

function damerauLevenshteinOne(left, right) {
  if (!left || !right) return false;
  if (left === right) return false;
  if (Math.abs(left.length - right.length) > 1) return false;
  if (left.length === right.length) {
    const differences = [];
    for (let index = 0; index < left.length; index += 1) {
      if (left[index] !== right[index]) differences.push(index);
    }
    if (differences.length === 1) return true;
    return differences.length === 2
      && differences[1] === differences[0] + 1
      && left[differences[0]] === right[differences[1]]
      && left[differences[1]] === right[differences[0]];
  }
  const [shorter, longer] = left.length < right.length ? [left, right] : [right, left];
  let shortIndex = 0;
  let longIndex = 0;
  let skipped = false;
  while (shortIndex < shorter.length && longIndex < longer.length) {
    if (shorter[shortIndex] === longer[longIndex]) {
      shortIndex += 1;
      longIndex += 1;
    } else if (!skipped) {
      skipped = true;
      longIndex += 1;
    } else {
      return false;
    }
  }
  return true;
}

function assertPrivacySafe(value, location = "root") {
  if (Array.isArray(value)) {
    value.forEach((item, index) => assertPrivacySafe(item, `${location}[${index}]`));
    return;
  }
  if (value === null || typeof value !== "object") return;
  for (const [key, nested] of Object.entries(value)) {
    if (forbiddenReportFields.has(key)) {
      fail(`privacy-forbidden report field ${key} at ${location}`);
    }
    assertPrivacySafe(nested, `${location}.${key}`);
  }
}

function validateReport(report) {
  assertPrivacySafe(report);
  if (report.kind !== "owner-abbrev-competition-round3"
      || report.formatVersion !== 1
      || report.mechanism
        !== "existing-candidate-exact-prefix-two-word-factory-bottleneck-v1"
      || report.sourceKind !== "owner-private-quality-events-evaluation-only"
      || report.redactionState !== "aggregate-only-no-owner-text-or-context"
      || typeof report.productRankingActive !== "boolean"
      || report.factoryPrioritiesRole !== "measurement-only-rejected-round3-counterfactual"
      || report.denominator !== 37
      || report.readings !== 7
      || !Number.isInteger(report.prevented)
      || report.prevented < 0
      || report.prevented > report.denominator
      || report.preventedPercent !== Number((report.prevented / report.denominator * 100).toFixed(4))
      || !/^[0-9a-f]{64}$/u.test(report.sourceEventsSHA256)
      || !/^[0-9a-f]{64}$/u.test(report.rankingSHA256)
      || !/^[0-9a-f]{64}$/u.test(report.factoryPrioritiesSHA256)
      || !/^[0-9a-f]{64}$/u.test(report.runtimeSourceSHA256)
      || !Array.isArray(report.byReading)
      || report.byReading.length !== 7
      || report.byReading.reduce((sum, item) => sum + item.incidents, 0) !== 37
      || report.byReading.reduce((sum, item) => sum + item.prevented, 0)
        !== report.prevented) {
    fail("owner abbreviation competition aggregate report is invalid");
  }
}

async function evaluate() {
  const [eventBytes, rankingBytes, factoryPrioritiesBytes, mixerSourceBytes] =
    await Promise.all([
      readFile(eventsPath),
      readFile(rankingPath),
      readFile(factoryPrioritiesPath),
      readFile(mixerSourcePath),
    ]);
  const events = eventBytes.toString("utf8").split(/\r?\n/gu).filter(Boolean)
    .map((line, index) => {
      const event = JSON.parse(line);
      if (typeof event.pinyin !== "string"
          || typeof event.selectedText !== "string"
          || !Array.isArray(event.candidates)
          || event.candidates.some((candidate) => typeof candidate !== "string")
          || !Number.isInteger(event.selectedIndex)) {
        fail(`private quality event ${index + 1} has an unsupported schema`);
      }
      return event;
    });
  const ranking = JSON.parse(rankingBytes);
  const factoryPriorities = parseFactoryPriorities(factoryPrioritiesBytes);
  const productRankingActive =
    /promote_factory_supported_abbreviation_to_top\(\s*&base\.raw_input,/u
      .test(mixerSourceBytes.toString("utf8"));
  const targetRows = ranking.top30.filter(
    ({ attribution }) => attribution === "abbrev-competition",
  );
  if (targetRows.length !== 7
      || targetRows.reduce((sum, item) => sum + item.count, 0) !== 37) {
    fail("owner ranking no longer contains the frozen 7 readings / 37 incidents");
  }
  const targetReadings = new Set(targetRows.map(({ reading }) => reading));
  const relevantEvents = events.filter(
    (event) => event.selectedIndex > 0 && targetReadings.has(event.pinyin),
  );
  if (relevantEvents.length !== 37) {
    fail(`private source no longer matches frozen abbreviation denominator: ${relevantEvents.length}`);
  }

  const byReading = targetRows.map(({ reading, count: incidents }) => {
    const readingEvents = relevantEvents.filter((event) => event.pinyin === reading);
    if (readingEvents.length !== incidents) fail(`private source aggregate drift for ${reading}`);
    const prevented = readingEvents.filter(
      (event) => {
        const productTop1 = productRankingActive
          ? promotedTop1(event, factoryPriorities)
          : (event.candidates[0] ?? "");
        return productTop1 === event.selectedText;
      },
    ).length;
    return { reading, incidents, prevented };
  });
  const prevented = byReading.reduce((sum, item) => sum + item.prevented, 0);
  const report = {
    kind: "owner-abbrev-competition-round3",
    formatVersion: 1,
    mechanism: "existing-candidate-exact-prefix-two-word-factory-bottleneck-v1",
    sourceKind: "owner-private-quality-events-evaluation-only",
    redactionState: "aggregate-only-no-owner-text-or-context",
    sourceEventsSHA256: checksum(eventBytes),
    rankingSHA256: checksum(rankingBytes),
    factoryPrioritiesSHA256: checksum(factoryPrioritiesBytes),
    factoryPrioritiesRole: "measurement-only-rejected-round3-counterfactual",
    runtimeSourceSHA256: checksum(mixerSourceBytes),
    productRankingActive,
    readings: byReading.length,
    denominator: relevantEvents.length,
    prevented,
    preventedPercent: Number((prevented / relevantEvents.length * 100).toFixed(4)),
    byReading,
  };
  validateReport(report);
  return `${JSON.stringify(report, null, 2)}\n`;
}

const generated = await evaluate();
if (process.argv.includes("--write")) {
  await writeFile(reportPath, generated, { mode: 0o644 });
} else if (process.argv.includes("--check")) {
  const frozen = await readFile(reportPath, "utf8");
  if (frozen !== generated) fail("owner abbreviation competition aggregate report is stale");
} else {
  fail("choose --write or --check");
}
const report = JSON.parse(generated);
console.log(
  `owner_abbrev_competition_measurement=PASS prevented=${report.prevented}/${report.denominator} `
    + `percent=${report.preventedPercent.toFixed(4)} privacy=PASS`,
);
