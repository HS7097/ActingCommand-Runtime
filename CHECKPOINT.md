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

## 2026-06-19 P4c recognition pack real-data bridge

### Current status

- Completed Runtime P4c-1 from-disk recognition-pack integration test.
- Completed Runtime P4c-3 read-only `device-test recognize` entry.
- Cloned and used `HS7097/ActingCommand-Resources-AzurLane` locally for P4c resource-pack validation.
- Completed resource-side P4c-2b jp pack generation from neutral data with cropped patches.
- Performed P4c-4 observational calibration against emulator port `16384`.

### Files changed

Runtime repository:

- `Cargo.toml`
- `Cargo.lock`
- `apps/device-test/Cargo.toml`
- `apps/device-test/src/main.rs`
- `crates/recognition-pack/tests/from_disk.rs`
- `PLANS.md`
- `CHECKPOINT.md`

Resource repository:

- `README.md`
- `manifest.yaml`
- `tools/generate_azurlane_pack.py`
- `recognition/azurlane.jp.pack.json`
- `recognition/patches/azurlane/jp/**`

### Commands run

- Read task file: `C:\合作工作区\ActingCommand\TASK-P4c-recognition-pack-realdata.md`
- Read Runtime-local `AGENTS.md`, `PLANS.md`, and `CHECKPOINT.md`.
- Checked Runtime repository status.
- Cloned `HS7097/ActingCommand-Resources-AzurLane` to `C:\Users\Alice\Documents\Azur\ActingCommand-Resources-AzurLane`.
- `python tools\generate_azurlane_pack.py --server jp --clean`
- `cargo run -p actingcommand-device-test -- recognize --pack C:\Users\Alice\Documents\Azur\ActingCommand-Resources-AzurLane\recognition\azurlane.jp.pack.json --pack-root C:\Users\Alice\Documents\Azur\ActingCommand-Resources-AzurLane --check-pack`
- Resource repository path validation: 2005 targets, 0 missing template paths, 0 unsafe template paths.
- `cargo run -p actingcommand-device-test -- --port 16384 capture --out C:\Users\Alice\Documents\Azur\p4c-main.png`
- `cargo run -p actingcommand-device-test -- recognize --pack C:\Users\Alice\Documents\Azur\ActingCommand-Resources-AzurLane\recognition\azurlane.jp.pack.json --pack-root C:\Users\Alice\Documents\Azur\ActingCommand-Resources-AzurLane --target ui_white/MAIN_GOTO_CAMPAIGN_WHITE --scene C:\Users\Alice\Documents\Azur\p4c-main.png`
- `cargo run -p actingcommand-device-test -- --port 16384 recognize --pack C:\Users\Alice\Documents\Azur\ActingCommand-Resources-AzurLane\recognition\azurlane.jp.pack.json --pack-root C:\Users\Alice\Documents\Azur\ActingCommand-Resources-AzurLane --target ui_white/MAIN_GOTO_CAMPAIGN_WHITE --capture`
- `cargo test -p actingcommand-recognition-pack`
- `cargo test -p actingcommand-device-test`
- `cargo test --workspace`
- `cargo fmt --all -- --check`
- `cargo clippy -p actingcommand-recognition-pack -p actingcommand-device-test -- -D warnings`
- `cargo tree -p actingcommand-device-test --depth 1`
- Prohibited-feature scans over `apps/device-test` and `crates/recognition-pack`.
- `git diff --check`
- Resource repository `git diff --check`

### Test results

- `cargo test -p actingcommand-recognition-pack` passed, including the new from-disk integration test.
- `cargo test -p actingcommand-device-test` passed with 12 tests.
- `cargo test --workspace` passed.
- `cargo fmt --all -- --check` passed.
- `cargo clippy -p actingcommand-recognition-pack -p actingcommand-device-test -- -D warnings` passed.
- `device-test recognize --check-pack` accepted the resource repo jp pack.
- Resource jp pack has 2005 targets and 2005 generated patch PNG files under `recognition/patches/azurlane/jp`.
- Resource pack path validation found 0 missing template paths and 0 unsafe paths.
- Resource repository `git diff --check` passed after forcing generated pack JSON to LF line endings.
- `cargo tree -p actingcommand-device-test --depth 1` showed direct dependencies only on `actingcommand-device`, `actingcommand-recognition`, and `actingcommand-recognition-pack`.
- No direct `image`, `imageproc`, OpenCV, SQLite, OCR, UI, PageDetector, game logic, fallback, reconnect, or retry dependency was added to `device-test` or `recognition-pack`.
- Existing MaaTouch input commands remain in the existing input branch; `recognize` returns before that branch and does not start MaaTouch.

### P4c-4 calibration notes

- Port: `16384`.
- Capture command produced `C:\Users\Alice\Documents\Azur\p4c-main.png`.
- Scene size: `1280x720`.
- Target: `ui_white/MAIN_GOTO_CAMPAIGN_WHITE`.
- Pack: `C:\Users\Alice\Documents\Azur\ActingCommand-Resources-AzurLane\recognition\azurlane.jp.pack.json`.
- Offline scene result:
  - `passed=false`
  - `raw_score=0.853815`
  - `score=0.853815`
  - `threshold=0.900000`
  - `click=1123,438,137,142`
- Live capture result after full jp pack generation:
  - `passed=false`
  - `raw_score=0.861700`
  - `score=0.861700`
  - `threshold=0.900000`
  - `click=1123,438,137,142`
- Visual inspection of `p4c-main.png` showed the game on a secretary/home screen where the target white campaign button was not visible.
- Threshold conclusion for this observation: keep `0.90`; this is a non-hit observation below threshold, not evidence to lower the threshold.
- P5 prerequisite note: re-run calibration on `page_main_white` where `MAIN_GOTO_CAMPAIGN_WHITE` is visible before changing region/template/threshold.

### Current blocker

- No blocker for Runtime P4c-1/P4c-3 automation.
- Green hit calibration still needs the game placed on `page_main_white` with `MAIN_GOTO_CAMPAIGN_WHITE` visible.

### Next step

1. Commit and push Runtime repository changes.
2. Commit and push resource repository pack/converter changes.
3. Start P5 PageDetector only after a separate task is confirmed.

## 2026-06-19 P4c-fixup and P5 PageDetector

### Current status

- Completed P4c-fixup in Runtime.
- Added color diagnostics to read-only `device-test recognize` output while preserving one `key=value` line per output row.
- Added `RecognitionEvaluator::target_kind` and `TargetKind` support for eager PageDetector validation.
- Completed P5 `actingcommand-page-detector` as a new Rust workspace crate.
- P5 only evaluates existing `Scene` values through `RecognitionEvaluator`; it does not start ADB, MaaTouch, Screencap, SQLite, UI, OCR, click execution, navigation, or game task logic.
- Runtime documentation was updated in this repository only.

### Files changed

- `Cargo.toml`
- `Cargo.lock`
- `apps/device-test/src/main.rs`
- `crates/recognition-pack/src/lib.rs`
- `crates/page-detector/Cargo.toml`
- `crates/page-detector/src/lib.rs`
- `PLANS.md`
- `CHECKPOINT.md`

### Commands run

- Read task file: `C:\合作工作区\ActingCommand\TASK-P4c-fixup-calibration-and-P5.md`
- Read Runtime-local `PLANS.md`, `CHECKPOINT.md`, and `NOTICE.md`.
- Checked Runtime repository status.
- `cargo fmt --all`
- `cargo test -p actingcommand-page-detector`
- `cargo test -p actingcommand-device-test`
- `cargo test -p actingcommand-recognition-pack`
- `cargo test --workspace`
- `cargo fmt --all -- --check`
- `cargo clippy -p actingcommand-page-detector -p actingcommand-device-test -p actingcommand-recognition-pack -- -D warnings`
- `cargo tree -p actingcommand-page-detector --depth 1`
- `rg -n "SQLite|sqlite|OCR|ocr|state machine|game logic|opencv|rusqlite|fallback|reconnect|retry|MaaTouch|Screencap|CaptureBackend|Device|tap\(|swipe\(|long_tap\(|reset\(" crates\page-detector`
- `rg -n "image\s*=|imageproc\s*=|opencv|rusqlite|sqlite|actingcommand-device|actingcommand-runtime-core" crates\page-detector\Cargo.toml`
- `git diff --check`
- `cargo run -p actingcommand-device-test -- --port 16384 recognize --pack C:\Users\Alice\Documents\Azur\ActingCommand-Resources-AzurLane\recognition\azurlane.jp.pack.json --pack-root C:\Users\Alice\Documents\Azur\ActingCommand-Resources-AzurLane --target ui_white/MAIN_GOTO_CAMPAIGN_WHITE --capture`
- `cargo run -p actingcommand-device-test -- --port 16384 capture --out C:\Users\Alice\AppData\Local\Temp\actingcommand-calibration-16384.png`

### Test results

- `cargo test -p actingcommand-page-detector` passed with 22 tests.
- `cargo test -p actingcommand-device-test` passed with 18 tests.
- `cargo test -p actingcommand-recognition-pack` passed with 24 unit tests, 1 integration test, and doc tests.
- `cargo test --workspace` passed.
- `cargo fmt --all -- --check` passed.
- `cargo clippy -p actingcommand-page-detector -p actingcommand-device-test -p actingcommand-recognition-pack -- -D warnings` passed.
- `git diff --check` passed.
- `cargo tree -p actingcommand-page-detector --depth 1` showed direct dependencies only on `actingcommand-recognition`, `actingcommand-recognition-pack`, `serde`, and `serde_json`.
- Prohibited-feature scans found no OCR, UI, SQLite, game logic, fallback, reconnect, retry, MaaTouch, Screencap, CaptureBackend, device dependency, click execution, `image`, `imageproc`, OpenCV, or runtime-core dependency in `crates/page-detector`.

### P4c-fixup details

- Template recognize output now includes `message`.
- Template recognize output with `color_check` now includes `color_distance`, `color_max_distance`, `color_mean`, and `color_expected`.
- Color recognize output now includes `message`, `color_mean`, and `color_expected`.
- ClickOnly recognize no longer requires `--scene` or `--capture` and still returns only click metadata plus `evaluated=false`.
- Template and Color recognize still fatal when neither `--scene` nor `--capture` is provided.
- `--scene` and `--capture` remain mutually exclusive.
- `recognize` remains read-only and returns before MaaTouch input command handling.

### P5 PageDetector details

- Added `actingcommand-page-detector` workspace crate.
- Added `PageSet`, `PageDefinition`, `PageDetector`, `PageEvaluation`, `PageTargetEvaluation`, and `PageTargetRole`.
- Added `load_page_set_from_json_str`.
- Added structural validation for schema version, empty/duplicate page ids, empty required lists, duplicate target ids, and cross-role target conflicts.
- Added eager evaluator validation with `RecognitionEvaluator::target_kind`.
- ClickOnly targets are fatal when used as required, optional, or forbidden page evidence.
- Matching rule is `all required passed && no forbidden passed`; optional evidence is diagnostic only.

### Calibration notes

- Port: `16384`.
- Temporary capture path: `C:\Users\Alice\AppData\Local\Temp\actingcommand-calibration-16384.png`.
- Scene size: `1280x720`.
- Visual inspection confirmed the game was on the main page with the white/right-side Campaign button visible.
- Target: `ui_white/MAIN_GOTO_CAMPAIGN_WHITE`.
- Pack: `C:\Users\Alice\Documents\Azur\ActingCommand-Resources-AzurLane\recognition\azurlane.jp.pack.json`.
- Repeated live capture results:
  - `passed=false`
  - `raw_score=0.997019`, then `0.996998`, then `0.996995`
  - `score=0.997019`, then `0.996998`, then `0.996995`
  - `threshold=0.900000`
  - `message=color check failed`
  - `color_distance=22.158520`
  - `color_max_distance=20.000000`
  - `color_mean=155,172,186`
  - `color_expected=156,165,165`
- Calibration conclusion: template evidence is strong, but the current color gate fails by about `2.16`. Do not lower the template threshold. Revisit the target color region, expected color, or color distance policy before treating this real AzurLane target as green.

### Current blocker

- No blocker for Runtime P4c-fixup or synthetic P5 PageDetector.
- Real AzurLane P5b page samples should wait for a follow-up calibration decision on `ui_white/MAIN_GOTO_CAMPAIGN_WHITE` color diagnostics.

### Next step

1. Commit and push Runtime repository changes.
2. Decide whether to continue with P5b real AzurLane page samples or P6 minimal task-loop draft.
3. Before real AzurLane page definitions become authoritative, review the Campaign button color check using the recorded `color_mean` and `color_expected`.

## 2026-06-19 P5c detect-page and P6a dry-run task loop

### Current status

- Completed P5c `device-test detect-page`.
- Completed read-only PageSet validation for AzurLane, Arknights, and BlueArchive resource repositories.
- Completed P6a `actingcommand-task-loop` dry-run task loop.
- Completed `device-test task-dry-run`.
- No UI, SQLite, OCR, real click execution, scheduler, background loop, page navigation, game task logic, ADB input fallback, or OpenCV was added.
- MaaTouch was not started by detect-page, task-dry-run, tests, or resource PageSet validation.

### Files changed

- `Cargo.toml`
- `Cargo.lock`
- `apps/device-test/Cargo.toml`
- `apps/device-test/src/main.rs`
- `crates/page-detector/src/lib.rs`
- `crates/task-loop/Cargo.toml`
- `crates/task-loop/src/lib.rs`
- `PLANS.md`
- `CHECKPOINT.md`

### Commands run

- Read task file: `C:\合作工作区\ActingCommand\TASK-P5c-and-P6a-dry-run.md`
- Read Runtime-local `AGENTS.md`, `PLANS.md`, and `CHECKPOINT.md`.
- Checked Runtime repository status.
- Checked or cloned read-only resource repositories:
  - `C:\Users\Alice\Documents\Azur\ActingCommand-Resources-AzurLane`
  - `C:\Users\Alice\Documents\Azur\ActingCommand-Resources-Arknights`
  - `C:\Users\Alice\Documents\Azur\ActingCommand-Resources-BlueArchive`
- `git fetch origin`
- `git pull --ff-only`
- `cargo test -p actingcommand-page-detector`
- `cargo test -p actingcommand-task-loop`
- `cargo test -p actingcommand-device-test`
- `cargo test --workspace`
- `cargo fmt --all -- --check`
- `cargo clippy -p actingcommand-page-detector -p actingcommand-task-loop -p actingcommand-device-test -- -D warnings`
- `cargo tree -p actingcommand-task-loop --depth 1`
- `rg -n "SQLite|sqlite|OCR|ocr|state machine|game logic|opencv|rusqlite|fallback|reconnect|retry|MaaTouch|Screencap|CaptureBackend|Device|tap\(|swipe\(|long_tap\(|reset\(" crates\page-detector crates\task-loop`
- `rg -n "image\s*=|imageproc\s*=|opencv|rusqlite|sqlite|actingcommand-device|actingcommand-runtime-core" crates\page-detector\Cargo.toml crates\task-loop\Cargo.toml`
- `git diff --check`

### Resource repository validation

- AzurLane resource repository:
  - path: `C:\Users\Alice\Documents\Azur\ActingCommand-Resources-AzurLane`
  - commit: `8503ca1`
  - command: `cargo run -p actingcommand-device-test -- detect-page --pack C:\Users\Alice\Documents\Azur\ActingCommand-Resources-AzurLane\recognition\azurlane.jp.pack.json --pack-root C:\Users\Alice\Documents\Azur\ActingCommand-Resources-AzurLane --pages C:\Users\Alice\Documents\Azur\ActingCommand-Resources-AzurLane\recognition\azurlane.jp.pages.json --check-pages`
  - result: `check_pages=passed`
- Arknights resource repository:
  - path: `C:\Users\Alice\Documents\Azur\ActingCommand-Resources-Arknights`
  - commit: `00199ee`
  - command: `cargo run -p actingcommand-device-test -- detect-page --pack C:\Users\Alice\Documents\Azur\ActingCommand-Resources-Arknights\recognition\arknights.cn.pack.json --pack-root C:\Users\Alice\Documents\Azur\ActingCommand-Resources-Arknights --pages C:\Users\Alice\Documents\Azur\ActingCommand-Resources-Arknights\recognition\arknights.cn.pages.json --check-pages`
  - result: `check_pages=passed`
- BlueArchive resource repository:
  - path: `C:\Users\Alice\Documents\Azur\ActingCommand-Resources-BlueArchive`
  - commit: `a5a9749`
  - command: `cargo run -p actingcommand-device-test -- detect-page --pack C:\Users\Alice\Documents\Azur\ActingCommand-Resources-BlueArchive\recognition\bluearchive.jp.pack.json --pack-root C:\Users\Alice\Documents\Azur\ActingCommand-Resources-BlueArchive --pages C:\Users\Alice\Documents\Azur\ActingCommand-Resources-BlueArchive\recognition\bluearchive.jp.pages.json --check-pages`
  - result: `check_pages=passed`

### Test results

- `cargo test -p actingcommand-page-detector` passed with 23 tests.
- `cargo test -p actingcommand-task-loop` passed with 13 tests.
- `cargo test -p actingcommand-device-test` passed with 40 tests.
- `cargo test --workspace` passed.
- `cargo fmt --all -- --check` passed.
- `cargo clippy -p actingcommand-page-detector -p actingcommand-task-loop -p actingcommand-device-test -- -D warnings` passed.
- `git diff --check` passed.
- `cargo tree -p actingcommand-task-loop --depth 1` showed direct dependencies only on `actingcommand-page-detector`, `actingcommand-recognition`, `actingcommand-recognition-pack`, `serde`, and `serde_json`.
- Prohibited-feature scans over `crates/page-detector` and `crates/task-loop` had no matches.
- Direct-dependency scan found no direct `image`, `imageproc`, OpenCV, SQLite, `actingcommand-device`, or `actingcommand-runtime-core` dependency in `crates/page-detector` or `crates/task-loop`.

### P5c detect-page details

- Added `detect-page --check-pages`.
- Added `detect-page --page <page_id> --scene <png>`.
- Added `detect-page --page <page_id> --capture`.
- Added `detect-page --all --scene/--capture`.
- Output remains one `key=value` entry per line.
- Per-target page evidence is printed as `target=<id>,role=<role>,passed=<bool>,message=<message>`.
- `--check-pages` is mutually exclusive with `--page`, `--all`, `--scene`, and `--capture`.
- `detect-page` is a read-only command and is guarded from mixing with MaaTouch input commands.

### P6a task-loop and task-dry-run details

- Added `actingcommand-task-loop`.
- Added TaskPlan schema v0.1 with `complete` and `click` dry-run actions.
- `DryRunTaskLoop::new` validates structure only.
- `DryRunTaskLoop::validate` validates all page and click-target references before dry-run.
- `DryRunTaskLoop::dry_run` evaluates steps in order and stops at the first matching page.
- Added `task-dry-run --scene`.
- Added `task-dry-run --capture`.
- `task-dry-run` validates the task plan before loading scene/capture.
- `task-dry-run` output remains one `key=value` entry per line and always prints `executed=false`.
- `task-dry-run` is a read-only command and is guarded from mixing with MaaTouch input commands.

### Current blocker

- None for P5c or P6a.
- Live `detect-page` and `task-dry-run` verification against real devices remains an Alice/operator step.
- Real TaskPlan ownership is still undecided and should not be placed in resource repositories by default.

### Next step

1. Commit and push Runtime repository changes.
2. Alice can manually run live `detect-page` and `task-dry-run` checks for Azur, Ark, and BA.
3. Choose a next milestone: P6b controlled click execution, Runtime API contract for UI, or capture metadata / SQLite schema design.
4. Add a regression frame-set lane before real page definitions become broad or authoritative.
