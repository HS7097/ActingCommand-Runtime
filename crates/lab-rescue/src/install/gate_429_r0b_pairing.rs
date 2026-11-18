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
// Workflow #437 G1 fix round 2 (register §1.433, 0.12.67) raises `max` to 10 and keeps the floor:
// a console that speaks at most 9 ([3, 9], the revision before) still meets this build's [3, 10].
// Workflow #463 S3-8 (register §1.444, 0.12.68) raises `max` to 11 and keeps the floor: a console
// that speaks at most 10 ([3, 10], the revision before) still meets this build's [3, 11]. #480
// PG-2c (register §1.472, 0.12.73) raises `max` to 12 and keeps the floor: the console of today,
// pinned to v0.12.71 ([3, 11]), still meets this build's [3, 12]. #480 PG-2e (register §1.485,
// 0.12.75) raises the floor this file is about to 12: the Runtime answers the instance pause's
// `pause_mode` and `in_the_way`, which a reader below 12 refuses, so [12, 12]; a console that reads
// 12 pairs, one whose `max` is below 12 fails `ui_meets_next` alone (the second test, renamed).

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
    // Workflow #432 AGT-1b (register §1.246) raises `max` to 5, #407 PERF-READ (§1.280) to 6,
    // #428 S168-LB1a (§1.311) to 7, #434 LAB-6S1a (§1.287) to 8, #463 V3 (§1.383) to 9, #437
    // G1 fix round 2 (§1.433) to 10, #463 S3-8 (§1.444) to 11 and #480 PG-2c (§1.472) to 12; the
    // floor this test is about stayed at 3 until #480 PG-2e (§1.485) raised it to 12.
    assert_eq!(own_speaks().get("runtime-client"), Some(&[12, 12]));
}

#[test]
fn r0b_a_console_that_reads_12_pairs_and_one_whose_max_is_below_12_is_refused() {
    // #480 PG-2e: a console that reads revision 12 pairs ([3, 12], the console pinned to v0.12.73
    // or later, and [12, 12]); one whose `max` is below the floor ([3, 11], the console pinned to
    // v0.12.71, and [2, 2], the one R-0b refused) fails `ui_meets_next`, the only failed edge.
    for range in [[3, 12], [12, 12]] {
        let edges = edges_with(range);
        assert!(
            failed(&edges).is_empty(),
            "a console at {range:?}: {edges:?}"
        );
    }
    for range in [[3, 11], [2, 2]] {
        let refused = edges_with(range);
        assert_eq!(
            failed(&refused),
            [("runtime-client", "ui_meets_next")],
            "a console at {range:?}: {refused:?}"
        );
    }
    let refused = edges_with([3, 11]);
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
                {"party": "U*", "source": "manifest", "range": [3, 11]},
                {"party": "R*", "source": "manifest", "range": [12, 12]},
            ],
        })
    );
}
