# 2026-07-02 R1 PPOCR ONNX Full-Frame Smoke

## Scope

This report records the first full-frame OCR smoke for the ActingCommand-owned PPOCR ONNX provider.

The provider remains source-only in this repository. No MAA release binary, ONNXRuntime DLL, OCR model, OCR dictionary, OCR data, or upstream source file is copied into the repository by this increment.

## Provider

Provider crate:

```text
providers/ppocr-onnx-json
```

Exported ABI:

- `ac_fastdeploy_ppocr_read_text_json`
- `ac_vision_free_buffer`

The provider now runs PPOCR detector inference for full-frame OCR requests, converts detector probability maps into bounded text regions, and runs PPOCR recognizer inference on detected text boxes.

Sub-frame OCR requests still use recognizer-only ROI OCR and return an explicit warning for that path.

## Local-only smoke artifacts

Local ignored paths used for validation:

- provider DLL: `external-tools/vision/fastdeploy/ac_fastdeploy_ppocr.dll`
- ONNXRuntime DLL: `external-tools/vision/onnxruntime/onnxruntime.dll`
- detector model: `target/maa-r1-ocr-audit/MAA-v6.13.0-win-x64/resource/PaddleCharOCR/det/inference.onnx`
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

Refresh local ignored provider DLL:

```powershell
Copy-Item -LiteralPath 'target\release\actingcommand_ppocr_onnx_json_provider.dll' -Destination 'external-tools\vision\fastdeploy\ac_fastdeploy_ppocr.dll' -Force
```

Check manifest:

```powershell
target\debug\actingcommand-vision-provider-check.exe --manifest target\ppocr-smoke\ppocr-char-manifest.json --backend fastdeploy_ppocr --require-existing
```

Check ABI:

```powershell
target\debug\actingcommand-vision-provider-check.exe --manifest target\ppocr-smoke\ppocr-char-manifest.json --backend fastdeploy_ppocr --abi-check
```

Run full-frame OCR smoke:

```powershell
target\debug\actingcommand-vision-provider-check.exe --manifest target\ppocr-smoke\ppocr-char-manifest.json --backend fastdeploy_ppocr --ocr-frame target\ppocr-smoke\ocr_ascii.png
```

## Result

Full-frame OCR smoke returned:

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
    "blocks": [
      {
        "text": "ABC123",
        "rect": {
          "x": 15,
          "y": 14,
          "width": 190,
          "height": 48
        },
        "confidence": 0.9998682141304016
      }
    ],
    "confidence": 0.9998682141304016,
    "backend": "fast_deploy_ppocr",
    "warnings": []
  }
}
```

## Artifact lock

The local-only artifact lock reported:

- total size: `26136034` bytes
- provider library: `534016` bytes, SHA-256 `d45b0967f4fc1589afc614c39b5835f48fa2f2e20eea140dfc5c62b21025f225`
- ONNXRuntime runtime library: `14203464` bytes, SHA-256 `b95efb2113b603bbbf3f191061c5516a871ed546893c820e4f3b7b6c358dbf2a`
- detector model: `2434618` bytes, SHA-256 `b4752bec39a670f4a59dbbd064bcfdbed2f06606a82622538b5089e038f90575`
- recognizer model: `8963651` bytes, SHA-256 `6d27f689925254336e9eaa9fa490e77ca0726899d34e11aeacb3b1190b30bb72`
- dictionary: `285` bytes, SHA-256 `f27a6aa993c9cb67a588e7ea9aea90bb96b8e51dec6ce98bd7e76c104c1829fe`

## Decision Impact

R1 now has a real ActingCommand-owned full-frame OCR provider path and a real model-backed full-frame OCR output through the existing JSON ABI.

Release packaging remains blocked until exact ONNXRuntime/PPOCR model/dictionary license texts, third-party notices, binary provenance, and redistribution obligations are recorded for any bundled artifacts.
