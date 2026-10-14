# Instance fact store

The instance fact store (`InstanceFactStore`, private to `runtime-host`) is the
fact projection rebuilt from the ledger: observations of the game or application
under automation, scoped to one instance, one server or one game. The contract
half lives in `crates/actingcommand-contract/src/fact.rs` (`FactRecord`,
`FactScope`, `FactValue`, `InstanceFactContext`, `InstanceFactSnapshot`); a key
belongs to exactly one of the eight families `identity.`, `display.`,
`session.`, `resource.`, `inventory.`, `timeline.`, `health.`, `env.`. The
Runtime's facts about itself live in the separate runtime fact store
(`contracts/runtime-fact-store.md`); the two key families are disjoint.

Every change is one ledger event under the `fact_write_gate`: `fact.published`
carries one observation (1–256 records of one scope and one
`source_snapshot_id`), `fact.invalidated` drops one record. The store recovers
at startup by replaying every event, afterwards synchronizes incrementally from
`last_sequence + 1`, and can replay history at an exact ledger position
(`at_position`; position 0 or one beyond the latest sequence is refused).
`docs/live-fact-pools.md` describes the adapter publication path and the
store's acceptance rules (idempotent identical retries, `not newer`, capacity,
invalidated snapshots, incomplete refreshes).

The official input path holds `fact_write_gate` from outcome construction through the durable
`input.committed` or `input.failed` append and synchronization of its declared invalidations.
Policy admission holds the same gate from its authoritative fact projection through the
dispatch intent append. Thus an input result that precedes a later intent also precedes that
intent's fact validation, including shared game/server facts across instances. An already
committed intent retains its existing execution semantics. Device execution, lifecycle
cleanup and pipeline observation run outside this hold; the lock order remains
`policy_outcome_gate` → `policy` → `fact_write_gate` → facts. Lock, append or synchronization
failure propagates as a fatal host failure.

> **Superseded by Workflow #383** (the two paragraphs above): the store stops being a projection
> rebuilt from the ledger. See "Accept first, revisions and tagged load" below; AO-3 (single
> entry, revisions, gate order) and AO-6 (start load) delete those paragraphs when they merge.

This document freezes the store's role as the source of the policy input
instance set (Workflow #313, item 4).

## Accept first, revisions and tagged load (Workflow #383 target)

The target of [Ledger observer](ledger-observer.md) for this store (LEDG §3.2–§3.5, §8). Slice
AO-3 implements the entry, the revisions and the gate order; AO-6 the start load.

- **One entry.** `InstanceFactStore::apply(change)` takes every change: publication, invalidation,
  seed, derived invalidation, add or subtract. The store applies an accepted change first; the gate
  G2 then copies it into the capture queue. The acceptance rules stay exactly as above (identical
  records are idempotent, same-millisecond or older observations are refused, a full store refuses
  without eviction). A refused change keeps the revision, returns to the caller as a request error
  and produces no `fact.*` record.
- **Revision and lineage.** The store has a monotonic `revision` (+1 per accepted change) and a
  `lineage` that is new at every process start, whether the store starts empty or loads tagged
  records. The ledger therefore never holds the same (lineage, revision) twice; the base record
  `tagged_state_loaded` says from which previous (lineage, revision) the new lineage continues.
- **Fact snapshot id** = hash(context, instance store (lineage, revision), workbench (lineage,
  revision), records), no longer the ledger position. Admission compares it as before
  (`policy_facts_stale`).
- **Order guarantees** (the three of today, held by in-memory order): an input's result and its
  invalidations are applied under `fact_write_gate` before any later intent's fact check (shared
  game and server facts included); the store commit and the gate's copy are one ordered step, so
  ledger order equals business order; the workbench shares the gate. Nothing waits for the ledger
  inside the gate or the `policy` lock.
- **`observe_trigger` invariant.** Every recorded event passes `observe_trigger` at capture, in
  capture order, through the host's single `capture` function, so `invalidate_on` keeps working for
  any event type except those [refused as triggers](ledger-observer.md#3-the-fan-out-point-and-the-observe_trigger-invariant-ao-3).
- **Add or subtract.** A record of an add or subtract change carries the value after the change
  (it may also carry the delta). Amounts come only from live recognition (D2 Q-D2).
- **Start load.** Every key of this store is tagged. At start the store is loaded from the index
  table's `state` rows and the records they point to (ledger-observer.md §11.5), not by replaying
  the ledger; `latest_observed`, tombstones and the input boundary are derived from the same
  records. Tombstones stay in the store (at most 256). History at a ledger position
  (`at_position`) remains for forensics only.
- The projection below reads the store at the evaluation's revision pair instead of a
  `ledger_position` (AO-4; the `input_ledger_position` member of the dispatch intent follows
  [the v0.12.0 plan](v012-register.md)).

## Adjustment invalidations (Workflow #384 D2-S3)

A run's confirmed resource adjustment that reads no new value of its key drops the key's record
(D2 model §3.6; [resource-readings.md](resource-readings.md), "Adjustments"). The drop is one
`fact.invalidated` (no new event type or member) whose trigger is the confirmed step's
`task.step_finished`, on the run's instance. It is the one invalidation a record need not list
in `invalidate_on`: `fact.invalidated` of an instance-scoped `resource.*` or `inventory.*` key
whose `invalidated_by_event_type` is `task.step_finished` (`is_adjustment_invalidation`). Every
other rule holds: the record was published before the trigger, the trigger precedes the
invalidation and its timestamp is `invalidated_at_unix_ms`, the trigger's instance is in the
record's scope, and the tombstone projection is written in the same transaction. The live store
(`HostShared::invalidate_adjusted_fact`, under `fact_write_gate`), its replay, the history at a
ledger position and the state projection's verification apply the rule alike; no invalidation
of this kind is derived from an event.

## Policy instance facts

Three keys of the `session.` family describe one instance to the scheduling
evaluator. The configuration seeds them; the store owns them.

| Key | Value | Meaning |
| --- | --- | --- |
| `session.instance.available` | `boolean` | `InstanceSnapshot.available` |
| `session.instance.capabilities` | `record_list`, one row `{ "operation_id": <string> }` per entry | `InstanceSnapshot.capability_operation_ids` |
| `session.instance.preferred_tasks` | `record_list`, one row `{ "task_id": <string> }` per entry | `InstanceSnapshot.preferred_task_ids` |

The evaluation consumes these keys through the instance projection below; they
are never overlaid as ordinary `facts` of an evaluation or of a forward
projection, and a catalog predicate cannot observe them as facts.

## Seeding at startup

`start_with_provider` seeds the store right after the instance fact store is
synchronized from the ledger, before the configuration manifest is recorded,
before any thread is spawned and before the host is announced ready. A host
without configured policy inputs (`RuntimeHostConfig::with_policy_inputs`
absent: tests without a policy section, `actinglab` goldens,
`ledger-maintenance`) seeds nothing.

For every configured policy instance — the `policy.facts.instances` entries of
the `actingd` configuration, as `PolicyInputSnapshot` carries them — the host
publishes the three records above with scope `instance { instance_id: <alias>
}`, each through the internal publish path (`publish_fact`: system links,
source `runtime`, origin module `fact-store`, actor `runtime`), one
`fact.published` event per key. The records share:

- `observed_at_unix_ms` — the host clock when the seed is published;
- no `expires_at_unix_ms`, no `ttl_policy`; `confidence_milli` 1000;
- `source_detector` `runtime.policy-configuration`, `schema_version` `fact.v1`;
- `source_snapshot_id` `snapshot:policy-config:<digest>` and
  `resource_bundle_hash` `<digest>`, where `<digest>` is the lowercase hex
  SHA-256 of the canonical JSON array `[<alias>, <key>, <value>]`: a stable hash
  of the configured value, one per instance and key;
- no `invalidate_on` events.

**Idempotency.** Before publishing a key, the host reads the active record
stored under that scope and key. When it carries the same `source_snapshot_id`,
the key is already published: the stored content is verified against the
configured value (`policy_instance_seed_identity_conflict`, fatal, on any
difference) and nothing is appended, so a restart with the same configuration
leaves the ledger unchanged whatever the clock now says. Otherwise — no record,
or a record of another snapshot id because the configured value changed or
another producer overwrote the key — the seed is a new observation at the host
clock and replaces the stored record; it must be strictly newer than the stored
observation. A configuration change that leaves an instance's three values
unchanged appends nothing for that instance.

**Failure.** Every refusal fails startup through the normal abort path (Fail
Loud); only the idempotent path is success. The refusals are the store's own:
`fact_record_invalid` (for example more than 256 capability or preferred-task
identifiers, the `record_list` bound), `fact_observation_not_newer`,
`fact_store_capacity_exceeded`, `fact_source_snapshot_invalidated` (a seed of
this snapshot id was invalidated), `policy_fact_authority_conflict` (the
configured static `facts` declare the same key for the same instance), and the
ledger's append errors. `policy_instance_seed_encode_failed` (a seed that cannot
be serialized for hashing) and `policy_instance_seed_identity_conflict` are the
fatal codes of the seeding itself.

The test hook `evaluate_policy_cycle_with_test_inputs` replaces the
configuration and seeds the store the same way before it evaluates.

**Device self-checked instances** (Workflow #317 sc3, "Backend self-check
availability" below): for a policy instance on a physical device the host does
not seed `session.instance.available`; its startup connection preparation
phase publishes that key instead (`false` first, the seed's own record once the
self-check passes). Its two other keys are seeded as above.

## Backend self-check availability

Workflow #317 slice sc3 (the strict reading of #316 goal 5, replacing sc2's
bounded one): a policy instance on a physical device is available only while
its configured seed says so **and** its latest backend self-check passed.

**Which instances.** The rule, and the connection preparation phase that
drives it (`runtime-fact-store.md`, "Connection preparation phase"), apply to a
*device self-checked* instance: a registered instance with physical-device
provenance and a device endpoint (a bound or pending ADB endpoint), which is
every physical instance the device registry assembles. A fixture simulation,
and a provider that resolves physical provenance without any endpoint (the
test fakes), has no device connection to check: its `session.instance.available`
stays the configured seed, whatever its opens report. Only a configured policy
instance is gated: without policy inputs, or for an instance that is not a
policy instance, nothing is published.

**The self-check passes** when no `backend.selfcheck.<entry>.status` of the
instance (`runtime-fact-store.md`, "Producers") holds `failed` and either
`nemu` holds `passed` (a Nemu pair covers input and capture) or both `input`
and `capture` hold `passed`. `unknown`, a missing entry and `failed` all fail
it.

**Decisions.** The host re-decides after every recorded open (right after its
four facts and its `device.self_check` event) and after emulator control
invalidated the facts; the recording, the decision and its publication hold
one host-wide self-check availability gate (taken before the
`fact_write_gate`), so concurrent opens and invalidations cannot interleave.
It reads the instance's active record of scope `instance { instance_id:
<alias> }`:

- A failing self-check publishes `session.instance.available = false` unless
  that record already holds `false`. The record goes through the seed's publish
  path and scope (`publish_fact`: system links, source `runtime`, origin module
  `fact-store`, actor `runtime`, one `fact.published` event) and has the seed's
  form except `source_detector` `runtime.backend-selfcheck` and
  `source_snapshot_id` `backend_selfcheck:<cause>`: `<entry>:<generation>` of
  the open just recorded (`<entry>` is `input`, `capture` or `nemu`,
  `<generation>` its session generation), or `unchecked` when no recorded open
  caused it (startup before the preparation phase, an invalidation, a failed
  preparation step); `resource_bundle_hash` is the seed digest of the value
  `false`, and the observation time is the host clock.
- A passing self-check, when the active record is such a `backend_selfcheck:`
  record, republishes the configured seed — the seed's own record (detector,
  snapshot id and digest of the configured value) as a new observation at the
  host clock. An instance configured `available = false` therefore stays
  `false`.
- Workflow #398 RD-1b: a handset the private binding file binds no device to
  ([device-binding.md](device-binding.md), "Unbound handset") is unavailable
  whatever its self-check says: the startup withdrawal and every decision
  publish `false` with `source_detector` `runtime.device-link` and
  `source_snapshot_id` `device_link:unbound` (the device link's state, beside
  `backend_selfcheck:`), unless that record is already in effect; a passing
  self-check never restores it.

**Lifecycle.** At startup every device self-checked policy instance starts
unavailable (`backend_selfcheck:unchecked`, published right before its
preparation phase) and becomes available when that phase's opens pass; a
self-check fact a clean restart kept never makes an instance available before
this start's preparation recorded new opens. Emulator `stop`, `start` and
`restart` invalidate the self-check facts and withdraw availability until the
preparation phase that follows a successful `start` / `restart` passes; an
owner takeover drops the facts with the `backend.` family and the startup
withdrawal applies. A lease release, a task end or a retained-session close
keeps the facts and the availability. A preparation step other than an open
that fails (its lease was refused, the session close failed) withdraws
availability with `backend_selfcheck:unchecked`; the next recorded self-check
decides again.

The evaluator is unchanged; it excludes an instance projected
`available = false`, so the policy does not dispatch to it until the seed value
returns. A refusal of the publication (for example
`fact_observation_not_newer` after a wall-clock step back) is returned to the
open's observation consumer, whose existing failure path poisons the Runtime
(at startup it fails the start); a fatal refusal marks the Runtime fatal as
every `publish_fact` does, and an instance the registry does not hold fails
`backend_selfcheck_instance_unregistered` (fatal).

## Projection into the evaluation

`project_authoritative_policy_inputs_under_gate` (every evaluation, dispatch
admission, policy input identity and strategic report) builds the evaluation's
`instances` from two sources:

1. the configured **static identity** of each policy instance — alias,
   `host_id`, `server_id`, `game_id`, taken from the configuration as before;
2. the fact-store projection at the evaluation's `ledger_position` — the live
   store at the latest sequence, or the `at_position` replay for an earlier
   position — resolved per instance for the three keys.

For each identity the host resolves each key among the active records that
apply to the instance's context (`InstanceFactContext { instance_id, server_id,
game_id }`), the most specific scope first (instance over server over game).
`available` is the `boolean` value; `capability_operation_ids` and
`preferred_task_ids` are the row field values of the `record_list`, every row
exactly one string field of the documented name. The projected lists are sorted
as before.

**Fail-closed.** A key with no applicable active record, or whose record is
artifact-backed or of another shape, is *missing*. An instance missing any of
the three is projected `available = false` (its lists are what was found,
empty for a missing key); it is never silently available and the projection
never panics. Expiry and confidence of these records are not evaluated — the
seeds carry neither — so the projection stays a pure function of the
configuration and the ledger position.

**Explanation.** For a cycle evaluated by `evaluate_policy_cycle`, every
`TaskDecision` made for such an instance carries, after the evaluator's own
reasons (`instance_unavailable`, …), one `DecisionReason` per missing key whose
outcome is `policy_instance_fact_missing` (warning) with the key as `fact_key`, and
a detail naming the instance and the ledger position. No decision on such an instance can be selected, so no
dispatch intent, reason chain or `policy.dispatch_intent` event carries the
reason; the other projections use the same instance set without it.

**Identity.** The projected `instances` are hashed into `fact_snapshot_id` as
before, so a changed availability, capability list or preferred-task list
changes the identity and an already evaluated dispatch is refused as
`policy_facts_stale`. Because the three keys are not overlaid as `facts`, the
evaluation's `facts` list and the `fact_snapshot_id` of an unchanged
configuration are the same as before the seeds existed: the first evaluation
after a startup with unchanged configuration produces the same inputs and
outputs as when `instances` came from the configuration directly.

## Policy input authority

`validate_policy_input_authority` compares the registered alias set with the
configured static identity set (`policy_instance_metadata_untrusted` on any
difference) and requires every configured `host_id` to exist in
`resources.hosts` (`policy_resource_metadata_untrusted`). The configured
`available`, `capability_operation_ids` and `preferred_task_ids` are seeds, not
authority, and are not compared.

## Unchanged

The `facts` and `outcomes` sources of the configuration and their conflict
checks (`policy_fact_authority_conflict`, `policy_outcome_authority_conflict`),
the `ObservedOutcome` projection, the wire format, the eight key families and
the ledger event types are as before this slice.

## Priority offset facts

Workflow #308 slice 4a-2 adds manual priority offsets — a person's or an
agent's adjustment of a task's score (`EvaluationFacts.priority_offsets`,
`contracts/scheduling/README.md` "Score-Assisted Priority") — as ordinary
records of the `session.` family.

| Key | Value | Scope |
| --- | --- | --- |
| `session.task.<task_id>.priority_offset` | `integer`, the offset in milli | `instance` (that instance only), or `server` / `game` (a task-level offset for every instance of that server or game) |

**Publication.** Offsets travel through `PublishFact` / `PublishFacts` and the
normal fact rules (one scope and snapshot per observation, idempotent identical
retries, strictly newer observations, the 256-record store bound). The
publisher chooses the lifetime: an absent `expires_at_unix_ms` never expires;
a present one follows the usual TTL policy validation. On top of the ordinary
record validation the Runtime refuses, as the non-fatal request error
`priority_offset_invalid`, any offset record whose `<task_id>` breaks the
scheduling identifier charset (`^[a-z0-9][a-z0-9._:-]*$`, 1–128 bytes), whose
value is not an inline integer within ±1,000,000, or which declares
`invalidate_on` events (an offset ends by its TTL or by a newer offset, never by
an event). `FactRecord::priority_offset(scope, task_id, offset_milli,
observed_at_unix_ms, source_detector)` builds a record without expiry whose
snapshot id and bundle hash are the SHA-256 of that content.

**Origin exception.** The request origin gate of `PublishFact` / `PublishFacts`
accepts an observation whose every key is a priority offset key from `(agent,
adapter)`, `(user, ui)` and `(cli, cli)`. Any other observation stays
`(agent, adapter)` only (`invalid_agent_dispatcher_origin`), and an observation
that mixes offset keys with other keys is refused from every origin as
`fact_origin_mixed`.

**Ledger.** An offset-only observation published by a request keeps that
request's source and actor on its `fact.published` event (origin module
`fact-store`), so the ledger shows who set the offset; its links carry the
request and correlation identities as for every request publication. Every
other `fact.published` event keeps source `runtime` and actor `runtime`. No
event type is added.

**Projection.** `project_authoritative_policy_inputs_with_gaps_under_gate` reads
the active offset records at the projected ledger position whose scope applies
to at least one projected instance. An instance-scoped record becomes the entry
`{task_id, instance_id: <alias>}`; a task's single server- or game-scoped record
that applies to every projected instance becomes the task-level entry
`{task_id, instance_id: null}`, which the evaluator overrides per instance with
an instance-level entry. When a task's server/game records apply to only part of
the projected instances, or several apply, a task-level entry cannot express
them, and every projected instance instead gets its own entry from its most
specific record (instance over server over game). `origin` is `user` when the
publishing event's actor is `user` or `cli`, `agent` otherwise; `observed_at_unix_ms`
is the record's. For a cycle evaluated by `evaluate_policy_cycle` an offset
whose `expires_at_unix_ms` lies before the evaluation instant is not passed; its
expiry does not by itself wake the evaluator. The other projections (admission,
policy input identity, strategic report) pass every active offset. An entry that
collides with a statically configured `priority_offsets` entry of the same task
and instance is refused as `policy_fact_authority_conflict`. A record whose task
identifier or value breaks the publication rules (only possible for a record
published before those rules) fails the projection as `priority_offset_invalid`.

Offset keys are never overlaid as ordinary `facts` of an evaluation or of a
forward projection, and a catalog predicate or a selection document cannot
observe them as facts. The forward projection receives no store offsets.

**Unknown tasks.** An offset whose task the active catalog does not declare is
still passed (the evaluator ignores it). A cycle evaluated by
`evaluate_policy_cycle` reports each such task once: one extra `TaskDecision`
of that task with no instance, eligibility `unknown`, state `blocked`, no rank
and the single reason `priority_offset_unknown_task` (warning; value `task_id`). It is not an error,
selects nothing and reaches no dispatch intent or ledger event.

**Identity.** `combined_policy_snapshot_id` hashes, next to its previous
inputs, the `(scope, task_id, offset_milli, observed_at_unix_ms)` tuple of every
offset record the projection consumed, so a changed offset changes the
`fact_snapshot_id` and an already evaluated dispatch is refused as
`policy_facts_stale`. Without offsets the hashed input is exactly the previous
one, so identities of offset-free inputs are unchanged. This is not a wire
change.

**Configured game.** For the `actingctl task-offset` default scope, the control
plane status (`RuntimeInstanceStatus.game_id`) names the game of each
instance's configured policy identity; it is absent when the host runs without
policy inputs.

## Resource target policy facts

Workflow #308 RT-S1a stores an instance's resource target policy as the instance fact
`session.resource_targets` (`contracts/resource-targets.md`).

| Key | Value | Scope |
| --- | --- | --- |
| `session.resource_targets` | `record_list`: a `policy` header, `target` rows and `task` rows | `instance` only |

**Formal entry.** Only `ApplyResourceTargets` writes it, through `publish_facts` with the
request's origin (source `adapter`, actor `agent`) on the `fact.published` event. An ordinary
`PublishFact` / `PublishFacts` record of this key is refused before the fact write gate as the
non-fatal `resource_targets_formal_entry_required`. A configured policy input fact or a
forward projection caller's fact of this key is refused as `resource_targets_key_reserved`:
configured, it fails every authoritative projection (and so the daemon's first evaluation).

**Replay.** Under the fact write gate, after the store is synchronized, a policy whose
`resource_bundle_hash` (the policy's SHA-256 hex) equals the active, unexpired record's appends
nothing and answers the active revision (`FactPublication { replayed: true }`); otherwise the
active record's sequence is the new policy's `previous_version`.
`InstanceFactStore::active_revision(scope, key)` reads the active record with the sequence and
id of its event.

**Projection.** The key is not excluded from the overlay: an unexpired or expired policy is an
ordinary overlaid fact, so it reaches `EvaluationFacts.facts`, the combined
`fact_snapshot_id`, admission staleness and forward projections. It takes one of the 256
active fact identities per instance. The evaluator scores it as `contracts/resource-targets.md`
("Evaluation") states; a server- or game-scoped record of the key, which only an ordinary
publication before RT-S1a could write, is never a policy: it is ignored with the reason
`resource_target_policy_ignored` on the candidates of every instance it covers.

## Resource reading facts

Workflow #335 S5b. A package's resource readings (`contracts/resource-readings.md`) are
written by the host, through `publish_facts`, after a run that succeeds with a confirmed
settlement. Each reading is one observation with one instance-scoped `resource.` or
`inventory.` record. Its origin is the Runtime's own (source and actor `runtime`, origin module
`fact-store`), with system links. The observation time is the capture time of the frame the
reading was taken on (Workflow #384 D2-S2: a mark may stand on any page of the task), the TTL is
the declared `valid_for_ms` and `invalidate_on` is empty. The store
accepts or refuses it like any other observation. No write site is added: the host terminal
calls the existing `publish_facts`.

## Display facts

Workflow #392 DA-A (model 392 v1.1, sections 8.1 and 12.1): a device instance's display is
recorded, never checked. No frame is refused for its aspect ratio; a contained task's frame
must still equal its package's resolution (`contained_task_frame_resolution_mismatch`), and its
aspect ratio must still match the logical display it was captured from
(`contained_task_geometry_frame_display_mismatch`).

| Key | Value |
| --- | --- |
| `display.frame_extent` | `record_list`, one row `{ "width": <integer>, "height": <integer> }`: the device frame |
| `display.rotation` | `string`: `r0`, `r90`, `r180`, `r270`, or `unknown` when the capture source does not provide the rotation |
| `display.orientation` | `string`: `landscape` (width greater than height), `portrait` or `square` |
| `display.aspect` | `record_list`, one row `{ "width": <integer>, "height": <integer> }`: the frame reduced by the greatest common divisor of its width and height (2340x1080 is 13:6) |

**When.** The host writes the four keys when a contained task's first frame passes its task
geometry (`task.geometry_observed` with conclusion `pass`), for an instance bound to a device:
physical provenance and a device endpoint, the instances whose connection is self-checked
("Backend self-check availability" above). A fixture simulation and a provider without a device
endpoint record nothing.

**Record.** Scope `instance { instance_id: <alias> }`; one observation of the four records
through the internal publish path (`publish_facts`: system links, source and actor `runtime`,
origin module `fact-store`); the host clock as `observed_at_unix_ms`; no expiry, no TTL policy,
no `invalidate_on`; `confidence_milli` 1000; `source_detector` `runtime.display-adapter`;
`schema_version` `fact.v1`; `source_snapshot_id` `display:<digest>` and `resource_bundle_hash`
`<digest>`, the lowercase hex SHA-256 of the alias and the four values. The record carries
dimensions and rotation only, never a device identifier.

**Idempotency.** When the instance's active records of the four keys already hold these values,
nothing is appended, so the same display at the next run or after a restart leaves the ledger
unchanged; any changed value publishes all four again.

**Failure.** A refusal of the publication (for example `fact_observation_not_newer` after a
wall-clock step back) fails the run under its `task.geometry_observed` record, before any input;
a fatal refusal marks the Runtime fatal as every `publish_facts` does.

**No reader.** `display.aspect` is recorded only: no Runtime code reads it to decide anything
(architecture guard `da_a_t3_display_aspect_has_no_reader`).

## Reading the seeds

The seeds are visible as `fact.published` events of origin module `fact-store`
and source `runtime` in the ledger, through `RuntimeHost::instance_fact_snapshot`
for one instance context, and through the general read below.

**The instance fact read** (Workflow #389 MCP-4, [v0.12.0 register](v012-register.md) W2).
`RuntimeOperation::InstanceFactSnapshot { instance }` names a registered instance alias
(another is refused `instance_unknown`) and answers `RuntimeResult::InstanceFactSnapshot
{ snapshot, source }`. `snapshot` is an `InstanceFactView {instance_alias, server_id?,
game_id?, records}`: every active record of the instance's own scope and of the server and
game its policy identity binds it to (`policy.facts.instances`), read under the
`fact_write_gate` after the store caught up with the ledger, ordered by key, then scope. A
Runtime without a policy section binds no server and no game: both are absent and only the
instance's own records apply. The view carries no ledger position and no snapshot id
([ledger-observer.md](ledger-observer.md) §13); `source` is the read's
`RuntimeStateSource` (register W1, [runtime-state-observation.md](runtime-state-observation.md)).
The read appends nothing; a reply larger than the host's `maximum_frame_bytes` is refused
`instance_fact_snapshot_too_large_for_frame` (receipt state `denied`, not fatal).

`actingctl facts --instance <alias|instance_id|ADB port> --state-root <state-root>
[--key-prefix <prefix>]` and MCP `ac_facts` with `scope` `instance` resolve the selector with
one status read and print `{scope, read_at_unix_ms, snapshot, records_total, records_kept}`:
`read_at_unix_ms` is the source's `sampled_completed_at_unix_ms`, against which each record's
`observed_at_unix_ms` and `expires_at_unix_ms` tell its freshness; `--key-prefix` keeps only
the records whose key starts with it.

## Capacity ledger (Workflow #383 AO-0)

The store holds at most 256 active facts for the whole host (`MAX_ACTIVE_FACTS`,
`crates/runtime-host/src/fact_store.rs`), shared by every instance; a new key on a full store is
refused (`fact_store_capacity_exceeded`), never evicted. Tombstones have their own bound of 256 and
do not count. Every slice that adds keys to this store books them here first (LEDG §3.1, AO-0 row).

Per instance *i* with game *g* and server *s*; a server- or game-scoped key shared by several
instances is booked once.

| Family | Keys | Scope | Rows per instance | Slice | Today's deployment (3 instances, each with its own game and server) |
|---|---|---|---|---|---|
| policy instance seeds | `session.instance.available`, `.capabilities`, `.preferred_tasks` | instance | 3 | existing (#313) | 9 |
| resource target policy | `session.resource_targets` | instance | 1 | existing (#308 RT-S1a) | ≤ 3 |
| priority offsets | `session.task.<task_id>.priority_offset` | instance, server or game | one per task with an offset set | existing (#308 4a-2) | *O* (usually 0–3) |
| readings (D2 reality keys) | `resource.*`, `inventory.*` | instance | *R*: the distinct reading keys of the packages bound to the instance | existing (#335 S5b); refreshed by D2 | 44 (6 + 15 + 23: the distinct `fact_key` readings of the three resource repositories at `d41137c0`, `6dd529a1`, `dfb3729`) |
| pack schedule | `session.pack.schedule` | instance, server, game | ≤ 3 (one per scope used) | SR-11 | ≤ 9 |
| feature locks | `session.feature.<feature_key>` | instance | *F*: the distinct feature keys the bound packages declare | SR-5a, SR-5b | 0 (no package declares `feature_locks` yet) |
| instance performance baseline | `health.perf_baseline.<signal>.<backend>` | instance | 2 (two signals × backends in use, usually one) | P-m | 6 |
| instruction lines (reserved) | `session.instructions` | instance, server, game | ≤ 3 (one per scope used) | GOAL-1 | ≤ 9 |
| selector identities (#308 SEL-L2) | — | — | 0: the business identity domain is package data, not a fact | — | 0 |

- **Today's deployment total**: 9 + 3 + *O* + 44 + 9 + 0 + 6 + 9 = **80 + *O***, well under 256.
- **Five instances** (two more with unknown packages): 15 + 5 + *O* + *R₄* + *R₅* + 44 + 15 + 10 + 15
  = **104 + *O* + *R₄* + *R₅* + *F***; the reading keys of new packages are the variable that
  matters.
- **Outside the Runtime**: today's deployment generator publishes one-shot request facts
  (`session.w2.manual_request`, predicates over up to 200 such keys, D2M research B §3.3); each
  published request holds one active fact until it expires. The task request of MCP-17 (a
  `run_once` row in `session.instructions`) replaces them; until it does, the deployment must keep
  the number of live requests small.
- A slice that would push the computed total over 256 for the deployed instance count stops and
  reports instead of merging; the store keeps refusing, not evicting.

## Pack schedule facts

Workflow #386 slice SR-11 (scheduling model v1.4.2 §6.1). A person or an agent states, as a
state and not as a one-off command, during which periods a task package may be scheduled on an
instance. The state is stored in this store as one record of the `session.` family per scope.

**Status.** Written ahead of its code (documentation slice SR-0); slice SR-11 implements it.
Until then a `session.pack.schedule` record is an ordinary `session.` record that nothing reads.

| Key | Value | Scope |
| --- | --- | --- |
| `session.pack.schedule` | `record_list`, one row `{ "package_id": <string>, "from_unix_ms": <timestamp_ms>, "until_unix_ms": <timestamp_ms>, "schedulable": <boolean> }` per period | `instance` (that instance), or `server` / `game` (every instance of that server or game) |

**Rows.** A row says "from `from_unix_ms` until `until_unix_ms` (half-open), the task package
`package_id` is schedulable or not". `package_id` is the task package's id as the procedure
manifest names it; the granularity is the task package, so every catalog task of one package
opens and closes together, never one task and never a whole standard package. A row whose
`until_unix_ms` is not after its `from_unix_ms`, rows of one `package_id` that overlap, or a
value of another shape are refused as the non-fatal request error
`session_pack_schedule_invalid` (`fact_key`, `field` pointing at the member). One scope holds at
most the `record_list` bound of 256 rows, for all its packages together.

**Writing.** One observation writes a scope's whole list, atomically. The publisher chooses the
lifetime: no `expires_at_unix_ms` never expires. Publishing an empty list withdraws every row of
the scope; publishing the list without a package's rows withdraws only those. The writers are
`actingctl pack-schedule set` and `pack-schedule get`, the MCP general tools
`ac_pack_schedule_set` and `ac_pack_schedule_get` (no tier), and the UI console (slice UI-2);
they write the same rows. Under Workflow #389 the key is in the general key family
(`fact_key_family`, `contracts/runtime-interface.md`), so all three counters may write it, and
an observation that mixes it with keys of another family is refused as `fact_origin_mixed`.
Before #389's slice U-6 removes the origin exception, SR-11 accepts it like a priority offset:
from `(agent, adapter)`, `(user, ui)` and `(cli, cli)`. There is no offline writer: a write goes
through a running actingd, like `actingctl task-offset`.

**Which row applies.** For instance `I`, task package `P` and time `t`:

1. the instruction layer: the `session.pack.schedule` record that applies to `I`, the most
   specific scope first (instance over server over game, per key: an instance record replaces
   the server and game records as a whole); a row of `P` that covers `t` decides;
2. otherwise the standard package layer: the default rows the instance's standard package
   declares in a data file at its root, compiled when the catalog is activated. The Runtime
   only reads them and never stores them here; the file's name and format belong to the
   standard-package data-file model of Workflow #373 (NAV-M). A default row should not close a
   recognition maintenance package;
3. otherwise `P` is schedulable.

Schedulable does not mean that it runs: the task's own trigger, budgets, cooldown and blackouts
still decide, and a task that runs only on request still runs only on request.

**Evaluation.** When `P` is not schedulable for `I` at the evaluation instant, every candidate of
`P` on `I` is deferred with `pack_schedule_closed` (info; `package_id`, and
`next_eligible_unix_ms` the end of the covering row). Its place in the order of deferrals is
after an active blackout and after `pack_registry_not_loaded` of Workflow #389
(`scheduling-blackout.md`, "Order of the deferrals"). A row of a package that #389 has unloaded
stays stored and takes effect again when the package is loaded. A changed record wakes the
evaluation (`FactsChanged`); a row boundary wakes it through the deferral's wake time.

**Unknown packages.** A row naming a package that no task of the active catalog uses is not an
error: a cycle evaluated by `evaluate_policy_cycle` reports it once as
`pack_schedule_unknown_package` (info; `package_id`) and the row has no effect.

**Identity.** As for priority offsets, `combined_policy_snapshot_id` hashes the records the
projection consumed, so a record written after an evaluation and before its admission refuses
the intent as `policy_facts_stale` and a just-closed package is never dispatched. Without any
such record the hashed input is exactly the previous one, so identities do not change.

**Capacity and ledger.** The key takes one active fact identity per scope that uses it (the
instance, its server, its game), counted in the store's shared capacity of 256 active facts
(Workflow #383 AO-0 keeps the capacity ledger); when the store is full the write fails as today
and nothing is evicted. A write is an ordinary `fact.published` event of the publishing request;
like every key of this store it is a tagged record that the start loads from the ledger
(Workflow #383).

**Codes.** `pack_schedule_closed` and `pack_schedule_unknown_package` (policy),
`session_pack_schedule_invalid` (runtime-host); none of them triggers the recovery ladder.
