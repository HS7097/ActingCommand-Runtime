# AGENTS.md

## Repository scope

This repository is `HS7097/ActingCommand-Runtime`.

Runtime work should keep planning and checkpoint files inside this repository. Do not rely on the umbrella repository for routine Runtime task tracking.

Before making Runtime changes, read these files if they exist:

- `PLANS.md`
- `CHECKPOINT.md`
- `LICENSE_POLICY.md`
- `NOTICE.md`

## Runtime direction

- Rust is the main implementation line.
- Python runtime materials are legacy/mock only.
- Go runtime/contract materials are historical reference and benchmark material only.
- Runtime work should stay behind explicit API, schema, and primitive boundaries.

## Issue 35 architecture authority

- GitHub Issue #35 and its approved v3 specification supersede the Issue #33/#34 direction that treated Lab as the application core.
- Lab is an optional debug and sealed-test client. Production packages must not directly or transitively depend on `actingcommand-lab`.
- The long-lived Runtime host and scheduler own production state, leases, device authority, and task lifecycle. Clients submit typed requests and consume ledger projections.
- All state-changing input operations must ultimately pass through the Runtime-owned DeviceProxy with scheduler fencing. The C3a read-only capture exception does not grant input capability.
- The global ledger is the production fact source. Critical intent must be durable before action, outcome must be appended after action, and secret values must be redacted before ledger ingress.
- Keep Issue #33/#34 branches suspended. Do not resume their A8b/A8c/A9 migration targets or merge the paused RED branch under the old architecture.
- Read `docs/architecture/runtime-ledger-v3-c0-freeze.md` before Issue #35 implementation after its C0 hash is approved.

## Error handling

- Severe errors must never silently fail.
- Critical failures must not return empty objects, empty arrays, null, fake defaults, or silently skipped results.
- Transient issues may use bounded fallback only when the fallback path is fully logged.
- Fatal device, capture, recognition, storage, or adapter errors must surface explicitly.

## Planning and checkpoint updates

For each Runtime task:

- update this repository's `PLANS.md` when phase, scope, boundaries, or next steps change;
- update this repository's `CHECKPOINT.md` with changed files, commands run, validation, blockers, and next steps;
- commit `PLANS.md` and `CHECKPOINT.md` in the same Runtime commit or same Runtime task branch as the source changes;
- push the Runtime repository after the task is completed and verified unless the user explicitly says not to push.

Do not mirror planning files into `HS7097/ActingCommand` after routine Runtime tasks. Use the umbrella repository only for umbrella-level planning, cross-repository policy, or meta-documentation.

Do not merge, copy, or synchronize routine Runtime updates into the umbrella/main `HS7097/ActingCommand` repository by default. Keep Runtime changes in this repository until the user explicitly confirms a specific merge point.

## Resource repository freshness

For any ActingCommand Runtime task that reads or uses resource repository content, first update the relevant resource repositories from their remotes before executing the resource-dependent step.

- Use `git fetch origin` and `git pull --ff-only` for each relevant resource repository.
- Record the resource repository paths and commit hashes used in `CHECKPOINT.md`.
- If a resource repository is dirty, missing, unavailable, or cannot fast-forward, stop before the resource-dependent action and report the blocker unless the user explicitly gives a one-off override.
- This applies to current and future resource repositories, including AzurLane, Arknights, and BlueArchive resources.

## Current boundaries

- Do not add game logic unless the current plan explicitly requires it.
- Do not add UI code here.
- Do not add SQLite, OCR, recognition data loading, or scheduler behavior inside narrow primitive milestones unless that milestone explicitly requires it.
- Do not copy upstream source code or assets without license and attribution review.
- Keep comments sparse and useful; prefer clear names, small functions, and explicit state models.
