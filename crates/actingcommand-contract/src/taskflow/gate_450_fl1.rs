// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #450 FL-1a (register §1.340; `contracts/resource-readings.md`, "Lists from fields",
// "Declaration"): the one rule of a task's `field_lists`, which the declaration gate and admission
// share. A field list groups the fields of one `group` of the task's `fields_v1` declaration, keyed
// by a dictionary field, into one list. Every refusal names its member: the gate reports the
// pointer and the reason, admission the detail `<id>:<member>`. The task, the fields, the targets
// and the keys are made up (`inventory.list.example_kind_a`).

use super::*;
use crate::ResourceDeclarationReason as Reason;
use serde_json::{Value, json};
use std::collections::BTreeSet;

fn field(id: &str, group: &str, privacy: &str, value: Value) -> Value {
    json!({"id": id, "group": group, "target_id": format!("text/{id}"), "required": false,
           "privacy": privacy, "trim": "whitespace_v1", "value": value})
}

fn dictionary() -> Value {
    json!({"type": "dictionary_entry", "dictionary": {"path": "names.json"}})
}

fn integer(max: u64) -> Value {
    json!({"type": "unsigned_integer", "min": 0, "max": max})
}

// The task's fields: the group `row` reads a name, an amount, a label, a personal code and a wide
// number; the group `other` reads another name.
fn fields() -> OcrFieldsDeclaration {
    serde_json::from_value(json!({
        "mode": "fields_v1", "page_ids": ["item", "neutral/last"],
        "fields": [
            field("name", "row", "public", dictionary()),
            field("amount", "row", "public", integer(MAX_RESOURCE_READING_VALUE)),
            field("label", "row", "public", dictionary()),
            field("alias", "row", "personal", dictionary()),
            field("code", "row", "personal", integer(99)),
            field("wide", "row", "public", integer(u64::MAX)),
            field("other_name", "other", "public", dictionary())
        ],
        "limits": {"max_frames": 256, "max_items": 4096, "max_string_bytes": 256,
                   "max_total_bytes": 65536, "max_truth_entries": 64},
        "outcome_key": "fields_recorded"
    }))
    .expect("fields declaration")
}

// The fields' outcome ends on `last`; another outcome ends on `item`.
fn scheduling() -> SchedulingOutcomeDeclaration {
    serde_json::from_value(json!({"mappings": [
        {"outcome_key": "fields_recorded", "effect": "no_designated_effect",
         "terminal_pages": ["neutral/last"]},
        {"outcome_key": "stopped_early", "effect": "no_designated_effect",
         "terminal_pages": ["item"]}
    ]}))
    .expect("scheduling outcome")
}

fn list() -> Value {
    json!({
        "id": "list_a", "group": "row", "key_field": "name", "key_column": "id",
        "columns": [{"column": "count", "field": "amount"}],
        "minimum_confidence_milli": 900,
        "fact_key": "inventory.list.example_kind_a", "valid_for_ms": 86_400_000,
        "start": {"forbidden": ["mark/previous"]},
        "end": {"page_ids": ["last"]}
    })
}

fn declared(value: Value) -> FieldListDeclaration {
    serde_json::from_value(value).expect("field list declaration")
}

// The task's operation `next` and its mark `count_mark` take their ids; its mark writes
// `inventory.example_count` and its `read_list` step `inventory.list.example_kind_b`.
fn taken() -> (BTreeSet<String>, BTreeSet<String>) {
    (
        BTreeSet::from(["next".to_owned(), "count_mark".to_owned()]),
        BTreeSet::from([
            "inventory.example_count".to_owned(),
            "inventory.list.example_kind_b".to_owned(),
        ]),
    )
}

fn check(schema: &str, lists: &[FieldListDeclaration]) -> Result<(), FieldListRefusal> {
    let (fields, scheduling) = (fields(), scheduling());
    let (taken_ids, written_keys) = taken();
    validate_field_lists(
        lists,
        FieldListTask {
            game: "neutral",
            schema_version: schema,
            fields: Some(&fields),
            scheduling: Some(&scheduling),
            taken_ids: &taken_ids,
            written_keys: &written_keys,
        },
    )
}

#[test]
fn field_list_declaration_rules() {
    // The declaration reads back as written, with no member it does not declare.
    let written = list();
    let declaration = declared(written.clone());
    assert_eq!(
        serde_json::to_value(&declaration).expect("serialized"),
        written
    );
    let mut unknown = list();
    unknown["policy"] = json!({});
    assert!(serde_json::from_value::<FieldListDeclaration>(unknown).is_err());

    // Task schemas 0.8 and 0.9 declare it; so do a list without columns, a string column, pages
    // with or without the game prefix, a start on a field page and both conditions' targets.
    for schema in ["0.8", "0.9"] {
        check(schema, std::slice::from_ref(&declaration)).expect("the list is declared");
    }
    let patched = |patch: &dyn Fn(&mut Value)| {
        let mut value = list();
        patch(&mut value);
        value
    };
    let variants = [
        patched(&|list| list["columns"] = json!([])),
        patched(&|list| {
            list["columns"] = json!([{"column": "count", "field": "amount"},
                                     {"column": "label", "field": "label"}]);
        }),
        patched(&|list| {
            list["start"] = json!({"page_ids": ["neutral/item", "last"],
                                   "required": ["mark/first"], "forbidden": ["mark/previous"]});
            list["end"] = json!({"page_ids": ["neutral/last"], "required": ["mark/final"],
                                 "forbidden": ["mark/next"]});
        }),
    ];
    for variant in variants {
        check("0.9", &[declared(variant.clone())])
            .unwrap_or_else(|refusal| panic!("{variant}: {refusal:?}"));
    }

    // The rules of the task: the schema, the count, the fields and the scheduling outcome.
    let task_refusal = |member: &str, reason: Reason| FieldListRefusal {
        index: None,
        id: None,
        member: member.to_owned(),
        reason,
    };
    for schema in ["0.7", "0.6", "unknown"] {
        let refusal =
            check(schema, std::slice::from_ref(&declaration)).expect_err("an older schema");
        assert_eq!(
            refusal,
            task_refusal("field_lists", Reason::UnconsumedField)
        );
        assert_eq!(refusal.pointer(), "/field_lists");
        assert_eq!(refusal.detail(), "field_lists");
    }
    assert_eq!(
        check("0.9", &[]),
        Err(task_refusal("field_lists", Reason::InvalidValue))
    );
    let mut second = declaration.clone();
    second.id = "list_b".to_owned();
    second.fact_key = "inventory.list.example_kind_c".to_owned();
    assert_eq!(
        check("0.9", &[declaration.clone(), second]),
        Err(task_refusal("field_lists", Reason::InvalidValue))
    );
    let (fields, scheduling) = (fields(), scheduling());
    let (taken_ids, written_keys) = taken();
    let task = FieldListTask {
        game: "neutral",
        schema_version: "0.9",
        fields: Some(&fields),
        scheduling: Some(&scheduling),
        taken_ids: &taken_ids,
        written_keys: &written_keys,
    };
    let without_fields = FieldListTask {
        fields: None,
        ..task
    };
    let refusal = validate_field_lists(std::slice::from_ref(&declaration), without_fields)
        .expect_err("no fields");
    assert_eq!(
        refusal,
        task_refusal("post_admission_ocr", Reason::MissingField)
    );
    assert_eq!(refusal.pointer(), "/post_admission_ocr");
    let without_outcome = FieldListTask {
        scheduling: None,
        ..task
    };
    let refusal = validate_field_lists(std::slice::from_ref(&declaration), without_outcome)
        .expect_err("no scheduling outcome");
    assert_eq!(
        refusal,
        task_refusal("scheduling_outcome", Reason::MissingField)
    );
    assert_eq!(refusal.pointer(), "/scheduling_outcome");

    // Each rule of an entry, refused `InvalidValue` at its member.
    let columns = |count: usize| {
        json!(
            (0..count)
                .map(|index| json!({"column": format!("c{index}"), "field": "amount"}))
                .collect::<Vec<_>>()
        )
    };
    let targets = |count: usize| {
        json!(
            (0..count)
                .map(|index| format!("mark/t{index}"))
                .collect::<Vec<_>>()
        )
    };
    let cases: Vec<(Value, &str)> = vec![
        (patched(&|list| list["id"] = json!("")), "id"),
        (patched(&|list| list["id"] = json!("list a")), "id"),
        (patched(&|list| list["id"] = json!("next")), "id"),
        (patched(&|list| list["id"] = json!("count_mark")), "id"),
        (patched(&|list| list["group"] = json!("missing")), "group"),
        (
            patched(&|list| list["key_field"] = json!("amount")),
            "key_field",
        ),
        (
            patched(&|list| list["key_field"] = json!("alias")),
            "key_field",
        ),
        (
            patched(&|list| list["key_field"] = json!("other_name")),
            "key_field",
        ),
        (
            patched(&|list| list["key_field"] = json!("missing")),
            "key_field",
        ),
        (
            patched(&|list| list["key_column"] = json!("Id")),
            "key_column",
        ),
        (
            patched(&|list| list["key_column"] = json!("a".repeat(33))),
            "key_column",
        ),
        (patched(&|list| list["columns"] = columns(64)), "columns"),
        (
            patched(&|list| list["columns"][0]["column"] = json!("id")),
            "columns/0/column",
        ),
        (
            patched(&|list| {
                list["columns"] = json!([{"column": "count", "field": "amount"},
                                         {"column": "count", "field": "label"}]);
            }),
            "columns/1/column",
        ),
        (
            patched(&|list| list["columns"][0]["field"] = json!("wide")),
            "columns/0/field",
        ),
        (
            patched(&|list| list["columns"][0]["field"] = json!("code")),
            "columns/0/field",
        ),
        (
            patched(&|list| list["columns"][0]["field"] = json!("name")),
            "columns/0/field",
        ),
        (
            patched(&|list| list["columns"][0]["field"] = json!("other_name")),
            "columns/0/field",
        ),
        (
            patched(&|list| {
                list["columns"] = json!([{"column": "count", "field": "amount"},
                                         {"column": "again", "field": "amount"}]);
            }),
            "columns/1/field",
        ),
        (
            patched(&|list| list["minimum_confidence_milli"] = json!(0)),
            "minimum_confidence_milli",
        ),
        (
            patched(&|list| list["minimum_confidence_milli"] = json!(1001)),
            "minimum_confidence_milli",
        ),
        (
            patched(&|list| list["fact_key"] = json!("inventory.example_list")),
            "fact_key",
        ),
        (
            patched(&|list| list["fact_key"] = json!("inventory.list.example_kind_b")),
            "fact_key",
        ),
        (
            patched(&|list| list["valid_for_ms"] = json!(0)),
            "valid_for_ms",
        ),
        (
            patched(&|list| list["valid_for_ms"] = json!(31_536_000_001_u64)),
            "valid_for_ms",
        ),
        (patched(&|list| list["start"] = json!({})), "start"),
        (
            patched(&|list| list["start"] = json!({"required": [], "forbidden": []})),
            "start",
        ),
        (
            patched(&|list| list["start"] = json!({"page_ids": []})),
            "start/page_ids",
        ),
        (
            patched(&|list| list["start"] = json!({"page_ids": ["item", "last", "item"]})),
            "start/page_ids",
        ),
        (
            patched(&|list| list["start"] = json!({"page_ids": ["home"]})),
            "start/page_ids/0",
        ),
        (
            patched(&|list| list["start"] = json!({"page_ids": ["item", "neutral/item"]})),
            "start/page_ids/1",
        ),
        (
            patched(&|list| list["start"] = json!({"forbidden": targets(9)})),
            "start/forbidden",
        ),
        (
            patched(&|list| list["start"] = json!({"required": ["mark/a", "mark/a"]})),
            "start/required/1",
        ),
        (
            patched(&|list| list["start"] = json!({"required": ["mark a"]})),
            "start/required/0",
        ),
        (patched(&|list| list["end"] = json!({})), "end/page_ids"),
        (
            patched(&|list| list["end"] = json!({"required": ["mark/final"]})),
            "end/page_ids",
        ),
        (
            patched(&|list| list["end"] = json!({"page_ids": ["item"]})),
            "end/page_ids/0",
        ),
        (
            patched(&|list| list["end"] = json!({"page_ids": ["home"]})),
            "end/page_ids/0",
        ),
        (
            patched(&|list| list["end"] = json!({"page_ids": ["last"], "required": targets(9)})),
            "end/required",
        ),
        (
            patched(&|list| list["end"] = json!({"page_ids": ["last"], "forbidden": ["mark x"]})),
            "end/forbidden/0",
        ),
    ];
    for (value, member) in cases {
        let refusal = check("0.9", &[declared(value.clone())]).expect_err("the entry is refused");
        assert_eq!(
            refusal,
            FieldListRefusal {
                index: Some(0),
                id: Some(value["id"].as_str().unwrap_or_default().to_owned()),
                member: member.to_owned(),
                reason: Reason::InvalidValue,
            },
            "{value}"
        );
        assert_eq!(refusal.pointer(), format!("/field_lists/0/{member}"));
        assert_eq!(
            refusal.detail(),
            format!("{}:{member}", value["id"].as_str().unwrap_or_default())
        );
    }
}
