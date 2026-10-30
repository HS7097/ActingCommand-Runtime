# Scheduling Catalog Contract

The versioned [V2 contract](v2/README.md) adds explicit timeline availability
through the same compiler and evaluator. This document specifies V1.

The scheduling catalog is a data-only contract of four required documents plus one optional selection document. It cannot contain executable code, scripts, network requests, device actions, or implicit defaults.

## Offline inspection

`actinglab scheduling compile --tasks <tasks.json> --pools <pools.json>
--activity <activity.json> --timeline <timeline.json> [--selection <selection.json>]`
reads four explicit local files, plus the optional selection document when
`--selection` names one, through Lab and calls the existing policy
`compile_catalog` entrypoint.
With `--json`, one ordinary CLI envelope contains the compiler's `dry_run_json`
report, including summary and catalog hash. Rejection returns exit 2 and the
original structured compiler diagnostics under `error.details`. File read errors
also fail explicitly. V1 compilation and catalog identities retain their contract.

`actinglab scheduling timeline` takes those same file flags (including the
optional `--selection`) plus repeated
`--event-id <id>`, `--unix-ms <u64>`, `--monotonic-ms <u64>`,
`--instance-id <id>`, `--server-id <id>` and `--game-id <id>`. All are explicit;
instance context must correspond to the configuration being inspected. The
command reads no system clock, Runtime configuration, session, facts, capabilities
or task state. Routing/backend/execution global flags are rejected. Both commands
are `offline` and `read_only` and create no state directory or persistent facts.

The timeline response contains the compilation report and the pure policy query:
catalog ID/version/hash, supplied time and context, requested events in request
order, each event's declared scope and `scope_applies`, `availability.state`
(`true`, `false` or `unknown`), `active_interval` as `[start, end]` Unix milliseconds
representing the half-open interval `[start,end)`, and `next_wake_unix_ms`.
An inactive event has a null active interval. The top-level query wake is the
minimum wake across the selected events; it is not the full scheduler's wake.
Unknown or duplicate event IDs and empty selections are input errors.

Policy `inspect_timeline` shares the availability and scope owner used by
`TimelineActive`, including its existing clock, duration and validity semantics.
Positive availability requires explicit V2 validity. Zero-duration reset events
remain unknown for availability, including V1 reset events; their production
invalidation purpose is unchanged. Applicable closed/expired events are false,
and inapplicable scopes are unknown. Arithmetic failures retain policy errors.

Each source is limited to 1 MiB, and the compiler enforces the 4 MiB catalog bound.
The reader uses at most one additional sentinel byte per document to preserve the
compiler's size diagnostic. Paths use the existing 1024-byte source-text bound;
server/game context and event IDs use 128 bytes; the instance context uses the
registered 256-byte alias rule. Selection is limited to 4096 events. CLI query
arguments are bounded to 8212 tokens and 1 MiB, enough for all events and ten
single-value flags. Output payloads are limited to 16 MiB (four catalog budgets)
to allow structured source locations and diagnostics. Exceeding a bound fails
visibly without silently truncating a decision.

These commands verify compilation and event availability at a supplied time.
Production catalog activation, PolicyHost binding, admission and execution remain
the Runtime's normal chain; an offline query establishes no production run fact.

## Frozen V1 Documents

- `tasks.schema.json`: task entrypoints, bounded triggers, feedback stop conditions, effects, failure policy, load profile, loop budget, and instance overrides.
- `pools.schema.json`: scoped resource pools, regeneration or nonregenerating projections, observations, bounded group delay, and an optional resource valuation ("Pool Valuation" below).
- `activity.schema.json`: scoped activity windows, per-instance importance, bounded sessions, sampling policy, and goals.
- `timeline.schema.json`: scoped reset, maintenance, activity, and deadline events.
- `diagnostic.schema.json`: stable compiler diagnostic envelope.
- `selection` (optional fifth document): an `actingcommand.selection-policy.v1` scoring
  policy owned by [the selection-policy contract](../selection-policy.md). It carries its
  own schema version and no `catalog` descriptor.

All four catalog documents must carry the exact schema version `actingcommand.scheduling.v1` and an identical `catalog` descriptor. A mismatch rejects the whole catalog.

## Pool Projection

`projection` is required in V1 and V2 pools and accepts exactly one of these closed
objects:

- `{"amount": 1, "per_ms": 1000}`: both integers are positive. Each complete period
  adds `amount`, up to `capacity`, using the existing regeneration and wake rules.
- `{"kind": "none"}`: an available snapshot keeps its amount as time passes. It
  produces no regeneration wake and contributes zero fullness urgency to tasks
  consuming it. Deadline urgency and urgency from other regenerating pools remain.

Missing or null projections, unknown kinds or fields, mixed shapes, and zero
regeneration amounts or periods reject the catalog. Existing regenerating objects
retain their serialization and catalog hash. A `none` declaration changes the hash
and follows the ordinary version, approval and activation path.

Capacity keeps its positive safe-integer bound. Zero is a known empty amount;
missing observations remain Unknown. For inventory, a declared numeric ceiling
does not assert an account's physical storage limit. Neither projection mode
clamps an invalid observed amount into a valid balance. A V2 pool may instead
read its capacity from a fact of its scope (#386 CF-5, "Pool fact references"
below): an account's cap is read, never written as a constant.

Projection and value source are independent. `static_snapshot` keeps the caller's
fixed snapshot; it has no additional TTL or automatic update from resource readings.
`ledger_fact` keeps the original scope/key, confidence, expiry, native-instance
binding and input invalidation requirements. FactsChanged, expiry, timeline and
task-state wakes remain; time passing never refreshes a fact. Workflow #390
POOL-1: a package resource reading (or adjustment write) of the key is the live
pool's base, published without input invalidations, which the pool needs only
from a record that is not such a reading; the next reading or adjustment resets
the base, and the projection above regenerates from its observation time
(`contracts/resource-readings.md`, "Live pools"). A live pool with no usable
record (none, expired, below the pool's minimum confidence, or not an integer
within `0..=capacity`; #386 CF-5: against a capacity read from a fact, any
non-negative integer is usable) leaves its `resource_projection` predicate Unknown with
that fact reason (`fact_missing`, `fact_expired`, `fact_low_confidence`,
`type_mismatch`) instead of `resource_observation_missing`, so the task raises
the fact refresh's demand for the key ("Fact Refresh"). A resource target on a
live pool reads the same projected inventory ([resource-targets.md](../resource-targets.md)).

The pure forward projection advances a nonregenerating pool's simulated time
without increasing its amount or accumulating regeneration waste. Declared task
effects still affect the simulation, including insufficient-resource rejection,
production clipping and waste at capacity, overflow and insufficient-evidence
handling. It does not update real inventory or renew the observation's TTL.
`consumes`, `produces`, valuation, eligibility and resource-target scoring keep
their existing meanings; they do not reserve or settle real resource quantities.

A Runtime without `none` rejects the new shape. Historical catalog generations
are compiled when loaded, including for dispatch recovery, so switching the active
configuration to an older catalog does not establish binary downgrade safety for
a lineage containing `none`. Catalog rollback uses the existing approved-catalog
route on a Runtime that understands its recorded generations. No fact or event
wire field is added.

## Pool Valuation

Workflow #335 S2a. A pool may end with an optional `valuation` block that states
what one step of its resource is worth. V1 and V2 catalogs accept it alike, and a
catalog without it keeps its identity byte for byte ("Canonical Serialization And
Hash" below).

```json
"valuation": {
  "name": "Primary energy",
  "unit": "point",
  "scale": 1000,
  "base_weight_milli": 100,
  "gap": { "rule": "shortfall_linear", "weight_milli": 100 }
}
```

| Field | Bound | Meaning | Diagnostic |
| --- | --- | --- | --- |
| `name` | 1..=128 UTF-8 bytes, nonblank after trimming whitespace, no control characters | display name only | longer: `limit_exceeded`; blank or control character: `type_mismatch` |
| `unit` | 1..=32 UTF-8 bytes, the same text rule | display unit only | as `name` |
| `scale` (Q) | 1..=9,007,199,254,740,991 | measuring step: the resource amount one weight applies to | `limit_exceeded` |
| `base_weight_milli` (B) | 0..=1,000,000 | standing weight in milli for each produced step | `limit_exceeded` |
| `gap` | optional | shortfall conversion; when absent, a resource target must carry its own importance | — |
| `gap.rule` | `shortfall_linear` only | the target is to hold at least an amount | `type_mismatch` |
| `gap.weight_milli` (G) | 1..=1,000,000 | extra milli for each produced step per missing step of shortfall | `limit_exceeded` |

The resource kind is the pool `id`; there is no separate kind field. Task
quantities stay in `tasks[].produces` (`amount` or v2 `expected_amount_milli`, plus `confidence_milli`). Both
objects reject unknown fields (`unknown_field`), and a missing required field is
`missing_required_field`. Every violation rejects the complete catalog. A bound
violation or an unknown `rule` value carries the field's own JSON Pointer and
its source line and column. For a missing or unknown field the reason names the
field, while `json_path` and the line and column point at the nearest preceding
value (the existing parser's behaviour).

SS-2b (since 0.12.32; register §1.225; Alice's ruling 116, 「无害占位符或者空值」): in a V2 catalog a
pool may write `"valuation": "pending"`, the marker that its worth is not known yet. It does
nothing but make the net benefit of a judged task that touches the pool unknown ("Net benefit
within a tier"): every other reader sees no valuation, as without the member. Any other string
is `type_mismatch` at `/pools/<i>/valuation`, and so is the marker in a V1 document; `null`
stays what it was, no valuation. A catalog without the marker keeps its hash.

The block is a declaration. The compiler validates it and hashes it with the
catalog. Since SS-1 (0.12.30; Alice's ruling 100) the evaluator reads it for
every instance: its base weight weighs every producing task with or without a
resource target policy, and an active `actingcommand.resource-targets.v2`
policy only adds its targets' shortfall (`contracts/resource-targets.md`, "Base
valuation on every instance" and "Resource weights (v2)"). Until 0.12.29 it was
read only for an instance with an active v2 policy. A catalog without a
valuation ranks exactly as before, apart from the v1 `U` change of SS-1
(`contracts/resource-targets.md`, "One-way guard").

The bounds above are checked when the catalog is compiled (`compile_catalog`,
`actinglab scheduling compile`, Runtime activation and startup reload).
`validate_catalog_declaration` (`actinglab resource validate`) only parses a
document and checks its version, so an out-of-range value passes it and is
rejected at compilation. A Runtime build without this field rejects a catalog
that declares it (`unknown_field`) and cannot reopen a state root on which such
a catalog was activated or dispatched, even after its configuration returns to
an older catalog.

## Expected production (v2)

Each effect has exactly one quantity. `amount` is a positive integer in real inventory
units and keeps its existing semantics. Only v2 `produces` may instead declare
`expected_amount_milli`, an integer in `0..=9007199254740991` measured in thousandths of
a real unit per completed task run. It has no default, cannot exceed `pool.capacity * 1000`
(a constant capacity; #386 CF-5), and is mutually exclusive with `amount`. Explicit null, missing quantities and unknown
fields are invalid. V1 effects and all `consumes` require `amount`.

```json
{"pool_id":"material","direction":"produce","expected_amount_milli":400,"observation_source":"self_reported","confidence_milli":1000}
```

This declares an expected 0.4 units per completed run. Zero is a known zero yield;
an unknown yield must remain undeclared, not be replaced by zero. The producer computes
the expectation from separately established drop probabilities, quantities and batch counts.
The declaration is already that total: the Runtime does not multiply it by a probability,
batch count or `confidence_milli`. `observation_source` and `confidence_milli` remain required
evidence metadata; confidence is in `0..=1000` and is not a drop probability.

Task mapping, coverage and resource scoring retain these milli-units. Actual inventory
facts, pool capacities and inventory targets remain in real units. See
[`resource-targets.md`](../resource-targets.md) for final score rounding and checked bounds.
Forward projection returns `EvidenceInsufficient` with `effect_evidence_insufficient`
before applying a dispatched expected effect. The integer strategy planner returns an
unavailable production rate for a relevant expected producer, which follows its existing
needs-detection path; it rejects a reported definite rate inconsistent with that result.
Neither path records expected production as actual inventory.

SS-2b (since 0.12.32; register §1.225): in a V2 catalog an effect of either direction may write
`"amount": "pending"` as its one quantity, the marker that it is not known yet. It does nothing
but make a judged task's net benefit unknown ("Net benefit within a tier"): the resource terms,
the reserve gate, the tie-break and the spend values read it as no declared quantity of that
entry, forward projection as `effect_evidence_insufficient`, and the strategy planner as an
unavailable rate. With `expected_amount_milli` beside it the effect is `limit_exceeded`, as two
quantities are; any other string is `type_mismatch`, and so is the marker in a V1 document (at
the entry's `amount`). A catalog without the marker keeps its hash.

The compiler enforces quantity exclusivity, version, direction and capacity together;
the declaration-only parser checks wire types. Existing integer declarations serialize
identically when the new field is absent. A binary that does not understand the field
rejects it, so a state lineage containing these declarations requires a compatible reader.

## Compatibility

V1 rejects unknown fields and any schema version other than the exact supported value. A future schema revision is a new immutable contract and requires an explicit, separately tested translator before V1 input can be promoted. Readers must never reinterpret a V1 field under newer semantics, partially load a catalog, or invent defaults for missing data.

The `env.*` namespace remains an ordinary fact-key family. Existing execution substitutions that use `{env:...}` remain an execution-boundary concern; scheduling predicates reference the same stored values through typed `fact` predicates without changing that substitution syntax.

## Catalog Source

ROUT-L (frozen model v1.0 §3.1–§3.2; slice RL-1a; [standard-pack-schedule.md](../standard-pack-schedule.md)).
A catalog comes from one of two sources:

- **The configuration** (today's path): `policy.catalog` names the four documents, and
  `policy.catalog_approval_ids` and `policy.procedure_manifest` bind them. Everything below
  ("Catalog Lineage", "Scope And Overrides") describes that path. RL-C removes it.
- **The standard packs**: each game root may hold `schedule.json`, the game's rules without
  scope, instance or approval. The policy crate's `compose_standard_pack_rules` composes the
  games of the bound instances into one V2 catalog `{catalog_id: "standard-packs",
  catalog_version, approval_refs: []}`: one copy of each rule and item per bound instance, the
  file's ids with one instance per (game, server) and `<id>@<alias>` ids, with references
  rewritten and `continues_from` the file's id, with two or more. The composed documents are
  bounded by the 4 MiB catalog bound, not 1 MiB each (`compile_composed_catalog`); every other
  compiler rule applies. A composition without rules is a valid empty catalog. The scheduler
  workbench that loads, composes and activates it is RL-1b's
  ([standard-pack-schedule.md](../standard-pack-schedule.md), "The scheduler workbench
  (RL-1b)"): it runs when the `policy` section names no `catalog`, and a configuration that
  names one keeps the configured path for this release. Under a composed generation a
  `policy.resources.pools` reading for a pool no composed game declares is left out of the
  evaluation (`policy_pool_reading_unused`), and `continues_from` is read per instance.

Both sources compile into the same `CompiledCatalog` and are evaluated alike: the evaluator
orders candidates by score, tie-breaker, task id and instance, never by document order.

## Catalog Lineage

Workflow #361 A. A state root is not bound to one catalog id. The generations that were active
in a ledger are the targets of its successful `catalog.activated` and `catalog.rolled_back`
records and of the legacy active-pointer migration; they are rebuilt from the ledger on every
start and never deleted. `actingd` decides the transition its configured catalog asks for
before it records anything, in this order:

| Active | Configured catalog | `policy.catalog_transition` | Plan | Recorded |
|---|---|---|---|---|
| none | any | absent | `first` | `catalog.activated` |
| none | any | present | refused `catalog_transition_expectation_mismatch` | nothing |
| A | the hash of A | any | `unchanged` | nothing |
| A | the id of A at a higher version | absent, or `replace` expecting A | `forward` | `catalog.activated` |
| A | anything else | absent | refused `catalog_activation_not_newer` | nothing |
| A | any | `replace` expecting another hash | refused `catalog_transition_expectation_mismatch` | nothing |
| A | a generation that was active | `replace` expecting A | `rollback` | `catalog.rolled_back` |
| A | another id, never active, above every version of that id that was active | `replace` expecting A | `switch` | `catalog.activated` |
| A | otherwise | `replace` expecting A | refused `catalog_replace_version_not_newer` | nothing |

`replace` is the only transition kind; `expected_active_catalog_hash` is a compare-and-swap
value, and the transition is recorded under the same compare-and-swap
(`catalog_active_generation_changed` when another transition won). The field may stay in the
configuration after it was applied: the next start finds the same hash and plans `unchanged`.
A `catalog_activation_not_newer` refusal names the active and the configured generation and the
`replace` transition, expecting the active hash, that would switch or roll back instead; `actingd`
prints it on its `FATAL` line. Proposal promotion stays forward-only. Replay accepts exactly these records: a successful
`catalog.activated` keeps the id at a higher version or changes the id, a successful
`catalog.rolled_back` keeps the id at a lower version or returns to a generation of another id
that was active before; every other sequence stays `catalog_generation_source_conflict`. Every
ledger that satisfied the earlier rule satisfies this one and projects to the same generation.
A Runtime older than this rule refuses a ledger with a cross-id transition at start with
`catalog_generation_source_conflict`; switching an installation back to such a Runtime after a
`replace` therefore fails loudly at its first start.

There is no deactivation record. Stopping the scheduler is a configuration without `policy`
(recorded as the disabled `policy_driver` subsystem) or the global scheduling pause; the last
active generation stays the active generation of the lineage, and a later configuration with
any catalog continues from it with `replace`.

Task ids are global: dispatch history, last dispatch time, budgets and failure streaks are kept
per (task id, instance) across a switch, so a catalog that keeps its task ids keeps its clock
state.

Workflow #372 K5: a V2 task may declare `continues_from`, the task ids of an earlier catalog it
continues (`v2/README.md`). Under the active catalog a task's accounting group is its own id and
those ids, and every per-(task, instance) state the Runtime derives from its ledger is read over
the group on that instance: the task budgets are summed (the budget receipt's
`task_daily_used`, `task_window_used` and `task_runtime_reserved_ms` count the group, each
entry keeping only its own id's runs); the last dispatch, terminal state and completed window
come from the group's latest admitted dispatch, and the last duration and failure streak from
its settlements folded in ledger order; an outcome of a continued id's run is read as the
task's; the failure streak, a pause, a backoff and a retry round are those of the group's id
whose failure state a settlement set or cleared last, and a new settlement of the task counts
on from it under the task's own id; a clock-slot record of a continued id counts for the task.
The group is read at read time only from the active catalog: nothing in the ledger or the
control state is re-keyed, replay stays per dispatch against its own generation, and the
declaration is not followed across generations (a task that continues a task which itself
continued another lists both ids). The Runtime never infers a rename: an undeclared rename is a
new task, and a task that keeps its id keeps its state as above.

## Scope And Overrides

Runtime evaluation replaces task runtime snapshots with its replayed admitted and
completed dispatches at the input ledger position. The latest admitted dispatch
supplies `last_dispatched_unix_ms`; a completed dispatch supplies its recorded
execution success/failure. This is scheduling execution state, not a business
effect or a catalog outcome key. No observed eligibility-start time is invented.

Before assigning an instance winner, Runtime consults the existing control owner's
read-only admission predicate for task/activity daily, window and runtime budgets,
activity windows and sampled intervals. Unavailable candidates leave the instance
and host capacity available to the next ranked task. The same predicate supplies
the next possible admission time at bounded weekly window/cadence boundaries.
Final admission still checks and commits the actual counters under the original lock.
An operator scheduling pause (`contracts/scheduling-pause.md`) presents the same way: a
candidate on a paused instance is deferred with reason `dispatch_paused_global` or
`dispatch_paused_instance` and no next wake time, until `ResumeScheduling` lifts the pause.

`policy.dispatch_rejected` retains its immutable ranking reasons and optionally
carries `rejection`: the original Runtime error code, operation and fatal flag,
the decisive budget dimension with used/requested/limit values when applicable,
and the next eligible Unix time when one is available. These are bounded typed
control facts; native text and secrets are not copied into ranking or rejection
details. Existing events without rejection details remain unchanged.

Every task, pool, activity profile, and timeline event declares `instance`, `server`, or `game` scope. `instance_overrides` is the only V1 task override layer. A null override field means inherit the catalog value; it does not mean zero, false, or an inferred default. Activity profiles scoped to an instance carry its importance and goals.

The `instance_id` in instance scopes and overrides stores the exact registered
instance alias: 1–256 UTF-8 bytes, with Unicode control characters prohibited.
Case, permitted whitespace (including all-space aliases), and Unicode bytes are
preserved through compilation, facts, evaluation, admission and registry lookup.
The Runtime's typed `InstanceId` continues to identify links and leases. Catalog,
task, pool, server and game identifiers and references retain their own rules.
JSON Schema bounds alias character length; the shared contract/compiler also
enforces the UTF-8 byte limit.

In the Runtime the policy sees an instance's availability only as the
`session.instance.available` instance fact (`contracts/instance-fact-store.md`):
the configuration seeds it, and for an instance on a physical device it holds the
seed only while the instance's latest backend self-check passed: a physical instance
starts unavailable at every Runtime start and after every emulator stop/start/restart
until its connection preparation phase passes (Workflow #317 sc3, replacing sc2's bounded reading).
The policy never reads `backend.selfcheck.*` itself; an instance projected unavailable
receives no dispatch.

Activity sampling uses a ledger-derived seed and `same_round_stable`: the host records the seed once and must reuse the sampled value throughout the same scheduling round. Resampling within a round is invalid.

## Score-Assisted Priority

The predicate gate (trigger, feedback stop, timeline validity, cooldown, placement)
decides eligibility on its own; a score never overrides it. Between that gate and
ranking, the evaluator runs an optional score stage:

- With a `selection` document, every eligible (task, instance) pair becomes one
  selection candidate `<task_id>@<instance_id>` whose fields are `task.priority`,
  `task.strategic_weight_milli`, `task.urgency_milli`, `task.aging_ms`,
  `task.load_cost_milli` (integers), `instance.affinity` (boolean), for a task
  with `reserves` the integers `task.consumes.<pool>`, `task.after.<pool>` and
  `task.reserve_gap.<pool>` of each reserved pool ("Task Reserves" below), and every
  scalar fact the instance can see (its own, its server's and its game's scope,
  the most specific scope per key; timestamps and durations become integer
  milliseconds; record lists are unusable). A fact field carries only a record
  that has not expired at the evaluation instant, by the fact contract's own
  predicate (`now > expires_at` is expired); the `fact` source still reads an
  expired record as `fact_expired`. The document is evaluated once per instance
  over that instance's candidates and its own fact projection, pinned to the
  evaluation's `fact_snapshot_id` and instant, with the selection-policy crate's
  collect option (`evaluate_collect`): the `on_unknown` clauses the document
  declares do not apply at this layer. Only a candidate's score is consumed; a
  candidate a gate rejects carries no score. A document the crate cannot
  evaluate fails with `selection_policy_invalid`.
- A candidate whose gates or terms meet an unknown input is deferred on its own
  (`Deferred`, with no wake: it is evaluated again when the instance facts, the
  catalog or the configuration change) and takes no part in ranking. The other
  candidates of the instance and every other instance are scored as usual, and
  the evaluation succeeds. Its reasons follow the first rule that matches
  (Workflow #384, D2):
  - an input that is not a realistic key gives one `selection_input_unknown`
    (`instance_id=<id> rule_id=<gate or term id> unknown_reason=<reason>`) per
    gate or term that met one. A realistic key is an instance fact key of the
    `resource.` or `inventory.` family, read without a lookup miss; instruction
    keys (`session.`), pack data and the fields `task.*` and
    `instance.affinity` are not, and a reading cannot supply them;
  - otherwise every unknown key gives one `fact_refresh_unbound`
    (`instance_id=<id> fact_key=<key> unknown_reason=<reason>`). A fact field
    whose record has expired reports `fact_expired`; one with no record reports
    `field_missing`. In the Runtime, which hands the evaluator its readers, these
    keys go to the fact refresh instead ("Fact Refresh" below), which gives each
    key its reason.

  `<reason>` is one of `fact_missing`, `fact_expired`, `fact_stale`,
  `fact_low_confidence`, `fact_not_scalar`, `type_mismatch`, `field_missing` or
  `lookup_miss`.
- Every document fact a scored or gate-rejected candidate's evaluation read bounds
  its `facts_fresh_until_unix_ms`: a `fact` source by the earlier of its record's
  `expires_at` and its publication plus the document's `max_age_ms`, a fact field
  by its record's `expires_at`. The evaluation wakes at that bound + 1, as for an
  applied resource target, and admission refuses an intent past it as
  `policy_facts_stale`.
- `EvaluationFacts.priority_offsets` carries manual offsets
  `{task_id, instance_id?, offset_milli, origin: user|agent, observed_at_unix_ms}`;
  an instance-level entry overrides the task-level entry, `|offset_milli|` is
  bounded to 1,000,000, and duplicate `(task_id, instance_id)` pairs are input
  errors. Offsets apply with or without a selection document.
- `base_milli = score_milli (0 when absent) + utility_milli + offset_milli`
  (checked; the slice 5a utility term is 0 without a declared value), and
  `effective_milli = base_milli`, plus an applied resource target's task target
  score `s` (below): `base_milli + s` in `adjust` mode, `utility_milli + s` in
  `override` mode. It is added to the candidate's `total_score` as
  `effective_milli * 1000`, the same unit as the urgency and strategic terms. The
  decision's `rank.effective_milli` carries it (0 for a candidate the stage did
  not score). An overflow fails the evaluation with
  `policy_evaluation_numeric_overflow`.
- Resource targets (Workflow #308 RT-S1b, `contracts/resource-targets.md`): the
  evaluator reads each instance's stored `session.resource_targets` policy from
  the facts before the time-validity projection and resolves every target once
  per instance against the active catalog and the projected inventory. A named
  candidate whose target has a gap `g > 0` is *applied*: it enters the stage even
  without a selection document, value or offset, and `s = min(⌊g·I·u/(S·U)⌋,
  1_000_000)`. `adjust` adds `s`; `override` supersedes the selection score and
  the manual offset with `s`, keeps the utility term and every other ranking
  term, and is never deferred or promoted (its disposition is `none`). Every
  other candidate, including a named one that is satisfied (`g = 0`, which
  releases an override), pending, unmapped, expired or unreadable, keeps its rank
  and disposition byte for byte and only gains reasons. Every admission
  predicate stays in force.
- The cycle thresholds (absolute pair or percentiles) are computed from the
  staged candidates' `base_milli`, never from a target score, so a policy on one
  instance moves no threshold of another. A scored candidate is compared with its
  `effective_milli`.
- The tasks document may declare a catalog-level `priority_selection` block
  `{defer_below_milli, defer_for_ms (1..=86,400,000), promote_above_milli}` with
  `defer_below_milli < promote_above_milli`; it is only allowed next to a
  selection document (`priority_selection_without_document` otherwise). A scored
  candidate with `effective_milli < defer_below_milli` is deferred with reason
  `score_deferred` and wakes at `now + defer_for_ms`, taking no slot of the shared run limits;
  one with `effective_milli > promote_above_milli` is promoted and ranks ahead of
  every other candidate (`score_promoted`). An unscored candidate is never
  deferred or promoted: its effective value is `utility_milli + offset_milli`,
  plus an applied resource target's `s` in `adjust` mode; in `override` mode `s`
  supersedes the offset (the utility term is kept). The reason `score_unknown`
  (value `rule_id`, the gate's id) records why it is unscored, and
  `score_cost_unknown` that its utility term is dropped.
- Every candidate that passed through the stage carries the reason
  `score_computed` with detail `score=<milli|none> offset=<milli> effective=<milli>
  disposition=<deferred|promoted|none>`, followed by ` target=<id>:<mode>:<s>`
  for an applied target. Each reason is a registered code with its values (an
  outcome, Workflow #378 A2-P); its detail is developer text and is never
  persisted. The decision identity is unchanged.
- Resource target reasons follow `score_computed` and `score_unknown` and precede the
  disposition reason, at most three per candidate: one policy-level reason
  (the first that applies of `resource_target_policy_ignored`,
  `resource_target_policy_unreadable`, `resource_target_policy_expired`,
  `resource_target_tasks_unevaluable`), one target-level reason
  (`resource_target_applied`, `resource_target_satisfied`,
  `resource_target_pending` or `resource_target_unmapped`) and, for an applied
  override, `resource_target_override`, each with the target's id as the value
  `resource_target_id`. A candidate that bypasses the stage carries them too.
- An applied target bounds the candidate's `facts_fresh_until_unix_ms` by the
  inventory observation's expiry and the policy's `valid_until_unix_ms`, so
  admission refuses the intent as `policy_facts_stale` once either lapses, and
  wakes the evaluation at `expiry + 1` and `valid_until + 1` when later than now.
  No other target state changes freshness or wakes, and none requests detection.

Without a selection document, without a declared value, without offsets and
without a stored resource target record (a withdrawal counts as none) the stage
is a no-op and every evaluation output is byte-identical to a catalog compiled
before this stage existed, except for the dispatch decision record at the end of
each selected chain ("Dispatch Decision Record" below).

In the Runtime (Workflow #308 slice 4a-2) manual offsets are instance facts
`session.task.<task_id>.priority_offset` (integer milli; instance scope for one
instance, server or game scope for a task-level offset) in the instance fact
store, published through `PublishFacts` by an agent (`agent`/`adapter`) or by a
person: the console (`user`/`ui`) or `actingctl task-offset` (`cli`/`cli`).
Every evaluation projects the active records at its input ledger position into
`priority_offsets` (origin `user` for a person, `agent` otherwise), drops those
whose TTL has elapsed at the evaluation instant, and reports an offset naming a
task the catalog does not declare once per cycle as
`priority_offset_unknown_task` (warning; value `task_id`) without failing; the offsets are part of
the `fact_snapshot_id`. `contracts/instance-fact-store.md` ("Priority offset
facts") owns the key, its validation, the origin exception and the projection.
The `score_computed` reason travels in the reason chain the `policy.dispatch_intent`
payload already carries, and so does the dispatch decision record, in the same
`reasons`; each persisted reason is its outcome (`code`, `category`, `values`), and
no detail is persisted.

The Runtime catalog store accepts the selection document with the other four:
activation stages it as `selection.json` in the immutable generation, records it
as a fifth source in the generation manifest only when present, and reloads it
from there; the stored `catalog_hash` is verified against a recompilation as for
the four documents. An agent proposal compiled against a catalog with a
selection document carries that document unchanged into the proposed catalog;
proposal patches address only the four catalog documents.

## Fact Refresh

Workflow #384 D2-S4 (`rd-1009/d2/model-d2-fact-refresh.md` v1.4.2 §3.3, §3.4,
§3.7, §3.9). A realistic instance fact (a `resource.` or `inventory.` key,
which recognition writes) that a candidate reads and finds unknown never fails
the round and is never scored with an old value: the instance's recognition
maintenance package R_I (`instances[].recognition_package`) is dispatched to
read it, and the candidate waits.

- **Demand.** A candidate raises demand for the unknown realistic keys its
  selection document read ("Score-Assisted Priority"), and a task for those its
  undecided trigger or feedback stop read in exactly its own instance's scope
  (`trigger_unknown` / `feedback_stop_unknown` stay; an instruction key such as
  `session.*`, a server or game scope, a dependency or an outcome raises none).
  A `resource_projection` predicate on a `ledger_fact` pool of exactly the
  instance's scope reads the pool's key: without a usable record it raises
  demand for that key (Workflow #390 POOL-1, "Pool Projection"); a static
  snapshot pool raises none. #386 CF-5: a pool of exactly the instance's scope
  whose capacity fact gives no capacity raises demand for that fact's key
  ("Pool fact references").
  Workflow #411 6438574534 follow-up 2 (since 0.12.26): a task's own placement
  never holds back the demand its undecided trigger or feedback stop raises.
  Whether the reading can be placed is R_I's own gates' to decide (below): an
  unavailable instance holds R_I back, and the task takes the row that gate
  gives it (`fact_refresh_pending` with `gate_code` `instance_unavailable`).
  The task's own load (a heavy-scene slot it would take, a load its host
  cannot take) binds only its own dispatch, never R_I's light one. A task its
  instance cannot run whatever is read (the instance does not provide its
  operation, or its instance override disables it) raises none.
  Only candidates past the closure's positions 1-2 raise demand: on an instance
  an operator pause, a holder, a waiting claim or a takeover cooldown holds as
  the cycle starts, a candidate the selection document deferred carries that
  hold's reason alone (`dispatch_paused_*`, `dispatch_instance_held`,
  `dispatch_instance_cooldown`) and no fact refresh reason, and nothing is
  dispatched there. Workflow #411 ruling 77 A (Alice 10-28; since 0.12.25): a
  task whose undecided trigger or feedback stop read such a key there carries
  the same hold's reason after its `trigger_unknown` or
  `feedback_stop_unknown`, as its last reason and with the hold's wake, and no
  fact refresh reason; its decision's detection suggestions name the keys it
  waits for. Nothing is read while the hold lasts; the first round after it
  ends raises the demand as above. A reader the Runtime's admission holds (a
  paused pair) is R_I's own limit below: the task names the key and the
  package under `fact_refresh_exhausted`.
- **No reader.** A key no reader can serve gives `fact_refresh_unbound` with
  `reader_problem`: `disabled` (`scheduling.fact_refresh.enabled` is off),
  `package_unbound` (the instance has no kept recognition maintenance package),
  `package_unreadable` (the Runtime start could not read its package's marks)
  or `key_not_marked` (its package has no mark for the key).
- **The reader's candidate.** R_I is requested for the keys its marks write:
  its decision carries one `fact_refresh_requested` (`instance_id`, `fact_key`,
  `unknown_reason`, and `rule_id` when a selection document rule read it) per
  key, and so does its dispatch's reason chain, which is how the run learns its
  requested key set (it reads only those keys' marks; a `linear_steps` package
  runs only the segments that reach them). Its decision and dispatch use the
  reserved task id `_fact_refresh`, operation and procedure `_fact_refresh`
  (a catalog id never starts with `_`), the instance's activity profile, a light
  load and a reserved run time of 60 s; its loop budget never binds. It takes
  the highest rank (catalog rank plus manual offset) of the candidates it
  unlocks and competes with the instance's other candidates; one winner per
  instance and the shared run limits ("Shared Run Limits") apply as usual.
- **Its own gates**, before the allocation: an instance without an activity
  profile (a limit, `activity_profile_missing`); its failed runs in a row since
  its last success reaching `max_consecutive_failures` with its latest run in
  the current activity day, and its dispatches in the current activity day
  reaching `daily_limit` (limits, waking when the activity day's window ends);
  an unavailable instance, a host that cannot take its load (for now); and
  `task_cooldown_active` until both `min_interval_ms` after its latest dispatch
  and, after a failed run, `failure_backoff_ms` after that run's end (for now).
  Past these, #405 IDLE-1P: on an instance the Runtime's idle action closed as
  the cycle starts, R_I waits, with no wake, for the instance's next natural
  wake unless a candidate it reads for is due (its trigger true): its decision
  carries `instance_idle_action` and the candidates `fact_refresh_pending` with
  that `gate_code` (for now), so a reading alone never brings a closed game back
  ([runtime-config.md](../runtime-config.md), "The `idle` section"). When one
  of the gates above holds R_I there too, its decision keeps that gate's
  reasons and wake and adds `instance_idle_action`, and the candidates keep
  the row that gate gives them (its `gate_code`, or `fact_refresh_exhausted`)
  but take no wake of R_I's, neither as their wake nor as
  `next_eligible_unix_ms`.
  In the allocation the Runtime's admission defers it like any candidate; a
  closed activity window or a spent activity budget, a spent task budget and a
  paused pair are limits, everything else is for now.
- **Each affected candidate's reason**, per key R_I reads, by the first row that
  matches; the candidate is deferred, never failed, and the round always
  succeeds:
  1. R_I is selected this round: `fact_refresh_pending` (`instance_id`,
     `fact_key`, `unknown_reason`, `package_id`), no wake;
  2. a limit holds R_I back: `fact_refresh_exhausted` (`instance_id`,
     `fact_key`, `package_id`, `refresh_limit` = `daily_limit`,
     `failure_pause`, `activity_window` or `placement`; `gate_code` when the
     Runtime's admission named the gate; `next_eligible_unix_ms` when known);
  3. R_I's latest dispatch settled successfully at or after L, the instant this
     Runtime finished loading the instance facts, and the key is still unknown
     because of it (no record since that dispatch, or one recognition could not
     use: too little confidence, not a scalar, another type):
     `fact_refresh_unresolved` (`package_id`; `next_eligible_unix_ms`, R_I's
     next eligible time);
  4. anything else: `fact_refresh_pending` with `gate_code` (the code that
     deferred R_I), `failure_code` (the code R_I's latest run failed with, when
     it failed) and `next_eligible_unix_ms` when the gate names one.
- **Settings.** `scheduling.fact_refresh.enabled`, `min_interval_ms`,
  `daily_limit`, `failure_backoff_ms` and `max_consecutive_failures`
  ([runtime-config.md](../runtime-config.md)), read at every cycle; they are not
  part of any `detection_budget`.
- **Dispatch.** R_I is a policy dispatch of the instance's one queue (claim kind
  `policy_dispatch`): the Runtime admits, budgets and settles it like a catalog
  task's, under the Runtime-owned task spec, and runs the kept package by its
  location. Before any input the run checks that the package still qualifies as
  a reader; one that no longer does (a designated operation, no marks,
  `recognize_only`, a stability termination) fails the run with
  `fact_reader_binding_invalid` and its `reader_problem`, which counts as one of
  R_I's failures. The readings are published as instance facts before the run's
  terminal, and the next round scores the candidates that read them. The fact
  refresh writes nothing to the ledger itself: R_I's dispatch, run and readings
  are recorded as every policy dispatch, run and reading is.
- **Transitions** (Workflow #384 D2-S5, D2 model §5.1 MP-3, §5.3 A-5, §6 S5).
  At each evaluated cycle's yield point a bounded recorder reads the reasons of
  the waiting candidates (the four codes above) and turns their changes into
  transitions, per (instance, key): the key starts to be waited for (its
  reason), the reason's code changes (the new reason), the key is no longer
  waited for (`fact_refresh_resolved`, info: `instance_id`, `fact_key`, and the
  `package_id` that served it), and one wait has lasted
  `scheduling.fact_refresh.overdue_ms` (`fact_reader_overdue`, warning:
  `instance_id`, `fact_key`, `elapsed_ms`, `package_id`; once per wait, only
  while a package serves the key, never while the setting is unset). The same
  input again gives no transition; an instance the closure's positions 1-2 hold
  raises no demand, so its keys keep their state and their wait goes on. Cycles
  and transitions are numbered from 1 per owner epoch, one by one, never reset.
  The recorder holds one entry per waited-for (instance, key), so at most the
  instances times the keys their candidates read. It only produces transitions:
  no decision reads it, and it writes nothing to the ledger; actingd writes each
  transition as one developer line (its code at its category's level, its
  values, `cycle_seq=<n> transition_seq=<n>`) and one `OUTCOME actingd:` line in
  the same write, and the ledger observer's G5 gate copies them from AO-4 on as
  `policy.fact_refresh_transition` ([v012-register.md](../v012-register.md) §1.2
  D1). `selection_input_unknown` names a rule, not a key, and is no transition.
- **Status read.** `FactRefreshStatus` answers what the recorder holds after the
  latest cycle: `{cycle_seq, transition_seq, instances: [{instance_alias,
  demands: [{fact_key, outcome, package_id?, since_unix_ms, overdue}]}]}`, from
  memory, appending nothing; `actingctl fact-refresh` prints it and `ac_overview`
  answers it as its block `fact_refresh`
  ([runtime-interface.md](../runtime-interface.md)).

## Remedies (Workflow #411 B15)

Written ahead of its code (B15 R0); slices R2 (the evaluator's plan) and R4 (the host) implement
it. [remedies.md](../remedies.md) is the contract: a policy run that succeeds on a mapped key whose
mapping declares a `remedy` opens a remedy round for its (task, instance) pair, and the evaluation
plans the round from the remedy table, the pair's recent runs, the recorded stops, the instance's
target record `session.remedy.choices` (an overlaid instance fact) and the configuration rows.

- **Trigger override and promotion.** Beside the immediate rerun (R22, `failure_retry_immediate`)
  and an owed rerun (`policy_rerun_owed`), a third Runtime-made trigger override exists: an allowed
  provider of the blocked pair's need is treated as triggered and past its cooldown and promoted
  ahead of the instance's other candidates, with one `policy_remedy_requested` reason per blocked
  pair it serves (at most 16); after the remedy is served, the blocked pair is treated so once,
  with `policy_remedy_retry`. Placement, the feedback stop, the deferrals, budgets, activity
  windows, one winner per instance and the shared run limits still apply. Between several requested
  providers the ranking of "Score-Assisted Priority" decides, unchanged.
- **Hold and stop.** While its round is open the blocked pair is deferred with
  `policy_remedy_waiting` at position 8a of the order of deferrals
  ([scheduling-blackout.md](../scheduling-blackout.md)); a stopped round defers it there with
  `policy_remedy_unchosen`, `policy_remedy_unavailable` or `policy_remedy_exhausted` until the
  stop is lifted. For the clock-slot records below, the three stop codes are persistent gates
  (`clock_slot_refused`) and `policy_remedy_waiting` is a waiting gate (`clock_slot_deferred` only
  once the window passed).
- **Run parameters.** A dispatch of a task whose package declares parameters carries
  `policy_dispatch_parameters` with the values the run reads as `session.dispatch.parameter.*`.
  Workflow #411 B8: a dispatch of a V2 task with `dispatch_parameters: true` carries, on the same
  reason and whether or not remedies are on, its schedule values (`next_occurrence_unix_ms`,
  `window_end_unix_ms`, `last_in_window`, `last_daily_slot`, `daily_remaining`), which the run
  reads as `session.dispatch.schedule.*` ([remedies.md](../remedies.md), section 4).
- **Records.** The reasons travel in the existing dispatch reason chains; the stop, lift and expiry
  are `goal_missed` planning signals with the `signal_id` prefix `signal:remedy:`; an intent whose
  round closed between evaluation and admission is refused with `policy_remedy_round_closed`. The
  evaluation's output keeps its shape, and with no remedy declared every evaluation is
  byte-identical to one without remedies.

## Chain Handover (Workflow #411 B41)

A chain step that hands over on its page goes first (coordinator triage Q15; register §1.157).

- **Handover.** On an instance, a round has a handover when a candidate of the round, the
  *successor*, follows a predecessor on the page the instance rests on: its trigger names
  `dependency_completed` (outside a `not`, accepting `succeeded`) of the predecessor; the
  predecessor's latest run on the instance succeeded; the instance's `task.page` runtime fact (the
  page its latest recognition matched) was recorded between that run's admission and its end, so
  nothing recognized another page since; and the entry page set of the successor's package holds
  that page (its declared `entry_page` or entry page set, or the entry its entry gate checks, each
  without the game prefix). The Runtime reads the package its procedure binding locates, only for
  such a candidate; a package it cannot read holds no page, and its run's own admission refuses
  it. A takeover, an unreachable ADB or an emulator restart drops `task.page`, so no instance has a
  handover until a run recognizes a page again. The first such candidate in rank order is the
  instance's successor.
- **The successor goes first.** It is allocated before every candidate of its instance that ranked
  ahead of it. While it is dispatched, each of those waits with `policy_chain_handover_waiting`
  (info; `instance_id`, `task_id` the successor, `page_id` the page), its decision deferred and
  with no wake of its own: a fact refresh reading too (its decision ends with that reason, and
  each candidate it reads for gets `fact_refresh_pending` with `gate_code`
  `policy_chain_handover_waiting`). The Runtime does not predict where another package's run ends,
  so every candidate that ranked ahead waits, not only those whose entry or prerequisite layer
  goes home. The candidates ranked behind the successor lose to it with
  `instance_already_selected` as before.
- **The waiting work keeps its own due time.** It runs after the chain step. The successor's
  random delay (`dispatch_random_delay_waiting`, position 9 of the order of deferrals) is part of
  its dispatch: while the successor waits for its planned start the candidates it went before keep
  waiting with `policy_chain_handover_waiting` (coordinator ruling 10-21). When any other gate
  defers the successor in the round (an admission gate at positions 1 to 8a, or a shared run limit),
  the candidates it went before take their turn in the same round, in rank order. When the
  successor is no longer a candidate (withdrawn, refused, no longer due) there is no handover.
- **Exception.** A successor whose own trigger or selection document reads a key the fact refresh
  reads is deferred for that key and is no candidate: the refresh goes first, as before. Its run
  then starts wherever the refresh left the instance, and its entry recovery decides as before.
- **Records.** No record is new: the waiting candidates' decisions carry the reason, the
  successor's dispatch decision record lists them with that `why`, and the not-run explanation
  (`ac_schedule`) shows it as their `why`. Without a resting page every evaluation is the one
  before B41.

## Ties and chain heads (SS-2)

Since 0.12.32 (Alice's ruling 104 points 2, 4 and 5, approved 2026-10-30, Workflow #386
comment 6523113979; register §1.222). Point 1, two priority tiers, is the resource side's
(RS-8; [standard-pack-schedule.md](../standard-pack-schedule.md), "Two priority tiers" warns of a
third). Point 3, net benefit within a tier, is SS-2b's ("Net benefit within a tier" below); for a
catalog that marks no task the rank value (`total`, "Dispatch Decision Record" item 1) is the
one of before.

- **Chain link.** A task's trigger *requires* a task when it names `dependency_completed` of
  it, accepting `succeeded`, reached from the trigger's root only through `all`: the trigger
  cannot be true unless that task's latest run on the instance ended in an accepted state. A
  dependency under an `any` is one alternative among others, an order and not a continuation,
  and one under a `not` the opposite: neither is a link. A task that names itself is not its own
  link. ("Chain Handover" also counts the `any` branches: it only decides who goes first on a
  page the instance already rests on.)
- **The head's run window.** A candidate continues a task it requires while, on its instance,
  that task's latest run succeeded and the candidate has not served it: it has no dispatch since
  that run's dispatch, or its own latest run since did not succeed (the test of
  `since_own_run`, "Condition edges"). Of several such tasks, the one with the latest dispatch
  (then the smaller id). Its *chain head* is found by walking up: from the task it continues, to
  a task that one requires whose latest run succeeded and was dispatched at or before its own
  latest dispatch, and so on, never visiting a task twice. The other task is read as a gate
  reads another task (after the timeline resets), the candidate itself from its own last result
  (Workflow #411 P19).
- **Chain inheritance.** In that window the candidate ranks with its chain head's `priority`
  and `strategic_weight_milli` (the head's `instance_overrides` entry for the instance applied)
  instead of its own; every other term (aging, urgency, contention, the score stage, its resource
  term) stays its own. Outside every window, it ranks with its own. Only the task layer reads
  this: the pool weights and resource terms read no task state (one-way rule, SS-1). The fact
  refresh lends a candidate's rank as before, inheritance included.
- **Tie-break (point 4).** Of candidates equal in `promoted`, `total` and `affinity`, the one
  closer to losing its eligibility goes first, by a key computed at the evaluation instant from
  the inputs its gates read, in integer arithmetic:
  - *time left*: for each `timeline_active`, and each `clock` with `missed`, that the trigger
    requires (through `all`), the milliseconds until it stops being true (the event window's end,
    the occurrence's grace end); the least of them;
  - *runs left*: for each threshold its own runs lower, the runs, this one included, that still
    meet it, by the task's declared consumption `c` of the pool: a required `resource_projection`
    on a pool it consumes, or a required `fact` on the reading key of a pool it consumes (same
    scope), compared `greater_than_or_equal` T (`⌊(x − T)/c⌋ + 1`, 0 below T) or `greater_than`
    T (the same with T + 1), T a number or a fact reference plus its offset; and each of its
    `reserves` on a pool it consumes (`⌊(x − at_least)/c⌋`, 0 below it), x the pool's projection
    or the fact's integer as the gate reads them; the least of them;
  - *chances*: the fewer of the runs left and the runs that can still start before the time left
    ends (`⌈time left / E⌉`, E the candidate's expected duration).

  A key with a time left or runs left is `bounded`; one without either is `none`; one that meets
  an unknown input (no pool value, a fact read as unknown, an evaluation error) is `unknown` and
  never fails the evaluation. Bounded keys go first, by fewer chances, then less time left, then
  fewer runs left. Then (the fallback, point 4) the key with more *conditions* (the trigger's
  required nodes, an `any` or a `not` counting one, plus its `reserves`); then (point 5)
  *declared order*, the task's position in the catalog's task list (a composed catalog keeps the
  rules' order of `schedule.json` per instance), the earlier first. The Runtime's own fact refresh
  task has no key (`unknown`, no condition) and comes after every declared task in a tie. The
  deterministic `tie_breaker` hash comes only after, for one task's candidates on different
  instances.
- **Records.** No record is new: `rank_breakdown` and the ranked `decision_candidate_not_selected`
  details carry `chain_head` and the tie-break key ("Dispatch Decision Record"); the developer
  text is never persisted. Without a chain window and without a tie, every evaluation, its ranks,
  reasons and details included, is the one before SS-2. No catalog member, configuration key or
  outcome code of the evaluation is added; a catalog serializes and hashes as before.

## Net benefit within a tier (SS-2b)

Since 0.12.32 (Alice's ruling 104 point 3, approved with 104 on 2026-10-30 and checked by the
research channel, Workflow #386 comment 6523148251; ruling 116 of 2026-10-30: business dispatch
waits until its values exist, 「当业务有了数据之后直接写进任务包」; register §1.225). No configuration
key and no per-instance mode: a deployed Runtime judges every marked task.

- **Who is judged.** A V2 task may carry `benefit_judged: true` ([v2/README.md](v2/README.md));
  the standard-pack composer writes it on every rule whose package is not maintenance-role
  ([standard-pack-schedule.md](../standard-pack-schedule.md), "Classification"). Such a task is a
  *business* task. Its candidate is *judged* unless it ranks with a chain head ("Ties and chain
  heads") or is a manual request: a task whose trigger requires an instruction, a `fact` or
  `fact_time_reached` predicate on a key of the `session.` family reached from the root only
  through `all` (design `rd-1029/design-sch-state.md` §4.9). Every other candidate is unjudged.
- **The net benefit** of a judged candidate `k`, in milli, the exact sum floored once:
  `B(k) = Σ_p produce(k,p)·W(k,p)/Q_p − Σ_p consume(k,p)·P(k,p)/Q_p`, with checked 128-bit
  intermediates. `produce(k,p)` is the resource layer's own `r(k,p)` (`amount` or v2
  `expected_amount_milli`; [resource-targets.md](../resource-targets.md), "Resource weights (v2)"),
  `W(k,p)` its effective weight `B_p + Γ` (the base weight plus a covering goal's shortfall
  weight), `consume(k,p)` the task's declared consumption of the pool (its `consumes` amounts, as
  the reserve gate reads them) and `Q_p` the pool's `valuation.scale`. `P(k,p)`, the cost of
  consuming pool `p`, is `W(k,p)`, discounted for a regenerating pool between the task's reserve
  line on it (its `reserves` `at_least`, 0 without one) and its capacity: `P =
  W·clamp((cap − cur)/(cap − reserve), 0, 1)`, `cur` the pool's projected amount as
  `resource_projection` reads it: 0 at capacity, `W` at the reserve. A pool that does not
  regenerate costs `W`. Nothing here writes a pool weight (the one-way rule, SS-1).
- **Unknown is never 0.** A judged candidate whose task declares no `produces` and no `consumes`
  entry, an entry whose `amount` is the pending marker, a pool it touches whose `valuation` is the
  pending marker or absent (for the instance), or a regenerating pool it consumes whose reading is
  unknown or expired has an unknown `B`: the known part is never added up. It is not dispatched:
  deferred with `benefit_unknown` (info; `benefit_gap` `no_effects`, `entry_pending`,
  `valuation_pending`, `valuation_missing` or `reading_unknown`; `pool_id`, and `fact_key` with
  `unknown_reason` for an unknown reading of a fact), no wake of its own. An unknown reading also
  asks the fact refresh for it ("Fact Refresh"), as a predicate on it would; that candidate's
  decision then ends with the refresh's reason.
- **Not positive.** A judged candidate whose known `B ≤ 0` is not dispatched: deferred with
  `policy_benefit_nonpositive` (info; `benefit_milli`, `expected_duration_ms`); a regenerating
  pool it consumes wakes the evaluation at its next step, its cost falling as it fills.
- **Ranking, per instance** (catalogs that mark a task; "Dispatch Decision Record" item 1):
  `promoted` first, as before; then the tier, the maintenance tier (every unmarked task, and the
  Runtime's own reading) before the business tier (the marked tasks), as ruling 104 point 1 puts
  maintenance high and business normal; within a tier the unjudged candidates first, in the order
  of before (`total`, then `affinity`, then the tie-break), then the judged candidates by their
  net benefit per expected millisecond `B ÷ E`, the higher first, `E` the duration the utility
  term divides by (the pair's expected duration E, else its last settled run's, else the declared
  one; at least 1), compared exactly as `B·E' > B'·E`; then `affinity` and the tie-break of
  "Ties and chain heads" on integer equality. The handwritten `priority` no longer orders judged
  candidates of one tier. The reserve gate takes each instance's candidates in this order.
- **Where it sits.** The gate is part of the score stage: it follows the stage's own deferrals
  and precedes the reserve gate, so a candidate it holds takes no reserve room, and it comes before
  the closure's positions ([../scheduling-blackout.md](../scheduling-blackout.md), "Order of the
  deferrals"), as `score_deferred` does. The idle plan counts a decision it holds neither due nor
  as a wake of its own.
- **Records.** No ledger record kind is new. A judged winner's `rank_breakdown` carries
  `benefit_milli` and `expected_duration_ms` as values and ends its detail with ` benefit_milli=<B>
  expected_duration_ms=<E>` (a ranked judged candidate of the record does too); the decision
  record and the not-running row name the new reasons as `why`.
- **No mark, no change.** A catalog without `benefit_judged` keeps its hash and decides byte for
  byte as before: no candidate is judged and the order is the one of before.

## Task Reserves (Workflow #411 B16 part 2)

A V2 task may declare `reserves: [{pool_id, at_least}]`: the least amount, in the pool's real
units, that each named pool must still hold after a run of the task ([v2/README.md](v2/README.md)
gives the member's rules). The reserve is a hard gate, never a score penalty (register §1.167).

- **What a run leaves.** In the score stage ("Score-Assisted Priority" above) each candidate of
  such a task gets, per reserved pool, `after = projected − consumes`: the pool's projected
  amount at the evaluation (as the `resource_projection` predicate reads it) less the task's
  declared consumption of the pool (the sum of its `consumes` amounts for it; 0 when it declares
  none). The selection document reads them as the integer fields `task.consumes.<pool>`,
  `task.after.<pool>` and `task.reserve_gap.<pool>` (`after − at_least`); the last two only
  while the pool's amount is known. A catalog with reserves runs the score stage even without a
  selection document, value, offset or resource target; every candidate without reserves keeps
  its rank, its reasons and its fields.
- **The gate.** After the stage, on each instance, the ranked candidates are taken by
  `effective_milli`, the higher first (a tie in allocation order). A candidate with reserves is
  not admitted this round when, for one of its reserved pools:
  1. the pool's amount is unknown (no usable reading or snapshot, or, #386 CF-5, a capacity fact
     that gives no capacity), so the reserve cannot be shown to hold;
  2. `after < at_least`;
  3. `after`, less the declared consumption of every other task whose latest dispatch on the
     instance is at or after the pool's latest observation (a reading that does not show that
     run yet), is below `at_least`;
  4. that amount, less the declared consumption of a candidate of the instance taken before it
     and not held, is below `at_least`: of two tasks that each fit alone but not together, the
     higher `effective_milli` runs this round, whatever their priorities. SS-2b (since 0.12.32):
     in a catalog that marks a task `benefit_judged` the candidates are taken in the allocation
     order instead (the tier, then `B ÷ E` for the judged ones; "Net benefit within a tier"),
     and a candidate the benefit gate holds is not among them.

  Its decision is deferred with `resource_reserve_insufficient` (info, `ladder_class` none;
  `pool_id`, `reserve_at_least`; `amount_after`, what the pool would hold after it, absent in
  case 1; `task_id` in cases 3 and 4, the task whose consumption goes first), and it takes no
  part in the allocation. A regenerating pool wakes the evaluation at its next projection step;
  otherwise the next reading (FactsChanged) brings the next evaluation. A candidate without
  reserves is never held, and its consumption still counts against the others.
- **Scored again after the refresh.** The other task of such a pair waits while the pool's
  latest reading predates the winner's dispatch (case 3), and is scored again on the first
  reading or adjustment taken since. A pool that is never read again after the winner keeps it
  waiting; a package that consumes a reserved pool should read or adjust it at its end
  ([resource-readings.md](../resource-readings.md)).
- **Several instances on one pool.** The gate counts the candidates and dispatches of the
  candidate's own instance only. Instances that share one pool are Workflow #367's
  multi-dispatch and are not handled here.
- **Records.** No record is new: the reason travels in the decisions and, when a dispatch
  decision record lists a held candidate, as its `why`. Without `reserves` every evaluation is
  the one before B16 part 2, and the catalog keeps its bytes and its hash.
- **Spend values (SS-6, since 0.12.31; register §1.218).** A selected dispatch of a task with
  reserves on pools it consumes carries, on `policy_dispatch_parameters`, each such pool's
  allowance (the gate's own available amount, its projection less the consumption step 3
  counts as not shown yet, less `at_least`, at least 0), and a run cap from them (at most 1000);
  a task that produces a pool an active goal covers also carries the goal's gap
  ([../remedies.md](../remedies.md), section 4, "Spend values"). An allowance below the task's
  declared consumption of the pool, 0 included, is never dispatched: the gate holds it.

## Shared Run Limits (Workflow #427-B)

Since 0.12.22 (Workflow #427 plan r2 point 5, approved whole by Alice on 2026-10-28) the instances
of a host share only what is really shared, so that five instances run like five players once
their runs go on in parallel (#427-A):

- **No virtual deduction in the round.** The evaluator no longer charges a chosen dispatch's CPU,
  GPU and IO load against one host budget for the rest of the round, and there is no budget pool
  per instance either. `host_budget_deferred`, the reason that deduction gave, is retired.
- **Placement stays.** A task whose own load (its `load_profile`, or its instance override's)
  exceeds its host's `cpu_available_milli`, `gpu_available_milli` or `io_available_milli` as the
  round starts is blocked at placement with `host_capacity_insufficient`, as before.
- **Heavy-scene slots.** A heavy candidate (`performance.load.heavy_threshold_milli`) takes one of
  its host's heavy-scene slots, `performance.pacing.heavy_dispatch_limit.normal`, or `.paused`
  while the performance controller is above Normal. The slots in use are the heavy policy
  dispatches admitted and not yet settled (running, or admitted and not started; any instance)
  plus the heavy ones chosen earlier in the round. Each dispatch counts once: the controller's
  count, capped at the limit, and the policy host's own count of its admitted dispatches name the
  same dispatches, and the larger is used, so under pressure the real number is reported. A
  refused admission was never admitted and a settled (completed, failed or cancelled) dispatch is
  no longer, so either frees its slot; a candidate an admission gate defers in the round takes
  none. When the admitted ones alone take every slot, a heavy candidate is blocked at placement
  with `heavy_scene_budget_exhausted`; when the round's earlier choices fill them, it is deferred
  with `heavy_scene_budget_deferred`.
- **At most so many runs** (optional). `performance.pacing.max_concurrent_runs` (an integer from 1,
  no default; [runtime-config.md](../runtime-config.md)) limits the policy dispatches admitted or
  running across every instance plus those chosen earlier in the round. A candidate past it is
  deferred with `policy_concurrent_runs_deferred`. Without the key there is no such limit; `1`
  runs at most one policy dispatch at a time across every instance, the fallback should parallel
  runs misbehave. It is checked before the heavy-scene slots and holds every candidate, the
  recognition maintenance package's too.
- **The numbers.** The three reasons carry `used` (the slots taken), `limit_count` (the limit),
  `admitted_count` (taken by dispatches admitted or running) and `round_selected_count` (taken
  by dispatches chosen earlier in the round), so the explanation reads, for example, heavy scene
  2/2 or concurrent runs 3/3. A `heavy_scene_budget_exhausted` has `round_selected_count` 0; its
  `used` may be above `limit_count` when the limit fell under pressure.
- **Pacing modes.** The limits do not depend on `performance.pacing.mode`: `observe` (which never
  slows anything down; the release default before 0.12.31, `pace` since, Workflow #429 R-2), `pace`
  and `full` bind them alike, so under `observe` they are the protection.
- **Online (Workflow #429 R-2, since 0.12.31).** `heavy_dispatch_limit.normal` / `.paused` and
  `max_concurrent_runs` are settable and read from the configuration in effect at every
  evaluation: a change applies from the next round, and no dispatch admitted or running is cut.
- **Where.** The Runtime reads the admitted dispatches from its dispatch journal at each evaluation
  (`RunSlots`, `evaluate_with_run_slots`); the library's other entry points have none beyond the
  snapshot's `active_heavy_dispatches` and no maximum. Nothing is persisted: the reasons travel in
  the decisions, the schedule explanation (`why` keeps its values) and, when a dispatch decision
  record lists such a candidate, as its `why`.

## Application Update Hold (UPD-1)

Since 0.12.30 (design `rd-1029/design-upd1.md` §4, §6; Alice's rulings 95 and 96 of
2026-10-29; register §1.215). When any run, the readings refresh (fact refresh) and the startup
package included (register §1.215 UPD-16), meets the game's "client out of date" dialog (an
`interrupts.json` page with `result: "application_update_required"`,
[package-reference.md](../package-reference.md)), the Runtime records the instance fact
`health.application.update_required` true ([instance-fact-store.md](../instance-fact-store.md),
"Application update facts") and the instance waits for its update:

- **The hold.** Position 4a of the eligibility closure
  ([scheduling-blackout.md](../scheduling-blackout.md), "Order of the deferrals") defers every
  candidate of that instance with `application_update_pending` (`instance_id`, `fact_key`) and no
  wake, except a store update task: a task whose procedure's bound package declares
  `store_update`. The fact refresh is held too (`fact_refresh_pending`, `gate_code`
  `application_update_pending`). The hold does not depend on the ranking (fix round 3): a
  candidate behind the store update task that took the instance, behind the shared run limits or
  held by a chain handover is deferred `application_update_pending` too, never left eligible
  with `instance_already_selected`. Other instances are not held (#419). Nothing in a catalog
  names the hold: it is the Runtime's, so every rule of the game is held without a change.
- **The store update rule.** The game root's `schedule.json` writes one rule per server for its
  store update package whose trigger is the typed `fact` predicate
  `health.application.update_required eq true` (`max_age_ms` null), with its own `cooldown_ms`,
  the time before it looks again when the store has no new version yet
  ([standard-pack-schedule.md](../standard-pack-schedule.md)). The rule passes position 4a and
  every other gate as any task does.
- **After the update.** The store update package's successful run (with or without an update)
  records the fact false before its terminal; the evaluation it wakes lets the instance's
  candidates through again, the runs the dialog interrupted first, as owed reruns
  (`policy_rerun_owed`, "Condition termination and owed reruns" of
  [policy-suspension.md](../policy-suspension.md)); the rest follow their own triggers. The path
  makes no startup claim. A failed store update run leaves the fact true and the instance held;
  its task fails as any task does.
- **Explanation.** `actingctl schedule` and `ac_schedule` show `application_update_pending` as a
  held candidate's `why`; `ac_facts --instance` shows the fact and the run that wrote it. The idle
  plan counts a held decision neither due nor as a wake, so it does not wake the instance for it.
  The gate only sees candidates, so (fix round 1) the idle plan reads the waiting instances and
  the store update tasks of the same evaluation and counts no decision of a waiting instance but
  the store update task's, whatever its reason: a task in its cooldown or before its clock
  trigger, and the fact refresh, wake no waiting instance; the store update task's own cooldown
  still does.
- **No configuration key.** The store update rule's cadence is its `cooldown_ms`; the hold and
  its order are code.

## Dispatch Decision Record

Workflow #308 RT-S1c. Every selected candidate's reason chain ends with the
record of why it was dispatched, appended after `placement_selected` and
`rank_assigned`; the selected decision's `reasons` stay equal to its chain. No other
decision gains a reason. The details below are developer text, kept only in the
in-memory decision; the dispatch events persist each reason's outcome, its code with its
values. Why a listed candidate did not run is persisted as values (Workflow #411 A26,
since 0.12.5): `decision_candidate_not_selected` carries `why`, `decision_state` and
`eligibility`, and `decision_record` carries `omitted_why`. The ranking terms of the
details (`total`, `behind_by`, `priority`, … `tie_breaker`, and `rank_breakdown`'s) have
no value keys and stay developer text.

1. `rank_breakdown`: `total=<t> base=<b> priority=<p> aging_ms=<a>
   strategic_milli=<w> urgency_milli=<u> contention=<c> effective=<e>
   score=<s|none> utility=<ut> offset=<o> target=<s|none> target_id=<id|->
   mode=<adjust|override|-> superseded=<0|1> promoted=<0|1> affinity=<0|1>
   tie_breaker=<u64> saturated=<0|1> lead=<l|none>`.
   - `base` is `total_score` before the score stage,
     `p*1_000_000 + a + w*1000 + u*1000 - c` (`c` is the contention penalty,
     load cost x contention basis points x 100), and `total = base + e*1000`
     unless `saturated=1`.
   - `score`, `utility`, `offset` and `target` are the score-stage terms
     (`none`/0 for a candidate the stage left untouched); `target_id` and `mode`
     name an applied resource target, `superseded=1` an applied override.
   - The ranking order is `promoted=1` first (ahead of every other candidate),
     then `total` descending, then `affinity=1`, then (SS-2, since 0.12.32) the
     tie-break of "Ties and chain heads" above: the candidate closer to losing
     its eligibility, then the one with more conditions, then the one declared
     first; then `tie_breaker` ascending, the lower winning, which now only
     orders one task's candidates on different instances (they never compete for
     one instance). `saturated=1` when a saturating sum clipped the total (the
     unsaturated 128-bit sum differs). SS-2b (since 0.12.32): in a catalog that
     marks a task `benefit_judged`, the tier comes after `promoted` and judged
     candidates rank by `B ÷ E` within it ("Net benefit within a tier"); a
     judged winner's breakdown then ends with ` benefit_milli=<B>
     expected_duration_ms=<E>`, and its outcome carries both as values.
   - SS-2 (since 0.12.32): ` chain_head=<task id>` follows when the candidate
     ranks with its chain head's priority and strategic weight; and when
     `lead=0` (another ranked candidate of the instance has the same total, so
     the tie-break decided) the winner's key follows: ` tie=<bounded|none|unknown>
     chances=<n|none> time_left_ms=<n|none> runs_left=<n|none> conditions=<n>
     declared=<n|none>`. A candidate in no chain window and without a tie keeps
     the detail of before byte for byte.
   - `lead` is `total` minus the highest total among the instance's other
     ranked candidates, `score_deferred` ones included (`none` without one). It
     is negative when the winner was promoted above a higher total, or when a
     higher-total candidate was deferred by score, a shared run limit or the
     admission state; it can be positive while a better-ranked (promoted)
     candidate was deferred.
2. `decision_record`: `targets=<active:<sha>@<applied_at>|expired:<sha>@<applied_at>|unreadable:<code>|none>[ ignored=<server|game>@<ms>]
   related=<n> shown=<k> omitted=<m> omitted_why=<code>:<count>,...`.
   - `targets` is the instance's stored resource target policy, its version
     being `policy_sha256@applied_at` (`contracts/resource-targets.md`). The
     instance, catalog, input ledger position and fact snapshot are fields of
     the dispatch payload already.
   - The related candidates are every other decision of the same instance in
     the same evaluation. `omitted_why` counts the unlisted ones by their last
     reason code in ascending code order, `none` when none is omitted; the codes
     that would take the detail past 1010 bytes fold into `+<n>more` (`n` codes).
   - Value `omitted_why` (optional): the same counts, one record `{why, count}` per
     code in ascending code order, every code (no fold; a records value keeps 64 and
     adds `omitted_why_total`); left out when none is omitted.
3. `decision_candidate_not_selected` (values `task_id`, `why`, `decision_state`,
   `eligibility`), one per listed related candidate, at most eight: those that were
   ranked first, by total descending, then the others, each by task id.
   - `why` (a code) is the `why` of the detail, `decision_state` (vocabulary
     `decision_state`) its `state`, and `eligibility` (vocabulary `eligibility`:
     `true`, `false`, `unknown`) the candidate's eligibility, which the detail of a
     candidate that was not ranked prints.
   - Ranked: `why=<code> state=<state> total=<t> behind_by=<winner total - t>
     priority=… aging_ms=… strategic_milli=… urgency_milli=… contention=…
     effective=… score=… utility=… offset=… target=<s|none> promoted=<0|1>
     tie_breaker=<u64>` (the lower `tie_breaker` wins a tie the SS-2 tie-break
     leaves), then, SS-2, ` chain_head=<task id>` as in `rank_breakdown` and,
     when `behind_by=0`, the candidate's own tie-break key, so the two keys
     side by side show why the winner went first.
   - Others: `why=<code> state=<state> eligibility=<true|false|unknown>`.
   - `why` is the candidate's last reason, the one that settled it
     (`instance_already_selected`, `heavy_scene_budget_deferred`,
     `policy_concurrent_runs_deferred` ("Shared Run Limits" above; before 0.12.22 also
     `host_budget_deferred`), an admission eligibility code,
     `score_deferred`, `task_cooldown_active`, `trigger_false`, …); it is
     never `none` ("Why A Candidate Does Not Run" below).

A stored `actingcommand.resource-targets.v2` policy is named
`targets=active.v2:<sha>@<applied_at>` or `expired.v2:<sha>@<applied_at>`; while
it is active the target fields carry the candidate's resource term instead
(`target=<R>`, `target_id=<override id|*>`, `mode=<override|adjust>`) and
`rank_breakdown` ends with ` offset_kept=<0|1>`, as
`contracts/resource-targets.md`, "Resource weights (v2)", states. Since SS-1
(0.12.30) a candidate with a base term on any other instance carries the same
fields with its base term `R` (`target_id=*`, `mode=adjust`, ` offset_kept=1`),
unless a v1 target applies to it, which keeps `target=<s>` ("Base valuation on
every instance").

Bounds, every truncation marked: a chain of `L` reasons gains `rank_breakdown`,
`decision_record` and at most `min(8, 128 - L - 2)` candidates when
`128 - L >= 2`, otherwise nothing, and `rank_assigned`'s detail then ends with
`; decision_record omitted: reason chain at <L>/128`. A detail over 1024 bytes
is cut at a character boundary to at most 1010 bytes and ends with
`…[truncated]`. Codes carry no whitespace.

The record is written only where the chain already is: the `reasons` of the
policy dispatch events of an admission attempt (`policy.dispatch_intent`, then
`policy.dispatch_admitted` or `policy.dispatch_rejected`, and
`policy.dispatch_completed`). An evaluation round is never recorded on its own,
so a poll that dispatches nothing records nothing. Ranks, states, intents
(decision identity and prerequisites), wake times and selections are unchanged
by it.

## Why A Candidate Does Not Run

Workflow #386 (scheduling model v1.4.2 §8; slice SR-6a). Every decision that is not selected
carries at least one reason, and its last reason is a registered outcome code
(the outcome catalog `contracts/outcome-codes/`, the policy fragment for the evaluator's own codes): that is the
code `why` names in a dispatch decision record, and it is never `none`. An evaluator path that
would end a decision without a reason is a programming error: the candidate is still deferred,
with the reason `decision_reason_missing` (error; `task_id`, `instance_id`), and the round goes
on, so the gap is visible instead of silent. The order in which the deferrals of the scheduling
rules are decided, and so which code a deferred candidate carries, is in
[`scheduling-blackout.md`](../scheduling-blackout.md), "Order of the deferrals". Its last
position (Workflow #405 D1) is the random delay: on an instance whose
`scheduling.random_delay.enabled` is true in effect, a candidate that passed every other gate is
deferred with `dispatch_random_delay_waiting` (info; `instance_id` the alias, `task_id`,
`due_at_unix_ms`, `random_delay_ms`, `next_eligible_unix_ms` the planned start) until its drawn
delay has passed, and the intent it is then dispatched with records the delay
(`policy.dispatch_intent` `random_delay`). Evaluations
stay out of the ledger; a person sees the reason where the next paragraph says.

**Where a person sees it** (Workflow #386 SR-6b, MCP-19a; [`v012-register.md`](../v012-register.md)
§1.143). The policy host keeps the latest evaluated cycle's not-run explanation in memory, per
instance that has a decision in that evaluation, by alias:
- the instance's earliest decision wake;
- the decision selected for it: its task, its decision id and the state of its dispatch when the
  explanation is read (`pending` before its intent is recorded, then `intent`, `admitted`,
  `rejected` or `completed`, as the dispatch projection holds it; a rejected one did not run
  either);
- every other decision as a candidate row `{task_id, state, eligibility, why, reasons,
  reasons_omitted?, next_wake_unix_ms?, since_unix_ms, expected_runtime_ms,
  expected_runtime_source}` (the last two SCH-M ED-2: the duration the pair's dispatch would
  reserve in that evaluation, its expected duration E or its declared duration, and `runs:<n>` or
  `declared`; "Settlement Feedback And Eligibility Age"): `state` is `blocked` for a blocked
  decision and `deferred` for every other (an eligible one that another task outranked waited
  this round); `why` is the full outcome of its last reason, the code `why=` names in a dispatch
  decision record; `reasons` are the codes of its reasons in order, the last 32 when there are
  more (`reasons_omitted` counts the others), so it ends with `why`'s code; the wake is the
  decision's own. Due rows (eligibility `true`) come first, ranked ones by total score, highest
  first, then unranked ones, each tie by task id; then the rest by their wake, earliest first and
  none last, then by task id;
- one not-running row (scheduling model §8.2 item 4): the first due row's task, `why`, wake and
  since; with no due row, no task and `why` `due_task_missing` (info; `instance_id` the alias,
  `next_eligible_unix_ms` the instance's earliest wake when one is known).

A row's `since_unix_ms` is the first evaluation of an unbroken run of evaluations in which the
same (instance, task) had the same `why` code; the not-running row's, of the same (instance, task
or none, `why` code). Every evaluated cycle replaces the table, before the host adds its notes to
the decisions (so `eligible_since_unknown` and `policy_instance_fact_missing` below are not part
of it); a deferred cycle or a failed evaluation leaves it as it was, and a start begins with none,
so a run counts from the first evaluation after the start. Nothing of it is written to the ledger
and no decision reads it. The general read `ScheduleExplanation`
([`runtime-interface.md`](../runtime-interface.md), "New request kinds") answers it; its outputs
are MCP-19b's: `actingctl schedule`, MCP `ac_schedule`, the `not_running` row of `ac_overview` and
the `not_run` rows of `ac_diagnose`.

What the ledger keeps (Workflow #411 A26, since 0.12.5): only a round that dispatches records
anything, in the reasons of its dispatch events ("Dispatch Decision Record" above). There each
candidate the record lists carries `why` (that code), `decision_state` and `eligibility` as values
of `decision_candidate_not_selected`, and `decision_record` counts the candidates it does not
list by `why` in its value `omitted_why`. Before A26, `why` was only in the developer detail,
which is not persisted.

Besides the deferrals, one reason ends a decision after admission: a dispatch that was admitted
but has not started is withdrawn when its task package is unloaded, and its decision record and
reason chain end with `pack_registry_dispatch_withdrawn` (info; `package_id`, optional
`request_id`, `instance_id`; Workflow #389 U-3, [`runtime-interface.md`](../runtime-interface.md),
"Unloading a package in use"). It is not a deferral, so it has no place in the order above.

**Status.** Written ahead of its code (documentation slice SR-0) and in force since SR-6a
(v0.12.0). A reason counts as registered when its spelling is an active code of the outcome
catalog; the evaluator's existing reason spellings are registered by the outcome-code typing of
the policy crate (Workflow #378 A2d).

The evaluator gives `decision_reason_missing` as it closes the round, before it writes any
dispatch decision record. A decision left eligible becomes deferred. So `why`, and the codes
`omitted_why` counts, never name `none`; `omitted_why=none` still means that no candidate was
omitted. `why` is always the last reason the evaluator wrote. The host can add reasons to a
cycle's decisions after the evaluation: `eligible_since_unknown` ("Settlement Feedback And
Eligibility Age" below) and `policy_instance_fact_missing` (value `fact_key`)
([`instance-fact-store.md`](../instance-fact-store.md), "Explanation"). These follow the
evaluator's reasons and do not replace the reason that settled the decision.

A `fact_time_reached` predicate that is false adds its own reason before `trigger_false` when it
decides the trigger (Workflow #405, "Fact-time wake" below): `fact_time_pending` (info;
`fact_key`, `next_eligible_unix_ms`) while the time its fact holds plus its offset has not come,
and `fact_time_unavailable` (info; `fact_key`, `fact_time_problem`) when its fact gives no time.

An edge member that makes its predicate false adds its own reason the same way (SCH-M CF-1,
"Condition edges" below): `dependency_already_served` (info; `task_id`, `predecessor_task_id`)
for `dependency_completed` with `since_own_run`, and `fact_time_served` (info; `fact_key`,
`served_by_dispatch_unix_ms`) for `fact_time_reached` with `once_per_reading`.

A `fact` predicate whose value is a fact reference adds `fact_reference_unavailable` (info;
`fact_key` the referenced fact, `unknown_reason`) the same way when the referenced fact gives no
integer (#386 CF-4, "Fact references" below), and so does a `resource_projection` whose value is
one (#386 CF-5, "Pool fact references" below).

## Settlement Feedback And Eligibility Age

In the Runtime (Workflow #308 slice 5b) every `TaskRuntimeSnapshot` the host
projects carries what earlier runs of that task on that instance settled:

- `last_duration_ms` is the duration of the latest policy run completed at or
  before the input ledger position (`policy.dispatch_completed`): its
  `runtime_ms` when it succeeded, `observed_at_unix_ms - admitted_at_unix_ms`
  of its execution record when it failed (0 when a reconciled settlement
  precedes its admission). The score stage uses it as the utility cost:
  `cost_ms = last_duration_ms` when above zero, else the declared
  `expected_duration_ms`, and the `score_computed` detail prints that `cost_ms`.
  When the snapshot carries the expected duration E (below), the cost is E.
- `expected_runtime_ms` and `expected_runtime_runs` (SCH-M slice ED-1, frozen model v1.0 §3.4
  D-12, D-15) carry the expected duration E of the pair and how many samples it is the mean of:
  E = min(⌈mean of the latest `runs` successful scheduled runs of the task's package on the
  instance⌉ + `margin_ms`, the task's `loop_budget.max_runtime_ms`; SS-3, since 0.12.30: no
  ceiling when the V2 task does not write it), with the instance's
  `scheduling.expected_runtime.*` in effect (`actingcommand_policy::expected_runtime`). With no
  sample, or with `enabled` false, E is the declared value and both members are absent, so the
  evaluation input and every evaluation are byte for byte those without samples. When present, E
  (at most `max_runtime_ms`) replaces the declared `expected_duration_ms` as the candidate's cost,
  as the dispatch intent's `expected_duration_ms` (hence the budget reservations, forward
  planning and the random-delay cut) and in the strategy's production estimate. Hard deadlines,
  lease TTLs and `loop_budget.max_runtime_ms` never use E. SCH-M slice ED-2: the host fills the
  members in every authoritative projection from the instance's `health.run_duration` record
  ([instance-fact-store.md](../instance-fact-store.md), "Run duration facts"): the samples are the
  durations of the instance's successful scheduled runs of the task's package, written at their
  settlement; tasks that share a package share them. A fact refresh read's declared duration is
  `scheduling.fact_refresh.expected_duration_ms`, and its E reads the samples of the instance's
  recognition maintenance package.
  - **Admission** compares the intent's `expected_duration_ms` with the duration the admission's
    own projection gives the pair: with E there, a different value is `policy_facts_stale` (a
    sample landed after the evaluation, and the next cycle evaluates again); without, the declared
    duration must match as before (`policy_intent_catalog_mismatch`).
  - **The dispatch intent record** (`policy.dispatch_intent`) carries `expected_duration_ms` when
    the duration it reserved is not its task spec's declared one (E, or a configured fact refresh
    duration); only an intent carries it (`invalid_policy_payload`, field
    `expected_duration_ms`), never 0. Recovery and replay reserve that value (the coordinator's
    ruling of 10-24 14:1x), so a receipt reserved with E replays without
    `policy_budget_receipt_mismatch`; an intent without it reserved its spec's declared duration.
    An immediate retry round keeps the declared duration it opened under, which a later catalog's
    task must still declare.
  - **Not-run explanation**: every candidate row of `ScheduleExplanation` (`ac_schedule`,
    `actingctl schedule`) carries `expected_runtime_ms`, the duration the pair's dispatch would
    reserve in that evaluation, and `expected_runtime_source`, `declared` or `runs:<n>`.
  - With `scheduling.expected_runtime.enabled` false for an instance nothing is written for it and
    nothing is projected: its evaluations, intents and records are those without samples. This is
    the rollback without a restart (`ConfigureSettings`).
- `failure_streak` is the number of consecutive failed runs ending at that
  latest completed run, 0 after a success. The evaluator validates and carries
  it; the score stage does not consume it yet.
- Both are absent while no run of the pair has completed. They are
  position-exact, read from the same policy dispatch records as the four
  settlement facts of `contracts/runtime-fact-store.md` (`task.<task_id>.<name>`
  for `last_duration_ms`, `last_outcome`, `failure_streak` and
  `completed_at_unix_ms`), and saturate at the evaluator's input bounds
  (86,400,000 ms and 10,000), while the facts keep the exact values.

`eligible_since_unix_ms` is the eligibility age the evaluator turns into
`aging_ms = now - eligible_since_unix_ms`. The host keeps it in memory only
(iron rule 13), one instant per (task, instance):

- After every evaluated cycle, a pair whose decision is `eligible`, `selected`
  or deferred with `score_deferred`, and that has no dispatch in flight, keeps
  its instant or starts one at the cycle's evaluation instant. A pair deferred
  for another reason (cooldown, Runtime availability) or with a dispatch in
  flight keeps an existing instant but starts none. Every other pair (blocked,
  not eligible, or not evaluated) loses its instant. Admitting a pair's intent
  ends its instant at once.
- A cycle's verdicts take effect when the next evaluation starts, before its
  inputs are projected, so an evaluation and the admission of its intents
  project the same ages. An instant is reported only to an evaluation instant
  strictly after it (a zero age reads as absent), and admission projects as of
  its intent's evaluation instant. Projections bound to a ledger position alone
  (`ProjectPolicyInputIdentity`, strategic reports) report no age.
- `fact_snapshot_id` hashes the projected task snapshots, so each of the three
  fields is part of the snapshot identity and of the decision identity derived
  from it. Admission compares it per instance (`contracts/instance-fact-store.md`,
  "Admission staleness per instance"): a run settled on another instance earlier
  in the same cycle leaves an intent fresh.
- Nothing about ages reaches the ledger: evaluations are not ledger events, so
  a restarted host cannot rebuild them. It starts with no ages; the first cycle
  evaluated after a start adds `eligible_since_unknown` to every decision whose
  pair starts aging there, and those ages count from that cycle.

## Settlement Outcome Records

In the Runtime (Workflow #313 goal 5) every settled policy run that projects its
mapped outcome record projects three `ObservedOutcome` records for its task and
instance:

- `<outcome_key>`: `Boolean(true)`, as before; predicate semantics are unchanged.
- `<outcome_key>.duration_ms`: `Integer` milliseconds, the same value slice 5b
  records as the `last_duration_ms` settlement fact, i.e. the run's `runtime_ms`.
  A failed run projects no outcome record at all (an outcome residual for it
  remains the fatal `policy_outcome_failed_run_residual`), so the failure form
  of that duration never reaches these records.
- `<outcome_key>.completed_at_unix_ms`: `Integer`, the run's terminal timestamp.

The three share `observed_at_unix_ms` (the terminal timestamp),
`activity_window_id` and `expires_at_unix_ms` (none). A value beyond the `i64`
range fails with the fatal `policy_settlement_outcome_overflow`. Because the
three share observation, window and expiry, the window-result and feedback-stop
checks reach the same result as with the mapped record alone. Remedies (Workflow #411
B15, [remedies.md](../remedies.md)) read a settled successful run's terminal
`scheduling_disposition.outcome_key` whatever the catalog references, and leave these records
unchanged. The derived keys
obey the evaluator's input bounds: 128 bytes per key and 16,384 outcome
records per evaluation, now three per settled pair. An outcome key may
therefore be at most 107 bytes (`<outcome_key>.completed_at_unix_ms` within
128): catalog activation, rollback and promotion refuse a catalog that
references a longer outcome key for any of its tasks with
`outcome_key_too_long_for_settlement` (request class) before anything is
appended, next to the task id check `task_id_too_long_for_facts`. `fact_snapshot_id` hashes the projected
outcomes, so it differs from before for any input with a settled run, and the
decision identities derived from it follow; this is not a wire change. The
policy input structures, the ledger event set and the decision identity rules
are unchanged.

**Outcomes a successful run did not produce** (Workflow #390 OUT-1, forced
compromise O-04). Admission refuses a catalog that references an outcome key
the task's package does not declare (Runtime Boundary), so the package declared
every key the run's own catalog referenced for the task. For the pair's latest
run, when it succeeded, each of those keys that the run did not produce and the
active catalog still references projects one more `ObservedOutcome`:
`Boolean(false)`, with the run's observation instant, activity window and
expiry. An outcome predicate on such a key evaluates `false`, not Unknown. A
failed or cancelled run, a pair without a completed run, and a key the run's
own catalog did not reference stay Unknown (`outcome_missing`). When the active
catalog does not reference the produced key, the produced key's three records
are left out; the pair still projects its `false` records, and projects nothing
when it has none (Workflow #400). These records count against the same bound of
16,384 outcome records per evaluation.

A strategic goal's `MetricRef::Outcome` reads the record with its exact key for
the assessment's instance. As for `MetricRef::Fact`, an `Integer` value is the
metric, any other value is none, and a record whose `expires_at_unix_ms` is
before the report's `as_of_unix_ms` is none. Naming
`<outcome_key>.duration_ms` or `<outcome_key>.completed_at_unix_ms` reads the
settled duration or completion time.

Before it projects a completed run, the host confirms from the ledger that the
run's admission request is the run's only `policy.dispatch_admitted` up to its
terminal (Workflow #317 item F). The confirmation is cached in memory only,
keyed by the run's `(decision_id, completed_sequence)` and held once per
(task, instance) pair: a repeated cycle that projects the same run through the
same terminal does not read the ledger again, while a different key (a newer
run of the pair), a different run identity or terminal, or a restarted host
confirms again. A failed confirmation is not cached and keeps its error. The
failed-run residual check, the run and outcome identity match and the outcome
re-projection still run on every cycle.

## Runtime Enforcement

Time validity is evaluated over the pinned input snapshot. Timeline invalidation
matches an observation's key prefix and scope (the same scope or a containing
server/game scope). Outcome keys are `outcome.<task_id>.<outcome_key>`; settled
execution state without mapped outcomes uses `task.<task_id>.terminal_state`.
Only schedule occurrences inside the event's V2 half-open validity interval
invalidate observations. V1 occurrences are unbounded. Duration controls
availability, including zero-duration resets, and does not undo invalidation.
An observation at or before the last matching reset is Unknown until replaced
by a newer observation. Equal timestamps are conservative because the input
contains no ordering evidence within that millisecond. A V2 event may name key
prefixes it does not invalidate, and a reset never clears a task's own last
result for the task's own edges ("Reset exclusions and the task's own last
result" below).

A fact or outcome whose observed value kind differs from the kind its predicate
compares (`fact`, `outcome`), a `record_deadline` fact that is not a
`record_list`, or a record whose timestamp field is not `timestamp_ms` resolves
that predicate Unknown with reason `type_mismatch`; a record missing its
timestamp field resolves it Unknown with `field_missing` (the selection-policy
words; Workflow #355 D12). Neither fails the evaluation: the other tasks and
instances evaluate and dispatch normally, and the reason surfaces as the
planning signal whose outcome is `planning_fact_unknown` with the values
`fact_key` and `unknown_reason` (`type_mismatch` or `field_missing`). Each
new fact snapshot reserves one more detection for that instance's activity
window when that instance has no other dispatch in the cycle and its activity
window is open (as for `fact_missing`), so an unrepaired pack type error ends as
one detection-quota-exhausted signal per window. Errors of the catalog's own
comparison (an operator on kinds it does not accept, such as `contains` on two
integers) and arithmetic overflow still fail the evaluation; Runtime reports
them as `policy_evaluation_rejected` and keeps the policy code and message as
the record's native detail. Since 0.12.28 (ISO-PE, #419) such a failure no
longer stops the Runtime: the evaluation is skipped and evaluated again later
("Evaluation failures" below).

`ObservedOutcome.expires_at_unix_ms` is optional and uses the fact TTL convention:
the expiry millisecond is valid and expiry + 1 wakes reevaluation. Matching future
resets cap admission freshness at occurrence - 1 and wake at the occurrence.
Ledger-derived pools consume the same effective fact projection.

The first dispatch depends on `trigger`. `feedback_stop` is evaluated only after
a valid result for the same task and instance in the current activity window.
`ObservedOutcome.activity_window_id` comes from the settled run's admission;
`TaskRuntimeSnapshot.completed_window` carries the settled window ID and completion
timestamp for tasks with no mapped outcome consumer. Window identity follows the
selected activity profile and its first active declared window, including its
local start day for windows crossing midnight. A result must both name that
window and have completed inside it. An enabled stop predicate retains Unknown
when its required observation is unavailable. Missing historical window fields
default to `None`, which supplies no evidence for feedback stop; a missing outcome
expiry supplies no additional TTL. An enabled feedback stop's activity-window
closure also bounds freshness and wakes reevaluation. These input projections add no ledger wire or stored
transaction state.

The evaluator pins the selected activity profile in every dispatch intent. Runtime owns activity sampling, budget counters, retry state, and failure escalation; callers cannot supply remaining-budget values. Admission and execution ledger events record the selected profile, sample seed, activity window, cadence, cumulative task and activity budget receipts, and classified outcome.

Recoverable failures receive a positive, bounded backoff. Only repeated failures with the same failure code, failure identity and failure class share a consecutive-failure streak, and sensitive or severe failures are never automatically restarted. A failure records its `outcome` (whose category is the record's severity) and, for a scheduled `linear_steps` or (Workflow #390 EXEC-3) page-graph task, its `failure_identity`, which is rerun at once and lifted by a package update (`../policy-suspension.md`). Goal-missed, feasibility-red, and drift-predicted signals are informational planning facts: they do not consume failure tax, advance a failure streak, or pause execution.

### Optional run-count bounds

SS-3 (since 0.12.30; Alice's ruling 100 on Workflow #386: dispatch follows live account state, and there is no "run task X N times"; register §1.212). In an `actingcommand.scheduling.v2` catalog a task's `loop_budget` may be left out, and so may each of its three items, `daily_limit`, `window_iteration_limit` and `max_runtime_ms`, on its own; so may each of an activity profile's `daily_budget`, `max_window_iterations` and `session_max_ms`. V1 keeps all six required (`missing_required_field` at the member). In V1 and V2 a bound that is written is a value: an explicit `null` is refused (`type_mismatch`), as the V2 schemas declare and as for an effect's `amount`; it is never read as "not written". A bound that is written applies exactly as before, and must still be above 0 (`loop_budget_missing`); a catalog that writes all of them serializes, hashes, admits, settles and records byte for byte as before. A bound that is not written is no dimension:

- **Admission** reserves nothing for it and never refuses on it (`policy_budget_exhausted` names only a written dimension). The dispatch intent carries only the written items in `prerequisites` (`daily_limit`, `window_iteration_limit`, `max_runtime_ms`), and the intent check requires each one it carries to be above 0 and the intent's expected duration to be within a carried `max_runtime_ms`.
- **The budget receipt** of `policy.dispatch_admitted` and `policy.dispatch_completed` carries neither member of an unwritten dimension (`task_daily_used`/`task_daily_limit`, `task_window_used`/`task_window_limit`, `task_runtime_reserved_ms`/`task_runtime_limit_ms` and the three activity pairs); a carried dimension holds both members with the count between 1 and the limit, as before, and a receipt with one member of a pair is invalid. The receipt is part of the admission record those two events carry (and the retry round of `policy.control_state_recorded` keeps), and the global ledger query's public projection hands it on unchanged as `PublicPayload.policy_admission`, which reaches clients through runtime-client. The Runtime keeps no count for an unwritten dimension, and a condition rerun's refund gives back only the counts the admission took.
- **The retry round** does not judge an unwritten dimension; retries stay bounded by `on_failure` and the round's window.
- **Settlement** never rewrites a run to `policy_runtime_budget_exceeded` on an unwritten runtime dimension: without `max_runtime_ms` (and `session_max_ms`) a window's cumulative runtime has no bound. The per-run deadline (the contained-task response bound and the package's own timeout) is unchanged, and so are failure retries and escalation, the per-occurrence rerun limit, the recovery ladder, in-pack loop caps and concurrency.
- **Expected duration E** has no ceiling without `max_runtime_ms` ("Settlement Feedback And Eligibility Age").
- **Workflow #411 B8** hands a task that opts in to `dispatch_parameters` `daily_remaining` and `last_daily_slot` only from the daily dimensions written (the task's `daily_limit`, the profile's `daily_budget`, the fewer of those written); with neither written both are absent (`../remedies.md`, section 4).
- **The strategy report's production estimate** takes the fewest runs an hour of the bounds written (`daily_limit`, `window_iteration_limit`, `max_runtime_ms` / E) and the period bound (one hour over the task's cycle), which is always present.

**Readers before 0.12.30.** A receipt that leaves members out is written only after the resource side first leaves a bound out; until then every receipt is written as before, and every build reads it. From then on a reader older than 0.12.30, which requires all twelve members, cannot decode it:

- **A Runtime before 0.12.30** refuses the ledger with `corrupt_ledger_record`: at open, when such a record lies among the newest `ledger_observer.open_verify_tail_records` rows it verifies (`../ledger-store.md`), and otherwise later, when the background recheck reaches the record, latches a Global failure and stops the Runtime with the same code (`../ledger-observer.md`). It also refuses a V2 catalog that leaves `loop_budget` out.
- **Tools and the UI before 0.12.30** fail to decode a ledger event query page (`RuntimeEventQueryPage`) that carries such a receipt in `PublicPayload.policy_admission`.

No ledger revision is bumped for this (`ledger` stays [3, 4]); the bump comes with SS-9, which deletes the members. After the resource side first leaves a bound out, no rollback below 0.12.30 is possible, so the resource side leaves bounds out only once every deployment runs 0.12.30 or later (the 0.12.30 release notes and Workflow #428 say both).

Deleting the members (SS-9), the no-progress guard (SS-4) and batch allowances (SS-6) are later slices.

## Task Execution Fields

`entrypoint.operation_id` names the Runtime mechanism capability that may execute the task. `procedure_ref` is the immutable external package or procedure-definition identity pinned into the dispatch intent and the immutable catalog generation referenced by its reason chain. It is not a file path, script, executable capability, or approval authority; Runtime must resolve it through the approved adapter/package boundary and reject any mismatch at admission.

Before an intent becomes trusted, a Runtime-owned manifest content-addressably binds `procedure_ref`, the package SHA-256 digest, `operation_id`, and the ordered `yield_points`. The binding digest participates in decision identity and is persisted with dispatch events. Admission resolves the alias again and requires the exact package and binding digests before requesting a lease.

`expected_duration_ms` is the declared reservation and planning duration D; a pair whose snapshot carries the expected duration E (SCH-M ED-1, "Settlement Feedback And Eligibility Age") uses E instead. `cooldown_ms` is the minimum interval after the last dispatch before the task can become eligible again. `next_run_clamp_ms` caps recoverable retry delay. `yield_points` names the only package-declared safe cooperation points that may be exposed to a mechanism adapter; it never grants a new operation. `sensitive` disables automatic restart after failure and does not weaken fatal-error propagation.

## Clock Sources

Every clock schedule declares exactly one source:

- `local` uses the host-provided monotonic coordinate and is valid only for interval schedules. The evaluator projects its next occurrence back to Unix time for transport.
- `server` uses a pinned timezone identity, base UTC offset, explicit DST offset, and bounded maintenance drift.
- `reveal` has the same calendar fields plus `reveal_source`, the immutable evidence identity from which the catalog author derived the pinned schedule.

A clock occurrence of a clock without `missed` is owed from the moment it occurs until the task is next dispatched; it has no expiry (Workflow #361). Several occurrences missed in a row collapse into the latest one. An occurrence that fell while the task's activity window was closed, or while no Runtime was running, is dispatched once at the first admissible evaluation of the next window, and that dispatch counts against the loop budget of that window's day. A timeline reset invalidates the task's settled state but does not settle a missed occurrence.

A catalog that must skip a missed occurrence instead of replaying it gives the clock `"missed": {"kind": "skip_after", "grace_ms": N}` (Workflow #361 R7; `actingcommand.scheduling.v2` only, `grace_ms` from 60 000 to 604 800 000). Such a clock's latest occurrence O, not yet served, is due only while the evaluation is before `O + grace_ms`, on the schedule's own clock; from that instant the clock is false until its next occurrence, so a skipped occurrence never takes the budget of a later day. A pair that was never dispatched follows the same rule: its latest occurrence is due only inside its grace. While the occurrence is due, the clock's next wake and its freshness end at `O + grace_ms`; once it expired, the clock adds the info reason `clock_occurrence_expired` {`occurrence_unix_ms`, `grace_ms`} to the decision's reason chain (the decision stays `trigger_false` when the clock decides the trigger). The skip is recorded once as `clock_slot_expired` (Workflow #372 K4, below). `missed` is refused under a `not` (`predicate_uncomputable`), in a V1 document as an unknown member, and with a grace outside its bounds (`limit_exceeded` at `…/missed/grace_ms`); an absent `missed` leaves the catalog's serialization and hash unchanged.

A due occurrence that a gate stops, or that a dispatch serves after its activity window opened, leaves one record; so does an occurrence lost to its `missed` grace or to a later occurrence with no other record of it (Workflow #372; "Clock-slot records" below).

The catalog contains the effective DST offset instead of consulting a hidden timezone database. The base UTC offset is bounded to `[-840, 840]` minutes and the explicit DST offset is independently bounded to `[-120, 120]` minutes; their effective sum is therefore bounded to `[-960, 960]` minutes. A DST transition, server-clock correction, reveal change, or maintenance delay creates a new immutable catalog generation and triggers full recomputation. `maintenance_drift_ms` shifts nominal occurrences and is bounded to seven days. Calendar and absolute schedules cannot use `local`, because monotonic coordinates do not identify wall-clock instants and are not portable across host boot epochs.

`next_wake_unix_ms` remains the earliest lower bound across timeline and predicate wakes. When a task wake is known, `preload_hint` pairs that lower bound with the task ID, its `procedure_ref` as `package_ref`, and an explicit confidence. A preload hint is advisory only and never constitutes admission or execution permission.

### Clock-slot records

A due clock occurrence that is not dispatched leaves one trace (Workflow #372). The trace is a `policy.planning_signal_observed` record of kind `goal_missed` whose severity is its code's category (`warning` for a refused or superseded slot, `info` for a deferred, owed or expired one): like every planning signal it consumes no failure tax, advances no failure streak, pauses nothing and wakes no agent. `actingd suspended` lists the records with their settled fate (`../policy-suspension.md`), and MCP `ac_diagnose` shows up to 10 of an instance's.

- **Came due, served, owed.** An occurrence O came due for a (task, instance) pair when an evaluation ranks the pair as a candidate (its trigger is true) and O is its due occurrence. A pair whose trigger is false did not come due and leaves nothing: a follower after a failed head or after its group's reset, an occurrence outside its own `timeline_active` window, a true `feedback_stop`. O is served by the pair's first admitted dispatch after O. A selected dispatch is owed when its O lies before the start of the activity window it is selected in; it is a first run when the pair had no dispatch.
- **Codes.** `clock_slot_refused` (warning): a persistent gate refused a due O. `clock_slot_deferred` (info): a transient gate held it back. `clock_slot_owed` (info): the first evaluation that selects the pair for an owed O. `clock_slot_expired` (info) and `clock_slot_superseded` (warning): an unserved O lost to its clock's `missed` grace or to a later occurrence (Workflow #372 K4, "Lost occurrences" below). A pair with a dispatch in flight gets no record.

| gate | codes | recorded as | when |
|---|---|---|---|
| persistent | `policy_budget_exhausted` (with its `budget_dimension`), `policy_activity_window_closed`, `policy_task_paused`; at placement `instance_override_disabled`, `capability_missing`, `activity_profile_missing` | `clock_slot_refused` | first sight |
| operator pause | `dispatch_paused_global`, `dispatch_paused_instance` | `clock_slot_deferred` | first sight |
| queue wait | `dispatch_instance_held`, `policy_retry_backoff_active`, `policy_activity_interval_active` and any other eligibility deferral; at placement `instance_unavailable` | `clock_slot_deferred` | only at an evaluation where the pair's activity window is closed: the wait outlasted the window |
| waiting in the round | the evaluator's own deferrals (another candidate selected, host or scene budgets, capacity, score, cooldown) | nothing | once the window closes, such a candidate meets `policy_activity_window_closed` |

- **A pair with no dispatch** gets refused and deferred records only at an evaluation where its activity window is open, or when it has no activity profile. An occurrence before that, for example from an activation at night, is covered by the owed record of the pair's first run, which carries `first_run`.
- **Once per occurrence.** The record's `signal_id` is `signal:clock_slot:<refused|deferred|owed>:<hex>`, where `<hex>` is the lowercase SHA-256 of the UTF-8 bytes `task \0 instance \0 O`, with O in decimal ASCII. It is checked against the planning-signal projection before the record is written, so each kind is written at most once per (pair, O), across restarts too. A refused record stops later deferred ones. A deferred record stops a later refused one only for `policy_activity_window_closed`, the consequence of the pause or wait, not a new cause; a later budget, task-paused or configuration refusal is still written. Within a class the first gate seen wins.
- **Outcome.** The record's `instance_id` is the instance alias and its `task_id` the task. Its `outcome` is the code with its values (Workflow #378 A2-P): `task_id` and `alias` always, then each present value; an absent value is left out, never written as `none` or `null`. Refused and deferred records carry `gate_code` and `occurrence_unix_ms`, and when known `budget_dimension`, `window_end_unix_ms` (E, the end of the activity window open at the record), `next_eligible_unix_ms` (N) and `next_occurrence_unix_ms` (X). Owed records carry `occurrence_unix_ms` and `window_start_unix_ms`, `next_occurrence_unix_ms` (X) when known, and `first_run: true` for a first run. `gate_code` is a code, `budget_dimension` a vocabulary token, the times are Unix milliseconds and `first_run` is a boolean.
- **Fate.** A refused or deferred record predicts `same_window` when E is present and N < min(E, X): served late in the same window; `next_window` when N < X and E is absent or N ≥ E: served in the next window as an owed run, which for a daily-limited task displaces that day's own slot; `collapsed` when N ≥ X: absorbed by the next occurrence; and `pending` without N (a pause, a wait, an unavailable instance, a paused task, the configuration). The pair's first admitted dispatch D after the record settles it: `collapsed` when D ≥ X, `same_window` when D < E, `next_window` otherwise; without D, `collapsed` once X has passed. An owed record settles as `served` when D < X, and as `absorbed` when D ≥ X or, without D, once X has passed. An expired record is settled `expired` and a superseded one `collapsed`; a refused or deferred record of an occurrence that has an expired record settles as `expired` (Workflow #372 K4).
- **Fact invalidation.** Planning signals are appended without an instance link, so a clock-slot record invalidates every active fact whose `invalidate_on` lists `policy.planning_signal_observed`, on any instance.
- **Lost occurrences (Workflow #372 K4).** After the three kinds above, every evaluation reads the pairs of its task snapshots that have a dispatch history. O is unserved when the pair has no admitted dispatch at or after O and none in flight. Over the counted clocks (those reached through `all` and `any`, not under `not`; none when one is an interval under one hour), the evaluation takes the occurrences after the pair's last dispatch, at or before now and within 48 h before now, at most 32, newest first; occurrences of several clocks at one instant are one. With O′ the next occurrence after O over every counted clock:
  - `clock_slot_expired` {`occurrence_unix_ms`, `grace_ms`, `next_occurrence_unix_ms` (O′) when known}: O's clock has `missed`, `O + grace_ms` ≤ now, and O′ is absent or `O + grace_ms` ≤ O′. It is written even after a refused or deferred record of O, whose fate then settles as `expired`.
  - `clock_slot_superseded` {`occurrence_unix_ms`, `next_occurrence_unix_ms` (O′)}: O′ ≤ now, O did not expire, and no refused, deferred or expired record of O exists. In a trigger that mixes clocks with and without `missed`, the successor is taken over every counted clock.
  - A record is written only while the pair is live: its task is placed on the instance, and its trigger, evaluated with the evaluation's facts once every counted clock is counted due, is true. A follower whose head failed or a task outside its timeline window gets none. A pair that was never dispatched gets none either: R7's grace bounds its first run, and K2's owed record carries `first_run`. While the Runtime runs, the first evaluation after `O + grace_ms` or O′ comes within one cycle; after a Runtime gap, liveness reads the facts at the restart.
  - Each is written once per (pair, O), its `signal_id` `signal:clock_slot:<expired|superseded>:<hex>` with K2's digest, checked against the projection before it is written.
- **Not recorded.** An occurrence a catalog skips by making its trigger false, and the occurrences of a pair that was never dispatched.

## Fact-time wake (Workflow #405)

A task that is due when earlier work ends, at an instant a package read and published as a fact, names the predicate `{"kind": "fact_time_reached", "scope": …, "fact_key": …, "offset_ms": N, "max_age_ms": M}` (`actingcommand.scheduling.v2` only; [v2/README.md](v2/README.md)). The scheduler wakes at that instant instead of polling.

- **True** when the fact at (`scope`, `fact_key`) exists, its confidence is above 0, it is fresh exactly as for `fact` (observed no longer ago than `max_age_ms` when M is not null, and not past its own `expires_at_unix_ms`), its value is a `timestamp_ms` T, and the evaluation is at or after `T + N`. The sum is signed and never overflows; a sum below 0 counts as 0. While the predicate is true, its freshness is the fact's and it wakes the evaluation one millisecond after that freshness ends, as `fact` does.
- **False, not reached**, before `T + N`: the predicate adds the info reason `fact_time_pending` {`fact_key`, `next_eligible_unix_ms` = `T + N`} and wakes the evaluation at `T + N`, or one millisecond after the fact's freshness ends when that comes first. A newer reading of the fact wakes the scheduler as every fact publication does, and the next evaluation reads the new time.
- **False, no time**: the fact is missing, its confidence is 0, it is older than M, it is past its own expiry, or its value is not a `timestamp_ms`. The predicate adds the info reason `fact_time_unavailable` {`fact_key`, `fact_time_problem`: `missing`, `low_confidence`, `stale`, `expired` or `not_a_time`} and no wake. Unlike `fact` it is false, not Unknown: the time is the output of the package that ran the work, which the instance's recognition maintenance package cannot read, so the predicate never asks for a fact refresh ("Fact Refresh") and gives no detection suggestion.
- **Minimum interval.** Once T + N has passed, the predicate stays true until a newer reading moves the time, unless it carries `once_per_reading` ("Condition edges" below). The least time between two dispatches of the task is its `cooldown_ms` ("Task Execution Fields"); the predicate has no interval of its own.
- **Fallback.** A catalog may write the predicate and a clock in one `any`: whichever is true first makes the task due, so the clock still runs it while the fact gives no time. Under `all` and `not` the predicate combines as any other, and every other predicate evaluates as before. When the predicate decides the trigger, the decision ends with `trigger_false` after its reason.
- It is no counted clock for the clock-slot records (above) and references no outcome key.

## Condition edges (SCH-M CF-1)

A chain step runs once per new result of its predecessor, and a completion time is served once, without the pass, round and slot reset events that cleared `task.` and `outcome.` facts so that a level could fire again. Two optional boolean members ([v2/README.md](v2/README.md); `actingcommand.scheduling.v2` only) make a predicate an edge. Both are absent by default: a catalog without them evaluates, serializes and hashes exactly as before. Their inputs are the `TaskRuntimeSnapshot` fields of this task and of the predecessor on the decision's instance ("Settlement Feedback And Eligibility Age"); nothing else is read. This task's own fields are its own last result, which no timeline reset clears (since 0.12.25, "Reset exclusions and the task's own last result").

- **`dependency_completed` with `since_own_run: true`.** True when the predecessor's latest terminal state is one of `terminal_states`, and this task has not served it yet: it has no admitted dispatch on the instance, or its latest dispatch is before the predecessor's latest dispatch, or its latest run did not succeed. Otherwise false, with the info reason `dependency_already_served` {`task_id` this task, `predecessor_task_id`} and no wake; the predecessor's next accepted terminal makes it true again.
  - A failed successor stays due. Its cooldown, its retry and backoff (the Runtime's `on_failure` handling) and its daily limits bound how often it runs again.
  - A predecessor with no settled run is Unknown (`dependency_state_missing`) as before, and a terminal state the predicate does not accept is false as before.
  - With `false` or without the member the predicate is the level it always was: true whenever the predecessor's latest terminal state is accepted.
  - Chain handover ("Chain Handover") names the predecessor the same way, with or without the member.
- **`fact_time_reached` with `once_per_reading: true`.** A time T the fact holds counts as **served** when this task's latest run on the instance succeeded and was dispatched at or after max(`T + N`, the fact's `observed_at_unix_ms`). A served time makes the predicate false, with the info reason `fact_time_served` {`fact_key`, `served_by_dispatch_unix_ms` that dispatch} and no wake. It stays false until a newer reading, observed after that dispatch, gives a time; the publication of that reading wakes the scheduler, as every fact publication does. A run that failed, or one dispatched before the time or before the reading was observed, served nothing. Every other case is "Fact-time wake" above.
  - This closes the loop of an emptied slot: a slot left empty shows no countdown, so its last (past) time would otherwise stay true forever. A catalog names a free-slot count fact in an `any` beside it.
- When a member decides the trigger, the decision ends with `trigger_false` after its reason.
- **Version.** A V1 document that writes `since_own_run`, with either value, is refused as `unknown_field` at the member; `fact_time_reached` is V2 only as a whole.
- **Capability.** A Runtime that reads both members reports the always-on subsystem `condition_edges` in `config.subsystems` ([`actingd-check-config.md`](../actingd-check-config.md)). The catalog generator emits neither member for a Runtime whose check-config does not name it; an older Runtime refuses a catalog that writes them (`unknown_field`).

## Fact references (#386 CF-4)

A red line is the current cap less a margin, never a fixed number (Alice 10-26): a package publishes the cap as a fact beside the amount, and a rule compares the two. A `fact` predicate's `value` may be a **fact reference** instead of a typed value ([v2/README.md](v2/README.md); `actingcommand.scheduling.v2` only):

```text
{"kind": "fact", "scope": S, "fact_key": A, "comparison": C,
 "value": {"type": "fact", "fact_key": B, "offset": N}, "max_age_ms": M}
```

It reads: the integer the fact at (S, A) holds compares by C with the integer the fact at (S, B) holds plus N. All three members of the reference are required; N is a signed integer. A predicate with a typed value evaluates, serializes and hashes exactly as before.

- **Same scope.** The reference names no scope: B is read in the predicate's own scope S. In a `schedule.json` the composer fills S for the predicate as for every `fact` node, and the reference needs none.
- **One freshness.** Both facts obey the predicate's `max_age_ms` and each its own `expires_at_unix_ms`, and each needs a confidence above 0; the reference has no `max_age_ms` of its own. While the predicate is true or false, its freshness ends at the earlier end of the two facts' freshness, and it wakes the evaluation one millisecond after that, as `fact` does.
- **The fact itself** (S, A) is read as for every `fact` predicate: missing, at confidence 0, older than M or past its expiry, it is Unknown with its detection suggestion ("Fact Refresh"); a value that is not an `integer` is Unknown with `type_mismatch` ("Runtime Enforcement").
- **The referenced fact** (S, B) that gives no integer makes the predicate **false**, not Unknown: the info reason `fact_reference_unavailable` {`fact_key` B, `unknown_reason`: `fact_missing`, `fact_low_confidence`, `fact_stale`, `fact_expired` or `type_mismatch`}, no wake and no detection suggestion. The next reading of B wakes the scheduler, as every fact publication does. When the predicate decides the trigger, the decision ends with `trigger_false` after its reason. Under `not` a false predicate becomes true, as any other.
- **Comparison.** `eq`, `not_eq`, `less_than`, `less_than_or_equal`, `greater_than` and `greater_than_or_equal`, over the integers A and B + N; the sum never overflows. `contains` is refused.
- **Refusals**, each at its pointer: in a V1 document, `unknown_field` at `value/type`; `contains`, `predicate_uncomputable` at `comparison`; a reference `fact_key` that is no identifier, `type_mismatch` at `value/fact_key`; N outside -9007199254740991..=9007199254740991, `limit_exceeded` at `value/offset`; a reference that names a `scope` (a cross-scope reference), `unknown_field` at `value/scope`. A `value` whose `type` is neither a value kind nor `fact`, or an N that is no integer, is refused by the reader (`type_mismatch`).
- **Capability.** A Runtime that reads fact references reports the always-on subsystem `fact_reference` in `config.subsystems` ([`actingd-check-config.md`](../actingd-check-config.md)). A `schedule.json` whose rule writes one must list `fact_reference` in `requires` ([`standard-pack-schedule.md`](../standard-pack-schedule.md)); the composer refuses the file without it, so a Runtime that does not name the subsystem refuses the file (`schedule_requires_missing`) instead of misreading it. An older Runtime given a catalog with a reference refuses it at its reader.
- For example, a task due while the amount is at least the capacity less 1000: `"fact_key": "resource.neutral.amount", "comparison": "greater_than_or_equal", "value": {"type": "fact", "fact_key": "resource.neutral.capacity", "offset": -1000}`.

## Pool fact references (#386 CF-5)

An account's cap is read, never written as a constant (Alice 10-26). Two more V2 members accept the fact reference of "Fact references" above, `{"type": "fact", "fact_key": B, "offset": N}` ([v2/README.md](v2/README.md); `actingcommand.scheduling.v2` only): a pool's `capacity`, and a `resource_projection` predicate's `value`. Both read B **in the pool's own scope**; all three members are required and the reference names no scope. A pool or a predicate that writes a number evaluates, serializes and hashes exactly as before.

- **A capacity read from its fact.** `"capacity": {"type": "fact", "fact_key": B, "offset": N}`: at each evaluation the pool's capacity is the integer B holds plus N. B is read as the pool reads its own value: in the pool's scope, with at least the pool's `minimum_confidence_milli` (above 0 for a `static_snapshot` pool), not past its own `expires_at_unix_ms` (a timeline reset that invalidates its prefix removes it, as every fact), and the capacity must be at least 1. It has no `max_age_ms`: a reading's capacity expires with the reading (a `current_capacity` mark writes both parts with one lifetime, [resource-readings.md](../resource-readings.md)).
- **No capacity, no value: the projection defers.** While B gives no capacity the pool has no value, and no constant stands in for it. Its `resource_projection` is Unknown with the detection suggestion of B in the pool's scope and B's unknown reason: `fact_missing`, `fact_low_confidence`, `fact_expired`, or `type_mismatch` (no integer, or no positive capacity). So the task is deferred with `trigger_unknown` (or `feedback_stop_unknown`), and the fact refresh is asked for B ("Fact Refresh"), as for an unknown `fact` predicate. The pool's own reading is checked first: a pool whose reading is unusable asks for its own key, as before. A reserve on the pool is case 1 of "Task Reserves", a task that consumes it gets no fullness urgency from it, and a resource target on a live pool awaits observation with the matching reason ([resource-targets.md](../resource-targets.md)).
- **A reading above the capacity stays as read.** Against a capacity read from its fact, a pool's reading or snapshot above it is the pool's value (an amount can exceed its cap, for example through items): it is neither refused nor clamped, and it does not regenerate. Below the capacity each complete period adds `amount` up to the capacity, as before: regeneration stops at the capacity. Against a constant capacity a reading above it still leaves the pool without a value ("Pool Projection").
- **Freshness and wake.** The projection's freshness ends at the earlier end of the pool reading's and B's, and the evaluation wakes one millisecond after B's expiry, as after the reading's.
- **Bounds against a constant only.** The compiler checks a reserve's `at_least` ("Task Reserves") and an effect's quantity ("Expected production (v2)") against a constant capacity only. Against a capacity read from a fact nothing is checked at compilation: a reserve above the read capacity holds its task until the pool holds it.
- **A comparand read from a fact.** `"value": {"type": "fact", "fact_key": B, "offset": N}` on a `resource_projection` compares the pool's projected amount with the integer B holds in the pool's scope plus N, by its `comparison` (`contains` is refused as before); the sum never overflows. B obeys its own expiry and needs a confidence above 0, as a reference of a `fact` predicate without `max_age_ms` (a projection has none). A B that gives no integer makes the predicate **false**, with the info reason `fact_reference_unavailable` {`fact_key` B, `unknown_reason`}, no wake and no fact refresh, then `trigger_false` when it decides the trigger, as for a `fact` predicate ("Fact references"). The pool is read first: a pool without a value leaves the predicate Unknown, as above.
- **Forward projection** ("Forward Planning And Maintenance") reads B once, at its start, and holds that capacity across its horizon. A pool whose B gives no capacity there is left out, as a pool without a snapshot is, and reported as the evidence gap {`code` B's unknown reason, `fact_key` B}.
- **Refusals**, each at its pointer, as for a reference of a `fact` predicate: in a V1 document, `unknown_field` at `capacity/type` or `value/type`; a key that is no identifier, `type_mismatch` at `…/fact_key`; N outside -9007199254740991..=9007199254740991, `limit_exceeded` at `…/offset`; a reference that names a `scope`, `unknown_field` at `…/scope`. A `type` other than `fact`, or an N that is no integer, is refused by the reader (`type_mismatch`).
- **Capability.** A Runtime that reads both forms reports the always-on subsystem `pool_fact_reference` in `config.subsystems` ([`actingd-check-config.md`](../actingd-check-config.md)). A `schedule.json` that writes either must list `pool_fact_reference` in `requires` ([`standard-pack-schedule.md`](../standard-pack-schedule.md)); the composer refuses the file without it, so a Runtime that does not name the subsystem refuses the file (`schedule_requires_missing`) instead of misreading it. An older Runtime given a catalog with either refuses it at its reader.
- For example, a task due while the projected amount is at least the read capacity less 20: the pool `"capacity": {"type": "fact", "fact_key": "resource.neutral.capacity", "offset": 0}` and the trigger `{"kind": "resource_projection", "pool_id": "pool.neutral", "comparison": "greater_than_or_equal", "value": {"type": "fact", "fact_key": "resource.neutral.capacity", "offset": -20}}`.

## Reset exclusions and the task's own last result (Workflow #411 P19)

A daily reset that clears a family of readings must not clear the ready times in that family, which are absolute times that expire by themselves, and a reset must not make a chain step forget its own last run (#411 6455013768 items 1 and 2; since 0.12.25).

- **Excluded prefixes.** A V2 timeline event may carry `"excludes_fact_prefixes": [P, …]` ([v2/README.md](v2/README.md)): key prefixes it does **not** invalidate. An occurrence of the event invalidates the observation of a key when the key starts with one of its `invalidates_fact_prefixes` and with none of its `excludes_fact_prefixes` ("Runtime Enforcement"); this holds for facts, for `outcome.<task_id>.<outcome_key>` and for `task.<task_id>.terminal_state` alike. An excluded key keeps its observation through the event and expires only by its own `expires_at_unix_ms` (and a predicate's `max_age_ms`), and its expiry is not capped at the event's next occurrence. For example `"invalidates_fact_prefixes": ["resource."], "excludes_fact_prefixes": ["resource.neutral.ready."]` clears every `resource.` reading at the reset except the ready times below `resource.neutral.ready.`, and a `resource.` key added later is cleared without a change to the event.
  - The member is optional; absent, the event and its catalog serialize and hash exactly as before. It holds at most 128 entries, each an identifier (`type_mismatch` at the entry otherwise) that starts with one of the event's `invalidates_fact_prefixes`: an entry below none of them would exclude nothing and is refused as `dangling_reference` at the entry. A V1 document that writes it is refused as `unknown_field` at the member.
  - The event still wakes the evaluation at its occurrences, as before; a wake that clears nothing is harmless.
- **The task's own last result.** A reset that invalidates `task.<task_id>.terminal_state` clears the task's settled state for everything that reads it as a result of the current period: the window result before `feedback_stop` ("Runtime Enforcement") and a `dependency_completed` of **another** task that names it, which is Unknown (`dependency_state_missing`) until the task settles again. It never clears the task's own last result, which the task's own edges read:
  - a `dependency_completed` whose `task_id` is the task itself reads the task's latest terminal state on the instance, as settled, whatever reset came since;
  - `since_own_run` ("Condition edges") reads this task's own latest run that way, so a reset of the successor alone does not make an edge it already served due again;
  - `once_per_reading`'s served check ("Condition edges") reads this task's own latest run that way, so a reading the task served stays served through a reset, until a newer reading.
  - The latest dispatch, which `clock` and the cooldown read, was never cleared by a reset and is unchanged.
  - **The class of rule this changes.** A rule that uses a `dependency_completed` on its own task as the result of the current period, for example a retry branch on its own `failed`, now reads the task's last result across a reset, so after the reset it sees a result from before it. A retry branch that must fire only within the current period needs, as its period gate, a `dependency_completed` on an earlier task of the same period, which the reset clears.
- **Capability.** A Runtime that reads `excludes_fact_prefixes` and keeps the task's own last result reports the always-on subsystem `reset_exclusions` in `config.subsystems` ([`actingd-check-config.md`](../actingd-check-config.md)). A `schedule.json` whose event writes the member must list `reset_exclusions` in `requires` ([`standard-pack-schedule.md`](../standard-pack-schedule.md)); the composer refuses the file without it, so a Runtime that does not name the subsystem refuses the file (`schedule_requires_missing`) instead of misreading it. A `schedule.json` that relies on a reset keeping the task's own last result names it as well: nothing in the file shows that reliance, so the composer cannot require it. An older Runtime given a catalog whose event writes the member refuses it at its reader (`unknown_field`).

## Runtime Boundary

V1 persistent scheduling state is single-host, local-filesystem state. Catalog generations, active pointers, ledgers, leases, budget journals, fact projections, approval projections, and release-set pointers must not be shared concurrently by independent hosts. A future multi-host revision must add host identity and fencing to every one of those owners instead of partially extending the V1 files.

Cross-run decisions are reevaluated by the scheduler after each outcome. A bounded rule table may make mechanical choices inside one run, but it cannot call back into the scheduler for mutable external state. External state required by a run must be pinned into dispatch parameters and its reason chain before admission.

Scheduled contained admission carries the validated, package-bound task request
into the existing lease acquisition. Its TTL uses the request's response budget
plus the same heartbeat/closure reserve as direct task execution, with the
Scheduler's existing maximum and checked arithmetic. The declared task timeout,
request deadline, lease boundary and execution permissions still constrain the
run. Lease expiry and the effective task deadline retain their typed ledger
records. Planning duration remains a reservation estimate; actual runtime usage
is recorded by the existing completion/failure owner.

For a scheduled contained task, an empty catalog outcome-reference set means
there is no scheduling-result consumer. The package may declare and produce its
own valid outcomes; its actual final page, designated effect and terminal
`scheduling_disposition` remain in GlobalLedger. Completion and recovery create
no policy outcome projection or completed-run consumption identity in this case.
When the reference set is nonempty, it must be a subset of the package's declared
outcome keys (Workflow #390 OUT-1, forced compromise O-03): a package may declare
outcomes the catalog does not read, and a run may produce one of them. A
referenced key the package does not declare refuses the run before any effect
(`policy_run_outcome_declaration_mismatch`). Package validity and execution
evidence requirements apply in both cases.

## Per-Instance Lanes

Workflow #427-A (plan r2, approved by Alice on 2026-10-28), behind
`scheduling.parallel.enabled` (default `false`;
[runtime-config.md](../runtime-config.md), "Per-instance lanes"). actingd reads the switch at
the start of every policy round, the initial round included; a run already handed to a worker
finishes and is settled there whatever the switch says afterwards.

- **One evaluator.** Evaluation and admission stay on actingd's policy driver thread. With the
  switch on, a granted scheduled dispatch is handed, with its request, to its instance's
  worker (`actingd-instance-<alias>`), which runs it and settles it exactly as the driver does
  with the switch off (the contained run, then the settlement of its scheduling result), and
  then wakes the driver: with the run's recompute wake when its settlement projected one,
  otherwise with `FactsChanged`, as a failed run does. One run per instance at a time; the
  runs of different instances overlap. The round never waits for a run, so its time budget
  (`policy_cycle_yielded`) keeps its evaluation meaning only, and the initial round no longer
  delays `runtime_ready`.
- **One owner from hand-off to settlement.** The hand-off registers the run as in flight on
  its instance, with its decision id, while its lease is still held; the registration ends
  only after the run's settlement (or its refusal's) is recorded. It is the one source of:
  the instance's occupancy (the evaluation defers, and the admission refuses
  `dispatch_instance_held` with `holder_kind` `policy_dispatch`, a new dispatch on a registered
  instance, also once the run's terminal is written and its lease released and before it is
  settled); the reconciliation (a run a worker still holds is skipped, never settled early or
  twice); and the waits of an instance's scheduling pause and of an install drain.
- **Lifecycle.** The worker takes a lifecycle (continuation) admission when it takes the run;
  its work covers the run and the settlement, so a drain or a stop waits for it. A run handed
  off before an install drain or a scheduling pause began goes ahead, as the round's own run
  admitted before them does with the switch off: the drain waits for it, and the pause drains
  it like any contained run admitted before its gate closed; neither records a failure for it
  (fix 1, review L1). Only a run the worker takes after the Runtime's stop was requested is
  refused (`runtime_stopping`): it never starts, its lease is released, and it gets a live
  interrupted settlement (`policy_settlement_interrupted`, with the elapsed `runtime_ms`, not
  the start's recovered shape), lifted after the restart like the start's, the first one of its
  pair (fix 2, re-review L-2; wording, re-review 2 L-A); its registration is
  ended. Nothing is replayed after a lost receipt.
- **Failures (#419).** The run's failures go through the instance's failure containment: an
  instance-level failure fences that instance and the other lanes go on; only a global fault
  stops the Runtime. A worker panic drops the run (not retried, its lease lapses at its TTL);
  its registration stays, marked abandoned, so the instance stays occupied. A run whose
  settlement failed without stopping the Runtime is left the same way. Once such a run's lease
  ended, the reconciliation settles it by the start's rule (fix 1, review M1): as at start, a
  lease that already ended without a run-linked release (its expiry, a release without run
  links, a transfer, or an expiry and a transfer as one end) gets no release, so the lease
  keeps its one end (fix 2, re-review M-1; #670 rulings 2 and 3); the run is settled once,
  from its terminal or, without one, as interrupted from that end
  (`policy_settlement_interrupted`, which pauses the task on that instance as a run cut by a
  restart does); its registration is forgotten and the instance is free again. A lease that
  ended in more than one way leaves the run held, and the next start records it
  (`policy_settlement_dispatch_left_open`).
- **Re-evaluation (C1, not behind the switch).** An evaluation whose validation finds its
  sampled outcome keys changed (`policy_outcome_snapshot_changed`: another run settled between
  the sample and the validation) is sampled again, at most three times per round, with one
  daemon line each (`actingd policy_cycle_resampled`); past that the round is given up
  (`actingd policy_cycle_given_up`) and deferred to its next trigger. It never stops the
  driver.
- **Shutdown.** The driver stops taking triggers and is joined; the workers hold no reference
  to actingd's host handle, so the host is released and closed, and its close joins the
  workers: a run a worker took finishes and is settled, one still queued is refused
  (`runtime_stopping`) and settled as interrupted, as above.

## Evaluation failures

ISO-PE (#419, 0.12.28; production v0.12.26 on 10-29 11:19, where one
`policy_evaluation_input_invalid` ended the policy driver and the Runtime with it). Not behind
the switch.

- **What stops the Runtime.** Only a failure whose scope is `Global`
  ([runtime-outcomes.md](../runtime-outcomes.md), the failure scope) is returned from an
  evaluation and ends actingd's driver, and with it the Runtime. On the evaluation path these are:
  a ledger read that failed (`ledger_failure`: the one ledger is the evaluation's source of
  truth); a poisoned lock (`runtime_state_poisoned`: a thread died while it changed the host's
  shared state); the Runtime clock (`monotonic_clock_regressed`, `system_clock_invalid`); the
  configuration in effect that cannot be read; the shared policy projection disagreeing with the
  ledger (`policy_outcome_projection_mismatch`, `policy_outcome_failed_run_residual`, and the
  Global list's `policy_admission_event_invalid`, `policy_state_ledger_incomplete`); and any
  condition the host latched (its monitor stops the Runtime whatever the driver does). Each of
  them leaves no instance's work trustworthy, so none is one instance's.
- **What is skipped.** Every other evaluation failure (an input the evaluator refuses, a
  configured input the Runtime does not trust, an authority conflict, one instance's fault) is
  recorded and skipped: `policy_evaluation_skipped` (warning) with the failure as its
  `caused_by` link, and the cycle is a deferred one with no evaluation, due
  `scheduling.evaluation.retry_ms` later ([runtime-config.md](../runtime-config.md), "Skipped
  evaluations"). The driver keeps its trigger and evaluates again then; nothing is dispatched
  from the skipped evaluation, runs already handed to workers go on, and the Runtime keeps
  running. `scheduling.evaluation.escalate_after` skips in a row record
  `policy_evaluation_awaiting_person` (error) once; an evaluation that evaluates ends the run of
  skips. Two failures that are not global keep their own bounded handling:
  `policy_outcome_snapshot_changed` is sampled again (above), and `outcome_projection_not_ready`
  waits one debounce, with or without the trigger's recompute wakes, while fewer than
  `escalate_after` evaluations in a row have not evaluated. Each such wait counts as one of them
  (the `count` of the run) and records nothing; once they reach `escalate_after`, the next
  `outcome_projection_not_ready` is skipped as above: recorded, evaluated again `retry_ms` later
  and escalated as every run of skips. (Until 0.12.28 a projection that stayed not ready was
  evaluated again every debounce, with no record and no bound, when the trigger had recompute
  wakes, and ended the driver when it had none.)
- **The detail.** `policy_evaluation_rejected` carries the policy's code as its `caused_by`
  link and the policy's code and developer text as its native detail. An input check that names
  what it refused gives that link `policy_input`, `policy_input_problem` and the `fact_key`,
  `instance_id`, `task_id` or `pool_id` it names; a time check (`observed_after_evaluation`)
  also gives the input's `observed_at_unix_ms` and the evaluation's `evaluated_at_unix_ms`.
- **The evaluation's time.** The Runtime clock is sampled for an evaluation once it holds
  `policy_outcome_gate` and `fact_write_gate`, the gates under which a settlement and every fact
  are written and under which it reads its inputs. Every input it reads was committed before
  that sample, so none was observed after the evaluation's time. (Until 0.12.28 the time was
  sampled as the evaluation started, before it waited for the gates; a settlement on an instance
  worker committed in between made the evaluation refuse its own inputs as "later than
  evaluation time".) An input can still lie ahead of the Runtime clock when the wall clock is set
  back: a settlement's time is its admission's wall time plus the run's monotonic duration. Such
  an evaluation is skipped until the clock passes it.

## Forward Planning And Maintenance

Forward planning is a bounded dry-run of the same pure evaluator used for live policy decisions. It projects at most 24 hours, performs no ledger write, lease operation, execution, or device action, and reports incomplete evidence instead of inventing resource effects. This is a projection facility, not another scheduler.

Predictive maintenance compares ledger-pinned execution duration and fact-confidence trends within an explicit lookback window. Both evidence series must meet their declared sample minimum before a recheck can be suggested. Missing evidence produces an `evidence_insufficient` assessment and no planning signal.

## Bounds

The compiler enforces both schema limits and UTF-8 byte limits:

| Item | Limit |
| --- | ---: |
| One document | 1,048,576 bytes |
| Selection document | 524,288 bytes |
| Catalog (all documents together) | 4,194,304 bytes |
| Priority offsets per evaluation | 16,384, each within ±1,000,000 milli |
| Identifier/reference | 128 bytes |
| Registered instance alias | 256 UTF-8 bytes |
| Diagnostic text, fact string, or source URI | 1,024 bytes |
| Approval references | 64 |
| Tasks | 4,096 |
| Pools | 1,024 |
| Activity profiles | 1,024 |
| Timeline events | 4,096 |
| Predicate depth | 16 |
| Predicate nodes per root | 512 |
| Effects, references, or instance overrides per task | 128 each |
| Windows or goals per activity profile | 128 each |
| Pool valuation name / unit | 128 / 32 UTF-8 bytes |
| Pool valuation scale | 1..=9,007,199,254,740,991 |
| Pool valuation base weight / gap weight | 0..=1,000,000 / 1..=1,000,000 milli |

Loop budgets are mandatory in V1; in V2 each run-count bound is optional (SS-3, "Optional run-count bounds"). Arrays and strings that exceed their limit reject the entire catalog. Duplicate identifiers, duplicate object keys, and unbounded recursive input are invalid.

## Canonical Serialization And Hash

The catalog hash is computed as follows:

1. Parse all four documents while rejecting duplicate object keys and invalid UTF-8. Parse the optional selection document with the selection-policy crate's own reader and validation.
2. Validate the exact V1 schemas and cross-document invariants.
3. Construct the JSON object `{"activity": A, "pools": P, "tasks": T, "timeline": L}` from the validated documents, adding `"selection": S` only when a selection document is present. No field is removed and no default is inserted, so a catalog without a selection document keeps its existing hash. In the same way a pool's `valuation`, and the `gap` inside one, appear only when declared: a catalog without them keeps its existing hash, and a declared block is hashed field by field, `name` and `unit` included, so changing any of its values is a new catalog identity that needs its own approval.
4. Serialize that object with RFC 8785 JSON Canonicalization Scheme. Object keys are sorted by JCS rules, array order is preserved, and no insignificant whitespace is emitted.
5. Compute SHA-256 over the canonical UTF-8 bytes.
6. Encode the result as `sha256:` followed by 64 lowercase hexadecimal characters.

Catalog producers must emit semantically unordered arrays in deterministic order. Because array order is preserved, reordering any array changes the hash. Approval references authorize the exact catalog version and hash; they are not executable permissions by themselves.

## Diagnostics

Compiler failures use `CatalogDiagnostic`. `code` is stable within V1; `reason` is human-readable and must not be parsed for control flow. `json_path` uses RFC 6901 JSON Pointer. `source` identifies the document, source URI, and one-based line and column. Version and catalog fields may be null only when malformed input prevents their extraction.

Any error-severity diagnostic rejects the complete four-document catalog. Warnings may accompany a successful dry-run but cannot conceal an error. The compiler must sort diagnostics deterministically by document, source position, code, and JSON path.

## Compiler Boundary

`actingcommand_policy::compile_catalog` accepts four in-memory `CatalogDocumentSource` values plus an optional fifth for the selection document. It performs no file access, script execution, network request, clock read, sleep, ledger write, lease operation, or device action. Success returns one complete `CompiledCatalog`; any error returns `CatalogCompileFailure` and no partial IR. Both outcomes expose canonical, byte-stable dry-run JSON.

## Neutral Example

`examples/catalog-a` is a synthetic, product-neutral catalog. It exercises all four documents without embedding external project, game, account, device, or private workflow data.

The example is the V1 upgrade map for the earlier `task-catalog.v0-draft` shape:

| Draft concept | V1 location |
| --- | --- |
| task identifier and operation | `tasks[].id`, `tasks[].entrypoint` |
| start condition | `tasks[].trigger` |
| feedback termination | `tasks[].feedback_stop` |
| resource assumptions | typed `tasks[].consumes` and `tasks[].produces` plus `pools[]` |
| retry behavior | `tasks[].on_failure` |
| loop and session bounds | `tasks[].loop_budget` and `activity.profiles[]` |
| schedule/reset/deadline data | `activity.profiles[].windows` and `timeline.events[]` |
| instance-specific priority | `tasks[].instance_overrides` |
| instance importance and targets | instance-scoped `activity.profiles[].importance_milli` and `goals` |

Draft fields that relied on runtime defaults or implementation-specific behavior have no implicit V1 mapping and must be supplied explicitly.
