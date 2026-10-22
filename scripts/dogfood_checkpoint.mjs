import {
  accessSync,
  chmodSync,
  constants as fsConstants,
  existsSync,
  lstatSync,
  linkSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  realpathSync,
  renameSync,
  rmdirSync,
  unlinkSync,
  writeFileSync,
} from "node:fs";
import { spawnSync } from "node:child_process";
import { createHash, randomUUID } from "node:crypto";
import os from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

export const REQUIRED_CHECKPOINT_MINUTES = Object.freeze([0, 30, 60]);
export const FINAL_THRESHOLDS = Object.freeze({
  minimumElapsedMinutes: 60,
  maximumBreakMinutes: 10,
  minimumApps: 6,
  minimumCommitsPerApp: 15,
  minimumKeyEvents: 750,
  minimumCommits: 150,
  minimumAppSwitches: 12,
});
export const OPERATOR_DECLARATION = "human-operated-not-automated-or-replayed";

const RELEASE_VERSION = readFileSync(new URL("../VERSION", import.meta.url), "utf8").trim();
const EXPECTED_ARTIFACT = Object.freeze({
  bundleIdentifier: "com.hongye.facai.inputmethod.zhizi",
  version: RELEASE_VERSION,
  minimumBuild: 1_020,
});
const ARTIFACT_KEYS = Object.freeze([
  "installLocation",
  "artifactChecksum",
  "executableChecksum",
  "bundleIdentifier",
  "version",
  "build",
  "productCommit",
  "productInputsSHA256",
  "teamIdentifierSHA256",
  "leafAuthoritySHA256",
]);
const ARTIFACT_KEY_SET = new Set(ARTIFACT_KEYS);
const RELEASE_POINTER_KEYS = Object.freeze([
  "pointerSHA256",
  "pointerReportSetSHA256",
  "inputArtifactChecksum",
  "managerArtifactChecksum",
  "artifactVersion",
  "artifactBuild",
  "productCommit",
  "productInputsSHA256",
  "teamIdentifierSHA256",
  "leafAuthoritySHA256",
  "runId",
]);
const RELEASE_POINTER_KEY_SET = new Set(RELEASE_POINTER_KEYS);
const RELEASE_POINTER_COMMENT_KEYS = new Set([
  "artifact_version", "artifact_build", "product_commit", "product_inputs_sha256",
  "team_identifier_sha256", "leaf_authority_sha256", "run_id", "pointer_report_set_sha256",
  "behavior_report", "base_report", "fuzzy_report", "userdb_report",
  "userdb_process_report", "performance_report", "harness_build_report",
]);
const FROZEN_ARTIFACT_METADATA_KEYS = new Set([
  "format_version",
  "artifact_version",
  "artifact_build",
  "product_commit",
  "product_inputs_sha256",
  "team_identifier_sha256",
  "leaf_authority_sha256",
  "input_basename",
  "manager_basename",
  "input_artifact_sha256",
  "manager_artifact_sha256",
]);
const RELEASE_REPORTS = Object.freeze({
  behavior_report: Object.freeze({ task: "V03-01", suffix: "behavior-golden.txt", format: "behavior-text" }),
  base_report: Object.freeze({ task: "V03-02", suffix: "base-500.json", format: "json" }),
  fuzzy_report: Object.freeze({ task: "V03-02", suffix: "fuzzy-56.json", format: "json" }),
  userdb_report: Object.freeze({ task: "V03-04", suffix: "userdb.json", format: "json" }),
  userdb_process_report: Object.freeze({
    task: "V03-04",
    suffix: "userdb-process-recovery.txt",
    format: "userdb-process-text",
  }),
  performance_report: Object.freeze({ task: "V03-06", suffix: "performance-stress.json", format: "json" }),
  harness_build_report: Object.freeze({ task: "V03-06", suffix: "harness-build.txt", format: "harness-text" }),
});
const RUNTIME_KEYS = Object.freeze([
  "formatVersion",
  "generation",
  "processID",
  "launchedAt",
  "bundleChecksum",
  "executableChecksum",
  "bundleIdentifier",
  "version",
  "build",
  "productCommit",
  "productInputsSHA256",
]);
const RUNTIME_KEY_SET = new Set(RUNTIME_KEYS);
const STATE_KEYS = new Set([
  "formatVersion", "sessionId", "artifactChecksum", "installedArtifact",
  "releasePointer", "runtimeAttestation", "startedAt", "checkpoints",
  "formalMode", "testOnly", "validatorSHA256", "productCommit", "productInputsSHA256",
  "pointerReportSetSHA256", "finalReportSHA256",
]);
const CHECKPOINT_KEYS = new Set(["scheduledMinute", "recordedAt", "report"]);
const REPORT_KEYS = new Set([
  "artifactChecksum", "processInstanceID", "firstSequence", "lastSequence", "startedAt",
  "checkpointAt", "elapsedMinutes", "keyEvents", "commits",
  "appActivations", "appSwitches",
  "apps", "perAppCommits", "secureDegradedEvents", "detectedRecoveryEvents",
  "p50Milliseconds", "p95Milliseconds", "p99Milliseconds", "containsTextFields",
  "installedArtifact", "releasePointer", "runtimeAttestation",
  "formalMode", "testOnly", "validatorSHA256", "productCommit", "productInputsSHA256",
  "pointerReportSetSHA256",
]);
const ZERO_SHA256 = "0".repeat(64);
const repositoryRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const validatorPath = fileURLToPath(import.meta.url);
const FORMAL_INVOCATION = Symbol("formal-dogfood-cli");

const NON_KEY_EVENT_KINDS = new Set([
  "session-open",
  "app-activate",
  "focusLost",
  "deactivate",
  "candidateClick",
]);
const RECOVERY_EVENT_KINDS = new Set([
  "unclean-recovery",
  "process-restart",
  "input-method-restart",
]);
const TEXT_FIELD_KEYS = new Set([
  "apikey",
  "body",
  "candidate",
  "candidates",
  "candidatetext",
  "clipboard",
  "committext",
  "composition",
  "compositiontext",
  "content",
  "input",
  "inputtext",
  "markedtext",
  "password",
  "payload",
  "secret",
  "text",
  "value",
]);
const DIAGNOSTIC_EVENT_KEYS = new Set([
  "timestamp",
  "eventKind",
  "status",
  "consumed",
  "secureDegraded",
  "coreElapsedNanoseconds",
  "totalElapsedNanoseconds",
  "candidateCount",
  "committed",
  "selectionEventID",
  "clientHash",
  "detailed",
  "processInstanceID",
  "sequence",
]);
const DIAGNOSTIC_EVENT_KINDS = new Set([
  "text", "backspace", "left", "right", "space", "enter", "escape",
  "pageUp", "pageDown", "up", "down", "shiftTap", "capsLock",
  "focusLost", "deactivate", "sleep", "resume", "deleteCandidate", "unsupported",
  "candidateClick", "session-open", "app-activate", "session-sleep", "session-resume",
  "snapshot-unavailable", "snapshot-loaded", "unclean-recovery",
  "system-sleep", "system-resume", "runtime-attestation-unavailable",
]);
const DOGFOOD_APP_CATEGORIES = Object.freeze({
  "APP-01": "standard-appkit",
  "APP-02": "browser-rich-text",
  "APP-03": "browser-rich-text",
  "APP-04": "browser-rich-text",
  "APP-05": "code-editor-terminal",
  "APP-06": "code-editor-terminal",
  "APP-07": "code-editor-terminal",
  "APP-08": "instant-messaging",
});
export const DOGFOOD_APP_HASHES = Object.freeze({
  "APP-01": "c82d193a0e636606",
  "APP-02": "7cd9df4fce2816bc",
  "APP-03": "f3d365f50a793bdc",
  "APP-04": "9aefac56d356c43b",
  "APP-05": "f238a5c8b2b01acf",
  "APP-06": "29cc372c34959742",
  "APP-07": "3ded7287f3d6f274",
  "APP-08": "37d3b9dbfc68fcaf",
});
const REQUIRED_APP_CATEGORIES = Object.freeze([
  "standard-appkit",
  "browser-rich-text",
  "code-editor-terminal",
  "instant-messaging",
]);
const ATTESTATION_KEYS = new Set([
  "formatVersion",
  "sessionId",
  "artifactChecksum",
  "declaredAt",
  "operatorDeclaration",
  "humanOperated",
  "automatedReplay",
  "syntheticInput",
  "inputMethodCrash",
  "hang",
  "restartUsedToMaskFailure",
  "lostText",
  "duplicateCommit",
  "crossSessionState",
  "secureInputLeak",
  "p0Count",
  "openP1Count",
  "breaks",
  "coveredApps",
]);
const BREAK_KEYS = new Set(["startedAt", "endedAt"]);
const COVERED_APP_KEYS = new Set(["appId", "clientHash"]);

function resolvePaths() {
  const userHome = os.userInfo().homedir;
  const stateDirectory = path.join(userHome, "Library/Application Support/AgentInputMethod/Dogfood");
  return {
    stateDirectory,
    statePath: path.join(stateDirectory, "active-session.json"),
    logDirectory: path.join(userHome, "Library/Logs/AgentInputMethod"),
    inputMethodBundle: path.join(userHome, "Library/Input Methods/Agent Input Method.app"),
    runtimeMarker: path.join(userHome, "Library/Application Support/AgentInputMethod/Runtime/running.marker"),
    releasePointer: path.join(repositoryRoot, "docs/current/evidence/v0.2-v0.3/RC_ARTIFACT.sha256"),
  };
}

function parseTimestamp(value, label) {
  const milliseconds = Date.parse(value);
  if (!Number.isFinite(milliseconds)) throw new Error(`${label} must be an ISO-8601 timestamp`);
  return milliseconds;
}

function assertChecksum(value, label = "artifact checksum") {
  if (!/^[0-9a-f]{64}$/u.test(value ?? "")) throw new Error(`${label} is required`);
}

function hasOnlyKeys(value, allowedKeys) {
  return value && typeof value === "object" && !Array.isArray(value)
    && Object.keys(value).every((key) => allowedKeys.has(key));
}

function hasExactKeys(value, allowedKeys) {
  return hasOnlyKeys(value, allowedKeys) && Object.keys(value).length === allowedKeys.size;
}

function sha256(value) {
  return createHash("sha256").update(value).digest("hex");
}

function assertAuthorizedTeamIdentifierHash(teamIdentifierSHA256, label) {
  assertChecksum(teamIdentifierSHA256, `${label} TeamIdentifier hash`);
  const authorizedTeamIdentifier = process.env.AIM_AUTHORIZED_TEAM_ID;
  if (authorizedTeamIdentifier !== undefined
    && (!/^[A-Z0-9]{10}$/u.test(authorizedTeamIdentifier)
      || sha256(authorizedTeamIdentifier) !== teamIdentifierSHA256)) {
    throw new Error(`${label} TeamIdentifier does not match AIM_AUTHORIZED_TEAM_ID`);
  }
}

function lstatIfPresent(filePath) {
  try {
    return lstatSync(filePath);
  } catch (error) {
    if (error?.code === "ENOENT") return null;
    throw error;
  }
}

function runChecked(executable, arguments_, label) {
  const result = spawnSync(executable, arguments_, { encoding: "utf8" });
  if (result.error || result.status !== 0) {
    const detail = `${result.stderr ?? ""}${result.stdout ?? ""}`.trim();
    throw new Error(`${label} failed${detail ? `: ${detail}` : ""}`);
  }
  return `${result.stdout ?? ""}${result.stderr ?? ""}`.trim();
}

function readBoundedEvidenceFile(evidenceRoot, relativePath, label, maximumBytes) {
  if (typeof relativePath !== "string" || relativePath.length === 0 || path.isAbsolute(relativePath)) {
    throw new Error(`${label} path must be a non-empty relative path`);
  }
  const segments = relativePath.split("/");
  if (segments.some((segment) => segment.length === 0 || segment === "." || segment === "..")) {
    throw new Error(`${label} path is invalid`);
  }
  let current = evidenceRoot;
  for (const [index, segment] of segments.entries()) {
    current = path.join(current, segment);
    const status = lstatSync(current);
    if (status.isSymbolicLink()) throw new Error(`${label} path may not contain a symlink`);
    if (index < segments.length - 1 && !status.isDirectory()) throw new Error(`${label} parent must be a directory`);
    if (index === segments.length - 1 && !status.isFile()) throw new Error(`${label} must be a regular file`);
  }
  const status = lstatSync(current);
  if (status.nlink !== 1) throw new Error(`${label} must be a single-link file`);
  if (status.size > maximumBytes) throw new Error(`${label} is unexpectedly large`);
  const data = readFileSync(current);
  const statusAfterRead = lstatSync(current);
  if (status.dev !== statusAfterRead.dev || status.ino !== statusAfterRead.ino
    || status.size !== statusAfterRead.size || status.mtimeMs !== statusAfterRead.mtimeMs
    || statusAfterRead.nlink !== 1) {
    throw new Error(`${label} changed while it was being read`);
  }
  return { absolutePath: current, data };
}

function parseManifest(manifestData, label) {
  const entries = new Map();
  for (const line of manifestData.toString("utf8").split(/\r?\n/u)) {
    if (!line) continue;
    const match = line.match(/^([0-9a-f]{64})  \.\/(.+)$/u);
    if (!match || match[2].includes("/") || entries.has(match[2])) {
      throw new Error(`${label} contains an invalid or duplicate entry`);
    }
    entries.set(match[2], match[1]);
  }
  if (entries.size === 0) throw new Error(`${label} is empty`);
  return entries;
}

function textMetadata(data, label) {
  const metadata = new Map();
  const supported = new Set([
    "artifact_checksum", "manager_checksum", "artifact_version", "artifact_build",
    "product_commit", "product_inputs_sha256", "run_id",
  ]);
  for (const line of data.toString("utf8").split(/\r?\n/u)) {
    const match = line.match(/^([a-z0-9_]+)=(.*)$/u);
    if (!match || !supported.has(match[1])) continue;
    if (metadata.has(match[1])) throw new Error(`${label} contains duplicate artifact metadata`);
    metadata.set(match[1], match[2]);
  }
  return metadata;
}

function assertReportArtifactMetadata(data, format, pointer, label) {
  if (format === "json") {
    let report;
    try {
      report = JSON.parse(data.toString("utf8"));
    } catch {
      throw new Error(`${label} is not valid JSON`);
    }
    if (!report || typeof report !== "object" || Array.isArray(report)
      || report.artifactChecksum !== pointer.inputArtifactChecksum
      || report.artifactVersion !== pointer.artifactVersion
      || report.artifactBuild !== pointer.artifactBuild
      || report.productCommit !== pointer.productCommit
      || report.productInputsSHA256 !== pointer.productInputsSHA256
      || report.runId !== pointer.runId) {
      throw new Error(`${label} artifact metadata does not match the frozen RC`);
    }
    return;
  }

  const metadata = textMetadata(data, label);
  const expected = new Map([
    ["artifact_checksum", pointer.inputArtifactChecksum],
    ["artifact_build", pointer.artifactBuild],
    ["product_commit", pointer.productCommit],
    ["product_inputs_sha256", pointer.productInputsSHA256],
    ["run_id", pointer.runId],
  ]);
  if (["behavior-text", "userdb-process-text", "harness-text"].includes(format)) {
    expected.set("artifact_version", pointer.artifactVersion);
  }
  if (format === "harness-text") expected.set("manager_checksum", pointer.managerArtifactChecksum);
  for (const [key, value] of expected) {
    if (metadata.get(key) !== value) throw new Error(`${label} ${key} does not match the frozen RC`);
  }
}

function inspectPointerReportSet(pointerPath, comments, pointer) {
  const evidenceRoot = path.dirname(pointerPath);
  const manifestCache = new Map();
  const summaries = [];
  const prefix = `rc-build${pointer.artifactBuild}-${pointer.inputArtifactChecksum.slice(0, 12)}-${pointer.runId}`;
  for (const [commentKey, contract] of Object.entries(RELEASE_REPORTS)) {
    const expectedName = `${prefix}-${contract.suffix}`;
    const expectedRelativePath = `${contract.task}/artifacts/${expectedName}`;
    const relativePath = comments.get(commentKey);
    if (relativePath !== expectedRelativePath) {
      throw new Error(`frozen RC pointer ${commentKey} path or filename is invalid`);
    }
    const label = `frozen RC ${commentKey}`;
    const report = readBoundedEvidenceFile(evidenceRoot, relativePath, label, 128 * 1_024 * 1_024);
    const reportSHA256 = sha256(report.data);
    const manifestRelativePath = `${contract.task}/artifacts/MANIFEST.sha256`;
    let manifest = manifestCache.get(manifestRelativePath);
    if (!manifest) {
      const manifestFile = readBoundedEvidenceFile(
        evidenceRoot,
        manifestRelativePath,
        `${contract.task} evidence manifest`,
        16 * 1_024 * 1_024,
      );
      manifest = {
        entries: parseManifest(manifestFile.data, `${contract.task} evidence manifest`),
      };
      manifestCache.set(manifestRelativePath, manifest);
    }
    if (manifest.entries.get(expectedName) !== reportSHA256) {
      throw new Error(`${label} is missing from MANIFEST.sha256 or its checksum changed`);
    }
    assertReportArtifactMetadata(report.data, contract.format, pointer, label);
    summaries.push({
      key: commentKey,
      relativePath,
      reportSHA256,
    });
  }
  return sha256(JSON.stringify(summaries));
}

function readPlistValue(infoPlist, key) {
  return runChecked(
    "/usr/libexec/PlistBuddy",
    ["-c", `Print :${key}`, infoPlist],
    `read ${key}`,
  ).trim();
}

function assertInstalledArtifact(artifact, label = "installed input method") {
  if (!hasExactKeys(artifact, ARTIFACT_KEY_SET)) throw new Error(`${label} metadata is incomplete or contains unsupported fields`);
  for (const key of ARTIFACT_KEYS) {
    if (typeof artifact[key] !== "string" || artifact[key].length === 0) {
      throw new Error(`${label} metadata ${key} is missing`);
    }
  }
  assertChecksum(artifact.artifactChecksum, `${label} checksum`);
  assertChecksum(artifact.executableChecksum, `${label} executable checksum`);
  if (artifact.installLocation !== "standard-user-input-methods") throw new Error(`${label} install location is invalid`);
  if (artifact.bundleIdentifier !== EXPECTED_ARTIFACT.bundleIdentifier) throw new Error(`${label} bundle ID is invalid`);
  if (artifact.version !== EXPECTED_ARTIFACT.version) throw new Error(`${label} version is not ${EXPECTED_ARTIFACT.version}`);
  if (!/^[1-9][0-9]*$/u.test(artifact.build) || Number(artifact.build) < EXPECTED_ARTIFACT.minimumBuild) {
    throw new Error(`${label} build must be at least ${EXPECTED_ARTIFACT.minimumBuild}`);
  }
  if (!/^(?:[0-9a-f]{40}|[0-9a-f]{64})$/u.test(artifact.productCommit)) {
    throw new Error(`${label} ProductCommit is invalid`);
  }
  assertChecksum(artifact.productInputsSHA256, `${label} ProductInputsSHA256`);
  assertAuthorizedTeamIdentifierHash(artifact.teamIdentifierSHA256, label);
  assertChecksum(artifact.leafAuthoritySHA256, `${label} signing authority hash`);
}

function artifactMismatchFailures(actual, expected, label) {
  const failures = [];
  try {
    assertInstalledArtifact(actual, label);
  } catch (error) {
    failures.push(error.message);
    return failures;
  }
  for (const key of ARTIFACT_KEYS) {
    if (actual[key] !== expected?.[key]) failures.push(`${label} ${key} does not match the frozen RC`);
  }
  return failures;
}

function requireFrozenArtifact(actual, expected) {
  const failures = artifactMismatchFailures(actual, expected, "installed input method");
  if (failures.length > 0) throw new Error(failures.join("; "));
}

export function inspectInstalledArtifact(bundlePath) {
  let status;
  try {
    status = lstatSync(bundlePath);
  } catch {
    throw new Error(`installed input method is missing: ${bundlePath}`);
  }
  if (status.isSymbolicLink() || !status.isDirectory()) {
    throw new Error(`installed input method path must be a real directory: ${bundlePath}`);
  }
  const canonicalBundlePath = realpathSync(bundlePath);
  const infoPlist = path.join(canonicalBundlePath, "Contents/Info.plist");
  runChecked("/usr/bin/codesign", ["--verify", "--deep", "--strict", "--verbose=2", canonicalBundlePath], "strict code-signature verification");
  const signature = runChecked("/usr/bin/codesign", ["-d", "--verbose=4", canonicalBundlePath], "read code signature");
  const teamIdentifier = signature.match(/^TeamIdentifier=(.+)$/mu)?.[1]?.trim() ?? "";
  const signingAuthority = signature.match(/^Authority=(.+)$/mu)?.[1]?.trim() ?? "";
  if (!/^[A-Z0-9]{10}$/u.test(teamIdentifier)) {
    throw new Error("installed input method TeamIdentifier is invalid");
  }
  if (!signingAuthority.startsWith("Apple Development:")) {
    throw new Error("installed input method signing authority is invalid");
  }
  const artifactChecksum = runChecked(
    "/bin/bash",
    [path.join(repositoryRoot, "script/artifact_checksum.sh"), canonicalBundlePath],
    "input method checksum",
  );
  const artifact = {
    installLocation: "standard-user-input-methods",
    artifactChecksum,
    executableChecksum: runChecked(
      "/usr/bin/shasum",
      ["-a", "256", path.join(canonicalBundlePath, "Contents/MacOS/AgentInputMethod")],
      "input method executable checksum",
    ).split(/\s+/u)[0],
    bundleIdentifier: readPlistValue(infoPlist, "CFBundleIdentifier"),
    version: readPlistValue(infoPlist, "CFBundleShortVersionString"),
    build: readPlistValue(infoPlist, "CFBundleVersion"),
    productCommit: readPlistValue(infoPlist, "AgentIMEProductCommit"),
    productInputsSHA256: readPlistValue(infoPlist, "AgentIMEProductInputsSHA256"),
    teamIdentifierSHA256: sha256(teamIdentifier),
    leafAuthoritySHA256: sha256(signingAuthority),
  };
  assertInstalledArtifact(artifact);
  return artifact;
}

function assertReleasePointer(pointer, label = "frozen RC pointer") {
  if (!hasExactKeys(pointer, RELEASE_POINTER_KEY_SET)) throw new Error(`${label} metadata is incomplete or contains unsupported fields`);
  for (const key of RELEASE_POINTER_KEYS) {
    if (typeof pointer[key] !== "string" || pointer[key].length === 0) throw new Error(`${label} ${key} is missing`);
  }
  for (const key of [
    "pointerSHA256", "pointerReportSetSHA256", "inputArtifactChecksum", "managerArtifactChecksum",
    "productInputsSHA256", "teamIdentifierSHA256", "leafAuthoritySHA256",
  ]) {
    assertChecksum(pointer[key], `${label} ${key}`);
  }
  if (pointer.artifactVersion !== EXPECTED_ARTIFACT.version) throw new Error(`${label} version is not ${EXPECTED_ARTIFACT.version}`);
  if (!/^[1-9][0-9]*$/u.test(pointer.artifactBuild) || Number(pointer.artifactBuild) < EXPECTED_ARTIFACT.minimumBuild) {
    throw new Error(`${label} build must be at least ${EXPECTED_ARTIFACT.minimumBuild}`);
  }
  if (!/^(?:[0-9a-f]{40}|[0-9a-f]{64})$/u.test(pointer.productCommit)) throw new Error(`${label} ProductCommit is invalid`);
  assertAuthorizedTeamIdentifierHash(pointer.teamIdentifierSHA256, label);
  if (!/^[0-9]{8}T[0-9]{6}Z-[0-9]+$/u.test(pointer.runId)) throw new Error(`${label} run ID is invalid`);
}

function pointerMismatchFailures(actual, expected, label) {
  const failures = [];
  try {
    assertReleasePointer(actual, label);
  } catch (error) {
    failures.push(error.message);
    return failures;
  }
  for (const key of RELEASE_POINTER_KEYS) {
    if (actual[key] !== expected?.[key]) failures.push(`${label} ${key} changed after checkpoint 0`);
  }
  return failures;
}

function assertFrozenTreeContract(rootPath) {
  const pending = [rootPath];
  while (pending.length > 0) {
    const entryPath = pending.pop();
    const status = lstatSync(entryPath);
    if (status.isSymbolicLink()) throw new Error("frozen RC tree contains a symlink");
    if (!status.isDirectory() && !status.isFile()) {
      throw new Error("frozen RC tree contains a special filesystem entry");
    }
    if ((status.mode & 0o222) !== 0) throw new Error("frozen RC tree contains writable mode bits");
    let effectivelyWritable = true;
    try {
      accessSync(entryPath, fsConstants.W_OK);
    } catch (error) {
      if (error?.code === "EACCES" || error?.code === "EPERM") effectivelyWritable = false;
      else throw error;
    }
    if (effectivelyWritable) throw new Error("frozen RC tree is writable through effective permissions");
    if (status.isFile()) {
      if (status.nlink !== 1) throw new Error("frozen RC tree contains a hard-linked file");
      continue;
    }
    for (const name of readdirSync(entryPath)) pending.push(path.join(entryPath, name));
  }
}

function parseFrozenArtifactMetadata(metadataPath) {
  const status = lstatSync(metadataPath);
  if (status.isSymbolicLink() || !status.isFile()) throw new Error("FROZEN_ARTIFACTS must be a regular non-symlink file");
  if (status.size > 16 * 1_024) throw new Error("FROZEN_ARTIFACTS is unexpectedly large");
  const metadata = new Map();
  const lines = readFileSync(metadataPath, "utf8").split(/\r?\n/u);
  if (lines.at(-1) === "") lines.pop();
  for (const line of lines) {
    const match = line.match(/^([a-z0-9_]+)=(.*)$/u);
    if (!match || !FROZEN_ARTIFACT_METADATA_KEYS.has(match[1]) || metadata.has(match[1])) {
      throw new Error("FROZEN_ARTIFACTS contains an invalid, unknown, or duplicate field");
    }
    metadata.set(match[1], match[2]);
  }
  if (metadata.size !== FROZEN_ARTIFACT_METADATA_KEYS.size) {
    throw new Error("FROZEN_ARTIFACTS is missing a required field");
  }
  return metadata;
}

function inspectFrozenFilesystemContract(artifacts, pointer) {
  for (const artifact of artifacts) {
    if (!path.isAbsolute(artifact.bundlePath)) throw new Error("frozen RC artifact path must be absolute");
  }
  const frozenRoot = path.dirname(artifacts[0].bundlePath);
  if (path.dirname(artifacts[1].bundlePath) !== frozenRoot) {
    throw new Error("frozen RC input and manager must share one directory");
  }
  if (path.basename(frozenRoot) !== `${EXPECTED_ARTIFACT.version}-build${pointer.artifactBuild}`
    || path.basename(path.dirname(frozenRoot)) !== "rc"
    || path.basename(path.dirname(path.dirname(frozenRoot))) !== "build") {
    throw new Error("frozen RC artifact directory name is invalid");
  }
  if (realpathSync(frozenRoot) !== frozenRoot
    || artifacts.some((artifact) => realpathSync(artifact.bundlePath) !== artifact.bundlePath)) {
    throw new Error("frozen RC artifact path may not contain a symlink");
  }
  const expectedEntries = ["Agent Input Method.app", "Agent Input Method.inputmethod", "FROZEN_ARTIFACTS"].sort();
  const actualEntries = readdirSync(frozenRoot).sort();
  if (actualEntries.length !== expectedEntries.length
    || actualEntries.some((entry, index) => entry !== expectedEntries[index])) {
    throw new Error("frozen RC directory must contain exactly its two bundles and FROZEN_ARTIFACTS");
  }
  assertFrozenTreeContract(frozenRoot);
  const metadata = parseFrozenArtifactMetadata(path.join(frozenRoot, "FROZEN_ARTIFACTS"));
  const expectedMetadata = new Map([
    ["format_version", "1"],
    ["artifact_version", pointer.artifactVersion],
    ["artifact_build", pointer.artifactBuild],
    ["product_commit", pointer.productCommit],
    ["product_inputs_sha256", pointer.productInputsSHA256],
    ["team_identifier_sha256", pointer.teamIdentifierSHA256],
    ["leaf_authority_sha256", pointer.leafAuthoritySHA256],
    ["input_basename", "Agent Input Method.inputmethod"],
    ["manager_basename", "Agent Input Method.app"],
    ["input_artifact_sha256", pointer.inputArtifactChecksum],
    ["manager_artifact_sha256", pointer.managerArtifactChecksum],
  ]);
  for (const [key, value] of expectedMetadata) {
    if (metadata.get(key) !== value) throw new Error(`FROZEN_ARTIFACTS ${key} does not match the frozen RC pointer`);
  }
}

function readFrozenReleasePointer(pointerPath, { verifyReportSet = true } = {}) {
  let status;
  try {
    status = lstatSync(pointerPath);
  } catch {
    throw new Error("frozen RC pointer is missing; complete the joint RC gate first");
  }
  if (status.isSymbolicLink() || !status.isFile() || status.nlink !== 1) {
    throw new Error("frozen RC pointer must be a regular non-symlink single-link file");
  }
  if (status.size > 64 * 1_024) throw new Error("frozen RC pointer is unexpectedly large");
  const data = readFileSync(pointerPath);
  const statusAfterRead = lstatSync(pointerPath);
  if (status.dev !== statusAfterRead.dev || status.ino !== statusAfterRead.ino
    || status.size !== statusAfterRead.size || status.mtimeMs !== statusAfterRead.mtimeMs
    || statusAfterRead.nlink !== 1) {
    throw new Error("frozen RC pointer changed while it was being read");
  }
  const content = data.toString("utf8");
  const comments = new Map();
  const artifacts = [];
  for (const line of content.split(/\r?\n/u)) {
    if (!line) continue;
    if (line.startsWith("# ")) {
      const separator = line.indexOf("=");
      if (separator <= 2) throw new Error("frozen RC pointer contains an invalid comment");
      const key = line.slice(2, separator);
      if (!RELEASE_POINTER_COMMENT_KEYS.has(key) || comments.has(key)) {
        throw new Error("frozen RC pointer contains an unknown or duplicate field");
      }
      comments.set(key, line.slice(separator + 1));
      continue;
    }
    const match = line.match(/^([0-9a-f]{64})  (.+)$/u);
    if (!match) throw new Error("frozen RC pointer contains an invalid line");
    artifacts.push({ checksum: match[1], bundlePath: match[2] });
  }
  if (comments.size !== RELEASE_POINTER_COMMENT_KEYS.size) throw new Error("frozen RC pointer is missing a required field");
  if (artifacts.length !== 2) throw new Error("frozen RC pointer must contain exactly input and manager artifacts");
  if (path.basename(artifacts[0].bundlePath) !== "Agent Input Method.inputmethod"
    || path.basename(artifacts[1].bundlePath) !== "Agent Input Method.app") {
    throw new Error("frozen RC pointer artifact order or names are invalid");
  }
  const pointer = {
    pointerSHA256: sha256(data),
    pointerReportSetSHA256: comments.get("pointer_report_set_sha256") ?? "",
    inputArtifactChecksum: artifacts[0].checksum,
    managerArtifactChecksum: artifacts[1].checksum,
    artifactVersion: comments.get("artifact_version") ?? "",
    artifactBuild: comments.get("artifact_build") ?? "",
    productCommit: comments.get("product_commit") ?? "",
    productInputsSHA256: comments.get("product_inputs_sha256") ?? "",
    teamIdentifierSHA256: comments.get("team_identifier_sha256") ?? "",
    leafAuthoritySHA256: comments.get("leaf_authority_sha256") ?? "",
    runId: comments.get("run_id") ?? "",
  };
  assertReleasePointer(pointer);
  inspectFrozenFilesystemContract(artifacts, pointer);
  for (const [index, artifact] of artifacts.entries()) {
    const actual = runChecked(
      "/bin/bash",
      [path.join(repositoryRoot, "script/artifact_checksum.sh"), artifact.bundlePath],
      index === 0 ? "frozen input artifact checksum" : "frozen manager artifact checksum",
    );
    if (actual !== artifact.checksum) throw new Error("frozen RC artifact no longer matches its pointer checksum");
  }
  const computedReportSetSHA256 = verifyReportSet
    ? inspectPointerReportSet(pointerPath, comments, pointer)
    : null;
  inspectFrozenFilesystemContract(artifacts, pointer);
  return { pointer, computedReportSetSHA256 };
}

export function computeFrozenPointerReportSetSHA256(pointerPath) {
  return readFrozenReleasePointer(pointerPath).computedReportSetSHA256;
}

export function inspectFrozenReleasePointer(pointerPath) {
  const { pointer, computedReportSetSHA256 } = readFrozenReleasePointer(pointerPath);
  if (pointer.pointerReportSetSHA256 === ZERO_SHA256
    || pointer.pointerReportSetSHA256 !== computedReportSetSHA256) {
    throw new Error("frozen RC pointer report-set checksum does not match its reports and manifests");
  }
  return pointer;
}

export function inspectCurrentFrozenReleasePointer(pointerPath) {
  return readFrozenReleasePointer(pointerPath, { verifyReportSet: false }).pointer;
}

function requireArtifactMatchesPointer(artifact, pointer) {
  const matches = artifact.artifactChecksum === pointer.inputArtifactChecksum
    && artifact.version === pointer.artifactVersion
    && artifact.build === pointer.artifactBuild
    && artifact.productCommit === pointer.productCommit
    && artifact.productInputsSHA256 === pointer.productInputsSHA256
    && artifact.teamIdentifierSHA256 === pointer.teamIdentifierSHA256
    && artifact.leafAuthoritySHA256 === pointer.leafAuthoritySHA256;
  if (!matches) throw new Error("installed input method does not match the frozen RC pointer");
}

function assertRuntimeAttestation(runtime, label = "running input method") {
  if (!hasExactKeys(runtime, RUNTIME_KEY_SET)) throw new Error(`${label} attestation is incomplete or contains unsupported fields`);
  if (runtime.formatVersion !== 1) throw new Error(`${label} attestation format is invalid`);
  if (!/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/u.test(runtime.generation ?? "")) {
    throw new Error(`${label} generation is invalid`);
  }
  if (!Number.isSafeInteger(runtime.processID) || runtime.processID <= 1) throw new Error(`${label} process ID is invalid`);
  parseTimestamp(runtime.launchedAt, `${label} launch time`);
  assertChecksum(runtime.bundleChecksum, `${label} bundle checksum`);
  assertChecksum(runtime.executableChecksum, `${label} executable checksum`);
  if (runtime.bundleIdentifier !== EXPECTED_ARTIFACT.bundleIdentifier) throw new Error(`${label} bundle ID is invalid`);
  if (runtime.version !== EXPECTED_ARTIFACT.version) throw new Error(`${label} version is invalid`);
  if (!/^[1-9][0-9]*$/u.test(runtime.build) || Number(runtime.build) < EXPECTED_ARTIFACT.minimumBuild) throw new Error(`${label} build is invalid`);
  if (!/^(?:[0-9a-f]{40}|[0-9a-f]{64})$/u.test(runtime.productCommit)) throw new Error(`${label} ProductCommit is invalid`);
  assertChecksum(runtime.productInputsSHA256, `${label} ProductInputsSHA256`);
}

function runtimeMismatchFailures(actual, expected, label) {
  const failures = [];
  try {
    assertRuntimeAttestation(actual, label);
  } catch (error) {
    failures.push(error.message);
    return failures;
  }
  for (const key of RUNTIME_KEYS) {
    if (actual[key] !== expected?.[key]) failures.push(`${label} ${key} changed after checkpoint 0`);
  }
  return failures;
}

export function inspectRunningInputMethod(markerPath) {
  let status;
  try {
    status = lstatSync(markerPath);
  } catch {
    throw new Error("running input method attestation is missing; activate the frozen input method first");
  }
  if (status.isSymbolicLink() || !status.isFile() || status.nlink !== 1 || (status.mode & 0o077) !== 0) {
    throw new Error("running input method attestation must be a private regular non-symlink single-link file");
  }
  if (status.size > 64 * 1_024) throw new Error("running input method attestation is unexpectedly large");
  let runtime;
  try {
    runtime = JSON.parse(readFileSync(markerPath, "utf8"));
  } catch {
    throw new Error("running input method attestation is invalid");
  }
  const statusAfterRead = lstatSync(markerPath);
  if (status.dev !== statusAfterRead.dev || status.ino !== statusAfterRead.ino
    || status.size !== statusAfterRead.size || status.mtimeMs !== statusAfterRead.mtimeMs
    || statusAfterRead.nlink !== 1) {
    throw new Error("running input method attestation changed while it was being read");
  }
  assertRuntimeAttestation(runtime);
  try {
    process.kill(runtime.processID, 0);
  } catch {
    throw new Error("attested input method process is not running");
  }
  const commandName = runChecked(
    "/bin/ps",
    ["-p", String(runtime.processID), "-o", "ucomm="],
    "input method process identity",
  );
  if (commandName !== "AgentInputMethod") throw new Error("attested process is not AgentInputMethod");
  return runtime;
}

function requireRuntimeMatchesArtifact(runtime, artifact) {
  const matches = runtime.bundleChecksum === artifact.artifactChecksum
    && runtime.executableChecksum === artifact.executableChecksum
    && runtime.bundleIdentifier === artifact.bundleIdentifier
    && runtime.version === artifact.version
    && runtime.build === artifact.build
    && runtime.productCommit === artifact.productCommit
    && runtime.productInputsSHA256 === artifact.productInputsSHA256;
  if (!matches) throw new Error("running input method does not match the installed frozen RC");
}

function provenanceForPointer(pointer, formalMode) {
  assertReleasePointer(pointer);
  const validatorSHA256 = sha256(readFileSync(validatorPath));
  if (formalMode) {
    const supportScript = path.join(repositoryRoot, "script/rc_gate_support.sh");
    const verifiedCommit = runChecked(
      "/bin/bash",
      [supportScript, "verify-inputs", repositoryRoot, pointer.productCommit],
      "formal ProductCommit input verification",
    );
    if (verifiedCommit !== pointer.productCommit) throw new Error("formal ProductCommit verification returned an unexpected commit");
    const productInputsSHA256 = runChecked(
      "/bin/bash",
      [supportScript, "fingerprint", repositoryRoot, pointer.productCommit],
      "formal ProductInputs fingerprint",
    );
    if (productInputsSHA256 !== pointer.productInputsSHA256) {
      throw new Error("current ProductInputs do not match the frozen RC pointer");
    }
  }
  return {
    formalMode,
    testOnly: !formalMode,
    validatorSHA256,
    productCommit: pointer.productCommit,
    productInputsSHA256: pointer.productInputsSHA256,
    pointerReportSetSHA256: pointer.pointerReportSetSHA256,
  };
}

function assertProvenance(value, pointer, label) {
  if (typeof value.formalMode !== "boolean" || typeof value.testOnly !== "boolean"
    || value.formalMode === value.testOnly) {
    throw new Error(`${label} formal/test-only provenance is invalid`);
  }
  assertChecksum(value.validatorSHA256, `${label} validator checksum`);
  if (value.productCommit !== pointer.productCommit
    || value.productInputsSHA256 !== pointer.productInputsSHA256
    || value.pointerReportSetSHA256 !== pointer.pointerReportSetSHA256) {
    throw new Error(`${label} product or pointer-report provenance is invalid`);
  }
}

function requireCurrentProvenance(actual, expected, label = "dogfood provenance") {
  for (const key of [
    "formalMode", "testOnly", "validatorSHA256", "productCommit", "productInputsSHA256",
    "pointerReportSetSHA256",
  ]) {
    if (actual[key] !== expected[key]) throw new Error(`${label} ${key} changed after checkpoint 0`);
  }
}

function completeReport(baseReport, installedArtifact, releasePointer, runtimeAttestation, provenance) {
  return {
    ...baseReport,
    installedArtifact,
    releasePointer,
    runtimeAttestation,
    ...provenance,
  };
}

function provenanceFields(value) {
  return {
    formalMode: value.formalMode,
    testOnly: value.testOnly,
    validatorSHA256: value.validatorSHA256,
    productCommit: value.productCommit,
    productInputsSHA256: value.productInputsSHA256,
    pointerReportSetSHA256: value.pointerReportSetSHA256,
  };
}

function assertNonnegativeInteger(value, label) {
  if (!Number.isSafeInteger(value) || value < 0) throw new Error(`${label} is invalid`);
}

function assertReport(report, label = "dogfood report") {
  if (!hasExactKeys(report, REPORT_KEYS)) throw new Error(`${label} schema is invalid or contains unsupported fields`);
  if (containsTextFields(report)) throw new Error(`${label} contains a forbidden text field`);
  assertChecksum(report.artifactChecksum, `${label} artifact checksum`);
  if (!/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/u.test(report.processInstanceID ?? "")) {
    throw new Error(`${label} process instance ID is invalid`);
  }
  const started = parseTimestamp(report.startedAt, `${label} start`);
  const checkpoint = parseTimestamp(report.checkpointAt, `${label} checkpoint`);
  if (!Number.isFinite(report.elapsedMinutes) || report.elapsedMinutes < 0
    || Math.abs((checkpoint - started) / 60_000 - report.elapsedMinutes) > 1e-6) {
    throw new Error(`${label} elapsed time is invalid`);
  }
  for (const key of [
    "keyEvents", "commits", "appActivations", "appSwitches", "secureDegradedEvents",
    "detectedRecoveryEvents",
  ]) assertNonnegativeInteger(report[key], `${label} ${key}`);
  for (const key of ["p50Milliseconds", "p95Milliseconds", "p99Milliseconds"]) {
    if (!Number.isFinite(report[key]) || report[key] < 0) throw new Error(`${label} ${key} is invalid`);
  }
  const sequences = [report.firstSequence, report.lastSequence];
  if (sequences.some((value) => value !== null && (!Number.isSafeInteger(value) || value <= 0))
    || (sequences[0] === null) !== (sequences[1] === null)
    || (sequences[0] !== null && sequences[0] > sequences[1])) {
    throw new Error(`${label} sequence range is invalid`);
  }
  if (!Array.isArray(report.apps) || !report.apps.every((clientHash) => /^[0-9a-f]{16}$/u.test(clientHash))) {
    throw new Error(`${label} App hash list is invalid`);
  }
  if (new Set(report.apps).size !== report.apps.length
    || JSON.stringify([...report.apps].sort()) !== JSON.stringify(report.apps)) {
    throw new Error(`${label} App hash list must be unique and sorted`);
  }
  if (!report.perAppCommits || typeof report.perAppCommits !== "object" || Array.isArray(report.perAppCommits)
    || JSON.stringify(Object.keys(report.perAppCommits).sort()) !== JSON.stringify(report.apps)) {
    throw new Error(`${label} per-App commit schema is invalid`);
  }
  for (const [clientHash, commits] of Object.entries(report.perAppCommits)) {
    if (!/^[0-9a-f]{16}$/u.test(clientHash)) throw new Error(`${label} per-App hash is invalid`);
    assertNonnegativeInteger(commits, `${label} per-App commits`);
  }
  if (report.containsTextFields !== false) throw new Error(`${label} failed the no-text privacy gate`);
  assertInstalledArtifact(report.installedArtifact, `${label} installed input method`);
  assertReleasePointer(report.releasePointer, `${label} frozen RC pointer`);
  assertRuntimeAttestation(report.runtimeAttestation, `${label} running input method`);
  requireArtifactMatchesPointer(report.installedArtifact, report.releasePointer);
  requireRuntimeMatchesArtifact(report.runtimeAttestation, report.installedArtifact);
  if (report.artifactChecksum !== report.installedArtifact.artifactChecksum
    || report.processInstanceID !== report.runtimeAttestation.generation) {
    throw new Error(`${label} artifact or process identity is inconsistent`);
  }
  assertProvenance(report, report.releasePointer, label);
}

export function containsTextFields(value) {
  if (!value || typeof value !== "object") return false;
  if (Array.isArray(value)) return value.some(containsTextFields);
  return Object.entries(value).some(([key, nestedValue]) =>
    TEXT_FIELD_KEYS.has(key.toLowerCase()) || containsTextFields(nestedValue));
}

export function countAppSwitches(activations) {
  let switches = 0;
  let previousHash;
  for (const activation of activations) {
    const clientHash = activation?.clientHash;
    if (!clientHash || clientHash === "system") continue;
    if (previousHash !== undefined && clientHash !== previousHash) switches += 1;
    previousHash = clientHash;
  }
  return switches;
}

function validateDiagnosticEvent(event) {
  if (!hasExactKeys(event, DIAGNOSTIC_EVENT_KEYS)) throw new Error("diagnostic event schema is invalid");
  parseTimestamp(event.timestamp, "diagnostic event timestamp");
  if (typeof event.eventKind !== "string" || !DIAGNOSTIC_EVENT_KINDS.has(event.eventKind)) {
    throw new Error("diagnostic event kind is invalid");
  }
  if (!Number.isInteger(event.status) || event.status < -2_147_483_648 || event.status > 2_147_483_647) {
    throw new Error("diagnostic status is invalid");
  }
  for (const key of ["consumed", "secureDegraded", "committed", "detailed"]) {
    if (typeof event[key] !== "boolean") throw new Error(`diagnostic ${key} is invalid`);
  }
  for (const key of ["coreElapsedNanoseconds", "totalElapsedNanoseconds", "candidateCount", "selectionEventID", "sequence"]) {
    if (!Number.isInteger(event[key]) || event[key] < 0) throw new Error(`diagnostic ${key} is invalid`);
  }
  if (event.sequence === 0 || !Number.isSafeInteger(event.sequence)) throw new Error("diagnostic sequence is invalid");
  if (event.candidateCount > 10_000) throw new Error("diagnostic candidate count is invalid");
  if (event.clientHash !== "system" && !/^[0-9a-f]{16}$/u.test(event.clientHash ?? "")) {
    throw new Error("diagnostic client hash is invalid");
  }
  if (!/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/u.test(event.processInstanceID ?? "")) {
    throw new Error("diagnostic process instance is invalid");
  }
}

export function buildReport({
  artifactChecksum,
  startedAt,
  events,
  processInstanceID,
  now = new Date(),
}) {
  assertChecksum(artifactChecksum);
  const startedMilliseconds = parseTimestamp(startedAt, "session start");
  const nowMilliseconds = now.getTime();
  if (nowMilliseconds < startedMilliseconds) throw new Error("checkpoint precedes session start");

  const inSessionEvents = events
    .filter((event) => {
      const timestamp = parseTimestamp(event.timestamp, "diagnostic event timestamp");
      return timestamp >= startedMilliseconds && timestamp <= nowMilliseconds;
    })
    .sort((left, right) => left.timestamp.localeCompare(right.timestamp));
  for (const event of inSessionEvents) validateDiagnosticEvent(event);
  const eventsByProcess = new Map();
  for (const event of inSessionEvents) {
    const processEvents = eventsByProcess.get(event.processInstanceID) ?? [];
    processEvents.push(event);
    eventsByProcess.set(event.processInstanceID, processEvents);
  }
  for (const processEvents of eventsByProcess.values()) {
    processEvents.sort((left, right) => left.sequence - right.sequence);
    for (let index = 1; index < processEvents.length; index += 1) {
      if (processEvents[index].sequence !== processEvents[index - 1].sequence + 1) {
        throw new Error("diagnostic sequence is not contiguous within an input method process generation");
      }
    }
  }
  const interactions = inSessionEvents.filter((event) => event.clientHash !== "system");
  const keyEvents = interactions.filter((event) => !NON_KEY_EVENT_KINDS.has(event.eventKind));
  const activations = interactions.filter((event) => event.eventKind === "app-activate");
  const commitEvents = interactions.filter((event) => event.committed === true);
  const appHashes = [...new Set([...keyEvents, ...commitEvents].map((event) => event.clientHash))]
    .filter(Boolean).sort();
  const perAppCommits = Object.fromEntries(appHashes.map((clientHash) => [
    clientHash,
    commitEvents.filter((event) => event.clientHash === clientHash).length,
  ]));
  const latencies = keyEvents.map((event) => Number(event.totalElapsedNanoseconds) / 1_000_000)
    .filter(Number.isFinite).sort((left, right) => left - right);
  const percentile = (fraction) => latencies.length
    ? latencies[Math.min(latencies.length - 1, Math.max(0, Math.ceil(latencies.length * fraction) - 1))]
    : 0;

  const elapsedMinutes = (nowMilliseconds - startedMilliseconds) / 60_000;
  return {
    artifactChecksum,
    processInstanceID: processInstanceID ?? inSessionEvents[0]?.processInstanceID ?? null,
    firstSequence: inSessionEvents[0]?.sequence ?? null,
    lastSequence: inSessionEvents.at(-1)?.sequence ?? null,
    startedAt,
    checkpointAt: now.toISOString(),
    elapsedMinutes,
    keyEvents: keyEvents.length,
    commits: commitEvents.length,
    appActivations: activations.length,
    appSwitches: countAppSwitches(activations),
    apps: appHashes,
    perAppCommits,
    secureDegradedEvents: keyEvents.filter((event) => event.secureDegraded === true).length,
    detectedRecoveryEvents: inSessionEvents.filter((event) => RECOVERY_EVENT_KINDS.has(event.eventKind)).length,
    p50Milliseconds: percentile(0.50),
    p95Milliseconds: percentile(0.95),
    p99Milliseconds: percentile(0.99),
    containsTextFields: containsTextFields(inSessionEvents),
  };
}

function hasInteractionBetweenBreaks(events, lowerBound, upperBound) {
  return events.some((event) => {
    if (!event || event.clientHash === "system") return false;
    const timestamp = parseTimestamp(event.timestamp, "diagnostic event timestamp");
    if (timestamp <= lowerBound || timestamp >= upperBound) return false;
    return event.eventKind === "candidateClick" || !NON_KEY_EVENT_KINDS.has(event.eventKind);
  });
}

function validateBreaks(breaks, startedMilliseconds, declaredMilliseconds, events = []) {
  if (!Array.isArray(breaks)) return { failures: ["attestation breaks must be an array"], maximumMinutes: 0 };
  const failures = [];
  const intervals = [];
  for (const [index, entry] of breaks.entries()) {
    if (!hasExactKeys(entry, BREAK_KEYS)) {
      failures.push(`break ${index + 1} contains unsupported fields`);
      continue;
    }
    try {
      const start = parseTimestamp(entry.startedAt, `break ${index + 1} start`);
      const end = parseTimestamp(entry.endedAt, `break ${index + 1} end`);
      if (end < start) failures.push(`break ${index + 1} ends before it starts`);
      if (start < startedMilliseconds || end > declaredMilliseconds) {
        failures.push(`break ${index + 1} is outside the declared session`);
      }
      const minutes = (end - start) / 60_000;
      if (minutes > FINAL_THRESHOLDS.maximumBreakMinutes) {
        failures.push(`break ${index + 1} exceeds ${FINAL_THRESHOLDS.maximumBreakMinutes} minutes`);
      }
      intervals.push({ start, end, minutes });
    } catch (error) {
      failures.push(error.message);
    }
  }
  intervals.sort((left, right) => left.start - right.start);
  const merged = [];
  for (const interval of intervals) {
    const previous = merged.at(-1);
    const overlaps = previous && interval.start <= previous.end;
    const hasInteraction = previous
      && !overlaps
      && hasInteractionBetweenBreaks(events, previous.end, interval.start);
    if (previous && (overlaps || !hasInteraction)) {
      if (interval.start < previous.end) failures.push("break intervals overlap");
      previous.end = Math.max(previous.end, interval.end);
      previous.minutes = (previous.end - previous.start) / 60_000;
    } else {
      merged.push({ ...interval });
    }
  }
  for (const interval of merged) {
    if (interval.minutes > FINAL_THRESHOLDS.maximumBreakMinutes) {
      failures.push(`uninterrupted break intervals exceed ${FINAL_THRESHOLDS.maximumBreakMinutes} minutes`);
    }
  }
  return {
    failures,
    maximumMinutes: merged.length ? Math.max(...merged.map((entry) => entry.minutes)) : 0,
  };
}

function validateCheckpoints(checkpoints, expectedArtifact, expectedPointer) {
  const failures = [];
  if (!Array.isArray(checkpoints)) return ["checkpoint history is missing"];
  if (checkpoints.length !== REQUIRED_CHECKPOINT_MINUTES.length) failures.push("checkpoint history has an unexpected entry count");
  for (const [index, checkpoint] of checkpoints.entries()) {
    if (!hasExactKeys(checkpoint, CHECKPOINT_KEYS) || typeof checkpoint.report !== "object" || checkpoint.report === null) {
      failures.push(`checkpoint entry ${index + 1} schema is invalid`);
      continue;
    }
    try {
      assertReport(checkpoint.report, `checkpoint ${checkpoint.scheduledMinute} report`);
    } catch (error) {
      failures.push(error.message);
      continue;
    }
    try {
      const recorded = parseTimestamp(checkpoint.recordedAt, `checkpoint ${checkpoint.scheduledMinute} recorded time`);
      const reported = parseTimestamp(checkpoint.report.checkpointAt, `checkpoint ${checkpoint.scheduledMinute} report time`);
      const started = parseTimestamp(checkpoint.report.startedAt, `checkpoint ${checkpoint.scheduledMinute} session start`);
      if (recorded !== reported) failures.push(`checkpoint ${checkpoint.scheduledMinute} recorded time does not match its report`);
      const derivedElapsed = (recorded - started) / 60_000;
      if (Math.abs(derivedElapsed - Number(checkpoint.report.elapsedMinutes)) > 1e-6) {
        failures.push(`checkpoint ${checkpoint.scheduledMinute} elapsed time is inconsistent`);
      }
    } catch (error) {
      failures.push(error.message);
    }
  }
  const byMinute = new Map(checkpoints.map((entry) => [entry.scheduledMinute, entry]));
  if (byMinute.size !== checkpoints.length) failures.push("checkpoint history contains duplicate minutes");
  for (const scheduledMinute of REQUIRED_CHECKPOINT_MINUTES) {
    const checkpoint = byMinute.get(scheduledMinute);
    if (!checkpoint) {
      failures.push(`checkpoint ${scheduledMinute} is missing`);
      continue;
    }
    const elapsed = Number(checkpoint.report?.elapsedMinutes);
    const maximum = scheduledMinute === 0 ? 1 : scheduledMinute + FINAL_THRESHOLDS.maximumBreakMinutes;
    if (!Number.isFinite(elapsed) || elapsed < scheduledMinute || elapsed > maximum) {
      failures.push(`checkpoint ${scheduledMinute} was not recorded in its allowed window`);
    }
    if (checkpoint.report?.containsTextFields !== false) {
      failures.push(`checkpoint ${scheduledMinute} failed the no-text privacy gate`);
    }
    failures.push(...artifactMismatchFailures(
      checkpoint.report?.installedArtifact,
      expectedArtifact,
      `checkpoint ${scheduledMinute} installed input method`,
    ));
    failures.push(...pointerMismatchFailures(
      checkpoint.report?.releasePointer,
      expectedPointer,
      `checkpoint ${scheduledMinute} frozen RC pointer`,
    ));
  }
  const ordered = REQUIRED_CHECKPOINT_MINUTES.map((minute) => byMinute.get(minute)).filter(Boolean);
  for (let index = 1; index < ordered.length; index += 1) {
    const currentRecorded = Date.parse(ordered[index].recordedAt);
    const previousRecorded = Date.parse(ordered[index - 1].recordedAt);
    if (Number.isFinite(currentRecorded) && Number.isFinite(previousRecorded) && currentRecorded <= previousRecorded) {
      failures.push("checkpoint recorded times are not strictly increasing");
    }
    for (const field of ["elapsedMinutes", "keyEvents", "commits", "appSwitches"]) {
      if (ordered[index].report[field] < ordered[index - 1].report[field]) {
        failures.push(`checkpoint ${field} counters are not monotonic`);
      }
    }
  }
  return failures;
}

function validateFinalInternal({
  state,
  report,
  attestation,
  events = [],
  now = new Date(),
  acceptanceToken = null,
}) {
  const failures = [];
  try {
    assertReport(report, "final dogfood report");
  } catch (error) {
    failures.push(error.message);
  }
  if (state.formalMode !== true || state.testOnly !== false
    || report.formalMode !== true || report.testOnly !== false) {
    failures.push("test-only dogfood output cannot satisfy formal acceptance");
  }
  if (acceptanceToken !== FORMAL_INVOCATION) {
    failures.push("imported final validation cannot satisfy formal acceptance");
  }
  try {
    assertProvenance(state, state.releasePointer, "dogfood state");
    requireCurrentProvenance(report, state, "final dogfood provenance");
  } catch (error) {
    failures.push(error.message);
  }
  failures.push(...artifactMismatchFailures(state.installedArtifact, state.installedArtifact, "dogfood state installed input method"));
  failures.push(...artifactMismatchFailures(report.installedArtifact, state.installedArtifact, "final installed input method"));
  failures.push(...pointerMismatchFailures(state.releasePointer, state.releasePointer, "dogfood state frozen RC pointer"));
  failures.push(...pointerMismatchFailures(report.releasePointer, state.releasePointer, "final frozen RC pointer"));
  if (!hasExactKeys(attestation, ATTESTATION_KEYS)) {
    failures.push("attestation contains unsupported fields; free-form text and identity fields are forbidden");
  }
  if (containsTextFields(attestation)) failures.push("attestation contains a forbidden text field");
  if (attestation?.formatVersion !== 2) failures.push("attestation formatVersion must be 2");
  if (attestation?.sessionId !== state.sessionId) failures.push("attestation session ID mismatch");
  if (attestation?.artifactChecksum !== state.artifactChecksum) failures.push("attestation artifact checksum mismatch");
  if (attestation?.operatorDeclaration !== OPERATOR_DECLARATION) failures.push("human operator declaration is missing");
  if (attestation?.humanOperated !== true) failures.push("humanOperated must be true");
  if (attestation?.automatedReplay !== false) failures.push("automated replay cannot satisfy dogfood");
  if (attestation?.syntheticInput !== false) failures.push("synthetic input cannot satisfy dogfood");
  if (attestation?.inputMethodCrash !== false) failures.push("inputMethodCrash must be false");
  if (attestation?.hang !== false) failures.push("hang must be false");
  if (attestation?.restartUsedToMaskFailure !== false) failures.push("restartUsedToMaskFailure must be false");
  if (attestation?.lostText !== false) failures.push("lostText must be false");
  if (attestation?.duplicateCommit !== false) failures.push("duplicateCommit must be false");
  if (attestation?.crossSessionState !== false) failures.push("crossSessionState must be false");
  if (attestation?.secureInputLeak !== false) failures.push("secureInputLeak must be false");
  if (attestation?.p0Count !== 0) failures.push("P0 count must be zero");
  if (attestation?.openP1Count !== 0) failures.push("open P1 count must be zero");

  let declaredMilliseconds = Number.NaN;
  let maximumBreakMinutes = 0;
  try {
    declaredMilliseconds = parseTimestamp(attestation?.declaredAt, "attestation declaration");
    const startedMilliseconds = parseTimestamp(state.startedAt, "session start");
    if (declaredMilliseconds < startedMilliseconds || declaredMilliseconds > now.getTime() + 60_000) {
      failures.push("attestation declaration time is outside the session");
    }
    const finalCheckpoint = state.checkpoints?.find((entry) => entry.scheduledMinute === 60);
    if (finalCheckpoint && declaredMilliseconds < parseTimestamp(finalCheckpoint.recordedAt, "checkpoint 60 time")) {
      failures.push("attestation must be declared after checkpoint 60");
    }
    const breakResult = validateBreaks(attestation?.breaks, startedMilliseconds, declaredMilliseconds, events);
    maximumBreakMinutes = breakResult.maximumMinutes;
    failures.push(...breakResult.failures);
  } catch (error) {
    failures.push(error.message);
  }

  failures.push(...validateCheckpoints(
    state.checkpoints,
    state.installedArtifact,
    state.releasePointer,
  ));
  const finalCheckpoint = state.checkpoints?.find((entry) => entry.scheduledMinute === 60);
  if (finalCheckpoint) {
    for (const field of ["elapsedMinutes", "keyEvents", "commits", "appSwitches"]) {
      if (report[field] < finalCheckpoint.report[field]) {
        failures.push(`final ${field} regressed below checkpoint 60`);
      }
    }
  }
  if (report.elapsedMinutes < FINAL_THRESHOLDS.minimumElapsedMinutes) {
    failures.push(`elapsed time is below ${FINAL_THRESHOLDS.minimumElapsedMinutes} minutes`);
  }
  if (report.keyEvents < FINAL_THRESHOLDS.minimumKeyEvents) {
    failures.push(`key event count is below ${FINAL_THRESHOLDS.minimumKeyEvents}`);
  }
  if (report.commits < FINAL_THRESHOLDS.minimumCommits) {
    failures.push(`commit count is below ${FINAL_THRESHOLDS.minimumCommits}`);
  }
  if (report.appSwitches < FINAL_THRESHOLDS.minimumAppSwitches) {
    failures.push(`App switch count is below ${FINAL_THRESHOLDS.minimumAppSwitches}`);
  }
  const coveredApps = Array.isArray(attestation?.coveredApps) ? attestation.coveredApps : [];
  if (!Array.isArray(attestation?.coveredApps)) failures.push("coveredApps must be an array");
  const seenAppIDs = new Set();
  const seenClientHashes = new Set();
  const coveredCategories = new Set();
  for (const [index, coveredApp] of coveredApps.entries()) {
    if (!hasExactKeys(coveredApp, COVERED_APP_KEYS)) {
      failures.push(`covered App ${index + 1} contains unsupported fields`);
      continue;
    }
    const category = DOGFOOD_APP_CATEGORIES[coveredApp.appId];
    if (!category) failures.push(`covered App ${index + 1} has an unknown APP ID`);
    else coveredCategories.add(category);
    if (!/^[0-9a-f]{16}$/u.test(coveredApp.clientHash ?? "")) {
      failures.push(`covered App ${index + 1} clientHash must be 16 lowercase hex characters`);
    }
    if (DOGFOOD_APP_HASHES[coveredApp.appId] !== coveredApp.clientHash) {
      failures.push(`covered App ${coveredApp.appId ?? index + 1} clientHash does not match the frozen APP matrix`);
    }
    if (seenAppIDs.has(coveredApp.appId)) failures.push(`covered App ID ${coveredApp.appId} is duplicated`);
    if (seenClientHashes.has(coveredApp.clientHash)) failures.push("covered App clientHash is duplicated");
    seenAppIDs.add(coveredApp.appId);
    seenClientHashes.add(coveredApp.clientHash);
    if (!report.apps.includes(coveredApp.clientHash)) failures.push(`covered App ${coveredApp.appId} has no diagnostic interaction`);
    if ((report.perAppCommits[coveredApp.clientHash] ?? 0) < FINAL_THRESHOLDS.minimumCommitsPerApp) {
      failures.push(`covered App ${coveredApp.appId} has fewer than ${FINAL_THRESHOLDS.minimumCommitsPerApp} commits`);
    }
  }
  if (coveredApps.length < FINAL_THRESHOLDS.minimumApps) failures.push("fewer than 6 Apps were declared for dogfood");
  for (const category of REQUIRED_APP_CATEGORIES) {
    if (!coveredCategories.has(category)) failures.push(`dogfood App category ${category} is missing`);
  }
  if (report.containsTextFields !== false) failures.push("diagnostics contain forbidden text fields");

  return {
    ...report,
    requiredCheckpoints: REQUIRED_CHECKPOINT_MINUTES,
    thresholds: FINAL_THRESHOLDS,
    coveredApps,
    coveredCategories: [...coveredCategories].sort(),
    maximumBreakMinutes,
    operatorAttestation: {
      declaredAt: attestation?.declaredAt,
      humanOperated: attestation?.humanOperated,
      automatedReplay: attestation?.automatedReplay,
      syntheticInput: attestation?.syntheticInput,
      inputMethodCrash: attestation?.inputMethodCrash,
      hang: attestation?.hang,
      restartUsedToMaskFailure: attestation?.restartUsedToMaskFailure,
      p0Count: attestation?.p0Count,
      openP1Count: attestation?.openP1Count,
    },
    humanAttestationValidated: failures.length === 0,
    passed: failures.length === 0,
    failures,
  };
}

export function validateFinal(input) {
  return validateFinalInternal({ ...input, acceptanceToken: null });
}

function captureDiagnosticLogs(logDirectory) {
  const events = [];
  if (!existsSync(logDirectory)) return { events };
  const directoryStatus = lstatSync(logDirectory);
  if (directoryStatus.isSymbolicLink() || !directoryStatus.isDirectory() || (directoryStatus.mode & 0o077) !== 0) {
    throw new Error("diagnostic log root must be a private real directory");
  }
  const names = readdirSync(logDirectory);
  let totalBytes = 0;
  for (const name of names.sort()) {
    if (!name.endsWith(".jsonl")) continue;
    if (!/^events-[0-9]{4}-[0-9]{2}-[0-9]{2}\.jsonl$/u.test(name)) throw new Error("diagnostic log filename is invalid");
    const filePath = path.join(logDirectory, name);
    const status = lstatSync(filePath);
    if (status.isSymbolicLink() || !status.isFile() || status.nlink !== 1 || (status.mode & 0o077) !== 0) {
      throw new Error("diagnostic log must be a private regular non-symlink single-link file");
    }
    const data = readFileSync(filePath);
    totalBytes += data.length;
    if (data.length > 128 * 1_024 * 1_024 || totalBytes > 512 * 1_024 * 1_024) {
      throw new Error("diagnostic logs exceed the bounded checkpoint reader limit");
    }
    const lines = data.toString("utf8").split(/\r?\n/u);
    for (const [index, line] of lines.entries()) {
      if (!line) continue;
      let event;
      try { event = JSON.parse(line); } catch {
        throw new Error(`invalid diagnostic JSON in ${name}:${index + 1}`);
      }
      events.push(event);
    }
  }
  return { events };
}

function readState(statePath) {
  const status = lstatSync(statePath);
  if (status.isSymbolicLink() || !status.isFile() || status.nlink !== 1 || (status.mode & 0o077) !== 0) {
    throw new Error("dogfood state must be a private regular non-symlink single-link file");
  }
  if (status.size > 4 * 1_024 * 1_024) throw new Error("dogfood state is unexpectedly large");
  const state = JSON.parse(readFileSync(statePath, "utf8"));
  if (!hasExactKeys(state, STATE_KEYS) || state.formatVersion !== 6 || !Array.isArray(state.checkpoints)) {
    throw new Error("start a new formatVersion 6 dogfood session");
  }
  assertChecksum(state.artifactChecksum);
  assertInstalledArtifact(state.installedArtifact, "dogfood state installed input method");
  if (state.installedArtifact.artifactChecksum !== state.artifactChecksum) {
    throw new Error("dogfood state checksum does not match its installed input method metadata");
  }
  assertReleasePointer(state.releasePointer, "dogfood state frozen RC pointer");
  assertRuntimeAttestation(state.runtimeAttestation, "dogfood state running input method");
  assertProvenance(state, state.releasePointer, "dogfood state");
  if (state.productCommit !== state.installedArtifact.productCommit
    || state.productInputsSHA256 !== state.installedArtifact.productInputsSHA256) {
    throw new Error("dogfood state ProductCommit/ProductInputs provenance is inconsistent");
  }
  if (state.finalReportSHA256 !== null) assertChecksum(state.finalReportSHA256, "dogfood state final report checksum");
  requireArtifactMatchesPointer(state.installedArtifact, state.releasePointer);
  requireRuntimeMatchesArtifact(state.runtimeAttestation, state.installedArtifact);
  if (!/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/u.test(state.sessionId ?? "")) {
    throw new Error("dogfood session ID is invalid");
  }
  parseTimestamp(state.startedAt, "session start");
  const seenMinutes = new Set();
  let previousMinute = -1;
  for (const checkpoint of state.checkpoints) {
    if (!hasExactKeys(checkpoint, CHECKPOINT_KEYS) || typeof checkpoint.report !== "object" || checkpoint.report === null) {
      throw new Error("dogfood checkpoint state schema is invalid");
    }
    if (!REQUIRED_CHECKPOINT_MINUTES.includes(checkpoint.scheduledMinute)
      || checkpoint.scheduledMinute <= previousMinute
      || seenMinutes.has(checkpoint.scheduledMinute)) {
      throw new Error("dogfood checkpoint state order is invalid");
    }
    previousMinute = checkpoint.scheduledMinute;
    seenMinutes.add(checkpoint.scheduledMinute);
    parseTimestamp(checkpoint.recordedAt, "checkpoint recorded time");
    assertReport(checkpoint.report, `checkpoint ${checkpoint.scheduledMinute} report`);
    requireCurrentProvenance(checkpoint.report, state, `checkpoint ${checkpoint.scheduledMinute} provenance`);
    if (checkpoint.report.artifactChecksum !== state.artifactChecksum
      || checkpoint.report.containsTextFields !== false) {
      throw new Error("dogfood checkpoint state contains a failed or mismatched gate");
    }
    const artifactFailures = artifactMismatchFailures(checkpoint.report.installedArtifact, state.installedArtifact, "checkpoint installed input method");
    const pointerFailures = pointerMismatchFailures(checkpoint.report.releasePointer, state.releasePointer, "checkpoint frozen RC pointer");
    if ([...artifactFailures, ...pointerFailures].length > 0) {
      throw new Error("dogfood checkpoint state artifact provenance is invalid");
    }
  }
  const checkpointZero = state.checkpoints[0];
  if (checkpointZero?.scheduledMinute !== 0
    || checkpointZero.recordedAt !== state.startedAt
    || checkpointZero.report.startedAt !== state.startedAt) {
    throw new Error("dogfood checkpoint 0 does not match the session start");
  }
  return state;
}

function writePrivateJson(filePath, value, { privateParent = true, replaceExisting = true } = {}) {
  const parent = path.dirname(filePath);
  mkdirSync(parent, { recursive: true, mode: privateParent ? 0o700 : 0o755 });
  const parentStatus = lstatSync(parent);
  if (parentStatus.isSymbolicLink() || !parentStatus.isDirectory()) throw new Error("JSON output parent must be a real directory");
  if (privateParent) chmodSync(parent, 0o700);
  const destinationStatus = lstatIfPresent(filePath);
  if (destinationStatus) {
    if (destinationStatus.isSymbolicLink() || !destinationStatus.isFile() || destinationStatus.nlink !== 1) {
      throw new Error("JSON output must be a regular non-symlink single-link file");
    }
    if (!replaceExisting) throw new Error("JSON evidence output is immutable and already exists");
  }
  const temporaryPath = `${filePath}.tmp-${process.pid}-${randomUUID()}`;
  try {
    writeFileSync(temporaryPath, `${JSON.stringify(value, null, 2)}\n`, { mode: 0o600, flag: "wx" });
    chmodSync(temporaryPath, 0o600);
    if (replaceExisting) {
      renameSync(temporaryPath, filePath);
    } else {
      linkSync(temporaryPath, filePath);
      unlinkSync(temporaryPath);
    }
    chmodSync(filePath, 0o600);
  } catch (error) {
    const temporaryStatus = lstatIfPresent(temporaryPath);
    if (temporaryStatus && !temporaryStatus.isSymbolicLink() && temporaryStatus.isFile()) {
      unlinkSync(temporaryPath);
    }
    throw error;
  }
}

function verifyEvidenceManifest(artifactDirectory, ignoredNames = new Set()) {
  const manifestPath = path.join(artifactDirectory, "MANIFEST.sha256");
  const manifestStatus = lstatSync(manifestPath);
  if (manifestStatus.isSymbolicLink() || !manifestStatus.isFile() || manifestStatus.nlink !== 1) {
    throw new Error("formal evidence MANIFEST.sha256 must be a regular non-symlink single-link file");
  }
  if (manifestStatus.size > 16 * 1_024 * 1_024) throw new Error("formal evidence MANIFEST.sha256 is unexpectedly large");
  const manifestData = readFileSync(manifestPath);
  const entries = parseManifest(manifestData, "formal V03-06 evidence manifest");
  for (const [name, expectedSHA256] of entries) {
    const evidenceFile = readBoundedEvidenceFile(
      artifactDirectory,
      name,
      `formal V03-06 evidence ${name}`,
      512 * 1_024 * 1_024,
    );
    if (sha256(evidenceFile.data) !== expectedSHA256) {
      throw new Error(`formal V03-06 evidence ${name} does not match MANIFEST.sha256`);
    }
  }
  const actualNames = readdirSync(artifactDirectory)
    .filter((name) => name !== "MANIFEST.sha256" && !ignoredNames.has(name))
    .sort();
  const manifestNames = [...entries.keys()].sort();
  if (JSON.stringify(actualNames) !== JSON.stringify(manifestNames)) {
    throw new Error("formal V03-06 evidence manifest does not exactly cover its artifact directory");
  }
  return { manifestPath, manifestData, manifestMode: manifestStatus.mode & 0o777, entries };
}

export function publishFormalDogfoodEvidence(reportPath, report, pointerPath) {
  if (!report || typeof report !== "object" || Array.isArray(report)) {
    throw new Error("formal dogfood final report must be a JSON object");
  }
  const artifactDirectory = path.join(path.dirname(pointerPath), "V03-06", "artifacts");
  const canonicalArtifactDirectory = realpathSync(artifactDirectory);
  if (canonicalArtifactDirectory !== artifactDirectory
    || path.dirname(path.resolve(reportPath)) !== artifactDirectory
    || !/^[A-Za-z0-9][A-Za-z0-9._-]*\.json$/u.test(path.basename(reportPath))) {
    throw new Error("formal dogfood final report must be a direct V03-06 JSON artifact");
  }
  const artifactDirectoryStatus = lstatSync(artifactDirectory);
  if (artifactDirectoryStatus.isSymbolicLink() || !artifactDirectoryStatus.isDirectory()) {
    throw new Error("formal V03-06 evidence directory must be a real directory");
  }
  const lockName = ".dogfood-manifest.lock";
  const lockPath = path.join(artifactDirectory, lockName);
  try {
    mkdirSync(lockPath, { mode: 0o700 });
  } catch {
    throw new Error("another formal evidence publisher is active or requires explicit recovery");
  }

  let manifest;
  let outputPublished = false;
  let manifestPublished = false;
  let reportSHA256 = "";
  const manifestTemporaryPath = path.join(
    artifactDirectory,
    `.dogfood-manifest-${process.pid}-${randomUUID()}.tmp`,
  );
  const rollbackTemporaryPath = path.join(
    artifactDirectory,
    `.dogfood-manifest-rollback-${process.pid}-${randomUUID()}.tmp`,
  );
  const ignoredNames = new Set([lockName]);
  try {
    manifest = verifyEvidenceManifest(artifactDirectory, ignoredNames);
    if (lstatIfPresent(reportPath)) throw new Error("formal dogfood final evidence is immutable and already exists");
    writePrivateJson(reportPath, report, { privateParent: false, replaceExisting: false });
    outputPublished = true;
    reportSHA256 = sha256(readFileSync(reportPath));
    const reportStatus = lstatSync(reportPath);
    if (reportStatus.isSymbolicLink() || !reportStatus.isFile() || reportStatus.nlink !== 1) {
      throw new Error("formal dogfood final evidence is not a regular single-link file");
    }
    manifest.entries.set(path.basename(reportPath), reportSHA256);
    const manifestText = [...manifest.entries.entries()]
      .sort(([left], [right]) => left < right ? -1 : left > right ? 1 : 0)
      .map(([name, checksum]) => `${checksum}  ./${name}\n`)
      .join("");
    writeFileSync(manifestTemporaryPath, manifestText, {
      mode: manifest.manifestMode,
      flag: "wx",
    });
    chmodSync(manifestTemporaryPath, manifest.manifestMode);
    renameSync(manifestTemporaryPath, manifest.manifestPath);
    manifestPublished = true;
    verifyEvidenceManifest(artifactDirectory, ignoredNames);
    inspectCurrentFrozenReleasePointer(pointerPath);
    return { reportPath, reportSHA256, manifestPath: manifest.manifestPath };
  } catch (error) {
    let rollbackFailure = "";
    if (manifestPublished && manifest) {
      try {
        writeFileSync(rollbackTemporaryPath, manifest.manifestData, {
          mode: manifest.manifestMode,
          flag: "wx",
        });
        chmodSync(rollbackTemporaryPath, manifest.manifestMode);
        renameSync(rollbackTemporaryPath, manifest.manifestPath);
      } catch (rollbackError) {
        rollbackFailure = `; manifest rollback failed: ${rollbackError.message}`;
      }
    }
    if (outputPublished) {
      try {
        const outputStatus = lstatSync(reportPath);
        const unchanged = !outputStatus.isSymbolicLink() && outputStatus.isFile()
          && outputStatus.nlink === 1 && sha256(readFileSync(reportPath)) === reportSHA256;
        if (unchanged) unlinkSync(reportPath);
        else rollbackFailure += "; final report changed before rollback";
      } catch (rollbackError) {
        rollbackFailure += `; final report rollback failed: ${rollbackError.message}`;
      }
    }
    throw new Error(`${error.message}${rollbackFailure}`);
  } finally {
    for (const temporaryPath of [manifestTemporaryPath, rollbackTemporaryPath]) {
      const temporaryStatus = lstatIfPresent(temporaryPath);
      if (temporaryStatus && !temporaryStatus.isSymbolicLink() && temporaryStatus.isFile()) {
        unlinkSync(temporaryPath);
      }
    }
    rmdirSync(lockPath);
  }
}

function writeState(statePath, stateDirectory, state, { archiveExisting = false, now = new Date() } = {}) {
  const existing = lstatIfPresent(statePath);
  if (archiveExisting && existing) {
    if (existing.isSymbolicLink() || !existing.isFile() || existing.nlink !== 1) {
      throw new Error("existing dogfood state must be a regular non-symlink single-link file");
    }
    const archiveDirectory = path.join(stateDirectory, "archive");
    mkdirSync(archiveDirectory, { recursive: true, mode: 0o700 });
    const archiveStatus = lstatSync(archiveDirectory);
    if (archiveStatus.isSymbolicLink() || !archiveStatus.isDirectory()) throw new Error("dogfood archive must be a real directory");
    chmodSync(archiveDirectory, 0o700);
    const archivePath = path.join(archiveDirectory, `session-${now.getTime()}-${randomUUID()}.json`);
    renameSync(statePath, archivePath);
    chmodSync(archivePath, 0o600);
  }
  writePrivateJson(statePath, state);
}

function checkpointEntry(
  scheduledMinute,
  state,
  events,
  now,
  installedArtifact,
  releasePointer,
  runtimeAttestation,
) {
  const report = completeReport(
    buildReport({
      artifactChecksum: state.artifactChecksum,
      startedAt: state.startedAt,
      events,
      processInstanceID: runtimeAttestation.generation,
      now,
    }),
    installedArtifact,
    releasePointer,
    runtimeAttestation,
    provenanceFields(state),
  );
  assertReport(report, `checkpoint ${scheduledMinute} report`);
  const maximumMinute = scheduledMinute === 0 ? 1 : scheduledMinute + FINAL_THRESHOLDS.maximumBreakMinutes;
  if (report.elapsedMinutes < scheduledMinute || report.elapsedMinutes > maximumMinute) {
    throw new Error(`checkpoint ${scheduledMinute} must be recorded between minute ${scheduledMinute} and ${maximumMinute}`);
  }
  if (report.containsTextFields !== false) throw new Error("diagnostic privacy gate invalidates the dogfood round");
  return { scheduledMinute, recordedAt: now.toISOString(), report };
}

function nextCheckpointMinute(state, elapsedMinutes) {
  const recorded = new Set(state.checkpoints.map((entry) => entry.scheduledMinute));
  return [...REQUIRED_CHECKPOINT_MINUTES].reverse().find((minute) =>
    !recorded.has(minute)
      && elapsedMinutes >= minute
      && elapsedMinutes <= (minute === 0 ? 1 : minute + FINAL_THRESHOLDS.maximumBreakMinutes));
}

export function runCommand(argv = process.argv, environment = process.env, now = new Date(), dependencies = {}) {
  const formalMode = dependencies.formalInvocation === FORMAL_INVOCATION;
  if (formalMode) {
    const unsupported = Object.keys(dependencies).filter((key) => key !== "formalInvocation");
    if (unsupported.length > 0) throw new Error("formal dogfood CLI does not allow injected dependencies");
  } else if (dependencies.testOnly !== true) {
    throw new Error("imported dogfood execution is test-only and must explicitly declare testOnly");
  }
  const command = argv[2] ?? "report";
  const paths = formalMode ? resolvePaths() : dependencies.paths;
  if (!paths) throw new Error("test-only dogfood execution requires injected paths");
  const artifactInspector = dependencies.inspectInstalledArtifact ?? inspectInstalledArtifact;
  const startPointerInspector = dependencies.inspectFrozenReleasePointer ?? inspectFrozenReleasePointer;
  const currentPointerInspector = dependencies.inspectCurrentFrozenReleasePointer
    ?? dependencies.inspectFrozenReleasePointer
    ?? inspectCurrentFrozenReleasePointer;
  const runtimeInspector = dependencies.inspectRunningInputMethod ?? inspectRunningInputMethod;

  if (command === "start") {
    const artifactChecksum = argv[3] ?? "";
    assertChecksum(artifactChecksum);
    const releasePointer = startPointerInspector(paths.releasePointer);
    assertReleasePointer(releasePointer);
    const provenance = provenanceForPointer(releasePointer, formalMode);
    if (releasePointer.inputArtifactChecksum !== artifactChecksum) {
      throw new Error("requested dogfood checksum does not match the frozen RC pointer");
    }
    const installedArtifact = artifactInspector(paths.inputMethodBundle);
    assertInstalledArtifact(installedArtifact);
    if (installedArtifact.artifactChecksum !== artifactChecksum) {
      throw new Error("requested dogfood checksum does not match the installed input method");
    }
    requireArtifactMatchesPointer(installedArtifact, releasePointer);
    const runtimeAttestation = runtimeInspector(paths.runtimeMarker);
    assertRuntimeAttestation(runtimeAttestation);
    requireRuntimeMatchesArtifact(runtimeAttestation, installedArtifact);
    if (parseTimestamp(runtimeAttestation.launchedAt, "running input method launch time") > now.getTime() + 1_000) {
      throw new Error("running input method launch time is after the dogfood start");
    }
    const startedAt = now.toISOString();
    let currentStateStatus = lstatIfPresent(paths.statePath);
    if (currentStateStatus && (currentStateStatus.isSymbolicLink() || !currentStateStatus.isFile())) {
      throw new Error("existing dogfood state must be a regular non-symlink file");
    }
    const initialReport = completeReport(
      buildReport({
        artifactChecksum,
        startedAt,
        events: [],
        processInstanceID: runtimeAttestation.generation,
        now,
      }),
      installedArtifact,
      releasePointer,
      runtimeAttestation,
      provenance,
    );
    assertReport(initialReport, "checkpoint 0 report");
    const state = {
      formatVersion: 6,
      sessionId: randomUUID(),
      artifactChecksum,
      installedArtifact,
      releasePointer,
      runtimeAttestation,
      startedAt,
      checkpoints: [{ scheduledMinute: 0, recordedAt: startedAt, report: initialReport }],
      ...provenance,
      finalReportSHA256: null,
    };
    writeState(
      paths.statePath,
      paths.stateDirectory,
      state,
      { archiveExisting: Boolean(currentStateStatus), now },
    );
    return {
      output: {
        sessionId: state.sessionId,
        artifactChecksum: state.artifactChecksum,
        ...state.checkpoints[0],
        formalMode: state.formalMode,
        testOnly: state.testOnly,
        validatorSHA256: state.validatorSHA256,
        productCommit: state.productCommit,
        productInputsSHA256: state.productInputsSHA256,
        pointerReportSetSHA256: state.pointerReportSetSHA256,
      },
      exitCode: 0,
    };
  }

  const state = readState(paths.statePath);
  const releasePointer = currentPointerInspector(paths.releasePointer);
  const pointerFailures = pointerMismatchFailures(releasePointer, state.releasePointer, "frozen RC pointer");
  if (pointerFailures.length > 0) throw new Error(pointerFailures.join("; "));
  const provenance = provenanceForPointer(releasePointer, formalMode);
  requireCurrentProvenance(provenance, state);
  const installedArtifact = artifactInspector(paths.inputMethodBundle);
  requireFrozenArtifact(installedArtifact, state.installedArtifact);
  requireArtifactMatchesPointer(installedArtifact, releasePointer);
  const runtimeAttestation = runtimeInspector(paths.runtimeMarker);
  assertRuntimeAttestation(runtimeAttestation);
  requireRuntimeMatchesArtifact(runtimeAttestation, installedArtifact);
  const diagnosticCapture = captureDiagnosticLogs(paths.logDirectory);
  const events = diagnosticCapture.events;
  if (command === "report") {
    const report = completeReport(
      buildReport({
        artifactChecksum: state.artifactChecksum,
        startedAt: state.startedAt,
        events,
        processInstanceID: runtimeAttestation.generation,
        now,
      }),
      installedArtifact,
      releasePointer,
      runtimeAttestation,
      provenance,
    );
    assertReport(report, "dogfood report");
    return {
      output: report,
      exitCode: 0,
    };
  }
  if (command === "checkpoint") {
    const explicitMinute = argv[3] === undefined ? undefined : Number(argv[3]);
    const elapsedMinutes = (now.getTime() - parseTimestamp(state.startedAt, "session start")) / 60_000;
    const scheduledMinute = explicitMinute ?? nextCheckpointMinute(state, elapsedMinutes);
    if (!REQUIRED_CHECKPOINT_MINUTES.includes(scheduledMinute)) {
      throw new Error("checkpoint minute must be one of 0, 30, 60 and within its allowed window");
    }
    if (state.checkpoints.some((entry) => entry.scheduledMinute === scheduledMinute)) {
      throw new Error(`checkpoint ${scheduledMinute} is already recorded`);
    }
    const entry = checkpointEntry(
      scheduledMinute,
      state,
      events,
      now,
      installedArtifact,
      releasePointer,
      runtimeAttestation,
    );
    state.checkpoints.push(entry);
    state.checkpoints.sort((left, right) => left.scheduledMinute - right.scheduledMinute);
    state.finalReportSHA256 = null;
    writeState(paths.statePath, paths.stateDirectory, state);
    return {
      output: {
        ...entry,
        formalMode: state.formalMode,
        testOnly: state.testOnly,
        validatorSHA256: state.validatorSHA256,
        productCommit: state.productCommit,
        productInputsSHA256: state.productInputsSHA256,
        pointerReportSetSHA256: state.pointerReportSetSHA256,
      },
      exitCode: 0,
    };
  }
  if (command === "finalize") {
    const attestationPath = argv[3];
    if (!attestationPath) throw new Error("finalize requires an operator attestation JSON path");
    if (formalMode && !argv[4]) {
      throw new Error("formal finalize requires an immutable V03-06 evidence output path");
    }
    const attestationStatus = lstatSync(attestationPath);
    if (attestationStatus.isSymbolicLink() || !attestationStatus.isFile()
      || attestationStatus.nlink !== 1 || attestationStatus.size > 64 * 1_024) {
      throw new Error("operator attestation must be a bounded regular non-symlink single-link file");
    }
    const attestationData = readFileSync(attestationPath);
    const attestationStatusAfterRead = lstatSync(attestationPath);
    if (attestationStatus.dev !== attestationStatusAfterRead.dev
      || attestationStatus.ino !== attestationStatusAfterRead.ino
      || attestationStatus.size !== attestationStatusAfterRead.size
      || attestationStatus.mtimeMs !== attestationStatusAfterRead.mtimeMs
      || attestationStatusAfterRead.nlink !== 1) {
      throw new Error("operator attestation changed while it was being read");
    }
    const attestation = JSON.parse(attestationData.toString("utf8"));
    const report = completeReport(
      buildReport({
        artifactChecksum: state.artifactChecksum,
        startedAt: state.startedAt,
        events,
        processInstanceID: runtimeAttestation.generation,
        now,
      }),
      installedArtifact,
      releasePointer,
      runtimeAttestation,
      provenance,
    );
    assertReport(report, "final dogfood report");
    const finalReportBody = validateFinalInternal({
      state,
      report,
      attestation,
      events,
      now,
      acceptanceToken: formalMode ? FORMAL_INVOCATION : null,
    });
    const finalReportSHA256 = sha256(JSON.stringify(finalReportBody));
    state.finalReportSHA256 = finalReportSHA256;
    writeState(paths.statePath, paths.stateDirectory, state);
    const finalReport = {
      ...finalReportBody,
      finalReportSHA256,
    };
    if (argv[4]) {
      const finalReportPath = path.resolve(argv[4]);
      if (formalMode) publishFormalDogfoodEvidence(finalReportPath, finalReport, paths.releasePointer);
      else writePrivateJson(finalReportPath, finalReport, { privateParent: false, replaceExisting: false });
    }
    return { output: finalReport, exitCode: finalReport.passed ? 0 : 3 };
  }
  throw new Error("usage: dogfood_checkpoint.mjs start <checksum> | report | checkpoint [0|30|60] | finalize <attestation.json> [report.json]");
}

function main() {
  try {
    if (process.execArgv.length !== 0) {
      throw new Error("Node execution options are forbidden in the formal CLI");
    }
    if (process.env.HOME !== undefined && path.resolve(process.env.HOME) !== path.resolve(os.userInfo().homedir)) {
      throw new Error("HOME override is forbidden in the formal CLI");
    }
    for (const forbidden of [
      "AIM_DOGFOOD_STATE_DIR",
      "AIM_DIAGNOSTIC_LOG_DIR",
      "AIM_DOGFOOD_INPUT_METHOD_BUNDLE",
      "NODE_OPTIONS",
      "NODE_PATH",
      "DYLD_INSERT_LIBRARIES",
      "DYLD_LIBRARY_PATH",
      "DYLD_FRAMEWORK_PATH",
    ]) {
      if (process.env[forbidden] !== undefined) {
        throw new Error(`${forbidden} is forbidden in the formal CLI environment`);
      }
    }
    const result = runCommand(process.argv, process.env, new Date(), { formalInvocation: FORMAL_INVOCATION });
    console.log(JSON.stringify(result.output, null, 2));
    process.exitCode = result.exitCode;
  } catch (error) {
    console.error(`dogfood checkpoint failed: ${error.message}`);
    process.exitCode = 2;
  }
}

const invokedPath = process.argv[1] ? pathToFileURL(path.resolve(process.argv[1])).href : "";
if (import.meta.url === invokedPath) main();
