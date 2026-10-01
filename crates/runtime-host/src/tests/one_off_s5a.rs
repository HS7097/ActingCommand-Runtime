// SPDX-License-Identifier: AGPL-3.0-only

// one-off (to be reverted): Workflow #335 S5a, a failed reading on a schema 0.8 fields task.
// Kernel: the fields report trace is recorded exactly once on the failure path, and a 0.8
// package without readings keeps its report on both of its paths. Host (physical 16x9 fixture
// geometry): the failed reading ends contained_task_resource_reading_unresolved without poison,
// one report envelope is stored, and the same Host then runs the readings package to the S5a
// placeholder and the package without readings to completion. ONE-OFF-S5A lines in the CI log.

use super::*;
use actingcommand_recognition_pack::{
    OcrExecutionProviderKind, OcrProviderExecutionEvidence, OcrProviderObservation,
    OcrProviderTextBlock,
};
use std::sync::Mutex;

fn report(line: impl AsRef<str>) {
    // Direct handle writes are not captured by the test harness, so they reach the CI log.
    let mut stderr = std::io::stderr().lock();
    let _ = writeln!(stderr, "ONE-OFF-S5A FIX {}", line.as_ref());
}

#[derive(Debug)]
struct DigitsProvider {
    text: Mutex<&'static str>,
    confidence: Mutex<f32>,
    calls: AtomicU64,
}

impl DigitsProvider {
    fn new() -> Self {
        Self {
            text: Mutex::new("238,334,214"),
            confidence: Mutex::new(0.97),
            calls: AtomicU64::new(0),
        }
    }

    fn set(&self, text: &'static str, confidence: f32) {
        *self.text.lock().expect("text lock") = text;
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
        let text = *self.text.lock().expect("text lock");
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

/// Schema 0.8 zero-input fields on home (the observation package shape) at the physical 16x9
/// fixture geometry, optional credits reading.
fn observation_package(with_readings: bool) -> Vec<u8> {
    let mut task = serde_json::json!({
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
                "id": "credits",
                "group": "home_snapshot",
                "target_id": "ocr/credits",
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
    if with_readings {
        task["resource_readings"] = serde_json::json!([{
            "id": "credits",
            "fact_key": "resource.credits",
            "page_id": "home",
            "target_id": "ocr/credits",
            "trim": "whitespace_v1",
            "value": {"type": "unsigned_integer", "min": 0, "max": 9007199254740991_u64, "format": "comma_grouped"},
            "minimum_confidence_milli": 900,
            "valid_for_ms": 21600000
        }]);
    }
    let control = serde_json::json!({
        "schema_version": "Lab-1y.control.v1",
        "package_id": "neutral.one-off.readings",
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
    let pack = serde_json::json!({
        "schema_version": "0.6",
        "game": "neutral",
        "server": "test",
        "coordinate_space": {"width": 16, "height": 9},
        "defaults": {"color_max_distance": 0.0},
        "targets": [
            {"type": "color", "id": "page/home", "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": [255, 0, 0]},
            {
                "type": "ocr",
                "id": "ocr/credits",
                "region": {"x": 1, "y": 0, "width": 1, "height": 1},
                "languages": ["en"],
                "timeout_ms": 1000,
                "match_mode": "contains",
                "expected": ["0"],
                "case_sensitive": true,
                "minimum_confidence": 0.0,
                "model_ref": "PP-OCRv6_medium",
                "model_sha256": "a".repeat(64)
            }
        ]
    });
    let pages = serde_json::json!({"schema_version": "0.3", "pages": [
        {"id": "neutral/home", "required": ["page/home"], "optional": [], "forbidden": []}
    ]});
    let manifest = serde_json::json!({"schema_version": "0.3", "entry_task_id": "task"});
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, value) in [
        ("control.json", &control),
        ("resources/manifest.json", &manifest),
        ("resources/operations/task/task.json", &task),
        ("resources/recognition/neutral.test.pack.json", &pack),
        ("resources/recognition/neutral.test.pages.json", &pages),
    ] {
        zip.start_file(path, options).expect("zip entry");
        zip.write_all(&serde_json::to_vec(value).expect("fixture JSON"))
            .expect("zip bytes");
    }
    zip.finish().expect("finish zip").into_inner()
}

/// Records the kernel traces of one run over a repeated home frame.
struct TraceRuntime {
    frame: Frame,
    captures: usize,
    inputs: usize,
    traces: Vec<ContainedTaskTrace>,
}

impl ContainedTaskRuntime for TraceRuntime {
    type Error = &'static str;

    fn capture(&mut self) -> Result<ObservedFrame, Self::Error> {
        self.captures += 1;
        Ok(self
            .frame
            .try_clone()
            .map_err(|_| "fixture copy failed")?
            .into())
    }

    fn input(
        &mut self,
        _action: InputAction,
        _frame: Option<InputFrameContext>,
    ) -> Result<(), Self::Error> {
        self.inputs += 1;
        Ok(())
    }

    fn record(&mut self, trace: ContainedTaskTrace) -> Result<(), Self::Error> {
        self.traces.push(trace);
        Ok(())
    }
}

fn kernel_case(label: &str, with_readings: bool, text: &'static str, confidence: f32) {
    let provider = Arc::new(DigitsProvider::new());
    provider.set(text, confidence);
    let bytes = observation_package(with_readings);
    let expected = actingcommand_pack_containment::Sha256Hash::digest(&bytes).to_string();
    let task = PreparedContainedTask::load_with_vision_provider(
        "neutral.instance",
        &bytes,
        ExternalExpectedSha256::parse_hex(&expected).expect("hash"),
        provider.clone(),
    )
    .expect("package admitted");
    let mut pixels = vec![0_u8; 16 * 9 * 3];
    pixels[..3].copy_from_slice(&[255, 0, 0]);
    let mut runtime = TraceRuntime {
        frame: Frame::from_pixels(
            16,
            9,
            pixels,
            PixelFormat::Rgb8,
            CaptureBackendName::FixtureSimulation,
        )
        .expect("fixture frame"),
        captures: 0,
        inputs: 0,
        traces: Vec::new(),
    };
    let outcome = task.run(&mut runtime);
    let names = runtime
        .traces
        .iter()
        .map(|trace| {
            format!("{trace:?}")
                .split(|character: char| !character.is_ascii_alphanumeric())
                .next()
                .unwrap_or_default()
                .to_owned()
        })
        .collect::<Vec<_>>();
    let reports = names
        .iter()
        .filter(|name| name.as_str() == "PostAdmissionOcrFields")
        .count();
    report(format!(
        "kernel {label}: outcome={outcome:?} fields_report_traces={reports} captures={} inputs={} ocr_calls={} traces={names:?}",
        runtime.captures,
        runtime.inputs,
        provider.calls.load(Ordering::Acquire)
    ));
}

#[test]
fn one_off_s5a_fix_failed_reading_keeps_one_fields_report() {
    kernel_case("readings, confidence 0.85", true, "238,334,214", 0.85);
    kernel_case("readings, confidence 0.97", true, "238,334,214", 0.97);
    kernel_case("no readings, success", false, "238,334,214", 0.97);
    kernel_case("no readings, field unparseable", false, "238334214", 0.97);

    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    // The Host's task geometry check needs the physical 16x9 fixture frame.
    state.physical_task_geometry.store(true, Ordering::Release);
    let vision = Arc::new(DigitsProvider::new());
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(
            FakeProvider::one("neutral.instance", instance_id(), Arc::clone(&state))
                .with_vision_provider(vision.clone()),
        ),
    )
    .expect("runtime host");
    let mut client = TestClient::connect(&host);
    for (label, with_readings, confidence) in [
        ("readings, confidence 0.85", true, 0.85_f32),
        ("readings, confidence 0.97", true, 0.97),
        ("no readings", false, 0.97),
    ] {
        vision.set("238,334,214", confidence);
        let bytes = observation_package(with_readings);
        let package = root.path().join(format!("observation-{with_readings}.zip"));
        fs::write(&package, &bytes).expect("write package");
        let expected = actingcommand_pack_containment::Sha256Hash::digest(&bytes).to_string();
        let correlation = client.ids.mint_correlation_id().expect("correlation");
        let correlation_id = *correlation.transport();
        let request = client.request_with_correlation(
            correlation,
            RuntimeOperation::run_contained_task(
                "neutral.instance",
                client.ids.mint_holder_id().expect("holder"),
                ContainedTaskRequest::new(package.display().to_string(), expected)
                    .expect("task request"),
            ),
        );
        let ocr_before = vision.calls.load(Ordering::Acquire);
        let receipt = client.send(&request);
        report(format!(
            "host {label}: receipt state={:?} error={:?}",
            receipt.state(),
            receipt.error_projection()
        ));
        report(format!(
            "host {label}: ocr_calls={}",
            vision.calls.load(Ordering::Acquire) - ocr_before
        ));
        let events = host
            .query_persisted_events_for_test(EventQuery {
                correlation_id: Some(correlation_id),
                ..EventQuery::default()
            })
            .expect("events");
        let payloads = events
            .iter()
            .map(|event| {
                (
                    event.event_type(),
                    serde_json::to_string(event.payload()).expect("payload JSON"),
                )
            })
            .collect::<Vec<_>>();
        report(format!(
            "host {label}: event types={:?}",
            payloads
                .iter()
                .map(|(event_type, _)| *event_type)
                .collect::<Vec<_>>()
        ));
        report(format!(
            "host {label}: fact.published events={}",
            payloads
                .iter()
                .filter(|(event_type, _)| *event_type == EventType::FactPublished)
                .count()
        ));
        for (event_type, text) in &payloads {
            if let Some(position) = text.find("\"failure_code\"") {
                let end = (position + 80).min(text.len());
                report(format!(
                    "host {label}: {event_type:?} {}",
                    text.get(position..end).unwrap_or("<non-boundary>")
                ));
            }
        }
        let projected = projected_events(
            &mut client,
            EventQuery {
                correlation_id: Some(correlation_id),
                ..EventQuery::default()
            },
        );
        let mut envelopes = 0;
        let mut unreadable = 0;
        for artifact in projected
            .iter()
            .filter(|event| event.event_type == EventType::ArtifactVerified)
            .flat_map(|event| event.artifacts.iter())
            .filter(|artifact| artifact.kind() == ArtifactKind::DiagnosticJson)
        {
            match read_projected_verified(root.path(), artifact) {
                Ok(bytes) => {
                    let document: serde_json::Value =
                        serde_json::from_slice(&bytes).unwrap_or_default();
                    if document["schema_version"]
                        == "actingcommand.runtime.post-admission-ocr-comparison-envelope.v1"
                    {
                        envelopes += 1;
                    }
                }
                Err(_) => unreadable += 1,
            }
        }
        report(format!(
            "host {label}: fields report envelopes={envelopes} unreadable_diagnostics={unreadable}"
        ));
    }
    host.close().expect("close host");
}
