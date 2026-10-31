// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #429 R-0b (register §1.238, since 0.12.36; contracts/component-interfaces.md, the
// `runtime-client` rows; contracts/runtime-interface.md, "Workbench request kinds", revision rule
// 3): this build fills `RuntimeInstanceStatus.activity`, which a revision-2 reader refuses (the
// status denies unknown members), so its declaration raises `runtime-client` `min` to 3: [3, 4].
// Read from the declaration this build carries in its manifests (`host.rs`'s `OWN_DECLARATION`,
// the same file), as R* and as every other Runtime party:
// - a console that speaks 3 or 4 pairs with it (`ui_meets_next` passes): [3, 3], [3, 4] (what the
//   UI's v0.12.0 and v0.12.1 declare) and [4, 4];
// - a console whose `max` is 2 does not: the existing refusal, the failed `ui_meets_next` row
//   naming both sides with their source and range, the only failed edge.

use super::manifest::{Speaks, UiDeclaration, parse_declaration};
use super::plan::{Edge, EdgeResult, ui_edges};
use serde_json::{Value, json};

/// The declaration this build copies into its Runtime and Tools manifests.
const OWN_DECLARATION: &str =
    include_str!("../../../../distribution/windows/component-interfaces.json");

fn own() -> Value {
    serde_json::from_str(OWN_DECLARATION).expect("the build's declaration is JSON")
}

fn own_speaks() -> Speaks {
    parse_declaration(&own()).expect("the build's declaration follows the parse rules")
}

/// A console that declares what this build declares, except `runtime-client` `range`.
fn console(range: [u64; 2]) -> UiDeclaration {
    let mut declared = own();
    declared["speaks"]["runtime-client"] = json!(range);
    UiDeclaration::parse(&declared).expect("a valid UI declaration")
}

fn edges_with(console_range: [u64; 2]) -> Vec<Edge> {
    let runtime = own_speaks();
    ui_edges(&runtime, &runtime, &runtime, &console(console_range))
}

fn failed(edges: &[Edge]) -> Vec<(&'static str, &'static str)> {
    edges
        .iter()
        .filter(|edge| edge.result == EdgeResult::Fail)
        .map(|edge| (edge.interface, edge.check))
        .collect()
}

#[test]
fn r0b_the_build_declares_runtime_client_three_to_four() {
    assert_eq!(own_speaks().get("runtime-client"), Some(&[3, 4]));
}

#[test]
fn r0b_a_console_at_three_or_four_pairs_and_one_whose_max_is_two_is_refused() {
    for range in [[3, 3], [3, 4], [4, 4]] {
        let edges = edges_with(range);
        assert!(
            failed(&edges).is_empty(),
            "a console at {range:?}: {edges:?}"
        );
    }
    let refused = edges_with([2, 2]);
    assert_eq!(
        failed(&refused),
        [("runtime-client", "ui_meets_next")],
        "{refused:?}"
    );
    let row = refused
        .iter()
        .find(|edge| edge.interface == "runtime-client" && edge.check == "ui_meets_next")
        .expect("the runtime-client row");
    assert_eq!(
        serde_json::to_value(row).expect("a row"),
        json!({
            "interface": "runtime-client",
            "check": "ui_meets_next",
            "result": "fail",
            "parties": [
                {"party": "U*", "source": "manifest", "range": [2, 2]},
                {"party": "R*", "source": "manifest", "range": [3, 4]},
            ],
        })
    );
}
