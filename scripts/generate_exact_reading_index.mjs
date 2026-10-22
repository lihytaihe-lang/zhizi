import { readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(fileURLToPath(new URL("..", import.meta.url)));
const outputPath = path.join(root, "native/resources/Rime/agent_exact_readings.tsv");
// Words of the layers added after July that the frozen index above does not know (it keeps reproducing the ROUND-3 counterfactual
// for the owner impact evaluators). The core reads them only to tell, under an abbreviation, a dictionary word from an assembled
// one: without them 是的 (36,000) was sorted behind every word of the old layers for `sd` (2026-10-17).
const extraOutputPath = path.join(root, "native/resources/Rime/agent_extra_readings.tsv");
const factoryPriorityOutputPath = path.join(
  root,
  "native/resources/Rime/agent_factory_reading_priorities.tsv",
);
// ROUND-5 rollback: this large index remains frozen because the owner impact evaluators need to reproduce the rejected ROUND-3
// counterfactual. The core also compiles it in (mixer.rs: which candidate texts are dictionary words and how they read).
const inputPaths = [
  "native/resources/Rime/agent_pinyin.dict.yaml",
  "native/resources/Rime/agent_common_fix.dict.yaml",
  "native/resources/Rime/agent_daily.dict.yaml",
  "native/resources/Rime/agent_english.dict.yaml",
  "native/resources/Rime/agent_project_terms.dict.yaml",
  "native/resources/Rime/agent_tech_terms.dict.yaml",
  "native/resources/Rime/agent_idioms.dict.yaml",
  ".build-deps/rime-pinyin-simp/0c6861ef7420ee780270ca6d993d18d4101049d0/pinyin_simp.dict.yaml",
].map((relative) => path.join(root, relative));
const factoryPriorityInputPaths = [
  ...inputPaths,
  ...[
    "native/resources/Rime/agent_thuocl_it.dict.yaml",
    "native/resources/Rime/agent_thuocl_caijing.dict.yaml",
    "native/resources/Rime/agent_modern.dict.yaml",
    "native/resources/Rime/agent_general.dict.yaml",
    "native/resources/Rime/agent_newwords.dict.yaml",
    "native/resources/Rime/agent_spoken.dict.yaml",
    "native/resources/Rime/agent_ai_names.dict.yaml",
    "native/resources/Rime/agent_pack_poetry.dict.yaml",
    "native/resources/Rime/agent_pack_cad.dict.yaml",
    "native/resources/Rime/agent_pack_industry.dict.yaml",
  ].map((relative) => path.join(root, relative)),
];

function canonicalReading(value) {
  const syllables = value.trim().toLowerCase().split(/[\s']+/gu).filter(Boolean);
  return syllables.length > 0 && syllables.every((syllable) => /^[a-z]+$/u.test(syllable))
    ? syllables.join(" ") : null;
}

const readingsByText = new Map();
const factoryPriorityByTextAndReading = new Map();
for (const inputPath of inputPaths) {
  const contents = await readFile(inputPath, "utf8");
  for (const line of contents.split(/\r?\n/u)) {
    if (!line || line.startsWith("#") || line.startsWith("-") || line.startsWith(".")) continue;
    const [rawText, rawReading, rawWeight] = line.split("\t", 3);
    const text = rawText?.trim();
    const reading = rawReading && canonicalReading(rawReading);
    if (!text || !reading || /[\t\r\n,]/u.test(text)) continue;
    const values = readingsByText.get(text) ?? [];
    if (!values.includes(reading)) values.push(reading);
    readingsByText.set(text, values);
  }
}
for (const inputPath of factoryPriorityInputPaths) {
  const contents = await readFile(inputPath, "utf8");
  for (const line of contents.split(/\r?\n/u)) {
    if (!line || line.startsWith("#") || line.startsWith("-") || line.startsWith(".")) continue;
    const [rawText, rawReading, rawWeight] = line.split("\t", 3);
    const text = rawText?.trim();
    const reading = rawReading && canonicalReading(rawReading);
    if (!text || !reading || /[\t\r\n,]/u.test(text)) continue;
    const weight = /^\d+$/u.test(rawWeight?.trim() ?? "")
      ? Number.parseInt(rawWeight.trim(), 10)
      : 0;
    const key = `${text}\t${reading}`;
    factoryPriorityByTextAndReading.set(
      key,
      Math.max(weight, factoryPriorityByTextAndReading.get(key) ?? 0),
    );
  }
}

const extraReadingsByText = new Map();
for (const inputPath of factoryPriorityInputPaths.slice(inputPaths.length)) {
  const contents = await readFile(inputPath, "utf8");
  for (const line of contents.split(/\r?\n/u)) {
    if (!line || line.startsWith("#") || line.startsWith("-") || line.startsWith(".")) continue;
    const [rawText, rawReading] = line.split("\t", 3);
    const text = rawText?.trim();
    const reading = rawReading && canonicalReading(rawReading);
    if (!text || !reading || /[\t\r\n,]/u.test(text) || readingsByText.has(text)) continue;
    const values = extraReadingsByText.get(text) ?? [];
    if (!values.includes(reading)) values.push(reading);
    extraReadingsByText.set(text, values);
  }
}
const extraRows = [...extraReadingsByText.entries()]
  .sort(([left], [right]) => Buffer.compare(Buffer.from(left), Buffer.from(right)))
  .map(([text, readings]) => `${text}\t${readings.join(",")}`);
const extraGenerated = `${extraRows.join("\n")}\n`;
const rows = [...readingsByText.entries()]
  .sort(([left], [right]) => Buffer.compare(Buffer.from(left), Buffer.from(right)))
  .map(([text, readings]) => `${text}\t${readings.join(",")}`);
const generated = `${rows.join("\n")}\n`;
const factoryPriorityRows = [...factoryPriorityByTextAndReading.entries()]
  .sort(([left], [right]) => Buffer.compare(Buffer.from(left), Buffer.from(right)))
  .map(([key, weight]) => `${key}\t${weight}`);
const factoryPrioritiesGenerated = `${factoryPriorityRows.join("\n")}\n`;

if (process.argv.includes("--check")) {
  const [current, currentFactoryPriorities] = await Promise.all([
    readFile(outputPath, "utf8"),
    readFile(factoryPriorityOutputPath, "utf8"),
  ]);
  if (current !== generated) throw new Error("agent_exact_readings.tsv is stale; regenerate it");
  if ((await readFile(extraOutputPath, "utf8")) !== extraGenerated) throw new Error("agent_extra_readings.tsv is stale; regenerate it");
  if (currentFactoryPriorities !== factoryPrioritiesGenerated) {
    throw new Error("agent_factory_reading_priorities.tsv is stale; regenerate it");
  }
} else {
  await Promise.all([
    writeFile(outputPath, generated, { mode: 0o644 }),
    writeFile(extraOutputPath, extraGenerated, { mode: 0o644 }),
    writeFile(factoryPriorityOutputPath, factoryPrioritiesGenerated, { mode: 0o644 }),
  ]);
}

console.log(
  `exact_reading_index=PASS rows=${rows.length} bytes=${Buffer.byteLength(generated)} `
    + `extra_rows=${extraRows.length} factory_priority_rows=${factoryPriorityRows.length} `
    + `factory_priority_bytes=${Buffer.byteLength(factoryPrioritiesGenerated)}`,
);
