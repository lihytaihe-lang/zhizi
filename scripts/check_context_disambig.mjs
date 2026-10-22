#!/usr/bin/env node

import { createHash } from "node:crypto";
import { readFileSync, statSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const directory = resolve(root, "third_party/context-disambig");
const lock = readLock(resolve(directory, "LOCKED_VERSION"));
const asset = resolve(directory, lock.get("asset") ?? "missing");
const provenancePath = resolve(directory, "provenance.tsv");
const targetsPath = resolve(root, "quality/mined/context-disambig-targets-v1.json");

function readLock(path) {
  const values = new Map();
  for (const line of readFileSync(path, "utf8").split(/\r?\n/u)) {
    if (line.length === 0 || line.startsWith("#")) continue;
    const separator = line.indexOf("=");
    if (separator <= 0) throw new Error(`invalid disambiguation lock line: ${line}`);
    values.set(line.slice(0, separator), line.slice(separator + 1));
  }
  return values;
}

function sha256(path) {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}

function requireValue(key, expected) {
  if (lock.get(key) !== expected) throw new Error(`disambiguation lock mismatch for ${key}`);
}

requireValue("asset", "disambig-table-v1.json");
requireValue("sha256", sha256(asset));
requireValue("license", "project-generated");
requireValue("generated", "2026-07-20");
requireValue("rules", "22");
requireValue("purpose", "INPUT-QUALITY Chunk Q4 context disambiguation (replaces failed C2 bigram)");
if (statSync(asset).size > 1024 * 1024) throw new Error("disambiguation table exceeds 1 MiB");

const table = JSON.parse(readFileSync(asset, "utf8"));
const targetDocument = JSON.parse(readFileSync(targetsPath, "utf8"));
if (!table || Object.keys(table).join(",") !== "rules" || !Array.isArray(table.rules)) {
  throw new Error("disambiguation table root is noncanonical");
}
if (table.rules.length !== 22) throw new Error("disambiguation rule count drift");
const targetWords = new Map(targetDocument.targets.map((item) => [
  item.pinyin,
  new Set(item.targets.map((target) => target.word)),
]));
const readings = new Set();
let senses = 0;
let triggers = 0;
for (const rule of table.rules) {
  if (Object.keys(rule).join(",") !== "pinyin,senses"
      || typeof rule.pinyin !== "string"
      || !/^[a-z]+$/u.test(rule.pinyin)
      || readings.has(rule.pinyin)
      || !Array.isArray(rule.senses)
      || rule.senses.length === 0) {
    throw new Error("invalid or duplicate disambiguation rule");
  }
  readings.add(rule.pinyin);
  const allowedWords = targetWords.get(rule.pinyin);
  if (!allowedWords) throw new Error(`disambiguation reading is outside frozen targets: ${rule.pinyin}`);
  const words = new Set();
  for (const sense of rule.senses) {
    if (Object.keys(sense).join(",") !== "word,triggers"
        || typeof sense.word !== "string"
        || !allowedWords.has(sense.word)
        || words.has(sense.word)
        || !Array.isArray(sense.triggers)
        || sense.triggers.length === 0) {
      throw new Error(`invalid sense for ${rule.pinyin}`);
    }
    words.add(sense.word);
    const uniqueTriggers = new Set(sense.triggers);
    if (uniqueTriggers.size !== sense.triggers.length
        || sense.triggers.some((trigger) => typeof trigger !== "string"
          || trigger.length === 0
          || /[\u0000-\u001f\u007f]/u.test(trigger))) {
      throw new Error(`invalid trigger for ${rule.pinyin}/${sense.word}`);
    }
    senses += 1;
    triggers += sense.triggers.length;
  }
}

const provenanceLines = readFileSync(provenancePath, "utf8").trimEnd().split(/\r?\n/u);
if (provenanceLines.length !== 2
    || provenanceLines[0] !== "asset\tlicense\tgenerator\tsha256\tgenerated"
    || provenanceLines[1] !== `disambig-table-v1.json\tproject-generated\tclaude-subagent-blind\t${lock.get("sha256")}\t2026-07-20`) {
  throw new Error("disambiguation provenance does not match the frozen asset");
}

process.stdout.write(
  `context_disambig=PASS rules=${table.rules.length} senses=${senses} triggers=${triggers} bytes=${statSync(asset).size} sha256=${lock.get("sha256")}\n`,
);
