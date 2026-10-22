#!/usr/bin/env node

import { createHash } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const sourcePath = resolve(root, "native/resources/Rime/agent_exact_readings.tsv");
const outputPath = resolve(root, "quality/corpus/ascii-prefix-position-v1.json");
const checkOnly = process.argv.includes("--check");

function damerauLevenshteinOne(left, right) {
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
  let skipped = false;
  for (let shortIndex = 0, longIndex = 0; longIndex < longer.length; longIndex += 1) {
    if (shortIndex < shorter.length && shorter[shortIndex] === longer[longIndex]) {
      shortIndex += 1;
    } else if (skipped) {
      return false;
    } else {
      skipped = true;
    }
  }
  return true;
}

function syllablePrefixAbbreviation(raw, syllabified) {
  const syllables = syllabified.toLowerCase().split(/[\s']+/).filter(Boolean);
  if (syllables.length < 2 || raw.length < syllables.length) return false;
  function consumes(remainingRaw, remainingSyllables) {
    if (remainingSyllables.length === 0) return remainingRaw.length === 0;
    const [syllable, ...rest] = remainingSyllables;
    const maximum = Math.min(syllable.length, remainingRaw.length - rest.length);
    for (let length = 1; length <= maximum; length += 1) {
      if (syllable.startsWith(remainingRaw.slice(0, length))
        && consumes(remainingRaw.slice(length), rest)) return true;
    }
    return false;
  }
  return consumes(raw, syllables);
}

function plausible(raw, syllabified) {
  const compact = syllabified.toLowerCase().replace(/[\s']/g, "");
  return compact.startsWith(raw)
    || damerauLevenshteinOne(raw, compact)
    || syllablePrefixAbbreviation(raw, syllabified);
}

const readings = readFileSync(sourcePath, "utf8")
  .split("\n")
  .filter(Boolean)
  .flatMap((line) => {
    const [text, rawReadings] = line.split("\t");
    // The exact-reading index also contains ASCII product terms. Those are
    // competitors, not evidence that a Chinese candidate exists for a query.
    if (!/\p{Script=Han}/u.test(text)) return [];
    return rawReadings.split(",").map((reading) => ({ text, reading: reading.trim() }));
  });
const letters = "abcdefghijklmnopqrstuvwxyz".split("");
const queries = [
  ...letters,
  ...letters.flatMap((left) => letters.map((right) => `${left}${right}`)),
];
const cases = queries.map((query) => {
  // The frozen Rime spelling algebra has no top-level Chinese spelling that
  // begins with i, u, or v. (Its v derivations apply after j/q/x/y/n/l.) A
  // dictionary edit-distance match such as i -> yi is therefore not a
  // candidate the decoder can actually emit and must not become an oracle.
  const rimeCanProduceChinese = !["i", "u", "v"].includes(query[0]);
  // `sv` is the verifier's accepted real-librime example: Svelte may lead,
  // but 是/说 must remain on the first page. The reading index cannot express
  // that algebra-derived candidate, so preserve the observed contract here.
  const requiresChineseFirstPage = query === "sv"
    || (rimeCanProduceChinese && readings.some(({ reading }) => plausible(query, reading)));
  return {
    id: `${query.length === 1 ? "single" : "double"}-${query}`,
    category: requiresChineseFirstPage
      ? "exact-or-plausible-chinese" : "no-chinese-to-displace",
    query,
    allowed: [],
    mustPassTop1: false,
    requiresChineseFirstPage,
  };
});
const sourceChecksum = createHash("sha256").update(readFileSync(sourcePath)).digest("hex");
const output = `${JSON.stringify({
  kind: "ascii-prefix-position-regression",
  formatVersion: 1,
  authoringMethod: "Deterministic 26 single-letter plus 676 double-letter enumeration. The must-keep-Chinese bit is derived only from Han-script entries in agent_exact_readings.tsv with the production prefix, distance-one, and syllable-prefix rules, constrained by the frozen Rime spelling algebra (no top-level i/u/v spelling), plus the verifier-frozen sv example whose Chinese candidates come from Rime algebra. ASCII product-term entries are competitors and never evidence that a Chinese candidate exists.",
  source: "native/resources/Rime/agent_exact_readings.tsv",
  sourceChecksum,
  cases,
}, null, 2)}\n`;

if (checkOnly) {
  const current = readFileSync(outputPath, "utf8");
  if (current !== output) {
    console.error(`generated ASCII prefix probe is stale: ${outputPath}`);
    process.exit(1);
  }
} else {
  writeFileSync(outputPath, output);
}

const chineseRequired = cases.filter(({ requiresChineseFirstPage }) => requiresChineseFirstPage).length;
console.log(`ascii-prefix-position cases=${cases.length} chinese-required=${chineseRequired} no-chinese=${cases.length - chineseRequired} source-sha256=${sourceChecksum}`);
