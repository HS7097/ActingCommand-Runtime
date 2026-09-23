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
(see "Producers"), and the program-instance inputs consumed by policy. The
remaining producers (`task.`, `host.`, … facts) land in later slices.

## Records

`RuntimeFactRecord` (contract module `runtime_fact`):

- `scope` — `runtime` (the whole process) or `instance { instance_id }`.
- `key` — at most 128 bytes, no whitespace or control characters, and inside
  exactly one of nine families: `device.`, `backend.`, `task.`, `host.`,
  `lease.`, `config.`, `provider.`, `eligible.`, `application.`.
- `value` — the existing `FactValue` variants; a `record_list` value holds at
  most 256 rows of at most 64 fields.
- `observed_at_unix_ms` — when the producing module observed the value.
- `source` — the `OriginModule` that produced it. The host is the only writer.
- `ttl_ms` — optional lifetime, never zero. Expiry is a read predicate:
  `is_expired(now)`; expired records stay in the store until superseded or
  invalidated, and consumers decide what an expired value means.

`RuntimeFactInvalidation` records a drop with a reason (`runtime_takeover`,
`device_closed`, `adb_unreachable`, `expired`, `operator`); its `validate` checks
the key.
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
3. **Apply.** Both fact projections consume the committed tail, including this
   event, and the program store is marked dirty when its records change. If it refuses
   a change the ledger already holds, memory and ledger disagree: the host
   marks itself fatal with `runtime_fact_store_desync` and returns it.

The Host's program-store wrapper tracks the last sequence actually applied;
the instance store retains its own applied cursor. Both inspect every event
through the selected position, apply their own event families, and reject a
gap or incomplete prefix. A snapshot label never advances a cursor. Fatal
append/application failures close projection eligibility before the caller
releases `fact_write_gate`, including the interval before fatal propagation.

The store keeps no pending queue: the per-record append is the durability
step. Anything not appended is gone with the process (iron rule 13).

## Startup replay

`recover_runtime_fact_store` runs during `start_with_provider`, after
`runtime.started` / `runtime.takeover` and the `runtime.instance_bound`
events, next to the instance fact store's recovery:

1. The newest `runtime.fact_snapshot` whose event is at or before the requested
   cut is read. Its coverage must precede its own event, and the interval
   between coverage and that event must be complete and contain no omitted
   recorded/invalidated change. If one exists the store is replaced from it
   with `replay`, and the cursor advances to that actual snapshot event.
2. Every `runtime.fact_recorded` / `runtime.fact_invalidated` with a sequence
   greater than that snapshot's — or every one of them when no snapshot exists
   — is applied through the requested cut in ledger order through
   `record` / `invalidate`. All intervening events advance actual prefix
   progress. Subsequent snapshots must match the already applied state.
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
record whose key starts with `device.`, `backend.` or `application.` is
dropped, ledger first:
one `runtime.fact_invalidated` with reason `runtime_takeover` per record
(`at_unix_ms` = the host clock now, instance link from the record's scope).
Each committed invalidation is applied through that exact sequence; an absent
or refused target is `runtime_fact_store_desync`, never a skipped drop. The
store is then dirty. Zero matching
records append nothing, so a fresh ledger and the startup event order are
unchanged.

## Periodic snapshot

`append_runtime_fact_snapshot_if_dirty` seals the store only when it is dirty:
it synchronizes both stores to the selected position and reads the clock, builds the snapshot,
validates it (the size and position codes above surface here as fatal host
errors), appends `runtime.fact_snapshot`, clears the dirty flag, and consumes
the new event through the same synchronization path. A store
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
the write gate at both stores' actual synchronized position, with `taken_at_unix_ms` from
the host clock. `ledger_position` therefore names the last event the reader
can rely on having been applied; every accepted record and invalidation at or
below it is reflected. The common synchronization first settles any pending
instance-fact invalidations through their existing ledger transaction and
acknowledgment path; it appends no program snapshot. Reading alone does not
mark program records dirty. `RuntimeHost::runtime_fact_snapshot` and
`RuntimeClient::runtime_fact_snapshot` expose the same read.

`actingctl facts --program --state-root <state-root>` prints the snapshot as
JSON. The existing Host instance-fact snapshot also uses the same real cut.

## Program-instance policy inputs

Startup seeds each configured, registered instance through the original
pre-check → ledger append → memory-application path, after configuration
manifest recording and before worker admission. The fixed keys use the
registered UUID's instance scope, source `runtime`, and no TTL:

| Key | Existing value type | Meaning |
| --- | --- | --- |
| `config.policy_instance.seeded` | `boolean` (`true`) | Durable first-seed occurrence; immutable and non-invalidatable |
| `config.policy_instance.identity` | `string` | Serialized `InstanceSnapshot` containing the declared instance/server/game/host identity, `available=false`, and empty capability/preference lists |
| `config.policy_instance` | `string` | Serialized original configured `InstanceSnapshot`, including availability, capabilities and preferences |

The JSON string encoding reuses `InstanceSnapshot`'s required fields and
unknown-field rejection and the existing fact/event/snapshot bounds. It
preserves exact string case and list order and does not deduplicate values.
The codec rejects a string larger than the existing 512 KiB snapshot byte
bound before append or decode; whole-store snapshot and record-count bounds
still apply independently.
These are declared planning metadata; server/game labels are scope identity,
not screen observations. The seed makes no claim about backend self-check,
device readiness or execution permission.

The marker is committed first. Existing records are retained, and a recovered
marker prevents configuration from refilling missing or invalidated values,
including after an interrupted startup. No policy configuration creates no
seed or default instance. An unregistered alias has no legal seed scope and
remains subject to the original `policy_instance_metadata_untrusted` consumer
check. Existing host-resource authority checks remain in place.

Formal current evaluation, re-projection and historical policy-input identity
reads construct `instances` from committed program facts. Configuration only
supplies the expected alias list; it never supplies a missing runtime field.
Missing/expired/invalid planning values, or an identity mismatch, produce an
unavailable instance with empty capability/preference lists and an explicit
key/cause in the existing `instance_unavailable` reason chain. This uses the
committed identity record. If that identity or the seed occurrence itself is
unavailable, the read fails explicitly with `policy_instance_fact_unavailable`
and its instance/key/cause. It cannot fabricate a successful empty collection.
`InstanceSnapshot.unavailable_reason` is optional and omitted when absent, so
ordinary initial instance serialization is unchanged. The original overlay
ordering and policy validation continue to apply.

Under the original outcome → fact lock order, current projection first consumes
the instance tail and settles generated invalidations. Their acknowledgment
does not advance the instance cursor: the appended events are actually
consumed in another tail pass. Work is bounded by the existing 256 active-fact
limit. The program projection then consumes through that same real position
P. Both stores are copied while the fact gate is held. This path is incremental,
not a replay from the first event on each evaluation. Program snapshots and
the affected instance snapshots use this same position discipline.

An explicit historical P uses a temporary program projection from the latest
valid snapshot event at or before P plus its tail, and the existing instance
`at_position(P)` projection. Prefix gaps or unavailable cuts are errors. TTL
is evaluated at the timestamp of the actual cut event, not today's clock.
Historical reconstruction neither runs takeover invalidation nor changes
either live store. Forward what-if projection retains its caller-supplied
scenario and read-only behavior: temporary copies consume to one cut, while
real fact-scope/pool bindings use program identity at that cut.

Instance-fact publication and pool-source checks bind server/game scopes from
program metadata. The policy identity combines the existing semantic hash
with just the committed identity/planning records actually consumed. Sampling
timestamps and unrelated program records do not change it. A relevant value,
source revision, catalog, outcome or instance fact still invalidates the
original identity. The original stale, approval, capacity, lease and fencing
checks remain the final admission controls.

## Producers

The following producers write the store; all go through the append-first rule
above with source `runtime`.

- `config.policy_instance.*` and `config.policy_instance` — the one-time
  configuration seed and declared identity described above.

- `device.connected` (slice #316-B) — instance scope, `boolean`: the running
  state emulator instance control observed after `status` / `start` / `stop` /
  `restart`. After `stop` the key is also invalidated with `device_closed`.
  Slice #316-B3 adds a second invalidation path: an ADB failure inside the
  foreground gate (below) invalidates the key with `adb_unreachable`, even while
  a Nemu session still delivers frames, because ADB is the only health anchor.
- `application.foreground` (slice #316-B3) — instance scope, `string`: the
  package name Android reported as the resumed activity, read through ADB
  (`dumpsys activity activities`) by the foreground gate before every pointer
  input (`contracts/application-lifecycle.md`). The gate records the fact only
  when the observed value differs from the stored one, so a run that stays in
  one application appends one record; two observations inside one millisecond
  keep the first. Invalidated with `device_closed` after emulator `stop`, with
  `adb_unreachable` on an ADB failure, and with `runtime_takeover` like every
  device-bound family. Never written for a fixture instance.
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
`runtime_fact_instance_unknown`, `policy_instance_fact_unavailable`,
`policy_instance_seed_immutable`, `policy_instance_fact_payload_too_large` (request class);
`runtime_fact_store_desync`, `runtime_fact_replay_failed`,
`invalid_runtime_config_manifest` (fatal).
