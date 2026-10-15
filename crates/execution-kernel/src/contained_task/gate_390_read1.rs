// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #390 READ-1 (forced-compromises mapping O-08, row READ-1): a fraction reading also
// writes its denominator, the capacity, as a second reading key. A `current_capacity` mark that
// declares `capacity_fact_key` gives two readings from one evaluation on its frame, its value
// under `fact_key` and its capacity under `capacity_fact_key`; the second key is a reading key
// of the package like the first (the keys it marks, a requested key set). Only a fraction has a
// denominator, and every reading key of a task is unique.
//
// The device shows one page at a time: a frame's first pixel is the page's colour and its second
// pixel the code of the fraction the page shows. A tap moves the device to the screen its tap
// table names. Captures are stamped one second apart from the Unix epoch. The OCR provider reads
// the text of the code in the pixel its region starts on, at confidence 0.99.

use super::*;
use actingcommand_contract::ResourceDeclarationReason;
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

const WIDTH: u32 = 4;
const MODEL_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const PAGES: [(&str, [u8; 3]); 2] = [("home", [255, 0, 0]), ("terminal", [0, 0, 255])];
// The text of each code a pixel can show.
const TEXTS: [&str; 3] = ["", "17/20", "35/60"];

fn colour(page: &str) -> [u8; 3] {
    PAGES
        .iter()
        .find(|(name, _)| *name == page)
        .map(|(_, colour)| *colour)
        .unwrap_or_else(|| panic!("unknown fixture page {page}"))
}

// Reads the text of the code in the pixel its region starts on, at confidence 0.99.
#[derive(Debug, Default)]
struct ShownFractions;

impl VisionProvider for ShownFractions {
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
                invocation_id: "reading-ocr".to_owned(),
                session_id: "reading-session".to_owned(),
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

// What the device shows: its page and the code of the fraction on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Screen {
    page: &'static str,
    fraction: u8,
}

const fn screen(page: &'static str, fraction: u8) -> Screen {
    Screen { page, fraction }
}

// The device and the run's records.
struct Device {
    screen: Screen,
    taps: BTreeMap<i32, Screen>,
    captures: u64,
    shown: Vec<(SystemTime, &'static str)>,
    traces: Vec<ContainedTaskTrace>,
}

impl Device {
    // Starts on `home` showing 17/20; the tap at column 1 moves to `terminal` showing 35/60.
    fn on_home() -> Self {
        Self {
            screen: screen("home", 1),
            taps: [(1, screen("terminal", 2))].into_iter().collect(),
            captures: 0,
            shown: Vec::new(),
            traces: Vec::new(),
        }
    }

    // The page the frame captured at `captured_at` showed.
    fn page_at(&self, captured_at: SystemTime) -> &'static str {
        self.shown
            .iter()
            .find(|(at, _)| *at == captured_at)
            .map(|(_, page)| *page)
            .expect("a captured frame")
    }

    // Each joined frame's page and its readings as (key, value, confidence milli), in join
    // order; and every mark that gave no reading.
    #[allow(clippy::type_complexity)]
    fn joined(&self) -> (Vec<(&'static str, Vec<(String, u64, u16)>)>, usize) {
        let mut frames = Vec::new();
        let mut misses = 0;
        for trace in &self.traces {
            if let ContainedTaskTrace::ResourceReadings {
                captured_at,
                readings,
                unresolved,
            } = trace
            {
                frames.push((
                    self.page_at(*captured_at),
                    readings
                        .iter()
                        .map(|reading| {
                            (
                                reading.fact_key.clone(),
                                reading.value,
                                reading.confidence_milli,
                            )
                        })
                        .collect(),
                ));
                misses += unresolved.len();
            }
        }
        (frames, misses)
    }
}

impl ContainedTaskRuntime for Device {
    type Error = &'static str;

    fn capture(&mut self) -> Result<ObservedFrame, Self::Error> {
        self.captures += 1;
        let mut pixels = vec![0_u8; WIDTH as usize * 3];
        pixels[..3].copy_from_slice(&colour(self.screen.page));
        pixels[3] = self.screen.fraction;
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

    fn record(&mut self, trace: ContainedTaskTrace) -> Result<(), Self::Error> {
        self.traces.push(trace);
        Ok(())
    }
}

// The pages' colour targets on the first pixel and the fraction on the second.
fn pack_json() -> Value {
    let mut targets = PAGES
        .iter()
        .map(|(page, colour)| {
            json!({"type": "color", "id": format!("page/{page}"),
                   "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": colour})
        })
        .collect::<Vec<_>>();
    targets.push(json!({
        "type": "ocr", "id": "count/fraction", "region": {"x": 1, "y": 0, "width": 1, "height": 1},
        "languages": ["en"], "timeout_ms": 1000, "match_mode": "exact", "expected": ["0"],
        "case_sensitive": false, "minimum_confidence": 0.9, "model_ref": "PP-OCRv6_medium",
        "model_sha256": MODEL_SHA256
    }));
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

fn control(max_steps: Option<usize>) -> Value {
    let mut control = json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": "neutral.reading.task",
        "execution_mode": "navigable_route",
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

// A mark of the fraction on `page`, written as `resource.<id>`; with `capacity`, its capacity
// is written as that second key.
fn fraction_mark(id: &str, page: &str, format: &str, capacity: Option<&str>) -> Value {
    let mut mark = json!({
        "id": id, "fact_key": format!("resource.{id}"), "page_id": page,
        "target_id": "count/fraction", "trim": "whitespace_v1",
        "value": {"type": "unsigned_integer", "min": 0, "max": 999, "format": format},
        "minimum_confidence_milli": 900, "valid_for_ms": 60000
    });
    if let Some(capacity) = capacity {
        mark["capacity_fact_key"] = json!(capacity);
    }
    mark
}

// The page-graph task: one tap `open` from its entry page `home` to `terminal`, with `marks`.
fn open_task(marks: Vec<Value>) -> Value {
    json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": WIDTH, "height": 1},
        "entry_page": "home",
        "target_page": "terminal",
        "scheduling_outcome": {"mappings": [{"outcome_key": "done",
                                             "effect": "no_designated_effect",
                                             "terminal_pages": ["terminal"]}]},
        "operations": [{
            "id": "open", "from": "home", "to": "terminal",
            "click": {"kind": "point", "x": 1, "y": 0},
            "unguarded_trusted_coordinate": true, "retryable": false
        }],
        "resource_readings": marks
    })
}

// `home` marks its fraction without a second key, `terminal` with `resource.k1_capacity`.
fn two_marks() -> Vec<Value> {
    vec![
        fraction_mark("k2", "home", "current_capacity", None),
        fraction_mark(
            "k1",
            "terminal",
            "current_capacity",
            Some("resource.k1_capacity"),
        ),
    ]
}

fn prepared(program: Value) -> PreparedContainedTask {
    let control: TaskControl = serde_json::from_value(control(None)).expect("task control");
    let program: TaskProgram = serde_json::from_value(program).expect("task program");
    let evaluator = RecognitionEvaluator::with_vision_provider(
        load_pack_from_json_str(&pack_json().to_string()).expect("recognition pack"),
        Arc::new(FsAssetResolver::new(PathBuf::new())),
        Arc::new(ShownFractions),
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
        entry_count: 1,
        task_count: 1,
    }
}

// Admits the task `task` as a sealed package, through the declaration gate.
fn admit(task: Value) -> Result<PreparedContainedTask, ContainedTaskError> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, value) in [
        ("control.json", control(Some(1))),
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
    PreparedContainedTask::load_with_vision_provider(
        "neutral.instance",
        &bytes,
        Arc::new(ShownFractions),
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

fn succeeded(
    result: Result<ContainedTaskOutcome, ContainedTaskRunError<&'static str>>,
) -> ContainedTaskOutcome {
    match result {
        Ok(outcome) if outcome.outcome == TaskOutcome::Success => outcome,
        other => panic!("expected a successful run, got {other:?}"),
    }
}

// READ-1 (FCM O-08, row READ-1 "分数格式读数同时输出分母（容量）为第二读数键"): a fraction mark
// with a second reading key is admitted, and the package marks both keys, the second after the
// first. Only a fraction (`current_capacity`) has a denominator to write; the second key follows
// the reading key rule (`resource.` or `inventory.`) and, like every reading key, is unique in
// the task. Each refusal names the second key's field.
#[test]
fn read1_the_gate_admits_a_second_reading_key_only_on_a_fraction_mark() {
    let admitted = admit(open_task(two_marks()))
        .expect("a fraction mark with a second reading key is admitted");
    assert_eq!(
        admitted.reading_keys(),
        ["resource.k2", "resource.k1", "resource.k1_capacity"]
    );
    assert_eq!(
        admitted.package_descriptor().reading_keys(),
        ["resource.k2", "resource.k1", "resource.k1_capacity"]
    );

    let capacity_field = |index: usize| {
        (
            format!("/resource_readings/{index}/capacity_fact_key"),
            format!("{:?}", ResourceDeclarationReason::InvalidValue),
        )
    };
    // Not a fraction: a plain number has no denominator.
    assert_eq!(
        refused_at(admit(open_task(vec![fraction_mark(
            "k1",
            "terminal",
            "ascii_decimal",
            Some("resource.k1_capacity"),
        )]))),
        capacity_field(0)
    );
    // Not a reading key.
    assert_eq!(
        refused_at(admit(open_task(vec![fraction_mark(
            "k1",
            "terminal",
            "current_capacity",
            Some("session.k1_capacity"),
        )]))),
        capacity_field(0)
    );
    // The key of another mark.
    assert_eq!(
        refused_at(admit(open_task(vec![
            fraction_mark("k2", "home", "current_capacity", None),
            fraction_mark("k1", "terminal", "current_capacity", Some("resource.k2")),
        ]))),
        capacity_field(1)
    );
}

// READ-1 (FCM O-08): on the frame the run reaches `terminal` on, the mark reads 35/60 and
// writes two readings in mark order, 35 under `resource.k1` and the denominator 60 under its
// second key `resource.k1_capacity`, at the one evaluation's confidence; the `home` mark without
// a second key still writes its current value 17 alone. Nothing is a miss.
#[test]
fn read1_a_fraction_mark_writes_its_denominator_as_a_second_reading() {
    let mut device = Device::on_home();

    let outcome = succeeded(prepared(open_task(two_marks())).run(&mut device));

    assert_eq!(outcome.final_page.as_deref(), Some("neutral/terminal"));
    assert_eq!(
        device.joined(),
        (
            vec![
                ("home", vec![("resource.k2".to_owned(), 17, 990)]),
                (
                    "terminal",
                    vec![
                        ("resource.k1".to_owned(), 35, 990),
                        ("resource.k1_capacity".to_owned(), 60, 990),
                    ]
                ),
            ],
            0
        )
    );
}

// READ-1 with Workflow #384 D2-S2 §3.4 (a requested key set reads only the marks of its keys):
// the key set {`resource.k1_capacity`}, the second key alone, reads the `terminal` mark, value
// and denominator, and not the `home` mark.
#[test]
fn read1_a_key_set_of_the_second_key_reads_its_mark() {
    let mut device = Device::on_home();
    let keys = ["resource.k1_capacity".to_owned()]
        .into_iter()
        .collect::<BTreeSet<_>>();

    succeeded(prepared(open_task(two_marks())).run_for_reading_keys(&mut device, &keys));

    assert_eq!(
        device.joined(),
        (
            vec![(
                "terminal",
                vec![
                    ("resource.k1".to_owned(), 35, 990),
                    ("resource.k1_capacity".to_owned(), 60, 990),
                ]
            )],
            0
        )
    );
}
