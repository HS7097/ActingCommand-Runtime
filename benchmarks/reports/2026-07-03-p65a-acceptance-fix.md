# P6.5-A Acceptance Fix Report

Task file:

- `C:\合作工作区\ActingCommand\FIX-P6.5-A-acceptance-aea10a4.md`

Baseline:

- Remote head before this fix: `bb10374`.
- Runtime source baseline under review: `aea10a4`.
- `bb10374` is a README-only follow-up, so the acceptance fixes target the Runtime code state closed at `aea10a4`.

## Scope

This report records the Codex implementation of the P6.5-A acceptance defects F1-F12.

No upstream source, upstream binaries, OCR models, resource repositories, UI, SQLite, scheduler behavior, device live operation, or game logic was added by this fix.

## Required fixes

| Item | Status | Evidence |
| --- | --- | --- |
| F1 MuMu adb fallback dead branch | Done | `crates/device/src/discovery.rs`; tests `discovery_falls_back_to_existing_nx_main_adb_when_sibling_missing` and `discovery_prefers_existing_sibling_adb`. |
| F2 PE export parser bounds and overflow hardening | Done | `apps/vision-provider-check/src/main.rs`; tests for small optional header, overflowing RVA range, and truncated name table. |
| F3 ONNX inference watchdog leak | Done | `crates/onnx-provider-support`; providers now use `InferenceWatchdog`, and test `watchdog_reports_early_cancel_before_timeout` verifies early cancellation with a channel. |
| F4 ORT runtime init race | Done | `OrtRuntimeInitializer` serializes initialization; test `ort_runtime_initializer_is_idempotent_under_concurrency` verifies one fake init under concurrent calls. |
| F5 PPOCR runtime library selection | Done | `select_onnxruntime_library` now fails loudly when no onnxruntime-named library is configured; test `rejects_runtime_library_list_without_onnxruntime_name`. |
| F6 JSON pixel payload size | Done | `VisionFrame.pixels` serializes as base64; tests verify base64 payload shape, round trip, and 1080p-size overhead. |
| F7 ProjectInterface misspelled option keys and null defaults | Done | `deny_unknown_fields` plus validation reject misspelled fields and null defaults. |
| F8 ProjectInterface preset references | Done | `validate_interface` checks all preset operation and recognition references at load time. |
| F9 MuMu instance parsing | Done | Invalid `-v` no longer aliases to instance 0; recoverable `MuMuPlayer-N` segments are parsed even when not final; diagnostics are emitted for skipped invalid processes. |
| F10 PPOCR session reload | Done | Shared `SessionCache` protects cached sessions with `Mutex`; tests verify loader counts for same and distinct paths. |
| F11 provider-check gate behavior | Done | Export-audit and artifact-lock `ok:false` become process-gate failures; artifact-lock supports expected-lock diffing; ABI check validates configured runtime libraries load. |
| F12 A2 consistency | Done as option B | Runtime persistent backend switching is not claimed. Stale expected-change frames remain visible diagnostics; startup/request-level fresh probing remains supported. The unused `switch_backend` decision field was removed. |

## Hardening status

| Item | Status | Notes |
| --- | --- | --- |
| H1 recovery loop diagnostic label | Deferred | Not changed in this acceptance fix. It remains a diagnostic-quality follow-up before recovery execution is promoted. |
| H2 FFI response size cap | Done | `take_owned_buffer` rejects response buffers over `128 MiB` before copying. This is accident hardening, not a malicious-provider sandbox. |
| H3 artifact manifest root allow-list | Deferred | Requires a configurable artifact root policy; not changed in this acceptance fix. |
| H4 PowerShell discovery timeout | Deferred | Discovery timeout hardening remains a follow-up before discovery is wired into a blocking startup path. |

## Validation

Final public validation is recorded in `CHECKPOINT.md`.
