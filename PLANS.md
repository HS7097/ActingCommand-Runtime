# PLANS.md

## Repository goal

`ActingCommand-Runtime` is the Rust mainline runtime repository for ActingCommand.

The runtime owns device/control primitives, capture primitives, recognition primitives, and later runtime orchestration components behind explicit interfaces.

## Current Runtime ledger and removable-Lab correction chain

The active approved architecture target is GitHub Issue #35:

- `[架构纠偏][P0] 全局事件账本核心与可移除 Lab 调试管道`
- Technical source of truth: `TASK-runtime-ledger-core-and-optional-lab-correction-v3.md`
- Specification SHA-256: `28273b85491b0d43aa7a7b7a7ece10db681de9df4d9100e85f9e9b086dd107a6`
- Approved resource-authoring amendment: `AMENDMENT-Issue35-Lab-resource-authoring-ownership.md`
- Amendment SHA-256: `de039ad910b0b8208d52b8582b260868d4bcd2b04ec2b272977890661a136b54`
- Accepted implementation baseline: `981f61f650c51a62f3c6c22fda781d2b98b3ceb8`
- Historical paused RED evidence: `ead23d2acb3752507b5c6110c1cbf049e878cbbd`
- Implementation branch: `issue-35-runtime-ledger-v3`
- Approved C1 hardening design: `docs/superpowers/specs/2026-07-11-c1-ledger-hardening-design.md`
- C1 hardening implementation plan: `docs/superpowers/plans/2026-07-11-c1-ledger-hardening.md`

Issue #35 supersedes the goal that made Lab the application core. Lab is now an optional debug/sealed-test client. The long-lived Runtime host and scheduler own production state and device authority, and the global ledger is the shared production fact source.

The approved chain is:

- seed critical path: `C0 -> C1 -> C3a -> C4`;
- C2 may proceed independently after C0;
- expansion path: `C3b -> C5 -> C6 -> C7`.

Current phase: C1 is complete at `checkpoint/20260711-c1-ledger-hardening`, C3a is complete at `checkpoint/20260711-c3a-runtime-seed`, C4 is complete at `checkpoint/20260711-c4-minimal-vertical-slice`, C2 is complete at `checkpoint/20260711-c2-artifact-evidence`, and C3b is complete at `checkpoint/20260711-c3b-resident-control-plane`. C5 is active under `docs/plans/2026-07-11-c5-production-capability-relocation.md`. Task 1 is complete: `docs/architecture/c5-ownership-inventory.md` freezes current state/side-effect owners, destinations, removal gates, the 30-case protocol matrix, and migration evidence; architecture tests now reject direct or transitive production reachability to future resource-tooling. Task 2 is active and will mechanically extract package/convert/MAA behavior into Lab-owned developer-only resource-tooling while preserving envelopes and resource semantics. Later C5 tasks absorb task-loop decisions into execution ownership, move env/recognition/drive/run/recovery into production execution modules, keep frame/output authority in artifact-store and ledger, move Session/monitor/stream live state into Runtime host, and reduce Lab to optional authoring/debug/sealed adapters. C6 retains full authoring workflow and transactional publication. Alice approved the complete Issue #35 chain and clarified that C0-C7 nodes do not require separate approval. The approved C0 architecture is `docs/architecture/runtime-ledger-v3-c0-freeze.md`, payload SHA-256 `6c72a9c39ff67ec5a2868e0ed262d2a2f0a2c4b0fbfc473b7c55a9df610bf0a7`.

Issue #36 is the only execution-tracking Issue for this chain. Stage progress, implementation details, verification evidence, commits, and rollback anchors are recorded there; no additional subtask Issues are created.

The approved resource-authoring amendment changes only the internal C5/C6 ownership split:

- production Runtime consumes immutable packages through `pack-containment` and does not record, author, convert, build, or publish resources;
- Lab is the formal user-facing resource-authoring workflow;
- future `actingcommand-resource-tooling` is a Lab-owned developer-only deep module for deterministic draft, convert, package, validation, and transactional publish operations;
- production crates must not directly or transitively depend on `actingcommand-resource-tooling`;
- `pack-containment` remains shared by authoring validation and production `LoadedBundle` consumption;
- explicit resource publication is transactional, ledger-recorded, rollback-safe, and never performs automatic Git commit or push;
- Issue #26 G2/G3 remain consumption-side Runtime/containment responsibilities.

This amendment does not change C1, C3a, C4, C2, C3b, the task order, or their acceptance semantics.

C0 decisions include:

- branch from the accepted A7 baseline without cherry-picking the paused A8a RED commit;
- preserve A0/A1 and reusable A2b-A7 behavior while correcting ownership;
- add production modules for runtime host, runtime client, scheduler, execution kernel, artifact store, and resource tooling in their scheduled phases;
- retain and deepen the existing contract, ledger, device, recognition, and containment crates;
- prohibit every non-Lab workspace package from directly or transitively depending on `actingcommand-lab`;
- rewrite A8a requirements as global-ledger writer and daemon ownership/fencing tests instead of implementing per-command state-file locking;
- treat the approved frozen C0 payload as the binding C1-C7 architecture.

Current implementation: C1 is closed with typed event/query/subscription/projection contracts, schema-owned pre-persistence redaction, opaque ledger-owned facts, recoverable single-writer append storage, correlated ordered indexes, bounded gap-free replay plus live delivery, absorbing terminal subscriptions, separate CLI/UI/Lab projections, validated intent-durable-before-act and outcome-after-act ordering, hard-kill recovery proof, five-source correlation, complete secret-class redaction coverage, and all-features dependency-law guards. Artifact-bearing recovery remains intentionally fail-closed until C2 provides the durable artifact authority. C1 guarantees one action invocation per call after durable intent; it does not provide cross-call exactly-once, retries, action-id deduplication, reconciliation, or compensation. C3a owns request idempotency, `owner_epoch` fencing, pending-intent reconciliation, lease lifecycle, and production recovery decisions.

## Historical CLI-to-Lab extraction chain (superseded)

GitHub Issue #33/#34 is retained as implementation and audit history. Its original architecture target is no longer active:

- `[架构] CLI=管道 · lab=应用核 抽层任务链(A0–A9)`
- Frozen technical source of truth: `TASK-lab-extraction-chain.md`
- Frozen specification SHA-256: `efb9e37f10807ce2a615205e3924021ad91eb073a54e4c65cd178e14b0aeab3b`
- Runtime baseline: `6941b9a046da182185b1a64745f77651e0aec5f0`

The amended chain is strictly linear:

`A0 -> A1 -> A2a -> A2b -> A3 -> A4 -> A5 -> A6 -> A7 -> A8a -> A8b -> A8c -> A9`

Current tracking surface: GitHub issue #34 on branch `issue-34-lab-extraction-chain`.

A0 delivers:

- structured dependency-law guards for S1 G-a through G-e;
- a checked-in `apps/actinglab/src/main.rs` line ratchet;
- a source-derived command inventory and checked-in denominator snapshot;
- no Runtime behavior changes and no A1 golden-test work.

A0 implementation is complete and verified on its task branch:

- S1 guards cover G-a through G-e, including AST inspection for public `serde_json::Value` signatures and Cargo metadata dependency direction;
- the exact `main.rs` ratchet baseline is `60161` lines;
- the checked-in denominator is `44` top-level dispatch arms, expanded to `119` concrete commands;
- deliberate line growth and stale command-count mutations both failed before restoration;
- focused tests, formatting, diff checks, workspace Clippy, and workspace tests pass.

Alice previously authorized the A0-A9 chain. Issue #35 now suspends that chain after the accepted A7 checkpoint; its completed commits remain provenance and migration assets.

Historical stop point: A7 is implemented and verified. The old A8a/A8b/A8c/A9 targets are cancelled, and the paused A8a RED evidence is not part of this branch.

Approved amendments:

- A7 interface amendment: `docs/architecture/actinglab-a7-interface-amendment.md`, frozen payload SHA-256 `eb753f19c03cd71bafdc50ac9847800c070713b148b07fccd7f9b335be7264e0`.
- Chain amendment: `docs/architecture/actinglab-chain-amendment-20260710.md`, frozen payload SHA-256 `09ac0a1e8c891f54eeaccd3e0aae3f59851621df61d9e9c53bbec97907d54fe6`.
- A8a now owns both Lab2 arbitrator locking/recovery and same-session runtime-ledger writer conflict detection, followed by golden review/re-freeze.
- A8b remains the pure Lab2 migration.
- New A8c migrates all remaining use cases needed to reach the original A9 terminal gates.
- A9 remains unchanged: `main.rs <= 6000`, at least 95 percent of the 44 frozen dispatch arms satisfy S3, and the full invariant/guard/golden/retirement audit must pass.

A1 delivers:

- a static 30-case protocol baseline covering 15 required command families with success and failure paths;
- sealed synthetic resource, scene, package, app-state, Session-state, and fake-ADB fixtures;
- canonical JSON comparison, exact process exit-code comparison, and stdout/stderr protocol-channel assertions;
- deterministic normalization for generated IDs, timestamps, absolute paths, env instance IDs, lease/process facts, retention byte counts, and the generated Lab output artifact hash;
- a checked-in CLI exit-code table, baseline review record, and explicit recorder script that is never used during normal comparison;
- deliberate baseline tampering evidence: changing a retained `cli_version` field makes the golden suite fail, and restoration returns it to green.

A2a interface freeze:

- document: `docs/architecture/actinglab-a2a-interface-freeze.md`;
- SHA-256: `de50cfbb8906549662017efed4e39e0ac6f2375c77024772f5d3ca489c062c57`;
- promote the existing `actingcommand-contract` in place rather than creating `lab-contract`;
- add `crates/lab` as the sole application core;
- reuse `InputBackend` and `CaptureBackend` through factories instead of duplicating device/capture operation traits;
- add only the missing ledger, clock, and config ports in Lab;
- keep semantic errors in contract and the process exit-code table in the CLI adapter;
- expose typed Lab use-case request/response methods and claim I5a state ownership only.

A2b implementation:

- promotes `actingcommand-contract` with the frozen generic envelope, semantic error classes, environment, drive, lease, arbitration, and ledger-projection DTOs;
- adds `crates/lab` with the sole `Lab::new` construction path, explicit arbitrator/environment/session state domains, semantic request context, and typed projection facility;
- freezes the Lab effect boundary as input/capture factories over existing device traits plus ledger, clock, and config ports;
- routes ActingLab envelope construction, semantic errors, semantic ledger context, and receipt projection through the contract/Lab crates while leaving process exit-code mapping in the CLI adapter;
- preserves every A1 golden protocol case and lowers the `main.rs` ratchet from `60161` to `59967` lines;
- adds no runtime behavior, schema-version, scheduler, resident-process, UI, device, or game-logic change.

A3 environment-detection migration:

- moves the environment catalog, detector evaluation, result persistence/locking, freshness checks, marker resolution, and all 20 established tests into `crates/lab`;
- exposes typed `detect_env`, `resolve_env`, `env_status`, and generic typed marker-resolution APIs without public `serde_json::Value` signatures;
- uses the contract `EnvDetected`, `EnvResolved`, and `NeedsDetection` DTOs for load-bearing facts;
- executes capture and touch effects only through the frozen Lab factories, with injected clock and state roots and no behavioral environment reads in Lab;
- leaves a thin app adapter for flags, production port construction, semantic-ledger projection, serialization, and process behavior;
- preserves all A1 protocol goldens and existing Runtime behavior while keeping all three dispatch arms below the S3 limit;
- keeps implementation and test files below the 2600-line module limit.

A4 read-only recognition/page migration:

- moves `recognize`, `detect-page`, `current-page`, and `is-visible` use cases into typed `crates/lab` APIs;
- shares the A3 scene-loading and env-marker resolution path instead of retaining a second implementation in the CLI;
- emits `NeedsDetection` and `EnvResolved` through the A3/contract DTOs, including the distinct `detect-page` and `current-page` command identities;
- preserves detect-page semantic-ledger records, request/reco ids, daemon routing, capture selection, output fields, exit codes, and all A1 goldens;
- leaves four one-line command functions in `main.rs` and a production-only app adapter for flag/config parsing, port construction, ledger projection, and response serialization;
- adds sealed Lab family tests for evaluatable recognition, click-only recognition, page detection/current page, and negative visibility;
- lowers the `main.rs` ratchet from `59967` to `59792` lines without changing Runtime behavior.

A5 touch-control migration:

- moves `tap-target` and `navigate` recognition, safety, route planning, touch execution, and arrival polling into typed `crates/lab` APIs;
- executes tap/drag operations exclusively through the A2a-frozen `InputBackendFactory` and preserves production backend diagnostics in typed responses;
- keeps Session-daemon routing, semantic request/reco/action ids, ledger drive records, destructive-action guards, env-resolution hints, real-execution capture requirements, and failure codes unchanged;
- moves the #29 absolute-coordinate translation helper into Lab without changing its established behavior, while leaving #30 retry/recovery behavior and call surfaces unchanged;
- leaves two one-line command functions in `main.rs` and a production adapter for flag/config parsing, deferred device/timing resolution, Lab port construction, and serialization;
- adds sealed Lab tests for dry-run planning, failed recognition, navigation routing, real input-port execution, and coordinate derivation;
- lowers the `main.rs` ratchet from `59792` to `59466` lines;
- validates one real safe AK-device tap on `127.0.0.1:16416` after mirroring the Arknights resource repository, with no game-progress or paid-resource action.

A6 package/conversion migration:

- moves `package build-task`, `package validate`, and `resource convert` into typed `crates/lab` use-case APIs while retaining flag parsing, output serialization, process exit mapping, and package ledger-event attachment in the app;
- keeps adjacent `package build-pack` and `resource compile-maa` orchestration app-owned, with only the shared typed package/converter machinery located in Lab;
- restores the complete pre-move generated-package validator, including control, manifest, operation, guard, coordinate, asset, recognition-pack, page, and navigation validation;
- injects temporary-root discovery from the app and extends architecture guards so Lab cannot read behavioral process environment state;
- moves package/converter/MAA graph tests with their implementation and retains app-level full/split `build-pack` command coverage;
- records the intentionally unresolved Issue #26 G2/G3 seams in `docs/architecture/actinglab-a6-issue26-handoff.md` without changing those semantics;
- preserves all 30 protocol goldens, keeps every Lab source file below 2600 lines, and lowers the exact `main.rs` ratchet from `59466` to `59185` lines.

A7 Lab run/validate migration:

- moves `lab run` and `lab validate` into typed `crates/lab` APIs while the app retains flags, process context, concrete device/config adapters, serialization, and exit mapping;
- keeps containment before control/resource parsing and records the unchanged Issue #26 G2/G3 insertion points in `docs/architecture/actinglab-a7-issue26-splice.md`;
- moves safe frame-store, run lifecycle, retry/recovery, ledger projection, archive, and cleanup logic into Lab while the Windows memory sampler remains app-owned;
- keeps exactly one `LedgerSink`, uses Lab-owned opaque ledger DTOs at the public boundary, and keeps concrete `LabLedger` ownership in the app adapter;
- validates the complete selected device configuration before context assignment, normal ledger creation, and lease acquisition, while capture opening stays post-lease and touch opening stays first-input only;
- preserves selected-only resolution, archive/ledger ordering, wire fields, exits, all 30 goldens, the `59185` line ratchet, and every source/dependency guard;
- leaves Issue #26 G2/G3 behavior unchanged and adds no resource, device, scheduler, UI, SQLite, OCR, or game-logic change.

## Current environment detection memory and env pointer node

The active approved Runtime work is GitHub issue #31:

- `[新功能] 环境检测任务 + 每实例检测内存/落盘 + {env:} 指针解析(通用运行时状态探测,首用例=AK 16 主题)`
- Technical source of truth: `C:\合作工作区\ActingCommand\TASK-detection-task-and-detected-memory.md`

Implemented direction:

- Add a generic `detect` command for environment detection tasks authored in resource data under `env-detection/detections.json`.
- Keep Runtime logic game-agnostic: detector ids, keys, candidates, thresholds, `allowed_values`, and TTL are data-defined.
- Accept both the internal detector/key catalog shape and the resource-authored flat `env-detections.v1` shape used by the current Arknights resource repository.
- Persist per-instance detection results to `env-detection/<instance_id>/result.json` below the resolved resource root.
- Derive `instance_id` from a salted hash of the normalized instance identity; result files do not include raw ADB endpoints by default.
- Store required result provenance: schema version, instance id, game/server, detector id/version, resource pack id/hash, generated time, and per-key detection facts.
- Validate env values against `allowed_values` and path-safety rules before writing or resolving them.
- Treat missing, stale, expired, low-confidence, schema-mismatched, detector-mismatched, or resource-hash-mismatched env results as visible failures instead of defaulting.
- Concurrent env result writes are covered by a focused test proving the lock/atomic-write path leaves a readable `result.json` and no stale lock file even when one writer hits a lock conflict.
- Add `env resolve` and `env status` commands for `{env:<key>}` pointer resolution and freshness inspection.
- `env status` and `env resolve` now surface machine-readable `needs_detection` diagnostics for missing or stale env results, including reason, detector id/version, instance id, result path, stale detection summary when present, and `recommended_action=run_detect`.
- Stored env result values that are unsafe for paths or outside detector `allowed_values` are covered by focused stale-resolution tests and must fail visibly instead of resolving to a resource path.
- Allow detection tasks to declare generic pre-recognition interaction steps (`tap`, `long_tap`, `swipe`, and `wait`) in resource data; Runtime executes those steps through the existing touch backend only when the detector is run with `--capture`.
- Allow detection candidates to be template-based or scene-size based, so resource data can declare non-template environment facts such as resolution without adding game-specific Runtime logic.
- Resolve `{env:<key>}` markers during runtime recognition-pack loading so normal `recognize`, `detect-page`, `is-visible`, `tap-target`, and semantic navigation paths consume the detected per-instance value instead of requiring a separate manual path rewrite.
- Resolve `{env:<key>}` markers during selected `package build-task` pack/operation assembly when an instance result exists, so instance-specific dry-run packages validate with concrete assets.
- Surface `needs_detection` hints when env-backed recognition or page detection fails, and record `env_needs_detection` drive records for semantic control/read paths that can feed #30 recovery or future scheduler redetection without Runtime making a scheduling decision itself.
- Support `target` / `target_center` navigation clicks that resolve at execution time through a recognition target match, with destructive-overlap checks applied to the resolved concrete click point before device input.
- Record `env_detected` and `env_resolved` stages through the existing semantic ledger path when a run root is configured.
- Surface implicit `{env:<key>}` consumption in normal semantic command and Lab2 outputs through `env_resolved`, and record `env_resolved` drive records when those commands write a runtime ledger.
- Use atomic JSON publication plus a per-result lock file for concurrent detection writes.

Current boundary:

- This node implements the generic Runtime P1 mechanism only.
- Scheduler auto-triggering, SwitchTheme recovery, OCR, SQLite, UI, and game-specific logic remain out of scope.
- Interactive detection steps are implemented as generic data-defined pre-recognition steps; no game-specific step logic is embedded in Runtime.
- The current mirrored Arknights resource repository now contains `ours/env-detection` and `ours/hometheme`.
- The current mirrored BlueArchive and AzurLane resource repositories now contain server-neutral `ours/env-detection/detect_resolution` catalogs that use resource-authored scene-size candidates.
- Runtime validation covered the current Arknights `detect_ui_theme` catalog, offline synthetic-scene detection, live read-only AK capture detection on `127.0.0.1:16416`, `env status`, `{env:ui_theme}` resolution to an existing theme resource path, direct `recognize` loading of an env-backed target, fail-loud missing-env loading, `package build-task --dry-run` for the AK env-backed home routes, and a `detect-page --run-root ... --capture` smoke that wrote `env_resolved` into the runtime ledger.
- Runtime validation now also covers missing-result and stale-result `env status`/`env resolve` diagnostics through unit tests and a CLI smoke using a synthetic temporary resource root under `target`, stored unsafe/unlisted env result values through focused tests, plus concurrent result-write safety through a focused test.
- Resource repositories were mirrored before this resource-dependent task. The Arknights resource repository required a small catalog correction for Dreamland/LoneTrail Day/Night template paths; Runtime remains game-agnostic.
- Current HEAD has an issue #31 readiness audit recorded in `CHECKPOINT.md`: Runtime and all three resource repositories are aligned with `origin/main`, the local thick-spec acceptance items have direct evidence, and the remaining scheduler-triggered redetection and SwitchTheme fallback work is intentionally future-scope.

## Current guarded-click and retry/recovery execution node

The active user-approved Runtime work covers GitHub issues #29 and #30:

- #29 `[BUG][严重] 带守卫的绝对点击被漂移平移到越界/误点`
- #30 `[新功能] 执行层重试+恢复策略(MAA 标准 + ALAS 补充)`
- Technical source of truth:
  - `C:\合作工作区\ActingCommand\FIX-runtime-guarded-absolute-click-drift-oob.md`
  - `C:\合作工作区\ActingCommand\TASK-exec-retry-recovery-MAA-ALAS.md`
  - `C:\合作工作区\ActingCommand\FIX-30-navigation-retry-inert-by-default.md`

Implemented direction:

- Absolute coordinate clicks (`rect`, `specific_rect`, `point`, `long_press`/`long_tap`, and `drag` from/to) remain guarded but no longer shift by `matched_rect - expected_rect`.
- `offset` remains relative to the matched template rectangle.
- Absolute coordinate guards may be template or color-probe guards; only `offset` and recognition-target clicks require a template guard because they need a matched rectangle.
- Operation bundle schema `0.6` accepts bounded execution flow fields for operation timeout, max attempts, retry interval, pre/post delay, pre/post wait-freezes, retryability, effect, and `on_error`.
- Task-level `error_pages`, `recovery`, `max_task_retries`, and `on_exhausted` are parsed and validated.
- Navigation-only or explicitly retryable operations use bounded L0/L1 retry.
- Retry, recovery, task retry exhaustion, and `paused_needs_human` states emit ledger/light-event records.
- `return_home` recovery is loaded as a contained operation bundle and is bounded; recovery failure or exhausted task retry fails loud with `paused_needs_human`.
- Passed forbidden targets that represent explicit `error_pages` or `negative_*` targets are treated as error-page signals even when the page detector intentionally returns no matched page.
- Selected task packages prune non-resident `page_rules` pages and soft optional/forbidden targets while keeping full-pack validation strict.
- Runtime candidate-page discovery includes package-resident `page_rules` pages, allowing recovery bundles to recognize error/current pages that are not direct operation `from` pages.
- Runtime operation selection prefers page-specific operations before `from: any` fallback operations so universal recovery steps do not loop over generic actions.
- Existing schema `0.3` page-transition operations now default to navigation-only retry semantics structurally: a non-empty `to` page with empty `consumes` and `produces` is retryable even when `purpose` is `Navigate ...` and no schema `0.6` flow fields are present.
- Existing selected task packages that contain `operations/return_home/task.json` now treat that contained bundle as implicit recovery even when the entry task has no explicit `recovery` or operation-level `on_error`.
- `package build-task --include-recovery` keeps a broad recognition page/target context for recovery packages while leaving executable operation bundles selected to the entry task plus `return_home`; this lets contained `return_home` identify post-click pages outside the entry task.
- After at least one real retryable attempt, a retry-preparation `page_guard_mismatch` from an unknown or wrong page can enter the bounded recovery path instead of failing before recovery; initial guard mismatch, target mismatch, resource drift, invalid coordinates, and side-effect operations remain fail-loud.
- Recognition-target click modes (`target` and `target_center`) provide the Phase 3 explicit recognition-after-click path without changing absolute coordinate semantics; `target` uses the deterministic uniform point sampler, while `target_center` now clicks the matched rectangle center.
- The retry/recovery branch policy is factored and covered by regression tests for retry, recover, fail, error-page recovery, exhausted attempts, and non-retryable side effects.

Current boundary:

- Deterministic failures such as package validation errors, guard mismatch, resource drift, invalid coordinates, lease/security failures, and missing recovery packages remain fail-loud and are not hidden behind fake success.
- Runtime implementation is covered by unit/workspace verification; live AK `home -> depot`, non-edge `home -> mission`, full-pack `return_home`, and forced `error_page -> return_home -> retry original task` were revalidated on `127.0.0.1:16416` with MaaTouch before the default-retry follow-up.
- The default-retry follow-up is covered by focused flow-policy tests, recovery-configuration tests, retry-pre-execution recovery tests, recovery recognition packaging tests, workspace gates, and a live AK `open_depot` rerun that reached `retry -> implicit return_home recovery -> whole task rerun -> paused_needs_human` instead of silent success or one-shot failure.
- Current AK `open_depot --include-recovery` recovery-package recognition no longer depends on a full-pack run to see the post-click recruit/gacha page class; the selected package now contains the broader recognition context required by `return_home`.
- Current AK `open_friends` / `open_operator` resource routes still need resource-route follow-up, but #29 now has separate non-edge live evidence through `open_mission`.
- No UI, SQLite, OCR, new capture backend, scheduler, game logic, resource-repository modification, or upstream source import is part of this node.

## Current runtime-ledger unified fact source chain

The active approved issue is Runtime issue #28:

- `[主任务] runtime-ledger 统一事实源:泛化现有 ledger,一切输出改投影(本轮基础可用)`
- Technical source of truth: `C:\合作工作区\ActingCommand\TASK-runtime-ledger-chain.md`
- L0 repair source: `C:\合作工作区\ActingCommand\FIX-runtime-ledger-record-before-act-1d9e181.md`
- Previous baseline `1d9e181` was revoked for the record-before-act blocker in `LabRunContext::finish`.

Current node:

- L0 `record-before-act` repair is implemented, verified, committed, tagged, and pushed.
- `output_zip_written` is no longer recorded before terminal output work.
- `finalizing` is recorded as a non-terminal `Drive` record for summary, diagnostics, and environment projection.
- `write_logs`, `write_output_zip`, output zip `sync_all`, and `file_sha256` now form the terminal artifact stage.
- `output_zip_written` with real SHA-256 and terminal `finish_ok`/`finish_error` receipts are recorded only after the terminal artifact stage succeeds.
- Terminal artifact failure writes last-resort error evidence and records `finish_error`; it must not leave `finish_ok` or a fake `output_zip_written`.
- `actingcommand-ledger` now exposes the lightweight `commit_then_record` / `CommitProof` guard used by this terminal recording path.
- L2 `lab run` completed-run projection is implemented locally: post-finish result output reads run id, status, terminal receipt type, output zip path, SHA-256, and ledger path from the runtime ledger terminal projection.
- Completed-run projection fails loudly when `finalizing` is missing or when terminal `finish_ok`/`finish_error` receipt is missing.
- L3 has started with the known `run_session_request_cancel` record-before-act repair: cancellation now publishes a cancellation response as the queue execution guard, removes the queued request, and only then appends the session journal entry.
- `session request cancel` dry-run output now declares that a real cancellation would write a response, remove the request, and record the journal.
- Cancellation journal append failure now fails visibly after the request has been removed and the cancellation response is available, preventing a durable "cancelled" journal from being the only fact while the request remains executable.
- L3 Session journal bridging has started: `append_session_request_journal` now writes a `session_request_receipt` into the shared runtime-ledger crate before writing the legacy request journal.
- `actingcommand-ledger` now has `LabLedger::open_or_create` so long-lived Session ledgers can append without duplicating headers.
- `session request-state get` now includes the ledger receipt, can report a ledger-only completed/failed state when the legacy journal is missing, and fails loudly if legacy journal and runtime-ledger statuses conflict.
- `session journal`, `session events`, and `session request-state list` now read a runtime-ledger-backed compatibility projection for completed/failed request facts.
- Ledger-only Session request receipts now appear in journal/events/list outputs without requiring the legacy journal file.
- The legacy Session request journal remains readable for compatibility, but if it conflicts with the runtime-ledger receipt for the same request id the projection fails loudly.
- Session request submit now records a `session_request_dispatch` ledger fact after publishing the request file; if the dispatch ledger append fails, the just-published request is removed or the cleanup failure is surfaced.
- Session request acknowledgement timeout now removes the unacknowledged request and records a failed `session_request_receipt` with `request_ack_timeout`.
- Synchronous daemon-routed request output now waits for the runtime-ledger receipt, rejects response/ledger status conflicts, and includes the ledger receipt in successful command output.
- `session response get|wait` now refuses to return a queued daemon response unless a matching runtime-ledger receipt exists, rejects response/ledger status conflicts, and includes the ledger receipt in response output.
- `session queue` now reports runtime-ledger consistency for pending/running requests and pending responses; missing dispatch or receipt facts visibly block queue admission with `runtime_ledger_inconsistent`.
- Daemon request processing no longer leaves a request executable when legacy journal compatibility output fails after the runtime-ledger receipt exists.
- L3 Session status projection convergence has started: `read_session_request_journal`, `request-state get`, cancellation not-found checks, journal entry counts, and `session status --diagnostics` now consume the runtime-ledger-backed projection by default instead of legacy-only request-journal facts.
- Session status diagnostics now expose `source=runtime_ledger_projection`, the session runtime-ledger path, and ledger projection counters so UI/scheduler clients can distinguish ledger-backed receipts from legacy compatibility entries.
- L4 direct Lab-1 semantic entrance bridging has started: direct `detect-page`, `tap-target`, and `navigate` calls now receive `req_id` values and write runtime-ledger dispatch, drive, and receipt records when `--run-root` or configured `run_root` is available.
- Direct Lab-1 `detect-page` records recognition `reco_id`; `tap-target` records recognition `reco_id` plus action `action_id`; `navigate` records recognition `reco_id` and route action ids.
- `lab receipt --req <req_id>` can reconstruct the direct Lab-1 semantic command chain from the same `actingcommand-ledger` records.
- L4 containment light-event bridging has started: `package validate`, `package inspect`, and direct blocked `package run` write package containment light events when a run root is available.
- L6 ledger read-only query and diagnostic commands are implemented: `ledger show`, `ledger events`, `ledger receipts`, `ledger diagnose`, and `ledger evidence` now query existing runtime-ledger/evidence files without device I/O.
- L6 queries support `--run-root` plus `--run-id`, `--req-id`, and `--instance-id` filters where applicable; corrupt JSONL tail counts are surfaced in query outputs instead of hiding partial-read state.
- `ledger diagnose` reports incomplete/not-found/terminal status, receipt counts, finalizing counts, terminal receipt data when present, and missing-output diagnostics for `finish_ok` records.
- L7 Evidence / FrameStore convergence has started: `lab run` now indexes materialized screenshot files into `EvidenceStore`, records an `evidence_index` ledger drive record, projects screenshot evidence refs into `summary.json`/`diagnostics.json`, and writes `logs/evidence.json` into result.zip from runtime-ledger projection data.
- Screenshot evidence indexing is explicitly degraded, not silent, when a materialized screenshot file cannot be read or stored; the degradation reason is recorded in ledger-backed evidence metadata and emits an `evidence_index_degraded` event.
- L7 recognition traceability has started: `lab run` now issues a `reco_id` for each recognition capture, stores it in the recognition drive record id chain, and projects the same id into result.zip `logs/recognition.jsonl`.
- L7 capture backend attempt traceability has started: `lab run` now records the requested/used capture backend and backend attempt diagnostics as a runtime-ledger drive record after backend selection.
- L7 action/step traceability has started: executed `lab run` operation steps now receive an `action_id`, carry it through started/finished input events, and store it in both the step drive record id chain and projected step payload.
- L7 recognition evidence traceability has started: `lab run` now stores per-recognition detail payloads in `EvidenceStore`, projects the evidence ref into `logs/recognition.jsonl`, and records the `evidence_id` in the recognition drive record id chain.
- L5 CLI projection convergence has started: Lab-2 command success and error payloads now read back the durable runtime-ledger receipt after write, project the CLI response from that receipt, and expose `projection_source.kind=runtime_ledger`.
- L5 semantic CLI projection convergence has started: direct Lab-1 semantic commands `detect-page`, `tap-target`, and `navigate` now read back the durable runtime-ledger receipt and project their CLI output from it with `projection_source.kind=runtime_ledger`.
- L8 acceptance closeout is recorded in `benchmarks/reports/2026-07-09-runtime-ledger-issue28-l8-acceptance.md`; current implemented-module fact surfaces meet the 95 percent gate at 23/23, and successful Lab output-zip receipts now reject missing files or sha256 mismatches during projection.

Issue #28 completed status and follow-up tail:

- L0-L8 are accepted for the issue #28 current implemented-module scope.
- The Session `request-journal.jsonl` compatibility sidecar still exists, but covered read/status/event paths consume or cross-check runtime-ledger receipt projections and fail loudly on conflict.
- Future UI, scheduler, agent-only protocol, database, encrypted log service, and new game-logic modules remain excluded from the issue #28 denominator and need their own runtime-ledger integration when implemented.

Current boundary:

- This L0 node only changes terminal Lab result recording order and the minimal ledger guard API.
- `LabRunContext` remains the Lab execution context, archive assembler, and owner of frame-store paths and screenshots.
- The current L3 work repairs session cancellation ordering, adds a first Session request receipt bridge into runtime-ledger, records submit dispatch facts, records failed acknowledgement-timeout receipts, prevents legacy journal write failure from keeping an already-receipted request executable, and makes `request-state get`, `session journal`, `session events`, `request-state list`, `session response get|wait`, `session queue`, `session status --diagnostics`, cancellation not-found checks, and synchronous daemon-routed request output consume or cross-check ledger facts with conflict detection; full Session convergence remains unfinished while the compatibility request journal still exists.
- The current L4 work adds a direct Lab-1 semantic command bridge for `detect-page`, `tap-target`, and `navigate`, plus first package containment light events for `package validate|inspect|run`; full L4 closure, remaining CLI-wide projection, UI, database, scheduler projection, encryption log service, and game logic remain out of this node.
- The current L5 work projects the covered Lab-2 and direct Lab-1 semantic CLI outputs from durable runtime-ledger receipts; future modules excluded from issue #28 will need the same projection rule when implemented.
- The current L6 work adds read-only ledger/evidence inspection commands only; it does not trigger capture, touch, runtime daemon, or game-device work.
- The current L7 work connects already-materialized `lab run` screenshots, per-capture `reco_id` values, per-recognition detail evidence refs, capture-backend selection attempts, and per-step `action_id` values to runtime-ledger projection output only; it does not add new capture, OCR, matching, UI, database, scheduler, or game-device behavior.
- The current L8 closeout adds projection-time validation for successful output-zip receipts and records the 23/23 implemented-module acceptance matrix; it does not add live-device work or expand scope to excluded future modules.
- Resource repositories are not read or modified by this task.

## Current task-pack containment module

The active approved issue is Runtime issue #26:

- `[基础模块] 任务包加载模块(全内存):资源加载唯一咽喉 · 每实例一操作台 + 哈希校验 + LoadedBundle capability`
- Technical source of truth: `C:\合作工作区\ActingCommand\DECISION-task-pack-containment-module.md`

Implementation direction:

- Add `crates/pack-containment` as the resource loading chokepoint for single-task zip packages.
- Keep one isolated bench per instance, with at most one resident loaded package per bench.
- Verify the compressed zip hash before decompression.
- Decompress into memory only, with compressed size, total decompressed size, per-entry size, entry count, and per-instance resident-byte limits.
- Reject zip-slip paths, duplicate entries, executable/script entries, malformed zips, oversized packages, manifest hash mismatches, and invalid package structure as fatal containment errors.
- Make `LoadedBundle` the capability returned only by `Containment::load`; fields stay private so downstream code cannot fabricate a verified bundle.
- Keep Lab zip package resources in memory as `LoadedBundle` data; do not unpack Lab resources to a temporary directory for `lab validate --zip` or `lab run --zip`.
- Allow CLI callers to provide an external expected compressed-zip SHA-256 with `--expected-sha256`; when omitted, local Lab/package validation computes the expected hash from the selected local input file.
- Extend `recognition-pack` with an asset resolver abstraction so contained packages can feed recognition from memory while existing filesystem-backed callers continue to work.
- Route `package validate/inspect/run` and `lab validate/run` through containment before execution or inspection.

Current boundary:

- The containment crate now owns the shared bomb/slip/manifest-hash guards for `package` validation.
- `lab validate/run` now consume a containment-created `LoadedBundle` directly from memory for control, manifest, operation, recognition pack, pages, navigation, and operation assets.
- `--resource-root` directory-scattered loading is not part of the Lab zip execution path; older resource-root commands remain outside this task until separately migrated.
- No scheduler, UI, OCR, SQLite, game logic, encryption, trusted channel, or upstream source import is part of this task.

## Current Lab-2 chain repair round 5 closeout

The active repair task is `C:\合作工作区\ActingCommand\FIX-Lab-2-chain-round5-2a03180.md`.

Repair order:

- D1: make implicit Lab-2 lease release use real platform process liveness; stop using production fake-liveness release wrappers; drop expired queued requests with ledger records.
- D2: make explicit `lab arbitrator acquire` create bearer/long leases without `holder_pid`, reject `reclaim-dead` for those leases, keep `force-unlock` as the administrative escape, and prevent concurrent `do --lease-id` double driving.
- D3/D4: make PATH-ADB baseline warnings visible in doctor/devices/device connection payloads, remove stale "does not fall back to PATH" wording, ignore empty/relative PATH entries, require `adb.exe` on Windows PATH, and isolate ProgramFiles discovery in tests.
- D5: add a true child-process contract test for dead queued requester drop on release.
- D6: keep Lab-2 ledger failures from masking original usage errors, remove session/config env leakage in contract helpers, fix the destructive-gate error axis, and record boundaries for remaining lease/session closeout follow-ups.

Round-5 status:

- Implemented locally and verified against Runtime baseline `2a03180`.
- Implicit short-lease release now uses `platform_process_is_alive`; production `release`/`reclaim_dead_holder` fake-liveness wrappers are test-only.
- Explicit arbitrator acquire now grants a bearer long lease without `holder_pid`; `reclaim-dead` fails loudly when liveness cannot be proven, while `force-unlock` remains the explicit bypass.
- Explicit `do --lease-id` calls now set an active driver request on the long lease and reject concurrent drivers with `lease_in_use` until the first driver finishes.
- Expired queued requests and dead queued requester drops now write dispatch ledger records from the arbitrator boundary.
- PATH-ADB baseline warning metadata is surfaced through doctor, devices, capture, touch, stream, session instance, and session app paths; MuMu/Nemu IPC targets reject PATH baseline unless `ACTINGCOMMAND_ALLOW_PATH_ADB_FOR_MUMU=1` is set.
- Lab-2 contract helpers remove `ACTINGLAB_SESSION_STATE_DIR` and `ACTINGLAB_CONFIG_PATH`; ledger-write failure now preserves the original usage error in the response.
- No UI, OCR, SQLite, live-device operation, resource repository changes, or upstream code copying were done in this round.

Round-5 follow-ups:

- The alive-promotion SessionLease projection remains bounded to existing explicit projection calls; no new projection ownership model was added.
- Adjacent `session_closeout.rs` environment isolation should be reviewed in a future task if those tests start sharing process-level env state.
- Real-device validation remains outside this synthetic/offline repair round.

## Current Lab-2 chain repair round 4 closeout

The active repair task is `C:\合作工作区\ActingCommand\FIX-Lab-2-chain-round4-ea3744e.md`.

Repair order:

- C1: seal Lab-2 tests from real `%LOCALAPPDATA%` and `%APPDATA`; child-process helpers now require an explicit app-data TempDir.
- C2: route `admit_lab2_request` usage-validation failures for invalid `--priority` and `--queue-deadline-ms` through dispatch/receipt ledger records.
- C3: add a non-MuMu PATH-ADB baseline channel with explicit warning metadata, and keep R8.6/do-stub disclosures visible.
- C4: add the missing R6 coverage for non-dry-run resource drift before fake touch, destructive tri-state opt-in, and projection of 20 decision targets with `_more`/`_full`.
- C5: keep the RAII panic-cleanup item deferred, add Win32 FFI `SAFETY` notes, and register the PID reuse/creation-time-key risk.
- C6: make queued-request promotion liveness-aware so dead queued requesters are dropped and ledgered instead of becoming orphan holders.

Round-4 status:

- Implemented locally and verified against Runtime baseline `ea3744e`.
- Lab-2 unit and child-process contract tests are isolated from the user's real app-data directories.
- `lab arbitrator release`, `reclaim-dead`, and `force-unlock` now check queued requester liveness before promotion; dropped queued requests write `queue_dropped_dead_requester` ledger records.
- The PATH-ADB fallback is intentionally a baseline channel for non-MuMu environments and reports a warning rather than silently pretending to be MuMu-specific discovery.
- True-device `do` execution remains represented in tests by the explicit fake-touch/test-stub path; these tests verify admission, drift, and safety gates but do not claim live click validation.
- R8.6 remote-drive state warning and RAII short-lease cleanup remain follow-up items; pid-aware reclaim is the current crash-residue recovery boundary.
- PID reuse remains a registered low-frequency TOCTOU risk; a future process creation-time key should be added before treating pid identity as strong proof.
- No UI, OCR, SQLite, game logic, resource repository changes, live-device operation, or upstream code copying are part of this round.

## Current Lab-2 chain repair round 3

The active repair task is `C:\合作工作区\ActingCommand\FIX-Lab-2-chain-round3-1da3aa0.md`.

Repair order:

- T1: add a child-process stdout purification guard that exercises the vendor-stdio redirection path with Win32-handle diagnostic noise instead of relying only on dry-run static PNG output.
- T2/T3: make `reclaim-dead` honest by storing `holder_pid` on short-lived lease holders and reclaiming only after real process-liveness checks; long leases without a pid require explicit `force-unlock`.
- T4: route arbitrator state-load/admission failures and `lab arbitrator ...` command failures through Lab-2 dispatch/receipt ledger writes.
- T5: require recognition packs to declare `coordinate_space` so missing or zero authored resolution fails loudly instead of accepting arbitrary scene sizes.
- T7: make `lab arbitrator acquire` the single long-lease issuer and project the same lease id into SessionLease state so `do --lease-id <id>` works without manual dual-lease alignment.
- T6: keep lower-priority cleanup honest by documenting the test-only capture delay flag and keeping existing projection/destructive/drift coverage visible without claiming true-device execution for fake-touch tests.

Round-3 status:

- Implemented and committed in Runtime source commit `53ff3b0`; this checkpoint metadata update will be pushed with the same round-3 closeout.
- `VendorStdioSession` now has an actinglab child-process self-test entrypoint that proves stdout starts with JSON while simulated vendor stdout/stderr noise is captured internally.
- `RequestEnvelope` and `LeaseGrant` carry optional `holder_pid`; pid-aware reclaim is covered by arbitrator unit tests and CLI liveness honesty assertions.
- `lab arbitrator acquire` now writes a SessionLease projection with the same lease id, and release/reclaim/force-unlock remove the matching projection.
- Admission and arbitrator command failures now produce dispatch/receipt records that `lab receipt --req` can reconstruct.
- Recognition pack validation now rejects missing or zero `coordinate_space`; affected Runtime/device-test fixtures have been updated to declare authored resolution.
- No UI, OCR, SQLite, live-device operation, resource repository changes, or upstream code copying are part of this round.

## Current Lab-2 chain repair round 2

The active repair task is `C:\合作工作区\ActingCommand\FIX-Lab-2-chain-round2-b2507b7.md`.

Repair order:

- R1: make the Lab-2 degraded arbitrator persistent by default under the app state root when `--state-dir` is not provided.
- R7: define lease lifecycle semantics: bare `do`/`ensure` use implicit short leases that occupy the persistent arbitrator during execution and release on success or failure; explicit long leases are acquired with `lab arbitrator acquire` and reused by `do --lease-id`.
- R2: guarantee that every post-admission exit path for `observe`, `wait`, `do`, and `ensure` writes dispatch/receipt ledger records.
- R3/R4: move Nemu vendor stdout/stderr capture to a session boundary that covers DLL load and Win32 handle writes, and keep tests honest about synthetic versus true device paths.
- R8: remove the hard-coded `D:\BST\MuMuPlayer` production discovery path, make ADB text decoding lossy with diagnostics instead of fatal, require navigation `coordinate_space`, derive probe-run click space from runtime metadata, and mark Nemu channel order as MuMu-verified rather than universal.

Round-2 status:

- R1 is implemented: `explicit_lab2_state_dir` now defaults to `app_state_root()/lab2`, so production-style calls no longer silently fall back to an in-memory arbitrator.
- R7 is implemented: implicit short leases are acquired, persisted, and released around bare `do`/`ensure`; `lab arbitrator acquire` provides the long-lease path; `reclaim-dead` no longer marks a live holder dead, and `force-unlock` is explicit about bypassing liveness.
- R2 is implemented structurally with a shared post-admission result finalizer for success and error responses.
- R3/R4 are implemented at the capture boundary with `VendorStdioSession`, including a Win32-handle snapshot test. True Nemu/live validation remains an external machine check, not claimed by synthetic tests.
- R8.1/R8.2/R8.3/R8.4 code-level safeguards are implemented for the Runtime paths touched in this round. R8.5/R8.6 remain recorded as lower-priority portability follow-ups.
- Focused tests now cover default arbitrator persistence, short-lease no-self-lock, concurrent bare-write blocking, explicit long-lease reuse and third-party blocking, post-admission receipts for all four Lab-2 verbs, lossy ADB decode diagnostics, navigation coordinate-space-derived click bounds, and MuMu-specific Nemu channel-order labeling.

## Current Lab-2 CLI chain repair

The active repair task is `C:\合作工作区\ActingCommand\FIX-Lab-2-chain-9feebc3.md`.

Repair order:

- W: wire the existing arbitrator, ledger, evidence, suspicion, and coordinate derivation logic into real Lab-2 CLI paths.
- P: preserve decision arrays through projection by summarizing instead of deleting them.
- S: enforce destructive-action safety geometrically and keep rejected actions auditable.
- L0b: extend vendor stdout/stderr capture to Win32 standard handles.
- T: add tests that cover the previously hollow integration paths, including non-dry-run device drive behind SessionLease plus Lab-2 arbitrator admission.

Repair status:

- W1-W4 are implemented: persistent Lab-2 arbitrator state, real-click lease authorization, shared live-match coordinate derivation, ledger retention, suspicion helpers, `lab evidence --id`, and `lab arbitrator ...` maintenance commands are connected to real CLI paths.
- P1-P3 are implemented locally: min projection keeps decision arrays with bounded summaries and `_full` evidence pointers, while verbose/field-aware errors use the shared Lab-2 projection request.
- S1-S2 are implemented: `do` checks destructive geometry against navigation metadata and writes ledger records for post-admission rejection paths, including admission failures that previously returned before audit.
- L0b is implemented at the device crate boundary by capturing Win32 standard-handle writes in addition to CRT fd writes and exposing captured vendor output through capture diagnostics.
- T coverage has been strengthened with persistent-arbitrator six-state CLI coverage, queue deadline expiry, cancellation, reclaim, preemption, destructive-ledger, evidence, unknown+candidates, five error axes, verbose-error, live-match drift, delayed stub observe latency, cross-game synthetic pack coverage, child-process stdout purity, and non-dry-run fake-touch tests.
- S7 / L6 MCP projection remains a documented deferred optional node.
- Final public five-command gate passed locally for repair source commit `cf471d9`.

## Current Lab-2 CLI chain

The active task is `C:\合作工作区\ActingCommand\TASK-Lab-2-chain.md`.

This task chain is being implemented as independent nodes. Each completed node must pass the public five-command gate before commit:

- `cargo fmt --all -- --check`
- `cargo build --release`
- `cargo clippy --workspace -- -D warnings`
- `cargo test --workspace`
- `git diff --check`

Current node:

- S8 / L7 chain acceptance.

S7 / L6 MCP projection status:

- Deferred. The task file marks MCP projection as optional and non-blocking.
- No MCP server or MCP-specific semantics were added in this Runtime pass.
- The shared Lab-2 command contracts and capability/schema generation now provide the source material a later MCP projection should consume.

S8 scope:

- Add synthetic acceptance checks for Lab-2 CLI safety and machine-readable contracts.
- Verify compact min projection size, synthetic observe latency, ANSI-free JSON output, actionable error details, one-response `do` observation, and req_id ledger-chain reconstruction.
- Record which acceptance gates are covered by previous S1-S6 tests instead of duplicating large channel/vendor/ledger-retention fixtures.

S8 status:

- Complete locally.
- Added Lab-2 chain acceptance tests for min projection size, observe latency, error `req_id/state/hint`, no ANSI output, `do` actual-click/guard/observation in one response, and `lab receipt --req` reconstruction.
- S1-S6 remain the source for the three-backend channel contract, stdout purity, ledger projection/retention, degraded arbitration, CLI verbs, capability/schema, and recovery transparency gates.
- Public five-command gate passed locally.

Lab-2 chain status:

- S1-S6 and S8 are implemented in Runtime with synthetic/offline tests.
- S7 MCP projection remains a documented deferred optional node.

S6 scope:

- Let Lab-2 `observe` remain read-only during a recovery-in-progress state while surfacing `state:"recovering"` and a light-event-shaped recovery progress payload.
- Let write verbs such as `do` and `ensure` wait by default for recovery to clear, with bounded timeout/poll controls.
- Support `--no-wait` for immediate structured failure when recovery is active, including `req_id`, recovery context, and planned action details.
- Keep this node a degraded bridge over the existing Session Layer/C2 recovery work: no full scheduler, no UI, no OCR, no SQLite, no live device validation, and no game logic.

S6 status:

- Complete locally.
- Lab-2 reads `lab2-recovery-state.json` from the selected Session state directory as the current degraded recovery-state bridge.
- `observe` now reports `state:"recovering"` and a `recovery.state.changed` light event when that bridge says recovery is active.
- `do` and `ensure` now wait for recovery to clear by default and fail fast with `--no-wait`.
- Capabilities and command schema now describe the recovery transparency behavior and wait flags.
- S6 targeted synthetic tests are added.
- Public five-command gate passed locally.

Next Lab-2 node:

- S7 / L6 MCP projection after S6 is committed and pushed, unless it is explicitly deferred.

S5 scope:

- Extend `capabilities` with Lab-2 verbs, parameter/output schemas, click kinds, schema-version range, recognition-engine capability truth, current config instances, error/exit code tables, and escape-toolbox groups.
- Extend `schema <command>` for Lab-2 `observe`, `do`, `ensure`, `wait`, and `lab receipt`.
- Add `lab receipt --req <id>` to reconstruct the ledger chain for a request id.
- Carry `req_id` through the request, arbitration, lease, recognition, action, and receipt records for Lab-2 `do`.
- Keep this node synthetic/offline-only: no live device work, UI, OCR, SQLite, full scheduler, true resource repository validation, or game logic.

S5 status:

- Complete locally.
- `capabilities` now exposes a Lab-2 capability block generated from the same command-contract source used by `schema`.
- `schema do`, `schema observe`, `schema ensure`, `schema wait`, and `schema lab receipt` describe the single-line JSON output shape and command parameters.
- `do` now issues a `reco_id` alongside `action_id` and writes recognition/action drive records plus an id-chained final receipt when a run root is configured.
- `lab receipt --req <id>` loads all matching JSONL ledger records from the configured run root.
- S5 targeted synthetic tests are added.
- Public five-command gate passed locally.

Next Lab-2 node:

- S6 / L5 self-heal transparency after S5 is committed and pushed.

S4 scope:

- Add top-level Lab-2 `observe`, `do`, `ensure`, and `wait` verbs to the existing `actinglab` binary.
- Route each verb through the S2 ledger projection boundary and the S3 degraded arbitrator request envelope.
- Keep the implementation synthetic/offline-testable: no live devices, true resource repositories, UI, OCR, SQLite, game logic, or upstream code copying.
- Preserve existing semantic recognition, navigation, guarded click, and ROI-stability helpers instead of duplicating them.

S4 status:

- Complete locally.
- `observe` returns page, requested target evaluations, navigation actions, frame age/backend metadata, req_id, arbitration summary, optional `--with-frame` output path, and compact ledger projection output.
- `do` performs a guarded target check before action planning/execution, returns `actual_click` and `guard_result`, supports dry-run, and reports guard failures with structured error details.
- `ensure` handles idempotent target-page success and dry-run navigation route planning through the existing navigation graph.
- `wait` supports page wait and target-stability wait using the existing Lab ROI target-stability comparator.
- S4 targeted synthetic tests are added.
- Public five-command gate passed locally.
- Next Lab-2 node is S5 / L4 capabilities and safety unless the task order is revised.

Completed Lab-2 nodes:

- S8 / L7 chain acceptance.
- S6 / L5 self-heal transparency.
- S5 / L4 capabilities and safety.
- S4 / L3 CLI core verbs.
- S3 / L2 degraded arbitrator and lease.
- S2 / L1 logging module core.
- S1 / L0 prerequisite repair batch.

S3 status:

- `actingcommand-arbitrator` exists as an independent workspace crate.
- Request envelopes, queue length 1, priority admission, queue-full rejection, queued cancellation, deadline expiry, lease release, dead-holder reclamation, device-drive authorization, and dispatch/receipt record generation are implemented as pure logic.
- The runtime-core parallel `SchedulerGate` file now carries a deprecated-prototype banner.
- The dead `actingcommand-runtime-core` and `actingcommand-task-loop` dependency declarations were removed from `apps/actinglab/Cargo.toml`.
- Public five-command gate passed locally.

S2 scope:

- Add the independent `actingcommand-ledger` crate required by the 2026-07-05 r5 task update.
- Keep ID issuing, ledger JSONL append/read, light events, evidence references, retention selection, compact output projection, and suspicion helpers in this crate instead of `actingcommand-runtime-core`.
- Keep the node synthetic-only and avoid real samples, resource repositories, live devices, OCR, UI, SQLite, scheduler behavior, or game logic.

S2 status:

- `actingcommand-ledger` exists as an independent workspace crate.
- L1 id issuing, ledger, light event, evidence, retention-selection, projection, and suspicion-helper tests are implemented.
- Targeted crate tests and clippy passed locally.
- Public five-command gate passed locally.
- Next Lab-2 node is S3 / L2 degraded arbitrator and lease unless the task order is revised.

S1 scope:

- L0.a: fix the Nemu IPC capture channel-order contract so MuMu RGBA bottom-up buffers are converted to Runtime top-down RGBA without swapping red and blue channels.
- L0.a: keep adb screencap, DroidCast_raw, and Nemu IPC covered by synthetic channel-order contract tests.
- L0.b: add a process fd-level vendor stdout/stderr capture boundary around Nemu IPC DLL calls so vendor noise cannot leak into ActingLab command stdout before the command JSON.
- L0.b: keep captured vendor output in an internal diagnostics buffer until the Lab-2 logging ledger lands in L1.

S1 boundaries:

- no real samples or resource repositories;
- no live device validation;
- no OCR, UI, SQLite, game logic, scheduler, or full logging-ledger implementation;
- no upstream code copying.

S1 status:

- L0.a implementation is complete locally and covered by synthetic tests.
- L0.b implementation is complete locally and covered by synthetic fd-capture tests.
- Public five-command gate passed locally.
- S1 was committed and pushed as `5e28f2d`.

## Current AK MAA data fidelity chain

The active task is `C:\合作工作区\ActingCommand\TASK-AK-maa-data-fidelity.md`.

The active repair task is `C:\合作工作区\ActingCommand\FIX-fidelity-stage1-673102e.md`.

The active round-two repair task is `C:\合作工作区\ActingCommand\FIX-fidelity-round2-e376c8b.md`.

The active round-three repair task is `C:\合作工作区\ActingCommand\FIX-fidelity-round3-8a03613.md`.

Round-three repair status:

- P1 task-name `@` component safety is implemented. Materialization now rejects task names with more than 64 `@`-composition components before recursive split/materialize paths can overflow the process stack. Synthetic tests cover 5000 components, 63/64 allowed, and 65 rejected.
- P2 coordinate derivation is unified for absolute coordinate actions. `rect`, `specific_rect`, `point`, `long_press`, and both ends of `drag` now derive from the live matched template guard delta; `offset` keeps the existing matched-rect-plus-offset relative behavior. Color-probe guards cannot drive coordinate translation, and explicit trusted unguarded coordinates remain the only no-guard path.
- P3 closeout is implemented: `should_default_template` has line-anchored protocol comments, Return/Return2 has an implicit-algorithm golden, `replaceFull` is stripped as an OCR-specific field, the `template` field ownership comment includes the FeatureMatch path, and CLI help documents the shared `recognize --target` output shape.
- No true resource repository, real sample, or live device validation is used for this round-three repair.
- Round-three public gates passed: `cargo fmt --all -- --check`, `cargo build --release`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace`, and `git diff --check`.

Round-two repair status:

- R1 plain `baseTask` template semantics are restored to the MAA protocol value: a template-matching derived task without its own `template` now defaults to `<child task>.png` even when the base task declares a template. The previous B2a inheritance behavior is removed, and Return/Return2-style synthetic coverage is added.
- R2 `@` composition growth cycles are bounded by a 64-step expansion depth guard. Exact virtual-cycle detection remains in place, while growing names such as `A@A#next` now fail during conversion instead of recursing without bound.
- R3 T5 coverage is added for source-anchor guard synthesis, `rect`/`specific_rect` missing-guard rejection, guarded drag start-point derivation from the live matched guard target, guard-target mismatch rejection, and explicit trusted unguarded gesture opt-in.
- R4 `inputText` is now treated as a TaskInfo field and survives algorithm changes. Algorithm-specific inheritance comments now point to the relevant MAA protocol lines and AsstTypes.h field ownership.
- R5 synthetic coverage is added for drift target not-found/ambiguous errors, measured `matched_rect` drift amend input without proposed region, action-time offset target mismatch, shared `recognize --target` evaluation JSON shape, and `--maa-tasks` help visibility.
- No true resource repository, real sample, or live device validation is used for this round-two repair.
- Round-two public gates passed: `cargo fmt --all -- --check`, `cargo build --release`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace`, and `git diff --check`.

Stage-one repair status:

- S1/W1 MAA compiler wiring is implemented in the narrowed explicit-input form.
- `resource convert --maa-tasks <dir>` now enables MAA task graph expansion before pack generation.
- Default Operation Bundle conversion remains unchanged when `--maa-tasks` is not supplied.
- In explicit MAA mode, compiled task fields can feed template pack target metadata: `templThreshold` -> `threshold`, `method`/`matchMethod` -> `method`, `maskRange` -> `mask`, and `rectMove` -> `rect_move`.
- S1 tests are synthetic-only and cover explicit `--maa-tasks` mode, expanded `baseTask`/`@`/`#` data landing in pack targets, and default-mode regression.
- S2/B1 virtual reference cycle handling is implemented. `#next`/`#sub`/other virtual field expansion now preserves the in-progress task stack through expression parsing and nested virtual lookups, so cycles fail with an explicit chain instead of recursing.
- S2 tests are synthetic-only and cover self, two-node, three-node, and nested-expression virtual cycles plus a legal deep virtual chain.
- S3/B2a plain `baseTask` template inheritance is implemented. Plain `baseTask` derivations now preserve explicit templates inherited from the base chain, synthesize a task-specific fallback only when the whole plain base chain has no template, and keep explicit child templates authoritative.
- S3 tests are synthetic-only and cover inherited parent templates, all-template-missing fallback defaults, child template override, and the existing `@` derived-template behavior.
- S4/B2b+B4+B5 protocol-check batch is implemented. The MAA task compiler now records local protocol-source comments for `@` list rebasing, explicit/default template naming, bare `#back`, and algorithm-specific inheritance filtering.
- S4 tests are synthetic-only and cover implicit `@` template inheritance, explicit `@` template rebasing, explicit `@` plus declared `baseTask` priority, bare `#back` skipping, and algorithm-change inheritance filtering.
- S5/B3 matched-rect offset semantics are implemented. Template matching now carries matched width/height, target diagnostics expose `matched_rect`, offset clicks use the live matched template rectangle plus offset instead of static guard `expected_rect`, and color-probe guards are rejected for offset clicks.
- S5 tests are synthetic-only and cover matched-rect offset click placement, color-probe offset rejection, and propagated template match dimensions.
- S6/W3 M3 Phase A loading diagnostics are implemented. Recognition pack loading now records unsupported method/mask targets, lab package diagnostics expose unsupported target counts/details, and package validation reports unsupported recognition target counts without evaluating templates.
- S6 tests are synthetic-only and cover unsupported method/mask load markers, pure NCC count zero, lab validate diagnostics, package validate diagnostics, and fail-loud evaluation behavior.
- S7/W4 expect-after and rectMove-to-offset flow is implemented. Operation primitives preserve `expect_after`, lab execution uses `expect_after.page_id` as an explicit after-action confirmation guard, and MAA-derived `rectMove` can convert guarded point/rect clicks into matched-rect `offset` clicks.
- S7 tests are synthetic-only and cover expect-after hit/mismatch verification and MAA `rectMove` landing as a generated primitive offset click with preserved `expect_after`.
- S8/W2 drift-to-amend is implemented. `session record amend --from-drift-diagnostics <path>` now consumes the resource-drift diagnostic contract, locates the matching anchor or verify-template record step, rejects proposed changes outside the region/threshold whitelist, refreshes frame-backed artifacts/evaluation, and leaves `session record build-task` as the bundle materialization authority.
- S8 tests are synthetic-only and cover the drift diagnostic contract, whitelist rejection, missing diagnostics failure, amend-to-record-to-build-task flow, schema 0.4 pack construction, and schema describe output through 0.5.
- S1-S8 are complete for the Codex synthetic Runtime lane. The next gate is external stage-one adversarial acceptance; true resource-repository validation remains outside this Codex task boundary.

The 2026-07-04 r3/r4 task-file update narrows the current Codex lane to Runtime logic and synthetic/offline program construction only:

- Codex owns the Runtime implementation for M1-M4 and M7.
- Codex tests must use synthetic bundles, synthetic task graphs, and synthetic frames for this chain.
- Codex must not operate on, validate, sample, or submit true resource-repository data for this chain.
- Claude owns true resource-repository verification, M6 calibration, M8 template/art processing, and data-vs-parser attribution on true samples.

Codex offline implementation status:

- M1 MAA task graph compiler is implemented as a Runtime-owned data-boundary pass. `resource compile-maa` expands MAA task JSON inheritance, explicit and implicit `@` rebasing, and `#` virtual list references without importing or copying the upstream MAA engine.
- M2 schema `0.5` support is implemented across resource conversion, operation bundle loading, recognition pack loading, page detection loading, and session-record build output while keeping `0.3` and `0.4` compatibility where the existing readers require it.
- M3 Phase A is implemented. Recognition packs can load `method` and `mask` metadata, count unsupported targets, and fail loudly when unsupported `rgb_count`, `hsv_count`, or mask semantics are evaluated instead of silently treating them as NCC.
- M4 offset and `specific_rect` click safety is implemented. Offset clicks require guard metadata and use the guard hit rect plus the offset rect; `specific_rect` remains subject to the existing guarded-coordinate requirement unless an explicit reviewed trusted-coordinate opt-in is present.
- M7 offline record closeout is implemented for schema `0.5`: session recording can build point, drag/swipe, and long-press operations; generated bundles include top-level provenance; and the record-build golden path verifies record -> convert -> package validation plus a recorded navigation edge.

Codex validation status:

- Public local gates passed: `cargo fmt --all -- --check`, `cargo build --release`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace`, and `git diff --check`.
- Historical pre-r3 true-resource smoke evidence remains recorded in `CHECKPOINT.md` and `benchmarks/reports/ACCEPTANCE-AK-maa-data-fidelity-2026-07-03.md`, but it is no longer treated as a current Codex-owned completion gate.
- Runtime guard synthesis now supports three reviewed sources: explicit `verify_templates`, source-page anchors, and operation-level verify templates with click-derived expected rects. `page_id: "any"` guards remain target-checked and are used only for generic recovery controls.
- Runtime `resource convert` supports source `page_rules`, and the historical AK retained-frame repair evidence remains useful for attribution, but further true retained-frame calibration now belongs to the Claude resource lane.
- Page detection now supports `any_of` page groups. Runtime conversion emits mutually exclusive anchor variants such as AK `operator_0` / `operator_1` as one `any_of` group instead of requiring both visual states simultaneously, and generated page validation/check-pages accepts that schema without weakening forbidden-target gates.

Remaining chain gates before the broader CLI entry condition:

- Claude-owned stage-two true-resource verification, M6 calibration, and M8 first-batch template/art processing remain the broader CLI-entry blockers.
- If true-resource verification finds source data correct but Runtime output wrong, that becomes a new Codex parser/Runtime fix task with synthetic regression coverage.
- If true-resource verification finds source data stale, weak, or wrong, that remains a resource-lane data repair.
- M5 overlay layering remains intentionally non-blocking for CLI entry.
- M3 Phase B/C masked NCC and RGB/HSV count engines remain future work; Phase A fail-loud handling prevents silent misuse until they are implemented.

## Current Lab self-heal acceptance repair

The active task is `C:\合作工作区\ActingCommand\FIX-selfheal-chain-acceptance-87c1e4a.md`.

Blocking repair status:

- R5 recovery-resource path double-`ours`: implemented. `--use-recovery-resource` now resolves from the already-normalized resource root as `recovery/<game>.<server>.recovery.json`, and a reorganized `repo -> ours` regression test covers the path.
- R1 C2 recovery loop true execution: implemented locally. Non-dry-run monitor resource recovery now executes through an injectable Session Layer throat adapter; dry-run remains plan-only; non-constant detect routes through the injected detector path.
- R2 recovery lease-yield: implemented locally. Active mismatched leases defer recovery before any page-changing Session Layer action; no active lease or a matching lease allows execution.
- R3 C3 escalation ladder and reference resolvability: implemented locally. Recovery actions preserve resource order, failed/not-arrived actions advance to the next escalation action, and named flows/control points/external sources must resolve during plan construction without game-specific branching.
- R4 stored operation package guard migration: implemented locally. `resource convert` now synthesizes guard metadata from operation-level `verify_template` data when the matching `verify_templates` entry provides a rectangular region, preserves explicit trusted unguarded opt-in, fails loudly when guard data cannot be derived, and validates converted guard page/target/type references across pack/pages/primitives.

The full C0.c -> C3 self-heal acceptance repair has passed local public validation and remote CI verification. R1-R5 are closed for this acceptance repair; the non-blocking P-series items remain follow-up work unless a later task pulls them in.

## Current implementation line

- Rust workspace is the mainline implementation.
- Python runtime is legacy/mock only and lives outside this repository.
- Go runtime/core is historical reference and benchmark material only and lives outside this repository.

## Current Lab self-heal chain

The `TASK-Lab-selfheal-chain.md` C-line is active as the current Runtime self-heal task chain.

Delivery order:

1. C0.c guarded coordinate action: implemented in `benchmarks/reports/2026-07-03-lab-selfheal-c0c.md`.
2. C0.b ROI stability gate: implemented in `benchmarks/reports/2026-07-03-lab-selfheal-c0b.md`.
3. C0.a resource drift stop-loss: implemented in `benchmarks/reports/2026-07-03-lab-selfheal-c0a.md`.
4. C1 trigger classification and priority routing: implemented in `benchmarks/reports/2026-07-03-lab-selfheal-c1.md`.
5. C2 live recovery loop wiring plus H1 loop detection fix: implemented in `benchmarks/reports/2026-07-03-lab-selfheal-c2.md`.
6. C3 login/wake resource wiring: implemented in `benchmarks/reports/2026-07-03-lab-selfheal-c3.md`.

Current C0.c behavior:

- Operation Bundle fixed-coordinate actions require guard metadata by default.
- Guard metadata records `page_id`, `target_id`, `expected_rect`, and a `verify_template` or `color_probe`.
- Missing guard metadata fails package validation loudly unless `unguarded_trusted_coordinate: true` is explicitly set.
- ActingLab captures and evaluates a fresh execution-time guard frame before opening the touch backend or sending MaaTouch input.
- Page mismatch and target mismatch refuse the click and report the current screen state through run events and failure diagnostics.
- `session record build-task` emits guard metadata for recorded coordinate operations.

Current C0.b behavior:

- ActingLab now runs a ROI-level stability gate after the C0.c guard passes and before opening the touch backend or sending MaaTouch input.
- The gate reuses the guard target evaluation as its baseline and evaluates the same target across follow-up frames.
- Static ROI targets pass on the first follow-up frame, changing targets wait until stable, and continuously changing targets fail loudly as `unstable_page`.
- Page changes during the stability wait refuse execution as `page_guard_mismatch`.
- The gate reuses existing recognition target evaluation and does not add a whole-frame freeze detector.

Current C0.a behavior:

- When the C0.c page guard passes but the guarded target mismatches, ActingLab runs a bounded resource drift probe before opening the touch backend or sending MaaTouch input.
- The probe reuses the guarded target in its expected rect across follow-up frames.
- A target that recovers to a passing evaluation returns to the normal C0.b ROI stability gate.
- A stable target mismatch is classified as `resource_drift` and fails loudly with recalibration diagnostics.
- Drift diagnostics include the target id, expected rect, measured target result, observed frame count, operation provenance, and provenance version when available.
- Moving mismatches remain `unstable_page`; page changes remain `page_guard_mismatch`.
- `session self-heal-plan --trigger resource_drift` is a stop-loss plan: no retry, no app restart, no heavy recovery candidate, and a resource recalibration blocker.

Current C1 behavior:

- `session self-heal-policy` now reports the canonical trigger set: `stale_frame`, `hang`, `resource_drift`, `session_expired`, `standby`, `modal_popup`, `off_route_page`, and `unstable_page`.
- Legacy monitor trigger names remain accepted and are mapped to canonical triggers for compatibility.
- `session self-heal-plan` accepts repeated `--trigger` values and comma-separated `--triggers` values, then selects the deterministic highest-priority trigger.
- Trigger priority is evidence-first: stale/hang before confirmed drift, then session/standby, modal, off-route, and unstable-page action gates.
- Recovery routes are trigger-specific: stale/hang to capture backend recovery, standby to wake, modal to dismissal, off-route to maintenance navigation, session-expired to startup login, drift to stop-loss recalibration, and unstable page to action-gate failure.
- `monitor --once` includes canonical trigger metadata for current monitor diagnoses without changing the existing status strings.

Current C2/H1 behavior:

- Resource repositories were refreshed before resource-dependent recovery wiring, and their local paths and commit hashes are recorded in `CHECKPOINT.md`.
- `monitor --recover --use-recovery-resource` resolves `ours/recovery/<game>.<server>.recovery.json` from the selected resource root.
- `monitor --recover --recovery-resource <path>` uses an explicit resource path and fails loudly if the file is missing or invalid.
- The monitor loop selects recovery rules by the C1 canonical trigger and executes them through `recovery_exec`.
- Daemon monitor policy recovery can run the same resource recovery graph when lease holder and lease id match.
- Recovery actions are represented as Session Layer signal actions; direct device execution is not allowed.
- Restart-class actions in recovery resources are skipped and recorded instead of executed.
- Resource drift remains a stop-loss condition, and unstable page remains an action gate; neither is sent into a recovery graph.
- Recovery output records journal metadata, graph status, visited nodes, selected rule id, actions, skipped restart actions, and the Session Layer/no-bypass boundary.
- H1 is fixed: recovery graph loop detection is checked before max-attempt exhaustion can mask a repeated node.

Current C3 behavior:

- Resource repositories are refreshed before login/wake recovery resources are read, and their paths and commit hashes are recorded in `CHECKPOINT.md`.
- Monitor recovery preserves recovery action metadata from `ours/recovery/<game>.<server>.recovery.json`.
- `run_recovery_flow` actions resolve named `recovery_flows` entries and fail loudly when a referenced flow is missing.
- `session_expired` prioritizes the `startup_login` recovery flow before other light actions so the login loop is the first Session Layer signal action.
- Arknights monitor recovery loads `STARTUP-LOGIN.md` for the startup-login resource loop and fails loudly when that required file is missing.
- AzurLane and BlueArchive recovery resources can use embedded `recovery_flows.startup_login` without requiring an external `STARTUP-LOGIN.md`.
- BlueArchive standby wake preserves wake control-point metadata, including the `(300, 2)` wake/dead-zone resource.
- Recovery output preserves `ref`, `args`, `control_point`, `flow`, and optional `startup_login` metadata while keeping `via=session_layer`, `direct_device_allowed=false`, and `executed_directly=false`.

These slices do not add OCR, UI, SQLite, scheduler behavior, FeatureMatch relocation, automatic resource rewrites, live emulator recovery execution, or game logic.

## Current P6.5-A punchlist repair

The `FIX-P6.5-A-punchlist-6ca6b0c.md` punchlist is implemented as the final P6.5-A acceptance tail repair.

Scope:

- `benchmarks/reports/2026-07-03-p65a-punchlist.md` records P1-P4 and S1-S6 status.
- The old P6.5-A closeout report now matches the A2 option-B wording: startup/request-level fresh probe is complete, while runtime-persistent backend switching remains deferred.
- MuMu discovery now exposes `DeviceDiscoveryReport` from both the production `discover_devices()` entry and the process-snapshot API, so discovery diagnostics are no longer silently discarded.
- MuMu processes without a recoverable instance id are skipped with diagnostics instead of being aliased to instance `0`.
- Artifact-lock expected-lock mismatch is covered at the `run()` process-gate level.
- Runtime library loadability has direct corrupt-DLL test coverage.
- Additional base64 decode, base64 padding, watchdog timeout, and FFI oversized-response boundary comments are included.

This punchlist does not add upstream source, upstream binaries, OCR models, resource repository data, UI, SQLite, scheduler behavior, device live operation, or game logic.

## Current P6.5-A acceptance defect fix

The `FIX-P6.5-A-acceptance-aea10a4.md` acceptance repair is implemented as a focused Runtime hardening pass on top of the README-only remote head `bb10374`.

Scope:

- `benchmarks/reports/2026-07-03-p65a-acceptance-fix.md` records F1-F12 and H1-H4 status.
- MuMu discovery now falls back only to existing adb candidates and no longer aliases invalid instance ids to `0`.
- `apps/vision-provider-check` now treats failed export-audit and artifact-lock checks as process-gate failures, supports expected artifact-lock diffing, validates configured runtime libraries during ABI checks, and uses checked PE export parsing.
- ONNXRuntime provider lifecycle support is shared through `crates/onnx-provider-support`, including idempotent runtime initialization, cancelable inference watchdogs, and mutex-protected session caching.
- `providers/onnxruntime-json` and `providers/ppocr-onnx-json` now use cancelable watchdogs instead of detached timeout sleepers.
- PPOCR ONNX sessions are cached behind mutexes and ONNXRuntime DLL selection fails loudly when no onnxruntime-named path is configured.
- `VisionFrame.pixels` now serializes JSON payloads as base64 instead of JSON byte arrays.
- `ProjectInterface` now rejects unknown fields, null option defaults, and invalid preset references at load time.
- A2 is recorded as startup/request-level capture probing plus visible stale-frame diagnostics only; persistent runtime backend switching is deferred and the unused `switch_backend` decision field is removed.
- H2 FFI oversized-response hardening is included. H1, H3, and H4 remain documented follow-ups.

This acceptance fix does not add upstream source, upstream binaries, OCR models, resource repository data, UI, SQLite, scheduler behavior, device live operation, or game logic.

## Current P6.5-A MaaFramework fusion chain closeout

The `TASK-P6.5-A-maa-fusion-chain.md` Runtime source-level gate is closed as of 2026-07-02.

Closeout evidence:

- `benchmarks/reports/2026-07-02-p65a-maa-fusion-chain-closeout.md` maps the task-file gates to current tests, reports, and source modules.
- P0, A2, A1.1, A3, B, E, R1, R3, A4, and O1 all have recorded evidence.
- The public validation commands passed locally: `cargo fmt --all -- --check`, `git diff --check`, `cargo test --workspace`, `cargo clippy --workspace -- -D warnings`, and `cargo build --release`.
- The latest Runtime GitHub Actions CI runs for the full-frame OCR implementation and checkpoint commits passed.
- The local `%LOCALAPPDATA%\ActingCommand\actinglab\config.json` file was absent before closeout validation, matching the task-file CI-style test input requirement.

This closeout does not bundle ONNXRuntime, FastDeploy/PPOCR, MAA release artifacts, OCR models, OCR dictionaries, OCR data, upstream source, UI, SQLite, scheduler behavior, device operation, or game logic. Release packaging review for OCR/NN artifacts remains future work and must be tracked as a separate task.

## Current P6.5-A MaaFramework fusion chain R1/R3

The 2026-07-02 P6.5-A R1/R3 OCR/NN route is accepted as an FFI-boundary-first implementation path and now has a callable dynamic-library adapter surface plus a provider artifact contract.

Scope:

- `crates/vision-ffi` defines the first safe Rust boundary for future OCR and NN engines.
- `crates/vision-ffi` now defines `VisionProviderArtifactManifest`, `FastDeployPpocrArtifacts`, and `OnnxRuntimeArtifacts` so real provider libraries, OCR runtime dependency libraries, PPOCR model/data paths, ONNX model paths, labels, CPU-only execution, and default timeouts have an explicit contract before any real binary/model is attached.
- `VisionProviderArtifactManifest` can be loaded from JSON and now exposes fail-loud backend section requirements plus whole-manifest existing-file validation.
- `resources/vision-provider-artifacts.example.json` documents the expected local artifact layout without bundling binaries, models, OCR data, or upstream source.
- The route decision is `ffi_boundary_then_fastdeploy_ppocr_and_onnxruntime`.
- OCR is routed toward FastDeploy/PPOCR.
- NN is routed toward ONNXRuntime.
- GPU and DirectML are disabled for this route to avoid the known ONNX-GPU/DirectML lifetime risk described in the task file.
- `UnavailableOcrBackend` and `UnavailableNnBackend` fail loudly when the real engines are not linked or configured.
- `FastDeployPpocrBackend` dynamically loads an OCR provider exporting `ac_fastdeploy_ppocr_read_text_json`.
- `OnnxRuntimeBackend` dynamically loads an NN provider exporting `ac_onnxruntime_classify_json`.
- Both backends require a paired `ac_vision_free_buffer` symbol and treat missing libraries, missing symbols, malformed buffers, non-zero provider status, empty responses, and invalid JSON as fatal errors.
- `from_artifacts` constructors require reviewed local artifact paths to exist before loading a provider; missing provider libraries, models, dictionaries, or label files fail loudly.
- Artifact-backed FFI calls send explicit JSON envelopes containing both the inference request and the reviewed artifact contract. Plain `from_library_path` keeps the earlier raw request ABI for compatibility.
- `from_manifest` constructors let real OCR/NN backends be created from the reviewed manifest once artifact files are supplied.
- Unit tests cover the required `ocr_reads_text_from_frame` and `nn_classifies_frame` acceptance names through ABI-compatible test functions.
- Unit tests also cover artifact contract validation, CPU-only NN artifact configuration, missing artifact fatal errors, and artifact-envelope OCR/NN invocation.
- `apps/vision-provider-check` provides a small Runtime-owned CLI entry for validating the provider manifest and, with `--require-existing`, failing loudly when reviewed local provider/model/data artifacts are missing.
- `apps/vision-provider-check` also exposes real-provider smoke entry points through `--ocr-frame <png>` and `--nn-frame <png>` so reviewed FastDeploy/PPOCR and ONNXRuntime artifacts can produce actual OCR/NN JSON output once they are supplied; these smoke paths load real providers from the manifest and fail loudly while artifacts are absent.
- `apps/vision-provider-check --artifact-lock` can compute artifact sizes and SHA-256 hashes from a reviewed manifest, giving the release/NOTICE review a reproducible size and provenance report without committing provider binaries, models, OCR data, or upstream source.
- FastDeploy/PPOCR artifact locks now include configured `runtime_library_paths`, so OCR dependency DLLs are hashed and reviewed together with the provider library, detector model, recognizer model, dictionary, and optional classifier model.
- `apps/vision-provider-check --export-audit` can parse a PE export table without loading the DLL, allowing raw runtime libraries to be checked for the ActingCommand provider symbols before they are mistaken for providers.
- `apps/vision-provider-check --abi-check` now verifies that reviewed provider libraries load and export the ActingCommand JSON ABI symbols before any OCR/NN smoke run is trusted.
- The ABI check requires selected backend artifacts to exist first and fails loudly for missing files, invalid dynamic libraries, or libraries that do not export the required `ac_*` symbols.
- `OnnxRuntimeArtifacts` now distinguishes the ActingCommand JSON provider library from the reviewed ONNX Runtime dynamic library through `provider_library_path` and optional `runtime_library_path`.
- `providers/onnxruntime-json` is the first Runtime-owned ONNXRuntime JSON-ABI provider crate. It builds a cdylib exporting `ac_onnxruntime_classify_json` and `ac_vision_free_buffer`.
- The provider uses the `ort` Rust wrapper with default features disabled and `load-dynamic`/CPU-only runtime loading. It does not enable `download-binaries`, `copy-dylibs`, CUDA, DirectML, or other GPU execution providers.
- The provider requires `runtime_library_path` during real invocation, validates reviewed artifact paths, loads the ONNX model, converts RGB/RGBA/gray frames to f32 NHWC or NCHW tensors only when the model input shape is compatible, and fails loudly for missing runtime libraries, unsupported model input shapes, non-f32 outputs, output/label count mismatches, or non-finite scores.
- `providers/ppocr-onnx-json` is the first Runtime-owned PPOCR OCR JSON-ABI provider crate. It builds a cdylib exporting `ac_fastdeploy_ppocr_read_text_json` and `ac_vision_free_buffer`.
- The PPOCR provider uses dynamic CPU ONNXRuntime loading and reviewed PPOCR recognizer artifacts behind the existing `FastDeployPpocrArtifacts` contract.
- The PPOCR provider now runs detector-plus-recognizer full-frame OCR when the OCR request region is the full frame.
- Sub-frame OCR requests still use recognizer-only ROI OCR and return an explicit warning for that path.
- A local-only real R3 NN smoke passed with reviewed local paths for ONNXRuntime `v1.24.4` CPU x64, the Runtime-built ONNXRuntime JSON provider DLL, and an ONNX Models SqueezeNet `Opset16` model. The smoke used `apps/vision-provider-check --nn-frame`, returned `ok: true`, frame `224x224 rgb8`, top label `class_623`, and top score `6.899109363555908`.
- A local-only real R1 ROI OCR smoke passed with reviewed local paths for the Runtime-built PPOCR ONNX JSON provider DLL, ONNXRuntime CPU x64, MAA release `PaddleCharOCR` recognizer ONNX model, and dictionary. The smoke used `apps/vision-provider-check --ocr-frame --ocr-region`, returned `ok: true`, frame `320x80 rgb8`, text `ABC123`, and confidence `0.9997550845146179`.
- A local-only real R1 full-frame OCR smoke passed with reviewed local paths for the Runtime-built PPOCR ONNX JSON provider DLL, ONNXRuntime CPU x64, MAA release `PaddleCharOCR` detector and recognizer ONNX models, and dictionary. The smoke used `apps/vision-provider-check --ocr-frame` without `--ocr-region`, returned `ok: true`, frame `320x80 rgb8`, one detected text block, text `ABC123`, and confidence `0.9998682141304016`.
- `benchmarks/reports/2026-07-02-r1-r3-ffi-boundary.md` records the boundary decision, size estimate, and redistribution boundary.
- `benchmarks/reports/2026-07-02-r1-maa-ocr-artifact-audit.md` records the local-only MAA `v6.13.0` OCR artifact audit, including `fastdeploy_ppocr_maa.dll`, `MaaCore.dll`, PaddleOCR/PaddleCharOCR ONNX model files, dictionaries, sizes, hashes, and the release-packaging boundary.
- `benchmarks/reports/2026-07-02-r1-maa-provider-export-audit.md` records that `fastdeploy_ppocr_maa.dll` exposes MSVC C++ FastDeploy/PPOCR symbols and `MaaCore.dll` exposes MAA task-level C APIs; neither exports the ActingCommand OCR provider ABI.
- `benchmarks/reports/2026-07-02-r1-ppocr-onnx-roi-smoke.md` records the local-only real PPOCR ROI recognizer smoke, artifact sizes, commands, result, limitation, and release-packaging boundary.
- `benchmarks/reports/2026-07-02-r1-ppocr-onnx-full-frame-smoke.md` records the local-only real PPOCR detector-plus-recognizer full-frame smoke, artifact sizes, commands, result, and release-packaging boundary.
- `benchmarks/reports/2026-07-02-r3-onnxruntime-real-smoke.md` records the local-only real ONNXRuntime smoke, artifact sizes, commands, and release-packaging boundary.
- `resources/upstream-manifest.toml` now records current repository LICENSE verification through GitHub API for FastDeploy/PaddleOCR/ONNXRuntime, while keeping binary provenance, model/data terms, third-party notices, and redistribution review as release blockers.

This increment does not bundle FastDeploy, PPOCR, ONNXRuntime runtime binaries, MAA release binaries, models, OCR data, upstream source code, UI, SQLite, scheduler behavior, device access, game logic, or release-ready provider binaries. The ONNXRuntime and PPOCR provider crates are source-only; the real NN smoke, real OCR smokes, and MAA OCR artifact/export audits used ignored local artifacts and do not grant redistribution rights. R1 now has real detector-plus-recognizer full-frame OCR output through the ActingCommand provider ABI. Release packaging remains blocked until exact ONNXRuntime/PPOCR model/dictionary license texts, third-party notices, binary provenance, and redistribution obligations are recorded for any bundled artifacts.

## Current P6.5-A MaaFramework fusion chain E

The 2026-07-02 P6.5-A E FeatureMatch pre-research gate is closed as a benchmark decision report, not as a recognition backend implementation.

Scope:

- `benchmarks/reports/2026-07-02-feature-match-gate.md` records the FeatureMatch gate review.
- The three ActingCommand resource repositories were mirrored before the review.
- The current `actingcommand-recognition` dependency surface was checked and remains `image` plus `imageproc`.
- No current Runtime code or dependency exposes SIFT, ORB, AKAZE, RANSAC, OpenCV, or a feature-descriptor matcher.
- The refreshed resource repositories do not yet provide a curated real-frame FeatureMatch benchmark corpus with ground-truth cross-resolution matches.
- Pure Rust FeatureMatch is not accepted at this gate.
- FeatureMatch is routed to the R-class FFI decision lane; no FeatureMatch primitive is exposed as passing.

E is complete as a pre-research gate. It adds no production dependencies, no recognition hot-path code, no OCR, no NN, no UI, no SQLite, no scheduler behavior, no device access, no game logic, and no upstream source copying.

## Current P6.5-A MaaFramework fusion chain A4

The 2026-07-02 P6.5-A A4 replay unit is implemented as a clean-room Rust JSON-line input record and replay layer in `crates/device`.

Scope:

- `crates/device/src/replay.rs` defines recorded input actions and JSON-line input events.
- `RecordingInputBackend` wraps an explicit `InputBackend` and records successful input actions as JSON-line-compatible events.
- `parse_replay_json_lines` and `write_replay_json_lines` handle replay stream boundaries with fatal errors for empty or malformed streams.
- `replay_input_records` reads recorded actions and executes them against a supplied backend in order.
- Replay does not select fallback backends, retry, reconnect, capture frames, run recognition, open UI, write SQLite, or add game logic.
- Backend replay failures are promoted to fatal replay errors so replay cannot silently proceed after a missed input action.

A4 is complete as a device input record/replay slice. It remains limited to device input record/replay plumbing and does not add OCR, NN, FeatureMatch, live recovery wiring, resources, scheduler behavior, UI, upstream source copying, or new CLI surface.

## Current P6.5-A MaaFramework fusion chain O1

The 2026-07-02 P6.5-A O1 ProjectInterface unit is implemented as a declarative assembly contract in ActingLab.

Scope:

- `apps/actinglab/src/project_interface.rs` defines `ProjectInterface`, options, presets, operation specs, recognition specs, selections, and runnable configs.
- `assemble_runnable_config` merges option defaults, preset options, and user selections with validation.
- Presets can select an operation and recognition resources.
- User selections can override the operation and append recognition resources.
- Missing operations, missing recognition refs, unknown options, invalid option values, and malformed interface declarations fail visibly.

O1 is complete as an assembly-contract slice. It does not read resources, execute operations, run recognition, open devices, touch scheduler state, add UI, SQLite, OCR, NN, live recovery, game logic, upstream source copying, or a new CLI surface.

## Current P6.5-A MaaFramework fusion chain B

The 2026-07-02 P6.5-A Phase 2 B recovery executor unit is implemented as a pure declarative recovery graph executor.

Scope:

- `apps/actinglab/src/recovery_exec.rs` defines `RecoveryGraph`, `RecoveryNode`, `RecoverySignal`, `DetectKind`, and `RecoveryAction`.
- Recovery nodes support `next`, `on_error`, `save_on_error`, `reco_timeout`, and `action_escalation`.
- `execute_recovery_graph` follows the MAA-style run result branch shape: detect/action success follows `next`, failure follows `on_error`, and missing `on_error` fails visibly.
- `wait_freezes` is represented as an explicit recovery action primitive and is delegated to the caller-provided `RecoveryRuntime`.
- `max_attempts` and `max_node_visits` bound recovery execution and report max-attempt or loop-detected status instead of retrying indefinitely.
- The executor is device-independent and testable with a fake runtime.

B is complete as the first pure executor slice. It does not add live recovery execution, capture, touch execution, app restart execution, OCR, NN, replay, ProjectInterface, UI, SQLite, resource repository writes, game logic, upstream source copying, or a new CLI surface.

## Current P6.5-A MaaFramework fusion chain A3

The 2026-07-02 P6.5-A A3 device discovery unit is implemented as clean-room Rust process-metadata discovery for MuMu devices.

Scope:

- `crates/device` now exposes `DiscoveredDevice` and `DeviceDiscoveryProcess`.
- `discover_devices()` enumerates Windows process metadata and derives running MuMu device serials without calling ADB device-listing or server-control commands, returning `DeviceDiscoveryReport` so diagnostics stay visible to callers.
- MuMu serials are inferred from `MuMuNxDevice.exe` instance ids with the known localhost port pattern.
- MuMu ADB paths are inferred from the running MuMu device process path, keeping discovery on MuMu-provided ADB instead of PATH ADB.
- A pure `discover_mumu_devices_from_processes` report path supports deterministic tests with mocked process snapshots and preserves diagnostics for skipped processes.

A3 is complete and remains limited to discovery metadata. It does not add app lifecycle control, capture, touch execution, recovery execution, OCR, NN, replay, ProjectInterface, UI, SQLite, resources, game logic, upstream source copying, or ADB server mutation.

## Current P6.5-A MaaFramework fusion chain A1.1

The 2026-07-02 P6.5-A A1.1 Minitouch unit is implemented as clean-room Rust support for the public minitouch text protocol.

Scope:

- `MinitouchBackend` is added as an optional input backend behind the existing `InputBackend` trait.
- The default touch priority chain becomes `MaaTouch -> Minitouch -> adb_shell_input`.
- `TouchBackendChoice` accepts explicit `minitouch` selection.
- Minitouch uses a local operator-provided binary path and does not vendor or commit minitouch binaries.
- Minitouch transport, startup, handshake, push/chmod, process, stdin, and flush failures are transient fallback-eligible errors.
- Minitouch input validation, default pressure validation, and invalid coordinate mapping remain fatal.
- Minitouch maps screen-space coordinates into minitouch raw coordinates with scale and basic orientation handling.
- NOTICE records the upstream minitouch source and Apache-2.0 license reference without importing upstream source code.

A1.1 is complete and remains limited to input backend selection and minitouch protocol plumbing. It does not add recovery execution, OCR, NN, replay, ProjectInterface, UI, SQLite, resources, game logic, upstream source copying, or new vendored binaries. The only capture-area touch in this unit is a test-only cache guard added after full device tests exposed the previous A2 cache test as parallel-order sensitive.

## Current P6.5-A MaaFramework fusion chain A2

The 2026-07-02 P6.5-A A2 capture autotune unit is implemented as clean-room Rust capture-selection and freshness behavior.

Scope:

- `CaptureBackendChoice` now includes explicit `auto-fastest` selection.
- Auto capture probing records per-backend availability, elapsed time, cache status, and diagnostics.
- Auto capture probe results are cached by serial, adb path, and backend with a bounded TTL so repeated backend creation does not probe every backend every time.
- `auto` keeps priority order: `nemu_ipc -> droidcast_raw -> adb_screencap`.
- `auto-fastest` probes or uses cached probe results across the same bounded backend set and selects the lowest elapsed successful backend.
- `CaptureBackendName` reserves ADB-side `adb_screencap_encode` and `adb_screencap_raw_gzip` names for the ADB encode/raw-gzip lane; these modes are not active capture implementations in this unit.
- Capture freshness classification is context-aware: static unchanged frames are not treated as stale unless the caller explicitly expects a change.
- Expected-change freshness checks still classify unchanged consecutive frames as stale and report visible diagnostics. Startup/request-level fresh probing is supported; persistent runtime backend switching is deferred.
- Capture diagnose preserves `--require-fresh` through session request routing.

A2 is complete and remains limited to capture autotune/freshness plumbing. It does not add Minitouch, recovery execution, OCR, NN, replay, ProjectInterface, UI, SQLite, resources, game logic, upstream source copying, or new external binaries.

## Current P6.5-A MaaFramework fusion chain P0

The 2026-07-02 P6.5-A MaaFramework fusion chain is being implemented as clean-room Rust behavior and protocol work.

P0 refines the existing A1 touch fallback milestone instead of expanding to Minitouch, capture autotune, recovery, OCR, NN, replay, ProjectInterface, UI, SQLite, resources, or game logic.

Scope:

- Device errors now distinguish `Transient` from `Fatal`.
- Runtime touch fallback is allowed only for transient backend/transport failures.
- Fatal validation or configuration errors do not fall back and stay fail-loud.
- MaaTouch transport-class failures such as device availability, install/chmod, app_process start, handshake, process exit, command write, and flush are classified as transient where they are eligible for fallback.
- MaaTouch input validation failures remain fatal.
- Shared touch coordinate validation runs before dispatch so `adb shell input` cannot bypass stricter MaaTouch coordinate bounds.
- Touch diagnostics include attempt id, action, original backend, error reason, fallback backend, elapsed time, selection state, and WARNING-level fallback context.

P0, A2, A1.1, A3, the first B recovery executor slice, E FeatureMatch gate, O1 ProjectInterface, and A4 replay are the completed units in the larger P6.5-A chain. Later chain work remains separate:

- Live recovery wiring beyond the pure executor.
- Release packaging review for any future bundled OCR/NN binaries, models, dictionaries, or labels.

## Current P6.5-A1 device input fallback

The 2026-07-02 P6.5-A1 device input fallback from baseline `04ca117` is implemented as a clean-room Rust device-control milestone.

Scope:

- Touch fallback chain is `MaaTouch -> adb shell input`.
- `adb shell input` is implemented only for the touch subset: `tap`, `long_tap`, and `swipe`.
- `key` and `text` remain out of A1 fallback semantics; they fail loudly if the selected fallback backend cannot support them.
- Minitouch remains deferred to A1.1 pending binary/source/license decisions.
- No UI, OCR, SQLite, game logic, capture backend work, resource repository reads, upstream code copying, reconnect loop, or new external binary was added.

Completed A1 work:

- `crates/device` now exposes `TouchBackendChoice`, `TouchBackendConfig`, `SelectedTouchBackend`, `AdbShellInputBackend`, structured `TouchBackendDiagnostics`, and `touch_probe_report`.
- Default touch selection uses fixed priority `MaaTouch -> adb shell input`; `auto-fastest` is available as an explicit manual choice.
- Single-backend transient failures are recorded in structured attempts/warnings and then fall back to the next backend.
- Fatal touch validation/configuration errors do not fall back.
- Full-chain touch failure returns a fatal error containing backend attempts and warnings.
- `apps/actinglab` direct touch, stream input relay, semantic tap/navigation, and Lab run device input now use the shared selector instead of constructing `MaaTouchBackend` directly.
- ActingLab Session Layer routed requests preserve the global touch backend choice so daemon-routed control commands use the same selector request as local commands.
- `apps/device-test` direct touch/reset, benchmark control submission, and probe-run input now use the shared selector.
- `touch-probe` exposes an input-side probe report without sending tap/swipe actions.
- `--touch-backend <auto|auto-fastest|maatouch|adb_shell_input>` is available for Runtime CLI entry points that resolve device config.

## Current full-validation rerun

The 2026-07-02 full Runtime validation rerun rechecked the old B0/B1 "one failure amplified into 109 failures" concern against the current `main` code path.

The rerun confirmed that local `main` matched `origin/main` at `0d6f4ab` before the cleanup, GitHub CI run `28533744122` was green for that remote state, and the workspace passes format, dev clippy, release clippy, release build, the targeted B0-adjacent session-events test, and full `cargo test --workspace --no-fail-fast` locally.

The old 109-failure event was the documented B0/B1 cascade from ambient local config plus poisoned `ENV_LOCK`, not a current Runtime failure. During stricter release validation, `cargo clippy --workspace --release -- -D warnings` found a release-only dead-code warning on the debug crash-injection environment constant. The fix gates that constant behind `#[cfg(debug_assertions)]`, preserving the debug-only crash-injection behavior while keeping release validation warning-clean.

## Current Session Layer D6 reliability close-out

The 2026-07-02 D6 reliability close-out from baseline `fb00856` is implemented in the Rust `actinglab` code path.

This close-out intentionally downgrades D6 from same-user anti-forgery authentication to local runtime reliability. The Session Layer now records that `state_dir` and loopback endpoint write access are part of the trusted same-user local environment. Same-user forged state or a fake endpoint that can echo the daemon id remains an accepted risk for the current phase; authentication, key material, memory protection, and remote-control security are deferred to the trusted-channel/scheduler/UI lane.

Completed D6 reliability work:

- Request routing now requires an early daemon acknowledgement before returning success. Acknowledgement is defined as either a `running/` request marker or a response file.
- `--request-ack-timeout-ms` bounds the acknowledgement wait, and unacknowledged requests fail fast as `runtime_not_running` instead of hanging until the full response timeout.
- Unacknowledged routed requests are removed from `requests/` to avoid leaving stale work for a future daemon.
- Daemon liveness now records and verifies a process creation key when the platform supports it, closing the PID reuse gap.
- Windows uses `GetProcessTimes` as a hard creation-time check. Linux uses `/proc/<pid>/stat` starttime. Unsupported platforms report an explicit degraded PID-reuse protection status instead of silently claiming full protection.
- `session contract`, `session api`, and readiness policy output now document the local reliability threat model and the deferred trusted-channel security boundary.
- Offline coverage includes stale/dead/mismatched liveness, PID creation-time mismatch, fake endpoint/no-request-processing fail-fast, real daemon no-wait acknowledgement, and updated route-preference tests.

No secret challenge, HMAC, ed25519, memory encryption, UI, OCR, SQLite, game logic, resource repository reads, upstream source copying, ADB input fallback, reconnect loop, or live device work was added in this D6 close-out.

## Current Session Layer true-acceptance close-out

The 2026-07-01 Session Layer acceptance close-out from baseline `0925c2a` is implemented in the Rust `actinglab` code path.

Completed close-out work:

- P0/D6: resident daemon readiness now requires a live daemon-owned local liveness endpoint in addition to pid, heartbeat, and daemon-id state. Two forged JSON state files plus a live non-daemon PID can no longer produce `ready=true`.
- P1.1: a real child-process daemon lifecycle test starts the daemon, observes readiness, kills it non-gracefully, and verifies readiness falls back to not-ready.
- P1.2/D3: child-process crash injection covers response-written, journal-appended, and request-removed windows, then restarts the daemon and verifies no duplicate execution, no pending request, no stale running marker, one journal entry, and a preserved response.
- P1.3: adversarial coverage includes forged state identity, path traversal and directory-alias artifact escapes, outside artifact sources, wrong/preempted lease control attempts, and untrusted transport checks.
- P1.4: `CONFIG_ENV` mutation in tests is sealed behind fixture helpers; the no-config path points to a missing temp config instead of removing the env var, and a source guard prevents direct `remove_var` or new bare `set_var(CONFIG_ENV)` usage.
- P1.5: GitHub Actions CI has been added for `cargo fmt --all -- --check`, `cargo clippy --workspace -- -D warnings`, and `cargo test --workspace`.
- P2/D4: daemon startup cleans stale JSON temp files only when the file name matches the tmp pattern, the owner PID is not alive, and the temp file is older than the configured threshold.
- P2/D5/D7: record artifact directory creation performs a canonical containment recheck after `create_dir_all`, with Windows final-path normalization covering 8.3 short-path components before safety prefix comparison.
- P2/D9: corrupt journal lines are counted and surfaced in status diagnostics and recommended actions instead of being silently treated as healthy.

No UI, OCR, SQLite, game logic, resource repository reads, live device work, upstream source copying, ADB input fallback, reconnect loop, or broad runtime redesign was added in this close-out.

## Current Session Layer Round 2 close-out

The 2026-07-01 Session Layer Round 2 fix from baseline `6151553` is implemented in the Rust `actinglab` code path.

Completed Round 2 fixes:

- D6: resident daemon liveness now requires daemon identity binding, not only a live PID. `SessionInfo` and `SessionHeartbeat` carry a shared daemon id, diagnostics expose identity presence/match state, and readiness/session-info routing rejects a live non-daemon PID with mismatched or missing identity.
- D5/D7: artifact directories and artifact source paths now pass canonical containment checks that resolve symlink/junction/alias escapes while still rejecting absolute/outside inputs before artifact use.
- D9: request-journal reads skip corrupt or truncated JSONL lines, count skipped lines, and expose the count in journal/status diagnostics while journal writes still fail loudly.
- D4: atomic JSON writes clean only current-process orphan temp files before publish and remove the current temp file on publish failure.
- D3/D9: request processing preserves the queued request when response/journal/remove ordering fails before safe cleanup.
- D1: no-endpoint local connect-plan behavior is covered by regression tests and remains safe because unchecked local/no-endpoint transport does not block client preflight.

No resource repositories were used in this Round 2 fix, and no UI, OCR, SQLite, game logic, upstream source copying, live device validation, fallback path, reconnect path, or transport listener was added.

## Current Session Layer audit close-out

The 2026-07-01 Session Layer audit fix from baseline `cf9095a01606654c42e6202e281260b5d5d21de9` is implemented in the Rust `actinglab` code path.

Completed fixes:

- B1: test environment lock poisoning now recovers instead of cascading one panic into many false failures.
- B0/D6: session liveness checks now require a fresh heartbeat and a live process before readiness or daemon-preferred routing can be accepted.
- D2: preempted leases no longer authorize control requests; release identity checks remain separate so cleanup can still proceed.
- D8: local `stream` input relay now requires a matching lease unless the command is already executing inside the resident daemon request handler.
- D4/D9: session JSON files and JSONL journal lines are written through complete in-memory JSON payloads, unique temporary file names, flush, and sync.
- D3: daemon request journal entries are made durable before request files are removed.
- D5/D7: record artifact directories and build/promote artifact source paths must stay inside the Session Layer state/record boundary.
- D1: checked transport records missing `safe_to_connect` are treated as unsafe; only explicitly unchecked local/no-endpoint transport remains allowed.

No resource repositories were used in this audit fix, and no UI, OCR, SQLite, game logic, upstream source copying, or device live validation was added.

## Current completed milestones

- P6.5-A1 device input fallback: touch input now uses a structured `MaaTouch -> adb shell input` selector with `auto`, `auto-fastest`, explicit backend selection, fallback diagnostics, and CLI routing through the shared selector.
- ActingLab Session Layer D6 reliability close-out: request acknowledgement fail-fast, PID creation-time verification, explicit unsupported-platform PID-reuse degradation, and documented same-user forgery accepted risk for deferred trusted-channel work.
- ActingLab Session Layer Round 2 close-out: daemon identity-bound liveness, canonical artifact containment, corrupt-journal skip diagnostics, orphan temp cleanup, request-processing failure-window regression coverage, and no-endpoint connect-plan regression coverage.
- ActingLab Session Layer audit close-out: liveness/process readiness, lease preemption, local stream relay lease gating, durable request journaling, atomic JSON writes, artifact path containment, and safe transport defaults.
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
- ActingLab session recording anchor self-backtest: frame-backed anchor crops now run an offline source-frame template self-test and persist score, threshold, metric, match point, and pass/fail evaluation metadata.
- ActingLab session recording build-task draft: authorized, backtested recording steps can now assemble a local draft operation bundle and asset directory without device I/O, resource writes, UI, SQLite, or game logic.
- ActingLab session recording anchor contrast validation: frame-backed anchor crops can now be checked against an explicit contrast frame so ambiguous anchors fail before draft bundle generation.
- ActingLab session recording package handoff: generated draft bundles now satisfy the existing `package build-task` dry-run validation path, with early page-anchor and click-bound checks in `record build-task`.
- ActingLab session recording current-frame inlet: authorized anchor steps can now use `--capture` / `--current-frame` to materialize a source frame through the existing CaptureBackend path, persist provenance/freshness metadata, and reuse the same crop/backtest path as local PNG frames.
- ActingLab session recording amend re-backtest loop: frame-backed anchor amendments now immediately re-crop, re-materialize, and re-run self/contrast backtests from the recorded source frame instead of degrading to deferred evaluation.
- ActingLab session recording auto-region candidate slice: frame-backed `--region auto` anchor steps now deterministically resolve to a local rect candidate, materialize an artifact, and reuse the existing self/contrast backtest path while no-frame auto anchors remain deferred.
- ActingLab session recording auto-region candidate report: frame-backed `--region auto` now records candidate regions, luma variance, contrast scores, selected reason, and prefers candidates rejected by the contrast frame before final artifact/backtest evaluation.
- ActingLab session recording amend-by-candidate loop: `session record amend` can now choose a previously reported auto-region candidate by index, preserve operator-selection provenance, and immediately re-run artifact/backtest evaluation.
- ActingLab session recording candidate preview: `session record candidates` can now list an anchor step's recorded auto-region candidates for operator review before amend/build.
- ActingLab session recording resource promotion: `session record promote` can now publish a validated recording draft into an existing resource repository/root with overwrite protection and package dry-run compatibility.
- ActingLab session recording anchor color-check output: frame-backed anchors recorded with `--color-check` now emit bundle `color_check` data derived from the authorized source-frame region instead of only storing the request as provenance.
- ActingLab session recording standalone color-probe output: `session record step --kind color-probe` can now sample an authorized frame region into `color_probes[]`, and `resource convert` emits those probes as recognition-pack `type=color` targets.
- ActingLab session recording standalone verify-template output: `session record step --kind verify-template` can now materialize an authorized template crop into `verify_templates[]`, and `resource convert` emits those templates as recognition-pack template targets.
- ActingLab session recording color-probe/verify-template amend loop: `session record amend` can now correct standalone color-probe and verify-template steps, recompute authorized frame-backed data, and keep metadata-only steps visibly deferred.
- ActingLab session recording top-level CLI contract alias: `record ...` now routes to the same implementation as `session record ...`, matching the Session Layer interface draft while preserving existing behavior.
- ActingLab session recording build-task capability close-out: top-level and session-scoped record capabilities now advertise `build-task`, and top-level `record build-task` is covered by route tests.
- ActingLab session interface surface alignment: record lifecycle capabilities now advertise start/status/stop, and the future interactive `stream` command is explicitly reserved instead of being an unknown command.
- ActingLab daemon app lifecycle routing: `session app launch|stop|restart` can now be submitted through the resident daemon request queue with lease metadata, and daemon-side app requests are lease-gated before device I/O.
- ActingLab daemon instance lifecycle routing: `session instance list|health|reconnect` can now be submitted through the resident daemon request queue, with `reconnect` lease-gated before device I/O.
- ActingLab daemon capture routing: `capture --via-daemon` and `session request capture` can now submit normal one-shot capture through the resident daemon request queue while preserving `--out` and freshness flags.
- ActingLab daemon Lab run routing: `lab run --via-daemon` and `session request lab-run` can now submit trusted Lab package execution through the resident daemon request queue, with daemon-side lease validation before zip or device I/O.
- ActingLab daemon package/operation run routing: `package run --via-daemon`, `operation run --via-daemon`, `session request package-run`, and `session request operation-run` now submit through the resident daemon request queue with daemon-side lease validation before package, operation, or device I/O.
- ActingLab bounded stream scaffold: `stream --max-frames N` now exposes a local bounded frame-sampling contract, `stream --via-daemon` and `session request stream` route through the resident Session Layer request queue, and interactive input relay remains explicitly reserved.
- ActingLab daemon request journal: processed resident daemon requests now append a durable JSONL journal, and `session journal` exposes recent request outcomes for diagnostics after response files are consumed.
- ActingLab session status diagnostics: `session status --diagnostics` now reports queue depths, daemon state paths, journal totals, recent request summary, and latest request error for UI/scheduler health inspection.
- ActingLab request journal retention: daemon request journals now rotate the active JSONL file into a single local archive when it exceeds the fixed retention cap, and diagnostics expose the active/archive byte counts and policy.
- ActingLab daemon-routed status diagnostics: `session request status --diagnostics` can now return the same status and diagnostics payload through the resident daemon request queue for future UI/API consumption.
- ActingLab top-level daemon-routed status entry: `status --via-daemon` now submits the top-level status diagnostic through the resident daemon request queue while bare `status` keeps its existing local runtime probe behavior.
- ActingLab session diagnostics daemon routing: `session status --via-daemon` and `session journal --via-daemon` now route local session diagnostics through the resident daemon request queue while preserving their offline local forms.
- ActingLab daemon-preferred read-only routing: when session info indicates a resident daemon is running, read-only/diagnostic CLI entries now prefer the daemon request queue by default and use `--local` as the explicit local override.
- ActingLab daemon-routed journal diagnostics: `session request journal [--limit]` can now return recent request-journal entries through the resident daemon request queue for future UI/API consumption.
- ActingLab daemon-routed lease interface: `session request lease acquire|release|preempt|status` can now run through the resident daemon request queue, using the daemon state directory and preserving lease holder/id command arguments.
- ActingLab daemon-routed recording interface: `session request record start|status|stop|...` can now run through the resident daemon request queue, using the daemon state directory and preserving holder/lease provenance command arguments.
- ActingLab daemon-routed devices diagnostics: `devices --via-daemon` and `session request devices` can now submit device enumeration through the resident daemon request queue instead of requiring the caller to run the ADB listing directly.
- ActingLab daemon-preferred control routing: when session info indicates a resident daemon is running, direct touch/input and semantic control entries prefer the daemon request queue by default while daemon-side handlers force local execution to avoid recursive requeue.
- ActingLab daemon-preferred lifecycle and run routing: monitor, instance lifecycle diagnostics/reconnect, app lifecycle, Lab run, package run, and operation run now prefer the resident daemon request queue when session info exists.
- ActingLab manual lease run UX: `session lease run -- <command...>` now acquires a temporary local lease, submits a daemon request with lease metadata, and releases the lease after success or failure.
- ActingLab session lease diagnostics: `session status --diagnostics` now reports active lease files for UI/scheduler visibility, and corrupt lease state fails visibly.
- ActingLab LabLease aliases: `lab status`, `lab lease`, and `lab release` now expose the Lab-facing lease/status surface as thin aliases over the Session Layer status and lease files.
- ActingLab LabLease preempt alias: `lab preempt` now exposes the Session Layer preempt path from the Lab-facing CLI surface and preserves previous-holder provenance.
- ActingLab LabLease status alias: `lab lease status` now exposes the same Session Layer lease status file from the Lab-facing CLI surface.
- ActingLab bounded stream input relay scaffold: `stream --input-relay <tap|swipe|long-tap|key|text>` can now carry one input action through the bounded local stream contract, and daemon-routed input relay requires a matching Session Layer lease.
- ActingLab bounded stream multi-event relay: repeated `--input-event <action,args>` can now carry multiple tap/swipe/long-tap/key/text events through one bounded stream request, with daemon-side lease enforcement unchanged.
- ActingLab stale capture recovery plan: `session recover --stale-capture` now exposes a read-only recovery plan that diagnoses stale frames and recommends capture-backend recovery before heavy app restart.
- ActingLab session liveness diagnostics: `session status --diagnostics` now classifies daemon heartbeat state for UI/scheduler health checks.
- ActingLab daemon liveness-gated routing: automatic daemon-preferred routing now requires an alive heartbeat, and explicit daemon requests fail fast before queueing when the daemon state is stale, missing a heartbeat, or pid-mismatched.
- ActingLab session start liveness gate: `session start` now treats stale or heartbeat-missing session state as a visible runtime error instead of reporting false `already_running`, and new daemon startup waits for an alive heartbeat before reporting `started`.
- ActingLab session stop liveness gate: `session stop` now refuses stale, missing-heartbeat, or pid-mismatched daemon state before writing a stop request, while alive daemon state keeps the existing stop path.
- ActingLab stale session cleanup: `session cleanup --stale` now provides an explicit local cleanup path for stale daemon state without touching devices, apps, journals, resources, or game logic.
- ActingLab session diagnostics recommended actions: `session status --diagnostics` now emits machine-readable next actions for stopped or stale daemon state so UI/scheduler consumers do not need to infer recovery commands from free text.
- ActingLab bounded stream contract envelope: `stream` now reports a `session.stream.v0.1` contract and ordered stream events so future UI/API clients can consume bounded frame streams and input relay status without scraping command-specific fields.
- ActingLab daemon-routed capabilities contract: `session request capabilities` now lets a running resident daemon report the same command list and a `session.capabilities.v0.1` Session Layer access/safety contract to future UI/API clients.
- ActingLab Session access contract: `session contract` and `session request contract` expose a machine-readable `session.access.v0.1` access boundary for local CLI and future trusted UI/API clients.
- ActingLab Session events view: `session events` and `session request events` expose daemon request journal outcomes as stable `session.events.v0.1` event data for future UI/API clients.
- ActingLab Session API contract: `session api` and `session request api` expose `session.api.v0.1`, documenting local CLI access, reserved trusted remote access, daemon request queue fields, CLI/event envelopes, command classes, and required failure codes.
- ActingLab Session events cursor: `session events` and `session request events` now support `--after-unix-ms` plus cursor metadata for incremental local CLI and future UI/API event consumption.
- ActingLab Session request-id event cursor: `session events` and `session request events` now support `--after-request-id` plus request-id cursor fields so future UI/API clients can continue event reads without losing same-millisecond events.
- ActingLab lease-gated daemon monitor recovery policy: daemon monitor policies can opt into maintenance recovery only when stored lease metadata matches the active Session Layer lease; daemon ticks persist recovery results or visible recovery errors in monitor state.
- ActingLab lease-deferred daemon monitor recovery coordination: daemon monitor recovery now defers visibly with `deferred_by_lease` when the active lease is missing or held by another client, so self-heal does not fight scheduler or Lab ownership.
- ActingLab monitor-policy lease recommendation surface: `session status --diagnostics` now translates `deferred_by_lease` monitor recovery into scheduler/UI-facing recommended actions for lease inspect, acquire, or preempt decisions without executing them.
- ActingLab Session events command filter: `session events` and `session request events` now support repeatable `--command <name>` filters so future UI/API clients can poll stream, lease, monitor, or control event slices without scanning the full request journal.
- ActingLab Session request data summary: daemon request journal events now retain compact stream response summaries so future UI/API clients can observe stream ids, frame counts, event counts, and input relay status from `session events` without reading full response files.
- ActingLab capture diagnosis event summaries: daemon request journal events now retain compact stale-capture and capture-diagnose summaries so future UI/scheduler clients can observe fresh-frame status and recommended capture-backend recovery without reading full response files.
- ActingLab data-summary event filter: `session events` and `session request events` now support repeatable `--data-summary-kind <kind>` filters so future UI/scheduler clients can poll stream, capture-diagnose, or stale-capture recovery slices directly.
- ActingLab request-status event filter: `session events` and `session request events` now support repeatable `--status completed|failed` filters so future UI/scheduler clients can poll success and failure slices without scraping the full journal.
- ActingLab target-scoped event stream: daemon request journal entries now preserve request target selectors, and `session events` / `session request events` can filter by instance/game/server selectors and repeatable lease holder.
- ActingLab target-scoped journal view: `session journal` and `session request journal` now reuse the Session event filter contract for command, data-summary, status, instance/game/server, and lease-holder diagnostics.
- ActingLab pending request diagnostics: `session status --diagnostics` now exposes a bounded pending-request preview for future UI/scheduler queue inspection, and corrupt pending request files fail visibly.
- ActingLab pending response diagnostics: `session status --diagnostics` now exposes a bounded pending-response preview for unconsumed daemon responses, and corrupt response files fail visibly.
- ActingLab session queue health diagnostics: `session status --diagnostics` now reports queue health across pending requests and unclaimed responses using the daemon request timeout threshold.
- ActingLab session response view: `session response get <request-id> [--consume]` and `session request response get <request-id> [--consume]` expose pending daemon response files as a stable response-consumption surface for UI/scheduler clients.
- ActingLab session request no-wait submit: `session request <command> --no-wait` now queues daemon requests and returns a request id plus response lookup commands without waiting for or consuming the daemon response.
- ActingLab session request-state view: `session request-state get <request-id>` and `session request request-state get <request-id>` summarize queued, response-available, completed, failed, and unknown daemon request lifecycle states for UI/scheduler clients.
- ActingLab session request-state list view: `session request-state list` and `session request request-state list` expose a bounded aggregate request lifecycle view across pending requests, pending responses, and recent request journal entries.
- ActingLab session response wait view: `session response wait <request-id>` and `session request response wait <request-id>` provide a bounded wait/read/consume surface for a specific daemon response without custom polling in UI/scheduler clients.
- ActingLab session events wait view: `session events wait` and `session request events wait` provide bounded long-polling over the request-journal event cursor for UI/scheduler clients without custom event polling loops.
- ActingLab session request cancel view: `session request cancel <request-id>` removes queued daemon requests that have not produced a response, records a durable `request_cancelled` journal failure, and exposes the result through request-state and event views.
- ActingLab session running request state view: resident daemon request processing now writes a `running/` marker while executing a request, and request-state/status diagnostics expose `running` lifecycle state between queued and response-available.
- ActingLab session request-state wait view: `session request-state wait <request-id>` and `session request request-state wait <request-id>` provide bounded lifecycle waiting over queued/running/response/journal request states for UI/scheduler clients.
- ActingLab session lease wait view: `session lease wait` and `session request lease wait` provide bounded waiting for free or held lease state, including holder and lease-id filters for scheduler/Lab/UI coordination.
- ActingLab session lease list view: `session lease list` and `session request lease list` expose all active Session Layer lease records with holder and lease-id filters for scheduler/Lab/UI arbitration diagnostics.
- ActingLab target-scoped request-state list: `session request-state list` and `session request request-state list` now filter by global instance/game/server selectors and repeatable `--lease-holder`, matching the journal/events target-slice model for UI/scheduler queue views.
- ActingLab LabLease list/wait aliases: `lab lease list` and `lab lease wait` now expose the same Session Layer lease-list and lease-wait views from the Lab-facing CLI surface.
- ActingLab session lease touch view: `session lease touch`, `session request lease touch`, and `lab lease touch` let current lease holders refresh lease freshness metadata without executing device work.
- ActingLab session lease freshness diagnostics: `session lease status`, `session lease list`, and `session status --diagnostics` now report lease freshness metadata for scheduler/UI visibility without reclaiming leases automatically.
- ActingLab stale lease recommended action surface: `session status --diagnostics` now emits `stale_lease_inspect` recommendations for stale leases, marked as scheduler decisions rather than automatic recovery.
- ActingLab capture health recommended action surface: `session status --diagnostics` now turns recent stale/unavailable capture journal summaries into read-only scheduler/UI recommendations before anyone treats a game as frozen.
- ActingLab queue health recommended action surface: `session status --diagnostics` now turns blocked queued/running requests and unclaimed responses into read-only inspect/read actions for UI/scheduler clients.
- ActingLab failed request recommended action surface: `session status --diagnostics` now turns the latest failed daemon journal entry into a read-only `failed_request_inspect` action for UI/scheduler clients.
- ActingLab trusted transport preflight surface: `session transport check --endpoint <url>` now exposes the existing local/trusted-remote endpoint policy as a machine-readable Session Layer preflight without starting a listener.
- ActingLab bounded stream preflight surface: `stream check` and `session request stream check` now expose a machine-readable safety preflight for bounded frame streams and per-request input relay without capturing frames, starting MaaTouch, or starting a listener.
- ActingLab session readiness surface: `session readiness` and `session request readiness` now aggregate daemon liveness, status diagnostics, optional transport endpoint checks, stream-preflight availability, blockers, and recommended actions for UI/scheduler consumers without device I/O.
- ActingLab command-check surface: `session command-check <command...>` and `session request command-check <command...>` now classify a target command, report daemon-route readiness and lease-gate status, and return `safe_to_submit` without enqueueing, capturing, starting MaaTouch, or touching devices.
- ActingLab command-check queue gate: `session command-check` now includes `queue_gate` and treats blocked daemon queues or unclaimed daemon responses as `safe_to_submit=false` before UI/scheduler clients enqueue more work into the Session Layer.
- ActingLab daemon request admission queue gate: actual `session request <command>` submissions now fail visibly with `request_queue_needs_attention` before writing a new pending request when daemon queue health needs attention, keeping the command-check preflight and real admission path aligned.
- ActingLab Session queue view: `session queue` and `session request queue` now expose `session.queue.v0.1` with queue counts, health, previews, admission status, and queue-specific recommended actions for UI/scheduler clients without enqueueing or touching devices.
- ActingLab submit-plan preflight view: `session submit-plan <command...>` and `session request submit-plan <command...>` now aggregate readiness, command-check, and queue admission into one no-device/no-enqueue submission plan for UI/scheduler clients.
- ActingLab stream input relay preflight contract: `session command-check stream --input-event <action,args>` and `--relay-event <action,args>` are locked as lease-gated control preflights while `stream check --input-event ...` remains a read-only preflight that does not enqueue, capture, start MaaTouch, or touch devices.
- ActingLab Phase C acceptance gates contract: `session phase-c-plan` now embeds `session.phase_c_acceptance_gates.v0.1`, separating offline-verifiable gates from live-device gates for self-heal, interaction flow, trusted channel, and live acceptance.
- ActingLab Phase C acceptance diagnostics summary: `session status --diagnostics`, `session bootstrap`, and `session readiness` now expose compact Phase C acceptance-gate status for UI/scheduler clients without requiring them to call the full `session phase-c-plan` payload first.
- ActingLab connect-plan Phase C preflight summary: `session connect-plan` now exposes `session.connect_phase_c_preflight.v0.1`, aggregating self-heal, interaction-flow, trusted-channel, and live-acceptance readiness for UI/scheduler clients before they open bounded streams or trusted remote channels.
- ActingLab control request admission gate: control-class `session request ... --no-wait` submissions are now lease-validated before queueing, so missing or mismatched LabLease metadata fails visibly and leaves no pending request file.
- ActingLab request cancellation lease gate: `session request cancel <request-id>` preserves read-only request cleanup while requiring matching lease metadata before cancelling a lease-gated queued control request.
- ActingLab blocked queue cancel recommendation: `session status --diagnostics` now distinguishes cancellable blocked queued requests from lease-gated blocked queued requests, marking cancel suggestions as non-device-touching scheduler decisions and exposing the request lease metadata.
- ActingLab request cancellation dry-run: `session request cancel <request-id> --dry-run` now performs the same queue-state and lease checks without deleting the queued request or writing a cancellation journal, giving UI/scheduler clients a safe preflight before queue mutation.
- ActingLab blocked queue cancel dry-run recommendation: `session status --diagnostics` now emits a non-mutating `blocked_request_cancel_dry_run` recommended action before queue-cancel recommendations, including lease metadata and runnable dry-run arguments for UI/scheduler preflight.
- ActingLab readiness queue summary: `session readiness` and `session request readiness` now expose top-level `queues` with queue counts, queue health, and blocked-request recommendation kinds so UI/scheduler clients do not need to parse the full embedded status diagnostics for queue state.
- ActingLab readiness instance summary: `session readiness` and `session request readiness` now expose top-level `instances` with registry availability, selected instance, missing required fields, and configured instance summaries for UI/scheduler consumers.
- ActingLab readiness selected-instance gate: `session readiness` and `session request readiness` now mark selected missing or incomplete instances as `ready=false` with explicit instance blockers, so UI/scheduler consumers do not submit work against unusable instance config.
- ActingLab command-check instance gate: `session command-check` and `session request command-check` now gate device-affecting commands on selected-instance configuration, and `session submit-plan` exposes a compact `preflight_summary` for UI/scheduler consumers.
- ActingLab live validation plan surface: `session validation-plan` and `session request validation-plan` now expose deferred live-validation policy, allowed offline checks, and no-device guarantees as machine-readable data for UI/scheduler clients.
- ActingLab client bootstrap surface: `session bootstrap` and `session request bootstrap` now aggregate API/access contracts, capabilities, readiness, queue, and validation-plan into one no-device startup envelope for UI/scheduler clients.
- ActingLab live validation acceptance matrix: `session validation-plan` now includes Phase A-D/cross-cutting acceptance boundaries and an AK stale-capture validation scope, keeping offline progress separate from `requires-live-device` acceptance.
- ActingLab pending live acceptance checklist: `session validation-plan` now includes a `pending_live_acceptance` block titled `待真机验收`, listing every skipped live/device/operator validation item and the evidence required before it can be marked passed.
- ActingLab unique Session throat-policy surface: `session throat-policy` and `session request throat-policy` expose a machine-readable policy that Session Layer is the only device/control throat, while UI, scheduler, and agents must not directly touch adb/devices.
- ActingLab command-check throat gate summary: `session command-check` now includes a compact `throat_gate`, and `session submit-plan` mirrors its key fields into `preflight_summary` so UI/scheduler clients can see whether a command requires the Session Layer, daemon routing, or a LabLease before submitting.
- ActingLab capture freshness policy surface: `session capture-policy` and `session request capture-policy` expose the fresh-frame/stale-capture policy from the AK stale screencap finding without touching devices or reading resources.
- ActingLab Phase C self-heal policy surface: `session self-heal-policy` and `session request self-heal-policy` expose the maintenance-only observe/diagnose/plan/execute recovery boundary without touching devices or reading resources.
- ActingLab command-check Phase C scope: `session command-check` now includes `phase_c_scope`, classifying self-heal, interaction-flow, trusted-channel, and live-validation relevance before clients submit or enqueue work.
- ActingLab command-check Phase C event summary: daemon request journals and `session events --data-summary-kind command_check` now expose compact command-check Phase C scope for UI/scheduler event consumers.
- ActingLab submit-plan Phase C event summary: daemon request journals and `session events --data-summary-kind submit_plan` now expose compact submit-plan admission and Phase C execution-preflight summaries for UI/scheduler event consumers.
- ActingLab Phase C plan event summary: daemon request journals and `session events --data-summary-kind phase_c_plan` now expose compact Phase C next-action lane summaries for UI/scheduler event consumers.
- ActingLab readiness event summary: daemon request journals and `session events --data-summary-kind readiness` now expose compact startup readiness, queue, instance, diagnostics, Phase C, and live-deferred fields for UI/scheduler event consumers.
- ActingLab bootstrap event summary: daemon request journals and `session events --data-summary-kind bootstrap` now expose the compact startup envelope state for UI/scheduler event consumers.
- ActingLab readiness client policy summary: `session readiness` now includes a compact no-device policy summary for UI, scheduler, and agent startup logic, covering Session throat, capture freshness, self-heal, stream, trusted transport, and deferred live validation.
- ActingLab client connect-plan preflight: `session connect-plan` and `session request connect-plan` aggregate readiness, trusted transport checks, and stream preflight into one no-device startup plan for UI, scheduler, and agent clients.
- ActingLab interactive stream-plan preflight: `session stream-plan` and `session request stream-plan` now expose a no-device Phase C stream startup plan that combines connect-plan, stream preflight, lease-gated input relay state, and reserved trusted-remote encrypted-channel status.
- ActingLab trusted-channel transport plan: `session transport plan` now exposes a no-listener/no-TCP-probe plan for local CLI, daemon file-IPC, and reserved encrypted trusted remote access.
- ActingLab Phase C self-heal plan preflight: `session self-heal-plan` and `session request self-heal-plan` expose a no-device maintenance recovery plan that turns observed triggers into lease/queue/readiness-gated recovery candidates without executing input.
- ActingLab Phase C self-heal lease-gate alignment: `session self-heal-plan` now makes the embedded `lease_gate` follow the selected recovery candidate, so stale-capture/read-only recovery reports `required=false` while standby/modal/unexpected-page/login/session-expired maintenance control remains lease-gated.
- ActingLab Phase C self-heal escalation preflight: `session self-heal-plan` now exposes escalation policy for repeated transient capture failures and maintenance-control loops without executing heavy recovery.
- ActingLab Phase C self-heal event summary: daemon request data summaries now carry self-heal escalation category, heavy-recovery candidate, no-execute guarantee, and operator/live-validation requirement for journal/events consumers.
- ActingLab Phase C self-heal escalation recommended action: `session status --diagnostics` now turns recent self-heal-plan escalation summaries into a read-only `self_heal_escalation_review` action for UI/scheduler clients.
- ActingLab interaction/trusted-channel diagnostics recommended actions: `session status --diagnostics` now turns recent `stream_plan` and `transport_plan` summaries into read-only `interactive_stream_preflight_review` and `trusted_channel_preflight_review` actions for UI/scheduler clients.
- ActingLab pending-live validation diagnostics summary: `session status --diagnostics` now exposes the `待真机验收` summary from `session validation-plan` for UI/scheduler clients without marking live checks passed.

- ActingLab Phase C aggregate plan surface: `session phase-c-plan` and `session request phase-c-plan` now expose one read-only roadmap for self-heal, interaction flow, trusted channel, and live-validation boundaries without requiring a live instance.
- ActingLab Phase C interaction-flow aggregate plan view: `session phase-c-plan` now embeds `session.phase_c_interaction_plan.v0.2`, making the interaction lane explicitly require an instance-scoped `session stream-plan` preflight before UI frame relay or input relay can start.
- ActingLab Phase C aggregate next-action summary: `session phase-c-plan` and `session request phase-c-plan` now expose a machine-readable `next_actions` roadmap for self-heal review, interaction-flow review, trusted-channel transport review, pending live acceptance, and status diagnostics.
- ActingLab Phase C plan diagnostics recommended action: `session status --diagnostics` now turns recent `phase_c_plan` request summaries into a read-only `phase_c_plan_review` action for UI/scheduler clients.
- ActingLab client bootstrap diagnostics summary: `session bootstrap` and `session request bootstrap` now embed a compact `status_diagnostics` startup summary so UI/scheduler clients can read liveness, queue health, pending live-validation state, and Phase C follow-up hints from the startup envelope.
- ActingLab connect-plan next-action summary: `session connect-plan` and `session request connect-plan` now expose a machine-readable `next_actions` startup decision list for UI/scheduler clients, covering readiness blockers, stream preflight, trusted transport policy, Phase C review, and deferred live-validation boundaries.
- ActingLab stream-plan next-action summary: `session stream-plan` and `session request stream-plan` now expose a machine-readable `next_actions` interaction decision list for UI/scheduler clients, covering connect blockers, input-relay lease review, trusted remote stream boundaries, Phase C review, and deferred live-validation boundaries.
- ActingLab self-heal-plan next-action summary: `session self-heal-plan` and `session request self-heal-plan` now expose a machine-readable `next_actions` recovery decision list for UI/scheduler clients, covering observe-first selection, stale-capture diagnosis, readiness/queue/lease blockers, recovery review, Phase C review, and deferred live-validation boundaries.
- ActingLab self-heal execution gate: `session self-heal-plan` now exposes `execution_gate` as a compact maintenance-recovery gate for UI/scheduler clients, mirroring trigger selection, readiness, queue admission, lease readiness, blocker reasons, and safe-to-execute state.
- ActingLab transport-plan next-action summary: `session transport plan` and `session request transport plan` now expose a machine-readable `next_actions` trusted-channel decision list for UI/API clients, covering endpoint classification, endpoint-policy blockers, token/certificate preparation, listener/TLS design review, request serialization/audit review, and deferred live-validation boundaries.
- ActingLab trusted remote gate: `session transport plan` now exposes `trusted_remote_gate` as a compact encrypted-channel readiness gate for UI/API clients, mirroring endpoint policy, auth material, listener/TLS/token implementation state, blocked reasons, and live-validation deferral.
- ActingLab validation-plan next-action summary: `session validation-plan` and `session request validation-plan` now expose a machine-readable `next_actions` live-acceptance decision list for UI/scheduler/operator clients, covering pending live acceptance review, readiness, capture freshness, Phase C self-heal, interaction stream, trusted-channel transport, and status diagnostics.
- ActingLab validation-plan diagnostics routing: request-journal validation-plan summaries are now discoverable through event data-summary filters and can produce a read-only `validation_plan_review` status recommendation for UI/scheduler/operator clients.
- ActingLab validation-plan status-view contract alignment: `session api` now advertises `status_view.validation_plan_actions=["validation_plan_review"]`, so UI/scheduler clients can discover the validation-plan diagnostics action without hard-coding it.
- ActingLab validation diagnostics next-action summary: `session status --diagnostics` now embeds a compact validation `next_actions` summary even without a recent validation-plan journal entry, exposing action count, first action, ordered action kinds, Phase C lane summaries, live-validation deferral, and no-live-pass guarantees.
- ActingLab Phase C diagnostics next-action summary: `session status --diagnostics` now embeds a compact `diagnostics.phase_c.next_actions` summary even without a recent Phase C journal entry, exposing self-heal, interaction-flow, trusted-channel, live-validation, and no-device/no-listener guarantees.
- ActingLab Phase C status-view API contract alignment: `session api` now advertises `status_view.phase_c_summary_field="diagnostics.phase_c"`, so UI/scheduler clients can discover the compact Phase C diagnostics summary without hard-coding the field path.
- ActingLab Phase C bootstrap diagnostics summary: `session bootstrap` now embeds the compact `status_diagnostics.phase_c` summary so UI/scheduler startup clients can read self-heal, interaction-flow, trusted-channel, and live-validation next actions from the first no-device startup envelope.
- ActingLab Phase C bootstrap-view API contract alignment: `session api` now advertises `bootstrap_view.status_diagnostics_phase_c_field="status_diagnostics.phase_c"`, so startup clients can discover the embedded Phase C diagnostics summary from the API contract.
- ActingLab bootstrap validation diagnostics API contract alignment: `session api` now advertises `bootstrap_view.status_diagnostics_validation_field="status_diagnostics.validation"`, so startup clients can discover the embedded pending-live validation summary from the API contract.
- ActingLab capture freshness status diagnostics summary: `session status --diagnostics` now embeds `diagnostics.capture_freshness`, exposing stale-frame policy, backend order, AK stale-screencap classification, and lighter recovery guidance without touching devices.
- ActingLab bootstrap capture freshness API contract alignment: `session bootstrap` now embeds `status_diagnostics.capture_freshness`, and `session api` advertises both `status_view.capture_freshness_summary_field` and `bootstrap_view.status_diagnostics_capture_freshness_field`.
- ActingLab interaction/trusted-channel status diagnostics summary: `session status --diagnostics` and `session bootstrap` now expose compact `interaction_flow` and `trusted_channel` summaries, and `session api` advertises their status/bootstrap field paths.
- ActingLab self-heal status diagnostics summary: `session status --diagnostics` and `session bootstrap` now expose compact `self_heal` summaries, and `session api` advertises their status/bootstrap field paths.
- ActingLab readiness diagnostics summary: `session readiness` now exposes a compact `diagnostics_summary` for liveness, queue health, capture freshness, self-heal, interaction flow, trusted channel, and live-validation state without forcing clients to parse the full embedded status view.
- ActingLab session recording authorization policy: `session record-policy` and `session request record-policy` expose the active recording authorization model, allowed step kinds, frame-source policy, resource-write policy, safety policy, and no-device/no-resource-write guarantees for UI/scheduler/agent clients.
- ActingLab session recording command-check classification: `session command-check` now distinguishes current-frame recording from local-frame daemon-state work, so `session record step --capture` and `--current-frame` are device-affecting read-only while local `--frame` authoring remains daemon-state.
- ActingLab live acceptance checklist granularity: `session validation-plan` now lists record current-frame authoring, interactive stream/input relay, and trusted-channel security as separate `deferred: requires-live-device` acceptance items.
- ActingLab queue event summary: daemon request journals and `session events --data-summary-kind queue` now expose compact queue health, admission, request counts, recommended actions, and no-device/no-capture guarantees for UI/scheduler clients.
- ActingLab self-heal policy event summary: daemon request journals and `session events --data-summary-kind self_heal_policy` now expose compact maintenance-only recovery policy, trigger guard, lease/scheduler boundary, and no-device/no-resource guarantees for UI/scheduler clients.
- ActingLab throat policy event summary: daemon request journals and `session events --data-summary-kind throat_policy` now expose compact unique-control-throat, strict-session, route, lease, trusted-remote, and no-device/no-resource guarantees for UI/scheduler clients.
- ActingLab capture policy event summary: daemon request journals and `session events --data-summary-kind capture_policy` now expose compact fresh-frame, stale-capture, AK false-freeze, lighter-recovery, deferred-live, and no-device/no-capture guarantees for UI/scheduler clients.
- ActingLab record policy event summary: daemon request journals and `session events --data-summary-kind record_policy` now expose compact active authorization, explicit step-kind selection, no passive full recording, explicit resource promotion, safety, deferred-live, and no-device/no-resource-write guarantees for UI/scheduler clients.
- ActingLab detailed target-file status table: `CHECKPOINT.md` now breaks `TASK-Lab-session-layer.md` and `FINDING-AK-game-freeze-2026-06-27.md` into 56 explicit rows marked `已完成`, `待真机验收`, `未完成`, `未来范围`, or `待资源补齐`.

## Current ActingLab Live Acceptance Checklist Granularity

This increment makes the skipped live/device/operator work explicit enough for later operator验收 without treating offline checks as live passes.

- `session validation-plan.deferred_live_tasks` now separates:
  - prepared emulator/session-layer validation;
  - AK stale-capture fresh-frame recovery validation;
  - live ADB/device-control/screenshot validation;
  - operator acceptance observation;
  - record current-frame authoring live validation;
  - interactive stream/input-relay live validation;
  - trusted-channel security live validation.
- `session validation-plan.pending_live_acceptance.items` mirrors the same split and keeps every item at `status=deferred` with `deferred_code=requires-live-device`.
- Record current-frame authoring requires live frame capture evidence, freshness/provenance/hash metadata, and operator artifact review before promotion.
- Interactive stream/input relay requires observed stream frames, lease-gate behavior, and journal/audit evidence before being marked accepted.
- Trusted-channel security requires reviewed encrypted/authenticated transport plus request admission and audit logging before remote control can be considered accepted.
- The validation-plan query remains offline: it does not enqueue, capture, touch devices, start MaaTouch, start apps, start listeners, issue tokens, start TLS, read resource repositories, or mark live validation passed.

## Current ActingLab Command-Check Phase C Scope

This increment makes the Phase C lane decision visible from the earliest no-device command preflight.

- `session command-check <command...>` now returns `phase_c_scope.schema_version=session.command_phase_c_scope.v0.1`.
- `phase_c_scope` classifies the target command into self-heal, interaction-flow, trusted-channel, and live-validation lanes using the same lane vocabulary as `submit-plan` Phase C preflight.
- Self-heal remains maintenance-only: it can plan observe/diagnose/recover work, but must not execute game-progress actions from this preflight surface.
- Interaction flow remains lease-gated: input relay commands are visible as control work requiring a matching LabLease, and direct ADB/device access stays prohibited.
- Trusted channel remains reserved and closed by default: encrypted/authenticated remote access is documented as required, while listeners, token issuance, and TLS startup remain unimplemented in this increment.
- Live validation remains `deferred` with `deferred_code=requires-live-device`; `command-check` must not mark any live check accepted.
- This is an offline contract/discovery increment only: it does not enqueue daemon work, capture frames, touch devices, start MaaTouch, start apps, start listeners, issue tokens, start TLS, read resource repositories, write SQLite, or run game logic.

## Current ActingLab Command-Check Phase C Event Summary

This increment makes command-check Phase C scope discoverable from daemon request journals and event filters.

- `session_request_data_summary` now emits `kind=command_check` for successful `session request command-check ...` responses.
- The command-check summary carries command class, lease requirement, safe-to-submit status, Session throat status, direct-device-access prohibition, blockers, and compact Phase C lane data.
- Interaction-flow preflights such as `stream --input-event <action,args>` can now be retrieved with `session events --data-summary-kind command_check`.
- The event summary preserves no-device guarantees: no enqueue, no capture, no MaaTouch, no listener, no token issuer, no TLS startup, and no live-validation pass marker.
- `session api` now advertises `command_check` in `event_view.data_summary_kinds`.
- This is an offline journal/event contract increment only; it does not execute recovery, open streams, start listeners, issue tokens, start TLS, capture frames, touch devices, read resource repositories, write SQLite, or run game logic.

## Current ActingLab Submit-Plan Phase C Event Summary

This increment makes submit-plan admission and Phase C execution preflight discoverable from daemon request journals and event filters.

- `session_request_data_summary` now emits `kind=submit_plan` for successful `session request submit-plan ...` responses.
- The submit-plan summary carries readiness, command-check, queue admission, lease, Session throat, instance gate, and compact Phase C execution-preflight fields.
- Interaction-flow preflights such as `session request submit-plan stream --input-event <action,args>` can now be retrieved with `session events --data-summary-kind submit_plan`.
- The summary preserves no-device guarantees: no enqueue from the query, no capture, no MaaTouch, no listener, no token issuer, no TLS startup, and no live-validation pass marker.
- `session api` now advertises `submit_plan` in `event_view.data_summary_kinds`.
- This is an offline journal/event contract increment only; it does not execute recovery, open streams, start listeners, issue tokens, start TLS, capture frames, touch devices, read resource repositories, write SQLite, or run game logic.

## Current ActingLab Phase C Plan Event Summary

This increment makes the aggregate Phase C roadmap discoverable from daemon request journals and event filters.

- `phase_c_plan` request summaries now carry compact `next_actions` lane fields for self-heal, interaction flow, trusted channel, and pending live acceptance.
- `session events --data-summary-kind phase_c_plan` can now return the aggregate Phase C plan with next-action schema, first action, lane statuses, trusted-channel encryption/authentication requirements, pending live-acceptance count, and no-live-pass guarantees.
- The event summary preserves the three Phase C boundaries: self-heal stays observe/plan-first, interaction flow stays lease-gated and instance-preflighted, and trusted remote control stays reserved until encryption/authentication/listener/token/TLS/audit work is explicitly implemented and live-validated.
- This is an offline journal/event contract increment only; it does not execute recovery, open streams, start listeners, issue tokens, start TLS, capture frames, touch devices, read resource repositories, write SQLite, or run game logic.

## Current ActingLab Recording Command Preflight Classification

This increment aligns Session Layer preflight classification with the active recording policy.

- `session command-check session record step --capture` and `session command-check session record step --current-frame` now report `command_class=read_only`, `device_affecting=true`, and `requires_lease=false`.
- `session command-check session record step --frame <png>` remains `command_class=daemon_state`, `device_affecting=false`, and `requires_lease=false`.
- Unknown `session record <action>` values now fail loudly from command-check instead of being accepted as a broad daemon-state command.
- `session contract`, `session api`, and Session Layer capability contracts now advertise current-frame recording as a device-affecting read-only example and local-frame recording as daemon-state work.
- The command-check and submit-plan preflight queries remain offline and keep their guarantees: they do not enqueue, capture, touch devices, start MaaTouch, start apps, start listeners, issue tokens, start TLS, read resource repositories, or mark live validation passed.
- Live validation remains `deferred: requires-live-device`.

## Current ActingLab Phase C Interaction Plan View

This increment makes the Phase C aggregate plan more explicit about the interaction-flow lane without requiring a live instance or running stream checks.

- `session phase-c-plan` now embeds `interaction_flow.plan.schema_version=session.phase_c_interaction_plan.v0.2`.
- The embedded interaction plan reports `status=requires_instance_preflight`.
- The plan points clients to `session stream-plan [--endpoint <url>]` and `session request stream-plan [--endpoint <url>]` for the actual instance-scoped stream preflight.
- The plan keeps bounded local CLI stream and daemon-bounded stream request surfaces available, while trusted remote long-lived stream remains `reserved`.
- Input relay remains lease-gated, and `interaction_flow.plan.next_actions` starts with `run_instance_stream_plan`.
- `session api` now advertises the interaction plan schema version and stream-plan contract field under `phase_c_plan_view`.

This is an offline contract/discovery increment only:

- no daemon request is enqueued;
- no stream is opened;
- no stream check is run;
- no device, capture backend, MaaTouch backend, app lifecycle, resource repository, UI, scheduler runtime, SQLite, OCR/OpenCV, network listener, token issuer, TLS layer, or game logic is touched;
- live validation stays `deferred: requires-live-device`.

## Current ActingLab Readiness Diagnostics Summary

This increment gives UI, scheduler, and agent clients a compact startup-readiness diagnostic slice while preserving `status_view` as the full source of truth.

- `session readiness` now includes `diagnostics_summary`.
- `diagnostics_summary` is derived from `session status --diagnostics`; it does not run capture, self-heal, stream, transport, or device operations again.
- The summary exposes liveness, queue health, stale-capture classification, self-heal default recovery state, interaction-flow relay lease requirement, trusted-channel security state, and pending live-validation deferral.
- `session request readiness` returns the same `diagnostics_summary` through the daemon request path.
- `session api` now declares `envelopes.readiness_view.diagnostics_summary_field=diagnostics_summary`.
- `session api` now declares `envelopes.readiness_view.diagnostics_summary_schema_version=session.readiness_diagnostics_summary.v0.1`.

## Current ActingLab Self-Heal Status Diagnostics Summary

This increment makes the Phase C self-heal lane directly visible from ordinary status diagnostics and startup bootstrap output without executing recovery.

- `session status --diagnostics` now includes `diagnostics.self_heal`.
- `diagnostics.self_heal` reuses the offline `session self-heal-policy` boundary and adds a compact default blocked plan.
- The self-heal summary exposes trigger policy, recovery order, maintenance-only boundaries, lease/scheduler coordination, next action kinds, and deferred live validation state.
- `session bootstrap` now embeds `status_diagnostics.self_heal`.
- `session api` now declares `envelopes.status_view.self_heal_summary_field=diagnostics.self_heal`.
- `session api` now declares `envelopes.bootstrap_view.status_diagnostics_self_heal_field=status_diagnostics.self_heal`.
- The helper is intentionally non-recursive and does not call the full `session self-heal-plan`; status/readiness/bootstrap remain offline and no-device.

## Current ActingLab Interaction And Trusted-Channel Status Diagnostics Summary

This increment makes the Phase C interaction-flow and trusted-channel lanes directly visible from ordinary status diagnostics and startup bootstrap output.

- `session status --diagnostics` now includes `diagnostics.interaction_flow`.
- `session status --diagnostics` now includes `diagnostics.trusted_channel`.
- `session bootstrap` now embeds `status_diagnostics.interaction_flow` and `status_diagnostics.trusted_channel`.
- `session api` now declares `envelopes.status_view.interaction_flow_summary_field=diagnostics.interaction_flow`.
- `session api` now declares `envelopes.status_view.trusted_channel_summary_field=diagnostics.trusted_channel`.
- `session api` now declares `envelopes.bootstrap_view.status_diagnostics_interaction_flow_field=status_diagnostics.interaction_flow`.
- `session api` now declares `envelopes.bootstrap_view.status_diagnostics_trusted_channel_field=status_diagnostics.trusted_channel`.
- The interaction-flow summary is non-recursive and does not depend on instance configuration; it reports bounded local stream availability, daemon request serialization, lease-gated input relay, reserved trusted remote long-lived stream status, and read-only next actions.
- The trusted-channel summary reports the local CLI lane, daemon file-IPC lane, reserved trusted remote lane, encryption/authentication requirements, listener/token/TLS non-implementation, and read-only next actions.
- The change does not enqueue requests, mutate queues, capture frames, start MaaTouch, touch devices, start listeners, probe TCP, issue tokens, start TLS, read resource repositories, execute recovery, or perform live validation.

## Current ActingLab Bootstrap Capture Freshness API Contract Alignment

This increment aligns startup and API-discovery surfaces with the capture freshness diagnostics summary.

- `session bootstrap` now includes `status_diagnostics.capture_freshness`.
- `session api` now declares `envelopes.status_view.capture_freshness_summary_field=diagnostics.capture_freshness`.
- `session api` now declares `envelopes.bootstrap_view.status_diagnostics_capture_freshness_field=status_diagnostics.capture_freshness`.
- Startup clients can discover and read stale-frame policy, backend order, AK stale-screencap classification, and live-validation deferral from the bootstrap/API surfaces.
- The change does not enqueue requests, mutate queues, capture frames, start MaaTouch, touch devices, start listeners, issue tokens, start TLS, read resource repositories, execute recovery, or perform live validation.

## Current ActingLab Capture Freshness Status Diagnostics Summary

This increment makes the AK stale screencap finding visible from the normal Session Layer status diagnostics surface.

- `session status --diagnostics` now includes `diagnostics.capture_freshness`.
- The summary points clients to `session capture-policy`, `capture diagnose --require-fresh`, and `session capture diagnose --require-fresh`.
- The summary exposes the preferred capture backend order `nemu_ipc -> droidcast_raw -> adb_screencap`, with `adb_screencap` marked as the last resort.
- The summary records that stale `adb_screencap` evidence alone must not be classified as game freeze.
- The summary records that capture-backend recovery should be considered before heavy app restart.
- The summary keeps live validation deferred as `requires-live-device` and guarantees that the diagnostics query does not enqueue requests, capture frames, start MaaTouch, touch devices, start listeners, issue tokens, start TLS, read resource repositories, or mark live validation passed.

## Current ActingLab Bootstrap Validation Diagnostics API Contract Alignment

This increment aligns the bootstrap API contract with the validation diagnostics summary embedded in `session bootstrap`.

- `session api` now declares `envelopes.bootstrap_view.status_diagnostics_validation_field=status_diagnostics.validation`.
- The field points UI, scheduler, and agent startup clients to the embedded pending-live validation summary.
- This complements `bootstrap_view.status_diagnostics_phase_c_field=status_diagnostics.phase_c` and the existing `validation_plan_field=validation_plan`.
- The change does not enqueue requests, mutate queues, capture frames, start MaaTouch, touch devices, start listeners, issue tokens, start TLS, read resource repositories, execute recovery, or perform live validation.

## Current ActingLab Phase C Bootstrap-View API Contract Alignment

This increment aligns the bootstrap API contract with the compact Phase C diagnostics summary embedded in `session bootstrap`.

- `session api` now declares `envelopes.bootstrap_view.status_diagnostics_phase_c_field=status_diagnostics.phase_c`.
- The field points UI, scheduler, and agent startup clients to the embedded no-device Phase C status summary.
- This complements `bootstrap_view.status_diagnostics_field=status_diagnostics` and `status_view.phase_c_summary_field=diagnostics.phase_c`.
- The change does not enqueue requests, mutate queues, capture frames, start MaaTouch, touch devices, start listeners, issue tokens, start TLS, read resource repositories, execute recovery, or perform live validation.

## Current ActingLab Phase C Bootstrap Diagnostics Summary

This increment aligns the startup bootstrap envelope with the compact Phase C diagnostics summary exposed by `session status --diagnostics`.

- `session bootstrap` now includes `status_diagnostics.phase_c` with schema `session.phase_c_diagnostics.v0.1`.
- The embedded Phase C summary carries `next_actions` with schema `session.phase_c_next_actions.v0.1`.
- Startup clients can read the first Phase C follow-up action, trusted-channel encryption/authentication boundaries, no-TLS/no-listener guarantees, and live-validation deferral directly from bootstrap.
- The change does not enqueue requests, mutate queues, capture frames, start MaaTouch, touch devices, start listeners, issue tokens, start TLS, read resource repositories, execute recovery, or perform live validation.

## Current ActingLab Phase C Status-View API Contract Alignment

This increment aligns the machine-readable API contract with the compact Phase C diagnostics summary added to `session status --diagnostics`.

- `session api` now declares `envelopes.status_view.phase_c_summary_field=diagnostics.phase_c`.
- The field points UI, scheduler, and agent clients to the compact no-device Phase C status summary.
- The API contract already advertises `phase_c_plan_actions=["phase_c_plan_review"]`; this increment adds the direct summary field path for startup/status consumers.
- The change does not enqueue requests, mutate queues, capture frames, start MaaTouch, touch devices, start listeners, issue tokens, start TLS, read resource repositories, execute recovery, or perform live validation.

## Current ActingLab Phase C Diagnostics Next-Action Summary

This increment makes the Phase C roadmap visible from the ordinary status diagnostics startup path, without requiring clients to first create a `phase-c-plan` journal entry.

- `session status --diagnostics` now includes `diagnostics.phase_c` with schema `session.phase_c_diagnostics.v0.1`.
- `diagnostics.phase_c.next_actions` is derived from the same Phase C next-action contract used by `session phase-c-plan`.
- The compact summary exposes action count, first action, ordered action kinds, self-heal summary, interaction-flow summary, trusted-channel summary, live-validation deferral, and guarantees.
- The self-heal lane remains observe-first and reports `recovery_kind=observe_first` in this compact status context.
- The interaction-flow lane keeps long-lived UI stream status reserved and records that input relay requires a matching lease.
- The trusted-channel lane records encryption/authentication requirements plus no-listener, no-token, and no-TLS guarantees.
- Live validation remains `deferred: requires-live-device`; offline diagnostics must not mark live validation passed.
- The change does not enqueue requests, mutate queues, capture frames, start MaaTouch, touch devices, start listeners, issue tokens, start TLS, read resource repositories, execute recovery, or perform live validation.

## Current ActingLab Phase C Aggregate Plan Surface

This increment gives UI, scheduler, and agent clients one Phase C planning envelope instead of requiring them to manually stitch together self-heal, stream, trusted-channel, and validation-plan queries.

- `session phase-c-plan [--endpoint <url>] [--trigger <kind>] [--to <page>]` returns `session.phase_c_plan.v0.1`.
- `session request phase-c-plan ...` exposes the same surface through the resident daemon request queue.
- The plan includes self-heal policy/plan data, an interaction-flow contract summary, trusted-channel transport plan data, and the pending live-validation plan.
- Interaction flow remains a contract/preflight summary in this aggregate view; instance-level stream readiness still belongs to `session stream-plan`.
- Trusted remote remains reserved: the aggregate plan records that no listener is started, no token is issued, and no TLS implementation is started.
- Live validation remains `deferred: requires-live-device`, and offline checks must not mark live items passed.
- `session bootstrap`, `session api`, `session contract`, `session capabilities`, `session command-check`, request data summaries, and event data-summary filters now advertise the Phase C plan surface.

No device input, capture, MaaTouch startup, app lifecycle action, heavy recovery execution, daemon startup, network listener, TCP probe, TLS implementation, token issuance, UI, scheduler runtime, SQLite, OCR/OpenCV, game logic, direct ADB input fallback, reconnect loop, cooperation-workspace copy, resource repository sync/read, or live validation was added.

## Current ActingLab Validation-Plan Next-Action Summary

This increment turns the live-validation boundary into an ordered no-device checklist for UI, scheduler, and operator clients.

- `session validation-plan` now embeds `next_actions` with schema `session.validation_next_actions.v0.1`.
- `session request validation-plan` returns the same next-action summary through the resident daemon request queue.
- The summary orders read-only follow-up actions for pending live acceptance, session readiness, capture freshness policy, Phase C self-heal review, interaction stream review, trusted-channel transport review, and status diagnostics.
- The pending-live summary keeps every acceptance item marked `deferred: requires-live-device`.
- Phase C planning remains split into self-heal, interaction flow, and trusted-channel lanes, all referenced from the validation-plan next-action surface.
- The change does not enqueue requests, capture frames, start MaaTouch, touch devices, start listeners, issue tokens, start TLS, read resource repositories, or mark live validation passed.

## Current ActingLab Validation-Plan Diagnostics Routing

This increment connects the validation-plan summary to the existing Session Layer diagnostics surfaces.

- `session events` and `session request events` now advertise `validation_plan` as a supported `--data-summary-kind`.
- `session status --diagnostics` can turn a recent validation-plan request summary into a read-only `validation_plan_review` recommended action.
- The recommended action preserves pending live item count, next-action count, first next action, Phase C lane summaries, and the source request id.
- The recommendation is explicitly operator/live-environment work: it requires live acceptance and must not mark live validation passed from offline checks.
- `session api` now declares the status-view validation action kind through `status_view.validation_plan_actions`.
- `session status --diagnostics` now carries a compact `diagnostics.validation.next_actions` summary derived from the same validation-plan next-action contract.
- The compact status summary includes action count, first action, ordered action kinds, Phase C lane summaries, live-validation deferral, and no-live-pass guarantees.
- The change does not enqueue requests, mutate queues, capture frames, start MaaTouch, touch devices, start listeners, issue tokens, start TLS, read resource repositories, or perform live validation.

## Current ActingLab Phase C Aggregate Next-Action Summary

This increment turns the aggregate `session phase-c-plan` view into an actionable no-device roadmap for UI, scheduler, and agent clients.

- `session phase-c-plan` now embeds `next_actions` with schema `session.phase_c_next_actions.v0.1`.
- `session request phase-c-plan` returns the same next-action summary through the resident daemon request queue.
- The summary orders read-only follow-up actions for self-heal plan review, interaction stream-plan review, trusted-channel transport review, pending live acceptance review, and current status diagnostics.
- The self-heal summary records trigger, recovery kind, maintenance readiness, escalation category, and operator/live-validation requirement.
- The interaction-flow summary keeps long-lived UI stream status reserved and records that input relay requires a matching lease.
- The trusted-channel summary records reserved transport status, endpoint policy result, token/certificate configuration signals, and no-listener/no-token/no-TLS guarantees.
- The live-validation summary carries `deferred: requires-live-device` and the pending live acceptance payload from `session validation-plan`.
- `phase_c_plan` daemon request summaries now retain next-action count and first next action for journal/events consumers.
- The Session API contract now advertises `next_actions_field=next_actions` under `phase_c_plan_view`.

No device input, capture, MaaTouch startup, app lifecycle action, self-heal execution, heavy recovery execution, daemon startup, network listener, TCP probe, TLS implementation, token issuance, UI, scheduler runtime, SQLite, OCR/OpenCV, game logic, direct ADB input fallback, reconnect loop, cooperation-workspace copy, resource repository sync/read, or live validation was added.

## Current ActingLab Phase C Self-Heal Next-Action Summary

This increment turns the detailed `session self-heal-plan` preflight into an actionable no-device roadmap for UI, scheduler, and agent clients.

- `session self-heal-plan` now embeds `next_actions` with schema `session.self_heal_next_actions.v0.1`.
- `session request self-heal-plan` returns the same next-action summary through the resident daemon request queue.
- The summary orders follow-up actions for observe-first trigger selection, stale-capture freshness diagnosis, session readiness, daemon queue inspection, maintenance lease resolution, recovery-candidate review, submission preflight, status diagnostics, and Phase C aggregate review.
- Stale capture and capture-backend-unavailable triggers place `inspect_capture_freshness` before any heavy recovery review, preserving the rule that stale `adb_screencap` evidence alone must not be treated as game-freeze proof.
- Maintenance-control triggers such as standby, unexpected page, modal popup, startup-login required, and session expired surface `resolve_maintenance_lease` when the current lease gate is not satisfied.
- The recovery summary records trigger, target page, recovery kind, matching-lease requirement, maintenance readiness, heavy-recovery candidate, and the no-heavy-recovery guarantee.
- The readiness, queue, and lease summaries expose the blockers that a UI/scheduler should resolve before submitting recovery work.
- `self_heal_plan` daemon request summaries now retain next-action count, first next action, and deferred live-validation status for journal/events consumers.
- The Session API contract now advertises `next_actions_field=next_actions` under `self_heal_plan_view`.

No device input, capture, MaaTouch startup, app lifecycle action, self-heal execution, heavy recovery execution, daemon startup, network listener, TCP probe, TLS implementation, token issuance, UI, scheduler runtime, SQLite, OCR/OpenCV, game logic, direct ADB input fallback, reconnect loop, cooperation-workspace copy, resource repository sync/read, or live validation was added.

## Current ActingLab Phase C Plan Diagnostics Recommended Action

This increment connects the aggregate Phase C planning surface to the status diagnostics recommendation lane.

- `session status --diagnostics` inspects recent request journal entries for `data_summary.kind=phase_c_plan`.
- If the latest Phase C aggregate plan still has reserved interaction flow, reserved trusted-channel work, or deferred live validation, diagnostics emits `phase_c_plan_review`.
- The action routes back to `session phase-c-plan`, preserving summarized trigger and endpoint context when present.
- The action records the source request id, source command, next review queries, and full data summary.
- The Session API contract advertises `phase_c_plan_actions` under `status_view`.
- The action explicitly records that it does not touch devices, start a listener, issue tokens, start TLS, or mark live validation passed.

No device input, capture, MaaTouch startup, app lifecycle action, heavy recovery execution, daemon startup, network listener, TCP probe, TLS implementation, token issuance, UI, scheduler runtime, SQLite, OCR/OpenCV, game logic, direct ADB input fallback, reconnect loop, cooperation-workspace copy, resource repository sync/read, or live validation was added.

## Current ActingLab Connect-Plan Next-Action Summary

This increment gives future UI and scheduler clients a deterministic startup decision list after they call `session connect-plan`.

- `session connect-plan` now embeds `next_actions` with schema `session.connect_next_actions.v0.1`.
- `session request connect-plan` returns the same next-action summary through the resident daemon request queue.
- The summary orders read-only follow-up actions for readiness blockers, trusted transport policy, stream preflight, and Phase C aggregate review.
- The summary records readiness status and recommended-action kinds, stream safety, input-relay lease requirements, trusted remote reserved status, and live-validation deferred status.
- `connect_plan` daemon request summaries now retain next-action count, first next action, trusted-remote status, and live-validation status for journal/events consumers.
- The Session API contract now advertises `next_actions_field=next_actions` under `connect_plan_view`.
- Trusted remote remains reserved: the next-action summary does not start a listener, issue tokens, start TLS, probe TCP, or accept remote clients.
- Live validation remains `deferred: requires-live-device`, and offline checks must not mark live items passed.

No device input, capture, MaaTouch startup, app lifecycle action, heavy recovery execution, daemon startup, network listener, TCP probe, TLS implementation, token issuance, UI, scheduler runtime, SQLite, OCR/OpenCV, game logic, direct ADB input fallback, reconnect loop, cooperation-workspace copy, resource repository sync/read, or live validation was added.

## Current ActingLab Stream-Plan Next-Action Summary

This increment gives future UI and scheduler clients a deterministic interaction-flow decision list after they call `session stream-plan`.

- `session stream-plan` now embeds `next_actions` with schema `session.stream_next_actions.v0.1`.
- `session request stream-plan` returns the same next-action summary through the resident daemon request queue.
- The summary orders follow-up actions for connect-plan blockers, stream preflight review, input-relay lease review, trusted remote stream-boundary review, Phase C aggregate review, and bounded local stream opening when safe.
- The summary records connect safety, stream safety, input-relay request count, lease gate data, Phase C self-heal/interaction/trusted-channel plan commands, trusted remote reserved status, and live-validation deferred status.
- `stream_plan` daemon request summaries now retain next-action count, first next action, trusted-remote status, and live-validation status for journal/events consumers.
- The Session API contract now advertises `next_actions_field=next_actions` under `stream_plan_view`.
- Self-heal remains observe/diagnose/plan first; this summary points clients to `session self-heal-plan` and `session phase-c-plan` without executing recovery.
- Interaction flow remains gated by Session Layer readiness, queue state, and lease state; the summary does not relay input by itself.
- Trusted remote remains reserved: the next-action summary does not start a listener, issue tokens, start TLS, probe TCP, or accept remote clients.
- Live validation remains `deferred: requires-live-device`, and offline checks must not mark live items passed.

No device input, capture, MaaTouch startup, app lifecycle action, heavy recovery execution, daemon startup, network listener, TCP probe, TLS implementation, token issuance, UI, scheduler runtime, SQLite, OCR/OpenCV, game logic, direct ADB input fallback, reconnect loop, cooperation-workspace copy, resource repository sync/read, or live validation was added.

## Current ActingLab Client Bootstrap Diagnostics Summary

This increment makes the bootstrap envelope the first read for future UI and scheduler clients.

- `session bootstrap` now embeds `status_diagnostics` with schema `session.bootstrap_diagnostics.v0.1`.
- `session request bootstrap` returns the same embedded diagnostics through the resident daemon request queue.
- The summary is derived from `session status --diagnostics` and exposes daemon running state, liveness, queue health, pending live-validation summary, recommended-action kinds, and the Phase C aggregate-plan command.
- The Session API contract now advertises `status_diagnostics_field=status_diagnostics` under `bootstrap_view`.
- UI clients should read bootstrap first, then use `session status --diagnostics`, `session readiness`, `session submit-plan`, or `session phase-c-plan` for deeper views.
- Scheduler clients should keep using Session Layer as the only control throat and treat the embedded diagnostics as startup state, not permission to bypass lease or queue gates.
- Self-heal remains observe/diagnose/plan first; maintenance execution still requires the existing lease/queue/readiness gates.
- Trusted remote remains reserved: no listener is started, no token is issued, and no TLS implementation is started.
- Live validation remains `deferred: requires-live-device`, and offline checks must not mark live items passed.

No device input, capture, MaaTouch startup, app lifecycle action, heavy recovery execution, daemon startup, network listener, TCP probe, TLS implementation, token issuance, UI, scheduler runtime, SQLite, OCR/OpenCV, game logic, direct ADB input fallback, reconnect loop, cooperation-workspace copy, resource repository sync/read, or live validation was added.

## Current ActingLab Pending-Live Validation Diagnostics Summary

This increment makes the skipped live-device acceptance list visible from the main status diagnostics surface.

- `session validation-plan` remains the source of truth for pending live acceptance.
- `session status --diagnostics` now exposes `diagnostics.validation`.
- The diagnostics summary includes the validation-plan command, pending-live title, status, deferred code, owner, item count, and the rule that offline checks must not mark live items passed.
- The Session API contract advertises `validation_summary_field=diagnostics.validation` under `status_view`.
- This keeps the 2026-06-28 requirement to collect skipped live/device/operator work into a visible "待真机验收" lane while offline implementation continues.

No device input, capture, MaaTouch startup, app lifecycle action, heavy recovery execution, daemon startup, resource repository read, network listener, TCP probe, TLS implementation, token issuance, UI, scheduler runtime, SQLite, OCR/OpenCV, game logic, direct ADB input fallback, reconnect loop, cooperation-workspace copy, resource repository sync, or live validation was added.

## Current ActingLab Interaction And Trusted-Channel Recommended Actions

This increment connects the existing stream and transport planning summaries to the status diagnostics recommendation surface.

- `session status --diagnostics` inspects recent request journal entries for `data_summary.kind=stream_plan` and `data_summary.kind=transport_plan`.
- A blocked or reserved stream plan emits `interactive_stream_preflight_review`.
- A reserved or blocked trusted-channel transport plan emits `trusted_channel_preflight_review`.
- Stream review actions route back to `session stream-plan` and preserve instance context.
- Trusted-channel review actions route back to `session transport plan` and preserve the endpoint when one was summarized.
- The trusted-channel action explicitly records that it does not start a listener, issue tokens, or start TLS.
- The Session API contract advertises `interaction_channel_actions` under `status_view`.

No device input, capture, MaaTouch startup, app lifecycle action, heavy recovery execution, daemon startup, resource repository read, network listener, TCP probe, TLS implementation, token issuance, UI, scheduler runtime, SQLite, OCR/OpenCV, game logic, direct ADB input fallback, reconnect loop, cooperation-workspace copy, resource repository sync, or live validation was added.

## Current ActingLab Phase C Self-Heal Escalation Recommended Action

This increment connects the self-heal escalation summary to the existing status diagnostics recommendation surface.

- `session status --diagnostics` inspects recent request journal entries for `data_summary.kind=self_heal_plan`.
- When the latest self-heal plan reaches a heavy-recovery candidate while preserving `does_not_execute_heavy_recovery=true`, status diagnostics emits `self_heal_escalation_review`.
- The recommended action is read-only and routes back to `session self-heal-plan --trigger <kind>`.
- The action records the source request id, source command, escalation category, heavy-recovery candidate, operator/live-validation requirement, and full data summary.
- UI and scheduler clients can now see that repeated transient failures need review without executing app restart or other heavy recovery.
- The Session API contract advertises `self_heal_escalation_actions` under `status_view`.

No device input, capture, MaaTouch startup, app lifecycle action, heavy recovery execution, daemon startup, resource repository read, network listener, TLS implementation, token issuance, UI, scheduler runtime, SQLite, OCR/OpenCV, game logic, direct ADB input fallback, reconnect loop, cooperation-workspace copy, resource repository sync, or live validation was added.

## Current ActingLab Phase C Self-Heal Event Summary

This increment makes the already-added self-heal escalation policy observable through lightweight request summaries.

- `session.request.data_summary.v0.1` for `kind=self_heal_plan` now includes `escalation_category`.
- The same summary includes `heavy_recovery_candidate`, `does_not_execute_heavy_recovery`, and `operator_live_validation_required`.
- UI and scheduler consumers can observe repeated-transient-failure escalation policy through `session journal`, `session events`, and request-state summaries without reading the full response body.
- This keeps the Phase C self-heal line aligned with the event-stream and queue-inspection surfaces built for UI/API clients.

No device input, capture, MaaTouch startup, app lifecycle action, heavy recovery execution, daemon startup, resource repository read, network listener, TLS implementation, token issuance, UI, scheduler runtime, SQLite, OCR/OpenCV, game logic, direct ADB input fallback, reconnect loop, cooperation-workspace copy, resource repository sync, or live validation was added.

## Current ActingLab Phase C Self-Heal Escalation Preflight

This increment extends `session.self_heal_plan.v0.1` with an `escalation` field so UI, scheduler, and agent clients can reason about repeated transient failures without guessing.

- Capture stale or capture-backend unavailable triggers report `category=transient_capture_path`.
- Capture-path escalation keeps lightweight capture-backend recovery first and only names app restart as a heavy-recovery review candidate.
- Capture-path escalation explicitly says stale ADB screencap alone must not be treated as a game freeze.
- Standby, unexpected-page, modal, and session-expired triggers report `category=maintenance_control_path`.
- Startup login reports `category=startup_login_path`, a bounded startup-login loop, and the required `STARTUP-LOGIN.md` resource.
- Heavy recovery remains non-executing in this preflight and requires matching lease metadata plus operator/live validation before any future execution path can be accepted.
- Repeated transient failures must be fully logged and escalated visibly instead of looping silently.

No device input, capture, MaaTouch startup, app lifecycle action, heavy recovery execution, resource repository read, network listener, TLS implementation, token issuance, UI, scheduler runtime, SQLite, OCR/OpenCV, game logic, direct ADB input fallback, reconnect loop, cooperation-workspace copy, resource repository sync, or live validation was added.

## Current ActingLab Phase C Self-Heal Lease-Gate Alignment

This increment tightens the UI/scheduler-facing self-heal preflight semantics after the initial `session.self_heal_plan.v0.1` surface landed.

- `session self-heal-plan --trigger capture_stale_suspected` reports `recovery.requires_matching_lease=false`.
- Its embedded `lease_gate` now also reports `required=false` and `status=not_required`.
- Control-oriented maintenance triggers such as `standby` still report `recovery.requires_matching_lease=true` and a blocking lease gate when no matching lease is provided.
- This keeps stale capture recovery visibly read-only while preserving the Session Layer rule that real maintenance input must be lease-gated.

No device input, capture, MaaTouch startup, app lifecycle action, resource repository read, network listener, TLS implementation, token issuance, UI, scheduler runtime, SQLite, OCR/OpenCV, game logic, direct ADB input fallback, reconnect loop, cooperation-workspace copy, resource repository sync, or live validation was added.

## Current ActingLab Phase C Self-Heal Plan Preflight

This increment gives UI, scheduler, and agent clients a concrete Phase C recovery preflight between the static `session self-heal-policy` and actual `session recover` execution.

- `session self-heal-plan` returns `session.self_heal_plan.v0.1`.
- `session request self-heal-plan` returns the same schema through the resident daemon request path.
- The payload accepts optional `--trigger <kind>` and `--to <page>`.
- Supported triggers are `capture_stale_suspected`, `capture_backend_unavailable`, `standby`, `unexpected_page`, `modal_popup`, `startup_login_required`, and `session_expired`; omitting `--trigger` returns an observe-first plan.
- The payload reports observe/diagnose/plan/execute stages, target page, recovery candidate, readiness, queue admission, lease gate, blockers, and no-side-effect guarantees.
- Stale-capture recovery remains read-only and does not require a lease.
- Standby, unexpected page, modal popup, startup-login, and session-expired maintenance recovery require a matching lease before execution.
- Request journal summaries now support `data_summary.kind=self_heal_plan`.
- `session api`, `session bootstrap`, `session contract`, `session command-check`, command capabilities, and readiness policy summary now advertise the self-heal-plan surface.

No device input, capture, MaaTouch startup, app lifecycle action, resource repository read, network listener, TLS implementation, token issuance, UI, scheduler runtime, SQLite, OCR/OpenCV, game logic, direct ADB input fallback, reconnect loop, cooperation-workspace copy, resource repository sync, or live validation was added.

## Current ActingLab Trusted-Channel Transport Plan

This increment gives UI/API clients a stable no-device, no-listener plan for the future encrypted trusted channel before a real network transport is implemented.

- `session transport plan` returns `session.transport_plan.v0.1`.
- `session request transport plan` returns the same schema through the resident daemon request path.
- The payload reports local CLI availability, daemon file-IPC availability, trusted remote reserved status, auth env configuration, optional endpoint policy, blockers, and no-side-effect guarantees.
- Optional `--endpoint <url>` classifies local versus trusted remote policy without probing TCP reachability.
- Remote `http://` endpoints remain blocked by `trusted_remote_transport_blocked`.
- Remote `https://` endpoints with token or client certificate auth are policy-safe, but `ready_to_accept_remote_clients` remains false because the listener is still reserved.
- Request journal summaries now support `data_summary.kind=transport_plan`.
- `session api`, `session transport`, `session contract`, command capabilities, and access-channel contracts now advertise the transport-plan surface.

No network listener, TLS implementation, token issuance, long-lived trusted remote stream, UI, scheduler execution behavior, SQLite, OCR/OpenCV, game logic, resource repository access, new capture/input backend, direct ADB input fallback, reconnect loop, app restart, live device action, cooperation-workspace copy, resource repository sync, or TCP reachability probe was added.

## Current ActingLab Trusted-Channel Transport Next-Action Summary

This increment turns `session transport plan` into an actionable no-listener roadmap for the future encrypted trusted channel.

- `session transport plan` now embeds `next_actions` with schema `session.transport_next_actions.v0.1`.
- `session request transport plan` returns the same next-action summary through the resident daemon request queue.
- The summary orders follow-up actions for endpoint policy classification, blocked endpoint review, token/client-certificate preparation, listener/TLS design review, request serialization/audit review, and live acceptance review.
- Missing endpoint policy produces `classify_endpoint_policy` before any remote-channel design work is considered ready.
- Remote HTTP or otherwise unsafe endpoints produce `review_endpoint_policy_blocker` and keep the status `blocked`.
- Remote HTTPS with configured token or client-certificate auth remains `reserved`, not ready, because no listener/TLS implementation exists yet.
- The trusted-remote summary records endpoint policy state, token/certificate configuration state, listener readiness, remote-client readiness, and blocker count.
- `transport_plan` daemon request summaries now retain next-action count, first next action, auth-material configuration state, and deferred live-validation status for journal/events consumers.
- The Session API contract now advertises `plan_next_actions_field=next_actions` under `transport_view`.

No network listener, TLS implementation, token issuance, long-lived trusted remote stream, UI, scheduler execution behavior, SQLite, OCR/OpenCV, game logic, resource repository access, new capture/input backend, direct ADB input fallback, reconnect loop, app restart, live device action, cooperation-workspace copy, resource repository sync, or TCP reachability probe was added.

## Planned ActingLab Phase C Self-Heal, Interaction, And Trusted Channel

Phase C should stay split into three explicit surfaces so UI, scheduler, and trusted remote clients do not accidentally gain direct device ownership.

- Self-heal flow: keep recovery as `observe -> diagnose -> plan -> execute`, with execution allowed only under a matching lease and only for maintenance-safe recovery. Repeated transient failures should escalate visibly instead of looping forever or silently falling back.
- Interaction flow: UI and agent clients should consume compact session state, readiness, events, stream-plan, transport-plan, and command-check envelopes. User-approved commands should travel through explicit Session Layer requests rather than importing device, MaaTouch, or recognition internals.
- Trusted encrypted channel: keep remote access reserved until a reviewed listener, TLS or mutually authenticated IPC, token or client-certificate authentication, request serialization, and audit logging exist. Preflight commands may classify endpoint policy, but must not start listeners, probe TCP, issue tokens, or claim remote readiness.

Execution plan:

- Self-heal lane: continue building read-only diagnosis and submit-plan event summaries first, then add lease-gated maintenance execution only for bounded recovery actions such as stale-capture handling, standby wake, modal close, login/startup continuation, or safe navigation back to a known page. Any repeated transient failure must surface as an escalation recommendation before a heavier action is allowed.
- Interaction-flow lane: keep the UI/agent path as `bootstrap/readiness -> command-check -> submit-plan -> request queue -> event/journal observation`. Input relay must stay lease-gated, serialized through the Session Layer, and visible in event summaries before a real stream is opened.
- Trusted-channel lane: keep remote control closed by default while the offline contract matures. The future implementation must add reviewed listener ownership, encrypted/authenticated transport, token or certificate material, request serialization, replay/audit records, and live security validation before any remote channel is marked ready.
- Cross-lane rule: Phase C offline surfaces may advertise plans, blockers, and no-device guarantees, but they must not mark live validation passed, start listeners, issue tokens, start TLS, capture frames, start MaaTouch, or directly touch devices.

Current offline implementation status:

- `session self-heal-policy` describes the maintenance recovery boundary.
- `session connect-plan` and `session stream-plan` describe client startup and interaction readiness.
- `session transport plan` describes local CLI, daemon file-IPC, and reserved trusted remote transport policy.
- Live validation and any real trusted-channel listener remain `deferred: requires-live-device` or `reserved` until a later reviewed implementation.

## Current ActingLab Interactive Stream-Plan Preflight

This increment gives UI, scheduler, and agent clients a stable no-device plan for entering the future interactive stream flow without opening a stream or touching devices.

- `session stream-plan` returns `session.stream_plan.v0.1`.
- `session request stream-plan` returns the same schema through the resident daemon request path.
- The payload embeds `connect_plan` and `stream_preflight` so clients can consume one startup envelope.
- The payload reports `safe_to_open_stream`, `safe_to_start_client`, `safe_to_start_stream`, `safe_to_connect_transport`, `input_relay_requested`, `input_relay_action_count`, `stream_modes`, and `blockers`.
- Bounded local CLI stream remains available as a local, non-long-lived mode.
- Daemon-routed stream remains serialized by the resident Session Layer and keeps input relay behind a matching lease.
- Trusted remote long-lived stream remains `reserved`, requires encryption and authentication, and only reports token/certificate configuration status without starting a listener.
- Request journal summaries now support `data_summary.kind=stream_plan`.
- `session api`, `session contract`, `session bootstrap`, `session command-check`, and capabilities now advertise or classify the stream-plan surface.

No network listener, TLS implementation, token issuance, long-lived trusted remote stream, UI, scheduler execution behavior, SQLite, OCR/OpenCV, game logic, resource repository access, new capture/input backend, direct ADB input fallback, reconnect loop, app restart, live device action, cooperation-workspace copy, or resource repository sync was added.

## Current ActingLab Client Connect-Plan Preflight

This increment gives UI, scheduler, and agent clients a single no-device preflight for client startup and interaction planning.

- `session connect-plan` returns `session.connect_plan.v0.1`.
- `session request connect-plan` returns the same schema through the resident daemon request path.
- The payload aggregates `session readiness`, optional trusted transport endpoint checks, and `stream check`.
- The payload reports `safe_to_start_client`, `safe_to_start_stream`, `safe_to_connect_transport`, `client_surfaces`, and `blockers`.
- Untrusted remote HTTP remains blocked by `trusted_remote_transport_blocked`.
- Stream input relay remains lease-gated by the existing stream preflight.
- Request journal summaries now support `data_summary.kind=connect_plan`.
- `session api`, `session contract`, `session bootstrap`, `session command-check`, and capabilities now advertise or classify the connect-plan surface.

No network listener, TLS implementation, token issuance, long-lived trusted remote stream, UI, scheduler execution behavior, SQLite, OCR/OpenCV, game logic, resource repository access, new capture/input backend, direct ADB input fallback, reconnect loop, app restart, live device action, cooperation-workspace copy, or resource repository sync was added.

## Current ActingLab Readiness Client Policy Summary

This increment gives UI, scheduler, and agent clients a compact policy summary inside the existing readiness envelope instead of requiring them to independently stitch together every policy surface before deciding whether to connect or submit work.

- `session readiness` still returns `session.readiness.v0.1`.
- The readiness payload now includes `policy_summary` with `schema_version=session.readiness_policy_summary.v0.1`.
- The summary points clients to the detailed `session throat-policy`, `session capture-policy`, `session self-heal-policy`, `stream check`, and `session transport check` surfaces.
- The summary records that only Session Layer may touch devices and clients must not directly touch ADB or devices.
- The summary records that stale `adb_screencap` alone is not game-freeze evidence.
- The summary records that self-heal execution requires a matching lease and cannot execute game-progress actions.
- The summary records that interactive stream input relay requires a matching lease and trusted remote long-lived streams are still reserved.
- The summary records that trusted remote access requires encryption and authentication, while loopback local checks remain allowed.
- The summary records that live validation remains `deferred: requires-live-device` and must not be marked passed by offline checks.
- `session api` now advertises the `policy_summary` readiness field.

No new command, network listener, TLS implementation, token issuance, UI, scheduler execution behavior, SQLite, OCR/OpenCV, game logic, resource repository access, new capture/input backend, direct ADB input fallback, reconnect loop, app restart, live device action, cooperation-workspace copy, or resource repository sync was added.

## Current ActingLab Phase C Self-Heal Policy Surface

This increment turns Phase C maintenance self-heal boundaries into a stable no-device policy query for UI, scheduler, and agent clients.

- `session self-heal-policy` returns `session.self_heal_policy.v0.1`.
- `session request self-heal-policy` returns the same policy through the resident daemon request path.
- The payload records the Phase C goal: return a session to home or another known-good page without executing game-progress actions.
- The payload records the self-heal flow as `observe -> diagnose -> plan -> execute`.
- The payload records supported triggers including stale capture, capture backend unavailable, standby, unexpected page, modal popup, startup login required, and session expired.
- The payload records recovery order: read-only diagnosis, capture backend recovery, maintenance navigation, startup-login loop, and heavy app lifecycle restart as the last step.
- The payload records that stale `adb_screencap` alone is not game-freeze evidence and that restart requires diagnosis first.
- The payload records maintenance boundaries: no destructive action, no premium or paid resource use, no PvP/exercise, no blind confirmation, and navigation-only by default.
- The payload records scheduler/lease policy: scheduler owns arbitration, Session Layer owns device mechanism, and execution requires a matching lease.
- The payload tells interactive streams to report recovery state but not execute recovery input without a lease.
- `session bootstrap`, `session api`, `session contract`, `session command-check`, and command capabilities now advertise or classify the self-heal-policy surface.
- The surface guarantees it does not enqueue, capture, start MaaTouch, touch devices, start apps, start listeners, or read resource repositories.

Live-device and operator validation remain deferred for this round as `requires-live-device`. No live result is faked or marked passed by this implementation.

No trusted remote network listener, TLS implementation, token issuance, UI, scheduler execution behavior, SQLite, OCR/OpenCV, game logic, resource repository access, new capture/input backend, direct ADB input fallback, reconnect loop, app restart, live device action, cooperation-workspace copy, or resource repository sync was added.

## Current ActingLab Capture Freshness Policy Surface

This increment turns the AK stale screencap finding and the Session Layer fresh-frame rules into a stable no-device policy query for UI, scheduler, and agent clients.

- `session capture-policy` returns `session.capture_policy.v0.1`.
- `session request capture-policy` returns the same policy through the resident daemon request path.
- The payload records fresh-frame commands, stale-frame visibility expectations, backend preference order, and fallback logging requirements.
- The payload records that stale `adb_screencap` evidence alone must not be classified as game freeze and should be diagnosed before navigation or restart decisions.
- The payload records that lighter capture-backend recovery should be considered before heavy app restart.
- `session bootstrap`, `session api`, `session contract`, `session command-check`, and command capabilities now advertise or classify the capture-policy surface.
- The surface guarantees it does not enqueue, capture, start MaaTouch, touch devices, start apps, start listeners, or read resource repositories.

Live-device and operator validation remain deferred for this round as `requires-live-device`. No live result is faked or marked passed by this implementation.

No trusted remote network listener, TLS implementation, token issuance, UI, scheduler execution behavior, SQLite, OCR/OpenCV, game logic, resource repository access, new capture/input backend, direct ADB input fallback, reconnect loop, app restart, live device action, cooperation-workspace copy, or resource repository sync was added.

## Current ActingLab Unique Throat Policy Surface

This increment turns the Session Layer "唯一咽喉" rule into a stable no-device contract query for UI, scheduler, and agent clients.

- `session throat-policy` returns `session.throat_policy.v0.1`.
- `session request throat-policy` returns the same policy through the resident daemon request path.
- The payload records that Session Layer is the only control throat and that UI, scheduler, and agents must not directly touch adb or devices.
- The payload records strict Session Layer routing controls: `--require-session`, `ACTINGLAB_REQUIRE_SESSION_DAEMON`, and `session_daemon_required`.
- The payload records daemon/control routing policy, lease-gate expectations, trusted-remote reservation, and deferred live acceptance as `requires-live-device`.
- `session bootstrap`, `session api`, `session contract`, `session command-check`, and command capabilities now advertise or classify the throat-policy surface.
- The surface guarantees it does not enqueue, capture, start MaaTouch, touch devices, start apps, start listeners, or read resource repositories.

Live-device and operator validation remain deferred for this round as `requires-live-device`. No live result is faked or marked passed by this implementation.

No trusted remote network listener, TLS implementation, token issuance, UI, scheduler execution behavior, SQLite, OCR/OpenCV, game logic, resource repository access, new capture/input backend, direct ADB input fallback, reconnect loop, app restart, live device action, cooperation-workspace copy, or resource repository sync was added.

## Current ActingLab Client Bootstrap Surface

This increment gives future UI/scheduler clients a single read-only startup envelope instead of requiring them to independently discover the Session Layer contracts, queue state, readiness state, and live-validation deferral policy.

- `session bootstrap` returns `session.bootstrap.v0.1`.
- `session request bootstrap` returns the same schema through the resident daemon request path.
- The payload includes `access_contract`, `api_contract`, `session_layer`, `commands`, `readiness`, `queue`, and `validation_plan`.
- The API/access/capability contracts now advertise the bootstrap entry point.
- `session command-check bootstrap` classifies the bootstrap query as read-only.
- The surface guarantees it does not enqueue, capture, start MaaTouch, touch devices, start apps, start listeners, or read resource repositories.

Live-device and operator validation remain deferred for this round as `requires-live-device`. No live result is faked or marked passed by this implementation.

No trusted remote network listener, TLS implementation, token issuance, UI, scheduler execution behavior, SQLite, OCR/OpenCV, game logic, resource repository access, new capture/input backend, direct ADB input fallback, reconnect loop, app restart, live device action, cooperation-workspace copy, or resource repository sync was added.

## Current ActingLab Live Validation Plan Surface

This increment turns the current live-validation deferral into a stable Session Layer query instead of relying on checkpoint prose. Future UI/scheduler clients can distinguish offline verification progress from live acceptance work without touching devices.

- `session validation-plan` returns `session.validation_plan.v0.1`.
- `session request validation-plan` returns the same schema through the resident daemon request path.
- The payload reports `live_validation_status=deferred` and `deferred_code=requires-live-device`.
- The payload includes `phase_acceptance_matrix` entries for Phase A, Phase B, Phase C, Phase D, and cross-cutting Session Layer work.
- The payload includes `ak_stale_capture_validation`, which records that stale `adb_screencap` output must not be treated as proof of a game freeze by itself and that lighter capture-backend recovery should be considered before app restart.
- The payload includes `pending_live_acceptance` titled `待真机验收`; each item remains `deferred: requires-live-device` and lists the required live environment plus evidence.
- Deferred live tasks include prepared-emulator Session Layer validation, AK stale-capture/fresh-frame recovery validation, live ADB/device/screenshot validation, and operator acceptance observation.
- Offline verification remains allowed for unit tests, contract tests, dry-runs, fixture-frame tests, and static prohibited-feature scans.
- The surface guarantees it does not enqueue, capture, start MaaTouch, touch devices, start apps, start listeners, or read resource repositories.

Live-device and operator validation remain deferred for this round as `requires-live-device`. No live result is faked or marked passed by this implementation.

No trusted remote network listener, TLS implementation, token issuance, UI, scheduler execution behavior, SQLite, OCR/OpenCV, game logic, resource repository access, new capture/input backend, direct ADB input fallback, reconnect loop, app restart, live device action, cooperation-workspace copy, or resource repository sync was added.

## Current ActingLab Command-Check Instance Gate

This increment aligns command preflight with the selected-instance readiness gate. Future UI/scheduler clients can use either `session command-check <command...>` or the aggregated `session submit-plan <command...>` view without accidentally treating an incomplete selected instance as submittable.

- `session command-check` now includes `instance_gate`.
- Device-affecting commands with `--instance <id>` are blocked when the selected instance is missing from the registry or lacks required `serial`, `game`, or `server` configuration.
- Non-device commands and commands without a selected instance keep non-blocking `instance_gate` statuses.
- `session submit-plan` now includes `preflight_summary` with readiness, command, queue, lease, and instance-gate status fields.
- `session api` now advertises `command_check_view.instance_gate_field` and `submit_plan_view.preflight_summary_field`.
- This remains a pure local/daemon preflight surface; it does not enqueue, capture, start MaaTouch, touch devices, start a listener, or read resource repositories.

Live-device and operator validation for Session Layer behavior is deferred for this round as `requires-live-device`. Offline code, contracts, and unit tests continue to move forward without claiming live acceptance.

No trusted remote network listener, TLS implementation, token issuance, UI, scheduler execution behavior, SQLite, OCR/OpenCV, game logic, resource repository access, new capture/input backend, direct ADB input fallback, reconnect loop, app restart, live device action, cooperation-workspace copy, or resource repository sync was added.

## Current ActingLab Readiness Selected-Instance Gate

This increment turns the previous readiness instance summary into an actionable gate for the selected instance. Future UI/scheduler clients can now treat readiness as the single pre-submit source for daemon liveness, transport safety, queue state, and selected-instance usability.

- `session readiness` now reports `instances.selected_status`.
- `selected_status` values are `not_selected`, `ready`, `needs_configuration`, `not_found`, and `registry_unavailable`.
- A selected instance with missing required fields makes `ready=false` and emits an `instance_configuration` blocker.
- A selected instance that is absent from the registry makes `ready=false` and emits an `instance_not_found` blocker.
- `session api` now advertises `instances.selected_status` and `instances.selected_missing_required`.
- The selected-instance gate remains a pure status/config projection; it does not enqueue, capture, start MaaTouch, touch devices, start a listener, or read resource repositories.

No trusted remote network listener, TLS implementation, token issuance, UI, scheduler execution behavior, SQLite, OCR/OpenCV, game logic, resource repository access, new capture/input backend, direct ADB input fallback, reconnect loop, app restart, live device action, cooperation-workspace copy, or resource repository sync was added.

## Current ActingLab Readiness Instance Summary

This increment gives future UI/scheduler clients a compact instance-registry summary inside the existing readiness envelope, so they can decide whether instance configuration is usable without parsing the full embedded status diagnostics payload.

- `session readiness` now includes `instances.schema_version = session.readiness_instances.v0.1`.
- `session request readiness` returns the same instance summary through the resident daemon request path.
- The summary reports registry availability, instance count, status, selected instance, missing required fields, and configured instance entries.
- Instance summary statuses are `unavailable`, `empty`, `ready`, and `needs_configuration`.
- `session api` now advertises the readiness `instances` field and `instances.status` field.
- The readiness instance summary does not enqueue, capture, start MaaTouch, touch devices, start a listener, or read resource repositories.

No trusted remote network listener, TLS implementation, token issuance, UI, scheduler execution behavior, SQLite, OCR/OpenCV, game logic, resource repository access, new capture/input backend, direct ADB input fallback, reconnect loop, app restart, live device action, cooperation-workspace copy, or resource repository sync was added.

## Current ActingLab Submit-Plan Preflight View

This increment gives future UI/scheduler clients one read-only pre-submit envelope instead of requiring them to separately call readiness, command-check, and queue views before deciding whether to submit work to the Session Layer.

- `session submit-plan <command...>` returns `session.submit_plan.v0.1`.
- `session request submit-plan <command...>` returns the same schema through the resident daemon request handler when queue admission allows the diagnostic request.
- The payload embeds the existing readiness, command-check, and queue payloads, reports `ready_to_submit`, and aggregates blockers.
- Queue admission is checked even when the target command's command-check route would otherwise skip the queue gate, so clients get a single conservative submission decision.
- Control commands still require matching Session Layer lease metadata through the existing command-check lease gate.
- The submit-plan query records that it does not enqueue, capture, start MaaTouch, touch devices, or start a listener.
- `session api`, `session contract`, and `capabilities` now advertise the submit-plan view.

No trusted remote network listener, TLS implementation, token issuance, UI, scheduler execution behavior, SQLite, OCR/OpenCV, game logic, resource repository access, new capture/input backend, direct ADB input fallback, reconnect loop, app restart, live device action, cooperation-workspace copy, or resource repository sync was added.

## Current ActingLab Session Queue View

This increment gives future UI/scheduler clients a direct queue inspection surface. It advances the Session Layer unique-throat and admission-safety work by making queue state inspectable without requiring clients to parse the full status diagnostics payload.

- `session queue` returns `session.queue.v0.1`.
- `session request queue` returns the same schema through the resident daemon request handler when queue admission is healthy enough to accept the diagnostic request.
- The payload reports queue counts, full queue health, bounded pending/running/response previews, recommended action kinds, and queue-specific recommended actions.
- The payload includes an `admission` object with `can_enqueue=false` and `request_queue_needs_attention` when pending, running, or response queues need operator/scheduler attention before another request is submitted.
- The local `session queue` query is the blocked-queue inspection path; it does not enqueue, capture, start MaaTouch, touch devices, or start a listener.
- `session api`, `session contract`, and `capabilities` now advertise the queue view.

No trusted remote network listener, TLS implementation, token issuance, UI, scheduler execution behavior, SQLite, OCR/OpenCV, game logic, resource repository access, new capture/input backend, direct ADB input fallback, reconnect loop, app restart, live device action, cooperation-workspace copy, or resource repository sync was added.

## Current ActingLab Command-Check Surface

This increment gives future UI/scheduler clients a command-specific preflight before they submit work to the Session Layer. It advances the lease/arbitration and unique-throat requirements by making safety gates inspectable without executing the target command.

- `session command-check <command...>` returns `session.command_check.v0.1`.
- `session request command-check <command...>` returns the same schema through the resident daemon request handler.
- The payload reports `command_class`, `requires_lease`, `device_affecting`, route state, lease-gate state, blockers, and `safe_to_submit`.
- Control commands such as `tap`, `key`, `text`, `tap-target`, `navigate`, app lifecycle, Lab/package/operation run, and non-stale recovery require a matching Session Layer lease.
- Read-only commands such as status, readiness, capture/recognition/page queries, stale-capture recovery, and stream checks do not require a lease.
- `session command-check` itself remains local and does not auto-enqueue through daemon-preferred routing; use `session request command-check` when checking from the resident request path.
- The check records that it does not enqueue, touch devices, start MaaTouch, capture frames, or start a listener.

No trusted remote network listener, TLS implementation, token issuance, UI, scheduler execution behavior, SQLite, OCR/OpenCV, game logic, resource repository access, new capture/input backend, direct ADB input fallback, reconnect loop, app restart, live device action, cooperation-workspace copy, or resource repository sync was added.

## Current ActingLab Session Readiness Surface

This increment adds a compact readiness envelope for future UI and scheduler clients. It answers whether the Session Layer can accept requests now, while preserving the existing detailed `session status --diagnostics` surface for deeper inspection.

- `session readiness` returns `session.readiness.v0.1`.
- `session request readiness` returns the same readiness schema through the resident daemon request queue.
- The readiness payload includes daemon liveness, `ready`, `status`, blockers, recommended action kinds, full recommended actions, and an embedded status view.
- Optional `--endpoint <url>` runs the existing transport endpoint policy/reachability check and contributes transport blockers when unsafe.
- Stream-preflight availability is advertised with the existing `stream check` / `session request stream check` commands and explicitly records that it does not capture, start MaaTouch, start a listener, or execute input.
- A stopped, stale, missing-heartbeat, or pid-mismatched daemon is reported as `ready=false` and `status=not_ready`; readiness never silently reports success.

No trusted remote network listener, TLS implementation, token issuance, UI, scheduler execution behavior, SQLite, OCR/OpenCV, game logic, resource repository access, new capture/input backend, direct ADB input fallback, reconnect loop, app restart, live device action, cooperation-workspace copy, or resource repository sync was added.

## Current ActingLab Bounded Stream Preflight Surface

This increment advances the interactive stream requirement without implementing the future trusted remote long-lived stream. UI/scheduler clients can now ask whether a bounded stream request is safe to start before capture or input relay execution.

- `stream check` returns `session.stream_check.v0.1`.
- `session stream check` routes to the same local preflight surface.
- `session request stream check` routes through the resident daemon request queue as a read-only request.
- The preflight reports routing, daemon liveness, frame count settings, fresh-frame settings, input-relay actions, and lease-gate status.
- Read-only stream checks do not require a lease.
- Input-relay checks report `safe_to_start=false` when the caller has not supplied a matching Session Layer lease.
- `stream check` does not capture frames, start MaaTouch, execute input, start a network listener, implement TLS/auth, or create a long-lived trusted remote stream.

## Current ActingLab Trusted Transport Preflight Surface

This increment advances the multi-channel access requirement without starting a network service. Future UI/API clients can now ask the Session Layer whether a configured endpoint is local, trusted remote, encrypted, authenticated, and safe to connect to.

- `session transport check --endpoint <url>` returns `session.transport_check.v0.1`.
- Loopback endpoints are classified as `local_direct` and do not require authentication.
- Non-loopback HTTP endpoints are reported as blocked with `trusted_remote_transport_blocked`.
- Trusted remote endpoints still require encrypted transport and token/certificate auth before any connection is considered safe.
- `session request transport check --endpoint <url>` reuses the same preflight payload through the resident daemon request path.
- `session api`, `session transport`, and `capabilities` now advertise the transport-check surface.

No network listener, TLS implementation, token issuance, UI transport, unbounded long-lived stream transport, scheduler execution behavior, SQLite, OCR/OpenCV, game logic, resource repository access, new capture/input backend, direct ADB input fallback, reconnect loop, app restart, live device action, cooperation-workspace copy, or resource repository sync was added.

## Current ActingLab Failed Request Recommended Action Surface

This increment closes another diagnostics gap for future UI and scheduler clients: the latest failed daemon request is no longer only exposed as raw `journal.last_error`, but also as an explicit next diagnostic action.

- `session status --diagnostics` finds the most recent failed daemon request in the recent request journal.
- A recent failed request emits `failed_request_inspect`, pointing to `session request-state get <request-id>`.
- The recommendation includes the failed request id, source command, completion timestamp, original error payload, and `read_only=true`.
- `session api` advertises the journal-error recommendation action in the status-view contract.

No trusted remote network transport, unbounded long-lived stream transport, scheduler execution behavior, UI, SQLite, OCR/OpenCV, game logic, resource repository access, new capture/input backend, direct ADB input fallback, reconnect loop, app restart, live device action, cooperation-workspace copy, or resource repository sync was added.

## Current ActingLab Queue Health Recommended Action Surface

This increment closes a diagnostics gap for future UI and scheduler clients: queue-health states are no longer only raw counters and status strings, but also include safe next actions.

- `session status --diagnostics` computes queue health once and reuses it for both diagnostics output and recommended-action generation.
- Blocked queued requests emit `blocked_request_inspect`, pointing to `session request-state get <request-id>`.
- Blocked running requests emit `blocked_running_request_inspect`, also pointing to `session request-state get <request-id>`.
- Unclaimed responses emit `unclaimed_response_read`, pointing to `session response get <request-id>`.
- Queue-health recommendations include queue kind, request id, queue-health details, and read-only flags.
- `unclaimed_response_read` records `consumes_response=false`; consumers can inspect without deleting response files.
- `session api` advertises the queue-health recommendation actions in the status-view contract.

No trusted remote network transport, unbounded long-lived stream transport, scheduler execution behavior, UI, SQLite, OCR/OpenCV, game logic, resource repository access, new capture/input backend, direct ADB input fallback, reconnect loop, app restart, live device action, cooperation-workspace copy, or resource repository sync was added.

## Current ActingLab Capture Health Recommended Action Surface

This increment connects the AK stale-screencap finding to the Session Layer diagnostics surface. Recent capture-health journal summaries can now become machine-readable next actions without executing recovery automatically.

- `session status --diagnostics` inspects the latest recent `capture_diagnose` or `stale_capture_recovery` data summary from the daemon request journal.
- A latest stale capture signal emits `stale_capture_recover`, pointing to `session recover --stale-capture --capture`.
- A latest capture-unavailable signal emits `capture_backend_health_check`, pointing to `session instance health --capture-diagnose`.
- A newer fresh capture signal suppresses older stale recommendations so UI/scheduler consumers are not misled by stale historical events.
- Recommendations include the source request id, source command, and compact data summary for auditability.
- The recommended actions are read-only and explicitly record that they do not execute app restart.
- `session api` advertises the capture-health recommendation actions in the status-view contract.

No trusted remote network transport, unbounded long-lived stream transport, scheduler execution behavior, UI, SQLite, OCR/OpenCV, game logic, resource repository access, new capture/input backend, direct ADB input fallback, reconnect loop, app restart, live device action, cooperation-workspace copy, or resource repository sync was added.

## Current ActingLab Stale Lease Recommended Action Surface

This increment turns stale lease freshness into a scheduler/UI-facing recommendation without moving arbitration into the Session Layer.

- `session status --diagnostics` now emits `stale_lease_inspect` in `diagnostics.recommended_actions` when a held lease is older than the diagnostic freshness threshold.
- The recommendation points to `session lease status --instance <id>` for inspection.
- The recommendation includes the affected instance, lease holder, lease id, and freshness metadata.
- The recommendation is marked with `requires_scheduler_decision=true`.
- `session api` advertises `stale_lease_inspect` under the status-view lease freshness actions.
- The Session Layer still does not release, preempt, acquire, or mutate leases based on stale freshness.

No trusted remote network transport, unbounded long-lived stream transport, scheduler execution behavior, UI, SQLite, OCR/OpenCV, game logic, resource repository access, new capture/input backend, direct ADB input fallback, reconnect loop, app restart, live device action, cooperation-workspace copy, or resource repository sync was added.

## Current ActingLab Session Lease Freshness Diagnostics

This increment makes the preceding lease touch surface observable: scheduler, Lab, and future UI clients can distinguish recently refreshed leases from stale lease records without scraping timestamps or inventing their own stale threshold.

- `session lease status` now includes a `freshness` object for held leases.
- `session lease list` includes `freshness` per lease and the diagnostic `stale_after_ms` threshold.
- `session status --diagnostics` includes the same `freshness` metadata under `diagnostics.leases`.
- `session api` advertises the lease freshness field, status values, and stale threshold.
- Freshness is diagnostic-only: it does not release, preempt, acquire, or mutate leases.
- Stale lease recovery remains a scheduler/UI decision through the existing inspect/acquire/preempt surfaces.

No trusted remote network transport, unbounded long-lived stream transport, scheduler execution behavior, UI, SQLite, OCR/OpenCV, game logic, resource repository access, new capture/input backend, direct ADB input fallback, reconnect loop, app restart, live device action, cooperation-workspace copy, or resource repository sync was added.

## Current ActingLab Session Lease Touch View

This increment closes a lease freshness observability gap in the Session Layer: scheduler, Lab, and future UI clients can now refresh a held lease's `updated_at_unix_ms` without touching devices or inferring liveness from unrelated command activity.

- `session lease touch` updates only `updated_at_unix_ms` on the matching lease record.
- `session request lease touch` exposes the same touch operation through the resident daemon request queue.
- `lab lease touch` is a thin Lab-facing alias over the same Session Layer lease touch path.
- Touch requires the active holder to match, and optional `--lease-id` must match when supplied.
- Missing leases fail visibly with `lease_not_held`; holder and lease-id mismatches keep the existing safety-blocked errors and do not mutate the lease file.
- `session api` advertises `session.lease_touch.v0.1`, and capabilities advertise the local, daemon-routed, and Lab-facing touch surfaces.

No trusted remote network transport, unbounded long-lived stream transport, scheduler execution behavior, UI, SQLite, OCR/OpenCV, game logic, resource repository access, new capture/input backend, direct ADB input fallback, reconnect loop, app restart, live device action, cooperation-workspace copy, or resource repository sync was added.

## Current ActingLab LabLease List/Wait Alias View

This increment keeps the Lab-facing CLI surface aligned with the Session Layer lease arbitration views: Lab users can inspect and wait on lease ownership without dropping down to raw `session lease ...` commands.

- `lab lease list` is a thin alias over `session lease list`.
- `lab lease wait` is a thin alias over `session lease wait`.
- Both aliases preserve the existing state-dir, holder, lease-holder, lease-id, status, timeout, and poll behavior of the Session Layer lease views.
- `lab lease list` does not require a default configured instance, matching the global lease-list semantics.
- Capabilities now advertise `lab lease list` and `lab lease wait`.

No trusted remote network transport, unbounded long-lived stream transport, scheduler execution behavior, UI, SQLite, OCR/OpenCV, game logic, resource repository access, new capture/input backend, direct ADB input fallback, reconnect loop, app restart, live device action, cooperation-workspace copy, or resource repository sync was added.

## Previous ActingLab Session Lease List View

This increment closes an arbitration observability gap in the Session Layer: scheduler, Lab, and future UI clients can list all active lease records without inferring global state from single-instance status calls or raw `lease-*.json` files.

- `session lease list` reads the local Session Layer state directory and returns `session.lease_list.v0.1`.
- `session request lease list` exposes the same view through the resident daemon request queue.
- The list output includes active lease count, released-during-read count, state directory, filters, and one entry per active lease.
- `--holder`, `--lease-holder`, and `--lease-id` filters allow scheduler/UI consumers to isolate ownership.
- `session lease list` does not require a configured default instance, because it is a global lease view.
- Corrupt lease files fail visibly through the existing JSON parse error path.
- `session api` and `capabilities` advertise the new local and daemon-routed lease list surfaces.

No trusted remote network transport, unbounded long-lived stream transport, scheduler execution behavior, UI, SQLite, OCR/OpenCV, game logic, resource repository access, new capture/input backend, direct ADB input fallback, reconnect loop, app restart, live device action, cooperation-workspace copy, or resource repository sync was added.

## Previous ActingLab Session Lease Wait View

This increment closes a lease-arbitration observability gap in the Session Layer: consumers can wait for an instance lease to become free or to be held by an expected owner without writing custom polling against `lease-*.json`.

- `session lease wait [--status free|held]` waits in the local Session Layer state directory.
- `session request lease wait [--status free|held]` exposes the same wait behavior through the resident daemon request queue.
- The default expected status is `free`, supporting Lab/UI consumers waiting for a scheduler-owned lease to be released before attempting `acquire`.
- `--status held` can be combined with `--holder` or `--lease-holder` and `--lease-id` to wait for a specific owner.
- Timeout returns the current lease-state payload with `wait.timed_out=true`; it does not claim the desired lease state was reached.
- Invalid status filters, invalid poll intervals, and corrupt lease files fail visibly.
- `session lease status` now includes schema `session.lease_status.v0.1` and a machine-readable `status` field.
- `session api` and `capabilities` advertise the new local and daemon-routed lease wait surfaces.

No trusted remote network transport, unbounded long-lived stream transport, scheduler execution behavior, UI, SQLite, OCR/OpenCV, game logic, resource repository access, new capture/input backend, direct ADB input fallback, reconnect loop, app restart, live device action, cooperation-workspace copy, or resource repository sync was added.

## Previous ActingLab Session Request-State Wait View

This increment closes a UI/scheduler polling gap in the Session Layer request lifecycle: clients can now wait for a specific request id to reach an expected lifecycle state without implementing custom file polling.

- `session request-state wait <request-id>` waits in the local Session Layer state directory.
- `session request request-state wait <request-id>` exposes the same wait behavior through the resident daemon request queue.
- The default expected statuses are `response_available`, `completed`, and `failed`.
- Callers can use repeated `--status <state>` filters to wait for `queued`, `running`, `response_available`, `completed`, `failed`, or `unknown`.
- Timeout returns the current request-state payload with `wait.timed_out=true`; it does not invent a successful state or hide that the desired transition did not occur.
- Invalid status filters, invalid request ids, corrupt state files, and invalid poll intervals fail visibly.
- `session api` and `capabilities` advertise the new local and daemon-routed wait surfaces.

No trusted remote network transport, unbounded long-lived stream transport, scheduler execution behavior, UI, SQLite, OCR/OpenCV, game logic, resource repository access, new capture/input backend, direct ADB input fallback, reconnect loop, app restart, live device action, cooperation-workspace copy, or resource repository sync was added.

## Previous ActingLab Session Running Request State View

This increment closes a Session Layer observability gap for future UI and scheduler clients: a daemon request that has been picked up for execution is now distinguishable from merely queued work.

- Resident daemon request processing writes `running/<request-id>.json` before executing the request and removes it after response and journal materialization.
- `session request-state get <request-id>` reports `running` when a running marker is present and no response is available yet.
- `session request-state list [--status running]` includes running requests and counts them separately from queued, response-available, completed, and failed requests.
- `session status --diagnostics` exposes running request count, running request preview, and running request queue health.
- `session request cancel <request-id>` now refuses running requests instead of deleting their pending request file.
- Corrupt running markers fail visibly like corrupt pending requests or pending responses.
- `session api` advertises `running` as an official request-state lifecycle status.
- This phase does not start trusted remote network transport, implement unbounded long-lived stream transport, add scheduler execution behavior, add UI, add SQLite, add OCR/game logic, add capture/input backends, use direct ADB input fallback, run live devices, access resource repositories, or modify cooperation-workspace files.

Validation for this phase:

- Focused Session request-state tests.
- Focused Session request cancel tests.
- Focused Session diagnostics tests.
- Focused daemon journal cleanup test.
- Focused Session API contract test.
- `cargo fmt --all -- --check`
- `git diff --check`
- Added-line prohibited-feature scan over `apps/actinglab/src/main.rs`
- `cargo clippy --workspace -- -D warnings`
- `cargo test --workspace`

## Current ActingLab Session Request Cancel View

This increment adds an explicit queue-management surface for future UI and scheduler clients that submit Session Layer daemon work with `--no-wait` and later need to discard still-pending queued work.

- `session request cancel <request-id> [--reason text]` validates the request id, removes only the matching pending request file, and writes a durable request-journal entry with `ok=false` and error code `request_cancelled`.
- Cancelled requests become observable through the existing `session request-state get/list` and `session events` views as failed journaled requests instead of disappearing silently.
- Requests that already have a pending response, are already completed in the journal, are missing from the queue, or use unsafe request ids fail visibly with `request_not_cancellable` or validation errors.
- Cancel is local queue management and is advertised as an offline `session request cancel` capability; it does not require the resident daemon to be alive and does not submit work back into the daemon queue.
- `session api` advertises the cancel query, `request_cancelled` error code, and journal-recording behavior.
- This phase does not start trusted remote network transport, implement unbounded long-lived stream transport, add scheduler execution behavior, add UI, add SQLite, add OCR/game logic, add capture/input backends, use direct ADB input fallback, run live devices, access resource repositories, or modify cooperation-workspace files.

Validation for this phase:

- Focused Session request cancel tests.
- Focused Session API contract test.
- Focused command capabilities test.
- `cargo fmt --all -- --check`
- `git diff --check`
- Added-line prohibited-feature scan over `apps/actinglab/src/main.rs`
- `cargo clippy --workspace -- -D warnings`
- `cargo test --workspace`

## Current ActingLab Session Events Wait View

This increment adds a bounded events wait surface for future UI and scheduler clients that need to observe Session Layer changes through the existing request-journal event view without implementing their own polling loop.

- `session events wait [--timeout-ms N] [--poll-ms N]` waits within the selected local Session Layer state directory until the filtered event view returns at least one event or the timeout expires.
- `session request events wait [--timeout-ms N] [--poll-ms N]` exposes the same bounded wait behavior through the resident daemon request queue.
- Existing event filters, target selectors, and cursors are preserved: `--limit`, `--after-unix-ms`, `--after-request-id`, `--command`, `--data-summary-kind`, `--status`, `--lease-holder`, plus global `--instance`, `--game`, and `--server`.
- The wait view reuses schema `session.events.v0.1` and adds a `wait` object with completion, timeout, elapsed, and poll metadata.
- A wait timeout is an explicit empty event result with `wait.timed_out=true`, not a fake success with hidden missing data.
- Invalid polling intervals, unknown event subcommands, corrupt request-journal entries, and missing request-id cursors continue to fail visibly.
- `session api` advertises the events wait contract, and `capabilities` advertises local and daemon-routed wait commands.
- This phase does not start trusted remote network transport, implement unbounded long-lived stream transport, add scheduler execution behavior, add UI, add SQLite, add OCR/game logic, add capture/input backends, use direct ADB input fallback, run live devices, access resource repositories, or modify cooperation-workspace files.

Validation for this phase:

- Focused Session events wait tests.
- Focused Session API contract test.
- Focused command capabilities test.
- `cargo fmt --all -- --check`
- `git diff --check`
- Added-line prohibited-feature scan over `apps/actinglab/src/main.rs`
- `cargo clippy --workspace -- -D warnings`
- `cargo test --workspace`

## Current ActingLab Session Response Wait View

This increment adds a bounded response wait surface for future UI and scheduler clients that already have a daemon request id and need to wait for the matching response without implementing their own polling loop.

- `session response wait <request-id> [--timeout-ms N] [--poll-ms N] [--consume]` waits within the selected local Session Layer state directory for a specific pending daemon response file.
- `session request response wait <request-id> [--timeout-ms N] [--poll-ms N] [--consume]` exposes the same wait/read/consume behavior through the resident daemon request queue.
- The wait view reuses schema `session.response.v0.1` and adds a `wait` object with completion, elapsed, timeout, and poll metadata.
- `--consume` still deletes the response file only after successful read, parse, and request-id validation.
- Missing responses after timeout, corrupt response JSON, response id mismatches, invalid request ids, invalid polling intervals, and failed consume deletes fail visibly.
- `session api` advertises the response wait contract, and `capabilities` advertises local and daemon-routed wait commands.
- This phase does not start trusted remote network transport, implement long-lived stream transport, add scheduler execution behavior, add UI, add SQLite, add OCR/game logic, add capture/input backends, use direct ADB input fallback, run live devices, access resource repositories, or modify cooperation-workspace files.

Validation for this phase:

- Focused Session response tests.
- Focused daemon response wait test.
- Focused Session API contract test.
- `cargo fmt --all -- --check`
- `git diff --check`
- Added-line prohibited-feature scan over `apps/actinglab/src/main.rs`
- `cargo clippy --workspace -- -D warnings`
- `cargo test --workspace`

## Current ActingLab Session Request-State List View

This increment adds a bounded aggregate request lifecycle view for future UI and scheduler clients that need to inspect the Session Layer queue without scraping `requests/`, `responses/`, or the request journal directly.

- `session request-state list [--limit N] [--status <state>]` reads the local Session Layer state directory and reports request lifecycle items from pending request files, pending response files, and recent request-journal entries.
- `session request request-state list [--limit N] [--status <state>]` routes the same read-only view through the resident daemon request queue.
- The list view uses schema `session.request_state_list.v0.1`.
- Status filters support `queued`, `response_available`, `completed`, and `failed`.
- Queue files have priority over journal entries for the same request id, so active queued work and unclaimed responses are not hidden by older completed journal records.
- The payload includes status counts, source paths, disappeared-file counters for queue races, compact response summaries, and bounded sorted items.
- `session api` advertises the request-state list contract, and `capabilities` advertises the local and daemon-routed list commands.
- This phase does not start trusted remote network transport, implement long-lived stream transport, add scheduler execution behavior, add UI, add SQLite, add OCR/game logic, add capture/input backends, use direct ADB input fallback, run live devices, access resource repositories, or modify cooperation-workspace files.

Validation for this phase:

- Focused Session request-state list tests.
- Focused daemon request-state list test.
- Focused Session API contract test.
- `cargo fmt --all -- --check`
- `git diff --check`
- Added-line prohibited-feature scan over `apps/actinglab/src/main.rs`
- `cargo clippy --workspace -- -D warnings`
- `cargo test --workspace`

## Current ActingLab Session Request-State View

This increment adds a small request lifecycle lookup surface for future UI and scheduler clients that submit daemon work with `--no-wait`.

- `session request-state get <request-id>` reads the local Session Layer state directory and reports whether the request is queued, has a pending response, is completed or failed in the durable request journal, or is unknown.
- `session request request-state get <request-id>` routes the same read-only view through the resident daemon request queue.
- The request-state view uses schema `session.request_state.v0.1`.
- Request ids are restricted to ASCII letters, digits, `-`, and `_` before any queue or response path is built.
- The payload includes request/response/journal paths, pending request data, pending response data, compact response data summary, and matching journal event data when available.
- `session api` advertises `request_state_view`, and `capabilities` advertises the local and daemon-routed request-state commands.
- This phase does not start trusted remote network transport, implement long-lived stream transport, add scheduler execution behavior, add UI, add SQLite, add OCR/game logic, add capture/input backends, use direct ADB input fallback, run live devices, access resource repositories, or modify cooperation-workspace files.

Validation for this phase:

- Focused Session request-state tests.
- Focused daemon request-state test.
- Focused Session API contract test.
- `cargo fmt --all -- --check`
- `git diff --check`
- Added-line prohibited-feature scan over `apps/actinglab/src/main.rs`
- `cargo clippy --workspace -- -D warnings`
- `cargo test --workspace`

## Current ActingLab Session Request No-Wait Submit

This increment adds the request-submission half of asynchronous Session Layer file-IPC consumption for future UI and scheduler clients.

- `session request <command> --no-wait` writes the request JSON to the existing daemon request queue and returns immediately.
- The returned payload includes status `queued`, request id, request path, response path, and suggested `session response get` / `session response get --consume` commands.
- Default `session request <command>` behavior remains synchronous: it waits for the response up to `--request-timeout-ms` and consumes the response on success.
- `--no-wait` is treated as a client-only flag and is stripped before the request payload reaches daemon command execution.
- The `session api` contract now documents `sync_wait` and `no_wait` submit modes.
- `capabilities` advertises `session request --no-wait`.
- This phase does not start trusted remote network transport, implement long-lived stream transport, add scheduler execution behavior, add UI, add SQLite, add OCR/game logic, add capture/input backends, use direct ADB input fallback, run live devices, access resource repositories, or modify cooperation-workspace files.

Validation for this phase:

- Focused no-wait Session request tests.
- Focused client-only payload stripping test.
- Focused Session API contract test.
- `cargo fmt --all -- --check`
- `git diff --check`
- Added-line prohibited-feature scan over `apps/actinglab/src/main.rs`
- `cargo clippy --workspace -- -D warnings`
- `cargo test --workspace`

## Current ActingLab Session Response View

This increment gives future UI and scheduler clients a direct way to inspect or claim a specific daemon response file after it has been left in the Session Layer response queue.

- `session response get <request-id>` reads a response from the selected session state directory without deleting it.
- `session response get <request-id> --consume` deletes the response file only after it has been read, parsed, and request-id checked.
- `session request response get <request-id>` lets the resident daemon execute the same response-view logic through the existing serialized request queue.
- The response view uses schema `session.response.v0.1`.
- Request ids for direct response-file lookup are restricted to ASCII letters, digits, `-`, and `_` to avoid path traversal or accidental arbitrary-file reads.
- Missing responses, corrupt response JSON, and response id mismatches fail visibly instead of returning empty success data.
- `session api` advertises the `response_view` contract, and `capabilities` advertises the local and daemon-routed response commands.
- This phase does not start trusted remote network transport, implement long-lived stream transport, add scheduler execution behavior, add UI, add SQLite, add OCR/game logic, add capture/input backends, use direct ADB input fallback, run live devices, access resource repositories, or modify cooperation-workspace files.

Validation for this phase:

- Focused Session response tests.
- Focused daemon response-view test.
- Focused Session API contract test.
- `cargo fmt --all -- --check`
- `git diff --check`
- Added-line prohibited-feature scan over `apps/actinglab/src/main.rs`
- `cargo clippy --workspace -- -D warnings`
- `cargo test --workspace`

## Current ActingLab Session Queue Health Diagnostics

This increment turns the request/response queue previews into an explicit health summary for future UI/scheduler consumers.

- `session status --diagnostics` now includes `diagnostics.queues.health`.
- The health payload uses schema `session.queue_health.v0.1`.
- Queue health reports overall status as `clear`, `active`, or `needs_attention`.
- Pending request health reports `clear`, `pending`, or `blocked`.
- Pending response health reports `clear`, `available`, or `unclaimed`.
- The health threshold reuses the daemon request timeout default, currently `10_000 ms`, instead of introducing a separate hidden threshold.
- Health summaries include the oldest pending request/response ids, commands, timestamps, and ages.
- `session api` advertises `diagnostics.queues.health` as part of the status view contract.
- This phase does not start trusted remote network transport, implement long-lived stream transport, add scheduler execution behavior, add UI, add SQLite, add OCR/game logic, add capture/input backends, use direct ADB input fallback, run live devices, access resource repositories, or modify cooperation-workspace files.

Validation for this phase:

- Focused queue diagnostics test.
- Focused clear queue health test.
- Focused Session API contract test.
- `cargo fmt --all -- --check`
- `git diff --check`
- Added-line prohibited-feature scan over `apps/actinglab/src/main.rs`
- `cargo clippy --workspace -- -D warnings`
- `cargo test --workspace`

## Current ActingLab Pending Response Diagnostics

This increment completes the basic daemon queue observability pair for future UI/scheduler consumers.

- `session status --diagnostics` now includes `diagnostics.queues.pending_response_preview`.
- The preview uses schema `session.pending_responses.v0.1`.
- The preview is bounded to the first 10 pending response JSON files sorted by file name.
- Each preview entry includes request id, command, completion status, error, data summary, and start/completion timestamps.
- Queue files that disappear during client consumption are counted as `disappeared_during_read`.
- Corrupt pending response JSON fails visibly with `runtime_not_running`, matching corrupt journal and pending request diagnostics behavior.
- `session api` advertises `diagnostics.queues.pending_response_preview` as part of the status view contract.
- This phase does not start trusted remote network transport, implement long-lived stream transport, add scheduler execution behavior, add UI, add SQLite, add OCR/game logic, add capture/input backends, use direct ADB input fallback, run live devices, access resource repositories, or modify cooperation-workspace files.

Validation for this phase:

- Focused queue diagnostics test.
- Focused corrupt pending response diagnostics test.
- Focused Session API contract test.
- `cargo fmt --all -- --check`
- `git diff --check`
- Added-line prohibited-feature scan over `apps/actinglab/src/main.rs`
- `cargo clippy --workspace -- -D warnings`
- `cargo test --workspace`

## Current ActingLab Pending Request Diagnostics

This increment improves the Session Layer health surface without touching devices, resources, UI, or runtime execution.

- `session status --diagnostics` now includes `diagnostics.queues.pending_request_preview`.
- The preview uses schema `session.pending_requests.v0.1`.
- The preview is bounded to the first 10 pending request JSON files sorted by file name.
- Each preview entry includes request id, command, target selector, lease metadata, creation time, and argument count.
- Queue files that disappear during daemon consumption are counted as `disappeared_during_read`.
- Corrupt pending request JSON fails visibly with `runtime_not_running`, matching corrupt journal diagnostics behavior.
- `session api` advertises `diagnostics.queues.pending_request_preview` as part of the status view contract.
- This phase does not start trusted remote network transport, implement long-lived stream transport, add scheduler execution behavior, add UI, add SQLite, add OCR/game logic, add capture/input backends, use direct ADB input fallback, run live devices, access resource repositories, or modify cooperation-workspace files.

Validation for this phase:

- Focused queue diagnostics test.
- Focused corrupt pending request diagnostics test.
- Focused Session API contract test.
- `cargo fmt --all -- --check`
- `git diff --check`
- Added-line prohibited-feature scan over `apps/actinglab/src/main.rs`
- `cargo clippy --workspace -- -D warnings`
- `cargo test --workspace`

## Current ActingLab Target-Scoped Journal View

This increment gives future UI and scheduler clients a raw request-journal diagnostic view with the same target slicing as the event stream.

- `session journal` supports repeatable `--command`, `--data-summary-kind`, `--status completed|failed`, and `--lease-holder` filters.
- `session journal` also inherits global `--instance`, `--game`, and `--server` selectors as target filters.
- `session request journal` supports the same filters through the resident daemon request path.
- The journal payload includes `command_filter`, `data_summary_kind_filter`, `status_filter`, and `target_filter` for auditability.
- Filtered journal reads expand their internal read window to the recent 1000 entries before applying filters, then return the requested `--limit`.
- The same `SessionEventFilters` logic powers both journal and event matching, reducing drift between raw diagnostics and event-stream views.
- `session api` advertises the journal filters and `entries[].global` selector field.
- This phase does not store full response payloads in the request journal, start trusted remote network transport, implement long-lived stream transport, add scheduler behavior, add UI, add SQLite, add OCR/game logic, add capture/input backends, use direct ADB input fallback, run live devices, access resource repositories, or modify cooperation-workspace files.

Validation for this phase:

- Focused journal filter test.
- Focused request journal compatibility/rotation tests.
- Focused Session events test set.
- Session API contract test.
- `cargo fmt --all -- --check`
- `git diff --check`
- Added-line prohibited-feature scan over `apps/actinglab/src/main.rs`
- `cargo clippy --workspace -- -D warnings`
- `cargo test --workspace`

## Current ActingLab Target-Scoped Event Stream

This increment moves the Session Layer event surface closer to the unique-throat model in `TASK-Lab-session-layer.md`: future UI and scheduler clients can read the event slice for the instance or lease they own instead of scanning every daemon request.

- New request journal entries preserve the request `global` selector metadata: instance, game, server, resource root, capture backend, and dry-run state.
- `session events` inherits global `--instance`, `--game`, and `--server` selectors as target filters.
- `session request events` supports the same selector filters through the resident daemon request path because daemon requests already carry `SessionCommandGlobal`.
- `session events --lease-holder <holder>` filters events by lease holder and is repeatable.
- Event payloads include `events[].global` and `target_filter` so consumers can audit why an event matched.
- Older journal entries without selector metadata remain readable; selector filters only match entries with matching recorded selectors.
- Cursor handling is unchanged: `--after-request-id` is resolved against the complete recent journal before command, data-summary, status, or target filters are applied.
- `session api` advertises global filters and lease-holder filtering.
- This phase does not store full response payloads in the request journal, start trusted remote network transport, implement long-lived stream transport, add scheduler behavior, add UI, add SQLite, add OCR/game logic, add capture/input backends, use direct ADB input fallback, run live devices, access resource repositories, or modify cooperation-workspace files.

Validation for this phase:

- Focused instance-selector event filter test.
- Focused lease-holder event filter test.
- Focused Session events test set.
- Request journal compatibility/rotation tests.
- Session API contract test.
- `cargo fmt --all -- --check`
- `git diff --check`
- Added-line prohibited-feature scan over `apps/actinglab/src/main.rs`
- `cargo clippy --workspace -- -D warnings`
- `cargo test --workspace`

## Current ActingLab Request-Status Event Filter

This increment narrows the Session Layer event view for future UI/scheduler clients that need failure-first polling or success-only confirmation windows.

- `session events --status completed|failed` filters daemon request events by stable event status.
- The filter is repeatable for clients that need both explicit status classes in one request.
- Unsupported status values fail visibly with validation errors instead of returning fake empty results.
- Cursor handling is unchanged: `--after-request-id` is resolved against the complete recent journal before command, data-summary, or status filters are applied.
- `session request events` supports the same filter through the resident daemon request path.
- `session api` advertises `--status`, the allowed status values, and repeatable filter support.
- This phase does not store full response payloads in the request journal, start trusted remote network transport, implement long-lived stream transport, add scheduler behavior, add UI, add SQLite, add OCR/game logic, add capture/input backends, use direct ADB input fallback, run live devices, access resource repositories, or modify cooperation-workspace files.

Validation for this phase:

- Focused Session events status filter test.
- Focused invalid status filter test.
- Focused Session events test set.
- Session API contract test.
- `cargo fmt --all -- --check`
- `git diff --check`
- Added-line prohibited-feature scan over `apps/actinglab/src/main.rs`
- `cargo clippy --workspace -- -D warnings`
- `cargo test --workspace`

## Current ActingLab Data-Summary Event Filter

This increment makes the Session Layer event view easier for future UI/scheduler clients to consume without reading the full request journal.

- `session events --data-summary-kind <kind>` filters daemon request events by `events[].data_summary.kind`.
- The filter is repeatable for clients that need multiple summary classes in one poll.
- Cursor handling is unchanged: `--after-request-id` is resolved against the complete recent journal before command or data-summary filters are applied.
- `session request events` supports the same filter through the resident daemon request path.
- `session api` advertises `--data-summary-kind` and records that the filter is repeatable.
- The intended summary kinds are `stream`, `capture_diagnose`, and `stale_capture_recovery`.
- This phase does not store full response payloads in the request journal, start trusted remote network transport, implement long-lived stream transport, add scheduler behavior, add UI, add SQLite, add OCR/game logic, add capture/input backends, use direct ADB input fallback, run live devices, or access resource repositories.

Validation for this phase:

- Focused Session events data-summary-kind filter test.
- Focused Session events test set.
- Session API contract test.
- `cargo fmt --all -- --check`
- `git diff --check`
- Added-line prohibited-feature scan over `apps/actinglab/src/main.rs`
- `cargo clippy --workspace -- -D warnings`
- `cargo test --workspace`

## Current ActingLab Capture Diagnosis Event Summaries

This increment carries the AK stale-screenshot finding into the Session Layer event surface without adding new capture behavior.

- Successful `capture_diagnose` daemon requests now write a compact `data_summary` with status, requested backend, freshness, attempt count, frame presence, and recovery recommendation summary.
- Successful `recover` daemon requests whose response mode is `stale_capture_recovery` now write a compact `data_summary` with diagnosis status, requested backend, fresh-delay timing, and recovery recommendation summary.
- `session events` and `session request events` expose these summaries through the existing `events[].data_summary` field.
- `session api` advertises the supported data summary kinds: `stream`, `capture_diagnose`, and `stale_capture_recovery`.
- Failed requests and unrelated recovery requests do not write response data summaries.
- This phase does not store full response payloads in the request journal, start trusted remote network transport, implement long-lived stream transport, add scheduler behavior, add UI, add SQLite, add OCR/game logic, add capture/input backends, use direct ADB input fallback, run live devices, or access resource repositories.

Validation for this phase:

- Focused capture diagnosis summary test.
- Session API contract test.
- `cargo fmt --all -- --check`
- `git diff --check`
- Added-line prohibited-feature scan over `apps/actinglab/src/main.rs`
- `cargo clippy --workspace -- -D warnings`
- `cargo test --workspace`

## Current ActingLab Session Request Data Summary

This increment makes daemon request events more useful for future UI/API stream consumers while keeping journal storage bounded.

- Successful daemon-routed `stream` requests now write a compact `data_summary` into the request journal.
- `session events` and `session request events` expose `events[].data_summary` for journaled stream requests.
- The stream summary includes `stream_id`, mode, event count, frame count, input relay status, capture dry-run/require-fresh flags, and trusted-channel status.
- Failed requests and non-stream requests do not write response data summaries.
- `session api` advertises `events[].data_summary` as the event-view summary field.
- This phase does not store full response payloads in the request journal, start trusted remote network transport, implement long-lived stream transport, add scheduler behavior, add UI, add SQLite, add OCR/game logic, add new capture/input backends, use direct ADB input fallback, run live devices, or access resource repositories.

Validation for this phase:

- Focused request journal and Session events tests.
- Session API contract test.
- `cargo fmt --all -- --check`
- `git diff --check`
- Added-line prohibited-feature scan over `apps/actinglab/src/main.rs`
- `cargo clippy --workspace -- -D warnings`
- `cargo test --workspace`

## Current ActingLab Session Events Command Filter

This increment improves the event-consumption side of the Session Layer API without starting a network listener or implementing UI code.

- `session events --command <name>` filters request-journal events by command.
- `--command` is repeatable for future clients that need several command slices.
- `--after-request-id` still locates the cursor in the complete journal first, then applies the command filter, so filtered polling can resume after a non-matching cursor request.
- `session request events` supports the same command filter through the resident daemon request path.
- `session api` advertises `--command` as an event-view filter and records that the filter is repeatable.
- This phase does not add trusted remote network transport, long-lived stream transport, scheduler implementation, UI, SQLite, OCR, game logic, new capture/input backends, direct ADB input fallback, live device action, or resource repository access.

Validation for this phase:

- Focused `session_events` tests.
- Session API contract test.
- `cargo fmt --all -- --check`
- `git diff --check`
- Added-line prohibited-feature scan over `apps/actinglab/src/main.rs`
- `cargo clippy --workspace -- -D warnings`
- `cargo test --workspace`

## Current ActingLab Monitor-Policy Lease Recommendation Surface

This increment keeps scheduler ownership intact while making lease-deferred monitor recovery actionable for future UI and scheduler clients.

- `session status --diagnostics` now includes monitor-policy lease actions in `diagnostics.recommended_actions` when the latest recovery result is `deferred_by_lease`.
- Missing active leases recommend `monitor_policy_inspect_lease` followed by scheduler-owned `monitor_policy_acquire_lease`.
- Holder or lease-id mismatches recommend `monitor_policy_inspect_lease` followed by scheduler-owned `monitor_policy_preempt_lease`.
- Every monitor-policy lease action includes the deferral reason, affected instance, target command arguments, priority, and `requires_scheduler_decision = true`.
- `session api` advertises that status-view clients should consume `diagnostics.recommended_actions` and lists the monitor-policy lease action names.
- This phase does not add scheduler implementation, UI, SQLite, OCR, game logic, new capture/input backends, direct ADB input fallback, app restart behavior, live device action, or resource repository access.

Validation for this phase:

- Focused monitor policy and status-diagnostics recommendation tests.
- Session API/access contract tests.
- `cargo fmt --all -- --check`
- `git diff --check`
- Added-line prohibited-feature scan over `apps/actinglab/src/main.rs`
- `cargo clippy --workspace -- -D warnings`
- `cargo test --workspace`

## Current ActingLab Lease-Deferred Daemon Monitor Recovery Coordination

This increment tightens Session Layer Phase C coordination with scheduler-owned arbitration without implementing the scheduler itself.

- Recovery-capable monitor policies still require stored lease metadata at configuration time.
- When daemon-owned monitoring diagnoses a non-healthy state, recovery is attempted only if the current active lease matches the stored policy lease.
- If the active lease is missing, held by another holder, or has a mismatched lease id, monitor state records `last_recovery.status = deferred_by_lease` and `executed = false`.
- A lease deferral is visible machine-readable state, not a fake recovery success and not an attempt to click without ownership.
- Matching leases still allow the existing maintenance-only `session recover` path to run, including dry-run planning.
- This phase does not add scheduler implementation, UI, SQLite, OCR, game logic, new capture/input backends, direct ADB input fallback, app restart behavior, or resource repository access.

Validation for this phase:

- Focused monitor policy, Session API/access, and capability tests.
- `cargo fmt --all -- --check`
- `git diff --check`
- Added-line prohibited-feature scan over `apps/actinglab/src/main.rs`
- `cargo clippy --workspace -- -D warnings`
- `cargo test --workspace`

## Current ActingLab Lease-Gated Daemon Monitor Recovery Policy

This increment keeps the Session Layer monitor policy daemon-owned while adding a guarded maintenance recovery path.

- `session monitor-policy set --recover` now requires stored lease metadata (`--lease-holder`/`--holder` plus optional `--lease-id`) before the policy can be saved.
- The daemon monitor tick still runs diagnosis first, then validates the active lease before invoking the existing `session recover` path for non-healthy states.
- Monitor state now records either `last_recovery` or `last_recovery_error`, so recovery failures are visible instead of silently downgrading to a successful monitor observation.
- The policy payload advertises that recovery requires a matching lease and that normal monitor policy remains read-only by default.
- This phase does not add scheduler implementation, UI, SQLite, OCR, game logic, new capture/input backends, direct ADB input fallback, app restart behavior, or resource repository access.

Validation for this phase:

- `cargo fmt --all -- --check`
- `git diff --check`
- Added-line prohibited-feature scan over `apps/actinglab/src/main.rs`
- `cargo clippy --workspace -- -D warnings`
- `cargo test --workspace`
- ActingLab bounded stream event envelope: `stream` now emits a `stream_id`, `session.stream.event.v0.1` event records, and stable event indexes for future UI/API stream consumers.
- ActingLab Session transport contract: `session transport` and `session request transport` expose `session.transport.v0.1`, describing local CLI, resident daemon file-IPC, reserved trusted remote, and interactive stream transport boundaries.
- ActingLab trusted remote endpoint policy: runtime endpoint use now distinguishes local direct endpoints from trusted remote endpoints, blocks unencrypted remote endpoints, requires explicit trusted remote auth material, and reports the policy through `doctor` and Session contracts.
- ActingLab strict Session throat policy: `--require-session` and `ACTINGLAB_REQUIRE_SESSION_DAEMON` force device/control commands through an alive resident Session daemon or fail visibly with `session_daemon_required`.
- ActingLab session instance capture health diagnostics: `session instance health --capture-diagnose` reports fresh-frame status, backend attempts, frame digest, and stale-capture recovery recommendations through the Session Layer health surface.
- ActingLab session status instance registry diagnostics: `session status --diagnostics` and daemon-routed status diagnostics now expose configured instance summaries for future UI/scheduler health views.
- ActingLab instance registry backend fields: instance config now stores `adb_path` and `capture_backend`; status/list diagnostics expose them; capture commands use instance default backend unless CLI `--capture-backend` overrides it.
- ActingLab session instance registry contract: `session instance registry` now exposes a machine-readable `session.instance_registry.v0.1` config contract with required/recommended fields, effective capture backend, configured flags, and validation diagnostics for future UI/scheduler consumers.
- ActingLab daemon-routed instance registry contract advertisement: Session capabilities, access contract, and API contract now explicitly advertise `session request instance registry`, and daemon request tests verify the resident queue can return the registry contract.
- ActingLab session instance keep-alive surface: `session instance keep-alive` now exposes an explicit no-click instance reachability probe, and Session capabilities, access contract, API contract, and daemon request naming advertise `session request instance keep-alive` for future UI/scheduler consumers.
- ActingLab session instance health contract surface: Session access and API contracts now expose `session request instance health` and an `instance_health_view` so UI/scheduler clients can discover the existing health and optional capture-diagnosis payload.
- ActingLab session app lifecycle contract surface: Session access and API contracts now expose `session request app <launch|stop|restart>` as a lease-gated app lifecycle control surface for future UI/scheduler consumers.
- ActingLab session instance connect lifecycle surface: `session instance connect` now completes the explicit connect/reconnect/keep-alive Phase A instance lifecycle surface, advertises `session request instance connect`, and routes daemon usage through the same lease-gated control path as reconnect.
- ActingLab session instance app lifecycle alias: `session instance app <launch|stop|restart>` now matches the Session Layer task contract while reusing the existing lease-gated `session app <launch|stop|restart>` implementation.
- ActingLab capture backend CLI alias: `--backend <auto|adb|droidcast_raw|nemu_ipc>` now matches the Session Layer task contract as a thin alias of the existing `--capture-backend` option.
- ActingLab app force-stop lifecycle alias: `session app force-stop` and `session instance app force-stop` now match the Phase A lifecycle wording while reusing the existing force-stop implementation behind the lease-gated app lifecycle path; workspace validation also hardened recognition-pack test temp-dir uniqueness for Windows parallel test stability.
- ActingLab stream transport/API contract truthfulness: `session transport`, `session api`, and `stream` now distinguish available bounded local stream/per-request input relay surfaces from the still-reserved trusted remote long-lived stream.
- ActingLab stale capture recovery diagnostic execution: `session recover --stale-capture --capture` can now run the fresh-frame probe and return evidence-backed recovery advice without clicking, restarting, or opening MaaTouch.
- ActingLab stale capture recovery read-only routing: stale-capture recovery is now classified as a read-only Session Layer diagnostic surface in contracts, capabilities, top-level routing, and `session request recover --stale-capture`; ordinary `session recover` remains lease-gated control.
- ActingLab monitor stale-capture diagnosis integration: `monitor --capture --require-fresh` can now report `capture_stale_suspected` or `capture_unavailable` as structured monitor states, with stale capture pointing to the read-only `session recover --stale-capture --capture` path.
- ActingLab daemon-owned read-only monitor policy invocation: the resident daemon can store a read-only `monitor --once` policy, run it on its own interval, and persist the latest monitor state without clicking, recovering, or restarting apps.

## Current ActingLab Daemon-Owned Read-only Monitor Policy Invocation

The current Runtime task advances Session Layer Phase C from ad-hoc monitor calls toward resident daemon ownership. The daemon can now store a monitor policy under the session state directory and run read-only `monitor --once` diagnostics on its own tick. This is the first automatic daemon-owned observation loop; it intentionally does not execute recovery yet.

Scope:

- Add `session monitor-policy set|status|clear` for local session state.
- Add `session request monitor-policy ...` so daemon/API clients can inspect or update the same policy through the resident request queue.
- Store policy in `monitor-policy.json` and latest result in `monitor-state.json`.
- Run due policies from the resident daemon loop and write success/failure results into monitor state.
- Expose monitor policy status through `session status --diagnostics`, capabilities, Session access contract, and Session API contract.
- Reject `--recover` and multi-iteration monitor arguments in policy storage so the daemon loop stays read-only in this milestone.

Safety direction:

- This is daemon-owned observation only.
- It does not click, launch MaaTouch, reconnect, change capture backend configuration, restart apps, run startup-login recovery, implement scheduler arbitration, add UI, add SQLite, add OCR/OpenCV, touch resource repositories, or add game logic.
- Monitor failures are recorded in `monitor-state.json` as visible errors; they are not converted into fake healthy state.

Validation status:

- Focused monitor-policy tests passed.
- Session API/access/transport/capability contract tests passed.
- `cargo fmt --all -- --check`, `git diff --check`, added-line prohibited-feature scan, `cargo clippy --workspace -- -D warnings`, and `cargo test --workspace` passed.

## Previous ActingLab Monitor Stale-Capture Diagnosis Integration

The current Runtime task connects the AK stale-frame finding into the Session Layer monitor surface. Fresh-frame failure during monitor capture is no longer only a capture-stage fatal path for the monitor caller; it becomes a structured monitor diagnosis that future scheduler/UI clients can consume.

Scope:

- Let `monitor --capture --require-fresh` preserve fresh-frame probe results inside `scene_source`.
- Return `status=capture_stale_suspected` when the probe sees unchanged frames and no fresh frame is available for page detection.
- Return `status=capture_unavailable` when requested capture backends cannot provide probe frames.
- For stale capture, expose a read-only recovery recommendation for `session recover --stale-capture --capture`.
- For capture unavailable, expose `session instance health --capture-diagnose` as the next diagnostic path instead of pretending recovery executed.
- Keep normal page-based monitor statuses and maintenance recovery behavior unchanged.

Safety direction:

- This is monitor diagnosis and recovery-routing work only.
- It does not click, launch MaaTouch, change capture backend configuration, reconnect, restart apps, implement a scheduler loop, touch resources, add UI, add SQLite, add OCR/OpenCV, or add game logic.
- `monitor --recover` uses the stale-capture recovery entry only when the monitor diagnosis is `capture_stale_suspected`; `capture_unavailable` remains a visible non-executed recovery blocker.

Validation status:

- Focused monitor recovery JSON and stale-capture recover-argument tests passed.
- Existing `monitor_` test family passed.
- `cargo fmt --all -- --check`, `git diff --check`, added-line prohibited-feature scan, `cargo clippy --workspace -- -D warnings`, and `cargo test --workspace` passed.
- The first workspace test attempt surfaced one transient `current_page_resolves_semantic_page` failure; single-test rerun and the second full workspace run passed.

## ActingLab Stale Capture Recovery Read-only Routing

The current Runtime task tightens the Session Layer contract around AK stale-frame recovery. Stale capture recovery is a diagnosis/planning surface and must not be treated like maintenance navigation recovery unless it is explicitly changed to execute input or app lifecycle actions in a later milestone.

Scope:

- Route `session recover --stale-capture` through daemon read-only request handling when a resident Session daemon is available.
- Keep `session recover --stale-capture --local` as the explicit local override outside strict Session-throat mode.
- Route `session request recover --stale-capture` as a read-only daemon request without LabLease metadata.
- Keep ordinary `session recover` and maintenance recovery daemon requests lease-gated.
- Expose `stale_capture_recovery_view` in `session api`.
- List `session recover --stale-capture` under read-only examples and capabilities.

Safety direction:

- `session recover --stale-capture` remains `executed=false`, `click_allowed=false`, and `app_restart_executed=false`.
- `--capture` / `--diagnose` may run fresh-frame diagnosis, but must not click, launch MaaTouch, reconnect, switch backend configuration, restart apps, or write resources.
- Heavy app restart stays behind ordinary lease-gated lifecycle/recovery controls.

Validation status:

- Focused contract, API, stale-capture no-lease, and maintenance-recover lease-gate tests passed.
- `cargo fmt --all -- --check`, `git diff --check`, added-line prohibited-feature scan, `cargo clippy --workspace -- -D warnings`, and `cargo test --workspace` passed.
- CLI smoke confirmed `session recover --stale-capture` remains `executed=false`, `click_allowed=false`, and `app_restart_executed=false`.
- CLI smoke confirmed `session api` exposes `stale_capture_recovery_view` with `requires_lease=false` and `executes_app_restart=false`.

## ActingLab Stale Capture Recovery Diagnostic Execution

The current Runtime task advances the AK stale-frame finding from a static recovery plan to an optional read-only diagnostic execution path.

Scope:

- Keep `session recover --stale-capture` as a no-device static plan by default.
- Add `--capture` / `--diagnose` to run the existing fresh-frame probe from the stale-capture recovery entry point.
- Report `diagnosed_fresh`, `diagnosed_stale`, or `diagnosis_unavailable` based on probe evidence.
- Preserve the existing recommendation ordering: fresh probe, faster capture backends, device health, and only then heavy `session app restart`.
- Keep daemon-side stale-capture recovery compatible with lease-free planning/diagnosis because no input or restart is executed.

Safety direction:

- This is a read-only diagnosis enhancement.
- It does not click, start MaaTouch, switch capture backend configuration, reconnect, restart the app, run a scheduler loop, touch resource repositories, add UI, add SQLite, add OCR/OpenCV, or add game logic.
- Heavy app restart remains a separate lease-gated lifecycle command.

Validation status:

- Focused stale-capture recovery and capture-diagnosis tests passed.
- CLI smoke confirmed default `session recover --stale-capture` remains a no-device plan with `diagnosis_executed=false` and `app_restart_executed=false`.
- `cargo fmt --all -- --check`, `git diff --check`, added-line prohibited-feature scan, `cargo clippy --workspace -- -D warnings`, and `cargo test --workspace` passed.

## Current ActingLab Stream Transport/API Contract Truthfulness

The current Runtime task aligns the machine-readable Session Layer contracts with the implementation that already exists today.

Scope:

- Mark the bounded local CLI stream surface as available in `session transport`.
- Mark daemon-routed bounded stream requests as available while preserving lease requirements for input relay.
- Mark per-request stream input relay as available, with actions `tap`, `swipe`, `long-tap`, `key`, and `text`.
- Keep trusted remote long-lived stream transport explicitly reserved.
- Add a `stream_view` envelope to `session api` so future UI/scheduler clients can discover the bounded stream schema and relay lease rules.
- Add explicit availability and non-long-lived relay fields to `stream` output.

Safety direction:

- This is a contract alignment only.
- It does not implement a network listener, TLS, token issuance, UI transport, scheduler behavior, daemon queue semantics, capture backend behavior, input backend behavior, SQLite, OCR/OpenCV, resource access, or game logic.
- It does not turn the bounded local stream into a persistent trusted remote channel.
- Control-capable stream input relay remains lease-gated when routed through the daemon.

Validation status:

- Focused `session_transport_request_returns_transport_contract`, `session_api_request_returns_api_contract`, and `stream_` tests passed.
- CLI smoke confirmed `session transport`, `session api`, and `stream --dry-run` expose the corrected stream contract fields.
- `cargo fmt --all -- --check`, `git diff --check`, added-line prohibited-feature scan, `cargo clippy --workspace -- -D warnings`, and `cargo test --workspace` passed.

## Current ActingLab App Force-Stop Lifecycle Alias

The current Runtime task closes the remaining app lifecycle wording gap from the Session Layer task draft, which explicitly calls out `force-stop`.

Scope:

- Add `session app force-stop` as an alias for the existing stop lifecycle operation.
- Add `session instance app force-stop` through the existing instance app lifecycle alias path.
- Preserve `session app stop` and `session instance app stop` for compatibility.
- Expose `force-stop` in capabilities, `session contract`, `session api`, and Session Layer control examples.
- Keep daemon-routed force-stop requests lease-gated before app/device I/O.

Safety direction:

- This is a lifecycle wording alias only.
- It reuses the existing `adb.force_stop` implementation that already powered `session app stop`.
- It does not change package resolution, app launch/restart behavior, ADB path selection, device backend behavior, capture backend behavior, daemon queue semantics, resource repositories, UI code, scheduler implementation, SQLite, OCR/OpenCV, or game logic.
- Explicit `--via-daemon` requests still fail with `runtime_not_running` when no alive resident daemon exists instead of falling back to local execution.
- During workspace validation, a recognition-pack test-only temp directory race was fixed by adding a monotonic test sequence to temp directory names; recognition-pack runtime behavior was not changed.

Validation status:

- Focused lease-gate, daemon-route, access contract, API contract, and capability tests passed.
- CLI smoke confirmed capabilities expose `session app force-stop` and `session instance app force-stop`.
- CLI smoke confirmed both force-stop daemon routes fail visibly with `runtime_not_running` when no daemon is present.
- `cargo fmt --all -- --check`, `git diff --check`, added-line prohibited-feature scan, `cargo clippy --workspace -- -D warnings`, and `cargo test --workspace` passed.

## Current ActingLab Capture Backend CLI Alias

The current Runtime task closes a CLI wording mismatch in the Session Layer task draft, which documents `capture --backend ...`.

Scope:

- Add `--backend <auto|adb|droidcast_raw|nemu_ipc>` as an alias of `--capture-backend <auto|adb|droidcast_raw|nemu_ipc>`.
- Preserve existing global parsing behavior, so the alias works before or after the subcommand.
- Preserve existing backend priority: CLI backend option, then configured instance `capture_backend`, then `auto`.
- Expose the alias in `help`.

Safety direction:

- This is a CLI compatibility alias only.
- It does not change capture backend implementation, fresh-frame probing, backend ordering, daemon queue behavior, device/backend code, resource repositories, UI code, scheduler implementation, SQLite, OCR/OpenCV, or game logic.
- It does not add any fallback loop or retry behavior; existing severe capture errors still fail visibly.

Validation status:

- Focused parser and help tests passed.
- CLI smoke confirmed `help` lists the `--backend` alias.
- `cargo fmt --all -- --check`, `git diff --check`, added-line prohibited-feature scan, `cargo clippy --workspace -- -D warnings`, and `cargo test --workspace` passed.

## Current ActingLab Session Instance App Lifecycle Alias

The current Runtime task closes a CLI surface mismatch with the Session Layer task draft, which groups app lifecycle under instance lifecycle management.

Scope:

- Add `session instance app <launch|stop|restart>` as a thin alias for existing app lifecycle control.
- Add `session request instance app <launch|stop|restart>` as the daemon request alias.
- Reuse the existing `session app <launch|stop|restart>` implementation without duplicating app launch/stop/restart logic.
- Require the same Session Layer lease for daemon-routed `session request instance app ...` requests.
- Advertise the alias in capabilities, `session contract`, `session api`, and Session Layer control examples.

Safety direction:

- This is an API/CLI compatibility alias only.
- It does not change app launch/stop/restart execution, package resolution, ADB commands, device backend behavior, capture backend behavior, daemon queue semantics, resource repositories, UI code, scheduler implementation, SQLite, OCR/OpenCV, or game logic.
- Explicit `--via-daemon` requests still fail with `runtime_not_running` when no alive resident daemon exists instead of falling back to local execution.

Validation status:

- Focused strict-throat, lease-gate, daemon-route, access contract, API contract, and capability tests passed.
- CLI smoke confirmed capabilities expose `session request instance app` and concrete `session instance app ...` commands.
- CLI smoke confirmed `session instance app launch --via-daemon` fails visibly with `runtime_not_running` when no daemon is present.
- `cargo fmt --all -- --check`, `git diff --check`, added-line prohibited-feature scan, `cargo clippy --workspace -- -D warnings`, and `cargo test --workspace` passed.

## Current ActingLab Session Instance Connect Lifecycle Surface

The current Runtime task closes the remaining Phase A naming gap for explicit instance connection control.

Scope:

- Add `session instance connect` as a discoverable instance lifecycle command.
- Reuse the existing device reachability path that verifies ADB device state and screen size.
- Advertise `session request instance connect` in capabilities.
- Add `daemon_controls.instance_connect = session request instance connect` to `session contract`.
- Add `envelopes.instance_connect_view` to `session api`.
- Treat connect as a lease-gated control when routed via the daemon or strict Session throat.
- Preserve existing `session instance reconnect`, `session instance health`, and `session instance keep-alive` behavior.

Safety direction:

- This is a narrow Session Layer API and discoverability change for an existing reachability path.
- It does not add a new ADB API, device backend, capture backend, app launch/stop/restart behavior, resource repository behavior, UI code, scheduler implementation, SQLite, OCR/OpenCV, or game logic.
- Explicit `--via-daemon` requests still fail with `runtime_not_running` when no alive resident daemon exists instead of falling back to local ADB.

Validation status:

- Focused lease-gate, daemon-route, contract, API, capability, and strict-throat tests passed.
- CLI smoke confirmed capabilities and `session api` expose the connect contract.
- CLI smoke confirmed `session instance connect --via-daemon` fails visibly with `runtime_not_running` when no daemon is present.
- `cargo fmt --all -- --check`, `git diff --check`, added-line prohibited-feature scan, `cargo clippy --workspace -- -D warnings`, and `cargo test --workspace` passed.

## Current ActingLab Session App Lifecycle Contract Surface

The current Runtime task closes a Phase A discoverability gap for app lifecycle controls.

Scope:

- Add `daemon_controls.app_lifecycle = session request app <launch|stop|restart>` to `session contract`.
- Add `envelopes.app_lifecycle_view` to `session api`.
- Expand Session Layer control examples from generic `app` to `session app launch`, `session app stop`, and `session app restart`.
- Ensure strict Session throat coverage includes `session instance keep-alive`.
- Replace the strict-session env CLI test with a pure throat-decision test so parallel tests do not leak `ACTINGLAB_REQUIRE_SESSION_DAEMON` into unrelated commands.
- Preserve the existing `session app launch|stop|restart` execution path and daemon lease gate.

Safety direction:

- This is a contract and discoverability change for existing lease-gated control commands.
- It does not change app launch/stop/restart execution, device backend behavior, daemon queue semantics, resource repositories, UI code, scheduler implementation, SQLite, OCR/OpenCV, or game logic.

Validation status:

- Focused access contract and API contract tests passed.
- CLI smoke confirmed `session contract` exposes `daemon_controls.app_lifecycle`.
- CLI smoke confirmed `session api` exposes `envelopes.app_lifecycle_view` with `requires_lease = true`.
- Focused strict-throat keep-alive and prior flaky navigation tests passed.
- `cargo fmt --all -- --check`, `git diff --check`, added-line prohibited-feature scan, `cargo clippy --workspace -- -D warnings`, and `cargo test --workspace` passed.

## Current ActingLab Session Instance Health Contract Surface

The current Runtime task closes a discoverability gap for the existing instance health command.

Scope:

- Add `daemon_queries.instance_health = session request instance health` to `session contract`.
- Add `envelopes.instance_health_view` to `session api`.
- Include `session instance health` in read-only Session Layer examples.
- Preserve `session instance health --capture-diagnose` as the capture-diagnosis form.

Safety direction:

- This is a contract and discoverability change for an existing read-only diagnostic command.
- It does not change device health execution, capture diagnosis behavior, daemon queue semantics, resource repositories, UI code, scheduler implementation, SQLite, OCR/OpenCV, or game logic.

Validation status:

- Focused access contract and API contract tests passed.
- CLI smoke confirmed `session api` exposes `instance_health_view`.
- CLI smoke confirmed `session contract` exposes `daemon_queries.instance_health`.
- `cargo fmt --all -- --check`, `git diff --check`, added-line prohibited-feature scan, `cargo clippy --workspace -- -D warnings`, and `cargo test --workspace` passed.

## Current ActingLab Session Instance Keep-Alive Surface

The current Runtime task adds an explicit Phase A keep-alive entry point for configured instances.

Scope:

- Add `session instance keep-alive` as a discoverable instance command.
- Reuse the existing device reachability path that verifies ADB device state and screen size.
- Return `action = keep-alive` and `keep_alive = true` in the command payload.
- Advertise `session request instance keep-alive` in capabilities.
- Include the daemon keep-alive query in `session contract`.
- Include the keep-alive view in `session api`.
- Include `session instance keep-alive` in read-only Session Layer examples.

Safety direction:

- This is a no-click, no-restart reachability probe.
- It does not capture frames, start MaaTouch, read resource repositories, change daemon queue semantics, call the scheduler, or add game logic.
- Explicit `--via-daemon` requests still fail with `runtime_not_running` when no alive resident daemon exists instead of falling back to local ADB.

Validation status:

- Focused capabilities, access contract, API contract, and capability registration tests passed.
- CLI smoke confirmed capabilities and `session api` expose the keep-alive contract.
- CLI smoke confirmed `session instance keep-alive --via-daemon` fails visibly with `runtime_not_running` when no daemon is present.
- `cargo fmt --all -- --check`, `git diff --check`, added-line prohibited-feature scan, `cargo clippy --workspace -- -D warnings`, and `cargo test --workspace` passed.

## Current ActingLab Daemon-Routed Instance Registry Contract Advertisement

The current Runtime task closes the gap between the instance registry contract and the daemon/API surfaces future UI or scheduler clients will use.

Scope:

- Advertise `session request instance registry` in capabilities.
- Include the registry view in `session api`.
- Include the daemon registry query in `session contract`.
- Include `session instance registry` in read-only Session Layer examples.
- Verify daemon-side `SessionCommandRequest { command: "instance", args: ["registry"] }` returns `session.instance_registry.v0.1`.

Safety direction:

- This is a contract and discoverability change for an already implemented read-only command.
- It does not touch devices, start MaaTouch, capture frames, read resource repositories, change daemon queue semantics, call the scheduler, or add game logic.
- Control request lease gates are unchanged.

Validation status:

- Focused daemon registry request test passed.
- Focused capabilities, access contract, and API contract tests passed.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- Source diff prohibited-feature scan found no direct ADB input, fallback additions, device/capture backend creation, SQLite, OCR/OpenCV, scheduler implementation, or game logic.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed, including `268` `actingcommand-actinglab` tests.

## Current ActingLab Session Instance Registry Contract

The current Runtime task adds a dedicated read-only registry contract so future UI and scheduler clients do not need to infer instance readiness from ad-hoc status diagnostics.

Scope:

- Add `session instance registry`.
- Return `schema_version = session.instance_registry.v0.1`.
- Expose required registry fields: `serial`, `game`, and `server`.
- Expose recommended operational fields: `package`, `adb_path`, and `capture_backend`.
- Expose supported capture backend ids: `auto`, `adb`, `droidcast_raw`, and `nemu_ipc`.
- For each configured instance, report raw fields, configured flags, effective capture backend, ADB path source, missing required fields, missing recommended fields, and `ready_for_device_control`.
- Validate manually edited `instance.<id>.capture_backend` values when reading the contract so bad config fails visibly instead of reaching UI/scheduler as fake-valid state.

Safety direction:

- This is a read-only configuration contract.
- It does not touch devices, start MaaTouch, capture frames, read resource repositories, call the scheduler, or add game logic.
- `session instance list` remains the simple compatibility list; the new contract is the structured UI/scheduler-facing surface.

Validation status:

- Focused registry contract tests passed.
- Existing `session instance list` test passed.
- `capabilities` test confirms the new command is advertised.
- Temporary-config CLI smoke confirmed `session instance registry` returns the expected schema, effective backend, and missing-field diagnostics.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- Source diff prohibited-feature scan found no direct ADB input, fallback additions, device/capture backend creation, SQLite, OCR/OpenCV, scheduler implementation, or game logic.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed, including `267` `actingcommand-actinglab` tests.

## Current ActingLab Instance Registry Backend Fields

The current Runtime task fills a Phase A registry gap: configured instances can now carry their own ADB path and capture backend preference, in addition to serial, game, server, and package.

Scope:

- Add `instance.<id>.adb_path` and `instance.<id>.capture_backend` to `config get/set`.
- Validate configured instance capture backend values at write time.
- Expose `adb_path` and `capture_backend` in `session instance list`.
- Expose the same fields and configured flags in `session status --diagnostics` / daemon-routed status diagnostics.
- Let capture-capable commands use the instance capture backend as the default when no CLI `--capture-backend` is provided.
- Preserve CLI `--capture-backend` as the highest-priority override.

Safety direction:

- This is a configuration and routing-default change only.
- Existing ADB path resolution priority is preserved: environment and reviewed MuMu discovery still precede configured paths.
- No device backend, capture backend implementation, resource repository, UI code, scheduler implementation, SQLite, OCR/OpenCV, or game logic was changed.

Validation status:

- Focused config get/set tests passed.
- Focused device-config capture backend priority tests passed.
- `session instance list` and `session_status` tests passed.
- Manual CLI smoke confirmed `diagnostics.instances` reports configured `adb_path` and `capture_backend`.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- Source diff prohibited-feature scan found no direct ADB input, fallback/reconnect additions, SQLite, OCR/OpenCV, scheduler implementation, or game logic.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed, including `265` `actingcommand-actinglab` tests.

## Current ActingLab Session Status Instance Registry Diagnostics

The current Runtime task extends the Session Layer status surface so local CLI, daemon requests, future UI clients, and scheduler health views can see the configured instance registry alongside liveness, queues, leases, and journals.

Scope:

- Add `diagnostics.instances` to `session status --diagnostics`.
- Include configured instance id, serial, game, server, package, and per-field configured flags.
- Expose the same instance registry summary through `session request status --diagnostics`.
- Advertise the status diagnostics instance-registry field in Session capability and API contracts.
- Keep internal status payload tests hermetic by making config-backed diagnostics an explicit caller option.

Safety direction:

- This is a read-only diagnostic change.
- No device backend, capture backend, resource repository, UI code, scheduler implementation, SQLite, OCR/OpenCV, or game logic was changed.
- Corrupt or unreadable config remains a visible failure on config-backed CLI/daemon diagnostics instead of silently dropping instances.

Validation status:

- Focused instance-registry diagnostics tests passed.
- Focused CLI status diagnostics test passed.
- `session_status` tests passed.
- Session API and access contract tests passed.
- Manual CLI smoke confirmed `diagnostics.instances` reports a configured temporary instance.
- `cargo fmt --all -- --check`, `git diff --check`, prohibited-feature scan, `cargo clippy --workspace -- -D warnings`, and `cargo test --workspace` passed.

## Current ActingLab Session Instance Capture Health Diagnostics

The current Runtime task extends the Session Layer instance-health surface so scheduler/UI/agent clients can ask the resident layer whether an instance is connected and whether capture freshness looks healthy. This directly addresses the AK stale `adb_screencap` finding without requiring callers to touch ADB or independently probe screenshots.

Scope:

- Add optional `session instance health --capture-diagnose`.
- Reuse the existing fresh-frame probe and backend order: `nemu_ipc`, `droidcast_raw`, then `adb_screencap` when `--capture-backend auto` is in effect.
- Report `status=device_connected` when capture diagnosis is not requested.
- Report `status=healthy`, `capture_stale_suspected`, or `capture_unavailable` when capture diagnosis is requested.
- Include capture freshness details, backend attempts, optional frame digest, and recovery recommendations in the health response.
- Advertise `session instance health --capture-diagnose` as a read-only Session API/access example.

Safety direction:

- Capture diagnosis remains opt-in on instance health and performs no clicks or game progress actions.
- The resident daemon path remains preferred when alive, and explicit daemon requests fail visibly when no daemon is running.
- No capture backend implementation, device input logic, resource repository, UI code, scheduler implementation, SQLite, OCR/OpenCV, or game logic was added.

Validation status:

- Focused tests passed for instance-health status mapping and capture-diagnosis JSON.
- Focused capture-diagnosis tests still pass.
- Manual CLI check confirmed `session instance health --capture-diagnose --via-daemon` without a daemon returns `runtime_not_running` instead of executing local ADB.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- Source diff prohibited-feature scan found no direct ADB input, shell screencap, fallback/reconnect/retry-loop additions, SQLite, OCR/OpenCV, scheduler implementation, or game logic.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed, including `258` `actingcommand-actinglab` tests.

## Current ActingLab Strict Session Throat Policy

The current Runtime task adds an explicit strict Session Layer gate for local CLI clients and future UI/API callers. When strict mode is enabled, device-touching or game-control command surfaces must route through the resident Session daemon instead of falling back to direct local ADB, MaaTouch, capture, or Lab execution paths.

Scope:

- Add global `--require-session`.
- Add environment flag `ACTINGLAB_REQUIRE_SESSION_DAEMON`.
- Block direct local execution of device/control commands when strict mode is enabled and no alive daemon heartbeat is available.
- Block explicit `--local` bypasses for strict-mode device/control commands even when daemon state exists.
- Preserve explicit `--via-daemon` request behavior so missing or stale daemon state continues to fail as `runtime_not_running`.
- Keep daemon-internal request handlers unblocked so resident daemon requests can execute local implementations without recursive requeue.
- Advertise `session_daemon_required` through capabilities, access, transport, and API contracts.

Safety direction:

- This milestone tightens the Session Layer "only throat" boundary without changing device backends, capture backends, resource repositories, UI code, scheduler implementation, SQLite, OCR/OpenCV, or game logic.
- Strict mode is opt-in for now through the CLI flag or environment variable.
- Severe bypass attempts fail visibly with safety exit code `3`.

Validation status:

- Focused strict-throat tests passed for missing daemon, explicit `--local` bypass, explicit `--via-daemon` liveness failure, and environment-variable activation.
- Manual CLI check confirmed strict `capture` without a daemon returns `session_daemon_required`.
- Manual CLI check confirmed strict `capture --via-daemon` without a daemon returns `runtime_not_running`.
- Manual CLI check confirmed strict `session status --diagnostics` remains an offline diagnostic command.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- Source diff prohibited-feature scan found only the strict classification reference to existing `session instance reconnect`; no reconnect logic, fallback, direct ADB input, shell screencap, SQLite, OCR/OpenCV, scheduler implementation, or game logic was added.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed, including `256` `actingcommand-actinglab` tests.

## Current ActingLab Trusted Remote Endpoint Policy

The current Runtime task tightens the Session Layer multi-channel boundary. Local loopback endpoints remain allowed for local CLI/runtime checks, while non-loopback runtime endpoints are treated as trusted remote access and must satisfy encryption plus authentication policy before any runtime reachability check is considered valid.

Scope:

- Classify runtime endpoints as `local_direct` or `trusted_remote`.
- Allow loopback endpoints such as `localhost` and `127.x.x.x` without trusted-remote auth material.
- Block non-loopback endpoints unless they use `https://`.
- Require `ACTINGLAB_TRUSTED_REMOTE_TOKEN` or `ACTINGLAB_TRUSTED_REMOTE_CLIENT_CERT` for non-loopback `https://` endpoints.
- Return visible safety errors `trusted_remote_transport_blocked` and `trusted_remote_auth_required` instead of silently probing unsafe endpoints.
- Report runtime endpoint policy diagnostics through `doctor`.
- Advertise trusted remote auth environment variables and failure codes through the capability, access, transport, and API contracts.

Safety direction:

- This milestone does not implement a network listener, TLS stack, token issuance, UI transport, or remote server.
- The policy prevents future trusted remote wiring from accidentally accepting plain HTTP or unauthenticated remote endpoints.
- Local CLI loopback use remains low-friction and still fails visibly when the runtime is unreachable.

Validation status:

- Focused endpoint policy tests passed for loopback, remote HTTP blocking, remote HTTPS auth blocking, and remote HTTPS with token.
- Focused CLI tests passed for `status` blocking untrusted remote endpoints and `doctor` reporting trusted remote policy errors without failing the diagnostic command.
- Focused Session contract tests passed for access, API, transport, and capabilities surfaces.
- Manual CLI check confirmed remote `http://` status returns `trusted_remote_transport_blocked`.
- Manual CLI check confirmed remote `https://` without auth appears in `doctor` as `trusted_remote_auth_required`.
- Manual CLI check confirmed loopback `http://127.0.0.1:1` remains a local runtime reachability failure with `runtime_not_running`.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- Source diff prohibited-feature scan found no newly added `adb shell input`, `input tap`, `input swipe`, `adb shell screencap`, fallback, reconnect, retry loop, SQLite, OCR/OpenCV, scheduler implementation, or game logic in the touched source file.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed, including `252` `actingcommand-actinglab` tests.

Out of scope:

- No network listener, TLS/auth transport implementation, UI code, scheduler implementation, device I/O behavior change, capture backend change, recognition, resource repository access, SQLite, OCR/OpenCV, or game logic was added.

## Current ActingLab Session Transport Contract

The current Runtime task makes the Session Layer transport boundary machine-readable for local CLI clients, the resident daemon request channel, and future trusted UI/API clients. This is a contract-only milestone and does not start a network listener.

Scope:

- Add `session transport` as an offline, read-only contract query.
- Add `session request transport` as a resident daemon read-only query.
- Expose `session.transport.v0.1` with local CLI, daemon file-IPC, reserved trusted remote, and interactive stream channel descriptions.
- Link the transport view from `session.access.v0.1`, `session.api.v0.1`, and command capabilities.
- Keep trusted remote transport reserved with required encryption and authentication.
- Keep interactive stream transport reserved while referencing the existing bounded stream event envelope.

Safety direction:

- This milestone only documents and routes existing Session Layer access boundaries.
- `session request transport` fails visibly when the resident daemon is unavailable.
- No trusted network API, TLS/auth transport implementation, UI code, scheduler implementation, device I/O behavior change, capture backend change, recognition, resource repository access, SQLite, OCR/OpenCV, or game logic was added.

Validation status:

- Focused transport tests passed for offline and daemon contract paths.
- Focused API, access contract, capabilities, and no-daemon tests passed.
- Manual CLI check confirmed `session transport` returns `session.transport.v0.1`.
- Manual CLI check confirmed `session request transport` without a daemon returns `runtime_not_running`.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- Source diff prohibited-feature scan found no newly added `adb shell input`, `input tap`, `input swipe`, `adb shell screencap`, fallback, reconnect, retry loop, SQLite, OCR/OpenCV, scheduler implementation, or game logic in the touched source file.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed, including `246` `actingcommand-actinglab` tests.

Out of scope:

- No trusted network API, TLS/auth transport implementation, UI code, scheduler implementation, device I/O, capture backend change, recognition, resource repository access, SQLite, OCR/OpenCV, or game logic was added.

## Current ActingLab Bounded Stream Event Envelope

The current Runtime task makes the bounded local stream scaffold easier for future UI/API clients to consume without scraping array positions. Each bounded stream response now has a stream id, and every stream event carries the same stream id, an event schema version, and a stable event index.

Scope:

- Add top-level `stream_id` to `stream` output.
- Add `contract.event_schema_version = session.stream.event.v0.1`.
- Add `contract.event_fields` documenting the minimum stream event envelope.
- Add `schema_version`, `stream_id`, and `event_index` to `stream.started`, `stream.frame_sampled`, `stream.input_relay`, and `stream.completed` events.
- Preserve existing bounded local stream behavior, dry-run behavior, and input relay behavior.

Safety direction:

- This milestone is a JSON contract tightening for the existing bounded stream scaffold.
- No device I/O behavior, input backend behavior, capture backend behavior, trusted network listener, UI, scheduler implementation, resource repository access, SQLite, OCR/OpenCV, or game logic was added.
- The trusted remote stream channel remains reserved.

Validation status:

- Focused stream tests passed for dry-run stream contract, input relay, daemon lease gates, and no-daemon failure paths.
- Manual CLI check confirmed `stream --dry-run --max-frames 2 --input-event ...` returns a `stream_id` and `session.stream.event.v0.1` events with stable indexes.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- Source diff prohibited-feature scan found no newly added `adb shell input`, `input tap`, `input swipe`, `adb shell screencap`, fallback, reconnect, retry loop, SQLite, OCR/OpenCV, scheduler implementation, or game logic in the touched source file.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed, including `243` `actingcommand-actinglab` tests.

Out of scope:

- No trusted network API, TLS/auth transport, UI code, scheduler implementation, device I/O behavior change, capture backend change, recognition, resource repository access, SQLite, OCR/OpenCV, or game logic was added.

## Current ActingLab Session Request-Id Event Cursor

The current Runtime task tightens the incremental Session event view for future UI/API clients. Timestamp cursors remain available, and request-id cursors now provide a precise continuation point when multiple daemon request events share the same completion timestamp.

Scope:

- Add `--after-request-id <request_id>` to `session events`.
- Add the same request-id cursor filter to resident daemon `session request events`.
- Return `after_request_id`, `cursor.latest_request_id`, and `cursor.next_after_request_id` in `session.events.v0.1`.
- Return visible `event_cursor_not_found` errors when a supplied request cursor is not present in the recent request journal.
- Document the new filter, cursor fields, and cursor error code in `session.api.v0.1`.

Safety direction:

- This milestone is read-only and only projects existing request-journal data.
- Missing request cursors fail visibly instead of returning a fake empty event list.
- No device I/O, emulator control, capture backend change, resource repository access, UI, trusted network listener, scheduler implementation, SQLite, OCR/OpenCV, or game logic was added.

Validation status:

- Focused event tests passed for stable event output, timestamp cursors, request-id cursors with same-timestamp events, and missing-cursor failure.
- Focused API contract tests passed and now cover request-id filters plus request-id cursor fields.
- Manual CLI check confirmed missing `--after-request-id` emits `event_cursor_not_found`.
- Manual CLI check confirmed `session api` advertises `--after-request-id`, request-id cursor fields, and `event_cursor_not_found`.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- Source diff prohibited-feature scan found no newly added `adb shell input`, `input tap`, `input swipe`, `adb shell screencap`, fallback, reconnect, retry loop, SQLite, OCR/OpenCV, scheduler implementation, or game logic in the touched source file.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed, including `243` `actingcommand-actinglab` tests.

Out of scope:

- No trusted network API, TLS/auth transport, UI code, scheduler implementation, device I/O, capture backend change, recognition, resource repository access, SQLite, OCR/OpenCV, or game logic was added.

## Current ActingLab Session Events Cursor

The current Runtime task advances Session Layer requirement #8 and #10 by making the request-journal event view incrementally consumable. Local CLI clients and future trusted UI/API clients can request only events completed after a known timestamp and use the returned cursor for the next poll.

Scope:

- Add `--after-unix-ms <timestamp>` to `session events`.
- Add the same filter to resident daemon `session request events`.
- Keep `--limit` validation at `1..=1000`.
- Expose `after_unix_ms`, `cursor.latest_timestamp_unix_ms`, and `cursor.next_after_unix_ms` in `session.events.v0.1`.
- Document event filters and cursor fields in `session.api.v0.1`.

Safety direction:

- This milestone is read-only and only projects existing request-journal data.
- No device I/O, emulator control, capture backend change, resource repository access, UI, trusted network listener, scheduler implementation, SQLite, OCR/OpenCV, or game logic was added.
- Daemon-routed event queries still fail visibly when the resident daemon is unavailable or stale.

Validation status:

- Focused event tests passed for stable request-event output and incremental `--after-unix-ms` filtering.
- Focused API contract tests passed and now cover event filters plus cursor fields.
- Manual CLI check confirmed empty local `session events --after-unix-ms 0` returns `session.events.v0.1` with cursor fields.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- Source diff prohibited-feature scan found no newly added `adb shell input`, `input tap`, `input swipe`, `adb shell screencap`, fallback, reconnect, retry loop, SQLite, OCR/OpenCV, scheduler implementation, or game logic in the touched source file.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed, including `241` `actingcommand-actinglab` tests.

Out of scope:

- No trusted network API, TLS/auth transport, UI code, scheduler implementation, device I/O, capture backend change, recognition, resource repository access, SQLite, OCR/OpenCV, or game logic was added.

## Current ActingLab Session API Contract

The current Runtime task advances Session Layer requirement #10 by exposing the internal command/envelope contract as machine-readable data. This lets local CLI clients and future trusted UI/API clients discover the same API shape without starting a network listener or implementing UI transport.

Scope:

- Add `session api` as a local, read-only API contract query.
- Add `session request api` as a resident daemon read-only query.
- Define `session.api.v0.1` with local CLI and reserved trusted remote access channels.
- Record that trusted remote access is not implemented yet and will require encryption plus authentication.
- Describe daemon request queue fields, response fields, CLI envelope fields, and event-view schema.
- Record read-only versus control command classes and lease requirements.
- Register both commands in the capability table and expose the daemon query through the access contract.

Safety direction:

- This milestone does not start a network listener and does not implement TLS, token issuance, UI transport, scheduler behavior, device I/O, capture backend changes, recognition, resource access, SQLite, OCR/OpenCV, or game logic.
- The contract states that clients must not directly touch ADB or devices.
- Control requests remain lease-gated and serialized through the resident daemon request queue.

Validation status:

- Focused API tests passed for offline output, daemon-side handler output, no-daemon failure, capability registration, and access-contract discovery.
- Manual CLI check confirmed `session api` returns `session.api.v0.1`.
- Manual CLI check confirmed `session request api` fails visibly with `runtime_not_running` when no daemon exists.
- Manual resident-daemon smoke check started a temporary daemon, queried `session request api`, and stopped the daemon successfully.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- Source diff prohibited-feature scan found no newly added `adb shell input`, `input tap`, `input swipe`, `adb shell screencap`, fallback, reconnect, retry loop, SQLite, OCR/OpenCV, scheduler implementation, or game logic in the touched source file.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed, including `240` `actingcommand-actinglab` tests.

Out of scope:

- No trusted network API, TLS/auth transport, UI code, scheduler implementation, device I/O, capture backend change, recognition, resource repository access, SQLite, OCR/OpenCV, or game logic was added.

## Current ActingLab Session Events View

The current Runtime task advances the Session Layer observable-event surface without adding UI, network transport, scheduler behavior, device logic, resources, SQLite, OCR/OpenCV, or game logic. The durable daemon request journal can now be projected into a stable event view for future UI/API consumers.

Scope:

- Add `session events` as a local, read-only event query over the request journal.
- Add `session request events` as a resident daemon read-only query.
- Define `session.events.v0.1` as a list envelope for recent request events.
- Define per-entry `session.event.v0.1` data with event type, request id, command, status, lease metadata, error metadata, and timing fields.
- Register both commands in the capability table and the Session access contract.

Safety direction:

- This milestone is read-only and does not touch devices, resources, screenshots, scheduler state, UI, or game automation.
- The event view is derived from the existing request journal and does not create a second mutable runtime history.
- A missing or stale resident daemon still fails visibly with `runtime_not_running` for daemon-routed requests.

Validation status:

- Focused event tests passed for daemon-side handler output, local event projection over success/failure journal entries, no-daemon failure, and capability registration.
- Manual CLI check confirmed empty local `session events` returns `session.events.v0.1` with `event_count = 0`.
- Manual CLI check confirmed `session request events` fails visibly with `runtime_not_running` when no daemon exists.
- Manual resident-daemon smoke check started a temporary daemon, queried `session request contract`, queried `session request events --limit 1`, and stopped the daemon successfully.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- Source diff prohibited-feature scan found no newly added `adb shell input`, `input tap`, `input swipe`, `adb shell screencap`, fallback, reconnect, retry loop, SQLite, OCR/OpenCV, scheduler implementation, or game logic in the touched source file.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed, including `237` `actingcommand-actinglab` tests.

Out of scope:

- No trusted network API, TLS/auth transport, UI code, scheduler implementation, device I/O, capture backend change, recognition, resource repository access, SQLite, OCR/OpenCV, or game logic was added.

## Current ActingLab Session Access Contract

The current Runtime task advances Session Layer requirement #10 by making the access boundary queryable as data. Local CLI clients and future trusted UI/API clients can read the same contract either offline or through the resident daemon request queue.

Scope:

- Add `session contract` as a local, read-only access-boundary query.
- Add `session request contract` as a resident daemon read-only query.
- Define `session.access.v0.1` with local CLI and reserved trusted remote entrypoints.
- Record that future trusted remote access requires authentication and encryption before use.
- Record that clients, including UI, must not directly touch ADB or devices.
- Record read-only versus control request classes and lease requirements.
- Register both commands in the capability table.

Safety direction:

- This milestone does not start a network listener and does not implement TLS, token issuance, UI transport, scheduler behavior, device I/O, or game logic.
- The trusted remote channel remains reserved until authentication and encryption are implemented.
- Control requests remain lease-gated and serialized through the resident daemon request queue.

Validation status:

- Focused contract tests passed for offline output, daemon-side handler output, no-daemon failure, and capability registration.
- Manual CLI check confirmed `session contract` returns `session.access.v0.1`.
- Manual CLI check confirmed `session request contract` fails visibly with `runtime_not_running` when no daemon exists.
- Manual resident-daemon smoke check started a temporary daemon, queried `session request contract`, and stopped the daemon successfully.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- Source diff prohibited-feature scan found no newly added `adb shell input`, `input tap`, `input swipe`, `adb shell screencap`, fallback, reconnect, retry loop, SQLite, OCR/OpenCV, scheduler implementation, or game logic in the touched source file.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed, including `235` `actingcommand-actinglab` tests.

Out of scope:

- No trusted network API, TLS/auth transport, token store, UI code, scheduler implementation, device I/O, capture backend change, recognition, resource repository access, SQLite, OCR/OpenCV, or game logic was added.

## Current ActingLab Daemon-Routed Capabilities Contract

The current Runtime task advances the multi-channel Session Layer access model. Future UI/API clients can now query capabilities through the resident daemon request queue instead of relying only on the offline top-level CLI.

Scope:

- Add `session request capabilities` as a read-only resident daemon request.
- Add `session.capabilities.v0.1` to `capabilities` output.
- Describe local CLI versus future trusted remote access channels.
- Describe read-only versus control request classes and lease requirements.
- Register `session request capabilities` in the command capability table.

Safety direction:

- The new request is read-only and does not require a LabLease.
- The trusted remote channel remains reserved and explicitly requires authentication and encryption before use.
- The contract reiterates that UI clients must not directly touch ADB or devices; Session Layer remains the only control throat.

Validation status:

- Focused capabilities tests passed after adding daemon handler, CLI error-path, and offline contract coverage.
- Manual CLI checks confirmed top-level `capabilities` includes `session.capabilities.v0.1`.
- Manual CLI check confirmed `session request capabilities` fails visibly with `runtime_not_running` when no daemon exists.
- Manual resident-daemon smoke check started a temporary daemon, queried `session request capabilities`, and stopped the daemon successfully.
- Full workspace validation initially exposed two `detect-page` tests missing the shared environment lock; those tests now use the existing `ENV_LOCK` and pass under the default concurrent test runner.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- Source diff prohibited-feature scan found no newly added `adb shell input`, `input tap`, `input swipe`, `adb shell screencap`, fallback, reconnect, retry loop, SQLite, OCR/OpenCV, scheduler implementation, or game logic in the touched source file.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed, including `232` `actingcommand-actinglab` tests.

Out of scope:

- No trusted network API or TLS/auth transport was added.
- No UI code was added.
- No scheduler implementation, device I/O, capture backend change, recognition, resource repository access, SQLite, OCR/OpenCV, or game logic was changed.

## Current ActingLab Bounded Stream Contract Envelope

The current Runtime task advances the Session Layer interactive-stream surface without adding UI, network transport, scheduler behavior, device logic, resource access, or a long-lived video channel. The bounded local `stream` command now emits a stable contract object and event list for future UI/API consumers.

Scope:

- Add `contract.schema_version = session.stream.v0.1` to `stream` output.
- Describe bounded frame delivery, capture timing parameters, input relay support, and safety boundaries in the stream contract.
- Emit ordered `stream.started`, `stream.frame_sampled`, optional `stream.input_relay`, and `stream.completed` events beside existing frame summaries.
- Keep dry-run stream behavior device-free.
- Keep daemon-routed input relay lease enforcement unchanged.

Safety direction:

- The contract states that the Session Layer is the only control throat and future UI clients must not directly touch ADB or devices.
- Trusted remote transport remains reserved; this task does not expose a network API.
- Input relay support remains bounded and uses existing lease-gated daemon routing when routed through the resident daemon.

Validation status:

- Focused stream tests passed after adding contract and event assertions.
- Dry-run CLI checks confirmed `stream` returns the new contract and event envelope without touching devices.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- Source diff prohibited-feature scan found no newly added `adb shell input`, `input tap`, `input swipe`, `adb shell screencap`, fallback, reconnect, retry loop, SQLite, OCR/OpenCV, scheduler implementation, or game logic in the touched source file.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed, including `230` `actingcommand-actinglab` tests.

Out of scope:

- No real trusted UI/API stream transport was added.
- No long-lived interactive relay protocol was added.
- No device backend, capture backend, recognition, scheduler, resource repository, SQLite, UI, OCR/OpenCV, or game logic was changed.

## Current ActingLab Session Diagnostics Recommended Actions

The current Runtime task makes Session Layer diagnostics directly actionable for future UI and scheduler consumers. `session status --diagnostics` now includes `recommended_actions` derived from the same liveness state used by daemon routing and lifecycle gates.

Scope:

- Add `diagnostics.recommended_actions` to `session status --diagnostics`.
- Emit no action when the resident daemon is alive and can accept requests.
- Recommend `session start` when the session is stopped.
- Recommend `session cleanup --stale --dry-run`, `session cleanup --stale`, then `session start` when the session state is stale, heartbeat-missing, or pid-mismatched.
- Include both machine-readable `args` and a human-readable `command` string for each action.
- Keep daemon loop behavior, cleanup behavior, capture, input, scheduler, UI, SQLite, OCR/OpenCV, resource access, and game logic unchanged.

Safety direction:

- Recommendations do not execute anything.
- Stale cleanup remains explicit and operator-driven.
- The action list is structured so UI/API/scheduler consumers can display or run approved commands without parsing diagnostic prose.

Validation status:

- Focused `cargo test -p actingcommand-actinglab session_status_diagnostics_ -- --nocapture` passed after adding stopped/alive/stale recommendation coverage.
- `cargo run -q -p actingcommand-actinglab -- --json session status --diagnostics --state-dir <temp>` returned a stopped-state `start_session` recommendation.
- `cargo fmt --all -- --check`, `git diff --check`, source diff prohibited-feature scan, `cargo clippy --workspace -- -D warnings`, and `cargo test --workspace` passed.

Out of scope:

- No automatic recovery execution was added.
- No new daemon request type was added.
- No trusted network API, UI, scheduler implementation, device I/O, capture backend change, resource repository access, or game-specific logic was added.

## Current ActingLab Stale Session Cleanup

The current Runtime task adds a conservative operator-facing cleanup path for stale Session Layer state. `session cleanup --stale` gives `session start`/`session stop` users a safe next command after liveness gates report stale, missing-heartbeat, or pid-mismatched daemon state.

Scope:

- Add `session cleanup --stale` as an explicit local command.
- Refuse cleanup when liveness is `alive`; operators should use `session stop` for a healthy daemon.
- Remove only local stale session files: `session.json`, `heartbeat.json`, `stop.request`, and pending request/response JSON files.
- Preserve request journals and archives for provenance.
- Support global `--dry-run` so operators can inspect planned cleanup before deletion.
- Advertise `session cleanup` through `capabilities`.
- Keep daemon loop behavior, capture, input, scheduler, UI, SQLite, OCR/OpenCV, resource access, and game logic unchanged.

Safety direction:

- Cleanup is never automatic; it requires the explicit `--stale` flag.
- Alive daemon state is protected from accidental cleanup.
- Pending stale request and response JSON files are removed with the stale state so a future daemon does not process old requests.
- Journals remain available for diagnostics after cleanup.

Validation status:

- Focused `cargo test -p actingcommand-actinglab session_cleanup_ -- --nocapture` passed after adding required-flag, alive-refusal, stale-cleanup, and dry-run coverage.
- `cargo test -p actingcommand-actinglab capabilities_are_offline -- --nocapture` passed.
- `cargo run -q -p actingcommand-actinglab -- --json capabilities` lists `session cleanup`.
- `cargo fmt --all -- --check`, `git diff --check`, source diff prohibited-feature scan, `cargo clippy --workspace -- -D warnings`, and `cargo test --workspace` passed.

Out of scope:

- No automatic stale cleanup was added.
- No corrupt-state cleanup path was added.
- No OS process probing was added.
- No live daemon was started for this milestone.
- No UI/API transport, scheduler arbitration, device I/O, capture backend change, resource repository access, or game-specific logic was added.

## Current ActingLab Session Stop Liveness Gate

The current Runtime task extends Session Layer liveness checks into daemon shutdown. `session stop` no longer treats `session.json` alone as proof that a resident daemon can accept the stop request.

Scope:

- Reuse the same liveness states and threshold used by `session status --diagnostics`, daemon routing, and `session start`.
- Write `stop.request` only when the existing daemon state is alive, pid-matched, and fresh.
- Fail visibly when existing state is stale, heartbeat-missing, or pid-mismatched instead of reporting a misleading stop request.
- Keep daemon loop behavior, capture, input, scheduler, UI, SQLite, OCR/OpenCV, resource access, and game logic unchanged.

Safety direction:

- Stale or inconsistent daemon state is reported as not accepting requests before any stop request is written.
- A successful stop request now means the daemon state is alive enough to receive it, not merely that a stale info file exists.
- This is a lifecycle consistency change and does not reconnect devices, click, capture, restart apps, or change scheduler ownership.

Validation status:

- Focused `cargo test -p actingcommand-actinglab session_stop_ -- --nocapture` passed after adding missing/stale/alive stop coverage.
- `cargo fmt --all -- --check`, `git diff --check`, source diff prohibited-feature scan, and `cargo clippy --workspace -- -D warnings` passed.
- The first `cargo test --workspace` run reported one isolated `current_page_resolves_semantic_page` failure; the focused rerun passed, and the second full `cargo test --workspace` passed.

Out of scope:

- No stale state cleanup command was added.
- No OS process probing was added.
- No live daemon was started for this milestone.
- No UI/API transport, scheduler arbitration, device I/O, capture backend change, resource repository access, or game-specific logic was added.

## Current ActingLab Session Start Liveness Gate

The current Runtime task extends Session Layer liveness from request routing into daemon lifecycle startup. `session start` no longer treats `session.json` alone as proof that the resident daemon is healthy.

Scope:

- Reuse the same liveness states and threshold used by `session status --diagnostics`.
- Return `already_running` only when an existing state directory has an alive, pid-matched, fresh heartbeat.
- Fail visibly when existing state is stale, heartbeat-missing, or pid-mismatched instead of pretending the daemon is running.
- Wait for a freshly spawned daemon to write an alive heartbeat before returning `started`.
- Keep daemon loop behavior, capture, input, scheduler, UI, SQLite, OCR/OpenCV, resource access, and game logic unchanged.

Safety direction:

- Corrupt status, heartbeat, lease, or journal files still fail visibly through existing status paths.
- Stale or inconsistent daemon state is reported as not accepting requests instead of silently treating any `session.json` file as healthy.
- Startup success now means the daemon is alive enough to accept requests, not merely that a stale info file exists.
- This is a lifecycle consistency change and does not reconnect devices, click, capture, restart apps, or change scheduler ownership.

Validation status:

- Focused `cargo test -p actingcommand-actinglab session_start_ -- --nocapture` passed after adding stale/alive startup coverage.
- `cargo fmt --all -- --check`, `git diff --check`, diff-only prohibited-feature scan, `cargo clippy --workspace -- -D warnings`, and `cargo test --workspace` passed.

Out of scope:

- No OS process probing was added.
- No live daemon was started for this milestone.
- No UI/API transport, scheduler arbitration, device I/O, capture backend change, resource repository access, or game-specific logic was added.

## Current ActingLab Stale Capture Recovery Plan

The current Runtime task records the AK stale-frame finding as an explicit Session Layer recovery entry point. `session recover --stale-capture` is a diagnostic/planning command: it does not click, restart apps, open MaaTouch, or require resource packs.

Scope:

- Add `session recover --stale-capture` as a read-only plan for suspected stale capture surfaces.
- Reuse the existing capture diagnosis recovery recommendations.
- Make the plan order explicit: fresh-frame probe, `nemu_ipc`, `droidcast_raw`, device health, and only then heavy `session app restart`.
- Allow daemon-side stale-capture recovery planning without a LabLease because no input or restart is executed.
- Keep normal `session recover` route recovery lease-gated and unchanged.

Safety direction:

- The stale-capture recovery plan treats unchanged frames as a capture reliability problem first, not proof of game freeze.
- The command returns `executed=false`, `click_allowed=false`, and `app_restart_executed=false`.
- Real app restart remains a separate heavy recovery command and still requires the normal Session Layer lease path.

Validation status:

- `cargo test -p actingcommand-actinglab session_recover_stale_capture -- --nocapture` passed with `2` tests.
- `cargo test -p actingcommand-actinglab capture_diagnosis_recommends_fast_backends_before_restart_for_adb_stale -- --nocapture` passed with `1` test.
- `cargo run -q -p actingcommand-actinglab -- --json --capture-backend adb session recover --stale-capture` returned the planned recovery sequence.
- `cargo fmt --all -- --check`, `git diff --check`, diff-only prohibited-feature scan, `cargo clippy --workspace -- -D warnings`, and `cargo test --workspace` passed.

Out of scope:

- No live emulator operation was required.
- No capture backend hot-path change was made.
- No app restart automation, scheduler implementation, UI, SQLite, OCR/OpenCV, game logic, fallback loop, reconnect loop, or resource repository access was added.

## Current ActingLab Bounded Stream Multi-Event Relay

The current Runtime task moves the Session Layer stream relay closer to the future interactive UI/API shape by allowing a bounded stream request to carry multiple input events. This is still a local CLI/daemon scaffold, not a network UI stream, but it proves the internal command surface can serialize several relay events through the same Session Layer path.

Scope:

- Keep the previous `stream --input-relay <action> ...` single-action form working.
- Add repeated `--input-event <action,args>` and `--relay-event <action,args>` for multiple relay events.
- Support `tap`, `swipe`, `long-tap`, `key`, and `text` relay events.
- Cap each bounded stream request at `16` relay events.
- In dry-run mode, return the planned action list without opening MaaTouch.
- In non-dry-run mode, execute all relay events in order through one MaaTouch session.
- Keep daemon-routed stream relay behind the same Session Layer lease validation.

Safety direction:

- Multi-event relay is bounded and local; it does not add UI, WebSocket, TLS, remote API, scheduler, SQLite, OCR/OpenCV, resource repository writes, or game logic.
- Ordinary bounded stream sampling remains read-only when no input events are present.
- Daemon-routed stream relay remains task-level input and must pass `ensure_session_request_lease` before any action can run.

Validation status:

- `cargo test -p actingcommand-actinglab stream_input_relay_dry_run_reports -- --nocapture` passed with `2` tests.
- `cargo test -p actingcommand-actinglab session_stream_input_relay_request -- --nocapture` passed with `2` tests.
- `cargo test -p actingcommand-actinglab stream_command_reports_bounded_dry_run_contract -- --nocapture` passed with `1` test.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- Source scan found no newly added `adb shell input`, `input tap`, `input swipe`, `adb shell screencap`, fallback, reconnect, retry loop, OCR/OpenCV, SQLite, scheduler implementation, or game logic in the touched source file.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed.

Known follow-ups:

- Replace the bounded local stream scaffold with a real trusted UI/API stream transport in a later milestone.
- Define a long-lived interactive relay protocol after the scheduler/UI lease ownership model is finalized.
- Continue live prepared-emulator validation after the Session Layer task sequence is ready.

## Current ActingLab Bounded Stream Input Relay Scaffold

The current Runtime task advances the Session Layer interactive-stream requirement without adding UI, network transport, or a full long-lived video channel. The bounded local `stream` command can now include one input relay action using the same MaaTouch-backed action model as existing direct control commands.

Scope:

- Keep ordinary `stream` read-only and daemon-routed through the existing read-only request path.
- Add `stream --input-relay <tap|swipe|long-tap|key|text> ...` and the `--interactive-input` alias as a bounded local relay scaffold.
- Route stream requests with input relay through the daemon control request path when a resident daemon is visible.
- Require a matching Session Layer lease before daemon-side stream input relay can run.
- Reuse existing MaaTouch input methods for tap, swipe, long-tap, key, and text.
- Keep dry-run input relay visible as a planned action without opening MaaTouch.

Safety direction:

- Stream input relay is not a UI, WebSocket, TLS, remote API, scheduler, SQLite, OCR/OpenCV, or game-logic implementation.
- Ordinary bounded frame sampling remains read-only.
- A daemon-routed stream request with input relay is task-level input and must pass `ensure_session_request_lease` before any action can run.
- Direct local input relay is still manual/local only when no resident daemon is visible, matching existing direct input command behavior.

Validation status:

- `cargo test -p actingcommand-actinglab stream_command_reports_bounded_dry_run_contract -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab stream_input_relay_dry_run_reports_planned_action -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab session_stream_input_relay_request -- --nocapture` passed with `2` tests.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- Source scan found no newly added `adb shell input`, `input tap`, `input swipe`, `adb shell screencap`, fallback, reconnect, retry loop, OCR/OpenCV, SQLite, scheduler implementation, or game logic in the touched source file.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed.

Known follow-ups:

- Replace the bounded local stream scaffold with a real trusted UI/API stream transport in a later milestone.
- Add interactive multi-event input relay once the trusted channel and scheduler/UI ownership model are defined.
- Continue live prepared-emulator validation after the Session Layer task sequence is ready.

## Current ActingLab LabLease Status Alias

The current Runtime task tightens the Lab-facing lease surface by allowing `lab lease status` to read the same Session Layer `lease-*.json` records as `session lease status`. This keeps Lab-facing acquire, status, preempt, and release operations on one lease model without introducing a second Lab-only state path.

Scope:

- Keep `lab lease ...` defaulting to `session lease acquire ...` for existing callers.
- Route explicit `lab lease status ...` to the existing `session lease status ...` implementation.
- Advertise `lab lease status` as an available LabLease capability.
- Keep all lease state in the same Session Layer `lease-*.json` files.

Safety direction:

- `lab lease status` is read-only and performs no device, scheduler, UI, SQLite, OCR/OpenCV, capture, MaaTouch, or game-logic work.
- Corrupt or unreadable lease state remains a visible error through the existing Session Layer JSON read path.
- This milestone does not implement scheduler ownership, trusted network API, interactive stream relay, live emulator execution, or resource repository mutation.

Validation status:

- `cargo test -p actingcommand-actinglab lab_lease_status_alias_reads_session_lease_file -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab lab_lease_capabilities_are_available -- --nocapture` passed with `1` test.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- Source scan found no newly added `adb shell input`, `input tap`, `input swipe`, `adb shell screencap`, fallback, reconnect, retry loop, OCR/OpenCV, SQLite, scheduler implementation, or game logic in the touched source file.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed.

Known follow-ups:

- Connect LabLease ownership to the real scheduler arbitration layer once that layer exists.
- Surface LabLease state through the future trusted UI/API channel.
- Continue live prepared-emulator validation after the Session Layer task sequence is ready.
- Implement trusted interactive stream/input relay in a later milestone.

## Current ActingLab LabLease Preempt Alias

The current Runtime task completes the Lab-facing lease surface by adding `lab preempt` as a thin alias over `session lease preempt`. This aligns the Lab CLI with the task contract that leases support acquire, release, and preempt without creating a second lease model.

Scope:

- Route `lab preempt` to the existing `session lease preempt` implementation.
- Preserve existing previous-holder provenance in the resulting `lease-*.json` state.
- Advertise `lab preempt` as an available LabLease capability.
- Keep all lease state in the same Session Layer `lease-*.json` files.

Safety direction:

- `lab preempt` is only a local lease-state interface; it does not implement scheduler ownership or arbitration policy.
- Existing holder, lease-id, and daemon-side lease checks remain the authority for task-level input.
- This milestone does not add scheduler implementation, UI, SQLite, OCR/OpenCV, game logic, ADB input fallback, capture hot-path algorithm change, reconnect loop, retry loop, silent fallback, live emulator execution, or trusted network API.

Validation status:

- `cargo test -p actingcommand-actinglab lab_preempt_alias_records_previous_session_lease -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab lab_lease_capabilities_are_available -- --nocapture` passed with `1` test.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- Source scan found no newly added `adb shell input`, `input tap`, `input swipe`, `adb shell screencap`, fallback, reconnect, retry loop, OCR/OpenCV, SQLite, UI, scheduler implementation, or game logic in the touched source file.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed.

Known follow-ups:

- Connect LabLease ownership to the real scheduler arbitration layer once that layer exists.
- Surface LabLease state through the future trusted UI/API channel.
- Continue live prepared-emulator validation after the Session Layer task sequence is ready.
- Implement trusted interactive stream/input relay in a later milestone.

## Previous ActingLab LabLease Aliases

The current Runtime task aligns the Lab-facing CLI surface with the Session Layer lease contract. `lab status`, `lab lease`, and `lab release` now reuse the existing Session Layer status and lease file implementation instead of staying reserved behind an unavailable runtime endpoint.

Scope:

- Route `lab status` to the existing `session status` implementation.
- Route `lab lease` to `session lease acquire`.
- Route `lab release` to `session lease release`.
- Advertise `lab status`, `lab lease`, and `lab release` as available capabilities.
- Keep all lease state in the same Session Layer `lease-*.json` files.

Safety direction:

- These commands are aliases only; they do not implement a scheduler or create a second lease model.
- Existing holder and lease-id checks remain the authority for release and later daemon control validation.
- This milestone does not add scheduler implementation, UI, SQLite, OCR/OpenCV, game logic, ADB input fallback, capture hot-path algorithm change, reconnect loop, retry loop, silent fallback, live emulator execution, or trusted network API.

Validation status:

- `cargo test -p actingcommand-actinglab lab_status_alias_uses_session_status_without_runtime_endpoint -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab lab_lease_and_release_alias_session_lease_files -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab lab_lease_capabilities_are_available -- --nocapture` passed with `1` test.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- Source scan found no newly added `adb shell input`, `input tap`, `input swipe`, `adb shell screencap`, fallback, reconnect, retry loop, OCR/OpenCV, SQLite, UI, scheduler implementation, or game logic in the touched source file.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed.

Known follow-ups:

- Connect LabLease ownership to the real scheduler arbitration layer once that layer exists.
- Surface LabLease state through the future trusted UI/API channel.
- Continue live prepared-emulator validation after the Session Layer task sequence is ready.
- Implement trusted interactive stream/input relay in a later milestone.

## Previous ActingLab Session Lease Diagnostics

The current Runtime task improves Session Layer observability for the lease/arbitration surface. `session status --diagnostics` now includes the active `lease-*.json` records in the selected session state directory, so future UI, scheduler, and operator tools can see who currently owns control without issuing one lease-status command per instance.

Scope:

- Add read-only lease diagnostics to `session status --diagnostics`.
- Report active lease count, holder, lease id, timestamps, preempt provenance, and lease file path.
- Keep concurrent release races visible through `released_during_read_count` instead of pretending a stale lease is still active.
- Reject corrupt lease JSON visibly instead of skipping it.

Safety direction:

- This milestone is diagnostics only and performs no device I/O.
- Corrupt lease state is a state-integrity problem and must not be silently ignored.
- This milestone does not add scheduler implementation, UI, SQLite, OCR/OpenCV, game logic, ADB input fallback, capture hot-path algorithm change, reconnect loop, retry loop, silent fallback, live emulator execution, or trusted network API.

Validation status:

- `cargo test -p actingcommand-actinglab session_status_diagnostics_reports_active_leases -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab session_status_diagnostics_rejects_corrupt_lease_file -- --nocapture` passed with `1` test.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- Source-only added-code prohibited-feature scan returned `NO_PROHIBITED_CODE_ADDED_LINES`.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed.

Known follow-ups:

- Surface these diagnostics through the future trusted UI/API channel.
- Connect lease ownership to the real scheduler arbitration layer once that layer exists.
- Continue live prepared-emulator validation after the Session Layer task sequence is ready.
- Implement trusted interactive stream/input relay in a later milestone.

## Previous ActingLab Manual Lease Run UX

The current Runtime task adds a small manual-operator convenience layer above the existing Session Layer lease and daemon request queue. Operators can run one daemon-routed command through `session lease run -- <session-request-command...>` without manually acquiring and releasing a lease in separate commands.

Scope:

- Add `session lease run -- <session-request-command...>`.
- Acquire a temporary local lease for the selected instance and holder.
- Generate or preserve lease metadata and attach it to the delegated daemon request.
- Release the temporary lease after command success or failure.
- Keep daemon-side lease validation as the actual authority before control, lifecycle, Lab package, package, operation, recovery, or device I/O.
- Reject missing command separators and reject use through `session request lease`.

Safety direction:

- This wrapper must not bypass the resident Session Layer daemon request queue.
- A command timeout or daemon failure must fail visibly, and the local temporary lease must still be released when possible.
- This milestone does not add scheduler implementation, UI, SQLite, OCR/OpenCV, game logic, ADB input fallback, capture hot-path algorithm change, reconnect loop, retry loop, silent fallback, or live emulator execution.

Validation status:

- `cargo test -p actingcommand-actinglab session_lease_run_requires_command_separator -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab session_lease_run_submits_with_generated_lease_and_releases_on_timeout -- --nocapture` passed with `1` test.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- Source-only added-code prohibited-feature scan returned `NO_PROHIBITED_CODE_ADDED_LINES`.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed.

Known follow-ups:

- Connect this operator UX to the future scheduler lease arbitration policy.
- Expose lease acquisition and release status through the future trusted UI/API channel.
- Continue live prepared-emulator validation after the Session Layer task sequence is ready.
- Implement trusted interactive stream/input relay in a later milestone.

## Previous ActingLab Daemon-Preferred Lifecycle And Run Routing

The current Runtime task continues the Session Layer "sole throat" direction by routing the remaining device/lifecycle/run entry points through the resident daemon whenever session info indicates the daemon is running. This keeps CLI users and agents from bypassing the Session Layer for monitor diagnosis/recovery, instance health/reconnect, app launch/stop/restart, trusted Lab packages, package execution, and operation execution.

Scope:

- Apply daemon-preferred read-only routing to `monitor --once` and `session instance list|health`.
- Apply daemon-preferred control routing to `monitor --recover`, `session instance reconnect`, `session app launch|stop|restart`, `lab run`, `package run`, and `operation run`.
- Preserve existing local/direct behavior when no session info exists.
- Preserve daemon-side lease validation for control requests before app lifecycle, reconnect, Lab package, package, operation, recovery, or device I/O.
- Keep validation-only and build-only commands such as `lab validate`, `package validate|inspect|build-task|build-pack`, and `operation validate|inspect|explain` local/offline.

Safety direction:

- With a resident Session Layer daemon visible, humans/agents/CLI should not directly touch ADB, app lifecycle, MaaTouch, package execution, or operation execution paths.
- Missing or unprocessed daemon requests fail visibly with `runtime_not_running` timeout instead of silently falling back to direct device access.
- This milestone does not add scheduler implementation, UI, SQLite, OCR/OpenCV, game logic, ADB input fallback, capture hot-path algorithm change, reconnect loop, retry loop, or silent fallback.

Validation status:

- `cargo test -p actingcommand-actinglab device_lifecycle_and_run_entrypoints_prefer_daemon_when_session_info_exists -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab daemon_internal_handlers_do_not_requeue_to_daemon -- --nocapture` passed with `1` test.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- Source-only added-code prohibited-feature scan returned `NO_PROHIBITED_CODE_ADDED_LINES`.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed.

Known follow-ups:

- Add operator lease-acquisition UX for manual control.
- Expose the Session Layer boundary through the future trusted UI/API channel.
- Continue scheduler lease arbitration integration and live prepared-emulator validation.
- Implement trusted interactive stream/input relay in a later milestone.

## Previous ActingLab Daemon-Preferred Control Routing

The current Runtime task extends the Session Layer default from diagnostics toward control safety: when a resident session daemon is visible through the session state info file, direct control CLI entries prefer the daemon request queue without requiring `--via-daemon`. Daemon-side request handlers now mark their reconstructed `GlobalOptions` as already inside the resident daemon, so they execute local command implementations instead of submitting a second request back into the same queue.

Scope:

- Add an internal daemon-execution marker to `GlobalOptions`.
- Make daemon-side request execution set the marker when reconstructing `GlobalOptions`.
- Update read-only daemon-preference helpers so daemon-side handlers always stay local.
- Add a control daemon-preference helper that routes client-side control commands to the daemon when session info exists or `--via-daemon` is present.
- Apply daemon-preferred control routing to `tap`, `swipe`, `long-tap`, `key`, `text`, `tap-target`, `navigate`, and `session recover`.
- Preserve existing local/direct behavior when no session info exists.
- Preserve daemon-side lease validation for control requests before device I/O.

Safety direction:

- Client-side control commands should not directly touch MaaTouch/device paths when a resident Session Layer daemon is visible.
- Daemon-side handlers must not recursively requeue their own work.
- Missing or unprocessed daemon requests fail visibly with `runtime_not_running` timeout instead of silently falling back to direct device access.
- This milestone does not add scheduler implementation, UI, SQLite, OCR/OpenCV, game logic, ADB input fallback, capture hot-path algorithm change, reconnect loop, retry loop, or silent fallback.

Validation status:

- `cargo test -p actingcommand-actinglab direct_touch_prefers_daemon_when_session_info_exists -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab daemon_internal_handlers_do_not_requeue_to_daemon -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab direct_touch_via_daemon_accepts_lease_flags_before_daemon_lookup -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab status_prefers_daemon_when_session_info_exists -- --nocapture` passed with `1` test.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- Source-only added-code prohibited-feature scan returned `NO_PROHIBITED_CODE_ADDED_LINES`.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed.

Known follow-ups:

- Decide when app lifecycle, instance reconnect, Lab/package/operation run, and monitor recovery should also default to daemon-preferred routing.
- Add operator lease-acquisition UX for manual control.
- Expose the same Session Layer boundary through the future trusted UI/API channel.
- Continue scheduler lease arbitration integration and live prepared-emulator validation.

## Previous ActingLab Daemon-Preferred Read-Only Routing

The current Runtime task moves from opt-in daemon routing toward the Session Layer default: when a resident session daemon is visible through the session state info file, read-only and diagnostic CLI entries prefer the daemon request queue without requiring `--via-daemon`. If the daemon is absent, existing local/offline behavior remains available. `--local` is the explicit diagnostic override for local state reads or direct one-shot read-only commands.

Scope:

- Add a shared read-only routing helper that treats `--via-daemon` as forced daemon routing, `--local` as forced local routing, and existing session info as daemon-preferred routing.
- Apply daemon-preferred routing to `status`, `devices`, `capture`, `capture diagnose`, `recognize`, `detect-page`, `current-page`, `is-visible`, `locate`, `stream`, `session status`, and `session journal`.
- Keep control commands such as `tap`, `swipe`, `long-tap`, `key`, `text`, `tap-target`, `navigate`, and recovery lease-gated behind explicit daemon/control request paths for now.
- Strip `--local` from daemon payload args just like other client-side routing flags.
- Add regression tests for daemon preference, local override, and client-only payload stripping.

Safety direction:

- Daemon-preferred routing is limited to read-only or diagnostic commands in this milestone.
- Missing daemon state still preserves existing local behavior unless `--via-daemon` is explicitly requested.
- Stale or unprocessed daemon request files fail visibly with `runtime_not_running` timeout instead of silently falling back to direct device access.
- The milestone does not change control command lease requirements, scheduler implementation, UI, SQLite, OCR/OpenCV, game logic, ADB input fallback, capture hot-path algorithm change, reconnect loop, retry loop, or silent fallback.

Validation status:

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

Known follow-ups:

- Decide when control commands should become daemon-preferred by default and how to surface lease acquisition UX for manual operators.
- Expose daemon-preferred read-only routing through the future trusted UI/API channel once that channel exists.
- Continue moving user-facing diagnostic and control surfaces behind the resident Session Layer request/API boundary.

## Previous ActingLab Session Diagnostics Daemon Routing

The current Runtime task routes `session status` and `session journal` diagnostics through the resident daemon request queue when `--via-daemon` is present. Their bare forms remain local/offline state readers, while the routed forms reuse the existing daemon-side `status` and `journal` request handlers.

Scope:

- Add `session status --via-daemon`.
- Add `session journal --via-daemon`.
- Reuse the existing daemon-side `status` and `journal` request handling instead of adding duplicate diagnostics paths.
- Preserve bare `session status` and `session journal` local behavior.
- Add no-daemon regression tests for both routed session diagnostics.

Safety direction:

- The routed session diagnostics are read-only.
- Missing daemon state remains a visible runtime-not-running error.
- The milestone does not change device control, capture/input paths, scheduler implementation, UI, SQLite, OCR/OpenCV, game logic, ADB input fallback, capture hot-path algorithm change, reconnect loop, retry loop, or silent fallback.

Validation status:

- `cargo test -p actingcommand-actinglab session_status_via_daemon_without_daemon_is_runtime_error -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab session_journal_via_daemon_without_daemon_is_runtime_error -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab session_status_without_daemon_is_offline_ok -- --nocapture` passed with `1` test.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- Source-only added-code prohibited-feature scan returned `NO_PROHIBITED_CODE_ADDED_LINES`.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed.

Known follow-ups:

- Decide whether the default `session status` and `session journal` commands should auto-prefer the resident daemon when it is already running.
- Expose daemon session diagnostics through the future trusted UI/API channel once that channel exists.
- Continue moving user-facing diagnostic and control surfaces behind the resident Session Layer request/API boundary.

## Previous ActingLab Top-Level Daemon-Routed Status Entry

The current Runtime task routes the top-level `status` diagnostic entry point through the resident daemon request queue when `--via-daemon` is present. Bare `status` remains the local runtime-info probe, while `status --via-daemon --diagnostics` now reaches the same daemon-side status diagnostics already exposed by `session request status --diagnostics`.

Scope:

- Add `status --via-daemon`.
- Reuse the existing daemon-side `status` request handling instead of adding a duplicate status path.
- Preserve bare `status` behavior for local runtime probing.
- Add a no-daemon regression test for `status --via-daemon --diagnostics`.

Safety direction:

- The routed status request is diagnostic and read-only.
- Missing daemon state remains a visible runtime-not-running error.
- The milestone does not change device control, capture/input paths, scheduler implementation, UI, SQLite, OCR/OpenCV, game logic, ADB input fallback, capture hot-path algorithm change, reconnect loop, retry loop, or silent fallback.

Validation status:

- `cargo test -p actingcommand-actinglab status_via_daemon_without_daemon_is_runtime_error -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab session_request_status_without_daemon_is_runtime_error -- --nocapture` passed with `1` test.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- Source-only added-code prohibited-feature scan returned `NO_PROHIBITED_CODE_ADDED_LINES`.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed.

Known follow-ups:

- Decide whether the default top-level `status` command should auto-prefer the resident daemon when it is already running.
- Expose daemon status diagnostics through the future trusted UI/API channel once that channel exists.
- Continue moving user-facing diagnostic and control surfaces behind the resident Session Layer request/API boundary.

## Previous ActingLab Daemon-Routed Devices Diagnostics

The current Runtime task routes the `devices` diagnostic entry point through the resident daemon request queue. Local `devices` remains available, while `devices --via-daemon` and `session request devices` can now serialize device enumeration through the daemon.

Scope:

- Add `devices --via-daemon`.
- Add `session request devices`.
- Add daemon-side handling for the `devices` request.
- Advertise `session request devices` in capabilities.

Safety direction:

- The routed devices request is diagnostic and read-only.
- Missing daemon state remains a visible runtime-not-running error.
- The milestone does not change device control, capture/input paths, scheduler implementation, UI, SQLite, OCR/OpenCV, game logic, ADB input fallback, capture hot-path algorithm change, reconnect loop, retry loop, or silent fallback.

Validation status:

- `cargo test -p actingcommand-actinglab devices_via_daemon_without_daemon_is_runtime_error -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab session_request_devices_without_daemon_is_runtime_error -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab direct_touch_commands_are_capability_registered -- --nocapture` passed with `1` test.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- Source-only added-code prohibited-feature scan returned `NO_PROHIBITED_CODE_ADDED_LINES`.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed.

Known follow-ups:

- Decide whether the default top-level `devices` command should auto-prefer the resident daemon when it is already running.
- Expose the same daemon request devices/status diagnostics through the future trusted UI/API channel once that channel exists.
- Continue moving user-facing diagnostic and control surfaces behind the resident Session Layer request/API boundary.

## Previous ActingLab Daemon-Routed Recording Interface

The current Runtime task routes the Session Layer recording interface through the resident daemon request queue. Local `session record ...` and top-level `record ...` remain available, and `session request record ...` can now serialize recording lifecycle and authoring commands through the daemon.

Scope:

- Add `session request record ...`.
- Preserve recording provenance arguments such as `--holder`, `--lease-holder`, and `--lease-id` in daemon payloads while still stripping client-only request flags.
- Add daemon-side handling for the `record` request.
- Ensure daemon-routed recording operations use the daemon's state directory instead of the client's default session state path.
- Advertise `session request record` in capabilities.

Safety direction:

- `session request record start|status|stop` does not perform device I/O.
- Recording commands continue to fail visibly for invalid task ids, missing active recording sessions, malformed state, or unsupported actions.
- Missing daemon state remains a visible runtime-not-running error.
- The milestone does not change device control, capture/input paths, scheduler implementation, UI, SQLite, OCR/OpenCV, game logic, ADB input fallback, capture hot-path algorithm change, reconnect loop, retry loop, or silent fallback.

Validation status:

- `cargo test -p actingcommand-actinglab session_state_request_payload_preserves_holder_and_lease_id -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab session_record_request_starts_statuses_and_stops_in_daemon_state_dir -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab session_request_record_without_daemon_is_runtime_error -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab direct_touch_commands_are_capability_registered -- --nocapture` passed with `1` test.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- Source-only added-code prohibited-feature scan returned `NO_PROHIBITED_CODE_ADDED_LINES`.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed.

Known follow-ups:

- Expose the same daemon request/record surface through the future trusted UI/API channel once that channel exists.
- Decide which recording subcommands should require explicit scheduler/lease ownership before live device capture is allowed through the daemon.
- Connect recording review/promotion flows to the future scheduler and resource-review workflow.
- Implement the actual trusted interactive frame/input channel after the Runtime service boundary is accepted.

## Previous ActingLab Daemon-Routed Lease Interface

The current Runtime task routes the Session Layer lease interface through the resident daemon request queue. Local `session lease ...` remains available, and `session request lease ...` can now serialize lease acquire/release/preempt/status operations through the daemon.

Scope:

- Add `session request lease ...`.
- Preserve lease command arguments such as `--holder`, `--lease-holder`, and `--lease-id` in daemon payloads while still stripping client-only request flags.
- Add daemon-side handling for the `lease` request.
- Ensure daemon-routed lease operations use the daemon's state directory instead of the client's default session state path.
- Advertise `session request lease` in capabilities.

Safety direction:

- `session request lease` does not perform device I/O.
- Lease conflicts and holder/id mismatches remain visible safety-blocked errors.
- Missing daemon state remains a visible runtime-not-running error.
- The milestone does not change device control, capture/input paths, scheduler implementation, UI, SQLite, OCR/OpenCV, game logic, ADB input fallback, capture hot-path algorithm change, reconnect loop, retry loop, or silent fallback.

Validation status:

- `cargo test -p actingcommand-actinglab session_lease_request_payload_preserves_holder_and_lease_id -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab session_lease_request_acquires_and_releases_in_daemon_state_dir -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab session_request_lease_without_daemon_is_runtime_error -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab direct_touch_commands_are_capability_registered -- --nocapture` passed with `1` test.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed.
- Source-only added-code prohibited-feature scan returned `NO_PROHIBITED_CODE_ADDED_LINES`.

Known follow-ups:

- Expose the same daemon request/lease surface through the future trusted UI/API channel once that channel exists.
- Connect lease ownership to the real scheduler arbitration layer once that layer exists.
- Implement the actual trusted interactive frame/input channel after the Runtime service boundary is accepted.
- Decide the daemon transport/API shape for long-lived frame streams instead of bounded local CLI sampling.
- Add live prepared-emulator validation for real captured stream frames when safe target states are available.

## Previous ActingLab Daemon-Routed Journal Diagnostics

The current Runtime task extends the resident Session Layer diagnostic surface by routing request-journal reads through the daemon request queue. Local `session journal` remains available, and `session request journal [--limit N]` can now submit the same read-only query through the resident daemon.

Scope:

- Extract shared journal rendering into `session_journal_payload`.
- Keep local `session journal [--limit]` behavior stable.
- Add `session request journal [--limit]`.
- Add daemon-side handling for the read-only `journal` request.
- Advertise `session request journal` in capabilities.

Safety direction:

- `session request journal` is read-only and requires no lease.
- Missing daemon state remains a visible runtime-not-running error.
- Corrupt journal lines remain visible runtime errors.
- The milestone does not change daemon command execution, request ordering, response retention, lease enforcement, capture/input paths, scheduler implementation, UI, SQLite, OCR/OpenCV, game logic, ADB input fallback, capture hot-path algorithm change, reconnect loop, retry loop, or silent fallback.

Validation status:

- `cargo test -p actingcommand-actinglab session_journal_request_returns_daemon_journal_entries -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab session_request_journal_without_daemon_is_runtime_error -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab session_request_journal_records_success_and_error -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab top_level_record_capability_is_available -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab direct_touch_commands_are_capability_registered -- --nocapture` passed with `1` test.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- `cargo clippy --workspace -- -D warnings` passed after correcting a needless-borrow warning in the extracted journal helper.
- `cargo test --workspace` passed.
- Source-only added-code prohibited-feature scan returned `NO_PROHIBITED_CODE_ADDED_LINES`.

Known follow-ups:

- Expose the same daemon request/journal surface through the future trusted UI/API channel once that channel exists.
- Implement the actual trusted interactive frame/input channel after the Runtime service boundary is accepted.
- Decide the daemon transport/API shape for long-lived frame streams instead of bounded local CLI sampling.
- Add live prepared-emulator validation for real captured stream frames when safe target states are available.
- Review UI/API stream consumption after the trusted channel contract lands.

## Previous ActingLab Daemon-Routed Status Diagnostics

The current Runtime task moves the Session Layer status surface one step closer to the shared internal API. Local `session status --diagnostics` remains available, and `session request status --diagnostics` can now submit a read-only request through the resident daemon queue and return the same daemon state/diagnostics payload.

Scope:

- Extract shared status rendering into `session_status_payload`.
- Keep local `session status [--diagnostics]` behavior stable.
- Add `session request status [--diagnostics]`.
- Add daemon-side handling for the read-only `status` request.
- Advertise `session request status` in capabilities.

Safety direction:

- `session request status` is read-only and requires no lease.
- Missing daemon state remains a visible runtime-not-running error.
- The milestone does not change daemon command execution, request ordering, response retention, lease enforcement, capture/input paths, scheduler implementation, UI, SQLite, OCR/OpenCV, game logic, ADB input fallback, capture hot-path algorithm change, reconnect loop, retry loop, or silent fallback.

Validation status:

- `cargo test -p actingcommand-actinglab session_status_request_returns_daemon_diagnostics -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab session_request_status_without_daemon_is_runtime_error -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab session_status_without_daemon_is_offline_ok -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab top_level_record_capability_is_available -- --nocapture` passed with `1` test.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- `cargo clippy --workspace -- -D warnings` passed after correcting a needless-borrow warning in the extracted status helper.
- `cargo test --workspace` passed.
- Source-only added-code prohibited-feature scan returned `NO_PROHIBITED_CODE_ADDED_LINES`.

Known follow-ups:

- Expose the same daemon request/status surface through the future trusted UI/API channel once that channel exists.
- Implement the actual trusted interactive frame/input channel after the Runtime service boundary is accepted.
- Decide the daemon transport/API shape for long-lived frame streams instead of bounded local CLI sampling.
- Add live prepared-emulator validation for real captured stream frames when safe target states are available.
- Review UI/API stream consumption after the trusted channel contract lands.

## Previous ActingLab Request Journal Retention

The current Runtime task adds a bounded retention policy to the resident daemon request journal. This keeps a long-running Session Layer from growing `request-journal.jsonl` without limit while preserving the most recent active entries for `session journal` and diagnostics.

Scope:

- Add a fixed `1 MiB` active journal cap for `request-journal.jsonl`.
- Rotate an oversized active journal to `request-journal.1.jsonl` before appending the next processed request entry.
- Keep one local archive file and replace the previous archive on the next rotation.
- Keep `session journal` reading the active journal only, preserving the recent diagnostics surface.
- Extend `session status --diagnostics` with active journal path/bytes, retention policy, and archive path/existence/bytes.

Safety direction:

- Journal rotation happens before appending a new entry.
- Failure to remove an old archive, rename the active journal, stat the journal, encode, write, or flush remains a visible runtime error.
- The milestone does not change daemon request execution, response publication, request removal, lease enforcement, capture/input paths, command routing, scheduler implementation, UI, SQLite, OCR/OpenCV, game logic, ADB input fallback, capture hot-path algorithm change, reconnect loop, retry loop, or silent fallback.

Validation status:

- `cargo test -p actingcommand-actinglab session_request_journal_rotates_when_active_file_exceeds_retention_limit -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab session_status_diagnostics_reports_queue_and_journal_summary -- --nocapture` passed with `1` test.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed.
- Source-only added-code prohibited-feature scan returned `NO_PROHIBITED_CODE_ADDED_LINES`.

Known follow-ups:

- Expose the same diagnostics through the future trusted UI/API channel once that channel exists.
- Implement the actual trusted interactive frame/input channel after the Runtime service boundary is accepted.
- Decide the daemon transport/API shape for long-lived frame streams instead of bounded local CLI sampling.
- Add live prepared-emulator validation for real captured stream frames when safe target states are available.
- Review UI/API stream consumption after the trusted channel contract lands.

## Previous ActingLab Session Status Diagnostics

The current Runtime task surfaces the resident daemon request journal through `session status --diagnostics`. This keeps normal `session status` stable while giving UI, scheduler, and operator tooling a single health surface for queue depth and recent daemon request outcomes.

Scope:

- Add `session status --diagnostics`.
- Report daemon state paths for info, heartbeat, requests, responses, and journal.
- Report pending request and pending response JSON file counts.
- Report whether the request journal exists.
- Report parsed journal total count.
- Report a recent-entry limit of `5`, recent count, last entry, and last error entry.
- Parse all journal lines while counting total entries so corrupt historical lines fail visibly.

Safety direction:

- This milestone is read-only diagnostics only.
- It does not change daemon request execution, request ordering, lease enforcement, capture/input paths, or command routing.
- A corrupt journal line fails loudly with a runtime error instead of silently omitting bad data.
- This milestone adds no UI, scheduler implementation, SQLite, OCR/OpenCV, game logic, ADB input fallback, capture hot-path algorithm change, reconnect loop, retry loop, or silent fallback.

Validation status:

- `cargo test -p actingcommand-actinglab session_status_diagnostics_reports_queue_and_journal_summary -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab session_status_diagnostics_corrupt_journal_is_runtime_error -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab session_status_without_daemon_is_offline_ok -- --nocapture` passed with `1` test.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed.
- Source-only added-code prohibited-feature scan returned `NO_PROHIBITED_CODE_ADDED_LINES`.

Known follow-ups:

- Expose the same diagnostics through the future trusted UI/API channel once that channel exists.
- Implement the actual trusted interactive frame/input channel after the Runtime service boundary is accepted.
- Decide the daemon transport/API shape for long-lived frame streams instead of bounded local CLI sampling.
- Add live prepared-emulator validation for real captured stream frames when safe target states are available.
- Review UI/API stream consumption after the trusted channel contract lands.

## Previous ActingLab Daemon Request Journal

The current Runtime task adds persistent diagnostics to the resident Session Layer request queue. A daemon-processed request now leaves a JSONL journal entry after the response is written and the request file is removed, so later UI, scheduler, or operator diagnostics can inspect what the single control throat actually accepted and returned.

Scope:

- Add `request-journal.jsonl` under the session state directory.
- Record request id, command, sanitized command args, lease metadata, success/error outcome, and created/started/completed timestamps.
- Write the daemon response first and remove the request file before appending the journal entry, avoiding duplicate command execution if journal writing fails.
- Add `session journal --state-dir <dir> [--limit N]` for recent journal inspection.
- Validate `--limit` as `1..=1000`.
- Treat corrupt journal lines as visible runtime errors instead of returning incomplete or fake success.
- Advertise `session journal` as an available offline diagnostic capability.

Safety direction:

- Journal append happens only after the request response is published and the request file is removed.
- A journal read failure or corrupt line fails loudly with a runtime error.
- This milestone does not change command execution semantics, lease enforcement, capture/input paths, or daemon request ordering.
- This milestone adds no UI, scheduler implementation, SQLite, OCR/OpenCV, game logic, ADB input fallback, capture hot-path algorithm change, reconnect loop, retry loop, or silent fallback.

Validation status:

- `cargo test -p actingcommand-actinglab session_request_journal_records_success_and_error -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab session_journal_corrupt_line_is_runtime_error -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab direct_touch_commands_are_capability_registered -- --nocapture` passed with `1` test.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed.
- Source-only added-code prohibited-feature scan returned `NO_PROHIBITED_CODE_ADDED_LINES`.

Known follow-ups:

- Surface journal summaries in future daemon health/status outputs when the UI/API contract is ready.
- Decide retention/rotation policy for long-running daemon journals.
- Implement the actual trusted interactive frame/input channel after the Runtime service boundary is accepted.
- Decide the daemon transport/API shape for long-lived frame streams instead of bounded local CLI sampling.
- Add live prepared-emulator validation for real captured stream frames when safe target states are available.
- Review UI/API stream consumption after the trusted channel contract lands.

## Previous ActingLab Bounded Stream Scaffold

The current Runtime task turns the future `stream` command from an unknown/reserved placeholder into a small, bounded, read-only Session Layer surface. It samples capture frames through the existing capture backend path, reports frame metadata, and keeps the future trusted input relay explicitly unimplemented.

Scope:

- Add `stream --max-frames <N>` bounded local frame sampling.
- Add `stream --dry-run --max-frames <N>` contract validation without device I/O.
- Add `stream --via-daemon` routing through the resident Session Layer request queue.
- Add `session request stream` as the explicit daemon request form.
- Cap frame count at `1..=60` to avoid accidental unbounded local streaming.
- Report frame digest, dimensions, backend, freshness, and capture backend attempts for captured frames.
- Report `input_relay.status=not_implemented` and `trusted_channel.status=reserved` until the trusted interactive channel is implemented.

Safety direction:

- The stream scaffold is read-only and capture-only; it does not start MaaTouch or issue input.
- `--input-relay` and `--interactive-input` fail explicitly with `stream_input_relay_not_implemented`.
- Daemon-routed stream requests do not require a lease because they are read-only, matching capture and semantic read-only requests.
- This milestone adds no UI, scheduler implementation, SQLite, OCR/OpenCV, game logic, ADB input fallback, capture hot-path algorithm change, reconnect loop, retry loop, or silent fallback.

Validation status:

- `cargo test -p actingcommand-actinglab stream_command_reports_bounded_dry_run_contract -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab stream_input_relay_is_explicitly_not_implemented -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab stream_via_daemon_without_daemon_is_runtime_error -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab session_request_stream_without_daemon_is_runtime_error -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab direct_touch_commands_are_capability_registered -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab top_level_record_capability_is_available -- --nocapture` passed with `1` test after updating the former reserved-stream assertion.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed.
- Source-only added-code prohibited-feature scan returned `NO_PROHIBITED_CODE_ADDED_LINES`.

Known follow-ups:

- Implement the actual trusted interactive frame/input channel after the Runtime service boundary is accepted.
- Decide the daemon transport/API shape for long-lived frame streams instead of bounded local CLI sampling.
- Add live prepared-emulator validation for real captured stream frames when safe target states are available.
- Review UI/API stream consumption after the trusted channel contract lands.

## Previous ActingLab Daemon Package/Operation Run Routing

The current Runtime task moves the remaining package/operation execution surfaces behind the resident Session Layer request boundary. `package run --via-daemon` and `operation run --via-daemon` now submit daemon control requests, with explicit `session request package-run` and `session request operation-run` forms.

Scope:

- Add `package run --via-daemon` routing.
- Add `operation run --via-daemon` routing.
- Add `session request package-run` routing.
- Add `session request operation-run` routing.
- Require matching session lease metadata before daemon-side package/operation run requests can read package or operation inputs or reach device I/O.
- Preserve existing direct local `package run` and `operation run` safety-blocked behavior.
- Advertise `session request package-run` and `session request operation-run` as available lease-gated capabilities.

Safety direction:

- Daemon-routed package/operation run requests are task-level control requests and require `--lease-holder` metadata plus an active matching lease.
- The lease gate runs before package zip validation, operation directory validation, capture backend creation, or MaaTouch/input setup.
- This milestone does not implement the reserved operation adapter or change the existing `package run` safety-blocked result behavior.
- This milestone adds no scheduler implementation, UI, SQLite, OCR/OpenCV, game logic, ADB input fallback, capture hot-path algorithm change, reconnect loop, retry loop, or silent fallback.

Validation status:

- `cargo test -p actingcommand-actinglab session_package_run_request_requires_lease_before_zip_or_device_io -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab session_operation_run_request_requires_lease_before_device_io -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab package_run_via_daemon_accepts_lease_flags_before_daemon_lookup -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab operation_run_via_daemon_accepts_lease_flags_before_daemon_lookup -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab session_request_package_run_without_daemon_is_runtime_error -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab session_request_operation_run_without_daemon_is_runtime_error -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab direct_touch_commands_are_capability_registered -- --nocapture` passed with `1` test.
- `cargo fmt --all -- --check` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed.
- `git diff --check` passed.
- Source-only added-code prohibited-feature scan returned no matches for fallback, reconnect/retry loops, direct capture/input execution in the new daemon package/operation routing, SQLite, OCR/OpenCV, or ADB shell input/screencap.

Known follow-ups:

- Implement the actual interactive frame/input stream after the Runtime service boundary and trusted-channel API are accepted.
- Add live prepared-emulator validation for `--capture --require-fresh` recording when a safe target state is available.
- Review whether direct local task-level execution should remain available for trusted manual use after scheduler/API integration lands.

## Previous ActingLab Daemon Lab Run Routing

The current Runtime task moves the trusted Lab package execution entry point behind the resident Session Layer request boundary. `lab run --via-daemon` now submits a daemon control request, and `session request lab-run` provides the explicit request form.

Scope:

- Add `lab run --via-daemon` routing.
- Add `session request lab-run` routing.
- Require matching session lease metadata before daemon-side Lab run requests can read the package zip or reach device I/O.
- Reuse the existing `lab run` implementation after the daemon lease gate; do not change Lab package execution semantics.
- Advertise `session request lab-run` as an available lease-gated capability.

Safety direction:

- Daemon-routed Lab runs are task-level control requests and require `--lease-holder` metadata plus an active matching lease.
- The lease gate runs before package zip validation, capture backend creation, or MaaTouch/input setup.
- This milestone does not change direct local `lab run` execution behavior.
- This milestone adds no scheduler implementation, UI, SQLite, OCR/OpenCV, game logic, ADB input fallback, capture hot-path algorithm change, reconnect loop, retry loop, or silent fallback.

Validation status:

- `cargo test -p actingcommand-actinglab session_lab_run_request_requires_lease_before_zip_or_device_io -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab lab_run_via_daemon_accepts_lease_flags_before_daemon_lookup -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab session_request_lab_run_without_daemon_is_runtime_error -- --nocapture` passed with `1` test.
- `cargo fmt --all -- --check` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed.
- `git diff --check` passed.
- Source-only added-code prohibited-feature scan returned no matches for fallback, reconnect/retry loops, direct capture/input execution in the new daemon Lab run routing, SQLite, OCR/OpenCV, or ADB shell input/screencap.

Known follow-ups:

- Continue moving package/operation execution workflows through the resident daemon request boundary.
- Implement the actual interactive frame/input stream after the Runtime service boundary and trusted-channel API are accepted.
- Add live prepared-emulator validation for `--capture --require-fresh` recording when a safe target state is available.

## Previous ActingLab Daemon Capture Routing

The current Runtime task moves the normal one-shot `capture` command behind the resident Session Layer request boundary. `capture --via-daemon --out <path>` now submits a read-only daemon request, and `session request capture --out <path>` provides the explicit request form.

Scope:

- Add `capture --via-daemon` routing.
- Add `session request capture` routing.
- Keep `--out`, `--require-fresh`, `--fresh-delay-ms`, and capture backend selection available to the daemon-executed command.
- Keep capture daemon requests read-only and lease-free.
- Advertise `session request capture` as an available capability.

Safety direction:

- This milestone does not change capture backend selection, stale-frame probing, PNG artifact writing semantics, or device input behavior.
- Capture writes only the requested local `--out` artifact and does not execute input.
- This milestone adds no scheduler implementation, UI, SQLite, OCR/OpenCV, game logic, ADB input fallback, direct MaaTouch startup, capture hot-path algorithm change, reconnect loop, retry loop, or silent fallback.

Validation status:

- `cargo test -p actingcommand-actinglab capture_via_daemon_without_daemon_is_runtime_error -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab session_request_capture_without_daemon_is_runtime_error -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab direct_touch_commands_are_capability_registered -- --nocapture` passed with `1` test.
- `cargo fmt --all -- --check` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed.
- `git diff --check` passed.
- Source-only added-code prohibited-feature scan returned no matches for fallback, reconnect/retry loops, ADB shell input/screencap, direct MaaTouch startup, SQLite, or OCR/OpenCV.

Known follow-ups:

- Continue moving package/operation execution workflows through the resident daemon request boundary.
- Implement the actual interactive frame/input stream after the Runtime service boundary and trusted-channel API are accepted.
- Add live prepared-emulator validation for `--capture --require-fresh` recording when a safe target state is available.

## Previous ActingLab Daemon Instance Lifecycle Routing

The current Runtime task moves the remaining Phase A instance lifecycle surface behind the resident Session Layer request boundary. `session instance list|health|reconnect` remains available as direct local Session Layer commands, and each can now also be submitted to the running daemon with `--via-daemon` or `session request instance ...`.

Scope:

- Add `session instance <list|health|reconnect> --via-daemon` routing.
- Add `session request instance <list|health|reconnect>` routing.
- Keep `list` and `health` read-only daemon requests.
- Require matching session lease metadata before daemon-side `reconnect` can reach device I/O.
- Advertise concrete `session instance ...` and `session request instance ...` capabilities.

Safety direction:

- Daemon-routed `list` and `health` are diagnostic/read-only.
- Daemon-routed `reconnect` is a device-affecting lifecycle request and requires `--lease-holder` metadata plus an active matching lease.
- This milestone does not change direct local `session instance` execution behavior.
- This milestone adds no scheduler implementation, UI, SQLite, OCR/OpenCV, game logic, ADB input fallback, direct MaaTouch startup, capture algorithm changes, reconnect loop, retry loop, or silent fallback.

Validation status:

- `cargo test -p actingcommand-actinglab session_instance_ -- --nocapture` passed with `4` tests.
- `cargo test -p actingcommand-actinglab session_request_instance_without_daemon_is_runtime_error -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab direct_touch_commands_are_capability_registered -- --nocapture` passed with `1` test.
- `cargo fmt --all -- --check` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed.
- `git diff --check` passed.
- Source-only added-code prohibited-feature scan returned no matches for fallback, reconnect/retry loops, direct capture/input execution in the new daemon instance routing, SQLite, OCR/OpenCV, or ADB shell input/screencap.

Known follow-ups:

- Continue moving package/operation execution workflows through the resident daemon request boundary.
- Implement the actual interactive frame/input stream after the Runtime service boundary and trusted-channel API are accepted.
- Add live prepared-emulator validation for `--capture --require-fresh` recording when a safe target state is available.

## Previous ActingLab Daemon App Lifecycle Routing

The current Runtime task moves one more Phase A lifecycle operation behind the resident Session Layer request boundary. `session app launch|stop|restart` remains available as a direct local Session Layer command, and it can now also be submitted to the running daemon with `--via-daemon` or `session request app ...`.

Scope:

- Add `session app <launch|stop|restart> --via-daemon` routing.
- Add `session request app <launch|stop|restart>` routing.
- Require matching session lease metadata before daemon-side app lifecycle requests reach device I/O.
- Advertise `session request app` and the concrete `session app launch|stop|restart` capabilities.

Safety direction:

- Daemon app lifecycle requests are task-level control requests and require `--lease-holder` metadata plus an active matching lease.
- This milestone does not change the direct `session app` execution behavior.
- This milestone adds no scheduler implementation, UI, SQLite, OCR/OpenCV, game logic, ADB input fallback, direct MaaTouch startup, capture algorithm changes, reconnect loop, retry loop, or silent fallback.

Validation status:

- `cargo test -p actingcommand-actinglab session_app -- --nocapture` passed with `2` tests.
- `cargo test -p actingcommand-actinglab session_request_app_without_daemon_is_runtime_error -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab direct_touch_commands_are_capability_registered -- --nocapture` passed with `1` test.
- `cargo fmt --all -- --check` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed.
- `git diff --check` passed.
- Source-only added-code prohibited-feature scan returned no matches for fallback, reconnect/retry loops, direct capture/input execution in the new daemon app routing, SQLite, OCR/OpenCV, or ADB shell input/screencap.

Known follow-ups:

- Continue moving lifecycle and device-control workflows through the resident daemon request boundary.
- Implement the actual interactive frame/input stream after the Runtime service boundary and trusted-channel API are accepted.
- Add live prepared-emulator validation for `--capture --require-fresh` recording when a safe target state is available.

## Previous ActingLab Session Interface Surface Alignment

The current Runtime task aligns the visible ActingLab CLI surface with the Session Layer interface draft without implementing the future UI/interactive stream itself.

Scope:

- Advertise `record start`, `record status`, and `record stop` as available offline capabilities.
- Advertise `session record start`, `session record status`, and `session record stop` as available offline capabilities.
- Add a top-level `stream` command entry point matching the Session Layer draft's future interactive frame/input channel.
- Keep `stream` explicitly reserved with a stable `stream_not_implemented` error instead of returning an unknown-command failure or fake success.

Safety direction:

- This milestone is interface-surface alignment only.
- This milestone adds no frame streaming, input relay, UI, TLS/authentication, scheduler, SQLite, OCR/OpenCV, game logic, device I/O, direct MaaTouch startup, ADB shell input/screencap, fallback, reconnect, or retry path.

Validation status:

- `cargo test -p actingcommand-actinglab stream_command_is_reserved_not_unknown -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab top_level_record_capability_is_available -- --nocapture` passed with `1` test.
- `cargo fmt --all -- --check` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed.
- `git diff --check` passed.
- Source-only added-code prohibited-feature scan returned no matches for device I/O, capture/input execution, SQLite, OCR/OpenCV, fallback, reconnect, or retry.

Known follow-ups:

- Implement the actual interactive frame/input stream after the Runtime service boundary and trusted-channel API are accepted.
- Add live prepared-emulator validation for `--capture --require-fresh` recording when a safe target state is available.
- Add UI/API surfaces for candidate review, color-check review, color-probe review, verify-template review, promotion, and amend flows after the CLI shape is accepted.

## Previous ActingLab Session Recording Build-Task Capability Close-Out

The current Runtime task closes the small interface gap left after enabling top-level `record ...`: the Session Layer interface draft names `record build-task`, and the implementation already routed it, but the capabilities surface did not advertise `record build-task` or `session record build-task`.

Scope:

- Advertise `session record build-task` as an available offline capability.
- Advertise top-level `record build-task` as an available offline capability.
- Add a top-level `record build-task` route test that proves the command reaches the existing recording implementation and fails with the same explicit `record_session_not_active` error when no recording context exists.
- Keep generated bundle behavior, resource promotion behavior, and existing `session record build-task` semantics unchanged.

Safety direction:

- This milestone is a CLI capability and routing close-out only.
- This milestone adds no device I/O, UI, SQLite, OCR/OpenCV, game logic, direct MaaTouch startup, ADB shell input/screencap, fallback, reconnect, or retry path.

Validation status:

- `cargo test -p actingcommand-actinglab top_level_record -- --nocapture` passed with `3` tests.
- `cargo test -p actingcommand-actinglab session_record_build_task_requires_record -- --nocapture` passed with `1` test.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed.
- Source-only added-code prohibited-feature scan returned no matches for ADB shell input/screencap, MaaTouch startup, direct tap/swipe execution, SQLite, OCR/OpenCV, fallback, reconnect, or retry.

Known follow-ups:

- Add live prepared-emulator validation for `--capture --require-fresh` recording when a safe target state is available.
- Add UI/API surfaces for candidate review, color-check review, color-probe review, verify-template review, promotion, and amend flows after the CLI shape is accepted.

## Previous ActingLab Session Recording Top-Level CLI Contract Alias

The current Runtime task aligns the Phase D recording CLI with the Session Layer interface draft by enabling the documented top-level `record ...` entry point. The existing `session record ...` command remains available and unchanged; both surfaces now share the same implementation and state files.

Scope:

- Route top-level `record <action> ...` to the existing recording implementation.
- Keep `session record <action> ...` fully compatible.
- Update capabilities so `record`, `record step`, `record candidates`, `record amend`, and `record promote` are available instead of reserved.
- Preserve the existing JSON envelope, exit-code mapping, validation rules, state path behavior, and offline/device capability labels.

Safety direction:

- This milestone is a CLI contract alias only.
- This milestone adds no device I/O, UI, SQLite, OCR/OpenCV, game logic, direct MaaTouch startup, ADB shell input/screencap, fallback, reconnect, or retry path.

Validation status:

- `cargo test -p actingcommand-actinglab top_level_record -- --nocapture` passed with `2` tests.
- `cargo test -p actingcommand-actinglab session_record -- --nocapture` passed with `41` tests.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- First `cargo test --workspace` run failed once in `detect_page_returns_standby_when_no_page_matches`; the isolated rerun passed and the full workspace rerun passed.
- `cargo test --workspace` passed on rerun.
- Source-only added-code prohibited-feature scan returned no matches for ADB shell input/screencap, MaaTouch startup, direct tap/swipe execution, SQLite, OCR/OpenCV, fallback, reconnect, or retry.

Known follow-ups:

- Add live prepared-emulator validation for `--capture --require-fresh` recording when a safe target state is available.
- Add UI/API surfaces for candidate review, color-check review, color-probe review, verify-template review, promotion, and amend flows after the CLI shape is accepted.

## Previous ActingLab Session Recording Amend Loop For Standalone Resources

The current Runtime task advances Phase D's "record, correct, and generate resources" path by extending the existing `session record amend` correction loop from anchors and operations to standalone `color-probe` and `verify-template` steps.

Scope:

- Allow `session record amend` to update `color-probe` ids and regions.
- Recompute frame-backed color-probe `expected` RGB values from the recorded source frame after amendments.
- Keep metadata-only color-probe amendments visibly `deferred` with reason `amended_without_frame_provenance` instead of producing fake colors.
- Allow `session record amend` to update `verify-template` ids, regions, thresholds, and clear-threshold requests.
- Re-materialize frame-backed verify-template artifacts and rerun offline self-backtests after amendments.
- Keep metadata-only verify-template amendments visibly `deferred` with reason `amended_without_frame_provenance` instead of producing fake artifacts.
- Extend `session record candidates` to report recorded auto-region candidates for standalone resource steps, while preserving `anchor_id` as a compatibility alias.

Safety direction:

- This milestone remains limited to offline recording metadata and artifact correction.
- This milestone adds no UI, SQLite, OCR/OpenCV, game logic, direct MaaTouch startup, ADB shell input/screencap, fallback, reconnect, or retry path.
- Device capture remains available only through the existing explicit recording inlet; the new tests use synthetic local PNG frames only.

Validation status:

- `cargo test -p actingcommand-actinglab session_record_amend_` passed with `9` tests.
- `cargo test -p actingcommand-actinglab session_record_candidates_` passed with `3` tests.
- `cargo test -p actingcommand-actinglab session_record -- --nocapture` passed with `40` tests.
- `cargo test -p actingcommand-actinglab` passed with `150` tests.
- `cargo fmt --all -- --check` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed.
- `git diff --check` passed.
- Source-only added-code prohibited-feature scan returned no matches for ADB shell input/screencap, MaaTouch startup, direct tap/swipe execution, SQLite, OCR/OpenCV, fallback, reconnect, or retry.

Known follow-ups:

- Add live prepared-emulator validation for `--capture --require-fresh` recording when a safe target state is available.
- Add UI/API surfaces for candidate review, color-check review, color-probe review, verify-template review, promotion, and amend flows after the CLI shape is accepted.

## Previous ActingLab Session Recording Standalone Verify-Template Output

The current Runtime task advances Phase D's "authorized resource generation" path by adding a standalone verify-template step kind. This records a reusable visual template target from an operator-authorized region and passes it through the same draft bundle and recognition-pack conversion pipeline as other generated resources.

Scope:

- Add `session record step --kind verify-template` and alias `--kind verify_template`.
- Support metadata-only verify-template steps as visibly `deferred` when no source frame is provided.
- Support frame-backed verify-template materialization through the existing local `--frame` / `--source-frame` path and explicit `--capture` / `--current-frame` inlet.
- Reuse the existing template crop, frame provenance, and offline self-backtest path used by anchors.
- Make `session record build-task` emit `verify_templates[]` in generated Operation Bundle 0.3 drafts.
- Copy generated verify-template assets into the draft task asset directory.
- Make build-task fail visibly when a verify-template is deferred and therefore has no frame artifact.
- Make `resource convert` validate verify-template asset paths and translate `verify_templates[]` into recognition-pack `type=template` targets.

Safety direction:

- This milestone is still limited to authoring and packaging data paths.
- This milestone adds no UI, SQLite, OCR/OpenCV, game logic, direct MaaTouch startup, ADB shell input/screencap, fallback, reconnect, or retry path.
- Real device capture remains available only through the already explicit `--capture` / `--current-frame` recording inlet.

Validation status:

- `cargo test -p actingcommand-actinglab session_record_step_verify_template -- --nocapture` passed with `2` tests.
- `cargo test -p actingcommand-actinglab session_record_build_task_rejects_deferred_verify_template -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab build_pack_includes_verify_template_targets -- --nocapture` passed with `1` test after the test fixture was corrected to use an absolute repository root.
- `cargo test -p actingcommand-actinglab session_record_build_task_writes_draft_bundle -- --nocapture` passed with `1` test and verified package dry-run compatibility.
- `cargo test -p actingcommand-actinglab session_record -- --nocapture` passed with `36` tests.
- `cargo test -p actingcommand-actinglab resource_convert -- --nocapture` passed with `7` tests.
- `cargo test -p actingcommand-actinglab` passed with `146` tests.
- `cargo fmt --all -- --check` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed after rerunning a transient full-suite failure that did not reproduce in the isolated test or the rerun.
- `git diff --check` passed.
- Source-only added-code prohibited-feature scan returned no matches for ADB shell input/screencap, MaaTouch startup, direct tap/swipe execution, SQLite, OCR/OpenCV, fallback, reconnect, or retry.

Known follow-ups:

- Add live prepared-emulator validation for `--capture --require-fresh` recording when a safe target state is available.
- Add a UI/API surface for candidate review, color-check review, color-probe review, verify-template review, and promotion after the CLI shape is accepted.

## Previous ActingLab Session Recording Standalone Color-Probe Output

The current Runtime task advances Phase D's "authorized resource generation" path by adding a standalone color-probe step kind. This is separate from anchor `--color-check`: color-probe records an explicit color target resource from an authorized region and sends it through the operation-bundle and recognition-pack conversion path.

Scope:

- Add `session record step --kind color-probe` and alias `--kind color_probe`.
- Support metadata-only color-probe steps as visibly `deferred` when no source frame is provided.
- Support frame-backed color-probe sampling through the existing local `--frame` / `--source-frame` path and explicit `--capture` / `--current-frame` inlet.
- Derive `expected` as the average RGB value over the authorized region in the recorded source frame.
- Preserve source-frame provenance and auto-region metadata when a frame-backed color-probe is materialized.
- Make `session record build-task` emit `color_probes[]` in generated Operation Bundle 0.3 drafts.
- Make build-task fail visibly when a color-probe is deferred and therefore has no expected color.
- Make `resource convert` translate `color_probes[]` into recognition-pack `type=color` targets.

Safety direction:

- This milestone is still limited to authoring and packaging data paths.
- This milestone adds no UI, SQLite, OCR/OpenCV, game logic, direct MaaTouch startup, ADB shell input/screencap, fallback, reconnect, or retry path.
- Real device capture remains available only through the already explicit `--capture` / `--current-frame` recording inlet.

Validation status:

- `cargo test -p actingcommand-actinglab session_record_step_color_probe -- --nocapture` passed with `2` tests.
- `cargo test -p actingcommand-actinglab build_pack_includes_color_probe_targets -- --nocapture` passed with `1` test.
- `cargo test -p actingcommand-actinglab session_record_build_task_writes_draft_bundle -- --nocapture` passed with `1` test and verified package dry-run compatibility.
- `cargo test -p actingcommand-actinglab session_record -- --nocapture` passed with `33` tests.
- `cargo test -p actingcommand-actinglab resource_convert -- --nocapture` passed with `6` tests.
- `cargo test -p actingcommand-actinglab` passed with `142` tests.
- `cargo fmt --all -- --check` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed.
- `git diff --check` passed.
- Source-only added-code prohibited-feature scan returned no matches for ADB shell input/screencap, MaaTouch startup, direct tap/swipe execution, SQLite, OCR/OpenCV, fallback, reconnect, or retry.

Known follow-ups:

- Add live prepared-emulator validation for `--capture --require-fresh` recording when a safe target state is available.
- Add amend support for color-probe steps only after the desired correction semantics are defined.
- Add a UI/API surface for candidate review, color-check review, color-probe review, and promotion after the CLI shape is accepted.

## Previous ActingLab Session Recording Anchor Color-Check Output

The current Runtime task advances Phase D's "authorized resource generation" path by making `record step --kind anchor --color-check` produce an actual bundle color check. Previously the flag was preserved only as provenance while the generated anchor still had `color_check: null`.

Scope:

- When a frame-backed recorded anchor has `color_check=true`, `session record build-task` now derives `color_check.expected` from the average RGB value of the authorized anchor rectangle in the recorded source frame.
- The generated `color_check.region` uses the same rect as the materialized anchor artifact.
- Anchors without `--color-check` continue to emit `color_check: null`.
- Missing frame provenance for a requested color check fails visibly during build.
- Existing package compatibility remains unchanged: the generated bundle still passes the existing `package build-task --dry-run` path.

Safety direction:

- This milestone is pure offline bundle generation.
- This milestone performs no device I/O, MaaTouch startup, frame capture, resource repository write, OCR/OpenCV, SQLite, UI, or game logic.

Validation status:

- `cargo test -p actingcommand-actinglab session_record_build_task_writes_draft_bundle -- --nocapture` passed with `1` test and verified the emitted color-check data plus package dry-run compatibility.
- `cargo test -p actingcommand-actinglab session_record -- --nocapture` passed with `30` tests.
- `cargo test -p actingcommand-actinglab` passed with `138` tests.
- `cargo fmt --all -- --check` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed.
- `git diff --check` passed.
- Source-only added-code prohibited-feature scan returned no matches for ADB shell input/screencap, MaaTouch startup, direct tap/swipe execution, SQLite, OCR/OpenCV, fallback, reconnect, or retry.

Known follow-ups:

- Add live prepared-emulator validation for `--capture --require-fresh` recording when a safe target state is available.
- Add additional recording resource kinds such as standalone color-probe or verify-template after the anchor/operation promotion loop is accepted.
- Add a UI/API surface for candidate review, color-check review, and promotion after the CLI shape is accepted.

## Previous ActingLab Session Recording Resource Promotion

The current Runtime task advances Phase D's "record, correct, and generate resources" loop by adding an explicit promotion path from a recording context into a resource repository. This keeps resource writes deliberate and guarded instead of making `build-task` silently overwrite a repository.

Scope:

- Add `session record promote --repo <resource-repo-or-root>` for existing resource roots and repositories containing `ours/`.
- Add alias `session record publish` for the same guarded promotion path.
- Reuse the existing `session_record_build_draft` validation so promoted tasks must pass the same anchor, operation, coordinate, and page-reference checks as draft builds.
- Resolve repository roots consistently with the existing resource/package builder path, including `<repo>/ours`.
- Refuse to overwrite an existing task directory unless `--force` is supplied.
- When `--force` is supplied, replace only the promoted task directory.
- Preserve an existing shared `operations/resources.json`; create the empty placeholder only when it is missing.
- Return promoted task paths, resource root/layout, resource action, counts, and asset destinations in JSON.
- Expose the command through `capabilities`.

Safety direction:

- This milestone is an offline resource-write path only; it does not open MaaTouch or perform device I/O.
- This milestone does not capture frames, run OCR/OpenCV, touch SQLite, implement UI, or add game logic.
- Existing task directories fail visibly by default and require explicit `--force` before replacement.

Validation status:

- `cargo test -p actingcommand-actinglab session_record_promote -- --nocapture` passed with `1` test, including `package build-task --dry-run` against the promoted resource repository.
- `cargo test -p actingcommand-actinglab session_record -- --nocapture` passed with `30` tests.
- `cargo test -p actingcommand-actinglab` passed with `138` tests.
- `cargo fmt --all -- --check` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed.
- `git diff --check` passed.
- Source-only added-code prohibited-feature scan returned no matches for ADB shell input/screencap, MaaTouch startup, direct tap/swipe execution, SQLite, OCR/OpenCV, fallback, reconnect, or retry.

Known follow-ups:

- Add live prepared-emulator validation for `--capture --require-fresh` recording when a safe target state is available.
- Add additional recording resource kinds such as color-probe or verify-template after the anchor/operation promotion loop is accepted.
- Add a UI/API surface for candidate review and promotion after the CLI shape is accepted.

## Previous ActingLab Session Recording Candidate Preview

The current Runtime task advances Phase D's "suggest, confirm, and micro-adjust" loop by adding a read-only candidate preview command. Operators can inspect the candidate report produced by `--region auto` before choosing one with `session record amend --candidate-index`.

Scope:

- Add `session record candidates <step-id>` for anchor steps with an existing auto-region report.
- Add alias `session record candidate-list <step-id>` for the same read-only path.
- Return record id, task id, instance, step id, anchor id, current region, evaluation status, full `auto_region` report, `candidate_count`, and `selected_index`.
- Require an existing `evaluation.auto_region.candidates` report and fail visibly when a step has no candidate report.
- Expose the command through `capabilities`.

Safety direction:

- This milestone is read-only against the recording context.
- This milestone performs no direct click/navigation execution and does not open MaaTouch.
- This milestone does not capture frames, write resource repositories, run OCR/OpenCV, touch SQLite, implement UI, or add game logic.

Validation status:

- `cargo test -p actingcommand-actinglab session_record_candidates -- --nocapture` passed with `2` tests.
- `cargo test -p actingcommand-actinglab session_record -- --nocapture` passed with `29` tests.
- `cargo test -p actingcommand-actinglab` passed with `137` tests.
- `cargo fmt --all -- --check` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed.
- `git diff --check` passed.
- Source-only added-code prohibited-feature scan returned no matches for ADB shell input/screencap, MaaTouch startup, direct tap/swipe execution, SQLite, OCR/OpenCV, fallback, reconnect, or retry.

Known follow-ups:

- Add live prepared-emulator validation for `--capture --require-fresh` recording when a safe target state is available.
- Add a UI/API surface for candidate review after the CLI shape is accepted.

## Previous ActingLab Session Recording Amend-By-Candidate Loop

The current Runtime task advances Phase D's "suggest, confirm, and micro-adjust" loop. After an auto-region step records candidate metadata, an operator can now select one of those candidates directly during `session record amend` without manually copying rectangle coordinates.

Scope:

- Add `session record amend <step> --candidate-index <n>` for anchor steps.
- Add alias `--auto-candidate <n>` for the same candidate-selection path.
- Require an existing `evaluation.auto_region.candidates` report; candidate selection fails visibly when the step has no candidate report.
- Reject missing, conflicting, or out-of-range candidate index input.
- Convert the selected candidate into the step's explicit rect region.
- Preserve candidate provenance in `evaluation.auto_region` with `selected_reason=operator_selected_candidate`.
- Immediately re-crop, rewrite the artifact, and re-run existing self/contrast backtests after candidate selection.

Safety direction:

- This milestone performs no direct click/navigation execution and does not open MaaTouch.
- This milestone does not write resource repositories, run OCR/OpenCV, touch SQLite, implement UI, or add game logic.
- Selecting a bad candidate is allowed but never hidden: final self/contrast backtest can fail visibly and will block downstream build-task usage.

Validation status:

- `cargo test -p actingcommand-actinglab session_record_amend -- --nocapture` passed with `6` tests.
- `cargo test -p actingcommand-actinglab session_record_step_anchor_auto -- --nocapture` passed with `3` tests.
- `cargo test -p actingcommand-actinglab session_record -- --nocapture` passed with `27` tests.
- `cargo test -p actingcommand-actinglab` passed with `135` tests.
- `cargo fmt --all -- --check` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed.
- `git diff --check` passed.
- Source-only added-code prohibited-feature scan returned no matches for ADB shell input/screencap, MaaTouch startup, direct tap/swipe execution, SQLite, OCR/OpenCV, fallback, reconnect, or retry.

Known follow-ups:

- Add live prepared-emulator validation for `--capture --require-fresh` recording when a safe target state is available.
- Add resource-promotion/write flow after recording correction semantics are accepted.

## Current ActingLab Session Recording Auto-Region Candidate Report

The current Runtime task advances Phase D toward operator-confirmable resource generation. `session record step --kind anchor --region auto` now exposes why a region was selected, rather than silently choosing one rectangle. When a contrast frame is provided, selection prefers candidates that still match the source frame but are rejected by the contrast frame.

Scope:

- Add `evaluation.auto_region` metadata for source-frame-backed auto-region anchors.
- Record selection strategy, selected reason, selected rect, and the full bounded candidate list.
- Record per-candidate luma variance.
- When a contrast frame is provided, record each candidate's contrast score and pass/fail result.
- Prefer contrast-rejected candidates before falling back to the lowest contrast score.
- Keep final self/contrast backtest semantics unchanged: selected candidates still must pass existing evaluation before build-task can use them.
- Keep no-frame `--region auto` deferred and artifact-free.

Safety direction:

- This milestone performs no direct click/navigation execution and does not open MaaTouch.
- This milestone does not write resource repositories, run OCR/OpenCV, touch SQLite, implement UI, or add game logic.
- Contrast-frame read/decode/scoring failures surface as validation errors; no candidate metadata is fabricated.

Validation status:

- `cargo test -p actingcommand-actinglab session_record_step_anchor_auto -- --nocapture` passed with `3` tests.
- `cargo test -p actingcommand-actinglab session_record -- --nocapture` passed with `25` tests.
- `cargo test -p actingcommand-actinglab` passed with `133` tests.
- `cargo fmt --all -- --check` passed.
- `cargo clippy --workspace -- -D warnings` passed after grouping auto-region resolution data into `SessionRecordAnchorRegionResolution`.
- `cargo test --workspace` passed.
- `git diff --check` passed.
- Source-only added-code prohibited-feature scan returned no matches for ADB shell input/screencap, MaaTouch startup, direct tap/swipe execution, SQLite, OCR/OpenCV, fallback, reconnect, or retry.

Known follow-ups:

- Add explicit operator selection or amend-by-candidate once the CLI/API shape is accepted.
- Add live prepared-emulator validation for `--capture --require-fresh` recording when a safe target state is available.
- Add resource-promotion/write flow after recording correction semantics are accepted.

## Current ActingLab Session Recording Auto-Region Candidate Slice

The current Runtime task advances Phase D by making `session record step --kind anchor --region auto` usable when a source frame is explicitly provided. The first implementation keeps candidate selection local and deterministic, then immediately passes the selected rect through the existing crop, artifact, self-backtest, and optional contrast-backtest path.

Scope:

- Support frame-backed `--region auto` with local `--frame` / `--source-frame` and explicit current-frame capture input.
- Resolve auto-region to a stored `rect` before writing the step, so generated draft bundles receive usable coordinates.
- Use a bounded brightness-variance heuristic over a small deterministic candidate grid.
- Keep no-frame `--region auto` supported as an explicit deferred authoring intent with reason `frame_not_provided`.
- Reuse the same source-frame provenance, artifact generation, and self/contrast backtest path as rect-backed anchors.
- Allow amended frame-backed anchors that still contain `auto` metadata to resolve through the same source-frame path.

Safety direction:

- This milestone performs no direct click/navigation execution and does not open MaaTouch.
- This milestone does not write resource repositories, run OCR/OpenCV, touch SQLite, implement UI, or add game logic.
- Auto-region selection never fabricates success without a source frame: source-backed selection must pass the existing materialization/backtest path, and no-frame anchors remain visibly deferred.

Validation status:

- `cargo test -p actingcommand-actinglab session_record_step_anchor_auto -- --nocapture` passed with `2` tests.
- `cargo test -p actingcommand-actinglab session_record -- --nocapture` passed with `24` tests.
- `cargo test -p actingcommand-actinglab` passed with `132` tests.
- `cargo fmt --all -- --check` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed.
- `git diff --check` passed.
- Source-only added-code prohibited-feature scan returned no matches for ADB shell input/screencap, MaaTouch startup, direct tap/swipe execution, SQLite, OCR/OpenCV, fallback, reconnect, or retry.

Known follow-ups:

- Add richer multi-candidate reporting or operator selection UI after this minimal candidate path is accepted.
- Add live prepared-emulator validation for `--capture --require-fresh` recording when a safe target state is available.
- Add resource-promotion/write flow after recording correction semantics are accepted.

## Current ActingLab Session Recording Amend Re-Backtest Loop

The current Runtime task advances Phase D by closing the correction loop required by the session-layer recording plan. When an authorized anchor step already has source-frame provenance, `session record amend` now recalculates the template artifact and evaluation immediately after supported anchor metadata changes.

Scope:

- Reuse existing source-frame provenance for frame-backed anchor amendments.
- Preserve original source-frame capture/local provenance, freshness metadata, and recorded timestamp when re-reading the source frame.
- Re-crop and rewrite the anchor artifact after changing region, id, color-check flag, or threshold.
- Re-run the existing self-backtest and optional contrast-backtest path after amendment.
- Keep no-frame anchors explicit: amendments remain deferred with reason `amended_without_frame_provenance`.
- Keep operation amendment behavior unchanged.

Safety direction:

- This milestone performs no device I/O and does not capture a new frame during amend.
- This milestone does not write resource repositories, open MaaTouch, click, navigate, run OCR/OpenCV, touch SQLite, implement UI, or add game logic.
- Missing or unreadable recorded source frames fail visibly during amend instead of silently keeping stale evaluation data.

Validation status:

- `cargo test -p actingcommand-actinglab session_record_amend -- --nocapture` passed with `4` tests.
- `cargo test -p actingcommand-actinglab session_record -- --nocapture` passed with `23` tests.
- `cargo test -p actingcommand-actinglab` passed with `131` tests.
- `cargo fmt --all -- --check` passed.
- `cargo clippy --workspace -- -D warnings` passed after grouping anchor-amend mutable fields into `SessionRecordAnchorAmendTarget`.
- `cargo test --workspace` passed.
- `git diff --check` passed.
- Source-only added-code prohibited-feature scan returned no matches for ADB shell input/screencap, MaaTouch startup, direct tap/swipe execution, SQLite, OCR/OpenCV, fallback, reconnect, or retry.

Known follow-ups:

- Add live prepared-emulator validation for `--capture --require-fresh` recording when a safe target state is available.
- Add resource-promotion/write flow after recording correction semantics are accepted.
- Add `--region auto`, color-probe, or verify-template recording step kinds in later milestones.

## Current ActingLab Session Recording Current-Frame Inlet

The current Runtime task advances Phase D by connecting authorized anchor recording to the Session Layer capture path. `session record step --kind anchor` can still operate fully offline with `--frame`, and now can also explicitly request the current device frame with `--capture` or `--current-frame`.

Scope:

- Add `--capture` / `--current-frame` to frame-backed anchor recording.
- Keep capture explicit; recording still does not auto-record anything.
- Reuse the existing `capture_for_command` path, selected capture backend, `--require-fresh`, and `--fresh-delay-ms`.
- Persist a source-frame PNG under the recording artifact directory when a frame is captured from the device.
- Add provenance metadata for current-capture anchors: capture backend, freshness record, and capture attempts.
- Keep local `--frame` / `--source-frame` behavior unchanged and reject mixing local frame input with `--capture`.
- Reuse the same crop, self-backtest, contrast-frame, and artifact generation path for local and captured source frames.
- Mark `session record step` as both offline and device-capable in the capability list.

Safety direction:

- This milestone does not write resource repositories.
- This milestone does not open MaaTouch, click, navigate, run OCR/OpenCV, touch SQLite, implement UI, or add game logic.
- Capture failures surface as device errors; local frame/crop/validation failures surface as validation errors.
- `--require-fresh` remains available for stale-frame-sensitive recording, and captured-frame provenance records the freshness result.

Validation status:

- `cargo test -p actingcommand-actinglab session_record -- --nocapture` passed with `22` tests.
- `cargo test -p actingcommand-actinglab` passed with `130` tests.
- `cargo fmt --all -- --check` passed.
- `cargo clippy --workspace -- -D warnings` passed after replacing an over-wide helper signature with a small recording-step context.
- `cargo test --workspace` passed.
- `git diff --check` passed.
- Source-only added-code prohibited-feature scan returned no matches for ADB shell input/screencap, MaaTouch startup, direct tap/swipe execution, SQLite, OCR/OpenCV, fallback, reconnect, or retry.

Known follow-ups:

- Run a live `--capture --require-fresh` recording flow only when the target emulator state is intentionally prepared for recording.
- Add resource-promotion/write flow after generated draft bundles and current-frame provenance are accepted.
- Add `--region auto`, color-probe, or verify-template recording step kinds in later milestones.

## Current ActingLab Session Recording Package Handoff

The current Runtime task advances Phase D by closing the offline handoff between the recording authoring path and the existing Lab package builder. A `session record build-task` draft now has to be structurally acceptable to `package build-task --dry-run`, rather than merely writing a local `task.json`.

Scope:

- Keep `session record build-task` as an offline draft-output command.
- Preserve the Operation Bundle 0.3-style output under `<out>/operations/<task_id>/task.json`.
- Add package compatibility coverage by running `package build-task --dry-run` against a generated recording draft in tests.
- Use a numeric `defaults.color_max_distance` value in generated drafts so the resulting recognition pack validates.
- Require operation `from`, `to`, `entry_page`, and `target_page` page references to have matching anchors, with `any` and `<page>_variant` anchors following the existing converter semantics.
- Validate point-click coordinates against the bundle coordinate space before writing a draft.
- Keep unresolved target-click operations rejected before page-reference validation.

Safety direction:

- This milestone performs no device I/O and does not open MaaTouch.
- This milestone does not live-capture frames, write resource repositories, touch SQLite, implement UI, add OCR/OpenCV, or add game logic.
- Missing page anchors, out-of-bounds clicks, unresolved target clicks, malformed clicks, and package-incompatible defaults fail visibly during `record build-task` or the package dry-run test path.

Validation status:

- `cargo test -p actingcommand-actinglab session_record_build_task -- --nocapture` passed with `5` tests.
- `cargo test -p actingcommand-actinglab session_record -- --nocapture` passed with `20` tests.
- `cargo test -p actingcommand-actinglab` passed with `128` tests.
- `cargo fmt --all -- --check` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed.
- `git diff --check` passed.
- Source-only added-code prohibited-feature scan returned no matches for ADB input fallback, `adb shell screencap`, MaaTouch startup, live capture routing, SQLite, OCR/OpenCV, fallback, reconnect, or retry.

Known follow-ups:

- Add current-frame integration only after stale-frame policy and daemon routing are fully aligned with recording.
- Add resource-promotion/write flow after the offline package handoff remains stable.
- Add additional recording step kinds such as color-probe or verify-template in later milestones.

## Current ActingLab Session Recording Anchor Contrast Validation

The current Runtime task advances Phase D by adding optional offline contrast-frame validation to authorized frame-backed anchor steps. A usable anchor now can prove both sides of the intended distinction: it must match its source frame inside the authorized rect, and, when provided, it must not match the supplied contrast frame above the same threshold.

Scope:

- Add optional `--contrast-frame <png>` to `session record step --kind anchor`.
- Add alias `--negative-frame <png>` for the same contrast-frame role.
- Preserve the existing no-contrast behavior: frame-backed anchors still self-test and pass/fail exactly as before when no contrast frame is supplied.
- When a contrast frame is supplied, persist a `contrast_backtest` record with source, path, hash, dimensions, metric, region, match point, score, threshold, and pass/fail.
- Mark the anchor evaluation `passed` only when the source-frame self-test passes and the contrast-frame score remains below threshold.
- Mark the anchor evaluation `failed` with reason `contrast_backtest_matched` when the contrast frame also matches.
- Clear contrast-backtest metadata when an anchor is amended.

Safety direction:

- This milestone performs no device I/O and does not open MaaTouch.
- This milestone does not live-capture frames, write resource repositories, touch SQLite, implement UI, add OCR/OpenCV, or add game logic.
- Contrast frame read/decode errors and recognition errors fail visibly.
- Failed contrast validation is recorded visibly in the step evaluation, and downstream `build-task` continues to reject non-passed anchors.

Validation status:

- `cargo test -p actingcommand-actinglab session_record_step_anchor -- --nocapture` passed with `6` tests.
- `cargo test -p actingcommand-actinglab session_record -- --nocapture` passed with `18` tests.
- `cargo test -p actingcommand-actinglab` passed with `126` tests.
- `cargo fmt --all -- --check` passed.
- `cargo clippy --workspace -- -D warnings` passed after boxing anchor-step evaluation metadata to keep the recording enum compact without changing the JSON shape.
- `cargo test --workspace` passed.
- `git diff --check` passed.
- Source-only added-code prohibited-feature scan returned no matches for device input fallback, `adb shell screencap`, MaaTouch startup, live capture routing, SQLite, OCR/OpenCV, fallback, reconnect, or retry.

Known follow-ups:

- Add current-frame integration only after stale-frame policy and daemon routing are fully aligned with recording.
- Add resource-promotion/write flow after draft bundle and contrast validation semantics are accepted.
- Consider additional resource types such as color-probe or verify-template in later recording milestones.

## Current ActingLab Session Recording Build-Task Draft

The current Runtime task advances Phase D by adding `session record build-task` as an offline draft bundle generator. It consumes an existing local recording context, requires frame-backed anchors to have passed their self-backtest, copies draft anchor artifacts into an operation-task directory, and emits an Operation Bundle 0.3-style `task.json` plus a placeholder `operations/resources.json`.

Scope:

- Add `session record build-task --out <dir>`.
- Allow active or stopped recording contexts.
- Resolve game/server from flags, global options, configured instance metadata, or game defaults.
- Infer coordinate space from the first frame-backed anchor, or require `--resolution <width>x<height>` when no frame provenance exists.
- Require at least one operation step.
- Reject unresolved target-click operations; only explicit coordinate clicks are bundle-ready in this milestone.
- Require every exported anchor to have a local artifact and a `passed` self-backtest.
- Copy draft anchor PNG artifacts into `<out>/operations/<task_id>/assets/`.
- Write `<out>/operations/<task_id>/task.json` and `<out>/operations/resources.json`.
- Add `--dry-run` validation mode that returns the assembled bundle without writing files.
- Use `u64` Unix millisecond timestamps for session/lease/record JSON persistence so records can be written and read back reliably through `serde_json`.

Safety direction:

- This milestone performs no device I/O and does not open MaaTouch.
- This milestone does not live-capture frames, run contrast-frame validation, write resource repositories, touch SQLite, implement UI, or add game logic.
- Missing records, unsafe task ids, missing anchor artifacts, failed/deferred anchor backtests, missing resolution, unresolved target clicks, and file-copy/write failures fail visibly.

Validation status:

- `cargo test -p actingcommand-actinglab session_record -- --nocapture` passed with `16` tests after fixing JSON timestamp persistence from `u128` to `u64`.
- `cargo test -p actingcommand-actinglab` passed with `124` tests.
- `cargo fmt --all -- --check` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed.
- `git diff --check` passed.
- Source-only added-code prohibited-feature scan returned no matches for device input fallback, `adb shell screencap`, MaaTouch startup, live capture routing, SQLite, OCR/OpenCV, fallback, reconnect, or retry.

Known follow-ups:

- Add capture/current-frame integration only after stale-frame policy and daemon routing are fully aligned with recording.
- Add contrast-frame or cross-frame validation before promoting draft artifacts into resource repositories.
- Add explicit resource-write/promotion flow later; this milestone writes only a local draft output tree.

## Current ActingLab Session Recording Anchor Self-Backtest

The current Runtime task advances Phase D by changing frame-backed anchor recording from draft-only materialization to draft materialization plus immediate offline self-backtest. The check reuses the existing recognition primitive: it matches the generated crop against the supplied source frame inside the authorized rect and records the result on the anchor evaluation.

Scope:

- For `session record step --kind anchor --frame <png> --region x,y,width,height`, run a local self-backtest after crop artifact generation.
- Record evaluation status `passed` or `failed` with reason `self_backtest_passed` or `self_backtest_below_threshold`.
- Persist backtest metadata:
  - source
  - metric
  - region
  - match point
  - raw score
  - normalized score
  - effective threshold
  - pass/fail boolean
- Reuse existing `--metric` parsing where a frame-backed anchor is supplied.
- Use explicit `--threshold` when provided; otherwise use a conservative `0.95` anchor self-test threshold.
- Preserve metadata-only anchor steps as `deferred` with reason `frame_not_provided`.
- Reset evaluation back to `deferred` with no backtest when an anchor is amended.

Safety direction:

- This milestone performs no device I/O and does not open MaaTouch.
- This milestone does not live-capture frames, run contrast-frame validation, write resource packs, generate task bundles, touch SQLite, implement UI, or add game logic.
- Backtest failures are recorded visibly in the step evaluation; decode, crop, and matching errors fail the command.

Validation status:

- Runtime was already aligned with `origin/main` before this task.
- `cargo test -p actingcommand-actinglab session_record_step_anchor -- --nocapture` passed with `4` tests.
- `cargo test -p actingcommand-actinglab session_record -- --nocapture` passed with `14` tests.
- `cargo test -p actingcommand-actinglab` passed with `122` tests.
- `cargo fmt --all -- --check` passed.
- `cargo clippy --workspace -- -D warnings` passed.
- `cargo test --workspace` passed when run by itself; an earlier parallel run with clippy produced one transient `lab_validate_accepts_minimal_self_contained_package` failure, and the failing test passed immediately when rerun alone.
- `git diff --check` passed.
- Added-code prohibited-feature scan over source changes returned no matches for device input fallback, `adb shell screencap`, MaaTouch startup, SQLite, OCR/OpenCV, fallback, reconnect, retry, or live capture routing.

Known follow-ups:

- Add capture/current-frame integration only after the session daemon and stale-frame policy are wired into the recording path.
- Add contrast-frame or cross-frame validation before promoting draft artifacts into resource repositories.
- Implement `session record build-task` and resource-write integration in a later milestone.

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

## Current ActingLab Command-Check Throat Gate Summary

This increment keeps the Session Layer unique-throat boundary visible at the command-preflight level.

`session command-check <command...>` now exposes `throat_gate` with:

- whether the command is device-affecting;
- whether the command must go through the Session Layer;
- whether daemon routing is required, selected, and alive;
- whether direct ADB/device access is disallowed;
- whether device control requires a LabLease;
- whether a local override is allowed or blocked.

`session submit-plan <command...>` mirrors the key throat fields into `preflight_summary`, so UI and scheduler clients can present a compact decision without parsing the whole command-check payload.

This is a contract/discovery increment only:

- no daemon request is enqueued;
- no device, ADB, capture backend, MaaTouch backend, resource repository, OCR, SQLite, UI, or game logic is touched;
- no self-heal action is executed;
- no interactive stream is started;
- no trusted remote listener, token issuer, TLS layer, or encrypted channel is implemented.

Phase C plan alignment:

- Self-heal should stay observe-first: diagnose capture freshness, queue health, daemon liveness, lease ownership, and selected-instance readiness before recommending or executing maintenance-only recovery.
- Interaction flow should remain lease-aware and replayable: streams expose bounded events and optional input relay only through Session Layer, and input relay requires matching lease metadata.
- Trusted encrypted channel should remain reserved until the local CLI/file-IPC contract is stable: future remote access must require authentication, authorization, bounded request admission, and a clear failure code before it can submit Session Layer work.

## Current ActingLab Self-Heal Execution Gate

This increment keeps Phase C self-heal planning readable by UI and scheduler clients without requiring them to reconstruct readiness, queue, lease, and blocker state from separate fields.

`session self-heal-plan [--trigger <kind>] [--to <page>]` now exposes `execution_gate` with:

- `schema_version=session.self_heal_execution_gate.v0.1`;
- trigger selection state;
- recovery kind and target page;
- readiness, queue-admission, and lease readiness booleans;
- `safe_to_execute_maintenance`;
- mirrored blocked reasons;
- the submit-plan command for the selected recovery path.

This is still a contract/discovery increment only:

- no recovery is executed;
- no daemon request is enqueued;
- no capture, MaaTouch, ADB, app lifecycle, resource repository, SQLite, UI, listener, token, or TLS work is performed;
- live validation remains `deferred: requires-live-device`.

## Current ActingLab Trusted Remote Gate

This increment keeps the future encrypted trusted-channel lane explicit without starting a remote listener or introducing TLS/token implementation work in this offline milestone.

`session transport plan [--endpoint <url>]` now exposes `trusted_remote_gate` with:

- `schema_version=session.trusted_remote_gate.v0.1`;
- endpoint-policy checked/safe state;
- endpoint channel and trusted-remote-requested state;
- token/client-certificate/auth-material readiness;
- listener, TLS, and token-issuer implementation flags;
- request-serialization and audit-log requirements;
- blocked reasons;
- live-validation deferral.

The local CLI and daemon file-IPC surfaces remain available without encryption, but trusted remote/UI access remains reserved until later implementation work provides encryption, authentication, request admission, audit logging, and live validation.

This is still a contract/discovery increment only:

- no network listener is started;
- no TCP endpoint is probed;
- no token is issued;
- no TLS layer is implemented;
- no daemon request, capture, MaaTouch, ADB, app lifecycle, resource repository, SQLite, UI, or scheduler runtime work is performed;
- live validation remains `deferred: requires-live-device`.

## Current ActingLab Phase C Implementation Plan Contract

This increment makes the Phase C roadmap itself machine-readable through `session phase-c-plan`.

`session phase-c-plan [--endpoint <url>] [--trigger <kind>] [--to <page>]` now embeds `implementation_plan` with:

- `schema_version=session.phase_c_implementation_plan.v0.1`;
- ordered rollout lanes for `self_heal`, `interaction_flow`, `trusted_channel`, and `live_acceptance`;
- self-heal implementation steps that classify stale capture before assuming game freeze, prefer lighter capture-backend recovery before app restart, gate maintenance execution on daemon/queue/lease readiness, and journal recovery decisions;
- interaction-flow implementation steps that keep bounded local streams available, route daemon stream work through Session Layer requests, and require matching lease metadata before any input relay;
- trusted-channel implementation steps that keep local CLI/file-IPC as the current trusted surface while blocking trusted remote clients until encryption, authentication, listener, token/client-certificate material, request admission, and audit logging are implemented;
- explicit live-validation deferral for prepared-device checks.

`session api` now advertises the implementation-plan field and schema version under `phase_c_plan_view`.

This is still a contract/discovery increment only:

- no recovery is executed;
- no daemon request is enqueued;
- no stream is opened;
- no capture, MaaTouch, ADB, app lifecycle, resource repository, SQLite, UI, listener, token, or TLS work is performed;
- live validation remains `deferred: requires-live-device`.

## Current ActingLab Capture Freeze Classification Gate

This increment makes the AK stale-screencap finding harder for clients to misinterpret as a confirmed game freeze.

`session capture-policy` now embeds `freeze_classification_gate` with:

- `schema_version=session.capture_freeze_classification_gate.v0.1`;
- `status=blocked_without_fresh_backend_evidence`;
- `safe_to_classify_game_frozen=false`;
- the explicit rule that `adb_screencap` evidence alone must not classify the game as frozen;
- insufficient-evidence examples including same-md5 `adb_screencap`, reconnect-same-md5, input-ok-without-fresh-frame, high CPU without stronger evidence, and page detection from a stale frame;
- required evidence before a live game-freeze label: fresh-frame diagnosis, backend/hash/timestamp evidence, frame comparison or stale proof, lighter non-ADB screenshot backend checks where available, and operator/live evidence;
- live validation still marked `deferred: requires-live-device`.

`session api` now advertises the freeze-classification gate field and schema version under `capture_policy_view`.

Resident daemon request journals now summarize `capture_policy` results, including freeze-gate status, safe-to-classify state, required-evidence count, and live-validation deferral.

This is still a contract/discovery increment only:

- no capture is run;
- no daemon request is enqueued by the policy command;
- no recovery is executed;
- no capture backend, MaaTouch, ADB, app lifecycle, resource repository, SQLite, UI, listener, token, or TLS work is performed;
- live validation remains `deferred: requires-live-device`.

## Current ActingLab Session Recording Authorization Policy

This increment makes the recording/resource-authoring boundary discoverable before UI, scheduler, or agent clients ask Runtime to record steps.

`session record-policy` now exposes:

- `schema_version=session.record_policy.v0.1`;
- an active authorization model where passive full recording is not allowed;
- explicit allowed step kinds: `anchor`, `operation`, `color-probe`, and `verify-template`;
- frame-source policy for local PNG and explicitly requested current-frame capture;
- resource-write policy where draft build and repository promotion remain explicit commands;
- safety policy blocking blind confirmation, game-progress actions, and paid-resource use;
- client guidance for UI prompts, step-kind selection, frame-source selection, amend-before-build review, and promote warnings;
- live validation marked `deferred: requires-live-device`.

`session request record-policy` returns the same payload through the resident daemon request queue, and request journals summarize `record_policy` results without forcing clients to parse the full policy payload.

`session api`, `session bootstrap`, `session command-check`, and `capabilities` now advertise the record-policy route and view contract.

This is still a contract/discovery increment only:

- no recording session is created;
- no daemon request is enqueued by the local policy command;
- no frame is captured;
- no MaaTouch, ADB, app lifecycle, resource repository, SQLite, UI, listener, token, or TLS work is performed;
- no resource repository is read or written;
- live validation remains `deferred: requires-live-device`.

## Current Phase C session-layer direction

- Self-heal remains observe-first: detect stale capture, frozen page, daemon liveness, blocked queue, stale lease, and failed request states before recovery is proposed.
- Self-heal recovery execution must stay lease-aware and maintenance-only until live validation proves the route for each game/server.
- Interaction flow uses bounded `stream` plus explicit input relay events, but UI/scheduler clients must call `session command-check` or `session submit-plan` before submitting control input.
- `stream --input-event <action,args>` and `stream --relay-event <action,args>` are control requests and require a matching Session Layer lease.
- `stream check --input-event <action,args>` is a read-only preflight surface for reviewing input-relay readiness and must not start capture, MaaTouch, listeners, or device I/O.
- Trusted remote access remains reserved until authenticated and encrypted transport is implemented; current contract work may expose plans and checks but must not start a network listener, issue tokens, or start TLS.
- Live validation for self-heal, interactive stream input relay, and trusted encrypted channel remains deferred with `requires-live-device`.
- `session phase-c-plan` exposes `acceptance_gates` so UI/scheduler clients can distinguish offline gates from live gates before they interpret readiness, stream, self-heal, or trusted-channel status.
- The acceptance gate contract keeps all live gates tied to `requires-live-device`; offline checks may prove contracts and preflights, but they must not mark live validation as accepted.
- `session status --diagnostics`, `session bootstrap`, and `session readiness` mirror compact Phase C acceptance-gate fields so normal health/readiness clients can see self-heal, interaction-flow, trusted-channel, and live-acceptance blockers without executing recovery, stream, listener, token, TLS, capture, MaaTouch, app, resource, SQLite, UI, or live-device work.
- `session connect-plan` mirrors the same Phase C gate state into `phase_c_preflight`, so future UI/scheduler clients can make a single client-start decision while still seeing self-heal, interaction-flow, trusted-channel, and live-validation blockers.
- `session submit-plan` now mirrors Phase C execution-readiness into `phase_c_execution_preflight`, so UI/scheduler clients can inspect self-heal, interaction-flow, trusted-channel, lease, queue, and live-deferred gates before submitting a command.
- `session queue` and `session events --data-summary-kind queue` expose the queue/admission slice used by Phase C preflights, so UI/scheduler clients can see blocked work before they propose recovery, stream input, or future trusted remote control.
- `session self-heal-policy` and `session events --data-summary-kind self_heal_policy` expose the maintenance-only self-heal policy slice used before any recovery plan or execution request is accepted.
- `session throat-policy` and `session events --data-summary-kind throat_policy` expose the unique Session Layer throat slice that every self-heal, interaction-flow, and trusted-channel client must respect.
- `session capture-policy` and `session events --data-summary-kind capture_policy` expose the fresh-frame and AK stale-screencap classification guard before clients interpret frozen-looking frames as game-freeze evidence.
- `session record-policy` and `session events --data-summary-kind record_policy` expose the active-recording authorization slice before UI/agents ask operators to authorize anchor, operation, color-probe, verify-template, build, or promote steps.

## Current ActingLab record policy event summary

This increment makes the active recording authorization policy discoverable from daemon request journals and event filters.

`session request record-policy` summaries now expose:

- active authorization requirement and passive full-recording prohibition;
- allowed recording step kind count;
- current-frame live-validation deferral;
- explicit promotion command and explicit resource repository requirement;
- policy command resource-write and promote guarantees;
- destructive-operation, game-progress, and blind-confirmation safety fields;
- live-validation deferred code;
- no-enqueue/no-device/no-capture/no-MaaTouch/no-resource-read/no-resource-write/no-listener guarantees.

`session events --data-summary-kind record_policy` can now return this recording safety slice without forcing UI/scheduler clients to parse the full policy payload.

Phase D alignment:

- Recording stays active and operator-authorized; passive full recording remains forbidden.
- Navigation and normal operation do not become resources unless an explicit record step is submitted.
- Resource promotion remains an explicit command with an explicit repository boundary.
- Current-frame capture and live recording acceptance remain `deferred` with `requires-live-device`; offline summaries must not mark live validation accepted.

This is still a contract/discovery increment only:

- no recording session is created;
- no daemon work is enqueued;
- no frame is captured;
- no draft or resource repository is written;
- no listener, token, TLS, capture, MaaTouch, ADB, app lifecycle, SQLite, UI, OCR, or game logic work is performed;
- live validation remains `deferred: requires-live-device`.

## Current ActingLab capture policy event summary

This increment makes the fresh-frame and stale-capture policy discoverable from daemon request journals and event filters.

`session request capture-policy` summaries now expose:

- `--require-fresh` guidance for callers that need live frame evidence;
- preferred backend count and the fact that `adb_screencap` is the last resort;
- full fallback logging requirement for transient capture-backend failures;
- stale-capture status and the AK stale-screencap finding reference;
- the freeze-classification gate and `safe_to_classify_game_frozen=false` when only stale `adb_screencap` evidence exists;
- lighter capture-backend recovery guidance before heavy app restart;
- deferred live-validation code;
- no-device/no-capture/no-MaaTouch/no-resource-read guarantees.

`session events --data-summary-kind capture_policy` can now return this capture safety slice without forcing UI/scheduler clients to parse the full policy payload.

Phase C alignment:

- Self-heal: capture policy events keep stale-frame diagnosis ahead of recovery execution and app restart.
- Interaction flow: capture policy events let stream/UI clients show degraded capture state before relaying input based on stale frames.
- Trusted channel: future remote clients must see the same capture reliability blocker through Session Layer events.
- Live validation: live-only capture recovery acceptance remains `deferred` with `requires-live-device`; offline summaries must not mark live validation accepted.

This is still a contract/discovery increment only:

- no daemon work is enqueued;
- no recovery is executed;
- no stream is opened;
- no listener, token, TLS, capture, MaaTouch, ADB, app lifecycle, resource repository, SQLite, UI, OCR, or game logic work is performed;
- live validation remains `deferred: requires-live-device`.

## Current ActingLab throat policy event summary

This increment makes the unique Session Layer control-throat policy discoverable from daemon request journals and event filters.

`session request throat-policy` summaries now expose:

- resident daemon and only-control-throat flags;
- client/UI/scheduler/agent no-direct-ADB-or-device rules;
- strict Session Layer flag, environment variable, visible failure code, and failure visibility;
- local read-only routing policy and explicit `--local` override;
- control request routing through the resident daemon and matching lease requirement;
- daemon-internal local execution guard to avoid recursive request requeue;
- trusted remote reserved state, encryption/authentication requirements, and blocking codes;
- lease matching field count and live-validation deferral;
- severe-error/no-silent-failure policy;
- no-enqueue/no-device/no-capture/no-MaaTouch/no-listener/no-app/no-resource-read guarantees.

`session events --data-summary-kind throat_policy` can now return this unique-throat policy slice without forcing UI/scheduler clients to parse the full policy payload.

Phase C alignment:

- Self-heal: throat policy events make recovery execution depend on Session Layer routing and matching lease instead of direct device access.
- Interaction flow: throat policy events make stream/input-relay clients see the same no-bypass rule before they submit input.
- Trusted channel: throat policy events expose reserved remote status and encryption/authentication blockers before any transport implementation exists.
- Live validation: live-only gates remain `deferred` with `requires-live-device`; offline policy summaries must not mark live validation accepted.

This is still a contract/discovery increment only:

- no daemon work is enqueued;
- no recovery is executed;
- no stream is opened;
- no listener, token, TLS, capture, MaaTouch, ADB, app lifecycle, resource repository, SQLite, UI, OCR, or game logic work is performed;
- live validation remains `deferred: requires-live-device`.

## Current ActingLab self-heal policy event summary

This increment makes the Phase C self-heal policy discoverable from daemon request journals and event filters.

`session request self-heal-policy` summaries now expose:

- self-heal target state and live-validation deferral;
- observe/diagnose/plan/execute flow stage count;
- supported trigger count and the stale-ADB-screencap classification guard;
- recovery order count, first recovery kind, and heavy recovery kind;
- maintenance boundary fields that forbid game progress, destructive actions, and paid-resource use;
- scheduler/lease policy requiring matching lease control execution;
- UI and interactive-stream guidance that forbids bypassing the Session Layer;
- no-enqueue/no-device/no-capture/no-MaaTouch/no-listener/no-app/no-resource-read guarantees.

`session events --data-summary-kind self_heal_policy` can now return this self-heal policy slice without forcing UI/scheduler clients to parse the full policy payload.

Phase C alignment:

- Self-heal: policy events expose observe-first and maintenance-only recovery boundaries before any recovery plan is submitted.
- Interaction flow: policy events expose that streams may report recovery state but must not execute recovery or input relay without a matching lease.
- Trusted channel: policy events keep UI/remote clients behind the Session Layer boundary and do not start listeners, issue tokens, or start TLS.
- Live validation: live-only gates remain `deferred` with `requires-live-device`; offline policy summaries must not mark live validation accepted.

This is still a contract/discovery increment only:

- no daemon work is enqueued;
- no recovery is executed;
- no stream is opened;
- no listener, token, TLS, capture, MaaTouch, ADB, app lifecycle, resource repository, SQLite, UI, OCR, or game logic work is performed;
- live validation remains `deferred: requires-live-device`.

## Current ActingLab queue event summary

This increment makes queue health and request admission discoverable from daemon request journals and event filters.

`session request queue` summaries now expose:

- queue status and pending/running/response counts;
- queue health status for pending requests, running requests, and pending responses;
- admission status, enqueue permission, and blocked code;
- recommended action count and action kinds;
- no-enqueue/no-device/no-capture/no-MaaTouch/no-listener guarantees.

`session events --data-summary-kind queue` can now return this scheduler queue slice without forcing UI/scheduler clients to parse the full queue payload.

Phase C alignment:

- Self-heal: queue events expose blocked queue state before any maintenance recovery is considered.
- Interaction flow: queue events expose request pressure and admission state before stream/input-relay work is submitted.
- Trusted channel: queue events keep remote-control admission visible through the Session Layer instead of letting a future transport bypass the request queue.
- Live validation: live-only gates remain `deferred` with `requires-live-device`; offline queue summaries must not mark live validation accepted.

This is still a contract/discovery increment only:

- no daemon work is enqueued;
- no recovery is executed;
- no stream is opened;
- no listener, token, TLS, capture, MaaTouch, ADB, app lifecycle, resource repository, SQLite, UI, OCR, or game logic work is performed;
- live validation remains `deferred: requires-live-device`.

## Current ActingLab bootstrap event summary

This increment makes the startup bootstrap envelope discoverable from daemon request journals and event filters.

`session request bootstrap` summaries now expose:

- readiness state and queue health;
- validation-plan status and pending live-acceptance count;
- Session throat and AK stale-capture classification guard state;
- status diagnostics for liveness, Phase C, self-heal, interaction flow, and trusted channel;
- trusted-channel reserved state, encryption/authentication requirements, and listener-not-implemented state;
- no-enqueue/no-device/no-capture/no-MaaTouch/no-listener/no-resource-read guarantees.

`session events --data-summary-kind bootstrap` can now return this startup slice without forcing UI/scheduler clients to parse the full bootstrap contract payload.

Phase C alignment:

- Self-heal: bootstrap events expose the first self-heal review action and capture-staleness guard without executing recovery.
- Interaction flow: bootstrap events expose stream/input-relay preflight status without opening streams or relaying input.
- Trusted channel: bootstrap events expose remote-channel requirements while preserving no listener/token/TLS startup.
- Live validation: live-only gates remain `deferred` with `requires-live-device`; offline bootstrap summaries must not mark live validation accepted.

This is still a contract/discovery increment only:

- no daemon work is enqueued;
- no recovery is executed;
- no stream is opened;
- no listener, token, TLS, capture, MaaTouch, ADB, app lifecycle, resource repository, SQLite, UI, OCR, or game logic work is performed;
- live validation remains `deferred: requires-live-device`.

## Current ActingLab readiness event summary

This increment makes startup readiness discoverable from daemon request journals and event filters.

`session request readiness` summaries now expose:

- daemon liveness and request-acceptance state;
- queue health and queue counts;
- selected-instance readiness;
- capture-staleness and AK stale-frame classification guard state;
- Phase C self-heal, interaction-flow, trusted-channel, and pending-live validation fields;
- no-device/no-capture/no-MaaTouch/no-listener/no-resource-read guarantees.

`session events --data-summary-kind readiness` can now return this startup slice without forcing UI/scheduler clients to parse the full readiness payload.

Phase C alignment:

- Self-heal: readiness events show the first recovery-planning action and deferred live gate without executing recovery.
- Interaction flow: readiness events expose bounded local CLI availability and matching-lease input-relay policy without starting streams or relaying input.
- Trusted channel: readiness events expose encryption/authentication requirements and listener-not-implemented state without starting a listener, issuing tokens, or starting TLS.
- Live validation: live-only gates remain `deferred` with `requires-live-device`; offline readiness summaries must not mark live validation accepted.

This is still a contract/discovery increment only:

- no daemon work is enqueued;
- no recovery is executed;
- no stream is opened;
- no listener, token, TLS, capture, MaaTouch, ADB, app lifecycle, resource repository, SQLite, UI, OCR, or game logic work is performed;
- live validation remains `deferred: requires-live-device`.

## Current ActingLab Submit-Plan Phase C execution preflight

This increment keeps the unique-throat and Phase C gates visible at the final command submission preflight boundary.

`session submit-plan <command...>` now exposes `phase_c_execution_preflight` with:

- `schema_version=session.submit_phase_c_execution_preflight.v0.1`;
- whether the target command is Phase C relevant;
- the relevant lanes: `self_heal`, `interaction_flow`, `trusted_channel`, and/or `live_acceptance`;
- command class, device-affecting status, and lease requirement;
- readiness, command safety, queue admission, throat status, direct-device-access prohibition, lease status, and queue status;
- lane-specific preflight commands for self-heal, interaction flow, and trusted channel;
- live validation status fixed to `deferred` with `requires-live-device`;
- guarantees that the preflight does not enqueue, touch devices, capture frames, start MaaTouch, start listeners, issue tokens, start TLS, read resource repositories, or mark live validation passed.

This is still a contract/discovery increment only:

- no daemon request is enqueued;
- no device, ADB, capture backend, MaaTouch backend, app lifecycle, resource repository, SQLite, OCR, UI, listener, token, or TLS work is performed;
- live validation remains `deferred: requires-live-device`.

## Repo-local planning policy

Runtime planning and checkpoint records live in this repository.

For Runtime tasks, update `PLANS.md` and `CHECKPOINT.md` here and commit them with the Runtime source changes. Do not mirror Runtime task planning files into the umbrella repository by default.

Routine Runtime updates must stay in `HS7097/ActingCommand-Runtime`. Do not merge, copy, or synchronize Runtime changes into the umbrella/main `HS7097/ActingCommand` repository unless the user explicitly confirms that a specific Runtime state is ready for that merge.

## Active boundaries

- Touch input fallback is limited to the P6.5-A1 chain: `MaaTouch -> adb shell input`.
- `adb shell input` is available only for `tap`, `long_tap`, and `swipe`; `key`, `text`, Minitouch, reconnect, retry loops, and arbitrary fallback remain out of scope.
- MaaTouch transient transport failure is recorded as structured diagnostics and may fall back to `adb shell input`; MaaTouch fatal validation/configuration failure does not fall back; full-chain input failure is fatal.
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
