# C1 Global Event Contract And Ledger Skeleton Implementation Plan

> **Superseded implementation record:** Issue #35 C1 was hardened and closed through
> `docs/superpowers/plans/2026-07-11-c1-ledger-hardening.md`. This file preserves the
> original Task 1-5 history only. Where its generic interfaces, startup sequence, recovery
> schema, or closeout instructions conflict with the approved hardening plan and design,
> the 2026-07-11 documents are authoritative. Do not resume implementation from this file.

**Goal:** Deliver the Issue #35 C1 typed event contract and recoverable single-writer global ledger needed by C3a without changing Runtime, scheduler, device, Lab, UI, resource, or game behavior.

**Architecture:** `actingcommand-contract` owns storage-independent event vocabulary, raw-to-sanitized type transitions, typed correlation, query, and projection DTOs. `actingcommand-ledger` owns the SHA-256 redaction adapter, one bounded-ingress writer thread, OS-held writer ownership, segmented JSONL persistence, recovery, in-memory indexes, query/subscription/projection, and critical intent/action/outcome ordering. Legacy `LabLedger` stays intact for compatibility; new production code uses `GlobalLedger` only.

**Tech Stack:** Rust 2024, `serde`, `serde_json`, existing workspace `sha2`, standard-library threads/channels/file locks, and `tempfile` for tests.

## Global Constraints

- Bind implementation to Issue #35 v3 SHA-256 `28273b85491b0d43aa7a7b7a7ece10db681de9df4d9100e85f9e9b086dd107a6` and approved C0 payload SHA-256 `6c72a9c39ff67ec5a2868e0ed262d2a2f0a2c4b0fbfc473b7c55a9df610bf0a7`.
- Use Issue #36 as the only progress and evidence Issue; do not create subtask Issues.
- Do not cherry-pick the paused Issue #34 RED commit.
- Production packages must not directly or transitively depend on `actingcommand-lab`.
- `actingcommand-contract` must not depend on ledger storage.
- Ledger ingress accepts only `SanitizedEventDraft<P>` or its erased sanitized representation, never `EventDraft<P>`.
- Critical order is `typed intent -> redact -> durable append -> act -> typed outcome -> redact -> durable append -> receipt`.
- Secret originals, account values, authentication material, machine paths, and device endpoints must not reach durable JSONL, indexes, error messages, or projections.
- Final-tail truncation may be quarantined explicitly; complete-line or non-final-segment corruption is fatal.
- No SQLite, async runtime, network API, scheduler behavior, DeviceProxy, artifact bytes, UI, resource data, OCR, recognition, game logic, or new external dependency in C1.
- Preserve all existing protocol goldens and legacy `LabLedger` behavior.

---

## File Structure

- `crates/actingcommand-contract/src/event.rs`: event types, identifiers, origin, sensitivity/redaction schema, raw/sanitized payload transition, typed payload families, queries, subscriptions cursors, and projection DTOs.
- `crates/actingcommand-contract/src/lib.rs`: export the new event module.
- `crates/ledger/src/global.rs`: small public `GlobalLedger` interface and writer-thread command loop.
- `crates/ledger/src/global/storage.rs`: writer lock metadata, segmented JSONL append/recovery, sequence allocation, and in-memory indexes.
- `crates/ledger/src/global/projection.rs`: query matching and CLI/UI/Lab projection profiles over sanitized persisted events.
- `crates/ledger/src/global/tests.rs`: deterministic unit tests for append/query/subscribe/projection/recovery/redaction.
- `crates/ledger/src/critical.rs`: critical intent/action/outcome orchestration over a narrow `EventAppender` seam.
- `crates/ledger/tests/global_ledger_process.rs`: process-level writer conflict and hard-kill recovery tests.
- `crates/ledger/Cargo.toml`: add the existing workspace contract dependency.
- `crates/ledger/src/lib.rs`: export `critical` and `global` while retaining legacy exports.
- `tools/actinglab-architecture/tests/workspace_guards.rs`: enforce contract/storage dependency direction and Lab removability after C1.
- `PLANS.md`, `CHECKPOINT.md`: record C1 implementation and evidence.

## Frozen Interfaces

The contract module produces these public interfaces:

```rust
pub trait FieldRedactor {
    fn fingerprint(&self, field_name: &str, value: &str)
        -> Result<String, SanitizationError>;
}

pub trait RedactablePayload {
    type Sanitized: SanitizedPayload;
    fn family(&self) -> EventFamily;
    fn sanitize(
        self,
        redactor: &dyn FieldRedactor,
    ) -> Result<Self::Sanitized, SanitizationError>;
}

pub trait SanitizedPayload:
    serde::Serialize + Clone + Send + Sync + 'static
{
    const SCHEMA: &'static str;
    const FAMILY: EventFamily;
    fn sensitivity(&self) -> Sensitivity;
}

impl<P: RedactablePayload> EventDraft<P> {
    pub fn sanitize(
        self,
        redactor: &dyn FieldRedactor,
    ) -> Result<SanitizedEventDraft<P::Sanitized>, SanitizationError>;
}

impl<P: SanitizedPayload> SanitizedEventDraft<P> {
    pub fn erase(self) -> Result<ErasedSanitizedEventDraft, EventContractError>;
}
```

Typed payload aliases use one shared structured implementation and distinct stage enums:

```rust
pub type CommandPayloadDraft = StructuredPayloadDraft<CommandStage>;
pub type SchedulerPayloadDraft = StructuredPayloadDraft<SchedulerDecision>;
pub type LeasePayloadDraft = StructuredPayloadDraft<LeaseTransition>;
pub type TaskPayloadDraft = StructuredPayloadDraft<TaskTransition>;
pub type InputPayloadDraft = StructuredPayloadDraft<InputTransition>;
pub type ClientPayloadDraft = StructuredPayloadDraft<ClientActionKind>;
pub type LedgerPayloadDraft = StructuredPayloadDraft<LedgerTransition>;
```

The ledger module exposes one deep interface:

```rust
pub struct GlobalLedgerConfig {
    pub root: std::path::PathBuf,
    pub owner_id: String,
    pub segment_max_bytes: u64,
    pub ingress_capacity: usize,
}

pub struct GlobalLedger { /* private writer thread and bounded ingress */ }

impl GlobalLedger {
    pub fn open(config: GlobalLedgerConfig) -> GlobalLedgerResult<Self>;
    pub fn append<P: SanitizedPayload>(
        &self,
        draft: SanitizedEventDraft<P>,
    ) -> GlobalLedgerResult<PersistedEvent>;
    pub fn query(&self, query: EventQuery) -> GlobalLedgerResult<Vec<PersistedEvent>>;
    pub fn subscribe(&self, cursor: SubscriptionCursor)
        -> GlobalLedgerResult<LedgerSubscription>;
    pub fn project(
        &self,
        query: EventQuery,
        profile: ProjectionProfile,
    ) -> GlobalLedgerResult<Vec<ProjectedEvent>>;
    pub fn close(self) -> GlobalLedgerResult<()>;
}
```

`GlobalLedger::append` always synchronously acknowledges after the JSONL record is flushed and synced in C1. Later phases may add an explicitly bounded noncritical path without weakening this interface.

---

### Task 1: Typed Event Contract And Pre-Persistence Redaction

**Files:**
- Create: `crates/actingcommand-contract/src/event.rs`
- Modify: `crates/actingcommand-contract/src/lib.rs`

**Interfaces:**
- Consumes: existing `serde` and `serde_json` workspace dependencies.
- Produces: all types and traits in `Frozen Interfaces`, plus `EventType`, `EventFamily`, `EventSeverity`, `Sensitivity`, `RedactionPolicy`, `ClassifiedField`, `SanitizedField`, `EventOrigin`, `EventLinks`, `ArtifactReference`, `EventQuery`, `SubscriptionCursor`, `ProjectionProfile`, `ProjectedEvent`, and `PersistedEvent`. `EventSeverity` intentionally avoids colliding with the legacy public `Severity = String` alias.

- [x] **Step 1: Add RED tests for raw/sanitized separation and field policies**

Add module tests named:

```rust
raw_event_draft_is_sanitized_before_serialization
secret_and_sensitive_fields_never_survive_sanitization
invalid_keep_policy_for_secret_is_rejected_without_value_in_error
event_family_mismatch_is_rejected
identifier_fields_reject_paths_and_endpoints
```

The secret test injects unique token, account, machine-path, and endpoint strings and asserts none occurs in serialized sanitized JSON or sanitization errors.

- [x] **Step 2: Verify RED**

Run: `cargo test -p actingcommand-contract event::tests -- --nocapture`

Expected: compile failure because `event` types do not exist.

- [x] **Step 3: Implement the minimal event module**

Implement exhaustive v3 event names as `EventType`, grouped by `EventFamily`. `ClassifiedField` constructors enforce:

```text
public   -> keep
internal -> keep | mask | fingerprint | drop
sensitive -> mask | fingerprint | drop
secret   -> fingerprint | drop
```

`EventDraft<P>` deliberately has no `Serialize` implementation. `SanitizedEventDraft<P>` and `PersistedEvent` are serializable. Identifier and module fields accept only bounded ASCII identifiers and reject separators used by local paths or device endpoints.

- [x] **Step 4: Verify GREEN and docs**

Run:

```text
cargo test -p actingcommand-contract event::tests -- --nocapture
cargo test -p actingcommand-contract --doc
cargo clippy -p actingcommand-contract -- -D warnings
```

Expected: all pass; injected originals are absent.

- [x] **Step 5: Commit**

Commit message: `feat(contract): add typed global event contract`

---

### Task 2: Single-Writer Segmented JSONL Storage And Recovery

**Files:**
- Modify: `crates/ledger/Cargo.toml`
- Modify: `crates/ledger/src/lib.rs`
- Create: `crates/ledger/src/global.rs`
- Create: `crates/ledger/src/global/storage.rs`
- Create: `crates/ledger/src/global/tests.rs`

**Interfaces:**
- Consumes: Task 1 sanitized event types; existing `sha2`; standard `File::try_lock` and bounded `sync_channel`.
- Produces: `Sha256FieldRedactor`, `GlobalLedgerConfig`, `GlobalLedger`, `GlobalLedgerError`, `LedgerSubscription`.

- [x] **Step 1: Add RED tests for redactor, ownership, append, recovery, and sequence**

Add tests named:

```rust
sha256_redactor_requires_non_empty_private_salt
second_writer_is_rejected_while_first_is_alive
malformed_writer_metadata_is_fatal
stale_active_owner_is_recovered_explicitly
append_assigns_contiguous_sequences_across_reopen
truncated_final_tail_is_quarantined_and_reported
complete_corrupt_line_is_fatal
non_final_segment_corruption_is_fatal
duplicate_event_id_is_fatal
```

Each failure assertion checks a stable error code and confirms the error display does not include configured root paths or injected secret values.

- [x] **Step 2: Verify RED**

Run: `cargo test -p actingcommand-ledger global::tests -- --nocapture`

Expected: compile failure because `global` storage does not exist.

- [x] **Step 3: Implement writer ownership and recovery**

Use one retained OS file lock at `<root>/writer.lock`. Metadata contains schema version, owner id, pid, active state, and timestamps, but no machine path or endpoint. A locked file yields `writer_conflict`; an unlocked active record yields an explicit `ledger.recovered` event; malformed nonempty metadata is fatal.

Persist events under `<root>/segments/segment-NNNNNN.jsonl`. Serialize a complete event before `write_all`, append one newline, and call `sync_all` before acknowledging. Rebuild sequence and in-memory indexes at startup. Quarantine only bytes after the last newline in the final segment. Any invalid complete line, sequence gap, duplicate sequence, duplicate event id, or corruption outside the final tail is fatal.

- [x] **Step 4: Implement the bounded writer thread and public handle**

`GlobalLedger::open` starts one writer thread and waits for startup recovery. All append/query/subscribe/project/shutdown commands enter one bounded `sync_channel`. Startup or writer death returns a fatal error; no command returns fake empty success when the writer is unavailable.

- [x] **Step 5: Verify GREEN**

Run:

```text
cargo test -p actingcommand-ledger global::tests -- --nocapture
cargo test -p actingcommand-ledger
cargo clippy -p actingcommand-ledger -- -D warnings
```

Expected: all tests pass.

- [x] **Step 6: Commit**

Commit message: `feat(ledger): add recoverable single writer storage`

---

### Task 3: Query, Subscribe, And CLI/UI/Lab Projections

**Files:**
- Modify: `crates/ledger/src/global.rs`
- Modify: `crates/ledger/src/global/storage.rs`
- Create: `crates/ledger/src/global/projection.rs`
- Modify: `crates/ledger/src/global/tests.rs`

**Interfaces:**
- Consumes: Task 2 recovered in-memory event list and indexes.
- Produces: correlation filters, race-free history-plus-live subscription, and named CLI/UI/Lab projections.

- [x] **Step 1: Add RED tests for every required query key and stream ordering**

Add tests named:

```rust
query_filters_by_sequence_and_all_typed_correlation_ids
subscription_replays_after_cursor_then_receives_live_events
cli_projection_is_concise_and_correlated
ui_projection_exposes_sanitized_state_without_secret_fields
lab_projection_exposes_full_sanitized_fact
indexes_rebuild_after_reopen
```

- [x] **Step 2: Verify RED**

Run: `cargo test -p actingcommand-ledger global::tests::query -- --nocapture`

Expected: tests fail because query/subscription/projection behavior is absent.

- [x] **Step 3: Implement query indexes and projections**

Retain the validated recovered event sequence from storage, then build in-memory indexes for `event_id`, `instance_id`, `request_id`, `correlation_id`, `causation_id`, `task_id`, `run_id`, `lease_id`, `frame_id`, `action_id`, and `reco_id`. Query results remain sequence ordered and indexes rebuild after reopen.

Register subscriptions inside the writer command loop after replay events are selected so no live event can race between replay and registration. CLI projection omits detailed payload fields; UI retains sanitized payload and user-facing state; Lab/forensic retains the full sanitized persisted fact. No projection reads raw drafts.

- [x] **Step 4: Verify GREEN**

Run:

```text
cargo test -p actingcommand-ledger global::tests -- --nocapture
cargo test -p actingcommand-ledger
```

Expected: all pass.

- [x] **Step 5: Commit**

Commit message: `feat(ledger): add correlated query and projections`

---

### Task 4: Critical Intent/Action/Outcome Ordering

**Files:**
- Create: `crates/ledger/src/critical.rs`
- Modify: `crates/ledger/src/global.rs`
- Modify: `crates/ledger/src/lib.rs`

**Interfaces:**
- Consumes: erased sanitized event drafts and persisted append receipts.
- Produces: narrow `EventAppender` seam and `execute_critical` orchestration.

- [x] **Step 1: Add RED tests for each failure position**

Add tests named:

```rust
intent_append_failure_prevents_action
successful_action_requires_durable_success_outcome
failed_action_requires_durable_failure_outcome
outcome_append_failure_returns_indeterminate_fatal_without_success_receipt
successful_path_orders_intent_action_outcome
```

Use an in-memory test adapter with an append call counter and an action closure counter; do not mock filesystem behavior covered by Task 2.

- [x] **Step 2: Verify RED**

Run: `cargo test -p actingcommand-ledger critical::tests -- --nocapture`

Expected: compile failure because the module does not exist.

- [x] **Step 3: Implement the minimal ordering module**

`execute_critical` appends sanitized intent first. It invokes the action only after a persisted intent receipt. It then requires a sanitized success or failure outcome. Outcome persistence failure returns a fatal indeterminate-persistence error with a typed `action_performed` flag and never returns success. Implement the narrow `EventAppender` seam for `GlobalLedger` without exposing raw writer internals.

- [x] **Step 4: Verify GREEN**

Run: `cargo test -p actingcommand-ledger critical::tests -- --nocapture`

Expected: all pass with exact call ordering.

- [x] **Step 5: Commit**

Commit message: `feat(ledger): enforce critical event ordering`

---

### Task 5: Process Recovery And Cross-Source Acceptance

**Files:**
- Create: `crates/ledger/tests/global_ledger_process.rs`
- Modify: `tools/actinglab-architecture/tests/workspace_guards.rs`

**Interfaces:**
- Consumes: Tasks 1-4 public interfaces only.
- Produces: process-level proof and dependency-law enforcement.

- [x] **Step 1: Add RED process and architecture tests**

Add tests named:

```rust
hard_killed_writer_releases_os_lock_and_records_recovery
five_sources_share_one_correlated_ledger
secret_injection_is_absent_from_files_queries_errors_and_projections
critical_append_failure_blocks_side_effect
```

The helper test process opens the ledger, writes a ready marker, and waits. The parent uses bounded polling, kills the child, waits for exit, reopens the same ledger, and requires a recovery fact plus contiguous sequence.

The cross-source test writes CLI command, scheduler decision, device input intent/outcome, UI action, and Lab request events with one correlation id and proves one ordered query returns all sources.

Extend workspace guards to prove:

```text
actingcommand-contract has no dependency path to actingcommand-ledger
all non-Lab packages still have no dependency path to actingcommand-lab
```

- [x] **Step 2: Verify RED where applicable**

Run:

```text
cargo test -p actingcommand-ledger --test global_ledger_process -- --nocapture
cargo test -p actingcommand-actinglab-architecture --test workspace_guards -- --nocapture
```

Expected: process acceptance initially fails until the complete public path is wired; unchanged architecture guards remain green.

- [x] **Step 3: Complete only the missing public wiring**

Make the smallest corrections required for process-level use. Do not add daemon, IPC, scheduler state, device execution, or Lab adapters.

- [x] **Step 4: Verify GREEN and forbidden-source scans**

Run:

```text
cargo test -p actingcommand-ledger --test global_ledger_process -- --nocapture
cargo test -p actingcommand-actinglab-architecture --test workspace_guards -- --nocapture
rg -n "EventDraft<" crates/ledger/src
rg -n "actingcommand[_-]lab" crates/actingcommand-contract crates/ledger
```

Expected: tests pass; any `EventDraft<` occurrence in ledger is limited to explicit compile-fail documentation or negative tests, never an ingress signature; no production Lab dependency exists.

- [x] **Step 5: Commit**

Commit message: `test(ledger): prove C1 process recovery and correlation`

---

### Task 6: C1 Full Gate, Documentation, And Remote Evidence

**Files:**
- Modify: `PLANS.md`
- Modify: `CHECKPOINT.md`
- Modify: this plan, marking completed checkboxes.

**Interfaces:**
- Consumes: all C1 commits and tests.
- Produces: reviewed C1 rollback point and Issue #36 evidence comment.

- [ ] **Step 1: Run the full validation gate**

Run in order:

```text
cargo fmt --all
cargo fmt --all -- --check
git diff --check
cargo clippy --workspace -- -D warnings
cargo test --workspace
cargo test --workspace --exclude actingcommand-lab --exclude actingcommand-actinglab
```

Expected: every command exits 0.

- [ ] **Step 2: Run adversarial acceptance scans**

Verify secrets are absent from produced JSONL/quarantine/projection fixtures, C1 does not add SQLite/network/UI/device execution, and legacy A1 goldens remain unchanged. Confirm `cargo tree -i actingcommand-lab` contains only optional Lab roots.

- [ ] **Step 3: Update planning and checkpoint evidence**

Record exact commits, files, commands, test counts, negative-test results, remaining C3a boundary, and any unverified limit. Mark C1 complete only if every v3 C1 delivery and acceptance item has evidence.

- [ ] **Step 4: Commit, tag, and push**

Commit message: `docs(runtime): checkpoint issue 35 C1`

Create checkpoint tag: `checkpoint/20260710-issue35-c1`

Push branch and tag to `origin`.

- [ ] **Step 5: Comment Issue #36**

Record C1 scope, exclusions, source commits, checkpoint tag, full gates, negative secret-injection result, process recovery result, blockers, and next C3a step. Do not create another Issue.

---

## Self-Review

- Spec coverage: typed envelope, four seed families, device/client acceptance families, sensitivity schema, durable pre-act intent, post-act outcome, append/query/subscribe/project, CLI/UI/Lab projections, single writer, segmented recovery, correlation, sequence, severity, and artifact references all map to Tasks 1-5.
- Security coverage: raw drafts are nonserializable; only sanitized drafts reach ingress; four secret classes are injected into file/query/error/projection negative tests.
- Failure coverage: writer contention, stale owner, hard kill, malformed metadata, truncated final tail, mid-file corruption, duplicate identity, append-before-act failure, and outcome persistence failure are explicit tests.
- Scope coverage: C1 does not introduce daemon, scheduler behavior, DeviceProxy, artifact storage, Lab migration, UI, resources, SQLite, or game logic.
- Type consistency: Task 1 produces the exact sanitized/event/query/projection types consumed by Tasks 2-5; Task 4 uses erased sanitized drafts and the same persisted receipt as `GlobalLedger`.
- Placeholder scan: no implementation step contains deferred placeholders; later-phase work is explicitly excluded rather than represented by empty interfaces.
