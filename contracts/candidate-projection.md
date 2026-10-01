# Candidate projection `actingcommand.candidate-projection.v1`

A candidate layout declared for a page yields, on one frame, a bounded set of candidates:
the rectangles a select step may act on, each with the feature values a selection policy
reads ([Workflow #308](https://github.com/HS7097/ActingCommand-Workflow/issues/308)). This
document freezes the projection's shape, its identifier grammar, its hash, its budgets, its
public and controlled forms and their validation.

The contract crate owns all of these (`crates/actingcommand-contract/src/candidate_projection.rs`).
The recognition pack produces a projection from a scene and a pack through one entry that the
in-task select step, the online observation and the offline Lab share; the same scene and
pack give byte-identical output everywhere. A feature that needs a provider the offline Lab
does not have fails with `candidate_feature_provider_missing`. Implementation status: the
contract types, the ledger record and the producer for `fixed_slots` layouts exist (section
Generation); `repeated_anchor` layouts, the observation outputs and the select step arrive with
later #308 slices.

## Shape

```json
{"schema_version": "actingcommand.candidate-projection.v1",
 "page_id": "list_page", "layout_id": "layout/main_slots", "layout_kind": "fixed_slots",
 "frame": {"width": 1280, "height": 720},
 "candidates": [
   {"id": "layout/main_slots#00", "instance_index": 0, "actionable": true,
    "rect": {"x": 214, "y": 204, "width": 176, "height": 90},
    "click": {"x": 224, "y": 214, "width": 156, "height": 60},
    "features": {"mark_score": {"type": "integer", "value": 991, "confidence": 991},
                 "marked": {"type": "boolean", "value": true, "confidence": 991},
                 "open": {"type": "boolean", "value": true, "confidence": null}}}],
 "candidate_set_sha256": "e45f4db1262ff314e6ffd40c588bfc02bbf4be76d9eb626434c7158488719e37"}
```

| Field | Meaning |
| --- | --- |
| `page_id` | The recognized page the layout is declared for. |
| `layout_id`, `layout_kind` | The declared layout; `fixed_slots` or `repeated_anchor`. |
| `frame` | Width and height of the frame the projection was taken on; not its identity. |
| `candidates[]` | In instance order: `instance_index` equals the position, `0` first. |
| `rect`, `click` | Frame pixels. `click` is where an input may be sampled. |
| `actionable` | `false` for an instance that reaches outside the frame; it carries no features and is never evaluated. |
| `features` | By declared name, sorted by name; at most one value per name. A feature the layout declares but the candidate lacks is absent. |
| `candidate_set_sha256` | The sealed hash below. |

An actionable candidate's `rect` and `click` lie entirely inside the frame. A rectangle has a
positive width and height; its origin is signed because an instance at the frame edge may
start outside it.

A feature is `{"type": "boolean"|"integer", "value": …, "confidence": …}`. A `passed` feature
is a boolean; a `measure_milli` feature is an integer. `confidence` is the backend's own
confidence in integer milli for template, OCR and NN targets (the same value as their
measure), and `null` for color, color digest and composite targets. v1 does not hand
`confidence` to the evaluator.

## Candidate IDs

A candidate ID is `{layout_id}#{NN}`: the layout ID, `#`, and the instance index as two
zero-padded decimal digits (`00`–`63`). It is structurally the pair (layout ID, instance
index). A layout ID matches `^[a-z0-9][a-z0-9_./-]{0,63}$` and so never holds `#` or `[`;
a candidate ID cannot collide with a page element ID, which starts with `[`. A feature name
matches `^[a-z][a-z0-9_]{0,31}$`.

`candidate_id(layout_id, index)` formats an ID and `parse_candidate_id(id)` splits one; both
reject any other form with `invalid_candidate_id`.

## Candidate-set hash

`candidate_set_sha256` is the SHA-256, as 64 lowercase hexadecimal digits, of the compact
JSON (no whitespace) of

```text
{schema_version, page_id, layout_id, layout_kind, frame{width, height},
 candidates[{id, instance_index, actionable, rect{x, y, width, height},
             click{x, y, width, height}, features{name: {type, value}}}]}
```

in exactly this field order, with the features of each candidate sorted by name. It covers
every candidate's geometry and every feature's type and value, integer measures included. It
does not cover `confidence` and never the frame's identity, so two captures of an unchanged
screen hash alike. The core computes it over the full projection; the public form carries it
unchanged and nothing recomputes it from a withheld form.

For the projection above the hashed text is

```text
{"schema_version":"actingcommand.candidate-projection.v1","page_id":"list_page","layout_id":"layout/main_slots","layout_kind":"fixed_slots","frame":{"width":1280,"height":720},"candidates":[{"id":"layout/main_slots#00","instance_index":0,"actionable":true,"rect":{"x":214,"y":204,"width":176,"height":90},"click":{"x":224,"y":214,"width":156,"height":60},"features":{"mark_score":{"type":"integer","value":991},"marked":{"type":"boolean","value":true},"open":{"type":"boolean","value":true}}}]}
```

and its hash is `e45f4db1262ff314e6ffd40c588bfc02bbf4be76d9eb626434c7158488719e37`.

## Budgets

Every budget is a contract constant. Exceeding one is an error that names the item; nothing
is truncated.

| Item | Limit | Constant | Error |
| --- | --- | --- | --- |
| Candidates per layout | 64 | `CANDIDATE_PROJECTION_MAX_CANDIDATES` | `candidate_projection_budget_exceeded` (`candidates`) |
| Features per layout | 8 | `CANDIDATE_PROJECTION_MAX_FEATURES` | same (`features`) |
| Layouts per page | 4 | `CANDIDATE_PROJECTION_MAX_LAYOUTS_PER_PAGE` | same |
| Layouts per package | 64 | `CANDIDATE_PROJECTION_MAX_LAYOUTS_PER_PACKAGE` | same |
| OCR and NN feature evaluations per projection | 16 | `CANDIDATE_PROJECTION_MAX_PROVIDER_EVALUATIONS` | same |
| Compact JSON of one core projection | 32 KiB | `CANDIDATE_PROJECTION_MAX_BYTES` | same (`projection_bytes`) |
| Compact JSON of every public candidate set of one observation | 32 KiB | `CANDIDATE_SETS_MAX_BYTES` | `candidate_sets_budget_exceeded` (`candidate_sets_bytes`), raised when the observation is built, not when an artifact is written |

The layout counts per page and per package are checked when the recognition pack is admitted
([selection-graph.md](selection-graph.md), section Candidate layouts); the producer checks the
candidate, feature and provider-evaluation budgets again before it evaluates anything.

The observation total keeps a Lab terminal record within its 256 KiB artifact: the before
observation holds a 32 KiB page projection, 64 KiB of facts and 32 KiB of candidate sets,
and the after observation holds 32 KiB and 64 KiB and no candidate sets. An evidence
artifact adds the controlled candidate features, a subset of the full sets, and stays within
the same bound.

## Public and controlled forms

The core projection carries no privacy. Observation surfaces follow the page observation's
`facts` / `private_facts` precedent:

- A candidate's privacy is the strictest privacy of the targets its features read.
- The public form (`PublicCandidateSet`) keeps every set-level field, the hash included. A
  `personal` candidate keeps `id`, `instance_index`, `actionable`, `privacy`, `rect` and
  `click` and has no `features` key; every other candidate keeps its features.
- The controlled form (`private_candidate_features`) maps each personal candidate's ID to its
  features. It appears only in evidence.

`CandidateProjection::split(privacy)` takes one privacy per candidate, in candidate order, and
returns both forms without rehashing.

## Validation

`CandidateProjection::new` checks the candidates, seals the hash and checks the byte budget.
`validate` checks, in addition, that a decoded projection's sealed hash equals the recomputed
one:

- `schema_version` is `actingcommand.candidate-projection.v1`;
- `page_id` is non-empty, at most 256 bytes and free of control characters;
- `layout_id` follows its grammar; the frame has no zero dimension;
- candidate `k` has `instance_index` `k` and ID `{layout_id}#{k:02}`;
- every rectangle is non-empty; an actionable candidate's rectangles lie inside the frame;
- a candidate that is not actionable carries no features; feature names follow their grammar;
- the budgets above hold.

A shape violation is `invalid_candidate_projection` with the field it concerns. The public
form is validated the same way, except that the hash cannot be recomputed, and exactly the
personal candidates withhold their features.

## Generation

`SceneEvaluation::project_candidates(layout_id)` of the recognition pack
(`crates/recognition-pack/src/candidate_layout.rs`) is the only producer. Its inputs are the
scene and the admitted pack, nothing else; the layouts it reads are declared as in
[selection-graph.md](selection-graph.md), section Candidate layouts.

1. The pack declares a `fixed_slots` layout `layout_id`, or the projection fails with
   `candidate_layout_unknown`. The frame has the pack's coordinate space, or it fails with
   `invalid_candidate_projection` (`frame`).
2. Before anything is evaluated, the slots, the features and the OCR and NN evaluations the
   layout needs are within their budgets, or it fails with
   `candidate_projection_budget_exceeded` (`candidates`, `features`, `provider_evaluations`).
   The OCR and NN evaluations are one per distinct OCR or NN target the slots read and one per
   OCR or NN member of each distinct composite they read.
3. Slot `k` becomes candidate `{layout_id}#{k:02}` with `instance_index` `k`, `actionable`
   `true`, and the slot's `rect` and `click`.
4. For each declared feature, in declaration order, that the slot maps to a target, the target
   is evaluated through the frame's scene evaluation, the same one pages use; each distinct
   target is evaluated once per projection, and a template reuses the frame's template result.
   The feature takes the target's `passed` verdict as a boolean or its `measure_milli` as an
   integer, with the target's `confidence`, as the table in selection-graph.md defines.
5. A measure or confidence the backend does not give is absent: a `measure_milli` feature of
   an OCR result without a confidence, or of an NN result without a selected score, is left out
   of the candidate, and a `passed` feature then carries `confidence` `null`. Nothing is
   defaulted.
6. Any evaluation error fails the whole projection: `candidate_feature_provider_missing` when
   the target, or a member of a composite target, needs an OCR or NN provider the evaluator was
   built without, and `candidate_feature_failed` otherwise, including a measure that has no
   finite integer milli value. The detail names the target and the recognition error code, and
   the recognition error, with its PP-OCR reports and region evidence, travels with the
   failure. No partial projection is returned.
7. `CandidateProjection::new` checks the candidates, seals the hash and checks the byte budget.

## Consumers

- The task ledger's `task.selection_evaluated` record embeds the full core projection of the
  step's frame; see [selection-graph.md](selection-graph.md), section Records.
- Online and offline observation outputs and Lab artifacts carry the public form.
- `actinglab do <layout_id>#<NN> --projection-hash <H>` names a candidate and the hash of the
  set it was read from.
