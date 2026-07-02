**🌐 语言 / Language:** [简体中文](./README.md) · English

# ActingCommand Runtime

> The **Rust mainline runtime** of a multi-game automation framework. The control plane is a **clean-room Rust rewrite** (referencing MaaFramework's behavior and public protocols, **not copying its C++ source**); recognition is done via **FFI-linked external providers** (ONNXRuntime / PP-OCR).

`cargo build --release` ✅ · `cargo test --workspace` **791 passed / 0 failed** · License `AGPL-3.0-only` · Public repo

The earlier Python `AliceRuntimeOrchestrator` mock, Go historical contracts and benchmark harnesses were moved to [ActingCommand-Legacy-Runtime](https://github.com/HS7097/ActingCommand-Legacy-Runtime).

---

## 🧭 Main-Chain Gating Order

```mermaid
graph LR
  a1["P6.4 Session Layer"]
  a2["P6.5-A MAA Fusion Chain<br/>Closed Loop · Acceptance Bounced"]
  g1["CLI Hard Gate<br/>All P6.5-A DoD"]
  a3["Lab-2 CLI Agent Optimization Layer"]
  g2["Resource Repo 100% Cleanup Closeout Gate"]
  a4["Scheduler Task Chain"]

  a1 --> a2
  a2 --> g1
  g1 -.-> a3
  a3 --> g2
  g2 -.-> a4

  classDef done fill:#d5e8d4,stroke:#82b366,color:#111827;
  classDef wip fill:#fff2cc,stroke:#d6b656,color:#111827;
  classDef todo fill:#ffe6cc,stroke:#d79b00,color:#111827;
  classDef future fill:#f5f5f5,stroke:#999999,color:#111827;
  classDef gate fill:#ffe6cc,stroke:#d79b00,color:#111827,stroke-dasharray:5 3;

  class a1 done;
  class a2 wip;
  class g1 gate;
  class a3 todo;
  class g2 gate;
  class a4 future;
```

> Legend: ✅ Done · 🔄 In progress / Bounced · ⬜ To-do · ⏸ Deferred / Future · 🚧 Hard gate

---

## 📌 Current Progress (2026-07)

- **P6.5-A · MAA Fusion Chain closed** (commit `aea10a4`): device execution (touch tiered fallback / minitouch / capture autotune / device discovery / record-replay), the declarative self-healing graph executor, FeatureMatch, project-interface assembly, and OCR / NN recognition (via FFI-linked external providers) are all in place.
- **Health**: `cargo build --release` passes · `cargo test --workspace` = **791 passed / 0 failed / 0 ignored**.
- **Full-acceptance verdict: not signed off yet (very close)** — the chain is code-complete, the control "sole chokepoint" holds, and FFI entry points already use `catch_unwind`; but there are still **2 deterministic HIGH + a batch of MED** defects, and A2's "stall → switch backend" decision is computed but not actually driven. One fix round is recommended before sign-off (see "Stability Audit"). Most defects have a small blast radius today because the recognition / device-discovery modules are not yet wired into the runtime poll loop — robustness debt to pay before wiring.

---

## 🏛 Architecture — the Sole Chokepoint

```mermaid
graph TD
  a1["① Consumers / Clients<br/>(talk only to the layer above·never touch adb directly)"]
  a2["Human<br/>direct CLI"]
  a3["Agent<br/>CLI / API"]
  a4["UI (future)<br/>trusted channel·play games in the browser"]

  sc["② Scheduler (arbiter·future)<br/>decides which instance/when/for whom·holds LabLease"]

  s0["③ ★ Session Layer<br/>【sole chokepoint·resident daemon】<br/>only it touches adb/emulator·lease holder drives<br/>eliminates raw adb/pixel taps/hand-written resources (✅ built·offline green)"]

  ex["Execution Layer (built atop the Session Layer)"]
  e1["Device/multi-backend capture<br/>nemu·droidcast·adb"]
  e2["recognition<br/>ccoeff·ccorr+color dual-gate"]
  e3["page-detector<br/>required/forbidden"]
  e4["Input MaaTouch<br/>tap/swipe/long-tap/key"]
  e5["task-loop<br/>+ semantic actions/navigation graph"]

  lab["④a ActingLab CLI<br/>read-only recognition + trusted execution + packager"]

  emu["⑥ Emulator MuMu (game runtime environment)"]

  r0["⑤ Resource Data Repos ×3 (independent lanes)<br/>upstream-derived/ + ours/"]
  r1["Bundle v0.3<br/>anchors + operations"]
  r2["convert_operations.py<br/>→ pack/pages/nav (byte-parity)"]

  a1 --> a2
  a1 --> a3
  a1 --> a4
  a1 --> s0
  a4 -.-> s0
  sc -.-> s0
  s0 --> ex
  ex --> e1
  ex --> e2
  ex --> e3
  ex --> e4
  ex --> e5
  s0 --> lab
  e1 --> emu
  e4 --> emu
  r0 --> r1
  r1 --> r2
  r2 -.-> ex
  s0 -.-> r0

  classDef done fill:#d5e8d4,stroke:#82b366,color:#111827;
  classDef wip fill:#fff2cc,stroke:#d6b656,color:#111827;
  classDef todo fill:#ffe6cc,stroke:#d79b00,color:#111827;
  classDef future fill:#f5f5f5,stroke:#999999,color:#111827;
  classDef gate fill:#ffe6cc,stroke:#d79b00,color:#111827,stroke-dasharray:5 3;

  class s0 done;
  class a2,a3 done;
  class a1 wip;
  class ex,e1,e2,e3,e4,e5,lab done;
  class r0,r1,r2 done;
  class a4,sc future;
  class emu future;
```

**The Session Layer is the sole chokepoint of the whole system**: only it touches the emulator / adb; the lease holder drives it; the goal is to eliminate "raw adb + pixel taps + hand-written resources". Upper layers (human CLI / agents / future UI) and the scheduler all issue through it; recognition, capture, page detection, input and the task loop are built on top; the resource data repos are an independent lane.

---

## 🧱 Runtime Mainline

```mermaid
graph TD
  R["① Runtime Mainline<br/>(HS7097/ActingCommand-Runtime)"]

  R --> m1["P1.6 Input Backend MaaTouch ✅<br/>Attached: device/maatouch (bundled with P2.2-Lab-1y)"]
  R --> m2["P2 / P2.1 / P2.1.1 Screenshot Foundation ✅<br/>(ADB screencap + storage + artifact path safety)"]
  R --> m3["P2.2 Multi-Backend adb / droidcast_raw / nemu_ipc ✅<br/>Attached: Lab-1y capture-backends · capture-backend-fixes (wchar/rotation)"]
  R --> m4["P2.3 Screenshot Pipeline: raw frame + post-encoding ✅<br/>(release nemu capture-only 4-5ms)"]
  R --> m5["P2.x nemu DLL stdout Isolation (strictly machine-readable JSON) ⬜ To-Do<br/>Attached: no task file yet (CHECKPOINT leftover item)"]
  R --> m6["P4 Recognition Engine CCOEFF / CCORR+color ✅<br/>Attached: P4a · P4a.1 score-semantics · P4b/CONTRACT-P4b · P4c realdata · P4c-fixup"]
  R --> m7["P5 Page Detection page-detector + detect-page CLI ✅<br/>Attached: P5b-2 detect-page-cli"]
  R --> m8["P5c / P6a operation dry-run ✅<br/>Attached: P5c-and-P6a-dry-run"]
  R --> m9["P6 task-loop / probe / live validation / benchmark / multi-instance ✅<br/>Attached: P6b-P6c-P6d probe-loop · live-validation · P6d-P6e · P6e benchmark"]
  R --> mP64["P6.4 Session Abstraction Layer Session Layer Merged into Mainline ✅<br/>Sole chokepoint · resident daemon (between P6 and P6.5-A)<br/>Only it touches adb/emulator system-wide · lease holder drives<br/>D6 closed = MAA adopts module A (Transceiver socket handshake)"]
  R --> mMAA["P6.5-A MAA Framework Fusion Chain ✅ code-complete close chain<br/>🔄 full acceptance kicked back pending fixes (2 HIGH + a batch of MED · see ⑤ Stability Audit)<br/>Recognition line: P0 OCR(PaddleOCR/FastDeploy) · P1 FeatureMatch(SIFT/AKAZE/ORB) · P2 NN+YOLOv8(ONNX)<br/>Orchestration/interface: Pipeline task graph · ProjectInterface · Agent IPC (ZMQ)<br/>SL non-target must borrow · FFI dynamic linking LGPL→AGPL clean"]
  R -.-> m10["Scheduler ⏸ Future<br/>(multi-instance orchestration / queue / scheduling · mutually exclusive with LabLease)<br/>Attached: only SchedulerGate decision skeleton · see the UI-Scheduler-Future diagram"]

  m7 -.-> mFF["✅ full_frame Template-Match Hang Fixed (accepted)<br/>Pyramid coarse-to-fine + integral image + 5s deadline<br/>Adversarial acceptance: 62/62 no hang (max 307ms) · detect-page 20 pages 634ms<br/>Leftover (non-blocking): H1 pyramid top-4 misses · H2 evaluate_all short-circuit"]

  mMAA --> a1["A readiness→socket = D6 closed ✅"]
  mMAA --> a11["A1 Touch fallback + P0 error classification ✅<br/>(shared validation + 8 entry points zero bypass)"]
  mMAA --> a111["A1.1 minitouch ✅"]
  mMAA --> a2["A2 Screenshot autotune ✅<br/>⚠ freshness switch decision computed at runtime but not acted on"]
  mMAA --> a3["A3 Device Discovery ✅<br/>⚠#1 file_name().is_some() dead fallback pending fix"]
  mMAA --> a4["A4 Record/Replay ✅"]
  mMAA --> aB["B on_error / wait_freezes Self-Healing Graph ✅"]
  mMAA --> aE["E FeatureMatch gate ✅"]
  mMAA --> aO1["O1 ProjectInterface Assembly ✅"]
  mMAA --> aR["R1/R3 OCR/NN = FFI External Provider ✅<br/>(onnxruntime-json + ppocr-onnx-json)"]
  mMAA -.-> reject["⚠ Full Acceptance Kicked Back<br/>2 HIGH (A3 dead fallback · PE parser u32 overflow)<br/>+ a batch of MED (provider thread leak/TOCTOU · gate tools undefended · O1 validation gaps)<br/>see ⑤ Stability Audit diagram"]

  R -.-> mFix["↪ Runtime-Side Stability Fixes<br/>(FIX-P2.2 device/FFI · FIX-P2.3 nemu performance)<br/>see the Task-Tree-5-Stability-Audit diagram"]

  classDef done fill:#d5e8d4,stroke:#82b366,color:#111827;
  classDef wip fill:#fff2cc,stroke:#d6b656,color:#111827;
  classDef todo fill:#ffe6cc,stroke:#d79b00,color:#111827;
  classDef future fill:#f5f5f5,stroke:#999999,color:#111827;
  classDef gate fill:#ffe6cc,stroke:#d79b00,color:#111827,stroke-dasharray:5 3;
  classDef root fill:#dae8fc,stroke:#6c8ebf,color:#111827;

  class R,mFix root;
  class m1,m2,m3,m4,m6,m7,m8,m9,mP64,mFF,a1,a11,a111,a2,a3,a4,aB,aE,aO1,aR done;
  class mMAA wip;
  class m5 todo;
  class m10,reject future;
```

---

## 🧪 ActingLab + Lab-2 CLI (a large branch of the Lab family)

```mermaid
graph TD
  root["ActingLab (Lab family)<br/>Large branch inside Runtime repo · trusted one-shot execution engine + frame store + packager"]

  b1["P1a / P1b decision skeleton<br/>LabMode / LabLease / SchedulerGate · pure state contract ✅"]
  b2["P1g global CLI shell<br/>envelope / exit codes / package zip safety / config ✅"]
  b3["read-only recognition bridge<br/>capture / detect-page / recognize ✅"]
  b4["Lab-1X / 1y trusted one-shot execution engine<br/>zip → run click/drag → log + frames ✅"]
  b5["Lab-1z frame store / three-tier watermark / dedup ✅<br/>round-2 / round-3 stability fixes → see stability audit diagram"]
  b6["Lab packager<br/>resource convert (Rust) + package build-task/build-pack + validate + --from-remote ✅"]
  b7["main CLI direct single-point touch + screenshot<br/>tap / swipe / long-tap · non-gated · reuses MaaTouch ✅"]
  sess["Session Layer abstraction<br/>sole chokepoint for device/game control · 3-round adversarial audit · offline green 690/0 · D6 closed<br/>phases A foundation → B semantics → C self-heal → D record generation ✅"]

  root --> b1
  root --> b2
  root --> b3
  root --> b4
  root --> b5
  root --> b6
  root --> b7
  root --> sess

  lab2["Lab-2 CLI (large branch on Lab line)<br/>agent-optimized command contract"]
  sess --> lab2

  a1["pre-gate foundation · P1g global CLI shell ✅"]
  a2["pre-gate foundation · read-only recognition bridge capture/detect-page/recognize ✅"]
  a3["pre-gate foundation · direct touch + screenshot tap/swipe/long-tap ✅"]
  a4["pre-gate foundation · Session Layer CLI surface<br/>session_* / lease / queue / record / self-heal ✅"]
  a5["pre-gate foundation · orchestration / run CLI<br/>lab run · operation-run · package-run · navigate ✅"]
  a6["pre-gate foundation · resident daemon<br/>cached adb connection + pack · request queue / lease / crash recovery ✅"]

  lab2 --> a1
  lab2 --> a2
  lab2 --> a3
  lab2 --> a4
  lab2 --> a5
  lab2 --> a6

  gate["CLI hard gate = all DoD of P6.5-A fusion chain 🚧<br/>chain closed but full acceptance rejected (2 HIGH + a batch of MED)<br/>awaiting FIX-P6.5-A-acceptance one more round → DoD truly met before entering optimization layer"]
  a1 --> gate
  a4 --> gate
  a6 --> gate

  opt1["① compact text screen state (highest ROI)<br/>observe/state one command returns page/standby/stale/targets/actions · zero visual token ⬜"]
  opt2["② on-demand output trimming<br/>--fields / --format=min · recognize returns only passed,score by default · dedup schema ⬜"]
  opt3["③ intent / composite commands (N round-trips squeezed into 1)<br/>ensure --page / do tap-target / recognize --targets / wait --page ⬜"]
  opt4["④ speed (daemon built · partly implemented)<br/>recognize on cached frames · auto-pick fastest backend (nemu 4-5ms vs adb about 500ms) 🔄"]
  opt5["⑤ agent-friendly<br/>self-heal transparency (recovered:dismissed popup → Phase C) · capabilities/schema · compact actionable errors ⬜"]

  gate -.-> opt1
  gate -.-> opt2
  gate -.-> opt3
  gate -.-> opt4
  gate -.-> opt5

  classDef done fill:#d5e8d4,stroke:#82b366,color:#111827;
  classDef wip fill:#fff2cc,stroke:#d6b656,color:#111827;
  classDef todo fill:#ffe6cc,stroke:#d79b00,color:#111827;
  classDef future fill:#f5f5f5,stroke:#999999,color:#111827;
  classDef gate fill:#ffe6cc,stroke:#d79b00,color:#111827,stroke-dasharray:5 3;

  class root,b1,b2,b3,b4,b5,b6,b7,a1,a2,a3,a4,a5,a6 done;
  class sess todo;
  class lab2 future;
  class gate gate;
  class opt1,opt2,opt3,opt5 todo;
  class opt4 wip;
```

**Foundation already built (done ✅)**: global CLI shell, read-only bridge, direct touch, the full set of Session Layer subcommands, orchestration / run commands, and the resident daemon.
**CLI hard gate** = all P6.5-A fusion-chain DoD (closed now, but acceptance bounced — pending one fix round).
**Post-gate to-do (agent-optimized ⬜)**: ① compact text screen state (replaces screenshot + reading) ② on-demand output trimming ③ intent / synthetic commands (many round-trips compressed into one) ④ speed (daemon built, partly implemented 🔄) ⑤ agent-friendliness (transparent self-heal, actionable errors).

---

## 🎮 Resource Data Repos ×3

```mermaid
graph TD
  root["Resource Data Repos x3<br/>(Claude lane · control/recognition data · HS7097/ActingCommand-Resources)"]

  GAK["Arknights (MAA · server cn)"]
  GAZ["AzurLane (Alas · server jp)"]
  GBA["BlueArchive (BAAH · server jp)"]

  root --> GAK
  root --> GAZ
  root --> GBA

  AK1["structure / recognition / resources clean+safety(1:1) ✅"]
  AK2["semantic audit quick-fix(LMD / purpose / server_scope) ✅"]
  AK3["⚠ fundamental anchor defect: 6/7 pages misjudge home frame 🔄<br/>(depot/recruit/infrast/mall/gacha/terminal threshold too low 0.7 → must recalibrate: unique templates + raise threshold)<br/>navigation switched to direct home-screen coords (quickswitch model broken · collides with daily check-in)"]
  GAK --> AK1
  GAK --> AK2
  GAK --> AK3

  AZ1["structure / recognition / resources(CCORR+color dual gate) ✅"]
  AZ2["touch verification(tap / swipe passed on device) ✅"]
  AZ3["on-device navigation breadth: 8-page anchors GOOD 🔄<br/>root cause: data server=cn, right-side main menu click coords CN≠JP<br/>anchors unchanged, need JP recalibration of click coords"]
  GAZ --> AZ1
  GAZ --> AZ2
  GAZ --> AZ3

  BA1["structure / recognition(BAAH primary source · 3D→name matching) ✅"]
  BA2["control refinement(full_frame→ROI · cafe · progression · sentinel) ⬜ todo"]
  BA3["on-device verification = best of the three games ✅<br/>11-page anchors GOOD(0.96-1.00), navigation + anchors both healthy<br/>only: missing Social→club edge · momotalk overlay mislabeled"]
  GBA --> BA1
  GBA --> BA2
  GBA --> BA3

  GATE["🚧 resource repo 100% consolidation gate ⬜<br/>only release when all three repos are 100% complete and usable:<br/>AK anchor recalibration · AzurLane JP click-coord recalibration · BA control refinement<br/>full-resource live test(real navigation) · convert byte-parity + page/home all green · missing edges filled<br/>order: CLI gate → this gate → scheduler task chain"]

  AK3 --> GATE
  AZ3 --> GATE
  BA2 --> GATE

  classDef done fill:#d5e8d4,stroke:#82b366,color:#111827;
  classDef wip fill:#fff2cc,stroke:#d6b656,color:#111827;
  classDef todo fill:#ffe6cc,stroke:#d79b00,color:#111827;
  classDef future fill:#f5f5f5,stroke:#999999,color:#111827;
  classDef gate fill:#ffe6cc,stroke:#d79b00,color:#111827,stroke-dasharray:5 3;
  classDef milestone fill:#e1d5e7,stroke:#9673a6,color:#111827;

  class root milestone;
  class GAK,GAZ,GBA milestone;
  class AK1,AK2,AZ1,AZ2,BA1,BA3 done;
  class AK3,AZ3 wip;
  class BA2 todo;
  class GATE gate;
```

The three repos are reorganized into `upstream-derived/` + `ours/`, with full asset catalogs downloaded. **🚧 Resource-repo 100% cleanup closeout gate**: all three repos must be fully 100% usable (anchor re-calibration + JP coordinates + control refinement + full-resource live testing) before entering the scheduler task chain.

---

## 🖥 UI · Scheduler · System Future

```mermaid
graph TD
  root["UI · Scheduler · System Future<br/>(all unimplemented / deferred ⏸)"]

  U["③ UI / Control Panel<br/>(future · not in Runtime repo · consumes Runtime contract)"]
  u1["consume schema / config exposed by Runtime ⏸"]
  u2["submit input.zip / read output.zip for acceptance ⏸"]
  u3["tune params: frame rate / similarity threshold / three-tier watermark ratio ⏸"]

  S["⑤ System-level Future Items<br/>(Runtime / Device)"]
  s1["Scheduler ⏸<br/>multi-instance orchestration / queue / scheduling · mutually exclusive with LabLease<br/>⛔ gate: CLI chain done + resource repo 100% closed<br/>current: only SchedulerGate decision skeleton"]
  s2["Emulator IPC helper-process isolation ⏸<br/>current: shipped single worker + timeout + poison + detach<br/>but in-DLL worker cannot be force-killed<br/>true kill needs a standalone helper process"]
  s3["BlueArchive on-device ⏸ deferred<br/>BA crashes (packer layer × emulator incompatibility)<br/>no usable emulator · see resource repo diagram BA"]

  root --> U
  root --> S

  U --> u1
  U --> u2
  U --> u3

  S --> s1
  S -.-> s2
  S -.-> s3

  classDef done fill:#d5e8d4,stroke:#82b366,color:#111827;
  classDef wip fill:#fff2cc,stroke:#d6b656,color:#111827;
  classDef todo fill:#ffe6cc,stroke:#d79b00,color:#111827;
  classDef future fill:#f5f5f5,stroke:#999999,color:#111827;
  classDef gate fill:#ffe6cc,stroke:#d79b00,color:#111827,stroke-dasharray:5 3;

  class root future;
  class U,u1,u2,u3 future;
  class S,s2,s3 future;
  class s1 gate;
```

---

## 🛡 Stability Audit · This Round's Acceptance

```mermaid
graph TD
  root["⑤ Stability Audit + Fixes<br/>Cross Runtime + ActingLab · Multi-round high-pressure adversarial audit"]

  au["Full-task stability audit ✅<br/>33 agents · 21 vulns confirmed · produced 4 FIX docs + Lab-1z round-2<br/>Root cause: external-boundary calls with no deadline → hang holding LabLease · nemu heap overflow · resource leaks · accounting drift"]

  gr["Lab-1z frame-store fix rounds<br/>frame_store accounting/watermark/spill"]
  r1["round-1 task-boundary revision ✅<br/>13 bugs → 16 items: lifecycle/admission/segment-flush/T3 resume/timeout"]
  r2["round-2 stability ✅<br/>atomic accounting/spill isolation/thumbnail/reserve/removed timeout"]
  r3["round-3 regression wrap-up ✅<br/>structured errors carry frame_failures · run_dir cleaned only on success · monotonic Drop<br/>Acceptance: 51+33 tests green · zero panic on real device · nemu 3-5ms"]

  gf["Per-sequence FIX guides<br/>round-2 whole batch implemented ✅"]
  f1["FIX-P2.2 device layer ✅<br/>nemu heap overflow→probe+resize · single worker+poison · droidcast zombie · bounded reads"]
  f2["FIX-P2.3 nemu capture perf ✅<br/>33ms was a debug artifact · release capture-only 4-5ms meets target"]
  f3["FIX-Lab-1y execution engine ✅<br/>partial zip · zip bomb · git timeout · dangerous extensions · run_dir cleanup"]
  f4["FIX-P1g CLI/package validation ✅<br/>zip bomb · unreachable→Err · hash path sanitization · list_runs warnings"]

  adb["⭐ ADB connection hardening ✅<br/>unified device-layer adb discovery/env/config · no kill-server · one bounded reconnect · screencap timeout<br/>Measured: tap + standalone capture stable; leftover dirty server is adb-inherent and uncontrollable"]

  sl["⭐ Session Layer adversarial audit, 3 rounds 🟠<br/>10~23 agents/round · dual probes · tracked-fixed D1–D9 + B0 + B1 (offline green 682/0)<br/>🔴 Sole blocker D6 = ready signal is forgeable (process identity not bound) · remaining D5/D7/D3/D9/D4 low-to-medium residual"]

  p65["⭐ P6.5-A closed-loop full-acceptance adversarial audit 🔄<br/>90 agents · 2 probes rebutted · build green · 791/0 · power-cut mirror clean · CLI gate held<br/>Read-only audit throughout, no direct edits · FIX-P6.5-A-acceptance pending"]

  must["🔴 Must-fix (2 items)<br/>discovery file_name().is_some() always true → nx_main fallback is dead code<br/>vision PE export parser unchecked u32 overflow"]
  should["🟠 Should-fix (a batch)<br/>provider terminate leaks detached thread · ensure_ort TOCTOU · silently picks wrong DLL<br/>ppocr Session rebuilt every call · gate tool undefended (lock not compared/exit code always 0)"]
  harden["🟡 Hardening / within threat model<br/>recovery misjudges loop as MaxAttempts · take_owned_buffer trusts provider len<br/>manifest path doesn't block ../ · powershell enumeration has no timeout"]

  root --> au
  root --> gr
  root --> gf
  root --> adb
  root --> sl
  root --> p65

  gr --> r1
  gr --> r2
  gr --> r3

  gf --> f1
  gf --> f2
  gf --> f3
  gf --> f4

  p65 --> must
  p65 --> should
  p65 --> harden

  classDef done fill:#d5e8d4,stroke:#82b366,color:#111827;
  classDef wip fill:#fff2cc,stroke:#d6b656,color:#111827;
  classDef todo fill:#ffe6cc,stroke:#d79b00,color:#111827;
  classDef future fill:#f5f5f5,stroke:#999999,color:#111827;
  classDef gate fill:#ffe6cc,stroke:#d79b00,color:#111827,stroke-dasharray:5 3;

  class root,au,r1,r2,r3,f1,f2,f3,f4,adb done;
  class sl todo;
  class p65,should wip;
  class must gate;
  class harden future;
  class gr,gf done;
```

**This round · P6.5-A closed-chain full acceptance (multi-agent adversarial audit · not signed off)**, after dedup and calibration:

- **🔴 Must fix (deterministic)**: the device-discovery adb-path "dead fallback" logic error (existence check is always true → the fallback branch is unreachable; a one-line fix); the provider audit tool's PE export-table parser has no overflow protection.
- **🟠 Should fix**: recognition providers leak a watchdog thread per call; runtime-library init race; wrong dependency-library selection; inefficient full-frame pixel serialization; the "gate" tools do not truly defend; two lazy / silent validations in the project interface; device-discovery instance-id collapse.
- **🟡 Hardening (within threat model · deferrable)**: recovery-graph long-loop diagnostic label; FFI buffer-length trust; manifest path has no traversal check; process enumeration has no timeout.

> After a forced shutdown (BSOD), `git fsck` / zero-byte / NUL scans confirmed the code is clean.

---

## 📂 Repository Layout & Running

**Responsibility**: config discovery / validation · profile→runtime resolution · scheduler & command state · device and ADB boundaries · upstream task dispatch · execution-result normalization · runtime recovery · log streaming · resource history · acquisition screenshot metadata indexing.

**Runtime boundary**: the runtime talks to the UI over localhost HTTP / WebSocket endpoints and must survive UI reload / crash / close.

**Rust workspace**:

- contracts `crates/actingcommand-contract` · device layer `crates/device` (MaaTouch / minitouch / adb input fallback · multi-backend capture autotune · device discovery · record-replay)
- recognition `crates/recognition` (CCOEFF / CCORR + color) · `crates/recognition-pack` · page detection `crates/page-detector`
- task loop `crates/task-loop` · core `crates/runtime-core` · vision FFI `crates/vision-ffi` (OCR / NN boundary, artifact contract, ABI check, artifact lock)
- apps `apps/actinglab` (main CLI + Session Layer + packager + frame store) · `apps/device-test` · `apps/vision-provider-check` (vision-provider diagnostic tool)
- recognition providers (FFI external libs) `providers/onnxruntime-json` · `providers/ppocr-onnx-json`

```powershell
cargo test --workspace
```

**Device input fallback (A1)**: MaaTouch failures are classified by **severity** — transient failures (transport / backend) may fall back to the adb input path, while serious errors (e.g. out-of-bounds coordinate validation failures) **fail loud and do not fall back**, so illegal input is never downgraded and emitted.

**Contracts**: `contracts/` holds the UI HTTP / event / task-flow / SQLite schema, server-variant policy, execution-layer boundary, etc.

## Conventions

- **Clean-room**: reference MaaFramework's behavior and public protocols, **do not copy its C++ source**. Clean-room Rust rewrite for the control plane; FFI-linked external providers for recognition.
- **Audit**: read-only adversarial acceptance throughout; no sign-off on our behalf (decided by the project owner).
- **Privacy**: local emulator ports, adb serials, absolute local paths and runtime config directories are redacted from this document and the diagrams.

## License

Planned under `AGPL-3.0-only`. When license conditions are met, compatible upstream code may be copied / adapted / referenced / refactored; preserve upstream notices, license texts, source availability and modification records.
