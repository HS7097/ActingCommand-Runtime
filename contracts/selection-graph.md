# Selection graph

This document collects the selection graph of
[Workflow #308](https://github.com/HS7097/ActingCommand-Workflow/issues/308). Each section is
frozen by the #308 slice that adds it; this revision holds the sections **Checks**,
**Candidate layouts**, **Select step** and **Records**. Nodes, edges and gates are frozen by
later #308 slices. No game-specific values are part of this contract.

## Checks

A check is a named combination of existing recognition targets that is judged as one
unit. The check states which targets it combines, and so which backends; each member keeps
its own threshold. No package is forced onto a single backend.

Implementation status: recognition pack schema `0.7` admits and evaluates `composite` and
`color_digest` targets and per-target color `max_distance`. The source parser admits the
`checks` family, digest entries and `max_distance` in task sources, `guard.check`, and a
digest named by `guard.color_probe` (`resource-declarations.md`, section Pack schema `0.7`
declarations).

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
version: a `composite` target, a `color_digest` target (see `color-digest.md`), a
per-target color `max_distance` (on a `color` target or a template's `color_check`), or a
candidate layout (section Candidate layouts). Every
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

Implementation status: recognition pack schema `0.7` admits `fixed_slots` and
`repeated_anchor` layouts and the recognition pack projects them. The source parser admits the
`candidate_layouts` family of `task.json` and derives it into `pack.json` (section Source
declaration).

### Source declaration

A task source declares its layouts in the top-level `candidate_layouts` family of `task.json`,
in the shape of the pack below, with `page_id` naming one of the task's pages as everywhere
else in `task.json` and `targets` naming derived target IDs, as check members do. The family
is accepted for task schema `0.6` through `0.9`; the task schema version does not change. An
older task schema refuses it with `UnconsumedField` at `/candidate_layouts`.

The declaration gate (`resource validate` and every parse) checks each layout's structure
against `task.json`'s own `coordinate_space`. The parser then checks the references while it
derives the pack. Each refusal names the task's `task.json` and the pointer of the offending
field:

| Rule | Pointer | Checked by |
| --- | --- | --- |
| The fields of the layout's kind, of the right JSON types: `id`, `page_id`, `kind` and `features`, with `slots` for `fixed_slots`, or `anchor`, `max_instances`, `order`, `instance_rect`, `click` and the optional `suppress_iou_milli` and `readable_band` for `repeated_anchor`. Unknown fields and fields of the other kind are refused. | the field | gate |
| The ID matches `^[a-z0-9][a-z0-9_./-]{0,63}$`. | `/candidate_layouts/i/id` | gate |
| The kind is `fixed_slots` or `repeated_anchor`. | `/candidate_layouts/i/kind` | gate |
| 1 to 8 features `{name, value}`, with distinct names matching `^[a-z][a-z0-9_]{0,31}$` and a value of `passed`, `measure_milli`, `identity` or `ocr_integer`. A feature may add its own `target` (a string) and `offset` (`{x, y}` integers), both or neither; every `repeated_anchor` feature declares both. | `…/features`, `…/features/j/name`, `…/features/j/value`, `…/features/j/target`, `…/features/j/offset` | gate |
| A `fixed_slots` layout has 1 to 64 slots `{rect, click, targets}`. | `/candidate_layouts/i/slots` | gate |
| `rect` and `click` are integer rectangles with a non-negative origin and a positive size, entirely inside the task's `coordinate_space`. | `…/slots/k/rect`, `…/slots/k/click` (a field's type or sign at `…/x` and so on) | gate |
| Each `targets` key is a declared feature that reads no target of its own, and each value a string. | `…/slots/k/targets/<name>` | gate |
| A `repeated_anchor` layout names its `anchor` (a string), `max_instances` 1 to 64, an `order` of `top_to_bottom` or `left_to_right`, and optionally `suppress_iou_milli` 0 to 999. | `/candidate_layouts/i/anchor`, `…/max_instances`, `…/order`, `…/suppress_iou_milli` | gate |
| Its `instance_rect` and `click` are integer rectangles relative to the anchor match: a signed origin and a positive size. Its optional `readable_band` is an integer rectangle with a non-negative origin and a positive size, entirely inside the task's `coordinate_space`. | `/candidate_layouts/i/instance_rect`, `…/click`, `…/readable_band` (a field's type or value at `…/x` and so on) | gate |
| The page is a page the declaring task itself declares (its `entry_page`, `target_page`, `error_pages`, `scheduling_outcome` terminal pages, or an operation's `from`, `to` or `expect_after.page_id`). Every build of the task therefore holds it. | `/candidate_layouts/i/page_id` | parser |
| Each slot target is a `template`, `color`, `color_digest`, `composite`, `ocr` or `nn` target of the derived pack; a `measure_milli` feature never reads a composite. Every layout of the selected tasks needs its targets in the same build, as a check does. | `…/slots/k/targets/<name>` | parser |
| A feature's own `target` is a `template`, `color`, `color_digest` or `ocr` target of the derived pack: an `nn` or `composite` feature cannot be read at an offset. | `…/features/j/target` | parser |
| A `repeated_anchor` layout's `anchor` is a `template` target of the derived pack. | `/candidate_layouts/i/anchor` | parser |
| The same layout ID with an identical derived definition, repeated by several tasks, is kept once; any other reuse of the ID is refused at the entry that reuses it. Layout IDs have their own namespace, separate from target IDs. | `/candidate_layouts/i/id` | parser |

The derived layout keeps the source's structure, with two normalizations: `page_id` becomes the
full page-set ID `<game>/<page>` that the page set declares and the load-site check compares
exactly (an ID that already holds `/` is kept as written), and the fields of a layout, a
feature, a slot and a rectangle are written in the order of the examples below; a slot's
`targets` keep their declared order. A feature writes `name`, `value`, `target` and `offset`,
then `identity`, `consensus` and `integer` as declared. Layouts are written in the order the tasks declare them,
after `targets`. The pack's own rules (layouts per page and per pack, OCR and NN evaluations per
projection) are checked by the recognition pack when the derived pack is validated or loaded
(section Rules).

`pack.json` is written at schema `0.7` when it holds a layout; the page set, navigation,
operation index and primitives stay at `0.6`. A source without layouts derives every document
byte for byte as before.

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

A `repeated_anchor` layout declares one anchor template instead of slots. Every accepted match
of the anchor is one instance; `instance_rect`, `click` and each feature's `offset` are
relative to the match's top-left corner:

```json
{"id": "layout/rows", "page_id": "list_page", "kind": "repeated_anchor",
 "anchor": "ui/row_marker", "max_instances": 12, "order": "top_to_bottom", "suppress_iou_milli": 0,
 "instance_rect": {"x": -20, "y": -10, "width": 600, "height": 70},
 "click": {"x": 400, "y": 5, "width": 120, "height": 40},
 "readable_band": {"x": 0, "y": 120, "width": 1280, "height": 480},
 "features": [{"name": "open", "value": "passed", "target": "state/row_open", "offset": {"x": 480, "y": 10}}]}
```

- Only recognition pack schema `0.7` accepts the key. A pack of any other schema that has it
  is rejected with `UnconsumedField` at `/candidate_layouts`. An absent key means no layouts,
  and a pack without layouts loads and judges exactly as before.
- Unknown fields, fields of the other layout kind and wrong JSON types are rejected with the
  pointer of the field, and so are a `kind` other than `fixed_slots` or `repeated_anchor`, an
  `order` other than `top_to_bottom` or `left_to_right`, and a feature `value` this runtime
  does not know.
- The anchor's `region` is the search region, and its threshold and a `template_relative`
  `color_check` apply at every position. `order` numbers the matches: `top_to_bottom` by
  (`y`, `x`), `left_to_right` by (`x`, `y`). Matches above `max_instances` fail the
  projection, and a search that runs out of its 5 s limit fails with
  `candidate_search_incomplete` ([candidate-projection.md](candidate-projection.md), section
  Generation).
- `readable_band` is an absolute rectangle; absent, it is the whole frame. An instance is
  actionable, and its features are evaluated, only when its `instance_rect` and its `click`
  both lie inside the band. Any other instance keeps its anchor position and rectangles in the
  projection.
- A slot's `targets` maps a feature name to the ID of one of the pack's own targets, the
  derived target ID, as a check member does. A slot may leave a declared feature out; that
  feature is then absent from the slot's candidate, and the selection policy's `on_unknown`
  decides.
- Each slot reads existing targets. A feature of either kind may instead declare its own
  `target` and `offset`: it reads that target with its region moved to the instance origin
  (the slot rectangle's origin, or the anchor match's top-left corner) plus the offset, keeping
  the target's own size. A template read this way goes through the recognition pack's template
  region evaluation, the evaluation of one template over the regions of several instances.
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
| The kind is `fixed_slots` or `repeated_anchor`, and the layout declares only its own kind's fields; a `fixed_slots` layout declares no `readable_band`. | `/candidate_layouts/i/kind`, the field |
| 1 to 8 features, with distinct names matching `^[a-z][a-z0-9_]{0,31}$`. | `/candidate_layouts/i/features`, `…/features/j/name` |
| A `fixed_slots` layout has 1 to 64 slots; a `repeated_anchor` layout has none. | `/candidate_layouts/i/slots` |
| A `repeated_anchor` anchor is a raw `template` target with a static search region; a `color_check`, when it declares one, is `template_relative`. | `/candidate_layouts/i/anchor` |
| `max_instances` is 1 to 64, `order` is declared, `suppress_iou_milli` is 0 to 999, `instance_rect` and `click` have a positive size, and `readable_band` has a non-negative origin and a positive size and lies inside `coordinate_space`. | `…/max_instances`, `…/order`, `…/suppress_iou_milli`, `…/instance_rect`, `…/click`, `…/readable_band` |
| Every `repeated_anchor` feature reads a target at an offset. Its consensus samples one frame only, so its samples may differ in jitter and template metric but never in frame: a scan cannot capture further frames. | `…/features/j`, `…/features/j/consensus` |
| A feature's `target` and `offset` appear together. The target exists, is raw, and is a `template`, `color`, `color_digest` or `ocr` target with a rectangle region; a template's `color_check` is `template_relative`; an identity or `ocr_integer` feature reads OCR, and an icon identity reads its template pool instead. | `…/features/j`, `…/features/j/target`, `…/features/j/identity` |
| The region a feature reads at an offset, for each of its samples, lies inside the `instance_rect`, or inside each slot's `rect`; a slot's `targets` never map such a feature. | `…/features/j/offset`, `…/slots/k`, `…/slots/k/targets/<name>` |
| `rect` and `click` have a non-negative origin and a positive size and lie entirely inside `coordinate_space`. | `…/slots/k/rect`, `…/slots/k/click` |
| Each `targets` key is a declared feature; its target exists and is a `template`, `color`, `color_digest`, `composite`, `ocr` or `nn` target, never `click_only`; a `measure_milli` feature never reads a composite. | `…/slots/k/targets/<name>` |
| One projection needs at most 16 OCR and NN evaluations: one per distinct OCR or NN target the slots read, and one per OCR or NN member of each distinct composite they read. One instance of a `repeated_anchor` layout needs at most 16; the producer checks the product with the actionable instances on each frame. | `/candidate_layouts/i/slots`, `…/features` |

`coordinate_space` is required for every pack schema, so layouts need no rule of their own for
it.

The pack has no pages. A loader that holds the page set checks that each layout's `page_id`
is a declared page, after it has validated the page set against the pack, and rejects the
package with a message that names `/candidate_layouts/i/page_id`: contained task admission
with `contained_task_recognition_invalid`, and online observation preparation with
`observation_resources_invalid`.

### Feature values

Identity and OCR integer values, explicit attribute fallback, and finite consensus are defined
in [business-identity-consensus.md](business-identity-consensus.md). They extend this same
source/pack family and projection owner within its existing budgets.

| Target kind | `passed` | `measure_milli` | `confidence` |
|---|---|---|---|
| `template` | verdict | `floor(score × 1000)` | `floor(score × 1000)` |
| `color` | verdict | `floor(distance × 1000)` | `null` |
| `color_digest` | verdict | `mean_milli` | `null` |
| `composite` | verdict | not allowed | `null` |
| `ocr` | verdict | `ocr_confidence_milli(confidence)` | `ocr_confidence_milli(confidence)` |
| `nn` | verdict | `floor(selected_score × 1000)` | `floor(selected_score × 1000)` |

A `passed` feature is a boolean and a `measure_milli` feature an integer. An OCR confidence goes
through `ocr_confidence_milli`, `floor(clamp(confidence, 0, 1) * 1000)`, the one conversion that
resource readings use as well ([resource-readings.md](resource-readings.md)); every other score
is widened from `f32` to `f64` before it is multiplied. An OCR result without a confidence or an
NN result without a selected score has no measure and no confidence: its `measure_milli` feature
is absent and its `confidence` is `null`; no value is defaulted.

## Select step

A select step is an operation whose effect is one tap on a candidate that a selection policy
chooses, at run time, among the candidates a layout of the step's page projects.

Implementation status: the source parser admits the `select` effect and its policy document,
contained task admission and the interpreter execute it, and the host appends its
`task.selection_evaluated` record. Lab's own package reader (`lab validate`, and the Lab
validation of a task schema `0.6`/`0.7` package that `package dry-run` runs) refuses a select
step with `lab_run_select_unsupported`.

### Declaration

```json
{"id": "choose_slot", "from": "list_page", "to": "detail_page",
 "select": {"layout_id": "layout/main_slots",
            "policy": {"path": "policies/main_slots.json", "sha256": "<64 lowercase hex>"}},
 "guard": {"page_id": "list_page", "target_id": "ui/close",
           "expected_rect": {"x": 1124, "y": 86, "width": 30, "height": 31},
           "verify_template": "assets/close.png"},
 "expect_after": {"page_id": "detail_page", "timeout_ms": 10000, "interval_ms": 500},
 "retryable": false, "max_attempts": 1, "retry_interval_ms": 1, "post_delay_ms": 200}
```

- `select` is the third effect of an operation: exactly one of `click`, `application` and
  `select` is declared. It is accepted for task schema `0.6` through `0.9`; an older schema
  refuses it with `UnconsumedField` at `/operations/<i>/select`.
- `layout_id` names a candidate layout that the same task declares on the step's `from` page.
- `policy` is a selection-policy document ([selection-policy.md](selection-policy.md)) at
  `policies/<name>.json` in the task's directory, sealed by `sha256`, the SHA-256 of its bytes
  (64 lowercase hex digits). The document is part of the package like every task file.
- `guard` is required. It may be any guard the parser and admission accept:
  `verify_template`, `color_probe` (a color or a color digest target) or `check`. The rule
  that a `target`, `target_center` or `offset` click needs a template guard does not apply. The
  guard is judged on the confirmation frame.
- `expect_after` is required, and `unguarded_trusted_coordinate` is refused.

These rules are checked by the declaration gate (`resource validate` and every parse) and
again by contained task admission, which runs the same declaration validation on the sealed
task. Each refusal names its file and the JSON pointer of the offending field:

| Rule | Pointer | File |
| --- | --- | --- |
| `select` holds exactly `layout_id` and `policy`, of the right JSON types. | the field | `task.json` |
| `layout_id` matches `^[a-z0-9][a-z0-9_./-]{0,63}$`. | `/operations/<i>/select/layout_id` | `task.json` |
| `policy` is `{path, sha256}`: a safe task-local `policies/<name>.json` path and 64 lowercase hex digits. | `…/select/policy/path`, `…/select/policy/sha256` | `task.json` |
| No other effect is declared. | `/operations/<i>/select` | `task.json` |
| `guard` and `expect_after` are declared (`MissingField`); `unguarded_trusted_coordinate` is not `true`. | `/operations/<i>/guard`, `…/expect_after`, `…/unguarded_trusted_coordinate` | `task.json` |
| The document's bytes hash to `policy.sha256`. | `/operations/<i>/select/policy/sha256` | `task.json` |
| The task declares the layout, and the layout's page is the step's `from` page. | `/operations/<i>/select/layout_id` | `task.json` |
| The document is at most 512 KiB, decodes as a selection-policy document, has an unambiguous canonical form (no floats, no unsafe integers, no duplicate keys) and validates, in that order. | the document | the policy |
| `applies_to.candidate_layout_id` is the step's `layout_id`. | `/applies_to/candidate_layout_id` | the policy |
| Each `fields[j]` names a feature of the layout and is `boolean` for a `passed` feature, `integer` for a `measure_milli` feature. A layout feature the document does not read is allowed. | `/fields/<j>/name`, `/fields/<j>/value_type` | the policy |
| `selection.required_count` is `1`. | `/selection/required_count` | the policy |

`actinglab resource validate` reports a policy document with the family `selection_policy`.

Contained task admission then checks the step against the loaded package and refuses it before
any input with its own codes:

| Code | When |
| --- | --- |
| `contained_task_select_invalid` | The step lacks its guard or `expect_after`, is a trusted coordinate, or its `select` is malformed; or the layout is not a layout of the recognition pack, or not on the step's `from` page. |
| `contained_task_select_policy_missing` | The document is not in the package. |
| `contained_task_select_policy_hash_mismatch` | The SHA-256 of the document's bytes in the package differs from `policy.sha256`. The bytes are hashed directly, not looked up in a manifest, so a ZIP and a content-directory package are sealed the same way. |
| `contained_task_select_policy_invalid` | The document does not read as a selection-policy document. |
| `contained_task_select_policy_mismatch` | The document does not apply to the layout, as above. |

The sealed binding of a select step is therefore the document path and SHA-256 inside the
package, the package's own identity (its ZIP SHA-256 or content-directory digest) and the
direct byte hash. A per-request policy binding is not part of v1.

### Derived documents

- A select step becomes no navigation edge and no page operation, so no route passes one.
- Its primitive keeps the shape of a click primitive with `click: null` and the declared
  `guard`, and adds the `select` object as declared. The primitives stay at schema `0.6`.
- The sealed task (`canonical_task`) keeps `select` and the declared `guard` (its page ID
  canonicalized as for every guard); no click is inferred.
- The operation index is unchanged.

### Execution

One attempt of a select step runs, before any input:

1. The step's frame F1 (the frame the step was dispatched on, recognized as its `from` page) is
   projected with the step's layout: the projection P1 and its `candidate_set_sha256` H1.
2. The runtime supplies the instance fact snapshot and the evaluation instant. The host reads
   its policy inputs and releases them, takes the instance's snapshot under the fact write gate
   (`instance_fact_snapshot`) and samples its clock; it holds no policy lock. The snapshot's
   context is the instance alias with the game and server of the instance's configured policy
   identity, or of the package's `control.json` when none is configured.
3. The policy is evaluated over P1's actionable candidates, each feature a field of its name (a
   `passed` feature a boolean, a `measure_milli` feature an integer), with the snapshot's facts
   at that instant.
4. When the outcome is `selected` (one candidate), a confirmation frame F2 is captured and
   recognized like every frame, with its capture and recognition records and the `CapturePage`
   timing boundary, but it feeds neither the stability sampling nor the post-admission OCR
   collector. F2 must be the step's page, the step's guard must pass on F2, and the projection
   P2 of F2 must hash to H1.
5. The decision is recorded once as `task.selection_evaluated` (section Records).
6. On a match, one tap is sampled inside the chosen candidate's `click` rectangle (taken from
   P2, equal to P1's since the hashes are equal) as a `rect` click is sampled, with the step's
   action seed. `task.effect_intent` records it and the input follows, bound to F2's committed
   input frame.
7. The step's `expect_after` is awaited as for every step.

Any outcome other than `selected` (`empty`, `insufficient`, `ambiguous`, `unknown`) is the v1
fallback, an abort: the step fails with `selection_not_selected` and detail
`<kind>:<outcome_key>`. A policy document never names a task. One candidate is chosen per step;
choosing several, fallback choices and data tables are not part of v1.

Every selection failure happens before the input and takes the pre-execution guard path: the
first attempt fails, with neither retry nor recovery, as a failed click guard does. A
task-level retry belongs to the scheduler. A select step adds one capture and one recognition
to its step, within the task's timeout.

| Failure | Code | Record |
| --- | --- | --- |
| F1 cannot be projected | `candidate_projection_budget_exceeded`, `candidate_feature_failed`, `candidate_feature_provider_missing`, `invalid_candidate_projection` | none |
| The runtime has no snapshot or instant, or an invalid one | `selection_state_unavailable` | none |
| The snapshot's game or server is not the package's | `selection_fact_context_mismatch` | none |
| The evaluator refuses its inputs, or the decision cannot be mirrored into the record | `selection_evaluation_failed` | none |
| The outcome is not `selected` | `selection_not_selected` | `not_attempted` |
| F2 is not the step's page | `selection_page_changed` | `page_changed` |
| The guard fails on F2 | the guard's own code | `guard_failed` with that code |
| F2 cannot be captured, validated or recognized, or projected | that failure's code | `capture_failed` with that code |
| The runtime's capture of F2 fails, nonfatally or fatally | the runtime's own error, unchanged | `capture_failed` with `selection_confirmation_capture_failed` |
| P2 hashes differently | `selection_projection_mismatch` | `mismatched` |
| The record exceeds 64 KiB or is invalid | `selection_record_too_large`, or the record's own validation code | none |

A failure before the decision writes no `task.selection_evaluated` record, because the record
holds a decision. It is reported where every task failure is: the run's terminal event
(`task.failed`) carries the code, the task diagnostic stream's terminal record carries the code
and its detail, the client's receipt carries the code, and the step's `task.step_started`
precedes it in the ledger. A record that cannot be written (too large or invalid) is reported
the same way, and no input follows. A failure of the runtime's own record path ends the run as
for every record, without a further record.

A fatal failure of the runtime's capture of F2 (on the host, a device capture failure after
`capture.failed`) ends the run, and its decision is still recorded: the record is appended
after `capture.failed` and before the run's terminal events, and the task fails with the
runtime's original error. When that record cannot be written (the task's deadline passed or
it was cancelled or paused during the capture, the record fails its own validation, or its
append fails), the task still fails with the original error and its code, on a manual and on
a scheduled run alike. The host records the refusal when it happens, before the run's terminal
events: one `runtime.failed` lifecycle record linked to the run, with stage
`runtime.lifecycle.selection_record` and the refusal's own code and detail. The runtime is
poisoned when the refusal poisons it. Only when the ledger refuses that record as well does the
host join the refusal to the run's failure as the related failure `selection_record` in its
native detail, and poison the runtime. A capture failure its runtime cannot classify forbids
further records and returns without one.

### Offline

`package dry-run` evaluates a select step with an empty fact snapshot
(`snapshot:offline:empty` at position 1 of no ledger, instance `offline.simulation`, the
package's game and server) at the instant the Lab process read when the dry run started, and
confirms on the same saved frame. The result carries that instant as `selection_now_unix_ms`,
and the decision fingerprint binds it, only when a select step was evaluated; every other
result and fingerprint is unchanged. The kernel's `dry_run_select` evaluates one select step on
one saved frame with given facts and instant, for the `actinglab select` tool of a later slice.

## Records

### `task.selection_evaluated`

A select step records its decision in the task ledger as the event
`task.selection_evaluated` (family `task`), carrying the task fact

```json
{"kind": "selection_evaluated", "step_index": 3, "operation_label": "choose_slot",
 "selection": { … }}
```

Each select attempt that reaches a decision appends exactly one, before any input and before
any failure return (section Select step). The host appends it linked to the step's action and
to the last frame the attempt read. A package without a select step never produces the event.

The record has the shape of a `policy.*` decision: identity hashes and a complete breakdown.

| Field | Meaning |
| --- | --- |
| `layout_id`, `page_id` | The evaluated layout and its page; equal to the projection's. |
| `projection` | The full core candidate projection of the step's frame ([candidate-projection.md](candidate-projection.md)), including its `candidate_set_sha256`. |
| `policy.path` | The package-relative path of the policy document the step declares, `operations/<task>/<declared path>` below the resource root. |
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
