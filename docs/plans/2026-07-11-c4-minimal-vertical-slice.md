# C4 Minimal Vertical Slice Implementation Plan

Status: active.

## Authority and baseline

- GitHub authority: Issue #35, authored by `HS7097` and labeled `状态:已批准`.
- Frozen specification: `TASK-runtime-ledger-core-and-optional-lab-correction-v3.md`.
- Frozen specification SHA-256:
  `28273b85491b0b43aa7a7b7a7ece10db681de9df4d9100e85f9e9b086dd107a6`.
- Approved C0 architecture:
  `docs/architecture/runtime-ledger-v3-c0-freeze.md`.
- C4 baseline: `61e9868b03e74eb40f47ad7958b43d405f645ab6`.
- Implementation branch: `issue-35-runtime-ledger-v3`.

## Goal

Complete the seed phase with one read-only observation request and one safe control request that
use the same typed Runtime request/receipt contract from both the production CLI and ActingLab.
The control path must pass through scheduler admission, a daemon-owned write lease, DeviceProxy,
and the input backend. The read path must use a capability that cannot recover input authority,
then capture and recognize outside the daemon under the C3a transition exception. User-visible
success must be backed by durable ledger facts and a Runtime ledger projection.

## Frozen slice choices

### Read path: frame observation

- Command meaning: admit one read-only observation for a target instance, capture one frame, and
  decode it through the recognition layer without game-specific resources.
- The daemon issues an opaque read-only capability bound to the current owner epoch, instance,
  connection, frame, recognition, and correlation context.
- The capture backend remains client-side for C4. The shared Runtime client accepts an injected
  `CaptureBackend`; it does not construct a device backend.
- Recognition validates and decodes the captured PNG and records only schema-owned frame
  dimensions and a closed recognition verdict. No image bytes enter the Runtime request or
  ledger.
- Begin, complete, and failed observation states are typed and durable. A capture or recognition
  failure is reported to Runtime and remains a visible failed terminal projection.

### Safe control path: input reset

- The safe control operation is `input.reset`. It carries no coordinate, text, key, purchase,
  resource-spending, navigation, or game-specific intent.
- One high-level Runtime request performs admission, lease grant, daemon DeviceProxy reset, and
  lease release under one correlation.
- DeviceProxy validates fencing immediately before backend use. The client never receives or
  constructs a writable backend.
- Success is returned only after the input outcome and lease release terminal facts are durable.

### Client surfaces

- Add `apps/actingctl` as the first thin production CLI surface. It contains argument parsing,
  capture adapter construction for the C3a read-only exception, output formatting, and no
  scheduler or device-write logic.
- Add a narrow ActingLab Runtime-slice adapter in its own module. It delegates to the same
  runtime-client flow and does not duplicate the protocol or create an input backend.
- Both surfaces serialize the same Runtime receipt and correlated ledger projection shape.
- ActingLab remains optional; excluding `actingcommand-lab` and `actingcommand-actinglab` must
  leave `actingctl`, Runtime, scheduler, daemon, and non-Lab tests buildable.

## Contract and ledger changes

- Extend `RuntimeOperation` with typed begin/finish read observation and safe reset operations.
- Extend `ReadOnlyCaptureCapability` so only Runtime-issued capabilities can be promoted to
  frame/recognition event links after host-side registry validation.
- Add closed capture and recognition event families for requested, completed, and failed states.
- Add a typed observation result containing non-zero width, non-zero height, and a closed verdict.
- Preserve one correlation across every operation belonging to one user command.
- Return the final host-issued `RuntimeReceipt` together with events obtained through
  `RuntimeOperation::QueryEvents`; clients do not synthesize successful terminal state.
- Keep critical ordering for the reset action:
  `input intent durable -> DeviceProxy reset -> input outcome durable -> lease release durable`.

## Implementation tasks

### Task 1: Contract and event schema

- Add RED tests for strict serde, capability issuance, observation validation, new event families,
  public/full projections, and unknown-field rejection.
- Implement the minimum typed DTO and event additions.
- Keep every runtime-controlled code closed; do not add generic JSON payloads.

### Task 2: Runtime host

- Add connection-scoped pending read capability ownership and cleanup.
- Implement begin/finish observation events and terminal receipts.
- Implement high-level safe reset by composing existing scheduler and DeviceProxy internals.
- Reject forged, stale-epoch, wrong-instance, wrong-connection, reused, and mismatched-correlation
  observation capabilities before durable completion.
- Ensure partial ledger failure never produces a successful receipt.

### Task 3: Runtime client

- Add a correlation-scoped flow helper without holding the IPC mutex during capture/recognition.
- Implement read observation with an injected capture backend and recognition callback.
- Implement safe reset without exposing the lease token or writable backend to callers.
- Query the correlated Runtime projection after the terminal receipt is durable.
- On local observation failure, report the typed failure to Runtime; if reporting also fails,
  return a combined fatal error rather than hiding either failure.

### Task 4: Thin clients

- Add `actingctl observe` and `actingctl reset` with explicit state-root and instance arguments.
- Support a real read-only capture configuration and an explicit sealed-frame test mode; sealed
  input is never accepted as production success without its sealed marker.
- Add ActingLab `runtime observe` and `runtime reset` routing in a separate module.
- Preserve existing ActingLab command behavior and protocol goldens outside the new commands.

### Task 5: Acceptance and guards

- Run a real child Runtime process with a sealed file-backed input provider.
- Run `actingctl` and ActingLab as disposable client processes against that Runtime.
- Prove both surfaces return the same receipt/projection contract.
- Prove reset produces exactly one daemon-owned backend reset and no client-side write backend.
- Prove observation uses a read-only capability, records capture/recognition terminal facts, and
  cannot be converted into input authority.
- Cover forged/stale/reused capability denial, local capture/recognition failure reporting,
  ledger failure before action, process disconnect cleanup, and Runtime survival after clients
  exit.
- Extend architecture guards so production clients cannot construct writable input backends and
  non-Lab packages remain Lab-free under all features.

### Task 6: Closeout

- Run focused contract, ledger, host, client, `actingctl`, ActingLab, process, and architecture
  suites.
- Run full workspace tests, non-Lab workspace tests, Clippy with warnings denied, formatting,
  all-features dependency checks, source guards, and `git diff --check`.
- Perform a fresh C4 review and fix every Critical or Important finding.
- Update `PLANS.md` and `CHECKPOINT.md`, push each completed unit, create a stable C4 checkpoint
  tag, and record evidence in Issue #36.
- Do not merge this branch into `main` or the umbrella repository.

## Explicit non-goals

- No queue, priority, preemption, scheduled execution, or task lifecycle.
- No daemon-owned capture or full execution kernel; those remain C3b/C5 work.
- No game logic, resource repository read, OCR, SQLite, UI, remote API, or live-device action.
- No retry, reconnect, automatic fallback, fake success, or new production framework.
- No C2 artifact storage, image persistence, retention, evidence ZIP, or pinned-frame behavior.
- No claim that C4 completes the terminal device throat beyond the C3a write path.

## Acceptance criteria

1. `actingctl` and ActingLab use the same typed Runtime flow and output shape.
2. Read observation follows IPC admission -> read-only capability -> capture -> recognition ->
   durable terminal projection.
3. Safe reset follows IPC -> scheduler/lease -> daemon DeviceProxy -> input backend, with intent
   durable before reset and outcome durable afterward.
4. The read capability has no input method, writable getter, downcast path, or public constructor
   that can mint Runtime authority.
5. Output terminal identity and correlated events come from Runtime receipts and ledger
   projection, not client-generated success fields.
6. Process-level and sealed tests pass while Runtime survives disposal of both client surfaces.
7. Excluding Lab packages leaves the production CLI, Runtime, scheduler, daemon, and non-Lab
   tests buildable and runnable.
8. Full workspace tests, Clippy, formatting, dependency guards, source guards, and diff checks
   pass.
9. C4 is documented only as the seed-phase acceptance anchor; C3b remains required for the
   terminal capture/execution throat.
