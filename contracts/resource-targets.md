# Instance resource targets

Workflow #308 RT-S1a/S1b. An agent states, per instance, how much of a catalog resource it
wants kept (`at least T`) and which tasks produce it. `RuntimeOperation::ApplyResourceTargets`
is the only formal entry: the Runtime parses and checks the document once, stores the checked
policy as the instance fact `session.resource_targets` and answers with the stored version and
what each target currently observes, or refuses the document with a field position and changes
nothing.

The evaluator reads the stored policy in every evaluation and turns each open gap into a task
target score in the score stage ("Evaluation" below). An instance without a policy decides
exactly as before; the input identity (`fact_snapshot_id`) includes a stored policy like any
other overlaid fact.

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
    "importance_milli": 100000,
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
and types, bounded values, scheduling identifiers for the target, resource and task ids,
unique targets, and task rows that name an earlier target and reference each task once;
anything else is `unsupported_schema_version`, `malformed_rows` or `instance_mismatch`.

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

## Evaluation

Workflow #308 RT-S1b. Every policy evaluation reads the stored records of this key from
`EvaluationFacts.facts` before the time-validity projection, so a timeline reset never drops a
policy. The evaluator is pure and runs under the caller's locks; nothing here takes a lock.

- An instance-scoped record that the strict reader accepts, with targets and an expiry, is the
  instance's policy: **active** while `now <= valid_until_unix_ms`, **expired** after. Its
  version is `policy_sha256` and `applied_at`, the record's `observed_at_unix_ms` (unique in
  the ledger: a second submission in the same millisecond is refused as not newer); its
  confidence is not read. A withdrawal is no policy. A record that is not a record list, that
  the reader refuses, or that holds targets without an expiry is **unreadable** and the
  instance runs base scheduling.
- A server- or game-scoped record of this key can only come from an ordinary publication
  before RT-S1a. It is never a policy and never fails the evaluation: every candidate of every
  instance it covers carries `resource_target_policy_ignored`, and the instance's own record
  still applies.
- No stored state fails the evaluation (a failed first evaluation keeps the daemon from
  starting): unreadable, expired, ignored, unmapped and pending are reasons. Only a state that
  a bug reaches fails, as `policy_evaluation_input_invalid`, and an arithmetic overflow as
  `policy_evaluation_numeric_overflow`.

Per instance, each target of an active policy is resolved once with the entry's own resolver:
its pool (`unknown_resource`, `resource_not_observable`, `resource_out_of_scope`), then each
named task (`unknown_task`, `task_out_of_scope`, `task_disabled`, or `task_not_producing` for
`r = 0`). The inventory is observed once per target in the projected facts exactly as for the
receipt's `conditions` (`missing`, `low_confidence`, `expired`, `invalid_value`). Each score
stage candidate (a task that passed trigger, feedback stop, cooldown and placement) meets one
effect:

| Effect | When | Rank and disposition | Reason |
| --- | --- | --- | --- |
| none | no policy on the instance, or no target names the task | unchanged | none |
| unreadable | the instance's record cannot be read | unchanged | `resource_target_policy_unreadable`, every candidate of the instance |
| expired | the policy's lifetime has passed | unchanged | `resource_target_policy_expired`, named candidates |
| unmapped | the target's pool, or this task, no longer maps | unchanged | `resource_target_unmapped:<id>` |
| pending | no usable inventory observation | unchanged | `resource_target_pending:<id>` |
| satisfied | `g = 0` | unchanged; an override is released | `resource_target_satisfied:<id>` |
| applied | `g >= 1` | scored as below | `resource_target_applied:<id>`, and `resource_target_override:<id>` in override mode |

A named task that can have no candidate on the instance (unknown, out of scope or disabled) is
listed on every candidate of the instance as `resource_target_tasks_unevaluable`, so a target
never lapses silently.

Score (`shortfall_linear`; exact integers with 128-bit intermediates, truncating):

```text
g   = max(T - c, 0)                          gap of the target (at_least T, inventory c)
w   = min(floor(g*I / S), 1_000_000)         target weight (scale S, importance_milli I)
r_k = sum of floor(amount * confidence_milli / 1000) over task k's produces of the pool
u_k = min(r_k, g)                            useful contribution of one run
U   = max u_j over the target's tasks that still map   (>= 1 while g >= 1)
s_k = min(floor(g*I*u_k / (S*U)), 1_000_000) task target score, "capped" at the bound
```

The task with the largest useful contribution scores `s = w`; the other tasks score at most
`s` (`s_k = min(floor(g*I*u_k / (S*U)), 1e6)`), and at the cap, or when `u_k = U`, they can
tie with it. `s_k` never grows as `c` grows. With `base = score + utility + offset` (see
`contracts/scheduling/README.md`, "Score-Assisted Priority"):

- `adjust`: `effective = base + s`. The disposition follows the thresholds as for any scored
  candidate, so with a selection document `s` takes part in the promotion comparison.
- `override`: `effective = utility + s`. The selection score and the manual offset are
  superseded; utility, priority, aging, strategic weight, urgency and contention stay. The
  disposition is always `none`: never deferred, never promoted.

The thresholds are computed from `base`, never from `s`. `total_score` gains
`effective * 1000`, so `s = 1000` weighs one priority level; the effective score also orders
the host budget allocation and the performance arbitration like any other. Every admission
predicate (authorization, budgets, windows, pause, cooldown, availability, host capacity)
stays in force.

An applied candidate's `facts_fresh_until_unix_ms` is bounded by the inventory observation's
expiry and `valid_until_unix_ms`, so admission refuses its intent as `policy_facts_stale` once
either lapses; the evaluation wakes at each plus one. No other effect changes freshness or
wakes, and none requests detection. The policy and the inventory are part of the combined
`fact_snapshot_id`, so a changed inventory also refuses an already evaluated intent as stale.

Reasons follow `scored` (whose detail ends with ` target=<id>:<mode>:<s>` when applied) and
`score_unknown:*`, and precede the disposition reason: at most one policy-level reason (the
first that applies of ignored, unreadable, expired, tasks_unevaluable), one target-level reason
and one override reason. Codes carry no whitespace; every detail stays within 1024 bytes.

| Code | Detail |
| --- | --- |
| `resource_target_applied:<id>` | `policy=<sha> applied_at=<ms> resource=<pool> fact_key=<k> mode=<m> current=<c> at_least=<T> gap=<g> scale=<S> importance=<I> weight=<w> per_run=<r> useful=<u> best_useful=<U> task_target=<s>[ capped]` |
| `resource_target_override:<id>` | `superseded score=<s\|none> offset=<o>; utility kept` |
| `resource_target_satisfied:<id>` | `policy=<sha> applied_at=<ms> current=<c> at_least=<T> gap=0 mode=<m>`, plus `; override released` in override mode |
| `resource_target_pending:<id>` | `fact_key=<k> reason=<missing\|expired\|low_confidence\|invalid_value>; configuration applied, waiting for a valid observation` |
| `resource_target_unmapped:<id>` | `resource=<pool> task=<task> why=<unknown_resource\|resource_not_observable\|resource_out_of_scope\|task_not_producing>` |
| `resource_target_policy_expired` | `policy=<sha> applied_at=<ms> valid_until=<ms>` |
| `resource_target_policy_unreadable` | `stored policy cannot be read (<code>); instance runs base scheduling` |
| `resource_target_tasks_unevaluable` | `<target>/<task>:<why>,...` in target and task order, at most 1010 bytes of items, the rest as `+<n>more` |
| `resource_target_policy_ignored` | `a <server\|game> scoped session.resource_targets record (observed_at=<ms>) was not written by the formal entry and is ignored` |

The reasons travel in the reason chain the dispatch events already carry; no event, payload
field or persisted structure is added.

## Choosing scale and importance

On one instance, let H be the candidate that ranks first without the target and T a named
candidate, neither promoted, in `adjust` mode. In effective milli (one priority level = 1000,
one second of aging = 1, strategic weight, urgency, offset, score and utility 1:1, a load
cost `cost * bp / 10`), T wins exactly when `s > Δ` with

```text
Δ = 1000*(p_H - p_T) + (aging_H - aging_T)/1000 + (w_H - w_T) + (u_H - u_T)
    - (contention_H - contention_T)/1000 + (effective_H - base_T)
```

At `s = Δ` affinity and then the deterministic tie breaker decide. The target's best task
scores `s = min(floor(g*I/S), 1e6)`, so it overtakes from the gap `g* = ceil((Δ+1)*S/I)`
(integer Δ); with `S = T` it leads while `c <= T - g*`. The other tasks of the target score at
most `s` (`s_k = min(floor(g*I*u_k / (S*U)), 1e6)`); at the cap, or when `u_k = U`, they can
tie. Each task belongs to one target.

1. Estimate Δ: `1000 * priority difference + seconds the competitor may have waited longer +
   strategic difference + load cost difference * bp / 10 + utility difference` (the last only
   when the catalog declares `value_milli`).
2. Choose S: the target weight reaches I at a gap of S. `S = T` by default.
3. Choose I: to overtake from a gap of `g_want`, `I >= ceil((Δ+1)*S / g_want)`; to overtake only
   at a full gap S, `I > Δ`.
4. I also bounds how long the target holds a competitor off: every second the competitor waits
   adds 1 to its aging, so at a full gap aging overtakes after about `I - Δ` seconds (I = 100000,
   Δ = 5000: about 26.4 hours; I = 1e6: about 11.5 days). This is the existing starvation
   guard and stays.
5. `s <= 1_000_000`, 1000 priority levels, the bound of a manual offset. A Δ of 1e6 or more (a
   priority difference of 1000 levels, or heavy against light at bp 10000 plus 400 levels) is
   not crossed: treat it as a hard tier (explicit tiers are a later slice, S3).
6. Promotion is a strict tier. An override never promotes and cannot beat a promoted
   competitor: use `adjust` there. In `adjust` mode with a selection document a large `s` can
   promote the candidate ahead of every unpromoted one regardless of priority. The Runtime's
   catalog source runs no selection document today.
7. An operator can always counter: in `adjust` mode an offset on the named task (up to ±1e6)
   cancels `s`; in `override` mode a positive offset on the competitor, a withdrawal
   (`targets: []`) or pausing dispatch.

Worked example: `T = S = 10000`, `I = 100000`, one named task with `r = 1000` (so `s = 10 * g`)
against a light competitor five priority levels higher with equal strategic weight and no
urgency or value (`Δ = 5000` plus the aging difference in seconds):

| c | g | s | Winner at equal aging |
| --- | --- | --- | --- |
| 0 | 10000 | 100000 | named task |
| 9000 | 1000 | 10000 | named task |
| 9499 | 501 | 5010 | named task |
| 9500 | 500 | 5000 | tie, decided by the tie breaker |
| 9501 | 499 | 4990 | competitor |
| 9800 | 200 | 2000 | competitor |
| 10000 | 0 | satisfied | competitor |

The flip gap is 501 (`c <= 9499`) at equal aging, 507 (`c <= 9493`) when the competitor waited
60 s longer and 861 (`c <= 9139`) after one hour. The example document's `importance_milli`
of 100000 is sized this way; 1000 would weigh like a single priority level.

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
