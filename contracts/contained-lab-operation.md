# Contained Lab operation

`RuntimeDebugSession::run_contained_lab_operation` submits the Lab-only
`RunContainedLabOperation` request. The request selects exactly one current
projection element ID or an explicit `InputAction::Tap` / `InputAction::LongTap` /
`InputAction::Swipe`.
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
`--pretty`, besides the global options. Any other option, including
`--destructive` and `--allow-destructive` (read only by offline and `--dry-run` planning), is
refused with `validation_failed` (exit 2) before any Runtime connection; its details name each
refused option (`argument`, `argument_problem`: `conflicting` when another path of `do` reads it,
otherwise `unknown`), and nothing is accepted and ignored (#411 Lab L1).

`--instance` and the existing profile selection still choose the Runtime
instance. `--projection-sequence` and `--projection-hash` supply optional source
hints. Element IDs are the complete `observation.elements[].id` values returned
by observation. An absent or unresolvable element fails with
`capability_insufficient` and performs no input. The caller can explicitly
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
each a transaction, with a 500 ms interval bounded by the remaining RuntimeClock
deadline. The first
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
