# Scheduling pause and resume

Workflow #191 slices ps1 and ps2 (launcher design v3, Workflow #297 §3.1, §3.2 stages
(a)(b)(c), the resume paragraph and §3.3). `RuntimeOperation::PauseScheduling` stops the
Runtime's own policy dispatch, globally or for one instance, and an instance pause hands the
instance's device back to the person; `RuntimeOperation::ResumeScheduling` lifts the pause again
and an instance resume reconnects the device at once, answering with its self-check.

## Operations and origin gate

```text
PauseScheduling  { scope, reason_code, drain_timeout_ms }
ResumeScheduling { scope }
scope = { "kind": "global" } | { "kind": "instance", "instance_alias": "<alias>" }
```

- `RuntimeRequest::validate` admits both only when `(actor, source)` is `(User, Ui)` or
  `(Cli, Cli)`; every other origin is `invalid_scheduling_pause_origin`. No scheduler,
  agent or Lab path can pause or resume dispatch. A `Ui` request needs no governance
  identity card.
- `reason_code` is a closed code: 1..=64 bytes of `[a-z0-9_.-]`
  (`invalid_scheduling_pause_reason`).
- `drain_timeout_ms` is bounded to `1_000..=600_000`
  (`invalid_scheduling_pause_drain_timeout`). It is validated for both scopes and used only
  by an instance pause.
- An instance scope names a registered physical instance (`require_physical_instance_alias`):
  a fixture instance is `fixture_execution_scope_forbidden`, an unknown alias
  `instance_unknown`; this holds for pause and resume alike.

## The dispatch gate

`admit_policy_dispatch` checks the pauses right after `replay_admission` (a replayed decision
is still suppressed as before) and before the performance and capacity gates: the global
pause first (`dispatch_paused_global`), then the pause of the intent's instance
(`dispatch_paused_instance`). A hit only sets the admission's gate error; the performance and
capacity gates are not consulted. The trace is the existing one of a refused admission:
`policy.dispatch_intent`, then `policy.dispatch_rejected` (`Denied`, `NotPerformed`) with the
code in its `rejection`.

Every policy evaluation reads the same pauses as it starts: a candidate on a paused instance
is `CandidateEligibility::Deferred` with reason code `dispatch_paused_global` /
`dispatch_paused_instance` and no next wake time, so a paused cycle emits no dispatch intent
for it.

The gate covers policy dispatch only. Client requests (`task-run`, leases, observation), the
startup package and the stuck-recovery ladder are not gated.

## Global pause

`PauseScheduling { scope: global }` closes the gate for every instance and answers at once:
`SchedulingPaused { scope, revision, drained: absent }`. It closes no device session and
touches no in-flight run.

## Instance pause: three stages

`Status` shows the stage of an instance pause: `draining` during (a) and (b), `paused` once (b)
is done while (c) runs, `released` once (c) is done. The request answers after (c).

1. **(a) Stop dispatch.** The instance's gate closes at once; `Status` shows its pause with
   `stage: draining`.
2. **(b) Drain.** The request waits for the instance's in-flight contained runs (those in
   flight at the pause and any that start while it drains) to end on their own. A run still
   in flight once `drain_timeout_ms` elapsed is asked to stop at its next checkpoint: it ends
   as a typed failure with the code `contained_task_paused`
   (`ContainedTaskCancellationReason::PausedByOperator`,
   `RuntimeErrorCode::ContainedTaskPaused`): a policy run commits `task.failed` with that
   failure code, a client run commits `task.cancelled` and its receipt names the reason. No
   lease is preempted or reclaimed and `cancel_contained_task` is not used; the scheduler's
   `TransferNotSafe` and `Deferred` guards are unchanged. Once no run of the instance is in
   flight the stage becomes `paused` and the request answers
   `SchedulingPaused { scope, revision, drained: { finished, cancelled } }` once (c) is
   done: `finished` runs ended on their own, `cancelled` runs were asked to stop by this pause
   (a run drained again in (c) is counted too).
3. **(c) Hand the device back** (ps2). The instance's device session is closed through the
   existing fenced resource-close path (`close_retained_instance_while_guarded`, no second
   close implementation), so the emulator is free for the person. Right before the close,
   under the instance admission guard, the pause re-checks that nothing still uses the device:
   - a non-empty lease queue fails the pause at once with the close path's own rule
     (`prepare_resource_close`: `TransferNotSafe`): a queued client waits for the device;
   - a contained run that started after (b) (a policy dispatch admitted just before the gate
     closed; the pause first waits for policy admissions that read the gate before it closed)
     is drained again under the same drain and grace deadlines; the device is never closed
     under a running task;
   - a client whose run ended cancelled owes the instance its usual `SafeReset`
     (`runtime-client` sends it right after a `ContainedTaskCancelled` receipt, on the same
     connection); the close waits until that reset has finished or that connection closed;
   - an active lease is waited for until it is released.

   These waits end at the grace deadline (the drain timeout plus `30_000` ms); past it the
   pause fails with `scheduling_pause_release_busy` (`LeaseBusy`). With nothing left, the
   close takes no active lease: it grants the dedicated resource-close lease
   (`prepare_resource_close`, the fixed resource-close connection, `CapacityUse::Drain`),
   closes capture first and then input under `DeviceCloseAuthority::FencedDeviceWrite`, records
   the usual `runtime.lifecycle_observed` `ResourceQuiescence` observation and releases the
   lease with `LeaseReleaseReason::InstancePaused` (a `lease.released` event; the reason is the
   scheduler's release reason, the event carries no reason field). An instance whose device
   session is not open (never opened, or already closed by a lease release or a stopped policy
   run's failure cleanup) closes nothing and records nothing. The stage then becomes
   `released`.

If a stage fails, the request fails as a whole (`Failed` receipt) and the gate it closed is
lifted again: no half-open pause is left behind. A drain whose stopped runs have not reached
their next checkpoint `30_000` ms after the drain timeout
(`SCHEDULING_PAUSE_CHECKPOINT_GRACE_MS`) fails with `scheduling_pause_drain_incomplete`
(`ContainedTaskBusy`); a Runtime shutdown during the drain fails it with
`scheduling_pause_drain_interrupted`, during the hand-back with
`scheduling_pause_release_interrupted` (`RuntimeUnavailable`). A failed device close fails the
request with the close's own error; an unconfirmed close stays fatal as on every close path. The
client's receipt wait is the drain timeout plus that grace plus its IO timeout.

## Resume

`ResumeScheduling { scope }` lifts the matching gate and bumps the revision of that scope. A
global resume answers `SchedulingResumed { scope, revision, selfcheck: absent }` and
reconnects nothing.

An instance resume (ps2) accepts a pause in stage `released` only. Under the instance
admission guard it lifts the gate and then reconnects the device at once instead of at the next
lazy open: `ExecutionKernel::open_instance_backends` opens the instance's input and capture
backends through the same provider opens as the lazy paths (a backend the session still holds
is reused; a Nemu pair opens once) and takes the first frame of a capture it opened, as the
first lazy capture does, then drops it (no frame artifact is written). The opens are recorded
like every open (`backend-open-observation.md`): `backend.open_observed`, the
`backend.selfcheck.*` facts, and a `failed` self-check withdrawing the instance's policy
availability. The receipt carries the self-check projected from those reports:

```text
selfcheck = {
  capture: { ok, width, height, capture_backend },
  touch:   { ok, backend, max_x, max_y, invasive: false },
  failure_code: <device code> | null
}
```

- `capture.ok`: the Capture (or Nemu pair) report's open status and `capture_check` passed;
  `width` / `height` are its frame size, `capture_backend` its selected backend.
- `touch.ok`: the Input (or Nemu pair) report's open status and `input_check` passed;
  `backend` is its selected backend, `max_x` / `max_y` its connection's input geometry, else
  its handshake limits (absent for a Nemu pair). The check reads the connection back and sends
  no touch.
- A side the session still held has no report of this resume: `ok: false`, no values.
- `failure_code` is the failed open's device code verbatim, for example
  `input_backend_open_failed`, `capture_backend_open_failed`,
  `capture_backend_operation_failed` or `paired_backend_open_failed`.

A failed reconnect does not roll the resume back and is not retried: the resume is performed,
the failed session is closed through the existing capture-failure close path, and the receipt
reports the failure. A successful reconnect keeps the session open for the next task. Only an
unconfirmed close or a failed record fails the request. A resume triggers no extra policy
evaluation: the next cycle runs on its ordinary trigger.

## Revisions and refusals

The global pause and every instance pause hold their own revision, and neither implies the
other. Setting or lifting a gate (a failed stage included) bumps the revision of its scope; the
stage changes `draining` to `paused` to `released` do not.

| Situation | Receipt | Host code |
|---|---|---|
| Pause while the same scope is already paused (either stage) | `Denied`, `InvalidRequest` | `scheduling_already_paused` |
| Resume of a scope that is not paused | `Denied`, `InvalidRequest` | `scheduling_not_paused` |
| Resume of an instance whose pause is still draining | `Denied`, `InvalidRequest` | `scheduling_pause_draining` |
| Resume of an instance whose pause is still handing its device back (`paused`) | `Denied`, `InvalidRequest` | `scheduling_pause_releasing` |

## Status

`Status` (`RuntimeControlPlaneStatus`) carries `scheduling_pause: { revision, reason_code,
since_unix_ms }` while the global pause holds, and each `RuntimeInstanceStatus` carries
`pause: { revision, reason_code, since_unix_ms, stage: draining | paused | released }` while
its instance is paused. Both fields are absent otherwise (additive wire: a
`deny_unknown_fields` reader must move to this contract before it reads a paused status).

## No persistence, no expiry

Pauses live in the host's memory only: a restart starts unpaused. A pause has no TTL; only
`ResumeScheduling` lifts it. The ledger event set is unchanged: a pause or resume leaves its
trace in the request receipt, the refused dispatch admissions, the terminals of the runs it
stopped, the device close's existing records and the reconnect's existing open records.

## CLI

```bash
actingctl pause  --state-root <state-root> [--instance <alias>] [--reason <code>] [--drain-timeout-ms <n>]
actingctl resume --state-root <state-root> [--instance <alias>]
actingctl status --state-root <state-root>
```

Origin `(Cli, Cli)`. Without `--instance` the scope is global. `--reason` defaults to
`operator` and `--drain-timeout-ms` to `60000`; both are checked against the bounds above
before any request is sent. `pause` / `resume` print the result; `status` prints the global
and per-instance pause states within the status.
