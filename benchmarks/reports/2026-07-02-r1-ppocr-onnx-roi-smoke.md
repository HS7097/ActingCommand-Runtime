# 2026-07-02 R1 PPOCR ONNX ROI Smoke

## Scope

This report records the first ActingCommand-owned OCR provider smoke for the P6.5-A R1 gate.

The provider is source-only in this repository. No MAA release binary, ONNXRuntime DLL, OCR model, OCR dictionary, OCR data, or upstream source file is copied into the repository by this increment.

## Provider

Added provider crate:

```text
providers/ppocr-onnx-json
```

Exported ABI:

- `ac_fastdeploy_ppocr_read_text_json`
- `ac_vision_free_buffer`

The provider uses ONNXRuntime through `ort` with dynamic CPU runtime loading. It reads the existing `FastDeployPpocrInvokeRequest` JSON envelope and runs PPOCR recognizer inference on the requested ROI.

Important limitation:

- This increment runs recognizer-only ROI OCR.
- Detector/full-frame text box detection is not implemented yet.
- The provider returns an explicit warning in OCR results so this limitation is not silent.

## Local-only smoke artifacts

Local ignored paths used for validation:

- provider DLL: `external-tools/vision/fastdeploy/ac_fastdeploy_ppocr.dll`
- ONNXRuntime DLL: `external-tools/vision/onnxruntime/onnxruntime.dll`
- detector model path in manifest: `target/maa-r1-ocr-audit/MAA-v6.13.0-win-x64/resource/PaddleCharOCR/det/inference.onnx`
- recognizer model: `target/maa-r1-ocr-audit/MAA-v6.13.0-win-x64/resource/PaddleCharOCR/rec/inference.onnx`
- dictionary: `target/maa-r1-ocr-audit/MAA-v6.13.0-win-x64/resource/PaddleCharOCR/rec/keys.txt`
- smoke frame: `target/ppocr-smoke/ocr_ascii.png`
- smoke manifest: `target/ppocr-smoke/ppocr-char-manifest.json`

These paths are intentionally ignored and are not release packaging decisions.

## Commands

Build provider:

```powershell
cargo build -p actingcommand-ppocr-onnx-json-provider --release
```

Check manifest:

```powershell
target\debug\actingcommand-vision-provider-check.exe --manifest target\ppocr-smoke\ppocr-char-manifest.json --backend fastdeploy_ppocr --require-existing
```

Check ABI:

```powershell
target\debug\actingcommand-vision-provider-check.exe --manifest target\ppocr-smoke\ppocr-char-manifest.json --backend fastdeploy_ppocr --abi-check
```

Run OCR smoke:

```powershell
target\debug\actingcommand-vision-provider-check.exe --manifest target\ppocr-smoke\ppocr-char-manifest.json --backend fastdeploy_ppocr --ocr-frame target\ppocr-smoke\ocr_ascii.png --ocr-region 0,0,320,80
```

## Result

OCR smoke returned:

```json
{
  "ok": true,
  "backend": "fastdeploy_ppocr",
  "frame": {
    "path": "target\\ppocr-smoke\\ocr_ascii.png",
    "width": 320,
    "height": 80,
    "pixel_format": "rgb8"
  },
  "result": {
    "text": "ABC123",
    "confidence": 0.9997550845146179,
    "backend": "fast_deploy_ppocr",
    "warnings": [
      "ppocr_onnx_provider currently runs recognizer-only ROI OCR; detector/full-frame OCR is not enabled in this increment"
    ]
  }
}
```

## Artifact lock

The local-only artifact lock reported:

- total size: `26100706` bytes
- provider library: `498688` bytes, SHA-256 `c708a4b48c19e163b46e66a787a1e449750e10176c341c64d612d4f43884220f`
- ONNXRuntime runtime library: `14203464` bytes, SHA-256 `b95efb2113b603bbbf3f191061c5516a871ed546893c820e4f3b7b6c358dbf2a`
- detector model: `2434618` bytes, SHA-256 `b4752bec39a670f4a59dbbd064bcfdbed2f06606a82622538b5089e038f90575`
- recognizer model: `8963651` bytes, SHA-256 `6d27f689925254336e9eaa9fa490e77ca0726899d34e11aeacb3b1190b30bb72`
- dictionary: `285` bytes, SHA-256 `f27a6aa993c9cb67a588e7ea9aea90bb96b8e51dec6ce98bd7e76c104c1829fe`

## Decision Impact

R1 now has a real ActingCommand-owned OCR provider path and a real model-backed OCR output through the existing JSON ABI.

R1 is not fully closed by this increment because detector/full-frame OCR is not implemented. The next OCR increment should either:

1. implement detector/full-frame text box detection, or
2. explicitly accept ROI recognizer-only OCR as the required R1 scope for the current CLI gate.
