import { createHash } from "node:crypto";
import { readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(fileURLToPath(new URL("..", import.meta.url)));
const legacyPath = path.join(root, "quality/corpus/short-input-learning-evidence-v1.json");
const rankingPath = path.join(root, "quality/corpus/owner-error-ranking-v1.json");
const outputPath = path.join(root, "quality/corpus/learning-cleanup-evidence-v2.json");
const check = process.argv.includes("--check");

const [legacy, ranking] = await Promise.all([
  readFile(legacyPath, "utf8").then(JSON.parse),
  readFile(rankingPath, "utf8").then(JSON.parse),
]);

const cases = new Map();
for (const item of legacy.cases) {
  // The old probe's single-letter rows prove arm displacement but contain no
  // historical owner-choice evidence. A single initial stands for many real
  // words, so it is evidence for ordering only, never enough for batch delete.
  const eligible = item.query.length > 1 && item.cleanTop1 !== item.observedOwnerTop1;
  cases.set(item.query, {
    query: item.query,
    cleanTop1: item.cleanTop1,
    observedOwnerTop1: item.observedOwnerTop1,
    attribution: eligible ? "learned-pollution" : null,
    batchCleanupEligible: eligible,
  });
}
for (const item of ranking.top30) {
  const rankingCase = {
    query: item.reading,
    cleanTop1: item.cleanCandidates[0],
    observedOwnerTop1: item.ownerCandidates[0],
    attribution: item.attribution,
    batchCleanupEligible: item.attribution === "learned-pollution",
  };
  // The current Top-30 evidence is authoritative for overlapping readings.
  // In particular b is abbreviation competition, not an automatic deletion
  // merely because its owner arm differs from the factory arm.
  cases.set(item.reading, rankingCase);
}

const ordered = [...cases.values()].sort((left, right) =>
  left.query.localeCompare(right.query, "en"));
if (ordered.length !== 49) {
  throw new Error(`learning cleanup evidence must contain 49 unique readings, got ${ordered.length}`);
}
for (const item of ordered) {
  if (item.batchCleanupEligible !== (item.attribution === "learned-pollution")) {
    throw new Error(`cleanup eligibility diverged from attribution for ${item.query}`);
  }
}

const generated = `${JSON.stringify({
  kind: "learning-cleanup-evidence",
  formatVersion: 2,
  sources: [
    "quality/corpus/short-input-learning-evidence-v1.json",
    "quality/corpus/owner-error-ranking-v1.json",
  ],
  cases: ordered,
}, null, 2)}\n`;

if (check) {
  if (await readFile(outputPath, "utf8") !== generated) {
    throw new Error("learning cleanup evidence v2 is stale");
  }
} else {
  await writeFile(outputPath, generated, { mode: 0o644 });
}

const eligible = ordered.filter(({ batchCleanupEligible }) => batchCleanupEligible).length;
const checksum = createHash("sha256").update(generated).digest("hex");
console.log(`learning_cleanup_evidence=PASS readings=${ordered.length} eligible=${eligible} checksum=${checksum}`);
