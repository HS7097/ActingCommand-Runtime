// SPDX-License-Identifier: AGPL-3.0-only

//! #411 A44 writer (0.12.x; ledger revision 4 row LR-8, register §1.153 and §1.173; host key
//! 174), T3: the host writes the verdicts a page recognition hands it into the stored
//! `task.recognition_completed` fact, which the ledger reads back.
//!
//! A synthetic `neutral` package of two pages, `page_a` and `page_b`, each one OCR read of its own
//! one-pixel cell. The scripted provider reads `ok` in both cells, `page_a`'s at 0.375 and
//! `page_b`'s at 0.875, and a read passes at 0.5 or more, so the second of the two candidate pages
//! matches. `page_b` is the package's target page, so the run ends there without a step; its
//! single entry page `page_a` is no required entry, so the run's recognition evaluates both
//! pages.

use super::*;
use actingcommand_contract::{RecognitionCandidateScore, RecognitionCandidateScores};
use actingcommand_recognition_pack::OcrProviderTextBlock;
use serde_json::{Value, json};

const ALIAS: &str = "neutral.instance";
const MODEL_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
/// The confidence the provider reads in each cell: `page_a`'s, then `page_b`'s.
const CONFIDENCES: [f32; 2] = [0.375, 0.875];

/// Reads `ok` in every cell, at the confidence [`CONFIDENCES`] scripts for its column.
#[derive(Debug)]
struct ScriptedConfidence;

impl VisionProvider for ScriptedConfidence {
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
        let cell = usize::try_from(request.region.x).expect("a cell on the frame");
        let confidence = CONFIDENCES[cell];
        Ok(OcrProviderResult {
            ppocr_diagnostics: Vec::new(),
            text: "ok".to_owned(),
            blocks: vec![OcrProviderTextBlock {
                text: "ok".to_owned(),
                rect: request.region,
                confidence: Some(confidence),
            }],
            confidence: Some(confidence),
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

/// The package: `page_a` requires `cell/a` (cell 0), `page_b` requires `cell/b` (cell 1); one tap
/// leads from `page_a`, its entry page, to `page_b`, its target page.
fn package() -> Vec<u8> {
    let ocr = |id: &str, cell: u32| {
        json!({
            "type": "ocr", "id": id,
            "region": {"x": cell, "y": 0, "width": 1, "height": 1},
            "languages": ["en"], "timeout_ms": 1000, "match_mode": "exact",
            "expected": ["ok"], "case_sensitive": false, "minimum_confidence": 0.5,
            "model_ref": "PP-OCRv6_medium", "model_sha256": MODEL_SHA256
        })
    };
    let files: [(&str, Value); 5] = [
        (
            "control.json",
            json!({
                "schema_version": "Lab-1y.control.v2",
                "package_id": "neutral.a44.task",
                "execution_mode": "navigable_route",
                "game": "neutral",
                "server": "test",
                "resolution": {"width": 16, "height": 9},
                "entry_task_id": "task",
                "capture_interval_ms": 10,
                "step_timeout_ms": 200,
                "timeout_ms": 5000,
                "max_steps": 2
            }),
        ),
        (
            "resources/manifest.json",
            json!({"schema_version": "0.3", "entry_task_id": "task"}),
        ),
        (
            "resources/operations/task/task.json",
            json!({
                "schema_version": "0.9",
                "task_id": "task",
                "game": "neutral",
                "server_scope": ["test"],
                "coordinate_space": {"width": 16, "height": 9},
                "entry_page": "page_a",
                "target_page": "page_b",
                "operations": [{
                    "id": "op_a", "from": "page_a", "to": "page_b",
                    "click": {"kind": "point", "x": 1, "y": 0},
                    "unguarded_trusted_coordinate": true, "retryable": false
                }]
            }),
        ),
        (
            "resources/recognition/neutral.test.pack.json",
            json!({
                "schema_version": "0.7",
                "game": "neutral",
                "server": "test",
                "coordinate_space": {"width": 16, "height": 9},
                "defaults": {"color_max_distance": 0.0},
                "targets": [ocr("cell/a", 0), ocr("cell/b", 1)]
            }),
        ),
        (
            "resources/recognition/neutral.test.pages.json",
            json!({
                "schema_version": "0.3",
                "pages": [
                    {"id": "neutral/page_a", "required": ["cell/a"], "optional": [], "forbidden": []},
                    {"id": "neutral/page_b", "required": ["cell/b"], "optional": [], "forbidden": []}
                ]
            }),
        ),
    ];
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, value) in files {
        zip.start_file(path, options).expect("zip entry");
        zip.write_all(&serde_json::to_vec(&value).expect("package JSON"))
            .expect("zip content");
    }
    zip.finish().expect("finish zip").into_inner()
}

/// T3 (host 174): the run's stored `task.recognition_completed` names both candidate pages and
/// `page_b` as the matched page, and carries both entries in candidate order: `page_a` not passed,
/// best 375, failed at `cell/a`; `page_b` passed, best 875; nothing omitted. The fact passed its
/// own validation when the host appended it (the run completed) and is read back from the ledger.
#[test]
fn a44_t3_the_stored_recognition_fact_carries_every_candidate_pages_verdict() {
    let root = TempDir::new().expect("tempdir");
    let package_path = root.path().join("a44-candidates.zip");
    fs::write(&package_path, package()).expect("write package");
    let state = Arc::new(FakeState::default());
    state.physical_task_geometry.store(true, Ordering::Release);
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(
            FakeProvider::one(ALIAS, instance_id(), Arc::clone(&state))
                .with_vision_provider(Arc::new(ScriptedConfidence)),
        ),
    )
    .expect("runtime host");
    let mut client = TestClient::connect(&host);
    client.set_receipt_read_timeout();
    let correlation = client.ids.mint_correlation_id().expect("correlation");
    let correlation_id = *correlation.transport();
    let request = client.request_with_correlation(
        correlation,
        RuntimeOperation::run_contained_task(
            ALIAS,
            client.ids.mint_holder_id().expect("holder"),
            ContainedTaskRequest::new(package_path.display().to_string()).expect("task request"),
        ),
    );
    let receipt = client.send(&request);
    drop(client);
    let events = host
        .query_persisted_events_for_test(EventQuery {
            correlation_id: Some(correlation_id),
            ..EventQuery::default()
        })
        .expect("the run's stored records");
    host.close().expect("close runtime host");

    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    let recognitions = events
        .iter()
        .filter_map(|event| match event.payload() {
            EventPayload::Task(TaskPayload::Semantic(payload)) => match payload.fact() {
                TaskSemanticFact::RecognitionCompleted {
                    candidate_pages,
                    matched_page,
                    candidate_scores,
                    ..
                } => Some((
                    candidate_pages.clone(),
                    matched_page.clone(),
                    candidate_scores.clone(),
                )),
                _ => None,
            },
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(
        !recognitions.is_empty(),
        "the run stored no task.recognition_completed"
    );
    let expected = RecognitionCandidateScores {
        pages: vec![
            RecognitionCandidateScore {
                page_id: "neutral/page_a".to_owned(),
                passed: false,
                best_score_milli: Some(375),
                first_failed_target_id: Some("cell/a".to_owned()),
            },
            RecognitionCandidateScore {
                page_id: "neutral/page_b".to_owned(),
                passed: true,
                best_score_milli: Some(875),
                first_failed_target_id: None,
            },
        ],
        omitted: 0,
    };
    for (candidate_pages, matched_page, candidate_scores) in recognitions {
        assert_eq!(candidate_pages, ["neutral/page_a", "neutral/page_b"]);
        assert_eq!(matched_page.as_deref(), Some("neutral/page_b"));
        assert_eq!(candidate_scores.as_deref(), Some(&expected));
    }
}
