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
  "rules":    [ a V2 task without scope, entrypoint and procedure_ref, with "package_id" ],
  "pools":    [ a V2 pool without scope, with an optional "server" ],
  "activity": [ a V2 activity profile without scope, with an optional "server" ],
  "timeline": [ a V2 timeline event without scope, with an optional "server" ],
  "held":     [ { "package_id", "reason", "reference"? } ]
}
```

Every member but `schema_version` may be absent, which means empty. The reader
(`actingcommand_contract::parse_standard_pack_schedule`) refuses, each as
`standard_pack_root_invalid` with `standard_pack_problem` and the JSON pointer of the member:

| Rule | Problem |
|---|---|
| UTF-8 JSON of at most 1 MiB, no duplicate key at any depth, no trailing data; an object | `schedule_invalid` (pointer empty) |
| `schema_version` is `actingcommand.schedule.v1`; no member besides the seven above | `schedule_invalid` |
| No member named `scope` at any depth of a rule, pool, profile or event (the item's own and those of predicate nodes such as `fact`, `record_deadline`, `fact_time_reached`) | `schedule_invalid` at the `scope` member |
| A rule has a string `package_id` (1–128 bytes, no control character), no `entrypoint` and no `procedure_ref`; its `instance_overrides` is absent or empty | `schedule_invalid` at the member |
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
   `resource_projection`'s `value` is one, and the given subsystems name each entry), the
   references and the classification. A refused game is reported with its game id.
3. **Instantiate**: for each bound instance `i` of game `g` on server `s`, every rule whose
   package belongs to `s` and is not held, and every pool, profile and event for every server or
   for `s`, each with every scope filled with `{kind: instance, instance_id: i}` (the item's own
   and that of each predicate node that reads a fact, the nodes with a `fact_key`); a rule's
   `entrypoint.operation_id` is `operation.package.run`, its `procedure_ref` is
   `package.<package_id>`, and its `instance_overrides` is empty.
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
  `standard_pack_package_unclassified` in `warnings`.
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

## What does not change

No ledger record type or member is added. A composed generation is activated with the existing
catalog records and no approval (RL-1b); `standard_pack_registered` gains the optional values
`schedule_sha256`, `rules`, `held` and `unclassified`. The configuration's catalog section, its approvals and
its procedure manifest stay as they are until RL-C removes them.
