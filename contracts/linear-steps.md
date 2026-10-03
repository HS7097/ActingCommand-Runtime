# Linear steps

This document is the `linear_steps` execution mode of contained task packages
([Workflow #336](https://github.com/HS7097/ActingCommand-Workflow/issues/336), rulings R12 and
R13). A linear task runs the one path it declares, step by step: each step first recognizes
its own page, then clicks once, and the click leads to the next step. A step may instead
launch, restart or stop the instance's assigned application (rulings R24 and R25, "Application
steps" below). A step whose page does not always appear may be declared optional and is then
skipped when its page does not appear ([Workflow #339](https://github.com/HS7097/ActingCommand-Workflow/issues/339),
"Optional steps" below). Packages that do not declare the mode keep their page-graph execution
unchanged. No game-specific values are part of this contract.

A Lab recording generates such packages (`record stop`, [Lab recording](lab-recording.md)).
How a scheduled linear task's failures are rerun, pause it and are lifted by a package update
is [Policy suspension](policy-suspension.md); the content containers a package may come in
(directory, ZIP, single JSON) are [Package references](package-reference.md), "Containers".

## Declaration

The mode is declared only in `control.json`:

```json
{"schema_version": "Lab-1y.control.v2", "execution_mode": "linear_steps", "max_steps": 2, "...": "..."}
```

`task.json` has no mode field. Each operation may carry two new fields, `transition` and
`optional`, which are admitted only under `linear_steps`:

```json
"transition": {"kind": "page", "page_id": "transition_01", "timeout_ms": 8000, "interval_ms": 50}
"transition": {"kind": "window", "min_ms": 1000, "max_ms": 3000}
"optional": {"settle_ms": 2000}
```

`control.json` may also name a prerequisite package, `"prerequisite_package_id": "<package id>"`,
admitted only under `linear_steps` ("Prerequisite packages" below).

Omitting `transition` (or writing `null`) declares no intermediate state. The resource
declaration check admits the field's shape (`kind` is `page` or `window`; `page_id` a string;
`timeout_ms`, `interval_ms`, `min_ms` and `max_ms` unsigned integers; no other field) and
reports a wrong shape with the existing reasons `UnknownField`, `MissingField`,
`InvalidType` and `InvalidValue`. The values are checked at Runtime admission (below).

Omitting `optional` (or writing `null`) declares a required step. `optional` is an object
with exactly one field, `settle_ms`, a required unsigned integer; `"optional": true` is not
admitted, so that the settle is always chosen. The resource declaration check reports a value
that is not an object or a `settle_ms` that is not an unsigned integer as `InvalidType`, a
missing `settle_ms` as `MissingField` and any other field as `UnknownField`. This operation
field is unrelated to the `optional` targets of a page rule (`page_rules.<page>.optional`).

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
| each operation's effect is `click` or `application`; no `select` or `on_error` | `operation_effect` |
| each operation's `to` / `expect_after` names exactly one page | `destination` |
| each operation's destination differs from its `from` | `to_equals_from` |
| `entry_page` declared and equal to the first operation's `from` (both `"any"` for an application entry) | `entry_page` |
| each operation's destination equals the next operation's `from` | `chain` |
| `target_page` names one page, equal to the last operation's destination | `target_page` |
| a `page` transition names a page other than its operation's `from` and destination | `transition_page` |
| a `page` transition's `timeout_ms` is in `1..=1800000` | `transition_timeout` |
| a `page` transition's `interval_ms` is in `1..=5000` | `transition_interval` |
| a `window` has `0 <= min_ms <= max_ms`, `1 <= max_ms <= 1800000`, and `min_ms` below the task timeout | `transition_window` |
| a `scheduling_outcome`, when declared, has no designated operation | `designated_operation` |
| every terminal page of that outcome is the target page | `terminal_page` |
| the application rules of "Application steps" below | `application_retry`, `any_from`, `any_requires_application`, `input_after_application_stop`, `application_without_home` |
| an application entry declares no `prerequisite_package_id` ("Prerequisite packages" below) | `prerequisite_with_application_entry` |
| the optional-step rules of "Optional steps" below | `optional_first_step`, `optional_application`, `optional_restart_segment_end`, `optional_settle`, `optional_candidates`, `transition_page` |

Every operation still passes the existing operation checks (guard or trusted coordinate, click
shape, retry fields, `post_delay_ms`); operation ids are unique (`contained_task_program_invalid`).
Every page above except an application entry's `any` is resolved to exactly one detector page
id (`<game>/<page>`, with or without the game prefix in the declaration); a page that resolves
to none or to several is the
existing `contained_task_page_set_invalid`. All comparisons use the resolved ids. A
`transition` on an operation of any other mode is `contained_task_operation_invalid`, and so
is an `optional` (detail `operation=<id> optional requires linear_steps`). The
page-graph checks (scheduling outcome coverage, phases) are not run, and a linear task has no
required home entry page, so the host runs no home entry preflight for it; a task with a
prerequisite package runs through the prerequisite gate instead ("Prerequisite packages" below).

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
An application entry waits for nothing ("Application steps" below). Then, for each click
operation dispatched, with `step_index` `d` the number of operations dispatched before it
(counted from 0, as the page-graph path counts its steps):

1. The run progress becomes `d + 1`, and `StepStarted` names the operation and the step's page.
2. The guard is evaluated on the frame on which the step's page passed; a refusal is the
   existing `contained_task_guard_refused`, with no input and no retry.
3. The effect intent, the input bound to that frame's input context and the effect completion
   follow, in the order of the page-graph path.
4. The post-input wait is `post_delay_ms`, or the window's `min_ms` when larger.
5. The intermediate state, then the next step's page, are awaited (below).
6. `StepFinished` names the page that passed (the next step's page, or a page of a run of
   optional steps), and that frame is the next dispatched operation's decision frame.

When no operation follows, the run finishes with the target page and `executed_steps` equal
to the number of operations dispatched. A package without optional steps dispatches every
operation in order, so its `step_index` is the operation index and its `executed_steps` the
operation count. The next operation is always the one the passing page selects (below), never
the result of a comparison with the target page: a path that passes the target page before its
end runs on to its last operation.

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
operation is the first page or pages awaited after its input: its intermediate page, or the
next step's page (every candidate of "Optional steps" below).

- Once an attempt has seen its intermediate page, a next step's page that does not follow
  fails the task (`page_confirmation_failed`, `intermediate_seen=true`), with no retry
  decision and no further input. Likewise once an attempt has seen the skip target of a run of
  optional steps (`skip_target_seen=true`, below): the input took effect.
- Any other failed attempt with attempts left waits `retry_interval_ms` and then waits for
  `step_timeout_ms` for either the gate or the step's own page, the gate first:
  - the gate passes: the input took effect late; the attempt continues without another input
    (an intermediate gate then counts as seen; a skip target with a settle is watched from
    that frame on, as below, for its settle plus the arrival budget);
  - the step's own page passes: the input was swallowed; `StepFinished` names the step's page
    and the next attempt starts on that frame with another `StepStarted` of the same
    `step_index`;
  - neither passes: `StepFinished` names `<unrecognized>` and the task fails.
- With no attempt left, `StepFinished` names `<unrecognized>` and the task fails.

`executed_steps` counts dispatched operations: a retry adds none.

## Optional steps

Workflow #339. An operation declaring `optional` may find its page absent: the whole operation,
its recognition, its click and its intermediate state, is then skipped and writes nothing.

**Runs.** A run is a maximal sequence of consecutive optional operations `O_a .. O_b`; their
pages (the `from` of each) are the optional pages `o_a .. o_b`. The operation before a run is
required. The skip target `N` of the run is the destination of `O_b`: the page of the
required operation after the run, or the target page. The recorded path is still one chain;
optional only changes how an arrival is read.

**Candidates.** The pages an input may reach are its candidates, evaluated in this order:

- after the operation before a run: `o_a .. o_b` in path order, then `N`;
- after an optional operation `O_i` of the run: the optional pages of the run not yet run,
  other than `o_i`, in path order, then `N`;
- after any other operation: the next step's page, as before.

On a frame where several candidates pass, the first one wins, so an optional page wins over
`N` (a dimmed popup over the next page). The page that passes selects the next operation: an
optional page runs its operation next; `N` skips the rest of the run and continues after it,
or finishes the run when the run ends the path. Within a run the order is free and every
optional operation runs at most once per run; a page outside the candidates is never
recognized, no candidate lies past `N`, and nothing before the operation that precedes the run
is awaited again.

**Settle.** The settle `S` of candidates is the largest `settle_ms` of their optional pages
(0 when they have none). An optional page that passes is reached at once. `N` is reached only
on a frame whose capture started at least `S` after the capture on which `N` first passed, so
that an optional page appearing within `S` of `N` is still run; frames on which no candidate
passes are ignored. The wait gives up after the arrival budget `T` (as in "Intermediate
states"), or, once `N` was seen, `S + T` after the capture on which `N` first passed. The
settle only shortens the sleeps while it lasts; after it, captures keep the interval and never
run back to back. With one candidate, the wait is the wait above, unchanged. When the retry
decision's frame shows `N` (below), the settle counts from the moment that frame has been
evaluated rather than from its capture start, so on that path the settle may last longer by
one capture and its recognition; this is conservative and never judges "no popup" early.

**Guard and retries.** An optional operation's guard is evaluated on the frame on which its
page passed; the operation after `N` is guarded on the frame that ended the settle. An optional
operation may declare an intermediate state and a retry, with the rules above; a close-popup
step is best declared with `max_attempts` 2: when its click is swallowed, `N` does not pass,
the attempt fails and the retry decision finds the popup's page again. The retry decision
awaits every candidate, then the operation's own page (with an intermediate page: that page,
then the own page). An attempt that saw `N` and then neither `N` nor an optional page within
`S + T` fails with no retry decision.

**Admission** (`contained_task_linear_invalid`, `operation=<id>`):

| Rule | `reason` |
|---|---|
| the first operation is not optional (the entry gate checks only the first step's page; a step 2 may be optional, also after an application entry) | `optional_first_step` |
| an application operation is not optional (it would be a conditional restart) | `optional_application` |
| after the last `launch` or `restart`, the first page that is the main interface ("Application steps") is not the page of an optional operation | `optional_restart_segment_end` |
| `settle_ms` is in `0..=60000` | `optional_settle` |
| for each run, the optional pages, `N` and the page of the operation before the run (none for an application entry) are distinct, so that no candidate or retry-decision list repeats a page | `optional_candidates` |
| a `page` transition of the operation before a run or of a run member is none of the run's optional pages and `N` | `transition_page` |

The last operation may be optional. `N` is reached on every path, so the static rule after a
launch or restart still guarantees the main interface. Each wait evaluates every candidate on
every frame: the cost of a frame grows with the length of a run, which has no limit of its own
beyond 1000 operations and the 1024 candidate pages of a recognition.

**The skip target must be popup-sensitive.** `N` must not pass on a frame of an optional page:
a hand-written package gives `N`'s page rule a target that fails under the popup (for example
a color in an area the popup dims or covers) or declares the popup as a `forbidden` target of
`N`. Otherwise, when a close click is swallowed, an unrecorded popup appears or a popup is late
beyond its settle, `N` passes under the popup and the run goes on as if no popup had appeared.

## Application steps

Workflow #336 R24 and R25. An operation's one effect may be the `application` effect of
[Application lifecycle](application-lifecycle.md) instead of a click,
`"application": {"action": "launch" | "restart" | "stop"}`, acting on the application the
instance is assigned; the package never names an application. Only the first operation may
start from any screen, the application entry:

```json
"entry_page": "any",
"operations": [{"id": "step_01_app", "from": "any", "to": "home",
  "application": {"action": "restart"},
  "expect_after": {"page_id": "home", "timeout_ms": 90000}, "post_delay_ms": 1000}]
```

**Admission** (`contained_task_linear_invalid`, in addition to the rules above; the existing
operation checks already refuse a `guard` or `unguarded_trusted_coordinate` on an application
effect with `contained_task_operation_invalid`):

| Rule | `reason` |
|---|---|
| an application operation declares none of `retryable`, `max_attempts`, `retry_interval_ms` | `application_retry` |
| `from: "any"` appears only on the first operation | `any_from` |
| a first operation `from: "any"` is an application operation | `any_requires_application` |
| after a `stop`, the next operation, if any, is a `launch` or `restart` | `input_after_application_stop` |
| after the last `launch` or `restart`, some step's page is the main interface (below) | `application_without_home` |

**Main interface.** A page of a linear package is the main interface when its canonical anchor
(the page id without its `<game>/` prefix) is `home`, or is `step_<digits>_home` exactly (for
example `step_03_home`, the page Lab records for `--page home`; `step_03_homepage` is not one).
Only `linear_steps` packages use this predicate (Workflow #336 R25, ruling 5961093808); the
page-graph home entry, the scheduler and the other home checks keep the literal `home`.

The application entry is not resolved to a detector page and is not compared with its
destination; a `page` transition after it only has to differ from its destination. An
application operation may declare a `transition`, checked as for a click. Writing
`expect_after.timeout_ms` on an application operation is recommended: a cold start often
outlasts the control `step_timeout_ms`, which is at most 60000.

**Capability.** The existing check `application_effect_requires_assigned_application`
(`invalid_request`, denied) runs after `RunStarted` and before any capture for linear packages
exactly as for page-graph packages: an instance without an assigned application (a fixture
instance) and the offline simulation refuse a package with an application operation before
any frame.

**Execution** of an application operation dispatched as `step_index` `d`:

1. The run progress becomes `d + 1`, and `StepStarted` names the step's page, or
   `<unrecognized>` for the application entry (the literal the page-graph path writes for an
   unrecognized initial frame).
2. No guard, no effect intent, no foreground gate. After the task deadline check, the run's
   application lifecycle path records `application.intent`, then `application.completed`,
   followed by `EffectCompleted`. An `application.failed` ends the step and the task, with no
   `EffectCompleted`.
3. The post-input wait, the intermediate state and the next step's page (or the candidates of
   a run of optional steps after it) follow as for a click.
4. A candidate passes: `StepFinished` names it, and that frame is the next dispatched
   operation's decision frame. Otherwise `StepFinished` names `<unrecognized>` and the task
   fails with `contained_task_linear_application_unconfirmed`.

The application entry captures and recognizes nothing before its effect: the ruling puts no
recognition before it, a failing capture must not block a restart that may repair it, and the
effect clears the committed input frames anyway. The run's first capture is the arrival wait
after the effect. An application step is never retried and has no retry decision: the
decision compares the gate with the step's own page, which an application entry does not
have, and a failed cold start (a slow start, a forced update, a crash) is environmental; the
next dispatch runs the package again as its own run. After a `stop` the assigned application
is not in the foreground and the foreground gate refuses every pointer input, so only a
`launch` or `restart` may follow it; the first click after an application step passes the
foreground gate, which records `application.foreground`.

**First frame after a restart.** When an application-entry package runs as the task of its
own run (a startup package, a recovery ladder rung), the run's first capture follows the
restart, so the host's first-capture rule (`application-lifecycle.md`, #316-P4) applies to
that frame. A page-graph application package takes its initial frame before the restart, and
the frame after it is not the first.

**Severity.** An adb failure of the effect is `application_backend_operation_failed` on the
host's `application.failed` chain. The device error is fatal, so the task's terminal severity
is `fatal`, as for an adb failure of a click. The settlement of a scheduled linear task treats
this code and `input_backend_operation_failed` as recoverable when the run was not poisoned:
it is only run again and never pauses the task (R25-2, `policy-suspension.md`, "Which failures
accumulate"); a poisoning failure, such as an unconfirmed resource close, still pauses it at
the first failure. The terminal and its severity are unchanged.

## Prerequisite packages

Workflow #336 R14 and R15. A linear package records its path from its first step; a package
that should also run from elsewhere names, in `control.json`, the package that brings the screen
to that first step:

```json
{"schema_version": "Lab-1y.control.v2", "package_id": "neutral.test.stage_select",
 "execution_mode": "linear_steps", "prerequisite_package_id": "neutral.test.stage_page", "...": "..."}
```

**Declaration.** `prerequisite_package_id` is a string, not empty, at most 256 bytes, with no
control character, other than the package's own `package_id`, and only under `linear_steps`.
A violation is `contained_task_control_invalid` with detail `reason=prerequisite_id_invalid`,
`reason=prerequisite_self` or `reason=prerequisite_requires_linear_steps`; an application entry
that declares one is `contained_task_linear_invalid`, `reason=prerequisite_with_application_entry`
(the entry recognizes nothing before its effect, so there is no first step to lead to). Only a
hand-written `control.json` or `record stop --requires` (`lab-recording.md`) carries the field: `package build`
writes a fixed set of control fields, and `lab run` ignores the field and never runs a
prerequisite package.

**Resolution.** The package id names an entry of the `actingd` configuration's top-level
`prerequisite_packages` (package id, package locator, content reference; see
`actingd-check-config.md`, "Prerequisite packages"); nothing scans a package directory. The
chain is resolved and every package admitted when the run is prepared: before any lease for a
direct (`task-run`) run and for a startup package or a recovery ladder rung, inside the lease
the policy already holds for a scheduled run. A `linear_steps` prerequisite package may name its
own prerequisite package; a page-graph package ends the chain. Layer by layer, in this order:

| Check | `failure_code` |
|---|---|
| the map has the package id | `contained_task_prerequisite_unbound` |
| the package id is not already in the chain (the dependent package included) | `contained_task_prerequisite_cycle` |
| at most three prerequisite packages besides the dependent package | `contained_task_prerequisite_depth_exceeded` |
| the package is admitted against its content reference (the loader's code is attached as the related failure, a resource declaration rejection travels with it) | `contained_task_prerequisite_admission_failed` |
| the admitted package's `package_id` is the map key | `contained_task_prerequisite_mismatch` |
| the package is no `recognize_only` package and declares no stability termination, post-admission OCR, OCR fields, resource readings or designated scheduling operation; its game, server and resolution are those of the package that names it | `contained_task_prerequisite_incompatible`, detail `reason=<recognize_only\|stability_termination\|post_admission_ocr\|resource_readings\|designated_operation\|game\|server\|resolution>` |
| the maximum steps of the whole chain and the dependent package add up to at most 1000 | `contained_task_prerequisite_step_limit` |

Each refusal is denied (`package_invalid`) with detail `layer=<n> package_id=<id>` (plus
`declared_package_id=<id>` for a mismatch and `reason=<reason>` for an incompatible package; the
step limit's detail is `maximum_executed_steps=<n>`), writes no `task.*` record and has no task terminal, so it never starts the stuck-recovery ladder; a
scheduled run's refusal is recorded with the policy's lease released, as any other preparation
refusal. The admission deadline of a prerequisite package is the run's deadline (for a scheduled
run, the one the run derives from its request and lease). A prerequisite package's
`scheduling_outcome` without a designated operation is allowed and ignored. A request's
`--recovery-package` binding plays no part: it still goes to the stuck-recovery ladder only. The
map is read when `actingd` starts; a change needs a restart.

**Gate.** A package whose chain is not empty runs through the entry gate instead of starting
directly; a package whose prerequisite chain is empty starts as above. Layer `i` is a package `X_i`
(`X_0` the dependent package) and its prerequisite package `X_(i+1)`:

1. **First check, one frame.** One capture is evaluated for `X_i`'s first step's page only
   (diagnostic phase `home_preflight`, no task timing). `EntryRecognition { Initial }` names that
   page, followed by `EntryRecoveryDecision`. When the page passes, the layer is done and
   `X_(i+1)` does not run.
2. **Prerequisite package.** `EntryRecoveryPackageAdmitted { X_(i+1) }`; when `X_(i+1)` has a
   prerequisite package itself, its own layer runs first. Then `EffectiveConfigurationFacts::
   EntryRecovery { X_(i+1) }` (when the run recorded its initial configuration) and the package
   runs inside the same run: same task, run and lease, its step indices following the steps
   already run, its budget origin `entry_recovery`, its `PackageAdmitted`, `RunStarted` and
   `Finalizing` not written. Its own failure code is carried out unchanged (a page-graph
   package's `contained_task_page_unknown` still starts the ladder). It ends with
   `EntryRecoveryCompleted { X_(i+1), final page, its own executed steps }`; its final page is not
   compared with `X_i`'s pages, since page ids of different packages are not comparable.
3. **Recheck, bounded wait.** `X_i`'s first step's page is evaluated on one frame at a time,
   every capture interval of `X_i`, for at most `X_i`'s `step_timeout_ms`, since its markers may
   still be fading in when the prerequisite package reached its own end. Each capture is a
   `CapturePage` boundary and each sleep a `PageRecognitionWait` boundary of the gate's own
   budget (origin `task` for `X_0`, `entry_recovery` above), in the preflight phase.
   `EntryRecognition { PostRecovery }` follows; when the page did not pass, the run fails with
   `contained_task_prerequisite_entry_unmatched`, detail `layer=<i> package_id=<X_i>
   required_page=<page>`, timing `page_recognition` / `entry_recognition` with `limit_ms` the
   step timeout.

Each prerequisite package runs at most once per run. On any failure every prerequisite package
opened and not yet closed gets one `EntryRecoveryFailed` with the failure code, innermost first,
and the gate one `EntryTargetDisposition { FailClosed }`; a recognition failure, an unknown page
or an input backend failure is recorded so even when the run spent its last execution budget,
as the home entry recovery does. The one exception is a capture or recognition error of the
outermost first check, before the gate wrote any fact: it is returned directly, as the existing
home entry preflight returns its first check's error; every later failure, the outermost recheck
included, ends with the one `FailClosed`. When every layer passed,
`EntryTargetDisposition { Started }` is written, then the dependent package's `PackageAdmitted`,
and the package runs with its step indices after the prerequisite packages' steps; on success
`executed_steps` adds the prerequisite packages' steps. Its own first step is awaited again as
above and normally passes at once. The host deadline still ends the run with `contained_task_deadline_exceeded` (or
cancelled, paused) from any capture of the gate.

**Ledger.** Only the existing `TaskEntryPreflight` facts and effective configuration records
are written; a run has at most seven effective configuration records (initial, one per
prerequisite package, the return-home package of "Return-home fallback" below included,
capture, input). The facts are told apart by these rules, with no layer
field:

- A `TaskEntryPreflight` fact belongs to the innermost prerequisite package that was opened by
  `EntryRecoveryPackageAdmitted` and not yet closed by `EntryRecoveryCompleted` or
  `EntryRecoveryFailed`; with none open it belongs to the dependent package. An
  `EntryRecognition`'s `required_page` is that package's first step's page.
- The packages of one chain have different package ids, so their content references differ;
  each run prerequisite package has its own `EntryRecovery { package reference }` record.
- Every package's step indices are a range of their own, as long as its
  `EntryRecoveryCompleted.executed_steps`.

A replayed direct request is answered from its ledger when its `EntryRecoveryPackageAdmitted`
facts are at most four (three declared, one return-home), all different, each the request's recovery binding or a package of the
`prerequisite_packages` map; otherwise it is `contained_task_request_recovery_reused`.

**Failure codes of the gate.**

| Situation | `failure_code` | Starts the ladder |
|---|---|---|
| After the prerequisite package ran, the first step's page did not pass within `step_timeout_ms` | `contained_task_prerequisite_entry_unmatched` | no |
| The prerequisite package ended without a final page (a safeguard) | `contained_task_prerequisite_final_page_missing` | no |
| The steps run would exceed 1000 (a safeguard; preparation already checks) | `contained_task_prerequisite_step_limit` | no |
| The prerequisite package failed | its own code | as that code does |
| A recognition error of the gate | `contained_task_recognition_failed` | no |
| A refusal at preparation (table above) | `contained_task_prerequisite_*` | no |

No `contained_task_prerequisite_*` code starts the stuck-recovery ladder: the prerequisite
package already reached its own end, so the game responds, and a first step that still does not
pass points at a stale marker or a wrong declaration, which a restart does not repair.

**What the gate cannot do.** A first step marked only by templates may pass on a frame dimmed by
an overlay (a notice over the main interface), so the prerequisite package would not run: give
the first step a marker that a dimmed frame does not pass, such as a color in a fixed bright
area. A linear chain end does not accept a start that is already on its target page; a
page-graph chain end succeeds with no step on its target page, so it is the better end of a
chain. A flow whose first page appears only sometimes stays a page-graph package.

## Return-home fallback

Workflow #336 R16. A `linear_steps` package that declares no `prerequisite_package_id` and
whose first step is a page (no application entry) falls back to the return-home package that
the `actingd` configuration's top-level `return_home_packages` names for its game and server
(`actingd-check-config.md`, "Prerequisite packages"). That package id is a key of the
`prerequisite_packages` map and is resolved through it as a declared one is.

**Resolution.** The return-home package is one more layer at the end of the chain, below the
dependent package when it declares nothing, or below the last declared prerequisite package when
that is a `linear_steps` package that declares nothing. It is looked up by the game and server of
that layer's control and passes the same checks as a declared layer (the table of "Prerequisite
packages"); a refusal has the same code, with `source=return_home` at the end of its detail. The
return-home layer is not one of the three declared layers, so a chain has at most four; it falls
back no further, and there is none when the map has no entry for the game and server or its
package is already in the chain. A page-graph package and an application entry
(`from: "any"`) never fall back. The maximum steps of the whole chain, the return-home package
included, still add up to at most 1000. Which layer came from the configuration is known only
in memory: the ledger does not tell a return-home layer from a declared one, except by comparing
its package reference with the configuration of the time.

**Gate.** The return-home layer runs through the gate as a declared layer does: first check,
`EntryRecoveryPackageAdmitted`, its run, recheck, close. The one difference: when the first
step's page still does not pass within `step_timeout_ms` after the return-home package ran, the
run fails with `contained_task_return_home_entry_unmatched`, detail `layer=<i>
package_id=<X_i> required_page=<page> return_home=<return-home package id>`, with the timing of
`contained_task_prerequisite_entry_unmatched`. A package with neither a declared prerequisite
package nor a fallback starts directly, as before, and fails with
`contained_task_linear_entry_unmatched` when the run does not start on its first step.

So the first step of a package that relies on the fallback should be the page where the
return-home package ends (the main interface). A package recorded from another screen declares
a prerequisite package that leads to its first step; otherwise every run from elsewhere fails
with `contained_task_return_home_entry_unmatched`.

| Situation | `failure_code` | Starts the ladder |
|---|---|---|
| After the return-home package ran, the first step's page did not pass within `step_timeout_ms` | `contained_task_return_home_entry_unmatched` | no |
| The return-home package failed | its own code (a page-graph package's `contained_task_page_unknown`, for example) | as that code does |
| The return-home package is refused at preparation | the `contained_task_prerequisite_*` code of the refusal, detail ending in `source=return_home` | no |

The code is kept apart from `contained_task_prerequisite_entry_unmatched` so that a stale
return-home package, maintained with the program, is told from a prerequisite package the
author declared; neither starts the stuck-recovery ladder. A request's recovery binding plays
no part in the resolution. A linear package run by the ladder's return-home rung falls back like
any linear run, and so does a linear startup package.

**Ladder and page-graph home entry (Workflow #336 L2d, R23).** A run whose request binds no
recovery package uses the return-home package of its package's game and server in two more
places. The stuck-recovery ladder's `return_home` rung runs it, with the maximum response
deadline (`contracts/emulator-control.md`, "Stuck-recovery ladder"). The page-graph home entry
admits it where it reported `contained_task_home_recovery_binding_missing` before: only after
the first check did not pass, with the existing failure handling and facts, and it runs as a
prerequisite package does (its `scheduling_outcome` without a designated operation allowed and
ignored). On both paths the package passes, once admitted, the checks a return-home chain layer
passes (the table of "Prerequisite packages": no `recognize_only`, stability termination,
post-admission OCR, OCR fields, resource readings or designated operation; the game, server and
resolution of the failed or entering package), and it declares no `prerequisite_package_id` of
its own, which neither path runs. Otherwise it is refused before it runs with
`contained_task_prerequisite_incompatible`, detail `package_id=<id> reason=<reason>
source=return_home` (the reasons of that table, or `return_home_declares_prerequisite`), which
starts no ladder: on the rung before any lease, recorded as the rung's failure (`runtime.failed`,
category `recovery_ladder`, the detail as native detail); in the home entry with no
`EntryRecoveryPackageAdmitted`, as `EntryTargetDisposition { FailClosed }` and the
task terminal, the detail on a runtime lifecycle failure record after the terminal as under
"Failure detail" below. In the home entry, a page-graph package's final page must still be the
home page, while a `linear_steps` package's final page (a Lab page id such as
`<game>/step_03_home`) is not compared, and only the recheck decides. With a request binding both
places behave exactly as before; with neither, the rung is skipped (`no_recovery_package`) and
the home entry fails with `contained_task_home_recovery_binding_missing`.

**Failure detail.** When the package a run executes is a `linear_steps` package, the kernel
detail of its task failure is always the native detail of a runtime lifecycle failure record
(the existing `runtime.failed` with its lifecycle part) written right after the task terminal
and naming it, as an outcome with an extra native detail is recorded; at most 1024 bytes. The
record has the terminal's severity (`warning` for a scheduled run, `error` for a direct run), so
the failure is not counted again at a higher severity. The receipt does not carry it. For example `operation=<id> attempts=<n> after_page=<page>
hit_error_page=<bool>` of `page_confirmation_failed`, or the gate's own details above. A
failure without a detail is recorded by the terminal alone, as before. The gate
carries a prerequisite or return-home package's own failure out as its code, with its detail
only for a recognition failure or an unknown page ("Prerequisite packages" above). A page-graph
package's failure is recorded as before.

## Failure codes

| Situation | `failure_code` | Timing (existing values) |
|---|---|---|
| The first step's page does not pass within `step_timeout_ms` | `contained_task_linear_entry_unmatched` | `page_recognition` / `entry_recognition` |
| The intermediate page is not seen within its timeout, with no attempt left | `contained_task_linear_intermediate_unobserved` | `postcondition` / `postcondition` |
| The next step's page does not pass in its budget (with no attempt left), the retry decision sees neither page, the next step's page does not follow a seen intermediate page, or no candidate passes within `S + T` after a seen skip target | `page_confirmation_failed`, detail `transition=none\|page\|window ...` (`intermediate_seen=true` after a seen intermediate page) | `postcondition` / `postcondition`, `limit_ms` the spent budget |
| After an application step, the intermediate page is not seen, the next step's page does not pass in its budget, or it does not follow a seen intermediate page | `contained_task_linear_application_unconfirmed`, detail `operation=<id> application=<action> attempts=1 transition=none\|page\|window ... intermediate_seen=<bool>` | `postcondition` / `postcondition`, `limit_ms` the spent budget |
| An instance without an assigned application, including the offline simulation | `application_effect_requires_assigned_application` (`invalid_request`, denied), before any capture | none |
| The adb command of an application effect fails | `application_backend_operation_failed` | none |
| A guard refusal | `contained_task_guard_refused` | none |
| The task deadline | `contained_task_timeout` | `task` / the stage it expired in |
| A recognition error | `contained_task_recognition_failed` | none |
| A frame of another size | `contained_task_frame_resolution_mismatch` | none |
| A package outside the admission rules | `contained_task_linear_invalid` (admission) | none |
| The prerequisite gate ("Prerequisite packages" above) | `contained_task_prerequisite_*` | `page_recognition` / `entry_recognition` for `contained_task_prerequisite_entry_unmatched` |
| The return-home package ran and the first step's page did not pass ("Return-home fallback" above) | `contained_task_return_home_entry_unmatched` | `page_recognition` / `entry_recognition` |

When the operation awaited more than one candidate (a run of optional steps), the detail of
`page_confirmation_failed` and `contained_task_linear_application_unconfirmed` ends with
` awaited=<the candidates' detector page ids, comma-separated>`, followed by
` skip_target_seen=true` when the skip target had passed; with one candidate the detail is
unchanged. Neither key is part of a detail whitelist. An optional step adds no runtime failure
code; its admission refusals are new `reason` values of `contained_task_linear_invalid`. A
known limit: one cause can fail at the operation before a run on a day without a popup and at
the optional step just run on a day with one, so the last `StepStarted` operation and the
detail's `operation=` differ between such runs, and a rule that keys repeated failures by
operation counts them apart; this favours rerunning and hides no failure.

The new codes are strings; none of them starts the stuck-recovery ladder.

A scheduled linear task records its failure as a failure identity built from these codes, the
step and the detail; which of them can pause the task, the immediate rerun and the lift by a
package update are in `policy-suspension.md`.

## Ledger

A linear run writes only existing records with existing fields and enum values: no event
type, payload field, `PackageRef` variant or enum value (`ResourceDeclarationReason`,
`TaskTimingStage`, `TaskTimingScope`, `TaskTimingBoundary`) is added, so a ledger written by a
linear run is read by older builds.

| When | Existing record | Values |
|---|---|---|
| Admission | `PackageAdmitted` | the package reference |
| Each capture | the capture records and their evidence | unchanged |
| Each recognition | `RecognitionStarted`, `RecognitionCompleted` | `candidate_pages` the detector page ids awaited: one or two, or the candidates of a run of optional steps (its optional pages in path order, the skip target last), with the retry decision's own page after them; `matched_page` the passing one, or none (the targets of a frame without a passing page are those of the first candidate, as before) |
| Step start | `StepStarted` | `step_index` the dispatch index `d`, the operation id, `from_page` the step's detector page id (`<unrecognized>` for the application entry), no phase |
| Skipped optional step | none | a skip shows as the preceding `StepFinished` naming the skip target instead of its declared destination, and as no `StepStarted` of that operation in the run |
| Input | `EffectIntent`, `EffectCompleted` | unchanged |
| Application effect | `application.intent`, then `application.completed` followed by `EffectCompleted`, or `application.failed`, which ends the step and the task with no `EffectCompleted` (the `application.*` records are written by the host, with the task and run ids and their own action id); no `EffectIntent` | unchanged |
| Intermediate page | `RecognitionStarted`, `RecognitionCompleted` | the intermediate page as the only candidate; it is seen when it is the matched page |
| Attempt end | `StepFinished` | the page that passed (the next step's page, or a candidate of a run of optional steps), the step's own page (a swallowed input) or `<unrecognized>` |
| Waits | the task timing boundaries `PostInputWait`, `PostconditionWait` (also the sleeps of a settle), `PageRecognitionWait`, `RetryWait`, `CapturePage`; `limit_ms` of a timing failure | existing values only |
| End | `Finalizing`, `TerminalCommitted` | the final page is the target page; `executed_steps` the operations dispatched, on success and on failure (the operation count when the package has no optional step) |
| A failure with a kernel detail | a runtime lifecycle failure record (`runtime.failed`) after the terminal, naming it, at the terminal's severity | the detail as its native detail ("Failure detail" above) |
| Scheduling outcome | the host's existing rule on the last attempt's `StepFinished` of the last dispatched operation, which names the target page | unchanged |

Candidate pages are never duplicated, since the admission rules keep an operation's own page,
its intermediate page, its destination and the candidates of a run distinct. Phase evidence is
not used.

The ledger cannot tell that a run was linear, which window it declared, that it declared an
intermediate page or which steps are optional: the package reference of `PackageAdmitted`
names the content that carries them. Operation ids and page names are conventions, not types. No capture precedes an
application entry, so its step has no pre-input frame evidence; the entry itself shows only as
`from_page` `<unrecognized>` and in the package.

## Older builds

A build without this mode refuses a linear package before `PackageAdmitted`: the control's
`execution_mode` is `contained_task_control_invalid`, and an operation's `transition` is
first refused by the resource declaration check (`resource_declaration_invalid`, reason
`UnknownField`). A build without prerequisite packages refuses a control that declares
`prerequisite_package_id` the same way (`resource_declaration_invalid`, `UnknownField` at
`/prerequisite_package_id`), before `PackageAdmitted` and, for a direct run, before any lease;
its `actingd` refuses a configuration with `prerequisite_packages` (`config_decode_failed`) and
does not start. A build without the return-home fallback refuses a configuration with
`return_home_packages` the same way (`config_decode_failed`). A build without optional steps
(v0.9.0, v0.9.1, and the earlier `linear_steps` builds) refuses an operation's `optional` the
same way, before `PackageAdmitted`: `resource_declaration_invalid`, `UnknownField` at
`/operations/<k>/optional`; so does its `package build`.

## Tools

The offline simulation runs the same interpreter: its first decision of a linear package is
the first operation's click, or a refusal; the first operation is never optional. A package
with an application operation is always
refused, `application_effect_requires_assigned_application` with no capture, provided the frame
list is not empty (an empty list is `offline_fixture_missing` before the interpreter). `package build --execution-mode linear_steps`,
`lab run` and the Lab capability listing accept the mode; a built package is admitted by the
rules above. `lab run` passes `transition` and `optional` through unread, ignores
`prerequisite_package_id` and runs no prerequisite package; the offline simulation runs the
package alone, without the gate. `record stop` (`lab-recording.md`, "Package generation")
generates a linear package from a Lab recording and admits it by these rules, without a vision
provider, before it writes it.

## What a linear task cannot express

Branches and loops of variable length: only the declared path runs, and a run of optional
steps only skips steps of it. A run of a package without a prerequisite package or return-home
fallback that does not start on the first step's page, including one already on the target
page, fails with `contained_task_linear_entry_unmatched`, so a path whose first page does not
always appear is not suited to a fixed-interval schedule; a package with a prerequisite package
runs it first ("Prerequisite packages" above), and one without runs the configured return-home
package first ("Return-home fallback" above). The first step cannot be optional, with or
without a prerequisite package. An intermediate state that only sometimes appears is declared
as a window, or not at all.

Popups that appear only sometimes after a step's click (a daily sign-in, a notice, an update
prompt after a cold start, a reward card) are declared as optional steps, in a free order and
each at most once per run; a popup that may appear several times is recorded as several
optional steps. They cannot cover:

- a popup that appears later than its settle after the next page, more often than it was
  recorded, or that was never recorded: the next step judges or clicks on it, and, with a
  popup-sensitive skip target, the run fails loudly;
- a popup already on the screen before the preceding step's click: it is cleared by a
  prerequisite package, or the path starts earlier;
- a detour that confirms and returns to the same page (`A -> start -> [prompt?] -> confirm ->
  A' -> start -> B`): on a day without the prompt, `A'` passes on the remaining frame of `A`,
  the screen has moved on after the settle and the run fails; such a path is a page-graph
  package or two packages.

An application step always runs: there is no conditional restart, and the package cannot name
an application. A restart package records up to the main interface,
`any -> restart -> title -> [sign-in?] [notice?] -> home`, its optional steps covering the
screens a cold start shows only sometimes; the main interface itself is a required step. A
failure inside such a restart segment is still not counted (R25).
