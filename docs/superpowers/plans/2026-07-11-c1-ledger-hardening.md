# C1 Global Ledger Hardening Implementation Plan

> **Execution rule:** Follow repository `AGENTS.md`, verified `HS7097` authority, and this approved plan directly. Do not invoke Superpowers. Steps use checkbox syntax for tracking.

**Goal:** Close every Critical, Important, and Minor finding from the Issue #35 whole-C1 review without adding C3a or later-phase behavior.

**Architecture:** Replace the generic producer-classified event payload with a closed contract-owned v2 payload enum, move persisted facts into the ledger, and retain one typed sanitized ingress. Deepen the ledger with bounded replay, absorbing terminal subscriptions, synchronously owned startup, and a durable repair journal. Critical execution builds and redacts its selected outcome after the action and reports effect disposition, while exactly-once reconciliation remains explicitly owned by C3a.

**Tech Stack:** Rust 2024, existing `serde`, `serde_json`, `sha2`, standard-library threads/channels/filesystem locks, and existing test dependencies only.

## Global Constraints

- Binding design: `docs/superpowers/specs/2026-07-11-c1-ledger-hardening-design.md`.
- Binding upstream task: Issue #35 and `TASK-runtime-ledger-core-and-optional-lab-correction-v3.md`, SHA-256 `28273b85491b0d43aa7a7b7a7ece10db681de9df4d9100e85f9e9b086dd107a6`.
- Event wire schema becomes exactly `actingcommand.event.v2`; old generic v1 segment data fails loudly and is not silently migrated.
- Ledger ingress accepts only the closed typed `SanitizedEventDraft`.
- No public event, persisted fact, or projection payload uses `serde_json::Value`.
- The producer cannot select sensitivity or redaction policy for runtime data.
- Critical order is `typed intent -> sanitize -> durable append -> act once in this call -> build selected typed outcome -> sanitize -> durable append -> receipt`.
- C1 does not provide cross-call exactly-once, retry, deduplication, compensation, or reconciliation.
- The ledger library never installs or replaces the process panic hook.
- Replay work and transient memory are bounded; fatal subscription states are absorbing.
- Startup never returns while another thread may later acquire ledger ownership or mutate the root.
- Destructive tail repair cannot become durable without a durable, eventually unique recovery fact.
- No SQLite, async runtime, network API, scheduler behavior, DeviceProxy, UI, artifact bytes, resource data, OCR, recognition, game logic, or new production dependency.
- Preserve legacy `LabLedger`, protocol goldens, dependency direction, and non-Lab removability.
- Severe errors fail loudly; no fallback or retry is added in C1.

## File Structure

- `crates/actingcommand-contract/src/event.rs`: event vocabulary and public re-exports.
- `crates/actingcommand-contract/src/event/ids.rs`: opaque ID, static code, hash, media type, and object-key newtypes.
- `crates/actingcommand-contract/src/event/payload.rs`: raw schema-owned family drafts, sanitization, `EventPayload`, and public projection payloads.
- `crates/actingcommand-contract/src/event/artifact.rs`: complete v2 artifact-reference contract.
- `crates/actingcommand-contract/src/event/envelope.rs`: raw and sanitized envelopes, links, origin, query, cursor, and projection DTOs.
- `crates/ledger/src/fact.rs`: opaque ledger-owned `PersistedEvent` and private storage record conversion.
- `crates/ledger/src/critical.rs`: post-action outcome orchestration and effect disposition.
- `crates/ledger/src/global.rs`: writer commands, bounded subscriptions, owned startup, and public ledger API.
- `crates/ledger/src/global/storage.rs`: append/recovery, owner metadata, segment validation, and repair journal.
- `crates/ledger/src/global/projection.rs`: typed profile projections.
- `crates/ledger/src/global/tests.rs`: unit and state-machine coverage.
- `crates/ledger/src/global/recovery_tests.rs`: test-only process kill matrix for repair boundaries.
- `crates/ledger/tests/global_ledger_process.rs`: process acceptance and cross-source proof.
- `tools/actinglab-architecture/tests/workspace_guards.rs`: public-contract and dependency guards.

Execution atomicity: Tasks 1 and 2 are one implementation/review unit. Removing the public v1 contract necessarily breaks the existing ledger until the opaque v2 fact migration is complete, so there must be no pushed or reviewed intermediate commit with a broken workspace.

Tasks 1-2 completion note: the atomic v2 migration and review fixes are implemented by `0a3b6a6`, `0a610ce`, `6bb3406`, and `5a5fd19`. Artifact attachments require an opaque store-issued capability. Until C2 supplies the durable artifact owner and verifier, recovery of any artifact-bearing stored event fails fatally with `artifact_store_verification_unavailable`; C1 never promotes self-consistent public metadata into a trusted fact.

---

### Task 1: Schema-Owned Event Contract V2

**Files:**
- Create: `crates/actingcommand-contract/src/event/ids.rs`
- Create: `crates/actingcommand-contract/src/event/payload.rs`
- Create: `crates/actingcommand-contract/src/event/artifact.rs`
- Create: `crates/actingcommand-contract/src/event/envelope.rs`
- Modify: `crates/actingcommand-contract/src/event.rs`
- Modify: `crates/actingcommand-contract/src/lib.rs`

**Interfaces:**

```rust
pub const GLOBAL_EVENT_SCHEMA_VERSION: &str = "actingcommand.event.v2";

pub trait SecretFingerprinter {
    fn fingerprint(
        &self,
        field: SecretField,
        original: &str,
    ) -> Result<Sha256Fingerprint, SanitizationError>;
}

pub enum SecretField {
    AccountIdentity,
    AuthenticationMaterial,
}

pub struct AuditInput { /* private raw values */ }

impl AuditInput {
    pub fn new() -> Self;
    pub fn with_account(self, value: impl Into<String>) -> Self;
    pub fn with_authentication(self, value: impl Into<String>) -> Self;
    pub fn with_machine_path(self, value: impl Into<String>) -> Self;
    pub fn with_device_endpoint(self, value: impl Into<String>) -> Self;
}

pub enum EventPayloadDraft {
    Command(CommandPayloadDraft),
    Scheduler(SchedulerPayloadDraft),
    Lease(LeasePayloadDraft),
    Task(TaskPayloadDraft),
    Input(InputPayloadDraft),
    Client(ClientPayloadDraft),
    Ledger(LedgerPayloadDraft),
}

pub enum EventPayload {
    Command(CommandPayload),
    Scheduler(SchedulerPayload),
    Lease(LeasePayload),
    Task(TaskPayload),
    Input(InputPayload),
    Client(ClientPayload),
    Ledger(LedgerPayload),
}

pub struct EventDraft { /* private */ }

impl EventDraft {
    pub fn new(
        event_id: EventId,
        timestamp_unix_ms: u64,
        severity: EventSeverity,
        origin: EventOrigin,
        links: EventLinks,
        payload: EventPayloadDraft,
    ) -> Self;

    pub fn with_artifacts(self, artifacts: Vec<ArtifactReference>) -> Self;

    pub fn sanitize(
        self,
        fingerprinter: &dyn SecretFingerprinter,
    ) -> Result<SanitizedEventDraft, SanitizationError>;
}

pub struct SanitizedEventDraft { /* private, Serialize, not Deserialize */ }

pub enum ProjectionPayload {
    Omitted,
    Public(PublicEventPayload),
    Full(EventPayload),
}
```

The ID macro creates `EventId`, `InstanceId`, `RequestId`, `CorrelationId`, `CausationId`, `TaskId`, `RunId`, `LeaseId`, `FrameId`, `ActionId`, `RecognitionId`, and `ArtifactId`. Each public constructor accepts `[u8; 16]`; serialization is a fixed type prefix plus 32 lowercase hex characters. `StaticCode::new` accepts only `&'static str` matching bounded ASCII code syntax.

Family draft constructors are semantic and have no generic field list:

```rust
CommandPayloadDraft::{received, validated, rejected}
SchedulerPayloadDraft::{admitted, queued, denied, preempted}
LeasePayloadDraft::{requested, granted, transferred, released, expired,
                    transition_intent, transition_failed}
TaskPayloadDraft::{requested, started, step_started, step_finished,
                   completed, failed, cancelled,
                   terminal_intent, terminal_commit_failed}
InputPayloadDraft::{intent, committed, completed, failed}
ClientPayloadDraft::{ui_action, cli_command, lab_request}
LedgerPayloadDraft::recovered
```

Critical outcome constructors require `EffectDisposition`; noncritical observational variants reject it. Account is fingerprinted, authentication material is dropped, machine path and endpoint are masked. No constructor exposes `Sensitivity` or `RedactionPolicy`.

`ArtifactReference::new` requires artifact ID, kind, optional run/frame/correlation IDs, object key, media type, byte count, SHA-256, creation timestamp, producer static code, retention class, and redaction state. All fields are private and validated.

- [x] **Step 1: Add RED contract tests**

Add tests named:

```text
producer_cannot_select_redaction_policy
all_runtime_secret_classes_follow_schema_owned_policy
hash_shaped_original_cannot_survive_as_fingerprint
runtime_values_cannot_enter_static_code_or_typed_ids
sanitized_event_is_typed_and_not_deserializable
artifact_reference_requires_complete_v3_metadata
artifact_object_key_rejects_absolute_or_parent_paths
event_v2_round_trips_every_c1_payload_variant
```

Add compile-fail documentation proving `ClassifiedField`, `StructuredPayloadDraft`, public raw-policy construction, sanitized-draft field mutation, and deserialization of `SanitizedEventDraft` are unavailable.

- [x] **Step 2: Verify RED**

Run: `cargo test -p actingcommand-contract event:: -- --nocapture`

Expected: compile/test failure because the v2 modules and interfaces do not exist.

- [x] **Step 3: Implement IDs, payload schemas, sanitization, and artifact contract**

Implement only the interfaces and variants listed above. `Sha256Fingerprint::new` rejects a candidate when either the full candidate or its digest portion equals the original.

- [x] **Step 4: Verify GREEN and forbidden API surface**

Run:

```text
cargo test -p actingcommand-contract event:: -- --nocapture
cargo test -p actingcommand-contract --doc
rg -n "pub .*ClassifiedField|pub .*StructuredPayloadDraft|serde_json::Value" crates/actingcommand-contract/src/event.rs crates/actingcommand-contract/src/event
cargo clippy -p actingcommand-contract -- -D warnings
cargo fmt --all -- --check
git diff --check
```

Expected: tests pass; scans find no public caller-selected policy type or `Value` payload.

- [x] **Step 5: Continue directly into Task 2 without committing**

Do not create or push a contract-only commit. Keep the RED/GREEN evidence in the Task 1 report and complete the ledger migration below before committing.

---

### Task 2: Opaque Ledger Facts And Typed Projections

**Files:**
- Create: `crates/ledger/src/fact.rs`
- Modify: `crates/ledger/src/lib.rs`
- Modify: `crates/ledger/src/global.rs`
- Modify: `crates/ledger/src/global/storage.rs`
- Modify: `crates/ledger/src/global/projection.rs`
- Modify: `crates/ledger/src/global/tests.rs`
- Modify: `crates/ledger/tests/global_ledger_process.rs`

**Interfaces:**

```rust
pub struct PersistedEvent { /* private, Serialize, not Deserialize */ }

impl PersistedEvent {
    pub fn sequence(&self) -> u64;
    pub fn event_id(&self) -> &EventId;
    pub fn timestamp_unix_ms(&self) -> u64;
    pub fn event_type(&self) -> EventType;
    pub fn severity(&self) -> EventSeverity;
    pub fn sensitivity(&self) -> Sensitivity;
    pub fn origin(&self) -> &EventOrigin;
    pub fn links(&self) -> &EventLinks;
    pub fn payload(&self) -> &EventPayload;
    pub fn artifacts(&self) -> &[ArtifactReference];
}

impl GlobalLedger {
    pub fn append(&self, draft: SanitizedEventDraft)
        -> GlobalLedgerResult<PersistedEvent>;
    pub fn query(&self, query: EventQuery)
        -> GlobalLedgerResult<Vec<PersistedEvent>>;
    pub fn project(&self, query: EventQuery, profile: ProjectionProfile)
        -> GlobalLedgerResult<Vec<ProjectedEvent>>;
}
```

`StoredEventRecord` is crate-private and is the only `Deserialize` form. Conversion to `PersistedEvent` validates schema v2, sequence, event type/payload match, identifiers, typed payload invariants, and artifacts. `PersistedEvent::from_sanitized` is `pub(crate)`.

- [x] **Step 1: Add RED fact and projection tests**

Add tests named:

```text
persisted_event_cannot_be_constructed_or_deserialized_by_consumers
storage_assigns_the_only_sequence
v1_generic_segment_fails_loudly
typed_record_recovery_rebuilds_same_fact
concise_projection_omits_payload
ui_projection_contains_only_public_typed_payload
lab_projection_contains_full_sanitized_typed_payload
ui_projection_omits_artifact_object_key
```

- [x] **Step 2: Verify RED**

Run: `cargo test -p actingcommand-ledger global:: -- --nocapture`

Expected: compile failure until the ledger owns the fact and callers are migrated.

- [x] **Step 3: Implement the fact, private storage record, matching, and typed projection**

Remove contract-owned `PersistedEvent`, `ErasedSanitizedEventDraft`, and `EventQuery::matches`. Query matching lives in ledger projection/index code and uses typed ID getters.

- [x] **Step 4: Verify GREEN**

Run:

```text
cargo test -p actingcommand-contract -p actingcommand-ledger -- --nocapture
cargo clippy -p actingcommand-contract -p actingcommand-ledger -- -D warnings
rg -n "pub .*serde_json::Value|payload:[[:space:]]*(Option<)?Value|derive\([^)]*Deserialize[^)]*\).*PersistedEvent|pub fn from_.*PersistedEvent" crates/actingcommand-contract crates/ledger/src
cargo fmt --all -- --check
git diff --check
```

Expected: tests pass; no public `Value` payload, persisted deserializer, or public fact constructor exists.

- [x] **Step 5: Commit**

Commit message: `feat(ledger): enforce typed event v2 facts`

---

### Task 3: Post-Action Critical Outcomes

**Files:**
- Modify: `crates/actingcommand-contract/src/event.rs`
- Modify: `crates/actingcommand-contract/src/event/payload.rs`
- Modify: `crates/ledger/src/critical.rs`
- Modify: `crates/ledger/src/lib.rs`
- Modify: `crates/ledger/tests/global_ledger_process.rs`

**Interfaces:**

```rust
pub enum EffectDisposition {
    NotPerformed,
    Performed,
    Indeterminate,
}

pub enum DefiniteEffectDisposition {
    NotPerformed,
    Performed,
}

pub enum CriticalActionReport<T, E> {
    Succeeded { value: T, effect: DefiniteEffectDisposition },
    Failed { error: E, effect: EffectDisposition },
}

pub enum CriticalOperation {
    CommandValidation,
    DeviceWrite,
    LeaseTransition(LeaseTransitionTarget),
    TaskTerminal(TaskTerminalTarget),
}

pub struct CriticalEventPlan { /* operation plus sanitized intent */ }

pub fn execute_critical<T, E, SB, FB>(
    appender: &impl EventAppender,
    fingerprinter: &dyn SecretFingerprinter,
    plan: CriticalEventPlan,
    action: impl FnOnce() -> CriticalActionReport<T, E>,
    success_builder: SB,
    failure_builder: FB,
) -> Result<CriticalReceipt<T>, CriticalExecutionError<E>>
where
    SB: FnOnce(&T, DefiniteEffectDisposition) -> Result<EventDraft, SanitizationError>,
    FB: FnOnce(&E, EffectDisposition) -> Result<EventDraft, SanitizationError>;
```

The executor does not call `catch_unwind` and does not install a panic hook. Panic propagation leaves the durable intent without a guessed outcome. Outcome build, sanitize, role, stable-link, or append failure after the action returns `OutcomeUndurable { effect, stage, code }` with no success receipt.

- [x] **Step 1: Add RED critical tests**

Add tests named:

```text
intent_append_failure_prevents_action_and_builders
success_builder_observes_action_value_and_effect
failure_builder_observes_action_error_and_effect
outcome_sanitization_failure_is_fatal_without_receipt
wrong_post_action_role_is_fatal_without_receipt
outcome_append_failure_preserves_effect_without_receipt
lease_transition_role_map_is_complete
task_terminal_role_map_is_complete
same_action_id_is_not_deduplicated_by_c1
panic_propagates_after_durable_intent_without_installing_hook
critical_debug_does_not_disclose_value_error_payload_path_or_endpoint
```

- [x] **Step 2: Verify RED**

Run: `cargo test -p actingcommand-ledger critical::tests -- --nocapture`

Expected: compile/test failure against the prebuilt-outcome API.

- [x] **Step 3: Implement post-action builders and non-disclosing errors**

Delete the process-global panic hook and all prebuilt outcome selection. Validate the selected outcome after building and sanitizing it. Custom `Debug` must not require or format `T: Debug` or `E: Debug`.

- [x] **Step 4: Verify GREEN and no false exactly-once claim**

Run:

```text
cargo test -p actingcommand-ledger critical::tests -- --nocapture
cargo test -p actingcommand-ledger --test global_ledger_process -- --nocapture
rg -n "take_hook|set_hook|exactly.once|catch_unwind" crates/ledger/src/critical.rs PLANS.md CHECKPOINT.md docs
cargo clippy -p actingcommand-ledger -- -D warnings
cargo fmt --all -- --check
git diff --check
```

Expected: tests pass; source contains no panic-hook mutation, unwind catch, or C1 exactly-once claim.

- [x] **Step 5: Commit**

Commit message: `fix(ledger): build critical outcomes after action`

---

### Task 4: Bounded Replay And Absorbing Subscription State

**Files:**
- Modify: `crates/ledger/src/global.rs`
- Modify: `crates/ledger/src/global/storage.rs`
- Modify: `crates/ledger/src/global/tests.rs`

**Interfaces:**

```rust
pub struct SubscriptionOptions { replay_page_events: usize }

impl SubscriptionOptions {
    pub fn new(replay_page_events: usize) -> GlobalLedgerResult<Self>;
}

impl GlobalLedger {
    pub fn subscribe(&self, cursor: SubscriptionCursor)
        -> GlobalLedgerResult<LedgerSubscription>;
    pub fn subscribe_with_options(
        &self,
        cursor: SubscriptionCursor,
        options: SubscriptionOptions,
    ) -> GlobalLedgerResult<LedgerSubscription>;
}

impl LedgerSubscription {
    pub fn recv_timeout(&mut self, timeout: Duration)
        -> GlobalLedgerResult<PersistedEvent>;
    pub fn resume_cursor(&self) -> SubscriptionCursor;
    pub fn replay_through_sequence(&self) -> u64;
}
```

Registration captures high-water `S`, registers live events for `> S`, and returns without cloning history. `WriterCommand::ReplayPage` returns at most `replay_page_events` events in `(after, S]`. Fatal lag/replay/writer errors and clean closure latch permanently; timeout does not.

- [x] **Step 1: Add RED subscription tests**

Add tests named:

```text
subscription_registration_does_not_clone_unbounded_history
replay_pages_are_bounded_and_writer_remains_responsive
paged_replay_then_live_is_gap_free_and_ordered
subscription_lag_is_absorbing_and_discards_buffered_events
terminal_writer_failure_preempts_replay_and_is_stable
resume_cursor_recovers_every_missing_event
future_cursor_remains_gap_free
invalid_replay_page_size_is_rejected
```

- [x] **Step 2: Verify RED**

Run: `cargo test -p actingcommand-ledger global::tests::subscription -- --nocapture`

Expected: tests fail because replay is currently one unbounded clone and terminal errors are consumable.

- [x] **Step 3: Implement bounded page commands and terminal state machine**

Use the existing in-memory durable event vector for bounded page reads. A page command may clone at most 1024 events. The subscription checks terminal state before and after every page/live receive and clears all buffers when terminal.

- [x] **Step 4: Verify GREEN**

Run:

```text
cargo test -p actingcommand-ledger global::tests::subscription -- --nocapture
cargo test -p actingcommand-ledger --test global_ledger_process -- --nocapture
rg -n "events_after\(|VecDeque::from\(store\.events" crates/ledger/src
cargo clippy -p actingcommand-ledger -- -D warnings
cargo fmt --all -- --check
git diff --check
```

Expected: tests pass and the unbounded replay-clone path is absent.

- [x] **Step 5: Commit**

Commit message: `fix(ledger): bound subscription replay`

---

### Task 5: Owned Startup And Storage Invariants

**Files:**
- Modify: `crates/ledger/src/global.rs`
- Modify: `crates/ledger/src/global/storage.rs`
- Modify: `crates/ledger/src/global/tests.rs`

**Startup design:**

1. Spawn a writer thread that blocks on a one-shot `Receiver<SegmentStore>` and cannot touch the ledger root.
2. Open `SegmentStore` synchronously in `GlobalLedger::open`.
3. On store-open failure, close the one-shot channel, join the waiting writer, and return the store error.
4. On successful store open, send the owned store to the writer.
5. If sending fails, recover the store from `SendError`, close it, join the writer, and return fatal `writer_unavailable`.
6. Remove the startup timeout and detached initialization worker.

- [x] **Step 1: Add RED lifecycle and corruption tests**

Add tests named:

```text
open_never_returns_with_a_detached_future_owner
store_open_failure_joins_waiting_writer
immediate_retry_after_failed_open_has_no_writer_conflict
backward_close_wall_clock_is_valid_diagnostic_metadata
empty_non_final_segment_is_fatal
sole_final_empty_segment_is_valid
config_debug_redacts_owner_and_root
```

- [x] **Step 2: Verify RED**

Run: `cargo test -p actingcommand-ledger global::tests -- --nocapture`

Expected: existing startup-timeout, timestamp-order, empty-segment, and debug behavior fails the new assertions.

- [x] **Step 3: Implement owned startup and invariant fixes**

Remove `open_with_store` timeout behavior. Treat nonzero wall-clock timestamps as observations without ordering. Reject zero-record non-final segments. Render `owner_id` as `<redacted-owner-id>` in `Debug`.

- [x] **Step 4: Verify GREEN**

Run:

```text
cargo test -p actingcommand-ledger global::tests -- --nocapture
rg -n "writer_start_timeout|drop\(writer\)|field\(\"owner_id\", &self\.owner_id\)" crates/ledger/src
cargo clippy -p actingcommand-ledger -- -D warnings
cargo fmt --all -- --check
git diff --check
```

Expected: tests pass; detached-startup and owner disclosure paths are absent.

- [x] **Step 5: Commit**

Commit message: `fix(ledger): own startup and storage lifecycle`

---

### Task 6: Crash-Atomic Tail Repair Journal

**Files:**
- Modify: `crates/ledger/src/global/storage.rs`
- Create: `crates/ledger/src/global/recovery_tests.rs`
- Modify: `crates/ledger/src/global.rs`
- Modify: `crates/ledger/src/global/tests.rs`

**Durable record:**

```rust
enum RepairJournalState { Prepared, Completed }

struct TailRepairRecord {
    schema_version: String, // actingcommand.ledger-repair.v1
    repair_id: String,
    state: RepairJournalState,
    segment_index: u64,
    original_len: u64,
    repaired_len: u64,
    tail_sha256: String,
    quarantine_key: String,
}
```

`repair_id` is deterministic SHA-256 over segment index, original length, repaired length, and tail hash. Journal records are append-only JSONL with duplicate-key rejection and `sync_all` after every record. The deterministic recovery event ID is derived from `repair_id`.

- [x] **Step 1: Add RED repair state-machine and process tests**

Add tests named:

```text
prepared_repair_resumes_before_open_succeeds
unexpected_segment_length_during_repair_is_fatal
recovery_event_is_unique_when_crash_precedes_completion
successful_open_has_no_unresolved_prepared_repair
kill_after_prepare_recovers_one_fact
kill_after_quarantine_recovers_one_fact
kill_after_truncate_recovers_one_fact
kill_after_recovery_append_recovers_one_fact
kill_after_completion_reopens_cleanly
```

Process tests use `#[cfg(test)]` barriers in the unit-test binary. No production environment-variable failpoint is compiled.

- [x] **Step 2: Verify RED**

Run: `cargo test -p actingcommand-ledger global::recovery_tests -- --nocapture`

Expected: tests fail because destructive truncation currently precedes its recovery fact.

- [x] **Step 3: Implement prepare/quarantine/truncate/event/complete recovery**

Write and sync `Prepared` before quarantine or truncation. Resume idempotently on startup. If a deterministic recovery event already exists, verify it and append `Completed` without duplication. Any unrecognized journal schema, duplicate key, inconsistent state transition, or unexpected segment length is fatal.

Bump writer metadata to `actingcommand.ledger-writer.v2`. Because no production C1 root is approved, reject v1 writer metadata and v1 event segments explicitly instead of silently upgrading them.

- [x] **Step 4: Verify GREEN and process matrix**

Run:

```text
cargo test -p actingcommand-ledger global::recovery_tests -- --nocapture
cargo test -p actingcommand-ledger --test global_ledger_process -- --nocapture
cargo clippy -p actingcommand-ledger -- -D warnings
cargo fmt --all -- --check
git diff --check
```

Expected: every kill boundary yields exactly one recovery fact, contiguous sequence, one quarantine object, and no unresolved prepared repair after successful open.

- [x] **Step 5: Commit**

Commit message: `fix(ledger): journal tail recovery atomically`

---

### Task 7: C1 Adversarial Acceptance And Closeout

**Files:**
- Modify: `crates/ledger/tests/global_ledger_process.rs`
- Modify: `tools/actinglab-architecture/tests/workspace_guards.rs`
- Modify: `docs/superpowers/plans/2026-07-10-c1-global-event-ledger.md`
- Modify: `PLANS.md`
- Modify: `CHECKPOINT.md`

- [x] **Step 1: Add final adversarial acceptance and architecture guards**

Add or update tests proving:

```text
five_sources_share_one_correlated_typed_ledger
all_secret_classes_are_absent_from_files_indexes_errors_and_every_projection
critical_append_failure_blocks_side_effect
crash_after_intent_never_forges_an_outcome
contract_has_no_public_value_payload_or_persisted_fact
ledger_ingress_accepts_only_sanitized_event_v2
all_non_lab_packages_remain_lab_free_with_all_features
```

Add source guards for panic-hook mutation, generic caller-selected policy types, public persisted constructors, and unbounded replay helpers.

- [x] **Step 2: Run the complete C1 gate**

Run:

```text
cargo test -p actingcommand-contract -p actingcommand-ledger -p actingcommand-actinglab-architecture -- --nocapture
cargo test --workspace
cargo test --workspace --exclude actingcommand-lab --exclude actingcommand-actinglab
cargo clippy --workspace -- -D warnings
cargo fmt --all -- --check
git diff --check
rg -n "pub .*serde_json::Value|payload:[[:space:]]*(Option<)?Value|ClassifiedField|StructuredPayloadDraft|ErasedSanitizedEventDraft|take_hook|set_hook|events_after\(" crates/actingcommand-contract/src/event.rs crates/actingcommand-contract/src/event crates/ledger/src/critical.rs crates/ledger/src/fact.rs crates/ledger/src/global.rs crates/ledger/src/global crates/ledger/tests/global_ledger_process.rs
rg -n "actingcommand[_-]lab" crates/actingcommand-contract crates/ledger
```

Expected: all tests and gates pass; the C1 surface scan and Lab-dependency scan have no hits. The scan is intentionally scoped away from the explicitly preserved legacy `LabLedger` compatibility API in `crates/ledger/src/lib.rs`, whose six public `serde_json::Value` signatures remain covered by legacy tests and are not GlobalLedger ingress.

- [x] **Step 3: Update planning and checkpoint evidence**

Record every hardening commit, RED/GREEN evidence, schema change, deferred C3a responsibilities, full gate result, and rollback anchor. Mark the old C1 Task 1-5 plan as superseded by this approved hardening closeout where its frozen generic interfaces conflict.

- [x] **Step 4: Run a fresh whole-C1 review**

Review the complete C1 range from `3e65741` through the hardening head against Issue #35, v3, C0, and the approved hardening design. Fix every Critical and Important finding and repeat until clean.

- [x] **Step 5: Commit, push, and record Issue #36 evidence**

Commit message: `docs(runtime): close C1 ledger hardening`

Push `issue-35-runtime-ledger-v3`, verify local/remote synchronization, and post the final C1 evidence to Issue #36. Do not merge into `main` or the umbrella repository.
