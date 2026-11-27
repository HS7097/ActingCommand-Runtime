# Standard-pack goal declaration (`goals.json`)

Workflow #455, slice G-R1 (Runtime 0.12.60; [v012-register.md](v012-register.md) §1.356). On board
#455 a person (through the console) or an agent (through the Lab) sets goals for an application.
The console only renders: a standard pack says which goal entries its game offers. This document
is that declaration: where it lives, an entry's members, the five types of version 2, the blocks
and the fallback rule, defaults, texts and limits, every refusal of its check with its pointer,
who reads it, its interface revision, and what is not this file's.

**Workflow #463 V1** (Runtime 0.12.61; [v012-register.md](v012-register.md) §1.378;
[package-variables.md](package-variables.md)) replaced the four types that governed packages
(`integer`, `choice`, `switch`, `ordered`) by the type `variable`: a package declares the value it
reads in its own `control.json`, and an entry of this file only places it on the page. Release
0.12.60 shipped version 1 with its eight types, and a released format does not change its shape
in place, so the file of the five types is **version 2** (`actingcommand.goals.v2`; V1 fix round
3, the coordinator's ruling 2 on the design's questions). This build reads version 2 only: a
version 1 file is `goal_version_retired` at `/schema_version` and is read no further. No pack
carries a version 1 file ("Before any pack carries the file"), so nothing is converted and no
reader of version 1 stays. A version 2 file that writes one of the four types is
`goal_type_unknown` at it. What a Runtime, an installer and a Lab of 0.12.60 do with a version 2
file is in "Interface".

**No value is read here.** The values a person or an agent sets, the requests that set and read
them, the run fact a governed package reads and the scheduler's reading of a resource goal are
later slices' (G-R2a to G-R5); a game root without the file is exactly what it was before G-R1.

The core stays game-neutral: names, ids, option values, table keys and locales are opaque strings,
compared for equality and measured in bytes; the Runtime interprets no title, no help text, no
unit text and no option title. They are data for the console.

## The file

`goals.json` is an optional root file of the game root, `<standard_packs.root>/<game>/goals.json`,
beside `bundle.json`, by the root-file rule ([package-reference.md](package-reference.md),
"Standard package root", "Naming"): `schema_version` `actingcommand.goals.v2` (Workflow #463 V1
fix round 3; version 1, `actingcommand.goals.v1`, is what release 0.12.60 read) and `game` equal
to the directory's name.

- It is not `ui.json` (since Workflow #477 PC-1, the display part of `pack-catalog.json`,
  [pack-catalog.md](pack-catalog.md)): the Runtime must enforce types and ranges, and that file is
  a declaration no Runtime reader opens and that never changes a run.
- It is not a member of `schedule.json`: that reader refuses any eighth member, so an older Runtime
  would refuse the whole schedule.

```text
{schema_version: "actingcommand.goals.v2", game,
 entries: [{name, type, block?, title: {<locale>: text}, help?: {<locale>: text}, governs?,
            <the type's members>}
         | {name, type: "variable", block?}]}

governs:  {pool: <pool id of schedule.json>}            type resource
        | {rules: [<rule id of schedule.json>, ...]}     type tier
```

The document has exactly the members `schema_version`, `game` and `entries`.

## An entry

- **Name** (`name`, required): 1 to 64 bytes of `[a-z0-9][a-z0-9_]*`, unique in the file. It is
  the stable name the Lab uses; a `variable` entry's name is the name of the variable it places.
  Renaming an entry is removing it and adding another.
- **Position**: the file's order, one page from top to bottom. Nothing numbers an entry.
- **Type** (`type`, required): one of the five of version 2 (below).
- **Block** (`block`, optional): how a reader shows the entry (below); absent, the type's base
  block, and for a `variable` entry the placed variable's.
- **Texts**: `title` (required) and `help` (optional), each a text per locale; a `variable` entry
  writes none (the variable carries its own).
- **Governs** (`governs`): what the entry sets a goal on, required for `resource` and `tier` and
  absent for the other types. A `resource` entry names one pool of `schedule.json`; a `tier` entry
  1 to 32 rules of `schedule.json`. Each id is 1 to 128 bytes without a control character and named
  once in its list. The `governs` object holds exactly the member its type takes. A `resource`
  entry's pool is the file's id on every instance of the game: on a game with two instances the
  composition names each instance's copy `<id>@<alias>`, and a goal set on the entry still names
  the file's id, which the Runtime reads as that instance's copy (#503 GOAL-1, since 0.12.88;
  [resource-targets.md](resource-targets.md), "A target's pool").
- **Default**: written in the file, never a pointer into the schedule. A `resource` entry's default
  is "no goal": it writes no `default`.

Every other member is refused.

### The five types of version 2

Version 1, which release 0.12.60 read, had eight: these five without `variable`, and `integer`,
`choice`, `switch` and `ordered`, which governed packages (Workflow #463 V1 fix round 3).

| `type` | Members besides `name`, `type`, `block`, `title`, `help` | Its value (later slices) |
|---|---|---|
| `heading` | none: a group title between entries | none |
| `note` | none: the author's text | none |
| `resource` | `governs: {pool}`; `rate?: {floor_milli_per_day, ceiling_milli_per_day}` | `{at_least, by_unix_ms?}` |
| `tier` | `governs: {rules}`; `options: [{value, title, scale_milli}]` (2 to 32), `default` | one option value |
| `variable` | none; it writes neither `title` nor `help`, only `name`, `type` and an optional `block` | the placed variable's ([package-variables.md](package-variables.md)) |

- **`rate`** (a `resource` entry): the pool's net income per day, times 1000, signed, at its low end
  (`floor_milli_per_day`) and at its high end (`ceiling_milli_per_day`), written by the resource
  side from its research tables and never from an application's readings. A deadline reads it
  (G-R4); the shape is accepted now so that packs need no second revision.
- **Option values** (`tier`): 1 to 64 bytes of `[a-z0-9][a-z0-9_.:-]*`, each once in its list. A
  tier's `scale_milli` is an integer from 1 to 1,000,000 (times 1000): a later slice (G-R5) scales
  the fixed value of the governed rules by it.
- The `tier` type governs rules that write `fixed_value_milli`; its behaviour is G-R5's, and its
  shape is accepted now.
- **`variable`** (Workflow #463 V1): places on the page the variable of its `name` that some
  package of the game root declares in its `control.json` ([package-variables.md](package-variables.md));
  the variable's type, bounds, default and texts are the declaration's. A variable no entry places
  is shown after the entries, grouped by package (a later slice's view). The four types it replaced
  (`integer`, `choice`, `switch`, `ordered`, with `from_table`) are the variable kinds of
  package-variables.md, without `governs` (the governed package is the declaring one) and, until a
  later slice, without `from_table`.

### Neutral example

```json
{"schema_version": "actingcommand.goals.v2", "game": "neutral",
 "entries": [
  {"name": "stock", "type": "heading", "title": {"en": "Stock", "zh-Hans": "储备"}},
  {"name": "points", "type": "resource", "governs": {"pool": "pool.neutral.points"},
   "title": {"en": "Points to hold", "zh-Hans": "要攒的点数"},
   "rate": {"floor_milli_per_day": 120000, "ceiling_milli_per_day": 900000}},
  {"name": "route", "type": "variable"}]}
```

The third entry places the variable `route` a package of the root declares (Workflow #463 V1; in
version 1 it was a `choice` entry that governed `neutral.collect`).

## Blocks

Version 2 has the seven blocks of version 1, one for each type or variable kind, its base block:

| `block` | Type | What a person sees |
|---|---|---|
| `group` | `heading` | a heading between entries; no nesting, no pages |
| `text` | `note` | text to read, not to change |
| `goal_row` | `resource` | the amount to hold, the current reading, the gap, later the deadline's state |
| `number` | an `integer` variable | a number typed in, with its unit and its limits |
| `choice` | `tier`, a `choice` variable | one of a few options: side by side up to four, a drop-down above |
| `switch` | a `switch` variable | on or off |
| `ordered_list` | an `ordered` variable | values in an order a person sets, with search when the list is long |

`block` may be left out or name the type's base block; any other word is `goal_block_mismatch`. A
`variable` entry may leave it out or name one of the four blocks a variable is shown with, which
must be the placed variable's base block (Workflow #463 V1); any other is `goal_block_mismatch`.
**The fallback rule**: a later version may give a type more blocks (a slider for an `integer`, say);
a reader that meets a block it does not know shows the type's base block, and one that meets a type
it does not know shows the entry's title and its value read-only, never writes it, and says so.
Not in version 2: a slider, multiple choice and a table (later types or blocks), and a picker
(merged into the ordered list and the single choice).

## Texts and limits

Texts have the shape and the bounds that `ui.json`'s had and that the display part of
`pack-catalog.json` keeps ([pack-catalog.md](pack-catalog.md), "Limits"; Workflow #477 PC-1):
each text has 1 to 16 locales, each locale name 1 to 35 bytes. The words are
this file's own: a list, a map or a text's locales of a count outside its limit, and a name, an
option value, an id, a locale or a text of a length outside its limit, are `goal_out_of_limits` at
what exceeds, as `pack-catalog.json` has `catalog_out_of_limits` for its own (`ui.json` had
`ui_out_of_limits`) (G-R1 fix round 1; before it this
file said `malformed`). An integer outside the safe integers is not of its type (`malformed`), and
a `scale_milli` outside its range is what the entry declares as its values (`goal_range_invalid`;
"The checks"). The value types' limits Workflow #463 V1 moved to a package's `variables`
([package-variables.md](package-variables.md)). Bounds of the shape, not tunables:

| What | Limit |
|---|---|
| the file | at most 1 MiB (1,048,576 bytes) |
| `entries` | 0 to 96 entries (the entry rows an application's goal record keeps beside its targets and task rows within the scheduler's 256 rows) |
| `name` | 1 to 64 bytes |
| `title`, an option's `title` | 1 to 128 bytes per locale |
| `help` | 1 to 1024 bytes per locale |
| a `governs` list | 1 to 32 ids, each 1 to 128 bytes |
| `options` | 2 to 32 (`tier`) |
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
| `malformed` | the file is no JSON or no object (a file that begins with a byte-order mark too: the reader takes the bytes as they are); a member repeated in one object, unknown, missing or not of its type (an integer outside the safe integers too; a `title` or `help` a `variable` entry writes); a `schema_version` that is neither version 2's nor version 1's (a version this build does not know yet, a later one included); a name, an option value or an id outside its form; a `governs` id named twice | the member (empty for the whole document) |
| `goal_version_retired` | Workflow #463 V1 fix round 3: `schema_version` is `actingcommand.goals.v1`, version 1, which release 0.12.60 read and this build reads no more. The file is read no further: after the members repeated in one object and the document's own members unknown or missing, it is the file's only problem | `/schema_version` |
| `goal_out_of_limits` | the file is larger than 1 MiB; a list, a map or a text's locales of a count outside its limit; a name, an option value, an id, a locale or a text of a length outside its limit (the "Limits" table; an empty one too). G-R1 fix round 1 | what exceeds: the list, the map, the text or the member (empty for the whole file) |
| `game_mismatch` | `game` is not the directory's | `/game` |
| `goal_name_duplicate` | a name an earlier entry has | `/entries/<i>/name`, the later |
| `goal_type_unknown` | `type` is none of the five of version 2, a later version's type included, and (Workflow #463 V1) the four types `variable` replaced, `integer`, `choice`, `switch` and `ordered`; the entry is read no further | `/entries/<i>/type` |
| `goal_block_mismatch` | `block` is not the type's base block; Workflow #463 V1: a `variable` entry's `block` that is none of the four a variable is shown with (by the reader), or not the placed variable's base block (by the references) | `/entries/<i>/block` |
| `goal_range_invalid` | what the entry declares as its values cannot hold one: an option value written twice (at the later `value`); a tier's `scale_milli` outside 1 to 1,000,000 (at it; one that is no safe integer is `malformed`, as every integer member of the file) | the member named |
| `goal_default_invalid` | a `tier`'s `default` is not one of its option values, judged once its options hold | `/entries/<i>/default` |
| `goal_rate_invalid` | a `rate` whose `ceiling_milli_per_day` is below its `floor_milli_per_day` | `/entries/<i>/rate/ceiling_milli_per_day` |
| `goal_pool_unknown` | a `resource` entry's pool is not a pool of the composed schedule (no pool without `schedule.json`) | `/entries/<i>/governs/pool` |
| `goal_pool_unvalued` | that pool gives a goal no step and importance: its `valuation` lacks `scale` or `gap` (a pending or absent valuation too), and (#455 B3-P, since 0.12.82) it writes no `target_defaults` with both `scale` and `importance_milli` in their place, which only a pool without a valuation (the member absent or `null`, as the compiler reads it) may ([resource-targets.md](resource-targets.md), "Who gives a goal its step and importance") | `/entries/<i>/governs/pool` |
| `goal_package_unknown` | (G-R1 fix round 1) the package a `tier` entry's rule runs is not a package of `bundle.json`, which the composition has refused before (`rule_package_unregistered`), so that the check never passes a rule whose package it was not told of; Workflow #463 V1: or a package that declares a placed variable is not | `.../governs/rules/<j>`, or `/entries/<i>/name` |
| `goal_rule_unknown` | a `tier` entry's rule is not a rule of `schedule.json` | `/entries/<i>/governs/rules/<j>` |
| `goal_tier_without_fixed_value` | that rule, its package passed, writes no `fixed_value_milli` | `/entries/<i>/governs/rules/<j>` |
| `goal_variable_unknown` | Workflow #463 V1: a `variable` entry's name is no variable a package of `bundle.json` declares (every declared name counts, one the root check lets apply or not) | `/entries/<i>/name` |

`goal_table_unknown`, the word of an `ordered` entry's table, stays registered and is no longer
given: Workflow #463 V1 replaced the `ordered` entry, and no entry or variable takes a table until
a later slice brings `from_table` to a variable.

`goal_governs_irreversible` stays registered and is no longer given (Workflow #455 G-SLx, Runtime
0.12.78, register [v012-register.md](v012-register.md) §1.501): until 0.12.77 the check refused at
`.../governs/rules/<j>` a `tier` entry's rule whose package could do something irreversible (it
provided a capability any package of the root marks `irreversible: true`, or its remedies listed
one), or one of whose followers' packages could, and at `/entries/<i>/name` a `variable` entry
whose variable such a package declared. An entry governs, reaches or follows such a package as
any other since ("What is not this file's").

**The references are made only from the root's packages** (the contract crate's
`GoalReferences::new`, G-R1 fix round 1; since G-SLx it takes their ids alone, no longer what each
provides): there is no default that leaves a package out, and a package the references were not
told of is never passed (`goal_package_unknown`).

**A refused file never fails its game root.** The game runs as without the file: no entry applies
for that game, and a reader of the declaration says why.

## Who reads it

- **`actinglab standard-pack check --root <game root>`** reads the file beside the other root files
  and lists it in `root_files` (after `display-layout.json`, before `loading.json`). After the
  composition of `schedule.json` it already runs, it reads the file and holds an accepted file to
  the root: the pools of `schedule.json` and its rules with the packages they run, and every package
  of `bundle.json` with (Workflow #463 V1) the variables it declares (read by the Runtime's metadata
  reader, as the remedy report reads them; until Workflow #455 G-SLx, 0.12.77, also the
  capabilities its `control.json` provides and their `irreversible` marks); since V1 it reads no
  table. A version 1 file is the warning
  `goal_version_retired` and is read no further (V1 fix round 3). Each problem is one
  `standard_pack_root_invalid` warning, after the other warnings (the root check of the packages'
  variables among them, [package-variables.md](package-variables.md)), in file order; the root is
  checked as without the file, and the answer is otherwise what it is without the file. The file
  is read through its bound plus one byte, so a file of any size is a problem of the file
  (`goal_out_of_limits` above the bound; before fix round 1 `malformed`), never the check's error;
  a `goals.json` or a package's declaration that exists and cannot be read is the check's error (an
  OS failure is no problem of the file).
- **`actinglab package bundle --goals <goals.json>`** reads the file after the composition it already
  runs (with `--schedule`; without it every `resource` and `tier` entry names an unknown pool or
  rule) and holds an accepted file to the root the bundle makes, its placements to the variables the
  bundle's packs declare. A refused file refuses the bundle as a refused root file does
  (`standard_pack_root_invalid` with its first problem, `path` the file given, every problem in the
  message), `--out` not written; an accepted one is laid beside `bundle.json` byte for byte. The
  operation table's row `package bundle` has the parameter `goals`; the MCP server serves it as the
  row says. `package bundle` writes no `interfaces.json`: a bundle that carries the file is declared
  `package` [12, 12] by its packer (G-R1), since Workflow #463 V1 fix round 3 [14, 14], the
  revision of version 2 (`GOALS_PACKAGE_REVISION`). A version 1 file refuses the bundle
  `goal_version_retired`.
- **`actinglab resource verify` and `actinglab standard-pack build`** (Workflow #455 GOALS-PACK-1,
  Runtime 0.12.77, register [v012-register.md](v012-register.md) §1.498;
  [resource-verify.md](resource-verify.md), the `bundle` step): a resource repository is one game
  root, its root files at its top; when it holds `goals.json`, the `bundle` step of both rows
  checks the file once `package bundle` wrote the bundle, with the check above, called, not copied
  (the resource tooling's `check_bundle_goals`: the file read as `standard-pack check` reads it,
  through its bound plus one byte, and `goal_issues`, which holds an accepted file to the root the
  bundle makes: the composed schedule's pools and rules, every package of `bundle.json` and the
  variables the bundle's packs declare). A refused file fails the step with every problem, each
  `standard_pack_root_invalid` with `game`, `path` (the repository's file), `standard_pack_problem`,
  `json_pointer` and `step`, in file order, as `standard-pack check` gives them; the step's index
  and copy checks still run in the same run, and no zip is written; a version 1 file is so refused
  with its one problem, `goal_version_retired`, as `package bundle --goals` refuses it. An
  accepted file is laid beside `bundle.json` byte for byte, as
  `package bundle --goals` lays it; the zip, its read-back and the installed root the step checks
  with `standard-pack check` carry it; the step's counts name it (`goals`: the game, the path and
  how many entries of each type); and it is a read path of the `checkout` precondition, so the file
  a zip carries is the committed blob. `resource verify`'s `interfaces.json` declares `package`
  [14, 14] for it, the highest of 3, 10 (with `notices/`) and 14 (with `goals.json`): it reads no
  pack's content, so a pack that declares `variables` or a shared read's `lines` does not raise
  it; only `standard-pack build` raises its declaration by content, and declares the revision its
  content needs, 14 at least with the file. A repository without the file is verified, bundled and
  declared byte for byte as before, with one exception: a `goals.json` in the work tree that git
  ignores now fails `resource verify`'s `checkout` precondition (`resource_verify_checkout_dirty`,
  `dirty_problem` `ignored_in_read_path`, no content step run), where 0.12.76 and before passed
  over it ([resource-verify.md](resource-verify.md), "Clean"). No outcome code is added. (In
  0.12.76 and before, `resource verify` passed over the file without a word and left it out of its
  zip, and `standard-pack build` gave it to `package bundle --goals`, which names only the first
  problem of a refused file.)
- **The installer's `place`** (the Lab's rescue module) takes `goals.json` from a bundle zip as a root
  file, placed last among them (after `schedule.json`, whose pools and rules its entries name), and
  sets it aside as `goals.json.old-<ms>` when the game root holds one and a later zip carries none;
  `undo` puts it back. A zip that carries the file declares a `package` maximum of 12 at least
  (G-R1 fix round 1, as a zip that carries the notices directory declares 10), 14 since Workflow
  #463 V1 fix round 3 (`GOALS_PACKAGE_REVISION`, the revision of version 2, as a zip whose packs
  declare variables declares 14): otherwise its plan refuses it `pack_invalid` caused by
  `standard_pack_root_invalid` with `goal_undeclared` at `<zip>\interfaces.json`, the pointer
  empty, before the hold and any write. A zip that declares 12, as a bundle of version 1 did, is
  refused so. It reads the file's name only, so the version is not judged here; its `post_check`
  runs `standard-pack check`, which reports a refused file (a version 1 file
  `goal_version_retired`) without failing.
- **The Runtime** opens no `goals.json` in G-R1: its root reader, its startup and run preparation,
  the policy crate and check-config read named files only. A later slice's scheduler workbench reads
  it with `schedule.json`. Since G-R2s (0.12.60) the workbench reads every package's declaration
  as this check does, with the same reader; since Workflow #455 G-R2x (0.12.77, register §1.499)
  that reading serves the warning for a package that cannot be read
  (`standard_pack_package_unreadable`) and the variables' judgement (from 0.12.60 to 0.12.76 it
  served the composer's irreversible mark too,
  [standard-pack-schedule.md](standard-pack-schedule.md), "The irreversible mark"), and since
  Workflow #455 G-SLx (0.12.78, register §1.501) the judgement reads only the variables each
  package declares, no longer the rule of irreversibility. **Since
  Workflow #463 V2 fix round 1 (Runtime 0.12.61, register §1.382)** the workbench reads
  `goals.json` too, where it reads every package's declaration (and so do the offline readers that
  read a root as it does, check-config and `actingd suspended`): with this reader, once per read of
  the root, for the variables' judgement alone ([package-variables.md](package-variables.md), "A
  variable that does not apply"): the names an accepted file takes, the parsed document kept with
  the root's declarations, a refused one taking none. Fix round 2: a refused file, or one that
  cannot be read, is `standard_pack_goals_not_applied` (warning; `goals_failure` `refused` with the
  reader's first problem as its cause, a `standard_pack_root_invalid`, or `unreadable` with the
  operating system's failure), recorded once per sha256 of a refused file and once until an
  unreadable one reads again; neither stops the root. No other reader of the Runtime opens it, and
  the programs of 0.12.60 open none.
- **The goal page** (Workflow #463 V3, Runtime 0.12.61, register §1.383; `GoalView` and
  `EditGoal`, [runtime-interface.md](runtime-interface.md), "Goal request kinds") opens no file: it
  takes the parsed document the workbench's last check of the root keeps
  (`RuntimeHost::standard_pack_goal_roots`), lists the entries in file order, each variable at its
  placement (a placement no package the active catalog binds on the application declares
  `not_offered`), and checks a name set by it against them. A refused file is named with its
  sha256 and first problem and lists no entry, as an unreadable or absent one; until the
  workbench has checked the root after a start the page answers `not_checked_yet` and an edit is
  refused with that word (fix round 2 of #1731: `root_failing` for a root no check since the start
  could read, `no_schedule` for a game without `schedule.json`).

## Interface

`package` revision 12 ([component-interfaces.md](component-interfaces.md)): a bundle that carries
`goals.json` version 1 declares `package` [12, 12] (the contract crate's `GOALS_PACKAGE_REVISION`
until version 2, below); a bundle without it keeps its revision, and the Runtime declares [3, 12]
(from Workflow #463 K1 on [3, 13], [component-interfaces.md](component-interfaces.md), the
`package` row 13; since Workflow #463 V1 [3, 14], [package-variables.md](package-variables.md),
"Interface"; since Workflow #463 K2 [3, 15], the `package` row 15). The installer of 0.12.60 refuses a zip that carries the file and declares a maximum
below 12 (`goal_undeclared`, "Who reads it"; below 14 since version 2). An
installer before 0.12.60 refuses such a zip in the plan, before the hold and any write: through its
`package` edge (`interface_edge`) when the zip declares 12, at its layout check (`pack_invalid`
naming `goals.json`) when it declares less. A Runtime before 0.12.60 reads a game root that holds
the file as one without it. **A rollback is not refused**: a Runtime before this slice treats the
file as absent; an installer before this slice refuses a bundle that carries the file, and when it
places a bundle without it, it leaves an existing `goals.json` where it is. `ledger`,
`runtime-client`, `actingd-config` and `runtime-outcomes` do not move: the seventeen words (the
sixteen of the file, `goal_variable_unknown` the fifteenth since Workflow #463 V1 and
`goal_version_retired` the sixteenth since its fix round 3, and `goal_undeclared`) are words of
the existing vocabulary `standard_pack_problem` under the existing code
`standard_pack_root_invalid`.

**Version 2, `package` revision 14** (Workflow #463 V1 fix round 3, Runtime 0.12.61; register
§1.378; [component-interfaces.md](component-interfaces.md), the `package` row 14). Release 0.12.60
read version 1, so the file of the five types is version 2, and a bundle that carries it declares
`package` [14, 14] (`GOALS_PACKAGE_REVISION` is 14 since; the revision also brings a package's
`variables`, which a placement names, [package-variables.md](package-variables.md), "Interface").
This build reads version 2 only (a version 1 file is `goal_version_retired`), and its installer
refuses a zip that carries the file and declares a maximum below 14 (`goal_undeclared`). What the
programs of 0.12.60 (`package` [3, 12], version 1 only) do with a version 2 file:

- **The Runtime** opens no `goals.json`: its root reader, its startup and run preparation, the
  policy crate, check-config and the workbench read named files only (G-R2s reads the packages'
  declarations, not this file). It reads a game root that holds a version 2 file as one without it.
- **The installer's `place`** refuses a bundle that declares [14, 14] through its `package` edge:
  the bundle's maximum is outside its own range [3, 12], so `interface_edge` at
  `<zip>\interfaces.json`, in the plan, before the hold and any write
  (`crates/lab-rescue/src/install/place.rs`, the edge check of `plan`). A bundle that carried a
  version 2 file and declared 12 would pass that edge and its `goal_undeclared`, and be placed;
  its post check's `standard-pack check` then warns as below and passes.
- **The Lab** reads `schema_version` `actingcommand.goals.v1` only (the contract crate's reader of
  0.12.60, `Check::document`): a version 2 file is `malformed` at `/schema_version`, and each
  `variable` entry `goal_type_unknown` at its `type` (its eight types have no `variable`), the
  other entries read on. `standard-pack check` reports each problem as a `standard_pack_root_invalid`
  warning and checks the root as without the file (`crates/resource-tooling/src/standard_pack_goals.rs`,
  `goal_issues` and `goal_warnings`); `package bundle --goals` refuses the bundle with its first
  problem, `malformed` at `/schema_version`, `--out` not written (`refuse_goals`).

**A rollback to 0.12.60 is not refused** for the file: no released bundle carries it; a root that
holds a version 2 file runs as without it on that Runtime, its check warns, and its installer
refuses a bundle that declares 14 and sets an existing `goals.json` aside when it places a bundle
without one, as it does today.

**Workflow #455 G-SLx (Runtime 0.12.78; register §1.501) moves no version and no revision.** The
file's shape and its reader do not change; only the references' check accepts more (an entry
that governs, reaches or follows a package that can do something irreversible), and the check is
the Tools', not the file's version. The document's versioning asks a new version for a new shape
("a released format does not change its shape in place", the head of this file), and there is
none, so a bundle that carries the file still declares 14. What the programs of 0.12.77 answer
for a version 2 file one of whose entries governs such a package:

- **`standard-pack check`** warns `goal_governs_irreversible` at each such entry (one
  `standard_pack_root_invalid` warning each, at its pointer), applies no entry and checks the root
  as without the file, as for any refused file; **`package bundle --goals`** refuses the bundle with
  that word, `--out` not written, and **`resource verify`** and **`standard-pack build`** fail their
  `bundle` step with it, no zip. Each refusal is clean: nothing is written or placed.
- **The installer's `place`** reads the file's name only and places it.
- **The Runtime's workbench** reads the file with the reader alone (it holds no references), so it
  accepts it and the goal page lists its entries. A variable such an entry places does not apply on
  that Runtime (its judgement keeps it from applying, `irreversible_by_root`), and a package whose
  own `provides` marks the capability and that declares variables cannot be read there at all
  (`standard_pack_package_unreadable`; its admission refuses it, `variables_on_irreversible`), which
  keeps every variable of its root from applying (`package_unreadable`). Its installer does not let
  such a package in through a bundle that declares its revision ([package-variables.md](package-variables.md),
  "Interface").

## What is not this file's

- **No value is stored in it.** A value is the installation's, written by a person or an agent
  (later slices); the file declares the entries, their bounds and their defaults only.
- **No pointer into the schedule.** An entry names a pool or rules, or places a variable; its
  default is its own or the variable's. No entry writes a rule's member, a configuration key, a
  remedy choice or a pack switch.
- **No entry kind writes anything else**: not a remedy choice or a capability's parameter (those
  stay in the remedy layer), not a configuration key, not a pack switch. The one run value an entry
  sets is a package's variable ([package-variables.md](package-variables.md)).
- **Real money is kept out by the packages themselves** (Workflow #455 G-SLx, Runtime 0.12.78,
  register [v012-register.md](v012-register.md) §1.501, on the owner's decision recorded there).
  No package holds a press that leads to a platform payment (a rule of pack making), so no goal
  entry and no variable can reach one; the Runtime holds no lock of its own for it. Everything else
  a package may do, premium currency, retiring, scrapping, buying, drawing and training that spends
  materials included, is a goal's and a variable's to govern like anything else:
  - an entry may govern, reach or follow a package that can do something irreversible: a `tier`
    over its rule or over a rule it follows, a `variable` entry whose variable it declares, an
    entry that reaches a package whose remedies list such a capability; the check holds each to
    the root as it holds any other ("The checks"), and a `resource` entry's goal raises every rule
    that produces its pool (since Workflow #455 G-R2x, 0.12.77, register §1.499;
    [resource-targets.md](resource-targets.md), "A task that can do something irreversible");
  - a package that provides an irreversible capability, marked by itself or by another package of
    the root, may declare variables, which apply and are handed to its runs as any package's
    ([package-variables.md](package-variables.md)).

  What stays: a capability's `irreversible` mark in a package's declaration, which the remedy layer
  reads ([remedies.md](remedies.md) §2.3), the Lab's `packs` and the console show, and the
  standard-pack trial asks an acknowledgement for ([standard-pack-trial.md](standard-pack-trial.md));
  a Lab operation that is irreversible records who started it. Until 0.12.77 this section kept
  "premium currency, real money, retiring and scrapping" out at three places: the check refused
  an entry that governed, reached or followed such a package (`goal_governs_irreversible`, judged
  by the contract crate's `IrreversiblePackages` over the whole root, the rules that follow a rule
  walked since G-R1 fix round 2), kernel admission refused variables on a package whose own
  `provides` marked such a capability (`variables_on_irreversible`) and `standard-pack check` kept
  a variable of a package irreversible by the whole root from applying
  (`variable_irreversible_by_root`), and the resource side was to review every file for entries
  about them; the scheduler's line (G-R2s) went with G-R2x. G-SLx removed the check's refusal, its
  walk over the rules that follow a rule (the policy crate's `standard_pack_rule_followers` went
  with it, read by nothing else), the two refusals of variables and that review.

## Before any pack carries the file

What G-R1 fix round 1 said a later slice owed before any standard pack carries `goals.json` is
built for the repository's verification (Workflow #455 GOALS-PACK-1, Runtime 0.12.77, register
[v012-register.md](v012-register.md) §1.498; "Who reads it"): `resource verify` checks a
repository's `goals.json` with the one check, names an accepted file in its answer and fails on a
refused one with every problem; its `bundle` step carries an accepted file in the zip it makes
under `--work`, beside `bundle.json`, byte for byte; and that zip's `interfaces.json` declares
`package` [14, 14] for it, which the installer's `place` takes. That zip is the verification's, not
a release: the four resource repositories make their release zips with `actinglab package bundle`
from their own release script, not with `resource verify` and not with `standard-pack build` (the
resource side's answer of 11-21; the Runtime side has not read those repositories). So this slice
gives the repository's verification the check and the file; a release carries `goals.json` only
once the resource side changes its release script (below). The Tools owe nothing more for the file: `standard-pack build` has carried an
accepted one and declared the revision its content needs since 0.12.61, and `package bundle
--goals` lays one. What is still owed, by the resource side:

- its release script moves to `standard-pack build`, or gives the file to `package bundle --goals`
  and writes `package` 14 at least into the zip's `interfaces.json` itself (`package bundle` writes
  none), so that its release zip carries an accepted `goals.json` and declares the revision;
- its CI's `resource verify` checks and carries the file only once its pinned Tools are 0.12.77 or
  later: an earlier `resource verify` leaves the file out of its zip and says nothing of it;
- a zip that carries the file and declares `package` [14, 14] is placed by the installers of
  0.12.61 and later; the installer of 0.12.60 (`package` [3, 12]) and every earlier one refuse it in
  the plan at their `package` edge (`interface_edge`), before the hold and any write.

(Until Workflow #455 G-SLx, 0.12.78, this list also owed a resource-side review of every file for
entries about premium currency, real money, retiring and scrapping; it went with the safety lines,
"What is not this file's".)
