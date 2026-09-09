# Runtime database owner

`actingcommand-runtime-database` owns the SQLite connection, its mutex, database
path and integrity key. Runtime shares one `Arc<RuntimeDatabase>` with its typed
state facade. `runtime-state` owns the business schema, documents, projections,
revision/CAS rules, release files, pointer consistency and transaction boundaries.
GlobalLedger continues to use its Segment backend and the
[private ledger store contract](ledger-store.md).

## Construction and file ownership

The Host creates the shared database at its existing state construction point,
through `RuntimeStateStore::open_database`, and injects it using `from_database`.
`RuntimeStateStore::open` provides the same assembly for existing callers. Each
assembly opens one connection; facades share that owner rather than reopening it.
The Host's existing OwnerGuard continues to arbitrate process ownership.

Construction preserves this order:

1. Validate the bootstrap seed length (16–1024 bytes), create and validate the root.
2. Run the state-owned release-blobs preparation callback. This happens before
   key/connection creation, preserving the original file and error ordering.
3. Inspect any existing database path; load or create the 32-byte key.
4. Open SQLite; set its five-second busy timeout, foreign keys, WAL and FULL
   synchronous mode; execute the state-supplied schema and check the existing
   `state_meta` version; run `quick_check(1)`.
5. Construct the state facade and validate every typed document, projection,
   release and pointer relation through its existing validation path.

`crates/runtime-state/src/schema.sql` and its contract mirror remain unchanged.
The state owner supplies the existing nine-table DDL and version to database
construction. The database owner performs that initialization and metadata check;
schema semantics remain with state.

The files remain `runtime-state.sqlite` and `runtime-state.key`. Existing state
filename constants refer to the database owner's constants. State retains the
`release-blobs` directory and its file copy, hashing, publication and cleanup
behavior. Database/key path checks retain the existing regular-file/directory
and symlink/reparse rules. The existing platform-specific directory-sync behavior
is preserved for both owners' respective files.

Host startup still acquires OwnerGuard, opens ArtifactStore/GlobalLedger, assembles
the Provider and monitor registry, constructs State, then performs Policy and
state/projection reconciliation in the existing order. Failures unwind through
the existing owners and error path. There is no new database thread or control
entry; the connection's lifetime ends when its final shared owner is dropped.

## Scoped connection and integrity services

Database access returns a lifetime-bound `MutexGuard<Connection>` to the internal
typed owner. State retains the guard for exactly the existing operation scope.
Its five Immediate transactions keep their original begin, commit, early-return
and rollback points. Domain validation and SQL request errors remain state-owned.
The SQLite busy timeout applies to SQLite lock contention; it does not establish
a timeout for acquisition of the Rust mutex.

`integrity_tag` reads the immutable key without acquiring the connection mutex.
Row validation and transaction callbacks can compute tags while holding the
connection guard. Callers must not reacquire that same mutex within its scope.
Client-facing contracts remain typed Runtime requests, with no SQL connection
or statement in the wire/API data. The existing client dependency guard also
covers the database owner, and the generic Runtime identity guard covers its root.

Tags retain `actingcommand-keyed-integrity-v1\0`, the same length-prefixed domain,
key and field bytes, SHA-256 and `sha256:` hexadecimal output. Each length remains
a big-endian `u64`; state still selects the domains, fields and ordering. Existing
keys are read unchanged even if a later bootstrap seed differs. A missing key
beside an existing database fails instead of creating a replacement key. First
creation retains the original seed/time/process derivation and write/sync sequence.

## Error and validation contract

Physical errors retain their existing `state_*` code and operation, including
`open_runtime_state`, key failures, schema/version failures, quick-check failure
and the caller operation on a poisoned connection. `RuntimeDatabaseError` carries
those two fields; state maps it to its existing Fatal class. Domain Request/Fatal
classification, SQL operation-specific error mapping and public formatting remain
unchanged. Database errors expose no key or file contents.

The existing runtime-state document, projection, legacy-document migration,
integrity-key/reopen, release-file, pointer rollback, replay and concurrent revision
specifications exercise the delegated owner through the same state assembly.
Host specifications exercise the shared construction at startup. S0's corpus and
the existing structural guards continue in native CI. This extraction adds no
test function or local product execution.

Database backup/restore, general schema upgrades, SQLite ledger cutover and
cross-ledger/state transactions remain assigned to their stages in the
[S1–S5 acceptance map](ledger-store.md#s1s5-ownership-and-acceptance-map).
