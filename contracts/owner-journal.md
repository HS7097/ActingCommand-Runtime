# Owner journal

`<state_root>/owner.lock` is the Runtime owner journal and the owner lock: one
file, held with one exclusive OS lock by the running owner (OwnerGuard) or by
`actingd unlock-owner`. It is JSON Lines. Its native reader is the only source of
the prior-epoch close evidence the ledger imports and re-checks at every start
(`contracts/ledger-store.md`, "Prior-epoch scope close (proven or unproven)").
Workflow #191 O bounds its growth by folding records into one checkpoint line.

## Records

Every line is one owner record: `schema_version`, `revision`, `owner_epoch`,
`pid`, `started_at_unix_ms`, `active`, `active_instances` (sorted, unique),
`closed_at_unix_ms` (present exactly when inactive) and, in v2 only,
`resource_disposition`. `actingcommand.runtime-owner.v1` records carry no
disposition; `actingcommand.runtime-owner.v2` records must, and an inactive v2
record's disposition is `none`. Revisions are consecutive: from 1, or from the
checkpoint's base (below). Revisions never restart. A complete line that fails
these rules is fatal `owner_record_invalid`; only an incomplete final line left by
a crash mid-append is truncated.

Writers, all through the handle that holds the lock:

| Writer | Record |
|---|---|
| startup acquire | a new epoch's first record: active, the taken-over instances, `none` |
| active-instance change | same epoch, new instance set; an unchanged value writes nothing |
| resource disposition | same epoch, `in_use` or `confirmed_closed`; unchanged writes nothing |
| retention | same epoch, `unconfirmed`; the lock stays held until process exit |
| close | same epoch, inactive, `closed_at_unix_ms`, `none`; then unlock |
| `actingd unlock-owner` | the last epoch again, `confirmed_closed` (`contracts/actingd-unlock-owner.md`) |

## Derived state

A block is a maximal run of records of one epoch. The reader keeps, per epoch,
its last block: first and final revision, pid and `started_at_unix_ms` of the
first record, last activity and disposition, whether some record is v1
(`legacy`) or `confirmed_closed`, and `consistent` (the epoch appears in no
earlier block, every record is v2 with the first record's pid and start, and no
inactive record precedes an active one). A consistent block with a
`confirmed_closed` record has a positive proof: the suffix from its last such
record, accepted only when it validates. Each complete read also exposes its
bounds (length, SHA-256 and last revision) and its prefixes: the length and
SHA-256 before every record line and of the whole file.

## Checkpoint

The first line may be a checkpoint, schema
`actingcommand.runtime-owner-checkpoint.v1`. It never appears elsewhere and never
twice. Fields, in order: `predecessor_bytes` and `predecessor_sha256` (the replaced
file, audit only), `last_record` (the last folded record, verbatim; its revision
is the base of the next physical record), `epochs` (per folded epoch, strictly
ascending: `owner_epoch`, `pid`, `started_at_unix_ms`, `first_revision`,
`final_revision`, `legacy`, `consistent`, `confirmed_closed`, `last_active`, and
optional `last_disposition` and `proof` with `confirmed_revision` and `suffix`) and
`prefixes` (carried bounds, strictly ascending by `bytes` then `sha256`).

Every read validates it, or fails with `owner_checkpoint_invalid`: known fields
only, lower-case 64-digit SHA-256 values, bounds within 4 MiB, a `last_record`
that passes the record rules with revision at least 1, every epoch with nonzero
pid and start, `1 <= first <= final <= last_record.revision` and a disposition
unless legacy, and a summary for the last record's epoch that matches its final
revision, activity, disposition and, when consistent, pid and start. A carried
proof requires a consistent, non-legacy, confirmed-closed epoch whose suffix ends
in its last activity and disposition, and it must validate with this read's
bounds; an invalid carried proof is an error, never silently dropped.

The checkpoint seeds the reader's blocks and proofs. A first physical block of the
last record's epoch continues that epoch's summary: M1 it stays consistent only if
the summary is, its records are v2 with the summary's pid and start, and no
inactive record precedes an active one across the boundary; M2 `legacy` and
`confirmed_closed` combine, first revision, pid and start come from the summary,
the rest from its final record; M3 its proof starts at its own last
`confirmed_closed` record, else extends the carried suffix with its records, and
is accepted only when it validates; M4 a proof that is not continued takes this
read's bounds. Every other block is derived as without a checkpoint, and an
epoch already summarised counts as reused. Supported prefixes are the physical
prefixes plus the carried ones.

## Compaction

Only the running OwnerGuard folds, only immediately before an in-epoch
active-instance or disposition append, and only when that append would take the
bytes after the checkpoint line past 64 KiB, the journal holds at least two
physical records, the guard is not retained and has no cached journal failure.
Acquire, retention, close and `unlock-owner` never fold. Every record but the last
is folded, so the running epoch's latest record and every close record stay
physical: readers of physical lines alone, such as `actingctl request-shutdown
--wait`, need no checkpoint support (the checkpoint has no top-level `owner_epoch`
or `pid`). Acquire does not fold because an epoch that only starts and stops adds
about as many bytes as its summary, and because startup reconciliation must read
the unfolded journal before anything rewrites it; every fold happens after that
startup's reconciliation.

The image is a checkpoint derived from the previous checkpoint and every physical
record but the last, followed by the last record's bytes unchanged. Its prefixes
are the previous carried prefixes and the start of every physical block except a
continued one. Before anything is written, the image must derive the same last
record, revision, blocks and proofs (read bounds aside) as the file, carry those
prefixes and fit in 4 MiB; otherwise `owner_compaction_mismatch` or
`owner_journal_too_large`.

Block starts are carried because every startup re-checks every sealed import
against the journal, and every sealed complete-read bound is a block start: the
reconciliation reads the journal before the acquire appends the new epoch's first
record at its end. Other record boundaries are not carried.

## Side file

A fold writes `owner.lock.compact`: the image followed by one seal line
(`actingcommand.runtime-owner-compaction.v1`: `image_bytes`, `image_sha256`,
`image_through_revision`, `predecessor_bytes`, `predecessor_sha256`). S1 builds and
checks the image; S2 creates or truncates, writes and synchronizes the side file;
S3 writes the image at offset 0 of the locked journal, sets its length and
synchronizes it; S4 removes the side file. Directories are synchronized where the
platform allows. The new record is appended only after all four steps succeed.

Acquire and `unlock-owner` recover before reading when the side file exists, then
remove it. With F the journal, S the side file valid when it ends in a newline,
its last line is the seal, `image_bytes` plus the seal line is its length and the
image hash matches:

- R1 S invalid: stale; F is untouched and goes to the ordinary reader.
- R2 F starts with the predecessor: stale; later appends are kept.
- R3 F starts with the image: complete when nothing follows, or the next line is
  a complete record with revision `image_through_revision + 1`; when F has exactly
  the predecessor's length, the residue is cut at the image length; when what
  follows has no newline, the torn append goes to the ordinary tail recovery;
  anything else is `owner_compaction_unrecoverable`.
- R4 F has the image's length, or a length from the predecessor's to the larger of
  the two: an interrupted S3; the image is written again.
- Anything else, including an empty F: `owner_compaction_unrecoverable`.

No state is decided by guessing: F is never cut except in R3 and never rewritten
outside the R4 window.

## Limits and compatibility

The 4 MiB ceiling is unchanged. A journal is rewritten only when an in-epoch
append would take the bytes after its checkpoint past 64 KiB; until then its bytes
stay as written, and a journal without a checkpoint reads exactly as before.
Folding bounds the running epoch; what remains grows by about 0.59 KB per epoch
(its summary and one carried prefix), about 7,000 epochs before the ceiling, which
fails explicitly as `owner_journal_too_large`.

A build older than Workflow #191 O reads a checkpoint as `owner_record_invalid`
and does not start. To go back to such a build, restore the owner journal backed
up before its first fold and delete `owner.lock.compact` if present.

## Error codes

| Code | Meaning |
|---|---|
| `owner_checkpoint_invalid` | the checkpoint is misplaced, repeated or fails its validation |
| `owner_compaction_incomplete_tail` | a fold found an incomplete final line |
| `owner_compaction_mismatch` | the image does not derive the journal's state |
| `owner_compaction_side_read_failed` | the side file exists but cannot be read |
| `owner_compaction_stage_failed` | S2 failed; the journal is untouched |
| `owner_compaction_rewrite_failed` | S3 failed; every later write of that guard returns it |
| `owner_compaction_cleanup_failed` | the side file could not be removed; the journal is consistent |
| `owner_compaction_unrecoverable` | the side file and journal match no R1-R4 state |
| `owner_compaction_repair_failed` | an R3 or R4 repair failed |

All are fatal: a failed fold fails the append that triggered it, and startup or
`unlock-owner` fails with the code.
