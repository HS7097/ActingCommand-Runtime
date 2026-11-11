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
per-target color `max_distance` or (Workflow #411 B3) color `alternatives` (on a `color`
target or a template's `color_check`), or a candidate layout (section Candidate layouts). Every
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
| Page gating post-admission OCR | Yes (Workflow #396 REC-6: any backend) | Yes |
| OCR field `target_id`, OCR relative anchor, navigation `target_center` | No | No |
| Lab observe, is-visible, probe `ObserveTargets` | Yes, without geometry | Yes, without geometry |
| Lab `wait --stable` | Rejected explicitly | Yes, by its region |

Workflow #396 REC-6: a page may reference a post-admission OCR field or reading target,
directly or as a member of a composite the page references; the gate's observation of the
frame is the field's or reading's. A composite may hold a value test
(`resource-declarations.md`, section Task schema `0.9` value tests), and then is never in
`forbidden`.

A `target` or `offset` click takes its box from a template guard (`guard.verify_template`)
or, from task schema `0.9`, from a located-text guard (`guard.verify_text`, `ocr-fields.md`,
section Located text); a `target_center` click takes a template guard only (Workflow #396
REC-1).

### Relation to pages

A page is `all_of(required)`, each `any_of` group, and `none_of(forbidden)`; these fields
do not change. A page rule references a check by its ID like any other target. The task
record keeps its shape: `task.recognition_completed` records a composite as one recognized
target without a region, and a color digest target with its declared region.

### Selection-graph mapping

- A node's recognition slot names a check ID; checks replace custom recognition.
- A direct hit is a node without a check.
- Inversion belongs to the gate.
- Numeric gates on read values, such as a value at or above a threshold, are value tests
  (Workflow #396 REC-6, `resource-declarations.md`, section Task schema `0.9` value tests).

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
| 1 to 8 features `{name, value}`, with distinct names matching `^[a-z][a-z0-9_]{0,31}$` and a value of `passed`, `measure_milli`, `identity`, `ocr_integer` or (Workflow #404 FS-2, since 0.12.43) `anchor_milli`. A feature may add its own `target` (a string) and `offset` (`{x, y}` integers), both or neither; every `repeated_anchor` feature declares both, except an `anchor_milli` feature, which declares neither (`UnknownField` at the `target` or `offset` it names). Only a `repeated_anchor` layout declares `anchor_milli` (`InvalidValue` at `…/features/j/value` in a `fixed_slots` layout). | `…/features`, `…/features/j/name`, `…/features/j/value`, `…/features/j/target`, `…/features/j/offset` | gate |
| A `fixed_slots` layout has 1 to 64 slots `{rect, click, targets}`. | `/candidate_layouts/i/slots` | gate |
| `rect` and `click` are integer rectangles with a non-negative origin and a positive size, entirely inside the task's `coordinate_space`. | `…/slots/k/rect`, `…/slots/k/click` (a field's type or sign at `…/x` and so on) | gate |
| Each `targets` key is a declared feature that reads no target of its own, and each value a string. | `…/slots/k/targets/<name>` | gate |
| A `repeated_anchor` layout names its `anchor` (a string), `max_instances` 1 to 64, an `order` of `top_to_bottom` or `left_to_right`, and optionally `suppress_iou_milli` 0 to 999. | `/candidate_layouts/i/anchor`, `…/max_instances`, `…/order`, `…/suppress_iou_milli` | gate |
| Its `instance_rect` and `click` are integer rectangles relative to the anchor match: a signed origin and a positive size. Its optional `readable_band` is an integer rectangle with a non-negative origin and a positive size, entirely inside the task's `coordinate_space`. | `/candidate_layouts/i/instance_rect`, `…/click`, `…/readable_band` (a field's type or value at `…/x` and so on) | gate |
| Workflow #396 REC-8: a `fixed_slots` layout may declare `shared_ocr` (section Shared OCR read) from task schema `0.9` on; an older task schema refuses it with `UnconsumedField`, and a `repeated_anchor` layout with `UnknownField`. Its `region` is a `rect` or `full_frame` region in the form of an OCR target's; `languages` are strings; `timeout_ms` is an unsigned integer; `model_ref`, `model_sha256` and `blocks` are strings; the optional `line_merge` is a string or `{"gap_factor": n}` with an unsigned `n`. Workflow #428 CFL-2: the optional `preprocess` is a chain of [recognition-preprocess.md](recognition-preprocess.md), refused at its own pointer below `…/shared_ocr/preprocess`, and a `threshold` of `"otsu"` in it is `InvalidValue` at `…/preprocess/steps/<k>/value` (section Shared OCR read, Preprocessing). | `/candidate_layouts/i/shared_ocr` and its fields | gate |
| The page is a page the declaring task itself declares (its `entry_page`, `target_page`, `error_pages`, `scheduling_outcome` terminal pages, or an operation's `from`, `to` or `expect_after.page_id`). Every build of the task therefore holds it. | `/candidate_layouts/i/page_id` | parser |
| Each slot target is a `template`, `color`, `color_digest`, `composite`, `ocr` or `nn` target of the derived pack; a `measure_milli` feature never reads a composite. Every layout of the selected tasks needs its targets in the same build, as a check does. | `…/slots/k/targets/<name>` | parser |
| A feature's own `target` is a `template`, `color`, `color_digest` or `ocr` target of the derived pack: an `nn` or `composite` feature cannot be read at an offset. | `…/features/j/target` | parser |
| A `repeated_anchor` layout's `anchor` is a `template` target of the derived pack. | `/candidate_layouts/i/anchor` | parser |
| Workflow #404 FS-2: an `anchor_milli` feature derives as `{name, value}`; the parser refuses it in a `fixed_slots` layout and with a `target`, as the gate does (`InvalidValue`). | `…/features/j/value`, `…/features/j/target` | parser |
| The same layout ID with an identical derived definition, repeated by several tasks, is kept once; any other reuse of the ID is refused at the entry that reuses it. Layout IDs have their own namespace, separate from target IDs. | `/candidate_layouts/i/id` | parser |

The derived layout keeps the source's structure, with two normalizations: `page_id` becomes the
full page-set ID `<game>/<page>` that the page set declares and the load-site check compares
exactly (an ID that already holds `/` is kept as written), and the fields of a layout, a
feature, a slot and a rectangle are written in the order of the examples below; a slot's
`targets` keep their declared order. A feature writes `name`, `value`, `target` and `offset`,
then `identity`, `consensus` and `integer` as declared. A layout writes its `shared_ocr` last,
with its region in the pack's form (a rectangle or `full_frame`) and its other members as
declared. Layouts are written in the order the tasks declare them,
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
  `color_check` apply at every position; with `alternatives`, any declared color within the
  check's distance qualifies a position (Workflow #411 B3, `resource-declarations.md`, section
  Task schema `0.9` several expected colors). `order` numbers the matches: `top_to_bottom` by
  (`y`, `x`), `left_to_right` by (`x`, `y`). Matches above `max_instances` fail the
  projection, and a search that runs out of its 5 s limit fails with
  `candidate_search_incomplete` ([candidate-projection.md](candidate-projection.md), section
  Generation).
- Workflow #404 FS-1 (since 0.12.43): every position of the anchor's region, `full_frame`
  included, is scored on the exact score map, never through the downsampled pyramid. The
  anchor may declare a `mask` and `scales` as any template target does
  ([resource-declarations.md](resource-declarations.md), sections Task schema `0.9` template
  masks and multi-scale templates): only the pixels the mask selects are scored, each factor is
  searched on its own map, and the matches of every factor are merged before they are ordered
  and suppressed, so one object found at several factors is one instance. An instance's
  `instance_rect`, `click` and each feature's `offset` scale with the factor its match hit,
  `(|v| × f + 500) div 1000` with the sign of `v`, a width or height at least 1; the target a
  feature reads keeps its own size. A select over a scan refuses an anchor that declares
  `scales` (section Select over a scan).
- Workflow #404 FS-2 (since 0.12.43): a feature `{"name", "value": "anchor_milli"}` reads no
  target: it carries the instance's own anchor match score (section Feature values).
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

### Shared OCR read

Workflow #396 REC-8. A `fixed_slots` layout may declare one OCR detection read that its slots
share:

```json
{"id": "layout/roster", "page_id": "list_page", "kind": "fixed_slots",
 "features": [{"name": "member", "value": "identity", "identity": {}}],
 "slots": [],
 "shared_ocr": {"region": {"x": 40, "y": 120, "width": 1200, "height": 480},
                "languages": ["en"], "timeout_ms": 5000,
                "model_ref": "<model>", "model_sha256": "<64 hex>",
                "blocks": "detect", "line_merge": {"gap_factor": 0}}}
```

- The read runs the detector in its region, a rectangle or `full_frame`, and reads every box on
  its own, with the declared line merge (`blocks` and `line_merge` as for located text,
  [ocr-fields.md](ocr-fields.md), section Located text).
- An OCR target without `locate` that a slot or a feature reads takes its text from the read
  when its `model_ref`, `model_sha256` and `languages` equal the read's and its region lies
  entirely inside the read's region. The region is the one the sample reads: moved to the slot
  origin plus the feature's offset, shifted by the sample's jitter, or placed at its anchor's
  match. The target's text is that of the blocks whose center `(x + floor(w/2), y + floor(h/2))`
  lies in its region, in the sorted order `(y, x, height, width, text)`, joined with `\n`; its
  confidence is the lowest of theirs, and absent without blocks or when one block has none.
  Without blocks the text is empty, and the feature's existing rules make it Unknown. The
  target's verdict and evidence are those of an OCR target that read this text. Every other OCR
  target reads on its own.
- One projection calls the provider once for each frame whose read serves a target; the read's
  PP-OCR reports go with the first target it serves.
- When the detector found more boxes than the model folder's `max_boxes`, the projection fails
  with `candidate_feature_failed`, whose cause is the evaluation error
  `text_read_blocks_truncated` (stage `text_read_shared`, `count` boxes detected, `limit_count`
  the folder's `max_boxes`): a slot may lack its blocks.
- The read counts as one provider evaluation for each frame it serves, and a target it serves
  counts none ([candidate-projection.md](candidate-projection.md), section Budgets). Admission
  knows a target's region before the frame unless it is `template_relative`; such a target
  keeps its own evaluation in the count, and the frame may still serve it.

#### Preprocessing (Workflow #428 CFL-2, since 0.12.29)

The read may declare `preprocess`, the chain of
[recognition-preprocess.md](recognition-preprocess.md), with the white-text example:

```json
"shared_ocr": {"region": {"x": 40, "y": 120, "width": 1200, "height": 480},
               "languages": ["en"], "timeout_ms": 5000,
               "model_ref": "<model>", "model_sha256": "<64 hex>",
               "blocks": "detect", "line_merge": {"gap_factor": 0},
               "preprocess": {"steps": [
                 {"kind": "keep_colors", "colors": [[255, 255, 255]], "tolerance": 40},
                 {"kind": "invert"}
               ]}}
```

- **Pixel by pixel only.** A shared read's chain takes no `threshold` of `"otsu"`: Otsu's
  threshold reads the whole region, so a target's part of the shared result would differ from
  what the target gives on its own. Admission refuses it with `resource_declaration_invalid`,
  `InvalidValue`, at `/candidate_layouts/<i>/shared_ocr/preprocess/steps/<k>/value` (the
  source's own pointer in `task.json`). Every other step works pixel by pixel.
- **Which targets it serves.** Besides the rules above, a target takes its text from the read
  only when its own `preprocess` equals the read's, step by step and with the colors of
  `keep_colors` in their declared order; neither declaring one is equal too. A target with
  another chain, an `otsu` one among them, reads on its own. The admission rule "at least one
  target takes its text from the read" counts with chains.
- **The read.** The chain runs once over the read's region before the provider call, the
  provider reads the processed region ([recognition-preprocess.md](recognition-preprocess.md),
  "Hand-over") and the processed region is kept for the projection. After the chain the sample
  deadline is checked again, as before the read.
- **Evidence.** Every evaluation the read serves carries `shared_read`: the read's `rect`, the
  detector's `detected` and `limit` when the provider gives them, and with a chain the read's
  own `kept_milli` and `threshold_value`. With a chain its `preprocess` evidence has `scope`
  `shared`, `rect` the target's own region and `source_rect` the read's; its `kept_milli` is
  counted over the target's own region of the kept processed region, without running the chain
  again, and equals what the target would report on its own. Both are read-only and Lab
  output; the ledger is unchanged.
- **Truncation** still fails the projection with `text_read_blocks_truncated`, with a chain as
  without one.
- **The authoring warning.** `package bundle` and `actinglab standard-pack check` report one
  `package_bundle_preprocess_divergent` warning (`package_id`, `target_id`, `json_pointer` of
  the read in the recognition pack) for each target that lies in the read with its model and
  languages but declares another chain, so that it reads on its own. Nothing is refused.

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
| A `repeated_anchor` anchor is a raw `template` target with a static search region; a `color_check`, when it declares one, is `template_relative`. Since 0.12.43 (Workflow #404 FS-1) it may declare `mask` and `scales`. | `/candidate_layouts/i/anchor` |
| `max_instances` is 1 to 64, `order` is declared, `suppress_iou_milli` is 0 to 999, `instance_rect` and `click` have a positive size, and `readable_band` has a non-negative origin and a positive size and lies inside `coordinate_space`. | `…/max_instances`, `…/order`, `…/suppress_iou_milli`, `…/instance_rect`, `…/click`, `…/readable_band` |
| Every `repeated_anchor` feature reads a target at an offset, except an `anchor_milli` feature (Workflow #404 FS-2), which reads none. Its consensus samples one frame only, so its samples may differ in jitter and template metric but never in frame: a scan cannot capture further frames. | `…/features/j`, `…/features/j/consensus` |
| Workflow #404 FS-2: only a `repeated_anchor` layout declares an `anchor_milli` feature, and it declares no `target` or `offset`; it takes no `identity`, `integer` or `consensus` either. | `…/features/j/value`, `…/features/j` |
| A feature's `target` and `offset` appear together. The target exists, is raw, and is a `template`, `color`, `color_digest` or `ocr` target with a rectangle region; a template's `color_check` is `template_relative`; an identity or `ocr_integer` feature reads OCR, and an icon identity reads its template pool instead. | `…/features/j`, `…/features/j/target`, `…/features/j/identity` |
| The region a feature reads at an offset, for each of its samples, lies inside the `instance_rect`, or inside each slot's `rect`; a slot's `targets` never map such a feature. | `…/features/j/offset`, `…/slots/k`, `…/slots/k/targets/<name>` |
| `rect` and `click` have a non-negative origin and a positive size and lie entirely inside `coordinate_space`. | `…/slots/k/rect`, `…/slots/k/click` |
| Each `targets` key is a declared feature; its target exists and is a `template`, `color`, `color_digest`, `composite`, `ocr` or `nn` target, never `click_only`; a `measure_milli` feature never reads a composite. | `…/slots/k/targets/<name>` |
| One projection needs at most 16 OCR and NN evaluations: one per distinct OCR or NN target the slots read, and one per OCR or NN member of each distinct composite they read; a shared OCR read counts one for each frame it serves and the targets it serves none (section Shared OCR read). One instance of a `repeated_anchor` layout needs at most 16; the producer checks the product with the actionable instances on each frame. | `/candidate_layouts/i/slots`, `…/features` |
| Workflow #396 REC-8: only a `fixed_slots` layout declares `shared_ocr`. Its region is a rectangle with a non-negative origin and a positive size inside `coordinate_space`, or `full_frame`; `blocks` is `detect` with a `line_merge` whose `gap_factor` is 0 to 16; `timeout_ms` is 1 to 60000; and at least one OCR target of its slots or features takes its text from it. | `/candidate_layouts/i/shared_ocr`, `…/shared_ocr/region`, `…/shared_ocr/blocks`, `…/shared_ocr/line_merge`, `…/shared_ocr/timeout_ms` |

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

Workflow #404 FS-2 (since 0.12.43): an `anchor_milli` feature of a `repeated_anchor` layout
reads no target. It is an integer, `floor(score × 1000)` of the instance's own anchor match
(the normalized score of the position and factor the enumeration took, widened to `f64`), and
its `confidence` is the same value. Like every feature it is carried only by an actionable
instance. A policy reads it as `integer`, in the domain 0..=1000.

## Select step

A select step is an operation whose effect is one tap on a candidate that a selection policy
chooses, at run time, among the candidates a layout of the step's page projects.

Implementation status: the source parser admits the `select` effect and its policy document,
contained task admission and the interpreter execute it, and the host appends its
`task.selection_evaluated` record. Lab's own package reader (`lab validate`, and the Lab
validation of a task schema `0.6`/`0.7` package that `package dry-run` runs) refuses a select
step with `lab_run_select_unsupported`. Workflow #308 SEL-L4b (frozen selector model S0 v2.1
§1.2–§1.4, §1.7, §2, §3) lets a select step declare `scan`, a select over a list read across
viewports (section Select over a scan), and `on_empty`, a normal end of the run when nobody
listed is present (section Ending empty), and lets the `expect_after` of any operation declare
`expect_identity` (section Expected identity).

### Declaration

```json
{"id": "choose_slot", "from": "list_page", "to": "detail_page",
 "select": {"layout_id": "layout/main_slots",
            "policy": {"path": "policies/main_slots.json"}},
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
  `policies/<name>.json` in the task's directory. Workflow #336 PKG-S2 (option 丙): a `sha256`
  the policy may still declare is optional and ignored, whatever its value: it is neither
  validated nor compared, and no record carries it (PKG-S1b refuses the key again once the
  resource repositories no longer carry it). The document is part of the package like every
  task file.
- `guard` is required. It may be any guard the parser and admission accept:
  `verify_template`, `color_probe` (a color or a color digest target) or `check`. The rule
  that a `target`, `target_center` or `offset` click needs a template guard does not apply. The
  guard is judged on the confirmation frame.
- `expect_after` is required, and `unguarded_trusted_coordinate` is refused.
- `select` may also declare `scan` (below), which makes the step a select over a scan; a step
  without it is a single-frame select.
- `select` may also declare `on_empty: {"outcome_key": "<key>"}` (section Ending empty). A
  single-frame select may declare it only when its layout is `fixed_slots`: one frame of a
  `repeated_anchor` list cannot show that the list ends, so a list that fits one screen declares
  `scan`, whose start and end checks then both pass on its first viewport.
- A single-frame select may also declare `confirm` (Workflow #411 B4, register §1.166):
  `"candidate_set"`, the default, compares the candidate sets of the two frames (section
  Execution); `"decision"` compares their decisions (section Confirming by the decision). A
  select over a scan confirms the chosen identity, and admission refuses it with
  `confirm: "decision"` (`contained_task_select_invalid`, detail `confirm=decision
  scan=declared`). The source gate refuses a `confirm` that is no string (`InvalidType`) or
  another string (`InvalidValue`) at `/operations/<i>/select/confirm`; `null` is absent. The
  member belongs to `package` revision 6.
- Workflow #404 FS-3 (since 0.12.43, register §1.284): a single-frame select over a
  `repeated_anchor` layout may declare `confirm: "track"` with `track_px`, an integer
  1..=64: the choice is found again on the confirmation frame within `track_px` pixels and
  tapped where that frame shows it (section Confirming by tracking). Admission refuses either
  member anywhere else with `contained_task_select_invalid`, its detail naming the rule:
  `track_px` without `track` (`track_px=declared confirm=not_track`), `track` without
  `track_px` (`confirm=track track_px=absent`), a `track_px` outside 1..=64
  (`confirm=track track_px=<n> range=1..=64`), `track` beside `scan`
  (`confirm=track scan=declared`) and `track` on a `fixed_slots` layout
  (`confirm=track layout=not_repeated_anchor`). The source gate takes the value `track` and
  refuses a `track_px` that is no unsigned integer (`InvalidType` at
  `/operations/<i>/select/track_px`); `null` is absent. Both belong to `package` revision 7.
- The `expect_after` of any operation may add `expect_identity: {"target_id", "choice_of"}`,
  or from SEL-L4b-T `expect_identity: {"choice_of", "templates": [{"id", "target_id"}]}`
  (section Expected identity).

A scan is declared with every member but `early_stop`; the Runtime supplies no default. The
numbers below are neutral examples, not values:

```json
"scan": {"identity_feature": "member", "axis": "vertical",
  "advance": {"kind": "drag", "from": {"x": 560, "y": 600, "width": 160, "height": 30},
              "to": {"x": 560, "y": 400, "width": 160, "height": 30}, "duration_ms": 1000,
              "min_content_step_px": 170, "max_content_step_px": 240},
  "reposition": {"kind": "drag", "from": {"x": 560, "y": 640, "width": 160, "height": 30},
                 "to": {"x": 560, "y": 260, "width": 160, "height": 30}, "duration_ms": 600,
                 "max_content_step_px": 460},
  "settle": {"interval_ms": 200, "timeout_ms": 3000},
  "track": {"area": {"x": 0, "y": 140, "width": 1280, "height": 560}, "edge_margin_px": 12,
            "signature": {"rect": {"x": 0, "y": 0, "width": 160, "height": 160},
                          "columns": 8, "rows": 8},
            "signature_max_distance_milli": 3000, "position_tolerance_px": 6,
            "min_overlap_instances": 3,
            "spacing": {"max_axis_gap_px": 300, "columns": 6, "max_trailing_gap_px": 320,
                        "max_cross_lead_px": 40, "max_cross_gap_px": 210}},
  "start": {"check": "check/list_start", "rewind": true},
  "end": {"check": "check/list_end"},
  "bounds": {"max_viewports": 80, "max_reposition_viewports": 40, "max_ms": 240000},
  "early_stop": "upper_bound"}
```

- `identity_feature` names the layout's identity feature that identifies a position; `axis` is
  `vertical` or `horizontal`.
- `advance` is the move to the next viewport, with `min_content_step_px` and
  `max_content_step_px`, the smallest and largest content displacement one move gives (measured,
  not the finger's displacement); a corrective move is `advance` reversed. `reposition` is the
  longer move, with `max_content_step_px`, used reversed to rewind and in either direction to
  return. A drag is sampled between random points of its `from` and `to` rectangles; the
  further forms of both moves are SEL-L3b's (register §1.58, L3B-3).
- `settle` paces the fresh frames after a move; `track` says where instances are tracked
  (`area`, `edge_margin_px`), how they are told apart (`signature`, compared within
  `signature_max_distance_milli`), how far positions may differ and still be one
  (`position_tolerance_px`), how many must overlap between viewports (`min_overlap_instances`)
  and how they are spaced (`spacing`).
- `start` is `{check, rewind}` and `end` is `{check}`: checks that give positive evidence of the
  list's two ends; `rewind: true` lets the scan move toward the start until its check passes.
- `bounds` caps the coverage viewports (`max_viewports`), the rewind and return viewports
  together (`max_reposition_viewports`) and the scan's time (`max_ms`, section Select over a
  scan). `early_stop: "upper_bound"` lets the scan stop before the list's end once no position
  not yet read can outrank the best member.

These rules are checked by the declaration gate (`resource validate` and every parse) and
again by contained task admission, which runs the same declaration validation on the sealed
task. Each refusal names its file and the JSON pointer of the offending field:

| Rule | Pointer | File |
| --- | --- | --- |
| `select` holds `layout_id` and `policy`, and optionally `scan` and `on_empty`, of the right JSON types. | the field | `task.json` |
| `scan` holds `identity_feature`, `axis`, `advance`, `reposition`, `settle`, `track`, `start`, `end` and `bounds` (`MissingField`), and optionally `early_stop`; any other member is `UnknownField`. Admission reads their values. | `…/select/scan/<member>` | `task.json` |
| `on_empty` is `{outcome_key}`, a string. | `…/select/on_empty/outcome_key` | `task.json` |
| `expect_after.expect_identity` holds `choice_of` and optionally `target_id`, both strings, and optionally `templates`, an array of `{id, target_id}`, both strings (SEL-L4b-T); any other member is `UnknownField`. Admission requires exactly one of `target_id` and `templates`. | `/operations/<i>/expect_after/expect_identity/<member>`, `…/expect_identity/templates/<j>/<member>` | `task.json` |
| `layout_id` matches `^[a-z0-9][a-z0-9_./-]{0,63}$`. | `/operations/<i>/select/layout_id` | `task.json` |
| `policy` is `{path}`: a safe task-local `policies/<name>.json` path (an optional `sha256` is ignored). | `…/select/policy/path` | `task.json` |
| No other effect is declared. | `/operations/<i>/select` | `task.json` |
| `guard` and `expect_after` are declared (`MissingField`); `unguarded_trusted_coordinate` is not `true`. | `/operations/<i>/guard`, `…/expect_after`, `…/unguarded_trusted_coordinate` | `task.json` |
| The task declares the layout, and the layout's page is the step's `from` page. | `/operations/<i>/select/layout_id` | `task.json` |
| The document is at most 512 KiB, decodes as a selection-policy document, has an unambiguous canonical form (no floats, no unsafe integers, no duplicate keys) and validates, in that order. | the document | the policy |
| Each `gate_id` and `term_id` is a token (lowercase ASCII letters, digits, `_`, `.`, `:`, `-`), the form of the `rule_id` a selection reason carries (`InvalidValue`). | `/gates/<j>/gate_id`, `/scoring/<k>/term_id` | the policy |
| `applies_to.candidate_layout_id` is the step's `layout_id`. | `/applies_to/candidate_layout_id` | the policy |
| Each `fields[j]` names a feature of the layout and is `boolean` for a `passed` feature, `integer` for a `measure_milli`, `ocr_integer` or (Workflow #404 FS-2) `anchor_milli` feature; an `anchor_milli` field only on a `repeated_anchor` layout, the one kind that carries it (the gate takes it since FS-2b, register §1.285; on any other layout it is a field typed unlike its feature). A layout feature the document does not read is allowed. | `/fields/<j>/name`, `/fields/<j>/value_type` | the policy |
| `selection.required_count` is `1`. | `/selection/required_count` | the policy |

A select step whose policy declares more than one candidate is a multi select (Workflow #357,
frozen model M-357b v1.0); it takes effect from slice DB-3b, and until then admission refuses
it as above. Its record shape is already part of ledger revision 3 (section Records,
`multi_select`).

`actinglab resource validate` reports a policy document with the family `selection_policy`.

Contained task admission then checks the step against the loaded package and refuses it before
any input with its own codes:

| Code | When |
| --- | --- |
| `contained_task_select_invalid` | The step lacks its guard or `expect_after`, is a trusted coordinate, or its `select` is malformed; or the layout is not a layout of the recognition pack, or not on the step's `from` page. |
| `contained_task_select_policy_missing` | The document is not in the package. |
| `contained_task_select_policy_invalid` | The document does not read as a selection-policy document. |
| `contained_task_select_policy_mismatch` | The document does not apply to the layout, as above. |

The binding of a select step is therefore the document path inside the package the run
admitted; since Workflow #336 PKG-S1 neither the document's declared SHA-256 nor a package
digest is compared. A per-request policy binding is not part of v1.

Workflow #308 SEL-L4b adds these admission checks, each refused with
`contained_task_select_invalid` and a detail naming the rule. A `scan` passes the static checks
of S0 §2.1, where *b* is the readable band's extent on the axis and *e* the `instance_rect`
extent on the axis:

- the layout is `repeated_anchor` with an `instance_rect`, `identity_feature` names one of its
  identity features, its `unknown_identity` is `reject`, and its consensus samples one frame
  only;
- Workflow #404 FS-1 (since 0.12.43): its anchor declares no `scales` (detail `scan
  anchor=scaled`): a scan tracks its instances and their signatures at the declared offsets
  from each match, unscaled; an anchor's `mask` is taken;
- the policy's last tie-break key is `candidate_id lowest_first`, so that rows with the same
  business ID never end `ambiguous` and the first one seen wins;
- `signature` has 1–8 columns and 1–8 rows, and its rectangle lies inside `instance_rect`;
- `settle.interval_ms` is 1..=5000, and `settle.timeout_ms` is at least the interval and at most
  `bounds.max_ms`;
- `max_viewports` is 2..=128, `max_reposition_viewports` 1..=128, and `max_ms` at least 1 and at
  most the task's `timeout_ms`;
- `min_overlap_instances`, `spacing.columns` and every `spacing` distance are at least 1;
- the readable band lies inside `track.area`, which lies inside the frame; `edge_margin_px` is at
  least `position_tolerance_px`; and the core, the area less the margin at both of its axis
  edges, is at least *e* long;
- the largest finger displacement a drag's two rectangles allow is at most its
  `max_content_step_px`; 1 ≤ `advance.min_content_step_px` ≤ `advance.max_content_step_px` ≤
  *b* − *e*; and 1 ≤ `reposition.max_content_step_px` ≤ *b*;
- `start.check`, `end.check` and every page check of a page flip are targets of the pack.

For a step that declares `scan` or `on_empty`, every field the policy reads has a domain the
layout gives it: a `passed` feature boolean, a `measure_milli` or (Workflow #404 FS-2)
`anchor_milli` feature 0..=1000, an `ocr_integer` feature its declared range, and the step's
identity feature its entries. An
identity field other than the step's identity feature is refused, because one hypothesis fixes
one identity.

A step that declares `on_empty` is admitted only when its policy's `selection.mode` is
`none_allowed` (`exactly_one` gives `insufficient` on zero survivors), its task declares
`scheduling_outcome`, a single-frame select's layout is `fixed_slots`, the task's
post-admission OCR is no comparison (a run would otherwise have two sources for its key), and,
when the task declares `post_admission_fields`, their `page_ids` include the step's `from` page.
The step's state is also a terminal of the task's route: exactly one mapping with the effect
condition of that state lists the step's page, and its key is the `on_empty` key. A task without
phases checks this in its scheduling-outcome coverage check
(`contained_task_outcome_declaration_incomplete`), a phased task in its route check
(`task_phases_outcome_incomplete`; [README.md](README.md), phases). A package with an `on_empty`
step serves neither as a prerequisite package (`contained_task_prerequisite_incompatible`,
detail `reason=select_on_empty`) nor for return-home use, which reads the same reason, because
an early end on the step's page would leave the dependent package short of its entry page.

An `expect_identity` is admitted only when `choice_of` names a select step of the same task that
is the declaring step or precedes it (in phase order, or in operation order without phases), that
step has an identity feature (`choice_of=no_identity` otherwise), the declaring operation is not
retryable (a retry would tap again after a mismatch), and exactly one of `target_id` and
`templates` is declared (`expect_identity=target_and_templates`, `=neither`). The text path
(`target_id`) needs a step whose identity feature is recognized by `ocr_aliases` (the read text
is mapped through it; `choice_of=identity_not_text`) and an OCR target of the pack
(`target=not_ocr`). The template path (Workflow #308 SEL-L4b-T, `templates`) is admitted for an
identity of either recognition kind; its pool holds 1..=16 entries (`templates=count`), and each
entry names a template target of the pack (`template=not_template`) that no other entry names
(`template=duplicate`), an `id` that is an entry ID of the step's identity declaration
(`template=out_of_domain`), a target whose effective threshold is above 0
(`template=threshold_zero`) and whose region is a `rect` (`template=region_not_rect`), the same
rect for every entry (`template=regions_differ`). Each refusal is `contained_task_select_invalid`
with the detail token shown.

### Derived documents

- A select step becomes no navigation edge and no page operation, so no route passes one.
- Its primitive keeps the shape of a click primitive with `click: null` and the declared
  `guard`, and adds the `select` object as declared. The primitives stay at schema `0.6`.
- The sealed task (`canonical_task`) keeps `select` and the declared `guard` (its page ID
  canonicalized as for every guard); no click is inferred.
- The operation index is unchanged.

### Tables

Workflow #357 DB-2 (Runtime 0.12.37; [selection-policy.md](selection-policy.md), "Tables and `table`";
register §1.244). **Admission reads the document, never a table**: a document that declares one needs
a layout whose `unknown_identity` is `reject`, and each `table` read's `key_field` is the layout's
identity field (`contained_task_select_policy_mismatch`; the declaration gate refuses the same with
`resource_declaration_invalid`). So admission depends only on the package and the kernel, and a
suspension or its lift never meets a table.

**The run's preparation reads them.** For each package of the run that declares a table, its game's
root is read (a broken root refuses with the root's own code), each table once, each document bound.
Any failure refuses this one run, before any lease and any task record: the receipt is `denied` with
`runtime_code` `package_invalid` and one of `standard_pack_table_missing` (no root, no game root, no
file), `standard_pack_table_unreadable`, `standard_pack_table_invalid`,
`standard_pack_table_mismatch`, with its `table_mismatch`. Nothing else stops: not the root, not the
Runtime, not another instance. A refusal in preparation is not a failure of the task and never
suspends it. A layer the run admits later (a shared interrupt's handler, `then.run`, a navigation
package, an entry recovery package) binds its tables at its admission, and a table failure ends that
run with the same code.

**Fixed for the run.** `ContainedTaskRuntime::standard_pack_table` answers from what was bound and
reads no file, so a single frame, each viewport of a scan and a confirmation by decision read one
table; a file replaced in place is the next run's. A runtime without tables answers none, and the step
fails `selection_evaluation_failed`. The limits are the format's (`tables/` in
[package-reference.md](package-reference.md)), not configuration keys.

**Dropped by the table.** A static table drop is no unknown instance. In a single-frame step it is
left out of `unknown_check`. In a scan it is listed in `candidates[]`, numbered in first-seen order
with the members, with its `unknown_dropped` verdict; it has no score, never counts toward *S*, is
never chosen and is not in `unknown[]`, and it counts toward the 64 candidates. Nobody left to rank,
`exactly_one` is `insufficient` (`selection_not_selected`), and `none_allowed` with `on_empty` is
`empty`. A candidate not of the class is gated out. A pinned fact unavailable stays an unknown
instance, with rules 2 and 3 of the scan. `task.selection_evaluated` gains no member.

**What a run's records add.** Tables are outside every content hash, so a run that binds a table
says which file it read: one more `runtime.lifecycle_observed` record of phase `standard_pack_read`
per table per run, `standard_pack_table_read` (info), with `game`, `path` and `actual_sha256`, the
digest of the file as it was read. It is written once, on the success path, in the run's preparation,
or at the admission of a layer the run admits later; a failed bind keeps its refusal and writes none,
and a run that reads no table writes none. Like every `standard_pack_read` record it has system
links, so a reader finds it by `game` and `path` at or before the run's start. That digest, with the
game root's release, is how a reader locates the table a selection record's `input_sha256` read.

### Execution

One attempt of a single-frame select runs, before any input:

1. The step's frame F1 (the frame the step was dispatched on, recognized as its `from` page) is
   projected with the step's layout: the projection P1 and its `candidate_set_sha256` H1.
2. The runtime supplies the instance fact snapshot and the evaluation instant. The host reads
   its policy inputs and releases them, takes the instance's snapshot under the fact write gate
   (`instance_fact_snapshot`) and samples its clock; it holds no policy lock. The snapshot's
   context is the instance alias with the game and server of the instance's configured policy
   identity, or of the package's `control.json` when none is configured.
3. The policy is evaluated over P1's actionable candidates, each feature a field of its name (a
   `passed` feature a boolean, a `measure_milli`, `ocr_integer` or `anchor_milli` feature an
   integer), with the
   snapshot's facts at that instant.
4. When the outcome is `selected` (one candidate), and, for a step that declares `on_empty`, no
   unknown instance blocks it (rules 2 and 3, section Select over a scan), a confirmation frame
   F2 is captured and recognized like every frame, with its capture and recognition records and
   the `CapturePage` timing boundary, but it feeds neither the stability sampling nor the
   post-admission OCR collector. F2 must be the step's page, the step's guard must pass on F2,
   and the projection P2 of F2 must hash to H1 (a step that declares `confirm: "decision"`
   compares the decisions instead, section Confirming by the decision; one that declares
   `confirm: "track"` finds its choice again on P2, section Confirming by tracking).
5. The decision is recorded once as `task.selection_evaluated` (section Records). A step that
   declares `on_empty` adds `unknown_check` to its v1 record.
6. On a match, one tap is sampled inside the chosen candidate's `click` rectangle (taken from
   P2, equal to P1's since the hashes are equal; for `confirm: "track"`, the click of the
   candidate found again on P2) as a `rect` click is sampled, with the step's action seed.
   `task.effect_intent` records it and the input follows, bound to F2's committed input
   frame.
7. The step's `expect_after` is awaited as for every step.

Any outcome other than `selected` (`empty`, `insufficient`, `ambiguous`, `unknown`) is the v1
fallback, an abort: the step fails with `selection_not_selected` and detail
`<kind>:<outcome_key>`. A policy document never names a task. One candidate is chosen per step;
choosing several, fallback choices and data tables are not part of v1. A step that declares
`on_empty` (Workflow #308 SEL-L4b) differs in two ways: an `empty` decision with no unknown
instance ends the run normally (section Ending empty), and a decision that unknown instances
could change fails `selection_result_blocked` (rules 2 and 3, section Select over a scan). Its
unknown instances are the actionable candidates the kernel keeps from the evaluator (an
unresolved consensus feature, an identity feature that read no identity) and those the
evaluator drops as unknown; its confirmation is unchanged (H1 = H2).

Every failure of a single-frame select happens before the input and takes the pre-execution
guard path: the first attempt fails, with neither retry nor recovery, as a failed click guard
does. A task-level retry belongs to the scheduler. A single-frame select adds one capture and
one recognition to its step, within the task's timeout. A select over a scan moves the list
before its tap, so its failures follow other rules (section Select over a scan).

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
| `confirm: "decision"`: F2 cannot be decided, or its decision hash cannot be formed | that failure's code (`selection_evaluation_failed`) | `capture_failed` with that code |
| `confirm: "decision"`: F2's decision hashes differently | `selection_decision_mismatch` | `decision_mismatched` |
| `confirm: "track"`: F2 cannot be decided, its decision hash cannot be formed, or its choice has no anchor match origin | that failure's code (`selection_evaluation_failed`) | `capture_failed` with that code |
| `confirm: "track"`: F2's decision selects nobody, or a candidate farther than `track_px` | `selection_decision_mismatch`, values `confirm` `track` and `selection_kind` (nobody) or `shift_px` (too far) | `track_lost` |
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
`runtime_lifecycle_selection_record` and the refusal's own code and detail. The runtime is
poisoned when the refusal poisons it. Only when the ledger refuses that record as well does the
host join the refusal to the run's failure as the related failure `selection_record` in its
native detail, and poison the runtime. A capture failure its runtime cannot classify forbids
further records and returns without one.

### Confirming by the decision

Workflow #411 B4 (B-train plan §2.4, D3; register §1.166). A row's reading may change between
F1 and F2 without changing what the step would choose, for example a counter on a row the
gates reject; its candidate set then hashes differently, and the default comparison taps
nothing. A single-frame select that declares `confirm: "decision"` compares only the decisions:

1. Steps 1 to 3 and F2's capture, page and guard checks and projection P2 run as in section
   Execution.
2. P2 is decided again by the step's policy with the same instance fact snapshot and the same
   instant as P1, over P2's actionable candidates as in step 3.
3. The decision hash of each decision is formed over its own projection: the SHA-256, as 64
   lowercase hexadecimal digits, of the compact JSON

   ```text
   {"candidates":[{"id","click":{"x","y","width","height"},"status"}],"selected":[…],"outcome_key"}
   ```

   in exactly this member order: the projection's actionable candidates in projection order,
   each with its click rectangle and its verdict's `status` (`ranked`, `gate_rejected` or
   `unknown_dropped`; `null` for an actionable candidate the kernel keeps from the evaluator,
   an unresolved consensus feature or an identity feature that read no identity), the
   decision's `selected` IDs in its order and its `outcome_key`. No feature value, score, rank,
   gate result or reason enters it. It is the step's own comparison hash, never a package
   digest; the writer forms it and no reader recomputes it.
4. Equal hashes record `decision_matched` (section Records) and the tap of step 6 follows,
   inside the chosen candidate's click rectangle on P2, bound to F2. Different hashes record
   `decision_mismatched` and the step fails `selection_decision_mismatch` before any input,
   as every other failure of a single-frame select does. A failure of F2's decision, or of a
   hash, records `capture_failed` with that failure's code, as a projection failure of F2 does.

The decision of record is P1's in either case; the record keeps F2's candidate-set hash and
F2's decision hash in its confirmation, and P1's decision hash equals the latter exactly when the
confirmation is `decision_matched`. A step without `confirm`, or with `"candidate_set"`,
compares and records as before.

### Confirming by tracking

Workflow #404 FS-3 (since 0.12.43; register §1.284; ledger revision 7). A target that moves
between F1 and F2, such as a marker on a figure that walks around, never confirms by the
candidate set or by the decision, because both hashes cover the click rectangles. A
single-frame select over a `repeated_anchor` layout that declares `confirm: "track"` with
`track_px` finds its choice again on F2 instead:

1. Steps 1 to 3 run on F1 as in section Execution: P1 is projected and decided, and its one
   selected candidate is c1. F2's capture, page and guard checks and projection P2 run as
   there too.
2. P2 is decided again by the step's policy with the same instance fact snapshot and the same
   instant as P1, over P2's actionable candidates; its decision hash is formed over P2 as in
   section Confirming by the decision.
3. When that decision is `selected` with one candidate c2, the shift is the Chebyshev distance
   in pixels, `max(|x1 − x2|, |y1 − y2|)`, between the anchor match origins of c1 and c2: the
   top-left corner of the anchor match each instance was placed at (candidate-projection.md,
   Generation step 3), on F1 and on F2. The origins are the producer's and enter no hash and no
   record.
4. A shift of at most `track_px` records `tracked` (section Records) and the tap of step 6
   follows inside **c2's** `click` rectangle on P2, bound to F2. c2's ID may differ from c1's:
   the numbering follows the declared order on each frame.
5. Otherwise (F2's decision selects nobody, or the shift is above `track_px`) the step records
   `track_lost` and fails `selection_decision_mismatch` before any input, as every other
   failure of a single-frame select does. Its values are `confirm` `track` with `shift_px`, the shift, or,
   when F2's decision selected nobody, `selection_kind`, its kind. A failure of F2's decision,
   of its hash or of an origin records `capture_failed` with that failure's code, as a
   projection failure of F2 does.

The decision of record is P1's: `selected` keeps c1. F2's choice is identified by its decision
hash, which covers F2's selected ID. Only one frame pair is compared (F1 and F2); no target is
followed across more frames, and the step taps once. `track_px` is part of the package, not a
configuration key: how far a target may move between two frames is a property of what the
package shows.

### Select over a scan

Workflow #308 SEL-L4b (S0 §1.3, §2.2, §3.2). A select step that declares `scan` reads its list
across viewports with the scan session of SEL-L3 and SEL-L3b (register §1.49, §1.58), then
decides, returns to the chosen member and confirms it. One attempt runs:

1. **Pin.** One instance fact snapshot and one evaluation instant serve the whole attempt, and
   every record list the policy declares is resolved. The step's dispatched frame F1 is settled
   before it becomes the first viewport.
2. **Scan.** The start check (with a rewind when declared), then the coverage viewports: each
   `advance`, the settle, the page and the guard, the alignment against the previous viewport,
   the gap and spacing checks, the reading of every position and the evaluation of the
   positions the viewport fixed, until the end check passes (coverage `complete`) or the early
   stop holds (`early_stopped`).
3. **Decide.** A position that was tracked but never readable fails
   `selection_scan_position_unread`, and the spacing of the whole list is checked. The members,
   the positions whose fixed reading the evaluator ranked, are numbered `<layout>#NN` in
   first-seen order: by the viewport that fixed them, then by axis start, then by cross-axis
   start. The policy is evaluated once over them, with the pinned snapshot and instant, and rules
   1–3 below decide whether the decision stands.
4. **Return.** Let [*L*, *R*] be the content offsets at which the chosen member's content
   rectangle lies inside the readable band, an interval *b* − *e* wide. When the latest offset
   `last` lies in it, `Confirm` follows at once. Otherwise let *D* be the distance from `last` to
   the nearer end of the interval and *T* = max(`advance.max_content_step_px`,
   `reposition.max_content_step_px` − (*b* − *e*)). The scan moves toward the interval with one
   `reposition` while *D* > *T*, and with one `advance` (reversed toward the start) once
   *D* ≤ *T*, so no move passes the interval. Each settled return viewport passes the page and
   the guard, is aligned directly against the content map (every tracked coverage position,
   never a chain of return viewports) within its move's window, and sets `last`. Every return
   viewport counts toward `max_reposition_viewports`, together with the rewind viewports; past it
   the scan fails `selection_scan_bounds_exceeded` (`scan_bound: reposition_viewports`). Return
   failures are reported at the stage `select_return`.
5. **Confirm.** A fresh frame F2, settled as after an input. In this order: its capture and
   settle; its page (`selection_page_changed`); the step's guard; its alignment against the
   content map within `last` ± `position_tolerance_px` (`selection_scan_alignment_rejected` at
   the stage `select_confirm`); then the identity at the chosen member's content position, where
   the nearest instance within `position_tolerance_px` on both axes must be actionable and read
   the chosen business ID. Nothing else on F2 is compared: no hash and no other instance. While
   that position reads an Unknown identity, F2 is captured again every `settle.interval_ms` until
   `settle.timeout_ms`, with no input in between; another Known ID fails at once. No actionable
   instance there, an Unknown reading at the timeout, or another ID fails
   `selection_identity_unconfirmed` (stage `select_confirm`, `expected_identity`,
   `identity_state` `absent`, `unknown` or `different`, and `observed_identity` for `different`).
6. **Tap.** The decision is recorded (section Record per path), and one tap is sampled with the
   step's action seed in the confirmed instance's click rectangle (the layout's `click`, placed
   by the instance F2 shows), as a `task.effect_intent` and its input, bound to F2. The step's
   one `task.effect_completed` follows, then its `expect_after` and, when declared, its
   `expect_identity`.

| # | When | Result |
| --- | --- | --- |
| 1 | `empty` over complete coverage, with no unknown instance | With `on_empty`, the run ends normally (section Ending empty); without it, `selection_not_selected`, as for a single-frame select |
| 2 | `empty` or `insufficient`, with at least one unknown instance | `selection_result_blocked`, `selection_blocking: unknown_instances` |
| 3 | `selected` with the chosen score *S*, and the largest upper bound *U* over the unknown instances is unbounded or above *S* | `selection_result_blocked`, `selection_blocking: outranked_possible`; no runner-up is chosen and nothing is confirmed |

An unknown instance is a position that was readable at least once but holds no fixed Known
reading: its identity or a feature the policy reads was never Known, a later reading
contradicted it, or the evaluator dropped it as unknown. Out of the dictionary counts as
Unknown, never as recognized. Its upper bound is taken at its Known identity, or over the whole
identity domain without one. Rule 3 compares strictly, so a tie never blocks; a member set,
whose scores are all equal, is never blocked by it. An `ambiguous` decision, and an
`insufficient` one with no unknown instance, fail `selection_not_selected`. An evaluator abort
(`unknown`) while scanning, or a declared record list that is unusable at `Pin`, is recorded and
fails `selection_not_selected`.

`selection_result_blocked` (execution-kernel, error) carries the stage `select_decide`,
`selection_kind` (the evaluator's kind verbatim: `selected`, `empty`, `insufficient`,
`ambiguous` or `unknown`), `outcome_key` (the decision's own key from the policy's
`outcome_keys`), `selection_blocking` (`unknown_instances` or `outranked_possible`) and `count`
(the unknown instances). Rules 2 and 3 apply in the same way to a single-frame select that
declares `on_empty`.

Every scan move (advance, corrective, rewind and return) is a `task.effect_intent` of the
step's action, linked to the frame it was sampled on and followed by its input, and none is
followed by `task.effect_completed` (register §1.49 L3-1, §1.58 L3B-2). The task evidence
report (`actingledger export --task-evidence`, [README.md](README.md)) pairs each input with the
step's intent linked to the input's own before-frame, and gives an intent only the inputs
sampled on its frame, so the moves and the tap of one step stay distinct.

The step's deadline is min(StepStarted + `scan.bounds.max_ms`, task deadline); the step timeout
does not apply to it. The step's preparation budget is checked against it, and every state of
the scan checks it first (`selection_scan_bounds_exceeded`, `scan_bound: duration`). Every scan
capture (settle, viewport, rewind, return and F2) is taken without the post-admission OCR
collector, so it feeds neither the post-admission OCR nor the stability sampling; only the
dispatched frame F1 does.

The scan's moves are inputs that come before the selecting tap, so a failure after one is a
failure after input: the step is not retried, the task does not recover within itself, and the
run fails with the failure's code. The selecting tap still comes after every failure path.

The same scan session reads a list for a `read_list` step (Workflow #428 S168-4,
[resource-readings.md](resource-readings.md), "List readings"): steps 1 and 2 only, with no
policy, no early stop, no `Return`, no `Confirm` and no tap. Its scan failures end the run as
here, with the stage `read_list`; a position it could not read is a miss of the list, not a
failure. It stops on the viewport that reads one position more than a list may hold, the miss
`list_rows_exceeded`: the 1025th from S168-LB1a (not built yet; [v012-register.md](v012-register.md)
§1.306), the 257th until it. The scan's own bounds above do not move, so with at most 128
viewports a list reaches 1024 rows only when each advance brings about 8 new positions
([resource-readings.md](resource-readings.md), "Rows").

### Record per path

Workflow #308 SEL-L4b (S0 §3.4). A failure before the decision exists writes no record, as for a
single-frame select. Once the decision exists, exactly one `task.selection_evaluated` precedes
the step's return, with these members:

| Path | `selection_result` | `confirmation` | `unknown_check.blocking` | `coverage.state` | Task result |
| --- | --- | --- | --- | --- | --- |
| A failure before the decision: start, settle, alignment, gap, spacing, an identity mismatch while scanning (`selection_scan_identity_mismatch`), end, unread position, bounds, page, guard, geometry, budget, projection | no record | | | | the failure's code |
| A declared record list is unusable at `Pin` | `unknown` | `not_attempted` | `none` | `not_started` | `selection_not_selected` |
| The evaluator aborts in a viewport | `unknown`, no verdicts | `not_attempted` | `none` | `incomplete` | `selection_not_selected` |
| Rule 1 with `on_empty` | `empty` | `not_attempted` | `none` | `complete` | the run ends Completed with the `on_empty` key |
| `empty` without `on_empty`; `ambiguous`; `insufficient` with no unknown instance | verbatim | `not_attempted` | `none` | `complete` | `selection_not_selected` |
| Rule 2 | `empty` or `insufficient` | `not_attempted` | `unknown_instances` | `complete` | `selection_result_blocked` |
| Rule 3 | `selected` | `not_attempted` | `outranked_possible` | `complete` | `selection_result_blocked` |
| `selected`; F2's capture or settle fails (a capture error, `selection_scan_settle_incomplete`, `candidate_sample_page_or_geometry_changed`, the scan's deadline), its page changed, or the guard fails on it | `selected` | `capture_failed`, `page_changed` or `guard_failed` | `none` | `complete` or `early_stopped` | that code |
| `selected`; `Return` fails | `selected` | `reposition_failed {outcome, stage: return}` | `none` | the same | that code |
| `selected`; F2 does not align against the content map | `selected` | `reposition_failed {outcome, stage: confirm}` | `none` | the same | `selection_scan_alignment_rejected` |
| `selected`; F2's identity is absent, Unknown until the timeout, or another ID | `selected` | `identity_unconfirmed {identity_state, observed_identity?, identity_unknown?}` | `none` | the same | `selection_identity_unconfirmed` |
| `selected`; confirmed | `selected` | `identity_matched {candidate_id, frame_id, content_offset_px}` | `none` | the same | the tap, then `expect_after` and `expect_identity` |

The record of a select over a scan carries the projection `actingcommand.candidate-projection.v2`
(register §1.19, SL1): the coverage, the coverage viewports, the rewind and return viewport
counts (`reposition`), the members numbered as at `Decide` (`candidates`), the unknown positions
(`unknown`) and `gated_out {count, identities_sha256}`, where `identities_sha256` is the SHA-256,
as 64 lowercase hexadecimal digits, of the compact JSON array of the gated-out positions'
business IDs in ascending order. Its `unknown_check` is always present (SL3), and an abort keeps
no verdict. `identity_matched` names the selected candidate and F2 with its aligned content
offset; `observed_identity` is present exactly when the state is `different`.
`identity_matched`, `identity_unconfirmed` and `reposition_failed` are used only with v2, and
`matched` and `mismatched` only with v1 (SL2). A failure of the runtime itself after the
decision is recorded, its confirmation naming `selection_confirmation_capture_failed`, only
when the runtime classifies it as fatal, as for a single-frame confirmation; any other returns
without a record.

A single-frame select that declares `on_empty` keeps its v1 record and adds `unknown_check`. Its
paths are those above without `coverage`, and in place of the identity variants it records
`matched` or `mismatched` as before.

### Ending empty

Workflow #308 SEL-L4b (S0 §1.4). A select step that declares `on_empty` ends the run normally
when rule 1 holds: the evaluator decided `empty` (mode `none_allowed`), the coverage is complete
(for a single-frame select, its frame is the coverage), and no instance is unknown. No further
operation runs and no input follows: the step records no tap, no `task.effect_intent` for it and
no `task.effect_completed`. After the step's record, the kernel records the step's
`StepFinished` with `page_label` the step's page as recognized on its last frame (a scan's last
viewport, a single-frame select's frame) and, for a phased task, `phase` with
`completed: false`, because the step completes no stage. It moves the run to Completed on that
page and finishes it with `executed_steps` including the step, `final_page` that page and
`selected_scheduling_outcome` the `on_empty` key. The host derives the scheduling disposition
from that last `StepFinished` as for any end: the mapping of the run's effect condition that
lists the page, whose key it checks against the kernel's. Of a select step's results this is the
only normal end in the middle of a stage; a judge step ends the run there with every result
(below), and so do the other keyed ends ([resource-declarations.md](resource-declarations.md),
"Per-run operation caps and keyed guard refusals", "Wait steps and wait pages"). An `empty`
decision without `on_empty` still fails. The admission of
`on_empty`, including the route's terminal and the prerequisite and return-home refusal
`select_on_empty`, is in section Declaration.

A judge step (Workflow #428 S168-5b, section Judge step) ends the run on its page the same way,
after its record and with no input, for every result it reaches: a decision with or without
candidates judged worth, a decision with unknown instances, and an `unknown` evaluation. Its end
key is the step's `outcome_key` only for a known decision whose unknown check does not block and
in which no candidate is `unknown_dropped`, and its `unknown_outcome_key` otherwise (section Judge
step, "Which key"); a mapping tells the two apart by its `judge_result` (section Judge step,
"Mapping the two keys").

### Expected identity

Workflow #308 SEL-L4b (S0 §1.3). `expect_after.expect_identity: {target_id, choice_of}` reads
back what a select step chose. Once the step's destination page (`expect_after.page_id`)
matches, `target_id`, an OCR target on that page, is read on that frame and on every poll at
`expect_after.interval_ms` until a Known reading or `expect_after.timeout_ms`; no input occurs.
The text is mapped through the identity declaration of the step `choice_of` names, with its
entries, normalization, `max_distance`, `minimum_margin` and confidence floor. The business ID
that step chose in this run passes. Another Known ID fails at once, only Unknown readings until
the timeout fail, and a `choice_of` step that chose nothing in this run fails. Each failure is
`selection_identity_mismatch` (execution-kernel, error) at the stage `select_expect_identity`,
with `identity_state` `different`, `unknown` or `no_choice`, `expected_identity` (absent for
`no_choice`) and `observed_identity` (for `different`); the step's `StepFinished` precedes it.
It comes after the tap, so it appears only in the task's terminal outcome; the decision record
is unchanged. There is no retry and no fallback.

Workflow #308 SEL-L4b-T: the template path reads the identity where the destination shows it as
an image. The neutral example below is not a value:

```json
"expect_after": {"page_id": "detail", "timeout_ms": 10000, "interval_ms": 500,
  "expect_identity": {"choice_of": "pick",
    "templates": [{"id": "member.a", "target_id": "page/detail_title_a"},
                  {"id": "member.b", "target_id": "page/detail_title_b"}]}}
```

Each entry is a business ID of the `choice_of` step's identity domain and an existing template
target of the pack; several entries may share an `id` (two images of one identity). On every
frame of the destination page, the first and each poll, every entry's target is evaluated by its
own declaration (threshold, metric, mask, colour check and scales); REC-7's flat-field floor and
mask minimums already held for it at package admission. All entries search one rect, the
destination's identity region. The business IDs of the entries that pass are the frame's
reading: none is `absent`, exactly one is that Known ID, two or more are `unknown`. There is no
margin setting beyond the targets' thresholds. The chosen ID passes; another Known ID fails at
once (`different`); `absent` and `unknown` poll again, and at `expect_after.timeout_ms` the step
fails with the state of the last frame read, `absent` or `unknown`; a `choice_of` step that chose
nothing fails `no_choice`. The code, stage, values, poll loop and boundaries are the text path's,
and the template path applies to a choice of either recognition kind, a single-frame select or a
select over a scan.

### Offline

`package dry-run` evaluates a select step with an empty fact snapshot
(`snapshot:offline:empty` at position 1 of no ledger, instance `offline.simulation`, the
package's game and server) at the instant the Lab process read when the dry run started, and
confirms on the same saved frame. The result carries that instant as `selection_now_unix_ms`,
and the decision fingerprint binds it, only when a select step was evaluated; every other
result and fingerprint is unchanged. The kernel's `dry_run_select` evaluates one select step on
one saved frame with given facts and instant, for the `actinglab select` tool of a later slice.
#411 Lab RP: it takes the instance fact snapshot and the package's run parameters, overlaid on
the snapshot as a run's select step holds them (`session.dispatch.parameter.<name>`; the
declared defaults, or given values in their place; [remedies.md](remedies.md), section 4), and
evaluates the rows of the record lists the evaluated policy declares.

`actinglab replay --package <path> --frames <directory> [--facts <file>]` (Workflow #308
SEL-L6a) replays the whole package over a saved frame sequence, the PNG files of the directory
in file-name order, with the production interpreter. Every capture takes the next frame: a
select step's confirmation frame is the frame after the one it was evaluated on, and every
sample of a multi-frame consensus is a frame of its own. A select step evaluates the snapshot of
`--facts`, the instance answer of `actingctl facts --instance` or MCP `ac_facts` read without a
key prefix (`replay_facts_invalid` otherwise), at that answer's `read_at_unix_ms`; without
`--facts` it evaluates the empty offline snapshot at the instant the replay started. The replay
reports each step's decision record, its inputs, which perform nothing, and the page it reached;
a run that does not complete exits with `replay_run_refused`, its end's outcome in the details.
#411 Lab L6: it also reports the readings and fields the run took. `readings` holds one entry
per frame the run handed to recognition, in hand-off order: `frame`, the frame's position in the
sequence from 1, `readings`, the readings that held (`{reading_id, fact_key, value,
confidence_milli}`, in mark order), and `unresolved`, the marks that gave none (`{reading_id,
fact_key, reading_problem}`). `fields` is the run's fields report (`fields_v1`) as the Runtime
records it, with every field its declaration classifies `personal` redacted (no `raw_text`,
`normalized_text`, `value`, `detail` or `extraction`; `redacted` true). Each is absent when the
run took none.

Workflow #406 T4: `package dry-run` and a whole `replay` draw every box point as a run does, from
the run's seed (`--seed <u64>`, else 0) and the touch keys of the root configuration the Lab
reads, each step from its own action seed, without a landing history
([resource-declarations.md](resource-declarations.md), "The Lab's draw"). The dry run's
`would_click` and each replayed effect carry `sampling`, the draw's evidence as the ledger records
it, when the click drew a box point, and the result carries `seed` when a click drew from it; a
replay given the reported seed draws every point again. A decision or effect without a drawn
point serializes as before. A scan replay (`--select`, `--scan`) does not read `--seed`, and
refuses it (`validation_failed`, `argument_problem` `conflicting`).

`actinglab replay --package <path> --frames <directory> --select <operation> --scan <file>
[--facts <file>]` (Workflow #308 SEL-L6b) replays the scan of the select step `<operation>`
through the kernel's own scan path (`dry_run_scan`) instead of the whole package. The frames are
the step's dispatched frame and then every capture its scan took, in order, as a forward run
records them; the scan's inputs perform nothing. `--scan` is the step's scan declaration (S0
§2.1), read from the file. Since SEL-L4b a package declares a step's `scan` itself (section
Declaration), and the replay still takes the declaration `--scan` gives; the session's own
checks apply to it, while the static checks of a package's admission (section Declaration) are
not run on it. The replay never runs `Return` or `Confirm` (section Select over a scan). The scan's
policy evaluates the facts a whole replay gives a select step, checked as the step checks them
(`selection_state_unavailable`, `selection_fact_context_mismatch`), with the rows of the record
lists the step's policy declares. The result is `mode` `offline_scan_replay`, the package,
task and operation, `end` and `scan`: its coverage, start evidence, coverage viewports, members,
unknown and gated-out positions, and the evaluator's decision when it ended the scan. A scan
that covers its list (`end.status` `covered`, coverage `complete` or `early_stopped`) exits 0;
one the evaluator ended before its coverage (`aborted`) or that failed (`failed`, its outcome in
`end.outcome`) exits with `replay_scan_refused`. A move the sequence holds no frame for (a
corrective, a rewind, a capture after the last frame) fails
`selection_scan_replay_frame_missing`; it never passes.

#411 Lab RP: the scan's snapshot also holds the package's run parameters, as a run's select step
holds them ([remedies.md](remedies.md), section 4): `session.dispatch.parameter.<name>` for
every parameter the package declares, with its declared default, observed at the replay's
instant, added before the snapshot is checked. `--parameter <name>=<value>`, repeatable with one
flag per distinct name, replaces that parameter's default, as a dispatch's chosen value does.
Every refusal is `validation_failed` (exit 2) with the details `{argument: "--parameter",
argument_problem, arguments: [{argument, argument_problem}]}`, never an ignored flag: before
anything is read, a flag without its value is `value_missing`, a value that is not
`<name>=<value>` `not_recognised`, a name given twice `repeated`, and `--parameter` without
`--select` (a whole replay, which reads the declared defaults) `conflicting`; once the package is
admitted and before any frame is read, a name the package does not declare (any name, on a
package that declares none) or a value that is not one of its declared options is
`not_recognised`. A package that declares no parameter replays exactly as before.

## Judge step

Workflow #428 S168-5b, written ahead of its code by S168-0 and corrected by its fixes
([v012-register.md](v012-register.md) §1.291). A judge step evaluates the candidates one frame
shows with a selection policy, records the decision, and ends the run with one of two keys its
package declares. It performs no input. Its decision is advice kept in the ledger: no Runtime
component acts on it. **Status:** until S168-5b a package that declares one is refused
(`UnknownField` at `/operations/<i>/judge`).

### Declaration

The names are neutral examples:

```json
{"id": "judge_choices", "from": "choice_page", "to": "choice_page",
 "judge": {"layout_id": "layout/choices", "policy": {"path": "policies/choices.json"},
           "outcome_key": "choices_judged", "unknown_outcome_key": "choices_unknown"}}
```

| Rule | Refusal |
| --- | --- |
| `judge` is an effect: exactly one of `click`, `application`, `select`, `wait`, `read_list` and `judge`; task schema `0.9` only; a page-graph task | an older schema: `UnconsumedField` at `/operations/<i>/judge`; `linear_steps`: `contained_task_linear_invalid`, `reason=judge` |
| `judge` holds `layout_id`, `policy`, `outcome_key` and `unknown_outcome_key`, all four required: `layout_id`, `outcome_key` and `unknown_outcome_key` strings, `policy` the object `{path}` with `path` a string, a safe task-local `policies/<name>.json` | `MissingField`, `InvalidType` or `UnknownField` at the member; a `path` outside `policies/<name>.json`: `InvalidValue` at `…/judge/policy/path` |
| `outcome_key` and `unknown_outcome_key` are distinct | `judge keys=equal` |
| `to` is the `from` page | `judge to=not_from` |
| `layout_id` names a `fixed_slots` layout the task declares on the `from` page, whose `unknown_identity` is `reject`: one frame shows every candidate it can hold. A list that scrolls is read by a list reading, never judged on one frame, and a judge declares no `scan` | `judge layout=not_fixed_slots`, `judge scan=declared` |
| The document passes every check of a select step's (the 512 KiB limit, decode, canonical form, `validate`, `applies_to.candidate_layout_id`, `fields`), with `selection.mode` `none_allowed` and `required_count` at least the layout's slot count (so at most 64 are chosen). Its `required_count` above 1 does not make the step a multi select | `contained_task_select_policy_missing`, `_invalid`, `_mismatch`; `judge mode=not_none_allowed`, `judge required_count=below_slots` |
| The admission rules of a step that declares `on_empty` apply to a judge step, each as written today in section Select step, "Declaration": the task's post-admission OCR is no comparison (a run would otherwise have two sources for its key; the paragraph "A step that declares `on_empty` is admitted only when…", today `TaskProgram::validate_on_empty_post_admission`); when the task declares `post_admission_fields`, their `page_ids` include the step's `from` page (the same paragraph and function); and every field the policy reads has a domain the layout gives it (the paragraph "For a step that declares `scan` or `on_empty`…", today `bound_inputs` in `prepare_select_steps`), which the upper bound of step 4 of section "Execution of a judge step" is computed with | `judge post_admission_ocr=comparison`, `judge post_admission_fields=from_page_absent`, `upper_bound <detail>` (the details of the `on_empty` rules, with `judge` in place of `on_empty`) |
| The task declares `scheduling_outcome`, and the step's state is mapped for both keys: exactly one mapping with that state's effect condition and `judge_result` `judged` lists the `from` page, with key `outcome_key`, and exactly one with `judge_result` `unknown` lists it, with key `unknown_outcome_key` (section "Mapping the two keys"; [README.md](README.md), phases; the outcome coverage check of a task without phases) | `contained_task_outcome_declaration_incomplete`, `task_phases_outcome_incomplete` |
| No member an input uses, and every other member of the operation as the table "Members of a judge step" below says | as that table says; mostly `judge <member>=declared` |
| The step is a terminal: the static walk ends there (paragraph "The static walks" below). It is neither the designated operation nor a member of a designated effect set, since it has no effect | `judge designated=declared` |
| A package with a judge step is no prerequisite or return-home layer, as a package with an `on_empty` step is not: an early end on the step's page would leave the dependent package short of its entry page | `contained_task_prerequisite_incompatible`, `reason=judge_end` |

The declaration gate refuses the structure with `resource_declaration_invalid` at the pointer;
the parser and Runtime admission refuse the rest before any run, the step rules with
`contained_task_select_invalid` and the detail token shown.

**Members of a judge step.** Every member the task schema lets an operation carry (the operation
members of the declaration gate, `DeclarationValidator::operation` in
`crates/pack-containment/src/source/declarations.rs`, with the effects of S168-4 and S168-5b)
either applies to a judge step with its existing meaning or is refused, as follows. A member that
only concerns an input, its confirmation, a retry or recovery of it, or a repeat of the step is
refused: a judge has none of them. Admission checks these members of a judge step before the
common operation checks, as it checks a wait step's members (`TaskProgram::validate_wait_steps`;
[resource-declarations.md](resource-declarations.md), "Wait steps and wait pages"), so a refused
member gives the refusal shown, the first one in this order:

| Member | On a judge step | Refusal |
| --- | --- | --- |
| `id` | applies: the operation's id | as for every operation |
| `from` | applies: the step's page, which is the page of its layout, so never `any` | a layout on another page: `contained_task_select_invalid`, as for a select step |
| `to` | required: the `from` page | `judge to=not_from` |
| `judge` | the effect (the table above) | the table above |
| `click`, `application`, `select`, `wait`, `read_list` | refused: exactly one effect | the first row of the table above |
| `when` | applies with its meaning ([resource-declarations.md](resource-declarations.md), "Task schema `0.9` candidate operations"): judged on the frame the step would start on; a judge whose `when` does not hold is withdrawn, writes no record, ends nothing, and the page's next candidate is taken | as for every operation |
| `retryable` | `false` or absent: a judge's failures end the run on its first attempt, with neither retry nor recovery, as a single-frame select's do | `true`: `judge retryable=declared` |
| `guard`, `unguarded_trusted_coordinate`, `verify_template`, `expect_after`, `post_delay_ms`, `input_recheck`, `observation_window`, `on_guard_refused` | refused: a guard and its source form, a confirmation, a wait after an input, a recheck of an input, a window after a confirmation, a guard refusal; a judge has no input to guard or confirm | `judge <member>=declared` |
| `max_attempts`, `retry_interval_ms`, `on_error` | refused: a judge is never retried and never recovers within the run | `judge <member>=declared` |
| `max_runs`, `on_max_runs` | refused: a judge ends the run the first time it runs, so a per-run cap is never reached | `judge <member>=declared` |
| `pre_delay_ms`, `pre_wait_freezes_ms`, `post_wait_freezes_ms` | refused: timings around an input, which the kernel does not read and a judge has no use for | `judge <member>=declared` |
| `transition`, `optional` | refused: `linear_steps` members, and a judge runs in a page graph | `judge <member>=declared` |
| `threshold`, `method`, `mask`, `maskRange`, `rect_move`, `maa_task`, `maa_task_id` | refused by the gate's existing rule: only beside `verify_template`, which a judge refuses | `UnconsumedField` at the member |
| `destructive` | refused, as on every operation | `UnconsumedField` at the member |
| `purpose`, `provenance`, `verified_live`, `effect` (the string member, not an effect), `timeout_ms`, `consumes`, `produces` | apply with their existing meaning: the gate checks their types and the kernel reads none of them; `purpose`, `consumes` and `produces` go to the step's primitive as for every operation (section "Derived documents of a judge step" below) | as for every operation |

**The static walks.** What they do today, read in the code on main `7910349a`: no rule refuses a
judge step as an end in the middle of a stage, because no walk knows the step; the declaration
gate refuses `judge` itself first (`UnknownField`, Status above). Were a judge step handed to
them unchanged, the phased route's walk, `validate_phased_route_from`
(`crates/actingcommand-contract/src/taskflow.rs`), would take it as an ordinary operation of its
stage: no keyed end (`TaskPhaseOperation::keyed_ends` knows `on_empty`, a keyed `on_max_runs`, a
keyed `on_guard_refused` and a wait step's `on_timeout`), and one destination, its `to`, which is
its `from`. That destination leads back to the state the walk is in, so the path adds nothing
there: the judge's end is no terminal, its two mappings are not checked, and a route whose only
end is a judge step is refused `task_phases_unreachable` (no terminal); where the page is a
target or an end page of the stage, the destination would even count as the stage's advance or
an end page. The page graph's coverage check, `validate_scheduling_outcome_coverage`
(`crates/execution-kernel/src/contained_task.rs`), likewise takes it as an edge to its own page.
S168-5b changes them:

- `TaskPhaseOperation` carries the judge's two keys; its two builders fill them,
  `validate_phases_bundle` (`crates/pack-containment/src/source/mod.rs`) and
  `TaskProgram::phase_operations` (`crates/execution-kernel/src/contained_task.rs`).
- `validate_phased_route_from` takes a judge step's state as a terminal under the state rule of
  "Mapping the two keys" (`task_phases_outcome_incomplete` otherwise), counts it as a terminal,
  and follows no destination of it: the run ends there. A judge that declares `when` may be
  withdrawn, so the page's next candidate is followed too, as after every such candidate.
- `validate_scheduling_outcome_coverage` does the same in a page graph
  (`contained_task_outcome_declaration_incomplete`).
- Every other terminal is mapped among the mappings without `judge_result` ("Which ends"), so
  each mapping count of one (`keyed_terminal_holds`, `end_page_terminal` and the last stage's
  target check in `validate_phased_route_from`; the coverage check's terminal count and
  `keyed_end_coverage`) counts only the mappings without `judge_result`.

### Execution of a judge step

One judge step runs, with no input:

1. Its frame F1, the dispatched frame recognized as its `from` page, is projected with the
   layout: P1 (as section Execution, step 1).
2. The host supplies the instance fact snapshot, with the record lists the document declares and
   the run's parameters, and the evaluation instant, as for a select step (section Execution,
   step 2). The tables are the run's (section Tables).
3. The policy is evaluated over P1's actionable candidates (section Execution, step 3).
4. The unknown instances are counted as for a single-frame select that declares `on_empty`: the
   actionable candidates the kernel keeps from the evaluator (an unresolved consensus feature,
   an identity feature that read no identity; out of the dictionary counts as unknown) and those
   the evaluator drops as unknown, with their upper bound computed as there. A static table drop
   is no unknown instance here either, as there (section Tables, "Dropped by the table"): the
   record's unknown check does not count it. For the judge's key it still counts as a candidate
   that could not be judged ("Which key" below). When the evaluation is `unknown` there is no
   verdict and nothing the evaluator dropped: the check still counts the slots the kernel kept
   from the evaluator, with their upper bound, as a select over a scan counts its unknown
   positions when the evaluator aborts (section Select over a scan: "nothing blocks it"), and it
   never blocks: `blocking` is `none` whatever their count. The run then ends with
   `unknown_outcome_key` because of the evaluation, not because of the check.
5. The decision is recorded once, as `task.selection_evaluated` (section Records), with the
   confirmation `judge_only` and `unknown_check`.
6. The kernel records the step's `StepFinished` with `page_label` its page as recognized on F1
   (for a phased task, `phase` with `completed: false`), moves the run to Completed on that page
   and finishes it with `executed_steps` including the step, `final_page` that page and
   `selected_scheduling_outcome` the step's `outcome_key` or its `unknown_outcome_key`, as the
   table below says. The host derives the disposition from that last `StepFinished` as for any
   end, with the mapping of section "Mapping the two keys".

No confirmation frame is captured, no guard is judged, no point is sampled, and no
`task.effect_intent`, input or `task.effect_completed` belongs to the step.

**Which key.** The run ends with `outcome_key` only when all three hold:

- the evaluation is known (not `unknown`);
- the record's unknown check does not block (no unknown instance);
- no candidate of the layout was dropped because a read was Unknown: a table read with no row
  for the key, a null cell with no fallback, an unusable list, a list maximum that is Unknown
  ([selection-policy.md](selection-policy.md), "Table maximum over a list"). In the record's
  words, no candidate is `unknown_dropped`.

Otherwise it ends with `unknown_outcome_key`. A static table drop is such a drop: it is not an
unknown instance and the unknown check does not count it (step 4), and it still makes the run end
with `unknown_outcome_key`. A candidate that a gate decides against on a known value (the class
gate's `absent`, a comparison whose operands are known) is a known decision, `gate_rejected`, and
does not count.

**A list maximum read as a lower bound** (Workflow #428 S168-LB1b, not built yet, amending this
planned text; [v012-register.md](v012-register.md) §1.306; the coordinator's ruling 8 of 11-11,
not Alice's words; [selection-policy.md](selection-policy.md), "Lower bound of a list maximum"). A
reference that opts in and reads "at least L" is no Unknown read. A comparison the bound settles
is a known verdict, `ranked` or `gate_rejected`, and does not count against `outcome_key`: the
judgment is `judged`. A comparison the bound does not settle is Unknown: its candidate is
`unknown_dropped` under `drop_candidate`, or the evaluation `unknown` under `abort_evaluation`,
and the run ends with `unknown_outcome_key`: the judgment is `unknown`. A score taken at L is a
`ranked` verdict, `judged`; the record carries the bound in the read's
`selection_table_list_max` (`bound` `at_least`, `lower_bound`, `lower_bound_identity`,
`unknown_count`) and in the decision's `selection_decided_on_lower_bound`. The record gains no
member and its unknown reasons no word (an unsettled comparison reads `lookup_miss`), so the
judge's `ledger` row does not move for it. Whichever of S168-LB1b and S168-5b merges second
builds this paragraph.

**Every result ends the run Completed, with one of the two keys:**

| Result | The record | The run |
| --- | --- | --- |
| A decision, `selected` (candidates judged worth) or `empty` (none), in which every candidate was judged: the evaluation is known, the record's unknown check does not block, and no candidate is `unknown_dropped` (each is `ranked` or `gate_rejected`) | verbatim; `unknown_check.blocking` `none` | Completed with `outcome_key` |
| The same with candidates that could not be judged, the named gap: unknown instances (the unknown check blocks), or a candidate `unknown_dropped`, a static table drop included (the check does not block on that alone) | verbatim, with every verdict it has; `blocking` `unknown_instances` and `unknown_instances` their count when there are unknown instances, else `blocking` `none` | Completed with `unknown_outcome_key` |
| `unknown`: the evaluator ended the evaluation (an unusable record list, an `abort_evaluation` rule) | `unknown`, no verdict, its reason and rule; `unknown_check.blocking` `none`, `unknown_instances` the slots the kernel kept from the evaluator (step 4) | Completed with `unknown_outcome_key` |

Neither end makes any input, and the record is the same whichever key ends the run. The run's
scheduling outcome tells a judgment from "could not judge": `outcome_key` for a judgment of every
candidate, `unknown_outcome_key` otherwise, so that a catalog can tell the two apart,
for example to run the judge again. The record tells the rest apart: the decision's
own `outcome_key`, the string the document's `applies_to.outcome_keys` declares for its outcome,
and `unknown_check`. Each verdict tells one candidate apart: `ranked` and chosen (judged worth),
`gate_rejected` (not worth; the gate that decided names why), `unknown_dropped` (could not be
judged; its reason).

**Failures before the decision** end the run with their code and write no record, as for a
single-frame select (section Execution): F1 not projected (`candidate_projection_budget_exceeded`,
`candidate_feature_failed`, `candidate_feature_provider_missing`,
`invalid_candidate_projection`), `selection_state_unavailable`,
`selection_fact_context_mismatch`, `selection_evaluation_failed`, and a record too large or
invalid (`selection_record_too_large`). None of them follows an input: there is none.

### Mapping the two keys

The keyed-terminal rule ([README.md](README.md), phases;
[resource-declarations.md](resource-declarations.md), "Per-run operation caps and keyed guard
refusals") maps one page under one effect condition to exactly one mapping, and the host derives
a run's key from the mapping of the run's effect condition that lists its final page (section
Ending empty). That is how an `on_empty` key is mapped. A judge's two keys end on the same page
under the same effect condition, so that means cannot tell them apart. A mapping of
`scheduling_outcome` therefore may declare `judge_result`, a closed vocabulary of exactly two
words. The names are neutral examples:

```json
"mappings": [
  {"outcome_key": "choices_judged", "effect": "no_designated_effect",
   "terminal_pages": ["choice_page"], "judge_result": "judged"},
  {"outcome_key": "choices_unknown", "effect": "no_designated_effect",
   "terminal_pages": ["choice_page"], "judge_result": "unknown"}
]
```

| Word | The end it maps |
| --- | --- |
| `judged` | a judge step's end with its `outcome_key`: a known evaluation, an unknown check that does not block, and no candidate `unknown_dropped` |
| `unknown` | a judge step's end with its `unknown_outcome_key`: an `unknown` evaluation, unknown instances, or a candidate `unknown_dropped` (section "Execution of a judge step", "Which key") |

- **Which ends.** A mapping with `judge_result` maps only a judge step's end with that result, on
  the pages it lists. A mapping without it never maps a judge step's end; every other end of the
  task is mapped among the mappings without it, by the rules as before, and such a mapping may
  list a judge step's page too.
- **Overlap.** Today's declaration check, `SchedulingOutcomeDeclaration::validate`
  (`crates/actingcommand-contract/src/event/payload.rs`), refuses two mappings with the same
  effect condition that list the same page (`invalid_scheduling_outcome_declaration`, field
  `mapping_overlap`). S168-5b changes that check: two mappings overlap only when they have the
  same effect condition, list the same page, and have the same `judge_result` or both have none.
  A mapping with `judge_result` and a mapping without it, on the same condition and page, do not
  overlap: the first maps the judge's ends, the second maps every other end at that page. Nor do
  two mappings whose `judge_result` differs, so the two mappings of the example above, which
  differ only in `judge_result`, are admitted.
- **The state rule.** For a judge step's state (its `from` page and the effect condition the walk
  reaches it under), exactly one mapping with that condition and `judged` lists the page, and its
  key is the step's `outcome_key`; exactly one mapping with that condition and `unknown` lists it,
  and its key is the step's `unknown_outcome_key`. A judge state reachable under both effect
  conditions would need two mappings with one key, which unique keys forbid, so it is refused.
- **The host** derives the disposition from the run's last `StepFinished` as for any end, and
  matches the mapping on the run's effect condition, the page and, when that step is a judge
  step, the result the kernel's key names (`judged` for the step's `outcome_key`, `unknown` for
  its `unknown_outcome_key`); it checks the mapping's key against the kernel's.
- **Persistence.** No persisted record carries the member. A mapping without it serializes byte
  for byte as before. It joins `package` revision 9 with the judge step
  ([component-interfaces.md](component-interfaces.md), "Planned: Workflow #428 168").

| Rule | Refusal |
| --- | --- |
| `judge_result` is the string `judged` or `unknown` | `invalid_scheduling_outcome_declaration`, field `judge_result` (the source parser: `resource_declaration_invalid`, `InvalidType` at `…/judge_result` for no string, `InvalidValue` there for another string) |
| task schema `0.9` only | an older schema: `UnconsumedField` at `…/judge_result` |
| only in a task with a judge step; every page the mapping lists is a judge step's `from` page; never together with `busy` | `invalid_scheduling_outcome_declaration`, field `judge_result` (the source parser: `resource_declaration_invalid`, `InvalidValue` at the declaration); Runtime admission: `contained_task_outcome_declaration_invalid` |
| no two mappings overlap: the same effect condition, a page both list, and the same `judge_result` or none on both (bullet "Overlap" above) | `invalid_scheduling_outcome_declaration`, field `mapping_overlap`, the refusal and field of today's check, which S168-5b changes in `SchedulingOutcomeDeclaration::validate` |
| the state rule above | `contained_task_outcome_declaration_incomplete` in a page graph, `task_phases_outcome_incomplete` in a phased route |

### No input, and how that is enforced

Three layers, each sufficient on its own:

1. **Declaration.** The effect excludes every other effect and every member an input uses, and
   its layout is `fixed_slots` without a scan, so nothing moves on the page. The declaration
   gate, the parser and admission refuse anything else before any run.
2. **Kernel.** The judge path is a function of the projection, the snapshot, the instant, the
   tables and the document that returns the step's end; it is not handed the run's input port or
   the step's action seed, so it cannot sample a point or send an input. The run is Completed
   after it, so no later operation runs.
3. **Host.** The host accepts no `task.effect_intent` and no input for a judge step's action: one
   would end the run with `contained_task_judge_input_refused` (runtime-host, error) before the
   input reaches the device. It ends that run only, never the Runtime or another instance.

So a judge step that ran is followed in the ledger by no `task.effect_intent`, input or
`task.effect_completed` of its action. The tests of [v012-register.md](v012-register.md) §1.291
pin all three layers.

### Derived documents of a judge step

As section "Derived documents" says for a select step, with the judge's own effect:

- **Navigation edge and page operation:** none. A judge step makes no tap and its `to` is its
  `from`, so the source build skips it as it skips a select or wait step, and no route passes
  one.
- **Primitive:** the shape of a click primitive with `click: null` and `guard: null` (it declares
  no guard), with `purpose`, `consumes` and `produces` as for every operation, and the `judge`
  object added as declared, as a select step's primitive adds `select`. The primitives stay at
  schema `0.6`.
- **Sealed task** (`canonical_task`): keeps `judge` as declared; no click and no guard are
  inferred.
- **Operation index:** unchanged.
- **`task.effect_completed`:** none, and no `task.effect_intent` (section "Execution of a judge
  step").

### Account data and offline

The verdicts and the list maxima a judge reads depend on what the instance holds: they are
account data, written to the ledger only, and no Runtime path writes them anywhere else.
`actinglab replay` runs a judge step with the production interpreter and reports its record;
`package dry-run` evaluates it with the empty offline snapshot, as it evaluates a select step
(section Offline), and confirms nothing.

## Records

### `task.selection_evaluated`

A select step records its decision in the task ledger as the event
`task.selection_evaluated` (family `task`), carrying the task fact

```json
{"kind": "selection_evaluated", "step_index": 3, "operation_label": "choose_slot",
 "selection": { … }}
```

Each select attempt that reaches a decision appends exactly one. A single select appends it
before any input and before any failure return (section Select step). A multi select (section
Multi select below) appends it once, when its tap sequence ends, at its first failure or where
it stops, before the step's `task.effect_completed`; a decision with nothing to tap is appended
after the decision, as for a single select. The host appends it linked to the step's action and
to the last frame the attempt read. A judge step (Workflow #428 S168-5b, section Judge step)
appends exactly one as well, linked to its action and to F1, before its `StepFinished`. A
package without a select or judge step never produces the event.

The record has the shape of a `policy.*` decision: identity hashes and a complete breakdown.

| Field | Meaning |
| --- | --- |
| `layout_id`, `page_id` | The evaluated layout and its page; equal to the projection's. |
| `projection` | The full core candidate projection of the step's frame ([candidate-projection.md](candidate-projection.md)), including its `candidate_set_sha256`. |
| `policy.path` | The package-relative path of the policy document the step declares, `operations/<task>/<declared path>` below the resource root. |
| `policy.policy_sha256` | The evaluator's canonical document identity, `sha256:<hex>`. |
| `policy.policy_id` | The document's `policy_id`. |
| `fact_snapshot_id`, `input_ledger_position` | The instance fact snapshot the evaluation read and its ledger position. |
| `now_unix_ms` | The instant the host supplied to the evaluation. |
| `input_sha256` | The evaluator's canonical identity of candidates, facts and instant, `sha256:<hex>`. |
| `outcome`, `outcome_key`, `selected[]` | The decision, its resource-declared key and the chosen candidate IDs. |
| `verdicts[]` | One breakdown per evaluated candidate: status, score, rank, gates, terms, reasons. |
| `reasons[]` | The decision's reason chain, each reason's outcome (`code`, `category`, `values`; Workflow #378 A2-P). |
| `confirmation` | How the confirmation frame compared, below. |

`confirmation` is one of

| `kind` | When |
| --- | --- |
| `not_attempted` | The outcome is not `selected`. |
| `matched` `{candidate_set_sha256}` | The confirmation frame's candidate set hashes like the evaluated one. |
| `mismatched` `{candidate_set_sha256}` | It hashes differently; no input follows. |
| `decision_matched` `{candidate_set_sha256, decision_sha256}` | A step with `confirm: "decision"` (ledger revision 4, register §1.153 LR-1; written since §1.166): the confirmation frame's decision hashes like the evaluated decision. `candidate_set_sha256` is the confirmation frame's own and may differ from the evaluated projection's; `decision_sha256` is its decision hash (section Confirming by the decision). |
| `decision_mismatched` `{candidate_set_sha256, decision_sha256}` | As `decision_matched`, but the confirmation frame's decision hashes differently; no input follows. |
| `tracked` `{candidate_set_sha256, decision_sha256, shift_px}` | A step with `confirm: "track"` (ledger revision 7, Workflow #404 FS-3, register §1.284): the confirmation frame's decision selected one candidate whose anchor match origin lies `shift_px` (0..=`track_px`, so at most 64) from the evaluated choice's; the tap goes into that candidate's click box on the confirmation frame. `candidate_set_sha256` and `decision_sha256` are the confirmation frame's own (section Confirming by tracking). |
| `track_lost` `{candidate_set_sha256, decision_sha256, shift_px?}` | As `tracked`, but the confirmation frame's decision selected nobody (`shift_px` absent) or a candidate farther than `track_px` (`shift_px` its shift); no input follows. |
| `page_changed` | The confirmation frame is not the step's page. |
| `guard_failed` `{code}` | The step's guard failed on the confirmation frame. |
| `capture_failed` `{code}` | The confirmation frame could not be captured. |
| `judge_only` | A judge step's record, whatever its outcome (Workflow #428 S168-5b, section Judge step; planned `ledger` revision 8, [component-interfaces.md](component-interfaces.md), "Planned: Workflow #428 168"): no confirmation is attempted and no input follows. No member. |

A confirmation is attempted exactly when the outcome is `selected`, except in a judge step's
record, whose confirmation is always `judge_only` and never attempted. Together with the
projection, a matched or mismatched record holds both candidate-set hashes. A record with
`unknown_check` (Workflow #308 SEL-L4b) attempts it only when, besides, nothing blocks the
decision, and a select over a scan records the v2 projection and its own confirmation kinds
(section Record per path).

#### Multi select

Workflow #357 (frozen model M-357b v1.0 §5, slice DB-3s): a step whose policy declares more
than one candidate (2–16) chooses up to N of them in one attempt and records them in one
`task.selection_evaluated`, with one confirmation per pick. DB-3s brings the shape, its
readers and its rules; DB-3b (single frame) and DB-3c (scan) write it. A single select's record
carries none of it and keeps its bytes.

The record gains the optional member `multi_select`, after `unknown_check`:

| Member | Meaning |
| --- | --- |
| `declared_count` | The policy's `selection.required_count`, 2–16. |
| `required_count` | N, 1..=`declared_count`: min(`declared_count`, `free_slots`) when `free_slots` is present, else `declared_count`. |
| `free_slots` | The free-slot fact's reading, 1–65535; absent when the policy declares none or the reading was unavailable. |
| `order` | `rank` or `content`: the step's tap order. `selected` keeps the evaluator's rank order; the tap order is in `picks`. |
| `mark_feature` | The step's mark feature, a feature name of the layout. |
| `pick_set_sha256` | The decision frame's unmarked hash, 64 lowercase hex digits; present exactly with a v1 projection. Its definition is the writer's; a reader checks its presence and grammar and never recomputes it. |
| `picks` | At most 16, in tap order: `{candidate_id, confirmation, tap_check?}`, the confirmation before the pick's tap and the check after it, present only when that check reached a result. |

`confirmation` gains six kinds and `reposition_failed` the stage `tap_check`:

| `kind` | Where | Meaning |
| --- | --- | --- |
| `pick_matched` `{frame_id}` | a v1 pick's confirmation | The frame confirmed the pick before its tap: F2 for the first pick, the previous pick's tap-check frame for every later one. |
| `pick_moved` `{frame_id, pick_set_sha256}` | a v1 pick's first confirmation or any tap check | The frame's unmarked hash differs from the decision frame's: the list moved. |
| `mark_unconfirmed` `{candidate_id, mark_expectation, mark_state, frame_id}` | a pick's confirmation or tap check | The mark of `candidate_id` (an actionable v1 candidate, or a selected v2 one) read `marked`, `unmarked` or `unknown` where `marked` or `unmarked` was required; never the reading required. |
| `tap_marked` `{frame_id}` | a tap check | The tapped pick reads as marked. |
| `picks_confirmed` `{count}` | the top level | Every pick was confirmed, tapped and checked. |
| `picks_stopped` `{count}` | the top level | The sequence stopped outside the record (a deadline, the task timeout, an input failure, the run's end) after `count` picks were tapped and checked. |
| `reposition_failed` stage `tap_check` | a v2 tap check | The frame after the tap did not align. |

Rules M1–M8 (M-357b §5.2) hold beside the path rules; a violation is
`invalid_task_selection_record` with the field named:

- M1 (`multi_select`): the counts and N as above; the outcome is not `ambiguous`; `selected`
  holds at most N and `insufficient.required` is at most N; `mark_feature` is a feature name;
  `pick_set_sha256` as above.
- M2 (`picks`): when the top-level confirmation was attempted, the picks are exactly
  `selected`, each once, in `selected`'s order for `rank` and in increasing candidate index for
  `content`.
- M3 (`picks`): a v1 pick's first confirmation is `pick_matched`, `pick_moved`,
  `mark_unconfirmed`, `page_changed`, `guard_failed`, `capture_failed` or `not_attempted`, and
  every later one `pick_matched` on the previous pick's tap-check frame or `not_attempted`; a
  v2 pick's confirmation is `identity_matched` naming the pick, `identity_unconfirmed`,
  `reposition_failed` at `return` or `confirm`, `mark_unconfirmed`, `page_changed`,
  `guard_failed`, `capture_failed` or `not_attempted`; a tap check is `tap_marked`,
  `mark_unconfirmed`, `page_changed`, `guard_failed` or `capture_failed`, a v1 one also
  `pick_moved` and a v2 one also `reposition_failed` at `tap_check`.
- M4 (`picks`): the picks succeed (confirmed, then `tap_marked`) up to a first pick j whose
  confirmation failed (no tap check), whose tap check failed, that was confirmed with no tap
  check, or that was not attempted (no tap check); every pick after j was not attempted.
- M5 (`confirmation`): the top level is `not_attempted` exactly when there is no pick; it is
  `picks_confirmed` with the number of picks when every pick succeeded, a verbatim copy of j's
  failed confirmation or tap check, or `picks_stopped` with j when j was confirmed with no tap
  check or not attempted. A multi-select record's top level is never `matched`, `mismatched`,
  `identity_matched`, `pick_matched` or `tap_marked`; a single select's never one of the six
  kinds or the stage `tap_check`.
- M6 (`path_rule_4`): `unknown_instances` blocks a partial `selected` (fewer than N) with an
  unknown instance too.
- M7 (`path_rule_5`): `outranked_possible` needs a full `selected` (N), and compares the bound
  with the lowest selected score.
- M8 (`path_rule_1`): an early-stopped scan holds a full `selected` and no
  `selection_quota_unfilled` decision reason.
- "A record with `unknown_check` chooses exactly one candidate" holds only without
  `multi_select`, and (Workflow #428 S168-5b) without the confirmation `judge_only`.

Each tap is one `task.effect_intent` linked to its pick's confirmation frame, before the
record; a scan's moves between picks never link to a confirmation frame; the step has one
`task.effect_completed`, after the record. The record's frame is the last tap-check or failing
frame. No reader requires the record before the step's first intent.

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
| `PolicyReasonRecord` | `DecisionReason` | the reason's `outcome` (`code`, `category`, `values`); the `detail` is not recorded |

The record takes the remaining decision fields under its own names: `policy_id` and
`policy_sha256` into `policy`, `candidate_layout_id` as `layout_id`, `evaluated_at_unix_ms` as
`now_unix_ms`, and `candidates` as `verdicts`; `input_sha256`, `fact_snapshot_id`, `outcome`,
`outcome_key`, `selected` and `reasons` keep their names. The decision's `schema_version` is
not recorded: it is the schema of the policy document that `policy_sha256` identifies.

#### Validation and bounds

A record is checked when it is written and when it is read:

- the projection is valid and its sealed hash equals the recomputed one; `layout_id` and
  `page_id` equal the projection's;
- `policy_sha256` and `input_sha256` are `sha256:<hex>`, `fact_snapshot_id` is a token,
  `input_ledger_position` and `now_unix_ms` are positive; `policy` has no other member
  (Workflow #336 PKG-S2: a record that carries `policy.package_sha256` is not read);
- at most 64 verdicts, each for a distinct actionable candidate of the projection;
- `selected` holds exactly `count` distinct IDs of `ranked` verdicts for a `selected` outcome
  and nothing otherwise; `selected` has a positive count and `insufficient` fewer survivors
  than required;
- `matched` carries the projection's hash and `mismatched` a different 64-hex hash;
- `tracked` and `track_lost` (ledger revision 7) belong to a record without `multi_select`
  whose projection is v1 of a `repeated_anchor` layout, carry two 64-hex hashes and, like the
  other confirmations, an optional embedded projection that seals the first; a `tracked`
  `shift_px` is at most 64; neither the decision hash nor the shift is recomputed;
- `judge_only` (Workflow #428 S168-5b; planned `ledger` revision 8) belongs to a record without
  `multi_select` whose projection is v1 of a `fixed_slots` layout and which carries
  `unknown_check` with `blocking` `none` or `unknown_instances` (with a `selected` or `empty`
  outcome, `none` exactly when `unknown_instances` is 0; with an `unknown` outcome always `none`,
  whatever `unknown_instances` counts; never `outranked_possible`); its outcome is `selected`,
  `empty` or `unknown`, and `selected` holds exactly the `ranked` verdicts, 0 to 64 of them, in
  rank order;
  any other confirmation never comes with a judge step's decision;
- reason codes and confirmation codes are tokens, reason details text of at most 1024 bytes,
  the decision chain holds 1–128 reasons, as for a policy dispatch; labels and IDs are at most
  256 bytes without control characters.

A structural violation is `invalid_task_selection_record` with the field it concerns. Only
the writer measures bytes, on the current encoding: the projection within its 32 KiB budget
(`candidate_projection_budget_exceeded`) and the record's compact JSON within 64 KiB
(`selection_record_too_large`). The producer runs the same check before any input, so an
oversized record fails the task and nothing is truncated; a multi select's producer runs it
once before its first tap (M-357b §5.3), so that bound too fails before any input.

#### Ledger placement

- Durability: durable. A single select's record is the decision an input follows; a multi
  select's is the decision and the confirmations of the taps that preceded it.
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
  The same holds for `multi_select` and the six multi-select kinds (M-357b §5.1): a reader
  built before DB-3s rejects a ledger that holds one, so the UI build paired with v0.12.0
  pins a Runtime revision that includes DB-3s, and so does `actingledger`. They join ledger
  revision 3 because DB-3s merges before the v0.12.0 tag; 0.12.x adds no member to them.
  The confirmation `judge_only` (Workflow #428 S168-5b) joins `ledger` revision 8, shared with
  Workflow #434's LAB-6 in one Runtime release: a reader built before it rejects a ledger that
  holds one, as for `tracked`, so the console and `actingledger` move to a Runtime that knows it
  before such a ledger reaches them.
