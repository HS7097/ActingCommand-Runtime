# Session Record Drift Amend Contract

`session record amend --from-drift-diagnostics <path>` consumes synthetic or recorded resource-drift diagnostics and updates only the recorded resource that caused the drift stop-loss.

## Input Schema

The diagnostics JSON must include:

- `trigger: "resource_drift"`
- `target_id`: recognition target id, such as `page/home`
- `measured.matched_rect`: the observed template match rectangle, with `x`, `y`, `width`, and `height`

Optional `proposed_changes` may include only:

- `region`: replacement rectangle, either as `{ "x", "y", "width", "height" }` or `{ "mode": "rect", "rect": { ... } }`
- `threshold`: finite number from `0.0` through `1.0`

Any other proposed change field is rejected. Operation fields, click fields, ids, artifact paths, frame provenance, and destructive flags are not accepted through drift diagnostics.

## Target Resolution

The command locates an existing `anchor` or `verify_template` record step whose resource id matches `target_id` directly or through the `page/<anchor_id>` target-id convention.

If `--step-id` or one positional selector is provided, it must select the same step. Ambiguous or missing matches are fatal.

## Amend Behavior

- The selected step region is replaced from `proposed_changes.region` when present, otherwise from `measured.matched_rect`.
- The selected step threshold is replaced only when `proposed_changes.threshold` is present.
- The step artifact and evaluation are refreshed through the existing frame-backed amend path when frame provenance is available.
- The session record is updated atomically after the amend succeeds.
- `session record build-task` remains the authority for materializing the amended bundle.
