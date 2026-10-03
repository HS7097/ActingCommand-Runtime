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

Two document versions exist. `actingcommand.resource-targets.v1` (Workflow #308 RT-S1) is
frozen: its fields, rejections, rows, identity and evaluation stay exactly as specified here,
and its override still supersedes the manual offset. `actingcommand.resource-targets.v2`
(Workflow #335 S2b) weighs every produced resource of the instance with the pool's declared
`valuation` (`contracts/scheduling/README.md`, "Pool Valuation") plus its target's shortfall
("Document `actingcommand.resource-targets.v2`" and "Resource weights (v2)" below). Agents
submit v2; a v1 document keeps its v1 meaning.

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
| `tasks` | `1..=32` per target, at most 128 in the document, each task once in the document; the task exists, its scope covers the instance, no `instance_overrides` entry of the instance sets `enabled: false`, and one run produces the resource (`r > 0`, below) | `out_of_range` @ `/targets/i/tasks` (or the 129th reference); `unknown_task` / `task_out_of_scope` / `task_disabled` / `unmapped_task` / `duplicate_task` @ `/targets/i/tasks/j` |

`r` of a task for a resource is the sum of its `produces` entries of that pool.
An integer `amount` contributes `floor(amount * confidence_milli / 1000)` real units.
A scheduling.v2 `expected_amount_milli` contributes exactly that many thousandths of a
real unit, without confidence discounting. Mapping and coverage require `r > 0`;
positive subunit expectations therefore map in either resource-target document version.
Both forms sum in milli-units using bounded u128 arithmetic; inventory and target values
stay in real units. A valid zero yield is unmapped; an absent/invalid quantity fails catalog
compilation. Expected quantities never become observed inventory facts.

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

## Document `actingcommand.resource-targets.v2`

Workflow #335 S2b. The same entry, parser, size bound and general reasons as v1; the closed
reason set is unchanged.

```json
{
  "schema_version": "actingcommand.resource-targets.v2",
  "instance": "instance-a",
  "valid_until_unix_ms": 1791000000000,
  "targets": [
    { "id": "credits-floor", "resource": "pool.credits",
      "condition": { "kind": "at_least", "amount": 100000 },
      "apply": { "mode": "adjust", "weight": "score_stage" } },
    { "id": "energy-push", "resource": "pool.energy",
      "condition": { "kind": "at_least", "amount": 200 }, "scale": 10, "importance_milli": 500,
      "apply": { "mode": "override", "weight": "score_stage", "manual_offset": "supersede" },
      "tasks": ["task.collect-income"] }
  ]
}
```

| Field | v2 rule | Rejection reason @ path |
| --- | --- | --- |
| `instance`, `valid_until_unix_ms`, `targets` (`0..=16`, `[]` withdraws), `id`, `condition` | as v1 | as v1 |
| `resource` | the v1 pool checks; at most one target per resource in the document | as v1; `duplicate_id` @ `/targets/j/resource` |
| `scale` | optional, `1..=2^53-1`: the gap step `S`; absent takes the pool's `valuation.scale`, so it is required when the pool declares no valuation | `out_of_range` / `missing_field` @ `/targets/i/scale` |
| `importance_milli` | optional, `1..=1_000_000`: `I`; absent takes the pool's `valuation.gap.weight_milli`, so it is required when the pool declares no `gap` | `out_of_range` / `missing_field` @ `/targets/i/importance_milli` |
| `rule` | optional, `shortfall_linear` only; absent takes the pool's `valuation.gap.rule`, and `shortfall_linear` without a gap block | `invalid_type` |
| `apply.mode`, `apply.weight` | as v1 | as v1 |
| `apply.manual_offset` | optional, `keep` or `supersede`, only in `override` mode; absent is `keep` | `invalid_value` @ `/targets/i/apply/manual_offset` in `adjust` mode; `invalid_type` for another value |
| `tasks` | optional in `adjust` mode, required in `override` mode; when given, the v1 task checks; when absent, at least one task of the instance (scope covers it, not disabled) must produce the resource (`r > 0`) | `missing_field` @ `/targets/i/tasks`; as v1 @ `/targets/i/tasks/j`; `unmapped_task` @ `/targets/i/resource` |

**Version probe.** The Runtime first reads the document as plain JSON: exactly when its
top-level `schema_version` is the string `actingcommand.resource-targets.v2` it takes the v2
checks; anything else, including a document the probe cannot read, takes the v1 path, so every
non-v2 document meets the v1 rejections unchanged. The probe itself refuses nothing; UTF-8,
JSON and duplicate-key refusals come from the shared declaration parser in both paths.
`validate_catalog_declaration` with kind `resource_targets` dispatches by the same probe.

The first failing check is reported, in this order: size, the declaration parse, schema
version, bounds and identifier charset, uniqueness (target ids, tasks, resources), the apply
rules (`manual_offset` in `adjust` mode, `override` without `tasks`), whether
`valid_until_unix_ms` belongs; then the instance; then the lifetime; then per target in
document order its resource, whether `S` and `I` resolve, and its tasks.

A left-out `scale`, `importance_milli` or `rule` is not fixed at submission: every evaluation
resolves it from the active catalog again ("Resource weights (v2)"). The identity is the
canonical serialization of the v2 document as for v1 (a left-out field is absent from it), so a
v1 and a v2 policy never share an identity; replacing a v1 policy with a v2 document is a new
version. Receipts keep their shape; neither the base nor the effective weights appear in them.

A Runtime build before this revision refuses a v2 document at the typed parse, usually as
`missing_field` or `unknown_field` rather than `unsupported_schema_version`, and reads a stored
v2 policy as unreadable (`unsupported_schema_version`), running that instance on base
scheduling.

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

The observation can come from an agent's `PublishFact` or, automatically, from a package's
resource reading (Workflow #335 S5b, `contracts/resource-readings.md`). A package that declares
a reading of the pool's `fact_key` writes that record after a successful run on the reading
page, with the declared lifetime. The target then turns from `awaiting_observation` to
`computed`, and back to `expired` once the lifetime has passed. The Runtime does not schedule
readings; the agent decides when to run the reading task again.

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

A v2 policy stores the same three row kinds (at most 10 fields a row, 145 rows):

```text
{row: "policy", schema_version: "actingcommand.resource-targets.v2", instance, policy_sha256}
{row: "target", id, resource, at_least, mode, weight
 [, scale][, importance_milli][, rule][, manual_offset]}                   one per target, document order
{row: "task", target, task}                                                 one per task reference, document order
```

A target row carries an optional field exactly when the document states it. A target without
`task` rows has no `tasks` (it covers every producing candidate); an `override` target has at
least one. A header of the v2 version selects the v2 reader, as strict as v1's: the required
fields exactly, optional fields only from the list above, `manual_offset` only with
`mode: override`, unique target ids and resources, and the v1 task-row rules. Every other
record list, of any header, meets the v1 reader unchanged.

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
r_k = sum of effective production in real units, retaining milli-unit fractions (defined above)
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
field or persisted structure is added. A dispatched candidate's target contribution is also
read in its dispatch decision record (Workflow #308 RT-S1c; `contracts/scheduling/README.md`,
"Dispatch Decision Record"): `rank_breakdown` (`target`, `target_id`, `mode`, `superseded`)
and `decision_record` (`targets=active:<sha>@<applied_at>`, `expired:…`, `unreadable:<code>`
or `none`, plus ` ignored=…`).

### Resource weights (v2)

Workflow #335 S2b. Everything above applies to a v1 policy only; a v2 policy is read, stored and
reported as below, and the instance-level rules (unreadable, ignored, no stored state fails the
evaluation) are shared.

**Enabled instance.** An instance is enabled exactly while its stored record decodes as v2,
holds targets and `now <= valid_until_unix_ms`. Without a policy, after a withdrawal, after
expiry, with an unreadable record or with a v1 policy no base weight counts: a v1 policy takes
the path above and never reads `valuation`.

**Per evaluation.** Each target is resolved once for the instance against the active catalog:
its pool (as v1), then `S = scale ?? valuation.scale` and `I = importance_milli ??
valuation.gap.weight_milli` (the rule is `shortfall_linear`, the only one), then its named
tasks (as v1), then its inventory in the time-validity projected facts (as v1). A left-out term
whose pool no longer declares it (a catalog update removed `valuation` or `gap`) makes the
target *unresolved*: it contributes no shortfall weight and says so, never silently zero.

A target without `tasks` *lapses* when the active catalog no longer supports it on the
instance: its pool no longer resolves (`unknown_resource`, `resource_not_observable`,
`resource_out_of_scope`), or no task whose scope covers the instance and that no instance
override disables produces the pool (`no_producing_task`). A lapsed target weighs nothing and
is listed as `<id>@<pool>:<why>` in `resource_target_tasks_unevaluable` on every candidate of
the instance, beside the v1-style `<target>/<task>:<why>` items of named tasks that can have no
candidate. Only these catalog-level failures are listed: a producing task that is merely held
back this round (trigger, feedback stop, cooldown, placement, budget) is not a lapse. A target
with `tasks` whose pool no longer resolves shows `resource_target_unmapped:<id>` on its named
candidates, as v1.

For an enabled instance `i` and a candidate `k` (a task that passed trigger, feedback stop,
cooldown and placement), with milli-unit quantities and checked 128-bit intermediates:

```text
for each pool p whose scope covers i:
  r(k,p) = sum of effective production in real units, retaining milli-unit fractions
  B_p    = p.valuation.base_weight_milli, 0 without a valuation
target t (at most one per pool) of pool p:
  scope(t) = t.tasks; without tasks every candidate of i with r(k,p) > 0
  with a known inventory c and g = max(T - c, 0) >= 1:
      Γ_t = min(floor(I * g / S), 1_000_000)          ("capped" at the bound)
  otherwise (satisfied, pending, unresolved): Γ_t = 0
W(k,p) = B_p + (k in scope(t) ? Γ_t : 0)             effective resource weight, <= 2_000_000
Q_p    = p.valuation.scale, else t.scale
T(k,p) = floor(r(k,p) * W(k,p) / Q_p)
F(k,p) = fractional remainder of r(k,p) * W(k,p) / Q_p for pools with an explicit expected effect; 0 otherwise
R(k)   = min(sum of T(k,p) + floor(sum of F(k,p)), 1_000_000) ("capped" at the bound)
```

A pool takes part for `k` when `r(k,p) > 0` and it declares a valuation or a target covering
`k` names it. `W` belongs to the resource; a target's scope only decides who gets the
shortfall part. Only `produces` count: `consumes` still drives urgency alone. `Γ`, `W`, `T` and
`R` never fall as the gap grows; at `g = 0` the base weight stays.

Integer-only resources keep their established per-resource floor. Resources with explicit
expected quantities aggregate all their effects before weighting and retain exact rational
score remainders until the final task-score floor. For example, 400 + 400 milli-units of one
resource and 400 of another, each weighted 1 per real unit, contribute floor(0.8 + 0.4) = 1.
`resource_weights` reports each per-resource integer term and the final aggregate term;
its `r` quantity is in real units with up to three decimal places. Rational intermediates
are reduced before addition; an unrepresentable u128 fraction or target-score intermediate
returns an explicit evaluation error, not zero, an unmapped task, or a saturated success.
The existing final score cap remains 1,000,000.

**Stage and score.** On an enabled instance a candidate with `R > 0`, or covered by an
effective override (an `override` target naming it with `g >= 1`, step and importance
resolved), enters the score stage even without a selection document, value or offset; every
other candidate is released untouched and only gains reasons. With `base = score + utility +
offset`:

- no effective override: `effective = base + R`;
- an effective override: `effective = utility + R`, plus `offset` when `manual_offset` is
  `keep` (the default); the selection score is superseded and the disposition is `none`. At
  `g = 0` the override is released and the first formula applies.

The thresholds are computed from `base` only and `total_score` gains `effective * 1000`, as
for v1. Every predicate gate, budget, window, pause, eligibility and admission check stays. The
Runtime's catalog source runs no selection document, so an override that keeps the offset
ranks like `adjust`; they differ in disposition and reasons only.

**Other instances.** Their scores, relative order, thresholds and reasons are unchanged. The
shared host budget is still allocated in global total order, so while it is short an enabled
instance's candidates can take it first and another instance's candidate then records
`host_budget_deferred` for the round, as with a v1 target.

**Freshness.** A candidate that `R > 0` or an effective override moves is fresh at most until
`valid_until_unix_ms` and the expiry of every inventory observation behind a positive
shortfall weight (or the effective override) covering it; admission refuses its intent as
`policy_facts_stale` once one lapses and the evaluation wakes at each plus one. Forward
projections run the same evaluator and include the resource term.

**Reasons.** On an enabled instance every candidate gains, after `scored` and
`score_unknown:*` and before the disposition reason, at most five reasons:

| Code | Detail |
| --- | --- |
| policy level (as v1, at most one) | `resource_target_policy_ignored`, `resource_target_policy_unreadable` or `resource_target_tasks_unevaluable` (items in target order, `<target>/<task>:<why>` for a named task as v1 and `<id>@<pool>:<unknown_resource\|resource_not_observable\|resource_out_of_scope\|no_producing_task>` for a lapsed target without `tasks`, joined by `,`, at most 1010 bytes, the rest as `+<n>more`), on every candidate of the instance; after expiry `resource_target_policy_expired` on every candidate the policy would weigh |
| `resource_targets` | the targets covering the candidate in document order, joined by `; `, each `<id>@<pool>:applied current=<c> at_least=<T> gap=<g> step=<S> importance=<I> gap_weight=<Γ>[ capped]`, `<id>@<pool>:satisfied current=<c> at_least=<T> gap=0 gap_weight=0`, `<id>@<pool>:pending reason=<missing\|expired\|low_confidence\|invalid_value> gap_weight=pending` or `<id>@<pool>:unresolved why=<valuation_missing\|gap_missing> gap_weight=0` |
| `resource_target_unmapped:<id>` | as v1, for a task a target names explicitly |
| `resource_target_override:<id>` | `superseded score=<s\|none>; offset=<o> kept (manual_offset=keep); utility kept` or `superseded score=<s\|none> offset=<o> (manual_offset=supersede); utility kept` |
| `resource_weights` | `policy=<sha>@<applied_at> term=<R>[ capped] items=<pool>:r=<r>,per=<Q>,base=<B>,gap=<Γ>,effective=<W>,term=<T>;…`, items by `T` descending then pool id; only for a candidate with at least one item |

List details are counted while joining and stay within 1010 bytes; items that do not fit fold
into `+<n>more`. On an enabled instance `scored` ends with ` resources=<R>` and, under an
effective override, ` override=<id>:<keep|supersede>`; the dispatch decision record shows
`target=<R>` in `rank_breakdown` and `candidate_not_selected`, `target_id=<override id|*>`,
`mode=<override|adjust>`, `superseded=1` under an effective override, and ends `rank_breakdown`
with ` offset_kept=<0|1>`; `decision_record` names the policy `active.v2:<sha>@<applied_at>`
or `expired.v2:<sha>@<applied_at>`. A v1 instance and an instance without a policy keep every
reason byte for byte.

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
   not crossed.
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

## Choosing valuation and target values (v2)

1. `Q` is the resource's natural step; `B` is the standing value of one step (1000 milli = one
   priority level), so `B/Q` is the exchange rate between resources and adding their terms is
   meaningful.
2. Δ is defined as above; when the competitor has a resource term too, add its `R` to Δ.
3. For a task producing `r` per run to overtake Δ from a gap `g_want`, choose
   `G >= ceil((ceil((Δ+1)*Q/r) - B) * S / g_want)`.
4. `R <= 1_000_000`. A waiting competitor gains 1 of aging per second, so at the full term it
   overtakes after about `R - Δ` seconds.
5. The base weight is a standing preference on every producing task of an enabled instance and
   also orders the shared host budget, so keep `B` small.

Worked example: `Q = S = 1000`, `B = 100`, `G = 100`, `T = 100000`, one task with `r = 1000`
against a light competitor five priority levels higher (`Δ = 5000` plus the aging difference in
seconds):

| c | g | Γ | W | R | Winner at equal aging |
| --- | --- | --- | --- | --- | --- |
| 0 | 100000 | 10000 | 10100 | 10100 | producing task |
| 50990 | 49010 | 4901 | 5001 | 5001 | producing task |
| 51000 | 49000 | 4900 | 5000 | 5000 | tie: affinity, then the tie breaker |
| 51001 | 48999 | 4899 | 4999 | 4999 | competitor |
| 60000 | 40000 | 4000 | 4100 | 4100 | competitor |
| 100000 | 0 | 0 | 100 | 100 | competitor (the base weight stays) |

With `Q = S = 10000`, `B = 20`, `G = 10` and `T = 5000000`, a task producing `r = 300000` scores
`T = 5010` at `c = 4853000` and leads Δ = 5000, and `4980` one unit later; item 3 gives
`G >= 10` for `g_want = 147000`. Another resource with `Q = 10`, `B = 500` and no target adds
`floor(20 * 500 / 10) = 1000` to a task producing 20 of it, whatever the first resource's gap.

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
