# Standard-pack authoring: `standard-pack adopt`

Workflow #464, slice P1 (Runtime 0.12.60; [v012-register.md](v012-register.md) §1.377). A
recording ends in a package (`record stop`, [lab-recording.md](lab-recording.md)); before this
slice a person copied it into a resource repository by hand and wrote its rule into
`schedule.json` member by member. `actinglab standard-pack adopt` does it in one operation: it
makes the package a member of its game's standard pack in the repository's checkout, with a rule
the Runtime's composer accepts and its evaluator dispatches
([standard-pack-schedule.md](standard-pack-schedule.md)). It writes only inside the checkout it is
given and under the Lab's own state directory, runs every check before its first write, deletes
nothing in the checkout, and answers every member it derives with where the value came from. Its
answer never says more than is true, and a call that cannot finish fails before it writes. Whether
a package is ready to be scheduled on an installation is another slice's.

## The operation

```text
actinglab standard-pack adopt --repo <checkout> (--record-id <id> | --package <path>)
    [--decide <decision.json>] [--replace] [--state-dir <dir>]
```

The row `standard-pack adopt` of the operation table ([lab-operations.json](lab-operations.json),
module `authoring`, slice `SP-ADOPT`); over MCP the tool `ac_standard_pack_adopt`, served by a
`serve` child ([lab-two-entries.md](lab-two-entries.md)). Under the global `--dry-run` it runs every
check and writes nothing in the checkout ("`--dry-run`" below).

| Input | What it is |
|---|---|
| `--repo` | The game's working root in the source layout: `applications.json` (its game and servers), `maintenance.json` (a list, `[]` when none), `packs/` (one directory per package) and the root files the repository holds (`schedule.json`, `pages.json`, `navigation.json`, `interrupts.json`, `display-layout.json`, `loading.json`, `goals.json`, `notices/`). |
| `--record-id` | A stopped recording of the state directory, `<state>/record-artifacts/<record>/lab/`: its `recording.json` and the package its last stop generated, `out/<package_id>.<zip or json>`. |
| `--package` | Any package instead: a content directory, a `.zip` or a `.json` container. |
| `--decide` | The decision document ("The decision document" below); required on a first adoption. |
| `--replace` | Adopt a package adopted before again ("Repeating" below). |
| `--state-dir` | The Lab's state directory, as the recording commands take it (default actinglab's session directory): it holds the recordings, the kept copies (`adopt/`) and the check trees (`adopt-check/`). With `--replace` it must lie on the checkout's volume, since the replaced directory is moved into it. |

## The decision document

`actingcommand.pack-decision.v1` holds only what the Lab cannot know; every other member of the
rule is derived ("What is derived"). A member it does not list is refused at its pointer.

| Member | Value |
|---|---|
| `schema_version` | `actingcommand.pack-decision.v1` |
| `class` | `business` (a rule judged by its value), `maintenance` (a rule declaring `"maintenance": true`, or a `maintenance.json` entry by its `uses`) or `held` (a `held` entry: kept from scheduling, with its reason). The class decides the package's one role ("One role"). |
| `trigger` | The rule's trigger, a predicate object as `schedule.json` writes it. A new rule needs it. |
| `value` | `"pending"`, or `{fixed_value_milli, origin}`: a whole number of at least 1, `origin` absent or `pack` ("The value's origin"). A new business rule needs it, but a follower of a chain, which gives none ("A pending value"). |
| `consumes`, `produces` | Lists of `{pool_id, amount or expected_amount_milli, observation_source?, confidence_milli?}`, each `pool_id` a pool `schedule.json` declares; the direction and the evidence members not given are derived. |
| `limited`, `realization` | `true` when written: the marks of [standard-pack-schedule.md](standard-pack-schedule.md). |
| `uses` | For the class `maintenance` without a trigger: `startup`, `return_home` or `recognition`, or a list of them; a `maintenance.json` entry for the package's server. |
| `held` | For the class `held`: `{reason, reference?}`, each a non-blank text of at most 1024 bytes without a control character. |
| `overrides` | A derived member stated with its value, where the table of "What is derived" says it may be: `id`, `priority`, `feedback_stop`, `on_failure`, `sensitive`, `next_run_clamp_ms`, `yield_points`, `expected_duration_ms`, `cooldown_ms`, `load_profile`, `strategic_weight_milli`, `loop_budget`. |

The classes refuse the members they do not take: `business` takes no `uses` and no `held`;
`maintenance` no `value`, `limited`, `realization` or `held`; `held` nothing but `held`.

A business rule with its value, daily on the local clock:

```json
{
  "schema_version": "actingcommand.pack-decision.v1",
  "class": "business",
  "trigger": {
    "kind": "clock",
    "schedule": {
      "kind": "interval",
      "clock_source": {"kind": "local"},
      "every_ms": 86400000,
      "anchor_ms": 0
    }
  },
  "value": {"fixed_value_milli": 9000, "origin": "pack"}
}
```

The same rule while its value is not known yet (it is written and not dispatched until a value is
given, "A pending value"):

```json
{
  "schema_version": "actingcommand.pack-decision.v1",
  "class": "business",
  "trigger": {
    "kind": "clock",
    "schedule": {
      "kind": "interval",
      "clock_source": {"kind": "local"},
      "every_ms": 86400000,
      "anchor_ms": 0
    }
  },
  "value": "pending"
}
```

## The value's origin

(The coordinator's ruling on #464.) Adopt never writes `fixed_value_origin: "model"` on a caller's
word. A decided value is `"pending"` or a number of the pack's own: adopt writes the number as
`fixed_value_milli` and no `fixed_value_origin`, as a hand-written rule has it, and answers it as
given with the origin `pack`; `"pending"` writes no fixed value and is answered as given. A
decision whose `value` names the origin `model` is refused `value_origin_model` before anything is
written: only the model's export writes `model`; give the number with `pack`, or `pending`. A rule
adopt meets again that already carries `fixed_value_origin: "model"` keeps its value and its origin
whatever the decision names, and the answer reports both members in `model_kept`; adopt does not
remove the origin and does not copy it to another rule. Today no writer of `model` exists (the
model's export writes thresholds only), so no rule carries it and the Lab does not write it; the
provenance listing of `standard-pack check` therefore shows which values wait for the model.

## A pending value

A business rule whose value is not known (`"pending"`, or no fixed value, no `produces` and no
`consumes` entry, or an entry or a pool with the marker or without its valuation) is written and
composed, and the Runtime's evaluator does not dispatch it until a value is given: each of its
candidates is held with `benefit_unknown` and its `benefit_gap`. The answer says so in
`waits_for_value`: that registered reason as an outcome (`benefit_unknown`, `benefit_gap`, and
`pool_id` where a pool is why) and the same in words with what gives the value. The coverage stays
the composer's word (`undeclared` or `pending`, [standard-pack-schedule.md](standard-pack-schedule.md),
"Judged rules"), with its warning.

A follower of a chain (a rule that, by its trigger alone, can only be due in its chain head's run
window and rides with a rule whose value the file declares; "Judged rules" there) is dispatched
with its head whatever its own value: its coverage is `follower`, `waits_for_value` is absent, and
a new business rule that the check finds to be a follower may give no `value` at all. A decision
that says `"pending"` for a follower is refused `decision_invalid` at `/value`, since "pending"
would be untrue: give the follower no value at all, or adopt it apart from the chain (a trigger of
its own) with its value. A new business rule that is no follower and gives no value is refused
`decision_invalid` at `/value`. Both are told after the check of the tree, which finds the
followers, and before anything is written.

## One role

The decision's class decides, and adopt writes the root files it writes, `schedule.json` and
`maintenance.json`, so that the package has exactly one role there:

| Class | Written | Removed in the same call |
|---|---|---|
| `business` | its rule, without `maintenance` | its `held` entry; every `maintenance.json` entry of the package |
| `maintenance` | its rule with `"maintenance": true` (a trigger), or its `maintenance.json` entry (`uses`), or both | its `held` entry |
| `held` | its `held` entry (its rule, if any, is kept and not composed while the hold lasts) | every `maintenance.json` entry of the package |

A maintenance role another root file gives (a `pages.json` feature, a `navigation.json` edge or
recovery chain, an `interrupts.json` handler or `then.run`) is not ended by adopt, which does not
write those files: a decision of the class `business` or `held` for such a package is refused
`decision_invalid` at `/class`, naming the file. The answer lists in `removed_entries` every entry
it removed and in `replaced_entries` every entry that was there and that it changed (the rule, the
held entry, the `maintenance.json` entry), each with its file, its pointer in the file as read, the
entry as it was and, when replaced, as written. Without a decision nothing is removed or replaced,
and the class is the earlier adoption's as the composer reads the files (held, else maintenance
when a root file gives the package a maintenance role, else the rule's).

## The checks

Every check runs before the first write, in this order; a refusal is `adopt_refused`, its
`adopt_problem` naming the check with the member, entry or pointer below, and writes nothing in
the repository and keeps no copy.

| Check | `adopt_problem` | Named by |
|---|---|---|
| `--repo` holds `applications.json` (its game and servers), `maintenance.json`, `packs/` and a `schedule.json` that reads; no `.part` of a root file is left ("If a call was interrupted") | `repository_invalid` | `entry`; for `schedule.json` the reader's `json_pointer` and its refusal as the cause |
| The recording is as its last stop left it: `actingcommand.lab-recording.v1`, its `record_id`, `stopped`, not marked again since (`amended_at_unix_ms` absent or null), its package there | `recording_not_current` | `path` of `recording.json`, `member` |
| The package is admitted as `package preflight` admits it, and its `package_id` names one directory `packs/<package_id>` that no root file reserves | `package_invalid` | `path`, the loader's refusal as the cause; or `package_id` |
| Its game is `applications.json`'s and its server one of that file's | `package_mismatch` | `member` `game` or `server` |
| No `packs/<package_id>.part` is left ("If a call was interrupted"); not adopted before (its directory, its rule, its held entry, its `maintenance.json` entry), or `--replace`; the kept copy's place free | `pack_exists` | `entry` the directory or its `.part`, or `json_pointer` of the entry; `path` of a taken place |
| With `--replace`, no change below `packs/<package_id>` the version control does not hold (`git status --porcelain=v1 -z --untracked-files=all`, which writes nothing) | `pack_dirty` | `entry`, `count`; or the version control's failure as the cause |
| With `--replace`, the kept copy's place lies on the checkout's volume (the volume of the nearest existing part of each path, links resolved: the drive or network share on Windows, the device elsewhere; a volume mounted on a folder of another is not told apart on Windows), so that the replaced directory can be moved there | `replace_across_volumes` | `entry` the directory, `path` the kept place; the message says to give a `--state-dir` on the checkout's volume and, for a recording of the default state directory, its package by `--package <state>/record-artifacts/<record>/lab/out/<package_id>.<ext>` instead of `--record-id` |
| The decision document | `decision_invalid` | `path`, `json_pointer` (`""` for a first adoption without one) |
| Its value's origin ("The value's origin") | `value_origin_model` | `path`, `json_pointer` `/value/origin` |
| A class `business` or `held` beside a maintenance role a root file adopt does not write gives the package ("One role") | `decision_invalid` | `path`, `json_pointer` `/class` |
| A class other than `held` needs a package run through: its entry task's provenance a recording's, the cross-check `passed`, the first decision evaluated (no `first_decision_not_evaluated` warning, no application step) and no step whose click needs review | `recording_unreviewed` | `package_id`; the details carry `review` |
| No text of the package holds an absolute local path (a drive or network path, the state directory, the checkout) | `local_path` | `entry` the file, `json_pointer` the string |
| A root file adopt must change is in the canonical form ("The canonical form") | `root_file_not_canonical` | `entry` |
| Every outcome predicate of `schedule.json` as it would be written that reads the rule's own outcome (`kind` `outcome`, `task_id` the rule's id: its trigger and its `feedback_stop`, given, derived or kept, and another rule's or a pool's that reads it) names a key the package's entry task declares (`scheduling_outcome.mappings`), since the Runtime refuses every dispatch of a task whose outcome is read for a key its package does not declare ([scheduling/README.md](scheduling/README.md)) | `decision_invalid` | `path` the decision and `json_pointer` in it (`/trigger/…`, `/overrides/feedback_stop/…`) when the decision gave the member; otherwise `entry` `schedule.json` and `json_pointer` in the file as it would be written; the details carry `declared_outcome_keys` |
| The checkout's check tree is the tool's own and free (below) | `check_tree_unmarked`, `check_tree_busy` | `path` the check tree |
| The tree as it would be written composes (below) | `rule_refused` | `entry` `schedule.json`, the composer's `json_pointer`, its refusal as the cause; or `package_id` when the package comes out unclassified |
| A business rule's value ("A pending value") | `decision_invalid` | `path`, `json_pointer` `/value` |

The check of the tree lays the tree as it would be written in the checkout's check tree,
`<state>/adopt-check/<checkout>/`: the repository's pack directories with the package's own as it
would be written, the root files adopt changes, `package bundle` of them with every root file the
repository holds (its maintenance declarations those `maintenance.json` would hold), and the
bundle's installed layout checked by `standard-pack check`; both compose the schedule with one
instance per server. Another refusal of those two is answered as it is, and wrote nothing in the
repository either. The check tree is named in the answer and kept until the next call on the
checkout.

**The check tree is the tool's own scratch, not anyone's data** (the coordinator's ruling): one
directory for each checkout, `<checkout>` its last segment and 16 hexadecimal digits of the
SHA-256 of its resolved path; adopt writes the marker `.adopt-check-tree` into it when it makes it,
holds the marker open and locked for the whole call, and at the start of each call empties the
directory of everything but the marker, only when the marker is there, and never anything else. A
directory at that place without the marker is not touched and the call is refused
`check_tree_unmarked`; a call while another adoption of the checkout with the same state directory
holds the marker is refused `check_tree_busy`. The rule "nothing is removed" holds for the checkout
and for a replaced pack directory's kept copy; the check tree alone is emptied, by the tool, of
what the tool laid there.

## What is derived

Each member the decision does not give is derived, answered in `derived` with its value and where
it came from: `package` (the package itself), `file` (the file's rules of the same tier: the rules
declaring maintenance or of a maintenance-role package, or the others), `neutral` (a constant of
this contract) or `neutral_floor` (this contract's floor, which the package's own value falls
below); `feedback_stop` and `expected_duration_ms` also with `how`, in words. Nothing is chosen
silently: a member nothing gives is refused `decision_invalid` at `/overrides/<member>`.

| Member | Derived as | May the decision state it |
|---|---|---|
| `id` | The package's `package_id` (`package`). | yes, `overrides.id` |
| `package_id` | The package's `package_id` (`package`). | no |
| `priority` | With exactly two priorities among the file's rules, the lower for a business rule and the higher for a maintenance rule; else the one value the rules of its tier agree on (`file`); else none, and the decision must state it. | yes, `overrides.priority` |
| `trigger` | Never derived. | must, for a new rule |
| `feedback_stop` | Never copied from another rule (a copied stop may never come true, and the task would be dispatched again at every cooldown): "this rule's own run ended with a key its package declares", `{kind: outcome, task_id: <rule>, outcome_key: <key>, comparison: eq, value: {type: boolean, value: true}}` for every key of a normal end the entry task declares (every mapping's but a `busy` one's and one with a `remedy`), under `any` when there are several (`package`), so that the task runs once for each time its trigger comes true; none when the package declares no such key, and the decision must state it. | yes, `overrides.feedback_stop`, on keys the package declares ("The checks") |
| `consumes`, `produces` | `[]` (`neutral`) when the decision names none; for each decided entry its direction and, when not given, `observation_source` `self_reported` and `confidence_milli` 1000 (`neutral`). | yes, `consumes` and `produces` |
| `fixed_value_milli` | Never derived. | yes, `value` ("The value's origin") |
| `on_failure` | `{action: pause, retry_limit: 1, retry_backoff_ms: 0, escalation_threshold: 2}` (`neutral`). | yes, `overrides.on_failure` |
| `sensitive` | `false` (`neutral`). | yes, `overrides.sensitive` |
| `yield_points` | `[]` (`neutral`). | yes, `overrides.yield_points` |
| `expected_duration_ms` | The package's lower bound of one run, the sum over its operations of `post_delay_ms` and `expect_after.interval_ms` (`package`), never below the neutral 60000: a lower bound below it gives 60000 (`neutral_floor`), since a recorded package's waits (some 700 ms a step) would make its value per time look far higher than it is and rank the task first among its instance's tasks. The measured runs replace it. | yes, `overrides.expected_duration_ms`, within the schedule's own limits |
| `next_run_clamp_ms` | The most frequent of the tier's rules (`file`); else 3600000 (`neutral`). | yes, `overrides.next_run_clamp_ms` |
| `cooldown_ms` | The most frequent of the tier's rules (`file`); else 60000 (`neutral`). | yes, `overrides.cooldown_ms` |
| `load_profile` | The most frequent of the tier's rules (`file`); else `{kind: light}` (`neutral`). | yes, `overrides.load_profile` |
| `strategic_weight_milli` | The most frequent of the tier's rules (`file`); else 1000 (`neutral`). | yes, `overrides.strategic_weight_milli` |
| `loop_budget` | Absent. | yes, `overrides.loop_budget` |
| `requires` words | The words the rule needs and the file does not name (below). | no |

The members the decision gave are answered in `given`, in the order written; the value as
`fixed_value_milli` with the origin `pack`, or as `value` `"pending"`. A given member is written as
given, its outcome predicates with the keys they name. A new rule is written in the
order `id`, `package_id`, `maintenance`, `priority`, `trigger`, `feedback_stop`, `consumes`,
`produces`, `fixed_value_milli`, `realization`, `limited`, `on_failure`, `sensitive`,
`next_run_clamp_ms`, `yield_points`, `expected_duration_ms`, `cooldown_ms`, `load_profile`,
`strategic_weight_milli`, `loop_budget`, each only when written. `requires` gains the words the
rule needs and the file does not name ([standard-pack-schedule.md](standard-pack-schedule.md),
"`requires`": `benefit_fixed_value`, `fact_reference`, `pool_fact_reference`,
`fact_unknown_probe`, `consumption_fact`, `queue_wait`), after `schema_version` when the file has no
`requires`; the answer lists them in `requires_added`.

## What it writes

Only inside `--repo` and under the state directory, each part written whole and renamed, nothing
deleted in the checkout or of a kept copy:

- `packs/<package_id>/`, written as `packs/<package_id>.part/` and renamed: only `control.json` and
  `resources/**` of the package, each JSON file in the canonical form. With `--replace` the old
  directory is first moved whole to `<state>/adopt/<package_id>.old-<ms>/`, which the answer names
  in `kept`. When the new directory then cannot take its place (on Windows a file in use there
  makes the rename fail), the old directory is moved back by a rename, the new one stays as its
  `.part`, and the refusal says what happened; when moving it back fails too, it says where the old
  directory is.
- `schedule.json` (the rule appended or changed, the held entry, `requires`) and `maintenance.json`
  (an entry `{package_id, server, uses}`, or the package's entries removed, "One role"), each
  written as `<name>.part` and renamed, each only when it changes. Before the first write, and
  again immediately before each rename, the file is read again and compared with the bytes the
  call computed from; when it changed (another adoption, which the MCP server allows at the same
  time, or a person), the call stops with `adopt_write_failed` at that file, renames nothing more
  and overwrites nothing of that work.

A failed write is `adopt_write_failed`: `path` where it stopped and `paths` what it had written or
moved before, in order, the operating system's refusal as the cause when there is one; nothing is
removed in the checkout (a `.part` and a kept copy stay where they are).

**If a call was interrupted** (a crash, a stopped process) after it began writing, it leaves
`packs/<package_id>.part/`, `schedule.json.part` or `maintenance.json.part` in the checkout. The
next call refuses as long as one of its own is there (`pack_exists` for the package's `.part`,
`repository_invalid` for a root file's), and its message names each leftover and says what to do:
when the interrupted call began, `packs/<package_id>` was absent or committed (adopt refuses a
pack directory with changes), so `git status -- packs/<package_id>` shows exactly what it left
there; `schedule.json` and `maintenance.json` are each written whole and renamed in one step, so
each is as it was before that call or as that call computed it (`git diff` shows which); a
replaced directory it had moved away is in the last commit and under the state directory's
`adopt/`. The leftovers named can be moved out of the checkout by hand (adopt deletes nothing);
then run adopt again.

The repository's own items adopt does not write are named in `next` when the checkout holds them:
a regression case (`regression/<game>/`), the catalog section (`docs/PACKS.md`) and the census row
(`ours/coverage/business-v1.csv`).

## Repeating (`--replace`)

Without a decision the rule, the held entry and the maintenance entry are kept byte for byte, and
only the pack directory is replaced; the kept rule's outcome predicates on the rule itself are
checked against the new package's keys ("The checks"). A decision changes only the members it
names, and decides the package's one role ("One role"; a decision of another class than `held`
lifts a hold, so the rule is composed again); a value the model wrote is kept ("The value's
origin"). A derived member the derivation would now give otherwise is answered in `derived_drift`
with the written and the derived value, and never written.

## `--dry-run`

Every check runs, the check tree under `<state>/adopt-check/` included; the answer is `validated`
with `written` empty and `would_write` what an adoption would write; nothing is written in the
repository and no copy is kept ([actinglab-dry-run.md](actinglab-dry-run.md)).

## The answer

`status` (`adopted` or `validated`), `dry_run`, `repo`, `game`, `server`, `package_id`,
`entry_task_id`, `class`, `replaced`, `review` (`recorded`, `record_id`, `cross_check`,
`first_decision_evaluated`, `steps_needing_review`, `run_through`), `rule`, `held` and
`maintenance` as written, `removed_entries` and `replaced_entries` ("One role"),
`requires_added`, `given`, `derived` (each `member`, `value`, `from` and, where it says more,
`how`), `derived_drift`, `model_kept`, `coverage` (a business rule: `complete`, `pending`,
`undeclared` or `follower`, as the composer counts the file's judged rules), `waits_for_value` ("A
pending value"), `warnings` (every warning of the check, the game's included, each once),
`written`, `would_write`, `kept`, `check_tree` and `next`.

## The canonical form

The form `package fmt` writes: two-space indentation, one member per line, members in their order,
LF line ends and a final newline. A root file adopt must change and that is not in it is refused
(`root_file_not_canonical`), so that the diff of an adoption shows only what it changed: write it so
once, in a commit of its own.
