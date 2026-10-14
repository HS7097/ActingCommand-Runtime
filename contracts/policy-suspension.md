# Policy suspension of linear tasks

A scheduled `linear_steps` task that fails mid-run is recorded, rerun at once, and paused when
the same problem happens again; the pause is lifted automatically once its package (or a
package its run went through) is updated (Workflow #336, R17-R19, R21, R22, R25). Everything
here uses the existing policy `on_failure`, the existing `policy.execution_recorded` and
`runtime.failed` records and the existing `paused_task` disposition: no ledger event type or
enum value is added. Since Workflow #378 A2-P the failure part of `policy.execution_recorded`
carries the run's failure as `outcome` and the failure identity as `failure_identity` (one
`error_code` string before); a ledger is read and replayed by the build that wrote it.

The last three sections ("Task package suspension", "Condition termination and owed reruns" and
their configuration) are the scheduling rules of Workflow #386, specified ahead of their code;
unlike the rest of this document, the condition termination adds one persisted member.

## Catalog

The recommended `on_failure` of a linear task is

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
loop and activity budgets and the activity windows still apply, and every rerun consumes the
budgets like any dispatch. A page-graph task and a failure recorded without a failure
identity keep waiting for their trigger and cooldown.

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
writes the failure identity as `failure.failure_identity` of its `policy.execution_recorded`,
beside the run's failure as `failure.outcome` (Workflow #378 A2-P):

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

Every other run writes no failure identity: a main package that was not admitted (its mode is
unknown), a page-graph package, and the startup reconciliation of an interrupted settlement.

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
no failure of the task, so the restart that recorded it lifts it (see "Lifting"). Only the
first interrupted settlement of a pair in a row is lifted; a second one in a row (the pair's
previous execution was an interrupted settlement too, `consecutive_same_error` 2 or more)
holds the pair as before, until its package changes, so a task that itself ends the daemon is
not re-admitted after every restart.

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

A paused pair is admitted again, by the evaluation and by admission, once any of these differs
from the paused dispatch:

- the main package digest now bound to its procedure;
- for a `p` layer, the digest the prerequisite mapping now has for its id hash (`x` when none,
  changed when several ids share the hash);
- for the `r` layer, the return-home entry of its game and server (missing or another id is a
  change), then its digest as for a `p` layer.

A procedure that is no longer bound never lifts. The configuration is the one the daemon read
at startup, so an update takes effect after a restart. An interrupted settlement
(`policy_settlement_interrupted`) is lifted without a package update (Workflow #361 M6) when
it is the first of its pair in a row: the next dispatch of the pair is admitted as usual,
within its loop and activity budgets (the interrupted admission already counted against them).
A second one in a row is lifted by a package update only. The count is rebuilt from the
execution records, so nothing persisted changes. The next dispatch is admitted as usual;
a linear task that fails again starts a new streak, since `M` or a layer changed. A page-graph
task records no failure identity, so the same failure pauses it again at once. A suspension in
an older ledger, including an exceeded budget, is lifted by a changed main package digest too; the first
interrupted settlement in a row in an older ledger is lifted at the next start of this Runtime. There is
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
 "warnings":[],
 "clock_slots":[{"code":"clock_slot_refused","task_id":"...","instance_id":"...",
   "gate_code":"policy_budget_exhausted","budget_dimension":"task_daily",
   "occurrence_unix_ms":0,"window_end_unix_ms":0,"next_eligible_unix_ms":0,"next_occurrence_unix_ms":0,
   "slot_fate":"next_window","slot_fate_settled":true,
   "recorded_sequence":0,"recorded_at_unix_ms":0,"next_dispatch":{"sequence":0,"admitted_at_unix_ms":0}}]}
```

- `suspended`: each pair whose latest execution paused it and is not lifted. `previous` is the
  failed execution it repeats, `null` when it paused at its first failure. `step` is re-derived
  from the run's rows for a failure identity (`null` for another code). `error_code` is the
  execution's `failure_identity`, else its failure's code. `detail` is the native detail of the
  run's lifecycle failure record of the failure code, or `null`. The frames are
  compared again (`source` `recomputed`); a recomputed `different` for an accumulating identity
  adds the warning `comparison_disagrees_with_record:<task>/<instance>`. A missing frame or an
  unreadable artifact gives `status` `unavailable` with its reason.
- `lifted`: each pair paused by its latest execution whose suspension the configuration lifts
  and that was not dispatched again. `effective` is `pending_restart` when the configuration
  file changed after the daemon's start, else `active`. The first interrupted settlement of a
  pair in a row is listed here with `lifted_by` `restart` and `effective` `active` (Workflow #361
  M6); a second one in a row is listed under `suspended` like any other pause.
- `repeating`: each pair whose latest execution is a failure that does not accumulate and whose
  previous execution failed with the same identity prefix: a prerequisite or return-home
  package, the restart segment or the device failing again and again.
- `daemon_started_at_unix_ms`: the observation time of the ledger's latest `config.subsystems`
  fact, recorded at every start (and only then; a `config.parameters.<section>` is also recorded
  when an instruction changes a setting, Workflow #387 D1b). Warnings: `config_newer_than_daemon_start` when the
  configuration file changed after it, `daemon_start_unknown` without it,
  `error_code_unparsed:<task>/<instance>` for a failure identity that does not
  parse (judged on its main digest only), `task_not_in_catalog:<task>/<instance>`.
- `clock_slots` (Workflow #372 K3): the clock-slot records (`scheduling/README.md`, "Clock-slot
  records") whose event time is within 48 h of the ledger's latest event, newest first, at most
  200; `clock_slots_total` is added with their number when the list is cut. A record is a
  `policy.planning_signal_observed` of kind `goal_missed` whose `signal_id` starts with
  `signal:clock_slot:`; it is selected by these two, never by its `outcome`.
  - Its `outcome` is read back into `code` and the outcome's values other than `task_id`
    and `alias`; `task_id` and `instance_id` are the record's own. Absent values are left
    out. `recorded_sequence` and `recorded_at_unix_ms` place the record in the ledger.
  - `next_dispatch` is the pair's first `policy.dispatch_admitted` after the record, as its
    `sequence` and `admitted_at_unix_ms` (D); it is left out when there is none. The reference
    time is the ledger's latest event time.
  - A refused or deferred record, with E its `window_end_unix_ms` and X its
    `next_occurrence_unix_ms`: `slot_fate` is `collapsed` when D ≥ X, `same_window` when D < E,
    and `next_window` otherwise, with `slot_fate_settled` true. Without D it is `collapsed` and
    settled once the reference time reaches X; before that it is unsettled and shows the fate
    its `next_eligible_unix_ms` predicts by the same rule, or `pending` without it.
  - An owed record: `served` when D < X; `absorbed` when D ≥ X, or without D once the reference
    time reaches X; both settled. Otherwise `pending`, unsettled.
  - An outcome that does not read back (a code that is not a clock-slot code, a value the
    entry does not hold, a value of the wrong type, a missing required value) gives
    `{"parse_state":"unparsed","signal_id":"...","code":"..."}`, `code` being the outcome's
    code, and no warning.

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
- The startup reconciliation records no failure identity, so the next failure starts a new
  streak and is rerun once more; an interrupted settlement is recorded as a pause and, when it is
  the first of its pair in a row, lifted by the same restart (Workflow #361 M6).

## Task package suspension (Workflow #386 SR-3)

**Status.** Written ahead of its code (documentation slice SR-0, scheduling model v1.4.2 §2.4
with option 丙 of board #336); slice SR-3 implements it, after PKG-S3 provides the content
hash. Until then a package that admission refuses is prepared and refused again at every due
dispatch.

**What is suspended.** When admission refuses a task package with a code listed in
`scheduling.pack_admission.suspend_codes` (default only `recognition_sample_budget_invalid`:
the package's declared sampling needs more than the smaller of its step and task timeouts), the
Runtime suspends that task package, identified by its `package_id`, under the Runtime build that
refused it. Admission depends only on the package's content and the kernel, so the object is
neither a (task, instance) pair nor a standard package: a standard package is always installed
and activated whole. Right after the refusal the Runtime measures the package's content hash
once (the `content-directory.v1` computation, or the SHA-256 of the file for a legacy assembled
ZIP). Admission itself never computes it.

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
`scheduling-blackout.md`): no intent and no further preparation. The other task packages of the
same standard package are scheduled as usual. The refused dispatch keeps its execution record;
no record shape changes.

**Lifting.** A suspension lifts when the package's content hash differs from the recorded one
(the package was fixed in place) or the Runtime build differs. The hash is measured again after
actingd starts (in the background), when the package is loaded (Workflow #389 LoadPack), and
when the evaluation finds a task of the package due while it is suspended, at most once per
package per minimum interval (a configuration key that Workflow #383 AO-0 registers, default
60 s). No restart is needed. A package that cannot be measured at a check (removed, being
replaced, unreadable) stays suspended, reports the warning `lift_check_hash_unmeasured`
(`package_id`; M-PKG D18; spelling proposed here, registered by PKG-S3 with the lift check) and
is checked again. After a lift the next preparation tries the package once; the same refusal
under the same content hash and build suspends it again, so nothing loops. There is no manual
lift.

**Notices.** All are registered codes, never sentences:

- `actingd suspended` gains the top-level list `packs` (an addition; `schema_version` is
  unchanged): one entry per suspended package with `package_id`, `failure_code`,
  `runtime_commit`, `paused_hash_prefix` and `current_hash_prefix`;
- MCP `ac_diagnose` carries the `packs` that concern the instance, and the not-running reason of
  `ac_overview` and of the instance status shows `pack_admission_suspended`;
- the UI console shows it in the instance's status line (UI slice UI-1).

**Ledger.** The probes copy what happened; there is no direct write. A suspension found by
dispatch preparation is visible as that dispatch's execution record. One found by the
background check is copied by the evaluation probe as one `pack_admission_suspended` outcome
(`package_id`, `actual_digest` holding the content hash, `failure_code`, `runtime_commit`), and a
lift as `pack_admission_lifted` (`package_id`, `actual_digest`, `runtime_commit`); which existing
record type carries them is decided before SR-3 is dispatched. The suspension is an `error`: a
person has to change the package, and the rest of the Runtime works on. It never triggers the
recovery ladder.

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
without repeating any input, `scheduling.recognition.sample_timeout_retries` times (default 2)
plus the pacing curve's extra retries (slice SR-1). An operator pause (`contained_task_paused`)
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
