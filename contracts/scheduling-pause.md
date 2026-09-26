# Scheduling pause and resume

Workflow #191 slice ps1 (launcher design v3, Workflow #297 §3.1 and §3.2 stages (a)(b)).
`RuntimeOperation::PauseScheduling` stops the Runtime's own policy dispatch, globally or for
one instance; `RuntimeOperation::ResumeScheduling` lifts it again. Stage (c) of an instance
pause (handing the device back: closing the instance's device session) and the reconnect with
its self-check on an instance resume are slice ps2; this slice answers an instance pause once
its stage is `paused`.

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
   `SchedulingPaused { scope, revision, drained: { finished, cancelled } }`: `finished` runs
   ended on their own, `cancelled` runs were asked to stop by this pause.
3. **(c) Hand the device back.** Slice ps2.

If a stage fails, the request fails as a whole (`Failed` receipt) and the gate it closed is
lifted again: no half-open pause is left behind. A drain whose stopped runs have not reached
their next checkpoint `30_000` ms after the drain timeout
(`SCHEDULING_PAUSE_CHECKPOINT_GRACE_MS`) fails with `scheduling_pause_drain_incomplete`
(`ContainedTaskBusy`); a Runtime shutdown during the drain fails it with
`scheduling_pause_drain_interrupted`. The client's receipt wait is the drain timeout plus that
grace plus its IO timeout.

## Resume

`ResumeScheduling { scope }` lifts the matching gate, bumps the revision of that scope and
answers `SchedulingResumed { scope, revision, selfcheck: absent }`; `selfcheck` is filled by
slice ps2. A resume triggers no extra policy evaluation: the next cycle runs on its ordinary
trigger.

## Revisions and refusals

The global pause and every instance pause hold their own revision, and neither implies the
other. Setting or lifting a gate (a failed drain included) bumps the revision of its scope; the
stage change `draining` to `paused` does not.

| Situation | Receipt | Host code |
|---|---|---|
| Pause while the same scope is already paused (either stage) | `Denied`, `InvalidRequest` | `scheduling_already_paused` |
| Resume of a scope that is not paused | `Denied`, `InvalidRequest` | `scheduling_not_paused` |
| Resume of an instance whose pause is still draining | `Denied`, `InvalidRequest` | `scheduling_pause_draining` |

## Status

`Status` (`RuntimeControlPlaneStatus`) carries `scheduling_pause: { revision, reason_code,
since_unix_ms }` while the global pause holds, and each `RuntimeInstanceStatus` carries
`pause: { revision, reason_code, since_unix_ms, stage: draining | paused }` while its
instance is paused. Both fields are absent otherwise (additive wire: a
`deny_unknown_fields` reader must move to this contract before it reads a paused status).

## No persistence, no expiry

Pauses live in the host's memory only: a restart starts unpaused. A pause has no TTL; only
`ResumeScheduling` lifts it. The ledger event set is unchanged: a pause or resume leaves its
trace in the request receipt, the refused dispatch admissions and the terminals of the runs it
stopped.

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
