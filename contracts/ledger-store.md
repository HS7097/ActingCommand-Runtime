# GlobalLedger storage contract

`GlobalLedger` is the public fact owner and single writer behind the private
`global::store::LedgerStore` boundary. RuntimeHost opens the formal SQLite medium
through [Ledger maintenance and cutover](ledger-maintenance.md). The explicit
[SQLite candidate](sqlite-ledger-candidate.md) and Segment corpus share the same
semantic core, query, projection and subscription behavior.

## Open, ownership and durable append

`GlobalLedger::open_with_store` accepts an internal constructor. Configuration
validation precedes creation of a waiting writer thread. The constructor acquires
exclusive ownership, validates/reconstructs persisted facts and indexes, verifies
artifacts when present, and completes recovery before returning a store. It runs
on the opening thread so the existing artifact verifier may borrow its owner.
The recovered store then moves to the writer. Failed construction joins the
waiting thread; failed transfer closes the opened store and joins the thread.
The caller never receives a ledger backed by an unfinished open operation.

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
errors. A duplicate EventId is the nonfatal `duplicate_event_id / append_event`
request error, consumes no sequence and leaves the writer usable. Error severity
is taken from the returned error, not inferred from a scenario's historical name.

A failed append is not proof of absence: write/sync or later accounting may fail
after bytes or a fact have become durable. The writer terminates on a fatal
append/settlement error, informs subscribers and returns the error to the caller.
Callers must recover/read authoritative facts before deciding whether an effect
occurred. For external effects, durable intent still precedes the attempt and
outcome follows it; storage cannot make a device action transactional.

Close drains the existing command path, syncs storage and releases ownership.
Successful explicit close yields `subscription_closed` to subscribers; close
failure propagates as an error. Physical Segment writer metadata, repair journal,
quarantine and rotation remain owned by `storage.rs`. Commit statistics describe
successful writes by the current owner; timings and the owner incarnation are
observations, not event identity or portable equality inputs.

## Reads, projections and subscriptions

`verify_transaction_event(&RuntimeDatabase, &RuntimeTransaction, &PersistedEvent)`
checks an already verified opaque fact synchronously inside the same owner's
borrowed transaction. It authenticates the ledger metadata/format and compares the
exact sequence's canonical row, identity, hash/tag, predecessor hash, link row and
ordered artifact metadata through the Ledger's existing private representation.
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

Only the Ledger can construct its private `LedgerEventMetadata`. It retains the
typed envelope/payload and structurally valid `ProjectedArtifactReference` values
with their original object keys. It has no conversion to `PersistedEvent` or
`VerifiedArtifactReference`. The ordinary recovery and export APIs still require
the ArtifactStore verifier and exact verified-reference equality.

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

Segment recovery validates strict typed records, schemas, sequence continuity,
unique EventIds and payload/link/reference consistency before rebuilding indexes.
A dangling final segment tail is quarantined and repaired through the persisted
repair journal, with one recovery event; complete corruption and corruption in a
non-final segment fail closed. Existing crash-boundary and writer ownership
specifications remain authoritative for this physical backend.

Artifact-bearing recovery requires the ArtifactStore verifier. Missing verification
or a mismatched/missing artifact fails startup; no reference is accepted solely
because its metadata is self-consistent. File paths, secret fields and forged
metadata retain their existing non-disclosure rules. ArtifactStore continues to
own files; the ledger owns references and verified event facts.

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

Actor remains audit provenance. Sensitivity is an indexed event column; the
sending-side personal-information switch removes account/player identifiers from
outbound content. Exact view predicates and run recovery evidence are frozen with
their S4 query implementation, without interpreting unrelated success as recovery.

Text events append durably without eviction. Warning-or-higher events pin and
persist their referenced frames and a bounded preceding window in the same run;
Lab frames are pinned. Ordinary successful-operation frames may be evicted under
pressure while the ledger keeps their original hash/reference and the view reports
the registered hash with an evicted-frame state. This requires explicit retention
evidence: a missing required artifact must still fail verification. Retention
cannot silently reinterpret file loss as authorized eviction.

The frame owner reuses `frame_store`'s three watermarks, near-duplicate handling and
pinning. ArtifactStore owns pin/persist/evict actions and file integrity, while the
ledger owns immutable references and the derived query result. The same design
connects #287 disk-capacity observations and #97-P7 `enforce_retention`; memory
watermarks alone do not authorize disk deletion. The S4 package must freeze the
run-window bound, pin/retention evidence and I/O failure handling before enabling
these actions. No S0 file retention or verifier behavior changes.

## S1–S5 ownership and acceptance map

The S1 physical owner and preserved state assembly are described in
[Runtime database owner](runtime-database.md).

The future source-tree `PackageRef` belongs to the separately frozen package
identity/containment contract in #288. Its issuer, source-tree identity and ledger
representation require coordination at that shared boundary. S0 preserves current
package facts and artifact references for the storage comparison. Parallel policy
time/window work (#267) and translator-owner relocation (#288) retain their own
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
event identity, sequence and canonical-record digest. `capture_release_source_reference`
checks the opaque fact against the same borrowed RuntimeDatabase transaction before
generating the locator. Deserialization confers no fact authority. Each later
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
