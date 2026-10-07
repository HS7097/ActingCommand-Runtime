# Emulator instance control

Runtime slice #316-B. `RuntimeOperation::ControlEmulatorInstance { instance_alias, action }`
starts, stops or restarts the emulator instance itself through the provider that discovered it.
It is the only path that dispatches the vendor's documented `control` subcommand. The
application lifecycle inside a running instance (`ApplicationLifecycle`) is unchanged.

## Operation and origin gate

- `action` is `start`, `stop` or `restart` (`EmulatorInstanceAction`, snake_case). No lease or
  holder id is part of the request.
- `RuntimeRequest::validate` admits the operation only when `(actor, source)` is
  `(User, Ui)` or `(Cli, Cli)`; every other origin is `invalid_emulator_control_origin`.
  `Adapter` / `Agent` are excluded on purpose: the scheduler and agents can never issue it, and
  there is no policy that restarts the emulator by itself. The startup package (slice
  #316-B3, "Startup package hook" below) is the one thing a successful `start` / `restart`
  sets in motion: a configuration-declared, fully ledgered contained task, not a restart.
- The instance must be a registered physical instance (`fixture_execution_scope_forbidden`
  otherwise).

## Fence classification and the busy denial

The host takes the per-instance admission guard and classifies the instance exactly like
monitor recovery (`monitor_recovery_admission`): an active lease, an expired lease, an active
destructive step, a pending preemption, the takeover cooldown or queued lease requests each deny
the request with `RuntimeErrorCode::RuntimeBusy`, host code `emulator_control_busy`, receipt state
`denied`; the `runtime.failed` record carries the reason (`fence=<active_lease |
lease_expired | destructive_step_active | preemption_pending | takeover_cooldown |
queued_lease_requests>`) as native detail. Only `scheduler_available` proceeds. The admission
guard stays held for the whole request, so no lease is granted on that instance while the
emulator changes state. A concurrent READ-ONLY observation such as `capture_sequence` /
`observe` holds no lease and is therefore NOT fenced: it fails typed with `capture.failed` once
the session is closed; only lease holders, queued lease requests, destructive steps, preemption
and the takeover cooldown deny the control.

## Close before the action

For every action (`start` included since slice #316-B2, because a session must not outlive the
endpoint it was opened on) the instance's retained device session is closed first through
`close_retained_instance_while_guarded` (the daemon keeps running; other instances are
untouched; closing when no session is open is a no-op). A refusal because a lease is held is the
same busy denial; a close failure is recorded through the existing session-close lifecycle path
and fails the request. After `start` / `restart` of a device self-checked instance the connection
preparation phase (`runtime-fact-store.md`) reopens the session and keeps it open (Workflow #191 H:
the session belongs to the instance and only an external command disconnects it; an instance
outside the multi-Nemu gate closes it again, as before). After `stop` nothing reopens it; it
opens on its next use.

## Tool dispatch, timeouts and wait criteria

`actingcommand-device::mumu_manager::control_instance(manager_path, index, action, wait)`:

- Dispatches `MuMuManager.exe control -v <index> launch | shutdown | restart` (no console
  window), bounded by `MUMU_MANAGER_CONTROL_TIMEOUT` = 60 s. The vendor documents only the
  syntax: no return value, exit code, JSON or blocking behaviour. A non-zero exit
  (`mumu_manager.control_exit`; the exit code equals `errcode` when an envelope is present) or
  an `{"errcode","errmsg"}` envelope with `errcode != 0` and exit 0
  (`mumu_manager.control_errcode`) is a typed failure carrying the exit code and a bounded
  (<= 1 KiB, control characters other than `\n` `\r` `\t` stripped) stdout+stderr summary.
  `{"errcode": 0, "errmsg": ""}` with exit 0 is the success acknowledgment (observed, below)
  and the readiness wait proceeds; an `errcode` of `0` is never read as a failure by any
  `MuMuManager` reader. A spawn failure is `mumu_manager.run`; an expired 60 s bound is
  `mumu_manager.timeout`.
- Then polls `info -v <index>` every 1 s, each poll bounded by `MUMU_MANAGER_COMMAND_TIMEOUT`
  = 10 s, through the lax reader `read_instance_state` (`InstanceState`): the flat and the map
  shape are both accepted, `adb_port` and `player_state` may be absent (a stopped instance
  has neither), the booleans and a consistent `index` are required, `launch_err_code` /
  `launch_err_msg` default to `0` / empty when absent. A poll whose 10 s bound expires
  (`mumu_manager.timeout`, observed while stopping) is no observation yet: the loop keeps
  polling until the action deadline. Only an envelope with `errcode != 0`
  (`mumu_manager.errcode` / `.entry` / `.exit`), a shape or other typed reader failure
  (`.shape`, `.decode`, `.json`, `.output_bound`, `.run`) or `launch_err_code != 0` ends the
  wait early.
- Success: `start` / `restart` when `is_process_started && is_android_started && adb_port != 0`;
  `stop` when `!is_process_started`. Default waits `MUMU_MANAGER_STATE_WAIT_START` = 120 s and
  `MUMU_MANAGER_STATE_WAIT_STOP` = 120 s (raised from 60 s: `info` may not answer for more than
  30 s while the instance is stopping), counted from the moment `control` returned.
- Failure: `launch_err_code != 0` (`mumu_manager.launch_error`, carrying `launch_err_msg` in the
  summary), an `info` refusal (its existing typed stages, an expired poll excepted) or the
  deadline (`mumu_manager.wait_timeout`, carrying the last observed state, or `no observation`
  when every poll expired, and the count of expired polls).
- `player_state` is undocumented: it is recorded opaquely and never branched on.
- Caveat from the undocumented behaviour: if `control restart` reports the old running state
  before the shutdown phase, the wait may satisfy the running criterion early. The Runtime does
  not assume it either way; the bounds are Runtime policy.
- The hidden `api` subcommand is never dispatched.

### Observed on MuMuManager 6.5.7.0 (acceptance run of #437; observations, not vendor guarantees)

- `control -v 2 launch` returned in about 1 s with exit code 0 and stdout exactly
  `{"errcode": 0, "errmsg": ""}` (35 bytes, stderr empty). It does not block: 5 s later
  `info -v 2` showed `is_process_started=true`, `is_android_started=false`, `adb_port=16448`,
  `player_state="starting_rom"`; about 15 s later `is_android_started=true`,
  `player_state="start_finished"`. `info -v 2` answered normally throughout the launch.
- `control -v 2 shutdown` (dispatched by the Runtime) also exited 0; the instance went to
  `player_state="stopping"` with `is_android_started=false` while `adb_port` was still
  present, and during that phase `info -v 2` did NOT answer for more than 30 s (a bounded poll
  expired). A stopped instance answers `info -v 2` with a FLAT object:
  `{"android_version","created_timestamp","disk_size_bytes","error_code":0,"hyperv_enabled","index":"2","info_source":"rpc","is_android_started":false,"is_main","is_process_started":false,"name"}`:
  no `adb_host_ip`, no `adb_port`, no `player_state`, no `launch_err_code`, and the key is
  `error_code` (not `errcode`), so it is not an envelope.
- `info -v all` lists a stopped instance with those same flat fields (no `adb_port`).

## Cold start (slice #316-B2)

A configured instance that discovery reports stopped is bound PENDING at startup instead of
refused (`contracts/provider-startup.md`): the registry entry carries the discovered facts and
the host the binding will be completed with, but no port; the startup `runtime.instance_bound`
event carries `binding_source: discovered` with the discovered index, name and provider version
and with `adb_host` and `adb_port` omitted; `actingctl emulator status` reports `adb_port: null`
for it. Nothing is ever bound with a guessed port. A stopped instance elsewhere in the inventory
does not break discovery either.

While the binding is pending, every path that would open the instance's device session refuses
typed before any backend is touched, host code `instance_not_running` (`invalid_request`,
receipt state `denied`):

- lease acquisition (`acquire_lease` and `queue_lease`, before the scheduler prepares or queues
  anything) and read-only observation (`observe`, `capture_sequence`, `observe_contained_page`,
  after `command.received`) record `command.rejected` (diagnostic `runtime.diagnostic`, effect
  `not_performed`, the receipt terminal) plus one `runtime.failed` (stage `operation_cleanup`)
  whose native detail names the alias (`instance_alias=<alias>; adb_endpoint=pending; ...`);
- the monitor probe records `monitor.failed` (diagnostic `runtime.diagnostic`, runtime code
  `invalid_request`) plus the same `runtime.failed` instead of a command event, and the probe
  is rescheduled as after any other refusal.

Explicit entries and fixture instances are never pending. The registry keeps one last typed
guard so nothing can bypass the host checks: `open_input`, `open_capture` and
`control_application` on a pending entry fail with a device error at stage
`adb.endpoint_pending` (category `protocol`).

Emulator control itself works on a pending instance: the fence and the close-before step do not
require a bound endpoint. After a successful `start` or `restart`, still under the per-instance
admission guard, the registry binds the entry to the host it was registered with and the port the
control outcome reported (`EmulatorControlOutcome.adb_port`), the host refreshes its
`registered_instances` record (endpoint and audit endpoint together, under the registry lock
that every identity check now also holds), and one more `runtime.instance_bound` event for the
instance (same discovered fields, now with `adb_host` and `adb_port`, linked to the control
request) is appended after `command.validated` and before `runtime.fact_recorded`. A running
outcome that carries no port fails typed with `emulator_control_endpoint_unresolved`
(`backend_operation_failed`, receipt state `failed`, effect `indeterminate`); the binding stays as
it was and the request may be repeated.

Lease queue/acquire uses only the stable instance identity to locate its admission lock.
After acquiring that lock it resolves the current Host/registry binding again; the endpoint
check and scheduler preparation, queue context and commit use that same current binding.
Stop/start cannot admit or reject a waiting lease request using its earlier endpoint snapshot.
The existing active-lease/queued-request fence and device-registry endpoint gate remain.

ADB baseline (slice #316-B3): the vendor reports `running` a few seconds before adbd answers,
so a bound `start` / `restart` succeeds only once the ADB baseline answers. Still under the
admission guard, after the rebinding, the host probes the bound endpoint (`ensure_device`
with a connect attempt allowed) every 500 ms against one absolute 30 s deadline; the probes are not recorded,
the wait is part of the receipt's `elapsed_ms`. Each get-state/connect/get-state command uses
only its remaining budget, checks shutdown before spawn, during process polling and before
success, and the next poll/sleep shares that deadline. The existing bounded process/pipe
cleanup remains mandatory and can finish after the execution deadline; no late success or
hard FFI cancellation is claimed. Original command errors and unconfirmed cleanup survive;
unconfirmed resource closure remains fatal. Other callers retain their default command
timeouts, and the client receipt budget stays 230 s. A timeout fails typed with
`emulator_control_adb_not_ready` (`backend_operation_failed`, receipt state `failed`, effect
`indeterminate`; native detail carries the alias, the port, the milliseconds waited and the
last ADB error): `command.validated`, the second `runtime.instance_bound`, `device.connected`
and the startup package are all withheld, the binding keeps the reported port, and the
  request may be repeated. After a successful `stop` the entry returns to pending
and `status` shows `adb_port: null` again; no event beyond the existing `device.connected`
invalidation records that transition.

Note for #322 readers: the port recorded by the newest `runtime.instance_bound` of an instance
represents that instance from then on; older events keep the port they were recorded with.

The registry (`ExecutionBackendRegistry::control_instance`) serves discovery-bound entries only,
using the `MuMuManager.exe` path carried on the `DiscoveredInstanceBinding`; an explicit entry
refuses with `emulator_control.unavailable` (host code `emulator_control_unavailable`,
`invalid_request`, `denied`), a fixture instance likewise, and a provider without a control
surface keeps the trait default `emulator_control.unsupported`
(`emulator_control_unsupported`). No device session is opened or touched by the registry path.

## Event shape

Every request is recorded intent -> result, all linked to the instance:

1. `client.cli_command` (Cli) or `client.ui_action` (Ui) and `command.received`, action
   `emulator.instance.start | stop | restart`.
2. When a device session was open, the existing resource-close events of that session
   (`runtime.lifecycle_observed` with `resource_quiescence`, or the session-close failure).
3. On success `command.validated` with effect `performed` (the receipt terminal), then after
   `start` / `restart` one `runtime.instance_bound` carrying the resolved `adb_host` and
   `adb_port`, then `runtime.fact_recorded` for `device.connected` (and after `stop` a
   `runtime.fact_invalidated` with reason `device_closed`).
4. On denial or failure `command.rejected` (the receipt terminal; diagnostic
   `backend.operation_failed` for tool failures, `lease.fencing_denied` for the busy denial,
   `runtime.diagnostic` otherwise; effect `not_performed` before the tool ran, `indeterminate`
   once it ran) plus one `runtime.failed` (stage `operation_cleanup`) whose record carries
   `raw_os_error` = the tool exit code, `native_detail` = the bounded vendor output
   (Sensitive) and a primary detail `{category, stage, backend: mumu_manager, operation:
   control_instance}` declared Sensitive, where `stage` is the typed step
   (`mumu_manager.control_exit`, `.control_errcode`, `.launch_error`, `.wait_timeout`, `.run`,
   `.timeout`, `.path`, an `info` reader stage, `emulator_control.unavailable`, `.unsupported`).

Host codes: `emulator_control_busy` (`runtime_busy`, denied), `emulator_control_unavailable`
and `emulator_control_unsupported` (`invalid_request`, denied), `emulator_control_wait_timeout`,
`emulator_control_failed`, `emulator_control_endpoint_unresolved` and
`emulator_control_adb_not_ready` (`backend_operation_failed`, failed). Device-facing requests on a pending instance are denied with `instance_not_running`
(`invalid_request`), see "Cold start". The receipt carries the host code and its operation
in the optional, additive `host_code`/`host_operation` error-projection fields (closed static
codes; a client built before them cannot decode such a receipt, `deny_unknown_fields`), so
`actingctl` shows, for example, `host code emulator_control_busy during control_emulator_instance`.

## The `device.connected` program fact

On success the host records the instance-scoped runtime fact `device.connected` =
`boolean(running)` (`observed_at_unix_ms` = the host clock, source `runtime`, no TTL) through
`record_runtime_fact`, ledger first. After `stop` the key is additionally invalidated with reason
`device_closed`; an absent key is not an error there. This is the first producer of the Runtime
fact store (`runtime-fact-store.md`).

## Result schema

`RuntimeResult::EmulatorInstanceControlled`:

```json
{
  "kind": "emulator_instance_controlled",
  "instance_alias": "<alias>",
  "action": "start | stop | restart",
  "instance_index": 0,
  "running": true,
  "adb_port": 16384,
  "elapsed_ms": 12345,
  "startup_package": "none"
}
```

`adb_port` is omitted when the last observation carried none (a stopped instance). The receipt
state is `completed` with the `command.validated` event as terminal. `startup_package` is
`scheduled` when a successful `start` / `restart` handed the instance's configured startup
package to the host's scheduling thread (see "Startup package hook"), `none` otherwise
(nothing configured, or `stop`).

## Startup package hook (slice #316-B3)

An instance may declare `startup_package { "package": <locator>, "expected_sha256": <hex> }`
in its `actingd` configuration (`contracts/actingd-check-config.md`), the same locator and
digest semantics as `actingctl task-run --package / --expected-sha256`. Nothing is opened or
hashed at startup. The full contract lives in `contracts/application-lifecycle.md`; the part
that belongs to emulator control:

- Only a successful `start` / `restart` sets the package in motion, and success includes the
  ADB baseline answering ("Cold start" above); `stop`, a refused or failed action (including
  `emulator_control_adb_not_ready`), and a daemon that finds the instance already running at
  startup schedule nothing. A configured package is always invoked; an instance without one
  never has anything pulled.
- The control request only *schedules* it, after `command.validated`, the second
  `runtime.instance_bound` and the `device.connected` fact: one `runtime.lifecycle_observed`
  (phase `startup_package_scheduled { instance_id }`, the package locator in the audit machine
  path, links of the control request plus a freshly minted causation id) is appended, the
  entry is queued for the host's own scheduling thread, and the receipt returns as before with
  `startup_package: scheduled`. The 230 s control wait is never spent on the package.
- The scheduling thread (`actingcommand-runtime-startup`, a peer of the monitor thread) runs
  the package as an ordinary contained task under the same causation id: self-minted request,
  correlation and holder ids, origin `(Agent, Adapter)`, a synthesized connection, hash
  admission, its own lease, and the complete `command.received` -> `command.validated` ->
  `lease.*` -> `task.requested` ... `task.completed` / `task.failed` -> `lease.released` chain.
  Its success or failure is read from those events, never from the control receipt.
- The thread runs the package only after one more ADB baseline probe of the instance; a
  probe failure is `startup_package_adb_not_ready` (`backend_operation_failed`), recorded
  and consumed without a lease. Admission refusals fail typed before any lease:
  `startup_package_missing` when a ZIP locator does not open,
  `startup_package_admission_failed` for every other admission refusal (the underlying
  `contained_task_package_*` code attached as related failure, a resource declaration
  rejection carried along). For a digest-named content-directory locator (Workflow #288) a
  missing or unreadable directory is also `startup_package_admission_failed`, with the
  loader's code attached as related failure (for example `content_directory_missing` or
  `content_directory_digest_mismatch`). Every failure of the run is recorded as
  `runtime.failed` (stage `operation_cleanup`, category `startup_package`) linked to the
  instance and the causation id; a fatal one poisons the host as any other.

## Stuck-recovery ladder (slice #316-B4)

A scheduled (policy) contained task run on a physical instance whose
`task.failed` terminal carries `failure_code` `contained_task_page_unknown`, any
`contained_task_recovery_*` code or any `contained_task_home_recovery_*` code (entry recovery /
return home failed, for example `contained_task_home_recovery_persistently_non_home`) starts a
recovery ladder for the instance, unless the
instance's `stuck_recovery` is `false` (`contracts/actingd-check-config.md`). Fixture-simulated
instances, startup package runs and the ladder's own rung runs never trigger one. The
`contained_task_prerequisite_*` codes a `linear_steps` package's prerequisite gate reports
itself never trigger one, nor does `contained_task_return_home_entry_unmatched`; a code a
prerequisite or return-home package reports while it runs is judged by the rule above
(`contracts/linear-steps.md`, "Prerequisite packages" and "Return-home fallback"). A direct run
(`actingctl task-run`, the console's task run, MCP `ac_run_pack`) never starts a ladder
(Workflow #369-3, coordinator ruling Q1: the ladder exists for the routine; whoever ran it has
the receipt and decides); nothing is recorded for it. The ladder never runs on the run's thread:
a scheduled run's trigger (no client receipt) is admitted as the run returns its failure, and
the accepted ladder is queued for the scheduling thread of the startup package hook
(`actingcommand-runtime-startup`). `task.failed` and every receipt keep their shape; the
original task is never re-run.

Ordinary physical-instance backend failures during the daemon start's preparation also
enter this owner after the original temporary resources and installed session backends have
confirmed disposition, and the preparation lease has been released. The trigger carries
`stage: startup_preparation`, the actual preparation event reference
`preparation { sequence, event_id }`, and the original `failure_code`; it has no task/run IDs.
A connection preparation (`stage: connection_preparation`) is the preparation of a direct
request (emulator `start` / `restart`, an instance resume, `SelfCheckInstance`); since
Workflow #369-3 its failure is recorded as before and starts no ladder. Ledgers written
earlier may still hold ladders with that trigger stage.
Successful fallback, invalid input parameters/configuration, admission/budget denial,
owner/ledger failure and unconfirmed resource disposal do not enter this path. Preparation
performed by the ladder has `stage: recovery_preparation` and cannot trigger another ladder.
The current owner remembers only the latest preparation event. A subsequent preparation,
backend open or binding invalidation supersedes a queued preparation trigger.

Instance aliases, indices, tool paths and addresses come from formal configuration and the
registered provider binding. Provider control polls discover the current process state and
port; the new Start must match the registered instance index. Native process identity is
resolved by the configured backend when it opens on that binding. Application identities,
package locators/digests, game/server and target pages come from resource/configuration
declarations. The ladder contains no host-specific paths, ports, process IDs or game rules;
vendor protocols remain in the existing provider adapter.

Rungs, in this fixed order, each existing work under the instance lease:

- `return_home`: the failed run's bound recovery package (`--recovery-package`) runs as a
  standalone contained task (default response deadline, self-minted request / correlation /
  holder ids, its own lease and `task.*` chain, under the ladder's causation id). When the run
  had none bound, the return-home package that `actingd`'s `return_home_packages` names for the
  failed package's game and server runs instead, with the maximum response deadline; one that
  does not match the failed package's game, server or resolution, fails a check of a return-home
  chain layer or declares a prerequisite package of its own is refused before any lease with
  `contained_task_prerequisite_incompatible` (Workflow #336 L2d; `contracts/linear-steps.md`,
  "Return-home fallback"). Skipped with `no_recovery_package` when the run had none bound and
  none is configured. A preparation trigger resolves the game/server through the configured
  startup package's hash/containment admission, then uses its bound recovery package or the
  configured return-home mapping. Each actual run admits its own material. An ADB baseline
  probe failure, when the admitted actions need ADB, is `recovery_ladder_adb_not_ready`;
  admission refusals keep their `contained_task_package_*` code; failures are recorded as
  `runtime.failed` with category `recovery_ladder`.
- `application_restart`: the assigned application is restarted (Workflow #369-2, Alice's
  09-25 ruling "restore home, then restart the application, then restart the emulator"). The
  checks the startup package's run makes come first: a known unavailable capture or input
  channel its entry needs skips the rung (`capture_unavailable` / `input_unavailable`), and an
  ADB baseline that does not answer within 30 s skips it (`adb_unavailable`), with the game
  untouched. Then the assigned `application_id` is force-stopped by a host-minted
  `ApplicationLifecycle { stop }` request under the ladder's causation id, with its own lease
  (`command.received`, `command.validated`, `application.intent`, `application.completed`); a
  failed stop fails the rung with its code, recorded as `runtime.failed`. Then the instance's
  startup package is scheduled (`startup_package_scheduled` under the ladder's links, a fresh
  causation id) and run, as after `emulator start`; after the stop an unavailable entry fails
  the rung instead of skipping it. A startup package that cannot be admitted, whose
  prerequisite chain is refused, or that declares resource readings (a startup run's
  incompatibility) is run without the stop, so its refusal is recorded as before and the game
  is left alone. Skipped with `no_startup_package` when none is configured.
- `emulator_restart`: Stop and then Start through this contract's existing provider control
  path, under one instance admission guard. Each action records `command.received`, then
  `command.validated` or `command.rejected` + `runtime.failed`. Stop must observe the old
  process gone before `recovery_instance_stopped` is recorded and Start is considered.
  Admission/fencing is checked again before Start. Stop failure, timeout, ambiguous identity
  or unconfirmed close ends the rung without Start. Start binds its newly observed port and
  completes the existing ADB baseline. Then (Workflow #369-1) the rung waits for readiness
  within 120 s of Start: Android must report a resumed activity (the read-only foreground
  query of the ADB baseline; no session is opened) and a fresh input/capture preparation
  (stage `recovery_preparation`) must pass. Only `capture.ok && touch.ok && failure_code ==
  null` records `recovery_environment_ready`, naming the passing preparation. A failed
  preparation is retried only when the existing preparation rule calls it recoverable (an
  ordinary acquisition failure with confirmed disposal and successful cleanup) or the ADB
  baseline does not answer; any other failure ends the rung `recovery_environment_not_ready`
  at once. Before every retry the rung polls the ADB baseline every 500 ms until it answers
  again (bounded by the window), then waits 5 s, 10 s, then 20 s each time, never past the
  window; each attempt writes its own `instance_preparation_finished` and is rechecked for
  admission. The instance admission guard is held across Stop, Start and the first attempt,
  and released while waiting. The waits stop at once on shutdown
  (`recovery_ladder_shutdown_requested`) or an install drain
  (`recovery_ladder_drain_requested`). Not ready within the window fails the rung
  `recovery_environment_not_ready`, or `recovery_android_not_booted` when the boot check never
  passed and so no preparation ran. The ladder runs on the host's single host-work thread:
  while the rung waits, queued startup packages and ladders of other instances wait behind it,
  for at most the window; the policy thread is not blocked. Skipped with
  `no_emulator_control` when the instance is not discovery-bound. With no startup package the
  rung and ladder finish `environment_ready`. With a package, the rung then schedules it
  (`startup_package_scheduled` under the ladder's links) and runs it; the environment fact remains separate and its
  ordinary bounded run must reach the package target before the rung and ladder finish
  `recovered`. After the restart, an unavailable capture, input or ADB entry of that run fails
  the rung instead of skipping it. Shared managers, ADB servers and other instances are
  outside this instance control operation.

R1/R2 channel eligibility uses the hash-admitted program and current typed backend facts.
A known failed capture/input channel skips an entry that needs it with `capture_unavailable`
or `input_unavailable`; an actual failed ADB baseline required by application actions skips
with `adb_unavailable`. An `any` linear application entry needs neither capture nor input
before its application action. A page entry still needs capture for its entry/prerequisite
gate. Missing/unknown channel observations are attempted through the normal provider path.
Input unavailability never establishes ADB unavailability. A rung is tried at most once.

A rung recovers when its package run completes `success` (its target page reached); any other
end fails it, with the failure code as `reason` (`recovery_rung_target_not_reached` for a run
that completed without reaching its target, `recovery_ladder_shutdown_requested` when the host
is shutting down). The first successful application rung ends the ladder `recovered`; a
successful cold environment without a package ends it `environment_ready`. When every rung
failed or was skipped it ends `exhausted`. No result replays an uncertain business operation.

Cool-down: at most one ladder per instance per `stuck_recovery_cooldown_secs` (default 600),
measured from the accepted trigger. A trigger inside the window records
`recovery_ladder_suppressed { reason: "cooldown", until_unix_ms }` and does nothing else; a
trigger while a ladder for the instance is queued or running records `reason:
"already_running"` (`until_unix_ms` is the running ladder's window end).
Scheduling pause, shutdown or capacity denial suppresses admission and is rechecked at each
rung/control boundary (`admission_denied`). A superseded preparation trigger records
`preparation_superseded`; these two reasons use `until_unix_ms: 0` (no inferred wake time).

Facts are `runtime.lifecycle_observed` events (origin `(Runtime, Runtime, Runtime)`) linked to
the instance and the trigger's actual correlation id, under a fresh request id and the ladder's
own causation id. Phases (`kind`, snake_case, unknown fields refused):

| phase | fields | severity |
| --- | --- | --- |
| `instance_preparation_finished` | `stage`, `capture`, `input` (typed backend observation statuses), `failure_code?`; linked to the instance and real self-check request | failure warning; otherwise info |
| `recovery_ladder_started` | `trigger { stage, run_id?, task_id?, preparation?, failure_code }`, `rungs: [{ rung, state: "pending" \| "skipped", reason? }]` (all three rungs, in order; `reason` exactly when skipped) | info |
| `recovery_instance_stopped` | `stop { sequence, event_id }` (confirmed Stop command terminal) | info |
| `recovery_environment_ready` | `stop`, `start`, `preparation` (actual event references in increasing sequence order) | info |
| `recovery_rung_finished` | `rung`, `outcome: "recovered" \| "environment_ready" \| "failed" \| "skipped"`, `run_id?`, `reason?`, `environment?` | info; `failed` warning |
| `recovery_ladder_finished` | `outcome: "recovered" \| "environment_ready" \| "exhausted"`, `rungs_tried` (rungs executed, not skipped) | success info; `exhausted` error |
| `recovery_ladder_suppressed` | `reason: "cooldown" \| "already_running" \| "preparation_superseded" \| "admission_denied"`, `until_unix_ms` | admission denial warning; otherwise info |

Task triggers use `stage: task` (the deserialization default), require task/run IDs and omit
`preparation`. Preparation triggers require `preparation` and omit task/run IDs.
`environment_ready` is valid only for R3, with no run ID and an `environment` event reference.
`recovered` continues to require a real successful package run ID. Unknown fields are refused.

`rung` is `return_home`, `application_restart` or `emulator_restart`. A ladder with no
recovery package, no startup package and no emulator control records `recovery_ladder_started`
(all rungs skipped), three `recovery_rung_finished` (`skipped`) and `recovery_ladder_finished`
(`exhausted`, `rungs_tried` 0). Adding these phases is an additive wire change: readers built
against an older contract refuse the events.

Host configuration: `RuntimeHostConfig::with_stuck_recovery` takes the settings by instance
alias (an instance without an entry uses the defaults); `validate` refuses an invalid alias or
a cool-down outside `1..=86400` with `invalid_stuck_recovery`, and startup refuses an alias
that is not registered with `stuck_recovery_instance_unknown` (both fatal).

## Client and CLI

`RuntimeClient::control_emulator_instance(instance_alias, action)` sends the operation with an
explicit receipt wait of 230 s (`EMULATOR_CONTROL_RESPONSE_TIMEOUT`: 60 s tool bound + 120 s
readiness wait, the same for start, restart and stop + one 10 s poll that may straddle the
deadline + the 30 s ADB baseline wait after start / restart = 220 s, plus the IO margin) and
returns the `EmulatorInstanceControlled` result verbatim.

```
actingctl emulator status  --state-root <state-root> --instance <alias>
actingctl emulator start   --state-root <state-root> --instance <alias>
actingctl emulator stop    --state-root <state-root> --instance <alias>
actingctl emulator restart --state-root <state-root> --instance <alias>
```

`status` is the existing `Status` read filtered to the alias (that instance's
`RuntimeInstanceStatus`, or `instance_unknown`); `start` / `stop` / `restart` print the result
JSON above. All four require `--instance`.
`RuntimeInstanceStatus` also carries `resource_package { path, kind }` when the instance declares a
default resource package (`contracts/actingd-check-config.md`), and omits the key otherwise.

## Instance discovery query

`RuntimeOperation::DiscoverInstances` (`{"operation": "discover_instances"}`, no fields) re-runs
the provider's instance discovery on demand (`ExecutionBackendProvider::discover_instances`, the
same `MuMuManager` resolution as startup, see `provider-startup.md`). It spawns the vendor tool,
so its origin gate is the emulator control one: only `(User, Ui)` or `(Cli, Cli)`, otherwise
`invalid_emulator_control_origin`. No lease, admission guard, fence or device session is
involved, and nothing is bound, rebound or registered.

`RuntimeResult::InstancesDiscovered`:

```json
{
  "kind": "instances_discovered",
  "discovery": {
    "provider_version": "<MuMuManager version>",
    "source": {
      "event_id": "<command.validated event>",
      "sequence": 42,
      "sampled_started_at_unix_ms": 1790000000000,
      "sampled_completed_at_unix_ms": 1790000000800
    },
    "instances": [
      {
        "instance_index": 0,
        "instance_name": "<name>",
        "adb_host": "127.0.0.1",
        "adb_port": 16384,
        "running": true,
        "bound_alias": "<alias>",
        "android_version": "12"
      }
    ]
  }
}
```

Instances are ordered by index without duplicates; each name is 1-256 bytes (the startup
binding's `MAX_DISCOVERED_INSTANCE_NAME_BYTES`). `adb_host`, `adb_port`, `bound_alias` and
`android_version` are omitted when absent. `bound_alias` is the registered instance whose
discovery binding carries this index, else the explicit HOST:PORT instance configured with this
ADB port. No `MuMuManager` path, install root or resolution source is on the wire. The request records one `command.validated` event carrying the
`InstanceDiscovery` state observation (`runtime-state-observation.md`) and `source` points at
it; the receipt is `completed` without a terminal, like `Status`.

A refusal appends `command.rejected` (the receipt terminal, diagnostic
`backend.operation_failed` or `runtime.diagnostic`, effect `not_performed`) plus one
`runtime.failed` (stage `operation_cleanup`) whose `native_detail` keeps the provider's device
error; the primary detail `{category, stage, backend: execution_backend_provider, operation:
discover_instances}` is a controlled template declared Sensitive. Host operation
`discover_instances`:

| Provider refusal | Host code | Projection | Receipt |
| --- | --- | --- | --- |
| `instance_discovery_unavailable`, stage `instance_discovery.unsupported` (no discovery surface) | `instance_discovery_unavailable` | `invalid_request` | `denied` |
| `instance_discovery_unavailable`, any other stage (no install, tool spawn, exit, timeout, decode, JSON) | `instance_discovery_unavailable` | `backend_operation_failed` | `failed` |
| `mumu_manager_version_unsupported` | `mumu_manager_version_unsupported` | `backend_operation_failed` | `failed` |
| An answer the contract refuses (duplicate index, empty or over-long name) | `instance_discovery_unavailable` | `backend_operation_failed` | `failed` |

`RuntimeClient::discover_instances()` (and `RuntimeProjectClient::discover_instances()`) waits
25 s for the receipt: two vendor commands (`version`, `info -v all`) of at most 10 s each, plus
the IO margin.

`actingctl emulator discover --state-root <state-root>` (no `--instance`) prints the
`RuntimeInstanceDiscovery` above as one JSON line and exits 0; a refusal is the usual
`FATAL actingctl:` line with a non-zero exit.
