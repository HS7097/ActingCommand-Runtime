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
the periodic snapshot, the read operation, the offline read, and the producers built so far
(see "Producers"). The `task.` family has three producers (`task.game`,
`task.server`, `task.page`); the remaining producers (`host.`, … facts),
policy-input rewiring, and the per-instance read operation land in later
slices.

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
record whose key starts with `device.`, `backend.` or `application.` is
dropped, and so is the single key `task.page` (the other `task.` keys survive a
takeover), ledger first:
one `runtime.fact_invalidated` with reason `runtime_takeover` per record
(`at_unix_ms` = the host clock now, instance link from the record's scope),
then `invalidate_instance` for the families and `invalidate` for `task.page`
per distinct instance with the same time. The
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

`actingctl status --config --state-root <state-root>` runs the same
`runtime_fact_snapshot` read (no new operation) and prints only the two
`config.subsystems` and `config.parameters` records, as a JSON array of
`RuntimeFactRecord` objects in that order and in the same shape the snapshot
carries them. A snapshot without both records is an error
(`config_facts_missing`, exit code 1), never an empty array. Plain `status`
is unchanged; `--config` on any other command is a usage error.

## Offline read

`actingcommand_ledger_forensics::runtime_facts_at(state_root, position,
deadline)` is the single offline implementation of the startup replay above,
bound to a ledger position (inclusive) instead of the latest one. The UI and
every other offline consumer call it; none folds `runtime.fact_*` events
itself.

1. It opens one read-only metadata snapshot of the state root
   (`GlobalLedger::open_metadata`, the source `actingledger views` uses) and
   reads through the shared view page at snapshot `position`.
2. The latest `runtime.fact_snapshot` with a sequence at or below `position`
   replaces the empty store after its own validation; without one the store
   stays empty.
3. Every `runtime.fact_recorded` / `runtime.fact_invalidated` after that
   snapshot, through `position`, is applied in ledger order under the store's
   rules. An identical record changes nothing; an older or equal observation,
   a new key on a full store, an invalid record, an absent key on
   invalidation, or any other payload under `runtime-facts` fails with
   `runtime_fact_replay_failed` naming the sequence. Nothing is skipped and no
   partial store is returned.

Takeover and device-close drops need no rule of their own: every dropped key is
its own `runtime.fact_invalidated` event. A position between `runtime.takeover`
and that takeover's `runtime_takeover` invalidations shows the previous epoch's
records; the host serves no online read there.

The result `ForensicRuntimeFactsResult` is one of:

- `available { source, position, facts }` — `facts` is the `RuntimeFactSnapshot`
  the online read returns, records in scope-then-key order, with
  `ledger_position` = `position`. `taken_at_unix_ms` is the ledger timestamp of
  the event at `position`; the online read samples the host clock instead,
  which the ledger does not record. Expired records are included, as online;
  expiry stays the consumer's `is_expired(taken_at_unix_ms)`. `source` is the
  final page's `LedgerReadScope` (`offline`, complete, scanned through
  `position`).
- `not_available { position, latest_sequence, reason }` — `ledger_empty` (the
  snapshot holds no event) or `position_beyond_snapshot`.
- `failed { position, code, operation, detail, io_kind? }` — position 0
  (`runtime_fact_ledger_position_invalid`), an empty state root
  (`invalid_state_root`), the ledger's own open and read codes (`ledger_io` for
  an absent ledger, `ledger_read_budget_exceeded`), a source that is not
  completely readable (`runtime_facts_source_incomplete`), no event at the
  position (`runtime_facts_position_missing`), the cooperative deadline
  (`runtime_facts_read_budget_exceeded`), or `runtime_fact_replay_failed`.

`actingledger --state-root <state-root> facts --at <sequence>` prints the
result as one JSON line under `command: facts`, with a 30 s deadline. It exits
0 only for `available`; otherwise the result is printed first and the tool
exits nonzero with `runtime_facts_not_available` or the failure code.

## Producers

Five producers write the store today; all go through the append-first rule
above with source `runtime`.

- `device.connected` (slice #316-B) — instance scope, `boolean`: the running
  state emulator instance control observed after `status` / `start` / `stop` /
  `restart`. After `stop` the key is also invalidated with `device_closed`.
  Slice #316-B3 adds a second invalidation path: an ADB failure inside the
  foreground gate (below) invalidates the key with `adb_unreachable`, even while
  a Nemu session still delivers frames, because ADB is the only health anchor.
  Slice #316-P4 adds the second `adb_unreachable` invalidation point: a physical
  instance's first contained-task capture that fails while one ADB baseline
  probe fails too invalidates this key and `application.foreground` the same way.
- `application.foreground` (slice #316-B3) — instance scope, `string`: the
  package name Android reported as the resumed activity, read through ADB
  (`dumpsys activity activities`) by the foreground gate before every pointer
  input (`contracts/application-lifecycle.md`). The gate records the fact only
  when the observed value differs from the stored one, so a run that stays in
  one application appends one record; two observations inside one millisecond
  keep the first. Invalidated with `device_closed` after emulator `stop`, with
  `adb_unreachable` on an ADB failure, and with `runtime_takeover` like every
  device-bound family. Never written for a fixture instance.
- `task.game` and `task.server` (Workflow #191, slice g) — instance scope,
  `string`, no lifetime: the admitted contained-task package's `control.json`
  `game` and `server` declarations, copied verbatim (the Runtime names no game
  or server of its own). The host records both for the leased instance right
  after `task.package_admitted` is appended, each only when its value differs
  from the stored one, so repeated runs of packages with the same declarations
  append nothing; two observations inside one millisecond keep the first. An
  entry-recovery package's admission records nothing. Invalidated with
  `device_closed` after emulator `stop`; an ADB failure and a takeover leave
  them in place (they name the last admitted package, not device state).
  Written for fixture and physical instances alike.
- `task.page` (Workflow #191, slice g) — instance scope, `string`, no lifetime:
  the page label the last contained-task recognition matched (`matched_page`
  of `task.recognition_completed`), copied verbatim. Recorded right after that
  event is appended, only when a page matched and the label differs from the
  stored one; an unmatched recognition leaves the stored value in place.
  Invalidated with `device_closed` after emulator `stop`, with
  `adb_unreachable` wherever `device.connected` is (foreground gate and
  contained-task capture), and with `runtime_takeover`. Written for fixture and
  physical instances alike.
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
    yet). Values are effective values, not raw file contents: since Workflow
    #318 cfg2 every one is read back from the assembled `RuntimeHostConfig`
    (scheduler, policy cadence, performance control, performance monitor
    including `performance_monitor.pressure_start_samples` /
    `performance_monitor.pressure_end_samples`, I/O timeout, frame bound,
    capacity thresholds, frame retention), and a host that cannot report one
    fails assembly with `config_manifest_incomplete` instead of writing a
    default. Configured daemon-level device tool paths appear as
    `device_paths.<name>` (`explicit`); unconfigured ones are omitted. The
    secret fingerprint salt is never a value: only
    `secret_fingerprint_salt_bytes`, its byte length, is reported. Since
    Workflow #318 cfg3 every configured instance, in declaration order, adds
    `instance.<instance_id>.stuck_recovery` (`boolean`) and
    `instance.<instance_id>.stuck_recovery_cooldown_secs` (`integer`), read
    back from the host's per-instance stuck-recovery settings and `explicit`
    when the instance named the field; they are keyed by the registry's bounded
    `instance_id` (`instance_<32 hex>`), never by the alias, which may exceed
    the 128-byte key bound. `allow_env_overrides` (`boolean`, default `false`)
    reports whether the `ACTINGCOMMAND_*` environment fallbacks are read; the
    `env_overrides` subsystem's reason names every set but ignored variable as
    `env_override_ignored:<VAR>` (`contracts/actingd-check-config.md`,
    "Environment overrides").

`contracts/actingd-check-config.md` lists the subsystems and parameters
`actingd` reports; `actingctl facts --program` returns both records as part of
the snapshot and `actingctl status --config` returns just the two (see "Read
operation").

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
`invalid_runtime_config_manifest` (fatal). Offline read:
`runtime_facts_source_incomplete`, `runtime_facts_position_missing`,
`runtime_facts_read_budget_exceeded`; `actingledger facts`:
`runtime_facts_not_available`.
