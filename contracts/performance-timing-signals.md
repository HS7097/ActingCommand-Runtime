# Performance timing signals (part 1)

Two typed durations join the existing event payloads and the performance
context. Both are measured in **microseconds** (`_us`), are optional, and are
absent from the legacy wire: an old event without the field decodes as `None`.
`None` means unmeasured; a measured value is never replaced by `0`.

## Payload fields

- `input.committed` (`InputPayload::Committed`, the shared `OutcomePayload`)
  gains `touch_response_us`. It is the span of the host's input backend call.
  Until the kernel-level backend span lands, the host measures around its call
  into the execution kernel, so the value includes the host→kernel channel
  round-trip on top of the backend write itself. MaaTouch/Minitouch backends
  measure their local write+flush; for AdbShellInput the child process exit is
  the real acknowledgement. `InputPayloadDraft::committed_with_touch_response`
  carries it; `InputPayloadDraft::committed` leaves it unset.
- `capture.completed` (`CapturePayload::Completed`, the shared
  `ObservationResultPayload`) gains `capture_acquire_us`. It is the span of one
  backend frame acquisition, measured host-side around the capture call in the
  read-only observation, the monitor probe capture, and the contained task's
  `CaptureBackend` boundary (the boundary's own span, not a second clock read).
  `CapturePayloadDraft::completed_with_capture_acquire` carries it;
  `recognition.completed` shares the payload type but never carries the field.

Family scope is enforced by `EventPayload::validate`:

- `touch_response_us` present on any payload other than `input.committed`
  fails with `invalid_touch_response_scope`.
- `capture_acquire_us` present on any payload other than `capture.completed`
  fails with `invalid_capture_acquire_scope`.

The public projection does not expose either field (read-face changes are a
separate slice). Existing `capture_latency_ms` semantics, derived from the
`capture.requested` → `capture.completed` timestamps, are unchanged; the new
fields are separate measurements.

## Performance context

`PerformanceContext` gains `max_touch_response_us` and `max_capture_acquire_us`:
the window maximum of the respective payload field over the pipeline samples in
the context window. Both default to `None` when absent (legacy `perf_context`
and `perf.summary` records decode without migration) and are omitted from the
wire when unset. `validate` is unchanged.

The pipeline monitor now observes `input.committed` events as well as the
capture, recognition and task-effect events it already tracked; it reads the
two typed fields from the persisted payload and folds them into the same
per-instance sample queue. The host responsiveness score gains two entries,
`touch_response_us` against a 100 ms target and `capture_acquire_us` against a
250 ms target (the capture acquisition alone sits under the existing 500 ms
event-to-event capture latency target).

## Pipeline observation

Every event the host persists through `append_event` / `append_event_raw` is
handed to the pipeline monitor by the append hook itself (after the fact write
gate is released), so the read-only observation, capture sequence, online
observation, lab operation frame and monitor probe paths all feed the monitor
without a call of their own; only paths that append under the fact write gate
or straight to the ledger (contained task, input receipt, lifecycle,
governance, planning, release control, policy catalog) observe explicitly once
the gate is dropped. The pipeline maxima therefore appear only
in the 60 s counters `perf.summary` (context `sample_count` > 0), never in the
2 s capacity fact `perf.summary` (`capacity` present, context
`PerformanceContext::unavailable`, all fourteen metrics listed as unavailable),
which is the ledger commit of the capacity admission fact and not a monitor
reading.

## Summary instance semantics

The periodic (60 s) `perf.summary` context previously queried the pipeline
samples for a literal `"runtime"` instance id, so its pipeline maxima never
matched the per-instance-alias samples and stayed empty. The summary is a
host-level event with no instance link, so it now folds the pipeline samples of
**every instance alias** seen in the window: each `max_*` field is the maximum
over all instances, not a per-instance breakdown. Per-instance contexts (policy
failure `perf_context`, the runtime API context) keep filtering by the
requested instance alias.

## Touch backend diagnostics

`TouchBackendDiagnostics` now records a successful action on the primary
(selected) backend as an attempt with its elapsed milliseconds, matching what
the fallback path already recorded. No other behaviour of the selection or
fallback logic changes.
