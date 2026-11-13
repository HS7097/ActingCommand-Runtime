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
(Workflow #384 D2-S3, "Adjustments" below). A mark writes one number; a list a page shows,
every position of it, is written as one `record_list` fact by a list reading (Workflow #428
S168-4, "List readings" below) or, for a list a run reads one position per frame, by a field list
(Workflow #450, "Lists from fields" below).

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
| `fact_key` | starts with `resource.` or `inventory.`, never with `inventory.list.` (from Workflow #428 S168-4: that prefix holds list readings only, "Capacity and reserved keys" below), satisfies the instance fact key rule, unique in the task | `InvalidValue` @ `…/fact_key` (reason `resource_reading_fact_key_invalid`) |
| `capacity_fact_key` | optional (Workflow #390 READ-1); only with `value.format` `current_capacity`; the key rule of `fact_key`, so never under `inventory.list.` either; unique in the task among every key the task's marks write | `InvalidValue` @ `…/capacity_fact_key` (reason `resource_reading_capacity_fact_key_invalid`, or `resource_reading_fact_key_duplicate`) |
| `page_id` | an identifier like `id` (not `any`) naming a page the task declares: its entry, target, error or `scheduling_outcome` pages, or a page one of its operations leaves, reaches or passes through (Workflow #384 D2-S2; it was a terminal page only); the page gate may use any recognition backend | `InvalidValue` @ `…/page_id`; cross-reference: `package_invalid` naming `…/page_id`; before any input the Runtime refuses a page its detector does not hold (`page_undeclared`) |
| `target_id` | an identifier like `id`, exactly one entry of the task's own `ocr_targets`; a page gate may also name it (Workflow #396 REC-6), and the gate's observation of the frame handed to recognition is then the reading's | `InvalidValue` @ `…/target_id`; cross-reference: `package_invalid` naming `…/target_id`; a target the projection metadata marks personal is refused by the Runtime before any input |
| `trim` | `whitespace_v1` only | `InvalidValue` @ `…/trim` |
| `value` | `type` `unsigned_integer` or (SCH-M CF-2) `timestamp_ms`. `unsigned_integer`: `format` one of `ascii_decimal` (the default), `comma_grouped`, `current_capacity`, `times_prefixed`, `clock_hms`; `min <= max <= 2^53-1`; no `base`. `timestamp_ms` ("Completion times" below): exactly `type`, `format` `clock_hms` and `base` `observation`, both required | `InvalidValue` @ `…/value/type`, `…/value/format`, `…/value/min`, `…/value/max` (a copied `u64::MAX` is refused at `…/value/max`), `…/value/base`; a member of the other type: `UnknownField` @ `…/value/<member>` |
| `value.dictionary` | optional (Workflow #411 B1): the number's dictionary, applied after the trim and before the parser ([ocr-fields.md](ocr-fields.md), the paragraph of Workflow #411 B1) | `InvalidValue` @ `…/value/dictionary/<member>` as listed there |
| `minimum_confidence_milli` | `1..=1000` | `InvalidValue` @ `…/minimum_confidence_milli` |
| `valid_for_ms` | `1..=31_536_000_000` | `InvalidValue` @ `…/valid_for_ms` |
| `agreement` | optional (Workflow #411 B1): `{"frames", "interval_ms", "k"?}`, `frames` `2..=5`, `interval_ms` `1..=1000`, and `k`, when present, equal to `frames`: a mark writes a fact, so every frame must read its value ("Kernel reading" below) | `InvalidValue` @ `…/agreement/frames`, `…/agreement/k`, `…/agreement/interval_ms`; `MissingField`, `InvalidType` and `UnknownField` at their member |
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

### Completion times

SCH-M CF-2 (frozen model `rd-1024/sched-design/model.md` D-9). A mark may read a completion time:
a countdown the screen shows, the time left until some work is done. Its `value` is

```json
{ "type": "timestamp_ms", "format": "clock_hms", "base": "observation" }
```

- `format` is required and is `clock_hms` only ([ocr-fields.md](ocr-fields.md)): `H:MM:SS`,
  the time left in seconds. Any other format is `InvalidValue` @ `…/value/format`.
- `base` is required and is `observation` only: the countdown is added to the device capture
  time of the frame the reading was taken on. Any other value is `InvalidValue` @
  `…/value/base`.
- `min`, `max` and `dictionary` are members of `unsigned_integer` only (`UnknownField` at the
  member), and `capacity_fact_key` is refused as for every mark that is not a fraction.
- **The published value** is `timestamp_ms` = the frame's `observed_at_unix_ms` + the seconds ×
  1000 ("Host" below). The fact key keeps the mark key rule (`resource.` or `inventory.`) and
  `valid_for_ms` stays as declared. The catalog predicate `fact_time_reached` reads it
  ([scheduling/README.md](scheduling/README.md), "Fact-time wake").
- **An agreement** compares the instants its frames convert to, not the countdowns ("Kernel
  reading" below), so a running countdown agrees with itself.
- **No adjustment.** A completion time is no amount: a `resource_adjustments` entry that names
  the key of a completion-time mark, with or without `amount`, is refused at declaration,
  `InvalidValue` @ `/resource_adjustments/<i>` (coordinator ruling of 2026-10-24; the shared
  rule's reason is `resource_declaration_invalid`, and Runtime admission refuses it as
  `contained_task_resource_reading_invalid` with that reason).
- **An absent countdown.** A slot whose region shows no countdown gives no reading (the mark's
  miss and its warning, as today); nothing is published for it. A catalog covers an emptied slot
  with `once_per_reading` and a free-slot count fact (SCH-M CF-1).
- A resource side that keys completion times uses a sub-prefix that no reset invalidates
  (`resource.<…>.ready.<name>`) and lists the counter prefixes its daily resets clear (model
  D-10, data side).

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
Workflow #411 B1's `agreement` and `value.dictionary` join `package` revision 6, which the B
train declares at its end (register §1.163); until then they are admitted by code, and a
Runtime before them refuses them with `UnknownField` at the member.
SCH-M CF-2's `value` `timestamp_ms` joins `package` revision 6 as well (BT-PK6 declares it;
until then it is admitted by code). A Runtime before CF-2 refuses such a mark at its value,
never silently: its declaration gate knows neither the member `base` nor the type
`timestamp_ms`, and refuses the first with `UnknownField` @ `…/value/base`. So a pack that
declares one is placed only on a Runtime that names the subsystem `resource_reading_time` in
its `config.subsystems` ([actingd-check-config.md](actingd-check-config.md)).

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
  writes, the integer value (SCH-M CF-2: a completion-time mark's countdown in seconds) and
  the confidence milli (a mark with a second reading key gives two,
  its value then its capacity); per mark that gave no reading the declaration and its
  `reading_problem` (`empty`, `invalid_integer`, `overflow`, `out_of_range`,
  `region_unresolved`, `provider_failed`, `confidence_missing`, `low_confidence`, and
  `join_timeout` for a recognition that had not returned when the join timeout ended, and
  Workflow #411 B1's `no_agreement` below). The traces are never serialized.
- **Agreement** (Workflow #411 B1, register §1.163). On a pass of a page whose wanted marks
  declare an `agreement`, the kernel captures after the frame it hands off as many more frames
  as the largest agreement needs (`frames - 1`), each the largest declared `interval_ms` after
  the one before (paced as every wait), records each as a capture and as a
  `ResourceReadingFrame`, and hands each to recognition for the marks whose agreement reads it.
  A mark with an agreement reads the handed-off frame and the next `frames - 1`. It holds only
  when each of them gives the same readings (its value and, with a second reading key, its
  capacity); its readings are then the handed-off frame's, at the lowest confidence of its
  frames. A frame whose recognition failed or had not returned gives the mark `provider_failed`
  or `join_timeout`; any other difference is the miss `no_agreement`, and the mark writes no
  fact: no default, no last value. Each later frame's evaluations are recorded on that frame,
  and the frame is joined after the handed-off one, with no readings of its own. A runtime that
  does not sample refuses the pass with `recognition_sampling_runtime_unsupported`, and a
  capture failure or a cancellation between these captures ends the run, as during target
  consensus sampling. Without an agreement a pass hands off its one frame, as before.
  SCH-M CF-2: a completion-time mark's frames agree when each gives its reading and the
  instants they convert to (each frame's capture time plus the countdown it shows) lie within
  (`frames` - 1) × `interval_ms` + 1000 ms of one another, the latest minus the earliest;
  otherwise the miss is `no_agreement`. Its reading is the handed-off frame's countdown.
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
refused with the same code and the rule's reason as detail; from Workflow #428 S168-4 (0.12.50) a
mark's own rule (`ResourceReadingDeclaration::validate`) is `<id>:<reason>`, the mark's id before
its reason, and the rules of the marks together (their count, a repeated id or key) keep the
reason alone.

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
| `content` | inline `integer`, the reading; SCH-M CF-2: for a completion-time mark inline `timestamp_ms`, `observed_at_unix_ms` + the countdown's seconds × 1000 (a sum beyond `u64` is a record the fact record cannot hold: `contained_task_resource_reading_rejected`, `<id>:fact_observation_invalid`) |
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
| `fact_key` | the key rule of a mark (`resource.` or `inventory.`); one operation adjusts a key at most once; without `amount`, the task has a mark of the key (one that writes it, as `fact_key` or as `capacity_fact_key`); SCH-M CF-2: not the key of a completion-time (`timestamp_ms`) mark, with or without `amount` | `InvalidValue` @ `…/fact_key`; a completion-time key: `InvalidValue` @ `/resource_adjustments/<i>` (the shared rule's reason `resource_declaration_invalid`) |
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
  qualifies as a reader of a server (variant) that declares no `recognition` package. Only the
  one a server declares counts (Workflow #411 A32): the other qualifying packs of a server that
  declares one raise nothing. The resource repositories make it an error once their readers are
  declared (D2-S6).

## List readings

Workflow #428 S168-4 (Runtime 0.12.50), written ahead of its code by S168-0
([v012-register.md](v012-register.md) §1.291, the S168-4 subsection there). A list reading reads
a list a page shows, across as many viewports as it takes, and writes it as one instance fact of
value `record_list`: one row per position, keyed by the position's business identity, with the
values the package declares for it. It is one of two ways a package's reading becomes a
record-list fact; the other is a field list ("Lists from fields" below), which groups OCR fields
read one position per frame into the same fact. An OCR field never becomes a fact on its own
([ocr-fields.md](ocr-fields.md)). **Status:** built by
S168-4, `package` revision 8 ([component-interfaces.md](component-interfaces.md)); a Runtime
before 0.12.50 refuses a package that declares one (`UnknownField` at
`/operations/<i>/read_list`).

### Declaration

A list is read by a step: an operation of a task schema `0.9` page-graph task whose effect is
`read_list`, beside `click`, `application`, `select` and `wait`. The following is the operation
only; the numbers and names are neutral examples, not values:

```json
{"id": "read_kind_a", "from": "list_page", "to": "list_page",
 "read_list": {"layout_id": "layout/members", "key_feature": "member", "key_column": "id",
               "columns": [{"column": "rank", "feature": "rank"},
                           {"column": "open", "feature": "open"}],
               "fact_key": "inventory.list.members_kind_a", "valid_for_ms": 86400000,
               "scan": {"identity_feature": "member", "axis": "vertical", "advance": {…},
                        "reposition": {…}, "settle": {…}, "track": {…}, "start": {…},
                        "end": {…}, "bounds": {…}}},
 "guard": {…}}
```

| Member | Rule | Refusal @ pointer |
| --- | --- | --- |
| the effect | exactly one of `click`, `application`, `select`, `wait`, `read_list` (and, from Workflow #428 S168-5b, `judge`); task schema `0.9` only; a page-graph task (`navigable_route`, phased or not); the task declares `scheduling_outcome`; at most 16 `read_list` operations in a task | an older schema: `UnconsumedField` @ `/operations/<i>/read_list`; `linear_steps`: `contained_task_linear_invalid`, `reason=read_list`; no outcome: `MissingField` @ `/scheduling_outcome`; a 17th: `InvalidValue` @ `/operations` |
| `to` | the `from` page: the step stays on its page | `InvalidValue` @ `/operations/<i>/to` |
| `layout_id` | a `repeated_anchor` layout the task declares on the `from` page, whose `unknown_identity` is `reject` | `InvalidValue` @ `…/read_list/layout_id` |
| `key_feature` | an `identity` feature of the layout; the scan's `identity_feature` | `…/read_list/key_feature` |
| `key_column` | a token `^[a-z][a-z0-9_]{0,31}$`: the row member that holds the business ID | `…/read_list/key_column` |
| `columns` | 0 to 63 entries `{column, feature}`: `column` a token as `key_column`, distinct, never the `key_column`; `feature` a feature of the layout other than `key_feature`, each at most once, of value `passed` (a `boolean` column), `ocr_integer` (an `integer` column) or `identity` (a string column, the business ID it reads) | `…/read_list/columns/<j>/column`, `…/columns/<j>/feature` |
| `fact_key` | starts with `inventory.list.`, satisfies the instance fact key rule, and is unique among every key the task writes (each mark's `fact_key` and `capacity_fact_key`, each other list, a field list's included) | `…/read_list/fact_key` |
| `valid_for_ms` | `1..=31_536_000_000` | `…/read_list/valid_for_ms` |
| `scan` | required: a select step's `scan` ([selection-graph.md](selection-graph.md), "Select step"), with every static check admission runs on it, but no `early_stop`: every position is read | `MissingField` @ `…/read_list/scan`; `early_stop`: `UnknownField` @ `…/scan/early_stop`; the static checks as a select step's |
| `guard` | required, any guard a select step takes; judged on every viewport | `MissingField` @ `/operations/<i>/guard` |
| the operation's other members | as the table "Members of a `read_list` step" below says | as that table says |
| any other member of `read_list` | none | `UnknownField` @ `…/read_list/<member>` |

**Members of a `read_list` step.** Every member the task schema lets an operation carry (the
operation members of the declaration gate, `DeclarationValidator::operation` in
`crates/pack-containment/src/source/declarations.rs`, with the effects of S168-4 and S168-5b)
either applies to a `read_list` step with its existing meaning or is refused, as follows. The
declaration gate refuses at the pointer shown, and admission, which runs the same validation
before the common operation checks (as it checks a wait step's members first), refuses with
`contained_task_resource_reading_invalid` as for every row above:

| Member | On a `read_list` step | Refusal @ pointer |
| --- | --- | --- |
| `id` | applies: the operation's id, the `reading_id` of its warning and part of its `source_detector` | as for every operation |
| `from` | applies: the step's page, which is the page of its layout, so never `any` | the `layout_id` row above |
| `to` | the `from` page | the `to` row above |
| `read_list` | the effect | the rows above |
| `click`, `application`, `select`, `wait`, `judge` | refused: exactly one effect | the first row above |
| `guard` | required | the `guard` row above |
| `when` | applies with its meaning ([resource-declarations.md](resource-declarations.md), "Task schema `0.9` candidate operations"): a step whose `when` does not hold is withdrawn and the page's next candidate is taken; it has not run, so the once-per-run rule below does not pass it over yet | as for every operation |
| `retryable` | `false` or absent: the scan's moves are inputs, and a retry would repeat them | `true`: `InvalidValue` @ `/operations/<i>/retryable` |
| `max_attempts`, `retry_interval_ms`, `on_error` | refused: the step is never retried, and after its moves the run does not recover within itself (section Reading) | `UnknownField` @ `/operations/<i>/<member>` |
| `unguarded_trusted_coordinate`, `verify_template` | refused: the step declares its guard; `verify_template` is a click's source form of one | `UnknownField` @ `/operations/<i>/<member>` |
| `expect_after`, `post_delay_ms`, `input_recheck`, `observation_window`, `on_guard_refused` | refused: the step confirms nothing after its moves (its end check is its evidence), has no tap to wait after or recheck, and a guard refusal on a viewport fails the scan | `UnknownField` @ `/operations/<i>/<member>` |
| `max_runs`, `on_max_runs` | refused: the step runs once per run by its own rule (below) | `UnknownField` @ `/operations/<i>/<member>` |
| `transition`, `optional` | refused: `linear_steps` members, and a list reading runs in a page graph | `UnknownField` @ `/operations/<i>/<member>` |
| `threshold`, `method`, `mask`, `maskRange`, `rect_move`, `maa_task`, `maa_task_id` | refused by the gate's existing rule: only beside `verify_template`, which the step refuses | `UnconsumedField` @ the member |
| `destructive` | refused, as on every operation | `UnconsumedField` @ the member |
| `purpose`, `provenance`, `verified_live`, `effect` (the string member, not an effect), `timeout_ms`, `pre_delay_ms`, `pre_wait_freezes_ms`, `post_wait_freezes_ms`, `consumes`, `produces` | apply with their existing meaning: the gate checks their types and the kernel reads none of them (the scan paces its moves by its own `settle`); `purpose`, `consumes` and `produces` go to the step's primitive as for every operation ("Derived documents" below) | as for every operation |

A list that fits one viewport declares `scan` too: its start and end checks then both pass on its
first viewport and the step makes no input. A layout of another kind is refused: one frame of a
list cannot show where the list ends, and a list without positive evidence of both ends is never
written.

The declaration gate (`resource validate` and every parse) checks the structure, and the parser
the references, each refusal naming the task's `task.json` and the pointer of the offending field
as for a select step. Runtime admission runs the same validation on the sealed task and refuses
a program that breaks it, before any input, with `contained_task_resource_reading_invalid` and
the detail `<operation id>:<reason>`. S168-4 built it so: the declaration gate checks the step's
structure in the operation's own check (`Declaration::read_list_step`) and the rules of the whole
task with its references in the task's own `candidate_layouts` (`Declaration::list_readings`);
admission's own checks run before the common operation checks (`validate_list_reading_members`)
and with the recognition pack (`prepare_list_readings`), and `<reason>` is the member the gate
refuses at, below the operation (`read_list`, also for another effect beside it, `to`, `guard`,
`retryable`, a refused member, `read_list/layout_id`, `read_list/columns/<j>/feature`, …),
`scheduling_outcome` or `operations` for a rule of the whole task, and
`read_list/scan <key>=<value>` for a select step's static check of the scan. One case differs: a
`read_list` whose members do not make up a declaration (one missing, `scan` for example, or one of
the wrong type) is refused at `read_list`, where the gate names the member (`read_list/scan`).
Admission runs the declaration gate on the sealed task before its own checks, so a
sealed task that breaks a rule the gate checks is refused there first, with
`resource_declaration_invalid` at the pointer; in a run, admission's own refusal is the one of a
rule only it checks (the scan's values against a select step's static checks, the references
against the recognition pack), and its other checks stand behind the gate's. A startup,
stuck-recovery return-home or prerequisite
package that declares one is refused as for readings
(`contained_task_resource_reading_run_kind_unsupported`); a bound recovery entry and
`recognize_only` take none.

Workflow #448 SBUD-1 (since 0.12.57; [v012-register.md](v012-register.md) §1.333): admission also
compares the step's sampling with its scan's `max_ms`, capped by the task's `timeout_ms`, as for a
select over a scan: in a pack that declares target consensus, one page recognition, which every
capture of the scan samples; then the candidate projection's provider need of its layout
([selection-graph.md](selection-graph.md), "Select step"), which a list reading's identity key
feature always makes compared. A larger need is refused with `recognition_sample_budget_invalid`
(`required_ms`, `limit_ms`, `pack_operation_id`); a need equal to the limit is admitted.

**Once per run.** A `read_list` step runs at most once in a run: once it ran, the page's
operations are chosen without it, as for an operation whose `max_runs` 1 was reached with
`on_max_runs` `next_candidate` ([README.md](README.md), "Per-run operation caps"). The static walk
takes it as an edge from its page to the same page and then follows the page's next operation. A
requested key set (`run_for_reading_keys`) never leaves one out: a page-graph package runs whole.

### Reading

One `read_list` step runs:

1. **Scan.** The scan session of a select over a scan, its steps 1 and 2
   ([selection-graph.md](selection-graph.md), "Select over a scan"): the dispatched frame settled
   as the first viewport; the start check, with a rewind when declared; then the coverage
   viewports, each with its move, settle, page, guard, alignment, gap and spacing checks and the
   reading of every position, until the end check passes. No policy is evaluated, so nothing
   stops it early.
2. **Rows.** Each position's fixed Known reading is one row: `{<key_column>: <business ID>,
   <column>: <value>, …}`, each declared column the fixed reading of its feature. The rows are
   ordered by key, in ascending byte order.
3. **End.** No `Return`, no `Confirm`, no tap. The kernel records the step's `StepFinished` with
   `page_label` its page as recognized on its last viewport (for a phased task, `phase` with
   `completed: false`), and the run goes on.

The list holds only when every position was read. Otherwise it gives no reading:

| `reading_problem` | When |
| --- | --- |
| `list_position_unread` | a position that was readable at least once holds no fixed Known reading of its identity or of a column's feature: never Known, contradicted later, or out of the dictionary, which counts as Unknown |
| `list_duplicate_key` | two positions read the same business ID |
| `list_rows_exceeded` | a 1025th position was read (since S168-LB1a, 0.12.55; before it, a 257th): the list holds more rows than one fact can (1024, before S168-LB1a 256; section "Rows" below). The scan stops there; nothing is cut to fit |
| `list_bytes_exceeded` (since S168-LB1a, 0.12.55) | the list was read whole, within the row bound, and its record is larger than one observation can hold (512 KiB; section "Rows" below). Nothing is cut to fit. The host finds it when it takes the step's list, measured on the record the run's end would publish. Before S168-LB1a such a list failed its run at publication instead |

Each is the warning `contained_task_resource_reading_unresolved` (`reading_id` the operation's
id, `fact_key`, `reading_problem`, and `count`: the unread positions, the repeated keys, 1025
(since S168-LB1a, 0.12.55; before it 257), or for `list_bytes_exceeded` the rows read;
`count` is an optional value S168-4 adds to that code, [v012-register.md](v012-register.md)
§1.291 S168C-3),
written at once as one task diagnostic row of kind `error` (`TaskDiagnosticErrorData::Recognition`,
phase `post_admission`, without `target_id`) on the step's last viewport. The step completes, the
run goes on and settles as it would without the list, and the key's earlier record stays until
its own expiry. A list that gave no reading is never written in part: no row, no empty list and
no earlier value is published for it.

A failure of the scan itself (the start, a settle, an alignment, a gap, the spacing,
`selection_scan_identity_mismatch`, the end, `selection_scan_bounds_exceeded`, the page, the
guard, the geometry, the budget, a projection) ends the run with its code, with the stage
`read_list`, as for a select over a scan: the moves were inputs, so the step is not retried and
the run does not recover within itself. A run that fails or is cancelled publishes no list.

Zero positions, with both checks passed, is an empty list: a reading that holds, with no row.
At this step a layout whose anchor template matches nothing on the page cannot be told from a
list that is really empty: both read zero positions and, when both checks pass, publish an empty
list. A package must therefore make its start and end checks specific to the list page, so that
they do not both pass where the list does not show.

### Rows (S168-LB1a, since 0.12.55)

Workflow #428 S168-LB1 ([v012-register.md](v012-register.md) §1.309; the coordinator's ruling 1
of 11-11, not Alice's words). Before S168-LB1a a list held at most 256 rows, and a roster or a list
of holdings longer than that is never written: the run succeeds, warns `list_rows_exceeded` and
publishes nothing, so a selection that reads the list has nothing. Since S168-LB1a:

- **1024 rows.** A list holds at most 1024 rows, the bound of a business identity domain
  (`MAX_IDENTITY_ENTRIES`; [selection-policy.md](selection-policy.md), "Limits": enumerated
  members 1024). A row is one business ID and a list read whole never repeats one, so a list
  never holds more rows than its dictionary has IDs. The kernel counts the positions it has
  read, whatever their identity, and stops the scan on the viewport that reads the 1025th
  (`list_rows_exceeded`, `count` 1025), before it resolves that position's key.
- **One constant, the format's.** `MAX_LIST_READING_ROWS` is 1024 and equal to the row bound of
  a `record_list` fact, a constant of its own apart from the store's record counts
  ([instance-fact-store.md](instance-fact-store.md), "List reading facts", "Row bound"). It is no
  configuration key: every reader of the fact checks it, so a configured value would let one
  build write a record that another refuses.
- **The scan's bounds stay.** At most 128 viewports (`max_viewports`,
  [selection-graph.md](selection-graph.md), "Select step"), within `bounds.max_ms` and the task's
  `timeout_ms`: a list whose first viewport shows *f* positions and whose every advance brings *n*
  new ones reads at most *f* + 127 × *n* positions, so 1024 rows need about 8 new positions per
  advance. A scan that runs out of its bounds before its end check passes fails as today
  (`selection_scan_bounds_exceeded`).
- **The observation's bytes stay.** One list is one observation of at most 512 KiB (section
  Publication), each row at most 64 fields, each string at most 1024 bytes. Measured on the
  serialized record by the read-only survey of 11-11 (register §1.309): a row of the key and 3
  short columns takes about 170 bytes, of the key and 8 about 356 bytes, of the key and 63 about
  2.6 KB, so 512 KiB hold about 3,079, 1,470 and 203 such rows. 1024 rows fit up to about the key
  and 8 short columns; a wider list meets the byte bound first. Before S168-LB1a that failed its run at
  publication (`contained_task_resource_reading_rejected`), as a list of the key and 63 columns
  did near 200 rows. Since S168-LB1a it is a miss of the list, as a list over the row bound is
  (`list_bytes_exceeded` above; the coordinator's answer to question 4 of §1.309): the run settles
  as without the list, the previous record stays, and the warning says so at every run. A list
  that grows with its account must not start failing its run one day.
- **One instance's whole read stays within one frame.** All of an instance's records together
  cross the 1 MiB frame of the instance fact read, which is refused past it
  ([instance-fact-store.md](instance-fact-store.md), "Reading the seeds"): about 6,000 rows of the
  key and 3 short columns, or about 2,900 of the key and 8, over every list of the instance, less
  its other records.

### Publication

The host writes the lists, through `publish_facts`, at a successful terminal, in stage 2 of
"Host" above: the run's lists travel in memory on its terminal draft, as its readings do (a
successful terminal that carries lists goes through the three stages as one that carries
readings), and each list is its own single-record observation, published in the order of the capture times of
the viewports their end checks passed on, after the readings of the same frame. No write site is
added.

| Field | Value |
| --- | --- |
| `scope` | `{"kind": "instance", "instance_id": <the run's registered instance alias>}` |
| `key` | `fact_key` |
| `content` | inline `record_list`, the rows in ascending byte order of their key |
| `observed_at_unix_ms` | the device capture time of the viewport the end check passed on |
| `expires_at_unix_ms` | `observed_at_unix_ms + valid_for_ms` |
| `ttl_policy` | `{"minimum_ms": valid_for_ms, "maximum_ms": valid_for_ms, "source": "detector_contract"}` |
| `confidence_milli` | the lowest confidence of the features the rows read that carry one (an identity always does; a `passed` feature of a color or composite target carries none); 1000 for an empty list, whose evidence is its passed start and end checks |
| `source_detector` | `list_reading:<entry task id>/<operation id>` |
| `source_snapshot_id` | `run:<run id>/frame:<frame id>/<operation id>`, the frame of the observation time |
| `schema_version` | `fact.v1` |
| `resource_bundle_hash` | as a reading's |
| `invalidate_on` | `[]` |

A row holds at most 64 fields (63 columns and the key). Since S168-LB1a (0.12.55) a list whose record is over 512 KiB is the miss `list_bytes_exceeded`,
written when its step ends ("Reading" above); before it such a record failed the run here. A
record the fact record cannot otherwise hold is `contained_task_resource_reading_rejected` with
`<operation id>:fact_observation_invalid`, before anything is published, as for a reading; the
store's refusals are those of "Host" above, `fact_store_capacity_exceeded` among them, and fail
the run the same way.

### Staleness

- **Every complete read is published**, also when its rows did not change: a new observation (its
  run and frame) with a strictly newer observation time. A list's age is therefore the age of its
  last complete read, and `valid_for_ms` bounds it. A document that reads it states its own
  `max_age_ms` ([selection-policy.md](selection-policy.md), "Record lists").
- **No invalidation.** A run that changes what a list holds does not drop the list: the package
  reads it again, or its rows stand until their expiry.
- **At the run's end.** A run's lists are published at its successful end, so a later step of
  the same run (a judge step) reads them as they stood before the run.
- **Nothing refreshes a list on demand.** The fact refresh raises demand only for keys a catalog
  predicate reads, and no predicate reads a record list ([selection-policy.md](selection-policy.md),
  "Scheduling consumer"); the standard package schedules its reading task itself. Since S168-LB1a
  (0.12.55) the scheduler's fact projection leaves every key under `inventory.list.` out,
  and the catalog compile refuses a catalog that names one to read
  ([instance-fact-store.md](instance-fact-store.md), "Projection into the evaluation"): a list is
  the selection's input, never the scheduler's.

### Capacity and reserved keys

- Each list is one fact identity, `(instance scope, fact_key)`, whatever its rows and however
  often they change: the store replaces the record in place. Identities count against the store's
  256 active facts, shared by the whole host and never reclaimed
  ([instance-fact-store.md](instance-fact-store.md), "Capacity ledger"). A package may declare
  several lists, for example one per kind of member, each under its own key: *n* lists cost *n*
  identities on every instance that runs it. On a full store the publication is refused
  (`fact_store_capacity_exceeded`, the run fails `contained_task_resource_reading_rejected`);
  nothing is evicted.
- A client never writes a list: a `PublishFact` or `PublishFacts` of a key under
  `inventory.list.` is refused before the fact write gate, as the non-fatal request error
  `list_reading_key_reserved` (`fact_key`), with nothing appended, from every client envelope
  and on the Runtime's own ordinary publication path. Since Workflow #434 LAB-6S1b every client
  envelope may publish facts (the agent dispatcher's (Agent, Adapter), the CLI's (Cli, Cli), the
  console's (User, Ui), the Lab's (Lab, Lab)); before it only the first reached the Runtime with
  such a key, the others being refused when the request was formed
  (`invalid_agent_dispatcher_origin`, retired), as for any publication of a key outside the
  families with the origin exception.
- A mark never writes under `inventory.list.` either: from S168-4, a mark's `fact_key` or
  `capacity_fact_key` under that prefix is refused with the refusals of a mark's other key rules
  (`ResourceReadingDeclaration::validate`, "Declaration" above). The declaration gate refuses it
  as `InvalidValue` @ `…/fact_key` with reason `resource_reading_fact_key_invalid`, or @
  `…/capacity_fact_key` with `resource_reading_capacity_fact_key_invalid`; Runtime admission
  refuses the program before any input with `contained_task_resource_reading_invalid` and the
  detail `<id>:<reason>`. So every record under the prefix is the `record_list` of a list reading
or of a field list ("Lists from fields" below). The
  key rule is the marks' and the adjustments' one rule, so an adjustment's `fact_key` under the
  prefix is refused too. As for the step (section Declaration), admission runs the gate on the
  sealed task first, which refuses such a mark at its pointer before admission's own check.

### Evidence

The list's evidence is the run's own records: the fact record names the run and the viewport;
every viewport is a capture with its recognition records and every move a `task.effect_intent`
with its input, as a scan's; a miss is its warning row. No record type and no member is added.
The rows are account data: they stay in the instance fact store and the ledger, and no Runtime
path writes them anywhere else. Offline, `package dry-run` reads no list and reports the task's
post-admission reading as `pending_real_execution`, as for a task with marks.

### Derived documents

As [selection-graph.md](selection-graph.md), "Select step", "Derived documents" says for a select
step, with the list reading's own effect:

- **Navigation edge and page operation:** none. The step's moves are inputs a route never
  passes and its `to` is its `from`, so the source build skips it as it skips a select or wait
  step.
- **Primitive:** the shape of a click primitive with `click: null` and the declared `guard`, with
  `purpose`, `consumes` and `produces` as for every operation, and the `read_list` object added as
  declared, as a select step's primitive adds `select`. The primitives stay at schema `0.6`.
- **Sealed task** (`canonical_task`): keeps `read_list` and the declared guard (its page ID
  canonicalized as for every guard); no click is inferred.
- **Operation index:** unchanged.
- **`task.effect_completed`:** none. Each scan move is a `task.effect_intent` of the step's action
  followed by its input and by no `task.effect_completed`, as a scan's moves are
  ([selection-graph.md](selection-graph.md), "Select over a scan"); with no tap, the step has no
  `task.effect_completed` of its own.

## Lists from fields

Workflow #450 FL-1 (Runtime 0.12.59; [v012-register.md](v012-register.md) §1.340; the owner
approved the feature on 11-13, board #450). A package that reads a set of items one by one (it
opens an item's detail page, reads its name and its exact quantity as OCR fields, and steps to the
next) groups those fields into one list: a **field list**. The host writes it as one instance fact
of value `record_list` under `inventory.list.<name>`, exactly as it writes a list reading's
("List readings" above): the same record shape, row bound and order, so a reader of the fact
cannot tell the producer. **Status:** built by FL-1 (the declaration, the gate and the kernel)
with the host's halves in FL-2, one Runtime release; `package` revision 11
([component-interfaces.md](component-interfaces.md)); a Runtime before it refuses a package that
declares one (`UnknownField` at `/field_lists`).

### Declaration

`field_lists` is an optional top-level family of a task schema `0.8` or `0.9` `task.json` that
declares `post_admission_ocr` in the `fields_v1` form ([ocr-fields.md](ocr-fields.md)). The
names are neutral examples:

```json
"field_lists": [
  {"id": "list_a", "group": "row", "key_field": "name", "key_column": "id",
   "columns": [{"column": "count", "field": "amount"}],
   "minimum_confidence_milli": 900,
   "fact_key": "inventory.list.example_kind_a", "valid_for_ms": 86400000,
   "start": {"forbidden": ["mark/previous"]},
   "end": {"page_ids": ["last"]}}
]
```

| Member | Rule | Refusal (gate: `resource_declaration_invalid` at the pointer) |
| --- | --- | --- |
| the array | exactly 1 entry in this revision; task schema `0.8` or `0.9`; the task declares `post_admission_ocr` (`fields_v1`) and `scheduling_outcome` | an older schema: `UnconsumedField` @ `/field_lists`; empty or longer: `InvalidValue` @ `/field_lists`; `MissingField` @ `/post_admission_ocr` or `/scheduling_outcome` |
| `id` | the identifier rule of a mark's `id`; distinct from every operation id and mark id of the task (it names the producer in `source_detector`) | `…/id` |
| `group` | a `group` of the task's fields | `…/group` |
| `key_field` | a field of that group of type `dictionary_entry`; its canonical entry is the row's key | `…/key_field` |
| `key_column` | a token `^[a-z][a-z0-9_]{0,31}$` | `…/key_column` |
| `columns` | 0 to 63 `{column, field}`: `column` a distinct token other than `key_column`; `field` another field of the group, at most once: `unsigned_integer` whose `max` is at most 2^53−1 (an integer column) or `dictionary_entry` (a string column) | `…/columns`, `…/columns/<j>/column`, `…/columns/<j>/field` |
| the named fields | `privacy` `public`, and their targets not marked personal by the projection metadata | `…/key_field`, `…/columns/<j>/field`; admission: `<id>:target_personal` |
| `minimum_confidence_milli` | `1..=1000`, required; applies to the key and every column, on top of each target's own `minimum_confidence` | `…/minimum_confidence_milli` |
| `fact_key` | starts with `inventory.list.`, a valid instance fact key, unique among every key the task writes (marks, `read_list` steps, this list) | `…/fact_key` |
| `valid_for_ms` | `1..=31_536_000_000` | `…/valid_for_ms` |
| `start` | `{page_ids?, required?, forbidden?}` with at least one condition: `page_ids` 1 or 2 distinct field pages; `required` and `forbidden` each 0 to 8 distinct template or colour targets of the task (no OCR, so a condition makes no provider call) | `…/start`, `…/start/<member>`, `…/start/<member>/<k>` |
| `end` | `{page_ids, required?, forbidden?}`: `page_ids` 1 or 2 distinct pages, each a field page and a terminal page of the fields' `fields_recorded` mapping; the rest as `start` | `…/end/<member>`, `…/end/<member>/<k>` |
| any other member | none | `UnknownField` @ `…/<member>` |

The pointers lie below `/field_lists/<i>`. Wrong JSON types are `InvalidType` and absent members
`MissingField` at their own pointer; a floor beyond 16 bits is `InvalidValue`. The rules of the
values are one function of the contract crate (`validate_field_lists`), which the declaration
gate and admission share: the gate reports its pointer and reason, admission its detail. The
package parser checks the targets of `start` and `end` against the derived recognition pack
(`package_invalid` naming `…/start/required/<k>`, …). Admission runs the gate on the sealed task
first, then the same rule against the task's declared fields, then the targets against the
recognition pack (template or colour) and the fields against the projection metadata; it refuses
with `contained_task_resource_reading_invalid` and the detail `<id>:<member>` (`list_a:key_field`,
`list_a:start/required/0`, `list_a:target_personal`), or the task's member alone (`field_lists`,
`post_admission_ocr`, `scheduling_outcome`). A startup, stuck-recovery return-home or prerequisite
package that declares one is refused as for readings (`contained_task_resource_reading_run_kind_unsupported`;
a layer's check names `field_lists`, or `post_admission_ocr` first); a bound recovery entry,
`recognize_only` and a declared operation's one step read none. Offline, `package dry-run`
reports the task's post-admission reading as `pending_real_execution`, as for a task with marks.

### When the list is whole

The list's frames are the frames the fields collector collects, in collection order; frame *i* is
the fields report's `frame_index` *i*. The collector keeps each frame's key and column values with
the confidences of their observations, in memory; the fields report and its projection do not
change.

1. **Start.** The list's first frame meets `start`: on one of its `page_ids` when declared, every
   `required` target passing and no `forbidden` one, each judged with its own declaration as a page
   rule judges it. Otherwise the list's miss is `list_start_unseen`.
2. **End.** The first frame that meets `end` ends the list, and the list is decided on that frame.
   The end pages are terminal pages, where the run ends. A frame of the group collected after the
   list was decided fails the run (`contained_task_state_invalid`, `runtime_invariant`), so a list
   is never published with a frame after its end. A successful run that collected frames of the
   group and met no `end` is `list_end_unseen`; so is a run whose end page lay past `max_frames`
   and was not collected.
3. **A frame's reading.** The key and every column are `resolved` ([ocr-fields.md](ocr-fields.md))
   and each observation's confidence (`ocr_confidence_milli`) is present and at least
   `minimum_confidence_milli`. Otherwise the frame is unresolved: `list_position_unread`. A
   low-confidence key or value therefore fails the list, never the run (a field the package marks
   `required` still ends the run when it does not resolve, as every field does).
4. **Repeats.** A key read on two frames of the list, adjacent or not, with equal or different
   columns, is `list_duplicate_key`. The core never folds, never sums and never picks one (the
   coordinator's decision of 11-13: the measured repeats were different items sharing a display
   name). A page or tab that shows items sharing a display name cannot be a field list in this
   revision.
5. **Every item visited** rests on the start condition on the first frame, the end condition on
   the last, and one guarded, confirmed tap between consecutive frames. No total is checked.
6. **Rows.** One row per frame: `{<key_column>: <canonical dictionary entry>, <column>: <value>,
   …}`, an integer column as an integer and a string column as its canonical entry, in ascending
   byte order of the key. The record's `confidence_milli` is the lowest confidence of the fields
   the rows read. A field list is never empty, because its end frame holds a position.
7. **Order of misses.** Start, unread, repeated key, rows: the first that applies is the list's one
   miss.

| `reading_problem` | `count` | `frame_indices` |
| --- | --- | --- |
| `list_start_unseen` | the frames collected | the first frame |
| `list_position_unread` | the unresolved frames | those frames |
| `list_duplicate_key` | the repeated keys | every frame that holds a repeated key |
| `list_rows_exceeded` | 1025, the position past the bound | its frame |
| `list_end_unseen` | the frames collected | the last frame |
| `list_bytes_exceeded` | the rows read (the host's, as for a list reading) | none |

Each is the warning `contained_task_resource_reading_unresolved` (`reading_id` the list's `id`,
`fact_key`, `reading_problem`, `count`, and the optional value `frame_indices`, the fields report's
frame indices in ascending order, at most 64 with `frame_indices_total` past that), written at once
as one task diagnostic row, on the frame the list was decided on, or for `list_end_unseen` on the
frame captured last at the run's successful end. With the frame index, `run evidence` and the
fields projection show that frame's text, reason and confidence. A list that is not whole writes
nothing, in part or empty, and the key's earlier record stays. A failed or cancelled run publishes
nothing; its fields report is saved as before.

**Bounds.** Rows: at most `max_frames`, at most 256, so the 1024-row bound of a record list
("Rows" above) is checked (`list_rows_exceeded`) but cannot be reached in this revision; it is
kept, though its reason (a dictionary of at most 1024 business IDs) does not carry over to a
fields dictionary of up to 4096 entries. Bytes: the host's 512 KiB check, unchanged. Steps and
time: one run reads at most about min(`max_frames`, `max_steps` + 1, the task's timeout ÷ the time
per step) positions. A list longer than one run is split by the package (one reader, one start,
one end and one key per tab or category; a list maximum reads 1 to 16 lists), not by raising
`max_frames`, the fields contract's bound that the client's projection checks.

### Kernel and host

- **Admission.** `prepare_field_lists` (`crates/execution-kernel/src/contained_task/field_list.rs`)
  runs after the fields are prepared, with the key set of the task's marks and `read_list` steps.
  `has_list_readings` counts field lists, so the startup, layer and offline paths treat them as
  list readings.
- **The run.** On each frame of the group the collector keeps the key's and the columns' readings.
  On the list's first frame the kernel judges `start`, on each its `end`, after the frame's field
  failure check and before any reading hand-off. On the end frame, the frame captured last, it
  builds the list (`field_list_of`) and records `ListReadingFrame` when the list is whole, then
  `ListReading` with the list's declaration (`operation_id` the list's `id`), the frames a miss
  names riding with it. At a successful end, before the readings join, a list that collected frames
  and was not decided records `ListReading` with `list_end_unseen` on the frame captured last; the
  kernel notes every frame a run captures for it.
- **The host.** Every trace is one the host takes for a `read_list` step, in an order it accepts:
  a whole list is published at the run's successful end with `source_detector`
  `list_reading:<entry task>/<list id>` and `source_snapshot_id`
  `run:<run>/frame:<end frame>/<list id>`, the observation time the end frame's capture time; a
  miss is its warning row at once. "Publication", "Staleness", "Capacity and reserved keys" and
  "Evidence" above apply unchanged. FL-2 holds the host's halves on a fake device.
- **Ledger.** No record shape changes and `ledger` stays 8: a list is a `fact.published` of the
  existing `record_list` value; a miss is an existing warning row whose outcome gains two words of
  `reading_problem` and the optional value `frame_indices`, which a reader relays unchanged.

## Never written

`produces` amounts, effective weights, gaps and any other amount the policy computes are never
written back. Workflow #384 D2-S3: an adjustment's amount comes only from live recognition after
its operation is confirmed (a mark read after it, or the change the screen shows), never from
the policy or a constant in the package. No event type, no persisted field and no diagnostic
record kind is added.
