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

This document freezes the store's role as the source of the policy input
instance set (Workflow #313, item 4).

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

## Backend self-check availability

Workflow #317 slice sc2 (the bounded reading of #316 goal 5): a failed backend
self-check withdraws a configured policy instance's availability. Right after
the host records an open's `backend.selfcheck.<entry>.*` runtime facts
(`runtime-fact-store.md`, "Producers") it compares the recorded `status` with
the instance's active `session.instance.available` record of scope
`instance { instance_id: <alias> }`; the recording, this decision and its
publication hold one host-wide self-check availability gate (taken before the
`fact_write_gate`), so concurrent opens and invalidations cannot interleave:

- `failed` (any entry) publishes `session.instance.available = false` unless
  that record already holds `false`. The record goes through the seed's publish
  path and scope (`publish_fact`: system links, source `runtime`, origin module
  `fact-store`, actor `runtime`, one `fact.published` event) and has the seed's
  form except `source_detector` `runtime.backend-selfcheck` and
  `source_snapshot_id` `backend_selfcheck:<entry>:<generation>`, where
  `<entry>` is `input`, `capture` or `nemu` and `<generation>` the open's
  session generation; `resource_bundle_hash` is the seed digest of the value
  `false`, and the observation time is the host clock.
- `passed` (any entry), when the active record is such a `backend_selfcheck:`
  record and no entry of the instance still holds `failed`, republishes the
  configured seed — the seed's own record (detector, snapshot id and digest of
  the configured value) as a new observation at the host clock. An instance
  configured `available = false` therefore stays `false`.
- `unknown` (an unobserved provider, a fixture simulation, a check the open
  did not make) never changes availability, so an open that observes nothing
  can never withdraw an instance.

When emulator control invalidates the self-check facts after `stop`, `start`
or `restart`, the same restore applies (no entry is `failed` any more); the
next open decides again. An owner takeover invalidates them with the
`backend.` family before seeding, and a clean restart keeps them; in both
cases the seed, whose snapshot id differs from the `backend_selfcheck:`
record, replaces that record as described under "Seeding at startup", and the
next open decides again.

Only a configured policy instance is gated: without policy inputs, or for an
instance that is not a policy instance, nothing is published. The evaluator
is unchanged; it excludes an instance projected `available = false`, so the
policy stops dispatching to the instance and resumes once the seed value
returns. A refusal of the publication (for example `fact_observation_not_newer`
after a wall-clock step back) is returned to the open's observation consumer,
whose existing failure path poisons the Runtime; a fatal refusal marks the
Runtime fatal as every `publish_fact` does, and an instance the registry does
not hold fails `backend_selfcheck_instance_unregistered` (fatal). The strict
reading — an instance unavailable until a self-check passed — needs a
controlled preparation phase (opens happen inside a lease) and is not built.

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
reasons (`instance_unavailable`, …), one `DecisionReason` per missing key with
`code` `policy_instance_fact_missing:<key>` and a detail naming the instance
and the ledger position. No decision on such an instance can be selected, so no
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
and the single reason `priority_offset_unknown_task:<task>`. It is not an error,
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

## Reading the seeds

The per-instance read (`actingctl facts` without `--program`) is not built.
The seeds are visible as `fact.published` events of origin module `fact-store`
and source `runtime` in the ledger, and through `RuntimeHost::instance_fact_snapshot`
for one instance context.
