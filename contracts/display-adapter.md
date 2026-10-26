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
| DA-4b-D | the declared safe area: declaration files of device models, an instance's model setting, the live change (Alice's ruling 69; it replaces DA-4b, which read a device's cutout) | section 9 |
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
  insets, and needs insets (declared, section 9; else the host's
  `contained_task_geometry_unavailable`) that leave a width and a height (else `collapsed`).
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
  `safe_area_unavailable` (section 8); DA-4b-D adds `safe_area_declaration_invalid`,
  `safe_area_model_repeated`, `safe_area_model_undeclared` and `safe_area_form_undeclared`, with
  the optional values `path`, `other_path`, `json_pointer` and `config_key` (section 9.4).
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
- **The safe area.** No capture backend reads insets; a frame's insets are declared (section 9).
  A declaration that measures on the safe area on a frame without insets (a handset that declares
  no device model) ends the run with `contained_task_geometry_unavailable` (`display_problem`
  `safe_area_unavailable`) before any input.

## 9 The declared safe area (DA-4b-D)

Alice's ruling 69 (2026-10-26, board #392): 「只有安全区适配不做，但是需要留出来声明式的安全区接口，也就是说可以
依照声明的文件直接切换安全区形态」 and 「一份文件对应一个机型或多个机型，并且修改设置后实时生效」. The Runtime does
not adapt to a device's safe area by itself: it never reads a display's cutout (no `dumpsys
display` parsing, no `display.safe_area.reported`) and never detects a device's model. It offers a
declared interface instead: files declare device models and their insets, an instance declares
its model by a setting, and either change takes effect while the Runtime runs. This section
replaces the DA-4b row of model 392 v1.2 (§4.5 keeps its arithmetic; §8.2 is not done).

### 9.1 The declaration file `actingcommand.display-safe-area.v1`

Every regular file of the declaration directory (9.2) whose extension is `json` (any case) is one
declaration; other entries are not read. A file declares one or more device models and, for
them, the insets of each display size and rotation it covers:

```json
{
  "schema_version": "actingcommand.display-safe-area.v1",
  "models": ["neutral-model-a", "neutral-model-b"],
  "forms": [
    {
      "display": {"width": 1000, "height": 2000},
      "rotation": "r0",
      "insets": {"left": 0, "top": 80, "right": 0, "bottom": 0}
    },
    {
      "display": {"width": 2000, "height": 1000},
      "rotation": "r90",
      "insets": {"left": 80, "top": 0, "right": 0, "bottom": 0}
    }
  ]
}
```

| Member | Rule |
|---|---|
| `schema_version` | exactly `actingcommand.display-safe-area.v1` |
| `models` | a non-empty list of non-empty strings, none twice: the device models the file declares (any names; the Runtime compares them as written) |
| `forms` | a non-empty list; no two forms with the same `display` and `rotation` |
| `forms[].display` | `{width, height}`, each at least 1: the logical display, device pixels, at that rotation (the frame's `logical_display_extent`) |
| `forms[].rotation` | `r0`, `r90`, `r180` or `r270`: the display's rotation as the frame's geometry records it (a top cutout lies on another side at `r90` than at `r270`) |
| `forms[].insets` | `{left, top, right, bottom}`, device pixels of that display at that rotation; `left + right < width` and `top + bottom < height` |

Every member is required and no other is read. A file that breaks a rule is refused at the JSON
pointer of its first unknown, missing or misfit member (empty for the whole document). One model
may be declared by one file only. As JSON Schema (draft 2020-12):

```json
{
  "$id": "actingcommand.display-safe-area.v1",
  "type": "object",
  "additionalProperties": false,
  "required": ["schema_version", "models", "forms"],
  "properties": {
    "schema_version": {"const": "actingcommand.display-safe-area.v1"},
    "models": {"type": "array", "minItems": 1, "uniqueItems": true,
               "items": {"type": "string", "minLength": 1}},
    "forms": {"type": "array", "minItems": 1, "items": {
      "type": "object", "additionalProperties": false,
      "required": ["display", "rotation", "insets"],
      "properties": {
        "display": {"type": "object", "additionalProperties": false,
                    "required": ["width", "height"],
                    "properties": {"width": {"type": "integer", "minimum": 1, "maximum": 4294967295},
                                   "height": {"type": "integer", "minimum": 1, "maximum": 4294967295}}},
        "rotation": {"enum": ["r0", "r90", "r180", "r270"]},
        "insets": {"type": "object", "additionalProperties": false,
                   "required": ["left", "top", "right", "bottom"],
                   "properties": {"left": {"type": "integer", "minimum": 0, "maximum": 4294967295},
                                  "top": {"type": "integer", "minimum": 0, "maximum": 4294967295},
                                  "right": {"type": "integer", "minimum": 0, "maximum": 4294967295},
                                  "bottom": {"type": "integer", "minimum": 0, "maximum": 4294967295}}}
      }}}
  }
}
```

The schema cannot say three rules, which the Runtime checks as well: the insets leave a width and
a height, no two forms of a file share a display and a rotation, and no model is in two files.

### 9.2 The keys

| Key | Type, default | Meaning |
|---|---|---|
| `display_safe_area.directory` | string, `display-safe-area` | the directory of the declaration files; a relative path resolves against the Runtime's state root; empty is refused |
| `display_safe_area.check_interval_ms` | milliseconds `0..=3600000`, `1000` | the least time between two reads of the directory; 0 reads it at every frame that needs it |
| `display_safe_area.model` | string, empty | the device model every instance declares unless it names its own; empty declares none |
| `instance.<id>.display_safe_area.model` | string, default the section's `display_safe_area.model` | the device model the instance declares (the `instances[]` entry's `display_safe_area.model`) |

Every key is settable ([runtime-config.md](runtime-config.md), "The `display_safe_area` section").

### 9.3 Which insets a frame takes

Only a run whose game's display declaration measures the scale or the anchors on the safe area
(`scale.area` or `anchor_area` `safe_area`) asks. A `screen` declaration, a run without a
declaration and a frame whose producer recorded no geometry are never touched by any declaration
file or setting. For a frame of such a run, the host reads the instance's model in effect at that
frame and:

| Instance | Insets |
|---|---|
| declares a model | the insets of the form whose `display` is the frame's logical display and whose `rotation` its recorded rotation, in the file that declares the model |
| declares none, an emulator (no handset bound) | 0 on every side: an emulator needs no file |
| declares none, a handset | none: `contained_task_geometry_unavailable` (`safe_area_unavailable`), section 8 |

The insets are stamped on the frame's own geometry (`CaptureGeometry.safe_insets`, register §1.25
X6), where the run's map (section 2) and the kernel's frame gate read them. A map they move is a
translation, which waits for DA-2b; insets that leave the scale space and the anchor area as the
base (for example a top inset under a scale by width on the safe area with the anchors on the
screen) keep the identity.

**Live effect.** The directory is read again at a frame once `display_safe_area.check_interval_ms`
has passed since its last read; files of the same names and bytes as before (their SHA-256) keep
the catalog they had, any change is parsed again. The model setting is read at every frame. So an
edit of a declaration file, or a change of an instance's model (`ConfigureSettings`, MCP
`ac_config_set`), takes effect from the next frame that reads it, without a restart: the frame's
insets differ from those the run's map was built on, its geometry is read again and recorded
`changed` with trigger `insets_changed`, and the map is rebuilt (DA-2a's C2 path, section 8).

**What a record shows.** The insets in effect are those of the frame in the run's
`task.geometry_observed` records (`frame.producer.safe_insets`); a map that measures on the safe
area names its insets' source `declared` (`display_map.insets_source`, X7), an emulator's 0
included. `display.safe_area.declared` and `display.safe_area.reported` (X10) keep no writer.

### 9.4 Refusals

A frame whose declared insets cannot be given ends its run before any input,
`display_map_unresolved` at stage `display_map_build` with the frame's `frame_width` and
`frame_height` (only that run; never the Runtime, #419):

| Case | `display_problem` | Values |
|---|---|---|
| the directory or a file cannot be read, or a file breaks 9.1 | `safe_area_declaration_invalid` | `path` (the file, or the directory), `json_pointer` |
| one model in two files | `safe_area_model_repeated` | `path` (the later file in name order), `other_path` (the earlier) |
| no file declares the instance's model (a directory that does not exist declares none) | `safe_area_model_undeclared` | `config_key` (the setting the model was read from), `path` (the directory) |
| the model's file covers no display size and rotation the frame shows (a source that records no rotation matches no form) | `safe_area_form_undeclared` | `path` (the file) |

A broken directory refuses every model until it is mended: a file that does not parse may declare
any model.
