// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #453 S8 (register §1.355; contracts/package-reference.md, the answer of `standard-pack
// check`; coordinator work order rd-1114/brief-453-s7s8-vision-manifest.md): the models a package
// names are the `(model_ref, model_sha256)` of its recognition pack's OCR and NN targets, each
// once, in byte order of the reference, then of the identity; `standard-pack check` reads them
// from the package it admitted (`NavigationPackageDescriptor::recognition_models`). A package of
// a game root is a source package, and a task source declares no NN target (the source compiler
// has no NN family), so the case of an OCR target and an NN target of different models is held
// here, on a prepared package whose recognition pack declares both:
// - `s8_a_pack_with_an_ocr_and_an_nn_target_of_different_models_lists_both`: two OCR targets of
//   one model and an NN target of another give both models, each once, the NN model first by its
//   reference's bytes, in the prepared package and in the descriptor the cross-check reads.
// The pack, its pages and its task are made up; no model file exists and none is read.

use super::*;
use actingcommand_recognition_pack::RecognitionPack;
use serde_json::json;
use std::path::PathBuf;

const OCR_MODEL: &str = "neutral.ocr";
const OCR_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const NN_MODEL: &str = "neutral.classifier";
const NN_SHA256: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

/// One OCR target of the OCR model.
fn ocr(id: &str) -> serde_json::Value {
    json!({
        "type": "ocr", "id": id, "region": "full_frame", "languages": ["en"],
        "timeout_ms": 1000, "match_mode": "exact", "expected": ["7"],
        "case_sensitive": false, "minimum_confidence": 0.8,
        "model_ref": OCR_MODEL, "model_sha256": OCR_SHA256
    })
}

/// Two pages told apart by the left pixel; two OCR targets of one model and an NN target of
/// another between them.
fn pack() -> RecognitionPack {
    let colour = |id: &str, expected: [u8; 3]| {
        json!({"type": "color", "id": id,
               "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": expected})
    };
    serde_json::from_value(json!({
        "schema_version": "0.6",
        "game": "neutral",
        "server": "test",
        "coordinate_space": {"width": 2, "height": 1},
        "defaults": {"color_max_distance": 0.0},
        "targets": [
            colour("page/a", [255, 0, 0]),
            colour("page/b", [0, 0, 255]),
            ocr("text/first"),
            {
                "type": "nn", "id": "kind/first", "region": "full_frame",
                "model_ref": NN_MODEL, "model_sha256": NN_SHA256,
                "candidate_labels": ["one", "two"], "minimum_score": 0.75,
                "selection": "label", "expected_label": "one", "timeout_ms": 1000
            },
            ocr("text/second")
        ]
    }))
    .expect("recognition pack")
}

fn page_set() -> PageSet {
    serde_json::from_value(json!({
        "schema_version": "0.3",
        "pages": [
            {"id": "page_a", "required": ["page/a"]},
            {"id": "page_b", "required": ["page/b"]}
        ]
    }))
    .expect("page set")
}

/// A package with one trusted click from `page_a` toward `page_b`, its recognition pack
/// [`pack`].
fn task() -> PreparedContainedTask {
    let control: TaskControl = serde_json::from_value(json!({
        "schema_version": "Lab-1y.control.v1",
        "package_id": "neutral.s8.models",
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": 2, "height": 1},
        "entry_task_id": "task",
        "capture_interval_ms": 100,
        "step_timeout_ms": 1_000,
        "timeout_ms": 60_000,
        "max_steps": 3
    }))
    .expect("task control");
    let program: TaskProgram = serde_json::from_value(json!({
        "schema_version": "0.6",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": 2, "height": 1},
        "target_page": "page_b",
        "operations": [{
            "id": "tap_a",
            "from": "page_a",
            "click": {"kind": "point", "x": 1, "y": 0},
            "unguarded_trusted_coordinate": true,
            "retryable": false
        }]
    }))
    .expect("task program");
    for operation in &program.operations {
        operation
            .validate(&control, TaskOperationDefaults::default(), "0.6")
            .expect("an admitted operation");
    }
    let evaluator =
        RecognitionEvaluator::new(PathBuf::new(), pack()).expect("recognition evaluator");
    let detector = PageDetector::new(page_set()).expect("page detector");
    detector
        .validate(&evaluator)
        .expect("page detector targets");
    PreparedContainedTask {
        control,
        program,
        evaluator,
        detector,
        entry_page: None,
        scheduling_outcome: None,
        post_admission_ocr: None,
        post_admission_fields: None,
        entry_count: 1,
        task_count: 1,
    }
}

#[test]
fn s8_a_pack_with_an_ocr_and_an_nn_target_of_different_models_lists_both() {
    let task = task();
    let expected = vec![
        (NN_MODEL.to_owned(), NN_SHA256.to_owned()),
        (OCR_MODEL.to_owned(), OCR_SHA256.to_owned()),
    ];
    assert_eq!(
        task.recognition_models(),
        expected,
        "both models, each once, in byte order of the reference"
    );
    assert_eq!(
        task.navigation_package_descriptor().recognition_models(),
        expected.as_slice(),
        "the descriptor the standard-pack cross-check reads carries the same models"
    );
}
