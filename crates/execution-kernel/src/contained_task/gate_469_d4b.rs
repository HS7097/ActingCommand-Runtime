// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #469 D4b (0.12.63; register §1.401; the owner's approval on board #469 on 11-15 of the
// four corrections that change what the program does, this one of them; the design
// `rd-1115/status-codes-a-design.md`, Part 2, A-2 item 1, the row of
// `contained_task_guard_evaluation_failed` and `contained_task_recognition_failed`, and the slice
// row D4; coordinator work order `rd-1115/brief-469-d4-owner-four.md`).
//
// A vision provider that fails, or answers invalidly, while the kernel evaluates a step's guard
// target, an operation's `when` target or an expected identity's target ends the run with the
// class-5 `contained_task_vision_provider_failed`, as a page evaluation's does (Workflow #387
// D1c-G), told apart as there by the cause of the evaluation's error, with the place's detail and
// no values. A real evaluation error keeps the place's own code and its detail byte for byte:
// `contained_task_guard_evaluation_failed` at the guard, `contained_task_recognition_failed` at the
// `when` and the identity; here a located text read that cannot conclude, because the provider
// kept fewer boxes than the screen held (the provider answered as it should). A provider that reads
// the name leaves the three places as they were: the guard passes and the `when` holds, each run
// taps once and ends on its target page, its records byte for byte as before (fix round 2: every
// trace, with the values the step traces carry, and every guard evaluation, in order), and the
// identity reads the business ID the name maps to.
//
// A scripted OCR provider answers every text read; the pages are colours, so a page evaluation
// never calls it. The runs play scripted frames on the scripted clock (EXEC-I4): the home page
// until the first input, the terminal page after it. The identity's reading of one destination
// frame is asked directly; the run's `expect_identity` returns its failure as it is.

use super::tests::scripted::{ScriptedFrame, ScriptedRuntime};
use super::*;
use actingcommand_contract::outcome::LadderClass;
use actingcommand_device::CaptureBackendName;
use actingcommand_recognition_pack::{
    CandidateIdentityDeclaration, FsAssetResolver, NnProviderRequest, NnProviderResult,
    OcrProviderDetection, OcrProviderRequest, OcrProviderResult, OcrProviderTextBlock,
    RecognitionPackCode, RecognitionPackError, VisionProviderError, VisionProviderErrorCode,
    load_pack_from_json_str,
};
use serde_json::{Value, json};
use std::path::PathBuf;

const MODEL_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const TASK_FILE: &str = "resources/operations/task/task.json";
const WIDTH: u32 = 8;
const HEIGHT: u32 = 4;
// The located OCR target the three places evaluate, and the text it expects.
const TEXT: &str = "text/name";
const NAME: &str = "NAME";
const HOME: [u8; 3] = [255, 0, 0];
const TERMINAL: [u8; 3] = [0, 0, 255];
const HOME_PAGE: &str = "neutral/home";
const TERMINAL_PAGE: &str = "neutral/terminal";

// What the scripted provider answers to every text read.
#[derive(Debug, Clone, Copy)]
enum Answer {
    // The provider fails with this code.
    Fails(VisionProviderErrorCode),
    // The provider reads a text that is not the expected one, and reports that the screen held
    // more boxes than it kept: the located read cannot conclude.
    CrowdedMiss,
    // The provider reads the expected name.
    Reads,
}

// The four ways a provider fails.
const PROVIDER_FAILURES: [VisionProviderErrorCode; 4] = [
    VisionProviderErrorCode::Unavailable,
    VisionProviderErrorCode::Timeout,
    VisionProviderErrorCode::Internal,
    VisionProviderErrorCode::InvalidResponse,
];

#[derive(Debug)]
struct ScriptedOcr {
    answer: Answer,
}

impl VisionProvider for ScriptedOcr {
    fn require_ocr_model(
        &self,
        _model_ref: &str,
        _model_sha256: &str,
    ) -> Result<(), VisionProviderError> {
        Ok(())
    }

    fn require_nn_model(
        &self,
        _model_ref: &str,
        _model_sha256: &str,
    ) -> Result<(), VisionProviderError> {
        Err(VisionProviderError::new(
            VisionProviderErrorCode::Unavailable,
            "no NN model in this fixture",
        ))
    }

    fn read_text(
        &self,
        request: OcrProviderRequest<'_>,
    ) -> Result<OcrProviderResult, VisionProviderError> {
        let read = |text: &str, detection| OcrProviderResult {
            ppocr_diagnostics: Vec::new(),
            text: text.to_owned(),
            blocks: vec![OcrProviderTextBlock {
                text: text.to_owned(),
                rect: request.region,
                confidence: Some(0.9),
            }],
            confidence: Some(0.9),
            detection,
        };
        match self.answer {
            Answer::Fails(code) => Err(VisionProviderError::new(code, "scripted provider failure")),
            Answer::CrowdedMiss => Ok(read(
                "OTHER",
                Some(OcrProviderDetection {
                    detected: 65,
                    limit: 64,
                }),
            )),
            Answer::Reads => Ok(read(NAME, None)),
        }
    }

    fn classify(
        &self,
        _request: NnProviderRequest<'_>,
    ) -> Result<NnProviderResult, VisionProviderError> {
        Err(VisionProviderError::new(
            VisionProviderErrorCode::Unavailable,
            "no NN model in this fixture",
        ))
    }
}

// Where the kernel evaluates the name.
#[derive(Debug, Clone, Copy)]
enum Place {
    // The guard of `claim`, a tap on the located name from `home` to `terminal`.
    Guard,
    // The `when` of `open`, a trusted tap from `home` to `terminal`, a candidate while the name
    // is present.
    When,
    // An `expect_identity` that reads the name on a destination frame.
    Identity,
}

const PLACES: [Place; 3] = [Place::Guard, Place::When, Place::Identity];

fn evaluator(answer: Answer) -> RecognitionEvaluator {
    let colour = |id: &str, expected: [u8; 3]| {
        json!({"type": "color", "id": id,
               "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": expected})
    };
    let pack = load_pack_from_json_str(
        &json!({
            "schema_version": "0.7",
            "game": "neutral",
            "server": "test",
            "coordinate_space": {"width": WIDTH, "height": HEIGHT},
            "defaults": {"color_max_distance": 0.0},
            "targets": [
                colour("page/home", HOME),
                colour("page/terminal", TERMINAL),
                {
                    "type": "ocr", "id": TEXT,
                    "region": {"x": 0, "y": 0, "width": WIDTH, "height": HEIGHT},
                    "languages": ["en"], "timeout_ms": 1000, "match_mode": "exact",
                    "expected": [NAME], "case_sensitive": false, "minimum_confidence": 0.5,
                    "model_ref": "PP-OCRv6_medium", "model_sha256": MODEL_SHA256,
                    "locate": {"normalize": "identity_v1", "blocks": "detect",
                               "line_merge": {"gap_factor": 0}}
                }
            ]
        })
        .to_string(),
    )
    .expect("recognition pack");
    RecognitionEvaluator::with_vision_provider(
        pack,
        Arc::new(FsAssetResolver::new(PathBuf::new())),
        Arc::new(ScriptedOcr { answer }),
    )
    .expect("recognition evaluator")
}

fn claim() -> Value {
    json!({"id": "claim", "from": "home", "to": "terminal",
           "click": {"kind": "target", "target_id": TEXT},
           "guard": {"page_id": "home", "target_id": TEXT,
                     "expected_rect": {"x": 0, "y": 0, "width": WIDTH, "height": HEIGHT},
                     "verify_text": TEXT}})
}

fn open() -> Value {
    json!({"id": "open", "from": "home", "to": "terminal",
           "click": {"kind": "point", "x": 1, "y": 0},
           "unguarded_trusted_coordinate": true,
           "when": {"present": TEXT}})
}

// The page-graph task of `place` that ends on `terminal`, its provider answering `answer`, admitted
// as the package's admission admits it.
fn task(answer: Answer, place: Place) -> PreparedContainedTask {
    let control: TaskControl = serde_json::from_value(json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": "neutral.d4b.task",
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": WIDTH, "height": HEIGHT},
        "entry_task_id": "task",
        "step_timeout_ms": 60_000,
        "timeout_ms": 600_000
    }))
    .expect("task control");
    control.validate().expect("an admitted control");
    let declared = match place {
        Place::Guard => claim(),
        Place::When | Place::Identity => open(),
    };
    let program: TaskProgram = serde_json::from_value(json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": WIDTH, "height": HEIGHT},
        "target_page": "terminal",
        "operations": [declared]
    }))
    .expect("task program");
    for operation in &program.operations {
        operation
            .validate(&control, TaskOperationDefaults::default(), "0.9")
            .expect("an admitted operation");
    }
    let evaluator = evaluator(answer);
    program
        .validate_when_targets(TASK_FILE, &evaluator)
        .expect("admitted when targets");
    program
        .validate_text_reads(TASK_FILE, &evaluator)
        .expect("admitted text reads");
    let detector = PageDetector::new(
        serde_json::from_value(json!({
            "schema_version": "0.6",
            "pages": [
                {"id": "neutral/home", "required": ["page/home"]},
                {"id": "neutral/terminal", "required": ["page/terminal"]}
            ]
        }))
        .expect("page set"),
    )
    .expect("page detector");
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

// A frame of `page`: its top-left pixel names the page, every other pixel is dark.
fn pixels(page: [u8; 3]) -> Vec<[u8; 3]> {
    let mut pixels = vec![[0, 0, 0]; (WIDTH * HEIGHT) as usize];
    pixels[0] = page;
    pixels
}

// The device: the home page until the first input, the terminal page after it.
fn device() -> ScriptedRuntime {
    let shown = |page| ScriptedFrame::rgb(WIDTH, HEIGHT, &pixels(page));
    ScriptedRuntime::new(vec![vec![shown(HOME)], vec![shown(TERMINAL)]])
}

// One record of a run, as the kernel hands it to its runtime: a guard evaluation (which one, its
// target, and whether the target passed: an evaluation error did not; none when no target was
// evaluated), or a trace, by its kind, with the values a step trace and the final trace carry (the
// step's index, operation and page, the effect intent's guard outcome, the page a recognition
// found, the run's outcome). Any other trace is kept whole.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Recorded {
    Guard(ContainedTaskGuardEvaluation, Option<String>, Option<bool>),
    PackageAdmitted,
    RunStarted,
    CaptureCompleted,
    RecognitionStarted,
    RecognitionCompleted(Option<String>),
    StepStarted(u32, String, String),
    EffectIntent(u32, String, ContainedTaskGuardOutcome),
    EffectCompleted(u32, String),
    StepFinished(u32, String, String),
    Finalizing(TaskOutcome),
    Other(String),
}

impl Recorded {
    fn of(trace: &ContainedTaskTrace) -> Self {
        match trace {
            ContainedTaskTrace::PackageAdmitted { .. } => Self::PackageAdmitted,
            ContainedTaskTrace::RunStarted => Self::RunStarted,
            ContainedTaskTrace::CaptureCompleted { .. } => Self::CaptureCompleted,
            ContainedTaskTrace::RecognitionStarted { .. } => Self::RecognitionStarted,
            ContainedTaskTrace::RecognitionCompleted { page_label, .. } => {
                Self::RecognitionCompleted(page_label.clone())
            }
            ContainedTaskTrace::StepStarted {
                step_index,
                operation_label,
                from_page,
                ..
            } => Self::StepStarted(*step_index, operation_label.clone(), from_page.clone()),
            ContainedTaskTrace::EffectIntent {
                step_index,
                operation_label,
                guard,
                ..
            } => Self::EffectIntent(*step_index, operation_label.clone(), guard.clone()),
            ContainedTaskTrace::EffectCompleted {
                step_index,
                operation_label,
            } => Self::EffectCompleted(*step_index, operation_label.clone()),
            ContainedTaskTrace::StepFinished {
                step_index,
                operation_label,
                page_label,
                ..
            } => Self::StepFinished(*step_index, operation_label.clone(), page_label.clone()),
            ContainedTaskTrace::Finalizing { outcome } => Self::Finalizing(*outcome),
            other => Self::Other(format!("{other:?}")),
        }
    }
}

// The device of the healthy runs: the scripted device above, which also writes down every record
// the kernel hands it, in order. It answers the kernel as the scripted device does.
struct Recording {
    script: ScriptedRuntime,
    recorded: Vec<Recorded>,
}

impl ContainedTaskRuntime for Recording {
    type Error = &'static str;

    fn capture(&mut self) -> Result<ObservedFrame, Self::Error> {
        self.script.capture()
    }

    fn input(
        &mut self,
        action: InputAction,
        frame: Option<InputFrameContext>,
    ) -> Result<(), Self::Error> {
        self.script.input(action, frame)
    }

    fn record_guard_evaluation(
        &mut self,
        target_id: Option<&str>,
        result: Option<&actingcommand_recognition_pack::RecognitionPackResult<TargetEvaluation>>,
        evaluation: ContainedTaskGuardEvaluation,
    ) -> Result<(), Self::Error> {
        self.recorded.push(Recorded::Guard(
            evaluation,
            target_id.map(str::to_owned),
            result.map(|evaluated| evaluated.as_ref().is_ok_and(|target| target.passed)),
        ));
        Ok(())
    }

    fn record(&mut self, trace: ContainedTaskTrace) -> Result<(), Self::Error> {
        self.recorded.push(Recorded::of(&trace));
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

    fn monotonic_now(&mut self) -> Instant {
        self.script.monotonic_now()
    }

    fn pause(&mut self, duration: Duration) {
        self.script.pause(duration);
    }

    fn is_bookkeeping_failure(error: &Self::Error) -> bool {
        <ScriptedRuntime as ContainedTaskRuntime>::is_bookkeeping_failure(error)
    }
}

// The records of one frame the run captured and recognised as `page`.
fn frame_records(page: &str) -> Vec<Recorded> {
    vec![
        Recorded::CaptureCompleted,
        Recorded::RecognitionStarted,
        Recorded::RecognitionCompleted(Some(page.to_owned())),
    ]
}

// What a healthy run of `operation` records, in order, as before this slice: its start, the home
// page's frame, then `before_step` (the `when` evaluated on that frame), the step's start, `guard`
// (the step's guard evaluation), the effect intent with the guard outcome `outcome`, the effect,
// the terminal page's frame, the step's end there and the run's success.
fn healthy_records(
    operation: &str,
    before_step: Vec<Recorded>,
    guard: Recorded,
    outcome: ContainedTaskGuardOutcome,
) -> Vec<Recorded> {
    [
        vec![Recorded::PackageAdmitted, Recorded::RunStarted],
        frame_records(HOME_PAGE),
        before_step,
        vec![
            Recorded::StepStarted(0, operation.to_owned(), HOME_PAGE.to_owned()),
            guard,
            Recorded::EffectIntent(0, operation.to_owned(), outcome),
            Recorded::EffectCompleted(0, operation.to_owned()),
        ],
        frame_records(TERMINAL_PAGE),
        vec![
            Recorded::StepFinished(0, operation.to_owned(), TERMINAL_PAGE.to_owned()),
            Recorded::Finalizing(TaskOutcome::Success),
        ],
    ]
    .concat()
}

// The scene of a home frame, the destination frame an expected identity reads.
fn home_scene() -> Scene {
    let frame = Frame::from_pixels(
        WIDTH,
        HEIGHT,
        pixels(HOME).concat(),
        PixelFormat::Rgb8,
        CaptureBackendName::FixtureSimulation,
    )
    .expect("fixture frame");
    scene_from_frame(&frame).expect("scene")
}

// An `expect_identity` of the text path, reading the name for the choice of `pick`.
fn expect_identity() -> TaskExpectIdentity {
    serde_json::from_value(json!({"choice_of": "pick", "target_id": TEXT}))
        .expect("expect_identity")
}

// The identity declaration of `pick`'s choice: `member.a` shows the name, `member.b` another text.
fn identity() -> CandidateIdentityDeclaration {
    let identity: CandidateIdentityDeclaration = serde_json::from_value(json!({
        "entries": [
            {"id": "member.a", "aliases": [NAME]},
            {"id": "member.b", "aliases": ["OTHER"]}
        ],
        "recognition": {"kind": "ocr_aliases", "max_distance": 0, "minimum_margin": 1,
                        "minimum_confidence_milli": 500}
    }))
    .expect("identity declaration");
    identity.validate().expect("a valid identity declaration");
    identity
}

// How evaluating the name at `place` ended when the provider answered `answer`: the run's failure,
// before any input, for the guard and the `when`; the failure of the reading of one destination
// frame for the identity.
fn failure(place: Place, answer: Answer) -> ContainedTaskError {
    let task = task(answer, place);
    match place {
        Place::Guard | Place::When => {
            let mut runtime = device();
            let error = match task.run(&mut runtime) {
                Err(ContainedTaskRunError::Task(error)) => error,
                other => panic!("{place:?}, {answer:?}: expected a task failure, got {other:?}"),
            };
            assert!(
                runtime.inputs().is_empty(),
                "{place:?}, {answer:?}: no input"
            );
            error
        }
        Place::Identity => {
            match task.read_expected_identity(&home_scene(), &expect_identity(), &identity()) {
                Err(error) => error,
                Ok(_) => panic!("{place:?}, {answer:?}: expected the reading to fail"),
            }
        }
    }
}

// The error the recognition pack gives for the name on a home frame when the provider answers
// `answer`, evaluated on its own.
fn evaluation_error(answer: Answer) -> RecognitionPackError {
    evaluator(answer)
        .evaluate_target(&home_scene(), TEXT)
        .expect_err("the name's evaluation fails")
}

// The detail `place` writes for `error`, the name's evaluation error, with the code of a read
// that refused to conclude as its `cause=` token at the guard and the `when`, as before.
fn detail(place: Place, error: &RecognitionPackError, cause: Option<&str>) -> String {
    let cause = cause
        .map(|code| format!(" cause={code}"))
        .unwrap_or_default();
    match place {
        Place::Guard => format!("{error}{cause}"),
        Place::When => format!("operation=open when={TEXT} {error}{cause}"),
        Place::Identity => error.to_string(),
    }
}

fn values(error: &ContainedTaskError) -> Value {
    Value::Object(error.outcome().values().clone())
}

// The code a real evaluation error ends with at `place`, as before.
fn evaluation_code(place: Place) -> &'static str {
    match place {
        Place::Guard => SharedCode::ContainedTaskGuardEvaluationFailed.as_str(),
        Place::When | Place::Identity => SharedCode::ContainedTaskRecognitionFailed.as_str(),
    }
}

// A provider that fails, or answers invalidly, ends each of the three places with the class-5
// provider code, the code the burst counts and the task stage's `pack_logic` item does not name,
// with the place's detail and no values, as a page evaluation's failure ends.
#[test]
fn d4b_a_provider_failure_at_a_guard_a_when_or_an_identity_ends_with_the_provider_code() {
    for code in PROVIDER_FAILURES {
        let answer = Answer::Fails(code);
        let cause = evaluation_error(answer);
        for place in PLACES {
            let error = failure(place, answer);
            assert_eq!(
                error.code().as_str(),
                ExecutionKernelCode::ContainedTaskVisionProviderFailed.as_str(),
                "{place:?}, provider {code:?}"
            );
            assert_eq!(
                error.detail(),
                Some(detail(place, &cause, None).as_str()),
                "{place:?}, provider {code:?}"
            );
            assert_eq!(values(&error), json!({}), "{place:?}, provider {code:?}");
        }
    }
    assert_eq!(
        ExecutionKernelCode::ContainedTaskVisionProviderFailed.ladder_class(),
        LadderClass::EnvironmentFault
    );
}

// A real evaluation error, a read that cannot conclude while the provider answered, keeps the
// place's own class-4 code, its detail byte for byte and no values.
#[test]
fn d4b_a_real_evaluation_error_keeps_the_place_s_own_code_and_detail() {
    let cause = evaluation_error(Answer::CrowdedMiss);
    assert_eq!(cause.code(), RecognitionPackErrorCode::TextReadRefused);
    let refused = RecognitionPackCode::TextReadBlocksTruncated.as_str();
    for place in PLACES {
        let error = failure(place, Answer::CrowdedMiss);
        assert_eq!(error.code().as_str(), evaluation_code(place), "{place:?}");
        let cause_token = match place {
            Place::Guard | Place::When => Some(refused),
            Place::Identity => None,
        };
        assert_eq!(
            error.detail(),
            Some(detail(place, &cause, cause_token).as_str()),
            "{place:?}"
        );
        assert_eq!(values(&error), json!({}), "{place:?}");
    }
    for class in [
        SharedCode::ContainedTaskGuardEvaluationFailed.ladder_class(),
        SharedCode::ContainedTaskRecognitionFailed.ladder_class(),
    ] {
        assert_eq!(class, LadderClass::PackLogicMismatch);
    }
}

// A provider that reads the name meets no failure: the guard passes and the `when` holds, each
// run taps once and ends on its target page after its one step, and the identity reads the
// business ID the name maps to. Fix round 2 (the work order's "byte for byte as before"): each run
// records what it recorded before this slice, every record in order. The guard's run: its start,
// the home frame, the step's start, the guard's evaluation of the name (passed), the effect intent
// with the passed guard (the home page, the name, an OCR target), the effect, the terminal frame,
// the step's end on the terminal page and the success. The `when`'s run: the same, with the
// `when`'s evaluation of the name (passed) on the home frame before the step starts, and the
// trusted tap's guard (no target evaluated) in place of the guard's.
#[test]
fn d4b_a_provider_that_reads_leaves_the_three_places_as_they_were() {
    let name = || Some(TEXT.to_owned());
    let guard_run = healthy_records(
        "claim",
        Vec::new(),
        Recorded::Guard(ContainedTaskGuardEvaluation::Evaluated, name(), Some(true)),
        ContainedTaskGuardOutcome::Passed {
            page_label: HOME_PAGE.to_owned(),
            target_id: TEXT.to_owned(),
            target_kind: "ocr".to_owned(),
        },
    );
    let when_run = healthy_records(
        "open",
        vec![Recorded::Guard(
            ContainedTaskGuardEvaluation::When,
            name(),
            Some(true),
        )],
        Recorded::Guard(ContainedTaskGuardEvaluation::TrustedCoordinate, None, None),
        ContainedTaskGuardOutcome::TrustedCoordinate,
    );
    for (place, records) in [(Place::Guard, guard_run), (Place::When, when_run)] {
        let mut runtime = Recording {
            script: device(),
            recorded: Vec::new(),
        };
        let outcome = task(Answer::Reads, place)
            .run(&mut runtime)
            .unwrap_or_else(|error| panic!("{place:?}: expected the run to end, got {error:?}"));
        assert_eq!(
            outcome,
            ContainedTaskOutcome {
                outcome: TaskOutcome::Success,
                final_page: Some(TERMINAL_PAGE.to_owned()),
                executed_steps: 1,
                selected_scheduling_outcome: None,
                designated_effect_caps: Vec::new(),
            },
            "{place:?}"
        );
        assert_eq!(runtime.script.inputs().len(), 1, "{place:?}: one tap");
        assert_eq!(runtime.recorded, records, "{place:?}: the run's records");
    }
    match task(Answer::Reads, Place::Identity).read_expected_identity(
        &home_scene(),
        &expect_identity(),
        &identity(),
    ) {
        Ok(ExpectedIdentityReading::Known(value)) => assert_eq!(value, "member.a"),
        Ok(ExpectedIdentityReading::Unread(_)) => panic!("identity: the name read no identity"),
        Err(error) => panic!("identity: expected a reading, got {error:?}"),
    }
}
