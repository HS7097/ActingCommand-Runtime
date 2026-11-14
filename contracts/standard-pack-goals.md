# Standard-pack goal declaration (`goals.json`)

Workflow #455, slice G-R1 (Runtime 0.12.60; [v012-register.md](v012-register.md) §1.356). On board
#455 a person (through the console) or an agent (through the Lab) sets goals for an application.
The console only renders: a standard pack says which goal entries its game offers. This document
is that declaration: where it lives, an entry's members, the eight types of version 1, the blocks
and the fallback rule, defaults, texts and limits, every refusal of its check with its pointer,
who reads it, its interface revision, and what is not this file's.

**No value is read here.** The values a person or an agent sets, the requests that set and read
them, the run fact a governed package reads and the scheduler's reading of a resource goal are
later slices' (G-R2a to G-R5); a game root without the file is exactly what it was before G-R1.

The core stays game-neutral: names, ids, option values, table keys and locales are opaque strings,
compared for equality and measured in bytes; the Runtime interprets no title, no help text, no
unit text and no option title. They are data for the console.

## The file

`goals.json` is an optional root file of the game root, `<standard_packs.root>/<game>/goals.json`,
beside `bundle.json`, by the root-file rule ([package-reference.md](package-reference.md),
"Standard package root", "Naming"): `schema_version` `actingcommand.goals.v1` and `game` equal to
the directory's name.

- It is not `ui.json`: the Runtime must enforce types and ranges, and `ui.json` is display metadata
  that no Runtime reader opens and that never changes a run.
- It is not a member of `schedule.json`: that reader refuses any eighth member, so an older Runtime
  would refuse the whole schedule.

```text
{schema_version: "actingcommand.goals.v1", game,
 entries: [{name, type, block?, title: {<locale>: text}, help?: {<locale>: text}, governs?,
            <the type's members>}]}

governs:  {pool: <pool id of schedule.json>}            type resource
        | {rules: [<rule id of schedule.json>, ...]}     type tier
        | {packages: [<package_id of bundle.json>, ...]} types integer, choice, switch, ordered
```

The document has exactly the members `schema_version`, `game` and `entries`.

## An entry

- **Name** (`name`, required): 1 to 64 bytes of `[a-z0-9][a-z0-9_]*`, unique in the file. It is
  the stable name the Lab uses and, in a later slice, the name of the fact a governed package
  reads. Renaming an entry is removing it and adding another.
- **Position**: the file's order, one page from top to bottom. Nothing numbers an entry.
- **Type** (`type`, required): one of the eight of version 1 (below).
- **Block** (`block`, optional): how a reader shows the entry (below); absent, the type's base
  block.
- **Texts**: `title` (required) and `help` (optional), each a text per locale.
- **Governs** (`governs`): what the entry sets a goal on, required for the six types that have a
  value and absent for `heading` and `note`. A `resource` entry names one pool of `schedule.json`;
  a `tier` entry 1 to 32 rules of `schedule.json`; an `integer`, `choice`, `switch` or `ordered`
  entry 1 to 32 packages of `bundle.json`. Each id is 1 to 128 bytes without a control character
  and named once in its list. The `governs` object holds exactly the member its type takes.
- **Default**: written in the file, never a pointer into the schedule. A `resource` entry's default
  is "no goal": it writes no `default`.

Every other member is refused.

### The eight types of version 1

| `type` | Members besides `name`, `type`, `block`, `title`, `help` | Its value (later slices) |
|---|---|---|
| `heading` | none: a group title between entries | none |
| `note` | none: the author's text | none |
| `resource` | `governs: {pool}`; `rate?: {floor_milli_per_day, ceiling_milli_per_day}` | `{at_least, by_unix_ms?}` |
| `integer` | `governs: {packages}`; `min`, `max`, `step?` (absent: 1), `unit?`, `default` | a whole number |
| `choice` | `governs: {packages}`; `options: [{value, title}]` (2 to 32), `default` | one option value |
| `switch` | `governs: {packages}`; `default` (a boolean) | on or off |
| `ordered` | `governs: {packages}`; exactly one of `options: [{value, title}]` (1 to 1024) and `from_table: {table, names?: {<key>: <text>}}`; `min_count`, `max_count` (1 to 32), `default` (a list) | an ordered list of values |
| `tier` | `governs: {rules}`; `options: [{value, title, scale_milli}]` (2 to 32), `default` | one option value |

- **`rate`** (a `resource` entry): the pool's net income per day, times 1000, signed, at its low end
  (`floor_milli_per_day`) and at its high end (`ceiling_milli_per_day`), written by the resource
  side from its research tables and never from an application's readings. A deadline reads it
  (G-R4); the shape is accepted now so that packs need no second revision.
- **`integer`**: `min`, `max`, `step` and `default` are safe integers; a value lies within
  `min..=max` on a step from `min`. `unit` is a text per locale, or one of the words `second`,
  `minute`, `hour` and `day`: then the values are milliseconds a reader shows in that unit.
- **Option values** (`choice`, `ordered`, `tier`): 1 to 64 bytes of `[a-z0-9][a-z0-9_.:-]*`, each
  once in its list. A tier's `scale_milli` is an integer from 1 to 1,000,000 (times 1000): a later
  slice (G-R5) scales the fixed value of the governed rules by it.
- **`from_table`**: `table` is a table id of the game root (`tables/<table_id>.json`, 1 to 64 bytes
  of `[a-z][a-z0-9_]*`); the values are the distinct keys of its key column in row order, at most
  1024; `names` gives a text per locale to some of them (a key is 1 to 128 bytes without a control
  character). `min_count` is 0 or more and at most `max_count`.
- The `tier` type governs rules that write `fixed_value_milli`; its behaviour is G-R5's, and its
  shape is accepted now.

### Neutral example

```json
{"schema_version": "actingcommand.goals.v1", "game": "neutral",
 "entries": [
  {"name": "stock", "type": "heading", "title": {"en": "Stock", "zh-Hans": "储备"}},
  {"name": "points", "type": "resource", "governs": {"pool": "pool.neutral.points"},
   "title": {"en": "Points to hold", "zh-Hans": "要攒的点数"},
   "rate": {"floor_milli_per_day": 120000, "ceiling_milli_per_day": 900000}},
  {"name": "route", "type": "choice", "governs": {"packages": ["neutral.collect"]},
   "title": {"en": "Route", "zh-Hans": "路线"}, "default": "a",
   "options": [{"value": "a", "title": {"en": "Route A"}}, {"value": "b", "title": {"en": "Route B"}}]}]}
```

## Blocks

Version 1 has seven blocks, one for each type, its base block:

| `block` | Type | What a person sees |
|---|---|---|
| `group` | `heading` | a heading between entries; no nesting, no pages |
| `text` | `note` | text to read, not to change |
| `goal_row` | `resource` | the amount to hold, the current reading, the gap, later the deadline's state |
| `number` | `integer` | a number typed in, with its unit and its limits |
| `choice` | `choice`, `tier` | one of a few options: side by side up to four, a drop-down above |
| `switch` | `switch` | on or off |
| `ordered_list` | `ordered` | values in an order a person sets, with search when the list is long |

`block` may be left out or name the type's base block; any other word is `goal_block_mismatch`.
**The fallback rule**: a later version may give a type more blocks (a slider for an `integer`, say);
a reader that meets a block it does not know shows the type's base block, and one that meets a type
it does not know shows the entry's title and its value read-only, never writes it, and says so.
Not in version 1: a slider, multiple choice and a table (later types or blocks), and a picker
(merged into the ordered list and the single choice).

## Texts and limits

Texts have the shape and the bounds of `ui.json`'s ([package-reference.md](package-reference.md),
`ui.json`, "Limits"): each text has 1 to 16 locales, each locale name 1 to 35 bytes. The words are
this file's own: a list, a map or a text's locales of a count outside its limit, and a name, an
option value, an id, a key, a locale or a text of a length outside its limit, are
`goal_out_of_limits` at what exceeds, as `ui.json` has `ui_out_of_limits` for its own (G-R1 fix
round 1; before it this file said `malformed`). An integer outside the safe integers is not of its
type (`malformed`), and a `max_count` or a `scale_milli` outside its range, or a table of more than
1024 keys, is what the entry declares as its values (`goal_range_invalid`; "The checks"). Bounds of
the shape, not tunables:

| What | Limit |
|---|---|
| the file | at most 1 MiB (1,048,576 bytes) |
| `entries` | 0 to 96 entries (the entry rows an application's goal record keeps beside its targets and task rows within the scheduler's 256 rows) |
| `name` | 1 to 64 bytes |
| `title`, a unit text, an option's `title`, a name under `from_table.names` | 1 to 128 bytes per locale |
| `help` | 1 to 1024 bytes per locale |
| a `governs` list | 1 to 32 ids, each 1 to 128 bytes |
| `options` | 2 to 32 (`choice`, `tier`), 1 to 1024 (`ordered`) |
| an `ordered` entry's values, `from_table.names` | at most 1024 |
| `max_count` | 1 to 32 |
| an option value | 1 to 64 bytes |
| `scale_milli` | 1 to 1,000,000 |
| an integer | a safe integer, at most 2^53 - 1 in magnitude |

## The checks

Two steps, both pure functions of the contract crate (`crates/actingcommand-contract/src/standard_pack_goals.rs`):
the reader, `parse_standard_pack_goals`, reads the file strictly into typed entries; then, for a file
the reader accepts, `check_standard_pack_goal_references` holds the entries to what the game root
offers. Every problem is `standard_pack_root_invalid` with `game`, `path` (the file),
`standard_pack_problem` and `json_pointer` (empty for the whole document), every problem reported,
at most 64, in the file's order: first every member repeated in one object (the reader meets them
while it reads the document, before it judges any member), then the reader's other problems in
document order, then the references' entry by entry. A file the reader refuses is not judged
against the root.

| `standard_pack_problem` | When | `json_pointer` |
|---|---|---|
| `malformed` | the file is no JSON or no object (a file that begins with a byte-order mark too: the reader takes the bytes as they are); a member repeated in one object, unknown, missing or not of its type (an integer outside the safe integers too, an `integer` entry's `default` excepted, which is `goal_default_invalid`); another `schema_version`; a name, an option value, an id, a key, a table id or a unit word outside its form; an `ordered` entry with both `options` and `from_table` (at `from_table`) or with neither (at `options`); a `governs` id named twice | the member (empty for the whole document) |
| `goal_out_of_limits` | the file is larger than 1 MiB; a list, a map or a text's locales of a count outside its limit; a name, an option value, an id, a key, a locale or a text of a length outside its limit (the "Limits" table; an empty one too). G-R1 fix round 1 | what exceeds: the list, the map, the text or the member (empty for the whole file) |
| `game_mismatch` | `game` is not the directory's | `/game` |
| `goal_name_duplicate` | a name an earlier entry has | `/entries/<i>/name`, the later |
| `goal_type_unknown` | `type` is none of the eight, a later version's type included; the entry is read no further | `/entries/<i>/type` |
| `goal_block_mismatch` | `block` is not the type's base block | `/entries/<i>/block` |
| `goal_range_invalid` | what the entry declares as its values cannot hold one: an `integer`'s `min` above its `max` (at `min`) or a `step` below 1 (at `step`); an option value written twice (at the later `value`); an `ordered` entry's `max_count` of 0 or above 32 (at `max_count`), its `min_count` above its `max_count` or above the number of its values (at `min_count`), a table of more than 1024 keys (at `from_table/table`); a tier's `scale_milli` outside 1 to 1,000,000 (at it; one that is no safe integer is `malformed`, as every integer member of the file) | the member named |
| `goal_default_invalid` | `default` is not a value of the declaration: an `integer`'s not a whole number in its range on its step (whatever it is: a string, a fraction or a number beyond the safe integers too), a `choice`'s or `tier`'s not one of its option values, a `switch`'s not a boolean, an `ordered` entry's not a list (at `default`), a value not one of its values or written twice (at `default/<j>`), or a count outside `min_count..=max_count` (at `default`). A default is judged once the range or the options it is judged against hold | `/entries/<i>/default`, or `.../default/<j>` |
| `goal_rate_invalid` | a `rate` whose `ceiling_milli_per_day` is below its `floor_milli_per_day` | `/entries/<i>/rate/ceiling_milli_per_day` |
| `goal_pool_unknown` | a `resource` entry's pool is not a pool of the composed schedule (no pool without `schedule.json`) | `/entries/<i>/governs/pool` |
| `goal_pool_unvalued` | that pool's `valuation` lacks `scale` or `gap` (a pending or absent valuation too) | `/entries/<i>/governs/pool` |
| `goal_package_unknown` | a governed package is not a package of `bundle.json`; or (G-R1 fix round 1) the package a `tier` entry's rule runs is not, which the composition has refused before (`rule_package_unregistered`), so that the check never passes a rule whose package it was not told of | `/entries/<i>/governs/packages/<j>`, or `.../governs/rules/<j>` |
| `goal_governs_irreversible` | a governed package, or (G-R1 fix round 1) the package a `tier` entry's rule runs (`package_id` of the rule), can do something irreversible: it provides a capability that a package of the game root marks irreversible, a `provides[]` entry of that package's `control.json` with the capability and `irreversible: true` ([resource-declarations.md](resource-declarations.md), the `control.json` members). A capability is irreversible when any provider declares it so ([remedies.md](remedies.md) §2.3), so the check first collects the capabilities every package of the root marks, then judges each governed package and each governed rule's package against them, whether it marks the capability itself or not | `/entries/<i>/governs/packages/<j>`, or `.../governs/rules/<j>` |
| `goal_rule_unknown` | a `tier` entry's rule is not a rule of `schedule.json` | `/entries/<i>/governs/rules/<j>` |
| `goal_tier_without_fixed_value` | that rule, its package passed, writes no `fixed_value_milli` | `/entries/<i>/governs/rules/<j>` |
| `goal_table_unknown` | an `ordered` entry's table is no table of the game root (absent, or refused by its own check) (at `from_table/table`), or a key under its `names` is no key of that table (at `from_table/names/<key>`) | the member named |

The references against the table (`goal_table_unknown`, a table of more than 1024 keys or fewer keys
than `min_count`, and an `ordered` default value that is no key of the table, `goal_default_invalid`
at `default/<j>`) are judged only by a reader that holds the game root's tables.

**`goal_governs_irreversible` is not optional.** It is one of the three places that keep the
safety lines out ("What is not this file's"). The references it judges are made only from what
every package of the root provides (the contract crate's `GoalReferences::new`, G-R1 fix round 1):
there is no default that says no package is irreversible, and a package the references were not
told of is never passed.

**A refused file never fails its game root.** The game runs as without the file: no entry applies
for that game, and a reader of the declaration says why.

## Who reads it

- **`actinglab standard-pack check --root <game root>`** reads the file beside the other root files
  and lists it in `root_files` (after `display-layout.json`, before `loading.json`). After the
  composition of `schedule.json` it already runs, it reads the file and holds an accepted file to
  the root: the pools of `schedule.json` and its rules with the packages they run, every package of
  `bundle.json` with the capabilities its `control.json` provides and their `irreversible` marks
  (read by the Runtime's metadata reader, as the remedy report reads them), and the root's tables.
  Each problem is one `standard_pack_root_invalid`
  warning, after the other warnings, in file order; the root is checked as without the file, and
  the answer is otherwise what it is without the file. The file is read through its bound plus one
  byte, so a file of any size is a problem of the file (`goal_out_of_limits` above the bound; before
  fix round 1 `malformed`), never the check's error; a `goals.json`, a package's
  declaration or a table that exists and cannot be read is the check's error (an OS failure is no
  problem of the file).
- **`actinglab package bundle --goals <goals.json>`** reads the file after the composition it already
  runs (with `--schedule`; without it every `resource` and `tier` entry names an unknown pool or
  rule) and holds an accepted file to the root the bundle makes. The bundle holds no table, so no
  table reference is judged. A refused file refuses the bundle as a refused root file does
  (`standard_pack_root_invalid` with its first problem, `path` the file given, every problem in the
  message), `--out` not written; an accepted one is laid beside `bundle.json` byte for byte. The
  operation table's row `package bundle` has the parameter `goals`; the MCP server serves it as the
  row says. `package bundle` writes no `interfaces.json`, and `resource verify`'s `bundle` step does
  not pass the file in G-R1: a bundle that carries it is declared `package` [12, 12] by its packer.
  `resource verify` passes over a `goals.json` in a repository without a word: a warning would need
  an outcome code, which G-R1 does not add ("Before any pack carries the file").
- **The installer's `place`** (the Lab's rescue module) takes `goals.json` from a bundle zip as a root
  file, placed last among them (after `schedule.json`, whose pools and rules its entries name), and
  sets it aside as `goals.json.old-<ms>` when the game root holds one and a later zip carries none;
  `undo` puts it back. A zip that carries the file declares a `package` maximum of 12 at least
  (G-R1 fix round 1, as a zip that carries the notices directory declares 10): otherwise its plan
  refuses it `pack_invalid` caused by `standard_pack_root_invalid` with `goal_undeclared` at
  `<zip>\interfaces.json`, the pointer empty, before the hold and any write. It reads the file's
  name only; its `post_check` runs `standard-pack check`, which reports a refused file without
  failing.
- **The Runtime** opens no `goals.json` in G-R1: its root reader, its startup and run preparation,
  the policy crate and check-config read named files only. A later slice's scheduler workbench reads
  it with `schedule.json`. Since G-R2s (0.12.60) the workbench reads every package's declaration
  as this check does, with the same reader and the same rule, for the composer's irreversible mark
  ([standard-pack-schedule.md](standard-pack-schedule.md), "The irreversible mark").

## Interface

`package` revision 12 ([component-interfaces.md](component-interfaces.md)): a bundle that carries
`goals.json` declares `package` [12, 12] (the contract crate's `GOALS_PACKAGE_REVISION`); a bundle
without it keeps its revision, and the Runtime declares [3, 12] (from Workflow #463 K1 on [3, 13],
[component-interfaces.md](component-interfaces.md), the `package` row 13). This release's installer refuses a
zip that carries the file and declares a maximum below 12 (`goal_undeclared`, "Who reads it"). An
installer before 0.12.60 refuses such a zip in the plan, before the hold and any write: through its
`package` edge (`interface_edge`) when the zip declares 12, at its layout check (`pack_invalid`
naming `goals.json`) when it declares less. A Runtime before 0.12.60 reads a game root that holds
the file as one without it. **A rollback is not refused**: a Runtime before this slice treats the
file as absent; an installer before this slice refuses a bundle that carries the file, and when it
places a bundle without it, it leaves an existing `goals.json` where it is. `ledger`,
`runtime-client`, `actingd-config` and `runtime-outcomes` do not move: the fifteen words (the
fourteen of the file and `goal_undeclared`) are words of the existing vocabulary
`standard_pack_problem` under the existing code `standard_pack_root_invalid`.

## What is not this file's

- **No value is stored in it.** A value is the installation's, written by a person or an agent
  (later slices); the file declares the entries, their bounds and their defaults only.
- **No pointer into the schedule.** An entry names a pool, rules or packages; its default is its
  own. No entry writes a rule's member, a run parameter, a configuration key, a remedy choice or a
  pack switch.
- **The safety lines stay out.** Premium currency, real money, retiring and scrapping are not opened
  here (the owner decides later). They are kept out at three places:
  1. **The check refuses** any entry that governs a package with an irreversible capability
     (`goal_governs_irreversible`): retire and purchase packages that declare it cannot read a
     goal; a package that does something irreversible without declaring it is caught only by the
     third place. Since G-R1 fix round 1 the check follows a `tier` entry's rules to the packages
     they run, and judges a capability across the whole root: a package that provides a capability
     another package marks irreversible is refused as that package is.
     Since G-R1 fix round 2 a `tier` entry's rule is also `goal_governs_irreversible` at its own
     pointer when a rule that follows it (which runs when it completes), directly or through
     further followers, runs such a package or one the references were not told of, each follower
     visited once. Since Workflow #455 G-R2s fix round 1 (0.12.60, register §1.369) the rules that
     follow a rule are every rule, not held, whose trigger has a chain link to it as the evaluator
     reads one (a `dependency_completed` reached only through `all` whose `terminal_states` holds
     `succeeded`), the lists the composer's irreversible mark reads
     ([standard-pack-schedule.md](standard-pack-schedule.md), "The irreversible mark"): a follower
     with a value of its own, one whose link also accepts another end and one behind a complete rule
     count too, not only the followers the coverage lists as riding (`benefit_followers`).
     **A limit that the slice which makes a tier act must lift first:** a rule whose trigger depends
     on the governed rule's completion only through a node the evaluator reads as no chain link (a
     `dependency_completed` under `any` or `not`, or one that does not accept `succeeded`) is not
     followed here, although a governed rule that runs more often can let it run more often too.
     Before a tier can scale anything, the check judges every rule whose trigger depends on the
     governed rule's completion.
     A `resource` entry can still push such a package indirectly: its pool may be produced by a
     rule of that package, and a goal's shortfall raises the weight of what produces the pool. The
     check does not refuse such an entry (it would forbid goals on ordinary pools); the rule
     belongs to the scheduler, and this check leaves it to the evaluator: **a goal's shortfall
     never raises the weight of a rule whose package can do something irreversible**. Workflow
     #455 G-R2s (0.12.60, register §1.369) builds it: the composer marks such a rule's tasks
     (`irreversible_reach`, [standard-pack-schedule.md](standard-pack-schedule.md), "The
     irreversible mark") and the evaluator weighs a marked candidate as on an instance without a
     target record ([resource-targets.md](resource-targets.md), "A task that can do something
     irreversible"), for every version of the record. **One copy of the rule:** the contract
     crate's `IrreversiblePackages` (a capability is irreversible when any package of the root
     marks it; a package is irreversible when it provides such a capability or, since G-R2s fix
     round 3 (the coordinator's ruling of 11-14), when a remedy declaration of its mappings lists
     one, every capability the remedy lists judged from the declarations alone; a package that
     could not be read leaves every package of the root counting as irreversible; the walk over the
     rules that follow a rule) is what `GoalReferences::new` holds and what the composer takes per
     game root, and the policy crate's `standard_pack_rule_followers` gives both the same lists of
     followers; there is no second implementation. So an entry that governs a package whose
     blocked run asks for an irreversible provider, a `tier` over its rule, and a `tier` over a
     rule it follows are `goal_governs_irreversible` too.
  2. **No entry kind writes anything else**: not a remedy choice (those stay in the remedy layer),
     not a run parameter, not a configuration key, not a pack switch. Packages keep their own
     refusals of premium-currency and real-money buttons, and no package reads a goal for them.
  3. **Resource-side review**: the Runtime is game-neutral and cannot know which pool is a premium
     currency, so no `goals.json` of version 1 declares a premium-currency pool or an entry about
     spending premium currency, real money, retiring or scrapping.

## Before any pack carries the file

What a later slice owes before any standard pack carries `goals.json` (G-R1 fix round 1):

- `resource verify` checks a repository's `goals.json` and the packer declares the revision. Today
  its `bundle` step neither passes the file nor declares `package` 12, and it says nothing of a
  `goals.json` it finds.
