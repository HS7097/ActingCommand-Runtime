// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #503 LIST-D (0.12.99; register §1.544; contracts/component-interfaces.md, the `package`
// row 31; contracts/resource-verify.md, "The `package` revision"; contracts/candidate-projection.md,
// "Budgets"): a candidate layout's `provider_evaluations` is refused by every Runtime before 0.12.99
// where it reads a layout (`UnknownField` at the member), so a pack whose layout writes it takes the
// `package` revision 31, which `standard-pack build`'s revision table
// (`apps/actinglab/src/resource_verify/revision.rs`) counts, for a layout of either kind, in a pack
// of either mode, and for no other place of the name. The zip entries are built in memory as the
// table reads them; every name is made up.

use serde_json::{Value, json};
use std::collections::BTreeMap;

/// The pack that holds the fixtures' control declaration and task.
const PACK: &str = "neutral.list";

/// A zip entry: its name and its bytes.
type Entry = (String, Vec<u8>);

fn entry(name: String, value: &Value) -> Entry {
    (name, serde_json::to_vec(value).expect("entry JSON"))
}

/// The revision `content_revision` gives the zip of `entries`.
fn revision_of(entries: &[Entry]) -> u64 {
    let zip = entries.iter().cloned().collect::<BTreeMap<_, _>>();
    crate::resource_verify::revision::content_revision(&zip)
}

/// [`PACK`]'s `control.json` naming the execution mode `mode`, and its task file holding the one
/// candidate layout `layout`.
fn pack_entries(mode: &str, layout: Value) -> Vec<Entry> {
    vec![
        entry(
            format!("packs/{PACK}/control.json"),
            &json!({"execution_mode": mode}),
        ),
        entry(
            format!("packs/{PACK}/resources/operations/task/task.json"),
            &json!({"candidate_layouts": [layout]}),
        ),
    ]
}

// The row of revision 31: a layout's `provider_evaluations` needs 31, whatever it holds, on a
// layout of either kind, in a page-graph pack and in a `linear_steps` pack alike; a layout without
// it keeps its revision (3), and so does the same name under a feature or a slot.
#[test]
fn list_d_a_layouts_own_evaluation_budget_needs_revision_31_and_nothing_else_does() {
    for mode in ["navigable_route", "linear_steps"] {
        for kind in ["repeated_anchor", "fixed_slots"] {
            for written in [json!(32), json!(16), json!("any")] {
                let layout = json!({"kind": kind, "features": [{"name": "member"}],
                                    "provider_evaluations": written});
                assert_eq!(
                    revision_of(&pack_entries(mode, layout)),
                    31,
                    "{mode} {kind} {written}"
                );
            }
        }
    }
    let plain = json!({"kind": "repeated_anchor", "features": [{"name": "member"}]});
    assert_eq!(revision_of(&pack_entries("navigable_route", plain)), 3);
    let on_a_feature = json!({"kind": "repeated_anchor",
                              "features": [{"name": "member", "provider_evaluations": 32}]});
    assert_eq!(
        revision_of(&pack_entries("navigable_route", on_a_feature)),
        3,
        "the name under a feature"
    );
    let on_a_slot = json!({"kind": "fixed_slots", "features": [{"name": "member"}],
                           "slots": [{"provider_evaluations": 32}]});
    assert_eq!(
        revision_of(&pack_entries("navigable_route", on_a_slot)),
        3,
        "the name under a slot"
    );
}
