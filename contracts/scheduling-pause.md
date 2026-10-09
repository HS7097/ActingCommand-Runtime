# Scheduling pause and resume

Workflow #191 slices ps1 and ps2 (launcher design v3, Workflow #297 §3.1, §3.2 stages
(a)(b)(c), the resume paragraph and §3.3). `RuntimeOperation::PauseScheduling` stops the
Runtime's own policy dispatch, globally or for one instance, and an instance pause waits until
the instance's device is idle (its session stays open, Workflow #191 H);
`RuntimeOperation::ResumeScheduling` lifts the pause again
and an instance resume reconnects the device at once through the connection preparation phase
(Workflow #317 sc3), answering with its self-check.

## Operations and origin gate

```text
PauseScheduling  { scope, reason_code, drain_timeout_ms }
ResumeScheduling { scope, expected? }
scope = { "kind": "global" } | { "kind": "instance", "instance_alias": "<alias>" }
expected = { "owner_epoch": "<epoch>", "revision": <u64 >= 1> }
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

The gate covers policy dispatch only. Client requests (`task-run`, leases, observation) and the
startup package are not gated (the startup package until Workflow #369 S6b). A stuck-recovery
ladder is admitted only while its instance is not paused; a pause that arrives while a ladder
holds the instance ends the ladder at its next step (`recovery_admission_denied`), and its one
release lets the pause complete (`contracts/emulator-control.md`, "Stuck-recovery ladder").

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
3. **(c) Wait until the device is idle** (ps2; Workflow #191 H). A pause is not a disconnect
   (owner ruling 2026-09-29): the instance's device session stays open and the Runtime keeps
   using nothing of it while the gate is closed. Under the instance admission guard the pause
   re-checks that nothing still uses the device:
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
   pause fails with `scheduling_pause_release_busy` (`LeaseBusy`). With nothing left the stage
   becomes `released`; the device session stays as it is and nothing is recorded. Only an
   instance outside the multi-Nemu gate (`read-session-resource-close.md`, "Device session
   lifetime") closes its session here, through the existing fenced resource-close path
   (`close_retained_instance_while_guarded`, no second close implementation) and without an
   active lease: it grants the dedicated resource-close lease (`prepare_resource_close`, the
   fixed resource-close connection, `CapacityUse::Drain`), closes capture first and then input
   under `DeviceCloseAuthority::FencedDeviceWrite`, records the usual
   `runtime.lifecycle_observed` `ResourceQuiescence` observation and releases the lease with
   `LeaseReleaseReason::InstancePaused` (a `lease.released` event; the reason is the
   scheduler's release reason, the event carries no reason field); such an instance whose
   device session is not open closes nothing and records nothing.

If a stage fails, the request fails as a whole (`Failed` receipt) and the gate it closed is
lifted again: no half-open pause is left behind. A drain whose stopped runs have not reached
their next checkpoint `30_000` ms after the drain timeout
(`SCHEDULING_PAUSE_CHECKPOINT_GRACE_MS`) fails with `scheduling_pause_drain_incomplete`
(`ContainedTaskBusy`); a Runtime shutdown during the drain fails it with
`scheduling_pause_drain_interrupted`, during the hand-back with
`scheduling_pause_release_interrupted` (`RuntimeUnavailable`). A failed device close (an
instance outside the multi-Nemu gate) fails the request with the close's own error; an
unconfirmed close stays fatal as on every close path. The
client's receipt wait is the drain timeout plus that grace plus its IO timeout.

## Resume

`ResumeScheduling { scope }` lifts the matching gate and bumps the revision of that scope. A
global resume answers `SchedulingResumed { scope, revision, selfcheck: absent }` and
reconnects nothing.

An instance resume (ps2) accepts a pause in stage `released` only. Under the instance
admission guard it lifts the gate and then reconnects the device at once instead of at the next
lazy open. Since Workflow #317 sc3 the reconnect is the instance's connection preparation phase
(`runtime-fact-store.md`, "Connection preparation phase"): under a dedicated preparation lease,
`ExecutionKernel::open_instance_backends` opens the instance's input and capture backends
through the same provider opens as the lazy paths (a Nemu pair opens once) and takes the first
frame of a capture it opened, as the first lazy capture does, then drops it (no frame artifact
is written). The opens are recorded like every open (`backend-open-observation.md`):
`backend.open_observed`, the `backend.selfcheck.*` facts, `device.self_check`, and the
instance's policy availability following the self-check. The lease is then released and the
session stays open for the next tasks (Workflow #191 H; a failed open, or an instance outside
the multi-Nemu gate, closes it before the release). The session kept through the pause, or
one a read-only observe retained after it (the existing sc2 design), is closed first, so a
resume is a disconnect and a reconnect and the receipt's self-check reflects this
preparation's complete opens (Workflow #191 h3). The receipt carries the self-check projected
from those reports:

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
- A side without a report of this resume has `ok: false` and no values.
- `failure_code` is the code of the failing step verbatim: a failed open's device code, for
  example `input_backend_open_failed`, `capture_backend_open_failed`,
  `capture_backend_operation_failed` or `paired_backend_open_failed`; a refused preparation
  lease's scheduler code (`lease_busy`, `lease_cooldown`, `lease_transfer_not_safe`, both sides
  `ok: false`); or a failed close's code.

A failed reconnect does not roll the resume back and is not retried: the resume is performed,
the failure is recorded, the instance stays unavailable until a self-check passes, and the
receipt reports it. Only an unconfirmed close or a failed record fails the request.
`actingctl selfcheck <alias>` (`SelfCheckInstance`) runs the same phase without a pause and
answers `InstanceSelfChecked { instance_alias, selfcheck }` with this `selfcheck` shape. The
client's receipt wait for an instance resume and for a self-check is the backend-open bound
`CONNECTION_PREPARATION_WAIT_MS` (`30_000` ms: the touch handshake and the capture prime) plus
its IO timeout (Workflow #191 h2); a global resume keeps the IO timeout. A resume triggers no
extra policy evaluation: the next cycle runs on its ordinary trigger.

## Conditional resume (Workflow #338 R4)

`ResumeScheduling` may carry `expected: { owner_epoch, revision }`, the owner epoch the caller
saw the pause in and the revision of its scope then (a pause receipt's `revision`, or the
`Status` pause's). The field is optional and absent from the wire when unset: a resume without
it is unchanged. `revision` is at least 1 (`invalid_scheduling_pause_expectation`).

With `expected`, the host compares while it holds the pause table's lock, before the state
checks and the lift of the table below: an `owner_epoch` other than its own is
`scheduling_pause_owner_epoch_mismatch`; a `revision` other than the scope's current revision
(zero for an instance scope never paused in this epoch) is `scheduling_pause_revision_mismatch`.
Both are refusals through the same `Denied` / `InvalidRequest` path as the other refusals. A
match then behaves exactly as an unconditional resume. Revisions restart with every owner
epoch, which is why the epoch is part of the condition.

An older Runtime does not know the field: `RuntimeOperation` denies unknown fields, the frame
fails to decode and the connection is dropped without a receipt (stage
`runtime.ipc.request_decode`, effect not performed). Only when the call wrote its own frame
and that connection then ended before the receipt header (end of stream, reset or abort) does
the client (`RuntimeClient::resume_scheduling_expected`) reconnect and, when the same owner
epoch answers, read `Status`; a connection already failed by an earlier call, a receipt
timeout and every other failure are returned unchanged. After such a drop, the scope still
paused at the expected revision means nothing was lifted and is reported as `runtime_operation_unsupported`; any other state, or a failed
reconnect or another epoch, is the uncertain `runtime_scheduling_resume_unconfirmed` (or
`runtime_owner_epoch_changed`). The client never falls back to an unconditional resume.

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
| Conditional resume naming another owner epoch | `Denied`, `InvalidRequest` | `scheduling_pause_owner_epoch_mismatch` |
| Conditional resume naming another revision of the scope | `Denied`, `InvalidRequest` | `scheduling_pause_revision_mismatch` |

## Status

`Status` (`RuntimeControlPlaneStatus`) carries `scheduling_pause: { revision, reason_code,
since_unix_ms }` while the global pause holds, and each `RuntimeInstanceStatus` carries
`pause: { revision, reason_code, since_unix_ms, stage: draining | paused | released }` while
its instance is paused. Both fields are absent otherwise (additive wire: a
`deny_unknown_fields` reader must move to this contract before it reads a paused status).

## Persistence, no expiry

A pause has no TTL; only `ResumeScheduling` lifts it. Since Workflow #361 B1 a pause survives a
restart. The held pauses are the runtime fact `host.scheduling_pause` (scope `Runtime`,
`record_list`, no lifetime; `contracts/runtime-fact-store.md`), one row per held pause:
`scope` (`global` or `instance`), `instance_alias` (instance rows), `reason_code`,
`since_unix_ms` (`timestamp_ms`), `set_in_owner_epoch` and, on a restored row,
`restored_from_owner_epoch`. An empty list means nothing is paused; a ledger without the fact
(any ledger older than this rule) starts unpaused, as before.

- **Write before change.** A pause records the rows it leads to before it closes the gate; a
  resume records the rows without its scope before it lifts the gate. Pause, resume and the
  restore are ordered by one persist gate. A refused record fails the request with
  `scheduling_pause_persist_failed` (`Failed`, `LedgerFailure`) and the gate stays as it was; a
  failed append poisons the Runtime as every runtime-fact append does. A process that dies
  between the record and the gate change restarts on the side the operator asked for. A failed
  instance pause stage lifts its gate and records the rows without it; if that record fails
  the request poisons the Runtime.
- **Restore at start.** A start that is not installer-held reads the fact before any physical
  instance is prepared and before the policy driver: the global pause comes back at revision 1
  in the new owner epoch, an instance pause at revision 1 with stage `released`, so the startup
  preparation, which skips a released instance pause, leaves its device untouched.
  Reason and `since` are kept. A pause of an alias that is no longer registered is dropped. The
  restored rows are recorded again with `restored_from_owner_epoch` (the owner epoch before
  this start); that record is the ledger trace of the restore and of every dropped row.
  `actingd` prints one stdout line per pause: `actingd scheduling_pause_restored
  scope=<global|instance:alias> since=<ms>` or `actingd scheduling_pause_dropped
  scope=instance:<alias> reason=instance_not_registered`. A malformed fact fails the start
  (`scheduling_pause_fact_invalid`).
- **Installer-held starts** keep the pauses their install transition carried
  (`host.install_transition.pauses`) and record them as `host.scheduling_pause`.
- **Conditional resume** after a restart needs the restored pause's new owner epoch and
  revision 1, read from `Status`.
- **Older Runtimes** ignore the fact: a switch back to one starts unpaused, and a resume made
  there does not clear the fact, so the next start of this Runtime restores that pause again
  (visible in `Status` and in the stdout line); resume it once more.

The ledger event set is unchanged: besides the fact records, a pause or resume leaves its trace
in the request receipt, the refused dispatch admissions, the terminals of the runs it stopped,
the device close's existing records (an instance outside the multi-Nemu gate) and the
reconnect's existing close and open records.

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
