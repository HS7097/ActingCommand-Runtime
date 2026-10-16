# Emulator instance control

Runtime slice #316-B. `RuntimeOperation::ControlEmulatorInstance { instance_alias, action }`
starts, stops or restarts the emulator instance itself through the provider that discovered it.
It is the only path that dispatches the vendor's documented `control` subcommand. A restart is
a Stop phase and then a Start phase composed by the host (Workflow #381 B, "Restart" below);
the vendor's `control ... restart` is never dispatched. The application lifecycle inside a
running instance (`ApplicationLifecycle`) is unchanged.

## Operation and origin gate

- `action` is `start`, `stop` or `restart` (`EmulatorInstanceAction`, snake_case). No lease or
  holder id is part of the request: the host takes the instance's key for it (see "The
  operator's claim and the fence").
- `RuntimeRequest::validate` admits the operation only when `(actor, source)` is
  `(User, Ui)` or `(Cli, Cli)`; every other origin is `invalid_emulator_control_origin`.
  `Adapter` / `Agent` are excluded on purpose: the scheduler and agents can never issue it, and
  there is no policy that restarts the emulator by itself. The startup package (slice
  #316-B3, "Startup package hook" below) is the one thing a successful `start` / `restart`
  sets in motion: a configuration-declared, fully ledgered contained task, not a restart.
- The instance must be a registered physical instance (`fixture_execution_scope_forbidden`
  otherwise).

## The operator's claim and the fence

<!-- Workflow #369 S6b (H-4): emulator control as a holder of the instance's key. -->
Since Workflow #369 S6b (model-369-queue H-4) an emulator control is an operator claim on the
instance's key in its one queue (`ClaimKind::EmulatorControl`, holder minted by the host, the
scheduler's default lease TTL, the request's own connection):

- **Its turn.** After its `command.received`, the request waits at most 5 s for the key (W-3).
  A free instance with no eligible waiting claim grants it at once (`lease.requested`,
  `scheduler.admitted`, `lease.granted` under the request's links); otherwise it waits as an
  operator entry (`lease.requested` + `scheduler.queued`), which a scheduling pause does not
  hold back. Not granted within the 5 s, its entry is removed and the request is refused
  `lease_queue_expired` (`RuntimeErrorCode::QueueExpired`, receipt state `denied`; detail
  `timeout_ms=5000`, `holder_kind=<the holder's kind>` when the instance has a holder,
  `queue_position=<n>`): one `scheduler.denied` `lease.queue_expired` at Warning under the
  request's links is the receipt's terminal, and nothing is driven. A restart is one claim.
- **A stopped instance.** The claim takes neither the bound-endpoint check nor the
  performance-control lease gate, and is granted with drain capacity (Q-2a), so a pending
  (stopped) instance can be started; no `instance_not_running` is recorded for it.
- **One hold.** The claim holds the key through the Stop, the Start, the ADB baseline and the
  connection preparation. It is renewed before the provider step, to the step's bound plus 5 s
  (a Stop 210 s, a Start 220 s with the ADB baseline; each phase of a restart separately), and
  again before the preparation, to 30 s plus 5 s, before the action's `command.validated`
  (Q-5; review L-3). A renewal is recorded as every renewal is (`scheduler.admitted`,
  `lease.transition_intent`, `lease.renewed`, Info, under the request's links). A refused
  renewal (`lease_expired`, `lease_missing`) ends the hold with the action's failure pair
  (`command.rejected` + `runtime.failed`) carrying that code.
- **The fence.** Under the per-instance admission guard, held across the device steps so no
  keyless observe meets the changing emulator, the request is fenced on its own key: the
  instance's lease must be that key (lease id and holder id), unexpired, with no destructive
  step open; otherwise the request is denied `emulator_control_busy` (`RuntimeBusy`, receipt
  state `denied`; the `runtime.failed` record carries `fence=<active_lease | lease_expired |
  destructive_step_active | preemption_pending | takeover_cooldown | queued_lease_requests>` as
  native detail, from `monitor_recovery_admission`). Other holders, waiting claims and the
  takeover cooldown are met in the queue, not by the fence. A concurrent READ-ONLY observation
  such as `capture_sequence` / `observe` holds no key: it fails typed with `capture.failed` once
  the session is closed.
- **The end.** The hold ends with the request's explicit release (`lease.transition_intent`,
  `lease.released` under its links), after the action's records: after a `start` / `restart`
  whose preparation passed it hands the key to the startup package's claim ("Startup package
  hook"). A hold that hands no startup claim over (a stop, a failed preparation) first cancels
  the instance's queued startup claim; then, after a failed preparation that admits a ladder,
  the release hands the key straight to that ladder ("Stuck-recovery ladder"); otherwise it
  hands the key to the next eligible claim, or frees the instance. A failed action ends the
  key as a failed lease end does (`lease.released` under the lease's own links, the key handed
  on), except after an unconfirmed close, which keeps it. Since Workflow #369 S6c a failed
  action that hands no startup claim over, one whose provider step itself failed included,
  first cancels the instance's queued startup claim as well (coordinator ruling on review #716,
  item 1).

## Close before the action

For every action (`start` included since slice #316-B2, because a session must not outlive the
endpoint it was opened on) the instance's retained device session is closed first through
`close_retained_instance_while_guarded` under the control's key (the daemon keeps running; other
instances are untouched; closing when no session is open is a no-op); a close failure is recorded
through the existing session-close lifecycle path and fails the request. After `start` /
`restart` of a device self-checked instance the connection preparation phase
(`runtime-fact-store.md`) reopens the session on the control's key, with no dedicated preparation
lease, and keeps it open (Workflow #191 H: the session belongs to the instance and only an
external command disconnects it; an instance outside the multi-Nemu gate closes it again, as
before). After `stop` nothing reopens it; it opens on its next use.

## Tool dispatch, timeouts and wait criteria

`actingcommand-device::mumu_manager::control_instance(manager_path, index, action, wait)`:

- Dispatches `MuMuManager.exe control -v <index> launch | shutdown` (no console window),
  bounded by `MUMU_MANAGER_CONTROL_TIMEOUT` = 60 s. A `restart` reaching it is refused before
  any vendor command with the device stage `mumu_manager_restart_refused` (effect
  `not_performed`); the host never passes one (Workflow #381 B, coordinator answer D4). The vendor documents only the
  syntax: no return value, exit code, JSON or blocking behaviour. A non-zero exit
  (`mumu_manager_control_exit`; the exit code equals `errcode` when an envelope is present) or
  an `{"errcode","errmsg"}` envelope with `errcode != 0` and exit 0
  (`mumu_manager_control_errcode`) is a typed failure carrying the exit code and a bounded
  (<= 1 KiB, control characters other than `\n` `\r` `\t` stripped) stdout+stderr summary.
  `{"errcode": 0, "errmsg": ""}` with exit 0 is the success acknowledgment (observed, below)
  and the readiness wait proceeds; an `errcode` of `0` is never read as a failure by any
  `MuMuManager` reader. A spawn failure is `mumu_manager_run`; an expired 60 s bound is
  `mumu_manager_timeout`.
- Then polls `info -v <index>` every 1 s, each poll bounded by `MUMU_MANAGER_COMMAND_TIMEOUT`
  = 10 s, through the lax reader `read_instance_state` (`InstanceState`): the flat and the map
  shape are both accepted, `adb_port` and `player_state` may be absent (a stopped instance
  has neither), the booleans and a consistent `index` are required, `launch_err_code` /
  `launch_err_msg` default to `0` / empty when absent. A poll whose 10 s bound expires
  (`mumu_manager_timeout`, observed while stopping) is no observation yet: the loop keeps
  polling until the action deadline. Only an envelope with `errcode != 0`
  (`mumu_manager_errcode` / `_entry` / `_exit`), a shape or other typed reader failure
  (`_shape`, `_decode`, `_json`, `_output_bound`, `_run`) or `launch_err_code != 0` ends the
  wait early.
- Success: `start` when `is_process_started && is_android_started && adb_port != 0`; `stop`
  when `!is_process_started` and no vendor device process of the instance is left (below).
  Default waits `MUMU_MANAGER_STATE_WAIT_START` = 120 s and `MUMU_MANAGER_STATE_WAIT_STOP` =
  120 s (raised from 60 s: `info` may not answer for more than 30 s while the instance is
  stopping), counted from the moment `control` returned.
- Stop's process check (Workflow #381 B F2, review M2): a stop is confirmed only when `info`
  reports `!is_process_started` **and** the system process listing shows no `MuMuNxDevice.exe`
  carrying `-v <index>` (observed on 10-09: Android stopped in about 11 s, the vendor's device
  process lived 41 s). The listing runs only after `info` reported the process stopped, at most
  once per `MUMU_MANAGER_PROCESS_LIST_INTERVAL` = 5 s, each one bounded by
  `MUMU_MANAGER_PROCESS_LIST_TIMEOUT` = 10 s (one PowerShell `Win32_Process` query, 1-3 s in
  practice, no console window). A listing that fails or passes its bound is a failed listing:
  `info`'s answer confirms the stop and the outcome records `device_process_present=unknown`
  (a warning, below); a failed listing never becomes a timeout. The outcome of a stop carries
  `device_process_present` (`false`, or `unknown`); a failed stop carries `true` when its last
  listing still showed the device process, `unknown` otherwise. A stop's worst case is 60 s
  (`control`) + 120 s (wait) + 10 s (last poll) + 10 s (last listing) = 200 s.
- Failure: `launch_err_code != 0` (`mumu_manager_launch_error`, carrying `launch_err_msg` in the
  summary), an `info` refusal (its existing typed stages, an expired poll excepted) or the
  deadline (`mumu_manager_wait_timeout`, carrying the last observed state, or `no observation`
  when every poll expired, and the count of expired polls).
- `player_state` is undocumented: it is recorded opaquely and never branched on.
- The hidden `api` subcommand is never dispatched.

## Restart (Workflow #381 B)

An operator `restart` (`actingctl emulator restart`, MCP `ac_emulator restart`) is a Stop phase
and then a Start phase under one instance admission guard, each through the same drive path as
`stop` and `start` (fence, close before the action, provider control, rebinding, self-check
invalidation, ADB baseline after the Start), as the ladder's emulator-restart rung does. Each
phase is its own provider call with its own deadline, measured from its own `control` return:
the Stop with `MUMU_MANAGER_STATE_WAIT_STOP`, the Start with `MUMU_MANAGER_STATE_WAIT_START`;
the vendor's `restart` verb is never dispatched (the 10-09 incident: the vendor's self-restart
of its shell was lost when that shell's own shutdown hung, and the instance stayed stopped).

- The Start follows a confirmed Stop, or a Stop that ended in `mumu_manager_wait_timeout`. After
  any other Stop failure (`control_exit`, `control_errcode`, `run`, `launch_error` and every
  other stage) no Start is dispatched.
- A restart succeeds only when both phases are confirmed; it then ends exactly as a successful
  start does (connection preparation, startup package scheduling), and the receipt result is
  `EmulatorInstanceControlled { action: restart, .. }` with `elapsed_ms` the whole request's
  provider and ADB baseline time.
- A restart whose Stop was not confirmed by its provider control fails
  `emulator_restart_stop_failed` (phase `stop`) whatever its Start did; a confirmed Stop
  followed by a failed provider control of the Start fails `emulator_restart_start_failed`
  (phase `start`). Every other failure of a phase (a busy fence, a session close, a rebinding,
  the ADB baseline) fails the restart with its own code, as for `start` and `stop`.
- Bounds: a Stop phase takes at most 200 s, 210 s when it fails (the state read below); a Start
  phase at most 220 s on success (with the 30 s ADB baseline), 200 s on failure. The client
  waits 445 s for a restart's receipt ("Client and CLI").

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
  after `command.received`) record `command.rejected` (diagnostic `runtime_diagnostic`, effect
  `not_performed`, the receipt terminal) plus one `runtime.failed` (stage `operation_cleanup`)
  whose native detail names the alias (`instance_alias=<alias>; adb_endpoint=pending; ...`);
- the monitor probe records `monitor.failed` (diagnostic `runtime_diagnostic`, runtime code
  `invalid_request`) plus the same `runtime.failed` instead of a command event, and the probe
  is rescheduled as after any other refusal.

Explicit entries and fixture instances are never pending. The registry keeps one last typed
guard so nothing can bypass the host checks: `open_input`, `open_capture` and
`control_application` on a pending entry fail with a device error at stage
`adb_endpoint_pending` (category `protocol`).

Emulator control itself works on a pending instance: its claim, the fence and the close-before
step do not require a bound endpoint (Workflow #369 Q-2a). After a successful Start (a `start`, or a restart's Start phase), still
under the per-instance admission guard, the registry binds the entry to the host it was
registered with and the port the control outcome reported (`EmulatorControlOutcome.adb_port`), the host refreshes its
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
Stop/start cannot admit or reject a waiting lease request using its earlier endpoint snapshot:
while the control holds the instance's key no other lease is granted on it (Workflow #369 H-4),
and the device-registry endpoint gate remains.

ADB baseline (slice #316-B3): the vendor reports `running` a few seconds before adbd answers,
so a bound Start succeeds only once the ADB baseline answers. Still under the
admission guard, after the rebinding, the host probes the bound endpoint (`ensure_device`
with a connect attempt allowed) every 500 ms against one absolute 30 s deadline; the probes are not recorded,
the wait is part of the receipt's `elapsed_ms`. Each get-state/connect/get-state command uses
only its remaining budget, checks shutdown before spawn, during process polling and before
success, and the next poll/sleep shares that deadline. The existing bounded process/pipe
cleanup remains mandatory and can finish after the execution deadline; no late success or
hard FFI cancellation is claimed. Original command errors and unconfirmed cleanup survive;
an unconfirmed cleanup of the wait's own ADB child or pipe fences the instance and fails the
action with `instance_resources_fenced` ("Resource fence" below, Workflow #387 D1c-G); it ends
the Runtime only where no fence may take it. Other callers retain their default command
timeouts; the client receipt budgets are in "Client and CLI". A timeout fails typed with
`emulator_control_adb_not_ready` (`backend_operation_failed`, receipt state `failed`, effect
`indeterminate`; native detail carries the alias, the port, the milliseconds waited and the
last ADB error): `command.validated`, the second `runtime.instance_bound`, `device.connected`
and the startup package are all withheld, the binding keeps the reported port, and the
  request may be repeated. After a successful `stop` the entry returns to pending
and `status` shows `adb_port: null` again; no event beyond the existing `device.connected`
invalidation records that transition.

State after a failure (Workflow #381 B F4, review M3): after any failed provider control
(`start`, `stop`, either phase of a restart, the ladder's rung 3) the host reads the instance's
state once through the provider's read-only surface (`info -v <index>`, bounded by
`MUMU_MANAGER_COMMAND_TIMEOUT` = 10 s, no device session) and applies it, still under the
admission guard and before the failure is recorded:

- stopped (`!is_process_started`): the binding returns to pending, the `backend.selfcheck.*`
  facts are invalidated, `device.connected` is recorded `false` and the stop invalidations
  follow (`device.connected`, the foreground and the task facts, `device_closed`);
- running: the reported port is bound (a changed binding records one more
  `runtime.instance_bound` under the request's links) and the self-check facts are invalidated;
- no settled observation (the read failed, or the instance is between the two): when the vendor
  command ran (effect `indeterminate`) the self-check facts and `device.connected` are
  invalidated; otherwise nothing changes, so a refusal that changed nothing keeps a running
  instance connected.

Each request still gets exactly one terminal.

Note for #322 readers: the port recorded by the newest `runtime.instance_bound` of an instance
represents that instance from then on; older events keep the port they were recorded with.

The registry (`ExecutionBackendRegistry::control_instance`) serves discovery-bound entries only,
using the `MuMuManager.exe` path carried on the `DiscoveredInstanceBinding`; an explicit entry
refuses with `emulator_control_binding` (host code `emulator_control_unavailable`,
`invalid_request`, `denied`), a handset (`device.kind` `handset`, Workflow #398 RD-1b,
[device-binding.md](device-binding.md)) and a fixture instance likewise, and a provider without a control
surface keeps the trait default `emulator_control_support`
(`emulator_control_unsupported`). No device session is opened or touched by the registry path.

## Event shape

Every request is recorded intent -> result, all linked to the instance:

1. `client.cli_command` (Cli) or `client.ui_action` (Ui) and `command.received`, action
   `emulator.instance.start | stop | restart`. A restart then writes, for each phase under the
   restart's links, `command.received` with the phase's action (`emulator.instance.stop`, then
   `emulator.instance.start`) and the phase's own events below; on success the Start phase's
   `command.validated` is the receipt terminal (Workflow #381 B M1: the event sequence of a
   restart changed, its structure did not).
2. When a device session was open, the existing resource-close events of that session
   (`runtime.lifecycle_observed` with `resource_quiescence`, or the session-close failure).
3. On success `command.validated` with effect `performed` (the receipt terminal), then after a
   Start one `runtime.instance_bound` carrying the resolved `adb_host` and `adb_port`, then
   `runtime.fact_recorded` for `device.connected` (and after a Stop a
   `runtime.fact_invalidated` with reason `device_closed`). A Stop that `info` alone confirmed
   because its process listing failed first records one `runtime.failed` at Warning,
   `emulator_stop_process_unverified` (`instance_id`, `device_process_present=unknown`,
   `stop_elapsed_ms`), under the request's links (on rung 3 the ladder's), before its
   `command.validated`.
4. On denial or failure `command.rejected` (the receipt terminal; diagnostic
   `backend_operation_failed` for tool failures, `lease_fencing_denied` for the busy denial,
   `runtime_diagnostic` otherwise; effect `not_performed` before the tool ran, `indeterminate`
   once it ran) plus one `runtime.failed` (stage `operation_cleanup`) whose record carries
   `raw_os_error` = the tool exit code, `native_detail` = the bounded vendor output
   (Sensitive) and a primary detail `{category, stage, backend: mumu_manager, operation:
   control_instance}` declared Sensitive, where `stage` is the typed step
   (`mumu_manager_control_exit`, `_control_errcode`, `_launch_error`, `_wait_timeout`, `_run`,
   `_timeout`, `_path`, an `info` reader stage, `emulator_control_binding`,
   `emulator_control_support`).
   The primary detail's message carries `key=value` tokens: the stage, exit status, elapsed
   time and last state of the failure, and for a stop `device_process_present`.
5. A failed restart records one `command.rejected` (action `emulator.instance.restart`, the
   receipt terminal) and one `runtime.failed`, after its Start phase when one ran (coordinator
   answer D2); a failed phase records nothing of its own. For `emulator_restart_stop_failed` and
   `emulator_restart_start_failed` the primary detail's stage, the exit code and the native
   detail are the failing phase's, and the message carries the tokens `restart_phase=stop|start`,
   `stop_result=confirmed|<the token the host's stop-result table gives the Stop's stage>`
   (vocabulary `emulator_stop_result`, Workflow #378 A2-P), `stop_elapsed_ms`,
   `start_dispatched`, `start_elapsed_ms` (when the Start ran), `process_started`,
   `android_started`, `adb_port_present` (the last state, when known: the Start's success, or
   the state read after the failing phase, or its last observation), `device_process_present`
   (the Stop's process check) and `cause=<emulator_control_wait_timeout |
   emulator_control_failed>`, the failing phase's control code until #378 A2 makes it a
   `caused_by` link. A Start that succeeded after a Stop whose wait expired still prepares the
   connection and schedules the startup package before that record.

Host codes: `lease_queue_expired` (`queue_expired`, denied; the claim's wait, see "The operator's
claim and the fence"), `emulator_control_busy` (`runtime_busy`, denied), `emulator_control_unavailable`
and `emulator_control_unsupported` (`invalid_request`, denied), `emulator_control_wait_timeout`,
`emulator_control_failed`, `emulator_control_endpoint_unresolved`,
`emulator_control_adb_not_ready`, and for a restart `emulator_restart_stop_failed` and
`emulator_restart_start_failed` (`backend_operation_failed`, failed; registered, pending since
0.12.0); `emulator_stop_process_unverified` is the stop's Warning record. Device-facing requests on a pending instance are denied with `instance_not_running`
(`invalid_request`), see "Cold start". The receipt carries the host code on top of its
`outcome`, with its operation as the `operation` value (receipt v2, Workflow #378 A4), so
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
`scheduled` when a successful `start` / `restart` handed its key to, or queued, the claim of the
instance's configured startup package (see "Startup package hook"), `none` otherwise (nothing
configured, a failed preparation, or `stop`).

## Startup package hook (slice #316-B3)

An instance may declare `startup_package { "package": <locator> }` in its `actingd`
configuration (`contracts/actingd-check-config.md`), the same locator semantics as
`actingctl task-run --package` (Workflow #336 PKG-S4: no digest). Nothing is opened or
hashed at startup. The full contract lives in `contracts/application-lifecycle.md`; the part
that belongs to emulator control:

- Only a successful `start` / `restart` sets the package in motion, and success includes the
  ADB baseline answering ("Cold start" above); `stop`, a refused or failed action (including
  `emulator_control_adb_not_ready`), and a daemon that finds the instance already running at
  startup schedule nothing. A configured package is always invoked; an instance without one
  never has anything pulled.
- The control request only *schedules* it, after `command.validated`, the second
  `runtime.instance_bound`, the `device.connected` fact and a passing connection preparation:
  one `runtime.lifecycle_observed` (phase `startup_package_scheduled { instance_id }`, the
  package locator in the audit machine path, links of the control request plus a freshly
  minted causation id) is appended, the control's key goes to the package's claim, and the
  receipt returns as before with `startup_package: scheduled`. The control's receipt wait is
  never spent on the package.
- Workflow #369 H-4, M6 (S6b): the claim is a routine claim (high priority, no deadline, never
  preempting). The control hands it the instance's key as it releases the key, through the
  gate: with the gate open the claim's own `lease.requested` + `scheduler.queued` (under the
  causation id) are followed by the control's `lease.transition_intent`, the claim's
  `lease.transition_intent`, `lease.transferred` and the control's `lease.released`; the
  claim takes no `lease.granted` of its own and bypasses the queue, so no dispatch can see the
  instance free between the start and the startup run. While a scheduling pause holds the
  instance or the global gate (a restored pause included), the claim waits queued as a gated
  routine entry and the control's key is released: no startup run happens until the resume,
  whose reconnect claim runs first and whose release hands the key to the startup claim
  (`contracts/scheduling-pause.md`). A claim of a kind that takes the admission checks passes
  them first; a refusal is recorded as the package's failure and the receipt still says
  `scheduled`. A new startup claim cancels one still queued for the same instance
  (`scheduler.denied lease_queue_cancelled`, warning), and so does a control whose hold hands
  no startup claim over (a `stop`, a `start` / `restart` whose preparation failed, and since
  Workflow #369 S6c a failed control, one whose provider step failed included): a startup run
  against a stopped or unprepared emulator is meaningless, so a start under a pause followed
  by a stop leaves no startup run at the resume.
- The instance's worker (`actingd-instance-<alias>`, Workflow #369 W-2) runs the package on
  the claim's key as an ordinary contained task under the same causation id: self-minted
  request and correlation ids, the claim's holder and synthesized connection, origin
  `(Agent, Adapter)`, hash admission, no lease of its own, and the complete
  `command.received` -> `command.validated` -> `task.requested` ... `task.completed` /
  `task.failed` -> `lease.released` chain (the release under the run's links ends the claim's
  hold). Its success or failure is read from those events, never from the control receipt.
- The worker runs the package only after one more ADB baseline probe of the instance; a
  probe failure is `startup_package_adb_not_ready` (`backend_operation_failed`), recorded
  before the run, and the claim's key is released. Admission refusals fail typed before the
  run:
  `startup_package_missing` when the locator does not exist (the loader's
  `content_directory_missing`), `startup_package_admission_failed` for every other admission
  refusal (the loader's code attached as related failure, a resource declaration rejection
  carried along). Workflow #336 PKG-S1: every locator is read by its path and no digest is
  compared. Every failure of the run is recorded as
  `runtime.failed` (stage `operation_cleanup`, category `startup_package`) linked to the
  instance and the causation id; a fatal one poisons the host as any other.

## Stuck-recovery ladder (slice #316-B4; Workflow #387 D1c)

Workflow #387 D1c (frozen model M387 v1.2): what starts a ladder, the rungs it climbs, their
bounds, the hold, the cool-down and five rules are the `recovery_ladder` section of `actingd`'s
configuration (`contracts/runtime-config.md`, "The recovery ladder section"), with every default
in `actingd.defaults.json` and none in code (Alice 10-09). Each key can change while the Runtime
runs (`ConfigureSettings`); a ladder takes the section in effect when it is admitted as its
snapshot and climbs by that one to its end, queue included. The defaults are the behaviour
before D1c, except that the trigger items now read a code's class (see "Triggers").

### Triggers

The section's `triggers` are an ordered list of trigger items. An item names its `id`, the
`stages` it looks at (`task`, `startup_preparation`, `connection_preparation`,
`stability_warnings`), and `classes` (ladder classes of the outcome catalog) or `codes` (codes,
or `prefix*`), with `start`, `start_rung` and `respect_cooldown` taken from `trigger_defaults`
when it leaves them out. A code is matched as follows: when `exclude_codes` names it (or a
prefix of it) nothing starts; otherwise the first item, in order, that names the stage and the
code (or its prefix, by code) or the code's class in the embedded catalog (by class) starts the
ladder; when none does, nothing starts and nothing is recorded. A code the embedded catalog does
not list reads as class `none` and is recorded once per process as
`recovery_ladder_code_unclassified` (warning, `raw_code`).

The defaults (M387 §7.1): `navigation_exhausted` (stage `task`, code
`contained_task_navigation_recovery_exhausted`, `start_rung` 3: a navigation recovery chain
already returned home and restarted the application, so the climb starts at the emulator restart;
Workflow #373 M-373 §15 ruling 1); `pack_logic` (stage `task`, class `pack_logic_mismatch`);
`stability_burst` (stage `stability_warnings`, classes `stability_warning` and
`environment_fault`, 3 records within 600 000 ms); `preparation_backend` (the two preparation
stages, codes `input_backend_open_failed`, `capture_backend_open_failed`,
`paired_backend_open_failed`, `capture_backend_operation_failed`); exclusions
`contained_task_prerequisite_*` and `contained_task_return_home_entry_unmatched` (the gate codes
of `contracts/linear-steps.md`, "Prerequisite packages" and "Return-home fallback").
`contained_task_navigation_entry_unconfirmed` is not excluded (Workflow #373 NAV-5, wave-17
acceptance ruling 7, in place of M-373 §9.4's proposal): the page list says the run reached its
entry and the package's own check says it did not, so the Runtime does not know where it is,
which is the ladder's job, as for `contained_task_page_unknown`; it starts the ladder by
`pack_logic`.

- **Task stage.** A scheduled (policy) contained task run on a physical instance whose
  `task.failed` terminal's outcome code (ledger revision 3; the relayed spelling's `raw_code`
  for `outcome_relay_unregistered`, `scheduled_failure_code_str`) an item matches at `task` starts a
  ladder for the instance, unless the instance's `stuck_recovery` is `false`. Fixture-simulated
  instances, startup package runs and the ladder's own rung runs never trigger one. A direct
  task run (`actingctl task-run`, the console's task run, MCP `ac_run_pack`) starts none while
  `rules.direct_runs` is `exempt` (the default; Workflow #369-3, coordinator rulings Q1 and P1:
  the ladder exists for the routine; whoever ran it has the receipt and decides); under
  `trigger` its failed end queues the ladder its item admits once its key is released. The
  ladder never runs on the run's thread. Workflow #369 H-1: a scheduled run's ladder is decided
  inside the run's failed lease end, before its release; an admitted ladder whose item starts
  `immediately` turns that end into a transfer of the instance's key to the ladder's claim (the
  claim's own `lease.requested` + `scheduler.queued`, high, no deadline; the releaser's and the
  receiver's `lease.transition_intent`; `lease.transferred` naming the claim; then
  `lease.released` under the run's links), so nothing can take the instance between the failure
  and the climb. An item that starts `after_performance_release` hands nothing on: the run's
  key is released and the ladder's claim waits in the instance's queue, as a routine claim does,
  until performance control releases the instance (until P-s: while its directive asks for
  neither suspension nor shutdown); then it takes the key without a person. `task.failed` and
  every receipt keep their shape; the original task is never re-run.
- **Preparation stages.** Ordinary physical-instance backend failures during startup or
  connection preparation also enter this owner after the original temporary resources and
  installed session backends have confirmed disposition, when an item matches the preparation's
  failure code at its stage. Its ladder is queued as a claim (routine, high, no deadline): after
  a daemon-start preparation, whose dedicated lease has been released, the free instance grants
  it at once; after a preparation on a claim's key (an emulator `start` / `restart` since
  Workflow #369 S6b, a resume's reconnect, a self-check), the claim's release hands that key
  straight to the ladder's claim (its own `lease.requested` + `scheduler.queued`, then
  `lease.transferred`, no `lease.granted` of its own), so no claim queued earlier, such as a
  startup claim held back by a pause, takes the instance first (H-1, P1). With no hand-off (a
  pause closed the gate since the admission, a destructive step is open, or the ladder waits for
  performance release or behind another ladder) the ladder waits as a claim and the key is
  released. A connection preparation is the one after emulator `start` / `restart` (including
  the `actingctl emulator start` of a logon script), an instance resume or `SelfCheckInstance`;
  its failure keeps starting a ladder (Workflow #369-3, coordinator ruling P1). Successful
  fallback, invalid input parameters/configuration, admission/budget denial, owner/ledger
  failure and unconfirmed resource disposal do not enter this path. Preparation performed by the
  ladder has `stage: recovery_preparation` and never triggers another ladder (no item may name
  that stage). The current owner remembers only the latest preparation event; a ladder whose
  preparation a later one superseded is suppressed (`preparation_superseded`) while
  `rules.preparation_superseded` is `suppress` (the default), and climbs under `keep`.
- **Direction ① (`stability_warnings`, M387 §3.4; Workflow #387 D1c-H, Alice 10-14).** As
  each record is committed, the Runtime counts, in memory and per instance, failures: a record
  of warning severity or worse whose outcome code a `stability_warnings` item matches counts as
  part of the failure it belongs to, and one failure counts once, however many matching records
  it commits. A record's failure is, in this order: the run it belongs to (its `run_id` link);
  else the request it answers (its `request_id` link); else the failure of the record it entered
  from (a `runtime.failed` record's `entered_event_id`, followed to the failure that record
  belongs to); else the record alone. So a lifecycle failure and its cause records, a run's
  records (`capture.failed`, `runtime.failed`, `task.failed`), a monitor probe's records, a
  command's records, a preparation's records, and a fence record and the unconfirmed failure it
  entered from each count once. A failure counts at its first matching record; the item's `count`-th
  failure within its `window_ms` registers a burst, and every window of the instance starts
  empty again, while the failures already counted stay counted, so their later records do not
  count again. An item that names both `environment_fault` and `stability_warning` (as the
  default `stability_burst` does) counts failures of the two classes in one window. Not counted:
  a record without an outcome (a backend open observation carries none: an open failure counts
  through the preparation, capture, input or task record that carries it); the records of an
  instance a ladder climbs on (its own work); while `rules.direct_runs` is `exempt`, every record
  committed while manual work holds the instance's key, the set a resource fence treats as manual
  (a direct task run, a client lease, an application or input command, a Lab operation), under
  `trigger` they count; and codes the catalog does not list. An emulator control command is not
  manual work, so its records count, and the connection preparation after an emulator `start` or
  `restart` still starts its ladder as above (coordinator ruling P1). The count takes no lock but
  its own: it reads the holder kind the scheduler last reported for the instance's key (the
  scheduler reports it under its own lock at every grant, transfer, release, expiry and
  rollback), and it records nothing and admits nothing on the write path. The lease sweep admits
  each burst's ladder (trigger `stage: stability_warnings`, the burst's last code, no run, task
  or preparation) and queues its claim. Its return-home rung takes the recovery package the
  instance's startup package resolves, as a preparation's ladder does.
  Windows and failures are memory only: a restart starts every window empty.
- **Resource fence (Workflow #403).** An unconfirmed close of one instance's own resources (an
  application, input or capture backend, or a session close, that could not confirm its
  processes or handles gone) fences that instance instead of ending the whole Runtime. The work
  that met it fails with `instance_resources_fenced` (non-fatal), recorded once as a
  `runtime.failed` caused by the unconfirmed failure and entering from that failure's record
  when it has one (`entered_event_id`, Workflow #387 D1c-H); the instance's policy availability is
  withheld, a policy dispatch to it is refused with that code, and its routine claims wait. The
  lease sweep hands the fence to a ladder admitted as every ladder is (trigger
  `stage: resource_fence` with the unconfirmed failure's code, no run, task or preparation, and
  no trigger item: it starts at `trigger_defaults.start_rung` and respects the cool-down as
  `trigger_defaults.respect_cooldown` says), whose claim waits for the instance's key; a ladder
  already holding or waiting for the instance decides instead. An ADB check of one instance's
  own whose ADB child or pipe could not be confirmed gone fences that instance the same way
  (Workflow #387 D1c-G): a ladder's checks (the `application_restart` entry probe; the
  `emulator_restart` readiness wait's boot check and ADB probes), the startup package's ADB probe
  (a startup run, a rung's startup run, a return-home run), the foreground gate's ADB query
  (`contracts/application-lifecycle.md`) and the ADB wait after an emulator `start` or `restart`
  ("ADB baseline" above). The work that met it fails with `instance_resources_fenced`; a rung
  that met it fails, and the ladder, which holds the instance, goes on and decides at its end.
  A fence that manual work met
  (the instance's key held by a direct task run, a client lease, an application or input command
  or a Lab operation) starts no ladder and waits for a person at once while `rules.direct_runs` is
  `exempt` (M387 B1, coordinator ruling 4 of D1c); under `trigger` it goes to a ladder as a
  routine fence does. Its return-home rung is skipped (`resource_fence`): it never prepares the
  instance again. The application restart rung, after its stop, prepares the instance again on
  the key; the emulator restart rung prepares it in its readiness wait. Only a preparation whose
  self-check passes proves the close and lifts the fence (`resource_fence_lifted`, naming that
  preparation); a person's resume (allowed while the instance is fenced without a pause),
  emulator start or restart, or self-check lifts it the same way. A ladder that ends with the
  instance still fenced, or a fence no ladder may take, leaves the instance waiting for a person
  (`Status`: `resource_fence.awaiting_person`). The fence is memory only: a restart prepares and
  self-checks the instance again, as every start does. Unconfirmed resources that no single
  registered instance owns, and ledger failures, still retain the owner and end the Runtime. So
  does any unconfirmed close once the Runtime is shutting down (a requested shutdown, or after a
  fatal): a fence would end with the process before a ladder or a person could take it, so the
  shutdown records the close failure and the owner journal says `unconfirmed`, as before.

Instance aliases, indices, tool paths and addresses come from formal configuration and the
registered provider binding. Provider control polls discover the current process state and
port; the new Start must match the registered instance index. Native process identity is
resolved by the configured backend when it opens on that binding. Application identities,
package locators/digests, game/server and target pages come from resource/configuration
declarations. The ladder contains no host-specific paths, ports, process IDs or game rules;
vendor protocols remain in the existing provider adapter.

### Rungs

`rungs` lists one to three actions, each at most once, in the order they climb; an element that
names only its `action` takes every bound from the defaults file's element of that action. The
default order is `return_home`, `application_restart`, `emulator_restart` (Alice 09-25). A
trigger item's `start_rung` (from 1) skips the rungs before it (`before_start_rung`); the plan
still lists them. Each rung is existing work under the ladder's key (Workflow #369 H-3). Before
each rung run the ladder renews the key to the run's response deadline plus the heartbeat
reserve and queues its own continuation (ordered first, never gated); the run's release, under
its own run links, hands the key back to that continuation. Rung 2's entry and stop, and rung
3's Stop, Start and readiness wait, are each preceded by a renewal to their configured bound
plus `hold.margin_ms`; a claim and each continuation are granted `hold.claim_ttl_ms`, and a key
within `host_claims.renewal_slack_ms` of a step's bound is not renewed:

- `return_home`: the failed run's bound recovery package (`--recovery-package`) runs as a
  standalone contained task (default response deadline, self-minted request and correlation
  ids, the ladder's holder and connection, no lease of its own, its `task.*` chain under the
  ladder's causation id). When the run had none bound, the return-home package the standard-pack
  registry of the failed package's game names for its server (Workflow #373 NAV-3a,
  `bundle.json` `maintenance[]` use `return_home`) runs instead, with the
  rung's `configured_package_deadline_ms`; one that is not the package of its registry entry is
  refused with `contained_task_prerequisite_mismatch`, one that does not match the failed package's game,
  server or resolution, fails a check of a return-home chain layer or declares a prerequisite
  package of its own is refused before the run with `contained_task_prerequisite_incompatible`
  (Workflow #336 L2d; `contracts/linear-steps.md`, "Return-home fallback"). Skipped with
  `no_recovery_package` when the run had none bound and the registry names none (a broken game
  root is recorded as a `standard_pack_read` lifecycle record and names none). A ladder without a
  failed run (a preparation's, a burst's) resolves the game/server through the configured
  startup package's containment admission, then uses its bound recovery package or the
  registry's return-home package. Each actual run admits its own material. An ADB baseline probe
  failure, when the admitted actions need ADB, is `recovery_ladder_adb_not_ready`; admission
  refusals keep the loader's code; failures are recorded as `runtime.failed` with category
  `recovery_ladder`.
- `application_restart`: the assigned application is restarted (Workflow #369-2). The checks
  the startup package's run makes come first: a known unavailable capture or input channel its
  entry needs (`capture_unavailable` / `input_unavailable`), or an ADB baseline that does not
  answer within the rung's `adb_probe_ms` (`adb_unavailable`), skips the rung, or fails it with
  that code under `rules.entry_unavailable = fail`, with the game untouched. Then the assigned
  `application_id` is force-stopped by a host-minted `ApplicationLifecycle { stop }` request
  under the ladder's causation id, on the ladder's key (`command.received`,
  `command.validated`, `application.intent`, `application.completed`); a failed stop fails the
  rung with its code, recorded as `runtime.failed`. Then the instance's startup package is
  scheduled (`startup_package_scheduled` under the ladder's links, a fresh causation id) and run,
  as after `emulator start`; after the stop an unavailable entry fails the rung instead of
  skipping it. A startup package that cannot be admitted, whose prerequisite chain is refused,
  or that declares resource readings (a startup run's incompatibility) is run without the stop,
  so its refusal is recorded as before and the game is left alone. Skipped with
  `no_startup_package` when none is configured. The entry hold (`entry_hold_ms`) covers the probe
  and the stop.
- `emulator_restart`: Stop and then Start through this contract's existing provider control
  path, under one instance admission guard, on the ladder's key: the control's fence admits that
  key (lease id and holder id, no destructive step open), the retained session is closed under
  it, and the readiness preparation runs on it and releases nothing. Each action records
  `command.received`, then `command.validated` or `command.rejected` + `runtime.failed` (a
  failed Stop's record carries `device_process_present`, `true` when its last process listing
  still showed the device process; coordinator answer D5). A confirmed Stop records
  `recovery_instance_stopped` and the Start follows. Workflow #381 B F3: a Stop that ended in
  `emulator_control_wait_timeout` also leads on to the Start and the readiness wait; its
  `command.rejected` serves as the `stop` of `recovery_environment_ready`, and no
  `recovery_instance_stopped` is written. Every other Stop failure, an ambiguous identity or an
  unconfirmed close ends the rung without Start, with its code as the rung's reason; a Start
  that fails or times out fails the rung with its code, so an instance left stopped is reported,
  never left down silently. Admission/fencing is checked again before Start. Start binds its
  newly observed port and completes the existing ADB baseline. Then (Workflow #369-1) the rung
  waits for readiness within `readiness_window_ms` of Start: Android must report a resumed
  activity (the read-only foreground query of the ADB baseline; no session is opened) and a
  fresh input/capture preparation (stage `recovery_preparation`) must pass. Only
  `capture.ok && touch.ok` without a failure `outcome` records `recovery_environment_ready`,
  naming the passing preparation. A failed preparation is retried only when it is an ordinary
  acquisition failure with confirmed disposal and successful cleanup whose code the trigger
  items name for a connection preparation, or the ADB baseline does not answer; any other
  failure ends the rung `recovery_environment_not_ready` at once. Before every retry the rung
  polls the ADB baseline every `readiness_poll_ms` until it answers again (bounded by the
  window), then waits the next `readiness_backoff_ms` value (its last value repeated), never past
  the window; each attempt writes its own `instance_preparation_finished`, makes the ladder's
  holding check and renews the key to the window's end. The instance admission guard is held
  across Stop, Start and the first attempt, and released while waiting. The waits stop at the
  next poll on shutdown (`recovery_ladder_shutdown_requested`), an install drain
  (`recovery_ladder_drain_requested`), a scheduling pause or a capacity refusal
  (`recovery_admission_denied`) or a lost key (`recovery_ladder_key_lost`). Not ready within the
  window fails the rung `recovery_environment_not_ready`, or `recovery_android_not_booted` when
  the boot check never passed and so no preparation ran. The ladder runs on its instance's
  worker and holds only that instance: other instances' host work runs on their own workers,
  and the policy thread is not blocked. Skipped with `no_emulator_control` when the instance is
  not discovery-bound. With no startup package the rung and ladder finish `environment_ready`.
  With a package, the rung then schedules it (`startup_package_scheduled` under the ladder's
  links) and runs it; the environment fact remains separate and its ordinary bounded run must
  reach the package target before the rung and ladder finish `recovered`. After the restart, an
  unavailable capture, input or ADB entry of that run fails the rung instead of skipping it.
  Shared managers, ADB servers and other instances are outside this instance control operation.
  `stop_bound_ms` and `start_bound_ms` may not be shorter than the device layer's own Stop and
  Start bounds (the section check refuses them).

R1/R2 channel eligibility uses the hash-admitted program and current typed backend facts. A
known failed capture/input channel makes an entry that needs it unavailable
(`capture_unavailable` or `input_unavailable`); an actual failed ADB baseline required by
application actions makes it unavailable with `adb_unavailable`; such a rung is skipped, or
fails under `rules.entry_unavailable = fail`. An `any` linear application entry needs neither
capture nor input before its application action, and neither does a page-graph application
entry, which a rung runs first (Workflow #390 EXEC-3, [Application
lifecycle](application-lifecycle.md)). A page entry still needs capture for its
entry/prerequisite gate. Missing/unknown channel observations are attempted through the normal
provider path. Input unavailability never establishes ADB unavailability. A rung is tried at
most once.

A rung recovers when its package run completes `success` (its target page reached); any other
end fails it, with the failure code as `reason` (`recovery_rung_target_not_reached` for a run
that completed without reaching its target, `recovery_ladder_shutdown_requested` when the host
is shutting down). The first successful application rung ends the ladder `recovered`; a
successful cold environment without a package ends it `environment_ready`. When every rung
failed or was skipped it ends `exhausted`. No result replays an uncertain business operation.

### Ladder profiles (Workflow #398 RD-4)

Model-398 v1.1 §6.2-§6.6, option B (coordinator ruling of 2026-10-15): a ladder climbs the rungs
of the profile its instance takes. `recovery_ladder.profiles` names profiles
(`{"<name>": {"rungs": [...]}}`; a name is 1 to 64 bytes of lowercase letters, digits and `_`,
starting with a letter), each profile's `rungs` written and checked as the section's are;
`recovery_ladder.kind_profiles` gives a device kind (`emulator`, `handset`;
[device-binding.md](device-binding.md)) a profile; an instance's own `recovery_ladder_profile`
(its `instances[]` entry) comes first. The order of use is the instance's key, then the kind
table, then the section's `rungs`. The defaults give the kind `handset` the profile `handset`:
`return_home`, then `application_restart`, with no emulator restart and no new action (a handset
has no emulator to restart, and the Runtime never restarts the phone); an emulator names no
profile and climbs the section's `rungs`. A ladder takes its profile with its snapshot when it is
admitted, so a profile changed while it waits applies to the next trigger only, and its
`recovery_ladder_started` names it (`profile`). A start rung is a position in the rungs the
instance climbs; `recovery_ladder.rules.start_beyond_profile`, the sixth rule beside B1-B5, says
what a start beyond a profile's rungs does:

- `exhaust` (the default): the climb starts, every rung of the profile is recorded skipped
  (`before_start_rung`), and the ladder ends `exhausted` with `rungs_tried` 0, as a ladder whose
  rungs are all skipped ends: no rung runs, the key is released once, and the cool-down opens as
  `rules.cooldown_opens_on` says. On a handset under the defaults, `navigation_exhausted` (start
  rung 3: the navigation recovery already returned home and restarted the application) ends this
  way, so nothing that recovery just did is done again;
- `last_rung`: the climb starts at the profile's last rung (on the default handset profile, the
  application restart);
- `refuse`: every trigger item's start rung and `trigger_defaults.start_rung` must lie within
  every profile's rungs; the section check refuses any other with `recovery_ladder_config_invalid`
  (`start_rung_out_of_range`, `config_key` `recovery_ladder.profiles.<name>`) at start and in
  check-config, and through an instruction the same value is `config_value_invalid` and the
  configuration in effect stays.

Under `exhaust` and `last_rung` a start rung is checked against the section's `rungs` only, as
before. No code, skip reason or record member is added for the rule. B5 applies to a handset's
start gate ([application-lifecycle.md](application-lifecycle.md), "The handset start gate"): a
rung whose run the gate holds before its first step (`device_locked`, `device_thermal_hold`,
`device_battery_low`, `device_in_use`) is skipped `device_not_ready`
(`recovery_rung_skipped_device_not_ready`), or fails with that code under
`rules.entry_unavailable = fail`; after the application restart's stop it fails, as every
unavailable entry after the stop does.

### Cool-down, suppression and interruption

Cool-down: `cooldown_secs` (default 600), overridden per instance by
`stuck_recovery_cooldown_secs`; at most one ladder per window. The window opens when a ladder
gets the key under `rules.cooldown_opens_on = admitted` (the default), or whenever an item
matches under `triggered`. A trigger inside the window whose item respects the cool-down
records `recovery_ladder_suppressed { reason: "cooldown", until_unix_ms, rule }` and does
nothing else; a trigger while a ladder holds or waits for the instance (read from the
scheduler) records `reason: "already_running"` (`until_unix_ms` is the window end) under
`rules.already_running = suppress` (the default), and under `queue` waits behind that ladder in
the same queue and is judged against the cool-down again when it gets the key. Scheduling
pause, shutdown or capacity denial suppresses admission (`admission_denied`).

The rules that stay in code (M387 §2.2b, Alice 10-10): only a physical instance starts a ladder;
the ladder's own preparation never does; shutdown and an install drain refuse a trigger and end
a climb; a lost key ends a climb; a capacity refusal and a scheduling pause refuse a trigger and
end a climb; an interrupted climb ends `exhausted` at warning and one whose rungs failed on their
own at error. Once a ladder holds the key (Workflow #369 H-3, ruling Q8), every rung boundary,
every control step and every readiness poll checks that it still holds the key and that no
scheduling pause or capacity refusal (`recovery_admission_denied`), install drain
(`recovery_ladder_drain_requested`) or shutdown (`recovery_ladder_shutdown_requested`)
intervenes; a key it no longer holds is `recovery_ladder_key_lost` (its continuation is
cancelled). Such an interruption fails the current rung with that reason and ends the ladder
`exhausted` at warning, followed by its one release. A superseded preparation trigger records
`preparation_superseded`; these two reasons use `until_unix_ms: 0` (no inferred wake time).

### Drills (Workflow #390 DRILL-1)

Forced compromise T-08 (the recovery route could not be tried by hand) and coordinator ruling 9
of wave 13: a person may ask for a drill of one physical instance's ladder,
`DrillRecoveryLadder { instance_alias, mode }`, an option-two operation (`actingctl
ladder-drill --instance <selector> [--device]`, MCP `ac_ladder_drill` with `instance` and
`device`; [runtime-interface.md](runtime-interface.md)). Only an explicit User+Ui or Cli+Cli
request may issue it (the emulator control origin gate: a drill may restart the emulator), and
only for a physical instance. The direct-run rule does not change: a direct task run still
starts no ladder while `rules.direct_runs` is `exempt`.

- **Recorded like a real ladder, tagged `drill`.** The request is recorded as a command
  (`command.received`, then `command.validated` or `command.rejected`). The drill is a ladder
  with trigger `stage: drill`, `failure_code` `recovery_ladder_drill_requested`, no run, task,
  preparation, rule or match, and `start: immediately`; it matches no trigger item, so it starts
  at `trigger_defaults.start_rung` and the cool-down holds it as
  `trigger_defaults.respect_cooldown` says, as a resource fence's ladder does. It is admitted as
  every ladder is (shutdown, the scheduling pause, business capacity, a ladder that holds or
  waits for the instance under `rules.already_running`, the cool-down), its claim
  is queued (routine, high, no deadline; claim kind `ladder`, so a later trigger sees it as a
  ladder), and it climbs on the instance's worker by the same plan, holding checks and records as
  any ladder. Every record of its own (`recovery_ladder_started`, `recovery_rung_finished`,
  `recovery_instance_stopped`, `recovery_environment_ready`, `recovery_ladder_finished`,
  `recovery_ladder_suppressed`) carries the payload member `drill: true`; no other record
  carries it, and a `recovery_ladder_started` carries it exactly when its trigger's stage is
  `drill`. Its records are linked to the instance and the request's correlation id, under the
  drill's own request id (`ladder_request_id`) and causation id.
- **Never the device unless the request says so.** `mode: plan` (the default of both frontends)
  never acts on the device: the plan is recorded as it is, every rung the ladder would run is
  recorded `skipped` with `recovery_rung_skipped_drill_plan` (reason `drill_plan`), and the climb
  ends `recovery_ladder_finished { ladder_result: "planned", rungs_tried: 0 }` at info. A plan
  drill changes nothing the ladder keeps: it opens no cool-down window and leaves a resource
  fence's stage as it was. `mode: device` climbs the rungs on the instance exactly as a real
  ladder does, under the same key, admission guard, fences and holding checks, opens the
  cool-down window as a ladder that got the key does, and ends as a real climb ends.
- **Answer and refusals.** Once the drill is admitted and its claim queued the request completes
  (`request_drill_recovery_ladder_completed`) with `RecoveryLadderDrillQueued { drill:
  {instance_alias, mode, ladder_request_id} }`, before the drill climbs. Refused, after its
  `command.rejected`: `recovery_ladder_drill_disabled` (stuck recovery is off for the instance),
  `recovery_ladder_drill_already_running` and `recovery_ladder_drill_cooling_down`
  (`next_eligible_unix_ms`), each after the drill's `recovery_ladder_suppressed`, and
  `recovery_admission_denied` when shutdown, a pause or capacity refused the admission (also
  after its suppression record); a refused claim keeps the claim's own refusal.

### Records

Facts are `runtime.lifecycle_observed` events (origin `(Runtime, Runtime, Runtime)`) linked to
the instance and the trigger's actual correlation id, under a fresh request id and the ladder's
own causation id. Phases (`kind`, snake_case, unknown fields refused):

| phase | fields | severity |
| --- | --- | --- |
| `instance_preparation_finished` | `stage` (a preparation stage: never `task` or `stability_warnings`), `capture`, `input` (typed backend observation statuses), `outcome?` (the preparation's failure); linked to the instance and real self-check request | a failure: its outcome's category; otherwise info |
| `recovery_ladder_started` | `trigger { stage, run_id?, task_id?, preparation?, failure_code, rule?, matched?, start }`, `rungs: [{ rung, state: "pending" \| "skipped", reason? }]` (one to three, in the configured order, no action twice; `reason` exactly when skipped), `config_digest` (64 lowercase hexadecimal digits: the SHA-256 of the section's values the ladder climbs by), `profile?` (the name of the ladder profile the instance climbs by, Workflow #398 RD-4; absent when it climbs the section's `rungs`) | info |
| `recovery_instance_stopped` | `stop { sequence, event_id }` (confirmed Stop command terminal) | info |
| `recovery_environment_ready` | `stop`, `start`, `preparation` (actual event references in increasing sequence order) | info |
| `recovery_rung_finished` | `rung`, `rung_result: "recovered" \| "environment_ready" \| "failed" \| "skipped"`, `run_id?`, `outcome?` (why a rung failed or was skipped; a skip is `recovery_rung_skipped_<reason>`, info), `environment?` | its outcome's category when it has one; otherwise info |
| `recovery_ladder_finished` | `ladder_result: "recovered" \| "environment_ready" \| "exhausted" \| "planned"`, `rungs_tried` (rungs executed, not skipped; at most 3; 0 for `planned`, above 0 for `recovered` and `environment_ready`), `outcome?` (exactly when `exhausted`: `recovery_ladder_exhausted`, caused by the last failed rung's outcome, or `recovery_ladder_interrupted`, caused by the holding interruption); `planned` only on a drill's record (Workflow #390 DRILL-1) | success and `planned` info; `exhausted` error (`recovery_ladder_exhausted`) or warning (`recovery_ladder_interrupted`) |
| `recovery_ladder_suppressed` | `reason: "cooldown" \| "already_running" \| "preparation_superseded" \| "admission_denied" \| "blackout"` (`blackout`: a trigger inside a blackout, `until_unix_ms` its end; written from SR-4a on, read from v0.12.0 on, [scheduling-blackout.md](scheduling-blackout.md)), `until_unix_ms`, `rule?` (the trigger item that matched; absent for a fence's ladder) | admission denial warning; otherwise info |

Ledger revision 3 (Workflow #378 A3): a phase's severity is its `outcome`'s category, else
the event type's (`info`); `outcome` is the record envelope (`contracts/v012-register.md`
§1.9, E3, E6).

A trigger names `rule` (the item's `id`: 1 to 64 bytes of lowercase letters, digits and `_`,
starting with a letter) and `matched`: `{ "by": "code" }`, `{ "by": "class", ladder_class }` or
`{ "by": "warning_burst", count, window_ms, first_unix_ms, last_unix_ms }`; and `start`
(`immediately` or `after_performance_release`). Validation checks only that a record agrees
with itself (no code table): task triggers use `stage: task` (the deserialization default),
require task/run IDs and a rule matched by code or class, and omit `preparation`; preparation
triggers require `preparation` (sequence above 0), a rule matched by code or class and omit
task/run IDs; `stability_warnings` triggers name nothing but a rule matched as a warning burst of
at least one record in a window above 0 whose first record is not after its last;
`recovery_preparation` triggers are refused; `resource_fence` and `drill` triggers name no run,
task, preparation, rule or match. A record of any other phase never carries `drill: true`, and an
`instance_preparation_finished` never names the stage `drill` (Workflow #390 DRILL-1).
`environment_ready` is valid only for R3, with no run ID and an
`environment` event reference. `recovered` continues to require a real successful package run
ID. Unknown fields are refused.

`rung` is `return_home`, `application_restart` or `emulator_restart`; a skip `reason` is
`no_recovery_package`, `no_startup_package`, `no_emulator_control`, `capture_unavailable`,
`input_unavailable`, `adb_unavailable`, `resource_fence`, `before_start_rung`,
`device_not_ready` (a handset's start gate held the rung's run before its first step;
Workflow #398 M398 §6.4, §6.5, written from RD-4 on) or `drill_plan` (a plan drill did not run the rung; Workflow
#390 DRILL-1). A ladder with no recovery package, no startup package and
no emulator control records `recovery_ladder_started` (all rungs skipped), one
`recovery_rung_finished` (`skipped`) per rung and `recovery_ladder_finished` (`exhausted`,
`rungs_tried` 0).

Host configuration: the `recovery_ladder` and `host_claims` sections reach the host in the
configuration manifest (a host started without one reads the defaults file's).
`RuntimeHostConfig::with_stuck_recovery` takes per-alias settings for an instance the manifest
does not carry; `validate` refuses an invalid alias or a cool-down outside `1..=86400` with
`invalid_stuck_recovery`, and startup refuses an alias that is not registered with
`stuck_recovery_instance_unknown` (both fatal).

## Configured start at daemon start (Workflow #369 S6c)

<!-- Workflow #369 S6c (model-369-queue v3.1 H-5, §7 row 8): #643's configured start rebuilt as
one claim per instance in the instance's one queue. -->
An instance configured with `"start_emulator": true` (`actingd` configuration, Workflow #369
S6a: off unless the instance names it; discovery-bound instances only,
`start_emulator_requires_discovery_binding` otherwise; manifest parameter
`instance.<instance_id>.start_emulator`) whose emulator is stopped when the daemon starts (its
ADB endpoint is pending) is started by the Runtime itself, on an autostart claim
(`ClaimKind::Autostart`). An instance already running gets the usual start preparation only.

- **When.** `actingd` enqueues the claims once, after policy initialization succeeded (without
  a `policy` section, right after the host started) and before `runtime_ready`; the enqueue
  does not block, so `runtime-info.json` is not delayed. Host configuration:
  `RuntimeHostConfig::with_emulator_autostart` takes the aliases in configuration order;
  startup refuses an alias that is not registered (`emulator_autostart_instance_unknown`) or a
  fixture instance (`emulator_autostart_requires_physical_instance`), both fatal, with the
  alias; `RuntimeHost::enqueue_emulator_autostart` enqueues the claims.
- **The claim.** A Runtime-internal routine claim: no deadline (`scheduler.queued` carries
  `u64::MAX`), normal priority, never preempting, under its own `AcquireLease` request (origin
  `(Agent, Adapter)`, the host's synthesized connection), with a 120 s key. Like an operator
  control it takes neither the bound-endpoint check nor the performance-control lease gate and
  is granted with drain capacity, so a stopped (pending) instance is granted it. Every claim is
  visible: a holder from the enqueue (`lease.requested`, `scheduler.admitted`, `lease.granted`
  under the claim's request; status `lease_active`), or, while a scheduling pause holds the
  instance or the global gate (a restored pause included), a gated entry from the first moment
  (`lease.requested` + `scheduler.queued`; status `queued_request_count` 1); the resume grants
  it. A takeover cooldown is waited out in the queue. A pending instance stays withheld from
  policy until its start and preparation pass.
- **Network first.** Once granted, the claim waits on its instance's worker
  (`actingd-instance-<alias>`) while it holds the key, never on a shared thread. It reads the
  operating system's connectivity status at once and every 5 s, for at most 600 s (on Windows
  `GetNetworkConnectivityHint`; internet access and constrained internet access count). The
  Runtime opens no connection and resolves no name. An unknown status (the operating system
  reports an unknown level, the query fails or does not exist) goes on at once and is reported
  once. When no internet access is reported within 600 s, the start is refused: the claim's
  `command.received`, then `command.rejected` + `runtime.failed` with
  `emulator_autostart_network_unready` (`alias`, `elapsed_ms`), and the claim releases its key;
  the instance stays stopped.
- **Holding checks.** The wait ticks every 500 ms. At each tick the claim renews its key to
  120 s whenever less than 30 s is left (the usual renew set, Info, under the claim's request;
  a refused renewal refuses the start with the pair, carrying `lease_expired` or
  `lease_missing`), and it ends its hold before the Start:
  - on a drain or a shutdown: it releases its key (the next daemon start enqueues it again);
  - when an operator emulator control waits on the instance: it is superseded and releases its
    key with no `command.*` record, so the operator's control is granted within its 5 s; once
    the Start is being driven the claim does not yield, and an operator control that arrives
    then waits its 5 s and may be refused `lease_queue_expired` naming holder `autostart`;
  - when a scheduling pause holds the instance or the global gate: it queues again as a gated
    entry, then releases its key, so the pause completes; the resume grants it again;
  - when the instance already runs (an operator started it): it releases its key and does
    nothing.
- **Order.** The claims holding their keys take a start turn one at a time, the first in
  configuration order first, so their Starts follow one another in that order; a claim that
  waits queued takes part once it is granted. The turn holds no key and no queue position, and
  a claim waiting for it keeps renewing its key.
- **The Start.** With the turn, after a last check of the above, the claim drives the operator
  start path ("The operator's claim and the fence") on its own key: its `command.received`
  (Runtime origin, written only now, so a claim that ended before leaves no open command), the
  fence on its key, the close, the renewal before the provider step, the provider Start, the
  binding, the ADB baseline, the renewal before the preparation, `command.validated` or
  `command.rejected` + `runtime.failed`, `runtime.instance_bound`, `device.connected` and the
  connection preparation on the key. A passing preparation hands the key to the startup
  package through the gate ("Startup package hook"); a failing one hands it to the ladder it
  admits ("Stuck-recovery ladder"); a failed start ends the key as a failed operator control
  does.
- **Report.** Each outcome is one report line, which `actingd` writes as a daemon line
  (`OUTCOME actingd:` with the developer line, Workflow #378 A6a): `emulator_autostart_queued`
  (`count`) when the claims are enqueued, nothing when no instance names `start_emulator`;
  per claim `emulator_autostart_network_ready` (`alias`, `elapsed_ms`) or
  `emulator_autostart_network_status_unknown` (`alias`, `connectivity_reason`, `os_error`), and
  one of `emulator_autostart_started`, `emulator_autostart_preparation_failed` (the backend
  self-check after the start did not pass), `emulator_autostart_failed` (`caused_by` the
  failure), `emulator_autostart_network_unready`, `emulator_autostart_superseded`,
  `emulator_autostart_yielded`, `emulator_autostart_skipped` (already running) and
  `emulator_autostart_interrupted` (a drain or a shutdown), each with the `alias`. A report
  line that cannot be written never stops the daemon. Only a failure the Runtime does not
  survive (a ledger append) stops the host.

## Client and CLI

`RuntimeClient::control_emulator_instance(instance_alias, action)` sends the operation with an
explicit receipt wait (`emulator_control_response_timeout(action)`) and returns the
`EmulatorInstanceControlled` result verbatim:

- start and stop: 235 s (`EMULATOR_CONTROL_RESPONSE_TIMEOUT`). A start takes 60 s tool bound +
  120 s readiness wait + one 10 s poll that may straddle the deadline + the 30 s ADB baseline
  = 220 s; a stop 60 + 120 + 10 s + one 10 s process listing that may straddle the deadline,
  + the 10 s state read after a failure = 210 s; plus the IO margin (230 s), plus at most 5 s
  for the emulator-control claim's turn in the instance's queue (Workflow #369 Q-2, W-3, S5).
- restart: 445 s (`EMULATOR_RESTART_RESPONSE_TIMEOUT`, Workflow #381 B, coordinator answer
  D3): a Stop phase of at most 210 s and a Start phase of at most 220 s, plus the 10 s IO
  margin (440 s), plus the claim's 5 s wait for its turn once, since a restart is one
  emulator-control claim (Workflow #369 S5, D1). MCP `ac_emulator` states both figures.

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
      "ledger_committed_head": 42,
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
ADB port. No `MuMuManager` path, install root or resolution source is on the wire. An
answered request appends no ledger record (Workflow #383 AO-4q): `source` is the state source
of `runtime-state-observation.md` (the ledger's committed head and the sampling interval; the
two store revisions are absent until AO-3), and the receipt is `completed` without a terminal,
like `Status`.

A refusal appends `command.rejected` (the receipt terminal, diagnostic
`backend_operation_failed` or `runtime_diagnostic`, effect `not_performed`) plus one
`runtime.failed` (stage `operation_cleanup`) whose `native_detail` keeps the provider's device
error; the primary detail `{category, stage, backend: execution_backend_provider, operation:
discover_instances}` is a controlled template declared Sensitive. Host operation
`discover_instances`:

| Provider refusal | Host code | Projection | Receipt |
| --- | --- | --- | --- |
| `instance_discovery_unavailable`, stage `instance_discovery_unsupported` (no discovery surface) | `instance_discovery_unavailable` | `invalid_request` | `denied` |
| `instance_discovery_unavailable`, any other stage (no install, tool spawn, exit, timeout, decode, JSON) | `instance_discovery_unavailable` | `backend_operation_failed` | `failed` |
| `mumu_manager_version_unsupported` | `mumu_manager_version_unsupported` | `backend_operation_failed` | `failed` |
| An answer the contract refuses (duplicate index, empty or over-long name) | `instance_discovery_unavailable` | `backend_operation_failed` | `failed` |

`RuntimeClient::discover_instances()` (and `RuntimeProjectClient::discover_instances()`) waits
25 s for the receipt: two vendor commands (`version`, `info -v all`) of at most 10 s each, plus
the IO margin.

`actingctl emulator discover --state-root <state-root>` (no `--instance`) prints the
`RuntimeInstanceDiscovery` above as one JSON line and exits 0; a refusal is the usual
`FATAL actingctl:` line with a non-zero exit.
