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

## Embedded in the Runtime executable, not committed

### scrcpy server 5.0.1

- Upstream: the scrcpy server by Genymobile and Romain Vimont, release v5.0.1 (`Copyright (C) 2018 Genymobile`, `Copyright (C) 2018-2026 Romain Vimont`).
- Source URL: https://github.com/Genymobile/scrcpy/releases/download/v5.0.1/scrcpy-server-v5.0.1 (the official versioned release asset; no other source).
- Verification: the Windows exact-SHA build downloads the asset and fails unless it has the pinned size (733930 bytes) and the official SHA-256 (`764eb6f79811d5211fe9df341120882ba9994c7a61b897d7bf3fb662e53bc536`, published as `PREBUILT_SERVER_SHA256` in the release's `install_release.sh` and reported by GitHub for the asset). `crates/device/build.rs` checks the same pin again and embeds the file; the device crate checks the embedded bytes against the pin before every push to a device. The pin is `components."scrcpy-server-5.0.1"` in `scripts/windows-tools/windows-tool-sources.v1.json`. Nothing of it is committed to this repository.
- License: Apache-2.0 (https://github.com/Genymobile/scrcpy/blob/v5.0.1/LICENSE). The Runtime artifact's `RELEASE-NOTES.md` carries the notice.
- Redistribution: bundled with the Runtime by the owner's decision (#402 ruling, 2026-10-13). This records the owner's decision; it is not a legal opinion.
- Purpose: the `scrcpy` capture backend for real devices (#402 RD-3). The Runtime pushes it to the device's `/data/local/tmp` and starts it with its control and audio channels off and no option that changes a device setting; it never runs on this machine.

### OpenH264 (compiled from source)

- Upstream: Cisco's OpenH264 (`Copyright (c) 2013, Cisco Systems`), at commit `4a2615fac570c6ca1ed4f157b9fdab9466edfd80` of https://github.com/cisco/openh264 as bundled by the `openh264-sys2` 0.9.8 crate, with the Rust wrapper `openh264` 0.9.8 (https://github.com/ralfbiedert/openh264-rs, Ralf Biedert and contributors).
- Crate checksums (crates.io SHA-256, also in `Cargo.lock`): `openh264` 0.9.8 `fcc9071a0b5f9c501ddd01066c2600d922cc799dc450a52975222bcda7755a08`; `openh264-sys2` 0.9.8 `6ada29a4cadc13d4d326737af87c13e224210d7d99fe47594e9f3f9b0102fd70`.
- License: BSD-2-Clause (OpenH264, `openh264` and `openh264-sys2`). The Runtime artifact's `RELEASE-NOTES.md` reproduces the notice and conditions.
- Build: the crates' default `source` feature compiles the bundled C++ source with the MSVC toolchain into `actingcommand-actingd.exe`; no prebuilt OpenH264 binary is downloaded or loaded. Built from source, it does not carry the patent licence Cisco attaches to its own binaries.
- Purpose: decoding the scrcpy server's H.264 stream (only H.264 is used).

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

## Rust crate dependencies

### FFT for the exact template score map

- Role (Workflow #396 REC-2, model D18): `crates/recognition` computes the template products `sum(T * I)` of the exact score map by an FFT cross-correlation (`contracts/resource-declarations.md`, section Task schema `0.9` unique matches). No other crate uses these crates.
- Source: crates.io, compiled from source into the binaries that link `crates/recognition`; `Cargo.lock` pins each version with its crates.io SHA-256 checksum. No source file is copied into this repository.
- Rust dependency license check: `realfft` 3.5.0 reports `MIT`; `rustfft` 6.4.1 and its dependencies `primal-check` 0.3.4, `strength_reduce` 0.2.4 and `transpose` 0.2.3 report `MIT OR Apache-2.0`. These are the licences stated in each crate's crates.io metadata, checked on 2026-10-13.
- Notes: all five are pure Rust. `num-complex`, `num-integer` and `num-traits`, which they also use, were already dependencies.

Before any upstream code, assets, screenshots, templates, OCR data, or model files are copied, adapted, or merged, update this file with:

- upstream project name
- upstream repository URL
- copied/adapted file path
- original license
- original copyright notice
- local destination path
- modification summary
