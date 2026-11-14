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
| `no_frames` | switch | no frame, and nothing read from one (text, values, labels), in the archive (section 6) |

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
and `..` taken out, compared without regard to ASCII case) is `report_output_refused` with `path`,
`report_output_root` (`install_root` or `state_root`, which root it is) and that root
(`install_root` or `state_root`). Only then are the installation's versions read and the Runtime
reached.

## 2 What is read

Every read names a request id, or an instance with the report's window; none reads without one,
and none starts from the ledger's first record (a page with a time bound starts at the first
record of the window, [ledger-observer.md](ledger-observer.md), "LN-WS"). One deadline of 120 s
covers the reads of one report, and each read stops 20 s after it starts at the latest. The runs
are read first, each instance's errors view after them (fix round 1), so a slow errors view
never spends the time the runs need.

| Read | Through | Bound |
|---|---|---|
| the instances | `Status` | the one named, else every one |
| each instance's failed and unfinished runs | the run listing (`runs`' code): the instance, the window, the states `failed`, `admitted`, `running`, `interrupted_unterminated` | 20 an instance, newest first |
| the run before the newest | the run listing on the newest run's instance, from the window's start to just before that run | one; kept only when it ended normally |
| a run's status | `run show`'s read, by its request id | one run |
| a run's steps | `run evidence`'s read, by its request id | every step |
| a run's geometry | `QueryEvents` of `task.geometry_observed`, by its request id | the run |
| the failing step's recognition | `QueryEvents` of `task.recognition_completed`, by the request id, from the step's first record to the next step's | that step's records |
| the diagnostic stream | `material`'s read of its reference (`ReadMaterial`, verified) | at most 8 MiB; a larger one is not read (`material_too_large`) |
| frames | `material`'s read of their references | at most 3 a run, each at most 32 MiB |
| each instance's errors view | `events`' search for the newest records (CLI-EV, [v012-register.md](v012-register.md) §1.306): the ledger's head once, the window's first record (one record of its time bound) as the floor, windows of sequences that end at the head, then the rows found with their payloads; view `errors`, the instance, the window's time bound | the newest 200; the window is not read from its start and its rows are not counted |

**A read that stops short is never "none"** (fix round 1). A run listing the deadline or the
Runtime's read budget ended before its end (`runs`' listing answers no run then) is the part
`run_listing` of `report_part_missing`, reason `not_read_to_end` (or `budget_spent` once the
report's deadline has passed), naming the instance; the summary says for each instance whether
its listing was read to its end, and says that the window holds no failed or unfinished run only
when every listing was. An errors view whose search stopped above the window's first record (its
largest window, or a read the Runtime cut) is the part `errors_view`, `not_read_to_end`, and the
summary says older rows were not searched.

Never read: a ledger, database, lock or state file directly (no `actingd suspended`, no
`check-config`, no `actingledger`); the state root's key file, the private device bindings. The
configuration (the selected `actingd.config.json`, at most 1 MiB) is read whole for its
`standard_packs.root` alone; nothing of it is written, its salt included. Never done: a capture,
a press, a pin, a confirmation that evidence was read (so the Runtime releases no kept frame
because of an export). Nothing leaves the machine: the only connection is the loopback Runtime
one, and the only file written is the archive.

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
| `report/manifest.json` | `schema_version`, `view`, `tool`, `generated_unix_ms`, `instances`, `window`, `max_runs`, `max_bytes`, `within_bound` (whether the archive is within `max_bytes`, section 8), `runtime_online`, `frames`, `files` (every other entry: `path`, `size`, `sha256`), `left_out` (`file`, `why`), `not_in_this_report_yet` (`part`, `slice`), `secrets` (what is read of the configuration and what is never opened, section 7), `masks_applied`, `warnings` |
| `report/summary.json` | the summary's data: `schema_version`, `view`, `generated_unix_ms`, `instances`, `window`, `runtime_online`, `frames`, `within_bound`, `versions`, `listings` (each instance: `alias`, `read_to_end`, `why` a token of `report_part_reason` when it was not), `runs` (each: its listing, the code's description, the step, its page, the page the run started from, the last page, the display line, the scores, its frames and whether each was kept, `excerpt_cut`, the parts missing), `succeeded_before_newest`, `errors` (each instance: `alias`, `file`, `read`, `why` when it was not, `kept`, `newest_only` (as many rows as the bound: older ones were not read), `searched_to_start`, `searched_from`, `in_archive`), `left_out`, `not_in_this_report_yet`, `warnings` |
| `report/versions.json` | `lab_ops` (this program's version); for `runtime`, `tools` and `ui` the member's `BUILD-MANIFEST.json` (`repository`, `commit_sha`, `tree_sha`, `source_artifact_name`, `interfaces`; null without one); `bundles`: each game root's bundle source `{repository, commit}` (a package is named by its `package_id` and its bundle's source, no digest) |
| `report/runs/<run id, else request id>/run.json` | the run's listing row and its status as `run show` answers it |
| `…/steps.json` | the run's steps as `run evidence` answers them (the last 1 000), `final`, `overlay` |
| `…/scores.json` | `step`, `scores` (section 3), `recognition` (the step's record as read), `excerpt_cut` |
| `…/geometry.json` | the display line and each observation (`device`, `display_map`, `conclusion`) |
| `…/overlay-excerpt.jsonl` | the failing step's stream records (section 3) |
| `…/frames/<sequence>-<role>.png` | the frames (section 4) |
| `report/instances/<alias>/errors.jsonl` | the instance's errors view: its newest rows of the window, at most 200, oldest first, each as `events --view errors --detail summary` answers it, with its `module` |

A run's and an instance's folder name keeps `A-Z a-z 0-9 _ -` of its id or alias and writes every
other character as `_`.

**The summary page** `SUMMARY.md`, fixed labels in English and Chinese, every value data:

1. The view, the instances, the window, that the Runtime answered, whether frames are in it.
2. The versions in one line (`runtime`, `tools`, `ui` by their short commit, `lab-ops`, each
   game's bundle source).
3. The screenshots (fix round 1): with frames, that the screenshots can show a player's name and
   numbers, and that the report can be exported without them (`--no-frames`; on the console,
   untick the screenshots), which leaves out what is read from them too; without, that it holds
   none and nothing read from them.
4. The size (fix round 1): within the bound of `max_mib`, or over it with every part that may be
   left out left out (section 8).
5. For each instance, whether the listing of its failed and unfinished runs was read to its end
   (and its reason when not). Then the runs' table: each failed or unfinished run (instance,
   state, package, the code it ended with, its last page, when it was first recorded), newest
   first, and the run that ended normally before the newest (`before 1`). "No failed or
   unfinished run in the window" is said only when every listing was read to its end; otherwise
   the page names the listings that were not, and says the window may hold some.
6. Each run: its terminal (state, code and the catalog's description), its package, the step it
   ended in with the page that step and the run started from, the last page recognised, the
   display line, the targets' table (target, kind, matched, last, best, threshold, margin,
   frames, what it read) and the pages' table, its frames (a frame the size bound left out says
   so), the parts missing.
7. Each instance's errors view: how many rows its file keeps, and whether they are every row of
   the window, the newest of it (as many as the bound: older ones were not read), or the newest
   found by a search that stopped above the window's start (older ones were not searched); then
   the newest ten (time, severity, code with the catalog's description, module, run).
8. What was left out and why; what is not in this report yet; the warnings.

## 6 Without frames

With `no_frames` no frame is read and none is written, and nothing read from a frame's pixels is
written either (the owner's ruling: frames are in by default and can be left out; leaving them
out leaves out what was read from them; fix round 1). The members that hold text or a value read
from a frame leave every written file:

| Where | Members left out |
|---|---|
| a diagnostic stream record of kind `ocr` | `data.raw_text`, `data.derived_text`, `data.matched_expected` |
| `ocr_block` | `data.raw.text` |
| `nn` | `data.selected_label` |
| `nn_label` | `data.raw.label` |
| `color_digest` | `data.observed_cells` (the region's colour grid, a small picture of it) |
| `target` | `data.value_test.left.raw_text`, `data.value_test.left.value`, `data.value_test.right.operand.raw_text`, `data.value_test.right.operand.value` (the text each side read and the value taken from it) |
| any stream record, and any row of an errors view | `raw_text` at any depth (an outcome keeps foreign text there as evidence, which can be a frame's text: a target's `failure`, an `error` record's outcome) |

They leave the excerpt `overlay-excerpt.jsonl`, the errors files and everything made from them
(the scores of `scores.json` and `summary.json`, the summary page); each record and row that held
any is marked `text_removed: true`, and `scores.text_removed` is true. What measures a frame
without showing what it shows stays: scores, thresholds and margins, confidences, colour
distances and means, rectangles, a value test's comparison and verdict, page and target
verdicts, sizes and the display map. What the Runtime itself recorded in the ledger beyond these
members stays as the ledger holds it (an outcome's codes and other values, a step's targets
matched or not). The summary says "none: what the screen showed is missing, and the text and
values read from it are removed", and the manifest's `left_out` names `runs/*/frames/*.png` with
`why` `no_frames`.

## 7 What is never in it, and the masks

Whatever the switches: anything of the configuration (it is read whole for its
`standard_packs.root` alone, and nothing of it is written: its `secret_fingerprint_salt` is
never in the archive; fix round 1, the manifest's `secrets` says so), the state root's key file
`runtime-state.key` (never opened), a device serial (`private\device-bindings.json` is never
opened), the installation wizard's configuration, the ledger or database, event payloads beyond
the summary detail, the whole diagnostic stream.

Every text part passes through the masks before it is written (fix round 1):

- **Replaced.** The process's user name (`USERNAME`, else `USER`) becomes `<user>`; each of the
  computer's names becomes `<computer>`: its NetBIOS name (`COMPUTERNAME`), its DNS host name and
  its full DNS name (host and domain) as the operating system gives them (`GetComputerNameExW`
  with `ComputerNameDnsHostname` and `ComputerNameDnsFullyQualified`; on another system, none).
  Each name of three characters or more is replaced wherever it stands, inside a longer word too
  (a folder `<name>_packs`), without regard to case (Unicode lowercase); the longest name is
  tried first, and a mark already written is never searched again. The segment after a `Users`
  folder of a path, with one or more separators of either kind on each side (`\Users\`,
  `/users/`, a doubly escaped `\\Users\\`), up to the next separator, quote or line break,
  becomes `<user>`, whoever's it is.
- **Not replaced.** A name of one or two characters; a name in another form than the text's own
  (percent-encoded, base64, UTF-16, split by an escape); another person's name that is not a
  `Users` folder's segment; the domain alone where it stands without the host; addresses (IP,
  MAC); anything inside a frame's pixels (frames are written as the Runtime verified them; leave
  them out with `no_frames`).

The manifest counts the masks applied (`masks_applied`). The answer line names the archive's
real path, for the caller to open it; it holds no secret.

## 8 The size bound

The archive is kept within `max_mib` where it can be. Its size is measured as written (fix round
1): each entry as the archive holds it (its local header, its packed bytes, its central directory
record: the zip of it alone less the end record), the summary, its data and the manifest
rendered for the parts kept, and the archive's end record. Parts are left out in this order while
it is over the bound:

1. the frames of the oldest run first, each run's last frame first, then the newer runs' frames;
2. the stream excerpts, the oldest run's first;
3. the errors files, the last instance's first.

Never left out: the summary, its data, the manifest, the versions, and each run's `run.json`,
`steps.json`, `scores.json` and `geometry.json`. Each part left out is a `left_out` row `{file,
why: size_bound}` in the manifest and the answer, a line of the summary, and the warning
`report_trimmed` counts them. When every part that may be left out is left out and the archive
is still over the bound (the parts never left out are larger than it), the archive is written
all the same, and says so (fix round 1): `report_trimmed` with `report_fit` `still_over` (its
`count` may be 0), `within_bound: false` in the manifest, the summary's data and the answer, and
the summary's size line. An archive that fits has `report_fit` `fits` (when anything was left out)
and `within_bound: true`.

## 9 The answer and the codes

`report_exported` (exit 0) with the archive's `path`; the line's members: `view`, `path`,
`sha256` (`sha256:<hex>`), `size`, `within_bound`, `instances`, `window`, `runs` (`request_id`,
`run_id`, `alias`, `state`, `package`), `frames`, `left_out`, `not_in_this_report_yet`, and
`warnings` when there are any.

Fix round 1 (the coordinator's ruling of 11-15, provisional while the owner refines the status
codes on board #469): one code never stands for situations a reader cannot tell apart, so each
of the four below carries a required value of a closed vocabulary.

| Code | Category | Values | When |
|---|---|---|---|
| `report_exported` | success | `path` | the archive was written |
| `report_output_refused` | error | `path`, `report_output_root` (required); `install_root` or `state_root` | the output lies inside the installation root or the state root; nothing was read or written |
| `report_write_failed` | error | `path`, `report_write_step` (required); `from_path`, `retained` | the archive could not be written at the step `report_write_step` names; caused by the operating system's failure; a partial file the export created and could not finish or move is left behind and named in `retained` |
| `report_part_missing` | warning | `report_part`, `report_part_reason` (required); `file`, `alias`, `request_id` where they apply | a part could not be read, or not to its end; caused by the read's outcome when it has one |
| `report_trimmed` | warning | `limit_bytes`, `count`, `report_fit` (required) | the size bound left `count` parts out, or could not bring the archive within it |

| Vocabulary | Tokens |
|---|---|
| `report_part` | `run_listing` (an instance's listing, `alias`), `run_before` (the listing of the run before the newest, `alias`), `run_status`, `run_steps`, `run_geometry`, `run_scores` (the failing step's recognition records), `run_excerpt` (the failing step's stream records), `frame` (each with `request_id` and `file`), `errors_view` (`alias`, `file`), `versions` (`file` `versions.json`) |
| `report_part_reason` | `not_read_to_end` (a listing or a search that stopped before its end), `budget_spent` (the report's deadline passed), `runtime_refused` (the Runtime answered with its refusal, or without the material), `read_failed` (the connection, the client's own check, the file system), `not_recorded` (a run without a diagnostic stream), `too_large` (`material_too_large`), `unreadable` (stream lines that are no record, a bundle file that is no JSON, a reference that is none) |
| `report_write_step` | `pack` (`path` the output directory), `create_directory`, `name_exists` (`path` the archive), `write_partial` (`path` the partial file), `rename` (`path` the archive, `from_path` the partial file) |
| `report_fit` | `fits`, `still_over` |
| `report_output_root` | `install_root`, `state_root` |

Reused with their own meaning: `actingctl_usage_invalid`, `since_out_of_range`,
`install_build_manifest_unreadable` (a member's build manifest that cannot be read: its version
is null), `call_budget_exhausted`, `material_too_large`, `run_unknown`, `clock_unavailable`, and
the Runtime client's codes. A fatal read (the Runtime connection gone) ends the report with its
outcome.
