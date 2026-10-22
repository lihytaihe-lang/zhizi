import { createHash } from "node:crypto";
import { existsSync } from "node:fs";
import { readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(fileURLToPath(new URL("..", import.meta.url)));
// The public source tree (marked by an empty PUBLIC_BUILD file) ships these layers as header-only shells and omits their
// provenance tables; the rows are distributed through the lexicon repository, so nothing about them can be pinned here.
const publicBuild = existsSync(path.join(root, "PUBLIC_BUILD"));
const shellLayers = new Set(["aiTerms", "aiNames", "packPoetry", "packCad", "packIndustry", "packTerms"]);
const isShellAsset = (name) => shellLayers.has(name.replace(/Provenance$/u, ""));
const assets = Object.freeze({
  commonFix: ["native/resources/Rime/agent_common_fix.dict.yaml", "e8b6206c93c799e8bcb20e32efa50f94dba29a4f421fcd96fa76a3c93c8c63ef"],
  daily: ["native/resources/Rime/agent_daily.dict.yaml", "2c15d6e53b7911259b3a93e3d47d69874991ea6e02d1fedb2e77124789495b5e"],
  english: ["native/resources/Rime/agent_english.dict.yaml", "47b351cd46bf975d98a77e67d901210c39fd5ce4952f10e2c8a727b1bb3749de"],
  tech: ["native/resources/Rime/agent_tech_terms.dict.yaml", "3865bba99acdbc5b1ad535a266a79e3d15e0eb8b5f14ff08b4e8be86e830af20"],
  idioms: ["native/resources/Rime/agent_idioms.dict.yaml", "5723df93fd4b3ce1dfdb612ca2f496862b7b8c80ea380c4c338937956c9af34f"],
  thuoclIt: ["native/resources/Rime/agent_thuocl_it.dict.yaml", "f2de2e7ef773e3951b9a1b444da81341ea6a1da80aceac3840683fd03d008187"],
  thuoclCaijing: ["native/resources/Rime/agent_thuocl_caijing.dict.yaml", "4497c28c3cb8e745c2d6408ad588a79e145efe2308682bbbbf9818dde6b2d157"],
  modern: ["native/resources/Rime/agent_modern.dict.yaml", "d1e6a40abca03593a85c38da0e5da9cf8d04404fa1ccb0039027268bd7b54682"],
  general: ["native/resources/Rime/agent_general.dict.yaml", "0d41f62fae2a717da06aeb5fefd77d569fce6ae215c51ea178659e6a63fc5cec"],
  generalProvenance: ["quality/corpus/agent-general-provenance.tsv", "7d540c4fc7ccf32934149bfbea63fdc505b113b2cad08784d55874d3d3faff17"],
  newwords: ["native/resources/Rime/agent_newwords.dict.yaml", "bef0359c6a89f0675712a1b50c11603f60f46a35cf9a8c218f2419d6c7d76870"],
  newwordsProvenance: ["quality/corpus/agent-newwords-provenance.tsv", "6a94bdb6996dcb6508cabb870bd623be42c487aa90055f11747772b713eea18d"],
  spoken: ["native/resources/Rime/agent_spoken.dict.yaml", "1294b968bfb0e6e2946991c349ea1f62dc3afa388bc984963216e2a26c2f4293"],
  spokenProvenance: ["quality/corpus/agent-spoken-provenance.tsv", "8651403810bea8474e955c7d1e9e2856f1bd35a834b710b7e3c473cf710aedfb"],
  aiTerms: ["native/resources/Rime/agent_ai_terms.dict.yaml", "2e4e058bc42a1a83f4fedf1c9b2a31ed93d63f1c7558f63dcdef01caed2ec4b3"],
  aiTermsProvenance: ["quality/corpus/agent-ai-terms-provenance.tsv", "464574b1bcb3e16fd7cae28dd3ba4617dd837c6425cb4c40f0c56a2ed0cf1680"],
  aiNames: ["native/resources/Rime/agent_ai_names.dict.yaml", "c458a49954b7e2f9f1e53942f2ee5b9deb4601fb2c80b53e256a72e947af9a7f"],
  aiNamesProvenance: ["quality/corpus/agent-ai-names-provenance.tsv", "b25c5e6d77715665675635593167b1667a7c3aa85b114e8fbfa419c9a20df4ec"],
  packPoetry: ["native/resources/Rime/agent_pack_poetry.dict.yaml", "a4dde1e7b9e1379399cf89ec1fe4f6e73e71d6977f60dfab9e880a96a3f10b76"],
  packPoetryProvenance: ["quality/corpus/agent-pack-poetry-provenance.tsv", "14543a44ea3ab8a8b1143724ee4439c9f460fd5b13fdc29dfdc6373906e4bc15"],
  packCad: ["native/resources/Rime/agent_pack_cad.dict.yaml", "1348f3b39281afd125c7e43fbcff1488d2e02b2ad51ea78f18314987325a5080"],
  packCadProvenance: ["quality/corpus/agent-pack-cad-provenance.tsv", "cd4540eec556def58428274133c349781f79ddf6635edaf40497882db8742040"],
  packIndustry: ["native/resources/Rime/agent_pack_industry.dict.yaml", "b1e4849e41b67addf8a706a41fe21ef2ef811c34c1efffcd7cca1d6df3b522cf"],
  packIndustryProvenance: ["quality/corpus/agent-pack-industry-provenance.tsv", "881c28fe0a8c7f73f50a4035f34375e7255c2a990fffc78796b5f17725c68823"],
  packTerms: ["native/resources/Rime/agent_pack_terms.dict.yaml", "996b22ce36ed3ddc4c97763d52bcd77d07bf57531d27b8177407b806c949a68d"],
  packTermsProvenance: ["quality/corpus/agent-pack-terms-provenance.tsv", "2385d7e53d6bfdfeb8e3651f04f17e20153641c81a5123e96e32cf92811e4077"],
  aggregate: ["native/resources/Rime/agent_pinyin.dict.yaml", "ecbb7bfb6e0e84039e68f78c5f628e19491e2059b9df4c98303536ccef6f81ee"],
  commonFixProvenance: ["quality/corpus/agent-common-fix-provenance.tsv", "cac917eb4b33155877e016d3a1a6113b9393e3342c2b2fcc13abe6d2f67d50cf"],
  provenance: ["quality/corpus/agent-daily-provenance.tsv", "1b32e5dde5d7b9912e3bcd7a99fb55a4d380de4f0c37e872c53b9ad051121d1f"],
  englishProvenance: ["quality/corpus/agent-english-provenance.tsv", "5072ef56b73f0d8273df92fc7b5db888747b3707ee84774620b65064f7517583"],
  techProvenance: ["quality/corpus/agent-tech-terms-provenance.tsv", "2e390be331a858974506a4b721dae18b311ff686eac7146925ed6049a3d1a79c"],
  thuoclItProvenance: ["quality/corpus/thuocl-it-provenance.tsv", "a3efe498ed334276324756590b849805d45bfc695fca98c40275cf103350fcd1"],
  thuoclCaijingProvenance: ["quality/corpus/thuocl-caijing-provenance.tsv", "a86b531b1ac7ff1503708b1febe3b8ca5ca571fd2200bf30d549d7e71105be5f"],
  modernProvenance: ["quality/corpus/agent-modern-provenance.tsv", "53294a5b4819f68019a7117d66814f782009e98552f8e18d750d2c7da6ebd8fa"],
  modernSource: ["quality/corpus/agent-modern-source.tsv", "396df3e3ffc6552340bc70340e300bc49275802b52990cc3586e7239eb86d2de"],
  mixedLanguage: ["quality/corpus/mixed-language-regression-v2.json", "4c43ecd1389f5f252d0f2e56e282a841a25d2b9a44ee8cecd5aa2867833d1d9e"],
});

function assert(condition, message) {
  if (!condition) throw new Error(message);
}

function sha256(contents) {
  return createHash("sha256").update(contents).digest("hex");
}

const loaded = {};
for (const [name, [relativePath, expected]] of Object.entries(assets)) {
  if (publicBuild && isShellAsset(name)) continue;
  const contents = await readFile(path.join(root, relativePath));
  const actual = sha256(contents);
  assert(actual === expected, `${relativePath} checksum changed: ${actual}`);
  loaded[name] = contents.toString("utf8");
}

const commonFixRows = loaded.commonFix.split(/\r?\n/u)
  .filter((line) => line && !line.startsWith("#") && !line.startsWith("---")
    && line !== "..." && !line.includes(":"))
  .map((line) => line.split("\t"));
assert(JSON.stringify(commonFixRows) === JSON.stringify([
  ["完成了", "wan cheng le", "520"],
  ["难用", "nan yong", "520"],
  ["超级烦", "chao ji fan", "520"],
]), "agent_common_fix rows changed");
assert(commonFixRows.length <= 300, "agent_common_fix exceeds the frozen 300-row ceiling");
const commonFixProvenanceRows = loaded.commonFixProvenance.trimEnd().split(/\r?\n/u)
  .slice(1).map((line) => line.split("\t"));
assert(commonFixProvenanceRows.length === commonFixRows.length,
  "agent_common_fix provenance row count changed");
for (let index = 0; index < commonFixRows.length; index += 1) {
  const [text, reading, weight] = commonFixRows[index];
  const [recordedText, recordedReading, recordedWeight, frozenSource, trigger, reason]
    = commonFixProvenanceRows[index];
  assert(text === recordedText && reading === recordedReading && weight === recordedWeight,
    `agent_common_fix provenance mismatch at row ${index + 1}`);
  assert(Number(weight) <= 520 && frozenSource.length > 0 && trigger.length > 0 && reason.length > 0,
    `agent_common_fix provenance fields missing at row ${index + 1}`);
}

const dictionaryRows = loaded.daily.split(/\r?\n/u)
  .filter((line) => line && !line.startsWith("#") && !line.startsWith("---")
    && line !== "..." && !line.includes(":"))
  .map((line) => line.split("\t"));
assert(JSON.stringify(dictionaryRows) === JSON.stringify([
  ["大", "da", "114000"],
  ["关", "guan", "16000"],
  ["热爱", "re ai", "3000"],
]), "agent_daily rows changed");

const provenanceRows = loaded.provenance.trimEnd().split(/\r?\n/u).slice(1).map((line) => line.split("\t"));
assert(provenanceRows.length === dictionaryRows.length, "agent_daily provenance row count changed");
for (let index = 0; index < dictionaryRows.length; index += 1) {
  const [text, pinyin, weight] = dictionaryRows[index];
  const [recordedText, recordedPinyin, recordedWeight, source, reason] = provenanceRows[index];
  assert(text === recordedText && pinyin === recordedPinyin && weight === recordedWeight,
    `agent_daily provenance mismatch at row ${index + 1}`);
  assert(source === "project-authored" && reason.length > 0, `agent_daily source/reason missing at row ${index + 1}`);
}

const englishRows = loaded.english.split(/\r?\n/u)
  .filter((line) => line && !line.startsWith("#") && !line.startsWith("---")
    && line !== "..." && !line.includes(":"))
  .map((line) => line.split("\t"));
const expectedEnglish = ["api", "claude", "codex", "github", "python", "token"];
assert(JSON.stringify(englishRows) === JSON.stringify(
  expectedEnglish.map((term) => [term, term, "100000"]),
), "agent_english rows changed");
const englishProvenanceRows = loaded.englishProvenance.trimEnd().split(/\r?\n/u)
  .slice(1).map((line) => line.split("\t"));
assert(englishProvenanceRows.length === englishRows.length, "agent_english provenance row count changed");
for (let index = 0; index < englishRows.length; index += 1) {
  const [text, reading, weight] = englishRows[index];
  const [recordedText, recordedReading, recordedWeight, source, license, reason] = englishProvenanceRows[index];
  assert(text === recordedText && reading === recordedReading && weight === recordedWeight,
    `agent_english provenance mismatch at row ${index + 1}`);
  assert(source === "V04-DEV-PLAN Part 0.7" && license === "project-authored" && reason.length > 0,
    `agent_english source/license/reason missing at row ${index + 1}`);
}

const techRows = loaded.tech.split(/\r?\n/u)
  .filter((line) => line && !line.startsWith("#") && !line.startsWith("---")
    && line !== "..." && !line.includes(":"))
  .map((line) => line.split("\t"));
const techProvenanceRows = loaded.techProvenance.trimEnd().split(/\r?\n/u)
  .slice(1).map((line) => line.split("\t"));
assert(techRows.length === 102 && techRows.length <= 500, "agent_tech_terms size changed");
assert(techProvenanceRows.length === techRows.length, "agent_tech_terms provenance row count changed");
const techKeys = new Set();
for (let index = 0; index < techRows.length; index += 1) {
  const [text, reading, weight] = techRows[index];
  const [recordedText, recordedReading, recordedWeight, category, source, license, reason]
    = techProvenanceRows[index];
  assert(text === recordedText && reading === recordedReading && weight === recordedWeight,
    `agent_tech_terms provenance mismatch at row ${index + 1}`);
  assert(/^[a-z]+$/u.test(reading) && Number(weight) <= 520,
    `agent_tech_terms invalid reading or weight at row ${index + 1}`);
  const isLegacy = source === "V04-DEV-PLAN Chunk 11 Part C"
    && reason === "canonical spelling from the named product official documentation or product website";
  const isQ2 = source === "INPUT-QUALITY Chunk Q2"
    && (reason === "canonical spelling frozen by the Q2 task card"
      || reason === "Q2 task-card abbreviation for the canonical TypeScript display spelling");
  const isQ3B2 = source === "INPUT-QUALITY Q3-B2"
    && reason === "owner-used technical term registered by the Q3-B2 task card";
  const isMixedNarrow = source === "CN-EN-MIXED-INPUT-NARROW"
    && reason === "owner-named canonical product spelling for deterministic mixed-input lookup";
  assert(category.length > 0 && (isLegacy || isQ2 || isQ3B2 || isMixedNarrow)
    && license === "project-authored",
  `agent_tech_terms source/license/reason missing at row ${index + 1}`);
  const key = `${text}\t${reading}`;
  assert(!techKeys.has(key), `agent_tech_terms duplicate at row ${index + 1}`);
  techKeys.add(key);
}
const requiredTech = new Map([
  ["mariadb", "MariaDB"], ["github", "GitHub"], ["postgres", "PostgreSQL"],
  ["nodejs", "Node.js"], ["typescript", "TypeScript"], ["macos", "macOS"],
  ["ios", "iOS"], ["nginx", "nginx"], ["windows", "Windows"],
  ["api", "API"], ["sdk", "SDK"], ["cli", "CLI"], ["ci", "CI"], ["cd", "CD"],
  ["cursor", "Cursor"], ["copilot", "Copilot"], ["gpt", "GPT"],
  ["prompt", "prompt"], ["token", "token"], ["embedding", "embedding"],
  ["ts", "TypeScript"],
  ["npm", "npm"], ["kafka", "Kafka"], ["safari", "Safari"], ["bun", "Bun"],
  ["deno", "Deno"], ["ansible", "Ansible"], ["grafana", "Grafana"],
  ["webpack", "Webpack"], ["vite", "Vite"],
  ["macbookpro", "MacBookPro"], ["macbook", "MacBook"], ["iphone", "iPhone"],
  ["ipad", "iPad"], ["icloud", "iCloud"], ["airpods", "AirPods"],
]);
for (const [reading, text] of requiredTech) {
  assert(techRows.some((row) => row[0] === text && row[1] === reading),
    `required canonical tech term missing: ${reading}`);
}

function parseDictionaryRows(contents) {
  return contents.split(/\r?\n/u)
    .filter((line) => line && !line.startsWith("#") && !line.startsWith("---")
      && line !== "..." && !line.includes(":"))
    .map((line) => line.split("\t"));
}

const idiomRows = parseDictionaryRows(loaded.idioms);
const thuoclItRows = parseDictionaryRows(loaded.thuoclIt);
const thuoclCaijingRows = parseDictionaryRows(loaded.thuoclCaijing);
const modernRows = parseDictionaryRows(loaded.modern);
const thuoclItProvenanceRows = loaded.thuoclItProvenance.trimEnd().split(/\r?\n/u)
  .slice(1).map((line) => line.split("\t"));
const thuoclCaijingProvenanceRows = loaded.thuoclCaijingProvenance.trimEnd().split(/\r?\n/u)
  .slice(1).map((line) => line.split("\t"));
assert(thuoclItRows.length === 12212, "THUOCL IT output row count changed");
assert(thuoclCaijingRows.length === 2464, "THUOCL caijing output row count changed");
assert(thuoclItProvenanceRows.length === thuoclItRows.length,
  "THUOCL IT provenance row count changed");
assert(thuoclCaijingProvenanceRows.length === thuoclCaijingRows.length,
  "THUOCL caijing provenance row count changed");
for (const [rows, provenanceRows, source] of [
  [thuoclItRows, thuoclItProvenanceRows, "THUOCL_IT.txt"],
  [thuoclCaijingRows, thuoclCaijingProvenanceRows, "THUOCL_caijing.txt"],
]) {
  for (let index = 0; index < rows.length; index += 1) {
    const [text, reading, weight] = rows[index];
    const [recordedText, recordedReading, recordedWeight, df, recordedSource, commit, license, tool]
      = provenanceRows[index];
    assert(text === recordedText && reading === recordedReading && weight === recordedWeight,
      `${source} provenance mismatch at row ${index + 1}`);
    assert(Number(weight) >= 104 && Number(weight) <= 520 && Number(df) > 0,
      `${source} invalid weight or DF at row ${index + 1}`);
    assert(recordedSource.startsWith(`${source}:`)
      && commit === "a30ce79d895d01ab5132a5c74c29703ff7efb4cc"
      && license === "MIT" && tool === "pypinyin-0.55.0",
    `${source} provenance contract changed at row ${index + 1}`);
  }
}
for (const [reading, text] of new Map([
  ["fu lei", "父类"], ["jia suo", "加锁"], ["lei ming", "类名"], ["si you yun", "私有云"],
])) {
  assert(thuoclItRows.some((row) => row[0] === text && row[1] === reading),
    `required THUOCL IT term missing: ${text}`);
}

const modernProvenanceRows = loaded.modernProvenance.trimEnd().split(/\r?\n/u)
  .slice(1).map((line) => line.split("\t"));
assert(modernRows.length === 151 && modernRows.length <= 500, "agent_modern size changed");
assert(modernProvenanceRows.length === modernRows.length, "agent_modern provenance row count changed");
for (let index = 0; index < modernRows.length; index += 1) {
  const [text, reading, weight] = modernRows[index];
  const [recordedText, recordedReading, recordedWeight, category, source, license, reason]
    = modernProvenanceRows[index];
  assert(text === recordedText && reading === recordedReading && weight === recordedWeight,
    `agent_modern provenance mismatch at row ${index + 1}`);
  assert(/^(?:[a-z]+)(?: [a-z]+)*$/u.test(reading) && Number(weight) <= 520,
    `agent_modern invalid reading or weight at row ${index + 1}`);
  assert(category.length > 0 && source === "DICT-EXPANSION D2" && license === "project-authored"
    && reason === "human-curated modern daily written-language term",
  `agent_modern source/license/reason missing at row ${index + 1}`);
}

const mixedLanguage = JSON.parse(loaded.mixedLanguage);
assert(mixedLanguage.kind === "mixed-language-regression" && mixedLanguage.formatVersion === 2,
  "mixed-language corpus contract changed");
assert(mixedLanguage.cases.length === 8, "mixed-language corpus must contain eight cases");
assert(JSON.stringify(mixedLanguage.cases.slice(0, 6).map((item) => item.query))
  === JSON.stringify(["codex", "claude", "github", "api", "token", "python"]),
"mixed-language technical cases changed");
assert(mixedLanguage.cases.every((item) => item.mustPassTop1 === true),
  "mixed-language corpus contains a non-Top1 case");
assert(JSON.stringify(mixedLanguage.cases.slice(0, 6).map((item) => item.allowed[0]))
  === JSON.stringify(["Codex", "Claude", "GitHub", "API", "token", "Python"]),
"mixed-language canonical spelling contract changed");

const importOrder = [...loaded.aggregate.matchAll(/^  - (.+)$/gmu)].map((match) => match[1]);
assert(JSON.stringify(importOrder) === JSON.stringify([
  "agent_common_fix", "agent_daily", "agent_idioms", "agent_thuocl_it", "agent_thuocl_caijing", "agent_modern",
  "agent_general", "agent_newwords", "agent_spoken", "agent_ai_names", "agent_pack_poetry", "agent_pack_cad", "agent_pack_industry", "pinyin_simp",
]),
  "agent_pinyin import order changed");
assert(!loaded.aggregate.includes("agent_english") && !loaded.aggregate.includes("agent_project_terms")
  && !loaded.aggregate.includes("agent_tech_terms"),
"ASCII dictionaries must stay outside Rime algebra/import tables");

const upstream = await readFile(path.join(
  root,
  ".build-deps/rime-pinyin-simp/0c6861ef7420ee780270ca6d993d18d4101049d0/pinyin_simp.dict.yaml",
), "utf8");
const upstreamRows = parseDictionaryRows(upstream);
const priorTexts = new Set([...parseDictionaryRows(loaded.daily), ...idiomRows, ...upstreamRows]
  .map((row) => row[0]));
const newTexts = new Set();
for (const [source, rows] of [["IT", thuoclItRows], ["caijing", thuoclCaijingRows]]) {
  for (const [text] of rows) {
    assert(!priorTexts.has(text), `THUOCL ${source} was not deduplicated from existing text: ${text}`);
    assert(!newTexts.has(text), `THUOCL ${source} cross-table duplicate: ${text}`);
    newTexts.add(text);
  }
}
for (const [text] of modernRows) {
  assert(!priorTexts.has(text), `agent_modern was not deduplicated from existing text: ${text}`);
  assert(!newTexts.has(text), `agent_modern cross-table duplicate: ${text}`);
  newTexts.add(text);
}
const precedingRows = [...parseDictionaryRows(loaded.commonFix), ...parseDictionaryRows(loaded.daily),
  ...idiomRows, ...thuoclItRows, ...thuoclCaijingRows, ...modernRows, ...upstreamRows];
const shippedTexts = new Set(precedingRows.map((row) => row[0]));
const highestWeight = new Map();
for (const [, reading, weight] of precedingRows) {
  highestWeight.set(reading, Math.max(highestWeight.get(reading) ?? 0, Number(weight)));
}
const syllables = new Set(upstreamRows.flatMap(([, reading]) => reading.toLowerCase().split(" ")));
assert(syllables.size === 415, "pinned schema syllable inventory changed");
for (const [name, expectedCount] of [["general", 26947], ["newwords", 14633], ["spoken", 10853]]) {
  const rows = parseDictionaryRows(loaded[name]);
  const provenance = loaded[`${name}Provenance`].trimEnd().split(/\r?\n/u).slice(1).map((line) => line.split("\t"));
  assert(rows.length === expectedCount && provenance.length === rows.length, `${name} row count changed`);
  const layerTexts = new Set();
  let existingTextOverlaps = 0;
  let existingReadingCeilingViolations = 0;
  for (let index = 0; index < rows.length; index += 1) {
    const [text, reading, weight] = rows[index];
    const fields = provenance[index];
    assert(fields.slice(0, 3).join("\t") === rows[index].join("\t"), `${name} provenance mismatch at ${index + 1}`);
    assert(/^[\p{Script=Han}]+$/u.test(text) && reading.split(" ").every((s) => syllables.has(s.replace(/^([ln])ve$/u, "$1ue"))), `${name} invalid text/reading: ${text}`);
    assert(/^\d+$/u.test(weight) && Number(weight) > 0, `${name} invalid weight: ${text}`);
    // The spoken layer lists a phrase once per accepted reading (a second opinion on heteronyms).
    const layerKey = name === "spoken" ? `${text}\t${reading}` : text;
    assert(!layerTexts.has(layerKey), `${name} duplicate text within layer: ${text}`);
    layerTexts.add(layerKey);
    if (shippedTexts.has(text)) existingTextOverlaps += 1;
    if (highestWeight.has(reading) && Number(weight) >= highestWeight.get(reading)) existingReadingCeilingViolations += 1;

    if (name === "general") {
      const licenses = {jieba: "MIT (jieba 0.42.1 dict.txt, PyPI sdist sha256 055ca12f62674fafed09427f176506079bc135638a14e23e25be909131928db2)",
        "pypinyin-phrases": "MIT (pypinyin 0.55.0 phrases_dict)",
        "phrase-layer-mined": "project-authored (n-grams mined from licensed corpora, DeepSeek-reviewed)",
        "phrase-layer-generated": "project-authored (DeepSeek-generated phrases, corpus-validated)",
        "owner-missing-curated": "project-authored (words the owner's records show missing, curated by hand)"};
      assert(fields.length === 7 && licenses[fields[5]] === fields[6]
        && fields.slice(3, 5).every((v) => /^\d+$/u.test(v)), `${name} unregistered provenance: ${text}`);
    } else if (name === "spoken") {
      assert(fields.length === 7 && fields[3] === "strong" && fields.slice(4, 6).every((v) => /^\d+$/u.test(v))
        && fields[6] === "CLI-generated", `${name} unregistered provenance: ${text}`);
    } else {
      assert(fields.length === 9 && fields[7] === "deepseek-flash generation, 3-pass review" && fields[8] === "project-authored"
        && fields.slice(5, 7).every((v) => /^\d+$/u.test(v)), `${name} unregistered provenance: ${text}`);
    }
    shippedTexts.add(text);
  }
  // Imported exports are byte-pinned; report source-policy discrepancies rather than editing their entries.
  console.log(`lexicon_${name} rows=${rows.length} existing_text_overlaps=${existingTextOverlaps} preceding_reading_ceiling_violations=${existingReadingCeilingViolations}`);
  for (const [, reading, weight] of rows) highestWeight.set(reading, Math.max(highestWeight.get(reading) ?? 0, Number(weight)));
}


// AI-era names (VOCAB-COVERAGE-PLAN family B): one provenance row per entry, each with an https source page and a date.
for (const [name, ascii] of publicBuild ? [] : [["aiTerms", true], ["aiNames", false]]) {
  const rows = parseDictionaryRows(loaded[name]);
  const provenance = loaded[`${name}Provenance`].trimEnd().split(/\r?\n/u).slice(1).map((line) => line.split("\t"));
  assert(rows.length > 0 && provenance.length === rows.length, `${name} row count`);
  const seen = new Set();
  rows.forEach(([text, reading, weight], index) => {
    const fields = provenance[index];
    assert(fields.slice(0, 3).join("\t") === [text, reading, weight].join("\t"), `${name} provenance mismatch at ${index + 1}`);
    assert(fields.length === 7 && /^https?:\/\//u.test(fields[4]) && /^\d{4}-\d{2}-\d{2}$/u.test(fields[5]) && fields[6] === "facts-only (names)",
      `${name} unregistered provenance: ${text}`);
    assert(/^\d+$/u.test(weight) && Number(weight) > 0, `${name} invalid weight: ${text}`);
    if (ascii) {
      assert(/^[A-Za-z][A-Za-z .-]*[A-Za-z]$/u.test(text) && reading === text.toLowerCase().replace(/[^a-z]/gu, ""), `${name} invalid ascii term: ${text}`);
    } else {
      assert(/^[\p{Script=Han}]{2,12}$/u.test(text) && reading.split(" ").length === [...text].length, `${name} invalid name: ${text}`);
    }
    assert(!seen.has(`${text}\t${reading}`), `${name} duplicate: ${text}`);
    seen.add(`${text}\t${reading}`);
  });
  console.log(`lexicon_${name} rows=${rows.length}`);
}

// Optional vocabulary packs (poetry, CAD, industry): researched lists, one provenance row per entry with an https source page,
// a check date and a licence note; Chinese texts are Han only, the terms layer is exact ASCII spellings.
for (const [name, ascii] of publicBuild ? [] : [["packPoetry", false], ["packCad", false], ["packIndustry", false], ["packTerms", true]]) {
  const rows = parseDictionaryRows(loaded[name]);
  const provenance = loaded[`${name}Provenance`].trimEnd().split(/\r?\n/u).slice(1).map((line) => line.split("\t"));
  assert(rows.length > 0 && provenance.length === rows.length, `${name} row count`);
  const seen = new Set();
  rows.forEach(([text, reading, weight], index) => {
    const fields = provenance[index];
    assert(fields.slice(0, 3).join("\t") === [text, reading, weight].join("\t"), `${name} provenance mismatch at ${index + 1}`);
    assert(fields.length === 7 && /^https?:\/\//u.test(fields[4]) && /^\d{4}-\d{2}-\d{2}$/u.test(fields[5])
      && /(MIT|CC-BY-SA-4\.0|LGPL-2\.1|CC0|Public domain|facts)/u.test(fields[6]), `${name} unregistered provenance: ${text}`);
    assert(/^\d+$/u.test(weight) && Number(weight) > 0, `${name} invalid weight: ${text}`);
    if (ascii) {
      assert(/^[A-Za-z][A-Za-z .&\/-]*[A-Za-z]$/u.test(text) && reading === text.toLowerCase().replace(/[^a-z]/gu, ""), `${name} invalid ascii term: ${text}`);
    } else {
      assert(/^[\p{Script=Han}]{2,16}$/u.test(text) && reading.split(" ").length === [...text].length, `${name} invalid entry: ${text}`);
    }
    assert(!seen.has(`${text}\t${reading}`), `${name} duplicate: ${text}`);
    seen.add(`${text}\t${reading}`);
  });
  console.log(`lexicon_${name} rows=${rows.length}`);
}

const aggregateChineseRows = [...thuoclItRows, ...thuoclCaijingRows, ...modernRows, ...upstreamRows];
for (const [reading, text] of new Map([
  ["wei xin", "微信"], ["wei bo", "微博"], ["dou yin", "抖音"],
  ["xiao hong shu", "小红书"], ["pin duo duo", "拼多多"], ["mai dian", "埋点"],
  ["da shu ju", "大数据"], ["qu kuai lian", "区块链"], ["bi te bi", "比特币"],
  ["zheng qi ji", "蒸汽机"], ["wa te", "瓦特"], ["ju li zi", "举例子"],
  ["shu du", "熟读"], ["zhen wei si", "甄维斯"],
])) {
  assert(aggregateChineseRows.some((row) => row[0] === text && row[1] === reading),
    `required modern aggregate term missing: ${text}`);
}
const upstreamWeights = new Map(upstream.split(/\r?\n/u).filter((line) => /^(?:打|管|热爱)\t/u.test(line))
  .map((line) => {
    const [text, , weight] = line.split("\t");
    return [text, Number(weight)];
  }));
assert(upstreamWeights.get("打") === 107925 && upstreamWeights.get("管") === 15029,
  "pinned upstream competitor weights changed");
assert(Number(dictionaryRows[0][2]) === Math.ceil(upstreamWeights.get("打") * 1.05 / 1000) * 1000,
  "大 weight no longer follows bounded formula");
assert(Number(dictionaryRows[1][2]) === Math.ceil(upstreamWeights.get("管") * 1.05 / 1000) * 1000,
  "关 weight no longer follows bounded formula");
assert(Number(dictionaryRows[2][2]) === Math.ceil(upstreamWeights.get("热爱") * 1.05 / 1000) * 1000,
  "热爱 weight no longer follows bounded formula");

const schema = await readFile(path.join(root, "native/resources/Rime/agent_pinyin.schema.yaml"), "utf8");
assert(/^  dictionary: agent_pinyin$/mu.test(schema), "schema no longer selects the aggregate dictionary");
assert(/^  user_dict: pinyin_simp$/mu.test(schema), "schema no longer preserves the v0.3 UserDB name");

for (const [name, [relativePath, checksum]] of Object.entries(assets)) {
  console.log(`${name}=${relativePath} sha256=${checksum}`);
}
