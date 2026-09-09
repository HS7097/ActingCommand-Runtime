# Saved artifact OCR

`actinglab recognize-artifact --request <absolute-request-json>` forwards one
typed `RecognizeArtifact` request to the Runtime selected by
`ACTINGCOMMAND_RUNTIME_STATE_ROOT`, set to the destination Runtime state directory.
The destination Runtime records a new historical recognition. It owns the
configured production Provider and closes it through
its normal lifecycle. This command requires Lab origin.

The request JSON contains `source`, `package_path`, `expected_sha256`, and
`target_id`. `source` contains an absolute `state_root`, `through_sequence`,
`frame_id`, the complete native `artifact` reference (including its `sha256`),
and the `created`, `verified`, and `captured` event locators. Each locator contains
`sequence` and `event_id`. The three source events must describe the same frame,
artifact and original request/correlation/run. Package SHA-256 is the external
64-character digest. Artifact SHA-256 retains the native `sha256:` prefix.

The source ledger must be closed, complete and distinct from the destination.
The Runtime holds the source's existing writer lock in shared mode, opens the
native read-only ledger, and verifies the exact source facts within the declared
prefix. Source verification uses the existing artifact verifier; no path or
caller-supplied reference alone proves provenance. Corruption, absent or
conflicting evidence, locked/active source writers and invalid prefixes fail.
The source is never repaired, overwritten or used as the destination.

The request has a 120-second deadline, including source preparation. The native
reader is bounded to 64 MiB of ledger data and 100,000 events; native referenced
artifact verification has a 4 GiB cumulative byte ceiling. The selected PNG
retains the existing 64 MiB artifact limit and must match the source capture
dimensions, with at most 16,777,216 pixels. The source target's existing OCR
timeout is capped by the remaining deadline; Provider ownership waiting consumes
that budget. The client waits the full request deadline plus its normal receipt
I/O allowance. A failed/uncertain IPC exchange retains the existing no-replay
rule. These bounds reject oversized work rather than returning partial evidence.

Containment verifies the external package hash before extraction. The Kernel
uses the original RGB decoding, target/ROI evaluator and attested Provider. It
does not capture, open input/capture backends, acquire device leases, execute a
TaskRun or publish policy facts. The original `recognize --scene` behavior is
unchanged.

The `ArtifactRecognized` receipt contains the original source identity, target,
new diagnostic ArtifactRef and its verification event. The diagnostic uses
`actingcommand.runtime.saved-artifact-ocr.v1` and preserves the complete native
`OcrObservationEvaluation`, including region, raw text, block order, blocks,
confidence and execution evidence. It also records the source writer identity,
frozen prefix event and source sensitivity. The new artifact has a new
correlation and no newly minted capture FrameId/run identity. Its redaction is
pending, so original paths and OCR values remain private. Created/verified events
precede the new recognition completion. Failures record their original detail
through the Runtime's existing diagnostic path; persistence failure remains
fatal. Host work admission remains held until this operation and its resource
scope finish.

This entry proves what the configured OCR owner returned for one verified
historical frame. Recognition confidence does not certify the displayed value's
accuracy, and the result is not a current balance.

Task: [Workflow #269, SAVED-ARTIFACT-OCR-v1](https://github.com/HS7097/ActingCommand-Workflow/issues/269).
