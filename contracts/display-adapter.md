# Display adapter

Workflow #392 (with #401), model 392 v1.2. The display adapter sits between a task package and
the capture and input backends: a package writes base coordinates, and the adapter maps them
onto whatever frame the device delivers. This document grows slice by slice:

| Slice | Section | State |
|---|---|---|
| DA-1 | the pure mapping crate `crates/display-adapter` (sections 1 to 7) | this document |
| DA-4 | the game's declaration file `display-layout.json` (`actingcommand.display-layout.v1`), parsed by the contract crate | its own section, later |
| DA-2a | the capture side: the frame gate, the scale frame, region mapping in recognition, evidence out of the adapter, rebuilds and the `display.*` facts | later |
| DA-2b | the nearest correction window, the `anchor` member, the refusal of crossing positional boxes | later |
| DA-3 | the input side: every click and drag kind, `display_map_stale`, the crop offset and the turn undone for input | later |
| DA-5 | task schema `0.10` and box references; the Lab | later |

Until DA-2a nothing in the Runtime calls the crate: a 1280×720 package on a 1280×720 frame
behaves exactly as before, byte for byte.

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
  `outside_frame`, `straddles_zones`, `orientation_mismatch` (only ever a cause under the run's
  own end).
- Locations (the value of `stage`): `display_map_build`, `display_region_map`,
  `display_input_map`.
- Refusals the host reports under its own codes: an unknown rotation
  (`contained_task_geometry_unknown`), a frame that does not show its display
  (`contained_task_geometry_frame_display_mismatch`), and a safe area without read insets
  (`contained_task_geometry_unavailable`).
- Every refusal ends only the run it belongs to, never the Runtime (#419).
