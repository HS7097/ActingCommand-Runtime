// SPDX-License-Identifier: AGPL-3.0-only

// #411 Lab L6 R1 (exec key 99; register §1.121; rd-1019/plan-lab-backends/plan.md §4 L6, R1): a
// vision root whose model folders hold NN models and no OCR model gives the offline tools a
// provider, as the Runtime accepts such a root. The provider serves the root's NN models, so NN
// targets evaluate with it, while OCR targets answer `lab_ocr_unavailable` caused by
// `lab_ocr_model_missing`, as before. Synthetic folders with empty model files: no model is
// loaded and no ONNX Runtime is initialised.

use super::*;
use actingcommand_contract::LabErrorClass;
use actingcommand_recognition_pack::load_pack_from_json_str;
use serde_json::json;
use std::fs;
use tempfile::TempDir;

const MODEL_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

// A vision root with `ort\onnxruntime.dll` and one NN model folder, `neutral-nn\model.onnx`,
// and no OCR model folder.
fn nn_only_vision_root() -> TempDir {
    let root = TempDir::new().expect("vision root");
    let ort = root.path().join("ort");
    fs::create_dir_all(&ort).expect("ort folder");
    fs::write(ort.join("onnxruntime.dll"), b"").expect("runtime library");
    let model = root.path().join("models").join("neutral-nn");
    fs::create_dir_all(&model).expect("NN model folder");
    fs::write(model.join("model.onnx"), b"").expect("classifier");
    root
}

// An NN target on the root's NN model and an OCR target.
fn pack() -> RecognitionPack {
    load_pack_from_json_str(
        &json!({
            "schema_version": "0.7",
            "coordinate_space": {"width": 64, "height": 32},
            "targets": [
                {"type": "nn", "id": "label/kind", "region": {"x": 0, "y": 0, "width": 32, "height": 32},
                 "model_ref": "neutral-nn", "model_sha256": MODEL_SHA256,
                 "candidate_labels": ["open", "closed"], "selection": "best",
                 "minimum_score": 0.5, "timeout_ms": 1000},
                {"type": "ocr", "id": "label/title", "region": {"x": 32, "y": 8, "width": 32, "height": 16},
                 "languages": ["en"], "timeout_ms": 1000, "match_mode": "exact", "expected": ["Start"],
                 "case_sensitive": false, "minimum_confidence": 0.8, "model_ref": "neutral-ocr",
                 "model_sha256": MODEL_SHA256}
            ]
        })
        .to_string(),
    )
    .expect("the neutral pack")
}

#[test]
fn l6_a_vision_root_with_nn_models_and_no_ocr_model_gives_a_provider_for_nn_targets() {
    let folder = nn_only_vision_root();
    let root = fs::canonicalize(folder.path()).expect("canonical root");

    let ocr = LabOcr::from_vision_root(folder.path());

    // The provider every offline evaluation is given serves the root's NN model, as the
    // Runtime's provider over the same listing does.
    let provider = ocr
        .provider()
        .expect("NN models give the offline tools a provider");
    provider
        .require_nn_model("neutral-nn", MODEL_SHA256)
        .expect("the listed NN model");
    assert_eq!(ocr.vision_root(), Some(root.as_path()));
    ocr.require(&pack(), ["label/kind"])
        .expect("an NN target needs no OCR model");

    // OCR stays unavailable, as before: `lab_ocr_unavailable` on the vision root, caused by
    // `lab_ocr_model_missing` on its `models` folder.
    assert!(ocr.ocr_provider().is_none(), "no provider reads text");
    let outcome = ocr.unavailable_outcome().expect("OCR is unavailable");
    assert!(outcome.is(LabCode::LabOcrUnavailable), "{outcome}");
    assert_eq!(outcome.value("path"), Some(&json!(root.to_string_lossy())));
    assert_eq!(outcome.causes().len(), 1, "{outcome}");
    let cause = outcome.causes()[0].outcome();
    assert!(cause.is(LabCode::LabOcrModelMissing), "{outcome}");
    assert_eq!(
        cause.value("path"),
        Some(&json!(root.join("models").to_string_lossy()))
    );

    let error = ocr
        .require(&pack(), ["label/title"])
        .expect_err("an OCR target needs the OCR model");
    assert_eq!(error.code, "lab_ocr_unavailable");
    assert_eq!(error.class, LabErrorClass::SafetyBlocked);
    assert_eq!(error.blocked_by, vec!["ocr_model".to_string()]);
    let refused = &error.details.as_ref().expect("the outcome")["outcome"];
    assert_eq!(refused["code"], "lab_ocr_unavailable");
    assert_eq!(refused["values"]["target_id"], "label/title");
    assert_eq!(refused["causes"][0]["code"], "lab_ocr_model_missing");
}
