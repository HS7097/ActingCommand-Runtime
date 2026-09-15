# Runtime fact store

The Runtime keeps a second, separate fact store for facts about itself. It is
distinct from the instance fact store (`crates/actingcommand-contract/src/fact.rs`,
`InstanceFactStore`), which carries observations of the game or application
under automation. A runtime fact never describes game state, and a game
observation never enters the runtime fact store; the two key families are
disjoint by construction.

Workflow #313 owns the design; this document freezes the contract half and the
pure store. Host wiring (producers, sealing, policy input construction) lands
after the host split in Workflow #161.

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
`device_closed`, `expired`, `operator`). `RuntimeFactSnapshot` is the sealed
image of every live record bound to one ledger position and carries schema
version `actingcommand.runtime-fact.v1`.

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

## Persistence

Iron rule 13 applies: the store never writes a file. The host appends every
accepted record to the `GlobalLedger` before calling `record`, seals the store
on a fixed period as a snapshot event, and rebuilds it after a restart from the
latest snapshot plus the records appended after it. The store keeps no pending
queue: the per-record append is the durability step, and the snapshot only
shortens replay. Anything not appended is gone with the process.
