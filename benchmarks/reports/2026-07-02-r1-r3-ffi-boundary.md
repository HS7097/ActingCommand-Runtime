# R1/R3 FFI Boundary Decision Report · 2026-07-02

This report records the P6.5-A R1/R3 route after Alice approved the recommended path:

`FFI boundary first, then FastDeploy/PPOCR for OCR and ONNXRuntime for NN`.

## Decision

- Add `crates/vision-ffi` as the safe Rust boundary for OCR and NN engines.
- Route R1 OCR toward FastDeploy/PPOCR.
- Route R3 NN toward ONNXRuntime.
- Keep MAA C API integration out of this increment because the needed OCR/NN layer is below MAA's public tasker/resource/controller API surface.
- Keep GPU and DirectML disabled for this route until a later task adds explicit lifecycle coverage.

## Implemented boundary

- `OcrEngine` trait.
- `NnEngine` trait.
- `VisionFrame`, `VisionRect`, OCR request/result, and NN request/result models.
- Fail-loud unavailable OCR and NN backends.
- Route decision metadata with the planned backend choices and size estimate.
- Dynamic-library adapter structs:
  - `FastDeployPpocrBackend`, loading `ac_fastdeploy_ppocr_read_text_json`.
  - `OnnxRuntimeBackend`, loading `ac_onnxruntime_classify_json`.
- A shared owned-buffer ABI with paired `ac_vision_free_buffer`.
- Unit tests named `ocr_reads_text_from_frame` and `nn_classifies_frame` through ABI-compatible test functions.
- Fatal tests for non-zero provider status and missing provider libraries.

## Size delta

- Current increment: no external OCR/NN binary, model, or runtime is bundled.
- Current repository size delta from OCR/NN artifacts: `0 MB`.
- Planned full route estimate from the task file: `150-250 MB` once reviewed FastDeploy/PPOCR, ONNXRuntime, and model artifacts are linked or packaged.

## License and redistribution boundary

No FastDeploy, PPOCR, ONNXRuntime, model file, OCR data, or upstream source file is copied or redistributed in this increment.

Before the next R1/R3 implementation step can be release-packaged, the exact artifact set must be recorded in `NOTICE.md`, including:

- upstream project URL;
- license text;
- model/data terms;
- copied artifact path;
- local destination path;
- redistribution and relinking obligations.

## Validation expectation

This report closes the Rust-side FFI boundary and dynamic adapter sub-step. The full R1/R3 gate remains open until reviewed FastDeploy/PPOCR and ONNXRuntime provider artifacts produce OCR and NN results behind this boundary and pass the public workspace validation commands.
