# Color digest `color_digest.v1`

`color_digest.v1` is a deterministic digest of the color layout of one declared rectangle
of a frame. A resource declares the digest it expects; evaluation computes the observed
digest of the same rectangle and compares the two. The digest verifies a known place and
never locates anything. It carries no game identity. Every step uses integer arithmetic,
so the same pixels always give the same digest, distance and verdict.
`actingcommand_recognition::color_digest` implements this section.

This document freezes the algorithm and the package declaration (source and derived JSON,
admission pointers). The adoption rule, authoring path and diagnostic records are frozen by
later slices of [Workflow #308](https://github.com/HS7097/ActingCommand-Workflow/issues/308).

## Algorithm

### Input and geometry

- The frame has the size of the package's declared coordinate space; the package layer
  checks this before digesting.
- Pixels are RGB8. An RGBA8 frame contributes its R, G and B channels; alpha is ignored.
- The region of interest (ROI) is an absolute integer rectangle: `x >= 0`, `y >= 0`,
  `width >= 1`, `height >= 1`, lying entirely inside the frame. It is never clipped.
  Template-relative regions are not supported.
- The grid has `C` columns and `R` rows, `1 <= C <= min(32, width)` and
  `1 <= R <= min(32, height)`. Both are always declared; the algorithm has no default grid.

### Cells and quantization

Cell `(c, r)`, for `0 <= c < C` and `0 <= r < R`, covers the half-open pixel ranges
`[x0, x1)` and `[y0, y1)`:

```text
x0 = x + floor(c * width / C)      x1 = x + floor((c + 1) * width / C)
y0 = y + floor(r * height / R)     y1 = y + floor((r + 1) * height / R)
n  = (x1 - x0) * (y1 - y0)
```

The grid bounds give every cell at least one pixel. For each channel `k` in R, G, B,
`S_k` is the unsigned 64-bit sum of that channel over the cell's pixels, and

```text
q_k = floor(S_k / (8 * n))         0 <= q_k <= 31
```

### Encoding

Cell `i = r * C + c` (row-major) contributes the bytes `D[3i..3i+3] = [q_R, q_G, q_B]`.
The digest is written as lowercase hexadecimal: `6 * C * R` digits `0-9a-f`, two per
byte, every byte at most `0x1f`. A different length, an uppercase digit, any other
character, or a byte above `0x1f` is rejected, never normalized.

### Distance

`exclude_cells` optionally lists cell indices `i` to leave out. It is strictly ascending,
every index is below `C * R`, and the active set `A` (all cells minus the excluded ones)
holds at least one cell. The expected and the observed digest use the same grid.

```text
d_i        = |dR| + |dG| + |dB|                      0 <= d_i <= 93
mean_milli = floor(1000 * sum(d_i, i in A) / |A|)
max_cell   = max(d_i, i in A)
worst_cell = the smallest i in A with d_i = max_cell
```

### Decision

```text
passed  <=>  mean_milli <= max_mean_milli
             and (no max_cell threshold, or max_cell <= the max_cell threshold)
```

`max_mean_milli` is always declared and the `max_cell` threshold is optional. Neither has
a default, and neither is inherited from package defaults.

### Errors

Every invalid input fails with an explicit code. Nothing is clipped, defaulted or
reported as a non-match.

| Code | Raised when |
|---|---|
| `color_digest_algorithm_unknown` | the algorithm name is not exactly `color_digest.v1` |
| `color_digest_grid_invalid` | `C` or `R` is outside `1..=32`, or exceeds the ROI width or height |
| `color_digest_region_invalid` | the ROI has a negative origin or a non-positive size, overflows, or leaves the frame |
| `color_digest_cells_invalid` | the hex has the wrong length for the grid, a character other than `0-9a-f`, or a byte above `0x1f` |
| `color_digest_exclude_cells_invalid` | `exclude_cells` is not strictly ascending, names a cell outside the grid, or leaves no active cell |
| `color_digest_grid_mismatch` | the expected and the observed digest use different grids |

## Golden values

ROI 4x2 at `(0, 0)`, grid 2x1 (two cells of 2x2 pixels):

- Left cell: all four pixels are `(200, 100, 50)`. `S = (800, 400, 200)`, `n = 4`,
  cell `[25, 12, 6]`.
- Right cell: the pixels are `(0, 0, 0)`, `(255, 255, 255)`, `(10, 20, 30)` and
  `(40, 50, 60)`. `S = (305, 325, 345)`, `n = 4`, cell `[9, 10, 10]`.
- Digest: `190c06090a0a`.

The observed digest `1a0c06090a0d` compared with it, no cell excluded: `d = (1, 3)`,
`mean_milli = 2000`, `max_cell = 3`, `worst_cell = 1`.

Partition: a width of 10 split into `C = 3` columns gives `[0, 3)`, `[3, 6)` and
`[6, 10)`, column widths 3, 3 and 4.

## Package declaration

### Source

A task source (`task.json`, task schema `0.6` through `0.9`) declares a digest as a
`color_probes` entry that carries `digest` instead of `expected`:

```json
{"id": "digest/menu_bar",
 "region": {"mode": "rect", "rect": {"x": 0, "y": 664, "width": 1280, "height": 56}},
 "digest": {"algorithm": "color_digest.v1", "columns": 8, "rows": 8,
            "cells": "<384 lowercase hex digits>", "exclude_cells": [5, 13],
            "max_mean_milli": 1500, "max_cell": 12}}
```

- `region` is `{"mode": "rect", "rect": {...}}` or `{"mode": "full_frame"}`, the whole
  coordinate space. Template-relative regions are refused.
- `algorithm`, `columns`, `rows`, `cells` and `max_mean_milli` are required; `exclude_cells`
  and `max_cell` are optional. Nothing has a default.
- A color probe declares exactly one of `expected` and `digest`, and a digest entry has no
  `max_distance`. An optional `provenance` object is kept as before.
- The digest ID shares the target namespace (`resource-declarations.md`, section Pack schema
  `0.7` declarations).

### Derived

The parser derives one `color_digest` target into `pack.json`, which is then written at
schema `0.7` (`selection-graph.md`, section Pack schema `0.7`):

```json
{"type": "color_digest", "id": "digest/menu_bar",
 "region": {"x": 0, "y": 664, "width": 1280, "height": 56},
 "algorithm": "color_digest.v1", "columns": 8, "rows": 8,
 "cells": "<384 lowercase hex digits>", "exclude_cells": [5, 13],
 "max_mean_milli": 1500, "max_cell": 12}
```

- `region` is always an absolute rectangle; `full_frame` becomes `x = 0`, `y = 0` and the
  coordinate space's width and height.
- `exclude_cells` is always written, empty when the source omits it; `max_cell` only when
  declared.
- The pack declares its `coordinate_space`, and the evaluated frame must have exactly that
  size.

### Admission

The source parser refuses an invalid entry with the task's `task.json` and the JSON pointer
of the field, below `/color_probes/<i>`:

| Pointer | Refused when | Reason |
| --- | --- | --- |
| `/digest` | declared together with `expected`; or the task schema is below `0.6` | `InvalidValue`; `UnconsumedField` |
| `/max_distance` | declared on a digest entry | `UnconsumedField` |
| `/region/mode` | not `rect` or `full_frame` | `InvalidValue` |
| `/region/rect/x`, `/y`, `/width`, `/height` | not an integer; a negative origin; a size below 1 | `InvalidType`; `InvalidValue` |
| `/region/rect` | the rectangle leaves the coordinate space | `InvalidValue` |
| `/digest/algorithm` | not exactly `color_digest.v1` | `InvalidValue` |
| `/digest/columns`, `/digest/rows` | outside `1..=32`, or more than the region's width or height | `InvalidValue` |
| `/digest/cells` | the wrong length for the grid, a character other than `0-9a-f`, or a byte above `0x1f` | `InvalidValue` |
| `/digest/exclude_cells` | not strictly ascending, outside the grid, or no active cell left | `InvalidValue` |
| `/digest/max_mean_milli`, `/digest/max_cell` | not an unsigned 32-bit integer | `InvalidType`; `InvalidValue` |
| `/digest/<field>` | any other field | `UnknownField` |
| `/id` | the ID is already declared by a different target | `InvalidValue` |

A missing required field is `MissingField` at its pointer. Loading the derived pack checks
the target again and fails with the error codes of the algorithm above.
