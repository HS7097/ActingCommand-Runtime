# Color digest `color_digest.v1`

`color_digest.v1` is a deterministic digest of the color layout of one declared rectangle
of a frame. A resource declares the digest it expects; evaluation computes the observed
digest of the same rectangle and compares the two. The digest verifies a known place and
never locates anything. It carries no game identity. Every step uses integer arithmetic,
so the same pixels always give the same digest, distance and verdict.
`actingcommand_recognition::color_digest` implements this section.

This document freezes the algorithm. The package declaration (source and derived JSON),
admission pointers, adoption rule, authoring path and diagnostic records are frozen by the
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
