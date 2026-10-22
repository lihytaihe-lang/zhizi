#!/usr/bin/env node

import { createHash } from "node:crypto";
import {
  chmodSync,
  existsSync,
  lstatSync,
  mkdirSync,
  openSync,
  readFileSync,
  writeFileSync,
  closeSync,
} from "node:fs";
import { basename, join, resolve } from "node:path";

const MAX_EVENTS_BYTES = 64 * 1024 * 1024;
const MAX_GOLDEN_BYTES = 32 * 1024 * 1024;
const SOURCE_KIND = "owner-quality-recording-context-c2-v1";
const BATCH_ID = "context-c2";

function fail(message) {
  process.stderr.write(`freeze context eval: ${message}\n`);
  process.exit(2);
}

function stable(value) {
  if (Array.isArray(value)) return `[${value.map(stable).join(",")}]`;
  if (value !== null && typeof value === "object") {
    return `{${Object.keys(value)
      .sort()
      .map((key) => `${JSON.stringify(key)}:${stable(value[key])}`)
      .join(",")}}`;
  }
  return JSON.stringify(value);
}

function sha256(value) {
  return createHash("sha256").update(value).digest("hex");
}

function requireString(event, key) {
  if (typeof event[key] !== "string" || event[key].length === 0) {
    fail(`quality event has invalid ${key}`);
  }
}

function requireEvent(event) {
  for (const key of ["id", "pinyin", "selectedText", "appHash", "precedingText"]) {
    requireString(event, key);
  }
  if (!Number.isSafeInteger(event.recordedAtMs) || event.recordedAtMs < 0) {
    fail("quality event has invalid recordedAtMs");
  }
  if (!Array.isArray(event.candidates) || !event.candidates.every((item) => typeof item === "string")) {
    fail("quality event has invalid candidates");
  }
  if (!Number.isSafeInteger(event.selectedIndex) || event.selectedIndex < 0) {
    fail("quality event has invalid selectedIndex");
  }
}

function exclusivePrivateWrite(path, bytes) {
  const descriptor = openSync(path, "wx", 0o600);
  try {
    writeFileSync(descriptor, bytes);
  } finally {
    closeSync(descriptor);
  }
  chmodSync(path, 0o600);
}

const [, , eventsArgument, outputArgument, frozenAtArgument] = process.argv;
if (!eventsArgument || !outputArgument || !frozenAtArgument) {
  fail("usage: <events.jsonl> <private-output-directory> <frozen-at-ms>");
}
const frozenAtMs = Number(frozenAtArgument);
if (!Number.isSafeInteger(frozenAtMs) || frozenAtMs <= 0) {
  fail("frozen-at-ms must be a positive integer");
}

const eventsPath = resolve(eventsArgument);
const outputDirectory = resolve(outputArgument);
const activePath = join(outputDirectory, "active-context-c2.json");
const inputStat = lstatSync(eventsPath);
if (!inputStat.isFile() || inputStat.isSymbolicLink() || inputStat.size > MAX_EVENTS_BYTES) {
  fail("events input must be a regular file no larger than 64 MiB");
}
mkdirSync(outputDirectory, { recursive: true, mode: 0o700 });
chmodSync(outputDirectory, 0o700);

if (existsSync(activePath)) {
  const activeStat = lstatSync(activePath);
  if (!activeStat.isFile() || activeStat.isSymbolicLink()) fail("active pointer is not a regular file");
  const active = JSON.parse(readFileSync(activePath, "utf8"));
  const goldenName = basename(active.fileName ?? "");
  if (!/^golden-context-c2-[0-9a-f]{64}\.json$/.test(goldenName)) fail("active pointer is invalid");
  const goldenBytes = readFileSync(join(outputDirectory, goldenName));
  if (sha256(goldenBytes) !== active.checksum) fail("active golden checksum mismatch");
  process.stdout.write(
    `context_eval_freeze=UNCHANGED samples=${active.sampleCount} checksum=${active.checksum} source_events_checksum=${active.sourceEventsChecksum}\n`,
  );
  process.exit(0);
}

const lines = readFileSync(eventsPath, "utf8").split("\n").filter((line) => line.length > 0);
const events = lines
  .map((line, index) => {
    try {
      return JSON.parse(line);
    } catch {
      fail(`invalid JSON at line ${index + 1}`);
    }
  })
  .filter((event) => typeof event.precedingText === "string" && event.precedingText.length > 0);
events.forEach(requireEvent);
events.sort((left, right) => left.recordedAtMs - right.recordedAtMs || left.id.localeCompare(right.id));
const ids = new Set();
for (const event of events) {
  if (ids.has(event.id)) fail("duplicate quality event id");
  ids.add(event.id);
}

const cases = events.map((event) => ({
  id: sha256(stable(event)),
  recordedAtMs: event.recordedAtMs,
  pinyin: event.pinyin,
  baselineCandidates: event.candidates,
  selectedIndex: event.selectedIndex,
  selectedText: event.selectedText,
  appHash: event.appHash,
  precedingText: event.precedingText,
  fuzzyMask: event.fuzzyMask ?? 0,
  typoCorrectionEnabled: event.typoCorrectionEnabled ?? false,
  activeProject: event.activeProject ?? null,
  activePacks: event.activePacks ?? [],
  secondaryEnabled: event.secondaryEnabled ?? true,
}));
const sourceEventsChecksum = sha256(stable(events));
const golden = {
  schemaVersion: 1,
  sourceKind: SOURCE_KIND,
  batchId: BATCH_ID,
  frozenAtMs,
  sourceEventsChecksum,
  cases,
};
const goldenBytes = Buffer.from(`${JSON.stringify(golden, null, 2)}\n`);
if (goldenBytes.length > MAX_GOLDEN_BYTES) fail("frozen evaluation set exceeds 32 MiB");
const checksum = sha256(goldenBytes);
const fileName = `golden-context-c2-${checksum}.json`;
exclusivePrivateWrite(join(outputDirectory, fileName), goldenBytes);
const active = {
  schemaVersion: 1,
  sourceKind: SOURCE_KIND,
  batchId: BATCH_ID,
  frozenAtMs,
  sourceEventsChecksum,
  checksum,
  fileName,
  sampleCount: cases.length,
};
exclusivePrivateWrite(activePath, Buffer.from(`${JSON.stringify(active, null, 2)}\n`));
process.stdout.write(
  `context_eval_freeze=CREATED samples=${cases.length} checksum=${checksum} source_events_checksum=${sourceEventsChecksum}\n`,
);
