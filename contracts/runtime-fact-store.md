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
(see "Producers"). The `task.` family has three single-key producers (`task.game`,
`task.server`, `task.page`) and the four per-task settlement facts
(`task.<catalog_task_id>.*`, Workflow #308 slice 5b); the `backend.` family has
the backend self-check facts (`backend.selfcheck.*`, Workflow #317 slice sc1); the remaining producers
(`host.`, … facts), policy-input rewiring, and the per-instance read operation land
in later slices.

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
`RuntimeFactSnapshot` is the sealed image of the live records bound to one
ledger position and carries schema version `actingcommand.runtime-fact.v1`.
Since Workflow #308 slice 5d-1 one image may be one part of a snapshot set:
`snapshot_id` (= `ledger_position`, the same in every part of one set),
`part` (from 1) and `parts` (at least 1). All three are defaulted on read, so a
snapshot sealed before them reads as part 1 of 1 with `snapshot_id` 0; the
change is additive on the wire, and a `deny_unknown_fields` reader pinned to an
older Runtime moves its pin.
Its `validate` checks one part: it rejects a foreign schema version
(`runtime_fact_schema_version_mismatch`), a zero ledger position
(`runtime_fact_ledger_position_invalid`), a `part` outside `1..=parts`
(`runtime_fact_snapshot_part_invalid`), more than 16 384 records
(`runtime_fact_snapshot_too_large`), and any invalid record; it is structural
only. A serialized part larger than `MAX_RUNTIME_FACT_SNAPSHOT_BYTES` — the
same 512 KiB bound one `fact.observed` event may carry — is a write-side rule:
`validate_for_append` rejects it with `runtime_fact_snapshot_payload_too_large`
when a part is sealed and again in `EventDraft::sanitize`. A decoded part is
never re-serialized for validation (Workflow #191 B1-S1), so a part sealed by an
older Runtime stays readable after an additive field. A store larger than one
part is sealed as several parts (see "Periodic snapshot"), never truncated.

## Store

`actingcommand_scheduler::facts::RuntimeFactStore` is memory-only and pure: no
clock, no ledger, no I/O. It holds at most 16 384 live records
(`MAX_RUNTIME_FACTS`, 4 096 before Workflow #308 slice 5d-1) keyed by scope and
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
- `snapshot(ledger_position, now)` returns the sealed image of every record as
  part 1 of 1 with `snapshot_id` = `ledger_position`; `replay(snapshot)`
  replaces the store from one part.

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
| `runtime.fact_snapshot` | `fact.snapshot` | one `RuntimeFactSnapshot` part | system links |

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

The store keeps no pending queue: the per-record append is the persistence
step. An instance-scoped record is observational: it survives a process crash
and becomes durable across power loss at the next durable commit; a
Runtime-scoped record (the configuration inventory) is durable (see
[Durability classes](ledger-store.md#durability-classes-workflow-191-i)).
Anything not appended is gone with the process (iron rule 13).

## Startup replay

`recover_runtime_fact_store` runs during `start_with_provider`, after
`runtime.started` / `runtime.takeover` and the `runtime.instance_bound`
events, next to the instance fact store's recovery:

1. The newest complete snapshot set is searched backwards from the ledger
   tail (Workflow #308 slice 5d-1): `runtime.fact_snapshot` events are read
   through `query_page` on that event type in sequence windows that start at
   the tail sequence and double, newest first; after a part other than part 1
   the next window reaches back no further than that part's `snapshot_id` (the
   set's earlier parts were appended after it). The search stops at the first
   complete set, so older snapshot events are not read. The newest part names
   the set's `snapshot_id` and `parts`; the set is complete when that part is
   part `parts` and the snapshot events before it are parts `parts - 1` down
   to 1 of the same `snapshot_id` and `parts`. An incomplete set (a stop while
   sealing) is skipped and the search continues with the next older part.
   Every part read is validated; a part that fails is fatal
   (`runtime_fact_replay_failed`).
2. Each skipped set is recorded, before replay, as one
   `runtime.lifecycle_observed` (severity `warning`, origin module `runtime`,
   system links) with phase
   `fact_snapshot_set_skipped { snapshot_id, parts_found, parts }`, where
   `parts_found` counts the contiguous parts found (`1 <= parts_found <
   parts`, otherwise `invalid_fact_snapshot_set_skipped`).
3. If a complete set exists, part 1 replaces the store with `replay` and the
   records of parts 2 to `parts` join it in part order through `record`.
4. Every `runtime.fact_recorded` / `runtime.fact_invalidated` with a sequence
   greater than the set's last part — or every one of them when no complete
   set exists — is applied in ledger order through `record` / `invalidate`,
   selected with `origin_module` `runtime-facts`. A `runtime.fact_snapshot`
   among them is a part of a skipped set and changes nothing.
5. Any rejection while replaying our own ledger (stale, invalid, capacity,
   missing, an unexpected payload under that module) is fatal:
   `runtime_fact_replay_failed`, with the offending sequence in the native
   detail. Nothing else is skipped.

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

`append_runtime_fact_snapshot_if_dirty` seals the store only when it is dirty,
under one `fact_write_gate` hold: it reads the ledger's latest sequence and the
clock, builds the snapshot, splits its records (in store order, greedily by
serialized size) into parts of at most 512 KiB serialized, all with
`snapshot_id` = that sequence and the same `parts` (an empty store is one empty
part), checks every part with `validate_for_append` (the size, part and
position codes above surface
here as fatal host errors, before anything is appended), appends one
`runtime.fact_snapshot` per part in part order, and clears the dirty flag only
once every part is appended. A failed part append is fatal like any failed
append; the parts already appended form an incomplete set that startup replay
skips. A store that never changed appends nothing, so there is no ledger noise
before producers exist.

The performance monitor thread calls it with its own elapsed accumulator, so a
seal is attempted at most once every `RUNTIME_FACT_SNAPSHOT_INTERVAL_MS`
(60 000 ms) regardless of the performance sample interval. When that thread
is not spawned (no performance sample interval and frame retention disabled)
or has exited (sampling stopped with retention disabled), periodic snapshots
stop; durability then rests entirely on the per-record events, which is the
rule anyway (instance-scoped records are observational, durable across power
loss from the next durable commit). The snapshot only shortens replay.

## Read operation

`RuntimeOperation::RuntimeFactSnapshot` (no fields, no source gate beyond a
valid client origin, like `Status`) returns
`RuntimeResult::RuntimeFactSnapshot { snapshot }`: the live store sealed under
the write gate at the ledger's latest sequence, with `taken_at_unix_ms` from
the host clock, as part 1 of 1 with `snapshot_id` = that sequence (the 512 KiB
part bound does not apply to the read). `ledger_position` therefore names the
last event the reader can rely on having been applied; every accepted record
and invalidation at or below it is reflected. The read appends nothing and does not mark the store
dirty. `RuntimeHost::runtime_fact_snapshot` and
`RuntimeClient::runtime_fact_snapshot` expose the same read. Over IPC, a store
whose reply receipt would exceed the host's `maximum_frame_bytes` is refused
with the request error `runtime_fact_snapshot_too_large_for_frame` (receipt
state `denied`, not fatal) instead of failing the frame write. There is no
paged read yet (`actingctl facts --program` reads the whole store); a paged
read is the follow-up. The offline read below has no frame bound.

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
2. The newest complete snapshot set at or below `position` is found with the
   startup rule (Workflow #308 slice 5d-1; the same contract function): the
   sequences of the snapshot events are read without payload, then the parts
   are read one at a time, newest first, each validated, until the first
   complete set. Its parts' records fill the empty store in part order;
   without a complete set the store stays empty. An incomplete set is passed
   over without a record (the read writes nothing).
3. Every `runtime.fact_recorded` / `runtime.fact_invalidated` after that
   set's last part, through `position`, is applied in ledger order under the
   store's rules; a `runtime.fact_snapshot` among them (a part of an
   incomplete set) changes nothing. An identical record changes nothing; an
   older or equal observation, a new key on a full store, an invalid record or
   snapshot part, an absent key on invalidation, or any other payload under
   `runtime-facts` fails with `runtime_fact_replay_failed` naming the sequence.
   No partial store is returned.

Takeover and device-close drops need no rule of their own: every dropped key is
its own `runtime.fact_invalidated` event. A position between `runtime.takeover`
and that takeover's `runtime_takeover` invalidations shows the previous epoch's
records; the host serves no online read there.

The result `ForensicRuntimeFactsResult` is one of:

- `available { source, position, facts }` — `facts` is the `RuntimeFactSnapshot`
  the online read returns, records in scope-then-key order, with
  `ledger_position` = `snapshot_id` = `position`, part 1 of 1.
  `taken_at_unix_ms` is the ledger timestamp of the event at `position`; the online read samples the host clock instead,
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

Seven producers write the store today; all go through the append-first rule
above, with source `runtime` except the settlement facts (source `policy`) and
the backend self-check facts (source `device-proxy` or `capture`).

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
- Settlement facts (Workflow #308, slice 5b) — instance scope (the registered
  instance the policy run was dispatched to), source `policy`, no lifetime,
  all four with `observed_at_unix_ms` = the run's settlement time. They describe
  the latest settled policy run of one catalog task on one instance:
  - `task.<catalog_task_id>.last_duration_ms` (`integer`): the run's
    `runtime_ms` when it succeeded; for a failure, the execution record's
    `observed_at_unix_ms` minus the admission's `admitted_at_unix_ms` (0 when a
    settlement reconciled from ledger timestamps precedes its admission).
  - `task.<catalog_task_id>.last_outcome` (`string`): `succeeded` or `failed`.
  - `task.<catalog_task_id>.failure_streak` (`integer`): consecutive failed
    runs of that task on that instance ending at this run; 0 after a success.
  - `task.<catalog_task_id>.completed_at_unix_ms` (`integer`): the settlement
    time.

  The host records them right after `policy.execution_recorded` and its
  `policy.dispatch_completed` are appended (live settlement, the replayed
  completion of an already recorded execution, and the reconciliation trigger),
  computed from the policy dispatch records, so the values equal the evaluator
  inputs of "Settlement feedback and eligibility age" in
  `contracts/scheduling/README.md`. Once per startup, after the configuration
  facts, the host derives the four facts of every pair's latest settled run
  again: settlements reconciled during startup (before this store exists) and a
  stop between a settlement and its facts reach the same store state as the
  live path, and identical records append nothing, so replay never appends a
  record twice. A pair whose instance is no longer registered has no instance
  scope and keeps what the ledger holds. Any refusal while recording them
  (stale, capacity, invalid, unknown instance) is fatal: the settlement is
  already durable. They survive a takeover and emulator `stop`.
  The key bound limits a catalog task id to 102 bytes (128 −
  `task.` − `.completed_at_unix_ms`); catalog activation, rollback and
  promotion refuse a catalog with a longer task id with
  `task_id_too_long_for_facts` (request class) before anything is appended, so
  no run of such a task can exist.
- Backend self-check facts (Workflow #317, slice sc1) — instance scope, no
  lifetime, source `device-proxy` for the `input` and `nemu` entries and
  `capture` for `capture`. Every original open of one entry for one instance,
  success or failure and whatever the provider (native, fixture simulation,
  unobserved), is recorded as one `runtime.lifecycle_observed` event with phase
  `backend_open_observed` (`backend-open-observation.md`); right after that
  event the host records four facts from its report, all from one host-clock
  sample, under `backend.selfcheck.<entry>.` where `<entry>` is `input`,
  `capture` or `nemu` (the `nemu_pair` entry):
  - `status` (`string`): `passed` when the report's `status` and `connection`
    are `passed` and so is the entry's own check (`input_check` for input,
    `capture_check` for capture, both for nemu); `failed` when any of those is
    `failed`; otherwise `unknown` (an unobserved provider, a fixture
    simulation, or a check the open did not make).
  - `generation` (`integer`): the report's `session_generation`.
  - `selected` (`string`): the selected backend name, or `-` when none was
    selected.
  - `checked_at_unix_ms` (`integer`): the clock sample, saturated at the
    largest integer for a wall clock beyond it (the records'
    `observed_at_unix_ms` stays exact).

  Each record's `observed_at_unix_ms` is the clock sample, or one millisecond
  after the stored record of the same key when the sample is not later (an
  open of the entry in the same millisecond or after a wall-clock step back),
  read and written under one `fact_write_gate` hold, so a newer open of the
  same entry always replaces all four and never meets `runtime_fact_stale`.
  A report whose event links carry no instance fails with
  `backend_selfcheck_instance_missing` (fatal); every refusal of a record is
  returned to the open's observation consumer, whose existing failure path
  poisons the Runtime exactly as a failed event append does. For a device
  self-checked instance (`instance-fact-store.md`, "Backend self-check
  availability") one `device.self_check` status hint follows the four facts
  under the same gate hold (`backend-open-observation.md`, "Device self-check
  event", Workflow #317 sc3). A lease release, a task end
  or a retained-session close leaves them in place: they describe the last
  open. Invalidated with `device_closed` by emulator control after `stop`,
  `start` and `restart`, once the endpoint was rebound (or returned to
  pending) and still under the instance admission guard, so a session opened
  on the new binding never loses its facts; and with `runtime_takeover` as
  part of the `backend.` family. An ADB failure leaves them in place. The
  policy layer does not read them; a device self-checked policy instance is
  available only while they pass (Workflow #317 sc3, which replaced sc2's
  bounded reading; `instance-fact-store.md`, "Backend self-check
  availability"). The connection preparation phase below writes them at
  startup, after emulator `start` / `restart`, on an instance resume and on
  `SelfCheckInstance`. `actingctl facts --program` lists them.
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

- `host.scheduling_pause` (Workflow #361 B1) — runtime scope, `record_list`,
  no lifetime: the operator's held scheduling pauses, one row per pause
  (`scope`, `instance_alias`, `reason_code`, `since_unix_ms`,
  `set_in_owner_epoch`, `restored_from_owner_epoch`), recorded before every
  pause or resume changes the gate and again by the start that restores them
  (`contracts/scheduling-pause.md`, "Persistence, no expiry").

`contracts/actingd-check-config.md` lists the subsystems and parameters
`actingd` reports; `actingctl facts --program` returns both records as part of
the snapshot and `actingctl status --config` returns just the two (see "Read
operation").

## Connection preparation phase

Workflow #317 sc3: opens happen inside leases (lazily, inside contained
tasks), so the host connects and self-checks a device outside any client
lease in a controlled preparation phase (`prepare_instance_connection`, host
internal). For one physical instance, under its admission guard:

1. **Preparation lease.** The host grants itself a dedicated lease following
   the resource-close lease precedent: `prepare_resource_close` (a
   resource-close-only lease, so no input can be sent under it; a waiting
   lease queue is `lease_transfer_not_safe`) for the fixed Runtime connection
   of the preparation phase, granted through `grant_prepared_lease_with_links`
   with `CapacityUse::Drain` (`lease.granted`, synthetic links). A refusal (an
   active lease `lease_busy`, the takeover cooldown `lease_cooldown`, a queue)
   skips the instance: one `runtime.failed` record with lifecycle stage
   `runtime.lifecycle.connection_preparation` names the instance and the code,
   and its availability is withdrawn.
2. **Opens.** The preparation phase closes a retained session before it
   connects, so the self-check covers every required entry (Workflow #191 h3:
   a read-only observe keeping its capture session open without a lease is the
   existing sc2 design, and the preparation phase reconciles it; the close is
   step 3's fenced close with its `ResourceQuiescence` record, and a failed one
   skips the opens and ends the phase as in step 3).
   `ExecutionKernel::open_instance_backends` opens the input and
   capture backends through the provider opens the lazy paths use (a Nemu pair
   once) and takes the first frame of a capture it opened, which is dropped
   inside the open (no frame artifact is written). The opens are recorded like
   every open: `runtime.lifecycle_observed` `backend_open_observed`, the four
   `backend.selfcheck.<entry>.*` facts, one `device.self_check` per entry and
   the availability they decide. A failed open is recorded the same way with
   its failure report.
3. **Keep and release.** When the self-check passed its opens, the session
   stays open for the instance's next leases (Workflow #191 H: the session
   belongs to the instance and only an external command disconnects it) and the
   preparation lease is released (`lease.released`). After a failed open, or for
   an instance outside the multi-Nemu gate (`read-session-resource-close.md`,
   "Device session lifetime"), the session is first closed through the fenced
   close path and the lease is released once the close is confirmed. The
   self-check facts stay. A failed close is recorded by the close path and
   withdraws availability; an unconfirmed one is fatal, as on every close path.

Nothing is sent to the device and nothing is retried but the daemon start's
cooldown retry (below). The phase answers the
self-check projected from its own opens in the shape of an instance resume's
(`scheduling-pause.md`, "Resume"), with the code of the failing step as
`failure_code`. Only a fatal failure (a ledger append, an unconfirmed close)
is returned as an error; every other failure leaves the instance unavailable.

**Triggers.**

- **Daemon start** — after the registry, the fact seeds, the configuration
  manifest, the settlement facts and the agent session expiry, before any
  thread is spawned and so before `actingd ready`: every registered device
  self-checked instance, in registry order, is first withdrawn
  (`backend_selfcheck:unchecked`) and then prepared. A failed preparation does
  not stop the start; a fatal one does. Workflow #191 h2: an instance whose
  preparation lease the takeover cooldown refused (`lease_cooldown`, after an
  unclean restart) is prepared once more after the first pass, once its
  cooldown deadline has passed and still before `actingd ready` (the whole wait
  is bounded by `takeover_cooldown_ms`); a second refusal is recorded like the
  first and leaves it unavailable.
- **Emulator control** — after a successful `start` / `restart` of a device
  self-checked instance (its `command.validated`, `runtime.instance_bound` and
  `device.connected` recorded, before its startup package is scheduled); the
  invalidation of the control already withdrew availability. A failed
  preparation does not fail the completed control action. The stuck-recovery
  emulator-restart rung drives the same path.
- **Instance resume** — `ResumeScheduling { Instance }` reconnects through the
  preparation phase (`scheduling-pause.md`, "Resume").
- **`SelfCheckInstance { instance_alias }`** — the operator's manual reconnect
  and self-check of one physical instance (`actingctl selfcheck <alias>`,
  origin gate User+Ui or Cli+Cli, `invalid_emulator_control_origin` otherwise),
  answered with `RuntimeResult::InstanceSelfChecked { instance_alias,
  selfcheck }`.

Fixture instances and providers without a device endpoint are not prepared by
the automatic triggers.

## Typed codes

Contract: `runtime_fact_ledger_position_invalid`,
`runtime_fact_snapshot_part_invalid`, `runtime_fact_snapshot_too_large`,
`runtime_fact_snapshot_payload_too_large`, `invalid_runtime_fact_snapshot`,
`invalid_runtime_fact_recorded_action`, `invalid_runtime_fact_invalidated_action`,
`invalid_runtime_fact_snapshot_action`, `invalid_fact_snapshot_set_skipped`; manifest:
`config_manifest_subsystems_too_many`, `config_manifest_parameters_too_many`,
`invalid_config_subsystem_name`, `invalid_config_subsystem_reason`,
`invalid_config_parameter_key`. Host: `runtime_fact_stale`,
`runtime_fact_capacity_exceeded`, `runtime_fact_missing`,
`runtime_fact_instance_unknown`, `task_id_too_long_for_facts` (request class;
a refusal while recording settlement facts is raised as fatal with the same
code); `runtime_fact_store_desync`, `runtime_fact_replay_failed`,
`invalid_runtime_config_manifest`, `policy_settlement_instance_unknown`,
`policy_settlement_missing`, `policy_settlement_fact_overflow`,
`backend_selfcheck_instance_missing`, `backend_selfcheck_fact_overflow` (fatal);
`runtime_fact_snapshot_too_large_for_frame` (request class, the IPC read). Offline read:
`runtime_facts_source_incomplete`, `runtime_facts_position_missing`,
`runtime_facts_read_budget_exceeded`; `actingledger facts`:
`runtime_facts_not_available`.
