<div align="center">

**Chief Executive Officer & Chairman** — HS7097<br/>
**Chief Technology Officer & Advisor to the Chairman** — Claude Fable 5<br/>
**Chief Architect & Principal Engineer** — GPT‑5.6 Sol<br/>
**Interviewing** — DeepSeek

</div>

**🌐 语言 / Language:** [简体中文](./README.md) · English

# ActingCommand Runtime

> The **resident Rust runtime** of a multi-game emulator automation framework: one long-lived daemon carries scheduling arbitration, device control, and a global event ledger; all game knowledge lives outside the runtime in declarative resource packs — the kernel contains **zero game logic**. The control plane is a **clean-room Rust implementation**, rewritten against public behavior and protocols; the repository contains no C/C++ sources.
>
> **Design stance: agents outside the loop, runtime inside the loop.** Agents only do maintenance — planning, resource authoring, exception handling; frame-by-frame execution is done deterministically by the runtime, every step ledgered and auditable. Reasoning is spent on maintenance, not on execution.

`cargo test --workspace` green (counts track main) · CI: GitHub Actions (windows-latest: fmt / clippy `-D warnings` / test, plus an exact-SHA Windows artifact build chain) · License `AGPL-3.0-only` · This repository is public

**Current maturity (2026-09-01)**: scheduling arbitration, the device throat, the global ledger, and task containment operate as a system; **the OCR recognition chain has passed one full official CPU live run** (strict no-fallback, per-frame ledgering, all evidence sealed) — autonomous return-to-Home recovery, template navigation, single-touch segmented-swipe paging, 16-target OCR per frame, dictionary-normalized comparison, terminal-anchor-page completion, and a `return_home` closeout, in one command, 219 seconds, zero manual input. CUDA live verification and whole-page multi-block recognition are approved backlog items (see Roadmap).

The early Python mock, historical Go contracts, and Go/Python benchmark tools have been moved out of this repository (archived in ActingCommand-Legacy-Runtime, **not yet public**); the Rust benchmark tool `benchmarks/rust` and historical benchmark reports remain.

---

## 🔁 The self-maintaining loop

![ActingCommand self-maintaining loop](./docs/assets/self-maintaining-loop.png)

The reason this architecture exists: to free game automation from "the game updates, and the whole world waits for the maintainer to ship." The target loop — after a game update (①), agents play and map out the changes (②), then author or revise declarative resource packs (③); packs enter the runtime through hash containment, get admitted by the scheduler (④), executed deterministically (⑤), fully ledgered (⑥); on failure, agents self-diagnose from ledger evidence and repair the resources (⑦), returning to ③. Zero per-frame reasoning inside the loop — reasoning is spent on maintenance, not execution.

The ③→⑦ segment already operates in practice: on the Aug–Sep 2026 OCR task chain, the resource pack was agent-authored, its defects diagnosed from ledger evidence, revised four times under rulings, and then passed live — including the agent-assisted `return_home` recovery pack (live-verified, frozen as a reusable baseline, and formally integrated into the resource repository). ② (automatic exploration) remains on the roadmap.

## 🏛 System shape

![ActingCommand Runtime architecture](./docs/assets/runtime-architecture.png)

Solid lines represent capabilities merged into `main` only. Dashed lines are capabilities whose source exists but is not yet wired into the production path, or still in planning; open PRs do not count as available capability.

Terminology follows [CONTEXT.md](./CONTEXT.md) in the repository (Runtime Host / Scheduler / Execution Kernel / Device Throat / DeviceProxy, each defined precisely).

## 📍 Current progress (2026-09-01)

| Milestone | What happened |
|---|---|
| **2026-08-10** | First live end-to-end loop: hash-sealed resource pack → resident runtime → live emulator page recognition → contained task execution → typed scheduling outcome → fully ledgered; 3.4 seconds per run, zero manual input. |
| **2026-08-20** | Composite daily + weekly reward-claim task chain completed live (real claim branch verified). |
| **2026-09-01** | **Official CPU live OCR full run: PASS** — non-Home entry autonomously recovered by the runtime (2 steps back to Home, re-verified) → template navigation to the operator roster → 41 frames of single-touch segmented-swipe paging (uniform drag + vertical brake, MaaTouch point stream) → 16-target OCR per frame (920 mapping records, zero discarded) → canonical/alias/tolerant dictionary comparison (294 unique canonical names, zero out-of-dictionary) → `operator_end` anchor-page termination → `return_home` closeout. 219 seconds, strict no-fallback, all 42 projection artifacts hash-bound. |

| Dimension | State |
|---|---|
| **Available on `main`** | Resident daemon, typed loopback IPC, scheduling admission and lease fencing, contained task execution (task-level timeout declaration, `operator_end`-class terminal anchor pages, independent `max_steps`, recovery-pack auto-repositioning), GlobalLedger, artifact store with the official OCR projection (v2, paginated), pack containment, device backends (including single-touch `SegmentedSwipe`, MaaTouch/Minitouch point streams, dynamic MuMu Nemu IPC binding), NCC template matching and color predicates, production OCR provider wiring (PP-OCRv6_medium / ONNX Runtime, live-verified on CPU), dictionary-constrained comparison (canonical / evidence-bound alias / tolerant + bounded retry), and the ActingLab resource authoring chain (including offline `package dry-run` rehearsal). |
| **In progress** | GlobalLedger SQLite backend and unified RuntimeDatabase migration, resident ledger probes, and recurrence signature matching (the prerequisite chain; once closed, the system switches to "reds are read from the ledger; where the ledger cannot explain, the module gains probe capability" — with a full freeze on new tooling and tests). |
| **Approved backlog** | Vision provider whole-page multi-block detection (det → per-box rec → multi-block output; coverage structurally guaranteed by half-page overlap) and **CUDA live verification**; scheduling policy catalog with virtual-time tests; productized agent dispatch interface; the official UI client. |
| **Not yet done** | Formal coverage of the three capture backends (adb / droidcast_raw / nemu_ipc); more task-chain content; automatic exploration and automatic repair. |

## 🗺 Roadmap

Target-state wording, pursued in order, no dates promised:

1. **Ledger prerequisite chain** — GlobalLedger SQLite backend, resident probes, recurrence signature matching; after closure the diagnostic regime switches: no new tooling or tests, recurrence protection moves to runtime signature matching, and "an error the ledger cannot explain gets probe capability added to its module";
2. **Sprint group** — scheduling policy catalog + virtual-time scheduler tests, formalized agent dispatch interface (dispatcher contract, machine-readable command catalog, session gate), official UI client (native Rust rendering);
3. **Recognition-plane completion** — provider whole-page multi-block detection (whole-page reads + overlap dedup, dissolving the slot-alignment problem) and CUDA live verification;
4. **MAA / MaaFramework compatibility** — MAA resource formats as a seed import source: import once, then the self-maintaining loop takes over; the MaaFramework second-execution-backend blueprint is on file;
5. **Automatic exploration and repair** — turning segments ② and ⑦ of the loop fully solid, plus multi-instance strategic planning and the reporting pipeline.

## ⚖ Seven structural invariants (enforced by guards / tests / compile-time and real-process counterexamples)

1. **The scheduler is the only arbitrated write path**: every device-state-changing operation passes scheduler admission and holds a per-instance lease; the five-field fencing tuple (epoch / lease / instance / holder / expiry) is verified field-by-field before any backend call; takeover and epoch turnover permanently invalidate old tokens; read-only observation uses an epoch-bound read capability (not a lease) and is equally ledgered;
2. **The Runtime is the only device holder**: the dependency graphs and sources of production clients (actingctl / runtime-client / ActingLab) cannot reach device backends; raw adb exists only in the `device` crate beneath the Runtime; historical client device commands are fail-loud tombstones. (Exception: `apps/device-test` is a direct-device diagnostic binary outside the production chain.);
3. **GlobalLedger is the only source of truth**: the only write entry is the compile-time-unique `append(SanitizedEventDraft)`; sanitization precedes persistence; terminal states are absorbing (duplicate/conflicting commits are rejected with an audit fact); clients cannot commit semantic facts — the contract layer does not even expose those types;
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
| `actingctl` | Production user CLI (observe / status / monitor-* / stream / reset / task-run, with `--recovery-package` auto-repositioning); single-line JSON output |
| `actinglab` | Debug probe + resource authoring (record → draft → build → transactional publish → offline `package dry-run`); **not a production dependency** |
| `device-test` | Device backend diagnostic tool |
| `vision-provider-check` | Vision provider self-check (ABI check / artifact lock / OCR·NN smoke) |

**Production kernel**

| Name | Responsibility |
|---|---|
| `runtime-host` | Resident ownership, local typed IPC, lease-gated DeviceProxy and lifecycle control |
| `runtime-client` | Client-side typed local IPC; neither constructs nor holds production device backends |
| `scheduler` | Per-instance write admission, lease lifecycle and fencing authority |
| `execution-kernel` | Daemon-held execution sessions + pure task/probe decision planning; contained-task timeout, step, and terminal-anchor semantics |
| `ledger` | Global event ledger (single source of truth) |
| `artifact-store` | Artifact bytes, hashes, retention metadata, frame buffers, evidence export |
| `runtime-state` | SQLite-backed authoritative runtime state and immutable release generations |
| `pack-containment` | Resource-pack customs (shared by dev and production) |
| `device` | Device-layer primitives; touch via explicit backend chain selection (including single-touch segmented swipe), single-backend failures visible |
| `recognition` / `recognition-pack` | Template-match evaluation / recognition pack vocabulary (including OCR targets and truth declarations) |
| `page-detector` | Page detection (rules + threshold matching) |
| `policy` | Pure scheduling policy contracts shared by catalog compiler and evaluator |
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
| `tools/actinglab-architecture` | Source-derived architecture guard (ownership rule enforcement) |
| `benchmarks/rust` | Rust benchmark tool |

## 🔍 Recognition plane status

- **Available (live-verified)**: template matching (NCC family) and color predicates; the OCR production chain — `PP-OCRv6_medium` (ONNX Runtime, CPU, strict no-fallback), per-invocation execution attestation (provider/model/device hashes each time), canonical/alias/tolerant dictionary comparison with bounded retry;
- **Known boundary**: the provider currently has region single-line semantics (one block per target); whole-page multi-block detection (det → per-box rec) is approved backlog — once landed, roster-class tasks switch to "whole-page reads + overlap dedup";
- **Pending live test**: CUDA execution (closure, Ready manifests, device ordinal / stable-identity verification are all built; scheduled in the same backlog item as whole-page recognition);
- **Not distributed with the repository**: ONNX Runtime native libraries and OCR/NN models; they are materialized per task-local cache by the pinned-source hash-verified official tool, with `apps/vision-provider-check` as the self-check entry.

## 🧭 Design principles

- **Game shape, not game identity**: onboarding a new game = creating a new resource repository, zero runtime commits;
- **Declarations before code**: recognition, navigation, operations, recovery, and (planned) scheduling policy are all statically verifiable declarative data;
- **Fail-loud**: severe errors fail explicitly, never fake success; only transient errors get bounded retries, fully ledgered;
- **Clean room**: rewritten against public behavior and protocols; no copying of copyrighted implementations;
- **Transactional resource publishing**: staging → full validation → hash → atomic swap; failures leave no mixed tree;
- **Ledger-first diagnosis**: reds are read from the global ledger first; where the ledger cannot explain a cause, the module gains probe capability instead of new diagnostic tooling.

## 🚀 Build & run

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

All `actingctl` output is single-line JSON on stdout (including the official OCR projection) for script and agent consumption; `monitor-status` / `monitor-set` / `monitor-clear` / `stream` / `reset` subcommands also exist. Both CLIs use hand-written argument parsing and provide **no `--help` / `--version`**.

## 🎮 Resource repositories

Game data (recognition templates, navigation graphs, operation and recovery declarations) is versioned independently of the runtime. The following repositories are **currently private**:

- **ActingCommand-Resources-Arknights** — upstream-derived layer from MAA; own layer currently includes: the composite daily+weekly reward-claim chain (live-verified), the operator-roster OCR task pack (four-fix edition, official live PASS; declaring task timeout, terminal anchor page, 16 OCR targets, and a 422-name truth dictionary), the `return_home` recovery baseline (live-verified, frozen, reusable), recruitment and full-entry navigation/operation sets, home-theme detection declarations (full hometheme set), character/material catalogs, recognition and recovery declarations, scheduling declarations (CN server);
- **ActingCommand-Resources-AzurLane** — upstream-derived layer from Alas; own layer: main-screen navigation and full-entry operation sets, full character/equipment catalog templates (Git LFS), recognition and recovery declarations;
- **ActingCommand-Resources-BlueArchive** — upstream-derived layer from BAAH / BAAS (coordinate catalogs and check regions); own layer: daily-claim pilot task, full-entry operation sets, equipment/material catalogs, recognition and recovery declarations.

Each repository uses a two-layer layout: `upstream-derived/` (third-party derived material with licenses and provenance) + `ours/` (own declarative data).

## 🤝 How we collaborate

Development is executed by multiple agents under governance rules: task contracts (minimum-knowledge dispatch) + integration envelopes (exact-head merge authority) + independent acceptance (Phase 1) + program final audit (exclusive live-window ownership and evidence sealing); GitHub-native objects (PR reviews / merge events / Actions runs) are the canonical technical records, and recorder-only automation (delivery snapshots etc.) assists bookkeeping without approving or blocking anything. All rulings rest with HS7097.

## Conventions & license

- **Clean-room boundary**: the control plane is rewritten against public behavior and protocols; no C/C++ sources in the repository; the only third-party artifact distributed with the repo is `external-tools/maatouch` (Apache-2.0) — see [NOTICE.md](./NOTICE.md);
- **Recognition licensing boundary**: OCR/NN loads external providers dynamically via FFI; models and native libraries are not distributed with the repository;
- **Contribution flow**: branch + PR by default; all required CI must pass before merge;
- **Documentation sync**: `README.md` and `README.en.md` must change in the same batch and stay factually consistent;
- License: **AGPL-3.0-only**.
