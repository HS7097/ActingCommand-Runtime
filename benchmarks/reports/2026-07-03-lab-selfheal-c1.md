# Lab Self-Heal C1 Trigger Classification And Priority Routing

Date: 2026-07-03

## Scope

This report records the C1 slice from `C:\合作工作区\ActingCommand\TASK-Lab-selfheal-chain.md`.

Implemented scope:

- `session self-heal-policy` reports the canonical C1 trigger set.
- `session self-heal-plan` normalizes legacy trigger aliases to canonical triggers.
- `session self-heal-plan` accepts repeated `--trigger` values and comma-separated `--triggers` values, then chooses the deterministic highest-priority trigger.
- Trigger priority is:
  1. `stale_frame` / `hang`
  2. `resource_drift`
  3. `session_expired` / `standby`
  4. `modal_popup`
  5. `off_route_page`
  6. `unstable_page`
- `monitor --once` reports canonical trigger metadata for current monitor diagnoses.

## Trigger routes

| Canonical trigger | Recovery strategy |
| --- | --- |
| `stale_frame` | `capture_backend_recovery` |
| `hang` | `capture_backend_recovery` |
| `resource_drift` | `resource_drift_stop_loss` |
| `session_expired` | `startup_login_loop` |
| `standby` | `standby_wake` |
| `modal_popup` | `modal_dismissal` |
| `off_route_page` | `maintenance_navigation` |
| `unstable_page` | `action_gate_failure` |

Legacy aliases are preserved for compatibility:

- `capture_stale_suspected` -> `stale_frame`
- `capture_backend_unavailable` -> `stale_frame`
- `startup_login_required` -> `session_expired`
- `unexpected_page` -> `off_route_page`

## Validation

Commands passed locally:

- `cargo test -p actingcommand-actinglab self_heal_trigger`
- `cargo test -p actingcommand-actinglab session_self_heal_plan`
- `cargo test -p actingcommand-actinglab monitor_diagnosis`
- `cargo clippy -p actingcommand-actinglab -- -D warnings`
- `cargo test -p actingcommand-actinglab`
- `cargo fmt --all -- --check`
- `cargo build --release`
- `cargo clippy --workspace -- -D warnings`
- `cargo test --workspace`
- `git diff --check`

`%LOCALAPPDATA%\ActingCommand\actinglab\config.json` was absent before the public validation commands.

## Boundaries

This slice does not implement C2 live recovery loop wiring, H1 recovery loop changes, C3 login/wake resource execution, resource repository reads, OCR, UI, SQLite, scheduler behavior, device live operation, or game logic.
