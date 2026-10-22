import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  chmodSync,
  mkdtempSync,
  mkdirSync,
  readFileSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";

const RELEASE_VERSION = readFileSync(new URL("../VERSION", import.meta.url), "utf8").trim();
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import {
  DOGFOOD_APP_HASHES,
  FINAL_THRESHOLDS,
  OPERATOR_DECLARATION,
  REQUIRED_CHECKPOINT_MINUTES,
  buildReport,
  containsTextFields,
  countAppSwitches,
  runCommand,
  validateFinal,
} from "./dogfood_checkpoint.mjs";

const scriptPath = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "dogfood_checkpoint.mjs");
const source = readFileSync(scriptPath, "utf8");
const checksum = "a".repeat(64);
const executableChecksum = "e".repeat(64);
const productCommit = "b".repeat(40);
const productInputsSHA256 = "c".repeat(64);
const authoritySHA256 = "d".repeat(64);
const teamIdentifierSHA256 = createHash("sha256").update("ABCDEFGHIJ").digest("hex");
const processOne = "11111111-1111-4111-8111-111111111111";
const processTwo = "22222222-2222-4222-8222-222222222222";
const startedAt = "2026-07-15T00:00:00.000Z";
const minute30 = new Date("2026-07-15T00:30:00.000Z");
const minute60 = new Date("2026-07-15T01:00:00.000Z");
const selectedAppIDs = ["APP-01", "APP-02", "APP-03", "APP-05", "APP-06", "APP-08"];
const apps = selectedAppIDs.map((appId) => DOGFOOD_APP_HASHES[appId]);

assert.deepEqual(REQUIRED_CHECKPOINT_MINUTES, [0, 30, 60]);
assert.deepEqual(FINAL_THRESHOLDS, {
  minimumElapsedMinutes: 60,
  maximumBreakMinutes: 10,
  minimumApps: 6,
  minimumCommitsPerApp: 15,
  minimumKeyEvents: 750,
  minimumCommits: 150,
  minimumAppSwitches: 12,
});
for (const removedMechanism of [
  /state-seal/u,
  /Keychain/u,
  /kern\.boottime/u,
  /process\.hrtime/u,
  /prefixSHA256/u,
  /clockBootID/u,
]) assert.doesNotMatch(source, removedMechanism);

assert.equal(containsTextFields({ counters: { commits: 1 } }), false);
assert.equal(containsTextFields({ nested: { commitText: "forbidden" } }), true);
assert.equal(countAppSwitches([
  { clientHash: apps[0] },
  { clientHash: apps[0] },
  { clientHash: apps[1] },
  { clientHash: "system" },
  { clientHash: apps[2] },
]), 2);

const installedArtifact = Object.freeze({
  installLocation: "standard-user-input-methods",
  artifactChecksum: checksum,
  executableChecksum,
  bundleIdentifier: "com.hongye.facai.inputmethod.zhizi",
  version: RELEASE_VERSION,
  build: "1050",
  productCommit,
  productInputsSHA256,
  teamIdentifierSHA256,
  leafAuthoritySHA256: authoritySHA256,
});
const releasePointer = Object.freeze({
  pointerSHA256: "f".repeat(64),
  pointerReportSetSHA256: "7".repeat(64),
  inputArtifactChecksum: checksum,
  managerArtifactChecksum: "9".repeat(64),
  artifactVersion: RELEASE_VERSION,
  artifactBuild: "1050",
  productCommit,
  productInputsSHA256,
  teamIdentifierSHA256,
  leafAuthoritySHA256: authoritySHA256,
  runId: "20260715T000000Z-1234",
});
const runtimeOne = Object.freeze({
  formatVersion: 1,
  generation: processOne,
  processID: 4_241,
  launchedAt: "2026-07-14T23:59:00.000Z",
  bundleChecksum: checksum,
  executableChecksum,
  bundleIdentifier: "com.hongye.facai.inputmethod.zhizi",
  version: RELEASE_VERSION,
  build: "1050",
  productCommit,
  productInputsSHA256,
});
const runtimeTwo = Object.freeze({
  ...runtimeOne,
  generation: processTwo,
  processID: 4_242,
  launchedAt: "2026-07-15T00:31:00.000Z",
});

function event({ timestamp, eventKind, clientHash, processInstanceID, sequence, committed = false }) {
  return {
    timestamp,
    eventKind,
    status: 0,
    consumed: true,
    secureDegraded: false,
    coreElapsedNanoseconds: 100_000,
    totalElapsedNanoseconds: 200_000,
    candidateCount: 1,
    committed,
    selectionEventID: 0,
    clientHash,
    detailed: false,
    processInstanceID,
    sequence,
  };
}

const events = [];
const sequences = new Map([[processOne, 0], [processTwo, 0]]);
const nextSequence = (generation) => {
  const value = (sequences.get(generation) ?? 0) + 1;
  sequences.set(generation, value);
  return value;
};
for (let index = 0; index < 750; index += 1) {
  const generation = index < 400 ? processOne : processTwo;
  const timestamp = new Date(Date.parse(startedAt) + 1_000 + index * 4_700);
  if (index === 400) {
    events.push(event({
      timestamp: new Date(timestamp.getTime() - 2).toISOString(),
      eventKind: "unclean-recovery",
      clientHash: "system",
      processInstanceID: generation,
      sequence: nextSequence(generation),
    }));
  }
  if (index % 50 === 0 && index <= 600) {
    const activationIndex = index / 50;
    events.push(event({
      timestamp: new Date(timestamp.getTime() - 1).toISOString(),
      eventKind: "app-activate",
      clientHash: apps[activationIndex % apps.length],
      processInstanceID: generation,
      sequence: nextSequence(generation),
    }));
  }
  events.push(event({
    timestamp: timestamp.toISOString(),
    eventKind: "text",
    clientHash: apps[index % apps.length],
    processInstanceID: generation,
    sequence: nextSequence(generation),
    committed: index < 150,
  }));
}

const aggregate = buildReport({
  artifactChecksum: checksum,
  startedAt,
  events,
  processInstanceID: processTwo,
  now: minute60,
});
assert.equal(aggregate.keyEvents, 750);
assert.equal(aggregate.commits, 150);
assert.equal(aggregate.appSwitches, 12);
assert.equal(aggregate.detectedRecoveryEvents, 1);
assert.equal(aggregate.containsTextFields, false);
for (const app of apps) assert.equal(aggregate.perAppCommits[app], 25);
assert.throws(
  () => buildReport({
    artifactChecksum: checksum,
    startedAt,
    processInstanceID: processOne,
    now: minute30,
    events: [{ ...events[0], text: "forbidden" }],
  }),
  /schema is invalid|forbidden text/u,
);

const forbiddenOverride = spawnSync(process.execPath, [scriptPath, "report"], {
  encoding: "utf8",
  env: { ...process.env, AIM_DOGFOOD_STATE_DIR: "/tmp/forbidden" },
});
assert.equal(forbiddenOverride.status, 2);
assert.match(forbiddenOverride.stderr, /AIM_DOGFOOD_STATE_DIR is forbidden/u);
const forbiddenNodeOptions = spawnSync(process.execPath, [scriptPath, "report"], {
  encoding: "utf8",
  env: { ...process.env, NODE_OPTIONS: "" },
});
assert.equal(forbiddenNodeOptions.status, 2);
assert.match(forbiddenNodeOptions.stderr, /NODE_OPTIONS is forbidden/u);

const temporaryRoot = mkdtempSync(path.join(os.tmpdir(), "aim-dogfood-test-"));
try {
  const stateDirectory = path.join(temporaryRoot, "state");
  const logDirectory = path.join(temporaryRoot, "logs");
  mkdirSync(logDirectory, { recursive: true, mode: 0o700 });
  chmodSync(logDirectory, 0o700);
  const logPath = path.join(logDirectory, "events-2026-07-15.jsonl");
  const logText = `${events.map((entry) => JSON.stringify(entry)).join("\n")}\n`;
  writeFileSync(logPath, logText, { mode: 0o600 });
  const paths = {
    stateDirectory,
    statePath: path.join(stateDirectory, "active-session.json"),
    logDirectory,
    inputMethodBundle: path.join(temporaryRoot, "installed.inputmethod"),
    runtimeMarker: path.join(temporaryRoot, "runtime.json"),
    releasePointer: path.join(temporaryRoot, "RC_ARTIFACT.sha256"),
  };
  let currentRuntime = runtimeOne;
  let startPointerInspections = 0;
  let currentPointerInspections = 0;
  const dependencies = {
    testOnly: true,
    paths,
    inspectInstalledArtifact: () => ({ ...installedArtifact }),
    inspectFrozenReleasePointer: () => {
      startPointerInspections += 1;
      return { ...releasePointer };
    },
    inspectCurrentFrozenReleasePointer: () => {
      currentPointerInspections += 1;
      return { ...releasePointer };
    },
    inspectRunningInputMethod: () => ({ ...currentRuntime }),
  };

  assert.throws(
    () => runCommand(["node", "dogfood_checkpoint.mjs", "start", checksum], {}, new Date(startedAt)),
    /explicitly declare testOnly/u,
  );
  assert.throws(
    () => runCommand(
      ["node", "dogfood_checkpoint.mjs", "start", checksum],
      {},
      new Date(startedAt),
      { ...dependencies, inspectFrozenReleasePointer: () => ({ ...releasePointer, inputArtifactChecksum: "8".repeat(64) }) },
    ),
    /does not match the frozen RC pointer/u,
  );

  const start = runCommand(
    ["node", "dogfood_checkpoint.mjs", "start", checksum],
    {},
    new Date(startedAt),
    dependencies,
  );
  assert.equal(start.exitCode, 0);
  assert.equal(start.output.scheduledMinute, 0);
  assert.equal(start.output.formalMode, false);
  assert.equal(start.output.testOnly, true);
  assert.match(start.output.sessionId, /^[0-9a-f-]{36}$/u);
  assert.equal(statSync(paths.statePath).mode & 0o777, 0o600);
  assert.equal(startPointerInspections, 1, "the seven-report closure inspector runs once at start");

  const report30 = runCommand(["node", "dogfood_checkpoint.mjs", "report"], {}, minute30, dependencies);
  assert.ok(report30.output.keyEvents > 0);
  assert.equal(startPointerInspections, 1);
  assert.equal(currentPointerInspections, 1);
  const checkpoint30 = runCommand(
    ["node", "dogfood_checkpoint.mjs", "checkpoint", "30"],
    {},
    minute30,
    dependencies,
  );
  assert.equal(checkpoint30.output.scheduledMinute, 30);

  // Rewriting the complete log bytes is allowed: there is no historical prefix lock.
  writeFileSync(logPath, `${events.map((entry) => ` ${JSON.stringify(entry)} `).join("\n")}\n`, { mode: 0o600 });
  currentRuntime = runtimeTwo;
  const checkpoint60 = runCommand(
    ["node", "dogfood_checkpoint.mjs", "checkpoint", "60"],
    {},
    minute60,
    dependencies,
  );
  assert.equal(checkpoint60.output.scheduledMinute, 60);
  assert.equal(checkpoint60.output.report.runtimeAttestation.generation, processTwo);
  assert.equal(checkpoint60.output.report.detectedRecoveryEvents, 1);
  assert.equal(startPointerInspections, 1, "later checkpoints do not repeat seven-report closure validation");
  assert.equal(currentPointerInspections, 3);

  assert.throws(
    () => runCommand(
      ["node", "dogfood_checkpoint.mjs", "report"],
      {},
      minute60,
      { ...dependencies, inspectRunningInputMethod: () => ({ ...runtimeTwo, bundleChecksum: "8".repeat(64) }) },
    ),
    /does not match the installed frozen RC/u,
    "the live host must still match the installed frozen RC after a restart",
  );
  assert.throws(
    () => runCommand(
      ["node", "dogfood_checkpoint.mjs", "report"],
      {},
      minute60,
      { ...dependencies, inspectCurrentFrozenReleasePointer: () => ({ ...releasePointer, pointerSHA256: "8".repeat(64) }) },
    ),
    /changed after checkpoint 0/u,
    "the frozen pointer identity remains locked",
  );

  const finalNow = new Date("2026-07-15T01:00:10.000Z");
  const finalReport = runCommand(["node", "dogfood_checkpoint.mjs", "report"], {}, finalNow, dependencies).output;
  const testState = JSON.parse(readFileSync(paths.statePath, "utf8"));
  const formalize = (report) => ({ ...report, formalMode: true, testOnly: false });
  const formalState = {
    ...testState,
    formalMode: true,
    testOnly: false,
    checkpoints: testState.checkpoints.map((checkpoint) => ({
      ...checkpoint,
      report: formalize(checkpoint.report),
    })),
  };
  const formalReport = formalize(finalReport);
  const attestation = {
    formatVersion: 2,
    sessionId: formalState.sessionId,
    artifactChecksum: checksum,
    declaredAt: "2026-07-15T01:00:05.000Z",
    operatorDeclaration: OPERATOR_DECLARATION,
    humanOperated: true,
    automatedReplay: false,
    syntheticInput: false,
    inputMethodCrash: false,
    hang: false,
    restartUsedToMaskFailure: false,
    lostText: false,
    duplicateCommit: false,
    crossSessionState: false,
    secureInputLeak: false,
    p0Count: 0,
    openP1Count: 0,
    breaks: [{
      startedAt: "2026-07-15T00:31:00.000Z",
      endedAt: "2026-07-15T00:36:00.000Z",
    }],
    coveredApps: selectedAppIDs.map((appId) => ({ appId, clientHash: DOGFOOD_APP_HASHES[appId] })),
  };
  const validExceptForImportedAuthority = validateFinal({
    state: formalState,
    report: formalReport,
    attestation,
    events,
    now: finalNow,
  });
  assert.deepEqual(
    validExceptForImportedAuthority.failures,
    ["imported final validation cannot satisfy formal acceptance"],
    "60-minute thresholds, recovery metadata, human declaration, checksum and coverage gates otherwise pass",
  );

  const failureCases = [
    ["human", { humanOperated: false }, /humanOperated/u],
    ["replay", { automatedReplay: true }, /automated replay/u],
    ["synthetic", { syntheticInput: true }, /synthetic input/u],
    ["crash", { inputMethodCrash: true }, /inputMethodCrash/u],
    ["hang", { hang: true }, /hang/u],
    ["masked restart", { restartUsedToMaskFailure: true }, /restartUsedToMaskFailure/u],
    ["lost text", { lostText: true }, /lostText/u],
    ["duplicate", { duplicateCommit: true }, /duplicateCommit/u],
    ["P0", { p0Count: 1 }, /P0 count/u],
    ["P1", { openP1Count: 1 }, /open P1/u],
  ];
  for (const [name, change, expected] of failureCases) {
    const result = validateFinal({
      state: formalState,
      report: formalReport,
      attestation: { ...attestation, ...change },
      events,
      now: finalNow,
    });
    assert.ok(result.failures.some((failure) => expected.test(failure)), `${name} must fail`);
  }

  for (const [field, value, expected] of [
    ["elapsedMinutes", 59.99, /below 60 minutes/u],
    ["keyEvents", 749, /below 750/u],
    ["commits", 149, /below 150/u],
    ["appSwitches", 11, /below 12/u],
  ]) {
    const result = validateFinal({
      state: formalState,
      report: { ...formalReport, [field]: value },
      attestation,
      events,
      now: finalNow,
    });
    assert.ok(result.failures.some((failure) => expected.test(failure)), `${field} threshold must fail closed`);
  }
  const perAppBelow = {
    ...formalReport,
    perAppCommits: { ...formalReport.perAppCommits, [apps[0]]: 14 },
  };
  assert.ok(validateFinal({
    state: formalState,
    report: perAppBelow,
    attestation,
    events,
    now: finalNow,
  }).failures.some((failure) => /fewer than 15 commits/u.test(failure)));
  assert.ok(validateFinal({
    state: formalState,
    report: formalReport,
    attestation: { ...attestation, coveredApps: attestation.coveredApps.slice(0, 5) },
    events,
    now: finalNow,
  }).failures.some((failure) => /fewer than 6 Apps|instant-messaging is missing/u.test(failure)));

  const templatePath = path.resolve(
    path.dirname(fileURLToPath(import.meta.url)),
    "../docs/current/evidence/v0.2-v0.3/DOGFOOD_ATTESTATION.template.json",
  );
  const template = JSON.parse(readFileSync(templatePath, "utf8"));
  assert.equal(template.formatVersion, 2);
  assert.equal(template.declaredAt, "REPLACE_AFTER_CHECKPOINT_60_WITH_FINAL_ISO_8601_TIME");
  assert.equal(validateFinal({
    state: formalState,
    report: formalReport,
    attestation: template,
    events,
    now: finalNow,
  }).passed, false, "the untouched operator template must fail");
} finally {
  rmSync(temporaryRoot, { recursive: true, force: true });
}

console.log("dogfood checkpoint tests: PASS");
