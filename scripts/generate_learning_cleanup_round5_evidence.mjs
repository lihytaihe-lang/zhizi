#!/usr/bin/env node

import { createHash } from "node:crypto";
import { readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(fileURLToPath(new URL("..", import.meta.url)));
const legacyPath = path.join(root, "quality/corpus/learning-cleanup-evidence-v2.json");
const rankingPath = path.join(root, "quality/corpus/owner-error-ranking-full-v1.json");
const probePath = path.join(root, "quality/corpus/learning-cleanup-round5-probe-v1.json");
const outputPath = path.join(root, "quality/corpus/learning-cleanup-evidence-v3.json");
const forbiddenFields = new Set([
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

function assertPrivacySafe(value, location = "root") {
  if (Array.isArray(value)) {
    value.forEach((item, index) => assertPrivacySafe(item, `${location}[${index}]`));
    return;
  }
  if (value === null || typeof value !== "object") return;
  for (const [key, nested] of Object.entries(value)) {
    if (forbiddenFields.has(key)) fail(`privacy-forbidden field ${key} at ${location}`);
    assertPrivacySafe(nested, `${location}.${key}`);
  }
}

function fullCohort(ranking) {
  const rows = ranking.allReadings.filter(
    ({ attribution }) => attribution === "learned-pollution",
  );
  const incidents = rows.reduce((sum, item) => sum + item.count, 0);
  if (rows.length !== 197 || incidents !== 210) {
    fail(`ROUND-5 cohort drifted from 197 readings / 210 incidents: ${rows.length}/${incidents}`);
  }
  return rows;
}

function buildProbe(ranking) {
  const cohort = fullCohort(ranking);
  return {
    kind: "short-input-learning-probe",
    formatVersion: 1,
    cases: cohort.map((item) => ({
      id: `round5-learning-${String(item.rank).padStart(3, "0")}`,
      category: "learned-pollution-round5",
      query: item.reading,
      allowed: [],
      mustPassTop1: false,
    })),
  };
}

function validateFrozen(document) {
  assertPrivacySafe(document);
  const cohortCases = document.cases.filter(
    ({ evaluationScope }) => evaluationScope === "round5-full-learned-pollution-cohort",
  );
  const eligible = cohortCases.filter(({ batchCleanupEligible }) => batchCleanupEligible);
  if (document.kind !== "learning-cleanup-evidence"
      || document.formatVersion !== 3
      || document.round5Cohort.sourceReadings !== 197
      || document.round5Cohort.sourceIncidents !== 210
      || document.round5Cohort.eligibleReadings !== eligible.length
      || document.round5Cohort.eligibleIncidents
        !== eligible.reduce((sum, item) => sum + item.sourceIncidents, 0)
      || document.round5Cohort.cleanOwnerComparisonCases !== 197
      || document.round5Cohort.ownerEntriesUnchanged !== true
      || !/^[0-9a-f]{64}$/u.test(document.round5Cohort.comparisonSHA256)
      || !/^[0-9a-f]{64}$/u.test(document.round5Cohort.artifactChecksum)
      || cohortCases.length !== 197
      || new Set(document.cases.map(({ query }) => query)).size !== document.cases.length) {
    fail("ROUND-5 learning cleanup evidence is invalid");
  }
  for (const item of cohortCases) {
    const eligibleByEvidence = item.cleanTop1 !== item.observedOwnerTop1
      && item.ownerTop1SelectedCount === 0;
    if (item.attribution !== "learned-pollution"
        || item.batchCleanupEligible !== eligibleByEvidence
        || !Number.isInteger(item.sourceIncidents)
        || item.sourceIncidents < 1
        || !Number.isInteger(item.ownerTop1SelectedCount)
        || item.ownerTop1SelectedCount < 0) {
      fail(`ROUND-5 cleanup criterion drift for ${item.query}`);
    }
  }
}

async function generate(comparisonFile) {
  const [legacy, ranking, comparisonBytes] = await Promise.all([
    readFile(legacyPath, "utf8").then(JSON.parse),
    readFile(rankingPath, "utf8").then(JSON.parse),
    readFile(comparisonFile),
  ]);
  const comparison = JSON.parse(comparisonBytes);
  const cohort = fullCohort(ranking);
  if (comparison.kind !== "short-input-learning-probe"
      || comparison.cases !== cohort.length
      || comparison.ownerEntriesUnchanged !== true
      || !Array.isArray(comparison.comparisons)
      || comparison.comparisons.length !== cohort.length) {
    fail("ROUND-5 clean/owner comparison is incomplete or mutated the owner copy");
  }
  const byReading = new Map(comparison.comparisons.map((item) => [item.query, item]));
  if (byReading.size !== cohort.length) fail("ROUND-5 comparison contains duplicate readings");

  const cases = new Map(legacy.cases.map((item) => [item.query, {
    ...item,
    sourceIncidents: 0,
    ownerTop1SelectedCount: 0,
    evaluationScope: "round1-legacy-noncohort",
    batchCleanupEligible: false,
  }]));
  for (const item of cohort) {
    const arms = byReading.get(item.reading);
    const cleanTop1 = arms?.clean?.[0] ?? "";
    const observedOwnerTop1 = arms?.owner?.[0] ?? "";
    if (!cleanTop1 || !observedOwnerTop1) {
      fail(`ROUND-5 comparison lacks a top-one candidate for ${item.reading}`);
    }
    const ownerTop1SelectedCount = item.selectionCounts
      .find(({ text }) => text === observedOwnerTop1)?.count ?? 0;
    cases.set(item.reading, {
      query: item.reading,
      cleanTop1,
      observedOwnerTop1,
      attribution: "learned-pollution",
      batchCleanupEligible:
        cleanTop1 !== observedOwnerTop1 && ownerTop1SelectedCount === 0,
      sourceIncidents: item.count,
      ownerTop1SelectedCount,
      evaluationScope: "round5-full-learned-pollution-cohort",
    });
  }
  const ordered = [...cases.values()].sort((left, right) =>
    left.query.localeCompare(right.query, "en"));
  const eligible = ordered.filter(
    ({ evaluationScope, batchCleanupEligible }) =>
      evaluationScope === "round5-full-learned-pollution-cohort"
        && batchCleanupEligible,
  );
  const document = {
    kind: "learning-cleanup-evidence",
    formatVersion: 3,
    sources: [
      "quality/corpus/learning-cleanup-evidence-v2.json",
      "quality/corpus/owner-error-ranking-full-v1.json",
      "private clean/owner replay on the ROUND-5 rollback product",
    ],
    round5Cohort: {
      sourceReadings: cohort.length,
      sourceIncidents: cohort.reduce((sum, item) => sum + item.count, 0),
      cleanOwnerComparisonCases: comparison.cases,
      eligibleReadings: eligible.length,
      eligibleIncidents: eligible.reduce((sum, item) => sum + item.sourceIncidents, 0),
      ownerEntryCount: comparison.ownerEntryCount,
      ownerEntriesUnchanged: comparison.ownerEntriesUnchanged,
      top1Differences: comparison.top1Differences,
      orderDifferences: comparison.orderDifferences,
      artifactChecksum: comparison.artifactChecksum,
      productCommit: comparison.productCommit,
      comparisonSHA256: checksum(comparisonBytes),
    },
    cases: ordered,
  };
  validateFrozen(document);
  return `${JSON.stringify(document, null, 2)}\n`;
}

if (process.argv.includes("--write-probe")) {
  const ranking = JSON.parse(await readFile(rankingPath, "utf8"));
  const generated = `${JSON.stringify(buildProbe(ranking), null, 2)}\n`;
  await writeFile(probePath, generated, { mode: 0o644 });
  console.log(`learning_cleanup_round5_probe=WRITTEN readings=197 incidents=210 checksum=${checksum(generated)}`);
} else if (process.argv.includes("--verify-frozen")) {
  const currentBytes = await readFile(outputPath);
  const current = JSON.parse(currentBytes);
  validateFrozen(current);
  console.log(
    `learning_cleanup_round5_evidence=PASS cohort=${current.round5Cohort.sourceIncidents}`
      + ` eligible_readings=${current.round5Cohort.eligibleReadings}`
      + ` eligible_incidents=${current.round5Cohort.eligibleIncidents}`
      + ` privacy=PASS checksum=${checksum(currentBytes)}`,
  );
} else {
  const comparisonFile = argument("--comparison");
  if (!comparisonFile) fail("usage: --comparison FILE (--write|--check)");
  const generated = await generate(comparisonFile);
  if (process.argv.includes("--write")) {
    await writeFile(outputPath, generated, { mode: 0o644 });
  } else if (process.argv.includes("--check")) {
    if (await readFile(outputPath, "utf8") !== generated) {
      fail("ROUND-5 learning cleanup evidence is stale");
    }
  } else {
    fail("choose --write or --check");
  }
  const report = JSON.parse(generated);
  console.log(
    `learning_cleanup_round5_evidence=PASS cohort=${report.round5Cohort.sourceIncidents}`
      + ` eligible_readings=${report.round5Cohort.eligibleReadings}`
      + ` eligible_incidents=${report.round5Cohort.eligibleIncidents}`
      + ` privacy=PASS checksum=${checksum(generated)}`,
  );
}
