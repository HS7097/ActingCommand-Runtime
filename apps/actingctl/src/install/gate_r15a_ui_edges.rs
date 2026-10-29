// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #429 A.4 a (R-15a, since 0.12.30; coordinator brief rd-1029/brief-wave3a.md R-15a;
// contracts/component-interfaces.md, "Checks per operation" and "Parse rules";
// contracts/install-cli.md, "Interface checks", "UI edges"): the installer judges the UI that runs
// after an operation (party U*) by the declaration its build manifest carries, beside the
// installer (I), the live build (Rp) and the build that will run (R*).
// - T1, the Speaks table: each U* edge passes on one table of ranges and fails on another, where
//   it is the only U* edge that fails;
// - T2: a failed edge names every side with the source and the range of its own, as the plan's
//   `edges` rows print it;
// - T3: a UI declaration that does not name `runtime-outcomes` counts as [0, 0], with the source
//   `default`.
// Every range is neutral; each table starts from the neutral build's declaration of `fixture.rs`,
// on which every U* edge passes.

use super::fixture::speaks;
use super::manifest::{BuildManifest, Speaks, UiDeclaration, parse_declaration};
use super::plan::{Edge, EdgeResult, ui_edges};
use serde_json::{Value, json};

const SCHEMA: &str = "actingcommand.component-interfaces.v1";
/// Every U* edge, as `(interface, check)`.
const UI_EDGES: [(&str, &str); 5] = [
    ("actingd-config", "installer_ui_next_common"),
    ("ledger", "previous_max_in_ui"),
    ("ledger", "next_max_in_ui"),
    ("runtime-client", "ui_meets_next"),
    ("runtime-outcomes", "next_max_in_ui"),
];

/// One party's change to the neutral declaration: `(party, interface, range)`, where `null`
/// leaves the interface out.
type Change = (&'static str, &'static str, Value);

/// The neutral declaration with every change of `party`.
fn declared(party: &str, changes: &[Change]) -> Value {
    let mut named = speaks();
    for (_, interface, range) in changes.iter().filter(|change| change.0 == party) {
        let members = named.as_object_mut().expect("speaks is an object");
        if range.is_null() {
            members.remove(*interface);
        } else {
            members.insert((*interface).to_owned(), range.clone());
        }
    }
    json!({"schema_version": SCHEMA, "speaks": named})
}

fn speaks_of(party: &str, changes: &[Change]) -> Speaks {
    parse_declaration(&declared(party, changes)).expect("a valid declaration")
}

/// The U* rows for the four declarations `changes` make.
fn edges_for(changes: &[Change]) -> Vec<Edge> {
    let ui = UiDeclaration::parse(&declared("U*", changes)).expect("a valid UI declaration");
    ui_edges(
        &speaks_of("I", changes),
        &speaks_of("Rp", changes),
        &speaks_of("R*", changes),
        &ui,
    )
}

/// The one row of `check` on `interface`.
fn row<'a>(edges: &'a [Edge], interface: &str, check: &str) -> &'a Edge {
    let mut rows = edges
        .iter()
        .filter(|edge| edge.interface == interface && edge.check == check);
    let found = rows
        .next()
        .unwrap_or_else(|| panic!("a row {interface} {check}: {edges:?}"));
    assert!(rows.next().is_none(), "one row {interface} {check}");
    found
}

/// The rows that fail, as `(interface, check)`.
fn failed(edges: &[Edge]) -> Vec<(&'static str, &'static str)> {
    edges
        .iter()
        .filter(|edge| edge.result == EdgeResult::Fail)
        .map(|edge| (edge.interface, edge.check))
        .collect()
}

/// One row of the Speaks table: the edge, a table on which it passes, one on which it fails, and
/// the parties its failed row names (`[party, source, range]`).
struct Case {
    edge: (&'static str, &'static str),
    pass: Vec<Change>,
    fail: Vec<Change>,
    fail_parties: Value,
}

fn table() -> Vec<Case> {
    vec![
        // One revision common to all three; in the failing table I ∩ R* and U* ∩ R* both hold
        // one, I ∩ U* does not.
        Case {
            edge: UI_EDGES[0],
            pass: vec![
                ("R*", "actingd-config", json!([5, 25])),
                ("U*", "actingd-config", json!([18, 25])),
            ],
            fail: vec![
                ("R*", "actingd-config", json!([5, 25])),
                ("U*", "actingd-config", json!([19, 25])),
            ],
            fail_parties: json!([
                {"party": "I", "source": "declaration", "range": [5, 18]},
                {"party": "U*", "source": "manifest", "range": [19, 25]},
                {"party": "R*", "source": "manifest", "range": [5, 25]},
            ]),
        },
        // Rp.max ∈ U*.
        Case {
            edge: UI_EDGES[1],
            pass: vec![("Rp", "ledger", json!([3, 3]))],
            fail: vec![("Rp", "ledger", json!([3, 5]))],
            fail_parties: json!([
                {"party": "Rp", "source": "manifest", "range": [3, 5]},
                {"party": "U*", "source": "manifest", "range": [3, 4]},
            ]),
        },
        // R*.max ∈ U*.
        Case {
            edge: UI_EDGES[2],
            pass: vec![
                ("R*", "ledger", json!([4, 5])),
                ("U*", "ledger", json!([3, 5])),
            ],
            fail: vec![("R*", "ledger", json!([4, 5]))],
            fail_parties: json!([
                {"party": "R*", "source": "manifest", "range": [4, 5]},
                {"party": "U*", "source": "manifest", "range": [3, 4]},
            ]),
        },
        // U* ∩ R*.
        Case {
            edge: UI_EDGES[3],
            pass: vec![("U*", "runtime-client", json!([1, 2]))],
            fail: vec![("U*", "runtime-client", json!([1, 1]))],
            fail_parties: json!([
                {"party": "U*", "source": "manifest", "range": [1, 1]},
                {"party": "R*", "source": "manifest", "range": [2, 2]},
            ]),
        },
        // R*.max ∈ U*.
        Case {
            edge: UI_EDGES[4],
            pass: vec![("U*", "runtime-outcomes", json!([0, 1]))],
            fail: vec![("U*", "runtime-outcomes", json!([0, 0]))],
            fail_parties: json!([
                {"party": "R*", "source": "manifest", "range": [1, 1]},
                {"party": "U*", "source": "manifest", "range": [0, 0]},
            ]),
        },
    ]
}

// T1: each U* edge, one table it passes on and one it fails on.
#[test]
fn r15a_t1_each_ui_edge_passes_and_fails_by_its_speaks_table() {
    let base = edges_for(&[]);
    let mut rows = base
        .iter()
        .map(|edge| (edge.interface, edge.check))
        .collect::<Vec<_>>();
    rows.sort_unstable();
    let mut expected = UI_EDGES.to_vec();
    expected.sort_unstable();
    assert_eq!(rows, expected, "one row per U* edge: {base:?}");
    assert!(failed(&base).is_empty(), "the neutral table: {base:?}");
    for case in table() {
        let (interface, check) = case.edge;
        let passed = edges_for(&case.pass);
        assert_eq!(
            row(&passed, interface, check).result,
            EdgeResult::Pass,
            "{interface} {check} on its passing table: {passed:?}"
        );
        assert!(
            failed(&passed).is_empty(),
            "{interface} {check}: {passed:?}"
        );
        let refused = edges_for(&case.fail);
        assert_eq!(
            row(&refused, interface, check).result,
            EdgeResult::Fail,
            "{interface} {check} on its failing table: {refused:?}"
        );
        assert_eq!(
            failed(&refused),
            vec![case.edge],
            "only {interface} {check} fails: {refused:?}"
        );
    }
}

// T2: a failed edge names both sides (all three for `actingd-config`), each with the source and
// the range of its own, in the row the plan prints.
#[test]
fn r15a_t2_a_failed_ui_edge_names_every_side_with_its_source_and_range() {
    for case in table() {
        let (interface, check) = case.edge;
        let refused = edges_for(&case.fail);
        let printed = serde_json::to_value(row(&refused, interface, check)).expect("a row");
        assert_eq!(
            printed,
            json!({
                "interface": interface,
                "check": check,
                "result": "fail",
                "parties": case.fail_parties,
            }),
            "{interface} {check}"
        );
    }
}

// T3: a UI declaration without `runtime-outcomes`, read from a UI build's manifest, is [0, 0]
// from the parse rules: R*.max 1 is not in it, R*.max 0 is; one that names [0, 0] is the same
// range from its manifest.
#[test]
fn r15a_t3_a_ui_declaration_without_runtime_outcomes_counts_as_zero_zero() {
    let manifest = |interfaces: Value| -> BuildManifest {
        serde_json::from_value(json!({
            "repository": "neutral/ui",
            "commit_sha": "3333333333333333333333333333333333333333",
            "files": [],
            "interfaces": interfaces,
        }))
        .expect("a UI build manifest")
    };
    let unnamed = [("U*", "runtime-outcomes", Value::Null)];
    let ui = UiDeclaration::of_manifest(&manifest(declared("U*", &unnamed)))
        .expect("a valid UI declaration");
    assert!(!ui.names_outcomes);
    assert_eq!(ui.speaks.get("runtime-outcomes"), Some(&[0, 0]));
    let neutral = speaks_of("R*", &[]);
    let outcomes_row = |target: &Speaks, ui: &UiDeclaration| {
        let edges = ui_edges(&neutral, &neutral, target, ui);
        let printed = row(&edges, "runtime-outcomes", "next_max_in_ui");
        serde_json::to_value(printed).expect("a row")
    };
    assert_eq!(
        outcomes_row(&neutral, &ui),
        json!({
            "interface": "runtime-outcomes",
            "check": "next_max_in_ui",
            "result": "fail",
            "parties": [
                {"party": "R*", "source": "manifest", "range": [1, 1]},
                {"party": "U*", "source": "default", "range": [0, 0]},
            ],
        })
    );
    let silent = speaks_of("R*", &[("R*", "runtime-outcomes", json!([0, 0]))]);
    assert_eq!(
        outcomes_row(&silent, &ui)["result"],
        json!("pass"),
        "R*.max 0 lies in [0, 0]"
    );
    let named = UiDeclaration::of_manifest(&manifest(declared(
        "U*",
        &[("U*", "runtime-outcomes", json!([0, 0]))],
    )))
    .expect("a valid UI declaration");
    assert!(named.names_outcomes);
    assert_eq!(named.speaks, ui.speaks, "the same ranges");
    assert_eq!(
        outcomes_row(&neutral, &named)["parties"][1],
        json!({"party": "U*", "source": "manifest", "range": [0, 0]})
    );
}
