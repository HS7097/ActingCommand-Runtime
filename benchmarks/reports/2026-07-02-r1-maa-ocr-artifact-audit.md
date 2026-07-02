# 2026-07-02 R1 MAA OCR Artifact Audit

## Scope

This audit is part of the P6.5-A R1 OCR gate. It inspected upstream release and local upstream-reference artifacts only. No upstream source code, OCR model, OCR data, dictionary, release binary, or dependency DLL is copied into this repository by this report.

## Inputs

- Repository: `MaaAssistantArknights/MaaAssistantArknights`
- Release: `v6.13.0`
- Release asset: `MAA-v6.13.0-win-x64.zip`
- Asset size: `268602958` bytes
- Asset SHA-256: `244d3baa2b3fd5077f5b1f7166d8cdbfebd0610c36308bd4e1ca04e4dd5a8df2`
- Local audit path: `target/maa-r1-ocr-audit`
- Default branch observed through GitHub API: `dev-v2`

## Findings

The current upstream release package contains OCR-related runtime and model artifacts that are useful as provenance references for R1:

| Relative path | Size bytes | SHA-256 |
| --- | ---: | --- |
| `fastdeploy_ppocr_maa.dll` | `1169920` | `4fef6aa94b40cef0557d3f1d64651b2c2e1be9a752dd7c5afb60d10c35aff19f` |
| `MaaCore.dll` | `11813888` | `53b068e9a37513d5a982e96f5041759f2ac26a4e6dfae7d13adc80a8b4857543` |
| `resource/PaddleOCR/det/inference.onnx` | `2434618` | `d572c1773fd00e72f2f2a4c6399513223c49d70f64bc8ccf52fc6cc500b2803c` |
| `resource/PaddleOCR/rec/inference.onnx` | `10736541` | `ece6e0173b177a79358b7610524d768711c7c887895f1b2b767e2bed83ec88cf` |
| `resource/PaddleOCR/rec/keys.txt` | `34240` | `38055a5ea5937ac7ea96f114fb2deaccab8c32f89d94a05a31acbf1293f9f83c` |
| `resource/PaddleCharOCR/det/inference.onnx` | `2434618` | `b4752bec39a670f4a59dbbd064bcfdbed2f06606a82622538b5089e038f90575` |
| `resource/PaddleCharOCR/rec/inference.onnx` | `8963651` | `6d27f689925254336e9eaa9fa490e77ca0726899d34e11aeacb3b1190b30bb72` |
| `resource/PaddleCharOCR/rec/keys.txt` | `285` | `f27a6aa993c9cb67a588e7ea9aea90bb96b8e51dec6ce98bd7e76c104c1829fe` |

The checked local upstream tree also includes PaddleOCR/PaddleCharOCR resource directories and ONNX helper code. The current upstream default branch tree exposes `resource/PaddleOCR`, `resource/PaddleCharOCR`, and `resource/onnx` paths.

## Runtime Contract Impact

`FastDeployPpocrArtifacts` now records optional `runtime_library_paths`. This is needed because a real OCR smoke cannot be represented safely by a single provider DLL plus model paths: dependency DLLs such as OCR runtime libraries must be validated, hashed, and reviewed with the same fail-loud artifact boundary.

The field is optional and defaults to an empty list so older manifests remain readable. When paths are configured, manifest existing-file validation and `apps/vision-provider-check --artifact-lock` include them.

## Boundary

This audit does not make MAA release artifacts redistributable by itself. Before bundling any MAA-derived binary, OCR model, dictionary, or OCR data, the release packaging task must record exact license texts, third-party notices, model/data terms, copied paths, and redistribution obligations in `NOTICE.md`.

No real OCR output has been produced in this increment. R1 remains open until a reviewed OCR provider path produces real `ocr_reads_text_from_frame` output through the ActingCommand FFI boundary.
