# Instance resource targets

Workflow #308 RT-S1a/S1b. An agent states, per instance, how much of a catalog resource it
wants kept (`at least T`) and which tasks produce it. `RuntimeOperation::ApplyResourceTargets`
is the only formal entry: the Runtime parses and checks the document once, stores the checked
policy as the instance fact `session.resource_targets` and answers with the stored version and
what each target currently observes, or refuses the document with a field position and changes
nothing.

The evaluator reads the stored policy in every evaluation and turns each open gap into a task
target score in the score stage ("Evaluation" below). On an instance without a policy the
pools' declared `valuation` alone weighs every producing task ("Base valuation on every
instance" below); with no valued pool it decides exactly as before. The input identity
(`fact_snapshot_id`) includes a stored policy like any other overlaid fact.

## Base valuation on every instance

SS-1 (0.12.30; Alice's rulings 100 and 102: with no player goal the scheduler runs the modelled
baseline optimum, a goal only adds its shortfall, and a met goal only stops adding it). A pool's
`valuation.base_weight_milli` counts for every producing task on every instance, whatever the
instance's stored record: none, withdrawn (`targets: []`), expired, unreadable, a v1 policy or an
active v2 policy. A goal only adds its shortfall term `Γ` on top; a met, pending or unresolved
goal adds nothing and removes nothing. Without an active v2 policy a candidate that produces at
least one valued pool covering its instance carries the base term `R` of "Resource weights (v2)"
computed with no target (`W = B`), with its `resource_weights` reason (`policy=none`), its
` resources=<R>` in `score_computed` and, unless a v1 target applies to it, its resource fields
in the dispatch decision record; it enters the score stage when `R > 0`. A base term bounds no
freshness and wakes nothing: the valuation is part of the catalog. Under a v1 policy the v1
effect below is computed as before and `R` is added to its effective score (`adjust`:
`base + s + R`; `override`: `utility + s + R`); its decision record keeps the v1 fields
(`target=<s>`). A candidate that produces no valued pool, and every candidate of a catalog
without a valuation, keeps every reason and record byte for byte, except the v1 `U` of the
one-way guard below (a v1 policy one of whose named tasks an instance override disables after
it was stored).

**One-way guard.** Resource weight feeds task weight, never the reverse (ruling 100). The
resource layer, the pool weights `W = B + Γ` and each task's term `R`, reads the pools'
valuation, the instance's inventory readings, the stored targets and each task's own `produces`
only (Workflow #455 G-R2s, 0.12.60 to 0.12.76, also read each task's composer mark
`irreversible_reach`; G-R2x, 0.12.77, withdrew it: "A task that can do something irreversible"
below): never a task's enablement, an instance override, a priority, a manual offset or a run
state. Changing any of them for one task changes no pool weight and no
other task's term. A
target that names a task an instance override disables keeps covering it (the task has no
candidate, and the disablement is only reported in `resource_target_tasks_unevaluable`); a v2
target without `tasks` that no enabled task produces still resolves and is only reported as
lapsed; and a v1 target's best useful contribution `U` includes the named tasks an instance
override disables ("Evaluation" below).

Two document versions exist. `actingcommand.resource-targets.v1` (Workflow #308 RT-S1) is
frozen: its fields, rejections, rows and identity stay exactly as specified here, and its
evaluation too, except that since SS-1 (0.12.30) its `U` includes the named tasks an instance
override disables ("One-way guard" above, "Evaluation" below), and that from Workflow #455 G-R2s
(0.12.60) until its withdrawal by G-R2x (0.12.77) a candidate whose task carried the
irreversible mark met no effect of it ("A task that can do something irreversible" below); its
override still supersedes the manual offset. `actingcommand.resource-targets.v2` (Workflow #335 S2b) weighs every
produced resource of the instance with the pool's declared `valuation`
(`contracts/scheduling/README.md`, "Pool Valuation") plus its target's shortfall ("Document
`actingcommand.resource-targets.v2`" and "Resource weights (v2)" below). Agents submit v2; a v1
document keeps its v1 meaning (its `U` aside, SS-1). Since SS-1 (0.12.30) the valuation weighs
every instance whatever its record ("Base valuation on every instance" above). Since Workflow
#455 G-R2a (0.12.60) a third version, `actingcommand.resource-targets.v3`, is the goal
document's value store: v2 targets, each with a lifetime of its own, and the values of named
entries, in the same record ("Document `actingcommand.resource-targets.v3`" below); the
evaluator reads its targets as a v2 policy's.

## A task that can do something irreversible

Workflow #455 G-R2x (0.12.77; register §1.499; the coordinator's withdrawal of 11-21 on board
#455, on the owner's decisions 79 of 10-28, that an account handed to the Runtime's scheduling is
the Runtime's to develop, and 234 of 11-18, that the scheduler may spend premium currency and
tickets). **A goal raises the weight of every task that produces its pool, a package that can do
something irreversible included.** Workflow #455 G-R2s (0.12.60, register §1.369) had built the
opposite line, a goal never raising such a task: the standard-pack composer marked it
`irreversible_reach: true` and the evaluator computed a marked candidate on an instance holding a
target record as on an instance without one, with the reason
`resource_target_package_irreversible`. That rule is withdrawn:

- **The evaluator weighs every candidate alike.** A candidate whose task carries the mark is
  computed with its instance's target record as any other candidate is, for every version of the
  record (v1, v2 or v3; active, expired or unreadable): the score stage's target term (a v1
  record's effect, override and wakes on a task it names; a v2 or v3 record's term `R`), the net
  benefit, its dispatch's values (`gap_remaining` and the goal term of `run_cap`) and the record's
  policy-level reasons. `resource_target_package_irreversible` is retired (0.12.77); no candidate
  carries it.
- **The composer writes no mark** ([standard-pack-schedule.md](standard-pack-schedule.md), "The
  irreversible mark"). The member stays readable in a V2 catalog, without effect, because a catalog
  generation of 0.12.60 to 0.12.76 carries it ([scheduling/v2/README.md](scheduling/v2/README.md)).
- **A candidate whose task never carried the mark keeps its own numbers.** The record's own
  resolution, which every such candidate read under G-R2s and reads now, is the one G-R2s left as it
  was; only the candidates of tasks that carried the mark read it now instead of the resolution
  without the record. So such a candidate keeps its own score-stage numbers and its net benefit.
  What can change on its instance is the competition and every record that lists competitors: the
  winner's lead (`rank_breakdown`'s `lead`), the decision record's rows of the candidates passed
  over (`decision_candidate_not_selected`, their `total` and `behind_by`), the order of a capped
  round by value per unit ([scheduling/README.md](scheduling/README.md), "Capped spending order"),
  the reserve gate ("Task Reserves" there) and the heavy-scene slots ("Shared Run Limits" there),
  because a candidate that carried the mark can now be raised by a goal. That is the purpose of the
  withdrawal. On an instance without a target record nothing changes.
- **Goals and variables govern such packages too** since Workflow #455 G-SLx (0.12.78, register
  §1.501, the owner's decision): the goal check's refusal of an entry that governs one
  (`goal_governs_irreversible`, [standard-pack-goals.md](standard-pack-goals.md), "What is not this
  file's"), kernel admission's refusal of `variables` on one (`variables_on_irreversible`),
  `standard-pack check`'s `variable_irreversible_by_root` and the judgement that such a variable
  does not apply ([package-variables.md](package-variables.md)), which stayed here until 0.12.77,
  are lifted. A value set on an application for a variable of such a package is that package's
  run's like any other. Real money stays out by the packages themselves: no package holds a press
  that leads to a platform payment. The capability's `irreversible` mark stays in its declaration
  (the remedy layer reads it), and the contract crate's `IrreversiblePackages` stays for the
  composer's warning of a package that cannot be read and the standard-pack trial's mark.

## A game whose root cannot be composed

**A game whose root cannot be composed has no goals in effect until it composes** (Workflow #455
G-R2s round 2, 0.12.60, the coordinator's ruling of 11-14; under the heading of the irreversible
task until 0.12.77, standing on its own since Workflow #455 G-R2x, register §1.499, which withdrew
the mark and left this rule exactly as it was). While the Runtime's scheduler workbench cannot
compose a game's root as it is now (the root's metadata cannot be read, the root is damaged, the
composition is refused as a whole, or a refused composition cannot be narrowed to a game it
composed), the active catalog generation may not hold that game as its files give it now; with no
composition of its tasks there is nothing to weigh by its goals. So every candidate of that game's
instances, on an instance that holds a target record, is computed exactly as on an instance
without a target record: its instance is resolved once more as without the record (a server- or game-scoped record
it ignores stays ignored), and the candidate reads that resolution for everything the record would
give it (the score stage's target term, the net benefit on base weights, no `gap_remaining` and no
goal term in `run_cap`, none of the record's policy-level reasons), and carries
`resource_target_root_uncomposed` (info) after the reasons of that resolution and before the base
term's `resource_weights`. The record's own resolution is left as it is. The workbench voids games
at a check that keeps the active generation, by these rules (fix rounds 3 and 4, the coordinator's
rulings of 11-14):

- **A game whose own root fails** (its metadata cannot be read, or it is damaged) is voided. The
  check still marks, reads and composes every other game, and does not activate that composition:
  the failed game keeps the active generation's rules.
- **While a game's own root fails, a game the active generation holds otherwise is voided too**:
  a game of which the active generation holds a task that the present composition of its files
  does not compose with the same id and procedure. (Until 0.12.77 also a task that composition
  marked irreversible and the active generation did not; that comparison went with the mark.) The
  check records each game voided this way: one `resource_target_root_uncomposed` (info) naming it
  (`game`) at each check that voids it; a game whose own root fails is named by that root's own
  record. Every other game keeps its goals, so one failing root voids only its own game, at the
  first check after a start too.
- **When the composition is refused as a whole** (or no refused game can be taken out), no
  composition of the others exists: every game in use whose files the check did not find unchanged
  since its last composed read is voided, with the failed games (a game whose files have not
  changed is composed in the active generation as it is now). The workbench holds no file marks at
  the first check after a start, so there such a refusal voids every game in use.

Every void ends at the first composition of every game that succeeds; until then each check reads
and composes again. The Runtime tells the evaluator per instance, in memory only: never a fact or
a snapshot. The void is part of the decision identity of each candidate of a voided instance that
holds a target record, the only candidates it changes (the identity hashed once more with it): a
root that heals appends nothing to the ledger, and two evaluations at one instant, ledger position
and snapshot must not give two decisions one identity. Every other candidate's identity is
unchanged, a voided instance's without a record included. It also covers a current generation
composed before 0.12.60, whose first check under this build meets a root it cannot compose. An
instance of a game whose root composes, and that the active generation holds as its files give it,
is not touched, and an installation whose roots all compose computes exactly what it computes
without this. **Known limit:** the forward projection does not read the void: while a game is
voided, its steps still weigh that game's candidates with their goals (it would need the voided
instances as an input, and its answer has no member for a candidate's reason); a later slice
changes it.

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
| `resource` | a pool of the active catalog, named as the game root's files write its id (#503 GOAL-1, since 0.12.88: "A target's pool" below), whose `observation` is `fact` with a `fact_key` of the `resource.` or `inventory.` family, or (#455 P2, since 0.12.79) `list_rows` (its list's key under `inventory.list.`; "A pool read from rows of a list" below), and whose scope covers the instance | `unknown_resource` / `resource_not_observable` / `resource_out_of_scope` @ `/targets/i/resource` |
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

### A target's pool

#503 GOAL-1 (since 0.12.88; register §1.531). A target names its pool as the game root's files
write it: its `resource` is the pool's id in the instance's `schedule.json` (the id a `goals.json`
resource entry's `governs.pool` names), in catalog mode the catalog's id, a scheduling identifier;
so in a document of every version, in an `EditGoal` value and in the stored record. The
composition gives a pool it copies to two or more instances the id `<id>@<alias>` in each copy
([standard-pack-schedule.md](standard-pack-schedule.md), composition step 4, "Ids"): that suffix
comes of how many instances the installation has, is not what a person said, and is never stored.

For one instance, the pool a target's `resource` names is the catalog pool of that id whose scope
covers the instance, else the instance's copy `<id>@<alias>`, else the first catalog pool the files
name so (which is then refused as every pool is judged: `resource_not_observable` where its
observation is none a target reads, else, its scope not covering the instance,
`resource_out_of_scope`); a text that is no scheduling identifier, a copy's id among them, names no
pool (`unknown_resource`). It is the rule the lines
given for one instance read a pool by ([scheduling/README.md](scheduling/README.md), "Lines for one
instance"). One function of the policy crate gives that pool, and every reader of a target's
`resource` takes it from there: a document's check, the targetable lists below, the conditions of
the view and of a receipt, the goal page's check and capacity ("One value by name"), and every
evaluation that applies a target, scores its tasks, or names the tasks an expired or lapsed target
would have weighed. A record therefore holds the same bytes on a game with one instance and on a
game with two, and a record written while the game had one instance names the same pool after a
second is added, and the other way round; the record's format does not change.

A copy's id is no scheduling identifier, so no record can hold one, and a document that names one
is refused `unknown_resource` @ `/targets/i/resource`. Until 0.12.88 a v2 or v3 document that named
the instance's own copy was taken and left a record its reader refuses (`malformed_rows`), and one
that named another instance's copy was `resource_out_of_scope`. Where such a record stands, the
instance's evaluation weighs no target, both views and the goal page refuse it, and so does every
`EditGoal` of the instance, a clear by name among them; a new document through
`ApplyResourceTargets`, a withdrawal among them, replaces it. A target's `tasks` name the
catalog's task ids, as before; a copy's id there is no identifier either and is refused
`unknown_task` @ `/targets/i/tasks/j` (naming a rule as the file writes it is not built, register
§1.531). A condition (of the view's `active`, of a receipt) names its pool as its target does, the
file's id: a composed copy's id is in no answer of the two views or of the two requests' receipts
(fix round 1). What an evaluation says of a candidate (its `resource_targets` items, a pool line's
values) names the catalog pool, as before: on a game with two instances that is the instance's copy.
On a game with one instance every stored byte and every evaluation is as before, and so is every
answer but two: an `EditGoal` value whose `resource` is no identifier is `wrong_type` at
`/resource`, where it was `pool_not_targetable` ("One value by name"); and, in an installation that
also has a game with two instances, a `not_targetable` list names each pool of that other game once,
by the file's id, where it named each copy.

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
| `scale` | optional, `1..=2^53-1`: the gap step `S`; absent takes the pool's `valuation.scale`, else (#455 B3-P, since 0.12.82) its `target_defaults.scale`, so it is required when the pool declares neither ("Who gives a goal its step and importance") | `out_of_range` / `missing_field` @ `/targets/i/scale` |
| `importance_milli` | optional, `1..=1_000_000`: `I`; absent takes the pool's `valuation.gap.weight_milli`, else (#455 B3-P) its `target_defaults.importance_milli`, so it is required when the pool declares neither a `gap` nor `target_defaults` | `out_of_range` / `missing_field` @ `/targets/i/importance_milli` |
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

## Document `actingcommand.resource-targets.v3`

Workflow #455 G-R2a (0.12.60; register §1.361; the design `rd-1114/455-goals-design-final.md`
§3.4): the goal document's value store. The console and the Lab will read and write an
application's goals in one document; this version holds them in the record that already
exists, `session.resource_targets`, with no new fact key and no new file. Since Workflow #463 V2
(0.12.61; [package-variables.md](package-variables.md), "The run") a policy dispatch reads an
entry whose name is a variable of the dispatched package as that variable's value, judged at
use against the package's declaration; no other entry is read. Nothing writes the entries one at
a time yet: the goal view and the edit of one entry (G-R2b, Workflow #463 V3), deadlines (G-R4)
and tiers (G-R5) are later slices. The same entry, parser, 64 KiB bound and general reasons as v1
and v2.

```json
{
  "schema_version": "actingcommand.resource-targets.v3",
  "instance": "instance-a",
  "targets": [
    { "id": "credits-floor", "resource": "pool.credits",
      "condition": { "kind": "at_least", "amount": 100000 },
      "apply": { "mode": "adjust", "weight": "score_stage" },
      "valid_until_unix_ms": 1791000000000 },
    { "id": "energy-floor", "resource": "pool.energy",
      "condition": { "kind": "at_least", "amount": 200 }, "scale": 10, "importance_milli": 500,
      "apply": { "mode": "adjust", "weight": "score_stage" } }
  ],
  "entries": [ { "name": "route", "value": "a" }, { "name": "rounds", "value": 3 } ]
}
```

| Field | v3 rule | Rejection reason @ path |
| --- | --- | --- |
| `instance`, `targets` (`0..=16`) | as v2 | as v2 |
| `targets[i]` | a v2 target: every v2 rule of its members holds (`id`, `resource` and one target per resource, `condition`, `scale`, `importance_milli`, `rule`, `apply` with `manual_offset`, `tasks`) | as v2 |
| `targets[i].valid_until_unix_ms` | optional: the target's own lifetime, `now < v <= now + 31_536_000_000`; absent, the target does not expire | `validity_out_of_range` @ `/targets/i/valid_until_unix_ms` |
| `targets[i].by_unix_ms` | not a member of a v3 target in this slice: deadlines come with a later slice (G-R4), which adds the member | `unknown_field` @ `/targets/i/by_unix_ms` (checked right after the schema version) |
| `valid_until_unix_ms` (top level) | not a member: a v3 document has no lifetime of its own | `unknown_field` @ `/valid_until_unix_ms` (checked right after the schema version) |
| `entries` | optional, `0..=96` | `out_of_range` @ `/entries` |
| `entries[i].name` | `^[a-z0-9][a-z0-9_]*$`, 1 to 64 bytes, unique in the document | `invalid_value` / `duplicate_id` @ `/entries/i/name` |
| `entries[i].value` | any JSON value whose canonical text (below) is at most 1024 bytes and holds no control character | `invalid_type` (a fraction or an integer beyond 2^53-1, which the canonical writer refuses), `out_of_range` (over 1024 bytes), `invalid_value` (a control character) @ `/entries/i/value` |

The reader is as strict as v2's: an unknown member anywhere is `unknown_field`, and every
refusal names its field. The closed reason set is unchanged: no word is added. This slice
checks an entry's form only: it does not know the pack's declaration (G-R1, G-R2b). Workflow #463
V2: a value a package's variable does not take is stored as written and judged when a run is to
read it, which then reads the default (`goal_value_not_applied`, [package-variables.md](package-variables.md)).

**Version probe.** Exactly when the top-level `schema_version` is the string
`actingcommand.resource-targets.v3` the Runtime takes the v3 checks; the v2 probe and the v1
path are unchanged for every other document. `validate_catalog_declaration` with kind
`resource_targets` dispatches by the same probes.

The first failing check is reported, in this order: size, the declaration parse, schema
version, a top-level `valid_until_unix_ms` and then a target's `by_unix_ms`, bounds, forms and
identifier charset (per target in document order its members; then the entries' count, each
name and each value), uniqueness (target ids, tasks, resources, entry names), the v2 apply
rules; then the instance; then each target's own lifetime
in document order; then per target its resource, whether `S` and `I` resolve, and its tasks, as
v2.

**Canonical text.** An entry's value is written as the policy crate's canonical JSON writer
writes it (`canonical_serialized` in `crates/policy/src/canonical.rs`, the writer of every
policy identity): object keys sorted by their UTF-16 code units, no whitespace, strings as
`serde_json` escapes them, integers only, each within ±(2^53-1). The writer escapes U+0000 to
U+001F; a value whose text would hold U+007F to U+009F is refused, as no fact string may hold one.

**Applying a document.** `ApplyResourceTargets` takes v1, v2 and v3 (the Lab's `targets set`,
which passes a document file through unchanged, the agents' tools and the console's entry with
`expected_version`, Workflow #455 T1-R). In the hold of the fact write gate that writes the
record, after the store is synchronized, the instance's active record is read and the record
the document writes is composed over it:

| Document | Over | Result |
| --- | --- | --- |
| v1 or v2 | no record, or a v1 or v2 record (or one the readers refuse) | exactly as before G-R2a: the rows, identity, lifetime, replay and receipt of a v1 or v2 policy |
| v1 | a v3 record | refused `unsupported_schema_version` @ `/schema_version` (the v1 model is frozen and its targets are not v2 targets); nothing appended |
| v2 | a v3 record | a v3 record: the document's targets, each with the document's `valid_until_unix_ms` as its own; the stored entries kept as they are |
| v3 | no record, or a v1 or v2 record | a v3 record of the document's targets and entries |
| v3 | a v3 record | the targets replaced; the stored entries kept as they are unless the document carries `entries`, which (an empty list included) replaces them all |

A composition with no target and no entry left is the withdrawal a v2 document with
`targets: []` writes, byte for byte (a v2 header alone, without expiry); with entries left it is
a v3 record with no target. A document's checks come first: a refused document is refused for
its own reason, and the v1 refusal above comes before the `expected_version` comparison, which is
T1-R's for every version. A record the v3 reader refuses counts as no v3 record: every document
applies over it as over none.

Only a v3 document's rows name who set them, so only a v3 document needs the request's channel
(the coordinator's ruling of 11-14 on #1702). The door tags every client request with its
connection's channel; a request that names none all the same applies a v1 or v2 document as any
other request does, and its v3 document is refused at request level: Denied `invalid_request`
with the host code `channel_binding_missing` (operation `apply_resource_targets`), in the receipt
only, nothing appended and nothing latched.

**Identity and replay.** A v3 record's `policy_sha256` is `sha256:<hex>` of the canonical
serialization of the document it holds: its targets, each with its own lifetime when it has one,
and its entries (members left out when absent, `entries` left out when there is none), so a
document that keeps the stored entries is the same policy as one that carries them. Replay is as
before: an equal identity over the active record appends nothing and answers `replayed`, also
after a restart. A v3 record has no expiry, so it is replayed until replaced; a document one of
whose targets' lifetime has passed is refused at that lifetime.

**The receipt.** `ResourceTargetsApplied` keeps its shape and its rules. The receipt of a v3
document never names a lifetime or conditions: `valid_until_unix_ms` is absent and `conditions`
empty, both together, whatever its targets carry (`ResourceTargetView` answers each target's own
lifetime and condition). The receipt of a v1 or v2 document is the one it answers before G-R2a,
also when the record it is applied over is a v3 record: the document's `valid_until_unix_ms` and
one condition per target. Its `policy_sha256`, `version` and `event_id` name the record
written: over a v3 record that is a v3 record, so `policy_sha256` is that record's identity,
not the v2 document's, the one difference from the receipt before G-R2a. The coordinator's ruling of 11-14 on #1702.
**`expected_version` is only ever the value a read answered (`active.version`, or `0` without
`active`); the receipt's `version` is not a value to send back.** After a withdrawal, or a v3
document that leaves a record of entries alone, the receipt's `version` is the sequence of the
record written while `ResourceTargetView` answers no `active`, so the next write sends `0`
(the coordinator's ruling of 11-14 on #1702; the receipt is unchanged).

**#429 R-13.** #429 R-13 had reserved the version name `actingcommand.resource-targets.v3` for a
target table of its own (`maintain` and `gain` targets, `paused`, a `stale_schema` mark, a
per-target edit); this design took the name instead, and none of that table was built:
`EditResourceTarget` stays answered `not_implemented`, and `ResourceTargetView` fills neither
`active.rows` nor `stale_schema`.

**An older Runtime.** A Runtime build before G-R2a refuses a v3 document at the typed parse
(usually `unknown_field` or `missing_field`), and reads a stored v3 record as unreadable
(`unsupported_schema_version`: its v1 reader takes every header that is not v2), running that
instance on base scheduling and saying so with `resource_target_policy_unreadable`; its view of
the instance is `resource_target_view_policy_unreadable`. Nothing fails, and a v1 or v2 document
applied again replaces the record.

## Entry and receipts

`RuntimeOperation::ApplyResourceTargets { document_json }` (wire operation
`apply_resource_targets`). Request validation refuses an empty or over-64-KiB document as
`resource_targets_document_invalid`, Denied with the validation code on top of the receipt's
`outcome` and `invalid_request` as its `runtime_code` (receipt v2, Workflow #378 A4), as for every
request that fails validation. Until Workflow #434 LAB-6S1b it also refused any origin other than
`(agent, adapter)` as `invalid_agent_dispatcher_origin` (retired); since then every client
envelope may send the document, the Lab's `targets set` included
([lab-two-entries.md](lab-two-entries.md) §2.4). The request schema is
`actingcommand.runtime.request.v4`.

**`expected_version`** (Workflow #455 T1-R, since 0.12.60; register §1.351). The optional
member of `ApplyResourceTargets { document_json, expected_version? }`, on the wire since
`runtime-client` 3, is the version of the instance's policy the writer read: `ResourceTargetView`'s
`active.version`, `0` when the view has no `active`. **`expected_version` is only ever the value a
read answered (`active.version`, or `0` without `active`); the receipt's `version` is not a value
to send back** (the coordinator's ruling of 11-14 on #1702): after a withdrawal or a record of
entries alone the receipt names the record's sequence while the read answers `0`, and sending
that sequence is the conflict below. The document is checked as without it; then,
in the hold of the fact write gate that writes the record and before the replay below, the
version the read answers is compared with it, in every state: the sequence of the
`fact.published` that holds the instance's `session.resource_targets` record when
`ResourceTargetView` shows it as `active` (expired or not), and `0` when it shows no `active`
(no record, a withdrawal, a v3 record of entries alone). The gate takes that judgement from the
read's own reading of the record (`resource_targets_without_active`, the evaluator's
`stored_policy_state`, through which the view reads it). Another value is Denied
`fact_observation_version_conflict` with `fact_key` `session.resource_targets` and
`current_version` (kept in codes mode), and nothing is appended; the equal value applies, or
answers an equal active policy's replay, exactly as a request without the member. Without the
member nothing is compared and the last writer wins, as before. The `ui` channel, the console's,
takes the kind from the same release ([lab-two-entries.md](lab-two-entries.md) §3). History: from T1-R
(0.12.60, register §1.351) until Workflow #455 G-R2a (register §1.361) a withdrawal compared
with its own sequence, which no read answers, so a writer that sent the view's `0` after a
withdrawal met a conflict it could not resolve; since G-R2a the version a writer reads is the
version compared in every state the view answers. What this gives up: while the record holds
entries and no target, two writers of whole documents that both read `0` are not told apart by
this check, and the later one wins (its document replaces the targets and, only if it carries
`entries`, the entries). The versioned edit of single entries is a later slice's request,
which reads the record's own version through its own view. Before 0.12.60 a request that named the member was refused `not_implemented`.

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
| `version`, `event_id` | the sequence and id of that event (the receipt terminal); not an `expected_version` to send back: that is only ever what a read answered (`expected_version` above) |
| `previous_version` | the sequence of the policy active before, or `null`; equals `version` on a replay |
| `replayed` | the equal policy was already active and unexpired; nothing was appended |
| `checked_catalog_hash` | the active catalog the document was checked against |
| `valid_until_unix_ms` | the policy lifetime; `null` exactly for a withdrawal (a v3 record: "The receipt" under the v3 document) |
| `conditions_at_position`, `conditions` | the ledger position of the projection and, per target in document order, `{target_id, resource, fact_key, state}` |

`state` separates the applied configuration from what the target can compute now:
`{"kind": "computed", "current": c, "observed_at_unix_ms": t, "gap": max(T - c, 0)}` (a gap of
0 is satisfied) or `{"kind": "awaiting_observation", "reason": r}`. The inventory is the most
specific inline record of the pool's `fact_key` the instance can see after the time-validity
projection (a timeline reset drops observations made before it). It is computed when it is a
non-negative integer with a positive confidence (at least the pool's minimum for a
`ledger_fact` pool) and is not expired; otherwise `r` is the first of `missing`,
`low_confidence`, `expired`, `invalid_value` that applies. For a `ledger_fact` pool with a
regenerating projection, `c` is that record projected to the evaluation instant by the pool's
regeneration rule, as the pool's own value is (Workflow #390 POOL-1: each complete period since
`t` adds the pool's `amount`, up to its `capacity`; a record above `capacity` is read as
observed); `t` stays the record's observation time. A pending target is applied and waits for
an observation; it is not an error. #386 CF-5: when such a pool reads its `capacity` from a fact
([scheduling/README.md](scheduling/README.md), "Pool fact references"), that fact is read as
the evaluator reads it; while it gives no capacity the target awaits observation with its
reason (`missing`, `low_confidence`, `expired`, or `invalid_value` for no positive integer), and
a computed target's freshness ends no later than the capacity fact's.

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
| `expires_at_unix_ms`, `ttl_policy` | `valid_until_unix_ms` with `{1, 31_536_000_000, runtime_default}`; both absent for a withdrawal and for a v3 record (Workflow #455 G-R2a), whose targets carry their own lifetimes, so its entries never lapse with a target |
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

A v3 record (Workflow #455 G-R2a) stores four row kinds, at most 1 + 16 + 128 + 96 = 241 rows,
inside the 256 the scheduler's input takes (`MAX_OBSERVED_RECORD_LIST_ROWS`; a longer list would
be left out of the evaluation), at most 14 fields a row:

```text
{row: "policy", schema_version: "actingcommand.resource-targets.v3", instance, policy_sha256}
{row: "target", id, resource, at_least, mode, weight
 [, scale][, importance_milli][, rule][, manual_offset][, valid_until_unix_ms]
 [, set_unix_ms, channel][, initiator]}                                     one per target, document order
{row: "task", target, task}                                                 one per task reference, document order
{row: "entry", name, value_json, set_unix_ms, channel [, initiator]}        one per entry, document order
```

`valid_until_unix_ms` and `set_unix_ms` are integers. A target row carries the v2 optional
fields exactly when the document states them and `valid_until_unix_ms` exactly when the target
has a lifetime. `set_unix_ms`, `channel` and `initiator` say who set the row and when: the
Runtime writes them from the request, never from the document: `set_unix_ms` the Runtime's
clock at the request (the record's `observed_at_unix_ms`), `channel` the channel the request is
recorded with (`cli`, `mcp`, `ui` or `lab`, as the receipt names it; the door always supplies
one, binding an unbound connection at its first request, and a request without one is a fatal
`resource_targets_record_invalid`, a state only a bug reaches), and `initiator` the word the
request's connection declared, only for a request from the Lab (as the receipt carries it,
[lab-two-entries.md](lab-two-entries.md) §4.3). A target that came through a v3 document carries
them; a target that came through a v2 document applied over a v3 record does not. Every entry
row carries them; a kept entry row is stored again exactly as it was, with its own. `value_json`
is the value's canonical text ("Canonical text" under the v3 document). A v3 header selects the
v3 reader: the header, the target rows without their v3 fields and the task rows are read by the
v2 reader, unchanged; a target's `valid_until_unix_ms` is a positive integer; `set_unix_ms` (at
least 0) and `channel` (one of the four words) come together, with an optional `initiator` (one of
the six words); an entry row has exactly its fields, a name of the entry form, unique, and a
`value_json` that is the canonical text of a value the v3 document takes; at most 96 entries; a v3
record has no expiry. Anything else is unreadable (`malformed_rows`, `unsupported_schema_version`
or `instance_mismatch`).

**Replay and withdrawal.** A submission whose `resource_bundle_hash` equals the active,
unexpired policy's appends nothing and answers that policy's version with `replayed: true`,
also after a Runtime restart. A different policy replaces the active one
(`previous_version` names it). `targets: []` withdraws: it stores a header-only policy without
expiry. An expired policy stays stored until replaced; applying the same document again is
refused because its lifetime has passed (`validity_out_of_range`).

**The formal entries only.** `ApplyResourceTargets` and, since Workflow #463 V3 (0.12.61),
`EditGoal` ("One value by name", below) are the two formal entries of this key. (Since Workflow
#455 RR2, 0.12.94, `EditGoal` writes a records entry's records into another key,
`session.goal_records`, of which it is the one entry: [instance-fact-store.md](instance-fact-store.md),
"Goal records facts".)
`PublishFact` / `PublishFacts` refuse any record of this key as
the non-fatal `resource_targets_formal_entry_required`, before the fact write gate. A
configured policy input fact of this key fails every authoritative projection, so the daemon's
first evaluation fails and the configuration must be corrected; a forward projection caller
supplying it is refused; both as `resource_targets_key_reserved`.

**Identity and budget.** The policy is not excluded from the overlay: it reaches
`EvaluationFacts.facts`, the combined `fact_snapshot_id`, admission staleness
(`policy_facts_stale`), the `FactsChanged` trigger and forward projections like every overlaid
fact. Each instance's policy takes one of the store's active facts, since Workflow #498 one of
the instance's own 224 ([instance-fact-store.md](instance-fact-store.md), "Capacity ledger"; before
it one of the host's 256 identities); each inventory key it reads takes its own.

### One value by name

Workflow #463 V3, which is Workflow #455 G-R2b with the variables in it (0.12.61, register
§1.383; [runtime-interface.md](runtime-interface.md), "Goal request kinds"). `EditGoal` changes
one row of the instance's stored record and keeps every other row as it is stored, the rows
another request wrote with who set them:

- **A variable or another entry** (`set`): the entry row of the name is replaced where it stands,
  or added after the others; `clear` removes it. The row names the Runtime's clock at the request,
  the request's channel and, for the Lab, the initiator its connection declared. Fix round 1: a
  `clear` removes the stored row of its name whatever the name's entry is (a heading's or a note's
  included), so no stored value is left that nobody can remove.
- **A resource entry** of `goals.json` (`set` with `{at_least}`) and **an other target** (a name
  no entry has, `set` with `{resource, at_least}`): one target row, mode `adjust`, weight
  `score_stage`, its step and importance the pool's valuation's, or (#455 B3-P, since 0.12.82) its
  `target_defaults` on a pool without a valuation, read at each evaluation and never written in the
  row (a pool that gives neither is refused `pool_unvalued`; "Who gives a goal its step and
  importance"), no tasks and no lifetime; it replaces the target on that pool
  whatever its id (the answer's `replaced_target_id`) and a target of the same id on another pool.
  `clear` removes the target and its task rows. #503 GOAL-1 (since 0.12.88; "A target's pool"):
  the row's `resource` is the pool's id as the game root's files write it, the resource entry's
  `governs.pool` or the other target's `resource`; an other target's `resource` that is no pool
  identifier (a composed copy's id among them) is refused `wrong_type` at `/resource`, one that
  no pool of the instance has `pool_not_targetable`, both before anything is composed.
- **Over a v2 policy** its targets become v3 targets with the policy's lifetime and no setter, as
  under a v2 document applied over a v3 record; a v1 policy or rows no reader reads are refused
  `record_unreadable`; a record left with no target and no entry is the withdrawal.
- **The version** compared is the sequence of the `fact.published` that holds the record (`0`
  without one), the one `GoalView` answers; a stored record of entries alone has that version too,
  where `ResourceTargetView`'s `active.version` reads `0`. It is compared under the fact write
  gate first; only then are the name and the value checked against the record about to change.
  Workflow #455 RR2 (since 0.12.94; [instance-fact-store.md](instance-fact-store.md), "Goal
  records facts"): the goal page keeps one version over this record and the instance's
  `session.goal_records` record, the records of its `records` entries, which `EditGoal` alone
  writes: the greater of the two sequences (`0` without either), the one `GoalView` answers and
  every `EditGoal`'s `expected_version` is compared with, so an edit of either record refuses a
  stale edit of the other; the answer's `version` is that version after the edit, and a refused
  stale edit names the page's version as `current_version` with the key of the record that holds
  it. An application that holds no goal records record compares exactly as before.
  `ApplyResourceTargets` compares its own `expected_version` with this record's alone, as before.

The whole record is read back by the v3 reader before it is published, as one `fact.published`
keeping the request's origin, under the fact write gate. #503 GOAL-1 (since 0.12.88): a composed
record that does not read back is a refusal of that request, Denied with
`resource_targets_record_invalid` (an error; its daemon line is written), nothing appended and the
stored record as it was; it latches nothing, so the Runtime and its other instances go on. Until
0.12.88 it was fatal and stopped the Runtime.

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
| unmapped | the target's pool, or this task, no longer maps | unchanged | `resource_target_unmapped` |
| pending | no usable inventory observation | unchanged | `resource_target_pending` |
| satisfied | `g = 0` | unchanged; an override is released | `resource_target_satisfied` |
| applied | `g >= 1` | scored as below | `resource_target_applied`, and `resource_target_override` in override mode |

A named task that can have no candidate on the instance (unknown, out of scope or disabled) is
listed on every candidate of the instance as `resource_target_tasks_unevaluable`, so a target
never lapses silently. From Workflow #455 G-R2s (0.12.60) until G-R2x (0.12.77) a candidate whose
task carried the irreversible mark met the effect `none` whatever named it; since G-R2x it meets
its effect as any named task does ("A task that can do something irreversible" above).

"Unchanged" in this table is the target's own part only: since SS-1 every effect leaves the base
term of a candidate that produces a valued pool in place ("Base valuation on every instance"
above).

Score (`shortfall_linear`; exact integers with 128-bit intermediates, truncating):

```text
g   = max(T - c, 0)                          gap of the target (at_least T, inventory c)
w   = min(floor(g*I / S), 1_000_000)         target weight (scale S, importance_milli I)
r_k = sum of effective production in real units, retaining milli-unit fractions (defined above)
u_k = min(r_k, g)                            useful contribution of one run
U   = max u_j over the target's tasks that still map, and those an instance override
      disables (SS-1 one-way guard)          (>= 1 while g >= 1)
s_k = min(floor(g*I*u_k / (S*U)), 1_000_000) task target score, "capped" at the bound
```

The task with the largest useful contribution scores `s = w`; the other tasks score at most
`s` (`s_k = min(floor(g*I*u_k / (S*U)), 1e6)`), and at the cap, or when `u_k = U`, they can
tie with it. `U` counts a disabled named task too (SS-1); when that task has the largest useful
contribution, no candidate reaches `w`. `s_k` never grows as `c` grows. With
`base = score + utility + offset` (see `contracts/scheduling/README.md`, "Score-Assisted
Priority"):

- `adjust`: `effective = base + s`. The disposition follows the thresholds as for any scored
  candidate, so with a selection document `s` takes part in the promotion comparison.
- `override`: `effective = utility + s`. The selection score and the manual offset are
  superseded; utility, priority, aging, strategic weight, urgency and contention stay. The
  disposition is always `none`: never deferred, never promoted.

The thresholds are computed from `base`, never from `s`. `total_score` gains
`effective * 1000`, so `s = 1000` weighs one priority level; the effective score also orders
the shared run limits allocation and the performance arbitration like any other. Every admission
predicate (authorization, budgets, windows, pause, cooldown, availability, host capacity)
stays in force.

An applied candidate's `facts_fresh_until_unix_ms` is bounded by the inventory observation's
expiry and `valid_until_unix_ms`, so admission refuses its intent as `policy_facts_stale` once
either lapses; the evaluation wakes at each plus one. No other effect changes freshness or
wakes, and none requests detection. The policy and the inventory are part of the combined
`fact_snapshot_id`, so a changed inventory also refuses an already evaluated intent as stale.

Reasons follow `score_computed` (whose detail ends with ` target=<id>:<mode>:<s>` when
applied) and `score_unknown`, and precede the disposition reason: at most one policy-level
reason (the first that applies of ignored, unreadable, expired, tasks_unevaluable), one
target-level reason and one override reason. With a valued pool a v1 instance's candidate also
gains ` resources=<R>` at the end of `score_computed` and `resource_weights` (`policy=none`)
after these ("Base valuation on every instance" above). Each reason is a registered code with
its values (Workflow #378 A2-P); the target-level and override reasons carry the target's id as
the value `resource_target_id`. The details are developer text, never persisted; every detail
stays within 1024 bytes.

| Code | Detail |
| --- | --- |
| `resource_target_applied` | `policy=<sha> applied_at=<ms> resource=<pool> fact_key=<k> mode=<m> current=<c> at_least=<T> gap=<g> scale=<S> importance=<I> weight=<w> per_run=<r> useful=<u> best_useful=<U> task_target=<s>[ capped]` |
| `resource_target_override` | `superseded score=<s\|none> offset=<o>; utility kept` |
| `resource_target_satisfied` | `policy=<sha> applied_at=<ms> current=<c> at_least=<T> gap=0 mode=<m>`, plus `; override released` in override mode |
| `resource_target_pending` | `fact_key=<k> reason=<missing\|expired\|low_confidence\|invalid_value>; configuration applied, waiting for a valid observation` |
| `resource_target_unmapped` | `resource=<pool> task=<task> why=<unknown_resource\|resource_not_observable\|resource_out_of_scope\|task_not_producing>` |
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
holds targets and `now <= valid_until_unix_ms`. Since SS-1 (0.12.30) the base weight counts on
every instance: without a policy, after a withdrawal, after expiry, with an unreadable record
or with a v1 policy, every candidate takes the term `R` below with no target (`Γ = 0`, `W = B`),
as "Base valuation on every instance" says; an enabled instance only adds its targets' shortfall
`Γ`. (Until 0.12.29 no base weight counted on an instance that was not enabled.) A v1 policy
still takes the path above for its own targets and never reads `valuation` for them.

**A v3 record** (Workflow #455 G-R2a) is read into the same resolved form as a v2 record; there is
no third evaluation path. At each evaluation the targets whose own lifetime has not passed
(`now <= valid_until_unix_ms`, or none) are the targets of an active v2 policy, whose lifetime is
the earliest of theirs (none when none of them has one); everything below holds for them as for a
v2 policy's: `R`, `Γ`, the override, the reasons, freshness and the wake at that lifetime plus one,
so the evaluation wakes at each target's expiry as at a v2 document's. A target whose own
lifetime has passed is left out: it weighs nothing, and every candidate it covered (a task of the
instance one run of which produces its pool, among its named tasks when it names some) carries
`resource_target_policy_expired` (`policy=<sha> applied_at=<ms> valid_until=<its lifetime>`) as
its policy-level reason, as under an expired v2 policy, before `resource_target_tasks_unevaluable`.
When every target's lifetime has passed the record is an expired v2 policy whose lifetime is the
latest of theirs. A record without targets (entries only) is no policy. The policy's version is
the record's `policy_sha256` and `applied_at`, and the dispatch decision record names it
`active.v2:` or `expired.v2:` as a v2 policy's, so a v3 record whose targets equal a v2 record's,
each with the v2 document's lifetime as its own, weighs byte for byte as that v2 record, its
identity aside. Entry rows are read by nothing yet.

**Per evaluation.** Each target is resolved once for the instance against the active catalog:
its pool (as v1), then `S = scale ?? valuation.scale ?? target_defaults.scale` and
`I = importance_milli ?? valuation.gap.weight_milli ?? target_defaults.importance_milli` (#455
B3-P, since 0.12.82: the third source; "Who gives a goal its step and importance"; the rule is
`shortfall_linear`, the only one), then its named tasks (as v1), then its inventory in the
time-validity projected facts (as v1). A left-out term whose pool no longer declares it (a catalog
update removed `valuation` or `gap`, and wrote no `target_defaults`) makes the target
*unresolved*: it contributes no shortfall weight and says so, never silently zero.

A target without `tasks` *lapses* when the active catalog no longer supports it on the
instance: its pool no longer resolves (`unknown_resource`, `resource_not_observable`,
`resource_out_of_scope`), or no task whose scope covers the instance and that no instance
override disables produces the pool (`no_producing_task`). A lapsed target weighs no candidate
and is listed as `<id>@<pool>:<why>` in `resource_target_tasks_unevaluable` on every candidate of
the instance, beside the v1-style `<target>/<task>:<why>` items of named tasks that can have no
candidate. Since SS-1 a `no_producing_task` lapse is a report only: the target still resolves,
and the only tasks it covers have no candidate (one-way guard); likewise a named task an
instance override disables is listed as `task_disabled` and keeps its coverage. Only these
catalog-level failures are listed: a producing task that is merely held back this round
(trigger, feedback stop, cooldown, placement, budget) is not a lapse. A target with `tasks`
whose pool no longer resolves shows `resource_target_unmapped` on its named candidates, as v1.

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
Q_p    = p.valuation.scale, else t.scale, else p.target_defaults.scale (#455 B3-P)
T(k,p) = floor(r(k,p) * W(k,p) / Q_p)
F(k,p) = fractional remainder of r(k,p) * W(k,p) / Q_p for pools with an explicit expected effect; 0 otherwise
R(k)   = min(sum of T(k,p) + floor(sum of F(k,p)), 1_000_000) ("capped" at the bound)
```

A pool takes part for `k` when `r(k,p) > 0` and it declares a valuation or a target covering
`k` names it. `W` belongs to the resource; a target's scope only decides who gets the
shortfall part. From Workflow #455 G-R2s (0.12.60) until G-R2x (0.12.77) a candidate whose task
carried the irreversible mark read `W(k,p) = B_p`; since G-R2x it reads this section as every
candidate does ("A task that can do something irreversible" above). Only `produces` count: `consumes` still drives urgency alone. `Γ`, `W`, `T` and
`R` never fall as the gap grows; at `g = 0` the base weight stays. A task's catalog `reserves`
(Workflow #411 B16 part 2) are a separate hard gate after the score stage, not a term of `R`:
of two candidates whose consumption fits a reserve alone but not together, the one with the
higher `effective_milli`, which includes this term, runs first ([scheduling/README.md](scheduling/README.md),
"Task Reserves").

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

**Other instances.** Their scores, relative order, thresholds and reasons are those of their
own record (with the base term of "Base valuation on every instance"), whatever this instance's
policy says. The shared run limits are still allocated in global total order, so while one is short an enabled
instance's candidates can take it first and another instance's candidate then records
`heavy_scene_budget_deferred` or `policy_concurrent_runs_deferred` for the round, as with a v1
target (since Workflow #427-B the round deducts no CPU, GPU or IO, and `host_budget_deferred`
is retired; [scheduling/README.md](scheduling/README.md), "Shared Run Limits").

**Freshness.** A candidate that `R > 0` or an effective override moves is fresh at most until
`valid_until_unix_ms` and the expiry of every inventory observation behind a positive
shortfall weight (or the effective override) covering it; admission refuses its intent as
`policy_facts_stale` once one lapses and the evaluation wakes at each plus one. Forward
projections run the same evaluator and include the resource term.

**Reasons.** On an enabled instance every candidate gains, after `score_computed` and
`score_unknown` and before the disposition reason, at most five reasons:

| Code | Detail |
| --- | --- |
| policy level (as v1, at most one) | `resource_target_policy_ignored`, `resource_target_policy_unreadable` or `resource_target_tasks_unevaluable` (items in target order, `<target>/<task>:<why>` for a named task as v1 and `<id>@<pool>:<unknown_resource\|resource_not_observable\|resource_out_of_scope\|no_producing_task>` for a lapsed target without `tasks`, joined by `,`, at most 1010 bytes, the rest as `+<n>more`), on every candidate of the instance (a candidate whose task carried the irreversible mark included since G-R2x, 0.12.77; under G-R2s it carried `resource_target_package_irreversible` instead, retired) but one of a game whose root cannot be composed ("A game whose root cannot be composed" above, `resource_target_root_uncomposed` instead); after expiry `resource_target_policy_expired` on every candidate the policy would weigh |
| `resource_targets` | the targets covering the candidate in document order, joined by `; `, each `<id>@<pool>:applied current=<c> at_least=<T> gap=<g> step=<S> importance=<I> gap_weight=<Γ>[ capped]`, `<id>@<pool>:satisfied current=<c> at_least=<T> gap=0 gap_weight=0`, `<id>@<pool>:pending reason=<missing\|expired\|low_confidence\|invalid_value> gap_weight=pending` or `<id>@<pool>:unresolved why=<valuation_missing\|gap_missing> gap_weight=0` |
| `resource_target_unmapped` | as v1, for a task a target names explicitly |
| `resource_target_override` | `superseded score=<s\|none>; offset=<o> kept (manual_offset=keep); utility kept` or `superseded score=<s\|none> offset=<o> (manual_offset=supersede); utility kept` |
| `resource_weights` | `policy=<sha>@<applied_at> term=<R>[ capped] items=<pool>:r=<r>,per=<Q>,base=<B>,gap=<Γ>,effective=<W>,term=<T>;…`, items by `T` descending then pool id; only for a candidate with at least one item. On an instance that is not enabled (SS-1), and for a candidate of a game whose root cannot be composed on any instance (G-R2s round 2; until G-R2x, 0.12.77, also a candidate whose task carried the irreversible mark), the base term alone, `policy=none` and every `gap=0`, after the reasons of its record (of no record for the candidate left out) |

List details are counted while joining and stay within 1010 bytes; items that do not fit fold
into `+<n>more`. On an enabled instance `score_computed` ends with ` resources=<R>` and, under an
effective override, ` override=<id>:<keep|supersede>`; the dispatch decision record shows
`target=<R>` in `rank_breakdown` and `decision_candidate_not_selected`, `target_id=<override id|*>`,
`mode=<override|adjust>`, `superseded=1` under an effective override, and ends `rank_breakdown`
with ` offset_kept=<0|1>`; `decision_record` names the policy `active.v2:<sha>@<applied_at>`
or `expired.v2:<sha>@<applied_at>`. On an instance that is not enabled a candidate with a base
term shows the same ` resources=<R>` and, unless a v1 target applies to it, the same record
fields (`target_id=*`, `mode=adjust`, ` offset_kept=1`); a candidate without one keeps every
reason byte for byte (SS-1; the v1 `U` of the one-way guard aside).

**Net benefit** (SS-2b, since 0.12.32; register §1.225). A judged candidate's net benefit reads
this section's `r(k,p)`, `W(k,p)` and `Q_p` for what it produces, and `W(k,p)` (the base weight
for a pool it only consumes) as the cost of what it consumes
([scheduling/README.md](scheduling/README.md), "Net benefit within a tier"). It reads the weights
the resource layer computed and writes none, so the one-way guard stands; `R` and every reason
above are unchanged. A pool whose `valuation` is the pending marker has no valuation here.
SS-2d (since 0.12.35; register §1.235): the same `W(k,p)` and `Q_p` bound the rounding of that
net benefit (scheduling/README.md, "Within tolerance"), and a candidate within it is released only
while a pool it produces has `W(k,p) > 0` on its instance, a goal's shortfall `Γ` included; a goal
that lifts the benefit above the bound makes it positive. Nothing here changes. From G-R2s
(0.12.60) until G-R2x (0.12.77) a candidate whose task carried the irreversible mark read the base
weights only; since G-R2x a goal lifts its benefit as any candidate's ("A task that can do
something irreversible" above).

**A goal on what a task with a fixed value produces** (#455 B1, since 0.12.82; register §1.518;
the coordinator's rulings of 11-23, not the owner's words). A task whose benefit is a fixed value
`V` produces only pools without a valuation (a priced or pending one beside `V` is refused,
[scheduling/README.md](scheduling/README.md), "Fixed values written in the pack"), so this section
weighs such a pool for it only through a target that covers it: `B_p` 0, `W(k,p) = Γ` and
`Q_p = t.scale`, the target's own `scale` and `importance_milli`, which the entry requires for a
pool without a valuation, or (#455 B3-P, since 0.12.82) the pool's `target_defaults` where the
target carries none, from the same resolution ("Who gives a goal its step and importance").
Each such pool whose `W(k,p)` is above 0 (a known gap `g ≥ 1`) adds its
production term `r(k,p)·W(k,p)/Q_p` to the task's net benefit `V − cost`, with its share of the
rounding bound; a satisfied, pending or unresolved target (`Γ = 0`) adds nothing, and no other
pool it produces is read. So a goal on a pool without a valuation now moves the fixed-value
stages that produce it within their tier and in a capped round; before, it reached only their
score-stage term `R`. Ruling 9 (a goal chooses which task spends a pool, the pool's lines choose
when): below the trigger line of a regenerating pool such a stage consumes (its capacity without a
written `trigger_line`), whether it may spend is judged without its goal terms, as before; at the
line, in the capped spending order, and for a pool that does not regenerate, they count
([scheduling/README.md](scheduling/README.md), "Fixed values written in the pack"). `R` and every
reason above are unchanged, and so is `gap_remaining`.

**A goal never takes `run_cap` away** (#455 B1 ruling 10, since 0.12.82; register §1.518). The
goal term of `run_cap` ([remedies.md](remedies.md), section 4, "Spend values") of a produced pool
with an open gap `g` that some `produces` entry declares by `expected_amount_milli` is
⌈g·1000 / `r(k,p)`⌉ runs, `r(k,p)` its expected yield per run in thousandths, at least 1; a pool
that yields nothing per run (its amount pending) bounds nothing. Before, such a pool removed
`run_cap` whole, the lines' and reserves' part included, so a sweep that reads it ran to its floor
or ended unresolved. `run_cap` is the least of the bounds that exist (the lines' or reserves'
runs, the goal's, 1 000) and is absent only where it was before for another reason (no consumed
pool takes part, one's reading or line is unknown, a consumed amount per run that is no integer).
An integer yield is bounded as before, and a task with no goal on what it produces is handed the
`run_cap` of before. A target needs nothing new: its pool is observed by a fact under `inventory.`
or `resource.` (or rows of a list), as every target's.

## Who gives a goal its step and importance

#455 B3-P (since 0.12.82; register §1.522; the coordinator's rulings of 11-23, not the owner's
words). A v2 or v3 target's gap step `S` and importance `I` come from three sources, in this order,
each number on its own:

1. **the target's own** `scale` and `importance_milli`;
2. **the pool's valuation**: `valuation.scale` and `valuation.gap.weight_milli`;
3. **the pool's `target_defaults`**: `target_defaults.scale` and `target_defaults.importance_milli`
   ([scheduling/README.md](scheduling/README.md), "A goal's step and importance on a pool without a
   valuation"), which only a pool that writes no `valuation` (neither a block nor the pending
   marker) may declare, so the second and the third never meet on one pool.

The pool's part (the second and the third) is read in one place of the policy crate
(`pool_target_terms`), from the catalogue in force at each check and each evaluation, and by every
reader of a target's terms: the entry's check of a target (`target_terms`, for v2 and v3 documents
and the goal page's edits), the evaluation's `Γ` (`target_terms`) and its `Q_p` (`weigh_production`:
the pool's valuation, else the covering target's own `scale`, else the pool's defaults), so B1's goal
term of a fixed-value task ("A goal on what a task with a fixed value produces") takes the same
`S` and `I`; and `ResourceTargetView`'s `default_scale` / `default_importance_milli` (with
`scale_required` / `importance_required` false). Nothing of it is copied into a stored goal: a goal
set by name, or a target without the two numbers, keeps none in its row, so when a later catalogue
changes a pool's `target_defaults` every goal already set weighs by the new numbers at the next
evaluation, with nothing set again. Where none of the three gives a number the target is refused
`missing_field` at its `scale` or `importance_milli` (the entry), the goal page answers
`pool_unvalued` and the Lab's check of `goals.json` `goal_pool_unvalued`, as before.

`target_defaults` makes nothing else of the pool valued: its base weight `B_p` stays 0, a fixed
value beside its production is taken, a judged task that consumes it or produces it without a fixed
value still has an unknown benefit (`valuation_missing`), its lines take no part, and a pool without
the member reads, hashes and decides exactly as before.

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
5. The base weight is a standing preference on every producing task of every instance (SS-1,
   with or without a target) and also orders the shared run limits, so keep `B` small.

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

## Read-only view: `ResourceTargetView` (Workflow #338 R5)

`RuntimeOperation::ResourceTargetView { instance_alias }` answers
`RuntimeResult::ResourceTargetView { view }` (`Completed`, no terminal). It has no origin gate
(like `Status`), records nothing and changes no policy. An unknown alias is `instance_unknown`.

```text
{instance_alias, policy_instance, evaluated_at_unix_ms, as_of_ledger_position, catalog_hash?,
 targetable:[{resource, fact_key, producing_tasks:[...], default_scale?, default_importance_milli?,
              scale_required, importance_required,
              observation:{current?, observed_at_unix_ms?, pending?}}],
 not_targetable:[{resource, reason}],
 active?: {policy_sha256, schema_version, version, event_id, valid_until_unix_ms?, expired,
           targets:[...], conditions:[...]}}
```

- `policy_instance` is the value a document's `instance` field takes for this instance (the
  policy instance id of the authoritative projection).
- `targetable` / `not_targetable` judge every pool of the active catalog with the predicates of
  a v2 target that names no tasks: the pool resolves, is observable through a `resource.` or
  `inventory.` fact and is in the instance's scope (else `resource_not_observable` or
  `resource_out_of_scope`), and some task of the instance produces at least one unit per run
  (else `unmapped_task`). `default_scale` / `default_importance_milli` are the pool valuation's
  `scale` / `gap.weight_milli`, else (#455 B3-P, since 0.12.82) its `target_defaults`' `scale` /
  `importance_milli` ("Who gives a goal its step and importance"); `scale_required` /
  `importance_required` say a target must state its own. `observation` reads the inventory through the time-validity projection
  exactly as a target condition does: `current` and `observed_at_unix_ms`, or `pending` with
  its reason. Without an active catalog both lists are empty and `catalog_hash` is absent.
  #455 P2 (since 0.12.79): a pool read from rows of a list is targetable under its list's key,
  its `observation` the reading the host computed under the same locks ("A pool read from rows
  of a list" below). #503 GOAL-1 (since 0.12.88; "A target's pool"): each list names a pool by
  the one form a target takes, its id as the game root's files write it, each once, and judges it
  as a target of that name: an instance's own copy is listed under the file's id; another
  instance's copy of a pool this instance has is no second pool of the instance and is not
  listed; the copies of a pool of other instances are one entry, by the file's id, with the
  reason a target of that name meets. With no copy in the catalog the lists are as before.
- `active` is the instance's stored policy as the evaluator reads it (the v1 or v2 row decoders
  and the active or expired state), with the `fact.published` event that holds it (`version`,
  `event_id`), `expired` once `valid_until_unix_ms` has passed, the targets in document form
  and one condition per target whose pool still resolves (the `ApplyResourceTargets`
  condition). It is absent without a policy or after a withdrawal. A stored policy the
  evaluator cannot read is `resource_target_view_policy_unreadable` (`Denied`,
  `ledger_failure`, the decoder's code as detail); a store revision whose hash differs from the
  projected policy is `resource_target_view_policy_inconsistent`.
- Workflow #455 G-R2a: for a v3 record with targets, `active` names `schema_version`
  `actingcommand.resource-targets.v3` and the record's version and event, lists every target in
  v3 document form with its own `valid_until_unix_ms` when it has one (lapsed targets included),
  and one condition per target whose pool still resolves. It has no `valid_until_unix_ms` (a v3
  record has no lifetime of its own), and `expired` is `true` exactly when every target's own
  lifetime has passed. A v3 record without targets has no `active`. The answer's shape and every
  answer for a v1 or v2 record are unchanged. Entries are not shown by this read: the goal view of
  G-R2b shows them. A reader of the previous revision (a console built against the released
  client) decodes a v3 record's view with the same strict types, which G-R2a does not change, and
  the answer passes their `validate`. It sees an `active.schema_version` it does not know and must
  not interpret the targets.

The view takes the check phase's locks in its order (`policy_outcome_gate` → `policy` →
`fact_write_gate`, the projection, then the fact store for the active revision) and releases
them; it appends nothing itself. The projection's fact-store synchronization is that of every
policy read.

An older Runtime does not know the operation: the frame fails to decode and the connection is
dropped without a receipt. Only when the call wrote its own frame and that connection then
ended before the receipt header does `RuntimeClient::resource_target_view(alias)` reconnect
and, when the same owner epoch answers, report `runtime_operation_unsupported`; any other
failure is returned unchanged; the skill then
falls back to reading the scheduling catalog.

## A pool read from rows of a list

#455 P2 (since 0.12.79; register §1.505; the coordinator's rulings of 11-22, not the owner's
words; [scheduling/README.md](scheduling/README.md), "A pool read from rows of a list"). A pool
whose `observation` is of the kind `list_rows` is observable, under its list's key L (under
`inventory.list.`, so of the `inventory.` family): a v1, v2 or v3 target may name it, and its
check refuses it no more with `resource_not_observable` (in 0.12.78 it did, as for a pool observed
by an outcome). Its inventory is the reading the host computed from the instance's list under the
same locks as the projection, never the list itself: known, the number projected by the pool's
regeneration rule, its `observed_at_unix_ms` the list's and its freshness the list's expiry;
unknown, `awaiting_observation` with the reading's reason (`missing` without a list or after a
reset that invalidated it, `low_confidence`, `expired`, `invalid_value` for a value that is no
non-negative integer or a sum beyond 2^53 − 1). The check, the receipt's `conditions`, the
evaluation's gap and weights (applied, satisfied, pending), `ResourceTargetView` (`targetable`
and `active`) and a goal entry's `current` all read it; no member and no word is added.

## Client and CLI

`RuntimeClient::apply_resource_targets(document_json) -> ResourceTargetsApplied` checks only
the length; a refusal's receipt is the error's `received_receipt()`.

```text
actingctl agent-apply-resource-targets --state-root <state-root> --policy-file <policy.json>
```

The CLI reads at most 65536 bytes, requires UTF-8 (`resource_targets_file_invalid` otherwise,
a local fatal) and does not parse the content. It prints `{"applied": ...}`, or
`{"receipt": ...}` with a non-zero exit when the Runtime refuses.
