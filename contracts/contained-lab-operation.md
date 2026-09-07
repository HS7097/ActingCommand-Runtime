# Contained Lab operation

`RuntimeDebugSession::run_contained_lab_operation` submits the Lab-only
`RunContainedLabOperation` request. The request selects exactly one current
projection element ID or an explicit `InputAction::Tap` / `InputAction::Swipe`.
It contains an absolute package path, external expected SHA-256, and optional
source projection sequence/content hash hints. Hints are recorded; resolution
uses only the newly captured Runtime projection.

The online CLI forms are:

```
actinglab do <element-id> --capture --zip <package> --expected-sha256 <hash>
actinglab do --tap <x,y> --capture --zip <package> --expected-sha256 <hash>
actinglab do --swipe <x1,y1,x2,y2,duration-ms> --capture --zip <package> --expected-sha256 <hash>
```

`--instance` and the existing profile selection still choose the Runtime
instance. `--projection-sequence` and `--projection-hash` supply optional source
hints. Element IDs are the complete `observation.elements[].id` values returned
by observation. An absent or unresolvable element fails with
`capability_insufficient` and performs no input. The caller can explicitly
submit a separate coordinate request. Coordinates are validated by the existing
InputAction limits and the current frame dimensions; a recognized page is not a
coordinate prerequisite. Element safety annotations are informational.

Offline `do --scene ... --dry-run`, the existing observation-only planning
mode, and raw observation remain separate existing entries. Executing online
`do` does not interpret a resource or choose a coordinate in the client.

## Runtime ownership

Lab keeps the logical publication's generation reader open until the RPC,
client verification, and result projection finish. Runtime reads the retained
physical generation and calls `PreparedPageObservation::load` with the external
hash before capture. Page evaluation uses its public `evaluate` result,
including the actual projection, public facts, private facts, and omission
counts.

The handler acquires one private lease through the existing owner. It captures
and persists the before frame inside the instance mutex, with validation of the
request's token before and after capture. It records the fresh projection and
prepared selection before calling the existing `HostShared::input` exactly
once. No outer instance lock or additional destructive-step scope surrounds
that call.

After input, the handler revalidates that same token for the after frame. The
existing input owner can transfer the lease at its safe boundary. A transferred
or expired token ends this request's observations; the handler never reacquires
a lease to complete the record. Original release, preemption, shutdown and fatal
handling remain the resource owners. The client response timeout is bounded by
four existing backend-open timeout allowances, the existing maximum input
duration, and the existing I/O allowance. The scheduler retains its configured
lease TTL.

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
one terminal record are stored as DiagnosticJson. Each is at most 256 KiB,
giving an aggregate ceiling of 1 MiB. At most two frames use the existing frame
budget. References retain their own ArtifactVerified event returned by append;
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

Lab renders only the verified result. Its normal result and error details use
the existing profile projector; `--verbose` includes the verified public
operation record. Private observation facts remain in the controlled diagnostic
artifacts, and the shared resource privacy merge controls public target/field
values and details. Geometry remains available as operation information. This
entry creates no local recording authority or persistent session fact store.
