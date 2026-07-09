# ActingLab A2a Interface Freeze

Status: frozen for issue #33 A2b-A9 implementation  
Parent specification: `TASK-lab-extraction-chain.md` frozen v2  
Parent specification SHA-256: `efb9e37f10807ce2a615205e3924021ad91eb073a54e4c65cd178e14b0aeab3b`  
Runtime implementation baseline: `83520c5ac36183be7c73200ab3cefe02bb3962c2`

This document freezes the interface decisions required by A2a. It does not change Runtime behavior. Any semantic change to these decisions requires an explicit amendment and a new frozen hash before implementation continues.

## 1. Decision Summary

1. Strengthen the existing `crates/actingcommand-contract` in place. Do not create a second `lab-contract` crate.
2. Create `crates/lab` with package name `actingcommand-lab` as the application core.
3. Keep process parsing, stdout/stderr, human formatting, and process exit-code mapping in `apps/actinglab`.
4. Keep semantic error codes and serialized protocol DTOs in `actingcommand-contract`.
5. Reuse the deep `actingcommand-device::InputBackend` and `actingcommand-device::CaptureBackend` traits behind factories. Do not create competing input or capture operation traits.
6. Define only the missing application ports in `crates/lab`: backend factories, ledger sink, clock, and config source.
7. A CLI invocation constructs one `Lab` instance. A future resident `actingd` process will construct one long-lived `Lab` instance and expose the same typed use-case methods.
8. A2b starts with typed load-bearing protocol DTOs and pipeline facilities. Long-tail payloads may remain typed wrappers around `serde_json::Value` until their use-case family migrates.
9. I5a is the only ownership claim in this chain. I5b resident-process ownership remains future `actingd`/scheduler work.

## 2. Existing Contract Inventory

### 2.1 `primitive.rs`

Current `PrimitiveLayer` methods:

| Existing method | Current role | A2a decision |
| --- | --- | --- |
| `connect_device` | Profile/device session creation | Retain as external/delegated engine boundary. Not used as the in-process Lab hot path. |
| `start_app` / `stop_app` | App lifecycle | Retain for future engine adapters. No Lab port added in this chain unless a migrated use case requires it. |
| `capture` | Returns persisted `CaptureRef` | Retain for external protocol use. Lab immediate capture reuses `CaptureBackend`, which returns an in-memory `Frame`; persistence remains a separate application effect. |
| `match_templates` | Recognition service request | Retain for delegated engines. In-process Lab use cases call existing recognition deep libraries directly. |
| `ocr` | OCR service request | Retain; OCR is outside issue #33. |
| `get_color` | Color probe service request | Retain for delegated engines. In-process recognition continues through the recognition evaluator. |
| `tap` / `swipe` | Remote primitive input | Retain for external protocol use. In-process Lab reuses `InputBackend`. |
| `wait_for` | Generic delegated wait | Retain; no duplicate generic wait port is added. Use cases receive `Clock` and bounded sleep through context policy. |

Associated request/response models are retained for compatibility. They are not deleted or silently repurposed during A2b-A8b.

### 2.2 `game_engine.rs`

`GameEngine` is an upstream/delegated engine contract for profile, runtime lifecycle, scheduler, logs, resource history, and acquisition queries.

A2a decision:

- retain the trait and DTOs;
- do not make `GameEngine` the Lab application API;
- future adapters may implement `GameEngine` by calling a resident Lab service, but CLI extraction use cases must call typed `Lab` methods directly;
- `start`/`stop`/`restart`/`refresh` remain runtime lifecycle vocabulary and do not become process exit semantics.

### 2.3 `taskflow.rs`

`TaskFlow`, `TaskDefinition`, `TaskStep`, `FailurePolicy`, and `TaskParamValue` describe declarative automation flows.

A2a decision:

- retain them as protocol vocabulary;
- do not force current package/operation JSON into these types during zero-behavior-change migration;
- use them only when an existing use case already consumes that contract or a later schema decision explicitly maps package data to it.

### 2.4 `types.rs`

Current shared types cover runtime profile/status, resources, acquisitions, logs, scheduler summaries, keys, resolution, context, and `RuntimeError`.

A2a decision:

- retain compatibility types and server constants;
- add Lab protocol DTOs in a new `lab.rs` module rather than overloading UI/runtime profile models;
- keep `RuntimeError` for the existing engine contract;
- add a separate `LabError` semantic error model whose stable code is independent from CLI exit code.

### 2.5 Current Consumers

At the A2a baseline, Cargo metadata shows:

- `crates/runtime-core` depends on `actingcommand-contract`;
- `runtime-core` is a deprecated prototype and is not an application-core foundation;
- `capture_store.rs` consumes `CaptureRef` and `Resolution`;
- no current application binary consumes `actingcommand-contract` directly.

This supports in-place contract strengthening without creating a second protocol crate.

## 3. Contract Crate Fate

### 3.1 Package

- Path: `crates/actingcommand-contract`
- Package: `actingcommand-contract`
- Rust crate: `actingcommand_contract`
- Fate: retained and promoted as the sole shared protocol/DTO crate.

### 3.2 Dependency Budget

The direct dependency set remains a subset of:

- `serde`
- `serde_json`
- `thiserror`

No device, filesystem, network, application, or deep recognition crate may be added to the contract crate.

### 3.3 New Contract Module

A2b adds `crates/actingcommand-contract/src/lab.rs` and re-exports it from `lib.rs`.

The first stable surface is:

```rust
pub const CLI_SCHEMA_VERSION: &str = "0.2";

pub struct Envelope<T> {
    pub schema_version: String,
    pub cli_version: String,
    pub runtime_version: String,
    pub ok: bool,
    pub command: String,
    pub data: Option<T>,
    pub error: Option<EnvelopeError>,
    pub run_id: Option<String>,
    pub artifacts: Option<serde_json::Value>,
}

pub struct EnvelopeError {
    pub code: String,
    pub message: String,
    pub blocked_by: Vec<String>,
    pub details: Option<serde_json::Value>,
}

pub struct LabError {
    pub class: LabErrorClass,
    pub code: String,
    pub message: String,
    pub blocked_by: Vec<String>,
    pub details: Option<serde_json::Value>,
}
```

`Envelope<T>` serialization must preserve the A1 golden field names and omission behavior. `CLI_SCHEMA_VERSION` stays `0.2` throughout issue #33.

### 3.4 Semantic Error Classes

```rust
pub enum LabErrorClass {
    UsageValidation,
    SafetyBlocked,
    DeviceInstance,
    RuntimeUnavailable,
    NotImplemented,
}
```

Stable string error codes remain data, not enum variant names. Initial constructors preserve current codes such as:

- `validation_failed`
- `package_invalid`
- `target_not_visible`
- `instance_not_found`
- `device_error`
- `runtime_not_running`

The CLI adapter owns the only process mapping:

| `LabErrorClass` | Exit code |
| --- | ---: |
| `UsageValidation` | 2 |
| `SafetyBlocked` | 3 |
| `DeviceInstance` | 4 |
| `RuntimeUnavailable` | 5 |
| `NotImplemented` | 6 |

No exit-code field or method is added to the contract crate.

## 4. Load-Bearing DTO Freeze

A2b-A3 must add the following protocol DTOs before migrated use cases consume them.

### 4.1 Environment DTOs

```rust
pub struct EnvDetected {
    pub key: String,
    pub value: String,
    pub confidence: f32,
    pub source: String,
    pub detector_id: String,
    pub detected_at_unix_ms: u64,
}

pub struct EnvResolved {
    pub key: String,
    pub value: String,
    pub confidence: f32,
    pub source: String,
    pub detector_id: String,
    pub source_result: String,
}

pub struct NeedsDetection {
    pub status: String,
    pub reason: String,
    pub command: Option<String>,
    pub subject: Option<String>,
    pub detector_ids: Vec<String>,
    pub keys: Vec<EnvResolved>,
    pub recommended_action: String,
}
```

Use-case-specific wrappers may add existing fields, but these load-bearing facts must not be rebuilt with ad hoc `json!` objects after A3.

### 4.2 Ledger Drive DTOs

```rust
pub enum DriveStage {
    Request,
    Recognition,
    EnvDetected,
    EnvResolved,
    EnvNeedsDetection,
    Planned,
    Executed,
    Finalizing,
}

pub struct DriveRecord<T> {
    pub stage: DriveStage,
    pub command: String,
    pub req_id: String,
    pub payload: T,
}
```

Serialization names must match current ledger stage strings. A long-tail `DriveRecord<serde_json::Value>` is permitted during migration; public Lab use-case responses may not expose raw `Value`.

### 4.3 Arbitration DTOs

```rust
pub struct LeaseGrant {
    pub schema_version: String,
    pub lease_id: String,
    pub req_id: String,
    pub instance: String,
    pub holder: String,
    pub holder_pid: u32,
    pub priority: String,
    pub acquired_at_ms: u64,
    pub updated_at_ms: u64,
    pub alive: bool,
    pub destructive_step_active: bool,
    pub preempt_requested: bool,
}

pub enum ArbitrationState {
    ReadonlyAccepted,
    LeaseGranted,
    Recovering,
    Rejected,
}

pub struct ArbitrationStatus {
    pub state: ArbitrationState,
    pub instance: String,
    pub req_id: String,
    pub lease: Option<LeaseGrant>,
}
```

These DTOs preserve protocol facts. A8a owns concurrency behavior; A2b does not change arbitration semantics.

## 5. `crates/lab` Application Interface

### 5.1 Construction and Lifetime

```rust
pub struct Lab<P> {
    ports: P,
    state: LabState,
}

impl<P: LabPorts> Lab<P> {
    pub fn new(ports: P, state: LabState) -> Result<Self, LabError>;
}
```

Rules:

- `Lab::new` is the only public Lab construction entry.
- Current CLI creates one Lab per process invocation.
- A future resident runtime creates one Lab and reuses it across requests.
- Application state paths are resolved by `LabState`, never by external direct filesystem access.
- The UI never imports Lab or owns its lifecycle.

### 5.2 Use-Case Method Shape

Each migrated family exposes typed request/response methods:

```rust
impl<P: LabPorts> Lab<P> {
    pub fn recognize(&mut self, request: RecognizeRequest) -> LabResult<RecognizeResponse>;
    pub fn detect_page(&mut self, request: DetectPageRequest) -> LabResult<DetectPageResponse>;
    pub fn current_page(&mut self, request: CurrentPageRequest) -> LabResult<CurrentPageResponse>;
    pub fn is_visible(&mut self, request: IsVisibleRequest) -> LabResult<IsVisibleResponse>;
}
```

The same shape applies to A3-A8b families. Request parsing and process exit mapping are not methods on `Lab`.

### 5.3 Public API Type Rule

- no public Lab function, trait method, type alias, request, or response may contain `serde_json::Value`;
- private migration helpers may use `Value` until their family moves;
- contract `Envelope<Value>` remains allowed only in the CLI adapter for unmigrated long-tail commands;
- migrated command adapters serialize typed responses through `Envelope<T>`.

## 6. Port Inventory and Decisions

### 6.1 Device Control

Decision: reuse `actingcommand_device::InputBackend`; do not add a competing tap/swipe trait.

Lab defines only a factory boundary because use cases currently select and open a backend from configuration:

```rust
pub trait InputBackendFactory {
    fn open(&self, request: InputBackendRequest)
        -> LabResult<Box<dyn actingcommand_device::InputBackend>>;
}
```

`InputBackendRequest` is a Lab-internal typed request containing resolved backend/device configuration. Backend close errors remain fatal and are combined with operation errors using existing device behavior.

### 6.2 Capture

Decision: reuse `actingcommand_device::CaptureBackend`; do not add another capture operation trait.

```rust
pub trait CaptureBackendFactory {
    fn open(&self, request: CaptureBackendRequest)
        -> LabResult<Box<dyn actingcommand_device::CaptureBackend>>;
}
```

Immediate capture returns `actingcommand_device::Frame`. Persisted image references are created only by a use case that stores a frame; `CaptureRef` remains the external/protocol reference model.

### 6.3 Ledger Sink

No existing deep-library trait represents the application append/projection boundary. Add:

```rust
pub trait LedgerSink {
    fn append_drive<T: serde::Serialize>(&mut self, record: &DriveRecord<T>) -> LabResult<()>;
    fn finish<T: serde::Serialize>(&mut self, response: &T) -> LabResult<LedgerProjection>;
}
```

The implementation wraps `actingcommand_ledger::LabLedger`. It must preserve record-before-act rules and fail loudly on write/projection errors.

### 6.4 Clock

Add:

```rust
pub trait Clock {
    fn now_unix_ms(&self) -> LabResult<u64>;
    fn sleep(&self, duration: std::time::Duration);
}
```

Production uses a system clock. Tests use a fixed clock. Retry and wait behavior remains bounded by existing policy.

### 6.5 Config Source

Add:

```rust
pub trait ConfigSource {
    fn load(&self) -> LabResult<UserConfig>;
    fn state_root(&self) -> LabResult<std::path::PathBuf>;
}
```

`UserConfig` belongs to `crates/lab`, not the contract crate, because it contains local implementation paths and backend choices. Environment variables are read only by the CLI/production adapter and converted into this injected source. Migrated Lab modules may not call `env::var` for behavior.

### 6.6 Final Port Set

The frozen A2b port set is:

- `InputBackendFactory`, returning the existing `InputBackend`;
- `CaptureBackendFactory`, returning the existing `CaptureBackend`;
- `LedgerSink`;
- `Clock`;
- `ConfigSource`.

No second recognition, OCR, app-lifecycle, generic filesystem, generic JSON, or generic command-execution port is introduced in issue #33.

## 7. State Ownership

`LabState` is split into explicit domains:

```rust
pub struct LabState {
    pub arbitrator: ArbitratorStore,
    pub environment: EnvStore,
    pub sessions: SessionStore,
}
```

Rules:

- each store has one public constructor owned by `LabState::open`;
- state paths are private implementation details;
- callers cannot obtain mutable filesystem paths and bypass the store;
- A8a adds cross-process arbitration locking, stale-lock recovery, and crash-safe write semantics in the original location before A8b moves the code;
- environment-result locking keeps current behavior until a separately approved change;
- Session ledger same-name conflict detection lands in A8b;
- no claim is made that separate CLI processes have one live in-memory owner.

## 8. Scheduler and Runtime Responsibilities

Current CLI phase:

- parses one request;
- constructs adapters, `LabState`, and `Lab`;
- invokes one typed use case;
- serializes one envelope;
- maps semantic error class to process exit code.

Future resident runtime phase:

- owns one long-lived `Lab` instance;
- scheduler is the sole admission/arbitration and lease-issuance authority;
- scheduler invokes the same typed Lab methods;
- Lab validates lease/state preconditions and executes the use case;
- API/UI layers never invoke deep device or recognition libraries directly.

The scheduler implementation, resident process, network protocol, and I5b process ownership are outside issue #33.

## 9. Migration Boundaries

### A2b

- add contract Lab DTOs and semantic errors;
- add `crates/lab`, ports, context, and state skeleton;
- move envelope construction, semantic ledger projection facilities, and error conversion;
- keep JSON field names and exit codes unchanged.

### A3-A8b

- move each approved family in strict order;
- CLI parses flags into typed request DTOs;
- migrated Lab functions expose typed responses;
- function bodies move mechanically except A8a's explicit behavior repair;
- tests move with the implementation; A1 goldens remain unchanged except the A8a lock-conflict addition and explicit re-freeze.

### A9

- retire deprecated crates only after consumer, replacement, and equivalence evidence;
- preserve the promoted contract crate;
- enable terminal pipeline guards and publish the final audit;
- claim I5a only, explicitly not I5b.

## 10. Explicit Rejections

- No new `lab-contract` crate.
- No dependency from a crate to `apps/actinglab`.
- No generic `utils` or `common` crate.
- No raw CLI `FlagArgs` in `crates/lab`.
- No process exit, stdout/stderr printing, or behavioral environment reads in `crates/lab`.
- No second input/capture operation trait parallel to the device crate traits.
- No schema-version change.
- No scheduler, daemon, UI, OCR, SQLite, or game-specific behavior in this chain.
