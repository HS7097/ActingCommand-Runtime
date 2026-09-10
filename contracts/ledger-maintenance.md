# Ledger maintenance and cutover

RuntimeHost owns one `Arc<RuntimeDatabase>` for State and GlobalLedger. Startup
acquires OwnerGuard, validates State/ArtifactStore and the formal Ledger metadata,
opens one SQLite writer, then assembles Provider and the remaining Runtime.
Existing roots without a formal marker require explicit maintenance. An empty
root initializes formal schema and metadata atomically. Candidate, partial,
invalid or unauthenticated metadata never enables a production writer.

## Offline entry

`actingd ledger-maintenance` reads the normal configuration only for state root
and fingerprint salt. It does not assemble Provider, policy, IPC or devices.
All operations acquire the existing OwnerGuard and Ledger OS locks; unresolved
resource ownership refuses maintenance. The canonical Ledger lock remains
`ledger/writer.lock`; a present candidate `writer.lock` is also held and must
have a provably closed owner.

```text
actingd ledger-maintenance backup --config runtime.json --backup frozen-backup
actingd ledger-maintenance dry-run --config runtime.json --backup frozen-backup
actingd ledger-maintenance import --config runtime.json --backup frozen-backup
actingd ledger-maintenance verify --config runtime.json
actingd ledger-maintenance restore --config runtime.json --backup frozen-backup --target restored-root
```

Backup paths must be disjoint from the source and must not already exist.
Import and dry-run explicitly reuse a completed, frozen pre-cutover backup.
Restore creates a new empty stopped root; `--artifact-root` optionally supplies
the original external artifact bytes. A successful receipt reports its precise
operation result and `activated: false`. Errors preserve their original identity,
physical I/O/SQL detail and secondary cleanup failures, followed by the normal
process FATAL/nonzero exit when no Ledger writer is available.

## Frozen source and one transaction

`LedgerMaintenance` locks without appending or repairing writer metadata. Backup
reads locked journals through the held handles. Source validation requires the
complete bounded Segment snapshot, verified bytes, no corrupt tail, and every
completed repair's quarantine and matching recovery fact. Its material identity
is compared before and after reads. Mutable writer metadata is frozen in the
backup; the Segment/repair source identity binds the immutable import material.

The importer preserves each original canonical event, sequence, identity,
timestamp, origin, links and artifacts. ArtifactStore verifies bytes outside the
database mutex. Only those frozen proofs are used while comparing the import
transaction's reconstructed facts and common indexes.

One Immediate transaction creates the Ledger schema, inserts all original
records and indexes, appends the typed `LedgerRecovered / storage_cutover`
completion, and writes the authenticated ready marker. The database format
header (`user_version = 1`) changes in that same transaction. It distinguishes
missing formal tables from an unmigrated database; State rows and their tags
remain unchanged. The completion records source/backup identity, count, first
and last sequence, original head/content digest, State digest and cutover position.
Ordinary append cannot submit a migration completion.

Dry-run verifies the same transaction and rolls it back. The deterministic
migration ID binds the source and original frozen backup. A matching committed
marker verifies and returns without inserting again; conflicts fail closed.
An uncertain COMMIT reports the original error and a readback classification;
it never retries the import. Writer startup consumes the same held lock handle
without an unlock/relock interval. Subsequent SQLite appends preserve the marker
and revalidate its original prefix and completion.

## Backup and restore

RuntimeDatabase uses SQLite's online backup API in bounded page steps. `Done`
is required, then explicit connection close, file sync and manifest verification.
The snapshot includes committed WAL content, original key, Segment/repair
material and release blobs. A keyed manifest binds every copied file and the
State/Ledger projection baseline. Artifact references and hashes are bound while
the bytes remain in ArtifactStore's external namespace.

The first Busy/Locked result is retained as a WARNING with its native SQLite
code and bounded recovery choice. At most three waits occur within the original
deadline. The formal maintenance receipt and backup metadata retain this warning;
a later backup failure also carries it in the formal error receipt. No diagnostic
writer is started for maintenance.

Restore first verifies the current source lineage, complete backup, State rows,
Ledger prefix and external artifact bytes. A pre-cutover restore requires the
current head to equal the cutover position and the State baseline to be unchanged.
New facts or unprovable current state refuse restoration. Copies use create-new
files, retain exact artifact references, and verify the restored State, Ledger
head and artifacts before returning. The source is retained and the result is
not activated. Failed destination material is retained for explicit disposition.

Defaults are 512 MiB total file material, 16,384 entries, 200,000 events, 120
seconds and 128 SQLite pages per step. Typed limits must be positive and cannot
exceed 4 GiB, 65,536 entries, 1,000,000 events, 600 seconds or 1,024 pages.
Artifact copying uses bounded chunks and the same operation deadline. Existing
State validation and database mutex acquisition retain their owner semantics.

## Portable evidence

`GlobalLedger::open_evidence(GlobalLedgerEvidenceConfig::new(state_root), verifier)`
accepts an explicit state root and selects from formal database metadata. One
verified immutable snapshot supplies events, query/page, latest position and
signature input. Actual writer metadata is read from the canonical lock;
Absent/Locked/unknown observations remain distinct from a closed owner.

Forensics open/export, event/task/performance/stability reports and signature
replay use this reader. SQLite reports its backend and completeness while Segment
byte snapshots, repair counts and tail observations are explicitly not applicable.
Segment reports retain their original physical meaning. Saved-artifact OCR holds
the source writer lock, rejects the target root, requires a complete closed
source and valid through-sequence, and preserves exact artifact/capture causality.
Package loading and OCR evaluation keep their existing entry semantics.

## Stage boundary

The existing storage/owner/artifact specifications cover source preservation,
dry-run rollback, idempotent import, marker corruption, normal startup and empty
root restoration using fixtures under CI. The existing SQLite process boundary
continues to prove committed append replay. Actual migration, process interruption
of import, old production writer inactivity and operational backup/restore remain
separate migration evidence; no real state/key/device execution is implied.
Database-internal Ledger/State transactions belong to S4 and full legacy code
retirement belongs to S5.
