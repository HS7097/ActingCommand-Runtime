// SPDX-License-Identifier: AGPL-3.0-only

// Test plan v2.2 E-3 G3 (§3.4 E-3 row, §3.10 "E-3 细则"; inventory-exec items 5 and 21, gap G3).
// Every kernel path on which the screen disagrees with the pack ends the run with its code; the
// code's registered ladder class is `pack_logic_mismatch` (model M387 v1.2 §3.2, row 4: "要看画面
// 才知道"), and the first `task`-stage trigger item of the shipped defaults file that names the
// code or its class is `pack_logic`, the code under no `exclude_codes` entry (M387 §3.1, §3.3;
// FCM E-09 for `contained_task_guard_refused`). A target consensus that does not hold (two
// frames that both miss, one hit of two) is a page that is not recognized and ends each path the
// same way (exec key 21; the other rows are exec key 5). The two sampling rows end with
// `recognition_sample_deadline_exceeded` (the sampling ran past its window and no retry is left)
// and `recognition_sample_budget_insufficient` (the task's remaining time cannot hold another
// window: scheduling model v1.4.2 §2.1 items 5-6, §2.3); their class is `budget_or_permission`
// and no trigger item of the defaults file names the class or the code (Alice 10-09, test plan
// §8.1 question 1: the four sampling cases never start the ladder; M387 §3.6; scheduling model
// §2.5). Every expected value comes from those rows, never from the code.
//
// The paths are G3's table {no consensus, both miss, one of two, samples short, sample deadline}
// x {step page wait, home entry, postcondition}; the guard, error page and conflict rows take the
// pack without consensus. The runs play scripted frames on the scripted clock (EXEC-I4): a
// frame's left pixel names its page, its right pixel is the guard's (or the banner's), and a
// frame the device is slow to deliver spends its observation's window.

use super::tests::scripted::{ScriptedFrame, ScriptedRuntime};
use super::*;
use actingcommand_contract::outcome::{LadderClass, catalog_json};
use actingcommand_recognition_pack::RecognitionPack;
use serde_json::{Map, Value, json};
use std::path::PathBuf;

const CAPTURE_INTERVAL_MS: u64 = 100;
const STEP_TIMEOUT_MS: u64 = 1_000;
// A task timeout no row reaches.
const TASK_TIMEOUT_MS: u64 = 60_000;
// A task timeout of one window: once a window is spent the task has less left than a new one.
const SHORT_TASK_MS: u64 = 1_000;
// The pause between the two frames of a page target's consensus.
const SAMPLE_INTERVAL_MS: u64 = 100;
// A slow frame arrives when this much of its observation's window is left: less than one sample
// interval, so the observation's sampling runs past the window.
const LEFT_MS: u64 = 50;

const HOME_PAGE: &str = "neutral/home";
// The trigger item M387 §3.3 names for the "screen disagrees with the pack" direction.
const PACK_LOGIC_ITEM: &str = "pack_logic";
// The shipped defaults file (M387 §5.2), the bytes the program embeds.
const DEFAULTS: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../distribution/windows/actingd.defaults.json"
));

const PAGE_UNKNOWN: &str = SharedCode::ContainedTaskPageUnknown.as_str();
const RECOGNITION_CONFLICT: &str = ExecutionKernelCode::ContainedTaskRecognitionConflict.as_str();
const HOME_ENTRY_NOT_MATCHED: &str = SharedCode::ContainedTaskHomeEntryNotMatched.as_str();
const GUARD_REFUSED: &str = ExecutionKernelCode::ContainedTaskGuardRefused.as_str();
const STEP_NOT_CONFIRMED: &str = ExecutionKernelCode::ContainedTaskStepUnconfirmed.as_str();
const ERROR_PAGE_SHOWN: &str = ExecutionKernelCode::ContainedTaskErrorPageReached.as_str();
const SAMPLE_DEADLINE: &str = ExecutionKernelCode::RecognitionSampleDeadlineExceeded.as_str();
const SAMPLE_BUDGET: &str = ExecutionKernelCode::RecognitionSampleBudgetInsufficient.as_str();

const HOME: [u8; 3] = [255, 0, 0];
const NEXT: [u8; 3] = [0, 0, 255];
const NOTICE: [u8; 3] = [255, 0, 255];
const BANNER: [u8; 3] = [255, 255, 0];
const READY: [u8; 3] = [0, 255, 0];
const DARK: [u8; 3] = [0, 0, 0];

// What the device shows.
#[derive(Debug, Clone, Copy)]
enum Screen {
    // Page `home` with the guard's target ready.
    Home,
    // Page `home` before the guard's target is ready.
    Unready,
    // Page `next`, the step's destination and the task's terminal page.
    Next,
    // Page `notice`, the task's error page.
    Notice,
    // Page `home` under page `banner`'s mark: two pages of the package on one frame.
    Overlap,
    // No page of the package.
    Blank,
}

fn shown(screen: Screen) -> ScriptedFrame {
    let pixels = match screen {
        Screen::Home => [HOME, READY],
        Screen::Unready => [HOME, DARK],
        Screen::Next => [NEXT, DARK],
        Screen::Notice => [NOTICE, DARK],
        Screen::Overlap => [HOME, BANNER],
        Screen::Blank => [DARK, DARK],
    };
    ScriptedFrame::rgb(2, 1, &pixels)
}

// `screen` on every frame.
fn steady(screen: Screen) -> Vec<ScriptedFrame> {
    vec![shown(screen)]
}

// `first` and `second` in turn, for longer than any row's wait: each pair of frames a consensus
// samples holds the page `second` shows on one frame of two.
fn alternating(first: Screen, second: Screen) -> Vec<ScriptedFrame> {
    (0..32)
        .flat_map(|_| [shown(first), shown(second)])
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sampling {
    // The pack declares no target consensus.
    SingleFrame,
    // Every page target needs both of two frames 100 ms apart (k = 2 of 2).
    TwoFrames,
}

// The recognition pack: four pages told apart by one pixel each, and the guard's target.
fn pack(sampling: Sampling) -> RecognitionPack {
    let colour = |id: &str, x: u32, expected: [u8; 3]| {
        json!({"type": "color", "id": id,
               "region": {"x": x, "y": 0, "width": 1, "height": 1}, "expected": expected})
    };
    let pages = [
        ("page/home", 0, HOME),
        ("page/next", 0, NEXT),
        ("page/notice", 0, NOTICE),
        ("page/banner", 1, BANNER),
    ];
    let mut targets = pages
        .iter()
        .map(|&(id, x, expected)| colour(id, x, expected))
        .collect::<Vec<_>>();
    targets.push(colour("guard/ready", 1, READY));
    let mut pack = json!({
        "schema_version": "0.7",
        "game": "neutral",
        "server": "test",
        "coordinate_space": {"width": 2, "height": 1},
        "defaults": {"color_max_distance": 0.0},
        "targets": targets
    });
    if sampling == Sampling::TwoFrames {
        let consensus = pages
            .iter()
            .map(|&(id, _, _)| {
                let two_frames = json!({"samples": [{"frame": 0}, {"frame": 1}], "k": 2,
                                        "sample_interval_ms": SAMPLE_INTERVAL_MS});
                (id.to_owned(), two_frames)
            })
            .collect::<Map<String, Value>>();
        pack["target_consensus"] = Value::Object(consensus);
    }
    serde_json::from_value(pack).expect("recognition pack")
}

fn page_set() -> PageSet {
    serde_json::from_value(json!({
        "schema_version": "0.3",
        "pages": [
            {"id": "neutral/home", "required": ["page/home"]},
            {"id": "neutral/next", "required": ["page/next"]},
            {"id": "neutral/notice", "required": ["page/notice"]},
            {"id": "neutral/banner", "required": ["page/banner"]}
        ]
    }))
    .expect("page set")
}

// One guarded click from `home` to the terminal page `next`; `notice` is the task's error page.
// A confirmation that fails is not retried (`retryable: false`).
fn task(sampling: Sampling) -> PreparedContainedTask {
    let control: TaskControl = serde_json::from_value(json!({
        "schema_version": "Lab-1y.control.v1",
        "package_id": "neutral.screen.task",
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": 2, "height": 1},
        "entry_task_id": "task",
        "capture_interval_ms": CAPTURE_INTERVAL_MS,
        "step_timeout_ms": STEP_TIMEOUT_MS,
        "timeout_ms": TASK_TIMEOUT_MS
    }))
    .expect("task control");
    let program: TaskProgram = serde_json::from_value(json!({
        "schema_version": "0.6",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": 2, "height": 1},
        "target_page": "next",
        "error_pages": ["notice"],
        "operations": [{
            "id": "open_next",
            "from": "home",
            "to": "next",
            "click": {"kind": "point", "x": 1, "y": 0},
            "guard": {"page_id": "home", "target_id": "guard/ready",
                      "expected_rect": {"x": 1, "y": 0, "width": 1, "height": 1},
                      "color_probe": "guard/ready"},
            "retryable": false
        }]
    }))
    .expect("task program");
    for operation in &program.operations {
        operation
            .validate(&control, TaskOperationDefaults::default(), "0.6")
            .expect("an admitted operation");
    }
    let evaluator =
        RecognitionEvaluator::new(PathBuf::new(), pack(sampling)).expect("recognition evaluator");
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

// The same task, whose ordinary run requires page `home` first.
fn with_home_entry(mut task: PreparedContainedTask) -> PreparedContainedTask {
    task.entry_page = Some(RequiredEntryPages::single(HOME_PAGE.to_owned()));
    task
}

fn failure(
    result: Result<ContainedTaskOutcome, ContainedTaskRunError<&'static str>>,
) -> ContainedTaskError {
    match result {
        Err(ContainedTaskRunError::Task(error)) => error,
        Err(other) => panic!("expected a task failure, got {other:?}"),
        Ok(outcome) => panic!("expected a task failure, the run ended {outcome:?}"),
    }
}

fn values(error: &ContainedTaskError) -> Value {
    Value::Object(error.outcome().values().clone())
}

// The entry facts the run recorded: the page each matched, `None` for an unmatched entry.
fn entry_verdicts(runtime: &ScriptedRuntime) -> Vec<Option<String>> {
    runtime
        .traces()
        .iter()
        .filter_map(|trace| match trace {
            ContainedTaskTrace::EntryRecognition { matched_page, .. } => Some(matched_page.clone()),
            _ => None,
        })
        .collect()
}

// The ladder class the catalog registers for `code` (M387 §3.2: the host reads it by spelling).
fn registered_class(code: &str) -> &'static str {
    let catalog = catalog_json().expect("the embedded catalog parses");
    catalog
        .get("codes")
        .and_then(|codes| codes.get(code))
        .and_then(|entry| entry.get("ladder_class"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("{code} has no registered ladder class"))
}

fn defaults() -> Value {
    serde_json::from_str(DEFAULTS).expect("the shipped defaults file parses")
}

// Whether `list` names `code`, as a code or as a `prefix*`.
fn names_code(list: &Value, code: &str) -> bool {
    list.as_array().is_some_and(|items| {
        items
            .iter()
            .filter_map(Value::as_str)
            .any(|pattern| match pattern.strip_suffix('*') {
                Some(prefix) => code.starts_with(prefix),
                None => pattern == code,
            })
    })
}

fn holds(list: &Value, token: &str) -> bool {
    list.as_array()
        .is_some_and(|items| items.iter().any(|item| item.as_str() == Some(token)))
}

// M387 §3.1: the trigger item a run's failure `code` starts at the `task` stage. None when
// `exclude_codes` names the code; otherwise the first item, in order, whose `stages` hold `task`
// and whose `codes` name the code or whose `classes` hold its class.
fn task_trigger(code: &str) -> Option<String> {
    let defaults = defaults();
    let section = &defaults["recovery_ladder"];
    if names_code(&section["exclude_codes"], code) {
        return None;
    }
    let class = registered_class(code);
    section["triggers"]
        .as_array()
        .expect("the defaults file lists trigger items")
        .iter()
        .find(|item| {
            holds(&item["stages"], "task")
                && (names_code(&item["codes"], code) || holds(&item["classes"], class))
        })
        .map(|item| item["id"].as_str().expect("a trigger item's id").to_owned())
}

// The trigger items, at any stage, whose `codes` name `code` or whose `classes` hold its class.
fn items_naming(code: &str) -> Vec<String> {
    let class = registered_class(code);
    defaults()["recovery_ladder"]["triggers"]
        .as_array()
        .expect("the defaults file lists trigger items")
        .iter()
        .filter(|item| names_code(&item["codes"], code) || holds(&item["classes"], class))
        .map(|item| item["id"].as_str().unwrap_or_default().to_owned())
        .collect()
}

// A "screen disagrees with the pack" code: class 4 of M387 §3.2, and it starts `pack_logic`.
fn assert_starts_pack_logic(code: &str) {
    assert_eq!(
        registered_class(code),
        LadderClass::PackLogicMismatch.as_str(),
        "{code}"
    );
    assert_eq!(
        task_trigger(code).as_deref(),
        Some(PACK_LOGIC_ITEM),
        "{code}"
    );
}

// A sampling code: class 2 of M387 §3.2, and no trigger item names it or its class (§3.6).
fn assert_starts_no_ladder(code: &str) {
    assert_eq!(
        registered_class(code),
        LadderClass::BudgetOrPermission.as_str(),
        "{code}"
    );
    let items = items_naming(code);
    assert!(
        items.is_empty(),
        "{code} is named by trigger items {items:?}"
    );
}

// G3 row `contained_task_page_unknown`, step page wait, no consensus: the screen shows no page
// of the package for the whole step timeout.
#[test]
fn gate_e3_g3_a_screen_with_no_page_of_the_pack_ends_the_page_wait_with_page_unknown() {
    let mut runtime = ScriptedRuntime::new(vec![steady(Screen::Blank)]);
    let error = failure(task(Sampling::SingleFrame).run(&mut runtime));
    assert_eq!(error.code().as_str(), PAGE_UNKNOWN, "{error:?}");
    assert!(runtime.inputs().is_empty());
    assert_starts_pack_logic(PAGE_UNKNOWN);
}

// G3 row `contained_task_recognition_conflict`: two pages of the package match one frame
// (contracts/ocr-fields.md: ambiguous recognition keeps the conflict failure).
#[test]
fn gate_e3_g3_a_frame_two_pages_match_ends_with_recognition_conflict() {
    let mut runtime = ScriptedRuntime::new(vec![steady(Screen::Overlap)]);
    let error = failure(task(Sampling::SingleFrame).run(&mut runtime));
    assert_eq!(error.code().as_str(), RECOGNITION_CONFLICT, "{error:?}");
    assert!(runtime.inputs().is_empty());
    assert_starts_pack_logic(RECOGNITION_CONFLICT);
}

// G3 row `contained_task_home_entry_not_matched`, home entry, no consensus. contracts/ocr-fields.md
// (Workflow #371-3): the entry looks at one frame every capture interval until `home` matches
// or min(step timeout, task timeout) is spent, then fails with the code and records the entry
// fact once.
#[test]
fn gate_e3_g3_a_home_entry_that_never_matches_ends_with_home_entry_not_matched() {
    let mut runtime = ScriptedRuntime::new(vec![steady(Screen::Blank)]);
    let error = failure(with_home_entry(task(Sampling::SingleFrame)).run(&mut runtime));
    assert_eq!(error.code().as_str(), HOME_ENTRY_NOT_MATCHED, "{error:?}");
    assert!(
        runtime.captures() as u64 >= STEP_TIMEOUT_MS / CAPTURE_INTERVAL_MS,
        "the entry waited for the window: {} frames",
        runtime.captures()
    );
    assert_eq!(entry_verdicts(&runtime), [None::<String>]);
    assert!(runtime.inputs().is_empty());
    assert_starts_pack_logic(HOME_ENTRY_NOT_MATCHED);
}

// G3 row `contained_task_guard_refused` (FCM E-09): the step's page shows, but the target its
// guard checks does not pass; no input is made.
#[test]
fn gate_e3_g3_a_guard_that_does_not_pass_ends_with_guard_refused() {
    let mut runtime = ScriptedRuntime::new(vec![steady(Screen::Unready)]);
    let error = failure(task(Sampling::SingleFrame).run(&mut runtime));
    assert_eq!(error.code().as_str(), GUARD_REFUSED, "{error:?}");
    assert!(runtime.inputs().is_empty());
    assert_starts_pack_logic(GUARD_REFUSED);
}

// G3 row `contained_task_step_unconfirmed`, postcondition, no consensus: the screen stays on
// `home` after the click, the destination never shows within the step timeout, and the
// operation is not retried (one input).
#[test]
fn gate_e3_g3_a_destination_that_never_shows_ends_with_step_unconfirmed() {
    let mut runtime = ScriptedRuntime::new(vec![steady(Screen::Home)]);
    let error = failure(task(Sampling::SingleFrame).run(&mut runtime));
    assert_eq!(error.code().as_str(), STEP_NOT_CONFIRMED, "{error:?}");
    assert_eq!(runtime.inputs().len(), 1);
    assert_starts_pack_logic(STEP_NOT_CONFIRMED);
}

// G3 row `contained_task_error_page_reached`: after the click the screen shows the task's error
// page, and the operation is not retried.
#[test]
fn gate_e3_g3_an_error_page_after_the_input_ends_with_error_page_reached() {
    let mut runtime = ScriptedRuntime::new(vec![steady(Screen::Home), steady(Screen::Notice)]);
    let error = failure(task(Sampling::SingleFrame).run(&mut runtime));
    assert_eq!(error.code().as_str(), ERROR_PAGE_SHOWN, "{error:?}");
    assert_eq!(runtime.inputs().len(), 1);
    assert_starts_pack_logic(ERROR_PAGE_SHOWN);
}

// The three observations of G3's table.
#[derive(Debug, Clone, Copy)]
enum Path {
    // The run's first wait for a page of the package.
    PageWait,
    // The ordinary run's required `home` entry.
    HomeEntry,
    // The wait for the step's destination after its input.
    Postcondition,
}

const PATHS: [Path; 3] = [Path::PageWait, Path::HomeEntry, Path::Postcondition];

impl Path {
    // The observation a sampling timeout on the path names (scheduling model §2.1 item 6).
    const fn stage(self) -> ExecutionKernelLocation {
        match self {
            Self::PageWait => ExecutionKernelLocation::ObservationPageWait,
            Self::HomeEntry => ExecutionKernelLocation::ObservationEntry,
            Self::Postcondition => ExecutionKernelLocation::ObservationAfterInput,
        }
    }

    // The inputs a run makes before the path's observation.
    const fn inputs(self) -> usize {
        match self {
            Self::Postcondition => 1,
            Self::PageWait | Self::HomeEntry => 0,
        }
    }
}

// A sampling row's run on `path`: the pack samples two frames per page target, the task has
// `task_timeout_ms`, each step `retries` observe-only retries, and the first frame of the path's
// observation arrives `latency` after its capture starts.
fn sampled_run(
    path: Path,
    task_timeout_ms: u64,
    retries: u32,
    latency: Duration,
) -> (ContainedTaskError, ScriptedRuntime) {
    let mut task = task(Sampling::TwoFrames);
    task.control.timeout_ms = Some(task_timeout_ms);
    let slow = |screen: Screen| vec![shown(screen).delivered_after(latency), shown(screen)];
    let screens = match path {
        Path::PageWait => vec![slow(Screen::Home)],
        Path::HomeEntry => {
            task = with_home_entry(task);
            vec![slow(Screen::Home)]
        }
        Path::Postcondition => vec![steady(Screen::Home), slow(Screen::Next)],
    };
    let mut runtime = ScriptedRuntime::new(screens).with_sample_timeout_retries(retries);
    let error = failure(task.run(&mut runtime));
    (error, runtime)
}

// G3 sampling row "sample deadline" on each path: a frame the device delivers late leaves less
// than a sample interval of the window, the observation's sampling runs past it, and no retry
// is left (scheduling model §2.1 item 6: the timeout's code with `attempts`, `timeout_ms` and
// `stage`). The code starts no ladder.
#[test]
fn gate_e3_g3_a_sampling_past_its_window_ends_with_the_deadline_code_and_starts_no_ladder() {
    for path in PATHS {
        let latency = Duration::from_millis(STEP_TIMEOUT_MS - LEFT_MS);
        let (error, runtime) = sampled_run(path, TASK_TIMEOUT_MS, 0, latency);
        assert_eq!(
            error.code().as_str(),
            SAMPLE_DEADLINE,
            "{path:?}: {error:?}"
        );
        assert_eq!(
            values(&error),
            json!({"attempts": 0, "timeout_ms": STEP_TIMEOUT_MS,
                   "stage": path.stage().as_str()}),
            "{path:?}"
        );
        assert_eq!(runtime.inputs().len(), path.inputs(), "{path:?}");
    }
    assert_starts_no_ladder(SAMPLE_DEADLINE);
}

// G3 sampling row "samples short" on each path: the late frame spends the window, the step's one
// retry would open a new window, but the task's deadline leaves less time than the recognition's
// two frames need (scheduling model §2.1 item 5, §2.3: the registered code with `required_ms`
// and `remaining_ms`). The code starts no ladder.
#[test]
fn gate_e3_g3_no_task_time_for_another_window_ends_with_the_budget_code_and_starts_no_ladder() {
    for path in PATHS {
        // The postcondition's window opens after the step's first observation (two frames one
        // sample interval apart) and ends at the task's deadline.
        let window_ms = match path {
            Path::Postcondition => SHORT_TASK_MS - SAMPLE_INTERVAL_MS,
            Path::PageWait | Path::HomeEntry => SHORT_TASK_MS,
        };
        let latency = Duration::from_millis(window_ms - LEFT_MS);
        let (error, runtime) = sampled_run(path, SHORT_TASK_MS, 1, latency);
        assert_eq!(error.code().as_str(), SAMPLE_BUDGET, "{path:?}: {error:?}");
        assert_eq!(
            values(&error),
            json!({"required_ms": SAMPLE_INTERVAL_MS, "remaining_ms": LEFT_MS}),
            "{path:?}"
        );
        assert_eq!(runtime.inputs().len(), path.inputs(), "{path:?}");
    }
    assert_starts_no_ladder(SAMPLE_BUDGET);
}

// Exec key 21, step page wait: a consensus that does not hold is a page that is not recognized,
// and the wait ends as an unrecognized screen does (test plan §3.10 G3: "共识 k-of-n 不过 → 页面
// 不认，同上").
#[test]
fn gate_e3_g3_consensus_not_met_at_the_step_page_wait_ends_with_page_unknown() {
    for (case, script) in [
        ("both frames miss", steady(Screen::Blank)),
        (
            "one frame of two hits",
            alternating(Screen::Blank, Screen::Home),
        ),
    ] {
        let mut runtime = ScriptedRuntime::new(vec![script]);
        let error = failure(task(Sampling::TwoFrames).run(&mut runtime));
        assert_eq!(error.code().as_str(), PAGE_UNKNOWN, "{case}: {error:?}");
        assert!(runtime.inputs().is_empty(), "{case}");
    }
    assert_starts_pack_logic(PAGE_UNKNOWN);
}

// Exec key 21, home entry: a pack with target consensus keeps its single consensus verdict
// (contracts/ocr-fields.md, Workflow #371-3) over the two frames it declares; a consensus that
// does not hold is an unmatched entry, recorded once.
#[test]
fn gate_e3_g3_consensus_not_met_at_the_home_entry_ends_with_home_entry_not_matched() {
    for (case, script) in [
        ("both frames miss", steady(Screen::Blank)),
        (
            "one frame of two hits",
            alternating(Screen::Blank, Screen::Home),
        ),
    ] {
        let mut runtime = ScriptedRuntime::new(vec![script]);
        let error = failure(with_home_entry(task(Sampling::TwoFrames)).run(&mut runtime));
        assert_eq!(
            error.code().as_str(),
            HOME_ENTRY_NOT_MATCHED,
            "{case}: {error:?}"
        );
        assert_eq!(runtime.captures(), 2, "{case}");
        assert_eq!(entry_verdicts(&runtime), [None::<String>], "{case}");
        assert!(runtime.inputs().is_empty(), "{case}");
    }
    assert_starts_pack_logic(HOME_ENTRY_NOT_MATCHED);
}

// Exec key 21, postcondition: after the click the destination's consensus does not hold (the
// screen stays on `home`, or `next` shows on one frame of two), so the step is not confirmed.
#[test]
fn gate_e3_g3_consensus_not_met_at_the_postcondition_ends_with_step_unconfirmed() {
    for (case, after_input) in [
        ("both frames miss", steady(Screen::Home)),
        (
            "one frame of two hits",
            alternating(Screen::Blank, Screen::Next),
        ),
    ] {
        let mut runtime = ScriptedRuntime::new(vec![steady(Screen::Home), after_input]);
        let error = failure(task(Sampling::TwoFrames).run(&mut runtime));
        assert_eq!(
            error.code().as_str(),
            STEP_NOT_CONFIRMED,
            "{case}: {error:?}"
        );
        assert_eq!(runtime.inputs().len(), 1, "{case}");
    }
    assert_starts_pack_logic(STEP_NOT_CONFIRMED);
}
