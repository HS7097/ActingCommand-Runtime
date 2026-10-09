# GlobalLedger storage contract

`GlobalLedger` is the public fact owner and single writer behind the private
`global::store::LedgerStore` boundary. RuntimeHost opens the formal SQLite medium
through [Ledger maintenance and cutover](ledger-maintenance.md). The explicit
[SQLite candidate](sqlite-ledger-candidate.md) and Segment corpus share the same
semantic core, query, projection and subscription behavior.

## Open, ownership and append durability

`GlobalLedger::open_with_store` accepts an internal constructor. Configuration
validation precedes creation of a waiting writer thread. The constructor acquires
exclusive ownership, validates/reconstructs persisted facts and indexes, verifies
artifacts when present, and completes recovery before returning a store. It runs
on the opening thread so the existing artifact verifier may borrow its owner.
The recovered store then moves to the writer. Failed construction joins the
waiting thread; failed transfer closes the opened store and joins the thread.
The caller never receives a ledger backed by an unfinished open operation.

The formal SQLite constructor first authenticates the keyed metadata row, then
reads and verifies the history in one pass, artifacts included, reading no
further than the authenticated head sequence and within the caller's deadline.
Exceeding either returns `ledger_read_budget_exceeded`, which RuntimeHost treats
as fatal. Open time and memory grow linearly with committed history and retained
artifact bytes; no persisted verification checkpoint, archive or rotation exists.

`LedgerStore` is private, `Send` and owned by one writer. It exposes append,
scheduled settlement, query, query page, replay page, latest sequence, commit
statistics and consuming close. Projection, request validation and subscription
coordination remain in the existing writer. Open/recovery is a constructor
precondition, including artifact verification; it is not a second writer API.

Append takes a sanitized typed draft. Runtime-issued identifiers, timestamps,
origin, links, payload schema, payload, sensitivity and artifact references survive
unchanged. Only the ledger assigns sequence, starting at 1 and continuing across
reopen; an empty ledger reports position 0. `PersistedEvent` remains opaque to
consumers. The existing scheduled continuation alone may derive recovery facts
and issue their identities from an already persisted chain.

The Segment implementation serializes the typed stored record, writes a complete
line and syncs the active file before publishing the event to its in-memory
indexes and recording successful commit statistics. Only then does append return
success. Sequence overflow, malformed facts and I/O failures retain their current
errors. An error built from a `std::io::Error` (`ledger_io`, `writer_spawn_failed`)
also carries `io_kind()`, a closed `LedgerIoKind` (`not_found`,
`permission_denied`, `already_exists`, `invalid_input`, `invalid_data`,
`timed_out`, `interrupted`, `unexpected_eof`, `unsupported`, `out_of_memory`,
`other` for every other kind), so callers never parse the OS text in `detail`.
A duplicate EventId is the nonfatal `duplicate_event_id / append_event`
request error, consumes no sequence and leaves the writer usable. Error severity
is taken from the returned error, not inferred from a scenario's historical name.

A failed append is not proof of absence: write/sync or later accounting may fail
after bytes or a fact have become durable. A successful observational append is not
proof of presence after a power loss (see [Durability classes](#durability-classes-workflow-191-i)).
The writer terminates on a fatal
append/settlement error, informs subscribers and returns the error to the caller.
Callers must recover/read authoritative facts before deciding whether an effect
occurred. For external effects, durable intent still precedes the attempt and
outcome follows it; storage cannot make a device action transactional.

Close drains the existing command path, syncs the Segment medium (the SQLite medium
adds no sync at close; see [Durability classes](#durability-classes-workflow-191-i))
and releases ownership.
Successful explicit close yields `subscription_closed` to subscribers; close
failure propagates as an error. Physical Segment writer metadata, repair journal,
quarantine and rotation remain owned by `storage.rs`. Commit statistics describe
successful writes by the current owner; timings and the owner incarnation are
observations, not event identity or portable equality inputs.

## Durability classes (Workflow #191 I)

Every append has a durability class fixed by the Ledger from its payload: durable
or observational. Callers cannot choose it; a joint `append_transaction` is always
durable, and `append_durable` refuses an observational draft with the fatal
`critical_event_not_durable`. A durable append returns after its WAL commit is
synced (`synchronous=FULL`). An observational append returns after its commit is
written without a sync (`NORMAL` for that one transaction, set by a freshly
prepared statement and read back; the shared connection is restored to and
verified at `FULL` before release). It survives a process crash and becomes
durable across power loss at the next durable commit on the same database (Ledger
or State) or at an SQLite checkpoint. The table is exhaustive and a new event type
or lifecycle phase is durable by default. The Segment medium still syncs every
append.

Observational: `provider.startup_observed`; `monitor.probe_requested`,
`probe_started`, `probe_completed`, `probe_failed`; `perf.pressure_started`,
`pressure_ended`, `stutter_detected`, `summary`, `monitor_degraded`,
`monitor_recovered`; `task.evidence_indexed`, `geometry_observed`,
`recognition_started`, `recognition_completed`; `capture.requested`, `completed`,
`failed`, `pressure_changed`, `dedup_window`, `policy_changed`;
`recognition.requested`, `completed`, `failed`; `artifact.verified`,
`artifact.pin_recorded`; `runtime.fact_recorded` and `runtime.fact_invalidated`
with an instance scope; `runtime.lifecycle_observed` carrying `device.self_check`
or the phases `backend_open_observed`, `adb_target_recovery` and
`device_diagnostic_detail`. Everything else is durable, including
`perf.balance_changed`, `artifact.created`, Runtime-scoped runtime facts (the
configuration inventory) and every other lifecycle phase (`vendor_stdio_close`
included, so it reaches storage before the owner journal records the close).
The task family's `task.selection_evaluated` (Workflow #308, the decision record a
select step appends before its input; see `selection-graph.md`, section Records) is
durable as well.

WAL recovery keeps the longest valid prefix ending at a commit, so power loss can
drop only a suffix of observational commits; sequence continuity and the hash
chain stay consistent at that boundary, and every State reference to a ledger
sequence was committed after that sequence in the same WAL. Every external
effect — device input and application lifecycle, lease and scheduling decisions,
task terminals, material publication (`artifact.created`) and deletion (eviction
intent), owner-journal writes, catalog/release/state/approval transitions, client
command records, owner unlock and Runtime lifecycle — follows a durable commit.
Replies, receipts and live deliveries may name observational facts that a power
loss removes; later appends then reuse their sequences but never their event ids.
Clients must re-read after a Runtime restart instead of trusting positions kept
across it. A connection whose level could not be set, read back or restored fails
the append with the fatal `state_database_sync_config_failed`; one left below
`FULL` is refused to every later user (`state_database_sync_relaxed`). Close adds
no barrier; the Host's last append before a successful close is the durable device
diagnostic summary.

## Deferred append

`GlobalLedger::append_deferred(draft)` accepts a sanitized draft on the same writer
command as `append`: the same bounded ingress queue, the same store transaction, the
same acknowledgement-before-live-delivery order and the same writer exit rules. The
only difference is that the caller does not wait for the reply; the ledger keeps the
reply channel with the draft's event type and acceptance time. Acceptance is not
commit. A full ingress queue is still the fatal `ingress_full`: the bounded queue is
the backpressure, and no unbounded buffer exists behind a deferred append.

`confirm_deferred(deadline)` establishes outcomes at explicit boundaries. It first
drains every reply that has already arrived without blocking, then waits up to
`deadline` for the rest, oldest first. Each committed reply becomes a
`PersistedEventRef` (event id, sequence, type); each failed reply is returned
verbatim in `failed`, never merged, downgraded or summarized into another error;
replies still missing at the deadline stay pending and are counted. A zero deadline
is a drain-only confirmation. Host boundaries are `HostShared::close`,
`finish_device_diagnostics` (and the aborted-start summary) and the performance
sampling tick that hosts the summary producer, each with the writer reply wait as
its budget except the drain-only tick. A boundary that finds a failure sets the
existing `lifecycle_append_failed` latch (once the host exists) and fails through
its existing error path, so close fails exactly as a synchronous append failure did.

Writer shutdown confirms deferred appends first, with the writer reply wait as its
budget. A failed reply fails the close with that error; a reply still pending is the
fatal `deferred_append_unconfirmed`, never a silent drop. The event id returned at
acceptance exists before commit: nothing may link to it, or treat the fact as
persisted, until a confirmation reports it committed. The device diagnostic close
summary is the first deferred producer and is durable; a deferred append takes the
same durability class as `append`. `append` and `append_transaction`,
subscriptions, the critical path and the writer exit rules are unchanged.

## Prior-epoch scope close (proven or unproven)

Before Provider assembly, Host completes pending eviction recovery and capacity
preflight, then passes the opaque complete owner-journal read to the existing
writer. The original exclusive OwnerGuard remains held. The reader preserves the
4 MiB ceiling, consecutive revisions from the checkpoint base
(`contracts/owner-journal.md`), fatal complete corruption and incomplete tail
recovery. After a fold the checkpoint carries the folded blocks and proofs, which
the reader re-checks as before, and every block-start prefix, which keeps
supporting the sealed complete-read bounds. Only a v2 ConfirmedClosed suffix
without later resource use, Unconfirmed, epoch reuse or contradictory identity
supports a proven import. An inactive record, None, a v1 record or an available
OS lock alone supplies no close evidence.

The first `PriorEpochOwnerImported` lifecycle fact seals the native schema,
subject epoch, positive/final revisions, complete-read bounds and SHA-256, and
replayable positive suffix. It also fixes the authenticated Ledger prefix and the
subject's contiguous epoch upper sequence. A later startup resumes that same
sealed range; subsequent identifiers cannot extend it. Imports originate only
from the native-reader capability, never an RPC or a caller-supplied verdict.

Every import and scope close carries a `basis`: `proven`, or `unproven` with one
of three reasons. A prior owner without a native proof is still imported and
closed, once per owner epoch and once per scope, through the same append path,
links rule and startup barrier. `legacy_journal`: the owner's journal epoch
contains a record older than the v2 schema. `process_exit_only`: a consistent v2
epoch ends in a normal exit record and never recorded ConfirmedClosed.
`proof_missing`: no journal epoch for the owner, or one whose records support no
positive close (a block that ends active, reuses an epoch, contradicts its own
identity or follows ConfirmedClosed with later resource use). An epoch whose last
record still declares InUse or Unconfirmed is left open; a block the native
reader could not have produced fails startup as `prior_epoch_unproven_unclassified`.
Such an epoch reaches this startup step when the OwnerGuard released it because
its process had exited (`contracts/actingd-unlock-owner.md`, "When startup
releases the owner itself"). That release is recorded as the lifecycle
observation `prior_epoch_owner_released_by_exit` (`pid`, `started_at_unix_ms`,
`last_disposition`); it is not close evidence, the epoch stays open here, and it
authorises nothing on the device side.
The unproven import seals the observation instead of a suffix: no positive close,
the block identity when one exists and the complete-read bounds, which a later
startup must still support. Facts sealed before `basis` existed read as proven.
An unproven close authorises nothing on the device side; device writes require a
live scheduler lease. Retention treats an unproven close exactly like a proven one.

The writer visits at most `RETENTION_ROUND_OBJECTS` original closure scopes per
command, counting unknown and completed scopes, within the original maintenance
deadline and command wait limit. It appends only missing typed scope closes,
with fresh system identities, current writer epoch outside and exact old
owner/instance/run/lease or request/correlation inside. A scope close references
its exact import and original scope source. Replay checks each fact against only
its preceding authenticated prefix. Errors or an exhausted deadline stop startup;
uncertain commits are recovered from Ledger, never blindly resent.

These facts close only Runtime-owned sessions and cached Unconfirmed resources
covered by the native positive evidence. They add no task terminal, LeaseReleased,
capture summary, settlement or device action. Retention keeps original verified
and terminal owners, all success/K-T, Lab, warning, preinput, permanent and material
protections. Synthetic close also requires the object's original references to
remain within the sealed range; later references/pins restore protection.
Admission of an Explicit pin release or an eviction Intent checks the exact
synthetic close source. Ordinary close authority is unchanged; replay validates
historical Intents for integrity only (see below).

## Reads, projections and subscriptions

`verify_transaction_event(&RuntimeDatabase, &RuntimeTransaction, &PersistedEvent)`
checks an already verified opaque fact synchronously inside the same owner's
borrowed transaction. It authenticates the ledger metadata/format, the exact
sequence's stored bytes by hash/tag and predecessor hash, and compares the fact's
identity, index columns, link row and ordered artifact metadata with that row.
It also binds the fact's payload and schema version to the stored record: equal
canonical bytes pass; otherwise the strictly decoded stored record must equal the
fact's record structurally (`ledger_record_mismatch` if not). A rejection never
comes from re-serialization alone.
Missing or inconsistent rows and a different Database identity fail explicitly.
The caller retains transaction/rollback ownership. The check does not acquire a
Database lock, send a writer command, commit, append or read artifact bytes. It is
an exact-row check for derived-state work; complete ledger recovery remains the
source of the opaque input and the authority for full-history validity.

`planning_signal_recovery_page(after, upper)` returns an opaque interval of at most
256 consecutive original facts. Non-planning facts remain inside the proof; the
Planning consumer sees only this interval's planning signals. Recovery fixes the
global upper sequence once, including an upper fact that is not a planning signal.
`verify_transaction_planning_page(database, transaction, page, current_checkpoint)`
checks the exact prior checkpoint, every original row in the complete interval and
the verified upper anchor in the same borrowed transaction. It performs no writer
call, material read, connection acquisition or write. Missing or inconsistent
interval rows and a mismatched checkpoint fail explicitly.

A new `PolicyPlanningSignalObserved` uses the existing joint append path with its
State-owned signal, optional cumulative detection quota and checkpoint projections.
Their namespaces, keys and encodings remain unchanged. State checks the prepared
baselines and exact signal identity, quota position and checkpoint monotonicity
before writing through the shared transaction. Host publishes its prepared quota
cache after COMMIT and releases the policy lock before downstream observations and
Timeline/Drift wakes. Historical recovery prepares and verifies each complete page,
then commits its projections and checkpoint together without adding a new signal.
Empty planning intervals still advance to their global through sequence; an empty
ledger produces no checkpoint. Historical quota arithmetic uses the original
catalog and recorded cumulative usage. Rollback/COMMIT uncertainty stays fatal
with original details, and post-commit failures retain the actual committed fact.

Store reads operate on the verified committed snapshot already materialized at
open and maintained by append. They perform no fallible storage I/O, so this
private interface returns values directly. A future backend using this boundary
must preserve that property; it must not convert a failed database read into an
empty query result. Large/lazy database reads require an explicit fallible
boundary in their assigned stage.

`query` returns matching facts in sequence order. All filters are conjoined;
sequence bounds and minimum severity are inclusive, typed links/module/source/
diagnostic/type comparisons are exact. `query_page` additionally intersects the
exclusive-after/inclusive-through interval and limits matching rows. The public
writer accepts page sizes 1–1024 and `after <= through`; invalid query and
projection pages return `invalid_query_page`. A through bound pins the result
even if later events have committed. See [query conditions](global-ledger-query.md).

`replay_page` uses the same `(after, through]` interval in sequence order, without
a filter. The subscription owner validates replay sizes (default 256, maximum
1024), registers the subscriber and captures the current high-water mark in one
serialized writer command. History is fetched in bounded pages; committed events
above the mark are delivered live. Future cursors suppress earlier events.
The writer sends an append acknowledgement before live delivery, both after
commit. A reconnect can replay from its last observed sequence when a process
exits between commit and notification. Live queues are bounded (default 64);
lag reports `subscription_lagged` and removes that subscriber. Timeout, clean
closure, dropped receivers and writer failure retain distinct existing behavior.

All seven existing `ProjectionProfile` values retain their current redaction:
Cli/Concise omit payload; Ui/Normal expose public payload; Lab/Verbose normally
expose sanitized full payload, with Runtime lifecycle/provider startup using
public payload; Forensic exposes the sanitized full form. Object-key inclusion
follows the existing projector. Projection never changes stored facts or query
membership. Profiles are not extended by the SQL views below.

Scheduling outcome projection stays in the writer: it pins a ledger position,
finds the exact run's terminal, admission and lease facts with bounded uniqueness
queries, then validates their full typed identity and order. Request partitioning
cannot hide a second terminal in the same run. Scheduled settlement accepts only
policy-owned outcome semantics, rechecks persisted admission/effect/release facts,
and appends at most one execution and one completion. Repeated reconciliation
returns the established completion and publishes only newly appended events.
An interrupted multi-event continuation is recovered from facts; S0 does not
claim an atomic multi-event transaction.

## Recovery, artifacts and forensic consumers

`GlobalLedger::open_metadata(GlobalLedgerEvidenceConfig)` returns an immutable
`GlobalLedgerMetadata` for an explicit Runtime state root. It selects the formal
SQLite or Segment reader through existing metadata and retains the supplied read
budget. SQLite validates canonical records, sequence and IDs, integrity tags,
relation indexes, head metadata and the complete migration marker/prefix. Segment
uses its original bounded snapshot scan and reports the verified prefix and any
corrupt tail. Neither path opens referenced artifact content.

`GlobalLedger::open_selected(root, event_types, deadline)` returns the events of
the given types. On SQLite it authenticates, in one read transaction, the keyed
meta row, sequence contiguity (`COUNT`, `MIN`, `MAX` equal to head, 1, head), the
head row against the meta head hash, and each selected row on its own (decode,
metadata, stored sequence, predecessor hash, tag and index columns), with its
link and artifact rows; a head of a selected type is read and checked once. It
uses `ledger_view_index_type_v1` when the derived views exist and scans
otherwise. It does not read other rows, run the retention replay or check the
migrated prefix, so it does not establish their integrity; artifacts are
Unrecorded. Its `query` refuses a query that names no selected type, names a
type it did not select, or names a view (`ledger_selection_query_unsupported`).
Segment roots are read whole.

Only the Ledger can construct its private `LedgerEventMetadata`. It retains the
typed envelope/payload and structurally valid `ProjectedArtifactReference` values
with their original object keys. It has no conversion to `PersistedEvent` or
`VerifiedArtifactReference`. The verifying openings (the candidate writer through
`open_sqlite_candidate_with_artifact_verifier`, and `open_read_only` with a verifier for
Segment evidence) still require the ArtifactStore verifier and exact verified-reference
equality; the formal openings read no material (Workflow #375 R5a, below).

`GlobalLedgerMetadata::project_view_page(&EventQuery, ProjectionProfile,
&RuntimeEventQueryPageRequest)` and the online
`GlobalLedger::project_view_page(EventQuery, ProjectionProfile,
RuntimeEventQueryPageRequest)` share the existing neutral projector and return
`GlobalLedgerResult<RuntimeEventQueryPage>`. Online requests use one read-only
writer command forwarding to `LedgerStore::project_view_page`; append, settlement
and notification order are unchanged. The projector receives the verified full
snapshot boundary and completeness, preserves complete related-run context, then
applies the output profile. Pages report `material_read: not_requested`; source
incompleteness is separate from count/byte pagination. The snapshot also exposes
`latest_sequence`, `read_complete`, `writer_metadata`, `backend` and `corrupt_tail`
without granting material access. CLI metadata pagination uses this entry rather
than a material-verifying evidence open.

SQLite page selection uses the six `ledger_view_*_v1` SQL views generated from
`LedgerView::definition()`, with indexes for type, source, module, severity
and time. Their versioned definitions are checked as one derived schema. Initial
creation, import and the existing writer's schema upgrade deploy them in one
transaction; the authenticated fact format, marker and ordered-u64 encoding stay
unchanged. Supported offline roots predating this read schema execute the same
definitions as read-only CTEs. Partial or conflicting derived definitions fail
closed. Offline opening and pagination never create schema objects.

The physical `RuntimeDatabase` owner supplies one read transaction for full
record/index/marker verification, SQL filtering, Lab links and page context. All
typed query conditions are conjoined through bindings. Diagnostic-code selection
uses the original typed payload projection from that verified snapshot. Lab's
closed request/correlation paths, directly or through the same run, share their
definition with the neutral selector; the earliest valid relation position keeps
future anchors and run links outside an older snapshot. Offline pages retain the
opened prefix hash and boundary while revalidating the current complete ledger.
An excluded corrupt row or changed prefix fails the read. A terminal online read
failure reaches subscribers and terminates the writer through its existing error
path. SQL candidate rows do not replace the full related-run recovery context or
its 1024-event bound. An empty match retains the verified global snapshot position.

Within one SQLite verification call, the private result retains the original stored
records and the metadata already constructed and structurally validated for each
record. It is returned only after all rows, relations, head and migration-marker
checks succeed. Offline and read-only query preparation consumes that metadata
after the requested prefix/hash check, retaining its per-item budget checks and
retention annotation. A Runtime query borrows the writer prefix built from this
metadata instead; its retention and event indexes advance only with rows that
the read transaction authenticated (Workflow #191 C). The original records are
released before preparation uses the metadata.
Metadata opening uses the same authenticated result; recovery and schema upgrade
continue to consume the original records. These representations belong only to
the current call and confer no artifact availability capability.

The shared SQLite row projector has a write side and a verification side. Append and
import serialize the complete stored record for canonical bytes, hash and integrity
tag; this write-side canonical form is unchanged. Verification (full and tail
recovery, read-only and forensic opens, `verify_transaction_event` and Release source
authentication) takes the stored bytes as they are: ledger integrity is the hash chain
plus the keyed integrity tag over those stored bytes, compared with the stored hash,
predecessor and tag columns. The index columns must match the decoded record (for
`verify_transaction_event`, the supplied fact). The index-column view borrows only the
original identity/type/origin/links/schema and ordered artifact fields; the same field
serializers and SQL column extraction preserve string/null handling and ordered-u64
values. Decoding stays strict and the decoded record is not re-serialized for
comparison, so an additive `serde(default)` field on a persisted contract structure
does not affect the readability of an existing ledger's rows. A Segment cutover's
imported-prefix content digest and source head digest are computed over the stored
prefix bytes (the same length-prefixed algorithm the import records over its
write-side canonical bytes) and compared only after every row, relation and the meta
row are authenticated. The cutover marker is authenticated as its stored text under
the keyed meta tag, plus strict decoding and record validation; its decoded record is
not re-encoded. A hash, tag or index-column mismatch remains a fatal
`ledger_record_mismatch`.

Segment recovery validates strict typed records, schemas, sequence continuity,
unique EventIds and payload/link/reference consistency before rebuilding indexes.
A dangling final segment tail is quarantined and repaired through the persisted
repair journal, with one recovery event; complete corruption and corruption in a
non-final segment fail closed. Existing crash-boundary and writer ownership
specifications remain authoritative for this physical backend.

The formal SQLite openings (the startup writer open, `actingd unlock-owner` and every
`ledger-maintenance` pass) read no artifact material (Workflow #375 R5a): they never
open, hash or stat a referenced file. A reference with an authenticated eviction proof
takes the proof's state, a `Failed` outcome included (`FailedEviction`); any other
reference is restored `Unrecorded`, so a missing or damaged artifact never fails
startup. Every row still carries the reference's object key, byte count and SHA-256,
so a later read checks the bytes it reads. Only the verifying openings still read
material: `open_sqlite_candidate_with_artifact_verifier` and `open_read_only` with a
verifier (Segment evidence). For them, missing verification or a mismatched/missing
artifact fails the opening, and no reference is accepted solely because its metadata
is self-consistent. `open_evidence` with `sqlite_material_per_artifact` (the forensic
export, stability and task-evidence reads, signature matching, `actinglab resource
restore`) also verifies each unevicted artifact, one at a time: a verifier `None`
leaves only that artifact `Unrecorded`, and the caller reports it. File paths, secret fields and forged metadata retain their existing
non-disclosure rules. ArtifactStore continues to own files; the ledger owns
references and verified event facts.

The #257 forensic/query/signature surfaces continue to read the original facts.
`GlobalLedgerReadOnly` opens a verified read-only prefix without writer ownership
or mutation, retaining explicit corruption/gap reporting. Its physical Segment
metadata/tail/repair observations describe that backend and must not be fabricated
for another backend. `ledger-forensics`, performance/stability exports and
[diagnostic signatures](diagnostic-signatures.md) consume their existing event
and snapshot contracts. Signature catalogs, versions, matches and retirements
are derived from ledger facts; replay creates no second fact or signature store.
S2/S5 must provide the corresponding database read-only source while preserving
portable event results and exposing backend-specific gaps accurately.

## Bounded contract corpus and evidence

`global/tests/store_contract.rs` accepts a ledger-opening function, so the same
writer assertions can run against a private candidate store. Existing named
tests remain entry points for the production Segment constructor. Their bodies
and assertions cover all query links/filters and reopened indexes, bounded pages,
exact-run outcome uniqueness, one-shot settlement, terminal writer propagation,
and paginated signature catalog/replay behavior.

The additional three-event `event_trace` accepts one already-minted draft array.
It captures full persisted facts, canonical typed records, all profile projections
and the duplicate error; it checks durable reopen equality, replay/live ordering,
pinned pages, clean close, sequence and all draft fields. S2 passes the *same*
array to Segment and SQLite on separate empty roots, compares the traces directly,
and runs the reusable assertion bodies for both. Identifiers/timestamps are not
normalized away. Recovery-generated facts additionally require their existing
identity/order/uniqueness assertions; persisted recovery corpus import must retain
the actual identities. S0's Segment run is a reference criterion, not evidence
that a SQLite differential has run.

| Existing evidence family | Criterion retained / S2 application |
| --- | --- |
| `global/tests.rs` and `tests/store_contract.rs` | Append/reopen sequence, every typed query filter, page bounds, exact-run identity, settlement/recovery protection, subscription timeout/lag/failure and signatures. Factory-based cases run unchanged for both stores. |
| `global/v2_tests.rs` | Opaque facts, typed reconstruction, profile redaction, duplicate/unknown JSON layers and artifact verification/forgery rejection. Reuse typed records and mutations with a backend-owned physical seed adapter in S2. |
| `global/recovery_tests.rs` | Prepared repair resumes; invalid journals/transitions fail; kill boundaries produce one recovery event. Retain Segment repair coverage, map the same semantic crash outcomes to the S2 database commit/recovery boundary. |
| `global/read_only_tests.rs` | Read-only bytes remain unchanged, queries match live indexes, incomplete/corrupt prefix is explicit, no writer contention. Preserve physical Segment observations and compare portable queries on both backends. |
| `ledger/tests/global_ledger_process.rs` | OS-lock ownership after process exit, crash after intent never forges outcome, append failure blocks side effects, correlation and secret absence. Keep the existing process/fixture route; no live device execution. |
| Existing scheduling, CaptureSummary, forensic and signature consumers | Reuse #95/#96 and #257 specifications unchanged for the S2 candidate; their public contracts cannot depend on store layout. |

CI executes this corpus through the existing workspace test entry. Required
format, workspace/all-target Clippy, workspace tests and native identity/Windows
gates bind the exact candidate. Local work is source editing/formatting only.
Raw first failures remain at their original CI records. No S0 real ledger import,
model/provider execution, device activity or second production writer is needed.

## Approved terminal views and frame retention

These are staged additions, with implementation and validation assigned below.
They do not change the current S0 event/profile results. Each view is a SQL view
over the one ledger; a row may belong to several. Consumers query through Runtime
host, which owns access to the database.

| View | Selected facts |
| --- | --- |
| Event stream | All events, the common base. |
| Observations and operations | Page decisions, OCR values, candidate pages, projection snapshots, input intent/commit and before/after frame references. |
| Changes | Package loading/unloading, dispatch/admission, leases, lifecycle and process state. |
| Errors | Existing Warning, Error and Fatal severities. Recovery status is derived from a failure and subsequent success in the same run, never stored as a mutable flag on the original row. |
| Health | Resource samples, stutter, clock jumps and disk capacity observations. |
| Lab | Source/session-filtered ledger facts with large debug artifacts mounted by ledger reference. |

The type lists of the Observations and Changes views are compiled into the stored
view DDL, which every open compares with its own definition
(`ledger_view_schema_mismatch`). A new event type therefore joins a type list only
together with a view schema upgrade; until then it belongs to the event stream and,
by severity and request context, to Errors and Lab. `task.selection_evaluated` is
in no type list.

Actor remains audit provenance. Sensitivity is an indexed event column; the
sending-side personal-information switch removes account/player identifiers from
outbound content. Exact view predicates and run recovery evidence are frozen with
their S4 query implementation, without interpreting unrelated success as recovery.

Text events append durably without eviction. Warning-or-higher events pin and
persist their referenced frames and a bounded preceding window in the same run;
Lab frames are pinned. Ordinary successful-operation frames may be evicted under
pressure while the ledger keeps their original hash/reference and the view reports
the registered hash with an evicted-frame state. This requires explicit retention
evidence: retention cannot silently reinterpret file loss as authorized eviction. A
missing file without an eviction proof stays `Unrecorded`; since Workflow #375 R5a no
formal opening reads material, so it no longer fails verification.

Failed or cancelled runs may satisfy the status condition through the configured
K/T policy: by default, three later successful runs with distinct RunIds on the
same InstanceId, or seven days from the original terminal's ledger timestamp.
The original Ledger writer derives a per-run terminal index and per-instance
ordered successes from the authenticated prefix; reconstruction spans owner
epochs without resetting the count. Duplicate/ambiguous terminals and missing
instance, owner or terminal time do not qualify. Selection and guarded admission
use the same rule, with a fresh writer-side check before sealing.

An eviction intent contains exactly one of the original `success` source or
`failed_run` evidence. The latter records the original terminal reference and
outcome, its timestamp, effective `successor_successes`/`retention_days`, the
writer evaluation timestamp, and either the ordered successor terminal references
or `elapsed_time`. K takes precedence when both conditions hold. Day expiry uses
checked elapsed time; K is ordered by committed sequence. The intent's own ledger
timestamp equals the frozen evaluation time. Guarded admission reproduces the
basis from that prefix and those sealed parameters, independently of the current
configuration; replay checks the evaluation time and that every sealed terminal
reference resolves, and does not reproduce the basis. Historical success intents
retain their required original proof; no missing historical facts are synthesized.

The failed/cancelled branch retains verified material identity, confirmed close,
capture summary and matching scheduled settlement. Scheduled failures use their
existing failed execution settlement; the production scheduled-cancellation path
already commits a failure terminal and settlement. Non-scheduled cancellations
retain their cancelled terminal. Warning/nearest-frame evidence, published-fact
artifacts, input before-frames, Lab association, permanent pins and
unlinked-warning protections remain. A CaptureSummary pin marks evidence only
and protects nothing; the writer's own stale-owner recovery report
(`ledger.recovered`, reason `stale_owner`, `affected_bytes` 0) is not an
unlinked warning. K/T eligibility does not imply that every failed frame can be
removed.
The same try-only material guard, intent-before-action/outcome-after-action and
round limits apply. Recovery consumes sealed pending intents without changing
their policy or re-judging eligibility. Under the same try-only guard it
re-reads the material: absent material records `recovery_absent`; material
still present with the sealed byte count and SHA-256 is removed once under the
original Intent and records `deleted` (or `failed`). Nothing is unlinked
without that verification. Disabling periodic retention leaves that startup
recovery intact. No unpin or synthetic-close permission follows
from K/T.

Explicit Lab unpin uses the existing `LabRequest` event type with a closed
`ClientPayload::LabPinRelease` target (artifact ID and pin sequence/event ID).
The Host first persists that Lab/Lab request, then the sole Ledger writer resolves
the identity and validates the exact active Lab pin before appending the original
`PinReleased { identity, pin, release }` fact. For Lab pins, `release` references
that exact typed request; Explicit pins still require their original confirmed
close, and that close must follow every pin of the object. A capture with no run
and no lease closes on its epoch's latest quiescence; when that quiescence precedes
the pin, the object is kept until a later close (Workflow #332 R-san).
Warning/DirectEvidence pins remain unreleasable. A repeat returns the
original durable release; a sealed eviction intent is always rejected.

The retention index rebuilds released Lab pins, material-reference scopes, Lab
anchors and run-to-request/correlation link positions from the committed prefix.
Only an object with explicit Lab release evidence and no remaining Lab pins can
pass the historical Lab-association check. The maximum covered prefix is its
released pins' original request sequence. Later Lab anchors, material/frame uses
in a Lab-related scope, or newly linked Lab scopes restore the protection; old
release evidence cannot cover them. Missing or ambiguous coverage keeps the
material. Intent admission performs this check at the sealed Intent prefix;
later releases never supply earlier permission. These indexes have no separate
durable store. The Runtime SQLite view prefix holds an
in-memory copy advanced with its authenticated tail and discarded with it; it has
no separate durable store either.

CaptureSummary pin reasons mark evidence only: since Workflow #332 H2f no summary
pin, historical ones included, grants retention. Warning/nearest-frame evidence,
published-fact artifacts, input before-frames and unlinked-warning protections
remain. Releasing a Lab pin does not remove any of those protections or create
close, success or K/T evidence. Material guards, the sole eviction
admission/outcome chain and pending-intent recovery are unchanged; no material
I/O occurs in the unpin command.

Replay validates integrity, not eligibility (Workflow #332 H2f Q3). Eligibility
(Warning/nearest-frame, input-before, Lab and unlinked-warning protections, active
pins, close, success or K/T, capture summary and settlement) is decided only by the
sole Ledger writer when it admits an Explicit pin release or an eviction Intent.
An Explicit pin release is judged once, at its append; only an eviction Intent
is judged at both admission points, when the writer builds it and again at its
append before it commits. Replay (every open, every
read face and the writer's own startup) checks, after the backend's record
authentication (keyed chain authentication on SQLite roots; the legacy Segment
read-only parser, kept for forensics and migration, has none), integrity only:
the contract structure, origin and links; that the object is in
the index with the same identity and verified source, its identity anchored in
the object's first, scope-checked pin; no earlier proof; that the Intent prefix
equals its position and a failed-run evaluation time equals its ledger time; that
every referenced source resolves in the preceding prefix; that an outcome answers
its exact Intent; and that sealed material is never used again. A reader never
rejects a root because its own eligibility rules differ from the writer's. A
missing file without an Intent and outcome is restored `Unrecorded` (Workflow #375
R5a: formal openings read no material). Recovery of
a pending Intent consumes the sealed Intent and does not re-judge eligibility.

The frame owner reuses `frame_store`'s three watermarks, near-duplicate handling and
pinning. ArtifactStore owns pin/persist/evict actions and file integrity, while the
ledger owns immutable references and the derived query result. The same design
connects #287 disk-capacity observations and #97-P7 `enforce_retention`; memory
watermarks alone do not authorize disk deletion. The S4 package must freeze the
run-window bound, pin/retention evidence and I/O failure handling before enabling
these actions. No S0 file retention or verifier behavior changes.

### Frame classes (Workflow #375 R5c)

The frame retention view classes every frame for the cleaner and the listing. It is
read-only and writes nothing: it is derived from the committed prefix and evaluated at
the ledger head and the caller's clock (`GlobalLedgerEvidence::frame_retention_view`
for a reader, which builds the retention index once per opening;
`GlobalLedger::frame_retention_view`, the writer's own index, for the cleaner). The
classes are cached by head and switches (R5d): at an unchanged head a call only applies
its clock. Kept folders and file names use the machine's local time, through the one
local-offset function of the ledger crate (`local_time::machine_local_offset_ms`); an
instant the system cannot convert refuses the view as a request error. It never
consults the unlinked-warning latch, the Warning ring or the K/T policy, and it adds no
persisted structure.

- **Frame.** A `capture.frame` artifact with its `PinRecorded` identity (instance,
  request, correlation, run, lease) and no eviction proof. Its time is the reference's
  `created_at_unix_ms`, the capture time.
- **Near-duplicate markers** are the existing `capture.dedup_window` records with a
  `preserved_frame_id` (material preserved). The frame store marks a persisted frame
  when its recognition is recorded, against its predecessor and against an already
  recognized successor: the same matched page, or "no page matched" on both, and a
  16x9 thumbnail similarity above 0.95 (not configurable). It never marks a frame
  labelled `initial` or `after-input`, a frame pinned at capture, a `Failed` verdict or
  a frame still `Pending`, and it marks a frame at most once, whichever of always-on
  marking and Tier1Dedup runs first. The host marks an observe capture against the
  previous observe capture of the same instance and origin (a `Lab` or an `Explicit`
  retention pin), captured less than 50 s earlier, in memory per process. A marker
  links the representative (the predecessor) and preserves the marked frame. A frame
  is **interior** when it is marked and another marker links it as the representative.
- **Error points.** A non-retention, non-`perf.*` event on a known instance (its own
  link, or its run's instance) that is a `TerminalCommitted` with outcome `Failure` at
  any severity, a run-linked event at Warning or higher, or an instance-linked event
  without a run at Error or higher. Its t is its ledger timestamp, except that a point
  on a run with no terminal in its own owner epoch, appended after that epoch ended, is
  dated at the run's last frame. A run's owner epoch ends at the first
  `runtime.started` or `runtime.takeover` after the run's first event. A run with frames
  and no terminal in its own epoch also gets an **epoch-end point**: its sequence is
  that start event, its t the run's last frame time.
- **Error window.** The frames of the point's instance captured in `[t - 30 s, t]`
  (`ERROR_WINDOW_MS`), and the frames the point's event names.
- **Settled.** The frame's entry is closed, the frame is at least 60 s old, and no run
  on its instance whose first frame is at most 30 s after it is still without a
  terminal while its epoch goes on. A Lab debug-package run (`task.requested` with the
  action `runtime.debug_package`) commits no `TerminalCommitted`: its `task.completed`,
  `task.failed` or `task.cancelled` is its terminal, and it never holds back the frames of
  its instance. A run closes by its terminal, its capture summary
  and a later `Performed` release of its lease (its own, or a run-less one), or by the
  end of its owner epoch; a run-less frame closes when its capture completes. The entry
  time is the terminal's timestamp, the ending start event's timestamp when that comes
  first, or a run-less frame's capture time. An unsettled frame is `running`.
- **Exempt** (kept even when interior): a capture-summary pin other than
  `recognition_evidence`; a frame an `input.intent` names as its before frame; an
  observe frame (one no capture summary names) whose capture carries a lease, that is,
  Lab operation evidence; a frame a `fact.published` names (as an artifact, or by the
  run and frame of a resource reading's snapshot id); a frame an error point names.
  The retention pins `Explicit` and `Lab` exempt nothing. A run closed by the end of its
  epoch without a capture summary has no duplicates, and no window drops its frames.
- **Classes**, in order of precedence (switches from `actingd.config.json`, revision 4):

  | Class | Test | Due |
  | --- | --- | --- |
  | Lab | an unreleased Lab pin or the index's Lab protection; with `frame_retention_dedup_lab` on, an interior, non-exempt Lab frame falls through | moved to its Lab folder at settle; deleted by people |
  | error | kept by at least one window; with `frame_retention_dedup_error` on (the default), a window drops an interior, non-exempt frame whose predecessor and successor are both in it | moved to its error folder at settle; deleted by people |
  | resource | a frame a `fact.published` with a `resource_reading:` detector names, whose snapshot id names the frame's run | 7 days after the entry time |
  | duplicate | interior and not exempt | at settle |
  | default | everything else | 1 day after the entry time |

- **Kept folder** (relative to `<state root>\kept`): `<YYYY-MM-DD>\<leaf>\<HHmmss-fff>_<object
  file name>`, in the machine's local time. An error frame belongs to the
  lowest-sequence point whose window contains it; its leaf is
  `<instance alias>-<sequence>-<code>`, dated at that point's t, where the alias is the
  one bound at the point (for an epoch-end point, at the run's last frame) and the code
  is the point's own `failure_code`, else its own code (a `rejection.code`, as in a
  `policy.dispatch_rejected`, before any other `code`), else the failure code of its
  run's failure terminal when that terminal precedes it, else its event type with `.` as
  `_` (an epoch-end point: `runtime_takeover` or `runtime_started`); characters outside
  `[A-Za-z0-9_-]` become `_`, and the code keeps at most 64 of them. A Lab frame's leaf
  is `lab-<instance alias>`, dated at its capture.

### Frame cleaner (Workflow #375 R5d)

The cleaner runs in actingd while `frame_retention_enabled` is on. It writes nothing to
the ledger: no eviction intent or outcome, no proof. It no longer calls eviction
admission, so the K/T policy, the unlinked-warning latch and the seals gate nothing;
replay of eviction records already in a ledger, and startup recovery of an intent left
without its outcome, stay.

- **Rounds and sweeps** (model v4.3 note). Each round of the performance-monitor loop
  (every 2 s) acts on at most 16 frames within 1 s. A sweep starts at most every
  10 minutes (the first at the first round after the start) from the frame view at the
  Runtime's clock, and lists the settled frames that are due: duplicates, default frames
  1 day and resource frames 7 days after their entry time are removed; error and Lab
  frames are moved into their kept folder. The sweep visits them in entry-time order over
  as many rounds as it needs. Running frames and frames not yet due are untouched.
- **Never a stale class.** While a sweep still has queued frames, each round reads the
  view again (cached by head, so at an unchanged head this only applies the clock) and
  derives every queued action again before acting: a frame whose class changed during
  the sweep is acted on by its current class, so a frame that became Lab-protected is
  moved, never removed, and a frame no longer due stays where it is.
- **Only the object key.** A removal or a move acts on `<state root>\<object key>` and
  nowhere else; the cleaner never looks in `kept\`. Nothing there means the frame is
  absent: deleted by hand, or already removed or moved. So a frame in a kept folder is
  never removed or moved again, whatever its class becomes after a switch change or a
  Lab pin release.
- **File handling.** The cleaner first stats the path; an absent frame costs no action
  and takes no lock. It then takes the frame's use lock
  (`artifact-use-locks\<name>.lock`) try-only and opens the file for deletion only,
  shared for deletion only, without reading or hashing it. A held lock, or os error 32
  or 33 on the open, the unlink or the rename, is **busy**: the frame is retried by the
  next sweep. Any other error is **failed**: one Warning (below), and the frame stays
  until a restart.
- **Moves.** The cleaner creates the kept folder if needed, puts the frame into its own
  kept map, renames the file, and then overwrites `kept\.moves` with its process start
  time and its move count (16 bytes). A failed rename takes the map entry out again. A
  rename that finds no folder (a leaf deleted by hand right after its creation) while
  the frame is still there creates the folder again and renames once more.
- **Done set.** The process remembers the frames it removed, moved or found absent, and
  never visits them again. After a restart, the first sweep finds the frames removed or
  moved before it absent, and counts them `rescanned_absent`.
- **Lock.** A round takes the cleaner's lock try-only and skips while `clear-kept` holds
  it.
- **Fail Loud.** One stdout line per completed sweep that visited frames, and always for
  the first sweep: `actingd frame_retention pass frames=<n> deleted=<n>
  deleted_bytes=<b> moved=<n> moved_bytes=<b> absent=<n> rescanned_absent=<n> busy=<n>
  failed=<n> kept_error=<n> kept_lab=<n> running=<n> rounds=<n> pass_ms=<ms>`. `frames`
  counts the due frames the sweep visited; `kept_error`, `kept_lab` and `running` count
  the settled error and Lab frames and the unsettled frames of its view. With the
  cleaner off, actingd prints `actingd frame_retention disabled` once at start and runs
  no sweep. A failed removal or move appends one Warning `runtime.failed` with system
  links, once per object per process, whose detail carries `host_code=`
  `frame_retention_remove_failed` (`artifact_id`, `io_kind`, `os_error`) or
  `frame_retention_move_failed` (adds `entry`, the kept folder relative to the state
  root). A move whose counter `kept\.moves` could not be written counts as moved and is
  reported once per process by its own code, `frame_retention_counter_failed` (`entry`,
  `io_kind`, `os_error`). Only a ledger error, including a failed append of such a
  Warning, is fatal; a held file never is.

### Moved-frame reads (Workflow #375 R5d)

Every material read goes through `open_projected_stream` (Runtime material reads,
failure comparison, evidence export, planning, signatures, `actingledger`, `actinglab
resource restore`, `ledger-maintenance restore`). When nothing is at the object key, it
looks the artifact id up in a per-process map of the kept folders, so every process
resolves alike.

- **Ids.** A kept file is `<HHmmss-fff>_artifact_<hex>.png`; its artifact id is the text
  after the first `_`, without `.png`. Only `artifact_<hex>.png` objects are looked up.
- **The map** is keyed by the canonical state root. A cached path that is gone (a leaf
  deleted, renamed or copied by hand) is dropped and the read counts as a miss; a frame
  in no kept folder is `artifact_read_failed` with `NotFound`, which readers report as
  missing (R5b).
- **When a miss walks.** The first miss in a process walks `kept\`. Later misses walk
  again when `kept\.moves` differs from the value read just before the last walk (an
  absent file reads as empty), or at most once per 60 s for changes by hand. actingd,
  the only mover, puts its own moves into its map before each rename, so it never misses
  them; misses of deleted frames with no move cause no walk.
- **The walk** lists the date folders and their leaves below `kept\`, and the files in
  each leaf, reading no file contents and never following a reparse point. `kept\`
  absent is an empty map. An entry that vanishes or is delete-pending (os error 303)
  during the walk, and a file where a folder is expected (`.moves`, `desktop.ini`), are
  skipped; a name that does not end in `_artifact_<hex>.png` is ignored. A `kept\` that
  is itself a reparse point (a junction to another drive, say), and any other listing
  error, fail the read with `artifact_kept_walk_failed` (`entry`, `io_kind`, `os_error`;
  model v4.3 note), and the previous map stays: `kept\` must stay a plain folder on the
  state root's volume. Each walk replaces the map whole, and one root has one walk at a
  time: a reader that misses meanwhile waits and uses that walk's map, unless
  `kept\.moves` changed since that walk began, when it walks again.
- **A read during a move.** The cleaner holds the frame for a moment; a reader then gets
  os error 32 or 33, or a held use lock, not `NotFound`. The whole open, the object key
  and then the lookup, is retried up to 3 times 20 ms apart.
- **Restore.** `ArtifactReader::resolved_relative_path()` names the file a reader
  opened. `restore_recovery_reference` writes a frame found in the source's `kept\` at
  the same `kept\` path in the target, verifies it there with the published file's
  bytes, and puts it into the target's map.

## S1–S5 ownership and acceptance map

The S1 physical owner and preserved state assembly are described in
[Runtime database owner](runtime-database.md).

The future source-tree `PackageRef` belongs to the separately frozen package
identity/containment contract in #288. Its issuer, source-tree identity and ledger
representation require coordination at that shared boundary. S0 preserves current
package facts and artifact references for the storage comparison. Parallel policy
time/window work (#267) and parser-owner relocation (#288) retain their own
owners; the six SQL views remain assigned to the stages below.

| Stage | Concrete boundary and remaining proof |
| --- | --- |
| S1 | Extract `RuntimeDatabase` as the physical SQLite/path/connection-mutex/key/tag owner, including existing startup and schema-version checks. RuntimeState supplies its schema and retains business transactions and behavior; the database owner has no business-crate dependencies. Database backup/restore and general schema upgrades belong to S3/S5. |
| S2 | Add private candidate `SqliteLedgerStore` through the same writer. Store canonical records, indexed event columns/links/artifact order, sequence and integrity metadata in one append transaction. Verify canonical hashes, chain/head and index consistency; run actual same-input differential and crash/verification matrix. Production stays on Segment until cutover. Prepare sensitivity columns and the schema needed by the terminal views. |
| S3 | Freeze the real migration object separately. Stop its owner, back up both stores, verify segments/artifacts, transactionally import unchanged identities/sequences/timestamps, compare results, then publish one atomic cutover marker. Prove interrupted import/reentry and backup/restore. No live dual write. Once SQLite contains new events, recovery uses SQLite backup/restore; never automatically restart the old writer. |
| S4 | Runtime-owned coordination makes database-internal catalog/release/state/projection changes and their ledger facts one transaction. Implement the six host-query views, sending-side filtering and derived run recovery status; connect frame pins/persistence, disk capacity and retention with explicit owner evidence. External effects keep intent-before-act/outcome-after-act. |
| S5 | Retire the production Segment writer and physical-layout dependencies, retain a bounded legacy importer, complete read-only database forensics and maintenance/backup/restore entry points. |

The complete acceptance matrix is retained by stage, with no S0 claim of future
proof: (1–2) identical facts and every original field, (3) query/page/projection/
scheduling equality, (4) gap-free nonduplicating replay/live and (5) exact-run/
CaptureSummary equality belong to S2; (6) verifier failure closes startup and
(9–10) commit-before-notify replay plus tamper/missing rows/duplicate sequence or
EventId/bad chain/bad link index rejection belong to S2 and cutover verification;
(7–8) interrupted import and zero old writes, and (13) dry-run/backup/restore
position/hash/projection/reference equality belong to S3/S5; (11) database-internal
crash atomicity belongs to S4; (12) external intent/outcome ordering and (14) no
consumer SQL write path apply throughout; (15) production independence from
Segment layout, excluding the importer, completes in S5.

After the approved differential is complete, add no tests or tooling. The
retirement plan inventories the existing corpus, backend fixtures, process
harnesses, guards and CI entries, associates each with the proof above and its
owner's typed module probe, then proposes exact retirements and CI changes in the
assigned stage. Duplicate ordinary evidence can retire only after its replacement
is established. Invariants, fail-closed cases, original failures and historical
evidence remain protected. This plan authorizes no deletion or CI gate removal.


## Catalog SQL transactions

The existing GlobalLedger writer accepts one sanitized catalog outcome or State migration
fact together with bounded, typed Runtime State work. The SQLite owner lends its exact
Immediate transaction to that work; a different database owner or unsupported backend
is rejected. State work cannot publish caches, call Host/GlobalLedger, or commit itself.
The independently durable catalog intent remains unchanged. Event rows, links, meta/tag
and the reserved `policy.catalog.active` document/history (plus its migration row when
applicable) commit once before Ledger indexes/statistics, ack/live and policy caches move.

A State/CAS error is returned as business rejection only after confirmed rollback. The
caller records the original failure fact while preserving Request/Fatal identity. Failed
rollback, uncertain COMMIT and incomplete post-commit publication stop the affected writer
or Host; they never claim NotPerformed or resend the successful operation. Commit readback
uses the same event position with ordered-u64-v1 encoding, exact event/link/artifact/meta
rows and State document/history/migration comparisons. Connection acquisition is nonblocking;
scans use the existing query row ceiling, 2 MiB and a checked two-second deadline, preserving
the database owner's native SQLite busy timeout. Unavailable, expired or conflicting evidence
remains Unknown. State's positive integer position encoding and integrity tags are unchanged.

Only the catalog owner writes the reserved key. Generic State document write, migration
and rollback APIs reject it with a Request error. Startup and historical catalog reads use
one ordered fold of matched intent/outcome and validated migration facts. Current State
must equal the latest effective source, including catalog identity/version and verified
material; an older matching hash is insufficient. File publication/removal, Provider,
device effects and capacity sampling remain outside the SQL transaction. The original
performance monitor and Business/Drain admission retain their own committed-fact boundary.

## Release source references

Release State persists a versioned `ReleaseLedgerSourceReference` containing the original
event identity, sequence and `record_sha256`, the record hash of the authenticated stored
row. `capture_release_source_reference` checks the opaque fact against that row in the
same borrowed RuntimeDatabase transaction and takes the row's stored hash; it does not
re-serialize the fact. Deserialization confers no fact authority. Each later
`verify_release_source_reference` checks the original record, native typed metadata,
canonical hash/tag, predecessor, indexed fields, links and ordered artifact metadata in
the caller's transaction. The returned opaque relationship exposes the original origin,
links and typed payload, with no material access capability. Only ReleaseStaged,
ReleaseActivated, ReleaseRolledBack and StateMigrated for `release.legacy.baseline` are
eligible; the Release owner checks the corresponding manifest, transition or migration.

`read_release_baseline_source` verifies the fixed authenticated global prefix in pages
of at most 256 original records before returning absence or one exact baseline source.
It reads every original record so damaged filter columns cannot conceal a boundary,
checks the chain/head and complete native index counts, and reports duplicate boundaries
with both original locators. No new fact is written. A standalone State database can
return absence only with format zero and no Ledger tables or other Ledger schema objects;
partial schema, existing format/metadata declarations and invalid rows fail. A persisted
locator always fails when its Ledger storage is absent. State also checks all its own
boundary/source/migration records before permitting first capture.

These synchronous interfaces borrow the existing transaction and neither acquire a new
connection nor wait for the writer, read artifact bytes, publish an event or commit.
Their module verification does not replace the complete Release consumer's recovery,
atomicity and required CI evidence.
