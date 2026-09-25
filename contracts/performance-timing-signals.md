# Performance timing signals (part 1)

Two typed durations join the existing event payloads and the performance
context. Both are measured in **microseconds** (`_us`), are optional, and are
absent from the legacy wire: an old event without the field decodes as `None`.
`None` means unmeasured; a measured value is never replaced by `0`.

## Payload fields

- `input.committed` (`InputPayload::Committed`, the shared `OutcomePayload`)
  gains `touch_response_us`. The kernel measures the original `execute_action`
  call after prepare, committed-frame checks and backend open, and before
  collecting selection/recovery metadata. `ExecutionInputOutcome` carries the
  optional successful span to the shared Host input consumer. It includes the
  synchronous backend action's validation, hold/segmented delays, write/flush
  and worker or child-process wait. MaaTouch/Minitouch have no action-completion
  acknowledgement; this is a program-side call span, not physical device
  response. ADB tap retains its original child termination and output-drain
  boundary. `InputPayloadDraft::committed_with_touch_response`
  carries it; `InputPayloadDraft::committed` leaves it unset.
- `capture.completed` (`CapturePayload::Completed`, the shared
  `ObservationResultPayload`) gains `capture_acquire_us`. It is the span of one
  backend frame acquisition. `CaptureBackend::capture_timed` measures the
  original real `capture` call, including its existing dimension reads, decode
  or worker round-trip. Selected/primed/Nemu view wrappers forward the original
  scalar stored in `Frame`; taking a prime does not measure the take. Automatic
  probes measure each existing candidate's acquisition separately, while their
  selection `elapsed_ms` retains its complete construction/probe/budget scope
  and ranking. The successful chosen frame supplies the event measurement.
  Normal, explicit, cached-selection and Nemu capture use their current call.
  `Frame::try_clone` carries the scalar without sampling or changing the copy's
  independent memory admission. Readonly/sequence/Lab, monitor and contained
  consumers use `Frame::capture_acquire_us`; contained tasks retain their outer
  `TaskTimingBoundary` measurement and its attribution/budget meaning separately.
  `CapturePayloadDraft::completed_with_capture_acquire` carries it;
  `recognition.completed` shares the payload type but never carries the field.

Both clocks use checked monotonic differences and checked microsecond conversion.
Unavailable measurements remain `None`; they do not fall back to a Host/channel
span or retained-frame take. Failed actions/captures keep the existing failure
path and do not publish these success fields. Timing holds no device authority
and changes no native call, cancellation, deadline, cleanup or event count.

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
the gate is dropped. The pipeline maxima therefore appear in the host-level
`perf.summary`. It has one producer, recorded on the summary interval (60 s) or
on a material capacity change (see `capacity-admission.md`), never on every
sample; each recorded summary carries both the monitor context of that moment
and the capacity admission fact. Its context is `PerformanceContext::unavailable`
only when counters are disabled or the window holds no data.

## Summary instance semantics

The periodic (60 s) `perf.summary` context previously queried the pipeline
samples for a literal `"runtime"` instance id, so its pipeline maxima never
matched the per-instance-alias samples and stayed empty. The summary is a
host-level event with no instance link, so it now folds the pipeline samples of
**every instance alias** seen in the window: each `max_*` field is the maximum
over all instances, not a per-instance breakdown. Per-instance contexts (policy
failure `perf_context`, the runtime API context) keep filtering by the
requested instance alias.

## Observation events the contract rejects

A performance observation event (`perf.pressure_started`, `perf.pressure_ended`,
`perf.stutter_detected`, `perf.summary`, `perf.monitor_degraded`,
`perf.monitor_recovered`, `perf.balance_changed`) whose draft fails contract
sanitization no longer ends the runtime. The host drops that one event and
records the failure through the existing monitor machinery
(`record_monitor_failure`): a `perf.monitor_degraded` event is written whose
`failure_code` is the contract's own sanitization code (for example
`invalid_performance_process`), `consecutive_failures` counts up, and after
`max_consecutive_failures` the degraded event is `terminal`. Recovery follows
the existing degraded rules. The capacity summary path keeps clearing the
committed capacity fact on the failed append. Every other ledger write (task,
lease, command, fact, lifecycle) still fails fatally, and every fatal
sanitization message now carries the concrete contract code instead of
`event_sanitization_failed`. A degraded event that fails sanitization itself is
fatal as before. Windows process records are normalised at the sampler boundary
(peak working set at least the working set, no peak without a non-zero creation
time, control characters replaced by `process-<pid>`, names cut to the contract
limit), so a raw counter value does not reach the contract as a rejection.

## Touch backend diagnostics

`TouchBackendDiagnostics` now records a successful action on the primary
(selected) backend as an attempt with its elapsed milliseconds, matching what
the fallback path already recorded. No other behaviour of the selection or
fallback logic changes.
