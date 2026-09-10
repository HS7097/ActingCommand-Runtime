# Capacity admission

Runtime uses the performance owner to sample the executable installation, State,
Artifact objects and Artifact stream staging directories. Windows resolves each
target to its actual volume GUID, including junctions, and queries the bytes
available to the Runtime account once per distinct volume per cycle. An unresolved
binding or unsupported platform produces Unknown with the original native cause.
Private directory paths are not capacity-fact fields.

`capacity_thresholds` in the actingd configuration contains `hard_bytes` and
`soft_bytes`. Defaults are 536870912 (512 MiB) and 2147483648 (2 GiB), with
`0 < hard_bytes < soft_bytes`. These are initial policy values, not a measurement
or a reservation. The existing performance interval defaults to two seconds;
capacity freshness is twice that interval. Optional counter shutdown does not
stop capacity sampling.

Each small typed B3 `PerformanceSummary.capacity` is committed directly to
GlobalLedger before it can authorize business. It includes owner epoch, original
Unix and monotonic observation times, thresholds, volume identities and purpose
sets, available bytes or Unknown, and native failure detail. It needs no context
artifact. Only the append receipt updates the derived view; queuing and appending
do not refresh observation time. Admission carries its decision time and the
committed EventId/sequence. Missing, failed, stale, future, changed-binding or
cross-owner facts refuse new work. Old summaries without capacity do not authorize
capacity admission.

For each required volume, `free < hard` refuses and `hard <= free < soft` warns.
Known new Artifact bytes require `free >= hard + bytes`, using checked addition.
All relevant volumes must pass, including the actual target of a future shard or
staging file. Capacity refusal is nonfatal for this attempted admission; it says
nothing about earlier device inputs or already committed effects.

The performance owner records first sampling failure, escalation on the third
consecutive failure, and recovery through existing monitor events. Low-space
transitions use the existing performance pressure family. Capacity remains
recoverable in the same loop; Ledger failures remain fatal.

In `start_with_provider`, startup preflight follows successful Ledger construction
and precedes Provider assembly. `start` passes a Provider that its caller has
already constructed. Preflight returns the same performance owner used by the existing thread. A
low/Unknown initial fact is recorded before the existing failed-start cleanup.
Direct/scheduled task admission follows replay resolution. Lease admission and
queued transfer authorization use the same predicate; rejection of a successor
must still release the previous owner. Lease-free observation and monitor windows
also require this admission.

If a monitor captures a frame and its subsequent Artifact byte admission is
refused, the capture remains recorded as performed. The existing monitor failure
owner records the failed attempt, unperformed recognition and capacity reference,
then keeps the monitor loop running. Actual Artifact I/O failures remain fatal.

ArtifactStore consumes Runtime's read-only committed projection at stream open,
each stream write, and prepared-byte commit. Trusted terminal/error/release owners
select Drain in Rust; there is no client or resource field for it. Sealing already
written bytes completes as drain. The projection does not reserve storage: an I/O
failure can still occur after successful admission. Original error code, operation,
native detail, optional raw OS code and bounded secondary causes remain intact,
with capacity fact references as context. A real cleanup or Ledger failure is
fatal even when its primary admission refusal was nonfatal.

Runtime evidence exports share this projection, including ZIP output writes. A
target on a volume absent from the committed sample is Unknown. The ZIP writer
retains its first write refusal/error through finalization and cleanup.
The same decision accompanies output-directory preparation, temporary-file
creation, verification, publication and cleanup failures. Primary and secondary
native OS errors are retained; Host classification follows the original error's
fatality.

Detachable offline resource tools do not construct a production Runtime capacity
owner. Formal offline Ledger maintenance retains its existing ownership, inactive
recovery and error rules.
