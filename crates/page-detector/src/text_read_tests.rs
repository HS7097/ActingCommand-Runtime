// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #396 REC-1: one page recognition reads located text for the whole page set, so the
//! set's distinct text reads stay within one consumer's static budget.

use super::*;
use actingcommand_contract::ResourceDeclarationReason;
use actingcommand_recognition_pack::{FsAssetResolver, load_pack_from_json_str};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;

/// A locate OCR target reading the 80 by 40 region at `x`.
fn locate(id: &str, x: i32, expected: &str) -> Value {
    json!({
        "type": "ocr", "id": id, "region": {"x": x, "y": 0, "width": 80, "height": 40},
        "languages": ["en"], "timeout_ms": 1000, "match_mode": "exact", "expected": [expected],
        "case_sensitive": false, "minimum_confidence": 0.8, "model_ref": "PP-OCRv6_medium",
        "model_sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "locate": {"normalize": "identity_v1", "blocks": "detect", "line_merge": {"gap_factor": 0}}
    })
}

fn detector(pages: Value) -> PageDetector {
    PageDetector::new(
        serde_json::from_value(json!({"schema_version": "0.6", "pages": pages})).expect("page set"),
    )
    .expect("page detector")
}

#[test]
fn a_page_set_reads_at_most_four_distinct_text_requests() {
    let pack = load_pack_from_json_str(
        &json!({
            "schema_version": "0.7",
            "coordinate_space": {"width": 640, "height": 360},
            "targets": [
                locate("text/a", 0, "ALPHA"),
                locate("text/a_twin", 0, "BETA"),
                locate("text/b", 100, "ALPHA"),
                locate("text/c", 200, "ALPHA"),
                locate("text/d", 300, "ALPHA"),
                locate("text/e", 400, "ALPHA")
            ]
        })
        .to_string(),
    )
    .expect("recognition pack");
    let evaluator = RecognitionEvaluator::with_asset_resolver(
        pack,
        Arc::new(FsAssetResolver::new(PathBuf::new())),
    )
    .expect("recognition evaluator");

    // Four distinct requests; the twin shares its request with text/a and counts once.
    detector(json!([
        {"id": "neutral/one", "required": ["text/a", "text/b", "text/a_twin"]},
        {"id": "neutral/two", "required": ["text/c", "text/d"]}
    ]))
    .validate(&evaluator)
    .expect("four distinct reads fit one page recognition");

    let error = detector(json!([
        {"id": "neutral/one", "required": ["text/a", "text/b"]},
        {"id": "neutral/two", "required": ["text/c", "text/d", "text/e"]}
    ]))
    .validate(&evaluator)
    .expect_err("a fifth distinct read exceeds one page recognition");
    let issue = error.declaration_issue().expect("the refused declaration");
    assert_eq!(
        issue.field_path, "/pages/1",
        "the page that exceeds the budget"
    );
    assert_eq!(issue.reason, ResourceDeclarationReason::InvalidValue);
}

// An ordinary OCR target reading the 80 by 40 region at `x`.
fn field(id: &str, x: i32) -> Value {
    json!({
        "type": "ocr", "id": id, "region": {"x": x, "y": 0, "width": 80, "height": 40},
        "languages": ["en"], "timeout_ms": 1000, "match_mode": "exact", "expected": ["ALPHA"],
        "case_sensitive": false, "minimum_confidence": 0.8, "model_ref": "PP-OCRv6_medium",
        "model_sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    })
}

// Workflow #396 REC-8: a field or reading target a page names is read by the page recognition
// once per target, since its observation is kept by target; other ordinary OCR targets are no
// reads.
#[test]
fn a_page_set_counts_each_kept_target_it_names_as_one_read() {
    let pack = load_pack_from_json_str(
        &json!({
            "schema_version": "0.7",
            "coordinate_space": {"width": 640, "height": 360},
            "targets": [
                field("text/a", 0),
                field("text/a_twin", 0),
                field("text/b", 100),
                field("text/c", 200),
                field("text/d", 300)
            ]
        })
        .to_string(),
    )
    .expect("recognition pack");
    let evaluator = RecognitionEvaluator::with_asset_resolver(
        pack,
        Arc::new(FsAssetResolver::new(PathBuf::new())),
    )
    .expect("recognition evaluator");
    let kept = |ids: &[&str]| {
        ids.iter()
            .map(|id| (*id).to_owned())
            .collect::<HashSet<_>>()
    };
    let pages = detector(json!([
        {"id": "neutral/one", "required": ["text/a", "text/a_twin"]},
        {"id": "neutral/two", "required": ["text/b", "text/c", "text/d"]}
    ]));

    pages
        .validate(&evaluator)
        .expect("ordinary OCR targets are no text reads");
    pages
        .validate_keeping(
            &evaluator,
            kept(&["text/a", "text/a_twin", "text/b", "text/c"]),
        )
        .expect("four kept targets fit one page recognition, the twin as its own read");

    let error = pages
        .validate_keeping(
            &evaluator,
            kept(&["text/a", "text/a_twin", "text/b", "text/c", "text/d"]),
        )
        .expect_err("a fifth kept target exceeds one page recognition");
    let issue = error.declaration_issue().expect("the refused declaration");
    assert_eq!(
        issue.field_path, "/pages/1",
        "the page that exceeds the budget"
    );
    assert_eq!(issue.reason, ResourceDeclarationReason::InvalidValue);
}
