// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #503 LIST-C (0.12.97; register §1.541; contracts/component-interfaces.md, the `package`
// row 30; contracts/resource-verify.md, "The `package` revision"; contracts/selection-graph.md, "An
// end that must also be stationary"): a scan's `end` that writes `require_stationary` is refused by
// every Runtime before 0.12.97 at its admission (its reader of `end` denies unknown members), so a
// pack that writes it takes the `package` revision 30, which `standard-pack build`'s revision table
// (`apps/actinglab/src/resource_verify/revision.rs`) counts for it, in the scan of a select step and
// of a list reading, in a pack of either mode, and for no other place of the member. The zip
// entries are built in memory as the table reads them; every name is made up.

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

/// A scan with its start and its end, the end writing `require_stationary` when it is given.
fn scan(stationary: Option<Value>) -> Value {
    let mut end = json!({"check": "check/list_end"});
    if let Some(stationary) = stationary {
        end["require_stationary"] = stationary;
    }
    json!({"start": {"check": "check/list_start", "rewind": false}, "end": end})
}

/// A select step over the scan `declared`.
fn select(declared: Value) -> Value {
    json!({"id": "pick", "from": "list", "to": "detail",
           "select": {"layout_id": "layout/cards", "policy": {"path": "policies/select.json"},
                      "scan": declared}})
}

/// A list reading over the scan `declared`.
fn reading(declared: Value) -> Value {
    json!({"id": "read_members", "from": "list", "to": "list",
           "read_list": {"layout_id": "layout/cards", "key_feature": "member",
                         "key_column": "id", "columns": [],
                         "fact_key": "inventory.list.example_kind_a",
                         "valid_for_ms": 86_400_000, "scan": declared}})
}

/// The step `step` makes of a scan, in the pack whose control is `mode`, needs 30 with the member
/// written `true` and written `false`, and `without` without it.
fn assert_row_30(case: &str, mode: &Entry, step: impl Fn(Value) -> Value, without: u64) {
    assert_eq!(
        revision_of(&[mode.clone(), task(step(scan(Some(json!(true)))))]),
        30,
        "{case}"
    );
    assert_eq!(
        revision_of(&[mode.clone(), task(step(scan(Some(json!(false)))))]),
        30,
        "{case}: the member's name, whatever it holds"
    );
    assert_eq!(
        revision_of(&[mode.clone(), task(step(scan(None)))]),
        without,
        "{case} without the member"
    );
}

// The row of revision 30: a scan's `end.require_stationary` needs 30 in a select step's scan and in
// a list reading's, in a page-graph pack and in a `linear_steps` pack alike, `true` and `false`
// alike (the member's name, whatever it holds); without it the same steps keep their revisions (a
// page-graph select 3, a linear select 13, a page-graph list reading 8, a linear one 22); the same
// name under the scan's `start` needs nothing.
#[test]
fn list_c_a_scan_ends_require_stationary_needs_revision_30_and_nothing_else_does() {
    let page_graph = control("navigable_route");
    let linear = control("linear_steps");
    assert_row_30("a page-graph select", &page_graph, select, 3);
    assert_row_30("a linear select", &linear, select, 13);
    assert_row_30("a page-graph list reading", &page_graph, reading, 8);
    assert_row_30("a linear list reading", &linear, reading, 22);
    let mut elsewhere = scan(None);
    elsewhere["start"]["require_stationary"] = json!(true);
    assert_eq!(
        revision_of(&[page_graph, task(select(elsewhere))]),
        3,
        "the name under the scan's start"
    );
}
