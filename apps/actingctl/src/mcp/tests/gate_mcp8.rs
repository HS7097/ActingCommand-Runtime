// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #389 MCP-8 (MCP-0 command table v1.1 §4.9): ac_overview's truncation order. An answer
// that does not fit the output budget is cut one block at a time, earlier blocks first:
// fact_refresh, host, startup, tagged_state, ledger, performance, watchdog, catalog,
// config_generation, then each instance row's run from the last instance back, then each row's
// monitor, then install.build. Each cut block is marked `<block>_truncated: true`. The daemon,
// the global pause, each row's identity, pause, lease_active and queued, and incomplete are never
// cut. Cutting stops as soon as the answer fits; when nothing is left to cut the answer stays as
// it is, and the call answers output_budget_exceeded (MCP-T1 gate 30). The overview below holds
// every block of §4.9, those later slices add included, so that the whole order is seen.

use super::super::observer::cut_to_fit;
use serde_json::{Value, json};
use std::cell::RefCell;

// The top-level blocks of §4.9, in its order.
const TOP_BLOCKS: [&str; 9] = [
    "fact_refresh",
    "host",
    "startup",
    "tagged_state",
    "ledger",
    "performance",
    "watchdog",
    "catalog",
    "config_generation",
];
const ROWS: usize = 3;

// One instance row with every member it can carry.
fn row(index: usize) -> Value {
    json!({
        "alias": format!("neutral.overview.{index}"),
        "instance_id": format!("instance_0192f3a4-5b6c-7d8e-9f01-23456789abc{index}"),
        "adb_port": 40_000 + index,
        "game_id": "neutral.game",
        "lease_active": index == 0,
        "queued": index,
        "pause": {"revision": 2, "stage": "paused", "pause_reason": "operator"},
        "monitor": {"policy": {"interval_ms": 30_000}, "state": {"failures": 0}},
        "run": {"state": "succeeded", "request_id": format!("request_{index}")},
    })
}

// An overview holding every block §4.9 names.
fn full_overview() -> Value {
    let mut overview = json!({
        "daemon": {"online": true, "owner_epoch": "owner_epoch_0001"},
        "global_pause": {"revision": 1, "stage": "paused"},
        "instances": (0..ROWS).map(row).collect::<Vec<_>>(),
        "run_window": {"since_unix_ms": 1_700_000_000_000_u64},
        "lab_tool": {"present": true},
        "install": {
            "root": "neutral-root",
            "state_root": "neutral-state",
            "build": {"commit_sha": "0123456789abcdef"},
        },
        "incomplete": true,
    });
    for block in TOP_BLOCKS {
        overview[block] = json!({"neutral": block});
    }
    overview
}

// What one cut removed, read from the marker it left: a top-level block by its name, a row's
// block as `<block>@<row>`, the build as `install.build`.
fn cut_between(before: &Value, after: &Value) -> String {
    let new_markers = |before: &Value, after: &Value| {
        after
            .as_object()
            .into_iter()
            .flatten()
            .filter(|(name, _)| name.ends_with("_truncated") && before.get(name.as_str()).is_none())
            .map(|(name, _)| name.trim_end_matches("_truncated").to_owned())
            .collect::<Vec<_>>()
    };
    let mut cuts = new_markers(before, after);
    for index in 0..ROWS {
        cuts.extend(
            new_markers(&before["instances"][index], &after["instances"][index])
                .into_iter()
                .map(|block| format!("{block}@{index}")),
        );
    }
    cuts.extend(
        new_markers(&before["install"], &after["install"])
            .into_iter()
            .map(|block| format!("install.{block}")),
    );
    assert_eq!(
        cuts.len(),
        1,
        "one cut leaves one marker: {cuts:?}\n{after}"
    );
    cuts.remove(0)
}

// Every answer `fits` was asked about, in order, and the cuts between them.
fn cuts_until(fits: impl Fn(&Value) -> bool) -> (Value, Vec<String>) {
    let mut overview = full_overview();
    let seen = RefCell::new(Vec::new());
    cut_to_fit(&mut overview, |candidate| {
        seen.borrow_mut().push(candidate.clone());
        fits(candidate)
    });
    let seen = seen.into_inner();
    let cuts = seen
        .windows(2)
        .map(|pair| cut_between(&pair[0], &pair[1]))
        .collect();
    (overview, cuts)
}

#[test]
fn gate_ac_overview_cuts_every_block_in_the_order_of_4_9_and_never_the_identity_or_pauses() {
    let (overview, cuts) = cuts_until(|_| false);
    let mut expected = TOP_BLOCKS.map(str::to_owned).to_vec();
    // Each row's run, from the last instance back.
    expected.extend((0..ROWS).rev().map(|index| format!("run@{index}")));
    assert_eq!(cuts.len(), expected.len() + ROWS + 1, "cuts: {cuts:?}");
    assert_eq!(
        &cuts[..expected.len()],
        expected.as_slice(),
        "cuts: {cuts:?}"
    );
    // Then each row's monitor (§4.9 names no order among the rows), then the build.
    let mut monitors = cuts[expected.len()..expected.len() + ROWS].to_vec();
    monitors.sort();
    assert_eq!(
        monitors,
        (0..ROWS)
            .map(|index| format!("monitor@{index}"))
            .collect::<Vec<_>>(),
        "cuts: {cuts:?}"
    );
    assert_eq!(cuts.last().map(String::as_str), Some("install.build"));

    // Nothing left to cut: what is never cut stays as it was, each cut block left its marker.
    let mut kept = full_overview();
    let members = kept.as_object_mut().expect("an object");
    for block in TOP_BLOCKS {
        members.remove(block);
        members.insert(format!("{block}_truncated"), json!(true));
    }
    for row in members["instances"]
        .as_array_mut()
        .expect("instance rows")
        .iter_mut()
    {
        let row = row.as_object_mut().expect("a row");
        for block in ["run", "monitor"] {
            row.remove(block);
            row.insert(format!("{block}_truncated"), json!(true));
        }
    }
    let install = members["install"].as_object_mut().expect("install");
    install.remove("build");
    install.insert("build_truncated".to_owned(), json!(true));
    assert_eq!(overview, kept);
}

#[test]
fn gate_ac_overview_stops_cutting_as_soon_as_the_answer_fits() {
    // It fits once the second row's run is gone.
    let (overview, cuts) = cuts_until(|candidate| candidate["instances"][1].get("run").is_none());
    let mut expected = TOP_BLOCKS.map(str::to_owned).to_vec();
    expected.extend(["run@2".to_owned(), "run@1".to_owned()]);
    assert_eq!(cuts, expected);
    let first = &overview["instances"][0];
    assert!(
        first.get("run").is_some() && first.get("run_truncated").is_none(),
        "the first row keeps its run: {overview}"
    );
    for index in 0..ROWS {
        assert!(
            overview["instances"][index].get("monitor").is_some(),
            "row {index} keeps its monitor: {overview}"
        );
    }
    assert_eq!(overview["install"], full_overview()["install"]);
}
