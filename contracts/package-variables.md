# Package variables (`control.json` `variables`)

Workflow #463 L4, slices V1 and V2 (Runtime 0.12.61; [v012-register.md](v012-register.md) §1.378,
§1.382). A
package made by recording, or by hand, leaves named slots that are filled from outside: "which
one", "how many", "in which order". A **variable** is such a slot: a value the package's policies
read at run time and do not choose themselves. The owner's requirement (board #463): the value
may come from a person in the console, from an agent through the Lab, or from the package's own
default, which its author or the model's export writes.

This document is what V1 builds: the declaration, the names a game root shares, the root check,
the place on the goal page, the default's origin, and the interface revision; and what V2 builds
(Runtime 0.12.61, register §1.382): the run, "The run" below.

## What is built now, and what is not

**Built in V1:**

- the member `variables` of a package's `control.json`, read by one reader of the contract crate
  (`parse_package_variables`, `crates/actingcommand-contract/src/package_variables.rs`) that
  kernel admission and the source parser both call, each refusal at its JSON pointer;
- the root check of `actinglab standard-pack check` over the variables of a game root's packages,
  and its provenance listing; since fix round 1 it judges irreversibility over the whole root
  with the goal check's rule (`variable_irreversible_by_root`) and the listing marks such a
  variable as not applying;
- the `goals.json` entry type `variable`, which places a variable on the page, in version 2 of
  the file (fix round 3: a version 1 file is `goal_version_retired`;
  [standard-pack-goals.md](standard-pack-goals.md));
- `package` interface revision 14, and (fix round 1) the installer's `place` refusing a bundle
  whose packs declare variables and that declares less (`variable_undeclared`); since fix round 3
  the revision also brings `goals.json` version 2, and `place` refuses a bundle that carries the
  file and declares less (`goal_undeclared`).

**Built in V2** (Runtime 0.12.61, register §1.382; "The run" below):

- a policy dispatch hands its run a value for every variable its package declares: the value set
  on the application (its entry row of that name in `session.resource_targets`), judged against the
  declaration when the run is to read it, else the package default; recorded with its source as
  `variable_values` on the dispatch's `policy_dispatch_parameters`, and overlaid as
  `session.dispatch.parameter.<name>` with the kind's type;
- a set value not applied is warned once (`goal_value_not_applied`);
- a run without a dispatch reads the defaults; the Lab's scan replay takes
  `--variable <name>=<json>`.

**Built in V3** (Runtime 0.12.61, register §1.383; "Reading and setting a value by name" below):
the requests `GoalView` and `EditGoal` (`runtime-client` 9), which read an application's goal page
and set or clear one value by its name, the Lab's rows `goals`, `goals get`, `goals set`,
`goals clear` and `package variables` with their MCP tools, and the console's list taking both
kinds.

**Built in S3-8, S3-9** (Runtime 0.12.68, register §1.444; "Runs without a dispatch" below;
[runtime-interface.md](runtime-interface.md), "The variable values of a contained run"): a direct
run's request gives values (`ContainedTaskRequest.variables`, `runtime-client` 11), which the host
judges before the run's admission as a dispatch judges a value and overlays where a dispatch's
values are overlaid; actinglab's `run start` and `standard-pack trial` take
`--variable <name>=<json>` and send them.

**What does not apply is enforced at run time since V2.** "Does not apply" (a name the root check
reports, "Shared names and the root check") is, in `actinglab standard-pack check`, a warning: the
check only warns wherever it runs (its exit stays 0). The installer runs it: the post check of
`place` runs the installation's `<root>\tools\actinglab.exe --json standard-pack check --root
<game root>` after every placement (its row `not_evaluated` when the installation holds no such
file) and reads only its exit code (`crates/lab-rescue/src/install/place.rs`, `check_step`), so a
variable warning does not stop a placement, its row `standard_pack_check` is `pass` with
`{exit: 0}`, and the answer, warnings included, is not kept (it is kept only when the check
fails). `resource verify`'s `bundle` step runs it on its installed root and relays the warnings.
`package bundle` does not run it. Since V2 the Runtime runs the same check (the contract crate's
`check_root_variables` and `IrreversiblePackages`, no second copy), and a variable that does not
apply is handed no value from outside: the run reads its default, whatever the application holds
("A variable that does not apply"). Since its fix round 1 (the coordinator's ruling), with a
composed standard pack the check is the root check over the whole root, as `standard-pack check`
runs it: every package of the registry, a package no rule runs included, against the names the
root's accepted `goals.json` takes, which the Runtime's scheduler workbench reads for this alone.

**Not built; nothing here promises it:**

- **The console's goal page is not drawn.** V3 builds the requests the console's page sends and
  puts both on the console's list; the page itself is slice V5 (the console's repository).
- **No other live run without a dispatch takes a value.** A run the host starts itself and a Lab
  run read the defaults, and so does a direct run whose request gives none; since S3-8, S3-9 only a
  direct run's request (`run start --variable`, `standard-pack trial --variable`, their MCP tools)
  and the offline scan replay take values. actingctl's first-generation MCP server
  (`ac_run_pack`) gives none. No run's answer lists the values it was given; since Workflow #463
  CH-1 (0.12.71) its admission record does ("The run's admission record").
- **A recording writes a variable for every select.** Since #463 S2-6 (0.12.68, register §1.442;
  fix round 1, the coordinator's ruling R1; [lab-recording.md](lab-recording.md), "Selecting in a
  list" and "The wanted value is a variable of the package") `record stop` declares one `ordered`
  variable per select step, marked first as holding a choice or given whole, its name the select's
  wanted name, its options the select's names and its default the select's default alone, the
  order the recording holds, and that step's policy reads it through `keyed_fact`; a run handed no
  value chooses what the constant of before chose. Editing the options and the texts after `record
  stop` other than by declaring the select again is not built.
- **No table-backed list** (`from_table`), no `table` type and no value-test operand that reads a
  variable: later slices, each with its own revision.

## The declaration

A top-level member of `control.json`, beside `provides`, never under it:

```json
"variables": [
  {"name": "pick", "type": "choice", "title": {"en": "Item"},
   "options": [{"value": "item_a", "title": {"en": "Item A"}},
               {"value": "item_b", "title": {"en": "Item B"}}],
   "default": "item_b", "default_origin": "model"},
  {"name": "floor", "type": "integer", "title": {"en": "Keep at least"},
   "min": 0, "max": 200, "step": 10, "default": 0},
  {"name": "order", "type": "ordered", "title": {"en": "Order"},
   "options": [{"value": "item_a", "title": {"en": "Item A"}},
               {"value": "item_b", "title": {"en": "Item B"}}],
   "min_count": 0, "max_count": 2, "default": ["item_b", "item_a"]},
  {"name": "extra", "type": "switch", "title": {"en": "Also the extra"}, "default": false}
]
```

Each entry has the members of the value types `goals.json` version 1 had (`integer`, `choice`,
`switch`, `ordered`; version 2 has none of them), without `governs` (the governed package is the
declaring one) and without `from_table`, and the optional `default_origin`. The value-type code is
the goal declaration's, moved into this reader: the same texts, blocks, ranges, steps, counts and
defaults are judged by the same code.

| Member | Rule |
|---|---|
| `variables` | optional; 1 to 16 entries |
| `name` | required; `^[a-z][a-z0-9_]{0,63}$` (the parameter grammar, so a name can be a parameter's and a goal entry's); unique across the package's `variables` and every `provides[].parameters[].name` |
| `type` | required; `choice`, `integer`, `switch` or `ordered` |
| `title`, `help` | `title` required, `help` optional; a text per locale, as `goals.json`'s (1 to 16 locales of 1 to 35 bytes; 1 to 128 bytes per locale for `title`, 1 to 1024 for `help`) |
| `block` | optional; the type's base block only: `choice`, `number`, `switch`, `ordered_list` |
| `choice` | `options: [{value, title}]`, 2 to 32; `default` one of them |
| `integer` | `min`, `max`, `step?` (absent: 1), `default`, safe integers, `min` not above `max`, `step` at least 1, `default` in `min..=max` on a step from `min`; `unit?` a text per locale or one of `second`, `minute`, `hour`, `day` (the values are then milliseconds) |
| `switch` | `default`, a boolean |
| `ordered` | `options: [{value, title}]`, 1 to 1024; `min_count`, `max_count` (1 to 32), `min_count` at most `max_count` and the number of options; `default` a list of option values, each once, of `min_count..=max_count` values |
| option values | `^[a-z][a-z0-9_]{0,63}$`, each once in its list, so every option can be a member of an `enum_string` fact and an identity id |
| `default_origin` | optional; only the word `"model"` |

Every other member is refused. A **set** (several things on or off) is an `ordered` variable whose
order the package ignores (`min_count` 0); one pick among many is an `ordered` variable with
`min_count` and `max_count` 1.

**Irreversibility is the line between a parameter and a variable.** A package that provides an
irreversible capability (a `provides[]` entry with `irreversible: true`) declares no variable: the
values that widen what an irreversible act may consume are its capability's parameters, set with
the permission they scope ([remedies.md](remedies.md) §2.2, §2.3). Admission judges the package's
own marks only. A package that provides, without the mark, a capability another package of the
root marks irreversible, or whose remedies list one, can only be judged with the root, by the one
rule the goal check uses (the contract crate's `IrreversiblePackages`): its variables do not
apply, `standard-pack check` reports each of them `variable_irreversible_by_root` (fix round 1,
"Shared names and the root check"), a `goals.json` entry that places one is
`goal_governs_irreversible` ([standard-pack-goals.md](standard-pack-goals.md), "The checks"), and
the slice that resolves values (V2) runs such a package on its defaults.

## The refusals

The first rule broken refuses the member, in this order: the irreversible package; the list; then
entry by entry, the first problem of the value-type rules; then the entry's name, once the entry
reads.

| Reason | When | JSON pointer |
|---|---|---|
| `variables_on_irreversible` | a capability of the package's `provides` is irreversible | `/variables` |
| `variables_invalid` | not a list of 1 to 16 entries; an entry that is no object; a member unknown, missing or not of its type; a name, an option value or a unit word outside its form; a text, a list or a name of a size outside its limit; a type that is none of the four; a block that is not the type's; a range, a step or a count that cannot hold a value; an option written twice; a default that is no value of the declaration; a `default_origin` other than `"model"` | the member that breaks the rule (`/variables` for the list) |
| `variable_name_taken` | the name of an earlier variable or of any parameter of `provides` | `/variables/<i>/name` |

- **Kernel admission** refuses `contained_task_control_invalid` with the detail
  `reason=<reason> json_pointer=<pointer>` (`crates/execution-kernel/src/contained_task/remedies.rs`,
  `parse_control_variables`), beside the words of `provides`.
- **The source parser**, the declaration gate every admission path runs on `control.json` first
  (`crates/pack-containment/src/source/declarations.rs`), refuses `resource_declaration_invalid`,
  `invalid_value` at the same pointer, its message ending with the reason. Since fix round 1 the
  containment keeps that text when it relays the gate's refusal (`declaration_error`,
  `crates/pack-containment/src/lib.rs`), so the refusal of an admission by path, as
  `standard-pack check` and `package bundle` admit a pack, names the reason too.
- The three reasons are words of the `reason` of `contained_task_control_invalid`, registered in
  this table and in [resource-declarations.md](resource-declarations.md), the `control.json`
  members, as `provides`' are.

A `control.json` without `variables` reads, serializes and runs byte for byte as before: no
member is added to anything a run or a record carries.

## Shared names and the root check

A variable's name is a name of the game root, not of the package. Several packages of one root
may declare the same name only with **byte-identical declarations** (the entry as written, member
order included, compared in compact JSON); they then form one variable, with one value per
application (one entry row of its name, which every package declaring it reads, V2).

`actinglab standard-pack check --root <game root>` reads every package of the registry with the
Runtime's metadata reader (`read_remedy_declarations`, which answers a package's `variables` and the
fact keys its designated effects read their caps from) and runs the contract crate's root check
(`check_root_variables`). Name by name, in the order the root first declares it (registry order,
then declaration order), the first of these holds:

| `standard_pack_problem` | When | `json_pointer`, in each declaring package's `control.json` |
|---|---|---|
| `variable_out_of_limits` | the name is the root's 97th or later (96 names: the entry rows an application's goal record keeps, as `goals.json` holds at most 96 entries) | `/variables/<i>/name` of each declaration |
| `variable_conflict` | the declarations of the name are not byte-identical | `/variables/<i>` of each declaration |
| `variable_conflict` | an entry of an accepted `goals.json` uses the name for anything but its placement | `/variables/<i>/name` of each declaration |
| `variable_irreversible_by_root` (fix round 1) | a package that declares it counts as able to do something irreversible by the whole root: it provides, unmarked, a capability another package of the root marks irreversible, or a remedy of its mappings lists one (the contract crate's `IrreversiblePackages`, the one copy of the rule the goal check holds a placement to; admission refuses `variables` on a package whose own `provides` marks the capability) | `/variables/<i>` of each such package's declaration |
| `variable_cap_range` | a package that declares it reads a designated effect's cap from `session.dispatch.parameter.<name>` (`max_repeats: {fact_key}`, which takes a whole number from 0 to 1000), and the variable is of another type than `integer` (fix round 1: a cap is read from an `integer` variable only), or an `integer` whose `min` is below 0 (fix round 1) or whose `max` is above 1000, the first of these | `/variables/<i>/type`, `/variables/<i>/min` or `/variables/<i>/max` of each such package's declaration |

Each problem is one `standard_pack_root_invalid` warning with `game`, `path` (the package's
`control.json`), `standard_pack_problem` and `json_pointer`, after the composition's warnings and
before those of `goals.json`. **The root still runs**, and no variable of that name applies (as a
refused `goals.json` applies no entry). The check warns only, wherever it runs: the installer's
`place` runs it after every placement and reads its exit code only, so a warning passes its post
check; `package bundle` does not run it. Since V2 the Runtime enforces it: it runs the same check
and hands a variable that does not apply no value from outside, for a conflicting name and for a
package irreversible by the root alike ("The run"); since V2's fix round 1, with a composed
standard pack, over the whole root as this check runs it, `goals.json` included. A `goals.json`
the reader refuses applies no entry, so it takes no name. The root check's four words, like V1's three others
(`goal_variable_unknown` of the placement, fix round 1's `variable_undeclared` of the installer
and fix round 3's `goal_version_retired` of a version 1 `goals.json`), are words of the existing
vocabulary `standard_pack_problem` under the existing code `standard_pack_root_invalid`.

**The provenance listing.** The answer of `standard-pack check` gains, after `models`,
`variable_provenance` `{pack: [names], model: [names]}` when a package of the root declares
variables: each variable that applies, once, in the root's order, by who wrote its default
(`pack` without `default_origin`, `model` with it), as the schedule's provenance lists its fixed
values ([standard-pack-schedule.md](standard-pack-schedule.md), "Provenance"). Since fix round 1
the listing marks as not applying each variable that a package counting as irreversible by the
whole root declares, which would otherwise read as in effect: `not_applying`
`[{name, standard_pack_problem: "variable_irreversible_by_root"}]`, once each in the root's order,
absent when there is none. A name another problem keeps from applying is in neither list (its
warning says why). A root whose packages declare none answers without the member, byte for byte
as before. `package bundle` runs no root check of variables.

## The default's origin

`"default_origin": "model"` says that the model's export wrote the default; without it the
package's own hand wrote it. Only `"model"` is a value. It decides nothing: only the provenance
listing reads it. A hand edit that leaves `model` in place stays the writer's responsibility, as
`fixed_value_origin` does. A changed default is a package change: the package's content hash
covers `control.json`.

## The place on the page

`goals.json`'s entry type `variable` (`{name, type: "variable", block?}`) places on the goal page
the variable of that name that some package of the root declares
([standard-pack-goals.md](standard-pack-goals.md), "The five types of version 2"). It writes no
text and no value: the variable carries its own. Its `block`, when written, is the placed
variable's base block (`goal_block_mismatch` otherwise). A name no package of the root declares is
`goal_variable_unknown`; a placement is held to every package that declares the variable as a
governed package was (`goal_governs_irreversible`, `goal_package_unknown`). The type replaces the
four types that governed packages. Release 0.12.60 shipped version 1 with those four, and a
released format does not change its shape in place, so the file of the five types is version 2
(`actingcommand.goals.v2`, fix round 3, the coordinator's ruling 2 on the design's questions): a
version 1 file is `goal_version_retired` at `/schema_version`; no pack carries either.

## The run

Workflow #463 V2 (Runtime 0.12.61; register §1.382). The owner's requirement (board #463, 11-14):
the number in a variable may come from a mathematical model, from a person or an agent, or from
elsewhere. The Runtime hands a run the value it is given and computes none: there are two sources,
the value set on the application and the package default.

**Resolution, once per selected dispatch.** For every variable of the dispatched package, in
declaration order:

1. **Set on the application**: the entry row of the variable's name in the application's
   `session.resource_targets` v3 record ([resource-targets.md](resource-targets.md), "Document
   `actingcommand.resource-targets.v3`"), with its `set_unix_ms` and `channel`, as the evaluation's
   facts hold it (the Runtime's memory store, projected for the evaluation: no ledger read is added
   to a dispatch); a record of another version, or one the v3 reader refuses, holds no entry.
2. **The package default**, otherwise.

There is no third source: the Runtime never computes a value and never rewrites a stored one. An
option such as `auto` hands the choice to the package's own policy, which reads facts and tables.
Resolution does not read `scheduling.remedy.enabled`: it runs with remedies on or off.

**A set value that no longer fits its declaration** is judged when a run is to read it
(`PackageVariable::judge`, the contract crate's one copy, which the kernel and the Lab read too);
the run reads the default, and the record and the warning name the word of the vocabulary
`variable_problem`:

| Kind | The set value | The run reads | `variable_problem` |
|---|---|---|---|
| `choice` | no string | the default | `type_changed` |
| `choice` | a string that is no option | the default | `not_an_option` |
| `integer` | no whole number in the safe range | the default | `type_changed` |
| `integer` | below `min` or above `max` | the default | `out_of_range` |
| `integer` | in the range, on no step from `min` | the default | `off_step` |
| `switch` | no boolean | the default | `type_changed` |
| `ordered` | no list of strings | the default | `type_changed` |
| `ordered` | a list that names a key twice | the default | `key_repeated` |
| `ordered` | keys the declaration no longer offers, the rest at least `min_count` | the rest, in its order (source `set`; fix round 1: no warning, since it was applied) | `keys_left_out` (fix round 1) |
| `ordered` | keys no longer offered, the rest fewer than `min_count` | the default | `keys_removed` |
| `ordered` | offered keys only, fewer than `min_count` or more than `max_count` | the default | `count_out_of_range` |
| any | (fix round 1) a stored entry row whose `set_unix_ms` or `channel` cannot be read (the v3 reader holds both to their form, and, fix round 2, both read the one table of the channels a row may name, so no record it accepts gives one: the word stays for the case it would not hold) | the default, the record naming no setter | `entry_unreadable` |

Fix round 1: no value is refused for its length. The goal store keeps a value of up to 1024 bytes
of canonical text ([resource-targets.md](resource-targets.md), `entries[i].value`), and the record
carries it in a `json` key of the same bound, so every value a run is to read of a set value fits
(`value_too_long` is gone).

**A variable that does not apply** is handed no value from outside: the run reads its default,
whatever the application holds. The Runtime runs the root check (`check_root_variables`, with
`IrreversiblePackages`; the rules stay the contract crate's, `variables_not_applying` gives each
name its word) where it can judge the root. Fix round 2: the rules of "does not apply" have one
copy, the contract crate's `judge_root_variables`, which the workbench, the table without a
composed standard pack and `standard-pack check` all call. It takes the goal document as its
reader read it (`RootGoalDocument`): absent, accepted with its entries (each takes the name it
uses for anything but a placement), refused (it applies no entry and takes no name) or unreadable
(no name of the root applies that no other rule already keeps: `goals_unreadable`); and it
answers, per name, the packages that declare it, its first declaration, and why it does not apply
with the detail of the cause (`RootVariableVerdict`; "What the next slice reads" below).
`standard-pack check` never passes an unreadable file: a `goals.json` it cannot read is its usage
error naming the file, and it checks nothing further, so it answers no root of which the Runtime
would judge otherwise; for every file it can read, absent, accepted or refused, its warnings and
its provenance listing come from the same judgement as the Runtime's.

- **With a composed standard pack** (fix round 1, the coordinator's ruling: the judgement is the
  root check's, over the whole root). The scheduler workbench runs it where it already reads the
  whole root, at each read of a game root (`read_declarations` of
  `crates/runtime-host/src/host/standard_pack_schedule.rs`): over every package of the registry, a
  package no rule runs included, read with the Runtime's metadata reader, with the rule of
  irreversibility built from what every package provides and what its remedies list, against the
  names the root's `goals.json` takes for anything but a placement, read with the contract crate's
  reader (`parse_standard_pack_goals`): an accepted file's names, none for a refused or absent file,
  and, for a file that cannot be read, every name of the root does not apply (`goals_unreadable`).
  Fix round 2: a refused file and one that cannot be read are each `standard_pack_goals_not_applied`
  (warning, ladder class `none`; `game`, `path` and the required `goals_failure`, `refused` with
  the reader's first problem as its cause, a `standard_pack_root_invalid` naming the file, or
  `unreadable` with the operating system's failure as its cause), recorded once per sha256 of a
  refused file and once until an unreadable one reads again: the root runs, and only its goal
  document is not applied, which `standard_pack_root_invalid` and `standard_pack_root_unreadable`
  do not say (for every other root file they mean the root is not composed). The parsed document,
  or why it is not applied, is kept with the root's declarations and, from one check to the next,
  in the workbench's state. The workbench reads a root again at a check (at most once per
  `standard_packs.check_interval_ms`) when `schedule.json`, `bundle.json`, `goals.json` or the
  declaration files of a package it registered changed, so a package replaced in place is judged
  with its new declaration after the next read; it keeps each game's judgement in memory and
  builds the table a dispatch reads from memory (no file and no ledger read at a dispatch). A game
  whose root fails at a check keeps its last judgement; its instances have no goals in effect
  meanwhile, so each of its variables reads its default. Fix round 2 (the coordinator's ruling:
  this text is right, and the code follows it): while a game's root fails its check, or its
  composition is refused or not activated (the games the workbench notes as uncomposed, whose
  instances' target records the cycle leaves out), nothing says whether a variable applies: each
  check builds the table with those games unjudged, so every variable of their instances reads its
  default with `root_unchecked`, the record says so and `goal_value_not_applied` is written once,
  as for any value not applied. The first composition of every game that succeeds judges them
  again. What the workbench notes is what the code does (`note_uncomposed`): a check adds to the
  note the games whose root failed and, when the composition is refused, every game in use whose
  files the generation in force does not hold as they are now; the next check does not clear the
  note by itself, only a check whose composition of every game succeeds. Two states follow from it,
  which the coordinator confirms as they are:
  - *A sound root, judged, in a game whose generation in force does not hold its present files*
    (another game's root is broken, or the whole composition was refused, as for its size): the
    values set on its applications read their defaults, the record says `root_unchecked`, and
    `goal_value_not_applied` is written once per value; they apply again when a composition
    succeeds.
  - *A game whose `schedule.json` is refused and that falls back to its earlier fragment* counts as
    composed: it is not noted, and the values set on its applications apply.
- **Without a composed standard pack** (a configured catalog), as V2 built it: once per remedy
  table, over the packages the table reads, grouped by the game of the instances each catalog task
  covers: each package once in catalog order, no `goals.json` (so `goal_name_conflict` and
  `goals_unreadable` do not arise there), and a task whose package could not be read counts as an
  unreadable package of its game, the safe side of the rule. The table is built at the first
  evaluation under a catalog, again when the procedure manifest in place changes, and at the remedy
  table's interval while a round is open, a stop recorded or an instance waits for its client
  update; it is memory only and dropped with the table. A package no catalog task binds is not
  read there.

Each name the check reports takes the word of its problem (one word, one cause):

| The root check's problem | `variable_problem` |
|---|---|
| `variable_conflict` at a declaration | `declaration_conflict` |
| `variable_conflict` at the name (a `goals.json` entry) | `goal_name_conflict` |
| `variable_irreversible_by_root`, a declaring package providing or listing a capability a package of the root marks irreversible | `irreversible_by_root` |
| `variable_irreversible_by_root` only because a package of the root could not be read (the rule's safe side counts every package) | `package_unreadable` (fix round 1) |
| `variable_out_of_limits` | `name_out_of_limits` |
| `variable_cap_range` | `cap_range` |
| (fix round 1) the root's `goals.json` could not be read | `goals_unreadable` |
| no check judges the instance's game now: with a composed standard pack (fix round 2), the workbench notes the game uncomposed (its root failed a check, or a composition was refused while the generation in force did not hold the game with its present files), until a check whose composition of every game succeeds; without one, the table was built before an instance of it was known | `root_unchecked` |

**A dispatched package whose variables cannot be read** (fix round 2). With a composed standard
pack, a dispatch whose package the workbench could not read (a declaration file's metadata, the
reader's refusal, a package without a registry entry, or a game root it has not read since the
start) has no variable in the table: the dispatch carries no `variable_values` and its run reads
the package's defaults, whatever is set. Nothing is skipped silently: the cycle writes
`goal_value_package_unreadable` (warning, ladder class `none`; `instance_id`, `task_id`,
`package_id`, all required), once per (application, package, state of the package's files: its
declaration files' marks and the reader's refusal) after a start, in memory only, as a Warning
`runtime.failed` record of the policy driver; the package's own `standard_pack_package_unreadable`
says why it cannot be read. Without a composed standard pack the remedy table names such a package
on its developer line (`actingd remedy_table unreadable`), as before.

**The overlay.** Each variable is overlaid as `session.dispatch.parameter.<name>`, the key remedy
parameters use, with its kind's type (`PackageVariable::fact_value`): a `choice` a string, a
`switch` a boolean, an `integer` an integer, an `ordered` variable a record list of one row
`{key, rank}` per key in its order, `rank` from 1. It is observed at the dispatch's admission, in
instance scope, with confidence 1000 and no expiry, after the parameters and before the schedule
values, in every instance fact snapshot the host takes for the run (the select steps' and the
designated effects' cap read), and the select step's `input_sha256` covers it. A policy reads a
`choice` as an `enum_string` fact, a `switch` as `boolean`, an `integer` as `integer` (an
`integer_compare` threshold, or `max_repeats: {fact_key}`, whose cap of 0 to 1000 the root check
holds the variable to), and an `ordered` variable through `keyed_fact` (key column `key`, column
`rank`). Within one package a variable and a parameter never share a name (`variable_name_taken`),
so one overlay holds each key once; a name shared across packages is overlaid only for the run of
a package that declares it. The kernel reads the dispatched value (`value_json`) under the
prepared package's own declaration, so a package changed since its dispatch reads the default for
a value it no longer takes; the dispatch's record still says what was dispatched.

**A limit, now recorded** (fix round 1; Workflow #463 CH-1, 0.12.71): a package replaced between
a dispatch and its run's start may read the default while the dispatch's record says `set`: the
dispatch judged the value under the declaration the workbench last read, the run under the one it
prepares. The record names the declaration's `declaration_sha256` only in the warning. Since CH-1
the run's admission record says what the run read ("The run's admission record"): it lists only
the values of its dispatch the run read, so such a run's record names no value for that variable
(it read its default), and its `content_hash` is the content it was prepared with.

**The record.** The dispatch's `policy_dispatch_parameters` reason (an existing info outcome)
gains `variable_values`, one record per variable of the package in declaration order:
`{variable_name, variable_source, value_json?, set_unix_ms?, set_channel?, variable_problem?}`,
and (fix round 2, in the ledger record only) `value_sha256?, value_bytes?, value_json_left_out?`.
`variable_source` is `set` or `default`; `value_json` is the value the run reads, as compact JSON,
a key of the type `json` (fix round 1: up to 1024 bytes, the goal store's bound; absent for a
default longer than that, which the run reads from its package, and, fix round 2, in a ledger
record whose text did not fit, "The record's size" below); `set_unix_ms` and `set_channel`
are the entry row's whenever the application holds a value, applied or not, and both can be read;
`variable_problem` says why a set value was not applied, or (fix round 1) `keys_left_out` that an
`ordered` value was applied in part, its source `set`. A pair whose remedy plan
gives it no such reason gets one of these values alone. A package without `variables` gets no
record: its reason chain is byte for byte as before.

**The record's size** (fix round 2). The dispatch's reasons are recorded in its policy dispatch
intent event, at most 128 reasons, each an outcome in its record form, whose bound is an
envelope's 32 KiB less the envelope's own room (`ENVELOPE_BODY_BUDGET`, 32 640 bytes; the emitter
cuts only `raw_text` and links to keep it, never a value); the event sets no byte bound of its own
beyond its reasons' (a query page holds 768 KiB, a frame 1 MiB). Sixteen values of 1024 bytes (the
most a package declares, each the most the goal store keeps, an `ordered` list of 32 keys with
64-byte names) take about 20 KB and fit; beside the most the same reason carries otherwise (64 run
parameters of 64-byte names and values, the schedule and spend values) they do not, by about
600 bytes. So the ledger records such a reason in its recorded form
(`recorded_dispatch_parameters`): for a value whose text does not fit, the longest first until the
reason fits, the record carries `value_sha256` (the text's sha256) and `value_bytes` (its length)
in place of `value_json`, and `value_json_left_out: true`. The dispatch is never dropped and the
run reads every value whole from the reason chain it was admitted with, in memory; the intent
event and a replay's comparison are built from the same recorded form. A reason that fits, and
every other reason, is recorded as it is. No persisted shape changes: SS-6 added values
to the same reason the same way, and every 0.12 reader keeps an outcome's values as a map without
a key check, so the ledger, the console and the Lab of 0.12.60 read such a reason as before and
show the values they know.

**The warning.** `goal_value_not_applied` (warning; runtime-host) is written when a new dispatch
does not hand its run a value set on the application, once per (application, variable name,
declaration sha256, and, fix round 1, the stored value's sha256) after a start, so a person who
replaces one value not applied by another is warned again, as a Warning `runtime.failed` record of
the policy driver (`policy_fact_rows_exceeded`'s form): `instance_id`, `task_id`, `package_id`,
`variable_name`, `declaration_sha256` and the required `variable_problem`. "Once" is kept in memory
only: after a restart it may be written once more; no state file and no record keeps it. Nothing
is refused and the run goes on with the default. A value applied in part (`keys_left_out`) is not
warned.

**Runs without a dispatch.** A direct run whose request gives no value (since S3-8 a direct run
may be given values, below), a run the host starts itself, a Lab run and the offline replay read
every variable at its default and record nothing, as parameters do (`default_run_values`). The
Lab's scan replay (`actinglab replay --select <operation> --scan
<file>`) takes `--variable <name>=<json>`, the counterpart of `--parameter`: the value replaces the
variable's default when the declaration takes it exactly as given; a name the package does not
declare, a value it does not take, or a right side that is no JSON is `validation_failed` naming
`--variable` with `argument_problem` `not_recognised`, a name given twice `repeated`, and
`--variable` without `--select` `conflicting`.

**A direct run given values** (Workflow #463 S3-8, S3-9, Runtime 0.12.68, register §1.444;
`runtime-client` 11; [runtime-interface.md](runtime-interface.md), "The variable values of a
contained run"). A direct run's request may carry `variables` (`{name: json}`, at most 16, each
under a name of a variable's form). The host judges them in the run's preparation, before any
record, claim or lease, each in the order of its name, with the dispatch's own pieces and no
second judgement: a name the package does not declare is `run_variable_undeclared`; a variable
that does not apply on the instance now, as the variable table a dispatch reads says it
(`VariableTable::root_word` for the instance's game, "A variable that does not apply" above), is
`run_variable_not_applying` with that word; a value is judged by `resolve_variable` as the
application's stored entry set through the request's channel, and anything but the value taken
whole (`set` and no word) is `run_variable_value_refused` with the judgement's word, an `ordered`
value a dispatch would take in part (`keys_left_out`) included, as `EditGoal` refuses it; a
judgement that gives neither the value nor a word, which cannot happen, is
`run_variable_unjudged` (the 0.12.68 train's fix round). Each is Denied with `package_id` and
`variable_name` (and `variable_problem`), and nothing runs. The
values pass into the run's parameters by the line a dispatch's values pass through
(`dispatched_variable_values`, `with_variables`), so the package reads each as
`session.dispatch.parameter.<name>` with its kind's type, and a variable not given reads its
default; a request without values reads every default, as before. S3-8 recorded nothing of a
direct run's values, as nothing of its parameters: its command record carries no parameters, and
recording them needed a new ledger member, which Workflow #463 CH-1 adds (0.12.71, ledger
revision 9, "The run's admission record"); the parameters are still not recorded. actinglab's `run
start --variable <name>=<json>` (repeatable) reads the values with the scan replay's own parser
(`lab_ops::core_ops::parse_variables`, one function for the three commands) before anything is
sent (fix round 1: more than 16 of them are `--variable` `out_of_range`, the word of a count out
of its range), and without `--variable` sends the request of revision 10 byte for byte;
`standard-pack trial` sends the values its plan shows
([standard-pack-trial.md](standard-pack-trial.md), "The run"). A Runtime below revision 11 cannot
decode a request that carries values: it ends the connection, nothing runs, and the client reports
the connection ended.

**The run's admission record** (Workflow #463 CH-1, Runtime 0.12.71, ledger revision 9; register
[v012-register.md](v012-register.md) §1.455; the owner's ruling of 11-16 on board #463, option A
of question 220). The run's `task.requested` (`PackageAdmitted`) carries the optional member
`variables`: the values handed to the run from outside, sorted by name, each
`{variable_name, value_json}` with the value's compact JSON text when that text is at most 1024
bytes (`MAX_VARIABLE_VALUE_JSON_BYTES`), else `{variable_name, value_sha256, value_bytes}`, the
text's SHA-256 (64 lowercase hexadecimal digits) and length only (`AdmittedVariable`):

- a direct run, the trial's run included: the values its request gave (judged before its
  admission, "A direct run given values", so each is the value the run reads); a variable left
  to its default is not in it, since its request handed it none;
- a policy run: the values of its dispatch's `variable_values` that the run read, each as it read
  it, judged under the declaration of the package it prepared by the rule of its overlay
  (`dispatched_variables_read`, beside `dispatched_variable_values`): a variable the dispatch
  names no value for, or whose value that declaration no longer takes, reads its default and is
  not listed; an `ordered` value taken in part is listed as the run read it. So it holds every
  value its dispatch carried and the run read, defaults included (a dispatch hands down a default
  of at most 1024 bytes as it hands down a set value, "The record" above), and does not say which
  were set and which were defaults (CH-1 fix round 1, the coordinator's ruling: the code stays as
  it is): the member is what was handed to the run from outside, and the dispatch's own reason
  says where each value came from (`variable_source`);
- a run handed no value (none given, a package without variables, a run the host starts itself)
  writes no `variables` member: its record is byte for byte the record without it.

Beside it, `content_hash` is the content hash the admission measured on the bytes it read
([policy-suspension.md](policy-suspension.md), "Content hash"). Both are only recorded: nothing
reads them to decide, the record's validation refuses a malformed member
(`invalid_task_semantic_payload`), and the public projection a client reads carries neither (the
coordinator's ruling: no data value leaves through it). The Lab's own reading of the record and
the ledger tools read them; `run show` answering them is a later slice. Which tiers of the
projection and which exports carry them (an exported evidence package does, so a person who hands
one on hands the values too): [ledger-observer.md](ledger-observer.md), "Later rulings applied",
item 1.

**The admission record's size** (CH-1 fix round 1). With a value's text recorded whole only up to
1024 bytes and at most sixteen values (a package declares at most 16 variables, a direct run's
request carries at most 16 values), the record is about 20 KB at most: an entry of a 1024-byte
text, escaped once inside the record (about 1.09 KB for a list of 32 keys), with its name takes
about 1.2 KB, sixteen of them about 19 KB, and the content hash and the rest of the fact well
under 1 KB. A longer text is recorded by its SHA-256 and length and can come only from a direct
run (a dispatch hands down no text above 1024 bytes). Nothing refuses, cuts or fails on it:
neither the ledger nor this fact's validation sets a byte bound on the record, and the bounds a
record meets on its way out,
a query page's 768 KiB (`MAX_RUNTIME_EVENT_QUERY_RESPONSE_BYTES`) and a frame's 1 MiB, are far
above it; the 32 640 bytes of an outcome's record form ("The record's size" above) bound outcomes,
not this fact.

**What the next slice reads** (fix round 2, for V3's view; no request, Lab row or view is added
here). From memory, never from a file:

- `RuntimeHost::standard_pack_goal_roots` (`None` without a composed standard pack): per game root
  the workbench read, a `StandardPackGoalRoot` kept from one check to the next, with the variable
  table its last check built (`StandardPackGoalRoots.table`, the one a dispatch reads):
  `goals` (`RootGoals`: absent, accepted with the parsed document, refused with its problems and
  sha256, or unreadable with the operating system's failure) and `variables`
  (`RootVariableJudgement`).
- `RootVariableJudgement::verdicts`: per name of the root, in the order the root first declares
  it, a `RootVariableVerdict`: the packages that declare it, its first declaration, and when it
  does not apply a `NotApplyingCause`, the word and its detail (`VariableCause`: the packages whose
  declaration conflicts, the type of the `goals.json` entry that takes the name, the packages that
  cannot be read, per declaring package the capabilities that make it irreversible, the name's
  place, or the packages that read it as a cap and the member at fault);
  `RootVariableJudgement::unreadable_packages`: the packages of the root that could not be read.
- `VariableTable::root_word(game, name)`, the word that keeps a name from applying on an instance
  of the game now (`root_unchecked` while no check judges its root), and the policy crate's
  `resolve_variable(declaration, stored entry, word)`, the one function that judges a stored value
  against a declaration: the dispatch calls it, so a view that calls it with the instance's entry
  (`stored_v3_entries`) reads what a dispatch would read, the value or the default with the word.

## Reading and setting a value by name

Workflow #463 V3 (Runtime 0.12.61, register §1.383; [runtime-interface.md](runtime-interface.md),
"Goal request kinds"). A person in the console and an agent through the Lab set a variable's value
on one application by its name; the store is the application's `session.resource_targets` v3
record ([resource-targets.md](resource-targets.md), "One value by name").

- **`GoalView`** reads memory only (the owner's ruling): what the scheduler workbench keeps of the
  game root from one check to the next ("What the next slice reads" above) and the remedy table the
  last evaluation built for the active catalog and procedure manifest; it reads no package and no
  file. Until both are there (after a start, before the first check) it answers the declaration
  `not_checked_yet` and lists nothing; fix round 2 of #1731: one word per cause, `root_failing`
  when the workbench checks the root and no check since the start could read it (with the file
  and why, the root to be repaired), `no_schedule` when the game has no standard pack root with
  `schedule.json` (nothing to check). Then it lists the entries of the root's accepted
  `goals.json` in their order and each variable of the packages the active catalog binds on the
  application once (a variable two packages declare once, with both), at its place when
  `goals.json` places it, else after the entries in the order the packages declare them: its
  declaration as `control.json` writes it (`default_origin` `model` when it says so), its default,
  the stored value with who set it and when, and what the next dispatch hands its run, as "The
  run" resolves it over the same projection and the variable table a dispatch reads
  (`effective_json`, `source`, the `variable_problem` word): a value not applied, or a variable
  that does not apply, is `not_applied` with its word and, when the word is the root's judgement's,
  its detail (`variable_cause`: the packages that declare it otherwise, the `goals.json` entry's
  type, the irreversible capabilities, the unreadable packages, the name's place, or the cap's
  readers and member); a value applied in part is `in_effect` with `keys_left_out` and the keys
  left out. Beside the bound packages that declare a variable (`packages`), the root's other
  packages that declare it are named `unbound_packages`. A placement whose variable no bound
  package declares is `not_offered`. The view judges no stored value itself. A stored value no
  entry shows (no tier, no placement, no listed variable: a value under a heading's name
  included) is an orphan. The bound packages' remedy parameters are listed beside them,
  read-only.
- **`EditGoal`** sets or clears one value. Its checks read the record they are about to change,
  so they run under the fact write gate, after the version comparison: a request of another
  version is `fact_observation_version_conflict` whatever its value. A page not in memory yet is
  `not_checked_yet` (send it again after the next check), a root no check since the start could
  read `root_failing`, a game without `schedule.json` `no_schedule` (asking again does not help;
  every name, a resource goal that needs no declaration included); a variable that does not apply is refused
  `variable_not_applying` with its `variable_problem`; a name the root declares that no bound
  package declares (a placement, or a variable of an unbound package) is `not_offered`; a value is
  judged by the dispatch's own function (`resolve_variable`), so `set` takes exactly what a
  dispatch applies whole, and a refusal says the judgement's word as a write (`goal_problem`
  `wrong_type`, `not_an_option` (a key a dispatch would leave out included), `out_of_range`,
  `off_step`, `key_repeated`, `count_out_of_range`), and the value is held to the store's 1024
  bytes (`value_too_large`); a remedy parameter's name is `remedy_parameter` and is never written.
  `clear` removes what is stored under the name whatever its entry is (a value nobody can remove
  must not exist): the next run reads the default.
- **The Lab**: `actinglab goals <inst> [--package <id>]`, `goals get <inst> <name>`, `goals set
  <inst> <name> <json> [--expected-version <n>]`, `goals clear <inst> <name>
  [--expected-version <n>]`, and `package variables --package <dir>`, which reads one package
  directory's declaration through the Runtime's metadata reader without a Runtime; their MCP tools
  `ac_goals`, `ac_goals_get`, `ac_goals_set`, `ac_goals_clear` and `ac_package_variables` answer the
  same lines.

## Interface

`package` revision 14 ([component-interfaces.md](component-interfaces.md)); revision 13 is
Workflow #463 K1's, a `linear_steps` task with a select step. A bundle whose packages declare
variables declares `package` [14, 14] (the contract crate's `VARIABLES_PACKAGE_REVISION`); a
bundle without them keeps its revision, and the Runtime declares [3, 14] ([3, 15] from Workflow
#463 K2 on, the same release: the `package` row 15, a `linear_steps` task's ending pages).

- **An older Runtime** refuses a pack that declares `variables`: every admission path runs the
  declaration gate on `control.json`, which closes its members, so the pack is refused
  `resource_declaration_invalid`, `UnknownField` at `/variables`, before the kernel's own reader
  (which would ignore an unknown member) is reached. Nothing ignores the member silently.
- **An older installer** (its `package` range ending below 14) refuses a bundle that declares
  [14, 14] through its `package` edge (`interface_edge`), in the plan, before the hold and any
  write. **This build's installer** (fix round 1, as `goal_undeclared` holds a bundle that carries
  `goals.json`) refuses a bundle whose packs declare variables (a pack's `control.json` with the
  top-level member `variables`) and that declares a `package` maximum below 14: `pack_invalid`
  caused by `standard_pack_root_invalid` with `variable_undeclared` at `<zip>\interfaces.json`, the
  pointer empty, `package_id` the first such pack, in the plan, before the hold and any write.
  `place` reads only the member's presence; what the variables declare is admission's.
- **`goals.json` version 2** (fix round 3): the revision also brings version 2 of the goal
  declaration, whose placements name these variables; a bundle that carries the file declares
  `package` [14, 14] (`GOALS_PACKAGE_REVISION`), and this build's `place` refuses one that
  declares a maximum below 14 (`goal_undeclared`, by the mechanism of revision 12). The programs
  of 0.12.60 read version 1 only: their Runtime opens no `goals.json` and runs the root as without
  it; their installer (range [3, 12]) refuses a bundle that declares [14, 14] at its `package`
  edge (`interface_edge`); their Lab's goal check reads a version 2 file as `malformed` at
  `/schema_version`, each placement `goal_type_unknown`, so `standard-pack check` warns and
  checks the root as without it, and `package bundle --goals` refuses the bundle
  ([standard-pack-goals.md](standard-pack-goals.md), "Interface").
- **A rollback** below 14 stays open while no root holds a pack that declares variables; a root
  that holds a version 2 `goals.json` runs on 0.12.60 as without the file.
- `ledger`, `runtime-client`, `actingd-config` and `runtime-outcomes` do not move: nothing persisted
  carries a variable in V1, and no request or answer of the workbench changes.
- **A build that declares 14 hands the values to a run** (Workflow #463 V2, the same release,
  register §1.382): the coordinator ships no 0.12.61 without it, so no installation holds a
  Runtime that admits a pack's `variables` and runs it on its defaults alone. V2 moves no
  revision: `variable_values` is an optional value of the existing `policy_dispatch_parameters`,
  `goal_value_not_applied` a new warning in an existing record form, and no request or answer
  changes. Fix round 2 adds no revision either: two warnings in existing record forms
  (`goal_value_package_unreadable`, `standard_pack_goals_not_applied`, with the vocabulary
  `goals_failure`), three optional fields of a `variable_values` record, and an in-process read of
  the workbench's state (`RuntimeHost::standard_pack_goal_roots`), no request. A build of 0.12.60
  reads the new codes as unknown spellings and the new fields as values it keeps in its map.
- **`runtime-client` 11** (Workflow #463 S3-8, S3-9, Runtime 0.12.68,
  [component-interfaces.md](component-interfaces.md), row 11): the request member
  `ContainedTaskRequest.variables`. `min` stays 3: a request without it is the request of revision
  10, and no answer changes. `package`, `ledger`, `actingd-config` and `runtime-outcomes` do not
  move.
- **`ledger` 9** (Workflow #463 CH-1, Runtime 0.12.71, [component-interfaces.md](component-interfaces.md),
  the `ledger` row 9): the admission record's optional `content_hash` and `variables` ("The run's
  admission record"). One way; a build that declares [3, 8] cannot read a ledger that holds one.
  `runtime-client`, `package`, `actingd-config` and `runtime-outcomes` do not move.

## What arrives later

- **V3** (the Lab and the view), built in 0.12.61 ("Reading and setting a value by name",
  `runtime-client` 9): reading, setting and clearing a value per application through the Lab's
  command line and MCP, and the view. Not in it: the values a live run without a dispatch is
  given, which S3-8 and S3-9 add for a direct run in 0.12.68 ("A direct run given values"); no
  run's answer lists them.
- **V4** (the recording): built by #463 S2-6 and its fix round 1 (0.12.68, register §1.442, "What
  is built now, and what is not" above): every select of a recording generates its variable and the
  gate that reads it, and its default is changed after `record stop` by declaring the select again.
  Still to come: editing the options and the texts after `record stop`.
- **V5** (the console) shows and sets the variables on the goal page; **V6** moves the resource
  repositories' numbers and lists out of capability parameters into variables.
