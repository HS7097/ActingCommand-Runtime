# C1 Global Ledger Hardening Design

Status: approved by Alice on 2026-07-11 as a narrow correction inside Issue #35 C1.

Authority:

- GitHub Issue #35, authored by `HS7097`;
- `TASK-runtime-ledger-core-and-optional-lab-correction-v3.md`;
- v3 SHA-256 `28273b85491b0d43aa7a7b7a7ece10db681de9df4d9100e85f9e9b086dd107a6`;
- C0 frozen payload SHA-256 `6c72a9c39ff67ec5a2868e0ed262d2a2f0a2c4b0fbfc473b7c55a9df610bf0a7`;
- the whole-C1 review recorded at commit `a92aa7c`.

This correction does not create a new top-level phase. It closes the C1 security, persistence, subscription, and lifecycle defects before C3a starts.

## Goals

1. Make pre-persistence redaction schema-owned instead of producer-selected.
2. Keep every persisted and projected payload typed; remove public `serde_json::Value` payload surfaces.
3. Make persisted facts ledger-owned and non-constructible by ordinary consumers.
4. complete the artifact-reference contract required by v3 section 5.1.
5. Make critical outcomes reflect the actual post-action result and effect disposition.
6. State truthfully that C1 orders one invocation but does not provide cross-invocation exactly-once execution.
7. Bound replay work and memory while preserving gap-free replay-plus-live ordering.
8. Make subscription terminal states absorbing.
9. Prevent startup failure from leaving a detached future ledger owner.
10. Make destructive tail repair and its recovery fact process-crash atomic.
11. Close the owner-clock, empty-segment, and diagnostic disclosure gaps.

## Non-Goals

- No scheduler, daemon IPC, DeviceProxy, fencing, task lifecycle, UI, game logic, recognition, OCR, SQLite, artifact bytes, retention implementation, or resource data.
- No automatic replay, retry, reconciliation, compensation, or action-id deduplication.
- No compatibility migration that trusts the unapproved generic v1 event payload format.
- No process-global panic-hook ownership in a library crate.
- No new production dependency.

## Event Contract V2

The C1 event schema becomes `actingcommand.event.v2`. Old v1 segment data fails loudly as an unsupported schema. C1 has no production consumers or approved durable v1 data, so silently translating producer-selected v1 classifications would preserve the security flaw.

### Schema-Owned Payloads

Remove the public generic combination of `StructuredPayloadDraft` and caller-selected `ClassifiedField` policies. Contract-owned payload variants define their fields and redaction behavior.

The C1 typed payload set covers:

- command received, validated, and rejected;
- scheduler admitted, queued, denied, and preempted;
- lease requested, granted, transferred, released, and expired;
- task requested, started, step started, step finished, completed, failed, and cancelled;
- input intent, committed, completed, and failed;
- UI action, CLI command, and Lab request;
- ledger recovery;
- lease transition intent/failure and task terminal intent/commit failure needed by critical ordering.

Each raw draft variant accepts only semantic typed fields. Public labels and codes are validated static identifiers or enums. Runtime-provided token, account, machine-path, endpoint, and free-form diagnostic values enter dedicated raw sensitive types whose policy is fixed by the contract:

- authentication material: drop;
- account identity: fingerprint;
- machine-local path: mask;
- device/network endpoint: mask;
- allowed internal diagnostic code: validated identifier;
- counts, booleans, timestamps, and effect disposition: typed scalar or enum.

The caller cannot choose `Sensitivity` or `RedactionPolicy` for a runtime value.

### Fingerprinting

`SecretFingerprinter` receives a schema-owned `SecretField` enum, not a caller-provided field name. A fingerprint is accepted only when it is canonical `sha256:<64 lowercase hex>`, differs from the original string, and its digest portion also differs from the original string. This rejects both raw-digest echo and canonical-hash-shaped echo.

### Envelope Identifiers

Envelope IDs and links use validated newtypes. Runtime-derived identifiers are created from fixed prefixes plus opaque bytes; local paths and endpoints are never accepted as IDs. Origin module and payload action names are validated static codes rather than arbitrary runtime strings. Debug output never prints unvalidated identifiers.

### Typed Sanitized Payload

Sanitization produces one closed `EventPayload` enum. `SanitizedEventDraft` is serializable but not deserializable or directly constructible. There is no public erased `serde_json::Value` ingress.

CLI/UI/Lab projection data uses a closed `ProjectionPayload` enum:

- `Omitted` for concise profiles;
- `Public` for public user-facing fields;
- `Full` for the complete already-sanitized typed payload.

## Ledger-Owned Persisted Facts

`PersistedEvent` moves to `actingcommand-ledger`. Its fields are private, it implements `Serialize` but not `Deserialize`, and only ledger storage can construct it. Public access uses getters.

Recovery deserializes a private `StoredEventRecord`, rejects duplicate keys and unsupported schemas, validates all typed payload and artifact invariants, then constructs the opaque persisted fact. Ordinary consumers cannot assign a sequence or manufacture a persisted-looking fact.

`EventQuery` remains a storage-independent contract DTO. Matching against persisted facts moves into ledger internals. Architecture guards reject:

- a contract dependency on ledger;
- public `serde_json::Value` payload fields;
- a public persisted-event constructor;
- `Deserialize` on ledger-owned `PersistedEvent`;
- raw event drafts at ledger ingress.

## Artifact Reference V2

`ArtifactReference` has private validated fields for:

- artifact ID and kind;
- optional run, frame, and correlation IDs;
- store-issued relative object key;
- media type;
- byte count;
- SHA-256;
- creation timestamp;
- producer module;
- retention class;
- redaction state.

The object key is an opaque relative store key, not a filesystem path. CLI/UI projections omit object keys; Lab/forensic projections may include the validated key. C2 will implement storage and retention behavior against this frozen reference.

## Critical Action Semantics

The critical executor follows:

```text
typed intent
  -> sanitize
  -> durable append
  -> invoke action once in this call
  -> action reports result plus effect disposition
  -> selected typed outcome builder observes the real result
  -> sanitize and validate outcome role/links
  -> durable append
  -> receipt
```

`EffectDisposition` is `not_performed`, `performed`, or `indeterminate`. Successful actions report a definite disposition. Failed actions may report any disposition. Outcome build, sanitization, role validation, or append failure after action returns a fatal `outcome_undurable` error carrying only a safe stage and disposition; it never returns success.

The operation map covers:

- command validation;
- device write;
- lease grant/transfer/release/expiry via typed lease transition intent/failure events;
- task terminal completion/failure/cancellation via typed task terminal intent/commit-failure events.

`input.completed` remains a later semantic completion fact, not the immediate backend-write commit outcome.

C1 guarantees one action invocation per call after a durable intent. It does not deduplicate repeated calls with the same action ID and does not claim exactly-once behavior across crashes or retries. C3a owns request idempotency, owner-epoch fencing, pending-intent reconciliation, and production recovery decisions.

The ledger library does not install, replace, or restore a process panic hook. An action panic propagates after the durable intent and leaves no guessed outcome; the unresolved intent is explicit reconciliation input for C3a. The Runtime process boundary will own one permanent redacting panic policy. Critical receipts and errors use custom non-disclosing `Debug` and `Display` implementations that never format action values, raw errors, payloads, paths, endpoints, or panic payloads.

## Bounded Subscription Replay

Keep `GlobalLedger::subscribe(cursor)` as a compatibility wrapper and add `subscribe_with_options(cursor, options)`. `SubscriptionOptions` validates `replay_page_events` in `1..=1024`, defaulting to 256.

Registration atomically captures replay high-water sequence `S` and registers live delivery only for sequences greater than `S`. The subscription pulls replay pages `(cursor, S]` through bounded writer commands. Each command clones at most one page, so writer work and transient replay memory are bounded. Live delivery remains bounded by the configured live channel.

The resulting sequence is exactly:

```text
(cursor, S] replay, in order
then
(S, infinity) live, in order
```

`LedgerSubscription` exposes the cursor after the last event returned and the fixed replay high-water mark. A fatal lag, replay failure, or writer failure transitions the subscription to an absorbing terminal state, clears buffered events, and returns the same terminal error on every later receive. Clean closure is also latched. A timeout is not terminal.

## Owned Startup

A writer thread is spawned first but blocks on a one-shot store receiver and has no filesystem authority. `GlobalLedger::open` then opens the store and acquires ownership synchronously on the caller thread. Store-open failure closes the one-shot channel and joins the waiting writer before returning. Successful open transfers the owned store to the writer; transfer failure recovers and closes the store and joins the writer. `GlobalLedger::open` therefore cannot return while a detached worker may later acquire the lock or mutate the root.

The old startup-timeout path is removed rather than emulated with a cancellable detached initializer. C1 does not promise a startup deadline; filesystem failures remain explicit fatal errors.

## Crash-Atomic Tail Repair

Before any destructive final-tail repair, storage writes and syncs a deterministic pending repair record containing:

- repair ID derived from segment index, original length, repaired length, and tail SHA-256;
- segment index and both lengths;
- quarantine object key and tail hash;
- state `prepared` or `completed`.

The process sequence is:

```text
prepare marker durable
  -> quarantine tail durable
  -> truncate and sync segment
  -> append deterministic ledger.recovered event durable
  -> mark repair completed durable
```

On restart, prepared records resume idempotently. Segment length must equal the original or repaired length; any third state is fatal. A deterministic recovery event ID prevents duplication. If the event is already present, startup verifies it and completes the marker. A successful open leaves no unresolved prepared repair.

The recovery journal uses `actingcommand.ledger-repair.v1`; writer metadata uses `actingcommand.ledger-writer.v2`. Unapproved v1 writer metadata and generic v1 event roots fail loudly instead of being treated as fully trusted.

## Remaining Storage Invariants

- Wall-clock start and close timestamps are nonzero diagnostic observations; close time is not required to be greater than start time.
- An empty segment is legal only when it is the final active segment. An empty non-final segment is fatal corruption.
- `GlobalLedgerConfig::Debug` prints constant placeholders for both root and owner ID.
- Severe repair, validation, sync, or ownership failures propagate; no recovery path silently skips evidence.

## Verification

The hardening is accepted only when all of the following pass:

1. Contract adversarial tests inject token, account, machine path, and endpoint values through every permitted raw sensitive field and attempt alternate envelope channels.
2. Canonical hash-shaped originals cannot survive as fingerprints.
3. Compile/architecture guards prove typed ingress, opaque persisted facts, and dependency direction.
4. Critical tests cover success/failure builders, effect disposition, outcome sanitization/append failure, lease/task role maps, repeated action IDs, panic propagation, and non-disclosing diagnostics.
5. Subscription tests cover paging, writer responsiveness, replay/live ordering, lag latching, terminal priority, resume cursor, and future cursors.
6. Process tests cover immediate reopen after failed startup and kill points across pending repair, quarantine, truncation, recovery append, and completion.
7. Recovery tests cover backward wall-clock data and empty non-final segments.
8. Existing legacy `LabLedger`, protocol goldens, full workspace tests, Clippy, formatting, forbidden-source scans, and dependency guards remain green.
9. A fresh whole-C1 review reports no Critical or Important findings before C1 closeout.
