#!/usr/bin/env node

import { createHash } from "node:crypto";
import { readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(fileURLToPath(new URL("..", import.meta.url)));
const eventsPath = argument("--events");
const factoryPrioritiesPath = path.join(
  root,
  "native/resources/Rime/agent_factory_reading_priorities.tsv",
);
const englishTermsPath = path.join(root, "native/resources/Rime/agent_english.dict.yaml");
const techTermsPath = path.join(root, "native/resources/Rime/agent_tech_terms.dict.yaml");
const projectTermsPath = path.join(root, "native/resources/Rime/agent_project_terms.dict.yaml");
const mixerSourcePath = path.join(root, "native/core/agent-ime-core/src/mixer.rs");
const reportPath = path.join(
  root,
  "docs/current/evidence/abbrev-competition-round4-full-impact-v1.json",
);
const forbiddenReportFields = new Set([
  "id",
  "recordedAtMs",
  "precedingText",
  "appHash",
  "selectedText",
  "candidates",
  "candidateComments",
  "selectedCandidateComment",
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

function sha256Text(value) {
  return createHash("sha256").update(value, "utf8").digest("hex");
}

function canonicalReading(value) {
  const compact = value.trim().toLowerCase().replace(/\s+/gu, "");
  return compact.length > 0 && /^[a-z']+$/u.test(compact) ? compact : null;
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

function parsePinyinSyllables(mixerSource) {
  const match = mixerSource.match(/const PINYIN_SYLLABLES: &str = "\n([\s\S]*?)\n";/u);
  if (!match) fail("cannot locate the runtime pinyin syllable inventory");
  return new Set(match[1].trim().split(/\s+/u));
}

function parseRegisteredReadings(bytes) {
  const readings = new Set();
  for (const line of bytes.toString("utf8").split(/\r?\n/gu)) {
    if (!line || line.startsWith("#") || line.startsWith("-") || line.startsWith(".")) continue;
    const columns = line.split("\t");
    if (columns.length >= 2 && columns[1].trim()) {
      readings.add(columns[1].trim().toLowerCase());
    }
  }
  return readings;
}

function validPinyinSpelling(reading, syllables) {
  if (!reading) return false;
  if (reading.includes("'")) {
    return reading.split("'").every((part) => validPinyinSpelling(part, syllables));
  }
  const reachable = new Array(reading.length + 1).fill(false);
  reachable[0] = true;
  for (let start = 0; start < reading.length; start += 1) {
    if (!reachable[start]) continue;
    for (let end = start + 1; end <= Math.min(reading.length, start + 6); end += 1) {
      if (syllables.has(reading.slice(start, end))) reachable[end] = true;
    }
  }
  return reachable[reading.length];
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

function damerauLevenshteinOne(left, right) {
  if (!left || !right || left === right || Math.abs(left.length - right.length) > 1) return false;
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

function promotedTop1(
  event,
  projectTermsEnabled,
  factoryPriorities,
  pinyinSyllables,
  registeredReadings,
  projectReadings,
) {
  const original = event.candidates[0] ?? "";
  if (event.candidates.length < 2) return original;
  const raw = canonicalReading(event.pinyin);
  if (!raw || validPinyinSpelling(raw, pinyinSyllables)) return original;
  if (registeredReadings.has(raw) || (projectTermsEnabled && projectReadings.has(raw))) {
    return original;
  }

  const coherentNearbyTop1Priority = Math.max(
    0,
    ...(factoryPriorities.get(original) ?? []).map(({ reading, weight }) => {
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
    const evidence = phrasePrefixMatchPriority(raw, candidate, factoryPriorities);
    if (evidence === null || evidence.syllableCount < 2) return;
    if (bestIndex === null || evidence.priority > bestPriority) {
      bestIndex = index;
      bestPriority = evidence.priority;
    }
  });
  if ((bestPriority ?? 0) <= coherentNearbyTop1Priority) return original;
  return event.candidates[bestIndex ?? 0] ?? original;
}

function stableByReading(events, predicate) {
  const counts = new Map();
  for (const event of events) {
    if (!predicate(event)) continue;
    counts.set(event.pinyin, (counts.get(event.pinyin) ?? 0) + 1);
  }
  return [...counts.entries()]
    .map(([reading, incidents]) => ({ reading, incidents }))
    .sort((left, right) => right.incidents - left.incidents
      || left.reading.localeCompare(right.reading, "en"));
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
  if (report.kind !== "owner-abbrev-competition-round4-full-impact"
      || report.formatVersion !== 1
      || report.mechanism
        !== "existing-candidate-exact-prefix-two-word-factory-bottleneck-v1"
      || report.sourceKind !== "owner-private-quality-events-evaluation-only"
      || report.redactionState !== "aggregate-only-no-owner-body-context-app-or-event-identity"
      || typeof report.productRankingActive !== "boolean"
      || report.factoryPrioritiesRole !== "measurement-only-rejected-round3-counterfactual"
      || report.scannedEvents !== 12_252
      || report.nonFirstSelections !== 875
      || report.originalTop1Correct !== 11_331
      || report.neitherNonFirstNorOriginalTop1Correct !== 46
      || report.benefit.denominator !== 875
      || report.cost.denominator !== 11_331
      || report.benefit.prevented < 0
      || report.benefit.prevented > report.benefit.denominator
      || report.cost.regressed < 0
      || report.cost.regressed > report.cost.denominator
      || report.benefit.preventedPercent
        !== Number((report.benefit.prevented / report.benefit.denominator * 100).toFixed(4))
      || report.cost.regressedPercent
        !== Number((report.cost.regressed / report.cost.denominator * 100).toFixed(4))
      || report.netPreventedMinusRegressed
        !== report.benefit.prevented - report.cost.regressed
      || !Number.isInteger(report.codeBiasSensitiveEvents)
      || report.codeBiasSensitiveEvents < 0
      || !Number.isInteger(report.codeBiasSensitiveInKnownCodeApps)
      || report.codeBiasSensitiveInKnownCodeApps < 0
      || report.codeBiasSensitiveInDefaultNonCodeApps
        !== report.codeBiasSensitiveEvents - report.codeBiasSensitiveInKnownCodeApps
      || report.historicalAppClassOverrideRecorded !== false
      || !Number.isInteger(report.benefit.lowerBound)
      || !Number.isInteger(report.benefit.upperBound)
      || report.benefit.lowerBound > report.benefit.prevented
      || report.benefit.upperBound < report.benefit.prevented
      || !Number.isInteger(report.cost.lowerBound)
      || !Number.isInteger(report.cost.upperBound)
      || report.cost.lowerBound > report.cost.regressed
      || report.cost.upperBound < report.cost.regressed
      || !/^[0-9a-f]{64}$/u.test(report.sourceEventsSHA256)
      || !/^[0-9a-f]{64}$/u.test(report.factoryPrioritiesSHA256)
      || !/^[0-9a-f]{64}$/u.test(report.runtimeSourceSHA256)
      || !Array.isArray(report.benefit.byReading)
      || !Array.isArray(report.cost.byReading)
      || report.benefit.byReading.reduce((sum, item) => sum + item.incidents, 0)
        !== report.benefit.prevented
      || report.cost.byReading.reduce((sum, item) => sum + item.incidents, 0)
        !== report.cost.regressed) {
    fail("owner full abbreviation impact aggregate report is invalid");
  }
}

async function evaluate() {
  const [
    eventBytes,
    factoryPrioritiesBytes,
    englishBytes,
    techBytes,
    projectBytes,
    mixerSourceBytes,
  ] = await Promise.all([
    readFile(eventsPath),
    readFile(factoryPrioritiesPath),
    readFile(englishTermsPath),
    readFile(techTermsPath),
    readFile(projectTermsPath),
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
  const factoryPriorities = parseFactoryPriorities(factoryPrioritiesBytes);
  const mixerSource = mixerSourceBytes.toString("utf8");
  const productRankingActive =
    /promote_factory_supported_abbreviation_to_top\(\s*&base\.raw_input,/u.test(mixerSource);
  const pinyinSyllables = parsePinyinSyllables(mixerSource);
  const registeredReadings = new Set([
    ...parseRegisteredReadings(englishBytes),
    ...parseRegisteredReadings(techBytes),
  ]);
  const projectReadings = parseRegisteredReadings(projectBytes);
  const knownCodeAppHashes = new Set([
    "com.apple.dt.Xcode",
    "com.apple.Terminal",
    "com.anthropic.claudefordesktop",
    "com.googlecode.iterm2",
    "com.mitchellh.ghostty",
    "com.microsoft.VSCode",
    "com.openai.codex",
    "com.todesktop.230313mzl4w4u92",
  ].map(sha256Text));
  const enriched = events.map((event) => {
    const originalTop1 = event.candidates[0] ?? "";
    const withoutCodeBias = productRankingActive
      ? promotedTop1(
        event,
        false,
        factoryPriorities,
        pinyinSyllables,
        registeredReadings,
        projectReadings,
      )
      : originalTop1;
    const withCodeBias = productRankingActive
      ? promotedTop1(
        event,
        true,
        factoryPriorities,
        pinyinSyllables,
        registeredReadings,
        projectReadings,
      )
      : originalTop1;
    const codeBiasEnabled = knownCodeAppHashes.has(event.appHash);
    return {
      event,
      promotedTop1: codeBiasEnabled ? withCodeBias : withoutCodeBias,
      promotedTop1WithoutCodeBias: withoutCodeBias,
      promotedTop1WithCodeBias: withCodeBias,
      codeBiasSensitive: withoutCodeBias !== withCodeBias,
      codeBiasSensitiveInKnownCodeApp:
        codeBiasEnabled && withoutCodeBias !== withCodeBias,
    };
  });
  const codeBiasSensitiveEvents = enriched.filter(({ codeBiasSensitive }) => codeBiasSensitive).length;
  const codeBiasSensitiveInKnownCodeApps = enriched.filter(
    ({ codeBiasSensitiveInKnownCodeApp }) => codeBiasSensitiveInKnownCodeApp,
  ).length;
  const nonFirst = enriched.filter(({ event }) => event.selectedIndex > 0);
  const originalCorrect = enriched.filter(
    ({ event }) => (event.candidates[0] ?? "") === event.selectedText,
  );
  const preventedEvents = nonFirst.filter(
    ({ event, promotedTop1: promoted }) => promoted === event.selectedText,
  );
  const regressedEvents = originalCorrect.filter(
    ({ event, promotedTop1: promoted }) => promoted !== event.selectedText,
  );
  const benefitBounds = nonFirst.reduce((bounds, item) => {
    const without = item.promotedTop1WithoutCodeBias === item.event.selectedText ? 1 : 0;
    const withBias = item.promotedTop1WithCodeBias === item.event.selectedText ? 1 : 0;
    bounds.minimum += Math.min(without, withBias);
    bounds.maximum += Math.max(without, withBias);
    return bounds;
  }, { minimum: 0, maximum: 0 });
  const costBounds = originalCorrect.reduce((bounds, item) => {
    const without = item.promotedTop1WithoutCodeBias !== item.event.selectedText ? 1 : 0;
    const withBias = item.promotedTop1WithCodeBias !== item.event.selectedText ? 1 : 0;
    bounds.minimum += Math.min(without, withBias);
    bounds.maximum += Math.max(without, withBias);
    return bounds;
  }, { minimum: 0, maximum: 0 });
  const changedWrongToOtherWrong = nonFirst.filter(
    ({ event, promotedTop1: promoted }) =>
      promoted !== (event.candidates[0] ?? "") && promoted !== event.selectedText,
  ).length;
  const report = {
    kind: "owner-abbrev-competition-round4-full-impact",
    formatVersion: 1,
    mechanism: "existing-candidate-exact-prefix-two-word-factory-bottleneck-v1",
    sourceKind: "owner-private-quality-events-evaluation-only",
    redactionState: "aggregate-only-no-owner-body-context-app-or-event-identity",
    scannedEvents: events.length,
    nonFirstSelections: nonFirst.length,
    originalTop1Correct: originalCorrect.length,
    neitherNonFirstNorOriginalTop1Correct:
      events.length - nonFirst.length - originalCorrect.length,
    sourceEventsSHA256: checksum(eventBytes),
    factoryPrioritiesSHA256: checksum(factoryPrioritiesBytes),
    factoryPrioritiesRole: "measurement-only-rejected-round3-counterfactual",
    runtimeSourceSHA256: checksum(mixerSourceBytes),
    productRankingActive,
    registeredEnglishTermsSHA256: checksum(englishBytes),
    registeredTechTermsSHA256: checksum(techBytes),
    registeredProjectTermsSHA256: checksum(projectBytes),
    codeBiasSensitiveEvents,
    codeBiasSensitiveInKnownCodeApps,
    codeBiasSensitiveInDefaultNonCodeApps:
      codeBiasSensitiveEvents - codeBiasSensitiveInKnownCodeApps,
    historicalAppClassOverrideRecorded: false,
    benefit: {
      denominator: nonFirst.length,
      prevented: preventedEvents.length,
      preventedPercent: Number((preventedEvents.length / nonFirst.length * 100).toFixed(4)),
      lowerBound: benefitBounds.minimum,
      upperBound: benefitBounds.maximum,
      changedWrongToOtherWrong,
      byReading: stableByReading(
        preventedEvents.map(({ event }) => event),
        () => true,
      ),
    },
    cost: {
      denominator: originalCorrect.length,
      regressed: regressedEvents.length,
      regressedPercent: Number((regressedEvents.length / originalCorrect.length * 100).toFixed(4)),
      lowerBound: costBounds.minimum,
      upperBound: costBounds.maximum,
      byReading: stableByReading(
        regressedEvents.map(({ event }) => event),
        () => true,
      ),
    },
    netPreventedMinusRegressed: preventedEvents.length - regressedEvents.length,
  };
  validateReport(report);
  return `${JSON.stringify(report, null, 2)}\n`;
}

const generated = await evaluate();
if (process.argv.includes("--write")) {
  await writeFile(reportPath, generated, { mode: 0o644 });
} else if (process.argv.includes("--check")) {
  const frozen = await readFile(reportPath, "utf8");
  if (frozen !== generated) fail("owner full abbreviation impact aggregate report is stale");
} else {
  fail("choose --write or --check");
}
const report = JSON.parse(generated);
console.log(
  `owner_abbrev_full_impact=PASS benefit=${report.benefit.prevented}/${report.benefit.denominator}`
    + ` regressions=${report.cost.regressed}/${report.cost.denominator}`
    + ` scanned=${report.scannedEvents} net=${report.netPreventedMinusRegressed}`
    + " privacy=PASS",
);
