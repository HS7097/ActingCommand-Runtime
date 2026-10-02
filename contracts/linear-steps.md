# Linear steps

This document is the `linear_steps` execution mode of contained task packages
([Workflow #336](https://github.com/HS7097/ActingCommand-Workflow/issues/336), rulings R12 and
R13). A linear task runs the one path it declares, step by step: each step first recognizes
its own page, then clicks once, and the click leads to the next step. Packages that do not
declare the mode keep their page-graph execution unchanged. No game-specific values are part
of this contract.

## Declaration

The mode is declared only in `control.json`:

```json
{"schema_version": "Lab-1y.control.v2", "execution_mode": "linear_steps", "max_steps": 2, "...": "..."}
```

`task.json` has no mode field. Each operation may carry one new field, `transition`, which is
admitted only under `linear_steps`:

```json
"transition": {"kind": "page", "page_id": "transition_01", "timeout_ms": 8000, "interval_ms": 50}
"transition": {"kind": "window", "min_ms": 1000, "max_ms": 3000}
```

Omitting `transition` (or writing `null`) declares no intermediate state. The resource
declaration check admits the field's shape (`kind` is `page` or `window`; `page_id` a string;
`timeout_ms`, `interval_ms`, `min_ms` and `max_ms` unsigned integers; no other field) and
reports a wrong shape with the existing reasons `UnknownField`, `MissingField`,
`InvalidType` and `InvalidValue`. The values are checked at Runtime admission (below).

The source parser takes a `page` transition's `page_id` into the task's page set, like an
operation's destination: the page is built from its `page_rules` entry, kept by a selected
(`package build`) build, and must be declared by an anchor or a page rule.

## Admission

A task whose control declares `linear_steps` is admitted by these rules after the common task
checks (schema, task id, coordinate space, task timeout and `max_steps` pairing). A violation
is `contained_task_linear_invalid` with detail `reason=<reason>`, plus `operation=<id>` when
one operation is at fault; an existing check keeps its existing code.

| Rule | `reason` |
|---|---|
| task schema `0.9` with control `Lab-1y.control.v2` | `schema_version` |
| no `phases` | `phases` |
| 1 to 1000 operations | `operation_count` |
| `max_steps` declared in task and control, both equal to the operation count | `max_steps` |
| no `error_pages`, `recovery`, `stability_termination`, `post_admission_ocr`, `resource_readings`; `stop_on_confirmation` is not `false` | the field name |
| each operation's effect is `click`; no `application`, `select` or `on_error` | `operation_effect` |
| each operation's `to` / `expect_after` names exactly one page | `destination` |
| each operation's destination differs from its `from` | `to_equals_from` |
| `entry_page` declared and equal to the first operation's `from` | `entry_page` |
| each operation's destination equals the next operation's `from` | `chain` |
| `target_page` names one page, equal to the last operation's destination | `target_page` |
| a `page` transition names a page other than its operation's `from` and destination | `transition_page` |
| a `page` transition's `timeout_ms` is in `1..=1800000` | `transition_timeout` |
| a `page` transition's `interval_ms` is in `1..=5000` | `transition_interval` |
| a `window` has `0 <= min_ms <= max_ms`, `1 <= max_ms <= 1800000`, and `min_ms` below the task timeout | `transition_window` |
| a `scheduling_outcome`, when declared, has no designated operation | `designated_operation` |
| every terminal page of that outcome is the target page | `terminal_page` |

Every operation still passes the existing operation checks (guard or trusted coordinate, click
shape, retry fields, `post_delay_ms`); operation ids are unique (`contained_task_program_invalid`).
Every page above is resolved to exactly one detector page id (`<game>/<page>`, with or without
the game prefix in the declaration); a page that resolves to none or to several is the
existing `contained_task_page_set_invalid`. All comparisons use the resolved ids. A
`transition` on an operation of any other mode is `contained_task_operation_invalid`. The
page-graph checks (scheduling outcome coverage, phases) are not run, and a linear task has no
required home entry page, so the host runs no entry preflight for it.

## Execution

After `PackageAdmitted` and `RunStarted`, the kernel runs the admitted path; no run state
machine is built and no page is recognized globally.

**Observing.** Each capture is recorded as on the page-graph path (the `CapturePage` timing
boundary, `CaptureCompleted`, the committed input frame context when the frame carries an
input reference, `RecognitionStarted`, the page evaluations, `RecognitionCompleted`), but only
the pages the run currently waits for are evaluated, and they are its candidate pages. The
frame's page is the first candidate that passes; several passing candidates are no conflict.
An evaluation error is `contained_task_recognition_failed`. A wait captures, returns on a
passing candidate, gives up once its budget is spent, and otherwise sleeps the smaller of its
interval and its remaining budget; the task deadline inside a wait is `contained_task_timeout`.

**Steps.** The first step waits for its page for `step_timeout_ms` at the control capture
interval; when it does not pass, the task fails with `contained_task_linear_entry_unmatched`.
Then, for operation `k` (`step_index` `k`, counted from 0):

1. The run progress becomes `k + 1`, and `StepStarted` names the step's page.
2. The guard is evaluated on the frame on which the step's page passed; a refusal is the
   existing `contained_task_guard_refused`, with no input and no retry.
3. The effect intent, the input bound to that frame's input context and the effect completion
   follow, in the order of the page-graph path.
4. The post-input wait is `post_delay_ms`, or the window's `min_ms` when larger.
5. The intermediate state, then the next step's page, are awaited (below).
6. `StepFinished` names the next step's page, and that frame is the next step's decision frame.

After the last operation, the run finishes with the target page and `executed_steps` equal to
the operation count.

## Intermediate states

The arrival budget `T` of an operation is its `expect_after.timeout_ms`, or the control
`step_timeout_ms`; its interval is `expect_after.interval_ms`, or the control capture interval.

- **None.** The next step's page is awaited for `T`.
- **Page.** The intermediate page must be seen: it is awaited for `timeout_ms` (default `T`)
  at `interval_ms` (default the control capture interval), and then the next step's page is
  awaited for `T`, counted from the frame that showed the intermediate page. A next step's
  page that already passes never skips the intermediate page: the intermediate page proves
  the input took effect before the next step is trusted. An intermediate page shorter than one
  capture interval is missed and the attempt fails; such a state is declared as a window.
- **Window.** Nothing is evaluated before `min_ms`; the next step's page is then awaited for
  `(max_ms - w) + T`, where `w` is the post-input wait, and passes as soon as it is seen.

## Retries

A retry exists only for a swallowed input; the existing `retryable`, `max_attempts` and
`retry_interval_ms` declare it, and an operation without them is not retried. The gate of an
operation is the first page awaited after its input: its intermediate page, or the next
step's page.

- Once an attempt has seen its intermediate page, a next step's page that does not follow
  fails the task (`page_confirmation_failed`, `intermediate_seen=true`), with no retry
  decision and no further input.
- Any other failed attempt with attempts left waits `retry_interval_ms` and then waits for
  `step_timeout_ms` for either the gate or the step's own page, the gate first:
  - the gate passes: the input took effect late; the attempt continues without another input
    (an intermediate gate then counts as seen);
  - the step's own page passes: the input was swallowed; `StepFinished` names the step's page
    and the next attempt starts on that frame with another `StepStarted` of the same
    `step_index`;
  - neither passes: `StepFinished` names `<unrecognized>` and the task fails.
- With no attempt left, `StepFinished` names `<unrecognized>` and the task fails.

`executed_steps` counts dispatched operations: a retry adds none.

## Failure codes

| Situation | `failure_code` | Timing (existing values) |
|---|---|---|
| The first step's page does not pass within `step_timeout_ms` | `contained_task_linear_entry_unmatched` | `page_recognition` / `entry_recognition` |
| The intermediate page is not seen within its timeout, with no attempt left | `contained_task_linear_intermediate_unobserved` | `postcondition` / `postcondition` |
| The next step's page does not pass in its budget (with no attempt left), the retry decision sees neither page, or the next step's page does not follow a seen intermediate page | `page_confirmation_failed`, detail `transition=none\|page\|window ...` (`intermediate_seen=true` after a seen intermediate page) | `postcondition` / `postcondition`, `limit_ms` the spent budget |
| A guard refusal | `contained_task_guard_refused` | none |
| The task deadline | `contained_task_timeout` | `task` / the stage it expired in |
| A recognition error | `contained_task_recognition_failed` | none |
| A frame of another size | `contained_task_frame_resolution_mismatch` | none |
| A package outside the admission rules | `contained_task_linear_invalid` (admission) | none |

The new codes are strings; none of them starts the stuck-recovery ladder.

## Ledger

A linear run writes only existing records with existing fields and enum values: no event
type, payload field, `PackageRef` variant or enum value (`ResourceDeclarationReason`,
`TaskTimingStage`, `TaskTimingScope`, `TaskTimingBoundary`) is added, so a ledger written by a
linear run is read by older builds.

| When | Existing record | Values |
|---|---|---|
| Admission | `PackageAdmitted` | the package reference |
| Each capture | the capture records and their evidence | unchanged |
| Each recognition | `RecognitionStarted`, `RecognitionCompleted` | `candidate_pages` the one or two detector page ids awaited; `matched_page` the passing one, or none |
| Step start | `StepStarted` | `step_index` `k`, the operation id, `from_page` the step's detector page id, no phase |
| Input | `EffectIntent`, `EffectCompleted` | unchanged |
| Intermediate page | `RecognitionStarted`, `RecognitionCompleted` | the intermediate page as the only candidate; it is seen when it is the matched page |
| Attempt end | `StepFinished` | the next step's page, the step's own page (a swallowed input) or `<unrecognized>` |
| Waits | the task timing boundaries `PostInputWait`, `PostconditionWait`, `PageRecognitionWait`, `RetryWait`, `CapturePage`; `limit_ms` of a timing failure | existing values only |
| End | `Finalizing`, `TerminalCommitted` | the final page is the target page; `executed_steps` the operation count on success, the dispatched operations on failure |
| Scheduling outcome | the host's existing rule on the last attempt's `StepFinished` of the last operation | unchanged |

Candidate pages are never duplicated, since the admission rules keep an operation's own page,
its intermediate page and its destination distinct. Phase evidence is not used.

The ledger cannot tell that a run was linear, which window it declared or that it declared an
intermediate page: the package reference of `PackageAdmitted` names the content that carries
them. Operation ids and page names are conventions, not types.

## Older builds

A build without this mode refuses a linear package before `PackageAdmitted`: the control's
`execution_mode` is `contained_task_control_invalid`, and an operation's `transition` is
first refused by the resource declaration check (`resource_declaration_invalid`, reason
`UnknownField`).

## Tools

The offline simulation runs the same interpreter: its first decision of a linear package is
the first operation's click, or a refusal. `package build --execution-mode linear_steps`,
`lab run` and the Lab capability listing accept the mode; a built package is admitted by the
rules above. `lab run` passes `transition` through unread.

## What a linear task cannot express

Branches, occasional popups and loops of variable length: only the declared path runs. A run
that does not start on the first step's page, including one already on the target page,
fails with `contained_task_linear_entry_unmatched`, so a path whose first page does not always
appear is not suited to a fixed-interval schedule. An intermediate state that only sometimes
appears is declared as a window, or not at all.
