# R3 ONNXRuntime Real Smoke

Date: 2026-07-02

This report records a local-only real ONNXRuntime smoke run for the R3 NN lane.
It does not add or redistribute ONNXRuntime binaries, ONNX models, labels, or
upstream source files in the Runtime repository.

## Local artifacts

All artifacts below were placed under ignored local paths for validation only:

| Role | Local path | Source | Size |
| --- | --- | --- | --- |
| ActingCommand provider DLL | `external-tools/vision/onnxruntime/ac_onnxruntime.dll` | built from `providers/onnxruntime-json` | 1,165,312 bytes |
| ONNXRuntime DLL | `external-tools/vision/onnxruntime/onnxruntime.dll` | `microsoft/onnxruntime` release `v1.24.4`, asset `onnxruntime-win-x64-1.24.4.zip` | 14,203,464 bytes |
| ONNX model | `external-tools/vision/onnxruntime/models/squeezenet1_0_Opset16.onnx` | `onnx/models`, path `Computer_Vision/squeezenet1_0_Opset16_torch_hub/squeezenet1_0_Opset16.onnx` | 5,009,885 bytes |
| Labels | `external-tools/vision/onnxruntime/models/squeezenet-labels.txt` | local smoke labels `class_0` through `class_999` | 10,890 bytes |

Artifact-lock total: 20,389,551 bytes.

## Commands

```powershell
cargo build -p actingcommand-onnxruntime-json-provider
cargo run -q -p actingcommand-vision-provider-check -- --manifest target\onnxruntime-smoke-artifacts\onnxruntime-smoke-manifest.json --backend onnxruntime --require-existing
cargo run -q -p actingcommand-vision-provider-check -- --manifest target\onnxruntime-smoke-artifacts\onnxruntime-smoke-manifest.json --backend onnxruntime --abi-check
cargo run -q -p actingcommand-vision-provider-check -- --manifest target\onnxruntime-smoke-artifacts\onnxruntime-smoke-manifest.json --backend onnxruntime --artifact-lock --lock-out target\onnxruntime-smoke-artifacts\onnxruntime-smoke-lock.json
cargo run -q -p actingcommand-vision-provider-check -- --manifest target\onnxruntime-smoke-artifacts\onnxruntime-smoke-manifest.json --backend onnxruntime --nn-frame target\onnxruntime-smoke-artifacts\squeezenet-rgb224.png --nn-model-id squeezenet1_0_opset16
```

## Result

The real NN smoke completed successfully through the ActingCommand JSON ABI:

- backend: `onnxruntime`
- frame: `224x224`, `rgb8`
- top label: `class_623`
- top score: `6.899109363555908`
- output label count: 1000

This validates that the Runtime-owned provider DLL can dynamically load the
reviewed local ONNXRuntime DLL, load an ONNX model, run CPU inference, and return
JSON classification output through `ac_onnxruntime_classify_json`.

## Boundary

This is not release packaging approval. Before bundling these artifacts in a
release, the project still needs exact release-asset provenance, license texts,
third-party notices, model terms, copied artifact paths, and redistribution
obligations recorded in `NOTICE.md`.

R1 OCR remains open because no FastDeploy/PPOCR provider, model, dictionary, or
real OCR output has been attached yet.
