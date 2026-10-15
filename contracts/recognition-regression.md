# Frame-replay recognition regression

Workflow #331 RSU-1 (additions-v012 §1.5b row RSU-1; forced-compromises mapping T-01).

`actinglab --json recognition-regression --state-root <Runtime state root> --evidence <file> [--evidence <file> ...] (--package <path> | --zip <published .zip>)`
replays the frames a run's ledger kept with the recognition of a new package, or a new version
of one. It compares each frame with the run's original record.

It is an offline package tool (option one), like `replay`. It opens the state root's ledger and
artifact store read-only, connects to no Runtime, touches no device and writes nothing. It is
not wired into any repository's CI. Whether it becomes a required gate is an open policy
question (additions-v012 question 5).

## Input

- `--evidence` is the run's evidence: the answer of `actingctl run evidence` or MCP
  `ac_run_evidence` (Workflow #389 MCP-6), saved unchanged. Several pages of one run may be given,
  one `--evidence` each; pages of different runs are refused.
- Only these members are read: `request_id`, and each step's `step_index` and `frames[].ref`.
  Every other member is ignored, including the CLI line's `outcome`.
- A frame that several steps reference is replayed once, with every step that names it.
- Each reference (`<sequence>:<event_id>:<artifact_id>`) must name a verified capture frame of
  the run's request in the ledger of `--state-root`. Otherwise the command fails with
  `recognition_regression_frame_unknown`.
- The package is read by its path, as the other offline commands read it. Its recognition pack
  gives the targets.

## The original record

A frame's original record is the last recognition fact (`task.recognition_completed`) that the
run recorded on that frame: each target with whether it passed.

A frame is listed, but not compared, in two cases:
- the run recorded no recognition on it (`no_original_record`);
- the retention rules removed its material (`material_missing`, Workflow #375).

## Comparison

Each target of a frame's original record is evaluated on that frame with the package:

- `compared`: the result has `matched` (whether it passed now), `agrees` (whether that equals
  the original record) and, when its kind has one, `score`:
  - a template: `score_milli` and `threshold_milli`;
  - a colour: `distance_milli` and `max_distance_milli`;
  - an OCR read: `confidence_milli`;
  - a classifier: the selected `score_milli`.
- `absent`: the package declares no target of that id.
- `failed`: the evaluation failed. `failure` holds the error.

Without an OCR model, a comparison that reaches an OCR target is refused with
`lab_ocr_unavailable` before any frame is evaluated.

## Output

- `frames`: every referenced frame, in the evidence's order, with its `ref`, `steps`,
  `frame_id`, `status` (`compared`, `material_missing`, `no_original_record`) and its
  `targets`.
- `targets`: one tally per target id. A tally has:
  - `frames`: the frames whose original record names the target;
  - `compared`, `absent`, `failed`;
  - `original_hits` and `hits`;
  - `lost` (passed in the original record, not now) and `gained` (the reverse);
  - `disagreements`, which is `lost` plus `gained`;
  - `error_rate_milli`: the disagreements per mille of the frames compared, rounded down; `null`
    when none was compared.
- `summary`: the same counts over every target entry, with `records` in place of `frames`.
  It also has `frames_referenced`, `frames_replayed`, `frames_material_missing` and
  `frames_without_original_record`.
- `mode` is `offline_recognition_regression`, with `executed: false`. The output also carries
  `request_id`, the package's `package_id`, the `ocr` status and
  `production_global_ledger_written: false`.

The command reports and decides nothing. It exits 0 whenever the comparison was made, whatever
the error rate.

## Errors

The command fails, and compares nothing, in these cases:

| Code | When |
|---|---|
| `recognition_regression_evidence_unreadable` | an evidence file cannot be read |
| `recognition_regression_evidence_invalid` | an evidence file is not a page of a run's evidence, its pages are of different runs, or it names no frame |
| `recognition_regression_ledger_unreadable` | the ledger does not open, is incomplete, or the run's events or an artifact do not read |
| `recognition_regression_frame_unknown` | a reference names no verified capture frame of the run |
| `recognition_regression_frame_invalid` | a referenced frame is not a PNG image, its material does not read for a reason other than its absence, or it does not decode |

The global Runtime, device and execution flags, `--dry-run` included, are refused with
`offline_device_scope_forbidden`.
