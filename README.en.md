<div align="center">

**Chief Executive Officer & Chairman** — HS7097<br/>
**Chief Technology Officer & Chief Architect** — GPT‑6 Astra<br/>
**Advisor to the Chairman** — Fable 5.1<br/>
**Principal Engineer** — GPT‑5.6 Sol<br/>
**Interviewing** — DeepSeek

</div>

**🌐 语言 / Language:** [简体中文](./README.md) · English

# ActingCommand Runtime

> The **resident Rust runtime** of a multi-game emulator automation framework: one long-lived daemon carries scheduling arbitration, device control, and a global event ledger; all game knowledge lives outside the runtime in declarative resource packs — the kernel contains **zero game logic**. The control plane is a **clean-room Rust implementation**, rewritten against public behavior and protocols; the repository contains no C/C++ sources.
>
> **Design stance: agents outside the loop, runtime inside the loop.** Agents only do maintenance — planning, resource authoring, exception handling; frame-by-frame execution is done deterministically by the runtime, every step ledgered and auditable. Reasoning is spent on maintenance, not on execution.

CI: [current main status](https://github.com/HS7097/ActingCommand-Runtime/actions/workflows/ci.yml?query=branch%3Amain) (Windows: fmt / clippy `-D warnings` / test) · [Exact-SHA Windows build artifacts](https://github.com/HS7097/ActingCommand-Runtime/actions/workflows/windows-remote-build.yml) · License `AGPL-3.0-only` · This repository is public

**Current implementation (2026-09-08)**: the resident Runtime exposes typed IPC, resource containment, scheduling and budgets, phased tasks, page/OCR projections, and durable receipts. GlobalLedger carries the sole event and diagnostic facts. Shared queries, registration/matching/retirement of finite diagnostic signatures, and read-only offline replay have official entry points. This document describes current mainline source; device, model, resource-pack, and long-running verification claims depend on their corresponding execution facts.

---

## 🔁 Execution and closeout

![ActingCommand startup, request execution, and shutdown flow](./docs/assets/self-maintaining-loop.png)

The three rows describe separate startup, single-request, and Host-shutdown lifecycles. Startup reads configuration and acquires OwnerGuard, opens ArtifactStore and GlobalLedger, then assembles the Provider and records startup facts before publishing the current owner's ready endpoint. Provider construction-ready attests this assembly; inference and lazy initialization require their own observations. See the [daemon entry point](./apps/actingd/src/main.rs) and [Provider startup contract](./contracts/provider-startup.md).

Runtime Host owns the request lifecycle. Resource SHA-256 verification precedes extraction; Scheduler manages admission and device-write leases, while Execution Kernel runs bounded tasks, phases, and recovery. Read-only observation uses an owner-epoch-bound read capability. Lease fencing governs device effects, and terminal receipts reference durable outcome facts. GlobalLedger records events throughout; ArtifactStore retains frames and large raw payloads. Task outcome and evidence completeness are reported separately.

Host shutdown stops admission, drains work, and reclaims the policy driver before Scheduler-authorized native-resource closure, quiescence, and the final M4 summary. Sessions retained by read-only observation use dedicated resource-close-only leases. Primary and cleanup errors are both preserved; unconfirmed closure retains Unconfirmed and owner protection. The Host remains resident after an individual request; the shutdown row is a separate lifecycle. See [read-session resource closure](./contracts/read-session-resource-close.md) and [native-resource ownership](./contracts/nemu-owned-resource-close.md).

Maintenance can recover resource drafts from ledger facts and verified artifacts, then author, build, and validate them for execution. Runtime Dispatcher implements wake records, session start/resume, responses, and bounded management. Automatic external-agent launch, exploration, and the complete autonomous maintenance loop still require further implementation and verification.

## 🏛 System shape

![ActingCommand Runtime component ownership map](./docs/assets/runtime-architecture.png)

The boxes show current ownership and capabilities; the execution diagram above shows lifecycle order. Production clients request Runtime work through typed loopback IPC. Runtime holds devices and native Providers, and Scheduler arbitrates write effects. Lab/resource tooling forms a detachable authoring and debugging layer. [CONTEXT.md](./CONTEXT.md) defines the terms and responsibilities.

| Boundary | Current behavior and source entry |
|---|---|
| **Host and Provider** | Host manages owner epoch, IPC, requests, and closure. Provider assembly follows store opening; startup results are ledgered before Ready. [Startup contract](./contracts/provider-startup.md) |
| **Resources and execution** | Containment verifies pack hashes; Scheduler manages leases and fencing; Kernel consumes contained resources for bounded tasks, phases, and recovery. [Contained operations](./contracts/contained-lab-operation.md) |
| **Events and artifacts** | GlobalLedger persists sanitized events as the sole diagnostic fact source. ArtifactStore retains referenced bytes, hashes, and retention metadata; persistence failures propagate explicitly. [Event query](./contracts/global-ledger-query.md) |
| **State and policy** | RuntimeState uses SQLite for state and immutable release generations, reconciled with GlobalLedger. Policy reuses one compiler, clock, and pure evaluation semantics. [Scheduling v2](./contracts/scheduling/v2/README.md) |
| **Read-only consumers** | actingledger / ledger-forensics read the ledger and ArtifactStore, preserving corruption locations, gaps, and frozen cursors. Input material stays read-only. [Finite signatures](./contracts/diagnostic-signatures.md) |
| **Lab and resource authoring** | Online observation/operations use Runtime; detachable Lab/resource-tooling supplies offline authoring, restoration, compilation, and pack validation. [Resource restoration](./contracts/resource-restore.md) |

## 📍 Current capabilities and verification boundaries

| Capability | Current implementation |
|---|---|
| **Execution and recognition** | Typed IPC, hash containment, task timeout/step/terminal-anchor limits, phased control and total budgets, recovery packs, template matching, color predicates, OCR dictionary comparison, and official page projections. |
| **Online Lab** | observe obtains Runtime's current page projection. do resolves an operation from current element projections and can wait for a target page within the same bounded request. Old-frame hints retain provenance; actual input uses current resolution. [Page projection](./contracts/page-projection.md), [operation contract](./contracts/contained-lab-operation.md) |
| **Offline resource consumption** | restore uses GlobalLedger, verified ArtifactStore references, and the original pack to recover supported operation drafts. convert → build-task → validate reuses the existing resource pipeline. Unrecoverable fields/dependencies produce explicit gaps; authors declare the business goal. [Resource restoration](./contracts/resource-restore.md) |
| **Calendars and budgets** | Four-document policy catalogs, v2 interval-validity predicates, server clocks, pure compilation/evaluation, immutable catalogs, dispatch, and budgets. Lab scheduling compile/timeline provides official offline entry points. Configuration-registered instance aliases remain intact through policy. [Scheduling guide](./contracts/scheduling/README.md) |
| **Live facts and pools** | agent-publish-facts atomically submits typed facts through Runtime. FactStore retains original observation times, TTL, provenance, and input watermarks; ledger_fact pools derive from the same fact snapshot. Expired, unknown, low-confidence, and input-invalidated observations keep explicit states. [Live fact pools](./docs/live-fact-pools.md) |
| **State and planning** | Instance facts, strategic deficit/capacity/urgency, reports, planning signals, proposals, and Dispatcher sessions; project interface v2 supplies bounded read-only projections. Status/MonitorStatus samples are ledgered before their fact-bound responses. [State observation](./contracts/runtime-state-observation.md) |
| **Queries and signatures** | EventQuery shares module, diagnostic-code, and correlation filters with lab watch and offline events. Explicit Lab register/match/retire operations ask Runtime to record signature facts; actingledger replays frozen catalogs and inputs read-only. Missing fields, multiple matches, and incomplete evidence are explicit. [Queries](./contracts/global-ledger-query.md), [signatures](./contracts/diagnostic-signatures.md) |

Signature matching classifies a finite set of registered failure contexts; root-cause conclusions still require the original facts. Probe, signature, and replay coverage is bounded, with absent observations preserved as gaps. Real calendar dispatch, long unattended operation, device/capture-backend matrices, OCR coverage, and CUDA results must identify their execution and materials. Source integration establishes capability.

## ⚖ Seven structural invariants (enforced by guards / tests / compile-time and real-process counterexamples)

1. **The scheduler is the only arbitrated write path**: every device-state-changing operation passes scheduler admission and holds a per-instance lease; the five-field fencing tuple (epoch / lease / instance / holder / expiry) is verified field-by-field before any backend call; takeover and epoch turnover permanently invalidate old tokens; read-only observation uses an epoch-bound read capability (not a lease) and is equally ledgered;
2. **The Runtime is the only device holder**: the dependency graphs and sources of production clients (actingctl / runtime-client / ActingLab) cannot reach device backends; raw adb exists only in the `device` crate beneath the Runtime; historical client device commands are fail-loud tombstones. (Exception: `apps/device-test` is a direct-device diagnostic binary outside the production chain.);
3. **GlobalLedger is the only source of truth**: its only write entry is `append(SanitizedEventDraft)`; sanitization precedes persistence; terminal states are absorbing (duplicate/conflicting commits are rejected with an audit fact). Clients may submit typed instance facts through `PublishFact`; Runtime controls their processing and ledger persistence, and clients never write the ledger directly;
4. **Containment is the only kernel entry for resources**: hash verification (constant-time comparison) precedes extraction, with a compressed-size upper-bound precheck; the `LoadedBundle` capability makes "using an unverified pack" unrepresentable by construction — pinned by trybuild compile-failure cases;
5. **Tasks must not summon tasks**: a task only emits pure-data successor suggestions and never chain-starts them; the production path fail-louds a successor suggestion back to the caller (`contained_task_requires_scheduler`), leaving successor adjudication to Scheduler;
6. **Lab and resource tooling are detachable**: proven by a dependency-graph guard under `--all-features` — no workspace package outside Lab / ActingLab / resource-tooling has any dependency path into them (with feature-gate-bypass counterexamples); resource tooling likewise cannot reach back into the Runtime or the device layer;
7. **Zero game identity**: Runtime-owned code, contracts, and defaults are scanned by an architecture guard banning known project identity terms (game names, package names, server suffixes), with tests in scope; coordinates and thresholds exist only in resource packs, not in runtime code — a design convention, not auto-enforced. The framework recognizes "game shape" (resource pools, pages, tasks), never "game identity". Comparison **algorithms** live in the Runtime; the compared **values** (truth dictionaries etc.) all come from packs — same invariant.

Nine further **completion acceptance invariants** (deterministic replay, zero-side-effect replay, budgeted loops, full recomputation on clock jumps, crash recovery rebuilding the same pending set, no starvation of eligible work, fail-loud on invalid input, unknown never silently treated as false, a complete reason chain for every dispatch) cover the scheduling policy plane; see `docs/architecture/runtime-completion-invariants.md`.

## 📦 Components (workspace members)

**Applications**

| Name | Responsibility |
|---|---|
| `actingd` | Resident daemon process adapter hosting all kernel components below |
| `actingctl` | Production user CLI: observe / status / monitor-* / stream / reset / task-run / agent-publish-facts / request-shutdown; JSON receipts and exit status |
| `actinglab` | Online Runtime observation/operations, queries and signatures; offline resource restoration/authoring/build/validation, scheduling compilation and timeline; **not a production dependency** |
| `device-test` | Device backend diagnostic tool; independent `ledger --state-root <runtime-state>` reads Runtime facts through B ([query options](contracts/global-ledger-query.md)) |
| `vision-provider-check` | Read Provider startup facts from a specified Runtime ledger; mechanical file hashes and PE exports |
| `actingledger` (`apps/ledger-forensics`) | Read-only GlobalLedger forensics CLI |

**Production kernel**

| Name | Responsibility |
|---|---|
| `runtime-host` | Resident ownership, local typed IPC, lease-gated DeviceProxy, instance facts and policy/budget dispatch, strategic reports, and Dispatcher session lifecycle |
| `runtime-client` | Client-side typed local IPC and project interface v2 paginated read-only projections; neither constructs nor holds production device backends |
| `scheduler` | Per-instance write admission, lease lifecycle and fencing authority |
| `execution-kernel` | Daemon-held execution sessions + pure task/probe decision planning; contained-task timeout, steps, phase/total budgets, and terminal-anchor semantics |
| `ledger` | Segmented persistent global event ledger (sole event fact source and authoritative diagnostic source) |
| `artifact-store` | Artifact bytes, hashes, retention metadata, frame buffers, evidence export |
| `runtime-state` | SQLite-backed runtime state and immutable release generations, reconciled with GlobalLedger |
| `pack-containment` | Resource-pack customs (shared by dev and production) |
| `device` | Device-layer primitives; touch via explicit backend chain selection (including single-touch segmented swipe), single-backend failures visible |
| `recognition` / `recognition-pack` | Template-match evaluation / recognition pack vocabulary (including OCR targets and truth declarations) |
| `page-detector` | Page detection (rules + threshold matching) |
| `policy` | Four-document policy catalog compilation, pure scheduling evaluation, strategic deficit/capacity/urgency computation, and bounded planning |
| `actingcommand-contract` | Mainline Rust contract definitions (protocol / device / engine boundary vocabulary) |
| `host-metrics` | Safe boundary for platform performance counters |

**Recognition FFI boundary (wired into the production recognition path)**

| Name | Responsibility |
|---|---|
| `vision-ffi` | Safe Rust boundary for OCR / NN engines (absolute-path guard for native closures, strict no-fallback attestation) |
| `onnx-provider-support` | Shared support for source-form ONNXRuntime providers (init, watchdog, session cache) |
| `providers/ppocr-onnx-json` | PP-OCR ROI recognition provider (OCR JSON ABI; currently region single-line semantics — whole-page multi-block is approved backlog) |
| `providers/onnxruntime-json` | ONNXRuntime NN provider (NN JSON ABI) |

**Development & verification plane (outside the production dependency graph)**

| Name | Responsibility |
|---|---|
| `lab` | Optional Lab authoring and debug adapter |
| `resource-tooling` | Deterministic resource compilation and pack validation (Lab / CI / sealed tests only) |
| `ledger-forensics` | Read-only ledger queries and forensics used by `actingledger` |
| `tools/actinglab-architecture` | Source-derived architecture guard (ownership rule enforcement) |
| `benchmarks/rust` | Rust benchmark tool |

## 🔍 Recognition plane status

- **Current path**: NCC-family template matching and color predicates; PP-OCR single-line ROI recognition, OCR/NN JSON ABIs, canonical/alias/tolerant dictionary comparison, bounded retries, and per-invocation execution provenance. Current facts identify the model, provider, and device.
- **Coverage boundary**: the provider currently uses region single-line semantics. Whole-page multi-block detection, complete roster coverage, CUDA, and capture-backend combinations require their respective implementation or verification. Existing CPU-run evidence remains bounded by its original records.
- **External dependencies**: ONNX Runtime native libraries and models use pinned-source, hash-verified materialization; see the [Windows tools guide](./scripts/windows-tools/README.md). They are not distributed with this repository.
- **Startup diagnosis**: vision-provider-check with `--state-root` reads Provider startup facts from the same Runtime through the read-only forensics layer. `--after`, `--through`, and `--limit` provide bounded cursors. Manifest, artifact-lock, and export-audit modes are mechanical file observations. Inference and lazy initialization require corresponding facts.

## 🧭 Design principles

- **Game shape, not game identity**: onboarding a new game = creating a new resource repository, zero runtime commits;
- **Declarations before code**: recognition, navigation, operations, recovery, and scheduling policy all use statically verifiable declarative data;
- **Fail-loud**: severe errors fail explicitly, never fake success; only transient errors get bounded retries, fully ledgered;
- **Clean room**: rewritten against public behavior and protocols; no copying of copyrighted implementations;
- **Transactional resource publishing**: staging → full validation → hash → atomic swap; failures leave no mixed tree;
- **Ledger-first diagnosis**: reds are read from the global ledger first; where the ledger cannot explain a cause, the module gains probe capability instead of new diagnostic tooling.

## 🚀 Build & run

Current CI uses Windows and Rust stable; the default Windows artifact target is `x86_64-pc-windows-msvc`. Local builds require Rust/Cargo, Git, and the corresponding MSVC build environment; exact-SHA build artifacts are another entry point. External tools and artifact verification are documented in the [Windows tools guide](./scripts/windows-tools/README.md).

For a first run, prepare a daemon configuration with at least one instance. It must declare `schema_version`, `state_root`, a loopback `bind_host`, a 16–1024-byte `secret_fingerprint_salt`, and nonempty `instances`. Each device instance needs an alias, `instance_id`, application identity, ADB addressing, and explicit capture/touch backends. The [configuration definition](./apps/actingd/src/config.rs) specifies all fields and validation. Device tasks additionally need working ADB/selected backends and a resource pack you provide; OCR tasks also need an external provider, models, and native-library manifest. See the [scheduling contract](./contracts/scheduling/README.md) for catalog documentation and neutral declaration examples, and [project interface v2](./contracts/runtime-project-interface.md) for the client query contract.

```bash
# The build reads git metadata; without .git set ACTINGCOMMAND_RUNTIME_HEAD=<40-char commit hash>
cargo build --release
cargo test --workspace

# Binaries land in target/release/ (call with the path if not on PATH)

# Start the resident daemon
# The config declares state_root, instance aliases and device addressing,
# capture/touch backends (explicit; `auto` is not accepted), and app identity.
# Field definitions: apps/actingd/src/config.rs; prints `actingd ready pid=… host=… port=…` when ready
actingcommand-actingd --config <actingd.json>

# <state-root> below must be the same directory as state_root in the config:
# clients read the daemon endpoint from it; no separate address is given

# Daemon-level status (does not accept --instance)
actingctl status --state-root <state-root>

# Read-only single-frame observation (owner-epoch-bound read capability; events and artifacts ledgered)
actingctl observe --state-root <state-root> --instance <alias>

# Execute a contained task pack (hash verification precedes extraction)
# --expected-sha256 is 64 lowercase hex chars, no `sha256:` prefix
# Optional: declare a recovery pack; if the entry page does not match,
# the runtime autonomously repositions once
actingctl task-run --state-root <state-root> --instance <alias> \
  --package <task.zip> --expected-sha256 <hash> \
  [--recovery-package <recovery.zip> --recovery-expected-sha256 <hash>]
```

`actingctl` writes single-line JSON to stdout, including the official OCR projection where applicable. Failed receipts can also include JSON and exit nonzero; argument, connection, and other errors write text to stderr. Integrations must handle both output streams and the exit code. `actingcommand-actingd` and `actingctl` use hand-written argument parsing and provide **no `--help` / `--version`**; see the [CLI entry point](./apps/actinglab/src/main.rs) for ActingLab commands and options.

Every `actingctl` command requires `--state-root`. The parameters consumed by each subcommand are listed below; the [argument parser](./apps/actingctl/src/main.rs) defines current behavior:

| Subcommand | Instance and command parameters |
|---|---|
| `status` / `monitor-status` / `request-shutdown` | Do not accept `--instance` |
| `agent-publish-facts` | Requires `--record-file`; the submission carries its fact scopes; does not accept `--instance` |
| `observe` / `reset` / `monitor-clear` | Require `--instance` |
| `monitor-set` | Requires `--instance`; optional `--interval-ms` (default 30000), `--expect` (default `home`), `--recover` |
| `stream` | Requires `--instance`; optional `--max-frames` (default 1), `--interval-ms` (default 250) |
| `task-run` | Requires `--instance`, `--package`, `--expected-sha256`; recovery parameters `--recovery-package` and `--recovery-expected-sha256` must be supplied together |

Use only the parameters belonging to the selected subcommand; the current parser accepting a known parameter does not imply that the subcommand uses it.

`actingctl request-shutdown --state-root <state-root>` is an ordinary local Cli/Cli maintenance entry point. The client freezes the discovered owner epoch, PID, and start time. Host checks that owner, active leases, queued work, and in-flight requests/native actions at its admission gate. A busy owner returns `RuntimeBusy` and continues serving; an owner mismatch returns `RuntimeOwnerMismatch`. This operation needs no governance secret; owner epoch is not authentication.

Acceptance first records the typed GlobalLedger target and decision, then stops admission. The daemon reclaims the policy driver and uses `RuntimeHost::close` to settle resources, the M4 summary, ledger, and owner. JSON `shutdown_accepted` and `admitted` mean acceptance only; completion requires actual closure facts, the final summary, and the process result. A lost receipt reports `runtime_shutdown_receipt_unconfirmed` with the original error. The client does not resubmit or switch owners. Existing fatal and unconfirmed-resource protection remains in force.

## 🎮 Resource packs and deployment

Game templates, navigation, operations, recovery, and calendar declarations are versioned in independent resource sources. Resource tooling produces formal packs for Runtime containment by exact hash. Authors maintain provenance, licenses, and material evidence. Reusable assets and deployment-specific tasks are organized separately, with account-specific selections and configuration kept in private deployments.

[Resource restoration](./contracts/resource-restore.md) describes drafting from existing ledger facts and the original pack; [scheduling declarations](./contracts/scheduling/README.md) connect tasks, procedures, facts, and budgets. Reproducing a task requires its resources, dependencies, and deployment configuration. Pack generation/compilation and actual execution results are recorded separately.

## 🤝 How we collaborate

Development uses branches and pull requests. Reviews rely on an identified source version, observable behavior, and relevant CI results; device verification should identify the backend, resource pack, and execution boundary. The public repository provides Runtime and resource-authoring entry points; reproducing game tasks also requires obtaining or authoring the corresponding resources.

## Conventions & license

- **Clean-room boundary**: the control plane is rewritten against public behavior and protocols; no C/C++ sources in the repository; the only third-party artifact distributed with the repo is `external-tools/maatouch` (Apache-2.0) — see [NOTICE.md](./NOTICE.md);
- **Recognition licensing boundary**: OCR/NN loads external providers dynamically via FFI; models and native libraries are not distributed with the repository;
- **Contribution flow**: branch + PR by default; all required CI must pass before merge;
- **Documentation sync**: `README.md` and `README.en.md` must change in the same batch and stay factually consistent;
- License: **AGPL-3.0-only**.
