// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #477 LR-1 (0.12.79; register §1.513; contracts/linear-steps.md, "Pressing on one page";
// the coordinator's rulings in the work order of board #477, not the owner's words): a click step of
// a `linear_steps` task whose `to` is its `from` may declare `presses`, how often it presses on its
// page: a whole number, or the value of `session.dispatch.parameter.<name>` less `minus`, never
// below 0, read before the run's first capture as a page graph reads a designated effect's fact
// cap. After each press come the step's post-input wait and the arrival of its own page; a count of
// 0 presses nothing and the step is done; a count that cannot be read ends the run with
// `contained_task_designated_effect_cap_unresolved` before any press; a press whose page does not
// come ends the run as a click step's last attempt does, the presses made before it recorded. The
// declaration gate refuses the member at its pointer on a step that is no press, on one that leaves
// its page, and a constant, a key or a `minus` out of its range. A package without the member runs
// as before.
//
// Every expected value comes from those rulings and the package values below, never from the code.
// The packages are synthetic `neutral` packages admitted by the kernel's whole admission
// (`PreparedContainedTask::load`, the declaration gate included), in the fixture of KP-1's gate
// (`gate_463_kp1.rs`): a frame is 2 x 1, pixel 0 names the page (home, list, done), pixel 1 is a
// mark. The press path taps from home into the list (`open`, a trusted point at pixel 0), presses
// on the list (`plus`, a trusted point at pixel 1, the list before and after it) and taps from the
// list to done (`start`, at pixel 0). The runs play scripted frames on the kernel's scripted clock
// (EXEC-I4): the device shows `screens[n]` after `n` inputs, each segment's last frame repeating;
// the runtime around it answers the run's fact snapshot as the host does (`selection_state`).

use super::tests::scripted::{ScriptedFrame, ScriptedRuntime};
use super::*;
use actingcommand_contract::{
    FactContent, FactRecord, FactValue, InstanceFactContext, InstanceFactSnapshot,
    ResourceDeclarationReason,
};
use serde_json::{Value, json};
use std::io::{Cursor, Write};
use zip::{ZipWriter, write::FileOptions};

const CONFIRMATION_FAILED: &str = ExecutionKernelCode::PageConfirmationFailed.as_str();
const UNRESOLVED: &str = ExecutionKernelCode::ContainedTaskDesignatedEffectCapUnresolved.as_str();

const WIDTH: u32 = 2;
const STEP_MS: u64 = 5_000;
const ARRIVAL_MS: u64 = 1_000;
const TASK_MS: u64 = 60_000;
const NOW_UNIX_MS: u64 = 1_000_000;
const INSTANCE: &str = "neutral.instance";
// The dispatch parameter the press count reads: the package's integer variable `turns`.
const TURNS_KEY: &str = "session.dispatch.parameter.turns";
// The detector pages.
const LIST_PAGE: &str = "neutral/list";
const DONE_PAGE: &str = "neutral/done";
// The detail `page_confirmation_failed` gives the end of a press whose page did not come
// (contracts/linear-steps.md, "Failure codes"), as it gives a click step's one attempt.
const PLUS_UNCONFIRMED: &str =
    "operation=plus attempts=1 after_page=<unrecognized> hit_error_page=false transition=none";

const HOME: [u8; 3] = [255, 0, 255];
const LIST: [u8; 3] = [255, 0, 0];
const DONE: [u8; 3] = [255, 255, 0];
const GREEN: [u8; 3] = [0, 255, 0];
const BLACK: [u8; 3] = [0, 0, 0];

type RunResult = Result<ContainedTaskOutcome, ContainedTaskRunError<&'static str>>;

// A screen of the page `page` with the mark shown or not.
fn screen(page: [u8; 3], marked: bool) -> ScriptedFrame {
    ScriptedFrame::rgb(WIDTH, 1, &[page, if marked { GREEN } else { BLACK }])
}

// The screens of the press path with `presses` presses: home; the list after `open` and after each
// press; done after `start`.
fn press_screens(presses: usize) -> Vec<Vec<ScriptedFrame>> {
    let mut screens = vec![vec![screen(HOME, false)]];
    screens.extend((0..=presses).map(|_| vec![screen(LIST, false)]));
    screens.push(vec![screen(DONE, false)]);
    screens
}

// The linear control of the package `package_id`, a path of `max_steps` operations (KP-1's).
fn control(package_id: &str, max_steps: usize) -> Value {
    json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": package_id,
        "execution_mode": "linear_steps",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": WIDTH, "height": 1},
        "entry_task_id": "task",
        "capture_interval_ms": 50,
        "step_timeout_ms": STEP_MS,
        "timeout_ms": TASK_MS,
        "max_steps": max_steps
    })
}

// The press path's control: three operations and the integer variable `turns`, from 0 to 1000,
// which a run reads as `session.dispatch.parameter.turns`.
fn press_control() -> Value {
    let mut control = control("neutral.linear.press", 3);
    control["variables"] = json!([{"name": "turns", "type": "integer", "title": {"en": "Turns"},
                                   "min": 0, "max": 1000, "default": 1}]);
    control
}

// A trusted tap `id` from `from` to `to`, expected within the arrival budget (KP-1's).
fn tap(id: &str, from: &str, to: &str) -> Value {
    json!({"id": id, "from": from, "to": to,
           "click": {"kind": "point", "x": 0, "y": 0},
           "unguarded_trusted_coordinate": true,
           "expect_after": {"page_id": to, "timeout_ms": ARRIVAL_MS}})
}

// The press step `plus` on the list, a trusted point at pixel 1 that stays on the list, pressing as
// `presses` says.
fn press_step(presses: Value) -> Value {
    json!({"id": "plus", "from": "list", "to": "list",
           "click": {"kind": "point", "x": 1, "y": 0},
           "unguarded_trusted_coordinate": true,
           "expect_after": {"page_id": "list", "timeout_ms": ARRIVAL_MS},
           "presses": presses})
}

// A task from `entry` to `target` over `operations` (KP-1's).
fn task(entry: &str, target: &str, operations: Vec<Value>) -> Value {
    let max_steps = operations.len();
    json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": WIDTH, "height": 1},
        "entry_page": entry,
        "target_page": target,
        "max_steps": max_steps,
        "operations": operations
    })
}

// The press path: `open` from home into the list, `plus` on the list, `start` from the list to done.
fn press_task(plus: Value) -> Value {
    task(
        "home",
        "done",
        vec![
            tap("open", "home", "list"),
            plus,
            tap("start", "list", "done"),
        ],
    )
}

// The page set: home, list and done told by pixel 0 (KP-1's).
fn pages() -> Value {
    let pages = ["home", "list", "done"].map(|page| {
        json!({"id": format!("neutral/{page}"), "required": [format!("page/{page}")],
               "optional": [], "forbidden": []})
    });
    json!({"schema_version": "0.3", "pages": pages})
}

// The recognition pack: the pages' marks on pixel 0 and the mark on pixel 1 (KP-1's).
fn pack() -> Value {
    let colour = |id: &str, x: u32, expected: [u8; 3]| {
        json!({"type": "color", "id": id,
               "region": {"x": x, "y": 0, "width": 1, "height": 1}, "expected": expected})
    };
    json!({
        "schema_version": "0.7",
        "game": "neutral",
        "server": "test",
        "coordinate_space": {"width": WIDTH, "height": 1},
        "defaults": {"color_max_distance": 0.0},
        "targets": [
            colour("page/home", 0, HOME),
            colour("page/list", 0, LIST),
            colour("page/done", 0, DONE),
            colour("guard/ready", 1, GREEN)
        ]
    })
}

fn bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).expect("JSON bytes")
}

// The package of `control` and `task` (KP-1's layout).
fn package(control: &Value, task: &Value) -> Vec<u8> {
    let entries = [
        ("control.json", bytes(control)),
        (
            "resources/manifest.json",
            bytes(&json!({"schema_version": "0.3", "entry_task_id": "task"})),
        ),
        ("resources/operations/task/task.json", bytes(task)),
        (
            "resources/recognition/neutral.test.pack.json",
            bytes(&pack()),
        ),
        (
            "resources/recognition/neutral.test.pages.json",
            bytes(&pages()),
        ),
    ];
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, content) in entries {
        zip.start_file(path, options).expect("zip entry");
        zip.write_all(&content).expect("zip bytes");
    }
    zip.finish().expect("finish zip").into_inner()
}

// The package as contained task admission prepares it, or its refusal.
fn load(control: &Value, task: &Value) -> Result<PreparedContainedTask, ContainedTaskError> {
    PreparedContainedTask::load(INSTANCE, &package(control, task))
}

fn admitted(case: &str, control: &Value, task: &Value) -> PreparedContainedTask {
    load(control, task).unwrap_or_else(|error| panic!("{case}: refused: {error}"))
}

fn failed(case: &str, result: RunResult) -> ContainedTaskError {
    match result {
        Err(ContainedTaskRunError::Task(error)) => error,
        Err(other) => panic!("{case}: expected a task failure, got {other:?}"),
        Ok(outcome) => panic!("{case}: expected a task failure, the run ended {outcome:?}"),
    }
}

fn value(error: &ContainedTaskError, key: &str) -> Value {
    error.outcome().value(key).cloned().unwrap_or(Value::Null)
}

// The run's normal end on `page` after `executed_steps`, with no key of the kernel's.
fn completed(page: &str, executed_steps: u32) -> ContainedTaskOutcome {
    ContainedTaskOutcome {
        outcome: TaskOutcome::Success,
        final_page: Some(page.to_owned()),
        executed_steps,
        selected_scheduling_outcome: None,
        designated_effect_caps: Vec::new(),
    }
}

// Ruling 5: what the run's records show of its steps and of each press, by the members the step
// and effect records carry, one line each in order: a step's start (its index, its operation and
// its page), each effect intent (the input as its JSON, whether it was drawn, the guard it was
// decided under), each effect completion, and a step's end on the page that passed.
fn step_lines(device: &ScriptedRuntime) -> Vec<String> {
    device
        .traces()
        .iter()
        .filter_map(|trace| match trace {
            ContainedTaskTrace::StepStarted {
                step_index,
                operation_label,
                from_page,
                ..
            } => Some(format!(
                "step_started {step_index} {operation_label} {from_page}"
            )),
            ContainedTaskTrace::EffectIntent {
                step_index,
                operation_label,
                action,
                sampling,
                guard,
            } => Some(format!(
                "effect_intent {step_index} {operation_label} {} sampling={} guard={}",
                serde_json::to_string(action).expect("the input as JSON"),
                if sampling.is_some() { "drawn" } else { "none" },
                if matches!(guard, ContainedTaskGuardOutcome::TrustedCoordinate) {
                    "trusted_coordinate"
                } else {
                    "evaluated"
                }
            )),
            ContainedTaskTrace::EffectCompleted {
                step_index,
                operation_label,
            } => Some(format!("effect_completed {step_index} {operation_label}")),
            ContainedTaskTrace::StepFinished {
                step_index,
                operation_label,
                page_label,
                ..
            } => Some(format!(
                "step_finished {step_index} {operation_label} {page_label}"
            )),
            _ => None,
        })
        .collect()
}

// The lines of `open`'s tap and its arrival on the list, and of `plus`'s start on the list.
const OPEN_AND_PLUS_STARTED: [&str; 5] = [
    "step_started 0 open neutral/home",
    r#"effect_intent 0 open {"kind":"tap","x":0,"y":0} sampling=none guard=trusted_coordinate"#,
    "effect_completed 0 open",
    "step_finished 0 open neutral/list",
    "step_started 1 plus neutral/list",
];
// The lines of one press of `plus`.
const PLUS_PRESS: [&str; 2] = [
    r#"effect_intent 1 plus {"kind":"tap","x":1,"y":0} sampling=none guard=trusted_coordinate"#,
    "effect_completed 1 plus",
];
// The lines of `plus`'s end on the list, then of `start` from the list to done.
const PLUS_DONE_AND_START: [&str; 5] = [
    "step_finished 1 plus neutral/list",
    "step_started 2 start neutral/list",
    r#"effect_intent 2 start {"kind":"tap","x":0,"y":0} sampling=none guard=trusted_coordinate"#,
    "effect_completed 2 start",
    "step_finished 2 start neutral/done",
];

// The lines of a whole run of the press path in which `plus` pressed `presses` times.
fn press_path_lines(presses: usize) -> Vec<String> {
    let mut lines = OPEN_AND_PLUS_STARTED.to_vec();
    for _ in 0..presses {
        lines.extend(PLUS_PRESS);
    }
    lines.extend(PLUS_DONE_AND_START);
    lines.into_iter().map(str::to_owned).collect()
}

// After each effect completion of `operation`, the first recognition that follows it: the pages it
// awaited and the page that passed.
fn recognitions_after(
    device: &ScriptedRuntime,
    operation: &str,
) -> Vec<(Vec<String>, Option<String>)> {
    let mut after = Vec::new();
    let mut pressed = false;
    for trace in device.traces() {
        match trace {
            ContainedTaskTrace::EffectCompleted {
                operation_label, ..
            } if operation_label == operation => pressed = true,
            ContainedTaskTrace::RecognitionCompleted {
                candidate_pages,
                page_label,
                ..
            } if pressed => {
                after.push((candidate_pages.clone(), page_label.clone()));
                pressed = false;
            }
            _ => {}
        }
    }
    after
}

// The records of the run's fact snapshot as the host overlays them for a dispatch that hands the
// variable `turns` the value `turns` (`dispatched_run_parameters`, `with_variables`, `overlay`):
// the one record `session.dispatch.parameter.turns`, an integer.
fn dispatched_turns(package: &PreparedContainedTask, turns: i64) -> Vec<FactRecord> {
    let mut snapshot = InstanceFactSnapshot {
        snapshot_id: "snapshot:lr1".to_owned(),
        ledger_position: 1,
        context: InstanceFactContext {
            instance_id: INSTANCE.to_owned(),
            server_id: "test".to_owned(),
            game_id: "neutral".to_owned(),
        },
        records: Vec::new(),
    };
    package
        .dispatched_run_parameters(Vec::new(), NOW_UNIX_MS - 1_000)
        .with_variables(package.dispatched_variable_values(&[("turns".to_owned(), json!(turns))]))
        .overlay(&mut snapshot, NOW_UNIX_MS);
    assert_eq!(
        snapshot
            .records
            .iter()
            .map(|record| (record.key.as_str(), &record.content))
            .collect::<Vec<_>>(),
        [(
            TURNS_KEY,
            &FactContent::Inline {
                value: FactValue::Integer(turns)
            }
        )],
        "turns {turns}"
    );
    snapshot.records
}

// The scripted device behind a runtime that answers the run's fact snapshot as the host does
// (`selection_state`): `records` on the run's instance of its game and server at `NOW_UNIX_MS`, or
// no snapshot at all (`None`). It counts how often it was asked; every other answer is the scripted
// runtime's.
struct FactDevice {
    script: ScriptedRuntime,
    records: Option<Vec<FactRecord>>,
    fact_reads: usize,
}

impl FactDevice {
    fn new(screens: Vec<Vec<ScriptedFrame>>, records: Option<Vec<FactRecord>>) -> Self {
        Self {
            script: ScriptedRuntime::new(screens),
            records,
            fact_reads: 0,
        }
    }
}

impl ContainedTaskRuntime for FactDevice {
    type Error = &'static str;

    fn capture(&mut self) -> Result<ObservedFrame, Self::Error> {
        self.script.capture()
    }

    fn selection_state(
        &mut self,
        request: SelectionStateRequest,
    ) -> Result<SelectionState, Self::Error> {
        self.fact_reads += 1;
        Ok(match &self.records {
            None => SelectionState::Unavailable,
            Some(records) => SelectionState::Snapshot {
                snapshot: InstanceFactSnapshot {
                    snapshot_id: "snapshot:lr1".to_owned(),
                    ledger_position: 1,
                    context: InstanceFactContext {
                        instance_id: INSTANCE.to_owned(),
                        server_id: request.server,
                        game_id: request.game,
                    },
                    records: records.clone(),
                },
                now_unix_ms: NOW_UNIX_MS,
            },
        })
    }

    fn input(
        &mut self,
        action: InputAction,
        frame: Option<InputFrameContext>,
    ) -> Result<(), Self::Error> {
        self.script.input(action, frame)
    }

    fn record(&mut self, trace: ContainedTaskTrace) -> Result<(), Self::Error> {
        self.script.record(trace)
    }

    fn sampling_supported(&self) -> bool {
        self.script.sampling_supported()
    }

    fn step_pacing(&mut self) -> Result<StepPacing, Self::Error> {
        self.script.step_pacing()
    }

    fn step_action_gap(&mut self) -> Result<Duration, Self::Error> {
        self.script.step_action_gap()
    }

    fn is_bookkeeping_failure(error: &Self::Error) -> bool {
        <ScriptedRuntime as ContainedTaskRuntime>::is_bookkeeping_failure(error)
    }

    fn monotonic_now(&mut self) -> Instant {
        self.script.monotonic_now()
    }

    fn pause(&mut self, duration: Duration) {
        self.script.pause(duration);
    }
}

// Test 1 (rulings 1, 2 and 5): `plus` declares the constant 3. It starts once on the list and
// presses three times, its intents and completions all of step 1; after each press its own page is
// awaited alone and passes on the list; it finishes on the list, `start` decides on that frame and
// the run is Completed on done after three steps, five inputs, no fact read.
#[test]
fn lr1_a_constant_count_presses_three_times_and_the_page_is_confirmed_after_each() {
    let prepared = admitted(
        "a constant count",
        &press_control(),
        &press_task(press_step(json!(3))),
    );
    let mut device = FactDevice::new(press_screens(3), None);

    let outcome = prepared
        .run(&mut device)
        .unwrap_or_else(|error| panic!("expected a normal end, got {error:?}"));

    assert_eq!(outcome, completed(DONE_PAGE, 3));
    assert_eq!(step_lines(&device.script), press_path_lines(3));
    assert_eq!(
        device.script.inputs().len(),
        5,
        "{:?}",
        device.script.inputs()
    );
    let confirmed = (vec![LIST_PAGE.to_owned()], Some(LIST_PAGE.to_owned()));
    assert_eq!(
        recognitions_after(&device.script, "plus"),
        [confirmed.clone(), confirmed.clone(), confirmed]
    );
    assert_eq!(device.fact_reads, 0);
}

// Test 2 (rulings 1, 2 and 5): `plus` reads the dispatch parameter `turns` with `minus` 1, the value
// a dispatch hands the run overlaid as the host overlays it. A value of 4 presses three times; of 1
// and of 0 nothing: the step starts and finishes on the list with no effect between, and `start`
// decides on the frame `plus` started on. Each run reads the snapshot once and is Completed on done
// after three steps.
#[test]
fn lr1_the_dispatch_parameter_less_one_presses_three_times_for_four_and_none_for_one_or_zero() {
    let prepared = admitted(
        "a dispatch parameter",
        &press_control(),
        &press_task(press_step(json!({"fact_key": TURNS_KEY, "minus": 1}))),
    );
    for (turns, presses) in [(4, 3), (1, 0), (0, 0)] {
        let records = dispatched_turns(&prepared, turns);
        let mut device = FactDevice::new(press_screens(presses), Some(records));

        let outcome = prepared
            .run(&mut device)
            .unwrap_or_else(|error| panic!("turns {turns}: expected a normal end, got {error:?}"));

        assert_eq!(outcome, completed(DONE_PAGE, 3), "turns {turns}");
        assert_eq!(
            step_lines(&device.script),
            press_path_lines(presses),
            "turns {turns}"
        );
        assert_eq!(
            device.script.inputs().len(),
            2 + presses,
            "turns {turns}: {:?}",
            device.script.inputs()
        );
        assert_eq!(device.fact_reads, 1, "turns {turns}");
    }
}

// Test 3 (ruling 3): the count cannot be read: with no fact snapshot, and with a snapshot that holds
// no record of the key, the run ends with `contained_task_designated_effect_cap_unresolved`, naming
// `plus` and the key (and, for the missing record, `unknown_reason` `fact_missing`), after
// `RunStarted` and before any capture, step start or input.
#[test]
fn lr1_a_count_that_cannot_be_read_ends_the_run_before_any_press() {
    let prepared = admitted(
        "a dispatch parameter",
        &press_control(),
        &press_task(press_step(json!({"fact_key": TURNS_KEY, "minus": 1}))),
    );
    for (case, records, extra) in [
        ("no fact snapshot", None, json!({})),
        (
            "no record of the key",
            Some(Vec::new()),
            json!({"unknown_reason": "fact_missing"}),
        ),
    ] {
        let mut device = FactDevice::new(press_screens(3), records);

        let error = failed(case, prepared.run(&mut device));

        assert_eq!(error.code().as_str(), UNRESOLVED, "{case}: {error}");
        let mut expected = json!({"pack_operation_id": "plus", "fact_key": TURNS_KEY});
        for (key, value) in extra.as_object().expect("extra values") {
            expected[key] = value.clone();
        }
        assert_eq!(
            Value::Object(error.outcome().values().clone()),
            expected,
            "{case}"
        );
        assert_eq!(device.fact_reads, 1, "{case}");
        assert_eq!(device.script.captures(), 0, "{case}");
        assert!(device.script.inputs().is_empty(), "{case}");
        assert!(step_lines(&device.script).is_empty(), "{case}");
        assert!(
            device
                .script
                .traces()
                .contains(&ContainedTaskTrace::RunStarted),
            "{case}"
        );
    }
}

// Test 4 (rulings 3 and 5): `plus` declares 3, and after its second press the screen shows done,
// not the list. That press's arrival on the list runs out and the closing decision follows, as after
// a click step's last attempt; the run fails with `page_confirmation_failed`, the click step's
// detail, the arrival's postcondition timing and the word `other_page_shown` (done, another page of
// the package, stands still on the decision's last frame). The records hold the two presses, and
// `plus` finishes on no page; no third press and no `start`.
#[test]
fn lr1_the_page_lost_after_the_second_press_ends_the_run_as_a_click_steps_last_attempt() {
    let prepared = admitted(
        "a constant count",
        &press_control(),
        &press_task(press_step(json!(3))),
    );
    let screens = vec![
        vec![screen(HOME, false)],
        vec![screen(LIST, false)],
        vec![screen(LIST, false)],
        vec![screen(DONE, false)],
    ];
    let mut device = FactDevice::new(screens, None);

    let error = failed("the page lost", prepared.run(&mut device));

    assert_eq!(error.code().as_str(), CONFIRMATION_FAILED, "{error}");
    assert_eq!(error.detail(), Some(PLUS_UNCONFIRMED), "{error}");
    assert_eq!(
        error
            .timing()
            .map(|failure| (failure.scope, failure.stage, failure.limit_ms)),
        Some((
            TaskTimingScope::Postcondition,
            TaskTimingStage::Postcondition,
            ARRIVAL_MS
        )),
        "{error}"
    );
    assert_eq!(
        value(&error, "confirmation_problem"),
        json!("other_page_shown"),
        "{:?}",
        error.outcome()
    );
    let mut lines = OPEN_AND_PLUS_STARTED.to_vec();
    lines.extend(PLUS_PRESS);
    lines.extend(PLUS_PRESS);
    lines.push("step_finished 1 plus <unrecognized>");
    assert_eq!(step_lines(&device.script), lines);
    assert_eq!(
        device.script.inputs().len(),
        3,
        "{:?}",
        device.script.inputs()
    );
}

// Test 5 (ruling 6): the declaration gate, which admission runs first, refuses the member at its
// pointer with `resource_declaration_invalid`, `InvalidValue`: on a step that is no press (a key
// step) and on one that leaves its page (the list to done), at `/operations/1/presses`; a constant
// of 0 and of 1001 there too; a key outside `session.dispatch.parameter.` at
// `/operations/1/presses/fact_key`; a `minus` of 1001 at `/operations/1/presses/minus`; and a press
// step that declares a retry at `/operations/1/retryable`. The press path with 3 is admitted and
// counts `package` 25 (the Lab's revision table, `gate_477_lr1` of actinglab's tests).
#[test]
fn lr1_the_declaration_gate_refuses_each_misplaced_or_out_of_range_count_at_its_pointer() {
    let key_step = json!({"id": "plus", "from": "list", "to": "list",
                          "key": {"key": "back"},
                          "unguarded_trusted_coordinate": true,
                          "expect_after": {"page_id": "list", "timeout_ms": ARRIVAL_MS},
                          "presses": 3});
    let mut leaving = press_step(json!(3));
    leaving["to"] = json!("done");
    leaving["expect_after"]["page_id"] = json!("done");
    let mut retried = press_step(json!(3));
    retried["retryable"] = json!(true);
    retried["max_attempts"] = json!(2);
    retried["retry_interval_ms"] = json!(10);
    for (case, plus, pointer) in [
        ("a key step", key_step, "/operations/1/presses"),
        (
            "a step that leaves its page",
            leaving,
            "/operations/1/presses",
        ),
        (
            "a constant of 0",
            press_step(json!(0)),
            "/operations/1/presses",
        ),
        (
            "a constant of 1001",
            press_step(json!(1001)),
            "/operations/1/presses",
        ),
        (
            "a key outside the parameter prefix",
            press_step(json!({"fact_key": "session.example.turns"})),
            "/operations/1/presses/fact_key",
        ),
        (
            "a minus of 1001",
            press_step(json!({"fact_key": TURNS_KEY, "minus": 1001})),
            "/operations/1/presses/minus",
        ),
        (
            "a press step with a retry",
            retried,
            "/operations/1/retryable",
        ),
    ] {
        let error = match load(&press_control(), &press_task(plus)) {
            Ok(_) => panic!("{case}: admitted"),
            Err(error) => error,
        };
        assert_eq!(
            error.code().as_str(),
            SharedCode::ResourceDeclarationInvalid.as_str(),
            "{case}: {error}"
        );
        let issue = error
            .declaration_issue()
            .unwrap_or_else(|| panic!("{case}: the declaration issue"));
        assert_eq!(issue.field_path, pointer, "{case}");
        assert_eq!(
            issue.reason,
            ResourceDeclarationReason::InvalidValue,
            "{case}"
        );
    }
    admitted(
        "the press path",
        &press_control(),
        &press_task(press_step(json!(3))),
    );
}

// Test 6 (the work order's "must not change"): a package without the member, the plain path of
// KP-1's gate as its test of a device without key input runs it (`open` from home into the list,
// then the trusted tap `back` from the list to done, its control `neutral.linear.key`), runs as
// before: Completed on done after both steps, two taps and no fact read, its step and effect
// records byte for byte the golden below.
#[test]
fn lr1_a_package_without_the_member_runs_byte_for_byte_as_before() {
    const GOLDEN: [&str; 8] = [
        "step_started 0 open neutral/home",
        r#"effect_intent 0 open {"kind":"tap","x":0,"y":0} sampling=none guard=trusted_coordinate"#,
        "effect_completed 0 open",
        "step_finished 0 open neutral/list",
        "step_started 1 back neutral/list",
        r#"effect_intent 1 back {"kind":"tap","x":0,"y":0} sampling=none guard=trusted_coordinate"#,
        "effect_completed 1 back",
        "step_finished 1 back neutral/done",
    ];
    let plain = task(
        "home",
        "done",
        vec![tap("open", "home", "list"), tap("back", "list", "done")],
    );
    let prepared = admitted("no member", &control("neutral.linear.key", 2), &plain);
    let screens = vec![
        vec![screen(HOME, false)],
        vec![screen(LIST, true)],
        vec![screen(DONE, false)],
    ];
    let mut device = FactDevice::new(screens, None);

    let outcome = prepared
        .run(&mut device)
        .unwrap_or_else(|error| panic!("expected a normal end, got {error:?}"));

    assert_eq!(outcome, completed(DONE_PAGE, 2));
    assert_eq!(step_lines(&device.script), GOLDEN);
    assert_eq!(
        device.script.inputs().len(),
        2,
        "{:?}",
        device.script.inputs()
    );
    assert_eq!(device.fact_reads, 0);
}
