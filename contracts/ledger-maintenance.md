# Ledger maintenance and cutover

RuntimeHost owns one `Arc<RuntimeDatabase>` for State and GlobalLedger. Startup
acquires OwnerGuard, validates State/ArtifactStore, classifies the formal Ledger
by its keyed metadata row alone without reading history, then opens the one
SQLite writer, which performs the single complete verification. That pass reads
no further than the authenticated head sequence and must finish within the held
maintenance operation deadline. It reads no artifact material (Workflow #375 R5a). Startup then assembles Provider and the
remaining Runtime.
Existing roots without a formal marker require explicit maintenance. An empty
root initializes formal schema and metadata atomically. Candidate, partial,
invalid or unauthenticated metadata never enables a production writer.

Segment construction, append, rotation and repair writes compile only in Ledger's
own physical specifications. Production retains the Legacy read-only parser,
repair journal and quarantine verification used by maintenance and forensic
consumers. The shared ownership and lock metadata remain part of the SQLite
writer; retiring Segment writes does not remove those files or read capabilities.

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

Relative destinations resolve against the process working directory before
validation, creation and sync. Backup paths must be disjoint from the source and
must not already exist.
Import and dry-run explicitly reuse a completed, frozen pre-cutover backup.
Restore creates a new empty stopped root; `--artifact-root` optionally supplies
the original external artifact bytes.

Every exit prints one stdout line, the v2 report
`actingcommand.ledger-maintenance.v2` (Workflow #378 A6a): `{schema_version,
outcome, …}`, where `outcome` is the full `actingcommand.outcome.v1` envelope
(only its registered names with `ACTINGCOMMAND_DETAIL=codes`). A completed pass
is `ledger_maintenance_completed` (success) with the receipt's members: `status`,
its precise operation result (`verified-sqlite`, `verified-segment`,
`backed-up`, `dry-run`, `imported`, `already-imported` or `restored`), `ledger`,
`backup_id`, `warnings` and `activated: false`. A failed pass is
`{schema_version, outcome, activated: false}`: the outcome keeps the failure's
original code with its `operation` when that is a registered location, an OS error of a
maintenance step as a
`foreign_os_error` link, each failed cleanup as a `cleanup` link and each
maintenance warning as a `note` link. v1's `status: "failed"` and
`error {code, operation, cause, cleanup, warnings}` are gone. An argument or
configuration refusal (for example `maintenance_usage_invalid`,
`maintenance_config_missing`, `maintenance_config_invalid`) prints its outcome
alone. A failure also writes its developer line `<LEVEL> actingd: <code> <text>`
on stderr, whose text keeps the English cause (SQL detail included), and the
process exits by the outcome's category: `0` on success, `1` for an error, `2`
for a fatal failure.

## Frozen source and one transaction

`LedgerMaintenance` locks without appending or repairing writer metadata. Backup
reads locked journals through the held handles. Source validation requires the
complete bounded Segment snapshot, verified bytes, no corrupt tail, and every
completed repair's quarantine and matching recovery fact. Its material identity
is compared before and after reads. Mutable writer metadata is frozen in the
backup; the Segment/repair source identity binds the immutable import material.

The importer preserves each original canonical event, sequence, identity,
timestamp, origin, links and artifacts. It reads no artifact material (Workflow #375
R5a): a reference without an eviction proof is imported `Unrecorded`, and the import
transaction's reconstructed facts and common indexes are compared without it.

One Immediate transaction creates the Ledger schema, inserts all original
records and indexes, appends the typed `LedgerRecovered / storage_cutover`
completion, and writes the authenticated ready marker. The database format
header (`user_version = 2`, ledger revision 3) changes in that same transaction; a
database whose ledger tables carry `user_version = 1` is refused with
`ledger_format_unsupported` (fatal) and never imported into. It distinguishes
missing formal tables from an unmigrated database; State rows and their tags
remain unchanged. The completion records source/backup identity, count, first
and last sequence, original head/content digest, State digest and cutover position.
Ordinary append cannot submit a migration completion.

Dry-run verifies the same transaction and rolls it back. The deterministic
migration ID binds the source and original frozen backup. A matching committed
marker verifies and returns without inserting again; conflicts fail closed.
An uncertain COMMIT reports the original error and a readback classification;
it never retries the import. Writer startup consumes the same held lock handle
without an unlock/relock interval. This writer open is the only complete
verification in startup and in `actingd unlock-owner`. Subsequent SQLite appends
preserve the marker and revalidate its original prefix and completion.

## Backup and restore

RuntimeDatabase uses SQLite's online backup API in bounded page steps. `Done`
is required, then explicit connection close, file sync and manifest verification.
The snapshot includes committed WAL content, original key, Segment/repair
material and release blobs. A keyed manifest binds every copied file and the
State/Ledger projection baseline. Artifact references and hashes are bound while
the bytes remain in ArtifactStore's external namespace.

The first Busy/Locked result is retained as a warning `{outcome}` (Workflow #378
A6a): `sqlite_backup_busy` or `sqlite_backup_locked` (category `warning`), the
code saying which one SQLite reported, with `operation` `step_database_backup`.
At most three waits occur within the original deadline. The receipt's `warnings`
and the backup manifest's (`DatabaseBackup.warnings`) retain this warning; a
later backup failure carries it as a `note` link of its outcome. A v1 backup
manifest does not decode. No diagnostic writer is started for maintenance.

Restore first verifies the current source lineage, complete backup, State rows and
Ledger prefix; a backup that records evictions restores too (Workflow #375 R5a). A
pre-cutover restore requires the current head to equal the cutover position and the
State baseline to be unchanged. New facts or unprovable current state refuse
restoration. Each distinct referenced artifact without a recorded `Deleted` or
`RecoveryAbsent` eviction whose file exists in the artifact root is copied as a
verified create-new copy with its exact reference, within the operation limits;
evicted and absent artifacts are skipped. Restore verifies the restored State and
Ledger head before returning, and actingd then prints one stderr line,
`INFO actingd: ledger_maintenance_completed restore artifacts_copied=<n> artifacts_evicted=<n>
artifacts_absent=<n>`; the receipt is unchanged. The source is retained and the
result is not activated. Failed destination material is retained for explicit
disposition.

Defaults are 512 MiB total file material, 16,384 entries, 200,000 events, 120
seconds and 128 SQLite pages per step. Typed limits must be positive and cannot
exceed 4 GiB, 65,536 entries, 1,000,000 events, 600 seconds or 1,024 pages.
Artifact copying uses bounded chunks and the same operation deadline. Existing
State validation and database mutex acquisition retain their owner semantics.
The defaults bound one maintenance operation. Startup, `actingd unlock-owner` and
`verify` of a formal root use only their deadline; their material bound is the
authenticated Ledger extent (Workflow #375). `verify` classifies a formal root by
its keyed meta row, as startup does, and verifies it in one complete read that
reads no further than the authenticated head; its receipt's `ledger` comes from
that read. A Missing (Segment) or candidate root keeps the full status read under
the limits. `backup` (with its binding read), dry-run, import and restore still
return `ledger_read_budget_exceeded` for a root beyond the default limits; such a
root needs `RuntimeHost::maintain_ledger` with explicit limits (at most 1,000,000
events, 4 GiB, 600 seconds). The CLI takes no limit options.

Workflow #375 R5a: no opening of a formal root reads artifact material. Startup,
`unlock-owner` and every maintenance pass (`verify`, `status`, `backup` with its
binding read, `dry-run`, `import`, `restore`'s checks) never open, hash or stat a
referenced file. A reference with an authenticated eviction proof takes the proof's
state (`Evicted`, `PendingEviction`, or `FailedEviction` for a `Failed` outcome, which
no longer stops an opening); any other reference is `Unrecorded`, so a missing or
damaged artifact never stops startup or `verify`. Row authentication, the hash chain,
the keyed head and the retention replay are unchanged. A formal `verify` refuses only
the retention state the ledger itself leaves unsettled: an eviction intent without its
outcome, which only the next actingd start completes. It fails with
`maintenance_artifact_material_unavailable`, whose value `count` is the number of
distinct pending artifacts (the developer text says `pending_evictions=<n>`;
start actingd once so that it finishes the eviction, then run
`verify` again); `Unrecorded`, `Evicted` and
`FailedEviction` pass. A Segment root still refuses any eviction. After the writer
open, the started daemon writes the daemon lines of `ledger_opened` (info) on
stderr, never as a Ledger event (Workflow #378 A6a; it replaced the stdout line
`actingd ledger_open …`), with the values `event_count`, `artifact_count`,
`artifact_bytes`, `worker_count`, `sql_read_ms`, `verify_ms`, `material_ms`,
`restore_ms` and `timeout_ms` (the deadline). A failed open carries the same
values as the `note` link `ledger_open_measured` of its failure. Material is never
read, so `artifact_count`, `artifact_bytes`, `worker_count` and `material_ms` are
always 0. A failed formal `verify` carries the same `ledger_open_measured` note
link, and its developer text names the read's phases as `ledger_open events=<n> …
restore_ms=<ms> deadline_ms=<ms>`.

Later startup steps keep
their own bounds: instance-fact and agent-dispatch recovery each query the full
history from the writer within its 10 second command timeout, and a restart
fails with `policy_scheduling_outcome_capacity_exceeded` when more than 16,384
distinct (catalog task, instance) pairs have a completed policy run that
references outcome keys; only each pair's latest completed run is re-verified.

## Portable evidence

`GlobalLedger::open_evidence(GlobalLedgerEvidenceConfig::new(state_root), verifier)`
accepts an explicit state root and selects from formal database metadata. One
verified immutable snapshot supplies events, query/page, latest position and
signature input. Actual writer metadata is read from the canonical lock;
Absent/Locked/unknown observations remain distinct from a closed owner.

Forensics open/export, event/task/performance/stability reports and signature
replay use this reader. SQLite reports its backend and completeness while Segment
byte snapshots, repair counts and tail observations are explicitly not applicable.

On an SQLite root, forensics open/events/chain/tail/repairs/performance open the
ledger with the same record authentication but read no artifact material; open
reports `artifact_material_complete` as `null`. Stability, task-evidence and export
verify each artifact separately. An artifact whose file does not exist (Workflow #375
R5b: a frame deleted by hand) is listed once in `material_missing` (export: a
`material_missing:` line) and is neither a failure nor a gap; the reports and the
export complete, and no later projection reads it. Other material failures enter
`failures` and a gap without de-duplication: one damaged artifact appears once per
referencing event at opening, without a source sequence, and again with its source
sequence when a report projects it. Export fails with an error naming each failed
artifact and withholds all content. One damaged artifact no longer fails the whole
snapshot. Artifact identity conflicts remain fatal; a failed eviction is reported as
`eviction_failed`. Segment roots keep their existing behavior.

Segment reports retain their original physical meaning. Saved-artifact OCR holds
the source writer lock, rejects the target root, requires a closed source whose
keyed head row and declared prefix authenticate (a segment source is read whole)
and a valid through-sequence, and preserves exact artifact/capture causality
(`contracts/saved-artifact-ocr.md`).
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
