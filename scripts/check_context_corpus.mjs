#!/usr/bin/env node

import { createHash } from "node:crypto";
import { readFileSync, statSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const corpusDirectory = resolve(root, "third_party/context-corpus");
const corpusLock = readLock(resolve(corpusDirectory, "LOCKED_VERSION"));
const derivedLock = readLock(resolve(root, "native/resources/ContextBigram.LOCKED_VERSION"));

function readLock(path) {
  const values = new Map();
  for (const line of readFileSync(path, "utf8").split(/\r?\n/u)) {
    if (line.length === 0 || line.startsWith("#")) continue;
    const separator = line.indexOf("=");
    if (separator <= 0) throw new Error(`invalid context lock line: ${line}`);
    values.set(line.slice(0, separator), line.slice(separator + 1));
  }
  return values;
}

function sha256(path) {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}

function requireValue(values, key, expected) {
  if (values.get(key) !== expected) throw new Error(`context lock mismatch for ${key}`);
}

const source = resolve(corpusDirectory, corpusLock.get("asset") ?? "missing");
const provenance = resolve(corpusDirectory, "provenance.tsv");
const model = resolve(root, derivedLock.get("derived_model") ?? "missing");
const builder = resolve(root, derivedLock.get("builder") ?? "missing");

requireValue(corpusLock, "asset", "context-corpus-v1.txt");
requireValue(corpusLock, "sha256", sha256(source));
requireValue(corpusLock, "license", "project-generated");
requireValue(derivedLock, "schema_version", "1");
requireValue(derivedLock, "training_input", "third_party/context-corpus/context-corpus-v1.txt");
requireValue(derivedLock, "training_input_sha256", sha256(source));
requireValue(derivedLock, "provenance", "third_party/context-corpus/provenance.tsv");
requireValue(derivedLock, "provenance_sha256", sha256(provenance));
requireValue(derivedLock, "segmenter", "Intl.Segmenter:zh-CN:word");
requireValue(derivedLock, "segmenter_icu", process.versions.icu);
requireValue(derivedLock, "derived_model_sha256", sha256(model));
requireValue(derivedLock, "derived_model_bytes", String(statSync(model).size));
requireValue(derivedLock, "ci_corpus_regeneration", "forbidden");
requireValue(derivedLock, "evaluation_input", "forbidden");
if (statSync(model).size > 20 * 1024 * 1024) throw new Error("context model exceeds 20 MiB");

const checked = spawnSync(process.execPath, [builder, "--check", "--segment-zh", source, model], {
  cwd: root,
  encoding: "utf8",
});
if (checked.status !== 0) {
  process.stderr.write(checked.stderr);
  process.exit(checked.status ?? 2);
}
const match = /entries=([0-9]+) bytes=([0-9]+) sha256=([0-9a-f]{64}) segmenter=zh-CN icu=([^\s]+)/u.exec(checked.stdout);
if (!match) throw new Error("context builder check output is invalid");
requireValue(derivedLock, "derived_model_entries", match[1]);
requireValue(derivedLock, "derived_model_bytes", match[2]);
requireValue(derivedLock, "derived_model_sha256", match[3]);
requireValue(derivedLock, "segmenter_icu", match[4]);
process.stdout.write(
  `context_corpus=PASS entries=${match[1]} bytes=${match[2]} sha256=${match[3]} source_sha256=${corpusLock.get("sha256")}\n`,
);
