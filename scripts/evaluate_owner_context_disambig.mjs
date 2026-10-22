#!/usr/bin/env node

import { createHash } from "node:crypto";
import { readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(fileURLToPath(new URL("..", import.meta.url)));
const eventsPath = argument("--events");
const tablePath = path.join(root, "third_party/context-disambig/disambig-table-v1.json");
const rankingPath = path.join(root, "quality/corpus/owner-error-ranking-v1.json");
const reportPath = path.join(
  root,
  "docs/current/evidence/context-disambig-active-owner-gate-v1.json",
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

function score(rule, precedingText, candidate) {
  const sense = rule?.senses.find(({ word }) => word === candidate);
  if (!sense) return null;
  let best = null;
  for (const trigger of sense.triggers) {
    if (precedingText.endsWith(trigger)) {
      const matchedBytes = Buffer.byteLength(trigger);
      best = best === null ? matchedBytes : Math.max(best, matchedBytes);
    }
  }
  return best;
}

function promotedTop1(rule, event) {
  return event.candidates
    .map((candidate, index) => ({
      candidate,
      index,
      score: score(rule, event.precedingText ?? "", candidate),
    }))
    .sort((left, right) => {
      if (left.score === right.score) return left.index - right.index;
      if (left.score === null) return 1;
      if (right.score === null) return -1;
      return right.score - left.score || left.index - right.index;
    })[0]?.candidate ?? "";
}

function assertPrivacySafe(value, location = "root") {
  if (Array.isArray(value)) {
    value.forEach((item, index) => assertPrivacySafe(item, `${location}[${index}]`));
    return;
  }
  if (value === null || typeof value !== "object") return;
  for (const [key, nested] of Object.entries(value)) {
    if (forbiddenReportFields.has(key)) fail(`privacy-forbidden report field ${key} at ${location}`);
    assertPrivacySafe(nested, `${location}.${key}`);
  }
}

function validateReport(report) {
  assertPrivacySafe(report);
  if (report.kind !== "owner-context-disambig-active"
      || report.formatVersion !== 1
      || report.sourceKind !== "owner-private-quality-events-evaluation-only"
      || report.redactionState !== "aggregate-only-no-owner-text-or-context"
      || report.denominator !== 137
      || report.readings !== 21
      || report.tableRules !== 22
      || !Number.isInteger(report.coveredReadings)
      || report.coveredReadings < 0
      || report.coveredReadings > report.readings
      || !Number.isInteger(report.contextAvailable)
      || report.contextAvailable < 0
      || report.contextUnavailable !== report.denominator - report.contextAvailable
      || report.maximumReachable !== report.contextAvailable
      || !Number.isInteger(report.prevented)
      || report.prevented < 0
      || report.prevented > report.denominator
      || report.preventedPercent !== Number((report.prevented / report.denominator * 100).toFixed(4))
      || !/^[0-9a-f]{64}$/u.test(report.sourceEventsSHA256)
      || !/^[0-9a-f]{64}$/u.test(report.rankingSHA256)
      || !/^[0-9a-f]{64}$/u.test(report.tableSHA256)
      || !Array.isArray(report.byReading)
      || report.byReading.length !== 21
      || report.byReading.reduce((sum, item) => sum + item.incidents, 0) !== 137
      || report.byReading.reduce((sum, item) => sum + item.prevented, 0) !== report.prevented) {
    fail("owner context disambiguation aggregate report is invalid");
  }
}

async function evaluate() {
  const [eventBytes, tableBytes, rankingBytes] = await Promise.all([
    readFile(eventsPath),
    readFile(tablePath),
    readFile(rankingPath),
  ]);
  const events = eventBytes.toString("utf8").split(/\r?\n/u).filter(Boolean).map((line, index) => {
    const event = JSON.parse(line);
    if (typeof event.pinyin !== "string"
        || typeof event.selectedText !== "string"
        || !Array.isArray(event.candidates)
        || event.candidates.some((candidate) => typeof candidate !== "string")
        || !Number.isInteger(event.selectedIndex)
        || (event.precedingText !== undefined && typeof event.precedingText !== "string")) {
      fail(`private quality event ${index + 1} has an unsupported schema`);
    }
    return event;
  });
  const table = JSON.parse(tableBytes);
  const ranking = JSON.parse(rankingBytes);
  const targetRows = ranking.top30.filter(({ attribution }) => attribution === "needs-context");
  if (targetRows.length !== 21
      || targetRows.reduce((sum, item) => sum + item.count, 0) !== 137) {
    fail("owner ranking no longer contains the frozen 21 readings / 137 incidents");
  }
  const rules = new Map(table.rules.map((rule) => [rule.pinyin, rule]));
  const targetReadings = new Set(targetRows.map(({ reading }) => reading));
  const relevantEvents = events.filter((event) =>
    event.selectedIndex > 0 && targetReadings.has(event.pinyin));
  if (relevantEvents.length !== 137) {
    fail(`private source no longer matches frozen needs-context denominator: ${relevantEvents.length}`);
  }

  const byReading = targetRows.map(({ reading, count: incidents }) => {
    const readingEvents = relevantEvents.filter((event) => event.pinyin === reading);
    if (readingEvents.length !== incidents) fail(`private source aggregate drift for ${reading}`);
    const rule = rules.get(reading);
    const prevented = readingEvents.filter((event) =>
      promotedTop1(rule, event) === event.selectedText).length;
    return { reading, incidents, prevented };
  });
  const prevented = byReading.reduce((sum, item) => sum + item.prevented, 0);
  const contextAvailable = relevantEvents.filter((event) =>
    typeof event.precedingText === "string" && event.precedingText.length > 0).length;
  const report = {
    kind: "owner-context-disambig-active",
    formatVersion: 1,
    sourceKind: "owner-private-quality-events-evaluation-only",
    redactionState: "aggregate-only-no-owner-text-or-context",
    sourceEventsSHA256: checksum(eventBytes),
    rankingSHA256: checksum(rankingBytes),
    tableSHA256: checksum(tableBytes),
    tableRules: table.rules.length,
    readings: byReading.length,
    coveredReadings: targetRows.filter(({ reading }) => rules.has(reading)).length,
    denominator: relevantEvents.length,
    contextAvailable,
    contextUnavailable: relevantEvents.length - contextAvailable,
    maximumReachable: contextAvailable,
    prevented,
    preventedPercent: Number((prevented / relevantEvents.length * 100).toFixed(4)),
    preventedOfContextAvailablePercent: Number((prevented / contextAvailable * 100).toFixed(4)),
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
  if (frozen !== generated) fail("owner context disambiguation aggregate report is stale");
} else {
  fail("choose --write or --check");
}
const report = JSON.parse(generated);
console.log(
  `owner_context_disambig_active_measurement=PASS prevented=${report.prevented}/${report.denominator} percent=${report.preventedPercent.toFixed(4)} covered_readings=${report.coveredReadings}/${report.readings} context_available=${report.contextAvailable}/${report.denominator} privacy=PASS`,
);
