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

Workflow #429 RB-1 (since 0.12.80; register §1.514). **A rollback is refused while a required name
is unknown to the target**, and so is an update to such a build. The installer's plan reads
`requires` of the `schedule.json` of each game in use and of each fragment of the active catalogue
generation (`fragments/<game>.json`, "The scheduler workbench" below), and refuses
(`interface_edge`, its row `requires` `next_names_required`, its member `requires_missing` naming
each game, file or generation and each missing name) when the target build's own check-config
does not name one ([install-cli.md](install-cli.md), "Update and rollback", "Subsystems the
schedules require"). The way back to such a build: place schedules that do not require the name,
let the Runtime activate a generation composed without it, and plan again. The plan of the
installer that rolls back decides: a rollback planned by an installer before 0.12.80 is not
checked.

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

#455 P1 (register §1.504, since 0.12.78). A pool whose `observation` is of the kind `list_rows`,
its inventory read from rows of a list reading ([scheduling/README.md](scheduling/README.md), "A
pool read from rows of a list"), requires `pool_list_rows`, alike: the composer refuses a file that
writes one while `requires` does not name it, `schedule_invalid` at the first such pool's
`observation` in file order (for example `/pools/0/observation`). The observation passes through
as written (it names no id and no scope; the composer fills the pool's), and the catalog compiler
checks it, mapped back to the pool. A Runtime names `pool_list_rows` in `config.subsystems`,
always on, from 0.12.79 (#455 P2, register §1.505): its workbench and check-config compose such a
file, and the host computes the pool's reading from the instance's list before each evaluation
([scheduling/README.md](scheduling/README.md), "A pool read from rows of a list"). **No Runtime of
0.12.78 names it**: such a Runtime, and its check-config, refuse a file that lists it with
`schedule_requires_missing` at that entry, and the workbench keeps the game's last good rules
("The scheduler workbench") when the active generation holds another file of the game; after a
rollback from 0.12.79 the refused file is the one the active generation was composed from, so
there is none to keep and the game has no rules (register §1.505, "At deploy"). The resource tools
check no `requires`: `actinglab standard-pack check`, `compose` and `adopt` accept and carry the
member. `standard-pack check` does not check that a package of the root reads the list L with the
columns K and V: the check reads a package's reading marks only (`resource_readings`), not its list
readings. At evaluation a list not keyed by them (a row without a text under K, or no row with V)
leaves the pool unknown, `type_mismatch` (#455 P2 fix round 1), never a known 0.

Workflow #477 CV-2 (register §1.508, since 0.12.80; the coordinator's rulings, the owner's words
being only "one cut-over, no coexistence"). **`catalog_business` switches a root's business part to
its `pack-catalog.json`** ([pack-catalog.md](pack-catalog.md), "Version 2"). A file whose
`requires` names it:

- writes none of the six business members on any rule (`consumes`, `produces`,
  `fixed_value_milli`, `fixed_value_origin`, `limited`, `realization`): the composer refuses one
  with its code for a refused file, `standard_pack_root_invalid`, and the word
  `catalog_business_member` at the member (the first of `consumes`, `produces`, `limited`,
  `realization`, `fixed_value_origin`, `fixed_value_milli` a rule writes);
- stands in a root whose `pack-catalog.json` is of version 2: a root without the file is refused
  `catalog_missing` (the whole document), one of version 1 `catalog_version_1` at its
  `schema_version`, one its reader refuses with its first error, each at `pack-catalog.json`;
- has every rule's business part taken from the file before any other check reads the rules
  (`take_catalog_business` of the contract crate, which the composer calls): the per-rule part
  where the package's entry holds one for the rule's id, else the entry's own part, else none (a
  rule with no effect, no fixed value and no mark, as one that writes empty `consumes` and
  `produces` and nothing more). The rule then holds what the reader would have given it had it
  written the part itself, its `consumes` and `produces` after its other members, so the
  composition is byte for byte the one of that file: the same documents, compiled catalog, catalog
  hash and rules digest, coverage and provenance. A part holding the realization mark or a value
  for a rule that declares itself maintenance is refused `schedule_invalid` at the part's member,
  as the reader refuses either beside the declaration.

Every refusal at a member a rule took from the file, the composer's own (`requires`, a mark on a
maintenance-role package) and the compiler's diagnostics alike, names the file and the part's
member: `/packs/<e>/<member>` for an entry's own part, `/packs/<e>/rules/<r>/<member>` for a
per-rule part (`value` for `fixed_value_milli`, `value/origin` for its origin), the rest of the
pointer kept. A file that does not name the subsystem is composed exactly as before and the file is
not read for it: no root mixes the two readings. A Runtime names `catalog_business` in
`config.subsystems`, always on, from 0.12.80; given subsystems that do not name it (a Runtime of
0.12.79) the composer reads the file as written and refuses it at `requires`,
`schedule_requires_missing`, by the rule above. The other requirements stay: a switched file whose
parts hold a fixed value still names `benefit_fixed_value`, one whose part reads a consumption from
a fact `consumption_fact`. The resource tools, which check no `requires`, compose a switched file
with the `pack-catalog.json` beside it.

SCHED-C1f (register §1.303; SCHED-C1f1, since 0.12.54). A rule whose `consumes` entry writes its `amount` as a
fact ("A run's consumption read from a fact" below) requires `consumption_fact`, alike: the composer
refuses a file that writes one while `requires` does not name it, `schedule_invalid` at the first
such `amount` in file order (for example `/rules/0/consumes/0/amount`).

Workflow #437 ORD-1a (register §1.352, since 0.12.60). A rule that writes `max_queue_wait_ms`, the
longest time its task may wait in its device's queue (a V2 task member that passes through,
[scheduling/v2/README.md](scheduling/v2/README.md)), requires `queue_wait`, alike: the composer
refuses a file that writes it while `requires` does not name it, whatever its value,
`schedule_invalid` at the first such member in file order (for example
`/rules/0/max_queue_wait_ms`). The catalog compiler checks the value, mapped back to the rule.

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

#477 CK-2 (since 0.12.81; [scheduling/README.md](scheduling/README.md), "Clock Sources";
[v012-register.md](v012-register.md) §1.516), advice to a resource repository on a rule's clock. A
rule meant to run once after the game's daily refresh writes one clock occurrence at the refresh
instant. From 0.12.81, for a rule that is not `sensitive`, that occurrence stays owed for the rest
of its activity day after a dispatch a restart interrupted (its run started or not) and after a
failure that paused the rule's pair (a screen the package never recorded included), so the rule
runs on it the same day once the interruption lets it through or its repaired package lifts the
pause: for these a
second occurrence is not needed. **For a failure that passes by itself (a maintenance, a popup) a
second occurrence the same day, written as a retry, is still needed and stays**: a failed run that
is retried, continued or of a sensitive task serves its occurrence as before, and nothing tries it
again later the same day (a pacing of its own for that is a later slice). Such a second occurrence
needs a `daily_limit` of 2 or none, as today, and while it is written a rule that succeeded at the
first occurrence runs again at the second, as before.

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
  apply`, by a value binding of the repository's `ours/model/bindings.json`); without it the pack's
  own hand wrote it (registered in that file as a hand entry with its `status` and `ref`). Only
  `"model"` is a value, and only beside `fixed_value_milli`. It decides nothing: only "Provenance"
  below reads it. (Workflow #464 P1, the coordinator's ruling) The Lab never writes `model`
  ([standard-pack-authoring.md](standard-pack-authoring.md), "The value's origin"); the model's
  writer is `acmodel bindings apply` (Workflow #463 S3-12 corrected this sentence, which said that
  no writer of `model` exists), and the provenance listing of `standard-pack check` shows which
  values are the pack's and which the model's.
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
- Workflow #477 CV-2 (0.12.80): of a root whose `schedule.json` names `catalog_business`, a fixed
  value's origin is its part's `value.origin` in `pack-catalog.json` (`"model"`, else the pack's),
  reported, in the tools' `benefit_provenance` and the workbench's `config.catalog_provenance`
  alike, in the form a rule's `fixed_value_origin` was; the list's ids are still the rules'.

**What a resource repository writes** (SCHED-C1; the coordinator's point 7). By rule class, for a
game's business rules:

| Rule class | The rule writes | Its pool writes | Decided by |
|---|---|---|---|
| Collect and claim (a reward, mail, a check-in, a finished job's collection: no pool consumed) | `fixed_value_milli` | — | `B = V`, by `B ÷ E` in the business tier |
| Consumer of a regenerating pool | its `consumes` (one run's amount), `fixed_value_milli` (`V`, its value without the pool's cost), `"limited": true` where a limit counts its runs; no "reading ≥ capacity − margin" threshold once the pool is valued ("Tools", the redundant trigger) | a constant `valuation` (`scale` Q, `base_weight_milli` W) beside its `stop_line` and `hard_line` (and `trigger_line`), `H = S − (the costliest run)` | below the trigger line `V − W·c`; from the trigger line the capped order by `V ÷ c`. A pack that wants a consumer to run only in capped rounds writes its pool's `W` so that `W·c ≥ V` for that consumer; a consumer meant to spend the reserve below the stop line (a time-limited, high-value run) has `V > W·c` and is still held by the hard line: the reserve Alice's planned red line keeps (11-07, #428 6706630435: 「不让资源达到上限，但是保留一些资源用于某些限时的，收益高的任务」) |
| Reading and navigation step inside a chain | a readings, return-home or recovery step: `"maintenance": true`; a step after the chain's first business rule: nothing (it rides with that rule, SS-2e); the chain's value once, on its first business rule | — | the maintenance tier, or its chain |
| Still needs the model (its value depends on live data or on a choice made at run time) | nothing, or `"amount": "pending"` | `"valuation": "pending"` | `benefit_unknown` until the model's export |

**Who writes the fixed value and the marks, since Workflow #477 CV-1** (0.12.79; register §1.507;
[pack-catalog.md](pack-catalog.md), "Version 2"). A rule's `consumes` and `produces`, its
`fixed_value_milli` with its `fixed_value_origin`, and its `limited` and `realization` marks may
also stand in its package's entry of a version 2 `pack-catalog.json`, with the same types, bounds
and meaning (one definition: the contract crate's `resource_effect`), the entry's own part or the
rule's per-rule part where the rules of one package differ. `actinglab standard-pack catalog
--refresh` (and `--schema-version 2`) writes them there from the rules; `--values <file>` is the
entry point of the model's export for the value and the marks (`acmodel bindings apply` still
writes the rules). **In 0.12.79 the rules are still what the composer reads**: nothing here
changes, a difference between a rule and its entry is the warning
`standard_pack_catalog_schedule_differs`, and no Runtime component opens the catalog. The next
slice (CV-2) has the composer take them from the catalog, switched on per root by an entry of
`requires`.

**The cut-over, since Workflow #477 CV-2** (0.12.80; register §1.508). A resource repository brings
every entry of its version 2 catalog up to its rules (`standard-pack catalog --refresh`, no
`standard_pack_catalog_schedule_differs` left) and then runs `standard-pack catalog --cut`, which
composes the root as it stands and switched and writes the switched `schedule.json` (its rules
without their business members, `catalog_business` in `requires`) only when the two are byte for
byte the same and no rule differs from its part. From then on the root's business part, its effects,
values and marks, is written in `pack-catalog.json` alone (effects by editing the entry, `--values`
for the model's values and marks; the writer takes nothing from the rules: `--refresh`, a missing
file and one of version 1 refused, a new entry its display only, fix round 1; Workflow #477 CV-2b,
0.12.80, register §1.515: `standard-pack adopt` writes the adopted rule's effects, value and marks
into the rule's part there and the rule without them, [standard-pack-authoring.md](standard-pack-authoring.md),
"On a switched root"; [pack-catalog.md](pack-catalog.md), "Who writes it"), and the composer, in
the tools and the Runtime alike, reads it there ("`requires`"). A repository adopts packages
before and after its cut-over alike: adopting into the root before it, `--refresh` of the package
and the cut give the files adopting after it gives, byte for byte.

**How many turns a sweep runs** (#477 LF-1, since 0.12.80; register §1.509). A package whose run
repeats one turn until it has spent enough (a sweep of a regenerating pool) reads how many from
`session.dispatch.schedule.run_cap` ([remedies.md](remedies.md), section 4, "Spend values"; as a
`max_repeats` fact cap, [resource-declarations.md](resource-declarations.md), "Fact caps"). The
Runtime computes it from the lines written on the pool, so a resource repository writes the
consumer's one run's amount in its `consumes` and the lines on the pool, once: `stop_line` and
`hard_line` (and `trigger_line`) beside the pool's `valuation`, or an instance's own lines
([scheduling/README.md](scheduling/README.md), "Lines for one instance"). The rule needs no
`reserves` for that and repeats no line: below the trigger line a judged consumer may spend down
to the hard line; on the head of the pool's capped episode its turns stop once the episode is
spent (the pool at or below its stop line and below its trigger line, the turn that crosses
included), never below the hard line
([scheduling/README.md](scheduling/README.md), "Capped spending order", "What the head's run is
handed"). A rule writes `reserves` only for a floor of its own above the pool's hard line. After
a run that was told its turns, every pool the rule consumes is unknown to the Runtime until it
is read after that run's end ([scheduling/README.md](scheduling/README.md), "Pool lines", "At
each evaluation"), so nothing that needs its value is dispatched from a reading the turns made
stale. A reading the run wrote itself does not count; where the fact refresh can serve the
pool's key one more reading run reads it, and a pool whose reading only a package's own rule
brings waits for that rule.

## Composition

`actingcommand_policy::compose_standard_pack_rules` is the one composer. The Runtime's
workbench, check-config and the resource tools all call it. It is pure: no file, clock or
ledger.

**Inputs.** The bound instances (alias → game and server, from `policy.facts.instances`); for
each game root that holds `schedule.json`, that file's bytes, the root as
`read_standard_pack_root` reads it, the bundle's `maintenance[]` entries and, Workflow #455 G-R2s
(since 0.12.60), which packages of the root can do something irreversible and which could not be
read (`IrreversiblePackages`, a required input without a default; since Workflow #455 G-R2x,
0.12.77, the composer reads only which could not be read, for its warning: "The irreversible
mark" below);
the subsystems `requires` must name, or none (the tools); the catalog version. The composer stays
pure: its caller reads the packages.

**Steps.**

1. **Games in use**: the games of the bound instances, in name order; the instances of each in
   alias order. A game without a bound instance is not read. A bound game whose root holds no
   `schedule.json` has no rules.
2. **Read** each game's file (above): the reader, `requires` (it names `fact_reference` when a
   rule writes a fact reference, `pool_fact_reference` when a pool's `capacity` or a
   `resource_projection`'s `value` is one, `pool_lines` when a pool writes `stop_line` or
   `hard_line`, SCHED-C1c (0.12.53) and SCHED-C1a (0.12.52): `pool_trigger_line` when one writes
   `trigger_line` and `benefit_fixed_value` when a rule writes `fixed_value_milli` or a pool
   `valuation_origin`, #455 P1 (0.12.78): `pool_list_rows` when a pool's `observation` is of the
   kind `list_rows`, SCHED-C1f1 (0.12.54): `consumption_fact` when a rule's `consumes`
   entry writes its `amount` as a fact, Workflow #437 ORD-1a (0.12.60): `queue_wait` when a rule
   writes `max_queue_wait_ms`,
   `reset_exclusions` when a timeline event writes
   `excludes_fact_prefixes`, `fact_unknown_probe` when a rule's `fact` predicate writes
   `on_unknown`, and the given subsystems name each entry), no timeline event
   invalidating `health.application.update_required` ("The store update rule"), the references
   and the classification. A refused game is reported with its game id. Workflow #477 CV-2
   (0.12.80): a file whose `requires` names `catalog_business` (and the given subsystems too, when
   given) first has each rule's business part taken from the root's `pack-catalog.json`, which the
   caller reads beside `schedule.json` and gives with it (`StandardPackGameFiles::catalog`;
   "`requires`").
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
   pool no `valuation_origin`); Workflow #455 G-R2s (0.12.60 to 0.12.76): a rule whose package, or
   a rule that follows it by a chain link (directly or further down), could do something
   irreversible gained `irreversible_reach: true`; since Workflow #455 G-R2x (0.12.77) no rule
   gains it ("The irreversible mark" below).
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

## The irreversible mark

**Withdrawn by Workflow #455 G-R2x (0.12.77; register §1.499; the coordinator's withdrawal of
11-21 on board #455, on the owner's decisions 79 and 234).** Workflow #455 G-R2s (0.12.60;
register §1.369) had the composer mark every task whose package could do something irreversible
(retire, scrap, spend what cannot be had back), and every rule such a package's rule followed, so
that a goal on an ordinary pool such a package also produces never raised its weight. Since
0.12.77 **a goal raises the weight of every task that produces its pool, a package that can do
something irreversible included** ([resource-targets.md](resource-targets.md), "A task that can do
something irreversible"):

- **The composer writes no mark.** No composed task carries `irreversible_reach`, whatever the
  root's packages declare; the walk that spread the mark (the rules a chain link leads from, the
  remedies' capabilities, the unreadable package of a root) is no longer run for the composer. A
  root composes the documents, the catalog hash and the rules digest it composes with no
  irreversible package. A rule of `schedule.json` that writes `irreversible_reach` itself is still
  refused (`schedule_invalid` at `/rules/<i>/irreversible_reach`), so the member is passed through
  to no task. A V2 catalog may carry the member, as a generation of 0.12.60 to 0.12.76 does; it is
  read and hashed as before and has no effect ([scheduling/v2/README.md](scheduling/v2/README.md)).
- **What stays of the irreversible declaration**: a capability's `irreversible` mark in a
  package's declaration and the remedy layer's own rules. Until 0.12.77 the goal check's refusal
  of an entry that governs such a package (`goal_governs_irreversible`), kernel admission's
  `variables_on_irreversible`, `standard-pack check`'s `variable_irreversible_by_root` and the
  judgement that such a variable does not apply stayed too; Workflow #455 G-SLx (0.12.78, register
  §1.501, the owner's decision) lifted all four ([standard-pack-goals.md](standard-pack-goals.md),
  "What is not this file's"; [package-variables.md](package-variables.md)). Their rule below stays
  for the composer's warning and the standard-pack trial's mark
  ([standard-pack-trial.md](standard-pack-trial.md)).
- **The composer's input stays required.** It reads only which packages of the root could not be
  read, for its warning `standard_pack_package_unreadable` (below), which is what tells a person
  that a package's declaration could not be read now that no task is marked for it.

What G-R2s wrote of the rule, its readers and the workbench's re-read, as it stands:

- **The rule, one copy.** The contract crate's `IrreversiblePackages` is made only from what a
  reader read of every package `bundle.json` registers: each package with the capabilities its
  control declaration provides (`provides[]`) and their `irreversible` marks and, fix round 3
  (the coordinator's ruling of 11-14), every capability a remedy declaration of its mappings lists
  (`DeclaredCapabilities`, both required), or "could not be read". A capability is irreversible
  when any package of the root marks it; a package is irreversible when it provides such a
  capability, marked by itself or not, or **when any remedy of its mappings lists one**: a run of
  it that ends blocked asks for that capability's provider, so a goal that raises its weight makes
  the irreversible remedy run more often. The remedies are judged from the declarations alone:
  every capability a remedy lists, its default or not, whatever an instance chooses
  ([remedies.md](remedies.md)). The Runtime's reader gives both (`read_remedy_declarations`, its
  `declared_capabilities`). Until Workflow #455 G-SLx (0.12.77) the goal check
  (`GoalReferences::new`) held the same type; since, its readers are the composer (which packages
  could not be read) and the standard-pack trial's mark; there is no second implementation.
- **The rules that follow a rule** (G-R2s fix rounds 1 and 3) were the lists the composer's mark
  walked until G-R2x and the goal check's until G-SLx: every rule whose trigger has a chain link
  to a rule as the evaluator reads one, walked through further followers. Since G-SLx nothing
  reads them: the policy crate's `standard_pack_rule_followers`, the contract crate's
  `IrreversiblePackages::followers_irreversible` and the goal check's `GoalRule::followers` are
  removed. The coverage's riding followers (`benefit_followers`, "Coverage" above) are another
  list and stay.
- **A package that could not be read.** Its capabilities are unknown, so the rule counts every
  package of its root as irreversible (`counts_irreversible`, the safe side; since G-SLx the
  standard-pack trial's mark alone reads it, the goal check and the variables' judgement no
  more). The composition warns `standard_pack_package_unreadable` once per such package (keys
  `game`, `package_id`), after its other warnings; until 0.12.76 every composed task of that game
  was marked too, and since 0.12.77 none is.
- **Who reads the packages.** The Runtime's workbench, check-config and `actingd suspended`
  (`compose_configured_standard_packs`, check-config's preview) and the resource tools
  (`standard-pack check`, `standard-pack compose`, `package bundle --schedule`) read each
  registered package below the game root with the package reader the goal check uses (the
  Runtime's metadata reader, `read_remedy_declarations`), for a game root that holds
  `schedule.json`. A package the reader refuses could not be read: the workbench records the
  warning with the reader's refusal as its cause; the tools list it in `warnings` with the same
  cause (fix round 1); and, fix round 1, **check-config shows it**: its `standard_pack_schedule`
  preview lists one `standard_pack_package_unreadable` per such package of a composed game in its
  `problems`, with the reader's refusal as its cause, and each is one of check-config's notes, so a
  package whose declaration cannot be read is visible before the Runtime starts.
- **The offline pack rows** (`actingctl packs`, and `ac_packs`, which answers the same rows;
  the coordinator's ruling 2 of 11-14) compose with their own reading of each registered package's
  `control.json` (the reading their `irreversible` row member uses, giving each provided capability
  with its mark), through the same rule; lab-ops does not reach the Runtime's package reader (the
  architecture guard keeps `execution-kernel` to the daemon's crates). A package whose control file
  cannot be read is unreadable there (its `package_binding_unreadable`, at its row or, for a
  package without a row, once after the rows). **Limit:** a package the Runtime's reader refuses for
  a reason outside what lab-ops reads (another file of the package, its operation declaration) is
  unreadable to the Runtime only; and lab-ops does not read the remedies (fix round 3), declared in
  a package's operation. Since G-R2x the composition marks no task by either input, so the offline
  rows' composition and the Runtime's are the same; check-config, the forward projection and the
  dispatch are the Runtime's and are the truth. The rows' own `irreversible` member keeps its
  meaning: the package's own declaration.
- **Three scopes of one word.** `irreversible` means today: the package's own declaration (the
  `actingctl packs` row member), the whole root's (a capability any package of the root marks:
  the standard-pack trial's mark; G-R2s's mark used it until G-R2x, and the goal check and the
  variables' judgement until Workflow #455 G-SLx, 0.12.78), and the providers bound on an instance
  (the remedy layer's request flag, [remedies.md](remedies.md) §2.3).
- **The workbench reads the declarations again** when a package's declaration files change (the
  coordinator's ruling 1 of 11-14). The marks it compares at each check ("The scheduler workbench"
  below) are, per game in use, those of `schedule.json` and `bundle.json`, since Workflow #463 V2
  fix round 1 (Runtime 0.12.61) that of the root's `goals.json` (read with the declarations for
  the variables' judgement alone, [package-variables.md](package-variables.md), "A variable that
  does not apply"; a metadata failure on it is a mark of its own and never stops the check), and,
  for each package
  the root registered when the workbench last read it (no extra read of `bundle.json` per check),
  the marks of the files the declaration reader opens by name: the reader reads the package's whole
  content directory, a set that follows the directory's listing and is not fixed, so the two files
  it opens by name before any file says anything are marked: the control file `control.json` and
  the package's own manifest `resources/operations/resources.json`; not the package's other files.
  Each mark is the file's length and modification time, absent when the file does not exist: the
  standard the workbench already uses for the two root files. **Its limit:** a change that keeps
  both the length and the modification time of both files is not seen until another change, a
  change of `bundle.json` or `schedule.json`, or a start. A changed mark is handled as a changed
  `bundle.json`: the root and its declarations are read again and the rules composed again, so a
  package replaced in place, by `place` or by hand, is seen at the next check, and a package
  readable again when one of its marked files changes. A metadata failure on a marked file (other
  than its absence) is a mark of its own and never stops the check: the root is read once and that
  package is unreadable there. Each package's files are marked just before the reader reads it, and
  those marks are the ones the next check compares, so no root keeps the declarations of an earlier
  read while a marked file is known to have changed. Since G-R2x the re-read serves the warning and
  the variables' judgement; the composition it gives is the one of before, so it activates nothing
  new.
- **Unreadable packages at deploy** (fix round 1, point 6; since G-R2x a package that cannot be read
  voids no goal). Any package `bundle.json` registers that the reader cannot read is one
  `standard_pack_package_unreadable` until it reads: a held package left half made, a package
  registered but not placed yet, and a package that marks a capability irreversible without the
  `parameters` the reader requires of one ([remedies.md](remedies.md)). Before a deploy, run
  `actinglab standard-pack check --root <game root>` over each game root and look for that
  warning. During a hot placement a root can be refused for one check (up to a minute:
  `bundle.json` or `schedule.json` half written); its game's goals are void for that long ("A game
  whose root cannot be composed", below).

## A game whose root cannot be composed

**A game whose root cannot be composed has no goals in effect until it composes** (Workflow #455
G-R2s round 2, 0.12.60, the coordinator's ruling of 11-14; under "The irreversible mark" until
0.12.77, standing on its own since Workflow #455 G-R2x, register §1.499, which withdrew the mark and
left this rule exactly as it was). Where the workbench keeps the active generation because a check
cannot compose (a root whose metadata cannot be read, a damaged root, a composition refused as a
whole, a refusal that cannot be narrowed to a game it composed), that generation may not hold a
game as its files give it now; so noted games' candidates are computed as on an instance without a
target record (`resource_target_root_uncomposed`, [resource-targets.md](resource-targets.md), "A
game whose root cannot be composed"). The rules (fix rounds 3 and 4, the coordinator's rulings of
11-14): **while a game's own root fails** (its metadata, or its read), the check records it once,
goes on for the other games (their files are marked, read and composed as ever) and does not
activate their composition (the failed game keeps the active generation's rules); it voids the
failed games and, **as a rule, every other game the active generation holds otherwise**: one of
whose tasks the active generation holds one that composition does not compose with the same id
and procedure (until 0.12.77 also one the composition marked irreversible and the active
generation did not; that comparison went with the mark). The check records each game it voids
that way, one `resource_target_root_uncomposed` (info, `game`) per such game at each check that
voids it, as a `standard_pack_read` record. One failing root voids only its own game and the games
the active generation holds otherwise, at the first check after a start too (when the workbench
holds no file marks). **A composition refused as a whole**, or a refusal no round can narrow,
voids the failed games and every game in use whose files the check did not find unchanged since
its last composed read: at the first check after a start, which holds no file marks, every game in
use. A refusal as a whole is recorded once per set of files composed, as one
`standard_pack_root_invalid` per composed game naming that game and its `schedule.json` (it named
no game before, which broke its catalog entry), as check-config gives it. A root's metadata
failure (`standard_pack_root_unreadable`) is recorded again after that game's root has composed in
between. The notes are memory only and end at the first composition of every game that succeeds;
while one is held, each check reads and composes again even when nothing changed.

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
- **What a rule's budget and threshold count** (Workflow #490 ENTRY-A, since 0.12.77, register
  §1.494; [policy-suspension.md](policy-suspension.md), "A failure at the entry"). A rule's
  `daily_limit` is spent at each admission, and an immediate rerun of a failure in a step of its
  package costs one, as before. A run that failed at its entry (its package executed no step: the
  game was on a screen its gate, prerequisite or return-home layer could not leave, a code of class
  `pack_logic_mismatch`, whatever the ladder's `exclude_codes` say of it, so a prerequisite's entry
  not reached and a return-home layer's recheck not passed count too; a capture backend's or device
  link's failure there does not) gives its admission back, is not rerun at once, owes no rerun and
  never counts toward `escalation_threshold`, so a rule whose threshold is 1 is not paused by it;
  the instance waits for its recovery (`instance_entry_blocked`; a store update rule's task still
  runs) and the rule is due again by its own trigger once it is unblocked. Fix round 4: no failure
  at the entry counts against the rule, whatever ran between. A rule that keeps failing at its
  entry on an instance waits there after each failure, counted from the end of the instance's
  block (one minute, doubling to thirty with the shipped values), while the instance's other
  rules run first, and once its failures that came although another rule got in reach its
  `escalation_threshold` it is set aside on that instance until its activity window ends, a
  person told once; it is not paused, and the next window starts from nothing. With the
  recommended `on_failure` (threshold 2) and a healthy rule due, a rule whose entry is broken is
  set aside at about its third failure in the window (about fourteen minutes), with a threshold
  of 1 at its second (about three minutes); alone on the instance it is never set aside.
  Activating a catalogue (a mended package) starts every such count from nothing at once, and so
  (fix rounds 5 and 6) does a placement that changes a package of a game in use without changing
  the rules: the workbench's next check finds it by the length and modification time of
  `bundle.json`, of `pages.json`, `interrupts.json` and `navigation.json` and of each registered
  package's `control.json` and `resources/operations/resources.json`, compared with what that
  comparison kept at the check before, so once per change, while another game's root is broken
  too (a change by hand of a package's other files alone is not seen, nor, until a composition has
  succeeded since the start, a change of a package's two files alone: the packages compared are
  the last successful composition's). No package or rule changes for it.
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
  `schedule.json` and `bundle.json` of each game in use, since Workflow #463 V2 fix round 1
  (0.12.61) its `goals.json`, and, since Workflow #455 G-R2s (0.12.60), the declaration files of
  each package the root registered when it was last read ("The irreversible mark" above), and
  reads the game roots only when one of them, or the bound instances, changed:
  「用到的时候再加载」. A game root without a bound instance is never read. A check that reads a
  root of N packages stats 3 + 2N files (2 + 2N before 0.12.61); the declarations and `goals.json`
  are read (N package reads and one file) only when one of those marks changed. Workflow #477 CV-2
  (0.12.80): **of a root whose `schedule.json` named `catalog_business` when it was last read, the
  check stats `pack-catalog.json` too** (4 + 2N), marked just before the file is read with the
  root, so a placement that changes only the catalog is read and composed at the next check (its
  length and modification time, the standard of the other marks, and its limit); its metadata
  failing is the root's own, as `schedule.json`'s. A root that does not name it is not looked at
  for the file, as before.
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
  (game, package, sha256). Workflow #477 CV-2 (0.12.80): a composed generation keeps beside the
  fragment of each switched `schedule.json` the `pack-catalog.json` it was composed with
  (`fragment-catalogs/<game>.json`, the directory only when there is one). A switched root whose
  catalog is missing or refused is refused as a file that does not compose is, its record naming
  `pack-catalog.json` (once per pair of the two files' sha256), and the game keeps its last good
  rules: the active generation's fragment with the catalog kept with it, also when
  `schedule.json` itself did not change.
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
  ([runtime-interface.md](runtime-interface.md), `packs`); Workflow #477 PC-1 (since 0.12.68):
  that display, and the package's business part (`catalog`, `{consumes, produces}`), come from
  its game's `pack-catalog.json`, which replaced `ui.json`, and each game carries the index both
  ways ([pack-catalog.md](pack-catalog.md)). Workflow #455 G-R2s (since 0.12.60):
  their composition takes the composer's irreversible input from that same reading of every
  registered package's `control.json` ("The irreversible mark", its limit there); the row member
  `irreversible` stays the package's own declaration.

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

## Readiness (Workflow #464 P3, since 0.12.61)

Register §1.375; the coordinator's rulings on board #464 and in the work order of its fix round 1,
not the owner's words. A pack that lacks one thing is not refused: it is never dispatched, and
nobody is told. The readiness answers, for every pack of one game root, what the scheduler can do
with it, or every reason it waits, item by item, in words a person and an agent can act on. It
writes nothing, starts nothing (no child process, no Runtime request) and reads no secret. Its
answer never says more than is true: no pack is answered `dispatchable`, or complete, that the
Runtime would refuse or never dispatch.

**The one function.** `actingcommand_resource_tooling::judge_standard_pack_readiness` judges the
thirteen items. It takes `StandardPackReadyRequest {root, declared_package, installation}`: `root`
a game root, installed (`<standard_packs.root>/<game>/`) or a bundle laid out as one (its
directory named as its game); `declared_package` the `package` range `[min, max]` the bundle
declares (`speaks.package` of the zip's `interfaces.json`), where the caller holds it; and
`installation`, what the caller read of an installation, each part optional: `bound` (each
instance with its game and, where the read tells it, its server), `package` (the range the
installation holds a bundle to), `subsystems` (the live build's configuration manifest
subsystems), `suspended` (each suspended pair by its package, with its task and instance where
told) and `models` (the models the vision root holds with the identity they are named by). The
root is read as `actinglab standard-pack check` reads it: a root that is no directory, that holds
no `bundle.json` or whose root files the structure check refuses is refused as that check refuses
it; nothing else refuses. Then (fix round 1) `standard-pack check` itself runs on the root
(`check_standard_pack_root`, called as it is): each of its refusals (every issue it lists, else its
outcome) is carried by the pack it names (its `package_id`, else the pack whose directory its
`path` is), or by every pack when it names none, as a failing item: under S1 when it is the
`package_invalid` of a pack it names, under S4 when it is `standard_pack_root_invalid` of
`schedule.json`, otherwise under S7. Its warnings stay warnings, the answer's `warnings` (ruling 7:
a recorded entry page the page list does not name is accepted with its
`package_bundle_entry_page_unlisted`). Two answers call it, each as `standard-pack ready --root`
calls it (no declared range, no installation): `standard-pack adopt`'s, on the game root its check
tree laid out, `<state>/adopt-check/<checkout>/installed/<game>/` (#463 S3-5, since 0.12.63;
[standard-pack-authoring.md](standard-pack-authoring.md), "The answer"), and
`standard-pack build`'s, on the installed root its steps laid out, `<out>/work/installed/<game>/`
(#463 S3-7, since 0.12.63; [resource-verify.md](resource-verify.md), "`standard-pack build`");
`build.json` does not carry it. The placement plan does not call it yet (a follow-up of P3):
`place --plan` does not answer it.

**The items.** Each item of each pack, in this order, is judged by the function that judges it
elsewhere, and answered `holds` (with its `basis` where more than one way exists), `fails` (with
`package_dispatch_waiting`), `not_judged` (with `package_readiness_unjudged`) or, I3 only,
`information`:

| Id | `readiness_item` | Judged by | Fails with `readiness_problem` |
|---|---|---|---|
| S1 | `admitted` | the Runtime's loader, the admission `standard-pack check` runs for every pack | `admission_refused`, caused by `package_invalid` with the loader's refusal |
| S2 | `classified` | the composer's classification ("Classification"), after it accepted the file | `schedule_missing` (no `schedule.json` and no root file gives a role), `unclassified` (caused by `standard_pack_package_unclassified`), `held` |
| S3 | `valued` | the composer's coverage of each rule of the pack ("Judged rules"): complete or a follower holds, the maintenance tier is exempt; a follower is not ahead of its head (below) | `value_pending` (`task_id`), `value_undeclared` (`task_id`, `pool_id`; caused by `standard_pack_benefit_undeclared`), `head_waiting` (`task_id` the follower, `blocking_package_id` the head's pack) |
| S4 | `composed` | the composer with one instance per server, as the tools compose ("Tools"), and `standard-pack check`'s refusals of `schedule.json` (a store update rule without a cooldown, a probe on a key no reading mark of its package writes) | `composition_refused`, caused by `standard_pack_root_invalid` at the file's pointer |
| S5 | `entry_reachable` | fix round 1: first the pack's prerequisite chain through the kernel's own predicate (`PrerequisiteChain`: `next` with the server's return-home package, `begin`, `admit`, `finish`), as the Runtime admits it before a run: its declared prerequisite packages, or, for a pack whose entry the navigation does not take and that may fall back, its server's return-home package, each layer the package the loader admitted here; then the admitted package's entry as the Runtime's gate resolves it ([linear-steps.md](linear-steps.md), "Return-home fallback"): an application entry or no page entry to check, else a declared prerequisite package of the root, else navigation data whose page list lists the entry, else the return-home package of its server (not the pack itself); fix round 2: a page-graph pack run as the run's own package keeps its home entry, which takes that package through no chain (R23), so it holds only when the Runtime's entry would run the package: the package the loader admitted, the kernel's `require_configured_package_id`, then its `prerequisite_incompatibility_with` for a configured return-home package (the host's `configured_return_home_incompatibility`) | `prerequisite_unregistered` (caused by the kernel's `contained_task_prerequisite_unbound`), `prerequisite_refused` (a layer of another game, server or resolution or one that cannot be a layer, a cycle, more than three declared layers, the step limit, or a layer the loader refuses; the kernel's code or the layer's `package_invalid` the cause), each with `blocking_package_id` the layer; `return_home_refused` (the home entry's return-home package of another game, server or resolution, one that cannot be a layer, one that declares a prerequisite package of its own, or one the loader refuses; the kernel's code or the package's `package_invalid` the cause, `blocking_package_id` the package); `entry_unreachable` (`server`, `page_ids`) |
| S6 | `models_named` | the admitted package's recognition models, what `standard-pack check` answers as `models` | never alone: the loader refuses a target that names no model (S1) |
| S7 | `root_checked` | fix round 1: `standard-pack check`, for what S1 to S6 do not judge (the navigation roles, the loading copies, the notices directory, the remedies' readers) | `check_refused` (a refusal this pack carries; the check's refusal the cause), `root_refused` (one only another pack carries: `blocking_package_id` that pack, the check's refusal the cause; the check judged nothing after it) |
| I1 | `instance_bound` | `bound` | `instance_unbound` (`game`), `server_unbound` (`game`, `server`) |
| I2 | `package_revision` | the rule of `place` at the edge ([install-cli.md](install-cli.md)), called, not copied (the second merge of main: the contract crate's `package_edge_holds` and `BundleRevisionNeed`, which `place` calls): `package` against the declared maximum, `min <= max_declared <= max`; then what the game root carries asks its revision of the declared maximum, in `place`'s order: the notices directory 10, `goals.json` 14, a pack whose `control.json` declares `variables` 14 (its presence, read by `control_declares_variables`, the first such pack in `bundle.json`'s order); a select step (`package` 13) asks nothing, as `place` asks nothing of it. `place` refuses a bundle whole, so every pack of the root answers it | `revision_outside` (`required_revision`, `range_min`, `range_max`); `revision_undeclared` (`required_revision` the revision asked; caused by `place`'s refusal, `standard_pack_root_invalid` with `notice_undeclared`, `goal_undeclared` or `variable_undeclared` at the root's `interfaces.json`, the pointer empty, `package_id` the declaring pack for `variables`) |
| I3 | `unrecorded_screen_code` | nothing: an information line, never a refusal (ruling 3: the code for an unrecorded screen moves no interface, so no revision tells it) | never |
| I4 | `requires_known` | the composer again with `subsystems`, as check-config and the Runtime check `requires` | `requires_unknown`, caused by `standard_pack_root_invalid` `schedule_requires_missing` at `/requires/<index>` |
| I5 | `pair_unsuspended` | `suspended` | `pair_suspended` (`task_id`, `instance_id` where told) |
| I6 | `models_installed` | `models` against S6's | `model_missing` (`model_name`) |

An item is `not_judged`, never guessed and never passed, with `readiness_unjudged`:
`installation_not_read` (no installation was given: I1, I2, I4, I5, I6), `part_not_read`,
`not_admitted` (S5, S6, I6 of a pack S1 refused), `composition_refused` (S2, S3, I4 of a refused
file), `no_rule` (S3 of a pack no rule composes for), `server_not_told` (I1 of a game whose bound
instances' servers the read does not tell), `package_undeclared` (I2 without the declared range:
an installed root keeps no `interfaces.json`), `check_stopped` (S7 of a pack whose S1 or S4 carries
the check's refusal: the check judged nothing after it). I3 is always `information`,
`no_revision_tells`. A recorded entry page that `pages.json` does not list keeps its
`package_bundle_entry_page_unlisted` warning, and the readiness requires S5 for it (ruling 7): a
`return_home` entry for its server, or a prerequisite package.

**The answers** (fix round 1; vocabulary `readiness_state`, the pack's member `readiness`). Each
pack answers one of four words:

- `waiting`: an item fails; every failing item is listed with its problem. An item of the root
  that was not judged comes only with one that fails, and never counts as holding. A follower is
  not ahead of its head: a pack whose every rule rides with another rule's chain (S3 by
  `follower`) waits, S3 `head_waiting` naming the first follower and its head's pack, while that
  pack waits (judged again until nothing changes);
- `ready_to_place`: every item of the root holds and no item fails, but not every item of an
  installation was judged (no installation was read, or not all of it): complete as far as the
  game root tells. The installation's items are judged where an installation is read anyway, at
  placement (the placement plan, a follow-up that calls this function; today no caller hands it an
  installation's readings), and, once the pack is live, by the scheduler's own reasons (the
  schedule explanation);
- `dispatchable`: every item of the root and every item of an installation holds (I3 is
  information), which is answered only when the caller handed the function an installation's
  readings; an installation item that was not judged never counts as holding;
- `not_scheduled`: the pack plays a supporting part a root file gives it (its `roles`, vocabulary
  `readiness_role`: `startup`, `return_home`, `recognition` of a `bundle.json` `maintenance[]`
  entry, `features` of `pages.json`, `edge` and `chain` of `navigation.json`, `handler` and
  `then_run` of `interrupts.json`) and no rule dispatches it (maintenance-role and not scheduled,
  "Classification"): other packs' runs use it. A failing item makes it `waiting` first.

`dispatchable` does not include a positive net benefit: a judged rule's candidates are not
dispatched while that benefit is unknown or not positive, which the scheduler judges only at run
time ("Judged rules", above). When a dispatchable pack is due is then its trigger's; the schedule
explanation tells why a candidate waits.

**Where an installation's items are read.** Without the Runtime: I1 from the configuration's
`policy.facts.instances` (game and server), I2 from the placing program's own `package`
declaration, I4 from check-config's `config_manifest.subsystems`, I5 from `actingd suspended`, I6
from the rescue row `vision check`. While the Runtime runs: its status tells I1's game (not the
server) and `config.subsystems` I4, and `actingd suspended` and `vision check` can be read as
well. The function itself reads none of them: its caller hands them in.

**The row.** `actinglab standard-pack ready --root <game root>` (operation table row `standard-pack
ready`, module `core`, offline; MCP `ac_standard_pack_ready`) answers the function for the game root
alone: `{root, game, installation: "not_read", dispatchable, ready_to_place, waiting,
not_scheduled, warnings, packs: [{package_id, server, readiness, roles?, held?, warnings?, items:
[{id, item, state, basis?, models?, outcome?}]}]}`, the packs in the registry's order, beside
`lab_operation_completed`. It reads no installation (the status read tells an instance's game but
not its server; the configuration's instances, check-config's subsystems, `actingd suspended` and
the vision root's models are read where the placement plan reads them), so its five installation
items are `not_judged`, `installation_not_read`, and its packs `ready_to_place` (or
`not_scheduled`) at best. It starts no child process: a child `actingd` under an installation root
could be taken for a running Runtime by the update path. Under `--dry-run` it answers the same (no
effect). A pack whose S2 fails `held` answers `held` (Workflow #463 S3-6, since 0.12.63),
`{reason, reference?}` as its `held` entry writes them, the shape of the entry and of adoption's
decision: text for a reader that nothing evaluates; `reference` is absent where the entry writes
none, and `held` is absent from every other pack's row. S2's own outcome stays
`package_dispatch_waiting` with `readiness_problem` `held`.

**A pack's own warnings** (Workflow #463 S3-6, register §1.413, since 0.12.63). Each pack's
`warnings` lists the warnings of `standard-pack check` that name it, in the check's order: a
warning names a pack by its `package_id` value, else by a `path` value that is the pack's
directory as the registry names it (fix round 2, as the check's refusals are carried) or lies in
the pack's directory (the root check of the variables names the declaring pack's `control.json`,
"A variable that does not apply" in [package-variables.md](package-variables.md), and no package
id). A warning that names no pack (a rule's hint by its `task_id`, a refused `goals.json`) is on
no pack's row. The answer's `warnings` lists every warning as before, so a pack's warning appears
in both places. A pack no warning names answers no `warnings` member, as a pack without `roles`
answers none. Every caller of the function answers both members as they are:
`actinglab standard-pack ready`, the operation table's row `standard-pack ready` (actinglab's own
command) and its MCP tool `ac_standard_pack_ready` (through a `serve` child) serialize the whole
answer.

## Where a pack stands: `standard-pack status` (Workflow #463 S3-14, since 0.12.68)

Register §1.446. To learn where one package stands an agent asked four places: the readiness, the
hold's reason, the trial records and the placement journals. This row is the one read, for the
part that needs no running installation. It reads only, offline: the game root, its packs'
directories, the Lab's state directory and, with `--repo`, the checkout's packs. It writes nothing,
starts nothing (no child process) and reads no installation, no secret and no Runtime (S3-15a:
but with `--installation`, which reads the installation's journals and starts its one `actingd
suspended`, "The installation's part", below); under `--dry-run` it answers the same
([actinglab-dry-run.md](actinglab-dry-run.md), `no_effect`).

```text
actinglab standard-pack status --root <game root> [--repo <checkout game root>]
    [--package-id <id>] [--state-dir <dir>] [--installation]
```

With `--installation` (Workflow #463 S3-15a, since 0.12.69; "The installation's part", below) the
row also reads the installation actinglab runs from: its placement journals and one run of its
`actingd suspended`. Without it everything below holds as written, and the answer is the one
0.12.68 gave, byte for byte.

The row `standard-pack status` of the operation table ([lab-operations.json](lab-operations.json),
module `authoring`, slice `463-S3-14`); over MCP the tool `ac_standard_pack_status`, served by a
`serve` child. `--root` is a game root as the readiness takes it; `--state-dir` the Lab's state
directory, where the trial keeps its records ([standard-pack-trial.md](standard-pack-trial.md), "The
record"), default actinglab's; `--repo` the game's working root in a resource repository's
checkout ([standard-pack-authoring.md](standard-pack-authoring.md)); `--package-id` one pack of the
root. A flag the row does not take, a positional word, a flag given twice or an option without its
value is refused before anything is read; a `--package-id` the root does not register is
`validation_failed` naming `--package-id` with `not_recognised`; a `--repo` that is no directory
`validation_failed`. The readiness's refusals of the root are this row's.

**The answer.** `{root, game, state_dir, repo?, stage_states, enabled_states, packs}`, beside
`lab_operation_completed`. `stage_states` counts the root's packs at each `stage_state`
(`not_admitted`, `not_judged`, `can_be_tried`, `supporting`, `enabled`) and `enabled_states` at
each `enabled.state`
(`enabled`, `held`, `not_placed`, `not_scheduled`, `not_judged`), each word with its count, 0
included; with `--package-id` too they are the root's. `packs` holds a row per pack in the
readiness's order, or the one row `--package-id` names: it is the part of the answer that grows
with the root, where it may be cut; through MCP an answer over the budget is written whole to the
export directory (`answer_exported`), and `--package-id` reads one pack. Each row:

| Member | What it is, and what it is judged from |
|---|---|
| `package_id`, `server` | As the readiness's row names them. |
| `formed.admitted` | Whether the readiness item S1 holds (the Runtime's loader admits the pack and `standard-pack check` refuses no admission of it). |
| `formed.content_hash_prefix` | The first 12 hex digits of the content hash of the pack's directory, by the trial's own function `content_hash_prefix_of` over the directory read as a trial reads the one it tries (the train of S3-12 and S3-14): the digits a trial names a content by ([standard-pack-trial.md](standard-pack-trial.md), "The package"). Absent when the containment's reader cannot read the directory of a pack whose S1 does not hold (the loader reads it by the same reader, so S1 carries the refusal); a directory of a pack whose S1 holds that cannot be read refuses the answer, `package_invalid`. |
| `formed.checkout` | With `--repo`: `{pack, content_hash_prefix}`, the checkout's `packs/<package_id>` and its prefix measured the same way, `null` when nothing is at that path; anything there that cannot be read as a pack directory refuses the answer, `package_invalid`. |
| `conditions` | The pack's thirteen items exactly as `standard-pack ready --root` answers them for the same root ("Readiness", above): the one function, called as that row calls it (no declared range, no installation), never a copy. |
| `missing` | Each item of `conditions` that fails, in their order, as answered; the S2 of a held pack carries `held`, `{reason, reference?}`, the hold as its `held` entry writes it. |
| `held` | The train of S3-12 and S3-14: `{reason, reference?}`, the pack's hold as the `held` entry of the root's `schedule.json` writes it, whenever the file the composer accepted holds the pack, whatever S2 answers. A held pack in a supporting role, whose S2 holds by its role (so neither `missing` nor the readiness's row names its hold), shows it here. Absent otherwise. |
| `trial` | The trials of this content (`formed.content_hash_prefix`), the trial's one summary of them ([standard-pack-trial.md](standard-pack-trial.md), "What the records say of one content"), which adoption's `review.trial` answers too: `state` (`passed`, `failed`, `none`, each record judged as "A trial that passed" there says), `content_hash_prefix`, `record` (the latest record's `authorization`, `at_unix_ms`, `request_id`, `end`, the business end its `outcome_key`, and `matched`), `passed_on_record` and `other_content`. A record of another content does not count. Absent when the content was not read, or beside `trial_not_read`. |
| `trial_not_read` | The train: when the trial's reader refuses this pack's records (a file of a record's name that is no record, `trial_record_unreadable` with its word, or the directory not listed), `{outcome, text}`, the reader's refusal with its code and the same in words, as adoption's `review.trial_not_read`; this pack's trial is not read, and every other pack is answered. |
| `enabled.state` | Whether the root's `schedule.json` enables the pack, judged from the classification the readiness's S2 reads ("Classification"), in this order: `not_judged` when the composer refuses the file (S2 `composition_refused`; the Runtime then keeps the last schedule it accepted, "Last good rules", which this root does not tell); `enabled` when a rule names the pack and it is not held, whatever supporting part it plays beside; `not_scheduled` when it plays a supporting part a root file gives it and no rule dispatches it (the readiness's `not_scheduled`); `held` when `schedule.json` holds it; `not_placed` otherwise: no rule names it, or the root holds no `schedule.json`. |
| `enabled.authorization` | Without `--installation`: `not_recorded`, with `enabled.authorization_unrecorded` `journals_not_read`, for every pack: the placement journals (`pack_place`, whose inputs name the authorization and the packages a placement enables, [install-cli.md](install-cli.md)) are not read. With it (S3-15a): the placement that last enabled the pack, `{reference, initiator?, at_unix_ms, operation}`, and no `authorization_unrecorded`; or `not_recorded` with one cause word (below). The tool never infers an authorization. |
| `stage_state` | Only what the row knows (the train of S3-12 and S3-14): `not_admitted` while S1 does not hold; else, by `enabled.state`: `not_judged` when it is `not_judged` (the composer refuses the root's `schedule.json`); `enabled` when it is `enabled`; `supporting` when it is `not_scheduled` (a supporting part other packs' runs use, no rule naming the pack); `can_be_tried` when it is `held` or `not_placed`. |

Three sentences the row keeps true: `enabled` is what the root's schedule says, not who authorized
it; a passed trial is not an authorization; `conditions` are the readiness's items for a root
judged without an installation (its installation items not judged), so the pack they describe is
`ready_to_place` at best, never `dispatchable`.

**Words.** Five vocabularies of the answer's members, registered in the outcome catalog (none is an
outcome value, and no outcome code is added): `stage_state` (`not_admitted`, `not_judged`,
`can_be_tried`, `supporting`, `enabled`), `enabled_state`, `trial_state` (`passed`, `failed`,
`none`; the vocabulary of adoption's `review.trial.state` too), `pack_authorization`
(`not_recorded`) and `authorization_unrecorded` (`journals_not_read`; since S3-15a also
`no_journal_names_it` and `journals_unreadable`).

**The installation's part** (Workflow #463 S3-15a, register §1.451, since 0.12.69; the
coordinator's rulings in the work order, not the owner's words). `--installation` (a switch; the
row's parameter `installation`) reads the installation actinglab runs from, located as every row of
the rescue module locates one (the program's own root, [install-cli.md](install-cli.md), "The Lab's
rescue module": no root word), and hands the status function what it read; the function reads no
installation itself. It still writes nothing and reaches no Runtime. In this order (fix round 1:
the child is started before the journals are read, so that its bound runs beside the readings
instead of after them):

1. **The root**, by the rescue module's one public read ([install-cli.md](install-cli.md), "What
   the placements enabled"): `--root` must be the installed game root of its game in that
   installation, `<standard_packs.root>\<game>` of its configuration (a relative one against the
   root, the game the root's directory name), as `place` finds a game root, the two compared as
   written, then resolved; else the line is refused `package_status_root_not_installed` (`path` the
   root given) before the game root, a journal or the report is read and before the child is
   started; a configuration that names no `standard_packs.root` places no game root, and is refused
   so too. A root or configuration the rescue module cannot read refuses the line with its outcome
   (`install_command_refused` of `install_status`), as does a program that runs from no
   installation (the location's own outcome, `install_root_unresolved`).
2. **One `actingd suspended`** for the whole answer, started now: the installation's own, started
   as `diagnose` starts it (the program of the generation selected now, `--config` its
   configuration, `--deadline-ms` the deadline less 2 s) and ended at `diagnose`'s command-line
   budget (24 s).
3. **The journals**, while it runs: every journal of the installation, by the journal's own reader:
   per package id, the newest `pack_place` of that game that finished (`finished_at_unix_ms`) with
   every `pack_item` step `done`, was not undone (no `undo` record, begun or ended) and not rolled
   back (no `rollback` record), and whose input `enabled` names the package; a later placement
   whose `enabled` does not name it (a content-only replacement) changes nothing. A journal that
   cannot be read hides none of the others. An operations folder that cannot be listed refuses the
   line with that listing's failure, after the child has ended (it is never left running).
4. **The composed bindings**, while it runs: each catalog task with the package it runs, as
   `actingctl packs` reads them (its installation rows' `package_id` and `task_ids`, the procedures
   `package.<package_id>` the scheduler workbench composes for the configured instances; "The
   readers of composer mode", above), and the warnings that reading gives which concern the root's
   game (each naming the game, or a path below its game root).
5. **The report**, read and judged by the function a diagnosis reads it with: printed, succeeded,
   and of the installation's state root. The row reads no ledger itself. A child that cannot be
   started, fails, prints no report, reads another state root or is ended at its deadline leaves
   the pairs not read, with the outcome a diagnosis gives it, and refuses nothing; the composed
   bindings are used only beside a report that was read.

What it answers, beside the members above:

| Member | What it is, and what it is judged from |
|---|---|
| `enabled.authorization`, `enabled.authorization_unrecorded` | The placement that last enabled the pack, `{reference, initiator?, at_unix_ms, operation}`: the reference its journal's input `authorization` holds, as given; the initiator that began it (absent where the journal names none); when it finished; its operation id. Whatever `enabled.state` says now: a pack a later placement held keeps the authorization it was last enabled under. Else `not_recorded` with `no_journal_names_it` (every journal was read and no placement that counts names it) or `journals_unreadable` (none that was read names it, and some could not be read: one of them may). |
| `pairs` | Each pair of a catalog task that runs the pack and an instance that the report names in its `suspended` or its `break_ins`, by task, then instance: `{task_id, instance_id, suspended?, break_in?}`. `suspended`, when the pair is suspended: `{pause_cause?}`, the report's word, absent where its row names none. `break_in`, when the report has the pair's row: that row's members as the report names them (`content_hash_prefix`, `runs`, `runs_needed`, `first_settled_at_unix_ms`, `since_first_ms`, `ms_needed`, `in_break_in`; [policy-suspension.md](policy-suspension.md), "`actingd suspended`"). A row's task is the pack's when the composed bindings bind it to the pack; a row whose task no binding names (a composition the configuration does not make now) is no pack's. A pack a bound task runs, of which the report names no pair, answers `[]`. Fix round 1: a pack the composed bindings bind no task to answers in their place its own word (`pairs_unread`) `no_task_bound`: a configuration that binds its procedures itself, whose rows name no task; a game the composition left out (its warnings in `warnings`); a pack no composed rule runs. When the pairs were not read, every pack answers in their place one word: `suspension_report_not_read` or `bindings_not_read`. |
| `installation` (the root) | `{operations_read, operations_unread, suspension_report_read, report_warnings?, pairs_unread?, pairs_unread_outcome?}`: how many journals were read, of any kind; the ids of those that could not be, oldest first; whether the report was read; (fix round 1) `report_warnings`, the words of the report's warnings the row passes on, as the report spells them: `["ledger_read_incomplete"]` when the report that was read carries it (its ledger read ran out of its deadline, so its rows are what it read), absent otherwise; and, when the pairs were not read, the word and beside it the outcome that says why (the child's own, as a diagnosis gives it, or the bindings' reader's). The outcome is given in the detail the line is (fix round 1): under `ACTINGCOMMAND_DETAIL=codes` its codes-only projection, its codes and no text (no tail of the child's stderr). `stage_states` and `enabled_states` do not change. |
| `warnings` (the root) | Fix round 1: this row's warnings only, each an outcome in its own shape `{code, category, values, …}`, as every row of this command line carries them: beside pairs that were read, the warnings the reading of the composed bindings gave that concern the root's game (a game root it could not read or that the composition refused, `standard_pack_root_invalid`; a file of it it could not read, `package_binding_unreadable`; and the like), as that reading gave them. Absent when there are none. |

`pairs` lies inside each row of `packs`, the part that grows with the root and where an answer over
the MCP budget is written whole to the export directory. What the row still does not read: the
readiness items of an installation, I1 to I6 ("Readiness", above), which stay `not_judged`
`installation_not_read` in `conditions`, and the placement plan's readiness; neither check-config
nor `vision check` is run. Where it is served: actinglab on Windows, where the rescue module is
built, on its command line and through a `serve` child (so the Lab's MCP server's
`ac_standard_pack_status` too); a build without the rescue module refuses `--installation`
`package_status_installation_unbuilt` before anything is read.

**Exit statuses** (fix round 1). The two refusals of S3-15a refuse (category `error`, class
`none`): exit 1 on actinglab's command line, which prints the CLI module's line, and in the
envelope a `--via` line prints exit 2 (their class `usage_validation`); the rescue module's
refusal of a root, configuration or operations folder it cannot read, and the location's refusal
of a program in no installation, exit 1 on actinglab's command line too, and 4 in a `--via` line's
envelope (class `device_instance`). Through MCP each is a refusal object.

Words and codes of S3-15a: `authorization_unrecorded` gains `no_journal_names_it` and
`journals_unreadable` (answer values); the vocabulary `pairs_unread` (`suspension_report_not_read`,
`bindings_not_read`, and since fix round 1 `no_task_bound`, a pack's own; answer values); and two
refusals, `package_status_root_not_installed` and `package_status_installation_unbuilt`, each
`error`.

## What does not change

No ledger record type or member is added. A composed generation is activated with the existing
catalog records and no approval (RL-1b); `standard_pack_registered` gains the optional values
`schedule_sha256`, `rules`, `held` and `unclassified`, and (#428 CW-1) `compile_warnings`, values
the ledger's reader only spells, on the records the workbench writes already (no record is added);
`config.catalog_warnings` and (#428 SCHED-C1b) `config.catalog_provenance` are new keys of the
`config.` fact family. The configuration's catalog section, its approvals and
its procedure manifest stay as they are until RL-C removes them.
