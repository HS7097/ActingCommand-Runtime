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

The source ledger must be closed and distinct from the destination. The Runtime
holds the source's existing writer lock in shared mode and opens the native
ledger read-only. For an SQLite source it authenticates the keyed head row, then
reads and authenticates only events `1..=through_sequence` (a migrated source
also reads through its cutover completion). Later events are not read, so this
request does not check their integrity; the source writer's own open and
`actingledger` still do. The exact source facts are verified within that prefix,
and artifact availability is taken as of the prefix. The selected frame is the
only artifact material read. Its length and SHA-256 are verified as it is read,
through the reference that the source events authenticate; no path or
caller-supplied reference alone proves provenance. If the selected PNG cannot be
read, the source is reopened once through its authenticated head, still without
reading material: an eviction recorded after the prefix fails with
`saved_source_artifact_evicted` (or `saved_source_artifact_pending_eviction`,
`saved_source_artifact_eviction_failed`) and a detail naming the eviction and
the prefix positions; otherwise the artifact store's code is returned with the
artifact identity and `no eviction recorded through head <h>`. A segment source
is read whole, with its referenced material verified up to a 4 GiB cumulative
ceiling. Corruption within the read prefix, absent or conflicting evidence,
locked/active source writers and a `through_sequence` beyond the source head
(`saved_source_incomplete`) fail. The source is never repaired, overwritten,
truncated or used as the destination.

The request has a 120-second deadline, including source preparation. The ledger
read is bounded by the declared prefix and that deadline, with no fixed byte or
event ceiling; `ledger_read_budget_exceeded` names the bound that was reached.
Its cost grows linearly with `through_sequence`, so the capture completion's
sequence is the cheapest valid value. Measured on the reference machine (warm
cache), the ledger phase took about 0.047 ms per event (2.6 s for 50,149 events,
127 MB of rows). In time, it reaches half the deadline near 1.27 million events,
about 100 days of ledger at about 12,000 events per day, and the ledger phase
alone exhausts the 120-second deadline near 2.5 million events, about 200 days,
less the time package loading and OCR take; requests start failing there. In
memory, its peak is about 3.4 times the bytes read, about 11 GB (transient) at
the 100-day point. These figures are a record, not a threshold. The source
writer's own open grows the same way. The selected PNG
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
frozen prefix event and source sensitivity. `source_ledger.source_read` records
what was read: the declared and the read prefix, the source head, the events and
ledger bytes read, and `material_scope` (`requested_frame`, or `all_referenced`
for a segment source). `source_ledger.read_complete` keeps its meaning, the
physical completeness of segment files. `phase_ms` records the wall time of the
ledger open (with its SQL read, verification and retention/restore phases for an
SQLite source), the PNG read, the package load and the OCR. The new artifact has a new
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

`expected_sha256` accepts the complete [package reference](package-reference.md). A source reference locates an exact local bundle directory through `package_path`; the original frozen-frame provenance and OCR deadline also bound source admission.
