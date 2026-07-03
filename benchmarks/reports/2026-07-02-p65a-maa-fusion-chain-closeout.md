# 2026-07-02 P6.5-A MaaFramework Fusion Chain Closeout

## Scope

This report audits `TASK-P6.5-A-maa-fusion-chain.md` against the current Runtime repository state.

The task was implemented as clean-room Rust behavior/protocol work plus FFI-bound OCR/NN providers. No MAA C++ source, MAA release binary, ONNXRuntime binary, OCR model, OCR dictionary, OCR data, UI, SQLite, scheduler behavior, device operation, or game logic is bundled by this closeout.

## Gate Evidence

| Gate | Status | Evidence |
| --- | --- | --- |
| P0 touch fallback classification | Complete | `fallback_skipped_on_serious_input_error`, `fallback_on_transient_backend_failure`, `fallback_records_full_context`, `fixed_priority_fails_loud_when_all_backends_fail`; `crates/device/src/touch.rs`; `crates/device/src/error.rs` |
| A2 capture autotune/freshness | Complete for startup/request-level probe; runtime-persistent backend switching deferred | `capture_autotune_caches_probe`, `capture_static_page_same_hash_does_not_switch`, `capture_expected_change_stall_marks_stale_without_runtime_switch`; `crates/device/src/capture.rs`; `apps/actinglab/src/main.rs` |
| A1.1 minitouch | Complete | `minitouch_in_priority_chain`, `minitouch_transient_failure_degrades`; `crates/device/src/touch.rs`; `NOTICE.md` minitouch entry |
| A3 device discovery | Complete | `discovery_lists_running_mumu_serials`; `crates/device/src/discovery.rs` |
| B declarative recovery executor | Complete | `recovery_follows_on_error_edge`, `recovery_wait_freezes_waits_until_stable`, `recovery_stops_at_max_attempts`; `apps/actinglab/src/recovery_exec.rs` |
| E FeatureMatch gate | Complete as decision gate | `benchmarks/reports/2026-07-02-feature-match-gate.md`; pure Rust FeatureMatch not accepted, routed to FFI decision lane |
| R1 OCR | Complete for source-only provider gate | `ocr_reads_text_from_frame`; `providers/ppocr-onnx-json`; `benchmarks/reports/2026-07-02-r1-ppocr-onnx-full-frame-smoke.md`; full-frame smoke text `ABC123`, confidence `0.9998682141304016` |
| R3 NN | Complete for source-only provider gate | `nn_classifies_frame`; `providers/onnxruntime-json`; `benchmarks/reports/2026-07-02-r3-onnxruntime-real-smoke.md`; real NN smoke top label `class_623`, score `6.899109363555908` |
| A4 record/replay | Complete | `replay_reproduces_recorded_action_types`; `crates/device/src/replay.rs` |
| O1 ProjectInterface | Complete | `project_interface_assembles_runnable_config`; `apps/actinglab/src/project_interface.rs` |

## Public Validation

The task file's public validation commands were run with the local ActingLab config already absent:

```text
%LOCALAPPDATA%\ActingCommand\actinglab\config.json
```

was not present before the closeout validation, matching the CI-style test input requirement.

Validation results:

- `cargo fmt --all -- --check`: passed.
- `git diff --check`: passed.
- `cargo test --workspace`: passed with 482 tests.
- `cargo clippy --workspace -- -D warnings`: passed.
- `cargo build --release`: passed.
- GitHub Actions CI run `28590433168`: passed for the full-frame OCR implementation commit.
- GitHub Actions CI run `28590692727`: passed for the follow-up checkpoint commit.

## Boundary Notes

- R1/R3 provider crates are source-only.
- Local real smoke artifacts remain ignored under `external-tools/` and `target/`.
- `NOTICE.md` and `resources/upstream-manifest.toml` record license/provenance boundaries for FastDeploy, PaddleOCR/PPOCR, MAA release OCR artifacts, ONNXRuntime, and the SqueezeNet smoke model.
- Redistribution of ONNXRuntime/PPOCR binaries, models, dictionaries, labels, or MAA release artifacts remains blocked until exact license texts, third-party notices, binary provenance, model/data terms, copied artifact paths, and redistribution obligations are recorded.

## Closeout Decision

P6.5-A is closed for the Runtime source-level gate defined by `TASK-P6.5-A-maa-fusion-chain.md`.

Future work should be tracked as new tasks instead of extending this gate:

- release packaging review for OCR/NN artifacts;
- broader OCR corpus validation;
- FeatureMatch FFI implementation, if later required;
- Lab-2 CLI work.
