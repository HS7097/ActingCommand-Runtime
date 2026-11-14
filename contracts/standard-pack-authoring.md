# Standard-pack authoring: `standard-pack adopt`

Workflow #464, slice P1 (Runtime 0.12.60; [v012-register.md](v012-register.md) §1.370). A
recording ends in a package (`record stop`, [lab-recording.md](lab-recording.md)); before this
slice a person copied it into a resource repository by hand and wrote its rule into
`schedule.json` member by member. `actinglab standard-pack adopt` does it in one operation: it
makes the package a member of its game's standard pack in the repository's checkout, with a rule
the Runtime's composer accepts ([standard-pack-schedule.md](standard-pack-schedule.md)). It writes
only inside the checkout it is given and under the Lab's own state directory, runs every check
before its first write, deletes nothing, and answers every member it derives with where the value
came from. Whether a package is ready to be scheduled on an installation is another slice's.

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
| `--state-dir` | The Lab's state directory, as the recording commands take it (default actinglab's session directory): it holds the recordings, the kept copies (`adopt/`) and the check trees (`adopt-check/`). |

## The decision document

`actingcommand.pack-decision.v1` holds only what the Lab cannot know; every other member of the
rule is derived ("What is derived"). A member it does not list is refused at its pointer.

| Member | Value |
|---|---|
| `schema_version` | `actingcommand.pack-decision.v1` |
| `class` | `business` (a rule judged by its value), `maintenance` (a rule declaring `"maintenance": true`, or a `maintenance.json` entry by its `uses`) or `held` (a `held` entry: kept from scheduling, with its reason). |
| `trigger` | The rule's trigger, a predicate object as `schedule.json` writes it. A new rule needs it. |
| `value` | `"pending"`, or `{fixed_value_milli, origin}`: a whole number of at least 1, `origin` absent or `pack` ("The value's origin"). A new business rule needs it. |
| `consumes`, `produces` | Lists of `{pool_id, amount or expected_amount_milli, observation_source?, confidence_milli?}`, each `pool_id` a pool `schedule.json` declares; the direction and the evidence members not given are derived. |
| `limited`, `realization` | `true` when written: the marks of [standard-pack-schedule.md](standard-pack-schedule.md). |
| `uses` | For the class `maintenance` without a trigger: `startup`, `return_home` or `recognition`, or a list of them; a `maintenance.json` entry for the package's server. |
| `held` | For the class `held`: `{reason, reference?}`, each a non-blank text of at most 1024 bytes without a control character. |
| `overrides` | Any derived member named with its value: `id`, `priority`, `feedback_stop`, `on_failure`, `sensitive`, `next_run_clamp_ms`, `yield_points`, `expected_duration_ms`, `cooldown_ms`, `load_profile`, `strategic_weight_milli`, `loop_budget`. |

The classes refuse the members they do not take: `business` takes no `uses` and no `held`;
`maintenance` no `value`, `limited`, `realization` or `held`; `held` nothing but `held`.

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

## The checks

Every check runs before the first write, in this order; a refusal is `adopt_refused`, its
`adopt_problem` naming the check with the member, entry or pointer below, and writes nothing in
the repository and keeps no copy.

| Check | `adopt_problem` | Named by |
|---|---|---|
| `--repo` holds `applications.json` (its game and servers), `maintenance.json`, `packs/` and a `schedule.json` that reads; no `.part` of a root file is left | `repository_invalid` | `entry`; for `schedule.json` the reader's `json_pointer` and its refusal as the cause |
| The recording is as its last stop left it: `actingcommand.lab-recording.v1`, its `record_id`, `stopped`, not marked again since (`amended_at_unix_ms` absent or null), its package there | `recording_not_current` | `path` of `recording.json`, `member` |
| The package is admitted as `package preflight` admits it, and its `package_id` names one directory `packs/<package_id>` that no root file reserves | `package_invalid` | `path`, the loader's refusal as the cause; or `package_id` |
| Its game is `applications.json`'s and its server one of that file's | `package_mismatch` | `member` `game` or `server` |
| Not adopted before (its directory, its `.part`, its rule, its held entry, its `maintenance.json` entry), or `--replace`; the kept copy's place free | `pack_exists` | `entry` the directory or `json_pointer` of the entry; `path` of a taken place |
| With `--replace`, no change below `packs/<package_id>` the version control does not hold (`git status --porcelain=v1 -z --untracked-files=all`, which writes nothing) | `pack_dirty` | `entry`, `count`; or the version control's failure as the cause |
| The decision document | `decision_invalid` | `path`, `json_pointer` (`""` for a first adoption without one) |
| Its value's origin ("The value's origin") | `value_origin_model` | `path`, `json_pointer` `/value/origin` |
| A class other than `held` needs a package run through: its entry task's provenance a recording's, the cross-check `passed`, the first decision evaluated (no `first_decision_not_evaluated` warning, no application step) and no step whose click needs review | `recording_unreviewed` | `package_id`; the details carry `review` |
| No text of the package holds an absolute local path (a drive or network path, the state directory, the checkout) | `local_path` | `entry` the file, `json_pointer` the string |
| A root file adopt must change is in the canonical form ("The canonical form") | `root_file_not_canonical` | `entry` |
| The tree as it would be written composes (below) | `rule_refused` | `entry` `schedule.json`, the composer's `json_pointer`, its refusal as the cause; or `package_id` when the package comes out unclassified |

The last check lays the tree as it would be written under `<state>/adopt-check/<ms>/`: the
repository's pack directories with the package's own as it would be written, the root files adopt
changes, `package bundle` of them with every root file the repository holds (its maintenance
declarations those `maintenance.json` would hold), and the bundle's installed layout checked by
`standard-pack check`; both compose the schedule with one instance per server. Another refusal of
those two is answered as it is, and wrote nothing in the repository either. The check tree is kept
and named in the answer.

## What is derived

Each member the decision does not give is derived, answered in `derived` with its value and where
it came from: `package` (the package itself), `file` (the file's rules of the same tier: the rules
declaring maintenance or of a maintenance-role package, or the others) or `neutral` (a constant of
this contract). Nothing is chosen silently: a member nothing gives is refused `decision_invalid` at
`/overrides/<member>`.

| Member | Derived as |
|---|---|
| `id`, `package_id` | The package's `package_id` (`package`). |
| `priority` | With exactly two priorities among the file's rules, the lower for a business rule and the higher for a maintenance rule; else the one value the rules of its tier agree on (`file`); else none. |
| `feedback_stop` | The most frequent of the file's rules of the tier, each with its own id renamed the rule's (`file`); else an outcome predicate on the rule itself, `{kind: outcome, task_id: <rule>, outcome_key, comparison: eq, value: true}` (`package`). Every outcome predicate on the rule itself takes the first outcome key the package's entry task declares (`scheduling_outcome.mappings`), since the Runtime refuses a scheduled run's key its package does not declare; none when it declares none. |
| `consumes`, `produces` | `[]` (`neutral`) when the decision names none; for each decided entry its direction and, when not given, `observation_source` `self_reported` and `confidence_milli` 1000 (`neutral`). |
| `on_failure` | `{action: pause, retry_limit: 1, retry_backoff_ms: 0, escalation_threshold: 2}` (`neutral`). |
| `sensitive` | `false` (`neutral`). |
| `yield_points` | `[]` (`neutral`). |
| `expected_duration_ms` | The package's lower bound of one run, the sum over its operations of `post_delay_ms` and `expect_after.interval_ms` (`package`); 60000 when it gives none (`neutral`). |
| `next_run_clamp_ms` | The most frequent of the tier's rules (`file`); else 3600000 (`neutral`). |
| `cooldown_ms` | The most frequent of the tier's rules (`file`); else 60000 (`neutral`). |
| `load_profile` | The most frequent of the tier's rules (`file`); else `{kind: light}` (`neutral`). |
| `strategic_weight_milli` | The most frequent of the tier's rules (`file`); else 1000 (`neutral`). |

The members the decision gave are answered in `given`, in the order written; the value as
`fixed_value_milli` with the origin `pack`, or as `value` `"pending"`. A new rule is written in the
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
deleted on any path:

- `packs/<package_id>/`, written as `packs/<package_id>.part/` and renamed: only `control.json` and
  `resources/**` of the package, each JSON file in the canonical form. With `--replace` the old
  directory is first moved whole to `<state>/adopt/<package_id>.old-<ms>/`, which the answer names
  in `kept`.
- `schedule.json` (the rule appended or changed, the held entry, `requires`) and `maintenance.json`
  (an entry `{package_id, server, uses}`), each written as `<name>.part` and renamed, each only when
  it changes.

A failed write is `adopt_write_failed`: `path` where it stopped and `paths` what it had written or
moved before, in order, the operating system's refusal as the cause; nothing is removed (a `.part`
and a kept copy stay where they are). The state directory and the checkout on two volumes make the
move of a replaced directory fail so.

The repository's own items adopt does not write are named in `next` when the checkout holds them:
a regression case (`regression/<game>/`), the catalog section (`docs/PACKS.md`) and the census row
(`ours/coverage/business-v1.csv`).

## Repeating (`--replace`)

Without a decision the rule, the held entry and the maintenance entry are kept byte for byte, and
only the pack directory is replaced. A decision changes only the members it names (a decision of
another class than `held` lifts a hold, so the rule is composed again); a value the model wrote is
kept ("The value's origin"). A derived member the derivation would now give otherwise is answered
in `derived_drift` with the written and the derived value, and never written.

## `--dry-run`

Every check runs, the check tree under `<state>/adopt-check/` included; the answer is `validated`
with `written` empty and `would_write` what an adoption would write; nothing is written in the
repository and no copy is kept ([actinglab-dry-run.md](actinglab-dry-run.md)).

## The answer

`status` (`adopted` or `validated`), `dry_run`, `repo`, `game`, `server`, `package_id`,
`entry_task_id`, `class`, `replaced`, `review` (`recorded`, `record_id`, `cross_check`,
`first_decision_evaluated`, `steps_needing_review`, `run_through`), `rule`, `held` and
`maintenance` as written, `requires_added`, `given`, `derived`, `derived_drift`, `model_kept`,
`coverage` (a business rule: `complete`, `pending`, `undeclared` or `follower`, as the composer
counts the file's judged rules), `warnings` (those of the check that name the package or its rule),
`written`, `would_write`, `kept`, `check_tree` and `next`.

## The canonical form

The form `package fmt` writes: two-space indentation, one member per line, members in their order,
LF line ends and a final newline. A root file adopt must change and that is not in it is refused
(`root_file_not_canonical`), so that the diff of an adoption shows only what it changed: write it so
once, in a commit of its own.
