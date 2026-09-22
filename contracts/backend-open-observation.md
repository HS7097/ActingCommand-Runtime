# Backend open observations

`ExecutionBackendProvider::open_input`, `open_capture` and the optional
`open_nemu_session` return `OpenedBackend<T>`. The backend and its report are
produced by the same original call. The execution session consumes the report
when it takes ownership of the backend. A reused session returns no new open
observation. `open_nemu_session = None` selects the independent input/capture
path without reporting a paired open.

The registry observes the selected backend's existing connection metadata:

- Touch factories supply their actual connect attempts and elapsed milliseconds,
  the selected backend, screen-size response, handshake limits and the ADB
  input geometry retained by that connection.
  The report identifies the selected touch backend's configured pressure and
  the exact MuMu installation source when that resolution was observed.
- Capture factories supply selection attempts and the dimensions already
  obtained by construction or an existing probe. A fresh automatic probe can
  report that capture check as passed. A cached selection reports Unknown and
  carries no current-attempt duration. An explicit constructor that has not
  acquired a frame leaves the capture check Unknown.
- A Nemu paired open reports one owner, its actual connection/resolution
  initialization and both backend roles. The resolution probe is distinct from
  acquisition of a complete frame.
- Production fixture providers report SimulationNotApplicable. A provider
  without observation data reports Unknown.

`status` describes the return of the open, `connection` the observed connection,
and `capture_check` the actual capture/layout check. A constructed backend can therefore
have Passed open status and Unknown connection/capture status. These observations
are not dispatch eligibility or evidence of an input effect. Existing connect,
capture, input, cleanup, cancellation and deadline operations retain their order
and count. The getters read data already in memory.

## Request and session identity

The kernel assigns a checked, monotonically increasing generation to each
logical execution session. It is interpreted with the Host event's owner epoch
and instance. It is an observation identity, not a lease, SDK handle or write
witness. A generation is never reused inside a kernel owner.

The report travels in `ExecutionInputOutcome`, in the capture result's `Frame`,
or in `ExecutionFailureContext` if open or a subsequent stage fails. The report
does not contain a `FencedWrite` or extend its lifetime. Nemu remains paired;
its input/capture views and original worker/close owner remain shared.

Host drains capture-result reports before passing the frame into material
processing. A report is not a new resident frame, a retained prime or a second
artifact. Original pixels, PNG bytes, geometry and their budget/publication
rules continue through the existing capture path.

## Ledger consumer and codec

The original `runtime.lifecycle_observed` event has phase
`backend_open_observed` and an optional `backend_open` typed payload. The event
reuses the triggering request/correlation/frame/run/instance links and the Host
owner epoch. Source/module follow the triggering capture or input operation,
including its fixture/Lab provenance; a paired open retains that caller's origin.
An open failure is Error severity. The primary operation outcome,
its diagnostics, cleanup and vendor stdio keep their original paths.

Readonly observations, capture sequences and Lab capture share the observation
consumer. Contained tasks and monitor captures consume the same report before
material processing. Formal input, including contained-task and Lab input,
uses the shared Host input consumer. Failed results are recorded under those
same request links before the existing cleanup path; lifecycle error propagation
retains the report as well. A shared occurrence receipt prevents a cleanup or
terminal propagation from reporting the original open twice.

Host appends under the existing fact write gate and synchronizes the existing
projection. A failed append remains a fatal observation-write failure. On a
failed backend result, the complete original failure remains attached if its
observation cannot be recorded. Successful input retains its actual performed
effect even if recording subsequently fails.

The serde codec denies unknown fields and uses closed stage/status/source
enums. Event sanitization requires a nonzero session generation and the matching
phase/payload pair. Reports contain at most eight attempts and eight warnings,
with explicit dropped counts; native detail uses the existing 1,024-byte UTF-8 boundary and
truncation marker. Native details stay Sensitive. The public projection retains
typed metadata and removes native attempt text and the raw screen-size response.
Historical lifecycle events decode with the new optional field absent. The
legacy `runtime-events.schema.json` describes a separate legacy event protocol;
the Event V2 Rust contract and its strict serde codec own this payload.

Connection/probe attempt durations retain their actual original measurement
scope. No new timing sample is taken. They do not populate `touch_response_us`
or `capture_acquire_us`; the original Input/Capture events remain the only source
of those performance samples. This event does not publish a RuntimeFactStore
availability record or change policy admission.

## First actual capture in the opening request

Before the kernel returns the original Capture result, it completes any native
Capture/NemuPair open report created in that same command from the actual frame
and `Frame::validate_layout`. A valid frame records Passed and its dimensions;
an acquisition or layout failure records Failed. This changes only the
`capture_check` and valid frame dimensions. Construct/Connect/CachedSelection
attempts, open status, connection status and their elapsed values keep their
original meanings. In particular, a cached selection remains Unknown with no
current selection duration even when its new actual frame passes. Simulation
remains SimulationNotApplicable; an unobserved provider remains Unknown.

The check runs before frame memory admission so a later budget refusal retains
the capture/layout result that actually occurred. Layout failure still reaches
the original consumer's request/material failure handling; it is not reclassified
by this observation. Host records a Failed capture check at Error severity in
the original lifecycle event, with the original operation failure and cleanup
facts unchanged. No extra acquisition or separate performance sample is emitted.

Automatic prime admission failures retain the actual check and valid dimensions
in the original DeviceError evidence before cleanup. Selection transfers those
scalars into its one open failure report even when open returns before the kernel
capture loop. The current candidate attempt remains Failed and the open has no
selected backend; a passed frame check cannot turn a rejected candidate into a
successful selection. Earlier attempts retain their order and details. Owner or
build failure before acquisition carries no capture-check evidence. Invalid
layout/acquisition on these early error returns records Failed without invented valid dimensions. This is
ephemeral metadata only; it retains no Frame, charge, permission or cached result.

Every reachable successful first capture currently has this same-command open
report: independent input opens no capture backend; its first Capture opens it.
A fresh paired Nemu open in Input cannot have a committed frame and fails through
the existing retained-close path. Capture and Input are the only prepare callers;
geometry observation opens nothing. Thus a prior Input cannot establish a usable
paired session whose first capture needs a new event carrier. A reused session
returns no new open report or first-check claim; its old occurrence is not copied
or mutated. Independent backends reopened after application invalidation supply
their new actual open report, while an existing paired owner retains its history.

The kernel stamps the original session generation and all formal Host consumers
drain the report under the current request/correlation/frame/run/instance links.
The existing strict codec, sanitizer, Sensitive native details and public summary
already carry the closed capture-check enum and dimensions; no wire field or
event type is added. This is a capture/layout observation, not a complete
connection self-check or dispatch availability.
