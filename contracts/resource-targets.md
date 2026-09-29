# Instance resource targets

Workflow #308 RT-S1a. An agent states, per instance, how much of a catalog resource it wants
kept (`at least T`) and which tasks produce it. `RuntimeOperation::ApplyResourceTargets` is
the only formal entry: the Runtime parses and checks the document once, stores the checked
policy as the instance fact `session.resource_targets` and answers with the stored version and
what each target currently observes, or refuses the document with a field position and changes
nothing.

**Evaluation is in S1b.** In S1a the evaluator does not read the policy: every decision is the
one it was before, except the input identity (`fact_snapshot_id`), which includes the stored
policy like any other overlaid fact.

## Document `actingcommand.resource-targets.v1`

The raw UTF-8 text is the request's `document_json` (`1..=65536` bytes); the Runtime parses it
with the scheduling declaration parser, so duplicate keys, unknown fields, floats and type
mismatches are refused with a line and column.

```json
{
  "schema_version": "actingcommand.resource-targets.v1",
  "instance": "instance-a",
  "valid_until_unix_ms": 1791000000000,
  "targets": [{
    "id": "credits-floor",
    "resource": "pool.credits",
    "condition": { "kind": "at_least", "amount": 10000 },
    "scale": 10000,
    "importance_milli": 1000,
    "rule": "shortfall_linear",
    "apply": { "mode": "adjust", "weight": "score_stage" },
    "tasks": ["task.collect-income"]
  }]
}
```

| Field | Bound | Rejection reason @ path |
| --- | --- | --- |
| `schema_version` | the v1 constant | `unsupported_schema_version` @ `/schema_version` |
| `instance` | an instance of the authoritative projection | `unknown_instance` @ `/instance` |
| `valid_until_unix_ms` | required when `targets` is non-empty, with `now < v <= now + 31_536_000_000`; absent when `targets` is empty | `missing_field` / `validity_out_of_range` / `invalid_value` @ `/valid_until_unix_ms` |
| `targets` | `0..=16` entries; `[]` withdraws every target | `out_of_range` @ `/targets` |
| `targets[i].id` | scheduling identifier `^[a-z0-9][a-z0-9._:-]*$`, at most 128 bytes, unique in the document | `invalid_value` / `duplicate_id` @ `/targets/i/id` |
| `resource` | a pool id of the active catalog whose `observation` is `fact` with a `fact_key` of the `resource.` or `inventory.` family and whose scope covers the instance | `unknown_resource` / `resource_not_observable` / `resource_out_of_scope` @ `/targets/i/resource` |
| `condition` | `{"kind": "at_least", "amount": 1..=2^53-1}`, the only v1 condition | `invalid_type` / `out_of_range` @ `/targets/i/condition/amount` |
| `scale` | `1..=2^53-1`: the gap at which the target weight equals `importance_milli` | `out_of_range` @ `/targets/i/scale` |
| `importance_milli` | `1..=1_000_000` | `out_of_range` @ `/targets/i/importance_milli` |
| `rule` | `shortfall_linear`, the only v1 rule | `invalid_type` |
| `apply` | `mode` `adjust` or `override`; `weight` `score_stage`, the only v1 weight | `invalid_type` |
| `tasks` | `1..=32` per target, at most 128 in the document, each task once in the document; the task exists, its scope covers the instance, no `instance_overrides` entry of the instance sets `enabled: false`, and one run produces the resource (`r >= 1`, below) | `out_of_range` @ `/targets/i/tasks` (or the 129th reference); `unknown_task` / `task_out_of_scope` / `task_disabled` / `unmapped_task` / `duplicate_task` @ `/targets/i/tasks/j` |

`r` of a task for a resource is `Σ ⌊amount · confidence_milli / 1000⌋` over the task's
`produces` entries of that pool.

General reasons: `invalid_json` (not UTF-8, not JSON, trailing data: the path is empty or the
scanner's), `duplicate_key`, `unknown_field`, `missing_field`, `invalid_type` (a float, a wrong
type, an unknown enum value, an explicit `null`), `out_of_range` @ `""` for a document over 64
KiB, and `catalog_unavailable` @ `""` when no catalog is active. Every rejection carries a line
and a column (at least 1); a JSON-pointer segment holding a control character shows it as
U+FFFD. The reasons form a closed set:

`invalid_json`, `duplicate_key`, `unknown_field`, `missing_field`, `invalid_type`,
`invalid_value`, `out_of_range`, `unsupported_schema_version`, `duplicate_id`,
`unknown_instance`, `validity_out_of_range`, `catalog_unavailable`, `unknown_resource`,
`resource_not_observable`, `resource_out_of_scope`, `unknown_task`, `task_out_of_scope`,
`task_disabled`, `unmapped_task`, `duplicate_task`.

The first failing check is reported, in this order: size, the declaration parse (JSON, keys,
fields and types), schema version, bounds and identifier charset, uniqueness, whether
`valid_until_unix_ms` belongs; then the instance; then the lifetime; then per target in
document order its resource and then its tasks.

The policy identity `policy_sha256` is `sha256:<hex>` of the policy crate's canonical
serialization of the document (sorted keys, no whitespace), so documents that differ only in
key order or whitespace are the same policy.

## Entry and receipts

`RuntimeOperation::ApplyResourceTargets { document_json }` (wire operation
`apply_resource_targets`). Request validation refuses an empty or over-64-KiB document as
`resource_targets_document_invalid` and any origin other than `(agent, adapter)` as
`invalid_agent_dispatcher_origin`; both are Denied + `InvalidRequest` without a host code, as
for every request that fails validation. The request schema stays
`actingcommand.runtime.request.v3`.

A refused document answers Denied + `InvalidRequest` with host code `resource_targets_rejected`
(operation `apply_resource_targets`), no terminal, and the additive receipt field

```json
"resource_targets_rejection": {"field_path": "/targets/0/tasks/0", "line": 12, "column": 16, "reason": "unknown_task"}
```

No event is appended and the active policy stays. Fact store refusals keep their codes and are
Denied (`fact_observation_not_newer`, `fact_store_capacity_exceeded`,
`policy_fact_authority_conflict`, `policy_inputs_unconfigured`,
`policy_instance_metadata_untrusted`, ...); a fatal store or ledger failure poisons the Runtime
as for every publication. Two submissions within the same millisecond may meet
`fact_observation_not_newer`; the caller retries.

An applied (or replayed) policy answers Completed with result
`resource_targets_applied { applied }` and the terminal of the `fact.published` event that
holds the policy:

| Field | Meaning |
| --- | --- |
| `instance_alias`, `policy_sha256` | the instance and the policy identity |
| `version`, `event_id` | the sequence and id of that event (the receipt terminal) |
| `previous_version` | the sequence of the policy active before, or `null`; equals `version` on a replay |
| `replayed` | the equal policy was already active and unexpired; nothing was appended |
| `checked_catalog_hash` | the active catalog the document was checked against |
| `valid_until_unix_ms` | the policy lifetime; `null` exactly for a withdrawal |
| `conditions_at_position`, `conditions` | the ledger position of the projection and, per target in document order, `{target_id, resource, fact_key, state}` |

`state` separates the applied configuration from what the target can compute now:
`{"kind": "computed", "current": c, "observed_at_unix_ms": t, "gap": max(T - c, 0)}` (a gap of
0 is satisfied) or `{"kind": "awaiting_observation", "reason": r}`. The inventory is the most
specific inline record of the pool's `fact_key` the instance can see after the time-validity
projection (a timeline reset drops observations made before it). It is computed when it is a
non-negative integer with a positive confidence (at least the pool's minimum for a
`ledger_fact` pool) and is not expired; otherwise `r` is the first of `missing`,
`low_confidence`, `expired`, `invalid_value` that applies. A pending target is applied and
waits for an observation; it is not an error.

`RuntimeReceipt::validate` binds both shapes: a rejection requires Denied, no terminal, no
result, error `InvalidRequest` and no resource declaration; an applied result requires
Completed and a terminal equal to `{version, event_id}`, and
`ResourceTargetsApplied::validate` checks both hashes, `version >= 1`,
`replayed => previous_version == version`, otherwise `previous_version < version`, at most 16
conditions, and `conditions` empty exactly when `valid_until_unix_ms` is absent.

## Storage: `session.resource_targets`

One instance-scoped fact per instance, published through the ordinary fact publication
(`publish_facts`) and persisted as one `fact.published` event; no event type, payload or
persisted structure is added. The record:

| Field | Value |
| --- | --- |
| `scope` / `key` | `{"kind": "instance", "instance_id": <instance>}` / `session.resource_targets` |
| `content` | inline `record_list` (rows below) |
| `observed_at_unix_ms` | the Runtime clock when the request was processed |
| `expires_at_unix_ms`, `ttl_policy` | `valid_until_unix_ms` with `{1, 31_536_000_000, runtime_default}`; both absent for a withdrawal |
| `confidence_milli` / `source_detector` | `1000` / `runtime.resource-targets` |
| `source_snapshot_id` | `snapshot:resource-targets:<sha256 hex of JSON (scope, key, policy_sha256, observed_at)>` |
| `schema_version` / `resource_bundle_hash` | `fact.v1` / the hex digits of `policy_sha256` |
| `invalidate_on` | `[]` |

The event keeps the request's origin (source `adapter`, actor `agent`, origin module
`fact-store`) and links to the request and its correlation.

Rows, in this order (at most 1 + 16 + 128):

```text
{row: "policy", schema_version, instance, policy_sha256}                  one header
{row: "target", id, resource, at_least, scale, importance_milli,
 rule, mode, weight}                                                        one per target, document order
{row: "task", target, task}                                                 one per task reference, document order
```

`at_least`, `scale` and `importance_milli` are integers, every other field a string. A
withdrawal stores the header alone. The reader is strict: exactly one leading header of a
known `schema_version` naming the fact's instance, known row kinds with exactly their fields
and types, bounded values, unique targets, and task rows that name an earlier target and
reference each task once; anything else is `unsupported_schema_version`, `malformed_rows` or
`instance_mismatch`.

**Replay and withdrawal.** A submission whose `resource_bundle_hash` equals the active,
unexpired policy's appends nothing and answers that policy's version with `replayed: true`,
also after a Runtime restart. A different policy replaces the active one
(`previous_version` names it). `targets: []` withdraws: it stores a header-only policy without
expiry. An expired policy stays stored until replaced; applying the same document again is
refused because its lifetime has passed (`validity_out_of_range`).

**The formal entry only.** `PublishFact` / `PublishFacts` refuse any record of this key as
the non-fatal `resource_targets_formal_entry_required`, before the fact write gate. A
configured policy input fact of this key fails every authoritative projection, so the daemon's
first evaluation fails and the configuration must be corrected; a forward projection caller
supplying it is refused; both as `resource_targets_key_reserved`.

**Identity and budget.** The policy is not excluded from the overlay: it reaches
`EvaluationFacts.facts`, the combined `fact_snapshot_id`, admission staleness
(`policy_facts_stale`), the `FactsChanged` trigger and forward projections like every overlaid
fact. Each instance's policy takes one of the store's 256 active fact identities; each
inventory key it reads takes its own.

## Lock order

The clock is sampled before any lock and the document is parsed without one. The check phase
takes `policy_outcome_gate` → `policy` → `fact_write_gate`, then the fact store, policy inputs
and registered instances inside the authoritative projection (as
`project_policy_input_identity`), computes purely and releases every lock. The store phase is
`publish_facts`: `fact_write_gate` → fact store (synchronize, active revision, preview) →
policy inputs → registered instances → fact store → ledger append → fact store. No lock is held
between the phases and `policy` is never taken under `fact_write_gate`. A catalog activated
between the phases is named by `checked_catalog_hash`.

## Client and CLI

`RuntimeClient::apply_resource_targets(document_json) -> ResourceTargetsApplied` checks only
the length; a refusal's receipt is the error's `received_receipt()`.

```text
actingctl agent-apply-resource-targets --state-root <state-root> --policy-file <policy.json>
```

The CLI reads at most 65536 bytes, requires UTF-8 (`resource_targets_file_invalid` otherwise,
a local fatal) and does not parse the content. It prints `{"applied": ...}`, or
`{"receipt": ...}` with a non-zero exit when the Runtime refuses.
