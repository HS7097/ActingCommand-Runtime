# Lab Self-Heal C3 Login/Wake Resource Wiring

## Scope

Implemented the sixth slice of `C:\合作工作区\ActingCommand\TASK-Lab-selfheal-chain.md`: C3 automatic login and wake resource wiring for monitor recovery resources.

This increment keeps recovery actions behind the Session Layer signal boundary. It does not add direct device execution, UI, OCR, SQLite, scheduler behavior, game logic, or live emulator recovery.

## Resource freshness

Resource repositories were refreshed before reading recovery/login resources.

| Resource repository | Commit |
| --- | --- |
| `C:\Users\Alice\Documents\Azur\ActingCommand-Resources-Arknights` | `2ab7ccddd63054ee16d3441ff71683a3feae1a6a` |
| `C:\Users\Alice\Documents\Azur\ActingCommand-Resources-AzurLane` | `e778cc7c8576c57bfc8f4df72b0c86efb5f65fb4` |
| `C:\Users\Alice\Documents\Azur\ActingCommand-Resources-BlueArchive` | `7cf3bae27d473d29efde1f5605f106b28a2df9fb` |

## Behavior

- `monitor --recover` resource execution now preserves recovery action metadata from `ours/recovery/<game>.<server>.recovery.json`.
- `run_recovery_flow` actions resolve their named `recovery_flows` entry and fail loudly if the flow is missing.
- `tap_control_point` actions preserve `ref`, `args`, and resolved control-point coordinates in the recovery output.
- `session_expired` recovery prioritizes `run_recovery_flow: startup_login` so the login loop is the first Session Layer signal action.
- Arknights monitor recovery loads `STARTUP-LOGIN.md` for the startup-login loop and fails loudly if the file is missing.
- AzurLane and BlueArchive recovery resources can use their embedded `recovery_flows.startup_login` without requiring an external `STARTUP-LOGIN.md`.
- BlueArchive standby wake preserves the wake control point, including the `(300, 2)` wake/dead-zone resource.
- All generated recovery actions continue to report `via=session_layer`, `direct_device_allowed=false`, and `executed_directly=false`.

## Validation

Focused validation:

- `cargo test -p actingcommand-actinglab monitor_resource_session_expired`
- `cargo test -p actingcommand-actinglab monitor_resource_standby`
- `cargo clippy -p actingcommand-actinglab -- -D warnings`

Public validation is recorded in `CHECKPOINT.md`.

## Boundaries

- No resource repository files were modified.
- No external binaries or upstream source were added.
- No MaaTouch session is opened by these dry-run/unit validations.
- Live standby/session-expired smoke remains part of the unified live batch and is not required for offline C3 acceptance.
