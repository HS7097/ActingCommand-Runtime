// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #438 HELD-1 (register §1.322 HELD-R9, §1.327; `contracts/scheduling/README.md`, "Held
//! records", "Where a person reads it"; `contracts/policy-suspension.md`, `held`): the diagnose
//! half of `held_t9` (the `actingd suspended` half is the host's `gate_438_held`), on an `actingd
//! suspended` document of the shape the host's half reads back, with no Runtime, device or clock.
//! `ac_diagnose` (and `actingctl diagnose`) carries the report's `held` rows of the diagnosed
//! instance, verbatim (every row without an instance), and marks `held_reported: true` on each
//! `not_run` row whose instance, task and `since_unix_ms` an open held row names.

use crate::diagnose::{Diagnosis, held_rows, mark_held_reported};
use crate::schedule::NotRun;
use actingcommand_contract::outcome::{Detail, Outcome};
use serde_json::{Value, json};

const ALIAS: &str = "neutral-a";
const OTHER_ALIAS: &str = "neutral-b";
const INSTANCE_ID: &str = "instance_0192f3a4-5b6c-7d8e-9f01-23456789abcd";
const SINCE: u64 = 1_700_000_000_000;
const MINUTE_MS: u64 = 60_000;

/// A `held` row of `actingd suspended`: a hold of `task_id` on `instance` since `since`, recorded
/// at `sequence`, with its end when one is given.
fn held_row(instance: &str, task_id: &str, since: u64, sequence: u64, ended: Option<u64>) -> Value {
    let mut row = json!({
        "code": "task_held",
        "task_id": task_id,
        "instance_id": instance,
        "gate_code": "policy_no_progress",
        "held_state": "due",
        "held_since_unix_ms": since,
        "held_for_ms": 30 * MINUTE_MS,
        "held_after_ms": 30 * MINUTE_MS,
        "recorded_sequence": sequence,
        "recorded_at_unix_ms": since + 30 * MINUTE_MS,
    });
    if let Some(ended) = ended {
        row["ended"] = json!({
            "sequence": ended,
            "at_unix_ms": since + 40 * MINUTE_MS,
            "held_end": "selected",
            "held_for_ms": 40 * MINUTE_MS,
        });
    }
    row
}

/// A `not_run` row of a diagnosis: a candidate row of `task_id` on `instance` since `since`.
fn not_run_row(instance: &str, task_id: &str, since: u64) -> Value {
    json!({
        "instance_alias": instance,
        "task_id": task_id,
        "state": "deferred",
        "eligibility": "true",
        "why": {"code": "policy_no_progress", "category": "info", "values": {}},
        "reasons": ["policy_no_progress"],
        "since_unix_ms": since,
    })
}

/// A diagnosis of the instance with only `held` and `not_run` rows.
fn diagnosis(held: Vec<Value>, not_run: Vec<Value>) -> Diagnosis {
    let total = not_run.len();
    Diagnosis {
        instance: json!({"alias": ALIAS, "instance_id": INSTANCE_ID}),
        since: SINCE,
        incomplete: false,
        incomplete_sections: Vec::new(),
        runs: Vec::new(),
        errors: Vec::new(),
        error_sequences: Vec::new(),
        errors_more: false,
        errors_next: None,
        snapshot: 9,
        continuable: false,
        repeating: Vec::new(),
        suspended: Vec::new(),
        packs: Vec::new(),
        remedies: Vec::new(),
        reruns: Vec::new(),
        held,
        lift: Vec::new(),
        report_warnings: Vec::new(),
        report_status: json!({"status": "ok", "through_sequence": 9}),
        report_outcome: None,
        clock_slots: Vec::new(),
        clock_slots_total: 0,
        runs_grouped: Vec::new(),
        failures_grouped: Vec::new(),
        not_run: Some(NotRun {
            rows: not_run,
            total,
        }),
        warnings: Vec::new(),
    }
}

fn envelope(outcome: &Outcome) -> Value {
    crate::cli_line::outcome_value(outcome, Detail::Full)
}

#[test]
fn held_t9_actingd_suspended_and_diagnose_list_an_instance_held_records() {
    let open = held_row(ALIAS, "task.neutral.head", SINCE, 120, None);
    let ended = held_row(
        ALIAS,
        "task.neutral.ended",
        SINCE + MINUTE_MS,
        110,
        Some(125),
    );
    let others = held_row(OTHER_ALIAS, "task.neutral.head", SINCE, 115, None);
    let unparsed = json!({
        "parse_state": "unparsed",
        "signal_id": format!("signal:task_held:held:{}", "c".repeat(64)),
        "code": "task_held",
    });
    let document = json!({
        "schema_version": "actingcommand.actingd.suspended.v2",
        "held": [open.clone(), ended.clone(), others.clone(), unparsed.clone()],
    });

    // The instance's rows, verbatim and in the report's order; every row without an instance.
    let instance_id = json!(INSTANCE_ID);
    let kept = held_rows(&document, Some((ALIAS, &instance_id)));
    assert_eq!(
        kept,
        vec![open.clone(), ended.clone()],
        "the instance's held rows only"
    );
    assert_eq!(
        held_rows(&document, None),
        vec![open, ended, others, unparsed],
        "every row without an instance"
    );

    // The mark: the row an open held record names (instance, task and start); not the row of an
    // ended hold, not a row of the same task with another start, not another instance's row.
    let mut rows = vec![
        not_run_row(ALIAS, "task.neutral.head", SINCE),
        not_run_row(ALIAS, "task.neutral.ended", SINCE + MINUTE_MS),
        not_run_row(ALIAS, "task.neutral.restarted", SINCE + 5 * MINUTE_MS),
        not_run_row(OTHER_ALIAS, "task.neutral.ended", SINCE),
    ];
    let unmarked = rows.clone();
    mark_held_reported(&mut rows, &kept);
    let mut marked = unmarked[0].clone();
    marked["held_reported"] = json!(true);
    assert_eq!(
        rows,
        vec![
            marked,
            unmarked[1].clone(),
            unmarked[2].clone(),
            unmarked[3].clone()
        ]
    );
    let mut later = [not_run_row(ALIAS, "task.neutral.head", SINCE + MINUTE_MS)];
    mark_held_reported(&mut later, &kept);
    assert!(
        later[0].get("held_reported").is_none(),
        "another start is another run: {later:?}"
    );

    // The answer carries the instance's held rows and the marked not_run rows.
    let diagnosis = diagnosis(kept.clone(), rows.clone());
    let result = diagnosis.render(
        &diagnosis.kept_all(),
        json!({"events": [], "more": false, "snapshot_ledger_position": 9}),
        &envelope,
    );
    assert_eq!(result["held"], Value::Array(kept), "{result:#}");
    assert!(result.get("held_truncated").is_none(), "{result:#}");
    assert_eq!(result["not_run"], Value::Array(rows), "{result:#}");
}
