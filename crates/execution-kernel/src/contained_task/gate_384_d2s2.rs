// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #384 D2-S2 (`rd-1009/d2/model-d2-fact-refresh.md` v1.4.2 §8, the S2 halves of rows
// T8-T13): reading marks on any page of a task, read on the frames the run reaches their pages
// on and joined before `Finalizing`; a mark that gives no reading is a warning of its own; the
// recognition maintenance package and the segments a requested key set runs.
//
// The device shows one page at a time: a frame's first pixel is the page's colour, and a tap at
// column `x` moves it to the page its tap table names. Captures are stamped one second apart
// from the Unix epoch, so each frame's capture time names it. The OCR provider reads the number
// the page shows. Every expected value comes from the model rows named on each test.

use super::*;
use actingcommand_contract::codes::RuntimeContractCode;
use actingcommand_contract::{
    BundleIndex, BundleIndexVersion, BundleMaintenance, BundlePack, MaintenanceUse, ReaderProblem,
    validate_bundle_maintenance_declarations,
};
use actingcommand_device::CaptureBackendName;
use actingcommand_recognition_pack::{
    FsAssetResolver, NnProviderRequest, NnProviderResult, OcrExecutionProviderKind,
    OcrProviderObservation, OcrProviderRequest, OcrProviderResult, OcrProviderTextBlock,
    VisionProviderError, VisionProviderErrorCode, load_pack_from_json_str,
};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::{Condvar, Mutex};
use std::time::UNIX_EPOCH;

const WIDTH: u32 = 8;
const MODEL_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
// The pages, their colours and the numbers they show.
const PAGES: [(&str, [u8; 3], &str); 5] = [
    ("home", [255, 0, 0], "100"),
    ("p1", [0, 255, 0], "11"),
    ("p2", [0, 0, 255], "22"),
    ("p3", [255, 255, 0], "33"),
    ("terminal", [0, 255, 255], "44"),
];

fn colour(page: &str) -> [u8; 3] {
    PAGES
        .iter()
        .find(|(name, _, _)| *name == page)
        .map(|(_, colour, _)| *colour)
        .unwrap_or_else(|| panic!("unknown fixture page {page}"))
}

fn page_of(pixel: &[u8]) -> &'static str {
    PAGES
        .iter()
        .find(|(_, colour, _)| colour.as_slice() == pixel)
        .map(|(name, _, _)| *name)
        .unwrap_or_else(|| panic!("no fixture page has colour {pixel:?}"))
}

fn number(page: &str) -> &'static str {
    PAGES
        .iter()
        .find(|(name, _, _)| *name == page)
        .map(|(_, _, number)| *number)
        .unwrap_or_else(|| panic!("unknown fixture page {page}"))
}

// A gate a recognition waits at until the test or the runtime opens it.
#[derive(Debug, Default)]
struct Gate {
    open: Mutex<bool>,
    opened: Condvar,
}

impl Gate {
    fn open(&self) {
        *self.open.lock().expect("gate lock") = true;
        self.opened.notify_all();
    }

    fn wait(&self) {
        let mut open = self.open.lock().expect("gate lock");
        while !*open {
            open = self.opened.wait(open).expect("gate lock");
        }
    }
}

// Reads the number the page shows, at confidence 0.99, or 0.5 on `low_confidence_page`; with a
// gate, every read waits until the gate opens.
#[derive(Debug, Default)]
struct PageNumbers {
    low_confidence_page: Option<&'static str>,
    gate: Option<Arc<Gate>>,
}

impl VisionProvider for PageNumbers {
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
        if let Some(gate) = &self.gate {
            gate.wait();
        }
        let page = page_of(&request.frame.rgb8_pixels[..3]);
        let text = number(page).to_owned();
        let confidence = if self.low_confidence_page == Some(page) {
            0.5
        } else {
            0.99
        };
        Ok(OcrProviderObservation {
            result: OcrProviderResult {
                ppocr_diagnostics: Vec::new(),
                text: text.clone(),
                blocks: vec![OcrProviderTextBlock {
                    text,
                    rect: request.region,
                    confidence: Some(confidence),
                }],
                confidence: Some(confidence),
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

// The device and the run's records. `join_timeout` is the runtime's join timeout; with
// `release_on_join`, the gate opens when the run asks for it, as the run starts its join.
struct Device {
    page: &'static str,
    taps: BTreeMap<i32, &'static str>,
    captures: u64,
    shown: Vec<(SystemTime, &'static str)>,
    taps_made: Vec<i32>,
    join_timeout: Option<Duration>,
    release_on_join: Option<Arc<Gate>>,
    evaluations: Vec<(SystemTime, String)>,
    traces: Vec<ContainedTaskTrace>,
}

impl Device {
    fn new(taps: &[(i32, &'static str)]) -> Self {
        Self {
            page: "home",
            taps: taps.iter().copied().collect(),
            captures: 0,
            shown: Vec::new(),
            taps_made: Vec::new(),
            join_timeout: None,
            release_on_join: None,
            evaluations: Vec::new(),
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

    // The capture time of the first frame that showed `page`.
    fn first_frame_of(&self, page: &str) -> SystemTime {
        self.shown
            .iter()
            .find(|(_, shown)| *shown == page)
            .map(|(at, _)| *at)
            .expect("a frame of the page")
    }

    fn handed_off(&self) -> Vec<SystemTime> {
        self.traces
            .iter()
            .filter_map(|trace| match trace {
                ContainedTaskTrace::ResourceReadingFrame { captured_at } => Some(*captured_at),
                _ => None,
            })
            .collect()
    }

    // Each joined frame: its capture time, its readings as (key, value) and its misses as
    // (key, problem), in join order.
    #[allow(clippy::type_complexity)]
    fn joined(
        &self,
    ) -> Vec<(
        SystemTime,
        Vec<(String, u64)>,
        Vec<(String, ResourceReadingProblem)>,
    )> {
        self.traces
            .iter()
            .filter_map(|trace| match trace {
                ContainedTaskTrace::ResourceReadings {
                    captured_at,
                    readings,
                    unresolved,
                } => Some((
                    *captured_at,
                    readings
                        .iter()
                        .map(|reading| (reading.declaration.fact_key.clone(), reading.value))
                        .collect(),
                    unresolved
                        .iter()
                        .map(|miss| (miss.declaration.fact_key.clone(), miss.problem))
                        .collect(),
                )),
                _ => None,
            })
            .collect()
    }

    fn position(&self, wanted: impl Fn(&ContainedTaskTrace) -> bool) -> usize {
        self.traces
            .iter()
            .position(wanted)
            .expect("the trace was recorded")
    }

    fn steps_started(&self) -> Vec<String> {
        self.traces
            .iter()
            .filter_map(|trace| match trace {
                ContainedTaskTrace::StepStarted {
                    operation_label, ..
                } => Some(operation_label.clone()),
                _ => None,
            })
            .collect()
    }
}

impl ContainedTaskRuntime for Device {
    type Error = &'static str;

    fn capture(&mut self) -> Result<ObservedFrame, Self::Error> {
        self.captures += 1;
        let mut pixels = vec![0_u8; WIDTH as usize * 3];
        pixels[..3].copy_from_slice(&colour(self.page));
        let mut frame = Frame::from_pixels(
            WIDTH,
            1,
            pixels,
            PixelFormat::Rgb8,
            CaptureBackendName::FixtureSimulation,
        )
        .expect("fixture frame");
        frame.captured_at = UNIX_EPOCH + Duration::from_secs(self.captures);
        self.shown.push((frame.captured_at, self.page));
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
        self.taps_made.push(x);
        self.page = self.taps.get(&x).copied().ok_or("the tap names no page")?;
        Ok(())
    }

    fn reading_join_timeout(&mut self) -> Result<Option<Duration>, Self::Error> {
        if let Some(gate) = self.release_on_join.take() {
            gate.open();
        }
        Ok(self.join_timeout)
    }

    fn record_reading_evaluation(
        &mut self,
        captured_at: SystemTime,
        target_id: &str,
        _result: &actingcommand_recognition_pack::RecognitionPackResult<
            actingcommand_recognition_pack::OcrObservationEvaluation,
        >,
    ) -> Result<(), Self::Error> {
        self.evaluations.push((captured_at, target_id.to_owned()));
        Ok(())
    }

    fn record(&mut self, trace: ContainedTaskTrace) -> Result<(), Self::Error> {
        self.traces.push(trace);
        Ok(())
    }
}

fn ocr_target(id: &str, region: Value) -> Value {
    json!({
        "type": "ocr", "id": id, "region": region, "languages": ["en"], "timeout_ms": 1000,
        "match_mode": "exact", "expected": ["0"], "case_sensitive": false,
        "minimum_confidence": 0.0, "model_ref": "PP-OCRv6_medium", "model_sha256": MODEL_SHA256
    })
}

// The pages' colour targets on the first pixel; one OCR target per mark on the second pixel and
// one over the whole frame (a whole-page mark).
fn pack() -> actingcommand_recognition_pack::RecognitionPack {
    let mut targets = PAGES
        .iter()
        .map(|(page, colour, _)| {
            json!({"type": "color", "id": format!("page/{page}"),
                   "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": colour})
        })
        .collect::<Vec<_>>();
    let cell = json!({"x": 1, "y": 0, "width": 1, "height": 1});
    for id in ["count/k1", "count/k2", "count/k3"] {
        targets.push(ocr_target(id, cell.clone()));
    }
    targets.push(ocr_target(
        "count/page",
        json!({"x": 0, "y": 0, "width": WIDTH, "height": 1}),
    ));
    load_pack_from_json_str(
        &json!({
            "schema_version": "0.7",
            "game": "neutral",
            "server": "test",
            "coordinate_space": {"width": WIDTH, "height": 1},
            "defaults": {"color_max_distance": 0.0},
            "targets": targets
        })
        .to_string(),
    )
    .expect("recognition pack")
}

fn detector() -> PageDetector {
    let pages = PAGES
        .iter()
        .map(|(page, _, _)| json!({"id": format!("neutral/{page}"), "required": [format!("page/{page}")]}))
        .collect::<Vec<_>>();
    PageDetector::new(
        serde_json::from_value(json!({"schema_version": "0.6", "pages": pages})).expect("page set"),
    )
    .expect("page detector")
}

fn mark(id: &str, page: &str, target: &str) -> Value {
    json!({
        "id": id, "fact_key": format!("resource.{id}"), "page_id": page, "target_id": target,
        "trim": "whitespace_v1",
        "value": {"type": "unsigned_integer", "min": 0, "max": 100000},
        "minimum_confidence_milli": 900, "valid_for_ms": 60000
    })
}

fn tap(id: &str, from: &str, to: &str, x: i32) -> Value {
    json!({
        "id": id, "from": from, "to": to,
        "click": {"kind": "point", "x": x, "y": 0},
        "unguarded_trusted_coordinate": true, "retryable": false
    })
}

fn control(execution_mode: &str, max_steps: Option<usize>) -> Value {
    let mut control = json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": "neutral.reading.task",
        "execution_mode": execution_mode,
        "game": "neutral",
        "server": "test",
        "resolution": {"width": WIDTH, "height": 1},
        "entry_task_id": "task",
        "capture_interval_ms": 1,
        "step_timeout_ms": 5000,
        "timeout_ms": 60000
    });
    if let Some(max_steps) = max_steps {
        control["max_steps"] = json!(max_steps);
    }
    control
}

// A page-graph task to `target` through `operations`, with `marks` and the scheduling outcome
// `outcome` when one is given.
fn route(target: &str, operations: Vec<Value>, marks: Vec<Value>, outcome: Option<Value>) -> Value {
    let mut program = json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": WIDTH, "height": 1},
        "target_page": target,
        "operations": operations
    });
    if !marks.is_empty() {
        program["resource_readings"] = json!(marks);
    }
    if let Some(outcome) = outcome {
        program["scheduling_outcome"] = outcome;
    }
    program
}

fn prepared(control: Value, program: Value, provider: Arc<PageNumbers>) -> PreparedContainedTask {
    let control: TaskControl = serde_json::from_value(control).expect("task control");
    let program: TaskProgram = serde_json::from_value(program).expect("task program");
    let evaluator = RecognitionEvaluator::with_vision_provider(
        pack(),
        Arc::new(FsAssetResolver::new(PathBuf::new())),
        provider,
    )
    .expect("recognition evaluator");
    let scheduling_outcome = program.scheduling_outcome.clone();
    PreparedContainedTask {
        control,
        program,
        evaluator,
        detector: detector(),
        entry_page: None,
        scheduling_outcome,
        post_admission_ocr: None,
        post_admission_fields: None,
        entry_count: 1,
        task_count: 1,
    }
}

fn no_designated_effect(terminal: &str) -> Value {
    json!({"mappings": [{"outcome_key": "read", "effect": "no_designated_effect",
                         "terminal_pages": [terminal]}]})
}

// A recognition maintenance package: it navigates from `home` into `p1` and reads `k1` there.
fn reader(provider: Arc<PageNumbers>) -> PreparedContainedTask {
    prepared(
        control("navigable_route", None),
        route(
            "p1",
            vec![tap("to_p1", "home", "p1", 1)],
            vec![mark("k1", "p1", "count/k1")],
            Some(no_designated_effect("p1")),
        ),
        provider,
    )
}

fn succeeded(
    result: Result<ContainedTaskOutcome, ContainedTaskRunError<&'static str>>,
) -> ContainedTaskOutcome {
    match result {
        Ok(outcome) if outcome.outcome == TaskOutcome::Success => outcome,
        other => panic!("expected a successful run, got {other:?}"),
    }
}

fn keys(keys: &[&str]) -> BTreeSet<String> {
    keys.iter().map(|key| (*key).to_owned()).collect()
}

// T8 (D2M:588; V2 "识别维护包每个标准包下只能有一个"): two `recognition` entries of one server
// are `duplicate_bundle_maintenance_role`; one passes the bundle's maintenance check; a reader
// that does not know a use refuses it explicitly instead of ignoring it.
#[test]
fn d2s2_t8_one_recognition_package_per_server_and_an_unknown_use_is_refused() {
    let entry = |package_id: &str| BundleMaintenance {
        package_id: package_id.to_owned(),
        server: "test".to_owned(),
        uses: vec![MaintenanceUse::Recognition],
    };
    let error = validate_bundle_maintenance_declarations(&[
        entry("neutral.reading.task"),
        entry("neutral.reading.other"),
    ])
    .expect_err("two recognition packages of one server are refused");
    assert_eq!(
        error.code().as_str(),
        RuntimeContractCode::DuplicateBundleMaintenanceRole.as_str()
    );

    let index = BundleIndex {
        schema_version: BundleIndexVersion::V4,
        game: "neutral".to_owned(),
        source: None,
        packs: vec![BundlePack {
            package_id: "neutral.reading.task".to_owned(),
            server: "test".to_owned(),
            entry_task_id: "task".to_owned(),
            path: "packs/neutral.reading.task".to_owned(),
            file_count: 3,
            byte_count: 64,
        }],
        maintenance: Some(vec![entry("neutral.reading.task")]),
        tables: None,
    };
    crate::validate_bundle_maintenance(
        &index,
        &[reader(Arc::new(PageNumbers::default())).package_descriptor()],
    )
    .expect("one recognition package of the server passes");

    serde_json::from_value::<BundleMaintenance>(json!({
        "package_id": "neutral.reading.task", "server": "test", "uses": ["reading"]
    }))
    .expect_err("a use the reader does not know is refused");
}

// T9 (D2M:589; V3 "进入一个或多个页面后退出"; §3.10): a package with a designated operation is no
// reader (`designated_operation`), nor one without marks (`no_marks`); a package that taps into
// a page and reads it qualifies, and the bundle check refuses the use for the others.
#[test]
fn d2s2_t9_a_reader_navigates_and_reads_and_declares_no_operation() {
    let provider = Arc::new(PageNumbers::default());
    let designated = prepared(
        control("navigable_route", None),
        route(
            "p1",
            vec![tap("to_p1", "home", "p1", 1)],
            vec![mark("k1", "p1", "count/k1")],
            Some(json!({"designated_operation": "to_p1", "mappings": [
                {"outcome_key": "done", "effect": "designated_effect_completed",
                 "terminal_pages": ["p1"]},
                {"outcome_key": "read", "effect": "no_designated_effect",
                 "terminal_pages": ["p1"]}
            ]})),
        ),
        Arc::clone(&provider),
    )
    .package_descriptor();
    let unmarked = prepared(
        control("navigable_route", None),
        route(
            "p1",
            vec![tap("to_p1", "home", "p1", 1)],
            Vec::new(),
            Some(no_designated_effect("p1")),
        ),
        Arc::clone(&provider),
    )
    .package_descriptor();
    let qualified = reader(provider).package_descriptor();

    assert_eq!(
        designated.recognition_problem(),
        Some(ReaderProblem::DesignatedOperation)
    );
    assert_eq!(
        designated.use_incompatibility(MaintenanceUse::Recognition),
        Some("designated_operation")
    );
    assert_eq!(unmarked.recognition_problem(), Some(ReaderProblem::NoMarks));
    assert_eq!(
        unmarked.use_incompatibility(MaintenanceUse::Recognition),
        Some("no_marks")
    );
    assert_eq!(qualified.recognition_problem(), None);
    assert_eq!(
        qualified.use_incompatibility(MaintenanceUse::Recognition),
        None
    );
    assert_eq!(qualified.reading_keys(), ["resource.k1"]);

    let index = BundleIndex {
        schema_version: BundleIndexVersion::V4,
        game: "neutral".to_owned(),
        source: None,
        packs: vec![BundlePack {
            package_id: "neutral.reading.task".to_owned(),
            server: "test".to_owned(),
            entry_task_id: "task".to_owned(),
            path: "packs/neutral.reading.task".to_owned(),
            file_count: 3,
            byte_count: 64,
        }],
        maintenance: Some(vec![BundleMaintenance {
            package_id: "neutral.reading.task".to_owned(),
            server: "test".to_owned(),
            uses: vec![MaintenanceUse::Recognition],
        }]),
        tables: None,
    };
    let error = crate::validate_bundle_maintenance(&index, &[designated])
        .expect_err("a package with a designated operation is no recognition package");
    assert_eq!(
        error.code().as_str(),
        ExecutionKernelCode::BundleMaintenanceInvalid.as_str()
    );
}

// T10 (D2M:590; V3 "业务包内我们也可以标记页面或区域", "可以整页"): a business package marks the
// whole entry page, a region of the non-terminal page `p1` and its terminal page; the run reads
// each on the first frame of its page and joins the three readings before `Finalizing`, each
// with that frame's capture time.
#[test]
fn d2s2_t10_marks_on_any_page_and_the_whole_page_are_read_on_their_frames() {
    let task = prepared(
        control("navigable_route", None),
        route(
            "terminal",
            vec![
                tap("to_p1", "home", "p1", 1),
                tap("to_terminal", "p1", "terminal", 2),
            ],
            vec![
                mark("k2", "home", "count/page"),
                mark("k1", "p1", "count/k1"),
                mark("k3", "terminal", "count/k3"),
            ],
            Some(no_designated_effect("terminal")),
        ),
        Arc::new(PageNumbers::default()),
    );
    let mut device = Device::new(&[(1, "p1"), (2, "terminal")]);

    let outcome = succeeded(task.run(&mut device));

    assert_eq!(outcome.final_page.as_deref(), Some("neutral/terminal"));
    let frames = ["home", "p1", "terminal"].map(|page| device.first_frame_of(page));
    assert_eq!(device.handed_off(), frames);
    assert_eq!(
        device.joined(),
        vec![
            (frames[0], vec![("resource.k2".to_owned(), 100)], Vec::new()),
            (frames[1], vec![("resource.k1".to_owned(), 11)], Vec::new()),
            (frames[2], vec![("resource.k3".to_owned(), 44)], Vec::new()),
        ]
    );
    assert_eq!(
        device.evaluations,
        vec![
            (frames[0], "count/page".to_owned()),
            (frames[1], "count/k1".to_owned()),
            (frames[2], "count/k3".to_owned()),
        ]
    );
    let finalizing =
        device.position(|trace| matches!(trace, ContainedTaskTrace::Finalizing { .. }));
    let last_joined = device
        .traces
        .iter()
        .rposition(|trace| matches!(trace, ContainedTaskTrace::ResourceReadings { .. }))
        .expect("readings were joined");
    assert!(last_joined < finalizing);
    for (captured_at, _, _) in device.joined() {
        assert!(["home", "p1", "terminal"].contains(&device.page_at(captured_at)));
    }
}

// T11 (D2M:591; §3.5 join; O-2): a recognition that has not returned when the run reaches its
// join is waited for, and its reading is recorded before `Finalizing`; one that is still running
// when the join timeout ends is the miss `join_timeout`, and the run succeeds.
#[test]
fn d2s2_t11_the_join_waits_before_finalizing_and_a_late_reading_is_a_join_timeout() {
    let zero_input = |gate: Arc<Gate>| {
        prepared(
            control("navigable_route", None),
            route(
                "home",
                Vec::new(),
                vec![mark("k1", "home", "count/k1")],
                Some(no_designated_effect("home")),
            ),
            Arc::new(PageNumbers {
                low_confidence_page: None,
                gate: Some(gate),
            }),
        )
    };

    // The recognition waits until the run asks for its join timeout, so it has not returned
    // when the run reaches the join.
    let gate = Arc::new(Gate::default());
    let mut device = Device::new(&[]);
    device.release_on_join = Some(Arc::clone(&gate));
    succeeded(zero_input(gate).run(&mut device));
    let frame = device.first_frame_of("home");
    assert_eq!(
        device.joined(),
        vec![(frame, vec![("resource.k1".to_owned(), 100)], Vec::new())]
    );
    let joined =
        device.position(|trace| matches!(trace, ContainedTaskTrace::ResourceReadings { .. }));
    let finalizing =
        device.position(|trace| matches!(trace, ContainedTaskTrace::Finalizing { .. }));
    assert!(joined < finalizing);

    // The recognition never returns within the run.
    let gate = Arc::new(Gate::default());
    let mut device = Device::new(&[]);
    device.join_timeout = Some(Duration::from_millis(1));
    let outcome = succeeded(zero_input(Arc::clone(&gate)).run(&mut device));
    gate.open();
    assert_eq!(outcome.final_page.as_deref(), Some("neutral/home"));
    assert_eq!(
        device.joined(),
        vec![(
            device.first_frame_of("home"),
            Vec::new(),
            vec![(
                "resource.k1".to_owned(),
                ResourceReadingProblem::JoinTimeout
            )]
        )]
    );
    assert!(device.evaluations.is_empty());
    let joined =
        device.position(|trace| matches!(trace, ContainedTaskTrace::ResourceReadings { .. }));
    let finalizing =
        device.position(|trace| matches!(trace, ContainedTaskTrace::Finalizing { .. }));
    assert!(joined < finalizing);
}

// T12 (D2M:592; §3.5; Q-D3): a business package's mark that reads below its confidence floor is
// one miss, `low_confidence`, of its own; the other mark is read and the run succeeds.
#[test]
fn d2s2_t12_a_mark_that_gives_no_reading_is_a_warning_and_the_run_succeeds() {
    let task = prepared(
        control("navigable_route", None),
        route(
            "terminal",
            vec![tap("to_terminal", "home", "terminal", 2)],
            vec![
                mark("k1", "home", "count/k1"),
                mark("k3", "terminal", "count/k3"),
            ],
            Some(no_designated_effect("terminal")),
        ),
        Arc::new(PageNumbers {
            low_confidence_page: Some("terminal"),
            gate: None,
        }),
    );
    let mut device = Device::new(&[(2, "terminal")]);

    let outcome = succeeded(task.run(&mut device));

    assert_eq!(outcome.final_page.as_deref(), Some("neutral/terminal"));
    assert_eq!(
        device.joined(),
        vec![
            (
                device.first_frame_of("home"),
                vec![("resource.k1".to_owned(), 100)],
                Vec::new()
            ),
            (
                device.first_frame_of("terminal"),
                Vec::new(),
                vec![(
                    "resource.k3".to_owned(),
                    ResourceReadingProblem::LowConfidence
                )]
            ),
        ]
    );
    let misses = device
        .traces
        .iter()
        .filter_map(|trace| match trace {
            ContainedTaskTrace::ResourceReadings { unresolved, .. } => Some(unresolved.clone()),
            _ => None,
        })
        .flatten()
        .collect::<Vec<_>>();
    let [miss] = misses.as_slice() else {
        panic!("exactly one miss: {misses:?}");
    };
    let warning = miss.outcome();
    assert_eq!(
        warning.code().as_str(),
        ExecutionKernelCode::ContainedTaskResourceReadingUnresolved.as_str()
    );
    assert_eq!(warning.value("reading_id"), Some(&json!("k3")));
    assert_eq!(warning.value("fact_key"), Some(&json!("resource.k3")));
    assert_eq!(
        warning.value("reading_problem"),
        Some(&json!("low_confidence"))
    );
}

// T13 (D2M:593; §3.4 key set): a recognition maintenance package of three segments, each from
// `home` into one marked page and back; the key set {K2} runs only the second segment and reads
// only K2; a run without a key set runs all three and reads all three.
#[test]
fn d2s2_t13_a_requested_key_set_runs_only_its_segments() {
    let segments = || {
        prepared(
            control("linear_steps", Some(6)),
            json!({
                "schema_version": "0.9",
                "task_id": "task",
                "game": "neutral",
                "server_scope": ["test"],
                "coordinate_space": {"width": WIDTH, "height": 1},
                "entry_page": "home",
                "target_page": "home",
                "max_steps": 6,
                "operations": [
                    tap("to_p1", "home", "p1", 1),
                    tap("back_from_p1", "p1", "home", 2),
                    tap("to_p2", "home", "p2", 3),
                    tap("back_from_p2", "p2", "home", 4),
                    tap("to_p3", "home", "p3", 5),
                    tap("back_from_p3", "p3", "home", 6)
                ],
                "resource_readings": [
                    mark("k1", "p1", "count/k1"),
                    mark("k2", "p2", "count/k2"),
                    mark("k3", "p3", "count/k3")
                ]
            }),
            Arc::new(PageNumbers::default()),
        )
    };
    let taps = [
        (1, "p1"),
        (2, "home"),
        (3, "p2"),
        (4, "home"),
        (5, "p3"),
        (6, "home"),
    ];

    let mut requested = Device::new(&taps);
    let outcome =
        succeeded(segments().run_for_reading_keys(&mut requested, &keys(&["resource.k2"])));
    assert_eq!(outcome.final_page.as_deref(), Some("neutral/home"));
    assert_eq!(outcome.executed_steps, 2);
    assert_eq!(requested.taps_made, [3, 4]);
    assert_eq!(requested.steps_started(), ["to_p2", "back_from_p2"]);
    assert_eq!(requested.page, "home");
    assert_eq!(
        requested.joined(),
        vec![(
            requested.first_frame_of("p2"),
            vec![("resource.k2".to_owned(), 22)],
            Vec::new()
        )]
    );

    let mut manual = Device::new(&taps);
    let outcome = succeeded(segments().run(&mut manual));
    assert_eq!(outcome.executed_steps, 6);
    assert_eq!(manual.taps_made, [1, 2, 3, 4, 5, 6]);
    assert_eq!(
        manual.joined(),
        vec![
            (
                manual.first_frame_of("p1"),
                vec![("resource.k1".to_owned(), 11)],
                Vec::new()
            ),
            (
                manual.first_frame_of("p2"),
                vec![("resource.k2".to_owned(), 22)],
                Vec::new()
            ),
            (
                manual.first_frame_of("p3"),
                vec![("resource.k3".to_owned(), 33)],
                Vec::new()
            ),
        ]
    );
}
