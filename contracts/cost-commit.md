# Entry-cost tasks (`cost_commit`)

Workflow #386 (scheduling rules, frozen model v1.4.2, section 4.6). Some tasks spend something
as soon as they pass a certain step: leaving the task after that step wastes what was spent. A
rerun after a condition termination would spend it again, and the Runtime can only give back
its own counters, never what the task spent. So such a task is ranked early, and it does not
enter its costly step when the time left is too short.

Which tasks spend on entry is stated only by the resource package. The Runtime reads the
declaration, checks its form and never infers it, rewrites it or learns an amount: amounts spent
or gained are read live by recognition (Workflow #384), not declared here.

**Status.** Written ahead of its code (documentation slice SR-0); slice SR-10 implements it. A
Runtime without SR-10 refuses a task that declares the member ("Older Runtimes"), so resource
repositories add declarations only in a release after SR-10 is deployed.

## Declaration

`cost_commit` is an optional top-level member of an operation `task.json`
(`resources/operations/<task_id>/task.json` in a content directory, `package-reference.md`):

```json
"cost_commit": { "step_id": "neutral.confirm", "after_ms": 90000 }
```

| Field | Rule |
|---|---|
| `step_id` | names one of the task's own operations: from that step's input on, leaving the task wastes what it spent |
| `after_ms` | the time from that step to the end of the task at normal speed (pacing factor 1), at least 1 and at most the task timeout |
| any other member | none |

A task whose package's maintenance use is `recognition` (the recognition maintenance package of
Workflow #384) must not declare it: recognition never spends. A business package that also
carries recognition markers may declare it. A declaration that breaks a rule is a check-config
error, `cost_commit_declaration_invalid` (`package_id`, `field` pointing at the member). The
declaration says only where spending starts; it carries no
game words, and the Runtime only compares step ids and durations.

## Task-to-entry-step table

At start the host reads the `cost_commit` declarations of the active catalog's task packages,
and only those, and builds the table task → entry step synchronously, before the start loads the
scheduling state from the ledger (the step that decides whether owed reruns survive a ledger
gap, Workflow #383). When another catalog is activated while the Runtime runs, the table is
built again the same way. The background admission check of `policy-suspension.md` ("Task
package suspension") does not build it. The table goes to the kernel and to the evaluator. The
scheduling catalog gains no field and the catalog generator does not change.

## Ranking

- With `scheduling.cost_commit.promote` (default true) the evaluator promotes a candidate whose
  task has an entry step ahead of the instance's ordinary candidates, with the same flag an owed
  rerun uses; when both apply, the total score decides. Such tasks therefore run early in the
  bookkeeping day, at a low pacing stage and far from a blackout.
- The selection document sees one more fixed candidate field, `task.cost_commit`: 1000 for a
  task with an entry step, otherwise 0, which a decision layer may weigh.

## Admission

- Time: every policy admission already requires the response deadline plus the quiet margin to
  end before the next blackout (`scheduling-blackout.md`, "Gradual quiet"), so an entry-cost task
  is never cut by a blackout in the middle.
- Pacing stage: while the instance's pacing stage is above `scheduling.cost_commit.max_stage`
  (default S2), such a candidate is deferred with `policy_cost_commit_waiting` (info, with
  `pacing_stage`) and evaluated again when the stage changes; the instance's other candidates
  run. At S3 a run is slowed down strongly and at S4 it is held, so entering then would most
  likely be paid for nothing. Until performance pacing leaves its observe-only mode (Workflow
  #385, released by Alice after she has watched the measurements), the stage the scheduler sees
  is always S0 and this gate has no effect.

## Before the entry step

Before it writes the `EffectIntent` of the entry step (after the sampling deadline check of that
step), the kernel checks:

1. the remaining task budget is at least `after_ms`; and
2. the remaining wall-clock time (the run's deadline, which admission placed before the next
   blackout) is at least `after_ms × f + scheduling.cost_commit.margin_ms`, where `f` is the
   current pacing factor.

The host hands `f` and the margin to the kernel with the step's pacing (the `step_pacing()`
seam); the kernel reads no configuration. When either check fails the run makes no input and
ends with `cost_commit_time_insufficient` (warning; `required_ms`, `remaining_ms`, `step_id`).
Nothing was spent. The code is a condition code: the run's counts are refunded, it adds to no
failure streak, it starts no recovery ladder, and it is rerun when the condition lifts
(`policy-suspension.md`, "Condition termination and owed reruns").

A run that passed its entry step and is still ended by a condition (a performance hold that
expired, the blackout safety net) settles and is rerun the same way; status and `ac_diagnose`
mark such an end with `cost_commit`, so a person can see that something may have been spent
twice. After a start that found a gap in the ledger, an owed rerun of an entry-cost task counts
as not owed: one rerun fewer is preferred to spending twice.

## Configuration

| Key | Default | Meaning |
|---|---|---|
| `scheduling.cost_commit.promote` | true | Rank entry-cost candidates first |
| `scheduling.cost_commit.max_stage` | S2 | Highest pacing stage at which such a candidate starts |
| `scheduling.cost_commit.margin_ms` | 60,000 | Wall-clock margin of check 2 |

The defaults live in `distribution/windows/actingd.defaults.json` (Workflow #387) and arrive
with the code that parses the keys (slice SR-1, v0.12.0). check-config lists them in
`config_manifest.parameters`.

## Older Runtimes

As for every `task.json` member (compare `resource-readings.md`, "Version gate"), a Runtime
older than SR-10 refuses a task that declares `cost_commit` on both load paths with
`UnknownField` @ `/cost_commit`; nothing ignores the member silently. SR-10 fixes the task schema
versions that accept it. The same holds for the `feature_locks` member that slice SR-5b adds:
resource repositories declare either member only in a release after the Runtime slice that
reads it is deployed.

## Codes

| Code | Category | Values | Slice |
|---|---|---|---|
| `cost_commit_time_insufficient` | warning | `required_ms`, `remaining_ms`, `step_id` | SR-10 |
| `cost_commit_declaration_invalid` | error | `package_id`, `field` | SR-10 |
| `policy_cost_commit_waiting` | info | `instance_id`, `pacing_stage`; `next_eligible_unix_ms` when known | SR-10 |

None of them triggers the recovery ladder.
