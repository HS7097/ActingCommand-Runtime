# Policy suspension of linear tasks

A scheduled `linear_steps` task that fails mid-run is recorded, rerun at once, and paused when
the same problem happens again; the pause is lifted automatically once its package (or a
package its run went through) is updated (Workflow #336, R17-R19, R21, R22, R25). Everything
here uses the existing policy `on_failure`, the existing `policy.execution_recorded` and
`runtime.failed` records and the existing `paused_task` disposition: no ledger event type,
payload field or enum value is added, and a ledger written this way is read and replayed by
older builds.

## Catalog

The recommended `on_failure` of a linear task is

```json
{"action":"pause","retry_limit":1,"retry_backoff_ms":0,"escalation_threshold":2}
```

The first failure is recorded and scheduled for a retry; a second failure with the same
failure identity reaches the escalation threshold and pauses the task (`paused_task`, Error
severity). Under performance pressure a recoverable failure does not escalate, and the second
failure is paused by `retry_limit` with `action` `pause` instead (Warning severity). Nothing is
applied automatically: `on_failure` is part of the approved catalog, and a change of it is a
catalog change to be approved again.

`record stop` of a Lab recording prints this value as `catalog_on_failure_example`
(`lab-recording.md`, "Output").

## Immediate rerun (R22)

A pair (catalog task, instance) whose latest failure carries a failure identity, is
scheduled for a retry and has a live retry round is treated by the evaluator as triggered and past its task cooldown
(reason `failure_retry_immediate`): it is rerun as soon as its retry backoff has passed, at the
next evaluation, instead of at the next clock occurrence. `actingd` evaluates again right after
a cycle in which a scheduled run failed, so that evaluation, not the wake computed before the
run, sets the next wake (the end of the backoff). The feedback stop, placement, the
loop and activity budgets and the activity windows still apply, and every rerun consumes the
budgets like any dispatch. A page-graph task and a failure recorded with its original code
keep waiting for their trigger and cooldown.

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

The live settlement of a scheduled run whose main package was admitted as `linear_steps`
writes the failure identity as `failure.error_code` of its `policy.execution_recorded`:

```text
<base>~v1~k<K>~m<M>[~p<I><D>]{0,3}[~r<G><I><D>]{0,1}~f<F>
```

| Segment | Content |
|---|---|
| `base` | The terminal `failure_code`, or the request error code when the run has no terminal. A code longer than 64 bytes or containing `~` is `h` and the first 16 hex digits of its SHA-256. |
| `K` | The first 12 hex digits of SHA-256 over `actingcommand.failure-key.v1`, the original code, the scope, the step and detail′, joined by newlines. |
| `M` | The first 12 hex digits of the main package's digest (a ZIP or content-directory SHA-256; for a Git source tree reference, the SHA-256 of its wire value). |
| `p<I><D>` | One per declared prerequisite layer, outermost first: `I` the first 8 hex digits of SHA-256 over the package id, `D` the first 12 hex digits of the digest `prerequisite_packages` maps it to, or `x` when it maps none. |
| `r<G><I><D>` | The return-home layer: `G` the first 8 hex digits of SHA-256 over game, NUL, server; `I` and `D` as above. |
| `F` | The error frame's artifact SHA-256 (first 12 hex digits) of the run where this identity first occurred; `na` without a frame; `u` and the first 12 hex digits of SHA-256 over the decision id for a failure that does not accumulate. |

The identity prefix is everything before `~f`. At most 206 bytes, without whitespace or
control characters.

Every other run writes its original code: a main package that was not admitted (its mode is
unknown), a page-graph package, and the startup reconciliation of an interrupted settlement.

### Scope and step

- `prepare`: the prerequisite chain was not resolved and admitted; the run has no task
  records. The step is `prepare`.
- `pre:<digest>`: the run has a prerequisite chain, the gate did not start the package
  (`EntryTargetDisposition` `Started`), and a prerequisite package was opened
  (`EntryRecoveryPackageAdmitted`) and not completed: the first 12 hex digits of the innermost
  one.
- `gate`: the same, with no prerequisite package open (the gate's own first check, its first
  capture, a recheck after a completed layer).
- `main`: every other failure.

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
  `contained_task_linear_intermediate_unobserved` or `contained_task_guard_refused`: the main
  package's own steps no longer match the screen;
- the `prepare` scope with a `contained_task_prerequisite_*` refusal: a reproducible
  configuration problem (a missing mapping, a digest that does not match, an incompatible
  package).

Every other failure is rerun only: its `F` is unique, so it never repeats the previous identity
and its count stays 1. This covers the gate and every prerequisite and return-home package,
the start not on the first step, `contained_task_linear_application_unconfirmed`, device and
backend errors, operator pause, cancellation and deadline, the task timeout and recognition
errors.

The restart segment (R25-1): the run had a `launch` or `restart` application effect
(`application.intent`) and no `StepFinished` after the last one names a main-interface page
(canonical anchor `home` or `step_<digits>_home`). A failure there is rerun only.

R25-2: a linear task's terminal `application_backend_operation_failed` or
`input_backend_operation_failed` that did not poison the run is settled with the original
class `recoverable` and rerun only. A poisoning failure (an unconfirmed resource close) stays
severe. The task terminal, its severity and the `application.*` records are unchanged; the
startup reconciliation still settles from the terminal's severity.

A severe failure, a sensitive task, an exceeded runtime budget and an interrupted settlement
pause at the first failure, as before. Since Workflow #361 M6 an interrupted settlement (a
scheduled run the daemon's own end cut short, settled at the next start as
`policy_settlement_interrupted`) is recorded the same way but does not hold the pair: it is
no failure of the task, so the restart that recorded it lifts it (see "Lifting").

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

When the scheduled path's record of the run is missing (an internal error), the original code
is written together with a Warning `runtime.failed` `policy_failure_identity_unavailable`.

### Error frames

A run's error frame is the capture frame artifact (`ArtifactVerified`, kind `capture_frame`)
of the frame of its latest `CaptureCompleted`. The previous run is the run of the pair's latest
failed dispatch. Frames are read and verified read-only; a failed run's last frames are
retained permanently.

Both frames are decoded as PNG and digested whole with `color_digest.v1` on a grid of
`min(32, width)` by `min(18, height)` cells. A cell has changed when `|dR| + |dG| + |dB|` of
its quantized channels exceeds `CELL_DELTA = 6`. The frames are different when more than
`MAX_CHANGED_MILLI = 250` changed cells per thousand, or when their sizes differ; otherwise
similar. The digest mean distance and the template correlation coefficient are only displayed.

## Replay

The count is unchanged: two failures share a streak only when their codes and classes are
equal. Replay hands the recorded code to that count and requires the recorded execution to come
out again; it never compares frames. A settlement that finds an execution already recorded for
its dispatch writes nothing new and keeps the recorded identity, after checking that its base
is the base of the terminal's code (`policy_execution_identity_conflict` otherwise).

## Lifting (R19)

A paused pair is admitted again, by the evaluation and by admission, once any of these differs
from the paused dispatch:

- the main package digest now bound to its procedure;
- for a `p` layer, the digest the prerequisite mapping now has for its id hash (`x` when none,
  changed when several ids share the hash);
- for the `r` layer, the return-home entry of its game and server (missing or another id is a
  change), then its digest as for a `p` layer.

A procedure that is no longer bound never lifts. The configuration is the one the daemon read
at startup, so an update takes effect after a restart. An interrupted settlement
(`policy_settlement_interrupted`) is lifted without a package update (Workflow #361 M6): the
next dispatch of the pair is admitted as usual, within its loop and activity budgets (the
interrupted admission already counted against them). The next dispatch is admitted as usual;
a linear task that fails again starts a new streak, since `M` or a layer changed. A page-graph
task keeps its code, so the same failure pauses it again at once. A suspension in an older
ledger, including an exceeded budget, is lifted by a changed main package digest too; one of an
interrupted settlement in an older ledger is lifted at the next start of this Runtime. There is
no manual lift; `actingctl task-run` is not gated by the policy and does not lift a
suspension.

## `actingd suspended`

```text
actingd suspended --config <path>
```

Loads and assembles the configuration exactly as startup does, then opens the state root's
ledger read-only, without referenced material and without the owner lock, so it runs beside a
running daemon. Nothing is written.

stdout is one JSON object:

```json
{"schema_version":"actingcommand.actingd.suspended.v1","status":"ok",
 "config_path":"...","state_root":"...","through_sequence":0,"daemon_started_at_unix_ms":0,
 "suspended":[{"task_id":"...","instance_id":"...","procedure_ref":"...",
   "error_code":"...","failure_code":"...","consecutive_same_error":2,
   "effective_class":"severe","severity":"error",
   "paused":{"execution_sequence":0,"observed_at_unix_ms":0,"decision_id":"...","run_id":"..."},
   "previous":{"execution_sequence":0,"disposition":"retry_scheduled","run_id":"..."},
   "step":{"scope":"main","operation_label":"..."},
   "detail":"...",
   "package":{"paused":{},"current":{},"layers":[{"kind":"return_home","package_id":"...",
     "paused_digest_prefix":"...","current_digest_prefix":"...","changed":false}]},
   "frames":{"previous":{"frame_id":"...","artifact_sha256":"..."},"current":{},
     "comparison":{"source":"recomputed","status":"similar","reason":null,
       "changed_cells_milli":0,"digest_mean_milli":0,"ccoeff":1.0,"ccoeff_error":null}},
   "lifts_when":"main_digest_changes_or_any_layer_changes",
   "takeover":"actingctl task-run --state-root <root> --instance <instance> --package <package> --package-ref <package-ref>"}],
 "lifted":[{"task_id":"...","instance_id":"...","lifted_by":"main_digest|layer:<n>|restart",
   "effective":"active|pending_restart","paused":{},"package":{}}],
 "repeating":[{"task_id":"...","instance_id":"...","error_code":"...","failure_code":"...",
   "step":{},"latest":{"execution_sequence":0,"run_id":"..."},
   "previous":{"execution_sequence":0,"run_id":"..."},"detail":null}],
 "warnings":[]}
```

- `suspended`: each pair whose latest execution paused it and is not lifted. `previous` is the
  failed execution it repeats, `null` when it paused at its first failure. `step` is re-derived
  from the run's rows for a failure identity (`null` for another code). `detail` is the native
  detail of the run's lifecycle failure record of the failure code, or `null`. The frames are
  compared again (`source` `recomputed`); a recomputed `different` for an accumulating identity
  adds the warning `comparison_disagrees_with_record:<task>/<instance>`. A missing frame or an
  unreadable artifact gives `status` `unavailable` with its reason.
- `lifted`: each pair paused by its latest execution whose suspension the configuration lifts
  and that was not dispatched again. `effective` is `pending_restart` when the configuration
  file changed after the daemon's start, else `active`. An interrupted settlement is listed
  here with `lifted_by` `restart` and `effective` `active` (Workflow #361 M6).
- `repeating`: each pair whose latest execution is a failure that does not accumulate and whose
  previous execution failed with the same identity prefix: a prerequisite or return-home
  package, the restart segment or the device failing again and again.
- `daemon_started_at_unix_ms`: the observation time of the ledger's latest `config.parameters`
  fact, recorded at every start. Warnings: `config_newer_than_daemon_start` when the
  configuration file changed after it, `daemon_start_unknown` without it,
  `error_code_unparsed:<task>/<instance>` for a code that looks like an identity and does not
  parse (judged on its main digest only), `task_not_in_catalog:<task>/<instance>`.

Exit 0 when the report is read, whatever it lists. Exit 1 on any error, with
`FATAL actingd: <code>` on stderr; once the configuration is loaded, stdout also carries
`{"schema_version":"actingcommand.actingd.suspended.v1","status":"failed","code":...,
"operation":...,"detail":...}`. Codes: `suspended_usage_invalid`, `suspended_option_invalid`,
`suspended_config_missing`, the configuration load and assembly codes,
`suspended_policy_unconfigured`, `policy_catalog_compile_failed`,
`suspended_ledger_unavailable` (the ledger's code or `state_root_missing` as detail),
`suspended_ledger_incomplete`.

## Known limitations

- A dialog that does not darken the screen and covers a medium part of it (about 90 to 170
  changed cells per thousand) is similar; if it appears at the same step twice, the task is
  paused, and a look at the frames resolves it.
- A failure that does not accumulate never pauses: an outdated prerequisite or return-home
  package shows only in `repeating`, and is rerun within its original window and budget cycle.
- The same cause can give a different `K` on days with and without an optional popup (the last
  `StepStarted` differs), which delays the pause by one more rerun.
- The startup reconciliation records the original code, so the next failure starts a new
  streak and is rerun once more; an interrupted settlement is recorded as a pause and lifted by
  the same restart (Workflow #361 M6).
