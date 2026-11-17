# The regression case of a resource repository

#463 RC-1 (register [v012-register.md](v012-register.md) §1.450, 0.12.69; #463 RC-2, §1.459,
0.12.71, the notes of a scene's construction read past). A package of a resource
repository comes with regression cases: scenes on which its pages must be recognised, or must not
be, and its targets found or not. This contract makes the case format the repositories write,
`actingcommand.resource-regression-case.v1`, a contract of the Runtime, with one reader and one
runner in the Lab. Writing cases (`record stop`, #463 S2-10) and running them where a repository
is verified and built (`resource verify`'s and `standard-pack build`'s step, RV-2a) call the same
runner; this version builds the reader, the runner and the offline operation `resource
regression`, and nothing else.

## A case

- A case is a directory `regression/<game>/<case>/` of a resource repository's checkout, holding
  `case.json` and the scene image `case.json` names. Its package is `packs/<package_id>/` of the
  same checkout.
- The scene is a PNG: a canvas of one colour with patches of a source frame pasted on it at the
  rectangles `scene_construction.rects` names, never a whole frame. No player name, account or
  identifier is pasted (the recognition region and the press region are chosen so; nothing here
  checks images for privacy).
- `case.json` is one JSON object of the members below. The expectations this version evaluates
  are closed: a member the table does not list in `expect`, `expect.observe` or an entry of
  `expect.recognize` is refused (`member_unknown`), so that a misspelt expectation is never read
  past. Its top level is open (fix round 1), and so are `scene_construction`, its `canvas` and each
  rectangle (#463 RC-2): a member of them the table does not list (a repository's note, or a note
  on how the scene image was made, such as the patches it was pasted from) is read past and named,
  with its place, in the case's warning `case_member_unknown`, never a failure, as the
  repositories' own runners read past it. What a case is run on is the scene file `scene_sha256`
  pins; the canvas's size and fill, the rectangles and the hash are read from the members the
  table lists alone, so such a member changes nothing that is run.

## The members of `actingcommand.resource-regression-case.v1`

| Member | Required | Form | This version |
|---|---|---|---|
| `schema_version` | yes | `actingcommand.resource-regression-case.v1` | read: another or none is not run (below) |
| `id` | yes | a text that is not empty | read and answered |
| `package_id` | yes | one file name (no `/`, `\` or `:`, not `.` or `..`) | read: the package `packs/<package_id>/`; **checked**: the `package_id` its package's control declares (fix round 1) |
| `scene` | yes | one file name, beside `case.json` | read |
| `scene_construction.kind` | yes | `roi_patch` | checked |
| `scene_construction.canvas.width`, `.height` | yes | integers from 1, at most 16 777 216 pixels in all | **evaluated**: the scene's size |
| `scene_construction.canvas.fill` | yes | `[r, g, b]`, each 0 to 255 | **evaluated**: the scene rule (a warning) |
| `scene_construction.rects[]` | yes | a list (maybe empty) of `{x, y, width, height, content?}`, integers, `width` and `height` from 1, `content` a text | **evaluated**: each inside the canvas; the scene rule |
| `scene_construction.scene_sha256` | yes | 64 lowercase hexadecimal characters | **evaluated**: the SHA-256 of the scene file's bytes |
| `source` | no | any | kept as read, not checked: where the scene came from |
| `incident`, `issue` | no | any | not checked: why the case exists |
| `expect` | yes | an object declaring at least one expectation | |
| `expect.observe.page` | with `observe` | a page id, or `null` for no page | **evaluated** |
| `expect.observe.candidates` | no | any | not run, `not_evaluated` |
| `expect.recognize[]` | no | a list of `{target, passed}`: a target id, a boolean | **evaluated**, each entry |
| `expect.entry` | no | any | not run, `not_evaluated` |
| `expect.prerequisite` | no | any | not run, `not_evaluated` |
| `expect.step` | no | any | not run, `not_evaluated` |
| `expect.terminal_outcome`, `expect.terminal_effect` | no | any | not run, `not_evaluated`, each |
| `expect.error_page` | no | any | not run, `not_evaluated` |
| `expect.after` | no | any | not run, `not_evaluated` |

`expect`, `expect.observe` and an entry of `expect.recognize` are closed objects; the top level,
`scene_construction`, `canvas` and a rectangle are open (above). An expectation that is not
evaluated is read and answered `not_run` with `not_evaluated`, each at its place: never passed. A
case `record stop` writes (#463 S2-10, 0.12.70; [lab-recording.md](lab-recording.md), "Regression
cases written with the package") names in `source` the recording's id and where it comes from and
nothing else: `{record_id, step}` (the step's serial number), `{record_id, final: true}` or
`{record_id, ending}` (the ending's key); a recording id the Lab made by default
(`<unix_ms>-<pid>-<task_id>`) holds the time the recording began and a process number, and `source`
never holds a path of the machine, an instance, an account or a device. Its `id` is `step_<nn>`,
`final` or `ending_<outcome_key>`; it expects its page (`expect.observe.page`) and every target that
page requires to pass (`expect.recognize`), and nothing else; or, when its page is recognised on
its scene beside another page of the package (`page_conflict` among whose pages its page is, which a
linear run never asks), without `expect.observe`, every target its page requires to pass and every
target its page forbids to fail (#463 S2-10 fix round 3), so that it states that this page holds on
this scene, named where it is written with the warning `record_case_page_ambiguous`; such a page
with no target to pin gives no case (the warning `record_case_ambiguous_not_written`).

`scene_sha256` is, in this contract, the SHA-256 of the scene file's bytes, and a scene whose
bytes have another is a failure of the case. The value of cases written before this contract was
never checked by anything; the first run of this operation on the repositories shows whether they
need regenerating.

## The reader

`read_regression_case(case_json)` (`crates/lab`, `regression_case.rs`) answers the case, or the
verdict the case already has:

- **Another schema.** A `schema_version` other than this one, or none (the repositories that
  write cases of several frames, or rectangles and checks without a schema, run them with their
  own scripts): verdict `not_run`, no check, and the warning `case_schema_unsupported` (with
  `schema_version` when it declares a text of at most 256 bytes without a control character). It
  fails nothing and refuses nothing.
- **A problem.** Every problem the reader finds is a check `failed` at its JSON pointer, each with
  its word: `case_unreadable` (no JSON object), `member_missing`, `member_unknown` (inside a closed
  object), `member_invalid` (a wrong type or a value out of its form), `expect_empty` (`expect`
  declares nothing, or only an empty `recognize`), `rect_outside_canvas`. A case with a problem is
  `failed` and nothing of it is evaluated.
- **A member the table does not list of the top level** (fix round 1), **of `scene_construction`,
  of its `canvas` or of a rectangle** (#463 RC-2): read past, and named in the warning
  `case_member_unknown`, one per place and name, first among the case's warnings (those of the top
  level, then `scene_construction`'s, its canvas's and each rectangle's in order). The warning
  carries `json_pointer`, the member's place, the JSON pointer a failure there would name (`/note`
  at the top level, `/scene_construction/note`, `/scene_construction/canvas/<name>`,
  `/scene_construction/rects/<index>/<name>`), when it is at most 512 bytes, and `member`, its
  name, when it is a text of at most 256 bytes without a control character. The case is read and
  run as if it were not there. A member the table lists there that is wrong stays
  `member_invalid`; one that is required and absent, `member_missing`.

## The runner

`RegressionPackage::load(package_dir, ocr)` admits the package as the offline commands admit
theirs (`ExternallyVerifiedBundle::load_path` with the vision provider of the Lab's OCR, the
admission deadline of a contained task) and takes it as `RegressionPackage::from_bundle` does. A
directory that is not there is `package_missing`, any other refusal `package_refused`.
`RegressionPackage::from_bundle(bundle, ocr)` (fix round 1) takes a package already loaded (a
`LoadedBundle`, the kernel's admission of it): its evaluator and its page set, which must agree
(else `package_refused`), and the `package_id` its control declares.

`RegressionPackage::run(case, scene_png)` then, in this order:

0. **The case's package** (fix round 1). The case's `package_id` is the one its package's control
   declares; otherwise `package_id_mismatch` (at `/package_id`, its `mismatch` naming the case's
   and the package's, `null` when the control declares none), and nothing is evaluated.
1. **The scene against its construction.** The SHA-256 of the scene's bytes is `scene_sha256`
   (`scene_sha256_mismatch`; fix round 1: its `mismatch` names the case's value and the bytes'),
   the scene is a PNG (`scene_unreadable`) of the canvas's size (`canvas_size_mismatch`). A scene
   that does not hold evaluates nothing; each problem is a check.
2. **The scene rule** (a warning, never a failure). Every pixel outside every rectangle is the
   canvas colour. Otherwise the warning `resource_regression_scene_outside_rects` with `count`, the
   number of such pixels; the case still runs. Its rectangles are `scene_construction.rects` alone
   (#463 RC-2): a patch a case names only in a member the table does not list is outside them. A
   case the Runtime writes holds it (#463 S2-10): it
   pastes, on a black canvas (`[0, 0, 0]`) of the recording's size, the regions every recognition
   target of the whole package reads, as the admitted package declares them (a template's search
   region and its colour check's, a colour or colour digest probe's region, an OCR or NN target's
   region; a check, a value test and a click-only target none of their own; [lab-recording.md](lab-recording.md),
   "Regression cases written with the package"), where they are in the frame, each rectangle once with the targets that read it as its
   `content`; not only the targets of the case's page, so that a page told apart from this one by
   a target it forbids sees that target's region as on the frame. It holds exactly those regions;
   where a target reads the whole frame (its region or a template's search area is the whole
   frame), that patch is the whole frame and so is the scene, unlike the patches of "A case" above:
   such a case is named where it is written with the warning `record_case_whole_frame` and the
   targets that read it (#463 S2-10 fix round 3), and nothing is refused for it. A page that
   the bare canvas answers too (a dark colour, for one) is named where the case is written (the
   warning `record_case_bare_canvas`), and a target that needs several frames leaves the case
   `not_run` or `partial` with `frames_required`, named there too; neither is forced into one
   scene. Without a text-recognition model for a text target the page detection reads, no case's
   page is judged, which `record stop` says once (`record_cases_ocr_unavailable`). `record stop`
   runs each case before it writes it and writes none that fails.
3. **The scene in its package's coordinate space**, brought there as the offline commands bring
   a stored image (`scene_in_base`, `crates/lab`, Workflow #392 DA-5cd): a scene of the package's
   `coordinate_space` as it is, a pure scale of it reduced to it. A scene it refuses (another
   ratio, smaller, the other orientation; the offline commands refuse it
   `lab_scene_size_unsupported`) is `scene_size_unsupported` and evaluates nothing; a scene the
   kernel's frame gate could not read at all (the offline commands relay it as a device error) is
   `scene_gate_unreadable` (fix round 1: one cause per word, told apart by the refusal's class)
   and evaluates nothing.
4. **`expect.observe.page`** with the kernel's page detection: `ReadonlyRecognitionEngine::
   current_page` (`crates/execution-kernel`, `readonly.rs`), the function offline `current-page`
   and `observe` call, which evaluates every target of every page and answers a conflict when
   several pages match. Several pages matching is `page_conflict` (every page named in
   `matched_pages`); another page, or none, than expected `page_mismatch`; any other refusal of the
   detection `page_detection_refused`.
5. **Each entry of `expect.recognize`** with the kernel's target evaluation:
   `ReadonlyRecognitionEngine::recognize`, the function offline `recognize` calls
   (`SceneEvaluation::evaluate_target`). Its `passed` other than expected is `target_mismatch`; a
   click-only target, which is never evaluated on a scene, `target_click_only`; a refusal (a target
   the package does not declare) `target_refused`.
6. **The expectations not evaluated**, each `not_run` with `not_evaluated`.

Nothing is evaluated a second way: the runner calls these two kernel functions and reads their
answers. An evaluation the offline commands refuse is `not_run`, with its word:
`ocr_unavailable` when the page detection, or the target, reaches a text-recognition target and
the Lab has no OCR model (the Lab's own check, `LabOcr::require_pages` or `LabOcr::require`);
`frames_required` when the package's target consensus reads more than one frame, which one scene
cannot give.

`run_regression_case_on_bundle(bundle, case_json, scene_png, ocr)` (fix round 1) is the one call
on a package already loaded, what `record stop` holds in memory (#463 S2-10: its last self-check,
`cases`, calls it on every case it makes, and once on the bare canvas): it reads the case, takes the
package (`from_bundle`) and runs the case; it reads no file. `run_regression_case(package_dir,
case_json, scene_png, ocr)` admits the package directory and calls it (a package missing or
refused answers the case refused by it, or the case's own verdict when the reader refuses it or it
is of another schema). Neither reads a command line or writes anything.

## The verdict

A case answers `{id, package_id, verdict, checks, warnings?}`:

- `id` and `package_id` as the case gives them as text, else `null`;
- `checks`: each `{at, result, problem?, reason?, page?, target?, mismatch?, detail?}`: `at` its
  JSON pointer (`""` the whole document), `result` `passed`, `failed` or `not_run`, `problem` the
  word of a failure, `reason` the word of a check not run, `page` `{expected, recognized,
  matched_pages?}`, `target` `{target, expected, recognized}` (`recognized` the kernel's answer,
  `null` when not evaluated), `mismatch` `{case, found}` the two values a check compared (fix round
  1: `scene_sha256_mismatch`, `package_id_mismatch`), `detail` the text of the reader, the loader or
  the kernel, for people;
- `warnings`: each `{outcome}` (`case_member_unknown` first, then the scene rule's);
- `verdict`: `failed` when a check failed; `passed` when every check passed; `partial` when what
  ran passed and something did not run; `not_run` when nothing ran (another schema, or no
  expectation could be evaluated).

## The operation: `resource regression`

```text
actinglab [--json] [--game <game>] resource regression --repo <checkout> [--case <case>]
    [--package-id <package_id>]
```

- **The cases** are the directories `regression/<game>/<case>/` of the checkout, in the order of
  their names. `--game` (actinglab's global option) keeps one game's, `--case` the directories of
  that name, `--package-id` the cases whose `case.json` names that package (a case whose package
  is not known, one without a readable `case.json` or another schema's without that
  `package_id`, is not selected then). A directory without a readable `case.json` is a case of its
  own, `failed` with `case_missing`; a scene file that cannot be read is `scene_missing`. No case
  selected is refused (`validation_failed`), and so are a global option that names a Runtime, a
  device, an instance, a server, a backend, a resource or a run root
  (`offline_device_scope_forbidden`), a positional, a flag off its list and a flag given twice.
- **The package** of a case is `packs/<package_id>/`, admitted once per run however many cases
  name it.
- **A row** (fix round 1) is a case, short: `{path, id, package_id, verdict, checks, warnings?}`,
  `path` `regression/<game>/<case>`, `id` and `package_id` as the case gives them (else `null`),
  `checks` only those that did not pass (failed or not run), each with its word and what it names
  (`at`, `page`, `target`, `mismatch`) and without its `detail`; never the case's document.
- **The answer** (fix round 1: it always fits, and says which cases failed first): `repo`; `counts`
  `{cases, passed, failed, partial, not_run}`; `warnings`. When no case failed: `cases`, the rows of
  every selected case in the order of their paths, only when `--game`, `--case` or `--package-id`
  asked for them. When a case failed: `failed_total`, how many cases did not pass (`failed` and
  `partial`); `cases`, the rows of the cases that did not pass, the failed ones first, each group in
  the order of their paths, at most **10** (the operation's bound); `listed`, how many rows are
  given (`failed_total` minus `listed` were left out). A case's warnings are also written on stderr
  as developer lines.
- **Partial cases are said aloud** (fix round 1): a run with `partial` cases answers, once, the
  warning `resource_regression_partial` with `count`, their number, in `warnings` (beside the rows),
  and writes its developer line; a run with `partial` and no failed case succeeds.
- **The exit.** When a case failed, the refusal `resource_regression_failed` whose details are the
  answer: exit 1 on actinglab's own line, 2 on the `--via` line, a refusal through MCP. Otherwise the
  answer, exit 0 on both lines. A case of another schema, `partial` and `not_run` fail nothing.
- **It reads only**: nothing is written, no Runtime connection is opened, no device is touched. Its
  dry run is `no_effect` ([actinglab-dry-run.md](actinglab-dry-run.md)).
- **MCP**: `ac_resource_regression` (`repo`, `game`, `case`, `package_id`), through a `serve`
  child. A success line over the answer budget (the rows a filter asked for) is written whole to
  the export directory and keeps `counts` beside it; a failure line is never exported, and its
  bound makes it fit ([apps/actinglab-mcp/MANUAL.md](../apps/actinglab-mcp/MANUAL.md), section 5).

## Codes and words

| Code or word | What it says | Refuses, warns or informs | Exit (actinglab's own line; the `--via` line) |
|---|---|---|---|
| `resource_regression_failed` (actinglab) | at least one case's verdict is `failed` | refuses | 1; 2 |
| `resource_regression_partial` (actinglab), `count` | `count` cases of the run are `partial` (fix round 1) | warns | 0; 0 when no case failed |
| `case_schema_unsupported` (lab) | a case of another `schema_version`, or none, is not run | warns | 0; 0 |
| `case_member_unknown` (lab), `json_pointer?`, `member?` | a member of a case's top level (fix round 1), of its `scene_construction`, its `canvas` or a rectangle (#463 RC-2) the contract does not name, read past; `json_pointer` its place (RC-2) | warns | 0; 0 |
| `resource_regression_scene_outside_rects` (lab), `count` | pixels outside the rectangles are not the canvas colour | warns | 0; 0 |
| `regression_verdict`: `passed`, `failed`, `partial`, `not_run` | a case's verdict | informs (`failed` makes the run refuse) | |
| `regression_result`: `passed`, `failed`, `not_run` | a check's result | informs | |
| `regression_problem`: `case_missing`, `case_unreadable`, `member_missing`, `member_unknown`, `member_invalid`, `expect_empty`, `scene_missing`, `scene_unreadable`, `scene_sha256_mismatch`, `canvas_size_mismatch`, `scene_size_unsupported`, `scene_gate_unreadable`, `rect_outside_canvas`, `package_missing`, `package_refused`, `package_id_mismatch`, `page_conflict`, `page_mismatch`, `page_detection_refused`, `target_mismatch`, `target_click_only`, `target_refused` | why a check failed, one cause each (fix round 1: `scene_gate_unreadable`, `package_id_mismatch`) | fails the case | |
| `regression_not_run`: `not_evaluated`, `ocr_unavailable`, `frames_required` | why a check did not run | informs | |

No decision reads a code: the run refuses by the verdict word `failed`.

## What this version does not do

- It does not evaluate the expectations the table marks not run; it answers them.
- `resource verify` and `standard-pack build` do not run cases yet: their step (RV-2a) is another
  slice ([resource-verify.md](resource-verify.md) only checks that the files under `regression/`
  are the committed ones).
- It does not read the other formats of the repositories' cases; they stay with each repository's
  own script until they are written in this format.
