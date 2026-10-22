// SPDX-License-Identifier: AGPL-3.0-only

// #411 B36 + B19, slice A35-S3 (0.12.x; design `rd-1021/train-0129/a35-design.md` §2.3 and §5.1
// "S3", test rows T1 to T5; coordinator decisions Q9 to Q11 of 2026-10-21; register §1.176; exec
// keys 175 to 179): a page-graph operation may be a wait step without input, and a confirmation
// may declare a wait page.
//
// - T1: a wait step from its wait page; the device shows the wait page, then a destination. The
//   step finishes on the destination with no input and no effect record, and the run goes on.
// - T2: the wait page beyond `max_ms`. With `on_timeout` the run ends Completed on the wait page
//   with that key; without it the run fails `contained_task_step_unconfirmed` with `page_id` the
//   wait page.
// - T3: the wait page, then a page the step does not name: the run fails after the away clock
//   (`expect_after.timeout_ms`, which restarts whenever the wait page shows), not after `max_ms`.
// - T4: a click whose `expect_after` declares a wait page: frames on the wait page beyond the
//   confirmation timeout but within the wait page's `max_ms`, then the destination, confirm the
//   step; the same frames without the wait page leave it unconfirmed.
// - T5: admission refuses a wait step with a click, a guard or a retry member, a wait step in a
//   `linear_steps` task, an `on_timeout` whose mapping does not list the step's page or is not
//   busy, and a wait page that is a destination, each with its code and rule; Lab B-3 S1's
//   admission of a declared operation refuses a wait step.
//
// Each package is a synthetic `neutral` package admitted through `PreparedContainedTask::load`: a
// 2 x 1 frame whose left pixel names the page. The runs play scripted frames on the scripted clock
// (EXEC-I4) at the factor 1000, where the budget clock is the runtime's: the device shows
// `screens[n]` after `n` inputs, each segment's last frame repeating, and every pause moves the
// clock by its length. A frame is delivered at once, so the run's clock is the sum of its pauses.
// The wait step takes a frame every `INTERVAL_MS` and its away clock is `AWAY_MS`.

use super::tests::scripted::{ScriptedFrame, ScriptedRuntime};
use super::*;
use serde_json::{Value, json};
use std::io::{Cursor, Write};
use zip::{ZipWriter, write::FileOptions};

const WIDTH: u32 = 2;
const PAGES: [(&str, [u8; 3]); 4] = [
    ("page_a", [255, 0, 0]),
    ("page_b", [0, 255, 0]),
    ("page_c", [0, 0, 255]),
    ("page_d", [255, 255, 0]),
];
const STEP_UNCONFIRMED_CODE: &str = "contained_task_step_unconfirmed";
const PROGRAM_INVALID: &str = "contained_task_program_invalid";
const LINEAR_INVALID: &str = "contained_task_linear_invalid";
const TASK_FILE: &str = "resources/operations/task/task.json";
// The wait step's `expect_after.interval_ms` and `expect_after.timeout_ms` (its away clock).
const INTERVAL_MS: u64 = 1_000;
const AWAY_MS: u64 = 3_000;

fn colour(page: &str) -> [u8; 3] {
    PAGES
        .iter()
        .find(|(name, _)| *name == page)
        .map(|(_, colour)| *colour)
        .unwrap_or_else(|| panic!("unknown fixture page {page}"))
}

// The frame of `page`: its colour on the left pixel, black on the right.
fn screen(page: &str) -> ScriptedFrame {
    ScriptedFrame::rgb(WIDTH, 1, &[colour(page), [0, 0, 0]])
}

// A device that shows the frames `segments[n]` after `n` inputs, one per capture.
fn device(segments: &[&[&str]]) -> ScriptedRuntime {
    ScriptedRuntime::new(
        segments
            .iter()
            .map(|pages| pages.iter().map(|page| screen(page)).collect())
            .collect(),
    )
}

fn with(mut operation: Value, extra: Value) -> Value {
    for (member, value) in extra.as_object().expect("extra members") {
        operation[member] = value.clone();
    }
    operation
}

// `op_wait`, a wait step from `page_a` that waits for `page_b`, its `wait` being `wait`, with
// `extra` members.
fn wait_step(wait: Value, extra: Value) -> Value {
    with(
        json!({
            "id": "op_wait",
            "from": "page_a",
            "wait": wait,
            "expect_after": {"page_id": "page_b", "timeout_ms": AWAY_MS, "interval_ms": INTERVAL_MS}
        }),
        extra,
    )
}

// `op_next`, a trusted tap from `page_b` to `page_c`.
fn next() -> Value {
    json!({
        "id": "op_next", "from": "page_b", "to": "page_c",
        "click": {"kind": "point", "x": 1, "y": 0},
        "unguarded_trusted_coordinate": true, "retryable": false
    })
}

// `op_tap`, a trusted tap from `page_a` confirmed on `page_b` within 2000 ms, a frame every
// 500 ms, its `expect_after` with `extra` members.
fn tap(extra: Value) -> Value {
    json!({
        "id": "op_tap", "from": "page_a",
        "click": {"kind": "point", "x": 1, "y": 0},
        "unguarded_trusted_coordinate": true, "retryable": false,
        "expect_after": with(
            json!({"page_id": "page_b", "timeout_ms": 2_000, "interval_ms": 500}),
            extra
        )
    })
}

// A mapping of `page` to `key` under no designated effect, `busy` when `busy`.
fn mapping(key: &str, page: &str, busy: bool) -> Value {
    let mapping =
        json!({"outcome_key": key, "effect": "no_designated_effect", "terminal_pages": [page]});
    if busy {
        with(mapping, json!({"busy": true}))
    } else {
        mapping
    }
}

// A page-graph task from `page_a` to `target`, with its scheduling outcome's `mappings` when
// there are any.
fn task(operations: Vec<Value>, target: &str, mappings: Vec<Value>) -> Value {
    let mut task = json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": WIDTH, "height": 1},
        "entry_page": "page_a",
        "target_page": target,
        "operations": operations
    });
    if !mappings.is_empty() {
        task["scheduling_outcome"] = json!({"mappings": mappings});
    }
    task
}

fn bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).expect("JSON bytes")
}

fn zip_entries(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, content) in entries {
        zip.start_file(*path, options).expect("zip entry");
        zip.write_all(content).expect("zip bytes");
    }
    zip.finish().expect("finish zip").into_inner()
}

// The package of `task` under the execution mode `mode`: a `Lab-1y.control.v2` control taking a
// frame every 1000 ms with a step timeout of 3000 ms within 30000 ms, and a recognition pack and
// page set that tell the fixture pages by their colour.
fn package(task: &Value, mode: &str) -> Vec<u8> {
    let mut control = json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": "neutral.wait.task",
        "execution_mode": mode,
        "game": "neutral",
        "server": "test",
        "resolution": {"width": WIDTH, "height": 1},
        "entry_task_id": "task",
        "capture_interval_ms": 1_000,
        "step_timeout_ms": 3_000,
        "timeout_ms": 30_000
    });
    if let Some(max_steps) = task.get("max_steps") {
        control["max_steps"] = max_steps.clone();
    }
    let targets = PAGES
        .iter()
        .map(|(page, colour)| {
            json!({"type": "color", "id": format!("page/{page}"),
                   "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": colour})
        })
        .collect::<Vec<_>>();
    let pack = json!({
        "schema_version": "0.7",
        "game": "neutral",
        "server": "test",
        "coordinate_space": {"width": WIDTH, "height": 1},
        "defaults": {"color_max_distance": 0.0},
        "targets": targets
    });
    let pages = PAGES
        .iter()
        .map(|(page, _)| {
            json!({"id": format!("neutral/{page}"), "required": [format!("page/{page}")],
                   "optional": [], "forbidden": []})
        })
        .collect::<Vec<_>>();
    zip_entries(&[
        ("control.json", bytes(&control)),
        (
            "resources/manifest.json",
            bytes(&json!({"schema_version": "0.3", "entry_task_id": "task"})),
        ),
        (TASK_FILE, bytes(task)),
        ("resources/recognition/neutral.test.pack.json", bytes(&pack)),
        (
            "resources/recognition/neutral.test.pages.json",
            bytes(&json!({"schema_version": "0.3", "pages": pages})),
        ),
    ])
}

fn admit(task: &Value, mode: &str) -> Result<PreparedContainedTask, ContainedTaskError> {
    PreparedContainedTask::load("neutral.instance", &package(task, mode))
}

fn admitted(case: &str, task: &Value) -> PreparedContainedTask {
    admit(task, "navigable_route")
        .unwrap_or_else(|error| panic!("{case}: admission refused the package: {error}"))
}

type RunResult = Result<ContainedTaskOutcome, ContainedTaskRunError<&'static str>>;

fn succeeded(case: &str, result: RunResult) -> ContainedTaskOutcome {
    match result {
        Ok(outcome) => {
            assert_eq!(outcome.outcome, TaskOutcome::Success, "{case}");
            outcome
        }
        Err(error) => panic!("{case}: expected the run to end Completed, got {error:?}"),
    }
}

fn failed(case: &str, result: RunResult) -> ContainedTaskError {
    match result {
        Err(ContainedTaskRunError::Task(error)) => error,
        Err(other) => panic!("{case}: expected a task failure, got {other:?}"),
        Ok(outcome) => panic!(
            "{case}: expected a task failure, the run ended {:?} on {:?} with {:?}",
            outcome.outcome, outcome.final_page, outcome.selected_scheduling_outcome
        ),
    }
}

fn value(error: &ContainedTaskError, key: &str) -> Value {
    error.outcome().value(key).cloned().unwrap_or(Value::Null)
}

fn ms(values: &[u64]) -> Vec<Duration> {
    values.iter().copied().map(Duration::from_millis).collect()
}

// The operations whose steps started, in order.
fn started(runtime: &ScriptedRuntime) -> Vec<&str> {
    runtime
        .traces()
        .iter()
        .filter_map(|trace| match trace {
            ContainedTaskTrace::StepStarted {
                operation_label, ..
            } => Some(operation_label.as_str()),
            _ => None,
        })
        .collect()
}

// The pages the steps of `operation` finished on, in order.
fn finished_on<'r>(runtime: &'r ScriptedRuntime, operation: &str) -> Vec<&'r str> {
    runtime
        .traces()
        .iter()
        .filter_map(|trace| match trace {
            ContainedTaskTrace::StepFinished {
                operation_label,
                page_label,
                ..
            } if operation_label == operation => Some(page_label.as_str()),
            _ => None,
        })
        .collect()
}

// The effect records (`EffectIntent`, `EffectCompleted`) of `operation`'s steps.
fn effect_records(runtime: &ScriptedRuntime, operation: &str) -> usize {
    runtime
        .traces()
        .iter()
        .filter(|trace| match trace {
            ContainedTaskTrace::EffectIntent {
                operation_label, ..
            }
            | ContainedTaskTrace::EffectCompleted {
                operation_label, ..
            } => operation_label == operation,
            _ => false,
        })
        .count()
}

// T1 (exec key 175; §2.3 (1) "Run"): `op_wait` waits on `page_a` for `page_b`. The device shows
// `page_a` (the run's first frame), `page_a` three more times, then `page_b`. The step finishes on
// `page_b` with no guard, no effect intent, no input and no effect record; the run goes on from
// that frame with `op_next`, whose one tap brings `page_c`, the target.
#[test]
fn a35_s3_t1_a_wait_step_finishes_on_its_destination_without_an_input() {
    let task = admitted(
        "wait step",
        &task(
            vec![wait_step(json!({"max_ms": 60_000}), json!({})), next()],
            "page_c",
            Vec::new(),
        ),
    );
    let mut runtime = device(&[
        &["page_a", "page_a", "page_a", "page_a", "page_b"],
        &["page_c"],
    ]);

    let outcome = succeeded("wait step", task.run(&mut runtime));

    assert_eq!(finished_on(&runtime, "op_wait"), ["neutral/page_b"]);
    assert_eq!(
        effect_records(&runtime, "op_wait"),
        0,
        "{:?}",
        runtime.traces()
    );
    assert_eq!(started(&runtime), ["op_wait", "op_next"]);
    assert_eq!(runtime.inputs().len(), 1, "only op_next's tap");
    assert_eq!(outcome.final_page.as_deref(), Some("neutral/page_c"));
    assert_eq!(outcome.executed_steps, 2);
}

// T2 (exec key 176; §2.3 (1) "Run", coordinator decision Q9): `op_wait` waits up to 5000 ms and
// the device shows `page_a` throughout. A frame every 1000 ms: the frame at 5000 ms of the step's
// wait on the run's budget clock reaches `max_ms` on the wait page, after five pauses.
// - With `on_timeout: still_busy` (the busy mapping of `page_a`, Q11), the step finishes on
//   `page_a` and the run is Completed there with `still_busy`, after the one step and no input.
// - Without `on_timeout`, the run fails `contained_task_step_unconfirmed` with `page_id` the
//   wait page `page_a`.
#[test]
fn a35_s3_t2_the_wait_page_beyond_max_ms_ends_on_its_key_or_unconfirmed() {
    let keyed = admitted(
        "with on_timeout",
        &task(
            vec![
                wait_step(
                    json!({"max_ms": 5_000, "on_timeout": "still_busy"}),
                    json!({}),
                ),
                next(),
            ],
            "page_c",
            vec![
                mapping("still_busy", "page_a", true),
                mapping("done", "page_c", false),
            ],
        ),
    );
    let mut runtime = device(&[&["page_a"]]);

    let outcome = succeeded("with on_timeout", keyed.run(&mut runtime));

    assert_eq!(outcome.final_page.as_deref(), Some("neutral/page_a"));
    assert_eq!(
        outcome.selected_scheduling_outcome.as_deref(),
        Some("still_busy")
    );
    assert_eq!(outcome.executed_steps, 1);
    assert_eq!(finished_on(&runtime, "op_wait"), ["neutral/page_a"]);
    assert!(runtime.inputs().is_empty(), "{:?}", runtime.inputs());
    assert_eq!(runtime.pauses(), ms(&[INTERVAL_MS; 5]), "with on_timeout");

    let plain = admitted(
        "without on_timeout",
        &task(
            vec![wait_step(json!({"max_ms": 5_000}), json!({})), next()],
            "page_c",
            Vec::new(),
        ),
    );
    let mut runtime = device(&[&["page_a"]]);

    let error = failed("without on_timeout", plain.run(&mut runtime));

    assert_eq!(error.code().as_str(), STEP_UNCONFIRMED_CODE, "{error}");
    assert_eq!(value(&error, "page_id"), json!("neutral/page_a"), "{error}");
    assert_eq!(
        value(&error, "pack_operation_id"),
        json!("op_wait"),
        "{error}"
    );
    assert!(runtime.inputs().is_empty(), "{:?}", runtime.inputs());
    assert_eq!(
        runtime.pauses(),
        ms(&[INTERVAL_MS; 5]),
        "without on_timeout"
    );
}

// T3 (exec key 177; §2.3 (1) "Run": the away clock): `op_wait` waits up to 60000 ms. The device
// shows `page_a` (the run's first frame and the step's frames at 0 ms and 1000 ms), then
// `page_d`, which the step does not name. The away clock (`expect_after.timeout_ms`, 3000 ms)
// restarts at the wait page's frame at 1000 ms and runs out on `page_d` at 4000 ms, after four
// pauses: the run fails `contained_task_step_unconfirmed` on `page_d` with the away clock's
// elapsed time and limit, long before `max_ms`.
#[test]
fn a35_s3_t3_another_page_ends_the_wait_after_the_away_clock_not_after_max_ms() {
    let task = admitted(
        "away",
        &task(
            vec![wait_step(json!({"max_ms": 60_000}), json!({})), next()],
            "page_c",
            Vec::new(),
        ),
    );
    let mut runtime = device(&[&["page_a", "page_a", "page_a", "page_d"]]);

    let error = failed("away", task.run(&mut runtime));

    assert_eq!(error.code().as_str(), STEP_UNCONFIRMED_CODE, "{error}");
    assert_eq!(value(&error, "page_id"), json!("neutral/page_d"), "{error}");
    let timing = error.timing().expect("the away clock's timing");
    assert_eq!((timing.elapsed_ms, timing.limit_ms), (AWAY_MS, AWAY_MS));
    assert_eq!(runtime.pauses(), ms(&[INTERVAL_MS; 4]));
    assert!(runtime.inputs().is_empty(), "{:?}", runtime.inputs());
}

// T4 (exec key 178; §2.3 (2) "Run"): `op_tap` taps `page_a` and is confirmed on `page_b` within
// 2000 ms, a frame every 500 ms. After the tap the device shows `page_c` eight times (until
// 3500 ms), then `page_b`.
// - With `wait_page: {page_c, max_ms 10000}`, the frames on `page_c` pause the confirmation
//   clock: `page_b` at 4000 ms, beyond the timeout but within `max_ms`, confirms the step and the
//   run is Completed on `page_b` after eight pauses.
// - The same task and frames without the wait page: the step is unconfirmed on `page_c`.
#[test]
fn a35_s3_t4_a_wait_page_pauses_the_confirmation_clock() {
    let frames: &[&[&str]] = &[
        &["page_a"],
        &[
            "page_c", "page_c", "page_c", "page_c", "page_c", "page_c", "page_c", "page_c",
            "page_b",
        ],
    ];
    let waiting = admitted(
        "with the wait page",
        &task(
            vec![tap(
                json!({"wait_page": {"page_id": "page_c", "max_ms": 10_000}}),
            )],
            "page_b",
            Vec::new(),
        ),
    );
    let mut runtime = device(frames);

    let outcome = succeeded("with the wait page", waiting.run(&mut runtime));

    assert_eq!(outcome.final_page.as_deref(), Some("neutral/page_b"));
    assert_eq!(runtime.inputs().len(), 1);
    assert_eq!(runtime.pauses(), ms(&[500; 8]));

    let plain = admitted(
        "without the wait page",
        &task(vec![tap(json!({}))], "page_b", Vec::new()),
    );
    let mut runtime = device(frames);

    let error = failed("without the wait page", plain.run(&mut runtime));

    assert_eq!(error.code().as_str(), STEP_UNCONFIRMED_CODE, "{error}");
    assert_eq!(value(&error, "page_id"), json!("neutral/page_c"), "{error}");
}

// T5 (exec key 179; §2.3 (1) and (2) "Admission", coordinator decision Q11; brief "Refusals"):
// each package is refused with its code and rule. A wait step's rules are
// `contained_task_program_invalid` naming the rule (`program_problem`), the task declaration's
// `path` and the refused member (`json_pointer`); a `linear_steps` task refuses a wait step with
// `contained_task_linear_invalid`, `reason=wait`. An admitted wait step named as the declared
// operation of a Lab operation (#412 Lab B-3 S1, register §1.161; Lab decision Q2: page-graph
// clicks only) is `contained_task_declared_operation_unsupported` with its `pack_operation_id`
// and `lab_operation_problem` `wait`.
#[test]
fn a35_s3_t5_admission_refuses_a_broken_wait_step_or_wait_page_with_its_rule() {
    let busy_elsewhere = vec![
        mapping("still_busy", "page_d", true),
        mapping("done", "page_c", false),
    ];
    let not_busy = vec![
        mapping("still_busy", "page_a", false),
        mapping("done", "page_c", false),
    ];
    let timeout = json!({"max_ms": 5_000, "on_timeout": "still_busy"});
    let rows: Vec<(&str, Value, &str, &str)> = vec![
        (
            "a wait step with a click",
            task(
                vec![
                    wait_step(
                        json!({"max_ms": 5_000}),
                        json!({"click": {"kind": "point", "x": 1, "y": 0},
                               "unguarded_trusted_coordinate": true}),
                    ),
                    next(),
                ],
                "page_c",
                Vec::new(),
            ),
            "wait_member_refused",
            "/operations/0/click",
        ),
        (
            "a wait step with a guard",
            task(
                vec![
                    wait_step(
                        json!({"max_ms": 5_000}),
                        json!({"guard": {"page_id": "page_a", "target_id": "page/page_a",
                                         "expected_rect": {"x": 0, "y": 0, "width": 1, "height": 1}}}),
                    ),
                    next(),
                ],
                "page_c",
                Vec::new(),
            ),
            "wait_member_refused",
            "/operations/0/guard",
        ),
        (
            "a wait step with a retry member",
            task(
                vec![
                    wait_step(json!({"max_ms": 5_000}), json!({"retryable": false})),
                    next(),
                ],
                "page_c",
                Vec::new(),
            ),
            "wait_member_refused",
            "/operations/0/retryable",
        ),
        (
            "an on_timeout whose mapping does not list the step's page",
            task(
                vec![wait_step(timeout.clone(), json!({})), next()],
                "page_c",
                busy_elsewhere,
            ),
            "wait_timeout_unmapped",
            "/operations/0/wait/on_timeout",
        ),
        (
            "an on_timeout whose mapping is not busy",
            task(
                vec![wait_step(timeout, json!({})), next()],
                "page_c",
                not_busy,
            ),
            "wait_timeout_not_busy",
            "/operations/0/wait/on_timeout",
        ),
        (
            "a wait page that is a destination",
            task(
                vec![tap(
                    json!({"wait_page": {"page_id": "page_b", "max_ms": 10_000}}),
                )],
                "page_b",
                Vec::new(),
            ),
            "wait_page_invalid",
            "/operations/0/expect_after/wait_page/page_id",
        ),
    ];
    for (case, task, problem, pointer) in rows {
        let error = match admit(&task, "navigable_route") {
            Ok(_) => panic!("{case}: admitted"),
            Err(error) => error,
        };
        assert_eq!(error.code().as_str(), PROGRAM_INVALID, "{case}: {error}");
        assert_eq!(value(&error, "program_problem"), json!(problem), "{case}");
        assert_eq!(value(&error, "json_pointer"), json!(pointer), "{case}");
        assert_eq!(value(&error, "path"), json!(TASK_FILE), "{case}");
    }

    // A `linear_steps` task of the one wait step from its entry page to its target page.
    let mut linear = task(
        vec![wait_step(json!({"max_ms": 5_000}), json!({}))],
        "page_b",
        Vec::new(),
    );
    linear["max_steps"] = json!(1);
    let error = match admit(&linear, "linear_steps") {
        Ok(_) => panic!("linear_steps: admitted"),
        Err(error) => error,
    };
    assert_eq!(error.code().as_str(), LINEAR_INVALID, "{error}");
    assert!(
        error
            .detail()
            .is_some_and(|detail| detail.split(' ').any(|token| token == "reason=wait")),
        "{error}"
    );

    // The declared operation `op_wait` of an admitted page-graph package.
    let prepared = admitted(
        "a declared wait step",
        &task(
            vec![wait_step(json!({"max_ms": 5_000}), json!({})), next()],
            "page_c",
            Vec::new(),
        ),
    );
    let error = match prepared.declared_operation("op_wait") {
        Ok(declared) => panic!("a declared wait step: admitted as {declared:?}"),
        Err(error) => error,
    };
    assert_eq!(
        error.code().as_str(),
        ExecutionKernelCode::ContainedTaskDeclaredOperationUnsupported.as_str(),
        "{error}"
    );
    assert_eq!(
        Value::Object(error.outcome().values().clone()),
        json!({"pack_operation_id": "op_wait", "lab_operation_problem": "wait"}),
        "{error}"
    );
}
