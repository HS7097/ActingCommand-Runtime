# Package variables (`control.json` `variables`)

Workflow #463 L4, slice V1 (Runtime 0.12.60; [v012-register.md](v012-register.md) §1.378). A
package made by recording, or by hand, leaves named slots that are filled from outside: "which
one", "how many", "in which order". A **variable** is such a slot: a value the package's policies
read at run time and do not choose themselves. The owner's requirement (board #463): the value
may come from a person in the console, from an agent through the Lab, or from the package's own
default, which its author or the model's export writes.

This document is what V1 builds: the declaration, the names a game root shares, the root check,
the place on the goal page, the default's origin, and the interface revision.

## What is built now, and what is not

**Built in V1:**

- the member `variables` of a package's `control.json`, read by one reader of the contract crate
  (`parse_package_variables`, `crates/actingcommand-contract/src/package_variables.rs`) that
  kernel admission and the source parser both call, each refusal at its JSON pointer;
- the root check of `actinglab standard-pack check` over the variables of a game root's packages,
  and its provenance listing; since fix round 1 it judges irreversibility over the whole root
  with the goal check's rule (`variable_on_irreversible`) and the listing marks such a variable as
  not applying;
- the `goals.json` entry type `variable`, which places a variable on the page
  ([standard-pack-goals.md](standard-pack-goals.md));
- `package` interface revision 14, and (fix round 1) the installer's `place` refusing a bundle
  whose packs declare variables and that declares less (`variable_undeclared`).

**What does not apply is not enforced at run time yet.** "Does not apply" (a name the root check
reports, "Shared names and the root check") is in this slice a warning and nothing more: the
root check runs in `actinglab standard-pack check` only, and it only warns; `package bundle`, the
installer and the Runtime's host do not run it, and no run reads a variable at all. The slice that
hands values to a run (V2) must enforce it in the Runtime, for a conflicting name and for a
variable of a package that counts as irreversible by the whole root alike: such a variable is
handed no value from outside.

**Not built in V1; nothing here promises it:**

- **No run reads a variable.** Nothing resolves a value, nothing overlays
  `session.dispatch.parameter.<name>` for a variable, and the reason chain of a dispatch records
  no variable. A package that declares variables runs exactly as the same package without them:
  its parameters, its defaults and its run are unchanged. Handing the value to a run, with its
  source, is slice V2.
- **No one sets or sees a value.** The Lab rows, the MCP tools, the requests that set and read a
  value per application and the console's goal page are slices V3 (the Lab and the view) and V5
  (the console).
- **No recording writes a variable.** Generating a variable from a recording's selection, and
  editing its default after `record stop`, are slice V4.
- **No table-backed list** (`from_table`), no `table` type and no value-test operand that reads a
  variable: later slices, each with its own revision.
- **No enforcement of "does not apply" at run time** (fix round 1): V2 enforces it in the Runtime,
  for a conflicting name and for a package irreversible by the root alike (above).

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

Each entry has the members of the value types `goals.json` version 1 had before V1 replaced them
(`integer`, `choice`, `switch`, `ordered`), without `governs` (the governed package is the
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
apply, `standard-pack check` reports each of them `variable_on_irreversible` (fix round 1,
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
application in a later slice.

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
| `variable_on_irreversible` (fix round 1) | a package that declares it counts as able to do something irreversible by the whole root: it provides, unmarked, a capability another package of the root marks irreversible, or a remedy of its mappings lists one (the contract crate's `IrreversiblePackages`, the one copy of the rule the goal check holds a placement to; admission refuses `variables` on a package whose own `provides` marks the capability) | `/variables/<i>` of each such package's declaration |
| `variable_cap_range` | a package that declares it reads a designated effect's cap from `session.dispatch.parameter.<name>` (`max_repeats: {fact_key}`, which takes a whole number from 0 to 1000), and the variable is of another type than `integer` (fix round 1: a cap is read from an `integer` variable only), or an `integer` whose `min` is below 0 (fix round 1) or whose `max` is above 1000, the first of these | `/variables/<i>/type`, `/variables/<i>/min` or `/variables/<i>/max` of each such package's declaration |

Each problem is one `standard_pack_root_invalid` warning with `game`, `path` (the package's
`control.json`), `standard_pack_problem` and `json_pointer`, after the composition's warnings and
before those of `goals.json`. **The root still runs**, and no variable of that name applies (as a
refused `goals.json` applies no entry): in this slice that is what the warning says, and nothing
enforces it at run time. The check warns only; `package bundle`, the installer and the Runtime's
host do not run the root check, and no run reads a variable. The slice that hands values to a
run (V2) must enforce it in the Runtime, for a conflicting name and for a package irreversible by
the root alike ("What is built now, and what is not"). A `goals.json` the reader refuses applies
no entry, so it takes no name. The root check's four words, like V1's two others
(`goal_variable_unknown` of the placement and, fix round 1, `variable_undeclared` of the
installer), are words of the existing vocabulary `standard_pack_problem` under the existing code
`standard_pack_root_invalid`.

**The provenance listing.** The answer of `standard-pack check` gains, after `models`,
`variable_provenance` `{pack: [names], model: [names]}` when a package of the root declares
variables: each variable that applies, once, in the root's order, by who wrote its default
(`pack` without `default_origin`, `model` with it), as the schedule's provenance lists its fixed
values ([standard-pack-schedule.md](standard-pack-schedule.md), "Provenance"). Since fix round 1
the listing marks as not applying each variable that a package counting as irreversible by the
whole root declares, which would otherwise read as in effect: `not_applying`
`[{name, standard_pack_problem: "variable_on_irreversible"}]`, once each in the root's order,
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
([standard-pack-goals.md](standard-pack-goals.md), "The five types of version 1"). It writes no
text and no value: the variable carries its own. Its `block`, when written, is the placed
variable's base block (`goal_block_mismatch` otherwise). A name no package of the root declares is
`goal_variable_unknown`; a placement is held to every package that declares the variable as a
governed package was (`goal_governs_irreversible`, `goal_package_unknown`). The type replaces the
four types that governed packages, in version 1 of the file (the tag `v0.12.60` did not exist when
V1 opened its pull request, and no pack carries the file).

## Interface

`package` revision 14 ([component-interfaces.md](component-interfaces.md)); revision 13 is
reserved by Workflow #463 K1. A bundle whose packages declare variables declares `package`
[14, 14] (the contract crate's `VARIABLES_PACKAGE_REVISION`); a bundle without them keeps its
revision, and the Runtime declares [3, 14].

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
- **A rollback** below 14 stays open while no root holds a pack that declares variables.
- `ledger`, `runtime-client`, `actingd-config` and `runtime-outcomes` do not move: nothing persisted
  carries a variable in V1, and no request or answer of the workbench changes.

## What arrives later

- **V2** (the run): a variable's value is overlaid as `session.dispatch.parameter.<name>`, the key
  remedy parameters use, with its kind's type (a string, a boolean, an integer, or a record list of
  rows `{key, rank}`); the value comes from the application's set value or the package default, and
  the dispatch's reason records each value with its source. A run without a dispatch takes the
  defaults or the values given to it. A set value that no longer fits the declaration runs on the
  default and is marked. V2 also enforces in the Runtime what the root check only reports here:
  a variable that does not apply, for a conflicting name or a package that counts as irreversible
  by the whole root alike, is handed no value from outside (fix round 1).
- **V3** (the Lab and the view): reading, setting and clearing a value per application through the
  Lab's command line and MCP, and the view that lists every variable with its value and source.
- **V4** (the recording): a recording's selection generates its variable and the gate that reads
  it; the default, the options and the texts can be edited after `record stop`.
- **V5** (the console) shows and sets the variables on the goal page; **V6** moves the resource
  repositories' numbers and lists out of capability parameters into variables.
