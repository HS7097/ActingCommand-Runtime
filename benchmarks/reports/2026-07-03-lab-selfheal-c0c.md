# 2026-07-03 Lab Self-Heal C0.c Guarded Action Report

## Scope

Implemented the C0.c guarded coordinate action slice from `TASK-Lab-selfheal-chain.md`.

This slice adds execution-time guards for fixed-coordinate Operation Bundle actions before any touch backend is opened or any MaaTouch input is sent.

## Behavior

- Operation Bundle coordinate actions now require explicit `guard` metadata by default.
- Missing guard metadata fails package validation loudly.
- A reviewed escape hatch is available through `unguarded_trusted_coordinate: true`.
- Guard metadata requires:
  - `page_id`
  - `target_id`
  - `expected_rect`
  - `verify_template` or `color_probe`
- At execution time, ActingLab captures a fresh frame before input, confirms the current page still matches the guarded page, and evaluates the guarded target.
- Page mismatch returns a visible `page_guard_mismatch` failure before clicking.
- Target mismatch returns a visible `target_guard_mismatch` failure before clicking.
- Trusted unguarded coordinates are recorded in step output and journal events.

## Package generation

- `package build-task` fixture packages were updated to include guard metadata.
- `session record build-task` now emits guard metadata from the operation source page anchor and click point.

## Boundaries

- No C0.b ROI stability gate was added in this slice.
- No C0.a resource drift trigger was added in this slice.
- No C1 trigger routing changes were added in this slice.
- No C2 recovery loop wiring was added in this slice.
- No C3 login/wake execution was added in this slice.
- No OCR, UI, SQLite, game logic, or new device backend was added.

## Validation

Targeted validation before public gates:

- `cargo test -p actingcommand-actinglab pre_execution_guard`: passed.
- `cargo test -p actingcommand-actinglab session_record_build_task_writes_draft_bundle`: passed.
- `cargo test -p actingcommand-actinglab`: passed.

