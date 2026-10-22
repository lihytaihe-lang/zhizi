#!/usr/bin/env node

import { createHash } from "node:crypto";
import { readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(fileURLToPath(new URL("..", import.meta.url)));
const candidateReportPath = argument("--candidate-report");
const baselineReportPath = argument("--baseline-report");
const tablePath = path.join(root, "third_party/context-disambig/disambig-table-v1.json");
const ownerGatePath = path.join(
  root,
  "docs/current/evidence/context-disambig-round2-owner-gate-v1.json",
);
const outputPath = path.join(
  root,
  "docs/current/evidence/context-disambig-round2-requalification-v1.json",
);

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

function percent(hits, cases) {
  return Number((hits / cases * 100).toFixed(4));
}

if (!candidateReportPath || !baselineReportPath) {
  fail("usage: summarize_context_disambig_requalification.mjs --candidate-report FILE --baseline-report FILE (--write|--check)");
}

const [candidateBytes, baselineBytes, tableBytes, ownerGateBytes] = await Promise.all([
  readFile(candidateReportPath),
  readFile(baselineReportPath),
  readFile(tablePath),
  readFile(ownerGatePath),
]);
const candidate = JSON.parse(candidateBytes);
const baseline = JSON.parse(baselineBytes);
const ownerGate = JSON.parse(ownerGateBytes);
if (candidate.schemaVersion !== 2
    || candidate.sampleCount !== 1150
    || candidate.repetitions !== 5
    || candidate.goldenChecksum !== "4fe103014ae4cb73b60ecd171bbb96284a84292dc0f225d61ed99ebafe759ee3"
    || candidate.candidateChecksum !== "4f53cda18c2baa0c0354bb5f9a3ecbe5ed12ab4d8e11ba873c2f11161202b945"
    || candidate.userDBChecksum !== "fa611270876d59ea0826bc6616ea3550abc031ec29f7b04ee84b99b0c00bcd10"
    || candidate.sourceKind !== "owner-quality-recording-v1"
    || !candidate.privacyRule?.includes("no pinyin or candidate text")
    || baseline.schemaVersion !== 1
    || baseline.sampleCount !== 1150
    || baseline.candidateTop1Hits !== 1054
    || baseline.candidateTop5Hits !== 1141
    || baseline.firstChoiceRegressions !== 22
    || ownerGate.denominator !== 137
    || ownerGate.tableSHA256 !== checksum(tableBytes)) {
  fail("private replay or owner gate does not match the frozen ROUND-2 inputs");
}

const minimumExclusiveHits = 1058;
const passed = candidate.candidateTop1Hits > minimumExclusiveHits;
const report = {
  kind: "context-disambig-round2-requalification",
  formatVersion: 1,
  privacy: "aggregate-only; private replay differences and owner text remain outside Git",
  sourceBindings: {
    tableSHA256: checksum(tableBytes),
    goldenSHA256: candidate.goldenChecksum,
    candidateSnapshotSHA256: candidate.candidateChecksum,
    immutableUserDBSHA256: candidate.userDBChecksum,
    ownerGateSHA256: checksum(ownerGateBytes),
  },
  gate: {
    rule: "candidateTop1Hits must be greater than 1058/1150 (strictly greater than 92%)",
    denominator: 1150,
    minimumExclusiveHits,
  },
  priorTable: {
    top1Hits: baseline.candidateTop1Hits,
    top1Percent: percent(baseline.candidateTop1Hits, baseline.sampleCount),
    top5Hits: baseline.candidateTop5Hits,
    firstChoiceRegressions: baseline.firstChoiceRegressions,
  },
  expandedTable: {
    top1Hits: candidate.candidateTop1Hits,
    top1Percent: percent(candidate.candidateTop1Hits, candidate.sampleCount),
    top5Hits: candidate.candidateTop5Hits,
    top5Percent: percent(candidate.candidateTop5Hits, candidate.sampleCount),
    firstChoiceRegressions: candidate.firstChoiceRegressions,
    jitterCases: candidate.jitterCases,
    latencySamples: candidate.latencySamples,
    latencyP95Milliseconds: candidate.latencyP95Milliseconds,
    latencyP99Milliseconds: candidate.latencyP99Milliseconds,
    deltaTop1VersusPriorTable: candidate.candidateTop1Hits - baseline.candidateTop1Hits,
    deltaTop5VersusPriorTable: candidate.candidateTop5Hits - baseline.candidateTop5Hits,
  },
  ownerTop30NeedsContext: {
    prevented: ownerGate.prevented,
    denominator: ownerGate.denominator,
    preventedPercent: ownerGate.preventedPercent,
    contextAvailable: ownerGate.contextAvailable,
    contextUnavailable: ownerGate.contextUnavailable,
  },
  decision: passed ? "PASS" : "BLOCKED",
  governanceRecommendation: passed
    ? "qualified for continued inclusion; installation and release remain separately unauthorized"
    : "do not ship the expanded table; remove the still-unqualified context table from the next product build unless the owner explicitly overrides this failed quality gate",
  measurementNote: "The candidate replay used the unchanged 6449df7 executable with only ContextDisambig.json replaced in a private temporary bundle; the build tool could not rebuild inside the managed sandbox because nested sandbox-exec is forbidden.",
};
const generated = `${JSON.stringify(report, null, 2)}\n`;
if (process.argv.includes("--write")) {
  await writeFile(outputPath, generated, { mode: 0o644 });
} else if (process.argv.includes("--check")) {
  if (await readFile(outputPath, "utf8") !== generated) {
    fail("context disambiguation requalification aggregate is stale");
  }
} else {
  fail("choose --write or --check");
}
console.log(
  `context_disambig_requalification=${report.decision} top1=${candidate.candidateTop1Hits}/1150 percent=${report.expandedTable.top1Percent.toFixed(4)} owner_prevented=${ownerGate.prevented}/137 p95_ms=${candidate.latencyP95Milliseconds} p99_ms=${candidate.latencyP99Milliseconds}`,
);
