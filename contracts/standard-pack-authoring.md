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
answer never says more than is true, and a call that cannot finish fails before it writes. Its
answer says how far the game root it lays out makes the package ready ("The answer", Workflow #463
S3-5); whether the package is ready to be scheduled on an installation is another slice's.
Workflow #463 AC-1 (Runtime 0.12.71; [v012-register.md](v012-register.md) §1.461): with
`--record-id` it takes the recording's regression cases into the checkout too, each run before the
first write ("The cases"), and judges the recording by `record ready`'s one function ("One
judgement of the recording").

## The operation

```text
actinglab standard-pack adopt --repo <checkout> (--record-id <id> [--session-state-dir <dir>]
    | --package <path>) [--decide <decision.json> | --held-reason <text> [--held-reference <text>]]
    [--replace] [--state-dir <dir>]
```

The row `standard-pack adopt` of the operation table ([lab-operations.json](lab-operations.json),
module `authoring`, slice `SP-ADOPT`); over MCP the tool `ac_standard_pack_adopt`, served by a
`serve` child ([lab-two-entries.md](lab-two-entries.md)). Under the global `--dry-run` it runs every
check and writes nothing in the checkout ("`--dry-run`" below).

| Input | What it is |
|---|---|
| `--repo` | The game's working root in the source layout: `applications.json` (its game and servers), `maintenance.json` (a list, `[]` when none), `packs/` (one directory per package) and the root files the repository holds (`schedule.json`, `pages.json`, `navigation.json`, `interrupts.json`, `display-layout.json`, `loading.json`, `goals.json`, `notices/`). |
| `--record-id` | A stopped recording of the recordings' directory (Workflow #463 AD-1: `--session-state-dir`, or the state directory without it; "The recordings' directory"), `<recordings>/record-artifacts/<record>/lab/`: its `recording.json` and the package its last stop generated, `out/<package_id>.<zip or json>`; Workflow #463 AC-1: and the regression cases that stop wrote beside it, `out/<package_id>.cases/` ("The cases"). |
| `--package` | Any package instead: a content directory, a `.zip` or a `.json` container; it carries no regression case. |
| `--decide` | The decision document ("The decision document" below); a first adoption needs it or `--held-reason`. |
| `--held-reason` | The decision of the class `held` without a document: its reason ("A held decision by two flags" below). Not with `--decide`. |
| `--held-reference` | With `--held-reason` only: that decision's reference. |
| `--replace` | Adopt a package adopted before again ("Repeating" below). |
| `--state-dir` | The Lab's state directory, as the recording commands take it (default actinglab's session directory): it holds the recordings, the kept copies (`adopt/`) and the check trees (`adopt-check/`), and (Workflow #463 S3-12) the trial records adoption reads (`trials/`, "The trial and the two warnings"). With `--replace` it must lie on the checkout's volume, since the replaced directory is moved into it. Workflow #463 AD-1: the recordings are read from it only without `--session-state-dir`. |
| `--session-state-dir` | Workflow #463 AD-1 (since 0.12.72): with `--record-id` only, the directory the recording is read from, the recording commands' session state directory; adoption only reads it ("The recordings' directory"). |

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
The command line cannot give a text that is the word `true` or that begins with `--`: its flag
parser takes no such word as a flag's value (a value `true` reads as no value, and a word that
begins with `--` as the next flag), so such a call is refused `validation_failed` before anything
is read, the flag missing its value or the word refused as a flag of its own; such a text is given
in a decision document by `--decide`. Before anything is read or written, `--decide` beside either
flag is refused `validation_failed`
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
remove the origin and does not copy it to another rule. The model's writer is `acmodel bindings
apply` (Workflow #463 S3-12 corrected this sentence, which said that no writer of `model` exists):
a value binding of the repository's `ours/model/bindings.json` names the rule, and the writer
writes the value the model exports into its `fixed_value_milli` with `fixed_value_origin:
"model"`; a number written by hand is registered there as a hand entry with its `status` and its
`ref`, and carries no origin. The Lab never writes `model`; the provenance listing of
`standard-pack check` shows which values are the pack's and which the model's. A rule adopted
without a value names that file in `next` ("The answer").

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
| The recording is as its last stop left it: `actingcommand.lab-recording.v1`, its `record_id`, `stopped`, not marked again since (`amended_at_unix_ms` absent or null), its package there. Workflow #463 S3-17: after its schema and its `record_id`, its status by `record ready`'s one function is `packaged` ("One judgement of the recording"); Workflow #463 AC-1: a cases directory beside its package can be listed | `recording_not_current` | `path` of `recording.json`, `member`; for a status other than `packaged` no `member`, the details' `record_ready` that function's answer; `path` the cases directory or its entry |
| Workflow #463 AC-1 fix round 1: `record ready`'s one function judged the recording, or refused it only as one the Lab cannot read whole (`lab_recording_unreadable`, which the check of its own members above judges) | `recording_not_judged` | `path` of `recording.json`; that function's refusal as the cause |
| The package is admitted as `package preflight` admits it, and its `package_id` names one directory `packs/<package_id>` that no root file reserves | `package_invalid` | `path`, the loader's refusal as the cause; or `package_id` |
| Its game is `applications.json`'s and its server one of that file's | `package_mismatch` | `member` `game` or `server` |
| No `packs/<package_id>.part` is left ("If a call was interrupted"); not adopted before (its directory, its rule, its held entry, its `maintenance.json` entry), or `--replace`; the kept copy's place free. Workflow #463 AC-1, when the adoption takes cases ("The cases"), after `packs/<package_id>.part`: no `.part` of a case of the package is left in `regression/<game>/`; after the rest: no case of the package an earlier adoption took from a recording is there, or `--replace`; with `--replace` the kept place of those cases free | `pack_exists` | `entry` the directory or its `.part`, or `json_pointer` of the entry; `path` of a taken place; without `--replace` the message names what the root files hold of the package, and when `schedule.json` and `maintenance.json` hold nothing of it beside its directory, either names the root file adopt does not write that gives it a role and what the call accepts for it, or says that an adoption was interrupted before it wrote the root files ("If a call was interrupted"); Workflow #463 AC-1: `entry` the earlier case or the case's `.part` (`regression/<game>/…`) and `package_id`, `path` the cases' kept place |
| With `--replace`, no change below `packs/<package_id>` the version control does not hold (`git status --porcelain=v1 -z --untracked-files=all`, which writes nothing) | `pack_dirty` | `entry`, `count`; or the version control's failure as the cause |
| With `--replace`, the kept copy's place lies on the checkout's volume (the volume of the nearest existing part of each path, links resolved: the drive or network share on Windows, the device elsewhere; a volume mounted on a folder of another is not told apart on Windows), so that the replaced directory can be moved there | `replace_across_volumes` | `entry` the directory, `path` the kept place (Workflow #463 AC-1: or, for the cases' kept place, which must lie there too, `entry` the first earlier case); the message says to give a `--state-dir` on the checkout's volume and, for a recording of the default state directory, its package by `--package <state>/record-artifacts/<record>/lab/out/<package_id>.<ext>` instead of `--record-id` |
| The decision document | `decision_invalid` | `path`, `json_pointer` (`""` for a first adoption without one); no `path` for the decision of `--held-reason` ("A held decision by two flags") |
| Its value's origin ("The value's origin") | `value_origin_model` | `path`, `json_pointer` `/value/origin` |
| A class `business` beside a maintenance role a root file adopt does not write gives the package ("One role") | `decision_invalid` | `path`, `json_pointer` `/class` |
| A class `maintenance` by its `uses` alone beside the package's business rule, a rule the composer judges by its value ("One role") | `business_rule_in_the_way` | `entry` `schedule.json`, `json_pointer` `/rules/<i>`, `package_id`; the message names the rule's id |
| Without a decision, no held entry of the package in `schedule.json` beside its `maintenance.json` entry ("One role", "If a call was interrupted") | `repository_invalid` | `entry` `maintenance.json`, `json_pointer` the entry, `package_id` |
| A class other than `held` needs a recording checked offline (`review.recording_checked`, "The trial and the two warnings"): its entry task's provenance a recording's, the cross-check `passed`, the first decision evaluated (no `first_decision_not_evaluated` warning, no application step) and no step whose click needs review (Workflow #463 S3-17: unchanged, since `record ready`'s function checks a packaged recording no further, "One judgement of the recording") | `recording_unreviewed` | `package_id`; the details carry `review`; the message says that the recording was not checked offline |
| No text of the package holds an absolute local path (a drive or network path, the state directory, the checkout) | `local_path` | `entry` the file, `json_pointer` the string |
| A root file adopt must change is in the canonical form ("The canonical form") | `root_file_not_canonical` | `entry` |
| The checkout's check tree is no link, the tool's own and free, and no directory beside it named as one a call makes the tree whole in is a link (below) | `check_tree_link`, `check_tree_unmarked`, `check_tree_busy` | `path` the check tree, or that directory |
| The tree as it would be written composes (below) | `rule_refused` | `entry` `schedule.json`, the composer's `json_pointer`, its refusal as the cause; or `package_id` when the package comes out unclassified |
| Every key the schedule as the check compiled it reads of the rule's own outcome is one the package's entry task declares (`scheduling_outcome.mappings`): the policy crate's own `referenced_outcome_keys`, which the Runtime runs, collects them from every composed rule's trigger and `feedback_stop` (the rule's own, given, derived or kept, and another rule's), every pool's observation and every activity target's measure. And every key an outcome predicate of the rule as it would be written reads of another task is one the package of that task declares. The Runtime refuses every dispatch of a task whose outcome is read for a key its package does not declare ([scheduling/README.md](scheduling/README.md)): the rule, or the task it reads and so the rule, would never run | `decision_invalid` | `path` the decision and `json_pointer` in it (`/trigger/…`, `/overrides/feedback_stop/…`) when the decision gave the member; otherwise `entry` `schedule.json` and `json_pointer` in the file as it would be written (`/rules/<i>/…`, `/pools/<i>/…`, `/activity/<i>/goals/<j>/metric/outcome_key`); the message names who reads, the rule, pool or activity profile by its id; the details carry `declared_outcome_keys` of the package of the task read and, for another task, its `task_id` and `package_id` |
| A business rule's value ("A pending value") | `decision_invalid` | `path`, `json_pointer` `/value` |
| Workflow #463 AC-1, when the adoption takes cases ("The cases"): each case's directory `<package_id>.<case>` is one file name: the case's name holds no dot, the whole is one path segment, and with its `.part` it is at most 255 bytes | `case_name_invalid` | `path` the case in the recording, `package_id` |
| Workflow #463 AC-1: each case's directory `regression/<game>/<package_id>.<case>` is free (names compared in ASCII lower case) or a case of the earlier set | `case_place_taken` | `entry` that directory, `package_id` |
| Workflow #463 AC-1: no case fails on the package as this adoption writes it, each run by the one runner | `case_failed` | `path` the recording's cases directory, `count` the failing cases, `package_id`; fix round 1: the details list the cases that did not pass as `resource regression` lists them ("The cases"): `failed_total`, `listed`, `cases` |

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
- Workflow #463 AC-1, last, after both root files are renamed, when the adoption takes cases ("The
  cases"): with `--replace` first the cases of the package an earlier adoption took from a
  recording, moved one directory at a time into `<state>/adopt/<package_id>.cases.old-<ms>/` (the
  answer's `cases.kept`); then each case of the recording, in the order of its name, written whole
  as `regression/<game>/<package_id>.<case>.part/` (its `case.json` and its scene, each flushed to
  the disk) and renamed into place. Each is in `written` after the root files, in that order.

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
with another `--state-dir` a recording's own directory by `--session-state-dir` (Workflow #463
AD-1, "The recordings' directory"), or its package by `--package` instead of `--record-id`. The
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
    run adopt with `--replace` and the decision that call was given (by `--decide`, or by
    `--held-reason` and its `--held-reference`; none when it was given none), which writes the
    package's one role from the root files as they are.
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
  adoption, with `--decide` or `--held-reason`); with it committed, adopt runs with `--replace`
  and `--decide` or `--held-reason`, which moves the committed directory to its kept place and so
  needs a `--state-dir` on the checkout's volume (above).
  - A root file adopt does not write gives the package a maintenance role (`pages.json`,
    `navigation.json` or `interrupts.json`, which the message names): the directory belongs to
    the pack by that file, whether or not an adoption was interrupted. The call accepts such a
    package with a decision of the class `held` (its hold beside that role, by `--held-reason` or
    `--decide`) or
    `maintenance` (a rule by its trigger, or a `maintenance.json` entry by its `uses`), never
    `business`, which is refused at `/class` beside that role ("One role").
  - No root file names it: that call was interrupted before it wrote the root files, or the
    directory was put there by hand.
- Workflow #463 AC-1: what a call that takes cases leaves while it writes them. It writes them
  last, after it put `packs/<package_id>` in place and renamed both root files, so the adoption is
  whole but for its cases: the package's directory and its one role are there, as that call decided
  them. Of the cases it leaves, in `regression/<game>/`: with `--replace`, the earlier cases of the
  package it had not moved yet to `<state>/adopt/<package_id>.cases.old-<ms>/` (those it moved are
  there); the cases of the recording it had renamed into place, in the order of their names; at
  most one `regression/<game>/<package_id>.<case>.part/`, whole or cut off. The next call that takes
  cases refuses as long as such a `.part` is there (`pack_exists` at it, `package_id`, before the
  earlier adoption is looked at), its message saying what that call had done and the steps below;
  a call that takes none (`none_found`, `package_path`) refuses nothing for it and warns
  `adopt_cases_left` naming it (fix round 1, "The cases").
  Without a `.part` it meets what any call that put its directory in place leaves: without
  `--replace` `pack_exists` (at `packs/<package_id>`, or at an earlier case of the package when the
  directory and the root files hold nothing of it); with `--replace` `pack_dirty` until
  `packs/<package_id>` is put back as it was or committed (above). Each of these ends in a call the
  checks accept:
  - To run it again: move the case's `.part`, if any, out of the checkout by hand; put
    `packs/<package_id>` back as it was, or commit it with what that call wrote (then with a
    `--state-dir` on the checkout's volume, above); then run adopt with `--replace` and the
    decision that call was given (none when it was given none, the root files then kept byte for
    byte). That call moves every case of the package taken from a recording that is in
    `regression/<game>/` (the earlier set's left there and those the interrupted call renamed in)
    to its own kept place and writes the recording's whole set.
  - To undo it: move the case's `.part` and the case directories that call renamed in out of the
    checkout; move the earlier cases it had moved back from `<state>/adopt/<package_id>.cases.old-<ms>/`
    into `regression/<game>/`; then undo the package and the root files as above.

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
a regression case (`regression/<game>/`; Workflow #463 AC-1: only when the adoption took none, "The
cases"), the catalog section (`docs/PACKS.md`) and the census row
(`ours/coverage/business-v1.csv`). Workflow #463 S3-12: and, whether the checkout holds it or not,
`ours/model/bindings.json` while the adopted rule has no value yet ("The trial and the two
warnings").

## Repeating (`--replace`)

Without a decision the rule, the held entry and the maintenance entry are kept byte for byte, and
only the pack directory is replaced; every key the schedule reads of the kept rule's outcome is
checked against the new package's keys, and every key the kept rule reads of another task against
that task's package ("The checks"). A decision changes only the members it
names, and decides the package's one role ("One role"; a decision of another class than `held`
lifts a hold, so the rule is composed again); a value the model wrote is kept ("The value's
origin"). A derived member the derivation would now give otherwise is answered in `derived_drift`
with the written and the derived value, and never written. Workflow #463 AC-1: when the adoption
takes cases, the cases an earlier adoption of the package took from a recording are replaced as a
set ("The cases").

## `--dry-run`

Every check runs, the check tree under `<state>/adopt-check/` included; the answer is `validated`
with `written` empty and `would_write` what an adoption would write, and its `readiness` judged and
its `review.trial` read as an adoption's are ("The answer"); nothing is written in the repository
and no copy is kept
([actinglab-dry-run.md](actinglab-dry-run.md)). Workflow #463 AC-1: the cases are still run, each
checked as an adoption checks it; `would_write` lists their directories after the root files, as
does `cases.would_write`, and `cases.replaced` the earlier cases an adoption would move.

## The answer

`status` (`adopted` or `validated`), `dry_run`, `repo`, `game`, `server`, `package_id`,
`entry_task_id`, `class`, `replaced`, `review` (`recorded`, `record_id`, `cross_check`,
`first_decision_evaluated`, `steps_needing_review`, `recording_checked`, and `trial` or
`trial_not_read`; "The trial and the two warnings" below; Workflow #463 S3-17: `ready_not_read`
`{outcome, text}` when `record ready`'s function did not read the recording, "One judgement of the
recording"), `rule`, `held` and
`maintenance` as written, `removed_entries` and `replaced_entries` ("One role"),
`requires_added`, `given`, `derived` (each `member`, `value`, `from` and, where it says more,
`how`), `derived_drift`, `model_kept`, `coverage` (a business rule: `complete`, `pending`,
`undeclared` or `follower`, as the composer counts the file's judged rules), `waits_for_value` and
`value_not_judged` (`because` `follower` or `manual_request`, and `text`; "A pending value"),
`warnings` (every warning of the check, the game's included, each once, then adoption's own two,
below, and, Workflow #463 AC-1 fix round 1, `adopt_cases_left`, "The cases"; Workflow #463 AD-1:
but each that names another package of the root), `other_packages_warnings` (Workflow #463 AD-1:
those, counted by code, "The warnings of other packages"; fix round 1: absent beside
`adopt_warnings_not_split`), `written` (in the order it was renamed into place), `would_write`, `kept`, `check_tree`,
`readiness` or `readiness_not_judged` (below), `next` (each item `{path}`; Workflow #463 S3-12:
`ours/model/bindings.json` with `fixed_value` too, below) and (Workflow #463 AC-1) `cases` ("The
cases").

**The trial and the two warnings** (Workflow #463 S3-12, Runtime 0.12.68;
[v012-register.md](v012-register.md) §1.445). Two things are told apart by name: that the
recording was checked offline, and that the content was tried on a real application.

- `review.recording_checked` (named `run_through` before this slice; no member of that name is
  left) is the offline review of ruling 2 on #464, its rule unchanged: the entry task's provenance
  a recording's, the cross-check `passed`, the first decision evaluated, no step whose click needs
  review. It says nothing of a run on a device. A class other than `held` still needs it
  (`recording_unreviewed`, "The checks").
- `review.trial` is the latest trial of the content this adoption writes, as
  [standard-pack-trial.md](standard-pack-trial.md) records trials: the records of the package under
  the state directory the adoption is given (`--state-dir`, default actinglab's session directory,
  the trial's own default), `<state>/trials/<package_id>/`, read by the trial's own reader
  (`trial_records`, in the order the trials ended). The content is named as a trial names it: the
  first 12 hex digits of the content hash (`content_hash_prefix`, the contract crate's
  `content_directory_digest` of each file's SHA-256, one function the trial calls too) of the files
  the adoption writes into `packs/<package_id>/` (or, under `--dry-run`, would write), so a trial of
  that directory, which is what a trial tries, records the same prefix. It is the trial's one
  summary of a content's trials (the train of S3-12 and S3-14,
  [standard-pack-trial.md](standard-pack-trial.md), "What the records say of one content"), which
  `standard-pack status` answers too: `state` (`passed`, `failed` or `none`, the vocabulary
  `trial_state`), `content_hash_prefix`, `record` (the latest record of this content:
  `authorization`, `at_unix_ms`, `request_id`, `end` as the record keeps it, and `matched` when the
  trial named the end it expected; absent with `none`), `passed_on_record` (whether any record of
  this content passed) and `other_content` (a record of another content of the same package id is
  on record; it counts for nothing). Whether a trial passed is what
  [standard-pack-trial.md](standard-pack-trial.md), "A trial that passed", says; this answer
  defines nothing of its own. A record of the directory as an earlier adoption wrote it, before a
  change (a new recording, a repaired package), is another content.
- When the reader refuses the package's records (a file of a record's name that is no record,
  `trial_record_unreadable` with its word, or the directory not listed), `review.trial` is absent
  and `review.trial_not_read` carries `outcome`, the reader's refusal with its code, and `text`, the
  same in words (the trial's one shape, which the status row answers too); the adoption is the same
  without it, as `readiness_not_judged` is, and `not_tried` is not warned, since whether this
  content was tried is not known.
- Two warnings, appended to `warnings`, refuse nothing and change nothing of what is written:
  `enabling_needs_authorization` (`package_id`) when there is a decision and its class runs
  (`business` or `maintenance`), which lifts the package's hold where it had one: adoption enables
  nothing, and the placement that does asks for `--authorization` and for `--enables` naming the
  package when it enters scheduling or takes a new supporting role there; a held decision, and an
  adoption without a decision, which keeps the package's role, do not warn it. `not_tried`
  (`package_id`) when no record of this content passed (`trial.passed_on_record` false: an earlier
  passed record of the same content keeps it away even when a later one failed, and `trial` then
  reads `state` `failed` with `passed_on_record` `true`).
- `next` names `ours/model/bindings.json` (the model's `acmodel.bindings.v1`, relative to
  `--repo`) when the adopted rule has no value yet: it writes no `fixed_value_milli` and
  `waits_for_value` is answered. Its `fixed_value` holds the rule's id and the two ways a value gets
  there, each the entry as that file writes it, the texts adopt cannot know left blank:
  `value_binding` `{value: "", file: "schedule.json", rule_id, path: "/fixed_value_milli"}`, an
  entry of `bindings` whose `value` names a value of the model's `exports.json` and which the
  model's writer `acmodel bindings apply` writes into the rule with `fixed_value_origin: "model"`;
  and `hand_entry` `{rule_id, status: "", ref: ""}`, an entry of `hand` that registers a number
  written by hand into the rule's `fixed_value_milli` (adopt `--replace --decide` with
  `{fixed_value_milli, origin: "pack"}`), how it was had and where it comes from. It is named
  whenever `waits_for_value` is answered and the rule writes no fixed value: so also for a rule
  judged by its `produces` and `consumes` when an entry's amount is `"pending"` or its pool's
  valuation is `"pending"` or missing ("A pending value"). A rule with its value, a rule whose
  every `produces` and `consumes` entry has its amount on a valued pool, a follower of a chain, a
  manual request and a package without a rule name nothing of it.

Nothing here starts a trial, lifts a hold or enables a package: a passed trial is not an
authorization, and a trial is the trial's own act under the owner's authorization
([standard-pack-trial.md](standard-pack-trial.md), "Never by itself").

**The adopted pack's readiness** (Workflow #463 S3-5, Runtime 0.12.63;
[v012-register.md](v012-register.md) §1.412). `readiness` is the adopted pack's row of the
readiness of the game root the check tree laid out,
`<state>/adopt-check/<checkout>/installed/<game>/`
([standard-pack-schedule.md](standard-pack-schedule.md), "Readiness"): the row `standard-pack ready
--root` answers for that pack on that directory, member for member. It is the answer of that one
function, called as it is, on the tree as adopt composed it, after every check passed, and by the
same path under `--dry-run`; a
refused adoption answers none. No installation is handed to it, so the five items an installation
tells are `not_judged` (`installation_not_read`), I3 is information, and the pack answers
`ready_to_place` at best, never `dispatchable`. So a package adopted as held answers `waiting` with
S2 `classified` failing `held`; a business rule with `"pending"` alone, `waiting` with S3 `valued`
failing `value_undeclared` (the composer counts a rule with no fixed value and no `produces` or
`consumes` entry undeclared; readiness's `value_pending` is a rule that reads an entry or a pool
with the pending marker, "A pending value"); a business rule with its value, `ready_to_place` when the other items of the game
root hold too; a package adopted as maintenance by its `uses` alone, no rule naming it,
`not_scheduled` with that role in `roles`. When the function refuses that root, or answers no row
of the pack, the answer carries `readiness_not_judged` instead: `outcome`, the function's refusal
with its code as the function answered it, or, when the function answered and the pack's row is
not in its answer, adoption's own `package_invalid` (the pack is no package of the bundle); and
`text`, the same in words. Nothing else of the answer changes by it, and the call does not fail by
it.

## The canonical form

The form `package fmt` writes: two-space indentation, one member per line, members in their order,
LF line ends and a final newline. A root file adopt must change and that is not in it is refused
(`root_file_not_canonical`), so that the diff of an adoption shows only what it changed: write it so
once, in a commit of its own.

## The cases

Workflow #463 AC-1 (Runtime 0.12.71; [v012-register.md](v012-register.md) §1.461; the
coordinator's work order on board #463, the rules here are the coordinator's). Since #463 S2-10 a
stopped recording has, beside its package, the regression cases of that package
([lab-recording.md](lab-recording.md), "Regression cases written with the package";
[resource-regression-case.md](resource-regression-case.md)), each run on the package before it was
written. Adoption takes them into the checkout.

- **Which.** With `--record-id`: every entry of the recording's `out/<package_id>.cases/`, as its
  last stop left it, each a case's directory named by the case's id, in the order of the names
  (`cases.state` `taken`). A recording stopped by a build before S2-10 has no such directory: the
  adoption is as before, `cases.state` `none_found`, and `next` names the regression case as
  before. A cases directory that cannot be listed is `recording_not_current` at it. With
  `--package` no case is read: `cases.state` `package_path`, the adoption as before.
- **Where.** Each case becomes the directory `regression/<game>/<package_id>.<case>/` of the
  checkout (`<game>` the game of `applications.json`, which is the package's), holding its
  `case.json` and the scene its `case.json` names, byte for byte as the recording holds them; the
  case's own `id` and `package_id` are not rewritten. The name is one file name: the package id is
  one path segment that no root name reserves (adoption's own check), and the case's name holds
  no dot (`record stop` names a case `step_<nn>`, `final` or `ending_<outcome_key>`, an outcome
  key matching `^[a-z0-9][a-z0-9_]{0,127}$`), so `<package_id>.<case>` neither ends with a dot or
  a space nor begins with `-`, holds no separator or reserved character, keeps the package id's
  first dotted part (no device name), and tells its package and its case apart (the case is what
  follows the last dot). Its length is not bounded by the two grammars together (a package id may
  have 256 bytes, an ending's case 135), so a name whose `.part` would be longer than 255 bytes, a
  file name's most, is refused before the first write (`case_name_invalid`), as is a case's name
  with a dot or one that is not one path segment; nothing is escaped.
- **Checked before the first write**, after every check above: each destination is free (names
  compared in ASCII lower case, as a file system that does not tell cases apart compares them) or
  a case of the earlier set (below); otherwise `case_place_taken` at it. Then each case is run by
  the one runner ([resource-regression-case.md](resource-regression-case.md), "The runner") on the
  package as this adoption writes it, the check tree's `packs-root/<package_id>/` (the very files
  it writes into `packs/<package_id>/`), admitted once by the runner (`RegressionPackage::load`;
  adoption's own admission hands it no admitted package), each case read by the one reader and its
  scene read as `resource regression` reads a checkout's case (fix round 1: by its one reading of
  a case directory, the one function both call). A case whose verdict is `failed` refuses the
  adoption, nothing written: `case_failed`, its details listing the cases that did not pass as
  `resource regression` lists them, by its one rule (fix round 1;
  [resource-regression-case.md](resource-regression-case.md), "The operation"): `failed_total`, how
  many did not pass (`failed` and `partial`); `cases`, the rows of the first of them, the failed
  ones first, each group in the order of their names, at most that operation's bound (10), each
  `{path, id, package_id, verdict, checks, warnings?}` with `path` the directory
  `regression/<game>/<package_id>.<case>` it would be written to and its `checks` those that did
  not pass, without their `detail`; `listed`, how many rows are given; the message names the first
  failing case. `partial` and `not_run` cases are taken and named in `cases.not_proven` (`{case,
  verdict, reasons}`, the runner's own words: `regression_verdict` and `regression_not_run`).
- **Repeating.** A case is the earlier adoption's, which a later adoption of the package replaces,
  when it is a directory (no link) of `regression/<game>/` whose `case.json` is an object naming
  the package's `package_id` and whose `source` is a recording's in the form `record stop` writes:
  an object of `record_id` (a text) and exactly one of `step` (a whole number), `final` (`true`) and
  `ending` (a text), nothing else; the member `source.record_id` in that form tells it. Without
  `--replace` such a case is refused `pack_exists`, as an adopted directory or rule is (at its
  place among the checks, after them). With `--replace` they are replaced as a set: moved, each
  by its name, into `<state>/adopt/<package_id>.cases.old-<ms>/` beside the replaced pack
  directory's kept copy (`<ms>` the call's time), which must be free (`pack_exists`, `path`) and on
  the checkout's volume (`replace_across_volumes`), then the new set is written. A case of the
  package written by hand (another `source`, or none) is never moved or rewritten. Nothing is
  deleted in the checkout. Fix round 1: an adoption that takes no case (`none_found`,
  `package_path`, with `--replace` or without) moves none and runs none, so the cases of the
  package an earlier adoption took from a recording stay, though they may not match the content
  it writes, and so does a case's `.part` an interrupted adoption left: it warns
  `adopt_cases_left` (`package_id`, `paths` those directories relative to `--repo` in the order of
  their names), and moves and removes nothing.
- **Written last** ("What it writes"; what an interrupted call leaves, "If a call was
  interrupted"). A `.part` of a case of the package left in `regression/<game>/`
  (`<package_id>.<case>.part`) is refused `pack_exists` at it before the earlier adoption is looked
  at.
- **The answer**, `cases`: `state`; `from` (with `taken`), the recording's cases directory;
  `written` the directories written, relative to `--repo`, in the order they were renamed into
  place (also in `written`); `would_write` under `--dry-run` (also in `would_write`); `replaced` the
  earlier cases moved (under `--dry-run`: to be moved); `kept` where they were moved; `not_proven`.
  `next` names the regression case only when none was taken.
- **Not changed.** What adoption writes of the package and the root files, byte for byte; its
  checks and their order before these; the kept copies; the runner and the reader; the recording's
  files, which adoption reads and never writes; `resource verify` and `standard-pack build`, which
  do not run cases.

## One judgement of the recording

Workflow #463 S3-17 (Runtime 0.12.71; [v012-register.md](v012-register.md) §1.461). With
`--record-id`, after the schema and the `record_id` of `recording.json`, adoption judges the
recording by `record ready`'s one function ([lab-recording.md](lab-recording.md), "What a recording
still lacks: `record ready`"; `recording_api::ready_of`), called on the recording by its id,
whatever instance it is of, with no option given. A status other than `packaged` is refused
`recording_not_current`, its details' `record_ready` that function's answer (`status`, `missing`
each item as it gives it, `missing_total`, `warnings`, `checks`, `checks_after`, `sources`), so an
agent reads one vocabulary in both commands. What adoption accepts and refuses does not move by it,
and where the function and the old check of the recording's members differ, the old check stays
beside it for that difference only:

- `recording_not_current`: `packaged` is stopped, its package written and not marked again since,
  which the old check's `status`, `amended_at_unix_ms` and `artifact` members ask, so for a
  recording the function reads the two refuse the same recordings, but for these. A packaged
  recording whose artifact names its package by no `package_id` of one path segment or no
  container `zip` or `json`, or whose package's file is not there, is `packaged` for the function
  and refused by the old check of its artifact, kept (`member` `artifact`). A `recording.json` the
  function cannot read (no whole `actingcommand.lab-recording.v1` as the Lab writes it: made by
  hand, or damaged) is judged by the old check of its `status` and `amended_at_unix_ms`, kept, the
  function's refusal the cause of a refusal and, when they pass, said as `review.ready_not_read`
  (`{outcome, text}`). Fix round 1 (the coordinator's ruling): that is the one refusal of the
  function the old check judges, told by the word the Lab's refusal carries
  (`lab_recording_unreadable`, [lab-recording.md](lab-recording.md)). A failure on the Lab's side
  is never a pass: every other refusal of the function (the state not read, the recording's
  identity not its own, `lab_recording_missing` or `lab_recording_mismatch`, its answer not
  serialized) refuses the adoption `recording_not_judged`, `path` the `recording.json`, that
  refusal its cause, nothing written.
- `recording_unreviewed` is unchanged and whole: the function checks a packaged recording no
  further (`missing` empty), so it refuses none of the packaged recordings the review refuses (its
  cross-check not `passed`, its first decision not evaluated, a step needing review), and a package
  given by `--package` has no recording. `review.recording_checked` is what it was.

`standard-pack status` reads no recording's state, and is not changed.

## Codes and words of the cases and of the judgement

| Code or word | What it says | Refuses, warns or informs | Exit: actinglab's own line; the `--via` line; MCP |
|---|---|---|---|
| `adopt_refused` `case_failed` | a case of the recording fails on the package as this adoption writes it | refuses | 1; 2; a refusal |
| `adopt_refused` `case_place_taken` | a case's destination is there and is no case of the earlier set | refuses | 1; 2; a refusal |
| `adopt_refused` `case_name_invalid` | a case's name gives no directory `<package_id>.<case>` of one file name | refuses | 1; 2; a refusal |
| `adopt_refused` `pack_exists` (new causes) | an earlier adoption's case of the package without `--replace`; a case's `.part` left; the cases' kept place taken | refuses | 1; 2; a refusal |
| `adopt_refused` `replace_across_volumes` (new cause) | the cases' kept place on another volume | refuses | 1; 2; a refusal |
| `adopt_refused` `recording_not_current` (new cause) | `record ready`'s status is not `packaged`; or the cases directory cannot be listed | refuses | 1; 2; a refusal |
| `adopt_refused` `recording_not_judged` (fix round 1) | `record ready`'s function refused the recording otherwise than as one the Lab cannot read whole | refuses | 1; 2; a refusal |
| `adopt_cases_left` (fix round 1) | an adoption that took no case leaves cases of the package in `regression/<game>/`, an earlier adoption's from a recording or a case's `.part` | warns | 0; 0; the answer's `warnings` |
| `adopt_cases` `taken` | the recording's cases were read | informs | 0; 0; the answer |
| `adopt_cases` `none_found` | the recording's last stop wrote no cases directory | informs | 0; 0; the answer |
| `adopt_cases` `package_path` | the package was given by its path, which carries no case | informs | 0; 0; the answer |
| `regression_verdict` `partial`, `not_run`; `regression_not_run` words | a case taken that the runner did not prove whole, in its own words | informs | 0; 0; the answer |

No decision reads a code by name: adoption decides by the runner's verdict word and by the
judgement's status word; fix round 1: the Lab tells a recording it cannot read whole from its other
refusals by the word its refusal carries (`reason` `lab_recording_unreadable`), never by its code.

## The recordings' directory

Workflow #463 AD-1 (Runtime 0.12.72; [v012-register.md](v012-register.md) §1.464; the
coordinator's work order on board #463, after what the channel that makes packages observed on
0.12.70). A recording lives in the recording commands' session state directory, which may lie on
another volume than the resource repository's checkout, and `--replace` moves the replaced pack
directory and case set into the state directory, which must lie on the checkout's volume
(`replace_across_volumes`). Before this slice adopt read the recording from its state directory, so
the two could not both hold, and an adoption by `--package` takes no case and judges no recording.

What the state directory is to each part:

| Part | Directory |
|---|---|
| The recording by its id, `record-artifacts/<record>/lab/` (`recording.json`, `out/<package_id>.<zip or json>`, `out/<package_id>.cases/`), and `record ready`'s judgement of it | the recordings' directory: `--session-state-dir`, or the state directory without it |
| The check tree, `adopt-check/<checkout>/` | the state directory |
| The kept copies, `adopt/<package_id>.old-<ms>/` and `adopt/<package_id>.cases.old-<ms>/` | the state directory |
| The trial records, `trials/<package_id>/` (`review.trial`) | the state directory |

- `--session-state-dir <dir>` is named after the recording commands' own word for that directory,
  their session state directory (`ACTINGLAB_SESSION_STATE_DIR`; each `record` command takes it as
  its `--state-dir`). Over MCP its parameter `session_state_dir` is a path: a relative one is
  refused `not_absolute`, as every path of the table is.
- With it, `--record-id` finds its recording there, and "The cases" and "One judgement of the
  recording" hold whole, as when both are one directory: the cases are read from the recording,
  run on the check tree's copy of the package as it is about to be written, and written last; the
  recording is judged by `record ready`'s function on that directory; with `--replace` the earlier
  pack directory and case set are moved under the state directory. Nothing is written in the
  recordings' directory; its path is one of the local paths a package's text may not hold, as the
  state directory's is.
- Without it, every byte and every refusal is as before: the recording is read from the state
  directory, and one that is not under it is not found (`recording_not_current` at its
  `recording.json`).
- With `--package` it has no meaning, since that package is read from its path: the line is
  refused before anything is read, `validation_failed` naming `--session-state-dir`,
  `argument_problem` `conflicting`, as `--held-reference` without `--held-reason` is.
- `standard-pack status` and `standard-pack trial` find no recording by its id (their state
  directory is where the trial records are read and written) and are not changed.
- Fix round 1 (the trial's records, older than this slice): `review.trial` is read from the trial
  records under the state directory adoption is given, `<state>/trials/<package_id>/`, and
  `standard-pack trial` writes its record under its own `--state-dir`, actinglab's session
  directory when none is given. So where adoption is given a `--state-dir` of its own (one on the
  checkout's volume, beside `--session-state-dir`), `standard-pack trial` and `standard-pack
  status` of the package are given that same `--state-dir`: a trial recorded under another
  directory is not read there, and the adoption answers `review.trial` `none` and warns
  `not_tried` for a package that was tried. Where a record is read or written is not changed.
  At deploy that `--state-dir` lies outside the checkout: `standard-pack trial` refuses a state
  directory that is or lies inside a repository (`trial_state_dir_refused`, `trial_state_place`
  `repository`; [standard-pack-trial.md](standard-pack-trial.md), "The record").
- The advice of `replace_across_volumes`, and of the steps after an interrupted call, names
  `--session-state-dir` beside `--package`.

## The warnings of other packages

Workflow #463 AD-1. The check lays out and checks the whole game root, so its warnings are of every
package of the root, and the answer carried every other package's too (some eighty on production),
past which an agent read to find its own package's.

- A warning names a package as a readiness row reads it ("The adopted pack's readiness";
  [standard-pack-schedule.md](standard-pack-schedule.md), Workflow #463 S3-6; one function): by
  its `package_id` when that is a package of the check tree's game root, else by a `path` that is
  a pack's directory or lies under it; otherwise it names no package.
- `warnings` holds, whole and in their order, each warning that names the adopted package or no
  package of the root (a root-level finding; a rule's warning that names only its `task_id`),
  adoption's own warnings among them.
- `other_packages_warnings` `{total, counts}` holds those that name another package: `counts` how
  many there are of each code, by the code, and `total` how many in all. None is dropped. The
  check tree is kept until the next call on the checkout. Its warnings are of two commands ("The
  checks"): `standard-pack check --root` on its `installed/<game>/` lists whole the warnings of
  that check; packaging's own warnings, which `package bundle` answers as it lays the bundle out
  and which `standard-pack check` does not list again (`recognition_pack_undeclared` among them),
  are answered whole in the `warnings` of `package bundle` (`actinglab package bundle`) of the
  checkout's packs and root files as the check tree lays them out.
- `--dry-run` answers the same two members. No check changes: what refused before refuses, and
  what warned before is answered whole or counted.
- Fix round 1 (the coordinator's ruling after the independent review): the reading of the check
  tree's game root that tells which package each warning names is the readiness's own reading of
  it, after every check passed. When it refuses, the adoption is not refused, as the readiness is
  not by the same failure ("The adopted pack's readiness", `readiness_not_judged`): every warning
  is answered whole in `warnings`, in its order, then one more, `adopt_warnings_not_split` (`path`
  that `installed/<game>/`, the reader's refusal as its cause), and `other_packages_warnings` is
  absent. The adoption writes, or under `--dry-run` validates, as without it.

`other_packages_warnings`, `total` and `counts` are members of the answer. Fix round 1 adds one
code, a warning: `adopt_warnings_not_split` (resource-tooling, `path`), which refuses nothing (exit
0 on actinglab's line, in `warnings`). The readiness's word for what it did not judge is not taken
for it: the readiness says it in a member of its own (`readiness_not_judged`) and in
`package_readiness_unjudged`, an information of a readiness item of a pack, neither a warning of the
adoption's answer about its warnings. No decision reads a code by name: the warnings are split, or
not, by whether that reading answered.
