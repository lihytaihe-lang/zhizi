import { createHash } from "node:crypto";
import { readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(fileURLToPath(new URL("..", import.meta.url)));
// The vocabulary the decoders outside Rime use (whole-sentence compositions, slip
// repairs, heavy garble): the pinned base dictionary above a weight floor, plus the
// product's own everyday layers in full, so words added to those layers (owner
// 2026-10-10: 捞手机 could not be composed) reach these decoders too. Domain packs
// (idioms, THUOCL IT/finance) stay with Rime only.
const sources = [
  { file: ".build-deps/rime-pinyin-simp/0c6861ef7420ee780270ca6d993d18d4101049d0/pinyin_simp.dict.yaml", minimumWeight: 500 },
  { file: "native/resources/Rime/agent_general.dict.yaml", minimumWeight: 0 },
  { file: "native/resources/Rime/agent_newwords.dict.yaml", minimumWeight: 0 },
  { file: "native/resources/Rime/agent_spoken.dict.yaml", minimumWeight: 0 },
  { file: "native/resources/Rime/agent_ai_names.dict.yaml", minimumWeight: 0 },
  { file: "native/resources/Rime/agent_modern.dict.yaml", minimumWeight: 0 },
  { file: "native/resources/Rime/agent_daily.dict.yaml", minimumWeight: 0 },
  { file: "native/resources/Rime/agent_common_fix.dict.yaml", minimumWeight: 0 },
];
const output = path.join(
  root,
  "native/core/agent-ime-core/resources/heavy_garble_words.tsv",
);
const check = process.argv.includes("--check");
const maximumCharacters = 7;

const digest = createHash("sha256");
const best = new Map();
for (const { file, minimumWeight } of sources) {
  const sourceBytes = await readFile(path.join(root, file));
  digest.update(file).update(sourceBytes);
  for (const line of sourceBytes.toString("utf8").split(/\r?\n/u)) {
    const [text, reading, rawWeight] = line.split("\t");
    const weight = Number(rawWeight);
    if (!text || !reading || !Number.isSafeInteger(weight) || weight < minimumWeight) continue;
    if (!new RegExp(`^[\\u3400-\\u9fff\\uf900-\\ufaff]{1,${maximumCharacters}}$`, "u").test(text)) continue;
    const syllables = reading.trim().toLowerCase().split(/\s+/u);
    if (syllables.length !== [...text].length
        || syllables.some((value) => !/^[a-z]+$/u.test(value))) continue;
    const key = `${text}\t${syllables.join(" ")}`;
    if ((best.get(key) ?? -1) < weight) best.set(key, weight);
  }
}
const sourceSha = digest.digest("hex");
const rows = [...best].map(([key, weight]) => `${key}\t${weight}`);
const document = [
  `# Generated from pinned pinyin_simp (weight >= 500) and the agent everyday layers; sources_sha256=${sourceSha}`,
  `# maximum_characters=${maximumCharacters}`,
  ...rows,
  "",
].join("\n");

if (check) {
  if (await readFile(output, "utf8") !== document) {
    throw new Error("heavy_garble_words.tsv is stale");
  }
} else {
  await writeFile(output, document, { mode: 0o644 });
}
console.log(`heavy_garble_words=PASS rows=${rows.length} source_sha256=${sourceSha}`);
