# Third-Party Notices — Zhizi Input Method (知字输入法)

This product includes or uses the following third-party components. Full license texts for directly embedded native assets and statically incorporated native dependencies are fixed by immutable source commit, byte count and SHA-256, and are staged into the input-method bundle by the reproducible bootstrap/build process. This file records provenance and licensing facts; it is not legal advice.

The exhaustive npm/Cargo package name, locked version, source and declared license inventory is `docs/current/licenses/LOCKED_DEPENDENCIES.json` format 2, SHA-256 `8ff759b9933941faacc49c31ee45fe796cf2ec2b53f18f2101ccbcb7cfddad31`. It binds all lockfile checksums, contains no unknown-license or copyleft-only package, and is packaged with this notice in the management App. Permissive dual-licensed packages use their MIT, Apache-2.0, BSD, ISC or other permissive option as recorded there. Build-only `caniuse-lite` metadata is CC-BY-4.0 and is not shipped as a runtime data asset.

## Rime / librime 1.16.1

Copyright (c) 2014, RIME Developers. Licensed under the BSD 3-Clause License.

Source: https://github.com/rime/librime

## Rime Pinyin Simplified Dictionary

The `rime-pinyin-simp` dictionary is licensed under the Apache License 2.0.

Source: https://github.com/rime/rime-pinyin-simp

The project-authored schema is separate from the unmodified upstream dictionary. The bundle carries the fixed upstream `LICENSE` and `AUTHORS` files.

## THUOCL lexicons (chengyu, IT, caijing)

`agent_idioms.dict.yaml`, `agent_thuocl_it.dict.yaml`, and `agent_thuocl_caijing.dict.yaml` are generated from the 8,519-row chengyu, 16,000-row IT, and 3,830-row caijing tables at commit `a30ce79d895d01ab5132a5c74c29703ff7efb4cc`. The generated dictionaries are deduplicated against the pinned base and each other and distributed under the MIT License, Copyright (c) 2018 THUNLP. The input-method bundle carries the existing full `THUOCL-MIT.txt` license. `pypinyin` 0.55.0 (MIT) supplies reproducible build-time readings; its code is not bundled. Its phrase data now also contributes to the general vocabulary layer below, which carries the full pypinyin notice.

Source: https://github.com/thunlp/THUOCL

## General vocabulary and new words

`agent_general.dict.yaml` contains 26,947 reviewed entries, SHA-256 `0d41f62fae2a717da06aeb5fefd77d569fce6ae215c51ea178659e6a63fc5cec`. Sources are jieba 0.42.1 `dict.txt` (MIT; PyPI source archive SHA-256 `055ca12f62674fafed09427f176506079bc135638a14e23e25be909131928db2`, author Sun, Junyi), pypinyin 0.55.0 phrase data (MIT; Copyright (c) 2016 mozillazg, 闲耘), and project-authored mined/generated phrases. Per-row sources, counts and grants are preserved in `quality/corpus/agent-general-provenance.tsv` (SHA-256 `7d540c4fc7ccf32934149bfbea63fdc505b113b2cad08784d55874d3d3faff17`). No third-party Python implementation is bundled.

`agent_newwords.dict.yaml` is a separately replaceable 14,633-entry project-authored layer (recent coinages, everyday modern words and spoken phrases, and category-by-category names of everyday things; DeepSeek-generated, 2026-10-17), SHA-256 `bef0359c6a89f0675712a1b50c11603f60f46a35cf9a8c218f2419d6c7d76870`; `quality/corpus/agent-newwords-provenance.tsv` (SHA-256 `6a94bdb6996dcb6508cabb870bd623be42c487aa90055f11747772b713eea18d`) preserves generation/review source and corpus counts. The original two exports were SHA-verified before import. The general layer was re-exported with 10 explicit frozen-gate exclusions; every retained entry and the new-word export are byte-identical. The new-word layer was re-exported from the repository after review found its first export had run outside it (179 duplicates of shipped words, no first-choice caps); it now drops every shipped word and caps 116 weights below the existing first choice.

`agent_spoken.dict.yaml` is a separately replaceable 10,853-row spoken-phrase layer (everyday chat combinations such as 你看下, 我刚刚, 捞手机, plus interface and everyday-operation phrases such as 前一页, 上一步, 长按保存; regenerated 2026-10-16), SHA-256 `1294b968bfb0e6e2946991c349ea1f62dc3afa388bc984963216e2a26c2f4293`; `quality/corpus/agent-spoken-provenance.tsv` (SHA-256 `8651403810bea8474e955c7d1e9e2856f1bd35a834b710b7e3c473cf710aedfb`) records, per row, how many independent generation calls produced it and how many backends did (one: DeepSeek). The phrases were generated from scenario prompts through the DeepSeek API (DeepSeek's terms of use, section 4.2(3), allow using outputs for derivative products and for training other models; the earlier version of this layer was generated with other providers whose terms do not allow this, and was replaced on 2026-10-16). A phrase is kept when it has at least three characters and at least five independent generation calls produced it. Readings come from pypinyin, with a second reading added where an independent check by DeepSeek disagreed on a heteronym character. Only the prompts left the machine; no owner data was read or sent. This layer can be dropped independently.

`agent_ai_terms.dict.yaml` (286 entries, SHA-256 `2e4e058bc42a1a83f4fedf1c9b2a31ed93d63f1c7558f63dcdef01caed2ec4b3`) and `agent_ai_names.dict.yaml` (24 entries, SHA-256 `c458a49954b7e2f9f1e53942f2ee5b9deb4601fb2c80b53e256a72e947af9a7f`) are separately replaceable layers of AI-era product, company, model and tool names (WorkBuddy, DeepSeek, Claude Code; 硅基流动, 火山方舟), 2026-10-13. Each entry was seen on an official or authoritative web page by a research pass and is recorded with that page's URL and the check date in `quality/corpus/agent-ai-terms-provenance.tsv` (SHA-256 `464574b1bcb3e16fd7cae28dd3ba4617dd837c6425cb4c40f0c56a2ed0cf1680`) and `quality/corpus/agent-ai-names-provenance.tsv` (SHA-256 `b25c5e6d77715665675635593167b1667a7c3aa85b114e8fbfa419c9a20df4ec`). Only names and their official spelling are used (facts); no page text is copied. Product names are trademarks of their owners and are listed only to be typed.

The optional professional vocabulary packs (poetry, CAD, industry, English product and software terms, AI product names) are not part of this repository. This repository carries only empty shells of those dictionaries so that the build resolves them. The pack rows, their per-row provenance and their licences (Chinese Wikipedia and Wiktionary titles under CC-BY-SA-4.0, chinese-poetry under MIT with public-domain originals, and further sources) are recorded in the `zhizi-lexicon` repository, which distributes them under CC BY-SA 4.0.

The built-in animated skins (`native/resources/Skins/Builtin/{shuitun,youzi,yueye,luoying,fenghong,denglong,songxue}`) carry PNG illustrations (capybara, yuzu capybara, crescent moon, cherry-blossom branch, maple branch, red lantern, snowy pine) that were generated with the Grok image tool inside Cursor (xAI) from prompts written for this project, then keyed, trimmed and scaled by `scripts/tools/skin_art.swift`. The neon hypercube strip of the `nihong` skin and the first-generation capybara frames are drawn by project code (`scripts/tools/make_neon_cube.swift`, `make_skin_art.swift`). These skins are distributed free of charge under `LICENSE-SKINS`; the project does not sell them. The project has not independently verified the ownership terms of the generated images.

The full `pypinyin-MIT.txt` and `jieba-MIT.txt` notices are bundled and locked by byte count and SHA-256. The jieba 0.42.1 source archive declares MIT but omits the LICENSE file; the notice is the upstream LICENSE at tag v0.42.1 (commit `1e20c89b66f56c9301b0feed211733ffaa1bd72a`, Copyright (c) 2013 Sun Junyi), 1,075 bytes, SHA-256 `18ba0984839f85853b29fadaf992f7dba8fd0ca0fbeae34de2b8735222dc7a37`.

## Common English word list

`native/core/agent-ime-core/resources/english_words.txt` (32,230 words, SHA-256 `ba76bf3152da792ce95d6b2f8f4189366376250eeb4493b19c2a473ffd4bd428`) is compiled into the core. It holds the lowercase whole-word tokens of the OpenAI CLIP BPE vocabulary (`vocab.json`, SHA-256 `e089ad92ba36837a0d31433e555c8f45fe601ab5c221d4f607ded32d9f7a4349`), in token order, generated by `scripts/generate_english_words.py`. CLIP is licensed under the MIT License, Copyright (c) 2021 OpenAI. Source: https://github.com/openai/CLIP. The full license text still has to be staged into the bundle with the other notices before a public release (pre-release license checklist).

## Project-owned Rime tables

`agent_pinyin.dict.yaml` (SHA-256 `7536fbce2da80eeead390c596f8fd32480fefba88cbcc6d97ba5983032cf7472`), `agent_project_terms.dict.yaml` (SHA-256 `4fb4ed762100d69e10c8d0581502a8c2b10f91414f99c99114b450a6418e20b7`), `agent_modern.dict.yaml` (SHA-256 `d1e6a40abca03593a85c38da0e5da9cf8d04404fa1ccb0039027268bd7b54682`), and the three-entry `agent_daily.dict.yaml` are project-owned assets, not third-party dictionaries. The modern table is generated from a 287-row human-readable repository curation, emits 151 unique terms after existing-table deduplication and one non-pinyin skip, and records project-authored provenance. Project-term extraction is capped at 300 repository-owned terms and records project-authored provenance; no external corpus or license is introduced.

## librime transitive components

- Boost 1.89.0 / `ef7fea34711a189472893b88205b1dd3c275677b` — Boost Software License 1.0
- glog `7b134a5c82c0c0b5698bb6bf7a835b230c5638e4` — BSD 3-Clause
- LevelDB `99b3c03b3284f5886f9ef9a4ef703d57373e61be` — BSD 3-Clause
- marisa-trie `3e87d53b78e15f2f43783d5e376561a8c9722051` — BSD 2-Clause option selected from its dual license
- OpenCC `556ed22496d650bd0b13b6c163be9814637970ae` — Apache License 2.0
- Darts-clone 0.32h / `87b71afd6cf784953e3c08f24c64203397f3b724` — BSD 2-Clause; a vendored/derived implementation is statically incorporated through OpenCC
- RapidJSON 1.1.0 / `f54b0e47a08782a6131cc3d60f94d038fa6e0a51` — MIT for the incorporated header implementation; a vendored/derived tree is statically incorporated through OpenCC
- yaml-cpp `2f86d13775d119edbb69af52e5f566fd65c6953b` — MIT License

Exact commits, fixed raw license URLs, byte counts, SHA-256 values and packaging conclusions are recorded in `docs/current/licenses/DEPENDENCY_REGISTRY.md` and `NATIVE_LICENSE_ASSETS.json`. The notice manifest now registers seventeen native payloads: all seventeen are locked. RapidJSON's full upstream `license.txt` inventories BSD and JSON-license subcomponents, but the OpenCC-consumed header tree does not include `bin/jsonchecker` or those optional third-party directories; the bundled notice is retained in full without representing those components as shipped. Final acceptance still requires exact byte-for-byte inventory validation inside the frozen input-method bundle.

## Rust

Portions of the native core use the Rust 1.94.1 standard library from commit `e408947bfd200af42db322daf0fadfe7e26d3bd1`, available under the Apache License 2.0 or MIT License. The input-method bundle carries both full license texts and Rust's version-specific generated `COPYRIGHT-library` inventory, including the standard library's in-tree and out-of-tree attribution/license exceptions.

The management App snapshot publisher uses `sha2 0.10.9` and its `digest`, `crypto-common`, `block-buffer`, `generic-array`, `typenum`, and `cpufeatures` transitive crates. These are licensed under MIT and/or Apache-2.0 as recorded in the dependency registry. None are linked into the input-method hot-path core.

## Exclusions

No Squirrel frontend code or rime-ice asset is included. No external librime plugin dylib—including Lua, octagram, prediction, or the separately distributed GPL legacy plugin—is packaged. V06-05 uses a project-owned local association predictor and makes no external plugin license claim. The BSD-licensed librime core still contains its built-in legacy preset/dictionary compatibility code and generic plugin loader.

## 皮肤水印与字体（2026-10-14）
- 皮肤水印：项目自己的菜单栏图标（`native/macos/Resources/AgentInputMethodMenu.pdf`）渲染成的灰色 PNG，无第三方素材。
- 皮肤里指名的中文字体（楷体、宋体、圆体、行楷、魏碑等）都是 macOS 系统自带字体，只在运行时按名字调用，不随安装包分发；机器上没有时回退到苹方。
