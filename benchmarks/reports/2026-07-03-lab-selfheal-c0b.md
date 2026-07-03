# Lab self-heal C0.b ROI stability gate

## Scope

Implemented `TASK-Lab-selfheal-chain.md` C0.b for ActingLab Operation Bundle execution.

This slice adds a ROI-level stability gate before fixed-coordinate actions are dispatched. It reuses the C0.c guard target and the existing recognition evaluator; it does not add a new recognition backend or whole-frame stability detector.

## Behavior

- C0.c still captures the execution-time guard frame first.
- When the guard passes, the guard target evaluation becomes the baseline ROI sample.
- Before MaaTouch input is opened or a click is sent, ActingLab captures follow-up frames and evaluates the same guard target.
- The action is allowed only after the target ROI is stable for the configured internal default of two consecutive samples.
- Template targets compare match position and normalized score within small tolerances.
- Color targets compare mean RGB and distance within small tolerances.
- A static ROI passes on the first follow-up frame.
- A changing ROI waits until it stabilizes.
- A continuously changing ROI fails loudly as `unstable_page`.
- If the page changes while waiting for stability, the action is refused as `page_guard_mismatch`.

## Boundaries

- No UI, SQLite, scheduler behavior, OCR, resource repository read, game logic, live device operation, recovery graph execution, or C1/C2/C3 trigger routing was added.
- Unguarded trusted coordinates keep the existing explicit escape hatch and do not invent fake ROI data.
- The gate is local to Operation Bundle coordinate execution.

## Validation

Targeted checks passed:

- `cargo test -p actingcommand-actinglab roi_stability_gate`
- `cargo test -p actingcommand-actinglab pre_execution_guard`
- `cargo test -p actingcommand-actinglab`
- `cargo clippy -p actingcommand-actinglab -- -D warnings`

Public validation passed:

- `cargo fmt --all -- --check`
- `cargo build --release`
- `cargo clippy --workspace -- -D warnings`
- `cargo test --workspace`
- `git diff --check`
