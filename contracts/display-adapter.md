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
| DA-3 | the input side: every click and drag kind, `display_map_stale`, the crop offset and the turn undone for input | section 10, built in 0.12.59 for the identity and the pure scales whose double is a whole number (3/2, 2, 5/2, 3, …); every other pure scale waits for the slice that draws the input points in the device box |
| DA-5ab | the Lab's online observation and its presses on a display that maps to its package's base, in the base coordinates | section 11, built in 0.12.67 |
| DA-5cd | the Lab's offline recognition on a stored scene of another size than its package; the base size of a recording and the refusal of a recording on a display that is not the base size | section 12, 0.12.67 |
| DA-5 | task schema `0.10` and box references; the conversion of a recording made on a display that is not the base size | later |
| F-3b | without a rule, a display within a pixel of the base's shape is the base's display scaled, each axis by its own factor (Workflow #479) | section 13, 0.12.75 |
| DA-L0 | the Lab's layout probe: where each template mark of a page went on a saved frame of the base's ratio or wider, offline | section 14, 0.12.83 |

From DA-2a every frame a run recognises passes the crate's map (section 8), and from DA-3 every
input a run makes leaves through it (section 10); a 1280×720 package on a 1280×720 frame behaves
exactly as before, byte for byte. From DA-5ab the Lab's online observations and operations take the
same map and the same exit (section 11), and from the train of DA-5ab and DA-5cd its stored scenes
the same reduction as its live frames (section 12).

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
  Otherwise `layout_undeclared`. Workflow #479 F-3b (section 13): the base's aspect ratio
  exactly or within a pixel; each axis then has its own factor, the layout's width over `Wb`
  across (`s`) and its height over `Hb` down, one fraction when the ratio is exact.
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

`round_half_up` rounds halves towards positive infinity. Workflow #479 F-3b: on a display of
section 13 each axis takes its own factor in every row, the width's across and the height's down;
on every other map the two are `s`.

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
| `display_map_stale` | warning | `stability_warning` | `stage` (optional); registered by DA-1, first emitted by DA-3 (section 10) |

- Vocabulary `display_problem`: `layout_undeclared`, `scale_below_base`, `collapsed`,
  `outside_frame`, `straddles_zones`, `orientation_mismatch` (only a cause under the run's own
  end from DA-2b on); DA-2a adds `translation_unsupported`, `input_scale_unsupported` and
  `safe_area_unavailable` (section 8; DA-3 narrows `input_scale_unsupported` to a pure scale
  whose double is not a whole number, a vertical-brake drag under a scale, and a Lab declared
  operation's input on a frame of another size than its package; DA-3b takes the vertical-brake
  drag out of it, section 10; DA-5ab takes the Lab declared operation out of it, whose input
  leaves under the frame's map as a run's does, section 11); DA-4b-D adds
  `safe_area_declaration_invalid`,
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
  crate's only. A run without a standard-pack root, and the Lab's dry runs, map without one, as
  the Lab's online observations and operations do (section 11).
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
  sample `candidate_sample_page_or_geometry_changed`). In a page graph's step confirmation and
  a step plan's wait after an input or an application effect, such an observation ends as a frame
  that did not confirm the step and the wait goes on in its window (#402 HS-5d, since 0.12.40;
  [application-lifecycle.md](application-lifecycle.md)).
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
- **Input.** Before DA-3 the input side mapped nothing, so the host made an input only on a frame
  whose map was the plain identity, and under a pure scale the step sent nothing and the run ended
  `display_map_unresolved` (`input_scale_unsupported`, stage `display_input_map`). DA-3 removes
  that refusal for the pure scales whose double is a whole number: their inputs are mapped
  (section 10).
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

## 10 The input side (DA-3)

Workflow #392 DA-3 (0.12.59), model 392 v1.2 §0.2 C2, C4 to C6, §4.6, §4.7, §6, §9; the
coordinator's work order narrows the slice to the maps the capture side admits (section 8): the
identity and a pure scale. Its fix 1 (the coordinator's rulings after the independent review)
narrows the pure scales to those whose double is a whole number, routes the Lab's declared
operations through the same refusal, and pairs the task evidence in one coordinate system.
Workflow #392 DA-3b (0.12.59; the coordinator's ruling of 11-13 on board #392, route E) sends the
vertical-brake drag under those scales too ("The vertical brake"), with no interface revision.

- **One exit.** Every input of a run (its own steps and its layers', a select's candidate, a
  linear step, a scan's moves and page flips, a store step's tap) leaves for the device at one
  place, the host's input of the run, after the input prerequisite. The kernel makes, checks and
  records every input in base pixels as before. A Lab declared operation's input, which the
  kernel makes in base pixels too, leaves at the Lab's own exit under the rule below ("The Lab's
  declared operations").
- **The identity.** Under the plain identity (section 8) the input leaves exactly as the kernel
  made it: nothing is mapped, kept, read or recorded beyond what was before. What remains on the
  path is the check DA-2a made, whether the run's map is the plain identity, and the same check
  where the step's effect intent is recorded; on a Lab declared operation's path, one comparison
  of the frame's size with the package's base.
- **The maps it sends under.** The identity and a pure scale whose double is a whole number,
  `s = num / den` reduced with `den` 1 or 2 (3/2, 2, 5/2, 3, …; `DisplayMap::admit_input`).
  Under any other map the input is refused before anything is sent, with the token the frame gate
  gives that map, `display_map_unresolved` at stage `display_input_map` with the frame's and the
  base's sizes: a translation or a cropped frame `translation_unsupported`, a turned frame
  `orientation_mismatch`. The capture side refuses such frames first, so no run reaches this
  refusal before DA-2b. A pure scale whose double is not a whole number (5/4, 4/3, 8/5, 9/4, …),
  which the capture side admits and recognises, is refused `input_scale_unsupported` at the run's
  first input, whatever its point, before anything is sent; the run ends with it, so the same
  device always ends the same run the same way. Such a scale waits for the slice that draws the
  input points in the device box.
- **The vertical brake.** `InputAction` and the device layer hold a
  `single_touch_drag_with_vertical_brake_v1`'s brake at exactly 100 pixels; on a scaled device the
  package's brake of 100 base pixels is `round_half_up(100 · s)` device pixels. DA-3b: under a
  scale other than 1 the host sends such a step as a `single_touch_drag_with_pause_and_brake_v1`:
  its press and its corner mapped as every point is ("Points"), drag `horizontal_duration_ms`
  (200 ms), pause `corner_hold_ms` (150 ms), and the brake to `(x2′, y2′ − round_half_up(100 · s))`
  over `brake_duration_ms` (200 ms). At 2 the drag (300, 150) → (20, 150) → (20, 50) leaves as
  (600, 300) → (40, 300) → (40, 100). For the same points the device layer performs both kinds
  with the same events: the press, a segment of the MAA 2.0 curve over 200 ms, a hold of 150 ms, a
  segment over 200 ms, the release (the vertical kind's slopes are constants no plan reads). A
  brake end outside the device frame is `contained_task_input_out_of_bounds`, never clamped; the
  kernel makes no such drag, since its corner lies at least the brake below the top. No interface
  revision moves and no code is added: `InputAction::validate` and the device layer accept the
  drag sent as they do today. Under a scale of 1 the drag leaves as the kind it is, byte for byte
  as before. A reader sees the bundle's kind in the effect intent and the sent kind in the input
  intent ("Records").
- **Stale.** The host reports `display_map_stale` (warning, stage `display_input_map`;
  `DisplayMap::current_for`) in exactly one case: an input whose frame reference names a frame of
  another size than the frame the run's map was last built for. Nothing is sent, and the run ends
  with it. The input prerequisite already requires the run's last frame to be the frame its
  geometry and map were taken on, and the map is rebuilt on every frame whose geometry changed
  (section 8), so the code reports a stale map only for an input decided on an earlier frame than
  the last one taken when the two differ in size. A change of the display's geometry after the
  last frame was taken (a rotation, a resize before the input) is not detected in this slice: the
  input leaves under the map of the last frame. That detection belongs to the rotation
  verification slice.
- **Points.** Each point goes into the scale frame by the anchor of its own zone (outside it:
  `outside_frame`), then to the device frame `round_half_up(v · s)` (section 5). It must lie in the
  device frame and, when the step drew it in a box, in the device box that box maps to, rounded
  inward: the step's effect intent names those boxes (its sampling regions: a tap's or a long
  press's one, a drag's press and end). A box with no whole device pixel is `collapsed`; a point
  outside the device frame or its device box is `contained_task_input_out_of_bounds`. A point is
  never clamped, and every refusal sends nothing. Under the scales the input side sends under no
  legal point meets either refusal, the frame's last column and row included (1279 at 3/2 is
  1919): for a box `a ≤ v < b` the point `⌊v · s + ½⌋` lies in `⌈a · s⌉ .. ⌊b · s⌋` for every
  `v` exactly when the fraction of every `a · s` is 0 or at least ½, which holds over a base
  exactly when `den ≤ 2` (`a · num` takes every residue modulo `den`), and the upper edge holds
  for every `s ≥ 3/2`. Under any other pure scale a point on a box's edge would be refused
  depending on which point was drawn, and `contained_task_input_out_of_bounds` is a declaration
  error that must be the same on every run; hence the up-front refusal above.
- **Kinds.** A tap and a long press map their point; a swipe its press and its end; a drag with a
  pause and a brake its press and end, and its brake's cross `round_half_up(|d| · s)` with its
  sign; a path drag its own box (inward) and every point, each inside that box. Durations, holds,
  slopes and the number of points never change. Keys, text and the reset carry no coordinate.
  Every input keeps its kind but a vertical-brake drag under a scale ("The vertical brake").
- **Records.** The task's effect intent keeps what the package asked for, in base pixels: the
  input, its sampling regions (the base boxes) and the device frame's size (`frame_extent`). The
  input intent records what is sent, in device pixels: the input of its provenance and its
  execution plan. No record gains a member; a reader that needs the scale finds it in the run's
  `task.geometry_observed` record (`display_map`, section 8). The Lab's `run evidence` reads them
  so: a step's `action` (its `point` and its `rect`) is the effect intent's, in base pixels, and a
  recognised target's `box` is the recognition record's, in device-frame pixels (section 8,
  "Evidence"). DA-3b: a vertical-brake step under a scale has the bundle's
  `single_touch_drag_with_vertical_brake_v1` in its effect intent (base pixels, its two sampling
  regions) and the `single_touch_drag_with_pause_and_brake_v1` sent in its input intent (device
  pixels), whose intent, as every such drag's, records no execution plan (register §1.58); both
  are `input.swipe`. `run evidence` shows the bundle's kind; the Lab's recording, which presses no
  brake drag, the diagnosis and the landing history, which read no input's kind, are unchanged.
- **The task evidence.** `actingledger task-evidence` pairs an input intent with the effect intent
  of its step in one coordinate system: with the run's last `task.geometry_observed` record before
  the effect intent naming a map (its `display_map`: never under the plain identity), the effect
  intent's input is converted by the product's own functions (`DisplayMap::admit_input`,
  `device_input`, display coordinates, the effect intent's sampling regions as its boxes) on the
  map of that record's base and the effect intent's `frame_extent`, with the host's rounding, and
  compared with the input sent; an input the product would have refused pairs with nothing
  (`source_mismatch`). Without such a record, under the plain identity, the comparison is the
  effect intent's input itself, byte for byte, as before this slice. The effect intent records no
  base, so that record is looked up backwards from the effect intent in the whole snapshot the
  command reads, not only in the page it reports: read in pages or whole, a run pairs the same.
  DA-3b: the same conversion turns a scaled vertical-brake step's effect intent into the drag with
  a pause and a brake sent, so the step pairs; an input that is not that drag still pairs with
  nothing.
- **The Lab's declared operations.** Until DA-5ab a Lab declared operation sent an input only on
  a frame of its package's size and refused any other, `display_map_unresolved`
  (`input_scale_unsupported`). DA-5ab replaces that refusal: its input leaves under the frame's
  map as a run's does (section 11). A base coordinate never leaves for a scaled frame unchanged.
- **The crop offset and the turn undone.** `DisplayMap::device_input` also adds a cropped frame's
  offset for a backend that takes display coordinates (`InputCoordinates::Display`) and undoes a
  turned frame's quarter turn for one that takes frame coordinates
  (`InputCoordinates::DeliveredFrame`; a vertical brake cannot keep its direction across a turn
  there: `orientation_mismatch`). The host refuses those maps until DA-2b, so the function and its
  tests carry them, no run.
- **Not in this slice.** The model's `anchored` click, a drag's `delta` and the box references
  (package members of task schema `0.10`, DA-5); moving the brake bound of `InputAction` and the
  device layer (DA-3b sends a scaled vertical brake without it, above); a path
  drag generated again in its device box (the host maps the points the kernel generated in the
  base box); a landing history kept per device box (it is kept per base box, which under one map
  names one device box); a step that takes its next frame after `display_map_stale` within the
  run; a geometry change after the last frame was taken (the rotation verification slice); the
  pure scales whose double is not a whole number (the slice that draws the points in the device
  box); a Lab declared operation on a scaled frame, and the Lab's own inputs (DA-5ab, section 11).

## 11 The Lab on a display that maps to the base (DA-5ab)

Workflow #392 DA-5ab (0.12.67; register §1.436). The owner decided for screen adaptation one set of
packages at the base size and a converter (board #392, decision 203); what the decision left
unwritten follows the default, and the coordinator ruled under it: **on a display that maps to the
package's base, the Lab speaks the base coordinates to an agent.** On a display of the base's size
nothing changes, byte for byte.

- **The one implementation.** A frame the Lab observes is read back from its picture, so it
  carries no producer geometry, and the Lab reads no game declaration: its map is the frame
  gate's map of section 8 built without either (the frame shows itself), so only the identity and
  a pure scale map. The kernel's `lab_scene_on_base` brings such a frame to the base with the frame
  gate's own code (its map and its one resample, area average); `lab_display_map` gives the same
  map for an input. Neither is a copy: the frame gate itself is built on them.
- **Every live observation.** The frames of these observations go through it, and are evaluated
  and projected on the base: `observe --capture` (its transaction and every further transaction of
  a candidate layout's series); a Lab operation's before observation, its after observation and
  each arrival sample (`do --capture` with an element, `--tap`, `--swipe`, a recorded click); a
  declared operation's frames (the kernel's frame gate, as before) and its after projection; and
  the client's `--capture` forms, which evaluate Runtime captures in the Lab (`do --capture
  --dry-run`, `ensure --capture` with its guard frames and its polls, `wait --capture --page` and
  `--stable`). A plain `capture`, `ac_frame` and a device command's frame (`tap`, `swipe`,
  `long-tap`, `key` and their `--record`) are the device's own frame and take device pixels.
- **The projection's frame.** On a mapped display the projection's frame identity is the device
  frame's artifact hash at the base's size, and the observation's `frame` (the capture's artifact,
  with the device's size) stays the device frame's reference. A verifier reads the device frame by
  that hash from the ledger, builds the map from its size and the base (the projection's size),
  finds that the map's scale frame is the projection's size, and recomputes the RGB8 hash as that
  of the device frame resampled once under the map. `ContainedPageObservation::validate` (the
  client's check of the receipt and of a Lab operation record's projections) takes a projection
  frame that is the observation frame itself, or of its ratio, no larger and with no side of zero
  (fix round 1: a zero side was taken before; Workflow #479 F-3b: "of its ratio" is the adapter's
  own test, `CaptureExtent::has_shape_of`, exactly or within a pixel, section 13, `runtime-client`
  13): what a pure
  scale brings that frame to. It knows the
  two sizes and not the package, so it cannot know the package's base: any base of the frame's
  ratio no larger than the frame passes, and so does a scale whose presses the input side does not
  send (5/4, whose observation stands); that the projection is the package's base is the
  verifier's, with the package. No record gains a member and no interface revision moves; the map
  is not recorded, since the frame's size and the base settle it.
- **Refusals.** A frame the map cannot take is refused with the code a run gives the same frame,
  `display_map_unresolved` at `display_map_build` with the frame's and the base's sizes: another
  ratio `layout_undeclared`, a frame smaller than the base `scale_below_base`, the other
  orientation `orientation_mismatch`. It is never `observation_projection_failed`.
- **Presses.** Every point, rectangle and swipe a Lab operation takes is in the base: a
  coordinate selection (`--tap`, `--swipe`, a recorded click), an element's press (its
  projection's rectangle and point) and a declared operation's input (the kernel's, its draw's
  boxes). On a frame of the base's size the input leaves as it was taken, byte for byte; on any
  other it leaves through the run's own exit, `DisplayMap::send_input` (`admit_input`, the frame it
  was decided on, `device_input` to the logical display, each point inside the inward device box of
  its box): at 3/2 the base point (640, 360) is sent as (960, 540) and the base column 1279 as 1919.
  A ratio the input side does not send under (a pure scale whose double is not a whole number, 5/4)
  refuses the operation at its `input` stage, `display_map_unresolved` (`input_scale_unsupported`
  at `display_input_map`), with nothing sent. The records keep the base input (the prepared
  record's `action` and `geometry`, an attempt's `action`); the input intent records what was sent,
  in device pixels, as a run's does. The client's `ensure --capture` sends its route's inputs the
  same way. A recorded click (`do --capture --record`) is planned in the recording's coordinates.
  The train of DA-5ab and DA-5cd (its rule R4; section 12, [lab-recording.md](lab-recording.md),
  "A recording's size"): nothing recorded is sent before the recording is known to take it.
  `do --capture --record` judges the recording as `record stop` judges it (`record_size_not_base`)
  and one frame it captures through the Runtime before its press as a frame that enters the
  recording (`record_display_not_base` for a recording without a frame, `record_frame_size_mismatch`
  when the display changed during the recording), before anything is pressed or written; `observe
  --capture --record` judges the recording before it captures and the Runtime's device frame after
  it observed the same way, before the frame file or the recording is written. So no device frame
  enters a recording beside an answer in the base, and no recorded press is sent on a display that
  is not the recording's size. (DA-5ab and its fix round 1 refused both against the package's base,
  `record_frame_size_mismatch`; the train replaced that check with the recording's own.) The device
  commands' `--record` (`capture`, `tap`, `swipe`, `long-tap`) and `session app --record` record the
  device's frames and pixels, as they speak them, under the same judgement.
- **The answers.** `frame_coordinates` on a mapped display is `{space: "base", base_pixels:
  [width, height], reduced_from: [device width, device height], orientation, applies_to}`; on a
  display of the base's size it is `{device_pixels, orientation, applies_to}` as before. The frame
  files and the previews an operation hands out (`observe --with-frame`, `--preview`, the after
  attachment's `frame_path` and `preview`) are the frames brought to the base. No client form of
  `do` sends its tap itself: `do --capture` without `--dry-run` is always the Lab operation, and `do
  --capture --dry-run` sends nothing (fix round 1 described a client form's `device.action` and
  `device.base_action`; that form was never reached, and the train's fix round 1 removed it). The
  MCP front's `ac_lab_observe` and `ac_do` answer the command line's line, and the first-generation
  server's `ac_lab_observe` and `ac_lab_do` (actingctl) say the same in their descriptions (fix
  round 1). The train: `FrameCoordinates` (the kernel's) builds both forms, for these answers and
  for the answers about a stored scene (section 12), which name the same base form when the scene
  was reduced.
- **Not in this section.** The stored scenes, which take the same function since the train
  (section 12); the recording on such a display, which stays refused (section 12); the run
  evidence's recognised boxes, which stay in device-frame pixels (section 10, "Records").

## 12 The Lab's offline scenes and the recording's size (DA-5cd)

Workflow #392 DA-5cd (0.12.67; register §1.437), as the train of DA-5ab and DA-5cd leaves it
(register §1.436 and §1.437, "Train"). Two things the Lab did with a display that is not its
package's size, now said and refused where they cannot hold. The Lab's live observation and its
presses on such a display are section 11's.

- **An offline scene of another size than its package.** Every offline evaluation of a scene, a
  stored image, takes it in its package's `coordinate_space` (its base, the recognition pack's):
  `recognize`, `detect-page`, `current-page`, `is-visible`, offline `observe`, `do --dry-run`,
  `ensure --dry-run` and `wait` on saved scenes, `tap-target --dry-run` and `navigate --dry-run`. A
  scene of the base's size is evaluated as it is, and its answer is byte for byte the one before
  DA-5cd. Any other scene is judged, reduced and answered exactly as a frame of its size is by the
  Lab's live observation (section 11), by the same function, the kernel's `lab_scene_on_base` (the
  frame gate's map, built with no producer geometry and no game declaration, and its one
  resample, area average): a scene of the base's ratio (Workflow #479 F-3b: exactly or within a
  pixel, section 13), not smaller and in its orientation, is
  reduced once to the base and evaluated there, a 5/4 one too (whether a press could land exactly
  is no reason to refuse a read; a press at 5/4 stays refused where presses are admitted, section
  11). Every rectangle and point of the answer is the base's, and the answer names
  `frame_coordinates` in section 11's base form (`{space: "base", base_pixels, reduced_from,
  orientation, applies_to}`; a frame set: each frame reduced, the current frame's size named): an
  answer that stated no `frame_coordinates` before states them for a reduced scene only, and
  offline `observe` and `do`, which state them for every scene, state the base form for a reduced
  one; `observe --with-frame` writes the reduced frame. Any other scene is refused before anything
  is evaluated, `lab_scene_size_unsupported` (exit 2 on a `--via` line; actinglab's own line and
  `serve --stdio --front mcp` exit 1 for every refusal), its values the gate's word `display_problem`
  (`layout_undeclared` another ratio, `scale_below_base` a scene smaller than the package,
  `orientation_mismatch` the package's other orientation), the scene's size (`frame_width`,
  `frame_height`) and the package's (`base_width`, `base_height`). Two refusals of a size, one cause
  each: `lab_scene_size_unsupported` is a stored scene's, `display_map_unresolved` a live capture's
  (section 11), and neither answers for the other's cause; the client's `--capture` forms reduce a
  Runtime capture once, on section 11's path. Before DA-5cd such a scene was refused
  `validation_failed` with the sizes in its message.
- **The base size and a recording.** The base size is the `coordinate_space` every standard
  package declares today, 1280×720. Nothing a package, a game root or its `display-layout.json`
  declares names a base of its own, so it has one named home, the contract crate's
  `STANDARD_PACK_BASE_SIZE`. A recording is made at the base size ([lab-recording.md](lab-recording.md),
  "A recording's size"): `record start` reads the instance's display as the Runtime last observed
  it (its `display.frame_extent` fact, [instance-fact-store.md](instance-fact-store.md), "Display
  facts", one read from memory with no device step) and refuses a display of another size,
  `record_display_not_base`, before anything is written or paused. That is an early warning; the
  guard (fix round 1) is the recording's header: `record start` writes the base size in it
  (`base_size`), and the first frame that enters the recording is checked against it before it is
  stored, a frame of any other size refused `record_display_not_base` with nothing written. `record
  stop` refuses a recording whose frames are not the base size, `record_size_not_base` (fix round 1:
  a header that names a base size, frames of any other size; one that names none, made before,
  frames of a larger display of the base size's ratio, a pure scale the adapter maps onto it), and
  converts nothing. The train (its rule R4: nothing is sent before the recording is known to take
  it): every path that records asks that judgement before anything goes to the device, and judges
  the frame it took as a frame that enters the recording, whether or not it stores it: `tap`,
  `swipe` and `long-tap --record` and `session app --record` before they capture (also when the
  open step is marked, so that the frame before the press is not stored) and the frame before the
  press (the train's fix round 1: a frame that would be the recording's first is judged, before it
  is stored, as if the recording already had its size, so that a recording whose header names no
  base size takes no first frame `record stop` would refuse: `record_display_not_base`, nothing
  sent; and a recording the Lab re-creates writes the base size in its header); `do --capture
  --record` the frame it captures through the Runtime before its press (its size as the Runtime's
  observation answers it);
  `observe --capture --record` before it captures and the Runtime's frame after it observed (section
  11, "Presses"). Three codes, one cause each: `record_display_not_base`, this instance's display
  is not the base size and the recording has no frame yet (`record start`, by the display last
  observed, and the first frame); `record_size_not_base`, this recording is not at the base size
  (`record stop`, `record ready`'s item, a frame added to it, every recorded step before it sends
  anything); `record_frame_size_mismatch`, a frame of another size than the recording already has
  (the display changed during the recording). Recording on a display that is not the base size
  stays refused: no recording is made of reduced frames.
- **Still open.** Converting such a recording, or recording on a scaled display in base
  coordinates (its frames reduced as they arrive, its presses mapped by section 10), is not done:
  the study behind this slice proposes it as the larger form of its slice (c), and the recording's
  refusal stands until the owner orders it. A display the Runtime does not know (no contained task
  has run on the device, or nothing listens) no longer starts a recording of another size: the
  start goes on, and the first frame is refused unless it is the base size (fix round 1).

## 13 A display within a pixel of the base's shape (F-3b)

Workflow #479 F-3b (0.12.75; register §1.487); the rule is the coordinator's ruling in the slice's
work order, not the owner's words. A package is written against its base (1280×720 for the games in
use). Without a declared rule a display of another size mapped to the base only when its shape was
exactly the base's, `W · Hb = H · Wb`. A 1334×750 display (960480 against 960000) and a 1366×768
one (983520 against 983040) failed that product by a third of a pixel and were `layout_undeclared`,
so the Lab, which never has a rule, could not read a frame of them, and every game would have had to
declare a rule for them. This is a fact about displays, not about a game.

- **The rule.** A display with **no declared rule** is of the base's shape
  (`CaptureExtent::has_shape_of`, in the contract crate, the one function the map and the client's
  check of a projection frame ask) when `W · Hb = H · Wb`, or when neither of its sides is the
  base's own length and its size, scaled to the base by its width, comes out at the base's height
  after rounding to whole pixels (`round_half_up(H · Wb / W) = Hb`), or, scaled by its height, at
  the base's width (`round_half_up(W · Hb / H) = Wb`). Such a display is the base's display scaled:
  a pure scale, each axis by its own factor, `sx = W / Wb` across and `sy = H / Hb` down, each
  reduced (`DisplayMap::scale`, `DisplayMap::height_scale`). Its scale space and scale frame are
  `round_half_up(W / sx) × round_half_up(H / sy)`, the base itself, its anchor area all of it, and
  its kind `scale`, with the members an exactly proportional display's map has; a factor below 1 is
  `scale_below_base`, as a smaller display of the exact shape is. Of exactly the base's shape the
  two factors are one fraction and the map is the one before this slice, member for member. No
  insets may be in use, as before.
- **The sizes it was reasoned on**, against 1280×720:

  | Display | `W·Hb`, `H·Wb` | Scaled by its width: rows | Scaled by its height: columns | Without a rule |
  |---|---|---|---|---|
  | 1334×750 | 960480, 960000 | 719.64, so 720 | 1280.64, so 1281 | a pure scale, 667/640 across, 25/24 down |
  | 1366×768 | 983520, 983040 | 719.65, so 720 | 1280.63, so 1281 | a pure scale, 683/640 across, 16/15 down |
  | 1335×750 | 961200, 960000 | 719.10, so 719 | 1281.60, so 1282 | `layout_undeclared`, as before |
  | 1280×719 | 921600, 920320 | 719 | 1281.78, so 1282 | `layout_undeclared`, as before |

  1335×750 is not such a display. On a landscape base the second test admits nothing the first
  does not (it asks `|W·Hb − H·Wb|` at most `H / 2`, the first at most `W / 2`), on a portrait base
  the reverse; either suffices.
- **Its limits** (fix round 1). The rule is for real displays, whose bases are hundreds of pixels
  on a side. Its tolerance is the rounding to a whole pixel of the base, half a base pixel whatever
  the base's size, so on a very small base it admits shapes far from the base's: against an 8×4
  base it admits 9×5 (36 against 40; scaled by its width, 5 · 8 / 9 = 4.44 rows, so 4). A display
  one of whose sides is the base's own length is not of the base's shape, whatever its other side;
  on a base about twice as wide as it is high or wider that guard refuses a size the rounding
  would admit (one pixel more across passes the test by the width when `Wb ≥ 2 · Hb − 1`; on a
  base as tall, one pixel more down the test by the height): against 2400×1080, 2401×1080 (scaled
  by its width, 1080 · 2400 / 2401 = 1079.55 rows, so 1080) is `layout_undeclared`. On 1280×720
  the guard decides nothing: no display with one side of the base's length meets either test. The
  code is as this section says for every base, these too.
- **How far a point can be off.** A base point goes to `(round_half_up(x · sx), round_half_up(y ·
  sy))`, the nearest device pixel of the point scaled by its own axis's factor (half a device
  pixel, as under every pure scale), and a device point goes back by the same two factors,
  `round_half_up(X / sx)` and `round_half_up(Y / sy)`; both factors exceed 1, so every base point
  comes back to itself. A press maps by the same two factors. A picture of the base drawn at one
  factor (a game that scales its base onto the display by its width, or by its height) lies at
  most `|W·Hb − H·Wb| / Hb` device pixels across and `|W·Hb − H·Wb| / Wb` down from the two-factor
  placement, at the base's far edges, beyond that rounding. For 1334×750 that is 480 / 720 = 0.667
  device pixel across (0.640 base pixel) and 480 / 1280 = 0.375 down (0.360 base pixel); for
  1366×768 0.667 across (0.625 base pixel) and 0.375 down (0.352 base pixel). The worst case over
  the two axes is across, 2/3 of a device pixel for both sizes: 0.640 base pixel on 1334×750, 0.625
  on 1366×768. Over every display the rule admits for 1280×720 it is at most `Wb / (2 · Hb)` =
  0.889 base pixel across and `Hb / (2 · Hb − 1)` = 720 / 1439 = 0.5003 base pixel down.
- **What a pure scale lets the rest do.** The run's frame gate and the Lab take such a frame as
  they take any pure scale (sections 8, 11 and 12): it is resampled once to the base's size (REC-5,
  area average, each axis by its own ratio) and recognised there, and its evidence boxes go back by
  the two factors, outward. The Lab's live observations and its stored scenes of such a size answer
  on the base, `frame_coordinates` in section 11's base form (`reduced_from` the device's size),
  where they were refused `display_map_unresolved` and `lab_scene_size_unsupported`
  (`layout_undeclared`). The client's check of an observation's projection frame takes the frame
  reduced to such a base. The input side sends only under factors whose double is whole (section
  10, `admit_input`, now asked of both factors): the factors of 1334×750 and 1366×768 are not, so a
  press on such a display is refused at its input, `display_map_unresolved` (`input_scale_unsupported`
  at `display_input_map`), with nothing sent, as on a 5/4 display (it was `layout_undeclared` at
  `display_map_build`); `device_input` converts it by the two factors (a length down, a brake, by
  `sy`) for the slice that draws the points in the device box.
- **The interface revision** (fix round 1; [component-interfaces.md](component-interfaces.md), the
  `runtime-client` row of revision 13). The client's check of a projection frame
  (`ContainedPageObservation::validate`, `reduced_to`) asks the same function, so it takes a frame
  reduced from such a display, which a client library of revision 12 or lower refuses: there the
  receipt of an `ObserveContainedPage` or a `RunContainedLabOperation` that carries one is
  `runtime_receipt_invalid`, and the library's connection latches. It shows only on a display of
  such a size. So this is `runtime-client` 13: the Runtime declares [12, 13], `min` where it was,
  and a console and a Lab pinned beside it are built from a build of revision 13 before such a
  display is observed. `ledger` does not move.
- **What is unchanged.** A declared rule decides first, as before: 1334×750 by width is the rule's
  pure scale with one factor, 667/640, on both axes; by height the rule's translation onto 1281
  columns, `translation_unsupported`. An exactly proportional display maps as before, byte for
  byte. A display that meets neither test is `layout_undeclared`, a turned frame `orientation_mismatch`
  and a translation or a cropped frame `translation_unsupported`, as before. A run's
  `task.geometry_observed` names such a map's `display_map` as it names an exactly proportional
  display's (`kind` `scale`, `scale_num` and `scale_den` the width's factor, `scale_width` and
  `scale_height` the base, `scale_by` `width`, `declared` false); the height's factor is the frame's
  height over the base's. No record, receipt, status member or package gains a member or a value, no
  closed list gains a word and no code is added; the one interface revision that moves is
  `runtime-client`'s, to 13 (above). A recording is still
  made at the base size only: `record start`'s display check and the first frame's judgement refuse
  such a display `record_display_not_base`, and `record stop` such frames `record_size_not_base`
  (section 12). Two paths that ask the map whether a size is the base scaled count these sizes too:
  a recording whose header names no base size (made before 0.12.67) holding frames of such a size is
  `record_size_not_base`, as one of 1920×1080 frames is; and an ending's frame of such a size is
  converted to its recording's size, as one of twice its size is
  ([lab-recording.md](lab-recording.md), "Endings").

## 14 The layout probe (DA-L0)

Workflow #392 DA-L0 (0.12.83; register §1.521); the rules are the coordinator's rulings in the
slice's work order, not the owner's words. On a frame wider than 16:9 an application does not
enlarge its 16:9 interface: some elements stay centred, some follow an edge or a corner, some
stretch, some move inward by a cutout's inset. Before anyone declares how an application lays out
such a frame, somebody has to measure, per application and per page, what each element does. The
Lab's offline operation `layout probe` (MCP `ac_layout_probe`; the operation table's row, slice
`392-DAL0`) measures it from one saved frame. It describes: it writes no declaration and decides
nothing, and the run path is as before (a run on a frame wider than the base's ratio is still
refused at its first frame, section 8).

- **What it reads.** A saved frame (`scene`, a PNG) and a package (`package` or `zip`), read as the
  offline `recognize` reads them (section 12: the externally verified bundle, its recognition pack
  and its page set, and the scene file); `page` names one page of the package, and without it
  every page is answered; `tolerance_px` (default 4) is how far, in scale-frame pixels, a mark may
  lie from a place and still be at it. No capture, no input, no run, no Runtime; nothing is
  written.
- **The frame.** A frame the run path's frame gate takes as the base's (section 8: the base's
  size, or its shape within a pixel as `CaptureExtent::has_shape_of` says, the kernel's
  `lab_scene_on_base`) is that gate's reduced frame: the base itself, no room, its factor the
  frame's height over the base's (fix round 1: a frame a pixel off the base's shape, 257x144 or
  255x144 for a base of 128x72, has no room, as the run path, where it had a room of 1 or was
  refused). Any other frame of the base's orientation, at least the base's height and whose width
  over its height is not below the base's (`W · Hb ≥ Wb · H`) is taken. It is brought to the base's
  height by the map this crate builds under the rule "by height, on the screen" (section 2.3,
  `s = H / Hb`, reduced) and resampled once by the function the frame gate resamples a frame of the
  base's ratio with (REC-5, area average, section 8): the **scale frame**, `round_half_up(W / s)`
  by `Hb`, the base's width and the **room** `R`. Any other frame is refused before anything is
  searched,
  `lab_scene_size_unsupported` (section 12's refusal: exit 2 on a `--via` line; actinglab's own
  line and `serve --stdio --front mcp` exit 1 for every refusal), with one word of
  `display_problem`, in this order: the base's other orientation (for a landscape base, a frame
  whose height exceeds its width) `orientation_mismatch`; lower than the base (a factor below 1)
  `scale_below_base`; narrower than the base's ratio `ratio_below_base` (a word of DA-L0's, which
  no map gives). `layout_undeclared` is never the probe's.
- **The marks of a page.** Every mark the page's rules name is answered, each once with the first
  rule that names it, in this order: its `required`, the members of its `any_of` groups, its
  `optional`, its `forbidden`; each mark's `rule` says which (`required`, `any_of`, `optional`,
  `forbidden`). Nothing a page names is left out: a mark the probe does not search is listed in
  `not_probed` (below).
- **A mark.** Each template mark, each once over the chosen pages, is searched over the whole scale
  frame, its declared region aside: its template scored as recognition scores it, with the pack's
  metric, the mask recognition scores it with (the recognition pack's `scored_mask`), the factors it
  searches at (`searched_factors`), on the exact score maps (the search of a template declared over
  the whole frame). Its best place (a rectangle in scale-frame pixels), its score and its threshold
  (the recognition pack's `template_threshold`: the mark's own, else the pack's default) are
  answered; it is **found** when its score reaches its threshold. `matches` is how many places
  reach the threshold, overlapping places taken once (the score maps' `matches_at_least` and
  `suppress_overlaps` with an overlap of 0); above one the mark is `repeated`: its best place is
  answered and it takes no part in the reading. Its colour check, its luma floor and its declared
  uniqueness are no part of the measurement. A mark that is no template (a colour, a colour digest,
  an OCR or NN target, a check, a value test) and a template this build cannot evaluate are listed
  in `not_probed` with their rule, their kind and their declared rectangle in the base (null where
  they declare none).
- **The base place** (fix round 1). A package keeps a template mark's search box and its image,
  not where the image was cut. A recording's default box is the crop grown by
  `LAB_RECORDING_DEFAULT_SEARCH_MARGIN` (`m`) on each side and clamped to the base
  (`default_search`), so a mark nearer an edge than `m` has a box cut off on that side, and the
  middle of the box is not the mark's place. The base place is taken on each axis by that rule read
  backwards, with the box's start `b`, its end `e`, the template's own length `t` (the image's size
  as the package holds it) and the base's length `F`: a box off the start edge (`b > 0`) puts the
  crop at `b + m`; a box at the start edge and off the end edge (`b = 0`, `e < F`) puts it at
  `e − m − t`; and either is taken (`default_growth`) only where the crop lies in the base and the
  recording's own `default_search` grows it back into exactly the box. Anything else is `unknown`
  on that axis: a box across the whole axis, a box of another size (a search area someone
  declared), a template declared over the whole frame (`full_frame`) or relative to another mark
  (`template_relative`), which declare no box. Each mark answers its declared box (`region`), its
  template's size (`template`) and its base place axis by axis (`base{x{at, by}, y{at, by}}`,
  `by` `default_growth` or `unknown`, `at` null where unknown). `dx` and `dy` are the best place's
  offset from the base place, each null on an axis where the base place is unknown. (Before fix
  round 1 the base place was the template centred in the box: a mark 10 pixels from the left edge,
  its box cut off at 0, came out 19 pixels left of its place and `other`.)
- **Classes.** The places are `start` (`dx` 0), `centre` (`⌊R / 2⌋`, as the middle of section 3)
  and `end` (`R`). A found mark whose `dy` lies within the tolerance of 0 (or is unknown) and whose
  `dx` lies within it of a place is at that place; any other found mark with a `dx` is `other`
  (the offsets say where); a mark not found is `missing` (its best score and its best place still
  answered). A found mark whose base place across is unknown has no class (`class` null): it is
  counted under `unknown` and takes no part in the reading. Without room the three places are one:
  a found mark within the tolerance of 0 both ways is `as_mapped`. **Too small:** where the room is
  above 0 and `⌊R / 2⌋ ≤ 2 · tolerance` (the windows of the start and of the centre overlap; those
  of the centre and of the end lie at least as far apart), `room_too_small` is true and no mark is
  classed or counted: every `class` is null and every count 0. With the default tolerance 4 a room
  of 17 is too small (`⌊17 / 2⌋ = 8 ≤ 8`) and a room of 18 is not (`9 > 8`); a tolerance of 5 makes
  18 too small (`9 ≤ 10`).
- **Per page and overall.** Each page answers its marks, its `not_probed` and a summary: how many
  marks it searched and how many were found, the counts by class (and `unknown`), the distinct `dx`
  of its found marks with their counts (an inset shows as a value beside 0 or beside `R`), and one
  line of reading. The reading reads the found marks that **take part**: a known base place
  across, not `repeated`, not `forbidden`. In this order: "the room is too small to tell"; no
  template mark the page looks for (forbidden marks aside), "no template mark to probe"; none of
  them found, "not this page"; found marks but none that takes part, "no found mark to read: its
  base place across is unknown or it is repeated"; without room, "as mapped: a frame of the base's
  ratio" when every mark that takes part is `as_mapped`, else "not where the base puts them: the
  offsets say where"; every one at the centre, "a centred base-ratio window"; otherwise, joined by
  "; ", "edges are followed" (marks at the start and at the end; "the start edge is followed" or
  "the end edge is followed" for one of them) and "an inset of about N" for each distinct inset (an
  `other` mark within the tolerance down or with an unknown `dy`, inward of an edge and nearer to
  it than to the centre, `N` its distance from that edge), or, where none of these holds, "neither
  a centred window nor a followed edge: the offsets say where". The reading describes the
  measurement; nothing reads it as a decision. `overall` is the summary of every template mark of
  the chosen pages, each once, as the first page that names it answers it (its rule there).
  The answer's first member, `pages_by_share`, orders the chosen pages by the share of their
  template marks found, the highest first, pages of one share in the package's order: the frame
  shows one page, and the caller reads the best.
- **What the answer holds.** Mark and page ids, rectangles, offsets, scores and thresholds,
  counts, the frame's size, the base's, the factor and the scale frame's size, the room, the
  tolerance, the places, the classes and the readings; no pixel, no text read from the frame and no
  crop of it, so that an answer about a frame that shows account data is safe to paste. Its
  members: `pages_by_share[{page, found, probed}]`, `frame{width, height}`, `base`, `factor{num,
  den}`, `scale_frame`, `room`, `tolerance_px`, `places{start, centre, end}`, `room_too_small`,
  `pages[{page, marks[{id, rule, region, template{width, height}, base{x{at, by}, y{at, by}},
  best, score, threshold, matches, repeated, dx, dy, class}], not_probed[{id, rule, kind,
  region}], summary}]` and `overall`; a summary is `{probed, found, counts{as_mapped, start,
  centre, end, other, missing, unknown}, dx_values[{dx, count}], reading}`.
- **What is not changed.** The kernel's frame gate (section 8), the host's input and the adapter's
  admission (section 10) and every run: a run on a frame wider than the base's ratio is refused at
  its first frame exactly as before. The recognition pack scores as before: fix round 1 made its
  `scored_mask` and `searched_factors` public and wrote the order of a template's threshold once
  (`RecognitionPack::template_threshold`), which its own evaluations, the kernel's selection check
  and the probe call. No interface revision moves and no configuration key is added.
