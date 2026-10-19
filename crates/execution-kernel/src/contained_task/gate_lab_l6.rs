// SPDX-License-Identifier: AGPL-3.0-only

// #411 Lab L6 R4, coordinator decision 10 of 2026-10-19 (exec key 99; register §1.121): the
// whole-package replay (`actinglab replay --frames`) reports the readings and fields its run took,
// which it ran and dropped before: `readings`, one entry per frame the run handed to recognition
// in hand-off order, with the frame's position in the sequence, the readings that held and the
// marks that gave none; `fields`, the run's fields report as the Runtime records it, each field
// its declaration classifies `personal` redacted. Neither appears when the run took none.
//
// A frame's first pixel marks the page `operator`; the provider reads the count "42" in column 1,
// in column 2 the stock, the red value of that column's pixel (so each frame shows its own
// stock), "n/a" in column 3 and the code "1234" in column 4, every read at confidence 0.75, each
// read its own invocation of one CPU stream, as a provider gives it.

use super::*;
use crate::{OfflineReplayEnd, OfflineReplayFacts, OfflineReplayResult, replay_contained_task};
use actingcommand_device::CaptureBackendName;
use actingcommand_recognition_pack::{
    FsAssetResolver, NnProviderRequest, NnProviderResult, OcrExecutionProviderKind,
    OcrProviderObservation, OcrProviderRequest, OcrProviderResult, OcrProviderTextBlock,
    VisionProviderError, VisionProviderErrorCode, load_pack_from_json_str,
};
use serde_json::{Value, json};
use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

const MODEL_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const WIDTH: u32 = 8;
const OPERATOR: [u8; 3] = [255, 0, 0];
// The stock the k-th frame of the sequence shows (from 1).
const STOCK_BASE: u8 = 10;

// `reads` numbers the provider's reads: a fields read refuses an invocation it has already taken.
#[derive(Debug, Default)]
struct ColumnText {
    reads: AtomicU64,
}

impl VisionProvider for ColumnText {
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
        let column = usize::try_from(request.region.x).expect("a column of the frame");
        let text = match column {
            1 => "42".to_owned(),
            2 => request.frame.rgb8_pixels[column * 3].to_string(),
            3 => "n/a".to_owned(),
            _ => "1234".to_owned(),
        };
        Ok(OcrProviderObservation {
            result: OcrProviderResult {
                ppocr_diagnostics: Vec::new(),
                text: text.clone(),
                blocks: vec![OcrProviderTextBlock {
                    text,
                    rect: request.region,
                    confidence: Some(0.75),
                }],
                confidence: Some(0.75),
                detection: None,
            },
            execution: Some(OcrProviderExecutionEvidence {
                invocation_id: format!("replay-ocr-{}", self.reads.fetch_add(1, Ordering::Relaxed)),
                session_id: "replay-session".to_owned(),
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

fn ocr_target(id: &str, column: i32) -> Value {
    json!({
        "type": "ocr", "id": id, "region": {"x": column, "y": 0, "width": 1, "height": 1},
        "languages": ["en"], "timeout_ms": 1000, "match_mode": "exact", "expected": ["unused"],
        "case_sensitive": false, "minimum_confidence": 0.5, "model_ref": "neutral-ocr",
        "model_sha256": MODEL_SHA256
    })
}

fn reading(id: &str, target_id: &str) -> Value {
    json!({
        "id": id, "fact_key": format!("resource.sample_{id}"), "page_id": "operator",
        "target_id": target_id, "trim": "whitespace_v1",
        "value": {"type": "unsigned_integer", "min": 0, "max": 1000},
        "minimum_confidence_milli": 500, "valid_for_ms": 60000
    })
}

// A zero-input fields task (operation schema 0.8) whose page `operator` is its target, fields and
// terminal page: the public field `count` and the personal field `code`; with `readings`, it reads
// the marks `stock` (column 2) and `spare` (column 3) on that page.
fn fields_task(readings: bool) -> PreparedContainedTask {
    let field = |id: &str, target_id: &str, privacy: &str| {
        json!({"id": id, "group": "snapshot", "target_id": target_id, "required": true,
               "privacy": privacy, "trim": "whitespace_v1",
               "value": {"type": "unsigned_integer", "min": 0, "max": 100000}})
    };
    let declaration = json!({
        "mode": "fields_v1", "page_ids": ["operator"],
        "fields": [field("count", "text/count", "public"), field("code", "text/code", "personal")],
        "limits": {"max_frames": 1, "max_items": 8, "max_string_bytes": 64,
                   "max_total_bytes": 4096, "max_truth_entries": 8},
        "outcome_key": "fields_recorded"
    });
    let mut program = json!({
        "schema_version": "0.8", "task_id": "task", "game": "neutral", "server_scope": ["test"],
        "coordinate_space": {"width": WIDTH, "height": 1}, "target_page": "operator",
        "timeout_ms": 1000, "post_admission_ocr": declaration,
        "scheduling_outcome": {"mappings": [{"outcome_key": "fields_recorded",
            "effect": "no_designated_effect", "terminal_pages": ["operator"]}]},
        "operations": []
    });
    if readings {
        program["resource_readings"] = json!([
            reading("stock", "text/stock"),
            reading("spare", "text/spare")
        ]);
    }
    let program: TaskProgram = serde_json::from_value(program).expect("task program");
    let fields = PreparedOcrFields {
        declaration: serde_json::from_value(declaration).expect("fields declaration"),
        dictionaries: BTreeMap::new(),
    };
    fields.declaration.validate().expect("fields declaration");
    let pack = load_pack_from_json_str(
        &json!({
            "schema_version": "0.7",
            "game": "neutral",
            "server": "test",
            "coordinate_space": {"width": WIDTH, "height": 1},
            "defaults": {"color_max_distance": 0.0},
            "targets": [
                {"type": "color", "id": "page/operator",
                 "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": OPERATOR},
                ocr_target("text/count", 1),
                ocr_target("text/stock", 2),
                ocr_target("text/spare", 3),
                ocr_target("text/code", 4)
            ]
        })
        .to_string(),
    )
    .expect("recognition pack");
    let evaluator = RecognitionEvaluator::with_vision_provider(
        pack,
        Arc::new(FsAssetResolver::new(PathBuf::new())),
        Arc::new(ColumnText::default()),
    )
    .expect("recognition evaluator");
    let pages: PageSet = serde_json::from_value(json!({"schema_version": "0.6", "pages": [
        {"id": "neutral/operator", "required": ["page/operator"]}
    ]}))
    .expect("page set");
    let control: TaskControl = serde_json::from_value(json!({
        "schema_version": CONTROL_SCHEMA,
        "package_id": "neutral.replay.fields",
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": WIDTH, "height": 1},
        "entry_task_id": "task",
        "capture_interval_ms": 1,
        "timeout_ms": 1000
    }))
    .expect("task control");
    PreparedContainedTask {
        control,
        scheduling_outcome: program.scheduling_outcome.clone(),
        program,
        evaluator,
        detector: PageDetector::new(pages).expect("page detector"),
        entry_page: None,
        post_admission_ocr: None,
        post_admission_fields: Some(fields),
        entry_count: 1,
        task_count: 1,
    }
}

// Eight frames of the page `operator`; the k-th shows the stock `STOCK_BASE + k`.
fn frames() -> Vec<Frame> {
    (1..=8u8)
        .map(|position| {
            let mut pixels = vec![0_u8; WIDTH as usize * 3];
            pixels[..3].copy_from_slice(&OPERATOR);
            pixels[6] = STOCK_BASE + position;
            Frame::from_pixels(
                WIDTH,
                1,
                pixels,
                PixelFormat::Rgb8,
                CaptureBackendName::FixtureSimulation,
            )
            .expect("fixture frame")
        })
        .collect()
}

fn replay(task: &PreparedContainedTask) -> (OfflineReplayResult, Value) {
    let result = replay_contained_task(
        task,
        frames().into_iter().map(Ok::<_, Infallible>),
        OfflineReplayFacts::Empty {
            now_unix_ms: 1_000_000,
        },
    )
    .expect("the replay gives a result");
    assert!(
        matches!(result.end, OfflineReplayEnd::Completed { .. }),
        "{:?}",
        result.end
    );
    let output = serde_json::to_value(&result).expect("the replay's output");
    (result, output)
}

#[test]
fn l6_replay_reports_the_readings_and_fields_its_run_took() {
    let (result, output) = replay(&fields_task(true));

    // One frame went to recognition: `stock` held with the stock that frame shows, at the read's
    // confidence; `spare` read no integer.
    let readings = output["readings"]
        .as_array()
        .unwrap_or_else(|| panic!("readings in {output}"));
    assert_eq!(readings.len(), 1, "{output}");
    let frame = readings[0]["frame"]
        .as_u64()
        .unwrap_or_else(|| panic!("the frame's position in {output}"));
    let frames_read = u64::try_from(result.frames_read).expect("a frame count");
    assert!(
        (1..=frames_read).contains(&frame),
        "frame {frame} of {} read",
        result.frames_read
    );
    assert_eq!(
        readings[0]["readings"],
        json!([{"reading_id": "stock", "fact_key": "resource.sample_stock",
                "value": u64::from(STOCK_BASE) + frame, "confidence_milli": 750}])
    );
    assert_eq!(
        readings[0]["unresolved"],
        json!([{"reading_id": "spare", "fact_key": "resource.sample_spare",
                "reading_problem": "invalid_integer"}])
    );

    // The fields report: the public `count` with its text and value, the personal `code`
    // redacted.
    let fields = &output["fields"];
    assert_eq!(
        fields["schema_version"],
        json!("actingcommand.runtime.post-admission-ocr-fields.v1")
    );
    assert_eq!(fields["frames_collected"], json!(1), "{fields}");
    let results = fields["records"][0]["fields"]
        .as_array()
        .unwrap_or_else(|| panic!("the field results in {fields}"));
    let field = |id: &str| {
        results
            .iter()
            .find(|result| result["field_id"] == id)
            .unwrap_or_else(|| panic!("field {id} in {fields}"))
    };
    let count = field("count");
    assert_eq!(count["raw_text"], json!("42"), "{count}");
    assert_eq!(
        count["value"],
        json!({"type": "unsigned_integer", "value": 42}),
        "{count}"
    );
    assert_eq!(count["redacted"], json!(false), "{count}");
    let code = field("code");
    for member in ["raw_text", "normalized_text", "value", "detail"] {
        assert_eq!(code[member], Value::Null, "{member}: {code}");
    }
    assert!(code.get("extraction").is_none(), "{code}");
    assert_eq!(code["redacted"], json!(true), "{code}");

    // A run that hands no frame to recognition reports no readings.
    let (_, output) = replay(&fields_task(false));
    assert!(output.get("readings").is_none(), "{output}");
    assert!(output.get("fields").is_some(), "{output}");
}
