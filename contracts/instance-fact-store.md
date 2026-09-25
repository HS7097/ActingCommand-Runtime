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

## Reading the seeds

The per-instance read (`actingctl facts` without `--program`) is not built.
The seeds are visible as `fact.published` events of origin module `fact-store`
and source `runtime` in the ledger, and through `RuntimeHost::instance_fact_snapshot`
for one instance context.
