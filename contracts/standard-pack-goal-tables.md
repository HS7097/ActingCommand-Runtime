# Standard-pack tables of goals (`goal-tables.json`)

Workflow #455, slice RR1 and its fix round 1 (Runtime 0.12.92; [v012-register.md](v012-register.md)
§1.535). A person
says which character to raise and to which grade; nobody types how many of each material that
takes. What it takes is the game's rule, the same for every account: this file. It is an optional
root file of the game root, beside `goals.json`, whose `records` entries name its tables
([standard-pack-goals.md](standard-pack-goals.md), "The records entry"). It declares what each grade
costs and how the costs come apart; it never holds what an account has or wants.

This slice gives the file its reader, its checks, its place in a bundle and in an installation,
and the Runtime's reading of it. Since Workflow #455 RR3 (0.12.95, §1.538) the Runtime computes
from it, before every evaluation, what the recorded characters still need ("What is computed").

The core stays game-neutral: every name of the file is an opaque string, compared for equality
and measured in bytes.

## The file

`<standard_packs.root>/<game>/goal-tables.json`, `schema_version`
`actingcommand.goal-tables.v1`, `game` equal to the directory's name. The document has exactly the
members `schema_version`, `game`, `items` and `tables`.

```text
{schema_version: "actingcommand.goal-tables.v1", game,
 items: {<item>: {pool?, recipe?: {makes, from: {<item>: amount}, cost?: {<item>: amount}}}},
 tables: [{name,
           progress: [{fact_key, key_column, held_by?, columns: [<reading column>]}],
           columns: [{name, per: "class" | "key", read: [<reading column>], requires?}
                   | {name, per: "sub", subs: {<sub>: [<reading column>]}, requires?}],
           classes: {<class>: {<column of per class>: ladder}},
           keys: {<key>: {class, own?: {<role>: <item>}, <column>: <what the key writes>}}}]}

requires: [{column, at_least: position, from?: position} | {condition}]
ladder:   {positions: [position], steps: [step]}         one step fewer than positions
position: [integer]                                       one for each reading column read
step:     {<item> | "@<role>": amount}
```

### Names

Fix round 1 states the forms the reader holds every name to (a name of a length outside its bound
is `table_out_of_limits`, one outside its form `malformed`):

- a table's, a column's, a class's, a sub's, a role's (in a key's `own` and after the `@` of a
  step's member) and a condition's name: an entry name's form, `[a-z0-9][a-z0-9_]*`, 1 to 64
  bytes, as `goals.json`'s entry names ([standard-pack-goals.md](standard-pack-goals.md)); no
  column is named `class` or `own`;
- an item's and a key's name: 1 to 128 bytes without a control character, an item's never
  beginning with `@`; a pool id: 1 to 128 bytes without a control character;
- a progress list's `key_column` and every reading column (of a list's `columns`, a column's
  `read`, a sub): a list reading's column form, `[a-z][a-z0-9_]*`, 1 to 32 bytes
  ([resource-readings.md](resource-readings.md));
- an override's index: a decimal whole number without a leading zero.

### Items

Each item has a name (1 to 128 bytes without a control character, never beginning with `@`), and
two optional members:

- **`pool`**: the pool of the root's `schedule.json` that holds it (an id, 1 to 128 bytes). An item
  without a recipe and without a pool is shown and weighs nothing. A pool is named by one item at
  most (fix round 1: a holding has one name in the file, and what the computation subtracts of a
  pool's holding it subtracts once; a later item that names it is `table_pool_duplicate`).
- **`recipe`**: how the item is made: one making turns out `makes` of it from the items of `from`
  (1 to 16) and costs the items of `cost` (0 to 16), each with its amount. **An item with a recipe
  is taken apart; one without is where taking apart stops.** An item with a recipe names a pool:
  its holding is what the taking apart subtracts, so without one its need could never be known.

An item carries no tier and no mark of how it is obtained: the order of taking apart is the recipes'
own, and where a need lands is the item without a recipe.

### Tables

A table has a name of an entry name's form (`[a-z0-9][a-z0-9_]*`, 1 to 64 bytes), unique in the
file; a `records` entry of `goals.json` names it.

- **`progress`**: 1 to 4 lists the table's progress is read from, each a list reading's `fact_key`
  (under `inventory.list.`, [resource-readings.md](resource-readings.md)), the `key_column` whose
  value is a key of the table, and the reading `columns` it gives (1 to 63, of a list reading's
  column form, each listed once among the table's lists). At most one list writes `held_by: true`:
  read whole, a list that holds no row of a key says the key is not held. Only the key's form is
  held: the packages that write the list may come later. Fix round 1: the lists of one table name
  distinct fact keys (a later list that names an earlier one's is `table_list_key_duplicate` at
  its `fact_key`), and a list's `key_column` is none of its own `columns` (the list would name
  that column twice: `table_reading_column_duplicate` at it among the `columns`).
- **`columns`**: 1 to 8, each a name unique in the table (never `class` or `own`, a key's own
  members) and its `per`: `class` (its ladders are written in the classes; a key may override a
  step), `key` (written in each key) or `sub` (written in each key, one ladder for each of its
  `subs`, 1 to 8, each with its own reading columns). A column of `class` or `key` reads 1 to 4
  reading columns (`read`), each one the table's lists give and named once; its positions are of
  that width. A sub likewise.
- **`requires`** (0 to 4): the column's prerequisites, each `{column, at_least, from?}` or
  `{condition}` ("Prerequisites", below). `{condition}` names a condition that is no column's (a
  word of the file's), marked and not counted.
- **`classes`**: 1 to 64 by name, each with its ladders of the table's columns of `per: "class"`.
- **`keys`**: 1 to 1024 by name (1 to 128 bytes without a control character; a key is the value of
  a list's `key_column`), each with its `class`, its `own` items by role (0 to 8 roles, each naming
  an item the file declares: a step's `@<role>` is that item for this key), and, for each column
  it writes: of `per: "class"`, `{steps: {<index>: step}}`, the steps of its class's ladder it uses
  instead (a decimal index without a leading zero); of `per: "key"`, its ladder; of `per: "sub"`,
  `{<sub>: ladder}` for the column's subs it has.

### Ladders and steps

A ladder has 2 to 512 positions, each distinct and of its column's width (integers within the safe
integers of JSON), and one step fewer: the step `i` is what going from position `i` to position
`i + 1` uses, and it *arrives at* position `i + 1`. A step uses 0 to 16 items or roles, each with
an amount from 1 within the safe integers. A position on a ladder is a reading tuple compared
element by element, for equality, with what the lists give.

**What a table reads** (fix round 1). A position is a tuple of whole numbers, one for each reading
column its column reads, in their order, and a column's `read` (and a sub's reading columns) names
list columns that hold whole numbers. The reader holds the file's own positions to that (a number
with a fraction, a text or a truth value in a position is `malformed`); what a list's column holds
is declared by the packages that write the list, which the reader does not see. A reading of
another kind never equals a position: the computation then takes the position as not known
(`goal_records_position_unknown`, "What is computed").

### Prerequisites

Fix round 1 states what a prerequisite `{column, at_least, from?}` of a column, the *requiring*
column, means; the checks hold its positions to it ("The checks").

- **Which column.** `column` names another column of the same table, of `per: "class"` or
  `per: "key"`. A column of `per: "sub"` has no one ladder and is refused
  (`table_requires_invalid` at the prerequisite's `column`); a name that is no column of the
  table is `table_column_unknown` there; a column that names itself reaches itself
  (`table_requires_cycle`).
- **Which ladder.** `at_least` is a position of the ladder the key walks in the required column:
  its class's ladder of a column of `per: "class"` (an override changes no position), its own of
  a column of `per: "key"`. `from` is a position of the requiring column's own ladder, its class's
  or its own; for a requiring column of `per: "sub"`, of the ladder of each of the key's subs, and
  the prerequisite then speaks of each sub's steps.
- **Order.** A position is *at or beyond* another when it is that position or stands after it in
  the `positions` list of the ladder concerned. It is the ladder's order: the numbers of two
  positions are never compared.
- **What is asked.** Without `from`: before any step of the requiring column is taken, the
  required column stands at `at_least` or beyond. With `from`: before a step that arrives at
  `from` or beyond is taken; a step that arrives at a position before `from` asks nothing.
- **Who is held to it.** The classes and keys that have steps in the requiring column: each key
  that has, and each such class of a required column of `per: "class"`, holds `at_least` on its
  ladder of the required column, and each of their ladders of the requiring column holds `from`
  (`table_requires_invalid` at that member otherwise). A class without a ladder of the requiring
  column, and a key without steps in it, are not held to it: nothing is asked of them.

*A worked example*, on the first example below. The column `mastery` (per sub) requires `phase`
at least `[2]` and `skill` at least `[4]`, both without `from`. The key `hero_a` walks its class
`rank_6`'s phase ladder `[0], [1], [2]` and its own skill ladder `[1], [2], [3], [4]`: no step of
either of its mastery subs (from `[0]` to `[1]`, or from `[1]` to `[2]`) is taken before phase
stands at `[2]` and skill at `[4]` (each the last position of its ladder, so only that position
meets it). `hero_b` has no mastery ladder and takes no step of the column: nothing is asked of it.
Were the first prerequisite `{"column": "phase", "at_least": [2], "from": [2]}`, a sub's step from
`[0]` to `[1]` would ask nothing of phase, and its step from `[1]` to `[2]`, which arrives at
`[2]`, would ask phase at `[2]`; skill, without `from`, would still be asked before the first
step. And the order is the list's: on the class's level ladder `[0, 1], [0, 2], [1, 1], [1, 2],
[2, 1], [2, 2]`, a prerequisite of level at least `[0, 2]` is met at `[0, 2]` and at the four
positions written after it, `[1, 1]` among them though its second number is the smaller.

## Limits

| What | Limit |
|---|---|
| the file | at most 4 MiB (4,194,304 bytes) |
| `items` | 1 to 1024 |
| an item's or a key's name, a pool id | 1 to 128 bytes |
| a recipe's `from` / `cost` | 1 to 16 / 0 to 16 items |
| `tables` | 1 to 16 |
| a table's, a column's, a class's, a sub's, a role's or a condition's name | 1 to 64 bytes |
| `progress` | 1 to 4 lists, each 1 to 63 columns of 1 to 32 bytes |
| `columns` | 1 to 8; `read` and a sub 1 to 4 reading columns; `subs` 1 to 8; `requires` 0 to 4 |
| `classes` / `keys` | 1 to 64 / 1 to 1024 |
| a key's `own` | 0 to 8 roles |
| a ladder's positions | 2 to 512 |
| a step | 0 to 16 uses |
| an amount, `makes` | 1 to 2^53 - 1 |

## The checks

Two pure functions of the contract crate (`crates/actingcommand-contract/src/standard_pack_goal_tables.rs`),
called by every reader, never copied. The reader, `parse_standard_pack_goal_tables`, reads the file
alone; then, for a file the reader accepts, `check_standard_pack_goal_table_references` holds its
pools to the composed root, with the same references (`GoalReferences`) the goal check holds
`goals.json` to. Every problem is `standard_pack_root_invalid` with `game`, `path` (the file),
`standard_pack_problem` and `json_pointer`, at most 64: first every member repeated in one object,
then the forms and limits in the reader's order (the document's members, `items` in file order,
each table in file order, its members in the order `name`, `progress`, `columns`, `classes`,
`keys`: the classes and keys are read against the columns, the keys against the classes too; a
name, a pool or a fact key written a second time where once is allowed is found where the second
is read), then, for a file whose forms all hold, what it says of itself (the recipes, then each
table's prerequisites, roles and prerequisites' positions), then the references'.

| `standard_pack_problem` | When | `json_pointer` |
|---|---|---|
| `malformed` | no JSON or no object (a byte-order mark too); a member repeated in one object, unknown, missing or not of its type (an integer outside the safe integers too); a `schema_version` that is not `actingcommand.goal-tables.v1`; a name outside its form (an item beginning with `@`, a column named `class` or `own`, an override index with a leading zero); a `per` none of the three; a second `held_by: true` of a table | the member (empty for the whole document) |
| `game_mismatch` | `game` is not the directory's | `/game` |
| `table_out_of_limits` | the file is larger than 4 MiB; a list or a map of a count outside its limit; a name or an id of a length outside its limit; an amount or a `makes` of 0 | what exceeds |
| `table_name_duplicate` | a table, or a column of a table, has a name an earlier one of its list has | the later `name` |
| `table_item_unknown` | a recipe's `from` or `cost`, a step or a key's `own` names an item the file does not declare | the name |
| `table_role_unknown` | a step names `@<role>` that a key which takes the step (its class's, as the key overrides it, or its own) does not declare in its `own`; each pointer once | the step's member |
| `table_class_unknown` | a key's `class` is no class of its table | `.../keys/<key>/class` |
| `table_column_unknown` | a reading column a `read` or a sub reads that no list of the table gives; a class's or a key's member that is no column of the table; a sub a key writes that its column does not have; a prerequisite's `column` | the name |
| `table_ladder_invalid` | a ladder of fewer than two positions (at `positions`), a position written twice or not of its column's width (at the position), a count of steps other than one fewer than its positions (at `steps`); a class's ladder of a column not of `per: "class"` (at it); a key's override of a ladder its class does not have (at the column) or of a step that ladder does not have (at the step) | the member named |
| `table_recipe_cycle` | an item reaches itself through the recipes (`from` and `cost`) | each such item's `recipe` |
| `table_requires_cycle` | a column reaches itself through its prerequisites (one that names itself too) | each such column's `requires` |
| `table_requires_invalid` | a prerequisite names a column of `per: "sub"` (at its `column`); or its `at_least` is no position of the required column's ladder, or its `from` no position of the requiring column's ladder, for a class or a key that has steps in the requiring column (at that member, once): a class that has no ladder of the requiring column is not held to it ("Prerequisites") | the member named |
| `table_recipe_without_pool` | an item with a recipe names no pool | `/items/<item>` |
| `table_pool_duplicate` | fix round 1: an item names a pool an earlier item names (a holding has one name in the file) | the later item's `pool` |
| `table_list_key_invalid` | a progress list's `fact_key` is no list reading's key | its `fact_key` |
| `table_list_key_duplicate` | fix round 1: a progress list's `fact_key` is the one of an earlier list of its table | the later list's `fact_key` |
| `table_reading_column_duplicate` | a reading column listed twice among a table's lists, or twice in one `read` or sub; fix round 1: a list's `key_column` listed among its own `columns` (the list names that column twice) | the later; the key column among the `columns` |
| `table_pool_unknown` | an item's pool is no pool of the composed `schedule.json` (none without the file) | `/items/<item>/pool` |
| `table_pool_unvalued` | the pool of an item without a recipe (the one a need lands on) is one a goal cannot be put on: the judgement the goal check gives a `resource` entry's pool (`goal_pool_unvalued`), the same map, not a second one. The pool of an item with a recipe need only be a pool of the root (every pool declares its reading) | `/items/<item>/pool` |

`goal_tables_undeclared` is the installer's ("Interface").

**A refused file never fails its game root.** It applies no `records` entry of `goals.json`; every
other entry of `goals.json` applies as before, and the root runs.

## Examples

Two neutral tables; every name and amount is made up. The first: a table whose classes carry two
ladders (a grade read from one reading column, a level read from two), whose keys carry their own
ladder and a column of two subs with prerequisites, one of them no column's, and items that are
made from others.

```json
{"schema_version": "actingcommand.goal-tables.v1", "game": "neutral",
 "items": {"gold": {"pool": "pool.neutral.gold"}, "exp": {"pool": "pool.neutral.exp"},
  "book_3": {"pool": "pool.neutral.book_3"}, "badge_a": {"pool": "pool.neutral.badge_a"},
  "ore_3": {"pool": "pool.neutral.ore_3"},
  "gel_3": {"pool": "pool.neutral.gel_3"},
  "ore_4": {"pool": "pool.neutral.ore_4",
            "recipe": {"makes": 1, "from": {"ore_3": 4}, "cost": {"gold": 300}}},
  "core_5": {"pool": "pool.neutral.core_5",
             "recipe": {"makes": 1, "from": {"ore_4": 1, "gel_3": 2}, "cost": {"gold": 400}}}},
 "tables": [{"name": "heroes",
  "progress": [{"fact_key": "inventory.list.neutral_roster", "key_column": "hero", "held_by": true,
                "columns": ["phase", "level"]},
               {"fact_key": "inventory.list.neutral_skills", "key_column": "hero",
                "columns": ["skill", "mastery_1", "mastery_2"]}],
  "columns": [{"name": "phase", "read": ["phase"], "per": "class"},
              {"name": "level", "read": ["phase", "level"], "per": "class", "requires": []},
              {"name": "skill", "read": ["skill"], "per": "key"},
              {"name": "mastery", "per": "sub", "subs": {"1": ["mastery_1"], "2": ["mastery_2"]},
               "requires": [{"column": "phase", "at_least": [2]}, {"column": "skill", "at_least": [4]},
                            {"condition": "side_quest"}]}],
  "classes": {"rank_6": {
     "phase": {"positions": [[0], [1], [2]],
               "steps": [{"badge_a": 4, "gold": 20000}, {"core_5": 3, "ore_4": 8, "gold": 100000}]},
     "level": {"positions": [[0, 1], [0, 2], [1, 1], [1, 2], [2, 1], [2, 2]],
               "steps": [{"exp": 100, "gold": 80}, {}, {"exp": 400, "gold": 300}, {},
                         {"exp": 900, "gold": 700}]}}},
  "keys": {"hero_a": {"class": "rank_6", "phase": {"steps": {"0": {"badge_a": 3, "gel_3": 2, "gold": 20000}}},
     "skill": {"positions": [[1], [2], [3], [4]],
               "steps": [{"book_3": 5}, {"book_3": 8, "ore_3": 4}, {"book_3": 8, "gel_3": 6}]},
     "mastery": {"1": {"positions": [[0], [1], [2]], "steps": [{"book_3": 8, "ore_4": 4}, {"book_3": 12, "core_5": 2}]},
                 "2": {"positions": [[0], [1], [2]], "steps": [{"book_3": 8, "gel_3": 5}, {"book_3": 12, "ore_4": 4}]}}},
   "hero_b": {"class": "rank_6", "skill": {"positions": [[1], [2]], "steps": [{"book_3": 5}]}}}}]}
```

A `goals.json` of the same game names its table:
`{"name": "raise", "type": "records", "table": "heroes", "max_records": 16, "title": {"en": "Whom to raise"}}`.

The second: one ladder of a class read from two reading columns, whose steps use a role that each
key names in its own items.

```json
{"schema_version": "actingcommand.goal-tables.v1", "game": "neutral2",
 "items": {"coins": {"pool": "pool.neutral2.coins"}, "shard_a": {"pool": "pool.neutral2.shard_a"},
           "shard_b": {"pool": "pool.neutral2.shard_b"}},
 "tables": [{"name": "units",
  "progress": [{"fact_key": "inventory.list.neutral2_units", "key_column": "unit", "held_by": true,
                "columns": ["star", "gear"]}],
  "columns": [{"name": "rank", "read": ["star", "gear"], "per": "class"}],
  "classes": {"all": {"rank": {"positions": [[1, 0], [2, 0], [3, 0], [4, 0], [5, 1], [5, 2], [5, 3]],
     "steps": [{"@shard": 20}, {"@shard": 40}, {"@shard": 60}, {"@shard": 80, "coins": 500000},
               {"@shard": 100}, {"@shard": 120}]}}},
  "keys": {"unit_a": {"class": "all", "own": {"shard": "shard_a"}},
           "unit_b": {"class": "all", "own": {"shard": "shard_b"}}}}]}
```

Both read, and the contract crate's tests read them as written here.

## Who reads it

Every reader calls the two functions above; none has a copy.

- **`actinglab standard-pack check --root <game root>`** reads the file beside `goals.json`
  through its bound plus one byte, lists it in `root_files` after `goals.json`, and checks it with
  `goals.json` in one goal check after the composition: the tables' pools held to the composed
  schedule's, then, for a file the reader and the references accept, its tables given to the
  references of `goals.json`, whose `records` entries name them (a refused file leaves them
  unjudged). Each problem is one `standard_pack_root_invalid` warning naming the file and the
  pointer, after `goals.json`'s; the root is checked as without the file, and the check exits as
  for a refused `goals.json`. A file that exists and cannot be read is the check's error.
- **`actinglab package bundle --goal-tables <goal-tables.json>`** checks the file with `--goals`
  by the same check and lays an accepted one beside `bundle.json` byte for byte; a refused file
  refuses the bundle as a refused `goals.json` does (`standard_pack_root_invalid` with its first
  problem, every problem in the message, `--out` not written), `goals.json`'s refusal first.
- **`actinglab resource verify` and `standard-pack build`**: a repository's `goal-tables.json` is a
  read path of the `checkout` precondition and is checked, laid, zipped, read back and installed in
  the `bundle` step as `goals.json` is, by the same check (`check_bundle_goals`); a refused file
  fails the step with every problem and no zip is written; the counts name an accepted one
  (`goal_tables {game, path, entries {tables, items}}`) ([resource-verify.md](resource-verify.md)).
  `standard-pack adopt` bundles it with the repository's other root files.
- **The installer's `place`** takes it from a bundle zip as a root file, placed after
  `schedule.json` and before `goals.json` (whose records entries name its tables), and sets it
  aside as `goal-tables.json.old-<ms>` when the game root holds one and a later zip carries none,
  after `goals.json`; `undo` puts it back. A zip that carries it declares a `package` maximum of 28
  at least ("Interface"). The readiness item I2 asks the same of a game root.
- **The Runtime's scheduler workbench** reads it where it reads `goals.json` (where it reads every
  package's declaration), with the reader alone, marked with the root's files so that a change of
  it is read at the next check. It keeps what it read with the root's goals for the generation in
  force, with the sha256 of the bytes (`StandardPackGoalRoot::goal_tables`), and **parses the file
  once per content**: a read whose bytes have the sha256 of what it keeps takes what it keeps. A
  file the reader refuses, or that cannot be read, applies no `records` entry:
  `standard_pack_goal_tables_not_applied` names it (a warning; `path` the file, `goals_failure`
  `refused` with the reader's first problem as its cause, or `unreadable` with the operating
  system's failure), recorded once per sha256 of a refused file and once until an unreadable one
  reads again; the root runs and every other entry of `goals.json` applies as before. Fix round 1
  gave the file this code of its own: `standard_pack_goals_not_applied`, with the same values, is
  `goals.json`'s alone, whose failure applies none of its entries, so a reader tells the two
  consequences apart by the code, not by `path`. The offline readers that read
  a root as the workbench does keep nothing between reads. Since Workflow #455 RR3 (0.12.95) the
  needs computed before every evaluation read the tables the workbench keeps for the generation in
  force, from memory, with their sha256 ("What is computed").

## What is computed

Workflow #455 RR3 (0.12.95; [v012-register.md](v012-register.md) §1.538). A records entry's records
are stored on an instance ([instance-fact-store.md](instance-fact-store.md), "Goal records facts",
since 0.12.94). Before every evaluation, and again at the admission of an intent, the Runtime
computes for each instance that holds records, with one pure function, how much of each item the
recorded characters still need and do not have, from: the instance's stored records (read by their
own reader); this file as the workbench keeps it for the generation in force; the instance's
progress lists as the fact store holds them; and the holdings, as the evaluator reads a target's
inventory. The result lives one evaluation: it is given to the evaluator in memory, stacked on the
stored goals there ([resource-targets.md](resource-targets.md), "Needs computed from the
records"), and nothing of it is stored. This file's shape is unchanged for it.

- **Applied records.** A record applies when its entry is a `records` entry of the root's accepted
  `goals.json` whose table is a table of this file, its key a key of that table, and its `to` one
  the goal edit's own check takes now. Any other record adds nothing: `goal_records_not_applied`
  with the word the edit would refuse it with (`name_unknown`, `not_an_option`, `out_of_range`,
  `wrong_type`; `record_unreadable` for a stored record its reader does not read).
- **A key is walked once** (fix round 1). Two `records` entries may name one table, and one key may
  be recorded under both. The first of its records in the stored order (the entry set first) is
  walked; a later record of the same table and key adds nothing and is said with the same code,
  `goal_records_not_applied` with `key_repeated` (a word no edit refuses it with: each entry's
  records are checked on their own), so a character's need is never counted twice. Keeping the
  character under one of the two entries ends it.
- **The progress lists.** A list is read when it is present, of a confidence above 0, a record list
  each of whose rows holds a text under the list's `key_column` (the rule a list a pool is read from
  obeys), not invalidated by a timeline reset at or after its observation and not past its expiry.
  The table's `held_by` list decides first: not known, the whole record adds nothing
  (`goal_records_progress_unknown`, with the list and its `unknown_reason`); known and without a row
  of the key, the character is not held and the record adds nothing (`goal_records_not_held`), which
  is known, not pending.
- **The walk.** For each column the record's `to` names (each named sub of a column of
  `per: "sub"`), the current position is the tuple of the column's reading columns in the key's row,
  equal member for member to one of the ladder's `positions`. Not known, that column adds nothing,
  each cause with its own code: its list not known (`goal_records_progress_unknown`); its list known
  without a row of the key (`goal_records_row_missing`, fix round 1: a list other than the `held_by`
  one, whose reading has not covered the character, which a later reading may); a row of the key
  without the reading column (`goal_records_reading_missing`: the package that reads the list does
  not write the column this file names, which no later reading of that package mends); readings
  that form no position, a value of another kind or a
  tuple equal to none (`goal_records_position_unknown`). Known, the need is the sum of the steps from
  the current position to the target in the order of `positions` (a key's override of its class's
  step taken instead, a step's `@<role>` the key's own item); a target at or behind the current
  position asks nothing.
- **Prerequisites** ("Prerequisites", above). A step the walk takes that asks a prerequisite of
  another column (every step without `from`; a step that arrives at `from` or beyond with it) takes
  that column to `at_least` as well, when the record's own target for it does not reach it; the
  required column's walk then counts from its own current position, and its own prerequisites
  follow. A requiring column whose position is not known takes no step and asks nothing. A
  `condition` is named (`goal_records_condition`) and asks nothing.
- **Taking apart, once.** The needs of all records are summed by item; then, in the topological
  order of the recipes from the top, an item with a recipe is covered by its holding up to its
  need, and what is not covered is made (rounded up by `makes`), adding its `from` and its `cost` to
  those items' needs. An item with a recipe whose holding is not known adds nothing below itself
  (`goal_records_holding_unknown`). What remains on an item without a recipe is its pool's need;
  an item without a recipe and without a pool is shown (`goal_records_item_unweighed`) and weighs
  nothing.
- **A lower bound.** What is not known adds nothing, so the result never exceeds the true need: a
  pending part never raises a gap.
- **The set** of one instance: each pool's need (by the id the root's files write it; the part a
  holding covers on the pool of an item with a recipe, the whole need while that holding is not
  known), every part that added nothing with its cause, this file's sha256, when the records were
  set, and when its freshness ends: the earliest expiry among the lists and holdings it read. Its
  identity hashes all of it.

For example, on the contract's first example above, with `hero_a` at phase `[0]` and a record
`{"phase": [2]}`: the walk takes the phase ladder's two steps (`hero_a`'s own first step, then the
class's second): 3 `badge_a`, 2 `gel_3`, 120000 `gold`, 3 `core_5` and 8 `ore_4`. With 1 `core_5`
and 2 `ore_4` held, 1 `core_5` is covered and 2 are made (2 `ore_4`, 4 `gel_3` and 800 `gold`
more), so `ore_4` is needed 10 times, 2 covered and 8 made (32 `ore_3` and 2400 `gold`): the needs
landing are 3 `badge_a`, 6 `gel_3`, 32 `ore_3` and 123200 `gold`.

## Interface

`package` revision 28 ([component-interfaces.md](component-interfaces.md), the row 28, with
`goals.json` version 3): a bundle that carries `goal-tables.json` declares `package` [28, 28] (the
contract crate's `GOAL_TABLES_PACKAGE_REVISION`), which `standard-pack build`'s revision table
counts by the file's presence at the zip's root, whatever it holds. The installer's `place` of an
earlier build (its range ending below 28) refuses such a bundle through its `package` edge
(`interface_edge`), in the plan, before the hold and any write; this build's `place` refuses a zip
that carries the file and declares a maximum below 28 `pack_invalid` caused by
`standard_pack_root_invalid` with `goal_tables_undeclared` at `<zip>\interfaces.json`, the pointer
empty, before the hold and any write. A Runtime before it opens no such file. A rollback is not
refused for it: an earlier Runtime reads a root that holds the file as one without it, and an
earlier installer refuses a bundle that carries it. `ledger`, `runtime-client`, `actingd-config`
and `runtime-outcomes` do not move: the words are the existing vocabulary `standard_pack_problem`'s
under the existing code `standard_pack_root_invalid`, and the Runtime's record is
`standard_pack_goal_tables_not_applied` (fix round 1), a new code of the catalog only, with the
values and the vocabulary `goals_failure` of `standard_pack_goals_not_applied`.
