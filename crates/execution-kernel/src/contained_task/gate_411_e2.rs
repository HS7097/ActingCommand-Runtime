// SPDX-License-Identifier: AGPL-3.0-only

// #411 E2 T1 (A28, 0.12.9): a run that exhausts its step budget ends with its own registered code,
// `contained_task_step_budget_exhausted` (error), not the code of a run that only a scheduler can
// go on with (`contained_task_requires_scheduler`). Two runs exhaust their budget of three steps:
// a page-graph run whose operation never reaches the target page, and a run whose stability
// declaration's step limit (equal to the run's `max_steps`) is reached before its screen holds
// still. Every expected value comes from the #411 A28 requirement line of the slice brief.
//
// The runs play scripted frames on the scripted clock (EXEC-I4): the left pixel names the page,
// the right pixel is the stability region.

use super::tests::scripted::{ScriptedFrame, ScriptedRuntime};
use super::*;
use actingcommand_contract::outcome::Category;
use actingcommand_recognition_pack::RecognitionPack;
use serde_json::{Value, json};
use std::path::PathBuf;

const STEP_BUDGET_EXHAUSTED: &str = "contained_task_step_budget_exhausted";
const MAX_STEPS: u32 = 3;

const PAGE_A: [u8; 3] = [255, 0, 0];
const PAGE_B: [u8; 3] = [0, 0, 255];

// The page `page_a` with `region` as the right pixel.
fn page_a(region: [u8; 3]) -> ScriptedFrame {
    ScriptedFrame::rgb(2, 1, &[PAGE_A, region])
}

// Two pages, told apart by the left pixel.
fn pack() -> RecognitionPack {
    let colour = |id: &str, expected: [u8; 3]| {
        json!({"type": "color", "id": id,
               "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": expected})
    };
    serde_json::from_value(json!({
        "schema_version": "0.3",
        "game": "neutral",
        "server": "test",
        "coordinate_space": {"width": 2, "height": 1},
        "defaults": {"color_max_distance": 0.0},
        "targets": [colour("page/a", PAGE_A), colour("page/b", PAGE_B)]
    }))
    .expect("recognition pack")
}

fn page_set() -> PageSet {
    serde_json::from_value(json!({
        "schema_version": "0.3",
        "pages": [
            {"id": "page_a", "required": ["page/a"]},
            {"id": "page_b", "required": ["page/b"]}
        ]
    }))
    .expect("page set")
}

// The stability declaration over the right pixel, its step limit the run's budget.
fn stability() -> Value {
    json!({
        "region": {"x": 1, "y": 0, "width": 1, "height": 1},
        "comparison": {"mode": "exact_pixels_v1", "parameters": {}},
        "consecutive_unchanged_threshold": 2,
        "max_steps": MAX_STEPS
    })
}

// One trusted click on `page_a` with no destination, a budget of three steps. A page-graph run
// waits for `page_b`; a stability run waits for its screen to hold still.
fn task(stability_run: bool) -> PreparedContainedTask {
    let mut control = json!({
        "schema_version": "Lab-1y.control.v1",
        "package_id": "neutral.step_budget.task",
        "execution_mode": if stability_run { "in_page_guard" } else { "navigable_route" },
        "game": "neutral",
        "server": "test",
        "resolution": {"width": 2, "height": 1},
        "entry_task_id": "task",
        "capture_interval_ms": 100,
        "step_timeout_ms": 1_000,
        "timeout_ms": 60_000,
        "max_steps": MAX_STEPS
    });
    let mut program = json!({
        "schema_version": "0.6",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": 2, "height": 1},
        "operations": [{
            "id": "tap_a",
            "from": "page_a",
            "click": {"kind": "point", "x": 1, "y": 0},
            "unguarded_trusted_coordinate": true,
            "retryable": false
        }]
    });
    if stability_run {
        control["stability_termination"] = stability();
        program["stability_termination"] = stability();
    } else {
        program["target_page"] = json!("page_b");
    }
    let control: TaskControl = serde_json::from_value(control).expect("task control");
    let program: TaskProgram = serde_json::from_value(program).expect("task program");
    for operation in &program.operations {
        operation
            .validate(&control, TaskOperationDefaults::default(), "0.6")
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

// Requirement A28: a run that exhausts its step budget reports the step budget's own code, an
// error, after the budget's steps.
#[test]
fn e2_t1_a_run_that_exhausts_its_step_budget_ends_with_the_step_budget_code() {
    for (case, stability_run, screens) in [
        (
            "page-graph run that never reaches its target page",
            false,
            vec![vec![page_a([0, 0, 0])]],
        ),
        (
            "stability run whose screen changes after every step",
            true,
            (0..=MAX_STEPS)
                .map(|step| {
                    let shade = u8::try_from(step * 40).expect("shade");
                    vec![page_a([shade, shade, shade])]
                })
                .collect(),
        ),
    ] {
        let task = task(stability_run);
        let mut runtime = ScriptedRuntime::new(screens);
        let error = match task.run(&mut runtime) {
            Err(ContainedTaskRunError::Task(error)) => error,
            Err(other) => panic!("{case}: expected a task failure, got {other:?}"),
            Ok(outcome) => panic!("{case}: expected a task failure, the run ended {outcome:?}"),
        };
        assert_eq!(
            runtime.inputs().len(),
            MAX_STEPS as usize,
            "{case}: the budget's steps ran"
        );
        assert_eq!(
            error.code().as_str(),
            STEP_BUDGET_EXHAUSTED,
            "{case}: {error:?}"
        );
        assert_eq!(error.outcome().category(), Category::Error, "{case}");
    }
}
