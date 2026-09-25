# Scheduling Catalog Contract

The versioned [V2 contract](v2/README.md) adds explicit timeline availability
through the same compiler and evaluator. This document specifies V1.

The scheduling catalog is a data-only contract of four required documents plus one optional selection document. It cannot contain executable code, scripts, network requests, device actions, or implicit defaults.

## Offline inspection

`actinglab scheduling compile --tasks <tasks.json> --pools <pools.json>
--activity <activity.json> --timeline <timeline.json>` reads four explicit local
files through Lab and calls the existing policy `compile_catalog` entrypoint.
With `--json`, one ordinary CLI envelope contains the compiler's `dry_run_json`
report, including summary and catalog hash. Rejection returns exit 2 and the
original structured compiler diagnostics under `error.details`. File read errors
also fail explicitly. V1 compilation and catalog identities retain their contract.

`actinglab scheduling timeline` takes those same file flags plus repeated
`--event-id <id>`, `--unix-ms <u64>`, `--monotonic-ms <u64>`,
`--instance-id <id>`, `--server-id <id>` and `--game-id <id>`. All are explicit;
instance context must correspond to the configuration being inspected. The
command reads no system clock, Runtime configuration, session, facts, capabilities
or task state. Routing/backend/execution global flags are rejected. Both commands
are `offline` and `read_only` and create no state directory or persistent facts.

The timeline response contains the compilation report and the pure policy query:
catalog ID/version/hash, supplied time and context, requested events in request
order, each event's declared scope and `scope_applies`, `availability.state`
(`true`, `false` or `unknown`), `active_interval` as `[start, end]` Unix milliseconds
representing the half-open interval `[start,end)`, and `next_wake_unix_ms`.
An inactive event has a null active interval. The top-level query wake is the
minimum wake across the selected events; it is not the full scheduler's wake.
Unknown or duplicate event IDs and empty selections are input errors.

Policy `inspect_timeline` shares the availability and scope owner used by
`TimelineActive`, including its existing clock, duration and validity semantics.
Positive availability requires explicit V2 validity. Zero-duration reset events
remain unknown for availability, including V1 reset events; their production
invalidation purpose is unchanged. Applicable closed/expired events are false,
and inapplicable scopes are unknown. Arithmetic failures retain policy errors.

Each source is limited to 1 MiB, and the compiler enforces the 4 MiB catalog bound.
The reader uses at most one additional sentinel byte per document to preserve the
compiler's size diagnostic. Paths use the existing 1024-byte source-text bound;
server/game context and event IDs use 128 bytes; the instance context uses the
registered 256-byte alias rule. Selection is limited to 4096 events. CLI query
arguments are bounded to 8210 tokens and 1 MiB, enough for all events and nine
single-value flags. Output payloads are limited to 16 MiB (four catalog budgets)
to allow structured source locations and diagnostics. Exceeding a bound fails
visibly without silently truncating a decision.

These commands verify compilation and event availability at a supplied time.
Production catalog activation, PolicyHost binding, admission and execution remain
the Runtime's normal chain; an offline query establishes no production run fact.

## Frozen V1 Documents

- `tasks.schema.json`: task entrypoints, bounded triggers, feedback stop conditions, effects, failure policy, load profile, loop budget, and instance overrides.
- `pools.schema.json`: scoped resource pools, regeneration projections, observations, and bounded group delay.
- `activity.schema.json`: scoped activity windows, per-instance importance, bounded sessions, sampling policy, and goals.
- `timeline.schema.json`: scoped reset, maintenance, activity, and deadline events.
- `diagnostic.schema.json`: stable compiler diagnostic envelope.
- `selection` (optional fifth document): an `actingcommand.selection-policy.v1` scoring
  policy owned by [the selection-policy contract](../selection-policy.md). It carries its
  own schema version and no `catalog` descriptor.

All four catalog documents must carry the exact schema version `actingcommand.scheduling.v1` and an identical `catalog` descriptor. A mismatch rejects the whole catalog.

## Compatibility

V1 rejects unknown fields and any schema version other than the exact supported value. A future schema revision is a new immutable contract and requires an explicit, separately tested translator before V1 input can be promoted. Readers must never reinterpret a V1 field under newer semantics, partially load a catalog, or invent defaults for missing data.

The `env.*` namespace remains an ordinary fact-key family. Existing execution substitutions that use `{env:...}` remain an execution-boundary concern; scheduling predicates reference the same stored values through typed `fact` predicates without changing that substitution syntax.

## Scope And Overrides

Runtime evaluation replaces task runtime snapshots with its replayed admitted and
completed dispatches at the input ledger position. The latest admitted dispatch
supplies `last_dispatched_unix_ms`; a completed dispatch supplies its recorded
execution success/failure. This is scheduling execution state, not a business
effect or a catalog outcome key. No observed eligibility-start time is invented.

Before assigning an instance winner, Runtime consults the existing control owner's
read-only admission predicate for task/activity daily, window and runtime budgets,
activity windows and sampled intervals. Unavailable candidates leave the instance
and host capacity available to the next ranked task. The same predicate supplies
the next possible admission time at bounded weekly window/cadence boundaries.
Final admission still checks and commits the actual counters under the original lock.

`policy.dispatch_rejected` retains its immutable ranking reasons and optionally
carries `rejection`: the original Runtime error code, operation and fatal flag,
the decisive budget dimension with used/requested/limit values when applicable,
and the next eligible Unix time when one is available. These are bounded typed
control facts; native text and secrets are not copied into ranking or rejection
details. Existing events without rejection details remain unchanged.

Every task, pool, activity profile, and timeline event declares `instance`, `server`, or `game` scope. `instance_overrides` is the only V1 task override layer. A null override field means inherit the catalog value; it does not mean zero, false, or an inferred default. Activity profiles scoped to an instance carry its importance and goals.

The `instance_id` in instance scopes and overrides stores the exact registered
instance alias: 1–256 UTF-8 bytes, with Unicode control characters prohibited.
Case, permitted whitespace (including all-space aliases), and Unicode bytes are
preserved through compilation, facts, evaluation, admission and registry lookup.
The Runtime's typed `InstanceId` continues to identify links and leases. Catalog,
task, pool, server and game identifiers and references retain their own rules.
JSON Schema bounds alias character length; the shared contract/compiler also
enforces the UTF-8 byte limit.

Activity sampling uses a ledger-derived seed and `same_round_stable`: the host records the seed once and must reuse the sampled value throughout the same scheduling round. Resampling within a round is invalid.

## Score-Assisted Priority

The predicate gate (trigger, feedback stop, timeline validity, cooldown, placement)
decides eligibility on its own; a score never overrides it. Between that gate and
ranking, the evaluator runs an optional score stage:

- With a `selection` document, every eligible (task, instance) pair becomes one
  selection candidate `<task_id>@<instance_id>` whose fields are `task.priority`,
  `task.strategic_weight_milli`, `task.urgency_milli`, `task.aging_ms`,
  `task.load_cost_milli` (integers), `instance.affinity` (boolean), and every
  scalar fact the instance can see (its own, its server's and its game's scope,
  the most specific scope per key; timestamps and durations become integer
  milliseconds; record lists are unusable). The document is evaluated once per
  instance over that instance's candidates and its own fact projection, pinned to
  the evaluation's `fact_snapshot_id` and instant. Only the per-candidate
  `score_milli` of a `ranked` verdict is consumed; a rejected or dropped verdict
  yields no score, and an `unknown` outcome (`abort_evaluation`) fails the whole
  evaluation with `selection_evaluation_aborted`. A document the crate cannot
  evaluate fails with `selection_policy_invalid`.
- `EvaluationFacts.priority_offsets` carries manual offsets
  `{task_id, instance_id?, offset_milli, origin: user|agent, observed_at_unix_ms}`;
  an instance-level entry overrides the task-level entry, `|offset_milli|` is
  bounded to 1,000,000, and duplicate `(task_id, instance_id)` pairs are input
  errors. Offsets apply with or without a selection document.
- `effective_milli = score_milli (0 when absent) + offset_milli` is added to the
  candidate's `total_score` as `effective_milli * 1000`, the same unit as the
  urgency and strategic terms.
- The tasks document may declare a catalog-level `priority_selection` block
  `{defer_below_milli, defer_for_ms (1..=86,400,000), promote_above_milli}` with
  `defer_below_milli < promote_above_milli`; it is only allowed next to a
  selection document (`priority_selection_without_document` otherwise). A scored
  candidate with `effective_milli < defer_below_milli` is deferred with reason
  `score_deferred` and wakes at `now + defer_for_ms`, consuming no host budget;
  one with `effective_milli > promote_above_milli` is promoted and ranks ahead of
  every other candidate (`score_promoted`). An unscored candidate is never
  deferred or promoted: only its offset applies and the reason
  `score_unknown:<gate or term id>` records why.
- Every candidate that passed through the stage carries the reason `scored`
  with detail `score=<milli|none> offset=<milli> effective=<milli>
  disposition=<deferred|promoted|none>`. Reason codes are free strings; the
  dispatch-intent shape and the decision identity are unchanged.

Without a selection document and without offsets the stage is a no-op and every
evaluation output is byte-identical to a catalog compiled before this stage
existed.

## Runtime Enforcement

Time validity is evaluated over the pinned input snapshot. Timeline invalidation
matches an observation's key prefix and scope (the same scope or a containing
server/game scope). Outcome keys are `outcome.<task_id>.<outcome_key>`; settled
execution state without mapped outcomes uses `task.<task_id>.terminal_state`.
Only schedule occurrences inside the event's V2 half-open validity interval
invalidate observations. V1 occurrences are unbounded. Duration controls
availability, including zero-duration resets, and does not undo invalidation.
An observation at or before the last matching reset is Unknown until replaced
by a newer observation. Equal timestamps are conservative because the input
contains no ordering evidence within that millisecond.

`ObservedOutcome.expires_at_unix_ms` is optional and uses the fact TTL convention:
the expiry millisecond is valid and expiry + 1 wakes reevaluation. Matching future
resets cap admission freshness at occurrence - 1 and wake at the occurrence.
Ledger-derived pools consume the same effective fact projection.

The first dispatch depends on `trigger`. `feedback_stop` is evaluated only after
a valid result for the same task and instance in the current activity window.
`ObservedOutcome.activity_window_id` comes from the settled run's admission;
`TaskRuntimeSnapshot.completed_window` carries the settled window ID and completion
timestamp for tasks with no mapped outcome consumer. Window identity follows the
selected activity profile and its first active declared window, including its
local start day for windows crossing midnight. A result must both name that
window and have completed inside it. An enabled stop predicate retains Unknown
when its required observation is unavailable. Missing historical window fields
default to `None`, which supplies no evidence for feedback stop; a missing outcome
expiry supplies no additional TTL. An enabled feedback stop's activity-window
closure also bounds freshness and wakes reevaluation. These input projections add no ledger wire or stored
transaction state.

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

The catalog contains the effective DST offset instead of consulting a hidden timezone database. The base UTC offset is bounded to `[-840, 840]` minutes and the explicit DST offset is independently bounded to `[-120, 120]` minutes; their effective sum is therefore bounded to `[-960, 960]` minutes. A DST transition, server-clock correction, reveal change, or maintenance delay creates a new immutable catalog generation and triggers full recomputation. `maintenance_drift_ms` shifts nominal occurrences and is bounded to seven days. Calendar and absolute schedules cannot use `local`, because monotonic coordinates do not identify wall-clock instants and are not portable across host boot epochs.

`next_wake_unix_ms` remains the earliest lower bound across timeline and predicate wakes. When a task wake is known, `preload_hint` pairs that lower bound with the task ID, its `procedure_ref` as `package_ref`, and an explicit confidence. A preload hint is advisory only and never constitutes admission or execution permission.

## Runtime Boundary

V1 persistent scheduling state is single-host, local-filesystem state. Catalog generations, active pointers, ledgers, leases, budget journals, fact projections, approval projections, and release-set pointers must not be shared concurrently by independent hosts. A future multi-host revision must add host identity and fencing to every one of those owners instead of partially extending the V1 files.

Cross-run decisions are reevaluated by the scheduler after each outcome. A bounded rule table may make mechanical choices inside one run, but it cannot call back into the scheduler for mutable external state. External state required by a run must be pinned into dispatch parameters and its reason chain before admission.

Scheduled contained admission carries the validated, package-bound task request
into the existing lease acquisition. Its TTL uses the request's response budget
plus the same heartbeat/closure reserve as direct task execution, with the
Scheduler's existing maximum and checked arithmetic. The declared task timeout,
request deadline, lease boundary and execution permissions still constrain the
run. Lease expiry and the effective task deadline retain their typed ledger
records. Planning duration remains a reservation estimate; actual runtime usage
is recorded by the existing completion/failure owner.

For a scheduled contained task, an empty catalog outcome-reference set means
there is no scheduling-result consumer. The package may declare and produce its
own valid outcomes; its actual final page, designated effect and terminal
`scheduling_disposition` remain in GlobalLedger. Completion and recovery create
no policy outcome projection or completed-run consumption identity in this case.
When the reference set is nonempty, it must exactly match the package's declared
outcome keys. Package validity and execution evidence requirements apply in both
cases.

## Forward Planning And Maintenance

Forward planning is a bounded dry-run of the same pure evaluator used for live policy decisions. It projects at most 24 hours, performs no ledger write, lease operation, execution, or device action, and reports incomplete evidence instead of inventing resource effects. This is a projection facility, not another scheduler.

Predictive maintenance compares ledger-pinned execution duration and fact-confidence trends within an explicit lookback window. Both evidence series must meet their declared sample minimum before a recheck can be suggested. Missing evidence produces an `evidence_insufficient` assessment and no planning signal.

## Bounds

The compiler enforces both schema limits and UTF-8 byte limits:

| Item | Limit |
| --- | ---: |
| One document | 1,048,576 bytes |
| Selection document | 524,288 bytes |
| Catalog (all documents together) | 4,194,304 bytes |
| Priority offsets per evaluation | 16,384, each within ±1,000,000 milli |
| Identifier/reference | 128 bytes |
| Registered instance alias | 256 UTF-8 bytes |
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

1. Parse all four documents while rejecting duplicate object keys and invalid UTF-8. Parse the optional selection document with the selection-policy crate's own reader and validation.
2. Validate the exact V1 schemas and cross-document invariants.
3. Construct the JSON object `{"activity": A, "pools": P, "tasks": T, "timeline": L}` from the validated documents, adding `"selection": S` only when a selection document is present. No field is removed and no default is inserted, so a catalog without a selection document keeps its existing hash.
4. Serialize that object with RFC 8785 JSON Canonicalization Scheme. Object keys are sorted by JCS rules, array order is preserved, and no insignificant whitespace is emitted.
5. Compute SHA-256 over the canonical UTF-8 bytes.
6. Encode the result as `sha256:` followed by 64 lowercase hexadecimal characters.

Catalog producers must emit semantically unordered arrays in deterministic order. Because array order is preserved, reordering any array changes the hash. Approval references authorize the exact catalog version and hash; they are not executable permissions by themselves.

## Diagnostics

Compiler failures use `CatalogDiagnostic`. `code` is stable within V1; `reason` is human-readable and must not be parsed for control flow. `json_path` uses RFC 6901 JSON Pointer. `source` identifies the document, source URI, and one-based line and column. Version and catalog fields may be null only when malformed input prevents their extraction.

Any error-severity diagnostic rejects the complete four-document catalog. Warnings may accompany a successful dry-run but cannot conceal an error. The compiler must sort diagnostics deterministically by document, source position, code, and JSON path.

## Compiler Boundary

`actingcommand_policy::compile_catalog` accepts four in-memory `CatalogDocumentSource` values plus an optional fifth for the selection document. It performs no file access, script execution, network request, clock read, sleep, ledger write, lease operation, or device action. Success returns one complete `CompiledCatalog`; any error returns `CatalogCompileFailure` and no partial IR. Both outcomes expose canonical, byte-stable dry-run JSON.

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
