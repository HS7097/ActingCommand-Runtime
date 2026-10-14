// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #386 SR-1 (scheduling model v1.4.2 §2.1, §2.2, §2.4; test rows T1, T2, T2b, T2c, T3,
// T4, T5 and T33 of §13). An observation whose sampling runs past its window is made again in a
// new window, never repeating an input, as many times as the step's pacing says; spent retries
// end the run with the timeout's code and `attempts`, `timeout_ms` and `stage`. The runtime's
// checkpoint only answers cancellation; whether it samples is `sampling_supported`. Admission
// refuses a package whose declared sampling cannot fit with `recognition_sample_budget_invalid`.
//
// The observation clock is the runtime's (`observation_now`): the fake below keeps it still and
// moves it only at scripted moments, so a window runs out exactly where a row says and nowhere
// else. Every expected value comes from the model rows named on each test.

use super::*;
use actingcommand_contract::outcome::Category;
use actingcommand_contract::{InstanceFactContext, InstanceFactSnapshot, PackageRef};
use actingcommand_device::CaptureBackendName;
use actingcommand_recognition_pack::RecognitionPack;
use serde_json::{Value, json};
use std::io::{Cursor, Write};
use std::path::PathBuf;
use zip::{ZipWriter, write::FileOptions};

// The step timeout, the window of every observation of these tasks.
const STEP_TIMEOUT: Duration = Duration::from_secs(10);
const TASK_TIMEOUT: Duration = Duration::from_secs(60);
// One move of the observation clock by more than a window ends the window in progress.
const PAST_WINDOW: Duration = Duration::from_secs(11);
const CANCELLED: &str = "cancelled by the runtime";
const RETIRED: &str = "recognition_sample_deadline_or_permission_lost";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Event {
    StepStarted,
    SelectionEvaluated,
    EffectIntent,
    Input,
    Other,
}

// A frame's left pixel names its page; its right pixel is the guard's colour.
fn page_frame(page: &str) -> Frame {
    let color = match page {
        "home" => [255, 0, 0],
        "terminal" => [0, 0, 255],
        _ => panic!("unknown fixture page {page}"),
    };
    Frame::from_pixels(
        2,
        1,
        [color, [0, 255, 0]].concat(),
        PixelFormat::Rgb8,
        CaptureBackendName::FixtureSimulation,
    )
    .expect("fixture frame")
}

// The Runtime as the kernel sees it: it samples, answers cancellation through its checkpoint,
// gives `retries` observe-only retries per step, and shows `home` until its first input and
// `terminal` after it. Its observation clock starts at the run's start and moves only by the
// scripted jumps.
struct PacedRuntime {
    retries: u32,
    cancelled: bool,
    origin: Option<Instant>,
    offset: Duration,
    capture_jumps: BTreeMap<usize, Duration>,
    step_started_jump: Option<Duration>,
    seed_jump: Option<Duration>,
    intent_jump: Option<Duration>,
    selection_jump: Option<Duration>,
    captures: usize,
    inputs: usize,
    events: Vec<Event>,
}

impl PacedRuntime {
    fn new(retries: u32) -> Self {
        Self {
            retries,
            cancelled: false,
            origin: None,
            offset: Duration::ZERO,
            capture_jumps: BTreeMap::new(),
            step_started_jump: None,
            seed_jump: None,
            intent_jump: None,
            selection_jump: None,
            captures: 0,
            inputs: 0,
            events: Vec::new(),
        }
    }

    fn jump(&mut self, by: Option<Duration>) {
        if let Some(by) = by {
            self.offset += by;
        }
    }

    fn count(&self, event: Event) -> usize {
        self.events.iter().filter(|seen| **seen == event).count()
    }
}

impl ContainedTaskRuntime for PacedRuntime {
    type Error = &'static str;

    fn observe_task_timing(&mut self, context: ContainedTaskTimingContext) {
        if self.origin.is_none() {
            self.origin = Some(context.deadline() - TASK_TIMEOUT);
        }
    }

    fn sampling_supported(&self) -> bool {
        true
    }

    fn candidate_sampling_checkpoint(&mut self) -> Result<(), Self::Error> {
        if self.cancelled {
            Err(CANCELLED)
        } else {
            Ok(())
        }
    }

    fn step_pacing(&mut self) -> Result<StepPacing, Self::Error> {
        Ok(StepPacing::new(self.retries))
    }

    fn observation_now(&mut self) -> Instant {
        self.origin
            .expect("the run gives its timing before it observes")
            + self.offset
    }

    fn capture(&mut self) -> Result<ObservedFrame, Self::Error> {
        self.captures += 1;
        let jump = self.capture_jumps.remove(&self.captures);
        self.jump(jump);
        let page = if self.inputs == 0 { "home" } else { "terminal" };
        Ok(page_frame(page).into())
    }

    fn action_seed(
        &mut self,
        _step_index: u32,
        _operation_label: &str,
    ) -> Result<Option<u64>, Self::Error> {
        let jump = self.seed_jump.take();
        self.jump(jump);
        Ok(None)
    }

    fn selection_state(
        &mut self,
        request: SelectionStateRequest,
    ) -> Result<SelectionState, Self::Error> {
        Ok(SelectionState::Snapshot {
            snapshot: InstanceFactSnapshot {
                snapshot_id: "snapshot:test:empty".to_owned(),
                ledger_position: 1,
                context: InstanceFactContext {
                    instance_id: "neutral.instance".to_owned(),
                    server_id: request.server,
                    game_id: request.game,
                },
                records: Vec::new(),
            },
            now_unix_ms: 1_000_000,
        })
    }

    fn input(
        &mut self,
        _action: InputAction,
        _frame: Option<InputFrameContext>,
    ) -> Result<(), Self::Error> {
        self.inputs += 1;
        self.events.push(Event::Input);
        Ok(())
    }

    fn record(&mut self, trace: ContainedTaskTrace) -> Result<(), Self::Error> {
        let event = match trace {
            ContainedTaskTrace::StepStarted { .. } => {
                let jump = self.step_started_jump.take();
                self.jump(jump);
                Event::StepStarted
            }
            ContainedTaskTrace::EffectIntent { .. } => {
                let jump = self.intent_jump.take();
                self.jump(jump);
                Event::EffectIntent
            }
            ContainedTaskTrace::SelectionEvaluated { .. } => {
                let jump = self.selection_jump.take();
                self.jump(jump);
                Event::SelectionEvaluated
            }
            _ => Event::Other,
        };
        self.events.push(event);
        Ok(())
    }
}

// A runtime with the trait's defaults (offline simulation, fixtures): it does not sample. It
// shows `terminal` throughout.
#[derive(Default)]
struct DefaultRuntime {
    captures: usize,
}

impl ContainedTaskRuntime for DefaultRuntime {
    type Error = &'static str;

    fn capture(&mut self) -> Result<ObservedFrame, Self::Error> {
        self.captures += 1;
        Ok(page_frame("terminal").into())
    }

    fn input(
        &mut self,
        _action: InputAction,
        _frame: Option<InputFrameContext>,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    fn record(&mut self, _trace: ContainedTaskTrace) -> Result<(), Self::Error> {
        Ok(())
    }
}

fn control() -> TaskControl {
    serde_json::from_value(json!({
        "schema_version": "Lab-1y.control.v1",
        "package_id": "neutral.sampled.task",
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": 2, "height": 1},
        "entry_task_id": "task",
        "capture_interval_ms": 1,
        "step_timeout_ms": STEP_TIMEOUT.as_millis() as u64,
        "timeout_ms": TASK_TIMEOUT.as_millis() as u64
    }))
    .expect("task control")
}

fn guard() -> Value {
    json!({
        "page_id": "home",
        "target_id": "guard/ready",
        "expected_rect": {"x": 1, "y": 0, "width": 1, "height": 1},
        "color_probe": "guard/ready"
    })
}

// The recognition pack: the two pages and the guard; with `consensus_interval_ms`, page `home`
// needs two frames that far apart; with `layout`, a candidate layout.
fn pack_json(consensus_interval_ms: Option<u64>, layout: Option<Value>) -> Value {
    let mut pack = json!({
        "schema_version": "0.7",
        "game": "neutral",
        "server": "test",
        "coordinate_space": {"width": 2, "height": 1},
        "defaults": {"color_max_distance": 0.0},
        "targets": [
            {"type": "color", "id": "page/home",
             "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": [255, 0, 0]},
            {"type": "color", "id": "page/terminal",
             "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": [0, 0, 255]},
            {"type": "color", "id": "guard/ready",
             "region": {"x": 1, "y": 0, "width": 1, "height": 1}, "expected": [0, 255, 0]}
        ]
    });
    if let Some(interval_ms) = consensus_interval_ms {
        pack["target_consensus"] = json!({
            "page/home": {"samples": [{"frame": 0}, {"frame": 1}], "k": 2,
                          "sample_interval_ms": interval_ms}
        });
    }
    if let Some(layout) = layout {
        pack["candidate_layouts"] = json!([layout]);
    }
    pack
}

// The pack of the in-memory tasks; with `consensus`, page `home` needs two frames 1 ms apart.
fn pack(consensus: bool, layout: Option<Value>) -> RecognitionPack {
    serde_json::from_value(pack_json(consensus.then_some(1), layout)).expect("recognition pack")
}

fn task(operation: TaskOperation, pack: RecognitionPack) -> PreparedContainedTask {
    let control = control();
    let mut program: TaskProgram = serde_json::from_value(json!({
        "schema_version": "0.6",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": 2, "height": 1},
        "target_page": "terminal",
        "operations": []
    }))
    .expect("task program");
    program.operations.push(operation);
    let evaluator = RecognitionEvaluator::new(PathBuf::new(), pack).expect("recognition evaluator");
    let page_set: PageSet = serde_json::from_value(json!({
        "schema_version": "0.3",
        "pages": [
            {"id": "neutral/home", "required": ["page/home"]},
            {"id": "neutral/terminal", "required": ["page/terminal"]}
        ]
    }))
    .expect("page set");
    let detector = PageDetector::new(page_set).expect("page detector");
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
        package_sha256: "fixture-sha256".into(),
        entry_count: 1,
        task_count: 1,
    }
}

// One guarded click from `home` to `terminal`; with `consensus`, every observation samples.
fn click_task(consensus: bool) -> PreparedContainedTask {
    let control = control();
    let mut operation: TaskOperation = serde_json::from_value(json!({
        "id": "open_terminal",
        "from": "home",
        "to": "terminal",
        "click": {"kind": "point", "x": 1, "y": 0},
        "guard": guard()
    }))
    .expect("task operation");
    operation
        .validate(&control, TaskOperationDefaults::default(), "0.6")
        .expect("valid operation");
    task(operation, pack(consensus, None))
}

// One select step from `home` to `terminal` over one slot whose `ready` feature needs two frames.
fn select_task() -> PreparedContainedTask {
    let layout = json!({
        "id": "choices",
        "page_id": "neutral/home",
        "kind": "fixed_slots",
        "sample_interval_ms": 1,
        "features": [{
            "name": "ready",
            "value": "passed",
            "consensus": {"samples": [{"frame": 0}, {"frame": 1}],
                          "aggregate": {"kind": "k_of_n", "k": 2}}
        }],
        "slots": [{
            "rect": {"x": 0, "y": 0, "width": 2, "height": 1},
            "click": {"x": 0, "y": 0, "width": 2, "height": 1},
            "targets": {"ready": "guard/ready"}
        }]
    });
    let policy: actingcommand_selection_policy::SelectionPolicy = serde_json::from_value(json!({
        "schema_version": "actingcommand.selection-policy.v1",
        "policy_id": "policy.ready",
        "applies_to": {
            "candidate_layout_id": "choices",
            "outcome_keys": {"selected": "ready_selected", "empty": "ready_empty",
                             "insufficient": "ready_insufficient",
                             "ambiguous": "ready_ambiguous", "unknown": "ready_unknown"}
        },
        "fields": [{"name": "ready", "value_type": {"type": "boolean"}}],
        "facts": [],
        "gates": [{"gate_id": "ready",
                   "predicate": {"kind": "boolean_equals",
                                 "value": {"source": "field", "field": "ready"},
                                 "expected": true},
                   "on_unknown": {"kind": "drop_candidate"}}],
        "scoring": [],
        "selection": {"mode": "exactly_one", "required_count": 1},
        "tie_break": [{"kind": "candidate_id", "direction": "lowest_first"}]
    }))
    .expect("selection policy");
    policy.validate().expect("valid selection policy");
    let mut operation: TaskOperation = serde_json::from_value(json!({
        "id": "pick",
        "from": "home",
        "to": "terminal",
        "guard": guard(),
        "select": {"layout_id": "choices",
                   "policy": {"path": "policies/select.json", "sha256": "b".repeat(64)}}
    }))
    .expect("select operation");
    operation.prepared_select = Some(Box::new(selection::PreparedSelect::for_test(
        "choices", policy,
    )));
    task(operation, pack(false, Some(layout)))
}

fn task_failure(
    result: Result<ContainedTaskOutcome, ContainedTaskRunError<&'static str>>,
) -> ContainedTaskError {
    match result {
        Err(ContainedTaskRunError::Task(error)) => error,
        Err(other) => panic!("expected a task failure, got {other:?}"),
        Ok(outcome) => panic!(
            "expected a task failure, the run ended {:?}",
            outcome.outcome
        ),
    }
}

fn values(error: &ContainedTaskError) -> Value {
    Value::Object(error.outcome().values().clone())
}

fn succeeded(result: Result<ContainedTaskOutcome, ContainedTaskRunError<&'static str>>) {
    match result {
        Ok(outcome) => assert_eq!(outcome.outcome, TaskOutcome::Success),
        Err(error) => panic!("expected the run to succeed, got {error:?}"),
    }
}

// T1 (L726 「先再试几次」; §2.1 items 2 and 4): the guard's observation runs past its window
// while the task has time. With a base of two retries the same observation is made again in a
// new window and the run goes on: the second observation does not time out on the first one's
// deadline. With a base of zero the run ends at once.
#[test]
fn sr1_t1_a_timed_out_observation_is_made_again_in_a_new_window() {
    let task = click_task(true);
    let mut runtime = PacedRuntime::new(2);
    runtime.step_started_jump = Some(PAST_WINDOW);
    succeeded(task.run(&mut runtime));
    // Two frames for the first page, two for the guard's new observation, two for the
    // destination.
    assert_eq!(runtime.captures, 6);
    assert_eq!(runtime.inputs, 1);

    let mut runtime = PacedRuntime::new(0);
    runtime.step_started_jump = Some(PAST_WINDOW);
    let error = task_failure(task.run(&mut runtime));
    assert_eq!(
        error.code().as_str(),
        ExecutionKernelCode::RecognitionSampleDeadlineExceeded.as_str()
    );
    assert_eq!(
        values(&error),
        json!({"attempts": 0, "timeout_ms": 10_000, "stage": "observation_before_input"})
    );
    assert_eq!(runtime.inputs, 0);
}

// T1 (§2.1 items 4 and 6): every retry's observation runs past its own new window too; once the
// two retries are spent the run ends with the timeout, `attempts` equal to the retries, the
// window and the observation, and no input.
#[test]
fn sr1_t1_spent_retries_end_the_run_with_the_timeout_and_its_attempts() {
    let task = click_task(true);
    let mut runtime = PacedRuntime::new(2);
    runtime.step_started_jump = Some(PAST_WINDOW);
    // The first sample frame of each new observation (captures 3 and 4) runs past its window.
    runtime.capture_jumps = BTreeMap::from([(3, PAST_WINDOW), (4, PAST_WINDOW)]);
    let error = task_failure(task.run(&mut runtime));
    assert_eq!(
        error.code().as_str(),
        ExecutionKernelCode::RecognitionSampleDeadlineExceeded.as_str()
    );
    assert_eq!(error.outcome().category(), Category::Error);
    assert_eq!(
        values(&error),
        json!({"attempts": 2, "timeout_ms": 10_000, "stage": "observation_before_input"})
    );
    assert_eq!(runtime.captures, 4);
    assert_eq!(runtime.inputs, 0);
    assert_eq!(runtime.count(Event::EffectIntent), 0);
}

// T2 (§2.1 items 1 and 2): a retry before the input writes no second `StepStarted` and no
// second intent; a retry of the observation after the input observes again and never inputs
// again, and spent retries there end the run after the one input.
#[test]
fn sr1_t2_a_retry_never_repeats_an_input_or_a_step_start() {
    let task = click_task(true);
    let mut runtime = PacedRuntime::new(1);
    runtime.step_started_jump = Some(PAST_WINDOW);
    succeeded(task.run(&mut runtime));
    assert_eq!(runtime.count(Event::StepStarted), 1);
    assert_eq!(runtime.count(Event::EffectIntent), 1);
    assert_eq!(runtime.inputs, 1);

    // After the input: the destination's first sample frame (capture 3) runs past its window.
    let mut runtime = PacedRuntime::new(1);
    runtime.capture_jumps = BTreeMap::from([(3, PAST_WINDOW)]);
    succeeded(task.run(&mut runtime));
    assert_eq!(runtime.inputs, 1);
    assert_eq!(runtime.captures, 5);

    let mut runtime = PacedRuntime::new(0);
    runtime.capture_jumps = BTreeMap::from([(3, PAST_WINDOW)]);
    let error = task_failure(task.run(&mut runtime));
    assert_eq!(
        error.code().as_str(),
        ExecutionKernelCode::RecognitionSampleDeadlineExceeded.as_str()
    );
    assert_eq!(
        values(&error),
        json!({"attempts": 0, "timeout_ms": 10_000, "stage": "observation_after_input"})
    );
    assert_eq!(runtime.inputs, 1);
}

// T2b (§2.1 item 3; ledger model §5.1): the sampling deadline is checked before the effect
// intent, never after it. A window that runs out between the guard and the intent is retried
// before any intent; one that runs out once the intent is written no longer stops the input.
// Every intent is followed by its input.
#[test]
fn sr1_t2b_the_sampling_deadline_is_checked_before_the_intent_only() {
    let task = click_task(true);
    let intent_then_input = |runtime: &PacedRuntime| {
        runtime
            .events
            .iter()
            .enumerate()
            .filter(|(_, event)| **event == Event::EffectIntent)
            .all(|(index, _)| runtime.events.get(index + 1) == Some(&Event::Input))
    };

    let mut runtime = PacedRuntime::new(1);
    runtime.seed_jump = Some(PAST_WINDOW);
    succeeded(task.run(&mut runtime));
    assert_eq!(runtime.count(Event::EffectIntent), 1);
    assert!(intent_then_input(&runtime), "{:?}", runtime.events);
    // The guard's new observation (captures 3 and 4) precedes the one intent.
    assert_eq!(runtime.captures, 6);

    let mut runtime = PacedRuntime::new(0);
    runtime.seed_jump = Some(PAST_WINDOW);
    let error = task_failure(task.run(&mut runtime));
    assert_eq!(
        error.code().as_str(),
        ExecutionKernelCode::RecognitionSampleDeadlineExceeded.as_str()
    );
    assert_eq!(runtime.count(Event::EffectIntent), 0);

    let mut runtime = PacedRuntime::new(0);
    runtime.intent_jump = Some(PAST_WINDOW);
    succeeded(task.run(&mut runtime));
    assert_eq!(runtime.count(Event::EffectIntent), 1);
    assert!(intent_then_input(&runtime), "{:?}", runtime.events);
}

// T2c (L726; §2.1 "列表选择包"): the candidate sampling's check after the selection record
// (SEL:809) runs past the window. The selection is observed and made again, recording a second
// selection, without repeating an input; with no retry the run ends with
// `candidate_sample_deadline_exceeded` and its attempts.
#[test]
fn sr1_t2c_a_candidate_sampling_timeout_is_observed_again_by_the_same_rule() {
    let task = select_task();
    let mut runtime = PacedRuntime::new(1);
    runtime.selection_jump = Some(PAST_WINDOW);
    succeeded(task.run(&mut runtime));
    assert_eq!(runtime.count(Event::SelectionEvaluated), 2);
    assert_eq!(runtime.count(Event::StepStarted), 1);
    assert_eq!(runtime.inputs, 1);

    let mut runtime = PacedRuntime::new(0);
    runtime.selection_jump = Some(PAST_WINDOW);
    let error = task_failure(task.run(&mut runtime));
    assert_eq!(
        error.code().as_str(),
        ExecutionKernelCode::CandidateSampleDeadlineExceeded.as_str()
    );
    assert_eq!(
        values(&error),
        json!({"attempts": 0, "timeout_ms": 10_000, "stage": "observation_before_input"})
    );
    assert_eq!(runtime.count(Event::SelectionEvaluated), 1);
    assert_eq!(runtime.inputs, 0);
}

// T3 (L726; §2.1 item 5, §2.3): when the task's remaining time cannot hold another
// observation, no retry captures anything: the run ends with
// `recognition_sample_budget_insufficient`, what the recognition needs and what is left.
#[test]
fn sr1_t3_no_time_for_another_window_ends_with_the_budget_code() {
    let task = click_task(true);
    let mut runtime = PacedRuntime::new(2);
    runtime.step_started_jump = Some(TASK_TIMEOUT);
    let error = task_failure(task.run(&mut runtime));
    assert_eq!(
        error.code().as_str(),
        ExecutionKernelCode::RecognitionSampleBudgetInsufficient.as_str()
    );
    let required_ms = selection::page_recognition_budget_ms(&task.evaluator, &task.detector, None)
        .expect("the page recognition budget");
    assert_eq!(
        values(&error),
        json!({"required_ms": required_ms, "remaining_ms": 0})
    );
    assert_eq!(runtime.captures, 2);
    assert_eq!(runtime.inputs, 0);
}

// T4 (L726 「权限丢失：不存在」; §2.2): a cancellation the runtime's checkpoint reports ends the
// run with the runtime's own failure, unchanged; the retired code is no registry entry and the
// catalog lists it as retired, replaced by `recognition_sample_deadline_exceeded`.
#[test]
fn sr1_t4_a_cancellation_ends_the_run_under_its_own_code_and_permission_lost_is_retired() {
    let task = click_task(true);
    let mut runtime = PacedRuntime::new(2);
    runtime.cancelled = true;
    match task.run(&mut runtime) {
        Err(ContainedTaskRunError::Boundary(error)) => assert_eq!(error, CANCELLED),
        other => panic!("expected the runtime's cancellation, got {other:?}"),
    }
    assert_eq!(runtime.inputs, 0);

    assert!(
        ExecutionKernelCode::ALL
            .iter()
            .all(|code| code.as_str() != RETIRED)
    );
    let fragment: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../contracts/outcome-codes/execution-kernel.json"
    )))
    .expect("the kernel's fragment");
    let entry = &fragment["codes"][RETIRED];
    assert_eq!(entry["status"], "retired");
    assert_eq!(
        entry["replaced_by"],
        ExecutionKernelCode::RecognitionSampleDeadlineExceeded.as_str()
    );
}

fn zip_entries(entries: &[(&str, Value)]) -> Vec<u8> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, value) in entries {
        zip.start_file(*path, options).expect("zip entry");
        serde_json::to_writer(&mut zip, value).expect("zip JSON");
        zip.write_all(b"\n").expect("zip newline");
    }
    zip.finish().expect("finish zip").into_inner()
}

// A package whose page `home` declares two frames 100 ms apart while its step timeout is 50 ms.
fn admit(control: Value, task: Value) -> Result<PreparedContainedTask, ContainedTaskError> {
    let pack = pack_json(Some(100), None);
    let bytes = zip_entries(&[
        ("control.json", control),
        (
            "resources/manifest.json",
            json!({"schema_version": "0.3", "entry_task_id": "task"}),
        ),
        ("resources/operations/task/task.json", task),
        ("resources/recognition/neutral.test.pack.json", pack),
        (
            "resources/recognition/neutral.test.pages.json",
            json!({"schema_version": "0.3", "pages": [
                {"id": "neutral/home", "required": ["page/home"], "optional": [], "forbidden": []},
                {"id": "neutral/terminal", "required": ["page/terminal"], "optional": [],
                 "forbidden": []}
            ]}),
        ),
    ]);
    let expected = Sha256Hash::digest(&bytes).to_string();
    PreparedContainedTask::load(
        "neutral.instance",
        &bytes,
        &PackageRef::LegacyZipSha256(expected),
    )
}

// T5 (L726; §2.4 item 1): a package whose declared sampling exceeds the smaller of its step and
// task timeouts is refused at admission with `recognition_sample_budget_invalid`, what it needs
// and the limit; a linear package the same.
#[test]
fn sr1_t5_admission_refuses_sampling_beyond_the_timeouts_as_budget_invalid() {
    let operation = json!({
        "id": "open_terminal",
        "from": "home",
        "to": "terminal",
        "click": {"kind": "point", "x": 1, "y": 0},
        "unguarded_trusted_coordinate": true,
        "retryable": false
    });
    let page_graph = admit(
        json!({
            "schema_version": "Lab-1y.control.v1",
            "package_id": "neutral.sampled.task",
            "execution_mode": "navigable_route",
            "game": "neutral",
            "server": "test",
            "resolution": {"width": 2, "height": 1},
            "entry_task_id": "task",
            "capture_interval_ms": 1,
            "step_timeout_ms": 50,
            "timeout_ms": 5000
        }),
        json!({
            "schema_version": "0.6",
            "task_id": "task",
            "game": "neutral",
            "server_scope": ["test"],
            "coordinate_space": {"width": 2, "height": 1},
            "target_page": "terminal",
            "operations": [operation.clone()]
        }),
    );
    let linear = admit(
        json!({
            "schema_version": "Lab-1y.control.v2",
            "package_id": "neutral.sampled.task",
            "execution_mode": "linear_steps",
            "game": "neutral",
            "server": "test",
            "resolution": {"width": 2, "height": 1},
            "entry_task_id": "task",
            "capture_interval_ms": 1,
            "step_timeout_ms": 50,
            "timeout_ms": 5000,
            "max_steps": 1
        }),
        json!({
            "schema_version": "0.9",
            "task_id": "task",
            "game": "neutral",
            "server_scope": ["test"],
            "coordinate_space": {"width": 2, "height": 1},
            "entry_page": "home",
            "target_page": "terminal",
            "max_steps": 1,
            "operations": [operation]
        }),
    );
    for refused in [page_graph, linear] {
        let error = refused.err().expect("admission refuses the package");
        assert_eq!(
            error.code().as_str(),
            ExecutionKernelCode::RecognitionSampleBudgetInvalid.as_str()
        );
        assert_eq!(values(&error), json!({"required_ms": 100, "limit_ms": 50}));
    }
}

// T33 (§2.2 call-site table): a runtime with the trait's defaults keeps today's behaviour. Its
// required home entry takes one frame and does not wait; multi-frame target and candidate
// sampling are still refused as unsupported.
#[test]
fn sr1_t33_a_runtime_without_sampling_keeps_the_single_frame_and_refuses_sampling() {
    let mut task = click_task(false);
    task.entry_page = Some(RequiredEntryPages::single("neutral/home".to_owned()));
    let mut runtime = DefaultRuntime::default();
    let error = match task.run(&mut runtime) {
        Err(ContainedTaskRunError::Task(error)) => error,
        other => panic!("expected the entry's refusal, got {other:?}"),
    };
    assert_eq!(
        error.code().as_str(),
        SharedCode::ContainedTaskHomeEntryNotMatched.as_str()
    );
    assert_eq!(runtime.captures, 1);

    let mut runtime = DefaultRuntime::default();
    let error = match click_task(true).run(&mut runtime) {
        Err(ContainedTaskRunError::Task(error)) => error,
        other => panic!("expected the sampling refusal, got {other:?}"),
    };
    assert_eq!(
        error.code().as_str(),
        ExecutionKernelCode::RecognitionSamplingRuntimeUnsupported.as_str()
    );
    assert_eq!(runtime.captures, 0);
}
