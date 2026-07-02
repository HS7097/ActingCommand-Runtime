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
- Current status: `FastDeployPpocrBackend` can dynamically load an ABI-compatible local provider library, but no FastDeploy, PPOCR, OCR model, OCR data, or upstream OCR source file is copied or redistributed in this increment.
- Release boundary: before any release bundles these artifacts, update this NOTICE with the exact upstream project URLs, license texts, model/data terms, copied artifact paths, and redistribution obligations.

### ONNXRuntime

- Intended role: future R3 NN backend behind `crates/vision-ffi`.
- Local destination: none in this repository.
- Current status: `OnnxRuntimeBackend` can dynamically load an ABI-compatible local provider library, but no ONNXRuntime binary, NN model, NN data, or upstream NN source file is copied or redistributed in this increment.
- Release boundary: GPU and DirectML are disabled for the selected route unless a later reviewed task explicitly enables them with lifecycle tests. Before any release bundles ONNXRuntime or models, update this NOTICE with exact licenses, binary provenance, model terms, copied artifact paths, and redistribution obligations.

Before any upstream code, assets, screenshots, templates, OCR data, or model files are copied, adapted, or merged, update this file with:

- upstream project name
- upstream repository URL
- copied/adapted file path
- original license
- original copyright notice
- local destination path
- modification summary
