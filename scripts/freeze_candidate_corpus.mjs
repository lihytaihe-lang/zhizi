import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(fileURLToPath(new URL("..", import.meta.url)));
const paths = Object.freeze({
  baseGroundTruth: "quality/corpus/base-500-ground-truth.txt",
  base: "quality/corpus/base-500.json",
  fuzzyGroundTruth: "quality/corpus/fuzzy-56-ground-truth.txt",
  fuzzy: "quality/corpus/fuzzy-56.json",
  holdout: "quality/corpus/generalization-smoke-20.json",
});
const lockedSha256 = Object.freeze({
  baseGroundTruth: "8cb632d158e312dfe14f6e752b84dd724bfa0ddfa2686a7a18bdb119f6b96a9c",
  base: "2439aa608f8aaa6e277ef65d9c9a2bd762669d2a2caeee45612d261c312e2225",
  fuzzyGroundTruth: "6035e610965382301db8776be522b4bd8e2c237b034ac3ac9cf89132df7f07bc",
  fuzzy: "d89c32a19d083b88e9bc366e3f11626035ef2aa8f8b64b27a24df1b1f9a0dfce",
  holdout: "1d7edf067a7ebe59705ff67a087052435a4dfd86d60dffb81ddd701b3861525a",
});

function sha256(contents) {
  return createHash("sha256").update(contents).digest("hex");
}

function assert(condition, message) {
  if (!condition) throw new Error(message);
}

function unique(values) {
  return new Set(values).size === values.length;
}

function parseBaseGroundTruth(contents) {
  return contents.split(/\r?\n/u)
    .filter((line) => line && !line.startsWith("#"))
    .map((line, index) => {
      const fields = line.split("|");
      assert(fields.length === 6, `base ground truth row ${index + 1} must have six fields`);
      const [id, category, query, allowed, mustPassTop1, boundaryKind] = fields;
      assert(mustPassTop1 === "true" || mustPassTop1 === "false", `invalid must-pass value for ${id}`);
      const result = {
        id,
        category,
        query,
        allowed: allowed.split(";"),
        mustPassTop1: mustPassTop1 === "true",
      };
      if (boundaryKind !== "-") result.boundaryKind = boundaryKind;
      return result;
    });
}

function parseFuzzyGroundTruth(contents) {
  return contents.split(/\r?\n/u)
    .filter((line) => line && !line.startsWith("#"))
    .map((line, index) => {
      const fields = line.split("|");
      assert(fields.length === 5, `fuzzy ground truth row ${index + 1} must have five fields`);
      const [id, group, canonicalQuery, fuzzyQuery, allowed] = fields;
      return { id, group, canonicalQuery, fuzzyQuery, allowed: allowed.split(";") };
    });
}

function stableJSON(value) {
  return JSON.stringify(value);
}

function validateAllowed(testCase) {
  assert(Array.isArray(testCase.allowed) && testCase.allowed.length > 0, `missing allowed answers for ${testCase.id}`);
  assert(unique(testCase.allowed), `duplicate allowed answer for ${testCase.id}`);
  assert(testCase.allowed.every((value) => /^[\u3400-\u9fff]+$/u.test(value)), `non-Han allowed answer for ${testCase.id}`);
}

const contents = {};
for (const [key, relativePath] of Object.entries(paths)) {
  contents[key] = await readFile(path.join(root, relativePath));
  const actual = sha256(contents[key]);
  assert(actual === lockedSha256[key], `${relativePath} checksum changed: ${actual}`);
}

const baseGroundTruth = parseBaseGroundTruth(contents.baseGroundTruth.toString("utf8"));
const base = JSON.parse(contents.base.toString("utf8"));
assert(base.formatVersion === 4, "base corpus formatVersion must be 4");
assert(base.groundTruth?.file === paths.baseGroundTruth, "base ground-truth path mismatch");
assert(base.groundTruth?.sha256 === lockedSha256.baseGroundTruth, "base ground-truth checksum binding mismatch");
assert(stableJSON(base.cases) === stableJSON(baseGroundTruth), "base JSON is not an exact projection of independent ground truth");
assert(base.cases.length === 500, "base corpus must contain 500 cases");
assert(unique(base.cases.map((item) => item.id)), "base corpus IDs must be unique");
assert(unique(base.cases.map((item) => item.query)), "base corpus queries must be unique");
assert(base.cases.every((item) => /^[a-z']+$/u.test(item.query)), "base query contains unsupported characters");
assert(base.cases.every((item) => (validateAllowed(item), true)), "base allowed validation failed");
const categoryCounts = Object.fromEntries(["single", "common-word", "short-sentence", "boundary"]
  .map((category) => [category, base.cases.filter((item) => item.category === category).length]));
assert(stableJSON(categoryCounts) === stableJSON({ single: 100, "common-word": 250, "short-sentence": 100, boundary: 50 }), "base category quotas changed");
assert(base.cases.filter((item) => item.mustPassTop1).length === 100, "must-pass top-1 set must contain 100 cases");
assert(base.cases.filter((item) => item.category === "single")
  .every((item) => item.allowed.every((value) => [...value].length === 1)), "single category contains a non-single answer");
assert(base.cases.filter((item) => item.category === "short-sentence")
  .every((item) => item.allowed.every((value) => [...value].length >= 2)), "short-sentence category contains an empty utterance");

const boundaryCounts = { abbreviation: 0, segmentation: 0, "long-pinyin": 0 };
for (const item of base.cases.filter((value) => value.category === "boundary")) {
  const [kind, reference] = item.boundaryKind?.split(":") ?? [];
  assert(Object.hasOwn(boundaryCounts, kind), `invalid boundary kind for ${item.id}`);
  boundaryCounts[kind] += 1;
  if (kind === "abbreviation") {
    assert(/^[a-z]+$/u.test(reference) && item.query.length < reference.length, `invalid abbreviation contract for ${item.id}`);
  } else if (kind === "segmentation") {
    assert(item.query.includes("'") && item.query.replaceAll("'", "") === reference, `invalid segmentation contract for ${item.id}`);
  } else {
    assert(item.query.length >= 16 && reference === "-", `invalid long-pinyin contract for ${item.id}`);
  }
}
assert(stableJSON(boundaryCounts) === stableJSON({ abbreviation: 20, segmentation: 15, "long-pinyin": 15 }), "boundary quotas changed");

const fuzzyGroundTruth = parseFuzzyGroundTruth(contents.fuzzyGroundTruth.toString("utf8"));
const fuzzy = JSON.parse(contents.fuzzy.toString("utf8"));
assert(fuzzy.formatVersion === 2, "fuzzy corpus formatVersion must be 2");
assert(fuzzy.groundTruth?.file === paths.fuzzyGroundTruth, "fuzzy ground-truth path mismatch");
assert(fuzzy.groundTruth?.sha256 === lockedSha256.fuzzyGroundTruth, "fuzzy ground-truth checksum binding mismatch");
assert(stableJSON(fuzzy.cases) === stableJSON(fuzzyGroundTruth), "fuzzy JSON is not an exact projection of independent ground truth");
assert(fuzzy.cases.length === 56, "fuzzy corpus must contain 56 cases");
assert(unique(fuzzy.cases.map((item) => item.id)), "fuzzy corpus IDs must be unique");
assert(fuzzy.cases.every((item) => (validateAllowed(item), true)), "fuzzy allowed validation failed");
const groups = ["zh-z", "ch-c", "sh-s", "n-l", "f-h", "an-ang", "en-eng", "in-ing"];
for (const group of groups) {
  assert(fuzzy.cases.filter((item) => item.group === group).length === 7, `fuzzy group ${group} must contain seven cases`);
}
assert(fuzzy.cases.every((item) => /^[a-z]+$/u.test(item.canonicalQuery)
  && /^[a-z]+$/u.test(item.fuzzyQuery)
  && item.canonicalQuery !== item.fuzzyQuery), "invalid fuzzy query pair");

const holdout = JSON.parse(contents.holdout.toString("utf8"));
assert(holdout.formatVersion === 1 && holdout.kind === "generalization-smoke", "holdout metadata changed");
assert(holdout.cases.length === 20, "holdout must contain 20 cases");
assert(unique(holdout.cases.map((item) => item.id)), "holdout IDs must be unique");
assert(unique(holdout.cases.map((item) => item.query)), "holdout queries must be unique");
const baseQueries = new Set(base.cases.map((item) => item.query));
const baseAnswers = new Set(base.cases.flatMap((item) => item.allowed));
for (const item of holdout.cases) {
  assert(item.category === "holdout" && item.mustPassTop1 === false, `invalid holdout metadata for ${item.id}`);
  assert(/^[a-z]+$/u.test(item.query), `invalid holdout query for ${item.id}`);
  validateAllowed(item);
  assert(!baseQueries.has(item.query), `holdout query overlaps base corpus for ${item.id}`);
  assert(item.allowed.every((answer) => !baseAnswers.has(answer)), `holdout answer overlaps base corpus for ${item.id}`);
}

for (const key of ["baseGroundTruth", "base", "fuzzyGroundTruth", "fuzzy", "holdout"]) {
  console.log(`${lockedSha256[key]}  ${paths[key]}`);
}
