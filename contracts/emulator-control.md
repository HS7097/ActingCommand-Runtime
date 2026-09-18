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
  there is no automatic restart policy.
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
emulator changes state.

## Close before stop

For `stop` and `restart` the instance's retained device session is closed first through
`close_retained_instance_while_guarded` (the daemon keeps running; other instances are
untouched). A refusal because a lease is held is the same busy denial; a close failure is
recorded through the existing session-close lifecycle path and fails the request. The device
session is NOT reopened by this operation after any action: it opens lazily on the next lease,
as before.

## Tool dispatch, timeouts and wait criteria

`actingcommand-device::mumu_manager::control_instance(manager_path, index, action, wait)`:

- Dispatches `MuMuManager.exe control -v <index> launch | shutdown | restart` (no console
  window), bounded by `MUMU_MANAGER_CONTROL_TIMEOUT` = 60 s. The vendor documents only the
  syntax: no return value, exit code, JSON or blocking behaviour. A non-zero exit
  (`mumu_manager.control_exit`) or an `{"errcode","errmsg"}` envelope with exit 0
  (`mumu_manager.control_errcode`) is a typed failure carrying the exit code and a bounded
  (<= 1 KiB, control characters other than `\n` `\r` `\t` stripped) stdout+stderr summary.
- Then polls `info -v <index>` every 1 s, each poll bounded by `MUMU_MANAGER_COMMAND_TIMEOUT`
  = 10 s, through the lax reader `read_instance_state` (`InstanceState`): the flat and the map
  shape are both accepted, `adb_port` and `player_state` may be absent (a stopped instance
  has neither), the booleans and a consistent `index` are required, `launch_err_code` /
  `launch_err_msg` default to `0` / empty when absent.
- Success: `start` / `restart` when `is_process_started && is_android_started && adb_port != 0`;
  `stop` when `!is_process_started`. Default waits `MUMU_MANAGER_STATE_WAIT_START` = 120 s and
  `MUMU_MANAGER_STATE_WAIT_STOP` = 60 s, counted from the moment `control` returned.
- Failure: `launch_err_code != 0` (`mumu_manager.launch_error`, carrying `launch_err_msg` in the
  summary), an `info` refusal (its existing typed stages) or the deadline
  (`mumu_manager.wait_timeout`, carrying the last observed state).
- `player_state` is undocumented: it is recorded opaquely and never branched on.
- Caveats from the undocumented behaviour: if `control launch` blocks until boot, the 60 s tool
  bound may fire before the readiness wait even starts; if `control restart` reports the old
  running state before the shutdown phase, the wait may satisfy the running criterion early.
  Both are vendor facts the Runtime does not assume either way; the bounds are Runtime policy.
- The hidden `api` subcommand is never dispatched.

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
2. For `stop` / `restart`, the existing resource-close events of the device session
   (`runtime.lifecycle_observed` with `resource_quiescence`, or the session-close failure).
3. On success `command.validated` with effect `performed` (the receipt terminal), then
   `runtime.fact_recorded` for `device.connected` (and after `stop` a
   `runtime.fact_invalidated` with reason `device_closed`).
4. On denial or failure `command.rejected` (the receipt terminal; diagnostic
   `backend.operation_failed` for tool failures, `lease.fencing_denied` for the busy denial,
   `runtime.diagnostic` otherwise; effect `not_performed` before the tool ran, `indeterminate`
   once it ran) plus one `runtime.failed` (stage `operation_cleanup`) whose record carries
   `raw_os_error` = the tool exit code, `native_detail` = the bounded vendor output
   (Sensitive) and a primary detail `{category, stage, backend: mumu_manager, operation:
   control_instance}` declared Sensitive, where `stage` is the typed step
   (`mumu_manager.control_exit`, `.control_errcode`, `.launch_error`, `.wait_timeout`, `.run`,
   `.path`, an `info` reader stage, `emulator_control.unavailable`, `.unsupported`).

Host codes: `emulator_control_busy` (`runtime_busy`, denied), `emulator_control_unavailable`
and `emulator_control_unsupported` (`invalid_request`, denied), `emulator_control_wait_timeout`
and `emulator_control_failed` (`backend_operation_failed`, failed).

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
  "elapsed_ms": 12345
}
```

`adb_port` is omitted when the last observation carried none (a stopped instance). The receipt
state is `completed` with the `command.validated` event as terminal.

## Client and CLI

`RuntimeClient::control_emulator_instance(instance_alias, action)` sends the operation with an
explicit receipt wait of 200 s (60 s tool bound + 120 s readiness wait + one 10 s poll + IO
margin) and returns the `EmulatorInstanceControlled` result verbatim.

```
actingctl emulator status  --state-root <state-root> --instance <alias>
actingctl emulator start   --state-root <state-root> --instance <alias>
actingctl emulator stop    --state-root <state-root> --instance <alias>
actingctl emulator restart --state-root <state-root> --instance <alias>
```

`status` is the existing `Status` read filtered to the alias (that instance's
`RuntimeInstanceStatus`, or `instance_unknown`); `start` / `stop` / `restart` print the result
JSON above. All four require `--instance`.
