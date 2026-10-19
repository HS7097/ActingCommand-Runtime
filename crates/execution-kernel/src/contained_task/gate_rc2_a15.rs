// SPDX-License-Identifier: AGPL-3.0-only

// #411 A15 (v0.12.1-rc.2; exec gate key 88): a page-graph step whose postcondition is not
// confirmed and whose operation decides to fail ends the run with
// `contained_task_step_unconfirmed`, which carries the operation (`pack_operation_id`), its
// destination pages (`page_ids`), its attempts (`attempts`) and the page the last frame of the
// wait matched (`page_id`), absent when that frame matched no page. The developer detail keeps
// `after_page`, the last frame that matched a page, which the failure identity reads. The same
// holds when an observation window ends without its destination.
//
// The runs play scripted frames on the scripted clock (EXEC-I4): a frame's left pixel names its
// page (none for a dark one). One trusted click goes from `page_a` to `page_b`; `page_c` is a
// page of the package the step does not expect. The operation is not retried, so one attempt.
// Every expected value comes from the requirement lines of #411 A15, never from the code.

use super::tests::scripted::{ScriptedFrame, ScriptedRuntime};
use super::*;
use actingcommand_recognition_pack::RecognitionPack;
use serde_json::{Value, json};
use std::path::PathBuf;

const STEP_NOT_CONFIRMED: &str = ExecutionKernelCode::ContainedTaskStepUnconfirmed.as_str();

const PAGE_A: [u8; 3] = [255, 0, 0];
const PAGE_B: [u8; 3] = [0, 0, 255];
const PAGE_C: [u8; 3] = [255, 255, 0];
const DARK: [u8; 3] = [0, 0, 0];

// What the device shows.
#[derive(Debug, Clone, Copy)]
enum Screen {
    // The step's departure page.
    A,
    // The step's destination.
    B,
    // A page of the package the step does not expect.
    C,
    // No page of the package.
    Blank,
}

fn shown(screen: Screen) -> ScriptedFrame {
    let page = match screen {
        Screen::A => PAGE_A,
        Screen::B => PAGE_B,
        Screen::C => PAGE_C,
        Screen::Blank => DARK,
    };
    ScriptedFrame::rgb(2, 1, &[page, DARK])
}

fn frames(screens: &[Screen]) -> Vec<ScriptedFrame> {
    screens.iter().copied().map(shown).collect()
}

// The three pages, told apart by the left pixel.
fn pack() -> RecognitionPack {
    let colour = |id: &str, expected: [u8; 3]| {
        json!({"type": "color", "id": id,
               "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": expected})
    };
    serde_json::from_value(json!({
        "schema_version": "0.7",
        "game": "neutral",
        "server": "test",
        "coordinate_space": {"width": 2, "height": 1},
        "defaults": {"color_max_distance": 0.0},
        "targets": [
            colour("page/a", PAGE_A),
            colour("page/b", PAGE_B),
            colour("page/c", PAGE_C)
        ]
    }))
    .expect("recognition pack")
}

fn page_set() -> PageSet {
    serde_json::from_value(json!({
        "schema_version": "0.3",
        "pages": [
            {"id": "page_a", "required": ["page/a"]},
            {"id": "page_b", "required": ["page/b"]},
            {"id": "page_c", "required": ["page/c"]}
        ]
    }))
    .expect("page set")
}

// One trusted click from `page_a` to `page_b`, not retried, with `extra` members.
fn task(extra: Value) -> PreparedContainedTask {
    let control: TaskControl = serde_json::from_value(json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": "neutral.unconfirmed.task",
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": 2, "height": 1},
        "entry_task_id": "task",
        "capture_interval_ms": 100,
        "step_timeout_ms": 1_000,
        "timeout_ms": 60_000
    }))
    .expect("task control");
    let mut operation = json!({
        "id": "open_b",
        "from": "page_a",
        "to": "page_b",
        "click": {"kind": "point", "x": 1, "y": 0},
        "unguarded_trusted_coordinate": true,
        "retryable": false
    });
    for (member, value) in extra.as_object().expect("extra members") {
        operation[member] = value.clone();
    }
    let program: TaskProgram = serde_json::from_value(json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": 2, "height": 1},
        "target_page": "page_b",
        "operations": [operation]
    }))
    .expect("task program");
    for operation in &program.operations {
        operation
            .validate(&control, TaskOperationDefaults::default(), "0.9")
            .expect("an admitted operation");
    }
    let evaluator =
        RecognitionEvaluator::new(PathBuf::new(), pack()).expect("recognition evaluator");
    let detector = PageDetector::new(page_set()).expect("page detector");
    detector
        .validate(&evaluator)
        .expect("page detector targets");
    PreparedContainedTask {
        control,
        program,
        evaluator,
        detector,
        entry_page: None,
        scheduling_outcome: None,
        post_admission_ocr: None,
        post_admission_fields: None,
        entry_count: 1,
        task_count: 1,
    }
}

// The run's failure when `after_input` shows after the click (`page_a` before it).
fn unconfirmed(task: &PreparedContainedTask, after_input: &[Screen]) -> ContainedTaskError {
    let mut runtime = ScriptedRuntime::new(vec![frames(&[Screen::A]), frames(after_input)]);
    let error = match task.run(&mut runtime) {
        Err(ContainedTaskRunError::Task(error)) => error,
        Err(other) => panic!("expected a task failure, got {other:?}"),
        Ok(outcome) => panic!("expected a task failure, the run ended {outcome:?}"),
    };
    assert_eq!(error.code().as_str(), STEP_NOT_CONFIRMED, "{error:?}");
    assert_eq!(runtime.inputs().len(), 1, "one attempt, one input");
    error
}

fn values(error: &ContainedTaskError) -> Value {
    Value::Object(error.outcome().values().clone())
}

// Requirement 1 (i): the tap was ignored, the screen stays on the departure page.
#[test]
fn a15_an_ignored_tap_names_the_departure_page() {
    let error = unconfirmed(&task(json!({})), &[Screen::A]);
    assert_eq!(
        values(&error),
        json!({
            "pack_operation_id": "open_b",
            "page_ids": ["page_b"],
            "attempts": 1,
            "page_id": "page_a"
        })
    );
}

// Requirement 1 (ii): the screen moved to a page the step does not expect.
#[test]
fn a15_a_move_to_another_page_names_it() {
    let error = unconfirmed(&task(json!({})), &[Screen::A, Screen::A, Screen::C]);
    assert_eq!(
        values(&error),
        json!({
            "pack_operation_id": "open_b",
            "page_ids": ["page_b"],
            "attempts": 1,
            "page_id": "page_c"
        })
    );
}

// Requirements 1-3 (iii): the last frame matched no page, so no `page_id`; the detail still
// names the last frame that matched one.
#[test]
fn a15_an_unrecognised_last_frame_names_no_page() {
    let error = unconfirmed(&task(json!({})), &[Screen::A, Screen::A, Screen::Blank]);
    assert_eq!(
        values(&error),
        json!({
            "pack_operation_id": "open_b",
            "page_ids": ["page_b"],
            "attempts": 1
        })
    );
    let detail = error.detail().expect("the developer detail");
    assert!(
        detail.split(' ').any(|token| token == "after_page=page_a"),
        "{detail}"
    );
}

// Requirement 2: an observation window that ends without its destination names the page of its
// last frame. The step arrives on `page_b`, the screen goes back to `page_a` for the rest of the
// window and the confirmation budget after it.
#[test]
fn a15_a_window_end_without_its_destination_names_the_last_page() {
    let window = task(json!({"observation_window": {"until": "window_end", "window_ms": 300}}));
    let error = unconfirmed(&window, &[Screen::B, Screen::A]);
    assert_eq!(
        values(&error),
        json!({
            "pack_operation_id": "open_b",
            "page_ids": ["page_b"],
            "attempts": 1,
            "page_id": "page_a"
        })
    );
}
