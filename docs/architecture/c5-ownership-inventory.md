# C5 Ownership and Side-Effect Inventory

Status: frozen migration inventory for C5 Task 1.

Baseline: `ff647cc` on `issue-35-runtime-ledger-v3`.

This inventory records current ownership, side effects, replacement owners, and equivalence gates.
It is not evidence that temporary Lab ownership is correct, and it is not permission to delete a
module before its replacement gate passes.

Current disposition update: C6 retired `actingcommand-arbitrator` after online Lab2 commands moved
behind Runtime scheduler IPC and offline `--scene` commands became state-free. The command parser
retains an explicit fail-loud compatibility tombstone, but no Lab arbitrator package, store, lease,
recovery file, or device backend authority remains.

## Workspace dependency snapshot

| Package | Current relevant consumers | C5 disposition |
| --- | --- | --- |
| `actingcommand-lab` | `actingcommand-actinglab` only | Reduce to optional authoring/debug/sealed adapters |
| `actingcommand-execution-kernel` | `actingcommand-runtime-host` | Deepen into production execution owner |
| `actingcommand-artifact-store` | Lab and ActingLab plus tests | Keep as frame/artifact/evidence owner; remove Lab ownership wrapper |
| `actingcommand-task-loop` | `actingcommand-device-test` only | Move accepted decision behavior into execution ownership, then retire |
| `actingcommand-runtime-core` | none | Retire only after a fresh reverse-dependency and behavior check |
| `actingcommand-arbitrator` | `actingcommand-actinglab` only | Keep as legacy/debug inventory until scheduler migration evidence permits retirement |
| `actingcommand-resource-tooling` | not created at this baseline | Add as Lab-owned developer-only compiler/validator |

## Capability inventory

| Current owner | Current capability and state | Side effects | C5 destination | Equivalence gate |
| --- | --- | --- | --- | --- |
| `crates/lab/context.rs`, `state.rs`, `ledger_port.rs` | Lab request context, legacy state roots, ledger wrappers | State-directory reads/writes and legacy ledger append/read | Runtime contract/ledger for online facts; Lab-only temporary state for sealed/offline work | Runtime projection/process tests plus Lab offline tests |
| `crates/lab/env_detection.rs`, `env_api.rs` | Environment detection, freshness, persisted result resolution, env-marker substitution | Capture/input through ports; resource/config/state reads; env-result writes | Execution kernel owns live detection; Runtime host owns config/state lifecycle; resource-tooling receives resolved `EnvResolved` snapshot only | `detect`, `env resolve`, `env status` goldens; existing env unit tests; stale-result adversarial tests |
| `crates/lab/readonly.rs`, `readonly_api.rs` | Recognition, page detection, current-page, visibility | Capture through Lab port; recognition/page evaluation | Execution kernel over daemon-owned capture and existing recognition/page crates | `recognize`, `detect-page`, `current-page`, `is-visible`, `observe` goldens and C3b observation process tests |
| `crates/lab/drive.rs`, `drive_api.rs` | Target tap planning/execution and navigation | Capture, recognition, input, semantic ledger records | Execution kernel after scheduler admission; ledger facts emitted through Runtime host | `tap-target`, `navigate`, `do` goldens; intent/outcome ordering and fencing counterexamples |
| `crates/lab/lab_run*` | Bundle validation, operation execution, recovery, run context, output assembly | Containment load, capture/input, frame buffering, ledger writes, output/ZIP files | Split across execution kernel, pack-containment, artifact-store, and ledger projection | `lab validate`/`lab run` goldens, Lab run API/tests, sealed package runs, evidence manifest verification |
| `crates/lab/frame_store.rs` | Error-mapping wrapper around artifact-store frame store | Temporary frame files and materialization through artifact-store | Direct artifact-store ownership; compatibility mapping removed with final Lab caller | Artifact-store frame-store/pipeline tests and Lab run output equivalence |
| `crates/lab/package_*`, `resource_convert`, `maa_task_graph` | Package API/build/validate, conversion, MAA compilation | Resource-tree reads/writes, ZIP writes, temporary clone, child `git`, containment validation | `actingcommand-resource-tooling`; Lab remains workflow caller | Package/convert/MAA unit tests, 30-case protocol matrix, containment round-trip, deterministic output hashes |
| `crates/task-loop` | Pure task/probe validation and decision planning | None | Execution-kernel domain module | JSON/API equivalence, all existing task-loop tests, device-test compile/run with fake data |
| `apps/actinglab` Session code | Legacy daemon info/heartbeat, request file queue, lease files, Session policy/views | State files, TCP liveness, process lifecycle, legacy queue/journal writes | Runtime host local IPC/control plane; scheduler remains lease owner | Session closeout/process tests, restart/reconnect, owner conflict/takeover, no legacy second authority |
| `apps/actinglab` monitor/recovery code | Monitor loop, diagnosis, recovery routing and resource plan execution | Capture, optional input, state reads, recovery calls | Runtime host lifecycle plus execution-kernel recovery | Monitor/self-heal tests, typed event/receipt projection, no task-to-task invocation |
| `apps/actinglab` stream code | Bounded frame stream and optional input relay | Capture cadence, optional input, event assembly | Runtime host bounded local stream; client receives typed events | Stream preflight/lease tests, bounded stream tests, reconnect and cancellation tests |
| `apps/actinglab` record code | Resource recording workflow and candidate files | Capture, draft/candidate/output files, authoring state | Lab workflow in C6; pure draft/file planning moves to resource-tooling | Existing record tests, then C6 authoring transaction tests |
| `apps/actingctl`, `crates/runtime-client` | Thin production commands and typed local IPC | Local IPC only | Retain as disposable clients | C3b source/dependency guards and process tests |
| `crates/runtime-core` | Disconnected prototype | None in current graph | Remove after fresh reverse-dependency proof | Full workspace and non-Lab build after deletion |

## Mutable state owners

| State | Current authoritative owner | Temporary duplicate | C5 terminal owner |
| --- | --- | --- | --- |
| Runtime process identity and owner epoch | Runtime host | Legacy Session daemon files in ActingLab | Runtime host only |
| Lease, queue, preemption, transfer | Scheduler | Legacy Lab2/session lease files | Scheduler only |
| Input and capture backend sessions | Execution kernel | No production client after C3b | Execution kernel only |
| Global event sequence and durable facts | Global ledger in Runtime host | Lab legacy ledger wrappers | Global ledger only; Lab becomes query/client adapter |
| Captured frame bytes and retention | Artifact-store | Lab frame-store wrapper | Artifact-store only |
| Environment result/freshness | Lab state/config path | None | Runtime configuration/state owner with execution-produced typed facts |
| Task/run/recovery execution state | Lab run and ActingLab monitor code | Task-loop pure decision state | Execution kernel only after admission |
| Session/monitor/stream lifecycle | ActingLab process/files | Runtime C3b control plane | Runtime host only |
| Resource compiler inputs/outputs | Lab package/convert modules | ActingLab orchestration | Resource-tooling algorithms; Lab owns authoring workflow |
| Offline/replay/debug state | Lab | None | Lab temporary isolated state only |

## Side-effect boundaries

| Side effect | Allowed C5 owner | Forbidden owner |
| --- | --- | --- |
| Live capture/input backend open | Execution kernel | Runtime client, actingctl, Lab online client, resource-tooling |
| Lease grant/renew/release/preempt | Scheduler through Runtime host | Execution code, Lab, resource-tooling |
| Global ledger append | Runtime host composition and approved producer ingress | Client-created receipts, resource compiler direct storage mutation |
| Artifact/frame writes | Artifact-store | Ledger JSON, resource-tooling, client process |
| Loose resource compilation and ZIP generation | Resource-tooling under explicit Lab/CI request | Production Runtime/execution path |
| Resource publication | C6 transactional Lab authoring workflow | Test success, resource compiler implicit action, Runtime |
| Child `git` or source acquisition | Lab authoring adapter with explicit request | Execution kernel, Runtime host, scheduler |
| TCP/local IPC listener | Runtime host | Execution domain/resource-tooling; legacy ActingLab listener retires after equivalence |

## Frozen compatibility evidence

### A1 protocol matrix

The checked-in golden matrix contains success and failure cases for exactly these 15 commands:

- `recognize`
- `detect-page`
- `current-page`
- `is-visible`
- `tap-target`
- `navigate`
- `package validate`
- `package build-task`
- `lab validate`
- `lab run`
- `detect`
- `env resolve`
- `env status`
- `observe`
- `do`

The 30 stored envelopes remain byte-for-byte normalized expectations during C5 ownership moves.

### Required suites by migration area

| Migration area | Required evidence before old owner is removed |
| --- | --- |
| Resource tooling | Package build/validate, convert, MAA, containment, protocol goldens, deterministic hash fixtures |
| Task-loop | All task-loop tests, JSON round trips, device-test fake-path compile/tests, reverse dependencies |
| Env/readonly | Env freshness/marker tests, recognition/page tests, protocol goldens, daemon capture process tests |
| Drive/run/recovery | Drive/Lab-run tests, scheduler fencing, intent/outcome ledger order, sealed package tests |
| Frame/output | Artifact-store pipeline/exporter suites, pinned/pressure counterexamples, evidence manifest checks |
| Session/monitor/stream | Session closeout, process restart/reconnect, monitor/self-heal, stream/lease/preflight tests |
| Lab reduction | Production metadata cannot reach Lab/resource-tooling; non-Lab workspace tests pass |

## Removal rule

A temporary module can be removed only when:

1. its destination owner is implemented;
2. its listed equivalence gates pass against the same fixtures;
3. a fresh reverse-dependency check shows no remaining caller;
4. protocol goldens remain unchanged when the module affects a frozen command;
5. planning and checkpoint evidence records the replacement commit.
