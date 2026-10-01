// SPDX-License-Identifier: AGPL-3.0-only

// one-off (to be reverted): Workflow #335 S5a K8. A package whose run takes a reading reaches
// the Host placeholder: the run fails with contained_task_resource_reading_unsupported, no
// reading fact is published and the Runtime is not poisoned (the same package without the
// readings still runs afterwards). The CI log carries the ONE-OFF-S5A lines.

use super::*;
use actingcommand_recognition_pack::{
    OcrExecutionProviderKind, OcrProviderExecutionEvidence, OcrProviderObservation,
    OcrProviderTextBlock,
};

fn report(line: impl AsRef<str>) {
    // Direct handle writes are not captured by the test harness, so they reach the CI log.
    let mut stderr = std::io::stderr().lock();
    let _ = writeln!(stderr, "ONE-OFF-S5A K8 {}", line.as_ref());
}

#[derive(Debug, Default)]
struct DigitsProvider {
    calls: AtomicU64,
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
        Ok(OcrProviderObservation {
            result: OcrProviderResult {
                ppocr_diagnostics: Vec::new(),
                text: "1,234".to_owned(),
                blocks: vec![OcrProviderTextBlock {
                    text: "1,234".to_owned(),
                    rect: request.region,
                    confidence: Some(0.99),
                }],
                confidence: Some(0.99),
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
/// fixture geometry, optional readings.
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

#[test]
fn one_off_s5a_k8_host_placeholder_refuses_taken_readings() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    // The Host's task geometry check needs the physical 16x9 fixture frame.
    state.physical_task_geometry.store(true, Ordering::Release);
    let vision = Arc::new(DigitsProvider::default());
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(
            FakeProvider::one("neutral.instance", instance_id(), Arc::clone(&state))
                .with_vision_provider(vision.clone()),
        ),
    )
    .expect("runtime host");
    let mut client = TestClient::connect(&host);
    for (label, with_readings) in [("with readings", true), ("without readings", false)] {
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
        let captures_before = state.capture_count.load(Ordering::Acquire);
        let receipt = client.send(&request);
        report(format!(
            "{label}: receipt state={:?} error={:?}",
            receipt.state(),
            receipt.error_projection()
        ));
        report(format!(
            "{label}: ocr_calls={} captures={} inputs={}",
            vision.calls.load(Ordering::Acquire) - ocr_before,
            state.capture_count.load(Ordering::Acquire) - captures_before,
            state.input_count.load(Ordering::Acquire)
        ));
        let events = host
            .query_persisted_events_for_test(EventQuery {
                correlation_id: Some(correlation_id),
                ..EventQuery::default()
            })
            .expect("events");
        report(format!(
            "{label}: event types={:?}",
            events
                .iter()
                .map(|event| event.event_type())
                .collect::<Vec<_>>()
        ));
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
            "{label}: fact.published events={} naming resource.credits={}",
            payloads
                .iter()
                .filter(|(event_type, _)| *event_type == EventType::FactPublished)
                .count(),
            payloads
                .iter()
                .filter(|(_, payload)| payload.contains("resource.credits"))
                .count()
        ));
        for (event_type, text) in &payloads {
            if let Some(position) = text.find("contained_task_resource_reading_unsupported") {
                let start = position.saturating_sub(160);
                let end = (position + 80).min(text.len());
                report(format!(
                    "{label}: {event_type:?} carries the code: ...{}...",
                    text.get(start..end).unwrap_or("<non-boundary>")
                ));
            }
        }
    }
    host.close().expect("close host");
}
