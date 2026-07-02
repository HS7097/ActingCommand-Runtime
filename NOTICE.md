# NOTICE.md

ActingCommand Runtime is planned to use `AGPL-3.0-only`.

This split repository was created from the local ActingCommand workspace. It contains the `AliceRuntimeOrchestrator` runtime prototype and independent runtime scripts.

No upstream automation source code has been copied into this repository as part of the split.

## Included external tool binaries

### MaaTouch

- Source project: `MaaAssistantArknights/MaaTouch`
- Source URL: https://github.com/MaaAssistantArknights/MaaTouch
- Source path reviewed locally through BAAH release `DATA/touch.zip` and the upstream `MaaTouch` repository.
- Local destination: `external-tools/maatouch/maatouch`
- License: Apache-2.0
- License text: `external-tools/maatouch/LICENSE`
- Attribution: MaaTouch is maintained by the `MaaAssistantArknights/MaaTouch` upstream project and contributors. The upstream repository and the reviewed `touch.zip/LICENSE.txt` do not provide a separate filled copyright notice beyond the Apache-2.0 license text.
- Purpose: MaaTouch/minitouch-compatible input backend binary used by `MaaTouchBackend`.
- Notes: included after license review by project owner instruction. Runtime touch input prefers ActingCommand's `MaaTouchBackend`; P6.5-A1 adds a public-protocol `adb shell input` fallback implemented in clean-room Rust without adding another external binary.

### minitouch

- Source project: `openstf/minitouch`
- Source URL: https://github.com/openstf/minitouch
- License URL reviewed: https://github.com/openstf/minitouch/blob/master/LICENSE
- License: Apache-2.0
- Copyright notice in upstream license: `Copyright © CyberAgent, Inc. All Rights Reserved.`
- Local destination: none in this repository.
- Purpose: optional local-only minitouch binary path for `MinitouchBackend`.
- Notes: P6.5-A1.1 implements the public minitouch text protocol in clean-room Rust and does not vendor or commit a minitouch binary. Operators must provide a local binary path when using this backend.

## Reviewed but not bundled OCR/NN dependencies

### FastDeploy / PPOCR

- Intended role: future R1 OCR backend behind `crates/vision-ffi`.
- Local destination: none in this repository.
- Current status: `FastDeployPpocrBackend` can dynamically load an ABI-compatible local provider library. `FastDeployPpocrArtifacts` records the reviewed provider library, PPOCR detector model, recognizer model, optional classifier model, dictionary, supported languages, and default timeout before artifact-backed invocation.
- Manifest boundary: `resources/vision-provider-artifacts.example.json` shows the local path contract only. It does not include FastDeploy, PPOCR, OCR models, dictionaries, or upstream source.
- Artifact lock boundary: `apps/vision-provider-check --artifact-lock` can produce size and SHA-256 metadata for locally reviewed artifacts, but that report is provenance metadata only and does not add redistribution rights by itself.
- ABI check boundary: `apps/vision-provider-check --abi-check` can verify that a locally reviewed provider library exports `ac_fastdeploy_ppocr_read_text_json` and `ac_vision_free_buffer`, but it does not grant redistribution rights or replace OCR smoke validation.
- License check: FastDeploy repository `LICENSE` was verified through GitHub API on 2026-07-02 as Apache-2.0. PaddleOCR repository `LICENSE` was verified through GitHub API on 2026-07-02 as Apache-2.0.
- Artifact contract status: the contract exists, but no FastDeploy, PPOCR, OCR model, OCR data, or upstream OCR source file is copied or redistributed in this increment.
- Release boundary: before any release bundles these artifacts, update this NOTICE with the exact upstream project URLs, license texts, model/data terms, dictionary terms, copied artifact paths, third-party notices, binary provenance, and redistribution obligations.

### ONNXRuntime

- Intended role: future R3 NN backend behind `crates/vision-ffi`.
- Local destination: `providers/onnxruntime-json` contains ActingCommand-owned Rust provider source. No ONNXRuntime runtime binary or model is bundled.
- Current status: `OnnxRuntimeBackend` can dynamically load an ABI-compatible local provider library. `OnnxRuntimeArtifacts` records the reviewed provider library, optional reviewed ONNX Runtime dynamic library path, ONNX model, labels or label file, CPU-only execution provider, and default timeout before artifact-backed invocation.
- Provider implementation: `providers/onnxruntime-json` exports `ac_onnxruntime_classify_json` and `ac_vision_free_buffer`. It uses the Rust `ort` wrapper with default features disabled and dynamic CPU-only runtime loading. It does not enable `download-binaries`, `copy-dylibs`, CUDA, DirectML, or other GPU execution providers.
- Rust dependency license check: `ort` 2.0.0-rc.12 and `ort-sys` 2.0.0-rc.12 were checked through `cargo info` on 2026-07-02 and report `MIT OR Apache-2.0`.
- Manifest boundary: `resources/vision-provider-artifacts.example.json` shows the local path contract only. It does not include ONNXRuntime binaries, models, labels, or upstream source.
- Artifact lock boundary: `apps/vision-provider-check --artifact-lock` can produce size and SHA-256 metadata for locally reviewed artifacts, but that report is provenance metadata only and does not add redistribution rights by itself.
- ABI check boundary: `apps/vision-provider-check --abi-check` can verify that a locally reviewed provider library exports `ac_onnxruntime_classify_json` and `ac_vision_free_buffer`, but it does not grant redistribution rights or replace NN smoke validation.
- License check: ONNX Runtime repository `LICENSE` was verified through GitHub API on 2026-07-02 as MIT.
- Artifact contract status: the contract exists, and the source-only provider crate exists, but no ONNXRuntime binary, NN model, NN data, or upstream NN source file is copied or redistributed in this increment.
- Release boundary: GPU and DirectML are disabled for the selected route unless a later reviewed task explicitly enables them with lifecycle tests. Before any release bundles ONNXRuntime or models, update this NOTICE with exact licenses, third-party notices, binary provenance, model terms, copied artifact paths, and redistribution obligations.

Before any upstream code, assets, screenshots, templates, OCR data, or model files are copied, adapted, or merged, update this file with:

- upstream project name
- upstream repository URL
- copied/adapted file path
- original license
- original copyright notice
- local destination path
- modification summary
