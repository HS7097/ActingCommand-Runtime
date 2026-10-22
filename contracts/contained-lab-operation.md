# Contained Lab operation

`RuntimeDebugSession::run_contained_lab_operation` submits the Lab-only
`RunContainedLabOperation` request. The request selects exactly one current
projection element ID or an explicit `InputAction::Tap` / `InputAction::LongTap` /
`InputAction::Swipe`; `run_contained_lab_declared_operation` submits the same request with an
operation the package declares ("A declared operation" below).
It contains an absolute package path, external expected SHA-256, and optional
source projection sequence/content hash hints. Hints are recorded; resolution
uses only the newly captured Runtime projection.

The Runtime presses the selection as the kernel presses a pack's click (#411 Lab L3). A tap is
`InputAction::Tap`. A drag that does not move, `from` equal to `to` in the same rectangle (the
form a projection element and an explicit long tap give a long press), is `InputAction::LongTap`
at that point over its `duration_ms`; every other drag is one `InputAction::Swipe`. The Runtime's
preparation and the evidence verifier resolve by the same rule, so the prepared `geometry` stays
a `drag` and its `action` is the long tap. A pack `drag` whose two rectangles are the same, and
`--swipe x,y,x,y,ms`, therefore also press a long tap; on adb it is the same command as a
zero-length swipe.

An optional typed `after` condition supplies an exact package page ID and
`timeout_ms`. Online `do --capture` exposes it as `--after-page <full-page-id>`
and `--after-timeout-ms <ms>` (default 5000, range 100–10000). The timeout flag
requires a page, and both flags require online capture. Runtime verifies that
the target belongs to the admitted package before lease acquisition or
input. Omitting the condition retains the single-after-frame mode.

The online CLI forms are:

```
actinglab do <element-id> --capture --zip <package>
actinglab do --tap <x,y> --capture --zip <package>
actinglab do --swipe <x1,y1,x2,y2,duration-ms> --capture --zip <package>
```

Online `do --capture` reads only these forms' options (`--package` in place of `--zip`), the
arrival condition above, the hints below, `--record` / `--tap-rect`, `--preview`, `--fields` and
`--pretty`, besides the global options, and (Workflow #406 T4) `--seed <u64>` with `--record` and
without `--tap` / `--swipe`: the point the Lab plans in the open step's click rectangle is drawn
from it ([resource-declarations.md](resource-declarations.md), "The Lab's draw"). The Runtime
presses an element at its projection's point and explicit coordinates exactly, so `--seed` with
an element, `--tap` or `--swipe` is `validation_failed` (`argument_problem` `conflicting`). Any other option, including
`--destructive` and `--allow-destructive` (read only by offline and `--dry-run` planning), is
refused with `validation_failed` (exit 2) before any Runtime connection; its details name each
refused option (`argument`, `argument_problem`: `conflicting` when another path of `do` reads it,
otherwise `unknown`), and nothing is accepted and ignored (#411 Lab L1).

`--instance` and the existing profile selection still choose the Runtime
instance. `--projection-sequence` and `--projection-hash` supply optional source
hints. Element IDs are the complete `observation.elements[].id` values returned
by observation. An absent or unresolvable element fails with
`capability_insufficient` and performs no input. On a matched projection, an element ID that
no element of the projection has and that is one of the package's recognition target IDs (what
offline `do` takes) is a target of the wrong type (#411 A37): the Runtime refuses it at
selection with `lab_target_type_mismatch`, whose outcome names `expected_target_type`
`element_id` and `received_target_type` `target_id`, and actinglab answers that code (exit 2,
`blocked_by: [target]`) with the two types in its details, `arrival` still last; no input is
performed.
The caller can explicitly
submit a separate coordinate request. Coordinates are validated by the existing
InputAction limits and the current frame dimensions; a recognized page is not a
coordinate prerequisite. Element safety annotations are informational.

Offline `do --scene ... --dry-run`, the existing observation-only planning
mode, and raw observation remain separate existing entries. Offline
`do --scene ... --dry-run` takes up to five frames, oldest first, the last the
current frame, on which it plans
([business-identity-consensus.md](business-identity-consensus.md); #411 Lab
B-2). Executing online `do` does not interpret a resource or choose a
coordinate in the client.

The `--capture` client forms, `do --capture --dry-run`, `ensure --capture` and
`wait --capture`, decide in the client on Runtime frames, and they take the
frames target consensus needs (#411 Lab L5c). Each of their observations (do's
one, ensure's start, every route guard and every arrival poll, and every poll of
wait) is the package's frame set: F = `target_sample_frames()` read-only Runtime
captures (2 to 5), the declared `target_sample_interval_ms()` apart (unscaled),
oldest first, the last the current frame; without target consensus it is one
capture, as before. Page detection, every target and the destructive check
evaluate the set as offline `--scene` frames are evaluated
([business-identity-consensus.md](business-identity-consensus.md)); the click, the
coordinates, `frame_coordinates`, `frame_age_ms` and the input's frame are the
current frame's, and `do --capture --dry-run` reports `recognition_coverage` as
offline `do` does. The client holds no lease across the captures. Through its
event query it checks, as the Runtime checks a keyless `observe`
([contained-page-observation.md](contained-page-observation.md)), that no
`InputIntent` reached the instance from the first capture's request to the last
capture's terminal event; one did, and the form fails with
`observation_samples_interrupted` (`device_error`, exit 4, `details.error`)
before anything is decided on the frames. Frames of different sizes are refused
with `validation_failed` (exit 2, `argument: --capture`, `argument_problem:
unusable`, `index`), as offline frames are. A wait poll takes one whole set, so
an observation takes at most five frames.

## Runtime ownership

Lab keeps the logical publication's generation reader open until the RPC,
client verification, and result projection finish. Runtime reads the retained
physical generation and calls `PreparedPageObservation::load` with the external
hash before capture. Page evaluation uses its public `evaluate` result,
including the actual projection, public facts, private facts, and omission
counts.

The handler acquires one private lease through the existing owner. It captures
and persists the before frame inside the instance mutex, with validation of the
request's token before and after capture. Each of its observations (before, after,
every arrival sample) is such a transaction under the same lease and fences
([contained-page-observation.md](contained-page-observation.md); #411 Lab L5): the
frames its package's target consensus needs, the declared interval apart, each
captured and fenced as one frame is, the last the observation's frame. It is a
page observation: a candidate layout that needs more than one frame is not
projected (`candidate_projection_skipped`) and does not make it partial. It records the fresh projection and
prepared selection before calling the existing `HostShared::input` exactly
once. No outer instance lock or additional destructive-step scope surrounds
that call. The token validations around capture are lease validation on
non-write paths; the input's device write runs only under the `FencedWrite` that
`HostShared::input` obtains from `begin_destructive_step`
(`read-session-resource-close.md`).

After input, the handler revalidates that same token for the after frame. The
existing input owner can transfer the lease at its safe boundary. A transferred
or expired token ends this request's observations; the handler never reacquires
a lease to complete the record. Original release, preemption, shutdown and fatal
handling remain the resource owners. The client response timeout is bounded by
four existing backend-open timeout allowances, the existing maximum input
duration, and the existing I/O allowance. The scheduler retains its configured
lease TTL.

With an arrival condition, input is still performed exactly once. The same
request/correlation/lease captures and projects up to 20 after observations,
each a transaction. #411 A18 option ④: the samples start every 500 ms, start to
start; after a sample's capture and evaluation the request waits on the
RuntimeClock for what is left of the 500 ms since that sample started (no wait
when they took longer), never past the remaining RuntimeClock deadline. The first
sample can be immediate. Each capture is fenced before and after; target
recognition also requires a still-valid token after projection. Only a unique,
complete match of the requested page before the deadline is `reached`.
No-match or another complete page may be sampled again; partial/conflicting
evaluation or a native failure aborts. Deadline or sample-budget exhaustion is
`timed_out`. The original token's transfer, expiration, cancellation or shutdown
ends waiting. The request thread waits without instance/owner locks; it creates
no additional input, lease or background worker. Client response allowance
includes the explicit wait and 23 existing backend-open timeout allowances for
the bounded capture/input/cleanup work. The existing I/O allowance and lease TTL
remain applicable.

## Durable records

The schemas are:

* `actingcommand.runtime.lab-operation-prepared.v1`: `LabOperationPrepared`.
* `actingcommand.runtime.lab-operation-terminal.v1`: `LabOperationRecord`.
* `actingcommand.runtime.contained-page-observation.v1`: the before and after
  observation artifacts, as produced by the shared observation owner.

The prepared record binds request/correlation/instance/package hash, the actual
lease ID, fresh before frame and projection references, caller selection and
source hints, the selected current element, and final Geometry/InputAction.
Unavailable stages remain absent. Its DiagnosticJson artifact is verified
before input.

The terminal record includes that exact prepared record and verified artifact,
the input-returned flag, native InputIntent/action ID and outcome event ID/sequence, the native
EffectDisposition, after frame/projection if available, and a typed failure
stage/code/native event reference. A secondary ordinary release failure is
retained separately. Each frame states whether its token remained valid after
capture. Frame IDs come from the existing issuer; no task/run/frame identity is
invented for a missing stage.

Prepared records include the exact optional arrival condition. Terminal records
include its `reached`, `timed_out` or `aborted` result, actual RuntimeClock elapsed
milliseconds and ordered, bounded frame/projection artifact references for all
returned samples. The existing after-frame/projection fields are the last
actual sample, including an absent projection when that stage failed. All
intermediate observations are persisted by the same original owners. Failure
before waiting is `aborted` with zero elapsed time and no samples. Timeout and
abort retain the native input effect and produce a failed receipt and nonzero
CLI result; a later release failure cannot replace the original failure.
actinglab answers a timed-out arrival `lab_arrival_timeout` (exit 3,
`blocked_by: [arrival_page]`), as it answers a navigation step whose page did
not arrive, not `device_error`; its details are the operation's answer. In
every online `do` answer `arrival` is the last member, so a reader that cuts
the text loses samples first.

`ContainedLabOperationResult` returns the record and its verified terminal
artifact. The outer receipt is `completed` only for the complete same-lease
sequence. An ordinary incomplete operation returns `failed`, its normal error
projection, and this result together. This result-bearing failure form is
restricted to the contained Lab operation result; other receipt variants retain
their existing rules.

After a nonfatal input return, Runtime queries the native EventQuery interface
for this request/correlation/instance/lease's unique InputIntent (limit 2), then
queries InputCommitted and InputFailed by its physical action ID (each limit 2).
Exactly one mutually exclusive outcome supplies the actual effect, event ID and
sequence. A returned terminal is also checked for exact identity and consistency.
Missing outcomes remain Indeterminate, with the original input failure retained;
they never authorize after-observation. Duplicate, conflicting, mismatched or
unreadable evidence fails explicitly. A fatal input return does not initiate
recovery queries. No event is selected by proximity or latest sequence.

Effect comes from the native InputCommitted/InputFailed event. Failure before input is NotPerformed. A committed
physical input remains Performed if after-observation fails. Native failed input
retains NotPerformed or Indeterminate. Ledger/artifact fatal failures follow the
existing fatal path, retaining already committed native facts without recursive
diagnostic appends.

At most one before projection, one prepared record, one after projection, and
one terminal record are stored as DiagnosticJson in single-frame mode. Each is
at most 256 KiB, giving an aggregate ceiling of 1 MiB. Arrival mode allows at most
23 such objects (before, up to 20 after, prepared and terminal), totaling at most
5888 KiB. It stores at most 2·F frames in single-frame mode and 21·F with
arrival, F the transaction's frame count (1–5), under the existing per-frame
budget (#411 Lab L5). The record
contains references to intermediate projections, not their duplicated contents.
References retain their own ArtifactVerified event returned by append;
no latest-ledger position is used as a substitute. Existing recognition
no-match/conflict/partial statuses and omission counts remain actual observation
results and do not create a page assertion for coordinate input.

## Consumer verification and privacy

RuntimeClient independently verifies the request, instance, package hash,
selection, hints, lease grant, native artifact lifecycles, frame identities and
hashes, projection artifacts, prepared/terminal content, input action/event and
effect, stage order, and final command event. Complete results also require
distinct before/after frame IDs with no intervening native lease terminal.
Integrity failure latches the existing client fatal state.

The shared verifier checks every recorded arrival sample's artifact lifecycle,
hash, frame/projection identity and order, the bounded object set and the final
condition/result consistency. Each observation may be preceded by exactly
`provided_frames − 1` further verified frames of the request (`provided_frames`
from its `recognition_coverage` row; none without the row), after the previous
projection (or the lease grant) and before its own frame; the observation the
operation failed in may lack its projection, and a transaction it failed in before
its frame leaves its earlier frames after the last projection, fewer than five in
either case. Any other frame is `runtime_lab_frame_budget_mismatch` (#411 Lab L5). Offline `resource restore` uses that same
verification. A requested arrival that did not succeed with a complete chain is
a `lab_arrival_not_reached` gap and contributes no executable operation. An
author-supplied target cannot replace an observed page. Original operations
without an arrival condition retain their existing verification path.

Lab renders only the verified result. Its normal result and error details use
the existing profile projector; `--verbose` includes the verified public
operation record. Private observation facts remain in the controlled diagnostic
artifacts, and the shared resource privacy merge controls public target/field
values and details. Geometry remains available as operation information. This
entry creates no local recording authority or persistent session fact store.

## A declared operation (#412 Lab B-3, ledger revision 4 row LR-10)

The selection `{mode: "operation", operation_id, seed?}` names an operation the package
declares: an `id` of its entry task's operations, run as the kernel runs it (its `when`, guard,
input recheck, point, gesture, `post_delay`, `expect_after` and retries); attempt 1 draws its
point from `seed` when given. The id follows the element id's rules (1 to 2048 bytes, no NUL).
A request with this selection and an arrival condition is refused
`invalid_lab_operation_selection`: the operation's own `expect_after` or `to` is its arrival.
No projection resolves the selection (`resolve` refuses it, `invalid_lab_operation_selection`);
the Runtime runs it as "Running a declared operation" below says (Lab B-3 S2).

Its records add two optional members, absent in element and coordinate records, whose bytes are
unchanged; the schema strings stay:

* the prepared record's `operation`: `phase_id?`, `from_page` (as declared), `destination_pages`
  (0 to 16), `max_attempts` (1 without a retry policy), `recheck_frames` (attempt 1's fresh
  recheck frames), `guard` (attempt 1's: `{status: "trusted_coordinate"}` or `{status:
  "passed", page_id, target_id, target_kind}`) and `sampling?` (attempt 1's draw);
* the terminal record's `operation_result`: `end` (vocabulary `lab_operation_end`: `confirmed`,
  `run_completed`, `passed_over`, `recovery_requested`, `failed`), `final_page?`,
  `outcome_key?`, `attempts` (one per effect intent: `attempt`, `input_frame_index`, `guard`,
  `action`, `sampling?`, `input_returned`, `input_action_id?`, `input_intent?`, `input_event?`,
  `effect`, `end_page?`) and `frames` (every frame the kernel captured, in order, at most 256:
  `frame`, `attempt` (0 before the first attempt), `after_input`, `page?`).

In such a record `before_frame` and `action` are attempt 1's input frame and action,
`after_frame` is the last frame and `after_projection` its projection; `geometry`,
`selected_element`, `before_projection`, `after` and `arrival` are absent. The record's
validation refuses (`invalid_lab_operation_record`) a record where: the two members are not
together, or not with the selection `operation`, except that a selection `operation` without
them is a request that failed before any input; attempts do not run 1, 2, …, exceed
`max_attempts`, or an attempt's intent is not after the previous attempt's outcome (the first
after the verified prepared record) and before its own outcome; an attempt's
`input_frame_index` names no frame of that attempt before its input; frames exceed 256 or are not
in capture order; the record's `input_*` members are not the last attempt's; `effect` is not the
aggregate (`performed` if an attempt performed, else `indeterminate` if one was, else
`not_performed`); `end` is `confirmed` or `run_completed` other than exactly when there is no
failure or only one at `release`; the prepared record does not state attempt 1 or the after
frame is not the last frame.

The shared verifier checks such a record's own way: every capture frame of the request is a
listed frame (in place of the transaction accounting); the diagnostics are the prepared and
terminal records and at most the after projection; each attempt that reached its input has
exactly its own `InputIntent` and outcome event, matching its action, and no other input event
belongs to the request (`runtime_lab_unreported_input`); the selection is not resolved again;
the final command event carries the aggregate effect. `RuntimeClient` and `resource restore`
read each attempt's action events. `resource restore` gives such a record the gap
`lab_declared_operation`, restores no operation from it, and its provenance names `end`.

### Running a declared operation (#412 Lab B-3 S2)

The online form is

```
actinglab do --operation <operation-id> --capture --package|--zip <package> [--seed <u64>] [--instance] [--preview] [--fields] [--pretty|--verbose]
```

and MCP `ac_lab_do` passes its `operation` as `--operation` (with `seed`). Before any Runtime
connection, `do --operation` refuses an element id, `--tap`, `--swipe`, `--after-page`,
`--after-timeout-ms`, `--record`, `--tap-rect`, `--projection-sequence`, `--projection-hash`,
`--dry-run` (do's and the global one), `--scene` and `--operation` without `--capture`, each with
`validation_failed` (exit 2) whose details name the argument with `argument_problem`
`conflicting`; `ac_lab_do` refuses the same arguments (with `sample_scenes`) and `operation`
without `capture: true` as `arguments_invalid {argument, argument_problem: conflicting}` before
any child runs. actinglab first runs the kernel's admission of the package and of the operation,
as the Runtime does: a package it refuses is `package_invalid` and an operation it does not run
this way answers the kernel's code (`contained_task_declared_operation_unknown`, `_unsupported`)
with its values, both exit 2 before any connection. The answer is online `do`'s with `operation`
(the prepared record's), `end` and `attempts`. It exits 0 when `end` is `confirmed` or
`run_completed`; 3 (`safety_blocked`, the failure's code) blocked by `page` (page mismatch,
candidates exhausted), `guard` (guard refused, input stability or still unconfirmed) or
`arrival_page` (step unconfirmed, page confirmation failed, error page reached,
`contained_task_timeout` after an input, recovery requested); anything else 4, as today.

The Runtime admits the package and the operation with its own vision provider before any lease;
a refusal is a Denied receipt with no record. The lease lasts the package's task timeout and the
longest input, plus the heartbeat reserve. The kernel then runs the operation in its
declared-operation mode over the Lab lease:

* every capture is the lease's, fenced before and after as an element operation's, and listed
  as a frame entry of the record (its attempt: the attempt started last, 0 before the first;
  the attempt's input frame is that attempt's); a fence that failed ends the operation. The
  capture after the 256th is refused with `lab_operation_frame_limit_reached` (`count`,
  `limit_count` 256; error, `ladder_class` `none`) and ends the operation;
* every input is preceded by the handset keepalive, as a run's, and goes through the lease's
  input with the frame the kernel bound it to; a frame other than the one captured last is the
  fatal `contained_task_effect_frame_identity_mismatch`. Each attempt's intent and outcome are
  read as an element operation's input is, from after the previous attempt's events;
* the point is drawn with the touch keys in effect and the instance's landing history, which
  runs share and which records the press; attempt 1 draws from the request's seed (or one
  derived from the request), attempt k > 1 from that seed and k;
* the Runtime clock at the factor 1000, the configured observe-only retries; no shared
  interrupt, application effect, selection state, task, fact, reading, scheduling or ladder
  record, and no designated effect is counted.

Attempt 1's effect intent persists and verifies the prepared record before the kernel's input; a
prepared record that cannot be persisted stops the operation before the input, without a record,
as an element operation's. When no attempt reached its effect intent the prepared record states
the operation as admitted, with no input frame, action or draw (its `guard` the
trusted-coordinate form) and is persisted after the run. A failure before the current attempt's
input is at `selection`, after it at `arrival`; a capture's at `before_frame` or `after_frame`, an
input's at `input`; `passed_over` answers the guard's refusal, `recovery_requested` the failure
its decision named. Once the operation ended `confirmed` or `run_completed` its last frame is
projected (the after projection, after the run, so no recognition comes between the decision and
the input). A request whose lease was not granted captured nothing; its records state no declared
operation. The client's receipt wait for such a request is the package's task timeout, four
backend-open allowances, the longest input, the I/O allowance and one projection transaction.

## The frame's coordinate system and the after-action attachment (Workflow #410 item 1)

Every `do` answer (online, `--capture --dry-run` and offline `--scene --dry-run`)
states the coordinate system of the frame its coordinates refer to, the before
frame online, as `frame_coordinates {device_pixels: [width, height],
orientation, applies_to: ["tap", "tap_rect", "swipe"]}`: the frame's width and
height in device pixels; `orientation` is `landscape` when the frame is wider
than high, `portrait` when higher than wide, `square` otherwise; and `--tap`,
`--tap-rect` and `--swipe` take coordinates in exactly these frame pixels,
counted from the top-left pixel. The statement is kept short because Min keeps
it whole and `observe`'s Min answer still targets 1 KiB (the soft limit).

After an action the online form attaches its result by default, so the caller
needs no extra observation. Whenever the operation has an after frame (also in a
failure's details), the answer carries `after_attachment`:

* the after projection is `recognized` (a known page): `{kind: "page_projection",
  observation}`, the after page's shared projection. In the default Min view it
  is cut on its own as `observe` cuts its projection (entries from the end while
  it is over 1 KiB, keeping one actionable element; the identity alone, marked
  truncated with its counts, when that is over 2 KiB); `--verbose` and `--pretty`
  answer it whole. It joins the answer after the operation members' Min
  projection, which keeps its own budget. With `--preview` it also has `preview`
  of the after frame.
* otherwise (no match, a conflict, a partial evaluation or no after projection):
  `{kind: "frame", frame_coordinates, frame_path, preview}`, the after frame's
  coordinate system, the after frame read verified from the Runtime's artifact
  store and written as `<sha256>.png`, and its preview, with or without
  `--preview`.

`preview` is `{path, width, height, scale}`: a JPEG (quality 75) whose long edge
is at most 1024 pixels, the other edge keeping the aspect; `scale` is frame
pixels per preview pixel (1 for a frame no larger), so a point read on the
preview is a frame point after multiplying x and y by `scale`. Frames and
previews are written into the Lab frame directory
`%LOCALAPPDATA%\ActingCommand\actinglab\frames`, named by the SHA-256 of the
frame's PNG (`<sha256>.png`, `<sha256>.preview.jpg`), through a partial file
renamed into place. No image is ever put into an answer, only paths. A part that
could not be written (the frame not read, the preview not encoded, a file not
written) is `{failure: {code: "lab_frame_export_failed", path?, raw_text?}}` in
its place, and the answer still succeeds: the action was performed. `--preview`
without `--capture`, or with `--dry-run` or the global `--dry-run`, is refused
with `validation_failed` before any Runtime connection: it previews the frame
after a performed action.
