# Standard-pack package declaration (`pack-catalog.json`)

Workflow #477, slice PC-1 (Runtime 0.12.68; [v012-register.md](v012-register.md) §1.438). The owner
approved on 11-16 (board #477) that each game's standard-pack root gets **one declaration per
package**: what the package is to a person (its display), what it consumes and what it produces,
later its value; and that the scheduler will read it as an index ("which packages produce this
resource, which consume it"). Before it, the same things stood in five places and no rule
declared a production, so a resource goal reached nothing. This document is that declaration:
where it lives, its two parts, what is an error and what a warning, the index, who reads it, who
writes it, and what the later slices add.

**This slice is safe whatever is later decided about scoring.** The Runtime does not read the
file; nothing is scored, dispatched or composed differently. Only the Lab's tools and, through
the Lab's `packs` answer, the console see it.

## The owner's six answers

Each choice below follows one of the owner's answers for this slice (board #477, her ruling of
11-16); the rest is the coordinator's ruling under her words "the rest is approved".

1. **The file replaces `ui.json` outright** (her rule of 11-15: nothing is kept compatible with
   what came before). Hence: no program reads `ui.json` any more, nothing reads both, and a root
   that still holds one is told so ("`ui.json` is retired").
2. **A package's consumption stands in two places in this slice**: the rules of `schedule.json`
   keep theirs untouched, the file's is for the index only. Hence: the tools compare the two and
   list every difference ("Differences from `schedule.json`"), and the writer never overwrites an
   entry silently.
3. **No value in this slice**: only what is produced and what is consumed. Hence: there is no value
   member, and the reader refuses one ("Strictness").
4. **The scheduler (the Runtime) does not read the file in this slice.** Hence: no Runtime
   component opens it, and a change of the file triggers no composition. (Fix round 1: a zip that
   carries it declares `package` 16, the installer's boundary, not the Runtime's: "Interface".)
5. **The file is written by tools, not edited by hand.** Hence: `actinglab standard-pack catalog`
   ("Who writes it"); the adoption of a new package and the model's writer write entries in later
   slices.
6. **A defect in the display part is a warning and everything still runs; a defect in the business
   part makes the root's declaration wrong.** Hence the two lists of "What is wrong".

## The file

`pack-catalog.json` is an optional root file of the game root, by the root-file rule
([package-reference.md](package-reference.md), "Standard package root", "Naming"):
`<standard_packs.root>/<game>/pack-catalog.json` beside `bundle.json` in an installation, and in a
resource repository the file at its checkout's root, beside `applications.json` and `packs/`
(fix round 1: a checkout holds no `bundle.json`, which the build makes of those two, and the build
carries the file from there into the zip's root). `schema_version` is
`actingcommand.pack-catalog.v1` and `game` the root's game: the directory's name in an
installation, the `game` of `applications.json` (the zip's `bundle.json`'s) for a checkout.

```text
{schema_version: "actingcommand.pack-catalog.v1", game,
 groups?: [{id, title: {<locale>: text}}],
 display?: {aspect_ratios: ["<w>:<h>"], safe_area?: word},
 packs: [{package_id,
          title?: {<locale>: text}, summary?: {<locale>: text}, group?: word, risk?: [word],
          consumes: [{pool_id, quantity?}],
          produces: [{pool_id, quantity?}]}]}

quantity:  {"kind": "constant", "amount": N}     N units per run, 1 to 2^53 - 1
         | {"kind": "pending"}                   the amount is not known yet
(absent)                                         the relation is known, the amount is not stated
```

The smallest file:

```json
{"schema_version": "actingcommand.pack-catalog.v1", "game": "neutral",
 "packs": [{"package_id": "neutral.daily", "consumes": [],
            "produces": [{"pool_id": "pool.points", "quantity": {"kind": "constant", "amount": 2}}]}]}
```

One entry per `package_id`; every list is a list, not a map, so the order is kept and a
duplicate has a pointer. The writer writes the document two-space indented with a line feed at
the end.

### The display part

`groups`, `display` and, per entry, `title`, `summary`, `group` and `risk`: what `ui.json` carried
(Workflow #429 R-11), with the same meaning and bounds, but `parameters` (a package's variables
carry their own texts since Workflow #463 V1, [package-variables.md](package-variables.md), so it
named nothing). Every text, locale, group id, risk word and the safe-area word is opaque: compared
only for equality and measured only in bytes. An aspect ratio is only checked to be `<w>:<h>` (1
to 5 decimal digits per side, no leading zero); nothing computes with it. The risk words decide
nothing: a client's risk badge is the Runtime's own `irreversible` plus these words.

An entry that writes none of the four display members has no display yet: the package shows by
its `package_id`, without a warning. An entry that writes one writes `title` and `summary`.

### The business part

Per entry, `consumes` and `produces` (both required, each possibly empty): each item a `pool_id`,
a pool that the root's `schedule.json` declares (its `pools[].id`), and an optional `quantity` of
the two forms above. A pool stands once on one side of an entry. The business part says what one
run of the package uses up and brings in; it is the input of the index and of the comparison with
`schedule.json`, and nothing else reads it in this slice.

### Strictness

A member the reader does not know at the document's own level, on an entry, on a pool or on a
quantity is an error at its JSON pointer: so a later slice adds the value under its own schema
step (a `value` member there is refused today), and a misspelt member is never read as absent.
Fix round 1 (the owner's answer 6): a member a group or `display` does not know is a display
defect, a warning, and that group or that `display` is left out ("Warnings: a display defect").

## What is wrong

### Errors: the root's declaration is wrong

Each an issue with its `standard_pack_problem` at its JSON pointer (empty for the whole
document), at most 64 in the reader's order:

| Problem | When |
|---|---|
| `malformed` | the file is no JSON or no object; a member repeated in one object; a member the reader does not know at the document's own level, on an entry, on a pool or on a quantity; a document or business member missing or not of its type; a `schema_version` other than `actingcommand.pack-catalog.v1`; a quantity that is not exactly one of its two forms (`kind` another word, `amount` missing, 0, above 2^53 - 1 or beside `pending`) |
| `game_mismatch` | `game` is not the directory's |
| `package_not_listed` | a `package_id` the root's `bundle.json` does not register |
| `catalog_duplicate` | a `package_id` listed twice, or a pool twice on one side of an entry (at the later one) |
| `catalog_pool_undeclared` | a `pool_id` the root's `schedule.json` does not declare (any pool, for a root without `schedule.json`) |
| `catalog_out_of_limits` | the file is larger than 1 MiB, `packs` holds more than 1024 entries, a side more than 256 pools, a `pool_id` is empty or longer than 128 bytes |

Where a tool reports one, it is `standard_pack_root_invalid` (`game`, `path` the file,
`standard_pack_problem`, `json_pointer`). A file that exists and cannot be read is the same kind of
failure, with the OS error as its cause.

### Warnings: a display defect

The file reads and every package runs; the defective group, the `display` block or the package's
display is left out, so that package shows by its `package_id`, as R-11 fell back. Each is
`standard_pack_catalog_display_invalid` (`game`, `path`, `standard_pack_problem`, `json_pointer`):

| Problem | When |
|---|---|
| `malformed` | a display member not of its type (a text that is no object, a word that is no string), (fix round 1) a member a group or `display` does not know, an aspect ratio not `<w>:<h>`, `display` without `aspect_ratios`, a group without `id` |
| `catalog_text_missing` | a group without `title`, or an entry that writes a display member without `title` or `summary` |
| `catalog_group_unknown` | an entry's `group` is no group's `id` |
| `catalog_duplicate` | a group id, a risk word of one entry or an aspect ratio written twice |
| `catalog_out_of_limits` | a list, a text, a locale or a word outside its limit ("Limits") |

### Neither: coverage

A package `bundle.json` registers and the file lists no entry for is neither an error nor a
warning: roots are filled over time. It runs as before and shows by its id. `standard-pack check`
counts and lists them.

### Limits

Bounds of the shape, not tunables:

| What | Limit |
|---|---|
| the file | at most 1 MiB |
| `groups` | 0 to 64 entries |
| `packs` | 0 to 1024 entries |
| a text (`title`, `summary`) | 1 to 16 locales, each locale name 1 to 35 bytes |
| `title` | 1 to 128 bytes per locale |
| `summary` | 1 to 1024 bytes per locale |
| a word: a group's `id`, an entry's `group`, a risk word, `safe_area` | 1 to 64 bytes |
| `risk` | 0 to 8 words |
| `aspect_ratios` | 1 to 8 entries |
| `consumes`, `produces` | 0 to 256 pools each |
| `pool_id` | 1 to 128 bytes |
| a constant `amount` | 1 to 2^53 - 1 |

## The index both ways

Computed by the reader from the business part, never stored: for the root, `producing` (pool → the
packages that produce it) and `consuming` (pool → the packages that consume it). A package appears
once per pool, however many rules name it; an item whose quantity is pending or absent is in the
index (the relation is what the index holds); producers and consumers are kept apart. Pools are in
byte order, each pool's packages in the order of the file's entries. A package without an entry
is in no list.

## Differences from `schedule.json`

By the owner's answer 2 a package's consumption and production stand in both files in this slice,
so the tools compare them, entry by entry in the file's order, `consumes` before `produces`. Each
difference is the warning `standard_pack_catalog_schedule_differs` (`game`, `package_id`, `pool_id`,
`catalog_side` `consumes` or `produces`, `catalog_difference`):

- `catalog_lacks`: a rule of the package in `schedule.json` names the pool on that side and the
  package's entry does not list it; `task_id` names the rule, once per rule and pool;
- `schedule_lacks`: the entry lists the pool on that side and no rule of the package names it
  there; no `task_id`.

A package without an entry is not compared. Nothing is refused for a difference: the rules run as
before, and the file's business part feeds only the index.

## Who reads it

None of the Runtime's components: its root reader, its startup and run preparation, the policy
crate, the scheduler workbench and check-config never open it, so the file never changes a run,
and a directory that holds only `pack-catalog.json` is no game root for them (for the readers below
it is `bundle_missing`).

- **`actingctl packs` and `ac_packs`** (offline, [runtime-interface.md](runtime-interface.md),
  `packs`): each sound game root of `standard_packs` carries the display part as a client reads it
  (`ui`, `{groups, packs}`: the groups and the package displays that keep their rules, as written),
  its `display`, and the `index`; each composed installation row carries its package's display
  (`ui`), its game's `display` and its business part (`catalog`, `{consumes, produces}`). A root
  without the file answers as before. A file wrong in its business part adds one
  `standard_pack_root_invalid` warning per error, and the root and its rows are listed as without
  the file; a display defect is its warning and that package shows by its id.
- **`actinglab standard-pack check`**: the answer's last member `catalog` is a note,
  `standard_pack_catalog_absent` (`game`, `path`) for a root without the file, else
  `standard_pack_catalog_coverage` (`game`, `path`, `count`) beside `unlisted`, the packages without
  an entry in the registry's order. A file unreadable or wrong in its business part refuses the
  check (`standard_pack_root_invalid` with its first error), so the root is not ready and whatever
  refuses a root that is not ready refuses it (`standard-pack ready`, the build, the installer's
  post-check). Each display defect and each difference is a warning; the file is listed among the
  root files. The composition is the same with and without the file.
- **`actinglab standard-pack build`** carries a commit's file into the zip byte for byte, checked
  against the zip's `bundle.json` and the repository's `schedule.json`: an error of its business
  part fails the `bundle` step, and nothing is built ([resource-verify.md](resource-verify.md)).
  `build.json` lists it among the root files. Fix round 1: a zip that carries it declares
  `package` [16, 16] ("Interface").
- **The installer's `place`** ([install-cli.md](install-cli.md)): the file is the last root file of
  the layout, placed with its SHA-256 as its item's digest, and set aside as
  `pack-catalog.json.old-<ms>` when a later zip carries none; a zip whose file is wrong in its
  business part is refused (`pack_invalid` caused by `standard_pack_root_invalid`) in the plan,
  before the hold.

## Who writes it

`actinglab standard-pack catalog --root <root> [--plan]` (the Lab's MCP tool
`ac_standard_pack_catalog`, `root` and `plan`, through a `serve` child), so that nobody edits the
file by hand. Fix round 1: `--root` takes either of two directories. Workflow #477 PC-2 (0.12.71;
[v012-register.md](v012-register.md) §1.462): `[--refresh <package_id>]... [--displays <file>]`
(the tool's `refresh`, a list, and `displays`), below.

- **A resource repository's checkout**, the way to write a repository's file: a directory that holds
  `applications.json` and no `bundle.json`. Its game is the `game` of `applications.json`; its
  registry is the `bundle.json` the build's own function, `package_bundle`, makes of
  `applications.json` and the directories of `packs/` (a file there, such as a `README.md`, is no
  package and is passed over, as the build passes it over), made in a scratch directory of the
  system's temporary folder (the packages copied there, following links as the build does) and
  removed before the line answers. The file is written at the checkout's root, where the build
  carries it from; the directory's own name does not matter. Commit it with the checkout.
- **A built game root** outside an installation: a directory that holds `bundle.json`, named as its
  game, whose registry is that file's (the form of the first round).

A directory that holds neither is `standard_pack_root_invalid`, `bundle_missing`, at `bundle.json`.
It builds or brings up to date the file from what the root already declares, by one rule
(`merge_standard_pack_catalog` of the contract crate):

- An entry for every package of the registry: the entries already in the file first, in its order,
  each **kept as written**, display and business part; then one new entry per package without one,
  in the registry's order.
- A new entry's display is the leftover `ui.json`'s entry for the package (`title`, `summary`,
  `group`, `risk` as written), else none; its `consumes` and `produces` are the union over the
  package's rules in `schedule.json`, each pool once in the order the rules first name it, with a
  constant quantity when every rule that names it states the same whole amount, else `pending`.
- `groups` and `display` are the file's when it writes them, else the leftover `ui.json`'s, else
  absent.
- **An entry already there is never overwritten** unless the line names it (PC-2, the next two
  points): its differences from the rules are answered
  (`differences`) and are warnings. What no longer holds is left out and answered: the entry of a
  package the registry no longer holds (`removed`) and a pool of a kept entry `schedule.json` no
  longer declares (`dropped`); fix round 1: each is also the warning
  `standard_pack_catalog_left_out` (`game`, `path`, `standard_pack_problem` `package_not_listed`
  or `catalog_pool_undeclared`, `json_pointer` in the existing file, `package_id`, and `pool_id`
  for a pool), in the existing file's order, after the display defects and before the
  differences. A file wrong in anything else is not brought up to date: the line is refused with
  its first error. Fix round 1: so is a rule of a package that is to get a new entry when it names
  a pool `schedule.json` does not declare: `standard_pack_root_invalid` at `schedule.json`, the
  file the pool is missing from, `catalog_pool_undeclared` at the rule's
  `/rules/<i>/<side>/<j>/pool_id`.
- **`--refresh <package_id>`** (PC-2; repeatable, once per package) is **the one way an entry's
  business part changes**. The entry of each package it names has its `consumes` and `produces`
  computed again from the package's rules in `schedule.json`, by the rule a new entry is made by
  (the same function), and they replace what the entry held; its display and its place in the file
  are kept. An entry that holds its rules' already is left as written. A named package without an
  entry is not refused: it gets its new entry in the same call, which is its rules' by the rule
  above (refusing it would only send the agent to a second line that gives the same entry). Its
  rules' pools are held to `schedule.json` as a new entry's are (`catalog_pool_undeclared` at the
  rule). The answer's `refreshed` gives each named package in the order named, `{package_id,
  state, before?, after}`: `state` `replaced`, `agrees` (no failure) or `added`, `before` and
  `after` each `{consumes, produces}` as the file writes them (`before` absent for `added`), so
  that a reader sees the change without comparing files; afterwards no
  `standard_pack_catalog_schedule_differs` names the package. Refused before anything is written,
  `standard_pack_catalog_refresh_refused` (`game`, `package_id`, `catalog_refresh_problem`): a
  package the registry does not hold (`package_unregistered`) or one named twice
  (`package_repeated`), the first in the order named. An entry not named is never overwritten.
- **`--displays <file>`** (PC-2; an absolute path) gives a display file,
  `{"packs": [{"package_id", "title", "summary", "group"?, "risk"?}]}`, each entry in the catalog's
  own shape without its business part. Each entry it names has its display replaced whole by the
  file's: the entry is written as its `package_id`, the display members the file gives in the order
  a new entry writes them, then its business part as it stands; an entry of the file that writes no
  display member leaves the package without a display (it shows by its id). Its business part and
  its place are untouched, and a new entry it names takes no display from `ui.json`. The file is
  read whole and checked before anything is written by the catalog's own checks: its size (1 MiB)
  and `packs` (1024 entries), `catalog_out_of_limits`; no JSON object, a member repeated, a member
  this shape does not know (a business member included) or a `package_id` that is no string,
  `malformed`; a package the registry does not hold, `package_not_listed`; one named twice,
  `catalog_duplicate` at the later one; and each entry's display by the reader's own display check
  ("Warnings: a display defect", the same words and limits) against the groups of the file being
  written, every display defect an error here. Any error refuses the line,
  `standard_pack_catalog_display_refused` (`game`, `path` the display file,
  `standard_pack_problem`, `json_pointer` in it), the first of its shape and packages before its
  display defects. **The option adds no group**: a `group` the file being written does not declare
  is `catalog_group_unknown` (whether a later tool may add a group is not settled). The answer's
  `display_written` names the packages whose display the file replaced, in its order. A package
  without an entry before the call has one after it, so "a package without an entry" is the same
  as one the registry does not hold.
- Both answer members are present only with their option: without the two, the line answers and
  writes as before, byte for byte. A second line with the same options changes nothing
  (`unchanged`; each refreshed package `agrees`). Both refusals are of the class usage or
  validation: exit status 1 on actinglab's own line and through `serve`, 2 on a `--via` line.
- With `--plan` it writes nothing and answers the `document` it would write (`status` `planned`).
  Without it, it writes the file atomically (a `.part` beside it, synced and renamed over it) when
  its bytes differ (`written`), and leaves it as it is when they do not (`unchanged`); a second run
  changes nothing. The answer also names `path`, `changed`, `added`, `kept` and `display_from_ui`.
- A root inside an installation is refused, `standard_pack_catalog_root_refused` (`path`,
  `install_root`), before anything is read: an installed root takes the file through the build and
  `place`. A failed write is `standard_pack_catalog_write_failed` (`path`, the OS error its cause);
  the file in place is then as it was. Fix round 1: it is of the class device or instance (exit
  status 4 on the command line), as the Lab's other local write failures, not a usage refusal; so
  is an OS failure of a checkout's `packs/` or of the scratch directory (`foreign_os_error`).
- Of the root it reads `bundle.json` (of a checkout, `applications.json` and `packs/` instead),
  `schedule.json`, `pack-catalog.json` and `ui.json` only (PC-2: and the display file `--displays`
  names, wherever it is); in the root it writes only
  `pack-catalog.json` and deletes nothing: a leftover `ui.json` stays, with its warning. Under the
  global `--dry-run` the line without `--plan` is `dry_run_unsupported`
  ([actinglab-dry-run.md](actinglab-dry-run.md)).

## `ui.json` is retired

`ui.json` (Workflow #429 R-11, 0.12.36 to 0.12.67) is no longer read by anything. A root that still
holds one gets the warning `standard_pack_ui_retired` (`game`, `path`): its content belongs in the
display part of `pack-catalog.json`, which `standard-pack catalog` takes from it. `packs`,
`standard-pack check`, `standard-pack catalog` and the build give it; the build does not carry the
file; `place` sets an installed one aside, and a zip that still carries one is refused as a member
no layout knows. The four words of its reader (`ui_out_of_limits`, `ui_duplicate`,
`ui_group_unknown`, `ui_parameter_undeclared`) went with it.

## Interface

**`package` 16** (fix round 1; [component-interfaces.md](component-interfaces.md), the `package` row
16): the Runtime reads no member of the file, but an installer of 0.12.67 or earlier knows no root
file of that name and would refuse a zip that carries it as a member no layout knows
(`pack_invalid`), not at the interface's boundary. So a zip that carries `pack-catalog.json`
declares `package` [16, 16] (the contract crate's `CATALOG_PACKAGE_REVISION`; `standard-pack build`
counts it by the file's presence at the zip's root), this build declares [3, 16], and an earlier
installer refuses such a zip at its `package` edge (`interface_edge`), in the plan, before the
hold. This build's `place` does not refuse a zip that carries the file and declares less: no such
zip is built (the build that writes the file declares 16), and the boundary that matters is the
earlier installer's.

The `packs` answer is the Lab's offline read, which no revision covers. Its new members (`index` on
a game, `catalog` on a row) are added beside the old ones; a client that reads the answer by member
name (the console) ignores them and keeps reading `ui` and `display` as before. A game's `ui` is
now `{groups, packs}` (R-11 gave the whole `ui.json`, whose `groups` and `packs` it was): the
members a client read are where they were.

## What the later slices add

- **The value**: a value per package or per pool, under its own schema step (the reader refuses
  a value member until then).
- **The scheduler's reading**: the Runtime reads the index to find which packages produce what a
  resource goal lacks, and the business part replaces the consumption the rules carry today.
- **The adoption's entries**: `standard-pack adopt` writes the entry of the package it adopts.
- **The model writer's entries**: the model's export writes the quantities and, later, the values.
