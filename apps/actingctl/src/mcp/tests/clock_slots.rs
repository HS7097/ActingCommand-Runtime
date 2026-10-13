// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #372 K3 (model-372-skip v3 §4.5 and §9 K3): ac_diagnose's `clock_slots`. It keeps
//! only the instance's rows of `actingd suspended` recorded since the window start, at most 10,
//! the refused and deferred rows first, with `clock_slots_total`; under the output budget it
//! cuts them before the errors page. Every expected value comes from the frozen model.

use super::super::runs::Diagnosis;
use super::super::tools::ToolError;
use crate::diagnose::clock_slot_rows;
use serde_json::{Value, json};
use std::cell::RefCell;

const ALIAS: &str = "fixture-instance-a";
const OTHER_ALIAS: &str = "fixture-instance-b";
const INSTANCE_ID: &str = "instance_0192f3a4-5b6c-7d8e-9f01-23456789abcd";
/// 2023-11-16 00:00 UTC, the start of the diagnosis window in every case.
const SINCE_UNIX_MS: u64 = 1_700_092_800_000;
const MINUTE_MS: u64 = 60_000;

/// A refused row of `actingd suspended`, as K3 writes it, recorded `minutes` after the window
/// start.
fn refused_row(alias: &str, index: u64, minutes: u64) -> Value {
    json!({
        "code": "clock_slot_refused",
        "task_id": format!("fixture.slot.{index:02}"),
        "instance_id": alias,
        "gate_code": "policy_budget_exhausted",
        "budget_dimension": "task_daily",
        "occurrence_unix_ms": SINCE_UNIX_MS + minutes * MINUTE_MS,
        "window_end_unix_ms": SINCE_UNIX_MS + 1_410 * MINUTE_MS,
        "next_eligible_unix_ms": SINCE_UNIX_MS + 1_800 * MINUTE_MS,
        "next_occurrence_unix_ms": SINCE_UNIX_MS + 1_830 * MINUTE_MS,
        "slot_fate": "next_window",
        "slot_fate_settled": true,
        "recorded_sequence": 118_000 + index,
        "recorded_at_unix_ms": SINCE_UNIX_MS + minutes * MINUTE_MS,
        "next_dispatch": {
            "sequence": 119_000 + index,
            "admitted_at_unix_ms": SINCE_UNIX_MS + 1_807 * MINUTE_MS,
        },
    })
}

/// A deferred row (an operator pause, not settled yet), recorded `minutes` after the window
/// start.
fn deferred_row(alias: &str, index: u64, minutes: u64) -> Value {
    json!({
        "code": "clock_slot_deferred",
        "task_id": format!("fixture.slot.{index:02}"),
        "instance_id": alias,
        "gate_code": "dispatch_paused_instance",
        "occurrence_unix_ms": SINCE_UNIX_MS + minutes * MINUTE_MS,
        "window_end_unix_ms": SINCE_UNIX_MS + 1_410 * MINUTE_MS,
        "next_occurrence_unix_ms": SINCE_UNIX_MS + 1_440 * MINUTE_MS + minutes * MINUTE_MS,
        "slot_fate": "pending",
        "slot_fate_settled": false,
        "recorded_sequence": 118_500 + index,
        "recorded_at_unix_ms": SINCE_UNIX_MS + minutes * MINUTE_MS,
    })
}

/// An owed row of a run admitted at `recorded_at_unix_ms`, which served the day before's slot.
fn owed_row(alias: &str, index: u64, recorded_at_unix_ms: u64) -> Value {
    json!({
        "code": "clock_slot_owed",
        "task_id": format!("fixture.slot.{index:02}"),
        "instance_id": alias,
        "occurrence_unix_ms": recorded_at_unix_ms - 1_437 * MINUTE_MS,
        "window_start_unix_ms": recorded_at_unix_ms - 7 * MINUTE_MS,
        "next_occurrence_unix_ms": recorded_at_unix_ms + 23 * MINUTE_MS,
        "slot_fate": "served",
        "slot_fate_settled": true,
        "recorded_sequence": 117_000 + index,
        "recorded_at_unix_ms": recorded_at_unix_ms,
        "next_dispatch": {"sequence": 117_100 + index, "admitted_at_unix_ms": recorded_at_unix_ms},
    })
}

/// The `actingd suspended` document with `clock_slots` newest first, as the report lists them.
fn report(mut clock_slots: Vec<Value>) -> Value {
    clock_slots.sort_by_key(|row| std::cmp::Reverse(row["recorded_sequence"].as_u64()));
    json!({
        "schema_version": "actingcommand.actingd.suspended.v1",
        "status": "ok",
        "suspended": [],
        "lifted": [],
        "repeating": [],
        "warnings": [],
        "clock_slots": clock_slots,
    })
}

fn sequences(rows: &[Value]) -> Vec<u64> {
    rows.iter()
        .map(|row| {
            row["recorded_sequence"]
                .as_u64()
                .unwrap_or_else(|| panic!("a row without recorded_sequence: {row}"))
        })
        .collect()
}

/// A diagnosis with only the clock-slot rows and the errors page rows given.
fn diagnosis(errors: Vec<Value>, clock_slots: Vec<Value>, clock_slots_total: usize) -> Diagnosis {
    let error_sequences = errors
        .iter()
        .map(|row| row["seq"].as_u64().expect("an errors row sequence"))
        .collect();
    Diagnosis {
        instance: json!({"alias": ALIAS, "instance_id": INSTANCE_ID}),
        since: SINCE_UNIX_MS,
        incomplete: false,
        runs: Vec::new(),
        errors,
        error_sequences,
        errors_more: false,
        errors_next: None,
        snapshot: 121_000,
        continuable: false,
        repeating: Vec::new(),
        suspended: Vec::new(),
        lift: Vec::new(),
        report_warnings: Vec::new(),
        report_status: json!({"status": "ok", "through_sequence": 121_000}),
        clock_slots,
        clock_slots_total,
        runs_grouped: Vec::new(),
        failures_grouped: Vec::new(),
    }
}

/// Renders `diagnosis` within the output budget. The errors page holds the first `kept` rows;
/// every `kept` it was asked for is returned too.
fn render(diagnosis: &Diagnosis) -> (Value, Vec<usize>) {
    let asked = RefCell::new(Vec::new());
    let page = |kept: usize| -> Result<Value, ToolError> {
        asked.borrow_mut().push(kept);
        let mut page = json!({
            "events": diagnosis.errors[..kept].to_vec(),
            "more": kept < diagnosis.errors.len(),
            "snapshot_ledger_position": diagnosis.snapshot,
        });
        if kept < diagnosis.errors.len() {
            page["truncated"] = json!(true);
        }
        Ok(page)
    };
    let result = diagnosis
        .render_within_budget(page)
        .unwrap_or_else(|error| panic!("ac_diagnose rendering: {}", error.to_value()))
        .result;
    (result, asked.into_inner())
}

/// An errors-view row with its error frame, shaped as ac_events lists it.
fn error_row(index: u64) -> Value {
    let sequence = 120_000 + index * 37;
    json!({
        "seq": sequence,
        "ts": SINCE_UNIX_MS + 370 * MINUTE_MS + index * 1_000,
        "type": "task.failed",
        "severity": "error",
        "instance": INSTANCE_ID,
        "request_id": format!("request_0192f3a4-5b6c-7d8e-9f01-{index:012}"),
        "run_id": format!("run_0192f3a4-5b6c-7d8e-9f01-{index:012}"),
        "code": "contained_task_step_unconfirmed",
        "materials": [{
            "ref": format!(
                "{sequence}:event_0192f3a4-5b6c-7d8e-9f01-{index:012}:artifact_0192f3a4-5b6c-7d8e-9f01-{index:012}"
            ),
            "media_type": "image/png",
            "size": 1_843_200,
        }],
    })
}

#[test]
fn ac_diagnose_keeps_ten_of_the_instances_clock_slots_in_its_window_refused_first() {
    // The instance's rows in the window: eight owed runs in the morning, four refused slots
    // after them and two deferred ones later. Two more of its rows were recorded before the
    // window, and another instance has rows in it.
    let mut rows = Vec::new();
    for index in 0..8 {
        rows.push(owed_row(
            ALIAS,
            index,
            SINCE_UNIX_MS + (360 + index) * MINUTE_MS,
        ));
    }
    for index in 0..4 {
        rows.push(refused_row(ALIAS, index, 390 + index));
    }
    for index in 4..6 {
        rows.push(deferred_row(ALIAS, index, 600 + index));
    }
    let before_window = [
        refused_row(ALIAS, 20, 0),
        owed_row(ALIAS, 21, SINCE_UNIX_MS - MINUTE_MS),
    ]
    .into_iter()
    .zip(116_000_u64..)
    .map(|(mut row, sequence)| {
        row["recorded_sequence"] = json!(sequence);
        row["recorded_at_unix_ms"] = json!(SINCE_UNIX_MS - 30 * MINUTE_MS);
        row
    });
    let mut all = rows.clone();
    all.extend(before_window);
    all.extend([
        owed_row(OTHER_ALIAS, 31, SINCE_UNIX_MS + 368 * MINUTE_MS),
        refused_row(OTHER_ALIAS, 30, 394),
        deferred_row(OTHER_ALIAS, 32, 700),
    ]);
    let document = report(all);

    let (kept, total) = clock_slot_rows(&document, ALIAS, &json!(INSTANCE_ID), SINCE_UNIX_MS);
    assert_eq!(total, 14, "the instance's rows in the window");
    // Newest first in each group: the refused and deferred rows, then the owed rows.
    let mut stopped = sequences(&rows[8..]);
    stopped.sort_unstable();
    stopped.reverse();
    let mut owed = sequences(&rows[..8]);
    owed.sort_unstable();
    owed.reverse();
    let expected = stopped
        .into_iter()
        .chain(owed.into_iter().take(4))
        .collect::<Vec<_>>();
    assert_eq!(sequences(&kept), expected);
    assert!(
        kept.iter().all(|row| row["instance_id"] == json!(ALIAS)),
        "{kept:#?}"
    );

    let (result, _) = render(&diagnosis(Vec::new(), kept.clone(), total));
    assert_eq!(result["clock_slots"], Value::Array(kept), "{result}");
    assert_eq!(result["clock_slots_total"], json!(14), "{result}");
    assert!(result.get("clock_slots_truncated").is_none(), "{result}");

    // All of them kept: no total.
    let few = report(rows[..3].to_vec());
    let (kept, total) = clock_slot_rows(&few, ALIAS, &json!(INSTANCE_ID), SINCE_UNIX_MS);
    assert_eq!((kept.len(), total), (3, 3));
    let (result, _) = render(&diagnosis(Vec::new(), kept, total));
    assert!(result.get("clock_slots_total").is_none(), "{result}");
}

#[test]
fn ac_diagnose_cuts_clock_slots_first_and_keeps_a_full_errors_page() {
    // A day of one instance: 10 refused heads and 35 owed runs, and a full errors page of
    // failed runs with their error frames.
    let mut rows = (0..10)
        .map(|index| refused_row(ALIAS, index, 390))
        .collect::<Vec<_>>();
    rows.extend((10..45).map(|index| owed_row(ALIAS, index, SINCE_UNIX_MS + 367 * MINUTE_MS)));
    let (kept, total) = clock_slot_rows(&report(rows), ALIAS, &json!(INSTANCE_ID), SINCE_UNIX_MS);
    assert_eq!((kept.len(), total), (10, 45));
    let errors = (0..20).map(error_row).collect::<Vec<_>>();

    let (result, asked) = render(&diagnosis(errors.clone(), kept.clone(), total));
    assert!(
        asked.iter().all(|kept| *kept == errors.len()),
        "the errors page was cut before the clock slots: {asked:?}"
    );
    assert_eq!(result["errors_page"]["events"], Value::Array(errors));
    assert!(result["errors_page"].get("truncated").is_none(), "{result}");
    assert_eq!(result["clock_slots_truncated"], json!(true), "{result}");
    assert_eq!(result["clock_slots_total"], json!(45), "{result}");
    let shown = result["clock_slots"]
        .as_array()
        .unwrap_or_else(|| panic!("clock_slots is a list: {result}"));
    assert!(
        shown.len() < kept.len(),
        "the clock slots were not cut: {result}"
    );
    assert_eq!(
        shown.as_slice(),
        &kept[..shown.len()],
        "the oldest rows go first"
    );
}
