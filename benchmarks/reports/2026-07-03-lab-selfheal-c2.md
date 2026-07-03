# Lab Self-Heal C2/H1 Recovery Loop Report

Date: 2026-07-03

Task: `C:\合作工作区\ActingCommand\TASK-Lab-selfheal-chain.md`

## Scope

Implemented the C2 live recovery loop wiring slice and the required H1 loop detection fix.

## Resource freshness

Before wiring resource recovery behavior, the local resource repositories were refreshed with `git fetch origin --prune --tags` and `git pull --ff-only`.

| Repository | Local path | Commit |
| --- | --- | --- |
| ActingCommand-Resources-Arknights | `C:\Users\Alice\Documents\Azur\ActingCommand-Resources-Arknights` | `2ab7ccddd63054ee16d3441ff71683a3feae1a6a` |
| ActingCommand-Resources-AzurLane | `C:\Users\Alice\Documents\Azur\ActingCommand-Resources-AzurLane` | `e778cc7c8576c57bfc8f4df72b0c86efb5f65fb4` |
| ActingCommand-Resources-BlueArchive | `C:\Users\Alice\Documents\Azur\ActingCommand-Resources-BlueArchive` | `7cf3bae27d473d29efde1f5605f106b28a2df9fb` |

## Implemented behavior

- `monitor --recover --use-recovery-resource` resolves recovery resources from `ours/recovery/<game>.<server>.recovery.json` under the selected resource root.
- `monitor --recover --recovery-resource <path>` uses an explicit recovery resource path and fails loudly if it is missing or invalid.
- Monitor recovery selects the resource rule by the C1 canonical self-heal trigger.
- Recovery actions are converted into `recovery_exec` signal actions and executed only through the Session Layer recovery runtime.
- Restart-class recovery actions are skipped and recorded instead of executed.
- Resource drift and unstable page triggers remain non-recoverable stop/action gates and do not enter the recovery graph.
- Daemon monitor policy recovery can run the same resource recovery graph when the held lease matches.
- Recovery output records journal metadata, selected rule id, graph status, attempted actions, skipped restart actions, and the no-direct-device boundary.
- H1 is fixed: recovery graph loop detection is checked before max-attempt exhaustion can mask a repeated node.

## Boundaries

- No game progress action was added.
- No OCR, UI, SQLite, scheduler rewrite, resource rewrite, or FeatureMatch work was added.
- No recovery action bypasses the Session Layer throat.
- No destructive or restart action is executed by this slice.
- Existing non-resource monitor recovery remains available when no resource recovery flag is supplied.

## Verification

Focused checks passed:

- `cargo test -p actingcommand-actinglab recovery_reports_loop_before_max_attempts`
- `cargo test -p actingcommand-actinglab monitor_loop_resolves_recovery_resource_from_resource_root`
- `cargo test -p actingcommand-actinglab monitor_loop_recover_uses_recovery_resource_graph`
- `cargo test -p actingcommand-actinglab monitor_loop_recovery_resource_missing_is_fatal_when_explicit`
- `cargo test -p actingcommand-actinglab daemon_monitor_policy_recovery_runs_resource_graph_with_matching_lease`
- `cargo test -p actingcommand-actinglab`
- `cargo clippy -p actingcommand-actinglab -- -D warnings`

Full public validation is recorded in `CHECKPOINT.md`.

## Remaining work

C3 automatic login/wake resource wiring remains pending.
