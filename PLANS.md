# PLANS.md

## Repository goal

`ActingCommand-Runtime` is the Rust mainline runtime repository for ActingCommand.

The runtime owns device/control primitives, capture primitives, recognition primitives, and later runtime orchestration components behind explicit interfaces.

## Current implementation line

- Rust workspace is the mainline implementation.
- Python runtime is legacy/mock only and lives outside this repository.
- Go runtime/core is historical reference and benchmark material only and lives outside this repository.

## Current completed milestones

- P1.6 MaaTouch input backend stability close-out.
- P2 ADB `exec-out screencap -p` capture backend.
- P2.1 capture artifact store.
- P2.1.1 capture artifact path security close-out.
- P4a threshold-free recognition primitive engine.
- P4a.1 recognition score semantics close-out.
- P4b recognition pack rule and threshold layer.
- P4c recognition pack disk fixtures, read-only recognize entry, and AzurLane JP resource-pack bridge.
- P4c-fixup recognize color diagnostics and ClickOnly CLI input handling.
- P5 PageDetector page recognition layer.
- P5c `device-test detect-page` CLI and multi-resource PageSet validation.
- P6a dry-run task loop.
- P6b/P6c/P6d probe-loop framework.
- P6d live validation and limited resource close-out.

## Recognition score semantics

P4a.1 clarifies template-match score semantics without starting P4b.

`TemplateMatch` carries both:

- `raw_score`: the method-native score returned by the current template matching algorithm.
- `score`: a normalized `0.0..=1.0` score for later rule-layer thresholding. This is not a probability.

Current template matching uses `imageproc` `CrossCorrelationNormalized`. For non-negative image pixels this metric is already in `0.0..=1.0`, so P4a.1 normalization is identity plus clamp, with `NaN` normalized to `0.0`.

P4a.1 remains threshold-free. P4b or higher-level callers own threshold selection, rule data loading, and decision policy.

## Recognition pack rule layer

P4b adds `actingcommand-recognition-pack` as the data-driven rule layer above the P4a primitive engine.

The pack layer owns:

- JSON pack parsing.
- recognition target validation.
- template threshold policy.
- color distance threshold policy.
- coordinate-space checks.
- click-target metadata lookup.

The pack layer deliberately does not own:

- OCR.
- UI.
- SQLite.
- navigation.
- state machines.
- game logic.
- click execution.
- capture persistence.

P4b keeps `crates/recognition` threshold-free and does not add serde to primitive `Rect`. Pack-facing geometry uses `PackRect` and converts into primitive geometry at evaluation time.

## Recognition pack real-data bridge

P4c connects the P4b pack layer to disk fixtures, the resource repository pack format, and a read-only CLI validation entry.

The Runtime side owns:

- synthetic from-disk pack/template/scene integration tests for `actingcommand-recognition-pack`;
- `device-test recognize --check-pack`;
- `device-test recognize --scene`;
- `device-test --port <port> recognize --capture`;
- fixed key-value output for template, color, and click-only targets.

The resource repository side owns:

- `recognition/azurlane.jp.pack.json`;
- cropped patch templates under `recognition/patches/azurlane/jp/`;
- neutral-to-pack conversion tooling.

P4c `recognize` is read-only. It does not start MaaTouch, does not execute clicks, does not write capture artifacts, does not write SQLite, does not run OCR, does not detect pages, and does not run game task logic.

P4c manual calibration is observational. A failed target match on a non-target page is recorded as threshold evidence, not treated as a green functional failure.

P4c-fixup keeps the key-value output format and adds diagnostics without changing read-only behavior:

- Template targets always print `message`.
- Template targets with `color_check` also print `color_distance`, `color_max_distance`, `color_mean`, and `color_expected`.
- Color targets print `message`, `color_mean`, and `color_expected`.
- ClickOnly targets can be queried without `--scene` or `--capture`, and still only print click metadata plus `evaluated=false`.

## PageDetector layer

P5 adds `actingcommand-page-detector` as a page recognition layer above `actingcommand-recognition-pack`.

The PageDetector layer owns:

- PageSet JSON parsing.
- structural page-definition validation.
- eager target-reference validation against `RecognitionEvaluator::target_kind`.
- required/optional/forbidden page evidence evaluation.
- page match summaries and per-target diagnostics.

The PageDetector layer deliberately does not own:

- device access.
- screenshots or capture backends.
- MaaTouch or any click execution.
- SQLite or capture persistence.
- OCR.
- UI.
- navigation.
- state machines.
- game task logic.

P5 evaluates an existing `Scene` with an existing `RecognitionEvaluator`. It only answers whether the current scene matches a page definition. ClickOnly targets are fatal when used as page evidence.

P5c exposes PageDetector through read-only `device-test detect-page`.

The detect-page CLI owns:

- PageSet validation with `--check-pages`.
- single-page scene/capture evaluation with `--page`.
- all-page scene/capture evaluation with `--all`.
- key-value output compatible with existing `recognize` output style.

The detect-page CLI remains read-only. It does not start MaaTouch, does not execute clicks, does not write capture artifacts, does not write SQLite, and does not run game task logic.

P5c also validates the current resource repositories as read-only inputs:

- `ActingCommand-Resources-AzurLane`
- `ActingCommand-Resources-Arknights`
- `ActingCommand-Resources-BlueArchive`

Resource repositories remain the owner of recognition packs, page sets, templates, and resource data. Runtime only consumes them through explicit pack/page schema boundaries.

## Resource repository freshness gate

Any Runtime task that reads or uses resource repository content must refresh the relevant resource repositories from their remotes before the resource-dependent step runs.

Current resource repositories:

- `C:\Users\Alice\Documents\Azur\ActingCommand-Resources-AzurLane`
- `C:\Users\Alice\Documents\Azur\ActingCommand-Resources-Arknights`
- `C:\Users\Alice\Documents\Azur\ActingCommand-Resources-BlueArchive`

The expected refresh flow is `git fetch origin` followed by `git pull --ff-only`. Record the path and commit hash used in `CHECKPOINT.md`.

If a resource repository is dirty, missing, unavailable, or cannot fast-forward, treat that as a blocker and do not continue with the resource-dependent task unless Alice gives an explicit one-off override.

## Dry-run task loop

P6a adds `actingcommand-task-loop` as a minimal dry-run decision layer above PageDetector.

The task-loop layer owns:

- TaskPlan JSON parsing.
- structural task-plan validation.
- reference validation against `PageDetector` and `RecognitionEvaluator`.
- ordered page evaluation by task step.
- dry-run action summaries for `complete` and `click` actions.

The task-loop layer deliberately does not own:

- device access.
- click execution.
- scheduler behavior.
- retries.
- background loops.
- SQLite or state persistence.
- UI.
- game-specific task logic.

P6a click actions return click metadata only. They are not executed.

## Limited-resource probe loop

P6b/P6c/P6d adds a controlled probe lane. P6d changes the execution standard from fully non-destructive to limited-resource operation, but the default live path remains conservative.

The `actingcommand-task-loop` probe layer owns:

- `ProbePlan` schema v0.1 parsing.
- structural probe validation.
- reference validation against `PageDetector`, `RecognitionEvaluator`, and explicit external reference overrides.
- pure probe-step decisions for `detect_page`, `observe_page`, `observe_targets`, and whitelisted click effects.
- effect-aware safety checks for destructive words.
- resource-policy validation for state-changing effects.

The task-loop probe layer deliberately does not own:

- device access.
- MaaTouch sessions.
- actual click-point generation.
- file journals.
- capture polling.
- scheduler behavior.
- retry loops.
- SQLite.
- UI.
- OCR.
- OpenCV.
- game task flow.

Allowed click effects are:

- `NavigationOnly`: page navigation only.
- `FreeClaim`: free reward collection only when a `free_reward` policy explicitly forbids premium currency, refill, and cost.
- `ConsumeRegeneratingResource`: only AzurLane oil, BlueArchive AP, or Arknights sanity with a declared `max_cost`, and still blocked from PvP/exercise routes.

Forbidden actions remain blocked:

- premium or paid currency use.
- paid oil/AP/sanity refill.
- shop purchases.
- gacha, construction, or recruitment.
- retire, delete, decompose, enhance, awaken, or similar destructive account changes.
- exercise/PvP battles.
- blind confirmation prompts.

`device-test probe-run` owns the executable probe bridge:

- required `--capture` mode.
- no `--scene` click execution.
- no mixing with `reset`, `tap`, `longtap`, or `swipe`.
- ScreencapBackend capture before and after actions.
- MaaTouchBackend only after safety checks pass.
- actual click-point generation inside the chosen click rect.
- operation journal files under the provided run root.
- post-click arrival polling.
- failure-visible summaries.
- page-guard failure stops later clicks and records `result=blocked`.
- checkpoint artifacts under `checkpoints/` when frame batches or risky effects require review.

`actual_click_point` records:

- seed.
- algorithm.
- source rect.
- final point.

For BlueArchive JP, `device-test` can load `navigation/bluearchive.jp.navigation.json` as data:

- `navigation/<id>` becomes an external click target.
- `control/<id>` becomes an external click target.
- `navigation/<id>/arrive_anchor` becomes an external page reference.
- `arrive_anchor` full-frame matching is a temporary `device-test` bridge only.
- The task-loop core does not know about BA-specific direct matching.
- Later work should migrate BA arrival anchors into recognition-pack full-frame targets after the schema supports them.

BA forbidden destructive points are checked by rect or radius. Exact-point-only checks are not sufficient.

P6d live validation used only `NavigationOnly` routes. No FreeClaim, regenerating-resource consumption, paid refill, purchase, exercise/PvP, or destructive action was executed.

## P6d benchmark and runner lane

`device-test benchmark` measures the current ActingCommand stack before live execution:

- screenshot latency through `ScreencapBackend`.
- control latency through `MaaTouchBackend` reset operations.
- recommended polling and minimum operation intervals.

`device-test runner` packages recognition, capture, probe-run, and MaaTouch control into a one-shot profile-driven unit:

- no scheduler.
- no background resident process.
- no SQLite.
- independent run directories per port/process.
- one failed probe is recorded without hiding the failure.

Runner multi-open validation currently uses the BA JP smoke profile. Non-BA devices are expected to stop at page guard with `click_count=0`; the BA device may execute only the verified `NavigationOnly` home-to-task-and-back route.

## Repo-local planning policy

Runtime planning and checkpoint records live in this repository.

For Runtime tasks, update `PLANS.md` and `CHECKPOINT.md` here and commit them with the Runtime source changes. Do not mirror Runtime task planning files into the umbrella repository by default.

Routine Runtime updates must stay in `HS7097/ActingCommand-Runtime`. Do not merge, copy, or synchronize Runtime changes into the umbrella/main `HS7097/ActingCommand` repository unless the user explicitly confirms that a specific Runtime state is ready for that merge.

## Active boundaries

- No ADB input fallback.
- MaaTouch failure is fatal.
- Capture failure is fatal.
- Recognition primitive errors are fatal.
- Recognition pack validation and evaluation errors are fatal.
- PageDetector parse, validation, and evaluation errors are fatal.
- Task-loop parse, validation, and dry-run errors are fatal.
- Runtime `recognize` errors are fatal and visible.
- Runtime `detect-page` and `task-dry-run` errors are fatal and visible.
- Runtime `probe-run` errors are fatal and visible.
- No OpenCV in P4a recognition primitives.
- No OCR until a separate scoped milestone.
- No SQLite until a separate scoped milestone.
- No UI in this repository.
- No game logic until a specific runtime/game milestone.
- No click execution in P4c recognition validation.
- No click execution or device access in P5 PageDetector.
- No click execution, scheduler, SQLite, background loop, or game logic in P6a task-loop.
- No device access or click execution in the P6b/P6c/P6d task-loop probe core.
- P6b/P6c/P6d device-test click execution is navigation-only and MaaTouch-only.
- Do not commit MaaTouch binaries; use local-only external tools or `--local <path>`.
- No upstream source or asset copying without license, attribution, and boundary review.

## Next steps

1. Deploy MaaTouch locally outside the repository or pass it with `--local <path>`.
2. Run BA JP `probe-run` on a live BA home screen and confirm `home_to_task` reaches the arrival anchor before enabling further probe steps.
3. Add resource definitions for AzurLane mission/commission pages before AzurLane probes.
4. Add Arknights operator/menu navigation targets before Arknights probes.
5. Upgrade BA arrival anchors from the temporary `device-test` direct bridge into recognition-pack full-frame targets.
6. Define Runtime API contracts for UI integration in a separate milestone.
7. Define capture metadata and SQLite schema in a separate scoped milestone.
8. Create a regression frame-set lane so each page has positive and negative sample frames guarding recognition drift.
9. Keep `CHECKPOINT.md` updated with every completed Runtime task.
