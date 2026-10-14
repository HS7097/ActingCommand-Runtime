// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #396 REC-6 logic tests of the kernel (REC-M v1.2.1 §8.2, §8.4, §12): a page that
// collects OCR may gate on any recognition backend and on its own collected targets, and the
// retired page-gate refusal is gone; the page gate's observation of a field or reading target
// is the one its frame collects, so the provider reads that target once; and a guard check on a
// value test whose values do not hold refuses its input. A scripted OCR provider answers on
// synthetic neutral frames.

use super::*;
use actingcommand_device::CaptureBackendName;
use actingcommand_recognition_pack::{
    FsAssetResolver, NnProviderRequest, NnProviderResult, OcrExecutionProviderKind,
    OcrProviderObservation, OcrProviderRequest, OcrProviderResult, OcrProviderTextBlock,
    VisionProviderError, VisionProviderErrorCode, load_pack_from_json_str,
};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

const MODEL_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const RETIRED: &str = "contained_task_post_admission_ocr_target_in_page_gate";

// Reads the configured text of a region as one block, with complete CPU execution evidence,
// and counts every call.
#[derive(Debug)]
struct CountingOcr {
    texts: Vec<(PackRect, &'static str)>,
    calls: AtomicUsize,
}

impl CountingOcr {
    fn new(texts: Vec<(PackRect, &'static str)>) -> Arc<Self> {
        Arc::new(Self {
            texts,
            calls: AtomicUsize::new(0),
        })
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl VisionProvider for CountingOcr {
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
        self.read_text_with_execution_evidence(request)
            .map(|observation| observation.result)
    }

    fn read_text_with_execution_evidence(
        &self,
        request: OcrProviderRequest<'_>,
    ) -> Result<OcrProviderObservation, VisionProviderError> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        let text = self
            .texts
            .iter()
            .find(|(region, _)| *region == request.region)
            .map_or("", |(_, text)| *text);
        Ok(OcrProviderObservation {
            result: OcrProviderResult {
                ppocr_diagnostics: Vec::new(),
                text: text.to_owned(),
                blocks: vec![OcrProviderTextBlock {
                    text: text.to_owned(),
                    rect: request.region,
                    confidence: Some(0.9),
                }],
                confidence: Some(0.9),
                detection: None,
            },
            execution: Some(OcrProviderExecutionEvidence {
                invocation_id: format!("call-{call}"),
                session_id: "session-1".to_owned(),
                session_generation: 1,
                requested_provider: OcrExecutionProviderKind::Cpu,
                resolved_provider: OcrExecutionProviderKind::Cpu,
                requested_cuda_ordinal: None,
                requested_cuda_identity: None,
                resolved_cuda_ordinal: None,
                resolved_cuda_identity: None,
                provider_implementation: "fixture-ocr".to_owned(),
                provider_binary_sha256: "b".repeat(64),
                runtime_version: "fixture-runtime".to_owned(),
                model_ref: "PP-OCRv6_medium".to_owned(),
                model_sha256: MODEL_SHA256.to_owned(),
                cpu_ep_registered: true,
                cpu_fallback_disabled: false,
                fallback_forbidden: true,
                fallback_observed: None,
                complete: true,
            }),
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

const COUNT_REGION: PackRect = PackRect {
    x: 10,
    y: 0,
    width: 20,
    height: 10,
};
const STOCK_REGION: PackRect = PackRect {
    x: 40,
    y: 0,
    width: 20,
    height: 10,
};

fn ocr(id: &str, region: PackRect, expected: &str) -> Value {
    json!({
        "type": "ocr", "id": id,
        "region": {"x": region.x, "y": region.y, "width": region.width, "height": region.height},
        "languages": ["en"], "timeout_ms": 1000, "match_mode": "exact", "expected": [expected],
        "case_sensitive": false, "minimum_confidence": 0.5, "model_ref": "PP-OCRv6_medium",
        "model_sha256": MODEL_SHA256
    })
}

// A red pixel at (0, 0) marks the page `operator`, a blue one the page `terminal`.
fn marked_frame(marker: [u8; 3]) -> Frame {
    let mut pixels = vec![0_u8; 64 * 16 * 3];
    pixels[0..3].copy_from_slice(&marker);
    Frame::from_pixels(
        64,
        16,
        pixels,
        PixelFormat::Rgb8,
        CaptureBackendName::FixtureSimulation,
    )
    .expect("fixture frame")
}

fn evaluator(targets: Vec<Value>, provider: Arc<CountingOcr>) -> RecognitionEvaluator {
    let mut all = vec![
        json!({"type": "color", "id": "page/operator",
               "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": [255, 0, 0]}),
        json!({"type": "color", "id": "page/terminal",
               "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": [0, 0, 255]}),
    ];
    all.extend(targets);
    let pack = load_pack_from_json_str(
        &json!({
            "schema_version": "0.7",
            "game": "neutral",
            "server": "test",
            "coordinate_space": {"width": 64, "height": 16},
            "defaults": {"color_max_distance": 0.0},
            "targets": all
        })
        .to_string(),
    )
    .expect("recognition pack");
    RecognitionEvaluator::with_vision_provider(
        pack,
        Arc::new(FsAssetResolver::new(PathBuf::new())),
        provider,
    )
    .expect("recognition evaluator")
}

fn page_set(pages: Value) -> PageSet {
    serde_json::from_value(json!({"schema_version": "0.6", "pages": pages})).expect("page set")
}

fn control(schema_version: &str) -> TaskControl {
    serde_json::from_value(json!({
        "schema_version": schema_version,
        "package_id": "neutral.values.task",
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": 64, "height": 16},
        "entry_task_id": "task",
        "capture_interval_ms": 1,
        "timeout_ms": 1000
    }))
    .expect("task control")
}

// A device that always shows one frame and records every trace and input.
struct ShowRuntime {
    frame: Frame,
    traces: Vec<ContainedTaskTrace>,
    inputs: Vec<InputAction>,
}

impl ShowRuntime {
    fn new(frame: Frame) -> Self {
        Self {
            frame,
            traces: Vec::new(),
            inputs: Vec::new(),
        }
    }
}

impl ContainedTaskRuntime for ShowRuntime {
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

    fn record(&mut self, trace: ContainedTaskTrace) -> Result<(), Self::Error> {
        self.traces.push(trace);
        Ok(())
    }
}

// A zero-input fields task (operation schema 0.8) whose page `operator` is its target, fields
// and terminal page; `pages` gate it and `readings` are read on it.
fn fields_task(
    pages: Value,
    targets: Vec<Value>,
    readings: Option<Value>,
    provider: Arc<CountingOcr>,
) -> PreparedContainedTask {
    let declaration = json!({
        "mode": "fields_v1", "page_ids": ["operator"],
        "fields": [{"id": "count", "group": "snapshot", "target_id": "text/count", "required": true,
                    "privacy": "public", "trim": "whitespace_v1",
                    "value": {"type": "unsigned_integer", "min": 0, "max": 100}}],
        "limits": {"max_frames": 1, "max_items": 8, "max_string_bytes": 64,
                   "max_total_bytes": 4096, "max_truth_entries": 8},
        "outcome_key": "fields_recorded"
    });
    let mut program = json!({
        "schema_version": "0.8", "task_id": "task", "game": "neutral", "server_scope": ["test"],
        "coordinate_space": {"width": 64, "height": 16}, "target_page": "operator",
        "timeout_ms": 1000, "post_admission_ocr": declaration,
        "scheduling_outcome": {"mappings": [{"outcome_key": "fields_recorded",
            "effect": "no_designated_effect", "terminal_pages": ["operator"]}]},
        "operations": []
    });
    if let Some(readings) = readings {
        program["resource_readings"] = readings;
    }
    let program: TaskProgram = serde_json::from_value(program).expect("task program");
    let fields = PreparedOcrFields {
        declaration: serde_json::from_value(declaration).expect("fields declaration"),
        dictionaries: BTreeMap::new(),
    };
    fields.declaration.validate().expect("fields declaration");
    PreparedContainedTask {
        control: control(CONTROL_SCHEMA),
        scheduling_outcome: program.scheduling_outcome.clone(),
        program,
        evaluator: evaluator(targets, provider),
        detector: PageDetector::new(page_set(pages)).expect("page detector"),
        entry_page: None,
        post_admission_ocr: None,
        post_admission_fields: Some(fields),
        package_sha256: "fixture".into(),
        entry_count: 1,
        task_count: 1,
    }
}

fn fields_report(runtime: &ShowRuntime) -> &OcrFieldsReport {
    runtime
        .traces
        .iter()
        .find_map(|trace| match trace {
            ContainedTaskTrace::PostAdmissionOcrFields { report } => Some(report),
            _ => None,
        })
        .expect("the fields report")
}

#[test]
fn a_page_that_collects_ocr_gates_on_any_backend_and_the_page_gate_refusal_is_retired() {
    let control = control(CONTROL_SCHEMA);
    let pages = page_set(json!([
        {"id": "neutral/operator", "required": ["page/operator", "text/count"],
         "any_of": [["text/locate", "check/values"]], "optional": ["text/stock"]},
        {"id": "neutral/terminal", "required": ["page/terminal"]}
    ]));

    validate_post_admission_ocr_page_set(&control, &pages, &["operator".to_string()])
        .expect("an OCR, located text and value test gate on the collecting page is admitted");

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
    assert_eq!(fragment["codes"][RETIRED]["status"], "retired");
}

#[test]
fn a_page_gate_and_the_fields_of_its_frame_share_one_provider_observation() {
    let provider = CountingOcr::new(vec![(COUNT_REGION, "42")]);
    let task = fields_task(
        json!([{"id": "neutral/operator", "required": ["page/operator", "text/count"]}]),
        vec![ocr("text/count", COUNT_REGION, "42")],
        None,
        provider.clone(),
    );
    let mut runtime = ShowRuntime::new(marked_frame([255, 0, 0]));

    let outcome = task.run(&mut runtime).expect("the fields task succeeds");

    assert_eq!(outcome.outcome, TaskOutcome::Success);
    assert_eq!(provider.calls(), 1, "the gate's observation is the field's");
    let field = &fields_report(&runtime).records[0].fields[0];
    assert_eq!(field.value, Some(OcrFieldValue::UnsignedInteger(42)));
    assert_eq!(field.raw_text.as_deref(), Some("42"));
    assert!(runtime.inputs.is_empty());
}

#[test]
fn a_page_gate_and_the_readings_of_its_terminal_frame_share_one_provider_observation() {
    let provider = CountingOcr::new(vec![(COUNT_REGION, "42"), (STOCK_REGION, "17")]);
    let task = fields_task(
        json!([{"id": "neutral/operator",
                "required": ["page/operator", "text/count", "text/stock"]}]),
        vec![
            ocr("text/count", COUNT_REGION, "42"),
            ocr("text/stock", STOCK_REGION, "17"),
        ],
        Some(json!([{
            "id": "stock", "fact_key": "resource.sample_stock", "page_id": "operator",
            "target_id": "text/stock", "trim": "whitespace_v1",
            "value": {"type": "unsigned_integer", "min": 0, "max": 1000},
            "minimum_confidence_milli": 500, "valid_for_ms": 60000
        }])),
        provider.clone(),
    );
    let mut runtime = ShowRuntime::new(marked_frame([255, 0, 0]));

    task.run(&mut runtime).expect("the fields task succeeds");

    assert_eq!(
        provider.calls(),
        2,
        "the gate reads each target once; the field and the reading take its observations"
    );
    let readings = runtime
        .traces
        .iter()
        .find_map(|trace| match trace {
            ContainedTaskTrace::ResourceReadings { readings, .. } => Some(readings),
            _ => None,
        })
        .expect("the readings");
    assert_eq!(readings.len(), 1);
    assert_eq!(readings[0].value, 17);
    assert_eq!(
        fields_report(&runtime).records[0].fields[0].value,
        Some(OcrFieldValue::UnsignedInteger(42))
    );
}

#[test]
fn a_guard_check_on_a_value_test_that_does_not_hold_refuses_the_input() {
    // The quantity 3 is checked against the cost 3 * 2 + 0: it does not hold.
    let provider = CountingOcr::new(vec![(COUNT_REGION, "3"), (STOCK_REGION, "3")]);
    let evaluator = evaluator(
        vec![
            ocr("text/count", COUNT_REGION, "unused"),
            ocr("text/stock", STOCK_REGION, "unused"),
            json!({
                "type": "value_test", "id": "check/quantity",
                "left": {"target_id": "text/count", "trim": "whitespace_v1",
                         "value": {"type": "unsigned_integer", "min": 0, "max": 99}},
                "op": "eq",
                "right": {"target_id": "text/stock", "trim": "whitespace_v1",
                          "value": {"type": "unsigned_integer", "min": 0, "max": 99},
                          "mul": 2, "add": 0}
            }),
        ],
        provider.clone(),
    );
    let control = control(PHASED_CONTROL_SCHEMA);
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
        operations: vec![
            serde_json::from_value(json!({
                "id": "confirm", "from": "operator", "to": "terminal",
                "click": {"kind": "rect", "x": 50, "y": 4, "width": 8, "height": 8},
                "guard": {"page_id": "operator", "target_id": "check/quantity",
                          "expected_rect": {"x": 50, "y": 4, "width": 8, "height": 8},
                          "check": "check/quantity"}
            }))
            .expect("task operation"),
        ],
    };
    let task = PreparedContainedTask {
        control,
        program,
        evaluator,
        detector: PageDetector::new(page_set(json!([
            {"id": "neutral/operator", "required": ["page/operator"]},
            {"id": "neutral/terminal", "required": ["page/terminal"]}
        ])))
        .expect("page detector"),
        entry_page: None,
        scheduling_outcome: None,
        post_admission_ocr: None,
        package_sha256: "fixture".into(),
        post_admission_fields: None,
        entry_count: 3,
        task_count: 1,
    };
    let mut runtime = ShowRuntime::new(marked_frame([255, 0, 0]));

    let Err(ContainedTaskRunError::Task(error)) = task.run(&mut runtime) else {
        panic!("the value test refuses the input");
    };

    assert_eq!(error.code().as_str(), "contained_task_guard_refused");
    assert_eq!(
        error.detail(),
        Some("operation=confirm target=check/quantity cause=value_test_not_met")
    );
    assert!(runtime.inputs.is_empty(), "zero inputs");
    assert_eq!(provider.calls(), 2);
}
