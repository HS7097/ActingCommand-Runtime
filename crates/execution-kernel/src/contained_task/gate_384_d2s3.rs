// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #384 D2-S3 (`rd-1009/d2/model-d2-fact-refresh.md` v1.4.2 §3.6, §8, the kernel halves of
// rows T15, T17 and T40): a package's `resource_adjustments` declaration and how a run confirms
// it. An amount is never a constant and an adjustment read after its operation needs a mark of
// its key; an operation that declares adjustments is confirmed on the frame its step finished
// on, an amount is read on that frame, and nothing is confirmed for an operation whose effect
// was not confirmed.
//
// The device shows one page at a time: a frame's first pixel is the page's colour, its second
// pixel the code of the balance the page shows, its third and fourth pixels the codes of the
// changes it shows. A tap moves the device to the page, balance and changes its tap table names.
// Captures are stamped one second apart from the Unix epoch. The OCR provider reads the text of
// the code in the pixel its region starts on. Every expected value comes from the model rows
// named on each test.

use super::*;
use actingcommand_contract::{PackageRef, ResourceDeclarationReason};
use actingcommand_device::CaptureBackendName;
use actingcommand_recognition_pack::{
    FsAssetResolver, NnProviderRequest, NnProviderResult, OcrExecutionProviderKind,
    OcrProviderObservation, OcrProviderRequest, OcrProviderResult, OcrProviderTextBlock,
    VisionProviderError, VisionProviderErrorCode, load_pack_from_json_str,
};
use serde_json::{Value, json};
use std::io::{Cursor, Write};
use std::path::PathBuf;
use std::time::UNIX_EPOCH;
use zip::{ZipWriter, write::FileOptions};

const WIDTH: u32 = 8;
const MODEL_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const PAGES: [(&str, [u8; 3]); 3] = [
    ("home", [255, 0, 0]),
    ("shop", [0, 255, 0]),
    ("terminal", [0, 0, 255]),
];
// The text of each code a pixel can show.
const TEXTS: [&str; 6] = ["", "100", "83", "+120", "20", "35"];

fn colour(page: &str) -> [u8; 3] {
    PAGES
        .iter()
        .find(|(name, _)| *name == page)
        .map(|(_, colour)| *colour)
        .unwrap_or_else(|| panic!("unknown fixture page {page}"))
}

// Reads the text of the code in the pixel its region starts on, at confidence 0.99.
#[derive(Debug, Default)]
struct ShownCodes;

impl VisionProvider for ShownCodes {
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
        Err(VisionProviderError::new(
            VisionProviderErrorCode::Unavailable,
            "readings request execution evidence",
        ))
    }

    fn read_text_with_execution_evidence(
        &self,
        request: OcrProviderRequest<'_>,
    ) -> Result<OcrProviderObservation, VisionProviderError> {
        let pixel = usize::try_from(request.region.x).expect("a region on the frame") * 3;
        let text = TEXTS[usize::from(request.frame.rgb8_pixels[pixel])].to_owned();
        Ok(OcrProviderObservation {
            result: OcrProviderResult {
                ppocr_diagnostics: Vec::new(),
                text: text.clone(),
                blocks: vec![OcrProviderTextBlock {
                    text,
                    rect: request.region,
                    confidence: Some(0.99),
                }],
                confidence: Some(0.99),
                detection: None,
            },
            execution: Some(OcrProviderExecutionEvidence {
                invocation_id: "adjustment-ocr".to_owned(),
                session_id: "adjustment-session".to_owned(),
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
                model_ref: request.model_ref.to_owned(),
                model_sha256: request.model_sha256.to_owned(),
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

// What the device shows: its page and the codes of its balance and of its two changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Screen {
    page: &'static str,
    balance: u8,
    gain: u8,
    spend: u8,
}

const fn screen(page: &'static str, balance: u8, gain: u8, spend: u8) -> Screen {
    Screen {
        page,
        balance,
        gain,
        spend,
    }
}

// The device and the run's records.
struct Device {
    screen: Screen,
    taps: BTreeMap<i32, Screen>,
    captures: u64,
    shown: Vec<(SystemTime, &'static str)>,
    adjustment_evaluations: Vec<(SystemTime, String)>,
    traces: Vec<ContainedTaskTrace>,
}

impl Device {
    fn new(start: Screen, taps: &[(i32, Screen)]) -> Self {
        Self {
            screen: start,
            taps: taps.iter().copied().collect(),
            captures: 0,
            shown: Vec::new(),
            adjustment_evaluations: Vec::new(),
            traces: Vec::new(),
        }
    }

    // The capture times of the frames that showed `page`, in capture order.
    fn frames_of(&self, page: &str) -> Vec<SystemTime> {
        self.shown
            .iter()
            .filter(|(_, shown)| *shown == page)
            .map(|(at, _)| *at)
            .collect()
    }

    fn position(&self, wanted: impl Fn(&ContainedTaskTrace) -> bool) -> Option<usize> {
        self.traces.iter().position(wanted)
    }

    fn confirmations(&self) -> Vec<(SystemTime, String)> {
        self.traces
            .iter()
            .filter_map(|trace| match trace {
                ContainedTaskTrace::ResourceAdjustmentConfirmed {
                    captured_at,
                    operation_label,
                } => Some((*captured_at, operation_label.clone())),
                _ => None,
            })
            .collect()
    }

    fn joined_adjustments(&self) -> Vec<(SystemTime, Vec<ResourceAdjustmentResult>)> {
        self.traces
            .iter()
            .filter_map(|trace| match trace {
                ContainedTaskTrace::ResourceAdjustments {
                    captured_at,
                    adjustments,
                } => Some((*captured_at, adjustments.clone())),
                _ => None,
            })
            .collect()
    }

    // Each joined frame's readings as (capture time, key, value).
    fn readings(&self) -> Vec<(SystemTime, String, u64)> {
        self.traces
            .iter()
            .filter_map(|trace| match trace {
                ContainedTaskTrace::ResourceReadings {
                    captured_at,
                    readings,
                    ..
                } => Some(readings.iter().map(|reading| {
                    (
                        *captured_at,
                        reading.declaration.fact_key.clone(),
                        reading.value,
                    )
                })),
                _ => None,
            })
            .flatten()
            .collect()
    }
}

impl ContainedTaskRuntime for Device {
    type Error = &'static str;

    fn capture(&mut self) -> Result<ObservedFrame, Self::Error> {
        self.captures += 1;
        let mut pixels = vec![0_u8; WIDTH as usize * 3];
        pixels[..3].copy_from_slice(&colour(self.screen.page));
        pixels[3] = self.screen.balance;
        pixels[6] = self.screen.gain;
        pixels[9] = self.screen.spend;
        let mut frame = Frame::from_pixels(
            WIDTH,
            1,
            pixels,
            PixelFormat::Rgb8,
            CaptureBackendName::FixtureSimulation,
        )
        .expect("fixture frame");
        frame.captured_at = UNIX_EPOCH + Duration::from_secs(self.captures);
        self.shown.push((frame.captured_at, self.screen.page));
        Ok(frame.into())
    }

    fn input(
        &mut self,
        action: InputAction,
        _frame: Option<InputFrameContext>,
    ) -> Result<(), Self::Error> {
        let InputAction::Tap { x, .. } = action else {
            return Err("the fixture takes taps only");
        };
        self.screen = self
            .taps
            .get(&x)
            .copied()
            .ok_or("the tap names no screen")?;
        Ok(())
    }

    fn record_adjustment_evaluation(
        &mut self,
        captured_at: SystemTime,
        target_id: &str,
        _result: &actingcommand_recognition_pack::RecognitionPackResult<
            actingcommand_recognition_pack::OcrObservationEvaluation,
        >,
    ) -> Result<(), Self::Error> {
        self.adjustment_evaluations
            .push((captured_at, target_id.to_owned()));
        Ok(())
    }

    fn record(&mut self, trace: ContainedTaskTrace) -> Result<(), Self::Error> {
        self.traces.push(trace);
        Ok(())
    }
}

fn ocr_target(id: &str, x: u32) -> Value {
    json!({
        "type": "ocr", "id": id, "region": {"x": x, "y": 0, "width": 1, "height": 1},
        "languages": ["en"], "timeout_ms": 1000, "match_mode": "exact", "expected": ["0"],
        "case_sensitive": false, "minimum_confidence": 0.9, "model_ref": "PP-OCRv6_medium",
        "model_sha256": MODEL_SHA256
    })
}

// The pages' colour targets on the first pixel, the balance on the second, the two changes on
// the third and the fourth.
fn pack_json() -> Value {
    let mut targets = PAGES
        .iter()
        .map(|(page, colour)| {
            json!({"type": "color", "id": format!("page/{page}"),
                   "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": colour})
        })
        .collect::<Vec<_>>();
    targets.push(ocr_target("count/balance", 1));
    targets.push(ocr_target("count/gain", 2));
    targets.push(ocr_target("count/spend", 3));
    json!({
        "schema_version": "0.7",
        "game": "neutral",
        "server": "test",
        "coordinate_space": {"width": WIDTH, "height": 1},
        "defaults": {"color_max_distance": 0.0},
        "targets": targets
    })
}

fn pages_json() -> Value {
    let pages = PAGES
        .iter()
        .map(|(page, _)| {
            json!({"id": format!("neutral/{page}"), "required": [format!("page/{page}")],
                   "optional": [], "forbidden": []})
        })
        .collect::<Vec<_>>();
    json!({"schema_version": "0.3", "pages": pages})
}

fn control(execution_mode: &str, max_steps: Option<usize>) -> Value {
    let mut control = json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": "neutral.adjustment.task",
        "execution_mode": execution_mode,
        "game": "neutral",
        "server": "test",
        "resolution": {"width": WIDTH, "height": 1},
        "entry_task_id": "task",
        "capture_interval_ms": 1,
        "step_timeout_ms": 200,
        "timeout_ms": 60000
    });
    if let Some(max_steps) = max_steps {
        control["max_steps"] = json!(max_steps);
    }
    control
}

fn tap(id: &str, from: &str, to: &str, x: i32) -> Value {
    json!({
        "id": id, "from": from, "to": to,
        "click": {"kind": "point", "x": x, "y": 0},
        "unguarded_trusted_coordinate": true, "retryable": false
    })
}

fn mark(id: &str, page: &str) -> Value {
    json!({
        "id": id, "fact_key": format!("resource.{id}"), "page_id": page,
        "target_id": "count/balance", "trim": "whitespace_v1",
        "value": {"type": "unsigned_integer", "min": 0, "max": 100000},
        "minimum_confidence_milli": 900, "valid_for_ms": 60000
    })
}

fn no_designated_effect(terminal: &str) -> Value {
    json!({"mappings": [{"outcome_key": "done", "effect": "no_designated_effect",
                         "terminal_pages": [terminal]}]})
}

// The page-graph task: one tap `buy` from its entry page `home` to `terminal`, with
// `adjustments` and `marks`.
fn buy_task(adjustments: Value, marks: Vec<Value>) -> Value {
    let mut task = json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": WIDTH, "height": 1},
        "entry_page": "home",
        "target_page": "terminal",
        "scheduling_outcome": no_designated_effect("terminal"),
        "operations": [tap("buy", "home", "terminal", 1)],
        "resource_adjustments": adjustments
    });
    if !marks.is_empty() {
        task["resource_readings"] = json!(marks);
    }
    task
}

fn prepared(control: Value, program: Value) -> PreparedContainedTask {
    let control: TaskControl = serde_json::from_value(control).expect("task control");
    let program: TaskProgram = serde_json::from_value(program).expect("task program");
    let evaluator = RecognitionEvaluator::with_vision_provider(
        load_pack_from_json_str(&pack_json().to_string()).expect("recognition pack"),
        Arc::new(FsAssetResolver::new(PathBuf::new())),
        Arc::new(ShownCodes),
    )
    .expect("recognition evaluator");
    let detector = PageDetector::new(
        serde_json::from_value(json!({
            "schema_version": "0.6",
            "pages": PAGES
                .iter()
                .map(|(page, _)| json!({"id": format!("neutral/{page}"),
                                        "required": [format!("page/{page}")]}))
                .collect::<Vec<_>>()
        }))
        .expect("page set"),
    )
    .expect("page detector");
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
        package_sha256: "fixture-sha256".into(),
        entry_count: 1,
        task_count: 1,
    }
}

// Admits the task `task` as a sealed package, through the declaration gate.
fn admit(task: Value) -> Result<PreparedContainedTask, ContainedTaskError> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, value) in [
        ("control.json", control("navigable_route", Some(1))),
        (
            "resources/manifest.json",
            json!({"schema_version": "0.3", "entry_task_id": "task"}),
        ),
        ("resources/operations/task/task.json", task),
        ("resources/recognition/neutral.test.pack.json", pack_json()),
        (
            "resources/recognition/neutral.test.pages.json",
            pages_json(),
        ),
    ] {
        zip.start_file(path, options).expect("zip entry");
        serde_json::to_writer(&mut zip, &value).expect("zip JSON");
        zip.write_all(b"\n").expect("zip newline");
    }
    let bytes = zip.finish().expect("finish zip").into_inner();
    let recorded = PackageRef::LegacyZipSha256(Sha256Hash::digest(&bytes).to_string());
    PreparedContainedTask::load_with_vision_provider(
        "neutral.instance",
        &bytes,
        &recorded,
        Arc::new(ShownCodes),
    )
}

fn refused_at(result: Result<PreparedContainedTask, ContainedTaskError>) -> (String, String) {
    let error = match result {
        Ok(_) => panic!("the declaration is refused"),
        Err(error) => error,
    };
    assert_eq!(
        error.code().as_str(),
        SharedCode::ResourceDeclarationInvalid.as_str()
    );
    let issue = error
        .declaration_issue()
        .expect("the refusal names its declaration");
    (issue.field_path.clone(), format!("{:?}", issue.reason))
}

fn gain(id: &str, key: &str) -> Value {
    json!({"id": id, "fact_key": key, "operation_id": "buy",
           "amount": {"target_id": "count/gain", "direction": "gain"}})
}

fn spend(id: &str, key: &str) -> Value {
    json!({"id": id, "fact_key": key, "operation_id": "buy",
           "amount": {"target_id": "count/spend", "direction": "consume"}})
}

// T15 (D2M:595; v1.4 Q-D2 "这个实时识别即可") and T40 (D2M:620): the declaration gate refuses an
// amount that names a constant (an unknown member) and an adjustment read after its operation
// when the task marks no page with its key; with a mark of the key, and with amounts read from
// the task's own OCR targets, the package is admitted.
#[test]
fn d2s3_t15_t40_the_gate_refuses_a_constant_amount_and_an_after_read_without_a_mark() {
    let constant = json!([{"id": "spend", "fact_key": "resource.k1", "operation_id": "buy",
                           "amount": {"target_id": "count/spend", "direction": "consume",
                                      "value": 20}}]);
    assert_eq!(
        refused_at(admit(buy_task(constant, Vec::new()))),
        (
            "/resource_adjustments/0/amount/value".to_owned(),
            format!("{:?}", ResourceDeclarationReason::UnknownField)
        )
    );

    let unmarked = json!([{"id": "spend", "fact_key": "resource.k1", "operation_id": "buy"}]);
    assert_eq!(
        refused_at(admit(buy_task(
            unmarked.clone(),
            vec![mark("k2", "terminal")]
        ))),
        (
            "/resource_adjustments/0/fact_key".to_owned(),
            format!("{:?}", ResourceDeclarationReason::InvalidValue)
        )
    );

    let task = admit(buy_task(unmarked, vec![mark("k1", "terminal")]))
        .expect("an adjustment read after its operation by a mark of its key is admitted");
    assert!(task.has_resource_adjustments());
    admit(buy_task(
        json!([gain("gain", "resource.k1"), spend("spend", "resource.k2")]),
        Vec::new(),
    ))
    .expect("adjustments that read their amounts are admitted");
}

// T15 and T17 (D2M:595, :597; §3.6 "量只来自识别"): the operation `buy` is confirmed on the frame
// its step finished on, and only then; each of its adjustments reads its change on that frame,
// "+120" as a gain of 120 and "20" as a consumption of 20, at the provider's confidence, joined
// before `Finalizing`. When the tap does not reach the terminal page, the effect is not
// confirmed: nothing is confirmed and no amount is read.
#[test]
fn d2s3_t15_t17_an_amount_is_read_on_the_frame_its_operation_is_confirmed_on() {
    let task = prepared(
        control("navigable_route", None),
        buy_task(
            json!([gain("gain", "resource.k1"), spend("spend", "resource.k2")]),
            Vec::new(),
        ),
    );
    let mut device = Device::new(screen("home", 1, 0, 0), &[(1, screen("terminal", 1, 3, 4))]);

    match task.run(&mut device) {
        Ok(outcome) if outcome.outcome == TaskOutcome::Success => {}
        other => panic!("expected a successful run, got {other:?}"),
    }

    let Some(confirmed_at) = device.frames_of("terminal").first().copied() else {
        panic!("no terminal frame: {:?}", device.shown);
    };
    assert_eq!(
        device.confirmations(),
        vec![(confirmed_at, "buy".to_owned())]
    );
    let finished = device
        .position(|trace| {
            matches!(trace, ContainedTaskTrace::StepFinished { operation_label, .. }
                if operation_label == "buy")
        })
        .expect("the step finished");
    let confirmed = device
        .position(|trace| {
            matches!(
                trace,
                ContainedTaskTrace::ResourceAdjustmentConfirmed { .. }
            )
        })
        .expect("the operation was confirmed");
    let joined = device
        .position(|trace| matches!(trace, ContainedTaskTrace::ResourceAdjustments { .. }))
        .expect("the adjustments were joined");
    let finalizing = device
        .position(|trace| matches!(trace, ContainedTaskTrace::Finalizing { .. }))
        .expect("the run finalized");
    assert!(finished < confirmed && confirmed < joined && joined < finalizing);
    let amounts = device
        .joined_adjustments()
        .into_iter()
        .map(|(captured_at, adjustments)| {
            (
                captured_at,
                adjustments
                    .into_iter()
                    .map(|adjustment| (adjustment.declaration.id, adjustment.amount))
                    .collect::<Vec<_>>(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        amounts,
        vec![(
            confirmed_at,
            vec![
                (
                    "gain".to_owned(),
                    Some(Ok(ResourceAdjustmentAmountValue {
                        value: 120,
                        confidence_milli: 990
                    }))
                ),
                (
                    "spend".to_owned(),
                    Some(Ok(ResourceAdjustmentAmountValue {
                        value: 20,
                        confidence_milli: 990
                    }))
                ),
            ]
        )]
    );
    assert_eq!(
        device.adjustment_evaluations,
        vec![
            (confirmed_at, "count/gain".to_owned()),
            (confirmed_at, "count/spend".to_owned()),
        ]
    );

    let mut unmoved = Device::new(screen("home", 1, 0, 0), &[(1, screen("home", 1, 3, 4))]);
    assert!(
        task.run(&mut unmoved).is_err(),
        "a tap that does not reach the terminal page fails the run"
    );
    assert!(unmoved.confirmations().is_empty());
    assert!(unmoved.joined_adjustments().is_empty());
    assert!(unmoved.adjustment_evaluations.is_empty());
}

// T40 (D2M:620; v1.4 Q-D2): a linear package goes from `home` to `shop` and spends back to
// `home`, whose balance it marks; the adjustment of the balance has no amount. The run reads the
// balance on its entry frame (100) and on the frame `spend` arrived on (83); `spend` is
// confirmed on that frame, and the adjustment is joined without an amount.
#[test]
fn d2s3_t40_a_change_read_after_its_operation_is_read_before_and_after_it() {
    let task = prepared(
        control("linear_steps", Some(2)),
        json!({
            "schema_version": "0.9",
            "task_id": "task",
            "game": "neutral",
            "server_scope": ["test"],
            "coordinate_space": {"width": WIDTH, "height": 1},
            "entry_page": "home",
            "target_page": "home",
            "max_steps": 2,
            "scheduling_outcome": no_designated_effect("home"),
            "operations": [
                tap("to_shop", "home", "shop", 1),
                tap("spend", "shop", "home", 2)
            ],
            "resource_readings": [mark("k1", "home")],
            "resource_adjustments": [
                {"id": "spent", "fact_key": "resource.k1", "operation_id": "spend"}
            ]
        }),
    );
    let mut device = Device::new(
        screen("home", 1, 0, 0),
        &[(1, screen("shop", 1, 0, 0)), (2, screen("home", 2, 0, 0))],
    );

    match task.run(&mut device) {
        Ok(outcome) if outcome.outcome == TaskOutcome::Success => {}
        other => panic!("expected a successful run, got {other:?}"),
    }

    let homes = device.frames_of("home");
    let (Some(entry), Some(after)) = (homes.first().copied(), homes.last().copied()) else {
        panic!("home frames: {:?}", device.shown);
    };
    assert!(entry < after);
    assert_eq!(
        device.readings(),
        vec![
            (entry, "resource.k1".to_owned(), 100),
            (after, "resource.k1".to_owned(), 83),
        ]
    );
    assert_eq!(device.confirmations(), vec![(after, "spend".to_owned())]);
    let joined = device.joined_adjustments();
    let [(captured_at, adjustments)] = joined.as_slice() else {
        panic!("one joined confirmation: {joined:?}");
    };
    assert_eq!(*captured_at, after);
    let [adjustment] = adjustments.as_slice() else {
        panic!("one adjustment: {adjustments:?}");
    };
    assert_eq!(adjustment.declaration.id, "spent");
    assert_eq!(adjustment.amount, None);
    assert!(device.adjustment_evaluations.is_empty());
}
