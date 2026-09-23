# Capacity admission

Runtime uses the performance owner to sample the executable installation, State,
Artifact objects and Artifact stream staging directories. Windows resolves each
target to its actual volume GUID, including junctions, and queries the bytes
available to the Runtime account once per distinct volume per cycle. An unresolved
binding or unsupported platform produces Unknown with the original native cause.
Private directory paths are not capacity-fact fields.

Each binding resolution opens its nearest existing ancestor with a temporary
attribute-query handle, follows reparse targets, and obtains the normalized final
GUID volume root. Sampling and admission use this same resolver; each call releases
its handle. Missing GUID identities, native errors and results exceeding the bounded
UTF-16 buffer remain unavailable. Only the volume root enters the capacity identity.

`capacity_thresholds` in the actingd configuration contains `hard_bytes` and
`soft_bytes`. Defaults are 536870912 (512 MiB) and 2147483648 (2 GiB), with
`0 < hard_bytes < soft_bytes`. These are initial policy values, not a measurement
or a reservation. The existing performance interval defaults to two seconds;
capacity freshness is twice that interval. Optional counter shutdown does not
stop capacity sampling.

Capacity is sampled every interval, but a sample is recorded only through the
single `perf.summary` producer. A summary is due when none has been recorded yet
in this owner epoch, when the summary interval (60 seconds by default) has
elapsed since the last recorded summary, or when the live sample changed
materially against the last recorded capacity fact: a volume (identity plus
purpose set) appeared or disappeared, a volume's state changed, its available
bytes went to or from unknown, or they moved by at least 5 % of the recorded
value or by at least 1 GiB. The recorded summary carries the host performance
context of that moment (unavailable only when counters are disabled or its
window holds no data), the latest capacity sample and the ledger commit window.
Ticks on which no summary is due record nothing.

Each small typed B3 `PerformanceSummary.capacity` is committed directly to
GlobalLedger before it can authorize business. It includes owner epoch, original
Unix and monotonic observation times, thresholds, volume identities and purpose
sets, available bytes or Unknown, and native failure detail. It needs no context
artifact. A recorded fact enters the derived view only with its append receipt;
queuing and appending do not refresh observation time. A sample that records no
summary replaces only the derived view's live sample and keeps the last recorded
fact's reference, so freshness, binding and thresholds follow the live sample.
Admission carries its decision time and the fact reference: the EventId/sequence
of the last recorded fact, which may lag the live sample by at most the summary
interval and the 5 % / 1 GiB band. A failed recording clears the derived view and
returns its error. Missing, failed, stale, future, changed-binding or
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
each stream write, and prepared-byte commit. These admissions and capacity
inheritance require an installed owner. Its absence returns fatal
`capacity_owner_missing` during `admit_artifact_bytes`, without a capacity decision
or fact reference. Opening a store for reads and verifying material remain valid
without an admission owner.

Trusted terminal/error/release owners select Drain in Rust; there is no client or
resource field for it. Drain still requires an owner. A stream retains the owner
used at its successful open; sealing its already-written bytes uses that same
owner, including when a store opened for the same root performs the seal.
Low or stale capacity decisions do not block this trusted completion. The
projection does not reserve storage: an I/O
failure can still occur after successful admission. Original error code, operation,
native detail, optional raw OS code and bounded secondary causes remain intact,
with capacity fact references as context. A real cleanup or Ledger failure is
fatal even when its primary admission refusal was nonfatal.

Runtime evidence exports share this projection, including ZIP output writes. A
capture pipeline receives its caller's existing ArtifactStore; its standalone
constructor keeps the original capture provenance and retention profile. A
target on a volume absent from the committed sample is Unknown. The ZIP writer
retains its first write refusal/error through finalization and cleanup.
The same decision accompanies output-directory preparation, temporary-file
creation, verification, publication and cleanup failures. Primary and secondary
native OS errors are retained; Host classification follows the original error's
fatality.

Detachable offline resource tools do not construct a production Runtime capacity
owner. Formal offline Ledger maintenance retains its existing ownership, inactive
recovery and error rules. Its recovery-reference restore uses a separate byte-copy
entry, outside the admission calls described above; this does not establish
capacity admission for that restore path.

Frame pressure uses the same existing `CaptureFrame` material path. FrameStore
owns resident bytes, pressure selection and recognition state; CapturePipeline
retains each frame's original context. Each selected frame is encoded or borrowed
individually, admitted as Business bytes, and committed through ArtifactStore.
Only successful Created, verification, Verified and the Host's required pin
permit the resident payload to be replaced by its original artifact reference.
Reads verify that reference and the retained original PNG length and hash.
Already published frames release resident memory without another write.

The original memory budget admits the live frame/copy, PNG workspace and material
verification buffer before allocation or publication. Host copies run inside one
synchronous pipeline operation: a fresh budget is sampled before the copy, and
the caller's original remains charged through recording, publication and pressure
polling, including failure returns. An incoming frame remains charged while
history is published, until its charge transfers to the resident entry. Each
material candidate refreshes the budget and includes both live charges alongside
resident bytes, verified-read material and the publication buffer. Encoding checks
that same live set with its codec workspace before allocating it. The original
charge is released only when the synchronous material operation returns.
RGB8/RGBA8 Fast/NoFilter
encoding reserves the locked codec's rows and both compression outputs, including
allocation overlap, and writes to a fixed output slice. Supplied original PNGs
are borrowed. No batch of encoded frames is assembled. Completed encoding releases
its workspace reservation; refused persistence keeps the actual resident charge.
Insufficient workspace returns nonfatal `frame_workspace_unavailable`, while
the original pixel-layout owner classifies malformed incoming frames before
copying or encoding. The admission error retains that category. Readonly capture
rejects that category with `capture_frame_invalid` / `CaptureFailed`, an
Indeterminate CaptureFailed and NotPerformed RecognitionFailed, and a Failed
receipt referencing the recognition terminal. The request does not poison Host
health; failure to record its facts remains fatal. Sealed-material validation,
identity/hash and real persistence/recording failures retain their fatal route.

Prepared-byte admission refusal emits the existing typed ArtifactStoreFailed
record at BeforePublication with its attempted identity and capacity decision;
it does not attach a readable artifact. Errors before preparation retain their
original frame context for the Host's existing lifecycle failure record.
Optional pressure refusals are typed per-frame results, stop further writes in
that pressure round and do not increment successful persistence or dropped-frame
counts. Explicit persistence cannot immediately retry the refused write.
The existing pressure poll may resume on available workspace and a fresh allowed
capacity decision; the original request deadline and cancellation still apply.
Required current-frame evidence must exist before an observation can succeed.

Host publication constructs one existing single-Verified sink per original frame.
History publication uses the history frame's links and pin; it cannot consume the
current frame's receipt. Frame pressure never uses Drain to admit a new PNG.
The existing stream's trusted zero-new-byte seal and publication rollback are
unchanged. FrameStore does not create segment ZIPs, side manifests or screenshot
files. Cleanup releases resident ownership only; old directories and all published
objects remain with their original Ledger/retention owners.
