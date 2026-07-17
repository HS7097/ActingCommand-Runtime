# Scheduling Catalog Contract

The scheduling catalog is a four-document, data-only contract. It cannot contain executable code, scripts, network requests, device actions, or implicit defaults.

## Frozen V1 Documents

- `tasks.schema.json`: task entrypoints, bounded triggers, feedback stop conditions, effects, failure policy, load profile, loop budget, and instance overrides.
- `pools.schema.json`: scoped resource pools, regeneration projections, observations, and bounded group delay.
- `activity.schema.json`: scoped activity windows, per-instance importance, bounded sessions, sampling policy, and goals.
- `timeline.schema.json`: scoped reset, maintenance, activity, and deadline events.
- `diagnostic.schema.json`: stable compiler diagnostic envelope.

All four catalog documents must carry the exact schema version `actingcommand.scheduling.v1` and an identical `catalog` descriptor. A mismatch rejects the whole catalog.

## Compatibility

V1 rejects unknown fields and any schema version other than the exact supported value. A future schema revision is a new immutable contract and requires an explicit, separately tested translator before V1 input can be promoted. Readers must never reinterpret a V1 field under newer semantics, partially load a catalog, or invent defaults for missing data.

The `env.*` namespace remains an ordinary fact-key family. Existing execution substitutions that use `{env:...}` remain an execution-boundary concern; scheduling predicates reference the same stored values through typed `fact` predicates without changing that substitution syntax.

## Scope And Overrides

Every task, pool, activity profile, and timeline event declares `instance`, `server`, or `game` scope. `instance_overrides` is the only V1 task override layer. A null override field means inherit the catalog value; it does not mean zero, false, or an inferred default. Activity profiles scoped to an instance carry its importance and goals.

Activity sampling uses a ledger-derived seed and `same_round_stable`: the host records the seed once and must reuse the sampled value throughout the same scheduling round. Resampling within a round is invalid.

## Runtime Enforcement

The evaluator pins the selected activity profile in every dispatch intent. Runtime owns activity sampling, budget counters, retry state, and failure escalation; callers cannot supply remaining-budget values. Admission and execution ledger events record the selected profile, sample seed, activity window, cadence, cumulative task and activity budget receipts, and classified outcome.

Recoverable failures receive a positive, bounded backoff. Only repeated failures with the same error code and failure class share a consecutive-failure streak, and sensitive or severe failures are never automatically restarted. Goal-missed, feasibility-red, and drift-predicted signals are informational planning facts: they do not consume failure tax, advance a failure streak, or pause execution.

## Task Execution Fields

`entrypoint.operation_id` names the Runtime mechanism capability that may execute the task. `procedure_ref` is the immutable external package or procedure-definition identity pinned into the dispatch intent and the immutable catalog generation referenced by its reason chain. It is not a file path, script, executable capability, or approval authority; Runtime must resolve it through the approved adapter/package boundary and reject any mismatch at admission.

Before an intent becomes trusted, a Runtime-owned manifest content-addressably binds `procedure_ref`, the package SHA-256 digest, `operation_id`, and the ordered `yield_points`. The binding digest participates in decision identity and is persisted with dispatch events. Admission resolves the alias again and requires the exact package and binding digests before requesting a lease.

`expected_duration_ms` is the declared reservation and planning duration. `cooldown_ms` is the minimum interval after the last dispatch before the task can become eligible again. `next_run_clamp_ms` caps recoverable retry delay. `yield_points` names the only package-declared safe cooperation points that may be exposed to a mechanism adapter; it never grants a new operation. `sensitive` disables automatic restart after failure and does not weaken fatal-error propagation.

## Clock Sources

Every clock schedule declares exactly one source:

- `local` uses the host-provided monotonic coordinate and is valid only for interval schedules. The evaluator projects its next occurrence back to Unix time for transport.
- `server` uses a pinned timezone identity, base UTC offset, explicit DST offset, and bounded maintenance drift.
- `reveal` has the same calendar fields plus `reveal_source`, the immutable evidence identity from which the catalog author derived the pinned schedule.

The catalog contains the effective DST offset instead of consulting a hidden timezone database. A DST transition, server-clock correction, reveal change, or maintenance delay therefore creates a new immutable catalog generation and triggers full recomputation. `maintenance_drift_ms` shifts nominal occurrences and is bounded to seven days. Calendar and absolute schedules cannot use `local`, because monotonic coordinates do not identify wall-clock instants and are not portable across host boot epochs.

`next_wake_unix_ms` remains the earliest lower bound across timeline and predicate wakes. When a task wake is known, `preload_hint` pairs that lower bound with the task ID, its `procedure_ref` as `package_ref`, and an explicit confidence. A preload hint is advisory only and never constitutes admission or execution permission.

## Runtime Boundary

V1 persistent scheduling state is single-host, local-filesystem state. Catalog generations, active pointers, ledgers, leases, budget journals, fact projections, approval projections, and release-set pointers must not be shared concurrently by independent hosts. A future multi-host revision must add host identity and fencing to every one of those owners instead of partially extending the V1 files.

Cross-run decisions are reevaluated by the scheduler after each outcome. A bounded rule table may make mechanical choices inside one run, but it cannot call back into the scheduler for mutable external state. External state required by a run must be pinned into dispatch parameters and its reason chain before admission.

## Forward Planning And Maintenance

Forward planning is a bounded dry-run of the same pure evaluator used for live policy decisions. It projects at most 24 hours, performs no ledger write, lease operation, execution, or device action, and reports incomplete evidence instead of inventing resource effects. This is a projection facility, not another scheduler.

Predictive maintenance compares ledger-pinned execution duration and fact-confidence trends within an explicit lookback window. Both evidence series must meet their declared sample minimum before a recheck can be suggested. Missing evidence produces an `evidence_insufficient` assessment and no planning signal.

## Bounds

The compiler enforces both schema limits and UTF-8 byte limits:

| Item | Limit |
| --- | ---: |
| One document | 1,048,576 bytes |
| Four-document catalog | 4,194,304 bytes |
| Identifier/reference | 128 bytes |
| Diagnostic text, fact string, or source URI | 1,024 bytes |
| Approval references | 64 |
| Tasks | 4,096 |
| Pools | 1,024 |
| Activity profiles | 1,024 |
| Timeline events | 4,096 |
| Predicate depth | 16 |
| Predicate nodes per root | 512 |
| Effects, references, or instance overrides per task | 128 each |
| Windows or goals per activity profile | 128 each |

Loop budgets are mandatory. Arrays and strings that exceed their limit reject the entire catalog. Duplicate identifiers, duplicate object keys, and unbounded recursive input are invalid.

## Canonical Serialization And Hash

The catalog hash is computed as follows:

1. Parse all four documents while rejecting duplicate object keys and invalid UTF-8.
2. Validate the exact V1 schemas and cross-document invariants.
3. Construct the JSON object `{"activity": A, "pools": P, "tasks": T, "timeline": L}` from the validated documents. No field is removed and no default is inserted.
4. Serialize that object with RFC 8785 JSON Canonicalization Scheme. Object keys are sorted by JCS rules, array order is preserved, and no insignificant whitespace is emitted.
5. Compute SHA-256 over the canonical UTF-8 bytes.
6. Encode the result as `sha256:` followed by 64 lowercase hexadecimal characters.

Catalog producers must emit semantically unordered arrays in deterministic order. Because array order is preserved, reordering any array changes the hash. Approval references authorize the exact catalog version and hash; they are not executable permissions by themselves.

## Diagnostics

Compiler failures use `CatalogDiagnostic`. `code` is stable within V1; `reason` is human-readable and must not be parsed for control flow. `json_path` uses RFC 6901 JSON Pointer. `source` identifies the document, source URI, and one-based line and column. Version and catalog fields may be null only when malformed input prevents their extraction.

Any error-severity diagnostic rejects the complete four-document catalog. Warnings may accompany a successful dry-run but cannot conceal an error. The compiler must sort diagnostics deterministically by document, source position, code, and JSON path.

## Compiler Boundary

`actingcommand_policy::compile_catalog` accepts four in-memory `CatalogDocumentSource` values. It performs no file access, script execution, network request, clock read, sleep, ledger write, lease operation, or device action. Success returns one complete `CompiledCatalog`; any error returns `CatalogCompileFailure` and no partial IR. Both outcomes expose canonical, byte-stable dry-run JSON.

## Neutral Example

`examples/catalog-a` is a synthetic, product-neutral catalog. It exercises all four documents without embedding external project, game, account, device, or private workflow data.

The example is the V1 upgrade map for the earlier `task-catalog.v0-draft` shape:

| Draft concept | V1 location |
| --- | --- |
| task identifier and operation | `tasks[].id`, `tasks[].entrypoint` |
| start condition | `tasks[].trigger` |
| feedback termination | `tasks[].feedback_stop` |
| resource assumptions | typed `tasks[].consumes` and `tasks[].produces` plus `pools[]` |
| retry behavior | `tasks[].on_failure` |
| loop and session bounds | `tasks[].loop_budget` and `activity.profiles[]` |
| schedule/reset/deadline data | `activity.profiles[].windows` and `timeline.events[]` |
| instance-specific priority | `tasks[].instance_overrides` |
| instance importance and targets | instance-scoped `activity.profiles[].importance_milli` and `goals` |

Draft fields that relied on runtime defaults or implementation-specific behavior have no implicit V1 mapping and must be supplied explicitly.
