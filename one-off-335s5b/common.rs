// SPDX-License-Identifier: AGPL-3.0-only

//! one-off (to be reverted): Workflow #335 S5b harness compiled on the exact merge-base 762dd274
//! and on the PR head in the same CI job. `H10|` lines (runs without readings: a zero-input
//! observation package, the same package on an unrecognized screen, a designated claim package)
//! must be equal on both builds. `BA|` lines (the observation package with two readings) show the
//! S5a placeholder on the merge-base and the publication on the head. Host path at the physical
//! 16x9 fixture geometry with the manual clock set to wall time (plus one minute, so a capture is
//! never in the Runtime clock's future).

use super::*;
use actingcommand_recognition_pack::{
    OcrExecutionProviderKind, OcrProviderExecutionEvidence, OcrProviderObservation,
    OcrProviderTextBlock,
};
use serde_json::{Value, json};
use std::sync::Mutex;

pub(super) const PRIMARY_TEXT: &str = "238,334,214";
pub(super) const SECONDARY_TEXT: &str = "1,234";
pub(super) const HOME: [u8; 3] = [255, 0, 0];
pub(super) const DONE: [u8; 3] = [0, 0, 255];
pub(super) const EMPTY: [u8; 3] = [0, 255, 0];

pub(super) fn out(tag: &str, text: impl AsRef<str>) {
    println!("{tag}|{}", text.as_ref());
}

/// OCR stub: the target at x = 1 reads `PRIMARY_TEXT`, the one at x = 2 `SECONDARY_TEXT`.
#[derive(Debug)]
pub(super) struct DigitsProvider {
    confidence: Mutex<f32>,
    pub(super) calls: AtomicU64,
}

impl DigitsProvider {
    pub(super) fn new() -> Self {
        Self {
            confidence: Mutex::new(0.97),
            calls: AtomicU64::new(0),
        }
    }

    pub(super) fn set_confidence(&self, confidence: f32) {
        *self.confidence.lock().expect("confidence lock") = confidence;
    }
}

impl VisionProvider for DigitsProvider {
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
            VisionProviderErrorCode::Internal,
            "fixture exposes OCR only",
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
        let call = self.calls.fetch_add(1, Ordering::AcqRel);
        let text = if request.region.x == 2 {
            SECONDARY_TEXT
        } else {
            PRIMARY_TEXT
        };
        let confidence = *self.confidence.lock().expect("confidence lock");
        Ok(OcrProviderObservation {
            result: OcrProviderResult {
                ppocr_diagnostics: Vec::new(),
                text: text.to_owned(),
                blocks: vec![OcrProviderTextBlock {
                    text: text.to_owned(),
                    rect: request.region,
                    confidence: Some(confidence),
                }],
                confidence: Some(confidence),
            },
            execution: Some(OcrProviderExecutionEvidence {
                invocation_id: format!("one-off-{call}"),
                session_id: "one-off-session".to_owned(),
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
            VisionProviderErrorCode::Internal,
            "fixture exposes OCR only",
        ))
    }
}

pub(super) fn ocr_target(id: &str, x: i32) -> Value {
    json!({
        "type": "ocr",
        "id": id,
        "region": {"x": x, "y": 0, "width": 1, "height": 1},
        "languages": ["en"],
        "timeout_ms": 1000,
        "match_mode": "contains",
        "expected": ["0"],
        "case_sensitive": true,
        "minimum_confidence": 0.0,
        "model_ref": "PP-OCRv6_medium",
        "model_sha256": "a".repeat(64)
    })
}

pub(super) fn color_target(id: &str, expected: [u8; 3]) -> Value {
    json!({
        "type": "color",
        "id": id,
        "region": {"x": 0, "y": 0, "width": 1, "height": 1},
        "expected": expected
    })
}

pub(super) fn page(id: &str, required: &str) -> Value {
    json!({"id": format!("neutral/{id}"), "required": [required], "optional": [], "forbidden": []})
}

pub(super) fn reading(id: &str, fact_key: &str, page: &str, target: &str) -> Value {
    json!({
        "id": id,
        "fact_key": fact_key,
        "page_id": page,
        "target_id": target,
        "trim": "whitespace_v1",
        "value": {"type": "unsigned_integer", "min": 0, "max": 9007199254740991_u64, "format": "comma_grouped"},
        "minimum_confidence_milli": 900,
        "valid_for_ms": 21600000
    })
}

pub(super) fn zip_package(
    control: &Value,
    task: &Value,
    targets: &[Value],
    pages: &[Value],
) -> Vec<u8> {
    let pack = json!({
        "schema_version": "0.6",
        "game": "neutral",
        "server": "test",
        "coordinate_space": {"width": 16, "height": 9},
        "defaults": {"color_max_distance": 0.0},
        "targets": targets
    });
    let pages = json!({"schema_version": "0.3", "pages": pages});
    let manifest = json!({"schema_version": "0.3", "entry_task_id": "task"});
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, value) in [
        ("control.json", control),
        ("resources/manifest.json", &manifest),
        ("resources/operations/task/task.json", task),
        ("resources/recognition/neutral.test.pack.json", &pack),
        ("resources/recognition/neutral.test.pages.json", &pages),
    ] {
        zip.start_file(path, options).expect("zip entry");
        zip.write_all(&serde_json::to_vec(value).expect("fixture JSON"))
            .expect("zip bytes");
    }
    zip.finish().expect("finish zip").into_inner()
}

/// Schema 0.8 zero-input fields on home (the observation package shape); `readings` adds the
/// family and the OCR targets it names.
pub(super) fn observation_package(readings: &[Value]) -> Vec<u8> {
    let mut task = json!({
        "schema_version": "0.8",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": 16, "height": 9},
        "target_page": "home",
        "scheduling_outcome": {"mappings": [
            {"outcome_key": "fields_recorded", "effect": "no_designated_effect", "terminal_pages": ["home"]}
        ]},
        "post_admission_ocr": {
            "mode": "fields_v1",
            "page_ids": ["home"],
            "fields": [{
                "id": "primary",
                "group": "home_snapshot",
                "target_id": "ocr/primary",
                "required": true,
                "privacy": "public",
                "trim": "whitespace_v1",
                "value": {"type": "unsigned_integer", "min": 0, "max": 9007199254740991_u64, "format": "comma_grouped"}
            }],
            "limits": {"max_frames": 1, "max_items": 2, "max_string_bytes": 64, "max_total_bytes": 4096, "max_truth_entries": 1},
            "outcome_key": "fields_recorded"
        },
        "operations": []
    });
    let mut targets = vec![color_target("page/home", HOME), ocr_target("ocr/primary", 1)];
    if !readings.is_empty() {
        task["resource_readings"] = Value::Array(readings.to_vec());
        if readings
            .iter()
            .any(|reading| reading["target_id"] == "ocr/secondary")
        {
            targets.push(ocr_target("ocr/secondary", 2));
        }
    }
    let control = json!({
        "schema_version": "Lab-1y.control.v1",
        "package_id": "neutral.one-off.observation",
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": 16, "height": 9},
        "entry_task_id": "task",
        "capture_interval_ms": 1,
        "step_timeout_ms": 1000,
        "timeout_ms": 5000,
        "max_steps": 1
    });
    zip_package(&control, &task, &targets, &[page("home", "page/home")])
}

/// Schema 0.9: one designated claim from home to done or empty (the after-task shape);
/// `pages` replaces the default page gates.
pub(super) fn claim_package_with_pages(readings: &[Value], pages: &[Value]) -> Vec<u8> {
    let mut task = json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": 16, "height": 9},
        "target_page": ["done", "empty"],
        "scheduling_outcome": {
            "designated_operation": "claim",
            "mappings": [
                {"outcome_key": "claimed", "effect": "designated_effect_completed", "terminal_pages": ["done", "empty"]},
                {"outcome_key": "idle", "effect": "no_designated_effect", "terminal_pages": ["done", "empty"]}
            ]
        },
        "operations": [{
            "id": "claim",
            "from": "home",
            "to": ["done", "empty"],
            "click": {"kind": "point", "x": 0, "y": 0},
            "unguarded_trusted_coordinate": true,
            "retryable": false
        }]
    });
    if !readings.is_empty() {
        task["resource_readings"] = Value::Array(readings.to_vec());
    }
    let targets = [
        color_target("page/home", HOME),
        color_target("page/done", DONE),
        color_target("page/empty", EMPTY),
        ocr_target("ocr/primary", 1),
    ];
    let control = json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": "neutral.one-off.claim",
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": 16, "height": 9},
        "entry_task_id": "task",
        "capture_interval_ms": 1,
        "step_timeout_ms": 1000,
        "timeout_ms": 5000,
        "max_steps": 2
    });
    zip_package(&control, &task, &targets, pages)
}

pub(super) fn claim_pages() -> Vec<Value> {
    vec![
        page("home", "page/home"),
        page("done", "page/done"),
        page("empty", "page/empty"),
    ]
}

pub(super) fn claim_package(readings: &[Value]) -> Vec<u8> {
    claim_package_with_pages(readings, &claim_pages())
}

pub(super) struct Bench {
    pub(super) root: TempDir,
    pub(super) state: Arc<FakeState>,
    pub(super) vision: Arc<DigitsProvider>,
    pub(super) clock: Arc<ManualRuntimeClock>,
    pub(super) instance: InstanceId,
    pub(super) host: RuntimeHost,
}

/// A host on one physical fixture instance (`fixture-instance-a`) at the 16x9 task geometry, with
/// the OCR stub and the manual clock one minute ahead of wall time.
pub(super) fn bench(edit: impl FnOnce(RuntimeHostConfig) -> RuntimeHostConfig) -> Bench {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    state.physical_task_geometry.store(true, Ordering::Release);
    let vision = Arc::new(DigitsProvider::new());
    let now = unix_ms_now().expect("wall clock") + 60_000;
    let clock = Arc::new(ManualRuntimeClock::new(now, now));
    let instance = instance_id();
    let host = RuntimeHost::start(
        edit(config(&root).with_runtime_clock(clock.clone())),
        Arc::new(
            FakeProvider::one(POLICY_INSTANCE_ALIAS, instance, Arc::clone(&state))
                .with_vision_provider(vision.clone()),
        ),
    )
    .expect("runtime host");
    Bench {
        root,
        state,
        vision,
        clock,
        instance,
        host,
    }
}

pub(super) fn write_package(bench: &Bench, name: &str, bytes: &[u8]) -> ContainedTaskRequest {
    let path = bench.root.path().join(format!("{name}.zip"));
    fs::write(&path, bytes).expect("write package");
    let sha256 = format!("{:x}", Sha256::digest(bytes));
    ContainedTaskRequest::new(path.display().to_string(), sha256).expect("task request")
}

pub(super) fn latest_sequence(host: &RuntimeHost) -> u64 {
    host.query_persisted_events_for_test(EventQuery::default())
        .expect("events")
        .iter()
        .map(PersistedEvent::sequence)
        .max()
        .unwrap_or(0)
}

pub(super) fn events_after(host: &RuntimeHost, sequence: u64) -> Vec<PersistedEvent> {
    host.query_persisted_events_for_test(EventQuery {
        from_sequence: Some(sequence + 1),
        ..EventQuery::default()
    })
    .expect("events")
}

pub(super) struct ManualRun {
    pub(super) receipt: RuntimeReceipt,
    pub(super) correlation_id: CorrelationId,
    pub(super) events: Vec<PersistedEvent>,
}

/// One manual `task-run` over IPC; the events are every event appended meanwhile.
pub(super) fn run_manual(bench: &Bench, request: ContainedTaskRequest) -> ManualRun {
    let mut client = TestClient::connect(&bench.host);
    let before = latest_sequence(&bench.host);
    let correlation = client.ids.mint_correlation_id().expect("correlation");
    let correlation_id = *correlation.transport();
    let message = client.request_with_correlation(
        correlation,
        RuntimeOperation::run_contained_task(
            POLICY_INSTANCE_ALIAS,
            client.ids.mint_holder_id().expect("holder"),
            request,
        ),
    );
    let receipt = client.send(&message);
    ManualRun {
        receipt,
        correlation_id,
        events: events_after(&bench.host, before),
    }
}

pub(super) fn receipt_text(receipt: &RuntimeReceipt) -> String {
    let result = match receipt.result() {
        Some(RuntimeResult::ContainedTaskCompleted {
            outcome,
            final_page,
            executed_steps,
            ..
        }) => format!(
            "completed outcome={outcome:?} final_page={final_page:?} executed_steps={executed_steps}"
        ),
        Some(other) => format!("result={}", serde_json::to_string(other).unwrap_or_default()),
        None => "result=none".to_owned(),
    };
    let error = receipt
        .error_projection()
        .map(|error| {
            format!(
                "code={:?} host_code={:?} host_operation={:?} fatal={}",
                error.code,
                error.host_code(),
                error.host_operation(),
                error.fatal
            )
        })
        .unwrap_or_else(|| "none".to_owned());
    format!(
        "state={:?} terminal={} {result} error={error}",
        receipt.state(),
        receipt.terminal().is_some()
    )
}

/// The run's own events (its correlation) and every `fact.published` appended meanwhile, in
/// ledger order, as event type names.
pub(super) fn sequence_text(run: &ManualRun) -> String {
    run.events
        .iter()
        .filter(|event| {
            event.links().correlation_id() == Some(&run.correlation_id)
                || event.event_type() == EventType::FactPublished
        })
        .map(|event| format!("{:?}", event.event_type()))
        .collect::<Vec<_>>()
        .join(",")
}

/// The terminal and finalizing facts of the run without timing.
pub(super) fn terminal_text(run: &ManualRun) -> Vec<String> {
    run.events
        .iter()
        .filter(|event| event.links().correlation_id() == Some(&run.correlation_id))
        .filter_map(|event| match event.payload() {
            EventPayload::Task(TaskPayload::Semantic(payload)) => match payload.fact() {
                TaskSemanticFact::Finalizing { outcome } => {
                    Some(format!("Finalizing outcome={outcome:?}"))
                }
                TaskSemanticFact::TerminalCommitted {
                    outcome,
                    final_page,
                    executed_steps,
                    failure_code,
                    scheduling_disposition,
                    ..
                } => Some(format!(
                    "TerminalCommitted outcome={outcome:?} final_page={final_page:?} executed_steps={executed_steps:?} failure_code={failure_code:?} disposition={}",
                    serde_json::to_string(scheduling_disposition).unwrap_or_default()
                )),
                TaskSemanticFact::TerminalRejected { reason, .. } => {
                    Some(format!("TerminalRejected reason={reason}"))
                }
                _ => None,
            },
            _ => None,
        })
        .collect()
}

pub(super) fn published_records(events: &[PersistedEvent]) -> Vec<(&PersistedEvent, &FactRecord)> {
    events
        .iter()
        .filter_map(|event| match event.payload() {
            EventPayload::Fact(actingcommand_contract::FactPayload::Published(payload)) => {
                Some(payload.records().map(move |record| (event, record)))
            }
            _ => None,
        })
        .flatten()
        .collect()
}

pub(super) fn report_run(tag: &str, label: &str, run: &ManualRun) {
    out(tag, format!("{label}: receipt {}", receipt_text(&run.receipt)));
    out(tag, format!("{label}: sequence {}", sequence_text(run)));
    for line in terminal_text(run) {
        out(tag, format!("{label}: {line}"));
    }
    out(
        tag,
        format!(
            "{label}: fact.published records={}",
            published_records(&run.events).len()
        ),
    );
}

#[test]
fn one_off_335s5b_common_h10_and_contrast() {
    let bench = bench(|config| config);

    let plain = write_package(&bench, "observation-plain", &observation_package(&[]));
    let run = run_manual(&bench, plain.clone());
    report_run("H10", "observation without readings", &run);

    bench.state.unknown_capture.store(true, Ordering::Release);
    let run = run_manual(&bench, plain);
    report_run("H10", "observation without readings, unrecognized screen", &run);
    bench.state.unknown_capture.store(false, Ordering::Release);

    bench
        .state
        .transition_capture_after_input
        .store(true, Ordering::Release);
    let claim = write_package(&bench, "claim-plain", &claim_package(&[]));
    let run = run_manual(&bench, claim);
    report_run("H10", "claim without readings", &run);
    bench
        .state
        .transition_capture_after_input
        .store(false, Ordering::Release);

    let readings = [
        reading("primary", "resource.primary", "home", "ocr/primary"),
        reading("secondary", "inventory.secondary", "home", "ocr/secondary"),
    ];
    let with_readings = write_package(
        &bench,
        "observation-readings",
        &observation_package(&readings),
    );
    let run = run_manual(&bench, with_readings);
    report_run("BA", "observation with two readings", &run);
    for (_, record) in published_records(&run.events) {
        out(
            "BA",
            format!(
                "observation with two readings: record key={} content={}",
                record.key,
                serde_json::to_string(&record.content).unwrap_or_default()
            ),
        );
    }
    out(
        "BA",
        format!(
            "fatal={:?}",
            bench
                .host
                .fatal_error()
                .expect("health")
                .map(|error| error.code())
        ),
    );
    bench.host.close().expect("close host");
}
