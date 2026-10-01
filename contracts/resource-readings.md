# Resource readings

Workflow #335 S5. A package declares, per resource, where the Runtime reads its current amount
after a task: which terminal page, which OCR target, how the text is parsed, the lowest
confidence it accepts, how long the reading stays valid and the instance fact key it becomes.
The Runtime reads only what the package declares and writes each reading back as an instance
fact. It never writes a `produces` amount, a weight or any other amount computed by the policy.

This document covers the declaration, its admission and the kernel reading (slice S5a). The
host bridge that publishes the readings as instance facts is slice S5b; until it lands, a run
that takes a reading fails as described in "Host" below.

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
| `target_id` | an identifier like `id`, exactly one entry of the task's own `ocr_targets`, named by no page gate (`required`, `optional`, `any_of`, `forbidden`) | `InvalidValue` @ `…/target_id`; cross-reference: `package_invalid` naming `…/target_id`; a target the projection metadata marks personal is refused by the Runtime before any input |
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

## Host

Slice S5a has no fact writer. The host answers the reading trace with the request failure
`contained_task_resource_reading_unsupported`: the run ends before `Finalizing` with a failure
terminal carrying that code, no `fact.published` is appended and the Runtime is not poisoned.
Slice S5b replaces this with the publication through `publish_facts`.

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
