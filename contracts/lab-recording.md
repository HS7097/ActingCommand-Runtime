# Lab recording (`record start` / `mark` / `status` / `stop`, `--record`)

A Lab recording turns a sequence of real screens into a linear-steps task package: each
step is one screen with its recognition marks and at most one effect, a click rectangle or
an application operation (R24), and the last step only recognizes. This contract
(Workflow #336) covers the recording state, the commands that build it, the five mark
families with their mark-time self-test, steps and transitions, remediation, application
steps, optional steps, `--record` on the device commands and the recording lock, and then
the package `record stop` generates, checks and writes ("Package generation").

The recording state lives in the Lab state directory. Nothing in this document writes to
the Runtime ledger; the clicks and application operations themselves are executed by the
Runtime and recorded there as any `do --capture` or `session app` is.

## Files

```
<state>/record-<instance>.json                               the `record` command file (unchanged)
<state>/record-<instance>.lock                               recording lock (operating-system lock, empty file)
<state>/record-<instance>.lock.json                          {pid, command, acquired_at_unix_ms} of the holder
<state>/record-artifacts/<record_id>/lab/recording.json      actingcommand.lab-recording.v1
<state>/record-artifacts/<record_id>/lab/frames/<sha256>.png frames, content-addressed
<state>/record-artifacts/<record_id>/lab/crops/<sha256>.png  template crops, content-addressed
<state>/record-artifacts/<record_id>/lab/out/<D>.<zip|json>  the package record stop generated
```

`<instance>` and `<record_id>` use the file-stem rule of the `record` command: ASCII
letters, digits, `-`, `_` and `.` are kept, every other character becomes `_`.

The `record` command file is never written by the Lab recording; the old actions (`step`,
`candidates`, `amend`, `build-task`, `promote`) keep their behaviour. The Lab recording
reads its `record_id`, `task_id`, `instance`, `status` and `started_at_unix_ms`, and
`recording.json` stores the first four (`record_started_at_unix_ms` for the last). Every
command compares them; a difference makes the Lab part unavailable
(`record_lab_unavailable`, reason `lab_recording_mismatch`).

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
   "closed":false,"closed_by":null}],
 "artifact":null}
```

- `index` is a serial number inside the recording, starting at 1 and never reused. Dropped
  steps and steps converted into a transition stay in the file. Every option that names a
  step (`--step`, `--drop-step`, …) uses the serial number. The package numbers the
  effective steps (neither dropped nor converted) again as 1..n; `record status` shows that
  number as `artifact_step`.
- Frame roles: `primary`, `sample`, `transition`, `transition_sample`. Frame sources:
  `local_png`, `lab_capture` (`capture --record`), `runtime_observation`
  (`observe --capture --record`, with the Runtime frame reference in `runtime_artifact`).
- All frames of a recording have one size; it becomes the package coordinate space and
  resolution (`record_frame_size_mismatch`).
- Fields added later use `serde(default, skip_serializing_if)`: `application` (see
  "Application steps") and `optional` (see "Optional steps") are absent on steps without
  one.

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

### record mark

```
record mark [--step <n>] [--frame <png>] [--sample <png>]… [--page <name>]
            [--template <id>=x,y,w,h]… [--color <id>=x,y,w,h]… [--reuse <id>]…
            [--click x,y,w,h | --click-from <id>] [--click-guard <id>] [--click-retry <n>]
            [--replace-click] [--remove <id>]… [--optional [--settle-ms <0..60000>] | --not-optional]
            [--dry-run] [--state-dir <dir>]
record mark [--step <n>] [--frame <png>] [--template …]… [--color …]… [--reuse <id>]…
            --application <launch|restart|stop|force-stop> [--replace-click]
record mark --step <k> --transition none|page|window [--frame <png>] [--sample <png>]…
            [--template …]… [--color …]… [--reuse <id>]… [--transition-timeout-ms <ms>]
            [--min-ms <a> --max-ms <b>] [--replace-transition]
record mark --drop-step <n> | --reopen-step <n> | --close-step | --to-transition <n>
record mark --request <file> | --request-json <json>
```

The request form (`actingcommand.lab-record-mark.v1`) carries every capability, including
the color digest, OCR and check families:

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

Rules:

- `--request` cannot be combined with the shortcut flags; single-valued flags given twice,
  unknown flags and positional arguments are refused (`validation_failed`, exit 2).
- Order: locate the step, the frame, remove, add, reuse, samples, click, self-test. All
  marks pass or nothing is written (`record_mark_rejected`, exit 3, `details.marks[]` with
  one `{id, reason, …}` per refused mark). `--dry-run` runs everything and writes nothing
  (`marks_validated`).
- Mark ids: 1–128 bytes without surrounding whitespace or control characters, not starting
  with `page/` (`record_mark_id_reserved`), unique among the marks of the effective steps
  and their transitions (`record_mark_id_conflict`). A removed or dropped mark frees its id.
- Template crops become `assets/<id lowercased, characters outside [a-z0-9_.-] replaced by
  _>.png`; two ids with one asset name are refused (`record_asset_name_conflict`).
- `--reuse <id>` adds a mark of another effective step to this step and tests it on this
  step's frames. The step's marks are its own marks plus the reused ones.
- `--sample <png>` adds a frame of the same screen; every mark of the step is tested on it.
- `--remove <id>` removes the entry only; a mark reused elsewhere, named by a check or used
  as the click guard is refused (`record_mark_in_use`).
- `--page <name>` (`^[a-z0-9][a-z0-9_]{0,47}$`) names the step's page.
- Click: exactly one of a rectangle (`--click`, inside the frame with width and height ≥ 1)
  and a mark of the step (`--click-from`, copying the mark's region; for a template that is
  the crop, not the search area; a check has no region: `record_click_source_invalid`).
  A second click needs `--replace-click` (`record_click_exists`); an executed click
  cannot be replaced before `--reopen-step` (`record_click_executed`). `--click-guard`
  names a template, color, color digest or check mark of the step
  (`record_guard_family_invalid`). `--click-retry <n>` declares `{max_attempts n (2..5),
  interval_ms 1000}`; the request form takes any `interval_ms` in 1..5000.

Output: `{status, record_id, step, step_opened, frame, samples, marks, reused, removed,
click, application, transition, step_state{marks, frames, effect, transition, closed,
optional}, closed_step, dry_run}`. `step_state.effect` is `none`, `click_declared`,
`click_executed`, `click_indeterminate`, `application_declared` or `application_executed`;
`step_state.optional` is `null` or `{settle_ms}`.

### record status

The existing output, plus `lab`: `null` without a Lab recording, `{status:"unavailable",
reason}`, or

```
{record_id, status, coordinate_space, defaults, open_step,
 steps:[{index, artifact_step|null, page, dropped, converted_to_transition,
         frames[{frame_id, role, sha256, w, h, superseded}],
         marks[{id, family, self_test{status}, margin}], reused,
         entry: "any" | "page",
         click{rect, source, executed, outcome, attempts, needs_review}|null,
         application{action, cli_verb, source, executed, attempts[], needs_review}|null,
         optional: null | {settle_ms},
         transition: null | {kind:"page", frames, marks, timeout_ms, source} | {kind:"window", min_ms, max_ms},
         closed, closed_by}],
 artifact|null, record_flag_state_dir, record_flag_reachable}
```

`click.outcome` is `declared` (no outcome yet), `executed` (Performed) or `indeterminate`
(the outcome of the last `do --capture --record` is unknown); `attempts` counts the earlier
outcomes moved aside by `--reopen-step`.

### record stop

Without Lab steps `record stop` behaves as before and adds `lab: null`; an empty active Lab
recording is stopped with the session. With Lab steps it generates, checks and writes the
package: see "Package generation". A build without the generator (#336 L3, L3b, L3c) refuses
with `record_stop_generation_not_implemented` (exit 6) and leaves both states active; the #336
L4 build refuses a recording with an optional step the same way (`details.reason:
"optional_steps"`).

## Marks and the mark-time self-test

Every mark is tested when it is recorded, on every live frame of its step (primary and
samples, not superseded frames). The result is `passed`, `failed` or `not_evaluated`; any
`failed` refuses the batch. A step with one frame shows `single_sample: true`.

| family | fields | self-test |
|---|---|---|
| template | `region` (crop), `search?` (default `region`; must contain it and lie in the frame), `threshold?` | the crop from the primary frame is matched in `search` with the recording metric; every frame needs score ≥ threshold; `min_score`, `margin` = min score − threshold, `matched_rect`, `location_ambiguous` (a match elsewhere than the crop still passes) |
| color | `region`, `max_distance?` | `expected` is the mean color of `region` on the primary frame; every frame's mean color must be within the distance; `margin` = limit − largest distance |
| color_digest | `region`, `columns`, `rows`, `max_mean_milli`, `max_cell?`, `exclude_cells?` (no defaults) | `cells` from the primary frame (`color_digest.v1`); every frame must pass; `margin` = `{mean_milli, max_cell}` limits minus the worst values |
| ocr | `region`, `languages`, `timeout_ms`, `match_mode` (`exact`/`contains`), `expected`, `case_sensitive`, `minimum_confidence`, `model_ref`, `model_sha256`, all required | always `not_evaluated` (`lab_ocr_provider_unverified`): Lab has no OCR provider, and not evaluated is never reported as passed |
| check | `all_of` or `any_of`: 2..8 distinct marks of the step, no checks | derived: all_of fails on any failed member and passes when all pass; any_of passes on any passed member and fails when all fail; otherwise not evaluated |

Shape errors (missing or misplaced fields, an invalid grid) are `validation_failed` (exit
2); a region outside the frame (`region_outside_frame`), an invalid search area
(`search_invalid`), a check member that is not a mark of the step (`check_member_invalid`)
and failed self-tests are reasons inside `record_mark_rejected`.

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

Only `capture`, `observe --capture`, `do --capture` with a point or rectangle click and
`session app` / `session instance app` with `launch`, `restart`, `stop` or `force-stop`
(see "Application steps") accept `--record`; every other command or combination is refused
before it runs:

| code (exit 2) | when |
|---|---|
| `record_flag_unsupported` | offline commands, `capture diagnose`, element or swipe clicks, `observe --capture --dry-run`, `do --dry-run`, `session app --dry-run` (global or after the command, including the instance alias), `session app` without one of its four actions, `tap`/`swipe`/`long-tap` and every other command |
| `dry_run_unsupported` | `capture --record --dry-run` and its `session capture` alias, global or after the command |
| `record_flag_takes_no_value` | `--record <value>` |
| `record_state_dir_unsupported` | `--record` with `--state-dir`; `--record` uses `ACTINGLAB_SESSION_STATE_DIR` or the default state root |
| `validation_failed` | `--tap-rect` without `--record` |

The record instance (`--instance` or the instance configuration) and the command instance
must be the same (`record_instance_mismatch`, exit 3); a missing or inactive session is
`record_session_not_active`. Every check runs before anything is captured or pressed.
All six device recording forms refuse `--dry-run` before recording locks, Runtime access,
frame attachment or recording persistence. The global parser handles the flag at any position.

- `capture --record` takes the frame through the Runtime as `capture` does; `--out` becomes
  optional. Output adds `record{status:"frame_recorded", record_id, step, step_opened,
  frame, closed_step}`.
- `observe --capture --record` stores the verified Runtime frame and its reference.
- `do --capture --record` presses inside the open step's click rectangle: its center, or
  `--tap x,y` when it lies inside (`record_click_outside_step_rect`). `--tap-rect x,y,w,h`
  declares the rectangle when the step has none, and must equal the declared one otherwise
  (`record_click_rect_conflict`). Without a rectangle the command refuses
  (`record_step_click_missing`). The click record stores the point, its rule
  (`rect_center`/`explicit`), the carrier package reference, the request, correlation,
  action and lease ids, any failure and the before/after frame summaries marked
  `informational`.
  - Performed: recorded; the step is closed.
  - Performed with a failure: recorded, the step closed and marked `needs_review`; exit 4
    `record_click_performed_with_failure`.
  - Not performed: nothing is recorded; the operation's own error is returned.
  - Indeterminate: recorded as the click outcome; the step stays open; exit 4
    `record_click_indeterminate`. Resolve with `--reopen-step` or `--close-step`.
  - A click that was sent but could not be recorded is
    `record_append_failed_after_input` (exit 3) with the operation summary.
- observe and do need a carrier package: any admissible package of the same game and
  resolution, including an earlier Lab package.
- Pause the instance's scheduling for the recording (`actingctl pause --instance <alias>`,
  then `resume`): a scheduled task between two commands would act on another screen.

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
- No frame is stored before or after the operation: `session app` returns none, the entry
  step has no screen before it, the screen right after the operation is usually a launcher
  or splash screen, and the arrival screen comes from the next `capture --record`.

### Where the operation lands

| recording | `session app … --record` (device) | `record mark --application` (offline, without `--step` and `--frame`) |
|---|---|---|
| no effective step | Performed: a new application entry step, executed and closed (`closed_by:"application"`). Indeterminate: a new entry step, declared with the attempt, open | a new entry step, declared, open |
| the last effective step is closed and no arrival screen came yet | `record_application_entry_invalid` (3), nothing sent | same |
| open step, a frame without marks | `record_application_step_marks_missing` (3), nothing sent | same |
| open step, marks, no effect | Performed: the step's effect, executed, the step closed. Indeterminate: declared with the attempt, open | the step's effect, declared, open |
| open step with a click | `record_step_effect_exists` (3), nothing sent | replaced with `--replace-click`, otherwise `record_step_effect_exists`; a click with an outcome is `record_click_executed` |
| open step with a declared application operation (also one left by an indeterminate result) | the same action is sent: Performed records the execution and closes the step, Indeterminate adds an attempt. Another action: `record_step_effect_exists` (3), nothing sent | replaced with `--replace-click` (its attempts are kept), otherwise `record_step_effect_exists` |

- With `--frame` (a new offline frame) or `--step k` the operation becomes the effect of
  that step, from its page. An open step needs marks first
  (`record_application_step_marks_missing`); a declared effect is replaced only with
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
- Typical use (restart, then the main interface): pause the instance's scheduling;
  `session app restart --record` (the entry step, serial 1); `capture --record` of the title
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
- Order: the step is planned first (the refusals above send nothing); the request is the
  `session app` request (`RuntimeClient::control_application`); then the Runtime result
  decides:
  - a completed receipt: recorded, the step closed; the output is the `session app` output
    plus `record{status:"application_recorded", record_id, step, step_opened, step_closed,
    application}`;
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
  run; every other error means it did not run and the recording is unchanged. Several of
  these errors exit with 4 (also the Runtime refusals mapped to `device_error`): tell them
  apart by the error code.
- The v0.9.0 ActingLab ignores `--record` on `session app` and runs the operation without
  recording it: do not use it during a recording. A build without application steps refuses
  the flag with `record_flag_unsupported` before any request.

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

Steps are only appended with new serial numbers, and a stopped recording cannot be marked:
a pop-up found missing after the recording means recording again, which can be assembled
offline from frames kept with the earlier recording.

## Recording lock

Every state-writing command (`record start`, `mark`, `stop` including `--dry-run`, the old
`step`, `amend`, `build-task`, `promote`/`publish`, and `capture`/`observe`/`do --record`)
takes `<state>/record-<instance>.lock` (`session app … --record` too, from before its
Runtime request until the result is recorded) with an exclusive operating-system lock without
waiting, holds it until its output is printed (`do --record` holds it across the device
click) and writes the holder to `.lock.json`. `record status` and `record candidates` do
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
| the session file was already stopped (for example by v0.9.0) but the Lab recording is active | generated as above; `lab.session_already_stopped: true` |

**`--dry-run`** (also the global `--dry-run`) runs everything: the pre-checks, generation,
admission, the first decision, the cross-check and the `--lab-dir` checks. It writes nothing,
both states stay active, and the output has `status: "validated"`, `lab.status: "validated"`,
`lab.would_write[]`, `lab.lab_dir_status: "to_write" | "present"` and `lab.lab_dir_to_create`.
Its `lab.warnings` are those the final stop prints: run it first, mark what it asks for, then
run `record stop`. The final stop closes the recording; afterwards `record mark` only answers
`record_session_not_active` and a missing mark means recording again.

A refusal before the writing stage leaves the recording and the session active and writes
nothing; a failure during writing is described under "Writing" (with `written_files`). Fix the
recording and run `record stop` again.

### Pre-checks

Only the effective steps count, numbered 1..n by serial number (`artifact_step`).

| check | code (exit) |
|---|---|
| at least two steps | `record_no_steps` (3) |
| an application entry step is step 1 only | `record_application_entry_not_first` (3) |
| steps 1..n−1 each have one effect (a click or an application operation) | `record_step_click_missing` (3) |
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
  members of its `any_of` checks. No `forbidden`, no anchors.
- Every own mark of an effective step and of its page transition is one target:
  `verify_templates[]` (`template: "assets/<name>.png"`, `region` the search area,
  `threshold` when declared), `color_probes[]` (`expected` and `max_distance` when declared, or
  the `color_digest.v1` digest), `ocr_targets[]`, `checks[]`. Templates and color probes carry
  a `provenance` object.
- `control.json`: `Lab-1y.control.v2`, `execution_mode: "linear_steps"`, the package id, game,
  server, resolution (the recording's frame size), `entry_task_id` (the task id),
  `prerequisite_package_id` with `--requires`, `timeout_ms`, `step_timeout_ms` and `max_steps`
  (the number of steps with an effect). `resources/operations/resources.json` is
  `{"schema_version":"1.0","resources":[],"resource_count":0}`.
- `task.json` (schema `0.9`): `server_scope: [server]`, the locale, the recording defaults,
  `timeout_ms` and `max_steps` equal to the control's, `target_page` the last page, and one
  `scheduling_outcome` mapping `{"outcome_key": "<task_id>_done", "effect":
  "no_designated_effect", "terminal_pages": [<last page>]}`.
- A click step's operation is `step_ii_click`: `from` its page, `to` and `expect_after.page_id`
  the next page, `click: {"kind": "rect", …}` with the recorded rectangle,
  `expect_after.timeout_ms` the arrival timeout T (`--arrival-timeout-ms`, default 15000),
  `interval_ms` 500, `post_delay_ms` 200, and `retryable`, `max_attempts`,
  `retry_interval_ms` only when the step declares a retry. Its guard target comes from the
  page's required marks only: the click guard, the click source unless it is OCR, the first
  template, the first color or color digest, the first check. The guard's `expected_rect` is
  the template's search area, the color or digest region, or the bounding box of the check's
  members. A page with only OCR marks gives no guard and `unguarded_trusted_coordinate: true`.
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
  `S·[step 1 is no application entry] + Σ a·(w + (b − w)⁺ + p + T) + Σ_app 10000 +
  Σ (a − 1)·(r + S) + Σ_runs max(settle) + 10000` over the steps with an effect, where S is the
  step timeout, a the attempts, w max(post delay, window minimum), b the window maximum (0
  without), p the page transition timeout (T without its own, 0 without a page transition), T
  the arrival timeout (A for an application step), r the retry interval, and max(settle) the
  largest `settle_ms` of a run of optional steps: the sums before it count every optional step
  as shown, the longest path, and a run settles at most once, only when it is skipped. Above
  1800000 it is clamped and the warning `task_timeout_clamped` added. The default covers the
  usual path only; give `--timeout-ms` for more.
- `provenance` (top level and per operation) records the recording, the generator, every step
  (record index, page, frame and sample sha256, frame size, marks with their mark-time result
  and margin, click, application, transition), the warnings, `arrival_by_time_window` and the
  cross-check. `generated_at_unix_ms` is the recording's `updated_at_unix_ms`, so the same
  recording generates the same bytes again.

### Container and digest

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
container lists the files in path order. D is the `content-directory.v1` digest of these
files (`package-reference.md`); the same content has the same D in either container and
unpacked as a directory.

### Self-checks

Preparation uses `PreparedContainedTask` and the same linear candidate-set and target-consensus
budget rules as Runtime. First-decision simulation uses the same kernel observation and guard
owners. The recorder emits no `target_consensus` declaration: its per-frame cross-checks and
first-decision result describe the generated package only, and do not certify multi-frame
consensus, live OCR/provider execution or prerequisite recovery. Author-supplied declarations
are checked by `package preflight` through that same preparation owner; actual sample and
guard execution still requires the corresponding frames, provider and Runtime evidence.

1. **Container round trip.** The encoded bytes expand (`expand_content_container`) to exactly
   the generated files, whose digest is D.
2. **Admission.** The kernel admits the files as actingd loads a content directory: the digest
   comparison, source compilation, declarations and the `linear_steps` rules, without a vision
   provider. A refusal is `record_artifact_admission_failed` (3) with
   `details{stage, loader_code, detail, declaration{file, pointer, reason}}`.
3. **First decision.** The offline simulation of the package on recorded frames:
   - a package with an application step, on its first live frame, must be refused with
     `application_effect_requires_assigned_application` before any capture:
     `first_decision{status:"not_evaluated", reason:"lab_application_effect_offline",
     refusal}`; when step 1 is a click, the warning `first_decision_not_evaluated` says its
     click was not simulated;
   - a step 1 that involves OCR (an OCR mark or a check with an OCR member among its required
     marks, which includes a trusted-coordinate step) is not simulated:
     `first_decision{status:"not_evaluated", reason:"lab_ocr_provider_unverified"}` and the
     warning `first_decision_not_evaluated`;
   - otherwise step 1's primary frame must give `would_click` of `step_01_click` with the
     point inside its rectangle.

   Anything else is `record_artifact_admission_failed` with the decision.
4. **Cross-check.** Every target is evaluated on its own with the admitted evaluator; OCR is
   not called and counts as undetermined, a check is derived from its members. A page matches
   when every required target passes, does not when one fails, and is undetermined otherwise.
   - **Self.** Every page on every live frame of its step (the primary frame and the samples,
     never a superseded frame; a page transition on its own frames) matches or is
     undetermined; a page that does not match one of its own frames is
     `record_step_self_mismatch` (3) naming the frame.
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
   - The result is `cross_check{status: "passed" | "partially_evaluated", self, gates,
     margins, single_sample_steps}`, also in the package provenance.
5. **Entry overlay.** Step 1's page is evaluated on its live frames with every channel times
   0.45, rounded down. When it still matches, the warning `entry_overlay_insensitive` asks for
   a `--color` or color digest mark on a fixed bright area: a darkening pop-up over step 1
   would be taken for step 1 and a prerequisite package would not run. Undetermined gives the
   same warning with `reason: "not_evaluated"`. For an application entry step it is
   `entry_overlay{status:"not_applicable", reason:"application_entry"}`.
6. **Stop.** The page after a `stop` step is the desktop or the launcher: the warning
   `application_stop_target_external`.
7. **Optional steps** (Workflow #339). For every run, with the step k before it and its skip
   target N, on live frames with the evaluator of item 4:
   - a. An optional page must not pass on a screen a run may show when it is absent: the
     frames of step k (of its page transition when k has one; nothing for an application
     entry step), the frames of N, and the frames of another optional step of the run.
     Otherwise `record_optional_step_ambiguous` (3, `details{step, page, against, frames}`,
     `against`: `previous`, `skip_target` or `other_optional`): the run would click the pop-up
     on that screen. Give the optional page a mark that does not hold there, such as the
     pop-up's title template or a button.
   - b. N must not pass on a frame of an optional step:
     `record_optional_skip_target_insensitive` (3, `details{step, optional_step, frames}`).
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

Warnings never refuse. The provenance and the output carry the same warnings.

### Writing

Everything above is computed in memory first. Then:

1. **`--lab-dir`** (usually `<install root>\packages\<game>\`; never guessed) is checked before
   anything is written: a missing directory needs an existing parent and only it is created
   (`lab_dir_created: true`), otherwise `record_lab_dir_invalid` (2). An existing
   `<lab-dir>/<D>.<ext>` holding the content D is `lab_dir_status: "present"` and is not
   written; holding anything else, or unreadable, it is `record_artifact_name_conflict` (3)
   and stays untouched.
2. The copy inside the recording, `lab/out/<D>.<ext>`, is written (an existing copy with the
   same bytes is reused; other bytes are `record_artifact_name_conflict`).
3. `<lab-dir>/<D>.<ext>.part-<unix_ms>` is written (create new) and renamed to
   `<D>.<ext>` (`lab_dir_status: "written"`); a target that appeared meanwhile is
   `record_artifact_name_conflict` and the `.part` file stays.
4. `recording.json` becomes `stopped` with its `artifact` (`container`, `digest`, `path`,
   `lab_dir_path`, `sha256` of the container bytes, `byte_count`, `package_id`, `requires`,
   `game`, `server`, `locale`, `timeout_ms`, `arrival_timeout_ms`,
   `application_arrival_timeout_ms` (defaults applied), `generated_at_unix_ms`); then the
   session file is stopped.

A failure in steps 2–4 leaves both states active and names the files already written
(`details.written_files`, `.part` files included); nothing is deleted. A stopped recording
given `--lab-dir` again checks its copy's sha256 and repeats steps 1 and 3. It is not generated
again, so a generation option that differs from what its package was generated with is refused
before anything is written: a different `--requires` is `record_requires_conflict` (3); a
different `--package-id`, `--game`, `--server`, `--locale`, `--timeout-ms`,
`--arrival-timeout-ms` or `--application-arrival-timeout-ms` is `record_stop_option_conflict`
(3, `details{field, recorded, given}`; a value the artifact did not record conflicts too). An
equal value is accepted.

### Output

`{status: "stopped" | "validated", dry_run, record, path, lab}` where `lab` is `null` or:

```
{status, dry_run, session_already_stopped?, container, digest, package_id, task_id,
 path, lab_dir_path, lab_dir_status, lab_dir_created, written_files | would_write,
 sha256, byte_count, entries, steps, click_steps, application_steps[{step, op, action, source}],
 pages, transitions, optional_steps[{step, op, settle_ms, skip_to, same_as}],
 timeouts{timeout_ms, step_timeout_ms, arrival_timeout_ms,
 application_arrival_timeout_ms}, warnings, arrival_by_time_window, marks_not_evaluated,
 steps_needing_review, cross_check, first_decision, entry_overlay,
 package_ref, requires, prerequisite_entry_example, binding_example, task_run_example,
 catalog_on_failure_example, binding_requires}
```

- `package_ref` is `{"schema_version":"actingcommand.package.content-directory.v1",
  "sha256":"<D>"}`; the package is an ordinary content-directory package and loads as
  `--package <D>.<ext> --package-ref '<package_ref>'`.
- `binding_example` is a complete `policy.procedure_manifest[]` entry: `procedure_ref` (the
  package id), `package_digest`, `operation_id: "operation.contained_task"`, `yield_points:
  []` and `scheduled_execution{mode: "device_registry", package_path}` with the absolute path
  (a configuration in the install root may use `packages/<game>/<D>.<ext>`). Whether it is
  scheduled still depends on the catalog task and its approval.
- `prerequisite_entry_example` is the entry for the actingd `prerequisite_packages` when
  another package names this one with `--requires`.
- `catalog_on_failure_example` is `{"action":"pause","retry_limit":1,
  "retry_backoff_ms":0,"escalation_threshold":2}`, the `on_failure` that
  `policy-suspension.md` ("Catalog") recommends; the `binding_requires` item about it says that
  the rerun happens immediately (R22). How failures are rerun, pause the task and are lifted is
  described there.
- `optional_steps` lists every optional step: its package step number, operation id,
  `settle_ms`, `skip_to` (the page of N) and `same_as` (the earlier step recorded for the same
  pop-up, or `null`); `[]` without optional steps.
- `binding_requires` lists what a binding also needs: the catalog task and its approval; the
  `prerequisite_packages` entry with `--requires`; without it, the return-home fallback (step
  1 should be the return-home package's final screen); the recommended `on_failure`; with
  application steps, that only an instance with an assigned application runs them and that an
  application entry package has no entry recognition (best configured as the instance's
  `startup_package`, unpacked as a directory); and, with optional steps, that they cover only
  screens appearing after the click of the step before them (a pop-up later than its
  `settle_ms`, shown more often than the copies recorded, or never recorded makes the package
  fail loudly).

## Error codes

| exit | codes |
|---|---|
| 2 | `validation_failed`, `record_flag_unsupported`, `record_flag_takes_no_value`, `record_state_dir_unsupported`, `record_frame_unreadable`, `record_transition_window_invalid`, `record_transition_has_click`, `record_application_action_invalid`, `record_application_with_click`, `record_optional_settle_invalid`, `record_locale_missing`, `record_lab_dir_invalid`, `record_requires_invalid` |
| 3 | `record_session_not_active`, `record_lab_unavailable`, `record_instance_mismatch`, `record_step_not_found`, `record_step_not_last`, `record_step_frame_missing`, `record_step_frame_conflict`, `record_frame_size_mismatch`, `record_frame_hash_mismatch`, `record_mark_rejected`, `record_mark_id_conflict`, `record_mark_id_reserved`, `record_mark_in_use`, `record_asset_name_conflict`, `record_click_exists`, `record_click_executed`, `record_click_source_invalid`, `record_click_rect_conflict`, `record_click_outside_step_rect`, `record_step_click_missing`, `record_step_click_not_executed`, `record_guard_family_invalid`, `record_append_failed_after_input`, `record_transition_without_click`, `record_transition_exists`, `record_to_transition_invalid`, `record_busy`, `record_application_entry_invalid`, `record_application_step_marks_missing`, `record_step_effect_exists`, `record_optional_first_step`, `record_optional_application`; stop: `record_no_steps`, `record_application_entry_not_first`, `record_step_without_recognition`, `record_final_step_has_click`, `record_too_many_steps`, `record_step_after_stop_invalid`, `record_application_without_home`, `record_optional_first_step`, `record_optional_final_step`, `record_optional_application`, `record_optional_restart_segment_end`, `record_step_self_mismatch`, `record_optional_step_ambiguous`, `record_optional_skip_target_insensitive`, `record_artifact_admission_failed`, `record_artifact_name_conflict`, `record_requires_conflict`, `record_stop_option_conflict` |
| 4 | `record_click_indeterminate`, `record_click_performed_with_failure`, `record_application_indeterminate` |
| 5 | `record_lock_failed`, `record_state_io_failed` (Lab state files cannot be read or written), the state directory cannot be created |
| 6 | `record_stop_generation_not_implemented` (a build without the package generator; the #336 L4 build also for a recording with optional steps, `details.reason: "optional_steps"`) |
