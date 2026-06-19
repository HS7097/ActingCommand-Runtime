# CHECKPOINT.md

## Current status

Runtime repository-local planning has been initialized.

Future Runtime tasks should update and commit this repository's `PLANS.md` and `CHECKPOINT.md` together with Runtime source changes.

## Recent Runtime milestones

- P2.1.1 capture artifact path security close-out:
  - commit `edb69302b4bfe25d2c2a61004b1b94ead32965b4`
  - tag `checkpoint/20260618-p2-1-1-capture-store-security`
- P4a recognition primitive engine:
  - commit `5083b136022abe4907af3dfd653b399952038a65`
  - tag `checkpoint/20260618-p4a-recognition-primitives`
- P4a.1 recognition score semantics close-out:
  - adds `raw_score` plus normalized `score` to `TemplateMatch`
  - keeps P4a threshold-free
- P4b recognition pack rule layer:
  - adds data-driven recognition pack parsing, validation, thresholding, and target evaluation

## 2026-06-18 Runtime repo-local planning initialization

### Current status

- Added Runtime-local `AGENTS.md`, `PLANS.md`, and `CHECKPOINT.md`.
- Supersedes the previous routine behavior of mirroring Runtime task planning files into the umbrella repository.
- Runtime future task close-out should commit and push planning/checkpoint updates in this repository.
- Runtime-local planning initialization was pushed to `HS7097/ActingCommand-Runtime`.

### Files changed

- `CHECKPOINT.md`
- `AGENTS.md`
- `PLANS.md`

### Commands run

- Checked Runtime repository status.
- Created Runtime-local planning files.
- Committed and pushed Runtime-local planning files.

### Test results

- Documentation/policy-only change; no code tests required.

### Current blocker

- None.

### Next step

1. Use Runtime-local `PLANS.md` and `CHECKPOINT.md` for the next Runtime task.

## 2026-06-18 Runtime-to-main merge policy

### Current status

- Clarified Runtime-to-main repository merge policy by user instruction.
- Routine Runtime updates stay in `HS7097/ActingCommand-Runtime`.
- Do not merge, copy, or synchronize Runtime changes into the umbrella/main `HS7097/ActingCommand` repository by default.
- Merge a Runtime state into the main repository only after the user explicitly confirms that merge point.
- Runtime-local policy update is recorded in commit `7e587f956067ab21384a11b784df60a8eab788fd`.

### Files changed

- `AGENTS.md`
- `PLANS.md`
- `CHECKPOINT.md`

### Commands run

- Updated Runtime-local policy files.
- `git commit -m "docs: clarify Runtime-to-main merge policy"`
- Amended the checkpoint with the final Runtime commit hash before pushing.

### Test results

- Documentation/policy-only change; no code tests required.

### Current blocker

- None.

### Next step

1. Use this merge policy for future Runtime work.

## 2026-06-18 P4a.1 recognition score semantics close-out

### Current status

- Completed P4a.1 recognition score semantics close-out.
- `TemplateMatch` now includes both `raw_score` and normalized `score`.
- `raw_score` is the method-native score from `imageproc` `CrossCorrelationNormalized`.
- `score` is normalized to `0.0..=1.0` for future rule-layer thresholding and is not a probability.
- `normalize_ncc_score` uses identity plus clamp for current NCC semantics and maps `NaN` to `0.0`.
- P4a remains threshold-free; P4b or callers own threshold selection and recognition data loading.
- No UI, SQLite, OCR, page navigation, game logic, fallback, reconnect, retry, OpenCV, or new dependency was added.
- No `crates/device` or `crates/runtime-core` source was modified.

### Files changed

- `crates/recognition/src/lib.rs`
- `PLANS.md`
- `CHECKPOINT.md`

### Commands run

- Read task file: `C:\合作工作区\ActingCommand\TASK-P4a.1-score-semantics.md`
- Read Runtime-local `AGENTS.md`, `PLANS.md`, and `CHECKPOINT.md`.
- Checked Runtime repository status and baseline commit.
- Inspected `crates/recognition/src/lib.rs` and `crates/recognition/Cargo.toml`.
- `cargo fmt --all`
- `cargo test -p actingcommand-recognition`
- `cargo test --workspace`
- `cargo fmt --all -- --check`
- `cargo clippy -p actingcommand-recognition -- -D warnings`
- `rg -n "OCR|ocr|SQLite|sqlite|\bUI\b|\bui\b|navigation|navigate|state machine|game logic|fallback|reconnect|retry|opencv|threshold\s*=|threshold\(" crates\recognition crates\recognition\Cargo.toml`
- `rg -n "raw_score|normalize_ncc_score|CrossCorrelationNormalized|TemplateMatch" crates\recognition\src\lib.rs`
- `git diff --check`

### Test results

- `cargo test -p actingcommand-recognition` passed.
- `cargo test --workspace` passed.
- `cargo fmt --all -- --check` passed.
- `cargo clippy -p actingcommand-recognition -- -D warnings` passed.
- `git diff --check` passed.
- Recognition tests now cover normalized NCC identity/clamp semantics, `NaN -> 0.0`, `raw_score` on perfect matches, and normalized `score` range.
- Prohibited-feature scan found no OCR, UI, SQLite, page navigation, game logic, fallback, reconnect, retry, OpenCV, or threshold implementation in `crates/recognition`.

### Current blocker

- None.

### Next step

1. Define P4b recognition data loading and threshold policy outside the P4a primitive engine.

## 2026-06-19 P4b recognition pack rule layer

### Current status

- Completed P4b recognition pack rule and threshold layer.
- Added `actingcommand-recognition-pack` as the P4b rule and threshold layer above the threshold-free P4a recognition primitive crate.
- P4b keeps OCR, UI, SQLite, navigation, state machines, game logic, click execution, and capture persistence out of scope.
- `crates/recognition` remains threshold-free.
- `recognition::Rect` remains serde-free; P4b uses `PackRect` and converts at the pack boundary.

### Files changed

- `Cargo.toml`
- `Cargo.lock`
- `crates/recognition-pack/Cargo.toml`
- `crates/recognition-pack/src/lib.rs`
- `PLANS.md`
- `CHECKPOINT.md`

### Commands run

- Read task file: `C:\合作工作区\ActingCommand\TASK-P4b-recognition-pack.md`
- Read Runtime-local `AGENTS.md`, `PLANS.md`, and `CHECKPOINT.md`.
- Checked Runtime repository status and baseline commit.
- Inspected `crates/recognition/src/lib.rs` and `crates/recognition/Cargo.toml`.
- `cargo fmt --all`
- `cargo test -p actingcommand-recognition-pack`
- `cargo test --workspace`
- `cargo fmt --all -- --check`
- `cargo clippy -p actingcommand-recognition-pack -- -D warnings`
- `cargo tree -p actingcommand-recognition-pack --depth 1`
- `rg -n "opencv|rusqlite|sqlite|SQLite|OCR|ocr|\bUI\b|\bui\b|navigation|navigate|state machine|game logic|fallback|reconnect|retry|adb|input tap|tap\(" crates\recognition-pack`
- `rg -n "image\s*=|imageproc\s*=|opencv|rusqlite|sqlite" crates\recognition-pack\Cargo.toml`
- `git diff --check`

### Test results

- `cargo test -p actingcommand-recognition-pack` passed with 24 tests.
- `cargo test --workspace` passed.
- `cargo fmt --all -- --check` passed.
- `cargo clippy -p actingcommand-recognition-pack -- -D warnings` passed.
- `cargo tree -p actingcommand-recognition-pack --depth 1` showed direct dependencies only on `actingcommand-recognition`, `serde`, and `serde_json`.
- Prohibited-feature scans found no OCR, UI, SQLite, navigation, state machine, game logic, fallback, reconnect, retry, ADB, or top-level `image`/`imageproc`/OpenCV dependency in `crates/recognition-pack`.
- `git diff --check` passed.

### Current blocker

- None.

### Next step

1. Commit and push Runtime repository changes.
2. Define the next recognition/runtime integration milestone separately.
