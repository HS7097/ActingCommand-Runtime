// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #477 LR-1 (0.12.79; register §1.513; contracts/component-interfaces.md, the `package`
// row 25; contracts/resource-verify.md, "The `package` revision"; contracts/linear-steps.md,
// "Pressing on one page"): a press step's count (`presses`) in a `linear_steps` pack is refused by
// every Runtime before 0.12.79 at its declaration gate (`UnknownField` at
// `/operations/<i>/presses`), so such a pack takes the `package` revision 25, which
// `standard-pack build`'s revision table (`apps/actinglab/src/resource_verify/revision.rs`) counts
// for it and for no other. The zip entries are built in memory as the table reads them; every name
// is made up.

use serde_json::{Value, json};
use std::collections::BTreeMap;

/// The pack that holds the fixtures' control declaration and task.
const PACK: &str = "neutral.press";

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

/// A click step that stays on its page, with `presses` when it is given.
fn press_step(presses: Option<Value>) -> Value {
    let mut step = json!({"id": "plus", "from": "list", "to": "list",
                          "click": {"kind": "point", "x": 1, "y": 0}});
    if let Some(presses) = presses {
        step["presses"] = presses;
    }
    step
}

// The row of revision 25: an operation's `presses` in a pack whose control names `linear_steps`
// needs 25, a whole number and the parameter form alike; the same click step without it stays at
// 3, and the same member in a `navigable_route` pack (refused by every build, `presses requires
// linear_steps` in this one) needs nothing either.
#[test]
fn lr1_a_press_count_of_a_linear_steps_pack_needs_revision_25_and_nothing_else_does() {
    let linear = control("linear_steps");
    let constant = press_step(Some(json!(3)));
    let parameter = press_step(Some(
        json!({"fact_key": "session.dispatch.parameter.turns", "minus": 1}),
    ));
    assert_eq!(revision_of(&[linear.clone(), task(constant.clone())]), 25);
    assert_eq!(revision_of(&[linear.clone(), task(parameter)]), 25);
    assert_eq!(
        revision_of(&[linear, task(press_step(None))]),
        3,
        "a click step without the member"
    );
    assert_eq!(
        revision_of(&[control("navigable_route"), task(constant)]),
        3,
        "the member in a page-graph pack"
    );
}
