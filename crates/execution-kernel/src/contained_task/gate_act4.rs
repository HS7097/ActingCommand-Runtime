// SPDX-License-Identifier: AGPL-3.0-only

// ACT-4 (Workflow #407, register §1.254; design `rd-1031/check-speed/model-check.md` §1.5, §1.6 and
// §7, `model-check-addendum.md` items 5 and 6; coordinator brief `brief-act4.md`):
//
// - `expect_after.target_present` and `target_gone`, two disjoint lists of single-frame template or
//   colour targets of the operation's `from` page or its destinations, admitted with named
//   `program_problem` refusals.
// - One final-state judgement for the settled path, the confirmation and the observation window:
//   the destination page, every `present` passing, every `gone` with a definite non-match. A target
//   that was not evaluated, whose evaluation failed or whose reason leaves it open is not gone.
// - A destination that names the page the step left goes at once on the settled path only on a
//   frame that turned a listed target from the frame that decided the input; a state true before
//   the input keeps `post_delay_ms` and the confirmation.
// - A step without `post_delay_ms` reads on the settled path up to the path's own limit, and its
//   confirmation goes on from what that reading spent: no added wait, no new timeout.
//
// Every expected value comes from those lines and the package values below. The runs play scripted
// frames on the scripted clock (EXEC-I4) at the factor 1000: the device shows `screens[n]` after
// `n` inputs, a frame is delivered at once, and every pause moves the clock by its length. A frame's
// left pixel names its page (`home`, `lobby`, `result`; dark is no page); a grey right pixel is the
// target `mark/popup` of `home`, a white one the target `mark/badge` of `lobby`. The package
// declares no capture interval, so a confirmation takes a frame every 50 ms; the back-off is off.
// The operation `close` taps on `home` and expects `home` again, once per run (`max_runs` 1); `open`
// taps from `home` to `result`, the task's target page.

use super::final_state::{Listed, TargetState, target_state};
use super::tests::scripted::{ScriptedEvent, ScriptedFrame, ScriptedRuntime};
use super::*;
use actingcommand_recognition_pack::{
    RecognitionPack, RecognitionPackCode, RecognitionPackError, load_pack_from_json_str,
};
use serde_json::{Value, json};
use std::path::PathBuf;

const CONFIRMATION_FAILED: &str = ExecutionKernelCode::PageConfirmationFailed.as_str();
const PROGRAM_INVALID: &str = ExecutionKernelCode::ContainedTaskProgramInvalid.as_str();
const TASK_FILE: &str = "tasks/task/task.json";
const MODEL_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

// The package's `post_delay_ms`.
const POST_DELAY_MS: u64 = 200;
const POST_DELAY: Duration = Duration::from_millis(POST_DELAY_MS);
// `poll_ms` 50, `settle_frames` 1, `settle_change_max_milli` 500; `fallback_hold_ms`.
const POLL_MS: u64 = 50;
const PATH: SettledPath = SettledPath::new(Duration::from_millis(POLL_MS), 1, 500);
const HOLD: Duration = Duration::from_millis(60_000);

const HOME: [u8; 3] = [255, 0, 0];
const LOBBY: [u8; 3] = [0, 0, 255];
const RESULT: [u8; 3] = [0, 255, 0];
const DARK: [u8; 3] = [0, 0, 0];
const GREY: [u8; 3] = [128, 128, 128];
const WHITE: [u8; 3] = [255, 255, 255];

type RunResult = Result<ContainedTaskOutcome, ContainedTaskRunError<&'static str>>;

fn frame(left: [u8; 3], right: [u8; 3]) -> ScriptedFrame {
    ScriptedFrame::rgb(2, 1, &[left, right])
}

// `home` with the popup, and without it.
fn popup() -> ScriptedFrame {
    frame(HOME, GREY)
}

fn clear() -> ScriptedFrame {
    frame(HOME, DARK)
}

fn result_frame() -> ScriptedFrame {
    frame(RESULT, DARK)
}

fn ms(values: &[u64]) -> Vec<Duration> {
    values.iter().copied().map(Duration::from_millis).collect()
}

fn pack_json() -> Value {
    let colour = |id: &str, x: u32, expected: [u8; 3]| {
        json!({"type": "color", "id": id,
               "region": {"x": x, "y": 0, "width": 1, "height": 1}, "expected": expected})
    };
    json!({
        "schema_version": "0.7",
        "game": "neutral",
        "server": "test",
        "coordinate_space": {"width": 2, "height": 1},
        "defaults": {"color_max_distance": 0.0},
        "targets": [
            colour("page/home", 0, HOME),
            colour("page/lobby", 0, LOBBY),
            colour("page/result", 0, RESULT),
            colour("mark/popup", 1, GREY),
            colour("mark/badge", 1, WHITE),
            {"type": "ocr", "id": "text/count",
             "region": {"x": 1, "y": 0, "width": 1, "height": 1},
             "languages": ["en"], "timeout_ms": 1000, "match_mode": "exact", "expected": ["0"],
             "case_sensitive": false, "minimum_confidence": 0.5,
             "model_ref": "PP-OCRv6_medium", "model_sha256": MODEL_SHA256},
            {"type": "value_test", "id": "check/count",
             "left": {"target_id": "text/count", "trim": "whitespace_v1",
                      "value": {"type": "unsigned_integer", "min": 0, "max": 999}},
             "op": "ge", "right": {"const": 3}}
        ]
    })
}

fn pack_of(pack: &Value) -> RecognitionPack {
    load_pack_from_json_str(&pack.to_string()).expect("recognition pack")
}

fn pack() -> RecognitionPack {
    pack_of(&pack_json())
}

// `home` names the popup, and with `check` the count check, a value test the runs could not read
// without a text reader; `lobby` names the badge.
fn page_set(check: bool) -> PageSet {
    let mut home = vec!["mark/popup"];
    if check {
        home.push("check/count");
    }
    serde_json::from_value(json!({
        "schema_version": "0.3",
        "pages": [
            {"id": "home", "required": ["page/home"], "optional": home},
            {"id": "lobby", "required": ["page/lobby"], "optional": ["mark/badge"]},
            {"id": "result", "required": ["page/result"]}
        ]
    }))
    .expect("page set")
}

fn control() -> TaskControl {
    let control: TaskControl = serde_json::from_value(json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": "neutral.final_state.task",
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": 2, "height": 1},
        "entry_task_id": "task",
        "timeout_ms": 600_000
    }))
    .expect("task control");
    control.validate().expect("an admitted control");
    control
}

// A trusted tap from `home` to `to`, with the package's `post_delay_ms` and `members`.
fn tap(id: &str, to: &str, members: Value) -> Value {
    let mut operation = json!({"id": id, "from": "home",
                               "click": {"kind": "point", "x": 1, "y": 0},
                               "unguarded_trusted_coordinate": true,
                               "post_delay_ms": POST_DELAY_MS,
                               "expect_after": {"page_id": to, "timeout_ms": 1_000}});
    for (member, value) in members.as_object().expect("operation members") {
        match member.as_str() {
            "expect_after" => {
                for (inner, value) in value.as_object().expect("expect_after members") {
                    operation["expect_after"][inner] = value.clone();
                }
            }
            _ => operation[member] = value.clone(),
        }
    }
    operation
}

// `close`, on `home` and back to it, once per run, with its `expect_after` members `expect`.
fn close(expect: Value) -> Value {
    tap(
        "close",
        "home",
        json!({"expect_after": expect, "max_runs": 1, "on_max_runs": "next_candidate"}),
    )
}

// `open`, from `home` to `result`.
fn open() -> Value {
    tap("open", "result", json!({}))
}

fn program(operations: Vec<Value>) -> TaskProgram {
    serde_json::from_value(json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": 2, "height": 1},
        "target_page": "result",
        "operations": operations
    }))
    .expect("task program")
}

fn evaluator() -> RecognitionEvaluator {
    RecognitionEvaluator::new(PathBuf::new(), pack()).expect("recognition evaluator")
}

// The task's admission of the final-state targets, as package admission runs it.
fn admit_targets(operations: Vec<Value>, pack: RecognitionPack) -> Result<(), ContainedTaskError> {
    let evaluator = RecognitionEvaluator::new(PathBuf::new(), pack).expect("evaluator");
    let detector = PageDetector::new(page_set(true)).expect("page detector");
    program(operations).validate_expected_targets(&control(), &detector, &evaluator, TASK_FILE)
}

// The page-graph task of `operations`, each operation and its targets admitted.
fn task(operations: Vec<Value>) -> PreparedContainedTask {
    let control = control();
    let program = program(operations);
    for operation in &program.operations {
        operation
            .validate(&control, TaskOperationDefaults::default(), "0.9")
            .expect("an admitted operation");
    }
    let evaluator = evaluator();
    let detector = PageDetector::new(page_set(false)).expect("page detector");
    detector
        .validate(&evaluator)
        .expect("page detector targets");
    program
        .validate_expected_targets(&control, &detector, &evaluator, TASK_FILE)
        .expect("admitted final-state targets");
    let scheduling_outcome = program.scheduling_outcome.clone();
    PreparedContainedTask {
        control,
        program,
        evaluator,
        detector,
        entry_page: None,
        scheduling_outcome,
        post_admission_ocr: None,
        post_admission_fields: None,
        entry_count: 1,
        task_count: 1,
    }
}

// Runs `task` on `screens` with the settled path `path` handed at every step, or none.
fn run(
    task: &PreparedContainedTask,
    screens: Vec<Vec<ScriptedFrame>>,
    path: Option<SettledPath>,
) -> (RunResult, ScriptedRuntime) {
    let runtime = ScriptedRuntime::new(screens);
    let mut runtime = match path {
        Some(path) => runtime.with_settled_path(path, HOLD),
        None => runtime,
    };
    let result = task.run(&mut runtime);
    (result, runtime)
}

fn succeeded(case: &str, result: &RunResult) {
    assert!(
        matches!(result, Ok(outcome) if matches!(outcome.outcome, TaskOutcome::Success)),
        "{case}: expected the run to succeed: {result:?}"
    );
}

// The pauses of the run, split at its inputs: `[n]` holds the pauses after the `n`-th input and
// before the next one.
fn pauses_by_input(runtime: &ScriptedRuntime) -> Vec<Vec<Duration>> {
    let mut segments = vec![Vec::new()];
    for event in runtime.events() {
        match event {
            ScriptedEvent::Input => segments.push(Vec::new()),
            ScriptedEvent::Paused(pause) => segments
                .last_mut()
                .expect("a segment of pauses")
                .push(*pause),
            ScriptedEvent::Recognized | ScriptedEvent::EffectIntent => {}
        }
    }
    segments
}

fn value(error: &ContainedTaskError, key: &str) -> Value {
    error.outcome().value(key).cloned().unwrap_or(Value::Null)
}

fn assert_refused(case: &str, refused: Result<(), ContainedTaskError>, problem: &str, at: &str) {
    let error = refused.expect_err(case);
    assert_eq!(error.code().as_str(), PROGRAM_INVALID, "{case}: {error}");
    assert_eq!(value(&error, "program_problem"), json!(problem), "{case}");
    assert_eq!(value(&error, "json_pointer"), json!(at), "{case}");
    assert_eq!(value(&error, "path"), json!(TASK_FILE), "{case}");
}

// The ordinary confirmation (no settled path): after the blind wait, two frames still show the
// popup on `home`, the destination page, and do not end the step; the frame on which the popup is
// definitely gone does. Without `target_gone` the first frame ends it.
#[test]
fn target_gone_keeps_a_same_page_step_waiting_until_it_fails() {
    let screens = || {
        vec![
            vec![popup()],
            vec![popup(), popup(), clear()],
            vec![result_frame()],
        ]
    };
    let task = task(vec![close(json!({"target_gone": ["mark/popup"]})), open()]);
    let (result, runtime) = run(&task, screens(), None);
    succeeded("target_gone", &result);
    assert_eq!(runtime.inputs().len(), 2);
    assert_eq!(runtime.captures_after(1), 3);
    assert_eq!(pauses_by_input(&runtime)[1], ms(&[POST_DELAY_MS, 50, 50]));

    let without = self::task(vec![close(json!({})), open()]);
    let (result, runtime) = run(&without, screens(), None);
    succeeded("no target_gone", &result);
    assert_eq!(runtime.captures_after(1), 1);
    assert_eq!(pauses_by_input(&runtime)[1], [POST_DELAY]);
}

// The settled path: the popup showed on the frame that decided the tap and is definitely gone on
// the first frame after it, so the step goes on that frame with no pause at all. Without
// `target_gone` the destination names the page the step left and the step keeps its blind wait.
#[test]
fn target_gone_lets_a_same_page_step_go_at_once_in_settled() {
    let screens = || vec![vec![popup()], vec![clear()], vec![result_frame()]];
    let task = task(vec![close(json!({"target_gone": ["mark/popup"]})), open()]);
    let (result, runtime) = run(&task, screens(), Some(PATH));
    succeeded("target_gone", &result);
    assert_eq!(runtime.captures_after(1), 1);
    assert!(
        pauses_by_input(&runtime)[1].is_empty(),
        "no blind wait: {:?}",
        pauses_by_input(&runtime)[1]
    );

    let without = self::task(vec![close(json!({})), open()]);
    let (result, runtime) = run(&without, screens(), Some(PATH));
    succeeded("no target_gone", &result);
    assert_eq!(pauses_by_input(&runtime)[1], [POST_DELAY]);
}

// Admission: a target that stands in the rules of neither the operation's `from` page (`home`)
// nor its destination is refused at its entry; the popup of `home` and a target of the
// destination `result` are admitted.
#[test]
fn target_gone_refuses_a_target_outside_from_page() {
    assert_refused(
        "a badge of lobby",
        admit_targets(
            vec![
                close(json!({"target_gone": ["mark/popup", "mark/badge"]})),
                open(),
            ],
            pack(),
        ),
        "expect_target_unreachable",
        "/operations/0/expect_after/target_gone/1",
    );
    admit_targets(
        vec![
            close(json!({"target_gone": ["mark/popup"]})),
            tap(
                "open",
                "result",
                json!({"expect_after": {"target_present": ["page/result"]}}),
            ),
        ],
        pack(),
    )
    .expect("targets of from and of the destination");
}

// Admission: a value test, a lazy target, is refused, though `home` names it; so is a colour
// target under target consensus, judged over several frames. So are a target the pack does not
// have, and one listed in both lists.
#[test]
fn target_gone_refuses_a_lazy_target() {
    assert_refused(
        "a value test",
        admit_targets(
            vec![close(json!({"target_gone": ["check/count"]})), open()],
            pack(),
        ),
        "expect_target_kind",
        "/operations/0/expect_after/target_gone/0",
    );
    let mut consensus = pack_json();
    consensus["target_consensus"] = json!({
        "mark/popup": {"samples": [{"frame": 0}, {"frame": 1}], "k": 2, "sample_interval_ms": 1}
    });
    assert_refused(
        "a target under consensus",
        admit_targets(
            vec![close(json!({"target_present": ["mark/popup"]})), open()],
            pack_of(&consensus),
        ),
        "expect_target_kind",
        "/operations/0/expect_after/target_present/0",
    );
    assert_refused(
        "an unknown target",
        admit_targets(
            vec![close(json!({"target_gone": ["mark/none"]})), open()],
            pack(),
        ),
        "expect_target_unknown",
        "/operations/0/expect_after/target_gone/0",
    );
    assert_refused(
        "both lists",
        admit_targets(
            vec![
                close(json!({"target_present": ["mark/popup"], "target_gone": ["mark/popup"]})),
                open(),
            ],
            pack(),
        ),
        "expect_targets_overlap",
        "/operations/0/expect_after/target_gone/0",
    );
}

// The settled path, a step that brings the popup up (`target_present`): on a frame that decided the
// tap with the popup already up, the final state is no sign of the tap; the step keeps its blind
// wait and the confirmation, as before. With the popup down on that frame and up on the first
// frame after the tap, the step goes at once.
#[test]
fn a_state_true_before_the_click_is_not_this_clicks_effect() {
    let task = task(vec![
        close(json!({"target_present": ["mark/popup"]})),
        open(),
    ]);
    let (result, runtime) = run(
        &task,
        vec![vec![popup()], vec![popup()], vec![result_frame()]],
        Some(PATH),
    );
    succeeded("already up", &result);
    assert_eq!(pauses_by_input(&runtime)[1], [POST_DELAY]);
    assert!(runtime.settled_fallbacks().is_empty());

    let (result, runtime) = run(
        &task,
        vec![vec![clear()], vec![popup()], vec![result_frame()]],
        Some(PATH),
    );
    succeeded("brought up", &result);
    assert!(pauses_by_input(&runtime)[1].is_empty());
}

// A `target_gone` is gone only on a definite non-match: an evaluated colour target that did not
// pass. A target that was not evaluated, an evaluation that failed and one whose registered reason
// leaves it open are none, so a step that lists it never reaches its final state on them.
#[test]
fn an_unevaluated_target_is_not_gone() {
    let evaluator = evaluator();
    let scene = Scene::from_rgb8(2, 1, &[HOME, DARK].concat()).expect("scene");
    let evaluated = evaluator.evaluate_target(&scene, "mark/popup");
    let gone = |result: actingcommand_recognition_pack::RecognitionPackResult<TargetEvaluation>| {
        Listed::Gone.holds(target_state(&result))
    };
    assert_eq!(target_state(&evaluated), TargetState::Unmatched);
    assert!(gone(evaluated.clone()), "an evaluated miss is gone");

    let mut open_reason = evaluated.expect("an evaluation");
    open_reason.failure = Some(Outcome::new(RecognitionPackCode::TargetMatchNotUnique));
    for (case, result) in [
        (
            "not evaluated",
            Ok(TargetEvaluation::not_evaluated(
                "mark/popup",
                TargetKind::Color,
            )),
        ),
        (
            "a failed evaluation",
            Err(RecognitionPackError::fatal("region outside the frame")),
        ),
        ("a reason that leaves it open", Ok(open_reason)),
    ] {
        assert_eq!(target_state(&result), TargetState::Unknown, "{case}");
        assert!(!gone(result), "{case}");
    }
}

// The settled path, the confirmation and the observation window judge one final state: each takes
// the frame on which the popup is definitely gone, never a frame of `home` that still shows it.
// The settled path and the confirmation see the popup once after the tap and go on the next
// frame; the observation window (`window_end`, 100 ms) arrives on a frame without it, then sees it
// twice, and ends only on the next frame without it, 50 ms past the window.
#[test]
fn the_three_paths_share_one_final_state_judgement() {
    let gone = json!({"target_gone": ["mark/popup"]});
    let screens = |after: Vec<ScriptedFrame>| vec![vec![popup()], after, vec![result_frame()]];
    let task_of = |window: bool| {
        let mut close = close(gone.clone());
        if window {
            close["observation_window"] = json!({"until": "window_end", "window_ms": 100});
        }
        task(vec![close, open()])
    };

    let (result, runtime) = run(&task_of(false), screens(vec![popup(), clear()]), Some(PATH));
    succeeded("the settled path", &result);
    assert_eq!(runtime.captures_after(1), 2, "the settled path");
    assert_eq!(pauses_by_input(&runtime)[1], ms(&[POLL_MS]));

    let (result, runtime) = run(&task_of(false), screens(vec![popup(), clear()]), None);
    succeeded("the confirmation", &result);
    assert_eq!(runtime.captures_after(1), 2, "the confirmation");
    assert_eq!(pauses_by_input(&runtime)[1], ms(&[POST_DELAY_MS, 50]));

    let (result, runtime) = run(
        &task_of(true),
        screens(vec![clear(), popup(), popup(), clear()]),
        None,
    );
    succeeded("the observation window", &result);
    assert_eq!(runtime.captures_after(1), 4, "the observation window");
    assert_eq!(
        pauses_by_input(&runtime)[1],
        ms(&[POST_DELAY_MS, 50, 50, 50])
    );
}

// A tap that declares no `post_delay_ms`, on a settled path whose own limit is 200 ms and whose
// poll is 20 ms. Its destination shows on the third frame: the step goes there, one poll apart.
// It never shows: the path reads ten polls to its limit, and the confirmation (timeout 1 000 ms,
// a frame every 50 ms) takes the next frame at once and runs only what is left, 800 ms. No pause
// is a blind wait, and the step's time is the confirmation's timeout, not the limit added to it.
#[test]
fn an_omitted_post_delay_hands_over_without_a_new_wait() {
    const TIMEOUT_MS: u64 = 1_000;
    let path = SettledPath::new(Duration::from_millis(20), 1, 500)
        .with_omitted_delay_limit(Duration::from_millis(200));
    let mut open = open();
    assert_eq!(
        open.as_object_mut()
            .expect("operation")
            .remove("post_delay_ms"),
        Some(json!(POST_DELAY_MS))
    );
    let task = task(vec![open]);

    let (result, runtime) = run(
        &task,
        vec![vec![clear()], vec![clear(), clear(), result_frame()]],
        Some(path),
    );
    succeeded("the destination shows", &result);
    assert_eq!(pauses_by_input(&runtime)[1], ms(&[20, 20]));

    let (result, runtime) = run(&task, vec![vec![clear()], vec![clear()]], Some(path));
    let error = match result {
        Err(ContainedTaskRunError::Task(error)) => error,
        other => panic!("expected a task failure, got {other:?}"),
    };
    assert_eq!(error.code().as_str(), CONFIRMATION_FAILED);
    let timing = error.timing().expect("the confirmation's timing");
    assert_eq!(
        (timing.elapsed_ms, timing.limit_ms),
        (TIMEOUT_MS, TIMEOUT_MS)
    );
    let mut expected = ms(&[20; 10]);
    expected.extend(ms(&[50; 16]));
    assert_eq!(pauses_by_input(&runtime)[1], expected);
    assert_eq!(
        runtime.pauses().iter().sum::<Duration>(),
        Duration::from_millis(TIMEOUT_MS)
    );
}
