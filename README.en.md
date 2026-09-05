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

**Current maturity (2026-09-05)**: scheduling arbitration, the device throat, task containment, declarative policy catalogs, and budgeted dispatch are wired into the resident runtime. Instance facts, strategic evaluation, reports and planning signals, proposal generation, and the Runtime Dispatcher session protocol are implemented. GlobalLedger is the sole global event fact source and is being extended into an authoritative debugging tool across modules. **The OCR recognition chain has passed one full official CPU live run**: autonomous return-to-Home recovery, template navigation, single-touch segmented-swipe paging, 16-target OCR per frame, dictionary-normalized comparison, terminal-anchor-page completion, and a `return_home` closeout, in one command, 219 seconds, zero manual input. Real scheduling time semantics, long unattended operation, OCR coverage, and CUDA still need their corresponding live evidence.

The early Python mock, historical Go contracts, and Go/Python benchmark tools have been moved out of this repository (archived in ActingCommand-Legacy-Runtime, **not yet public**); the Rust benchmark tool `benchmarks/rust` and historical benchmark reports remain.

---

## 🔁 The self-maintaining loop

![ActingCommand self-maintaining loop](./docs/assets/self-maintaining-loop.png)

The reason this architecture exists: to free game automation from "the game updates, and the whole world waits for the maintainer to ship." The target loop — after a game update (①), agents play and map out the changes (②), then author or revise declarative resource packs (③); packs enter the runtime through hash containment, get admitted by the scheduler (④), executed deterministically (⑤), fully ledgered (⑥); on failure, agents self-diagnose from ledger evidence and repair the resources (⑦), returning to ③. Zero per-frame reasoning inside the loop — reasoning is spent on maintenance, not execution.

Resource authoring, execution, and ledger diagnosis have been connected in practice: on the Aug–Sep 2026 OCR task chain, the resource pack was agent-authored, diagnosed and revised using ledger evidence, and then passed live — including the agent-assisted `return_home` recovery pack, now a live-verified reusable baseline in the resource repository. Runtime implements wake records, session start/resume, responses, and bounded session management. Actual automatic launch of external agents, ② (automatic exploration), and the complete autonomous maintenance loop remain planned.

## 🏛 System shape

![ActingCommand Runtime architecture](./docs/assets/runtime-architecture.png)

Green/blue nodes and solid lines represent current capabilities merged into `main` and wired into their corresponding entry points; orange nodes and dashed lines represent capabilities that are planned, in progress, or awaiting verification. GlobalLedger is the sole global event fact source; existing read-only forensics and future authoritative debugging both read from it. Source integration does not imply that every corresponding live scenario has been verified.

Terminology follows [CONTEXT.md](./CONTEXT.md) in the repository (Runtime Host / Scheduler / Execution Kernel / Device Throat / DeviceProxy, each defined precisely).

**GlobalLedger's diagnostic role**: typed events, durable receipts, and replay already exist, with `actingledger` providing read-only forensics. Module-wide probe coverage, recurrence signature matching, and replay assessment are in progress. The goal is to elevate the ledger into an authoritative debugging tool that locates normal results, degradation, and failure causes in the same event source, keeping every diagnosis traceable to its original facts. Complete diagnostic coverage is not yet available.

## 📍 Current progress (2026-09-05)

| Milestone | What happened |
|---|---|
| **2026-08-10** | First live end-to-end loop: hash-sealed resource pack → resident runtime → live emulator page recognition → contained task execution → typed scheduling outcome → fully ledgered; 3.4 seconds per run, zero manual input. |
| **2026-08-20** | Composite daily + weekly reward-claim task chain completed live (real claim branch verified). |
| **2026-09-01** | **Official CPU live OCR full run: PASS** — non-Home entry autonomously recovered by the runtime (2 steps back to Home, re-verified) → template navigation to the operator roster → 41 frames of single-touch segmented-swipe paging (uniform drag + vertical brake, MaaTouch point stream) → 16-target OCR per frame (920 mapping records, zero discarded) → canonical/alias/tolerant dictionary comparison (294 unique canonical names, zero out-of-dictionary) → `operator_end` anchor-page termination → `return_home` closeout. 219 seconds, strict no-fallback, all 42 projection artifacts hash-bound. |

| Dimension | State |
|---|---|
| **Execution and resource entry points on `main`** | Resident daemon, typed loopback IPC, scheduling admission and lease fencing, contained task execution (task timeout, terminal anchors, independent `max_steps`, recovery-pack auto-repositioning), pack containment, artifact store and official OCR projection (v2, paginated), device backends (including `SegmentedSwipe`, MaaTouch/Minitouch point streams, dynamic MuMu Nemu IPC binding), NCC template matching and color predicates, production OCR provider wiring, and dictionary-constrained comparison. ActingLab connects recording, drafts, pack building, transactional publication, and offline `package dry-run` rehearsal. |
| **Scheduling and maintenance interfaces on `main`** | Four-document declarative policy catalogs, the pure evaluator, immutable catalog versions, dispatch, and budgets are wired into `actingd`. Instance `PublishFact`, strategic deficit/capacity/urgency evaluation, reports, planning signals, and proposal generation are implemented. Runtime Dispatcher implements wake/session/start/resume/response, recovery, and bounded configuration. Project interface v2 provides paginated read-only projections of projects, instances, catalogs, facts, goals, decisions, runtime state, and diagnostics for future UI queries. |
| **Current persistence and diagnosis** | GlobalLedger uses segmented persistence and is the sole global event fact source. RuntimeState uses SQLite for runtime state and immutable release generations, reconciled with the Ledger. `ledger-forensics` / `actingledger` provide read-only forensics. |
| **In progress and awaiting verification** | Module-wide ledger probe coverage, signature matching and replay assessment; real scheduling time semantics and long unattended operation; expansion of the first complete resource task set; OCR coverage, whole-page multi-block detection, and CUDA live testing; coverage of the capture backend matrix (adb / droidcast_raw / nemu_ipc). CPU OCR has the single full live-run result recorded above. |
| **Future work** | Automatic launch of external agents and the complete autonomous maintenance loop; a native Rust read-only monitoring console; a GlobalLedger SQLite backend and unified RuntimeDatabase. |

## 🗺 Roadmap

Remaining capabilities and verification work, with no dates promised:

1. **Authoritative ledger debugging** — extend typed probes across modules and complete signature matching and replay assessment so normal results, degradation, and failure causes can be located in the ledger;
2. **Resident-operation evidence** — build on existing policy, budget, fact, and strategic-report capabilities to verify real time semantics, recovery, and long unattended operation, comparing planning signals with actual outcomes;
3. **Resources and recognition** — extend the first complete task set, verify roster coverage, and complete provider whole-page multi-block detection (whole-page reads + overlap dedup), CUDA, and capture-backend matrix verification;
4. **Clients and autonomous maintenance** — build a native Rust read-only monitoring console, connect external agent launch to the existing Dispatcher session interface, and progressively complete automatic exploration, resource revision, and re-verification;
5. **Future storage evolution** — plan a GlobalLedger SQLite backend and unified RuntimeDatabase while preserving the sole event fact source and recoverable state reconciliation;
6. **MAA / MaaFramework compatibility** — continue the MAA resource seed-import and MaaFramework second-execution-backend directions.

## ⚖ Seven structural invariants (enforced by guards / tests / compile-time and real-process counterexamples)

1. **The scheduler is the only arbitrated write path**: every device-state-changing operation passes scheduler admission and holds a per-instance lease; the five-field fencing tuple (epoch / lease / instance / holder / expiry) is verified field-by-field before any backend call; takeover and epoch turnover permanently invalidate old tokens; read-only observation uses an epoch-bound read capability (not a lease) and is equally ledgered;
2. **The Runtime is the only device holder**: the dependency graphs and sources of production clients (actingctl / runtime-client / ActingLab) cannot reach device backends; raw adb exists only in the `device` crate beneath the Runtime; historical client device commands are fail-loud tombstones. (Exception: `apps/device-test` is a direct-device diagnostic binary outside the production chain.);
3. **GlobalLedger is the only source of truth**: its only write entry is `append(SanitizedEventDraft)`; sanitization precedes persistence; terminal states are absorbing (duplicate/conflicting commits are rejected with an audit fact). Clients may submit typed instance facts through `PublishFact`; Runtime controls their processing and ledger persistence, and clients never write the ledger directly;
4. **Containment is the only kernel entry for resources**: hash verification (constant-time comparison) precedes extraction, with a compressed-size upper-bound precheck; the `LoadedBundle` capability makes "using an unverified pack" unrepresentable by construction — pinned by trybuild compile-failure cases;
5. **Tasks must not summon tasks**: a task only emits pure-data successor suggestions and never chain-starts them; the production path fail-louds a successor suggestion back to the caller (`contained_task_requires_scheduler`). Scheduler adjudication of successors is a planned next step;
6. **Lab and resource tooling are detachable**: proven by a dependency-graph guard under `--all-features` — no workspace package outside Lab / ActingLab / resource-tooling has any dependency path into them (with feature-gate-bypass counterexamples); resource tooling likewise cannot reach back into the Runtime or the device layer;
7. **Zero game identity**: Runtime-owned code, contracts, and defaults are scanned by an architecture guard banning known project identity terms (game names, package names, server suffixes), with tests in scope; coordinates and thresholds exist only in resource packs, not in runtime code — a design convention, not auto-enforced. The framework recognizes "game shape" (resource pools, pages, tasks), never "game identity". Comparison **algorithms** live in the Runtime; the compared **values** (truth dictionaries etc.) all come from packs — same invariant.

Nine further **completion acceptance invariants** (deterministic replay, zero-side-effect replay, budgeted loops, full recomputation on clock jumps, crash recovery rebuilding the same pending set, no starvation of eligible work, fail-loud on invalid input, unknown never silently treated as false, a complete reason chain for every dispatch) cover the scheduling policy plane; see `docs/architecture/runtime-completion-invariants.md`.

## 📦 Components (workspace members)

**Applications**

| Name | Responsibility |
|---|---|
| `actingd` | Resident daemon process adapter hosting all kernel components below |
| `actingctl` | Production user CLI (observe / status / monitor-* / stream / reset / task-run, with `--recovery-package` auto-repositioning); successful results are single-line JSON |
| `actinglab` | Debug probe + resource authoring (record → draft → build → transactional publish → offline `package dry-run`); **not a production dependency** |
| `device-test` | Device backend diagnostic tool |
| `vision-provider-check` | Vision provider self-check (ABI check / artifact lock / OCR·NN smoke) |
| `actingledger` (`apps/ledger-forensics`) | Read-only GlobalLedger forensics CLI |

**Production kernel**

| Name | Responsibility |
|---|---|
| `runtime-host` | Resident ownership, local typed IPC, lease-gated DeviceProxy, instance facts and policy/budget dispatch, strategic reports, and Dispatcher session lifecycle |
| `runtime-client` | Client-side typed local IPC and project interface v2 paginated read-only projections; neither constructs nor holds production device backends |
| `scheduler` | Per-instance write admission, lease lifecycle and fencing authority |
| `execution-kernel` | Daemon-held execution sessions + pure task/probe decision planning; contained-task timeout, step, and terminal-anchor semantics |
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

**Recognition FFI boundary (wired into the production recognition path; live-verified on CPU)**

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

- **Available (live-verified)**: template matching (NCC family) and color predicates; the OCR production chain — `PP-OCRv6_medium` (ONNX Runtime, CPU, strict no-fallback), per-invocation execution attestation (provider/model/device hashes each time), canonical/alias/tolerant dictionary comparison with bounded retry;
- **Known boundary**: the provider currently has region single-line semantics (one block per target). Roster coverage still needs verification; whole-page multi-block detection (det → per-box rec) awaits implementation and verification, targeting "whole-page reads + overlap dedup";
- **Pending live test**: CUDA execution (closure, Ready manifests, and device ordinal / stable-identity checks are implemented). One passing CPU run does not verify CUDA, whole-page recognition, or complete roster coverage;
- **Not distributed with the repository**: ONNX Runtime native libraries and OCR/NN models; they are materialized per task-local cache by the pinned-source hash-verified official tool, with `apps/vision-provider-check` as the self-check entry.

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

# Read-only single-frame observation (scheduler-admitted; events and frame artifacts ledgered)
actingctl observe --state-root <state-root> --instance <alias>

# Execute a contained task pack (hash verification precedes extraction)
# --expected-sha256 is 64 lowercase hex chars, no `sha256:` prefix
# Optional: declare a recovery pack; if the entry page does not match,
# the runtime autonomously repositions once
actingctl task-run --state-root <state-root> --instance <alias> \
  --package <task.zip> --expected-sha256 <hash> \
  [--recovery-package <recovery.zip> --recovery-expected-sha256 <hash>]
```

On success, `actingctl` writes single-line JSON to stdout, including the official OCR projection where applicable. Argument, connection, and other errors write text to stderr and exit nonzero. Integrations must handle both output streams and the exit code. Both CLIs use hand-written argument parsing and provide **no `--help` / `--version`**.

Every `actingctl` command requires `--state-root`. The parameters consumed by each subcommand are listed below; the [argument parser](./apps/actingctl/src/main.rs) defines current behavior:

| Subcommand | Instance and command parameters |
|---|---|
| `status` / `monitor-status` | Do not accept `--instance` |
| `observe` / `reset` / `monitor-clear` | Require `--instance` |
| `monitor-set` | Requires `--instance`; optional `--interval-ms` (default 30000), `--expect` (default `home`), `--recover` |
| `stream` | Requires `--instance`; optional `--max-frames` (default 1), `--interval-ms` (default 250) |
| `task-run` | Requires `--instance`, `--package`, `--expected-sha256`; recovery parameters `--recovery-package` and `--recovery-expected-sha256` must be supplied together |

Use only the parameters belonging to the selected subcommand; the current parser accepting a known parameter does not imply that the subcommand uses it.

## 🎮 Resource repositories

Game data (recognition templates, navigation graphs, operation and recovery declarations) is versioned independently of the runtime. The following repositories are **currently private**:

- **ActingCommand-Resources-Arknights** — upstream-derived layer from MAA; own layer currently includes: the composite daily+weekly reward-claim chain (live-verified), the operator-roster OCR task pack (four-fix edition, official live PASS; declaring task timeout, terminal anchor page, 16 OCR targets, and a 422-name truth dictionary), the `return_home` recovery baseline (live-verified, frozen, reusable), recruitment and full-entry navigation/operation sets, home-theme detection declarations (full hometheme set), character/material catalogs, recognition and recovery declarations, scheduling declarations (CN server);
- **ActingCommand-Resources-AzurLane** — upstream-derived layer from Alas; own layer: main-screen navigation and full-entry operation sets, full character/equipment catalog templates (Git LFS), recognition and recovery declarations;
- **ActingCommand-Resources-BlueArchive** — upstream-derived layer from BAAH / BAAS (coordinate catalogs and check regions); own layer: daily-claim pilot task, full-entry operation sets, equipment/material catalogs, recognition and recovery declarations.

Each repository uses a two-layer layout: `upstream-derived/` (third-party derived material with licenses and provenance) + `ours/` (own declarative data).

## 🤝 How we collaborate

Development uses branches and pull requests. Reviews rely on an identified source version, observable behavior, and relevant CI results; device verification should identify the backend, resource pack, and execution boundary. The public repository provides Runtime and resource-authoring entry points; reproducing game tasks also requires obtaining or authoring the corresponding resources.

## Conventions & license

- **Clean-room boundary**: the control plane is rewritten against public behavior and protocols; no C/C++ sources in the repository; the only third-party artifact distributed with the repo is `external-tools/maatouch` (Apache-2.0) — see [NOTICE.md](./NOTICE.md);
- **Recognition licensing boundary**: OCR/NN loads external providers dynamically via FFI; models and native libraries are not distributed with the repository;
- **Contribution flow**: branch + PR by default; all required CI must pass before merge;
- **Documentation sync**: `README.md` and `README.en.md` must change in the same batch and stay factually consistent;
- License: **AGPL-3.0-only**.
