# Context ranking model

Zhizi's optional 30M Core ML model reranks candidates locally. The model is distributed separately at https://huggingface.co/DabbleCove/shunshou-zhizi-30m under CC BY-SA 4.0. Without it, the input method remains usable and candidates follow lexicon ordering. Runtime inference does not access the network.

## Build-time tiers

`ZHIZI_MODEL_TIER` selects the model when building, not in the settings UI. The default is `official`.

| Tier | Intended user | Verification |
| --- | --- | --- |
| `official` | Maintainer with the exact release artifacts | All seven hashes in `native/resources/ContextModel/SHA256SUMS`; unchanged default behavior. |
| `public` | Source builder using the public download | `SHA256SUMS.public` pins itos.json and compiled metadata.json, model.mil, weights/weight.bin and analytics/coremldata.bin. The repository model card is structurally checked and copied. Top-level coremldata.bin is checked for presence, no symlink and size 256 B–64 KiB, without a hash. |
| `custom` | Developer supplying a different model | No hash verification. Required files, no symlinks anywhere in the source path/tree, string-array vocabulary with unique entries and reserved tokens, and a 512 MB total size cap. The repository model card is checked and copied. Core ML interface validation happens at runtime, not in Python. |

Public compilation changes top-level coremldata.bin conversion-date/UUID metadata. The weights and graph are still verified byte for byte. The repository model_card.json also differs from the official private release copy; public/custom use the repository copy. Public successful availability records carry `tier: public, official: true`, meaning the pinned core artifacts match; custom records carry `official: false`. Official availability records retain the original format. A separate management-console indication that a public-tier build is unofficial is **not yet provided**.

Observed top-level coremldata.bin size: 372 bytes in three local compiles and in the maintainer's pinned copy. The public-tier bounds of 256 B–64 KiB are loose sanity limits, not an integrity check; the weights and graph are pinned by hash.

## Commands (macOS)

`script/build_native.sh` invokes the copy script at line 100 and inherits these environment variables. The default source is `$ZHIZI_WORK/release/zhizi-30m-v3`; without `ZHIZI_WORK`, it is `ime_context_model/release/zhizi-30m-v3` under the checkout. Fetching needs network access and the Core ML compiler; runtime does not.

```sh
# Exact official release source (maintainer-provided directory)
ZHIZI_MODEL_TIER=official ZHIZI_WORK=/path/to/work bash script/build_native.sh debug

# Public source build; fetch refuses an existing nonempty destination
python3 scripts/context_model/fetch_public_model.py
ZHIZI_MODEL_TIER=public bash script/build_native.sh debug

# Custom compiled model
ZHIZI_MODEL_TIER=custom ZHIZI_MODEL_DIR=/path/to/custom-model bash script/build_native.sh debug
```

These are build instructions; this document does not claim that a product build was tested against them. A missing/invalid model prints a reason and `SKIP`, writes `available: false`, and returns exit code 0 so the input method can build without model ranking. `custom` success prints `WARN ... unofficial-model`. Unknown tiers also skip.

## Custom interface

The directory must contain `itos.json` and `zhizi30m_v2.mlmodelc/`, the compiled Core ML directory (not an mlpackage). Required compiled files are metadata.json, model.mil, coremldata.bin, weights/weight.bin and analytics/coremldata.bin. Vocabulary entries must be unique strings starting with `<pad>`, `<bos>`, `<unk>`; indices are pad=0, bos=1, unk=2. The runtime requires input `tokens` Int32[8,40] and output `logprobs` [8,39], and finite scores. Incompatible models fall back to lexicon ordering for the session instead of crashing. Python never loads Core ML to validate shapes. The copied repository card describes the expected interface, not independent provenance for a custom model.
