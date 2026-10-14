// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #396 REC-1 logic tests of the kernel: a guard on a locate OCR target, the click box
//! it gives, its refusal, and the admission rules of text guards, fields and readings. A scripted
//! OCR provider answers on synthetic neutral frames.

use super::*;
use actingcommand_device::CaptureBackendName;
use actingcommand_recognition_pack::{
    FsAssetResolver, NnProviderRequest, NnProviderResult, OcrProviderRequest, OcrProviderResult,
    OcrProviderTextBlock, VisionProviderError, VisionProviderErrorCode, load_pack_from_json_str,
};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

const MODEL_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const OPERATION_PATH: &str = "operations/task/task.json";

/// Reads the same blocks for every request.
#[derive(Debug)]
struct FixedOcr {
    blocks: Vec<(String, PackRect)>,
    calls: AtomicUsize,
}

impl FixedOcr {
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl VisionProvider for FixedOcr {
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
        _request: OcrProviderRequest<'_>,
    ) -> Result<OcrProviderResult, VisionProviderError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(OcrProviderResult {
            ppocr_diagnostics: Vec::new(),
            text: self
                .blocks
                .iter()
                .map(|(text, _)| text.as_str())
                .collect::<Vec<_>>()
                .join("\n"),
            blocks: self
                .blocks
                .iter()
                .map(|(text, rect)| OcrProviderTextBlock {
                    text: text.clone(),
                    rect: *rect,
                    confidence: Some(0.875),
                })
                .collect(),
            confidence: Some(0.875),
            detection: None,
        })
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

fn rect(x: i32, y: i32, width: i32, height: i32) -> PackRect {
    PackRect {
        x,
        y,
        width,
        height,
    }
}

/// GAMMA in the first cell, ALPHA in cell 5 (row 1, column 1) of the list's 4 by 2 grid.
fn list_blocks() -> Vec<(&'static str, PackRect)> {
    vec![
        ("GAMMA", rect(60, 80, 60, 20)),
        ("ALPHA", rect(190, 240, 80, 20)),
    ]
}

fn ocr(id: &str, region: PackRect, locate: Option<Value>) -> Value {
    let mut target = json!({
        "type": "ocr", "id": id,
        "region": {"x": region.x, "y": region.y, "width": region.width, "height": region.height},
        "languages": ["en"], "timeout_ms": 1000, "match_mode": "exact", "expected": ["ALPHA"],
        "case_sensitive": false, "minimum_confidence": 0.8,
        "model_ref": "PP-OCRv6_medium", "model_sha256": MODEL_SHA256
    });
    if let Some(locate) = locate {
        target["locate"] = locate;
    }
    target
}

fn detect() -> Value {
    json!({"normalize": "identity_v1", "blocks": "detect", "line_merge": {"gap_factor": 0}})
}

fn detect_in_cells() -> Value {
    let mut locate = detect();
    locate["cells"] = json!({"x": 0, "y": 0, "width": 120, "height": 120, "pitch_x": 140,
                             "pitch_y": 140, "columns": 4, "rows": 2});
    locate
}

fn control() -> TaskControl {
    serde_json::from_value(json!({
        "schema_version": "Lab-1y.control.v1",
        "package_id": "neutral.semantic.task",
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": 640, "height": 360},
        "entry_task_id": "task",
        "capture_interval_ms": 1,
        "step_timeout_ms": 5000
    }))
    .expect("task control")
}

/// One operation from the home page to the terminal page, guarded on `guarded` by
/// `verify_text`, with the given click.
fn operation(click: Value, guarded: &str) -> Value {
    json!({
        "id": "open_detail",
        "from": "home",
        "to": "terminal",
        "click": click,
        "guard": {
            "page_id": "home",
            "target_id": guarded,
            "expected_rect": {"x": 40, "y": 60, "width": 560, "height": 280},
            "verify_text": guarded
        }
    })
}

fn task(
    operation: Value,
    blocks: Vec<(&str, PackRect)>,
    extra_targets: Vec<Value>,
) -> (PreparedContainedTask, Arc<FixedOcr>) {
    let list = rect(40, 60, 560, 280);
    let mut targets = vec![
        json!({"type": "color", "id": "page/home", "region": {"x": 0, "y": 0, "width": 1, "height": 1},
               "expected": [255, 0, 0]}),
        json!({"type": "color", "id": "page/terminal", "region": {"x": 0, "y": 0, "width": 1, "height": 1},
               "expected": [0, 0, 255]}),
        ocr("text/entry", list, Some(detect())),
        ocr("text/cell", list, Some(detect_in_cells())),
        ocr("text/plain", list, None),
    ];
    targets.extend(extra_targets);
    let pack = load_pack_from_json_str(
        &json!({
            "schema_version": "0.7",
            "game": "neutral",
            "server": "test",
            "coordinate_space": {"width": 640, "height": 360},
            "defaults": {"color_max_distance": 0.0},
            "targets": targets
        })
        .to_string(),
    )
    .expect("recognition pack");
    let provider = Arc::new(FixedOcr {
        blocks: blocks
            .into_iter()
            .map(|(text, rect)| (text.to_owned(), rect))
            .collect(),
        calls: AtomicUsize::new(0),
    });
    let evaluator = RecognitionEvaluator::with_vision_provider(
        pack,
        Arc::new(FsAssetResolver::new(PathBuf::new())),
        provider.clone(),
    )
    .expect("recognition evaluator");
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
    let control = control();
    let program = TaskProgram {
        schema_version: "0.9".to_string(),
        task_id: "task".to_string(),
        game: "neutral".to_string(),
        server_scope: vec!["test".to_string()],
        coordinate_space: control.resolution,
        entry_page: None,
        phases: None,
        timeout_ms: None,
        max_steps: None,
        target_page: Some(PageDeclaration::Singleton("terminal".to_string())),
        error_pages: Vec::new(),
        scheduling_outcome: None,
        post_admission_ocr: None,
        resource_readings: None,
        stability_termination: None,
        recovery: None,
        defaults: TaskOperationDefaults::default(),
        operations: vec![serde_json::from_value(operation).expect("task operation")],
    };
    (
        PreparedContainedTask {
            control,
            program,
            evaluator,
            detector,
            entry_page: None,
            scheduling_outcome: None,
            post_admission_ocr: None,
            package_sha256: "fixture-sha256".into(),
            post_admission_fields: None,
            entry_count: 5,
            task_count: 1,
        },
        provider,
    )
}

fn home_frame() -> Frame {
    let mut pixels = vec![0_u8; 640 * 360 * 3];
    pixels[0..3].copy_from_slice(&[255, 0, 0]);
    Frame::from_pixels(
        640,
        360,
        pixels,
        PixelFormat::Rgb8,
        CaptureBackendName::FixtureSimulation,
    )
    .expect("home frame")
}

fn home_observation() -> PageObservation {
    PageObservation {
        sample_scenes: Vec::new(),
        remembered_ocr: Default::default(),
        sampling: None,
        input_context: None,
        page_label: "neutral/home".to_owned(),
        scene: scene_from_frame(&home_frame()).expect("scene"),
        stability_sample: None,
        captured_at: SystemTime::UNIX_EPOCH,
        frame_started: Instant::now(),
    }
}

/// A device that always shows the home page and records every input.
struct TapRuntime {
    frame: Frame,
    inputs: Vec<InputAction>,
}

impl TapRuntime {
    fn new() -> Self {
        Self {
            frame: home_frame(),
            inputs: Vec::new(),
        }
    }
}

impl ContainedTaskRuntime for TapRuntime {
    type Error = &'static str;

    fn capture(&mut self) -> Result<ObservedFrame, Self::Error> {
        Ok(self
            .frame
            .try_clone()
            .expect("copy the fixture frame")
            .into())
    }

    fn input(
        &mut self,
        action: InputAction,
        _frame: Option<InputFrameContext>,
    ) -> Result<(), Self::Error> {
        self.inputs.push(action);
        Ok(())
    }

    fn record(&mut self, _trace: ContainedTaskTrace) -> Result<(), Self::Error> {
        Ok(())
    }
}

fn click(value: Value) -> TaskClick {
    serde_json::from_value(value).expect("task click")
}

fn guarded_target(task: &PreparedContainedTask) -> TargetEvaluation {
    let mut runtime = TapRuntime::new();
    let (outcome, target) = task.program.operations[0]
        .guard_outcome(
            &task.control,
            &home_observation(),
            &task.evaluator,
            &mut runtime,
            None,
        )
        .expect("the text guard passes");
    assert!(matches!(outcome, ContainedTaskGuardOutcome::Passed { .. }));
    target.expect("the guard target's evaluation")
}

fn region(x: i32, y: i32, width: i32, height: i32) -> InputSamplingRegion {
    InputSamplingRegion::new(x, y, width, height).expect("sampling region")
}

/// The tap and the one region it was sampled from.
fn tap(
    task: &PreparedContainedTask,
    target: &TargetEvaluation,
    value: Value,
) -> ((i32, i32), Vec<InputSamplingRegion>) {
    let (action, sampling) = click(value)
        .input_action(&task.control.resolution, Some(target), Some(7))
        .expect("a sampled tap");
    let InputAction::Tap { x, y } = action else {
        panic!("a tap, got {action:?}");
    };
    (
        (x, y),
        sampling
            .expect("sampling evidence")
            .source_regions()
            .to_vec(),
    )
}

#[test]
fn a_found_text_gives_target_and_offset_click_boxes() {
    let (task, provider) = task(
        operation(
            json!({"kind": "target", "target_id": "text/entry"}),
            "text/entry",
        ),
        list_blocks(),
        Vec::new(),
    );
    let target = guarded_target(&task);
    assert_eq!(provider.calls(), 1);

    let ((x, y), regions) = tap(
        &task,
        &target,
        json!({"kind": "target", "target_id": "text/entry"}),
    );
    assert_eq!(
        regions,
        [region(190, 240, 80, 20)],
        "the block is the click box"
    );
    assert!((190..270).contains(&x) && (240..260).contains(&y));

    let ((x, y), regions) = tap(
        &task,
        &target,
        json!({"kind": "offset", "target_id": "text/entry",
               "offset": {"x": -8, "y": -6, "width": 120, "height": 36}}),
    );
    assert_eq!(
        regions,
        [region(182, 234, 120, 36)],
        "offset from the block's corner"
    );
    assert!((182..302).contains(&x) && (234..270).contains(&y));
}

#[test]
fn declared_cells_make_the_cell_of_the_found_text_the_click_box() {
    let (task, _) = task(
        operation(
            json!({"kind": "target", "target_id": "text/cell"}),
            "text/cell",
        ),
        list_blocks(),
        Vec::new(),
    );
    let target = guarded_target(&task);

    let (_, regions) = tap(
        &task,
        &target,
        json!({"kind": "target", "target_id": "text/cell"}),
    );
    assert_eq!(
        regions,
        [region(180, 200, 120, 120)],
        "cell 5 is the click box"
    );

    let (_, regions) = tap(
        &task,
        &target,
        json!({"kind": "offset", "target_id": "text/cell",
               "offset": {"x": 10, "y": 20, "width": 50, "height": 40}}),
    );
    assert_eq!(
        regions,
        [region(190, 220, 50, 40)],
        "offset from the cell's corner"
    );
}

#[test]
fn a_missing_text_refuses_the_guard_under_its_code_and_taps_nothing() {
    let (task, provider) = task(
        operation(
            json!({"kind": "target", "target_id": "text/entry"}),
            "text/entry",
        ),
        vec![
            ("GAMMA", rect(60, 80, 60, 20)),
            ("DELTA", rect(190, 240, 80, 20)),
        ],
        Vec::new(),
    );
    let mut runtime = TapRuntime::new();

    let Err(ContainedTaskRunError::Task(error)) = task.run(&mut runtime) else {
        panic!("the text guard refuses the input");
    };

    assert_eq!(error.code().as_str(), "contained_task_guard_refused");
    assert_eq!(
        error.detail(),
        Some("operation=open_detail target=text/entry cause=text_locate_not_found")
    );
    assert!(runtime.inputs.is_empty(), "zero inputs");
    assert_eq!(provider.calls(), 1);
}

#[test]
fn a_click_box_outside_the_frame_is_refused() {
    let (task, _) = task(
        operation(
            json!({"kind": "target", "target_id": "text/entry"}),
            "text/entry",
        ),
        list_blocks(),
        Vec::new(),
    );
    let target = guarded_target(&task);

    let error = click(json!({"kind": "offset", "target_id": "text/entry",
                             "offset": {"x": 400, "y": 0, "width": 100, "height": 10}}))
    .input_action(&task.control.resolution, Some(&target), Some(7))
    .expect_err("the box leaves the frame");

    assert_eq!(error.code().as_str(), "contained_task_input_out_of_bounds");
}

#[test]
fn text_guards_are_admitted_only_with_a_target_or_offset_click_on_their_own_locate_target() {
    let control = control();
    let validate = |value: Value| {
        serde_json::from_value::<TaskOperation>(value)
            .expect("task operation")
            .validate(&control, TaskOperationDefaults::default(), "0.9")
    };
    validate(operation(
        json!({"kind": "target", "target_id": "text/entry"}),
        "text/entry",
    ))
    .expect("a target click on a text guard");
    validate(operation(
        json!({"kind": "offset", "target_id": "text/entry",
               "offset": {"x": 0, "y": 0, "width": 10, "height": 10}}),
        "text/entry",
    ))
    .expect("an offset click on a text guard");

    let error = validate(operation(
        json!({"kind": "target_center", "target_id": "text/entry"}),
        "text/entry",
    ))
    .expect_err("a text target never takes target_center");
    assert_eq!(error.code().as_str(), "contained_task_operation_invalid");

    let mut other = operation(
        json!({"kind": "target", "target_id": "text/entry"}),
        "text/entry",
    );
    other["guard"]["verify_text"] = json!("text/cell");
    let error = validate(other).expect_err("verify_text names the guard target");
    assert_eq!(error.code().as_str(), "contained_task_guard_invalid");

    let (task, _) = task(
        operation(
            json!({"kind": "target", "target_id": "text/plain"}),
            "text/plain",
        ),
        list_blocks(),
        Vec::new(),
    );
    let error = task
        .program
        .validate_text_reads(OPERATION_PATH, &task.evaluator)
        .expect_err("verify_text names a locate OCR target");
    assert_eq!(error.code().as_str(), "contained_task_guard_invalid");
}

#[test]
fn a_guard_reading_beyond_the_budget_is_a_refused_declaration_at_the_guard() {
    let reads = |count: usize| {
        let mut targets = (0..count)
            .map(|index| {
                ocr(
                    &format!("text/read{index}"),
                    rect(40 + 100 * index as i32, 0, 80, 40),
                    Some(detect()),
                )
            })
            .collect::<Vec<_>>();
        targets.push(json!({
            "type": "composite", "id": "check/reads", "mode": "any_of",
            "members": (0..count).map(|index| format!("text/read{index}")).collect::<Vec<_>>()
        }));
        targets
    };
    let checked = || {
        json!({
            "id": "open_detail", "from": "home", "to": "terminal",
            "click": {"kind": "rect", "x": 40, "y": 60, "width": 20, "height": 20},
            "guard": {"page_id": "home", "target_id": "check/reads",
                      "expected_rect": {"x": 40, "y": 60, "width": 20, "height": 20},
                      "check": "check/reads"}
        })
    };

    let (within, _) = task(checked(), list_blocks(), reads(4));
    within
        .program
        .validate_text_reads(OPERATION_PATH, &within.evaluator)
        .expect("four distinct reads fit one guard");

    let (beyond, _) = task(checked(), list_blocks(), reads(5));
    let error = beyond
        .program
        .validate_text_reads(OPERATION_PATH, &beyond.evaluator)
        .expect_err("five distinct reads exceed one guard");
    assert_eq!(error.code().as_str(), "resource_declaration_invalid");
    let issue = error.declaration_issue().expect("the refused declaration");
    assert_eq!(issue.declaration_file, OPERATION_PATH);
    assert_eq!(issue.field_path, "/operations/0/guard");
    assert_eq!(
        issue.reason,
        actingcommand_contract::ResourceDeclarationReason::InvalidValue
    );
}

#[test]
fn a_locate_target_is_never_a_field_or_reading_target() {
    let (task, _) = task(
        operation(
            json!({"kind": "target", "target_id": "text/entry"}),
            "text/entry",
        ),
        list_blocks(),
        Vec::new(),
    );

    let error = validate_post_admission_ocr_target(&task.control, &task.evaluator, "text/entry")
        .expect_err("a field never locates text");
    assert_eq!(
        error.code().as_str(),
        "contained_task_post_admission_ocr_target_invalid"
    );
    validate_post_admission_ocr_target(&task.control, &task.evaluator, "text/plain")
        .expect("a plain OCR target is a field target");

    assert_eq!(
        reading_target_refusal(&task.evaluator, "text/entry"),
        Some("target_locates_text")
    );
    assert_eq!(reading_target_refusal(&task.evaluator, "text/plain"), None);
}

/// Workflow #373 NAV-6, test row N6-14 (model M-373 v1.1 §6.1, REC-M §3.3): two shared interrupt
/// pages whose page rules read the same located text request, evaluated as an interrupt check
/// evaluates its feature package (one scene evaluation of the offered frame): both match, and
/// the provider is called once.
#[test]
fn n6_14_two_interrupt_pages_on_one_located_read_call_the_provider_once() {
    let list = rect(40, 60, 560, 280);
    let pack = load_pack_from_json_str(
        &json!({
            "schema_version": "0.7",
            "game": "neutral",
            "server": "test",
            "coordinate_space": {"width": 640, "height": 360},
            "defaults": {"color_max_distance": 0.0},
            "targets": [
                json!({"type": "color", "id": "page/home", "region": {"x": 0, "y": 0, "width": 1, "height": 1},
                       "expected": [255, 0, 0]}),
                ocr("text/notice", list, Some(detect())),
                ocr("text/alert", list, Some(detect())),
            ]
        })
        .to_string(),
    )
    .expect("recognition pack");
    let provider = Arc::new(FixedOcr {
        blocks: list_blocks()
            .into_iter()
            .map(|(text, rect)| (text.to_owned(), rect))
            .collect(),
        calls: AtomicUsize::new(0),
    });
    let evaluator = RecognitionEvaluator::with_vision_provider(
        pack,
        Arc::new(FsAssetResolver::new(PathBuf::new())),
        provider.clone(),
    )
    .expect("recognition evaluator");
    let detector = PageDetector::new(
        serde_json::from_value(json!({
            "schema_version": "0.6",
            "pages": [
                {"id": "neutral/home", "required": ["page/home"]},
                {"id": "neutral/notice", "required": ["text/notice"]},
                {"id": "neutral/alert", "required": ["text/alert"]}
            ]
        }))
        .expect("page set"),
    )
    .expect("page detector");
    let control = control();
    let program = TaskProgram {
        schema_version: "0.9".to_string(),
        task_id: "task".to_string(),
        game: "neutral".to_string(),
        server_scope: vec!["test".to_string()],
        coordinate_space: control.resolution,
        entry_page: None,
        phases: None,
        timeout_ms: None,
        max_steps: None,
        target_page: Some(PageDeclaration::Singleton("home".to_string())),
        error_pages: Vec::new(),
        scheduling_outcome: None,
        post_admission_ocr: None,
        resource_readings: None,
        stability_termination: None,
        recovery: None,
        defaults: TaskOperationDefaults::default(),
        operations: Vec::new(),
    };
    let features = PreparedContainedTask {
        control,
        program,
        evaluator,
        detector,
        entry_page: None,
        scheduling_outcome: None,
        post_admission_ocr: None,
        package_sha256: "fixture-sha256".into(),
        post_admission_fields: None,
        entry_count: 5,
        task_count: 1,
    };
    let scene = scene_from_frame(&home_frame()).expect("scene");

    let matched = features
        .matching_pages_on_scene(&scene, &["neutral/notice", "neutral/alert"])
        .expect("one scene evaluation");

    assert_eq!(matched, ["neutral/notice", "neutral/alert"]);
    assert_eq!(provider.calls(), 1);
}
