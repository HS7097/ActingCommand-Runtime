# Lab self-heal C0.a resource drift stop-loss

## Scope

Implemented `TASK-Lab-selfheal-chain.md` C0.a for ActingLab Operation Bundle execution.

This slice classifies a stable target mismatch on the expected page as `resource_drift`. It reuses the C0.c page guard, guard target evaluation, existing recognition evaluator, and C0.b ROI stability comparison tolerances.

## Behavior

- C0.c still evaluates the execution-time page guard first.
- If the page identity anchor passes but the guard target mismatches, ActingLab does not click immediately and does not open the touch backend.
- ActingLab captures follow-up frames and evaluates the same guard target in the expected rect.
- If the target becomes valid again, execution returns to the normal ROI stability gate.
- If the mismatching target is stable across the required mismatch samples, the operation fails loudly as `resource_drift`.
- `resource_drift` diagnostics include the target id, expected rect, measured target result, observed frame count, provenance version when available, full provenance, and a `needs_recalibration` resource status.
- Moving or otherwise unstable mismatches remain `unstable_page` rather than being misclassified as drift.
- Page changes during the drift probe remain `page_guard_mismatch`.
- `session self-heal-plan --trigger resource_drift` now exposes a stop-loss recovery plan with `restart_allowed=false`, `executes_control=false`, no heavy recovery candidate, and a recalibration blocker.

## Boundaries

- No UI, SQLite, scheduler behavior, OCR, resource repository read, game logic, live device operation, recovery graph execution, C2/C3 wiring, or resource rewrite was added.
- This slice does not implement FeatureMatch relocation, `record amend`, or automatic recalibration.
- The self-heal plan change is stop-loss metadata only; it does not execute recovery or restart the app.

## Validation

Targeted checks passed:

- `cargo test -p actingcommand-actinglab resource_drift`
- `cargo test -p actingcommand-actinglab pre_execution_guard`
- `cargo test -p actingcommand-actinglab roi_stability_gate`
- `cargo test -p actingcommand-actinglab`
- `cargo clippy -p actingcommand-actinglab -- -D warnings`

Public validation passed:

- `cargo fmt --all -- --check`
- `cargo build --release`
- `cargo clippy --workspace -- -D warnings`
- `cargo test --workspace`
- `git diff --check`
