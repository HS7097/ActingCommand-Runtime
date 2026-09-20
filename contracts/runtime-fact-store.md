# Runtime fact store

The Runtime keeps a second, separate fact store for facts about itself. It is
distinct from the instance fact store (`crates/actingcommand-contract/src/fact.rs`,
`InstanceFactStore`), which carries observations of the game or application
under automation. A runtime fact never describes game state, and a game
observation never enters the runtime fact store; the two key families are
disjoint by construction.

Workflow #313 owns the design. This document freezes the contract half, the
pure store, and the host wiring that makes the store ledger-backed: the three
ledger events, the append-first rule, startup replay, takeover invalidation,
the periodic snapshot, the read operation, and the producers built so far
(see "Producers"). The remaining producers (`task.`, `host.`, … facts),
policy-input rewiring, and the per-instance read operation land in later
slices.

## Records

`RuntimeFactRecord` (contract module `runtime_fact`):

- `scope` — `runtime` (the whole process) or `instance { instance_id }`.
- `key` — at most 128 bytes, no whitespace or control characters, and inside
  exactly one of eight families: `device.`, `backend.`, `task.`, `host.`,
  `lease.`, `config.`, `provider.`, `eligible.`.
- `value` — the existing `FactValue` variants; a `record_list` value holds at
  most 256 rows of at most 64 fields.
- `observed_at_unix_ms` — when the producing module observed the value.
- `source` — the `OriginModule` that produced it. The host is the only writer.
- `ttl_ms` — optional lifetime, never zero. Expiry is a read predicate:
  `is_expired(now)`; expired records stay in the store until superseded or
  invalidated, and consumers decide what an expired value means.

`RuntimeFactInvalidation` records a drop with a reason (`runtime_takeover`,
`device_closed`, `expired`, `operator`); its `validate` checks the key.
`RuntimeFactSnapshot` is the sealed image of every live record bound to one
ledger position and carries schema version `actingcommand.runtime-fact.v1`.
Its `validate` rejects a foreign schema version
(`runtime_fact_schema_version_mismatch`), a zero ledger position
(`runtime_fact_ledger_position_invalid`), more than 4 096 records
(`runtime_fact_snapshot_too_large`), any invalid record, and a serialized
image larger than `MAX_RUNTIME_FACT_SNAPSHOT_BYTES` — the same 512 KiB bound
one `fact.observed` event may carry — with
`runtime_fact_snapshot_payload_too_large`. A too-large store is rejected,
never truncated.

## Store

`actingcommand_scheduler::facts::RuntimeFactStore` is memory-only and pure: no
clock, no ledger, no I/O. It holds at most 4 096 live records keyed by scope and
key.

- `record` accepts a validated record. An identical record is idempotent
  (`Unchanged`); a strictly newer observation replaces the stored one
  (`Updated`); an observation at the same or an older millisecond is rejected
  (`Stale`), so a late writer can never roll a value back and two producers
  cannot race within one millisecond. Capacity overflow is an error, not an
  eviction.
- `invalidate` drops one record and returns the audit entry;
  `invalidate_instance` drops every record of one instance under the given
  families and returns one audit entry per dropped record, used on owner
  takeover and device close.
- `snapshot(ledger_position, now)` returns the sealed image; `replay(snapshot)`
  replaces the store from one.

## Ledger events

Three events in the existing `Runtime` family carry the store. Family
membership is derived, so they appear in the `events` and `changes` views
without any view definition change. All three are appended by the host with
origin source `runtime`, origin module `runtime-facts`, actor `runtime`,
severity `info`, and derived sensitivity `internal`.

| Event | Action | Payload | Links |
| --- | --- | --- | --- |
| `runtime.fact_recorded` | `fact.publish` | one `RuntimeFactRecord` | system links, plus `instance_id` for an instance scope |
| `runtime.fact_invalidated` | `fact.invalidate` | one `RuntimeFactInvalidation` | system links, plus `instance_id` for an instance scope |
| `runtime.fact_snapshot` | `fact.snapshot` | one `RuntimeFactSnapshot` | system links |

Payload validation delegates to the record, invalidation, and snapshot
validators above; a wrong action is `invalid_runtime_fact_recorded_action`,
`invalid_runtime_fact_invalidated_action`, or
`invalid_runtime_fact_snapshot_action`. The instance link is issued for a
registered instance only; a record whose instance scope is not registered is
refused with `runtime_fact_instance_unknown` before anything is appended.
`EventQuery` selects all three with `origin_module` `runtime-facts`.

## Append-first rule

Every change is ledger-first, under the same `fact_write_gate` as the instance
fact store:

1. **Pre-check without mutating the store.** `record` runs the record's
   validation and the store's own acceptance rules against the live store: an
   identical record returns `Unchanged` and appends nothing; an older or
   equal observation is refused with `runtime_fact_stale`; a new key on a full
   store is refused with `runtime_fact_capacity_exceeded`; an invalid record
   is refused with its validation code. `invalidate` refuses an absent key with
   `runtime_fact_missing`. These are request-class errors; the ledger is
   untouched.
2. **Append.** `runtime.fact_recorded` or `runtime.fact_invalidated` is
   appended through the host's gated append path. An append failure returns
   the ledger error and touches nothing in memory.
3. **Apply.** The store is updated and marked dirty. If the store still refuses
   a change the ledger already holds, memory and ledger disagree: the host
   marks itself fatal with `runtime_fact_store_desync` and returns it.

The store keeps no pending queue: the per-record append is the durability
step. Anything not appended is gone with the process (iron rule 13).

## Startup replay

`recover_runtime_fact_store` runs during `start_with_provider`, after
`runtime.started` / `runtime.takeover` and the `runtime.instance_bound`
events, next to the instance fact store's recovery:

1. The newest `runtime.fact_snapshot` is read (`EventQuery` on that event
   type; the ledger's `query` is unpaged). If one exists the store is
   replaced from it with `replay`.
2. Every `runtime.fact_recorded` / `runtime.fact_invalidated` with a sequence
   greater than that snapshot's — or every one of them when no snapshot exists
   — is applied in ledger order through `record` / `invalidate`, selected with
   `origin_module` `runtime-facts`.
3. Any rejection while replaying our own ledger (stale, invalid, capacity,
   missing, an unexpected payload under that module) is fatal:
   `runtime_fact_replay_failed`, with the offending sequence in the native
   detail. Nothing is skipped.

Replay marks nothing dirty; the ledger already holds everything the store
holds.

## Takeover invalidation

When the owner guard reports a takeover (a new owner epoch over an existing
state root), device-bound facts of the previous epoch are stale until the
post-connect self-check writes them again. After replay, every instance-scoped
record whose key starts with `device.` or `backend.` is dropped, ledger first:
one `runtime.fact_invalidated` with reason `runtime_takeover` per record
(`at_unix_ms` = the host clock now, instance link from the record's scope),
then `invalidate_instance` per distinct instance with the same time. The
dropped keys must equal the appended keys, otherwise
`runtime_fact_store_desync` is fatal. The store is then dirty. Zero matching
records append nothing, so a fresh ledger and the startup event order are
unchanged.

## Periodic snapshot

`append_runtime_fact_snapshot_if_dirty` seals the store only when it is dirty:
it reads the ledger's latest sequence and the clock, builds the snapshot,
validates it (the size and position codes above surface here as fatal host
errors), appends `runtime.fact_snapshot`, and clears the dirty flag. A store
that never changed appends nothing, so there is no ledger noise before
producers exist.

The performance monitor thread calls it with its own elapsed accumulator, so a
seal is attempted at most once every `RUNTIME_FACT_SNAPSHOT_INTERVAL_MS`
(60 000 ms) regardless of the performance sample interval. When that thread
is not spawned (no performance sample interval and frame retention disabled)
or has exited (sampling stopped with retention disabled), periodic snapshots
stop; durability then rests entirely on the per-record events, which is the
rule anyway. The snapshot only shortens replay.

## Read operation

`RuntimeOperation::RuntimeFactSnapshot` (no fields, no source gate beyond a
valid client origin, like `Status`) returns
`RuntimeResult::RuntimeFactSnapshot { snapshot }`: the live store sealed under
the write gate at the ledger's latest sequence, with `taken_at_unix_ms` from
the host clock. `ledger_position` therefore names the last event the reader
can rely on having been applied; every accepted record and invalidation at or
below it is reflected. The read appends nothing and does not mark the store
dirty. `RuntimeHost::runtime_fact_snapshot` and
`RuntimeClient::runtime_fact_snapshot` expose the same read.

`actingctl facts --program --state-root <state-root>` prints the snapshot as
JSON. `facts` without `--program` (the per-instance read) is not built and is a
usage error; the command takes no `--instance`.

## Producers

Two producers write the store today; both go through the append-first rule
above with source `runtime`.

- `device.connected` (slice #316-B) — instance scope, `boolean`: the running
  state emulator instance control observed after `status` / `start` / `stop` /
  `restart`. After `stop` the key is also invalidated with `device_closed`.
- `config.subsystems` and `config.parameters` (Workflow #318, slice 1) —
  runtime scope, `record_list`, no lifetime: the in-memory runtime
  configuration manifest (`RuntimeConfigManifest` in contract module
  `runtime_fact`). `actingd` builds it in `assemble` from what it actually
  applies and hands it to the host with
  `RuntimeHostConfig::with_config_manifest`; `RuntimeHostConfig::validate`
  runs the manifest's own validation (at most 64 subsystems and 256
  parameters; names, keys and reasons non-empty, control-free, at most
  128 / 128 / 512 bytes) and refuses a bad one with
  `invalid_runtime_config_manifest`. The host records the two facts once per
  startup, after the startup events, replay and the instance-fact
  synchronization and before any thread is spawned, with one clock sample
  shared by both records; a refusal there (stale, capacity, invalid) fails
  startup through the normal abort path. On a restart the replayed records
  are superseded by the fresh observation, never refused as stale. Hosts
  built without a manifest (tests, `actinglab` goldens, `ledger-maintenance`)
  record nothing.
  - `config.subsystems` rows: `name` (string), `enabled` (boolean),
    `reason` (string, a short fact such as `configured`, `section absent`,
    `always on; mode shadow`).
  - `config.parameters` rows: `key` (string), `value` (the effective scalar:
    `string`, `integer`, `boolean` or `duration_ms`), `source` (`explicit` for
    a value set in the configuration file, `default` for a library default,
    `discovered` is reserved for values learned at startup and is not produced
    yet). Values are effective values, not raw file contents. The secret
    fingerprint salt is never a value: only `secret_fingerprint_salt_bytes`,
    its byte length, is reported.

`contracts/actingd-check-config.md` lists the subsystems and parameters
`actingd` reports; `actingctl facts --program` returns both records as part of
the snapshot.

## Typed codes

Contract: `runtime_fact_ledger_position_invalid`,
`runtime_fact_snapshot_payload_too_large`, `invalid_runtime_fact_snapshot`,
`invalid_runtime_fact_recorded_action`, `invalid_runtime_fact_invalidated_action`,
`invalid_runtime_fact_snapshot_action`; manifest:
`config_manifest_subsystems_too_many`, `config_manifest_parameters_too_many`,
`invalid_config_subsystem_name`, `invalid_config_subsystem_reason`,
`invalid_config_parameter_key`. Host: `runtime_fact_stale`,
`runtime_fact_capacity_exceeded`, `runtime_fact_missing`,
`runtime_fact_instance_unknown` (request class);
`runtime_fact_store_desync`, `runtime_fact_replay_failed`,
`invalid_runtime_config_manifest` (fatal).
