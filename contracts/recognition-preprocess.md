# Recognition preprocessing

Workflow #411 A58 and #428 (design CF, slice CFL-1, since 0.12.29). A recognition target may
declare one fixed chain of steps that turns the RGB pixels of its region into a gray or binary
region before the recognition reads it: keep only the text's colors, or stretch, threshold and
invert its gray. This document freezes the chain: its syntax, the arithmetic of every step and
the region size it is proven for, the evidence of one application, the refusals, golden values
and author examples. `actingcommand_recognition::preprocess` implements it.

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

- Two chains are the same exactly when their steps are equal, member by member. The colors of
  `keep_colors` compare in their declared order: the same colors in another order keep the same
  pixels, but make a different chain. To declare the same chain twice, copy it verbatim.
- A region that needs two treatments is declared as two targets with the same region, each with
  its own chain.
- A target has one chain and is read once with it. There are no fallback chains and no second
  read with another chain. A later member of `preprocess` beside `steps` may add them without
  changing `steps`.
- The limits (4 steps, 8 colors) belong to the declaration format and are not configuration.

## Arithmetic

The input is the RGB8 pixels of the region, at least one pixel and at most 2^24 (see "The region
domain and Otsu's bound"); the output has one 8-bit value per pixel and the size of the region. A
kept pixel becomes **255**, every other pixel **0**, and `invert` flips them. Every step except
`gray` is integer arithmetic. The version of this arithmetic is `preprocess.v1`
(`PREPROCESS_ALGORITHM`); a change to the output bytes of any step raises it, with a row in the
register.

- **`keep_colors`**: a pixel is kept when, for at least one listed color `c`,
  `max(|R - c.r|, |G - c.g|, |B - c.b|) <= tolerance` (the per-channel maximum difference).
- **`keep_range`, `rgb`**: a pixel is kept when `lower.k <= k <= upper.k` for each of R, G and B.
- **`keep_range`, `hsv`**: the pixel is converted to HSV on the 8-bit scale of OpenCV's
  documentation ("Color conversions", RGB to HSV: for 8-bit images H is the angle halved, 0 to
  179; S and V 0 to 255), in integers:

  ```text
  V = max(R, G, B)    m = min(R, G, B)    d = V - m
  S = 0 when V = 0, else floor((2 * 255 * d + V) / (2 * V))
  H = 0 when d = 0, else floor((2 * (30 * n + base * d) + d) / (2 * d)) mod 180
      with (n, base) = (G - B, 0) when V = R,
                       else (B - R, 60) when V = G,
                       else (R - G, 120)
  ```

  `floor` rounds toward negative infinity and `mod` gives 0 to 179, so a hue just below 360
  degrees becomes 0, and one a little further below becomes 179: `(255, 0, 8)`, 358.12 degrees
  (179.06 on the scale), gives `floor(-225 / 510) = -1` and so 179, where a truncating division
  would give 0. The pixel is kept when `lower.s <= S <= upper.s`, `lower.v <= V <= upper.v` and
  its hue is in range: `lower.h <= H <= upper.h` when `lower.h <= upper.h`, otherwise
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
  - The value is the same on every run of one build. With the default target features (no
    `fma`), which the Runtime and the Tools are built with, the conversion is plain `f32`
    multiplication and addition, without fused multiply-add. A build that enables `fma` changes
    the conversion of the scene and of the chain alike, because both use the same conversion, so
    they agree with or without it.
- **`contrast`**: `0` when `v <= low`, `255` when `v >= high`, otherwise
  `floor((2 * 255 * (v - low) + (high - low)) / (2 * (high - low)))`.
- **`threshold`**, a value: `255` when `v >= value`, otherwise `0`.
- **`threshold`, `"otsu"`**: Otsu's threshold of the region's values at this step, computed
  exactly in unsigned 128-bit integers, never in floating point ("Otsu's threshold" below). The
  step then binarizes as a threshold value does; when every value is the same, the whole region
  becomes 0 and there is no threshold.
- **`invert`**: `255 - v`.

The chain is a pure function of the region's pixels and the declaration: no clock, no state, no
cache. The same region and chain give the same bytes and the same evidence every time, so every
reader of a frame (the Runtime, the Lab, offline recognition) gives the same result.

### Otsu's threshold

Let `h` be the 256-level histogram of the region's values at this step, `N = Σ h[v]` the number
of values and `S = Σ v * h[v]` their sum. The step computes exactly this:

```text
best = none
n0 = 0;  s0 = 0
for t in 0 ..= 254:
    n0 = n0 + h[t];  s0 = s0 + t * h[t];  n1 = N - n0
    if n0 == 0 or n1 == 0: next t
    a   = |s0 * N - S * n0|
    num = a * a                 d = n0 * n1
    q   = num div d             r = num mod d
    better when: best is none, or q > best.q, or (q == best.q and r * best.d > best.r * d)
    if better: best = (t, q, r, d)
threshold = best.t + 1
when best is none (every value the same): the whole region becomes 0, there is no threshold
```

- **What is compared.** With `s1 = S - s0` and the class means `μ0 = s0 / n0`, `μ1 = s1 / n1`,
  `s0 * N - S * n0 = s0 * n1 - s1 * n0 = n0 * n1 * (μ0 - μ1)`. So `num / d = n0 * n1 * (μ0 - μ1)^2`,
  the between-class variance times `N^2`.
- **Why the comparison is exact.** `num / d = q + r / d` with `0 <= r / d < 1`, so comparing `q`
  first and `r / d` second compares the two rationals `num / d`. Both denominators are positive,
  so `r / d` against `best.r / best.d` is `r * best.d` against `best.r * d`. Nothing is rounded.
- **Ties.** The comparison is strict and `t` rises from 0, so of equal scores the smallest `t`
  wins (the golden `0 100 200` below).
- **The remainder can decide.** Two splits may share the quotient: the golden `0 0 0 1 1 1 2 4`
  below has `q = 75` at `t = 1` (`900 / 12`) and at `t = 2` (`529 / 7`); the remainders `0 / 12`
  and `4 / 7` decide for `t = 2`. A comparison of the quotients alone would give 2, not 3.

### The region domain and Otsu's bound

A region has at most **2^24 = 16 777 216 pixels** (4096 x 4096, which holds 3840 x 2160), the
same limit as the OCR of a saved frame.
- `Preprocess::apply` checks the size before any step runs (`check_region_size`, for every chain,
  not only Otsu's) and refuses a larger region with `PreprocessApplyError::RegionTooLarge
  {width, height}`, an empty one with `EmptyRegion`. It never truncates, wraps or approximates.
- `Scene::preprocess_region` makes the same check before it crops the region, and returns a
  refusal as a fatal recognition error with its text.
- The limit belongs to the algorithm, like the 4 steps and the 8 colors, and is no
  configuration.

Within the domain, every intermediate of the algorithm above fits in unsigned 128-bit integers,
the widest with 20 bits to spare. With `N <= 2^24`:

| Intermediate | Bound | Bits at most |
|---|---|---|
| `h[v]`, `n0`, `n1`, `N` | `<= 2^24` | 25 |
| `S`, `s0`, and `s1` | `<= 255 * N` | 32 |
| `s0 * N`, `S * n0` | `<= 255 * N^2` | 56 |
| `a` | `<= 255 * n0 * n1 <= 255 * N^2 / 4`, by the identity above, since `abs(μ0 - μ1) <= 255` | 54 |
| `num = a^2` | `<= 255^2 * N^4 / 16` | 108 |
| `d = n0 * n1` | `<= N^2 / 4 = 2^46` | 47 |
| `q` | `<= num / d <= 255^2 * n0 * n1` | 62 |
| `r` | `< d <= 2^46` | 46 |
| `r * best.d`, `best.r * d` | `< 2^46 * 2^46 = 2^92` | 92 |

- A direct cross-multiplication of two scores, `num * best.d`, would not fit: it needs 129 bits
  on the 1280 x 720 golden below and 154 bits on the 2^24-pixel one. That is why the comparison
  takes the quotient first and the remainder second.
- The products are still checked. Inside the domain the check cannot fail; outside it, the size
  check has already refused the region.

## Evidence

One application gives, beside the processed region:

| Member | Value |
|---|---|
| `kept_milli` | the share of pixels of at least 128 **before** `invert`, in thousandths, rounded down: `floor(1000 * count / N)`. After a color keep or a threshold it is the kept share; in a gray chain without a threshold, the share of light pixels |
| `inverted` | whether the chain has `invert` (always its last step) |
| `threshold_value` | the threshold the `threshold` step used (Otsu's as computed); `null` without a `threshold` step and for Otsu on a region of one value |

- From these a resource author sees whether a filter kept nothing (`kept_milli` near 0) or
  everything (near 1000).
- Because `kept_milli` is counted before `invert`, a chain with and without `invert` reports the
  same share, and `inverted` tells "kept 18%, then turned into dark text on white" apart from
  "kept 82%".
- The evidence goes to read-only and Lab output (CFL-2, CFL-3), never to the ledger.
- The processed image is not stored. It is a function of the frame, the chain and the region,
  and the chain is given with the evidence.

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
- A region outside the domain is not a refusal of the declaration and has no pointer:
  `Preprocess::apply` refuses it with `PreprocessApplyError` (`EmptyRegion`, `RegionTooLarge`;
  "The region domain and Otsu's bound").

## Golden values

`actingcommand-recognition`'s tests pin these bytes. A region is 4x2, row-major.

**Colored region:** `(255, 255, 255)`, `(240, 240, 232)`, `(250, 30, 20)`, `(20, 200, 40)`,
`(30, 60, 220)`, `(128, 128, 128)`, `(100, 200, 50)`, `(0, 0, 0)`.

| Chain | Output | `kept_milli` | `inverted` | `threshold_value` |
|---|---|---|---|---|
| `keep_colors` `[[255,255,255],[120,128,136]]`, tolerance 23 | `255 255 0 0 0 255 0 0` | 375 | false | null |
| the same, tolerance 22 | `255 0 0 0 0 255 0 0` | 250 | false | null |
| `keep_range` `rgb` `[200,0,0]` to `[255,100,100]` | `0 0 255 0 0 0 0 0` | 125 | false | null |
| `keep_range` `hsv` `[0,0,190]` to `[179,60,255]` | `255 255 0 0 0 0 0 0` | 250 | false | null |
| the same, then `invert` | `0 0 255 255 255 255 255 255` | 250 | true | null |
| `gray` | `255 239 76 150 65 128 168 0` | 625 | false | null |

Their HSV values: `[0,0,255]`, `[30,9,240]`, `[1,235,250]`, `[63,230,200]`, `[115,220,220]`,
`[0,0,128]`, `[50,191,200]`, `[0,0,0]`. Further hues: `(0, 255, 0)` 60, `(0, 0, 255)` 120,
`(255, 255, 0)` 30, `(0, 255, 255)` 90, `(255, 0, 255)` 150, `(255, 0, 1)` 0, `(255, 0, 8)` 179.

**Neutral region** (R = G = B): `0 50 51 100 125 199 200 255`.

| Chain | Output | `kept_milli` | `inverted` | `threshold_value` |
|---|---|---|---|---|
| `gray` | `0 50 51 100 125 199 200 255` | 375 | false | null |
| `gray`, `contrast` 50 to 200 | `0 0 2 85 128 253 255 255` | 500 | false | null |
| `gray`, `threshold` 128 | `0 0 0 0 0 255 255 255` | 375 | false | 128 |
| `gray`, `invert` | `255 205 204 155 130 56 55 0` | 375 | true | null |
| `gray`, `contrast` 50 to 200, `threshold` 128, `invert` | `255 255 255 255 0 0 0 0` | 500 | true | 128 |

**Hue wrap:** the pixels `(255,0,0)`, `(255,0,40)`, `(255,40,0)`, `(255,128,0)`, `(0,0,255)`,
`(255,0,128)`, `(200,180,180)`, `(128,0,0)` have the hues 0, 175, 5, 15, 120, 165, 0, 0 (the pale
one saturation 26). `keep_range` `hsv` `[170,100,100]` to `[10,255,255]` gives
`255 255 255 0 0 0 0 255`; `[11,100,100]` to `[169,255,255]` gives `0 0 0 255 255 255 0 0`.

**Otsu** (`gray`, `threshold` `"otsu"`, on neutral regions; `inverted` false):

| Values | Output | `threshold_value` |
|---|---|---|
| `50 55 60 65 70 75 150 160 170 180 190 200 210 220 230 240` | six 0, then ten 255 (`kept_milli` 625) | 76 |
| `0 100 200` (both splits score the same; the smallest `t`, 0, wins) | `0 255 255` (`kept_milli` 666) | 1 |
| six times `77` | six 0 (`kept_milli` 0) | null |
| `0 0 0 1 1 1 2 4` (`t = 1` and `t = 2` share the quotient 75; the remainder decides for `t = 2`) | `0 0 0 0 0 0 0 255` (`kept_milli` 125) | 3 |
| 1280 x 720: `0`, `128` and `255`, 460 000, 1 600 and 460 000 times | `0` stays 0, the rest 255 (`kept_milli` 500) | 1 |
| 4096 x 4096 (2^24 pixels): half `0`, half `255` | unchanged (`kept_milli` 500) | 1 |

**Region domain:** 4096 x 4096, 16 777 216 x 1 and 1 x 1 are accepted; 4097 x 4096,
16 777 217 x 1 and 4294967295 x 4294967295 are refused with `RegionTooLarge`, and 0 x 5 with
`EmptyRegion`.

## Author examples

These are the `preprocess` members of OCR targets in a `task.json` (accepted from CFL-2 on). A
color keep keeps the text white and inverts it by default, so the reader sees dark text on a light
ground. Copy one, then compare the variants in the Lab's preview (CFL-3): `kept_milli` and
`inverted`, and the confidence with and without `invert`.

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

A slot's OCR target takes its text from this read only when it declares the same chain, colors in
the same order; a target without it, or with another chain, reads on its own. A shared read does
not accept `otsu` (CFL-2): Otsu's threshold depends on the whole region, so a slot's part of the
shared result would differ from what the slot gives when it reads on its own. Every other step
works pixel by pixel.

## Sources

The only outside convention here is the 8-bit HSV scale, as OpenCV's documentation states it
("Color conversions", RGB to HSV). The steps, their arithmetic, the bound and every golden value
are this project's own, computed independently of the implementation.
