**🌐 语言 / Language:** [简体中文](./README.md) · English

# ActingCommand Runtime

> The **resident Rust runtime** of a multi-game emulator automation framework: a long-lived daemon owns scheduling arbitration, device control and a global event ledger, while all game knowledge lives in declarative resource packs — the runtime kernel contains **zero game logic**. The control plane is a **clean-room Rust implementation** — rewritten from public behavior and protocols, with no C/C++ source in this repository.

`cargo test --workspace` **all green (48 test binaries / 1703 tests, plus 14 doc-tests)** · CI: GitHub Actions (single windows-latest job: fmt / clippy `-D warnings` / test) · License `AGPL-3.0-only` · This repository is public

**Current maturity**: scheduling arbitration, the device throat, the global ledger and task containment are established and policed by the tests and architecture guards above; **recognition is currently template matching (NCC family) and color predicates**. The OCR / neural-network FFI boundary and two source-form providers are in place but **not yet wired into the runtime recognition path**, and resource-pack vocabulary cannot declare OCR/NN targets yet (see "Recognition status").

Early Python mocks and Go legacy contracts, together with the Go/Python benchmark tooling, moved out of this repository (archived in ActingCommand-Legacy-Runtime, **not yet public**); the Rust benchmark tool `benchmarks/rust` and historical benchmark reports remain here.

---

## 🏛 System shape

```mermaid
graph TD
  subgraph clients["Clients (all reach the daemon through runtime-client over typed loopback IPC)"]
    ctl["actingctl<br/>production user CLI<br/>dependency graph cannot reach device or recognition"]
    lab["ActingLab<br/>removable debug probe + resource authoring workbench<br/>guards: no device backend construction · no production ledger writes"]
    fut["UI / agent clients<br/>(planned)"]
  end

  rc["runtime-client<br/>typed IPC client (depends only on contract + policy)"]

  subgraph daemon["actingd resident daemon (exclusive state-root · crash takeover · epoch rotation)"]
    host["runtime-host — sole orchestrator<br/>request validation · receipts · replay idempotency<br/>sole ledger writer"]
    sched["scheduler — pure decision kernel<br/>admission · per-instance leases · fencing<br/>(depends only on contract)"]
    kernel["execution-kernel<br/>app lifecycle · contained-task state machine<br/>capture / fenced input / recovery"]
    ledger["GlobalLedger — single source of truth<br/>one append entry point (sanitize before persist)<br/>terminals absorbing, never overwritten"]
    store["artifact-store<br/>content-addressed · tiered retention · evidence export"]
  end

  contain["pack-containment — sole kernel resource ingress<br/>hash before unpack · bounded extraction · LoadedBundle capability"]
  dev["device<br/>adb / maatouch / minitouch / nemu-ipc backends"]
  res["Resource packs (separate repos)<br/>recognition / navigation / operations / recovery — all declarative"]
  emu["Android emulators ×N"]

  ctl --> rc
  lab --> rc
  fut -.-> rc
  rc --> host
  host -->|admission · lease decisions (pure function calls)| sched
  host -->|execution orchestration| kernel
  host -->|constructs and owns device backends| dev
  kernel -->|ExecutionBackendProvider| dev
  dev --> emu
  res -->|zip bytes + expected sha256| host
  host --> contain
  contain -->|LoadedBundle| kernel
  host ==>|sole append: every event<br/>scheduler / device-proxy / capture are attribution labels, not writers| ledger
  host --- store
  host -.->|ledger projections / subscriptions| clients
```

Terminology is defined in [CONTEXT.md](./CONTEXT.md) (Runtime Host / Scheduler / Execution Kernel / Device Throat / DeviceProxy and others, one entry each).

## ⚖ Seven structural invariants (enforced by guards, tests, and compile-time and real-process counterexamples)

1. **Scheduler is the sole arbiter of the write path**: every state-changing device operation is admitted first and holds a per-instance lease; the fencing tuple (epoch / lease / instance / holder / expiry) is checked field by field before any backend call, and takeover or epoch rotation permanently invalidates stale tokens; read-only observation uses an epoch-bound read capability instead of a lease and is equally recorded;
2. **Runtime is the sole device owner**: production clients (actingctl / runtime-client / ActingLab) can reach device backends neither through their dependency graph nor in source; raw adb exists only in the `device` crate beneath the Runtime; legacy client device commands are fail-loud tombstones. (Exception: `apps/device-test` is a direct-to-device diagnostic binary, outside the production path and not bound by this guard);
3. **GlobalLedger is the single source of truth**: the one write entry point is a compile-time-unique `append(SanitizedEventDraft)` — sanitization precedes persistence; terminals are absorbing (duplicate/conflicting commits are rejected with an audit fact); clients cannot submit semantic facts, because the contract layer does not expose semantic fact types at all;
4. **Containment is the sole kernel resource ingress**: hash verification (constant-time comparison) precedes extraction, with a compressed-size upper bound checked first; the `LoadedBundle` capability makes "unverified pack in use" unrepresentable by construction — pinned by a trybuild compile-fail case;
5. **Tasks never spawn tasks**: a task only produces a pure-data successor suggestion and never chains into it; on the production path a successor suggestion is returned fail-loud to the caller (`contained_task_requires_scheduler`). Scheduler adjudication of successors is the planned next step;
6. **Lab and resource tooling are removable**: proven by dependency-graph guards under `--all-features` — apart from Lab / ActingLab / resource-tooling themselves, no workspace package has any dependency path reaching them (with counterexample cases covering feature-gate bypass); resource tooling likewise must not reach back into the Runtime or device layer;
7. **Zero game identity**: architecture guards scan Runtime-owned code, contracts and defaults for known project identity words (game names, package ids, server suffixes), and test code within that scope is policed too; coordinates and thresholds live only in resource packs, never in runtime code — that part is a design convention, not automatically enforced. The framework understands *game shapes* (resource pools, pages, tasks), never *game identities*.

A separate family of **nine completion-acceptance invariants** (deterministic replay, replay without a second effect, bounded loops, full recomputation on clock jumps, crash recovery rebuilding the same pending set, no starvation of eligible work, fail-loud on invalid input, `unknown` never silently treated as false, and a complete reason chain per dispatch) covers the scheduling-policy axis — see `docs/architecture/runtime-completion-invariants.md`.

## 📦 Components (all 28 workspace members)

**Applications**

| Name | Responsibility |
|---|---|
| `actingd` | resident daemon process adapter hosting every kernel component below |
| `actingctl` | production user CLI (observe / status / monitor-* / stream / reset / task-run); emits single-line JSON |
| `actinglab` | debug probe + resource authoring (record→draft→build→transactional promote); **not a production dependency** |
| `device-test` | device backend diagnostics tool |
| `vision-provider-check` | vision provider self-check (ABI verification / artifact lock / OCR·NN smoke) |

**Production kernel**

| Name | Responsibility |
|---|---|
| `runtime-host` | resident ownership, local typed IPC, lease-gated DeviceProxy, lifecycle control |
| `runtime-client` | typed local IPC client; never constructs or owns production device backends |
| `scheduler` | per-instance write admission, lease lifetime and fencing authority |
| `execution-kernel` | daemon-owned execution sessions plus pure task and probe decision planning |
| `ledger` | global event ledger (single source of truth) |
| `artifact-store` | artifact bytes, hashes, retention metadata, frame buffering and evidence archive export |
| `runtime-state` | SQLite-backed authoritative Runtime state and immutable release generations |
| `pack-containment` | resource pack customs (shared by dev and production) |
| `device` | device-layer primitives; touch is selected through an explicit backend chain so single-backend failures stay visible |
| `recognition` / `recognition-pack` | template match evaluation / recognition pack declaration vocabulary |
| `page-detector` | page detection (rules + threshold matching) |
| `policy` | pure scheduling-policy contracts shared by the catalog compiler and evaluator |
| `actingcommand-contract` | Rust mainline contract definitions (protocol / device / engine boundary vocabulary) |
| `host-metrics` | safe boundary for platform performance counters |

**Recognition FFI boundary (not yet wired into the production recognition path)**

| Name | Responsibility |
|---|---|
| `vision-ffi` | safe Rust boundary for OCR / NN engines; stops at the process/FFI contract surface |
| `onnx-provider-support` | shared support for source-form ONNXRuntime providers (initialization, watchdogs, session caches) |
| `providers/ppocr-onnx-json` | PP-OCR ROI recognizer provider (implements the OCR JSON ABI) |
| `providers/onnxruntime-json` | ONNXRuntime NN provider (implements the NN JSON ABI) |

**Development and verification surface (outside the production dependency graph)**

| Name | Responsibility |
|---|---|
| `lab` | optional Lab authoring and debug adapter |
| `resource-tooling` | deterministic resource compilation and package validation (Lab / CI / sealed tests only) |
| `tools/actinglab-architecture` | source-derived architecture guards (ownership rule enforcement) |
| `benchmarks/rust` | Rust benchmark tool |

## 🔍 Recognition status

- **Available**: template matching (NCC family) and color predicates, provided by `recognition` / `recognition-pack` / `page-detector`;
- **In place but unwired**: `vision-ffi` defines the process-level OCR / NN JSON ABI, and the two providers under `providers/` are real inference implementations (loading ONNX Runtime dynamically through `ort` with `load-dynamic`);
- **Not yet available**: the runtime recognition path does not link these providers (no production member depends on both `recognition-pack` and `vision-ffi`), and resource-pack vocabulary cannot declare OCR/NN targets;
- **Not distributed here**: neither the ONNX Runtime native library nor OCR/NN models ship with this repository; local smoke runs require operator-supplied artifacts, and `apps/vision-provider-check` is the self-check entry point.

## 🧭 Design principles

- **Game shape, not game identity**: onboarding a new game = creating one resource repository, with zero runtime commits;
- **Declarations before code**: recognition, navigation, operations, recovery and (planned) scheduling policy are statically validatable declarative data;
- **Fail loud**: severe errors fail explicitly, never fake success; only transient errors may retry within bounds, fully recorded;
- **Clean room**: reference public behavior and protocols, never copy copyrighted implementations;
- **Transactional resource promotion**: staging → full validation → hashing → atomic swap; failures never leave a mixed tree.

## 🚀 Build & run

```bash
# The build must be able to read git metadata; without .git, set ACTINGCOMMAND_RUNTIME_HEAD=<40-hex commit> explicitly
cargo build --release
cargo test --workspace

# The binaries below land in target/release/ (call them by path unless you add it to PATH)

# Start the resident daemon
# The config declares state_root, instance aliases and device addressing, capture/touch backends
# (explicit — `auto` is rejected) and application identity; fields live in apps/actingd/src/config.rs.
# On readiness it prints `actingd ready pid=… host=… port=…` to stdout.
actingcommand-actingd --config <actingd.json>

# <state-root> below MUST be the same directory as state_root in the config file:
# clients read the daemon endpoint from it and take no address argument.

# Daemon-level status (does not accept --instance)
actingctl status --state-root <state-root>

# Read-only observation of one frame (scheduler-admitted; events and frame artifacts fully recorded)
actingctl observe --state-root <state-root> --instance <alias>

# Execute a contained task package (hash verified before unpacking)
# --expected-sha256 is 64 lowercase hex characters, with no `sha256:` prefix
actingctl task-run --state-root <state-root> --instance <alias> \
  --package <task.zip> --expected-sha256 <hash>
```

Every `actingctl` command writes a single line of JSON to stdout, which suits scripts and agents; `monitor-status` / `monitor-set` / `monitor-clear` / `stream` / `reset` are also available. Both CLIs parse arguments by hand and therefore provide **no `--help` / `--version`**.

## 🎮 Resource repositories

Game data (recognition templates, navigation graphs, operation and recovery declarations) is versioned independently of the runtime. The repositories below are **currently private** and not reachable by outside readers:

- ActingCommand-Resources-Arknights
- ActingCommand-Resources-AzurLane
- ActingCommand-Resources-BlueArchive

Each repository uses a two-tier layout: `upstream-derived/` (third-party derived material with licenses and provenance) + `ours/` (our own declarative data).

## Conventions & license

- **Clean-room boundary**: the control plane is rewritten from public behavior and protocols, and this repository contains no C/C++ source; the only third-party artifact shipped here is `external-tools/maatouch` (Apache-2.0) — provenance and license in [NOTICE.md](./NOTICE.md);
- **Recognition licensing boundary**: OCR/NN recognition dynamically loads external providers via FFI; models and native libraries are not distributed with this repository;
- **Contribution flow**: changes normally land via branch + PR with all required CI green before merge;
- **Documentation sync**: `README.md` and `README.en.md` must be changed in the same batch and stay factually identical;
- License: **AGPL-3.0-only**.
