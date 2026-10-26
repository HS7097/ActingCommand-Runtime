# Display adapter

Workflow #392 (with #401), model 392 v1.2. The display adapter sits between a task package and
the capture and input backends: a package writes base coordinates, and the adapter maps them
onto whatever frame the device delivers. This document grows slice by slice:

| Slice | Section | State |
|---|---|---|
| DA-1 | the pure mapping crate `crates/display-adapter` (sections 1 to 7) | this document |
| DA-4 | the game's declaration file `display-layout.json` (`actingcommand.display-layout.v1`), parsed by the contract crate | its own section, later |
| DA-2a | the capture side for the identity and a pure scale: the frame gate, the scale frame, evidence out of the adapter, the run's declaration, rebuilds and the geometry records | section 8 |
| DA-2b | translations and cropped frames (region mapping in recognition), the nearest correction window, the `anchor` member, the refusal of crossing positional boxes | later |
| DA-3 | the input side: every click and drag kind, `display_map_stale`, the crop offset and the turn undone for input | later |
| DA-5 | task schema `0.10` and box references; the Lab | later |

From DA-2a every frame a run recognises passes the crate's map (section 8); a 1280×720 package
on a 1280×720 frame behaves exactly as before, byte for byte.

## 1 Spaces

- **Base**: the pixels of a task package's `coordinate_space`, `Wb × Hb`.
- **Logical display**: the display the frame was captured from (`logical_display_extent`),
  at its current rotation. Backends that take display coordinates (`adb input`) receive these.
- **Device frame**: the frame as delivered, upright (section 2.1). It equals the logical display
  except when a stream crops it (`CaptureFrameTransform::Crop`, #402 RD-3).
- **Scale space**: the layout (section 2.2) divided by the scale factor `s`, `Wc × Hc`. The
  nine anchors and every base region are placed here.
- **Scale frame**: the part of the scale space the device frame covers: the whole scale space
  for an uncropped frame, the crop divided by `s` otherwise. Recognition runs on it.

## 2 The map M

`DisplayMap::build(base, observation, rule)` builds the map of one frame. `observation` holds the
delivered frame's size, the logical display's size, its rotation when known, the crop when a
stream recorded one, and the insets in use (logical-display pixels at the current rotation).
`rule` is the mapping side of the game's declaration, which the contract crate parses (DA-4):
`scale_by` (`height` or `width`), `scale_area` and `anchor_area` (`screen` or `safe_area`);
`None` when the game declares no display layout. A map
holds while the frame size, display, rotation, crop, insets, rule and base hold.

### 2.1 Orientation (the rotation table)

A size is landscape (wider), portrait (taller) or square. A frame agrees with a package when
both have the same orientation or either is square; the ratio itself is never checked.

| Frame against package | Display's rotation | Disposition |
|---|---|---|
| agrees | any | mapped as delivered |
| disagrees | known, and the display agrees with the package | turned upright: clockwise at 90°, counterclockwise at 270° (the turn the device layer gives such frames); the sides swap |
| disagrees | known, and the display disagrees too | this frame cannot be mapped: `display_map_unresolved`, `orientation_mismatch` |
| disagrees | unknown | the host's `contained_task_geometry_unknown` |

A disagreeing frame of an agreeing display at 0° or 180° is left as delivered and then fails the
frame–display check below.

### 2.2 The layout and the frame–display check

- An uncropped frame is its own layout: the upright frame must have its display's aspect ratio,
  else the host's `contained_task_geometry_frame_display_mismatch`.
- A cropped frame is laid out on its logical display: the crop must be exactly the frame's size
  and lie inside the display, else the same refusal.

### 2.3 The scale factor `s`

`s = num / den`, reduced, integer only.

- **Without a rule**, only a pure scale maps: the layout must have the base's aspect ratio and
  no insets may be in use (insets of 0 are not in use); then `s` = layout width / `Wb`.
  Otherwise `layout_undeclared`.
- **With a rule**, `s` is the scale area's height over `Hb` (`by: height`) or its width over
  `Wb` (`by: width`). The screen is the whole layout; the safe area is the layout less the
  insets, and needs insets that were read (else the host's `contained_task_geometry_unavailable`)
  and that leave a width and a height (else `collapsed`).
- `s` below 1 is refused: `scale_below_base`.

### 2.4 Scale space, anchor area, frame area

- `Wc = round_half_up(layout width · den / num)`, `Hc` likewise.
- The anchor area `L` is the scale space less the anchor area's insets, each inset divided by
  `s` to the nearest (none for `screen`).
- The frame area is `(0, 0, Wc, Hc)` for an uncropped frame, and the crop's offset and size,
  each divided by `s` to the nearest, for a cropped one.
- Any of them left without width or height is `collapsed`.

### 2.5 Kinds

| Kind | When |
|---|---|
| `identity` | `s` = 1, the scale space is the base, the anchor area is all of it |
| `scale` | `s` > 1, the scale space is the base, the anchor area is all of it |
| `translate` | anything else (only under a rule) |

## 3 Nine-grid anchors and corners

- Each base coordinate has a zone per axis: `3x < Wb` is the start, `3x > 2 · Wb` the end,
  anything else the middle, both third lines included. The nine anchors (`top_left`, `top`,
  `top_right`, `left`, `center`, `right`, `bottom_left`, `bottom`, `bottom_right`) are the nine
  pairs of zones.
- A coordinate is placed at `L.x + x + room` across (and `L.y + y + room` down), where the room
  `L.width − Wb` counts nothing for the start, half rounded down (`floor`) for the middle, and
  all of it for the end.
- A box maps corner by corner: its top-left corner by the zones of `(x, y)`, its bottom-right
  corner by the zones of its open ends `(x + width, y + height)`. With an explicit anchor the
  whole box moves with that anchor and keeps its size.
- A mapped box with no width or height left is `collapsed`; it is then cut to the scale frame
  (the frame area's offset subtracted), and a box with nothing left is `outside_frame`. A point
  outside the scale frame is `outside_frame` too.

## 4 Region classes

| Class | Examples | Under a translation, without an anchor, across a third line |
|---|---|---|
| search hint | a template's area, a text to locate, a scan's track | mapped corner by corner (it stretches; harmless) |
| positional | a plain text area, a colour probe, a slot, a still-frame region, an input box | refused: `straddles_zones`, no input (coordinator ruling of 2026-10-13, item 1) |

The identity and a pure scale never refuse a crossing box.

## 5 Rounding table

Integer arithmetic only, so the same inputs give the same results bit for bit on every host.

| Use | Scale frame → device frame |
|---|---|
| a point | `round_half_up(v · num / den)` |
| an input box (click, long press, drag, path drag, page turn) | inward: left and top edges `ceil`, right and bottom edges `floor`; less than one pixel wide or high is `collapsed` |
| an evidence or projection box | outward: left and top edges `floor`, right and bottom edges `ceil` |
| a length in base pixels (a brake, `cross_px`, `delta`, `min_segment_px`) | `round_half_up(v · num / den)`, then held to its own bounds by its caller |
| device frame → scale frame (the reverse; insets, the layout, the crop's offset) | `round_half_up(v · den / num)` |

`round_half_up` rounds halves towards positive infinity.

## 6 Cropped frames and turned frames

- A cropped frame's regions lose the crop's offset (divided by `s`) and are cut to the frame;
  its inputs gain the crop's offset (device pixels) to become logical-display coordinates.
  Example: a 1080×2340 display cropped to 1080×2336 at y = 2, for a 1080×2340 package, is the
  identity: a region moves up 2, an input down 2, and a region in the two cropped rows is
  `outside_frame`.
- A turned frame's device pixels are the upright frame's; a backend that takes frame
  coordinates receives each input point with the quarter turn undone.

## 7 Codes

The crate's registry is `crates/display-adapter/src/codes.rs`, its fragment
`contracts/outcome-codes/display-adapter.json`. Two codes (#420 S4), both pending:

| Code | Category | `ladder_class` | Values |
|---|---|---|---|
| `display_map_unresolved` | error | `declaration_error` | `display_problem` (required); `stage`, `frame_width`, `frame_height`, `base_width`, `base_height`, `target_id`, `pack_operation_id` (optional) |
| `display_map_stale` | warning | `stability_warning` | registered by DA-1, first emitted by DA-3 |

- Vocabulary `display_problem`: `layout_undeclared`, `scale_below_base`, `collapsed`,
  `outside_frame`, `straddles_zones`, `orientation_mismatch` (only a cause under the run's own
  end from DA-2b on); DA-2a adds `translation_unsupported`, `input_scale_unsupported` and
  `safe_area_unavailable` (section 8).
- Locations (the value of `stage`): `display_map_build`, `display_region_map`,
  `display_input_map`.
- Refusals the host reports under its own codes: an unknown rotation
  (`contained_task_geometry_unknown`), a frame that does not show its display
  (`contained_task_geometry_frame_display_mismatch`), and a safe area without read insets
  (`contained_task_geometry_unavailable`).
- Every refusal ends only the run it belongs to, never the Runtime (#419).

## 8 The capture side (DA-2a: the identity and a pure scale)

Workflow #392 DA-2a, model 392 v1.2 §0.2 C1 to C3 and C10, §3.2, §5.1, §6.6, §8.3; the
coordinator's work order narrows the slice to the maps a 16:9 or 9:16 emulator gives: the
identity and a pure scale. Translations and cropped frames come with DA-2b, the input side with
DA-3.

- **The run's declaration.** The preparation of a run reads its game root's
  `display-layout.json` (when the root holds one, as DA-4 reads it) and keeps the parsed
  declaration for the whole run; every layer of the run (its prerequisite, entry recovery,
  interrupt and navigation packages) maps its frames under it. The mapping rule is built from it
  (`DisplayRule::from(&StandardPackDisplayLayout)`); the declaration's shape is the contract
  crate's only. A run without a standard-pack root, and the Lab's dry runs, map without one.
- **The frame gate.** Every frame the kernel recognises passes one gate, at each of its six
  capture entries: the entry page, a navigation locate, a sample, a page frame, a reading's
  agreement frame and the Lab's dry-run select. The gate builds the frame's map from the frame's
  size and the geometry its producer recorded with it (`DisplayObservation::of_frame`: a producer
  that recorded none shows the frame as its own display, with no rotation, crop or insets), the
  package's base and the declaration:

  | Map | The scale frame | Otherwise |
  |---|---|---|
  | the identity, no crop | the frame itself, its own buffer | — |
  | a pure scale, no crop | the frame resampled once to the base's size (REC-5, area average) | — |
  | a translation, or any map of a cropped frame | — | `display_map_unresolved` (`translation_unsupported`), until DA-2b |
  | a frame turned upright | — | `display_map_unresolved` (`orientation_mismatch`), until DA-2b |
  | a refusal of the map | — | `display_map_unresolved` with the map's problem; a frame whose geometry does not show its display is `contained_task_frame_invalid` |

  Every refusal is at stage `display_map_build`, with the frame's and the base's sizes, and ends
  the run with no input. It replaces `contained_task_frame_resolution_mismatch`, retired (§0.2
  C10). Recognition, its regions and its lengths stay in base pixels on the scale frame; the frames
  of one observation keep one map (`recognition_sample_geometry_changed`, and for a candidate
  sample `candidate_sample_page_or_geometry_changed`).
- **Evidence.** A recognition record's frame size is the device frame's; each recognised target's
  region is the device box that covers it (outward, section 5). The task diagnostic stream's rows
  keep scale-frame pixels in this slice.
- **Geometry records and rebuilds.** The host builds the same map from the same frame for the
  run's task geometry. A passed observation names `display_map` (register §1.25 X7) when the map
  is not the plain identity; the identity leaves the record as it was. A frame whose size, or
  whose producer's rotation, crop or insets, differ from those the run's map was built on reads
  the display's geometry again and is recorded `changed`, with the first change as its trigger
  (`extent_changed`, `rotation_changed`, `insets_changed`; register §1.25 X8); its map is rebuilt
  and the instance's display facts are published again when they changed. A frame of another size
  no longer fails the input prerequisite.
- **Input.** The input side maps nothing before DA-3, so the host makes an input only on a frame
  whose map is the plain identity; under a pure scale the step sends nothing and the run ends
  `display_map_unresolved` (`input_scale_unsupported`, stage `display_input_map`).
- **The safe area.** No capture backend reads insets before DA-4b, so a declaration that
  measures on the safe area ends a run at its first frame with
  `contained_task_geometry_unavailable` (`display_problem` `safe_area_unavailable`) before any
  input.
