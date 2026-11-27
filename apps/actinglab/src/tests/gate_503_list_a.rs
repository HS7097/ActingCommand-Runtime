// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #503 LIST-A (0.12.90; register §1.533; contracts/component-interfaces.md, the `package`
// row 27; contracts/resource-verify.md, "The `package` revision"; contracts/resource-readings.md,
// "List readings", "A list of the wanted keys only"): a list reading's `others` is refused by every
// Runtime before 0.12.90 at its declaration gate (`UnknownField` at
// `/operations/<i>/read_list/others`), so a pack that declares it takes the `package` revision 27,
// which `standard-pack build`'s revision table (`apps/actinglab/src/resource_verify/revision.rs`)
// counts for it, in a pack of either mode, and for no other. The zip entries are built in memory as
// the table reads them; every name is made up.

use serde_json::{Value, json};
use std::collections::BTreeMap;

/// The pack that holds the fixtures' control declaration and task.
const PACK: &str = "neutral.list";

/// A zip entry: its name and its bytes.
type Entry = (String, Vec<u8>);

/// The entry `name` holding `value`.
fn entry(name: String, value: &Value) -> Entry {
    (name, serde_json::to_vec(value).expect("entry JSON"))
}

/// [`PACK`]'s `control.json`, naming the execution mode `mode`.
fn control(mode: &str) -> Entry {
    entry(
        format!("packs/{PACK}/control.json"),
        &json!({"execution_mode": mode}),
    )
}

/// [`PACK`]'s task file, its one operation `operation`.
fn task(operation: Value) -> Entry {
    entry(
        format!("packs/{PACK}/resources/operations/task/task.json"),
        &json!({"operations": [operation]}),
    )
}

/// The revision `content_revision` gives the zip of `entries`.
fn revision_of(entries: &[Entry]) -> u64 {
    let zip = entries.iter().cloned().collect::<BTreeMap<_, _>>();
    crate::resource_verify::revision::content_revision(&zip)
}

/// A list reading step, with `others` when it is given.
fn reading(others: Option<Value>) -> Value {
    let mut read = json!({"layout_id": "layout/cards", "key_feature": "member",
                          "key_column": "id", "columns": [],
                          "fact_key": "inventory.list.example_kind_a",
                          "valid_for_ms": 86_400_000, "scan": {}});
    if let Some(others) = others {
        read["others"] = others;
    }
    json!({"id": "read_members", "from": "list", "to": "list", "read_list": read})
}

// The row of revision 27: a list reading's `others` needs 27 in a page-graph pack and in a
// `linear_steps` pack alike, the word `pass_over` and a `null` alike; the same reading without it
// keeps row 8's revision in a page-graph pack and row 22's in a `linear_steps` pack.
#[test]
fn list_a_a_list_readings_others_needs_revision_27_and_nothing_else_does() {
    let page_graph = control("navigable_route");
    let linear = control("linear_steps");
    let passing_over = reading(Some(json!("pass_over")));
    assert_eq!(
        revision_of(&[page_graph.clone(), task(passing_over.clone())]),
        27
    );
    assert_eq!(revision_of(&[linear.clone(), task(passing_over)]), 27);
    assert_eq!(
        revision_of(&[page_graph.clone(), task(reading(Some(Value::Null)))]),
        27,
        "the member's name, whatever it holds"
    );
    assert_eq!(
        revision_of(&[page_graph, task(reading(None))]),
        8,
        "a page-graph list reading without the member"
    );
    assert_eq!(
        revision_of(&[linear, task(reading(None))]),
        22,
        "a linear list reading without the member"
    );
}
