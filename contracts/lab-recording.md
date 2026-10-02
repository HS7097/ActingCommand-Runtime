# Lab recording (`record start` / `mark` / `status` / `stop`, `--record`)

A Lab recording turns a sequence of real screens into a linear-steps task package: each
step is one screen with its recognition marks and at most one effect, a click rectangle or
an application operation (R24), and the last step only recognizes. This document is the
first half of the contract (Workflow #336): the recording state, the commands that build
it, the five mark families with their mark-time self-test, steps and transitions,
remediation, application steps, `--record` on the device commands and the recording lock.
Package generation by `record stop` is described in the second half.

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
  "Application steps") is absent on steps without one.

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
            [--replace-click] [--remove <id>]… [--dry-run] [--state-dir <dir>]
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
 "transition":null,"replace_transition":false,"step_action":null,"application":null}
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
click, application, transition, step_state{marks, frames, effect, transition, closed},
closed_step, dry_run}`. `step_state.effect` is `none`, `click_declared`, `click_executed`,
`click_indeterminate`, `application_declared` or `application_executed`.

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
         transition: null | {kind:"page", frames, marks, timeout_ms, source} | {kind:"window", min_ms, max_ms},
         closed, closed_by}],
 artifact|null, record_flag_state_dir, record_flag_reachable}
```

`click.outcome` is `declared` (no outcome yet), `executed` (Performed) or `indeterminate`
(the outcome of the last `do --capture --record` is unknown); `attempts` counts the earlier
outcomes moved aside by `--reopen-step`.

### record stop

Without Lab steps `record stop` behaves as before and adds `lab: null`; an empty active Lab
recording is stopped with the session. With Lab steps it generates the package (second
half of this contract); a build without the generator refuses with
`record_stop_generation_not_implemented` (exit 6) and leaves both states active.

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
  - `--to-transition n` turns step n (the last effective step: no click, at least one mark)
    into the page transition of the previous effective step m (which has a click and no
    transition yet). Its frames (roles `transition`/`transition_sample`), marks and reused
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
| `record_flag_unsupported` | offline commands, `capture diagnose`, element or swipe clicks, `do --dry-run`, `session app --dry-run` (global or after the command), `session app` without one of its four actions, `tap`/`swipe`/`long-tap` and every other command |
| `record_flag_takes_no_value` | `--record <value>` |
| `record_state_dir_unsupported` | `--record` with `--state-dir`; `--record` uses `ACTINGLAB_SESSION_STATE_DIR` or the default state root |
| `validation_failed` | `--tap-rect` without `--record` |

The record instance (`--instance` or the instance configuration) and the command instance
must be the same (`record_instance_mismatch`, exit 3); a missing or inactive session is
`record_session_not_active`. Every check runs before anything is captured or pressed.

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
  interface). `record stop` (package generation, second half of this contract) refuses a
  recording without it; `record mark` and `session app --record` do not check it. Screens a
  cold start shows only sometimes (a daily notice, an update prompt) cannot branch in a
  linear-steps package; a failure there, before the main interface, is only run again.

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

## Error codes

| exit | codes |
|---|---|
| 2 | `validation_failed`, `record_flag_unsupported`, `record_flag_takes_no_value`, `record_state_dir_unsupported`, `record_frame_unreadable`, `record_transition_window_invalid`, `record_transition_has_click`, `record_application_action_invalid`, `record_application_with_click` |
| 3 | `record_session_not_active`, `record_lab_unavailable`, `record_instance_mismatch`, `record_step_not_found`, `record_step_not_last`, `record_step_frame_missing`, `record_step_frame_conflict`, `record_frame_size_mismatch`, `record_frame_hash_mismatch`, `record_mark_rejected`, `record_mark_id_conflict`, `record_mark_id_reserved`, `record_mark_in_use`, `record_asset_name_conflict`, `record_click_exists`, `record_click_executed`, `record_click_source_invalid`, `record_click_rect_conflict`, `record_click_outside_step_rect`, `record_step_click_missing`, `record_step_click_not_executed`, `record_guard_family_invalid`, `record_append_failed_after_input`, `record_transition_without_click`, `record_transition_exists`, `record_to_transition_invalid`, `record_busy`, `record_application_entry_invalid`, `record_application_step_marks_missing`, `record_step_effect_exists` |
| 4 | `record_click_indeterminate`, `record_click_performed_with_failure`, `record_application_indeterminate` |
| 5 | `record_lock_failed`, `record_state_io_failed` (Lab state files cannot be read or written), the state directory cannot be created |
| 6 | `record_stop_generation_not_implemented` (a build without the package generator) |
