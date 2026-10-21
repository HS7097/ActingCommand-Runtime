// SPDX-License-Identifier: AGPL-3.0-only

// #411 B37 (slice A35-S1; design `rd-1021/train-0129/a35-design.md` §2.2 option B37-a, §5.1
// S1 T6; coordinator decision Q11 of 10-21): a `scheduling_outcome` mapping may declare
// `"busy": true`. It reads back and serializes as declared; it is refused together with a
// `remedy` and on a `designated_effect_completed` mapping (`invalid_scheduling_outcome_declaration`,
// field `busy`); a mapping without it serializes byte for byte as before.

use super::*;
use crate::outcome::Code;

fn refused_field(declared: serde_json::Value) -> (Code, &'static str) {
    let declaration: SchedulingOutcomeDeclaration =
        serde_json::from_value(declared).expect("the declaration's shape reads");
    let error = declaration
        .validate()
        .expect_err("the declaration breaks a busy rule");
    (error.code(), error.field())
}

/// T6: `busy` parses, validates, reads back and round-trips; with `remedy`, and on a
/// `designated_effect_completed` mapping, it is refused at the field `busy`; a mapping without
/// it serializes as before.
#[test]
fn b37_t6_a_busy_mapping_parses_and_is_refused_with_remedy_or_a_designated_effect() {
    let declared = serde_json::json!({
        "mappings": [
            {"outcome_key": "returned", "effect": "no_designated_effect",
             "terminal_pages": ["home"]},
            {"outcome_key": "still_running", "effect": "no_designated_effect",
             "terminal_pages": ["page_a"], "busy": true}
        ]
    });
    let declaration: SchedulingOutcomeDeclaration =
        serde_json::from_value(declared.clone()).expect("a busy mapping reads");
    declaration
        .validate()
        .expect("busy on a no_designated_effect mapping without a remedy");
    let busy = declaration
        .mappings()
        .iter()
        .map(SchedulingOutcomeMapping::busy)
        .collect::<Vec<_>>();
    assert_eq!(busy, [false, true], "each mapping reads back its own busy");
    assert_eq!(
        serde_json::to_value(&declaration).expect("declaration JSON"),
        declared,
        "the busy mapping serializes back as declared"
    );

    let refused = (
        SanitizationCode::InvalidSchedulingOutcomeDeclaration.code(),
        "busy",
    );
    assert_eq!(
        refused_field(serde_json::json!({
            "mappings": [{"outcome_key": "still_running", "effect": "no_designated_effect",
                          "terminal_pages": ["page_a"], "busy": true,
                          "remedy": {"need": "need_a", "capabilities": ["cap_x"]}}]
        })),
        refused,
        "busy together with remedy"
    );
    assert_eq!(
        refused_field(serde_json::json!({
            "designated_operation": "submit",
            "mappings": [{"outcome_key": "still_running",
                          "effect": "designated_effect_completed",
                          "terminal_pages": ["page_a"], "busy": true}]
        })),
        refused,
        "busy on a designated_effect_completed mapping"
    );

    let before = r#"{"mappings":[{"outcome_key":"returned","effect":"no_designated_effect","terminal_pages":["home"]}]}"#;
    let declaration: SchedulingOutcomeDeclaration =
        serde_json::from_str(before).expect("a mapping without busy");
    declaration.validate().expect("a valid declaration");
    assert!(!declaration.mappings()[0].busy(), "absent busy is false");
    assert_eq!(
        serde_json::to_string(&declaration).expect("declaration JSON"),
        before,
        "a mapping without busy serializes byte for byte as before"
    );
}
