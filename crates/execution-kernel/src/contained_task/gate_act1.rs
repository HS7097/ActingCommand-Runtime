// SPDX-License-Identifier: AGPL-3.0-only

// ACT-1 (Workflow #407, register §1.250; design `rd-1031/check-speed/model-check.md` §1.6 and §7;
// coordinator brief `brief-slices.md`): the settled path of a step after its input.
//
// - Off (`declared`, no settled path handed): the order before it, event by event.
// - On: a tap reads frames from the first frame after the input, one `poll` apart, and goes on the
//   first destination frame; `post_delay_ms` is the limit of that reading and never adds to it. At
//   the limit the confirmation runs as it always did, its own timeout from there. A frame of the
//   declared wait page goes to the confirmation at once, which waits for the destination with no
//   fixed time.
// - A step whose first frame would prove nothing keeps `post_delay_ms`: a destination that names the
//   source page, a designated effect, an observation window.
// - A failed confirmation keeps the run's later attempts on `post_delay_ms` and holds the operation
//   for `fallback_hold_ms`, across runs; no input is ever added.
// - `settle_frames` 2 takes two consecutive destination frames that did not change.
//
// ACT-2 (design addendum 1): a `fixed` or `random` gap comes before the frame that decides the input
// and its guard, is asked once per step whatever is verified again, adds to the pacing term and is
// never multiplied by the factor.
//
// Every expected value comes from those lines and the package values below. The runs play scripted
// frames on the scripted clock (EXEC-I4) at the factor 1000: the device shows `screens[n]` after
// `n` inputs, a frame is delivered at once, and every pause moves the clock by its length. A frame's
// left pixel names its page (`home`, `lobby`, `result`; dark is no page), and a grey right pixel is
// the target `flag/ready`. The package declares no
// capture interval, so a confirmation takes a frame every 50 ms; the back-off is off.

use super::tests::scripted::{ScriptedEvent, ScriptedFrame, ScriptedRuntime};
use super::*;
use actingcommand_recognition_pack::RecognitionPack;
use serde_json::{Value, json};
use std::path::PathBuf;

const CONFIRMATION_FAILED: &str = ExecutionKernelCode::PageConfirmationFailed.as_str();

// The package's `post_delay_ms`.
const POST_DELAY_MS: u64 = 200;
const POST_DELAY: Duration = Duration::from_millis(POST_DELAY_MS);
// `poll_ms` 50, `settle_frames` 1, `settle_change_max_milli` 500.
const POLL_MS: u64 = 50;
const PATH: SettledPath = SettledPath::new(Duration::from_millis(POLL_MS), 1, 500);
// `fallback_hold_ms`.
const HOLD: Duration = Duration::from_millis(60_000);

const HOME: [u8; 3] = [255, 0, 0];
const LOBBY: [u8; 3] = [0, 0, 255];
const RESULT: [u8; 3] = [0, 255, 0];
const DARK: [u8; 3] = [0, 0, 0];
const GREY: [u8; 3] = [128, 128, 128];

type RunResult = Result<ContainedTaskOutcome, ContainedTaskRunError<&'static str>>;

// A frame whose left pixel is `left`.
fn shown(left: [u8; 3]) -> ScriptedFrame {
    ScriptedFrame::rgb(2, 1, &[left, DARK])
}

fn ms(values: &[u64]) -> Vec<Duration> {
    values.iter().copied().map(Duration::from_millis).collect()
}

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
        "targets": [
            target("page/home", HOME),
            target("page/lobby", LOBBY),
            target("page/result", RESULT),
            {"type": "color", "id": "flag/ready",
             "region": {"x": 1, "y": 0, "width": 1, "height": 1}, "expected": GREY}
        ]
    }))
    .expect("recognition pack")
}

fn page_set() -> PageSet {
    serde_json::from_value(json!({
        "schema_version": "0.3",
        "pages": [
            {"id": "home", "required": ["page/home"]},
            {"id": "lobby", "required": ["page/lobby"]},
            {"id": "result", "required": ["page/result"]}
        ]
    }))
    .expect("page set")
}

// A trusted tap from `from` with the package's `post_delay_ms` and `members`.
fn tap(id: &str, from: &str, members: Value) -> Value {
    let mut operation = json!({"id": id, "from": from,
                               "click": {"kind": "point", "x": 1, "y": 0},
                               "unguarded_trusted_coordinate": true,
                               "post_delay_ms": POST_DELAY_MS});
    for (member, value) in members.as_object().expect("operation members") {
        operation[member] = value.clone();
    }
    operation
}

// The confirmation of a tap: `page_id` within `timeout_ms`.
fn expect(page_id: Value, timeout_ms: u64) -> Value {
    json!({"expect_after": {"page_id": page_id, "timeout_ms": timeout_ms}})
}

// The page-graph task that ends on `result` through `operations`, with `program_members` added to
// its program, each operation admitted as the package's admission admits it.
fn task_with(operations: Vec<Value>, program_members: Value) -> PreparedContainedTask {
    let control: TaskControl = serde_json::from_value(json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": "neutral.settled.task",
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": 2, "height": 1},
        "entry_task_id": "task",
        "timeout_ms": 600_000
    }))
    .expect("task control");
    control.validate().expect("an admitted control");
    let mut program = json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": 2, "height": 1},
        "target_page": "result",
        "operations": operations
    });
    for (member, value) in program_members.as_object().expect("program members") {
        program[member] = value.clone();
    }
    let program: TaskProgram = serde_json::from_value(program).expect("task program");
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

fn task(operations: Vec<Value>) -> PreparedContainedTask {
    task_with(operations, json!({}))
}

// A tap that a guard on `flag/ready` allows, on `home` only.
fn guarded_tap(id: &str, members: Value) -> Value {
    let mut operation = tap(id, "home", members);
    operation["unguarded_trusted_coordinate"] = json!(false);
    operation["guard"] = json!({"page_id": "home", "target_id": "flag/ready",
                                "expected_rect": {"x": 1, "y": 0, "width": 1, "height": 1},
                                "color_probe": "flag/ready"});
    operation
}

// `home` with the guard's target showing.
fn home_ready() -> ScriptedFrame {
    ScriptedFrame::rgb(2, 1, &[HOME, GREY])
}

// The events up to and including the first input.
fn up_to_the_input(runtime: &ScriptedRuntime) -> Vec<ScriptedEvent> {
    let events = runtime.events();
    let input = events
        .iter()
        .position(|event| *event == ScriptedEvent::Input)
        .expect("an input");
    events[..=input].to_vec()
}

// Runs `task` on `screens` with the settled path `path` handed at every step.
fn settled(
    task: &PreparedContainedTask,
    screens: Vec<Vec<ScriptedFrame>>,
    path: SettledPath,
) -> (RunResult, ScriptedRuntime) {
    let mut runtime = ScriptedRuntime::new(screens).with_settled_path(path, HOLD);
    let result = task.run(&mut runtime);
    (result, runtime)
}

// Runs `task` on `screens` with none handed.
fn declared(
    task: &PreparedContainedTask,
    screens: Vec<Vec<ScriptedFrame>>,
) -> (RunResult, ScriptedRuntime) {
    let mut runtime = ScriptedRuntime::new(screens);
    let result = task.run(&mut runtime);
    (result, runtime)
}

fn succeeded(result: &RunResult) {
    assert!(
        matches!(result, Ok(outcome) if matches!(outcome.outcome, TaskOutcome::Success)),
        "expected the run to succeed: {result:?}"
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

// Off: the frame before the input, the input, the whole `post_delay_ms`, then the confirmation's
// frames one interval apart, and nothing between.
#[test]
fn declared_mode_is_byte_for_byte_todays_order() {
    let task = task(vec![tap("open", "home", expect(json!("result"), 1_000))]);
    let (result, runtime) = declared(
        &task,
        vec![vec![shown(HOME)], vec![shown(DARK), shown(RESULT)]],
    );
    succeeded(&result);
    assert_eq!(
        runtime.events(),
        [
            ScriptedEvent::Recognized,
            ScriptedEvent::EffectIntent,
            ScriptedEvent::Input,
            ScriptedEvent::Paused(POST_DELAY),
            ScriptedEvent::Recognized,
            ScriptedEvent::Paused(Duration::from_millis(50)),
            ScriptedEvent::Recognized,
        ]
    );
}

// On: two steps whose destinations show at once take no pause at all; each next step goes on the
// frame its destination showed on. Off, the same run waits both delays.
#[test]
fn settled_goes_on_the_first_destination_frame() {
    let task = task(vec![
        tap("open", "home", expect(json!("lobby"), 1_000)),
        tap("start", "lobby", expect(json!("result"), 1_000)),
    ]);
    let screens = || vec![vec![shown(HOME)], vec![shown(LOBBY)], vec![shown(RESULT)]];
    let (result, runtime) = settled(&task, screens(), PATH);
    succeeded(&result);
    assert_eq!(runtime.inputs().len(), 2);
    assert_eq!(runtime.captures(), 3, "one frame per page");
    assert!(
        runtime.pauses().is_empty(),
        "no blind wait and no other pause: {:?}",
        runtime.pauses()
    );
    let (result, runtime) = declared(&task, screens());
    succeeded(&result);
    assert_eq!(runtime.pauses(), [POST_DELAY, POST_DELAY]);
}

// On: the frames start right after the input, one `poll_ms` apart. Here the poll is 20 ms: the
// third frame after the input, 40 ms on, is the destination's, long before `post_delay_ms`.
#[test]
fn settled_reads_from_the_first_frame_after_the_input() {
    let task = task(vec![tap("open", "home", expect(json!("result"), 1_000))]);
    let (result, runtime) = settled(
        &task,
        vec![
            vec![shown(HOME)],
            vec![shown(DARK), shown(GREY), shown(RESULT)],
        ],
        SettledPath::new(Duration::from_millis(20), 1, 500),
    );
    succeeded(&result);
    assert_eq!(pauses_by_input(&runtime)[1], ms(&[20, 20]));
    assert_eq!(runtime.captures_after(1), 3);
    let after_input = runtime
        .events()
        .iter()
        .position(|event| *event == ScriptedEvent::Input)
        .expect("an input");
    assert_eq!(
        runtime.events()[after_input + 1],
        ScriptedEvent::Recognized,
        "the next event after the input is a frame"
    );
}

// On: a frame of the declared wait page goes to the confirmation at once, which holds on it. The
// device shows the wait page for 500 ms in all, past `post_delay_ms` (200 ms) and the confirmation's
// timeout (100 ms): the step waits for the end of it, one frame per 50 ms, and goes on the
// destination. No pause is the blind wait.
#[test]
fn wait_page_holds_until_the_destination_with_no_fixed_time() {
    let task = task(vec![tap(
        "open",
        "home",
        json!({"expect_after": {"page_id": "result", "timeout_ms": 100,
                                "wait_page": {"page_id": "lobby", "max_ms": 10_000}}}),
    )]);
    let mut after_input = vec![shown(LOBBY); 11];
    after_input[0] = shown(DARK);
    after_input.push(shown(RESULT));
    let (result, runtime) = settled(&task, vec![vec![shown(HOME)], after_input], PATH);
    succeeded(&result);
    assert_eq!(runtime.inputs().len(), 1);
    // The dark frame, then the first wait-page frame ends the reading; the confirmation reads that
    // page again at once and nine times more, the last pause before the destination.
    assert_eq!(pauses_by_input(&runtime)[1], ms(&[50; 10]));
}

// On: a destination that never shows is read up to `post_delay_ms`, four polls; then the
// confirmation runs its whole timeout from there. The time the step takes is the time it takes off:
// the same 200 ms and 1 000 ms, and the run ends with the confirmation's own code and time.
#[test]
fn missing_destination_falls_through_to_the_confirmation_loop_at_post_delay() {
    const TIMEOUT_MS: u64 = 1_000;
    let task = task(vec![tap(
        "open",
        "home",
        expect(json!("result"), TIMEOUT_MS),
    )]);
    let screens = || vec![vec![shown(HOME)], vec![shown(HOME)]];
    let (result, runtime) = settled(&task, screens(), PATH);
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
    assert_eq!(pauses_by_input(&runtime)[1], ms(&[POLL_MS; 24]));
    let (_, off) = declared(&task, screens());
    let total = |runtime: &ScriptedRuntime| runtime.pauses().iter().sum::<Duration>();
    assert_eq!(total(&runtime), total(&off), "never less time than off");
}

// A destination that names the page the step left: its first frame would pass with no sign of the
// input, so the step keeps its `post_delay_ms`.
#[test]
fn same_page_destination_without_target_gone_keeps_post_delay() {
    let task = task(vec![tap(
        "open",
        "home",
        expect(json!(["home", "result"]), 1_000),
    )]);
    let (result, runtime) = settled(&task, vec![vec![shown(HOME)], vec![shown(RESULT)]], PATH);
    succeeded(&result);
    assert_eq!(pauses_by_input(&runtime)[1], [POST_DELAY]);
}

// A designated effect keeps its `post_delay_ms`.
#[test]
fn designated_effect_keeps_post_delay() {
    let task = task_with(
        vec![tap("open", "home", expect(json!("result"), 1_000))],
        json!({"scheduling_outcome": {
            "designated_operation": "open",
            "mappings": [
                {"outcome_key": "done", "effect": "designated_effect_completed",
                 "terminal_pages": ["result"]},
                {"outcome_key": "none", "effect": "no_designated_effect",
                 "terminal_pages": ["result"]}
            ]
        }}),
    );
    let (_, runtime) = settled(&task, vec![vec![shown(HOME)], vec![shown(RESULT)]], PATH);
    assert_eq!(runtime.inputs().len(), 1);
    assert_eq!(pauses_by_input(&runtime)[1], [POST_DELAY]);
}

// A step that declares an observation window keeps its `post_delay_ms`: the blind wait comes first,
// and the window's own frames after it.
#[test]
fn observation_window_step_keeps_post_delay() {
    let task = task(vec![tap(
        "open",
        "home",
        json!({"expect_after": {"page_id": "result", "timeout_ms": 1_000},
               "observation_window": {"until": "window_end", "window_ms": 100}}),
    )]);
    let (_, runtime) = settled(&task, vec![vec![shown(HOME)], vec![shown(RESULT)]], PATH);
    assert_eq!(runtime.inputs().len(), 1);
    assert_eq!(pauses_by_input(&runtime)[1][0], POST_DELAY);
}

// A failed confirmation: the run's next attempt keeps `post_delay_ms`, the next run's step of the
// same operation too, and once `fallback_hold_ms` has passed the path is taken again. One record of
// the fall-back. The first attempt: four polls, the confirmation's two frames apart (100 ms), the
// retry interval and the failure decision's confirmation.
#[test]
fn failure_in_settled_holds_the_operation_on_declared() {
    let mut members = expect(json!("result"), 100);
    members["retryable"] = json!(true);
    members["max_attempts"] = json!(2);
    members["retry_interval_ms"] = json!(10);
    let task = task(vec![tap("open", "home", members)]);
    let mut runtime = ScriptedRuntime::new(vec![
        vec![shown(HOME)],
        vec![shown(HOME)],
        vec![shown(RESULT), shown(HOME)],
        vec![shown(RESULT), shown(HOME)],
        vec![shown(RESULT)],
    ])
    .with_settled_path(PATH, HOLD);

    let first = task.run(&mut runtime);
    succeeded(&first);
    let pauses = pauses_by_input(&runtime);
    assert_eq!(pauses[1], ms(&[50, 50, 50, 50, 50, 50, 10, 50, 50]));
    assert_eq!(pauses[2], [POST_DELAY], "the run's next attempt");
    assert_eq!(runtime.settled_fallbacks(), ["open".to_owned()]);

    let second = task.run(&mut runtime);
    succeeded(&second);
    assert_eq!(
        pauses_by_input(&runtime)[3],
        [POST_DELAY],
        "the next run, within the hold"
    );

    runtime.pause(HOLD);
    let third = task.run(&mut runtime);
    succeeded(&third);
    assert!(
        pauses_by_input(&runtime)[4].is_empty(),
        "the next run, past the hold"
    );
    assert_eq!(runtime.settled_fallbacks().len(), 1);
}

// `settle_frames` 2: a destination frame, one that differs from it, one unchanged from that: the
// step goes on the third, one poll apart each. One frame goes on the first.
#[test]
fn settle_frames_two_requires_an_unchanged_frame() {
    let task = task(vec![tap("open", "home", expect(json!("result"), 1_000))]);
    let with_right = |right: [u8; 3]| ScriptedFrame::rgb(2, 1, &[RESULT, right]);
    let screens = || {
        vec![
            vec![shown(HOME)],
            vec![with_right(DARK), with_right(GREY), with_right(GREY)],
        ]
    };
    let (result, runtime) = settled(
        &task,
        screens(),
        SettledPath::new(Duration::from_millis(POLL_MS), 2, 500),
    );
    succeeded(&result);
    assert_eq!(pauses_by_input(&runtime)[1], ms(&[POLL_MS, POLL_MS]));
    assert_eq!(runtime.captures_after(1), 3);
    let (result, runtime) = settled(&task, screens(), PATH);
    succeeded(&result);
    assert!(pauses_by_input(&runtime)[1].is_empty());
    assert_eq!(runtime.captures_after(1), 1);
}

// The path adds no input: one when the destination shows, one when it never does and the step may
// not retry, `max_attempts` when it may.
#[test]
fn settled_never_adds_an_input() {
    let screens = |after: [u8; 3]| vec![vec![shown(HOME)], vec![shown(after)]];
    let once = task(vec![tap("open", "home", expect(json!("result"), 100))]);
    let (_, runtime) = settled(&once, screens(RESULT), PATH);
    assert_eq!(runtime.inputs().len(), 1);
    let (_, runtime) = settled(&once, screens(HOME), PATH);
    assert_eq!(runtime.inputs().len(), 1);
    let mut members = expect(json!("result"), 100);
    members["retryable"] = json!(true);
    members["max_attempts"] = json!(2);
    members["retry_interval_ms"] = json!(10);
    let twice = task(vec![tap("open", "home", members)]);
    let (_, runtime) = settled(&twice, screens(HOME), PATH);
    assert_eq!(runtime.inputs().len(), 2);
}

// A long gap: the step's arrival frame does not show the target, and a guard on it would refuse.
// The target shows by the end of the gap, and the step decides on the frame after the gap, with no
// wait between that frame and the input. Without a gap the guard judges the arrival frame and
// refuses.
#[test]
fn a_long_gap_decides_on_a_frame_after_the_gap() {
    const GAP: Duration = Duration::from_millis(60_000);
    let task = task(vec![guarded_tap("open", expect(json!("result"), 1_000))]);
    let screens = || vec![vec![shown(HOME), home_ready()], vec![shown(RESULT)]];
    let mut runtime = ScriptedRuntime::new(screens()).with_action_gap(GAP);
    let result = task.run(&mut runtime);
    succeeded(&result);
    assert_eq!(
        up_to_the_input(&runtime),
        [
            ScriptedEvent::Recognized,
            ScriptedEvent::Paused(GAP),
            ScriptedEvent::Recognized,
            ScriptedEvent::EffectIntent,
            ScriptedEvent::Input,
        ]
    );
    let (result, runtime) = declared(&task, screens());
    assert!(result.is_err(), "no gap: the arrival frame decides");
    assert!(runtime.inputs().is_empty());
}

// The page changes during the gap: the step sends no input, with a guard and with none, and ends
// with the guard's refusal.
#[test]
fn a_page_change_during_the_gap_is_seen_by_the_guard() {
    let refused = ExecutionKernelCode::ContainedTaskGuardRefused.as_str();
    for (case, operation) in [
        (
            "a guard",
            guarded_tap("open", expect(json!("result"), 1_000)),
        ),
        (
            "no guard",
            tap("open", "home", expect(json!("result"), 1_000)),
        ),
    ] {
        let task = task(vec![operation]);
        let mut runtime =
            ScriptedRuntime::new(vec![vec![home_ready(), shown(LOBBY)], vec![shown(RESULT)]])
                .with_action_gap(Duration::from_millis(500));
        let error = match task.run(&mut runtime) {
            Err(ContainedTaskRunError::Task(error)) => error,
            other => panic!("{case}: expected a task failure, got {other:?}"),
        };
        assert_eq!(error.code().as_str(), refused, "{case}");
        assert!(runtime.inputs().is_empty(), "{case}");
    }
}

// The guard is verified again on a fresh frame (`input_recheck`), and the step's confirmation fails
// and it clicks again: its gap was asked once, waited once.
#[test]
fn the_random_gap_is_drawn_once_when_the_guard_is_re_verified() {
    const GAP: Duration = Duration::from_millis(500);
    let mut members = expect(json!("result"), 100);
    members["input_recheck"] = json!({"kind": "fresh_frame"});
    members["retryable"] = json!(true);
    members["max_attempts"] = json!(2);
    members["retry_interval_ms"] = json!(10);
    let task = task(vec![guarded_tap("open", members)]);
    let mut runtime = ScriptedRuntime::new(vec![
        vec![home_ready()],
        vec![home_ready()],
        vec![shown(RESULT)],
    ])
    .with_action_gap(GAP);
    let result = task.run(&mut runtime);
    succeeded(&result);
    assert_eq!(runtime.inputs().len(), 2);
    assert_eq!(runtime.action_gap_draws(), 1);
    assert_eq!(
        runtime
            .pauses()
            .iter()
            .filter(|pause| **pause == GAP)
            .count(),
        1
    );
}

// The gap and the pacing term are two waits, the gap first, then the frame, then the term.
#[test]
fn gap_adds_to_the_pacing_term() {
    let gap = Duration::from_millis(40);
    let term = Duration::from_millis(75);
    let task = task(vec![tap("open", "home", expect(json!("result"), 1_000))]);
    let mut runtime = ScriptedRuntime::new(vec![vec![shown(HOME)], vec![shown(RESULT)]])
        .with_action_gap(gap)
        .with_step_extras(0, term);
    let result = task.run(&mut runtime);
    succeeded(&result);
    assert_eq!(
        up_to_the_input(&runtime),
        [
            ScriptedEvent::Recognized,
            ScriptedEvent::Paused(gap),
            ScriptedEvent::Recognized,
            ScriptedEvent::Paused(term),
            ScriptedEvent::EffectIntent,
            ScriptedEvent::Input,
        ]
    );
}

// At the factor 2000 the gap lasts what it says.
#[test]
fn gap_is_never_multiplied_by_the_factor() {
    let gap = Duration::from_millis(40);
    let task = task(vec![tap("open", "home", expect(json!("result"), 1_000))]);
    let mut runtime = ScriptedRuntime::new(vec![vec![shown(HOME)], vec![shown(RESULT)]])
        .with_action_gap(gap)
        .with_step_factors(vec![2_000]);
    let result = task.run(&mut runtime);
    succeeded(&result);
    assert_eq!(
        up_to_the_input(&runtime),
        [
            ScriptedEvent::Recognized,
            ScriptedEvent::Paused(gap),
            ScriptedEvent::Recognized,
            ScriptedEvent::EffectIntent,
            ScriptedEvent::Input,
        ]
    );
}
