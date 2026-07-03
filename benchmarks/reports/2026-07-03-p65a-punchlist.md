# P6.5-A Punchlist Report

Task file:

- `C:\合作工作区\ActingCommand\FIX-P6.5-A-punchlist-6ca6b0c.md`

Baseline:

- `6ca6b0c` (`docs: record p6.5-a acceptance ci`)

## Required punchlist status

| Item | Status | Evidence |
| --- | --- | --- |
| P1 closeout A2 downgrade | Done | `benchmarks/reports/2026-07-02-p65a-maa-fusion-chain-closeout.md` now uses the request-level fresh-probe wording and the current `capture_expected_change_stall_marks_stale_without_runtime_switch` test name. |
| P2 MuMu discovery diagnostics | Done | `discover_devices()` and `discover_mumu_devices_from_processes()` now return `DeviceDiscoveryReport`; no lossy diagnostics wrapper remains. Bare MuMu processes without `-v` or `MuMuPlayer-` evidence are skipped with diagnostics instead of aliasing to instance `0`. |
| P3 artifact-lock verify gate test | Done | `artifact_lock_expected_mismatch_fails_run_gate` verifies `run()` returns an error when the expected lock differs, while a matching lock passes. |
| P4 runtime library loadability test | Done | `runtime_library_loadability_rejects_corrupt_file` verifies corrupt runtime DLL paths fail through `validate_runtime_library_loadable`. A full `run_abi_check` runtime-library integration test remains blocked by the lack of a valid in-repo provider DLL fixture. |

## Suggested punchlist status

| Item | Status | Notes |
| --- | --- | --- |
| S1 invalid base64 decode branches | Done | `vision_frame_rejects_invalid_base64_pixel_payloads` covers non-multiple-of-four length, invalid byte, and invalid padding. |
| S2 base64 padding round trip | Done | `vision_frame_round_trips_base64_padding_payload` covers `=` padding with `gray8` data. |
| S3 watchdog timeout branch | Done | `watchdog_terminates_after_timeout_without_cancel` verifies timeout-driven termination through an injected counter. |
| S4 PPOCR provider read_text_json direct cache injection | Deferred | Existing `SessionCache` unit tests cover loader count semantics. Direct provider-level injection would require a larger production seam around `ort::Session`, which is not justified for this punchlist. |
| S5 H2 source comment | Done | `take_owned_buffer` now documents the oversized-response free path and the accident-hardening/FFI-trust boundary. |
| S6 explicit instance dedup precedence | Deferred | P2 removes the known alias-to-zero source. No current trigger path remains for a synthetic implicit-vs-explicit instance conflict, so this stays a future discovery-policy hardening item. |

## Scope boundary

No upstream source, upstream binaries, OCR models, resource repository data, UI, SQLite, scheduler behavior, device live operation, or game logic was added by this punchlist.

## Validation

Final public validation is recorded in `CHECKPOINT.md`.
