# Recognition preprocessing

Workflow #411 A58 and #428 (design CF, slice CFL-1, since 0.12.29). A recognition target may
declare one fixed chain of steps that turns the RGB pixels of its region into a gray or binary
region before the recognition reads it: keep only the text's colors, or stretch, threshold and
invert its gray. This document freezes the chain: its syntax, the arithmetic of every step, the
evidence of one application, the refusals, golden values and author examples.
`actingcommand_recognition::preprocess` implements it.

Where a chain is declared and how the processed region reaches the reader are not part of this
document. Slice CFL-2 adds the optional member `preprocess` to OCR targets (task schema `0.9`
`ocr_targets[]`, recognition pack schema `0.7` `ocr` targets) and to a candidate layout's
`shared_ocr`, with its admission pointers, in [resource-declarations.md](resource-declarations.md);
slice CFL-3 shows the evidence and a preview of the processed region in the Lab. Until CFL-2, a
package that declares `preprocess` is refused like any unknown member. This document adds no
configuration key, no outcome code, no ledger revision and no package revision.

## Chain

```json
"preprocess": {"steps": [
  {"kind": "keep_colors", "colors": [[255, 255, 255]], "tolerance": 40},
  {"kind": "invert"}
]}
```

`preprocess` is an object whose only member is `steps`: an array of 1 to 4 steps. A step is an
object with `kind` and the members of its kind. Every member is required, and none has a default.

| `kind` | Members | Input to output |
|---|---|---|
| `keep_colors` | `colors`: 1 to 8 distinct `[r, g, b]`, each 0 to 255; `tolerance`: an integer 0 to 255 | RGB to binary |
| `keep_range` | `space`: `"rgb"` or `"hsv"`; `lower`, `upper`: three integers each. In RGB each is 0 to 255; in HSV H is 0 to 179, S and V 0 to 255. `lower <= upper` on every component, except the HSV hue: a `lower` hue above the `upper` hue wraps through 0 | RGB to binary |
| `gray` | none | RGB to gray |
| `contrast` | `low`, `high`: `0 <= low < high <= 255` | gray to gray |
| `threshold` | `value`: an integer 1 to 255, or the string `"otsu"` | gray to binary |
| `invert` | none | gray or binary to the same |

### Shapes

A chain has one of two shapes. Its steps come in this order only, and each kind appears at most
once:

- **Color keep:** `keep_colors` or `keep_range`, then optionally `invert`.
- **Gray:** `gray`, then optionally `contrast`, `threshold` and `invert`, in that order.

The legal chains are therefore exactly these twelve:

```text
[keep_colors]   [keep_colors, invert]   [keep_range]   [keep_range, invert]
[gray]   [gray, contrast]   [gray, threshold]   [gray, invert]
[gray, contrast, threshold]   [gray, contrast, invert]   [gray, threshold, invert]
[gray, contrast, threshold, invert]
```

- A chain has only one spelling for what it does, so two chains are the same exactly when their
  steps are equal, member by member.
- A region that needs two treatments is declared as two targets with the same region, each with
  its own chain.
- A target has one chain and is read once with it. There are no fallback chains and no second
  read with another chain. A later member of `preprocess` beside `steps` may add them without
  changing `steps`.
- The limits (4 steps, 8 colors) belong to the declaration format and are not configuration.

## Arithmetic

The input is the RGB8 pixels of the region, at least one pixel; the output has one 8-bit value
per pixel and the size of the region. A kept pixel becomes **255**, every other pixel **0**, and
`invert` flips them. Every step except `gray` is integer arithmetic.

- **`keep_colors`**: a pixel is kept when, for at least one listed color `c`,
  `max(|R - c.r|, |G - c.g|, |B - c.b|) <= tolerance` (the per-channel maximum difference).
- **`keep_range`, `rgb`**: a pixel is kept when `lower.k <= k <= upper.k` for each of R, G and B.
- **`keep_range`, `hsv`**: the pixel is converted to HSV on the OpenCV 8-bit scale (H 0 to 179,
  S and V 0 to 255), in integers:

  ```text
  V = max(R, G, B)    m = min(R, G, B)    d = V - m
  S = 0 when V = 0, else floor((2 * 255 * d + V) / (2 * V))
  H = 0 when d = 0, else floor((2 * (30 * n + base * d) + d) / (2 * d)) mod 180
      with (n, base) = (G - B, 0) when V = R,
                       else (B - R, 60) when V = G,
                       else (R - G, 120)
  ```

  `floor` rounds toward negative infinity and `mod` gives 0 to 179, so a hue just below 360
  degrees becomes 0. The pixel is kept when `lower.s <= S <= upper.s`, `lower.v <= V <= upper.v`
  and its hue is in range: `lower.h <= H <= upper.h` when `lower.h <= upper.h`, otherwise
  `H >= lower.h` or `H <= upper.h`. A component may differ by one from OpenCV's floating-point
  conversion; calibration in the Lab absorbs that.
- **`gray`**: the gray every template search reads, the scene's (`Scene`, built from RGB8 or RGBA8
  pixels or from an 8-bit PNG): image 0.25's `to_luma8` on the region's own pixels,

  ```text
  Y = round(255 * (kr * R/255 + kg * G/255 + kb * B/255))
  ```

  in `f32`, rounded half away from zero, with `kr`, `kg`, `kb` the sRGB (BT.709, D65) luminance
  row the image crate derives, about 0.21265, 0.71518 and 0.07217. A neutral pixel `(v, v, v)`
  gives `v`.
  - So a `[gray]` chain gives the scene's gray byte for byte, and a chain that starts with `gray`
    starts from the very values every template search scores.
  - This is not `floor((2126 R + 7152 G + 722 B) / 10000)`, the formula the CF design assumed for
    the scene's gray: image 0.25.10 rounds a floating-point sum, and the two differ on about half
    of all colors (`(100, 200, 50)` gives 168, the integer formula 167). The chain follows the
    scene. Changing the scene's gray would change every template score, and is not part of this
    slice.
  - The value is the same on every run of one build: plain `f32` operations, without fused
    multiply-add on the targets the Runtime and the Tools are built for.
- **`contrast`**: `0` when `v <= low`, `255` when `v >= high`, otherwise
  `floor((2 * 255 * (v - low) + (high - low)) / (2 * (high - low)))`.
- **`threshold`**, a value: `255` when `v >= value`, otherwise `0`.
- **`threshold`, `"otsu"`**: Otsu's threshold of the region's values at this step.
  - Over the region's 256-level histogram, for each `t` from 0 to 254 whose split
    (`v <= t` against `v > t`) leaves both classes non-empty: with `N` values summing to `S`,
    and `n0`, `s0` the count and sum of the low class (`n1 = N - n0`),

    ```text
    a     = |s0 * N - S * n0|
    score = a^2 / (n0 * n1)      (the between-class variance times N^2)
    ```

  - The scores are compared exactly in unsigned 128-bit integers, by quotient and then by
    remainder (`r1 * d2` against `r2 * d1`), never in floating point. Should a product overflow,
    which only a region of more than 2^29 pixels can cause, the step fails with an error; it is
    never approximated.
  - The smallest `t` with the greatest score wins; the threshold is `t + 1`, and the step then
    binarizes as a threshold value does.
  - When no split has two non-empty classes (every value the same), the whole region becomes 0
    and there is no threshold.
- **`invert`**: `255 - v`.

The chain is a pure function of the region's pixels and the declaration: no clock, no state, no
cache. The same region and chain give the same bytes and the same evidence every time, so every
reader of a frame (the Runtime, the Lab, offline recognition) gives the same result.

## Evidence

One application gives, beside the processed region:

| Member | Value |
|---|---|
| `foreground_milli` | the share of processed pixels of at least 128, in thousandths, rounded down: `floor(1000 * count / N)` |
| `threshold_value` | the threshold the `threshold` step used (Otsu's as computed); `null` without a `threshold` step and for Otsu on a region of one value |

From these two numbers a resource author sees whether a filter kept nothing (`foreground_milli`
near 0) or everything (near 1000). They go to read-only and Lab output (CFL-2, CFL-3), never to
the ledger. The processed image is not stored: it is a function of the frame and the sealed
package.

## Refusals

`Preprocess::new` checks a chain in this order and refuses it at the first failure: the step
count; then step by step, the step's place in the shape (first step, repetition, order) and then
its members. A refusal is a `PreprocessError` with a reason, the step's index and the member;
`pointer()` gives its JSON pointer below the `preprocess` object.

| Reason | Pointer below `preprocess` | Refused when |
|---|---|---|
| `NoSteps` | `/steps` | the chain has no step |
| `TooManySteps` | `/steps` | the chain has more than 4 steps |
| `FirstStepNotAllowed` | `/steps/0/kind` | the first step is not `keep_colors`, `keep_range` or `gray` |
| `StepRepeated` | `/steps/<k>/kind` | the kind already appeared earlier in the chain |
| `StepOutOfOrder` | `/steps/<k>/kind` | the shape does not allow this kind after the step before it (a first-step kind later in the chain, `contrast` or `threshold` after a color keep, or the gray shape's order reversed) |
| `ColorCount` | `/steps/<k>/colors` | no color, or more than 8 |
| `ColorRepeated` | `/steps/<k>/colors` | a color repeats an earlier one |
| `HueOutOfRange` | `/steps/<k>/lower`, `/steps/<k>/upper` | an HSV hue bound above 179 |
| `BoundsReversed` | `/steps/<k>/upper` | `lower` above `upper` on a component (RGB: any; HSV: S or V) |
| `ContrastBoundsReversed` | `/steps/<k>/high` | `low >= high` |
| `ThresholdZero` | `/steps/<k>/value` | a fixed threshold of 0 |

- The declaration's parser (CFL-2) checks the JSON before it builds the chain. A `preprocess`
  that is not an object, a member other than `steps`, a missing or extra step member, a value of
  the wrong type, a number outside 0 to 255, or an unknown `kind` or `space` name is its own
  refusal (`UnknownField`, `MissingField`, `InvalidType`, `InvalidValue`) at the member's pointer.
  The names come from `PreprocessKind::from_name`, `PreprocessKind::members` and
  `ColorSpace::from_name`.
- CFL-2 reports every refusal of the table as `resource_declaration_invalid` with the reason
  `InvalidValue`, at the target's pointer followed by `/preprocess` and the pointer above.

## Golden values

`actingcommand-recognition`'s tests pin these bytes. A region is 4x2, row-major.

**Colored region:** `(255, 255, 255)`, `(240, 240, 232)`, `(250, 30, 20)`, `(20, 200, 40)`,
`(30, 60, 220)`, `(128, 128, 128)`, `(100, 200, 50)`, `(0, 0, 0)`.

| Chain | Output | `foreground_milli` | `threshold_value` |
|---|---|---|---|
| `keep_colors` `[[255,255,255],[120,128,136]]`, tolerance 23 | `255 255 0 0 0 255 0 0` | 375 | null |
| the same, tolerance 22 | `255 0 0 0 0 255 0 0` | 250 | null |
| `keep_range` `rgb` `[200,0,0]` to `[255,100,100]` | `0 0 255 0 0 0 0 0` | 125 | null |
| `keep_range` `hsv` `[0,0,190]` to `[179,60,255]` | `255 255 0 0 0 0 0 0` | 250 | null |
| the same, then `invert` | `0 0 255 255 255 255 255 255` | 750 | null |
| `gray` | `255 239 76 150 65 128 168 0` | 625 | null |

Their HSV values: `[0,0,255]`, `[30,9,240]`, `[1,235,250]`, `[63,230,200]`, `[115,220,220]`,
`[0,0,128]`, `[50,191,200]`, `[0,0,0]`. Further hues: `(0, 255, 0)` 60, `(0, 0, 255)` 120,
`(255, 255, 0)` 30, `(0, 255, 255)` 90, `(255, 0, 255)` 150, `(255, 0, 1)` 0.

**Neutral region** (R = G = B): `0 50 51 100 125 199 200 255`.

| Chain | Output | `foreground_milli` | `threshold_value` |
|---|---|---|---|
| `gray` | `0 50 51 100 125 199 200 255` | 375 | null |
| `gray`, `contrast` 50 to 200 | `0 0 2 85 128 253 255 255` | 500 | null |
| `gray`, `threshold` 128 | `0 0 0 0 0 255 255 255` | 375 | 128 |
| `gray`, `invert` | `255 205 204 155 130 56 55 0` | 625 | null |
| `gray`, `contrast` 50 to 200, `threshold` 128, `invert` | `255 255 255 255 0 0 0 0` | 500 | 128 |

**Hue wrap:** the pixels `(255,0,0)`, `(255,0,40)`, `(255,40,0)`, `(255,128,0)`, `(0,0,255)`,
`(255,0,128)`, `(200,180,180)`, `(128,0,0)` have the hues 0, 175, 5, 15, 120, 165, 0, 0 (the pale
one saturation 26). `keep_range` `hsv` `[170,100,100]` to `[10,255,255]` gives
`255 255 255 0 0 0 0 255`; `[11,100,100]` to `[169,255,255]` gives `0 0 0 255 255 255 0 0`.

**Otsu** (`gray`, `threshold` `"otsu"`, on neutral regions):

| Values | Output | `threshold_value` |
|---|---|---|
| `50 55 60 65 70 75 150 160 170 180 190 200 210 220 230 240` | six 0, then ten 255 (`foreground_milli` 625) | 76 |
| `0 100 200` (both splits score the same; the smallest `t`, 0, wins) | `0 255 255` (`foreground_milli` 666) | 1 |
| six times `77` | six 0 (`foreground_milli` 0) | null |

## Author examples

These are the `preprocess` members of OCR targets in a `task.json` (accepted from CFL-2 on). A
color keep keeps the text white and inverts it by default, so the reader sees dark text on a light
ground. Copy one, then compare the variants in the Lab's preview (CFL-3): `foreground_milli`, and
the confidence with and without `invert`.

**White digits on a colored icon:**

```json
{
  "id": "ocr/item_count",
  "region": {"mode": "template_relative", "anchor_target_id": "icon/item",
             "offset": {"x": 40, "y": 52}, "width": 56, "height": 24},
  "languages": ["en"], "timeout_ms": 1000,
  "match_mode": "exact", "expected": ["0"], "case_sensitive": false,
  "minimum_confidence": 0.9,
  "model_ref": "<model>", "model_sha256": "<64 lowercase hex digits>",
  "preprocess": {"steps": [
    {"kind": "keep_colors", "colors": [[255, 255, 255], [240, 240, 232]], "tolerance": 40},
    {"kind": "invert"}
  ]}
}
```

Only the near-white strokes stay; everything else becomes black, then the region is inverted to
dark digits on white. Readings, fields, value tests and candidate features name
`"ocr/item_count"` as before.

**A light name on a colored gradient bar:**

```json
"preprocess": {"steps": [
  {"kind": "keep_range", "space": "hsv", "lower": [0, 0, 190], "upper": [179, 60, 255]},
  {"kind": "invert"}
]}
```

Low saturation and high value keep white and light gray strokes whatever the bar's hue.

**Light text on a dark ground, by gray level:**

```json
"preprocess": {"steps": [{"kind": "gray"}, {"kind": "threshold", "value": "otsu"}, {"kind": "invert"}]}
```

For dark text on a light ground, leave out `invert`: the threshold already gives dark text on
white.

**One shared read over a grid of cells** (a candidate layout's `shared_ocr`, CFL-2):

```json
"shared_ocr": {"region": {"x": 40, "y": 120, "width": 1200, "height": 480},
               "languages": ["en"], "timeout_ms": 5000,
               "model_ref": "<model>", "model_sha256": "<64 hex digits>",
               "blocks": "detect", "line_merge": {"gap_factor": 0},
               "preprocess": {"steps": [
                 {"kind": "keep_colors", "colors": [[255, 255, 255]], "tolerance": 40},
                 {"kind": "invert"}
               ]}}
```

A slot's OCR target takes its text from this read only when it declares the same chain; a target
without it, or with another chain, reads on its own.
