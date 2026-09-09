# SQLite Ledger candidate

The `actingcommand-ledger` feature `sqlite-candidate` exposes explicit candidate
constructors. `GlobalLedger::open` and `open_with_artifact_verifier` retain the
Segment backend. RuntimeHost production assembly and consumer protocols keep
their existing route.

`open_sqlite_candidate` and `open_sqlite_candidate_with_artifact_verifier` take a
`GlobalLedgerConfig` and the assembly's `Arc<RuntimeDatabase>`. The configuration
root must resolve to that database root, with no Segment directory. The existing
OS writer lock is anchored to this root, so two facades, including separately
opened database handles, cannot create concurrent Ledger writers. The enclosing
Runtime owner retains its existing process ownership responsibilities.

RuntimeDatabase owns the connection, mutex, key and durability configuration.
Its component initializer allows a typed owner to supply schema startup while
the State convenience constructor preserves its current initialization and
error order. Ledger depends on this low-level owner, without a State dependency.

## Event semantics and durable records

`EventStore` holds the single common sequence, committed event vector, indexes,
validation, scheduled continuation and commit statistics. `SegmentStorage` and
`SqliteStorage` implement durable persistence and close. The common writer still
owns query/projection/subscription and scheduling outcome orchestration.

SQLite restores typed events and rebuilds the existing `EventIndexes`. All
query/page/replay behavior uses that verified committed memory snapshot. SQL
failures occur at open, append or the explicit read-only snapshot boundary and
propagate as fatal errors. Duplicate EventId ingress retains its nonfatal
Request result and consumes no sequence.

The Ledger-owned [schema](../crates/ledger/src/global/sqlite/schema.sql) contains
`ledger_events`, `ledger_links`, `ledger_artifacts` and `ledger_meta`. On initial
candidate use these tables are created atomically; partial schemas fail. Existing
tables are read and verified. The canonical `StoredEventRecord` determines every
indexed event/link value and ordered artifact row. Only sequence and EventId are
event identities with unique constraints; run/task/lease values span many facts.

An Immediate append transaction checks the current head/next metadata, inserts
the complete event/relations/artifacts, updates head/next/hash/keyed metadata and
commits under WAL/FULL. Only afterward does the common store publish memory
indexes and statistics. The existing writer then acknowledges and sends live
notifications. A failed append does not establish absence: persistence may have
completed before a later failure. Fatal errors terminate the writer and reach
subscribers through the existing path.

Recovery checks typed schemas, sequence continuity, EventId uniqueness, canonical
bytes, record hash, previous hash, domain-separated integrity tag, indexed fields,
artifact order and the final meta head/next/tag. Missing, extra or mismatched
relations fail startup. Artifact-bearing records require the ArtifactStore
verifier; matching metadata alone does not establish an artifact capability.
The key domains are `ledger-event-v1` and `ledger-meta-v1`. These checks detect
inconsistent persisted material within the established store; they do not claim
protection against an owner replacing the complete database, key and lock state.

## Integer representation

All logical u64 Ledger columns use the private `ordered-u64-v1` bijection:

```text
encode(n) = (n xor 0x8000000000000000) as i64
decode(s) = (s as u64) xor 0x8000000000000000
```

The signed SQL order equals the logical unsigned order. Sequence, timestamp,
artifact size, artifact timestamp, ordinal and meta positions round-trip their
full domain. SQLite keys are explicitly assigned; the original sequence issuer
and overflow criterion remain authoritative. Canonical records and public typed
values retain their original u64 representation. The fixed meta singleton is a
schema discriminator, not an encoded event position.

State tables, their positive integer positions and their tags retain their
existing encoding. S4 handles relationships between these representations and
the approved database-internal transactions explicitly.

## Read-only candidate source

`open_sqlite_candidate_read_only` takes the same database owner and a read-only
configuration. A Deferred transaction loads one consistent snapshot of all four
tables without DDL, recovery writes or fact writes. The connection guard is
released before typed reconstruction invokes ArtifactStore. It returns immutable
facts/query/page/latest-position results; it has no Segment repair/tail/byte
observations. The existing Segment forensic source retains those observations.

An optional read budget bounds logical SQL value bytes, event count and deadline,
including metadata and typed verification. These byte counts describe copied SQL
values, not physical database/WAL file sizes. The connection mutex can serialize
this snapshot with an append; SQLite's busy timeout is not a Rust mutex deadline.
The snapshot performs no logical database mutation and the writer can append
afterward. It does not promise an unchanged WAL/shared-memory file image.

## Bounded verification and later stages

Existing CI runs the same seven factory assertion bodies against SQLite and
compares the same already-minted three-event trace against Segment. IDs and time
are compared directly. The read-only query assertions use their backend adapter.
The approved matrix adds full-domain SQL ordering/round-trip, typed high-time
facts, artifact order and CaptureSummary recovery, schema/row/meta/hash/link
corruption, unique constraints, transaction rollback with subscriber failure,
and process exit after COMMIT before append acknowledgement/live publication.
The last case reuses the existing process barrier and its bounded parent wait.
Segment repair, quarantine, ownership and process specifications remain active.

The candidate proves S2's durable-medium and semantic boundaries through exact-head
CI and independent review. Migration/cutover and backup/restore belong to S3/S5;
shared internal transactions, six SQL views and frame retention belong to S4;
production Segment retirement completes in S5. Real data and devices are outside
this candidate validation.
