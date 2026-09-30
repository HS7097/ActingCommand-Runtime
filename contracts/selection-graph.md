# Selection graph

This document collects the recognition side of the selection graph of
[Workflow #308](https://github.com/HS7097/ActingCommand-Workflow/issues/308). Only the
section **Checks** is frozen. Nodes, edges, gates and selection are frozen by later #308
slices. No game-specific values are part of this contract.

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
