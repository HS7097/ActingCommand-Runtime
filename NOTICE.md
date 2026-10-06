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

## Redistributed in release artifacts, not committed

### Android SDK Platform-Tools 37.0.1

- Upstream: Android SDK Platform-Tools by Google, version 37.0.1 (`adb version` reports `Version 37.0.1-15733141`).
- Source URL: https://dl.google.com/android/repository/platform-tools_r37.0.1-win.zip (the official versioned URL; no other source, no self-hosting).
- Verification: every Windows exact-SHA build downloads the archive and fails unless it has the size (8044989 bytes) and SHA-1 (`e03e78b1d80b396f1c3358e31251cb31740e1110`) that Google publishes in its repository XML, the pinned SHA-256 (`45f4d63113e895ebde0c90f194099a4676b6ac653bd28d54314a9e022bbc1a99`), and each shipped file its pinned size and SHA-256. The pin and its provenance are `components."platform-tools-37.0.1"` in `scripts/windows-tools/windows-tool-sources.v1.json`.
- Distributed files: `platform-tools/adb.exe`, `platform-tools/AdbWinApi.dll`, `platform-tools/AdbWinUsbApi.dll`, `platform-tools/NOTICE.txt` and `platform-tools/source.properties`, in the Tools release artifact (`actingcommand-tools-<sha>`) only; acsetup installs them under `<install root>\tools\platform-tools\`. Nothing of it is committed to this repository. The archive's other files are not distributed.
- License and notices: `NOTICE.txt` (it begins with the Apache License) and `source.properties` ship unchanged in the same `platform-tools` directory as the binaries.
- Redistribution: redistributed by the owner's decision (Workflow #337, 2026-10-03). Android SDK License section 3.4 forbids copying or redistributing the SDK except where a third-party license requires otherwise; section 3.5 places components released under an open-source license only under that license. This records the owner's decision; it is not a legal opinion.
- Purpose: the adb `actingd` uses by default from an install root (see `external-tools/NOTICE.md`, "ADB version boundary").

## Reviewed but not bundled OCR/NN dependencies

### FastDeploy / PPOCR

- Intended role: R1 OCR backend behind `crates/vision-ffi`.
- Local destination: none in this repository.
- Current status (Workflow #360): `actingcommand-actingd.exe` links the ActingCommand-owned `providers/ppocr-onnx-json` engine in-process. It loads ONNX Runtime from the vision root's `ort\` folder and PP-OCR models from model folders (`contracts/vision-model-folders.md`) on first use. No provider DLL is built or shipped, and no FastDeploy, PPOCR model, dictionary or upstream source is bundled or redistributed.
- Model folder boundary: `apps/vision-provider-check --models-root <dir> [--hash]` lists model folders and computes content identities as metadata only; it loads no model and does not add redistribution rights.
- Provider startup boundary: `apps/vision-provider-check --state-root <runtime-state>` reads the Runtime ledger through B; actual startup facts do not grant redistribution rights.
- License check: FastDeploy repository `LICENSE` was verified through GitHub API on 2026-07-02 as Apache-2.0. PaddleOCR repository `LICENSE` was verified through GitHub API on 2026-07-02 as Apache-2.0.
- MAA release audit: `benchmarks/reports/2026-07-02-r1-maa-ocr-artifact-audit.md` records a local-only inspection of `MaaAssistantArknights/MaaAssistantArknights` release `v6.13.0` asset `MAA-v6.13.0-win-x64.zip`. The inspected artifact set includes `fastdeploy_ppocr_maa.dll`, `MaaCore.dll`, PaddleOCR/PaddleCharOCR ONNX model files, and dictionaries. These files remain ignored under `target/` and are not bundled here.
- Provider export audit: `benchmarks/reports/2026-07-02-r1-maa-provider-export-audit.md` records that the audited `fastdeploy_ppocr_maa.dll` does not export the ActingCommand OCR provider ABI and that `MaaCore.dll` exposes MAA assistant/task-level APIs instead of direct frame-to-OCR provider symbols. These findings do not add redistribution rights or copy any DLL into this repository.
- PPOCR ONNX ROI provider smoke: `benchmarks/reports/2026-07-02-r1-ppocr-onnx-roi-smoke.md` records a local-only real ROI OCR smoke using the Runtime-owned source-only `providers/ppocr-onnx-json` provider, ignored ONNXRuntime runtime DLL, and ignored MAA release PaddleCharOCR ONNX/dictionary artifacts.
- PPOCR ONNX full-frame provider smoke: `benchmarks/reports/2026-07-02-r1-ppocr-onnx-full-frame-smoke.md` records a local-only real full-frame OCR smoke using the Runtime-owned source-only `providers/ppocr-onnx-json` provider, ignored ONNXRuntime runtime DLL, ignored MAA release PaddleCharOCR detector/recognizer ONNX models, and ignored dictionary. The provider source is committed, but the provider DLL, runtime DLL, OCR models, and dictionary are not bundled here.
- Artifact contract status: the contract exists, but no FastDeploy, PPOCR, OCR model, OCR data, or upstream OCR source file is copied or redistributed in this increment.
- Release boundary: before any release bundles these artifacts, update this NOTICE with the exact upstream project URLs, license texts, model/data terms, dictionary terms, copied artifact paths, third-party notices, binary provenance, and redistribution obligations.

### ONNXRuntime

- Intended role: the runtime of the in-process OCR and NN engine behind `crates/vision-ffi`.
- Local destination: `providers/ppocr-onnx-json` contains the ActingCommand-owned Rust engine source for OCR and NN (Workflow #360; the former `providers/onnxruntime-json` crate is removed). No ONNXRuntime runtime binary or model is bundled.
- Current status: NN classification models run in the same in-process engine and ONNX Runtime environment as OCR, with CPU or CUDA as the configuration's `vision` section selects.
- Engine implementation: the Rust `ort` wrapper with default features disabled and dynamic runtime loading. It does not enable `download-binaries`, `copy-dylibs` or DirectML.
- Rust dependency license check: `ort` 2.0.0-rc.12 and `ort-sys` 2.0.0-rc.12 were checked through `cargo info` on 2026-07-02 and report `MIT OR Apache-2.0`.
- Model folder boundary: `apps/vision-provider-check --models-root <dir> [--hash]` lists model folders and computes content identities as metadata only; it loads no model and does not add redistribution rights.
- Provider startup boundary: `apps/vision-provider-check --state-root <runtime-state>` reads the Runtime ledger through B; actual startup facts do not grant redistribution rights.
- License check: ONNX Runtime repository `LICENSE` was verified through GitHub API on 2026-07-02 as MIT.
- Artifact contract status: the contract exists, and the source-only provider crate exists. A local-only R3 NN smoke was run on 2026-07-02 using ignored local artifacts: ONNXRuntime release `v1.24.4` CPU x64 asset `onnxruntime-win-x64-1.24.4.zip`, ONNX Models SqueezeNet `Opset16` model `Computer_Vision/squeezenet1_0_Opset16_torch_hub/squeezenet1_0_Opset16.onnx`, and local smoke labels. The smoke produced real ONNXRuntime JSON output through `ac_onnxruntime_classify_json`; these binaries/models/labels are not copied or redistributed in this repository.
- Release boundary: GPU and DirectML are disabled for the selected route unless a later reviewed task explicitly enables them with lifecycle tests. Before any release bundles ONNXRuntime or models, update this NOTICE with exact licenses, third-party notices, binary provenance, model terms, copied artifact paths, and redistribution obligations.

Before any upstream code, assets, screenshots, templates, OCR data, or model files are copied, adapted, or merged, update this file with:

- upstream project name
- upstream repository URL
- copied/adapted file path
- original license
- original copyright notice
- local destination path
- modification summary
