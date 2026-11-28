// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #463 KP-1 (register §1.448; contracts/linear-steps.md, "Key steps"; the owner's ruling
// on board #463 that the back key is not recorded while playing and that an agent adds such a step
// afterwards; the coordinator's rulings on the study `rd-1116/report-463-key-step-study.md` and in
// the work order `rd-1116/brief-463-kp1-linear-key-step.md`): a `linear_steps` operation's effect
// may be `"key": {"key": "back"}`, the one word of a closed vocabulary. The step is guarded as a
// click step is and declares no retry; its key goes out through the input as a click's tap does,
// and the step then awaits the next step's page as a click step does, a page that does not come
// ending the run as a click step's last attempt ends it. A run of a package with a key step asks
// the runtime once, after `RunStarted` and before any capture, input or effect intent, whether the
// instance's input sends the key; where it does not, the run ends there with
// `contained_task_linear_key_unsupported`. Admission refuses a word outside the vocabulary
// (`key_unknown`), a key step that declares a retry (`key_retry`) and a key beside a click
// (`operation_effect`). A package with a key step needs `package` 17; the offline first decision
// names a first key step as one (`would_press_key`).
//
// Every expected value comes from those lines and the package values below, never from the code.
// The packages are synthetic `neutral` packages admitted by the kernel's whole admission
// (`PreparedContainedTask::load`, the declaration gate included). A frame is 2 x 1: pixel 0 names
// the page (home, list, done; black is none of them), pixel 1 is the key step's guard mark. The
// path taps from home into the list (`open`, a trusted point) and presses the back key from the
// list to done (`back`, guarded by the mark). The runs play scripted frames on the kernel's
// scripted clock (EXEC-I4): the device shows `screens[n]` after `n` inputs, each segment's last
// frame repeating.
//
// Fix round 1 (`rd-1116/brief-463-kp1-fix1.md`, after the independent review of `ec64249f`): the
// product loads every package of the source layout (a pack directory, an installed pack, a
// recorded package) through the source parser, which takes a key step now. The same path is built
// in that layout, loaded by the product's entry (`PreparedContainedTask::load_content_entries`),
// admitted and run; and the key goes out bound to the frame the runtime committed, which a device
// that commits every frame as the host does (`FramedDevice`, as `gate_hs4`'s) keeps.

use super::tests::admission_deadline;
use super::tests::scripted::{ScriptedFrame, ScriptedRuntime};
use super::*;
use actingcommand_contract::{IdentifierIssuer, InputFrameReference, package_edge_holds};
use actingcommand_device::CaptureBackendName;
use serde_json::{Value, json};
use std::io::{Cursor, Write};
use zip::{ZipWriter, write::FileOptions};

const LINEAR_INVALID: &str = ExecutionKernelCode::ContainedTaskLinearInvalid.as_str();
const KEY_UNSUPPORTED: &str = ExecutionKernelCode::ContainedTaskLinearKeyUnsupported.as_str();
const CONFIRMATION_FAILED: &str = ExecutionKernelCode::PageConfirmationFailed.as_str();
const GUARD_REFUSED: &str = ExecutionKernelCode::ContainedTaskGuardRefused.as_str();

const WIDTH: u32 = 2;
const STEP_MS: u64 = 5_000;
const ARRIVAL_MS: u64 = 1_000;
const TASK_MS: u64 = 60_000;
// The detector pages.
const LIST_PAGE: &str = "neutral/list";
const DONE_PAGE: &str = "neutral/done";
const UNRECOGNIZED: &str = "<unrecognized>";
// The detail `page_confirmation_failed` gives the end of `back`'s one attempt
// (contracts/linear-steps.md, "Failure codes"), as it gives a click step's.
const BACK_UNCONFIRMED: &str =
    "operation=back attempts=1 after_page=<unrecognized> hit_error_page=false transition=none";
// The `package` revision a package with a key step needs (contracts/component-interfaces.md, the
// `package` row 17).
const KEY_REVISION: u64 = 17;
// The declaration this build copies into its manifests.
const OWN_DECLARATION: &str =
    include_str!("../../../../distribution/windows/component-interfaces.json");

const HOME: [u8; 3] = [255, 0, 255];
const LIST: [u8; 3] = [255, 0, 0];
const DONE: [u8; 3] = [255, 255, 0];
const GREEN: [u8; 3] = [0, 255, 0];
const BLACK: [u8; 3] = [0, 0, 0];

type RunResult = Result<ContainedTaskOutcome, ContainedTaskRunError<&'static str>>;

// A screen of the page `page` with the guard mark shown or not.
fn screen(page: [u8; 3], marked: bool) -> ScriptedFrame {
    ScriptedFrame::rgb(WIDTH, 1, &[page, if marked { GREEN } else { BLACK }])
}

// The base path's screens: home, the list with the mark, then done.
fn path_screens() -> Vec<Vec<ScriptedFrame>> {
    vec![
        vec![screen(HOME, false)],
        vec![screen(LIST, true)],
        vec![screen(DONE, false)],
    ]
}

// The linear control of a path of `max_steps` operations.
fn control(max_steps: usize) -> Value {
    json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": "neutral.linear.key",
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

// A trusted tap `id` from `from` to `to`, expected within the arrival budget.
fn tap(id: &str, from: &str, to: &str) -> Value {
    json!({"id": id, "from": from, "to": to,
           "click": {"kind": "point", "x": 0, "y": 0},
           "unguarded_trusted_coordinate": true,
           "expect_after": {"page_id": to, "timeout_ms": ARRIVAL_MS}})
}

// The key step `back` from `from` to `to`: guarded by the mark on `from`, it presses the key
// `word`, expecting `to` within the arrival budget.
fn key_step(word: &str, from: &str, to: &str) -> Value {
    json!({"id": "back", "from": from, "to": to,
           "key": {"key": word},
           "guard": {"page_id": from, "target_id": "guard/ready",
                     "expected_rect": {"x": 1, "y": 0, "width": 1, "height": 1},
                     "color_probe": "guard/ready"},
           "expect_after": {"page_id": to, "timeout_ms": ARRIVAL_MS}})
}

// A task from `entry` to `target` over `operations`.
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

// The base path: `open` from home into the list, then `back` from the list to done.
fn path_task(back: Value) -> Value {
    task("home", "done", vec![tap("open", "home", "list"), back])
}

// The page set: home, list and done told by pixel 0.
fn pages() -> Value {
    let pages = ["home", "list", "done"].map(|page| {
        json!({"id": format!("neutral/{page}"), "required": [format!("page/{page}")],
               "optional": [], "forbidden": []})
    });
    json!({"schema_version": "0.3", "pages": pages})
}

// The recognition pack: the pages' marks on pixel 0 and the guard mark on pixel 1.
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

// The package of `task` under a control of as many steps as it has operations.
fn package(task: &Value) -> Vec<u8> {
    let steps = task["operations"].as_array().map_or(0, Vec::len);
    let entries = [
        ("control.json", bytes(&control(steps))),
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

// The package of `task` as contained task admission prepares it, or its refusal.
fn load(task: &Value) -> Result<PreparedContainedTask, ContainedTaskError> {
    PreparedContainedTask::load("neutral.instance", &package(task))
}

fn admitted(case: &str, task: &Value) -> PreparedContainedTask {
    load(task).unwrap_or_else(|error| panic!("{case}: refused: {error}"))
}

fn run(task: &PreparedContainedTask, mut device: ScriptedRuntime) -> (RunResult, ScriptedRuntime) {
    let result = task.run(&mut device);
    (result, device)
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

// Each `StepFinished`: its step index, its operation and its page.
fn finished(device: &ScriptedRuntime) -> Vec<(u32, &str, &str)> {
    device
        .traces()
        .iter()
        .filter_map(|trace| match trace {
            ContainedTaskTrace::StepFinished {
                step_index,
                operation_label,
                page_label,
                ..
            } => Some((*step_index, operation_label.as_str(), page_label.as_str())),
            _ => None,
        })
        .collect()
}

// The operation of each effect intent, in order.
fn intents(device: &ScriptedRuntime) -> Vec<&str> {
    device
        .traces()
        .iter()
        .filter_map(|trace| match trace {
            ContainedTaskTrace::EffectIntent {
                operation_label, ..
            } => Some(operation_label.as_str()),
            _ => None,
        })
        .collect()
}

// The action and the guard outcome of `operation`'s first effect intent.
fn intent_of<'a>(
    device: &'a ScriptedRuntime,
    operation: &str,
) -> Option<(&'a InputAction, &'a ContainedTaskGuardOutcome)> {
    device.traces().iter().find_map(|trace| match trace {
        ContainedTaskTrace::EffectIntent {
            operation_label,
            action,
            guard,
            ..
        } if operation_label == operation => Some((action, guard)),
        _ => None,
    })
}

fn timing(error: &ContainedTaskError) -> Option<(TaskTimingScope, TaskTimingStage, u64)> {
    error
        .timing()
        .map(|failure| (failure.scope, failure.stage, failure.limit_ms))
}

// Fix round 1: the scripted device behind a runtime that commits every frame it captures, as the
// host does (`gate_hs4`'s): each capture gets a frame reference of its own, which
// `committed_input_frame` answers with its context. It keeps each input with the frame reference
// the kernel bound it to and the number of frames captured before it; every other answer is the
// scripted runtime's.
struct FramedDevice {
    script: ScriptedRuntime,
    ids: IdentifierIssuer,
    captured: Vec<InputFrameReference>,
    inputs: Vec<(InputAction, Option<InputFrameReference>, usize)>,
}

impl FramedDevice {
    fn new(screens: Vec<Vec<ScriptedFrame>>) -> Self {
        Self {
            script: ScriptedRuntime::new(screens),
            ids: IdentifierIssuer::new().expect("identifier issuer"),
            captured: Vec::new(),
            inputs: Vec::new(),
        }
    }

    // Of a run of the base path (`open`'s tap, then `back`'s key): the key is the second input and
    // names a frame the runtime committed after the tap and before the key, a frame of the list,
    // the page its guard passed on. A key bound to no frame, or to the tap's frame, fails here.
    fn assert_key_bound_to_the_lists_frame(&self, case: &str) {
        let [(_, _, tapped), (key, bound, keyed)] = self.inputs.as_slice() else {
            panic!("{case}: two inputs, got {:?}", self.inputs);
        };
        assert_eq!(key, &StepKey::Back.action(), "{case}");
        let bound = bound.unwrap_or_else(|| panic!("{case}: the key names no frame"));
        assert!(
            self.captured[*tapped..*keyed].contains(&bound),
            "{case}: the key names {bound:?}, not a frame captured after the tap: {:?}",
            self.captured
        );
    }
}

impl ContainedTaskRuntime for FramedDevice {
    type Error = &'static str;

    fn capture(&mut self) -> Result<ObservedFrame, Self::Error> {
        let mut observed = self.script.capture()?;
        let frame_id = *self
            .ids
            .mint_frame_id()
            .map_err(|_| "a frame id")?
            .transport();
        let reference = InputFrameReference {
            frame_id,
            width: observed.frame.width,
            height: observed.frame.height,
        };
        observed.input_reference = Some(reference);
        self.captured.push(reference);
        Ok(observed)
    }

    fn committed_input_frame(
        &mut self,
        reference: InputFrameReference,
    ) -> Result<Option<InputFrameContext>, Self::Error> {
        if self.captured.contains(&reference) {
            Ok(Some(InputFrameContext::for_test(reference)))
        } else {
            Ok(None)
        }
    }

    fn input(
        &mut self,
        action: InputAction,
        frame: Option<InputFrameContext>,
    ) -> Result<(), Self::Error> {
        self.inputs.push((
            action.clone(),
            frame.as_ref().map(InputFrameContext::reference),
            self.captured.len(),
        ));
        self.script.input(action, frame)
    }

    fn key_input_support(&mut self, key: StepKey) -> Result<KeyInputSupport, Self::Error> {
        self.script.key_input_support(key)
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

// Fix round 1: the base path in the source layout, the content table of a pack directory, an
// installed pack and a recorded package: the control declaration of two steps, the resource list
// and the one task, whose recognition the source parser derives from the task's colour probes
// (pixel 0 the page, pixel 1 the guard mark) and page rules. `open` taps from home into the list;
// `back`, guarded by the mark on the list, presses the key `word` to done.
fn source_entries(word: &str) -> BTreeMap<String, Vec<u8>> {
    let probe = |id: &str, x: u32, expected: [u8; 3]| {
        json!({"id": id,
               "region": {"mode": "rect", "rect": {"x": x, "y": 0, "width": 1, "height": 1}},
               "expected": expected})
    };
    let mut back = key_step(word, "list", "done");
    back["guard"] = json!({"page_id": "list", "target_id": "guard_ready",
                           "expected_rect": {"x": 1, "y": 0, "width": 1, "height": 1},
                           "color_probe": "guard_ready"});
    let task = json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "locale": "en-US",
        "goal": "KP-1 fixture",
        "coordinate_space": {"width": WIDTH, "height": 1},
        "defaults": {"template_threshold": 0.9, "color_max_distance": 0.0},
        "color_probes": [
            probe("page_home", 0, HOME),
            probe("page_list", 0, LIST),
            probe("page_done", 0, DONE),
            probe("guard_ready", 1, GREEN)
        ],
        "page_rules": {
            "home": {"required": ["page_home"]},
            "list": {"required": ["page_list"]},
            "done": {"required": ["page_done"]}
        },
        "entry_page": "home",
        "target_page": "done",
        "max_steps": 2,
        "operations": [tap("open", "home", "list"), back]
    });
    [
        ("control.json", control(2)),
        (
            "resources/operations/resources.json",
            json!({"schema_version": "1.0", "resources": [], "resource_count": 0}),
        ),
        ("resources/operations/task/task.json", task),
    ]
    .into_iter()
    .map(|(path, value)| (path.to_owned(), bytes(&value)))
    .collect()
}

// Fix round 1: the source package of `word`, as the product's entry admits it, or its refusal.
fn load_source(word: &str) -> Result<PreparedContainedTask, ContainedTaskError> {
    PreparedContainedTask::load_content_entries(
        "neutral.instance",
        source_entries(word),
        None,
        admission_deadline(),
    )
}

// A package whose second step is the back key: after `open`'s tap the list shows with the mark,
// `back`'s guard passes on that frame and its key goes out once, the input `Key` with Android's back
// code `4`, its intent naming the guard that passed; done comes and the run is Completed on it
// after both steps. The runtime was asked once whether its input sends the back key. On a list
// without the mark the guard refuses the step (`contained_task_guard_refused`): no key goes out and
// `back` writes no intent. Fix round 1 (the review: this test passed with a key sent without its
// frame): the run is on a device that commits every frame as the host does, and the key names a
// frame committed after the tap and before the key, the list's.
#[test]
fn kp1_a_back_key_step_sends_one_key_after_its_guard_and_goes_on_to_the_next_page() {
    let prepared = admitted("a key step", &path_task(key_step("back", "list", "done")));

    let mut framed = FramedDevice::new(path_screens());
    let outcome = prepared
        .run(&mut framed)
        .unwrap_or_else(|error| panic!("expected a normal end, got {error:?}"));
    framed.assert_key_bound_to_the_lists_frame("a key step");
    let device = &framed.script;

    assert_eq!(outcome, completed(DONE_PAGE, 2));
    assert_eq!(
        finished(device),
        [(0, "open", LIST_PAGE), (1, "back", DONE_PAGE)]
    );
    assert_eq!(device.inputs().len(), 2, "{:?}", device.inputs());
    assert_eq!(
        device.inputs()[1],
        InputAction::Key {
            key: "4".to_owned()
        }
    );
    assert_eq!(intents(device), ["open", "back"]);
    let (action, guard) = intent_of(device, "back").expect("back's effect intent");
    assert_eq!(action, &device.inputs()[1]);
    assert_eq!(
        guard,
        &ContainedTaskGuardOutcome::Passed {
            page_label: LIST_PAGE.to_owned(),
            target_id: "guard/ready".to_owned(),
            target_kind: "color".to_owned(),
        }
    );
    assert_eq!(device.key_input_asks(), &[StepKey::Back]);

    let (result, unmarked) = run(
        &prepared,
        ScriptedRuntime::new(vec![vec![screen(HOME, false)], vec![screen(LIST, false)]]),
    );
    let error = failed("the list without the mark", result);
    assert_eq!(error.code().as_str(), GUARD_REFUSED, "{error}");
    assert_eq!(unmarked.inputs().len(), 1, "{:?}", unmarked.inputs());
    assert_eq!(intents(&unmarked), ["open"]);
}

// After the key the list stays (the key was not taken): `back`'s arrival on done runs out and, as
// after a click step's last attempt, the closing decision follows and sees the step's own page.
// The run fails as a click step whose next page does not come fails: `page_confirmation_failed`
// with the click step's detail, the arrival's postcondition timing and the word `input_not_taken`;
// `back`'s `StepFinished` names no page and no second key goes out.
#[test]
fn kp1_the_next_page_not_coming_fails_as_a_click_step_does() {
    let prepared = admitted("a key step", &path_task(key_step("back", "list", "done")));

    let (result, device) = run(
        &prepared,
        ScriptedRuntime::new(vec![vec![screen(HOME, false)], vec![screen(LIST, true)]]),
    );
    let error = failed("the list stays", result);

    assert_eq!(error.code().as_str(), CONFIRMATION_FAILED, "{error}");
    assert_eq!(error.detail(), Some(BACK_UNCONFIRMED), "{error}");
    assert_eq!(
        timing(&error),
        Some((
            TaskTimingScope::Postcondition,
            TaskTimingStage::Postcondition,
            ARRIVAL_MS
        )),
        "{error}"
    );
    assert_eq!(
        value(&error, "confirmation_problem"),
        json!("input_not_taken"),
        "{:?}",
        error.outcome()
    );
    assert_eq!(
        finished(&device),
        [(0, "open", LIST_PAGE), (1, "back", UNRECOGNIZED)]
    );
    assert_eq!(device.inputs().len(), 2, "{:?}", device.inputs());
    assert_eq!(intents(&device), ["open", "back"]);
}

// A device whose input sends no key (as a capability table that says `input.key` unsupported): the
// run is refused after `RunStarted` and before any capture, input or effect intent, with
// `contained_task_linear_key_unsupported` naming the first key step (detail `operation=back`,
// value `pack_operation_id`). No step started, nothing was sent and nothing was intended, so the
// run's record holds no effect, none indeterminate; the runtime was asked once, for the back key.
// The same device runs a package without a key step as before and is asked nothing.
#[test]
fn kp1_a_backend_without_the_key_refuses_the_run_before_any_capture_input_or_intent() {
    let prepared = admitted("a key step", &path_task(key_step("back", "list", "done")));

    let (result, device) = run(
        &prepared,
        ScriptedRuntime::new(path_screens()).without_key_input(),
    );
    let error = failed("no key input", result);

    assert_eq!(error.code().as_str(), KEY_UNSUPPORTED, "{error}");
    assert_eq!(error.detail(), Some("operation=back"), "{error}");
    assert_eq!(value(&error, "pack_operation_id"), json!("back"));
    assert_eq!(device.key_input_asks(), &[StepKey::Back]);
    assert_eq!(device.captures(), 0);
    assert!(device.inputs().is_empty(), "{:?}", device.inputs());
    assert!(device.traces().contains(&ContainedTaskTrace::RunStarted));
    assert!(
        !device.traces().iter().any(|trace| matches!(
            trace,
            ContainedTaskTrace::CaptureCompleted { .. }
                | ContainedTaskTrace::StepStarted { .. }
                | ContainedTaskTrace::EffectIntent { .. }
                | ContainedTaskTrace::EffectCompleted { .. }
        )),
        "{:?}",
        device.traces()
    );

    let plain = admitted("no key step", &path_task(tap("back", "list", "done")));
    let (result, device) = run(
        &plain,
        ScriptedRuntime::new(path_screens()).without_key_input(),
    );
    let outcome =
        result.unwrap_or_else(|error| panic!("no key step: expected a normal end, got {error:?}"));
    assert_eq!(outcome, completed(DONE_PAGE, 2));
    assert!(device.key_input_asks().is_empty());
    assert_eq!(device.inputs().len(), 2, "{:?}", device.inputs());
}

// Admission, through the package's whole admission: a key whose word is not in the vocabulary
// (`home`) is `contained_task_linear_invalid` with `reason=key_unknown`; a key step that declares a
// retry is `reason=key_retry`; a step with a click and a key is `reason=operation_effect`; each
// names `back`.
#[test]
fn kp1_admission_refuses_an_unknown_key_a_retried_key_and_a_click_with_a_key() {
    let mut retried = key_step("back", "list", "done");
    retried["retryable"] = json!(true);
    retried["max_attempts"] = json!(2);
    retried["retry_interval_ms"] = json!(10);
    let mut both = tap("back", "list", "done");
    both["key"] = json!({"key": "back"});
    for (case, back, detail) in [
        (
            "an unknown word",
            key_step("home", "list", "done"),
            "reason=key_unknown operation=back",
        ),
        (
            "a key step with a retry",
            retried,
            "reason=key_retry operation=back",
        ),
        (
            "a click and a key",
            both,
            "reason=operation_effect operation=back",
        ),
    ] {
        let error = load(&path_task(back))
            .err()
            .unwrap_or_else(|| panic!("{case}: admitted"));
        assert_eq!(error.code().as_str(), LINEAR_INVALID, "{case}: {error}");
        assert_eq!(error.detail(), Some(detail), "{case}: {error}");
    }
}

// The revision pair (contracts/component-interfaces.md, the `package` row 17): this build reads
// `package` up to 17 (up to 18 since Workflow #463 K-c, register §1.454, up to 19 since
// Workflow #463 INT-1, register §1.473, and up to 20 since Workflow #463 SEL-2L, register
// §1.480, 17 within it), so at the installer's `package` edge
// (`package_edge_holds`) its range holds a bundle whose package has a key step, which declares
// [17, 17], and a range that ends below 17 refuses it; a bundle without a key step keeps its
// revision ([3, 15]) and both ranges hold it. This build admits both packages.
#[test]
fn kp1_a_package_with_a_key_step_needs_revision_17_and_one_without_keeps_its_revision() {
    let declared: Value =
        serde_json::from_str(OWN_DECLARATION).expect("the build's declaration is JSON");
    let own = match declared["speaks"]["package"].as_array().map(Vec::as_slice) {
        Some([min, max]) => [
            min.as_u64().expect("a minimum"),
            max.as_u64().expect("a maximum"),
        ],
        other => panic!("the build's package range: {other:?}"),
    };
    let earlier = [own[0], KEY_REVISION - 1];
    let with_key = [KEY_REVISION, KEY_REVISION];
    let without_key = [3, 15];

    // Workflow #463 K-c (register §1.454) declares 18, the scan select without an end, Workflow
    // #463 INT-1 (register §1.473) 19, an interrupt entry without handlers, Workflow #463
    // SEL-2L (register §1.480) 20, a repeated_anchor layout's shared read, Workflow #463
    // SEL-GRID G2 (register §1.491) 21, the lines of a shared read's name boxes, Workflow
    // #428 S168-LK (register §1.500) 22, a linear step's list reading, Workflow #455 G-SLx
    // (register §1.501) 23, a pack's variables beside an irreversible capability, Workflow #477
    // CV-1 (register §1.507) 24, a bundle that carries a version 2 package declaration,
    // Workflow #477 LR-1 (register §1.513) 25, a linear press step's count, Workflow #455 TK-1
    // (register §1.532) 26, a variable only a rule gives, Workflow #503 LIST-A (register §1.533)
    // 27, a list reading of the wanted keys only, Workflow #455 RR1 (register §1.535) 28, a
    // `goals.json` of version 3 and a root's `goal-tables.json`, and Workflow #503 LIST-B
    // (register §1.536) 29, a `repeated_anchor` layout's picture pool.
    assert_eq!(own, [3, KEY_REVISION + 12]);
    assert!(package_edge_holds(own, with_key));
    assert!(!package_edge_holds(earlier, with_key));
    assert!(package_edge_holds(own, without_key));
    assert!(package_edge_holds(earlier, without_key));
    admitted("a key step", &path_task(key_step("back", "list", "done")));
    admitted("no key step", &path_task(tap("back", "list", "done")));
}

// The offline first decision of a package whose first step is the back key, on one frame of the
// list with the mark: `would_press_key`, naming `back`, the key `back` and the guard that passed,
// never a click.
#[test]
fn kp1_the_offline_first_decision_names_the_key_step() {
    let prepared = admitted(
        "a first key step",
        &task("list", "done", vec![key_step("back", "list", "done")]),
    );
    let frame = Frame::from_pixels(
        WIDTH,
        1,
        [LIST, GREEN].concat(),
        PixelFormat::Rgb8,
        CaptureBackendName::FixtureSimulation,
    )
    .expect("fixture frame");

    let simulated = crate::simulate_contained_task(&prepared, vec![frame])
        .expect("the dry run simulates the frame");

    let written = serde_json::to_value(&simulated.decision).expect("the decision as JSON");
    assert_eq!(written["status"], json!("would_press_key"));
    assert_eq!(written["key"], json!("back"));
    match &simulated.decision {
        crate::OfflineDecision::WouldPressKey {
            operation_label,
            key: pressed,
            guard,
        } => {
            assert_eq!(operation_label.as_str(), "back");
            assert_eq!(*pressed, StepKey::Back);
            assert_eq!(
                guard,
                &ContainedTaskGuardOutcome::Passed {
                    page_label: LIST_PAGE.to_owned(),
                    target_id: "guard/ready".to_owned(),
                    target_kind: "color".to_owned(),
                }
            );
        }
        other => panic!("expected the key step's press, got {other:?}"),
    }
}

// Fix round 1, the gate in the product's form (the review's first point: every path the product
// loads a package by goes through the source parser, which refused a step without a click): the
// base path in the source layout, as a pack directory, an installed pack and a recorded package
// hold it, is admitted by the product's entry (`load_content_entries`: the declaration gate, the
// source parser, kernel admission). Run on a device that commits every frame as the host does,
// `open`'s tap shows the list with the mark, `back`'s guard passes there (its declared guard, the
// page canonical, the target `guard_ready`, a colour target) and its key goes out once, the input
// `Key` `4`, bound to a frame committed after the tap, and its intent names the guard that passed;
// done comes and the run is Completed on it after both steps, the runtime asked once for the back
// key. The same package whose key word is `home` is refused at admission
// (`contained_task_linear_invalid`, `reason=key_unknown operation=back`).
#[test]
fn kp1_fix1_a_source_package_with_a_key_step_is_admitted_by_the_products_entry_and_runs() {
    let prepared =
        load_source("back").unwrap_or_else(|error| panic!("the source package: refused: {error}"));

    let mut framed = FramedDevice::new(path_screens());
    let outcome = prepared
        .run(&mut framed)
        .unwrap_or_else(|error| panic!("expected a normal end, got {error:?}"));

    assert_eq!(outcome.outcome, TaskOutcome::Success);
    assert_eq!(outcome.final_page.as_deref(), Some(DONE_PAGE));
    assert_eq!(outcome.executed_steps, 2);
    framed.assert_key_bound_to_the_lists_frame("the source package");
    let device = &framed.script;
    assert_eq!(
        finished(device),
        [(0, "open", LIST_PAGE), (1, "back", DONE_PAGE)]
    );
    assert_eq!(intents(device), ["open", "back"]);
    let (action, guard) = intent_of(device, "back").expect("back's effect intent");
    assert_eq!(
        action,
        &InputAction::Key {
            key: "4".to_owned()
        }
    );
    assert_eq!(
        guard,
        &ContainedTaskGuardOutcome::Passed {
            page_label: LIST_PAGE.to_owned(),
            target_id: "guard_ready".to_owned(),
            target_kind: "color".to_owned(),
        }
    );
    assert_eq!(device.key_input_asks(), &[StepKey::Back]);

    let error = load_source("home")
        .err()
        .unwrap_or_else(|| panic!("the word home: admitted"));
    assert_eq!(error.code().as_str(), LINEAR_INVALID, "{error}");
    assert_eq!(
        error.detail(),
        Some("reason=key_unknown operation=back"),
        "{error}"
    );
}
