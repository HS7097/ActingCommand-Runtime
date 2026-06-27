# CHECKPOINT.md

## 2026-06-27 ActingLab daemon-preferred read-only routing

### Current status

- Added shared daemon-preferred routing for read-only/diagnostic ActingLab CLI entries.
- `--via-daemon` still forces resident daemon routing.
- New `--local` flag forces local handling for diagnostic/read-only entries.
- When session info exists, `status`, `devices`, `capture`, `capture diagnose`, `recognize`, `detect-page`, `current-page`, `is-visible`, `locate`, `stream`, `session status`, and `session journal` now prefer the resident daemon request queue by default.
- When session info is absent, existing local/offline behavior remains available.
- Control commands such as `tap`, `swipe`, `long-tap`, `key`, `text`, `tap-target`, `navigate`, and recovery remain lease/control-request gated only when explicitly routed through the daemon.
- Client-only `--local` is stripped from daemon request payload args.
- No scheduler implementation, UI, SQLite, OCR/OpenCV, game logic, ADB input fallback, capture hot-path algorithm change, reconnect loop, retry loop, or silent fallback was added.

### Resource mirrors used

- Runtime baseline before this task: `c61755c06a1bf49f357ee24eae78eaa033e4f376`.
- Resource repositories were not modified or used by this implementation step.

### Files changed

- `apps/actinglab/src/main.rs`
- `PLANS.md`
- `CHECKPOINT.md`

### Commands run

- `git fetch --prune --tags`
- `git status --short --branch`
- `git rev-parse HEAD`
- `git rev-parse origin/main`
- Re-read `C:\合作工作区\ActingCommand\TASK-Lab-session-layer.md`.
- Re-read `C:\合作工作区\ActingCommand\FINDING-AK-game-freeze-2026-06-27.md`.
- Re-read Runtime-local `AGENTS.md`, `PLANS.md`, and `CHECKPOINT.md`.
- Re-read local `rust-patterns` and `rust-testing` skill instructions.
- `cargo fmt --all`
- `cargo test -p actingcommand-actinglab status_prefers_daemon_when_session_info_exists -- --nocapture`
- `cargo test -p actingcommand-actinglab devices_prefers_daemon_when_session_info_exists -- --nocapture`
- `cargo test -p actingcommand-actinglab session_status_local_bypasses_daemon_preference -- --nocapture`
- `cargo test -p actingcommand-actinglab session_request_payload_strips_client_only_flags -- --nocapture`
- `cargo test -p actingcommand-actinglab session_status_via_daemon_without_daemon_is_runtime_error -- --nocapture`
- `cargo test -p actingcommand-actinglab session_journal_via_daemon_without_daemon_is_runtime_error -- --nocapture`
- `cargo test -p actingcommand-actinglab direct_touch_via_daemon_accepts_lease_flags_before_daemon_lookup -- --nocapture`
- `cargo fmt --all -- --check`
- `git diff --check`
- Source-only added-code prohibited-feature scan over `apps/actinglab/src/main.rs` for fallback, reconnect/retry loops, direct input fallback, ADB shell input/screencap, SQLite, OCR/OpenCV, and unreviewed trusted-channel implementation.
- `cargo clippy --workspace -- -D warnings`
- `cargo test --workspace`

### Test results

- `cargo test -p actingcommand-actinglab status_prefers_daemon_when_session_info_exists -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab devices_prefers_daemon_when_session_info_exists -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab session_status_local_bypasses_daemon_preference -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab session_request_payload_strips_client_only_flags -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab session_status_via_daemon_without_daemon_is_runtime_error -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab session_journal_via_daemon_without_daemon_is_runtime_error -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab direct_touch_via_daemon_accepts_lease_flags_before_daemon_lookup -- --nocapture` passed with `1` test.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- Source-only added-code prohibited-feature scan returned `NO_PROHIBITED_CODE_ADDED_LINES`.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed.

### Current blocker

- No blocker for the local implementation.
- Full Session Layer remains incomplete: default daemon-preferred control routing with operator lease UX, trusted UI/API diagnostics exposure, actual trusted interactive streaming, daemon transport/API for long-lived frame streams, live prepared-emulator validation, real scheduler lease arbitration integration, recording ownership review, and full scheduler/UI integration remain future work.

### Next step

1. Commit and push this Runtime milestone with checkpoint tag `checkpoint/20260627-daemon-preferred-readonly`.
2. Continue Session Layer follow-ups: default daemon-preferred control routing with operator lease UX, trusted UI/API diagnostics, scheduler lease arbitration integration, trusted interactive frame/input channel, long-lived stream transport/API, live prepared-emulator validation, UI/API review surfaces, recording ownership review, and scheduler/UI integration.

## 2026-06-27 ActingLab session diagnostics daemon routing

### Current status

- Added `session status --via-daemon` as a daemon-routed session status diagnostic request.
- Added `session journal --via-daemon` as a daemon-routed session request-journal diagnostic request.
- Bare `session status` and `session journal` keep their existing local/offline state-reader behavior.
- Routed session diagnostics reuse existing daemon-side `status` and `journal` request handlers instead of adding duplicate diagnostics implementations.
- Missing daemon state still fails visibly with `runtime_not_running`.
- No device control, capture/input path, scheduler implementation, UI, SQLite, OCR/OpenCV, game logic, ADB input fallback, capture hot-path algorithm change, reconnect loop, retry loop, or silent fallback was added.

### Resource mirrors used

- Runtime baseline before this task: `f0722a8df554a6f93d2fcd7cfd85d037bab5cde8`.
- Resource repositories were not modified or used by this implementation step.

### Files changed

- `apps/actinglab/src/main.rs`
- `PLANS.md`
- `CHECKPOINT.md`

### Commands run

- `git fetch --prune --tags`
- `git status --short --branch`
- `git rev-parse HEAD`
- `git rev-parse origin/main`
- Re-read `C:\合作工作区\ActingCommand\TASK-Lab-session-layer.md`.
- Re-read `C:\合作工作区\ActingCommand\FINDING-AK-game-freeze-2026-06-27.md`.
- Re-read Runtime-local `AGENTS.md`, `PLANS.md`, and `CHECKPOINT.md`.
- Re-read local `rust-patterns` and `rust-testing` skill instructions.
- `cargo fmt --all`
- `cargo test -p actingcommand-actinglab session_status_via_daemon_without_daemon_is_runtime_error -- --nocapture`
- `cargo test -p actingcommand-actinglab session_journal_via_daemon_without_daemon_is_runtime_error -- --nocapture`
- `cargo test -p actingcommand-actinglab session_status_without_daemon_is_offline_ok -- --nocapture`
- `cargo fmt --all -- --check`
- `git diff --check`
- Source-only added-code prohibited-feature scan over `apps/actinglab/src/main.rs` for fallback, reconnect/retry loops, direct input fallback, ADB shell input/screencap, SQLite, OCR/OpenCV, and unreviewed trusted-channel implementation.
- `cargo clippy --workspace -- -D warnings`
- `cargo test --workspace`

### Test results

- `cargo test -p actingcommand-actinglab session_status_via_daemon_without_daemon_is_runtime_error -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab session_journal_via_daemon_without_daemon_is_runtime_error -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab session_status_without_daemon_is_offline_ok -- --nocapture` passed with `1` test.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- Source-only added-code prohibited-feature scan returned `NO_PROHIBITED_CODE_ADDED_LINES`.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed.

### Current blocker

- No blocker for the local implementation.
- Full Session Layer remains incomplete: trusted UI/API diagnostics exposure, actual trusted interactive streaming, daemon transport/API for long-lived frame streams, live prepared-emulator validation, real scheduler lease arbitration integration, recording ownership review, default daemon-preferred CLI routing policy, and full scheduler/UI integration remain future work.

### Next step

1. Commit and push this Runtime milestone with checkpoint tag `checkpoint/20260627-daemon-session-diagnostics`.
2. Continue Session Layer follow-ups: trusted UI/API diagnostics, scheduler lease arbitration integration, trusted interactive frame/input channel, long-lived stream transport/API, live prepared-emulator validation, UI/API review surfaces, recording ownership review, default daemon-preferred CLI routing policy, and scheduler/UI integration.

## 2026-06-27 ActingLab top-level daemon-routed status entry

### Current status

- Added `status --via-daemon` as a daemon-routed top-level status diagnostic request.
- Bare `status` keeps its existing local runtime-info probe behavior.
- `status --via-daemon --diagnostics` reuses the existing daemon-side `status` request path instead of adding a duplicate diagnostics implementation.
- Missing daemon state still fails visibly with `runtime_not_running`.
- No device control, capture/input path, scheduler implementation, UI, SQLite, OCR/OpenCV, game logic, ADB input fallback, capture hot-path algorithm change, reconnect loop, retry loop, or silent fallback was added.

### Resource mirrors used

- Runtime baseline before this task: `c0fec6e293a9e288a85fdefab64ec128d7b213d4`.
- Resource repositories were not modified or used by this implementation step.

### Files changed

- `apps/actinglab/src/main.rs`
- `PLANS.md`
- `CHECKPOINT.md`

### Commands run

- `git fetch --prune --tags`
- `git status --short --branch`
- `git rev-parse HEAD`
- `git rev-parse origin/main`
- `cargo fmt --all`
- `cargo test -p actingcommand-actinglab status_via_daemon_without_daemon_is_runtime_error -- --nocapture`
- `cargo test -p actingcommand-actinglab session_request_status_without_daemon_is_runtime_error -- --nocapture`
- `cargo fmt --all -- --check`
- `git diff --check`
- Source-only added-code prohibited-feature scan over `apps/actinglab/src/main.rs` for fallback, reconnect/retry loops, direct input fallback, ADB shell input/screencap, SQLite, OCR/OpenCV, and unreviewed trusted-channel implementation.
- `cargo clippy --workspace -- -D warnings`
- `cargo test --workspace`

### Test results

- `cargo test -p actingcommand-actinglab status_via_daemon_without_daemon_is_runtime_error -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab session_request_status_without_daemon_is_runtime_error -- --nocapture` passed with `1` test.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- Source-only added-code prohibited-feature scan returned `NO_PROHIBITED_CODE_ADDED_LINES`.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed.

### Current blocker

- No blocker for the top-level daemon-routed status entry milestone.
- Full Session Layer remains incomplete: trusted UI/API diagnostics exposure, actual trusted interactive streaming, daemon transport/API for long-lived frame streams, live prepared-emulator validation, real scheduler lease arbitration integration, recording ownership review, default daemon-preferred CLI routing policy, and full scheduler/UI integration remain future work.

### Next step

1. Commit and push this Runtime milestone with checkpoint tag `checkpoint/20260627-daemon-status-entry`.
2. Continue Session Layer follow-ups: trusted UI/API diagnostics, scheduler lease arbitration integration, trusted interactive frame/input channel, long-lived stream transport/API, live prepared-emulator validation, UI/API review surfaces, recording ownership review, default daemon-preferred CLI routing policy, and scheduler/UI integration.

## 2026-06-27 ActingLab daemon-routed devices diagnostics

### Current status

- Added `devices --via-daemon` as a daemon-routed diagnostic request.
- Added `session request devices` as the matching Session Layer request form.
- Daemon-side request execution now supports the `devices` command and reuses the existing `run_devices` path.
- Capabilities now advertise `session request devices`.
- Missing daemon state still fails visibly with `runtime_not_running`.
- Local `devices` remains available with its existing behavior.
- No device control, capture/input path, scheduler implementation, UI, SQLite, OCR/OpenCV, game logic, ADB input fallback, capture hot-path algorithm change, reconnect loop, retry loop, or silent fallback was added.

### Resource mirrors used

- Runtime baseline before this task: `f6c403d66d02a885c0521cbc50370e9ee54d4afb`.
- Resource repositories were not modified or used by this implementation step.

### Files changed

- `apps/actinglab/src/main.rs`
- `PLANS.md`
- `CHECKPOINT.md`

### Commands run

- Re-read the current cooperation workspace task document `TASK-Lab-session-layer.md`.
- Re-read the current cooperation workspace finding document `FINDING-AK-game-freeze-2026-06-27.md`.
- Re-read Runtime-local `AGENTS.md`, `PLANS.md`, and `CHECKPOINT.md`.
- Re-read local `rust-patterns` and `rust-testing` skill instructions.
- `git fetch --prune --tags`
- `git status --short --branch`
- `cargo fmt --all`
- `cargo test -p actingcommand-actinglab devices_via_daemon_without_daemon_is_runtime_error -- --nocapture`
- `cargo test -p actingcommand-actinglab session_request_devices_without_daemon_is_runtime_error -- --nocapture`
- `cargo test -p actingcommand-actinglab direct_touch_commands_are_capability_registered -- --nocapture`
- `cargo fmt --all -- --check`
- `git diff --check`
- Source-only added-code prohibited-feature scan over `apps/actinglab/src/main.rs` for fallback, reconnect/retry loops, direct input fallback, ADB shell input/screencap, SQLite, OCR/OpenCV, and unreviewed trusted-channel implementation.
- `cargo clippy --workspace -- -D warnings`
- `cargo test --workspace`

### Test results

- `cargo test -p actingcommand-actinglab devices_via_daemon_without_daemon_is_runtime_error -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab session_request_devices_without_daemon_is_runtime_error -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab direct_touch_commands_are_capability_registered -- --nocapture` passed with `1` test.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- Source-only added-code prohibited-feature scan returned `NO_PROHIBITED_CODE_ADDED_LINES`.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed.

### Current blocker

- No blocker for the daemon-routed devices diagnostics milestone.
- Full Session Layer remains incomplete: trusted UI/API diagnostics exposure, actual trusted interactive streaming, daemon transport/API for long-lived frame streams, live prepared-emulator validation, real scheduler lease arbitration integration, recording ownership review, default daemon-preferred CLI routing policy, and full scheduler/UI integration remain future work.

### Next step

1. Commit and push this Runtime milestone with checkpoint tag `checkpoint/20260627-daemon-devices-diagnostics`.
2. Continue Session Layer follow-ups: trusted UI/API diagnostics, scheduler lease arbitration integration, trusted interactive frame/input channel, long-lived stream transport/API, live prepared-emulator validation, UI/API review surfaces, recording ownership review, default daemon-preferred CLI routing policy, and scheduler/UI integration.

## 2026-06-27 ActingLab daemon-routed recording interface

### Current status

- Added `session request record` as a daemon-routed Session Layer request.
- `session request record ...` now runs through the resident daemon request queue.
- Recording request payloads preserve `--holder`, `--lease-holder`, and `--lease-id` while stripping client-only request flags such as `--state-dir` and `--request-timeout-ms`.
- Daemon-routed recording operations use the daemon's state directory, so custom daemon state paths do not accidentally fall back to the client's default session state.
- Capabilities now advertise `session request record`.
- Missing daemon state still fails visibly with `runtime_not_running`.
- Local `session record ...` and top-level `record ...` behavior remain available through the same implementation.
- No device control, capture/input path, scheduler implementation, UI, SQLite, OCR/OpenCV, game logic, ADB input fallback, capture hot-path algorithm change, reconnect loop, retry loop, or silent fallback was added.

### Resource mirrors used

- Runtime baseline before this task: `ad70d715ae492f784b49ced3d263ad3606bd0bd4`.
- Resource repositories were not modified or used by this implementation step.

### Files changed

- `apps/actinglab/src/main.rs`
- `PLANS.md`
- `CHECKPOINT.md`

### Commands run

- Re-read the current cooperation workspace task document `TASK-Lab-session-layer.md`.
- Re-read the current cooperation workspace finding document `FINDING-AK-game-freeze-2026-06-27.md`.
- Re-read Runtime-local `PLANS.md` and `CHECKPOINT.md`.
- Re-read local `rust-patterns` and `rust-testing` skill instructions.
- `git status --short --branch`
- `cargo fmt --all`
- `cargo test -p actingcommand-actinglab session_state_request_payload_preserves_holder_and_lease_id -- --nocapture`
- `cargo test -p actingcommand-actinglab session_record_request_starts_statuses_and_stops_in_daemon_state_dir -- --nocapture`
- `cargo test -p actingcommand-actinglab session_request_record_without_daemon_is_runtime_error -- --nocapture`
- `cargo test -p actingcommand-actinglab direct_touch_commands_are_capability_registered -- --nocapture`
- `cargo fmt --all -- --check`
- `git diff --check`
- Source-only added-code prohibited-feature scan over `apps/actinglab/src/main.rs` for fallback, reconnect/retry loops, direct input fallback, ADB shell input/screencap, SQLite, OCR/OpenCV, and unreviewed trusted-channel implementation.
- `cargo clippy --workspace -- -D warnings`
- `cargo test --workspace`

### Test results

- `cargo test -p actingcommand-actinglab session_state_request_payload_preserves_holder_and_lease_id -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab session_record_request_starts_statuses_and_stops_in_daemon_state_dir -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab session_request_record_without_daemon_is_runtime_error -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab direct_touch_commands_are_capability_registered -- --nocapture` passed with `1` test.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- Source-only added-code prohibited-feature scan returned `NO_PROHIBITED_CODE_ADDED_LINES`.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed.

### Current blocker

- No blocker for the daemon-routed recording interface milestone.
- Full Session Layer remains incomplete: trusted UI/API diagnostics exposure, actual trusted interactive streaming, daemon transport/API for long-lived frame streams, live prepared-emulator validation, real scheduler lease arbitration integration, recording ownership review, and full scheduler/UI integration remain future work.

### Next step

1. Commit and push this Runtime milestone with checkpoint tag `checkpoint/20260627-daemon-record-interface`.
2. Continue Session Layer follow-ups: trusted UI/API diagnostics, scheduler lease arbitration integration, trusted interactive frame/input channel, long-lived stream transport/API, live prepared-emulator validation, UI/API review surfaces, recording ownership review, and scheduler/UI integration.

## 2026-06-27 ActingLab daemon-routed lease interface

### Current status

- Added `session request lease` as a daemon-routed Session Layer request.
- `session request lease acquire|release|preempt|status` now runs through the resident daemon request queue.
- Lease request payloads preserve `--holder`, `--lease-holder`, and `--lease-id` while stripping client-only request flags such as `--state-dir` and `--request-timeout-ms`.
- Daemon-routed lease operations use the daemon's state directory, so custom daemon state paths do not accidentally fall back to the client's default session state.
- Capabilities now advertise `session request lease`.
- Missing daemon state still fails visibly with `runtime_not_running`.
- Lease conflicts and holder/id mismatches remain visible safety-blocked errors.
- No device control, capture/input path, scheduler implementation, UI, SQLite, OCR/OpenCV, game logic, ADB input fallback, capture hot-path algorithm change, reconnect loop, retry loop, or silent fallback was added.

### Resource mirrors used

- Runtime baseline before this task: `e0ef4082b9063c71aa6c4b6925d7cd212d06d735`.
- Resource repositories were not modified or used by this implementation step.

### Files changed

- `apps/actinglab/src/main.rs`
- `PLANS.md`
- `CHECKPOINT.md`

### Commands run

- Re-read the current cooperation workspace task document `TASK-Lab-session-layer.md`.
- Re-read the current cooperation workspace finding document `FINDING-AK-game-freeze-2026-06-27.md`.
- Re-read Runtime-local `PLANS.md` and `CHECKPOINT.md`.
- Re-read local `rust-patterns` and `rust-testing` skill instructions.
- `git fetch --prune --tags`
- `git status --short --branch`
- `cargo fmt --all`
- `cargo test -p actingcommand-actinglab session_lease_request_payload_preserves_holder_and_lease_id -- --nocapture`
- `cargo test -p actingcommand-actinglab session_lease_request_acquires_and_releases_in_daemon_state_dir -- --nocapture`
- `cargo test -p actingcommand-actinglab session_request_lease_without_daemon_is_runtime_error -- --nocapture`
- `cargo test -p actingcommand-actinglab direct_touch_commands_are_capability_registered -- --nocapture`
- `cargo fmt --all -- --check`
- `git diff --check`
- Source-only added-code prohibited-feature scan over `apps/actinglab/src/main.rs` for fallback, reconnect/retry loops, direct input fallback, ADB shell input/screencap, SQLite, OCR/OpenCV, and unreviewed trusted-channel implementation.
- `cargo clippy --workspace -- -D warnings`
- `cargo test --workspace`

### Test results

- `cargo test -p actingcommand-actinglab session_lease_request_payload_preserves_holder_and_lease_id -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab session_lease_request_acquires_and_releases_in_daemon_state_dir -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab session_request_lease_without_daemon_is_runtime_error -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab direct_touch_commands_are_capability_registered -- --nocapture` passed with `1` test.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed.
- Source-only added-code prohibited-feature scan returned `NO_PROHIBITED_CODE_ADDED_LINES`.

### Current blocker

- No blocker for the daemon-routed lease interface milestone.
- Full Session Layer remains incomplete: trusted UI/API diagnostics exposure, actual trusted interactive streaming, daemon transport/API for long-lived frame streams, live prepared-emulator validation, real scheduler lease arbitration integration, and full scheduler/UI integration remain future work.

### Next step

1. Commit and push this Runtime milestone with checkpoint tag `checkpoint/20260627-daemon-lease-interface`.
2. Continue Session Layer follow-ups: trusted UI/API diagnostics, scheduler lease arbitration integration, trusted interactive frame/input channel, long-lived stream transport/API, live prepared-emulator validation, UI/API review surfaces, and scheduler/UI integration.

## 2026-06-27 ActingLab daemon-routed journal diagnostics

### Current status

- Added `session request journal` as a read-only daemon request.
- `session request journal [--limit N]` now returns recent resident daemon request-journal entries through the daemon request queue.
- Local `session journal [--limit N]` now shares the same `session_journal_payload` helper.
- Capabilities now advertise `session request journal`.
- Missing daemon state still fails visibly with `runtime_not_running`.
- Corrupt journal lines remain visible runtime errors.
- No daemon command execution semantics, request ordering, response retention, lease enforcement, capture/input path, scheduler implementation, UI, SQLite, OCR/OpenCV, game logic, ADB input fallback, capture hot-path algorithm change, reconnect loop, retry loop, or silent fallback was added.

### Resource mirrors used

- Runtime baseline before this task: `05dc5b2c3156928bdb59498f0efcda8824683a6c`.
- Resource repositories were not modified or used by this implementation step.

### Files changed

- `apps/actinglab/src/main.rs`
- `PLANS.md`
- `CHECKPOINT.md`

### Commands run

- Re-read the current cooperation workspace task document `TASK-Lab-session-layer.md`.
- Re-read the current cooperation workspace finding document `FINDING-AK-game-freeze-2026-06-27.md`.
- Re-read Runtime-local `PLANS.md` and `CHECKPOINT.md`.
- `git fetch --prune --tags`
- `git status --short --branch`
- `cargo fmt --all`
- `cargo test -p actingcommand-actinglab session_journal_request_returns_daemon_journal_entries -- --nocapture`
- `cargo test -p actingcommand-actinglab session_request_journal_without_daemon_is_runtime_error -- --nocapture`
- `cargo test -p actingcommand-actinglab session_request_journal_records_success_and_error -- --nocapture`
- `cargo test -p actingcommand-actinglab top_level_record_capability_is_available -- --nocapture`
- `cargo fmt --all -- --check`
- `git diff --check`
- Source-only added-code prohibited-feature scan over `apps/actinglab/src/main.rs` for fallback, reconnect/retry loops, direct input fallback, ADB shell input/screencap, SQLite, OCR/OpenCV, and unreviewed trusted-channel implementation.
- `cargo test -p actingcommand-actinglab direct_touch_commands_are_capability_registered -- --nocapture`
- `cargo clippy --workspace -- -D warnings`
- `cargo test --workspace`

### Test results

- `cargo test -p actingcommand-actinglab session_journal_request_returns_daemon_journal_entries -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab session_request_journal_without_daemon_is_runtime_error -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab session_request_journal_records_success_and_error -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab top_level_record_capability_is_available -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab direct_touch_commands_are_capability_registered -- --nocapture` passed with `1` test.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- `cargo clippy --workspace -- -D warnings` initially found a needless-borrow warning in the extracted journal helper; after correcting it, the command passed.
- `cargo test --workspace` passed.
- Source-only added-code prohibited-feature scan returned `NO_PROHIBITED_CODE_ADDED_LINES`.

### Current blocker

- No blocker for the daemon-routed journal diagnostics milestone.
- Full Session Layer remains incomplete: trusted UI/API diagnostics exposure, actual trusted interactive streaming, daemon transport/API for long-lived frame streams, live prepared-emulator validation, and full scheduler/UI integration remain future work.

### Next step

1. Commit and push this Runtime milestone with checkpoint tag `checkpoint/20260627-daemon-journal-diagnostics`.
2. Continue Session Layer follow-ups: trusted UI/API diagnostics, trusted interactive frame/input channel, long-lived stream transport/API, live prepared-emulator validation, UI/API review surfaces, and scheduler/UI integration.

## 2026-06-27 ActingLab daemon-routed status diagnostics

### Current status

- Added `session request status` as a read-only daemon request.
- `session request status --diagnostics` now returns the same status/diagnostics payload as local `session status --diagnostics`, but through the resident daemon request queue.
- Local `session status [--diagnostics]` now shares the same `session_status_payload` helper.
- Capabilities now advertise `session request status`.
- Missing daemon state still fails visibly with `runtime_not_running`.
- No daemon command execution semantics, request ordering, response retention, lease enforcement, capture/input path, scheduler implementation, UI, SQLite, OCR/OpenCV, game logic, ADB input fallback, capture hot-path algorithm change, reconnect loop, retry loop, or silent fallback was added.

### Resource mirrors used

- Runtime baseline before this task: `ae33290be9b6fc4601d9dc7c6d5810047c011e8d`.
- Resource repositories were not modified or used by this implementation step.

### Files changed

- `apps/actinglab/src/main.rs`
- `PLANS.md`
- `CHECKPOINT.md`

### Commands run

- Re-read the current cooperation workspace task document `TASK-Lab-session-layer.md`.
- Re-read the current cooperation workspace finding document `FINDING-AK-game-freeze-2026-06-27.md`.
- Re-read Runtime-local `PLANS.md` and `CHECKPOINT.md`.
- `git fetch --prune --tags`
- `git status --short --branch`
- `cargo fmt --all`
- `cargo test -p actingcommand-actinglab session_status_request_returns_daemon_diagnostics -- --nocapture`
- `cargo test -p actingcommand-actinglab session_request_status_without_daemon_is_runtime_error -- --nocapture`
- `cargo test -p actingcommand-actinglab session_status_without_daemon_is_offline_ok -- --nocapture`
- `cargo test -p actingcommand-actinglab top_level_record_capability_is_available -- --nocapture`
- `cargo fmt --all -- --check`
- `git diff --check`
- `cargo clippy --workspace -- -D warnings`
- `cargo test --workspace`
- Source-only added-code prohibited-feature scan over `apps/actinglab/src/main.rs` for fallback, reconnect/retry loops, direct input fallback, ADB shell input/screencap, SQLite, OCR/OpenCV, and unreviewed trusted-channel implementation.

### Test results

- `cargo test -p actingcommand-actinglab session_status_request_returns_daemon_diagnostics -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab session_request_status_without_daemon_is_runtime_error -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab session_status_without_daemon_is_offline_ok -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab top_level_record_capability_is_available -- --nocapture` passed with `1` test.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- `cargo clippy --workspace -- -D warnings` initially found a needless-borrow warning in the extracted status helper; after correcting it, the command passed.
- `cargo test --workspace` passed.
- Source-only added-code prohibited-feature scan returned `NO_PROHIBITED_CODE_ADDED_LINES`.

### Current blocker

- No blocker for the daemon-routed status diagnostics milestone.
- Full Session Layer remains incomplete: trusted UI/API diagnostics exposure, actual trusted interactive streaming, daemon transport/API for long-lived frame streams, live prepared-emulator validation, and full scheduler/UI integration remain future work.

### Next step

1. Commit and push this Runtime milestone with checkpoint tag `checkpoint/20260627-daemon-status-diagnostics`.
2. Continue Session Layer follow-ups: trusted UI/API diagnostics, trusted interactive frame/input channel, long-lived stream transport/API, live prepared-emulator validation, UI/API review surfaces, and scheduler/UI integration.

## 2026-06-27 ActingLab request journal retention

### Current status

- Added bounded retention for the resident daemon request journal.
- `request-journal.jsonl` now rotates to `request-journal.1.jsonl` before appending a new entry when the active file exceeds `1 MiB`.
- Only one local journal archive is retained; the previous archive is replaced on the next rotation.
- `session journal` continues to read the active journal only.
- `session status --diagnostics` now reports active journal path/bytes, retention policy, and archive path/existence/bytes.
- Rotation, stat, archive removal, rename, encode, write, and flush failures remain visible runtime errors.
- No daemon execution semantics, response publication ordering, request removal ordering, lease enforcement, capture/input path, command routing, scheduler implementation, UI, SQLite, OCR/OpenCV, game logic, ADB input fallback, capture hot-path algorithm change, reconnect loop, retry loop, or silent fallback was added.

### Resource mirrors used

- Runtime baseline before this task: `680e23bb3679f006573644dbb466e47ae03867ba`.
- Resource repositories were not modified or used by this implementation step.

### Files changed

- `apps/actinglab/src/main.rs`
- `PLANS.md`
- `CHECKPOINT.md`

### Commands run

- Re-read the current cooperation workspace task document `TASK-Lab-session-layer.md`.
- Re-read the current cooperation workspace finding document `FINDING-AK-game-freeze-2026-06-27.md`.
- Re-read Runtime-local `AGENTS.md`, `PLANS.md`, and `CHECKPOINT.md`.
- `git fetch --prune --tags`
- `git status --short --branch`
- `cargo fmt --all`
- `cargo test -p actingcommand-actinglab session_request_journal_rotates_when_active_file_exceeds_retention_limit -- --nocapture`
- `cargo test -p actingcommand-actinglab session_status_diagnostics_reports_queue_and_journal_summary -- --nocapture`
- `cargo fmt --all -- --check`
- `git diff --check`
- `cargo clippy --workspace -- -D warnings`
- `cargo test --workspace`
- Source-only added-code prohibited-feature scan over `apps/actinglab/src/main.rs` for fallback, reconnect/retry loops, direct input fallback, ADB shell input/screencap, SQLite, OCR/OpenCV, and unreviewed trusted-channel implementation.

### Test results

- `cargo test -p actingcommand-actinglab session_request_journal_rotates_when_active_file_exceeds_retention_limit -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab session_status_diagnostics_reports_queue_and_journal_summary -- --nocapture` passed with `1` test.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed.
- Source-only added-code prohibited-feature scan returned `NO_PROHIBITED_CODE_ADDED_LINES`.

### Current blocker

- No blocker for the request journal retention milestone.
- Full Session Layer remains incomplete: trusted UI/API diagnostics exposure, actual trusted interactive streaming, daemon transport/API for long-lived frame streams, live prepared-emulator validation, and full scheduler/UI integration remain future work.

### Next step

1. Commit and push this Runtime milestone with checkpoint tag `checkpoint/20260627-request-journal-retention`.
2. Continue Session Layer follow-ups: trusted UI/API diagnostics, trusted interactive frame/input channel, long-lived stream transport/API, live prepared-emulator validation, UI/API review surfaces, and scheduler/UI integration.

## 2026-06-27 ActingLab session status diagnostics

### Current status

- Added `session status --diagnostics` as a read-only daemon health surface.
- Normal `session status` output remains stable unless `--diagnostics` is requested.
- Diagnostics now report daemon state paths for info, heartbeat, requests, responses, and journal.
- Diagnostics now report pending request and pending response JSON file counts.
- Diagnostics now report request journal existence, total parsed entries, recent count, last entry, and last error entry.
- Journal total counting parses every non-empty journal line so corrupt historical lines fail visibly.
- No daemon request execution semantics, request ordering, lease enforcement, capture/input paths, command routing, scheduler implementation, UI, SQLite, OCR/OpenCV, game logic, ADB input fallback, capture hot-path algorithm change, reconnect loop, retry loop, or silent fallback was added.

### Resource mirrors used

- Runtime baseline before this task: `e00b369cb76caa56b91811823f09223fcf1cb1c4`.
- Resource repositories were not modified or used by this implementation step.

### Files changed

- `apps/actinglab/src/main.rs`
- `PLANS.md`
- `CHECKPOINT.md`

### Commands run

- Re-read the current cooperation workspace task document `TASK-Lab-session-layer.md`.
- Re-read the current cooperation workspace finding document `FINDING-AK-game-freeze-2026-06-27.md`.
- `git fetch --prune --tags`
- `git status --short --branch`
- `cargo fmt --all`
- `cargo test -p actingcommand-actinglab session_status_diagnostics_reports_queue_and_journal_summary -- --nocapture`
- `cargo test -p actingcommand-actinglab session_status_diagnostics_corrupt_journal_is_runtime_error -- --nocapture`
- `cargo test -p actingcommand-actinglab session_status_without_daemon_is_offline_ok -- --nocapture`
- `cargo fmt --all -- --check`
- `git diff --check`
- `cargo clippy --workspace -- -D warnings`
- `cargo test --workspace`
- Source-only added-code prohibited-feature scan over `apps/actinglab/src/main.rs` for fallback, reconnect/retry loops, direct input fallback, ADB shell input/screencap, SQLite, OCR/OpenCV, and unreviewed trusted-channel implementation.

### Test results

- `cargo test -p actingcommand-actinglab session_status_diagnostics_reports_queue_and_journal_summary -- --nocapture` passed with `1` test after correcting the pending-response expectation to account for the daemon response left for client consumption.
- `cargo test -p actingcommand-actinglab session_status_diagnostics_corrupt_journal_is_runtime_error -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab session_status_without_daemon_is_offline_ok -- --nocapture` passed with `1` test.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed.
- Source-only added-code prohibited-feature scan returned `NO_PROHIBITED_CODE_ADDED_LINES`.

### Current blocker

- No blocker for the session status diagnostics milestone.
- Full Session Layer remains incomplete: journal retention/rotation, trusted UI/API diagnostics exposure, actual trusted interactive streaming, daemon transport/API for long-lived frame streams, live prepared-emulator validation, and full scheduler/UI integration remain future work.

### Next step

1. Commit and push this Runtime milestone with checkpoint tag `checkpoint/20260627-session-status-diagnostics`.
2. Continue Session Layer follow-ups: journal retention/rotation policy, trusted UI/API diagnostics, trusted interactive frame/input channel, long-lived stream transport/API, live prepared-emulator validation, UI/API review surfaces, and scheduler/UI integration.

## 2026-06-27 ActingLab daemon request journal

### Current status

- Added a durable resident daemon request journal under the session state directory.
- Each processed daemon request now writes a `request-journal.jsonl` entry containing request id, command, args, lease metadata, success/error outcome, and created/started/completed timestamps.
- Journal append happens after the daemon response is published and the request file is removed, avoiding duplicate command execution if journal writing fails.
- Added `session journal --state-dir <dir> [--limit N]` to read recent request journal entries.
- `session journal` validates `--limit` as `1..=1000`.
- Corrupt journal lines fail visibly with a runtime error instead of returning incomplete/fake success.
- Capabilities now advertise `session journal`.
- No command execution semantics, lease enforcement, capture/input paths, request ordering, scheduler implementation, UI, SQLite, OCR/OpenCV, game logic, ADB input fallback, capture hot-path algorithm change, reconnect loop, retry loop, or silent fallback was added.

### Resource mirrors used

- Runtime baseline before this task: `41125cce5ffe08891b0defe9ae948aa7f09efe21`.
- Resource repositories were not modified or used by this implementation step.

### Files changed

- `apps/actinglab/src/main.rs`
- `PLANS.md`
- `CHECKPOINT.md`

### Commands run

- Re-read the current cooperation workspace task document `TASK-Lab-session-layer.md`.
- Re-read the current cooperation workspace finding document `FINDING-AK-game-freeze-2026-06-27.md`.
- `git fetch --prune --tags`
- `git status --short --branch`
- `cargo fmt --all`
- `cargo test -p actingcommand-actinglab session_request_journal_records_success_and_error -- --nocapture`
- `cargo test -p actingcommand-actinglab session_journal_corrupt_line_is_runtime_error -- --nocapture`
- `cargo test -p actingcommand-actinglab direct_touch_commands_are_capability_registered -- --nocapture`
- `cargo fmt --all -- --check`
- `git diff --check`
- `cargo clippy --workspace -- -D warnings`
- `cargo test --workspace`
- Source-only added-code prohibited-feature scan over `apps/actinglab/src/main.rs` for fallback, reconnect/retry loops, direct input fallback, ADB shell input/screencap, SQLite, OCR/OpenCV, and unreviewed trusted-channel implementation.

### Test results

- `cargo test -p actingcommand-actinglab session_request_journal_records_success_and_error -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab session_journal_corrupt_line_is_runtime_error -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab direct_touch_commands_are_capability_registered -- --nocapture` passed with `1` test.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed.
- Source-only added-code prohibited-feature scan returned `NO_PROHIBITED_CODE_ADDED_LINES`.

### Current blocker

- No blocker for the daemon request journal milestone.
- Full Session Layer remains incomplete: journal retention/rotation, actual trusted interactive streaming, daemon transport/API for long-lived frame streams, UI/API review surfaces, live prepared-emulator validation, and full scheduler/UI integration remain future work.

### Next step

1. Commit and push this Runtime milestone with checkpoint tag `checkpoint/20260627-daemon-request-journal`.
2. Continue Session Layer follow-ups: journal retention/rotation policy, trusted interactive frame/input channel, long-lived stream transport/API, live prepared-emulator validation, UI/API review surfaces, and scheduler/UI integration.

## 2026-06-27 ActingLab bounded stream scaffold

### Current status

- Turned the future `stream` command from a reserved placeholder into a bounded, read-only frame sampling scaffold.
- `stream --dry-run --max-frames <N>` now returns a local JSON contract without device I/O.
- `stream --max-frames <N>` captures up to `60` frames through the existing capture backend path and reports frame metadata, freshness, and capture attempts.
- `stream --via-daemon` now submits a read-only request through the resident Session Layer queue.
- `session request stream` is now accepted by the daemon request client surface.
- `--input-relay` and `--interactive-input` fail explicitly with `stream_input_relay_not_implemented`; the trusted input relay remains reserved.
- Capabilities now advertise `stream` and `session request stream` as available while the returned stream payload still marks trusted channel and input relay as unfinished.
- No scheduler implementation, UI, SQLite, OCR/OpenCV, game logic, ADB input fallback, capture hot-path algorithm change, reconnect loop, retry loop, or silent fallback was added.

### Resource mirrors used

- Runtime baseline before this task: `eabc0823ff9cf14d410acb5df383158562b195a1`.
- Resource repositories were not modified or used by this implementation step.

### Files changed

- `apps/actinglab/src/main.rs`
- `PLANS.md`
- `CHECKPOINT.md`

### Commands run

- Re-read the current cooperation workspace task document `TASK-Lab-session-layer.md`.
- Re-read the current cooperation workspace finding document `FINDING-AK-game-freeze-2026-06-27.md`.
- `git status --short --branch`
- `git fetch --prune --tags`
- `cargo fmt --all`
- `cargo test -p actingcommand-actinglab stream_command_reports_bounded_dry_run_contract -- --nocapture`
- `cargo test -p actingcommand-actinglab stream_input_relay_is_explicitly_not_implemented -- --nocapture`
- `cargo test -p actingcommand-actinglab stream_via_daemon_without_daemon_is_runtime_error -- --nocapture`
- `cargo test -p actingcommand-actinglab session_request_stream_without_daemon_is_runtime_error -- --nocapture`
- `cargo test -p actingcommand-actinglab direct_touch_commands_are_capability_registered -- --nocapture`
- `cargo fmt --all -- --check`
- `git diff --check`
- `cargo clippy --workspace -- -D warnings`
- `cargo test --workspace` initially failed because the former reserved-stream capability assertion still expected `stream` to be `reserved`.
- `cargo test -p actingcommand-actinglab top_level_record_capability_is_available -- --nocapture`
- `cargo test --workspace`
- Source-only added-code prohibited-feature scan over `apps/actinglab/src/main.rs` for fallback, reconnect/retry loops, direct input fallback, ADB shell input/screencap, SQLite, OCR/OpenCV, and unreviewed trusted-channel implementation.

### Test results

- `cargo test -p actingcommand-actinglab stream_command_reports_bounded_dry_run_contract -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab stream_input_relay_is_explicitly_not_implemented -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab stream_via_daemon_without_daemon_is_runtime_error -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab session_request_stream_without_daemon_is_runtime_error -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab direct_touch_commands_are_capability_registered -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab top_level_record_capability_is_available -- --nocapture` passed with `1` test after updating the old reserved-stream assertion.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed after the assertion update, with the full workspace test suite passing.
- Source-only added-code prohibited-feature scan returned `NO_PROHIBITED_CODE_ADDED_LINES`.

### Current blocker

- No blocker for the bounded stream scaffold milestone.
- Full Session Layer remains incomplete: actual trusted interactive streaming, daemon transport/API for long-lived frame streams, UI/API review surfaces, live prepared-emulator validation, and full scheduler/UI integration remain future work.

### Next step

1. Commit and push this Runtime milestone with checkpoint tag `checkpoint/20260627-bounded-stream-scaffold`.
2. Continue Session Layer follow-ups: trusted interactive frame/input channel, long-lived stream transport/API, live prepared-emulator validation, UI/API review surfaces, and scheduler/UI integration.

## 2026-06-27 ActingLab daemon package/operation run routing

### Current status

- Moved package/operation run surfaces one step closer to the resident Session Layer boundary.
- `package run --via-daemon` now submits a daemon control request before any local package validation.
- `operation run --via-daemon` now submits a daemon control request before any local operation/device work.
- `session request package-run` and `session request operation-run` are now accepted by the daemon request client surface.
- Daemon-side package/operation run requests require matching session lease metadata before package zip validation, operation directory validation, or device I/O can occur.
- Capabilities now advertise `session request package-run` and `session request operation-run`.
- Existing direct `package run` and `operation run` behavior remains safety-blocked/unchanged.
- No scheduler implementation, UI, SQLite, OCR/OpenCV, game logic, ADB input fallback, capture hot-path algorithm change, reconnect loop, retry loop, or silent fallback was added.

### Resource mirrors used

- Runtime baseline before this task: `77b9b215f03016ab8ef816603568d00a75478779`.
- Resource repositories were not modified or used by this implementation step.

### Files changed

- `apps/actinglab/src/main.rs`
- `PLANS.md`
- `CHECKPOINT.md`

### Commands run

- Re-read the current cooperation workspace task document `TASK-Lab-session-layer.md`.
- Re-read the current cooperation workspace finding document `FINDING-AK-game-freeze-2026-06-27.md`.
- `git status --short --branch`
- `git diff --stat`
- `git diff -- apps/actinglab/src/main.rs`
- `cargo fmt --all`
- `cargo test -p actingcommand-actinglab session_package_run_request_requires_lease_before_zip_or_device_io -- --nocapture`
- `cargo test -p actingcommand-actinglab session_operation_run_request_requires_lease_before_device_io -- --nocapture`
- `cargo test -p actingcommand-actinglab package_run_via_daemon_accepts_lease_flags_before_daemon_lookup -- --nocapture`
- `cargo test -p actingcommand-actinglab operation_run_via_daemon_accepts_lease_flags_before_daemon_lookup -- --nocapture`
- `cargo test -p actingcommand-actinglab session_request_package_run_without_daemon_is_runtime_error -- --nocapture`
- `cargo test -p actingcommand-actinglab session_request_operation_run_without_daemon_is_runtime_error -- --nocapture`
- `cargo test -p actingcommand-actinglab direct_touch_commands_are_capability_registered -- --nocapture`
- `cargo fmt --all -- --check`
- `git diff --check`
- `cargo clippy --workspace -- -D warnings`
- `cargo test --workspace`
- Source-only added-code prohibited-feature scan over `apps/actinglab/src/main.rs` for fallback, reconnect/retry loops, direct capture/input execution in the new daemon package/operation routing, SQLite, OCR/OpenCV, and ADB shell input/screencap.

### Test results

- `cargo test -p actingcommand-actinglab session_package_run_request_requires_lease_before_zip_or_device_io -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab session_operation_run_request_requires_lease_before_device_io -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab package_run_via_daemon_accepts_lease_flags_before_daemon_lookup -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab operation_run_via_daemon_accepts_lease_flags_before_daemon_lookup -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab session_request_package_run_without_daemon_is_runtime_error -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab session_request_operation_run_without_daemon_is_runtime_error -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab direct_touch_commands_are_capability_registered -- --nocapture` passed with `1` test.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed.
- Source-only added-code prohibited-feature scan returned `NO_PROHIBITED_CODE_ADDED_LINES`.

### Current blocker

- No blocker for the daemon package/operation run routing milestone.
- Full Session Layer remains incomplete: actual interactive streaming, trusted-channel API, live prepared-emulator recording validation, UI/API review surfaces, and full scheduler/UI integration remain future work.

### Next step

1. Commit and push this Runtime milestone with checkpoint tag `checkpoint/20260627-daemon-package-operation-routing`.
2. Continue full Session Layer follow-ups: actual interactive streaming, trusted-channel API, live prepared-emulator recording validation, UI/API review surfaces, and scheduler/UI integration.

## 2026-06-27 ActingLab daemon Lab run routing

### Current status

- Moved trusted Lab package execution one step closer to the resident Session Layer boundary.
- `lab run --via-daemon` now submits a daemon control request instead of direct-running Lab package execution in the client process.
- `session request lab-run` is now accepted by the daemon request client surface.
- Daemon-side Lab run requests require matching session lease metadata before package zip validation or device I/O can occur.
- Capabilities now advertise `session request lab-run`.
- Existing direct `lab run` behavior is unchanged.
- No scheduler implementation, UI, SQLite, OCR/OpenCV, game logic, ADB input fallback, capture hot-path algorithm change, reconnect loop, retry loop, or silent fallback was added.

### Resource mirrors used

- Runtime baseline before this task: `5a48a810be02359c768e3badafa5c4285240c7ab`.
- Resource repositories were not modified or used by this implementation step.

### Files changed

- `apps/actinglab/src/main.rs`
- `PLANS.md`
- `CHECKPOINT.md`

### Commands run

- Re-read the current cooperation workspace task document `TASK-Lab-session-layer.md`.
- Re-read the current cooperation workspace finding document `FINDING-AK-game-freeze-2026-06-27.md`.
- `git status --short --branch`
- `git diff --stat`
- `git diff -- apps/actinglab/src/main.rs`
- `cargo fmt --all`
- `cargo test -p actingcommand-actinglab session_lab_run_request_requires_lease_before_zip_or_device_io -- --nocapture`
- `cargo test -p actingcommand-actinglab lab_run_via_daemon_accepts_lease_flags_before_daemon_lookup -- --nocapture`
- `cargo test -p actingcommand-actinglab session_request_lab_run_without_daemon_is_runtime_error -- --nocapture`
- `cargo fmt --all -- --check`
- `git diff --check`
- `cargo clippy --workspace -- -D warnings`
- `cargo test --workspace`
- Source-only added-code prohibited-feature scan over `apps/actinglab/src/main.rs` for fallback, reconnect/retry loops, direct capture/input execution in the new daemon Lab run routing, SQLite, OCR/OpenCV, and ADB shell input/screencap.

### Test results

- `cargo test -p actingcommand-actinglab session_lab_run_request_requires_lease_before_zip_or_device_io -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab lab_run_via_daemon_accepts_lease_flags_before_daemon_lookup -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab session_request_lab_run_without_daemon_is_runtime_error -- --nocapture` passed with `1` test.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed.
- Source-only added-code prohibited-feature scan returned `NO_PROHIBITED_CODE_ADDED_LINES`.

### Current blocker

- No blocker for the daemon Lab run routing milestone.
- Full Session Layer remains incomplete: complete daemon-first package/operation routing, actual interactive streaming, trusted-channel API, live prepared-emulator recording validation, UI/API review surfaces, and full scheduler/UI integration remain future work.

### Next step

1. Commit and push this Runtime milestone with checkpoint tag `checkpoint/20260627-daemon-lab-run-routing`.
2. Continue full Session Layer follow-ups: package/operation daemon routing, actual interactive streaming, trusted-channel API, live prepared-emulator recording validation, UI/API review surfaces, and scheduler/UI integration.

## 2026-06-27 ActingLab daemon capture routing

### Current status

- Moved normal one-shot capture one step closer to the resident Session Layer boundary.
- `capture --via-daemon --out <path>` now submits a read-only daemon request instead of direct-running capture in the client process.
- `session request capture --out <path>` is now accepted by the daemon request client surface.
- Capture daemon requests remain read-only and do not require a lease.
- `--out`, `--require-fresh`, `--fresh-delay-ms`, and capture backend selection remain part of the daemon-executed command surface.
- Capabilities now advertise `session request capture`.
- Existing direct `capture` behavior is unchanged.
- No scheduler implementation, UI, SQLite, OCR/OpenCV, game logic, ADB input fallback, direct MaaTouch startup, capture hot-path algorithm change, reconnect loop, retry loop, or silent fallback was added.

### Resource mirrors used

- Runtime baseline before this task: `68e638aee73ce8d9ddfedbbfa287d7aab19aed9a`.
- Resource repositories were not modified or used by this implementation step.

### Files changed

- `apps/actinglab/src/main.rs`
- `PLANS.md`
- `CHECKPOINT.md`

### Commands run

- Re-read the current cooperation workspace task document `TASK-Lab-session-layer.md`.
- Re-read the current cooperation workspace finding document `FINDING-AK-game-freeze-2026-06-27.md`.
- `git status --short --branch`
- `git fetch --prune --tags origin`
- `git pull --ff-only origin main`
- `git diff --stat`
- `git diff -- apps/actinglab/src/main.rs`
- `cargo fmt --all`
- `cargo test -p actingcommand-actinglab capture_via_daemon_without_daemon_is_runtime_error -- --nocapture`
- `cargo test -p actingcommand-actinglab session_request_capture_without_daemon_is_runtime_error -- --nocapture`
- `cargo test -p actingcommand-actinglab direct_touch_commands_are_capability_registered -- --nocapture`
- `cargo fmt --all -- --check`
- `git diff --check`
- `cargo clippy --workspace -- -D warnings`
- `cargo test --workspace`
- Source-only added-code prohibited-feature scan over `apps/actinglab/src/main.rs` for fallback, reconnect/retry loops, ADB shell input/screencap, direct MaaTouch startup, SQLite, and OCR/OpenCV.

### Test results

- `cargo test -p actingcommand-actinglab capture_via_daemon_without_daemon_is_runtime_error -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab session_request_capture_without_daemon_is_runtime_error -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab direct_touch_commands_are_capability_registered -- --nocapture` passed with `1` test.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed.
- Source-only added-code prohibited-feature scan returned `NO_PROHIBITED_CODE_ADDED_LINES`.

### Current blocker

- No blocker for the daemon capture routing milestone.
- Full Session Layer remains incomplete: complete daemon-first package/operation routing, actual interactive streaming, trusted-channel API, live prepared-emulator recording validation, UI/API review surfaces, and full scheduler/UI integration remain future work.

### Next step

1. Commit and push this Runtime milestone with checkpoint tag `checkpoint/20260627-daemon-capture-routing`.
2. Continue full Session Layer follow-ups: package/operation daemon routing, actual interactive streaming, trusted-channel API, live prepared-emulator recording validation, UI/API review surfaces, and scheduler/UI integration.

## 2026-06-27 ActingLab daemon instance lifecycle routing

### Current status

- Moved instance lifecycle diagnostics/control one step closer to the resident Session Layer boundary.
- `session instance list|health|reconnect --via-daemon` now submits daemon requests instead of trying to direct-run through the client process.
- `session request instance <list|health|reconnect>` is now accepted by the daemon request client surface.
- Daemon-side `list` and `health` remain read-only and do not require a lease.
- Daemon-side `reconnect` requires matching session lease metadata before any device I/O can occur.
- Capabilities now advertise concrete `session instance ...` and `session request instance ...` commands.
- Existing direct `session instance` behavior is unchanged.
- No scheduler implementation, UI, SQLite, OCR/OpenCV, game logic, ADB input fallback, direct MaaTouch startup, capture algorithm change, reconnect loop, retry loop, or silent fallback was added.

### Resource mirrors used

- Runtime baseline before this task: `3a4d83908181ccd57e0d62634feac906af9dcd9a`.
- Resource repositories were not modified or used by this implementation step.

### Files changed

- `apps/actinglab/src/main.rs`
- `PLANS.md`
- `CHECKPOINT.md`

### Commands run

- Re-read the current cooperation workspace task document `TASK-Lab-session-layer.md`.
- Re-read the current cooperation workspace finding document `FINDING-AK-game-freeze-2026-06-27.md`.
- `git status --short --branch`
- `git diff --stat`
- `git log -1 --oneline --decorate`
- `git diff -- apps/actinglab/src/main.rs`
- `cargo fmt --all`
- `cargo test -p actingcommand-actinglab session_instance_ -- --nocapture`
- `cargo test -p actingcommand-actinglab session_request_instance_without_daemon_is_runtime_error -- --nocapture`
- `cargo test -p actingcommand-actinglab direct_touch_commands_are_capability_registered -- --nocapture`
- `cargo fmt --all -- --check`
- `git diff --check`
- `cargo clippy --workspace -- -D warnings`
- `cargo test --workspace`
- Source-only added-code prohibited-feature scan over `apps/actinglab/src/main.rs` for fallback, reconnect/retry loops, direct capture/input execution in the new daemon instance routing, SQLite, OCR/OpenCV, and ADB shell input/screencap.

### Test results

- `cargo test -p actingcommand-actinglab session_instance_ -- --nocapture` passed with `4` tests.
- `cargo test -p actingcommand-actinglab session_request_instance_without_daemon_is_runtime_error -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab direct_touch_commands_are_capability_registered -- --nocapture` passed with `1` test.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed.
- Source-only added-code prohibited-feature scan returned `NO_PROHIBITED_CODE_ADDED_LINES`.

### Current blocker

- No blocker for the daemon instance lifecycle routing milestone.
- Full Session Layer remains incomplete: complete daemon-first package/operation routing, actual interactive streaming, trusted-channel API, live prepared-emulator recording validation, UI/API review surfaces, and full scheduler/UI integration remain future work.

### Next step

1. Commit and push this Runtime milestone with checkpoint tag `checkpoint/20260627-daemon-instance-routing`.
2. Continue full Session Layer follow-ups: package/operation daemon routing, actual interactive streaming, trusted-channel API, live prepared-emulator recording validation, UI/API review surfaces, and scheduler/UI integration.

## 2026-06-27 ActingLab daemon app lifecycle routing

### Current status

- Moved app lifecycle control one step closer to the resident Session Layer boundary.
- `session app launch|stop|restart --via-daemon` now submits a daemon control request instead of trying to direct-run the app command.
- `session request app <launch|stop|restart>` is now accepted by the daemon request client surface.
- Daemon-side app lifecycle requests require matching session lease metadata before any device I/O can occur.
- Capabilities now advertise `session request app` and concrete `session app launch`, `session app stop`, and `session app restart` commands.
- Existing direct `session app` behavior is unchanged.
- No scheduler implementation, UI, SQLite, OCR/OpenCV, game logic, ADB input fallback, direct MaaTouch startup, capture algorithm change, reconnect loop, retry loop, or silent fallback was added.

### Resource mirrors used

- Runtime baseline before this task: `d0894c614d2220e13d0e766d4ee9ada1f1a340ca`.
- Resource repositories were not modified or used by this implementation step.

### Files changed

- `apps/actinglab/src/main.rs`
- `PLANS.md`
- `CHECKPOINT.md`

### Commands run

- Re-read the current cooperation workspace task document `TASK-Lab-session-layer.md`.
- Re-read the current cooperation workspace finding document `FINDING-AK-game-freeze-2026-06-27.md`.
- `git status --short --branch`
- `git fetch --prune --tags origin`
- `git pull --ff-only origin main`
- `cargo fmt --all`
- First targeted test run failed at compile time with Rust error `E0308` because `run_session_app` passed a `Vec<String>` to a helper expecting `&[String]`; corrected the call to pass the original args slice.
- `cargo test -p actingcommand-actinglab session_app -- --nocapture`
- `cargo test -p actingcommand-actinglab session_request_app_without_daemon_is_runtime_error -- --nocapture`
- `cargo test -p actingcommand-actinglab direct_touch_commands_are_capability_registered -- --nocapture`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace -- -D warnings`
- `cargo test --workspace`
- `git diff --check`
- Source-only added-code prohibited-feature scan over `apps/actinglab/src/main.rs` for fallback, reconnect/retry loops, direct capture/input execution in the new daemon app routing, SQLite, OCR/OpenCV, and ADB shell input/screencap.

### Test results

- `cargo test -p actingcommand-actinglab session_app -- --nocapture` passed with `2` tests.
- `cargo test -p actingcommand-actinglab session_request_app_without_daemon_is_runtime_error -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab direct_touch_commands_are_capability_registered -- --nocapture` passed with `1` test.
- `cargo fmt --all -- --check` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed.
- `git diff --check` passed.
- Source-only added-code prohibited-feature scan returned `NO_PROHIBITED_CODE_ADDED_LINES`.

### Current blocker

- No blocker for the daemon app lifecycle routing milestone.
- Full Session Layer remains incomplete: complete daemon-first routing, actual interactive streaming, trusted-channel API, live prepared-emulator recording validation, UI/API review surfaces, and full scheduler/UI integration remain future work.

### Next step

1. Commit and push this Runtime milestone with checkpoint tag `checkpoint/20260627-daemon-app-routing`.
2. Continue full Session Layer follow-ups: complete daemon-first routing, actual interactive streaming, trusted-channel API, live prepared-emulator recording validation, UI/API review surfaces, and scheduler/UI integration.

## 2026-06-27 ActingLab session interface surface alignment

### Current status

- Aligned the visible Session Layer CLI surface with the current task document without implementing the future stream transport.
- `record start`, `record status`, and `record stop` are now advertised as available offline capabilities.
- `session record start`, `session record status`, and `session record stop` are now advertised as available offline capabilities.
- Added a top-level `stream` command entry point for the future interactive frame/input channel.
- `stream` now fails explicitly with `stream_not_implemented` instead of returning an unknown-command error or fake success.
- Existing `session record` behavior, build-task generation, daemon request routing, lease gates, device capture/input behavior, and JSON envelope shape remain unchanged.
- No frame streaming, input relay, UI, TLS/authentication, scheduler, SQLite, OCR/OpenCV, game logic, device I/O, ADB shell input/screencap, direct MaaTouch startup, fallback, reconnect, or retry path was added.

### Resource mirrors used

- Runtime baseline before this task: `aff8fcbb18f28518856d06a247622e04750484a5`.
- Resource repositories were not modified or used by this implementation step.

### Files changed

- `apps/actinglab/src/main.rs`
- `PLANS.md`
- `CHECKPOINT.md`

### Commands run

- Re-read the current cooperation workspace task document `TASK-Lab-session-layer.md`.
- Re-read the current cooperation workspace finding document `FINDING-AK-game-freeze-2026-06-27.md`.
- Re-read Runtime-local `PLANS.md`, `CHECKPOINT.md`, `LICENSE_POLICY.md` if present, and `NOTICE.md`.
- `git status --short --branch`
- `git fetch --prune --tags origin`
- `git pull --ff-only origin main`
- `cargo fmt --all`
- `cargo test -p actingcommand-actinglab stream_command_is_reserved_not_unknown -- --nocapture`
- `cargo test -p actingcommand-actinglab top_level_record_capability_is_available -- --nocapture`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace -- -D warnings`
- `cargo test --workspace`
- `git diff --check`
- Source-only added-code prohibited-feature scan over `apps/actinglab/src/main.rs` for device I/O, capture/input execution, SQLite, OCR/OpenCV, fallback, reconnect, and retry.

### Test results

- `cargo test -p actingcommand-actinglab stream_command_is_reserved_not_unknown -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab top_level_record_capability_is_available -- --nocapture` passed with `1` test.
- `cargo fmt --all -- --check` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed.
- `git diff --check` passed.
- Source-only added-code prohibited-feature scan returned `NO_PROHIBITED_CODE_ADDED_LINES`.

### Current blocker

- No blocker for the interface-surface alignment milestone.
- Full Session Layer remains incomplete: actual interactive streaming, trusted-channel API, live prepared-emulator recording validation, UI/API review surfaces, and full scheduler/UI integration remain future work.

### Next step

1. Commit and push this Runtime milestone with checkpoint tag `checkpoint/20260627-session-interface-surface`.
2. Continue full Session Layer follow-ups: actual interactive streaming, trusted-channel API, live prepared-emulator recording validation, UI/API review surfaces, and scheduler/UI integration.

## 2026-06-27 ActingLab session recording build-task capability close-out

### Current status

- Closed the top-level recording CLI contract gap for `record build-task`.
- `session record build-task` is now advertised as an available offline capability.
- Top-level `record build-task` is now advertised as an available offline capability.
- Added a route test proving top-level `record build-task` reaches the existing recording implementation and fails explicitly with `record_session_not_active` when no recording context exists.
- Existing `session record build-task` behavior, generated bundle logic, resource promotion behavior, state files, JSON envelope shape, validation, and exit-code mapping remain unchanged.
- No device I/O, UI, SQLite, OCR/OpenCV, game logic, ADB shell input/screencap, direct MaaTouch startup, fallback, reconnect, or retry path was added.

### Resource mirrors used

- Runtime baseline before this task: `9d2b7546ccbfe42745224bab70b008e9e9374b99`.
- Resource repositories were not modified or used by this implementation step.

### Files changed

- `apps/actinglab/src/main.rs`
- `PLANS.md`
- `CHECKPOINT.md`

### Commands run

- Re-read the current cooperation workspace task document `TASK-Lab-session-layer.md`.
- Re-read the current cooperation workspace finding document `FINDING-AK-game-freeze-2026-06-27.md`.
- `git status --short --branch`
- `git fetch --prune --tags origin`
- `git pull --ff-only origin main`
- `cargo fmt --all`
- `cargo test -p actingcommand-actinglab top_level_record -- --nocapture`
- `cargo test -p actingcommand-actinglab session_record_build_task_requires_record -- --nocapture`
- `cargo fmt --all -- --check`
- `git diff --check`
- `cargo clippy --workspace -- -D warnings`
- `cargo test --workspace`
- Source-only added-code prohibited-feature scan over `apps/actinglab/src/main.rs` for ADB shell input/screencap, MaaTouch startup, direct tap/swipe execution, SQLite, OCR/OpenCV, fallback, reconnect, and retry.

### Test results

- `cargo test -p actingcommand-actinglab top_level_record -- --nocapture` passed with `3` tests.
- `cargo test -p actingcommand-actinglab session_record_build_task_requires_record -- --nocapture` passed with `1` test.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed.
- Source-only added-code prohibited-feature scan returned `NO_PROHIBITED_CODE_ADDED_LINES`.

### Current blocker

- No blocker for the build-task capability close-out.
- Full Session Layer remains incomplete: live prepared-emulator recording validation, UI/API review surfaces, resident streaming, and full scheduler/UI integration remain future work.

### Next step

1. Commit and push this Runtime milestone with checkpoint tag `checkpoint/20260627-record-build-task-capability`.
2. Continue full Session Layer follow-ups: live prepared-emulator recording validation, UI/API review surfaces, resident streaming, and scheduler/UI integration.

## 2026-06-27 ActingLab session recording top-level CLI contract alias

### Current status

- Aligned the Phase D recording CLI with the Session Layer interface draft by enabling top-level `record <action> ...`.
- Top-level `record ...` now routes to the same implementation as `session record ...`.
- Existing `session record ...` behavior, state files, JSON envelope shape, validation, and exit-code mapping remain unchanged.
- Capabilities now mark `record`, `record step`, `record candidates`, `record amend`, and `record promote` as available instead of leaving top-level `record` reserved.
- No device I/O, UI, SQLite, OCR/OpenCV, game logic, ADB shell input/screencap, direct MaaTouch startup, fallback, reconnect, or retry path was added.

### Resource mirrors used

- Runtime baseline before this task: `4cd427339defa93a24582e67b4a19d5732327380`.
- Resource repositories were not modified or used by this implementation step.

### Files changed

- `apps/actinglab/src/main.rs`
- `PLANS.md`
- `CHECKPOINT.md`

### Commands run

- Re-read the current cooperation workspace task document `TASK-Lab-session-layer.md`.
- Re-read the current cooperation workspace finding document `FINDING-AK-game-freeze-2026-06-27.md`.
- `git status --short --branch`
- `git fetch --prune --tags origin`
- `git pull --ff-only origin main`
- `cargo fmt --all`
- `cargo test -p actingcommand-actinglab top_level_record -- --nocapture`
- `cargo test -p actingcommand-actinglab session_record -- --nocapture`
- `cargo fmt --all -- --check`
- `git diff --check`
- `cargo clippy --workspace -- -D warnings`
- First `cargo test --workspace`
- `cargo test -p actingcommand-actinglab detect_page_returns_standby_when_no_page_matches -- --nocapture`
- Reran `cargo test --workspace`
- Source-only added-code prohibited-feature scan over `apps/actinglab/src/main.rs` for ADB shell input/screencap, MaaTouch startup, direct tap/swipe execution, SQLite, OCR/OpenCV, fallback, reconnect, and retry.

### Test results

- `cargo test -p actingcommand-actinglab top_level_record -- --nocapture` passed with `2` tests.
- `cargo test -p actingcommand-actinglab session_record -- --nocapture` passed with `41` tests.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- First `cargo test --workspace` failed once in `tests::detect_page_returns_standby_when_no_page_matches` with exit code `2` instead of `0`.
- Isolated `cargo test -p actingcommand-actinglab detect_page_returns_standby_when_no_page_matches -- --nocapture` passed with `1` test.
- Rerun `cargo test --workspace` passed.
- Source-only added-code prohibited-feature scan returned `NO_PROHIBITED_CODE_ADDED_LINES`.

### Current blocker

- No blocker for the top-level recording CLI alias.
- Full Session Layer remains incomplete: live prepared-emulator recording validation, UI/API review surfaces, resident streaming, and full scheduler/UI integration remain future work.

### Next step

1. Run full formatting, clippy, workspace tests, diff checks, and prohibited-feature scan.
2. Commit and push this Runtime milestone.
3. Add checkpoint tag `checkpoint/20260627-record-top-level-alias`.

## 2026-06-27 ActingLab session recording standalone resource amend loop

### Current status

- Advanced Phase D recording by extending `session record amend` to standalone `color-probe` and `verify-template` steps.
- `color-probe` amendments can update ids, regions, and auto-region candidate selections.
- Frame-backed color-probe amendments recompute `expected` RGB from the recorded source frame.
- Metadata-only color-probe amendments remain explicit and deferred with reason `amended_without_frame_provenance`.
- `verify-template` amendments can update ids, regions, thresholds, clear-threshold requests, and auto-region candidate selections.
- Frame-backed verify-template amendments re-materialize the template artifact and rerun offline self-backtests from the recorded source frame.
- Metadata-only verify-template amendments remain explicit and deferred with reason `amended_without_frame_provenance`.
- `session record candidates` now reports auto-region candidate previews for standalone resource steps, with `resource_kind` / `resource_id` and the existing `anchor_id` compatibility alias.
- No UI, SQLite, OCR/OpenCV, game logic, ADB shell input/screencap, direct MaaTouch startup, fallback, reconnect, or retry path was added.

### Resource mirrors used

- Runtime baseline before this task: `c3da2aad024d7429677a3b3b3f421026636e90ef`.
- Resource repositories were not modified or used by this implementation step.

### Files changed

- `apps/actinglab/src/main.rs`
- `PLANS.md`
- `CHECKPOINT.md`

### Commands run

- Read the current cooperation workspace task document `TASK-Lab-session-layer.md`.
- Read the current cooperation workspace finding document `FINDING-AK-game-freeze-2026-06-27.md`.
- `git status --short --branch`
- `git fetch --prune --tags origin`
- `git pull --ff-only origin main`
- `cargo fmt --all`
- First targeted `cargo test` command failed because Cargo accepts only one test filter.
- First `cargo test -p actingcommand-lab ...` command failed because the actual package name is `actingcommand-actinglab`.
- `cargo test -p actingcommand-actinglab session_record_amend_`
- `cargo test -p actingcommand-actinglab session_record_candidates_`
- `cargo test -p actingcommand-actinglab session_record -- --nocapture`
- `cargo test -p actingcommand-actinglab`
- `cargo fmt --all -- --check`
- `git diff --check`
- `cargo clippy --workspace -- -D warnings`
- `cargo test --workspace`
- Source-only added-code prohibited-feature scan over `apps/actinglab/src/main.rs` for ADB shell input/screencap, MaaTouch startup, direct tap/swipe execution, SQLite, OCR/OpenCV, fallback, reconnect, and retry.

### Test results

- `cargo test -p actingcommand-actinglab session_record_amend_` passed with `9` tests.
- `cargo test -p actingcommand-actinglab session_record_candidates_` passed with `3` tests.
- `cargo test -p actingcommand-actinglab session_record -- --nocapture` passed with `40` tests.
- `cargo test -p actingcommand-actinglab` passed with `150` tests.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed.
- Source-only added-code prohibited-feature scan returned `NO_PROHIBITED_CODE_ADDED_LINES`.

### Current blocker

- No blocker for the standalone resource amend-loop implementation.
- Full Phase D still has future work: live prepared-emulator validation and UI/API surfaces for candidate review, color-check review, standalone resource review, promotion, and amend flows.

### Next step

1. Commit and push this Runtime milestone.
2. Add checkpoint tag `checkpoint/20260627-session-record-resource-amend-loop`.
3. Continue Phase D with live prepared-emulator validation or UI/API review surfaces after this amend loop is accepted.

## 2026-06-27 ActingLab session recording standalone verify-template output

### Current status

- Advanced Phase D recording by adding a standalone `verify-template` step kind.
- `session record step --kind verify-template` and `--kind verify_template` now append an authorized reusable template target to an active recording context.
- Metadata-only verify-template steps remain explicit and deferred with reason `frame_not_provided`.
- Frame-backed verify-template steps reuse the existing crop/artifact/provenance/self-backtest path.
- `session record build-task` now emits `verify_templates[]` in generated Operation Bundle 0.3 drafts.
- Deferred verify-template steps without frame artifacts fail visibly during build-task instead of producing fake template targets.
- `resource convert` now validates verify-template asset paths and emits bundle `verify_templates[]` as recognition-pack `type=template` targets.
- No UI, SQLite, OCR/OpenCV, game logic, ADB shell input/screencap, direct MaaTouch startup, fallback, reconnect, or retry path was added.

### Resource mirrors used

- Runtime baseline before this task: `9b5da76e9ac04c9b54aaabad67329b3d7efcf977`.
- Resource repositories were not modified or used by this implementation step.

### Files changed

- `apps/actinglab/src/main.rs`
- `apps/actinglab/src/resource_convert.rs`
- `PLANS.md`
- `CHECKPOINT.md`

### Commands run

- Read the current cooperation workspace task document `TASK-Lab-session-layer.md`.
- Read the current cooperation workspace finding document `FINDING-AK-game-freeze-2026-06-27.md`.
- `git status --short --branch`
- `git fetch --prune --tags origin`
- `git pull --ff-only origin main`
- `cargo fmt --all`
- `cargo test -p actingcommand-actinglab session_record_step_verify_template -- --nocapture`
- `cargo test -p actingcommand-actinglab session_record_build_task_rejects_deferred_verify_template -- --nocapture`
- `cargo test -p actingcommand-actinglab build_pack_includes_verify_template_targets -- --nocapture`
- `cargo test -p actingcommand-actinglab session_record_build_task_writes_draft_bundle -- --nocapture`
- `cargo test -p actingcommand-actinglab session_record -- --nocapture`
- `cargo test -p actingcommand-actinglab resource_convert -- --nocapture`
- `cargo test -p actingcommand-actinglab`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace -- -D warnings`
- First `cargo test --workspace`
- `cargo test -p actingcommand-actinglab navigate_dry_run_uses_navigation_graph -- --nocapture`
- Reran `cargo test --workspace`
- `git diff --check`
- Source-only added-code prohibited-feature scan over `apps/actinglab/src/main.rs` and `apps/actinglab/src/resource_convert.rs` for ADB shell input/screencap, MaaTouch startup, direct tap/swipe execution, SQLite, OCR/OpenCV, fallback, reconnect, and retry.

### Test results

- `cargo test -p actingcommand-actinglab session_record_step_verify_template -- --nocapture` passed with `2` tests.
- `cargo test -p actingcommand-actinglab session_record_build_task_rejects_deferred_verify_template -- --nocapture` passed with `1` test.
- First `cargo test -p actingcommand-actinglab build_pack_includes_verify_template_targets -- --nocapture` failed because the converter unit-test fixture used relative repo root `.` and hit the existing `repo_rel` prefix guard; the fixture now uses an absolute current directory.
- Rerun `cargo test -p actingcommand-actinglab build_pack_includes_verify_template_targets -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab session_record_build_task_writes_draft_bundle -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab session_record -- --nocapture` passed with `36` tests.
- `cargo test -p actingcommand-actinglab resource_convert -- --nocapture` passed with `7` tests.
- `cargo test -p actingcommand-actinglab` passed with `146` tests.
- `cargo fmt --all -- --check` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- First `cargo test --workspace` failed once in `tests::navigate_dry_run_uses_navigation_graph` with exit code `2` instead of `0`.
- Isolated `cargo test -p actingcommand-actinglab navigate_dry_run_uses_navigation_graph -- --nocapture` passed with `1` test.
- Rerun `cargo test --workspace` passed.
- `git diff --check` passed.
- Source-only added-code prohibited-feature scan returned `NO_PROHIBITED_CODE_ADDED_LINES`.

### Current blocker

- No blocker for the standalone verify-template implementation.
- Full Phase D remains incomplete: live prepared-emulator validation, color-probe/verify-template amend semantics, UI/API wiring, and SQLite metadata remain future work.

### Next step

1. Commit and push this Runtime milestone.
2. Add checkpoint tag `checkpoint/20260627-session-record-verify-template`.
3. Continue Phase D with live prepared-emulator validation or explicit amend semantics after this verify-template path is accepted.

## 2026-06-27 ActingLab session recording standalone color-probe output

### Current status

- Advanced Phase D recording by adding a standalone `color-probe` step kind.
- `session record step --kind color-probe` and `--kind color_probe` now append an authorized color probe to an active recording context.
- Metadata-only color-probe steps remain explicit and deferred with reason `frame_not_provided`.
- Frame-backed color-probe steps sample the average RGB value over the authorized region and store `expected`, frame provenance, and evaluation metadata.
- `session record build-task` now emits `color_probes[]` in generated Operation Bundle 0.3 drafts.
- Deferred color-probes without `expected` fail visibly during build-task instead of producing fake defaults.
- `resource convert` now emits bundle `color_probes[]` as recognition-pack `type=color` targets.
- No UI, SQLite, OCR/OpenCV, game logic, ADB shell input/screencap, direct MaaTouch startup, fallback, reconnect, or retry path was added.

### Resource mirrors used

- Runtime baseline before this task: `31a2481564eee29cac186327f8bbe890500356ef`.
- Resource repositories were not modified or used by this implementation step.

### Files changed

- `apps/actinglab/src/main.rs`
- `apps/actinglab/src/resource_convert.rs`
- `PLANS.md`
- `CHECKPOINT.md`

### Commands run

- Read the current cooperation workspace task document `TASK-Lab-session-layer.md`.
- Read the current cooperation workspace finding document `FINDING-AK-game-freeze-2026-06-27.md`.
- `git fetch --prune --tags origin`
- `git pull --ff-only origin main`
- `git status --short --branch`
- `cargo test -p actingcommand-actinglab session_record_step_color_probe -- --nocapture`
- `cargo test -p actingcommand-actinglab build_pack_includes_color_probe_targets -- --nocapture`
- `cargo test -p actingcommand-actinglab session_record_build_task_writes_draft_bundle -- --nocapture`
- `cargo test -p actingcommand-actinglab session_record -- --nocapture`
- `cargo test -p actingcommand-actinglab resource_convert -- --nocapture`
- `cargo fmt --all`
- `cargo test -p actingcommand-actinglab`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace -- -D warnings`
- `cargo test --workspace`
- `git diff --check`
- Source-only added-code prohibited-feature scan over `apps/actinglab/src/main.rs` and `apps/actinglab/src/resource_convert.rs` for ADB shell input/screencap, MaaTouch startup, direct tap/swipe execution, SQLite, OCR/OpenCV, fallback, reconnect, and retry.

### Test results

- First targeted test compile failed because the new converter test called `Value::pointer` on a `serde_json::Map`; the test assertion was corrected to keep the target as `Value`.
- `cargo test -p actingcommand-actinglab session_record_step_color_probe -- --nocapture` passed with `2` tests.
- `cargo test -p actingcommand-actinglab build_pack_includes_color_probe_targets -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab session_record_build_task_writes_draft_bundle -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab session_record -- --nocapture` passed with `33` tests.
- `cargo test -p actingcommand-actinglab resource_convert -- --nocapture` passed with `6` tests.
- `cargo test -p actingcommand-actinglab` passed with `142` tests.
- `cargo fmt --all -- --check` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed.
- `git diff --check` passed.
- Source-only added-code prohibited-feature scan returned `NO_PROHIBITED_CODE_ADDED_LINES`.

### Current blocker

- No blocker for the standalone color-probe implementation.
- Full Phase D remains incomplete: live prepared-emulator validation, `verify-template` resource kind, color-probe amend semantics, UI/API wiring, and SQLite metadata remain future work.

### Next step

1. Commit and push this Runtime milestone.
2. Add checkpoint tag `checkpoint/20260627-session-record-color-probe`.
3. Continue Phase D with live prepared-emulator validation or `verify-template` after this color-probe path is accepted.

## 2026-06-27 ActingLab session recording anchor color-check output

### Current status

- Advanced Phase D recording by making `--color-check` produce an actual bundle color check.
- Frame-backed anchors recorded with `color_check=true` now emit `color_check.region` and `color_check.expected` in `session record build-task`.
- The expected color is derived as the average RGB value over the authorized anchor rectangle in the recorded source frame.
- Anchors without `--color-check` still emit `color_check: null`.
- If a requested color check lacks frame provenance, build-task fails visibly instead of silently omitting the check.
- The updated build-task test verifies the generated color-check JSON and still runs `package build-task --dry-run` against the generated draft repository.
- No device I/O, MaaTouch startup, frame capture, resource repository write, OCR, SQLite, UI, or game logic was added.

### Resource mirrors used

- Runtime baseline before this task: `deee8151d310e62a3566b45330b211a5b5e4b9d9`.
- Resource repositories were not modified or used by this implementation step.

### Files changed

- `apps/actinglab/src/main.rs`
- `PLANS.md`
- `CHECKPOINT.md`

### Commands run

- Searched current code and local Arknights resource examples for `color_check` format.
- `cargo fmt --all`
- First `cargo test -p actingcommand-actinglab session_record_build_task_writes_draft_bundle -- --nocapture`
- Fixed the test flag placement after the first run showed the asserted build-task fixture had not enabled `--color-check`.
- Reran `cargo fmt --all`
- Reran `cargo test -p actingcommand-actinglab session_record_build_task_writes_draft_bundle -- --nocapture`
- `cargo test -p actingcommand-actinglab session_record -- --nocapture`
- `cargo test -p actingcommand-actinglab`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace -- -D warnings`
- `cargo test --workspace`
- `git diff --check`
- Source-only added-code prohibited-feature scan over `apps/actinglab/src/main.rs` for ADB shell input/screencap, MaaTouch startup, direct tap/swipe execution, SQLite, OCR/OpenCV, fallback, reconnect, and retry.

### Test results

- First targeted test run failed because the test assertion expected color-check output while the build-task fixture had not enabled `--color-check`; this was a test setup issue, not a production path failure.
- Rerun `cargo test -p actingcommand-actinglab session_record_build_task_writes_draft_bundle -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab session_record -- --nocapture` passed with `30` tests.
- `cargo test -p actingcommand-actinglab` passed with `138` tests.
- `cargo fmt --all -- --check` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed.
- `git diff --check` passed.
- Source-only added-code prohibited-feature scan returned `NO_PROHIBITED_CODE_ADDED_LINES`.

### Current blocker

- No blocker for the anchor color-check output implementation.
- Full Phase D remains incomplete: live prepared-emulator validation, standalone color-probe/verify-template resource kinds, UI/API wiring, and SQLite metadata remain future work.

### Next step

1. Commit and push this Runtime milestone.
2. Add checkpoint tag `checkpoint/20260627-session-record-color-check`.
3. Continue Phase D with live prepared-emulator validation or standalone recording resource kinds after this color-check output path is accepted.

## 2026-06-27 ActingLab session recording resource promotion

### Current status

- Advanced Phase D recording with an explicit resource promotion command.
- `session record promote --repo <resource-repo-or-root>` now publishes a validated recording draft into an existing resource root or repository containing `ours/`.
- `session record publish` is accepted as an alias for the same guarded promotion path.
- Promotion reuses the existing `session_record_build_draft` validation, so anchors, operations, page references, coordinate bounds, and task-id safety are checked before any write.
- Repository roots are resolved the same way as package/resource commands, including `<repo>/ours`.
- Existing promoted task directories fail visibly by default with `record_promote_target_exists`.
- `--force` replaces only the promoted task directory.
- Existing shared `operations/resources.json` is preserved; the empty placeholder is created only when it is missing.
- Promoted output was verified by running `package build-task --dry-run` against the promoted resource repository inside the new test.
- No device I/O, MaaTouch startup, frame capture, OCR, SQLite, UI, or game logic was added.

### Resource mirrors used

- Runtime baseline before this task: `0f2184f057a54d77eeeaa7eca9886798759a4398`.
- Resource repositories were not modified. A temporary synthetic `repo/ours` fixture was used in tests.

### Files changed

- `apps/actinglab/src/main.rs`
- `PLANS.md`
- `CHECKPOINT.md`

### Commands run

- Re-read `C:\合作工作区\ActingCommand\TASK-Lab-session-layer.md` with UTF-8 output.
- Re-read `C:\合作工作区\ActingCommand\FINDING-AK-game-freeze-2026-06-27.md`.
- `git fetch --prune --tags origin`
- `git pull --ff-only origin main`
- `git status --short --branch`
- `git log -1 --oneline --decorate`
- Read current `PLANS.md` and `CHECKPOINT.md`.
- `cargo fmt --all`
- `cargo test -p actingcommand-actinglab session_record_promote -- --nocapture`
- `cargo test -p actingcommand-actinglab session_record -- --nocapture`
- `cargo test -p actingcommand-actinglab`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace -- -D warnings`
- `cargo test --workspace`
- `git diff --check`
- Source-only added-code prohibited-feature scan over `apps/actinglab/src/main.rs` for ADB shell input/screencap, MaaTouch startup, direct tap/swipe execution, SQLite, OCR/OpenCV, fallback, reconnect, and retry.

### Test results

- `cargo test -p actingcommand-actinglab session_record_promote -- --nocapture` passed with `1` test.
- The promote test also verified overwrite guard, `--force` task replacement, preservation of shared `operations/resources.json`, repo/ours resolution, and `package build-task --dry-run` compatibility.
- `cargo test -p actingcommand-actinglab session_record -- --nocapture` passed with `30` tests.
- `cargo test -p actingcommand-actinglab` passed with `138` tests.
- `cargo fmt --all -- --check` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed.
- `git diff --check` passed.
- Source-only added-code prohibited-feature scan returned `NO_PROHIBITED_CODE_ADDED_LINES`.

### Current blocker

- No blocker for the resource promotion implementation.
- Full Phase D remains incomplete: live prepared-emulator validation, additional recording resource kinds, UI/API wiring, and SQLite metadata remain future work.

### Next step

1. Commit and push this Runtime milestone.
2. Add checkpoint tag `checkpoint/20260627-session-record-promote`.
3. Continue Phase D with live prepared-emulator validation or additional recording resource kinds after this promotion path is accepted.

## 2026-06-27 ActingLab session recording candidate preview

### Current status

- Advanced Phase D recording with a read-only candidate preview command.
- `session record candidates <step-id>` now lists a recorded anchor step's `evaluation.auto_region` report.
- `session record candidate-list <step-id>` is accepted as an alias for the same read-only path.
- The output includes record id, task id, instance, record status, step id, anchor id, current region, evaluation status, full auto-region report, candidate count, selected index, and record path.
- Candidate preview requires an existing auto-region candidate report and fails visibly when the step has no report.
- The command is exposed through capabilities.
- No resource repository write, MaaTouch startup, click/navigation execution, frame capture, OCR, SQLite, UI, or game logic was added.

### Resource mirrors used

- Runtime baseline before this task: `1acdbc67f9665b08f1770eef1cf7c08132f893ae`.
- Resource repositories were not read or used by this implementation step.

### Files changed

- `apps/actinglab/src/main.rs`
- `PLANS.md`
- `CHECKPOINT.md`

### Commands run

- Read `C:\Users\Alice\.codex\plugins\cache\personal\ecc\2.0.0\skills\rust-patterns\SKILL.md`.
- `git fetch --prune --tags origin`
- `git pull --ff-only origin main`
- `git status --short --branch`
- `git log -1 --oneline --decorate`
- Read current `PLANS.md` and `CHECKPOINT.md`.
- `cargo fmt --all`
- `cargo test -p actingcommand-actinglab session_record_candidates -- --nocapture`
- `cargo test -p actingcommand-actinglab session_record -- --nocapture`
- `cargo test -p actingcommand-actinglab`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace -- -D warnings`
- `cargo test --workspace`
- `git diff --check`
- Source-only added-code prohibited-feature scan over `apps/actinglab/src/main.rs` for ADB shell input/screencap, MaaTouch startup, direct tap/swipe execution, SQLite, OCR/OpenCV, fallback, reconnect, and retry.

### Test results

- `cargo test -p actingcommand-actinglab session_record_candidates -- --nocapture` passed with `2` tests.
- `cargo test -p actingcommand-actinglab session_record -- --nocapture` passed with `29` tests.
- `cargo test -p actingcommand-actinglab` passed with `137` tests.
- `cargo fmt --all -- --check` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed.
- `git diff --check` passed.
- Source-only added-code prohibited-feature scan returned `NO_PROHIBITED_CODE_ADDED_LINES`.

### Current blocker

- No blocker for the candidate preview implementation.
- Full Phase D remains incomplete: live prepared-emulator validation, resource promotion/write flow, additional recording resource kinds, UI/API wiring, and SQLite metadata remain future work.

### Next step

1. Commit and push this Runtime milestone.
2. Add checkpoint tag `checkpoint/20260627-session-record-candidate-preview`.
3. Continue Phase D with live prepared-emulator validation or resource-promotion flow after this candidate preview command is accepted.

## 2026-06-27 ActingLab session recording amend-by-candidate loop

### Current status

- Advanced Phase D recording by letting operators confirm or override auto-region suggestions by candidate index.
- `session record amend <step> --candidate-index <n>` now selects a candidate from the step's recorded `evaluation.auto_region.candidates`.
- `--auto-candidate <n>` is accepted as an alias for the same selection path.
- Candidate selection requires an existing auto-region candidate report and fails visibly when the step has no candidate report.
- The selected candidate becomes the step's explicit rect region.
- `evaluation.auto_region` remains present after candidate selection, with `selected_reason=operator_selected_candidate` and exactly one selected candidate.
- The existing source-frame provenance is reused to re-crop, rewrite the artifact, and re-run self/contrast backtests immediately after candidate selection.
- Bad or ambiguous operator choices are not hidden: the final contrast/self backtest can fail visibly.
- No resource repository write, MaaTouch startup, click/navigation execution, OCR, SQLite, UI, or game logic was added.

### Resource mirrors used

- Runtime baseline before this task: `30dccb236eaaac97cb9d523e11a0218a4f42f17f`.
- Resource repositories were not read or used by this implementation step.

### Files changed

- `apps/actinglab/src/main.rs`
- `PLANS.md`
- `CHECKPOINT.md`

### Commands run

- Read `C:\合作工作区\ActingCommand\TASK-Lab-session-layer.md`.
- Read `C:\合作工作区\ActingCommand\FINDING-AK-game-freeze-2026-06-27.md`.
- Read current `PLANS.md` and `CHECKPOINT.md`.
- `git fetch --prune --tags origin`
- `git pull --ff-only origin main`
- `git status --short --branch`
- `git log -1 --oneline --decorate`
- `cargo fmt --all`
- `cargo test -p actingcommand-actinglab session_record_amend -- --nocapture`
- `cargo test -p actingcommand-actinglab session_record_step_anchor_auto -- --nocapture`
- `cargo test -p actingcommand-actinglab session_record -- --nocapture`
- `cargo test -p actingcommand-actinglab`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace -- -D warnings`
- `cargo test --workspace`
- `git diff --check`
- Source-only added-code prohibited-feature scan over `apps/actinglab/src/main.rs` for ADB shell input/screencap, MaaTouch startup, direct tap/swipe execution, SQLite, OCR/OpenCV, fallback, reconnect, and retry.

### Test results

- `cargo test -p actingcommand-actinglab session_record_amend -- --nocapture` passed with `6` tests.
- `cargo test -p actingcommand-actinglab session_record_step_anchor_auto -- --nocapture` passed with `3` tests.
- `cargo test -p actingcommand-actinglab session_record -- --nocapture` passed with `27` tests.
- `cargo test -p actingcommand-actinglab` passed with `135` tests.
- `cargo fmt --all -- --check` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed.
- `git diff --check` passed.
- Source-only added-code prohibited-feature scan returned `NO_PROHIBITED_CODE_ADDED_LINES`.

### Current blocker

- No blocker for the amend-by-candidate implementation.
- Full Phase D remains incomplete: candidate preview UI/API, live prepared-emulator validation, resource promotion/write flow, additional recording resource kinds, UI/API wiring, and SQLite metadata remain future work.

### Next step

1. Commit and push this Runtime milestone.
2. Add checkpoint tag `checkpoint/20260627-session-record-amend-candidate`.
3. Continue Phase D with candidate preview UI/API, live prepared-emulator validation, or resource-promotion flow after this amend-by-candidate loop is accepted.

## 2026-06-27 ActingLab session recording auto-region candidate report

### Current status

- Advanced Phase D recording by making auto-region selection explainable and contrast-aware.
- Frame-backed `session record step --kind anchor --region auto` now writes `evaluation.auto_region` metadata.
- The metadata includes strategy, selected reason, selected rect, candidate rects, per-candidate luma variance, and contrast score/pass state when a contrast frame is supplied.
- With a contrast frame, auto-region selection prefers candidates rejected by the contrast frame before final artifact materialization and self/contrast backtest.
- If no candidate is rejected by the contrast frame, selection falls back to the lowest contrast score and still lets the existing final contrast backtest fail visibly if the result is ambiguous.
- No-frame `--region auto` remains deferred with reason `frame_not_provided` and no fabricated artifact.
- No resource repository write, MaaTouch startup, click/navigation execution, OCR, SQLite, UI, or game logic was added.

### Resource mirrors used

- Runtime baseline before this task: `ee4ba27579a3f0c5d83255b4b3bfcf02f9a5db0d`.
- Resource repositories were not read or used by this implementation step.

### Files changed

- `apps/actinglab/src/main.rs`
- `PLANS.md`
- `CHECKPOINT.md`

### Commands run

- Read `C:\合作工作区\ActingCommand\TASK-Lab-session-layer.md`.
- Read `C:\合作工作区\ActingCommand\FINDING-AK-game-freeze-2026-06-27.md`.
- Read current `PLANS.md` and `CHECKPOINT.md`.
- `git fetch --prune --tags origin`
- `git pull --ff-only origin main`
- `git status --short --branch`
- `git log -1 --oneline --decorate`
- `cargo fmt --all`
- `cargo test -p actingcommand-actinglab session_record_step_anchor_auto -- --nocapture`
- `cargo test -p actingcommand-actinglab session_record -- --nocapture`
- `cargo test -p actingcommand-actinglab`
- `cargo fmt --all -- --check`
- First `cargo clippy --workspace -- -D warnings`
- Rerun `cargo fmt --all`
- Rerun `cargo test -p actingcommand-actinglab session_record -- --nocapture`
- Rerun `cargo clippy --workspace -- -D warnings`
- Rerun `cargo test -p actingcommand-actinglab`
- Rerun `cargo fmt --all -- --check`
- `cargo test --workspace`
- `git diff --check`
- Source-only added-code prohibited-feature scan over `apps/actinglab/src/main.rs` for ADB shell input/screencap, MaaTouch startup, direct tap/swipe execution, SQLite, OCR/OpenCV, fallback, reconnect, and retry.

### Test results

- `cargo test -p actingcommand-actinglab session_record_step_anchor_auto -- --nocapture` passed with `3` tests.
- `cargo test -p actingcommand-actinglab session_record -- --nocapture` passed with `25` tests.
- `cargo test -p actingcommand-actinglab` passed with `133` tests.
- `cargo fmt --all -- --check` passed.
- First `cargo clippy --workspace -- -D warnings` failed because `materialize_anchor_artifact_from_source` had too many arguments after auto-region metadata was added.
- Auto-region rect and metadata are now grouped into `SessionRecordAnchorRegionResolution`.
- Rerun `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed.
- `git diff --check` passed.
- Source-only added-code prohibited-feature scan returned `NO_PROHIBITED_CODE_ADDED_LINES`.

### Current blocker

- No blocker for the auto-region candidate report implementation.
- Full Phase D remains incomplete: explicit operator candidate selection, live prepared-emulator validation, resource promotion/write flow, additional recording resource kinds, UI/API wiring, and SQLite metadata remain future work.

### Next step

1. Commit and push this Runtime milestone.
2. Add checkpoint tag `checkpoint/20260627-session-record-auto-region-report`.
3. Continue Phase D with explicit operator candidate selection, live prepared-emulator validation, or resource-promotion flow after this report format is accepted.

## 2026-06-27 ActingLab session recording auto-region candidate slice

### Current status

- Advanced Phase D recording by making frame-backed `session record step --kind anchor --region auto` usable.
- `--region auto` now resolves to a deterministic rect candidate when a local source frame or explicit current capture frame is provided.
- The selected rect is stored back into the recorded anchor step as `mode=rect`, making downstream draft bundles receive usable coordinates.
- Auto-region selection uses a bounded local brightness-variance candidate grid and then reuses the existing crop/artifact/self-backtest/contrast-backtest path.
- No-frame `--region auto` remains an explicit deferred authoring state with reason `frame_not_provided`; it does not fabricate an artifact or successful evaluation.
- Frame-backed amend paths can resolve lingering `auto` region metadata through the same recorded source-frame path.
- No resource repository write, MaaTouch startup, click/navigation execution, OCR, SQLite, UI, or game logic was added.

### Resource mirrors used

- Runtime baseline before this task: `580c19837992ff6ca4aaf5abc44d16cc7faa16ac`.
- Resource repositories were not read or used by this implementation step.

### Files changed

- `apps/actinglab/src/main.rs`
- `PLANS.md`
- `CHECKPOINT.md`

### Commands run

- `git status --short --branch`
- `git log -1 --oneline --decorate`
- `rg -n "auto region|SessionRecordRegion|materialize_anchor_artifact|session_record_step_anchor_frame_requires_rect_region|record step" apps/actinglab/src/main.rs`
- Read current `PLANS.md` and `CHECKPOINT.md`.
- `cargo fmt --all`
- `cargo test -p actingcommand-actinglab session_record_step_anchor_auto -- --nocapture`
- `cargo test -p actingcommand-actinglab session_record -- --nocapture`
- `cargo test -p actingcommand-actinglab`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace -- -D warnings`
- `cargo test --workspace`
- `git diff --check`
- Source-only added-code prohibited-feature scan over `apps/actinglab/src/main.rs` for ADB shell input/screencap, MaaTouch startup, direct tap/swipe execution, SQLite, OCR/OpenCV, fallback, reconnect, and retry.

### Test results

- `cargo test -p actingcommand-actinglab session_record_step_anchor_auto -- --nocapture` passed with `2` tests.
- `cargo test -p actingcommand-actinglab session_record -- --nocapture` passed with `24` tests.
- `cargo test -p actingcommand-actinglab` passed with `132` tests.
- `cargo fmt --all -- --check` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed.
- `git diff --check` passed.
- Source-only added-code prohibited-feature scan returned `NO_PROHIBITED_CODE_ADDED_LINES`.

### Current blocker

- No blocker for the auto-region candidate implementation.
- Full Phase D remains incomplete: richer candidate review, live prepared-emulator validation, resource promotion/write flow, additional recording resource kinds, UI/API wiring, and SQLite metadata remain future work.

### Next step

1. Commit and push this Runtime milestone.
2. Add checkpoint tag `checkpoint/20260627-session-record-auto-region`.
3. Continue Phase D with richer candidate review, live prepared-emulator validation, or resource-promotion flow after this auto-region slice is accepted.

## 2026-06-27 ActingLab session recording amend re-backtest loop

### Current status

- Advanced Phase D recording by making frame-backed `session record amend` immediately re-test anchor changes.
- Anchor amendments now reuse existing `frame_provenance.path` instead of requiring a new capture.
- When a frame-backed anchor is amended, Runtime re-reads the recorded source frame, re-crops the anchor artifact, rewrites the artifact PNG, and re-runs self/contrast backtests.
- Existing source-frame provenance is preserved across amend, including source type, path, capture backend, freshness metadata, capture attempts, and original recorded timestamp.
- No-frame anchors still remain explicit and deferred, now with reason `amended_without_frame_provenance`.
- Missing or unreadable recorded source frames now fail visibly during amend instead of silently preserving stale evaluation.
- Operation amend behavior is unchanged.
- No device I/O, current capture, resource repository write, MaaTouch startup, click/navigation execution, OCR, SQLite, UI, or game logic was added.

### Resource mirrors used

- Runtime baseline before this task: `b657ac7f2a57d32b29362c4607d7732c3fc80b25`.
- Resource repositories were not read or used by this implementation step.

### Files changed

- `apps/actinglab/src/main.rs`
- `PLANS.md`
- `CHECKPOINT.md`

### Commands run

- Read repo-local `AGENTS.md`, `PLANS.md`, `CHECKPOINT.md`, and `NOTICE.md`; `LICENSE_POLICY.md` does not exist in this repository.
- `git fetch --prune --tags origin`
- `git pull --ff-only origin main`
- `git status --short --branch`
- `git log -1 --oneline --decorate`
- `cargo fmt --all`
- `cargo test -p actingcommand-actinglab session_record_amend -- --nocapture`
- `cargo test -p actingcommand-actinglab session_record -- --nocapture`
- `cargo test -p actingcommand-actinglab`
- `cargo fmt --all -- --check`
- First `cargo clippy --workspace -- -D warnings`
- Rerun `cargo fmt --all`
- Rerun `cargo clippy --workspace -- -D warnings`
- `cargo test --workspace`
- Rerun `cargo fmt --all -- --check`
- `git diff --check`
- Source-only added-code prohibited-feature scan over `apps/actinglab/src/main.rs` for ADB shell input/screencap, MaaTouch startup, direct tap/swipe execution, SQLite, OCR/OpenCV, fallback, reconnect, and retry.

### Test results

- `cargo test -p actingcommand-actinglab session_record_amend -- --nocapture` passed with `4` tests.
- `cargo test -p actingcommand-actinglab session_record -- --nocapture` passed with `23` tests.
- `cargo test -p actingcommand-actinglab` passed with `131` tests.
- `cargo fmt --all -- --check` passed.
- First `cargo clippy --workspace -- -D warnings` failed because `amend_anchor_record_step` and `refresh_amended_anchor_artifact` had too many arguments.
- Anchor-amend mutable fields are now grouped into `SessionRecordAnchorAmendTarget`.
- Rerun `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed.
- Rerun `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- Source-only added-code prohibited-feature scan returned `NO_PROHIBITED_CODE_ADDED_LINES`.

### Current blocker

- No blocker for the amend re-backtest loop.
- Full Phase D remains incomplete: live prepared-emulator validation, resource promotion/write flow, `--region auto`, additional recording resource kinds, UI/API wiring, and SQLite metadata remain future work.

### Next step

1. Commit and push this Runtime milestone.
2. Add checkpoint tag `checkpoint/20260627-session-record-amend-rebacktest`.
3. Continue Phase D with live prepared-emulator validation or resource-promotion flow after this amend loop is accepted.

## 2026-06-27 ActingLab session recording current-frame inlet

### Current status

- Advanced Phase D recording by adding an explicit current-frame inlet for authorized anchor steps.
- `session record step --kind anchor` now accepts `--capture` and alias `--current-frame`.
- `--capture` reuses the existing CaptureBackend command path and honors `--require-fresh` plus `--fresh-delay-ms`.
- Captured source frames are saved under the recording artifact directory before anchor cropping.
- Anchor frame provenance now records source type, source path, SHA-256, dimensions, capture backend, freshness data, and capture attempts.
- Local `--frame` / `--source-frame` behavior remains available and uses the same crop/backtest helper as current-capture frames.
- Local frame input and current-capture input are mutually exclusive and fail visibly if combined.
- `session record step` is now marked as both offline and device-capable in command capabilities.
- No resource repository write, MaaTouch startup, click/navigation execution, OCR, SQLite, UI, or game logic was added.

### Resource mirrors used

- Runtime baseline before this task: `1a8be32a19c445dc14d2c39abff750b77be29c24`.
- Resource repositories were not read or used by this implementation step.

### Files changed

- `apps/actinglab/src/main.rs`
- `PLANS.md`
- `CHECKPOINT.md`

### Commands run

- Re-read `C:\合作工作区\ActingCommand\TASK-Lab-session-layer.md`.
- Re-read `C:\合作工作区\ActingCommand\FINDING-AK-game-freeze-2026-06-27.md`.
- Read repo-local `AGENTS.md`, `PLANS.md`, `CHECKPOINT.md`, and `NOTICE.md`; `LICENSE_POLICY.md` does not exist in this repository.
- `git fetch --prune --tags origin`
- `git pull --ff-only origin main`
- `git status --short --branch`
- `git log -1 --oneline --decorate`
- `cargo fmt --all`
- `cargo test -p actingcommand-actinglab session_record -- --nocapture`
- `cargo test -p actingcommand-actinglab`
- `cargo fmt --all -- --check`
- First `cargo clippy --workspace -- -D warnings`
- Rerun `cargo fmt --all`
- Rerun `cargo clippy --workspace -- -D warnings`
- `cargo test --workspace`
- Rerun `cargo fmt --all -- --check`
- `git diff --check`
- Source-only added-code prohibited-feature scan over `apps/actinglab/src/main.rs` for ADB shell input/screencap, MaaTouch startup, direct tap/swipe execution, SQLite, OCR/OpenCV, fallback, reconnect, and retry.

### Test results

- `cargo test -p actingcommand-actinglab session_record -- --nocapture` passed with `22` tests.
- `cargo test -p actingcommand-actinglab` passed with `130` tests.
- `cargo fmt --all -- --check` passed.
- First `cargo clippy --workspace -- -D warnings` failed because `materialize_anchor_artifact` had too many arguments and one `create_dir_all` call used a needless generic borrow.
- The helper signature now uses a small `SessionRecordStepContext`, and the needless borrow was removed.
- Rerun `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed.
- Rerun `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- Source-only added-code prohibited-feature scan returned `NO_PROHIBITED_CODE_ADDED_LINES`.

### Current blocker

- No blocker for the current-frame recording inlet.
- Full Phase D remains incomplete: live prepared-emulator validation, resource promotion/write flow, `--region auto`, additional recording resource kinds, UI/API wiring, and SQLite metadata remain future work.

### Next step

1. Commit and push this Runtime milestone.
2. Add checkpoint tag `checkpoint/20260627-session-record-current-frame-inlet`.
3. Continue Phase D with live prepared-emulator validation or resource-promotion flow after this current-frame inlet is accepted.

## 2026-06-27 ActingLab session recording package handoff

### Current status

- Closed an offline handoff gap between `session record build-task` and the existing `package build-task` pipeline.
- The successful recording build-task test now immediately runs `package build-task --dry-run` against the generated draft repo.
- Generated drafts now use numeric `defaults.color_max_distance` so the generated recognition pack validates.
- `record build-task` now validates operation point-click coordinates against the inferred or supplied `coordinate_space`.
- `record build-task` now validates that operation `from`, `to`, `entry_page`, and `target_page` references have matching anchors.
- Page-reference validation accepts the same `any` and `<page>_variant` style that the existing converter recognizes.
- Unresolved target-click operations still fail before page-reference validation.
- The reorganized resource-root detect-page test is now isolated with the existing test environment lock to avoid reading another test's temporary `ACTINGLAB_CONFIG_PATH`.
- No device I/O, MaaTouch startup, live screenshot capture, resource repository write, OCR, SQLite, UI, or game logic was added.

### Resource mirrors used

- Runtime baseline before this task: `7537c67ca2d98f91138bd6d1942fa64527587eb3`.
- Resource repositories were not read or used by this implementation step.

### Files changed

- `apps/actinglab/src/main.rs`
- `PLANS.md`
- `CHECKPOINT.md`

### Commands run

- Read the current cooperation workspace task document `TASK-Lab-session-layer.md`.
- Read the current cooperation workspace finding document `FINDING-AK-game-freeze-2026-06-27.md`.
- Read repo-local `AGENTS.md`, `PLANS.md`, `CHECKPOINT.md`, and `NOTICE.md`; `LICENSE_POLICY.md` does not exist in this repository.
- `git fetch --prune --tags origin`
- `git pull --ff-only origin main`
- `git status --short --branch`
- `git log -1 --oneline --decorate`
- `cargo fmt --all`
- First `cargo test -p actingcommand-actinglab session_record_build_task_writes_draft_bundle -- --nocapture`
- Rerun `cargo fmt --all`
- First `cargo test -p actingcommand-actinglab session_record_build_task -- --nocapture`
- Rerun `cargo fmt --all`
- Rerun `cargo test -p actingcommand-actinglab session_record_build_task -- --nocapture`
- `cargo test -p actingcommand-actinglab session_record -- --nocapture`
- `cargo test -p actingcommand-actinglab`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace -- -D warnings`
- First `cargo test --workspace`
- Rerun `cargo fmt --all`
- Rerun `cargo test --workspace`
- Rerun `cargo fmt --all -- --check`
- Rerun `cargo clippy --workspace -- -D warnings`
- `git diff --check`
- Source-only added-code prohibited-feature scan over `apps/actinglab/src/main.rs` for ADB input fallback, `adb shell screencap`, MaaTouch startup, live capture routing, SQLite, OCR/OpenCV, fallback, reconnect, and retry.

### Test results

- First package handoff test failed because the fixture used a 12x10 coordinate space but clicked 100,200; the success fixture now uses an in-bounds click.
- `record build-task` now rejects out-of-bounds point clicks before a package dry-run.
- First package dry-run then failed because generated defaults used `color_max_distance: null`; generated drafts now use numeric `20.0`.
- Package dry-run then failed because operation `to` referenced `page/mail` without a matching anchor; the success fixture now includes a mail anchor and `record build-task` rejects missing page anchors explicitly.
- `cargo test -p actingcommand-actinglab session_record_build_task -- --nocapture` passed with `5` tests.
- `cargo test -p actingcommand-actinglab session_record -- --nocapture` passed with `20` tests.
- `cargo test -p actingcommand-actinglab` passed with `128` tests.
- `cargo fmt --all -- --check` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- First `cargo test --workspace` failed in `detect_page_accepts_reorganized_repo_root_resource_root` because that existing test could observe another test's temporary empty `ACTINGLAB_CONFIG_PATH` during parallel execution.
- `detect_page_accepts_reorganized_repo_root_resource_root` now uses the existing `ENV_LOCK` test guard.
- Rerun `cargo test --workspace` passed.
- Rerun `cargo fmt --all -- --check` passed.
- Rerun `cargo clippy --workspace -- -D warnings` passed.
- `git diff --check` passed.
- Source-only added-code prohibited-feature scan returned `NO_PROHIBITED_CODE_ADDED_LINES`.

### Current blocker

- No blocker for offline package handoff.
- Full recording remains incomplete: live current-frame integration, resource promotion, UI/API wiring, and SQLite metadata are still future work.

### Next step

1. Commit and push this Runtime milestone.
2. Add checkpoint tag `checkpoint/20260627-session-record-package-handoff`.
3. Continue Phase D current-frame/resource-promotion work only after this offline handoff is accepted.

## 2026-06-27 ActingLab session recording anchor contrast validation

### Current status

- Added optional offline contrast-frame validation for frame-backed `session record step --kind anchor`.
- `--contrast-frame <png>` and alias `--negative-frame <png>` can now be supplied with `--frame <png>`.
- Source-frame self-backtest remains unchanged when no contrast frame is supplied.
- When a contrast frame is supplied, the anchor evaluation now includes `contrast_backtest` metadata:
  - source
  - path
  - SHA-256
  - dimensions
  - metric
  - region
  - match point
  - raw score
  - normalized score
  - threshold
  - pass/fail boolean
- A frame-backed anchor now passes with a contrast frame only when the source self-test passes and the contrast-frame score is below threshold.
- A contrast frame that also matches marks the anchor `failed` with reason `contrast_backtest_matched`.
- Anchor amendment clears both self-backtest and contrast-backtest metadata.
- No device I/O, MaaTouch startup, live screenshot capture, resource repository write, OCR, SQLite, UI, or game logic was added.

### Resource mirrors used

- Runtime baseline before this task: `e43c0967e43ca207becb4d34e759a32287a8fc2c`.
- Resource repositories were not read or used by this implementation step.

### Files changed

- `apps/actinglab/src/main.rs`
- `PLANS.md`
- `CHECKPOINT.md`

### Commands run

- Read the current cooperation workspace task document `TASK-Lab-session-layer.md`.
- Read the current cooperation workspace finding document `FINDING-AK-game-freeze-2026-06-27.md`.
- Read repo-local `AGENTS.md`, `PLANS.md`, `CHECKPOINT.md`, and `NOTICE.md`; `LICENSE_POLICY.md` does not exist in this repository.
- `git fetch --prune --tags origin`
- `git pull --ff-only origin main`
- `git status --short --branch`
- `git log -1 --oneline --decorate`
- `cargo fmt --all`
- `cargo test -p actingcommand-actinglab session_record_step_anchor -- --nocapture`
- `cargo test -p actingcommand-actinglab session_record -- --nocapture`
- `cargo test -p actingcommand-actinglab`
- `cargo fmt --all -- --check`
- First `cargo clippy --workspace -- -D warnings`
- Rerun `cargo fmt --all`
- Rerun `cargo clippy --workspace -- -D warnings`
- Rerun `cargo fmt --all -- --check`
- `cargo test --workspace`
- `git diff --check`
- Source-only added-code prohibited-feature scan over `apps/actinglab/src/main.rs` for ADB input fallback, `adb shell screencap`, MaaTouch startup, live capture routing, SQLite, OCR/OpenCV, fallback, reconnect, and retry.

### Test results

- `cargo test -p actingcommand-actinglab session_record_step_anchor -- --nocapture` passed with `6` tests.
- `cargo test -p actingcommand-actinglab session_record -- --nocapture` passed with `18` tests.
- `cargo test -p actingcommand-actinglab` passed with `126` tests.
- `cargo fmt --all -- --check` passed.
- First `cargo clippy --workspace -- -D warnings` failed with `clippy::large-enum-variant` after contrast metadata enlarged the anchor variant.
- Anchor-step `evaluation` metadata was boxed; this keeps the JSON shape unchanged while reducing enum size.
- Rerun `cargo clippy --workspace -- -D warnings` passed.
- Rerun `cargo fmt --all -- --check` passed.
- `cargo test --workspace` passed.
- `git diff --check` passed.
- Source-only added-code prohibited-feature scan returned no matches.

### Current blocker

- No blocker for local contrast-frame validation.
- Full recording remains incomplete: live current-frame integration, resource promotion, UI/API wiring, and SQLite metadata are still future work.

### Next step

1. Commit and push this Runtime milestone.
2. Add a checkpoint tag if this is accepted as a stable recording-anchor-contrast rollback point.
3. Next implementation milestone should add current-frame integration or resource-promotion flow after contrast validation semantics are accepted.

## 2026-06-27 ActingLab session recording build-task draft

### Current status

- Added offline `session record build-task --out <dir>` draft bundle generation.
- Build-task consumes an existing local recording context and allows active or stopped records.
- Backtested anchor artifacts are copied into `<out>/operations/<task_id>/assets/`.
- The command writes `<out>/operations/<task_id>/task.json` and `<out>/operations/resources.json`.
- The returned JSON includes the assembled Operation Bundle 0.3-style draft, output paths, anchor count, operation count, and copied assets.
- Build-task rejects unresolved target-click operations; this milestone only exports explicit coordinate clicks.
- Build-task requires anchor artifacts to exist and anchor self-backtests to have passed.
- Build-task infers coordinate space from frame-backed anchor provenance, or requires `--resolution <width>x<height>` when no frame provenance exists.
- Added `--dry-run` validation mode that assembles and returns the bundle without writing files.
- Changed session/lease/record JSON timestamp fields from `u128` to `u64` so persisted JSON records can be read back reliably by `serde_json`.
- No device I/O, MaaTouch startup, live screenshot capture, contrast-frame validation, resource repository write, OCR, SQLite, UI, or game logic was added.

### Resource mirrors used

- Runtime baseline before this task: `64226654a1dd2b1d5bb67eae1f5bc89e32213b12`.
- Resource repositories were not read or used by this implementation step.

### Files changed

- `apps/actinglab/src/main.rs`
- `PLANS.md`
- `CHECKPOINT.md`

### Commands run

- Continued from the already-read cooperation workspace task document `TASK-Lab-session-layer.md`.
- Continued from the already-read cooperation workspace finding document `FINDING-AK-game-freeze-2026-06-27.md`.
- `git status --short --branch`
- `cargo fmt --all`
- `cargo test -p actingcommand-actinglab session_record -- --nocapture`
- `cargo test -p actingcommand-actinglab session_record_build_task_writes_draft_bundle -- --nocapture`
- Rerun `cargo fmt --all`
- Rerun `cargo test -p actingcommand-actinglab session_record -- --nocapture`
- `cargo test -p actingcommand-actinglab`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace -- -D warnings`
- `cargo test --workspace`
- `git diff --check`
- Source-only added-code prohibited-feature scan over `apps/actinglab/src/main.rs` for ADB input fallback, `adb shell screencap`, MaaTouch startup, live capture routing, SQLite, OCR/OpenCV, fallback, reconnect, and retry.

### Test results

- First targeted `session_record` run failed because persisted record JSON used `u128` Unix ms fields that `serde_json` could write but could not read back into the typed session record.
- Session/lease/record JSON timestamp fields were changed to `u64`.
- Rerun `cargo test -p actingcommand-actinglab session_record -- --nocapture` passed with `16` tests.
- `cargo test -p actingcommand-actinglab` passed with `124` tests.
- `cargo fmt --all -- --check` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed.
- `git diff --check` passed.
- Source-only added-code prohibited-feature scan returned no matches.

### Current blocker

- No blocker for local build-task draft generation.
- Full recording remains incomplete: live frame capture/current-frame integration, contrast-frame validation, resource promotion, UI/API wiring, and SQLite metadata are still future work.

### Next step

1. Commit and push this Runtime milestone.
2. Add a checkpoint tag if this is accepted as a stable recording-build-task rollback point.
3. Next implementation milestone should add current-frame/contrast validation or resource-promotion flow after the draft bundle shape is accepted.

## 2026-06-27 ActingLab session recording anchor self-backtest

### Current status

- Added offline self-backtest for frame-backed `session record step --kind anchor`.
- Frame-backed anchor crops now match the generated crop PNG against the supplied source frame inside the authorized rect.
- Anchor evaluation now records `passed` or `failed`, reason, source, metric, rect, match point, raw score, normalized score, effective threshold, and pass/fail boolean.
- Metadata-only anchor steps now remain `deferred` with reason `frame_not_provided`.
- Anchor amend resets evaluation to `deferred` and clears any prior backtest metadata.
- Reused existing recognition primitives and existing metric parsing; no new recognition algorithm was added.
- `session record build-task` remains explicit `record_authoring_not_implemented`.
- No device I/O, MaaTouch startup, live screenshot capture, contrast-frame validation, resource write, task bundle generation, OCR, SQLite, UI, or game logic was added.

### Resource mirrors used

- Runtime baseline before this task: `24edd8b5c655f7bae6d322791c8624926561a61b`.
- Resource repositories were not read or used by this implementation step.

### Files changed

- `apps/actinglab/src/main.rs`
- `PLANS.md`
- `CHECKPOINT.md`

### Commands run

- Read the current cooperation workspace task document `TASK-Lab-session-layer.md`.
- Read the current cooperation workspace finding document `FINDING-AK-game-freeze-2026-06-27.md`.
- `git fetch --prune --tags origin`
- `git status --short --branch`
- `git rev-list --left-right --count HEAD...origin/main`
- `git log -1 --oneline --decorate`
- `cargo fmt --all`
- `cargo test -p actingcommand-actinglab session_record_step_anchor -- --nocapture`
- `cargo test -p actingcommand-actinglab session_record -- --nocapture`
- `cargo test -p actingcommand-actinglab`
- `cargo fmt --all -- --check`
- `git diff --check`
- First `cargo test --workspace`
- `cargo clippy --workspace -- -D warnings`
- `cargo test -p actingcommand-actinglab lab_run::tests::lab_validate_accepts_minimal_self_contained_package -- --nocapture`
- Rerun `cargo test --workspace`
- Added-code prohibited-feature scan over source changes for ADB input fallback, `adb shell screencap`, MaaTouch startup, SQLite, OCR/OpenCV, fallback, reconnect, retry, and live capture routing.

### Test results

- First targeted compile failed because a duplicate `match_metric_name` helper was added; the duplicate was removed and the existing helper is reused.
- First targeted test run failed because the test compared an f32 threshold serialized as JSON to exact `0.95`; the test was changed to a bounded float comparison.
- `cargo test -p actingcommand-actinglab session_record_step_anchor -- --nocapture` passed with `4` tests.
- `cargo test -p actingcommand-actinglab session_record -- --nocapture` passed with `14` tests.
- `cargo test -p actingcommand-actinglab` passed with `122` tests.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- First `cargo test --workspace` was run in parallel with clippy and failed one `lab_validate_accepts_minimal_self_contained_package` test that had already passed in the package run.
- The failing test passed immediately when rerun alone.
- Rerun `cargo test --workspace` by itself passed.
- `cargo clippy --workspace -- -D warnings` passed.
- Source-only added-code prohibited-feature scan returned no matches.

### Current blocker

- No blocker for offline anchor self-backtest.
- Full recording remains incomplete: live frame capture/current-frame integration, contrast-frame validation, build-task/package output, and resource writes are still future work.

### Next step

1. Commit and push this Runtime milestone.
2. Add a checkpoint tag if this is accepted as a stable recording-anchor-self-backtest rollback point.
3. Next implementation milestone should add capture/current-frame integration, contrast-frame validation, or `record build-task` semantics before resource writes.

## 2026-06-27 ActingLab session recording anchor frame materialization

### Current status

- Added optional local PNG frame materialization for `session record step --kind anchor`.
- `--frame <png>` and `--source-frame <png>` attach frame provenance to the authorized anchor step.
- `--artifact-dir <dir>` can override the default draft artifact directory.
- Frame-backed anchors require a rect region; `--region auto` with a frame fails visibly because automatic candidate selection is not implemented.
- Rect crops are bounds-checked, encoded as draft PNG artifacts, and recorded with path, hash, dimensions, and source region.
- Metadata-only anchor steps still work when no frame is supplied.
- `session record build-task` remains explicit `record_authoring_not_implemented`.
- No device I/O, MaaTouch startup, live screenshot capture, resource write, task bundle generation, OCR, recognition change, SQLite, UI, or game logic was added.

### Resource mirrors used

- Runtime baseline before this task: `d1e2daa908916fc6f093669296819424c4df99da`.
- Resource repositories were not read or used by this implementation step.

### Files changed

- `apps/actinglab/src/main.rs`
- `PLANS.md`
- `CHECKPOINT.md`

### Commands run

- Continued from the already-read cooperation workspace task document `TASK-Lab-session-layer.md`.
- Continued from the already-read cooperation workspace finding document `FINDING-AK-game-freeze-2026-06-27.md`.
- `git status --short --branch`
- `git log -3 --oneline --decorate`
- `cargo fmt --all`
- `cargo test -p actingcommand-actinglab session_record_step_anchor -- --nocapture`
- `cargo test -p actingcommand-actinglab session_record -- --nocapture`
- `cargo test -p actingcommand-actinglab`
- `cargo test --workspace`
- `cargo fmt --all -- --check`
- `git diff --check`
- `cargo clippy --workspace -- -D warnings`
- Rerun `cargo fmt --all`
- Rerun `cargo test -p actingcommand-actinglab session_record_step_anchor -- --nocapture`
- Rerun `cargo clippy --workspace -- -D warnings`
- Rerun `cargo fmt --all -- --check`
- Rerun `git diff --check`
- Rerun `cargo test -p actingcommand-actinglab session_record -- --nocapture`
- Rerun `cargo test --workspace`
- Added-code prohibited-feature scan over source changes for ADB input fallback, `adb shell screencap`, MaaTouch startup, SQLite, OCR/OpenCV, fallback, reconnect, retry, and live capture routing.

### Test results

- First targeted test run failed at compile time because `amend_session_record_step` still matched the old anchor fields; the match was fixed with `..`.
- `cargo test -p actingcommand-actinglab session_record_step_anchor -- --nocapture` passed with `4` tests.
- `cargo test -p actingcommand-actinglab session_record -- --nocapture` passed with `14` tests.
- `cargo test -p actingcommand-actinglab` passed with `122` tests.
- `cargo test --workspace` passed.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- The first `cargo clippy --workspace -- -D warnings` found a large enum variant after adding anchor artifact metadata; optional `frame_provenance` and `artifact` fields were boxed without changing the JSON shape.
- Rerun `cargo clippy --workspace -- -D warnings` passed.
- A broad prohibited-feature scan over docs and code matched only documentation statements saying SQLite/OCR were not added.
- Source-only added-code prohibited-feature scan returned no matches.

### Current blocker

- No blocker for the local frame materialization implementation.
- Full recording remains incomplete: live frame capture/current-frame integration, anchor backtest, build-task/package output, and resource writes are still future work.

### Next step

1. Commit and push this Runtime milestone.
2. Add a checkpoint tag if this is accepted as a stable recording-anchor-frame rollback point.
3. Next implementation milestone should add capture/current-frame integration or anchor backtest semantics before resource writes.

## 2026-06-27 ActingLab session recording amend schema

### Current status

- Added `session record amend <step-id>` and `session record amend --step-id <id>`.
- Anchor steps can now amend:
  - `--id <page>`
  - `--region <auto|x,y,width,height>`
  - `--color-check`
  - `--no-color-check`
  - `--threshold <0..1>`
  - `--clear-threshold`
- Anchor amendment resets evaluation to `deferred` with reason `amended_needs_backtest`.
- Operation steps can now amend:
  - `--from <page>`
  - `--to <page|null>`
  - `--click <x,y|target>`
  - `--destructive`
  - `--non-destructive`
- `SessionRecordStep` now records `updated_at_unix_ms`.
- Missing/inactive recording contexts fail with `record_session_not_active`.
- Missing step ids fail validation.
- Unknown step ids fail with `record_step_not_found`.
- Amend commands that do not include a supported field for the target step kind fail validation instead of silently succeeding.
- `session record build-task` remains explicit `record_authoring_not_implemented`.
- `capabilities` now advertises `session record amend` as offline available.
- No device I/O, MaaTouch startup, screenshot capture, frame crop, resource write, task bundle generation, OCR, recognition change, SQLite, UI, or game logic was added.

### Resource mirrors used

- Runtime baseline before this task: `0559e16d92445c2566289a9d75428e2f960f61de`.
- Resource repositories were not read or used by this implementation step.

### Files changed

- `apps/actinglab/src/main.rs`
- `PLANS.md`
- `CHECKPOINT.md`

### Commands run

- Read `C:\合作工作区\ActingCommand\TASK-Lab-session-layer.md`.
- Read `C:\合作工作区\ActingCommand\FINDING-AK-game-freeze-2026-06-27.md`.
- Read repo-local `AGENTS.md`, `PLANS.md`, `CHECKPOINT.md`, and `NOTICE.md`; `LICENSE_POLICY.md` does not exist in this repository.
- `git fetch --prune --tags origin`
- `git pull --ff-only origin main`
- `cargo fmt --all`
- `cargo test -p actingcommand-actinglab session_record -- --nocapture`
- `cargo test -p actingcommand-actinglab`
- `cargo fmt --all -- --check`
- `cargo test --workspace`
- `cargo clippy --workspace -- -D warnings`
- `git diff --check`
- Added-code prohibited-feature scan for ADB input fallback, `adb shell screencap`, SQLite, OCR, OpenCV, fallback, reconnect, retry, screenshot backend additions, and MaaTouch startup additions.
- `git status --short --branch`
- `git diff --stat`

### Test results

- `cargo test -p actingcommand-actinglab session_record -- --nocapture` passed with `11` tests.
- `cargo test -p actingcommand-actinglab` passed with `119` tests.
- `cargo fmt --all -- --check` passed.
- `cargo test --workspace` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `git diff --check` passed.
- Added-code prohibited-feature scan returned no matches.

### Current blocker

- No blocker for the recording amend schema.
- Full recording remains incomplete: anchor capture/crop/backtest, build-task/package output, resource writes, and screenshot provenance are still future work.

### Next step

1. Commit and push this Runtime milestone.
2. Add a checkpoint tag if this is accepted as a stable recording-amend rollback point.
3. Next implementation milestone should define frame provenance for anchor capture/crop/backtest before any resource writes.

## 2026-06-27 ActingLab session recording step schema

### Current status

- Added typed recording-step models for active recording contexts.
- `session record step --kind anchor` now appends an authorized anchor step.
- Anchor steps require `--id <page>` and `--region <auto|x,y,width,height>`.
- Anchor steps accept `--color-check` and finite `--threshold <0..1>`.
- Anchor evaluation is stored as `deferred` with reason `capture_and_backtest_not_implemented`.
- `session record step --kind operation` now appends an authorized operation step.
- Operation steps require `--from <page>`, `--to <page|null>`, and `--click <x,y|target>`.
- Operation steps accept `--destructive`.
- `--step-id` is optional; omitted ids are generated as `step-0001`, `step-0002`, and so on.
- Duplicate step ids fail with `record_step_id_conflict`.
- Missing or inactive recording contexts fail with `record_session_not_active`.
- `session record amend|build-task` remain explicit `record_authoring_not_implemented` responses.
- `capabilities` now advertises `session record step` as offline available.
- No device I/O, MaaTouch startup, screenshot capture, frame crop, resource write, task bundle generation, OCR, recognition change, SQLite, UI, or game logic was added.

### Resource mirrors used

- Runtime baseline before this task: `9ea45003a1f5d091ac5232678c83a6b4a8d4aa56`.
- Resource repositories were not read or used by this implementation step.

### Files changed

- `apps/actinglab/src/main.rs`
- `PLANS.md`
- `CHECKPOINT.md`

### Commands run

- Read `C:\合作工作区\ActingCommand\TASK-Lab-session-layer.md`.
- Read `C:\合作工作区\ActingCommand\FINDING-AK-game-freeze-2026-06-27.md`.
- Read repo-local `AGENTS.md`, `PLANS.md`, `CHECKPOINT.md`, and `NOTICE.md`; `LICENSE_POLICY.md` does not exist in this repository.
- `git fetch --prune --tags origin`
- `git pull --ff-only origin main`
- `cargo fmt --all`
- `cargo test -p actingcommand-actinglab session_record -- --nocapture`
- `cargo test -p actingcommand-actinglab`
- `cargo fmt --all -- --check`
- `cargo test --workspace`
- `cargo clippy --workspace -- -D warnings`
- `git diff --check`
- Added-code prohibited-feature scan for ADB input fallback, `adb shell screencap`, SQLite, OCR, OpenCV, fallback, reconnect, retry, screenshot backend additions, and MaaTouch startup additions.
- `git diff --stat`
- `git status --short --branch`

### Test results

- `cargo test -p actingcommand-actinglab session_record -- --nocapture` passed with `8` tests.
- `cargo test -p actingcommand-actinglab` passed with `116` tests.
- `cargo fmt --all -- --check` passed.
- `cargo test --workspace` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `git diff --check` passed.
- Added-code prohibited-feature scan returned no matches.

### Current blocker

- No blocker for the recording step schema.
- Full recording remains incomplete: amend, anchor capture/crop/backtest, build-task/package output, resource writes, and screenshot provenance are still future work.

### Next step

1. Commit and push this Runtime milestone.
2. Add a checkpoint tag if this is accepted as a stable recording-step schema rollback point.
3. Next implementation milestone should define `session record amend` patch semantics or anchor frame-capture provenance before resource writes.

## 2026-06-27 ActingLab session recording context skeleton

### Current status

- Added `session record start|status|stop` as a local offline recording context skeleton.
- `session record start --task-id <id>` writes one structured JSON record context per instance in the selected session state directory.
- Record contexts include schema version, record id, task id, instance, status, optional holder, optional lease id, timestamps, and an empty `steps` array.
- `session record start` returns `auto_recording=false` to make clear that opening a context does not authorize capture or resource writes.
- A second active record start is blocked with `record_session_active` unless `--force` is supplied.
- `session record step|amend|build-task` return explicit `record_authoring_not_implemented` before creating state files.
- `capabilities` now advertises `session record` as offline available while the top-level `record` capability remains reserved.
- No device I/O, MaaTouch startup, screenshot capture, resource write, task bundle generation, OCR, recognition change, SQLite, UI, or game logic was added.

### Resource mirrors used

- Runtime baseline before this task: `5357013eaab48501ed17efb08a3f09a6aa2c7bc3`.
- Resource repositories were not read or used by this implementation step.

### Files changed

- `apps/actinglab/src/main.rs`
- `PLANS.md`
- `CHECKPOINT.md`

### Commands run

- Read `C:\合作工作区\ActingCommand\TASK-Lab-session-layer.md`.
- Read `C:\合作工作区\ActingCommand\FINDING-AK-game-freeze-2026-06-27.md`.
- Read repo-local `AGENTS.md`, `PLANS.md`, `CHECKPOINT.md`, and `NOTICE.md`; `LICENSE_POLICY.md` does not exist in this repository.
- `git fetch --prune --tags origin`
- `git pull --ff-only origin main`
- `cargo fmt --all`
- `cargo test -p actingcommand-actinglab session_record -- --nocapture`
- `cargo test -p actingcommand-actinglab`
- `cargo fmt --all -- --check`
- `cargo test --workspace`
- First `cargo clippy --workspace -- -D warnings`
- `cargo fmt --all`
- Rerun `cargo clippy --workspace -- -D warnings`
- `cargo test -p actingcommand-actinglab session_record`
- `git diff --check`
- First prohibited-feature scan over code diff, which matched only an existing context line.
- Added-code prohibited-feature scan for ADB input fallback, `adb shell screencap`, SQLite, OCR, OpenCV, fallback, reconnect, retry, screenshot backend additions, and MaaTouch startup additions.
- Rerun `cargo test --workspace`

### Test results

- `cargo test -p actingcommand-actinglab session_record -- --nocapture` passed with `4` tests.
- `cargo test -p actingcommand-actinglab` passed with `112` tests.
- `cargo fmt --all -- --check` passed.
- `cargo test --workspace` passed.
- The first `cargo clippy --workspace -- -D warnings` found one collapsible nested `if`; it was fixed without changing the milestone scope.
- Rerun `cargo clippy --workspace -- -D warnings` passed.
- `cargo test -p actingcommand-actinglab session_record` passed with `4` tests after the clippy fix.
- `git diff --check` passed.
- The first prohibited-feature scan matched only the existing diff context line `"capture" => run_capture(global, args)`.
- Added-code prohibited-feature scan returned no matches.
- Final `cargo test --workspace` passed.

### Current blocker

- No blocker for the recording context skeleton.
- Full recording authoring remains incomplete: explicit step authorization, amend, build-task/package output, resource writes, and any screenshot integration are still future work.

### Next step

1. Commit and push this Runtime milestone.
2. Add a checkpoint tag if this is accepted as a stable recording-context rollback point.
3. Next implementation milestone should define explicit `session record step` schemas before any capture/resource write integration.

## 2026-06-27 ActingLab daemon monitor recovery routing

### Current status

- Added `monitor` as a resident daemon request command.
- `monitor --via-daemon` no longer requires `--once`.
- `monitor --via-daemon` without `--recover` remains read-only and does not require a lease.
- `monitor --via-daemon --recover` now submits a bounded monitor request to the daemon and requires matching lease metadata before recovery can run.
- Added explicit `session request monitor`.
- `monitor-once` daemon requests remain read-only; `--recover` on `monitor-once` is still rejected with a visible safety error.
- The daemon `monitor` request reuses the existing bounded `run_monitor_loop` and existing `session recover` maintenance path.
- Recovery-related flags remain part of the inner monitor payload after client-only flags are stripped.
- Wrong holder and wrong lease id fail before capture, MaaTouch, or recovery logic runs.
- No ADB input fallback, reconnect loop, retry loop, OCR, SQLite, UI, scheduler body, recording, capture backend change, recognition algorithm change, or game logic was added.

### Resource mirrors used

- Runtime baseline before this task: `a4257b73205060329f670274fbd88cfe6c4ce991`.
- Resource repositories were not read or used by this implementation step.

### Files changed

- `apps/actinglab/src/main.rs`
- `PLANS.md`
- `CHECKPOINT.md`

### Commands run

- Read `C:\合作工作区\ActingCommand\TASK-Lab-session-layer.md`.
- Read `C:\合作工作区\ActingCommand\FINDING-AK-game-freeze-2026-06-27.md`.
- Read repo-local `AGENTS.md`, `PLANS.md`, `CHECKPOINT.md`, and `NOTICE.md`; `LICENSE_POLICY.md` does not exist in this repository.
- `git fetch --prune --tags origin`
- `git pull --ff-only origin main`
- `cargo fmt --all`
- `cargo test -p actingcommand-actinglab session_monitor`
- `cargo test -p actingcommand-actinglab monitor_via_daemon`
- First `cargo test -p actingcommand-actinglab`
- Rerun `cargo test -p actingcommand-actinglab`
- Local daemon smoke with an `ak` scheduler lease and mismatched recovery holder:
  - `cargo run -q -p actingcommand-actinglab -- --json session start --state-dir <target session-monitor-recover-smoke dir>`
  - `cargo run -q -p actingcommand-actinglab -- --json --instance ak session lease acquire --state-dir <target session-monitor-recover-smoke dir> --holder scheduler --lease-id smoke-lease`
  - `cargo run -q -p actingcommand-actinglab -- --json --instance ak monitor --via-daemon --recover --capture --state-dir <target session-monitor-recover-smoke dir> --lease-holder lab --lease-id smoke-lease --request-timeout-ms 20000`
  - `cargo run -q -p actingcommand-actinglab -- --json session stop --state-dir <target session-monitor-recover-smoke dir>`
- Local daemon smoke with an `ak` scheduler lease and matching recovery holder but intentionally missing resources:
  - `cargo run -q -p actingcommand-actinglab -- --json session start --state-dir <target session-monitor-recover-smoke dir>`
  - `cargo run -q -p actingcommand-actinglab -- --json --instance ak session lease acquire --state-dir <target session-monitor-recover-smoke dir> --holder scheduler --lease-id smoke-lease`
  - `cargo run -q -p actingcommand-actinglab -- --json --instance ak monitor --via-daemon --recover --state-dir <target session-monitor-recover-smoke dir> --lease-holder scheduler --lease-id smoke-lease --request-timeout-ms 20000`
  - `cargo run -q -p actingcommand-actinglab -- --json session stop --state-dir <target session-monitor-recover-smoke dir>`
- `cargo fmt --all -- --check`
- `cargo test --workspace`
- `cargo clippy --workspace -- -D warnings`
- `git diff --check`
- Code diff prohibited-feature scan for ADB input fallback, `adb shell screencap`, SQLite, OCR, OpenCV, fallback, reconnect, retry, and MaaTouch startup additions.

### Test results

- `cargo test -p actingcommand-actinglab session_monitor` passed with `3` tests.
- `cargo test -p actingcommand-actinglab monitor_via_daemon` passed with `2` tests.
- The first `cargo test -p actingcommand-actinglab` run failed two existing resource/config tests with exit code `2`, matching the known parallel environment race pattern.
- Rerun `cargo test -p actingcommand-actinglab` passed with `108` tests.
- Local daemon smoke with mismatched holder returned exit code `3` and safety-blocked `daemon_request_blocked` with message `lease for ak is held by scheduler, not lab`; no capture, recovery, or input was sent.
- Local daemon smoke with matching holder returned exit code `2` and normal `validation_failed` because resources/scene were intentionally absent, proving the request passed the lease gate and reached regular monitor validation.
- `cargo fmt --all -- --check` passed.
- `cargo test --workspace` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `git diff --check` passed.
- Code diff prohibited-feature scan returned no matches.

### Current blocker

- No blocker for daemon monitor recovery request routing and lease enforcement.
- Full live recovery on a real emulator still requires a safe current simulator state and current resource inputs.
- Full Session Layer remains incomplete: scheduler body, always-on background monitor ownership, package run, operation run, API/event streaming, UI integration, recording, mandatory daemon-only policy for non-manual callers, and live successful recovery validation remain open.

### Next step

1. Commit and push this Runtime milestone.
2. Add a checkpoint tag if this is accepted as a stable daemon monitor recovery routing rollback point.
3. Next implementation milestone should either live-validate matching-lease daemon recovery on a safe emulator/resource state or begin the recording/session record interface skeleton.

## 2026-06-27 ActingLab daemon lease-gated control request routing

### Current status

- Connected structured session lease metadata to daemon control requests.
- `SessionCommandRequest` now carries optional lease metadata outside inner command arguments.
- Client-only request flags now strip:
  - `--state-dir`
  - `--request-timeout-ms`
  - `--lease-holder`
  - `--holder`
  - `--lease-id`
- Added lease-gated daemon request support for:
  - `tap`
  - `swipe`
  - `long-tap`
  - `key`
  - `text`
  - `tap-target`
  - `navigate`
  - `recover`
- Top-level control commands now accept `--via-daemon` and submit to the resident daemon request queue.
- `session request` now accepts equivalent lease-gated control commands.
- Daemon execution checks the active instance lease before any task-level input, semantic tap, navigation, or recovery command.
- Missing lease metadata, missing active lease, wrong holder, and wrong lease id fail visibly with safety-blocked errors.
- Daemon lease errors are mapped back to client-side safety-blocked failures instead of fake success or misleading runtime-not-running output.
- `capabilities` now advertises the new lease-gated daemon control requests.
- Existing direct trusted manual commands remain available for local use; this milestone only gates daemon-routed control requests.
- No ADB input fallback, reconnect, retry loop, OCR, SQLite, UI, scheduler body, recording, capture backend, recognition algorithm, or game logic was added.

### Resource mirrors used

- Runtime baseline before this task: `cbe19b806ac169559e0b2b3e66b12d2489724350`.
- Resource repositories were not read or used by this implementation step.

### Files changed

- `apps/actinglab/src/main.rs`
- `PLANS.md`
- `CHECKPOINT.md`

### Commands run

- Read `C:\合作工作区\ActingCommand\TASK-Lab-session-layer.md`.
- Read `C:\合作工作区\ActingCommand\FINDING-AK-game-freeze-2026-06-27.md`.
- Read repo-local `AGENTS.md`, `PLANS.md`, `CHECKPOINT.md`, and `NOTICE.md`; `LICENSE_POLICY.md` does not exist in this repository.
- `git fetch --prune --tags origin`
- `git pull --ff-only origin main`
- `cargo fmt --all`
- Attempted `cargo test -p actingcommand-actinglab session_control_request direct_touch_via_daemon session_request_payload session_lease`; Cargo rejected multiple test filters in one command.
- `cargo test -p actingcommand-actinglab session_`
- `cargo test -p actingcommand-actinglab session_control_request_requires_lease_metadata`
- `cargo test -p actingcommand-actinglab session_control_request_rejects_wrong_holder_before_device_io`
- `cargo test -p actingcommand-actinglab session_control_request_rejects_wrong_lease_id_before_device_io`
- `cargo test -p actingcommand-actinglab direct_touch_via_daemon_accepts_lease_flags_before_daemon_lookup`
- `cargo test -p actingcommand-actinglab`
- `cargo fmt --all -- --check`
- `cargo test --workspace`
- `cargo clippy --workspace -- -D warnings`
- `git diff --check`
- Code diff prohibited-feature scan for ADB input fallback, `adb shell screencap`, SQLite, OCR, OpenCV, fallback, reconnect, retry, and MaaTouch startup additions.
- Local daemon smoke without an acquired lease:
  - `cargo run -q -p actingcommand-actinglab -- --json session start --state-dir <target session-control-smoke dir>`
  - `cargo run -q -p actingcommand-actinglab -- --json session lease acquire --state-dir <target session-control-smoke dir> --holder scheduler --lease-id smoke-lease`
  - `cargo run -q -p actingcommand-actinglab -- --json tap --via-daemon --state-dir <target session-control-smoke dir> --lease-holder lab --lease-id smoke-lease 100 200`
  - `cargo run -q -p actingcommand-actinglab -- --json session stop --state-dir <target session-control-smoke dir>`
- Local daemon smoke with an `ak` lease and mismatched holder:
  - `cargo run -q -p actingcommand-actinglab -- --json session start --state-dir <target session-control-smoke dir>`
  - `cargo run -q -p actingcommand-actinglab -- --json --instance ak session lease acquire --state-dir <target session-control-smoke dir> --holder scheduler --lease-id smoke-lease`
  - `cargo run -q -p actingcommand-actinglab -- --json --instance ak tap --via-daemon --state-dir <target session-control-smoke dir> --lease-holder lab --lease-id smoke-lease 100 200`
  - `cargo run -q -p actingcommand-actinglab -- --json session stop --state-dir <target session-control-smoke dir>`

### Test results

- New focused tests passed:
  - `session_control_request_requires_lease_metadata`
  - `session_control_request_rejects_wrong_holder_before_device_io`
  - `session_control_request_rejects_wrong_lease_id_before_device_io`
  - `direct_touch_via_daemon_accepts_lease_flags_before_daemon_lookup`
- `cargo test -p actingcommand-actinglab` passed with `105` tests.
- `cargo fmt --all -- --check` passed.
- `cargo test --workspace` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `git diff --check` passed.
- Code diff prohibited-feature scan returned no matches.
- `cargo test -p actingcommand-actinglab session_` failed once because the broad filter ran existing environment-mutating tests in parallel and exposed a transient config EOF race. The full package test passed afterward.
- Local daemon smoke without an acquired lease returned exit code `3` and safety-blocked `daemon_request_blocked` with message `daemon control request requires an active lease for default`; no device input was sent.
- Local daemon smoke with an `ak` scheduler lease returned exit code `3` and safety-blocked `daemon_request_blocked` with message `lease for ak is held by scheduler, not lab`; no device input was sent.

### Current blocker

- No blocker for lease-gated daemon control request rejection and routing.
- Matching-lease live execution still needs a safe simulator state before claiming end-to-end task-level control through the daemon.
- Full Session Layer is still incomplete: scheduler body, package run, operation run, API/event streaming, UI integration, recording, mandatory daemon-only policy for non-manual callers, and live matching-lease control validation remain open.

### Next step

1. Commit and push this Runtime milestone.
2. Add a checkpoint tag if this is accepted as a stable daemon-control authorization rollback point.
3. Next implementation milestone should live-validate matching-lease daemon control on a safe emulator state, then continue toward making daemon ownership mandatory for scheduler/Lab task paths.

## 2026-06-27 ActingLab session lease arbitration interface

### Current status

- Hardened the `session lease acquire|release|preempt|status` interface.
- Added a structured `SessionLease` model.
- Lease records now include:
  - `instance`
  - `holder`
  - `lease_id`
  - `acquired_at_unix_ms`
  - `updated_at_unix_ms`
  - `preempted`
  - optional previous lease provenance
- `acquire` still fails visibly when the instance already has a lease, and the conflict message includes the current holder when available.
- `release` now checks the holder unless `--force` is present.
- `release` checks optional `--lease-id` when provided.
- `preempt` writes a new lease and records the previous lease holder and id.
- Lease writes now use the existing atomic JSON publish path.
- No command has started enforcing leases yet; this task only improves the arbitration interface needed before daemon-routed input/recovery can be safely enabled.
- No tap, key, text, navigate, recover execution, app restart, scheduler body, game-task action, UI, SQLite, OCR, capture backend, recognition, fallback, reconnect, or retry path was added.

### Resource mirrors used

- Runtime baseline before this task: `affa33f5c50ffd7030734a3c62536199fe171e10`.
- `ActingCommand-Resources-Arknights`: `7509ed1da92504dc546e8ef46dd9a450243b52cc`.
- `ActingCommand-Resources-AzurLane`: `17f5efb8460e7c5a7cdfbf3dd8e751719ec57d0c`.
- `ActingCommand-Resources-BlueArchive`: `1bdea27c315e1d10e3e737679bcd67d83a482166`.

### Files changed

- `apps/actinglab/src/main.rs`
- `PLANS.md`
- `CHECKPOINT.md`

### Commands run

- Read `C:\合作工作区\ActingCommand\TASK-Lab-session-layer.md`.
- Read `C:\合作工作区\ActingCommand\FINDING-AK-game-freeze-2026-06-27.md`.
- Read repo-local `AGENTS.md`, `PLANS.md`, `CHECKPOINT.md`, and `NOTICE.md`; `LICENSE_POLICY.md` does not exist in this repository.
- `git fetch --prune --tags origin` and `git pull --ff-only origin main` for Runtime.
- `git fetch --prune --tags origin` and `git pull --ff-only origin main` for the three resource repositories.
- `cargo fmt --all`
- `cargo test -p actingcommand-actinglab session_lease`
- `cargo test -p actingcommand-actinglab`
- First `cargo test --workspace`
- `cargo test -p actingcommand-actinglab session_lease_enforces_holder_and_lease_id_on_release -- --nocapture`
- `cargo fmt --all -- --check`
- Rerun `cargo test --workspace`
- First `cargo clippy --workspace -- -D warnings`
- `cargo fmt --all -- --check`
- Rerun `cargo clippy --workspace -- -D warnings`
- `git diff --check`
- Diff prohibited-feature scan for ADB input fallback, `adb shell screencap`, SQLite, OCR, OpenCV, fallback, reconnect, retry, and MaaTouch startup additions.
- Local smoke:
  - `cargo run -q -p actingcommand-actinglab -- --json --instance ak session lease acquire --state-dir target\session-lease-smoke --holder scheduler --lease-id smoke-1`
  - `cargo run -q -p actingcommand-actinglab -- --json --instance ak session lease status --state-dir target\session-lease-smoke`
  - `cargo run -q -p actingcommand-actinglab -- --json --instance ak session lease release --state-dir target\session-lease-smoke --holder scheduler --lease-id smoke-1`

### Test results

- `cargo test -p actingcommand-actinglab session_lease` passed with `2` focused tests.
- `cargo test -p actingcommand-actinglab` passed with `101` tests.
- First `cargo test --workspace` failed once because the new lease tests did not take the shared `ENV_LOCK` while other tests mutate config-related environment variables.
- The focused failing lease test passed in isolation, confirming the failure was test isolation rather than lease logic.
- Added `ENV_LOCK` guards to the two lease tests.
- Rerun `cargo test --workspace` passed.
- `cargo fmt --all -- --check` passed.
- First `cargo clippy --workspace -- -D warnings` reported a collapsible nested `if` in lease-id validation.
- After flattening the condition, `cargo clippy --workspace -- -D warnings` passed.
- `git diff --check` passed.
- Diff prohibited-feature scan returned no matches in the current diff.
- Local lease smoke passed:
  - acquired `ak` lease for holder `scheduler` with lease id `smoke-1`;
  - status returned the structured lease;
  - release with matching holder and lease id returned `status = released`.

### Current blocker

- No blocker for the lease arbitration interface hardening.
- Full Session Layer is still incomplete: leases are not yet enforced for input, semantic tap, navigation, recovery execution, app restart, package run, operation run, API/event streaming, UI integration, recording, or scheduler body.
- The next risky commands must not be routed through the daemon until lease checks are connected.

### Next step

1. Commit and push the lease arbitration interface Runtime changes.
2. Add a checkpoint tag if this is accepted as a stable lease-interface rollback point.
3. Continue by adding daemon-side lease-aware request authorization for task-level input/recovery, or by exposing lease status through the daemon request lane.

## 2026-06-27 ActingLab session daemon monitor-once routing

### Current status

- Added read-only daemon routing for one-shot monitor diagnosis.
- Added `monitor --once --via-daemon`.
- Added `session request monitor-once`.
- `monitor --via-daemon` without `--once` fails visibly with `validation_failed`.
- `monitor --once --via-daemon --recover` is safety-blocked with `daemon_recovery_requires_lease`.
- The daemon execution path rejects recovery for `monitor_once` requests even if a crafted request file includes `--recover`.
- This task routes read-only monitor diagnosis only.
- No tap, key, text, navigate, recover execution, app restart, scheduler body, game-task action, UI, SQLite, OCR, new capture backend, fallback, reconnect, or retry path was added.

### Resource mirrors used

- Runtime baseline before this task: `a056c19aa67df4ed83acee210046f5b9aee67492`.
- `ActingCommand-Resources-Arknights`: `7509ed1da92504dc546e8ef46dd9a450243b52cc`.
- `ActingCommand-Resources-AzurLane`: `17f5efb8460e7c5a7cdfbf3dd8e751719ec57d0c`.
- `ActingCommand-Resources-BlueArchive`: `1bdea27c315e1d10e3e737679bcd67d83a482166`.

### Files changed

- `apps/actinglab/src/main.rs`
- `PLANS.md`
- `CHECKPOINT.md`

### Commands run

- `cargo fmt --all`
- `cargo test -p actingcommand-actinglab monitor_via_daemon`
- Attempted `cargo test -p actingcommand-actinglab monitor_once_via_daemon_without_daemon_is_runtime_error session_request`; Cargo rejected the extra test filter, so the tests were rerun separately.
- `cargo test -p actingcommand-actinglab monitor_once_via_daemon_without_daemon_is_runtime_error`
- `cargo test -p actingcommand-actinglab session_request`
- Started a live-safe session daemon in `target\session-monitor-request-smoke`.
- `cargo run -q -p actingcommand-actinglab -- --json --instance 127.0.0.1:16416 --capture-backend adb --resource-root C:\Users\Alice\Documents\Azur\ActingCommand-Resources-Arknights --game ark monitor --once --via-daemon --capture --state-dir target\session-monitor-request-smoke --request-timeout-ms 20000`
- Stopped the live-safe session daemon and checked session status.
- `cargo test -p actingcommand-actinglab`
- `cargo test --workspace`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace -- -D warnings`
- `git diff --check`
- Diff prohibited-feature scan for ADB input fallback, `adb shell screencap`, SQLite, OCR, OpenCV, fallback, reconnect, retry, and MaaTouch startup additions.

### Test results

- `cargo test -p actingcommand-actinglab monitor_via_daemon` passed with `2` focused tests.
- `cargo test -p actingcommand-actinglab monitor_once_via_daemon_without_daemon_is_runtime_error` passed.
- `cargo test -p actingcommand-actinglab session_request` passed with `3` focused tests.
- Live-safe daemon smoke passed:
  - started session daemon pid `41756`;
  - submitted AK `monitor --once --via-daemon --capture` for `127.0.0.1:16416`;
  - daemon response returned `mode = daemon_request`;
  - daemon response returned `daemon_command = monitor_once`;
  - inner response reported `status = healthy`, `expected_page = arknights/home`, and `click_allowed = false`;
  - no click, app restart, recovery, or screenshot file write was performed;
  - daemon was stopped and final status reported `running = false`.
- `cargo test -p actingcommand-actinglab` passed with `99` tests.
- `cargo test --workspace` passed.
- `cargo fmt --all -- --check` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `git diff --check` passed.
- Diff prohibited-feature scan returned no matches in the current diff.

### Current blocker

- No blocker for read-only monitor-once daemon routing.
- Full Session Layer is still incomplete: input routing, semantic tap, navigation, recovery execution, app restart, scheduler-owned lease arbitration, API/event streaming, UI integration, recording, and broader command dispatch remain future work.
- Recovery remains intentionally blocked in daemon monitor requests until scheduler lease arbitration is connected.

### Next step

1. Commit and push the monitor-once daemon-routing Runtime changes.
2. Add a checkpoint tag if this is accepted as a stable daemon-monitor rollback point.
3. Continue by adding a lease/arbitration interface or by routing more status-only session commands through the daemon.

## 2026-06-27 ActingLab session daemon read-only semantic routing

### Current status

- Extended the resident session daemon request lane beyond capture diagnosis.
- Added `--via-daemon` routing for `recognize`, `detect-page`, `current-page`, `is-visible`, and `locate`.
- Added `session request recognize`.
- Added `session request detect-page`.
- Added `session request current-page`.
- Added `session request is-visible`.
- Added `session request locate`.
- Request-only client flags are stripped before the daemon executes the inner command:
  - `--via-daemon`
  - `--state-dir`
  - `--request-timeout-ms`
- The daemon still processes queued JSON requests serially and returns structured success/error responses.
- This task routes read-only recognition/status/template-location work only.
- No tap, key, text, navigate, recover, app restart, scheduler body, game-task action, UI, SQLite, OCR, new capture backend, fallback, reconnect, or retry path was added.

### Resource mirrors used

- Runtime baseline before this task: `20af6bd37285c06d225115f94298948f2d41f69d`.
- `ActingCommand-Resources-Arknights`: `7509ed1da92504dc546e8ef46dd9a450243b52cc`.
- `ActingCommand-Resources-AzurLane`: `17f5efb8460e7c5a7cdfbf3dd8e751719ec57d0c`.
- `ActingCommand-Resources-BlueArchive`: `1bdea27c315e1d10e3e737679bcd67d83a482166`.

### Files changed

- `apps/actinglab/src/main.rs`
- `PLANS.md`
- `CHECKPOINT.md`

### Commands run

- Read `C:\合作工作区\ActingCommand\TASK-Lab-session-layer.md`.
- Read `C:\合作工作区\ActingCommand\FINDING-AK-game-freeze-2026-06-27.md`.
- Read repo-local `AGENTS.md`, `PLANS.md`, `CHECKPOINT.md`, and `NOTICE.md`; `LICENSE_POLICY.md` does not exist in this repository.
- `git fetch --prune --tags origin` and `git pull --ff-only origin main` for Runtime.
- `git fetch --prune --tags origin` and `git pull --ff-only origin main` for the three resource repositories.
- `cargo fmt --all`
- `cargo test -p actingcommand-actinglab session_request`
- Attempted `cargo test -p actingcommand-actinglab readonly_via_daemon_without_daemon_is_runtime_error direct_touch_commands_are_capability_registered`; Cargo rejected the extra test filter, so the tests were rerun separately.
- `cargo test -p actingcommand-actinglab readonly_via_daemon_without_daemon_is_runtime_error`
- `cargo test -p actingcommand-actinglab direct_touch_commands_are_capability_registered`
- `cargo test -p actingcommand-actinglab`
- `cargo clippy --workspace -- -D warnings`
- `cargo fmt --all -- --check`
- `cargo test --workspace`
- `git diff --check`
- Diff prohibited-feature scan for ADB input fallback, `adb shell screencap`, SQLite, OCR, OpenCV, fallback, reconnect, retry, and MaaTouch startup additions.
- Started a live-safe session daemon in `target\session-readonly-smoke`.
- `cargo run -q -p actingcommand-actinglab -- --json --instance 127.0.0.1:16416 --capture-backend adb --resource-root C:\Users\Alice\Documents\Azur\ActingCommand-Resources-Arknights --game ark current-page --via-daemon --state-dir target\session-readonly-smoke --request-timeout-ms 20000`
- `cargo run -q -p actingcommand-actinglab -- --json --instance 127.0.0.1:16416 --capture-backend adb --resource-root C:\Users\Alice\Documents\Azur\ActingCommand-Resources-Arknights --game ark current-page --via-daemon --capture --state-dir target\session-readonly-smoke --request-timeout-ms 20000`
- Stopped the live-safe session daemon and checked session status.

### Test results

- `cargo test -p actingcommand-actinglab session_request` passed with `3` focused tests.
- `cargo test -p actingcommand-actinglab readonly_via_daemon_without_daemon_is_runtime_error` passed.
- `cargo test -p actingcommand-actinglab direct_touch_commands_are_capability_registered` passed.
- `cargo test -p actingcommand-actinglab` passed with `96` tests.
- `cargo fmt --all -- --check` passed.
- `cargo test --workspace` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `git diff --check` passed.
- Diff prohibited-feature scan returned no matches in the current diff.
- The first live-safe daemon smoke without `--capture` returned visible `validation_failed` with message `command requires --scene <png> or --capture`.
- The corrected live-safe daemon smoke with `--capture` passed:
  - started session daemon pid `9924`;
  - submitted AK `current-page --via-daemon --capture` for `127.0.0.1:16416`;
  - daemon response returned `mode = daemon_request`;
  - daemon response returned `daemon_command = current_page`;
  - inner response reported `page = arknights/home`, `matched = true`, and `standby = false`;
  - no click, app restart, recovery, or screenshot file write was performed;
  - daemon was stopped and final status reported `running = false`.

### Current blocker

- No blocker for the read-only semantic request-routing increment.
- Full Session Layer is still incomplete: input routing, semantic tap, navigation, recovery, app restart, scheduler-owned lease arbitration, API/event streaming, UI integration, recording, and broader command dispatch remain future work.
- Arknights resource matching still reports several pages as matched on the same home frame; this is existing resource ambiguity and not fixed by this Runtime routing task.

### Next step

1. Commit and push the read-only semantic request-routing Runtime changes.
2. Add a checkpoint tag if this is accepted as a stable daemon-routing rollback point.
3. Continue by adding lease-gated maintenance recovery requests or by routing additional read-only status commands through the daemon.

## 2026-06-27 ActingLab session daemon request channel

### Current status

- Implemented the first resident session daemon request lane.
- The session daemon now polls a local file-IPC request queue under the session state directory.
- Added request and response directories under the session state directory.
- Added atomic request/response JSON writes so the daemon does not read half-written files.
- Added `session request capture-diagnose`.
- Added `capture diagnose --via-daemon`.
- Only read-only `capture_diagnose` requests are accepted by the daemon in this phase.
- Daemon request submission has a bounded `--request-timeout-ms`, default `10000`.
- Daemon responses preserve success payloads or structured visible errors.
- Daemon heartbeat now records `processed_request` after it processes one or more queued requests.
- No tap, key, text, navigate, recover, app restart, game-task action, scheduler body, UI, SQLite, OCR, new capture backend, fallback, reconnect, or retry path was added.

### Resource mirrors used

- Runtime baseline before this task: `2d3db811f1f8dd074b813c02bc272e5560868071`.
- `ActingCommand-Resources-Arknights`: `7509ed1da92504dc546e8ef46dd9a450243b52cc`.
- `ActingCommand-Resources-AzurLane`: `17f5efb8460e7c5a7cdfbf3dd8e751719ec57d0c`.
- `ActingCommand-Resources-BlueArchive`: `1bdea27c315e1d10e3e737679bcd67d83a482166`.

### Files changed

- `apps/actinglab/src/main.rs`
- `PLANS.md`
- `CHECKPOINT.md`

### Commands run

- Read `C:\合作工作区\ActingCommand\TASK-Lab-session-layer.md`.
- Read `C:\合作工作区\ActingCommand\FINDING-AK-game-freeze-2026-06-27.md`.
- `git fetch --prune --tags origin` and `git pull --ff-only origin main` for Runtime.
- `git fetch --prune --tags origin` and `git pull --ff-only origin main` for the three resource repositories.
- `cargo fmt --all`
- `cargo test -p actingcommand-actinglab session_request`
- `cargo test -p actingcommand-actinglab capture_diagnose_via_daemon_without_daemon_is_runtime_error`
- `cargo test -p actingcommand-actinglab direct_touch_commands_are_capability_registered`
- Started a live-safe session daemon in `target\session-request-smoke`.
- `cargo run -q -p actingcommand-actinglab -- --json --instance 127.0.0.1:16416 --capture-backend adb capture diagnose --via-daemon --state-dir target\session-request-smoke --fresh-delay-ms 200 --request-timeout-ms 15000`
- Stopped the live-safe session daemon and checked session status.
- `cargo test -p actingcommand-actinglab`
- `cargo test --workspace`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace -- -D warnings`
- `git diff --check`
- Diff prohibited-feature scan for ADB input fallback, `adb shell screencap`, SQLite, OCR, OpenCV, fallback, reconnect, retry, and MaaTouch startup additions.

### Test results

- `cargo test -p actingcommand-actinglab session_request` passed.
- `cargo test -p actingcommand-actinglab capture_diagnose_via_daemon_without_daemon_is_runtime_error` passed.
- `cargo test -p actingcommand-actinglab direct_touch_commands_are_capability_registered` passed.
- Live-safe daemon smoke passed:
  - started session daemon pid `35564`;
  - submitted AK `capture diagnose --via-daemon` for `127.0.0.1:16416`;
  - daemon response returned `mode = daemon_request`, `daemon_command = capture_diagnose`, and inner `status = fresh`;
  - first probe hash: `1f5cbe83ce47b5f1cfd523937a9755c528eb2791841161ce2291cdf1a52228ca`;
  - second probe hash: `3da5b2c37f107543354f6a40219ffd5f7bcea10436ccbe80adf3239cc80eb7d3`;
  - `click_allowed = false` and `action_executed = false`;
  - daemon was stopped and final status reported `running = false`.
- `cargo test -p actingcommand-actinglab` passed with `93` tests.
- `cargo test --workspace` passed.
- `cargo fmt --all -- --check` passed.
- First `cargo clippy --workspace -- -D warnings` found one `needless_match`; after simplifying the result assignment, clippy passed.
- `git diff --check` passed.
- Diff prohibited-feature scan returned no matches in the current diff.

### Current blocker

- No blocker for this first read-only daemon request lane.
- Full Session Layer is still incomplete: the daemon currently accepts only `capture_diagnose`.
- Scheduler-owned lease arbitration, input routing, recovery routing, API/event streaming, UI integration, recording, and broader command dispatch remain future work.

### Next step

1. Commit and push the session daemon request channel Runtime changes.
2. Add a checkpoint tag after push if this is accepted as a stable rollback point.
3. Continue by routing more read-only commands through the daemon, then add lease-gated maintenance recovery requests.

## 2026-06-27 ActingLab capture stale diagnostics

### Current status

- Implemented read-only capture stale diagnostics for the AK stale-frame finding.
- Added `capture diagnose`.
- Added `session capture diagnose` through the existing `session capture` route.
- Diagnose mode does not require `--out` and does not write screenshot files.
- Diagnose mode runs a two-frame fresh probe and reports `fresh`, `stale_suspected`, or `capture_unavailable`.
- Diagnose output includes backend attempts, freshness data, optional frame metadata, and structured recovery recommendations.
- Existing `capture --require-fresh` still fails visibly when no backend produces a changing probe frame.
- No capture hot-path rewrite, new capture backend, automatic app restart, click, reconnect loop, OCR, SQLite, UI, scheduler body, or game-task logic was added.

### Resource mirrors used

- Runtime baseline before this task: `d65982f17998e2bf63b75796252ce887847aedc4`.
- `ActingCommand-Resources-Arknights`: `7509ed1da92504dc546e8ef46dd9a450243b52cc`.
- `ActingCommand-Resources-AzurLane`: `17f5efb8460e7c5a7cdfbf3dd8e751719ec57d0c`.
- `ActingCommand-Resources-BlueArchive`: `1bdea27c315e1d10e3e737679bcd67d83a482166`.

### Files changed

- `apps/actinglab/src/main.rs`
- `PLANS.md`
- `CHECKPOINT.md`

### Commands run

- Read `C:\合作工作区\ActingCommand\TASK-Lab-session-layer.md`.
- Read `C:\合作工作区\ActingCommand\FINDING-AK-game-freeze-2026-06-27.md`.
- `git fetch --prune --tags origin` and `git pull --ff-only origin main` for Runtime and the three resource repositories.
- `cargo fmt --all`
- `cargo test -p actingcommand-actinglab capture_diagnosis`
- `cargo test -p actingcommand-actinglab fresh_auto_probe_prefers_fast_backends_before_adb`
- `cargo test -p actingcommand-actinglab direct_touch_commands_are_capability_registered`
- `cargo run -q -p actingcommand-actinglab -- --json --instance 127.0.0.1:16416 --capture-backend adb capture diagnose --fresh-delay-ms 200`
- `cargo test -p actingcommand-actinglab`
- `cargo test --workspace`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace -- -D warnings`
- `git diff --check`
- Diff prohibited-feature scan for ADB input fallback, `adb shell screencap`, SQLite, OCR, OpenCV, reconnect, retry, and MaaTouch startup additions.

### Test results

- `cargo test -p actingcommand-actinglab capture_diagnosis` passed with `2` focused tests.
- `cargo test -p actingcommand-actinglab fresh_auto_probe_prefers_fast_backends_before_adb` passed.
- `cargo test -p actingcommand-actinglab direct_touch_commands_are_capability_registered` passed.
- First `cargo test -p actingcommand-actinglab` run had one transient temp-config EOF failure in `detect_page_accepts_reorganized_repo_root_resource_root`; rerunning the specific test and then the package test passed.
- `cargo test -p actingcommand-actinglab` passed with `91` tests.
- `cargo test --workspace` passed.
- `cargo fmt --all -- --check` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `git diff --check` passed.
- Diff prohibited-feature scan returned no matches in the current diff.
- Read-only AK B server smoke on `127.0.0.1:16416` with explicit ADB screencap returned `status = fresh`.
- The AK smoke recorded different probe hashes:
  - first: `dfb461c345ffb42235811f22fce12ad09d4827ab916f6b6d464a441634cae693`
  - second: `12748ed0a477e9df73d67352aed1876093acf4783b802bf2e833b82a012bd927`
- No click, app restart, or screenshot file write occurred in the AK diagnosis smoke.

### Current blocker

- No blocker for this read-only diagnosis increment.
- Full Session Layer remains incomplete: daemon-resident capture monitoring, scheduler-owned recovery arbitration, backend switching policy, app restart policy, modal handling, and UI/API streaming are still future work.

### Next step

1. Commit and push the capture stale diagnostics Runtime changes.
2. Add a checkpoint tag after push if this is accepted as a stable diagnosis rollback point.
3. Continue toward daemon-resident monitor policy that consumes capture diagnosis results under scheduler ownership.

## 2026-06-27 ActingLab session layer Phase C bounded monitor loop

### Current status

- Implemented bounded `monitor` loop behavior for the Phase C session-health path.
- `monitor --once` remains the one-shot diagnosis command.
- `monitor` without `--once` now runs a bounded loop controlled by `--max-iterations`, default `1`, and `--interval-ms`, default `1000`.
- `--max-iterations 0` fails visibly as a validation error.
- Default `monitor` loop behavior is read-only and does not invoke recovery.
- `monitor --recover` explicitly delegates non-healthy diagnoses to the existing `session recover` path.
- Recovery delegation preserves `--to` / `--expect`, `--scene`, `--capture`, freshness flags, startup-login flags, action limits, step timeout, and poll interval.
- Real `monitor --recover --scene` without `--dry-run` fails visibly because `session recover` still requires `--capture` for real execution.
- No scheduler body, daemon-resident monitor, UI, SQLite, OCR, game-task logic, ADB input fallback, fallback/reconnect/retry loop, or new capture backend was added.

### Resource mirrors used

- Runtime baseline before this task: `cc07f596bce75c6bffa35098a5bb09c7a3b7d0a0`.
- `ActingCommand-Resources-Arknights`: `7509ed1da92504dc546e8ef46dd9a450243b52cc`.
- `ActingCommand-Resources-AzurLane`: `17f5efb8460e7c5a7cdfbf3dd8e751719ec57d0c`.
- `ActingCommand-Resources-BlueArchive`: `1bdea27c315e1d10e3e737679bcd67d83a482166`.

### Files changed

- `apps/actinglab/src/main.rs`
- `PLANS.md`
- `CHECKPOINT.md`

### Commands run

- `git status --short --branch`
- Read `C:\合作工作区\ActingCommand\TASK-Lab-session-layer.md`.
- `cargo fmt --all`
- `cargo test -p actingcommand-actinglab monitor_loop`
- `cargo test -p actingcommand-actinglab monitor_once`
- `cargo test -p actingcommand-actinglab session_recover`
- `cargo run -q -p actingcommand-actinglab -- --json --dry-run --resource-root C:\Users\Alice\Documents\Azur\ActingCommand-Resources-Arknights --game ark monitor --max-iterations 1 --interval-ms 0 --recover --startup-login --to home --scene target\session-startup-login-smoke\blank-1280x720.png`
- `cargo test --workspace`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace -- -D warnings`
- `git diff --check`
- Diff prohibited-feature scan for ADB input fallback, `adb shell screencap`, SQLite, OCR, OpenCV, fallback, reconnect, retry, and MaaTouch startup additions.

### Test results

- `cargo test -p actingcommand-actinglab monitor_loop` passed with `3` focused tests.
- `cargo test -p actingcommand-actinglab monitor_once` passed with `3` focused tests.
- `cargo test -p actingcommand-actinglab session_recover` passed with `7` focused tests.
- `cargo test --workspace` passed.
- `cargo fmt --all -- --check` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `git diff --check` passed.
- Diff prohibited-feature scan returned no matches in the current diff.
- The Arknights dry-run command used the real resource repository root and resolved `C:\Users\Alice\Documents\Azur\ActingCommand-Resources-Arknights\ours\STARTUP-LOGIN.md`.
- The Arknights dry-run diagnosed a standby frame, delegated through `monitor --recover --startup-login`, and planned popup-close `(1205, 67)` plus continue `(640, 360)` without connecting to the emulator or clicking.

### Current blocker

- No blocker for the bounded monitor-loop CLI increment.
- Full Phase C remains incomplete: the daemon-resident monitor, scheduler lease ownership, modal dismissal policy, app restart policy, stale-frame escalation policy, and resident event streaming are still future work.

### Next step

1. Commit and push the bounded monitor-loop Runtime changes.
2. Add a checkpoint tag after push if this is accepted as a stable monitor-loop rollback point.
3. Continue Phase C with scheduler-owned resident monitoring or modal/app-restart policy in a separately scoped task.

## 2026-06-27 ActingLab session layer Phase C startup-login resource loop

### Current status

- Implemented explicit `session recover --startup-login` for the first Phase C startup-login resource path.
- The command reads `STARTUP-LOGIN.md` from the resolved resource root, including reorganized `<repo>\ours` roots.
- Added `--startup-login-file <path>` for explicit startup-login resource validation.
- Extracts popup-close and continue/center coordinates from the resource file.
- Missing startup-login files, malformed coordinates, missing popup-close coordinates, and missing continue coordinates fail visibly with safety-blocked errors.
- Dry-run planning works with `--scene` and reports `safety_gate = maintenance_login_only`.
- Real execution remains gated by the existing `session recover` requirement for `--capture`.
- Real execution runs a bounded MaaTouch semantic tap loop, then recaptures and detects the page after each round.
- Loop bounds are explicit: `--startup-max-rounds`, default `25`, and `--startup-interval-ms`, default `2000`.
- No ADB input fallback, new capture backend, OCR, SQLite, UI, scheduler body, recording body, or game-task execution was added.

### Resource mirrors used

- Runtime baseline before this task: `28a44377078a`.
- `ActingCommand-Resources-Arknights`: `7509ed1da92504dc546e8ef46dd9a450243b52cc`.
- `ActingCommand-Resources-AzurLane`: `17f5efb8460e7c5a7cdfbf3dd8e751719ec57d0c`.
- `ActingCommand-Resources-BlueArchive`: `1bdea27c315e1d10e3e737679bcd67d83a482166`.

### Files changed

- `apps/actinglab/src/main.rs`
- `PLANS.md`
- `CHECKPOINT.md`

### Commands run

- `git fetch --prune --tags` for Runtime and the three resource repositories.
- `git status --short --branch` and hash checks for Runtime and the three resource repositories.
- Read `C:\合作工作区\ActingCommand\TASK-Lab-session-layer.md`.
- Read `C:\合作工作区\ActingCommand\FINDING-AK-game-freeze-2026-06-27.md`.
- `cargo fmt --all`
- `cargo test -p actingcommand-actinglab session_recover_startup_login`
- `cargo test -p actingcommand-actinglab session_recover`
- Generated `target\session-startup-login-smoke\blank-1280x720.png` for offline dry-run validation.
- `cargo run -q -p actingcommand-actinglab -- --json --dry-run --resource-root C:\Users\Alice\Documents\Azur\ActingCommand-Resources-Arknights --game ark session recover --startup-login --to home --scene target\session-startup-login-smoke\blank-1280x720.png`
- `cargo test --workspace`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace -- -D warnings`
- `git diff --check`
- Diff prohibited-feature scan for ADB input fallback, `adb shell screencap`, SQLite, OCR, OpenCV, fallback, reconnect, retry, and MaaTouch startup additions.

### Test results

- `cargo test -p actingcommand-actinglab session_recover_startup_login` passed with `3` focused tests.
- `cargo test -p actingcommand-actinglab session_recover` passed with `6` focused tests.
- `cargo test --workspace` passed.
- `cargo fmt --all -- --check` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `git diff --check` passed.
- Diff prohibited-feature scan returned no matches in the current diff.
- The Arknights dry-run command used the real resource repository root and resolved `C:\Users\Alice\Documents\Azur\ActingCommand-Resources-Arknights\ours\STARTUP-LOGIN.md`.
- The Arknights dry-run plan reported popup-close `(1205, 67)`, continue `(640, 360)`, `max_rounds = 25`, `interval_ms = 2000`, and `safety_gate = maintenance_login_only`.
- No emulator connection or click was performed in the dry-run validation.

### Current blocker

- No blocker for this explicit startup-login resource-loop task.
- Full Phase C remains incomplete: the resident monitor does not yet automatically invoke this loop under scheduler lease ownership.
- AzurLane and BlueArchive do not yet have equivalent startup-login resources wired through this command path.

### Next step

1. Commit and push the startup-login Runtime changes.
2. Continue Phase C with resident monitor invocation, scheduler lease coordination, modal dismissal policy, or app restart policy in a separately scoped task.

## 2026-06-27 Resource repository `ours` compatibility

### Current status

- Implemented resource-root compatibility for the 2026-06-26 resource repository reorganization.
- `--resource-root <repo>` now resolves to `<repo>\ours` when the input is a reorganized resource repository root.
- `resource validate --repo <repo>` reports `input`, resolved `resource_root`, and `resource_layout`.
- `resource convert --repo <repo>` uses the resolved resource root and reports `resource_layout`.
- `package build-task` and `package build-pack` resolve local or cloned repository roots to `ours` before loading operations, recognition, and navigation data.
- Direct `--resource-root <repo>\ours` and direct `--repo <repo>\ours` still work unchanged.
- No device input, capture backend, recognition hot-path, scheduler, UI, SQLite, OCR, recording, or game-task logic was changed.

### Resource mirrors used

- Runtime baseline before this task: `983d69c40dff`.
- `ActingCommand-Resources-Arknights`: `7509ed1da92504dc546e8ef46dd9a450243b52cc`.
- `ActingCommand-Resources-AzurLane`: `17f5efb8460e7c5a7cdfbf3dd8e751719ec57d0c`.
- `ActingCommand-Resources-BlueArchive`: `1bdea27c315e1d10e3e737679bcd67d83a482166`.

### Files changed

- `apps/actinglab/src/main.rs`
- `apps/actinglab/src/package_build.rs`
- `apps/actinglab/src/resource_convert.rs`
- `PLANS.md`
- `CHECKPOINT.md`

### Commands run

- `git fetch --prune --tags` for Runtime and the three resource repositories.
- `git status --short --branch` and hash checks for Runtime and the three resource repositories.
- Read `C:\合作工作区\ActingCommand\NOTICE-resource-repo-reorg-2026-06-26.md`.
- Read `C:\合作工作区\ActingCommand\TASK-Lab-session-layer.md`.
- Read `C:\合作工作区\ActingCommand\FINDING-AK-game-freeze-2026-06-27.md`.
- `cargo fmt --all`
- `cargo test -p actingcommand-actinglab detect_page_accepts_reorganized_repo_root_resource_root`
- `cargo test -p actingcommand-actinglab build_task_accepts_reorganized_repo_root_with_ours_layout`
- `cargo run -q -p actingcommand-actinglab -- --json resource validate --repo C:\Users\Alice\Documents\Azur\ActingCommand-Resources-Arknights`
- `cargo run -q -p actingcommand-actinglab -- --json resource validate --repo C:\Users\Alice\Documents\Azur\ActingCommand-Resources-AzurLane`
- `cargo run -q -p actingcommand-actinglab -- --json resource validate --repo C:\Users\Alice\Documents\Azur\ActingCommand-Resources-BlueArchive`
- `cargo run -q -p actingcommand-actinglab -- --json --resource-root C:\Users\Alice\Documents\Azur\ActingCommand-Resources-Arknights --game ark detect-page --check-pages`
- `cargo run -q -p actingcommand-actinglab -- --json --resource-root C:\Users\Alice\Documents\Azur\ActingCommand-Resources-AzurLane --game azur detect-page --check-pages`
- `cargo run -q -p actingcommand-actinglab -- --json --resource-root C:\Users\Alice\Documents\Azur\ActingCommand-Resources-BlueArchive --game ba detect-page --check-pages`
- `cargo run -q -p actingcommand-actinglab -- --json --dry-run resource convert --repo C:\Users\Alice\Documents\Azur\ActingCommand-Resources-Arknights --game ark --server cn --locale zh-CN`
- `cargo run -q -p actingcommand-actinglab -- --json --dry-run resource convert --repo C:\Users\Alice\Documents\Azur\ActingCommand-Resources-AzurLane --game azur --server jp --locale ja-JP`
- `cargo run -q -p actingcommand-actinglab -- --json --dry-run resource convert --repo C:\Users\Alice\Documents\Azur\ActingCommand-Resources-BlueArchive --game ba --server jp --locale ja-JP`
- `cargo run -q -p actingcommand-actinglab -- --json --dry-run package build-task --repo C:\Users\Alice\Documents\Azur\ActingCommand-Resources-Arknights --task open_terminal --game ark --server cn --out target\resource-root-compat\ak-open-terminal.zip`
- `cargo run -q -p actingcommand-actinglab -- --json --dry-run package build-pack --repo C:\Users\Alice\Documents\Azur\ActingCommand-Resources-BlueArchive --game ba --server jp --entry-task return_home --out target\resource-root-compat\ba-full.zip`
- `cargo test --workspace`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace -- -D warnings`
- `git diff --check`
- Diff prohibited-feature scan for ADB input fallback, `adb shell screencap`, SQLite, OCR, OpenCV, scheduler implementation, recording implementation, fallback, reconnect, retry, MaaTouch startup additions, and direct touch additions.

### Test results

- `cargo test -p actingcommand-actinglab detect_page_accepts_reorganized_repo_root_resource_root` passed.
- `cargo test -p actingcommand-actinglab build_task_accepts_reorganized_repo_root_with_ours_layout` passed.
- `cargo test --workspace` passed.
- `cargo fmt --all -- --check` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `git diff --check` passed.
- Diff prohibited-feature scan returned no matches in the current diff.
- `resource validate --repo <resource repo root>` passed for Arknights, AzurLane, and BlueArchive and resolved each to `<repo>\ours`.
- `detect-page --check-pages` passed for all three games using the resource repository root as `--resource-root`.
- `resource convert --dry-run` passed for all three games using the resource repository root as `--repo`.
- Arknights `package build-task --dry-run` for `open_terminal` passed using the resource repository root as `--repo`.
- BlueArchive `package build-pack --dry-run` passed using the resource repository root as `--repo`.

### Current blocker

- No blocker for this compatibility task.
- `--from-remote` package builds should be smoke-tested against actual remote resource repository URLs before a release package flow depends on that path.

### Next step

1. Commit and push the resource-root compatibility Runtime changes.
2. Continue the session-layer Phase C self-heal/login resource policy in a separately scoped task.

## 2026-06-27 ActingLab session layer Phase C monitor once

### Current status

- Implemented a read-only `monitor --once` entry for the Phase C session-health diagnosis path.
- `monitor --once` reports `healthy`, `standby`, or `unexpected_page`.
- `monitor --once` accepts `--expect <page>` or `--to <page>`, defaulting to `home`.
- When using `--capture`, `monitor --once` returns capture backend attempts and freshness diagnostics in `scene_source`.
- For standby, `monitor --once` reports whether `control_points.wake` is available and shows the maintenance recovery step.
- For unexpected pages, `monitor --once` checks the same safe recovery route gates used by `session recover`.
- Existing `monitor` without `--once` remains reserved for the future resident/background monitor.
- No scheduler implementation, UI, OCR, SQLite, recording implementation, game task logic, ADB input fallback, fallback/reconnect/retry loop, new capture backend, or MaaTouch startup path was added.
- Implementation commit: `97bdef0ebf313af03481a2e3121f8cde9648547a` (`Add session monitor once diagnostics`).

### Resource mirrors used

- Runtime baseline before this task: `f3de55cf0694`.
- `ActingCommand-Resources-Arknights`: `7509ed1da925`.
- `ActingCommand-Resources-AzurLane`: `17f5efb8460e`.
- `ActingCommand-Resources-BlueArchive`: `1bdea27c315e`.

### Files changed

- `apps/actinglab/src/main.rs`
- `PLANS.md`
- `CHECKPOINT.md`

### Commands run

- `git fetch --prune --tags` for Runtime and the three resource repositories.
- `git status --short --branch` and hash checks for Runtime and the three resource repositories.
- Read relevant Phase C and record sections from `C:\合作工作区\ActingCommand\TASK-Lab-session-layer.md`.
- Read `C:\合作工作区\ActingCommand\FINDING-AK-game-freeze-2026-06-27.md`.
- `cargo fmt --all`
- `cargo test -p actingcommand-actinglab monitor_once`
- `cargo test -p actingcommand-actinglab session_recover`
- `cargo fmt --all -- --check`
- `cargo test --workspace`
- `cargo clippy --workspace -- -D warnings`
- `git diff --check`
- Diff prohibited-feature scan for ADB input fallback, `adb shell screencap`, SQLite, OCR, OpenCV, scheduler implementation, recording implementation, fallback, reconnect, retry, and MaaTouch startup additions.
- `detect-page --check-pages` through `actinglab` for Arknights, AzurLane, and BlueArchive resource roots under `ours`.
- BlueArchive JP read-only `monitor --once --capture` on `127.0.0.1:16481` with `--capture-backend adb`.

### Test results

- `cargo test -p actingcommand-actinglab monitor_once` passed with `3` focused tests.
- `cargo test -p actingcommand-actinglab session_recover` passed with `3` focused tests.
- `cargo test --workspace` passed.
- `cargo fmt --all -- --check` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `git diff --check` passed.
- Diff prohibited-feature scan returned no matches.
- `detect-page --check-pages` passed for Arknights, AzurLane, and BlueArchive resource roots under `ours`.

### Live/read-only dry-run results

- BlueArchive JP `127.0.0.1:16481` `monitor --once --capture --expect home` returned `status=standby`.
- `monitor --once` reported `click_allowed=false`.
- `scene_source` recorded `capture_backend_used=adb_screencap`, the backend attempt, `1280x720`, and `freshness.required=false`.
- Recovery was reported as `available=true`, with recommended command `session recover --to bluearchive/home --capture`.
- The planned recovery step used `control_points.wake` at `(300, 2)`.
- No MaaTouch session was started and no click was sent.

### Current blocker

- Phase C is still incomplete: the persistent background monitor loop, automatic recovery invocation, login resource loop, modal dismissal policy, app restart policy, and scheduler-coordinated self-heal ownership are still future work.
- Arknights page anchors remain broad and can produce multiple simultaneous page matches; resource-lane tightening is needed before trusting live recovery decisions that depend on AK page state.
- Live recovery execution should wait for operator acceptance of the current resource quality and the intended maintenance route.

### Next step

1. Commit and push the Phase C `monitor --once` Runtime changes.
2. Add a checkpoint tag after push if this is accepted as a stable monitor-diagnosis rollback point.
3. Continue Phase C with the persistent monitor loop or login/modal resource policy in a separately scoped task.

## 2026-06-27 ActingLab session layer Phase C initial recovery

### Current status

- Implemented the first bounded Phase C recovery step from `TASK-Lab-session-layer.md`.
- Added `session recover --to <page>`, defaulting to `home`.
- Real recovery execution requires `--capture`; `--scene` is accepted only with `--dry-run`.
- Standby recovery now reads `control_points.wake` from navigation resources and can plan a wake step without clicking.
- Navigation `control_points` now accept both `point: "x,y"` and `point: [x, y]`.
- Recovery routes reuse existing navigation graph, destructive-name checks, destructive action overlap checks, PageDetector, recognition pack, capture path, and MaaTouch semantic input path.
- Added `--max-actions`, defaulting to `3`, to keep maintenance recovery bounded.
- No scheduler implementation, UI, OCR, SQLite, recording implementation, game task logic, ADB input fallback, fallback/reconnect/retry loop, or new capture backend was added.
- Implementation commit: `e62c23474c14a806af87801ac8e470b04bbc5850` (`Add session recovery command`).

### Resource mirrors used

- Runtime baseline before this task: `27459af7de2c`.
- `ActingCommand-Resources-Arknights`: `7509ed1da925`.
- `ActingCommand-Resources-AzurLane`: `17f5efb8460e`.
- `ActingCommand-Resources-BlueArchive`: `1bdea27c315e`.

### Files changed

- `apps/actinglab/src/main.rs`
- `PLANS.md`
- `CHECKPOINT.md`

### Commands run

- `git fetch --prune --tags` for Runtime and the three resource repositories.
- `git status --short --branch` and hash checks for Runtime and the three resource repositories.
- Read relevant Phase C and record sections from `C:\合作工作区\ActingCommand\TASK-Lab-session-layer.md`.
- Read `C:\合作工作区\ActingCommand\FINDING-AK-game-freeze-2026-06-27.md`.
- `cargo fmt --all`
- `cargo test -p actingcommand-actinglab session_recover`
- `cargo test -p actingcommand-actinglab`
- `cargo fmt --all -- --check`
- `cargo test --workspace`
- `cargo clippy --workspace -- -D warnings`
- `git diff --check`
- Diff prohibited-feature scan for ADB input fallback, `adb shell screencap`, SQLite, OCR, OpenCV, scheduler implementation, recording implementation, fallback, reconnect, and retry additions.
- BlueArchive offline dry-run `session recover --scene ...` against the `ours` resource root.
- BlueArchive JP read-only/dry-run `session recover --capture` on `127.0.0.1:16481` with `--capture-backend adb`.
- `detect-page --check-pages` through `actinglab` for Arknights, AzurLane, and BlueArchive resource roots under `ours`.

### Test results

- `cargo test -p actingcommand-actinglab session_recover` passed with `3` focused tests.
- `cargo test -p actingcommand-actinglab` passed with `78` tests.
- `cargo test --workspace` passed.
- `cargo fmt --all -- --check` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `git diff --check` passed.
- Diff prohibited-feature scan returned no matches.
- `detect-page --check-pages` passed for Arknights, AzurLane, and BlueArchive resource roots under `ours`.

### Live/read-only dry-run results

- BlueArchive JP `127.0.0.1:16481` `session recover --capture --dry-run` returned `status=planned`.
- The current BlueArchive frame was treated as `standby`.
- The planned recovery step used `control_points.wake` at `(300, 2)`.
- `executed=false`; no MaaTouch session was started and no click was sent.

### Current blocker

- Phase C is not complete: background monitor, login resource loop, modal dismissal policy, app restart policy, and scheduler-coordinated self-heal ownership are still future work.
- Arknights page anchors remain broad and can produce multiple simultaneous page matches; resource-lane tightening is needed before trusting live recovery decisions that depend on AK page state.
- Live `session recover` execution should wait for operator acceptance of the current resource quality and the intended maintenance route.

### Next step

1. Commit and push the Phase C initial recovery Runtime changes.
2. Add a checkpoint tag after push if this is accepted as a stable recovery rollback point.
3. Continue Phase C with monitor/self-heal policy only in a separately scoped task.

## 2026-06-27 ActingLab session layer Phase B

### Current status

- Implemented the Phase B semantic layer from `TASK-Lab-session-layer.md`.
- Added `current-page`, `is-visible`, `locate`, `tap-target`, and `navigate`.
- `current-page` now shares the same page-detection helper as `detect-page`.
- `is-visible` evaluates visual recognition targets and fails visibly for click-only targets.
- `locate` performs full-frame template localization for calibration.
- `tap-target` requires visual target recognition to pass before real MaaTouch execution.
- `tap-target` real execution requires `--capture`; `--scene` is dry-run/offline only.
- `navigate` loads the navigation graph, detects the current page, plans a route, applies navigation-only safety gates, and polls for arrival after each edge.
- `navigate --dry-run` exposes the planned route without touching the device.
- The shared scene-loading path now honors `--require-fresh` for semantic commands that use `--capture`.
- No UI, SQLite, OCR implementation, scheduler implementation, self-heal, recording, game task logic, ADB input fallback, or new capture backend was added.
- Implementation commit: `e60e2da` (`Add ActingLab semantic commands`).
- Checkpoint tag: `checkpoint/20260627-session-layer-phase-b`.

### Resource mirrors used

- Runtime baseline before this task: `1c52e55`.
- `ActingCommand-Resources-Arknights`: `7509ed1`.
- `ActingCommand-Resources-AzurLane`: `17f5efb8`.
- `ActingCommand-Resources-BlueArchive`: `1bdea27`.

### Files changed

- `apps/actinglab/src/main.rs`
- `PLANS.md`
- `CHECKPOINT.md`

### Commands run

- `git fetch --prune --tags` and status/hash checks for Runtime and the three resource repositories.
- Read Runtime-local `AGENTS.md`, `PLANS.md`, `CHECKPOINT.md`, and `NOTICE.md`; `LICENSE_POLICY.md` does not exist in this split Runtime repo.
- Read relevant Phase B lines from `C:\合作工作区\ActingCommand\TASK-Lab-session-layer.md`.
- `cargo fmt --all`
- `cargo test -p actingcommand-actinglab`
- `cargo test --workspace`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace -- -D warnings`
- `git diff --check`
- Prohibited-feature scan for ADB input fallback, `adb shell screencap`, SQLite, OCR implementation, OpenCV, scheduler implementation, and record-step/build-task strings in Runtime source paths.
- `cargo run -q -p actingcommand-actinglab -- --json --instance 127.0.0.1:16416 --capture-backend adb capture --out target\session-phase-b-smoke\ak.png`
- `cargo run -q -p actingcommand-actinglab -- --json --resource-root C:\Users\Alice\Documents\Azur\ActingCommand-Resources-Arknights\ours --game ark current-page --scene target\session-phase-b-smoke\ak.png`
- `cargo run -q -p actingcommand-actinglab -- --json --dry-run --resource-root C:\Users\Alice\Documents\Azur\ActingCommand-Resources-Arknights\ours --game ark navigate --to depot --scene target\session-phase-b-smoke\ak.png`
- `detect-page --check-pages` through `actinglab` for Arknights, AzurLane, and BlueArchive resource roots under `ours`.
- Read-only `current-page --capture` through `actinglab` on AzurLane JP `127.0.0.1:16385`, Arknights CN `127.0.0.1:16416`, and BlueArchive JP `127.0.0.1:16481`.

### Test results

- `cargo test -p actingcommand-actinglab` passed with `75` tests.
- `cargo test --workspace` passed.
- `cargo fmt --all -- --check` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `git diff --check` passed.
- `detect-page --check-pages` passed for Arknights, AzurLane, and BlueArchive resource roots under `ours`.

### Live read-only smoke results

- AK capture on `127.0.0.1:16416` wrote `target\session-phase-b-smoke\ak.png` at `1280x720`.
- AK `current-page --scene` and `current-page --capture` returned `arknights/home`.
- AK current-page evidence also matched several unrelated pages on the same home frame; this is a resource discriminator issue, not a CLI execution failure.
- AK `navigate --dry-run --to depot` planned one route edge: `home_to_depot`, point `(1194,640)`, with no click execution.
- AzurLane JP `current-page --capture` returned `azurlane/home`.
- BlueArchive JP `current-page --capture` returned standby and a visible wake-safe-point recovery hint; no wake click was sent.

### Scope scan

- No ADB input fallback was added.
- No `adb shell screencap` path was added.
- No SQLite, OpenCV, UI, scheduler implementation, self-heal, recording implementation, or game task logic was added.
- The only OCR scan hit is the pre-existing `actingcommand-contract` primitive trait declaration.

### Current blocker

- Arknights page anchors are too broad and can produce multiple simultaneous page matches.
- BlueArchive current live frame was standby or non-home; Phase C self-heal/wake handling is still future work.
- Live `tap-target` and live `navigate` clicks should wait for a user-selected safe route and current resource discriminator acceptance.

### Next step

1. Push Runtime `main` and `checkpoint/20260627-session-layer-phase-b`.
2. Tighten Arknights page anchors in the resource lane.
3. Continue Phase C self-heal only after a separate scoped task.

## 2026-06-27 ActingLab session layer Phase A

### Current status

- Implemented the Phase A mechanism layer from `TASK-Lab-session-layer.md`, with the AK stale-capture finding from `FINDING-AK-game-freeze-2026-06-27.md` reflected in capture diagnostics.
- Added `session start`, `session status`, `session stop`, and internal `session daemon`.
- The session daemon writes `session.json` and `heartbeat.json` under the requested session state directory and survives the parent CLI command.
- Windows session start uses `PowerShell Start-Process` without stdout/stderr redirection so the parent CLI returns a visible JSON result and the daemon does not inherit the caller's output handle.
- Added `session instance list|health|reconnect`.
- Added `session app launch|stop|restart` with explicit package resolution from `--package`, `instance.<id>.package`, or known game/server defaults.
- Added `session lease acquire|release|preempt|status` as a local lease interface placeholder. This is not a scheduler implementation.
- Added MaaTouch `key` and `text` support through the `InputBackend` trait and `MaaTouchBackend`; no ADB input fallback was added.
- Added top-level `key` and `text` trusted-manual CLI commands.
- Added `capture --require-fresh` and `session capture --require-fresh`; fresh capture compares two raw-pixel frame hashes and reports stale-frame diagnostics. `auto` fresh probing tries `nemu_ipc`, `droidcast_raw`, then `adb_screencap`.
- Implementation commit: `01e2f0f` (`Add ActingLab session layer phase A`).
- Checkpoint tag: `checkpoint/20260627-session-layer-phase-a`.

### Resource mirrors used

- Runtime baseline before this task: `3c76360`.
- `ActingCommand-Resources-Arknights`: `7509ed1`.
- `ActingCommand-Resources-AzurLane`: `17f5efb8`.
- `ActingCommand-Resources-BlueArchive`: `1bdea27`.

### Files changed

- `apps/actinglab/src/main.rs`
- `crates/device/src/adb.rs`
- `crates/device/src/input.rs`
- `crates/device/src/maatouch.rs`
- `PLANS.md`
- `CHECKPOINT.md`

### Commands run

- Mirrored Runtime from `origin/main` and verified baseline `3c76360`.
- Mirrored Arknights, AzurLane, and BlueArchive resource repositories from their remotes.
- Read `AGENTS.md`, `PLANS.md`, `CHECKPOINT.md`, and `NOTICE.md`; `LICENSE_POLICY.md` does not exist in this split Runtime repo.
- Read the task files from `C:\合作工作区\ActingCommand\FINDING-AK-game-freeze-2026-06-27.md` and `C:\合作工作区\ActingCommand\TASK-Lab-session-layer.md`.
- `cargo fmt --all`
- `cargo build -p actingcommand-actinglab`
- `cargo test -p actingcommand-actinglab`
- `cargo test -p actingcommand-device`
- `cargo test --workspace`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace -- -D warnings`
- `target\debug\actinglab.exe --json session start --state-dir target\session-smoke16`
- `target\debug\actinglab.exe --json session status --state-dir target\session-smoke16`
- `target\debug\actinglab.exe --json session stop --state-dir target\session-smoke16`
- `target\debug\actinglab.exe --json --instance 127.0.0.1:16416 --capture-backend adb capture --out target\session-layer-smoke\ak-capture.png`
- `target\debug\actinglab.exe --json --instance 127.0.0.1:16416 --capture-backend adb capture --out target\session-layer-smoke\ak-capture-fresh.png --require-fresh --fresh-delay-ms 250`
- `git diff --check`
- Scope scan for `adb shell input`, `shell input`, `input tap`, `input swipe`, `adb shell screencap`, retry/reconnect/fallback strings, SQLite, OCR, OpenCV, and Tesseract in touched source paths.
- `Get-Process actinglab -ErrorAction SilentlyContinue`

### Test results

- `cargo test -p actingcommand-actinglab` passed with `70` tests.
- `cargo test -p actingcommand-device` passed with `36` tests.
- `cargo test --workspace` passed.
- `cargo fmt --all -- --check` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `git diff --check` passed.

### Live read-only smoke results

- `session start/status/stop` passed using `target\session-smoke16`.
- After `session stop`, `Get-Process actinglab` found no remaining `actinglab` process.
- AK `127.0.0.1:16416` read-only capture wrote `target\session-layer-smoke\ak-capture.png` at `1280x720`.
- AK `127.0.0.1:16416` read-only `--require-fresh` capture wrote `target\session-layer-smoke\ak-capture-fresh.png` at `1280x720`.
- The fresh probe reported different hashes:
  - first: `9ff265caf49f3736016515cd9e0a2ee23fd83828f0cc38c075381f6b6c3a0afd`
  - second: `0df20dc844a3b61019375d92882b0316cba85fd4e4d6db712ea8f6e4e60ac9ee`
- No click, MaaTouch tap, task run, purchase, sortie, exercise, gacha, or resource-consuming action was executed during live smoke.

### Scope scan

- No ADB input fallback was added.
- No `adb shell screencap` path was added.
- No SQLite, OCR, OpenCV, Tesseract, UI, scheduler implementation, recording implementation, semantic navigation, monitoring, or game logic was added.
- Scan hits for `fallback` are existing capability metadata.
- Scan hits for `reconnect` are the explicit `session instance reconnect` command requested by the session layer.

### Current blocker

- No implementation blocker for Phase A.
- Later phases still need semantic navigation, monitoring/self-heal, record/amend/build-task, stream mode, and scheduler-owned arbitration.

### Next step

1. Commit and push this Phase A Runtime change.
2. Add a checkpoint tag if this is accepted as the stable session-layer Phase A rollback point.
3. Continue with Phase B semantic layer only after the user confirms this Phase A boundary.

## 2026-06-26 Runtime full-frame recognition hang fix

### Current status

- Implemented the `TASK-runtime-detect-page-hang.md` fix for pathological large template searches.
- `crates/recognition` now routes large search workloads through a bounded pyramid/refine matcher instead of the slow full sliding-window path.
- The optimized path covers `full_frame`, explicit whole-frame rectangles, and large bounded regions.
- Small bounded matching keeps the previous path and behavior.
- Large `ccoeff_normed` refinement uses integral-image window statistics plus exact row dot-products.
- Large `ccorr_normed` searches use the same bounded coarse/refine strategy.
- Target matching has a wall-clock deadline and returns a fatal recognition error instead of hanging forever.
- `crates/page-detector` now has a regression test proving `evaluate_page` does not evaluate unrelated pages.
- Implementation commit: `5711f1f1e240789c12672c3fc56439166c8493b0` (`Fix large recognition search hangs`).

### Resource mirrors used

- Runtime baseline before this task: `4b274c3`.
- `ActingCommand-Resources-Arknights`: `6a9d0b5`.
- `ActingCommand-Resources-AzurLane`: `6c9bba41`.
- `ActingCommand-Resources-BlueArchive`: `1b52342`.

### Files changed

- `crates/recognition/src/lib.rs`
- `crates/page-detector/src/lib.rs`
- `PLANS.md`
- `CHECKPOINT.md`

### Commands run

- `git fetch --prune --tags` and mirror/reset checks for Runtime, Arknights, AzurLane, BlueArchive, and UI repositories.
- `git reset --hard origin/main` and `git clean -fd` for Runtime and resource repositories before this resource-dependent task.
- `git status --short --branch`
- `Get-Content -LiteralPath 'C:\合作工作区\ActingCommand\TASK-runtime-detect-page-hang.md' -Raw`
- `cargo test -p actingcommand-recognition -p actingcommand-page-detector`
- `cargo build -p actingcommand-device-test`
- Offline `recognize` and `detect-page` runs against `C:\合作工作区\ActingCommand\fixtures\ba-detectpage-hang-repro.png`.
- Offline full-frame sweeps across BlueArchive, AzurLane, and Arknights recognition packs.
- `adb devices`
- Read-only `device-test capture` on ports `16385`, `16416`, and `16481`.
- Read-only live `detect-page --capture --all` on AzurLane JP `127.0.0.1:16385`, Arknights CN `127.0.0.1:16416`, and BlueArchive JP `127.0.0.1:16481`.
- `cargo test --workspace`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace -- -D warnings`
- `git diff --check`
- Scope scans over `crates\recognition\src` and `crates\page-detector\src` for capture hot-path work, ADB input fallback, UI, SQLite, OCR, OpenCV, retry loop, reconnect, fallback, sleep, and panic patterns.

### Test results

- `cargo test -p actingcommand-recognition -p actingcommand-page-detector` passed.
- `cargo test --workspace` passed.
- `cargo fmt --all -- --check` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `git diff --check` passed.

### Offline recognition results

- BA single full-frame target `page/task_center` on `ba-detectpage-hang-repro.png` returned in about `860 ms` with `passed=false`, `raw_score=0.653629`, and no hang.
- BA `detect-page --page bluearchive/home` on the same fixture matched in about `393 ms`.
- BA `detect-page --page bluearchive/task_center` on the same fixture returned in about `869 ms` with no hang.
- BA `detect-page --all` on the same fixture returned in about `7804 ms`.
- AzurLane `template/main_goto_fleet` on the same fixture returned in about `896 ms`; the template score passed but the color check failed, as expected for a non-Azur scene.
- Arknights `template/infrastpic__infrastoverview` on the same fixture returned in about `695 ms`.
- Full-frame sweep after the final fix:
  - BlueArchive: `21` full-frame targets, `0` command failures, max about `878 ms`.
  - AzurLane: `39` full-frame targets, `0` command failures, max about `798 ms`.
  - Arknights: `2` full-frame targets, `0` command failures, max about `739 ms`.

### Live read-only smoke results

- `adb devices` showed connected devices including `127.0.0.1:16385`, `127.0.0.1:16416`, and `127.0.0.1:16481`.
- `device-test capture` wrote `1280x720` screenshots for all three ports under `target\detect-page-hang-smoke`.
- Visual inspection identified:
  - `127.0.0.1:16385`: AzurLane JP home.
  - `127.0.0.1:16416`: Arknights CN home.
  - `127.0.0.1:16481`: BlueArchive JP standby/scene frame.
- Read-only live `detect-page --capture --all`:
  - AzurLane JP `127.0.0.1:16385`: matched `azurlane/home`, about `993 ms`.
  - Arknights CN `127.0.0.1:16416`: matched `arknights/home`, about `12690 ms`.
  - BlueArchive JP `127.0.0.1:16481`: returned in about `8187 ms`; home was not matched because the current frame was standby/scene, but it did not hang.
- No live click, MaaTouch command, task run, purchase, sortie, exercise, gacha, or resource-consuming action was executed.

### Scope scan

- No capture hot-path change was made.
- No ADB input fallback was added.
- No UI, SQLite, OCR, OpenCV, retry loop, reconnect, or fallback implementation was added in the touched recognition/page-detector paths.
- No sleep or panic pattern was added in the touched recognition/page-detector paths.

### Current blocker

- No implementation blocker.
- BlueArchive live page matching was not expected to match home because the current captured frame was a standby/scene frame; no wake click was performed in this task.

### Next step

1. Add a checkpoint tag for this stable milestone.
2. Push Runtime `main` and the checkpoint tag to GitHub.

## 2026-06-26 Runtime ADB connection hardening

### Current status

- Implemented the ADB connection hardening task from `TASK-runtime-adb-connection-hardening.md`.
- Unified Runtime adb resolution in `crates/device` instead of relying on PATH adb or the old `F:\AzurPilot` venv hint.
- Resolution order is `ACTINGCOMMAND_ADB_PATH`, MuMu discovery, then user-configured `adb_path`.
- MuMu discovery prefers `D:\BST\MuMuPlayer\nx_main\adb.exe`, then sorted `nx_device\*\shell\adb.exe` candidates.
- `actinglab`, `device-test`, ADB screencap capture, and MaaTouch device verification now share the same device-layer adb path model.
- Device verification now does at most one bounded `adb connect` when the target allows connect and the current state is not `device`.
- Runtime does not call `adb kill-server`.
- `adb exec-out screencap -p` remains the ADB screenshot path and keeps the existing wall-clock timeout.
- Added `external-tools/NOTICE.md` documentation for the MuMu adb version boundary and the no-committed-adb rule.
- Device-test CLI parsing no longer forces adb discovery for offline parse-only commands; device commands resolve adb before execution.
- AK-only live validation used `127.0.0.1:16416`.
- BA `127.0.0.1:16481` and AzurLane `127.0.0.1:16385` validation were skipped because those emulators were occupied by another process.
- Implementation commit: `8ae8dd31eb4a56db363c7afad545d12bf47bc4d3` (`Harden Runtime ADB path resolution`).

### Files changed

- `apps/actinglab/src/lab_run.rs`
- `apps/actinglab/src/main.rs`
- `apps/device-test/src/main.rs`
- `crates/device/src/adb.rs`
- `crates/device/src/capture.rs`
- `crates/device/src/maatouch.rs`
- `external-tools/NOTICE.md`
- `PLANS.md`
- `CHECKPOINT.md`

### Commands run

- `Get-Content -LiteralPath 'C:\Users\Alice\.codex\skills\implement\SKILL.md'`
- `rg -n "ActingCommand|ADB|adb|BlueArchive|match_metric|actingcommand-device-test" C:\Users\Alice\.codex\memories\MEMORY.md`
- `git status --short --branch`
- `Get-Content -LiteralPath C:\合作工作区\ActingCommand\TASK-runtime-adb-connection-hardening.md`
- `Get-Content -LiteralPath C:\合作工作区\ActingCommand\HANDOFF-Codex-lab-batch.md`
- `git diff -- crates/device/src/adb.rs`
- `git diff -- crates/device/src/capture.rs crates/device/src/maatouch.rs`
- `git diff -- apps/actinglab/src/main.rs apps/actinglab/src/lab_run.rs apps/device-test/src/main.rs`
- `git diff -- external-tools/NOTICE.md`
- `rg -n "parse_args|MaaTouchValidationConfig::default|--adb|capture --out|resolve_adb_path" apps\device-test\src\main.rs`
- `cargo fmt --all`
- `cargo test -p actingcommand-device-test`
- `cargo test -p actingcommand-device adb::tests`
- `cargo run -p actingcommand-actinglab -- --json paths`
- `cargo run -p actingcommand-actinglab -- --json doctor`
- `cargo run -p actingcommand-device-test -- --port 16416 --capture-backend adb capture --out target\adb-hardening-smoke\ak-device-test.png`
- `cargo run -p actingcommand-actinglab -- --json --instance 127.0.0.1:16416 --capture-backend adb capture --out target\adb-hardening-smoke\ak-actinglab.png`
- `$env:ACTINGCOMMAND_ADB_PATH='C:\definitely-missing\adb.exe'; cargo run -p actingcommand-actinglab -- --json paths`
- `cargo test --workspace`
- `cargo clippy --workspace -- -D warnings`
- `cargo clippy -p actingcommand-device -p actingcommand-actinglab -p actingcommand-device-test -- -D warnings`
- `cargo fmt --all -- --check`
- `git diff --check`
- Source scans for old adb hints, `adb shell screencap`, `adb shell input`, `adb kill-server`, reconnect loops, retry loops, and `println!/eprintln!` in `crates/device`.

### Test results

- `cargo test -p actingcommand-device-test` passed with 54 tests.
- `cargo test -p actingcommand-device adb::tests` passed with 3 tests.
- `cargo test --workspace` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo clippy -p actingcommand-device -p actingcommand-actinglab -p actingcommand-device-test -- -D warnings` passed.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.

### Live smoke results

- `actinglab paths` reported adb path `D:\BST\MuMuPlayer\nx_main\adb.exe` with source `mumu_discovery`.
- `actinglab doctor` reported adb path `D:\BST\MuMuPlayer\nx_main\adb.exe` with source `mumu_discovery`.
- Invalid `ACTINGCOMMAND_ADB_PATH=C:\definitely-missing\adb.exe` produced a visible fatal diagnostic in `actinglab paths` and did not silently fall back to another adb.
- `device-test capture` on AK `127.0.0.1:16416` wrote `target\adb-hardening-smoke\ak-device-test.png` at `1280x720`.
- `actinglab capture` on AK `127.0.0.1:16416` wrote `target\adb-hardening-smoke\ak-actinglab.png` at `1280x720`.
- Captured AK PNG was visually inspected and was a readable Arknights main-screen frame.
- BA and AzurLane live validation were intentionally skipped to avoid interfering with another active process.

### Scope scan

- No old `F:\AzurPilot` adb hint remains in Runtime source paths.
- No default bare `"adb"` path remains in Runtime source paths.
- No `adb shell screencap` or `adb shell input` path was found in Runtime source paths.
- No `adb kill-server` call was found.
- No reconnect loop or retry loop implementation was found.
- No `println!` or `eprintln!` exists in `crates/device/src`.

### Current blocker

- No implementation blocker.
- BA and AzurLane live validation remains paused while those emulators are owned by another process.

### Next step

1. Commit and push the checkpoint hash update.
2. Add a checkpoint tag after push.
3. If BA/AzurLane become available later, repeat live capture validation on those ports with the same MuMu adb.

## 2026-06-26 Lab packager

### Current status

- Continued the handoff batch after completing the direct touch CLI task.
- Re-read `C:\合作工作区\ActingCommand\TASK-Lab-packager.md` with UTF-8 output and confirmed the active scope is the Lab packager: Rust `resource convert`, `package build-task`, `package build-pack`, `--from-remote`, and offline `lab validate`.
- Refreshed the three resource repositories from their remotes before resource-dependent work:
  - `C:\Users\Alice\Documents\Azur\ActingCommand-Resources-Arknights`
  - `C:\Users\Alice\Documents\Azur\ActingCommand-Resources-AzurLane`
  - `C:\Users\Alice\Documents\Azur\ActingCommand-Resources-BlueArchive`
- Added `serde_json` `preserve_order` support so generated JSON keeps insertion order for converter parity.
- Implemented Rust-side `actinglab resource convert`, replacing the reserved CLI stub.
- Implemented `actinglab lab validate --zip <pkg.zip>` as an offline Lab-1y input package validator. It unpacks into a temporary directory, runs `LabControl::validate`, loads the Operation Bundle, recognition pack, pages, optional navigation, and runs detector validation without device access or LabLease.
- Implemented `actinglab package build-task` for self-contained single-task Lab input zips.
- Implemented `actinglab package build-pack` for full packages and `--split-dir` per-task packages.
- Implemented `--from-remote` shallow clone support for package builds. The default path remains local and offline.
- Package writes now go through a temporary zip plus `lab validate` before replacing the requested output path.
- Split-package builds validate into a temporary split directory before moving results, so a failing task does not silently leave a new partial split set.
- Confirmed the actual `lab run` route model: it executes only the selected entry task's own `operation_bundle.operations` by matching the current page to operation `from`; it does not route across tasks through the generated navigation graph.
- Final `build-task` closure strategy: include the selected task bundle by default; include `return_home` only when `--include-recovery` is explicitly requested and present.
- No UI, SQLite, OCR, scheduler implementation, capture hot-path rollback, ADB input fallback, reconnect loop, retry loop, game logic, or live emulator operation was added.
- Implementation commit: `f3e5c0eb3ce77c7f331a24c5dc9c31c0f5f0f993` (`Add Lab packager commands`).

### Files changed

- `Cargo.toml`
- `Cargo.lock`
- `apps/actinglab/src/main.rs`
- `apps/actinglab/src/lab_run.rs`
- `apps/actinglab/src/resource_convert.rs`
- `apps/actinglab/src/package_build.rs`
- `PLANS.md`
- `CHECKPOINT.md`

### Commands run

- `Get-Content -LiteralPath 'C:\Users\Alice\.codex\skills\implement\SKILL.md'`
- `Get-Content -LiteralPath 'C:\合作工作区\ActingCommand\TASK-actinglab-tap-cli.md'`
- `Get-Content -LiteralPath 'C:\合作工作区\ActingCommand\HANDOFF-Codex-lab-batch.md'`
- `Get-Content -LiteralPath 'C:\合作工作区\ActingCommand\TASK-Lab-packager.md' -Encoding UTF8`
- `git status --short --branch`
- Resource repository refresh commands for Arknights, AzurLane, and BlueArchive: `git status --short --branch`, `git fetch origin --prune --tags`, and `git pull --ff-only`.
- `cargo fmt --all`
- `cargo test -p actingcommand-actinglab package_build`
- `cargo test -p actingcommand-actinglab`
- `cargo clippy -p actingcommand-actinglab -- -D warnings`
- `cargo run -p actingcommand-actinglab -- --json resource convert --repo C:\Users\Alice\Documents\Azur\ActingCommand-Resources-Arknights --game arknights --server cn --locale zh-CN --dry-run`
- `cargo run -p actingcommand-actinglab -- --json resource convert --repo C:\Users\Alice\Documents\Azur\ActingCommand-Resources-BlueArchive --game bluearchive --server jp --locale ja-JP --dry-run`
- `cargo run -p actingcommand-actinglab -- --json resource convert --repo C:\Users\Alice\Documents\Azur\ActingCommand-Resources-AzurLane --game azurlane --server jp --locale ja-JP --dry-run`
- `cargo run -p actingcommand-actinglab -- --json package build-task --repo C:\Users\Alice\Documents\Azur\ActingCommand-Resources-Arknights --task open_terminal --game arknights --server cn --out target\lab-packager-smoke\ak-open-terminal.zip`
- `cargo run -p actingcommand-actinglab -- --json package build-pack --repo C:\Users\Alice\Documents\Azur\ActingCommand-Resources-Arknights --game arknights --server cn --entry-task open_terminal --out target\lab-packager-smoke\ak-full.zip`
- `cargo run -p actingcommand-actinglab -- --json package build-task --from-remote C:\Users\Alice\Documents\Azur\ActingCommand-Resources-Arknights --task open_terminal --game arknights --server cn --out target\lab-packager-smoke\ak-open-terminal-remote.zip`
- `Test-Path -LiteralPath 'C:\Users\Alice\AppData\Local\Temp\actinglab-resource-remote-8296-1782454684013043600'`
- `cargo run -p actingcommand-actinglab -- --json lab validate --zip target\lab-packager-smoke\ak-open-terminal.zip`
- `cargo run -p actingcommand-actinglab -- --json lab validate --zip target\lab-packager-smoke\ak-full.zip`
- `cargo run -p actingcommand-actinglab -- --json package build-pack --repo C:\Users\Alice\Documents\Azur\ActingCommand-Resources-Arknights --game arknights --server cn --split-dir target\lab-packager-smoke\ak-split-failcheck`
- `Test-Path -LiteralPath 'target\lab-packager-smoke\ak-split-failcheck'`
- Converter parity script over temporary copies under `target\resource-convert-parity-current`.
- `cargo test --workspace`
- `cargo clippy --workspace -- -D warnings`
- `cargo fmt --all -- --check`
- `git diff --check`
- Scope scans over new packager/converter files and existing ActingLab command registration paths.

### New or updated unit coverage

- `resource_convert::tests::derives_target_ids_like_python_converter`
- `resource_convert::tests::converts_region_and_click_shapes`
- `resource_convert::tests::resolves_page_required_variants`
- `resource_convert::tests::color_check_region_is_flattened`
- `resource_convert::tests::arknights_default_locale_matches_current_resource_pack`
- `lab_run::tests::lab_validate_accepts_minimal_self_contained_package`
- `lab_run::tests::lab_validate_rejects_missing_control`
- `package_build::tests::build_task_package_validates_and_rewrites_template_paths`
- `package_build::tests::build_pack_package_validates`
- `package_build::tests::split_pack_writes_one_package_per_task`
- `package_build::tests::build_task_rejects_dangerous_asset_entry`

### Converter parity

- Arknights parity passed for:
  - `recognition/arknights.cn.pack.json`
  - `recognition/arknights.cn.pages.json`
  - `navigation/arknights.cn.navigation.json`
  - `operations/operations.index.json`
  - `operations/operations.primitives.json`
- BlueArchive parity passed for:
  - `recognition/bluearchive.jp.pack.json`
  - `recognition/bluearchive.jp.pages.json`
  - `navigation/bluearchive.jp.navigation.json`
  - `operations/operations.index.json`
  - `operations/operations.primitives.json`
- AzurLane parity passed for:
  - `recognition/azurlane.jp.pages.json`
  - `navigation/azurlane.jp.navigation.json`
  - `operations/operations.index.json`
  - `operations/operations.primitives.json`
- For parity only, `generated_by: "actinglab resource convert"` was normalized to `generated_by: "tools/convert_operations.py"`.
- AzurLane `pack.json` remains outside this parity scope because it is produced by the separate `generate_azurlane_pack.py` template-cropping step.

### Real resource smoke results

- Arknights `resource convert --dry-run` succeeded with 10 bundles, 14 targets, 11 pages, 13 navigation edges, 7 page operations, 10 index tasks, and 25 primitives.
- BlueArchive `resource convert --dry-run` succeeded with 20 bundles, 22 targets, 20 pages, 19 navigation edges, 23 page operations, 20 index tasks, and 53 primitives.
- AzurLane `resource convert --dry-run` succeeded with 41 bundles, 81 targets, 41 pages, 43 navigation edges, 17 page operations, 41 index tasks, and 89 primitives.
- Arknights `package build-task` for `open_terminal` wrote and validated `target\lab-packager-smoke\ak-open-terminal.zip`.
- Arknights `package build-pack --entry-task open_terminal` wrote and validated `target\lab-packager-smoke\ak-full.zip`.
- Arknights `package build-task --from-remote <local git repo path>` wrote and validated `target\lab-packager-smoke\ak-open-terminal-remote.zip`, and the temporary clone directory was removed.
- Arknights `package build-pack --split-dir` against current real data fails loudly on unresolved `0,0` coordinates and did not create the new fail-check split output directory.
- BlueArchive split packaging also fails loudly on unresolved `0,0` coordinates in current data. BA and AzurLane live/emulator validation was skipped because another process was using those emulators.

### Test results

- `cargo test -p actingcommand-actinglab package_build` passed with 4 tests.
- `cargo test -p actingcommand-actinglab` passed with 65 tests.
- `cargo clippy -p actingcommand-actinglab -- -D warnings` passed.
- `cargo test --workspace` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- Scope scan over `apps/actinglab/src/package_build.rs` and `apps/actinglab/src/resource_convert.rs` found no ADB input fallback, `adb shell screencap`, quick screenshot backend, SQLite, OCR, scheduler implementation, reconnect, retry loop, or fallback implementation.
- Broader scan only matched existing scheduler stubs and existing schema text in `main.rs`, not new packager behavior.

### Current blocker

- No implementation blocker for the Lab packager code.
- Current real Arknights and BlueArchive resource data still contains unresolved `0,0` coordinates in some tasks, so real `build-pack --split-dir` for every task correctly fails until those resource bundles are resolved or explicitly marked non-executable.
- BA and AzurLane live/emulator validation was skipped because those emulators were occupied by another process.
- Resource-repository Python converter deletion remains a separate resource-lane step after downstream acceptance.

### Next step

1. Commit and push Runtime repository changes to `HS7097/ActingCommand-Runtime`.
2. Downstream resource lane can migrate regeneration commands to `actinglab resource convert` after acceptance.
3. Resolve or classify placeholder-coordinate tasks before expecting real `build-pack --split-dir` over current resource repositories to succeed.
4. Keep BA/AzurLane live validation paused while their emulators are occupied.

## 2026-06-26 ActingLab direct touch CLI

### Current status

- Re-read `C:\合作工作区\ActingCommand\TASK-actinglab-tap-cli.md` and `C:\合作工作区\ActingCommand\HANDOFF-Codex-lab-batch.md` with UTF-8 output.
- Confirmed the active first-priority task is the small `actinglab` direct trusted-manual touch CLI entry point, not the larger Lab packager work.
- Confirmed local `main` was aligned with `origin/main` at `90c2e0029b954ef4449b65df30836bfc4e44fb4b` before this task.
- Added main CLI commands `actinglab tap`, `actinglab swipe`, and `actinglab long-tap`.
- The new commands parse positional coordinates/duration in the same user-facing style as `device-test`, use the existing `MaaTouchBackend`, and return the normal JSON envelope/human output path.
- The new commands are registered in `command_capabilities()` with `needs=["device"]` and `status="available"`.
- `actinglab capture --out <png> --instance ...` remains the existing screenshot side and was not refactored.
- Autonomous execution safety gates were not relaxed: `lab run`, `package run`, `operation run`, and `control probe-click` still retain their LabLease/navigation/expect boundaries.
- No ADB input fallback, reconnect loop, retry loop, UI, scheduler behavior, new backend, OCR, SQLite, or game logic was added.
- Implementation commit: `4e047ef5caf6912908e201ce2a2f3ef610369580` (`Add actinglab direct touch commands`).
- Checkpoint tag: `checkpoint/20260626-actinglab-direct-touch-cli`.

### Files changed

- `apps/actinglab/src/main.rs`
- `PLANS.md`
- `CHECKPOINT.md`

### Commands run

- `Get-Content -LiteralPath 'C:\Users\Alice\.codex\skills\implement\SKILL.md' -Encoding UTF8`
- `git status --short --branch`
- `git diff -- apps/actinglab/src/main.rs`
- `Get-Content -LiteralPath 'C:\合作工作区\ActingCommand\TASK-actinglab-tap-cli.md' -Encoding UTF8`
- `Get-Content -LiteralPath 'C:\合作工作区\ActingCommand\HANDOFF-Codex-lab-batch.md' -Encoding UTF8`
- `rg -n "struct FlagArgs|impl FlagArgs|command_capabilities|command_cap\(|mod tests|#\[cfg\(test\)\]" apps\actinglab\src\main.rs`
- `cargo fmt --all`
- `rg -n "adb shell input|input tap|input swipe|fallback|reconnect|retry" apps\actinglab\src\main.rs`
- `cargo test -p actingcommand-actinglab`
- `cargo clippy -p actingcommand-actinglab -- -D warnings`
- `cargo test --workspace`
- `cargo fmt --all -- --check`
- `git diff --check`
- `cargo clippy --workspace -- -D warnings`
- `rg -n "adb shell input|input tap|input swipe|LabLease|navigation_only|expect" apps\actinglab\src\main.rs`

### New or updated unit coverage

- `direct_touch_positionals_parse`
- `direct_touch_missing_args_are_usage_errors`
- `direct_touch_commands_are_capability_registered`

### Test results

- `cargo test -p actingcommand-actinglab` passed with 54 tests.
- `cargo test --workspace` passed.
- `cargo clippy -p actingcommand-actinglab -- -D warnings` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- Touched-file prohibited scan found no new `adb shell input`, `input tap`, `input swipe`, reconnect, or retry implementation.
- Scope scan confirmed the LabLease/navigation/expect terms remain in the existing autonomous-control safety paths, not in the new direct trusted-manual touch commands.

### Current blocker

- No implementation blocker.
- Live true-device tap/capture acceptance has not been run in this Codex pass. The task document assigns that final true-device acceptance to Claude/user-side validation, and this implementation is verified by compile, unit, clippy, format, and scope checks.
- The larger `TASK-Lab-packager.md` item from the handoff remains a later task and was not started in this pass.

### Next step

1. Push `main` and the checkpoint tag to GitHub.
2. Await live acceptance or proceed to the next handoff task when explicitly started.

## 2026-06-26 Round2 regression close-out

### Current status

- Re-read `C:\合作工作区\ActingCommand\FIX-round2-regressions.md` and confirmed the active scope is RR-01, RR-02, RR-03, and RR-04.
- Confirmed local `main` was aligned with `origin/main` at `5836281bdf6c1ebde0997af84fb60f44f2f58d87` before this task.
- RR-01: `write_segment` now returns a structured segment write error carrying both the global spill-unavailable message and any per-frame encoding failures already collected. `flush_resident_segment` records those per-frame failures before recording the global spill-unavailable warning.
- RR-02: `run_lab_run` rejects `--out` paths inside the generated run directory, captures the run directory string before successful cleanup, reports `run_dir_cleaned: true` on success, and only removes the run directory on successful finalization. Failure finalization preserves the run directory for diagnostics.
- RR-03: removed the explicit `NemuIpcBackend::Drop` worker shutdown so `NemuIpcWorker::Drop` owns shutdown exactly once.
- RR-04: `Tier3PauseCheckpoint` now carries current step index, current step id, current operation id, current phase, expected page, and last matched page. `LabRunContext` fills those fields when a checkpoint is emitted.
- Out-of-scope items were not implemented: no Nemu helper-process isolation and no live gameplay package rerun.
- No UI, OCR, SQLite, scheduler behavior, game logic, new capture backend, ADB input fallback, reconnect loop, or retry loop was added.
- Current implementation commit: `dfce50a50eb048bbc0db5459317c3a58bb88f61c` (`Fix Round2 regression issues`).
- Checkpoint tag: `checkpoint/20260626-round2-regressions`.

### Files changed

- `apps/actinglab/src/frame_store.rs`
- `apps/actinglab/src/lab_run.rs`
- `crates/device/src/capture.rs`
- `PLANS.md`
- `CHECKPOINT.md`

### Commands run

- `git fetch origin --prune --tags`
- `git rev-parse HEAD`
- `git rev-parse origin/main`
- `git status --short --branch`
- `Get-Content -LiteralPath 'C:\合作工作区\ActingCommand\FIX-round2-regressions.md' -Encoding UTF8`
- `Get-Content -LiteralPath 'AGENTS.md' -Encoding UTF8`
- `Get-Content -LiteralPath 'PLANS.md' -Encoding UTF8 -TotalCount 180`
- `Get-Content -LiteralPath 'CHECKPOINT.md' -Encoding UTF8 -TotalCount 180`
- `Get-Content -LiteralPath 'NOTICE.md' -Encoding UTF8`
- `cargo fmt --all`
- `cargo test -p actingcommand-actinglab frame_store::tests::spill_io_failure_preserves_per_frame_encode_failures`
- `cargo test -p actingcommand-actinglab`
- `cargo test -p actingcommand-device`
- `cargo test --workspace`
- `cargo clippy --workspace -- -D warnings`
- `cargo fmt --all -- --check`
- `git diff --check`
- `rg -n "adb shell screencap|adb shell input|fallback|reconnect|println!|eprintln!" crates\device`
- `rg -n "helper process|live gameplay|SQLite|OCR|scrcpy|minicap|adb shell screencap|adb shell input|retry loop|reconnect" apps\actinglab\src crates\device\src`

### New or updated unit coverage

- `spill_io_failure_preserves_per_frame_encode_failures`
- `success_finish_cleans_run_dir_but_keeps_outside_zip`
- `path_inside_detects_run_dir_output`
- `tier3_pause_checkpoint_includes_step_context`
- Updated `failure_zip_materializes_frame_store_screenshots` to assert failed runs keep `run_dir` for diagnostics.

### Test results

- `cargo test -p actingcommand-actinglab` passed with 51 tests.
- `cargo test -p actingcommand-device` passed with 33 tests.
- `cargo test --workspace` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- Device-layer prohibited scan returned no matches for `adb shell screencap`, `adb shell input`, `fallback`, `reconnect`, `println!`, or `eprintln!`.
- Round3 scope scan returned no matches for helper-process implementation, live gameplay implementation, UI-adjacent data layers, quick screenshot backends, retry loops, or reconnect loops in the touched Runtime source paths.

### Current blocker

- No implementation blocker.
- Nemu IPC helper-process isolation remains intentionally out of scope for this task.
- Live gameplay rerun remains intentionally out of scope for this task.

### Next step

1. Create checkpoint tag and push `main` plus the tag to GitHub.
2. Await user-side validation or the next Runtime milestone.

## 2026-06-25 Lab-1z Round2 stability close-out

### Current status

- Re-read `C:\合作工作区\ActingCommand\TASK-Lab-1z-fixes-round2.md` and its referenced P2.2, Lab-1y, P1g, and P2.3 fix guides.
- Confirmed local `main` was aligned with `origin/main` at `abe39fd2b4e69eb67fed71ad6c66dcc010266d59` before the Round2 edits.
- Implemented the Round2 fixes in dependency order: device-layer stability, Lab execution stability, frame-store accounting/spill semantics, CLI/package robustness, then release benchmark validation.
- Device-layer fixes now cover ADB timeout kill-failure handling, bounded DroidCast response reads and child cleanup, backend-scoped Nemu IPC worker timeout/poison behavior, pre-capture Nemu dimension probe and buffer resize, MaaTouch gesture-duration cap, and MaaTouch stderr reader diagnostics.
- Lab execution fixes now cover bounded zip unpacking, dangerous zip entry skip-before-write, output zip partial cleanup, bounded noninteractive git commit lookup, per-run directory cleanup after finalization, and `partial_output` in `summary.json`.
- Frame-store fixes now cover zero-drift resident accounting, dropped-entry subtraction, spilled-thumbnail retention for later dedupe, global spill-unavailable warnings without poisoning every frame, per-frame spill failure isolation, and admission-spill failure without permanent encoder reserve pressure.
- Tier3 is documented and emitted as synchronous graceful partial-output failure. The former Tier3 pause-timeout control is no longer part of the active schema or CLI.
- CLI/package robustness fixes now cover package zip size limits, manifest hash path validation without echoing unsafe traversal strings, unknown list-kind usage errors instead of panic, and visible list warning collection.
- No UI, OCR, SQLite, scheduler behavior, game logic, new capture backend, ADB input fallback, scrcpy, minicap, reconnect loop, or retry loop was added.
- Current implementation commit: `33ee9840982e46011ac2dafb311af740e371ad53` (`Fix Lab-1z Round2 stability issues`).
- Checkpoint tag: `checkpoint/20260625-lab-1z-round2-stability`.

### Files changed

- `crates/device/src/adb.rs`
- `crates/device/src/capture.rs`
- `crates/device/src/maatouch.rs`
- `apps/actinglab/src/frame_store.rs`
- `apps/actinglab/src/lab_run.rs`
- `apps/actinglab/src/main.rs`
- `PLANS.md`
- `CHECKPOINT.md`

### Commands run

- `git fetch origin --prune --tags`
- `git rev-parse HEAD`
- `git rev-parse origin/main`
- `git status --short --branch`
- `git diff --stat`
- `Get-Content -LiteralPath 'C:\合作工作区\ActingCommand\TASK-Lab-1z-fixes-round2.md' -Encoding UTF8`
- `Get-Content -LiteralPath 'C:\合作工作区\ActingCommand\FIX-P2.2-capture-input-stability.md' -Encoding UTF8`
- `Get-Content -LiteralPath 'C:\合作工作区\ActingCommand\FIX-Lab-1y-stability.md' -Encoding UTF8`
- `Get-Content -LiteralPath 'C:\合作工作区\ActingCommand\FIX-P1g-cli-robustness.md' -Encoding UTF8`
- `Get-Content -LiteralPath 'C:\合作工作区\ActingCommand\FIX-P2.3-nemu-capture-perf.md' -Encoding UTF8`
- `cargo test -p actingcommand-device`
- `cargo test -p actingcommand-actinglab`
- `cargo test --workspace`
- `cargo clippy -p actingcommand-device -- -D warnings`
- `cargo clippy -p actingcommand-actinglab -- -D warnings`
- `cargo clippy --workspace -- -D warnings`
- `cargo fmt --all`
- `cargo fmt --all -- --check`
- `git diff --check`
- `rg -n "adb shell screencap|adb shell input|fallback|reconnect|println!|eprintln!" crates\device`
- `cargo build --release -p actingcommand-device-test`
- Release benchmark on `127.0.0.1:16416` with reviewed local DroidCast_raw and Nemu IPC external-tool paths:
  - `target\release\actingcommand-device-test.exe --port 16416 benchmark --rounds 5`

### New or updated unit coverage

- `gesture_duration_is_capped`
- `matched_same_page_frames_can_dedupe`
- `tier2_spills_segment_without_pausing`
- `spilled_frame_keeps_thumbnail_for_later_dedup`
- `spill_io_failure_degrades_without_panic`
- `failure_zip_materializes_frame_store_screenshots`
- `rejects_dangerous_zip_entry_without_writing_it`
- `read_zip_entry_limited_rejects_oversized_entry`
- `package_validate_rejects_unsafe_manifest_hash_path_without_echoing_traversal`
- `read_package_zip_entry_limited_rejects_oversized_entry`
- `list_resource_kind_unknown_returns_usage_error`

### Benchmark result

- Device: Arknights `127.0.0.1:16416`.
- `benchmark --rounds 5` succeeded.
- `adb_screencap`: `1280x720`, capture best/median/p90 `467/471/544ms`, encode best/median/p90 `6/6/8ms`, end-to-end best/median/p90 `468/471/544ms`.
- `droidcast_raw`: `1280x720`, capture best/median/p90 `224/234/811ms`, encode best/median/p90 `5/5/5ms`, end-to-end best/median/p90 `231/239/816ms`.
- `nemu_ipc`: `1280x720`, capture best/median/p90 `4/4/6ms`, encode best/median/p90 `6/6/7ms`, end-to-end best/median/p90 `11/11/13ms`.
- `recommend_poll_interval_ms=942`.
- `recommend_min_capture_interval_ms=544`.
- Control timing remains command-submission-only because MaaTouch reset has no device acknowledgement.

### Test results

- `cargo test -p actingcommand-device` passed with 33 tests.
- `cargo test -p actingcommand-actinglab` passed with 47 tests.
- `cargo test --workspace` passed.
- `cargo clippy -p actingcommand-device -- -D warnings` passed.
- `cargo clippy -p actingcommand-actinglab -- -D warnings` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- Device-layer prohibited scan over `crates/device` returned no matches for `adb shell screencap`, `adb shell input`, `fallback`, `reconnect`, `println!`, or `eprintln!`.

### Current blocker

- No implementation blocker.
- A Nemu IPC worker blocked inside a stuck C FFI call cannot be force-killed in-process. Current behavior marks the backend poisoned and avoids repeated use; true hard-kill isolation remains a later helper-process milestone if needed.
- Live gameplay package execution was not rerun in this pass; live validation was limited to the release benchmark.

### Next step

1. Push `main` and the checkpoint tag to GitHub.
2. Await user-side validation or the next Runtime milestone.

## 2026-06-25 Lab-1z fixes

### Current status

- Re-read `C:\合作工作区\ActingCommand\TASK-Lab-1z-fixes.md` with UTF-8 output and confirmed the active scope is Lab-1z fixes only.
- Added explicit frame recognition lifecycle state: `Pending`, `Matched { page_id }`, `CompletedNoMatch`, and `Failed { reason }`.
- Changed frame admission to estimate incoming memory before unconditional resident admission.
- Limited Tier1 same-page dedupe to non-key `Matched` frames with the same `page_id`; `Pending`, `CompletedNoMatch`, and `Failed` frames do not participate in ordinary same-page dedupe.
- Added synchronous Tier2 segment zip flushes under `frame-store-temp/segment-*.zip` plus `segment-manifest.jsonl`; no background flush thread was added.
- Changed Tier3 into a current-frame outcome with structured checkpoint, pause state, and partial-output finalization path.
- Added Tier3 resume-page safety events: `tier3_resume_capture`, `tier3_resume_page_check`, `tier3_resume_allowed`, and `tier3_resume_blocked`.
- Fixed spill selection so eligible single-frame and last-frame cases can spill; spill is now based on lifecycle state instead of protecting the final index.
- Spill I/O failures degrade with warnings and keep the frame resident instead of panicking or failing the entire run mid-flight.
- Frames involved in a spill failure are marked `spill_failed` to avoid repeated attempts against the same failed frame.
- Made `resident_bytes` a conservative estimate that includes payload, original PNG, thumbnail, metadata/string overhead, per-entry overhead, encoder workspace reserve, and spilled/dropped diagnostics.
- Validation now rejects `similarity_threshold = 1.0`, invalid ratios, non-distinct watermarks, invalid release lines, zero budgets, and Tier2/Tier3 gaps below `flush_workspace_reserve_bytes`.
- Added `flush_workspace_reserve_bytes` and a temporary Tier3 pause-timeout control to control JSON, schema output, and CLI flags. The Round2 close-out above removes that timeout control from the active schema and CLI.
- Added `frame-store-temp` cleanup after successful finish or partial-output finalization, with cleanup failures logged as warnings.
- Ensured segment-write failure paths clear `active_segment_id` so checkpoint state does not falsely report an in-flight flush after degradation.
- No UI, OCR, SQLite, scheduler behavior, game logic, reconnect/retry loop, scrcpy, minicap, new capture backend, input fallback, or P2.3 capture hot-path rollback was added.
- Implementation commit: `2fdaeb71bb4778338b92ad88a5042c15ad6c90c6` (`Implement Lab-1z frame store fixes`).
- Checkpoint tag: `checkpoint/20260625-lab-1z-fixes`.

### Files changed

- `apps/actinglab/src/frame_store.rs`
- `apps/actinglab/src/lab_run.rs`
- `apps/actinglab/src/main.rs`
- `PLANS.md`
- `CHECKPOINT.md`

### Commands run

- `git fetch origin --prune --tags`
- `git pull --ff-only`
- `Get-Content -LiteralPath 'C:\合作工作区\ActingCommand\TASK-Lab-1z-fixes.md' -Encoding UTF8`
- `git status --short --branch`
- `git diff --stat`
- `rg -n "active_segment_id|write_segment|spill_admission_frame|flush_resident_segment|spill_degraded|tier3_resume|backpressure_paused|RecognitionState|similarity_threshold|flush_workspace_reserve|tier3_pause_timeout" apps/actinglab/src/frame_store.rs apps/actinglab/src/lab_run.rs apps/actinglab/src/main.rs`
- `rg -n "adb shell screencap|scrcpy|minicap|reconnect|retry loop|SQLite|OCR|thread::spawn|std::thread::spawn" apps/actinglab/src/frame_store.rs apps/actinglab/src/lab_run.rs apps/actinglab/src/main.rs`
- `cargo fmt --all`
- `cargo test -p actingcommand-actinglab`
- `adb devices -l`
- Local external-tool path checks for DroidCast_raw APK, MuMu folder, and Nemu IPC DLL.
- With `ACTINGCOMMAND_DROIDCAST_RAW_APK`, `ACTINGCOMMAND_NEMU_FOLDER`, and `ACTINGCOMMAND_NEMU_IPC_DLL` set to reviewed local paths:
  - `cargo run -p actingcommand-device-test -- --port 16416 benchmark --rounds 3`
- `cargo fmt --all -- --check`
- `cargo test --workspace`
- `cargo clippy --workspace -- -D warnings`
- `git diff --check`
- Prohibited-scope scans over touched ActingLab files for UI-adjacent and capture-backend forbidden terms.

### New unit coverage

- `completed_no_match_frames_do_not_same_page_dedupe`
- `matched_same_page_frames_can_dedupe`
- `page_transition_is_retained_even_under_dedup`
- `single_frame_can_spill`
- `last_frame_can_spill_when_eligible`
- `tier3_returns_pause_required_on_current_frame`
- `tier2_spills_segment_without_pausing`
- `spilled_segment_materializes_to_screenshot_file`
- `tier3_alarm_still_materializes_partial_screenshots`
- `threshold_one_is_rejected`
- `resident_bytes_include_payload_metadata_thumbnail_and_workspace`
- `spill_io_failure_degrades_without_panic`
- `cleanup_temp_removes_segment_directory`
- `tier2_tier3_gap_too_small_is_rejected`
- `clock_rollback_does_not_underflow_dwell_delta`
- `thumbnail_handles_pathological_dimensions_without_panic`
- `hysteresis_releases_only_below_release_line`
- `memory_budget_uses_available_total_and_os_reserve`

### Benchmark result

- Device: Arknights `127.0.0.1:16416`.
- `device-test benchmark --rounds 3` succeeded for `adb_screencap`, `droidcast_raw`, and `nemu_ipc`.
- `adb_screencap`: `1280x720`, capture-only best/median/p90 `617/626/857ms`, encode-only median `139ms`, end-to-end median `626ms`.
- `droidcast_raw`: `1280x720`, capture-only best/median/p90 `342/393/1195ms`, encode-only median `108ms`, end-to-end median `502ms`.
- `nemu_ipc`: `1280x720`, capture-only best/median/p90 `27/29/29ms`, encode-only median `154ms`, end-to-end median `178ms`.
- P2.3 raw capture hot path remains in the expected tens-of-milliseconds range for Nemu IPC.
- This task does not claim a 300ms full pipeline target.

### Test results

- `cargo test -p actingcommand-actinglab` passed with 41 tests.
- `cargo test --workspace` passed.
- `cargo fmt --all -- --check` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `git diff --check` passed.
- Prohibited-scope scans found no new UI, SQLite, OCR, reconnect/retry loop, background flush thread, scrcpy, minicap, or `adb shell screencap` path in the touched ActingLab files.

### Current blocker

- No implementation blocker.
- Live Arknights `16416` Lab package execution was not run as gameplay validation in this pass. The live validation performed here was the capture benchmark only.
- Tier3 pause remains a synchronous graceful failure/finalization path in this task; a richer resumable paused service loop belongs to a later Runtime-service milestone.

### Next step

1. Commit, checkpoint-tag, and push the Runtime Lab-1z fixes with planning files.
2. If Alice wants gameplay validation, run a safe Arknights `16416` Lab package separately after selecting a reviewed package and confirming device state.
3. Plan a later Runtime-service milestone for true paused-run resume rather than extending the synchronous one-shot Lab path here.

## 2026-06-25 P2.3 capture pipeline refactor

### Current status

- Re-read the updated `C:\合作工作区\ActingCommand\TASK-P2.3-capture-pipeline.md` and implemented the revised mainline capture pipeline task.
- Refactored `Frame` so capture backends return raw pixels plus metadata without encoding PNG in `Frame::from_pixels`.
- Added optional `Frame::original_png` for ADB screencap frames and `Frame::png_for_artifact()` for save paths.
- Added fast PNG artifact encoding with `CompressionType::Fast` and `FilterType::NoFilter`.
- Added `Scene::from_rgb8`, `Scene::from_rgba8`, and `Scene::from_pixels` so recognition consumers can use raw captured pixels directly.
- Updated `device-test`, `actinglab`, Lab-1y capture loops, probe-run capture, and `CaptureStore` to use raw-frame recognition and artifact-only PNG writes.
- Cached Nemu IPC resolution at backend initialization and reused the cached dimensions per capture.
- Updated `device-test benchmark` to report capture-only, encode-only, and end-to-end capture-plus-artifact timing per backend.
- No Lab deduplication, frame-store watermarks, UI, OCR, SQLite, scheduler, game logic, ADB input fallback, scrcpy, minicap, or new fast screenshot backend was added.

### Files changed

- `crates/device/src/capture.rs`
- `crates/recognition/src/lib.rs`
- `crates/runtime-core/src/capture_store.rs`
- `apps/device-test/src/main.rs`
- `apps/device-test/src/probe_run.rs`
- `apps/actinglab/src/main.rs`
- `apps/actinglab/src/lab_run.rs`
- `PLANS.md`
- `CHECKPOINT.md`

### Commands run

- `git fetch origin --prune --tags`
- `git pull --ff-only`
- `Get-Content -LiteralPath 'C:\合作工作区\ActingCommand\TASK-P2.3-capture-pipeline.md' -Encoding UTF8`
- `cargo check --workspace`
- `cargo test --workspace`
- `cargo fmt --all`
- `cargo clippy --workspace -- -D warnings`
- `cargo fmt --all -- --check`
- `git diff --check`
- `adb devices -l`
- With `ACTINGCOMMAND_DROIDCAST_RAW_APK`, `ACTINGCOMMAND_NEMU_FOLDER`, and `ACTINGCOMMAND_NEMU_IPC_DLL` set to local reviewed external-tool paths:
  - `cargo run -p actingcommand-device-test -- --port 16416 benchmark --rounds 3`
- Prohibited-feature scans for old `frame.png` field usage, `adb shell screencap`, `shell screencap`, `scrcpy`, `minicap`, reconnect, retry-loop, OCR, and SQLite terms in Runtime paths.

### Live benchmark result

- Device: Arknights `127.0.0.1:16416`.
- External tools:
  - DroidCast_raw APK: `C:\.ClaudeCode\upstream-refs\AzurPilot\bin\DroidCast\DroidCast_raw-release-1.1.apk`
  - MuMu folder: `D:\BST\MuMuPlayer`
  - Nemu IPC DLL: `D:\BST\MuMuPlayer\nx_device\12.0\shell\sdk\external_renderer_ipc.dll`
- `device-test benchmark --rounds 3` succeeded for all three backends:
  - `adb_screencap`: `1280x720`, capture-only median `632ms`, encode-only median `142ms`, end-to-end median `632ms`.
  - `droidcast_raw`: `1280x720`, capture-only median `376ms`, encode-only median `118ms`, end-to-end median `482ms`.
  - `nemu_ipc`: `1280x720`, capture-only median `26ms`, encode-only median `136ms`, end-to-end median `164ms`.
- Nemu IPC capture-only is now in the expected tens-of-milliseconds range.
- The Nemu IPC DLL still printed external diagnostic lines before the benchmark output; this remains the known stdout-isolation residual.

### Test results

- `cargo check --workspace` passed.
- `cargo test --workspace` passed.
- `cargo fmt --all -- --check` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `git diff --check` passed.
- Prohibited scans found no `adb shell screencap`, no `shell screencap`, no `scrcpy`, no `minicap`, no reconnect or retry-loop logic in the changed capture path, and no remaining `frame.png` struct-field usage.
- Scan hits for `frame.png` were only literal output file names or `png_for_artifact()` method calls; the OCR hit was the pre-existing primitive contract method name, not new OCR implementation.

### Current blocker

- No blocker for P2.3.
- Later stdout isolation is still needed if Nemu-backed CLI output must be strict machine-readable JSON.
- Further PNG cost reduction should be handled by later frame-store/persistence design, not by adding Lab deduplication in P2.3.

### Next step

1. Run final verification after documentation updates.
2. Commit and push the Runtime P2.3 implementation with `PLANS.md` and `CHECKPOINT.md`.
3. Plan a separate stdout-isolation task for external Nemu IPC DLL diagnostics if strict JSON output is required.
4. Keep Lab deduplication / frame-store watermarks for the separate Lab-1z branch.

## 2026-06-25 P2.2 capture backend repair close-out

### Current status

- Re-read the updated `TASK-P2.2-capture-backend-fixes.md` and adjusted the implementation to the revised DroidCast rule: do not rotate frames that are already in the Runtime display coordinate size.
- Fixed Nemu IPC path passing:
  - `nemu_connect` now takes `*const u16`;
  - the MuMu folder path is passed as NUL-terminated UTF-16;
  - `disconnect` and `capture_display` signatures remain unchanged.
- Fixed DroidCast_raw display orientation:
  - reads MuMu natural screen size separately from display coordinate size;
  - reads active display orientation from `dumpsys display`, with `settings get system user_rotation` as a secondary source;
  - requests the orientation-adjusted DroidCast endpoint size;
  - decodes the raw RGB565 bytes as the MuMu natural buffer when natural and display dimensions are swapped;
  - rotates only when the decoded dimensions do not already match the display coordinate size.
- Fixed `actinglab lab run --capture-backend` priority:
  - global CLI `--capture-backend` now overrides the subcommand flag, which overrides `control.json`, which overrides default `auto`.
- Fixed `auto` backend downgrade:
  - each candidate backend is probed with one capture;
  - failed initialization, connection, or first capture records a failed attempt and continues to the next backend;
  - explicit single-backend selection still fails loudly.
- No UI, OCR, SQLite, scheduler behavior, game logic, ADB input fallback, reconnect, retry loop, scrcpy, minicap, or adb shell screencap path was added.
- The local helper `tests/build_lab_pkg.py` remains untracked and retained per Alice's instruction.

### Resource repository freshness

- `ActingCommand-Resources-Arknights`: refreshed before read-only resource recognition validation; `HEAD` at `eacf3e446ab62c9b3013f653b7986a85a8bf0213`.

### Files changed

- `crates/device/src/capture.rs`
- `apps/actinglab/src/lab_run.rs`
- `apps/actinglab/src/main.rs`
- `PLANS.md`
- `CHECKPOINT.md`

### Commands run

- Re-read `C:\合作工作区\ActingCommand\TASK-P2.2-capture-backend-fixes.md`.
- Inspected AzurPilot local reference files:
  - `C:\.ClaudeCode\upstream-refs\AzurPilot\module\device\method\droidcast.py`
  - `C:\.ClaudeCode\upstream-refs\AzurPilot\module\device\screenshot.py`
- `cargo check -p actingcommand-device`
- `cargo check -p actingcommand-actinglab`
- `cargo test -p actingcommand-device capture::tests`
- `cargo test -p actingcommand-actinglab lab_run_capture_backend_flag_is_global_even_after_subcommand`
- `adb devices -l`
- `cargo build -p actingcommand-actinglab -p actingcommand-device-test`
- `adb -s 127.0.0.1:16416 shell wm size`
- `adb -s 127.0.0.1:16416 shell settings get system user_rotation`
- `adb -s 127.0.0.1:16416 shell dumpsys display`
- `git fetch origin --prune --tags` in `ActingCommand-Resources-Arknights`
- `git pull --ff-only` in `ActingCommand-Resources-Arknights`
- `cargo fmt --all`
- `cargo fmt --all -- --check`
- `cargo test --workspace`
- `cargo clippy --workspace -- -D warnings`
- `git diff --check`
- Prohibited scans for `adb shell screencap`, `println!/eprintln!` in `crates/device`, and quick-backend/reconnect/retry terms in the touched capture/lab-run paths.

### Live validation result

- Device: Arknights `127.0.0.1:16416`.
- External tools:
  - DroidCast_raw APK: `C:\.ClaudeCode\upstream-refs\AzurPilot\bin\DroidCast\DroidCast_raw-release-1.1.apk`
  - MuMu folder: `D:\BST\MuMuPlayer`
  - Nemu IPC DLL: `D:\BST\MuMuPlayer\nx_device\12.0\shell\sdk\external_renderer_ipc.dll`
- Explicit `nemu_ipc` capture succeeded:
  - output: `target\p2_2_fix\nemu-16416.png`
  - size: `1280x720`
- Explicit `droidcast_raw` capture succeeded after the natural-buffer decode fix:
  - output: `target\p2_2_fix\droidcast-16416-readable.png`
  - size: `1280x720`
  - visual inspection showed a readable Arknights home/terminal screen, not the earlier stripe-noise image.
- `auto` backend selection succeeded:
  - normal environment selected `nemu_ipc`;
  - intentionally invalid Nemu DLL path downgraded to `droidcast_raw`;
  - intentionally invalid Nemu DLL and DroidCast APK paths downgraded to `adb_screencap`.
- `actinglab detect-page --capture --capture-backend droidcast_raw` with Arknights resources completed without a dimension mismatch.
- `actinglab lab run --capture-backend droidcast_raw` completed the existing safe `open_terminal` package:
  - output: `target\p2_2_fix\out_open_terminal_droidcast.zip`
  - run directory: `target\p2_2_fix\lab-runs\lab1y-20260625_064257_259`
  - `executed_step_count=2`
  - `screenshot_count=3`
  - `capture_backend_requested=droidcast_raw`
  - `capture_backend_used=droidcast_raw`
  - observed safe route stopped at `quickswitch_dropdown` then `terminal`.
- `device-test benchmark --rounds 3` on `127.0.0.1:16416` after rebuilding `device-test`:
  - `adb_screencap`: available, `1280x720`, median about `655ms`;
  - `droidcast_raw`: available, `1280x720`, median about `676ms`;
  - `nemu_ipc`: available, `1280x720`, median about `515ms`;
  - no 300ms capture claim is supported by this run.

### Test results

- `cargo check -p actingcommand-device` passed.
- `cargo check -p actingcommand-actinglab` passed.
- `cargo test -p actingcommand-device capture::tests` passed with 16 tests.
- `cargo test -p actingcommand-actinglab lab_run_capture_backend_flag_is_global_even_after_subcommand` passed.
- `cargo fmt --all -- --check` passed.
- `cargo test --workspace` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `git diff --check` passed.
- Prohibited scans passed:
  - no `adb shell screencap` or `shell screencap` in Runtime device/CLI paths;
  - no `println!`/`eprintln!` in `crates/device/src`;
  - no `scrcpy`, `minicap`, `reconnect`, or `retry` in the touched capture/lab-run paths.

### Current blocker

- No blocker for the P2.2 screenshot backend repair itself.
- The Nemu IPC DLL prints external diagnostic lines to process stdout before JSON output, for example during `nemu_ipc` capture. This should be handled in a later stdout-isolation task if strict machine-readable JSON is required for Nemu capture commands.
- Current Arknights resource page templates still match too broadly on the visible home/terminal-style frame; this remains resource data quality work.

### Next step

1. Commit and push this Runtime repair with `PLANS.md` and `CHECKPOINT.md`.
2. Decide whether to isolate external DLL stdout for strict JSON output.
3. Continue Arknights resource-template tightening separately from P2.2 backend repair.

## 2026-06-25 Lab-1y interpreter namespace normalization + synchronous capture cadence fix

### Current status

- Fixed Lab-1y interpreter page-id handling for namespaced detector pages such as `arknights/home` versus operation anchors such as `home`.
- Applied the same page-anchor normalization to:
  - initial page confirmation;
  - operation `from` selection;
  - `target_page` stop checks;
  - operation `to` arrival polling;
  - after-state writeback.
- Added task-scoped page evaluation for Lab-1y route execution so large page sets are not evaluated wholesale on every frame.
- Added `entry_task_id` integrity checking:
  - `control.json` remains authoritative;
  - if `resources/manifest.json` also declares `entry_task_id`, mismatches fail loudly.
- Updated `to: null` semantics:
  - `to: null` plus `verify_template: null` records `executed_unverified`;
  - `to: null` plus `verify_template` requires the template to verify.
- Kept the copied local helper script `tests/build_lab_pkg.py` in the working tree for this task. It is not committed in this checkpoint.
- No TaskRoute, full navigation model, OCR, SQLite, UI, or resource-bundle completion is claimed here.

### Resource repository freshness

- `ActingCommand-Resources-AzurLane`: refreshed before the task; `HEAD` at `b3451dd7c85ffc349f043530cf2f04f856180c12`.
- `ActingCommand-Resources-Arknights`: refreshed before the task; `HEAD` at `eacf3e446ab62c9b3013f653b7986a85a8bf0213`.
- `ActingCommand-Resources-BlueArchive`: refreshed before the task; `HEAD` at `1b52342c6e0db7b65f8a09d654ec97594921cf7b`.

### Files changed

- `apps/actinglab/src/lab_run.rs`
- `PLANS.md`
- `CHECKPOINT.md`

### Commands run

- Read `C:\合作工作区\ActingCommand\TASK-Lab-1y-fix-namespace-and-cadence.md`.
- Copied `C:\.ClaudeCode\ActingCommand\tests\build_lab_pkg.py` into `tests\build_lab_pkg.py` for local package-building assistance.
- `git status --short --branch`
- `git fetch origin --prune --tags`
- `git pull --ff-only`
- `git rev-parse HEAD` in Runtime and the three resource repositories.
- `python --version`
- `python tests\build_lab_pkg.py open_terminal navigable_route`
- `target\debug\actinglab.exe --json --instance 127.0.0.1:16416 --capture-backend auto capture --out target\actinglab-labpkg\ak16416-retest-current.png`
- `target\debug\actinglab.exe --json --run-root target\actinglab-labpkg\runs-retest lab run --zip target\actinglab-labpkg\in_open_terminal.zip --out target\actinglab-labpkg\out_open_terminal_retest.zip --instance 127.0.0.1:16416 --capture-backend auto`
- `cargo build -p actingcommand-actinglab`
- `cargo fmt --all`
- `cargo fmt --all -- --check`
- `cargo test -p actingcommand-actinglab`
- `cargo test --workspace`
- `cargo clippy --workspace -- -D warnings`
- `git diff --check`

### Live validation result

- Device: `127.0.0.1:16416`
- Package: `target\actinglab-labpkg\in_open_terminal.zip`
- Output: `target\actinglab-labpkg\out_open_terminal_retest.zip`
- Run directory: `target\actinglab-labpkg\runs-retest\lab1y-20260625_051921_950`
- Result: `ok=true`
- `executed_step_count=2`
- `screenshot_count=3`
- Observed route:
  - `home_open_quickswitch`: `home` -> `arknights/quickswitch_dropdown`
  - `quickswitch_to_terminal`: `quickswitch_dropdown` -> `arknights/terminal`
- The package stopped at `target_page=terminal`.
- The resource-consuming `terminal_start_mission` step did not run.
- No `actinglab` process or LabLease lock remained after the run.
- A previous live attempt was discarded as noisy because another program was using the same emulator.

### Test results

- `cargo test -p actingcommand-actinglab` passed with 20 tests.
- `cargo test --workspace` passed.
- `cargo fmt --all -- --check` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `git diff --check` passed.

### Current blocker

- Current Arknights page resources can match multiple coarse pages on the same frame, for example `home` and `terminal` on the home frame. The interpreter fix avoids the namespace failure, but page-template quality still needs resource work.
- The local helper `tests/build_lab_pkg.py` remains untracked for this task and should be either promoted deliberately as an offline test helper or removed later.

### Next step

1. Commit and push this Runtime fix with `PLANS.md` and `CHECKPOINT.md`.
2. Improve Arknights page templates/guards so `home`, `terminal`, and related quickswitch pages are not all matched by the same frame.
3. Add a committed, sanitized package-builder fixture only if this helper should become part of the repository test workflow.

## 2026-06-25 P2.2 capture backends and Lab-1y trusted execution engine

### Current status

- Implementation commit: `08fbfc0` (`runtime: add capture backend selection and Lab-1y`).
- Upgraded `crates/device` capture from ADB-only `Frame { width, height, png }` to a unified synchronous `CaptureBackend` contract.
- Added common `Frame` metadata:
  - actual width and height;
  - raw RGB/RGBA pixels;
  - PNG artifact bytes;
  - captured timestamp;
  - pixel format;
  - backend name.
- Kept `adb_screencap` on `adb exec-out screencap -p`; no `adb shell screencap` path was added.
- Added `droidcast_raw` backend behind an external APK boundary:
  - discovers APK only through `ACTINGCOMMAND_DROIDCAST_RAW_APK`;
  - pushes the reviewed local APK to `/data/local/tmp/DroidCast_raw.apk`;
  - starts `app_process` and reads `/screenshot` RGB565 frames when the external tool is available.
- Added `nemu_ipc` backend behind a Windows MuMu external DLL boundary:
  - discovers `external_renderer_ipc.dll` through `ACTINGCOMMAND_NEMU_FOLDER`, `ACTINGCOMMAND_NEMU_IPC_DLL`, or common MuMu install paths;
  - maps MuMu instance id from ADB serial ports such as `127.0.0.1:16384`;
  - converts bottom-up BGRA output into RGBA frames.
- Added `--capture-backend <auto|adb|droidcast_raw|nemu_ipc>` to `device-test` and `actinglab`.
- Added backend diagnostics to `actinglab capture`, `actinglab lab run`, and `device-test benchmark`.
- Updated `device-test benchmark` to report availability and latency rows for `adb_screencap`, `droidcast_raw`, and `nemu_ipc`.
- Upgraded `actinglab lab run` control handling to `Lab-1y.control.v1`.
- Added Lab-1y execution modes:
  - `navigable_route`;
  - `recognize_only`;
  - `in_page_guard`.
- Added local per-instance LabLease lock files under `%LOCALAPPDATA%\ActingCommand\actinglab\locks`.
- Added Lab output stats:
  - actual capture interval min/median/max;
  - capture duration min/median/max;
  - action duration min/median/max;
  - loop lag min/median/max;
  - capture backend requested/used and backend attempts.
- Added `external-tools/NOTICE.md` documenting that DroidCast APKs and MuMu/Nemu DLLs are local-only and not committed.
- No UI, OCR, SQLite, scheduler implementation, game logic, ADB input fallback, or package script execution was added.

### Files changed

- `.gitignore`
- `Cargo.lock`
- `crates/device/Cargo.toml`
- `crates/device/src/adb.rs`
- `crates/device/src/capture.rs`
- `crates/device/src/lib.rs`
- `crates/device/src/maatouch.rs`
- `crates/runtime-core/src/capture_store.rs`
- `apps/device-test/src/main.rs`
- `apps/actinglab/src/main.rs`
- `apps/actinglab/src/lab_run.rs`
- `external-tools/NOTICE.md`
- `PLANS.md`
- `CHECKPOINT.md`

### Resource repository freshness

- `ActingCommand-Resources-AzurLane`: refreshed by `git fetch origin --prune --tags` and `git pull --ff-only`; `HEAD` and `origin/main` at `b3451dd7c85ffc349f043530cf2f04f856180c12`.
- `ActingCommand-Resources-Arknights`: refreshed by `git fetch origin --prune --tags` and `git pull --ff-only`; `HEAD` and `origin/main` at `eacf3e446ab62c9b3013f653b7986a85a8bf0213`.
- `ActingCommand-Resources-BlueArchive`: refreshed by `git fetch origin --prune --tags` and `git pull --ff-only`; `HEAD` and `origin/main` at `1b52342c6e0db7b65f8a09d654ec97594921cf7b`.

### Commands run

- `git fetch origin --prune --tags`
- `git pull --ff-only`
- `git status --short --branch`
- `git rev-parse HEAD`
- `git fetch origin --prune --tags` and `git pull --ff-only` in all three resource repositories.
- `cargo check -p actingcommand-device`
- `cargo check -p actingcommand-device-test`
- `cargo check -p actingcommand-actinglab`
- `cargo check --workspace`
- `cargo test -p actingcommand-device -p actingcommand-runtime-core -p actingcommand-device-test -p actingcommand-actinglab`
- `cargo fmt --all`
- `cargo test --workspace`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace -- -D warnings`
- `cargo build -p actingcommand-actinglab -p actingcommand-device-test`
- `adb devices -l`
- `target\debug\actinglab.exe --json --instance 127.0.0.1:16384 --capture-backend auto capture --out target\p2_2_smoke\capture-16384.png`
- `target\debug\actinglab.exe --json --instance 127.0.0.1:16416 --capture-backend auto capture --out target\p2_2_smoke\capture-16416.png`
- `cargo run -q -p actingcommand-device-test -- --port 16384 capture --out target\p2_2_smoke\device-test-capture-16384.png`
- `cargo run -q -p actingcommand-device-test -- --port 16384 benchmark --rounds 2`
- `target\debug\actinglab.exe --json schema control`
- `target\debug\actinglab.exe --json capabilities`
- `target\debug\actinglab.exe --json --run-root target\p2_2_smoke\lab-runs lab run --zip target\p2_2_smoke\missing.zip --out target\p2_2_smoke\missing-output.zip`
- `.NET ZipFile OpenRead` over `target\p2_2_smoke\missing-output.zip`
- `git diff --check`
- Prohibited/binary scans for `adb shell screencap`, ADB input fallback strings, APK/DLL files tracked by git, OCR, SQLite, and OpenCV terms in the changed Runtime paths.

### Test results

- `cargo test --workspace` passed.
- `cargo fmt --all -- --check` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `git diff --check` passed.
- `actinglab capture` live smoke on `127.0.0.1:16384` succeeded:
  - backend used: `adb_screencap`;
  - frame size: `1280x720`;
  - auto attempts recorded missing Nemu IPC DLL and missing DroidCast_raw APK before ADB fallback.
- `actinglab capture` live smoke on `127.0.0.1:16416` succeeded:
  - backend used: `adb_screencap`;
  - frame size: `1280x720`;
  - auto attempts recorded missing Nemu IPC DLL and missing DroidCast_raw APK before ADB fallback.
- `device-test capture` on `127.0.0.1:16384` succeeded:
  - backend used: `adb_screencap`;
  - frame size: `1280x720`.
- `device-test benchmark --rounds 2` on `127.0.0.1:16384` succeeded:
  - `adb_screencap` available at `1280x720`;
  - `droidcast_raw` unavailable because `ACTINGCOMMAND_DROIDCAST_RAW_APK` is not set;
  - `nemu_ipc` unavailable because `external_renderer_ipc.dll` was not found;
  - MaaTouch control measurement remains `command_submission_only`.
- `actinglab schema control` reports `Lab-1y.control.v1` with `navigable_route`, `recognize_only`, `in_page_guard`, and capture backend choices.
- `actinglab capabilities` reports capture backend availability requirements.
- Failure-output smoke for a missing Lab zip returned non-zero and wrote an output zip containing only `logs/` and `screenshots/` roots.
- Git-tracked binary scan found no `.apk`, `.dll`, `.exe`, `.msi`, or `.jar` files.

### Current blocker

- DroidCast_raw live validation is blocked until a reviewed DroidCast_raw APK is supplied through `ACTINGCOMMAND_DROIDCAST_RAW_APK`.
- Nemu IPC live validation is blocked until a reviewed MuMu `external_renderer_ipc.dll` path is supplied or a supported MuMu install is discoverable.
- Full Lab-1y live navigation validation is blocked until a trusted Lab-1y input package is provided for a known current device state.

### Next step

1. Commit and push the Runtime P2.2/Lab-1y implementation.
2. Supply reviewed DroidCast_raw and/or Nemu IPC external tool paths, then rerun backend benchmark.
3. Build or provide a trusted Lab-1y live package and run `actinglab lab run` against a known emulator state.
4. Keep UI, OCR, SQLite, scheduler, and game logic out of Runtime until separate scoped milestones.

## 2026-06-24 Lab-1X trusted one-shot package execution engine

### Current status

- Implemented `actinglab lab run --zip <input.zip> --out <output.zip>` as the trusted one-shot Lab entry.
- Added Lab-1X package ingest for root-level `control.json` plus `resources/`.
- Added strict input zip validation:
  - rejects zip-slip, unsafe separators, absolute/drive paths, duplicate paths, and executable/script-style extensions;
  - rejects missing `control.json` or missing `resources/`;
  - accepts UTF-8 JSON files with a Windows UTF-8 BOM while still failing malformed JSON loudly.
- Added Lab-1X control validation for schema, execution mode, package metadata, resolution, and capture interval.
- Added resource validation for:
  - `resources/manifest.json`;
  - `resources/operations/<entry_task_id>/task.json`;
  - generated recognition pack/pages;
  - operation anchors and verify templates;
  - optional navigation JSON when present.
- Added Operation Bundle v0.3 execution support for trusted click/drag operations:
  - current page detection through existing recognition/page-detector crates;
  - operation selection from current page;
  - coordinate bounds and unresolved-coordinate rejection;
  - ScreencapBackend capture and MaaTouchBackend input;
  - page confirmation through target page or verify template;
  - actual click point logging with seed, algorithm, source rect, and final point.
- Added output zip generation with only `logs/` and `screenshots/` roots:
  - `logs/events.jsonl`;
  - `logs/summary.json`;
  - `logs/result.md`;
  - `logs/diagnostics.json`;
  - `logs/environment.json`;
  - `logs/recognition.jsonl`;
  - timestamp-based screenshot names when captures succeed.
- Added failure-output behavior: device/resource/runtime failures return non-zero and still write an output zip when possible.
- Added `lab run` to `actinglab capabilities`.
- No UI, SQLite, OCR, scheduler, resident Runtime service, alternate screenshot backend, raw ADB input fallback, package scripts, or semantic safety screening was added.

### Files changed

- `apps/actinglab/src/main.rs`
- `apps/actinglab/src/lab_run.rs`
- `PLANS.md`
- `CHECKPOINT.md`

### Resource repository freshness

- `ActingCommand-Resources-AzurLane`: refreshed by `git fetch origin --prune --tags` and `git pull --ff-only`; `HEAD` and `origin/main` at `b3451dd7c85ffc349f043530cf2f04f856180c12`.
- `ActingCommand-Resources-Arknights`: refreshed by `git fetch origin --prune --tags` and `git pull --ff-only`; `HEAD` and `origin/main` at `eacf3e446ab62c9b3013f653b7986a85a8bf0213`.
- `ActingCommand-Resources-BlueArchive`: refreshed by `git fetch origin --prune --tags` and `git pull --ff-only`; already up to date at `1b52342c6e0db7b65f8a09d654ec97594921cf7b`.

### Commands run

- `git fetch origin --prune --tags`
- `git pull --ff-only`
- `git status --short --branch`
- `git rev-parse HEAD`
- `git fetch origin --prune --tags` and `git pull --ff-only` in all three resource repositories.
- `cargo fmt --all`
- `cargo test -p actingcommand-actinglab`
- `cargo clippy -p actingcommand-actinglab -- -D warnings`
- `cargo fmt --all -- --check`
- `cargo test --workspace`
- `cargo clippy --workspace -- -D warnings`
- `git diff --check`
- Offline smoke:
  - `cargo run -p actingcommand-actinglab -- --json --run-root target\lab1x-smoke\runs lab run --zip target\lab1x-smoke\input.zip --out target\lab1x-smoke\output.zip --instance 127.0.0.1:1 --capture-interval-ms 300`
- Output package inspection:
  - `.NET ZipFile OpenRead` over `target\lab1x-smoke\output.zip`
- Prohibited-feature scan over Runtime source paths for raw `adb shell input`, fallback/reconnect, alternate screenshot backends, OCR, SQLite, and UI terms.

### Test results

- `cargo test -p actingcommand-actinglab` passed: 15 tests.
- `cargo clippy -p actingcommand-actinglab -- -D warnings` passed.
- `cargo fmt --all -- --check` passed.
- `cargo test --workspace` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `git diff --check` passed.
- Exact prohibited-feature scans found no `adb shell input`, `input tap`, `input swipe`, `adb shell screencap`, alternate screenshot backend names, OCR, or SQLite usage in the Lab-1X implementation path.
- Offline device-unreachable smoke returned:
  - JSON envelope `ok=false`;
  - error code `device_error`;
  - process exit code `4`;
  - failure report written to `target\lab1x-smoke\output.zip`.
- Output zip root entries were limited to:
  - `logs/`
  - `screenshots/`
  - files under `logs/`
- No real-device click validation was run in this pass because no trusted Lab-1X live package was provided for a selected device state. The offline device-unreachable acceptance path was verified without clicking.

### Current blocker

- No blocker for the implemented Lab-1X CLI engine and offline acceptance path.
- Live one-shot execution should be validated next with a trusted Lab-1X package selected for a known current device state.

### Next step

1. Commit and push the Lab-1X Runtime implementation.
2. Build or provide a trusted Lab-1X live package for one known emulator state.
3. Run `actinglab lab run` against that emulator and inspect `output.zip` screenshots/logs.

## 2026-06-24 ActingLab read-only resource recognition bridge

### Current status

- Added direct read-only device/resource execution for narrow `actinglab` commands that do not execute clicks:
  - `devices`
  - `capture`
  - `detect-page`
  - `recognize`
- Removed the resident Runtime requirement from those commands while leaving stateful/reserved flows behind Runtime boundaries.
- Added generated-resource resolution for `detect-page` and `recognize`:
  - explicit `--pack`, `--pack-root`, and `--pages` still work;
  - `--resource-root <repo> --game <alias>` resolves the expected generated recognition pack/pages path;
  - default servers are `cn` for Arknights and `jp` for AzurLane/BlueArchive.
- Added game aliases:
  - Arknights: `ak`, `ark`, `arknights`
  - AzurLane: `azur`, `azurlane`, `azur_lane`, `al`
  - BlueArchive: `ba`, `bluearchive`, `blue_archive`
- Added bare `--instance 127.0.0.1:<port>` handling as an ADB serial when no configured instance matches.
- Updated command capabilities so read-only device commands advertise `device` instead of `running_runtime`.

### Files changed

- `apps/actinglab/src/main.rs`
- `PLANS.md`
- `CHECKPOINT.md`

### Resource repository freshness

- `ActingCommand-Resources-Arknights`: `HEAD` and `origin/main` at `12eca5b881d5c1fe50a21da7c1c5309e9d14c530`.
- `ActingCommand-Resources-AzurLane`: `HEAD` and `origin/main` at `3ec82d4b1bd28ffcba29e6aedbefa6f7b59a3d38`.
- Both resource repositories were clean before Runtime commit validation.

### Live read-only retest after game restart

- Existing `target\debug\actingcommand-device-test.exe` was used for live resource retest before this commit.
- No taps, swipes, probe-run clicks, package runs, or source edits were performed during that retest.
- AK on `127.0.0.1:16416`:
  - capture succeeded at `1280x720`;
  - `detect-page --page arknights/home --capture` matched;
  - `recognize --target page/home --capture` passed with score `0.999885` and threshold `0.800000`.
- AzurLane JP on `127.0.0.1:16384`:
  - capture succeeded at `1280x720`;
  - visible screen was main/home;
  - `detect-page --page azurlane/home --capture` did not match;
  - `recognize --target page/home --capture` had template score `0.969314` over threshold `0.900000`, but failed the color gate;
  - observed `page/home` color mean was `223,225,224` versus expected `107,164,233`, with color distance `131.369705` over max `20.000000`;
  - `detect-page --page azurlane/campaign --capture` matched on the same home screen, so it should be treated as an entry-anchor match, not true campaign page-state evidence.

### Current blocker

- AzurLane `page/home` resource data is stale for the current JP main/home UI; refresh that resource anchor before setting AzurLane `verified_live=true`.
- AzurLane entry-anchor page definitions such as `azurlane/campaign` need tightening so visible home-screen buttons do not count as true target pages.

### Commands run

- `git fetch origin --prune --tags`
- `git fetch origin --prune --tags` for `ActingCommand-Resources-Arknights`
- `git fetch origin --prune --tags` for `ActingCommand-Resources-AzurLane`
- `cargo fmt --all`
- `cargo test -p actingcommand-actinglab`
- `cargo clippy -p actingcommand-actinglab -- -D warnings`
- `git diff --check`

### Test results

- `cargo test -p actingcommand-actinglab` passed: 12 tests.
- `cargo clippy -p actingcommand-actinglab -- -D warnings` passed.
- `git diff --check` passed.
- Runtime `HEAD` matched `origin/main` before commit validation: `b72bd398c6ce98760ca5db6d25b08d11ea8009f4`.

### Next step

1. Push the Runtime commit after validation.
2. Refresh AzurLane `page/home` in `ActingCommand-Resources-AzurLane` and rerun live `azurlane/home`.

## 2026-06-24 ActingLab-P1g global CLI contract shell

### Current status

- Implemented `apps/actinglab` as the user-facing `actinglab` CLI entry for the Runtime repository.
- Added a unified JSON envelope for all CLI commands with `schema_version`, `ok`, `command`, `data` or `error`, `cli_version`, and `runtime_version`.
- Added fixed exit-code mapping:
  - `0`: ok
  - `2`: usage or validation failure
  - `3`: safety blocked
  - `4`: device or instance issue
  - `5`: runtime not running
  - `6`: reserved or not implemented
- Added offline command support for `--version`, `paths`, `doctor`, `capabilities`, `schema`, `list`, `config get/set`, `resource validate/check-release`, `package validate/inspect`, and `operation validate/inspect/explain`.
- Added package zip safety validation:
  - zip-slip path rejection;
  - backslash, absolute path, drive-prefix, and duplicate path rejection;
  - `.py`, `.exe`, `.bat`, `.cmd`, `.ps1`, and `.sh` entry rejection;
  - required manifest and task path checks;
  - declared hash verification for `hashes` and `files` manifest forms.
- Added structured safety/stub behavior for commands whose Runtime services are not connected yet:
  - `status` returns `runtime_not_running` when no Runtime endpoint is configured or reachable.
  - `scheduler *` returns `scheduler_not_available`.
  - `package run`, `operation run`, and `control probe-click` require an exclusive `LabLease` and fail visibly instead of faking success.
- Added `detect-page` standby behavior for scene-driven validation: no matched page returns `page = "standby"` with a recovery hint and no automatic click.
- Added Windows launcher and user PATH helper scripts under `scripts/actinglab`.
- Applied a mechanical `while let` clippy cleanup in `benchmarks/rust/src/main.rs` so workspace clippy passes under `-D warnings`.
- No UI, SQLite, OCR, scheduler implementation, game logic, or real package-run execution was added.
- No resource repository content was read during validation, so the resource freshness gate was not triggered for this task.

### Files changed

- `Cargo.toml`
- `Cargo.lock`
- `benchmarks/rust/src/main.rs`
- `apps/actinglab/Cargo.toml`
- `apps/actinglab/src/main.rs`
- `scripts/actinglab/actinglab.cmd`
- `scripts/actinglab/actinglab.ps1`
- `scripts/actinglab/install-user-path.ps1`
- `scripts/actinglab/uninstall-user-path.ps1`
- `PLANS.md`
- `CHECKPOINT.md`

### Commands run

- Read task file: `C:\合作工作区\ActingCommand\TASK-ActingLab-P1g-CLI.md`
- Read task spec: `C:\Users\Alice\Downloads\ActingLab-P1g_CLI_package_monitor_scheduler_task_verified.json`
- Read Runtime-local `AGENTS.md`, `PLANS.md`, `CHECKPOINT.md`, and `NOTICE.md`.
- Checked Runtime repository status.
- Inspected existing `apps/device-test`, `crates/runtime-core`, `crates/recognition-pack`, `crates/page-detector`, and `crates/task-loop`.
- `cargo fmt --all`
- `cargo test -p actingcommand-actinglab`
- `cargo fmt --all -- --check`
- `cargo test --workspace`
- `cargo clippy -p actingcommand-actinglab -- -D warnings`
- `cargo clippy --workspace -- -D warnings`
- `scripts\actinglab\actinglab.cmd --json --version`
- `powershell -ExecutionPolicy Bypass -File scripts\actinglab\actinglab.ps1 --json --version`
- `target\debug\actinglab.exe --json capabilities`
- `target\debug\actinglab.exe --json status`
- `target\debug\actinglab.exe --json scheduler status`

### Test results

- `cargo test -p actingcommand-actinglab` passed with 10 tests.
- `cargo fmt --all -- --check` passed.
- `cargo test --workspace` passed.
- `cargo clippy -p actingcommand-actinglab -- -D warnings` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `.cmd` launcher smoke test returned a valid `--version` JSON envelope with exit code 0.
- `.ps1` launcher smoke test returned a valid `--version` JSON envelope with exit code 0.
- `actinglab --json capabilities` returned exit code 0 and reported command `needs` plus recognition match-metric policy.
- `actinglab --json status` returned exit code 5 with `runtime_not_running`, as expected when no Runtime endpoint is configured.
- `actinglab --json scheduler status` returned exit code 6 with `scheduler_not_available`, as expected for the reserved scheduler interface.

### Current blocker

- A resident Runtime service endpoint is not implemented yet, so Runtime-attached commands can only expose stable errors or endpoint probing.
- Real package-run execution, monitor frame streaming, Runtime lab sessions, and scheduler control remain reserved until the Runtime service and LabLease APIs are connected.
- `actinglab` currently contains some scene-driven read-only recognition plumbing for validation; the next Runtime milestone should move active recognition/capture command execution behind a Runtime service boundary so the CLI remains a pure user-facing entry.

### Next step

1. Commit and push the P1g CLI contract shell.
2. Connect `actinglab status/devices/lab/monitor` to a resident Runtime endpoint.
3. Move active `capture`, `detect-page`, `recognize`, `operation dry-run`, and `package run` execution behind the Runtime service boundary.
4. Implement real package-run only after LabLease, navigation-only, and expect-after Runtime gates are connected.

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

## 2026-06-24 ActingLab-P1a/P1b Rust scheduler-gate skeleton

### Current status

- Added the first Runtime-embedded ActingLab Rust module under `actingcommand-runtime-core`.
- Implemented pure state and scheduler-gate decision contracts for:
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
- `exclusive_drain` now has a pure decision model:
  - idle scoped instances produce `LeaseAcquired`;
  - running scoped instances produce `DrainingCurrentTask`;
  - manual-review-blocked scoped instances produce `Failed`;
  - click permission is true only after lease acquisition and only for `NavigationOnlyOnly`.
- `passive_mirror` is modeled as no-click and no-defer. It does not drain running tasks.
- `scheduler_noop` is modeled as no-click but scheduler-deferring for scoped instances.
- This is contract/state work only. It does not start devices, capture frames, run recognition, execute clicks, write journals, mutate scheduler queues, add UI, add SQLite, add OCR, or touch resource repositories.

### Files changed

- `crates/runtime-core/src/actinglab.rs`
- `crates/runtime-core/src/lib.rs`
- `PLANS.md`
- `CHECKPOINT.md`

### Commands run

- `git status --short --branch`
- `git log -5 --oneline --decorate`
- `git diff --stat`
- `Get-Content -Raw AGENTS.md`
- `Get-Content -Raw PLANS.md`
- `Get-Content -First 220 CHECKPOINT.md`
- `Get-Content -Raw Cargo.toml`
- `Get-ChildItem -Directory crates`
- `Get-Content -Raw crates\runtime-core\Cargo.toml`
- `Get-Content -Raw crates\runtime-core\src\lib.rs`
- `Get-Content -Raw crates\runtime-core\src\capture_store.rs`
- `git show --stat --oneline 72edc17`
- `git show --stat --oneline e24539f`
- `cargo fmt --all`
- `cargo test -p actingcommand-runtime-core`
- `cargo fmt --all -- --check`
- `cargo test --workspace`
- `cargo clippy -p actingcommand-runtime-core -- -D warnings`
- `rg -n "adb|MaaTouch|screencap|CaptureBackend|OCR|ocr|SQLite|sqlite|rusqlite|OpenCV|opencv|tap\(|swipe\(|long_tap\(|background loop|retry loop|reconnect" crates\runtime-core\src\actinglab.rs`
- `git diff --check`

### Test results

- Initial `cargo test -p actingcommand-runtime-core` caught a `HashMap` lookup type issue; fixed.
- Second `cargo test -p actingcommand-runtime-core` caught an incorrect passive-mirror draining decision; fixed so passive mirror remains no-click/no-defer/no-drain.
- Final `cargo test -p actingcommand-runtime-core` passed with 16 tests.
- `cargo fmt --all -- --check` passed.
- `cargo test --workspace` passed.
- `cargo clippy -p actingcommand-runtime-core -- -D warnings` passed.
- Prohibited-feature scan over `crates\runtime-core\src\actinglab.rs` found no ADB, MaaTouch, screencap, capture backend, OCR, SQLite, OpenCV, click execution, retry loop, background loop, or reconnect usage.
- `git diff --check` passed.

### Current blocker

- None for the Rust ActingLab scheduler-gate skeleton.
- No real scheduler integration exists yet; this milestone only defines the state and gate-decision contract.

### Next step

1. Commit and push the Runtime skeleton.
2. In a later milestone, connect `SchedulerGate` to real Runtime scheduler state and journal/frame-stream contracts.

## 2026-06-24 ActingLab-P1 runtime-embedded cleanup audit

### Current status

- Read `C:\Users\Alice\Downloads\ActingLab-P1_runtime_embedded_lab_task_chain_v3.json`.
- Confirmed the new ActingLab direction: ActingLab-P1 is a Runtime-embedded Rust module, not a standalone Python Lab/runtime debug program.
- Confirmed Runtime-side Lab code must reuse Runtime capture, recognition, page detection, input/click safety, scheduler gate, poll, journal, and frame-store modules.
- Confirmed resource-repository Python offline tools, such as importers, drift guards, and converters, are not part of this Runtime-side cleanup.
- Audited Runtime history for Python runtime/debug code.
- Found the old Python runtime prototype was added by `3f04663` as `runtime/alice_runtime_orchestrator.py`.
- Found it was already removed from the Rust mainline by `557831c` (`Move Python and Go legacy runtime out of Rust mainline`).
- Confirmed the current Runtime working tree has no tracked `.py` files.
- No revert commit was required because the bad Runtime-side Python runtime is already absent from `main`.
- Recorded the ActingLab runtime-embedded design in `PLANS.md`.
- Did not touch the pre-existing uncommitted Rust changes in `apps/device-test/src/probe_run.rs`, `crates/recognition-pack/src/lib.rs`, and `crates/recognition/src/lib.rs`.

### Files changed

- `PLANS.md`
- `CHECKPOINT.md`

### Commands run

- `Get-Content -Raw -Encoding UTF8 C:\Users\Alice\Downloads\ActingLab-P1_runtime_embedded_lab_task_chain_v3.json`
- `rg -n "ActingCommand|PLANS\.md|CHECKPOINT\.md|severe errors|resource repository" C:\Users\Alice\.codex\memories\MEMORY.md`
- `Get-Content -Raw AGENTS.md`
- `Get-Content -Raw PLANS.md`
- `Get-Content -Raw CHECKPOINT.md`
- `Get-Content -Raw NOTICE.md`
- `git status --short --branch`
- `git fetch origin`
- `git diff -- crates\recognition\src\lib.rs`
- `git log --name-status --oneline -- "*.py"`
- `git show --stat --oneline 557831c`
- `git show --name-only --oneline 557831c`
- `git show --stat --oneline 3f04663`
- `git show --stat --oneline c147a65`
- `rg --files -g "*.py"`
- `rg -n "ActingLab|actinglab|\blab\b|Python|\.py|screencap|MaaTouch|adb" apps crates contracts resources benchmarks -g "*.rs" -g "*.py" -g "*.md" -g "*.toml" -g "*.json"`
- `git diff --check`
- `cargo fmt --all -- --check`
- `cargo test --workspace`

### Test results

- `rg --files -g "*.py"` returned no files in the Runtime repository.
- `git diff --check` passed.
- `cargo test --workspace` passed.
- `cargo fmt --all -- --check` did not pass because of a pre-existing uncommitted `crates/recognition/src/lib.rs` formatting difference. This task did not modify that file and did not format or revert it.

### Current blocker

- None for the Python cleanup audit or ActingLab Runtime-embedded planning update.
- The repository still has unrelated pre-existing uncommitted modifications in `apps/device-test/src/probe_run.rs`, `crates/recognition-pack/src/lib.rs`, and `crates/recognition/src/lib.rs`; this checkpoint entry intentionally does not modify or revert them.

### Next step

1. Commit and push the Runtime planning/checkpoint update without staging the unrelated recognition change.
2. Start the next coding milestone as Rust Runtime work: `LabMode`, `LabLease`, scoped scheduler gate, and `exclusive_drain` contracts.

## 2026-06-22 P6d/P6e-half resource-independent close-out

### Current status

- Completed the resource-independent Runtime half of P6d/P6e.
- `task-loop` now rejects click actions that do not declare a non-empty `page_id`; this makes click provenance fatal at plan construction instead of allowing ambiguous clicks.
- MaaTouch remains the only input backend. The repository MaaTouch external-tool path is now preferred by default when running built binaries from `target/`.
- MaaTouch license provenance is recorded in `NOTICE.md`, and the Apache-2.0 license text is tracked at `external-tools/maatouch/LICENSE`.
- Control benchmark output now labels MaaTouch reset timing as command-submission-only and no longer derives a minimum operation interval from reset writes.
- BA live regression is blocked by resource/navigation data, not by Runtime source failure: the temporary `PAGE_TASK_CENTER` bridge matched home/returned-home frames and the manual task-center tap stayed on home.
- No OCR, SQLite, UI, scheduler, ADB input fallback, reconnect logic, long tap, swipe, purchase, refill, gacha, construction, recruitment, sortie, exercise/PvP, FreeClaim, or consuming-resource action was added or executed.

### Resource freshness

- Runtime base before this task: `2718e2a25c5b56e7a0d6fde28049c082bdddf470`
- `ActingCommand-Resources-AzurLane`: `e494e614fed2a36a8949bd909ca7e7769ded6509`
- `ActingCommand-Resources-Arknights`: `c57ff2ba8673f7878134c45a6786f11dc1810468`
- `ActingCommand-Resources-BlueArchive`: `aca24601405354e3af2fd4007c3630310e4814cf`

### Files changed

- `.gitignore`
- `NOTICE.md`
- `PLANS.md`
- `CHECKPOINT.md`
- `apps/device-test/src/main.rs`
- `crates/device/src/maatouch.rs`
- `crates/task-loop/src/probe.rs`
- `external-tools/maatouch/LICENSE`

### MaaTouch license review

- Verified upstream MaaTouch repository license through GitHub API: `MaaAssistantArknights/MaaTouch` reports Apache-2.0.
- Downloaded BAAH update package and inspected `DATA/touch.zip/LICENSE.txt`; the bundled license text is Apache-2.0.
- No separate filled copyright line was found in the bundled license appendix.
- Local tracked license destination: `external-tools/maatouch/LICENSE`.

### Benchmark results

Output directory: `target\p6d-p6e-half-benchmark-20260622`

| Port | Screenshot best / median / p90 | Screenshot grade | Control result |
| ---- | ------------------------------ | ---------------- | -------------- |
| `16384` | `508 / 533 / 660 ms` | Slow | `command_submission_only`, min interval not available |
| `16416` | `361 / 385 / 566 ms` | Slow | `command_submission_only`, min interval not available |
| `16448` | `409 / 431 / 564 ms` | Slow | `command_submission_only`, min interval not available |

### BA regression result

- Regression root: `target\regression-frames\bluearchive\jp`
- Final report: `target\regression-frames\bluearchive\jp\report.json`
- Markdown report: `target\regression-frames\bluearchive\jp\report.md`
- Final conclusion: `blocked`
- Blocker: BA task arrival anchor is not discriminative enough. The temporary `PAGE_TASK_CENTER` bridge matched returned-home/home frames, and manual tap at `navigation/home_to_task` stayed on home. Resource navigation and anchor data must be corrected before this regression can be treated as green.
- Successful safety-limited probe attempts during investigation:
  - `probe-1782138370875`
  - `probe-1782138383088`
  - `probe-1782138395030`
- Each successful probe attempt executed only the allowed navigation bridge clicks and reported:
  - `premium_currency_allowed=false`
  - `auto_refill_allowed=false`
  - `claims_executed=0`
  - `regenerating_resource_actions_executed=0`
- Safety note: one initial script invocation failed before running because the local PowerShell helper used the wrong parameter variable. No device action happened in that failed invocation.

### Commands run

- `git fetch origin`
- `git pull --ff-only`
- `git status --short --branch`
- `git rev-parse HEAD`
- `gh api repos/MaaAssistantArknights/MaaTouch/license`
- BAAH release/package inspection for `DATA/touch.zip/LICENSE.txt`
- `cargo test -p actingcommand-task-loop`
- `cargo test -p actingcommand-device`
- `cargo test -p actingcommand-device-test`
- `cargo test -p actingcommand-task-loop -p actingcommand-device-test`
- `cargo test --workspace`
- `cargo fmt --all`
- `cargo fmt --all -- --check`
- `cargo clippy -p actingcommand-task-loop -p actingcommand-device-test -- -D warnings`
- `cargo tree -p actingcommand-task-loop --depth 1`
- `cargo build --release -p actingcommand-device-test`
- `target\release\actingcommand-device-test.exe --port <port> benchmark --out <path>`
- BA `probe-run` safety-limited live validation on `127.0.0.1:16448`
- BA report generation into `target\regression-frames\bluearchive\jp`
- Prohibited-pattern scans for SQLite, OCR, OpenCV, scheduler/background loop, ADB input fallback, long-tap/swipe use in probe logic, and task-loop device dependencies
- `git diff --check`

### Test results

- `cargo test --workspace`: passed.
- `cargo fmt --all -- --check`: passed.
- `cargo clippy -p actingcommand-task-loop -p actingcommand-device-test -- -D warnings`: passed.
- `cargo tree -p actingcommand-task-loop --depth 1`: confirmed no `actingcommand-device`, SQLite, OCR, or image-processing backend dependency.
- Prohibited-pattern scans: no forbidden matches.
- `git diff --check`: passed.
- BA live regression: blocked by resource/navigation data as described above; no unsafe resource-consuming action was executed.

### Current blocker

- BA `home_to_task` navigation point and `PAGE_TASK_CENTER` arrival anchor need corrected resource data. The current temporary direct-match bridge is too weak and can match home/returned-home frames.
- FreeClaim and Consume-resource preflight remain deferred until the resource Operation Bundle is corrected.

### Next step

1. Correct BA resource navigation and task-center arrival anchor data in the resource repository.
2. Upgrade the BA task arrival anchor into a proper recognition-pack full-frame target instead of a temporary device-test bridge.
3. Re-run the BA regression from fresh captures after the corrected resources are pulled.
4. Add AzurLane and Arknights resource-independent regression coverage after their resource packs expose equivalent safe navigation anchors.

## 2026-06-22 resource repository freshness rule

### Current status

- Added a Runtime project rule: any task that reads or uses resource repository content must refresh the relevant resource repositories from remote before executing the resource-dependent step.
- The rule applies to current and future resource repositories, including AzurLane, Arknights, and BlueArchive resources.
- Dirty, missing, unavailable, or non-fast-forward resource repositories are blockers unless Alice gives an explicit one-off override.
- No Runtime source code was changed.

### Files changed

- `AGENTS.md`
- `PLANS.md`
- `CHECKPOINT.md`

### Commands run

- `rg -n "Azur|GachaPilot|ActingCommand|PLANS\.md|CHECKPOINT\.md|planning" C:\Users\Alice\.codex\memories\MEMORY.md`
- `Get-ChildItem -Name`
- `git status --short --branch`
- `Get-Content -Raw AGENTS.md`
- `Get-Content -Raw PLANS.md`
- `Get-Content -Raw CHECKPOINT.md`
- `Get-Content -Raw LICENSE_POLICY.md` (not present in this split Runtime repository)
- `Get-Content -Raw NOTICE.md`

### Test results

- Documentation/rule-only change; no Rust build or runtime test is required.

### Current blocker

- None.

### Next step

1. Before the next Runtime task that uses resource repository content, run `git fetch origin` and `git pull --ff-only` in each relevant resource repository.
2. Record the resource repository paths and commit hashes in this checkpoint.

## 2026-06-19 multirun request and upstream-script safety gate

### Current status

- Manually returned AzurLane JP on `127.0.0.1:16384` from sortie/chapter map to the main page.
- Confirmed `azurlane/main_white` matched after the home tap.
- Rechecked Arknights home on `127.0.0.1:16416`; `arknights/home` matched.
- BlueArchive JP on `127.0.0.1:16448` was in hidden/idle UI; captured first, visually confirmed hidden UI, sent one neutral wake tap, then confirmed `bluearchive/home`.
- Did not run upstream/original scripts for "all functions" because the current ActingCommand Runtime has no audited upstream adapter capable of enforcing the requested premium-resource and exercise/PvP safety rules across all original-script tasks.
- Ran a three-port parallel Runtime smoke instead. This used the existing safe BA navigation-only runner profile and page guards.

### Safety gate result

Direct upstream/original full-function execution was blocked by safety review.

Reasons:

- Current Runtime has no adapter-level policy gate for upstream scripts.
- Upstream script scans found real state-changing tasks:
  - Alas/AzurLaneAutoScript: `Exercise`, `GemsFarming`, `Retirement`, shop tasks, tactical/training, commission, research, and other scheduler tasks.
  - BAAS/BAAH: AP purchase, tactical challenge/arena, shop purchase, quest/sweep/fight tasks, and reward/claim tasks.
  - MAA: `Fight`, `Recruit`, `Mall`, `Award`, `Roguelike`, and other task chains.
- Some task families can consume limited daily attempts, oil/AP/sanity, shop currency, tickets, or account resources; some can approach premium-resource confirmation flows.
- "All functions except exercise" is not yet representable in ActingCommand as a reviewed allowlist with per-task resource-policy caps.
- Running those upstream scripts directly would bypass the current `ProbeClickEffect`, `ResourcePolicy`, page guard, forbidden geometry, and checkpoint controls.

### Device state and recognition

- AzurLane:
  - port: `127.0.0.1:16384`
  - home tap: `target\release\actingcommand-device-test.exe --port 16384 tap 1230 37`
  - frame: `target\resource-refresh-20260619\azur-16384-after-home-tap.png`
  - `azurlane/main_white`: matched
- Arknights:
  - port: `127.0.0.1:16416`
  - `arknights/home`: matched
- BlueArchive:
  - port: `127.0.0.1:16448`
  - pre-run frame: `target\resource-refresh-20260619\ba-16448-before-multirun.png`
  - observed state: hidden/idle UI
  - wake tap: `target\release\actingcommand-device-test.exe --port 16448 tap 640 360`
  - `bluearchive/home`: matched after wake

### Parallel smoke result

- Runner profile: `target\resource-refresh-20260619\runner-profiles\bluearchive.jp.runner.json`
- `127.0.0.1:16384`:
  - run dir: `target\multirun-20260619\16384\runner-bluearchive-jp-refresh-smoke-1781880833304`
  - result: `blocked`
  - message: `page_guard_not_matched`
  - executed: false
  - click count: 0
- `127.0.0.1:16416`:
  - run dir: `target\multirun-20260619\16416\runner-bluearchive-jp-refresh-smoke-1781880833303`
  - result: `blocked`
  - message: `page_guard_not_matched`
  - executed: false
  - click count: 0
- `127.0.0.1:16448`:
  - run dir: `target\multirun-20260619\16448\runner-bluearchive-jp-refresh-smoke-1781880833956`
  - result: `completed`
  - executed: true
  - click count: 2
  - final page: `bluearchive/home`
  - effects executed: `NavigationOnly` only
  - `claims_executed`: 0
  - `regenerating_resource_actions_executed`: 0
  - `premium_currency_allowed`: false
  - `auto_refill_allowed`: false

### Commands run

- `target\release\actingcommand-device-test.exe --port 16384 tap 1230 37`
- `target\release\actingcommand-device-test.exe --port 16384 capture --out target\resource-refresh-20260619\azur-16384-after-home-tap.png`
- `target\release\actingcommand-device-test.exe --port 16384 detect-page ... --page azurlane/main_white --capture`
- `target\release\actingcommand-device-test.exe --port 16416 detect-page ... --page arknights/home --capture`
- `target\release\actingcommand-device-test.exe --port 16448 capture --out target\resource-refresh-20260619\ba-16448-before-multirun.png`
- `target\release\actingcommand-device-test.exe --port 16448 tap 640 360`
- `target\release\actingcommand-device-test.exe --port 16448 detect-page ... --page bluearchive/home --capture`
- Three parallel `target\release\actingcommand-device-test.exe --port <port> runner ... --capture` runs for ports `16384`, `16416`, and `16448`.
- Read-only scans over upstream source directories for task names and premium/resource risk terms.

### Test results

- AzurLane home detection passed after manual home tap.
- Arknights home detection passed.
- BlueArchive hidden UI handling passed: capture first, one neutral wake tap, then home detection.
- Parallel Runtime smoke passed:
  - AzurLane and Arknights were safely blocked by page guard with zero clicks.
  - BlueArchive completed the verified navigation-only route with two clicks.
- No premium currency, paid refill, purchase confirmation, exercise/PvP, claim, or regenerating-resource consumption was executed.

### Current blocker

- Full upstream/original all-function testing needs a policy-enforced adapter layer before it can be safely run.
- The adapter must translate upstream tasks into an ActingCommand allowlist with:
  - exercise/PvP disabled;
  - premium-resource use disabled;
  - paid refill disabled;
  - purchase confirmation disabled;
  - task-specific resource caps for oil/AP/sanity/tickets;
  - explicit stop-on-confirmation behavior;
  - journaling for every state-changing action.
- ActingCommand currently has only a BA navigation-only probe fixture; AzurLane and Arknights need reviewed safe probe fixtures before real click validation beyond home detection.

### Next step

1. Define an upstream-task safety matrix before launching original scripts.
2. Add per-game safe probe fixtures for AzurLane and Arknights.
3. Add adapter-level resource policy checks before allowing original script task execution.
4. Re-run multi-open with a reviewed allowlist instead of raw upstream "all functions".

## 2026-06-19 resource refresh and live smoke revalidation

### Current status

- Refreshed the three resource repositories from their remotes with `git pull --ff-only`.
- Resource repositories were already up to date and remained clean.
- Rebuilt the release `actingcommand-device-test` binary.
- Captured fresh frames for AzurLane, Arknights, and BlueArchive.
- BlueArchive was in hidden/idle UI state on the first fresh capture, so only a neutral wake tap was sent before any BA recognition/probe action.
- Revalidated pack/page recognition and the current safe control path.
- No resource repository files were modified.
- No Runtime source code was modified in this task.

### Resource repository revisions

- AzurLane: `a72a13f`
- Arknights: `e9c2b7c`
- BlueArchive: `2fec019`

### Current live frames

- AzurLane:
  - port: `127.0.0.1:16384`
  - frame: `target\resource-refresh-20260619\azur-16384-now.png`
  - observed state: AzurLane JP sortie/chapter map, not main page
  - `azurlane/main_white`: not matched
  - action decision: no probe click; read-only detection only
- Arknights:
  - port: `127.0.0.1:16416`
  - frame: `target\resource-refresh-20260619\ark-16416-now.png`
  - observed state: Arknights home
  - `arknights/home`: matched
  - action decision: no probe click; read-only detection only
- BlueArchive:
  - port: `127.0.0.1:16448`
  - first fresh frame: `target\resource-refresh-20260619\ba-16448-now.png`
  - observed state: BlueArchive JP hidden/idle UI
  - wake action: `target\release\actingcommand-device-test.exe --port 16448 tap 640 360`
  - after-wake frame: `target\resource-refresh-20260619\ba-16448-after-wake.png`
  - `bluearchive/home`: matched after wake

### Recognition validation

- `detect-page --check-pages` passed for:
  - AzurLane JP resources
  - Arknights CN resources
  - BlueArchive JP resources
- Scene-based detection on fresh frames:
  - `azurlane/main_white`: not matched, expected for current sortie/map screen
  - `arknights/home`: matched
  - `bluearchive/home`: matched after wake

### Safe control validation

- BA probe-run:
  - command used release binary and current BlueArchive resources
  - run id: `probe-1781879689436`
  - artifact dir: `C:\Users\Alice\Documents\Azur\ActingCommand-Runtime\target\resource-refresh-20260619\probe-runs\probe-1781879689436`
  - result: `completed`
  - executed: true
  - click count: 2
  - final page: `bluearchive/home`
  - effects executed: `NavigationOnly` only
  - `claims_executed`: 0
  - `regenerating_resource_actions_executed`: 0
  - `premium_currency_allowed`: false
  - `auto_refill_allowed`: false
- Runner multi-open smoke:
  - profile: `target\resource-refresh-20260619\runner-profiles\bluearchive.jp.runner.json`
  - `127.0.0.1:16384`: `blocked`, `page_guard_not_matched`, executed false, click count 0
  - `127.0.0.1:16416`: `blocked`, `page_guard_not_matched`, executed false, click count 0
  - `127.0.0.1:16448`: `completed`, executed true, click count 2, final page `bluearchive/home`

### Commands run

- `git status --short --branch` in Runtime and all three resource repositories.
- `git fetch origin; git pull --ff-only; git rev-parse --short HEAD` in each resource repository.
- `cargo build --release -p actingcommand-device-test`
- `adb devices -l`
- `target\release\actingcommand-device-test.exe --port 16384 capture --out target\resource-refresh-20260619\azur-16384-now.png`
- `target\release\actingcommand-device-test.exe --port 16416 capture --out target\resource-refresh-20260619\ark-16416-now.png`
- `target\release\actingcommand-device-test.exe --port 16448 capture --out target\resource-refresh-20260619\ba-16448-now.png`
- `target\release\actingcommand-device-test.exe --port 16448 tap 640 360`
- `target\release\actingcommand-device-test.exe --port 16448 capture --out target\resource-refresh-20260619\ba-16448-after-wake.png`
- `target\release\actingcommand-device-test.exe detect-page ... --check-pages` for all three resource repositories.
- `target\release\actingcommand-device-test.exe detect-page ... --scene ...` for current AzurLane, Arknights, and BlueArchive frames.
- `target\release\actingcommand-device-test.exe --port 16448 detect-page ... --page bluearchive/home --capture`
- `target\release\actingcommand-device-test.exe --port 16448 probe-run ... --capture --checkpoint-frames 8`
- Three parallel runner runs for ports `16384`, `16416`, and `16448`.
- `cargo test -p actingcommand-device-test -p actingcommand-task-loop`

### Test results

- `cargo build --release -p actingcommand-device-test` passed.
- `cargo test -p actingcommand-device-test -p actingcommand-task-loop` passed:
  - `actingcommand-device-test`: 53 tests passed.
  - `actingcommand-task-loop`: 35 tests passed.
- Three resource `detect-page --check-pages` validations passed.
- BA hidden UI was handled by immediate screenshot, visible decision, one neutral wake tap, and re-detection before probe execution.

### Current blocker

- AzurLane live probe remains blocked until the device is returned to `azurlane/main_white` or a reviewed map-screen probe is defined.
- Arknights live probe remains blocked because no reviewed Arknights probe fixture/resource route exists yet.
- BlueArchive tested route remains limited to verified `NavigationOnly`; FreeClaim/AP-consuming paths are still blocked pending reviewed resources and an explicit reviewed/resume flow.

### Next step

1. Add regression frames for BA hidden UI, BA visible home, BA task center, Arknights home, and AzurLane map-vs-main negative evidence.
2. Define safe Arknights observe-only probe fixtures before any Ark click validation.
3. Return AzurLane to main page or add an explicitly reviewed map-screen read-only probe before Azur live clicks.

## 2026-06-19 P6d live validation and resource gap close-out

### Current status

- Completed P6d implementation and validation from baseline `f7a05cefaa0299a6414ac61687c7f3f6070a7f5c`.
- Probe-loop standard is now limited-resource operation with explicit `ProbeClickEffect` and `ResourcePolicy` validation.
- `device-test probe-run` now consumes the unified navigation schema used by the resource repositories.
- Page guard failure stops later clicks and records `result=blocked`.
- Forbidden geometry checks cover candidate rects, forbidden rects, forbidden point radius, and actual click points.
- Checkpoint support records frame-batch/risky-effect review artifacts and can pause with `result=paused_for_review`.
- `device-test benchmark` measures screenshot/control latency before live runs.
- `device-test runner` packages recognition, capture, probe-run, and MaaTouch control as a one-shot profile-driven unit.
- Included MaaTouch binary at `external-tools/maatouch/maatouch` after owner license review and explicit approval.
- No UI, SQLite, OCR, OpenCV, scheduler, background loop, ADB input fallback, reconnect, or retry loop was added.

### Files changed

- `.gitignore`
- `NOTICE.md`
- `PLANS.md`
- `CHECKPOINT.md`
- `apps/device-test/src/main.rs`
- `apps/device-test/src/probe_run.rs`
- `crates/task-loop/src/probe.rs`
- `external-tools/maatouch/maatouch`

### MaaTouch deployment status

- Local destination: `external-tools/maatouch/maatouch`
- Source reviewed locally: `C:\Users\Alice\Documents\Azur\upstream-sources\AzurLaneAutoScript\bin\MaaTouch\maatouch`
- Size: 13,775 bytes
- SHA256: `4EA8590CD0349CE900F39AB16EF3751DAD2356286B465B4293F80F9858C995D0`
- Input path remains `MaaTouchBackend`; no ADB input fallback is present.

### Resource repository read-only scan

- AzurLane resources:
  - commit `a72a13f`
  - pack targets: 2005
  - `pages.json` pages: 1
  - navigation pages: 42
  - navigation edges: 106
  - destructive markers: 18
  - `detect-page --check-pages`: passed
  - live executable now: blocked, current 16384 screen was AzurLane JP sortie/map state, not `azurlane/main_white`
  - FreeClaim executable now: blocked pending reviewed probe/resource targets
  - oil-consuming executable now: blocked pending resource policy and safe probe coverage
- Arknights resources:
  - commit `e9c2b7c`
  - pack targets: 2
  - `pages.json` pages: 1
  - navigation pages: 10
  - navigation edges: 18
  - destructive markers: 8
  - `detect-page --check-pages`: passed
  - live executable now: read-only home detection passed on 16416
  - operator observe: blocked pending richer targets/routes
  - sanity-consuming executable now: blocked pending resource policy and safe probe coverage
- BlueArchive resources:
  - commit `2fec019`
  - pack targets: 1
  - `pages.json` pages: 1
  - navigation pages: 20
  - navigation edges: 22
  - destructive markers: 23
  - `detect-page --check-pages`: passed
  - live executable now: yes for verified `NavigationOnly` home-to-task-and-back smoke route
  - FreeClaim executable now: not executed; claim/destructive points remain blocked
  - AP-consuming executable now: blocked pending resource policy and reviewed probe coverage

### Live device checks

- Device ports observed:
  - `127.0.0.1:16384`: AzurLane JP, current screen was sortie/map state; `azurlane/main_white` did not match.
  - `127.0.0.1:16416`: Arknights CN home; `arknights/home` matched.
  - `127.0.0.1:16448`: BlueArchive JP home after one neutral wake tap; `bluearchive/home` matched.
- Wake tap:
  - command: `target\release\actingcommand-device-test.exe --port 16448 tap 640 360`
  - purpose: reveal BA home UI from idle/hidden-UI state
  - result: MaaTouch handshake OK and subsequent BA home detection matched
- BA live probe-run:
  - command: `target\release\actingcommand-device-test.exe --port 16448 probe-run --pack C:\Users\Alice\Documents\Azur\ActingCommand-Resources-BlueArchive\recognition\bluearchive.jp.pack.json --pack-root C:\Users\Alice\Documents\Azur\ActingCommand-Resources-BlueArchive --pages C:\Users\Alice\Documents\Azur\ActingCommand-Resources-BlueArchive\recognition\bluearchive.jp.pages.json --probe apps\device-test\tests\fixtures\bluearchive.jp.probe.json --run-root C:\Users\Alice\Documents\Azur\ActingCommand-Runtime\target\probe-runs --navigation C:\Users\Alice\Documents\Azur\ActingCommand-Resources-BlueArchive\navigation\bluearchive.jp.navigation.json --capture --checkpoint-frames 8`
  - run id: `probe-1781878705771`
  - artifact dir: `C:\Users\Alice\Documents\Azur\ActingCommand-Runtime\target\probe-runs\probe-1781878705771`
  - result: `completed`
  - executed: `true`
  - click count: 2
  - effects executed: `NavigationOnly` only
  - `claims_executed`: 0
  - `regenerating_resource_actions_executed`: 0
  - `premium_currency_allowed`: false
  - `auto_refill_allowed`: false
  - purchase/refill/confirmation prompts encountered: no
  - exercise/PvP battle touched: no
  - final page: `bluearchive/home`

### Benchmark results

Release binary used: `C:\Users\Alice\Documents\Azur\ActingCommand-Runtime\target\release\actingcommand-device-test.exe`

| Port | Screenshot best | Screenshot median | Screenshot p90 | Rating | Control best | Control median | Recommended poll | Min capture interval | Min op interval |
| ---- | --------------- | ----------------- | -------------- | ------ | ------------ | -------------- | ---------------- | -------------------- | --------------- |
| 16384 | 416 ms | 564 ms | 769 ms | Slow | 0 ms | 0 ms | 1128 ms | 769 ms | 20 ms |
| 16416 | 514 ms | 620 ms | 895 ms | Slow | 0 ms | 0 ms | 1240 ms | 895 ms | 20 ms |
| 16448 | 458 ms | 641 ms | 897 ms | Slow | 0 ms | 0 ms | 1282 ms | 897 ms | 20 ms |

Control timing uses MaaTouch reset writes, so it reflects command-submission latency rather than a full UI arrival latency.

### Runner multi-open result

- Temporary runner profile: `target\p6d-runner-profiles\bluearchive.jp.runner.json`
- 16384:
  - run dir: `target\runner-runs\16384\runner-bluearchive-jp-smoke-1781878855427`
  - result: `blocked`
  - message: `page_guard_not_matched`
  - executed: false
  - click count: 0
- 16416:
  - run dir: `target\runner-runs\16416\runner-bluearchive-jp-smoke-1781878855427`
  - result: `blocked`
  - message: `page_guard_not_matched`
  - executed: false
  - click count: 0
- 16448:
  - run dir: `target\runner-runs\16448\runner-bluearchive-jp-smoke-1781878855544`
  - result: `completed`
  - executed: true
  - click count: 2
  - final page: `bluearchive/home`

The three runner processes used independent run roots and did not share mutable state.

### Commands run

- Read `C:\合作工作区\ActingCommand\TASK-P6d-live-validation-and-resource-closeout.md`.
- `git status --short --branch`
- `git rev-parse HEAD`
- Copied MaaTouch from local upstream source into `external-tools/maatouch/maatouch`.
- `cargo test -p actingcommand-task-loop`
- `cargo test -p actingcommand-device-test`
- `cargo test -p actingcommand-task-loop -p actingcommand-device-test`
- `cargo build --release -p actingcommand-device-test`
- Resource repository `git fetch origin` and `git pull --ff-only` for AzurLane, Arknights, and BlueArchive resource repos.
- Resource JSON scan for pack counts, page counts, navigation counts, and destructive marker counts.
- `target\release\actingcommand-device-test.exe --port 16384 benchmark --rounds 15`
- `target\release\actingcommand-device-test.exe --port 16416 benchmark --rounds 15`
- `target\release\actingcommand-device-test.exe --port 16448 benchmark --rounds 15`
- `target\release\actingcommand-device-test.exe --port 16384 detect-page ... --page azurlane/main_white --capture`
- `target\release\actingcommand-device-test.exe --port 16416 detect-page ... --page arknights/home --capture`
- `target\release\actingcommand-device-test.exe --port 16448 detect-page ... --page bluearchive/home --capture`
- `target\release\actingcommand-device-test.exe --port 16448 tap 640 360`
- `target\release\actingcommand-device-test.exe --port 16448 probe-run ... --capture --checkpoint-frames 8`
- Three parallel `target\release\actingcommand-device-test.exe --port <port> runner --profile target\p6d-runner-profiles\bluearchive.jp.runner.json --run-root target\runner-runs\<port> --capture` runs for ports 16384, 16416, and 16448.
- `cargo fmt --all`
- `cargo test --workspace`
- `cargo fmt --all -- --check`
- `cargo clippy -p actingcommand-task-loop -p actingcommand-device-test -- -D warnings`
- `cargo tree -p actingcommand-task-loop --depth 1`
- `git diff --check`
- Prohibited-feature scans over `apps/device-test`, `apps/device-test/src/probe_run.rs`, `crates/task-loop`, and `crates/task-loop/Cargo.toml`.

### Test results

- `cargo test -p actingcommand-task-loop` passed with 35 tests.
- `cargo test -p actingcommand-device-test` passed with 53 tests.
- `cargo test --workspace` passed.
- `cargo fmt --all -- --check` passed.
- `cargo clippy -p actingcommand-task-loop -p actingcommand-device-test -- -D warnings` passed.
- `git diff --check` passed.
- `cargo tree -p actingcommand-task-loop --depth 1` direct dependencies:
  - `actingcommand-page-detector`
  - `actingcommand-recognition`
  - `actingcommand-recognition-pack`
  - `serde`
  - `serde_json`
- Broad prohibited-feature scan only matched pre-existing `device-test` input subcommands `long_tap` and `swipe`.
- Probe lane and task-loop prohibited-feature scan had no matches for fallback, reconnect, retry loop, SQLite, OCR, OpenCV, ADB input, `long_tap`, or `swipe`.
- `crates/task-loop/Cargo.toml` scan had no direct dependency on SQLite/OpenCV/image/imageproc/device/runtime-core.
- `crates/device` has no `println!` or `eprintln!`.

### Current blocker

- AzurLane live route blocked because the current 16384 screen is not `azurlane/main_white`.
- Arknights has home detection but lacks enough reviewed operator/sanity probe resources for live mutation.
- BlueArchive only has verified live `NavigationOnly` coverage for home-to-task-and-back; FreeClaim/AP-consuming paths remain blocked until reviewed resources and checkpoints are added.
- Full-frame template arrival matching is functional but slow on BA task arrival; future work should move BA arrival anchors into recognition-pack targets and optimize recognition regions.

### Next step

1. Commit and push P6d Runtime changes.
2. Add regression frames for BA home/task positive and negative cases.
3. Expand AzurLane and Arknights resource packs with reviewed observe targets before live mutation.
4. Add explicit reviewed/resume flow before enabling FreeClaim or regenerating-resource consumption.

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

## 2026-06-19 P6b/P6c/P6d non-destructive probe loop

### Current status

- Implemented a P6b/P6c/P6d non-destructive probe lane.
- `actingcommand-task-loop` now has a pure probe module for `ProbePlan` schema v0.1.
- The task-loop probe core only parses, validates, and decides probe steps; it does not access devices, generate actual click points, write journals, start MaaTouch, or perform captures.
- `device-test probe-run` owns executable probe behavior, including ScreencapBackend capture, MaaTouchBackend taps after safety checks, actual click-point generation, operation journal files, and arrival polling.
- No MaaTouch binary was committed.
- `external-tools/maatouch/maatouch` is still absent locally, so any probe that reaches a click step will require `--local <path>` or a local-only external tool before it can tap.
- A safe BA JP probe smoke run on port `16384` completed with `executed=false` and `click_count=0` because the captured frame did not match `bluearchive/home`; no MaaTouch session was started and no click was sent.

### Files changed

- `Cargo.lock`
- `apps/device-test/Cargo.toml`
- `apps/device-test/src/main.rs`
- `apps/device-test/src/probe_run.rs`
- `apps/device-test/tests/fixtures/bluearchive.jp.probe.json`
- `crates/task-loop/src/lib.rs`
- `crates/task-loop/src/probe.rs`
- `PLANS.md`
- `CHECKPOINT.md`

### Probe implementation notes

- Probe actions:
  - `detect_page`
  - `observe_page`
  - `observe_targets`
  - `click` with `effect = navigation_only`
- Click steps require `expect_after.page_id`.
- Probe plans are capped at 10 steps.
- Runtime probe invocations are capped at 3 navigation clicks.
- Dangerous click names are rejected for click targets and click steps.
- Observe targets may contain words such as reward or collect because observe actions do not click.
- External references let `device-test` provide navigation-data click rects and temporary arrival-anchor pages without adding BA-specific direct matching to the task-loop core.

### Journal behavior

- Each `probe-run` writes:
  - `command.txt`
  - `probe-plan.json`
  - `input-paths.json`
  - `events.jsonl`
  - `summary.json`
  - `frames/`
  - `observations/`
- `actual_click_point` records:
  - seed
  - algorithm
  - rect
  - point
- Failure paths write `run_failed`; completed paths write `run_finished`.
- Post-click arrival validation uses polling rather than a single delayed frame.

### BlueArchive navigation bridge

- Read-only navigation file:
  - `C:\Users\Alice\Documents\Azur\ActingCommand-Resources-BlueArchive\navigation\bluearchive.jp.navigation.json`
- Resource repository commit:
  - `aaac863`
- `navigation/home_to_task` is mapped to a small randomizable click rect around `[66, 237]`.
- `control/home` is mapped to a small randomizable click rect around `[1236, 25]`.
- `navigation/home_to_task/arrive_anchor` is mapped to the full-frame `PAGE_TASK_CENTER.png` template.
- This arrival-anchor direct match is temporary in `device-test` only and should later become a recognition-pack full-frame target.
- `forbidden_destructive_points` are checked by rect or radius, not exact equality.

### Resource gap scan

- AzurLane:
  - repository: `C:\Users\Alice\Documents\Azur\ActingCommand-Resources-AzurLane`
  - commit: `8503ca1`
  - pages: 1
  - page: `azurlane/main_white`
  - targets: 2005
  - GOTO/MISSION/COMMISSION-like targets: 166
  - blocker: mission/commission and other destination page definitions are missing from pages JSON; navigation probe needs resource work first.
- Arknights:
  - repository: `C:\Users\Alice\Documents\Azur\ActingCommand-Resources-Arknights`
  - commit: `00199ee`
  - pages: 1
  - page: `arknights/home`
  - targets: 2
  - operator/menu-like targets: 0
  - blocker: operator/menu navigation targets are missing from pack/page data; probe needs resource work first.
- BlueArchive:
  - repository: `C:\Users\Alice\Documents\Azur\ActingCommand-Resources-BlueArchive`
  - commit: `aaac863`
  - pages: 1
  - page: `bluearchive/home`
  - targets: 1
  - navigation file: `bluearchive.jp.navigation.json`
  - executable now: structurally yes, but a real click requires the live game to be on BA home and MaaTouch binary to be supplied locally.

### Commands run

- Read task file: `C:\合作工作区\ActingCommand\TASK-P6b-P6c-P6d-probe-loop.md`
- Read Runtime-local `AGENTS.md`, `PLANS.md`, and `CHECKPOINT.md`.
- Checked Runtime repository status.
- Read current task-loop, device-test, page-detector, recognition-pack, device capture, and MaaTouch code.
- Read BlueArchive navigation, pack, and pages JSON.
- Scanned AzurLane, Arknights, and BlueArchive resource repositories read-only.
- `adb devices -l`
- `cargo fmt --all`
- `cargo test -p actingcommand-task-loop`
- `cargo test -p actingcommand-device-test`
- `cargo test -p actingcommand-task-loop -p actingcommand-device-test`
- `cargo clippy -p actingcommand-task-loop -p actingcommand-device-test -- -D warnings`
- `cargo test --workspace`
- `cargo fmt --all -- --check`
- `cargo tree -p actingcommand-task-loop --depth 1`
- `git diff --check`
- `rg -n "rusqlite|sqlite|SQLite|OCR|ocr|OpenCV|opencv|scheduler|background loop|retry loop|adb shell input|input tap|long_tap\(|swipe\(" apps/device-test crates/task-loop`
- `rg -n "rusqlite|sqlite|opencv|image\s*=|imageproc\s*=|actingcommand-device|actingcommand-runtime-core" crates/task-loop/Cargo.toml`
- `rg -n "adb shell input|input tap|fallback|reconnect|retry loop|background loop|rusqlite|SQLite|sqlite|OCR|ocr|OpenCV|opencv" apps/device-test/src/probe_run.rs crates/task-loop/src/probe.rs apps/device-test/tests/fixtures/bluearchive.jp.probe.json`
- `rg -n "long_tap\(|swipe\(" apps/device-test/src/probe_run.rs crates/task-loop/src/probe.rs`
- `cargo run -p actingcommand-device-test -- --port 16384 probe-run --pack C:\Users\Alice\Documents\Azur\ActingCommand-Resources-BlueArchive\recognition\bluearchive.jp.pack.json --pack-root C:\Users\Alice\Documents\Azur\ActingCommand-Resources-BlueArchive --pages C:\Users\Alice\Documents\Azur\ActingCommand-Resources-BlueArchive\recognition\bluearchive.jp.pages.json --probe apps\device-test\tests\fixtures\bluearchive.jp.probe.json --run-root C:\Users\Alice\Documents\Azur\ActingCommand-Runtime\target\probe-runs --navigation C:\Users\Alice\Documents\Azur\ActingCommand-Resources-BlueArchive\navigation\bluearchive.jp.navigation.json --capture`

### Probe smoke result

- Run id: `probe-1781872119434`
- Run directory: `C:\Users\Alice\Documents\Azur\ActingCommand-Runtime\target\probe-runs\probe-1781872119434`
- Result: `completed`
- `executed=false`
- `click_count=0`
- Captured frame size: `1280x720`
- First capture elapsed time: `695 ms`
- Reason no click occurred:
  - `bluearchive/home` page guard failed with `required target failed: page/home`
  - the follow-up `return_home` step was also skipped because the external arrival page guard was not known
- MaaTouch was not started in this smoke run.

### Test results

- `cargo test -p actingcommand-task-loop` passed with 31 tests.
- `cargo test -p actingcommand-device-test` passed with 47 tests.
- `cargo test -p actingcommand-task-loop -p actingcommand-device-test` passed.
- `cargo test --workspace` passed.
- `cargo fmt --all -- --check` passed.
- `cargo clippy -p actingcommand-task-loop -p actingcommand-device-test -- -D warnings` passed.
- `cargo tree -p actingcommand-task-loop --depth 1` showed direct dependencies only on `actingcommand-page-detector`, `actingcommand-recognition`, `actingcommand-recognition-pack`, `serde`, and `serde_json`.
- `git diff --check` passed.
- The task-loop Cargo.toml direct-dependency scan found no direct `image`, `imageproc`, OpenCV, SQLite, `actingcommand-device`, or `actingcommand-runtime-core` dependency.
- The new probe-code prohibited-feature scan found no ADB input fallback, reconnect, retry loop, background loop, SQLite, OCR, OpenCV, `long_tap`, or `swipe`.
- The broader task-specified scan over `apps/device-test` and `crates/task-loop` only matched pre-existing `device-test` `long_tap` and `swipe` input commands in `apps/device-test/src/main.rs`; the new probe lane does not call them.

### Current blocker

- `external-tools/maatouch/maatouch` is not present and must not be committed; pass `--local <path>` or place it in an ignored local-only external-tools path before real click execution.
- BA live click verification requires the target emulator to be on the BA JP home page.
- AzurLane and Arknights probes are blocked by missing resource page/target definitions.

### Next step

1. Run final workspace tests, formatting check, clippy, cargo tree, prohibited-feature scans, and `git diff --check`.
2. Commit and push Runtime repository changes.
3. After MaaTouch is supplied locally and BA is on the home screen, rerun BA JP `probe-run` for the real navigation click path.
4. Do not start P6e destructive operations without separate user confirmation.

## 2026-06-24 BA Resource Control Refinement Base

### Current status

- Read `C:\合作工作区\ActingCommand\TASK-resource-BA-control-refinement-and-progression.md`.
- Implemented the Runtime/resource compatibility needed before BA live control-data refinement:
  - recognition `match_metric` support with CCORR default and CCOEFF_NORMED opt-in.
  - recognition-pack support for generated `0.3` packs, target-level thresholds, and `"full_frame"` template regions.
  - page-detector support for generated `0.3` pages.
  - probe-run navigation drag execution via MaaTouch swipe, including actual from/to/duration journal data.
  - probe-run initial/final and last before/after page summary fields.
  - conservative standby wake tap when no page is detected and navigation provides a `wake` control point.
- Updated the BA resource converter and bundle defaults so generated BA packs set `match_metric: "ccoeff_normed"`.
- Regenerated `recognition/bluearchive.jp.pack.json` in `ActingCommand-Resources-BlueArchive`.

### Files changed

- Runtime:
  - `crates/recognition/src/lib.rs`
  - `crates/recognition-pack/src/lib.rs`
  - `crates/page-detector/src/lib.rs`
  - `apps/device-test/src/probe_run.rs`
- BlueArchive resource repo:
  - `tools/convert_operations.py`
  - `operations/SCHEMA.md`
  - all 20 `operations/*/task.json` bundle defaults
  - `recognition/bluearchive.jp.pack.json`

### Commands run

- `cargo fmt`
- `cargo test -p actingcommand-recognition -p actingcommand-recognition-pack -p actingcommand-page-detector -p actingcommand-task-loop -p actingcommand-device-test`
- `cargo run -q -p actingcommand-device-test -- recognize --pack C:\Users\Alice\Documents\Azur\ActingCommand-Resources-BlueArchive\recognition\bluearchive.jp.pack.json --pack-root C:\Users\Alice\Documents\Azur\ActingCommand-Resources-BlueArchive --check-pack`
- `cargo run -q -p actingcommand-device-test -- detect-page --pack C:\Users\Alice\Documents\Azur\ActingCommand-Resources-BlueArchive\recognition\bluearchive.jp.pack.json --pack-root C:\Users\Alice\Documents\Azur\ActingCommand-Resources-BlueArchive --pages C:\Users\Alice\Documents\Azur\ActingCommand-Resources-BlueArchive\recognition\bluearchive.jp.pages.json --check-pages`
- `python tools\convert_operations.py --root . --game bluearchive --server jp --locale ja-JP`
- `python -m py_compile tools\convert_operations.py`

### Test results

- Runtime focused tests passed.
- BA generated recognition pack check passed.
- BA generated pages check passed.
- BA operation converter completed with 20 bundles, 22 targets, 20 pages, 19 edges, 23 page operations, and 53 primitives.

### Current blocker

- The task file's BA data acceptance items are not complete yet:
  - full-frame anchors still need live CCOEFF ROI replacement.
  - 8 sentinel coordinates still need live resolution.
  - cafe reward collect and growth/progression bundles still need live data authoring and verification.
- Live BA ADB/device validation was not run in this checkpoint.

### Next step

1. Use the BA emulator and task-specified Python/OpenCV/ADB environment to capture live pages and replace full-frame anchors with tight CCOEFF ROIs.
2. Resolve the 8 sentinel coordinates and regenerate artifacts.
3. Add cafe collect and growth/progression operation bundles only after live evidence supports the data.
