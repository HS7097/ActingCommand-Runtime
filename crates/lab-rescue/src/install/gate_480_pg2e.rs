// SPDX-License-Identifier: AGPL-3.0-only

// #480 PG-2e (0.12.75; register §1.485; contracts/component-interfaces.md, the `runtime-client`
// row of revision 12; contracts/install-cli.md, "Interface checks"): the Runtime answers an
// instance pause's `pause_mode` and `in_the_way`, which a reader below 12 refuses, so this build
// declares `runtime-client` [12, 12] in the declaration its Runtime and Tools manifests carry, and
// the interfaces document's row says so. The installer's edges an update or a rollback computes
// (`plan::interface_edges`, the rows `install update plan`, `install rollback plan`, `install undo`
// and the Lab's `deploy` and `rollback` refuse by), read on this build's own declaration as R*,
// Rp and I, beside the builds the plan meets: 0.12.73 and 0.12.74 declare `runtime-client`
// [3, 12], 0.12.72 and older [3, 11]; every one of them `ledger` [3, 9] from 0.12.71 on,
// `runtime-outcomes` [1, 1] and `install-control` [0, 0], as this build (no placed pack here, so
// no `package` edge):
// - the update from 0.12.74 passes every edge without `--accept-ledger-one-way`, whether the
//   installer is the live build's (I [3, 12]) or this build's (I [12, 12]);
// - the way back to 0.12.74 and to 0.12.73, by this build's installer, passes every edge too;
// - the way back to a build whose tools end at 11 fails `runtime-client` `installer_meets_next`
//   alone, both parties named with their source and range; the ledger's edges pass.
// No Runtime, no clock.

use super::manifest::{Speaks, parse_declaration};
use super::plan::{Edge, EdgeResult, interface_edges};
use crate::codes::InstallLocation;
use serde_json::{Value, json};

/// The declaration this build copies into its Runtime and Tools manifests.
const OWN_DECLARATION: &str =
    include_str!("../../../../distribution/windows/component-interfaces.json");
/// The interfaces document, whose `runtime-client` rows state each revision and the range.
const INTERFACES_DOC: &str = include_str!("../../../../contracts/component-interfaces.md");
const CLIENT: &str = "runtime-client";

fn own() -> Speaks {
    let value: Value =
        serde_json::from_str(OWN_DECLARATION).expect("the build's declaration is JSON");
    parse_declaration(&value).expect("the build's declaration follows the parse rules")
}

/// A build that declares what this one declares, but `runtime-client` `range`.
fn with_client(range: [u64; 2]) -> Speaks {
    let mut speaks = own();
    speaks.insert(CLIENT.to_owned(), range);
    speaks
}

/// The edges of an update or a rollback (`operation`) from `live` to `target` that `installer`
/// computes, with no placed pack and no acknowledgment of a one-way ledger; nothing is warned.
fn edges_of(
    installer: &Speaks,
    live: &Speaks,
    target: &Speaks,
    operation: InstallLocation,
) -> Vec<Edge> {
    let mut warnings = Vec::new();
    let edges = interface_edges(
        installer,
        live,
        target,
        &[],
        false,
        operation,
        &mut warnings,
    );
    assert!(warnings.is_empty(), "{warnings:?}");
    edges
}

fn failed(edges: &[Edge]) -> Vec<(&'static str, &'static str)> {
    edges
        .iter()
        .filter(|edge| edge.result == EdgeResult::Fail)
        .map(|edge| (edge.interface, edge.check))
        .collect()
}

#[test]
fn pg2e_the_build_declares_runtime_client_12_to_12_and_its_row_says_so() {
    assert_eq!(own().get(CLIENT), Some(&[12, 12]));
    let twelve = INTERFACES_DOC
        .lines()
        .skip_while(|line| !line.starts_with("| `runtime-client` |"))
        .skip(1)
        .take_while(|line| line.starts_with("| | | | "))
        .find(|line| line.starts_with("| | | | 12 |"))
        .expect("the runtime-client row of revision 12");
    assert!(
        twelve.contains("declares [12, 12]"),
        "the row of revision 12 states the range"
    );
}

#[test]
fn pg2e_the_update_from_0_12_74_and_the_way_back_to_0_12_74_and_0_12_73_pass() {
    // 0.12.73 and 0.12.74, for the interfaces these edges read.
    let previous = with_client([3, 12]);
    let this = own();
    for (installer, live, target, operation) in [
        (&previous, &previous, &this, InstallLocation::UpdatePlan),
        (&this, &previous, &this, InstallLocation::UpdatePlan),
        (&this, &this, &previous, InstallLocation::RollbackPlan),
    ] {
        let edges = edges_of(installer, live, target, operation);
        assert!(failed(&edges).is_empty(), "{operation:?}: {edges:?}");
        // No flag: the ledger does not move, so its one-way edge passes without one.
        assert!(
            edges.iter().any(|edge| edge.interface == "ledger"
                && edge.check == "one_way"
                && edge.result == EdgeResult::Pass),
            "{operation:?}: {edges:?}"
        );
    }
}

#[test]
fn pg2e_the_way_back_to_a_build_whose_tools_end_at_11_is_refused_installer_meets_next() {
    // 0.12.72 and older, for the interfaces these edges read.
    let older = with_client([3, 11]);
    let this = own();
    let edges = edges_of(&this, &this, &older, InstallLocation::RollbackPlan);
    assert_eq!(
        failed(&edges),
        [(CLIENT, "installer_meets_next")],
        "{edges:?}"
    );
    let row = edges
        .iter()
        .find(|edge| edge.interface == CLIENT && edge.check == "installer_meets_next")
        .expect("the runtime-client row");
    assert_eq!(
        serde_json::to_value(row).expect("a row"),
        json!({
            "interface": "runtime-client",
            "check": "installer_meets_next",
            "result": "fail",
            "parties": [
                {"party": "I", "source": "declaration", "range": [12, 12]},
                {"party": "R*", "source": "manifest", "range": [3, 11]},
            ],
        })
    );
}
