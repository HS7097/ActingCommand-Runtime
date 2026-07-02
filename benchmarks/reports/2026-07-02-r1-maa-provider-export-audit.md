# 2026-07-02 R1 MAA Provider Export Audit

## Scope

This report continues the P6.5-A R1 OCR gate after the MAA OCR artifact audit. It checks whether reviewed local MAA release DLLs can be used directly as ActingCommand OCR providers.

No upstream source code, release binary, OCR model, OCR dictionary, or OCR data is copied into this repository by this report.

## Tooling

`apps/vision-provider-check` now includes:

```text
--export-audit <dll> [--expect none|fastdeploy_ppocr_provider|onnxruntime_provider]
```

This mode parses the PE export table without loading the DLL or its dependencies. It is an audit/report mode. ActingCommand provider enforcement remains the existing fail-loud `--abi-check` mode.

## Expected ActingCommand OCR Provider ABI

For a DLL to be a direct `FastDeployPpocrBackend` provider, it must export:

- `ac_fastdeploy_ppocr_read_text_json`
- `ac_vision_free_buffer`

## Audited Libraries

### `fastdeploy_ppocr_maa.dll`

Command:

```powershell
cargo run -q -p actingcommand-vision-provider-check -- --export-audit target\maa-r1-ocr-audit\MAA-v6.13.0-win-x64\fastdeploy_ppocr_maa.dll --expect fastdeploy_ppocr_provider
```

Result:

- `ok`: `false`
- export count: `763`
- MSVC C++ symbol count: `763`
- present ActingCommand provider symbols: none
- missing symbols:
  - `ac_fastdeploy_ppocr_read_text_json`
  - `ac_vision_free_buffer`

Interpretation:

`fastdeploy_ppocr_maa.dll` is a FastDeploy/PPOCR runtime-style DLL with MSVC C++ exports. It is not directly loadable as an ActingCommand JSON-ABI OCR provider.

### `MaaCore.dll`

Command:

```powershell
cargo run -q -p actingcommand-vision-provider-check -- --export-audit target\maa-r1-ocr-audit\MAA-v6.13.0-win-x64\MaaCore.dll --expect fastdeploy_ppocr_provider
```

Result:

- `ok`: `false`
- export count: `33`
- sample task-level C API exports include:
  - `AsstCreate`
  - `AsstLoadResource`
  - `AsstAppendTask`
  - `AsstStart`
  - `AsstStop`
  - `AsstGetImage`
  - `AsstGetImageBgr`
- present ActingCommand provider symbols: none
- missing symbols:
  - `ac_fastdeploy_ppocr_read_text_json`
  - `ac_vision_free_buffer`

Interpretation:

`MaaCore.dll` exposes MAA assistant/task-level APIs, not a direct frame-to-OCR provider ABI. It can inform a separate task-level MAA adapter decision, but it does not satisfy the current R1 direct OCR provider contract.

## Decision Impact

R1 cannot treat either `fastdeploy_ppocr_maa.dll` or `MaaCore.dll` as a drop-in provider. The next implementation step must be one of:

1. Build an ActingCommand-owned OCR provider DLL that exports `ac_fastdeploy_ppocr_read_text_json` and `ac_vision_free_buffer`, using reviewed OCR runtime/model artifacts behind that boundary.
2. Implement an ActingCommand-owned OCR path around reviewed ONNX/PPOCR model artifacts and expose it through the same JSON ABI.
3. Re-scope R1 to a task-level MAA adapter only if the project explicitly changes the direct OCR-provider requirement.

Until one of those paths produces real OCR output, R1 remains open.
