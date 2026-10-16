# Resource readings

Workflow #335 S5. A package declares, per resource, where the Runtime reads its current amount
during a task: which page, which OCR target, how the text is parsed, the lowest confidence it
accepts, how long the reading stays valid and the instance fact key it becomes. Such a
declaration is a reading mark (Workflow #384 D2-S2): it may stand on any page of the task, the
whole page or a region of it. The Runtime reads only what the package declares and writes each
reading back as an instance fact. It never writes a `produces` amount, a weight or any other
amount computed by the policy.

This document covers the declaration, its admission and the kernel reading (slice S5a, with
Workflow #384 D2-S2's marks on any page, hand-off and join), the host bridge that publishes the
readings as instance facts (slice S5b, "Host" below), the recognition maintenance package
(D2-S2, "Recognition maintenance packages" below) and the resource adjustments of an operation
(Workflow #384 D2-S3, "Adjustments" below).

## Declaration

`resource_readings` is an optional top-level family of an operation `task.json`. In a content
directory (`contracts/package-reference.md`) it is
`resources/operations/<task_id>/task.json`; the resource repository source layout uses the same
file. The following is the member only, not a complete task:

```json
"resource_readings": [
  {
    "id": "credits",
    "fact_key": "resource.credits",
    "page_id": "home",
    "target_id": "ocr/credits",
    "trim": "whitespace_v1",
    "value": { "type": "unsigned_integer", "min": 0, "max": 9007199254740991, "format": "comma_grouped" },
    "minimum_confidence_milli": 900,
    "valid_for_ms": 21600000
  }
]
```

| Field | Rule | Rejection @ pointer |
| --- | --- | --- |
| the array | `1..=16` entries; the task declares `scheduling_outcome` | empty or longer: `InvalidValue` @ `/resource_readings`; no outcome: `MissingField` @ `/scheduling_outcome` |
| `id` | `[A-Za-z0-9_\-/.:]`, `1..=128` bytes, unique in the task | `InvalidValue` @ `/resource_readings/<i>/id` |
| `fact_key` | starts with `resource.` or `inventory.`, satisfies the instance fact key rule, unique in the task | `InvalidValue` @ `…/fact_key` |
| `capacity_fact_key` | optional (Workflow #390 READ-1); only with `value.format` `current_capacity`; the key rule of `fact_key`; unique in the task among every key the task's marks write | `InvalidValue` @ `…/capacity_fact_key` (reason `resource_reading_capacity_fact_key_invalid`, or `resource_reading_fact_key_duplicate`) |
| `page_id` | an identifier like `id` (not `any`) naming a page the task declares: its entry, target, error or `scheduling_outcome` pages, or a page one of its operations leaves, reaches or passes through (Workflow #384 D2-S2; it was a terminal page only); the page gate may use any recognition backend | `InvalidValue` @ `…/page_id`; cross-reference: `package_invalid` naming `…/page_id`; before any input the Runtime refuses a page its detector does not hold (`page_undeclared`) |
| `target_id` | an identifier like `id`, exactly one entry of the task's own `ocr_targets`; a page gate may also name it (Workflow #396 REC-6), and the gate's observation of the frame handed to recognition is then the reading's | `InvalidValue` @ `…/target_id`; cross-reference: `package_invalid` naming `…/target_id`; a target the projection metadata marks personal is refused by the Runtime before any input |
| `trim` | `whitespace_v1` only | `InvalidValue` @ `…/trim` |
| `value` | `type` `unsigned_integer` only; `format` one of `ascii_decimal` (the default), `comma_grouped`, `current_capacity`, `times_prefixed`, `clock_hms`; `min <= max <= 2^53-1` | `InvalidValue` @ `…/value/type`, `…/value/format`, `…/value/min`, `…/value/max` (a copied `u64::MAX` is refused at `…/value/max`) |
| `minimum_confidence_milli` | `1..=1000` | `InvalidValue` @ `…/minimum_confidence_milli` |
| `valid_for_ms` | `1..=31_536_000_000` | `InvalidValue` @ `…/valid_for_ms` |
| any other member | none | `UnknownField` @ `…/<member>` |

Wrong JSON types are `InvalidType` and absent members `MissingField` at their own pointer. The
reasons are the existing closed `ResourceDeclarationReason` set. The declaration gate checks the
structure field by field; the value rules are the shared `ResourceReadingDeclaration::validate`
in the contract crate, whose reason names the field the gate reports. The cross-references are
checked by the package parser's validation of the source tree (a content directory is parsed
on admission).

`ResourceReadingDeclaration::parse` (trim, then the `OcrUnsignedIntegerFormat` parser with the
declared range) and `ocr_confidence_milli` (`floor(clamp(confidence, 0, 1) * 1000)`; a missing
or non-finite confidence has none) are the only implementations of reading parse and confidence
conversion. Workflow #308 slice 2 reuses both. The declaration type enters no event, artifact
or report.

### Second reading key

Workflow #390 READ-1 (forced compromise O-08). A `current_capacity` mark reads a fraction such as
`17/20`; with `capacity_fact_key` it writes both parts, the current value (`17`) as `fact_key`
and the capacity (`20`) as `capacity_fact_key`:

```json
{
  "id": "storage",
  "fact_key": "resource.storage",
  "capacity_fact_key": "resource.storage_capacity",
  "page_id": "home",
  "target_id": "ocr/storage",
  "trim": "whitespace_v1",
  "value": { "type": "unsigned_integer", "min": 0, "max": 999, "format": "current_capacity" },
  "minimum_confidence_milli": 900,
  "valid_for_ms": 21600000
}
```

The two readings come from one evaluation of the mark's target: one parse, one confidence, one
frame, the mark's `valid_for_ms`. `min..=max` bounds the current value only; the capacity is
written as read, and a capacity beyond `2^53-1` makes the mark give no reading (`out_of_range`).
A mark gives both readings or none: a mark that gives no reading is one miss, which names its
`fact_key`. Without `capacity_fact_key` a `current_capacity` mark writes the current value alone,
as before. The two readings are published as two observations, the capacity right after the
value, like the readings of two marks on one frame: the capacity's `source_snapshot_id` is the
value's followed by `/capacity` (the record table below). The fact store groups the records of
one snapshot id into one observation and refuses a second observation under a snapshot id that
already names an active record (`fact_observation_version_conflict`), so the value and the
capacity are refreshed, adjusted and invalidated each on its own.
`ResourceReadingDeclaration::parse_parts` returns both parts; `fact_keys` lists the keys a mark
writes (its `fact_key`, then its `capacity_fact_key`), which is what "the keys of a
mark" means everywhere below: a requested key set, an adjustment's mark of its key, the
recognition maintenance package's marked keys and their checks.

### Version gate

The family is accepted in task schema `0.8` and `0.9` only; in `0.3`–`0.7` it is
`UnconsumedField` @ `/resource_readings`. The task schema, `content-directory.v1`,
`control.json`, `resources.json` and the derived `pack.json` and `pages.json` versions are
unchanged; a task without the family parses, derives, simulates and runs exactly as before.

A Runtime older than this family refuses a package that declares it, on both load paths: a
sealed ZIP and a content directory both pass the same declaration gate, which reports
`UnknownField` @ `/resource_readings`. Nothing ignores the family silently. Likewise a Runtime
older than Workflow #390 READ-1 refuses a mark that declares `capacity_fact_key`, with
`UnknownField` @ `/resource_readings/<i>/capacity_fact_key`. Both are part of `package`
revision 3, which Runtime v0.12.0 declares once ([component-interfaces.md](component-interfaces.md)).

### Two uses, one declaration

- A recognition maintenance package: a package that only navigates and reads, the standard
  package's one reader of instance facts ("Recognition maintenance packages" below). A zero-input
  observation package (`operations: []`, schema `0.8`, the Workflow #308 zero-input observation
  package) is the simplest one.
- A mark in a business package: a task that changes the resource, or passes a page that shows
  it, marks that page; the reading is taken there.

The whole page is a mark whose OCR target covers the whole frame; no field is added for it. A
`linear_steps` package may carry marks (Workflow #384 D2-S2; it was refused before).

Not provided: an amount derived from a settlement (a constant or predicted amount) and a value
read from a pixel target. The value type is a closed set. Workflow #384 D2-S3: an amount that an
operation spends or gains comes only from live recognition after the operation is confirmed,
either a mark read after it or the change the screen shows (`+N`) read on its confirmation
frame ("Adjustments" below), never from the policy's `produces` or a constant in the package.

## Kernel reading

Workflow #384 D2-S2 (`rd-1009/d2/model-d2-fact-refresh.md` §3.4, §3.5). A run reads its marks on
the frames it reaches their pages on:

- **Hand-off.** When the run recognizes a page it reads marks on, on a frame that starts a pass
  of that page (the page differs from the page of the frame before: a stay on one page is one
  pass, and a page passed again is read again), the kernel records the in-memory trace
  `ContainedTaskTrace::ResourceReadingFrame { captured_at }` with that frame's device capture
  time and hands the frame to recognition. The run does not wait: the marks' OCR targets are
  evaluated on their own thread while the run goes on. A page-graph run hands off from its
  observations (its entry and every page it then recognizes); a `linear_steps` run from its
  entry frame and the frame each step arrives on. Workflow #396 REC-6: a mark's target that the
  page gate observed on a page-graph frame goes with the frame, and its recognition takes that
  observation instead of calling the provider again.
- **Join.** A successful run joins its readings in `finish_success`, after the operation `0.8`
  fields report (when the task has one) and always before `Finalizing`: frame by frame in
  hand-off order it waits for the frame's recognition, all frames together for at most the
  runtime's join timeout (`scheduling.fact_refresh.recognition_join_timeout_ms`, default
  10000; a runtime without the configuration waits until each recognition returns). Each mark's
  evaluation is recorded in the task diagnostic stream with the phase label `post_admission`, on
  its frame, then the text is trimmed and parsed and the confidence converted. A reading holds
  when the parse succeeds and the confidence is present and at least `minimum_confidence_milli`.
  For each frame the kernel then records the in-memory trace `ContainedTaskTrace::ResourceReadings
  { captured_at, readings, unresolved }`: per reading that holds the declaration, the key it
  writes, the integer value and the confidence milli (a mark with a second reading key gives two,
  its value then its capacity); per mark that gave no reading the declaration and its
  `reading_problem` (`empty`, `invalid_integer`, `overflow`, `out_of_range`,
  `region_unresolved`, `provider_failed`, `confidence_missing`, `low_confidence`, and
  `join_timeout` for a recognition that had not returned when the join timeout ended). The
  traces are never serialized.
- **A miss never fails the run.** Each mark that gave no reading is the warning
  `contained_task_resource_reading_unresolved` (`reading_id`, `fact_key`, `reading_problem`;
  [Host](#host) writes it), the run settles as it would without the mark, and an earlier fact of
  the key stays until its own expiry. A run that fails or is cancelled joins nothing and
  publishes nothing.
- **A requested key set** (`PreparedContainedTask::run_for_reading_keys`; the scheduler's
  reading dispatch, D2-S4) reads only the marks of its keys (a mark that writes any key of the
  set, READ-1, gives all its readings). A `linear_steps` package whose path
  starts on a page, ends on it and returns to it on the way, without optional operations, has
  one segment per return; with a key set, a segment that reaches no page marking one of its keys
  is not run, and the run stays on the entry page it starts and ends on. Any other package runs
  whole. A run without a key set (a manual `task-run`, MCP `ac_run_pack`, the Lab) reads every
  mark and runs every segment.
- Not in offline simulation: `actinglab package dry-run` reads nothing and reports
  `post_admission_ocr.pending_real_execution: true` for a task that declares OCR fields or
  readings.
- Not in a bound recovery entry, a prerequisite package or `recognize_only` (which cannot
  declare a scheduling outcome).

`judge_resource_reading` is the exported function the kernel judges one mark's evaluation with.

Before any input, the Runtime refuses a package whose reading target is not one OCR target,
locates text, or is marked personal by the projection metadata, with
`contained_task_resource_reading_invalid` and the detail `<id>:<reason>` (`target_not_ocr`,
`target_locates_text`, `target_personal`). A program that violates the declaration rules is
refused with the same code and the rule's reason as detail.

For these three target qualification refusals, the kernel attaches the verified bundle's
operation path and `/resource_readings/<index>/target_id` as a `ResourceDeclarationIssue`
with the operation schema and `InvalidValue`. For a manual request, the Host's existing
declaration rejection path records `runtime.failed` with the declared/verified package
identity and request links, then records the original code and `<id>:<reason>` in its linked
lifecycle failure. The ordinary receipt remains `PackageInvalid`, `Denied`, `fatal=false`
and references that declaration rejection. This occurs before task/run identifiers, lease
acquisition or input.
The lifecycle native detail retains its existing sensitive classification; no OCR text,
frame or observed amount is included. A failure to write either required ledger fact follows
the existing fatal ledger path instead of returning an unrecorded ordinary refusal.

## Host

Only the host writes readings, through `publish_facts`, the instance fact store's one
publication path; no write site is added.

**The traces.** The host accepts `ResourceReadingFrame` only before `Finalizing` and before the
run joins any frame, and only for the frame it captured last: the trace's `captured_at` must
equal the capture time the host kept with that frame's id, which the host keeps with the
hand-off. It accepts one `ResourceReadings` per handed-off frame, before `Finalizing` and in
hand-off order: its `captured_at` must be the next handed-off frame's. A hand-off after the join
began or after `Finalizing`, or readings after `Finalizing`
(`contained_task_resource_reading_duplicate`), a trace without a frame to name
(`contained_task_resource_reading_frame_missing`), one of another frame
(`contained_task_resource_reading_frame_mismatch`), and a successful `Finalizing` before every
handed-off frame is joined (`contained_task_finalizing_state_invalid`) poison the Runtime. The
readings that held travel in memory on the run's terminal draft, frame by frame. Each mark that
gave no reading is written at once as one task diagnostic row of kind `error`
(`TaskDiagnosticErrorData::Recognition`, phase `post_admission`, the mark's `target_id`) whose
`outcome` is the warning `contained_task_resource_reading_unresolved`, on its frame; a mark's
evaluation is recorded on its frame too.

**Run kinds.** A manual `task-run` and a policy dispatch both write. A startup package or a
stuck-recovery return-home package that declares readings is refused after admission, before
any lease and any input, with the request failure
`contained_task_resource_reading_run_kind_unsupported` (recorded as that package's
`runtime.failed`, `host_code=...`). A bound recovery entry and `recognize_only` take no
readings.

**The record.** Each reading is one `fact.published` with one record:

| Field | Value |
| --- | --- |
| `scope` | `{"kind": "instance", "instance_id": <the run's registered instance alias>}` |
| `key` | `fact_key`; for a mark's capacity, its `capacity_fact_key` (Workflow #390 READ-1; every other field but `source_snapshot_id` is as for the mark's value, `source_detector` included) |
| `content` | inline `integer`, the reading |
| `observed_at_unix_ms` | the device capture time of the frame the reading was taken on, in Unix ms. It is wall clock time, the clock domain of the Runtime clock that refuses an observation in the future |
| `expires_at_unix_ms` | `observed_at_unix_ms + valid_for_ms` |
| `ttl_policy` | `{"minimum_ms": valid_for_ms, "maximum_ms": valid_for_ms, "source": "detector_contract"}` |
| `confidence_milli` | the reading's `ocr_confidence_milli` |
| `source_detector` | `resource_reading:<entry task id>/<reading id>` |
| `source_snapshot_id` | `run:<run id>/frame:<frame id>/<reading id>`, the canonical `run_<hex>` and `frame_<hex>` ids; for a mark's capacity `run:<run id>/frame:<frame id>/<reading id>/capacity` (Workflow #390 READ-1, `resource_reading_capacity_snapshot_id`) |
| `schema_version` | `fact.v1` |
| `resource_bundle_hash` | the lowercase hex SHA-256 of `package:<package_id>`, the admitted package's `package_id` (Workflow #336 PKG-S2, option 丙): a package changed in place keeps the value, two packages never share one, and no package content is hashed |
| `invalidate_on` | `[]` |

The event has source `runtime`, actor `runtime`, origin module `fact-store` and system links.
It carries no run link: the run and the frame are named in `source_snapshot_id`.

**When.** Only a terminal whose outcome is `Success` and that carries readings or an adjustment
plan goes through the three stages below. Every other terminal, including every run without
readings or confirmed adjustments, runs the original terminal flow once, unchanged. (Workflow
#390 POOL-1 removed stage 0, the refusal of a reading of a live pool's key; see "Live pools".)

1. Under `fact_write_gate`, the original checks run: the run's chain, an already committed
   terminal, the capture summary and the settlement projection. Then the gate is released. An
   already committed terminal is rejected as before; a refused projection is rewritten as before
   (`contained_task_outcome_*`). Neither publishes. A successful projection of this terminal is
   the confirmed settlement.
2. No lock is held. Every record must fit a fact record first (else
   `contained_task_resource_reading_rejected`, `<id>:fact_observation_invalid`, before anything is
   published). Workflow #384 D2-S3: each planned drop is then one `fact.invalidated`, under
   `fact_write_gate`, of the record it names; a key whose record was replaced since the plan is
   `fact_observation_version_conflict`. Then each reading, frame by frame in the order of the
   frames' capture times and in mark order within a frame, and each adjustment write at its
   confirmation frame's time (after the readings of the same millisecond), is published through
   `publish_facts` as its own single-record observation; a key read on two frames is published
   twice, and the store keeps the newer observation. A reading uses the ordinary purpose; an
   adjustment write the `ResourceAdjustment` purpose, under which `publish_facts` checks, in the
   same hold of `fact_write_gate` as the write, that the key's record is still the one it
   adjusted (else `fact_observation_version_conflict`). A non-fatal refusal stops publishing: the
   records already written stay, and the terminal becomes the failure
   `contained_task_resource_reading_rejected`. A fatal store failure poisons the Runtime, as for
   every publication.
3. The gate is taken again and the original flow runs in full: the chain, an already committed
   terminal, the summary, the projection, the summary and terminal appends and the failed
   terminal note.

A refusal in stage 2 rewrites the terminal as a refused projection does: outcome `Failure`
with that code, no final page, no settlement, and the projection failure severity (`warning`
for a policy run). The request fails as `Failed` with that code and the failure terminal; the
detail is the failure's native detail.

`publish_facts` takes `fact_write_gate` itself, so nothing is published under the gate, and the
`policy` lock is never taken under it. Between stage 2 and stage 3 another path can commit the
run's terminal. Stage 3 then rejects the attempt as
`contained_task_terminal_already_committed`, and the facts stay written: they are real
observations of the frames they were read on, and the rejection is reported on its own.

**Ledger order.** `Finalizing` is the event `task.terminal_intent` and `TerminalCommitted` is
`task.completed` or `task.failed`.

- Success: the frames' recognitions, `Finalizing` (`success`), `fact.published` once per
  reading that held, `capture.summary_committed`, `TerminalCommitted` (`success`).
- Refused in stage 2: `Finalizing` (`success`), the readings published before the refusal,
  `capture.summary_committed`, `TerminalCommitted` (`failure`). This is the same sequence as for
  a refused settlement projection.

**Failure codes.** `TerminalCommitted.failure_code` holds the code alone. The detail is the
native detail of the run's host failure (`RuntimeHostError`). A policy run's driver receives
that error, and its failure record keeps the detail when the error was not already recorded.
The IPC receipt of a manual `task-run` carries the code (the top of its `outcome`, receipt v2)
but has no native detail field.

| Code | When | Native detail | Reading why from the ledger |
| --- | --- | --- | --- |
| `contained_task_resource_reading_invalid` | a declaration refused before any input | `<id>:<reason>` | the package admission refusal |
| `contained_task_resource_reading_run_kind_unsupported` | a startup or return-home package declares readings | none | the package's `runtime.failed` |
| `contained_task_resource_reading_rejected` | stage 2 | `<id>:<fact store code>` | see below |

For `contained_task_resource_reading_rejected`, the fact store code can be traced in the ledger:

- `fact_observation_not_newer`: a newer `fact.published` of the same scope and key.
- `policy_fact_authority_conflict`: the configuration's `policy.facts` declares the key.
- `fact_observation_incomplete_refresh`: the key's active record came from an observation with
  other keys.
- `fact_observation_in_future`: the Runtime clock is behind the device clock.
- `fact_store_capacity_exceeded`: the count of active fact identities.

The detail `<id>:fact_observation_invalid` also names a reading that a fact record cannot hold.

A refused run is a failed run for the policy (`failure_streak`). None of these codes triggers
stuck recovery. `contained_task_resource_reading_unresolved` is no failure code since Workflow
#384 D2-S2: a mark that gives no reading is a warning (task diagnostic row above) and the run
settles as it would without it; it adds nothing to `failure_streak`.

**Live pools.** Workflow #390 POOL-1 (forced compromise S-04): a `ledger_fact` pool takes its
data from readings. A reading, or an adjustment write, of the key the pool observes in the
run's instance scope is published like any other, with its empty `invalidate_on`, and is the
pool's base: its value at its observation time. The next reading or adjustment of the key
resets the base; no input invalidates it. The pool's value at an evaluation is that base
projected by the pool's regeneration rule (`contracts/scheduling/README.md`, "Pool
Projection": each complete period since the observation adds `amount`, up to `capacity`), as
long as the record is unexpired, at least the pool's minimum confidence and an integer within
`0..=capacity`; regeneration never revives an expired record. Such a record needs no input
invalidation: `pool_fact_invalidation_binding_missing` holds only for a record of the key that
is not a package reading or adjustment the Runtime published (its `source_detector` starts
with `resource_reading:` or `resource_adjustment:`), or that lost its native instance binding
(`docs/live-fact-pools.md`). A pool without a usable record asks for a reading: the task its
projection gates raises the fact refresh's demand for the key ([scheduling/README.md](scheduling/README.md),
"Fact Refresh"). A resource target on the pool reads the same projected inventory
([resource-targets.md](resource-targets.md)). An adjustment write keeps D2-S3's value (the
base's value plus or minus the amount, observed at the confirmation): regeneration between the
base's reading and the confirmation is known only from the next reading.

## Adjustments

Workflow #384 D2-S3 (`rd-1009/d2/model-d2-fact-refresh.md` §3.6). An operation that changes a
resource declares it, and the Runtime keeps the instance fact up to date from what the screen
shows after the operation; the amount is never written in the package.

### Declaration

`resource_adjustments` is an optional top-level family of an operation `task.json`, beside
`resource_readings`:

```json
"resource_adjustments": [
  {"id": "spent", "fact_key": "resource.credits", "operation_id": "buy"},
  {"id": "gained", "fact_key": "resource.points", "operation_id": "buy",
   "amount": {"target_id": "ocr/points_delta", "direction": "gain"}}
]
```

| Field | Rule | Rejection @ pointer |
| --- | --- | --- |
| the array | `1..=16` entries; the task declares `scheduling_outcome` | empty or longer: `InvalidValue` @ `/resource_adjustments`; no outcome: `MissingField` @ `/scheduling_outcome` |
| `id` | the identifier rule of a mark's `id`, unique in the task | `InvalidValue` @ `/resource_adjustments/<i>/id` |
| `fact_key` | the key rule of a mark (`resource.` or `inventory.`); one operation adjusts a key at most once; without `amount`, the task has a mark of the key (one that writes it, as `fact_key` or as `capacity_fact_key`) | `InvalidValue` @ `…/fact_key` |
| `operation_id` | an operation of the task | `InvalidValue` @ `…/operation_id` |
| `amount` | absent: the new value is read after the operation (a mark of the key); present: `{target_id, direction}` only | an amount source other than `target_id` (a constant, for example): `UnknownField` @ `…/amount/<member>` |
| `amount.target_id` | an identifier like `id`, exactly one entry of the task's own `ocr_targets`; the same target qualification as a mark's (`target_not_ocr`, `target_locates_text`, `target_personal`) | `InvalidValue` @ `…/amount/target_id`; cross-reference: `package_invalid` naming `…/amount/target_id` |
| `amount.direction` | `consume` (subtracted) or `gain` (added) | `InvalidValue` @ `…/amount/direction` |

The family is accepted in task schema `0.8` and `0.9` only (`UnconsumedField` @
`/resource_adjustments` otherwise); a Runtime older than the family refuses it with
`UnknownField`. The value rules are the shared `validate_resource_adjustments` in the contract
crate (`resource_adjustment_*` and `resource_adjustments_count_invalid` reasons); the kernel
refuses a program that breaks them with `contained_task_resource_reading_invalid`, the reason as
detail. A startup or return-home package and a prerequisite package that declare adjustments are
refused as for readings (`resource_adjustments`).

An amount is the change the screen shows, read on the confirmation frame by the OCR target and
the reading parser: the trimmed text, at most one sign of its direction (`+` for a gain, `-` for
a consumption), then ASCII decimal digits in `0..=2^53-1`; its confidence must be present and
at least the OCR target's `minimum_confidence`.

### Kernel

- **Confirmation.** When a step's operation succeeds (its `StepFinished` with the page it
  reached: a page-graph postcondition, a linear arrival), and the operation declares
  adjustments, the kernel records the in-memory trace
  `ContainedTaskTrace::ResourceAdjustmentConfirmed { captured_at, operation_label }` for the
  frame captured last, and hands that frame to recognition for each amount; the run does not
  wait. An operation whose effect is not confirmed (a failed, retried or unconfirmed attempt)
  confirms nothing.
- **Join.** A successful run joins, after its readings and before `Finalizing`, within the same
  join timeout, each confirmation in order: `ContainedTaskTrace::ResourceAdjustments
  { captured_at, adjustments }`, each adjustment with its amount (value and confidence) or the
  `reading_problem` it gave no amount for. An amount's evaluation is recorded on the
  confirmation frame. A run that fails or is cancelled joins and writes nothing.

### Host

- **The plan.** When the last confirmation is joined, before `Finalizing`, the host reads each
  adjusted key's current record (under `fact_write_gate`) and decides, confirmation by
  confirmation in order:
  - Without `amount`: a reading of the key that held on the confirmation frame or later is the
    new value and the adjustment writes nothing. Without one, the key's record is dropped and
    the adjustment is `fact_adjustment_skipped` (`after_read_missing`).
  - With `amount`, in this order: an amount that gave no reading is
    `contained_task_resource_reading_unresolved` (warning; `reading_id` is the adjustment's id)
    and `fact_adjustment_skipped` (`amount_unresolved`), and the key's record is dropped; a
    reading of the key on the confirmation frame or later, or a current record observed after
    the confirmation or published after its step, is `newer_observation`; no current integer
    record is `base_missing`; an expired one `base_expired`; a result below zero is
    `fact_adjustment_underflow` (warning) and the key's record is dropped. Otherwise the
    adjustment writes the key: the current record's value plus or minus the amount.
  - The current record is the latest of the key's record before the run, the run's readings of
    it before the confirmation and the run's earlier adjustment writes of it.
  - A key whose record is dropped keeps none of the run's records of it.
  Each adjustment's outcome is one task diagnostic row of kind `error`
  (`TaskDiagnosticErrorData::Recognition`, phase `post_admission`, the amount's `target_id`) on
  the confirmation frame, as a mark's miss is. `fact_adjustment_skipped` and
  `fact_adjustment_underflow` carry `instance_id` (the run's instance alias), `fact_key`,
  `adjustment_reason` (skipped only) and `task_id` (the package's entry task). The run succeeds
  in every case.
- **The write.** An adjustment write is one `fact.published` with one record, published in
  stage 2 below:

| Field | Value |
| --- | --- |
| `scope`, `key`, `schema_version`, `resource_bundle_hash`, `invalidate_on` | as a reading's |
| `content` | inline `integer`, the current value plus or minus the amount |
| `observed_at_unix_ms` | the confirmation frame's device capture time |
| `expires_at_unix_ms` | the adjusted record's own (a change never extends a fact's freshness) |
| `ttl_policy` | `{"minimum_ms": expires - observed, "maximum_ms": expires - observed, "source": "detector_contract"}`, none for a record without expiry |
| `confidence_milli` | the lower of the adjusted record's and the amount's |
| `source_detector` | `resource_adjustment:<entry task id>/<adjustment id>` |
| `source_snapshot_id` | `run:<run id>/frame:<frame id>/<adjustment id>/base:<event id>`: the run, the confirmation frame and the `fact.published` of the adjusted record |

- **The drop.** A dropped record is one `fact.invalidated` of the key's record from before the
  run, triggered by the confirmed step's `task.step_finished` (an adjustment invalidation, see
  [instance-fact-store.md](instance-fact-store.md)); a key without such a record, or whose
  record is newer than the confirmation, is left as it is.

## Scheduling connection
## Scheduling connection

The only connection to the scheduling catalog is `fact_key`: it equals the
`observation: {"kind": "fact", "fact_key": ...}` of a pool (`contracts/scheduling/`), and the
reading supplies only pools observed through such a fact. A pool observed through a task
outcome is unchanged and never supplied by a reading. A `ledger_fact` pool takes the reading as
its base and regenerates from it (Workflow #390 POOL-1, "Live pools" above); a
`static_snapshot` pool keeps the caller's snapshot, and only its targets read the reading. No pool id and no `resources.json` id is
named; the fact scope is always the instance of the run. The integer is the amount in the
pool's `valuation.unit` (Workflow #335 S2), without conversion.

An agent should not publish a reading's key by hand once a package reads it. A manual
observation that combines that key with other keys makes a later single-key reading an
incomplete refresh (`fact_observation_incomplete_refresh`), and a newer manual observation makes
it `fact_observation_not_newer`; publish at most that one key per observation.

## Recognition maintenance packages

Workflow #384 D2-S2 (`rd-1009/d2/model-d2-fact-refresh.md` §3.2, §3.10). The standard package's
recognition maintenance package is the one package the scheduler runs to read an instance's
facts (D2-S4). It is an ordinary task package that only navigates and reads.

- **Declaration.** The standard package root's `maintenance.json` names it with the use
  `recognition` ([package-reference.md](package-reference.md), "Maintenance declarations");
  each server has at most one (`duplicate_bundle_maintenance_role`), and a reader that does not
  know the use refuses the declaration.
- **Qualification** (`TaskPackageDescriptor::use_incompatibility(Recognition)`, the reader
  problem token): no `recognize_only` package (`recognize_only`), no stability termination
  (`stability_termination`), no designated operation (`designated_operation`), at least one
  mark (`no_marks`). Navigating into a page is allowed; it may not declare `cost_commit`
  (scheduling slice SR-10).
- **Binding.** The instance's `instances[].recognition_package` names it by location only,
  `{"package": "<path>"}` (never a digest, option 丙); a relative path resolves against the
  configuration file's directory ([runtime-config.md](runtime-config.md)). acsetup writes it
  from the bundle's declaration (D2-S6).
- **`check-config`** prepares each bound package and refuses a binding that is no reader with
  `fact_reader_binding_invalid` (stage `config_recognition_binding`; `reader_problem`: the
  qualification's token, `package_unreadable` with the loader's refusal as its cause, or
  `policy_fact_authority` for a mark whose key the configuration's `policy.facts` declares for
  the instance, since the fact store would refuse every such reading; a key a `ledger_fact`
  pool observes is no conflict since Workflow #390 POOL-1, and `live_pool_key` is no longer
  given). It reports `instances[].recognition_package`
  (`package`, `fact_keys`) and warns `fact_reader_missing:<instance>:<fact key>` for each
  realistic fact (`resource.*`, `inventory.*`) a catalog task's trigger or stop predicate reads
  in an instance's scope that the instance's reader does not mark, or that an instance without
  a reader cannot refresh.
- **A Runtime start** reads each bound package's marks without preparing it and runs without a
  binding it cannot read or whose marks the configuration claims; the `recognition_packages`
  row of `config.subsystems` notes each such binding as `fact_reader_binding_ignored` (caused by
  its `fact_reader_binding_invalid`) and the start goes on. The scheduler's reading dispatch
  checks the qualification again when it prepares the package (D2-S4).
- **Resource validation.** `package bundle --maintenance` adds one warning
  `recognition_pack_undeclared` (`package_id`) to its output's `warnings` for each pack that
  qualifies as a reader but is not declared `recognition`; the resource repositories make it an
  error once their readers are declared (D2-S6).

## Never written

`produces` amounts, effective weights, gaps and any other amount the policy computes are never
written back. Workflow #384 D2-S3: an adjustment's amount comes only from live recognition after
its operation is confirmed (a mark read after it, or the change the screen shows), never from
the policy or a constant in the package. No event type, no persisted field and no diagnostic
record kind is added.
