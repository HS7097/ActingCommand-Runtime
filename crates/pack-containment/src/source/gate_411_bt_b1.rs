// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #411 B1 logic test (slice BT-B1; B-train plan `rd-1021/train-0129/b-plan/plan.md`
// §2.1 items 1 and 2, test row T4 of §5.3; register §1.163): the declaration gate admits a
// reading mark's dictionary of 32 confusion pairs and an agreement whose `k` is its `frames`,
// and refuses 33 pairs, a `from` of two characters and a mark whose `k` is below its `frames`
// with `resource_declaration_invalid` at the member; the mark's shared rule refuses the last at
// Runtime admission too.

use super::{Bundle, declaration_file_requests};
use actingcommand_contract::ResourceReadingDeclaration;
use serde_json::{Value, json};
use std::path::PathBuf;

/// One reading mark on `page_a`; `dictionary` and `agreement` are declared when given.
fn reading(dictionary: Option<Value>, agreement: Option<Value>) -> Value {
    let mut reading = json!({
        "id": "count", "fact_key": "resource.count", "page_id": "page_a",
        "target_id": "ocr/count", "trim": "whitespace_v1",
        "value": {"type": "unsigned_integer", "min": 0, "max": 999},
        "minimum_confidence_milli": 900, "valid_for_ms": 60000
    });
    if let Some(dictionary) = dictionary {
        reading["value"]["dictionary"] = dictionary;
    }
    if let Some(agreement) = agreement {
        reading["agreement"] = agreement;
    }
    reading
}

/// A task schema `0.9` task whose one reading mark is `reading`.
fn task(reading: Value) -> Bundle {
    Bundle {
        task_id: "readings".to_owned(),
        dir: PathBuf::from("tasks/readings"),
        data: json!({
            "schema_version": "0.9", "task_id": "readings", "game": "game-a",
            "server_scope": ["server-a"],
            "coordinate_space": {"width": 64, "height": 40},
            "scheduling_outcome": {"mappings": [
                {"outcome_key": "read", "effect": "no_designated_effect",
                 "terminal_pages": ["page_a"]}
            ]},
            "resource_readings": [reading],
            "operations": []
        }),
    }
}

/// `count` confusion pairs, each a distinct capital letter to the digit 0.
fn pairs(count: u32) -> Value {
    Value::Array(
        (0..count)
            .map(|index| {
                let from = char::from_u32(u32::from('A') + index).expect("a letter");
                json!([from.to_string(), "0"])
            })
            .collect(),
    )
}

#[test]
fn bt_b1_t4_too_many_pairs_a_long_from_and_k_below_frames_are_refused_at_the_member() {
    declaration_file_requests(&[task(reading(
        Some(json!({"confusions": pairs(32)})),
        Some(json!({"frames": 3, "k": 3, "interval_ms": 100})),
    ))])
    .expect("32 pairs and an agreement whose k is its frames are admitted");

    for (case, dictionary, agreement, member) in [
        (
            "33 pairs",
            Some(json!({"confusions": pairs(33)})),
            None,
            "/resource_readings/0/value/dictionary/confusions",
        ),
        (
            "a from of two characters",
            Some(json!({"confusions": [["O", "0"], ["Ol", "1"]]})),
            None,
            "/resource_readings/0/value/dictionary/confusions/1/0",
        ),
        (
            "a mark whose k is below its frames",
            None,
            Some(json!({"frames": 3, "k": 2, "interval_ms": 100})),
            "/resource_readings/0/agreement/k",
        ),
    ] {
        let Err(error) = declaration_file_requests(&[task(reading(dictionary, agreement))]) else {
            panic!("{case} is admitted");
        };
        assert_eq!(error.code, "resource_declaration_invalid", "{case}");
        let issue = error.details.expect("a declaration issue");
        assert_eq!(issue["field_path"], member, "{case}");
        assert_eq!(issue["reason"], "invalid_value", "{case}");
    }

    // The shared rule refuses the same mark at Runtime admission.
    let mark: ResourceReadingDeclaration = serde_json::from_value(reading(
        None,
        Some(json!({"frames": 3, "k": 2, "interval_ms": 100})),
    ))
    .expect("the mark parses");
    assert!(
        mark.validate().is_err(),
        "a mark writes a fact, so its k is its frames"
    );
}
