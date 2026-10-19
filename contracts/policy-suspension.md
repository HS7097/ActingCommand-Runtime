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
unlike the rest of this document, the condition termination adds one persisted member.

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
page-graph task is rerun at once as a linear one is.

`PolicyControlState` owns the round. An admitted normal trigger opens it using the original
`PolicyActivitySample` and budget receipt. An admission whose reason is
`failure_retry_immediate` retains that origin; consecutive failures and new failure identities
do not renew it. The round uses that profile's original half-open activity occurrence and
budget limits. The shared `activity_window_at` calculation assigns an overnight occurrence to
its opening day; midnight inside it does not reset its budgets. A full-day occurrence ends at
the next local midnight, and an ordinary occurrence ends at its declared end. A different
profile/window or expected duration cannot use the original round's immediate permission.

Admission spends task/activity daily and window counts. Once any original count limit is
reached the round ends. Runtime usage settles the reservation to actual duration at execution
completion; if the remaining original task/activity runtime budget cannot reserve the next
run's expected duration, the round ends then. Pending reservations still constrain admission.
Shared activity consumption by another task of that instance uses the same counters and can
end the round. A closed round stays closed when budgets become available again. Window expiry
also ends immediate permission, including for an intent evaluated before the boundary and
admitted after it (`policy_retry_round_ended`). The next legal normal trigger opens a new round;
budget availability alone is not a trigger. A run already admitted finishes under its existing
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
| `F` | The error frame's artifact SHA-256 (first 12 hex digits) of the run where this identity first occurred; `na` without a frame; `u` and the first 12 hex digits of SHA-256 over the decision id for a failure that does not accumulate. |

The identity prefix is everything before `~f`. At most 250 bytes (Workflow #373 NAV-5: was
214 before `n` and `g`), without whitespace or control characters. A segment out of the order
`p`, `r`, `n`, `g` is no identity. A format v1 string (Workflow #336 L6, before PKG-S3) is not read.

**Content hash** (Workflow #336 PKG-S3, option 丙; M-PKG §3.3). A package's content hash is the
`content-directory.v1` hash of the content table its path holds (a directory, a `.json` or a
`.zip` content container), or the SHA-256 of the whole file for an assembled package ZIP (one
holding `resources/manifest.json`). It has one use: a pause. It is measured in two places only,
read-only: when a scheduled failure is settled (the main package at the path the run used and
each layer at the path the run's standard-pack registry gave it, Workflow #373 NAV-3a; SR-3
adds its suspension point) and at a lift check
("Lifting"). Admission never computes or compares it, and it is stored only inside the failure
identity.

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
  ruling 4);
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
  there are expected.

Every other failure is rerun only: its `F` is unique, so it never repeats the previous identity
and its count stays 1. This covers the gate and every prerequisite and return-home package,
the start not on the first step, `contained_task_linear_application_unconfirmed`, device and
backend errors, operator pause, cancellation and deadline, the task timeout and recognition
errors.

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
pause at the first failure, as before. Since Workflow #361 M6 an interrupted settlement (a
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
round instead.

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

## `actingd suspended`

```text
actingd suspended --config <path>
```

Loads and assembles the configuration exactly as startup does, then opens the state root's
ledger read-only and without the owner lock, so it runs beside a running daemon. Since
Workflow #336 PKG-S3 it reads the packages a paused pair names, read-only, to measure their
content hashes now, each package once per report. Nothing is written.

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
   "effective_class":"severe","severity":"error",
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
   "recorded_at_unix_ms":0,"lifted":{"sequence":0,"at_unix_ms":0}}]}
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
  coefficient that could not be computed, else `null`.
- `package`: the content hash prefixes the pause recorded (`paused_hash_prefix`, `x` when the
  settlement could not measure the package, `null` for a pause without a failure identity) and
  the ones measured now (`current_hash_prefix`, `null` when the package cannot be measured or
  the procedure is not bound); a layer is `changed` only when it was measured now and differs.
- `lifted`: each pair paused by its latest execution whose suspension a content hash change
  lifts and that was not dispatched again. `effective` is `next_evaluation`: the running daemon
  lifts it at its next check of the pair, without a restart. The first interrupted settlement of a
  pair in a row is listed here with `lifted_by` `restart` and `effective` `active` (Workflow #361
  M6); a second one in a row is listed under `suspended` like any other pause.
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
  alone or followed by `:<task>/<instance>`: first the daemon-start warning, if any, then, pair by pair
  in (task, instance) order, `error_code_unparsed`, `task_not_in_catalog`,
  `lift_check_hash_unmeasured` and `comparison_disagrees_with_record`. No entry is an outcome
  envelope; a reader matches the strings.
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
    time is the ledger's latest time: its maximum `timestamp_unix_ms`, not the last record's,
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
content hash. The not-running reason of `ac_overview` and of the instance status comes with
slice SR-6b (Workflow #369 S7); the LoadPack lift check with Workflow #389 U-3.

**What is suspended.** When admission refuses a task package with a code listed in
`scheduling.pack_admission.suspend_codes` (default only `recognition_sample_budget_invalid`:
the package's declared sampling needs more than the smaller of its step and task timeouts), the
Runtime suspends that task package, identified by its `package_id`, under the Runtime build that
refused it. Admission depends only on the package's content and the kernel, so the object is
neither a (task, instance) pair nor a standard package: a standard package is always installed
and activated whole. Right after the refusal the Runtime measures the package's content hash
once (the `content-directory.v1` computation, or the SHA-256 of the file for a legacy assembled
ZIP). Admission itself never computes it. The suspension is memory of the daemon process: a
restart, and so another build, starts without it, and the background check after start judges
the packages again.

**Where it is found.** One rule, three places:

1. check-config (`scheduling.pack_admission.check_in_check_config`, default true) runs this
   Runtime's full admission on every package of `policy.procedure_manifest`. A refusal with a
   suspend code becomes a `pack_admission_suspended` note and the configuration stays valid;
   any other refusal is handled as before (at dispatch). check-config reads the ledger
   read-only and writes nothing; acsetup prints its warning unchanged, installs the standard
   package and lists that task package as suspended.
2. After actingd has started (`scheduling.pack_admission.check_after_start`, default true), a
   background thread runs the same check outside the start window. It only suspends.
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
  the diagnosed instance (every row without one); the not-running reason of `ac_overview` and of
  the instance status shows `pack_admission_suspended` once SR-6b adds that reason;
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
SR-2b (v0.12.x, together with performance pacing slice P-s and after ledger slices AO-5 and
AO-6) writes it and owes the reruns. Until SR-2b, these failures settle as before.

**Which runs.** A policy run ends by a condition, not by a failure of the task, when its failure
code is listed in `scheduling.rerun.condition_codes`. The default list is
`recognition_sample_budget_insufficient`, `candidate_sample_budget_insufficient`,
`recognition_sample_deadline_exceeded`, `candidate_sample_deadline_exceeded`,
`blackout_run_stopped` (`scheduling-blackout.md`), `perf_run_yielded` and `perf_hold_expired`
(Workflow #385) and `cost_commit_time_insufficient` (`cost-commit.md`). A sampling timeout ends a
run only after its observe-only retries ran out: an observation that times out is repeated
without repeating any input, `scheduling.recognition.sample_timeout_retries` times (default 2;
slice SR-1) plus the pacing curve's extra retries of the step's stage (Workflow #385 P-k). An
operator pause (`contained_task_paused`)
is not in the default list.

**Decided once.** The live settlement decides it from the failure code, the configuration and
the number of consecutive condition terminations of the pair in this occurrence, and writes the
result into the failure part of the `policy.execution_recorded` record as the optional member
`condition: {refunded, count, expires_at_unix_ms}` (absent for an ordinary failure). `count` is
that consecutive number; `expires_at_unix_ms` is the end of the pair's bookkeeping window. A
replay or a start reads the member and never the current configuration, so changing
`condition_codes`, `max_per_occurrence` or `refund_counts` and restarting cannot make a replay
disagree with its record. The member is a persisted shape change of v0.12.0, registered by
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
rerun is let through at once.

**Limits.** An owed rerun not served by `expires_at_unix_ms` expires with `policy_rerun_expired`
(warning); the run's `Failed` terminal then appears in the snapshot and counts as an ordinary
failure, and the missed round is not carried into the next day. A pair that ends by a condition
more than `scheduling.rerun.max_per_occurrence` times (default 2) in one occurrence settles that
run as an ordinary recoverable failure, without `condition`, and records
`policy_rerun_exhausted` (warning).

**After a restart.** Before ledger slices AO-5 and AO-6 the owed reruns are rebuilt from the
execution records, as the failure streaks are. From AO-6 on they load from the pair's latest
`policy.control_state_recorded` value record, whose `condition_reruns` holds `{failure_code,
count, expires_at_unix_ms}` as the settlement wrote them; the `condition` member of execution
records is then read only by replay. Value records are committed as they arrive. After a start
that found a gap in the ledger, an owed rerun of an entry-cost task counts as not owed; any other
pair keeps its last committed value. `actingd suspended` gains the list `reruns` (an addition)
with the owed, waiting and expired reruns. It is read from the loaded rerun state, that is from
the pairs' value records (L7 of the [v0.12.0 register](v012-register.md)): SR-2b depends on AO-5
and AO-6, so no build with SR-2b derives the list from the execution records.

None of these codes triggers the recovery ladder.

## Configuration of the scheduling rules

| Key | Default | Meaning |
|---|---|---|
| `scheduling.recognition.sample_timeout_retries` | 2 | Base number of observe-only retries of a timed-out observation |
| `scheduling.rerun.condition_codes` | the eight codes above | Codes that end a run by a condition; read only at live settlement. Every entry is a code the catalog lists and has not retired |
| `scheduling.rerun.max_per_occurrence` | 2 | Condition terminations per occurrence before an ordinary failure; read only at live settlement |
| `scheduling.rerun.resume_stage` | S2 | Highest pacing stage at which an owed rerun runs |
| `scheduling.rerun.resume_settle_ms` | 60,000 | How long the stage must stay at or below the resume stage |
| `scheduling.rerun.promote` | true | Rank owed reruns first |
| `scheduling.rerun.refund_counts` | true | Give back the counts of a condition termination; read only at live settlement |
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
