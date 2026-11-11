# Standard-pack schedule rules (`schedule.json`)

ROUT-L (frozen model `rd-1024/rout-lean/model.md` v1.0, §3.1–§3.2, §3.5–§3.6; slice RL-1a).
Alice's ruling 51 (#386, #405): the schedule rules live in the standard package and the
scheduler workbench loads them when they are used, with no approval step. This document is the
file format and the composition; the workbench in the Runtime (loading, unloading, last good
rules, activation) is RL-1b's and is described in "The scheduler workbench".

The core stays game-neutral: the Runtime compares every id, package and server as an opaque
string and names no game here.

## The file

Each game root may hold `<standard_packs.root>/<game>/schedule.json` beside `bundle.json`
([package-reference.md](package-reference.md), "Standard package root"). It holds that game's
V2 tasks ("rules"), pools, activity profiles and timeline events
([scheduling/v2/README.md](scheduling/v2/README.md)), and names no instance, no scope, no
package path and no approval:

```text
{
  "schema_version": "actingcommand.schedule.v1",
  "requires": [ "<subsystem>", ... ],
  "rules":    [ a V2 task without scope, entrypoint and procedure_ref, with "package_id",
                an optional "maintenance": true, an optional "realization": true and an
                optional "limited": true ],
  "pools":    [ a V2 pool without scope, with an optional "server" (its V2 members, such as
                "stop_line" and "hard_line", pass through) ],
  "activity": [ a V2 activity profile without scope, with an optional "server" ],
  "timeline": [ a V2 timeline event without scope, with an optional "server" ],
  "held":     [ { "package_id", "reason", "reference"? } ]
}
```

SCHED-C1a (register §1.299, since 0.12.52) adds to a rule an optional `"fixed_value_milli"` with an
optional `"fixed_value_origin"` ("The fixed value" below), and to a pool an optional
`"valuation_origin"` ("Provenance" below). SCHED-C1c (register §1.299, since 0.12.53): a pool's V2
member `"trigger_line"` passes through as the other two lines do.

Every member but `schema_version` may be absent, which means empty. The reader
(`actingcommand_contract::parse_standard_pack_schedule`) refuses, each as
`standard_pack_root_invalid` with `standard_pack_problem` and the JSON pointer of the member:

| Rule | Problem |
|---|---|
| UTF-8 JSON of at most 1 MiB, no duplicate key at any depth, no trailing data; an object | `schedule_invalid` (pointer empty) |
| `schema_version` is `actingcommand.schedule.v1`; no member besides the seven above | `schedule_invalid` |
| No member named `scope` at any depth of a rule, pool, profile or event (the item's own and those of predicate nodes such as `fact`, `record_deadline`, `fact_time_reached`) | `schedule_invalid` at the `scope` member |
| A rule has a string `package_id` (1–128 bytes, no control character), no `entrypoint` and no `procedure_ref`; its `instance_overrides` is absent or empty | `schedule_invalid` at the member |
| A rule's `maintenance` (SS-2c, since 0.12.32; "Judged rules" below), when written, is `true` | `schedule_invalid` at `maintenance` |
| A rule's `realization` (SS-2d, since 0.12.35; "The realization mark" below), when written, is `true`, and a rule that declares `"maintenance": true` does not write it | `schedule_invalid` at `realization` |
| (SCHED-C1a, since 0.12.52; "The fixed value" below) a rule that declares `"maintenance": true` does not write `fixed_value_milli`; a rule's `fixed_value_origin`, when written, is `"model"`, beside a `fixed_value_milli` | `schedule_invalid` at the member |
| (SCHED-C1a, since 0.12.52; "Provenance" below) a pool's `valuation_origin`, when written, is `"model"`, beside a `valuation` that is a block (not the pending marker) | `schedule_invalid` at `valuation_origin` |
| Every rule, pool, profile and event is an object with a string `id`, unique within its list | `schedule_invalid` at `id`; a repeated id is `rule_id_conflict` |
| A pool, profile or event names its `server`, if at all, as a game-root identifier | `schedule_invalid` at `server` |
| `requires` lists at most 32 distinct names of 1–64 bytes of `[a-z][a-z0-9_]*` | `schedule_invalid` at the entry |
| `held` lists at most 4096 entries `{package_id, reason, reference?}` with distinct package ids, a non-blank `reason` and `reference` of at most 1024 bytes without a control character | `schedule_invalid` at the member |

The V2 members of a rule or an item are not read here; the catalog compiler checks them after
composition, and its diagnostics are mapped back to the rule or item they came from (see
below).

Against the game root (`validate_standard_pack_schedule_references`):

- every rule's and every `held` entry's `package_id` is a package of the root's registry
  (`bundle.json`): otherwise `rule_package_unregistered` at that `package_id`;
- every `server` a pool, profile or event names is a server of a registry package: otherwise
  `schedule_invalid` at `server`.

**Servers.** A rule belongs to the server of its package in `bundle.json`. A pool, profile or
event without `server` belongs to every server of the game.

**Ids.** Rule, pool, profile and event ids are unique within the file and across the games in
use. The resource side prefixes them by game; the Runtime does not rename them.

**`requires`** names the configuration-manifest subsystems the rules need (`config_manifest`
subsystems, for example `condition_edges`). A Runtime that loads the rules and does not name one
refuses the file with `schedule_requires_missing` at `/requires/<index>`. The resource tools
have no Runtime manifest at hand: they report `requires` and leave the check to the Runtime.

A rule whose `trigger` or `feedback_stop` holds a `fact` predicate with a fact reference
(`"value": {"type": "fact", "fact_key", "offset"}`, #386 CF-4,
[scheduling/README.md](scheduling/README.md), "Fact references") requires `fact_reference`. The
composer refuses a file whose rule writes one while `requires` does not name it, Runtime and
resource tools alike: `schedule_invalid` at the first such `value` in file order (for example
`/rules/0/trigger/predicates/1/value`). So a Runtime that cannot read a reference refuses the
file at `requires` instead of misreading the rule. The reference names no scope; the composer
fills the predicate's own.

A pool whose `capacity`, or a rule whose `resource_projection` `value`, is such a fact reference
(#386 CF-5, [scheduling/README.md](scheduling/README.md), "Pool fact references") requires
`pool_fact_reference`, alike: the composer refuses a file that writes one while `requires` does
not name it, `schedule_invalid` at the first in the order rules (their `trigger`, then
`feedback_stop`, as above) then pools (for example `/rules/0/trigger/value`, then
`/pools/0/capacity`). A file that writes both a `fact` reference and one of these names both
subsystems. Each reference reads its fact in the pool's scope, which the composer fills.

A pool that writes `stop_line` or `hard_line`, its two water lines (SS-2h, since 0.12.46, register
§1.289; [scheduling/README.md](scheduling/README.md), "Pool lines"), as a fact reference or a
constant, requires `pool_lines`, alike: the composer refuses a file that writes one while
`requires` does not name it, `schedule_invalid` at the first such pool's member in file order
(for example `/pools/0/stop_line`). The members pass through: the composer copies them into each
composed pool as written (they name no id and no scope; a reference names a fact of the pool's
own scope, which the composer fills), and the catalog compiler checks them (`pool_lines_invalid`
names the member). A reference form needs `pool_lines` only.

SCHED-C1a (register §1.299, since 0.12.52). A rule that writes `fixed_value_milli` or a pool that
writes `valuation_origin` ("The fixed value", "Provenance" below) requires `benefit_fixed_value`,
and (SCHED-C1c, since 0.12.53) a pool that writes `trigger_line`
([scheduling/README.md](scheduling/README.md), "Pool lines", "The trigger line") requires
`pool_trigger_line` beside `pool_lines`, alike: the composer refuses a file that writes one while
`requires` does not name its subsystem, `schedule_invalid` at the first such member in file order
(rules, then pools; for example `/rules/0/fixed_value_milli`, `/pools/0/trigger_line`).

SCHED-C1f (register §1.303; SCHED-C1f1, since 0.12.54). A rule whose `consumes` entry writes its `amount` as a
fact ("A run's consumption read from a fact" below) requires `consumption_fact`, alike: the composer
refuses a file that writes one while `requires` does not name it, `schedule_invalid` at the first
such `amount` in file order (for example `/rules/0/consumes/0/amount`).

A timeline event that writes `excludes_fact_prefixes`, with any value (Workflow #411 P19, since
0.12.25; [scheduling/README.md](scheduling/README.md), "Reset exclusions and the task's own last
result"), requires `reset_exclusions`, alike: the composer refuses a file that writes it while
`requires` does not name it, `schedule_invalid` at the first such member in file order (for
example `/timeline/0/excludes_fact_prefixes`). A file that relies on a reset keeping a rule's own
last result (a rule's `dependency_completed` on itself, its `since_own_run` or its
`once_per_reading` read through a reset that clears its `task.` state) names `reset_exclusions`
as well, so an older Runtime refuses the file instead of re-running the rule after each reset; the
composer cannot see that reliance and does not require it. A rule that uses a `dependency_completed`
on itself as the result of the current period (for example a retry branch on its own `failed`) now
reads its last result across the reset, from before it; a retry branch that must fire only within
the current period needs, as its period gate, a `dependency_completed` on an earlier rule of the
same period, which the reset clears.

### Self-read fact probes

A rule's `fact` predicate that writes `on_unknown` (SS-5, since 0.12.32;
[scheduling/README.md](scheduling/README.md), "Self-read fact probe") requires
`fact_unknown_probe`, alike: the composer refuses a file that writes it while `requires` does not
name it, `schedule_invalid` at the first such member in file order (a rule's `trigger`, then its
`feedback_stop`; for example `/rules/0/trigger/on_unknown`).

The probe admits the rule's task while the fact is unknown so that the run of the rule's own
package reads it, so its `fact_key` must be a key that package's reading marks write
([resource-readings.md](resource-readings.md), "Declaration": a mark's `fact_key` or
`capacity_fact_key`). Where a rule meets its package, `actinglab standard-pack check` and
`package bundle --schedule` read each probing rule's package with the Runtime's metadata reader
(its `resource_readings`, without preparing it), after the composition and the store update rule,
and refuse a probe whose key the package does not mark (a key only another package reads, for
example the recognition maintenance package's, or one no package reads):
`standard_pack_root_invalid`, `schedule_invalid` at its `on_unknown`. The composer has no package
content and the Runtime's compiler checks only what it can see (an instance scope, which the
composer fills; a `resource.` or `inventory.` key; a place in the trigger outside every `not`).

### A run's consumption read from a fact

SCHED-C1f (register §1.303; SCHED-C1f1, since 0.12.54; the coordinator's work order of 11-11 and the
coordinator's rulings of 11-11 14:5x on its nine questions, with this draft's names, not Alice's
words; [scheduling/README.md](scheduling/README.md), "A run's consumption read from a fact"). A consumer whose one run's amount of a pool is a state of the account (what the
player fields), which its pack reads at each run and writes as an instance fact, writes that amount
as the fact it reads, with the amount a run is judged by until the fact exists:

```json
{"pool_id": "<pool>", "direction": "consume", "amount": {"type": "fact", "fact_key": "resource.neutral.per_run", "fallback": 3}, "observation_source": "self_reported", "confidence_milli": 1000}
```

- **Composition.** The entry passes through as every V2 member of a rule: the composer copies the
  `amount` object into each composed task as written and fills no scope in it (it names none; the
  evaluation reads its `fact_key` in the scope of the instance it decides for, which for a composed
  task is the task's own); a `scope` member there is refused by the reader as at any depth ("The
  file"), and the entry's `pool_id` is rewritten for an instance copy as before. The catalog compiler checks the
  rest (the form's refusals are mapped back to the rule's `consumes/<j>/amount`).
- **`requires`** names `consumption_fact` ("`requires`" above).
- **Coverage** ("Judged rules"). An entry with the form has its quantity: it counts as a declared
  quantity, so the rule is *complete* when every pool it touches has a valuation, as with a
  constant `amount`.
- **The tools** ("Tools") answer, per game, `consumption_facts`: one `{rule_id, pool_id, fact_key,
  fallback}` per entry with the form, in file order (the file's ids), empty when none (SCHED-C1f1:
  of the rules not held, the ones composed and so checked by the compiler, as the coverage counts
  them; absent from an answer without `schedule.json`). Nothing
  checks the key against the rule's package (the coordinator's ruling 4; a later hint can be added
  without a shape change): a key that no run writes leaves every decision on its fallback, which
  the decisions show (`consumption_source` `fallback`).
- **What a resource repository writes.** A consumer of the row "Consumer of a regenerating pool"
  ("What a resource repository writes") whose amount is a state of the account writes it as the
  form instead of a constant, its fallback the amount a run of the usual setup consumes. The pack
  chooses how long its reading holds by the validity it writes, and keeps the key out of its
  resets (`excludes_fact_prefixes`) when the amount outlives a day: a reading that expired or that a
  reset invalidated is read as none, and the fallback decides. A pack that wrote such an
  amount as a constant replaces it by the form with that constant as its fallback, which keeps a
  capped episode under way ([scheduling/README.md](scheduling/README.md), "An episode under way and
  a round's frozen order").

## Classification

Every package of the game root's registry is one or more of (R6, R7):

1. **maintenance-role** by the root files (`standard_pack_maintenance_packages`): a
   `bundle.json` `maintenance[]` entry (startup, return home, recognition), a `navigation.json`
   edge or recovery-chain package, an `interrupts.json` handler or `then.run` package, or a
   `pages.json` feature package;
2. **scheduled**: named by a rule and not held;
3. **held**: listed in `held` with its reason (text for people; nothing evaluates it by
   machine). A rule that names a held package is not composed.

A package that is none of these is **unclassified**: it is not scheduled, and each is one
`standard_pack_package_unclassified` warning (keys `game`, `package_id`). The rest of the game
still runs.

**Judged rules** (SS-2b, since 0.12.32; Alice's rulings 104 point 3 and 116; register §1.225).
The composer marks every composed rule whose package is not maintenance-role (item 1)
`benefit_judged: true`: a business rule, ranked in the business tier and judged by its net
benefit ([scheduling/README.md](scheduling/README.md), "Net benefit within a tier"). Its
candidates are not dispatched while that benefit is unknown or not positive: until the rule
declares its `produces` and `consumes` and every pool they touch its `valuation` (SCHED-C1a, since
0.12.52: or its fixed value, with a valuation on every pool it consumes, "The fixed value"
below), business dispatch waits (ruling 116: 「当业务有了数据之后直接写进任务包」). A maintenance-role package's rule is
not marked and ranks as before. The mark is the composer's: a rule that writes `benefit_judged`
itself is refused, `schedule_invalid` at `/rules/<i>/benefit_judged`. Where a value is not known
yet the resource side writes the marker `"pending"` (a pool's `valuation`, an entry's `amount`;
[scheduling/v2/README.md](scheduling/v2/README.md)); it changes nothing but the count below.

**The maintenance declaration** (SS-2c, since 0.12.32; Alice's ruling 104 point 1: the
maintenance tier is startup, return home, recovery, readings maintenance and store update,
everything else business; register §1.227). The root files give a package a maintenance role
(item 1) for only part of that tier: no root file names a store update package, and a readings
package beyond a server's recognition package, or a return-home or recovery package that no
navigation edge, recovery chain or bundle use names, is business by item 1. So a rule may
declare itself maintenance:

```json
{"id": "<rule>", "package_id": "<package>", "maintenance": true, ...}
```

- The composer then does not mark the rule `benefit_judged`, whatever its package's role: it is
  unjudged, ranks in the maintenance tier in the order of before and runs as it ran before
  SS-2b. Nothing else changes: not its `priority`, its ranking within the tier, nor any gate.
- The member is `schedule.json`'s, not a V2 task member: the reader takes it out of the rule
  (`ScheduleRule::maintenance`), so the composed task never carries it, and a file that does not
  write it composes byte for byte as before (same documents, catalog hash and rules digest). A
  raw V2 catalog marks its tasks with `benefit_judged` itself, as before; a V2 task that writes
  `maintenance` is `unknown_field`.
- Only `true` declares. Any other value, `false` included, is refused, `schedule_invalid` at
  `/rules/<i>/maintenance`: `false` on a rule whose package is maintenance-role would read as
  "business" and change nothing (the role still exempts it), and one spelling for "not
  declared", no member, keeps the files uniform for the resource side's check.
- Declared on a rule whose package is maintenance-role already, it is redundant and harmless:
  the composition is byte for byte the one without it.
- The declaration is per rule, the role per package: two rules of one package may differ.

- **Coverage** (the measure of 「业务覆盖完整」). Each judged rule of the file (its package not
  maintenance-role, not declared maintenance, not held) counts once, whatever the instances it
  is composed for: *complete*
  when it has an entry, every entry its quantity and every pool it touches a valuation;
  *pending* when it is not complete and every value it lacks is the marker; *undeclared* when it
  has no `produces` and no `consumes` entry, or touches a pool with neither a valuation nor the
  marker. Each undeclared rule is one `standard_pack_benefit_undeclared` warning (keys `game`,
  `task_id` the rule's id; `pool_id` the pool without a valuation, when that is why), a new code
  because no code says "a business rule declares no value": `standard_pack_package_unclassified`
  names a package nothing schedules. SS-2c: *maintenance_declared* counts the rules, not held,
  that declare themselves maintenance, whatever their package's role; none of them is judged, so
  none counts as complete, pending or undeclared. SS-2d: *realization* counts the judged rules
  that carry the realization mark (below); each also counts as complete, pending or undeclared.
  SCHED-C1a (since 0.12.52): a rule that writes a fixed value ("The fixed value" below) is
  *complete* when every pool it consumes has a valuation (its `produces` are not read), *pending*
  when every value it lacks is the marker, and *undeclared* only for a consumed pool with neither;
  *fixed_value* counts these rules, each also counted in one of the three.
  COV-1 (register §1.299, since 0.12.53; the coordinator's ruling of 11-11, not Alice's words): a
  judged rule that would be undeclared for having no `produces`, no `consumes` entry and no fixed
  value is a *follower* instead when, by its trigger alone, it can only be due in its chain head's
  run window ([scheduling/README.md](scheduling/README.md), "Ties and chain heads") and rides with
  a rule whose value the file declares: (1) its trigger requires another rule by an *edge*, a
  `dependency_completed` reached from the root only through `all` whose `terminal_states` is
  `succeeded` alone and which writes `since_own_run: true` ("Condition edges" there), so that it
  is due only while that rule's latest run succeeded and it has not served that run; (2) the rule
  an edge names is a judged rule that counts as *complete* (a fixed value included) or is itself
  a follower: the walk goes up the edges in trigger order, depth first, each rule once, to the
  first complete rule, the one the follower rides with, and a walk that reaches none (a cycle, or
  only rules that are not judged, are *pending* or are *undeclared*) makes no follower; (3) every
  chain link of its trigger (each `dependency_completed` reached through `all` whose
  `terminal_states` holds `succeeded`, naming another rule) names a judged rule, since the
  evaluator walks up from the link dispatched last and judges on its own a candidate whose walked
  links are all unmarked. A rule without an edge (its dependencies lack `since_own_run`, also
  accept another state, or stand under an `any` or a `not`) can be due outside its head's window,
  where the evaluator judges it on its own: it stays undeclared, with its warning, and so does a
  rule that touches a pool without a valuation, whatever its trigger. *follower* counts the followers; none of them counts as undeclared or is
  a `standard_pack_benefit_undeclared`. The tools answer it as `benefit_coverage.follower`, absent
  while 0, and list the followers as `benefit_followers: [{task_id, rides_with}]`, rule ids in
  file order, absent while empty, so an answer for a file without followers is byte for byte the
  one of before. The count is a static reading of the file and decides nothing: whether a
  candidate is judged at an evaluation is the evaluator's
  ([scheduling/README.md](scheduling/README.md), "Who is judged"), and its decision's reasons
  show it.
  Only the resource tools report the coverage and these warnings ("Tools"); the Runtime's
  workbench records neither.

**The realization mark** (SS-2d, since 0.12.35; Alice's ruling 127, the review's version; register
§1.235; design `rd-1031/design-acm-price-addendum2.1.md` §7.3). Only the model knows whether a
business rule is a realization or conversion with a real downstream use, one more run of which
wastes nothing; `acmodel bindings apply` writes its verdict as

```json
{"id": "<rule>", "package_id": "<package>", "realization": true, ...}
```

- The composer then writes `benefit_realization: true` on the judged rule's task
  ([scheduling/v2/README.md](scheduling/v2/README.md)): its candidate within its rounding error
  may run in the last place of its tier ([scheduling/README.md](scheduling/README.md), "Net
  benefit within a tier", "Within tolerance"). The member is `schedule.json`'s, taken out of the
  rule by the reader (`ScheduleRule::realization`), so the composed task never carries
  `realization`; a file that does not write it composes byte for byte as before.
- Only `true` marks; any other value, `false` included, is refused, `schedule_invalid` at
  `/rules/<i>/realization`, as `maintenance` is. A rule that declares `"maintenance": true` is
  never judged, so the reader refuses the mark beside it; a rule whose package is
  maintenance-role is refused by the composer at the same pointer. A rule that writes
  `benefit_realization` itself is refused at `/rules/<i>/benefit_realization`: the task member is
  the composer's.
- The compiler's requirements on the composed task (a `consumes` and a `produces` entry, a reserve
  on and a fact reading of each consumed pool, no cycle) are mapped back to the rule as every
  compiler diagnostic is; `realization_cycle` names the rule's `realization`.

**The limited mark** (SS-2h, since 0.12.46; register §1.289; [scheduling/README.md](scheduling/README.md),
"Capped spending order"; design `rd-1031/design-c424-redline-1109.md` §1.6 item 6). Whether a
rule's runs are counted by a limit (the activity's own limit, or an income cap that counts it) is
the model's to say; `acmodel bindings apply` writes it as `"limited": true` on the rule. It is
written as the realization mark is:

- The composer writes `benefit_limited: true` on the judged rule's task
  ([scheduling/v2/README.md](scheduling/v2/README.md)): from a capped episode's second round its
  candidate is not listed. The member is `schedule.json`'s: the composer leaves it out of the
  composed task, so a file that does not write it composes byte for byte as before.
- Only `true` marks; any other value, `false` included, and the mark on a rule that declares
  `"maintenance": true` are refused, `schedule_invalid` at `/rules/<i>/limited`; a rule whose
  package is maintenance-role is refused at the same pointer. A rule that writes
  `benefit_limited` itself is refused at `/rules/<i>/benefit_limited`: the task member is the
  composer's. A compiler diagnostic at the task's `benefit_limited` names the rule's `limited`.

**The fixed value** (SCHED-C1a, register §1.299, since 0.12.52; the coordinator's rulings of 11-11,
not Alice's words, after her two kinds of scoring of 11-09 15:47 and 15:50;
[scheduling/README.md](scheduling/README.md), "Fixed values written in the pack"). A business rule
whose value is settled is written with that value, in milli of its game's one value unit (the unit
its pools' valuations weigh in):

```json
{"id": "<rule>", "package_id": "<package>", "fixed_value_milli": 12000, ...}
```

- The composer then writes `benefit_fixed_value_milli` with the same integer on the judged rule's
  task. The member is `schedule.json`'s, taken out of the rule by the reader (`ScheduleRule`), so
  the composed task never carries `fixed_value_milli`, and a file that does not write it composes
  byte for byte as before. A raw V2 catalog writes the task member itself.
- A rule that declares `"maintenance": true` is never judged, so the reader refuses the value
  beside it; a rule whose package is maintenance-role is refused by the composer at the same
  pointer, `/rules/<i>/fixed_value_milli`. A rule that writes `benefit_fixed_value_milli` itself is
  refused at `/rules/<i>/benefit_fixed_value_milli`: the task member is the composer's. The
  compiler's refusals of the task member (its range, `missing_required_field`, and
  `fixed_value_conflict` beside a priced product or the realization mark) name the rule's
  `fixed_value_milli`, as every compiler diagnostic is mapped back.
- A consumer writes its `consumes` beside its value, and the pool it consumes needs a valuation
  for its cost: the model's, or a constant block the pack writes on the pool
  ([scheduling/README.md](scheduling/README.md), "Pool Valuation", "A valuation written in the
  pack").
- `"fixed_value_origin": "model"` says that the model's export wrote the number (`acmodel bindings
  apply`); without it the pack's own hand wrote it. Only `"model"` is a value, and only beside
  `fixed_value_milli`. It decides nothing: only "Provenance" below reads it.
- The rule requires `benefit_fixed_value` ("`requires`" above).

**Provenance** (SCHED-C1a, since 0.12.52; the coordinator's point 6). So that a later model export
can replace them one by one, the composer reports, per game, which judged rules run on a fixed
value and which pools on a declared valuation block, each by who wrote it: `pack` (no origin
member) or `model` (`fixed_value_origin` or `valuation_origin` `"model"`). Absent means the pack
because a hand edit is how a number gets into the file without the model; the model's writer
marks what it writes, and a value it left unmarked stays listed as the pack's, on the list to
replace.

- A pool marks a valuation the model wrote with `"valuation_origin": "model"` beside its `valuation`
  block. The member is `schedule.json`'s: the reader takes it out of the pool as it takes `server`,
  so the composed pool never carries it.
- The tools answer `benefit_provenance` for each game ("Tools"): `{fixed_value: {pack: [rule ids],
  model: [rule ids]}, valuation: {pack: [pool ids], model: [pool ids]}}`, in file order.
- (SCHED-C1b, register §1.299, since 0.12.53) The workbench holds the active generation's
  provenance, every game's, in the program fact
  `config.catalog_provenance` (runtime scope, re-observed; [runtime-config.md](runtime-config.md),
  "Facts", its row with the code): one count row `{game, kind, origin, count}` per game, kind
  (`fixed_value`, `valuation`) and origin, then one row `{game, kind, origin, id}` per rule or
  pool, within the fact's 256 rows and cut as `config.catalog_warnings` is (the id rows first, the
  last row saying how many of each are missing). It is recorded after each composition whose rules
  digest the active generation has (so also when only an origin changed, which changes no digest),
  only when it changed, and never while absent with nothing to hold. `actingctl overview` and
  `ac_overview` show it in their `catalog` block as `benefit_provenance {fixed_value {pack, model},
  valuation {pack, model}}` (counts), with the ids in `items` ([runtime-interface.md](runtime-interface.md),
  with the code).
- An origin changes no decision and no catalog: two files that differ only in it compose the same
  catalog hash and rules digest.

**What a resource repository writes** (SCHED-C1; the coordinator's point 7). By rule class, for a
game's business rules:

| Rule class | The rule writes | Its pool writes | Decided by |
|---|---|---|---|
| Collect and claim (a reward, mail, a check-in, a finished job's collection: no pool consumed) | `fixed_value_milli` | — | `B = V`, by `B ÷ E` in the business tier |
| Consumer of a regenerating pool | its `consumes` (one run's amount), `fixed_value_milli` (`V`, its value without the pool's cost), `"limited": true` where a limit counts its runs; no "reading ≥ capacity − margin" threshold once the pool is valued ("Tools", the redundant trigger) | a constant `valuation` (`scale` Q, `base_weight_milli` W) beside its `stop_line` and `hard_line` (and `trigger_line`), `H = S − (the costliest run)` | below the trigger line `V − W·c`; from the trigger line the capped order by `V ÷ c`. A pack that wants a consumer to run only in capped rounds writes its pool's `W` so that `W·c ≥ V` for that consumer; a consumer meant to spend the reserve below the stop line (a time-limited, high-value run) has `V > W·c` and is still held by the hard line: the reserve Alice's planned red line keeps (11-07, #428 6706630435: 「不让资源达到上限，但是保留一些资源用于某些限时的，收益高的任务」) |
| Reading and navigation step inside a chain | a readings, return-home or recovery step: `"maintenance": true`; a step after the chain's first business rule: nothing (it rides with that rule, SS-2e); the chain's value once, on its first business rule | — | the maintenance tier, or its chain |
| Still needs the model (its value depends on live data or on a choice made at run time) | nothing, or `"amount": "pending"` | `"valuation": "pending"` | `benefit_unknown` until the model's export |

## Composition

`actingcommand_policy::compose_standard_pack_rules` is the one composer. The Runtime's
workbench, check-config and the resource tools all call it. It is pure: no file, clock or
ledger.

**Inputs.** The bound instances (alias → game and server, from `policy.facts.instances`); for
each game root that holds `schedule.json`, that file's bytes, the root as
`read_standard_pack_root` reads it, and the bundle's `maintenance[]` entries; the subsystems
`requires` must name, or none (the tools); the catalog version.

**Steps.**

1. **Games in use**: the games of the bound instances, in name order; the instances of each in
   alias order. A game without a bound instance is not read. A bound game whose root holds no
   `schedule.json` has no rules.
2. **Read** each game's file (above): the reader, `requires` (it names `fact_reference` when a
   rule writes a fact reference, `pool_fact_reference` when a pool's `capacity` or a
   `resource_projection`'s `value` is one, `pool_lines` when a pool writes `stop_line` or
   `hard_line`, SCHED-C1c (0.12.53) and SCHED-C1a (0.12.52): `pool_trigger_line` when one writes
   `trigger_line` and `benefit_fixed_value` when a rule writes `fixed_value_milli` or a pool
   `valuation_origin`, SCHED-C1f1 (0.12.54): `consumption_fact` when a rule's `consumes`
   entry writes its `amount` as a fact,
   `reset_exclusions` when a timeline event writes
   `excludes_fact_prefixes`, `fact_unknown_probe` when a rule's `fact` predicate writes
   `on_unknown`, and the given subsystems name each entry), no timeline event
   invalidating `health.application.update_required` ("The store update rule"), the references
   and the classification. A refused game is reported with its game id.
3. **Instantiate**: for each bound instance `i` of game `g` on server `s`, every rule whose
   package belongs to `s` and is not held, and every pool, profile and event for every server or
   for `s`, each with every scope filled with `{kind: instance, instance_id: i}` (the item's own
   and that of each predicate node that reads a fact, the nodes with a `fact_key`); a rule's
   `entrypoint.operation_id` is `operation.package.run`, its `procedure_ref` is
   `package.<package_id>`, and its `instance_overrides` is empty; SS-2b: a rule whose package
   is not maintenance-role gains `benefit_judged: true` ("Judged rules" above), SS-2c: unless it
   declares itself maintenance (`maintenance`, which the composed task does not carry); SS-2d:
   such a rule that carries `"realization": true` also gains `benefit_realization: true` ("The
   realization mark" above; the composed task does not carry `realization`); SS-2h: and one that
   carries `"limited": true` gains `benefit_limited: true` ("The limited mark" above; the composed
   task does not carry `limited`); SCHED-C1a (since 0.12.52): and one that carries
   `fixed_value_milli` gains `benefit_fixed_value_milli` with its value ("The fixed value" above;
   the composed task carries neither `fixed_value_milli` nor `fixed_value_origin`, and a composed
   pool no `valuation_origin`).
4. **Ids.** An item copied to exactly one instance keeps the file's id, so one instance per
   (game, server) composes ids byte-identical to the file. An item copied to two or more
   instances gets the id `<id>@<alias>` in each copy; every reference inside that copy (an
   `event_id`, a `pool_id`, a `task_id` of a predicate, an effect, a reserve, a pool
   observation or a goal metric) is rewritten to the copy's id; and each rule copy's
   `continues_from` gains `<id>` (Workflow #372 K5), so a later change from one instance to two
   keeps the first instance's budgets. Copies are counted per item: with instances on two
   servers of one game, a pool, profile or event without `server` has two copies and is suffixed.
5. **Join and compile**: one V2 catalog `{catalog_id: "standard-packs", catalog_version,
   approval_refs: []}` of the games in name order, compiled by the catalog compiler with the
   4 MiB catalog bound for each document instead of 1 MiB (`compile_composed_catalog`). Two
   composed rules, pools, profiles or events with one id are `rule_id_conflict` at the later
   one's `id` (games in name order); a package id that another game's composed rules already
   run (one `procedure_ref` for two games) is `rule_id_conflict` at the rule's `package_id`. A
   compiler diagnostic is `schedule_invalid` at the pointer of the rule or item it came from in
   that game's file, with the diagnostic's code in the developer text.

**Output.** The four V2 documents and the compiled catalog; the manifest (one entry per
`procedure_ref`: game, `package_id`, and the package directory below the game root); each game
in use with its instances, the `sha256:` of its file, its `requires`, its task count and its
classification; and the rules digest, `sha256:` over the canonical JSON of the compiled tasks,
pools, profiles and events (without the descriptor) and the manifest. Equal digests mean nothing
changed.

**No rules** is a valid state: no bound instance, no `schedule.json`, or a file without rules
composes a valid empty catalog (V2 accepts an empty task list and an empty `approval_refs`).

**Two priority tiers** (SS-2, since 0.12.32; Alice's ruling 104 point 1; register §1.222). A
game's rules carry two priorities: maintenance tasks (startup, return home, recovery, readings
maintenance, store update) one high value, every business task one normal value. A chain step
needs no priority of its own to follow its chain: the evaluator ranks it with its chain head
([scheduling/README.md](scheduling/README.md), "Ties and chain heads"), and business tasks of one
tier are ordered by the ranking and its tie-break, the rules' order in this file last. SS-2b
(since 0.12.32): the evaluator ranks the maintenance tier (the unmarked rules) before the
business tier (the rules marked `benefit_judged`, "Judged rules" above), whatever their
`priority`, and the judged business rules by their net benefit per expected millisecond
([scheduling/README.md](scheduling/README.md), "Net benefit within a tier"). SS-2f (since
0.12.46, register §1.288): that net benefit orders one instance's business rules only; across
instances the business tier goes to the instance least recently dispatched first ("Turns across
instances" there), so the rules of one game never outrank another game's by their values, and a
game's own rules keep their order on its instance. SS-2c (since
0.12.32): a maintenance rule whose package has no maintenance role by the root files declares
`"maintenance": true` ("Judged rules", "The maintenance declaration"), so that it stays in the
maintenance tier; its `priority` is still the high value of its tier. When the
rules a game composes for its bound instances use more than two distinct `priority` values, the
composition reports one `standard_pack_priority_tiers_exceeded` warning (keys `game`, `count` the
number of values, `limit_count` 2) after the unclassified-package warnings, and composes and
schedules the game as before: it never refuses the root. `actinglab standard-pack check` and
`compose` and `package bundle --schedule` list it in `warnings`; the Runtime's workbench does not
record it.

SS-2c (since 0.12.32): when they use exactly two values, a rule that declares itself maintenance
and carries the lower value, or a judged rule (no declaration, its package not maintenance-role)
that carries the higher, is one `standard_pack_priority_tier_mismatch` warning (keys `game`,
`task_id` the rule's id), in file order: a consistency hint for the resource side, most often a
maintenance rule that lacks its declaration (its candidates would wait for values a maintenance
task never declares). A rule whose package is maintenance-role and that declares nothing is in
neither case. It is never a refusal and changes nothing: the evaluator reads the tier from the
mark, not from the value. A new code, because no code says "a rule's declaration and its tier
disagree": `standard_pack_priority_tiers_exceeded` counts values. Only `actinglab standard-pack
check` and `package bundle --schedule` report it, after the `standard_pack_benefit_undeclared`
warnings; `standard-pack compose` and the Runtime's workbench do not.

## Tools

- `actinglab package bundle ... --schedule <schedule.json>` checks the file against the game
  root the bundle makes and lays it beside `bundle.json` byte for byte
  ([package-reference.md](package-reference.md), "Content-directory tools and the bundle
  index").
- `actinglab standard-pack check --root <game root>` reads `schedule.json` beside the other root
  files and checks it the same way.
- `actinglab resource validate` reads a resource repository's root `schedule.json` by
  `parse_standard_pack_schedule` alone (RL-T; [resource-declarations.md](resource-declarations.md)):
  the repository holds no registry, so the references and the composition are left to `package
  bundle --schedule`.
- Both compose the file with one bound instance per server of the registry, an alias of the
  check's own (`standard-pack-check.<server>`), so every rule and item is placed and compiled as
  the Runtime places it for one instance of each server. A refusal is
  `standard_pack_root_invalid` naming `schedule.json`; each unclassified package is one
  `standard_pack_package_unclassified` in `warnings`. Then (UPD-1 fix round 1, since 0.12.30) a
  store update rule whose `cooldown_ms` is 0 is refused the same way ("The store update rule").
  SS-2b (since 0.12.32): both answer `benefit_coverage: {complete, pending, undeclared}`, the
  game's judged rules ("Judged rules"), and list one `standard_pack_benefit_undeclared` per
  undeclared rule in `warnings`, after the composition's other warnings. Nothing is refused.
  `standard-pack compose` reports neither. SS-2c (since 0.12.32): `benefit_coverage` also
  answers `maintenance_declared`, the rules that declare themselves maintenance, and `warnings`
  ends with one `standard_pack_priority_tier_mismatch` per rule whose declaration disagrees with
  its tier ("Two priority tiers"); a `maintenance` of another shape is refused as above. SS-2d
  (since 0.12.35): `benefit_coverage` also answers `realization`, the judged rules that carry the
  realization mark, and a `realization` refused above is refused the same way.
  SS-4 (since 0.12.34): after those, `warnings` ends with one
  `standard_pack_escalation_threshold_high` per rule ("Failure escalation without a daily bound"
  below). SCHED-C1a (register §1.299, since 0.12.52): `benefit_coverage` also answers
  `fixed_value`, and both answer `benefit_provenance` ("Provenance" above). SCHED-C1c (since
  0.12.53): `warnings` ends with one `standard_pack_line_trigger_redundant` per rule and pool ("A
  redundant trigger" below). SCHED-C1f (register §1.303; SCHED-C1f1, since 0.12.54): both answer
  `consumption_facts`, the rules whose consumption is read from a fact ("A run's consumption read
  from a fact" above).
- **A redundant trigger** (SCHED-C1c, register §1.299, since 0.12.53; the coordinator's point 12;
  [scheduling/README.md](scheduling/README.md), "Capped spending order", "Old triggers on a capped
  pool"). A judged rule that consumes a pool writing both lines and a declared valuation block,
  and whose `trigger` requires through `all` the pool's reading (its `resource_projection`, or a
  `fact` on its reading key) compared `greater_than_or_equal` or `greater_than` with a fact
  reference to the pool's capacity fact, is one `standard_pack_line_trigger_redundant` warning
  {`game`, `task_id` the rule, `pool_id` the file's pool id}, once per rule and pool, in file order:
  the capped order already starts that pool at its trigger line and stops it at its stop line. A
  hint, never a refusal: the rule composes and runs as written, and the resource side removes the
  threshold in the change that values the pool. A pool without a valuation keeps such a threshold
  as its only spending trigger and is not reported. `standard-pack compose` and the Runtime's
  workbench do not report it.
- **Failure escalation without a daily bound** (SS-4, register §1.231; plan
  `rd-1029/plan-state-driven.md`, the notes on removing the counts). A rule that writes no daily
  bound (no `loop_budget`, or one without `daily_limit`) and an `on_failure.escalation_threshold`
  above 10 is one `standard_pack_escalation_threshold_high` warning {`game`, `task_id` the rule,
  `count` its threshold, `limit_count` 10}, once per rule, in file order. Without the daily limit
  a run that keeps failing is retried until that many failures pause the pair; the no-progress
  guard ([scheduling/README.md](scheduling/README.md), "No-progress guard") covers successful runs
  only. A hint, never a refusal: the rule composes and runs as written. The resource side lowers the
  threshold to 2 or 3 in the same change that removes a rule's `daily_limit`. `standard-pack
  compose` and the Runtime's workbench do not report it.
- `actinglab standard-pack compose --root <standard_packs.root> --instance
  <alias>=<game>/<server>... --out <new directory>` composes the game roots below `--root` for
  the given bindings (`--instance` once per bound instance; given after the command's words it is
  the command's own and not the global `--instance`, RL-T) and writes `tasks.json`,
  `pools.json`, `activity.json`, `timeline.json` (as compiled) and `manifest.json`
  (`{rules_digest, procedures: [{procedure_ref, game_id, package_id, directory}]}`) into
  `<out>.part`, renamed to `--out` once complete; `--out` and `<out>.part` must not exist, and a
  failure leaves `<out>.part` in place and names it. A bound game must have a game root below
  `--root`. Its answer is `{status: "composed", out, catalog_hash, rules_digest, tasks, pools,
  profiles, events, games, manifest, warnings}`. Under `--dry-run` it is refused
  (`dry_run_unsupported`; alternative `standard-pack check`).

## The scheduler workbench (RL-1b)

Model §3.3–§3.6 and §3.9. The workbench runs in the Runtime when the `policy` section names no
`catalog` (composer mode); `standard_packs.root` is then required. A configuration that names
`policy.catalog` keeps the configured catalog path for this one release, and the workbench reads
nothing.

- **Check.** At the start of a policy cycle, at most once per
  `standard_packs.check_interval_ms` (default 60 000) and at the first cycle. The games in use are
  the games of the configured instances (`policy.facts.instances`). A whole-instance pause does
  not unload a game's rules (pending decision 58, recommended answer A). Each check `stat`s
  `schedule.json` and `bundle.json` of each game in use and reads the game roots only when one of
  them, or the bound instances, changed: 「用到的时候再加载」. A game root without a bound
  instance is never read.
- **Compose and activate.** The roots are composed with `compose_standard_pack_rules`, its
  `requires` checked against the configuration manifest's subsystems (this build names
  `standard_pack_schedule`), and the catalog version is the ledger head plus one. A composition
  whose rules digest differs from the active one's is staged as a generation directory with its
  documents, `fragments/<game>.json` (each game's `schedule.json` as composed) and
  `standard-packs.json` (the rules digest, each game's `schedule.json` sha256, the composed
  procedures and pools), then activated with the existing `catalog.transition_intent` and
  `catalog.activated` records, with no promotion and no approval decision. Each changed
  `schedule.json` gives one `standard_pack_registered` record with `schedule_sha256`, `rules`,
  `held` and `unclassified`. The procedure manifest is the composition's: one binding per
  `package.<package_id>`, `operation.package.run`, no yield point. A scheduled run of a composed
  procedure names its package as the game's registry places it when it is dispatched.
- **Compile warnings** (#428 CW-1, since 0.12.47; register §1.290). The composer places each
  warning the compiled catalog carries (for example `pool_lines_unvalued`) in its game's
  `schedule.json` as it places a refusal, each place once (a pool composed for two instances
  warns at both copies). An activation records them on the `standard_pack_registered` record it
  writes for each changed `schedule.json` (above) as `compile_warnings` (`warning_code`,
  `json_pointer`; at most 64, with `compile_warnings_total`): a game's warnings are in the ledger
  at its own registration. Fix 1 (the coordinator's ruling): it writes no other record, so a game
  whose file did not change gets none, with or without warnings, and a game without warnings is
  recorded as before. After each composition the active generation holds (just activated, or
  with the active rules digest, so also at the first check after a start), the workbench holds
  its warnings, every game's, in the program fact `config.catalog_warnings` (runtime scope,
  re-observed): one count row `{warning, game?, count}` per code and game, then one place row
  `{warning, game?, pointer}` per warning. Past the fact's 256 rows nothing is left out silently
  (fix 1): the place rows are cut first, then the count rows, and the last row
  `{counts_omitted, places_omitted}` says how many of each are missing. The fact is recorded only
  when it changes and never while it is absent and the composition has no warning, so a Runtime
  whose compositions never had one records nothing new; once the active generation has none, it
  is recorded empty. `actingctl overview` and `ac_overview` show it in their `catalog` block as
  `compile_warnings {count, codes, items}`, with `items_omitted` and `counts_omitted` when the
  fact was cut ([runtime-interface.md](runtime-interface.md)). A warning the composer cannot place in a game's
  file (none so far) has no game, keeps its pointer in the composed document and is held in the
  fact only. Composing, activating and deciding do not depend on them.
- **Provenance** (SCHED-C1b, register §1.299, since 0.12.53). After the same compositions the
  workbench also holds the active generation's provenance in `config.catalog_provenance`
  ("Provenance" above), with no record of its own: the `standard_pack_registered` record is
  unchanged.
- **Unload.** A game whose instances are gone, or whose root holds no `schedule.json`, is left
  out of the next composition; activating it drops the previous generation, and the memory keeps
  at most `standard_packs.cached_generations` older generations (default 1): 「不用了直接从内存卸载」.
  A composed generation keeps no source bytes in memory; a dispatch pinned to an older generation
  reads it back from its directory to settle.
- **Last good rules.** A `schedule.json` the composer refuses is recorded once per (game,
  sha256) as `standard_pack_root_invalid`, and that game keeps the `schedule.json` of the active
  generation's `fragments/`; a game that never had a good one has no rules. The other games are
  composed as they are. A broken game root (its `bundle.json` or siblings) is recorded once and
  keeps the active generation until it is fixed. An unclassified package is recorded once per
  (game, package, sha256).
- **Static pool readings.** Under a composed generation a `policy.resources.pools` reading for a
  pool no composed game declares is left out of the evaluation and recorded once as
  `policy_pool_reading_unused` (`pool_id`), so it never stops the other instances.
- **Approval.** An intent of a composed generation names no approval, so its admission reads no
  approval projection.

## The readers of composer mode (RL-1c)

ROUT-L RL-1c (#411; v0.12.20). Every reader that reads `policy.catalog` or
`policy.procedure_manifest` in catalog mode reads, in composer mode, what the configuration
schedules: the composition the workbench makes for the configured instances, by the same
composer over the same games in use, with the procedure bindings the workbench installs. Offline
there is no active generation to fall back on, so a game whose root is broken or whose
`schedule.json` is refused is left out (its problem is reported where it was before), and the
other games are composed.

- **The admission after start** ([policy-suspension.md](policy-suspension.md), "Task package
  suspension") waits for the workbench's first check and then admits the packages the composed
  procedures locate.
- **`actingd check-config`** computes `fact_reader_missing`, `pack_admission_suspended` and
  `policy_remedy_declaration_unusable` over the composition
  ([actingd-check-config.md](actingd-check-config.md)).
- **`actingd suspended`** (and so `ac_diagnose`'s `suspended`, `packs`, `remedies`, `lift`,
  `repeating` and `clock_slots`) is judged against the composition's catalog and bindings
  ([policy-suspension.md](policy-suspension.md)).
- **`actingctl packs` / `ac_packs`** decode the configuration as actingd does and list one
  installation row per composed procedure (`package_id`, its place below the game root as
  `path` with `readable`, `procedure_ref` `package.<package_id>`, `operation_id`
  `operation.package.run`, `scheduled` true). They read the files as the workbench does (the
  contract crate's root check, the policy crate's composer) but cannot see the Runtime's
  subsystems, so `requires` is not checked there (check-config checks it). A game root that
  cannot be read adds `package_binding_unreadable`, a broken one or a refused `schedule.json`
  adds `standard_pack_root_invalid`, and that game has no row. The overview reads the same file
  without composing. Workflow #429 R-11 (since 0.12.36): each row also carries `task_ids`, the
  composed tasks that run its package in id order (the rule's id, or `<rule id>@<alias>` for a
  rule composed for two or more instances), `irreversible` when the package's `control.json`
  reads, and its game's `ui.json` entry for the package and `display`
  ([runtime-interface.md](runtime-interface.md), `packs`).

## The store update rule (UPD-1, since 0.12.30)

A game root whose `interrupts.json` names a page `result: "application_update_required"`
([package-reference.md](package-reference.md)) writes, per server, one rule for its store update
package (`control.json` `store_update`), triggered by the instance fact the Runtime records when a
run meets that page ([scheduling/README.md](scheduling/README.md), "Application Update Hold"). Only
the members that matter here; the rest are written as for the game's other rules (the composer
checks them), and the composer fills the scope:

```json
{"id": "<game>.store_update",
 "package_id": "<game>.store_update",
 "trigger": {"kind": "fact", "fact_key": "health.application.update_required",
             "comparison": "eq", "value": {"type": "boolean", "value": true}, "max_age_ms": null},
 "feedback_stop": {"kind": "fact", "fact_key": "health.application.update_required",
                   "comparison": "eq", "value": {"type": "boolean", "value": false}, "max_age_ms": null},
 "cooldown_ms": 1800000,
 "sensitive": false,
 "on_failure": {"action": "pause", "retry_limit": 1, "retry_backoff_ms": 0, "escalation_threshold": 2}}
```

- `cooldown_ms` is how long the rule waits before it looks again when the store has no new version
  yet (its run ends "not needed", the fact turns false, the next task meets the dialog again and
  the fact is true again); 30 minutes is an example, the resource side chooses it. It is the only
  pace of that loop, so it must be above 0 (UPD-1 fix round 1): `package bundle --schedule` and
  `actinglab standard-pack check` refuse a rule whose package declares `store_update` and whose
  `cooldown_ms` is 0 as `standard_pack_root_invalid`, `schedule_invalid` at
  `/rules/<i>/cooldown_ms` (a rule without the member is refused already, the member being
  required). This is a validity rule of the tools, not a tunable; the Runtime's composer does not
  repeat it.
- `requires` names nothing new (a typed `fact` predicate).
- No other rule changes: the hold of the instance's other tasks is the Runtime's.
- A timeline event whose `invalidates_fact_prefixes` covers `health.application.update_required`
  (the key itself or any prefix of it, `health.` included) is refused (UPD-1 fix round 5): the
  composer refuses the file, the Runtime and the resource tools (`package bundle --schedule`,
  `actinglab standard-pack check`) alike, as `standard_pack_root_invalid`, `schedule_invalid` at
  the first such entry in file order (`/timeline/<i>/invalidates_fact_prefixes/<j>`). Such a reset
  would clear the fact, so the rule's trigger would read the key missing (Unknown) and never run,
  while the Runtime's gate, which reads the fact as recorded, kept holding the instance. An event
  whose `excludes_fact_prefixes` keeps the key (an entry the key starts with) does not invalidate
  it and is not refused. A validity rule, not a key.
- The page itself is no ordinary package's own page; the store update package declares it as its
  entry page, and its "not needed" end is a success (for example a mapping `not_needed`), since
  only a success clears the fact.
- Without such a rule, an instance that met the dialog stays held, every candidate showing
  `application_update_pending`: loud, not silent.
- SS-2b (since 0.12.32): a store update package is no maintenance-role package of the root files
  (the bundle's `maintenance[]` uses are startup, return home and recognition; it is no handler,
  `then.run` or feature package), so the composer marks its rule `benefit_judged` like a
  business rule ("Judged rules"). Its candidate is then `benefit_unknown` until the rule declares
  values, and the held instance waits (the tools count the rule as undeclared). Register §1.225
  names what the resource side, or a later slice, must decide. SS-2c (since 0.12.32): the store
  update rule is a maintenance task (Alice's ruling 104 point 1), so it declares
  `"maintenance": true` ("The maintenance declaration"); it is then not judged and runs as it
  did before SS-2b.

## What does not change

No ledger record type or member is added. A composed generation is activated with the existing
catalog records and no approval (RL-1b); `standard_pack_registered` gains the optional values
`schedule_sha256`, `rules`, `held` and `unclassified`, and (#428 CW-1) `compile_warnings`, values
the ledger's reader only spells, on the records the workbench writes already (no record is added);
`config.catalog_warnings` and (#428 SCHED-C1b) `config.catalog_provenance` are new keys of the
`config.` fact family. The configuration's catalog section, its approvals and
its procedure manifest stay as they are until RL-C removes them.
