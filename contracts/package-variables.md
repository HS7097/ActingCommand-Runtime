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

- **No one sets or sees a value by name.** The Lab rows, the MCP tools, the requests that set and
  read a value per application and the console's goal page are slices V3 (the Lab and the view)
  and V5 (the console). Since V2 a value is set by writing the application's goal document
  (`targets set` with `entries`, [resource-targets.md](resource-targets.md)), and the next
  dispatch reads it.
- **No live run without a dispatch takes a value.** A direct run and a Lab run read the defaults;
  giving them values needs a member on a request (`ac_run_pack`, `--variable` on a run), which is
  slice V3's, with its `runtime-client` revision. Only the offline scan replay takes
  `--variable`.
- **No recording writes a variable.** Generating a variable from a recording's selection, and
  editing its default after `record stop`, are slice V4.
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
| any | (fix round 1) a stored entry row whose `set_unix_ms` or `channel` cannot be read (the v3 reader holds both to their form, so no record it accepts gives one) | the default, the record naming no setter | `entry_unreadable` |

Fix round 1: no value is refused for its length. The goal store keeps a value of up to 1024 bytes
of canonical text ([resource-targets.md](resource-targets.md), `entries[i].value`), and the record
carries it in a `json` key of the same bound, so every value a run is to read of a set value fits
(`value_too_long` is gone).

**A variable that does not apply** is handed no value from outside: the run reads its default,
whatever the application holds. The Runtime runs the root check (`check_root_variables`, with
`IrreversiblePackages`; the rules stay the contract crate's, `variables_not_applying` gives each
name its word) where it can judge the root:

- **With a composed standard pack** (fix round 1, the coordinator's ruling: the judgement is the
  root check's, over the whole root). The scheduler workbench runs it where it already reads the
  whole root, at each read of a game root (`read_declarations` of
  `crates/runtime-host/src/host/standard_pack_schedule.rs`): over every package of the registry, a
  package no rule runs included, read with the Runtime's metadata reader, with the rule of
  irreversibility built from what every package provides and what its remedies list, against the
  names the root's `goals.json` takes for anything but a placement, read with the contract crate's
  reader (`parse_standard_pack_goals`): an accepted file's names, none for a refused or absent file
  (a refused file is recorded once per sha256, `standard_pack_root_invalid`), and, for a file that
  cannot be read, every name of the root does not apply (`goals_unreadable`; recorded once until
  it reads again, `standard_pack_root_unreadable`). The parsed document, or its refusal, is kept
  with the root's declarations. The workbench reads a root again at a check (at most once per
  `standard_packs.check_interval_ms`) when `schedule.json`, `bundle.json`, `goals.json` or the
  declaration files of a package it registered changed, so a package replaced in place is judged
  with its new declaration after the next read; it keeps each game's judgement in memory and
  builds the table a dispatch reads from memory (no file and no ledger read at a dispatch). A game
  whose root fails at a check keeps its last judgement; its instances have no goals in effect
  meanwhile, so each of its variables reads its default.
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
| (without a composed standard pack: no check judged the instance's game, the table was built before an instance of it was known) | `root_unchecked` |

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

**A known limit** (fix round 1): a package replaced between a dispatch and its run's start may
read the default while the dispatch's record says `set`: the dispatch judged the value under the
declaration the workbench last read, the run under the one it prepares. The record names the
declaration's `declaration_sha256` only in the warning; no record says the run read otherwise.

**The record.** The dispatch's `policy_dispatch_parameters` reason (an existing info outcome)
gains `variable_values`, one record per variable of the package in declaration order:
`{variable_name, variable_source, value_json?, set_unix_ms?, set_channel?, variable_problem?}`.
`variable_source` is `set` or `default`; `value_json` is the value the run reads, as compact JSON,
a key of the type `json` (fix round 1: up to 1024 bytes, the goal store's bound; absent only for a
default longer than that, which the run reads from its package); `set_unix_ms` and `set_channel`
are the entry row's whenever the application holds a value, applied or not, and both can be read;
`variable_problem` says why a set value was not applied, or (fix round 1) `keys_left_out` that an
`ordered` value was applied in part, its source `set`. A pair whose remedy plan
gives it no such reason gets one of these values alone. A package without `variables` gets no
record: its reason chain is byte for byte as before. No persisted shape changes: SS-6 added values
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

**Runs without a dispatch.** A direct run, a run the host starts itself, a Lab run and the
offline replay read every variable at its default and record nothing, as parameters do
(`default_run_values`). The Lab's scan replay (`actinglab replay --select <operation> --scan
<file>`) takes `--variable <name>=<json>`, the counterpart of `--parameter`: the value replaces the
variable's default when the declaration takes it exactly as given; a name the package does not
declare, a value it does not take, or a right side that is no JSON is `validation_failed` naming
`--variable` with `argument_problem` `not_recognised`, a name given twice `repeated`, and
`--variable` without `--select` `conflicting`. No request gives a live run without a dispatch a
value (slice V3).

## Reading and setting a value by name

Workflow #463 V3 (Runtime 0.12.61, register §1.383; [runtime-interface.md](runtime-interface.md),
"Goal request kinds"). A person in the console and an agent through the Lab set a variable's value
on one application by its name; the store is the application's `session.resource_targets` v3
record ([resource-targets.md](resource-targets.md), "One value by name").

- **`GoalView`** lists each variable of the packages the active catalog binds on the application
  once (a variable two packages declare once, with both), at its place when `goals.json` places it,
  else after the entries in the order the packages declare them: its declaration as `control.json`
  writes it (`default_origin` `model` when it says so), its default, the stored value with who set
  it and when, and what the next dispatch hands its run, as "The run" resolves it over the same
  projection and the variable table a dispatch reads (`effective_json`, `source`, the
  `variable_problem` word): a value not applied, or a variable that does not apply, is
  `not_applied` with its word; a value applied in part is `in_effect` with `keys_left_out` and the
  keys left out. The view judges no stored value itself. A stored value no variable offers any more
  is an orphan. The bound packages' remedy parameters are listed beside them, read-only.
- **`EditGoal`** sets or clears one value: a variable that does not apply is refused
  `variable_not_applying` with its `variable_problem`; a value is held to its declaration as a
  write (a `choice` one of the options, an `integer` within the range on the step, a `switch` a
  boolean, an `ordered` list of options each once within the counts; `goal_problem`
  `wrong_type`, `not_an_option`, `out_of_range`, `off_step`, `key_repeated`,
  `count_out_of_range`) and to the store's 1024 bytes (`value_too_large`); a remedy parameter's
  name is `remedy_parameter` and is never written. `clear` removes the row: the next run reads the
  default.
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
bundle without them keeps its revision, and the Runtime declares [3, 14].

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
  changes.

## What arrives later

- **V3** (the Lab and the view), built in 0.12.61 ("Reading and setting a value by name",
  `runtime-client` 9): reading, setting and clearing a value per application through the Lab's
  command line and MCP, and the view. Not in it: the values a live run without a dispatch is
  given, listed in the run's answer.
- **V4** (the recording): a recording's selection generates its variable and the gate that reads
  it; the default, the options and the texts can be edited after `record stop`.
- **V5** (the console) shows and sets the variables on the goal page; **V6** moves the resource
  repositories' numbers and lists out of capability parameters into variables.
