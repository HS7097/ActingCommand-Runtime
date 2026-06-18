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

## Current boundaries

- Do not add game logic unless the current plan explicitly requires it.
- Do not add UI code here.
- Do not add SQLite, OCR, recognition data loading, or scheduler behavior inside narrow primitive milestones unless that milestone explicitly requires it.
- Do not copy upstream source code or assets without license and attribution review.
- Keep comments sparse and useful; prefer clear names, small functions, and explicit state models.
