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
and fails the request. The device session is NOT reopened by this operation after any action: it
opens lazily on the next lease, as before.

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
  `startup_package_missing` when the locator does not open,
  `startup_package_admission_failed` for every other admission refusal (the underlying
  `contained_task_package_*` code attached as related failure, a resource declaration
  rejection carried along). Every failure of the run is recorded as
  `runtime.failed` (stage `operation_cleanup`, category `startup_package`) linked to the
  instance and the causation id; a fatal one poisons the host as any other.

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
