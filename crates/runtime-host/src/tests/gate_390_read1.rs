// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #390 READ-1 (forced-compromises mapping O-08, row READ-1), the host: a fraction mark
// with a second reading key writes its denominator as an instance fact of its own. The run's
// terminal page reads 35/60; the mark writes 35 as `resource.k2` and 60 as
// `resource.k2_capacity`, both read on the same frame by the same mark, so every other field of
// the two records is the same.

use super::*;

const READ1_ALIAS: &str = "neutral.instance";

// Reads 35/60 on the terminal page and 17/20 elsewhere, at confidence 0.99.
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
    ) -> Result<actingcommand_recognition_pack::OcrProviderObservation, VisionProviderError> {
        use actingcommand_recognition_pack::{
            OcrExecutionProviderKind, OcrProviderExecutionEvidence, OcrProviderObservation,
            OcrProviderTextBlock,
        };
        let terminal = request.frame.rgb8_pixels.get(..3) == Some([0_u8, 0, 255].as_slice());
        let text = String::from(if terminal { "35/60" } else { "17/20" });
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

// A guarded tap from `home` to `terminal`, whose whole page is the fraction mark `k2` with the
// second reading key `resource.k2_capacity`.
fn fraction_package() -> Vec<u8> {
    let task = serde_json::json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": 2, "height": 1},
        "entry_page": "home",
        "target_page": "terminal",
        "scheduling_outcome": {"mappings": [
            {"outcome_key": "read", "effect": "no_designated_effect",
             "terminal_pages": ["terminal"]}
        ]},
        "operations": [{
            "id": "open_terminal",
            "from": "home",
            "to": "terminal",
            "click": {"kind": "point", "x": 1, "y": 0},
            "guard": {
                "page_id": "home",
                "target_id": "guard/ready",
                "expected_rect": {"x": 1, "y": 0, "width": 1, "height": 1},
                "color_probe": "guard/ready"
            },
            "retryable": false
        }],
        "resource_readings": [{
            "id": "k2", "fact_key": "resource.k2", "capacity_fact_key": "resource.k2_capacity",
            "page_id": "terminal", "target_id": "count/page", "trim": "whitespace_v1",
            "value": {"type": "unsigned_integer", "min": 0, "max": 999,
                      "format": "current_capacity"},
            "minimum_confidence_milli": 900, "valid_for_ms": 60000
        }]
    });
    let control = serde_json::json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": "neutral.reading.task",
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": 2, "height": 1},
        "entry_task_id": "task",
        "capture_interval_ms": 1,
        "step_timeout_ms": 50,
        "timeout_ms": 5000,
        "max_steps": 2
    });
    let pack = serde_json::json!({
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
             "region": {"x": 1, "y": 0, "width": 1, "height": 1}, "expected": [0, 255, 0]},
            {"type": "ocr", "id": "count/page",
             "region": {"x": 0, "y": 0, "width": 2, "height": 1}, "languages": ["en"],
             "timeout_ms": 1000, "match_mode": "exact", "expected": ["0"],
             "case_sensitive": false, "minimum_confidence": 0.0,
             "model_ref": "PP-OCRv6_medium",
             "model_sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}
        ]
    });
    let pages = serde_json::json!({
        "schema_version": "0.3",
        "pages": [
            {"id": "neutral/home", "required": ["page/home"], "optional": [], "forbidden": []},
            {"id": "neutral/terminal", "required": ["page/terminal"], "optional": [],
             "forbidden": []}
        ]
    });
    let files = [
        ("control.json", control),
        (
            "resources/manifest.json",
            serde_json::json!({"schema_version": "0.3", "entry_task_id": "task"}),
        ),
        ("resources/operations/task/task.json", task),
        ("resources/recognition/neutral.test.pack.json", pack),
        ("resources/recognition/neutral.test.pages.json", pages),
    ];
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, contents) in files {
        zip.start_file(path, options)
            .expect("fraction package entry");
        zip.write_all(&serde_json::to_vec(&contents).expect("fraction package JSON"))
            .expect("fraction package content");
    }
    zip.finish().expect("finish fraction package").into_inner()
}

fn fraction_record(host: &RuntimeHost, key: &str) -> FactRecord {
    host.instance_fact_snapshot(InstanceFactContext {
        instance_id: READ1_ALIAS.to_owned(),
        server_id: "test".to_owned(),
        game_id: "neutral".to_owned(),
    })
    .expect("instance facts")
    .records
    .into_iter()
    .find(|record| record.key == key)
    .unwrap_or_else(|| panic!("no record of {key}"))
}

// READ-1 (FCM O-08, row READ-1 "分数格式读数同时输出分母（容量）为第二读数键"), end to end: one
// manual run publishes the terminal page's 35/60 as `resource.k2` = 35 and its denominator as
// the second key `resource.k2_capacity` = 60. Read by one mark on one frame, the second record
// carries the same observation and expiry, confidence and detector as the first; it is an
// observation of its own (register RE2), its snapshot the first's followed by `/capacity`.
#[test]
fn read1_a_run_publishes_the_denominator_under_the_second_reading_key() {
    let root = TempDir::new().expect("tempdir");
    let package = root.path().join("fraction.zip");
    let bytes = fraction_package();
    fs::write(&package, &bytes).expect("write fraction package");
    let state = Arc::new(FakeState::default());
    // A direct run needs a device instance, and a device's first frame passes the task geometry
    // only with its display observed (Workflow #392 DA-A): the fake device observes a display of
    // the package's 2x1 resolution and serves frames of that size.
    state.physical_task_geometry.store(true, Ordering::Release);
    *state
        .physical_task_extent
        .lock()
        .expect("fake physical task extent") = Some((2, 1));
    state
        .transition_capture_after_inputs
        .store(1, Ordering::Release);
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(
            FakeProvider::one(READ1_ALIAS, instance_id(), Arc::clone(&state))
                .with_vision_provider(Arc::new(ShownFractions)),
        ),
    )
    .expect("runtime host");

    let mut client = TestClient::connect(&host);
    let request = client.request(RuntimeOperation::run_contained_task(
        READ1_ALIAS,
        client.ids.mint_holder_id().expect("holder"),
        ContainedTaskRequest::new(package.display().to_string()).expect("fraction task request"),
    ));
    let receipt = client.send(&request);
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "the run's outcome: {:?}; the Runtime's fatal error: {:?}",
        receipt.outcome(),
        host.fatal_error()
    );

    let value = fraction_record(&host, "resource.k2");
    let capacity = fraction_record(&host, "resource.k2_capacity");
    assert_eq!(
        value.content,
        FactContent::Inline {
            value: ContractFactValue::Integer(35)
        }
    );
    assert_eq!(
        capacity.content,
        FactContent::Inline {
            value: ContractFactValue::Integer(60)
        }
    );
    assert_eq!(
        capacity,
        FactRecord {
            key: "resource.k2_capacity".to_owned(),
            content: capacity.content.clone(),
            source_snapshot_id: format!("{}/capacity", value.source_snapshot_id),
            ..value.clone()
        },
        "the second record differs from the first only in its key, content and snapshot"
    );
    assert_eq!(
        value.expires_at_unix_ms,
        Some(value.observed_at_unix_ms + 60_000)
    );
    drop(client);
    host.close().expect("close the host");
}
