# C5 Production Capability Relocation Plan

Status: active.

## Authority and baseline

- GitHub authority: Issue #35, authored by `HS7097` and labeled `状态:已批准`.
- Execution tracker: Issue #36. No additional subtask Issue is created.
- Frozen specification: `TASK-runtime-ledger-core-and-optional-lab-correction-v3.md`.
- Frozen specification SHA-256:
  `28273b85491b0d43aa7a7b7a7ece10db681de9df4d9100e85f9e9b086dd107a6`.
- Approved resource-authoring amendment:
  `AMENDMENT-Issue35-Lab-resource-authoring-ownership.md`.
- Amendment SHA-256:
  `de039ad910b0b8208d52b8582b260868d4bcd2b04ec2b272977890661a136b54`.
- C5 baseline: `6dc0b6c`.
- C3b rollback tag: `checkpoint/20260711-c3b-resident-control-plane`.
- Implementation branch: `issue-35-runtime-ledger-v3`.

## Goal

Move accepted production behavior out of temporary Lab ownership and into the production modules
frozen by C0. Preserve existing command protocol, resource semantics, state-machine behavior, and
failure visibility while making every mutable state and side effect have one owner.

C5 relocates ownership. It does not redesign game behavior. Compatibility adapters may remain in
Lab while callers migrate, but production packages must not depend directly or transitively on Lab.

## Frozen ownership

### Runtime host and scheduler

- Runtime host owns live Session, monitor, and stream lifecycle plus typed IPC composition.
- Scheduler remains the only lease/admission/queue/preemption authority.
- Runtime host records typed ledger facts before projecting receipts.
- Neither module owns resource compilation, CLI parsing, recognition algorithms, or device backend
  implementations.

### Execution kernel and domain behavior

- Execution kernel owns live environment detection, recognition orchestration, input plans, task
  execution, and recovery state after scheduler admission.
- Existing recognition, page-detector, recognition-pack, device, and containment crates remain deep
  domain dependencies rather than being copied into the kernel.
- Production operations consume immutable `LoadedBundle` capabilities. Loose resource-root reads are
  not a production execution path.
- Execution code cannot schedule or invoke another task. It may return typed successor suggestions.

### Artifact and evidence

- Artifact store owns frame bytes, frame-store pressure behavior, naming, retention, and evidence
  export.
- Ledger owns durable event truth and projections. Output or diagnostics code cannot create a second
  state owner or successful receipt.
- The existing Tier1/Tier2/Tier3 thresholds, hysteresis, pinned-frame behavior, and tests move without
  semantic drift.

### Resource tooling and Lab

- `actingcommand-resource-tooling` is a Lab-owned developer-only compiler/validator module.
- It owns deterministic package build/validate, conversion, MAA compilation, typed draft
  materialization, and file planning.
- It receives a typed resolved environment snapshot; it does not create Lab, inspect live state,
  acquire leases, open devices, or call Runtime host/scheduler.
- Production packages cannot depend directly or transitively on resource-tooling.
- Lab remains the optional authoring/debug/sealed-test client. Online device work uses
  runtime-client; offline work uses explicit fake/replay/isolated adapters.

## Issue #26 G2 and G3

- G2 external expected hash remains a consumption-side request/containment rule. A self-computed hash
  cannot be reported as externally verified.
- G3 semantic operations must consume resources through `LoadedBundle`; default production writes
  cannot read unverified loose resources.
- Resource-tooling validates its output through the same containment boundary but does not own these
  production consumption rules.

## Implementation tasks

### Task 1: Freeze ownership inventory and guards

Status: complete.

- Record current command/API/module ownership, package dependencies, mutable state, file/network/
  device side effects, and replacement destination.
- Freeze the 30-case A1 protocol matrix, command inventory, package/convert/MAA tests, Session
  closeout tests, and C3b process gates as migration evidence.
- Add architecture test scaffolding for the resource-tooling production exclusion and C5 migrated
  capability boundary.
- Do not move behavior until the inventory names its replacement owner and equivalence test.

### Task 2: Extract resource-tooling

Status: complete.

Progress:

- Subtask 2a complete: the developer-only crate boundary now owns package API DTOs, package
  validation, resource conversion, and MAA compilation; Lab exposes compatibility wrappers only.
- Subtask 2b complete: resource-tooling owns package build behind a two-phase prepare/build API;
  Lab resolves required environment facts once and supplies an immutable typed snapshot, so the
  compiler cannot inspect Lab, device, config, scheduler, or Runtime state.

- Add `crates/resource-tooling` as a workspace crate.
- Mechanically move package API/build/validate, resource conversion, and MAA compilation behavior and
  tests out of `actingcommand-lab`.
- Replace `Lab<P>` compiler wrappers with small typed functions and a resolved environment snapshot
  based on contract `EnvResolved` facts.
- Keep ActingLab command envelopes and golden output unchanged through thin compatibility adapters.
- Add dependency guards proving resource-tooling cannot reach Lab, Runtime host, scheduler,
  execution-kernel, or device backends and production packages cannot reach resource-tooling.

### Task 3: Absorb task-loop decision behavior

Status: complete.

Progress:

- Subtask 3a complete: mechanically moved the pure planning implementation and tests into
  execution-kernel, migrate device-test to the new owner, and retain a temporary re-export-only
  task-loop shell for equivalence verification.
- Subtask 3b complete: focused planning/device-test tests, protocol goldens, architecture guards,
  JSON behavior, Clippy, and the full workspace passed before and after removing the unreferenced
  compatibility crate. Cargo metadata and lock state no longer contain task-loop.

- Move production-worthy `TaskPlan` and `ProbePlan` decision behavior into execution-kernel domain
  modules without adding device side effects to pure planning APIs.
- Migrate device-test and sealed tests to the new owner.
- Retire `actingcommand-task-loop` only after API equivalence, JSON compatibility, and reverse-
  dependency checks pass.

### Task 4: Move environment and read-only recognition

Status: active.

- Subtask 4a complete: `ReadonlyRecognitionEngine` and the existing response models now live in
  execution-kernel. The engine owns pure target recognition, visibility, page evaluation, page
  detection, and detection-hint decisions over caller-supplied evaluators, detectors, scenes, and
  resolved environment facts.
- Lab retains only the temporary compatibility adapter that reads paths, resolves environment
  markers, prepares an offline scene or capture, and maps typed execution errors into the existing
  Lab protocol. It no longer evaluates targets or pages itself.
- Architecture guards allow Lab to consume execution-kernel only as a removable client while
  keeping all production packages Lab-free. The read-only module rejects filesystem, capture
  factory, input factory, Runtime client, and Lab ownership tokens.
- Subtask 4b1 complete: `EnvironmentStateEngine` now owns typed result-schema/scope/detector/hash
  freshness checks, confidence/expiry/allowed-value validation, safe value checks, marker-key
  collection, single-key resolution, and recursive JSON marker replacement. Environment result
  DTOs moved with that owner. Lab maps typed errors into its existing protocol and retains only
  catalog/resource I/O, local instance identity, locking, atomic persistence, and effect adapters.
- Subtask 4b2a complete: the execution kernel now owns structured/flat catalog parsing and
  normalization, detector/key/step/candidate data models, schema and threshold validation, safe
  candidate checks, ROI decoding, detector selection, game/server canonicalization, scope checks,
  and device-free step plans. Lab retains only catalog file reads and maps step plans to its
  temporary touch adapter.
- Subtask 4b2b complete: Lab emits only typed candidate index/confidence observations from explicit
  template or scene-size adapters. `EnvironmentDetectionEngine` validates observation completeness,
  uniqueness, and score range; applies candidate/key thresholds; selects the best candidate; and
  constructs value/source/TTL plus the final `EnvDetectionResult` without device or filesystem
  access.
- Task 4b is complete. Subtask 4c is the next work item and remains paused: replace production Lab
  capture construction with daemon-owned observation requests while preserving sealed/offline
  scene adapters.
- Subtask 4c pending: replace production Lab capture construction with a daemon-owned observation
  request without weakening sealed/offline scene adapters.

- Move environment detection/resolution and read-only recognition/page evaluation from Lab into
  execution ownership with typed ports and results.
- Replace direct Lab capture factories in production paths with daemon-owned capture requests.
- Keep sealed/offline adapters explicit and unable to discover production devices.
- Preserve env freshness, marker resolution, recognition scoring, page detection, and failure
  semantics through focused equivalence and protocol tests.

### Task 5: Move drive, run, and recovery

Status: pending.

- Move target tapping, navigation, operation execution, run orchestration, and recovery state
  machines into execution ownership.
- Require scheduler fencing before every state-changing input and preserve intent-before-act plus
  outcome-after-act ledger ordering.
- Enforce `LoadedBundle` as the production resource capability and close G2/G3 with adversarial tests.
- Return typed successor suggestions instead of directly invoking another task.

### Task 6: Remove duplicate frame/output ownership

Status: pending.

- Remove the Lab frame-store ownership wrapper after consumers use artifact-store interfaces.
- Route run frames, diagnostics, output manifests, and evidence completeness through artifact-store
  and ledger projection.
- Preserve pressure thresholds, hysteresis, pinned frames, four-way screenshot counts, collision-
  safe naming, and exporter failure rules.

### Task 7: Move Session, monitor, and stream lifecycle

Status: pending.

- Move live Session registry, monitor loop, bounded stream state, and recovery coordination from the
  ActingLab process into Runtime host/control-plane modules.
- Expose typed Runtime operations/events/receipts; keep actingctl and ActingLab as disposable clients.
- Remove legacy file-queue/state ownership only after process-level equivalence and restart/reconnect
  evidence pass.
- Keep trusted remote long-lived streaming reserved; C5 adds no public remote API.

### Task 8: Reduce Lab to optional adapters

Status: pending.

- Keep debug request parsing, resource-authoring workflow, record/replay, sealed composition,
  projections, and evidence requests in Lab.
- Remove production scheduler, device, ledger-writer, execution, Session, monitor, and stream state
  ownership from Lab and ActingLab.
- Prove production build/tests remain runnable when Lab and resource-tooling are excluded.

### Task 9: Closeout

Status: pending.

- Run focused equivalence and adversarial tests after each migration batch.
- Run protocol goldens and command inventory after every client-visible batch.
- Run full workspace, non-Lab production, all-target/all-feature checks, Clippy with warnings denied,
  formatting, architecture/dependency/source guards, and `git diff --check`.
- Create a stable C5 rollback tag, update planning/checkpoint files, and record evidence in Issue #36.
- Do not merge this branch into `main` or the umbrella repository.

## C6-deferred work

- Full Lab authoring UX/API and candidate-review workflow.
- Transactional resource publication, rollback/recovery, and authoring ledger receipt completion.
- End-to-end record -> draft -> convert -> package -> containment -> sealed-run product workflow.
- Optional Lab online subscription/progress views and generalized evidence-export requests.

C5 may introduce the typed seams needed by that work but must not claim C6 acceptance early.

## Explicit non-goals

- No new game-specific logic or resource semantics.
- No UI visual work, SQLite, OCR implementation, or public remote API.
- No automatic task-to-task invocation.
- No fallback, reconnect, retry, fake success, or silent ownership bypass.
- No resource-repository or emulator operation unless a later acceptance step explicitly needs it. If
  resources become necessary, mirror the authoritative resource repository before use.
- No main-branch merge or cooperation-workspace write.

## Acceptance criteria

1. Every migrated production capability has one documented owner and no dependency path to Lab.
2. Runtime/scheduler/execution/artifact responsibilities remain separate and typed.
3. Production execution consumes only containment-issued immutable bundles.
4. Resource-tooling is unreachable from production packages and cannot open live devices or own
   Runtime state.
5. Existing package/convert/MAA, env/recognition/drive/run, Session/monitor/stream, frame-store, and
   output semantics retain equivalence evidence.
6. A1 protocol goldens and command inventory remain unchanged unless Issue #35 is explicitly amended.
7. Severe migration or backend failures remain visible; no path returns empty/fake success.
8. Excluding Lab and resource-tooling leaves production build and tests runnable.
9. Full workspace and all architecture/process gates pass before C5 is tagged complete.
