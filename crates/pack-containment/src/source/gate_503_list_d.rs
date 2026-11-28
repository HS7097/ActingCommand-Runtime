// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #503 LIST-D (0.12.99; register §1.544; contracts/selection-graph.md, "Source
// declaration"; contracts/component-interfaces.md, the `package` row 31): the declaration gate (`resource
// validate`, every parse, and admission on the sealed task) takes a candidate layout's
// `provider_evaluations`, of either kind, as `max_instances` is taken: an unsigned integer, 17 to
// 64. 16 and less, and above 64, are `InvalidValue` at the member; anything but an unsigned
// integer, `null` included, `InvalidType` there. A member the gate's list of a layout's members
// does not hold is `UnknownField` at it, which is what a gate before this slice says of
// `provider_evaluations`. The parser writes the member last, as declared; a layout without it
// derives byte for byte as before. Every name is made up.

use super::{Bundle, declaration_file_requests, derive_candidate_layouts};
use actingcommand_contract::{LabError, ResourceDeclarationIssue, ResourceDeclarationReason};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::PathBuf;

// The pointer of the member in the task's one layout.
const MEMBER_POINTER: &str = "/candidate_layouts/0/provider_evaluations";

// A task schema `0.6` task whose one candidate layout is `layout`.
fn task(layout: Value) -> Bundle {
    Bundle {
        task_id: "fixture".to_owned(),
        dir: PathBuf::from("operations/fixture"),
        data: json!({
            "schema_version": "0.6",
            "task_id": "fixture",
            "game": "fixture",
            "server_scope": ["test"],
            "coordinate_space": {"width": 96, "height": 72},
            "entry_page": "list_page",
            "operations": [],
            "candidate_layouts": [layout]
        }),
    }
}

// A `repeated_anchor` layout of rows, declaring `provider_evaluations` when given.
fn anchored(evaluations: Option<Value>) -> Value {
    let mut layout = json!({
        "id": "layout/rows", "page_id": "list_page", "kind": "repeated_anchor",
        "anchor": "ui/row_marker", "max_instances": 4, "order": "top_to_bottom",
        "suppress_iou_milli": 0,
        "instance_rect": {"x": -2, "y": -2, "width": 40, "height": 12},
        "click": {"x": 10, "y": 0, "width": 20, "height": 6},
        "readable_band": {"x": 0, "y": 0, "width": 96, "height": 40},
        "features": [{"name": "open", "value": "passed", "target": "state/row_open",
                      "offset": {"x": 30, "y": 1}}]
    });
    if let Some(evaluations) = evaluations {
        layout["provider_evaluations"] = evaluations;
    }
    layout
}

// A `fixed_slots` layout of one slot, declaring `provider_evaluations` when given.
fn slotted(evaluations: Option<Value>) -> Value {
    let mut layout = json!({
        "id": "layout/slots", "page_id": "list_page", "kind": "fixed_slots",
        "features": [{"name": "open", "value": "passed"}],
        "slots": [{"rect": {"x": 0, "y": 0, "width": 8, "height": 8},
                   "click": {"x": 0, "y": 0, "width": 8, "height": 8},
                   "targets": {"open": "state/row_open"}}]
    });
    if let Some(evaluations) = evaluations {
        layout["provider_evaluations"] = evaluations;
    }
    layout
}

// The issue of a refusal of the gate.
fn issue_of(error: LabError) -> ResourceDeclarationIssue {
    assert_eq!(error.code, "resource_declaration_invalid", "{error:?}");
    serde_json::from_value(error.details.expect("the declaration issue")).expect("issue shape")
}

// The gate refuses the task of `layout` at `pointer` for `reason`.
fn assert_refused(case: &str, layout: Value, pointer: &str, reason: ResourceDeclarationReason) {
    let error = declaration_file_requests(&[task(layout)]).expect_err(case);
    let issue = issue_of(error);
    assert_eq!(
        issue.declaration_file, "operations/fixture/task.json",
        "{case}"
    );
    assert_eq!(
        (issue.field_path.as_str(), issue.reason),
        (pointer, reason),
        "{case}"
    );
}

// Test 1 (ruling 1; the declaration gate): on a layout of either kind, 17, 32 and 64 are admitted;
// 16, 0 and 65 are `InvalidValue` at the member; a string, a fraction, a negative number and `null`
// are `InvalidType` there. Another member of a layout is `UnknownField` at it: the code a gate
// before this slice gives the member itself, whose list of a layout's members does not hold it.
#[test]
fn list_d_the_declaration_gate_takes_provider_evaluations_of_17_to_64_on_either_kind() {
    let kinds: [(&str, fn(Option<Value>) -> Value); 2] =
        [("repeated_anchor", anchored), ("fixed_slots", slotted)];
    for (kind, layout) in kinds {
        for evaluations in [17, 32, 64] {
            declaration_file_requests(&[task(layout(Some(json!(evaluations))))])
                .unwrap_or_else(|error| panic!("{kind} {evaluations}: {error:?}"));
        }
        for written in [json!(16), json!(0), json!(65)] {
            assert_refused(
                &format!("{kind} {written}"),
                layout(Some(written)),
                MEMBER_POINTER,
                ResourceDeclarationReason::InvalidValue,
            );
        }
        for written in [json!("32"), json!(32.5), json!(-1), Value::Null] {
            assert_refused(
                &format!("{kind} {written}"),
                layout(Some(written)),
                MEMBER_POINTER,
                ResourceDeclarationReason::InvalidType,
            );
        }
        let mut unknown = layout(None);
        unknown["evaluation_budget"] = json!(32);
        assert_refused(
            &format!("{kind}: another member"),
            unknown,
            "/candidate_layouts/0/evaluation_budget",
            ResourceDeclarationReason::UnknownField,
        );
    }
}

// Test 1 (ruling 5; the parser): the derived layout writes the member last, as declared, and the
// same layout without it derives byte for byte as before (the shape `tests.rs` pins for it).
#[test]
fn list_d_the_derived_layout_writes_the_member_last_and_one_without_it_as_before() {
    let targets = HashMap::from([
        (
            "ui/row_marker".to_owned(),
            json!({"type": "template", "id": "ui/row_marker"}),
        ),
        (
            "state/row_open".to_owned(),
            json!({"type": "color", "id": "state/row_open"}),
        ),
    ]);
    let derived = |layout: Value| {
        let layouts = derive_candidate_layouts(&targets, &[task(layout)], "fixture")
            .unwrap_or_else(|error| panic!("the layout derives: {error:?}"));
        serde_json::to_string(&layouts).expect("the derived JSON")
    };
    let before = concat!(
        r#"[{"id":"layout/rows","page_id":"fixture/list_page","kind":"repeated_anchor","#,
        r#""anchor":"ui/row_marker","max_instances":4,"order":"top_to_bottom","#,
        r#""suppress_iou_milli":0,"instance_rect":{"x":-2,"y":-2,"width":40,"height":12},"#,
        r#""click":{"x":10,"y":0,"width":20,"height":6},"#,
        r#""readable_band":{"x":0,"y":0,"width":96,"height":40},"#,
        r#""features":[{"name":"open","value":"passed","target":"state/row_open","#,
        r#""offset":{"x":30,"y":1}}]"#
    );
    assert_eq!(derived(anchored(None)), format!("{before}}}]"));
    assert_eq!(
        derived(anchored(Some(json!(32)))),
        format!("{before},\"provider_evaluations\":32}}]")
    );
}
