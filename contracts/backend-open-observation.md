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
  carries no current-attempt duration. An explicit selection primes one frame
  at open and reports that capture check ("Primed first frame" below).
- A Nemu paired open reports one owner, its actual connection/resolution
  initialization and both backend roles. The resolution probe is distinct from
  acquisition of a complete frame; a paired open by a Capture command also
  primes one frame inside the owner.
- Production fixture providers report SimulationNotApplicable. A provider
  without observation data reports Unknown.

`status` describes the return of the open, `connection` the observed connection,
and `capture_check` the actual capture/layout check. A constructed backend can therefore
have Passed open status and Unknown connection/capture status. These observations
are not dispatch eligibility or evidence of an input effect; only a `failed`
self-check fact derived from one withdraws a policy instance's availability
(`instance-fact-store.md`, "Backend self-check availability"). Existing connect,
capture, input, cleanup, cancellation and deadline operations retain their order
and count. The getters read data already in memory.

## Primed first frame

Workflow #317 slice sc2: an explicitly selected capture backend (`adb`,
`droidcast_raw`, `nemu_ipc`) acquires one frame at open through the same prime
as a fresh automatic probe. The frame's layout is checked and it is admitted to
the opening Capture command's `FrameMemoryBudget`, so the held frame counts
against that live set (an open without a budget fails
`frame_memory_owner_missing_or_mismatched`, as an automatic open does).
`open_report()` records `capture_check` and `connection` Passed and the
frame's dimensions. The first `capture()` returns that frame with its own
acquisition span (`capture_acquire_us`): open plus first capture acquire
exactly one frame, and the frame enters the performance samples once, when it
is returned. A paired Nemu open by a Capture command does the same inside its
owner (the capture view is primed through the owner's worker); a paired open by
an Input command has no budget and acquires nothing. A failed prime closes the
backend (for a pair, the owner) and fails the open with the check in its one
open failure report and no selected backend, exactly as an automatic probe
failure; construction failures before any acquisition keep their paths.

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
of those performance samples. The host maps each event to the instance's
`backend.selfcheck.*` runtime facts (`runtime-fact-store.md`, "Producers"); the
event publishes no availability record and changes no policy admission.

## Input parameters from the original connection

`input_check` in BackendOpenReport and its public Summary is the original
connection's parameter-check state. Missing historical fields decode as Unknown;
Unknown is omitted when encoding. Simulation input/pair reports use
SimulationNotApplicable. Capture-only reports always leave this check Unknown.
These observations do not grant input authority, availability or bootstrap.

The producer records Passed only at the following existing success points:

- MaaTouch: parsed positive contacts/x/y and the original pressure-range check,
  before saving the successful handshake. A nonpositive maximum pressure still
  takes the original default-pressure failure path. Invalid maxima are never
  placed in a successful handshake observation.
- Minitouch: the original four positive maxima and configured pressure check.
  A later screen/coordinate-mapping failure retains this completed parameter
  check while the connection still fails.
- ADB shell input: the original device-state, positive natural bounds and
  rotation chain. Read/transport failure before a value is checked stays Unknown;
  an observed invalid state, parsed bounds or rotation rejection records Failed.
  Original fenced recovery and cleanup remain unchanged. The shared ADB
  prerequisite of MaaTouch/Minitouch does not stand in for their handshake check.
- Nemu paired input/capture: the original version gate, positive handle,
  get-display/down/up symbols, nonnegative display and positive resolution.
  Construction and capture-only dimensions do not imply an input check. Actual
  parameter or required-symbol rejection retains Failed through original error
  and cleanup propagation; missing installation/transport observations remain
  Unknown. The capture-triggered pair reports its checked input role as well.

The checked pressure comes from the producer's actual start result. Registry
does not fill it from an assumed default. PID remains original handshake metadata;
this check neither queries process liveness nor adds a touch protocol-version
compatibility decision. No new handshake, command, tap/reset or timing is taken.

Each original bounded Connect attempt may also carry `input_parameters`: the
typed check and its valid handshake/pressure/geometry data. This is needed when
a successful check is followed by connection, selection or cleanup failure, or
an earlier candidate's check must survive a bounded fallback. Attempt order,
selection status, elapsed scope and native operation counts are unchanged.
The selected connection supplies the successful report's top-level check;
without a selection, the last actual attempt supplies that observation and its
metadata. Earlier attempts retain their own checks. A Passed parameter check
does not change a Failed candidate or Failed open.

These scalars travel in the original ConnectedTouchBackend/DeviceError and open
report. They retain no Frame, charge, SDK handle, FencedWrite or persistent owner.
Kernel generation assignment and all Host success/failure consumers use the
original request/correlation/frame/run/instance and occurrence receipt. Host
records actual Failed checks at Error severity; original Ledger write failure
remains fatal. The existing raw diagnostics stay Sensitive, while public attempt
summaries retain only these typed parameters. Strict serde and sanitization
enforce source/role, positive successful handshake/geometry/dimensions, legal
pressure and rotation, and the matching backend. Unknown and Simulation are not
failures. Original PR462 timings, capture checks, budget/charge, failure classes,
I1 step/finish/close and single performance samples are unchanged.

### Connections inside an existing input action

The original bounded `run_touch_action` fallback can open another backend after
the session's initial provider open. Each such actual `factory.connect` creates
one new observation from its returned ConnectedTouchBackend or DeviceError,
including the check and valid parameters. It contains only this connection's
attempt, not the accumulated diagnostics or an old open occurrence. A successful
connection remains a successful connection observation if the later action or
cleanup fails; that operation's original error and disposition remain intact.
Failed connect remains Failed, including when its parameter check had completed.
The existing combined fallback/action elapsed value stays in its original
diagnostics; the new connection-only duration is unknown, with no extra timer.

Success observations are moved out through `InputBackend::take_backend_open_observations`;
every action error, including fallback exhaustion and cleanup failure, carries
them in the original DeviceError. Storage is bounded by the original remaining
factory chain and holds only typed reports/occurrence receipts. The actingd
diagnostic wrapper forwards the success transfer and preserves error evidence.
Kernel drains after the original action timer ends on both success and failure,
orders the initial provider observation before action connections and applies
the same current session generation. Host's original shared input consumer writes
them once under the triggering request and occurrence, before its existing
success/failure receipt. Later session reuse drains no prior connection. No
extra connection, action, fallback, cleanup, write witness or performance sample
is introduced.

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
After a primed open the first capture is the primed frame, so this completion
restates the check and dimensions the open already recorded.

The check runs before frame memory admission so a later budget refusal retains
the capture/layout result that actually occurred. Layout failure still reaches
the original consumer's request/material failure handling; it is not reclassified
by this observation. Host records a Failed capture check at Error severity in
the original lifecycle event, with the original operation failure and cleanup
facts unchanged. No extra acquisition or separate performance sample is emitted.

Automatic and explicit prime admission failures retain the actual check and valid dimensions
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
connection self-check or dispatch availability by itself.
