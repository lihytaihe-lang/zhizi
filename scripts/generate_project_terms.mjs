import { createHash } from "node:crypto";
import { readdir, readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(fileURLToPath(new URL("..", import.meta.url)));
const outputPath = path.join(root, "native/resources/Rime/agent_project_terms.dict.yaml");
const provenancePath = path.join(root, "quality/corpus/agent-project-terms-provenance.tsv");
const abbreviationPath = path.join(root, "native/core/agent-ime-core/resources/project_abbreviations.tsv");
const check = process.argv.includes("--check");
const limit = 300;
const required = [
  "ProductInputs",
  "librime",
  "entitlement",
  "AgentInputMethod",
  "ProductCommit",
  "Developer",
  "Key",
];
const requiredSet = new Set(required.map((term) => term.toLowerCase()));

const roots = [
  "native/core/agent-ime-core/src",
  "src-tauri/src",
  "script",
].map((relative) => path.join(root, relative));
const explicitDocs = [
  "docs/current/PRODUCT_SPEC.md",
  "docs/current/TECHNICAL_SPEC.md",
  "docs/current/EVIDENCE_PROTOCOL.md",
  "docs/current/ADR_PLAN.md",
  "docs/current/PROJECT_SPEC_AUDIT_2026-07-15.md",
].map((relative) => path.join(root, relative));
const adrRoot = path.join(root, "docs/current/adr");
const currentDocsRoot = path.join(root, "docs/current");
// Credential implementation identifiers and generated catalog mirrors are not
// owner-facing project vocabulary. Including them would make an internal refactor
// or a data-only skin addition alter the frozen IME candidate surface.
const internalSourcePaths = new Set([
  "src-tauri/src/keychain.rs",
  "src-tauri/src/skin_catalog.rs",
  "src-tauri/src/skin_packages.rs",
  "src-tauri/src/context_model.rs",
]);
// This page records the generator's own before/after terms and checksums.
// Counting it would make the evidence mutate the surface it is proving.
const selfReferentialEvidencePaths = new Set([
  "docs/current/evidence/PROJECT-TERM-SELF-INGESTION.md",
]);
const stopwords = new Set([
  "about", "after", "allow", "before", "between", "build", "candidate", "change", "check",
  "const", "continue", "current", "default", "error", "expected", "false", "function", "input",
  "local", "return", "script", "source", "string", "tests", "true", "value", "version",
]);

// Project vocabulary is an owner-facing input surface, not an index of every
// identifier present in source. Keep policy rules broad enough to reject new
// instances of the same maintenance noise without growing a name-by-name list.
const excludedByPolicy = new Map();
function recordExclusion(category, term) {
  const values = excludedByPolicy.get(category) ?? new Map();
  values.set(term.toLowerCase(), term);
  excludedByPolicy.set(category, values);
}
function exclusionCategory(term) {
  // C1 inference plumbing is not owner-authored project vocabulary. Keeping
  // these private ABI/trait identifiers out preserves the frozen reading set.
  if ([
    "context_group_mask", "reorder_context_candidates",
    "composition_lead", "composition_offer", "offer_composition", "model_first_order", "group_order",
    "owner_keeps", "load_owner_style",
  ].includes(term.toLowerCase())) {
    return "context-model-implementation";
  }
  if (/^build\d+$/iu.test(term)) return "build-number";
  if (/^v?\d+\.\d+(?:\.\d+)?(?:-build\d+)?$/iu.test(term)) return "version-number";
  if (/(?:^|[_-])version$/iu.test(term)) return "metadata-version-key";
  if (/^[a-z]{1,3}\d{1,3}$/iu.test(term)) return "letter-serial-number";
  if (/^aimsnap\d*$/iu.test(term)
      || /^(?:test|mock|stub|fake)[_-].+/iu.test(term)
      || /(?:fixture|harness)/iu.test(term)) return "test-fixture-or-harness";
  return null;
}
function hasProjectTermShape(term) {
  return /[a-z][A-Z]|[A-Z]{2}|[\d_+-]/u.test(term);
}
function rustInternalTypeDeclarations(contents) {
  return [...contents.matchAll(
    /\b(?:pub(?:\([^)]*\))?\s+)?(?:struct|enum|type|trait)\s+([A-Za-z][A-Za-z0-9_]{2,63})/gu,
  )].map((match) => match[1]);
}

for (const [fixture, expected] of [
  ["build1050", "build-number"],
  ["0.3.0-build1050", "version-number"],
  ["v0.4.0", "version-number"],
  ["derived-or-vendored-version", "metadata-version-key"],
  ["hr9", "letter-serial-number"],
  ["sha256", "letter-serial-number"],
  ["AIMSNAP2", "test-fixture-or-harness"],
  ["AgentIMEBehaviorHarness", "test-fixture-or-harness"],
]) {
  if (exclusionCategory(fixture) !== expected) {
    throw new Error(`project-term exclusion policy does not classify ${fixture} as ${expected}`);
  }
}
if (exclusionCategory("base64") !== null || exclusionCategory("ProductInputs") !== null) {
  throw new Error("project-term exclusion policy rejected an owner-facing term");
}
if (!hasProjectTermShape("AgentInputMethod") || !hasProjectTermShape("FTS5")
    || hasProjectTermShape("ordinary")) {
  throw new Error("project-term shape policy no longer separates product tokens from Markdown prose");
}
const rustTypeFixture = "struct LocalState {} pub enum FutureInternalType {} pub(crate) trait LocalApi {}";
if (rustInternalTypeDeclarations(rustTypeFixture).join(",")
    !== "LocalState,FutureInternalType,LocalApi") {
  throw new Error("Rust internal type declaration policy no longer covers struct/enum/trait generically");
}

async function collectFiles(directory) {
  const entries = await readdir(directory, { withFileTypes: true });
  const files = [];
  for (const entry of entries.sort((left, right) => left.name.localeCompare(right.name, "en"))) {
    const absolute = path.join(directory, entry.name);
    if (entry.isDirectory()) files.push(...await collectFiles(absolute));
    else if (entry.isFile()) files.push(absolute);
  }
  return files;
}

function eligibleFile(file) {
  if (file.startsWith(path.join(root, "script") + path.sep)) return file.endsWith(".sh");
  return /\.(?:md|rs)$/u.test(file);
}

const files = [
  ...explicitDocs,
  ...(await collectFiles(adrRoot)).filter(eligibleFile),
  ...(await Promise.all(roots.map(collectFiles))).flat().filter(eligibleFile),
].filter((file) => !internalSourcePaths.has(path.relative(root, file)))
  .sort((left, right) => left.localeCompare(right, "en"));
if (files.some((file) => internalSourcePaths.has(path.relative(root, file)))) {
  throw new Error("credential implementation entered owner-facing project vocabulary");
}
const referenceFiles = [...new Set([
  ...files,
  ...(await collectFiles(currentDocsRoot)).filter((file) => file.endsWith(".md")),
])].filter((file) => !selfReferentialEvidencePaths.has(path.relative(root, file)))
  .sort((left, right) => left.localeCompare(right, "en"));

const terms = new Map();
function record(term, source, bonus, ownerFacing = false) {
  const excluded = exclusionCategory(term);
  if (excluded) {
    recordExclusion(excluded, term);
    return;
  }
  if (!/^[A-Za-z][A-Za-z0-9_+-]{2,63}$/u.test(term)) return;
  if (term.length > 40 || /^[0-9a-f]{6,}$/iu.test(term)) return;
  const key = term.toLowerCase();
  if (stopwords.has(key)) return;
  const current = terms.get(key) ?? {
    forms: new Map(),
    evidenceCount: 0,
    referenceCount: 0,
    ownerFacing: false,
    bonus: 0,
    sources: new Set(),
  };
  current.forms.set(term, (current.forms.get(term) ?? 0) + 1);
  current.evidenceCount += 1;
  current.bonus = Math.max(current.bonus, bonus);
  current.ownerFacing ||= ownerFacing;
  current.sources.add(source);
  terms.set(key, current);
}

for (const file of files) {
  const relative = path.relative(root, file);
  const contents = await readFile(file, "utf8");
  if (file.endsWith(".md")) {
    for (const match of contents.matchAll(/`([A-Za-z][A-Za-z0-9_+-]{2,63})`/gu)) {
      record(match[1], relative, 5000, true);
    }
  }
  if (file.endsWith(".md")) {
    for (const line of contents.split(/\r?\n/gu).filter((value) => /[\u3400-\u9fff]/u.test(value))) {
      for (const match of line.matchAll(/\b[A-Za-z][A-Za-z0-9_]{2,63}\b/gu)) {
        if (hasProjectTermShape(match[0]) || requiredSet.has(match[0].toLowerCase())) {
          record(match[0], relative, 20000, true);
        }
      }
    }
  }
  let identifierMatches;
  if (file.endsWith(".rs")) {
    // Rust structs, enums, type aliases and traits describe the implementation,
    // not owner-facing vocabulary. A type can still enter through independent
    // authoritative documentation, but its declaration never nominates it.
    for (const term of rustInternalTypeDeclarations(contents)) {
      recordExclusion("rust-internal-type-declaration", term);
    }
    identifierMatches = [
      ...contents.matchAll(/\bpub(?:\([^)]*\))?\s+(?:fn|const)\s+([A-Za-z][A-Za-z0-9_]{2,63})/gu),
      ...contents.matchAll(/"([A-Z][A-Z0-9_]{2,63})"/gu),
    ].map((match) => match[1]);
  } else {
    identifierMatches = file.endsWith(".md")
      ? []
      : file.endsWith(".sh")
      ? [path.basename(file, ".sh")]
      : [...contents.matchAll(/\b[A-Za-z][A-Za-z0-9_]{2,63}\b/gu)].map((match) => match[0]);
  }
  for (const term of identifierMatches) {
    const highSignal = /[a-z][A-Z]|[A-Z]{2}|\d/u.test(term);
    if (highSignal || term.length >= 6) {
      record(term, relative, highSignal ? 30000 : 0, file.endsWith(".md"));
    }
  }
}

// Count exact references across live source plus all current Markdown after
// candidate nomination. Historical task/evidence documents can confirm that an
// already nominated owner-facing term is genuinely common, but cannot nominate
// their own implementation symbols. The fixed 300-row surface first reserves
// owner-facing nominations, then evicts within each tier by real reference
// frequency.
for (const file of referenceFiles) {
  const relative = path.relative(root, file);
  const contents = await readFile(file, "utf8");
  for (const match of contents.matchAll(/\b[A-Za-z][A-Za-z0-9_]{2,63}\b/gu)) {
    const entry = terms.get(match[0].toLowerCase());
    if (!entry) continue;
    entry.referenceCount += 1;
    entry.sources.add(relative);
  }
}

function preferredForm(entry) {
  return [...entry.forms.entries()]
    .sort(([left, leftCount], [right, rightCount]) => rightCount - leftCount
      || left.localeCompare(right, "en"))[0][0];
}

const rankedCandidates = [...terms.entries()]
  .filter(([, entry]) => entry.referenceCount >= 2 || entry.bonus >= 5000)
  .map(([reading, entry]) => ({ reading, entry, text: preferredForm(entry) }))
  .sort((left, right) => Number(right.entry.ownerFacing) - Number(left.entry.ownerFacing)
    || right.entry.referenceCount - left.entry.referenceCount
    || right.entry.bonus - left.entry.bonus
    || right.entry.evidenceCount - left.entry.evidenceCount
    || left.reading.localeCompare(right.reading, "en"));
if (process.env.AIM_PROJECT_TERMS_AUDIT) {
  for (const requested of process.env.AIM_PROJECT_TERMS_AUDIT.split(",")) {
    const normalized = requested.trim().toLowerCase();
    const rank = rankedCandidates.findIndex(({ reading }) => reading === normalized);
    const item = rankedCandidates[rank];
    console.error(`project_term_audit term=${requested.trim()} rank=${rank + 1}`
      + ` references=${item?.entry.referenceCount ?? 0}`
      + ` ownerFacing=${item?.entry.ownerFacing ?? false}`
      + ` evidence=${item?.entry.evidenceCount ?? 0}`);
  }
}
const pinned = rankedCandidates.filter(({ text }) => requiredSet.has(text.toLowerCase()));
const selected = [
  ...pinned,
  ...rankedCandidates
    .filter(({ text }) => !requiredSet.has(text.toLowerCase()))
    .slice(0, limit - pinned.length),
].sort((left, right) => left.reading.localeCompare(right.reading, "en"));
for (const requiredTerm of required) {
  if (!selected.some(({ text }) => text === requiredTerm)) {
    const rank = rankedCandidates.findIndex(({ text }) => text === requiredTerm);
    const item = rankedCandidates[rank];
    throw new Error(`required repository term was not selected by the deterministic rules: ${requiredTerm}`
      + ` rank=${rank + 1} references=${item?.entry.referenceCount ?? 0}`
      + ` ownerFacing=${item?.entry.ownerFacing ?? false}`);
  }
}
if (selected.length > limit) throw new Error(`project term limit exceeded: ${selected.length}`);

const rows = selected.map(({ text, reading, entry }) => {
  const weight = Math.min(520, 300 + Math.round(Math.log2(entry.referenceCount + 1) * 35));
  return `${text}\t${reading}\t${weight}`;
});
const dictionary = `# Generated deterministically from repository-owned ProductInputs; do not edit.\n---\nname: agent_project_terms\nversion: "0.4.0"\nsort: by_weight\n...\n\n${rows.join("\n")}\n`;
const provenanceRows = selected.map(({ text, reading, entry }) => {
  const weight = Math.min(520, 300 + Math.round(Math.log2(entry.referenceCount + 1) * 35));
  return `${text}\t${reading}\t${weight}\t${entry.referenceCount}\t${[...entry.sources].sort().join(",")}`
    + "\tproject-authored\tdeterministic-repository-frequency-v2";
});
const provenance = "text\treading\tweight\toccurrences\tsources\tlicense\textraction_rule\n"
  + `${provenanceRows.join("\n")}\n`;

function abbreviationFor(term) {
  const words = term.match(/[A-Z]+(?=[A-Z][a-z]|\d|$)|[A-Z]?[a-z]+|\d+/gu) ?? [];
  const letters = words.filter((word) => /[A-Za-z]/u.test(word)).map((word) => word[0].toLowerCase()).join("");
  return letters.length >= 2 && letters.length <= 4 ? letters : null;
}
const abbreviationCandidates = new Map();
for (const item of selected) {
  const abbreviation = abbreviationFor(item.text);
  if (!abbreviation) continue;
  const values = abbreviationCandidates.get(abbreviation) ?? [];
  values.push(item);
  abbreviationCandidates.set(abbreviation, values);
}
const abbreviations = [...abbreviationCandidates.entries()]
  .filter(([, values]) => values.length === 1)
  .map(([abbreviation, [item]]) => `${abbreviation}\t${item.text}\t${item.reading}\tagent_input_method`)
  .sort((left, right) => left.localeCompare(right, "en"));
const abbreviationData = `# abbreviation\ttext\treading\tproject_scope\n${abbreviations.join("\n")}\n`;

async function updateOrCheck(file, generated) {
  if (check) {
    if (await readFile(file, "utf8") !== generated) throw new Error(`${path.relative(root, file)} is stale`);
  } else {
    await writeFile(file, generated, { mode: 0o644 });
  }
}

await updateOrCheck(outputPath, dictionary);
await updateOrCheck(provenancePath, provenance);
await updateOrCheck(abbreviationPath, abbreviationData);
console.log(`project_terms=PASS rows=${selected.length} max=300 eviction=owner-facing-then-reference-count abbreviations=${abbreviations.length}`
  + ` dictionary_sha256=${createHash("sha256").update(dictionary).digest("hex")}`);
const exclusionSummaries = [...excludedByPolicy.entries()]
  .sort(([left], [right]) => left.localeCompare(right, "en"))
  .map(([category, values]) => {
    const samples = [...values.values()].sort((left, right) => left.localeCompare(right, "en")).slice(0, 8);
    return `${category}=${values.size} samples=${samples.join(",")}`;
  });
const excludedTotal = [...excludedByPolicy.values()].reduce((total, values) => total + values.size, 0);
console.log(`project_term_exclusions=PASS total=${excludedTotal} ${exclusionSummaries.join("; ")}`);
