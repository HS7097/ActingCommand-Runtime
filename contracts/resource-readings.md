# Resource readings

Workflow #335 S5. A package declares, per resource, where the Runtime reads its current amount
after a task: which terminal page, which OCR target, how the text is parsed, the lowest
confidence it accepts, how long the reading stays valid and the instance fact key it becomes.
The Runtime reads only what the package declares and writes each reading back as an instance
fact. It never writes a `produces` amount, a weight or any other amount computed by the policy.

This document covers the declaration, its admission and the kernel reading (slice S5a), and the
host bridge that publishes the readings as instance facts (slice S5b, "Host" below).

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
| `page_id` | an identifier like `id` (not `any`) that is one of the task's `scheduling_outcome` terminal pages; the page gate may use any recognition backend | `InvalidValue` @ `…/page_id`; cross-reference: `package_invalid` naming `…/page_id` |
| `target_id` | an identifier like `id`, exactly one entry of the task's own `ocr_targets`, named by no page gate (`required`, `optional`, `any_of`, `forbidden`), directly or as a member of a check the page gate names | `InvalidValue` @ `…/target_id`; cross-reference: `package_invalid` naming `…/target_id`; a target the projection metadata marks personal is refused by the Runtime before any input |
| `trim` | `whitespace_v1` only | `InvalidValue` @ `…/trim` |
| `value` | `type` `unsigned_integer` only; `format` one of `ascii_decimal` (the default), `comma_grouped`, `current_capacity`; `min <= max <= 2^53-1` | `InvalidValue` @ `…/value/type`, `…/value/format`, `…/value/min`, `…/value/max` (a copied `u64::MAX` is refused at `…/value/max`) |
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

### Version gate

The family is accepted in task schema `0.8` and `0.9` only; in `0.3`–`0.7` it is
`UnconsumedField` @ `/resource_readings`. The task schema, `content-directory.v1`,
`control.json`, `resources.json` and the derived `pack.json` and `pages.json` versions are
unchanged; a task without the family parses, derives, simulates and runs exactly as before.

A Runtime older than this family refuses a package that declares it, on both load paths: a
sealed ZIP and a content directory both pass the same declaration gate, which reports
`UnknownField` @ `/resource_readings`. Nothing ignores the family silently.

### Two uses, one declaration

- An observation package: a zero-input task (`operations: []`, schema `0.8`) whose only purpose
  is to read, which is the Workflow #308 zero-input observation package.
- A reading after a task: a task that changes the resource declares the reading on its own
  terminal page.

Not provided: an amount derived from a settlement (a constant or predicted amount), an
increment reading (`+N`) and a value read from a pixel target. The value type is a closed set.

## Kernel reading

Readings are taken only when a run completes successfully on a page:

- In `finish_success`, which every successful completion reaches: after the operation `0.8`
  fields report (when the task has one) and always before `Finalizing`.
- Only from the terminal observation's own frame. There is no further capture and no input.
- Only the declarations whose `page_id` matches the completed page. A run that completes on
  another allowed terminal page reads nothing and still succeeds; the ledger shows it as a
  `task.terminal_committed` `final_page` that differs from the declared `page_id`, and nothing
  else is recorded.
- Not in offline simulation: `actinglab package dry-run` reads nothing and reports
  `post_admission_ocr.pending_real_execution: true` for a task that declares OCR fields or
  readings.
- Not in a bound recovery entry and not in `recognize_only` (which cannot declare a scheduling
  outcome).

Each reading evaluates its OCR target once on the terminal scene. The evaluation is recorded in
the existing task diagnostic stream with the phase label `post_admission`, then the text is
trimmed and parsed and the confidence converted. A reading holds when the parse succeeds and the
confidence is present and at least `minimum_confidence_milli`. The first reading that does not
hold fails the run with `contained_task_resource_reading_unresolved` and the detail
`<id>:<reason>`, where the reason is the snake-case `OcrFieldReason` (`empty`,
`invalid_integer`, `overflow`, `out_of_range`, `region_unresolved`, `provider_failed`),
`confidence_missing` or `low_confidence`. This happens before `Finalizing`, so no fact is
written and an earlier fact of the key stays until its own expiry. The raw text and confidence
remain in the task diagnostic. A schema `0.8` fields report already recorded by that
completion stays the only one; the failure does not record it again.

When every bound reading holds, the kernel emits the in-memory trace
`ContainedTaskTrace::ResourceReadings { captured_at, readings }` with the terminal frame's
device capture time and, per reading, the declaration, the integer value and the confidence
milli. The trace is never serialized. `evaluate_resource_readings` is the exported function the
kernel uses for this evaluation.

Before any input, the Runtime refuses a package whose reading target is not one OCR target, is
named by a page gate (directly or as a member of a composite target), or is marked personal by the projection metadata, with
`contained_task_resource_reading_invalid` and the detail `<id>:<reason>` (`target_not_ocr`,
`target_in_page_gate`, `target_personal`). A program that violates the declaration rules is
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

**The trace.** The host accepts `ResourceReadings` once per run and only before `Finalizing`,
and only for the frame it captured last: the trace's `captured_at` must equal the capture time
the host kept with that frame's id. A second trace or one after `Finalizing`
(`contained_task_resource_reading_duplicate`), a trace before any frame
(`contained_task_resource_reading_frame_missing`) or of another frame
(`contained_task_resource_reading_frame_mismatch`) poisons the Runtime. The readings then travel
in memory on the run's terminal draft.

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
| `key` | `fact_key` |
| `content` | inline `integer`, the reading |
| `observed_at_unix_ms` | the terminal frame's device capture time in Unix ms. It is wall clock time, the clock domain of the Runtime clock that refuses an observation in the future |
| `expires_at_unix_ms` | `observed_at_unix_ms + valid_for_ms` |
| `ttl_policy` | `{"minimum_ms": valid_for_ms, "maximum_ms": valid_for_ms, "source": "detector_contract"}` |
| `confidence_milli` | the reading's `ocr_confidence_milli` |
| `source_detector` | `resource_reading:<entry task id>/<reading id>` |
| `source_snapshot_id` | `run:<run id>/frame:<frame id>/<reading id>`, the canonical `run_<hex>` and `frame_<hex>` ids |
| `schema_version` | `fact.v1` |
| `resource_bundle_hash` | a content directory's `sha256`; a sealed ZIP's SHA-256; for a source tree reference, SHA-256 of its prefixed wire JSON |
| `invalidate_on` | `[]` |

The event has source `runtime`, actor `runtime`, origin module `fact-store` and system links.
It carries no run link: the run and the frame are named in `source_snapshot_id`.

**When.** Only a terminal whose outcome is `Success` and that carries readings goes through the
four stages below. Every other terminal, including every run without readings, runs the original
terminal flow once, unchanged.

0. No lock is held. The active catalog is read under the `policy` lock, as the policy forward
   projection reads it, and the lock is released. If a pool with `value_source: ledger_fact`
   observes a reading's `fact_key` in the run's instance scope, nothing is published and the
   terminal becomes the failure `contained_task_resource_reading_live_pool_unsupported`. Only
   pool bindings are read, never a score.
1. Under `fact_write_gate`, the original checks run: the run's chain, an already committed
   terminal, the capture summary and the settlement projection. Then the gate is released. An
   already committed terminal is rejected as before; a refused projection is rewritten as before
   (`contained_task_outcome_*`). Neither publishes. A successful projection of this terminal is
   the confirmed settlement.
2. No lock is held. Each reading, in declaration order, is published through `publish_facts`
   (ordinary purpose) as its own single-record observation. A non-fatal refusal stops
   publishing: the readings already written stay, and the terminal becomes the failure
   `contained_task_resource_reading_rejected`. A fatal store failure poisons the Runtime, as for
   every publication.
3. The gate is taken again and the original flow runs in full: the chain, an already committed
   terminal, the summary, the projection, the summary and terminal appends and the failed
   terminal note.

A refusal in stage 0 or 2 rewrites the terminal as a refused projection does: outcome `Failure`
with that code, no final page, no settlement, and the projection failure severity (`warning`
for a policy run). The request fails as `Failed` with that code and the failure terminal; the
detail is the failure's native detail.

`publish_facts` takes `fact_write_gate` itself, so nothing is published under the gate, and the
`policy` lock is never taken under it. Between stage 2 and stage 3 another path can commit the
run's terminal. Stage 3 then rejects the attempt as
`contained_task_terminal_already_committed`, and the facts stay written: they are real
observations of the terminal frame, and the rejection is reported on its own.

**Ledger order.** `Finalizing` is the event `task.terminal_intent` and `TerminalCommitted` is
`task.completed` or `task.failed`.

- Success: the terminal frame's recognition, `Finalizing` (`success`), `fact.published` once
  per reading, `capture.summary_committed`, `TerminalCommitted` (`success`).
- Refused in stage 0 or 2: `Finalizing` (`success`), the readings published before the refusal,
  `capture.summary_committed`, `TerminalCommitted` (`failure`). This is the same sequence as for
  a refused settlement projection.

**Failure codes.** `TerminalCommitted.failure_code` holds the code alone. The detail is the
native detail of the run's host failure (`RuntimeHostError`). A policy run's driver receives
that error, and its failure record keeps the detail when the error was not already recorded.
The IPC receipt of a manual `task-run` carries the code (`host_code`) but has no native detail
field.

| Code | When | Native detail | Reading why from the ledger |
| --- | --- | --- | --- |
| `contained_task_resource_reading_unresolved` | a reading does not hold, before `Finalizing` | `<id>:<reason>` | the OCR text and confidence in the task diagnostic |
| `contained_task_resource_reading_invalid` | a declaration refused before any input | `<id>:<reason>` | the package admission refusal |
| `contained_task_resource_reading_run_kind_unsupported` | a startup or return-home package declares readings | none | the package's `runtime.failed` |
| `contained_task_resource_reading_live_pool_unsupported` | stage 0 | `<id>:<pool id>` | the active catalog's `ledger_fact` pool of that key |
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
stuck recovery.

**Live pools.** Readings do not supply `ledger_fact` pools. Such a pool requires its records to
carry `input.committed` and `input.failed` in `invalidate_on`, and a reading carries neither,
so every later evaluation would fail with `pool_fact_invalidation_binding_missing`. Stage 0
therefore refuses. If an active record without those invalidations exists for such a pool's
key, recover by publishing a newer observation of the key by hand, with
`invalidate_on: ["input.committed", "input.failed"]`; it must pass the input boundary check
(`docs/live-fact-pools.md`).

## Scheduling connection

The only connection to the scheduling catalog is `fact_key`: it equals the
`observation: {"kind": "fact", "fact_key": ...}` of a pool (`contracts/scheduling/`), and the
reading supplies only pools observed through such a fact. A pool observed through a task
outcome is unchanged and never supplied by a reading. No pool id and no `resources.json` id is
named; the fact scope is always the instance of the run. The integer is the amount in the
pool's `valuation.unit` (Workflow #335 S2), without conversion.

An agent should not publish a reading's key by hand once a package reads it. A manual
observation that combines that key with other keys makes a later single-key reading an
incomplete refresh (`fact_observation_incomplete_refresh`), and a newer manual observation makes
it `fact_observation_not_newer`; publish at most that one key per observation.

## Never written

`produces` amounts, effective weights, gaps and any other amount the policy computes are never
written back. No event type, no persisted field and no diagnostic record kind is added.
