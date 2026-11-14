# Problem report (`report export`)

Workflow #467 RPT-1, since 0.12.61 ([v012-register.md](v012-register.md) §1.379). The row
`report export` of the operation table ([lab-operations.json](lab-operations.json), module
`core`, slice `RPT-1`; [lab-two-entries.md](lab-two-entries.md) §9.3) writes one archive from
which a person can tell, without asking again, which task failed, at which step, on which page,
which target was looked for and what it scored against which threshold, what the screen showed,
and whether the device's frames were the package's size or reduced. The code is
`crates/lab-ops/src/report.rs` with `report/{source,scores,summary,archive}.rs`.

The export is a view of the ledger (the owner's answer of 11-14 on board #467). This slice builds
the view `errors`: the newest failed and unfinished runs of a window and the rows of the ledger's
`errors` view in it. Every parameter, answer and manifest says which view it holds (`view`), so
the next views come without renaming anything. Not built yet, and named as missing in every
report ("Not in this report yet"): the views `package` and `run`, successes included (RPT-2);
the part that needs no Runtime (the configuration without its secrets, the tails of the Runtime's
and the console's logs, each instance's schedule and status, the machine, the plan of a report
before it is written, a report while the Runtime is stopped: RPT-3); the tool on `actingctl
mcp-serve` (RPT-4).

## 1 The row

| Parameter | Form | Meaning |
|---|---|---|
| `instance` | positional (also `--instance`) | the instance read (alias, `instance_id` or ADB port); absent: every instance of the Runtime's status |
| `view` | option, `enum:errors` | the view; `errors` (the default) is the only one of this slice |
| `out` | option, `dir` | the directory the archive is written into, made when missing; required on the command line and in `serve` (`--out` `required`); the Lab's MCP server gives its export directory (`%TEMP%\actingcommand-mcp\materials`), as for `frame`, and the tool takes no `out` |
| `since` | option, duration | how far back the window reaches; default `24h`, at most 7 days (`since_out_of_range` with the oldest start) |
| `until_unix_ms` | option, unsigned | the window's end; absent: now; one before the window's start is `actingctl_usage_invalid` (`--until-unix-ms`, `conflicting`) |
| `max_runs` | option, integer 1..10 | the failed and unfinished runs kept; default 5 |
| `max_mib` | option, integer 4..64 | the archive's bound in MiB; default 24 |
| `no_frames` | switch | no frame and no text OCR read in the archive (section 6) |

`writes` is false (nothing of the Runtime or the installation changes; one archive is written
into `out`, as `material` with `out` writes its file), `irreversible` false, `runtime`
`required`, `entries` `["lab", "ui"]` (it sends `Status`, `QueryEvents` and `ReadMaterial`, all
on the UI list), MCP tool `ac_report_export` on `actinglab-mcp`. A Runtime that does not answer
gives the line the Runtime client gives (the part that needs none comes with RPT-3).

```text
actinglab report export --out C:\Users\<user>\Documents\reports
actinglab report export <inst> --since 48h --no-frames --out C:\Users\<user>\Documents\reports
```

**Before anything is read.** The parameters are read, then the output is checked: a path that
exists and is no directory is `actingctl_usage_invalid` (`--out`, `unusable`); a directory inside
the installation root or the Runtime's state root (each resolved: the longest existing part of
the path with its junctions and links resolved and its letters in the file system's case, `.`
and `..` taken out, compared without regard to ASCII case) is `report_output_refused` with `path`
and that root (`install_root` or `state_root`). Only then are the installation's versions read
and the Runtime reached.

## 2 What is read

Every read names a request id, or an instance with the report's window; none reads without one,
and none starts from the ledger's first record (a page with a time bound starts at the first
record of the window, [ledger-observer.md](ledger-observer.md), "LN-WS"). One deadline of 120 s
covers the reads of one report, and each read stops 20 s after it starts at the latest; a read
the deadline stopped is a part missing with `call_budget_exhausted` as its cause.

| Read | Through | Bound |
|---|---|---|
| the instances | `Status` | the one named, else every one |
| each instance's errors view | `QueryEvents`, view `errors`, the instance, from the window's start (to its end) | every row of the window counted; the newest 200 kept |
| each instance's failed and unfinished runs | the run listing (`runs`' code): the instance, the window, the states `failed`, `admitted`, `running`, `interrupted_unterminated` | 20 an instance, newest first |
| the run before the newest | the run listing on the newest run's instance, from the window's start to just before that run | one; kept only when it ended normally |
| a run's status | `run show`'s read, by its request id | one run |
| a run's steps | `run evidence`'s read, by its request id | every step |
| a run's geometry | `QueryEvents` of `task.geometry_observed`, by its request id | the run |
| the failing step's recognition | `QueryEvents` of `task.recognition_completed`, by the request id, from the step's first record to the next step's | that step's records |
| the diagnostic stream | `material`'s read of its reference (`ReadMaterial`, verified) | at most 8 MiB; a larger one is not read (`material_too_large`) |
| frames | `material`'s read of their references | at most 3 a run, each at most 32 MiB |

Never read: a ledger, database, lock or state file directly (no `actingd suspended`, no
`check-config`, no `actingledger`); the configuration's salt, the state root's key file, the
private device bindings. Never done: a capture, a press, a pin, a confirmation that evidence was
read (so the Runtime releases no kept frame because of an export). Nothing leaves the machine:
the only connection is the loopback Runtime one, and the only file written is the archive.

**The runs.** The failed and unfinished runs of every instance read, newest first (by their first
record), at most `max_runs`, at most two of one package that ended with one code. The run that
ended normally just before the newest on its instance is listed in the summary's table only (one
line), so a reader sees where the game was left.

**The failing step** of a run: the last step that carries the run's failure, else the last that
did not finish, else the last.

## 3 Scores

From the run's diagnostic stream ([task-diagnostic-stream.md](task-diagnostic-stream.md)), the
failing step's records: from its last `step_started` to the next step's, and every `terminal`
record after them. For each target: its kind, matched or not (its last evaluation), how many
frames evaluated it, its last and best template score, the threshold, the margin (last score less
the threshold; for a colour, the largest distance less the distance), the colour distance, the
text an OCR target read and its confidence, a value test's sides, its failure. For each page the
step evaluated: matched or not, how often, the first target that failed it. `scores.json` says
`from: stream`.

Without the stream (the run has none, it is above 8 MiB, it cannot be read): what the failing
step's last `task.recognition_completed` holds (`from: recognition`): each target matched or not
(and its box, not exported), and each candidate page's verdict (`candidate_scores`, ledger
revision 4: `passed`, `best_score_milli`, `first_failed_target_id`; at most 16 pages,
`omitted` the rest). The record holds no score per target and no threshold; the summary says
so. Neither: `from: none`.

The excerpt `overlay-excerpt.jsonl` is the failing step's records as the stream holds them
(masked, section 7), the newest kept within 2 MiB (`excerpt_cut` when older ones were left out).

## 4 Frames and the display

**Frames**, each once, at most three a run: the failing step's decided frame (when the first of
its frames is the one it decided on), its last frame, and the run's last frame; written as
`frames/<sequence>-<role>.png` (`decided`, `step-last`, `run-last`) with the bytes the Runtime
verified, never re-encoded. A frame the Runtime no longer has (its cleaner removed it) is a part
missing; the rest is written.

**The display line** of a run, from its newest `task.geometry_observed` that names the device
frame (the stored frame is always the device's own; `display_map` is recorded only when the map
is not the plain identity, [display-adapter.md](display-adapter.md)):

| Recorded | Line |
|---|---|
| a frame and a map of kind `scale` | `device 1920×1080, package 1280×720, scale 3/2, recognition on the reduced frame` |
| a frame and a map of kind `translate` | `device W×H, package w×h, scale n/d, regions moved by anchors on a w×h frame` |
| a frame, no map, the observation passed | `device 1280×720, package 1280×720, not scaled` |
| a frame, no map, another conclusion | `device W×H, no display map recorded (geometry <conclusion>)` |
| no frame | `device frame not recorded (geometry <conclusion>)` |
| no observation | `no geometry observation recorded` |

## 5 The archive

One file `problem-report-<YYYYMMDD-HHMMSS>Z-<the first 8 hex digits of its SHA-256>.zip` in
`out`, written as `<name>.partial` and moved into place; a file of that name is never replaced
(`report_write_failed`). Schema `actingcommand.problem-report.v1`. Entries, in order, deflated
(frames stored), with no time of their own:

| Entry | Holds |
|---|---|
| `report/SUMMARY.md` | the page a person reads first (below) |
| `report/manifest.json` | `schema_version`, `view`, `tool`, `generated_unix_ms`, `instances`, `window`, `max_runs`, `max_bytes`, `runtime_online`, `frames`, `files` (every other entry: `path`, `size`, `sha256`), `left_out` (`file`, `why`), `not_in_this_report_yet` (`part`, `slice`), `secrets` (what is never read), `masks_applied`, `warnings` |
| `report/summary.json` | the summary's data: `schema_version`, `view`, `generated_unix_ms`, `instances`, `window`, `runtime_online`, `frames`, `versions`, `runs` (each: its listing, the code's description, the step, its page, the page the run started from, the last page, the display line, the scores, its frames and whether each was kept, `excerpt_cut`, the parts missing), `succeeded_before_newest`, `errors` (each instance: `alias`, `file`, `read`, `total`, `kept`, `in_archive`), `left_out`, `not_in_this_report_yet`, `warnings` |
| `report/versions.json` | `lab_ops` (this program's version); for `runtime`, `tools` and `ui` the member's `BUILD-MANIFEST.json` (`repository`, `commit_sha`, `tree_sha`, `source_artifact_name`, `interfaces`; null without one); `bundles`: each game root's bundle source `{repository, commit}` (a package is named by its `package_id` and its bundle's source, no digest) |
| `report/runs/<run id, else request id>/run.json` | the run's listing row and its status as `run show` answers it |
| `…/steps.json` | the run's steps as `run evidence` answers them (the last 1 000), `final`, `overlay` |
| `…/scores.json` | `step`, `scores` (section 3), `recognition` (the step's record as read), `excerpt_cut` |
| `…/geometry.json` | the display line and each observation (`device`, `display_map`, `conclusion`) |
| `…/overlay-excerpt.jsonl` | the failing step's stream records (section 3) |
| `…/frames/<sequence>-<role>.png` | the frames (section 4) |
| `report/instances/<alias>/errors.jsonl` | the instance's errors view: the newest 200 rows of the window, oldest first, each as `events --view errors --detail summary` answers it, with its `module` |

A run's and an instance's folder name keeps `A-Z a-z 0-9 _ -` of its id or alias and writes every
other character as `_`.

**The summary page** `SUMMARY.md`, fixed labels in English and Chinese, every value data:

1. The view, the instances, the window, that the Runtime answered, whether frames are in it.
2. The versions in one line (`runtime`, `tools`, `ui` by their short commit, `lab-ops`, each
   game's bundle source).
3. The runs' table: each failed or unfinished run (instance, state, package, the code it ended
   with, its last page, when it was first recorded), newest first, and the run that ended
   normally before the newest (`before 1`).
4. Each run: its terminal (state, code and the catalog's description), its package, the step it
   ended in with the page that step and the run started from, the last page recognised, the
   display line, the targets' table (target, kind, matched, last, best, threshold, margin,
   frames, what it read) and the pages' table, its frames (a frame the size bound left out says
   so), the parts missing.
5. Each instance's errors view: how many rows the window held, how many its file keeps, and the
   newest ten (time, severity, code with the catalog's description, module, run).
6. What was left out and why; what is not in this report yet; the warnings.

## 6 Without frames

With `no_frames` no frame is read and none is written; the members that hold text a frame shows
(`raw_text`, `derived_text`, `text`, at any depth) leave the excerpt and the scores, each record
that held some marked `text_removed: true`, and `scores.text_removed` is true. The summary says
"none: what the screen showed is missing, and the text OCR read is removed", and the manifest's
`left_out` names `runs/*/frames/*.png` with `why` `no_frames`. Scores, thresholds and margins
stay.

## 7 What is never in it, and the masks

Whatever the switches: the configuration's `secret_fingerprint_salt`, the state root's key file
`runtime-state.key`, a device serial (`private\device-bindings.json` is never opened), the
installation wizard's configuration, the ledger or database, event payloads beyond the summary
detail, the whole diagnostic stream.

Every text part passes through the masks before it is written: a path's segment after
`\Users\` (or `/Users/`, any case, either separator) becomes `<user>`; the process's user name
(`USERNAME`, else `USER`) and computer name (`COMPUTERNAME`), each of two characters or more, as
whole words in any ASCII case, become `<user>` and `<computer>`. The manifest counts the masks
applied (`masks_applied`). The answer line names the archive's real path, for the caller to open
it; it holds no secret.

## 8 The size bound

The archive is kept within `max_mib`. Each entry counts its bytes and 512 more, and 1 MiB is kept
for the summary, its data and the manifest. Parts are left out in this order until it fits:

1. the frames of the oldest run first, each run's last frame first, then the newer runs' frames;
2. the stream excerpts, the oldest run's first;
3. the errors files, the last instance's first.

Never left out: the summary, its data, the manifest, the versions, and each run's `run.json`,
`steps.json`, `scores.json` and `geometry.json`. Each part left out is a `left_out` row `{file,
why: size_bound}` in the manifest and the answer, a line of the summary, and the warning
`report_trimmed` counts them.

## 9 The answer and the codes

`report_exported` (exit 0) with the archive's `path`; the line's members: `view`, `path`,
`sha256` (`sha256:<hex>`), `size`, `instances`, `window`, `runs` (`request_id`, `run_id`,
`alias`, `state`, `package`), `frames`, `left_out`, `not_in_this_report_yet`, and `warnings` when
there are any.

| Code | Category | Values | When |
|---|---|---|---|
| `report_exported` | success | `path` | the archive was written |
| `report_output_refused` | error | `path`, `install_root` or `state_root` | the output lies inside the installation root or the state root; nothing was read or written |
| `report_write_failed` | error | `path`, `from_path` | the directory, the partial file or the archive could not be written, or a file of its name exists; caused by the operating system's failure |
| `report_part_missing` | warning | `file`, `request_id` | a part could not be read (caused by the read's outcome: `material_too_large`, `call_budget_exhausted`, the Runtime client's own codes; a run without a diagnostic stream has no cause) |
| `report_trimmed` | warning | `limit_bytes`, `count` | the size bound left `count` parts out |

Reused with their own meaning: `actingctl_usage_invalid`, `since_out_of_range`,
`install_build_manifest_unreadable` (a member's build manifest that cannot be read: its version
is null), `call_budget_exhausted`, `material_too_large`, `run_unknown`, `clock_unavailable`, and
the Runtime client's codes. A fatal read (the Runtime connection gone) ends the report with its
outcome.
