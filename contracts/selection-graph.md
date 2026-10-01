# Selection graph

This document collects the selection graph of
[Workflow #308](https://github.com/HS7097/ActingCommand-Workflow/issues/308). Each section is
frozen by the #308 slice that adds it; this revision holds the sections **Checks**,
**Candidate layouts** and **Records**. Nodes, edges, gates and selection are frozen by later
#308 slices. No game-specific values are part of this contract.

## Checks

A check is a named combination of existing recognition targets that is judged as one
unit. The check states which targets it combines, and so which backends; each member keeps
its own threshold. No package is forced onto a single backend.

Implementation status: recognition pack schema `0.7` admits and evaluates `composite` and
`color_digest` targets and per-target color `max_distance`. The source side (the `checks`
family, digest entries and `max_distance` in task sources, `guard.check`, and a digest
named by `guard.color_probe`) is admitted by the source parser in a later #308 slice.

### Declaration

A task source declares checks in the top-level `checks` family of `task.json`. The family
is accepted for task schema `0.6` through `0.9`; the task schema version does not change.

```json
"checks": [
  {"id": "check/claim_ready", "all_of": ["digest/claim_button", "ocr/claim_text"]},
  {"id": "check/home_any", "any_of": ["page/home", "digest/home_bar"]}
]
```

Each check derives one recognition target of type `composite`:

```json
{"type": "composite", "id": "check/claim_ready", "mode": "all_of", "members": ["digest/claim_button", "ocr/claim_text"]}
```

- Exactly one of `all_of` and `any_of` is declared; it becomes `mode`.
- A check has 2 to 8 members, and no member appears twice.
- Every member is an existing `template`, `color`, `color_digest`, `ocr` or `nn` target.
  A composite never nests another composite and never has a `click_only` member.
- Check IDs share one namespace with all target IDs. The same ID with an identical
  definition is kept once; the same ID with a different definition is rejected, as for OCR
  targets. The older declaration families keep their first-declaration rule.
- Thresholds stay on the member targets. A composite is never clicked and never locates
  anything.

### Pack schema `0.7`

Only `pack.json` is written at schema `0.7`, and only when it uses a construct of that
version: a `composite` target, a `color_digest` target (see `color-digest.md`), or a
per-target color `max_distance` (on a `color` target or a template's `color_check`). Every
other derived document, including the page set, stays at `0.6`. Packs of schema `0.1` and
`0.3` through `0.6` load and judge exactly as before. A reader that accepts only `0.1`
through `0.6` rejects a `0.7` pack explicitly, and a `0.7` construct in an older pack is
rejected with the pointer of the offending field.

A per-target `max_distance` is a finite number `>= 0`. When it is absent, the package's
`defaults.color_max_distance` applies, as before. The color evaluation records the
threshold that was applied.

### Evaluation

- Every member is evaluated, in declaration order, through the same scene evaluation as
  the rest of the frame. A template member reuses that frame's template result; the
  composite result itself is not cached, because OCR output is never cached
  (`ocr-fields.md`).
- `all_of` passes when every member passed; `any_of` passes when at least one member
  passed. Evaluation does not stop at the first decisive member.
- An evaluation error of any member is the composite's error, in both modes. It is
  reported on the existing error path with the composite and member IDs, and it is never
  counted as a member that did not pass. Evaluation stops at that member, as a page does.
- The composite's evaluation lists every member as `{target_id, passed, evaluation}` with
  the member's own evaluation, and its message names the members that did not pass. The
  members' PP-OCR reports travel with the composite on success and on error. A member keeps
  its own privacy treatment wherever the composite is serialized, and a composite with a
  personal member is itself personal in page projections and observation facts
  (`page-projection.md`).
- Serialized evaluations gain a `composite` object, and a `color_digest` object for digest
  targets, only for those kinds. Every other evaluation serializes exactly as before.

### Where a check may be used

| Place | Composite | Color digest |
|---|---|---|
| Page rules `required`, `any_of`, `optional`, `forbidden` | Yes | Yes |
| Operation guard | Yes, with `guard.check` | Yes, with `guard.color_probe` |
| `target`, `target_center` and `offset` clicks | No | No |
| Page gating post-admission OCR | Only when every member is a template, color or color digest target | Yes |
| OCR field `target_id`, OCR relative anchor, navigation `target_center` | No | No |
| Lab observe, is-visible, probe `ObserveTargets` | Yes, without geometry | Yes, without geometry |
| Lab `wait --stable` | Rejected explicitly | Yes, by its region |

A post-admission OCR field target is never referenced by a page, directly or as a member of
a composite the page references.

### Relation to pages

A page is `all_of(required)`, each `any_of` group, and `none_of(forbidden)`; these fields
do not change. A page rule references a check by its ID like any other target. The task
record keeps its shape: `task.recognition_completed` records a composite as one recognized
target without a region, and a color digest target with its declared region.

### Selection-graph mapping

- A node's recognition slot names a check ID; checks replace custom recognition.
- A direct hit is a node without a check.
- Inversion belongs to the gate.
- Numeric gates on OCR field values, such as a value at or above a threshold, are not part
  of v0.9. They are a registered gap, not an implied behavior.

## Candidate layouts

A candidate layout declares, for one page, where the candidates of a select step lie and which
feature values each candidate carries. On one frame it yields a candidate projection
([candidate-projection.md](candidate-projection.md)), which a selection policy evaluates.

Implementation status: recognition pack schema `0.7` admits `fixed_slots` layouts and the
recognition pack projects them. The source side (the `candidate_layouts` family of `task.json`
and its derivation into `pack.json`) and `repeated_anchor` layouts are frozen and admitted by
later #308 slices.

### Pack schema `0.7`

The derived `pack.json` carries the layouts in its top-level `candidate_layouts`:

```json
"candidate_layouts": [
  {"id": "layout/main_slots", "page_id": "list_page", "kind": "fixed_slots",
   "features": [
     {"name": "open", "value": "passed"},
     {"name": "marked", "value": "passed"},
     {"name": "mark_score", "value": "measure_milli"}],
   "slots": [
     {"rect":  {"x": 214, "y": 204, "width": 176, "height": 90},
      "click": {"x": 224, "y": 214, "width": 156, "height": 60},
      "targets": {"open": "state/slot_0_open", "marked": "ui/slot_0_mark", "mark_score": "ui/slot_0_mark"}}]}]
```

- Only recognition pack schema `0.7` accepts the key. A pack of any other schema that has it
  is rejected with `UnconsumedField` at `/candidate_layouts`. An absent key means no layouts,
  and a pack without layouts loads and judges exactly as before.
- Unknown fields and wrong JSON types are rejected with the pointer of the field, and so are a
  `kind` other than `fixed_slots` and a feature `value` other than `passed` or
  `measure_milli`.
- A slot's `targets` maps a feature name to the ID of one of the pack's own targets, the
  derived target ID, as a check member does. A slot may leave a declared feature out; that
  feature is then absent from the slot's candidate, and the selection policy's `on_unknown`
  decides.
- Each slot reads existing targets. Evaluating one template over the regions of several slots
  is not used by `fixed_slots` layouts.
- A layout declares no privacy. A candidate's privacy derives from the targets its features
  read ([candidate-projection.md](candidate-projection.md)).

### Rules

The recognition pack checks every rule that needs only the pack when its evaluator is built.
Each failure message starts with the pointer of the offending field.

| Rule | Pointer |
|---|---|
| The ID matches `^[a-z0-9][a-z0-9_./-]{0,63}$` and is unique in the pack. | `/candidate_layouts/i/id` |
| The page ID is non-empty, at most 256 bytes and free of control characters; a page has at most 4 layouts. | `/candidate_layouts/i/page_id` |
| A pack has at most 64 layouts. | `/candidate_layouts` |
| The kind is `fixed_slots`. | `/candidate_layouts/i/kind` |
| 1 to 8 features, with distinct names matching `^[a-z][a-z0-9_]{0,31}$`. | `/candidate_layouts/i/features`, `…/features/j/name` |
| 1 to 64 slots. | `/candidate_layouts/i/slots` |
| `rect` and `click` have a non-negative origin and a positive size and lie entirely inside `coordinate_space`. | `…/slots/k/rect`, `…/slots/k/click` |
| Each `targets` key is a declared feature; its target exists and is a `template`, `color`, `color_digest`, `composite`, `ocr` or `nn` target, never `click_only`; a `measure_milli` feature never reads a composite. | `…/slots/k/targets/<name>` |
| One projection needs at most 16 OCR and NN evaluations: one per distinct OCR or NN target the slots read, and one per OCR or NN member of each distinct composite they read. | `/candidate_layouts/i/slots` |

`coordinate_space` is required for every pack schema, so layouts need no rule of their own for
it.

The pack has no pages. A loader that holds the page set checks that each layout's `page_id`
is a declared page, after it has validated the page set against the pack, and rejects the
package with a message that names `/candidate_layouts/i/page_id`: contained task admission
with `contained_task_recognition_invalid`, and online observation preparation with
`observation_resources_invalid`.

### Feature values

| Target kind | `passed` | `measure_milli` | `confidence` |
|---|---|---|---|
| `template` | verdict | `floor(score × 1000)` | `floor(score × 1000)` |
| `color` | verdict | `floor(distance × 1000)` | `null` |
| `color_digest` | verdict | `mean_milli` | `null` |
| `composite` | verdict | not allowed | `null` |
| `ocr` | verdict | `floor(confidence × 1000)` | `floor(confidence × 1000)` |
| `nn` | verdict | `floor(selected_score × 1000)` | `floor(selected_score × 1000)` |

A `passed` feature is a boolean and a `measure_milli` feature an integer. A score is widened
from `f32` to `f64` before it is multiplied. An OCR result without a confidence or an NN result
without a selected score has no measure and no confidence: its `measure_milli` feature is
absent and its `confidence` is `null`; no value is defaulted.

## Records

### `task.selection_evaluated`

A select step records its decision in the task ledger as the event
`task.selection_evaluated` (family `task`), carrying the task fact

```json
{"kind": "selection_evaluated", "step_index": 3, "operation_label": "choose_slot",
 "selection": { … }}
```

Each select attempt appends exactly one, before any input and before any failure return.
Implementation status: the event type, the fact and the record exist; no producer appends
them yet. The host emits them with the in-task select step of a later #308 slice. A package
without a select step never produces the event.

The record has the shape of a `policy.*` decision: identity hashes and a complete breakdown.

| Field | Meaning |
| --- | --- |
| `layout_id`, `page_id` | The evaluated layout and its page; equal to the projection's. |
| `projection` | The full core candidate projection of the step's frame ([candidate-projection.md](candidate-projection.md)), including its `candidate_set_sha256`. |
| `policy.path` | The package-relative policy document path the step declares. |
| `policy.package_sha256` | SHA-256 of the document's bytes in the package, the step's declared `sha256` (64 lowercase hex). |
| `policy.policy_sha256` | The evaluator's canonical document identity, `sha256:<hex>`. |
| `policy.policy_id` | The document's `policy_id`. |
| `fact_snapshot_id`, `input_ledger_position` | The instance fact snapshot the evaluation read and its ledger position. |
| `now_unix_ms` | The instant the host supplied to the evaluation. |
| `input_sha256` | The evaluator's canonical identity of candidates, facts and instant, `sha256:<hex>`. |
| `outcome`, `outcome_key`, `selected[]` | The decision, its resource-declared key and the chosen candidate IDs. |
| `verdicts[]` | One breakdown per evaluated candidate: status, score, rank, gates, terms, reasons. |
| `reasons[]` | The decision's reason chain, `{code, detail}`. |
| `confirmation` | How the confirmation frame compared, below. |

`confirmation` is one of

| `kind` | When |
| --- | --- |
| `not_attempted` | The outcome is not `selected`. |
| `matched` `{candidate_set_sha256}` | The confirmation frame's candidate set hashes like the evaluated one. |
| `mismatched` `{candidate_set_sha256}` | It hashes differently; no input follows. |
| `page_changed` | The confirmation frame is not the step's page. |
| `guard_failed` `{code}` | The step's guard failed on the confirmation frame. |
| `capture_failed` `{code}` | The confirmation frame could not be captured. |

A confirmation is attempted exactly when the outcome is `selected`. Together with the
projection, a matched or mismatched record holds both candidate-set hashes.

#### Evaluator mirror

The breakdown types belong to the contract and mirror the selection-policy evaluator
(`crates/selection-policy/src/evaluator.rs`, `facts.rs`) field for field, with the same serde
shape. The contract does not depend on the evaluator; the kernel converts one into the other,
and the evaluator is unchanged.

| Contract type | Evaluator type | Fields and serde shape |
| --- | --- | --- |
| `TaskSelectionOutcome` | `SelectionOutcome` | tagged by `kind`: `selected{count}`, `empty`, `insufficient{surviving, required}`, `ambiguous{candidate_ids}`, `unknown{reason, detail}` |
| `TaskSelectionUnknownReason` | `UnknownReason` | `fact_missing`, `fact_expired`, `fact_stale`, `fact_low_confidence`, `fact_not_scalar`, `field_missing`, `type_mismatch`, `lookup_miss` |
| `TaskSelectionVerdict` | `CandidateVerdict` | `candidate_id`, `status`, `score_milli` (absent when none), `rank` (absent when none), `gates`, `terms`, `reasons` |
| `TaskSelectionCandidateStatus` | `CandidateStatus` | `ranked`, `gate_rejected`, `unknown_dropped` |
| `TaskSelectionGateResult` | `GateResult` | `gate_id`, `outcome` |
| `TaskSelectionGateOutcome` | `GateOutcome` | tagged by `kind`: `passed`, `failed`, `unknown_substituted{reason, passes}`, `unknown_dropped{reason}` |
| `TaskSelectionTermResult` | `TermResult` | `term_id`, `outcome`, `weight_milli`, `contribution_milli` |
| `TaskSelectionTermOutcome` | `TermOutcome` | tagged by `kind`: `scored{transformed_milli}`, `unknown_substituted{reason, transformed_milli}`, `unknown_dropped{reason}` |
| `PolicyReasonRecord` | `DecisionReason` | `code`, `detail` |

The record takes the remaining decision fields under its own names: `policy_id` and
`policy_sha256` into `policy`, `candidate_layout_id` as `layout_id`, `evaluated_at_unix_ms` as
`now_unix_ms`, and `candidates` as `verdicts`; `input_sha256`, `fact_snapshot_id`, `outcome`,
`outcome_key`, `selected` and `reasons` keep their names. The decision's `schema_version` is
not recorded: it is the schema of the policy document that `policy_sha256` identifies.

#### Validation and bounds

A record is checked when it is written and when it is read:

- the projection is valid and its sealed hash equals the recomputed one; `layout_id` and
  `page_id` equal the projection's;
- `policy.package_sha256` is 64 lowercase hex, `policy_sha256` and `input_sha256` are
  `sha256:<hex>`, `fact_snapshot_id` is a token, `input_ledger_position` and `now_unix_ms` are
  positive;
- at most 64 verdicts, each for a distinct actionable candidate of the projection;
- `selected` holds exactly `count` distinct IDs of `ranked` verdicts for a `selected` outcome
  and nothing otherwise; `selected` has a positive count and `insufficient` fewer survivors
  than required;
- `matched` carries the projection's hash and `mismatched` a different 64-hex hash;
- reason codes and confirmation codes are tokens, reason details text of at most 1024 bytes,
  the decision chain holds 1–128 reasons, as for a policy dispatch; labels and IDs are at most
  256 bytes without control characters.

A structural violation is `invalid_task_selection_record` with the field it concerns. Only
the writer measures bytes, on the current encoding: the projection within its 32 KiB budget
(`candidate_projection_budget_exceeded`) and the record's compact JSON within 64 KiB
(`selection_record_too_large`). The producer runs the same check before any input, so an
oversized record fails the task and nothing is truncated.

#### Ledger placement

- Durability: durable. It is the decision an input follows.
- Views: the event stream, Errors by severity and Lab by request context. The Observation
  and Changes type lists are compiled into the stored view DDL and compared at every open,
  so the event joins them only together with a view schema upgrade.
- Privacy: the ledger is a controlled surface. v1 values are booleans and integers, like the
  targets of `task.recognition_completed`, and the record carries no text read from the
  screen. The `Ui` and `Normal` profiles project it in the public payload like every task
  fact.
- Compatibility: the event is an added variant; no existing persisted structure gains a
  field, and every existing record re-serializes to the same bytes. A reader built before the
  variant rejects a ledger that contains it, so the UI and `actingledger` move to a Runtime
  that knows it before such a ledger reaches them.
