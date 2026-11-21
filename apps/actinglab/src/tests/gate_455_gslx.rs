// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #455 G-SLx (0.12.78; register §1.501; contracts/component-interfaces.md, the `package`
// row 23; contracts/resource-verify.md, "The `package` revision"): a package that declares
// variables beside a capability its own `provides` marks irreversible was refused at admission by
// every Runtime before 0.12.78 (`contained_task_control_invalid`,
// `reason=variables_on_irreversible`), so it takes the `package` revision 23, which
// `standard-pack build`'s revision table (`apps/actinglab/src/resource_verify/revision.rs`)
// counts for such a package and for no other.
// The zip entries are built in memory as the table reads them; every name is made up.

use serde_json::{Value, json};
use std::collections::BTreeMap;

/// The pack that holds the fixtures' control declaration.
const PACK: &str = "neutral.vars";

/// A zip entry: its name and its bytes.
type Entry = (String, Vec<u8>);

/// The entry `name` holding `value`.
fn entry(name: String, value: &Value) -> Entry {
    (name, serde_json::to_vec(value).expect("entry JSON"))
}

/// [`PACK`]'s `control.json` holding `value`.
fn control(value: &Value) -> Entry {
    entry(format!("packs/{PACK}/control.json"), value)
}

/// The revision `content_revision` gives the zip of `entries`.
fn revision_of(entries: &[Entry]) -> u64 {
    let zip = entries.iter().cloned().collect::<BTreeMap<_, _>>();
    crate::resource_verify::revision::content_revision(&zip)
}

/// One capability with the parameter an irreversible capability needs, its `irreversible` member
/// `marked` (absent when `None`).
fn provides(marked: Option<bool>) -> Value {
    let mut capability = json!({"capability": "cap_x", "parameters": [
        {"name": "param_p", "options": ["opt_1", "opt_2"], "default": "opt_1"}]});
    if let Some(marked) = marked {
        capability["irreversible"] = json!(marked);
    }
    json!([capability])
}

/// One declared variable.
fn variables() -> Value {
    json!([{"name": "pick", "type": "switch", "title": {"en": "Pick"}, "default": false}])
}

// The row of revision 23: `variables` beside a `provides` entry marked `irreversible: true` in one
// pack's `control.json` needs 23, whatever `variables` holds (the earlier reader refused the
// member whatever it held). Beside a capability marked `false` or not marked, a pack's variables
// need 14 (row 14), and an irreversible capability without variables 4 (row 4), as before; the
// two members in two packs need no more than 14. Beside a task file that needs 21 (a shared read's
// `lines`), the zip needs 23, the highest.
#[test]
fn gslx_variables_beside_an_irreversible_capability_need_revision_23_and_nothing_else_does() {
    let irreversible_with_variables = control(&json!({
        "provides": provides(Some(true)),
        "variables": variables()
    }));
    assert_eq!(
        revision_of(std::slice::from_ref(&irreversible_with_variables)),
        23
    );
    assert_eq!(
        revision_of(&[control(&json!({
            "provides": provides(Some(true)),
            "variables": []
        }))]),
        23,
        "any variables member"
    );
    for marked in [Some(false), None] {
        assert_eq!(
            revision_of(&[control(&json!({
                "provides": provides(marked),
                "variables": variables()
            }))]),
            14,
            "{marked:?}"
        );
    }
    assert_eq!(
        revision_of(&[control(&json!({"provides": provides(Some(true))}))]),
        4
    );
    let other = entry(
        "packs/neutral.other/control.json".to_owned(),
        &json!({"variables": variables()}),
    );
    assert_eq!(
        revision_of(&[control(&json!({"provides": provides(Some(true))})), other]),
        14,
        "the two members in two packs"
    );
    let lines = entry(
        format!("packs/{PACK}/resources/operations/task/task.json"),
        &json!({"candidate_layouts": [{
            "id": "layout/names",
            "kind": "fixed_slots",
            "shared_ocr": {"lines": {"max": 2, "line_height": 21}}
        }]}),
    );
    assert_eq!(revision_of(std::slice::from_ref(&lines)), 21);
    assert_eq!(revision_of(&[irreversible_with_variables, lines]), 23);
}
