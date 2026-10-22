# Zhizi Input Method (知字输入法)

English · [中文](README.zh-CN.md) · [Website](https://zhizi.dabblecove.com/)

Zhizi is a Simplified Chinese full-pinyin input method for macOS (Apple Silicon) and Windows 11 (x64). The typing core is fully offline; a small bundled context model, Shunshou-Zhizi, runs on-device and reorders candidates using the text before the caret.

Current version 0.1.5; see [CHANGELOG.md](CHANGELOG.md) (Chinese) for release notes.

## Features

- Full-pinyin input with initials-only abbreviation and mixed abbreviation; eight fuzzy-pinyin groups (zh/z, ch/c, sh/s, n/l, f/h, an/ang, en/eng, in/ing), each with its own switch.
- Whole-sentence input: long pinyin strings produce sentence candidates from multiple segmentations, and the model picks by context.
- Typo correction: common slips (wrong key, dropped key, swapped keys, extra vowel) can be corrected (on Windows this can be turned off in the Start menu).
- Context-aware ranking: the text before the caret participates in candidate ranking; on Windows it is read from the app's actual content, and code-oriented apps with no preceding text get half-width punctuation by default.
- Mixed Chinese/English input: English candidates appear directly under a pinyin string; tapping Shift toggles Chinese/English (on Windows with a 中/英 badge, taskbar mode, and per-app memory).
- `=` expands the panel to four rows; arrow keys move through the grid.
- User-dictionary learning (UserDB); Ctrl+Backspace clears the highlighted candidate's learned entry, Ctrl+Shift+Backspace blacklists it.
- Candidate-panel skins: static and animated skins (action sequences, decorations that extend outside the panel) plus a skin-manager window; the skin format is defined in `native/resources/Skins/skin.schema.json`.
- Layered lexicons: a base dictionary plus a daily-new-words layer (everyday names by category), a spoken-phrase layer, and idiom, IT and finance extension layers. Professional packs (poetry, CAD, industry, AI and English terms) are not part of this repository: they live in the zhizi-lexicon repository, which is the place to download them and to contribute entries.
- Offline: the input-method core and the model make no network requests (a network-boundary check, `script/check_core_network_boundary.sh`, is part of the release gates).

## Platforms

- macOS: Apple Silicon (arm64).
- Windows: Windows 11 x64; 32-bit application processes are not supported.
- Known limitations: the Windows installer is not code-signed, so SmartScreen or antivirus software may warn; behavior inside restricted processes such as UWP is not fully verified.
- Linux: not supported. The core and platform layers are architecturally separate, but there is no Linux port yet.

## Building

### macOS

```bash
./script/build_native.sh release
```

- Requires an Apple Silicon Mac, Xcode command-line tools, Rust, and Node.js (versions as locked by the repository).
- On the first build, `script/bootstrap_native.sh` downloads librime 1.16.1, the rime-pinyin-simp base dictionary, and Rust standard-library license files, and locks them by SHA-256 into `.build-deps/` (not committed); later builds run offline.
- The context model is not in this repository. Optionally run `python3 scripts/context_model/fetch_public_model.py` before building to download and compile it (see [Model and lexicons](#model-and-lexicons)); without it you get a model-less build with lower ranking quality.
- The output is `build/native/release/Agent Input Method.inputmethod`. Installing it into the system is a separate release-validation step; a successful build alone is not a release.

### Windows

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File script\build_windows.ps1 -Package
```

- Requires Windows 11 x64 and the MSVC toolchain; how to prepare librime, ONNX Runtime, the VC runtime, Inno Setup, and the expected directory layout is documented in the script itself.

## Model and lexicons

- Model: Shunshou-Zhizi (shunshou-zhizi-30m), a small context model for candidate reranking. It ships with the input method and runs on-device via Core ML on macOS and ONNX Runtime on Windows. The model card (training data, evaluation, memorization checks) and its license (CC BY-SA 4.0): <https://huggingface.co/DabbleCove/shunshou-zhizi-30m>
- Lexicons: the published lexicons are maintained in the separate zhizi-lexicon repository (under the same account as this repository). `native/resources/Rime/` in this repository holds the derived dictionaries bundled with the input method; they are generated — do not edit them by hand.

### Getting the model for a from-source build (macOS)

The model files are not committed to this repository: the weights are tens of megabytes, the model is published separately under its own license, and it has its own release cycle. The repository carries only the checksum manifests and a model-card copy under `native/resources/ContextModel/`.

- Fetch and compile it with one command before building: `python3 scripts/context_model/fetch_public_model.py`. The script downloads the pinned revision `b74f57068819380bce70ab3bec22696a81a95c95` of <https://huggingface.co/DabbleCove/shunshou-zhizi-30m> (HTTPS only, redirect hosts restricted to huggingface.co and its CDN), verifies every file against `native/resources/ContextModel/HF_SHA256SUMS`, compiles the `.mlpackage` locally with `xcrun coremlcompiler`, and installs the layout `copy_bundle_model.py` reads (`ime_context_model/release/zhizi-30m-v3`, or `$ZHIZI_WORK/release/zhizi-30m-v3` when set). It refuses to run if that directory already exists, so a private local model copy is never touched.
- Without this step the build still succeeds and the input method works — candidates simply follow the lexicon order, with no context reranking. The model itself is licensed CC BY-SA 4.0; training-data details are in the model card on the Hugging Face page.
- At runtime the model runs on-device via Core ML and makes no network requests (enforced by the `script/check_core_network_boundary.sh` release gate); fetching is a build-time step only. In the management app the feature is the experimental "context model ranking" switch (上下文模型排序（实验）), off by default.
- Reproducibility: the download is pinned by revision and SHA-256. Compiled artifacts are compared against `native/resources/ContextModel/SHA256SUMS`; on Xcode 27.0 every reproducible artifact matches, with two warned exceptions: `zhizi30m_v2.mlmodelc/coremldata.bin` embeds a compiler-generated conversion timestamp and UUIDs (never byte-reproducible). The script prints explicit warnings for both.

## Repository layout

| Path | Contents |
|---|---|
| `native/core/` | Cross-platform Rust core (state machine, candidates, dictionaries, model integration) |
| `native/host/` | Rust host and CLI tools (evaluation harnesses, settings tool) |
| `native/ipc/` | Inter-process communication component |
| `native/macos/` | macOS input method (Swift package: InputMethodKit host, candidate panel, tests) |
| `native/windows/` | Windows TSF shell, loader, and Inno Setup installer |
| `native/resources/` | Bundled resources: Rime dictionaries and skins (derived/frozen files) |
| `script/` | Bash build/install/regression entry points (`build_native.sh`, `build_windows.ps1`, `test_all.sh`) |
| `scripts/` | Node/Python generators and validators |
| `src/`, `src-tauri/` | Management app (Tauri 2 + React + TypeScript) with settings such as skin management |
| `quality/` | Lexicon provenance records (`corpus/`, frozen) and frozen regression evaluation sets (`golden/`) |
| `docs/` | Project documentation (specs, task cards, license registry) |

## License

- Input-method code: Apache-2.0; see [LICENSE](LICENSE).
- Lexicon snapshot data in `native/resources/Rime/`, `native/resources/Lexicon/`, and `quality/corpus/*provenance*.tsv`: CC BY-SA 4.0; see [LICENSE-LEXICON](LICENSE-LEXICON). Retain the upstream licenses and attribution described in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
- Free skin assets in `native/resources/Skins/`: separate terms in [LICENSE-SKINS](LICENSE-SKINS). Paid skins are excluded from this repository.
- Third-party dependencies retain their own licenses; see [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
- The shunshou-zhizi-30m model is published separately on [Hugging Face](https://huggingface.co/DabbleCove/shunshou-zhizi-30m) under CC BY-SA 4.0.

When bundling lexicons in an application, keep the lexicon portion's attribution and ShareAlike obligations under its license; confirm compliance for your particular distribution.

## Contributing

See [CONTRIBUTING.md](docs/public/CONTRIBUTING.md) (issues / pull requests / tests / lexicon-contribution format).

## Contact

- Issues and suggestions: GitHub Issues
- Security issues: GitHub private vulnerability reporting (the repository's Security tab → Report a vulnerability); see SECURITY.md
