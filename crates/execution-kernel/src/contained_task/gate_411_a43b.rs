// SPDX-License-Identifier: AGPL-3.0-only

// #411 A43b (exec gate keys 133 to 135; register §1.160; Workflow #411 A43, coordinator 10-21,
// Q8 of 10-21 20:2x; brief A43b): a wait that sees a still screen it cannot recognise is not
// stretched by the step's pacing factor. One rule for the page wait (`capture_until_page`) and
// the step's confirmation (`await_postcondition`):
//
// - "still and unrecognised" holds while the latest two frames the wait captured are unchanged
//   by BT-B12's frame-change measure (the whole frame's colour digest, still at most 1500 milli)
//   and neither matched a page the wait accepts;
// - while it holds, the rest of the wait is counted unpaced (the factor 1000); otherwise the
//   wait is paced as before (#385 P-k: each wait lasts its value × f / 1000, every limit is
//   budget time);
// - a recognised page, or a change, returns to that at once;
// - a wait that runs out ends with the same code and the same values.
//
// Every expected value comes from those lines and the package values below, never from the code.
// The runs play scripted frames on the scripted clock (EXEC-I4) at the factor 1250 (the pressure
// floor's, as in the A43 case), without an extra wait round. A frame's left pixel names its page
// (`home`, `result`; any other colour is no page) and its right pixel is dark. The package waits
// for a page every 300 ms for at most 1500 ms, and confirms its one click on the same values. At
// 1250 an interval paced is a pause of 375 ms that counts 300 ms; counted unpaced it is a pause of
// 300 ms.

use super::tests::scripted::{ScriptedFrame, ScriptedRuntime};
use super::*;
use actingcommand_recognition_pack::RecognitionPack;
use serde_json::json;
use std::path::PathBuf;

const PAGE_UNKNOWN: &str = SharedCode::ContainedTaskPageUnknown.as_str();
const CONFIRMATION_FAILED: &str = ExecutionKernelCode::PageConfirmationFailed.as_str();

// The pressure floor's factor (`performance.pacing.pressure_floor.factor_milli`).
const FACTOR_MILLI: u32 = 1_250;
const TASK_TIMEOUT_MS: u64 = 60_000;
// The page wait (`capture_interval_ms`, `step_timeout_ms`), which the click's confirmation takes
// too (it declares no `expect_after` values).
const INTERVAL_MS: u64 = 300;
const WAIT_MS: u64 = 1_500;
// One interval at 1250: 300 ms × 1250 / 1000.
const PACED_INTERVAL_MS: u64 = 375;

const HOME: [u8; 3] = [255, 0, 0];
const RESULT: [u8; 3] = [0, 255, 0];
const DARK: [u8; 3] = [0, 0, 0];
const GREY: [u8; 3] = [128, 128, 128];

// The wait under test.
#[derive(Debug, Clone, Copy)]
enum Site {
    // The run's first page wait: it accepts any page of the package.
    PageWait,
    // The confirmation after the click of `open`: it accepts `result`.
    Confirmation,
}

impl Site {
    const BOTH: [Self; 2] = [Self::PageWait, Self::Confirmation];

    // The code a run ends with when this wait runs out.
    fn ran_out_code(self) -> &'static str {
        match self {
            Self::PageWait => PAGE_UNKNOWN,
            Self::Confirmation => CONFIRMATION_FAILED,
        }
    }

    // The page this wait accepts, which the device shows in `toward` once it arrives.
    fn accepted(self) -> [u8; 3] {
        match self {
            Self::PageWait => HOME,
            Self::Confirmation => RESULT,
        }
    }

    // The device's script: the wait sees `seen`, the page wait from the run's first frame on, the
    // confirmation after the click (before it, `home`). After a click the page wait's run sees
    // `result`.
    fn script(self, seen: Vec<ScriptedFrame>) -> Vec<Vec<ScriptedFrame>> {
        match self {
            Self::PageWait => vec![seen, vec![shown(RESULT)]],
            Self::Confirmation => vec![vec![shown(HOME)], seen],
        }
    }
}

// A frame whose left pixel is `left`.
fn shown(left: [u8; 3]) -> ScriptedFrame {
    ScriptedFrame::rgb(2, 1, &[left, DARK])
}

// A frame on the way to `page`: its left pixel is `page` with every channel at most `level`, no
// page of the package.
fn toward(page: [u8; 3], level: u8) -> ScriptedFrame {
    shown(page.map(|channel| channel.min(level)))
}

fn ms(values: &[u64]) -> Vec<Duration> {
    values.iter().copied().map(Duration::from_millis).collect()
}

// Two pages told apart by the left pixel.
fn pack() -> RecognitionPack {
    let target = |id: &str, expected: [u8; 3]| {
        json!({"type": "color", "id": id,
               "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": expected})
    };
    serde_json::from_value(json!({
        "schema_version": "0.7",
        "game": "neutral",
        "server": "test",
        "coordinate_space": {"width": 2, "height": 1},
        "defaults": {"color_max_distance": 0.0},
        "targets": [target("page/home", HOME), target("page/result", RESULT)]
    }))
    .expect("recognition pack")
}

fn page_set() -> PageSet {
    serde_json::from_value(json!({
        "schema_version": "0.3",
        "pages": [
            {"id": "home", "required": ["page/home"]},
            {"id": "result", "required": ["page/result"]}
        ]
    }))
    .expect("page set")
}

// A page-graph task that ends on `result`: one click `open` from `home` to `result` without a
// retry policy. Its page wait and its confirmation take a frame every `INTERVAL_MS` for at most
// `WAIT_MS`, within `TASK_TIMEOUT_MS`.
fn task() -> PreparedContainedTask {
    let control: TaskControl = serde_json::from_value(json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": "neutral.still.task",
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": 2, "height": 1},
        "entry_task_id": "task",
        "capture_interval_ms": INTERVAL_MS,
        "step_timeout_ms": WAIT_MS,
        "timeout_ms": TASK_TIMEOUT_MS
    }))
    .expect("task control");
    let program: TaskProgram = serde_json::from_value(json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": 2, "height": 1},
        "target_page": "result",
        "operations": [{
            "id": "open",
            "from": "home",
            "to": "result",
            "click": {"kind": "point", "x": 1, "y": 0},
            "unguarded_trusted_coordinate": true
        }]
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

type RunResult = Result<ContainedTaskOutcome, ContainedTaskRunError<&'static str>>;

// Runs the task on `screens`, every step at `FACTOR_MILLI` without an extra round.
fn paced_run(screens: Vec<Vec<ScriptedFrame>>) -> (RunResult, ScriptedRuntime) {
    let mut runtime = ScriptedRuntime::new(screens).with_step_factors(vec![FACTOR_MILLI]);
    let result = task().run(&mut runtime);
    (result, runtime)
}

// The run ended as `site`'s wait ends when it runs out: its code, with the wait's elapsed time and
// limit both `WAIT_MS`, as a paced wait that runs out reports them.
fn assert_ran_out(site: Site, result: RunResult) {
    let error = match result {
        Err(ContainedTaskRunError::Task(error)) => error,
        other => panic!("{site:?}: expected the wait to run out, got {other:?}"),
    };
    assert_eq!(error.code().as_str(), site.ran_out_code(), "{site:?}");
    let timing = error.timing().expect("the wait's timing");
    assert_eq!(
        (timing.elapsed_ms, timing.limit_ms),
        (WAIT_MS, WAIT_MS),
        "{site:?}: the wait's elapsed time and limit"
    );
}

// T1 (exec key 133): the wait sees the same frame of no page throughout. Its first interval is
// paced (one frame cannot be still); from the second frame on the two latest frames are still and
// unrecognised, so the rest of the wait is counted unpaced: four pauses of 300 ms, and the wait
// ends after 375 + 4 × 300 = 1575 ms of the runtime's clock, not 5 × 375 = 1875 ms. The run ends
// with the wait's own code and values.
#[test]
fn a43b_t1_a_still_unrecognised_screen_ends_the_wait_at_the_unpaced_budget() {
    for site in Site::BOTH {
        let (result, runtime) = paced_run(site.script(vec![shown(DARK)]));
        assert_eq!(
            runtime.pauses(),
            ms(&[
                PACED_INTERVAL_MS,
                INTERVAL_MS,
                INTERVAL_MS,
                INTERVAL_MS,
                INTERVAL_MS
            ]),
            "{site:?}: the rest of the wait after the second still frame is unpaced"
        );
        assert_ran_out(site, result);
    }
}

// T2 (exec key 134): the wait sees frames of no page that change on every frame. The two latest
// frames are never still, so the wait is paced as before: five pauses of 375 ms, 1875 ms of the
// runtime's clock. The run ends with the wait's own code and values.
#[test]
fn a43b_t2_a_changing_unrecognised_screen_ends_the_wait_at_the_paced_budget() {
    for site in Site::BOTH {
        let changing = (0..8)
            .map(|frame| shown(if frame % 2 == 0 { DARK } else { GREY }))
            .collect();
        let (result, runtime) = paced_run(site.script(changing));
        assert_eq!(
            runtime.pauses(),
            ms(&[PACED_INTERVAL_MS; 5]),
            "{site:?}: a changing screen keeps the wait paced"
        );
        assert_ran_out(site, result);
    }
}

// T3 (exec key 135): the wait sees two still frames of no page, then frames that change toward
// the page it accepts (the second of them delivered 650 ms after its capture starts), then that
// page. The change returns the wait to its pacing, so those 650 ms count 520 ms and the page,
// on the fifth frame, arrives within the wait (at 1500 of its 1500 ms). Had the wait stayed
// unpaced after the change, it would have counted 300 + 300 + 300 + 650 = 1550 ms by the fourth
// frame and ended before the page. The page is reached and the run succeeds with its one click.
#[test]
fn a43b_t3_a_change_toward_the_page_returns_the_wait_to_its_pacing() {
    for site in Site::BOTH {
        let page = site.accepted();
        let (result, runtime) = paced_run(site.script(vec![
            shown(DARK),
            shown(DARK),
            toward(page, 64),
            toward(page, 128).delivered_after(Duration::from_millis(650)),
            shown(page),
        ]));
        assert!(
            matches!(&result, Ok(outcome) if matches!(outcome.outcome, TaskOutcome::Success)),
            "{site:?}: the page is reached: {result:?}"
        );
        assert_eq!(runtime.inputs().len(), 1, "{site:?}: one click");
    }
}
