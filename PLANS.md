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
- P6d/P6e-half resource-independent close-out: click page guard, MaaTouch license/path fix, benchmark labeling, and BA regression blocker report.
- ActingLab-P1 runtime-embedded direction: Python Runtime-side Lab cleanup audit and Rust embedded Lab planning.
- ActingLab-P1a/P1b Rust embedded lab skeleton: `LabMode`, `LabLeaseRequest`, `SchedulerGate`, scoped instance decisions, and no-click/passive-mode boundaries.
- ActingLab-P1g global CLI contract shell: `actinglab` app, unified JSON envelope, fixed exit-code mapping, config/doctor/capabilities, package zip safety validation, scheduler/lab safety stubs, and Windows user PATH launchers.
- Lab-1X trusted one-shot package execution engine: `actinglab lab run --zip <input.zip> --out <output.zip>` with Lab-1X control/resources ingest, resource integrity checks, Screencap/MaaTouch execution path, page confirmation, output report zips, and device-unreachable failure reporting.
- P2.2/Lab-1y capture-backend and trusted execution upgrade: unified capture frames, selectable `adb_screencap`/`droidcast_raw`/`nemu_ipc` backends, backend diagnostics, Lab-1y control modes, local LabLease lock, and output timing stats.
- P2.2 capture backend repair close-out: Nemu IPC UTF-16 path passing, DroidCast_raw natural-buffer rotation, `lab run --capture-backend` CLI priority, and auto backend probe downgrade.
- P2.3 capture pipeline refactor: capture backends now return raw pixel frames without hot-path PNG encoding, ADB preserves original screencap PNG for artifact writes, recognition can consume raw RGB/RGBA pixels, Nemu IPC caches resolution, and `device-test benchmark` reports capture-only, encode-only, and end-to-end timing.
- Lab-1z fixes: explicit frame recognition lifecycle, admission-before-store memory estimation, sync segment-zip flush, current-frame Tier3 pause/resume checkpointing, conservative resident-byte accounting, temp cleanup, and P2.3 capture hot-path non-regression benchmark.
- Lab-1z Round2 stability close-out: P2.2 device deadlines, Lab-1y cleanup, frame-store accounting/spill fixes, P1g package hardening, and release benchmark non-regression.
- Round2 regression close-out: segment-write failure keeps per-frame encode failures, Lab run-dir cleanup no longer deletes diagnostics or in-run outputs, Tier3 checkpoints include step context, and Nemu IPC worker shutdown is no longer double-invoked.
- ActingLab direct touch CLI: main `actinglab` now exposes trusted manual `tap`, `swipe`, and `long-tap` commands through the existing MaaTouch backend, while `capture` remains the screenshot side of the unified CLI.
- Lab packager foundation and production builder: `resource convert` is now Rust-backed with converter parity, `lab validate` validates Lab-1y input zips offline, and `package build-task` / `package build-pack` can build self-contained Lab packages from local or explicitly cloned resource repositories.
- Runtime ADB connection hardening: Runtime and CLI device entry points now resolve one matching adb path through the device layer, prefer the reviewed MuMu adb, avoid PATH/venv adb defaults, preserve wall-clock command deadlines, and only perform one bounded `adb connect` when device state is unavailable.
- Runtime full-frame recognition hang fix: large template searches now use a bounded pyramid/refine path with a fatal deadline, including `full_frame`, explicit whole-frame rectangles, and large bounded regions.
- ActingLab session layer Phase A: local session daemon lifecycle, instance health/reconnect, app launch/stop/restart, explicit MaaTouch key/text input, and stale-aware `--require-fresh` capture diagnostics.
- ActingLab session layer Phase B: semantic `current-page`, `is-visible`, `locate`, `tap-target`, and `navigate` CLI entry points with navigation-only safety gates.
- ActingLab session layer Phase C diagnosis and initial recovery: `monitor --once` reports current session health and recovery availability; `session recover` plans or executes maintenance-only recovery toward a known target page, using standby wake control points and safe navigation routes.
- Resource repository reorganization compatibility: ActingLab resource, recognition, navigation, and package-build entry points accept both direct resource roots and reorganized repository roots that contain `ours/`.
- ActingLab session layer Phase C startup-login resource loop: `session recover --startup-login` reads `STARTUP-LOGIN.md` and runs a bounded maintenance-only popup-close/continue loop toward the target page.
- ActingLab session layer Phase C bounded monitor loop: `monitor` now runs bounded diagnosis iterations and can explicitly delegate non-healthy states to `session recover` when `--recover` is present.
- ActingLab session layer Phase A/C capture stale diagnostics: `capture diagnose` and `session capture diagnose` run a structured fresh-frame probe, report stale/unavailable capture states, and recommend lighter capture-backend recovery before app restart.
- ActingLab session daemon request channel: the resident daemon now processes a narrow file-IPC request queue for read-only `capture_diagnose` requests, allowing `capture diagnose --via-daemon` and `session request capture-diagnose` to execute through the running daemon.
- ActingLab session daemon read-only semantic routing: `recognize`, `detect-page`, `current-page`, `is-visible`, and `locate` can now submit read-only requests through the same daemon queue with `--via-daemon` or `session request ...`.
- ActingLab daemon monitor-once routing: `monitor --once --via-daemon` and `session request monitor-once` now run read-only health diagnosis through the resident daemon while `--recover` remains blocked until lease arbitration exists.
- ActingLab session lease arbitration interface hardening: `session lease acquire|release|preempt|status` now uses structured lease records with holder checks, optional lease ids, force release, and preempt provenance.
- ActingLab daemon lease-gated control request routing: `tap`, `swipe`, `long-tap`, `key`, `text`, `tap-target`, `navigate`, and `session recover` can now be submitted to the resident daemon only with matching session lease metadata.
- ActingLab daemon monitor recovery routing: bounded `monitor --via-daemon --recover` and `session request monitor --recover` can now run through the resident daemon only after a matching session lease is validated.
- ActingLab session recording context skeleton: `session record start|status|stop` can create and inspect a local JSON recording context with `auto_recording=false`, while step authoring and task bundle generation remain explicitly reserved.
- ActingLab session recording step schema: `session record step --kind anchor|operation` now appends explicit authorized step metadata to an active recording context without capturing frames or writing resources.
- ActingLab session recording amend schema: `session record amend` can update existing authorized anchor/operation step metadata without capturing frames, backtesting, or writing resources.
- ActingLab session recording anchor frame materialization: authorized anchor steps can optionally attach a local PNG source frame, crop a rect-only template draft artifact, and record frame/artifact hashes without device I/O or resource writes.

## Current ActingLab Session Recording Anchor Frame Materialization

The current Runtime task advances Phase D by adding optional local frame provenance and rect crop materialization to authorized anchor steps. This remains an offline recording-authoring aid: it can consume a user-supplied PNG frame and write a draft crop artifact under the session state tree or an explicit artifact directory, but it does not capture from a device, backtest recognition, or write resource repositories.

Scope:

- Add optional `--frame <png>` and alias `--source-frame <png>` to `session record step --kind anchor`.
- Add optional `--artifact-dir <dir>` for local draft artifact output.
- When a frame is supplied, require `--region x,y,width,height`.
- Reject `--region auto` with a supplied frame because automatic anchor candidate selection is not implemented.
- Decode the local PNG into an in-memory frame, crop the requested rect, encode the crop as a PNG draft artifact, and record hashes and dimensions.
- Store `frame_provenance` and `artifact` metadata on the anchor step and persisted record context.
- Preserve the metadata-only anchor path when no frame is supplied.

Safety direction:

- This milestone performs no device I/O and does not open MaaTouch.
- This milestone does not live-capture frames, run recognition, backtest anchors, write resource packs, generate task bundles, touch SQLite, implement UI, or add game logic.
- Local frame decode, crop bounds, artifact directory creation, and artifact writes fail visibly instead of silently recording incomplete metadata.
- `session record build-task` remains explicitly not implemented.

Validation status:

- Runtime was already aligned with `origin/main` before this task.
- `cargo test -p actingcommand-actinglab session_record_step_anchor -- --nocapture` passed with `4` tests.
- `cargo test -p actingcommand-actinglab session_record -- --nocapture` passed with `14` tests.
- `cargo test -p actingcommand-actinglab` passed with `122` tests.
- `cargo fmt --all -- --check` passed.
- `cargo test --workspace` passed.
- `cargo clippy --workspace -- -D warnings` passed after boxing optional anchor artifact metadata to keep enum variants compact.
- `git diff --check` passed.
- Added-code prohibited-feature scan over source changes returned no matches for device input fallback, `adb shell screencap`, MaaTouch startup, SQLite, OCR/OpenCV, fallback, reconnect, retry, or live capture routing.

Known follow-ups:

- Add capture/current-frame integration only after the session daemon and stale-frame policy are wired into the recording path.
- Define anchor backtest semantics before promoting draft artifacts into resource repositories.
- Implement `session record build-task` and resource-write integration in a later milestone.

## Current ActingLab Session Recording Amend Schema

The current Runtime task adds local metadata amendment for already authorized recording steps. It lets an operator correct anchor or operation fields in the recording context, but it does not perform frame capture, template cropping, anchor backtesting, resource writes, or task-bundle generation.

Scope:

- Add `session record amend <step-id>` and `session record amend --step-id <id>`.
- Anchor amendments may update:
  - `--id <page>`
  - `--region <auto|x,y,width,height>`
  - `--color-check`
  - `--no-color-check`
  - `--threshold <0..1>`
  - `--clear-threshold`
- Anchor amendments reset evaluation to `deferred` with reason `amended_needs_backtest`.
- Operation amendments may update:
  - `--from <page>`
  - `--to <page|null>`
  - `--click <x,y|target>`
  - `--destructive`
  - `--non-destructive`
- Missing/inactive records and missing step ids fail visibly.
- Unknown step ids fail with `record_step_not_found`.
- Amend commands that contain no supported field for the target step kind fail instead of silently succeeding.
- Add `updated_at_unix_ms` to step records.
- Advertise `session record amend` as an offline available capability.

Safety direction:

- This milestone performs no device I/O and does not open MaaTouch.
- This milestone does not capture frames, crop templates, run recognition, backtest anchors, write resource packs, generate task bundles, touch SQLite, implement UI, or add game logic.
- `session record build-task` remains explicitly not implemented.
- The top-level `record` capability remains reserved because full frame-stream recording is not implemented.

Validation status:

- Runtime was fetched and confirmed aligned with `origin/main` before the task.
- `cargo test -p actingcommand-actinglab session_record -- --nocapture` passed with `11` tests.
- `cargo test -p actingcommand-actinglab` passed with `119` tests.
- `cargo fmt --all -- --check` passed.
- `cargo test --workspace` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `git diff --check` passed.
- Added-code prohibited-feature scan returned no matches for device input fallback, screenshot/capture backend additions, SQLite, OCR/OpenCV, reconnect, retry, or MaaTouch startup.

Known follow-ups:

- Define frame provenance and then implement anchor capture/crop/backtest.
- Implement `session record build-task` and resource-write integration in a later milestone.
- Consider moving the recording model out of the monolithic CLI file once the schema stabilizes.

## Current ActingLab Session Recording Step Schema

The current Runtime task advances Phase D by adding explicit recording-step authorization inside an active recording context. It records the operator's intent for anchor and operation steps, but keeps frame capture, backtesting, resource writes, amend, and build-task output for later milestones.

Scope:

- Add `session record step --kind anchor`.
- Anchor steps require `--id <page>` and `--region <auto|x,y,width,height>`.
- Anchor steps may include `--color-check` and `--threshold <0..1>`.
- Anchor step evaluation is recorded as `deferred` with reason `capture_and_backtest_not_implemented`.
- Add `session record step --kind operation`.
- Operation steps require `--from <page>`, `--to <page|null>`, and `--click <x,y|target>`.
- Operation steps may include `--destructive`.
- Add optional `--step-id`; otherwise ids are generated as `step-0001`, `step-0002`, and so on.
- Reject duplicate step ids and missing/inactive recording contexts visibly.
- Keep `session record amend` and `session record build-task` explicit `not_implemented` responses.
- Advertise `session record step` as an offline available capability.

Safety direction:

- This milestone performs no device I/O and does not open MaaTouch.
- This milestone does not capture frames, crop templates, run recognition, backtest anchors, write resource packs, generate task bundles, touch SQLite, implement UI, or add game logic.
- `session record step` only appends explicit operator-authorized metadata to the local recording context.
- The top-level `record` capability remains reserved because full frame-stream recording is not implemented.

Validation status:

- Runtime was fetched and confirmed aligned with `origin/main` before the task.
- `cargo test -p actingcommand-actinglab session_record -- --nocapture` passed with `8` tests.
- `cargo test -p actingcommand-actinglab` passed with `116` tests.
- `cargo fmt --all -- --check` passed.
- `cargo test --workspace` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `git diff --check` passed.
- Added-code prohibited-feature scan returned no matches for device input fallback, screenshot/capture backend additions, SQLite, OCR/OpenCV, reconnect, retry, or MaaTouch startup.

Known follow-ups:

- Implement `session record amend` once patch/update semantics for steps are defined.
- Implement anchor capture, crop, and backtest only after the frame provenance model is finalized.
- Implement `session record build-task` and resource-write integration in a later milestone.

## Current ActingLab Session Recording Context Skeleton

The current Runtime task advances Phase D of the session layer by adding the smallest useful recording-session context. It opens a local record context for a task and instance, but it does not capture screenshots, authorize steps, write resources, or build task bundles.

Scope:

- Add `session record start --task-id <id>` as an offline command.
- Add `session record status` and `session record stop`.
- Store one structured JSON context per instance under the selected session state directory.
- Include schema version, record id, task id, instance, status, optional holder, optional lease id, timestamps, and an empty `steps` array.
- Return `auto_recording=false` so callers cannot mistake the context for automatic capture.
- Block a second active `start` unless `--force` is provided.
- Keep `session record step`, `session record amend`, and `session record build-task` explicit `not_implemented` responses for the next authoring milestone.
- Advertise `session record` as an offline available capability.

Safety direction:

- This milestone performs no device I/O and does not open MaaTouch.
- This milestone does not capture screenshots, read/write resource packs, generate bundles, run OCR, run recognition, touch SQLite, implement UI, or add game logic.
- Reserved authoring actions return before creating state files.
- The top-level `record` capability remains reserved because full resource recording is not implemented.

Validation status:

- Runtime was fetched and confirmed aligned with `origin/main` before the task.
- `cargo test -p actingcommand-actinglab session_record -- --nocapture` passed with `4` tests.
- `cargo test -p actingcommand-actinglab` passed with `112` tests.
- `cargo fmt --all -- --check` passed.
- `cargo test --workspace` passed.
- `cargo clippy --workspace -- -D warnings` passed after collapsing one clippy-reported nested `if`.
- `git diff --check` passed.
- Added-code prohibited-feature scan returned no matches for device input fallback, screenshot/capture backends, SQLite, OCR/OpenCV, reconnect, retry, or MaaTouch startup.

Known follow-ups:

- Implement explicit `session record step` authoring only after step schemas are finalized.
- Implement record amend and build-task/package output in a later milestone.
- Add resource-write integration only after the recording authorization model is complete.

## Current ActingLab Daemon Monitor Recovery Routing

The current Runtime task advances Phase C by allowing bounded monitor recovery to run through the resident session daemon when the request has a valid session lease.

Scope:

- Add `monitor` as a daemon request command.
- Allow `monitor --via-daemon` without requiring `--once`.
- Keep `monitor --via-daemon` without `--recover` read-only and lease-free.
- Allow `monitor --via-daemon --recover` only when the daemon request includes matching lease metadata.
- Add `session request monitor` as the explicit request form.
- Keep `session request monitor-once` and `monitor-once` daemon requests read-only; `--recover` on `monitor-once` is still rejected with a visible safety error.
- Reuse the existing bounded monitor loop and existing `session recover` maintenance path instead of adding a second recovery implementation.
- Preserve the existing `--max-iterations`, `--interval-ms`, `--capture`, `--scene`, `--require-fresh`, `--fresh-delay-ms`, `--max-actions`, `--step-timeout-ms`, `--poll-ms`, and startup-login arguments through the daemon request payload.

Safety direction:

- Recovery through the daemon requires a matching lease holder and optional lease id before any diagnosis or maintenance action runs.
- Wrong holder and wrong lease id fail before capture, MaaTouch, or recovery logic is opened.
- `monitor-once` remains a read-only diagnosis command; bounded `monitor` is the recovery-capable command.
- The implementation does not add ADB input fallback, reconnect loops, retries, OCR, SQLite, UI, scheduler body, recording, capture backend changes, recognition algorithm changes, or game logic.

Validation status:

- Runtime was fetched and confirmed aligned with `origin/main` before the task.
- `cargo test -p actingcommand-actinglab session_monitor` passed.
- `cargo test -p actingcommand-actinglab monitor_via_daemon` passed.
- The first full `cargo test -p actingcommand-actinglab` run hit a transient existing parallel-test configuration race; the rerun passed with `108` tests.
- Local daemon smoke with an `ak` scheduler lease and mismatched recovery holder returned a lease-holder safety block before capture or input.
- Local daemon smoke with a matching `ak` scheduler lease reached normal validation after lease acceptance, proving the request was no longer rejected by the old daemon recovery gate.

Known follow-ups:

- Full live recovery on an emulator with real resources still needs a safe simulator state.
- Scheduler ownership and automatic daemon-resident background monitoring remain outside this milestone.
- Package run, operation run, API/event streaming, UI integration, recording, and mandatory daemon-only policy for non-manual callers are still open.

## Current ActingLab Session Daemon Lease-Gated Control Requests

The current Runtime task connects the structured session lease interface to the resident daemon request channel for task-level control commands.

Scope:

- Keep read-only daemon requests unchanged.
- Add lease metadata to `SessionCommandRequest`.
- Strip `--lease-holder`, `--holder`, and `--lease-id` from inner command arguments before daemon execution.
- Allow top-level control commands to use `--via-daemon`:
  - `tap`
  - `swipe`
  - `long-tap`
  - `key`
  - `text`
  - `tap-target`
  - `navigate`
  - `session recover`
- Allow equivalent `session request` control commands:
  - `session request tap`
  - `session request swipe`
  - `session request long-tap`
  - `session request key`
  - `session request text`
  - `session request tap-target`
  - `session request navigate`
  - `session request recover`
- Require `--lease-holder <id>` or `--holder <id>` for daemon control requests.
- Validate optional `--lease-id <id>` before any device I/O.
- Reject missing leases, wrong holders, and wrong lease ids as visible safety-blocked failures.
- Map daemon lease errors back to client-side safety-blocked errors instead of reporting fake runtime success or a misleading runtime-not-running failure.
- Advertise lease-gated daemon control requests in `capabilities`.

Safety direction:

- Only daemon-routed control requests are gated in this milestone.
- Existing direct local manual commands remain available for trusted manual use.
- The failure tests validate that lease errors happen before MaaTouch/device input is opened.
- The local daemon smoke used a mismatched holder and confirmed no tap was sent.
- No ADB input fallback, reconnect, retry loop, OCR, SQLite, UI, scheduler body, recording, capture backend, or recognition algorithm change was added.

Validation status:

- Runtime was fetched and confirmed aligned with `origin/main` before the task.
- `cargo test -p actingcommand-actinglab session_control_request_requires_lease_metadata` passed.
- `cargo test -p actingcommand-actinglab session_control_request_rejects_wrong_holder_before_device_io` passed.
- `cargo test -p actingcommand-actinglab session_control_request_rejects_wrong_lease_id_before_device_io` passed.
- `cargo test -p actingcommand-actinglab direct_touch_via_daemon_accepts_lease_flags_before_daemon_lookup` passed.
- `cargo test -p actingcommand-actinglab` passed.
- A local daemon smoke acquired an `ak` lease for `scheduler`, submitted `tap --via-daemon` with holder `lab`, and received a lease-holder safety block with exit code `3` before device input.

Known follow-ups:

- Matching-lease daemon control execution still needs live validation on a safe simulator state.
- `monitor --once --via-daemon --recover` remains blocked; recovery can be submitted through `session request recover` or `session recover --via-daemon` with a lease.
- Direct local manual commands still bypass daemon ownership by design for this milestone; future policy may make daemon routing mandatory for non-manual callers.
- Package run, operation run, scheduler body, API/event streaming, UI integration, and recording are still outside this milestone.

## Current ActingLab Session Lease Arbitration Interface

The current Runtime task hardens the session lease interface required by `TASK-Lab-session-layer.md` before input, navigation, and recovery can safely move behind the resident daemon.

Scope:

- Keep `session lease acquire|release|preempt|status` as the local scheduler/consumer lease interface.
- Store structured `SessionLease` records instead of loose JSON objects.
- Include `instance`, `holder`, `lease_id`, `acquired_at_unix_ms`, `updated_at_unix_ms`, `preempted`, and optional previous-lease provenance.
- Generate a lease id when `--lease-id` is not provided.
- `acquire` fails visibly with `lease_conflict` if a lease already exists.
- `release` now verifies `--holder` and optional `--lease-id`.
- `release --force` can release a mismatched lease for scheduler/manual recovery paths.
- `preempt` writes a new lease and records the previous holder and lease id.
- Lease files are published with the atomic write path.

Safety direction:

- This is an arbitration-interface milestone only.
- No command starts using the lease as an authorization gate yet.
- No tap, key, text, navigate, recover execution, app restart, scheduler body, UI, SQLite, OCR, capture backend, or recognition change was added.
- Future task-level input and maintenance recovery should require a matching lease holder before executing through the daemon.

Validation status:

- Runtime and the three resource repositories were fetched and confirmed aligned with `origin/main`.
- `cargo test -p actingcommand-actinglab session_lease` passed.
- `cargo test -p actingcommand-actinglab` passed.
- First `cargo test --workspace` exposed a parallel-test environment issue in the new lease tests; the lease tests now take the existing `ENV_LOCK`, and the rerun passed.
- `cargo test --workspace` passed.
- `cargo fmt --all -- --check` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `git diff --check` passed.
- Diff prohibited-feature scan returned no matches.
- A local CLI smoke acquired, inspected, and released a scheduler-held lease with `--lease-id smoke-1`.

Known follow-ups:

- Lease files are not yet enforced by input, navigation, package run, operation run, monitor recovery, or app recovery commands.
- The resident daemon still needs lease-aware request authorization before accepting task-level input or recovery commands.
- Scheduler integration remains outside this milestone.

## Current ActingLab Session Daemon Read-Only Request Routing

The current Runtime task continues converting one-shot CLI execution into a resident Session Layer mechanism by routing more read-only recognition and status commands through the local file-IPC request channel.

Scope:

- The session daemon polls pending JSON requests and processes them serially.
- Existing request and response directories under the session state directory remain the local CLI-to-daemon transport.
- `capture diagnose --via-daemon` and `session request capture-diagnose` remain available.
- Add `recognize --via-daemon` and `session request recognize`.
- Add `detect-page --via-daemon` and `session request detect-page`.
- Add `current-page --via-daemon` and `session request current-page`.
- Add `is-visible --via-daemon` and `session request is-visible`.
- Add `locate --via-daemon` and `session request locate`.
- Add `monitor --once --via-daemon` and `session request monitor-once`.
- Request-only client flags such as `--via-daemon`, `--state-dir`, and `--request-timeout-ms` are stripped before the daemon executes the inner command, preventing recursive request submission.
- Response files preserve success data or visible structured errors.
- Client-side request submission remains bounded by `--request-timeout-ms`, default `10000`.
- The daemon heartbeat records whether a request was processed.

Safety direction:

- The daemon-routed commands in this phase are read-only capture, recognition, page detection, visibility, and template-location checks.
- Daemon-routed monitor is one-shot diagnosis only. `--recover` is safety-blocked until scheduler lease arbitration is connected.
- No tap, key, text, navigate, recover, app restart, game-task action, scheduler body, UI, SQLite, OCR, or new capture backend was added.
- Device input commands remain outside daemon request dispatch until lease and arbitration rules are stronger.
- Daemon request failures propagate visibly instead of being treated as empty or successful responses.

Validation status:

- Runtime and the three resource repositories were fetched and confirmed aligned with `origin/main`.
- `cargo test -p actingcommand-actinglab session_request` passed.
- `cargo test -p actingcommand-actinglab readonly_via_daemon_without_daemon_is_runtime_error` passed.
- `cargo test -p actingcommand-actinglab monitor_via_daemon` passed.
- `cargo test -p actingcommand-actinglab monitor_once_via_daemon_without_daemon_is_runtime_error` passed.
- `cargo test -p actingcommand-actinglab direct_touch_commands_are_capability_registered` passed.
- A live-safe smoke started the session daemon, submitted AK `current-page --via-daemon --capture` for `127.0.0.1:16416`, received a daemon response with `mode = daemon_request`, `daemon_command = current_page`, and `page = arknights/home`, then stopped the daemon.
- A second live-safe smoke submitted AK `monitor --once --via-daemon --capture` for `127.0.0.1:16416`, received `daemon_command = monitor_once`, `status = healthy`, and `click_allowed = false`, then stopped the daemon.
- `cargo test -p actingcommand-actinglab` passed.
- `cargo test --workspace` passed.
- `cargo fmt --all -- --check` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `git diff --check` passed.
- Diff prohibited-feature scan returned no matches.

Known follow-ups:

- This is a read-only resident request lane, not the complete command routing layer.
- Raw capture file writing, input commands, semantic tap, navigation, recovery, app restart, lease arbitration, API/event streaming, UI integration, recording, and scheduler body routing are still outside this phase.
- The daemon still needs scheduler-owned lease arbitration before it can accept input or recovery requests.
- Future work should add lease-gated maintenance recovery requests and later route input/navigation only through explicit lease checks.

## Current ActingLab Capture Stale Diagnostics

The current Runtime task responds to `FINDING-AK-game-freeze-2026-06-27.md` by adding a read-only capture diagnosis path for stale-frame suspicion.

Scope:

- Add `capture diagnose`.
- Add `session capture diagnose` through the existing `session capture` route.
- Diagnose mode does not require `--out` and does not write a screenshot file.
- Diagnose mode performs the same two-frame fresh probe used by `--require-fresh`.
- Diagnose output reports `fresh`, `stale_suspected`, or `capture_unavailable`.
- Diagnose output includes backend attempts, frame hash metadata when fresh, and structured recovery recommendations.
- Existing `capture --require-fresh` still fails visibly when no backend produces a changing probe frame.
- Existing capture hot path and selected capture backends are not changed.

Safety direction:

- Diagnose mode is read-only: `click_allowed = false` and `action_executed = false`.
- Stale suspicion recommends capture-backend changes/configuration before the heavier `session app restart` recovery.
- Unavailable capture points to `session instance health`.
- No automatic restart, click, reconnect loop, OCR, SQLite, UI, game task, or new capture backend was added.

Validation status:

- Runtime and the three resource repositories were fetched and confirmed aligned with `origin/main`.
- `cargo test -p actingcommand-actinglab capture_diagnosis` passed.
- `cargo test -p actingcommand-actinglab fresh_auto_probe_prefers_fast_backends_before_adb` passed.
- `cargo test -p actingcommand-actinglab direct_touch_commands_are_capability_registered` passed.
- `cargo test -p actingcommand-actinglab` passed after rerunning a transient temp-config EOF test failure.
- `cargo test --workspace` passed.
- `cargo fmt --all -- --check` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `git diff --check` passed.
- Diff prohibited-feature scan returned no matches.
- Read-only Arknights B server smoke on `127.0.0.1:16416` with `--capture-backend adb capture diagnose --fresh-delay-ms 200` returned `status = fresh`; two ADB screencap probe hashes differed, so stale capture was not suspected in that run.

Known follow-ups:

- A daemon-resident monitor still needs to consume this diagnosis and decide, under scheduler ownership, when to retry a capture backend, switch backend, or restart the app.
- `capture diagnose` currently reports recommendations only; it deliberately does not execute recovery.

## Current ActingLab Session Layer Phase C Bounded Monitor Loop

The current Runtime task advances Phase C by turning the previously reserved `monitor` entry point into a bounded loop over the existing one-shot diagnosis and recovery mechanisms.

Scope:

- `monitor` without `--once` now runs a bounded loop.
- `--max-iterations <n>` controls the loop bound and must be greater than `0`.
- `--interval-ms <ms>` controls the delay between iterations.
- Default `monitor` behavior is read-only and does not recover or click.
- `monitor --recover` explicitly delegates non-healthy diagnoses to `session recover`.
- Recovery delegation reuses existing `session recover` safety gates instead of duplicating a second recovery path.
- Recovery arguments preserve target page, scene or capture source, freshness flags, action limits, poll timing, and startup-login options.
- Real recovery still requires `--capture`; using `--recover --scene` without `--dry-run` fails visibly.
- No scheduler body, daemon-resident monitor, UI, SQLite, OCR, game-task logic, ADB input fallback, or new capture backend was added.

Safety direction:

- Read-only monitor iterations never invoke `session recover`.
- `--recover` is an explicit opt-in and reports `read_only = false`.
- Dry-run recovery reports plans without touching the emulator.
- Real recovery remains gated by the existing capture requirement and existing maintenance-only recovery safety gates.
- Failures from diagnosis or delegated recovery propagate as visible CLI failures; no fake success is returned.

Validation status:

- Runtime and the three resource repositories were fetched and confirmed aligned with `origin/main`.
- `cargo test -p actingcommand-actinglab monitor_loop` passed.
- `cargo test -p actingcommand-actinglab monitor_once` passed.
- `cargo test -p actingcommand-actinglab session_recover` passed.
- `cargo test --workspace` passed.
- `cargo fmt --all -- --check` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `git diff --check` passed.
- Diff prohibited-feature scan returned no matches.
- Arknights dry-run `monitor --recover --startup-login` against the real resource repository root diagnosed a standby frame, delegated to startup-login recovery, and planned the popup-close `(1205, 67)` and continue `(640, 360)` maintenance taps without connecting to the emulator or clicking.

Known follow-ups:

- This is still a CLI-bounded loop, not a daemon-resident monitor.
- Scheduler lease ownership, background self-heal arbitration, modal dismissal policy, app restart policy, stale-frame escalation policy, and resident event streaming remain future Phase C work.

## Current ActingLab Session Layer Phase C Startup-Login Resource Loop

The current Runtime task connects the first explicit startup-login resource path from `TASK-Lab-session-layer.md`.

Scope:

- Add `session recover --startup-login`.
- Read `STARTUP-LOGIN.md` from the resolved resource root, including reorganized `<repo>/ours` roots.
- Support `--startup-login-file <path>` for explicit resource-file validation.
- Extract the required popup-close coordinate and continue/center coordinate from the resource file.
- Fail visibly if the startup-login resource file is missing, malformed, or missing either coordinate.
- Keep dry-run planning available with `--scene`.
- Keep real execution gated by the existing `session recover` requirement for `--capture`.
- During real execution, run a bounded loop using MaaTouch semantic taps, then recapture and detect the page after each round.
- Bound the loop with `--startup-max-rounds`, default `25`, and `--startup-interval-ms`, default `2000`.
- Reuse the existing PageDetector, capture path, resource-root resolution, and MaaTouch semantic input path.

Safety direction:

- This path is explicit; ordinary `session recover` behavior does not change unless `--startup-login` is present.
- The loop is maintenance-only and reports `safety_gate = maintenance_login_only`.
- No ADB input fallback, new capture backend, OCR, SQLite, UI, scheduler body, recording body, or game-task execution was added.
- Missing or unusable startup-login resources fail as safety-blocked errors instead of silently guessing coordinates.

Validation status:

- Runtime and the three resource repositories were fetched and confirmed aligned with `origin/main`.
- `cargo test -p actingcommand-actinglab session_recover_startup_login` passed.
- `cargo test -p actingcommand-actinglab session_recover` passed.
- `cargo test --workspace` passed.
- `cargo fmt --all -- --check` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `git diff --check` passed.
- Diff prohibited-feature scan returned no matches.
- Arknights dry-run `session recover --startup-login` against the real resource repository root read `ours/STARTUP-LOGIN.md` and planned the popup-close `(1205, 67)` and continue `(640, 360)` maintenance taps without connecting to the emulator or clicking.

Known follow-ups:

- The resident/background monitor still needs to invoke startup-login recovery under scheduler lease ownership.
- Modal dismissal policy, app restart policy, stale-frame escalation policy, and scheduler-coordinated self-heal ownership are still future Phase C work.
- Startup-login resources for AzurLane and BlueArchive should be added before this path can be used across all games.

## Current Resource Repository Reorganization Compatibility

The current Runtime task keeps the Codex workspace executable after the resource repositories were reorganized into `upstream-derived/` and `ours/`.

Scope:

- `--resource-root <repo>` now resolves to `<repo>/ours` when the input is a reorganized resource repository root.
- `--resource-root <repo>/ours` remains a direct resource root.
- `resource validate --repo <repo>` reports the original input, resolved resource root, and layout.
- `resource convert --repo <repo>` uses the resolved resource root and reports `resource_layout`.
- `package build-task` and `package build-pack` resolve local and cloned resource repositories to `ours` before loading operations, recognition, and navigation data.
- The packaged output format remains unchanged: package resources are still emitted under `resources/`.

Safety direction:

- This is a resource-root compatibility change only.
- No device input, capture backend, recognition hot-path, scheduler, UI, SQLite, OCR, or game-task logic is changed.
- If neither the provided path nor `<path>/ours` looks like a resource root, existing fatal validation errors still surface from the attempted direct path.

Validation status:

- Runtime and the three resource repositories were fetched and confirmed aligned with `origin/main`.
- `cargo test --workspace` passed.
- `cargo fmt --all -- --check` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `git diff --check` passed.
- Diff prohibited-feature scan returned no matches.
- `detect-page --check-pages` passed for Arknights, AzurLane, and BlueArchive when `--resource-root` was set to the repository root rather than `ours`.
- `resource convert --dry-run` passed for Arknights, AzurLane, and BlueArchive when `--repo` was set to the repository root.
- `package build-task --dry-run` passed for Arknights `open_terminal` from the repository root.
- `package build-pack --dry-run` passed for BlueArchive from the repository root.

Known follow-ups:

- `--from-remote` package builds should be smoke-tested against remote resource repository URLs before a release package flow depends on that path.
- Older checkpoint examples still contain historical pre-reorganization paths and should be treated as archival command records, not current invocation templates.

## Current ActingLab Session Layer Phase C Diagnosis And Initial Recovery

The current Runtime task implements the first bounded parts of Phase C from `TASK-Lab-session-layer.md`: diagnose whether a session is healthy, standby, or on an unexpected page, then recover a session toward a known-good page without adding scheduler, UI, recording, game-task, OCR, or SQLite logic.

Scope:

- Add read-only `monitor --once` for one-shot session diagnosis.
- `monitor --once` reports `healthy`, `standby`, or `unexpected_page`.
- `monitor --once` preserves capture backend attempts and freshness diagnostics when it uses `--capture`.
- `monitor --once` reports whether a maintenance recovery is available and shows the safe recovery command/route/step without clicking.
- Add `session recover --to <page>`, defaulting to `home`.
- Keep real recovery execution gated by `--capture`; `--scene` is accepted only with `--dry-run`.
- Treat standby as a maintenance state that may use `control_points.wake` from the navigation resource.
- Parse navigation `control_points` that use either `point: "x,y"` or `point: [x, y]`.
- Reuse existing PageDetector, recognition pack, capture, navigation graph, destructive overlap checks, and MaaTouch semantic input path.
- Bound recovery with `--max-actions`, defaulting to `3`.

Safety direction:

- `monitor --once` is read-only and never starts MaaTouch.
- `session recover` is maintenance-only and must not perform game progress actions.
- Standby dry-run only reports the wake plan; it does not click.
- Real execution requires a live capture source so stale-frame handling and current-page checks remain in the command path.
- Route recovery uses the same destructive-name and destructive-region safety gates as `navigate`.
- Missing wake control points, missing recovery routes, arrival failures, action-limit excess, and stale/failing captures fail visibly.

Validation status:

- Runtime and the three resource repositories were fetched and confirmed aligned with `origin/main`.
- `cargo test --workspace` passed.
- `cargo fmt --all -- --check` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `git diff --check` passed.
- Diff prohibited-feature scan found no ADB input fallback, `adb shell screencap`, SQLite, OCR, OpenCV, scheduler implementation, recording implementation, fallback, reconnect, retry, or MaaTouch startup additions in `monitor --once`.
- `detect-page --check-pages` passed for Arknights, AzurLane, and BlueArchive resource roots under `ours`.
- Unit tests cover `monitor --once` healthy, standby, and unexpected-page diagnostics.
- BlueArchive JP `127.0.0.1:16481` dry-run `session recover --capture` returned a standby recovery plan using `control_points.wake` at `(300, 2)` and did not start MaaTouch or click.
- BlueArchive JP `127.0.0.1:16481` read-only `monitor --once --capture` returned `standby`, included ADB screencap diagnostics, and reported an available `session recover --to bluearchive/home --capture` wake step without clicking.

Known follow-ups:

- Phase C still needs the background monitor loop, automatic invocation of recovery under scheduler ownership, login resource loop, modal dismissal policy, and scheduler-coordinated self-heal ownership.
- Arknights page anchors are still broad and should be tightened in the resource lane before trusting live recovery decisions that depend on those pages.
- `session recover` should only be executed live after the operator accepts the current page-resource quality and the intended maintenance route.

## Current ActingLab Session Layer Phase B

The current Runtime task implements the Phase B semantic layer from `TASK-Lab-session-layer.md`.

Scope:

- Add `current-page` as the user-facing semantic page-status command, reusing the existing PageDetector path.
- Add `is-visible <target>` for evaluating a visual recognition target without clicking.
- Add `locate <template>` / `locate --template <path>` for full-frame template localization used during calibration.
- Add `tap-target <target>` as a semantic MaaTouch click command that requires the target to pass visual recognition before real execution.
- Add `navigate --to <page>` using `navigation/<game>.<server>.navigation.json`, current-page detection, route planning, navigation-only safety checks, MaaTouch execution, and post-edge arrival polling.
- Keep real semantic click execution gated by `--capture`; `--scene` is allowed for dry-run planning and offline validation only.
- Reuse existing Runtime capture, recognition, page-detector, and MaaTouch modules. No OCR, SQLite, scheduler, UI, recording, self-heal, or game task logic is added.

Safety direction:

- `tap-target` rejects click-only targets because they cannot prove visibility.
- `tap-target` fails visibly if the target does not pass recognition.
- `tap-target` and `navigate` block obviously destructive semantic ids by default and require `--allow-destructive` to bypass the name gate.
- `navigate` only consumes the navigation edge list, not `page_operations` or `destructive_actions`.
- `navigate` blocks navigation edges whose click area overlaps a destructive action on the same source page, or on `any`.
- `navigate --dry-run` exposes the planned route without touching the device.

Validation status:

- Runtime and the three resource repositories were mirrored before work.
- `cargo test --workspace` passed.
- `cargo fmt --all -- --check` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `git diff --check` passed.
- `detect-page --check-pages` passed for Arknights, AzurLane, and BlueArchive resource roots under `ours`.
- Read-only `current-page --capture` smoke:
  - AzurLane JP `127.0.0.1:16385` matched `azurlane/home`.
  - Arknights CN `127.0.0.1:16416` returned `arknights/home`, but several other AK pages also matched the same frame; treat this as a resource discriminator issue.
  - BlueArchive JP `127.0.0.1:16481` returned standby with a visible recovery hint; no wake click was sent.
- AK dry-run navigation from the captured home frame to `arknights/depot` planned `home_to_depot` with no click execution.

Known resource follow-ups:

- Arknights page anchors are currently too broad and can produce multiple simultaneous page matches.
- BlueArchive standby/home detection still needs the later self-heal/wake phase and stronger page anchors.
- Live `tap-target` and live `navigate` clicks should be run only after the user chooses a safe navigation route and accepts the current resource discriminator state.

## Current ActingLab Session Layer Phase A

The current Runtime task implements the Phase A portion of `TASK-Lab-session-layer.md`, informed by `FINDING-AK-game-freeze-2026-06-27.md`.

Scope:

- Add an ActingLab local session daemon lifecycle through `session start`, `session status`, `session stop`, and internal `session daemon`.
- Keep the session layer as a mechanism boundary only. It does not implement the scheduler, UI, OCR, SQLite, route semantics, monitoring, self-heal, or recording phases.
- Add `session instance list|health|reconnect` for instance diagnostics and bounded ADB device-state checks.
- Add `session app launch|stop|restart` with explicit package resolution from `--package`, `instance.<id>.package`, or known game/server defaults.
- Add `session lease acquire|release|preempt|status` as a local lease interface placeholder for later scheduler arbitration.
- Add `key` and `text` direct trusted-manual commands through MaaTouch. No ADB input fallback is introduced.
- Add `capture --require-fresh` and `session capture --require-fresh`, which compare two captured raw-pixel frames and report stale-frame diagnostics. In `auto` mode the fresh probe tries `nemu_ipc`, `droidcast_raw`, then `adb_screencap`.

Validation status:

- Runtime and the three resource repositories were mirrored before work.
- `cargo test --workspace` passed.
- `cargo fmt --all -- --check` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `session start/status/stop` smoke passed with `target\session-smoke16`; no `actinglab` process remained afterward.
- Read-only AK capture on `127.0.0.1:16416` wrote `1280x720` PNG output.
- Read-only AK `capture --require-fresh --fresh-delay-ms 250` succeeded; the two probe frame hashes differed, so stale capture was not suspected in that smoke.
- Scope scan found no ADB input fallback, `adb shell screencap`, SQLite, OCR, OpenCV, or game logic in the touched paths.

## Current Runtime Full-Frame Recognition Hang Fix

The current Runtime task addresses `TASK-runtime-detect-page-hang.md`, where large template searches could hang when a target used `full_frame` or an equivalent large search region.

Implementation direction:

- `crates/recognition` keeps the existing small bounded matching path for normal regions.
- Large searches route through a downsampled coarse pass plus exact local refinement.
- `ccoeff_normed` refinement uses integral-image window statistics and exact row dot-products.
- `ccorr_normed` large searches use the same bounded coarse/refine search path.
- Target evaluation has a wall-clock deadline and returns a fatal recognition error instead of hanging forever.
- `crates/page-detector` has a regression test proving `evaluate_page` evaluates only the requested page, not unrelated pages.

Validation status:

- Resource repositories were mirrored before validation:
  - Arknights: `6a9d0b5`
  - AzurLane: `6c9bba41`
  - BlueArchive: `1b52342`
- BA fixture `C:\合作工作区\ActingCommand\fixtures\ba-detectpage-hang-repro.png` returned in seconds for single-target and detect-page checks.
- Full-frame offline sweep returned without failures:
  - BlueArchive: 21 targets, max about 878 ms
  - AzurLane: 39 targets, max about 798 ms
  - Arknights: 2 targets, max about 739 ms
- Live read-only `detect-page --capture --all` returned without clicking:
  - AzurLane JP `127.0.0.1:16385`: matched home, about 993 ms
  - Arknights CN `127.0.0.1:16416`: matched home, about 12.7 s
  - BlueArchive JP `127.0.0.1:16481`: standby/scene frame did not match home, about 8.2 s, no hang
- `cargo test --workspace` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- Scope scans found no capture hot-path work, ADB input fallback, UI, SQLite, OCR, OpenCV, retry loop, reconnect, or fallback implementation in the touched recognition/page-detector paths.

## Current Runtime ADB Connection Hardening

The current Runtime task addresses the adb version conflict reported in `TASK-runtime-adb-connection-hardening.md`.

Implementation direction:

- `crates/device` exposes a shared adb resolver.
- Resolution order is `ACTINGCOMMAND_ADB_PATH`, MuMu discovery, then user-configured `adb_path`.
- MuMu discovery prefers `nx_main\adb.exe`, then sorted `nx_device\*\shell\adb.exe` candidates.
- Runtime intentionally does not fall back to a bare `adb` on `PATH`.
- `actinglab`, `device-test`, screencap capture, and MaaTouch device verification all use the same device-layer adb path model.
- Device-state recovery is bounded to one `adb connect` attempt when the target allows connect.
- Runtime does not call `adb kill-server`.
- ADB screencap remains `adb exec-out screencap -p` with the existing wall-clock timeout path.

Validation status:

- `actinglab paths` and `actinglab doctor` report `D:\BST\MuMuPlayer\nx_main\adb.exe` from `mumu_discovery`.
- A deliberately invalid `ACTINGCOMMAND_ADB_PATH` reports a visible fatal diagnostic in `actinglab paths` and does not silently fall back.
- AK-only live smoke on `127.0.0.1:16416` captured `1280x720` through both `device-test capture` and `actinglab capture`.
- BA `127.0.0.1:16481` and AzurLane `127.0.0.1:16385` live validation were intentionally skipped because another process was using those emulators.
- `cargo test --workspace` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- Source scan found no `adb shell screencap`, no ADB input fallback, no old `F:\AzurPilot` adb hint, no `adb kill-server`, and no reconnect or retry loop implementation.

## Current ActingLab Direct Touch CLI

The current Runtime task completes the first item from `C:\合作工作区\ActingCommand\HANDOFF-Codex-lab-batch.md`: make the main `actinglab` CLI a unified trusted-manual control entry point for emulator touch and capture.

Scope:

- Add `actinglab tap <x> <y> --instance 127.0.0.1:<port>`.
- Add `actinglab swipe <x1> <y1> <x2> <y2> <duration_ms> --instance 127.0.0.1:<port>`.
- Add `actinglab long-tap <x> <y> <duration_ms> --instance 127.0.0.1:<port>`.
- Keep `actinglab capture --out <png> --instance ...` as the existing screenshot path.
- Reuse `crates/device` `MaaTouchBackend` and the same input backend path as `device-test`.

Safety boundary:

- These commands are direct trusted-manual controls for coordinating agents.
- They do not require LabLease, `navigation_only`, or expect page guards.
- Autonomous execution paths such as `lab run`, `package run`, `operation run`, and `control probe-click` keep their existing safety gates.
- No ADB input fallback, reconnect loop, retry loop, UI, scheduler behavior, new backend, OCR, SQLite, or game logic is added.

Validation status:

- `cargo test -p actingcommand-actinglab` passed with 54 tests.
- `cargo test --workspace` passed.
- `cargo clippy -p actingcommand-actinglab -- -D warnings` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- Touched-file scope scans found no new `adb shell input`, `input tap`, `input swipe`, reconnect, or retry implementation.
- Live device tap/capture validation is reserved for user/agent-side acceptance because this code change is already covered by compile/unit validation and the task names Claude as the true-device acceptance runner.

## Current Lab Packager

The current Runtime task completes the second item from `C:\合作工作区\ActingCommand\HANDOFF-Codex-lab-batch.md`: bring the resource Operation Bundle converter and Lab package producer into Rust-side ActingLab.

Implemented commands:

- `actinglab resource convert --repo <repo> [--game <g>] [--server <s>] [--locale <l>] [--dry-run]`
- `actinglab lab validate --zip <pkg.zip>`
- `actinglab package build-task --repo <repo> --task <task-id> --out <pkg.zip> [--game <g>] [--server <s>] [--from-remote <git-url>]`
- `actinglab package build-pack --repo <repo> --out <pkg.zip> [--entry-task <task-id>] [--split-dir <dir>] [--from-remote <git-url>]`

Package-building direction:

- Production package commands are offline data commands unless `--from-remote` is explicitly provided.
- `--from-remote` shallow-clones into a temporary directory and removes it after package validation.
- Packages are built as Lab-1y input zips with root `control.json` and a self-contained `resources/` tree.
- Package writes go through a temporary zip and `lab validate` before replacing the requested output path.
- `build-task` defaults to a single-entry-task closure. `--include-recovery` may include `return_home` when present, but it does not change the entry task.
- `build-pack --split-dir` validates split outputs in a temporary directory before moving them into the requested directory, so a failing task does not silently leave a new partial split set.

Confirmed Lab run route model:

- `lab run` currently executes the selected entry task's own `operation_bundle.operations`.
- It chooses an operation whose `from` anchor matches the current detected page, then verifies that operation's `to` page or `verify_template`.
- It does not perform cross-task routing over the generated navigation graph.
- Therefore the default `build-task` closure is the selected task bundle itself, plus only explicitly requested recovery data.

Validation status:

- Converter parity passed for Arknights and BlueArchive across pack, pages, navigation, index, and primitives after normalizing only `generated_by`.
- Converter parity passed for AzurLane pages, navigation, index, and primitives after normalizing only `generated_by`; AzurLane pack remains owned by its separate Python template-cropping step.
- `package build-task` produced and validated a real Arknights `open_terminal` package.
- `package build-pack` produced and validated a real Arknights full package with `entry-task=open_terminal`.
- `--from-remote` was smoke-tested with a local Git resource repository path as the clone source; the temporary clone directory was removed.
- Real `build-pack --split-dir` against current Arknights/BlueArchive resource data fails loudly on unresolved `0,0` coordinates, which is expected under the current no-placeholder execution rule. The split implementation itself is covered by a clean fixture test.

Out of scope:

- No UI, SQLite, OCR, scheduler implementation, capture hot-path rollback, ADB input fallback, reconnect loop, retry loop, game logic, or live emulator operation was added.
- Resource-repository deletion of Python converters remains a separate resource-lane step after downstream acceptance.

## Current Round2 Regression Close-Out

The current Runtime task fixes regressions introduced or revealed by the Lab-1z Round2 stability batch.

Scope:

- Fix RR-01, RR-02, RR-03, and RR-04 from `C:\合作工作区\ActingCommand\FIX-round2-regressions.md`.
- Do not implement Nemu IPC helper-process isolation in this task.
- Do not perform live gameplay package reruns as part of this code fix.
- Do not add UI, OCR, SQLite, scheduler behavior, game logic, new capture backends, ADB input fallback, reconnect loops, or retry loops.

Fixed behavior:

- Segment write failures now carry any per-frame encoding failures collected earlier in the same spill attempt. Even if segment zip creation/write/finish fails globally, frames that could not encode are still marked `spill_failed` and are not re-encoded forever.
- Successful Lab runs reject `--out` paths inside the generated run directory, report `run_dir_cleaned: true`, and clean the run directory only after the output archive has been written outside that directory.
- Failed Lab runs preserve the run directory for diagnostics.
- Tier3 pause checkpoints now include diagnostic step context: current step index, current step id, current operation id, current phase, expected page, and last matched page. This remains diagnostic/future-resume metadata; it does not change the current synchronous graceful-failure Tier3 behavior.
- `NemuIpcBackend` no longer explicitly shuts down its worker in its own `Drop`; `NemuIpcWorker::Drop` owns shutdown exactly once.

Validation status:

- Targeted `cargo test -p actingcommand-actinglab` passed with 51 tests after adding regression coverage.
- Targeted `cargo test -p actingcommand-device` passed with 33 tests.
- Full validation passed: `cargo test --workspace`, `cargo clippy --workspace -- -D warnings`, `cargo fmt --all -- --check`, `git diff --check`, device-layer prohibited scan, and Round3 scope scan.

## Current Lab-1z Round2 Stability Close-Out

The current Runtime task closes the Lab-1z Round2 stability batch in dependency order:

1. P2.2 device-layer stability.
2. Lab-1y execution stability.
3. Lab-1z frame-store accounting and spill semantics.
4. P1g CLI/package robustness.
5. P2.3 release benchmark non-regression.

This task does not add UI, OCR, SQLite, scheduler behavior, game logic, reconnect/retry loops, new capture backends, ADB input fallback, scrcpy, minicap, or new P2.3 capture hot-path behavior.

Round2 fixed behavior:

- ADB timeout handling now returns a fatal error immediately if child termination fails, instead of joining pipe-reader threads that may block forever.
- DroidCast raw capture cleans stale children before spawning and reads HTTP responses with a wall-clock deadline plus a bounded maximum response size.
- Nemu IPC capture now runs behind a backend-scoped worker with request timeouts and poison-on-timeout behavior. The worker probes display dimensions before the unsafe capture write and resizes its reusable buffer before capture.
- MaaTouch caps long gesture durations and preserves stderr reader errors in diagnostics.
- Lab input unpacking and package validation enforce per-entry and total decompressed-size limits.
- Dangerous zip entries are skipped before disk write, then reported as invalid input.
- Output zip creation removes partial archives on write failure.
- `git_commit()` is noninteractive and bounded by timeout.
- `finish()` cleans the per-run directory after successful or failed finalization.
- `summary.json` records `partial_output`.
- `backpressure_paused` reports Tier3 as `synchronous_graceful_failure`; the former Tier3 pause-timeout control is no longer part of the active schema/CLI.
- Frame-store resident accounting now replaces retained estimates atomically and maintains total plus payload, metadata, thumbnail, and encoder subcounters.
- Dropped entries no longer remain in resident-byte accounting.
- Spilled retained frames keep their thumbnails for later dedupe.
- Global spill-unavailable failures produce a global warning/backoff event without permanently poisoning every frame.
- Per-frame spill failures are localized to the failed frame.
- Admission-spill failure no longer leaves encoder workspace reserve counted as permanent resident pressure.
- CLI manifest hash paths are validated before use and unsafe traversal strings are not echoed.
- Unknown list kinds now return usage errors instead of panicking.
- Run listing keeps directory-entry warnings visible in JSON output.

Round2 validation status:

- `cargo test --workspace` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- `cargo test -p actingcommand-actinglab` passed with 47 tests.
- `cargo test -p actingcommand-device` passed with 33 tests.
- Device-layer prohibited scan over `crates/device` found no `adb shell screencap`, `adb shell input`, `fallback`, `reconnect`, `println!`, or `eprintln!`.
- Release benchmark on Arknights `127.0.0.1:16416` passed. `nemu_ipc` capture-only best/median/p90 was `4/4/6ms`; end-to-end capture plus artifact PNG best/median/p90 was `11/11/13ms`.

Known residuals:

- A Rust thread blocked inside a stuck Nemu FFI call cannot be force-killed in-process. The current worker is poisoned and abandoned on timeout; true hard-kill isolation remains a later helper-process milestone if needed.
- The aggregate screenshot benchmark row may still be dominated by slower backends, but the backend table confirms the `nemu_ipc` release path remains healthy.
- Live gameplay package execution was not rerun in this pass; the live work here was the release benchmark only.

## Current P2.3 Capture Pipeline Refactor

The current Runtime task removes PNG encoding from the capture hot path and keeps the screenshot pipeline split into explicit stages.

P2.3 capture direction:

- `Frame` is a raw pixel frame with width, height, pixels, pixel format, capture time, backend name, and optional `original_png`.
- `Frame::from_pixels` does not encode PNG during `capture()`.
- `Frame::from_png` decodes pixels once and preserves the source PNG in `original_png`, so ADB screencap artifacts can be written without decode-and-reencode.
- Artifact writes use `Frame::png_for_artifact()`: original PNG when available, otherwise fast PNG encoding.
- Fast PNG encoding uses `CompressionType::Fast` and `FilterType::NoFilter`.
- `Scene::from_rgb8`, `Scene::from_rgba8`, and `Scene::from_pixels` let Runtime recognition consume captured pixels directly.
- `device-test`, `actinglab capture`, `actinglab` read-only recognition, Lab-1y capture loops, probe-run frame capture, and `CaptureStore` now use the raw-frame/artifact boundary.
- `NemuIpcBackend` probes resolution at backend initialization and reuses the cached dimensions for captures.
- `device-test benchmark` reports capture-only, encode-only, and end-to-end capture-plus-artifact timing for each capture backend.

P2.3 validation status:

- Arknights `127.0.0.1:16416` benchmark with all three backends succeeded.
- Observed medians in this pass:
  - `adb_screencap`: capture-only `632ms`, encode-only `142ms`, end-to-end `632ms`.
  - `droidcast_raw`: capture-only `376ms`, encode-only `118ms`, end-to-end `482ms`.
  - `nemu_ipc`: capture-only `26ms`, encode-only `136ms`, end-to-end `164ms`.
- Nemu capture-only is now in the expected tens-of-milliseconds range.

Known residuals:

- The Nemu IPC DLL still prints external diagnostic lines to process stdout. This is not a screenshot pipeline blocker, but strict JSON consumers still need a later stdout-isolation task.
- `encode-only` still costs roughly `118-142ms` for `1280x720` frames with the current fast PNG path; later frame-store work may avoid PNG encoding entirely where only in-memory recognition is needed.

## Current P2.2 / Lab-1y Capture Backend And Trusted Execution Round

The current Runtime task upgrades capture and one-shot Lab execution without adding UI, OCR, SQLite, scheduler behavior, or game logic.

P2.2 capture direction:

- `crates/device` owns a single synchronous `CaptureBackend` trait.
- `Frame` now carries actual dimensions, raw RGB/RGBA pixels, PNG artifact bytes, capture time, pixel format, and backend name.
- `adb_screencap` remains the always-available baseline using `adb exec-out screencap -p`.
- `droidcast_raw` is implemented behind an external APK boundary and requires `ACTINGCOMMAND_DROIDCAST_RAW_APK`.
- `nemu_ipc` is implemented behind a Windows MuMu external DLL boundary and requires local MuMu discovery or `ACTINGCOMMAND_NEMU_FOLDER` / `ACTINGCOMMAND_NEMU_IPC_DLL`.
- No DroidCast APK or MuMu/Nemu DLL is committed to the repository.
- Explicit backend selection fails loudly if unavailable.
- `auto` may downgrade to the next backend, but the failed attempts and chosen backend must be recorded in diagnostics.
- `nemu_ipc` passes the MuMu folder to `nemu_connect` as a NUL-terminated UTF-16 path.
- `droidcast_raw` requests the orientation-adjusted endpoint size but decodes MuMu raw frames as the natural buffer before orienting them into the Runtime display coordinate space.
- `auto` probes each candidate with one capture; a connection, initialization, or first-capture failure marks that backend unavailable and continues to the next candidate.

Lab-1y execution direction:

- `control.json` uses `Lab-1y.control.v1`.
- Supported `execution_mode` values are `navigable_route`, `recognize_only`, and `in_page_guard`.
- `trusted_execution` and `producer` are provenance fields, not semantic blockers.
- `capture_backend` can be specified in control data, but CLI `--capture-backend` has higher priority.
- Lab run acquires a local per-instance lock before device execution.
- Output remains limited to `logs/` and `screenshots/`.
- Summary and diagnostics include capture backend, backend attempts, capture interval stats, capture duration stats, action duration stats, and loop lag stats.

Current P2.2 repair validation status:

- Arknights `127.0.0.1:16416` explicit `nemu_ipc` capture succeeded at `1280x720`.
- Arknights `127.0.0.1:16416` explicit `droidcast_raw` capture succeeded at `1280x720`, and the generated PNG was visually readable.
- `auto` selected `nemu_ipc` when available, downgraded to `droidcast_raw` when the Nemu DLL path was intentionally invalid, and downgraded to `adb_screencap` when both Nemu and DroidCast paths were intentionally invalid.
- `actinglab lab run --capture-backend droidcast_raw` completed the existing safe `open_terminal` package with `executed_step_count=2` and `screenshot_count=3`.
- 16416 benchmark medians measured in this pass: `adb_screencap` about `655ms`, `droidcast_raw` about `676ms`, and `nemu_ipc` about `515ms`. Do not claim a 300ms capture target from this evidence.

Known residuals:

- The Nemu IPC DLL writes its own diagnostic lines to process stdout before JSON output. Screenshot functionally succeeds, but strict JSON consumers need a later stdout isolation fix.
- Current Arknights page resources still have broad page-template false positives on the home/terminal-style frame. This is resource data quality work, not a P2.2 screenshot backend blocker.

## Current ActingLab read-only recognition round

The current Runtime task makes `actinglab` read-only device/resource checks usable without requiring a resident Runtime service for the narrow commands that are already read-only:

- `devices`
- `capture`
- `detect-page`
- `recognize`

Scope boundaries:

- no click execution;
- no package-run execution;
- no scheduler implementation;
- no monitor stream;
- no UI;
- no SQLite;
- no OCR;
- no game logic.

Resource-root recognition selection is supported for generated resource repositories:

- `--resource-root <repo> --game arknights` resolves `recognition/arknights.cn.pack.json` and `recognition/arknights.cn.pages.json`;
- `--resource-root <repo> --game azurlane` resolves `recognition/azurlane.jp.pack.json` and `recognition/azurlane.jp.pages.json`;
- `--resource-root <repo> --game bluearchive` resolves `recognition/bluearchive.jp.pack.json` and `recognition/bluearchive.jp.pages.json`;
- explicit `--pack`, `--pack-root`, and `--pages` remain supported for compatibility.

Live retest after game restart showed:

- AK `127.0.0.1:16416` matched `arknights/home`, with `page/home` recognize score `0.999885`;
- AzurLane `127.0.0.1:16384` captured the visible main/home screen, but `azurlane/home` failed because stale `page/home` color gating expected `107,164,233` and observed `223,225,224`;
- AzurLane `azurlane/campaign` matched on that same home screen, so it should be treated as an entry-anchor match, not true page-state evidence.

Next steps:

1. Refresh AzurLane `page/home` live anchor/color gate in `ActingCommand-Resources-AzurLane`.
2. Tighten AzurLane entry-anchor page definitions so visible home-screen buttons do not count as true target pages.
3. Keep Runtime `actinglab` read-only commands thin; deeper package-run, monitor, scheduler, and click paths still require separate Runtime service/LabLease milestones.

## Previous ActingLab Lab-1X Trusted One-Shot Package Execution Round

The current Runtime task promotes `actinglab lab run` into the standard trusted one-shot execution entry:

```text
input.zip -> actinglab lab run -> output.zip
```

Scope:

- accept a trusted Lab-1X input zip with `control.json` and `resources/`;
- reject unsafe paths, executable/script entries, malformed control data, missing resources, unresolved or out-of-bounds coordinates, unknown operation types, capture failures, input failures, and output write failures loudly;
- load Operation Bundle v0.3 plus generated recognition pack/page data from the zip;
- use existing Runtime device primitives (`ScreencapBackend` and `MaaTouchBackend`) for capture and input;
- write output zips containing only `logs/` and `screenshots/`;
- record actual screenshot intervals instead of claiming the requested interval was achieved;
- keep semantic safety decisions outside Lab because the package is trusted by its authoring pipeline.

Non-goals:

- no UI;
- no SQLite;
- no OCR;
- no resident Runtime service;
- no scheduler implementation;
- no new screenshot backend;
- no raw ADB tap fallback;
- no package script execution.

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
- control command-submission latency through `MaaTouchBackend` reset operations.
- recommended polling and minimum operation intervals.

Control benchmark output is explicitly labeled as `command_submission_only`.
MaaTouch reset currently writes and flushes commands without a device acknowledgement, so the benchmark must not present that number as a device round trip or derive a minimum operation interval from it.

`device-test runner` packages recognition, capture, probe-run, and MaaTouch control into a one-shot profile-driven unit:

- no scheduler.
- no background resident process.
- no SQLite.
- independent run directories per port/process.
- one failed probe is recorded without hiding the failure.

Runner multi-open validation currently uses the BA JP smoke profile. Non-BA devices are expected to stop at page guard with `click_count=0`; the BA device may execute only the verified `NavigationOnly` home-to-task-and-back route.

## P6d/P6e-half resource-independent close-out

This phase completed the resource-independent half only:

- `ProbeAction::Click` steps must declare a non-empty `page_id` at `ProbeDecisionLoop::new` time.
- MaaTouch is recorded as Apache-2.0 and the Apache-2.0 license text is kept beside the included binary.
- `MaaTouchConfig::default` resolves the default local tool path from the executable location, so `device-test` can run from a non-repository current working directory.
- Benchmark output no longer reports MaaTouch reset writes as a true control round trip.
- No FreeClaim preflight, ConsumeRegeneratingResource preflight, real reward claim, AP/oil/sanity consumption, broad NavigationOnly巡检, SQLite, UI, scheduler, OCR, OpenCV, or resource repository mutation was added.

The BA regression frame set was collected under `target/regression-frames/bluearchive/jp`, but it is blocked:

- `bluearchive/home` positive samples are available.
- Idle captures after the wait still matched `bluearchive/home`, so they were not suitable hidden/idle negatives in this run.
- The temporary `PAGE_TASK_CENTER` full-frame bridge matched returned-home/home frames and is not discriminative enough.
- A manual MaaTouch tap at `navigation/home_to_task` stayed on the home screen during this run.

Treat BA task-center regression as blocked until the BlueArchive resource repository supplies corrected navigation and arrival-anchor data.

Resource-dependent P6e work remains deferred until the resource Operation Bundle provides reviewed reward references, cost references, resource policies, and destructive-zone data.

## ActingLab-P1 Runtime Embedded Lab

ActingLab-P1 is now a Runtime-embedded developer tooling and debug lab, not a standalone Python runtime/debug program.

ActingLab must use the same implementation language and module system as the Runtime mainline. For this repository, that means Rust.

Runtime-embedded ActingLab must reuse existing Runtime modules instead of duplicating them:

- capture backend
- recognition primitives
- recognition pack evaluation
- page detection
- input backend and click safety
- scheduler gate/state interfaces
- poll loops
- journal and frame-store formats

Runtime-side Python Lab implementations that directly screenshot, recognize, click, poll, or write device-control journals are not allowed in this repository.

The previous Python runtime prototype was already removed from the Rust mainline by commit `557831c` (`Move Python and Go legacy runtime out of Rust mainline`). The current Runtime repository contains no tracked `.py` files.

Resource-repository Python scripts remain allowed when they are offline tooling only. Examples include importers, upstream drift guards, and operation converters. Those tools must not directly control devices or become Runtime-side Lab implementations.

### Lab modes

ActingLab-P1 introduces these Runtime-owned lab modes:

- `exclusive_drain`: request a scoped LabLease, stop new scheduler work for selected or affected instances, wait for the current scoped task to finish or reach a safe checkpoint, then acquire exclusive control and defer upcoming scoped tasks until release.
- `passive_mirror`: observe Runtime frames, recognition results, scheduler state, and events without pausing the scheduler and without click permission.
- `scheduler_noop`: let selected scheduler flows tick without executing device actions, recording `would_run` evidence instead.

`exclusive_pause` is not the design target. P1 must not hard-stop a running task by default.

### LabLease contract

Lab clicks require a `LabLease`.

A LabLease must be exclusive with scheduler device execution on the same instance. If the scheduler is currently executing device actions on a scoped instance, ActingLab cannot click until the lease is acquired.

Initial lease state model:

- `idle`
- `lab_requested`
- `draining_current_task`
- `lease_acquired`
- `lab_active`
- `releasing`
- `scheduler_restored`
- `failed`

If lease acquisition times out, ActingLab must fail visibly and must not click. If scheduler restore fails, the affected instances must remain blocked or require manual review, and the failure must be recorded as fatal evidence.

### Frame stream

P1 frame/video output is a frame-sequence evidence lane, not real-time video encoding.

Minimum outputs:

- `frames/000001.png`
- `events.jsonl`
- `summary.json`
- `recognition.jsonl`

Frame capture must use the Runtime capture backend. Recognition results must use Runtime recognition modules.

### P1a/P1b Rust skeleton

The first Runtime-embedded ActingLab code lives in `actingcommand-runtime-core`.

Implemented pure state and decision contracts:

- `LabMode`
- `InstanceScope`
- `DeferPolicy`
- `LabClickPolicy`
- `LabLeaseState`
- `LabLeaseRequest`
- `SchedulerTaskState`
- `SchedulerInstanceSnapshot`
- `SchedulerGateSnapshot`
- `SchedulerGate::evaluate`

This skeleton deliberately does not start devices, capture frames, run recognition, execute clicks, write journals, or mutate scheduler state. It only evaluates a scheduler snapshot and lease request into a visible decision:

- `exclusive_drain` acquires an idle scoped instance and allows only navigation-only clicks.
- `exclusive_drain` enters `DrainingCurrentTask` when a scoped instance is running, and cannot click while draining.
- `passive_mirror` can observe scoped instances without deferring tasks and without click permission.
- `scheduler_noop` can acquire scoped idle instances, defer scoped tasks, and still cannot click.
- manual-review-blocked instances fail lease acquisition visibly.

The next Runtime milestone should connect this contract to real scheduler state without changing the no-duplicate-runtime-module rule.

### P1g global CLI contract shell

P1g adds `apps/actinglab` as the user-facing global CLI entry.

The CLI owns:

- command parsing without adding a new UI framework or CLI framework dependency;
- unified `schema_version = 0.2` JSON envelope for success and failure output;
- stable exit-code mapping:
  - `0` ok;
  - `2` usage or validation failure;
  - `3` safety blocked;
  - `4` device or instance issue;
  - `5` runtime not running;
  - `6` reserved or not implemented;
- `config get/set` for user-level `adb_path`, `runtime_endpoint`, `run_root`, `resource_root`, and `instance.<id>.serial|game|server`;
- `doctor`, `paths`, `schema`, `list`, and `capabilities`;
- capability reporting for command `needs` and recognition match-metric policy;
- package zip validation and inspection with zip-slip protection, executable/script-entry rejection, and declared hash verification;
- structured scheduler/lab/package-run safety stubs that do not fake successful execution.

The CLI also includes Windows launchers under `scripts/actinglab`:

- `actinglab.cmd`;
- `actinglab.ps1`;
- `install-user-path.ps1`;
- `uninstall-user-path.ps1`.

The installer scripts only modify the user-level PATH and do not require administrator permissions.

Current P1g limitations are intentional:

- no full scheduler implementation;
- no resident Runtime service attach protocol beyond endpoint probing;
- no real `package run` operation execution;
- no active monitor frame stream;
- no UI;
- no SQLite;
- no OCR;
- no game logic.

Commands that would require missing Runtime services fail visibly with stable envelope errors instead of returning fake success.

### Lab-1y interpreter namespace normalization + synchronous capture cadence fix

This phase fixes the Lab-1y interpreter path that maps detected page ids to operation anchors.

The interpreter now treats detector page ids such as `arknights/home` as the namespaced form of operation anchors such as `home`. The normalization is applied consistently to:

- initial page confirmation;
- operation `from` selection;
- `target_page` stop checks;
- operation `to` arrival polling;
- after-state writeback.

The fix uses the known `{game}/` prefix and detector page existence checks instead of blindly splitting page ids on `/`.

Lab-1y route execution also constrains page evaluation to the relevant operation pages for the current task, so waiting and arrival polling continue to produce screenshots and recognition records without evaluating every page in a large resource pack.

`control.json` remains the authority for `entry_task_id`. If `resources/manifest.json` also declares `entry_task_id`, it must match `control.json`; mismatches fail loudly.

`to: null` operation results are no longer treated as verified success when `verify_template` is also null. Such operations are recorded as `executed_unverified`; `to: null` with `verify_template` still requires template verification.

This phase does not claim that TaskRoute, navigation models, or resource bundles are fully verified. Live validation only covered the `open_terminal` Arknights package path enough to confirm the interpreter no longer fails on namespaced page ids.

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
- No Runtime-side Python ActingLab/Lab implementation that directly controls devices, captures frames, runs recognition, polls pages, or writes device-control journals.
- ActingLab Runtime work must be Rust and must reuse Runtime modules instead of duplicating capture, recognition, page detection, click execution, poll, scheduler-state, or journal logic.

## Current BA Resource Control Refinement Round

Runtime/resource compatibility completed for the BA control-data refinement task:

- BA generated packs can opt into `match_metric: "ccoeff_normed"` while CCORR remains the default for existing packs.
- Runtime accepts generated `0.3` recognition packs/pages and `"full_frame"` template regions.
- Probe-run supports navigation drag actions through MaaTouch swipe and journals actual from/to/duration.
- Probe-run records initial/final and last before/after pages in checkpoint/summary output.
- BA resource bundles now generate `recognition/bluearchive.jp.pack.json` with CCOEFF defaults.

Remaining BA data work is still resource/live-verification work, not Runtime architecture work:

- replace full-frame BA anchors with tight live CCOEFF ROIs,
- resolve sentinel coordinates,
- add cafe collect,
- add growth/progression bundles,
- regenerate artifacts and run live ADB validation.

## Next steps

1. Keep Lab deduplication, frame-store watermarks, and retention policy out of P2.3 and handle them in the separate Lab-1z branch/task.
2. Decide whether to add stdout isolation for the external Nemu IPC DLL diagnostics before any strict JSON consumer depends on Nemu-backed capture output.
3. Connect `actinglab status/devices/lab/monitor` to a real resident Runtime endpoint instead of endpoint probing.
4. Move active `capture`, `detect-page`, `recognize`, `operation dry-run`, and `package run` behind a Runtime service boundary while keeping the CLI as a thin user entry.
5. Implement real package-run operation execution only after LabLease, navigation-only, and expect-after Runtime gates are connected.
6. Continue the BA resource control-refinement task with live CCOEFF ROI capture and sentinel-coordinate resolution.
7. Connect ActingLab `SchedulerGate` to real Runtime scheduler state in a separate scoped milestone.
8. Add Runtime-owned journal/frame-stream contracts for ActingLab passive-mirror evidence output.
9. Keep `device-test lab ...` as a thin wrapper only if used; actual lab logic must live in Runtime-owned Rust modules.
10. Preserve resource-repository offline Python tools as offline importer/drift/converter code only.
11. Fix BlueArchive `home_to_task` navigation and task-center arrival-anchor resource data before treating BA task regression as green.
12. Upgrade BA arrival anchors from the temporary `device-test` direct bridge into recognition-pack targets with positive and negative samples.
13. Add resource definitions for AzurLane mission/commission pages before AzurLane probes.
14. Add Arknights operator/menu navigation targets before Arknights probes.
15. Improve Arknights page templates/guards so `home`, `terminal`, and related menu pages do not all match the same frame.
16. Resume FreeClaim and ConsumeRegeneratingResource preflight only after the resource Operation Bundle lands reviewed reward/cost/resource-policy data.
17. Define Runtime API contracts for UI integration in a separate milestone.
18. Define capture metadata and SQLite schema in a separate scoped milestone.
19. Keep `CHECKPOINT.md` updated with every completed Runtime task.
