# Template-relative color checks

A schema `0.6` template target can require a color sampled relative to the current
template candidate. The declaration uses the existing `template_relative` region
form. Its `anchor_target_id` must equal the containing template target's own ID:

```json
{
  "type": "template",
  "id": "page/ready_marker",
  "template_path": "operations/example/assets/marker.png",
  "region": {"x": 20, "y": 40, "width": 300, "height": 400},
  "threshold": 0.95,
  "color_check": {
    "region": {
      "mode": "template_relative",
      "anchor_target_id": "page/ready_marker",
      "offset": {"x": 8, "y": -2},
      "width": 4,
      "height": 3
    },
    "expected": [220, 80, 30]
  }
}
```

Authoring anchors use the same `color_check` object, with the final `page/<anchor>`
ID. Native convert retains the relative declaration and checks its types, positive
dimensions and self-anchor binding. Existing page-to-template color propagation
binds the copied check to the destination template alias's own candidate. Expected
RGB components are bytes; offsets and dimensions are signed 32-bit integers.
Pack admission rejects unknown fields/modes, use on other target kinds, another
anchor, invalid dimensions, or dimensions exceeding the declared coordinate space.

Evaluation searches the declared template ROI in row-major order, using the
existing grayscale metric and exact scoring implementation. A candidate must meet
the template threshold and the color condition together. The color rectangle is
the candidate's top-left plus the declared offset; the entire rectangle must fit
the original frame. Arithmetic overflow or an out-of-frame rectangle disqualifies
that candidate without clipping. Color uses the existing mean RGB and Euclidean
distance predicate with `defaults.color_max_distance`.

The joint candidate with the highest raw template score is selected; equal raw
scores retain the first `(y, x)` position. A lower-scoring candidate with valid color can win over a
higher-scoring candidate with invalid color. The relative path visits the bounded
ROI once with the existing five-second matching deadline, including candidate color
work. It retains only the best template and best joint match. If that search cannot
finish in budget, evaluation fails explicitly and returns no partial success.
Choose an appropriate resource ROI for this exact joint search.

Direct recognition, same-frame page/operation evaluation and template-region batch
evaluation use this rule. With no joint match, `passed` is false and the best
template remains diagnostic evidence. A sampled relative color adds `color.region`
to the shared evaluation, readonly response and existing typed task diagnostic
record. These coordinates and the template coordinates describe the same candidate.
If no candidate reaches the template threshold, or the best candidate's sampling
region cannot be resolved, color is absent and the message gives the reason.
Selected color evidence is retained from the candidate search. Existing guard
admission uses `passed` before input.

Absolute color checks retain their fixed rectangle and existing matching behavior.
The optional diagnostic region is emitted for relative samples; existing absolute
records retain their serialization. External frames, OCR anchor resolution,
ownership, providers and caches retain their contracts. All data belongs to the
current scene evaluation and the existing GlobalLedger diagnostic path.

The four neutral regression scenarios are: a high-scoring gray status row; a lower
scoring valid row that moves; color without a matching template; and declaration /
resolved-region boundaries, including native convert and alias binding. Existing
absolute-color, OCR-region and fail-closed specifications remain in workspace CI.
First-failure and assignment: [Workflow #279, TEMPLATE-MATCH-COLOR-v1](https://github.com/HS7097/ActingCommand-Workflow/issues/279).
