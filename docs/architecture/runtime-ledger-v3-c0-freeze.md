# Runtime Ledger V3 C0 Architecture Freeze

- Status: C0 approval candidate for GitHub Issue #35
- Source specification: `TASK-runtime-ledger-core-and-optional-lab-correction-v3.md`
- Source specification SHA-256: `28273b85491b0d43aa7a7b7a7ece10db681de9df4d9100e85f9e9b086dd107a6`
- Implementation baseline: `981f61f650c51a62f3c6c22fda781d2b98b3ceb8`
- Paused RED evidence: `ead23d2acb3752507b5c6110c1cbf049e878cbbd`
Frozen payload SHA-256: `6c72a9c39ff67ec5a2868e0ed262d2a2f0a2c4b0fbfc473b7c55a9df610bf0a7`

This document is the C0 design candidate. It becomes the binding C1-C7 architecture only after Alice explicitly approves its frozen hash on Issue #35.

<!-- RUNTIME-LEDGER-V3-C0-FREEZE-BEGIN -->
## 1. Supersession Decision

Issue #35 replaces the Issue #33/#34 goal that made `crates/lab` the application core.

The accepted A7 commit remains the implementation baseline because it contains reviewed contracts, tests, device seams, recognition behavior, packaging behavior, frame-store behavior, and protocol goldens that must be preserved while ownership is corrected. Its file placement is not accepted as the terminal ownership model.

The paused A8a RED commit remains on `issue-34-lab-extraction-chain` as historical evidence. It is not cherry-picked into this branch. Its requirements are rewritten as daemon single-owner, global-ledger single-writer, fencing, contention, and crash-recovery tests in C1 and C3a. The pending lock-conflict golden is not added to the 30-case A1 protocol baseline.

The following earlier statements are superseded:

- Lab is the sole application core.
- A long-lived production process owns a long-lived `Lab` object.
- Production state and device factories belong to `LabState` or `LabPorts`.
- Every remaining CLI use case should move into Lab.
- Per-command cross-process state-file locking is the terminal arbitration model.
- `main.rs <= 6000` and the old S3 pipeline percentage are terminal architecture gates.

The following earlier assets remain binding unless a later approved node changes them:

- A0 source-derived command inventory and dependency tooling;
- A1 static protocol goldens and output-channel assertions;
- typed contract DTOs and semantic error classification;
- CLI process exit mapping staying outside domain modules;
- existing device, recognition, containment, page-detection, and resource behavior;
- fail-loud storage/device rules;
- the tested frame-store tier semantics;
- the accepted A7 behavior baseline and all behavior-preservation tests.

Frozen documents from Issue #33/#34 remain immutable audit records. This supersession document has higher architectural precedence without rewriting their frozen payloads.

## 2. Selected Architecture

Three approaches were considered:

1. Continue moving production behavior into Lab. Rejected because deleting Lab would delete production capability and state ownership.
2. Move all production behavior directly into the `actingd` binary. Rejected because it would replace the CLI monolith with a daemon monolith and make testing depend on process internals.
3. Keep a thin daemon binary over deep production modules, with typed client and event seams and an optional Lab client. Selected because ownership follows process lifetime while behavior remains testable without the process shell.

The selected dependency direction is:

```text
UI / Agent / actingctl / actinglab
              |
              v
   actingcommand-runtime-client
              |
              v  local typed IPC
          apps/actingd
              |
              v
  actingcommand-runtime-host
      |        |         |
      v        v         v
 scheduler   ledger   execution-kernel
      |        |         |
      +--------+---------+
               |
               v
        device + domain crates

all production modules -> typed event drafts -> redaction -> ledger ingress
capture/execution -> artifact-store -> artifact references -> ledger
resource-tooling is independent of the live control plane and uses ledger ingress when recording work
actinglab depends inward through runtime-client/query contracts; no production package depends on Lab
```

The process owns lifecycle; modules own behavior. `apps/actingd` contains startup, shutdown, argument/config loading, and process exit mapping only. Production behavior lives behind the `runtime-host`, scheduler, ledger, execution, artifact, and device interfaces.

## 3. Module Ownership

| Module | Decision | Owned responsibility | Explicitly not owned |
| --- | --- | --- | --- |
| `apps/actingd` | New in C3a | Thin process entry, state-root selection, host construction, shutdown and exit mapping | Scheduling policy, ledger storage logic, device operations, task behavior |
| `crates/runtime-host` | New in C3a | Single daemon lifecycle, local IPC sessions, owner guard, `owner_epoch`, backend-guard registry, DeviceProxy server, composition | CLI parsing, game logic, ledger format, scheduler decisions |
| `crates/runtime-client` | New in C3a | Typed local IPC client shared by Lab, user CLI and process tests | Scheduling, device backend access, local production state mutation |
| `crates/scheduler` | New in C3a | Per-instance admission and lease state, fencing decisions, renew/release idempotency, later queue/preemption/task lifecycle | Device implementation, ledger persistence, IPC transport |
| `crates/execution-kernel` | New in C5 | Production environment detection, recognition orchestration, input plans, task execution and recovery steps | Global scheduling, CLI parsing, resource compilation |
| `crates/actingcommand-contract` | Retain and extend | Shared wire/domain DTOs for events, requests, receipts, leases, DeviceProxy and artifacts | IO, mutable ownership, transport, persistence |
| `crates/ledger` | Retain and deepen in C1 | Global append/query/subscribe/project implementation, sequence allocation, storage recovery and retention facts | Scheduling decisions, device actions, UI formatting |
| `crates/device` | Retain | Device and capture backend implementations and diagnostics | Production admission or lease authority |
| `crates/artifact-store` | New in C2 | Artifact files, hashes, metadata, retention classes, evidence export | Runtime scheduling, event truth, task outcome |
| `crates/resource-tooling` | New in C5 | Package build/validate, resource conversion and MAA compilation | Live device, scheduler or Runtime ownership |
| `crates/lab` | Temporary holder, then optional | After C5/C6: sealed test composition, record/replay and debug-client helpers only | Production state, scheduler, ledger writer, device factory, execution kernel |
| `apps/actinglab` | Retain as optional Lab CLI | Debug request parsing, runtime-client calls, event projection and evidence export requests | Direct live scheduling, production state writes, production input backend |
| `apps/actingctl` | Future thin client | User-facing production request/status commands | Production logic or state |

No new production behavior is added to `crates/lab`. Until C5 moves accepted code to its true owner, that code is treated as migration inventory, not proof that Lab owns it.

## 4. Existing Asset Disposition

| Current asset | C0 decision | Destination or retirement gate |
| --- | --- | --- |
| `crates/actingcommand-contract` | Reuse | Add `event`, `runtime`, `scheduler`, `device_proxy`, and `artifact` contract modules in C1/C3a/C2; keep legacy Lab types only for compatibility while consumers migrate |
| `crates/ledger::LabLedger` | Reuse storage mechanics, not authority name | C1 introduces the global ledger writer and typed event storage; `LabLedger` remains a compatibility reader/writer until all old call sites migrate |
| `crates/arbitrator::DegradedArbitrator` | Legacy Lab2 authority; not production scheduler | C3a creates `crates/scheduler`; proven pure tests or algorithms may move mechanically, but queue/preempt behavior is not enabled in the seed; retire legacy authority after clients migrate |
| `crates/runtime-core::actinglab` | Retire prototype | It is explicitly disconnected and has no consumer; do not promote it into C3a |
| `crates/runtime-core::capture_store` | Move | C2 moves behavior into `artifact-store`, adds collision-safe naming and v3 metadata, then retires `runtime-core` after reverse-dependency proof |
| `crates/task-loop` | Retain temporarily | `device-test` is the only consumer; C5 moves production-worthy decision behavior into `execution-kernel`, then retires or renames only after equivalence evidence |
| `crates/lab::frame_store` | Move without semantic drift | C2/C5 moves it into the artifact/capture pipeline while preserving tier thresholds, hysteresis and recovery tests |
| Lab env/readonly/drive/run modules | Move | C5 moves them to execution/domain ownership; Lab later invokes them through runtime-client or sealed adapters |
| Lab package/convert/MAA modules | Move | C5 moves them to resource-tooling |
| ActingLab Session/monitor/stream behavior | Move | C3a/C3b/C5 moves live ownership to runtime-host/scheduler/execution; old direct paths become compatibility/debug adapters and are then removed |
| A0 command inventory | Preserve | Continue tracking the existing debug CLI; it is no longer a production-architecture completion denominator |
| A1 30-case goldens | Preserve unchanged | Guard compatibility while ownership moves; new production IPC/event contracts receive separate C1-C4 tests |
| A8a RED tests | Rewrite, do not cherry-pick | Ledger live-owner tests become C1 writer tests; lock contention becomes C3a daemon-owner/fencing tests; no old state-file lock implementation |

At baseline `981f61f`, Cargo metadata reports these relevant reverse dependencies:

| Package | Current workspace consumers |
| --- | --- |
| `actingcommand-lab` | `actingcommand-actinglab` only |
| `actingcommand-runtime-core` | none |
| `actingcommand-task-loop` | `actingcommand-device-test` only |
| `actingcommand-arbitrator` | `actingcommand-actinglab` only |
| `actingcommand-ledger` | `actingcommand-actinglab`, `actingcommand-arbitrator`, `actingcommand-lab` |
| `actingcommand-contract` | `actingcommand-actinglab`, `actingcommand-lab`, `actingcommand-runtime-core` |

This snapshot is evidence for disposition, not permission to delete. Every later retirement still requires a fresh reverse-dependency check plus replacement/equivalence evidence.

## 5. Process and State Ownership

One Runtime state root has at most one live `actingd` owner.

`actingd` owns:

- the owner file and process-liveness proof;
- one fresh `owner_epoch` per successful daemon start or takeover;
- the only global-ledger writer;
- the live scheduler state and per-instance lease table;
- all production input backend guards;
- the DeviceProxy server;
- C3b onward, production capture and execution backends;
- Runtime configuration snapshots and their change events.

The scheduler owns admission facts and lease transitions. The ledger owns persisted event ordering. The execution kernel owns task execution state after admission. Artifact store owns artifact bytes and lifecycle metadata. These modules do not obtain a second process-level authority.

Clients own only connection-local request state and cached projections. They do not recover authority from files, public getters, backend factories, or stale tokens.

C3a permits one transition exception: an admitted client may receive a read-only capture capability. That capability has no input method, no downcast path to a writable backend, and no public getter exposing one. Every operation that can change game state is served by daemon DeviceProxy.

C3b removes the capture exception and makes the daemon the only production owner of both input and capture backends.

## 6. Contract Freeze

### 6.1 Runtime request and receipt

All live clients use one typed request envelope with these required facts:

- `schema_version`;
- `request_id` and `correlation_id`;
- optional `causation_id`;
- `actor` and `source`;
- `instance_id` when the request targets a device;
- typed operation payload and its payload schema;
- submitted wall-clock time for display;
- requested retention class when overridden.

The production receipt contains:

- the same request/correlation identity;
- machine-readable state (`admitted`, `denied`, `completed`, `failed`, or `cancelled`);
- the terminal ledger sequence and terminal event id when available;
- typed result or typed error projection;
- `task_outcome` and `evidence_completeness` as independent fields when evidence applies.

A receipt is a projection of persisted facts. A client or adapter cannot construct a successful production receipt before the required terminal event is durable.

### 6.2 Event contract

Every persisted event contains the fields required by v3 section 4.2:

- schema and payload schema;
- event id and ledger-assigned sequence;
- timestamp, event type and severity;
- event-level sensitivity summary;
- source, module and actor;
- correlation identifiers and optional instance/task/run/lease/frame/action/recognition identifiers;
- one typed payload;
- zero or more artifact references.

Free-form `message` is diagnostic only. Control facts and correlation keys must be typed fields.

Event production has two type states:

1. `EventDraft<P>` may contain sensitive typed fields and cannot be persisted or serialized by ledger storage.
2. `SanitizedEventDraft<P>` is produced by the payload's declared redaction schema and is the only type accepted by ledger ingress.

Every payload field declares one of `public`, `internal`, `sensitive`, or `secret`. Secret originals are removed before ingress; correlation uses an irreversible fingerprint where explicitly allowed. Machine paths and device endpoints are normalized/redacted before ingress. Redaction failure for a critical event is fatal and blocks the related action.

### 6.3 Ledger interface

Production modules see a small ledger interface:

- append one sanitized event and receive the persisted identity/sequence;
- query by sequence range and typed correlation filters;
- subscribe from a sequence cursor;
- request a named projection profile.

They do not open ledger files or allocate sequences.

The daemon owns one writer task/thread with a bounded ingress channel. Critical appends synchronously acknowledge only after the durable policy is satisfied. Noncritical events may use bounded asynchronous ingress, but overflow emits a visible health/error transition and repeated overflow escalates to fatal.

C1 uses append-only segmented JSONL so existing recovery mechanics can be reused without introducing SQLite. Each complete record is serialized before append. On startup:

- a truncated final record may be quarantined and followed by an explicit recovery event;
- corruption before the final tail is fatal and the ledger cannot claim a complete projection;
- sequence resumes after the last verified record;
- indexes are rebuilt or verified from persisted facts;
- subscriptions begin only after recovery completes.

### 6.4 Critical ordering

All irreversible or state-changing operations follow:

```text
typed intent -> redact -> durable append -> perform action -> typed outcome -> redact -> durable append -> receipt projection
```

If intent append fails, the action does not run. If the action runs and outcome append fails, the operation returns a fatal indeterminate-persistence error, the Runtime enters explicit recovery/health degradation, and no success receipt is emitted.

Lease grants, transfers, releases, expirations, task terminal transitions and input commits use the same rule at their defined durability point.

### 6.5 Scheduler and fencing contract

C3a scheduler state is partitioned by `instance_id`. One busy instance cannot block admission for another instance.

The seed supports:

- request admission;
- one write lease per instance;
- grant, renew, release and expiry;
- busy rejection with current-holder facts permitted by redaction policy;
- idempotent renew and release keyed by request id;
- no queue, priority, preemption, scheduled execution, or task lifecycle.

Every write lease token binds:

- `owner_epoch`;
- `lease_id`;
- `instance_id`;
- `holder_id`;
- daemon-monotonic expiry.

Every DeviceProxy write request carries the same fencing tuple. The daemon validates every field immediately before invoking the backend. A mismatch, expiry, disconnected guard, old epoch, wrong instance, or cooldown state is a typed denial and produces no input call.

Heartbeat bounds and takeover cooldown are configuration fields with a validated invariant:

`takeover_cooldown > maximum_client_heartbeat_interval`.

C3a freezes concrete defaults in code and tests; configuration that violates the relation is fatal at daemon startup.

### 6.6 IPC and backend lifetime

The local IPC transport carries length-delimited typed messages with explicit schema versions. Transport framing, request decoding, size limits and timeouts are Runtime-host responsibilities; domain modules never read sockets or named pipes.

Each writable DeviceProxy handle belongs to one IPC connection guard. Disconnect, protocol failure, explicit close or host shutdown drops the guard, closes its backend handle, revokes associated leases and appends the corresponding events. Cleanup is idempotent.

Daemon takeover always creates a new `owner_epoch`. Tokens from every earlier epoch are permanently invalid. Each affected instance enters cooldown before another write lease may be granted.

## 7. Artifact and Evidence Ownership

Artifact bytes never enter event JSON. The event contract stores typed artifact references with id, kind, run/frame/correlation ids, object key, MIME type, byte count, SHA-256, producer, retention class and redaction state.

C2 owns:

- `debug_full`, `adaptive`, and `light` retention classes;
- the 300 ms default observation cadence and policy-change event;
- frame-store Tier1/Tier2/Tier3 semantics and hysteresis;
- semantic/pinned-frame bypass of similarity and pressure dropping;
- collision-safe `yyyyMMddHHmmssfff[-NN].png` naming;
- evidence ZIP generation and manifest verification;
- separate task outcome and evidence completeness.

Missing ordinary pressure-dropped frames can produce `partial`. A missing pinned frame produces `failed` regardless of task outcome. Evidence export success means the archive structure is durably complete, not that evidence completeness is `complete`.

## 8. Task and Client Rules

A task emits events, results and successor suggestions. It cannot invoke or enqueue another task. Only scheduler decisions or a new request from a user or trusted client may start subsequent work.

Lab online mode submits typed debug requests through runtime-client, subscribes to the same ledger, and asks the shared exporter for evidence. Lab offline mode uses explicit fake, replay or isolated adapters and temporary state. Offline mode cannot discover or open production devices implicitly.

The future user CLI is distinct from ActingLab. Both share runtime-client and request/receipt contracts, but Lab may add debug-only projections and sealed-test commands.

## 9. Enforced Dependency Rules

The architecture guard treats `actingcommand-lab` and `actingcommand-actinglab` as the only optional Lab roots. Every other workspace package is rejected if any direct or transitive dependency path reaches `actingcommand-lab`.

Additional gates by phase:

- C1: contract does not depend on storage; only sanitized drafts reach ledger ingress; one writer owns sequence allocation.
- C3a: clients cannot depend on writable device adapters; DeviceProxy validates fencing on every input operation.
- C4: one read and one write path use the shared request, scheduler, ledger and receipt contracts.
- C3b: no production client can construct input or capture backends.
- C5: migrated production capability has no dependency on Lab.
- C7: excluding Lab packages leaves production build/tests runnable.

The A1 protocol goldens remain unchanged during ownership migration. New IPC/event protocol expectations are separate static fixtures and cannot be generated by the implementation during comparison.

## 10. C0 Branch and RED Disposition

Issue #35 implementation uses branch `issue-35-runtime-ledger-v3` from accepted A7 commit `981f61f650c51a62f3c6c22fda781d2b98b3ceb8`.

The old paused branch and `ead23d2` remain immutable provenance. No production lock code exists there. The RED requirements are re-authored test-first at their new owners:

- C1: competing writers, stale owner, hard-kill recovery, sequence continuity, malformed ownership metadata and redaction failure;
- C3a: zero-stagger same-instance admission, independent instances, old-epoch denial, wrong-lease/instance denial, disconnect cleanup and takeover cooldown;
- C4: intent-before-input and outcome-after-input process-level evidence.

The Issue #34 branch is not merged into this branch or `main`. No #33/#34 issue status or label changes are part of C0.

## 11. C0 Exit Gate

C0 is complete only when:

- this document and its payload hash are reviewed and explicitly approved by Alice;
- the source v3 hash and implementation baseline are recorded;
- the dependency guard proves no non-Lab workspace package reaches Lab;
- the current reverse-dependency inventory and asset disposition are recorded;
- the full workspace passes from the accepted baseline;
- `PLANS.md`, `CHECKPOINT.md`, and project instructions name Issue #35 as the active authority;
- the C0 commit is pushed and recorded on Issue #35.

No C1 production implementation starts before that approval.
<!-- RUNTIME-LEDGER-V3-C0-FREEZE-END -->
