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
- Runtime `recognize` errors are fatal and visible.
- No OpenCV in P4a recognition primitives.
- No OCR until a separate scoped milestone.
- No SQLite until a separate scoped milestone.
- No UI in this repository.
- No game logic until a specific runtime/game milestone.
- No click execution in P4c recognition validation.
- No upstream source or asset copying without license, attribution, and boundary review.

## Next steps

1. Define P5 PageDetector using P4c recognition packs without click execution or task flow.
2. Define runtime-owned capture metadata and image reference lifecycle.
3. Define SQLite schema in a separate scoped milestone.
4. Define how Runtime exposes capture and recognition results to UI/API layers.
5. Keep `CHECKPOINT.md` updated with every completed Runtime task.
