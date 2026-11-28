# Policy suspension of linear tasks

A scheduled `linear_steps` task that fails mid-run is recorded, rerun at once, and paused when
the same problem happens again; the pause is lifted automatically once the content of its
package (or of a package its run went through) changes (Workflow #336, R17-R19, R21, R22, R25;
PKG-S3, option 丙: judged by the content hashes the failure identity recorded, without a
restart). Since Workflow #390 EXEC-3 (forced compromise E-02) a scheduled page-graph task
follows the same rules: its failure records a failure identity too. Everything
here uses the existing policy `on_failure`, the existing `policy.execution_recorded` and
`runtime.failed` records and the existing `paused_task` disposition: no ledger event type or
enum value is added. Since Workflow #378 A2-P the failure part of `policy.execution_recorded`
carries the run's failure as `outcome` and the failure identity as `failure_identity` (one
`error_code` string before); a ledger is read and replayed by the build that wrote it.

The last three sections ("Task package suspension", "Condition termination and owed reruns" and
their configuration) are the scheduling rules of Workflow #386, specified ahead of their code;
unlike the rest of this document, the condition termination adds one persisted member. The
section after them, "A stopped run" (Workflow #429 R-6b, since 0.12.55), adds a third result kind
of `policy.execution_recorded`, beside a success and a failure.

## Catalog

The recommended `on_failure` of a linear or a page-graph task is

```json
{"action":"pause","retry_limit":1,"retry_backoff_ms":0,"escalation_threshold":2}
```

The first failure is recorded and scheduled for a retry; a second failure with the same
failure identity reaches the escalation threshold and pauses the task (`paused_task`). Under
performance pressure a recoverable failure does not escalate, and the second failure is paused
by `retry_limit` with `action` `pause` instead. Either record's severity is its failure's
category (Workflow #378 A2-P), whatever the class. Nothing is applied automatically:
`on_failure` is part of the approved catalog, and a change of it is a catalog change to be
approved again.

`record stop` of a Lab recording prints this value as `catalog_on_failure_example`
(`lab-recording.md`, "Output").

## Immediate rerun (R22)

A pair (catalog task, instance) whose latest failure carries a failure identity, is
scheduled for a retry and has a live retry round is treated by the evaluator as triggered and past its task cooldown
(reason `failure_retry_immediate`): it is rerun as soon as its retry backoff has passed, at the
next evaluation, instead of at the next clock occurrence. `actingd` evaluates again right after
a cycle in which a scheduled run failed, so that evaluation, not the wake computed before the
run, sets the next wake (the end of the backoff). The feedback stop, placement, the
loop and activity budgets, the activity's sampled interval after its previous admission
(`policy_activity_interval_active`) and the activity windows still apply, and every rerun
consumes the budgets like any dispatch. A failure recorded without a failure identity keeps waiting for
its trigger and cooldown. A page-graph task's failure carries one (Workflow #390 EXEC-3), so a
page-graph task is rerun at once as a linear one is. Since 0.12.77 (Workflow #490 ENTRY-A) a
failure at the run's entry (a code of class `pack_logic_mismatch`) is no failure of the task and is
never rerun at once ("A failure at the entry" below): it settles as a condition, with no retry, and
blocks its instance; every such failure settles so (fix round 4: none counts against its rule).
Its pair then waits a growing interval before it is dispatched on that instance again ("A pair that
keeps failing at its entry").

`PolicyControlState` owns the round. An admitted normal trigger opens it using the original
`PolicyActivitySample` and budget receipt. An admission whose reason is
`failure_retry_immediate` retains that origin; consecutive failures and new failure identities
do not renew it. The round uses that profile's original half-open activity occurrence and
budget limits. The shared `activity_window_at` calculation assigns an overnight occurrence to
its opening day; midnight inside it does not reset its budgets. A full-day occurrence ends at
the next local midnight, and an ordinary occurrence ends at its declared end. A different
profile/window or expected duration cannot use the original round's immediate permission.
RR-1 (register §1.221, since 0.12.31): the evaluation offers a pair as `failure_retry_immediate`
only by the admission's own predicate, on the profile and expected duration its intent will carry
(the expected duration E of SCH-M ED-2 included, which a new run sample, or the pair's package
becoming known after a start, changes); a pair whose round those terms no longer match is not
offered and waits for its own trigger and cooldown, so it never takes its instance only to be
refused.

Admission spends task/activity daily and window counts. Once any original count limit is
reached the round ends. Runtime usage settles the reservation to actual duration at execution
completion; if the remaining original task/activity runtime budget cannot reserve the next
run's expected duration, the round ends then. Pending reservations still constrain admission.
Shared activity consumption by another task of that instance uses the same counters and can
end the round. A closed round stays closed when budgets become available again. Window expiry
also ends immediate permission, including for an intent evaluated before the boundary and
admitted after it (`policy_retry_round_ended`). RR-1: an immediate rerun the admission refuses
with `policy_retry_round_ended` is withdrawn at once: its round (that of the pair's accounting
group on the instance) ends in the Runtime's memory, the next evaluation does not offer it again
and the instance's other candidates are selectable; the round's end is in the pair's value record
written with the refusal, and a replay of the recorded refusal ends it too. The next legal
normal trigger opens a new round; budget availability alone is not a trigger. A run already admitted finishes under its existing
deadline and permission, with no additional stop at a date boundary.

Success clears the round. Ending immediate permission does not clear failure history or
change failure classification, backoff, sensitive/severe pauses or package-update lifting.
Restart reconstructs rounds in ledger order from accepted dispatch reasons, original admission
timestamps/receipts and execution settlements. It never substitutes restart time or reruns old
triggers. Historical accepted receipts remain replayable; their recorded immediate admissions
do not create a fresh origin after the original round ended.

## Failure identity

The live settlement of a scheduled run whose main package was admitted, as `linear_steps` or
(Workflow #390 EXEC-3) as a page-graph package, writes the failure identity as
`failure.failure_identity` of its `policy.execution_recorded`, beside the run's failure as
`failure.outcome` (Workflow #378 A2-P):

```text
<base>~v2~k<K>~m<I><H>[~p<I><H>]{0,3}[~r<G><I><H>]{0,1}[~n<I><H>]{0,1}[~g<H>]{0,1}~f<F>
```

| Segment | Content |
|---|---|
| `base` | The terminal `failure_code`, or the request error code when the run has no terminal. A code longer than 64 bytes or containing `~` is `h` and the first 16 hex digits of its SHA-256. |
| `K` | The first 12 hex digits of SHA-256 over `actingcommand.failure-key.v1`, the original code, the scope, the step and detail′, joined by newlines. |
| `m<I><H>` | The main package: `I` the first 8 hex digits of SHA-256 over its `package_id` (the admitted package's `package_label`), `H` the first 12 hex digits of its content hash measured when the failure is settled, or `x` when it could not be measured (removed, being replaced, unreadable). |
| `p<I><H>` | One per declared prerequisite layer, outermost first: `I` as above for the layer's package id, `H` the content hash of the package at the path the run's standard-pack registry gave it (Workflow #373 NAV-3a), measured at the settlement, or `x` when the registry bound none or the package could not be measured. A lift check finds the layer's package in the registries read at the check. |
| `r<G><I><H>` | The return-home layer: `G` the first 8 hex digits of SHA-256 over game, NUL, server; `I` and `H` as above. |
| `n<I><H>` | Workflow #373 NAV-5 (M-373 §8.5): the navigation, recovery chain or interrupt layer package a navigation failure happened in (the last `EntryRecoveryFailed` of a layer with role `navigation_hop`, `recovery_chain` or `interrupt_handler`), or the package a `contained_task_navigation_package_invalid` names; `I` and `H` as above, `H` at the path the registry of the run's game gives it. Absent when the failure happened in no such layer. |
| `g<H>` | Workflow #373 NAV-5 (M-373 §8.5): the first 12 hex digits of SHA-256 over the run's game root's `pages.json`, `navigation.json` and `interrupts.json`, in that order (an absent file adds nothing), `x` when they could not be read. Present, after `n`, on a failure whose code starts with `contained_task_navigation_` or `contained_task_interrupt_`, or that happened in such a layer; absent on every other failure. |
| `F` | The error frame's artifact SHA-256 (first 12 hex digits) of the run where this identity first occurred; `na` without a frame; `u` and the first 12 hex digits of SHA-256 over the decision id for a failure that does not accumulate; since 0.12.77 (Workflow #490 ENTRY-A) `i` and the same digits for such a failure at the run's entry that the settlement settled as a condition of the instance ("A failure at the entry"); it does not accumulate either, and (fix round 4) every failure at the entry is written so. |

The identity prefix is everything before `~f`. At most 250 bytes (Workflow #373 NAV-5: was
214 before `n` and `g`), without whitespace or control characters. A segment out of the order
`p`, `r`, `n`, `g` is no identity. A format v1 string (Workflow #336 L6, before PKG-S3) is not read.

**Content hash** (Workflow #336 PKG-S3, option 丙; M-PKG §3.3). A package's content hash is the
`content-directory.v1` hash of the content table its path holds (a directory, a `.json` or a
`.zip` content container), or the SHA-256 of the whole file for an assembled package ZIP (one
holding `resources/manifest.json`). It has two uses: a pause and (Workflow #463 U1b, since
0.12.63) a pair's break-in. For them it is measured in three places only, read-only: when a scheduled
failure is settled (the main package at the path the run used and each layer at the path the
run's standard-pack registry gave it, Workflow #373 NAV-3a; SR-3 adds its suspension point), at a
lift check ("Lifting"), and (U1b) when a scheduled run of a `linear_steps` main package is
prepared, at the path the run reads it from ("A failure that pauses at its first occurrence"),
through the lift checks' memory: a location measured within
`scheduling.suspension.lift_check_min_interval_ms`, by a preparation or a lift check, is not
measured again. For a package replaced inside that interval (fix round 3), until the interval ends
its runs are judged and counted against the content measured before; the new content's break-in
starts with the first measurement after it. Admission never compares it; it is stored inside the
failure identity and, as its first 12 hex digits, in the pair's break-in. Since Workflow #463 CH-1
(Runtime 0.12.71, ledger revision 9; the owner's ruling of 11-16, which relaxes option 丙 for
computing and recording only; [v012-register.md](v012-register.md) §1.455) the admission of a
run's package by its path also computes it once, by the same function, on the bytes it read, and
records it in the run's admission record `task.requested` (`content_hash`), so the ledger says
which content each run was admitted with. Nothing of the suspension reads that record: the
failure identity, the break-in and the lift checks measure the content themselves, in the three
places above, as before, and a pause, a break-in or a lift is decided exactly as without it.

Every other run writes no failure identity: a main package that was not admitted (its mode is
unknown) and the startup reconciliation of an interrupted settlement.

Every failed run records its failure as `failure.outcome`: the top node of its terminal's
outcome (code, category and values, without its links), the top node of its request error when
it has no terminal, `policy_runtime_budget_exceeded` when it ran over its runtime budget, or
`policy_settlement_interrupted` for an interrupted settlement. The record's severity is that
outcome's category. The failure's code is the outcome's code, or for
`outcome_relay_unregistered` the spelling it keeps as `raw_code`.

### Scope and step

- `prepare`: the prerequisite chain was not resolved and admitted; the run has no task
  records. The step is `prepare`.
- `pre:<I>`: the run has a prerequisite chain, the gate did not start the package
  (`EntryTargetDisposition` `Started`), and a prerequisite package was opened
  (`EntryRecoveryPackageAdmitted`) and not completed: `I` is the first 8 hex digits of the
  SHA-256 of the innermost one's `package_id`, the `package_label` its record carries
  (Workflow #336 PKG-S2; before, the first 12 hex digits of its package digest).
- `gate`: the same, with no prerequisite package open (the gate's own first check, its first
  capture, a recheck after a completed layer).
- `main`: every other failure.

A page-graph run without a prerequisite chain (Workflow #390 EXEC-3) that checked its
required entry (it wrote an `EntryRecoveryDecision`) treats that check as its gate and its
entry recovery package as its prerequisite package above: `pre:<I>` while that package is
open, `gate` before the entry started the package, `main` after. Without an entry check every
failure of such a run is `main`.

The step is the operation label of the scope's last `StepStarted`, `entry` without one.

### detail′

The failure's native detail (the text of its lifecycle failure record), reduced to its
`key=value` fragments whose key is one of `operation`, `after_page`, `hit_error_page`,
`transition`, `intermediate_seen`, `layer`, `package_id`, `required_page`, `return_home`,
`reason`, `application`, in their order, joined by single spaces. A detail without `=` is kept
whole; no detail is empty. `attempts` is not kept, nor are `awaited` and `skip_target_seen`.

## Which failures accumulate (R21, R25)

Only these failures can pause a task by repeating:

- the `main` scope, outside the restart segment, with `page_confirmation_failed`,
  `contained_task_linear_intermediate_unobserved`, `contained_task_step_unconfirmed` or
  `contained_task_guard_refused`: the main package's own steps no longer match the screen. A
  page-graph step unconfirmed after all its retries (`contained_task_step_unconfirmed`) is the
  graph counterpart of a linear intermediate page not seen (Workflow #390 EXEC-3, wave 14
  ruling 4). Workflow #463 U1: `contained_task_linear_unrecorded_screen` writes its identity as
  these do, though it pauses at its first occurrence ("A failure that pauses at its first
  occurrence" below);
- the `prepare` scope with a `contained_task_prerequisite_*` refusal: a reproducible
  configuration problem (a package the registry does not list, a package id that does not
  match, an incompatible package);
- Workflow #373 NAV-5 (M-373 §8.5, §15 ruling 4): every scope but `prepare`, the restart
  segment included, with a code that starts with `contained_task_navigation_`: the standard
  pack's navigation data does not bring the run to its entry (a recovery chain may restart
  the application, so its end is in a restart segment). Its identity names the data files
  (`g`), so a fix of the data lifts the pause;
- Workflow #373 NAV-5 (wave-17 acceptance ruling 2; M-373 does not say): every scope but
  `prepare`, the restart segment included, with a code that starts with
  `contained_task_interrupt_` (the failures `contained_task_interrupt_limit_reached` and
  `contained_task_interrupt_escalated`). For the reason of Workflow #390 EXEC-3 ruling 4: a
  screen the run cannot handle, again and again, means the standard pack no longer matches
  the game, so the pair pauses until the package or the data changes (its identity names the
  data files, `g`, and the handler's layer, `n`). `prepare` is excluded because interrupts
  there are expected. Workflow #411 B43: `contained_task_interrupt_environment_limit_reached`
  is excluded in every scope. It ends a run only on a page whose `interrupts.json` entry
  declares `limit_cause` `"environment"` ([package-reference.md](package-reference.md),
  "Standard package root"): the data says the screen is the game's network or servers, not a
  pack that no longer matches, so the pair is rerun (R22) and never paused by repeating. Its
  identity still names `g`. UPD-1 (0.12.30): `contained_task_application_update_required` (the
  game's update dialog, an `interrupts.json` entry with `result`) is not under the
  `contained_task_interrupt_` prefix and never accumulates; it ends a scheduled run by a
  condition ("Condition termination and owed reruns", "Which runs"). Workflow #463 INT-1
  (0.12.73): `contained_task_environment_reset` (an `interrupts.json` entry whose `result` is
  `"environment_reset"`: the environment took the game out of the task) is not under that prefix
  either and never accumulates in any scope: it is an ordinary failure rerun only, its identity's
  `F` unique, so two in a row on similar frames leave the pair unpaused and the next evaluation
  reruns it at once (R22). An entry without handlers and without `result` still ends its run with
  `contained_task_interrupt_escalated`, which accumulates as above.

Every other failure is rerun only: its `F` is unique, so it never repeats the previous identity
and its count stays 1. This covers the gate and every prerequisite and return-home package,
the start not on the first step, `contained_task_linear_application_unconfirmed`, device and
backend errors, operator pause, cancellation and deadline, the task timeout and recognition
errors. Since 0.12.77 (Workflow #490 ENTRY-A) such a failure at the run's entry whose code's
class is `pack_logic_mismatch` (raised by the entry's own layers, whatever `exclude_codes` says of
its code) and that its instance's block has a recovery for is not rerun either: it settles as a
condition of the instance ("A failure at the entry" below), always (fix round 4: it never
accumulates and never counts against its rule). A failure of any other class met at the entry (the
capture backend's, the device link's, a fence's) is rerun only, as before.

The restart segment (R25-1): the run had a `launch` or `restart` application effect
(`application.intent`) and no `StepFinished` after the last one names a main-interface page
(canonical anchor `home` or `step_<digits>_home`; for a page-graph run the literal `home`
only, Workflow #390 EXEC-3). A failure there is rerun only.

R25-2: a linear task's terminal `application_backend_operation_failed` or
`input_backend_operation_failed` that did not poison the run is settled with the original
class `recoverable` and rerun only. A poisoning failure (an unconfirmed resource close) stays
severe. The task terminal, its severity and the `application.*` records are unchanged; the
startup reconciliation still settles from the terminal's severity.

A severe failure, a sensitive task, an exceeded runtime budget and an interrupted settlement
pause at the first failure, as before; since 0.12.77 (Workflow #490 ENTRY-A fix round 5, ruling 1)
not for a failure at the entry: a sensitive task's failure at the entry settles as any task's ("A
failure at the entry"). Since Workflow #361 M6 an interrupted settlement (a
scheduled run the daemon's own end cut short, settled at the next start as
`policy_settlement_interrupted`) is recorded the same way but does not hold the pair: it is
no failure of the task, so the restart that recorded it lifts it (see "Lifting"). Only the
first interrupted settlement of a pair in a row is lifted; a second one in a row (the pair's
previous execution was an interrupted settlement too, `consecutive_same_error` 2 or more)
holds the pair as before, until its package changes, so a task that itself ends the daemon is
not re-admitted after every restart.

A run that ends on a mapped key whose mapping declares a remedy (Workflow #411 B15,
[remedies.md](remedies.md)) is a success: it never accumulates, records no failure identity and
clears the pair's streak and immediate-rerun round; a block that repeats is bounded by its remedy
round instead. A linear run that ends on a select step's `on_empty` key (Workflow #463 K1,
[linear-steps.md](linear-steps.md), "Select steps") or on an ending page (Workflow #463 K2, since
0.12.61, [linear-steps.md](linear-steps.md), "Endings") is a success in the same way.

## A failure at the entry (Workflow #490 ENTRY-A, since 0.12.77)

Register §1.494; the coordinator's work order of 11-20 on board #490, its constraint of 23:18 JST
and the rulings of its four fix rounds (11-21), not the owner's words. The production defect of
11-20 (v0.12.75): a package left a game on a screen no package knows; every task due after it
failed at its entry, was rerun at once (R22) until its runs of the day were spent, a rule whose
`escalation_threshold` is 1 was paused by one such failure, and once nothing failed any more
nothing triggered a recovery. The harms kept out: an instance left blocked with nothing to end it
and nobody told; a rule paused, or kept from running beyond its window's end, by failures at its
entry; a task that never runs again; a ladder started where the configuration deliberately
excluded one; an instance or its emulator restarted again and again for nothing.

**Which failures.** The live settlement of a scheduled run decides it, from what it already reads
for the failure identity ("Failure identity"), and writes the result into the record (below). A
run failed at its entry when all of these hold:

- its main package was admitted and its prerequisite chain resolved (not the `prepare` scope:
  a refusal of the chain is a configuration problem and accumulates as before);
- it checked its entry and its main package never started: a gated run (a prerequisite chain, a
  page-graph run's entry check) or a home entry's preflight (an `EntryRecoveryDecision` among its
  rows), and no `EntryTargetDisposition` `Started`, so the main package executed no step. The
  scopes are `gate` and `pre:<I>`, and the home entry's check and its recovery package (which
  "Scope and step" counts as `main` for a `linear_steps` run without a chain);
- its failure does not accumulate ("Which failures accumulate": the navigation and interrupt
  failures that accumulate in every scope stay accumulating failures of the task);
- **its code's class is `pack_logic_mismatch`** (fix round 2, ruling 1): the entry's own layers
  (its prerequisite chain, its return-home layer, its entry check) judged the screen not to be what
  their packages expect. Nothing else is a failure at the entry, whatever the ladder's triggers say
  of its code: a code of any other class (the capture backend's four, `environment_fault`, which
  the shipped item `task_capture_backend` takes; the device link's `device_link_lost`; a fence's
  `instance_resources_fenced`; a configuration's `declaration_error` codes; a vision provider's)
  sets no mark, blocks nothing and gives nothing back: it is settled exactly as before ENTRY-A,
  rerun at once (R22), its ladder the one its trigger item starts at its lease end, if any. The
  class is the code's own (`ladder_class`); no code is named in the decision;
- its instance's block has a recovery for it: the run's ladder rules allow a ladder at the task
  stage at all (not a direct run, a physical instance, `stuck_recovery` on) and a trigger item
  takes the class at the task stage, matched with `exclude_codes` not read (fix round 1: the
  block's recovery is the block's own trigger; with the shipped triggers `pack_logic`). Otherwise
  the failure settles as before ENTRY-A: an instance with no recovery is never blocked;
- its policy class is `recoverable` (a code the Runtime raises only as fatal, or a terminal its
  failure point marked `failure_severe`, is `severe` and settles as before). A sensitive task's
  failure is one too (fix round 5, ruling 1): its main package did not start, so nothing
  sensitive was begun.

So with the shipped configuration the set is every code of class `pack_logic_mismatch` that the
gate, a prerequisite or return-home layer, the home entry's check or its recovery package end a run
with, the excluded ones included: `contained_task_page_unknown`,
`contained_task_recognition_failed`, `contained_task_home_entry_not_matched`,
`contained_task_home_recovery_binding_missing`, `contained_task_prerequisite_entry_unmatched`,
`contained_task_prerequisite_step_limit`, `contained_task_prerequisite_final_page_missing`,
`contained_task_return_home_entry_unmatched`, a layer's own `page_confirmation_failed`,
`contained_task_linear_intermediate_unobserved`, `contained_task_step_unconfirmed` and
`contained_task_guard_refused`. A run the start's reconciliation settles from its terminal (its
settlement was interrupted) has no failure identity and settles as before. *A known gap, not
fixed in this slice* (fix round 6, ruling 6): a failure at the entry that a reconciliation settles
(the process died between the run's terminal state and its settlement, or the online settlement
of a run whose worker channel was abandoned) is settled from its terminal, so it counts against
its rule as any failure did before ENTRY-A (its streak, its `on_failure`): a rule whose
`escalation_threshold` is 1 is paused by it until its package's content changes. The ledger holds
what would tell it (the run's `EntryRecoveryDecision` and whether its main package started, its
`task.semantic` rows with the run's links); what forbids settling it as a condition is the
ledger's own rule that a failure settled from a terminal state carries an empty failure identity,
so the record cannot carry the entry mark `~fi`.

**Settlement.** Every such run settles by the condition path ("Condition termination and owed
reruns", "Settlement"): its record carries `condition` with `refunded` true whatever
`scheduling.rerun.refund_counts` says, `count` 1 and the end of the activity window it was admitted
in; it adds to no failure streak and no escalation (the pair's counters stay as they were), keeps
the disposition `continue` with no `retry_at_unix_ms` (so R22 does not rerun it), cannot pause its
pair whatever `escalation_threshold` says, gives back the four counts its admission took, and its
runtime is not counted. It owes no rerun: the pair's owed rerun stays as it was, and the task is a
candidate again by its own trigger and cooldown once its instance is no longer blocked and its own
wait has passed ("A pair that keeps failing at its entry"). Its failure identity is written as for
a failure that does not accumulate, with the frame mark `i` in place of `u` (the same digest of the
decision id): `<base>~v2~…~fi<12 hex>`. That mark is how the record says it: a replay reads
`condition` and the mark from the record, owes no rerun, and never reads the configuration. No
member and no word of the record is added; the record's outcome keeps its code. A sensitive task's
failure at the entry settles the same way (fix round 5, ruling 1; before it, such a failure paused
the pair at once and lifted only when the package changed): the sensitive rule is not applied, so
its record's `sensitive` is `false`, as every other record of a failure at the entry, which the
record's own check requires of a record that continues. *Before a rollback to 0.12.76* (fix round
6, ruling 5): a v0.12.76 Runtime judges whether a task is sensitive by the catalogue generation the
dispatch ran under, not by the record, and refuses a sensitive task's record that carries a
condition, so it stops at its start (`policy_execution_record_mismatch`) on one it compares; a start
with a start mark compares only the execution records after the decision memory's last value
record (`policy.control_state_recorded` or `policy.activity_state_recorded`), and this build writes
the pair's value record right after each settlement, so only a record whose value record was never
written (the process stopped between the two) stands there; a start that replays the whole
ledger (none of a start mark) compares every record. What a person checks, in two steps: (1)
whether any catalogue generation activated since this version was deployed (the
`catalog.activated` records since the deploy, each naming its generation; the generation's tasks)
holds a task whose `sensitive` is `true`; if none, nothing more to check and the rollback is safe
as far as this goes; (2) if one does, whether the ledger holds, after its last
`policy.control_state_recorded` or `policy.activity_state_recorded` record, a
`policy.execution_recorded` record of such a task (`task_id`) whose `failure` carries `condition`
and whose `failure.failure_identity` ends in `~fi` and twelve hex digits; with one there, a
v0.12.76 start stops on it, and with a start that replays the whole ledger any such record of the
ledger stops it.

**No failure at the entry counts against its rule** (fix round 4, ruling 1). The earlier rounds
judged some failures at the entry the pair's own (after a recovery, then after another task was let
in) and settled them by the rule's `on_failure`; each version of that evidence still paused a
healthy rule for another's fault (a task that leaves the game on a screen no return home can
leave, a store update, two broken pairs taking turns), and such a pause lifts only when the
package's content changes. So there is no "pair's own" failure: every failure at the entry settles
as above, whatever ran between, and a pair whose entry is broken is held back by two things that
end by themselves (below). The worst a wrong judgement can do is cost one rule the rest of one
activity window.

**A pair that keeps failing at its entry** (fix round 4, rulings 2, 3 and 5). The Runtime keeps in
its memory, per pair (instance, catalog task), its failures at the entry while its scheduled runs
keep failing there, with the packages the latest failure's identity names (the main package and
each declared prerequisite and return-home layer with its content hash) and the activity window
its admission was committed in (the `expires_at_unix_ms` its condition carries). No record, fact or
status member holds them.

- *The count in a row and the wait* (ruling 2). Each failure at the entry counts one more in a row
  when the pair's previous one named the same packages in the same window, else it is the first.
  After the n-th the pair is not dispatched on that instance before `scheduling.unavailable_probe`'s
  interval for n (`first_interval_ms`, times `factor` for each further one, at most
  `max_interval_ms`; shipped: one minute, two, four, … thirty; fix round 5, ruling 3 (i): past
  `max_interval_ms` one more `first_interval_ms` for each further failure in a row, thirty-one,
  thirty-two, …, so two pairs of an instance whose counts differ never come due together and the
  one that has just failed again comes after the others) has passed since the end of the block
  that failure set, whatever ends it (the coordinator's addition to fix round 4); until a block
  ends, and when none ended since the failure (it set none), since its settlement; and never past
  the window's end. At every block's end the pairs of the instance with a count in a row wait from
  that end in turn (fix round 7, ruling 1, which replaces fix round 6's rule; fix round 8, ruling
  1): first the pairs whose failures did not set the block, in the order of their latest failure
  at the entry, the one that failed longest ago first (equal times by the task's id, so the order
  is the same on every run), then the pairs whose failures set or renewed the block, in the same
  order among themselves; the pair at place n waits n `first_interval_ms` from that end, and a
  pair that set the block waits its own interval when that is later; a pair set aside takes no
  place; a wait still running is never shortened (the later time stands). With no other pair, the
  pair that set the block waits its own interval, as above. So after every block the pairs come
  due one at a time, least recently failed first and those that set the block last, and a pair
  the outage left with a long interval waits its turn, not that interval. Any rule that decides a
  pair's wait from intervals alone lets the evaluation's order pick among the pairs that come due
  together: under fix rounds 5 and 6 broken pairs could take turns ahead of a healthy pair for
  hours (the seventh review: a broken pair at four in a row first in the order, a second with none
  and a healthy pair at six, about eight hours), and fix round 7 left the pairs that set the block
  at their own interval, so one whose interval was a minute came due with the first turn and,
  first in the evaluation's order, ran again first. *The bound:* after an outage a healthy pair is
  let in after each broken pair ahead of it in the order has failed once more at most, about one
  block each. In a stretch that let nothing in, a block lasts about half an hour from its sixth on
  ("The worst cases").
  The candidate waits `instance_entry_task_waiting` (info; `instance_id`,
  `task_id`, `decision_id`, `failure_code`, `observed_at_unix_ms` of the latest failure, `count` the
  failures in a row, `next_eligible_unix_ms` the wait's end) and wakes then; a fact refresh names the
  code as its `gate_code`; the schedule explanation's candidate rows show it. Position 4b of the
  order of the deferrals ([scheduling-blackout.md](scheduling-blackout.md)) holds it, after the
  instance's block: the instance's other tasks are not held and run meanwhile. *A run let in ends
  every wait* (fix round 5, ruling 3 (ii)): a scheduled run let in on the instance (the event that
  counts below; not a store update task's run, not a client's or the Lab's run) sets the count in a
  row of every pair of that instance back to none and ends its wait at once, and a later block's
  end sets it none; what counts against a pair, its window and its packages stay, so its next
  failure at the entry is one more against it. No pair waits for another to be dispatched.
- *Set aside for the window* (ruling 3). Of those failures, the Runtime counts the ones that came
  although the instance had let another scheduled run in since the pair's previous failure at the
  entry: one that succeeded, or one whose main package started after an entry check it passed (an
  `EntryTargetDisposition` `Started` among its rows); a run whose package checks no entry starts its
  main package whatever the screen and is counted only when it succeeds; a client's or the Lab's run
  is no scheduled run; a store update task's run is none, whether it succeeds (ruling 5) or its
  main package starts and it fails (fix round 5, ruling 4). When that count reaches the
  rule's `on_failure.escalation_threshold` (the catalogue requires it of every task, 1 to 1000; where
  a rule's decision gives no `on_failure`, `standard-pack adopt` writes the neutral `{pause, 1, 0,
  2}`, so 2), the pair is not dispatched on that instance again until the end of the activity
  window its admission was in: the candidate waits `instance_entry_task_set_aside` (info; the same
  values but `count` the failures against it, `limit_count` the threshold and `window_end_unix_ms`)
  and wakes at the window's end, and a person is told once per pair and window, by
  `instance_entry_task_set_aside_reported` (a warning `runtime.failed` record under the instance,
  handed to the daemon's log; the task, the count, the threshold and the window's end), and again
  when the pair is set aside again after a catalogue's activation, a changed package or a Runtime
  start cleared its counts (fix round 5, ruling 6). A pair whose
  window the policy does not know is never set aside. Nothing else follows: no pause of the rule, no
  change to its streak, nothing that outlives the window; the next window starts from nothing.
- *Its end.* The counts and the wait start from nothing when a scheduled run of the pair settles any
  other way (its main package started, a success, a stop, an interrupted settlement, a failure of
  another class or before its chain was resolved), when the window its admission was in ends, when
  its packages change (at its next failure, which then counts from one), at a restart (memory only),
  and (ruling 5) when a store update task's run succeeds on the instance, for every pair of the
  instance: the game is another version. That success still ends the block and the stretch as any
  run's success does. A catalogue that becomes active (an activation or a rollback; the
  coordinator's addition to fix round 4) clears every pair's count, wait and being set aside at
  once, of every instance (the Runtime's one active catalogue serves them all): it may bind other
  packages to the pair's task, and the pairs' packages are not compared there. A placement can
  replace a registered package's files without changing the rules a composition activates (a
  scheduled run takes its package as the registry places it when it is dispatched), so (fix round
  5, ruling 5) the scheduler workbench's check that finds a package of a game in use changed frees
  every pair the same way. Fix round 6 (ruling 4): the check compares, per game in use, the length
  and modification time of `bundle.json` (the root's index of its packs: it holds each pack's file
  and byte counts, no digest, and `place` writes it again only when its bytes change), of the
  recognition files `pages.json`, `interrupts.json` and `navigation.json`, and of each registered
  package's `control.json` and `resources/operations/resources.json` (`place` replaces a
  package's directory whole, so every file of a placed package is written anew), with what this
  comparison kept at the check before, not with the marks of the last composition that succeeded:
  one change frees the pairs at one check, also while another game's root is broken or a
  composition is refused. The registered packages whose two files are compared are those of the
  last composition that succeeded (fix round 7, ruling 3): until a composition has succeeded
  since the start (while a game root has been broken or every composition refused since then),
  only the root's `bundle.json` and the three recognition files are compared, so a change of a
  package's files that leaves those alone is not seen until then. Not seen either: a change by
  hand to a package's other files (a task file, a
  template) that leaves those two files as they were, a change of `tables/`, `loading/`,
  `notices/` or the root's other files, and in configured mode (no workbench) anything but a
  catalogue's activation. A client's run, a
  recovery, a resume and a blackout end the block but leave the counts as they are (the wait then
  runs from that end, above).

**The worst cases** (fix round 4, ruling 3; fix round 5, ruling 3). Shipped values
(`first_interval_ms` 60 000, `factor` 2, `max_interval_ms` 1 800 000,
`recovery_ladder.cooldown_secs` 600), climbs that end within about two minutes and a recovery that
brings the game home each time (a pair whose own entry is broken), runs short:

- *A broken pair with a healthy task due* (two broken pairs ahead of the healthy one; a healthy
  task that never failed has no wait). The first broken pair fails at about 0; its block ends at
  about 2 and it waits its minute, the second broken pair fails at about 2 and its block's climb,
  held by the cool-down window, ends at about 12; the healthy task runs then and is let in, which
  ends both waits: the first pair fails at about 13, the first against it (a threshold of 1 sets
  it aside there), and the second at about 23 after the next climb (set aside there under 1).
  With the recommended `on_failure` (threshold 2) they are set aside at about 33 and 43, the
  healthy task running at every block's end. As fix round 4 (case (c) of fix round 5).
- *After a long outage* (fix round 5, cases (a) and (b); times as the turn order of fix rounds 7
  and 8 gives them). Every pair of the instance waits the maximum from the outage's last block's end (minute 0
  here), the broken one first in the evaluation's order. At 30 it fails again, its next wait
  thirty-one minutes; its climb (the stretch's pace is thirty minutes too) ends its block at about
  32, and the healthy pairs take turns: the first runs at about 33 and is let in, every wait ends,
  the broken pair fails at about 34 (one against it: set aside under a threshold of 1), its next
  block, a new stretch's, waits for the cool-down window and ends at about 42, the second healthy
  pair runs then, and the broken pair's next failure at about 43 is the second against it (set
  aside under the recommended threshold 2). With two broken pairs ahead of one healthy pair the
  healthy pair runs at about 33 when its latest failure is older than the second broken pair's,
  else that pair fails at 33 (its turn first), its block ends at about 64 and the healthy pair
  runs at about 65; each broken pair's next failure is then one against it. Fix round 6's rule
  gave 62 and 94.
- *A new broken pair ahead of healthy pairs the outage left with long waits* (fix round 6, ruling
  3; the sixth review's case: two healthy pairs at six and ten in a row, waiting thirty and
  thirty-four minutes from the outage's end at 0, a broken pair with no count first in the order).
  The broken pair fails at about 0; its block's climb, held by the stretch's pace, ends at about
  32; the healthy pairs take turns, one and two first intervals, and the broken pair, whose
  failure set the block, the third (fix round 8; fix round 7 gave it its own minute, so it came
  due with the first healthy pair and, first in the evaluation's order, failed again first): the
  first healthy pair runs at about 33 (34 when the pair with the longer wait failed longest ago:
  its own wait still runs past its turn) and is let in, the broken pair fails at about 34, the
  first against it (set aside there under a threshold of 1); under 2 its block's climb, a new
  stretch's, waits for the cool-down window to about 42, the second healthy pair runs then and
  the broken pair is set aside at about 43 (fix round 7: 65, 66, 74 and 75). With two such new
  broken pairs first, the second fails at 32 (it has no wait), its block ends at about 64, and the
  first healthy pair runs at about 65 (fix round 7: 97; fix round 6: 129). Before fix round 6
  every block's end pushed the healthy pairs back by their own thirty minutes and more: some
  three hours for six, five and a half for ten.
- *Broken pairs at different counts ahead of a healthy pair* (fix round 7; the seventh review's
  cases; times as fix round 8 gives them). A broken pair at four in a row first in the order, a
  second with none and a healthy pair at six: the second fails at 0 (its block ends at about 32);
  the healthy pair runs at about 33 when its latest failure is older than the first broken
  pair's, else that pair fails at 33 (its turn first), its block ends at about 64 and the healthy
  pair runs at about 65 (fix round 7: 65 either way; fix round 6: about 478, the two broken pairs
  taking turns ahead of it). Three broken pairs at two, eight and none ahead of a healthy pair at
  ten: the healthy pair runs at about 65 to 97 by the order of their latest failures (fix round 6:
  about 1100).
- *A broken pair with nothing else due.* Nothing is ever let in, so nothing counts against it and
  it is never set aside, whatever the threshold. It fails at about 0, 3, 14, 26, 40 and 58 minutes
  (each wait from its block's end, each later climb held by the stretch's pace and the ten-minute
  cool-down window), then its wait is thirty minutes and one more for each further failure in a
  row (fix round 5, ruling 3 (i)): at about 90, 123, 157, …, about thirty-five failures in a day's
  window (fifty before fix round 5). Each costs the instance one short run that is given back, one
  block and one climb of the stretch from the application restart (the game stopped and started
  and its startup package run; the emulator restarted at most once in the stretch, "When"), and the
  stretch's attention tells a person after `attention_after_ms`. A known bound, not changed (the
  coordinator's ruling 7 of fix round 5; register §1.494).
- *Every task failing at its entry for hours* (a condition nothing mends). No scheduled run gets
  in, so no failure counts against any pair and nobody is set aside, whatever the threshold. Each
  pair waits after each block's end (one minute doubling to thirty, then one more each, by its own
  count), the stretch's climbs are paced (about six in its first hour, then two an hour) and the
  emulator is restarted at most once in the stretch. At each block's end the pairs take turns a
  minute apart, least recently failed first and the pair whose failure set the block last, at
  its own interval when that is later (fix round 7, ruling 1; fix round 8, ruling 1). When the
  condition ends (a climb recovers, a person's run succeeds, a resume), the pair whose turn is
  first runs a minute after that block's end (later while its own wait still runs); once one
  scheduled run gets in, every other pair's wait ends and they run at once, in the evaluation's
  order.

**The block.** The settlement of a failure at the entry, a condition, then blocks the instance,
under the policy outcome gate the evaluation reads the blocks under: from then
on every candidate of the instance (owed reruns, immediate reruns, remedy requests and the fact
refresh included) waits with `instance_entry_blocked` ([scheduling-blackout.md](scheduling-blackout.md),
"Order of the deferrals", position 4b), but a store update package's task, which passes as it
passes position 4a: an instance whose game asks for an update fails every entry, and only that task
can end it; its success ends the block as any run's does. The block ends when a recovery ladder of
the instance ends `recovered` or `environment_ready`, when any run on the instance succeeds (a
scheduled run, a store update task's, a client's or the Lab's run, a startup package, a rung's run),
when a resume lifts the instance's pause or the global one, when a blackout covers the instance at
the lease sweep's tick or a fence takes it, or at a restart (memory only: the start prepares and
self-checks the instance again, and nothing of the screen is known to be wrong). Anything of these
between the run's start and its settlement leaves no block. A recovery the block held when it ends
is dropped with the record Workflow #489 AVAIL-1 writes for every recovery it drops (fix round 2,
ruling 9): its trigger's `recovery_ladder_suppressed` `cooldown` once more, under the ladder's links,
`until_unix_ms` the time it was held to; so is (fix round 3, ruling 7) a held recovery a newer one
replaces, at each place a recovery is held with a block or for a settlement still to come, and (fix
round 4, ruling 6) a recovery held for a run whose settlement does not block the instance, or whose
settlement finds that something ended a block since the run's start. The recovery's own runs and a
client's or the Lab's run are not scheduled candidates: the block never holds them.

**Where a block's recovery starts and when** (fix round 2, ruling 2; fix round 3, rulings 3, 5 and
6; fix round 4, ruling 4).

- *A stretch* of the instance runs from its first block since a run was last let in on it (a
  scheduled run let in as above, or a run there that is not the Runtime's own startup or rung run
  and succeeded: a scheduled run's, a client's or the Lab's; or since the Runtime started) until the
  next such run: blocks that something ended (a recovery, a resume, a blackout, a fence) and that
  came back with nothing let in are one stretch, measured on the Runtime's clock. Memory only.
- *Where.* A block set when no run was let in on the instance since its previous block ended is
  climbed from the first rung that restarts the application: the first rung, in the rungs the
  instance climbs (its profile's, else the section's), whose kind is `application_restart` or
  `emulator_restart`, the rungs before it skipped `before_start_rung`; whatever ended the previous
  block let nothing in, so the return home that has just failed is not run again. Any other block
  (the first since the Runtime started, one after a run got in, a client's or the Lab's successful
  run included) is climbed from its trigger item's `start_rung`, as configured. A later start the
  item names is kept, and rungs that hold no such kind are climbed as configured. The rung is told
  by its kind, never by a name or a position.
- *When.* The first climb of a stretch starts as before. Every later climb of the same stretch (the
  block's own climb after an exhausted one, an interrupted climb held again, the first climb of a
  later block of the stretch) waits `scheduling.unavailable_probe`'s interval for the number of
  climbs the stretch has had (`first_interval_ms`, times `factor` for each further one, at most
  `max_interval_ms`), counted from the previous climb's end, and never comes before the instance's
  cool-down window ends, **whatever ended the previous climb**: `recovered` or `environment_ready`
  after which nothing was let in, `exhausted`, or an interruption by business capacity, a drain or a
  shutdown. The lease end of a failure whose climb would be such a later one admits no ladder: it
  holds it for the block (the lease sweep takes it up). A person's pause is the one exception:
  nothing climbs under it, and its lifting ends the block. After a panicked worker nothing climbs
  again in the block; after the game's update dialog nothing climbs again in the stretch ("A
  recovery that ended without recovering").
- *The emulator at most once in a stretch* (fix round 4, ruling 4). Once a ladder of the stretch's
  blocks (a blocking run's own ladder or one of a block's own recoveries) has run the rung that
  restarts the emulator, every later climb of that stretch skips it: the rung's
  `recovery_rung_finished` records it `skipped` with the outcome
  `recovery_rung_skipped_emulator_restarted_in_stretch` (the climb's `recovery_ladder_started` plan
  lists it `pending`, as a plan drill's does: no word is added to the ledger's skip reasons, which a
  v0.12.76 reader would refuse), so the climb ends after the application's restart, `exhausted`
  when the startup package does not reach its page. A new stretch (after a run was let in) may
  restart the emulator once again; a restart of the Runtime starts with no stretch.
- *What the lease end reads* (ruling 6). The failure's lease end knows whether its settlement will
  judge the failure at the entry from what the host already holds of the run in memory, and reads no
  ledger: the code's class, the run's chain (its scheduled resolution), and the run's own entry facts
  as its control noted them when they were recorded (an `EntryRecoveryDecision`: it checked its
  entry; an `EntryTargetDisposition` `Started`: its main package started), the code's accumulation
  judged outside the restart segment, which only makes more failures accumulate. So it never says
  "at the entry" of a failure its settlement settles otherwise; nothing is read, and nothing can fail
  to be read, under the instance's admission lock. A ladder its lease end admits at once starts at
  the application restart when the block will; a held one is started there when it is taken up.
- *A night of a condition nothing mends* (shipped values: `first_interval_ms` 60 000, `factor` 2,
  `max_interval_ms` 1 800 000, `recovery_ladder.cooldown_secs` 600; climbs that end within about two
  minutes). The stretch's climbs start at about 0, 10, 20, 30 and 40 minutes (the cool-down window,
  ten minutes from each climb's start, is the longer wait while the interval is one to eight
  minutes) and 58 minutes (sixteen minutes after the fifth's end): six in the first hour; then every
  thirty minutes after the previous climb's end: one in the second hour (about 1:30), two in every
  hour after (about 2:02, 2:34, 3:06, …), some twenty in eight hours. The first climb runs as
  configured: the return-home package, then (failing) the application's stop, start and startup
  package, then (on an emulator with control) the emulator's stop and start and the startup package:
  the one emulator restart of the night (fix round 4, ruling 4). Every later one starts at the
  application restart: it stops and starts the game and runs the startup package, and ends there,
  `exhausted` when that fails, the emulator's rung recorded skipped
  (`recovery_rung_skipped_emulator_restarted_in_stretch`; a handset has the first two rungs only). A climb that ends `recovered` (a startup package that reaches its
  page while the condition stays) ends the block; the next due task then fails at its entry, a
  condition, and its block's climb is paced the same way; each pair waits after its failures and
  none is set aside, nothing being let in. A person is told once per block whose climb ended
  `exhausted` and once per stretch after `attention_after_ms` (an hour); no rule is paused.

**The way out.** No block without an end and without a person told:

- *Its trigger.* The failure's lease end entered the ladder as a failed run's does (but for a later
  climb of a stretch, held as above). For a failure `exclude_codes` keeps from starting a ladder by
  itself the lease end starts none, but prepares the ladder of the block's own trigger (the run's
  trigger, the item the class's matching gives with the exclusions not read, `pack_logic` with the
  shipped triggers) and keeps it for the run's settlement, which takes it as the block's held
  recovery, due at once (or at the stretch's paced time), or lets it go when the run did not fail
  at its entry. A recovery held for a run whose settlement is still to come is kept by that run, so
  no other run's (a manual run's) ladder displaces it, and none is kept for a run that settles
  nothing (a direct run). `exclude_codes` keeps excluding a code from starting a ladder at its lease
  end: an excluded code failing after the main package started starts no ladder at all.
- *Held, then taken up.* A ladder its cool-down suppressed is held with the block and admitted when
  the cool-down ends, as without the cool-down, by the lease sweep's step after the unavailable
  instances' probes (Workflow #489 AVAIL-1's mechanism, a new request, causation, holder and claim
  for it); the window in force is read again then, and a later window (another ladder's climb, a
  `triggered` admission) keeps one that respects the cool-down held until its end. A recovery
  AVAIL-1 held for an instance unavailable by its device self-check is taken up by AVAIL-1's probe;
  when the instance leaves that set (a probe passed) the block takes the recovery over instead of
  dropping it. While the instance is unavailable its candidates answer `instance_unavailable`,
  which the evaluation gives at placement, before the order of the deferrals. While a pause or an
  idle stop owns the instance, a held recovery stays held, nothing recorded (a resume ends the
  block; the idle wake lets it be taken up).
- *A blackout.* A blackout that covers a blocked instance ends the block at the lease sweep's next
  tick, whatever its recovery is doing, and its held recovery is dropped (recorded as above):
  nothing is dispatched during the blackout, and after it the first due task tries its entry anew
  and, failing, blocks the instance and triggers again; SR-4a3 is kept (no ladder of an earlier
  trigger after the blackout). A recovery whose admission or climb a covering blackout suppressed
  ends the block at once; one a blackout only coming within its rung's time limit stopped is held
  until that blackout's end, and the block ends once the blackout covers the instance.
- *A transient obstacle.* A recovery refused by the admission's capacity refusal (business
  capacity from the performance sample), by another ladder that holds or waits for the instance
  (`already_running`), or by a later preparation that superseded its own, and a climb a holding
  interruption ended (a pause, business capacity, a drain or a shutdown: `recovery_ladder_finished`
  `exhausted` at Warning with `recovery_ladder_interrupted`), is no failure of the recovery: the
  block stays, the recovery stays held and its admission is tried again once nothing holds the
  instance and, for an interrupted climb, as a later climb of the stretch ("When"); no person is
  told of an interruption. The sweep judges capacity and another ladder before it tries, so nothing
  is recorded while either refuses; a held preparation's ladder is admitted naming the instance's
  latest preparation (AVAIL-1), so the supersession it met is not met again; so none of them records
  per tick, and none warns by itself.
- *A claim the queue refuses* (fix round 2, ruling 7). Tried again no more often than once a second
  at first, the interval doubling at each refusal in the block to at most once a minute, while the
  block lasts; the refusal is recorded (its `runtime.failed` lifecycle record, as before ENTRY-A)
  when it first occurs in the block and when its cause (the refusal's code) changes, none per
  retry. Each try is a claim request of the queue, which writes its own request and denial
  (`lease.requested`, `scheduler.denied`), so at the longest interval the queue's trace of a refusal
  that does not end is two records a minute.
- *A fence.* A fence that takes the instance ends the block and drops its held recovery: the fence
  and its own recheck own the instance from there.
- *A recovery that ended without recovering* (fix round 2, ruling 5; fix round 3, rulings 3 and 4).
  One of the block's own recoveries (the blocking runs' lease ends' ladders, the recoveries the block
  held, took over or admitted) that ends without recovering leaves the instance blocked and brings it
  to a person's attention once per block: `instance_entry_blocked_recovery_failed` (a warning
  `runtime.failed` record under the instance, handed to the daemon's log), with how it ended
  (`entry_recovery_end`), the trigger item the ladder was admitted under (`trigger_rule`, when it had
  one) and, when another climb comes, its earliest time (`next_occurrence_unix_ms`). `exhausted` (its
  rungs ran and failed, or a busy or screen hold's limit stopped it): **the block tries again by
  itself**, its next climb starting at the application restart and paced as every later climb of the
  stretch ("When"), and so on while the block lasts. `update_dialog` (the game's update dialog ended
  a rung's run, UPD-1, which ends the ladder `exhausted` at Warning with `recovery_ladder_interrupted`):
  the block's ladder is not climbed again in that stretch (a held recovery is let go, recorded), no
  `next_occurrence_unix_ms`; the store update task passes the block as it passes position 4a, and its
  success ends the block. On an installation whose catalogue has no store update task nothing ends
  that block by itself: the instance stays blocked (position 4a holds it too, the update fact being
  true), nothing touches the device again, and the person told is told once more by the stretch's
  attention after `attention_after_ms`; a person updates the game and runs a task or resumes the
  instance (a resume ends the block), or the Runtime's restart forgets it. `worker_panicked`: no
  `next_occurrence_unix_ms`, and nothing climbs again in that block, as AVAIL-1's probe after a panic
  (a resume, a run's success or a restart ends the block). `worker_failed` (#503 ISO-A fix round 1,
  Runtime 0.12.102): the work that climbed the ladder ended with an error its instance worker
  contained, no panic; the error's own record under the same links says which, and the block does
  exactly what it does after `worker_panicked` (the climb not counted in the stretch, a held
  recovery let go, nothing climbing again in that block). A scheduled run's own recovery that ended
  so before the run's settlement does the same with the block that settlement sets; a ladder of
  another trigger does not make the block warn. No configuration key is added.
- *The emulator's restarts.* A climb that reaches the `emulator_restart` rung stops and starts the
  emulator through the control owner on its key; it neither adds to nor clears the emulator watch's
  bound (`emulator_control.restart_attempts` within `restart_window_ms`, which counts the watch's own
  restarts, and which a person's emulator control clears). Since fix round 4 (ruling 4) a stretch
  restarts the emulator at most once, so a night of a condition nothing mends restarts it once,
  where v0.12.76 restarted it at most a few times and fix round 3's head about twenty times. The
  stretch's climbs are bounded by their pace and the instance's cool-down window
  (`recovery_ladder.cooldown_secs`): with the shipped values about six in the first hour of a
  stretch and two an hour from the third hour on ("A night of a condition nothing mends"), each
  recorded (`recovery_ladder_started`, `recovery_ladder_finished`), a person told once per block and
  once per stretch. A night of them therefore uses up no restart bound and leaves no instance that
  nothing brings back and nobody is told of.
- *The attention* (fix round 2, ruling 6). An instance blocked in a stretch ("A stretch") that
  lasted `scheduling.unavailable_probe.attention_after_ms` is brought to a person's attention once
  per stretch, whatever holds it, a recovery still held or climbing included:
  `instance_entry_blocked_awaiting_person` (the same kind of record), with `elapsed_ms` the
  stretch's length and `limit_ms` the threshold. As AVAIL-1's attention for an unavailable
  instance, it is not raised while a pause (the instance's own or the global one), a fence or an
  idle stop owns the instance; its clock runs on, so it is raised at the first tick nothing owns the
  instance and the instance is still blocked. The one key serves AVAIL-1's unavailable instances
  and the entry blocks; no key is added.
- *Both.* One code, one cause: `instance_entry_blocked_recovery_failed` at most once per block and
  `instance_entry_blocked_awaiting_person` at most once per stretch, each when its own condition
  first holds, so a block may raise both, in the order their conditions came.

**Status.** No member of the status is added (a v0.12.75 reader refuses a status with a member it
does not know). The not-running row of `ScheduleExplanation` carries it: its `why` is
`instance_entry_blocked` with the blocking run's `task_id`, `decision_id`, `failure_code` and
`observed_at_unix_ms`, and its `since_unix_ms` the first evaluation that deferred the instance so.
A pair that waits or is set aside shows on its candidate rows (`instance_entry_task_waiting`,
`instance_entry_task_set_aside`, with until when); no member of any answer is added for them.

## A failure that pauses at its first occurrence (Workflow #463 U1)

Since 0.12.60 (register §1.372; the owner's ruling on board #463: the code tells an auditing agent
that a new result stopped the Runtime, and it affects no other task). A scheduled run that ends
with `contained_task_linear_unrecorded_screen` (a linear step met a screen its package never
recorded, [linear-steps.md](linear-steps.md), "Failure codes"; the kernel raises it only in the
`main` scope of a task's own run, outside the restart segment, never in a run the host starts
itself, a startup package or a ladder rung, so no such run can pause a pair) pauses its pair
(catalog task, instance) at its first occurrence, whatever the pair's `on_failure` says:

- the failure point marks its task terminal `failure_severe` (the existing member of
  `TaskSemanticFact::TerminalCommitted`), so the policy class is `severe` for the live settlement,
  the startup reconciliation and the ledger's re-derivation alike (`scheduled_failure_class`), and
  the settlement writes `paused_task` with `retry_attempt` 0 and no retry time: no immediate rerun
  (R22), and no escalation count is needed. The record's severity stays its outcome's category
  (`error`);
- the failure identity is written as for an accumulating failure ("Failure identity": its `F` the
  run's error frame, its main package and layers with the content hashes measured now);
- it lifts as every policy suspension does ("Lifting (R19)"): the main package's content (or a
  layer's the identity names) changed, at the pair's next due evaluation after the lift check's
  interval. No manual lift is added.

The code's ladder class is `pack_logic_mismatch` (the owner's ruling on board #463: the ladder
starts at once), so under the default triggers it starts the stuck-recovery ladder through
`pack_logic` as `page_confirmation_failed` does; the ladder and the pause are two separate
consequences, and the order on the instance is the code, the pair's pause, the ladder, then the
next due pair. Nothing else changes: the pause neither pauses nor fences the instance, no other
pair's admission, order or timing moves, the next due pair is dispatched as before (its entry gate
does what it did). Every other failure code follows `on_failure` exactly as before. U1 added no
configuration key, ledger event type, enum value or persisted member: the rule is code, as
UPD-1's is. A direct run of the package fails with the same code and pauses nothing.

**Only during a package's break-in** (Workflow #463 U1b, since 0.12.63, register §1.389; the
owner's ruling on board #463: the code and the pause at once only within a period after a package
is first run, the ordinary failure handling otherwise; the numbers and the scope are the
coordinator's proposal the owner agreed to). A scheduled run raises the code only while its pair
(catalog task, instance) is in the break-in of the content of the package it runs:

- **The content.** The main package's content hash (above, "Content hash"), its first 12 hex
  digits as the failure identity's `m` segment writes them, read when a scheduled run of a
  `linear_steps` main package is prepared, at the path the run reads it from, through the lift
  checks' memory. A page-graph package has none and counts nothing (its run never meets that
  verdict). A `linear_steps` package whose content is not measured then has none either (fix round
  3): its run counts nothing and is judged as one outside its break-in ("A break-in that cannot be
  read").
- **The break-in.** It starts at the first run of that content on that pair that began its
  package (its kernel run's `task.package_admitted` is recorded) and was settled as a success or
  as a failure, counts every such run of the same content, and ends once both hold: at least
  `scheduling.suspension.break_in_runs` such runs (default 20) and at least
  `scheduling.suspension.break_in_ms` since the first of them settled (default 604 800 000 ms,
  seven days). Neither starts nor counts it: a stopped run ("A stopped run"), a condition end
  ("Condition termination and owed reruns"), and a run that never began its package (fix round
  2: a refused preparation or outcome declaration, a failed switch before the run, a refusal of
  the handset's start gate, a prerequisite layer of the run's entry gate that failed, an
  interrupted settlement), which is no run of the package. A run
  settled with another content starts a new break-in, that run its first. Both keys are settable
  and read before each scheduled run executes, so a change counts from the next run
  ([runtime-config.md](runtime-config.md)).
- **The verdict.** Before a scheduled run executes, the host judges, on the Runtime clock, whether
  its pair is past the break-in of the content the run was prepared with, and tells the run
  which code a closing decision that finds a screen the package never recorded ends with
  ([linear-steps.md](linear-steps.md), "A screen the package never recorded"). Inside, nothing
  changes: this code with `confirmation_problem` `unrecorded_screen`, the pause at its first
  occurrence with `pause_cause` `unrecorded_screen` ("Why a pair is paused"), the lift by a
  content change. After it, the run ends with `page_confirmation_failed`, with that code's detail
  and timing for the same end and the same word `confirmation_problem` `unrecorded_screen`, and
  is settled as that code is (the ordinary repeat rule and `on_failure`; no pause at its first
  occurrence unless the task is marked sensitive, as for any recoverable failure; a pause it
  reaches says `repeated_failure`, `retries_exhausted` or `sensitive_task`; its ladder class is
  the same); the closing decision is still made with the same conditions, and
  the frame of the run's `Failure` pin, the decision's last, is still named as `artifact_id` on
  the terminal, the receipt and the recorded failure. A direct run and a Lab run have no pair and
  raise this code always.
- **A break-in that cannot be read** (fix rounds 2 and 3). A run is never ended, and its
  preparation never fails, because the host could not establish its break-in; every such failure
  falls on one side: the run goes on as a run outside its break-in (the ordinary code above), and
  before it executes the host records `policy_break_in_unread` (warning; a Warning
  `runtime.failed` with the run's links, `task_id`, `instance_id`, the failure's code as
  `failure_code` when registered and the failure as its `caused_by` link) with the required word
  `break_in_problem`:
  - `content_unmeasured`: the package's content was not measured when the run was prepared: the
    containment could not measure the package (its code as `failure_code` when its refusal names
    one), or the Runtime clock, `scheduling.suspension.lift_check_min_interval_ms` or the lift
    checks' memory could not be read. The run counts in no break-in.
  - `record_unread`: the pair's break-in could not be judged before the run executed: the two
    keys, the pair's break-in in the decision memory (its dispatch not held) or the Runtime clock
    could not be read. The run counts as any run of its content does.

  What the Runtime treats as fatal wherever it meets it is no failure of the break-in and is not
  softened here: a lock of the host's own run state that another thread left broken (the
  scheduled runs' resolutions, the policy's state) and a failure to write that record end the run
  and stop the Runtime, as they do at every other place.
- **Where it is kept.** In the pair's value record `policy.control_state_recorded` (register L7),
  the optional member `break_in: {content_hash, runs, first_settled_at_unix_ms}`, written by the
  live settlement that counts a run; nothing else is persisted. A loading start loads it with the
  rest of the decision memory; a seeding start ([ledger-observer.md](ledger-observer.md) §11.5,
  "Without the marker": a root's second start, whose first wrote no marker), which rebuilds the
  decision memory from the dispatch records, takes each pair's from its latest value record, since
  no dispatch record holds it (fix round 2). A run a start's or an abandoned worker's
  reconciliation settles has no preparation of this process and counts nothing.
- **A package that ran before this build.** Its pair's records hold no break-in: the records
  0.12.62 and earlier wrote do not say which content a successful run ran, so the Runtime cannot
  tell content that has run for weeks from content changed yesterday. As built, such a pair is
  inside its break-in from its first run on this build: from that run on, until it has settled
  `scheduling.suspension.break_in_runs` runs (20) that began the package and
  `scheduling.suspension.break_in_ms` (seven days) have passed since the first of them, a screen
  the package never recorded ends its scheduled run with this code and pauses the pair at once,
  until the package's content changes (a changed content starts a new break-in). While the pair
  is paused it runs nothing, so its count does not grow; the seven days do pass. Once both are
  reached, the same screen ends the run with `page_confirmation_failed` and the ordinary handling.
- **Where a person sees it.** `actingd suspended`'s list `break_ins` (below).

A pair paused by this code shows in `actingd suspended` under `suspended` (`failure` with the
code and its values, `artifact_id` among them, `pause_cause` `unrecorded_screen`, `previous`
`null`, `effective_class` `severe`, `lifts_when` `content_hash_changes`), in `ac_diagnose` (its
`suspended` rows and `failures_grouped` by the code), and in `ac_schedule` as a candidate row
whose `why` is `policy_task_paused`.

## Why a pair is paused (Workflow #469 H2)

Since 0.12.62 (register §1.398; the owner's option A on board #469). A paused pair's
`policy_task_paused` carries `pause_cause`, the word of why its settlement paused it, and
`failure_code`, the code of the failure it paused on (the record's failure code, "Replay"; left out
for a relayed spelling the catalog does not register). A pair held by its retry backoff carries
`failure_code` on `policy_retry_backoff_active` the same way, and no `pause_cause`. Both are values
of the outcome only: the reason's detail, the record, its failure identity and the streak's
comparison do not change, and nothing decides by them (when a pair is paused or lifted is as
before). The word is read from the execution record the settlement wrote, in the settlement's own
order, the first that holds:

| `pause_cause` | When |
|---|---|
| `runtime_budget_exceeded` | the original class is `severe` and the failure is the runtime-budget rewrite `policy_runtime_budget_exceeded` |
| `interrupted_again` | the original class is `severe` and the failure is `policy_settlement_interrupted` (only the second in a row holds the pair, "Lifting (R19)") |
| `unrecorded_screen` | the original class is `severe` and the failure is `contained_task_linear_unrecorded_screen` ("A failure that pauses at its first occurrence") |
| `severe_failure` | any other failure whose original class is `severe` |
| `sensitive_task` | the task is `sensitive` |
| `repeated_failure` | the effective class is `severe`: the streak reached `escalation_threshold` |
| `retries_exhausted` | the effective class is `recoverable`: the retries are used up and `on_failure.action` is `pause` |

The word is memory only, as the packages a pause is lifted against are: the live settlement and a
replay set it from the record, and a start that loads the pair's streak from its value record
(`ledger-observer.md` §11) takes it from the execution record of the streak's dispatch, which the
dispatch journal holds; no persisted member is added. `actingd suspended` computes the same word
from the same record for the pair's `suspended` row (`pause_cause`), and `ac_schedule`, the not-run
rows of `ac_diagnose` and a `policy.dispatch_rejected` record show the outcome with its values.
A pause carries no `pause_cause` when the dispatch journal holds no execution record of the
streak's dispatch that names its failure, which can be on a start that loaded the streak: that
start's journal holds the dispatches the pair's value record names, and once the pause was lifted,
a run that a condition ended leaves the streak as it was ("Condition termination and owed reruns
(Workflow #386 SR-2)", "Settlement") while the value record names that run, not the paused one.

## Settlement of an accumulating failure

| Previous identity of the pair | Written | Count |
|---|---|---|
| none, another format, a unique one, or another prefix | the prefix with this run's frame | 1 |
| same prefix, error frames different | the prefix with this run's frame | 1 |
| same prefix, error frames similar or not comparable | the previous identity unchanged | +1 |

A comparison that cannot be made counts as the same problem, since the prefix already matched.
In the `main` scope it also writes, before the execution record, a Warning `runtime.failed`
`policy_failure_frame_compare_unavailable` (operation `compare_failure_frames`) whose native
detail is `reason=<frame_missing|artifact_unreadable|png_invalid> previous_run=<run>
current_run=<run>`. The `prepare` scope has no frames and writes nothing.

When the scheduled path's record of the run is missing (an internal error), no failure
identity is written, together with a Warning `runtime.failed`
`policy_failure_identity_unavailable`.

### Error frames

A run's error frame is the capture frame artifact (`ArtifactVerified`, kind `capture_frame`)
of the frame of its latest `CaptureCompleted`. The previous run is the run of the pair's latest
failed dispatch. Frames are read and verified read-only. A failed run's frames are not kept
permanently: they follow the frame classes of `contracts/ledger-store.md` (Workflow #375), and
a frame no longer present makes the comparison unavailable (`frame_missing`, above).

Both frames are decoded as PNG and digested whole with `color_digest.v1` on a grid of
`min(32, width)` by `min(18, height)` cells. A cell has changed when `|dR| + |dG| + |dB|` of
its quantized channels exceeds `CELL_DELTA = 6`. The frames are different when more than
`MAX_CHANGED_MILLI = 250` changed cells per thousand, or when their sizes differ; otherwise
similar. The digest mean distance and the template correlation coefficient are only displayed.

## Replay

The count is unchanged: two failures share a streak only when their failure codes, failure
identities and classes are equal (the outcomes' other values are not compared). Replay hands
the recorded outcome and identity to that count and requires the recorded execution to come
out again; it never compares frames. A settlement that finds an execution already recorded for
its dispatch writes nothing new and keeps the recorded failure, after checking that its code is
the terminal's and its identity's base is the base of the terminal's code
(`policy_execution_identity_conflict` otherwise).

## Lifting (R19)

A paused pair is admitted again, by the evaluation and by admission, once a lift check finds
that any of these differs from what its failure identity recorded (Workflow #336 PKG-S3; the
recorded hashes come from the identity alone, never from the dispatch, so a restart rebuilds
them):

- the content hash of the main package, measured at the location its procedure's binding names
  (`scheduled_execution.package_path`);
- for a `p` layer, the content hash of the package the standard-pack registries now list
  under its id hash (Workflow #373 NAV-3a; `x` when none; changed when several packages share
  the hash);
- for the `r` layer, the registry's return-home package of its game and server (missing or
  another id is a change), then its content hash as for a `p` layer.

A lift check runs when the paused pair is due at an evaluation and again at its admission. It
measures one package at most once per `scheduling.suspension.lift_check_min_interval_ms`
(default 60 000 ms, [runtime-config.md](runtime-config.md)) and judges by that measurement until
the interval has passed, so a package changed in place lifts the pause at the pair's first due
evaluation after it, without a restart and without a configuration change. A package that
cannot be measured (removed, being replaced, unreadable, or a procedure whose binding names no
location) lifts nothing: the check keeps the pause, links a Warning
`lift_check_hash_unmeasured` (`path`, `package_id` for a layer, `caused_by` the read's refusal)
as a note of the `policy_task_paused` reason, and measures again at a later check. A recorded
`x` differs from every measured hash, so a package the settlement could not measure lifts the
pause once a check measures it. A pause recorded without a failure identity (an exceeded
runtime budget) names no package: its main package is compared with the hash
the pair's first lift check measured. A procedure that is no longer bound never lifts. The
registries are read at each lift check (once per check; `actingd suspended` reads the root its
configuration names), so a standard pack replaced in place counts at the next check. An interrupted settlement
(`policy_settlement_interrupted`) is lifted without a package update (Workflow #361 M6) when
it is the first of its pair in a row: the next dispatch of the pair is admitted as usual,
within its loop and activity budgets (the interrupted admission already counted against them).
A second one in a row is lifted by a package update only. The count is rebuilt from the
execution records, so nothing persisted changes. The next dispatch is admitted as usual;
a task that fails again starts a new streak, since a recorded hash changed. The first
interrupted settlement in a row is lifted at the next start of this Runtime. There is
no manual lift; `actingctl task-run` is not gated by the policy and does not lift a
suspension.

What such a failure leaves behind for a clock (#477 CK-2, since 0.12.81;
[v012-register.md](v012-register.md) §1.516; [scheduling/README.md](scheduling/README.md),
"Clock Sources"): when the pair's latest dispatch settled as interrupted (its run started or
not), or failed and paused its pair, whatever the failure's class (every `pause_cause` but
`sensitive_task`: a screen the package never recorded, the common case, included; not a run over
its runtime budget that reported its success), and the task is not `sensitive`, the clock
occurrence it answered stays owed, until the activity day that dispatch's admission was counted in
ends. So the pair is due on it while it is held: its
`policy_task_paused` is its deferral at every evaluation (a lift check each time, measuring a
package at most once per interval, as above), and its occurrence is recorded refused once. A
lift, or the first interrupted settlement in a row, which holds nothing, lets it run on that same
occurrence at the next evaluation instead of at the next occurrence: a package repaired after a
screen it never recorded runs the same day. A lift in a later activity day finds the occurrence
served and waits for the next one. A sensitive task's failure or interrupted settlement, which
pause too, and a failure scheduled for a retry or continued serve their occurrence as before:
after a sensitive task's lift, and after the restart that lifts its first interruption, the pair
waits for its next occurrence.

Workflow #490 FR-1 (since 0.12.81, register §1.517): **a pair of the fact refresh is never held
by a pause.** A run of the reserved task `_fact_refresh` ([scheduling/README.md](scheduling/README.md),
"Fact refresh") settles as every run does: a severe failure, the first one included, records
`paused_task` as before, and the failure counts once in the fact refresh's failed runs in a row.
But the evaluation and the admission admit such a pair at every check whatever its settlement
recorded: no lift check runs for it, no `lift_check_hash_unmeasured` is linked, and no
`policy_task_paused` names it. Its rest after failures is the fact refresh's own gate alone
("Its own gates": a rest that grows with its failures in a row and ends by itself). Nothing
persisted changes: a ledger that holds such a pause, written before 0.12.81, replays as before
(the settlement's records and the decision memory are the same) and admits the fact refresh at
the first evaluation after the start. Every other task's pause and its lifting are unchanged.

## `actingd suspended`

```text
actingd suspended --config <path> [--deadline-ms <ms>]
```

Loads and assembles the configuration exactly as startup does, then opens the state root's
ledger read-only and without the owner lock, so it runs beside a running daemon.

ROUT-L RL-1c fix 1 (#411 H1): the ledger read is bounded and never reads the whole table. It reads
the record types the report lists by the ledger's type index (`policy.dispatch_intent`,
`policy.execution_recorded`, `policy.dispatch_admitted`, `policy.planning_signal_observed`,
`task.failed`, `runtime.failed`; RL-1c fix 2, #411 M2: no longer `runtime.lifecycle_observed`,
and of `runtime.fact_recorded` only the newest `config.subsystems` fact, read newest first through
the type index in the same read), the ledger's latest time (the row of its maximum
`timestamp_unix_ms`, through the time index) and its head row (the last record), and a
run's own rows (its entry, step, application intent, capture and verified-artifact records) only
for the runs of each pair's last two executions it shows, each over the rows after the run's
dispatch intent and through its execution record. `--deadline-ms` is the caller's deadline,
counted from the command's start: the read stops there, and the report then answers with what it
read and the warning `ledger_read_incomplete` (before the selected records are read: no rows and
`through_sequence` `null`; a run left unread: its `step` is `null` and its `frames` comparison is
`unavailable` with `ledger_read_budget_exceeded`). `actingctl diagnose` and `ac_diagnose` pass
their call's deadline less 2 s and end the child still running at the deadline
(`suspended_report_timeout`). RL-1c follow-ups (#411 L1, L2 and review 2 L1; since 0.12.25):
- without `--deadline-ms` (a run by hand) the read's deadline is 60 s from the command's start;
- each run shown is read once, by one range read that is itself checked against the deadline
  (its extent check, every row it reads and the head row), so a run whose read the deadline
  passes in is left unread as above instead of running on until the caller ends the child;
- the walk for the latest `config.subsystems` fact is read after every other row of the
  selection and reads at most 32 768 `runtime.fact_recorded` rows; when it ends there or at the
  deadline before it found the fact, `daemon_started_at_unix_ms` is `null` with
  `daemon_start_unknown` and `ledger_read_incomplete`, and every other part of the report answers
  from what was read;
- the 60 s, the 2 s kept back by `diagnose` and the 32 768 rows are tunables that become
  configuration keys at the next configuration revision.

Since
Workflow #336 PKG-S3 it reads the packages a paused pair names, read-only, to measure their
content hashes now, each package once per report. Nothing is written.

ROUT-L RL-1c (#411; v0.12.20): in composer mode (a `policy` section without `catalog`) the
report is judged against what the configuration schedules: the composition the scheduler
workbench makes for the configured instances from the game roots below `standard_packs.root`
(its catalog in place of the configured one, its procedure bindings, each `package.<package_id>`
at its package's place below its game root, in place of `policy.procedure_manifest`), with the
same members. A game whose files are refused is left out of the composition, so its pairs carry
`task_not_in_catalog`; a composition refused as a whole is `policy_catalog_compile_failed`
(`operation` `read_suspension_config`) caused by each of its problems. Only a configuration
without a `policy` section is `suspended_policy_unconfigured`.

stdout is one JSON line, the v2 report (Workflow #378 A6a): `schema_version`, `outcome` and the
report's members. `outcome` is the full `actingcommand.outcome.v1` envelope of
`suspended_reported` (success); with `ACTINGCOMMAND_DETAIL=codes` in the environment it keeps
only its registered names (`"detail":"codes"`).

```json
{"schema_version":"actingcommand.actingd.suspended.v2",
 "outcome":{"schema_version":"actingcommand.outcome.v1","code":"suspended_reported","category":"success","values":{}},
 "config_path":"...","state_root":"...","through_sequence":0,"daemon_started_at_unix_ms":0,
 "suspended":[{"task_id":"...","instance_id":"...","procedure_ref":"...",
   "failure":{"code":"...","category":"error","values":{}},"failure_identity":"...",
   "run_failure":{"code":"...","category":"error","values":{}},"consecutive_same_error":2,
   "effective_class":"severe","pause_cause":"repeated_failure","severity":"error",
   "paused":{"execution_sequence":0,"observed_at_unix_ms":0,"decision_id":"...","run_id":"..."},
   "previous":{"execution_sequence":0,"disposition":"retry_scheduled","run_id":"..."},
   "step":{"scope":"main","operation_label":"..."},
   "detail":"...",
   "package":{"paused_hash_prefix":"...","current_hash_prefix":"...",
     "layers":[{"kind":"return_home","package_id":"...",
     "paused_hash_prefix":"...","current_hash_prefix":"...","changed":false}]},
   "frames":{"previous":{"frame_id":"...","artifact_sha256":"..."},"current":{},
     "comparison":{"source":"recomputed","status":"similar","outcome":null,
       "changed_cells_milli":0,"digest_mean_milli":0,"ccoeff":1.0,"ccoeff_failure":null}},
   "lifts_when":"content_hash_changes",
   "takeover":"actingctl task-run --state-root <root> --instance <instance> --package <package>"}],
 "lifted":[{"task_id":"...","instance_id":"...","lifted_by":"main_hash|layer:<n>|restart",
   "effective":"next_evaluation|active","paused":{},"package":{}}],
 "repeating":[{"task_id":"...","instance_id":"...","failure":{},"failure_identity":"...",
   "run_failure":{},"step":{},"latest":{"execution_sequence":0,"run_id":"..."},
   "previous":{"execution_sequence":0,"run_id":"..."},"detail":null}],
 "warnings":["config_newer_than_daemon_start"],
 "clock_slots":[{"code":"clock_slot_refused","task_id":"...","instance_id":"...",
   "gate_code":"policy_budget_exhausted","budget_dimension":"task_daily",
   "occurrence_unix_ms":0,"window_end_unix_ms":0,"next_eligible_unix_ms":0,"next_occurrence_unix_ms":0,
   "slot_fate":"next_window","slot_fate_settled":true,
   "recorded_sequence":0,"recorded_at_unix_ms":0,"next_dispatch":{"sequence":0,"admitted_at_unix_ms":0}}],
 "packs":[{"package_id":"...","failure_code":"recognition_sample_budget_invalid",
   "runtime_commit":"...","paused_hash_prefix":"...","current_hash_prefix":"...",
   "instances":["..."]}],
 "remedies":[{"code":"policy_remedy_unchosen","remedy_problem":"no_choice","task_id":"...",
   "instance_id":"...","outcome_key":"...","need":"...","recorded_sequence":0,
   "recorded_at_unix_ms":0,"lifted":{"sequence":0,"at_unix_ms":0}}],
 "held":[{"code":"task_held","task_id":"...","instance_id":"...","gate_code":"policy_no_progress",
   "held_state":"due","held_since_unix_ms":0,"held_for_ms":1800000,"held_after_ms":1800000,
   "recorded_sequence":0,"recorded_at_unix_ms":0,
   "ended":{"sequence":0,"at_unix_ms":0,"held_end":"selected","held_for_ms":2400000}}]}
```

- `suspended`: each pair whose latest execution paused it and is not lifted. `previous` is the
  failed execution it repeats, `null` when it paused at its first failure. `step` is re-derived
  from the run's rows for a failure identity (`null` for another code). Workflow #378 A6a
  replaces v1's `error_code` and `failure_code` by three members: `failure` is the failure the
  execution recorded, an outcome in record form (`{code, category, values, causes?}`);
  `failure_identity` is its failure identity, `null` when it has none; `run_failure` is the code
  of the run's failure terminal as an outcome, else the failure identity's base (the whole
  identity when it does not parse, the recorded failure's code when there is none); a spelling
  the catalog does not list becomes `outcome_relay_unregistered`. `detail` is the native detail
  of the run's lifecycle failure records of the terminal's code (else of the recorded failure's
  code): since ledger revision 3 the `raw_text` of a direct link of a record's outcome, from the
  latest record that has one, or `null`; it is text. The frames are compared again (`source`
  `recomputed`); a recomputed
  `different` for an accumulating identity adds the warning
  `comparison_disagrees_with_record:<task>/<instance>`. The comparison's `outcome` (v1's
  `reason`) says why the frames differ (`size_changed`, `digest_cells_changed`; `null` when they
  are similar) or, with `status` `unavailable`, why they could not be compared:
  `suspended_frame_missing` (a frame is absent), `suspended_frame_unreadable` (caused by the
  artifact read's failure) or `png_invalid` (the decoder's text as its `foreign_data_error`
  link). `ccoeff_failure` (v1's `ccoeff_error`) is the `png_invalid` outcome of a correlation
  coefficient that could not be computed, else `null`. Workflow #469 H2 (since 0.12.62):
  `pause_cause` is the word of why the execution paused the pair, from the execution record, as
  the pair's `policy_task_paused` carries it ("Why a pair is paused").
- `package`: the content hash prefixes the pause recorded (`paused_hash_prefix`, `x` when the
  settlement could not measure the package, `null` for a pause without a failure identity) and
  the ones measured now (`current_hash_prefix`, `null` when the package cannot be measured or
  the procedure is not bound); a layer is `changed` only when it was measured now and differs.
- `lifted`: each pair paused by its latest execution whose suspension a content hash change
  lifts and that was not dispatched again. `effective` is `next_evaluation`: the running daemon
  lifts it at its next check of the pair, without a restart. The first interrupted settlement of a
  pair in a row is listed here with `lifted_by` `restart` and `effective` `active` (Workflow #361
  M6); a second one in a row is listed under `suspended` like any other pause.
- Workflow #490 FR-1 (since 0.12.81): a pair of the fact refresh (`_fact_refresh`) is never held
  ("Lifting (R19)"), so an execution of it that recorded a pause is listed neither under
  `suspended` nor under `lifted` and gives no `task_not_in_catalog` warning; it is judged as any
  other failure (under `repeating` when it repeats).
- `repeating`: each pair whose latest execution is a failure that does not accumulate and whose
  previous execution failed with the same identity prefix: a prerequisite or return-home
  package, the restart segment or the device failing again and again.
- `daemon_started_at_unix_ms`: the observation time of the ledger's latest `config.subsystems`
  fact, recorded at every start (and only then; a `config.parameters.<section>` is also recorded
  when an instruction changes a setting, Workflow #387 D1b). Warnings: `config_newer_than_daemon_start` when the
  configuration file changed after it, `daemon_start_unknown` without it,
  `error_code_unparsed:<task>/<instance>` for a `~v2~` failure identity that does not
  parse (its pause is then judged by its main package alone),
  `lift_check_hash_unmeasured:<task>/<instance>` when a package the pause names cannot be
  measured now, `task_not_in_catalog:<task>/<instance>`.
- `warnings` is text in v0.12.0, not outcomes: Workflow #378 A6a turned the rest of the report
  into outcomes and left this list as v1 printed it. Each entry is a developer string, a token
  alone or followed by `:<task>/<instance>`: in composer mode first each problem of the
  composition as `<code>:<game>:sha256:<hex>` (RL-1c fix 1, #411 M1: the code, e.g.
  `standard_pack_root_invalid`, the game, and the sha256 of the `schedule.json` read for it,
  `unread` when none was read; it says why a game has no rows and why its paused pairs are
  `task_not_in_catalog`), then the daemon-start warning, if any, then, pair by pair
  in (task, instance) order, `error_code_unparsed`, `task_not_in_catalog`,
  `lift_check_hash_unmeasured` and `comparison_disagrees_with_record`, and last
  `ledger_read_incomplete` when the read ran out of its deadline (RL-1c fix 1). No entry is an
  outcome envelope; a reader matches the strings.
- `clock_slots` (Workflow #372 K3): the clock-slot records (`scheduling/README.md`, "Clock-slot
  records") whose event time is within 48 h of the ledger's latest time, newest first, at most
  200; `clock_slots_total` is added with their number when the list is cut. A record is a
  `policy.planning_signal_observed` of kind `goal_missed` whose `signal_id` starts with
  `signal:clock_slot:`; it is selected by these two, never by its `outcome`.
  - Its `outcome` is read back into `code` and the outcome's values other than `task_id`
    and `alias`; `task_id` and `instance_id` are the record's own. Absent values are left
    out. `recorded_sequence` and `recorded_at_unix_ms` place the record in the ledger.
  - `next_dispatch` is the pair's first `policy.dispatch_admitted` after the record, as its
    `sequence` and `admitted_at_unix_ms` (D); it is left out when there is none. The reference
    time is the ledger's latest time: its maximum `timestamp_unix_ms`, not the last record's
    (RL-1c fix 2, #411 M2: over the whole ledger again, its row read through the time index,
    no longer over the record types the report reads by the type index),
    since a record's time is not monotonic in `sequence` (#411 A24,
    [ledger-observer.md §8.1](ledger-observer.md#81-order-and-sync)). Workflow #372 K5: a
    record of a task id that a task of the configured catalog continues (`continues_from`)
    settles against the first admission after it of that id or of that task, whichever comes
    first.
  - A refused or deferred record, with E its `window_end_unix_ms` and X its
    `next_occurrence_unix_ms`: `slot_fate` is `collapsed` when D ≥ X, `same_window` when D < E,
    and `next_window` otherwise, with `slot_fate_settled` true. Without D it is `collapsed` and
    settled once the reference time reaches X; before that it is unsettled and shows the fate
    its `next_eligible_unix_ms` predicts by the same rule, or `pending` without it.
  - An owed record: `served` when D < X; `absorbed` when D ≥ X, or without D once the reference
    time reaches X; both settled. Otherwise `pending`, unsettled.
  - Workflow #372 K4: an expired record (`clock_slot_expired`, with `occurrence_unix_ms`,
    `grace_ms` and `next_occurrence_unix_ms` when known) is `expired`, and a superseded record
    (`clock_slot_superseded`, with `occurrence_unix_ms` and `next_occurrence_unix_ms`) is
    `collapsed`, both settled. A refused or deferred record whose task, instance and
    occurrence an expired record among those read (before the list is cut) also names is
    `expired`, settled, whatever D.
  - A record whose outcome does not read back (a code that is not a clock-slot code, a value
    the entry does not hold, a value of the wrong type, a missing required value), or that
    names no task, gives
    `{"parse_state":"unparsed","signal_id":"...","code":"..."}`, `code` being the outcome's
    code, and no warning.

- `packs` (Workflow #386 SR-3): the task packages suspended since the latest daemon start (the
  `daemon_started_at_unix_ms` above; every record when it is unknown) and not lifted, by
  `package_id`, read from the evaluation's copies (`policy.planning_signal_observed` of kind
  `goal_missed` whose `signal_id` starts with `signal:pack_admission:`; "Task package
  suspension"). A suspension is memory of one daemon process, so copies from before its start
  are not read. `failure_code` and `runtime_commit` are the latest copy's (`null` when absent),
  `paused_hash_prefix` the first twelve hex digits of its `actual_digest` (`null` when the
  package could not be measured), `current_hash_prefix` the package measured now at the location
  the copied task's procedure binds (`null` when it cannot be measured or the task is not in the
  catalog), and `instances` the instances it was copied for and not lifted on. Each member is a
  code, an id or a hash, never a sentence.

- `remedies` (Workflow #411 B15, [remedies.md](remedies.md); written ahead of slice R7, an
  addition like `packs`): the stopped remedy rounds. One entry per
  `policy.planning_signal_observed` record of kind `goal_missed` whose `signal_id` starts with
  `signal:remedy:stopped:` and that is still in effect, or was lifted within 48 h of the ledger's
  latest event; newest first, at most 200, with `remedies_total` added with their number when the
  list is cut. A record is selected by its kind and prefix, never by its outcome.
  - `code` is the stop code (`policy_remedy_unchosen`, `policy_remedy_unavailable` or
    `policy_remedy_exhausted`), `remedy_problem`, `outcome_key` and `need` are its outcome's
    values, `task_id` and `instance_id` the record's own, and `recorded_sequence` and
    `recorded_at_unix_ms` place it in the ledger.
  - `lifted: {sequence, at_unix_ms}` is present when a later `signal:remedy:lifted:` record of the
    same round (the same `<hex>`) exists; without it the stop is still in effect.
  - A record whose outcome does not read back (a code that is not a stop code, a value the entry
    does not hold, a value of the wrong type, a missing required value), or that names no task,
    gives `{"parse_state":"unparsed","signal_id":"...","code":"..."}`, as for `clock_slots`, and
    no warning.
  - MCP `ac_diagnose` carries the diagnosed instance's entries as its `remedies` rows, with the
    same members.

- `held` (Workflow #438 HELD-1, since 0.12.56; [scheduling/README.md](scheduling/README.md),
  "Held records"): the held records. One entry per `policy.planning_signal_observed` record of
  kind `goal_missed` whose `signal_id` starts with `signal:task_held:held:` and that is still
  open, or ended within 48 h of the ledger's latest event; newest first, at most 200, with
  `held_total` added with their number when the list is cut. A record is selected by its kind and
  prefix, never by its outcome.
  - `code` is `task_held`; `gate_code`, `held_state`, `held_since_unix_ms`, `held_for_ms` and
    `held_after_ms` are its outcome's values, `task_id` and `instance_id` (the alias) the record's
    own, and `recorded_sequence` and `recorded_at_unix_ms` place it in the ledger.
  - `ended: {sequence, at_unix_ms, held_end, held_for_ms?}` is present when a later
    `signal:task_held:ended:` record of the same hold (the same `<hex>`) exists and reads back;
    without it the hold is still open (`held_for_ms` is absent for `held_end` `restarted`).
  - A record whose outcome does not read back (a code that is not `task_held`, a value the entry
    does not hold, a value of the wrong type, a missing required value, a task or alias that is not
    the record's own), or that names no task, gives
    `{"parse_state":"unparsed","signal_id":"...","code":"..."}`, as for `clock_slots`, and no
    warning.
  - MCP `ac_diagnose` carries the diagnosed instance's entries as its `held` rows, with the same
    members, and marks `held_reported: true` on each `not_run` row whose instance, task and
    `since_unix_ms` an open entry names (its `instance_id`, `task_id` and `held_since_unix_ms`).

- `break_ins` (Workflow #463 U1b, since 0.12.63; "A failure that pauses at its first
  occurrence"): one entry per pair whose latest value record (`policy.control_state_recorded`)
  carries a break-in, ordered by task, then instance: `task_id`, `instance_id`,
  `content_hash_prefix` (the content it counts), `runs` of `runs_needed`,
  `first_settled_at_unix_ms`, `since_first_ms` (the report's now less the first settlement,
  `null` without one) of `ms_needed`, and `in_break_in`, `false` once both are reached. The
  report's now (fix round 2) is the ledger's latest event, its head row, as `remedies` and `held`
  take it: the report reads no clock of its own, and not the ledger's latest time, the maximum
  over every row, which counts the rows the ledger stamps by the system clock (SR-2b-2 fix round 2),
  while the first settlement is a time of the Runtime's clock. Of a stopped Runtime it is the time
  of its last record, so a row can show a pair inside that the Runtime, judging on its own clock
  at its next run, finds past its break-in. `runs_needed` and `ms_needed` are the two keys of
  the configuration the report loads; a value set while the Runtime runs is not seen here. The
  report measures no package: a changed package shows its new break-in from its first settled
  run, when the Runtime starts it. A pair with no such record has no entry (it is inside).

Every exit, usage errors included, prints that one stdout line, and the process exits by its
outcome's category (Workflow #378 A6a): 0 for `suspended_reported`, whatever the report lists,
1 for an error, 2 for a fatal one. A failure is its outcome alone,
`{"schema_version":"actingcommand.actingd.suspended.v2","outcome":{…}}` (v1's `status`, `code`,
`operation` and `detail` are gone), and stderr carries its developer line
`<LEVEL> actingd: <code> <text>`. Codes: `suspended_usage_invalid`, `suspended_option_invalid`,
`suspended_config_missing`, the configuration load and assembly codes,
`suspended_policy_unconfigured` and `policy_catalog_compile_failed` (`operation`
`read_suspension_config`), `suspended_ledger_unavailable` (`operation`
`open_suspension_ledger`, `state_root`, and the ledger's code as its `caused_by` link when the
state root is a directory whose ledger does not open), `suspended_ledger_incomplete`, and
`runtime_state_poisoned` (`operation` `check_suspension_lift`).

## Known limitations

- A dialog that does not darken the screen and covers a medium part of it (about 90 to 170
  changed cells per thousand) is similar; if it appears at the same step twice, the task is
  paused, and a look at the frames resolves it.
- A failure that does not accumulate never pauses: an outdated prerequisite or return-home
  package shows only in `repeating`, and is rerun within its original window and budget cycle.
- The same cause can give a different `K` on days with and without an optional popup (the last
  `StepStarted` differs), which delays the pause by one more rerun.
- The startup reconciliation records no failure identity, so the next failure starts a new
  streak and is rerun once more; an interrupted settlement is recorded as a pause and, when it is
  the first of its pair in a row, lifted by the same restart (Workflow #361 M6).

## Task package suspension (Workflow #386 SR-3)

**Status.** Written ahead of its code (documentation slice SR-0, scheduling model v1.4.2 §2.4
with option 丙 of board #336) and implemented by slice SR-3 (register §1.69), with PKG-S3's
content hash. The not-running reason is the not-running row of `ac_overview` and `ac_schedule`
(MCP-19; Workflow #369 S7); the instance status keeps no such member. The LoadPack lift check
comes with Workflow #389 U-3.

**What is suspended.** When admission refuses a task package with a code listed in
`scheduling.pack_admission.suspend_codes` (default only `recognition_sample_budget_invalid`:
the package's declared sampling needs more than the smaller of its step and task timeouts, or a
scan's `max_ms`; since 0.12.57 this includes a select step's or a list reading's candidate
provider need, Workflow #448 SBUD-1, [v012-register.md](v012-register.md) §1.333), the
Runtime suspends that task package, identified by its `package_id`, under the Runtime build that
refused it. Admission depends only on the package's content and the kernel, so the object is
neither a (task, instance) pair nor a standard package: a standard package is always installed
and activated whole. Right after the refusal the Runtime measures the package's content hash
once (the `content-directory.v1` computation, or the SHA-256 of the file for a legacy assembled
ZIP). The suspension never takes it from admission: since Workflow #463 CH-1 (Runtime 0.12.71)
the admission of a run's package by its path computes the same hash, but only for the run's
admission record ("Content hash" above), and nothing here reads that record. The suspension is
memory of the daemon process: a
restart, and so another build, starts without it, and the background check after start judges
the packages again.

**Where it is found.** One rule, three places:

1. check-config (`scheduling.pack_admission.check_in_check_config`, default true) runs this
   Runtime's full admission on every package of `policy.procedure_manifest`. A refusal with a
   suspend code becomes a `pack_admission_suspended` note and the configuration stays valid;
   any other refusal is handled as before (at dispatch). check-config reads the ledger
   read-only and writes nothing; acsetup prints its warning unchanged, installs the standard
   package and lists that task package as suspended. ROUT-L RL-1c: in composer mode (no
   `policy.catalog`) it admits every package the composition's procedures locate
   ([actingd-check-config.md](actingd-check-config.md)).
2. After actingd has started (`scheduling.pack_admission.check_after_start`, default true), a
   background thread runs the same check outside the start window. It only suspends. ROUT-L
   RL-1c: in composer mode the procedure bindings are the composition's, which the scheduler
   workbench installs at its first check (the first policy cycle), so the thread waits for that
   check, or for a stop, and then admits the packages the composed procedures locate.
3. Dispatch preparation: the first refusal with a suspend code suspends the package.

**While suspended.** Every task bound to the package, on every instance, is deferred in the
evaluation with `pack_admission_suspended` (third in the order of deferrals,
`scheduling-blackout.md`): no intent and no further preparation. A task is bound to the package
when its procedure's binding locates a package whose `package_id` is the suspended one (the
`package_id` admission read at that location). The other task packages of the same standard
package are scheduled as usual. The refused dispatch keeps its execution record; no record shape
changes. For the clock-slot records of Workflow #372 K2 the code is a persistent gate
(`clock_slot_refused`).

**Lifting.** A suspension lifts when the package's content hash differs from the recorded one
(the package was fixed in place) or the Runtime build differs. The hash is measured again after
actingd starts (in the background), when the package is loaded (Workflow #389 LoadPack), and
when the evaluation finds a task of the package due while it is suspended, at most once per
package per minimum interval (a configuration key that Workflow #383 AO-0 registers, default
60 s). No restart is needed. A package that cannot be measured at a check (removed, being
replaced, unreadable) stays suspended, links the warning `lift_check_hash_unmeasured` (`path`,
`package_id`, `caused_by` the read's refusal; M-PKG D18) as a note of the
`pack_admission_suspended` reason, and is checked again. A package whose hash could not be
measured right after the refusal lifts at the first check that measures it. After a lift the next preparation tries the package once; the same refusal
under the same content hash and build suspends it again, so nothing loops. There is no manual
lift.

**Notices.** All are registered codes, never sentences:

- `actingd suspended` gains the top-level list `packs` (an addition that left v1's
  `schema_version` unchanged; the report is v2 since Workflow #378 A6a): one entry per suspended
  package with `package_id`, `failure_code`,
  `runtime_commit`, `paused_hash_prefix`, `current_hash_prefix` and `instances` (see
  "`actingd suspended`");
- MCP `ac_diagnose` (and `actingctl diagnose`) carries the `packs` rows whose `instances` name
  the diagnosed instance (every row without one); the not-running row of `ac_overview` and
  `ac_schedule` (MCP-19) shows `pack_admission_suspended` when it is the instance's reason; the
  instance status keeps no such member;
- the UI console shows it in the instance's status line (UI slice UI-1).

**Ledger.** The probes copy what happened; there is no direct write. A suspension found by
dispatch preparation is also visible as that dispatch's execution record. The evaluation, the
policy-decision choke point the G5 probe copies (`ledger-observer.md`), copies each suspension it
defers a pair of as a `pack_admission_suspended` outcome (`package_id`, `actual_digest` holding
the content hash, `failure_code`, `runtime_commit`), once per instance, and each lift as
`pack_admission_lifted` (`package_id`, `actual_digest` measured now, `runtime_commit`), once per
instance the suspension was copied for. The record type is the existing
`policy.planning_signal_observed` of kind `goal_missed` (coordinator ruling 4 of wave 14; the
same carrier as the clock-slot records of Workflow #372 K2): `instance_id` is the instance, a
suspension's `task_id` the task whose pair it deferred first there (a lift's is absent), its
outcome the copied code, and its `signal_id` `signal:pack_admission:<suspended|lifted>:<hex>`,
unique per copy. Like every planning signal it consumes no failure tax, advances no streak and
wakes no agent. `actual_digest` is absent when the hash could not be measured, `runtime_commit`
for a build without a known commit. The suspension is an `error`: a person has to change the
package, and the rest of the Runtime works on. It never triggers the recovery ladder.

**Unlike `paused_task`.** The per-pair pause above needs every pair to fail on its own; this
suspension holds every pair of the package at once.

## Condition termination and owed reruns (Workflow #386 SR-2)

**Status.** Written ahead of its code (documentation slice SR-0, scheduling model v1.4.2 §2.3,
§4). Slice SR-2a (v0.12.0) reads the `condition` member below and settles a replay from it,
so a later build's ledger stays readable within v0.12.x; it never writes the member. Slice
SR-2b-1 (v0.12.27) writes it: the live settlement and the reconciliation of a run from its
terminal decide it, the counts are refunded, the streak does not grow, `policy_rerun_exhausted`
is recorded, and the owed rerun is kept in `condition_reruns` (below). Slice SR-2b-2
(v0.12.27) adds the rest of "The rerun" and "Limits": the promotion with `policy_rerun_owed`,
the stage gate `policy_rerun_waiting`, the expiry `policy_rerun_expired`, hiding the terminal
from the snapshot, and `actingd suspended`'s `reruns`.

**Which runs.** A policy run ends by a condition, not by a failure of the task, when its failure
code is listed in `scheduling.rerun.condition_codes`. The default list is
`recognition_sample_budget_insufficient`, `candidate_sample_budget_insufficient`,
`recognition_sample_deadline_exceeded`, `candidate_sample_deadline_exceeded`,
`blackout_run_stopped` (`scheduling-blackout.md`), `perf_run_yielded` and `perf_hold_expired`
(Workflow #385), `cost_commit_time_insufficient` (`cost-commit.md`), `contained_task_paused` and
`contained_task_application_update_required`.
A sampling timeout ends a run only after its observe-only retries ran out: an observation that
times out is repeated without repeating any input, `scheduling.recognition.sample_timeout_retries`
times (default 2; slice SR-1) plus the pacing curve's extra retries of the step's stage (Workflow
#385 P-k). `contained_task_paused` is in the default list since 0.12.28, by Alice's decision 92
of 2026-10-29: a scheduled run that an operator's scheduling pause (`ac_pause`, `actingctl
pause`) or a deploy's drain cut off at its drain timeout ([scheduling-pause.md](scheduling-pause.md))
ends by a condition: it adds to no streak, its counts are given back (with the default
`refund_counts`) and its rerun is owed after the resume. A sensitive task's paused run stays an
ordinary failure, as every sensitive task's failure does (below).
`contained_task_application_update_required` is in the default list since 0.12.30 (UPD-1, design
`rd-1029/design-upd1.md` §2.2 item 4, option A of Alice's possible question 109; a changed
default, no new key and no `actingd-config` revision): a scheduled run that met the game's
update dialog ([package-reference.md](package-reference.md), `interrupts.json` `result`) ends by a
condition, adds to no streak, is not rerun at once (R22), has its counts given back and owes its
rerun, which the application update hold ([scheduling-blackout.md](scheduling-blackout.md),
position 4a) keeps until a store update package's run has cleared
`health.application.update_required`; the owed rerun then goes first. A sensitive task's run that
met the dialog stays an ordinary failure, as above.

**Decided once.** The live settlement decides it from the failure code, the configuration and
the number of consecutive condition terminations of the pair in this occurrence, and writes the
result into the failure part of the `policy.execution_recorded` record as the optional member
`condition: {refunded, count, expires_at_unix_ms}` (absent for an ordinary failure). `count` is
that consecutive number; `expires_at_unix_ms` is the end of the pair's bookkeeping window, the
activity window the run was admitted in. The occurrence is that window: `count` is one more
than the owed rerun's when that one expires at the same window end, otherwise 1; a success or an
ordinary failure of the pair ends the series and removes the owed rerun. Only a recoverable
failure of a task that is not sensitive is decided this way. A run that a restart, or the
reconciliation of a run its worker abandoned, settles from its terminal is settled for the
first time there, so the same rule decides it from the terminal's failure code (the start's
reconciliation reads the `scheduling.rerun` values of actingd's configuration file, as the
values set while the Runtime ran are restored after it); a run with no terminal
(`policy_settlement_interrupted`, Severe) never ends by a condition. A
replay of a run already recorded reads the member and never the current configuration, so
changing `condition_codes`, `max_per_occurrence` or `refund_counts` and restarting cannot make a
replay disagree with its record. The member is a persisted shape change of v0.12.0, registered by
Workflow #383 AO-0 with the other shapes of the new ledger; rolling a ledger that SR-2b wrote
back to a v0.11 build fails at its start.

**Settlement.** A run with `condition`:

- adds to no failure streak and no escalation: the record keeps the previous counters and
  `failure_streaks` is not updated, so the latest real failure identity (the one lifting a
  `paused_task` compares) stays;
- keeps the disposition `continue` (no new disposition value);
- with `refunded` true (`scheduling.rerun.refund_counts`, default true) gives back the task
  daily, task window, activity daily and activity window counts it took, so a task limited to
  one run a day can still be rerun; what the task spent in the game is not given back
  (`cost-commit.md`);
- does not add its duration to the task or activity runtime, and is never re-judged as
  `policy_runtime_budget_exceeded`;
- while its rerun is owed, hides its `terminal_state` and `completed_window` from the task
  runtime snapshot, so neither `dependency_completed(head, failed)` nor
  `dependency_completed(head, succeeded)` of a chain becomes true; `last_dispatched_unix_ms`
  still counts the occurrence as served.

**The rerun.** An owed pair (`condition_reruns`: (task, instance) → `{failure_code, count,
expires_at_unix_ms}`) is an input of the evaluation like the immediate rerun above: the pair is
treated as triggered and past its cooldown with the reason `policy_rerun_owed`, and with
`scheduling.rerun.promote` (default true) it ranks ahead of the instance's other candidates.
Feedback stop, placement, blackouts, activity windows and budgets still apply. It is an ordinary
policy dispatch in the instance's one queue (`policy_dispatch` claim); a held instance defers it
with `dispatch_instance_held`, a wait and not a failure. Pacing gates it: an instance paused by
pacing defers it with `perf_scheduling_paused` like every candidate, and a pacing stage above
`scheduling.rerun.resume_stage` (default S2), or back at or below it for less than
`resume_settle_ms` (default 60 s), defers it with `policy_rerun_waiting` (with `pacing_stage`).
A stage change wakes the evaluation (`ResourcesChanged`); a blackout's end wakes it through the
deferral's wake time. While performance pacing is observe-only the stage is always S0 and the
rerun is let through at once. SR-2b-2: the stage is the instance's effective pacing stage as the
evaluation starts (the curve's under `pace`, the learning view's under `full`); the settle counts
from the first evaluation that finds it back at or below the resume stage, and its end is the
deferral's wake (`next_eligible_unix_ms`); an instance the Runtime has not seen above the resume
stage since it started is let through at once. The first wait of each owed rerun is recorded
once as `policy_rerun_waiting` (info), with the carrier of `policy_rerun_exhausted`.

**Limits.** An owed rerun not served by `expires_at_unix_ms` expires with `policy_rerun_expired`
(warning); the run's `Failed` terminal then appears in the snapshot and counts as an ordinary
failure, and the missed round is not carried into the next day. A pair that ends by a condition
more than `scheduling.rerun.max_per_occurrence` times (default 2) in one occurrence settles that
run as an ordinary recoverable failure, without `condition`, and records
`policy_rerun_exhausted` (warning). That run is an ordinary failure, so it ends the series and
removes the owed rerun: the pair's next condition termination counts from 1 again, in the same
window too. Such a later owed rerun in the window its pair was exhausted in is not promoted
(coordinator decision, SR-2b-2): it does not enter the evaluation as owed and waits for the
pair's own trigger and cooldown, so sustained load cannot turn each trigger into
`max_per_occurrence` + 1 runs. The immediate rerun (R22) that the exhausted run's failure
identity armed does not rerun it at once either, and the stage gate still holds it; it still
hides the terminal and expires as any owed rerun. The
exhaustion is kept in memory only, like its warning before it is recorded: after a restart a
later owed rerun of that window is promoted again. The expiry is judged at each evaluation:
`policy_rerun_expired` is recorded once per owed rerun (`window_end_unix_ms` its
`expires_at_unix_ms`), the run's record is not settled again, and the expired entry stays in the
value record until the pair's next settlement replaces it.

**After a restart.** The owed reruns load from the pair's latest
`policy.control_state_recorded` value record (ledger slices LN-3 and LN-4, which satisfy what
this text first asked of AO-5 and AO-6), whose `condition_reruns` holds `{failure_code, count,
expires_at_unix_ms}` as the settlement wrote them; the `condition` member of execution records
is then read only by replay, which rebuilds the same entry from it. Value records are committed
as they arrive. After a start
that found a gap in the ledger, an owed rerun of an entry-cost task counts as not owed; any other
pair keeps its last committed value. `actingd suspended` gains the list `reruns` (an addition)
with the owed, waiting and expired reruns. It is read from the loaded rerun state, that is from
the pairs' value records (L7 of the [v0.12.0 register](v012-register.md)): SR-2b depends on LN-3
and LN-4, so no build with SR-2b derives the list from the execution records. Each row (SR-2b-2)
is one pair whose latest value record owes a rerun, ordered by task and instance: `task_id`,
`instance_id`, `failure_code`, `count`, `expires_at_unix_ms` and `state`: `expired` once the
rerun's `policy_rerun_expired` record was written, else `waiting` when the rerun's
`policy_rerun_waiting` record was written, else `owed`. The expiry is the Runtime's, judged at an
evaluation on its own clock; the report does not compare the window end with the ledger's latest
time, which also counts rows the ledger stamps by the system clock (a start's prior-epoch close
facts). MCP `ac_diagnose` carries the rows of the diagnosed instance.

None of these codes triggers the recovery ladder.

## Configuration of the scheduling rules

| Key | Default | Meaning |
|---|---|---|
| `scheduling.recognition.sample_timeout_retries` | 2 | Base number of observe-only retries of a timed-out observation |
| `scheduling.rerun.condition_codes` | the ten codes above (`contained_task_paused` since 0.12.28, decision 92; `contained_task_application_update_required` since 0.12.30, UPD-1) | Codes that end a run by a condition; read when a run is first settled (live settlement or reconciliation from its terminal). Every entry is a code the catalog lists and has not retired |
| `scheduling.rerun.max_per_occurrence` | 2 | Condition terminations per occurrence before an ordinary failure; read when a run is first settled (live settlement or reconciliation from its terminal) |
| `scheduling.rerun.resume_stage` | S2 | Highest pacing stage at which an owed rerun runs |
| `scheduling.rerun.resume_settle_ms` | 60,000 | How long the stage must stay at or below the resume stage |
| `scheduling.rerun.promote` | true | Rank owed reruns first |
| `scheduling.rerun.refund_counts` | true | Give back the counts of a condition termination; read when a run is first settled (live settlement or reconciliation from its terminal) |
| `scheduling.rerun.expires` | `window_end` | When an owed rerun expires (the only value) |
| `scheduling.pack_admission.check_in_check_config` | true | check-config runs the full admission of the packages |
| `scheduling.pack_admission.check_after_start` | true | The background check after start |
| `scheduling.pack_admission.suspend_codes` | `["recognition_sample_budget_invalid"]` | Admission codes that suspend a task package |

The defaults live in `distribution/windows/actingd.defaults.json` (Workflow #387) and arrive
with the code that parses the keys: slice SR-1 parses the `recognition` and `rerun` keys (and
every other `scheduling` key of v0.12.0, [runtime-config.md](runtime-config.md)) and slice
SR-3 the `pack_admission` keys. check-config lists each key with its effective value and
source in `config_manifest.parameters`. A key the key table marks settable can also be changed
while the Runtime runs (Workflow #387, `ConfigureSettings`); the others change through the
configuration file and a restart.

## A stopped run (Workflow #429 R-6b, since 0.12.55)

**Status.** Written ahead of its code ([v012-register.md](v012-register.md) §1.314) and built in
two slices of Runtime 0.12.55: R-6b1 (§1.315), the form, its check and every reader, and R-6b2
(§1.316), the stop of a policy run and the settlement that write it. It shares `ledger` 8 and
`runtime-client` 7 with the slices of the same release ([component-interfaces.md](component-interfaces.md),
"Workflow #429 R-6b"). `StopInstanceRun` stops a policy run as a direct run
([runtime-interface.md](runtime-interface.md), "Workbench request kinds"). Every rule of this
section is in force since 0.12.55.

**Which runs.** A policy run (a catalog task's dispatch, a fact refresh read included) that a
`StopInstanceRun` stopped. The stop asks the run's control `ClientRequested`, which on a scheduled
run only that stop asks (a client's `CancelContainedTask` refuses a scheduled run), so the run's
terminal is the `task.failed` every scheduled cancel writes, with `contained_task_cancelled`. A
policy run's control enters the run registry before its package's preparation, so a stop sent
while the run prepares reaches it, and the run ends at its first check. A run the stop found
already ending for its deadline or a pause's drain keeps that end (an ordinary failure,
`contained_task_deadline_exceeded`, or a condition, `contained_task_paused`), and a run that ended
on its own before its check saw the stop (a preparation's refusal included) keeps its own result.

**The record.** Its `policy_result` is a third kind (since 0.12.55), beside a success's
`{"kind": "succeeded", "runtime_ms"}` and a failure's `{"kind": "failed", "failure"}`:

```text
{"kind": "stopped", "outcome": <the terminal outcome's top node>, "runtime_ms": <the run's wall time>}
```

It carries no failure identity, class, streak, retry, disposition, sensitivity, performance context
or `condition`: none of them applies to a stop. Its `outcome` is codes only in the public
projection, as a failure's is; its stored severity is its outcome's category, as for every
result-bearing record; the payload's `wall_elapsed_ms` and `budget_elapsed_ms` (#385 P-k) are
written as for any run, and nothing compares them. The record's check asks only that `outcome` is a
code. SQLite's format, `actingcommand.event.v3`, the record types, the tables and the indexes do
not change.

**Settlement.** Neither a success nor a failure of the task:

- no failure streak and no escalation (the pair's failure state keeps its latest real failure, the
  identity a lift compares), no retry or backoff, no `paused_task` (a sensitive task's run
  included), and no recovery ladder of its own: the stopped run's terminal starts none, whatever
  `recovery_ladder.triggers` lists, also when the stop's reset failed and fenced the instance; the
  fence a failed reset leaves is handed on as every fence is (#403, #419), to a `resource_fence`
  ladder or to a person;
- never a condition, whatever `scheduling.rerun.condition_codes` lists: it owes no rerun and gives
  back no count;
- never rewritten as `policy_runtime_budget_exceeded`; the counts its admission took stay taken,
  and its runtime counts in the task's and the activity's windows as a success's does (since
  0.12.55, register §1.314 question 4);
- the Runtime's own reruns its dispatch served end with it (since 0.12.55, question 3): an immediate
  rerun's round (R22) ends as at `policy_retry_round_ended`, an owed rerun is removed, a remedy
  retry closes its round as a failed retry does but without a failure, and a provider's request is
  used;
- it projects no outcome record, as a failed run projects none, adds no run-duration sample, and
  adds nothing to the failure count of the pair's later runs, which counts on from the pair's
  previous success or failure, as for a late settlement (UNSET-2).

**Replay.** The settlement rebuilds the record from its own members and requires it to come out
again, as for the two other kinds (`policy_execution_record_mismatch` otherwise); replaying it ends
the same reruns, so a restarted Runtime holds the same decision memory.

**After a restart.**

- The run's terminal is in the ledger, its execution record is not: the start's reconciliation,
  and the reconciliation of a run its worker abandoned, settle it from its terminal as `stopped`,
  the terminal's code being the stop's mark. The ledger's own check of a recovered settlement
  accepts `stopped` from a `task.failed` whose outcome is `contained_task_cancelled`, with
  `runtime_ms` measured as a failure's, and from no other terminal.
- No terminal (the Runtime ended between the stop's request and the run's next checkpoint): the
  ledger cannot tell that a stop was asked. The stop's own records (`command.received`,
  `command.validated`) name the stop's request and the instance, not the run, and a stop answered
  from the instance's latest stopped run writes the same `command.validated`. The run settles as
  interrupted (`policy_settlement_interrupted`), as before, which the restart lifts when it is
  the pair's first in a row (since 0.12.55, question 2).

**Readers.** The decision memory (above); the task snapshot, which shows `terminal_state`
`cancelled` (the evaluator's existing word) with its `completed_window`, and keeps
`last_duration_ms` and `failure_streak` of the pair's latest success or failure
([scheduling/README.md](scheduling/README.md), "Settlement Feedback And Eligibility Age"); the
settlement facts ([runtime-fact-store.md](runtime-fact-store.md)); the frame retention, which
matches the record with its `task.failed` terminal; `actingd suspended` and `ac_diagnose`, which
list no stopped run as a failure or a pause; the routine summary, which counts it under
`interruptions` already (R-12, a scheduled failure terminal with `contained_task_cancelled`); the
event query pages, which carry it in `PublicPayload.policy_execution_outcome` (`runtime-client` 7);
and the client's run summary of a fixture run, which reads it beside its `task.failed` terminal. A
Runtime of `ledger` 7 and a client of `runtime-client` 6 cannot decode the kind (the result has no
catch-all), which is why both revisions move and `ledger` 8 is one way.
