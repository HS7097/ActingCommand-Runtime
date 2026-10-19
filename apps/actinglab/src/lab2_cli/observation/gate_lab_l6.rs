// SPDX-License-Identifier: AGPL-3.0-only

// #411 Lab L6 R2 (exec key 99; register §1.121; rd-1019/plan-lab-backends/plan.md §4 L6, R2, as
// the coordinator widened it at 18:4x: every kind): offline `observe` evaluates an action's NN
// target when the provider it evaluates with holds the target's model, and fills the projection's
// values (`fields`) of every target it evaluated, of every kind, as the online observation fills
// them (`online_observation.rs`): the targets of every page in page order, then each target
// `--targets` names that no page evaluated, as an optional target of no page; per target one entry
// with all its evaluations and one more per field an operation declares on it. Offline equals
// online for the same package and frame. Without OCR the existing `ocr.not_evaluated` rule
// holds, also beside a provider that serves NN models only (L6 R1). A scripted provider; packages
// and frames are synthetic and neutral.

use super::*;
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use actingcommand_lab::{ExternallyVerifiedBundle, PreparedPageObservation};
use actingcommand_recognition_pack::{
    NnProviderLabel, NnProviderRequest, NnProviderResult, OcrProviderRequest, OcrProviderResult,
    OcrProviderTextBlock, VisionProvider, VisionProviderError, VisionProviderErrorCode,
};
use std::collections::BTreeMap;
use std::io::{Cursor, Write};
use std::sync::Arc;
use zip::{ZipWriter, write::FileOptions};

const WIDTH: u32 = 64;
const HEIGHT: u32 = 32;
const ENTRY: [u8; 3] = [40, 90, 160];
const PAGE_A: [u8; 3] = [200, 60, 60];
const MODEL_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
// The left edge of `text/count`'s region; `text/stock` reads elsewhere.
const COUNT_X: i32 = 8;

// Reads "42" in `text/count`'s region and "17" in any other, at confidence 0.95. When `holds_nn`
// it holds the NN model `neutral-nn`, which gives `label` the score 0.9 and the other label 0.1.
#[derive(Debug)]
struct Scripted {
    holds_nn: bool,
    label: &'static str,
}

impl VisionProvider for Scripted {
    fn require_ocr_model(
        &self,
        _model_ref: &str,
        _model_sha256: &str,
    ) -> Result<(), VisionProviderError> {
        Ok(())
    }

    fn require_nn_model(
        &self,
        model_ref: &str,
        _model_sha256: &str,
    ) -> Result<(), VisionProviderError> {
        if self.holds_nn && model_ref == "neutral-nn" {
            Ok(())
        } else {
            Err(VisionProviderError::new(
                VisionProviderErrorCode::ModelMismatch,
                "the NN model is not installed",
            ))
        }
    }

    fn read_text(
        &self,
        request: OcrProviderRequest<'_>,
    ) -> Result<OcrProviderResult, VisionProviderError> {
        let text = if request.region.x == COUNT_X {
            "42"
        } else {
            "17"
        };
        Ok(OcrProviderResult {
            ppocr_diagnostics: Vec::new(),
            text: text.to_owned(),
            blocks: vec![OcrProviderTextBlock {
                text: text.to_owned(),
                rect: request.region,
                confidence: Some(0.95),
            }],
            confidence: Some(0.95),
            detection: None,
        })
    }

    fn classify(
        &self,
        _request: NnProviderRequest<'_>,
    ) -> Result<NnProviderResult, VisionProviderError> {
        let other = if self.label == "open" {
            "closed"
        } else {
            "open"
        };
        Ok(NnProviderResult {
            labels: vec![
                NnProviderLabel {
                    label: self.label.to_owned(),
                    score: 0.9,
                },
                NnProviderLabel {
                    label: other.to_owned(),
                    score: 0.1,
                },
            ],
        })
    }
}

fn zip_entries(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, bytes) in entries {
        zip.start_file(*path, options).expect("zip entry");
        zip.write_all(bytes).expect("zip bytes");
    }
    zip.finish().expect("finish zip").into_inner()
}

fn json_bytes(value: Value) -> Vec<u8> {
    serde_json::to_vec(&value).expect("JSON")
}

fn ocr_target(id: &str, x: i32, expected: &str) -> Value {
    json!({"type": "ocr", "id": id, "region": {"x": x, "y": 8, "width": 16, "height": 16},
           "languages": ["en"], "timeout_ms": 1000, "match_mode": "exact",
           "expected": [expected], "case_sensitive": false, "minimum_confidence": 0.8,
           "model_ref": "neutral-ocr", "model_sha256": MODEL_SHA256})
}

// The package. Its pages are told apart by the colour of the frame's top-left pixel; `entry_page`
// also requires `text/count` to read "42". The edge `open_by_kind` from `entry_page` clicks the NN
// target `model/kind`, which passes when the classifier selects `open`; the OCR target
// `text/stock` is on no page; the task declares the field `count` on `text/count`.
fn package() -> Vec<u8> {
    let page_target = |page: &str, expected: [u8; 3]| {
        json!({"type": "color", "id": format!("page/{page}"),
               "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": expected})
    };
    zip_entries(&[
        (
            "control.json",
            json_bytes(json!({
                "game": "neutral",
                "server": "test",
                "package_id": "neutral.lab_l6",
                "entry_task_id": "task"
            })),
        ),
        (
            "resources/manifest.json",
            json_bytes(json!({"entry_task_id": "task"})),
        ),
        (
            "resources/operations/task/task.json",
            json_bytes(json!({
                "task_id": "task",
                "post_admission_ocr": {"mode": "fields_v1", "fields": [
                    {"id": "count", "target_id": "text/count", "privacy": "public"}
                ]}
            })),
        ),
        (
            "resources/recognition/neutral.test.pack.json",
            json_bytes(json!({
                "schema_version": "0.7",
                "game": "neutral",
                "server": "test",
                "coordinate_space": {"width": WIDTH, "height": HEIGHT},
                "targets": [
                    page_target("entry_page", ENTRY),
                    page_target("page_a", PAGE_A),
                    ocr_target("text/count", COUNT_X, "42"),
                    ocr_target("text/stock", 40, "17"),
                    {"type": "nn", "id": "model/kind",
                     "region": {"x": 0, "y": 0, "width": 32, "height": 32},
                     "model_ref": "neutral-nn", "model_sha256": MODEL_SHA256,
                     "candidate_labels": ["open", "closed"], "minimum_score": 0.5,
                     "selection": "label", "expected_label": "open", "timeout_ms": 1000}
                ]
            })),
        ),
        (
            "resources/recognition/neutral.test.pages.json",
            json_bytes(json!({
                "schema_version": "0.6",
                "pages": [
                    {"id": "neutral/entry_page", "required": ["page/entry_page", "text/count"]},
                    {"id": "neutral/page_a", "required": ["page/page_a"]}
                ]
            })),
        ),
        (
            "resources/navigation/neutral.test.navigation.json",
            json_bytes(json!({
                "schema_version": "0.6",
                "game": "neutral",
                "server": "test",
                "navigation": [
                    {"id": "open_by_kind", "from_page": "neutral/entry_page",
                     "to_page": "neutral/page_a", "source": "",
                     "click": {"kind": "target_center", "target_id": "model/kind"}}
                ]
            })),
        ),
    ])
}

// The package, admitted as pack containment admits it, with `provider`.
fn bundle(provider: Arc<dyn VisionProvider>) -> ExternallyVerifiedBundle {
    ExternallyVerifiedBundle::load_observation("semantic_observe", &package(), Some(provider))
        .unwrap_or_else(|error| panic!("the package is admitted: {error}"))
}

fn entry_frame() -> Scene {
    Scene::from_rgb8(WIDTH, HEIGHT, &ENTRY.repeat((WIDTH * HEIGHT) as usize)).expect("scene")
}

// Offline `observe`'s projection of the entry page, built as `run_observe` builds it, with the
// evaluations of the targets `--targets` names.
fn observe_entry_page(provider: Scripted, explicit_targets: &[&str]) -> PageProjection {
    let provider: Arc<dyn VisionProvider> = Arc::new(provider);
    let bundle = bundle(Arc::clone(&provider));
    let view = crate::contained_resources::observation_resources(&bundle)
        .unwrap_or_else(|error| panic!("observe admits the package: {}", error.message));
    let (evaluator, detector) = crate::contained_resources::recognition_pipeline(&bundle)
        .unwrap_or_else(|error| panic!("recognition pipeline: {}", error.message));
    let scene = entry_frame();
    let outcome = detect_current_page(&evaluator, &detector, &scene).expect("page detection");
    assert!(outcome.matched && outcome.page == "neutral/entry_page");
    let explicit = explicit_targets
        .iter()
        .map(|target| {
            let evaluation = evaluator
                .evaluate_target(&scene, target)
                .unwrap_or_else(|error| panic!("{target}: {error}"));
            ((*target).to_owned(), evaluation)
        })
        .collect::<Vec<_>>();
    let (projection, _) = build(
        &view,
        &evaluator,
        &scene,
        &outcome,
        &LabOcr::with_provider(provider),
        &explicit,
    )
    .unwrap_or_else(|error| panic!("projection: {}", error.message));
    projection
}

// The projection entry of the action `open_by_kind`: an element, or a missing one.
fn kind_entry(projection: &PageProjection) -> &Value {
    projection
        .elements
        .iter()
        .chain(&projection.missing)
        .find(|entry| entry["resource_id"] == "open_by_kind")
        .unwrap_or_else(|| panic!("no open_by_kind entry in {projection:?}"))
}

// The NN target of an action is evaluated with the provider that holds its model: it passes when
// the classifier selects the expected label and is then recognised, with no input (an NN target
// matches no box, as a colour target); it does not pass otherwise. A provider without the model
// leaves it not evaluated, as before.
#[test]
fn l6_observe_evaluates_an_action_nn_target_with_the_provider_that_holds_its_model() {
    for (provider, recognized, basis, reason) in [
        (
            Scripted {
                holds_nn: true,
                label: "open",
            },
            true,
            "target_evaluation",
            "matched_template_rect_unavailable",
        ),
        (
            Scripted {
                holds_nn: true,
                label: "closed",
            },
            false,
            "target_evaluation",
            "target_not_matched",
        ),
        (
            Scripted {
                holds_nn: false,
                label: "open",
            },
            false,
            "not_evaluated",
            "dynamic_target_not_evaluated",
        ),
    ] {
        let case = format!("{provider:?}");
        let projection = observe_entry_page(provider, &[]);
        let entry = kind_entry(&projection);
        assert_eq!(entry["target_id"], json!("model/kind"), "{case}: {entry}");
        assert_eq!(entry["recognized"], json!(recognized), "{case}: {entry}");
        assert_eq!(entry["recognition_basis"], json!(basis), "{case}: {entry}");
        assert_eq!(entry["blocked_reason"], json!(reason), "{case}: {entry}");
        assert_eq!(entry["actionable"], json!(false), "{case}: {entry}");
        assert_eq!(entry["input"], Value::Null, "{case}: {entry}");
    }
}

// The values of every target the observation evaluated, in catalog order, as online: the colour
// targets `page/entry_page` (the first evaluation, required on the matched page) and `page/page_a`
// (the third, on the page that did not match); `text/count`, required on the matched page (the
// second evaluation, target index 1), with one entry of its evaluation and one for its declared
// field `count`; `text/stock`, named by `--targets` and on no page, after every page's targets
// (the fourth evaluation), as an optional target of no page. `text/count` named by `--targets`
// too adds no evaluation, since a page evaluated it. The NN target `model/kind`, which only the
// action resolves, is no evaluation of the observation and has no value entry.
#[test]
fn l6_observe_fills_the_values_of_every_target_it_evaluated_as_online() {
    let projection = observe_entry_page(
        Scripted {
            holds_nn: true,
            label: "open",
        },
        &["text/stock", "text/count"],
    );

    let fields = &projection.fields;
    assert_eq!(fields.len(), 5, "{fields:?}");
    let count = json!({"task_id": "task", "field_id": "count", "target_id": "text/count"});
    for (entry, target, field) in [
        (&fields[0], "page/entry_page", Value::Null),
        (&fields[1], "page/page_a", Value::Null),
        (&fields[2], "text/count", Value::Null),
        (&fields[3], "text/count", count),
        (&fields[4], "text/stock", Value::Null),
    ] {
        assert_eq!(entry["target_id"], json!(target), "{entry}");
        assert_eq!(entry["field"], field, "{entry}");
        assert_eq!(entry["parsed"], json!(false), "{entry}");
        assert_eq!(entry["redacted"], json!(false), "{entry}");
        assert_eq!(entry["privacy"], json!("public"), "{entry}");
    }
    assert_eq!(fields[2]["value"], fields[3]["value"]);
    let evaluations = |entry: &Value| {
        entry["value"]["evaluations"]
            .as_array()
            .unwrap_or_else(|| panic!("evaluations in {entry}"))
            .clone()
    };

    for (entry, index, page, passed) in [
        (&fields[0], 0, "neutral/entry_page", true),
        (&fields[1], 2, "neutral/page_a", false),
    ] {
        let colour = evaluations(entry);
        assert_eq!(colour.len(), 1, "{colour:?}");
        assert_eq!(colour[0]["evaluation_index"], json!(index));
        assert_eq!(colour[0]["page"], json!(page));
        assert_eq!(colour[0]["role"], json!("Required"));
        assert_eq!(colour[0]["target_index"], json!(0));
        assert_eq!(colour[0]["evaluation"]["kind"], json!("color"));
        assert_eq!(colour[0]["evaluation"]["passed"], json!(passed));
    }

    let count = evaluations(&fields[2]);
    assert_eq!(count.len(), 1, "{count:?}");
    assert_eq!(count[0]["evaluation_index"], json!(1));
    assert_eq!(count[0]["page"], json!("neutral/entry_page"));
    assert_eq!(count[0]["role"], json!("Required"));
    assert_eq!(count[0]["group_index"], Value::Null);
    assert_eq!(count[0]["target_index"], json!(1));
    assert_eq!(count[0]["evaluation"]["id"], json!("text/count"));
    assert_eq!(count[0]["evaluation"]["passed"], json!(true));
    assert_eq!(count[0]["evaluation"]["ocr"]["text"], json!("42"));

    let stock = evaluations(&fields[4]);
    assert_eq!(stock.len(), 1, "{stock:?}");
    assert_eq!(stock[0]["evaluation_index"], json!(3));
    assert_eq!(stock[0]["page"], Value::Null);
    assert_eq!(stock[0]["role"], json!("Optional"));
    assert_eq!(stock[0]["group_index"], Value::Null);
    assert_eq!(stock[0]["target_index"], json!(0));
    assert_eq!(stock[0]["evaluation"]["id"], json!("text/stock"));
    assert_eq!(stock[0]["evaluation"]["passed"], json!(true));
    assert_eq!(stock[0]["evaluation"]["ocr"]["text"], json!("17"));
}

// Offline `observe` and the online observation fill the same value entries, a colour target's next
// to the OCR targets', for the same package, frame and requested target.
#[test]
fn l6_offline_value_entries_equal_the_online_observations_for_the_same_package_and_frame() {
    let scripted = || Scripted {
        holds_nn: true,
        label: "open",
    };
    let offline = observe_entry_page(scripted(), &["text/stock"]);

    let online = PreparedPageObservation::load(
        "semantic_observe",
        &package(),
        &["text/stock".to_string()],
        Some(Arc::new(scripted())),
    )
    .unwrap_or_else(|error| panic!("the online observation admits the package: {error:?}"));
    let png = Frame::from_pixels(
        WIDTH,
        HEIGHT,
        ENTRY.repeat((WIDTH * HEIGHT) as usize),
        PixelFormat::Rgb8,
        CaptureBackendName::FixtureSimulation,
    )
    .expect("fixture frame")
    .encode_png_fast()
    .expect("fixture PNG");
    let evaluated = online
        .evaluate(
            &png,
            FrameIdentity {
                kind: shared::FrameKind::Artifact,
                sha256: "5f0c".repeat(16),
                width: WIDTH,
                height: HEIGHT,
            },
        )
        .unwrap_or_else(|error| panic!("the online observation: {error:?}"));

    let kinds = offline
        .fields
        .iter()
        .map(|entry| entry["value"]["evaluations"][0]["evaluation"]["kind"].clone())
        .collect::<Vec<_>>();
    assert!(kinds.contains(&json!("color")), "{kinds:?}");
    assert!(kinds.contains(&json!("ocr")), "{kinds:?}");
    assert_eq!(offline.fields, evaluated.projection.fields);
}

// Without OCR an action's OCR target stays not evaluated and is named for `ocr.not_evaluated`, as
// before, also when the vision root's NN models give the Lab a provider (L6 R1).
#[test]
fn l6_without_ocr_an_action_ocr_target_stays_not_evaluated_beside_nn_models() {
    let folder = tempfile::TempDir::new().expect("vision root");
    let ort = folder.path().join("ort");
    std::fs::create_dir_all(&ort).expect("ort folder");
    std::fs::write(ort.join("onnxruntime.dll"), b"").expect("runtime library");
    let model = folder.path().join("models").join("neutral-nn");
    std::fs::create_dir_all(&model).expect("NN model folder");
    std::fs::write(model.join("model.onnx"), b"").expect("classifier");
    let ocr = LabOcr::from_vision_root(folder.path());
    let provider = ocr.provider().expect("NN models give a provider");
    assert!(ocr.ocr_provider().is_none(), "OCR is unavailable");

    let bundle = bundle(provider);
    let (evaluator, _) = crate::contained_resources::recognition_pipeline(&bundle)
        .unwrap_or_else(|error| panic!("recognition pipeline: {}", error.message));
    let input = SemanticInput::TargetCenter {
        target_id: "text/stock".to_string(),
    };
    let mut resolved = BTreeMap::new();
    let resolution = resolve(&input, &evaluator, &entry_frame(), &mut resolved, &ocr)
        .expect("an action's OCR target without OCR does not fail observe");
    assert!(
        matches!(
            &resolution,
            ElementResolution::NotEvaluated {
                target_id,
                reason: NotEvaluatedReason::DynamicTarget,
            } if target_id == "text/stock"
        ),
        "{resolution:?}"
    );
    assert_eq!(
        not_evaluated_ocr_targets(&evaluator, &resolved),
        std::collections::BTreeSet::from(["text/stock".to_string()])
    );
}
