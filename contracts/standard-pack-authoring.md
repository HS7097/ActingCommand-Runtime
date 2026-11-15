# Standard-pack authoring: `standard-pack adopt`

Workflow #464, slice P1 (Runtime 0.12.61; [v012-register.md](v012-register.md) §1.377). A
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
    [--decide <decision.json> | --held-reason <text> [--held-reference <text>]] [--replace]
    [--state-dir <dir>]
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
| `--decide` | The decision document ("The decision document" below); a first adoption needs it or `--held-reason`. |
| `--held-reason` | The decision of the class `held` without a document: its reason ("A held decision by two flags" below). Not with `--decide`. |
| `--held-reference` | With `--held-reason` only: that decision's reference. |
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
| `uses` | For the class `maintenance` without a trigger: `startup`, `return_home` or `recognition`, or a list of them; a `maintenance.json` entry for the package's server. Beside a business rule of the package (one the composer judges by its value: it does not declare `maintenance` and no root file gives the package a maintenance role) it is refused, since adopt never turns that rule into a maintenance rule unasked ("One role"). |
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

### A held decision by two flags

(Workflow #463 S3-4, Runtime 0.12.63; [v012-register.md](v012-register.md) §1.411.) A package
that has not run through on a real device is registered with its rules and stays held, and
packages are opened one by one as each is run through. So the first decision a recorded package
takes is the class `held` with a reason in words, and it needs no file:

```text
--held-reason <text> [--held-reference <text>]
```

is exactly the decision

```json
{
  "schema_version": "actingcommand.pack-decision.v1",
  "class": "held",
  "held": {"reason": "<text>", "reference": "<text>"}
}
```

(`reference` only when `--held-reference` is given). Adopt builds that document from the two texts
as given and hands it to the reader of a document given by `--decide`, at the same place among the
checks, so every check and every byte written is the one that document gives: its held entry, and
a rule the package already has kept and not composed while the hold lasts ("One role"). The texts
take what the document's `held` takes, a non-blank text of at most 1024 bytes without a control
character: a reason outside that is refused `decision_invalid` at `/held/reason` (a reference at
`/held/reference`), as the document's is, the outcome naming no `path`, since no file was read.
Before anything is read or written, `--decide` beside either flag is refused `validation_failed`
naming the held flag (`--held-reason` when it is given), `argument_problem` `conflicting`, and
`--held-reference` without `--held-reason` the same, naming `--held-reference`. The flags only
hold: nothing here lifts a hold, which a decision of another class given by `--decide` does
("Repeating").

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

A decided `"pending"` says only that adopt writes no fixed value; whether the rule is dispatched is
the Runtime evaluator's placement ([scheduling/README.md](scheduling/README.md), "Net benefit
within a tier", "Who is judged"), which adopt answers exactly. A business rule as written is in one of these rows, the first that
fits ("reads": with a fixed value its `produces` are not read, its `consumes` are):

| The business rule as written | Judged by its value | Dispatched | The answer |
|---|---|---|---|
| A follower of a chain (coverage `follower`) | no | with its chain's head, whatever its own value | `value_not_judged`, `because` `follower` |
| A manual request: its trigger requires an instruction of the `session.` family from its root through `all` alone | no | when the instruction is given, whatever its value | `value_not_judged`, `because` `manual_request` |
| No fixed value and no `produces` or `consumes` entry (`"pending"` alone) | yes | not until a value is given | `waits_for_value`, `benefit_gap` `no_effects` |
| An entry it reads with the amount `"pending"` | yes | not until a value is given | `waits_for_value`, `entry_pending` with its `pool_id` |
| A pool it reads with the valuation `"pending"` | yes | not until a value is given | `waits_for_value`, `valuation_pending` with its `pool_id` |
| A pool it reads without a valuation | yes | not until a value is given | `waits_for_value`, `valuation_missing` with its `pool_id` |
| A fixed value, or `"pending"` with `produces` or `consumes` entries, every entry it reads with its amount and every pool it reads valued | yes | when its net benefit (the value or what it produces, less the cost of what it consumes) is positive | neither |

So `"pending"` with a valued `produces` is judged by that entry and dispatched when its net benefit
is positive. A regenerating pool the rule consumes is also judged by its reading at each evaluation
(`reading_unknown` while that reading is not known), which is the instance's state, not the rule's
value, and which adopt does not answer. `waits_for_value` carries the registered reason the
evaluator holds each candidate with, as an outcome (`benefit_unknown`, `benefit_gap`, and `pool_id`
where a pool is why), and the same in words with what gives the value; `value_not_judged` says why
the value is not judged, in a word and in words. The coverage stays the composer's word
(`complete`, `pending`, `undeclared` or `follower`,
[standard-pack-schedule.md](standard-pack-schedule.md), "Judged rules"), with its warning: the
composer counts a manual request as any other rule it judges, so one without a value is
`undeclared` there although the evaluator dispatches it when it is asked for.

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
write those files: a decision of the class `business` for such a package, whose rule would never
be judged beside it, is refused `decision_invalid` at `/class`, naming the file. A decision of the
class `held` is taken (a package that `pages.json` names and that is not run through yet is
adopted as held): its hold is written beside that role, which stays as that file gives it.

A rule's role is the composer's judgement, which adopt calls and does not copy (the policy crate's
`is_judged_rule`, fix round 3): a rule is a business rule exactly when the composer judges it by its
value, that is when it does not declare `maintenance` and no root file (`maintenance.json`,
`pages.json`, `navigation.json`, `interrupts.json`) gives its package a maintenance role. So a rule
that declares nothing beside its package's `maintenance.json` entry, as a hand-written root may
hold it, is not a second role: the composer does not judge it, and it is part of the package's one
maintenance role. A decision of the class `maintenance` by its `uses` alone (no trigger) for a
package whose rule in `schedule.json` is a business rule is refused `business_rule_in_the_way`,
naming that rule by its id and its pointer (`entry` `schedule.json`, `json_pointer` `/rules/<i>`):
adopt never turns that rule into a maintenance rule unasked; take it out of `schedule.json` first,
or give the decision the trigger of a maintenance rule. Beside a rule the composer does not judge,
the same decision is taken, and the rule is kept as it was written (fix round 4): it gains no
`"maintenance": true`, which the composer does not need there, since its package's
`maintenance.json` entry (kept, or written by the decision's `uses`) or a role another root file
gives already leaves the rule unjudged (`is_judged_rule`), and which would only change what the
composer checks of the rule (`limited` is refused on a rule that declares maintenance, and its
`priority` is compared with the maintenance tier's). So the decision changes nothing of the rule
it does not name, and `given` names no `maintenance`. A decision of the class `maintenance` with a
trigger writes the rule with `"maintenance": true`; one without a trigger adds the declaration only
to a rule the composer would otherwise judge by its value (no `uses`, no `maintenance.json` entry
and no other root file's role). The answer lists in `removed_entries` every entry it removed and in
`replaced_entries` every entry that was there and that it changed (the rule, the held entry, the
`maintenance.json` entry), each with its file, its pointer in the file as read, the entry as it was
and, when replaced, as written. Without a decision nothing is removed or replaced, and the class is
the earlier adoption's as the composer reads the files (held, else the rule's: business when the
composer judges it, maintenance otherwise; else maintenance for a `maintenance.json` entry); when
`schedule.json` gives the package a held entry and `maintenance.json` an entry too (two roles,
which an adoption leaves only when it was interrupted between its two renames, "If a call was
interrupted"; a rule beside that entry is never a second role, since the composer judges no rule
of a package that entry names), a call without a decision is refused `repository_invalid` at that
entry (`entry` `maintenance.json`), its message saying what to do.

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
| No `packs/<package_id>.part` is left ("If a call was interrupted"); not adopted before (its directory, its rule, its held entry, its `maintenance.json` entry), or `--replace`; the kept copy's place free | `pack_exists` | `entry` the directory or its `.part`, or `json_pointer` of the entry; `path` of a taken place; without `--replace` the message names what the root files hold of the package, and when `schedule.json` and `maintenance.json` hold nothing of it beside its directory, either names the root file adopt does not write that gives it a role and what the call accepts for it, or says that an adoption was interrupted before it wrote the root files ("If a call was interrupted") |
| With `--replace`, no change below `packs/<package_id>` the version control does not hold (`git status --porcelain=v1 -z --untracked-files=all`, which writes nothing) | `pack_dirty` | `entry`, `count`; or the version control's failure as the cause |
| With `--replace`, the kept copy's place lies on the checkout's volume (the volume of the nearest existing part of each path, links resolved: the drive or network share on Windows, the device elsewhere; a volume mounted on a folder of another is not told apart on Windows), so that the replaced directory can be moved there | `replace_across_volumes` | `entry` the directory, `path` the kept place; the message says to give a `--state-dir` on the checkout's volume and, for a recording of the default state directory, its package by `--package <state>/record-artifacts/<record>/lab/out/<package_id>.<ext>` instead of `--record-id` |
| The decision document | `decision_invalid` | `path`, `json_pointer` (`""` for a first adoption without one); no `path` for the decision of `--held-reason` ("A held decision by two flags") |
| Its value's origin ("The value's origin") | `value_origin_model` | `path`, `json_pointer` `/value/origin` |
| A class `business` beside a maintenance role a root file adopt does not write gives the package ("One role") | `decision_invalid` | `path`, `json_pointer` `/class` |
| A class `maintenance` by its `uses` alone beside the package's business rule, a rule the composer judges by its value ("One role") | `business_rule_in_the_way` | `entry` `schedule.json`, `json_pointer` `/rules/<i>`, `package_id`; the message names the rule's id |
| Without a decision, no held entry of the package in `schedule.json` beside its `maintenance.json` entry ("One role", "If a call was interrupted") | `repository_invalid` | `entry` `maintenance.json`, `json_pointer` the entry, `package_id` |
| A class other than `held` needs a package run through: its entry task's provenance a recording's, the cross-check `passed`, the first decision evaluated (no `first_decision_not_evaluated` warning, no application step) and no step whose click needs review | `recording_unreviewed` | `package_id`; the details carry `review` |
| No text of the package holds an absolute local path (a drive or network path, the state directory, the checkout) | `local_path` | `entry` the file, `json_pointer` the string |
| A root file adopt must change is in the canonical form ("The canonical form") | `root_file_not_canonical` | `entry` |
| The checkout's check tree is no link, the tool's own and free, and no directory beside it named as one a call makes the tree whole in is a link (below) | `check_tree_link`, `check_tree_unmarked`, `check_tree_busy` | `path` the check tree, or that directory |
| The tree as it would be written composes (below) | `rule_refused` | `entry` `schedule.json`, the composer's `json_pointer`, its refusal as the cause; or `package_id` when the package comes out unclassified |
| Every key the schedule as the check compiled it reads of the rule's own outcome is one the package's entry task declares (`scheduling_outcome.mappings`): the policy crate's own `referenced_outcome_keys`, which the Runtime runs, collects them from every composed rule's trigger and `feedback_stop` (the rule's own, given, derived or kept, and another rule's), every pool's observation and every activity target's measure. And every key an outcome predicate of the rule as it would be written reads of another task is one the package of that task declares. The Runtime refuses every dispatch of a task whose outcome is read for a key its package does not declare ([scheduling/README.md](scheduling/README.md)): the rule, or the task it reads and so the rule, would never run | `decision_invalid` | `path` the decision and `json_pointer` in it (`/trigger/…`, `/overrides/feedback_stop/…`) when the decision gave the member; otherwise `entry` `schedule.json` and `json_pointer` in the file as it would be written (`/rules/<i>/…`, `/pools/<i>/…`, `/activity/<i>/goals/<j>/metric/outcome_key`); the message names who reads, the rule, pool or activity profile by its id; the details carry `declared_outcome_keys` of the package of the task read and, for another task, its `task_id` and `package_id` |
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
SHA-256 of its resolved path. Adopt makes it whole, so that no crash leaves it unmarked: as a
directory of the call's own beside it, `<checkout>.new-<process>-<ms>-<count>/` (the process, the
call's time and a count of the process; a name another directory already holds is passed over for
the next count, so that no two calls, of one process or of two, share one), with the marker
`.adopt-check-tree` written, renamed into place; when another call put its tree there first (two
first calls at once), the call removes its own directory, which holds only the marker it wrote
(unless that call, which holds the tree, cleared it already), and goes on with the other's tree, so
neither is refused `check_tree_unmarked` (one may be refused `check_tree_busy`). A crash before
that rename leaves only the call's own directory beside the place, which no call reads. Adopt holds
the marker open and locked for the whole call, and at the start of each call, while it holds it,
clears every directory beside the place named so (`<checkout>.new-` and three numbers) that carries
the marker, which only a crash leaves there, and empties the tree of everything but the marker,
only when the marker is there, and never anything else: a directory named so without the marker
(a crash before the marker was written) is not the tool's and is left alone. Before anything is
cleared, the place itself and those directories are checked: a link there (a symbolic link, or on
Windows a junction) is refused `check_tree_link`, and nothing is cleared through it or where it
points. A directory at the place without the marker is not touched and the call is refused
`check_tree_unmarked`; a call while another adoption of the checkout with the same state directory
holds the marker is refused `check_tree_busy`. The rule "nothing is removed" holds for the checkout
and for a replaced pack directory's kept copy; the check tree and those directories alone are
cleared, by the tool, of what the tool laid there.

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
| `feedback_stop` | Never copied from another rule (a copied stop may never come true, and the task would be dispatched again at every cooldown): "this rule's own run ended with a key its package declares", `{kind: outcome, task_id: <rule>, outcome_key: <key>, comparison: eq, value: {type: boolean, value: true}}` for every key of a normal end the entry task declares (every mapping's but a `busy` one's and one with a `remedy`), under `any` when there are several (`package`): after one normal end inside the activity window the rule is not dispatched again in that window ([scheduling/README.md](scheduling/README.md): a stop is read only of a result of the current activity window); none when the package declares no such key, and the decision must state it. | yes, `overrides.feedback_stop`, on keys the package declares ("The checks") |
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
deleted in the checkout or of a kept copy. Every file adopt renames into place, or whose directory
it renames into place (the files of `packs/<package_id>.part/`, both root `.part` files, the check
tree's marker), is flushed to the disk (`sync_all`) before that rename, as the Runtime's own
writers of state do (fix round 4), so that a rename a crash keeps never names a file whose bytes
the crash lost:

- `packs/<package_id>/`, written as `packs/<package_id>.part/` and renamed: only `control.json` and
  `resources/**` of the package, each JSON file in the canonical form. With `--replace` the old
  directory is first moved whole to `<state>/adopt/<package_id>.old-<ms>/`, which the answer names
  in `kept`. When the new directory then cannot take its place (on Windows a file in use there
  makes the rename fail), the old directory is moved back by a rename, the new one stays as its
  `.part`, and the refusal says what happened; when moving it back fails too, it says where the old
  directory is.
- `schedule.json` (the rule appended or changed, the held entry, `requires`) and `maintenance.json`
  (an entry `{package_id, server, uses}`, or the package's entries removed, "One role"), each only
  when it changes: both written whole as `<name>.part` first, then renamed one after the other,
  first the file that gives the package its new role (`maintenance.json` for the class
  `maintenance`, `schedule.json` for the others), then the file that ends its old one, so that a
  call interrupted between the two renames leaves the `.part` of the file it did not rename, which
  the next call sees ("If a call was interrupted"). Before the first write, and again immediately
  before each rename, the file is read again and compared with the bytes the call computed from;
  when it changed (another adoption, which the MCP server allows at the same time, or a person),
  the call stops with `adopt_write_failed` at that file, renames nothing more and overwrites
  nothing of that work; its message says what to do: before anything was written, run adopt
  again; after the call put its new `packs/<package_id>` in place, the steps "to run it again"
  below. A root file with changes not committed is read and written as it is: several adoptions in
  a row before one commit are normal use.

A failed write is `adopt_write_failed`: `path` where it stopped and `paths` what it had written or
moved before, in order, the operating system's refusal as the cause when there is one; nothing is
removed in the checkout (a `.part` and a kept copy stay where they are).

**If a call was interrupted** (a crash, a stopped process) after it began writing, it leaves in the
checkout what it had written, in this order: `packs/<package_id>.part/` until it is renamed, then
its new `packs/<package_id>` in place, then `schedule.json.part` or `maintenance.json.part` until
each is renamed. When the call began, `packs/<package_id>` was absent or committed (adopt refuses a
pack directory with changes), so `git status -- packs/<package_id>` shows exactly what it left
there, and a replaced directory it had moved away is in the last commit and under the state
directory's `adopt/`. So after a call that put its new directory in place, `--replace` is refused
`pack_dirty` until `packs/<package_id>` is put back as it was (moved out of the checkout by hand
and, where it was committed, restored with the version control) or committed, and adopt without
`--replace` is refused `pack_exists`. The two are not alike across volumes (fix round 4). Put back
as it was, `--replace` needs no state directory on the checkout's volume: absent (that call adopted
the package first), it moves nothing; committed (that call replaced it), that call passed the
volume check with its `--state-dir`, which passes again. Committed as that call left it,
`--replace` moves it to its kept place under the state directory, which must lie on the checkout's
volume (`replace_across_volumes` otherwise), a check that call never ran when it began without the
directory; actinglab's default state directory (its session directory under the user's
application data) may lie on another volume, so give a `--state-dir` on the checkout's volume, and
with another `--state-dir` a recording's package by `--package` instead of `--record-id`. The
next call refuses as long as a `.part` is there (`pack_exists` for the package's,
`repository_invalid` for a root file's), and its message names each leftover, says what it tells
of that call and gives the steps below, each of which a later call accepts:

- `packs/<package_id>.part/` (whole, or cut off): that call wrote no root file. Move the `.part`
  out of the checkout by hand (adopt deletes nothing), then run adopt again as that call was run.
- A root file's `.part`: that call had put its `packs/<package_id>` in place before it wrote the
  first, writes every root `.part` before it renames the first and renames them one after the
  other, so a root file whose `.part` is left was not renamed and is as it was before that call. A
  `.part` is trusted as its file whole only when it reads as that root file (`schedule.json` by the
  schedule reader, `maintenance.json` as its list of entries) and is in the canonical form, its
  final newline included, as a finished write leaves it; a `.part` cut off by a crash ends before
  its last bracket or its final newline, so it is not, and the message names each `.part` trusted
  or not and why. A `.part` not trusted was being written when the call stopped, so that call had
  renamed no root file: every root `.part` is flushed to the disk before the first root rename
  ("What it writes"). When one root `.part` is left and trusted, the other root file is in one of
  three states, which the next call cannot tell apart: renamed by that call, not to change in it,
  or to change and its `.part` not written yet (the call stopped between its two writes); look
  whether that file is already as the call decided it (the package's rule, held entry or
  `maintenance.json` entry there, or gone where the call ended one).
  - To finish the adoption, where every root `.part` left is trusted and, for a single one, the
    other file is as the call decided it: rename each `.part` over its file. The adoption is then
    whole, its directory included; nothing more is run.
  - To run it again: move the root `.part` files out of the checkout; put `packs/<package_id>`
    back as it was, or commit it (then with a `--state-dir` on the checkout's volume, above); then
    run adopt with `--replace` and the decision that call was given (none when it was given none),
    which writes the package's one role from the root files as they are.
  - To undo it: move the root `.part` files out, put `packs/<package_id>` back as it was, and
    restore with the version control the root file that call renamed (to its last commit: a change
    not committed before that call is not in it).
- Nothing left (a call interrupted after it put `packs/<package_id>` in place and before it wrote
  its first root `.part`): the next call without `--replace` is refused `pack_exists` at the
  directory, its message naming what the root files hold of the package. When `schedule.json` and
  `maintenance.json` hold nothing of it (no rule or held entry, no entry for its server), the
  message tells two cases apart by what it reads (fix round 4), and each advice ends in a call the
  checks accept. `git status -- packs/<package_id>` tells whether the directory is committed (a
  change there was left by an interrupted adoption or by hand); when it shows a change, put the
  directory back as it was. Then, with it absent, adopt runs without `--replace` (a first
  adoption, with `--decide`); with it committed, adopt runs with `--replace` and `--decide`, which
  moves the committed directory to its kept place and so needs a `--state-dir` on the checkout's
  volume (above).
  - A root file adopt does not write gives the package a maintenance role (`pages.json`,
    `navigation.json` or `interrupts.json`, which the message names): the directory belongs to
    the pack by that file, whether or not an adoption was interrupted. The call accepts such a
    package with a decision of the class `held` (its hold beside that role) or
    `maintenance` (a rule by its trigger, or a `maintenance.json` entry by its `uses`), never
    `business`, which is refused at `/class` beside that role ("One role").
  - No root file names it: that call was interrupted before it wrote the root files, or the
    directory was put there by hand.

When a root file's `.part` was moved away without either, the two files may give the package two
roles as the composer reads them: a held entry in `schedule.json` beside its `maintenance.json`
entry ("One role"; a rule beside that entry is part of the package's one maintenance role, never a
second one, since the composer does not judge it). A call with a decision writes its one role
again and answers the entry it removes in `removed_entries`; a call without one is refused
`repository_invalid` at that entry. Only a call with `--replace` meets that check (without it the
entry is refused `pack_exists` first), and only after `pack_dirty` passed, so `packs/<package_id>`
is then committed or absent and nothing comes before (fix round 4): its message says to run adopt
again with `--replace`, the same `--state-dir` and the decision the interrupted call was given; or,
to undo it, to put `packs/<package_id>` back as it was before that call and restore both files
with the version control.

The repository's own items adopt does not write are named in `next` when the checkout holds them:
a regression case (`regression/<game>/`), the catalog section (`docs/PACKS.md`) and the census row
(`ours/coverage/business-v1.csv`).

## Repeating (`--replace`)

Without a decision the rule, the held entry and the maintenance entry are kept byte for byte, and
only the pack directory is replaced; every key the schedule reads of the kept rule's outcome is
checked against the new package's keys, and every key the kept rule reads of another task against
that task's package ("The checks"). A decision changes only the members it
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
`undeclared` or `follower`, as the composer counts the file's judged rules), `waits_for_value` and
`value_not_judged` (`because` `follower` or `manual_request`, and `text`; "A pending value"),
`warnings` (every warning of the check, the game's included, each once), `written` (in the order
it was renamed into place), `would_write`, `kept`, `check_tree` and `next`.

## The canonical form

The form `package fmt` writes: two-space indentation, one member per line, members in their order,
LF line ends and a final newline. A root file adopt must change and that is not in it is refused
(`root_file_not_canonical`), so that the diff of an adoption shows only what it changed: write it so
once, in a commit of its own.
