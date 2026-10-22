import { execFileSync, spawnSync } from "node:child_process";
import { copyFileSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(fileURLToPath(new URL("..", import.meta.url)));
const output = path.resolve(process.argv[2] ?? path.join(os.tmpdir(), "agent-ime-fuzzy-56-report.json"));
const artifactChecksum = process.argv[3] ?? "pre-rc-unfrozen";
const bundle = path.resolve(process.argv[4] ?? path.join(root, "build/native/release/Agent Input Method.inputmethod"));
const rimeResources = path.join(bundle, "Contents/Resources/Rime");
const groups = ["zh-z", "ch-c", "sh-s", "n-l", "f-h", "an-ang", "en-eng", "in-ing"];
const coreLibraryDirectory = path.join(root, "native/core/target/release");
execFileSync("cargo", [
  "build", "--release", "--locked", "--offline", "--features", "librime",
  "--manifest-path", "native/core/Cargo.toml",
], { cwd: root, stdio: "inherit", env: { ...process.env, CARGO_NET_OFFLINE: "true" } });
const swiftScratch = mkdtempSync(path.join(os.tmpdir(), "agent-ime-fuzzy-swift-"));
const swiftEnvironment = { ...process.env, AIM_CORE_LIBRARY_DIR: coreLibraryDirectory };
const swiftArguments = [
  "build", "--package-path", "native/macos", "-c", "release",
  "--disable-automatic-resolution", "--scratch-path", swiftScratch,
];
execFileSync("swift", swiftArguments, { cwd: root, stdio: "inherit", env: swiftEnvironment });
const swiftBin = execFileSync("swift", [...swiftArguments, "--show-bin-path"], {
  cwd: root,
  encoding: "utf8",
  env: swiftEnvironment,
}).trim();
copyFileSync(
  path.join(root, ".build-deps/librime/1.16.1/lib/librime.1.dylib"),
  path.join(swiftBin, "librime.1.dylib"),
);

const aggregate = {
  artifactVersion: readFileSync(path.join(root, "VERSION"), "utf8").trim(),
  artifactChecksum,
  candidateDeduplicationRule: "unicode-nfkc-trimmed-text+lowercase-ascii-reading-v1",
  candidatesMissingReadingKey: 0,
  cases: 0,
  repetitions: 5,
  isolationCases: 0,
  attemptedSessions: 0,
  createdSessions: 0,
  executedSessions: 0,
  top5Hits: 0,
  stableCases: 0,
  isolationStableCases: 0,
  disabledTop5Leaks: 0,
  otherMaskTop5Leaks: 0,
  existingSessionReloadChecks: 0,
  existingSessionReloadFailures: 0,
  groups: {},
  failures: [],
};
for (const group of groups) {
  // Secure config writes require the Rime directory's parent to be owned by
  // the current user. A mkdtemp path used directly has /tmp as its parent and
  // correctly fails closed with status -106.
  const runtimeRoot = mkdtempSync(path.join(os.tmpdir(), `agent-ime-fuzzy-${group}-`));
  const userDirectory = path.join(runtimeRoot, "Rime");
  const reportPath = path.join(runtimeRoot, "report.json");
  const result = spawnSync(
    path.join(swiftBin, "AgentIMEFuzzyHarness"),
    [
      rimeResources,
      userDirectory,
      path.join(root, "quality/corpus/fuzzy-56.json"),
      reportPath,
      "5",
      group,
      artifactChecksum,
    ],
    { cwd: root, encoding: "utf8" },
  );
  if (result.status !== 0) {
    process.stderr.write(result.stdout);
    process.stderr.write(result.stderr);
    rmSync(runtimeRoot, { recursive: true, force: true });
    rmSync(swiftScratch, { recursive: true, force: true });
    process.exit(result.status ?? 1);
  }
  const report = JSON.parse(readFileSync(reportPath, "utf8"));
  if (report.candidateDeduplicationRule !== aggregate.candidateDeduplicationRule) {
    process.stderr.write(`candidate deduplication rule drifted for ${group}\n`);
    rmSync(runtimeRoot, { recursive: true, force: true });
    rmSync(swiftScratch, { recursive: true, force: true });
    process.exit(6);
  }
  aggregate.cases += report.cases;
  aggregate.isolationCases += report.isolationCases;
  aggregate.attemptedSessions += report.attemptedSessions;
  aggregate.createdSessions += report.createdSessions;
  aggregate.executedSessions += report.executedSessions;
  aggregate.candidatesMissingReadingKey += report.candidatesMissingReadingKey;
  aggregate.top5Hits += report.top5Hits;
  aggregate.stableCases += report.stableCases;
  aggregate.isolationStableCases += report.isolationStableCases;
  aggregate.disabledTop5Leaks += report.disabledTop5Leaks;
  aggregate.otherMaskTop5Leaks += report.otherMaskTop5Leaks;
  aggregate.existingSessionReloadChecks += report.existingSessionReloadChecks;
  aggregate.existingSessionReloadFailures += report.existingSessionReloadFailures;
  aggregate.groups[group] = report.groups[group] ?? 0;
  aggregate.failures.push(...report.failures);
  rmSync(runtimeRoot, { recursive: true, force: true });
}
aggregate.top5Rate = aggregate.top5Hits / aggregate.cases;
const serialized = `${JSON.stringify(aggregate, null, 2)}\n`;
writeFileSync(output, serialized);
process.stdout.write(serialized);
const passed = aggregate.cases === 56
  && aggregate.repetitions === 5
  && aggregate.top5Hits === aggregate.cases
  && aggregate.top5Rate >= 0.90
  && aggregate.stableCases === aggregate.cases
  && aggregate.isolationCases === 112
  && aggregate.isolationStableCases === aggregate.isolationCases
  && aggregate.disabledTop5Leaks === 0
  && aggregate.otherMaskTop5Leaks === 0
  && aggregate.attemptedSessions === 840
  && aggregate.createdSessions === aggregate.attemptedSessions
  && aggregate.executedSessions === aggregate.attemptedSessions
  && aggregate.candidatesMissingReadingKey === 0
  && aggregate.existingSessionReloadChecks === groups.length
  && aggregate.existingSessionReloadFailures === 0
  && aggregate.failures.length === 0
  && groups.every((group) => aggregate.groups[group] === 7);
rmSync(swiftScratch, { recursive: true, force: true });
process.exit(passed ? 0 : 6);
