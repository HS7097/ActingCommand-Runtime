// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #469 K2 (0.12.62; register §1.396; the owner's option A on board #469; the design
// `rd-1115/status-codes-a-design.md`, Part 2, places 1 ("Page graph"), 2 and 6 and the row K2 of
// its slice table; coordinator work order `rd-1115/brief-469-word-slices.md`): a page-graph step's
// end and a guard's refusal carry a reason word beside their code, a value of a closed vocabulary,
// and nothing else changes.
//
// - `confirmation_problem` on a page-graph step whose confirmation failed
//   (`page_confirmation_failed`, `contained_task_step_unconfirmed`,
//   `contained_task_error_page_reached`): `error_page` on a declared error page, `loading_limit`
//   when the loading allowance ended the wait, `screen_still` when the still limit did, and
//   otherwise the page of the wait's last frame: `nothing_recognised` for none, `input_not_taken`
//   for the step's own page, `other_page_shown` for another page of the package. A run standing on
//   an error page that no operation leaves: `error_page_no_exit`. A wait step that meets a
//   declared error page: `error_page`; one whose `max_ms` ran out on its own page: no word.
// - `contained_task_requires_scheduler` names the failure whose decision asked for the recovery
//   (`failure_code`) and, when that failure was a confirmation's, its `confirmation_problem`.
// - `guard_problem` on `contained_task_guard_refused`: `page_mismatch`, `target_not_passed`,
//   `page_moved`.
// - The word is a value of the outcome only: the code, the detail (whose fragments the failure
//   identity reads) and every other value are what they were.
//
// The runs play scripted frames on the scripted clock (EXEC-I4) at the factor 1000: the device
// shows `screens[n]` after `n` inputs, each segment's last frame repeating, a frame is delivered at
// once unless a test says otherwise, and every pause moves the clock by its length. A frame is
// three pixels: the first names the page (`home`, `lobby`, `result`, the error page `fail`; dark
// is none), the second is the guard's flag (green ready, grey busy) and a white third is the
// loading page `spinner`. The package declares no capture interval (a frame every 50 ms, without
// back-off), its loading pages' allowance is 30 000 ms, and every step goes from `home` to `result`
// with a confirmation of 1 000 ms. Every expected value comes from those lines and the design's
// table of words, never from the code.

use super::tests::scripted::{ScriptedFrame, ScriptedRuntime};
use super::*;
use actingcommand_page_detector::LoadingPageSet;
use actingcommand_recognition_pack::RecognitionPack;
use serde_json::{Map, Value, json};
use std::path::PathBuf;

const CONFIRMATION_FAILED: &str = ExecutionKernelCode::PageConfirmationFailed.as_str();
const STEP_NOT_CONFIRMED: &str = ExecutionKernelCode::ContainedTaskStepUnconfirmed.as_str();
const ERROR_PAGE_CODE: &str = ExecutionKernelCode::ContainedTaskErrorPageReached.as_str();
const REQUIRES_SCHEDULER: &str = ExecutionKernelCode::ContainedTaskRequiresScheduler.as_str();
const GUARD_REFUSED: &str = ExecutionKernelCode::ContainedTaskGuardRefused.as_str();
const TASK_FILE: &str = "resources/operations/task/task.json";
const UNRECOGNIZED: &str = "<unrecognized>";

const CONFIRMATION_MS: u64 = 1_000;
const WAIT_MAX_MS: u64 = 2_000;
const STILL_LIMIT_MS: u64 = 300;
const LOADING_MS: u64 = 200;
const TASK_MS: u64 = 600_000;
const PAGE_WAIT_MS: u64 = 60_000;

const HOME: [u8; 3] = [255, 0, 0];
const LOBBY: [u8; 3] = [0, 0, 255];
const RESULT: [u8; 3] = [0, 255, 0];
const FAIL: [u8; 3] = [255, 0, 255];
const DARK: [u8; 3] = [0, 0, 0];
const READY: [u8; 3] = [0, 255, 0];
const BUSY: [u8; 3] = [128, 128, 128];
const WHITE: [u8; 3] = [255, 255, 255];

// A frame of `page` (dark: none) with the flag `flag` and, with `spinner`, the loading page.
fn frame(page: [u8; 3], flag: [u8; 3], spinner: bool) -> ScriptedFrame {
    ScriptedFrame::rgb(3, 1, &[page, flag, if spinner { WHITE } else { DARK }])
}

// `page` with the flag ready and no spinner.
fn shown(page: [u8; 3]) -> ScriptedFrame {
    frame(page, READY, false)
}

fn pack() -> RecognitionPack {
    let colour = |id: &str, x: u32, expected: [u8; 3]| {
        json!({"type": "color", "id": id,
               "region": {"x": x, "y": 0, "width": 1, "height": 1}, "expected": expected})
    };
    serde_json::from_value(json!({
        "schema_version": "0.7",
        "game": "neutral",
        "server": "test",
        "coordinate_space": {"width": 3, "height": 1},
        "defaults": {"color_max_distance": 0.0},
        "targets": [
            colour("page/home", 0, HOME),
            colour("page/lobby", 0, LOBBY),
            colour("page/result", 0, RESULT),
            colour("page/fail", 0, FAIL),
            colour("flag/ready", 1, READY),
            colour("loading/spinner", 2, WHITE)
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
            {"id": "result", "required": ["page/result"]},
            {"id": "fail", "required": ["page/fail"]}
        ]
    }))
    .expect("page set")
}

fn loading_pages() -> LoadingPageSet {
    serde_json::from_value(json!({"max_ms": 30_000, "pages": [
        {"id": "spinner", "required": ["loading/spinner"]}
    ]}))
    .expect("loading pages")
}

// `operation` with `extra` members added or replaced.
fn with(mut operation: Value, extra: Value) -> Value {
    for (member, value) in extra.as_object().expect("extra members") {
        operation[member] = value.clone();
    }
    operation
}

// `open`: a trusted tap from `home`, confirmed on `result` within `CONFIRMATION_MS`.
fn open(extra: Value) -> Value {
    with(
        json!({"id": "open", "from": "home",
               "click": {"kind": "point", "x": 1, "y": 0},
               "unguarded_trusted_coordinate": true,
               "expect_after": {"page_id": "result", "timeout_ms": CONFIRMATION_MS}}),
        extra,
    )
}

// `claim`: the same tap, guarded by the flag on `home`.
fn claim(extra: Value) -> Value {
    with(
        json!({"id": "claim", "from": "home",
               "click": {"kind": "point", "x": 1, "y": 0},
               "guard": {"page_id": "home", "target_id": "flag/ready",
                         "expected_rect": {"x": 1, "y": 0, "width": 1, "height": 1},
                         "color_probe": "flag/ready"},
               "expect_after": {"page_id": "result", "timeout_ms": CONFIRMATION_MS}}),
        extra,
    )
}

// `hold`: a wait step on `home` for `result`, up to `WAIT_MAX_MS` on its own page and
// `CONFIRMATION_MS` away from it; no input.
fn hold() -> Value {
    json!({"id": "hold", "from": "home", "wait": {"max_ms": WAIT_MAX_MS},
           "expect_after": {"page_id": "result", "timeout_ms": CONFIRMATION_MS}})
}

// Two attempts, 1 ms apart.
fn retried() -> Value {
    json!({"retryable": true, "max_attempts": 2, "retry_interval_ms": 1})
}

// `open`'s confirmation with the loading allowance `LOADING_MS`.
fn loading_confirmation() -> Value {
    json!({"page_id": "result", "timeout_ms": CONFIRMATION_MS, "loading": {"max_ms": LOADING_MS}})
}

// The page-graph task of `operations` that ends on `result`, with the error page `fail`, admitted
// as the package's admission admits it.
fn task(operations: Vec<Value>) -> PreparedContainedTask {
    let control: TaskControl = serde_json::from_value(json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": "neutral.k2.task",
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": 3, "height": 1},
        "entry_task_id": "task",
        "step_timeout_ms": PAGE_WAIT_MS,
        "timeout_ms": TASK_MS
    }))
    .expect("task control");
    control.validate().expect("an admitted control");
    let program: TaskProgram = serde_json::from_value(json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": 3, "height": 1},
        "target_page": "result",
        "error_pages": ["fail"],
        "operations": operations
    }))
    .expect("task program");
    for operation in &program.operations {
        operation
            .validate(&control, TaskOperationDefaults::default(), "0.9")
            .expect("an admitted operation");
    }
    let evaluator =
        RecognitionEvaluator::new(PathBuf::new(), pack()).expect("recognition evaluator");
    let detector = PageDetector::new(page_set())
        .expect("page detector")
        .with_loading_pages(loading_pages())
        .expect("loading pages");
    detector
        .validate(&evaluator)
        .expect("page detector targets");
    program
        .validate_loading(&control, &detector, TASK_FILE)
        .expect("admitted loading declarations");
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

// The step's own frame on `home`, then `after_tap` after the tap.
fn tapped(after_tap: Vec<ScriptedFrame>) -> ScriptedRuntime {
    ScriptedRuntime::new(vec![vec![shown(HOME)], after_tap])
}

// The failure `task` ends with on `runtime`, which made `inputs` inputs.
fn ended(
    case: &str,
    task: &PreparedContainedTask,
    mut runtime: ScriptedRuntime,
    inputs: usize,
) -> ContainedTaskError {
    let error = match task.run(&mut runtime) {
        Err(ContainedTaskRunError::Task(error)) => error,
        Err(other) => panic!("{case}: expected a task failure, got {other:?}"),
        Ok(outcome) => panic!(
            "{case}: expected a task failure, the run ended {:?}",
            outcome.outcome
        ),
    };
    assert_eq!(runtime.inputs().len(), inputs, "{case}: inputs");
    error
}

// A run that ends with a word: its failure, the code it ends with, the detail and the values it had
// before this slice, and the values this slice adds.
struct Fixture {
    case: &'static str,
    error: ContainedTaskError,
    code: &'static str,
    detail: Option<String>,
    before: Value,
    added: Value,
}

// The word `token` of `confirmation_problem`.
fn word(token: &str) -> Value {
    json!({"confirmation_problem": token})
}

// The word `token` of `guard_problem`.
fn guard_word(token: &str) -> Value {
    json!({"guard_problem": token})
}

fn values(error: &ContainedTaskError) -> Value {
    Value::Object(error.outcome().values().clone())
}

// `before` with `added`.
fn joined(before: &Value, added: &Value) -> Value {
    let mut all = before.as_object().cloned().unwrap_or_default();
    all.extend(added.as_object().cloned().unwrap_or_default());
    Value::Object(all)
}

// `values` without the keys of `added`.
fn without(values: &Value, added: &Value) -> Value {
    let added = added.as_object().expect("added values");
    let left: Map<String, Value> = values
        .as_object()
        .expect("an outcome's values")
        .iter()
        .filter(|(key, _)| !added.contains_key(key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    Value::Object(left)
}

// The detail of `page_confirmation_failed` without a retry policy, after one attempt.
fn unpolicied_detail(after_page: &str, hit_error_page: bool) -> String {
    format!("operation=open attempts=1 after_page={after_page} hit_error_page={hit_error_page}")
}

// The detail of a decided confirmation failure after one attempt (Workflow #371-2), with the
// wait's elapsed time and limit when the wait ran out.
fn decided_detail(
    error: &ContainedTaskError,
    operation: &str,
    after_page: &str,
    hit_error_page: bool,
) -> String {
    let timing = error.timing().map_or_else(String::new, |timing| {
        format!(
            " confirm_elapsed_ms={} confirm_limit_ms={}",
            timing.elapsed_ms, timing.limit_ms
        )
    });
    format!(
        "operation={operation} attempts=1 after_page={after_page} hit_error_page={hit_error_page}{timing}"
    )
}

// `open` without a retry policy, `after_tap` shown after its tap: `page_confirmation_failed`.
fn unpolicied(
    case: &'static str,
    after_tap: ScriptedFrame,
    after_page: &str,
    hit_error_page: bool,
    added: Value,
) -> Fixture {
    Fixture {
        case,
        error: ended(
            case,
            &task(vec![open(json!({}))]),
            tapped(vec![after_tap]),
            1,
        ),
        code: CONFIRMATION_FAILED,
        detail: Some(unpolicied_detail(after_page, hit_error_page)),
        before: json!({}),
        added,
    }
}

// `open` deciding to fail (`retryable: false`), `after_tap` shown after its tap:
// `contained_task_step_unconfirmed` with the page of the wait's last frame.
fn decided(
    case: &'static str,
    after_tap: ScriptedFrame,
    page: Option<&str>,
    added: Value,
) -> Fixture {
    let error = ended(
        case,
        &task(vec![open(json!({"retryable": false}))]),
        tapped(vec![after_tap]),
        1,
    );
    let mut before = json!({"pack_operation_id": "open", "page_ids": ["result"], "attempts": 1});
    if let Some(page) = page {
        before["page_id"] = json!(page);
    }
    let detail = decided_detail(&error, "open", page.unwrap_or(UNRECOGNIZED), false);
    Fixture {
        case,
        error,
        code: STEP_NOT_CONFIRMED,
        detail: Some(detail),
        before,
        added,
    }
}

// A still dark screen after the tap under a still limit of `STILL_LIMIT_MS`, its frames 50 ms apart
// without back-off: the frame 300 ms after the first is the first at the limit.
fn still_runtime() -> ScriptedRuntime {
    tapped(vec![shown(DARK)]).with_capture_cadence(
        CaptureCadence::new(60_000, false, 5_000).with_still_limit_ms(STILL_LIMIT_MS),
    )
}

// `extra` members on `open` whose wait ends on the still limit.
fn still(case: &'static str, extra: Value) -> Fixture {
    let decides = extra.get("retryable").is_some();
    let error = ended(case, &task(vec![open(extra)]), still_runtime(), 1);
    let held = json!({"held_ms": STILL_LIMIT_MS, "limit_ms": STILL_LIMIT_MS});
    let (code, before, detail) = if decides {
        (
            STEP_NOT_CONFIRMED,
            joined(
                &json!({"pack_operation_id": "open", "page_ids": ["result"], "attempts": 1}),
                &held,
            ),
            decided_detail(&error, "open", UNRECOGNIZED, false),
        )
    } else {
        (
            CONFIRMATION_FAILED,
            held,
            unpolicied_detail(UNRECOGNIZED, false),
        )
    };
    Fixture {
        case,
        error,
        code,
        detail: Some(detail),
        before,
        added: word("screen_still"),
    }
}

// `extra` members on `open` with the loading allowance `LOADING_MS`, under `on`, `after_tap` (a
// frame under the spinner) shown after its tap: its wait ends on the allowance.
fn loading(
    case: &'static str,
    extra: Value,
    after_tap: ScriptedFrame,
    page: Option<&str>,
) -> Fixture {
    let decides = extra.get("retryable").is_some();
    let operation = with(open(extra), json!({"expect_after": loading_confirmation()}));
    let runtime = tapped(vec![after_tap]).with_loading_mode(LoadingMode::On);
    let error = ended(case, &task(vec![operation]), runtime, 1);
    let spent = json!({"loading_page_id": "spinner", "loading_ms": LOADING_MS,
                       "loading_limit_ms": LOADING_MS});
    let (code, before, detail) = if decides {
        let mut before =
            json!({"pack_operation_id": "open", "page_ids": ["result"], "attempts": 1});
        if let Some(page) = page {
            before["page_id"] = json!(page);
        }
        (
            STEP_NOT_CONFIRMED,
            joined(&before, &spent),
            decided_detail(&error, "open", UNRECOGNIZED, false),
        )
    } else {
        (
            CONFIRMATION_FAILED,
            spent,
            unpolicied_detail(UNRECOGNIZED, false),
        )
    };
    Fixture {
        case,
        error,
        code,
        detail: Some(detail),
        before,
        added: word("loading_limit"),
    }
}

// The step's own page on the first frame after the tap, delivered after the whole confirmation,
// then `then`: the first attempt's wait ends on its one frame, the retry's wait sees `then`.
fn late_then(then: [u8; 3]) -> ScriptedRuntime {
    tapped(vec![
        shown(HOME).delivered_after(Duration::from_millis(2 * CONFIRMATION_MS)),
        shown(then),
    ])
}

fn input_not_taken() -> Vec<Fixture> {
    vec![
        unpolicied(
            "no retry policy, the step's own page",
            shown(HOME),
            "home",
            false,
            word("input_not_taken"),
        ),
        decided(
            "a decided failure, the step's own page",
            shown(HOME),
            Some("home"),
            word("input_not_taken"),
        ),
    ]
}

fn other_page_shown() -> Vec<Fixture> {
    let lobby = || word("other_page_shown");
    let waited = ended(
        "a wait step away on another page",
        &task(vec![hold()]),
        ScriptedRuntime::new(vec![vec![shown(HOME), shown(LOBBY)]]),
        0,
    );
    let waited_detail = decided_detail(&waited, "hold", "lobby", false);
    vec![
        unpolicied(
            "no retry policy, another page",
            shown(LOBBY),
            "lobby",
            false,
            lobby(),
        ),
        decided(
            "a decided failure, another page",
            shown(LOBBY),
            Some("lobby"),
            lobby(),
        ),
        Fixture {
            case: "a wait step away on another page",
            error: waited,
            code: STEP_NOT_CONFIRMED,
            detail: Some(waited_detail),
            before: json!({"pack_operation_id": "hold", "page_ids": ["result"], "attempts": 1,
                           "page_id": "lobby"}),
            added: lobby(),
        },
    ]
}

fn nothing_recognised() -> Vec<Fixture> {
    let none = || word("nothing_recognised");
    vec![
        unpolicied(
            "no retry policy, no page",
            shown(DARK),
            UNRECOGNIZED,
            false,
            none(),
        ),
        decided("a decided failure, no page", shown(DARK), None, none()),
        Fixture {
            case: "a retry's wait that matched no page",
            error: ended(
                "a retry's wait that matched no page",
                &task(vec![open(retried())]),
                tapped(vec![shown(DARK)]),
                1,
            ),
            code: CONFIRMATION_FAILED,
            detail: Some(unpolicied_detail(UNRECOGNIZED, false)),
            before: json!({}),
            added: none(),
        },
    ]
}

fn screen_still() -> Vec<Fixture> {
    vec![
        still("no retry policy, the still limit", json!({})),
        still(
            "a decided failure, the still limit",
            json!({"retryable": false}),
        ),
    ]
}

fn loading_limit() -> Vec<Fixture> {
    vec![
        loading(
            "no retry policy, the loading allowance",
            json!({}),
            frame(DARK, READY, true),
            None,
        ),
        loading(
            "a decided failure, the loading allowance",
            json!({"retryable": false}),
            frame(LOBBY, READY, true),
            Some("lobby"),
        ),
    ]
}

fn error_page() -> Vec<Fixture> {
    let fail = || word("error_page");
    let decided_fail = ended(
        "a decided failure on the error page",
        &task(vec![open(json!({"retryable": false}))]),
        tapped(vec![shown(FAIL)]),
        1,
    );
    let retry_fail = ended(
        "a retry's wait on the error page",
        &task(vec![open(retried())]),
        late_then(FAIL),
        1,
    );
    let waited = ended(
        "a wait step on the error page",
        &task(vec![hold()]),
        ScriptedRuntime::new(vec![vec![shown(HOME), shown(FAIL)]]),
        0,
    );
    let on_fail = |operation: &str| {
        Some(format!(
            "operation={operation} attempts=1 after_page=fail hit_error_page=true"
        ))
    };
    vec![
        unpolicied(
            "no retry policy, the error page",
            shown(FAIL),
            "fail",
            true,
            fail(),
        ),
        Fixture {
            case: "a decided failure on the error page",
            error: decided_fail,
            code: ERROR_PAGE_CODE,
            detail: on_fail("open"),
            before: json!({}),
            added: fail(),
        },
        Fixture {
            case: "a retry's wait on the error page",
            error: retry_fail,
            code: ERROR_PAGE_CODE,
            detail: on_fail("open"),
            before: json!({}),
            added: fail(),
        },
        Fixture {
            case: "a wait step on the error page",
            error: waited,
            code: ERROR_PAGE_CODE,
            detail: on_fail("hold"),
            before: json!({}),
            added: fail(),
        },
    ]
}

fn error_page_no_exit() -> Vec<Fixture> {
    let case = "a run standing on the error page";
    vec![Fixture {
        case,
        error: ended(
            case,
            &task(vec![open(json!({}))]),
            ScriptedRuntime::new(vec![vec![shown(FAIL)]]),
            0,
        ),
        code: ERROR_PAGE_CODE,
        detail: Some("after_page=fail hit_error_page=true".to_owned()),
        before: json!({}),
        added: word("error_page_no_exit"),
    }]
}

// `extra` on top of a recovery the operation asks for when its failure decision fails.
fn recovering(extra: Value) -> Value {
    with(
        json!({"retryable": true, "max_attempts": 1, "retry_interval_ms": 1,
               "on_error": "return_home"}),
        extra,
    )
}

fn requires_scheduler() -> Vec<Fixture> {
    let case = "a confirmation failure asking for recovery";
    let confirmed = ended(
        case,
        &task(vec![open(recovering(json!({})))]),
        tapped(vec![shown(HOME)]),
        1,
    );
    let retry_case = "a retry's error page asking for recovery";
    let retry_fail = ended(
        retry_case,
        &task(vec![open(recovering(json!({"max_attempts": 2})))]),
        late_then(FAIL),
        1,
    );
    let guard_case = "a guard refusal asking for recovery";
    let refused = ended(
        guard_case,
        &task(vec![claim(recovering(json!({"max_attempts": 2})))]),
        tapped(vec![shown(LOBBY)]),
        1,
    );
    vec![
        Fixture {
            case,
            error: confirmed,
            code: REQUIRES_SCHEDULER,
            detail: None,
            before: json!({}),
            added: json!({"failure_code": CONFIRMATION_FAILED,
                          "confirmation_problem": "input_not_taken"}),
        },
        Fixture {
            case: retry_case,
            error: retry_fail,
            code: REQUIRES_SCHEDULER,
            detail: None,
            before: json!({}),
            added: json!({"failure_code": CONFIRMATION_FAILED, "confirmation_problem": "error_page"}),
        },
        Fixture {
            case: guard_case,
            error: refused,
            code: REQUIRES_SCHEDULER,
            detail: None,
            before: json!({}),
            added: json!({"failure_code": GUARD_REFUSED}),
        },
    ]
}

fn page_mismatch() -> Vec<Fixture> {
    let case = "the retry stopped on another page";
    vec![Fixture {
        case,
        error: ended(
            case,
            &task(vec![claim(retried())]),
            tapped(vec![shown(LOBBY)]),
            1,
        ),
        code: GUARD_REFUSED,
        detail: Some("operation=claim expected_page=home observed_page=lobby".to_owned()),
        before: json!({"page_id": "lobby"}),
        added: guard_word("page_mismatch"),
    }]
}

fn target_not_passed() -> Vec<Fixture> {
    let case = "the flag busy on the guard's page";
    vec![Fixture {
        case,
        error: ended(
            case,
            &task(vec![claim(json!({}))]),
            ScriptedRuntime::new(vec![vec![frame(HOME, BUSY, false)]]),
            0,
        ),
        code: GUARD_REFUSED,
        detail: Some("operation=claim target=flag/ready".to_owned()),
        before: json!({"page_id": "home"}),
        added: guard_word("target_not_passed"),
    }]
}

fn page_moved() -> Vec<Fixture> {
    let case = "the gap frame on another page";
    vec![Fixture {
        case,
        error: ended(
            case,
            &task(vec![open(json!({}))]),
            ScriptedRuntime::new(vec![vec![shown(HOME), shown(LOBBY)]])
                .with_action_gap(Duration::from_millis(100)),
            0,
        ),
        code: GUARD_REFUSED,
        detail: Some("operation=open expected_page=home observed_page=lobby".to_owned()),
        before: json!({"page_id": "lobby"}),
        added: guard_word("page_moved"),
    }]
}

// Each fixture ends with its code, and its outcome's values are those it had before this slice
// with the word added; at least one fixture reaches the word.
fn assert_words(fixtures: &[Fixture]) {
    assert!(!fixtures.is_empty(), "a fixture reaches the word");
    for fixture in fixtures {
        let case = fixture.case;
        assert_eq!(
            fixture.error.code().as_str(),
            fixture.code,
            "{case}: {:?}",
            fixture.error
        );
        assert_eq!(
            values(&fixture.error),
            joined(&fixture.before, &fixture.added),
            "{case}"
        );
    }
}

// Place 1: the wait's last frame on the step's own page.
#[test]
fn k2_input_not_taken_names_the_step_s_own_page_on_the_last_frame() {
    assert_words(&input_not_taken());
}

// Place 1: the wait's last frame on another page of the package, a wait step's away clock
// included.
#[test]
fn k2_other_page_shown_names_another_page_on_the_last_frame() {
    assert_words(&other_page_shown());
}

// Place 1: the wait's last frame on no page, a retry's wait included; the page graph compares no
// frames, so no moving-or-still word is offered.
#[test]
fn k2_nothing_recognised_names_a_last_frame_on_no_page() {
    assert_words(&nothing_recognised());
}

// Place 1: the wait ended on its still limit.
#[test]
fn k2_screen_still_names_the_still_limit() {
    assert_words(&screen_still());
}

// Place 1: the wait ended on its loading allowance.
#[test]
fn k2_loading_limit_names_the_loading_allowance() {
    assert_words(&loading_limit());
}

// Places 1 and 2: a declared error page, with and without a retry policy, on a retry's wait and in
// a wait step's wait.
#[test]
fn k2_error_page_names_a_declared_error_page() {
    assert_words(&error_page());
}

// Place 2: the run stood on an error page no operation leaves.
#[test]
fn k2_error_page_no_exit_names_an_error_page_no_operation_leaves() {
    assert_words(&error_page_no_exit());
}

// Place 2: the run a scheduler must go on with names the failure whose decision asked for the
// recovery, and that failure's word when it was a confirmation's.
#[test]
fn k2_requires_scheduler_names_the_failure_it_drops_and_its_word() {
    assert_words(&requires_scheduler());
}

// Place 6: a retry stopped on another page than the guard's.
#[test]
fn k2_page_mismatch_names_a_frame_off_the_guard_s_page() {
    assert_words(&page_mismatch());
}

// Place 6: the guard's target did not pass on its page; a target without a registered reason
// names no `failure_code`.
#[test]
fn k2_target_not_passed_names_a_guard_target_that_did_not_pass() {
    assert_words(&target_not_passed());
}

// Place 6: a trusted-coordinate step's gap frame left its page.
#[test]
fn k2_page_moved_names_a_gap_frame_off_the_step_s_page() {
    assert_words(&page_moved());
}

// The design leaves the word of a wait step's end to this slice: its `max_ms` reached on its own
// page is no situation of the vocabulary (no input was made, nothing else showed), so it carries
// no word and its values are what they were.
#[test]
fn k2_a_wait_step_held_on_its_own_page_carries_no_word() {
    let case = "a wait step held on its own page";
    let error = ended(
        case,
        &task(vec![hold()]),
        ScriptedRuntime::new(vec![vec![shown(HOME)]]),
        0,
    );
    assert_eq!(error.code().as_str(), STEP_NOT_CONFIRMED, "{error:?}");
    assert_eq!(
        values(&error),
        json!({"pack_operation_id": "hold", "page_ids": ["result"], "attempts": 1,
               "page_id": "home"})
    );
    assert_eq!(
        error.detail().map(str::to_owned),
        Some(decided_detail(&error, "hold", "home", false))
    );
}

// The design's test rule: for every fixture that reaches a word, what the failure identity and the
// streak read is what it was without the word. The identity's key reads the code and the
// whitelisted fragments of the detail (the host adds the scope and the step's operation label from
// the run's rows, which the word does not touch), and the streak compares the code, the identity
// and the class: the code is the same code, the detail is byte for byte the one the code had
// before this slice, and the outcome without the word is the outcome it had.
#[test]
fn k2_a_word_leaves_the_code_the_detail_and_every_other_value_as_they_were() {
    let fixtures = [
        input_not_taken(),
        other_page_shown(),
        nothing_recognised(),
        screen_still(),
        loading_limit(),
        error_page(),
        error_page_no_exit(),
        requires_scheduler(),
        page_mismatch(),
        target_not_passed(),
        page_moved(),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    assert_eq!(fixtures.len(), 23);
    for fixture in &fixtures {
        let case = fixture.case;
        assert_eq!(fixture.error.code().as_str(), fixture.code, "{case}");
        assert_eq!(
            fixture.error.detail(),
            fixture.detail.as_deref(),
            "{case}: the detail is the one before this slice"
        );
        assert_eq!(
            without(&values(&fixture.error), &fixture.added),
            fixture.before,
            "{case}: the other values are the ones before this slice"
        );
    }
}
