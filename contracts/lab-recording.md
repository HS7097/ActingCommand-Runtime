# Lab recording (`record start` / `mark` / `status` / `stop`, `--record`)

A Lab recording turns a sequence of real screens into a linear-steps task package: each
step is one screen with its recognition marks and at most one effect, a click rectangle or
an application operation (R24), and the last step only recognizes. This contract
(Workflow #336) covers the recording state, the commands that build it, the six mark
families (#411 Lab B-9 added the value test) with their mark-time self-test, steps and
transitions, remediation, application
steps, optional steps, `--record` on the device commands and the recording lock, and then
the package `record stop` generates, checks and writes ("Package generation"). Workflow #449
REC-ARCH: a recording kept only as material is ended with `record archive` and made active again
with `record reopen` ("Archiving"), and a swipe sent with `--record` is a step ("`--record` on
device commands"). #463 L1: `do --capture --record` records a whole step in one call, its marks,
its press and the next step's frame ("One call per step"). #463 R1: a step may choose in a list of
fixed slots by the name read in each slot, its click the slot it chooses ("Selecting in a list").
#463 R2: a stopped recording takes an ending from a frame, a screen that ends the business another
way after its last press, without recording again ("Endings"). #463 R4: it also takes an optional
step after the effect of one of its steps, a pop-up after which the path goes on, from a frame and
without recording again ("Inserting an optional step after stop"); and #464 P4: `record stop` keeps
the output an earlier stop wrote, refuses a `--lab-dir` inside an installed root and answers the
skeleton of the adoption ("Writing", "Output"). #463 S1-1: `tap`, `swipe` and `long-tap --record`
record a press by the command that makes it, with the screen before it and the screen after it, no
rectangle and no carrier package ("Recording while playing"). #463 S1-7: a recording holds its
instance: `record start` pauses the instance's scheduling with a reason of its own and the end of
the recording resumes only that pause ("A recording holds its instance").

The recording state lives in the Lab state directory. Nothing in this document writes to
the Runtime ledger; the clicks and application operations themselves are executed by the
Runtime and recorded there as any `do --capture` or `session app` is, and (#463 S1-7) a
recording's hold is a scheduling pause the Runtime sets, lifts and records as every pause.

## Files

```
<state>/record-<instance>.json                               the `record` command file (unchanged)
<state>/record-<instance>.lock                               recording lock (operating-system lock, empty file)
<state>/record-<instance>.lock.json                          {pid, command, acquired_at_unix_ms} of the holder
<state>/record-artifacts/<record_id>/lab/recording.json      actingcommand.lab-recording.v1
<state>/record-artifacts/<record_id>/lab/frames/<sha256>.png frames, content-addressed
<state>/record-artifacts/<record_id>/lab/crops/<sha256>.png  template crops, content-addressed
<state>/record-artifacts/<record_id>/lab/out/<package_id>.<zip|json>  the package record stop generated
<state>/record-artifacts/<record_id>/lab/archive-manifest-<n>.json      actingcommand.lab-recording-archive.v2 (Workflow #449; #463 S1-5: v1 on disk is read)
```

`<instance>` and `<record_id>` use the file-stem rule of the `record` command: ASCII
letters, digits, `-`, `_` and `.` are kept, every other character becomes `_`.

The `record` command file is never written by the Lab recording; the step actions (`step`,
`candidates`, `amend`) keep their behaviour. The Lab recording
reads its `record_id`, `task_id`, `instance`, `status` and `started_at_unix_ms`, and
`recording.json` stores the first four (`record_started_at_unix_ms` for the last). Every
command compares them; a difference makes the Lab part unavailable
(`record_lab_unavailable`, reason `lab_recording_mismatch`). A `recording.json` that does not
decode is unavailable the same way, reason `lab_recording_unreadable`; the file is left as it
is (#411 A10).

Every stored frame or crop is read back only after its sha256 matches the recorded value
(`record_frame_hash_mismatch`). Writes are atomic (temporary file, then rename); a failed
command deletes nothing and names the files it already wrote.

## recording.json

```json
{"schema_version":"actingcommand.lab-recording.v1",
 "record_id":"…","record_started_at_unix_ms":0,"task_id":"daily_open","instance":"emulator-0",
 "status":"active",
 "game":null,"server":null,"locale":null,
 "defaults":{"template_threshold":0.95,"color_max_distance":20,"match_metric":"ccoeff_normed"},
 "coordinate_space":{"width":1280,"height":720},
 "created_at_unix_ms":0,"updated_at_unix_ms":0,
 "steps":[{"index":1,"page":"home","dropped":false,"converted_to_transition":false,
   "frames":[{"frame_id":"f0001","role":"primary","path":"…","sha256":"…","width":1280,"height":720,
     "byte_count":0,"source":"local_png","runtime_artifact":null,"capture_backend":null,
     "freshness":null,"recorded_at_unix_ms":0,"superseded":false}],
   "marks":[…],"reused":[],
   "click":null,"click_guard":null,"transition":null,
   "closed":false,"closed_by":null,"initiator":"Lab-CLI"}],
 "artifact":null}
```

- Workflow #434 LAB-2 ([lab-two-entries.md](lab-two-entries.md) §4.2 and §4.3, "Recordings";
  open point 6, the coordinator's ruling): every step and every mark carries `initiator`, the
  word of the process that added it: `Lab-CLI`, or `Lab-制包` / `Lab-救急` from
  `ACTINGCOMMAND_INITIATOR`. The recording lock carries the holder's word, and the save writes it
  on each step and mark the command added; those of earlier commands keep theirs. The member is
  required: a recording written before LAB-2 does not decode (no compatibility in the testing
  stage). A `session record step` of the session file carries it too.

- `index` is a serial number inside the recording, starting at 1 and never reused. Dropped
  steps and steps converted into a transition stay in the file. Every option that names a
  step (`--step`, `--drop-step`, …) uses the serial number. The package numbers the
  effective steps (neither dropped nor converted) again as 1..n; `record status` shows that
  number as `artifact_step`.
- Frame roles: `primary`, `sample`, `transition`, `transition_sample`. Frame sources:
  `local_png`, `lab_capture` (`capture --record`), `runtime_observation`
  (`observe --capture --record`, with the Runtime frame reference in `runtime_artifact`).
- All frames of a recording have one size; it becomes the package coordinate space and
  resolution (`record_frame_size_mismatch`). #463 R2 fix 1: an ending's frame given at a pure
  scale of that size is converted to it, and its frame entry keeps `scaled_from: {width,
  height}`, the size it was given at ("Endings").
- Fields added later use `serde(default, skip_serializing_if)`: `application` (see
  "Application steps") and `optional` (see "Optional steps") are absent on steps without
  one; so are a click's `follow`, a template mark's `scales`, `mask` and `unique`, and a
  self-test's `matched_scales_milli` (see "Following a template", Workflow #396 REC-4); and
  (Workflow #390 LAB-2) a step's `forbidden` and `anchor`, a click's `gesture`, an
  execution's `to_point` and `duration_ms`, and the recording's `amended_at_unix_ms` (see
  "Forbidden marks and anchors", "Drags and long presses" and "Marking after stop"). #411 Lab
  B-9: a value-test mark keeps its declaration in `value_test: {left, op, right?}`, absent on
  every other mark, and has no `region` (see "Value tests"). #463 L2: a located-text mark keeps
  its `locate` in its `ocr` and its margin in `unique`, both absent on every other OCR mark (see
  "Following a located text"), and (fix 1) a click's `follow` keeps
  `names_changed_after_press: true` only when the followed mark's names were changed after the
  click was executed. A build before #463 L2 does not decode a `recording.json` that holds one
  (`ocr` and `follow` refuse the unknown member): its Lab part is unavailable,
  `lab_recording_unreadable`, and the file is left as it is; do not mix versions during a
  recording. #463 R1 (since 0.12.61): a step's `select` is absent on every step without one
  ("Selecting in a list"), so a recording without a select is byte-identical to one an earlier
  build wrote; a build before 0.12.61 does not decode a recording that holds one (the step refuses
  the unknown member), `lab_recording_unreadable` as above. #463 R2 (since 0.12.61): the
  recording's `endings` is absent on a recording without one ("Endings"), so such a recording is
  byte-identical to one an earlier build wrote; a build before 0.12.61 does not decode a recording
  that holds one, `lab_recording_unreadable` as above. Fix 1: a frame's `scaled_from` is absent on
  every frame the Lab did not convert. #463 R4 (since 0.12.63): a step's `inserted_after`, the
  recorded step an optional step inserted after the stop runs after ("Inserting an optional step
  after stop"), is absent on every other step, so a recording without one is byte-identical to one
  written before; the frames of such a step may carry `scaled_from` as an ending's do. #463 S1-1
  (since 0.12.64): a click execution's `before_frame`, `after_frame` and `after_capture` are absent
  on every press but one recorded by the command that makes it ("Recording while playing"), and its
  `sent_on_unstored_capture` on every press but such a press sent on a capture its command did not
  store (fix round 3), so a recording without such a press is byte-identical to one an earlier
  build wrote. #463 S1-2 (since 0.12.64): `after_capture`'s `interval_ms` and `still_ms` are absent
  on a record S1-1's build wrote, which loads as one whose screen after was not tested for
  stillness (`still` `null`); its `unfinished` is absent on every record but one whose after loop a
  failed capture ended. #463 S1-5 (since 0.12.64): an application operation's completed receipt
  keeps `executed_at_unix_ms`, when it was recorded (the Lab's clock, as a click execution's),
  absent on every operation an earlier build recorded ("Archiving", "The manifest",
  `at_unix_ms`).

## Commands

All commands accept the global `--json` and `--instance`. `record` and `session record`
are the same command. Errors use the 0.2 envelope and the exit codes 2 usage, 3 safety, 4
device, 5 Runtime/state, 6 not implemented.

### record start

```
record start --task-id <id> [--locale <l>] [--metric ccoeff_normed|ccorr_normed]
             [--template-threshold <0..1>] [--record-id <id>] [--force] [--holder …] [--lease-id …]
             [--state-dir <dir>]
```

The `record` command file is written as before. The Lab recording is created next to it
with status `active`. Game and server come from the global options or the instance
configuration and may be left open; the locale must be given explicitly before stop.
Defaults: metric `ccoeff_normed`, template threshold 0.95, color distance 20.

The Lab part is `unavailable` (the session itself still starts) when the task id does not
match `^[a-z0-9][a-z0-9_]{0,63}$` (reason `task_id_invalid_for_lab_package`: the id names
the package task folder) or when the Lab directory of the record id already exists (reason
`record_id_reused`, for example `--record-id X --force`). A session started by v0.9.0
gets its Lab recording on the first `record mark`, with the defaults.

Output adds `lab_recording{status, reason?, path, record_id, defaults}`,
`record_flag_state_dir` (the state root `--record` commands use: `ACTINGLAB_SESSION_STATE_DIR`
or the default) and `record_flag_reachable` (whether this state root is that one; when it is
not, frames can only be added with `record mark --frame`).

#463 S1-7: `record start` holds the instance (its scheduling paused with the recording's own
reason, or another party's pause it finds), keeps the hold in the session header (`hold`; fix
round 2: written before the pause is sent, put back when the Runtime refuses the pause) and
answers `hold{state, reason?, taken_over?, paused?, warnings?, runtime_error?, replaced_header?}`;
it waits as the pause waits, up to 335 s; a pause the Runtime refuses starts nothing ("A recording
holds its instance").

### record mark

```
record mark [--step <n>] [--frame <png>] [--sample <png>]… [--page <name>]
            [--template <id>=x,y,w,h[@x,y,w,h]]… [--scales <f>,…] [--color <id>=x,y,w,h]… [--reuse <id>]…
            [--forbid <id>]… [--anchor <id> | --no-anchor]
            [--click x,y,w,h | --click-from <id> | --click-follow <id> | --click x,y,w,h --click-follow <id>
             | --click x,y,w,h --drag-to x,y,w,h --duration-ms <ms> | --long-press x,y,w,h --duration-ms <ms>
             | --click x,y,w,h --brake-corner x,y,w,h]
            [--click-guard <id>] [--click-retry <n>]
            [--replace-click] [--remove <id>]… [--optional [--settle-ms <0..60000>] | --not-optional]
            [--dry-run] [--state-dir <dir>]
record mark [--step <n>] [--frame <png>] [--template …]… [--color …]… [--reuse <id>]…
            --application <launch|restart|stop|force-stop> [--replace-click]
record mark --step <k> --transition none|page|window [--frame <png>] [--sample <png>]…
            [--template …]… [--color …]… [--reuse <id>]… [--transition-timeout-ms <ms>]
            [--min-ms <a> --max-ms <b>] [--replace-transition]
record mark --drop-step <n> | --reopen-step <n> | --close-step | --to-transition <n>
record mark --ending --frame <png> --outcome-key <key> [--sample <png>]… [--page <name>]
            [--template <id>=x,y,w,h[@x,y,w,h]]… [--scales <f>,…] [--color <id>=x,y,w,h]… [--reuse <id>]…
record mark --remove-ending <key>
record mark --optional-after <k> --frame <png> [--sample <png>]… [--page <name>]
            [--template …]… [--scales <f>,…] [--color …]… [--reuse <id>]…
            (--click x,y,w,h | --click-from <id> | --click-follow <id> | …) [--settle-ms <0..60000>]
record mark --remove-inserted <n>
record mark --request <file> | --request-json <json>
```

The request form (`actingcommand.lab-record-mark.v1`) carries every capability, including
the color digest, OCR, check and value-test families:

```json
{"schema_version":"actingcommand.lab-record-mark.v1",
 "step":null,"frame":"D:\\frames\\f1.png","samples":[],"page":"home",
 "add":[{"id":"ui/start","family":"template","region":{"x":1180,"y":664,"width":73,"height":25}},
        {"id":"state/start","family":"color","region":{"x":1173,"y":680,"width":1,"height":1}},
        {"id":"hud/strip","family":"color_digest","region":{"x":0,"y":640,"width":1280,"height":80},
         "columns":16,"rows":2,"max_mean_milli":1500,"max_cell":12},
        {"id":"text/start","family":"ocr","region":{"x":1180,"y":664,"width":73,"height":25},
         "languages":["en"],"timeout_ms":1000,"match_mode":"contains","expected":["START"],
         "case_sensitive":false,"minimum_confidence":0.8,"model_ref":"model","model_sha256":"<64 hex>"},
        {"id":"check/ready","family":"check","all_of":["ui/start","state/start"]}],
 "reuse":[],"remove":[],
 "click":{"from":"ui/start"},"click_guard":null,"retry":null,"replace_click":false,
 "transition":null,"replace_transition":false,"step_action":null,"application":null,
 "optional":null,"optional_settle_ms":null}
```

A value test (#411 Lab B-9, request form only): a value read at the left region must be at
least 25000, and a second one at most twice a capacity read at the right region:

```json
{"schema_version":"actingcommand.lab-record-mark.v1","step":2,
 "add":[{"id":"value/count","family":"value_test",
         "left":{"region":{"x":1040,"y":20,"width":120,"height":28},"languages":["en"],
                 "timeout_ms":1000,"minimum_confidence":0.8,"model_ref":"model",
                 "model_sha256":"<64 hex>","trim":"whitespace_v1",
                 "value":{"type":"unsigned_integer","min":0,"max":9999999}},
         "op":"ge","right":{"const":25000}},
        {"id":"value/pair","family":"value_test",
         "left":{"region":{"x":600,"y":20,"width":80,"height":28},"languages":["en"],
                 "timeout_ms":1000,"minimum_confidence":0.8,"model_ref":"model",
                 "model_sha256":"<64 hex>","trim":"whitespace_all_v1",
                 "value":{"type":"unsigned_integer","min":0,"max":999,"format":"current_capacity"},
                 "part":"current"},
         "op":"le",
         "right":{"region":{"x":600,"y":20,"width":80,"height":28},"languages":["en"],
                  "timeout_ms":1000,"minimum_confidence":0.8,"model_ref":"model",
                  "model_sha256":"<64 hex>","trim":"whitespace_all_v1",
                  "value":{"type":"unsigned_integer","min":0,"max":999,"format":"current_capacity"},
                  "part":"capacity","mul":2,"add":0}}]}
```

A page transition after the click of step 3, a window transition, clearing it, and one step
operation:

```json
{"schema_version":"actingcommand.lab-record-mark.v1","step":3,
 "transition":{"kind":"page","frame":"D:\\frames\\loading.png","samples":[],
   "add":[{"id":"load/bar","family":"color","region":{"x":600,"y":358,"width":8,"height":4}}],
   "reuse":[],"timeout_ms":null},
 "replace_transition":false}
```
```json
{"schema_version":"actingcommand.lab-record-mark.v1","step":3,
 "transition":{"kind":"window","min_ms":1000,"max_ms":3000}}
```
```json
{"schema_version":"actingcommand.lab-record-mark.v1","step":3,"transition":{"kind":"none"}}
```
```json
{"schema_version":"actingcommand.lab-record-mark.v1","step_action":{"kind":"drop_step","step":4}}
```

The other step operations are `{"kind":"reopen_step","step":n}`, `{"kind":"close_step"}` and
`{"kind":"to_transition","step":n}`. Unknown fields are refused at every level of the request
(`validation_failed`, exit 2).

A click that follows a template mark, and a template mark with factors, a mask and a margin of
uniqueness (Workflow #396 REC-4, see "Following a template"):

```json
{"schema_version":"actingcommand.lab-record-mark.v1","frame":"D:\\frames\\f1.png",
 "samples":["D:\\frames\\f1_moved.png"],
 "add":[{"id":"ui/a","family":"template","region":{"x":40,"y":300,"width":64,"height":64},
         "search":{"x":0,"y":280,"width":640,"height":120},
         "scales":{"factors_milli":[1000,750]},"mask":{"type":"range","lower":150,"upper":255},
         "unique":{"margin_milli":50}}],
 "click":{"follow":"ui/a"}}
```

`"click":{"region":{…},"follow":"ui/a"}` declares the offset form.

A click that follows a located-text mark, the card whose name is `BETA` in a row of four cards,
on a screen its title template recognizes (#463 L2, see "Following a located text"):

```json
{"schema_version":"actingcommand.lab-record-mark.v1","frame":"D:\\frames\\cards.png",
 "samples":["D:\\frames\\cards_shuffled.png"],
 "add":[{"id":"ui/title","family":"template","region":{"x":40,"y":40,"width":200,"height":48}},
        {"id":"text/card","family":"ocr","region":{"x":100,"y":200,"width":600,"height":240},
         "languages":["en"],"timeout_ms":1000,"match_mode":"exact","expected":["BETA"],
         "case_sensitive":false,"minimum_confidence":0.8,"model_ref":"model","model_sha256":"<64 hex>",
         "locate":{"normalize":"identity_v1","blocks":"detect","line_merge":{"gap_factor":0},
                   "cells":{"x":0,"y":0,"width":140,"height":240,"pitch_x":150,"pitch_y":240,
                            "columns":4,"rows":1}},
         "unique":{"margin_milli":50}}],
 "click":{"follow":"text/card"}}
```

A page that forbids a mark of another step and is anchored by its own template, and the
gestures (Workflow #390 LAB-2, see "Forbidden marks and anchors" and "Drags and long presses"):

```json
{"schema_version":"actingcommand.lab-record-mark.v1","frame":"D:\\frames\\f2.png",
 "add":[{"id":"ui/title","family":"template","region":{"x":40,"y":20,"width":64,"height":24}}],
 "forbid":["ui/popup_close"],"anchor":"ui/title",
 "click":{"region":{"x":600,"y":500,"width":80,"height":80},
          "drag_to":{"x":600,"y":160,"width":80,"height":80},"duration_ms":400}}
```
```json
{"schema_version":"actingcommand.lab-record-mark.v1","step":3,
 "click":{"long_press":{"x":620,"y":340,"width":40,"height":40},"duration_ms":800},
 "replace_click":true}
```

A drag with a vertical brake (#411 Lab BD, see "Drags and long presses"):

```json
{"schema_version":"actingcommand.lab-record-mark.v1","step":3,
 "click":{"region":{"x":1000,"y":520,"width":60,"height":60},
          "brake_corner":{"x":200,"y":500,"width":60,"height":60}},
 "replace_click":true}
```

A path drag in a box (Workflow #406 T4, see "Drags and long presses"):

```json
{"schema_version":"actingcommand.lab-record-mark.v1","step":3,
 "click":{"region":{"x":400,"y":300,"width":480,"height":240},
          "path_drag":{"turns":4,"min_segment_px":60,"duration_ms":2000,"corner_hold_ms":50}},
 "replace_click":true}
```

`"clear_anchor": true` (`--no-anchor`) clears the step's anchor; `remove` takes a forbidden
mark off the page again.

Rules:

- `--request` cannot be combined with the shortcut flags; single-valued flags given twice,
  unknown flags and positional arguments are refused (`validation_failed`, exit 2).
- Order: locate the step, the frame, remove, add, reuse, samples, click, self-test. All
  marks pass or nothing is written (`record_mark_rejected`, exit 3, `details.marks[]` with
  one `{id, reason, …}` per refused mark). `--dry-run` runs everything and writes nothing
  (`marks_validated`).
- Mark ids: 1–128 bytes without surrounding whitespace or control characters, not starting
  with `page/` or (#463 R1, the ids of a select's generated targets) `select/`
  (`record_mark_id_reserved`), unique among the marks of the effective steps
  and their transitions (`record_mark_id_conflict`). A removed or dropped mark frees its id.
- Template crops become `assets/<id lowercased, characters outside [a-z0-9_.-] replaced by
  _>.png`; two ids with one asset name are refused (`record_asset_name_conflict`).
- `--reuse <id>` adds a mark of another effective step to this step and tests it on this
  step's frames. The step's marks are its own marks plus the reused ones.
- `--sample <png>` adds a frame of the same screen; every mark of the step is tested on it.
- `--remove <id>` removes the entry only; a mark reused elsewhere, named by a check or used
  as the click guard is refused (`record_mark_in_use`).
- `--page <name>` (`^[a-z0-9][a-z0-9_]{0,47}$`) names the step's page.
- Workflow #390 LAB-2: `--template <id>=x,y,w,h@x,y,w,h` gives the template its search area
  (`search`, the part after `@`) as the request form does; only `--template` takes one
  (`validation_failed`, exit 2). `--forbid`, `--anchor`, `--no-anchor`, `--drag-to`,
  `--long-press`, `--duration-ms` and (#411 Lab BD) `--brake-corner` are described in
  "Forbidden marks and anchors" and "Drags and long presses".
- Click: exactly one of a rectangle (`--click`, inside the frame with width and height ≥ 1),
  a mark of the step (`--click-from`, copying the mark's region; for a template that is
  the crop, not the search area; a check or a value test has no region:
  `record_click_source_invalid`), and a
  template mark or (#463 L2) a located-text mark of the step the click follows
  (`--click-follow`, alone or with `--click`; see "Following a template" and "Following a
  located text"). `follow` with `from` is `validation_failed`.
  A second click needs `--replace-click` (`record_click_exists`); an executed click
  cannot be replaced before `--reopen-step` (`record_click_executed`). `--click-guard`
  names a template, color, color digest, check or value-test mark of the step, or (#463 L2
  fix 1) the located-text mark the click follows, which is its guard anyway
  (`record_guard_family_invalid` for any other mark, an OCR mark the click does not follow among
  them, the message naming the mark the click follows; a click that no longer follows the OCR
  mark its guard names is refused the same way). `--click-retry <n>` declares `{max_attempts n (2..5),
  interval_ms 1000}`; the request form takes any `interval_ms` in 1..5000.

Output: `{status, record_id, step, step_opened, frame, samples, marks, reused, removed,
click, application, transition, step_state{marks, frames, effect, transition, closed,
optional, forbidden?, anchor?}, closed_step, dry_run, forbidden?, after_stop?, select?, ending?,
removed_ending?}` (#463 R2: `ending`, the ending the command took, and `removed_ending`, the one it
gave up, are present only then; such an answer names no step, its `status` is `ending_recorded` or
`ending_removed`, `marks_validated` with `--dry-run`; "Endings"; #463 R4: an optional step
inserted after the stop answers as a step, `step` its new serial number, with the status
`optional_step_inserted`, and one given up with `optional_step_removed`, `marks_validated` with
`--dry-run`; its `step_state` carries `inserted_after`, absent on every other step; #463
R1: `select`, the step's select with its self-test, is present only on a step that has one;
Workflow #390 LAB-2: `forbidden` lists the page's forbidden marks tested on the step's frames, and
`after_stop: true` says the recording is stopped and `record stop` will generate its package
again; both are absent otherwise). `step_state.effect` is `none`, `click_declared`,
`click_executed`, `click_indeterminate`, `application_declared` or `application_executed`;
`step_state.optional` is `null` or `{settle_ms}`.

### record status

The existing output, plus `lab`: `null` without a Lab recording, `{status:"unavailable",
reason}`, or

```
{record_id, status, coordinate_space, defaults, open_step,
 steps:[{index, artifact_step|null, page, dropped, converted_to_transition,
         frames[{frame_id, role, sha256, w, h, superseded}],
         marks[{id, family, self_test{status}, margin, value_test?, located_text?}], reused,
         entry: "any" | "page",
         click{rect, source, executed, outcome, attempts, needs_review, gesture?,
               names_changed_after_press?}|null,
         application{action, cli_verb, source, executed, attempts[], needs_review}|null,
         optional: null | {settle_ms},
         transition: null | {kind:"page", frames, marks, timeout_ms, source} | {kind:"window", min_ms, max_ms},
         closed, closed_by, forbidden?, anchor?, select?, inserted_after?}],
 artifact|null, amended_at_unix_ms?, endings?, record_flag_state_dir, record_flag_reachable}
```

#463 R4: `steps` are listed in the order a run meets them: the steps recorded on the path by their
serial numbers, each followed by the optional steps inserted after it after the stop, by theirs (the
same order `artifact_step` numbers); `inserted_after` is present only on such a step ("Inserting an
optional step after stop").

#463 R2: `endings` is present only on a recording that holds one, as `recording.json` holds them
("Endings").

#463 R1: `select` is present only on a step that chooses in a list, as `recording.json` holds it
(its `declaration`, `marked_at_unix_ms` and `self_test`; "Selecting in a list"); its click's
`source` is `select`.

Workflow #390 LAB-2: `gesture`, `forbidden` and `anchor` are present only on a drag or long
press, a page that forbids marks and an anchored page; `amended_at_unix_ms` only on a stopped
recording marked again since its package was generated ("Marking after stop"). #411 Lab B-9:
`value_test` is present only on a value-test mark, its declaration ("Value tests"). #463 L2 fix 1:
`names_changed_after_press: true` is present only on a click whose followed located-text mark had
its names changed after the click was executed: its `rect` is where the new names are found, and
the recorded press is not what the package presses ("Following a located text"). #463 L2 fix 2:
`located_text` is present only on a located-text mark (an OCR mark that declares `locate`): the
mark as the recording holds it, every member a request that changes only its names gives again.

`click.outcome` is `declared` (no outcome yet), `executed` (Performed) or `indeterminate`
(the outcome of the last `do --capture --record` is unknown); `attempts` counts the earlier
outcomes moved aside by `--reopen-step`.

Workflow #449 REC-ARCH: for an archived recording (its session header `archived`) `record.status`
and `lab.status` are both `archived`: the instance has no active recording. The step table, which
archiving never rewrites, still says `active`; `lab.status` gives the session's state.

#463 S1-7: `hold{state, reason?}`, what the recording holds, read from the session header alone
(no Runtime is asked): `own`, `foreign` with the other party's reason, `none`, or `kept` ("A
recording holds its instance").

### record stop

Without Lab steps `record stop` behaves as before and adds `lab: null`; an empty active Lab
recording is stopped with the session. With Lab steps it generates, checks and writes the
package: see "Package generation". A build without the generator (#336 L3, L3b, L3c) refuses
with `record_stop_generation_not_implemented` (exit 6) and leaves both states active; the #336
L4 build refuses a recording with an optional step the same way (`details.reason:
"optional_steps"`). #463 S1-7: a stop that ended the recording (no dry run) then releases the
recording's own hold and adds `hold` when the recording held one; a refused stop keeps it ("A
recording holds its instance").

## Marks and the mark-time self-test

Every mark is tested when it is recorded, on every live frame of its step (primary and
samples, not superseded frames). The result is `passed`, `failed` or `not_evaluated`; any
`failed` refuses the batch. A step with one frame shows `single_sample: true`.

| family | fields | self-test |
|---|---|---|
| template | `region` (crop), `search?` (must contain it and lie in the frame; Workflow #390 LAB-2: by default the crop grown by 48 px on each side and clamped to the frame, never the crop alone where the frame leaves room, so a control that moved a little still matches; `--template <id>=x,y,w,h@x,y,w,h` or the request's `search` replaces it), `threshold?`; Workflow #396 REC-4: `scales?` (`{"factors_milli"}`, REC-5; `--scales f,…` gives every `--template` of the command these factors), `mask?` (`{"type": "range", "lower", "upper"}`, REC-7), `unique?` (`{"margin_milli"}`, REC-2), as the task's `verify_templates[]` declares them, never added on their own | the crop from the primary frame is matched in `search` with the recording metric; every frame needs score ≥ threshold; `min_score`, `margin` = min score − threshold, `matched_rect`, `location_ambiguous` (a match elsewhere than the crop still passes). A mark with `scales`, `mask` or `unique` is evaluated on every frame as the Runtime evaluates the target the package declares for it (a recognition pack `0.7` template target; a declaration its rules refuse is `validation_failed`): with `scales`, `matched_scales_milli` lists the factors it passed at, and an own mark must pass at every factor on at least one frame of its step (put a sample of each zoom in the step); with `unique`, a second place within the margin fails the frame and the failure gives it as `uniqueness{score_milli, second_score_milli, margin_milli, second_rect}`, so the author can set the margin |
| color | `region`, `max_distance?` | `expected` is the mean color of `region` on the primary frame; every frame's mean color must be within the distance; `margin` = limit − largest distance |
| color_digest | `region`, `columns`, `rows`, `max_mean_milli`, `max_cell?`, `exclude_cells?` (no defaults) | `cells` from the primary frame (`color_digest.v1`); every frame must pass; `margin` = `{mean_milli, max_cell}` limits minus the worst values |
| ocr | `region`, `languages`, `timeout_ms`, `match_mode` (`exact`/`contains`), `expected`, `case_sensitive`, `minimum_confidence`, `model_ref`, `model_sha256`, all required; #463 L2: `locate?` (`normalize`, `blocks`, `line_merge`, `cells?`, as the task's `ocr_targets[].locate` declares them) and, with it, `unique?` (`{"margin_milli"}`), never added on their own | the OCR target the package declares for the mark is evaluated on every frame with the Lab's OCR provider (see "OCR in the Lab"); every frame must pass; a provider or evaluation error is `failed` with `self_test_error`. Without an OCR model it is `not_evaluated` (`lab_ocr_unavailable`), and not evaluated is never reported as passed. #463 L2: a located-text mark passes a frame when one of its names is found there (in a cell when `cells` is declared, alone within the margin when `unique` is declared); a frame on which it does not is `self_test_failed` with the evaluation's typed reason as `failure` (`text_locate_not_found`, `text_locate_outside_cells`, `target_match_not_unique` with the second place as `uniqueness`), and an evaluation error (`text_read_blocks_truncated` among them) is `self_test_error` with its outcome as `failure`; a declaration the Runtime refuses is `validation_failed` (see "Following a located text") |
| check | `all_of` or `any_of`: 2..8 distinct marks of the step, no checks | derived: all_of fails on any failed member and passes when all pass; any_of passes on any passed member and fails when all fail; otherwise not evaluated |
| `value_test` | `left`, `op`, `right?` (each side an OCR read: `region`, `languages`, `timeout_ms`, `minimum_confidence`, `model_ref`, `model_sha256`, `trim`, `value`, `part?`; the right side may be `{const}` or a side with `mul` and `add`) | the value test the package declares for the mark is evaluated on every frame with the Lab's OCR provider; every frame must hold; `value_test_unresolved` / `value_test_not_met` are the failure's details; without an OCR model it is `not_evaluated` (`lab_ocr_unavailable`) |

Shape errors (missing or misplaced fields, an invalid grid) are `validation_failed` (exit
2); a region outside the frame (`region_outside_frame`), an invalid search area
(`search_invalid`), a check member that is not a mark of the step (`check_member_invalid`)
and failed self-tests are reasons inside `record_mark_rejected`.

### Value tests

#411 Lab B-9 (Alice 2026-10-19: 「Lab的资源制作应当包含所有可用后端的识别以及触控」). A
`value_test` mark (request form only, as the color digest, OCR and check families) is the
value test of task schema `0.9` ([resource-declarations.md](resource-declarations.md),
section Task schema `0.9` value tests) with its sides read inline: each side is an OCR read of
its own region. Its members follow the Runtime's rules: `op` is `eq`, `ne`, `lt`, `le`, `gt`,
`ge` or `parses`; `parses` has no `right` and every other `op` needs one; `right` is `{"const":
n}` with `0 <= n <= 2^53-1` or a side with `mul` (1 to 1,000,000) and `add` (`-(2^53-1)` to
`2^53-1`), both required there and refused on `left`; `trim` is `whitespace_v1` or
`whitespace_all_v1`; `value` is `{"type": "unsigned_integer", "min", "max", "format"?}` with
`min <= max` and the format `ascii_decimal` by default; `part` (`current` or `capacity`) is
given exactly when the format is `current_capacity`; each side has non-empty `languages`,
`timeout_ms > 0`, `minimum_confidence` in 0..=1, a `model_ref` and a lowercase 64-hex
`model_sha256`. The mark has no `region`, `search`, `threshold` or any other family's field.
Anything else is `validation_failed` (exit 2); a side region outside the frame is
`region_outside_frame` inside `record_mark_rejected`.

- **Self-test.** On every live frame of the step the Runtime's evaluator evaluates the mark
  over a one-test pack (its side OCR targets and the value test) with the Lab's OCR provider,
  as an OCR mark's self-test does. It is `passed` only when the test holds on every frame; a
  frame on which it does not hold refuses the batch with reason `self_test_failed` and the
  details `{frame_id, failure, value_test}`: `failure` is the evaluation's outcome,
  `value_test_unresolved` (with `value_side` and `value_problem`) or `value_test_not_met` (with
  `value_op`), and `value_test` its evidence. A provider failure is `self_test_error`. Without
  an OCR model it is `not_evaluated` (`lab_ocr_unavailable`), never passed. The read texts and
  values appear only in these details and are not stored.
- **Recording and use.** `recording.json` keeps the declaration as `value_test: {left, op,
  right?}`; the mark output and the steps of `record status` show it. A check may name it as a
  member, and `--click-guard` may name it.
- **Refusals.** Where the Runtime refuses a value test, the Lab refuses the mark: another page
  cannot forbid it, directly or through a check (`validation_failed`, details `{mark_id,
  family}`); it is never an anchor (`anchor_invalid`, `not_own_template`), never a click source
  (`record_click_source_invalid`: it has no region) and never a followed template
  (`record_click_follow_invalid`, `not_template`). Its side targets take the ids
  `<id>/left` and `<id>/right`; a mark whose id or side id another mark id (or side id) of the
  recording already takes is refused with `record_mark_id_conflict`.
- **Use.** To act only when a value holds, mark the value test on an optional step's page
  ("Optional steps").

### Following a template

Workflow #396 REC-4 (REC-M v1.2.1 §6). A click may follow a template mark of its step (own or
reused) instead of clicking a fixed rectangle, for a button whose position moves:

- `--click-follow <id>` (`"click":{"follow":"<id>"}`): the package clicks the box the template
  matches on the frame it runs on, `{"kind": "target", "target_id": "<id>"}`. The step's
  `click.rect` is the crop and `click.source` is `followed_mark`.
- `--click x,y,w,h --click-follow <id>` (`"click":{"region":{…},"follow":"<id>"}`): the package
  clicks the matched box's top-left corner plus the offset, `{"kind": "offset", "target_id":
  "<id>", "offset": {x, y, width, height}}`, where the offset is the rectangle minus the crop's
  top-left corner (width and height kept). For a template with `scales` the kernel scales the
  offset by the factor it matched at. `click.rect` is the declared rectangle, `click.source`
  `declared`.
- `recording.json` records `click.follow{mark_id, form: "target" | "offset", offset?,
  names_changed_after_press?}` (the last only on a located-text follow, #463 L2 fix 1). The
  recorded rectangle is what `do --capture --record` executes and `record status` shows.
- The click's guard is the followed template: `target_id` the template, `verify_template` its
  asset, `expected_rect` its search area. A step without `follow` generates the same bytes as
  before, and no generated click is ever `target_center`.
- A template is not made `unique` for being followed; declare `unique` with a margin when a
  second copy could appear, using the second place its self-test reports.

`record_click_follow_invalid` (exit 3; `details.outcome` names `mark_id` and `follow_problem`)
refuses the command, and nothing is written, when the declaration cannot hold; the rules are
checked again on every `record mark` of the step:

| `follow_problem` | when |
|---|---|
| `not_template` | the followed mark is neither a template mark nor (#463 L2) a located-text mark of the step |
| `not_required` | the template is a member of an `any_of` check of the step, so it is not a required mark of the page (the guard comes from the required marks); #463 L2: so is a followed located-text mark that an `any_of` check of the step names |
| `guard_conflict` | `--click-guard` (or the step's guard) names another mark |
| `page_needs_followed_text` | #463 L2 fix 1, a click that follows a located-text mark: the step's page would need the located text to be recognized, because no other mark of the step recognizes the page or a check of the step names the located text ("Following a located text") |

The followed template cannot be removed while the click follows it (`record_mark_in_use`). On
every live frame of the step (the primary frame and the samples) the click must hold too;
otherwise the batch is refused under `record_mark_rejected` with one `{id: <the template>,
reason: "record_click_follow_invalid", frame_id, outcome, …}` for the first frame that does
not hold:

| `follow_problem` | when |
|---|---|
| `primary_mismatch` | on the primary frame the template's best match is not the crop itself (an earlier equal match elsewhere wins), or a template with `scales` matches there at another factor than 1000 |
| `template_unmatched` | on a frame the template does not pass in its search area |
| `click_outside_frame` | on a frame the followed box (with the scaled offset) does not lie inside the frame |
| `press_outside_found_box` | #463 L2, a click that follows a located-text mark: on the primary frame the press of the step's executed click lies outside the box the names are found in there ("Following a located text") |

The search area should cover every position the template moves to; with a search area equal
to the crop the generated box is the recorded rectangle. Whether the whole box is harmless to
click on every frame is for the author to check on the frames.

### Following a located text

#463 L2 (the owner's rule: an item is chosen by its name read on the screen, never by its
position; it is confirmed on a fresh frame before it is pressed; a name that is not found ends
with a named result, never with a guess). A click may follow a located-text mark instead of a
template: one screen, the name found on it, no scrolling.

- **The mark.** An OCR mark of the request form that declares `locate` is a located-text mark:
  the task's locate target ([ocr-fields.md](ocr-fields.md), "Located text") member for member.
  `locate` has `normalize` (`identity_v1`), `blocks` (`detect` or `region`), `line_merge`
  (`"none"` or `{"gap_factor": n}`, exactly with `detect`) and the optional `cells`
  (`x`, `y`, `width`, `height`, `pitch_x`, `pitch_y`, `columns`, `rows`, the grid relative to the
  region's corner); the mark may add `unique` (`{"margin_milli"}`, 1 to 1000). Its `expected`
  names, `match_mode`, `minimum_confidence`, `case_sensitive` (false, as normalized names are
  compared) and model are the OCR mark's own. Every member is the author's; the Lab adds no
  default the task format does not give. The Runtime's declaration rules are applied when the
  mark is recorded, with or without an OCR model: a declaration they refuse (a `line_merge`
  with `region`, a grid that leaves the region, `case_sensitive: true`, a name that normalizes
  to nothing) is `validation_failed` (exit 2). `locate` or `unique` on another family, and
  `unique` on an OCR mark without `locate`, are `validation_failed` too. There is no shortcut
  flag: the request form (`record mark --request` or `--request-json`, MCP `ac_record_mark`)
  declares the mark, and `--click-follow` may then follow it.
- **The click.** `"click": {"follow": "<id>"}` clicks the box the names are found in on the
  frame the package runs on: the found block, or the declared cell holding the block's center
  when `cells` is declared (`{"kind": "target", "target_id": "<id>"}`). The step's `click.rect`
  is that box on the primary frame, `click.source` `followed_mark`. `"click": {"region": {…},
  "follow": "<id>"}` clicks that box's top-left corner plus the offset (`{"kind": "offset",
  "target_id", "offset"}`), the offset being the rectangle minus the box's corner on the primary
  frame, width and height kept, never scaled; `click.rect` is the declared rectangle,
  `click.source` `declared`. The offset is fixed when the click is declared; on every later
  `record mark` of the step `click.rect` is the box the primary frame gives (the found box, or
  its corner plus the offset), which `do --capture --record` presses.
- **The guard, not the page** (fix 1, the coordinator's ruling; "the wanted thing is not on the
  screen today" must not look like "the package does not know this screen"). The followed
  located-text mark is the click's target and its guard's target and no mark of the step's page:
  the page rule does not list it, and the step's page is recognized by its other marks. So the
  step needs at least one other mark that recognizes its page (a template, a color, a color
  digest, a check or an OCR mark that is not the followed one); a step without one, and a step
  with a check that names the located-text mark (the page would need the name through it), are
  refused `record_click_follow_invalid` with `follow_problem` `page_needs_followed_text`, checked
  on every `record mark` of the step as the other follow rules are (removing the step's only
  other mark is refused the same way), and nothing is written; `record stop` refuses a recording
  in that state the same way when it generates the package. The Lab cannot judge how well a mark
  recognizes the page; the refusal says what the mark is for (fix 2): it tells this screen from
  the other screens of the task, for example a template of the screen's title or of a fixed
  element, and a mark that also holds on the next screen does not help. The rule of "Following a template"
  that the followed mark is a required mark of the page does not apply; the others hold with the
  followed mark in the template's place: `not_template` names a followed mark that is neither a
  template nor a located-text mark (an OCR mark without `locate` among them), `not_required` one
  that an `any_of` check of the step names, `guard_conflict` a click guard that names another
  mark. `--click-guard` may name the followed located-text mark, the guard the package has
  anyway; another OCR mark is refused `record_guard_family_invalid`, the message naming the mark
  the click follows. The followed mark cannot be removed while the click follows it, except by a
  request that removes it and adds a located-text mark of the same id again, which the click then
  follows (see "The names" below).
- **The self-test**, on every live frame of the step with the Lab's OCR provider, as the Runtime's
  guard evaluates the target: the mark's own self-test (one of its names found, in a cell, unique
  within the margin, with the kernel's typed reason otherwise, see the table above), and the
  step's page recognized on every live frame without it (each of the step's other marks passes on
  every live frame, as every mark's self-test requires); then `record_click_follow_invalid` under
  `record_mark_rejected`, naming the frame, when the click box on a frame does not lie inside it
  (`click_outside_frame`) or, once the step's click was executed, when its press lies outside the
  box the primary frame gives (`press_outside_found_box`: the recording would say one thing and
  the package do another). The press check holds whenever the follow is declared or the mark is
  changed in any way other than its names; a request that changes only the names is not held to
  it (below). Without an OCR model the mark is `not_evaluated` (`lab_ocr_unavailable`) as any OCR
  mark, and a click that follows it cannot be declared or checked: `record mark` on its step is
  refused `lab_ocr_unavailable` (exit 3, `blocked_by: ["ocr_model"]`, the mark as `target_id`) and
  nothing is written.
- **The names are the first form of a variable** (the owner: the variable can also be changed
  after the recording by an agent through the Lab or in the package). An agent changes them, also
  after `record stop` ("Marking after stop"), with one `record mark` on the existing step that
  removes the followed mark and adds it again with the same id and every other member equal and
  only its new `expected`. "Every other member" is every member the recording holds but the
  mark's `self_test`, `created_at_unix_ms` and `initiator`: for a located-text mark its `family`,
  `region` and `unique` and its OCR members `languages`, `timeout_ms`, `match_mode`,
  `case_sensitive`, `minimum_confidence`, `model_ref`, `model_sha256` and `locate`, which the
  request gives at its top level. The agent reads them in `record status`, which shows a
  located-text mark as the recording holds it in `steps[].marks[].located_text` (fix 2; the OCR
  members there are under `ocr`); elsewhere only `recording.json` and the `marks` of the
  `record mark` answer that added the mark hold them:

  ```json
  {"schema_version":"actingcommand.lab-record-mark.v1","step":1,"remove":["text/card"],
   "add":[{"id":"text/card","family":"ocr","region":{"x":100,"y":200,"width":600,"height":240},
           "languages":["en"],"timeout_ms":1000,"match_mode":"exact","expected":["GAMMA"],
           "case_sensitive":false,"minimum_confidence":0.8,"model_ref":"model","model_sha256":"<64 hex>",
           "locate":{"normalize":"identity_v1","blocks":"detect","line_merge":{"gap_factor":0},
                     "cells":{"x":0,"y":0,"width":140,"height":240,"pitch_x":150,"pitch_y":240,
                              "columns":4,"rows":1}},
           "unique":{"margin_milli":50}}]}
  ```

  Such a request is the change of the wanted value, not of the demonstration: the press check is
  not applied to it, nor to a later `record mark` that leaves the mark as it is. The self-test
  still needs one of the new names on every live frame of the step, by the same rules, and the
  step's `click.rect` becomes the box the new names give on the primary frame. When the step's
  click was executed, the click's `follow` records `names_changed_after_press: true`, which
  `record mark`'s `click` and `record status` show (the recorded press is then not what the
  package presses), and `record stop`'s cross-check lists the step in
  `names_changed_after_press` (see "Package generation"). A request that changes any other member
  of the mark (its region, its cells, its `locate`, its `minimum_confidence`, any member listed
  above) is held to the press again, and a passing one clears the record; when its press lies
  outside the box, the `press_outside_found_box` rejection lists the members besides the names in
  which the request differs from the recorded mark, by name, as `differing_members` (fix 2).
  `record mark --reopen-step` clears the record too, the press becoming an attempt. A
  name that is on no recorded frame cannot be declared yet (`text_locate_not_found`): record a
  sample that shows it; a later slice, the variable's, lifts this limit.
- **The package** (see "Package generation"): the mark is an `ocr_targets[]` entry with its
  `locate` and `unique`, named by no page rule; the click step's guard is `{"page_id",
  "target_id": "<id>", "expected_rect": <the region>, "verify_text": "<id>"}`, and the step
  declares `"input_recheck": {"kind": "fresh_frame"}` ([linear-steps.md](linear-steps.md),
  Workflow #411 B12; [resource-declarations.md](resource-declarations.md), "Task schema `0.9`
  observation windows and input rechecks"): after the guard passed on the step's frame, one fresh
  frame is captured, only the step's page is recognized on it, the guard (the locate target) is
  evaluated on it again, and the input is drawn from that frame's found box. A name that is not on
  the screen at run time leaves the step's page recognized by its other marks, and the guard
  refuses: the run fails with `contained_task_guard_refused` and the typed cause
  (`cause=text_locate_not_found` or the other typed reason), with no input, as a hand-written
  package does; the same on the fresh frame of the recheck. A located-text click has no normal end
  for "not found"; a select has one (#463 R1, "Selecting in a list").

A screen with a row of four cards, the wanted card found by its name: the request above
("record mark") declares the title template `ui/title`, which recognizes the screen, and
`text/card` over the row, the four cards as cells, the name `BETA` and a margin of 50, and the
click on the found cell. On the primary frame `BETA` stands in cell 1, so
the step's click is cell 1 (`{"x": 250, "y": 200, "width": 140, "height": 240}`); on the sample,
where the cards were shuffled, it stands in cell 3 and the self-test passes there too, its cell
inside the frame. The package declares:

```json
"ocr_targets": [{"id": "text/card", "region": {"mode": "rect", "rect": {"x": 100, "y": 200, "width": 600, "height": 240}},
  "languages": ["en"], "timeout_ms": 1000, "match_mode": "exact", "expected": ["BETA"],
  "case_sensitive": false, "minimum_confidence": 0.8, "model_ref": "model", "model_sha256": "<64 hex>",
  "locate": {"normalize": "identity_v1", "blocks": "detect", "line_merge": {"gap_factor": 0},
             "cells": {"x": 0, "y": 0, "width": 140, "height": 240, "pitch_x": 150, "pitch_y": 240, "columns": 4, "rows": 1}},
  "unique": {"margin_milli": 50}}],
"operations": [{"id": "step_01_click", "from": "step_01", "to": "step_02",
  "click": {"kind": "target", "target_id": "text/card"},
  "guard": {"page_id": "step_01", "target_id": "text/card",
            "expected_rect": {"x": 100, "y": 200, "width": 600, "height": 240}, "verify_text": "text/card"},
  "input_recheck": {"kind": "fresh_frame"}, "…": "…"}],
"page_rules": {"step_01": {"required": ["ui/title"]}, "…": "…"}
```

and its run presses the cell that holds `BETA` on the fresh frame, wherever the card stands; on a
day without `BETA` on the row the screen is still `step_01` and the guard refuses with
`cause=text_locate_not_found`, no input.

### Selecting in a list

#463 R1 (since 0.12.61, [v012-register.md](v012-register.md) §1.381; the owner's rule on board
#463: what stands in a list is chosen by what is read, never by its position; it is confirmed on a
fresh frame before the press; when the wanted item is not there the run ends normally with a
result that has a name, and never guesses; the rules here are the coordinator's). A step may
choose in a list of fixed slots on one screen by the name read in each slot: the request's
`select` (request form only, `record mark --request` or `--request-json`, MCP `ac_record_mark`, and
the mark request of "One call per step"; no shortcut flag). `record stop` generates it as the
select step of a `linear_steps` task ([linear-steps.md](linear-steps.md), "Select steps"). A list
that scrolls and the declaration of the wanted value as a variable are later slices; an ending added
after the stop is #463 R2's ("Endings").

```json
{"schema_version":"actingcommand.lab-record-mark.v1","step":3,
 "add":[{"id":"ui/list_title","family":"template","region":{"x":40,"y":20,"width":160,"height":32}}],
 "select":{
   "slots":[{"rect":{"x":100,"y":200,"width":180,"height":240},"name":{"x":110,"y":400,"width":160,"height":30}},
            {"rect":{"x":300,"y":200,"width":180,"height":240},"name":{"x":310,"y":400,"width":160,"height":30}}],
   "read":{"languages":["en"],"timeout_ms":1000,"minimum_confidence_milli":850,
           "model_ref":"model","model_sha256":"<64 hex>"},
   "names":[{"id":"item_a","aliases":["Item A"]},{"id":"item_b","aliases":["Item B"]}],
   "match":{"max_distance":1,"minimum_margin":1},
   "wanted":{"name":"list_item","default":"item_b"},
   "pressed":"item_a"}}
```

- **The members.** `slots`, 1 to 16 (the projection's OCR budget of one frame), each its `rect`,
  an optional `click` (the box a tap is drawn in, default `rect`) and `name`, the region its name is
  read in; `read`, the OCR members of every name read (`languages`, `timeout_ms`,
  `minimum_confidence_milli` 1 to 1000, `model_ref`, `model_sha256`); `names`, the identity domain:
  every name the list may show, each an `id` of the grammar `^[a-z][a-z0-9_]{0,63}$` (the parameter
  grammar, so a later slice needs no rename) and 1 to 8 `aliases`, the texts it is read as, up to
  1024 names; `match`, the `ocr_aliases` mapping's `max_distance` (0 to 8) and `minimum_margin` (1
  to 9); `wanted`, the wanted value: its `name` (the same grammar, its own in the recording) and its
  `default` (one of the names); `pressed`, the name the agent presses now as a demonstration (absent:
  the default); `on_empty_key`, the key of the run's normal end when the wanted name is on no slot
  (`^[a-z0-9][a-z0-9_]{0,127}$`, default `<task_id>_not_found`, so a second select of the recording
  gives its own: the default would be the first one's, `on_empty_key_taken`); `confirm`,
  `candidate_set` (the kernel's default) or `decision`.
- **What the Lab reads, and what only the kernel admits** (fix 1). The Lab's own reading of the
  request refuses `validation_failed` (exit 2): an unknown member or a wrong type; a slot count
  outside 1 to 16; a rectangle without width or height; an empty `languages` or a blank language, a
  `timeout_ms` of 0, a `minimum_confidence_milli` outside 1 to 1000, a blank `model_ref` or a
  `model_sha256` that is not 64 lowercase hex digits; and the identity domain and `match` as the
  identity declaration validates them (1 to 1024 names, 1 to 8 aliases each that normalize to a
  text, the match's distance and margin). Every other bound of the read is the kernel's alone, and
  the Lab does not copy it: a `timeout_ms` above what an OCR target admits, more languages than it
  takes or a `model_ref` it does not admit passes the Lab's reading and is refused by the kernel's
  admission of the probe package at the self-test, `record_select_invalid` with `kernel_refused`
  (exit 3), the kernel's code in `details.loader_code` and its text in `details.detail`.
- **`clear_select`** (fix 1; request form only, `true` or absent, not with `select`:
  `validation_failed`) takes the step's select off, also after `record stop`: see "The click"
  below. #463 R2: a step without a select has nothing to take off: `record_select_invalid`,
  `no_select`, and nothing is written.
- **The click.** The select is the step's effect: its click (`click.source: "select"`) is the
  click box of the slot the decision selects for `pressed` on the primary frame, which `do
  --capture --record` and the one call press as any recorded click ("One call per step": nothing
  there changed). A click with an outcome on record stays, the demonstration the self-test checks:
  a select declared on a step whose tap was recorded, or declared again after its press (to change
  its default, also after `record stop`). A click without an outcome, the select's own among them,
  is replaced only with `replace_click` (`record_click_exists`, so that a one-call step sent again
  presses nothing twice); a click declared in its place with `replace_click`, and an application
  operation, end the select. `--click-guard` names the select step's guard as a click's.
- **A recorded tap turned into a select** (fix 1), on an active recording or after `record stop`
  ("Marking after stop"): a select declared on a step whose tap has an outcome on record makes that
  tap the select's click (`source: "select"`, its rectangle the click box of the slot the pressed
  name selects on the primary frame) without recording again, when the tap lies in that slot; the
  whole select is self-tested again on the step's kept frames. It is refused when the tap does not
  lie there (`select_press_mismatch`), when the recorded click was a drag or a long press
  (`with_gesture`), followed a mark (`with_follow`) or declares a retry (`with_retry`), and on the
  last step of a stopped recording (`on_final_step`). The tap's own rectangle, source and mark are
  kept beside the select in `recording.json` (`select.click_before`) and carried over when the
  select is declared again.
- **Taking a select off** (fix 1): `clear_select` gives a tap the select took over its own
  rectangle, source and mark back (`click_before`), so the step is the one recorded before the
  select and the next `record stop` generates the package it generated before; the select's own
  click without an outcome goes with the select, with its click guard (the step has no effect
  again). The select's own press, once executed, has no click of its own to give back:
  `record_click_executed` (exit 3; `--reopen-step` first, which a stopped recording does not take).
- **The guard.** A select step needs a guard (its confirmation evaluates it on the fresh frame): the
  guard comes from the page's required marks as a click's does ("Pages and documents"). A page with
  only OCR marks gives none: `record_select_invalid`, `guard_missing`.

`record_select_invalid` (exit 3, `details{step, select_problem, outcome}`) refuses the command and
nothing is written; every rule is checked again on every `record mark` of the step and when `record
stop` generates the package:

| `select_problem` | when |
|---|---|
| `with_follow` | the request's click follows a mark, or the step's recorded click does |
| `with_gesture` | the request declares a drag, a long press, a brake drag or a path drag, or the step's recorded click is one |
| `with_click` | the request declares a click rectangle or a click source beside the select |
| `with_retry` | the request declares a click retry, or the step's click has one |
| `on_optional_step` | the request makes the step optional, or the step is |
| `with_application` | the request or the step declares an application operation (fix 1: also on the application entry step, which has no frame; a select on any other step without a frame is `record_step_frame_missing`, as a mark is) |
| `on_final_step` | the select is marked on the last step of a stopped recording: the package ends on its page, which would then be the select's empty end too (the kernel refuses that, `select_on_empty_target`); on an active recording the last step is the open step and gets its next screen |
| `slot_outside_frame` | a slot's rectangle, click box or name region does not lie inside the frame |
| `name_id_invalid` | a name's `id` breaks its grammar |
| `wanted_name_invalid` | `wanted.name` breaks its grammar (fix 1) |
| `name_duplicate` | two names have one `id` |
| `default_not_in_names` | `wanted.default` is no name's `id` |
| `pressed_not_in_names` | `pressed` is no name's `id` |
| `wanted_name_taken` | another select step of the recording has the same `wanted.name` |
| `on_empty_key_invalid` | `on_empty_key` breaks its grammar |
| `on_empty_key_reserved` | the `on_empty` key is `<task_id>_done`, the key of the run's usual end (fix 1) |
| `on_empty_key_taken` | the `on_empty` key is another select step's of the recording (a second select that leaves `on_empty_key` out takes the first one's default) |
| `guard_missing` | the step's page gives no guard |
| `kernel_refused` | the kernel's admission refuses the select step as `record stop` generates it (`details.loader_code`, the kernel's code, and `details.detail`): a bound of the read only the kernel checks ("What the Lab reads, and what only the kernel admits") |
| `no_select` | #463 R2: `clear_select` names a step that has no select |
| `on_empty_key_of_ending` | #463 R2 fix 1: the `on_empty` key is the outcome key of an ending of the recording, a select declared after the ending ("Endings"); at `record stop` the ending's own check refuses the pair (`record_ending_invalid`, `key_of_select`) |

**The mark-time self-test**, on every live frame of the step, is the kernel's own dry run of the
select step (`dry_run_select`, called, never copied) on a probe package: the step alone as `record
stop` generates it (its page with every mark it names, its layout, slot targets, policy document and
`on_empty` end), followed by a page that never passes, so that the step's page is the one page of
the probe on its frames; its budgets are the largest a package declares, since a budget is checked
by `record stop` only. Its refusals are reasons of `record_mark_rejected`, `{id: "select", reason,
frame_id, …}`, checked once every mark of the step holds:

| reason | when |
|---|---|
| `select_name_unread` | a slot's name reads no known name (nothing read, too low a confidence, a text out of the domain, two names within the margin): `slot` and the typed `cause` of the first, `unread` all of them, and `default`, the default's decision there (fix 1: `blocked` when it selects nothing) |
| `select_pressed_absent` | on the primary frame the pressed name selects no slot (`pressed`; `decision`, its kind) |
| `select_press_mismatch` | the step's press was executed and lies outside the slot the pressed name selects on the primary frame (`slot`, `click_rect`, `point`) |
| `select_other_page_passes` | fix 1: the page of another effective step the mark knows (every step before it; on a stopped recording every other step), or its page transition's page, passes on a live frame of the select step too, each of its marks evaluated as its self-test evaluates it (`step`, the other step's index; `transition`, whether it is the transition's page); the run's select would not know its page there. Pages of steps recorded later are checked by `record stop` (self-check item 8) |
| `self_test_error` | the dry run fails on a frame (`failure`: the kernel's `code` and `detail`), or (fix 1) a mark of another step's page cannot be evaluated on it (`step`, `mark_id`, `error`) |

A sample frame on which the pressed name stands in another slot is accepted: that is the point of
the select. The decision with the wanted value's default is reported on every frame, never a
refusal. Fix 1: each decision is reported by its kind, so that a frame on which the run would fail
shows: `{"result": "selected", "slot"}`, `{"result": "empty"}` (the run's normal end with the
`on_empty` key), `ambiguous`, `insufficient` and `unknown` (the evaluator's kinds of a decision the
run fails on) and `blocked` (an empty or insufficient decision while a slot reads no known name,
which the run refuses, since that slot may hold the wanted name). Without an OCR model the select
is `not_evaluated` (`lab_ocr_unavailable`), never passed, and `record stop` lists it in
`marks_not_evaluated` as `select/<page>`; a select whose step has no recorded press to keep a click
from is then refused `lab_ocr_unavailable` (exit 3), since its click comes from the read names.

**`recording.json`** keeps the step's `select`: `{declaration, marked_at_unix_ms, self_test{status,
reason?, frames[{frame_id, pressed, default}]}, click_before?}`, `pressed` the decision for the
pressed name and `default` the default's, each by its kind as above (on a sample frame without the
pressed name, `pressed` is `empty`); `click_before{rect, source, from}` (fix 1) the tap the select
took over, present only then. A step without a select has no key.

**What `record stop` generates** for a select on step i (page `step_ii`, [linear-steps.md](linear-steps.md)
"Select steps"):

- `task.json` `candidate_layouts[]`: `{"id": "layout/step_ii", "page_id": "step_ii", "kind":
  "fixed_slots", "features": [{"name": "name", "value": "identity", "identity": {"entries":
  [{"id", "aliases"}…], "recognition": {"kind": "ocr_aliases", "max_distance", "minimum_margin",
  "minimum_confidence_milli"}}}], "slots": [{"rect", "click", "targets": {"name":
  "select/step_ii/slot_kk"}}]}`;
- `ocr_targets[]`: `select/step_ii/slot_kk` per slot, its `name` region and the `read` members,
  `match_mode: "contains"`, `case_sensitive: false`, `minimum_confidence` the read's in milli / 1000,
  and `expected` the names' distinct aliases in declaration order, at most 64 (an OCR target lists
  at most 64 values; nothing reads the target's own verdict: the layout's identity feature maps the
  read text over the whole domain, so a domain with more aliases lists its first 64);
- the operation `step_ii_select`: `from`, `to`, `expect_after` and `post_delay_ms` as a click step,
  `select: {"layout_id": "layout/step_ii", "policy": {"path": "policies/step_ii_select.json"},
  "on_empty": {"outcome_key": K}}` (with `confirm` when declared), the guard, no click and no retry;
- `policies/step_ii_select.json`, the selection-policy document:

  ```json
  {"schema_version": "actingcommand.selection-policy.v1", "policy_id": "step_ii_select",
   "applies_to": {"candidate_layout_id": "layout/step_ii",
     "outcome_keys": {"selected": "<task_id>_step_ii_selected", "empty": "K",
                      "insufficient": "<task_id>_step_ii_insufficient",
                      "ambiguous": "<task_id>_step_ii_ambiguous", "unknown": "<task_id>_step_ii_unknown"}},
   "fields": [{"name": "name", "value_type": {"type": "enum_string", "allowed": ["item_a", "item_b"]}}],
   "facts": [],
   "gates": [{"gate_id": "wanted.list_item",
              "predicate": {"kind": "string_in", "value": {"source": "field", "field": "name"},
                            "allowed": ["item_b"]},
              "on_unknown": {"kind": "drop_candidate"}}],
   "scoring": [], "selection": {"mode": "none_allowed", "required_count": 1},
   "tie_break": [{"kind": "candidate_id", "direction": "lowest_first"}]}
  ```

- `scheduling_outcome`: after `<task_id>_done`, the mapping `{K, no_designated_effect, [step_ii]}`;
- the step's provenance: the declaration, the pressed name, K, `marked_at_unix_ms` and the
  self-test's decisions; the output `lab.selects[{step, record_index, op, layout, wanted, pressed,
  on_empty_key, frames[{frame_id, pressed, default}]}]`, each decision by its kind as at mark time
  (`frames[{frame_id, result: "undetermined"}]` without an OCR model);
- the default task timeout adds `S` (one confirmation capture) per select step.

**The wanted value is a constant of the package** until a later slice declares it as a variable:
`control.json` has no `variables` member for it, and the one gate `wanted.<name>` keeps
`allowed: [<default>]`. It is changed today by `record mark` on the step with the select declared
again with another `wanted.default` (on a stopped recording too, "Marking after stop"), then `record
stop` (`regenerated`); or, in the package, by that one line of the policy document. The gate's
name and the ids already follow the parameter grammar, so that slice rewrites only that gate and
adds the declaration. The recording keeps the wanted value's name in `select.wanted.name`; the
package's gate is `wanted.<name>` and its field is `name`, the layout's identity feature.

**Self-checks** (see "Self-checks"): admission by the kernel (the select-step rules of
[linear-steps.md](linear-steps.md)), the first decision of a select at step 1 and item 8.

**Which to use.** A control found by its fixed label whose position moves (a "Confirm" button):
a click that follows a located text ("Following a located text"). An item chosen by a value that
may change, where "not there" is a normal result: a select. A list that scrolls is a later slice.
The located text's `expected` names were the first form of the variable; since R1 the variable
belongs to the select.

**`package` revision.** A `linear_steps` task with a select step needs `package` 13
([component-interfaces.md](component-interfaces.md), row 13). `record stop` writes no revision: a
content directory declares none, and the revision of a bundle is the packaging's (the resource
repository's, and since Workflow #464 P2 the Lab's own `standard-pack build`, whose revision table
counts a select step as 13, [resource-verify.md](resource-verify.md), "The `package` revision");
an earlier Runtime refuses such a package at admission (`contained_task_linear_invalid`,
`reason=operation_effect`).

### Endings

#463 R2 (since 0.12.61, [v012-register.md](v012-register.md) §1.384; the owner's requirement on
board #463 that the business is done with its last press, that every declared ending is a normal
end and that a rare ending can be added later without recording again; the coordinator's rulings 4
and 5 on the design of board #463 and the rulings of its work order). A business may end otherwise
after its last press (too little balance, nothing to collect). A stopped recording takes such a
screen as an **ending**, from a frame and without recording again, and `record stop` generates it as
an ending page of the `linear_steps` task ([linear-steps.md](linear-steps.md), "Endings"): a run
whose last press reaches that screen finishes Completed on it, keyed by the ending's key, and its
pause written for that screen (Workflow #463 U1) lifts once the package that holds the ending is in
place.

```json
{"schema_version":"actingcommand.lab-record-mark.v1",
 "ending":{"frame":"D:\\frames\\short.png","samples":[],"page":"short",
           "outcome_key":"daily_open_short",
           "add":[{"id":"ui/short_dialog","family":"template","region":{"x":480,"y":260,"width":320,"height":48}}],
           "reuse":[]}}
```
```json
{"schema_version":"actingcommand.lab-record-mark.v1","remove_ending":"daily_open_short"}
```

- **The members.** `frame`, the ending's screen, a PNG file on the caller's machine (required);
  `samples`, more frames of it; `page`, the name of its page (the grammar of a step's `--page`,
  `validation_failed` otherwise); `outcome_key`, the key of the run's end on it (required,
  `^[a-z0-9][a-z0-9_]{0,127}$`); `add`, its marks, of every family, made from its frame as a step's
  are; `reuse`, marks of steps its page requires too. The command form is `record mark --ending
  --frame <png> --outcome-key <key> [--sample <png>]… [--page <name>] [--template …]… [--color …]…
  [--reuse <id>]…`: `--ending` makes these flags the ending's; `record mark --remove-ending <key>`
  gives an ending up. The MCP tool `ac_record_mark` takes the request form and hands it to this
  command unchanged, so the two faces answer the same.
- **Alone.** An ending is no step and changes none: its request names nothing else, no step, no
  frame, mark or click of a step, no step operation (`with_other_change`, `details.members` naming
  them); nothing of it is cut away.
- **Its frames** are stored as a step's are, content-addressed in the recording's `frames/` with the
  next `frame_id`, and have the recording's size: fix 1, a frame given at a pure scale of it is
  converted to it first ("Where the frame comes from", below), and any other size is
  `record_frame_size_mismatch`. The caller's path is written nowhere: `recording.json` names the
  stored copy, the package and its provenance the frame's sha256.
- **Its marks** are self-tested on its frames, the primary one and the samples, as any mark is
  (`record_mark_rejected` when one fails, nothing written). Its own marks share the recording's ids
  and asset names (`record_mark_id_conflict`, `record_asset_name_conflict`); a step's mark it reuses
  is in use (`record_mark_in_use` for a `remove` of it). Its marks carry `step` 0, the number of no
  step.
- **`recording.json`** keeps `endings[{outcome_key, page, frames, marks, reused, marked_at_unix_ms,
  initiator}]` in the order they were taken, absent without one. Taking or giving up an ending sets
  `amended_at_unix_ms`, and the next `record stop` generates the package again (`regenerated`).
  `remove_ending` takes the ending out of the list (its stored files stay, as every stored file
  does), so that the next `record stop` generates the package it generated before.
- **What `record stop` generates** ("Pages and documents"): the page `ending_kk` or
  `ending_kk_<page>` whose rule requires the ending's marks, its marks' targets, and its
  `scheduling_outcome` mapping `{outcome_key, "no_designated_effect", [its page]}` after the usual
  ending's and the select steps'; then self-check item 9 ("Self-checks"). `record stop` writes no
  `package` revision; a `linear_steps` task with an ending page needs `package` 15
  ([component-interfaces.md](component-interfaces.md), row 15), and an earlier Runtime refuses it
  at admission (`contained_task_linear_invalid`, `reason=terminal_page`).

`record_ending_invalid` (exit 3, `details{outcome_key, ending_problem, outcome}`) refuses the
command and nothing is written; the key rules are checked again when `record stop` generates the
package, and `ocr_alone_unchecked` is `record stop`'s alone:

| `ending_problem` | when |
|---|---|
| `recording_active` | the recording is active: an ending is taken, or given up, after `record stop`, which records the usual ending first |
| `with_other_change` | the request names a member of a step beside `ending` or `remove_ending`, or both of these (`details.members`) |
| `key_invalid` | the outcome key breaks `^[a-z0-9][a-z0-9_]{0,127}$` |
| `key_reserved` | the outcome key is `<task_id>_done`, the key of the run's usual end |
| `key_of_select` | the outcome key is a select step's `on_empty` key (fix 1: a select declared after the ending with its key is refused where it is declared, `record_select_invalid` with `on_empty_key_of_ending`; `record stop` checks the key again) |
| `key_exists` | another ending of the recording has the outcome key |
| `key_unknown` | `remove_ending` names a key no ending has (`details.endings`, the keys there are) |
| `page_exists` | another ending of the recording has the page name |
| `no_marks` | the ending adds and reuses no mark |
| `ocr_alone_unchecked` | fix 1, `record stop`'s self-check item 9: the ending's page rests on OCR marks alone (every mark it requires is an OCR mark or a value test, or a check whose members all are) and the Lab has no OCR model, so whether it passes on a screen the run may show instead could not be checked (`details{page, against, frames, step \| other_outcome_key}`, the first such screen); an ending that passed there would turn a failure into a silent success |

**Where the frame comes from.** A run that met a screen its package never recorded ends with
`contained_task_linear_unrecorded_screen` ([linear-steps.md](linear-steps.md), "A screen the
package never recorded"); its `artifact_id` names the frame of the run's `Failure` pin, the closing
decision's last frame. That id is no material ref: `run evidence <request_id>` (actinglab and
actingctl, MCP `ac_run_evidence`) lists the run's frames as material refs
`<seq>:<event_id>:<artifact_id>`, the run's last one as `final.frame_ref`, and the ref whose last
part is the failure's `artifact_id` is the frame; `material <ref> --out <dir>` (actinglab) writes
it whole as `<dir>/<sha256>.png`, the file `--frame` takes (MCP `ac_material` with `mode: export`
writes the same file into the server's materials folder). Take it in time: a pinned frame nobody
confirmed as read is removed once `frame_retention_evidence_max_age_days` (a top-level key, 15 by
default) have passed since its ingestion, and one confirmed as read (`evidence confirm-read`) at
the next pass ([ledger-store.md](ledger-store.md), the evidence-frame rule of the frame cleaner).

The pinned frame is the device's own capture: on an instance whose resolution differs from the one
the recording was made at, it has another size. Fix 1 (the coordinator's ruling): the Lab converts
it itself, as the Runtime converts every frame before it recognises anything
([display-adapter.md](display-adapter.md), section 8). When the display adapter's map of the frame
onto the recording's size is a pure scale (the recording's ratio, larger, neither cropped nor
turned; the map built without a display declaration, as the Lab's dry runs map), the frame is
resampled once to the recording's size with the REC-5 primitive (area average): the two calls the
kernel's frame gate makes (`DisplayMap::build`, `resample_pixels`), called, never copied. The
converted frame is the ending's frame, stored, hashed and self-tested as such; its entry in
`recording.json` and in the answer keeps `scaled_from: {width, height}`, the size it was given at.
The marks' regions are in the recording's pixels, on the converted frame the answer's `frame.path`
names. Any other size is refused as before (`record_frame_size_mismatch`, `details{expected,
actual}`): take the frame on an instance whose resolution is the recording's or a pure scale of
it. Give the file `material` wrote as it is: a frame scaled by another program cannot be told from
a pinned one, and a run would compare it with a frame the Runtime scaled its own way.

**What the kernel's admission asks of an ending** ([linear-steps.md](linear-steps.md), "Endings"):
none of its rules can refuse a generated ending page, which is a page of its own name, in one
mapping, off the path, no transition, loading or wait page, in a package whose control declares
no `store_update` (`terminal_page`, `ending_page`, `terminal_page_duplicate`,
`store_update_endings`). A key that the usual ending, a select step or another ending already has
is refused at the mark with its own word. A package with an ending is no prerequisite and no
return-home package (`contained_task_prerequisite_incompatible`, `reason=linear_endings`): that
binds a later use of the package, and `binding_requires` says so.

**Beside a select.** An ending stands after the last press and is no step; a select stands on no
last step of a stopped recording (`on_final_step`): the two never meet on one step. A select step
before the last press whose tap reaches the ending's screen ends the run on it
([linear-steps.md](linear-steps.md), "Endings"). An ending page that passes on the frames of the
step before the last press is item 9's `previous`, a select step's or not; one that passes on the
frames of an earlier select step is item 8's refusal (the run's select would not know its page
there). Fix 1: a select declared after an ending whose `on_empty` key is the ending's key is
refused where it is declared, `record_select_invalid` with `on_empty_key_of_ending` ("Selecting in
a list").

**An archived recording** takes no ending: `record mark` on it is `record_session_not_active`, as
every `record mark` of a session that is not active ("Other commands on an archived recording");
reopened, it is active (`recording_active`) until `record stop` generates its package.

**What cannot be added this way:** an ending that needs a press to close (its message is left on
the screen, and the next task's start clears it: the owner's default); a result after any press but
the last; a branch that continues the business; a pop-up after which the path goes on (#463 R4: an
optional step inserted after the stop, "Inserting an optional step after stop"); an ending that
looks like the screen before the last press, the last step or another ending on the recorded frames
(item 9).

### Forbidden marks and anchors

Workflow #390 LAB-2 (forced-compromises mapping row LAB-2, compromise L-02). A step's page may
forbid marks and may be anchored by one of its templates; a page transition only requires its
marks (`--forbid`, `--anchor` and `--no-anchor` with `--transition` are `validation_failed`).

- `--forbid <id>` (`"forbid": [...]`, repeatable) adds a mark the page must not show: an own
  mark of another effective step, as `--reuse` takes one. An id no step has is refused with
  reason `forbidden_source_missing` inside `record_mark_rejected`; a mark the page itself
  requires (an own or reused mark of the step) is `validation_failed`. #411 Lab B-9: forbidding
  a value test, or a check with one as member, is `validation_failed` (details `{mark_id,
  family}`), as the page detector refuses a value test in `forbidden` (a test that cannot read
  does not pass, which there would let the page match unseen; require the opposite comparison
  instead). `--remove <id>` takes a forbidden mark off the page again.
- On every `record mark` of the step, each forbidden mark is evaluated on every live frame of
  the step as its self-test evaluates it (a check from its members on that frame). One that
  passes on a frame refuses the batch with reason `forbidden_mark_passes` and the `frame_id`; an
  evaluation that fails is `self_test_error` (with `forbidden: true`); an OCR mark without an OCR
  model is not evaluated. The output lists them in `forbidden[{id, family, status, frames}]`,
  `status` `absent` or `not_evaluated`.
- A mark another page forbids is in use on its own step: removing it, dropping its step and
  turning its step into a transition are refused as for a reused mark.
- `--anchor <id>` (`"anchor": "<id>"`) makes an own template mark of the step the page's anchor;
  `--no-anchor` (`"clear_anchor": true`) clears it, and giving both is `validation_failed`. The
  anchor must be an own template mark the page requires: otherwise the batch is refused with
  reason `anchor_invalid` and `problem` `not_own_template` (a color, color digest, OCR, check,
  value-test or reused mark, or no mark of the step; a value test is never an anchor) or
  `not_required` (a member of an `any_of` check of the
  step). The anchor cannot be removed while it anchors the page (`record_mark_in_use`).
- A step that forbids marks or has an anchor cannot become a transition
  (`record_to_transition_invalid`, reason `page_rule`).
- `recording.json` keeps `forbidden: [ids]` and `anchor: id` on the step; `step_state` and the
  steps of `record status` show them when present.

### Drags and long presses

Workflow #390 LAB-2. A step's effect may be a drag or a long press instead of a tap:

- `--click x,y,w,h --drag-to x,y,w,h --duration-ms <ms>` (`"click": {"region", "drag_to",
  "duration_ms"}`) declares a drag that presses a point of the click rectangle, moves to a point
  of the destination rectangle and lifts there after the duration; a fast drag is a swipe.
- `--long-press x,y,w,h --duration-ms <ms>` (`"click": {"long_press": {"x", "y", "width",
  "height"}, "duration_ms"}`) declares a long press in that click box, held for the duration;
  the box is the step's click rectangle (Workflow #390 lpbox, Alice's rule of 10-09 that pack
  taps use click boxes: size it to the pressed control, so that every point of it is harmless).
  A bare point `x,y` is `validation_failed`; `x,y,1,1` is the box of one pixel.
- The duration is 1..=60000 ms. A drag needs its rectangle, its destination and a duration; a
  long press its box and a duration and no `--click` rectangle; neither takes `--click-from` or
  `--click-follow`; every rectangle lies inside the frame. Anything else is `validation_failed`
  (exit 2). Guard, retry and `--replace-click` apply as to a tap.
- #411 Lab BD: `--click x,y,w,h --brake-corner x,y,w,h` (`"click": {"region",
  "brake_corner"}`) declares a single-touch drag with a vertical brake: it presses a point of
  the click rectangle, moves to a point of the corner box, holds there and brakes 100 px
  straight up before it lifts, with the Runtime's fixed timings (horizontal move 200 ms, hold
  at the corner 150 ms, brake 200 ms; 550 ms in all), so it takes no duration. Both boxes lie
  inside the frame with width and height ≥ 1, and the corner's `y` is at least 100, so that the
  brake ends inside the frame (pack containment's bound). With `--duration-ms`, `--drag-to`,
  `--long-press`, `--click-from` or `--click-follow` (or their request members) it is
  `validation_failed` (exit 2); so is a corner whose `y` is less than 100 or a box outside the
  frame. Guard, retry and `--replace-click` apply as to a tap. A Lab operation does not press it
  (see "`--record` on device commands"); the package declares it as the Runtime runs it.
- Workflow #406 T4: the request form `"click": {"region", "path_drag": {"turns",
  "min_segment_px", "duration_ms", "corner_hold_ms"}}` declares a single-touch path drag in the
  click rectangle, its box ([resource-declarations.md](resource-declarations.md), "The path
  drag"): one touch that turns `turns` times inside the box, each segment at least
  `min_segment_px` long, over `duration_ms`, holding `corner_hold_ms` at each inner corner; the
  kernel generates its path when a package run performs it. The box lies inside the frame with
  width and height ≥ 1, and the numbers within the path drag's bounds (`turns` 1..=30,
  `min_segment_px` 1..=⌊(max(width, height) − 1) / 2⌋, `duration_ms`
  `turns + 1 ..= (turns + 1) × 60000`, `corner_hold_ms` 0..=60000); it takes no `duration_ms`
  of the click, `drag_to`, `long_press`, `brake_corner`, `from` or `follow`. Anything else is
  `validation_failed` (exit 2, a conflicting member named as `argument` with `argument_problem`
  `conflicting`, numbers outside their bounds `out_of_range`). The request form is the one way to
  declare it (`record mark --request` or `--request-json`; MCP `ac_record_mark`). Guard, retry and
  `--replace-click` apply as to a tap; it lasts `duration_ms` plus `turns × corner_hold_ms` in the
  task timeout. A Lab operation does not press it, as the brake drag.
- `recording.json` records `click.gesture`: `{"kind": "drag", "to", "duration_ms"}`,
  `{"kind": "long_press", "duration_ms"}`, (#411 Lab BD) `{"kind":
  "single_touch_drag_with_vertical_brake_v1", "corner"}` or (Workflow #406 T4) `{"kind":
  "single_touch_path_drag_v1", "turns", "min_segment_px", "duration_ms", "corner_hold_ms"}`;
  `record status` shows it on the click.
- Workflow #390 lpbox: the generated `long_press` click declares the box, and the kernel holds a
  point it picks inside the box, as it taps a `rect` click (the `long_press` paragraph of
  [resource-declarations.md](resource-declarations.md)). Every generated click is a box.

### OCR in the Lab

Workflow #396 LAB-3. actinglab and the Lab load the OCR provider the Runtime uses: the
in-process `ppocr-ctc` engine over the model folders (`models\<model_ref>\`) of the vision root
that the Runtime configuration's `vision` section names (`root`, else `vision` beside the install
root), always on the CPU whatever `vision.execution_provider` says. actinglab reads the
configuration its installation selects, or, on a single-location root
(`runtime\BUILD-MANIFEST.json` two levels above it, no install selection; Workflow #411 A12),
`<root>\actingd.config.json`; outside both there is none. As in the Runtime, model files are read
and ONNX Runtime is initialised on the first OCR request. The mark
self-test, `record stop`'s first decision and cross-check, `observe`, `recognize`, `is-visible`,
`detect-page`, `current-page`, the guard target of `do`/`wait` and the `package dry-run` replay
evaluate OCR targets with it. #411 Lab B-9: value-test marks are OCR: their self-test, `record
stop`'s first decision and cross-check read their sides with the same provider. #463 R1: so is a
select: its self-test, `record stop`'s first decision and self-check item 8 read the slots' names
with it ("Selecting in a list").

#411 Lab L6: the provider serves the root's NN model folders as well, so the same commands
evaluate NN targets with it; `observe` evaluates an action's NN target when the provider holds
its model, and otherwise leaves it not evaluated (`dynamic_target_not_evaluated`), as before. A
root whose folders hold NN models and no OCR model gives the provider too, as the Runtime accepts
such a root: NN targets evaluate, while OCR is unavailable as below, caused by
`lab_ocr_model_missing`. `observe` also fills the values (`fields`) of every target it
evaluated, as the online observation does ([page-projection.md](page-projection.md)).

Without a usable OCR model OCR is unavailable: `lab_ocr_unavailable`, whose `caused_by` link says
why (`lab_ocr_vision_unconfigured`, a configuration read or decode code,
`vision_root_unavailable`, `vision_models_empty`, `lab_ocr_model_missing` or
`lab_ocr_runtime_missing`). It is never skipped silently, and it fails only a command whose
purpose is to evaluate an OCR target:
- `recognize`, `is-visible`, `observe --targets` and the guard target of `do`/`wait` on an OCR
  target (directly, as a composite member, or as the side of a value test, Workflow #396 REC-6),
  and `observe`, `detect-page` and `current-page` on a page set with such a target, refuse with
  `lab_ocr_unavailable` (exit 3, `blocked_by: ["ocr_model"]`, `details.outcome` the outcome with
  the OCR target as `target_id`) before evaluating anything;
- an OCR or value-test mark's self-test and `record stop`'s first decision report it as their
  reason (#463 L2 fix 1: the first decision also when step 1's page reads no OCR but its click
  follows a located-text mark, whose guard reads it); #463 L2: `record mark` on a step whose
  click follows a located-text mark refuses with it (the click's box comes from the OCR,
  "Following a located text");
- an `observe` action whose target is an OCR target or a value test is not evaluated, as before,
  and the output names it in `ocr.not_evaluated`, also beside a provider that serves NN models
  only;
- `package dry-run` keeps its decision and exit code; `capabilities`, `package dry-run` and that
  `observe` output report the OCR state (`available`, `execution_provider: "cpu"`, `vision_root`,
  and `unavailable` with the outcome).

`package bundle` never loads the provider.

## Steps, frames and remediation

A frame arrives either from a `--record` command (device frame) or from `record mark
--frame` without `--step` (offline frame):

| open step | device frame | offline frame |
|---|---|---|
| none | a new step is opened with the next serial number | same |
| open, no marks and no click | the frame replaces it; the old frames are marked `superseded` | same |
| open, marks but no click | `record_step_click_missing` (3): a loading or other intermediate screen becomes a transition with `record mark --to-transition n`; a final step is checked with `record stop --dry-run` and then stopped | same |
| open, a declared click that was not executed | `record_step_click_not_executed` (3) | the step is closed (`closed_by:"offline_frame"`) and a new one opened |
| open, a click with an indeterminate outcome | `record_step_click_not_executed` (3): check the screen, then `--reopen-step k` to execute it again or `--close-step` to accept it | same |

An offline frame byte-identical (same sha256) to the open step's live primary frame is not an
arrival: `record mark --frame` without `--step` then targets that open step exactly as
`--step n` would, and no frame is added (`step_opened:false`, `closed_step:null`).

- The open step is the last effective step while it is not closed.
- `record mark --step k` adds marks or samples to any effective step, closed or not; a
  different primary frame is refused (`record_step_frame_conflict`).
- Without an open step and without `--frame`, `record mark` refuses with
  `record_step_frame_missing`.
- Remediation acts on the last effective step only (`record_step_not_last`) and deletes no
  file:
  - `--drop-step n` marks step n `dropped`; its marks free their ids; the previous step is
    unchanged. A mark of step n reused by another step is refused (`record_mark_in_use`).
  - `--reopen-step n` moves the recorded click outcome into `click.attempts[]` and opens
    the step again; the click can be executed again or replaced.
  - `--close-step` closes the open step that has a declared click (not executed, or with an
    indeterminate outcome) as `closed_by:"author"`.
  - `--to-transition n` turns step n (the last effective step: no click, not optional, at
    least one mark) into the page transition of the previous effective step m (which has a
    click and no transition yet). Its frames (roles `transition`/`transition_sample`), marks and reused
    ids move into `steps[m].transition` with `source:"converted_from_step"`,
    `converted_step:n`; step n stays in the file with `converted_to_transition:true`. The
    next frame opens a new step. Any unmet condition is `record_to_transition_invalid`.
- Step operations cannot be combined with marks, clicks, samples, frames, pages or
  transitions.

Typical uses: a swallowed click — `--drop-step` the extra step, `--reopen-step k`,
`do --capture --record`, `capture --record`. A loading screen captured as a step — mark it,
then `--to-transition` it. A transition that cannot be captured or recognized — declare a
window.

### Transitions

A transition belongs to an effective step k with an effect, a click or an application
operation (declared or executed), and describes the screens between that effect and the
next step
(`record_transition_without_click`, exit 3, otherwise; `--step k` is required).

- `--transition none` clears it; its marks free their ids.
- `--transition page`: the frame, samples and marks of the same command belong to the
  transition. Every page declaration, a replacement included, needs `--frame` and at least
  one mark; a transition is changed by declaring it again whole with
  `--replace-transition`, never piecemeal. Its marks follow
  the step rules and share the id space, but they are not marks of any step: they cannot
  be a click source or guard. A click in the same command is `record_transition_has_click`
  (exit 2). `--transition-timeout-ms` is 1..1800000.
- `--transition window --min-ms a --max-ms b` needs 0 ≤ a ≤ b and 1 ≤ b ≤ 1800000
  (`record_transition_window_invalid`, exit 2).
- An existing transition is replaced only with `--replace-transition`
  (`record_transition_exists`, exit 3).

## `--record` on device commands

Only `capture`, `observe --capture`, `do --capture` with a point, rectangle or (Workflow #390
LAB-2) swipe input,
`session app` / `session instance app` with `launch`, `restart`, `stop` or `force-stop`
(see "Application steps") and (#463 S1-1) `tap`, `swipe` and `long-tap` (see "Recording while
playing") accept `--record`; every other command or combination is refused before it runs:

| code (exit 2) | when |
|---|---|
| `record_flag_unsupported` | offline commands, `capture diagnose`, element clicks, `observe --capture --dry-run`, `do --dry-run`, `session app --dry-run` (global or after the command, including the instance alias), `session app` without one of its four actions, #463 S1-1: `tap`/`swipe`/`long-tap --record --dry-run` (global or after the command), `key`, `text` and every other command |
| `dry_run_unsupported` | `capture --record --dry-run` and its `session capture` alias, global or after the command |
| `record_flag_takes_no_value` | `--record <value>` |
| `record_state_dir_unsupported` | `--record` with `--state-dir`; `--record` uses `ACTINGLAB_SESSION_STATE_DIR` or the default state root |
| `validation_failed` | `--tap-rect` without `--record`; #463 L1: so are `--mark-request`, `--mark-request-json`, `--next-frame-after-ms` and `--next-frame-out` ("One call per step"); #463 S1-1: and `--after-ms`, `details{argument: "after_ms", reason: "record_only"}` ("Recording while playing"); #463 S1-2: and `--still-ms` and `--still-max-ms`, and on `tap`, `swipe` and `long-tap` `--interval-ms` (other commands take a flag of that name for their own), `argument` `still_ms`, `still_max_ms` or `interval_ms`; the train of stage one (#463 S1-3): on `session app` and `session instance app` `--interval-ms` too, the text naming `session app <launch\|restart\|stop\|force-stop> --record` where it names `tap, swipe or long-tap --record` on every other command |

The record instance (`--instance` or the instance configuration) and the command instance
must be the same (`record_instance_mismatch`, exit 3); a missing or inactive session is
`record_session_not_active`. Every check runs before anything is captured or pressed.
All nine device recording forms (#463 S1-1: `tap`, `swipe` and `long-tap --record` beside the
six before) refuse `--dry-run` before recording locks, Runtime access, frame attachment or
recording persistence. The global parser handles the flag at any position.

- `capture --record` takes the frame through the Runtime as `capture` does; `--out` becomes
  optional. Output adds `record{status:"frame_recorded", record_id, record_started_at_unix_ms,
  step, step_opened, frame, closed_step}`.
- Workflow #449 REC-ARCH: every action performed with `--record` names in its answer's `record` the
  recording it entered and the step it became: `record_id`, `record_started_at_unix_ms` (when that
  recording started, its session header's `started_at_unix_ms`) and `step`, added beside the
  members each answer had (`capture --record` and `observe --capture --record` above, `do --capture
  --record`'s `record{status, record_id, record_started_at_unix_ms, step, step_closed, click}`,
  `session app … --record`'s `record{status, record_id, record_started_at_unix_ms, step,
  step_opened, step_closed, application}`, also inside the details of their errors). A recording
  belongs to the instance: nothing is refused because another process started it or added its
  steps.
- `observe --capture --record` stores the verified Runtime frame and its reference, and in
  `capture_backend` the backend that captured it, named as `capture --record` names it
  (`adb_screencap`, `droidcast_raw`, `nemu_ipc`, `scrcpy`; Workflow #411 Lab L4).
- `do --capture --record` presses inside the open step's click rectangle: its center, or
  `--tap x,y` when it lies inside (`record_click_outside_step_rect`), or (Workflow #406 T4) with
  `--seed <u64>` the point drawn from that seed with the root configuration's touch keys
  ([resource-declarations.md](resource-declarations.md), "The Lab's draw"; the click is its
  command's first press with the empty label). `--tap-rect x,y,w,h`
  declares the rectangle when the step has none, and must equal the declared one otherwise
  (`record_click_rect_conflict`). Without a rectangle the command refuses
  (`record_step_click_missing`). The click record stores the point, its rule, the carrier
  package reference, the request, correlation, action and lease ids, any failure and the
  before/after frame summaries marked `informational`. The rule names how the Lab pressed:
  `rect_center` when it pressed the center, `explicit` for `--tap` or `--swipe`, and for a drawn
  press the draw's algorithm (`center_hotspot_normal_v1` with the touch keys,
  `xorshift64_uniform_rect_v1` without), with `sampling`, the evidence of the draw (its
  `action_seed`, the boxes, the parameters), beside it. `--seed` with `--tap` or `--swipe`, or
  without `--record`, is `validation_failed`.
  - Performed: recorded; the step is closed.
  - Performed with a failure: recorded, the step closed and marked `needs_review`; exit 4
    `record_click_performed_with_failure`.
  - Not performed: nothing is recorded; the operation's own error is returned.
  - Indeterminate: recorded as the click outcome; the step stays open; exit 4
    `record_click_indeterminate`. Resolve with `--reopen-step` or `--close-step`.
  - A click that was sent but could not be recorded is
    `record_append_failed_after_input` (exit 3) with the operation summary.
  - #463 L1: with a mark request it first marks the open step, and with `--next-frame-after-ms` it
    then takes the next step's frame: "One call per step".
- Workflow #390 LAB-2: on a step that declares a drag or a long press ("Drags and long
  presses"), `do --capture --record` executes it through the same Runtime operation, a drag as
  one swipe and a long press as one long tap. A drag goes from the center of its rectangle to
  the center of its destination over its duration (`rect_center`), or along
  `--swipe x1,y1,x2,y2,ms`, which must start in the one and end in the other
  (`record_click_outside_step_rect`) and last the declared duration
  (`record_click_rect_conflict`), rule `explicit`. A long press is a `LongTap` at the center of
  its click box over its duration (`rect_center`; #411 Lab L3, as the kernel holds the
  `long_press` click the recording generates); `--swipe` on it is
  `record_click_rect_conflict`. `--tap` and `--tap-rect` on such a step are
  `record_click_rect_conflict`; `--swipe` on a tap step is `record_click_rect_conflict`. The
  execution also records `to_point` and `duration_ms`. Without `--record`, `do --capture --swipe`
  is unchanged.
- **Recorded swipes** (Workflow #449 REC-ARCH; before it, `--swipe` on a step without a declared
  click was refused `record_step_click_missing` and nothing entered the step table):
  `do --capture --record --swipe x1,y1,x2,y2,ms` on the open step that declares no effect is that
  step's effect, as `--tap-rect` declares a tap. The step's click is a drag from a box of one pixel
  at the first point to a box of one pixel at the second over the duration: `rect` `{x1, y1, 1,
  1}`, `gesture{kind: "drag", to: {x2, y2, 1, 1}, duration_ms}`, `source: "swipe"`. The input is
  one swipe between the two points (rule `explicit`), recorded as any execution with its point,
  `to_point`, `duration_ms`, request ids and the frames before and after; a Performed swipe closes
  the step as a tap does. Both points lie inside the frame and the duration is 1..=60000 ms, as a
  declared drag's, else `validation_failed` (exit 2) and nothing is pressed; `--swipe` takes no
  `--tap`, `--tap-rect` or `--seed` (`validation_failed`). An archive's manifest gives the step the
  kind `swipe` with its parameters ("Archiving").
- **`record stop` and recorded swipes.** The standard package's task schema expresses such a swipe
  as it is: the `drag` click `{"kind": "drag", "from", "to", "duration_ms"}` with the two boxes of
  one pixel, which the kernel presses as one swipe between their points (a box of one pixel presses
  its point, [resource-declarations.md](resource-declarations.md), "Touch sampling"). `record stop`
  generates a swipe step as it generates a declared drag, counts its duration in the task timeout
  and checks it in its first decision as one; no code is added and the package schema is not
  extended. A recording without swipe steps generates the same bytes as before.
- Workflow #406 T4: with `--seed`, a drag's two ends and a long press's point are drawn in their
  boxes as a run draws them, and the rule names the draw, with `sampling`.
- #411 Lab BD: a brake drag is not pressed by a Lab operation: `do --capture --record` on its
  step is refused with `record_gesture_execution_unsupported` (exit 3, `details{step, gesture}`)
  before any Runtime connection, capture or recording change, whatever `--tap`, `--tap-rect` or
  `--swipe` it gives, and nothing is recorded. Workflow #406 T4: so is a path drag. Reach the screen after the gesture by other
  means, then close its step with an offline frame (`record mark --frame <png>`) or `record mark
  --close-step`; a device frame before that is `record_step_click_not_executed`, as for any
  declared click. The package still declares the gesture.
- observe and do need a carrier package: any admissible package of the same game and
  resolution, including an earlier Lab package.
- #463 S1-7: `record start` pauses the instance's scheduling for the recording and its end
  resumes that pause, so that no scheduled task acts on the instance between two commands ("A
  recording holds its instance"); no separate `pause` and `resume` is needed.

## One call per step

#463 L1 (0.12.60; the owner asked on board #463 that an agent playing through the Lab keeps the
recording on and marks as it goes, and found three calls a step slow; the rules here are the
coordinator's). A step was three calls: its frame arrives (`capture --record`, `observe --capture
--record`), `record mark` declares its marks and its click and self-tests them, `do --capture
--record` presses and closes it. `do --capture --record` now takes the second call and, when asked,
the next step's first in the same call:

```
do --capture --record (--package <p> | --zip <z>) [--instance <i>]
   [--tap x,y | --tap-rect x,y,w,h | --swipe x1,y1,x2,y2,ms | --seed <u64>]
   [--mark-request <file> | --mark-request-json <json>]
   [--next-frame-after-ms <0..60000> [--next-frame-out <png>]]
```

Through MCP it is `ac_lab_do` with `capture`, `record`, `mark` (the request object, passed as
`--mark-request-json`), `next_frame_after_ms` and `next_frame_out`; the Lab's MCP server's `ac_do`
takes the row's parameters (`mark_request`, `mark_request_json`, `next_frame_after_ms`,
`next_frame_out`, [lab-operations.json](lab-operations.json), the row `do`).

- **The mark request.** `--mark-request <file>` or `--mark-request-json <json>`, not both
  (`validation_failed`): one `actingcommand.lab-record-mark.v1` request, the document `record mark
  --request-json` takes (marks, samples, page, the click, its guard and retry, optional, ...), read
  by the same parse, so a member it does not have is `validation_failed`. It is applied by `record
  mark`'s own code: every check `record mark` makes and its mark-time self-test, on the step's live
  frames. A refusal ends the call with `record mark`'s own code and details: nothing is pressed and
  nothing is written.
- **The open step and nothing else** (fix round 1). The request marks the open step, the step the
  press is the click of. A request with `step_action` or `transition`, a request whose `step` is not
  the open step, and a request whose `frame` (an offline file) opens or replaces a step are
  refused, after `record mark`'s checks and the press's plan and before anything is written or
  sent: `validation_failed`, `details{member, open_step}` (`member` is `step_action`,
  `transition`, `step` or `frame`), its message saying to send that request with `record mark`
  (MCP `ac_record_mark`) and then press with `do --capture --record`. A `frame` byte-identical to
  the open step's frame, and `step` equal to the open step, are the open step and pass.
- **The carrier package.** `--package` or `--zip` (MCP `package` or `zip`) is the carrier package
  `do --capture` needs for every Lab operation, as above: any admissible package of the same game
  and resolution, an earlier Lab package included. It carries the Runtime request only; the press
  is the open step's, planned from the recording, never from the package's targets.
- **The press.** Then the press is planned on the open step, by the rules `do --capture --record`
  has (above): its click rectangle's centre, `--tap`, `--seed`, `--tap-rect` (equal to a declared
  click), a declared drag or long press, `--swipe`; the gestures it refuses stay refused
  (`record_gesture_execution_unsupported`), and a step without a click is
  `record_step_click_missing`. Every check of both, and the command's own (with a next frame, its
  device configuration; the package opened and read; the Runtime connection), runs before anything
  is written or pressed. The Runtime's own admission of the operation does not: it answers the
  press, after the marks are written, and a refusal there leaves the recording as a press that was
  not performed leaves it (below).
- **Order.** The marks are written once, as `record mark` writes them, right before the input is
  sent; then the press; then its outcome is recorded as the bullets of "`--record` on device
  commands" say. A press that fails leaves what `record mark` followed by `do --capture --record`
  leaves: not performed, refused by the Runtime, or no answer, the marks recorded with the click
  declared and no outcome, and the operation's own error; performed with a failure,
  `record_click_performed_with_failure`; indeterminate, `record_click_indeterminate`. Every error
  after the marks are written names them in its details (`mark`, the `record mark` output).
- **Repeating a press** (fix rounds 1 and 2, the coordinator's rulings). After a call that wrote
  the marks and recorded no outcome of its press (the press not performed, refused by the Runtime
  or unanswered, `record_append_failed_after_input`, or the call cut off between the press and its
  record), the step holds its marks, and its click when the request declared one (with
  `--tap-rect` or `--swipe` and a request without a click, the click is declared only when the
  press is recorded, so the step holds its marks and no click), and **the recording holds no
  outcome of a press** for it. That can mean the press was not performed, or that it was performed
  and could not be recorded: the earlier call's answer tells (`record_append_failed_after_input`
  means it was performed; a call cut off leaves no answer). The same call sent again is refused by
  `record mark`'s own check, `record_click_exists` for the step's click or
  `record_mark_id_conflict` for its marks, and nothing is pressed a second time. When the request
  is for the open step and every mark it adds is on the step already with the same members
  (`record_mark_id_conflict`), or the step's click exists (`record_click_exists`), the refusal
  keeps its code and its message says all of this: look at the screen first, with `observe
  --capture` without `--record` (MCP: `ac_lab_observe` with `capture` and without `record`, on
  today's server and the Lab's alike), or read `record status` (MCP: `ac_record_status`, on both
  servers); a frame with `--record` on that step is refused (`record_step_click_not_executed` for a
  declared click, `record_step_click_missing` without one). Only while the screen still shows the
  step's own page, send the same call without
  the mark request: the same command line without `--mark-request` and `--mark-request-json`,
  `--next-frame-after-ms` kept and, for a step without a click, its `--tap-rect` or `--swipe` kept
  (MCP: `ac_lab_do` with the same arguments without `mark`; the Lab's MCP server's `ac_do` with the
  same arguments without `mark_request_json` and `mark_request`; `tap_rect` or `swipe` kept for a
  step without a click). If the screen shows the page the press leads to, the press was performed:
  do not send it again. To change the marks or the click first, send a `record mark` request
  (`replace_click` replaces the click), then the call without the request.
- **The next frame.** `--next-frame-after-ms <ms>`, 0 to 60000 as a settle is (`validation_failed`
  otherwise; `--next-frame-out` needs it): after a press performed without a failure the command
  waits that long, takes one frame through the Runtime as `capture --record` takes it (its
  `CaptureSequence`; not the operation's after frame, which is taken as soon as the input is
  acknowledged and stays informational), writes its PNG to `--next-frame-out` when that is given,
  and lets it arrive as `capture --record`'s frame arrives ("Steps, frames and remediation": the
  step just closed, so it opens the next step). A press not performed, performed with a failure or
  indeterminate takes no next frame; without the option nothing is captured after the press. A next
  frame that cannot be taken (its capture, its file or its arrival) answers that error, its own code
  and class: the press and its closed step stand and nothing is rolled back; its details begin with
  `record` (the click as recorded) and `next_frame: {after_ms, taken: false}`, then the capture's
  own members and `mark`. Take the frame with `capture --record` or `observe --capture --record`;
  under an MCP server, with the tool that serves `observe --capture --record`, `ac_lab_observe`
  with `capture` and `record` (today's server and the Lab's alike), not with a command line; do not
  press again. The device configuration that capture uses (the user configuration's entry of the
  instance the recording lock names, with the global backend options) is resolved before the press
  (fix round 1): an error in it ends the call before anything is written or pressed.
- **Telling the two device errors apart.** A press that was not performed and a next frame that was
  not taken can both be `device_error`: the second's details have `record` (its `step_closed`
  true) and `next_frame.taken` false, the first's have neither. actinglab's own line keeps the
  details whole whatever `ACTINGCOMMAND_DETAIL` says, and so does the Lab's MCP server's refusal
  (`{outcome, message?, details?}`). Today's MCP server (`actingctl mcp-serve`) carries them as the
  `raw_text` of `lab_tool_failed`'s `foreign_other_error` link: with `--detail full` (the default)
  its first 4096 bytes, where `record` and `next_frame` lead; `--detail codes` withholds every
  foreign text, so there the two read alike (`lab_tool_failed` caused by `device_error`), and the
  step's state is read with `ac_record_status` (the open step, its click's execution) before
  anything is pressed again.
- **Answer.** `do --capture --record`'s answer, whose `record` gains `mark` (the `record mark`
  output) with a mark request and which, with a next frame, gains `next_frame`: `capture
  --record`'s members (`width`, `height`, `capture_backend_used`, `adb_source`, `adb_warning`,
  `capture_backend_attempts`, `freshness`) with `after_ms`, `out` and its `record{status:
  "frame_recorded", record_id, record_started_at_unix_ms, step, step_opened, frame, closed_step}`.
  `arrival` stays the last member.
- **Lock.** The command reads its flags first, the mark request among them (its file read, or its
  JSON text, and parsed; a request that does not parse is refused before any lock), then takes the
  recording lock as `do --capture --record` takes it ("Recording lock") and holds it across the
  rest of the call: the next frame's device configuration, the request applied to the recording
  and self-tested, the press planned, the marks written, the press and the next frame.
- **The last screen.** A package ends on the screen its last press arrives at: a step of its own
  with its recognition marks, its `page` and no click. The last press's next frame (or a capture)
  opens it, and a `record mark` request with `page` and without a click marks it (the example's
  last `record mark`); its `page` names the package's terminal page, `step_<nn>_<page>` (the
  `terminal_pages` of its task, the page a binding or a following package names; only `step_<nn>`
  without it). `record stop` refuses a last step that has a click (`record_final_step_has_click`)
  and a step without a mark (`record_step_without_recognition`).
- **Dry run.** `--dry-run` stays refused for the device form (`record_flag_unsupported`); `record
  mark --dry-run` with the same request is its preview.
- **One rectangle, declared once.** `"click": {"from": "<a mark of the request>"}` copies that
  mark's region: for a template the crop, not its search area. So one rectangle given once is the
  template's crop and the click (the example below). Nothing was added for it.
- **The recording.** A recording made in this form is the `recording.json` the three calls write for
  the same steps: the same steps, frames, marks, crops, clicks and executions; only the times
  differ, and the request, correlation and action ids the Runtime gives each press. The package
  `record stop` generates from it is the same. Nothing changed in `recording.json`, the request's
  schema, the six mark families, the package generator or the kernel; a recording an older build
  wrote is read as before.
- With `--operation` the four flags are refused as `conflicting`, as every flag of another path of
  `do` is.

A whole recording in the fewest calls: `record start`, the first frame, then one call for each step
with a press (its next frame the next screen), the final screen's marks, `record stop --dry-run`
and `record stop`; for n steps with a press, n + 5 calls where three calls a step took 3n + 5.
`record start` holds the instance and `record stop` releases it (#463 S1-7, "A recording holds its
instance").

In PowerShell 7.3 or later (the shell of [INSTALL.md](../distribution/windows/INSTALL.md), where a
program receives a single-quoted argument's double quotes as typed), with the install root
`C:\ActingCommand`, every path quoted and a backtick continuing a line. The first step's request is
given as JSON text; the next ones as files the agent writes after reading each next frame, UTF-8
without a byte order mark: `step-2.json` the marks of the screen in `step-2.png` and its click,
`final.json` the final screen's marks and its `page`, no click.

```powershell
$lab = 'C:\ActingCommand\tools\actinglab.exe'
& $lab --json --instance emu-a record start --task-id daily_open
& $lab --json --instance emu-a capture --record
& $lab --json --instance emu-a do --capture --record --package 'C:\ActingCommand\carrier' `
  --mark-request-json '{"schema_version":"actingcommand.lab-record-mark.v1","page":"title","add":[{"id":"ui/start","family":"template","region":{"x":1180,"y":664,"width":73,"height":25}}],"click":{"from":"ui/start"}}' `
  --next-frame-after-ms 800 --next-frame-out 'C:\ActingCommand\frames\step-2.png'
& $lab --json --instance emu-a do --capture --record --package 'C:\ActingCommand\carrier' `
  --mark-request 'C:\ActingCommand\requests\step-2.json' `
  --next-frame-after-ms 800 --next-frame-out 'C:\ActingCommand\frames\step-3.png'
& $lab --json --instance emu-a record mark --request 'C:\ActingCommand\requests\final.json'
& $lab --json --instance emu-a record stop --dry-run
& $lab --json --instance emu-a record stop
```

Through MCP the same: `ac_record_start`, `ac_lab_observe` with `capture` and `record`, then for each
step

```json
{"instance": "emu-a", "capture": true, "record": true, "package": "C:\\ActingCommand\\carrier",
 "mark": {"schema_version": "actingcommand.lab-record-mark.v1", "page": "title",
          "add": [{"id": "ui/start", "family": "template",
                   "region": {"x": 1180, "y": 664, "width": 73, "height": 25}}],
          "click": {"from": "ui/start"}},
 "next_frame_after_ms": 800, "next_frame_out": "C:\\ActingCommand\\frames\\step-2.png"}
```

as the arguments of `ac_lab_do` (on the Lab's MCP server `ac_do`, with the request as
`mark_request_json`, its JSON text, or `mark_request`, a file, in place of `mark`), then
`ac_record_mark` of the final screen, `ac_record_stop` with `dry_run` and `ac_record_stop` (the same
names on both servers).

## Recording while playing

#463 S1-1 (0.12.64; the owner approved the three-stage blueprint of package making on board #463:
stage one starts recording, plays the business normally and ends recording, and asks nothing else
of the agent while it plays; the rules here are the coordinator's). A press is recorded by the
command that makes it: no rectangle, no carrier package, nothing marked. Each press is a step of
the instance's recording with the screen before it and the screen after it.

```
tap <x> <y> --record [<after>]
swipe <x1> <y1> <x2> <y2> <ms> --record [<after>]
long-tap <x> <y> <ms> --record [<after>]

<after>: [--after-ms <0..60000>] [--interval-ms <20..1000>] [--still-ms <0..5000>]
         [--still-max-ms <0..60000>]
```

All with the global `--json --instance <i>`. Through the Lab's MCP server (`actinglab-mcp`) they are
`ac_tap`, `ac_swipe` and `ac_long_tap` with `record`, `after_ms`, `interval_ms`, `still_ms` and
`still_max_ms` ([lab-operations.json](lab-operations.json), the rows `tap`, `swipe` and
`long-tap`); today's server (`actingctl mcp-serve`) has no such tool. A whole business is `record
start`, its presses each with `--record`, and `record archive` ("Archiving"), which needs no
device: the last press's screen after it is the business's final screen. `record start` holds the
instance (its scheduling paused with the recording's own reason) and `record archive` releases it;
each press says in `record.warnings` when the instance is not held (#463 S1-7, "A recording holds
its instance").

- **The screen before.** Every recorded press is sent on a frame captured immediately before it.
  The recorded point is a pixel of that frame (fix round 2 of #463 S1-1). The command takes one
  frame through the Runtime as `capture --record` takes it (the `ObserveReadonly` read) right before
  every press, a step open or not, and sends the input with the Runtime's reference of that frame,
  built as the online scene of `do --capture` builds the reference of the frame it plans on
  (`do --capture --record` itself presses inside the Runtime's own operation): the Runtime
  resolves the frame before it presses (`input_frame_unavailable` when it cannot), and an instance
  whose input is the emulator's own presses only on such a frame. The command reaches the device
  only through the Runtime: unlike `capture --record` it reads no local device configuration (no
  ADB path), as `tap` without `--record` reads none (fix round 3). What is kept of the capture:
  when no step is open (a new recording, or the screen after the previous press could not be
  taken), it opens the step the press lands on; on an open step without marks it arrives as
  `capture --record`'s does on that step, its live primary frame, the older frame staying in the
  step, superseded; on an open step with marks (made on its stored frame) it is not stored and the
  stored frame stands, and it must be of the recording's size (`record_frame_size_mismatch`). The
  step's live primary frame is the screen the press is committed on.
- **When the frame is gone before the press** (fix round 3). Between the command's capture and
  its input the Runtime may drop the frame the input names (another keyless capture such as a
  monitor probe, a capture by a lease holder, the capture backend reopening). It then denies the
  input `input_frame_unavailable` and nothing is sent. On exactly that refusal (the denied receipt
  whose outcome's code is the frame's being unavailable) the command captures once more, as the
  first capture was taken, and sends the press once more on the new capture, once: a second
  refusal of any kind is answered as "The outcome" says. What is kept: the first capture, if it was
  stored, stays in the step, superseded by the second, which the press is committed on (its
  `before_frame`); on an open step with marks neither is stored. The answer's
  `record.before.captured_twice` is `true`. No other refusal is sent again.
- **The press.** The open step must declare no effect. A tap or a long press declares a box of one
  pixel at its point, `rect {x, y, 1, 1}`, `source: "point"`, rule `explicit` (a long press with
  `gesture{kind: "long_press", duration_ms}`, ending where it began); a swipe is the recorded swipe
  of "`--record` on device commands" (a drag between boxes of one pixel, `source: "swipe"`). The
  input is sent once through the Runtime's input proxy, with its own lease as `tap` takes it and the
  frame reference of the capture before it: a tap, one long tap or one swipe, as the kernel presses
  the click the recording generates for it.
- **Refused before anything is pressed** (exit 2 or 3): what the open step and the point refuse
  comes before anything is captured; on a new recording the frame the command took first stays.
  `validation_failed` with `details{argument, reason}`: a point outside the frame (`point`,
  `outside_frame`; a swipe's either point too), a long press or a swipe outside 1..=60000 ms
  (`duration_ms`, `duration_range`), a wait outside 0..=60000 ms (`after_ms`, `range`; a value that
  is no whole number too), `--after-ms` without `--record` (`after_ms`, `record_only`). #463 S1-2:
  `--interval-ms` outside 20..=1000, `--still-ms` outside 0..=5000 and `--still-max-ms` outside
  0..=60000 (`interval_ms`, `still_ms`, `still_max_ms`, `range`; a value that is no whole number
  too), each without `--record` (`record_only`), and a bound of more than 120 intervals,
  `still_max_ms` above 120 times `interval_ms`, a value out of the range the interval allows
  (`still_max_ms`, `range`, with `max` 120 × `interval_ms` and `interval_ms` in the details), so
  that a press never takes more than 121 captures after it. An open step
  that declares a click without an outcome is `record_click_exists`, one whose click has an outcome
  on record `record_click_executed`, one that declares an application operation
  `record_step_effect_exists`: such a step is pressed with `do --capture --record`. A frame of
  another size than the recording's is `record_frame_size_mismatch`. A capture that fails answers
  its own code, and one whose observation carries no frame id (the client's receipt check already
  refuses such an observation) is `device_error`: nothing is pressed and nothing of it is stored.
  `--dry-run` is `record_flag_unsupported`.
- **The outcome.** A receipt is a performed press: recorded, its step closed. An error after which
  the Runtime's own record of the input may say it is indeterminate is an indeterminate press, as
  `do --capture --record` records every press the Runtime did not record as not performed: every
  error but a refusal the Runtime denied with a usage or safety result class. That covers a device
  failure of the input backend, a Runtime failure, an input its operation check stopped, and an
  answer the client never read (uncertain). Such a press is recorded with the request and
  correlation ids the error carries, its step open, exit 4 `record_click_indeterminate` with
  `details.record` (status `press_indeterminate`) and `details.input_outcome`, and no screen after
  it is taken; check the screen, then `record mark --reopen-step k` or `--close-step`. A denied
  usage or safety refusal (the request, the frame or the foreground refused, a lease, queue or
  preemption rule) proves nothing was sent: no press is recorded, the capture before it stays in
  the step as a `capture --record` frame would (on an open step with marks it was not stored),
  and the command answers as it does without `--record` (`device_error` with `input_outcome`); so
  does a lease or connection that fails before the input. A press that was sent and could not be
  recorded is `record_append_failed_after_input`.
- **The screen after** (#463 S1-2; the owner on board #463: what stage one hands over is, for each
  press, the frame it happened on and the last result frame, and stage two must be able to give an
  agent that asks the several or several dozen frames before and after a press; the screen may
  change quickly after a press, so the captures after it are closer together; the rules here are
  the coordinator's). After a performed press the command captures repeatedly until the screen is
  still, and keeps every capture. Each capture is one frame as `capture --record` takes it.
  - *When.* The first capture starts `--after-ms` after the press (100 without it); then one
    capture every `--interval-ms` (100 without it), counted from the start of one capture to the
    start of the next; when a capture took longer than the interval the next starts at once. No
    wait is shorter than it is asked to be.
  - *Still* means the screen has not changed for at least `--still-ms` (300 without it), however
    long a capture takes (fix round 3, the coordinator's ruling): take the newest capture and the
    unbroken run of captures before it that each differ from the newest by at most the Runtime's
    own frame-change bound (`frame_change_milli` of the whole frame at most
    `FRAME_CHANGE_MAX_MILLI`, the measure the kernel's still waits use, `contracts/color-digest.md`);
    the screen is still when that run holds at least one earlier capture and the newest started at
    least `--still-ms` after the first capture of the run. A capture outside the bound ends the run,
    and a screen that comes back after it starts a new one. So with captures every 100 ms and
    `--still-ms 300` four unchanged captures are needed; with a capture cycle of 400 ms two suffice.
  - *The end.* The loop ends at the first still capture (`still: true`), or when the next capture
    would start more than `--still-max-ms` (5000 without it) after the first, or once it has taken
    `1 + still_max_ms / interval_ms` captures, whatever the caller's clock says: then the last
    capture is the screen after, `still: false`, and the answer's `record.warnings` holds
    `after_not_still` (the press is recorded, exit 0). So a press takes at most that many captures
    after it, 121 at most. With `--still-ms 0` the first capture is the screen after, without a
    stillness test (`still: null`).
  - *What is kept.* Every capture of the loop is a frame of the step the first of them opens, in
    the order taken; all but the last are superseded; the last is that step's live primary frame
    and the press's `after_frame`. A capture whose bytes equal the previous one's is the same file
    (frames are stored by their digest) but still an entry with its own time, so that a reader sees
    how long the screen stood. The frames are written once, when the loop ends.
  - *The next press.* On a step without marks its capture before it (see "The screen before")
    arrives on that step and supersedes the loop's last frame, which stays in the step. So a step's
    frames, in time order, are the frames of the previous press's after loop, then the frame its own
    press happened on: what stage two reads when an agent asks for the frames before and after a
    press. On a step marked before the next press that capture is not stored, and the step's frames
    end with the loop's last.
  - *A first capture that cannot be taken* answers its own code and class, nothing of the loop is
    stored, and the press and its closed step stand: its details begin with `record` (the press as
    recorded) and `after: {after_ms, taken: false}`, then the capture's own members; take the
    screen with `capture --record` (MCP: `ac_lab_observe` with `capture` and `record`), do not press
    again.
  - *A later capture that cannot be taken* (fix round 3; the owner asked that every capture be
    kept) ends the loop there, and what it took is kept: the captures taken are the step's frames
    as above, the last of them the press's `after_frame`, and `after_capture` says how many were
    taken (`captures`) and that the loop did not finish (`unfinished: true`, `still: null`). The
    command answers the capture's own code and class, loud: its details begin with `record` (the
    press as recorded, with that frame after) and `after: {after_ms, taken: true, captures}`, then
    the capture's own members; the press and its closed step stand, do not press again.
  - A failure after a performed and recorded press that does not undo it (the input lease's
    release, the receipt's encoding) is answered after the screen after it, `device_error` with
    `record` and `input_outcome` in its details.
- **In `recording.json`** (every member absent on every other press, so that a recording without
  such a press, one of `do --capture --record` included, is byte-identical to one an earlier build
  wrote): the click's `source` `point`; its execution's `before_frame`, the id of the step's live
  primary frame when the press was committed (what was on the screen); its `after_frame`, written
  once: the last capture of its own after loop, when the loop ends (a later capture that failed
  included), or, when the loop stored nothing (its first capture could not be taken), the first
  frame that opens a step after it (a `capture --record`, a `record mark --frame`, or the capture
  before the next press); a later frame never rewrites it; its `after_capture: {after_ms,
  interval_ms, still_ms, still_max_ms, still, waited_ms, captures, kept}`, how the screen after was
  taken: the four parameters as the press took them, `still` `true`, `false` or `null` (no
  stillness test, or a loop that did not finish), `waited_ms` from the press to the start of the
  last capture, `captures` the captures after the press and `kept` those kept beside the last
  (`captures - 1`), and `unfinished: true` only when a later capture could not be taken (absent
  otherwise); `after_capture` is absent when the loop stored nothing. For example, an animation
  that settles:
  `{"after_ms": 100, "interval_ms": 100, "still_ms": 300, "still_max_ms": 5000, "still": true,
  "waited_ms": 600, "captures": 6, "kept": 5}`. Its `sent_on_unstored_capture: true` when the press
  was sent on a frame the command captured and did not store (an open step with marks), so that
  `before_frame` names the stored frame the marks were made on and not the frame the click
  happened on (fix round 3 of #463 S1-1; absent on every other press). Its `carrier_package`,
  `before` and `after` are `null`: no Runtime Lab operation ran.
- **The answer.** The command's own answer (`status: "sent"`, `action`, `input_outcome`, …) with
  `record: {status: "press_recorded", record_id, record_started_at_unix_ms, step, step_closed, click,
  before: {frame_id, path, sha256, refreshed, sent_on_unstored_capture, captured_twice}, after:
  {step, frame_id, path, sha256, after_ms, interval_ms, still_ms, still_max_ms, still, waited_ms,
  captures, kept} | null, warnings: [] | ["after_not_still"]}`. `path` is the stored frame: the
  agent reads the next screen there. `refreshed` says the command captured the frame `frame_id`
  names; `sent_on_unstored_capture` says the press was sent on a frame the command captured and did
  not store (an open step with marks), so that `frame_id` names the stored frame the marks were
  made on; `captured_twice` says the frame was captured twice because the Runtime no longer held
  the first ("When the frame is gone before the press").
- **Coordinates.** The point is in the recording's coordinate space, the pixels of its frames, and
  the frame the press is sent on is of that size. The Runtime hands the input's coordinates to the
  instance's input backend as they are; an input backend that is the emulator's own reads them as a
  pixel of the frame the input names, every other one presses them as they are, the frame resolved
  but not read. Nothing converts between an input backend and a capture backend that see the
  display in different orientations; nothing is refused for it.
- **A recording made with `do --capture --record`** goes on with these commands: a press on its open
  step without a click declares the point click; a step that declares a click is refused
  `record_click_exists`. A stopped or archived recording takes no press (`record_session_not_active`).
- **`record stop`.** Once the steps are marked (`record mark --step k`), a point click is generated
  as the click of its box of one pixel, `{"kind": "rect", "x", "y", "width": 1, "height": 1}` (a
  long press `long_press` with that box), which the kernel admits and presses at its point.

In PowerShell 7.3 or later, with the install root `C:\ActingCommand`:

```powershell
$lab = 'C:\ActingCommand\tools\actinglab.exe'
& $lab --json --instance emu-a record start --task-id daily_open
& $lab --json --instance emu-a tap 962 658 --record
& $lab --json --instance emu-a swipe 900 400 300 400 300 --record --after-ms 1200
& $lab --json --instance emu-a long-tap 640 360 800 --record
& $lab --json --instance emu-a record archive
```

## A recording holds its instance

#463 S1-7 (0.12.64; the owner approved the three-stage blueprint of package making on board #463,
whose stage one asks nothing of the agent but to start recording, play and end recording; the rules
here are the coordinator's, fix round 2 included). An agent's presses are separate commands with
seconds or minutes of thinking between them, and each holds the instance's input lease for its
press alone. Before this
slice nothing kept the scheduler from dispatching a scheduled task to the instance between two
presses: the task's screen became a press's screen after it and opened the next step, and nothing
warned. A recording now holds its instance itself: nothing else is asked of the agent.

**Taking the hold (`record start`).** After the start's own refusals (`record_session_active`, the
options), `record start` connects to the Runtime the `--record` commands reach
(`ACTINGCOMMAND_RUNTIME_STATE_ROOT`, else the default state root; the connection's handshake is
`Health`, then `BindChannel`) and reads its status once. Nothing is written so far, so nothing
needs undoing when this fails. Then (fix round 2: the session header knows the hold before the
Runtime is asked to pause):

- **The instance's scheduling is not paused:** the session header is written first, naming the
  recording's own hold `{reason, own: true}`, and only then is the instance paused as the Lab's
  `pause` row pauses it (`PauseScheduling` of the instance, `lab_ops::scheduling`), with that row's
  drain (300000 ms) and the recording's own reason `lab-record-<ms>`, `<ms>` the recording's start
  (its session header's `started_at_unix_ms`, which every `--record` answer names as
  `record_started_at_unix_ms`), as the Lab's rescue module names its holds `lab-<word>-<ms>`. The
  pause waits by its own rules ([scheduling-pause.md](scheduling-pause.md), "Instance pause: three
  stages"): a scheduled run in flight is drained, or stopped at its next checkpoint once the drain
  elapsed (`contained_task_paused`, its task evaluated again), and an active lease is waited for.
  **So `record start` waits until the instance's running task has ended:** the client waits for the
  pause's receipt up to the drain (300000 ms), the pause's checkpoint grace
  (`SCHEDULING_PAUSE_CHECKPOINT_GRACE_MS`, 30000 ms) and its IO timeout (5000 ms), 335 s in all. A
  caller with a shorter time limit (a tool call, a shell's command timeout) must give it that time.
- **The instance is already paused** (another party's pause, which a pause would meet as
  `scheduling_already_paused`): nothing is sent. The recording runs under that pause, holds nothing
  of its own, remembers its reason as another party's, and never resumes it.
- **The pause of the recording it replaces** (`record start --force` over an active recording, or
  a start after an end that kept its hold, below): when the status shows exactly the replaced
  session header's own reason, that pause is taken over, the new header's own (`taken_over`):
  never paused twice, never lost.
- **Nothing listens** (no state root located, no runtime-info that names an address, nothing
  accepting a connection at that address: no Runtime is running): nothing can dispatch. The
  recording starts holding nothing (an own pause of the replaced header is carried over: the
  Runtime restores it at its start), exit 0, and the answer says so: `hold.warnings`
  `["no_runtime_listening"]` with `hold.runtime_error`, the connection's failure. A press with
  `--record` needs the Runtime anyway.
- **Something listens, but the handshake does not complete** (`Health` or `BindChannel` fails, is
  refused or gets no answer): a Runtime is there and may dispatch, so whether the instance would be
  held cannot be known. `record start` fails and writes nothing, with the failure as the Lab's
  commands that need the Runtime answer it (`runtime_slice_cli::map_runtime_error`:
  `runtime_not_running` when no receipt said otherwise, `device_error` when the Runtime refused
  with a code that is not one of unavailability). The client's connect reads the runtime-info,
  opens the connection and shakes hands in one call and does not say which of the two failed; so
  after a failed connect `record start` opens one plain connection to the address the
  runtime-info names, within the client's IO timeout, and closes it at once without sending
  anything: accepted, something listens; refused, nothing does. The Runtime takes a connection
  closed before its first frame as an ordinary close.
- **A status the Runtime refuses**, or leaves unanswered once the connection is made: `record
  start` fails with it as above, its message saying it paused nothing and started nothing, and
  writes nothing.
- **A pause the Runtime refuses** (its receipt says no pause was made: denied, or failed as a
  whole, which lifts the gate it closed): the session header is put back as it was before (when
  there was none, the new one does not stay), and `record start` fails with the refusal, its
  message saying it paused nothing and started nothing: no recording.
- **Any other end of the pause** (the connection lost, the wait for the receipt ended, the process
  killed or timed out by its caller): the session header stays as written, because a pause with
  that reason may exist. The failure (`runtime_not_running` when the connection was lost) carries
  `hold: {reason, own: true}` in its details, and so does every failure after the header names the
  hold (the Lab recording's start, the state directory's reach). `record stop` (or `record start
  --force`) then finds the header and releases by its reason; a release that finds no such pause
  is `already_gone`. (`record archive` of such a header, whose Lab recording never started, is
  refused as `lab_recording_missing`.)
- **`--force` over a session header that cannot be read** (fix round 2): the start goes on, which
  is what `--force` is for, but the hold that header named is not known: a pause the earlier
  recording took, if any, shows as another party's to this recording and is not released by it.
  The answer says so: `hold.warnings` gains `replaced_header_unread`, and `hold.replaced_header:
  {path, error}` names the header's path (as the answer's `path` names it) and why it could not be
  read.

The session header keeps what the recording holds: `hold: {reason, own}`, `own` `true` for its own
pause (set or taken over), `false` for another party's. A header that holds nothing has no `hold`
member, byte for byte as before: every header written before 0.12.64, and one started while
nothing listened. The recording library keeps these facts and says what they mean; the `record`
command asks the Runtime and hands them in. The answer adds `hold: {state, reason?, taken_over?,
paused?, warnings?, runtime_error?, replaced_header?}`: `state` as `record status` shows it,
`paused` the Runtime's `SchedulingPaused` result. Both warning words are warnings: the start goes
on and exits 0.

| `hold.warnings` word | Its one cause | Refuses or warns | Exit |
|---|---|---|---|
| `no_runtime_listening` | no connection to the installation's Runtime could be opened at all, so the recording holds nothing of its own | warns | 0 |
| `replaced_header_unread` | `--force` replaced a session header that could not be read, so a pause the earlier recording took, if any, is not released by this one | warns | 0 |

**While it plays.** Each `tap`, `swipe` and `long-tap --record`, each `do --capture --record` and
each `session app … --record` reads the status once before it, on a connection of its own (fix
round 2: a press's read no longer shares the press's connection, so a read that fails never leaves
that connection dead and the press `not_submitted`; `do --capture --record`, the one form today's
MCP server records a press with, reads it too, by the same function). When the instance's
scheduling shows not paused (neither its own pause nor the global pause), the press or the
operation is still made and recorded, and
`record.warnings` holds `instance_not_held`: the material may be mixed with a scheduled task's
screens. When the read gets no answer, `instance_hold_unread`: whether it is held is not known. A
pause of anyone counts as held. A press under the recording's own pause is an operator request, which
no pause gates ([scheduling-pause.md](scheduling-pause.md), "The dispatch gate": leases, observations
and Lab operations); an instance outside the multi-Nemu gate, whose device session the pause closed
(scheduling-pause.md, stage (c)), opens a new one at its next press, as it does after every lease end
(`keeps_device_session`, `crates/runtime-host/src/host/resource_close.rs`; a closed session is
started again, `crates/execution-kernel/src/kernel.rs`).

**Ending the recording releases its own hold, and only its own.** `record archive`, once its
manifest, the session header (`archived`) and the copy (`--copy-to`) are written, and `record stop`,
once a stop that is no dry run has ended the recording (a refused stop changes nothing: the
recording stays active and keeps the hold):

- The session header is first written with the hold `kept: true` (another party's pause: none).
- Another party's pause: nothing is sent; `hold: {reason, own: false, release: "not_own"}`.
- The own pause: one status read; only the instance's pause that shows exactly the reason is
  resumed, by the pause the status shows now (`ResumeScheduling` expecting the status's owner epoch
  and that pause's revision), as the rescue module's `release_own` finds its own: a Runtime restart
  restores a pause in a new owner epoch at revision 1, so a remembered revision is not enough. The
  header is then written without the hold; `hold: {reason, own: true, release: "resumed", resumed}`.
  A pause with another reason, or none, is left alone: `release: "already_gone"`, `shown` the
  reason the instance showed, or `null`.
- What the release costs: an instance resume reconnects the device and runs its self-check
  (scheduling-pause.md, "Resume"), up to 33 s plus the IO timeout. The archive's or the stop's own
  work is done before it.
- A release that fails, or a Runtime that does not answer, neither undoes nor fails the end's
  work: the manifest or the package, the header and the copy stand, and the header keeps `hold:
  {reason, own: true, kept: true}`. The answer is the one the Lab's rescue module gives for a hold
  it keeps: the release's failure (`runtime_not_running` when no Runtime answered, else the
  Runtime's refusal as the Lab maps it, `device_error`), exit 1 on the command line and the class's
  exit under `--via`; its message names the hold and says the end is done; its details are the
  end's answer (`record`, `path`, `lab`) with `hold: {reason, own: true, kept: true}` and (fix
  round 2) `next.release`, the command that tries the release again and can release only the
  recording's own pause, found by its reason: `["record", "stop", "--instance", "<instance>",
  "--state-dir", "<dir>"]` for a stopped recording, `["record", "archive", "--record-id", "<id>",
  "--instance", "<instance>", "--state-dir", "<dir>"]` for an archived one (an archive without
  `--record-id` finds no active recording and is refused, `record_session_not_active`, retrying
  nothing). `next` never offers an unconditional `resume` of the instance, which could lift a
  pause that is not the recording's (a person's). `record status` shows `kept`, and the next
  `record start` of the instance takes the pause over while the status still shows it.
- A copy that fails answers its failure as before, with `hold: {reason, own: true, kept: true}` in
  its details: the release was not tried; `record archive --record-id <id> --copy-to <dir>` tries
  both again.

`record reopen` (the offline work of stage two) takes no hold; an own pause the session header kept
stays named in the header it writes, so that the reopened recording's end releases it.

**`record status`** adds `hold: {state, reason?}`, read from the session header alone: `own` (an
active recording under its own pause), `foreign` with the other party's reason, `none` (no hold,
or no session header), `kept` (an own pause the recording's end could not release).

**A forgotten recording** keeps its instance's scheduling paused until it ends or a person resumes
it: a pause has no expiry and comes back after a restart; `status` and the overview show it with
its reason `lab-record-<ms>`.

**At a deploy.** The deploy's guard `foreign_pauses` (`guards --for stop`,
`crates/lab-rescue/src/install/guards.rs`) reports a recording's pause as another party's (the
guard fails on it); the rescue module records that report and goes on without touching the pause
(its hold takes only the instances not paused, so it neither pauses that instance again nor resumes
it); and the restarted Runtime restores the pause at its start (in its new owner epoch, at revision
1, stage `released`). The recording's end then releases it by its reason in the new owner epoch.

**Today's MCP server** (`actingctl mcp-serve`) runs `record start`, `record stop` and `record
archive` as actinglab children (`apps/actingctl/src/mcp/lab.rs`, `Program::call`): its
`ac_record_start`, `ac_record_stop` and `ac_record_archive` hold and release as the command line
does, and (fix round 2, texts only) their descriptions say so: the installed Lab they run holds
the instance at the start and releases it at the stop or the archive. An author-tier session
holding and releasing its own recording's instance is intended (the coordinator's ruling); no
behaviour of that server changes.

## Application steps

A step's effect may be an application operation instead of a click (R24): `launch`,
`restart` or `stop` of the application assigned to the instance
(`application-lifecycle.md`); the package never names the application. One step has one
effect.

```
actinglab --json --instance <i> session app <launch|restart|stop|force-stop> --record
actinglab --json --instance <i> session instance app <launch|restart|stop|force-stop> --record
actinglab --json [--instance <i>] record mark [--step <k>] [--frame <png>] [marks…]
          --application <launch|restart|stop|force-stop> [--replace-click]
```

`force-stop` is recorded as `stop`; the verb used stays in `cli_verb`. In the request form
the field is `"application": null | {"action": "restart"}`.

```json
{"index":1,"page":null,"frames":[],"marks":[],"click":null,
 "application":{"action":"restart","cli_verb":"restart","source":"executed",
   "executed":{"request_id":"…","correlation_id":"…","action_id":"…","receipt_state":"completed",
               "application_event_ids":["…","…"]},
   "attempts":[],"needs_review":false},
 "transition":null,"closed":true,"closed_by":"application"}
```

- `source` is `executed` (a completed receipt is on record) or `declared`. `attempts[]`
  holds `{receipt_state, runtime_code, request_id}` of operations without a completed
  receipt (`receipt_state` is the receipt state, or `none` when no receipt arrived) and of
  executions moved aside by `--reopen-step`. `needs_review` stays false: an application
  operation has no "performed with a failure" outcome.
- The application entry step is a step with an application operation and no frame (not a
  separate field; `record status` shows `entry:"any"`). It can only be the first effective
  step; its package operation starts `from:"any"`. `coordinate_space` is set by the first
  frame after it.
- #463 S1-3 (since 0.12.64): on a step with a frame the operation is recorded with the screen
  before it and the screen after it, as a press is ("In the middle of a business" below). The
  entry step has no screen before it and takes none after it: its arrival screen comes from the
  next press's screen before or the next `capture --record`.

### Where the operation lands

| recording | `session app … --record` (device) | `record mark --application` (offline, without `--step` and `--frame`) |
|---|---|---|
| no effective step | Performed: a new application entry step, executed and closed (`closed_by:"application"`). Indeterminate: a new entry step, declared with the attempt, open | a new entry step, declared, open |
| the last effective step is closed and no arrival screen came yet | #463 S1-3: one capture first, as a press takes it, which opens the step the operation lands on (the next row) | `record_application_entry_invalid` (3) |
| open step, no effect (#463 S1-3: with marks or without them) | Performed: the step's effect, executed, the step closed, then the screen after it. Indeterminate: declared with the attempt, open | the step's effect, declared, open |
| open step with a click | `record_step_effect_exists` (3), nothing sent | replaced with `--replace-click`, otherwise `record_step_effect_exists`; a click with an outcome is `record_click_executed` |
| open step with a declared application operation (also one left by an indeterminate result) | the same action is sent (#463 S1-3: no screen is taken before it; the step's frame stands): Performed records the execution and closes the step, Indeterminate adds an attempt. Another action: `record_step_effect_exists` (3), nothing sent | replaced with `--replace-click` (its attempts are kept), otherwise `record_step_effect_exists` |

- With `--frame` (a new offline frame) or `--step k` the operation becomes the effect of
  that step, from its page. A declared effect is replaced only with
  `--replace-click`, which now means "replace the step's effect" (`record_step_effect_exists`);
  an effect with an outcome on record is not replaced (`record_click_executed`).
- `--application` with `--click`, `--click-from`, `--click-guard` or `--click-retry`, and a
  click guard or retry on an application step, are `record_application_with_click` (2); an
  action other than the four is `record_application_action_invalid` (2).
- The entry step has no frame: `--page`, marks, samples and clicks aimed at it are
  `record_step_frame_missing` (3).
- The step rules above read "effect" where they say "click": a device frame on a step whose
  declared application operation has no completed receipt is
  `record_step_click_not_executed`, an offline frame closes such a step
  (`closed_by:"offline_frame"`), `--close-step` accepts it, `--reopen-step` moves the
  execution into `attempts[]`, and a transition (`--transition`, `--to-transition`) follows
  an application step as it follows a click, for example a splash screen captured after a
  restart. `do --capture --record` on a step with an application operation is
  `record_step_effect_exists`.
- Typical use (restart, then the main interface): `record start` (it holds the instance, #463
  S1-7); `session app restart --record` (the entry step, serial 1); `capture --record` of the title
  screen (serial 2), `record mark` of its marks and `do --capture --record` of its click;
  `capture --record` of the main interface (serial 3) and `record mark --page home` of its
  marks; `record stop --dry-run`, then `record stop`. The recorded package is
  `any → restart → title → home`.
- R25: a package with a `launch` or `restart` step must reach the main interface later, a
  step marked `--page home` (its page id is `step_<nn>_home`, which counts as the main
  interface). `record stop` ("Package generation") refuses a recording without it
  (`record_application_without_home`); `record mark` and `session app --record` do not check it. Screens a
  cold start shows only sometimes (a daily notice, an update prompt) are recorded as optional
  steps between the title and the main interface (`any → restart → title → [notice?] →
  home`, see "Optional steps"); the main interface step itself cannot be optional. A failure
  before the main interface is only run again.

### `session app … --record`

- Accepted only with one of the four actions and never with `--dry-run`, global or after
  the command (`record_flag_unsupported`, 2): `session app` does not read `--dry-run`, so the
  recording path offers no dry run. The other gate rules apply (`record_flag_takes_no_value`,
  `record_state_dir_unsupported`). Without `--record`, `session app` is unchanged.
- The record instance and the command instance must agree (`record_instance_mismatch`). The
  recording lock is taken before any Runtime request and held until the result is recorded;
  a busy lock is `record_busy` and nothing is sent.
- Order: #463 S1-3: on an open step the step is planned first (the refusals above capture and
  send nothing), then the screen before, captured immediately before the operation (but on the
  entry step and on a step that already declares the operation); without an open step after a
  closed one the screen before opens the step, which is then planned; the request is the
  `session app` request (`RuntimeClient::control_application`); then the Runtime result
  decides:
  - a completed receipt: recorded, the step closed, then (when it names its `before_frame`: not
    on the entry step) the screen after it; the output is the `session app` output plus `record{status:"application_recorded",
    record_id, step, step_opened, step_closed, application, after}` ("In the middle of a
    business");
  - a denied receipt (for example `fixture_execution_scope_forbidden` on a fixture instance,
    or a busy lease): the operation did not run; nothing is recorded and the error is
    returned as `session app` returns it;
  - anything else (a failed receipt such as `application_backend_operation_failed`, or no
    receipt at all): the operation may have run. The attempt is recorded on the planned
    step, which stays open, and the command fails with `record_application_indeterminate`
    (exit 4, `details{runtime_code, receipt_state, request_id, record, runtime_error}`).
    Check the instance, then run the same command again (launch, restart and stop can be
    repeated) or accept the step with `record mark --close-step`;
  - recording fails after the operation was sent: `record_append_failed_after_input` (3)
    with the plan, the effect and the cause.
- **Error codes, not exit codes.** Only `record_application_indeterminate` and
  `record_append_failed_after_input` mean the application operation may have run or did
  run; every other error means it did not run and the recording is unchanged. #463 S1-3: but
  for two things: the screen before, when the command took it, stays recorded; and an error
  whose details carry `after: {after_ms, taken: false}` is a screen after that was not taken
  (the train of stage one: `taken: true` with `captures`, a screen after whose loop a later
  capture stopped, what it took kept), the operation having run and been recorded. Several of
  these errors exit with 4 (also the Runtime refusals mapped to `device_error`): tell them
  apart by the error code.
- The v0.9.0 ActingLab ignores `--record` on `session app` and runs the operation without
  recording it: do not use it during a recording. A build without application steps refuses
  the flag with `record_flag_unsupported` before any request.

### In the middle of a business

#463 S1-3 (0.12.64; the owner approved the three-stage blueprint of package making on board #463,
whose stage one plays the business normally, and a business sometimes launches, restarts or stops
the application in its middle; the rules here are the coordinator's). An application operation in
the middle of a business is the open step's effect, as a press is ("Recording while playing"): the
step needs no mark, and the operation is recorded with the screen before it and the screen after
it.

```
session app <launch|restart|stop|force-stop> --record [--after-ms <0..60000>] [--interval-ms <20..1000>]
                                             [--still-ms <0..5000>] [--still-max-ms <0..60000>]
```

All with the global `--json --instance <i>`; `session instance app` the same. Through the Lab's MCP
server (`actinglab-mcp`) they are the tools of the rows `app launch`, `app restart` and `app stop`
with `record`, `after_ms`, `interval_ms`, `still_ms` and `still_max_ms`
([lab-operations.json](lab-operations.json)): these core rows take `record`, the slice's ruling
replacing the coordinator's ruling of 2026-11-11 (L5C-7) that kept it off the core rows, so no flag
of `session app` is an exception to its rows. The Lab's MCP server writes the actinglab command line
from the rows' parameters itself; `lab_ops::apps::App::recorded` (which puts `--record`,
`--after-ms` and `--still-max-ms` on its actinglab child's line) has no caller in the product.
Today's server (`actingctl mcp-serve`) is unchanged. The launch of a game is the application launch
only: nothing here presses a launcher icon or reads the home screen but for the frame.

- **The screen before** (round 2 of S1-3, after fix round 2 of S1-1). When the recording has an
  effective step, the screen before is a press's ("Recording while playing"): one frame captured
  as `capture --record` takes it, immediately before the operation, and kept as a press keeps it.
  When no step is open it opens the step the operation lands on. On an open step without marks it
  is the step's live primary frame, the older frame staying in the step, superseded. On an open
  step with marks it is not stored and the stored frame stands (it must be of the recording's
  size, `record_frame_size_mismatch`). The Runtime's reference of the frame, which a press is sent
  with, is not used: an application operation names no frame; a capture whose observation carries
  no frame id is `device_error` all the same, and nothing is sent. On an open step the step is
  planned first, and its refusals ("Where the operation lands") capture and send nothing. A step
  that already declares the operation (an earlier attempt without a completed receipt, or `record
  mark --application`) takes no frame before it: such a step refuses a device frame
  (`record_step_click_not_executed`), and the frame it was declared on stands.
- **The entry step is as before.** On a recording without an effective step the operation opens
  the application entry step, which has no frame: nothing is taken before it or after it, and no
  device is asked for. Its package operation starts `from: "any"`. The next press's screen before
  (or the next `capture --record`) is the business's first screen: it opens the step after the
  entry step and is no screen after it.
- **The screen after.** After a completed receipt that names its `before_frame` (not on the entry
  step, the entry launch run again after an indeterminate one included) the command captures the
  screen until it is still and keeps every capture, as after a press ("Recording while playing",
  "The screen after", the train of stage one): the first capture `--after-ms` after the operation
  (2000 without it), then one every `--interval-ms` (1000 without it), still when the screen has
  not changed for `--still-ms` (2000 without it; 0: the first capture, untested), at most
  `--still-max-ms` after the first (30000 without it), so at most 31 captures by default. Every
  capture is a frame of the step the first opens, the last its live primary frame and the
  operation's `after_frame`; at the bound `still` is `false` and the answer's `record.warnings`
  holds `after_not_still` (the operation recorded, exit 0). A receipt without completion is
  recorded as before (`record_application_indeterminate`, exit 4) and no screen after it is taken;
  a denied one records nothing. A first capture after it that cannot be taken answers its own code
  and class, and the operation and its closed step stand: its details begin with `record` (the
  operation as recorded), `after: {after_ms, taken: false}` and `operation` (the `session app`
  output), then the capture's own members; take the screen with `capture --record`, do not run
  the operation again. A later capture that cannot be taken keeps what the loop took
  (`after_capture.unfinished`), and `after` in that place is the loop's `{after_ms, taken: true,
  captures}`.
- **Refused before anything is sent** (exit 2): `validation_failed` with `details{argument,
  reason}`, the same refusals as a press's (the train of stage one: the flags are read and checked
  as a press's are): `--after-ms` outside 0..=60000 or no whole number (`after_ms`, `range`, the
  value as `after_ms`), `--interval-ms` outside 20..=1000, `--still-ms` outside 0..=5000 and
  `--still-max-ms` outside 0..=60000 (`interval_ms`, `still_ms`, `still_max_ms`, `range`; a value
  that is no whole number too), and a bound of more than 120 intervals, `still_max_ms` above 120
  times `interval_ms` (`still_max_ms`, `range`, with `max` and `interval_ms` in the details), all
  before the recording lock is taken; each of the four without `--record` (`record_only`, by the
  gate of "`--record` on device commands", whose text names `session app`, and by `session app`
  itself). `--dry-run` stays `record_flag_unsupported`.
- **In `recording.json`** (every member absent on the entry step and on every operation an earlier
  build recorded, so that a recording without such an operation is byte-identical to one an earlier
  build wrote): the completed receipt `steps[].application.executed` names `before_frame`, the id of
  the step's live primary frame when it was recorded (what was on the screen); `after_frame`, the
  last capture of its own after loop (when that loop stored nothing, the first frame that opens a
  step after it); and `after_capture: {after_ms, interval_ms, still_ms, still_max_ms, still,
  waited_ms, captures, kept}` (and `unfinished` as a press's), how the screen after was taken, as a
  press's.
- **The answer.** The `session app` output with `record: {status: "application_recorded",
  record_id, record_started_at_unix_ms, step, step_opened, step_closed, application, after: {step,
  frame_id, path, sha256, after_ms, interval_ms, still_ms, still_max_ms, still, waited_ms,
  captures, kept} | null, warnings: [] | ["after_not_still"]}` (`after` `null` on the entry step).
  `path` is the stored frame: the agent reads the next screen there.
- **`record stop`** still requires a recognition mark on every effective step but the entry step
  (`record_step_without_recognition`, "Pre-checks"): the steps are marked afterwards with `record
  mark --step k`, as a press's are.

In PowerShell 7.3 or later, with the install root `C:\ActingCommand`:

```powershell
$lab = 'C:\ActingCommand\tools\actinglab.exe'
& $lab --json --instance emu-a record start --task-id daily_open
& $lab --json --instance emu-a tap 962 658 --record
& $lab --json --instance emu-a session app restart --record --after-ms 3000
& $lab --json --instance emu-a tap 640 360 --record
& $lab --json --instance emu-a record archive
```

## The frames around a press: `record frames`

#463 S2-13 (0.12.64; the owner on board #463: in stage two the Lab may need to give an agent that
asks the several, or several dozen, frames before and after a press; the rules here are the
coordinator's). `record frames --step <k> [--before <0..100>] [--after <0..100>] [--record-id
<id>]` (MCP `ac_record_frames` with `step`, `before`, `after` and `record_id`) reads the recording
`record status` reads, or the one `--record-id` names (active, archived or stopped), takes no lock,
writes nothing and decodes no frame: it lists what the step table holds. Its sequence is every
frame stored under a step, its page transition's and superseded ones included (an ending's are
not), in the order the frames entered the step table, which their frame ids number: a frame
captured from the device enters when it is captured; a frame attached from a file (a mark's sample,
a page transition's frame, an offline frame, a step inserted after the stop) enters when it is
attached, and its time (`captured_at_unix_ms`, the step table's `recorded_at_unix_ms`) is that
moment, not the moment its screen was captured. `press` is step k's press as its click execution
holds it (`executed_at_unix_ms`, `point`, a swipe's `to_point`) with the frame it was sent on
(`before_frame`), `before` the last `--before` (3 without it) frames before that frame, `after` the
first `--after` (10) frames after it, across step boundaries, `before_total` and `after_total` how
many each side holds, and `offset_ms` a frame's time minus the press's time. A step without an
executed press (an application operation, an entry launch, a step without an effect) has no
`point`: its time and its `frame` are those its application operation's completed receipt names
(`executed_at_unix_ms`, recorded since 0.12.64, and `before_frame`), or `null`, and without a frame
the sides are split at the press's time (a press an earlier build recorded: the frames recorded at
or before it, and after it) or, without a time either, at the step (the frames stored under the
steps numbered below k, and those stored under k and above it), `offset_ms` `null` where there is
no time. Paths are absolute, as the archive's manifest names the files; a frame whose file is not
there is listed with `path: null` and named in the warning `frame_file_missing`, and a press sent
on a capture that was not stored (`sent_on_unstored_capture`) answers the stored frame its step
names and the warning `press_frame_not_stored`, its `after` being the frames after that one
recorded after the press's time (one recorded between them, a mark's sample or a transition's
frame, is `before`); `--before` or `--after` outside 0..=100 (`details{argument, reason:
"range", value, min, max}`), not a whole number or given twice (`details{argument, reason: "range",
value}`) is `validation_failed`, and a step that does not exist, is dropped or was converted is
`record_step_not_found`.

```json
{"record_id": "…", "step": 3, "artifact_step": 3, "kind": "tap",
 "press": {"at_unix_ms": …, "point": {"x": 962, "y": 658},
           "frame": {"frame_id": "f0006", "path": "…", "sha256": "…",
                     "captured_at_unix_ms": …, "stored_under_step": 3}},
 "before": [{"frame_id": "f0004", "path": "…", "sha256": "…", "captured_at_unix_ms": …,
             "offset_ms": -1830, "stored_under_step": 2}],
 "after": [{"frame_id": "f0007", "path": "…", "sha256": "…", "captured_at_unix_ms": …,
            "offset_ms": 104, "stored_under_step": 4}],
 "before_total": 4, "after_total": 12,
 "warnings": [{"code": "frame_file_missing", "frame_ids": ["f0009"]}]}
```

## Optional steps

A step whose screen a run does not always show (a daily notice, a reward pop-up, an update
prompt) is marked optional (Workflow #339). The package skips an optional step whose page
does not appear: its page is not recognized, its effect is not executed and nothing is
recorded for it. After the page that follows a run of optional steps is first seen, the
package keeps watching for a late optional page for `settle_ms`. `record stop` writes
`"optional":{"settle_ms":S}` on the operation of an optional step and checks the optional
pages against the screens a run may show instead ("Package generation").

```
record mark [--step <k>] [--frame <png>] [marks…] [--click … | --click-from <id>] [--click-retry <n>]
            --optional [--settle-ms <0..60000>]
record mark [--step <k>] --not-optional
```

- `--optional` applies to the step the command targets, as marks do: `--step k`, the step a
  new `--frame` opens, or the open step. Without `--settle-ms`, a step that is not optional
  yet takes 2000 ms and an optional step keeps its value. `--not-optional` clears it;
  clearing a step that is not optional changes nothing.
- They go in one command with the step's marks, samples, page and click. Combined with
  `--transition` or a step operation (`--drop-step`, `--reopen-step`, `--close-step`,
  `--to-transition`) they are `validation_failed` (2), as are `--optional` with
  `--not-optional` and `--settle-ms` without `--optional`.
- Request form: `"optional": null | true | false` and `"optional_settle_ms": null | n`;
  `optional_settle_ms` needs `"optional": true` (`validation_failed`, 2).
- The step is checked after its effect is applied, and a refusal writes nothing:

| code | exit | when |
|---|---|---|
| `record_optional_settle_invalid` | 2 | `--settle-ms` above 60000 (`details{settle_ms, max_settle_ms}`) |
| `record_optional_first_step` | 3 | the step is the first effective step: the package starts from its page; start the recording one screen earlier |
| `record_optional_application` | 3 | the step has an application operation, also one declared in the same command or added to an optional step: there is no conditional restart |
| `record_to_transition_invalid`, reason `optional` | 3 | `--to-transition n` on an optional step: a transition must be seen |

- `session app <verb> --record` and `session instance app <verb> --record` that would land on
  an optional step are refused with `record_optional_application` (3) while the step is
  planned, before any Runtime request; nothing is sent and the recording is unchanged.
- Whether an optional step is the last step, or the main interface after a restart, is
  checked by `record stop` ("Pre-checks"), not by `record mark`; `record stop` also refuses an
  optional page that passes where it is absent and a next page that passes under a pop-up
  ("Self-checks", item 7). Run `record stop --dry-run` before the final stop.
- `recording.json`: the step carries `"optional":{"settle_ms":2000,"marked_at_unix_ms":…}`;
  `marked_at_unix_ms` is set when the step becomes optional or its settle changes. A step
  without it has no `optional` key, so a recording without optional steps is byte-identical
  to one written by a build without them, and older recordings are read unchanged.
  `step_state` and the steps of `record status` show `optional: null | {settle_ms}`; the
  success status stays `marks_recorded`.
- Builds without optional steps (#336 L3 and L3b) refuse the flags (`record mark does not
  accept: --optional`) and a request with `optional` (unknown field), both exit 2 with the
  recording unchanged, and cannot read a recording that has an optional step (unknown
  field). Do not mix builds during a recording.

### Inserting an optional step offline

A pop-up that did not appear while recording is inserted from a whole frame of it with the
recording's size, taken from:

- `capture --out <png>` on a day the pop-up appears, or
- the frame of a failed run kept in the actingd evidence store (a PNG such as
  `artifacts/<nn>/artifact_<id>.png`).

Right after the effect of the step before it was executed:

1. `record mark --frame <popup.png> --template … --click … --optional [--settle-ms n]`: the
   previous step has its effect, so the offline frame opens a new step.
2. `record mark --close-step`: the offline step declares a click that was not executed, and
   `capture --record` would refuse it (`record_step_click_not_executed`).
3. `capture --record` of the next screen.
4. A second copy of the same pop-up from the same PNG also needs `--close-step` first: an
   offline frame byte-identical to the open step's primary frame does not open a new step
   (it targets that step). The copy takes the first copy's marks with `--reuse <id>` and
   the same click rectangle.

An active recording appends its steps with new serial numbers. #463 R4: a pop-up found missing
after the recording is inserted after the stop, from its frame, without recording again
("Inserting an optional step after stop", below). #463 R2: a screen that ends the business another
way after its last press (for example too little balance) is no pop-up: it is added after the stop
as an ending, from its frame, without recording again ("Endings").

### Inserting an optional step after stop

#463 R4 (since 0.12.63, [v012-register.md](v012-register.md) §1.388; the owner's rule two on board
#463: only the main path is recorded, and what a run meets later is added from that run's frame
without recording again). A pop-up that a run meets after the press of step k and after which the
path goes on (a daily notice, a reward) is taken by the stopped recording as an optional step after
step k's effect and before the step that follows it:

```json
{"schema_version":"actingcommand.lab-record-mark.v1",
 "optional_after":{"step":1,"frame":"D:\\frames\\notice.png","samples":[],"page":"notice",
   "add":[{"id":"ui/notice_title","family":"template","region":{"x":480,"y":200,"width":320,"height":40}}],
   "reuse":[],"click":{"region":{"x":600,"y":520,"width":80,"height":40}},"settle_ms":2000}}
```
```json
{"schema_version":"actingcommand.lab-record-mark.v1","remove_inserted":3}
```

- **The members.** `step`, the step k after whose effect the run met the pop-up (its serial number,
  as `record status` shows it). A run that stopped on the pop-up
  (`contained_task_linear_unrecorded_screen`) names that step by `pack_operation_id`,
  `step_<nn>_click` or `step_<nn>_select`, where `nn` is its number inside the package, which
  differs from its serial number once a step was inserted after the stop or dropped (fix round 2).
  The one way from the first to the second: in `record status`, find the row whose
  `artifact_step` is the number `nn` (`02` is 2) and take its `index`. `frame`, the pop-up's screen, a PNG file on the
  caller's machine;
  `samples`, more frames of it; `page`, its page's name (a step's grammar); `add` and `reuse`, its
  marks, as a step's; `click`, the click that closes it, as a step's `click` (declared, never
  executed: it is the click the package presses); `settle_ms`, its settle (0..60000, default 2000).
  The command form is `record mark --optional-after <k> --frame <png> …`: `--optional-after` makes
  `--frame`, `--sample`, `--page`, the marks, `--reuse`, the click flags and `--settle-ms` the
  inserted step's. `record mark --remove-inserted <n>` gives the inserted step `n` up. The MCP tool
  `ac_record_mark` hands the request form to this command unchanged.
- **Alone.** The request names nothing else (`with_other_change`, `details.members`): a retry or a
  click guard is added afterwards with `record mark --step <n>`, as on any step after stop.
- **Its frames** are admitted as an ending's ("Endings", "Where the frame comes from"): stored
  content-addressed in the recording, of the recording's size, a pure scale converted with
  `scaled_from` kept, any other size `record_frame_size_mismatch`. Take the frame from the run's
  pinned frame as an ending's: `run evidence <request_id>`, then `material <ref> --out <dir>`.
- **The step.** It takes the next serial number and `inserted_after`, the recorded step it runs
  after: k, or, when k is itself an inserted step, the recorded step k runs after, whose run the new
  step joins (the order inside a run of optional steps is free, [linear-steps.md](linear-steps.md),
  "Optional steps"). Several steps inserted after one step run in the order they were inserted. It is
  closed as `--close-step` closes a step whose click was declared and never executed
  (`closed_by: "author"`), and optional with its settle; its marks, click and page then go through
  the marks flow of `record mark --step <n>`: every mark is self-tested on its frames
  (`record_mark_rejected`), the click and optional rules hold, and nothing is written when one
  fails. Afterwards the step takes marks, samples, a transition, a retry or a guard as any step of a
  stopped recording; `--not-optional` on it is `clears_optional` (a step never recorded on the path
  stays optional).
- **`record stop`** generates it in its place: the package numbers it, its page and operation
  (`"optional": {"settle_ms"}`) come between step k's and the next step's, and every check of an
  optional step applies ("Pre-checks", self-check item 7): a page that passes where the pop-up is
  absent (`record_optional_step_ambiguous`) and a next page that passes under the pop-up
  (`record_optional_skip_target_insensitive`) are refused with their own codes and nothing is
  written. Its step provenance adds `inserted_after`. A run that shows the pop-up after step k's
  press presses its click and goes on; one that does not show it goes on after the settle, as
  before.
- **Giving it up** (`remove_inserted`) drops the step as `--drop-step` drops one: it stays in the
  file with its frames, its marks free their ids, and the next `record stop` generates the package
  it generated before (but for its generation time). A mark of it that another step, a transition or
  an ending reuses, or another page forbids, is `record_mark_in_use`.

`record_optional_after_invalid` (exit 3, `details{step, optional_after_problem, outcome}`) refuses
the command and nothing is written:

| `optional_after_problem` | when |
|---|---|
| `recording_active` | the recording is active: a step is inserted, or given up, after `record stop` |
| `with_other_change` | the request names another member beside `optional_after` or `remove_inserted`, or both of these (`details.members`) |
| `step_unknown` | `step` names no step of the recording, or a dropped or converted one |
| `step_last` | `step` names the last step: nothing is pressed after it (a screen after the last press is an ending) |
| `step_application` | `step` names a step whose effect is an application operation |
| `no_marks` | the inserted step adds and reuses no mark |
| `no_click` | the inserted step declares no click |
| `clears_optional` | `optional: false` (`--not-optional`) on an inserted step |
| `not_inserted` | `remove_inserted` names no inserted step that is not given up yet (`details.inserted`, the ones there are) |

A page name off its grammar is `validation_failed` (2) and a settle above 60000
`record_optional_settle_invalid` (2), as for a step.

## Recording lock

Every state-writing command (`record start`, `mark`, `stop` including `--dry-run`,
`step`, `amend`, Workflow #449 REC-ARCH's `archive` and `reopen`, and
`capture`/`observe`/`do --record`)
takes `<state>/record-<instance>.lock` (`session app … --record` too, from before its
Runtime request until the result is recorded) with an exclusive operating-system lock without
waiting, holds it until its output is printed (`do --record` holds it across the device
click; #463 L1, in "One call per step" across the marks, the press and the next frame) and writes
the holder to `.lock.json`. `record status` and `record candidates` do
not lock.

- Held by another process: `record_busy`, exit 3, `blocked_by:["record_lock"]`,
  `details{instance, lock_path, holder}` (holder from `.lock.json`, `null` when unreadable).
  Nothing is queued or retried: wait, run `record status` to see the current step, then
  decide; a `record mark` without `--step` retried blindly may land on a step the other
  process just opened.
- Other lock failures: `record_lock_failed`, exit 5.
- The lock is released with the process handle, so a crashed or killed holder leaves no
  stale lock; there is no unlock command. The lock files are never deleted.
- v0.9.0 ActingLab does not know the lock; do not mix versions during a recording.

## Archiving (`record archive`, `record reopen`)

Workflow #449 REC-ARCH (0.12.59; the owner approved the feature on board issue #449, the rules
here are the coordinator's). People also record only to keep material, the frames and where they
tapped or swiped, while they operate by hand. Such a recording cannot pass `record stop`, which
builds a package and so requires a mark on every step and a last step without an effect; and while
it stays active every later `--record` action on the instance, whoever sends it, adds a step to it.
`record archive` ends exactly such a recording and keeps all of it; `record reopen` makes it active
again, so that it can be marked and stopped later.

```
record archive [--record-id <id>] [--copy-to <dir> [--all-frames]] [--state-dir <dir>]
record reopen [--record-id <id>] [--state-dir <dir>]
```

#463 S1-5 (0.12.64): `record archive` is how stage one of package making ends: it hands over the
recorded material of the business ("The manifest" below), asks nothing of the steps, and needs no
device.

Both are rows of [lab-operations.json](lab-operations.json) (`record archive`, `record reopen`):
commands of actinglab (also as `session record archive|reopen`), calls of `serve --stdio` and the
tools `ac_record_archive` and `ac_record_reopen` of the Lab's MCP server. Neither has a dry run
(`dry_run_unsupported`, alternative `record status`). Each takes the recording lock, so the
holder file names `record archive` or `record reopen`; neither records a client action or any
other trace, as `record stop` records none.

**Naming.** A recording is named by its `record_id`: the id its session header carries
(`record-<instance>.json`, from `record start --record-id` or generated), which also names its
directory `record-artifacts/<record_id>/lab/`. Without `--record-id` both commands name the
recording of the instance's session header. A recording whose header was replaced (by `record
start --force`, or by a later `record start` after it was archived) keeps that id in its step
table and its directory, and is named by it (fix 1, "Left behind by `record start --force`").

### record archive

On the instance's active recording (its session header and its step table `active`), whoever
started it and whoever added its steps:

- It requires nothing of the steps: no mark, any last step, steps of any mix of initiators,
  dropped or converted steps, and a step table written by an earlier release (the format has not
  changed since 0.12.47, Workflow #434 LAB-2; no swipe step is needed or assumed).
- It writes the manifest `archive-manifest-<n>.json` into the recording's directory, beside
  `recording.json` and `frames/`: `n` is one above the highest manifest there, 1 for the first,
  and an existing file is never replaced.
- Then the `record` command sets the session header's `status` to `archived` (and its
  `updated_at_unix_ms`): the session is ended in a state of its own, distinct from `stopped`. The
  instance has no active recording, and `record start` needs no `--force`.
- It never deletes, moves or rewrites a frame or the step table (the step table still says
  `active`); the archive asks the Runtime nothing and reads no frame's pixels: it reads the step
  table and the session header, and hashes files only to check them. #463 S1-7: once its manifest,
  the session header and the copy are written, the recording releases its own hold through the
  Runtime ("A recording holds its instance"); a release that fails keeps the hold and undoes
  nothing of the archive.

**The manifest** (#463 S1-5, since 0.12.64: `actingcommand.lab-recording-archive.v2`, the only
schema written; the rules are the coordinator's, on board #463) holds only what the step table and
the session header hold, and states the material stage one hands over: each press with the frame it
happened on and where it was pressed, and the screen the business ended on. A reader without a
Runtime (stage two, another agent, a person) reconstructs the business from the manifest and the
files alone.

```json
{"schema_version":"actingcommand.lab-recording-archive.v2",
 "record_id":"…","instance":"…","task_id":"…","application":{"game":null,"server":null},
 "locale":null,"coordinate_space":{"width":1280,"height":720},"note":null,"archive_note":null,
 "started_at_unix_ms":0,"archived_at_unix_ms":0,"step_count":2,"written_by":"<commit>",
 "step_table":{"file":"recording.json","sha256":"…"},
 "material":{
   "clicks":[{"step":1,"kind":"tap",
     "frame":{"step":1,"frame_id":"f0001","file":"frames/<sha256>.png","sha256":"…"},
     "derived":false,"sent_on_unstored_capture":false,"entry":false,
     "point":{"x":962,"y":658},"to":null,"duration_ms":null,"action":null,"outcome":"performed"}],
   "final_frame":{"step":2,"frame_id":"f0002","file":"frames/<sha256>.png","sha256":"…"}},
 "copy":null,
 "steps":[{"step":1,"kind":"tap","dropped":false,"converted_to_transition":false,
   "frames":[{"frame_id":"f0001","role":"primary","file":"frames/<sha256>.png","sha256":"…",
              "superseded":false,"recorded_at_unix_ms":0}],
   "before_frame":"f0001","after_frame":{"step":2,"frame_id":"f0002"},
   "after_capture":{"after_ms":800,"still_max_ms":0,"still":null,"waited_ms":800,"captures":1,"kept":0},
   "derived":false,"outcome":"performed",
   "tap":{"rect":null,"point":{"x":962,"y":658},"source":"point"},
   "swipe":null,"long_press":null,"application_op":null,
   "before":null,"after":null,"page":null,"package":null,"note":null,
   "at_unix_ms":0,"request_id":"…"},
  {"step":2,"kind":"none","…":"…"}]}
```

- The header: `record_id`, `instance`, `task_id` and `started_at_unix_ms` as the session header
  names them, read from the step table, which carries the same four (`record start` writes them
  from the header, and every command holds them equal to it; for a recording `record start
  --force` left behind they are what its replaced header held); `application`, the game and server
  the step table names (the session header names
  no application; `null` where the step table has none); `locale` and `coordinate_space` (the size
  of its frames), the step table's; `note` and `archive_note`, `null` without one;
  `archived_at_unix_ms`; `step_count`;
  `written_by`, the commit of the program that wrote the manifest; `step_table`, the step table's
  file and its sha256 when the manifest was written, against which a copy is checked; `copy`,
  `null` in the recording's own directory, which holds every file the manifest lists ("The copy").
- `material`, what stage one hands over; a reader that wants only that reads `material` and
  nothing else:
  - `clicks`: one entry per effective step whose effect is on record (its `outcome` is not
    `null`), in the order of the steps: `step`; `kind`, the row's; `frame`, the frame the press
    happened on (the row's `before_frame`) with the step of the row that lists it, its `file` and
    its `sha256`, `null` when no frame was stored for it (the application entry step, which has no
    frame); `derived`, the row's (the frame was given by the rule below); `sent_on_unstored_capture`,
    whether the press was sent on a capture that was not stored (an open step with marks,
    "Recording while playing"), so that `frame` is the stored frame the step stood on: for a press
    whose execution names its own frame before (`derived: false`), the step table's own member
    (`steps[].click.execution.sent_on_unstored_capture`, left out there when `false`), `true` or
    `false`; `null` ("the step table does not say"; the member is written, never left out) for a
    press whose frame the manifest derives (`derived: true`: a press of `do --capture --record`, a
    press recorded the old way) and for an application operation, whose receipt does not record
    it; the manifest claims nothing the step table does not say; `entry`, the operation opened the
    application entry step (there was no screen before it); where it was pressed: `point` (a tap's
    or a long press's point, where a swipe began), `to` (where a swipe ended), `duration_ms` (a
    swipe's or a long press's), `action` (an application operation's, in place of a point: a force
    stop is `stop`, the row's `application_op.cli_verb` keeping `force-stop`), each `null` where it
    does not apply; and `outcome`, the row's.
  - `final_frame`: the screen the business ended on, the live primary frame of the last effective
    step (with its step, file and sha256) when that step has no effect and the effective step
    before it has its effect on record (the step that effect's screen after opened); `null`
    otherwise: fewer than two effective steps; the effective step before the last has no effect on
    record (as when no effect is on record at all); or the last effective step has an effect (its
    screen after could not be taken, its outcome is indeterminate, or the step after it was dropped
    or became its transition and no frame came after). Then the answer's `lab.warnings` holds
    `final_frame_missing`; the archive stands either way, since ending a recording asks nothing.
- One row per step of the step table, in its order, dropped and converted steps included:
  - `step`, the step's serial number; `kind`: `tap` (a click without a gesture), `swipe` (a drag: a
    recorded swipe or a declared drag), `none` (no input), or the step table's own word for any
    other effect (`long_press`, `single_touch_drag_with_vertical_brake_v1`,
    `single_touch_path_drag_v1`, `application`); `dropped` and `converted_to_transition`, as the
    step table has them.
  - `frames`: the step's frames and its page transition's, in the step table's order (its own as
    they arrived, then its transition's), superseded ones included, each `frame_id`, `role`, `file`
    (`frames/` and the file name of its recorded path), `sha256`, `superseded` and
    `recorded_at_unix_ms` as the step table has them.
  - `before_frame`, `after_frame` (`{step, frame_id}`, `step` the row whose `frames` list it) and
    `after_capture`: the frame the step's effect was on, the frame the screen became and how that
    frame was taken, as the step table names them (`steps[].click.execution`,
    `steps[].application.executed`; "Recording while playing", "In the middle of a business").
    `after_frame` is the last capture of the effect's own captures after it (#463 S1-2, "The screen
    after": the first opens the next step, each later one supersedes the one before it), or, when
    those kept nothing, the first frame that arrived after the effect: it may name a frame that has
    since been superseded (by the capture before the next press, or a later capture), a frame of a
    dropped step, or a frame of the step's own transition after `--to-transition`. An `after_frame`
    with `after_capture` `null` is a frame that arrived after the effect's own screen after could
    not be taken (the capture before the next press, or a `capture --record`).
  - `derived`: for an effect on record whose step table names no such frame (a press of `do
    --capture --record`, an application operation recorded before #463 S1-3, or one without a
    completed receipt) the row gives the rule's and says `derived: true`: `before_frame` is the
    step's live primary frame (a step's primary frame cannot change once its effect is on record,
    `record_step_frame_conflict`); `after_frame` of a click is the first frame of the next
    effective step by serial number when that frame was recorded at or after the click's
    `executed_at_unix_ms`, else `null`; of an application operation `null` (such a receipt or
    attempt carries no time); `after_capture` `null`. No frame id is invented and no stored id is
    called derived. The application entry step names none of them and is not derived.
  - `outcome`: a click's execution's (`performed` or `indeterminate`); an application
    operation's `performed` with its completed receipt (attempts may stand beside it: it was run
    again) and `indeterminate` with attempts and no completed receipt; `null` without an effect on
    record.
  - The effect by its kind, `null` otherwise: `tap` `{rect, point, source}` and `long_press`
    `{point, duration_ms, rect}`, `rect` `null` for a press declared by its point (`source:
    "point"`, "Recording while playing"); `swipe` `{from_rect, to_rect, from, to, duration_ms}`;
    `application_op` `{action, cli_verb, executed}`, `executed` whether its completed receipt is on
    record.
  - `before` and `after`, the Runtime frames the input's frame summaries name (`frame_id`,
    `sha256`), `null` without them (a press of stage one has none); `page`, the page recognised on
    the frame before the input, and `package`, the carrier package that recognised it, when the
    step table has them; `note`, `null` without one.
  - `at_unix_ms`: when the input was executed, a click's `executed_at_unix_ms` and an application
    operation's completed receipt's (`steps[].application.executed.executed_at_unix_ms`, recorded
    since 0.12.64; `null` for an operation an earlier build recorded and for one without a completed
    receipt: none is derived); for a step without an input, when its first frame was recorded;
    `request_id`, the input's request id (an application step's), `null` without one.
- **What a reader reconstructs.** What was on the screen at each press: `material.clicks[].frame`
  (a row's `before_frame`); where it was pressed: its `point`, `to`, `duration_ms` or `action`;
  what the screen became: a row's `after_frame`; where the business ended: `material.final_frame`.
  Every frame id names an entry of some row's `frames`, whose `file` lies under `frames/` with its
  sha256, and `record archive` hashes every file it lists, superseded frames included.
- **The frames around a press.** The frames before press k are the frames of row k up to and
  including its `before_frame`. The frames after it are the frames of the row `after_frame.step`
  names that were recorded at or after press k's `at_unix_ms` (its captures after it, the last of
  them its `after_frame`, the train of stage one), up to and including the next frame some row
  names as its `before_frame`, or to the end of that row's frames. A row lists its frames in the
  order they were recorded, each with `recorded_at_unix_ms` and `superseded`, and its effect is at
  `at_unix_ms`, so either run can be cut by time as well, however many captures a step holds.

**Manifests written before 0.12.64** (`actingcommand.lab-recording-archive.v1`, Workflow #449
REC-ARCH: the header without `locale`, `coordinate_space`, `note`, `archive_note`, `material` and
`copy`; rows of `step`, `kind`, `frames` of `{frame_id, role, file, sha256}`, `before`, `after`,
`page`, `package`, `tap` `{rect, point}`, `swipe`, `at_unix_ms` and `request_id`) are read and never
rewritten: named again their files are hashed; copied, the copy holds every file the v1 manifest
lists and the manifest byte for byte (it names no material, and the answer warns nothing); `record
reopen` names them; the next `record archive` of their recording writes the next manifest, as v2.
A manifest is read by the schema it names, exactly these two (`record_state_io_failed` for any
other); no flag writes v1.

**The copy** (`--copy-to <dir>`):

- Fix 1: `<dir>` is an absolute path. An empty value, a value of only white space and a relative
  path are refused `validation_failed` (exit 2, `details{path, reason}`, `empty` or
  `not_absolute`), as the module refuses its other parameters, before the session is read and
  before anything is written: no manifest, no copy, the session as it was. A relative path would
  name a place under the current directory of whichever process serves the command (an empty one
  that directory itself). The same rule holds on the command line and in `serve --stdio`; the
  Lab's MCP server refuses such a path itself before it sends anything (`arguments_invalid`,
  `not_absolute`), as it refuses every path parameter.
- The directory must not exist (it is created, with its parents) or must be empty. Anything else
  is refused `record_archive_target_not_empty` (exit 3, `details{path, reason}`, `not_empty` or
  `not_a_directory`) before anything is written: no manifest, the session as it was.
- Once the manifest is written and the session archived, `recording.json`, the manifest and (#463
  S1-5) the frame files the manifest's `material` names are copied under the same relative names;
  with `--all-frames` (the Lab's MCP server, `actinglab-mcp`: `all_frames`; the first-generation
  server's `ac_record_archive`, `actingctl mcp-serve`, has no such parameter and copies the
  material) every frame file the manifest lists, superseded ones included. The copy's manifest is
  the recording's saying which, `copy: {frames: "material" | "all"}`. The step table and the
  frames are copied first and each hashed against the manifest; the copy's manifest is written
  last, once they hold their sha256, and hashed against itself as written (#463 S1-5 fix round
  2): a copy that fails before it leaves no manifest in the directory, so no manifest there names
  a file the copy does not hold. A frame a row lists that the copy does not hold is not missing
  to a reader that reads `copy`: it stays in the recording's own directory, which keeps every
  file; an archive deletes nothing. The copy of a v1 manifest holds every file it lists and the
  manifest byte for byte, with or without `--all-frames`. `--all-frames` without `--copy-to` is
  `validation_failed` (`details{argument: "all_frames", reason: "copy_to_only"}`) before the
  session is read and before anything is written: exit 1 on actinglab's own command line, as every
  failure there and every other refusal of a flag given without the flag it needs; exit 2, the
  usage class, only on a `--via` line (the train of stage one: the code's exit, which its test
  pins).
- A file that cannot be copied is `record_state_io_failed` (exit 5, `details{file, path, target,
  reason: "copy_failed", copied}`); a copy that cannot be read or does not hold its sha256 is
  `record_archive_verify_failed` (exit 5, `details{file, path, checked: "copy", reason:
  "sha256_mismatch" | "unreadable", failures, copied}`). Both name the file and the reason and add
  `record_id`, `archived: true` and `manifest`: what was copied stays where it is, nothing is
  deleted, and the recording stays archived.

**Named again.** `record archive --record-id <id>` on an archived recording of the instance (the
session header names it `archived`, or its directory holds a manifest while its step table is
`active` and names the instance) writes no second manifest: it reads the latest one, v2 or v1
(`record_state_io_failed` when it does not read as that recording's manifest or names a file
outside the directory) and hashes every file it lists in the recording's directory, superseded
frames included
(`record_archive_verify_failed`, `checked: "recording"`, on a file that does not hold its sha256);
with `--copy-to` it then copies as above. That is the retry after a failed copy.

**Left behind by `record start --force`** (fix 1, the coordinator's ruling on the independent
review). `record start --force` replaces the session header and starts a new directory; the old
recording keeps its directory and its step table (`active`), and the header names another
recording. `record archive --record-id <id>` works on it as on any recording: it writes the
manifest from that directory's step table (`n` as above; `record_id`, `instance`, `task_id` and
`started_at_unix_ms` the step table's, which are what the replaced header held) and copies with
`--copy-to` exactly as above. It writes no session header, since the header belongs to the other
recording: the answer's `record` is `null` and `lab.session_header` is `{record_id, status,
changed: false}`, the recording the header names and its status (`null` without a header). Once it
has a manifest it is an archived recording: named again it gets no second manifest, and `record
reopen --record-id <id>` makes it the active one under the rule of every archived recording
("record reopen"), refused while another recording is active. The same holds for an archived
recording whose header a later `record start` replaced: its answer names that header too.

**After the manifest** (fix 1). Once the manifest is there, written now or before, a session
header that cannot be read or written fails the command with that failure, its `details` adding
`record_id` and `manifest{file, path, sha256, steps, written}`, so that the manifest's path is
known: the manifest stays, the header is as it was, and a later `record archive` of an active
recording writes the next manifest.

**The answer** is `{status: "archived" | "already_archived", record, path, lab}`: `record` the
session header when it names the recording, `null` otherwise; `lab` `{status, record_id,
record_started_at_unix_ms, instance, path (the recording's directory), manifest{file, path, sha256,
steps, written}, copy?{path, files}, session_header?{record_id, status, changed}, warnings}`,
`written` false for a recording named again, `session_header` present only when the session header
does not name the recording. #463 S1-5: `warnings` is `[]`, or `["final_frame_missing"]` when the
manifest it names (written now or named again, v2) has `material.final_frame` `null`: a warning
(exit 0), the archive standing; a v1 manifest warns nothing. #463 S1-7: and `hold{reason, own,
release, resumed?, shown?}` when the session header names the recording and held a pause ("A
recording holds its instance").

**Refused, nothing written:** `--copy-to` empty, blank or relative: `validation_failed` (above);
`--all-frames` without `--copy-to`: `validation_failed` (above); no
`--record-id` and no active recording (no session, or a stopped or archived one):
`record_session_not_active`; a named recording that is neither the instance's active one, nor
archived, nor one whose step table is `active` while the header names another recording:
`record_not_archived` (exit 3, `details{record_id, state}`, `state` `missing` without a step table
of that id, `stopped` for a stopped step table, the session header's status when it names the
recording and is neither `active` nor `archived`, `unarchived` when the header names it `archived`
and its directory holds no manifest); one of another instance: `record_instance_mismatch`; an
active session whose step table is missing or does not read: `record_lab_unavailable` (`reason`
`lab_recording_missing`, `lab_recording_unreadable` or `lab_recording_mismatch`).

### record reopen

Makes an archived recording of the instance its active recording again:

- It changes the session's state only. When the session header names the recording, its `status`
  becomes `active`; otherwise (the header of a later recording, stopped or archived, or none) the
  command writes a header of the recording in place of it, as `record start` replaces one:
  `record_id`, `task_id`, `instance` and `started_at_unix_ms` from the step table, `holder` and
  `lease_id` `null`, no `steps`. The step table needs no change (archiving never rewrote it), and
  every manifest stays as a file: a later `record archive` writes the next one
  (`archive-manifest-2.json`, …).
- Fix 1: a recording `record start --force` left behind is reopened so once `record archive
  --record-id` has written its manifest ("Left behind by `record start --force`"); before that it
  is `unarchived`. What the replaced header held besides those four (its `holder`, `lease_id` and
  the `record step` entries of `steps`, which `record stop` does not read) is not restored: nothing
  is guessed, and the package is built from the step table alone.
- Then `record mark` takes marks on its steps and `record stop` builds the package the same
  recording would have built had it never been archived.
- Refused, nothing written: while another recording is active on the instance,
  `record_session_active` (exit 3, `details{instance, active_record_id, record_id}`); on a
  recording that is not archived, `record_not_archived` (`state` `active` for the instance's active
  recording, `stopped`, `missing`, `unarchived`); on another instance's, `record_instance_mismatch`.
- The answer is `{status: "reopened", record, path, lab}`: `record` the session header as written;
  `lab` `{status, record_id, task_id, instance, record_started_at_unix_ms, path, manifest (the latest
  manifest's file name), steps, replaced_record_id}`, `replaced_record_id` the recording the
  replaced header named, `null` when the header named this one or there was none.

### Other commands on an archived recording

| command | on an archived recording (the session header `archived`) |
|---|---|
| `record status` | `record.status` and `lab.status` `archived`: no active recording |
| `record start` | starts the next recording without `--force`, replacing the header as after a stop; the archived recording keeps its directory and can be named by its id |
| `record mark`, `record step`, `record amend`, the `--record` device commands | `record_session_not_active`, as for every session that is not active |
| `record stop` | `record_session_not_active`, `details{reason: "recording_archived", record_id}`, nothing read or written: reopen it first |
| `record candidates` | as before (it reads the session header's own steps) |

## Package generation (`record stop`)

```
record stop [--lab-dir <dir>] [--package-id <id>] [--requires <package_id>]
            [--game <g>] [--server <s>] [--locale <l>]
            [--timeout-ms <ms>] [--arrival-timeout-ms <ms>] [--application-arrival-timeout-ms <ms>]
            [--dry-run] [--state-dir <dir>]
```

`record stop` turns a recording with steps into one `linear_steps` package
(`linear-steps.md`) in a content container (`package-reference.md`, "Containers"), checks it
the way actingd will load it, writes it, and closes both states. Other flags and positional
arguments are refused (`validation_failed`, exit 2), so a mistyped `--lab-dir` never skips
the package directory.

| state | `record stop` |
|---|---|
| no Lab recording, or one without steps | as before: the session file is stopped, `lab: null` (an empty Lab recording is stopped too) |
| an active Lab recording with steps | the package is generated, checked and written (below); `lab.status: "generated"` |
| a stopped Lab recording with a package | nothing is generated; with `--lab-dir` the package is copied there again (steps 1 and 3 of "Writing"); `lab.status: "already_generated"` |
| a stopped Lab recording marked again since its package was generated (Workflow #390 LAB-2, `amended_at_unix_ms`) | the package is generated, checked and written again ("Marking after stop"); `lab.status: "regenerated"` |
| the session file was already stopped (for example by v0.9.0) but the Lab recording is active | generated as above; `lab.session_already_stopped: true` |
| Workflow #449 REC-ARCH: the session is archived (`record archive`, "Archiving") | refused `record_session_not_active` (exit 3, `details{reason: "recording_archived", record_id}`) before anything is read or written; `record reopen` makes it active again |

**`--dry-run`** (also the global `--dry-run`) runs everything: the pre-checks, generation,
admission, the first decision, the cross-check and the `--lab-dir` checks. It writes nothing,
both states stay active, and the output has `status: "validated"`, `lab.status: "validated"`,
`lab.would_write[]`, `lab.lab_dir_status: "to_write" | "to_replace"` and `lab.lab_dir_to_create`.
Its `lab.warnings` are those the final stop prints: run it first, mark what it asks for, then
run `record stop`. The final stop closes the recording; afterwards `record mark` takes marks on
its existing steps only and `record stop` generates the package again ("Marking after stop"); a
missing pop-up is inserted after the stop (#463 R4, "Inserting an optional step after stop"), any
other missing step means recording again.

### Marking after stop

Workflow #390 LAB-2 (compromise L-02). A stopped recording whose package was generated (the
session file stopped too, as `record stop` leaves it) takes `record mark` on its existing
effective steps: marks (added, reused, removed, forbidden, an anchor), samples, pages, clicks,
click guards and retries (an executed click is still replaced by another click only after
`--reopen-step`, which is a step operation), transitions and optional steps, under the usual rules.
#463 R1 (fix 1): it also takes a select ("Selecting in a list"), declared or declared again: on a
step whose tap has an outcome on record the select turns that tap into its click, without
recording again, when the tap lies in the slot the pressed name selects on the primary frame
(`select_press_mismatch` otherwise; a drag, a long press, a followed or retried click and the last
step are refused, `record_select_invalid`), and the whole select is self-tested again on the
step's kept frames; the tap's own rectangle, source and mark stay beside the select
(`select.click_before`), and `clear_select` gives them back, so that the recording generates the
package it generated before. #463 R2: it also takes an ending ("Endings"), and gives one up. #463
R4: it also takes an optional step after the effect of one of its steps, from a frame, and gives
one up ("Inserting an optional step after stop"). It takes no other new step (a frame that is not
the targeted step's primary frame; an ending, "Endings", is not a step and is taken), no step
operation and no application operation: each is
`record_session_not_active` (exit 3) with `details{reason: "recording_stopped", change}`, and
nothing is written; nor does any `--record` device command (`record_session_not_active`).

- A change that is written sets the recording's `amended_at_unix_ms` (the status stays
  `stopped`); the output says `after_stop: true`. `--dry-run` writes nothing.
- The next `record stop` generates the package again through every stage of this section:
  pre-checks, generation, admission, first decision, cross-check and writing, which replaces
  `lab/out/<package_id>.<ext>` and, when `--lab-dir` is given again, `<lab-dir>/<package_id>.<ext>`
  in place. Each generation option `record stop` gives wins; otherwise the package keeps the value
  it was generated with (package id, game, server, locale, `--requires`, arrival timeouts),
  except the task timeout, which without `--timeout-ms` is computed again because the marks may
  change the steps' effects and transitions it is computed from. The recording's artifact is
  replaced and `amended_at_unix_ms` cleared; `lab.status` is `regenerated` (`validated` with
  `--dry-run`). A refusal leaves the recording, its old package and `amended_at_unix_ms` as they
  were.
- Without a change since the package was generated, `record stop` answers `already_generated`
  as before.

A refusal before the writing stage leaves the recording and the session active and writes
nothing; a failure during writing is described under "Writing" (with `written_files`). Fix the
recording and run `record stop` again.

### Pre-checks

Only the effective steps count (neither dropped nor converted to a transition), numbered 1..n in
the order a run meets them (`artifact_step`): the steps recorded on the path by their serial
numbers, each followed by the optional steps inserted after it after the stop, by theirs (#463 R4,
"Inserting an optional step after stop"); a recording without an inserted step is numbered in the
order of its serial numbers. The rows below count steps by these numbers; where a refusal's
`details` name a step, `step` is its serial number and `artifact_step` its number here.

| check | code (exit) |
|---|---|
| at least two steps | `record_no_steps` (3) |
| an application entry step is step 1 only | `record_application_entry_not_first` (3) |
| steps 1..n−1 each have one effect (a click, a select step's demonstrated press, or an application operation) | `record_step_click_missing` (3) |
| step n only recognizes | `record_final_step_has_click` (3) |
| every step but the application entry step has a mark; every page transition has a mark | `record_step_without_recognition` (3) |
| at most 1000 steps with an effect | `record_too_many_steps` (3) |
| game, server and locale are known: the stop options, else the recording (`record start`), else (game and server) the instance configuration | `record_locale_missing` (2) |
| `--package-id` (default `<game>.<server>.<task_id>`) is not empty, at most 256 bytes, without control characters | `validation_failed` (2) |
| `--requires`: not empty, at most 256 bytes, without control characters, not this package's id, and not on a package whose step 1 is an application entry (`details.reason`: `prerequisite_id_invalid`, `prerequisite_self`, `application_entry`) | `record_requires_invalid` (2) |
| after a `stop` step the next effect is a `launch` or `restart` | `record_step_after_stop_invalid` (3) |
| R25: after the last `launch` or `restart` a later step is the main interface (the kernel predicate `linear_main_interface`: page anchor `home`, or `step_<nn>_home` from `record mark --page home`) | `record_application_without_home` (3) |
| Workflow #339, in this order: step 1 is not optional (the entry gate checks only step 1); step n is not optional (every run ends on it); an application step is not optional (no conditional restart); the main interface after the last `launch` or `restart` (the first later step `linear_main_interface` recognizes) is not optional (`details{step, artifact_step}`) | `record_optional_first_step`, `record_optional_final_step`, `record_optional_application`, `record_optional_restart_segment_end` (3) |
| `--timeout-ms`, `--arrival-timeout-ms` and `--application-arrival-timeout-ms` given explicitly lie in 1..=1800000 | `validation_failed` (2) |
| every live frame and template crop still has its recorded sha256 and the recording's size | `record_frame_hash_mismatch`, `record_frame_size_mismatch` (3) |

A `--requires` that does not start with `<game>.<server>.` adds the warning
`requires_prefix_mismatch`. Steps whose click or application operation is marked
`needs_review` are not refused; they are listed in `lab.steps_needing_review`.

### Pages and documents

Numbers in page and operation ids have one width: two digits, or the digits of n above 99.

- Step i's page is `step_ii`, or `step_ii_<name>` with `--page <name>`; the application entry
  step has no page and its operation starts `from: "any"` (`entry_page: "any"`). A page
  transition after step k is the page `transition_kk`. Each page's rule is
  `{"required": [...]}`: the step's (or transition's) own marks and reused marks, without the
  members of its `any_of` checks and (#463 L2 fix 1) without the located-text mark its click
  follows, which is the click's guard only. Workflow #390 LAB-2: a step's page adds `"forbidden": [...]`,
  the marks it forbids ("Forbidden marks and anchors"); its anchor is not listed in `required`
  (the anchor declares the page, which requires the anchor's target `page/<page>`), and a page
  whose rule would be empty then has none. A page transition's rule only requires.
- Every own mark of an effective step and of its page transition is one target:
  `verify_templates[]` (`template: "assets/<name>.png"`, `region` the search area,
  `threshold` when declared), `color_probes[]` (`expected` and `max_distance` when declared, or
  the `color_digest.v1` digest), `ocr_targets[]`, `checks[]`. Templates and color probes carry
  a `provenance` object. Workflow #390 LAB-2: the template that anchors a page is that page's
  `anchors[]` entry instead (its `id` the page id, its provenance naming the mark as `mark_id`),
  whose target is `page/<page>`; every reference to it in the package (a guard's `target_id`, a
  followed click's `target_id`, a check member, another page's required or forbidden marks) names
  `page/<page>`.
- #411 Lab B-9: a value-test mark `<id>` is one `ocr_targets[]` entry per side, `<id>/left`
  and (when the right side reads a value) `<id>/right`, each with the side's region, languages,
  timeout, minimum confidence and model, `match_mode: "contains"`, `expected` the ten ASCII
  digits `"0"` to `"9"` and `case_sensitive: false` (the side's own verdict means "a digit was
  read"; the value test reads its text), and one `checks[]` entry `{"id": "<id>", "value_test":
  {"left": {"target_id": "<id>/left", "trim", "value", "part"?}, "op", "right"?}}` whose
  `right` is `{"const"}` or `{"target_id": "<id>/right", "trim", "value", "part"?, "mul",
  "add"}`. The page rule requires the value test as any own mark (not as a member of an
  `any_of` check); the side targets are never in a page rule.
- `control.json`: `Lab-1y.control.v2`, `execution_mode: "linear_steps"`, the package id, game,
  server, resolution (the recording's frame size), `entry_task_id` (the task id),
  `prerequisite_package_id` with `--requires`, `timeout_ms`, `step_timeout_ms` and `max_steps`
  (the number of steps with an effect). `resources/operations/resources.json` is
  `{"schema_version":"1.0","resources":[],"resource_count":0}`.
- `task.json` (schema `0.9`): `server_scope: [server]`, the locale, the recording defaults,
  `timeout_ms` and `max_steps` equal to the control's, `target_page` the last page, and the
  `scheduling_outcome` mapping `{"outcome_key": "<task_id>_done", "effect":
  "no_designated_effect", "terminal_pages": [<last page>]}`, then (#463 R1) one mapping per select
  step, its `on_empty` key on its page ("Selecting in a list"), then (#463 R2) one per ending, its
  key on its page, in the order the recording took them ("Endings").
- #463 R2: an ending's page is `ending_kk`, or `ending_kk_<name>` with its `page`, `kk` its number
  in the order the recording took the endings, in the width of the steps' numbers; its rule is
  `{"required": [...]}`, its own and reused marks as a step's page; its own marks are targets as a
  step's, their provenance naming the ending by its key (`ending`) and no step. The task's
  provenance adds `endings[{ending, outcome_key, page, frame_sha256, sample_sha256, frame_size,
  marks, marked_at_unix_ms}]`. A recording without an ending generates the same bytes as before.
- A click step's operation is `step_ii_click`: `from` its page, `to` and `expect_after.page_id`
  the next page, `click: {"kind": "rect", …}` with the recorded rectangle (Workflow #396
  REC-4: `{"kind": "target", …}` or `{"kind": "offset", …}` for a click that follows a template,
  see "Following a template"; Workflow #390 LAB-2: `{"kind": "drag", "from", "to",
  "duration_ms"}` with the recorded rectangle and destination for a drag, `{"kind": "long_press",
  "x", "y", "width", "height", "duration_ms"}` with the recorded click box for a long press;
  #411 Lab BD: `{"kind": "single_touch_drag_with_vertical_brake_v1", "from", "corner",
  "horizontal_duration_ms": 200, "corner_hold_ms": 150, "brake_distance_px": 100,
  "brake_duration_ms": 200}` with the recorded rectangle as `from` and the corner box for a
  brake drag; Workflow #406 T4: `{"kind": "single_touch_path_drag_v1", "box", "turns",
  "min_segment_px", "duration_ms", "corner_hold_ms"}` with the recorded rectangle as `box`,
  compiled to `box_rect`, for a path drag, whose first decision `record stop` checks as a path
  of `turns + 2` points inside the box, its segments summing to `duration_ms`, with the declared
  corner hold; see "Drags and long presses"),
  `expect_after.timeout_ms` the arrival timeout T (`--arrival-timeout-ms`, default 15000),
  `interval_ms` 500, `post_delay_ms` 200, and `retryable`, `max_attempts`,
  `retry_interval_ms` only when the step declares a retry. Its guard target is (#463 L2 fix 1)
  the located-text mark a click follows, a mark of the step and of no page rule; otherwise it
  comes from the page's required marks only: the template a click follows, the click guard, the
  click source unless it is OCR, the first template, the first color or color digest, the first
  check. A followed template that is not a required mark of the page, and a followed located-text
  mark that an `any_of` check names or whose page would need it (`page_needs_followed_text`), are
  refused with `record_click_follow_invalid`. The guard's `expected_rect` is
  the template's search area, the color or digest region, or the bounding box of the check's
  members. #411 Lab B-9: a value test named by `--click-guard` guards as a check does,
  `"check": "<id>"`, with `expected_rect` the bounding box of its sides' regions (a value-test
  member of a check counts with that box). A page with only OCR marks gives no guard and
  `unguarded_trusted_coordinate: true`, unless the click follows one of them, a located-text
  mark, which then guards it (`verify_text`). #463 R1: a select step needs a guard: its page needs
  a template, color, color digest or check mark (`record_select_invalid`, `select_problem:
  guard_missing`).
- #463 R1: a select step's operation is `step_ii_select`, with its `select`, its guard, its
  `expect_after` and its `post_delay_ms` and no click; its layout is in `candidate_layouts[]`, its
  slots' targets in `ocr_targets[]` and its policy document is `policies/step_ii_select.json`
  ("Selecting in a list"). A recording without a select generates the same bytes as before.
- An application step's operation is `step_ii_app`: `application: {"action"}`,
  `expect_after.timeout_ms` the application arrival timeout A
  (`--application-arrival-timeout-ms`, default 90000), `post_delay_ms` 1000, no guard and no
  retry.
- A transition is `{"kind": "page", "page_id": "transition_kk"}` (with `timeout_ms` when
  declared) or `{"kind": "window", "min_ms", "max_ms"}`.
- An optional step's operation adds `"optional": {"settle_ms": S}` (`linear-steps.md`,
  "Optional steps"); its operation provenance and its entry in the task provenance add
  `"optional": {"settle_ms", "marked_at_unix_ms"}`. A run is a maximal sequence of consecutive
  optional steps; the page after it is its skip target N. A recording without optional steps
  generates the same bytes as before.
- `control.step_timeout_ms` is min(T, 60000); a larger T adds the warning
  `step_timeout_clamped`. Without `--timeout-ms` the task timeout is
  `S·[step 1 is no application entry] + Σ a·(g + w + (b − w)⁺ + p + T) + Σ_app 10000 +
  Σ (a − 1)·(r + S) + Σ_runs max(settle) + 10000` over the steps with an effect, where S is the
  step timeout, a the attempts, g the duration of a drag or long press (Workflow #390 LAB-2; 0
  for a tap and an application operation; #411 Lab BD: 550 for a brake drag), w max(post delay,
  window minimum), b the window maximum (0 without), p the page transition timeout (T without
  its own, 0 without a page transition), T the arrival timeout (A for an application step), r
  the retry interval, and max(settle) the
  largest `settle_ms` of a run of optional steps: the sums before it count every optional step
  as shown, the longest path, and a run settles at most once, only when it is skipped. Above
  1800000 it is clamped and the warning `task_timeout_clamped` added. The default covers the
  usual path only; give `--timeout-ms` for more.
- `provenance` (top level and per operation) records the recording, the generator, every step
  (record index, page, frame and sample sha256, frame size, marks with their mark-time result
  and margin, click, application, transition), the warnings, `arrival_by_time_window` and the
  cross-check. `generated_at_unix_ms` is the recording's `updated_at_unix_ms`, so the same
  recording generates the same bytes again.
- #463 L2: a located-text mark is its `ocr_targets[]` entry with `locate` and, when declared,
  `unique`; a click that follows it is `{"kind": "target", …}` or `{"kind": "offset", …}` on it,
  guarded by it (`verify_text`, `expected_rect` its region) and (fix 1) named by no page rule,
  and its operation adds
  `"input_recheck": {"kind": "fresh_frame"}`: the guard is evaluated again on one fresh frame
  of the step's page and the input drawn from that frame ("Following a located text"). The
  default task timeout does not count that fresh frame (one capture of the step's page, within
  the margins of the usual path). A recording without a located-text mark generates the same
  bytes as before.
- #463 R1: the default task timeout adds `S` for each select step, its confirmation on a fresh
  frame.

### Container and name

With a template mark the package is a ZIP of the content-directory layout, otherwise one
`actingcommand.package.content-json.v1` document; both hold the same files:

```
control.json
resources/operations/resources.json
resources/operations/<task_id>/task.json
resources/operations/<task_id>/assets/<name>.png      (ZIP only)
```

JSON documents are pretty-printed with a final newline. The ZIP has no directory entries,
`/`-separated ASCII names in byte order, deflate and the timestamp 1980-01-01; the JSON
container lists the files in path order. Workflow #336 PKG-S5 (option 丙): the package is
named by its `package_id`, as `<package_id>.zip` or `<package_id>.json`; no digest of it is
computed, recorded or answered. `--package-id` (or the default `<game>.<server>.<task_id>`)
must therefore also be one path segment usable as a file name (`validation_failed`, exit 2).

### Self-checks

Preparation uses `PreparedContainedTask` and the same linear candidate-set and target-consensus
budget rules as Runtime. First-decision simulation uses the same kernel observation and guard
owners. The recorder emits no `target_consensus` declaration: its per-frame cross-checks and
first-decision result describe the generated package only, and do not certify multi-frame
consensus, live OCR/provider execution or prerequisite recovery. Author-supplied declarations
are checked by `package preflight` through that same preparation owner; actual sample and
guard execution still requires the corresponding frames, provider and Runtime evidence.

1. **Container round trip.** The encoded bytes expand (`expand_content_container`) to exactly
   the generated files, compared entry by entry (path and bytes); a difference is
   `record_artifact_admission_failed` with `details{stage: "container_round_trip", paths}`.
2. **Admission.** The kernel admits the files as actingd loads a content directory: source
   compilation, declarations and the `linear_steps` rules, with the Lab's OCR
   provider when it has one (admission never calls it). A refusal is `record_artifact_admission_failed` (3) with
   `details{stage, loader_code, detail, declaration{file, pointer, reason}}`. #463 L2 fix 1: the
   static read budget of OCR targets ([ocr-fields.md](ocr-fields.md)), a guard's reads (a
   located-text guard's among them) and a page's, is checked only here, never when a mark is
   recorded (a mark's self-test evaluates its one target alone): an author learns of an exceeded
   budget at `record stop`, for a guard as `loader_code` `resource_declaration_invalid` with the
   pointer `/operations/<i>/guard`.
3. **First decision.** The offline simulation of the package on recorded frames:
   - a package with an application step, on its first live frame, must be refused with
     `application_effect_requires_assigned_application` before any capture:
     `first_decision{status:"not_evaluated", reason:"lab_application_effect_offline",
     refusal}`; when step 1 is a click, the warning `first_decision_not_evaluated` says its
     click was not simulated;
   - without an OCR model, a step 1 that involves OCR (an OCR or, #411 Lab B-9, value-test
     mark, or a check with such a member, among its required marks, which includes a
     trusted-coordinate step; #463 L2 fix 1: or a click that follows a located-text mark, whose
     guard reads OCR though the page does not) is not
     simulated: `first_decision{status:"not_evaluated", reason:"lab_ocr_unavailable"}` and the
     warning `first_decision_not_evaluated`; with one it is simulated like any other step;
   - otherwise step 1's primary frame must give `would_click` of `step_01_click` with the
     point inside its rectangle. When that click follows a template (Workflow #396 REC-4),
     every live frame of step 1 (the primary frame and the samples) is simulated, and each must
     give the point inside the box the followed template gives on that frame, as the admitted
     evaluator matches it (the match box, or its corner plus the scaled offset); the decision
     shown is the primary frame's. #463 L2: a click that follows a located-text mark is checked
     the same way, against the box the admitted evaluator finds its names in on each frame; as
     its operation rechecks its guard on one fresh frame, each simulation is given the frame
     twice, the second time as that fresh frame. Workflow #390 LAB-2: a drag must be a swipe
     from inside its rectangle to inside its destination over its duration, a long press a long
     tap at its
     point over its duration. #411 Lab BD: a brake drag must be the kernel's
     `SingleTouchDragWithVerticalBrakeV1` with (x1, y1) inside its rectangle, (x2, y2) inside
     its corner box, x3 = x2 and y3 = y2 − 100, and the four constants (200, 150, 100, 200).
     A step 1 that forbids an OCR mark involves OCR as one that requires it does.
   - #463 R1: a select at step 1 involves OCR (its slots' names). With an OCR model the kernel's
     dry run of the select decides on the primary frame with the policy document as packaged (the
     wanted value's default); every slot must read a known name there. An empty decision is the
     run's normal end: `first_decision{status: "would_end_empty", operation_label, outcome_key,
     frame}` (the offline simulation does not report it: an empty end counts its step,
     `offline_simulation_executed_step_invariant`). A selected slot is then simulated as the run
     taps it (with the evaluation instant `generated_at_unix_ms`, the confirmation on the same
     frame): `would_click` of `step_01_select` with its point inside that slot's click box, the
     decision with its `slot`.

   Anything else is `record_artifact_admission_failed` with the decision.
4. **Cross-check.** Every target is evaluated on its own with the admitted evaluator, OCR
   with the Lab's OCR provider; without an OCR model OCR is not called and counts as
   undetermined. #411 Lab B-9: a value test is OCR: without a model it is undetermined, never
   passed, and with one the admitted evaluator evaluates it as the Runtime does. A check is
   derived from its members. A page matches
   when every required target passes and (Workflow #390 LAB-2) no forbidden target passes, does
   not when a required one fails or a forbidden one passes (`record_step_self_mismatch` names it
   in `forbidden_passed`), and is undetermined otherwise.
   - **Self.** Every page on every live frame of its step (the primary frame and the samples,
     never a superseded frame; a page transition on its own frames) matches or is
     undetermined; a page that does not match one of its own frames is
     `record_step_self_mismatch` (3) naming the frame.
   - **Guards on located text** (#463 L2 fix 2). The followed located-text mark is no mark of a
     page, so the pages above do not evaluate it: for every step whose click follows one, its
     guard (the located-text target, by the names the mark now holds) is evaluated on every live
     frame of the step as a page's targets are. Found on each is a pass. Not found on a frame is
     `record_step_self_mismatch` (3, `details{step, record_index, mark_id, frame_id, failure}`,
     `failure` the kernel's typed reason), except on a step whose names were changed after its
     press: its recorded frames show the demonstration, not the new value, so such a frame is
     listed with the step in `names_changed_after_press[].frames_without_names` and does not fail
     the cross-check. Without an OCR model the guard is undetermined, the status is
     `partially_evaluated` and `guards_not_evaluated` lists those steps. The result lists each such
     step in `located_text_guards[{step, record_index, mark_id, frames[{frame_id, result:
     "found" | "not_found" | "undetermined"}]}]`. The first decision (item 3) evaluates step 1's
     guard before, on the same frames, and refuses a frame on which the names are not found.
   - **Arrival gate.** After the effect of step k the run first waits for its gate: the page
     transition, or page k+1; without a page transition, the gate of a step followed by a run
     of optional steps is the run's skip target N, and an optional step's gate is checked in
     item 7. When the gate matches on a live frame of step k, the run cannot
     confirm that the effect took place: without a window transition this is the warning
     `arrival_unconfirmed` (`step`, `gate`, `frames`, `message`); with one, step k is listed in
     `arrival_by_time_window` (its arrival rests on the window's lower bound, which must be
     long enough). An undetermined gate is the same warning with `reason: "not_evaluated"`
     (or the same listing with a window). Templates match with `ccoeff_normed` by default and
     do not see an overall darkening: when a pop-up darkens the background, mark a darkened
     bright area with `--color` or a color digest, or declare a window. The gate of the
     application entry step is `not_applicable`.
   - **Names changed after the press** (#463 L2 fix 1). No check compares a step's recorded
     press with its generated click: a step 1 that follows a located-text mark is judged by the
     box its names give on each frame (item 3). A step whose names were changed after its press
     ("Following a located text") therefore neither fails for the difference nor passes
     silently: it is listed in `names_changed_after_press[{step, record_index, mark_id,
     recorded_point, frames_without_names?}]`, present only when a step is.
   - The result is `cross_check{status: "passed" | "partially_evaluated", self, gates,
     margins, single_sample_steps, names_changed_after_press?, located_text_guards?,
     guards_not_evaluated?}`, also in the package provenance; `located_text_guards` is present
     only when a step's click follows a located-text mark, `guards_not_evaluated` only when such
     a guard was undetermined.
5. **Entry overlay.** Step 1's page is evaluated on its live frames with every channel times
   0.45, rounded down. When it still matches, the warning `entry_overlay_insensitive` asks for
   a `--color` or color digest mark on a fixed bright area: a darkening pop-up over step 1
   would be taken for step 1 and a prerequisite package would not run. Undetermined gives the
   same warning with `reason: "not_evaluated"`. For an application entry step it is
   `entry_overlay{status:"not_applicable", reason:"application_entry"}`.
6. **Stop.** The page after a `stop` step is the desktop or the launcher: the warning
   `application_stop_target_external`.
7. **Optional steps** (Workflow #339). For every run, with the step k before it and its skip
   target N, on live frames with the evaluator of item 4 (#463 R4: an optional step inserted after
   the stop is a member of its run as a recorded one is, and is refused the same way):
   - a. An optional page must not pass on a screen a run may show when it is absent: the
     frames of step k (of its page transition when k has one; nothing for an application
     entry step), the frames of N, and the frames of another optional step of the run.
     Otherwise `record_optional_step_ambiguous` (3, `details{step, record_index, page, against,
     frames}`, `step` the optional step's number in the package, `record_index` its serial number,
     the one `record mark --step` takes (#463 R4 fix round 2: the two differ once a step is
     inserted after the stop or dropped); `against`: `previous`, `skip_target` or
     `other_optional`): the run would click the pop-up
     on that screen. Give the optional page a mark that does not hold there, such as the
     pop-up's title template or a button.
   - b. N must not pass on a frame of an optional step:
     `record_optional_skip_target_insensitive` (3, `details{step, record_index, optional_step,
     optional_record_index, frames}`, `step` and `optional_step` the package's numbers of N and of
     the optional step, `record_index` and `optional_record_index` their serial numbers, the ones
     `record mark --step` takes).
     Under the pop-up the run would take the screen for N, so a close click that did not
     register, a changed pop-up or a pop-up never recorded would pass silently. Templates do
     not see an overall darkening: add a `--color` or color digest mark on a bright area of N
     that the pop-up darkens (or, for a small pop-up that does not darken, a mark in the area it
     covers); the stored frames suffice.
   - c. One pop-up recorded more than once: an optional page that passes on another optional
     step's frames with exactly the same click rectangle is not refused. `optional_steps[]`
     names the earlier copy in `same_as`, and each such pair adds one `arrival_unconfirmed`
     warning (`same_as`, `gate`, `frames`) on the step whose old screen passes as the other's
     page: after its click the same position may be clicked again. With a window transition
     on that step it is listed in `arrival_by_time_window` instead.
   - d. The arrival gate of step k is N (item 4); its `arrival_unconfirmed` message adds that a
     run without the pop-up fails after the settle when the screen leaves step k's page, and
     that a detour back to the same page after a confirmation needs a page-graph package or two
     packages.
   - e. An undetermined evaluation (OCR) is the warning `optional_ambiguity_not_evaluated`
     (`check` names the refusal it stands for, with that refusal's details) and makes the
     cross-check `partially_evaluated`; it does not refuse.
   - f. Every pair of frames and page is evaluated once, and the gate of an optional step
     without a page transition is the result of b (`distinct` or `not_evaluated`). Any refusal
     comes before every warning.
8. **Select steps** (#463 R1, "Selecting in a list"). On every live frame of every select step the
   kernel's dry run of the admitted package decides, with the default and with the pressed name: the
   step's page must be the one page of the package that passes on the frame (the page rule and the
   layout share the frame; the run refuses a confirmation frame on which another page passes too,
   `selection_page_changed`), every slot's name reads a known name, and on the primary frame the
   pressed name selects a slot, the one the recorded press hit. A frame that does not hold is
   `record_step_self_mismatch` (3, `details{step, record_index, frame_id, reason, …}`, `reason`
   `self_test_error` with the kernel's `failure`, `select_name_unread`, `select_pressed_absent` or
   `select_press_mismatch`, as at mark time). Fix 1: the page of a step the mark already knew is
   refused at mark time (`select_other_page_passes`); another page that passes here (a step
   recorded after the select) is `self_test_error` with the kernel's `select_page_mismatch`. The
   result is the cross-check's `selects` (as `lab.selects`, each frame's decisions by their kind),
   present only with a select step; without an OCR model nothing is called, each frame is
   `undetermined`, the status `partially_evaluated` and `selects_not_evaluated` lists the steps.
9. **Endings** (#463 R2, "Endings"; checked after the cross-check, items 4 to 7, and before item 8,
   so that an ending page that passes on the frames of a select step before the last press is the
   ending's refusal; fix 1: the order the code has).
   On live frames with the evaluator of item 4, in the order the recording took the endings: every
   ending's page matches its own frames (`record_step_self_mismatch`, `details{outcome_key, page,
   frame_id, failed}`), and passes on no frame of a screen the run may show instead of it, since the
   run evaluates the endings before the usual ending (the coordinator's ruling 4):
   `record_ending_ambiguous` (3, `details{outcome_key, page, against, frames, step |
   other_outcome_key}`) with `against`:
   - `previous`: the frames of a step after whose press the run awaits the endings: the step before
     the last, and, when it is an optional step, the step before its run and the run's members (a
     step's page transition's frames when it has one, the screen left after its press). The run
     would end on the ending before the result of that press showed;
   - `usual`: the frames of the last step, the usual ending: the ending would win wherever the
     usual ending shows;
   - `other_ending`: the frames of another ending: the two could not be told apart.

   Give up such an ending (`--remove-ending`) and take it again with a mark of its own screen, such
   as a template of its message. The usual ending's page passing on an ending's frames is no
   refusal (the ending wins there, as it should), but the warning `ending_usual_insensitive`
   (`outcome_key`, `page`, `usual_page`, `frames`). Fix 1: an ending page that rests on OCR marks
   alone (every mark it requires reads OCR: an OCR mark or a value test, or a check whose members
   all do) cannot be checked against those screens without an OCR model, and an ending that passed
   there would turn a failure into a silent success: `record_ending_invalid` with
   `ocr_alone_unchecked` at the first screen it could not be checked on (`details{page, against,
   frames, step | other_outcome_key}`), nothing written; run `record stop` where the Lab has its OCR
   model, or take the ending again with a mark of another family. Any other undetermined
   evaluation (an ending with a mark of another family beside its OCR marks, or the usual page's
   OCR marks on an ending's frames) is the warning `ending_ambiguity_not_evaluated` (`check`:
   `record_ending_ambiguous` or `ending_usual_insensitive`, with that check's details) and makes
   the cross-check `partially_evaluated`. Every refusal comes before every warning. The result is the
   cross-check's `endings[{ending, outcome_key, page, frames[{frame_id, result}], usual}]` (`usual`:
   `distinct`, `passes` or `not_evaluated`), as `lab.endings`, present only with an ending.

Warnings never refuse. The provenance and the output carry the same warnings.

### Writing

Everything above is computed in memory first. Then (Workflow #336 PKG-S5, M-PKG v1.1 §3.5 and
D9: an output of the same name is replaced in place, never refused):

1. **`--lab-dir`** (never guessed) is checked before anything is written: a missing directory
   needs an existing parent and only it is created (`lab_dir_created: true`), otherwise
   `record_lab_dir_invalid` (2); an existing `<lab-dir>/<package_id>.<ext>` must be a file (a
   directory or a link there is `record_lab_dir_invalid`, reason `target_not_a_file`), and is
   replaced. #464 P4 (since 0.12.63; the coordinator's ruling 5 on board #464): a directory that is,
   or lies inside, an installed root (a directory holding the file `install\state.json`, the
   installer's state, [install-cli.md](install-cli.md), "Layout"; a folder named `install` alone,
   such as a folder of installers, is none; looked for on the directory made absolute and on its
   nearest existing part with its links resolved) is refused
   `record_lab_dir_invalid` with the reason `inside_installed_root` and `details.installed_root`,
   also under `--dry-run` and when an `already_generated` package is copied again: a recording's
   output never lands where an installation reads its packages, and a file there is no member of a
   standard pack anyway. A package reaches the schedule through `standard-pack adopt`
   (`adoption_example`, "Output").
2. The copy inside the recording, `lab/out/<package_id>.<ext>`, is written, replacing a file of
   that name.
3. `<lab-dir>/<package_id>.<ext>` is written the same way (`lab_dir_status: "written"`, or
   `"replaced"` when a file of that name was there).
4. `recording.json` becomes `stopped` with its `artifact` (`container`, `path`,
   `lab_dir_path`, `byte_count`, `package_id`, `requires`,
   `game`, `server`, `locale`, `timeout_ms`, `arrival_timeout_ms`,
   `application_arrival_timeout_ms` (defaults applied), `generated_at_unix_ms`); then the
   session file is stopped.

Every replacement (steps 2 and 3) writes `<name>.part` beside the target, sets an existing
target aside as `<name>.old-<unix_ms>` and renames the `.part` file to the target. #464 P4 (since
0.12.63): the set-aside file is kept, the output an earlier stop wrote: it is never deleted, and
never overwritten (a name a file already holds is passed over for the next millisecond); the answer
names it in `lab.kept`. The target's name only ever changes by rename, so a reader opens the old
file, the new one or none (between the two renames), never part of one. A failed step is
`record_state_io_failed` (5) and nothing is skipped: the `.part` file stays and is named; when
the new file cannot be renamed into place the old one is renamed back (`details.restored`).

A failure in steps 2–4 leaves both states active and names the files already written
(`details.written_files`, `.part` files included); nothing else is deleted. A stopped recording
given `--lab-dir` again reads its copy and repeats steps 1 and 3. It is not generated
again, so a generation option that differs from what its package was generated with is refused
before anything is written: a different `--requires` is `record_requires_conflict` (3); a
different `--package-id`, `--game`, `--server`, `--locale`, `--timeout-ms`,
`--arrival-timeout-ms` or `--application-arrival-timeout-ms` is `record_stop_option_conflict`
(3, `details{field, recorded, given}`; a value the artifact did not record conflicts too). An
equal value is accepted.

### Output

`{status: "stopped" | "validated", dry_run, record, path, lab}` where `lab` is `null` or:

```
{status, dry_run, session_already_stopped?, container, package_id, task_id,
 path, lab_dir_path, lab_dir_status, lab_dir_created, written_files | would_write, kept?,
 byte_count, entries, steps, click_steps, application_steps[{step, op, action, source}],
 pages, transitions, optional_steps[{step, op, settle_ms, skip_to, same_as}], selects?, endings?,
 timeouts{timeout_ms, step_timeout_ms, arrival_timeout_ms,
 application_arrival_timeout_ms}, warnings, arrival_by_time_window, marks_not_evaluated,
 steps_needing_review, cross_check, first_decision, entry_overlay,
 requires, prerequisite_entry_example, binding_example, adoption_example, task_run_example,
 catalog_on_failure_example, binding_requires}
```

- #464 P4 (since 0.12.63): `kept` lists the files a replacement set aside and kept ("Writing"),
  present only when one was. `adoption_example` is how the package becomes a member of its game's
  standard pack ([standard-pack-authoring.md](standard-pack-authoring.md)): `{command, decision}`,
  `command` the line `actinglab standard-pack adopt --repo <checkout> --record-id <record_id>
  --state-dir "<state>" --decide <decision.json>` with the recording's own id and state directory
  (the checkout and the decision file are placeholders), `decision` the skeleton of the decision it
  takes. Fix round 2 (the owner's rule: a package that has not run through on a real device is
  registered and stays held): `{"schema_version": "actingcommand.pack-decision.v1", "class":
  "held", "held": {"reason": ""}}`, a decision of the class `held` with its reason left blank and
  nothing else (no trigger, no value). Fill in the reason, why the package is held (for example
  that it has not run on a real device yet); adopt refuses a blank reason (`/held/reason`), and
  accepts the decision once it is filled. `already_generated` answers it too. `binding_example`
  stays for `record draft`, which reads the package path from it.

- `status` is `generated`, `validated` (`--dry-run`), `already_generated` or (Workflow #390
  LAB-2) `regenerated`, the package of a stopped recording marked again.
- The package is an ordinary content-directory package and loads as
  `--package <package_id>.<ext>` (`task_run_example`); no answer member names a digest or a
  package reference (Workflow #336 PKG-S5).
- `binding_example` is a complete `policy.procedure_manifest[]` entry: `procedure_ref` (the
  package id), `operation_id: "operation.contained_task"`, `yield_points: []` and
  `scheduled_execution{mode: "device_registry", package_path}` with the absolute path of the
  package `record stop` wrote. It stays for `record draft`, the reader that takes the package path
  from it. #464 P4, fix round 2: the package is not placed under an installed root's packages (a
  `--lab-dir` there is refused, "Writing"); it goes into its game's resource repository through
  adoption (`adoption_example`). Whether it is scheduled still depends on the catalog task and its
  approval.
- `prerequisite_entry_example` says where the package goes so that another package can name
  it with `--requires`: the standard-pack registry of its game (Workflow #373 NAV-3a,
  `package-reference.md`, "Standard package root"), the one place a prerequisite package is
  found. It is `{bundle, pack, directory}`: `bundle` is
  `<standard_packs.root>/<game>/bundle.json`, the file that must list it; `pack` is its
  `packs[]` entry `{package_id, server, entry_task_id, path: "packs/<package_id>"}`, which
  `package bundle` writes with `file_count` and `byte_count` added; `directory` is
  `<standard_packs.root>/<game>/<package_id>`, where the package sits as a content directory.
  `<game>`, the server and the entry task are the package's own; `<standard_packs.root>` is a
  placeholder for the actingd setting (`actingd-check-config.md`, "Standard packs"). The
  retired actingd key `prerequisite_packages` is no such place: a configuration that carries it
  is `config_decode_failed`.
- `catalog_on_failure_example` is `{"action":"pause","retry_limit":1,
  "retry_backoff_ms":0,"escalation_threshold":2}`, the `on_failure` that
  `policy-suspension.md` ("Catalog") recommends; the `binding_requires` item about it says that
  the rerun happens immediately (R22). How failures are rerun, pause the task and are lifted is
  described there.
- `optional_steps` lists every optional step: its package step number, operation id,
  `settle_ms`, `skip_to` (the page of N) and `same_as` (the earlier step recorded for the same
  pop-up, or `null`); `[]` without optional steps.
- #463 R1: `selects` lists every select step with its decisions on every live frame (self-check
  item 8), present only when the package has one; `click_steps` counts a select step, whose effect
  is its tap; `marks_not_evaluated` lists a select whose mark-time self-test was not evaluated as
  `select/<page>`.
- #463 R2: `endings` lists every ending with item 9's result, present only when the recording has
  one; `pages` lists the endings' pages after the steps' and the transitions'; `binding_requires`
  then adds that a run ending on one is a success with its key, its message left on the screen, and
  that such a package is no prerequisite or return-home package and needs a Runtime that admits
  ending pages.
- `binding_requires` lists what a binding also needs: the catalog task and its approval; the
  prerequisite package with `--requires` (listed by its game's installed standard pack,
  Workflow #373 NAV-3a); without it, the return-home fallback (step
  1 should be the return-home package's final screen); the recommended `on_failure`; with
  application steps, that only an instance with an assigned application runs them and that an
  application entry package has no entry recognition (best configured as the instance's
  `startup_package`, unpacked as a directory); and, with optional steps, that they cover only
  screens appearing after the click of the step before them (a pop-up later than its
  `settle_ms`, shown more often than the copies recorded, or never recorded makes the package
  fail loudly). Workflow #463 U1 (since 0.12.60): a screen the recording never met after a
  step's click, which stays still and shows no page of the package, no loading frame and no
  shared interrupt page for the whole closing decision that follows the failed wait (with or
  without `--click-retry`; it never clicks; [linear-steps.md](linear-steps.md), "Retries"), ends
  a run of the package (its own run, outside a restart segment; never a startup package's or a
  ladder rung's) with `contained_task_linear_unrecorded_screen`, whose `artifact_id` is the frame
  of that screen; a slow screen nobody recorded that lasts that long is taken for one too, and is
  declared in the package (a page or a loading page) to tell it apart; a
  scheduled run then pauses its task at once, until the package changes
  ([linear-steps.md](linear-steps.md), "Failure codes";
  [policy-suspension.md](policy-suspension.md), "A failure that pauses at its first occurrence").
  Record the missing ending from that frame: an optional step, or a step and a page; #463 R2: a
  screen that ends the business after the last press is taken as an ending ("Endings"); #463 R4: a
  pop-up after which the path goes on is inserted after the stop as an optional step, from that
  frame ("Inserting an optional step after stop").

## Error codes

| exit | codes |
|---|---|
| 2 | `validation_failed` (Workflow #449 REC-ARCH fix 1: also `record archive --copy-to` empty, blank or relative), `record_flag_unsupported`, `record_flag_takes_no_value`, `record_state_dir_unsupported`, `record_frame_unreadable`, `record_transition_window_invalid`, `record_transition_has_click`, `record_application_action_invalid`, `record_application_with_click`, `record_optional_settle_invalid`, `record_locale_missing`, `record_lab_dir_invalid`, `record_requires_invalid` |
| 3 | `record_session_not_active` (Workflow #390 LAB-2: also, with `details.reason: "recording_stopped"`, a change a stopped recording does not take), `record_lab_unavailable`, `record_instance_mismatch`, `record_step_not_found`, `record_step_not_last`, `record_step_frame_missing`, `record_step_frame_conflict`, `record_frame_size_mismatch`, `record_frame_hash_mismatch`, `record_mark_rejected`, `record_mark_id_conflict`, `record_mark_id_reserved`, `record_mark_in_use`, `record_asset_name_conflict`, `record_click_exists`, `record_click_executed`, `record_click_source_invalid`, `record_click_follow_invalid`, `record_click_rect_conflict`, `record_click_outside_step_rect`, `record_step_click_missing`, `record_step_click_not_executed`, `record_gesture_execution_unsupported` (#411 Lab BD), `record_guard_family_invalid`, `record_append_failed_after_input`, `record_transition_without_click`, `record_transition_exists`, `record_to_transition_invalid`, `record_busy`, `record_application_entry_invalid`, `record_step_effect_exists`, `record_optional_first_step`, `record_optional_application`, `record_select_invalid` (#463 R1), `record_ending_invalid` (#463 R2), `record_optional_after_invalid` (#463 R4); stop: `record_no_steps`, `record_application_entry_not_first`, `record_step_without_recognition`, `record_final_step_has_click`, `record_too_many_steps`, `record_step_after_stop_invalid`, `record_application_without_home`, `record_click_follow_invalid`, `record_optional_first_step`, `record_optional_final_step`, `record_optional_application`, `record_optional_restart_segment_end`, `record_step_self_mismatch`, `record_optional_step_ambiguous`, `record_optional_skip_target_insensitive`, `record_select_invalid` (#463 R1), `record_ending_invalid`, `record_ending_ambiguous` (#463 R2), `record_artifact_admission_failed`, `record_requires_conflict`, `record_stop_option_conflict`, and (Workflow #449 REC-ARCH) `record_session_not_active` with `details.reason: "recording_archived"`; archive and reopen (Workflow #449 REC-ARCH): `record_session_not_active`, `record_session_active` (reopen), `record_not_archived`, `record_archive_target_not_empty`, `record_instance_mismatch`, `record_lab_unavailable` |
| 4 | `record_click_indeterminate`, `record_click_performed_with_failure`, `record_application_indeterminate` |
| 5 | `record_lock_failed`, `record_state_io_failed` (Lab state files cannot be read or written; Workflow #449 REC-ARCH: also a copy file that cannot be written), the state directory cannot be created, `record_archive_verify_failed` (Workflow #449 REC-ARCH) |
| 6 | `record_stop_generation_not_implemented` (a build without the package generator; the #336 L4 build also for a recording with optional steps, `details.reason: "optional_steps"`) |
