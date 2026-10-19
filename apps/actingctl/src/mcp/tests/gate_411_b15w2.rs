// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #411 B15, slice R7 (apps gate key 75; register §1.117; model-b15 §7.5;
//! contracts/policy-suspension.md, `remedies`), the MCP half: ac_diagnose carries the `remedies`
//! rows of `actingd suspended` whose `instance_id` is the diagnosed instance (every row without
//! an instance), verbatim, with the members the report gives them, and its description says so.

use super::super::runs::Diagnosis;
use super::super::tools::{self, ToolError};
use crate::diagnose::remedy_rows;
use serde_json::{Value, json};

const ALIAS: &str = "fixture-instance-a";
const OTHER_ALIAS: &str = "fixture-instance-b";
const INSTANCE_ID: &str = "instance_0192f3a4-5b6c-7d8e-9f01-23456789abcd";

/// A `remedies` row of `actingd suspended`: a stopped round of `task_id` on `instance`, recorded
/// at `sequence`, with its lift when one is given.
fn remedy_row(instance: &str, task_id: &str, sequence: u64, lifted: Option<u64>) -> Value {
    let mut row = json!({
        "code": "policy_remedy_unchosen",
        "remedy_problem": "no_choice",
        "task_id": task_id,
        "instance_id": instance,
        "outcome_key": "blocked_k",
        "need": "need_a",
        "recorded_sequence": sequence,
        "recorded_at_unix_ms": 1_700_000_000_000_u64 + sequence,
    });
    if let Some(lifted) = lifted {
        row["lifted"] = json!({"sequence": lifted, "at_unix_ms": 1_700_000_000_000_u64 + lifted});
    }
    row
}

/// The `actingd suspended` document with `remedies`.
fn report(remedies: Vec<Value>) -> Value {
    json!({
        "schema_version": "actingcommand.actingd.suspended.v2",
        "suspended": [],
        "lifted": [],
        "repeating": [],
        "warnings": [],
        "clock_slots": [],
        "packs": [],
        "remedies": remedies,
    })
}

/// A diagnosis with only the `remedies` rows given.
fn diagnosis(remedies: Vec<Value>) -> Diagnosis {
    Diagnosis {
        instance: json!({"alias": ALIAS, "instance_id": INSTANCE_ID}),
        since: 1_700_092_800_000,
        incomplete: false,
        runs: Vec::new(),
        errors: Vec::new(),
        error_sequences: Vec::new(),
        errors_more: false,
        errors_next: None,
        snapshot: 121_000,
        continuable: false,
        repeating: Vec::new(),
        suspended: Vec::new(),
        packs: Vec::new(),
        remedies,
        lift: Vec::new(),
        report_warnings: Vec::new(),
        report_status: json!({"status": "ok", "through_sequence": 121_000}),
        report_outcome: None,
        clock_slots: Vec::new(),
        clock_slots_total: 0,
        runs_grouped: Vec::new(),
        failures_grouped: Vec::new(),
    }
}

/// Requirement 2: the diagnosed instance's stopped remedy rounds, a stop in effect and a lifted
/// one, verbatim and in the report's order; another instance's row and an unparsed row (which
/// names no instance) are not the instance's; without an instance every row is kept. The tool's
/// description names the list.
#[test]
fn b15w2_ac_diagnose_shows_the_instances_stopped_remedies() {
    let stopped = remedy_row(ALIAS, "t_main", 130, None);
    let others = remedy_row(OTHER_ALIAS, "t_main", 125, None);
    let lifted = remedy_row(ALIAS, "t_other", 120, Some(124));
    let unparsed = json!({
        "parse_state": "unparsed",
        "signal_id": format!("signal:remedy:stopped:{}", "c".repeat(64)),
        "code": "policy_remedy_expired",
    });
    let document = report(vec![
        stopped.clone(),
        others.clone(),
        lifted.clone(),
        unparsed.clone(),
    ]);

    let instance_id = json!(INSTANCE_ID);
    let kept = remedy_rows(&document, Some((ALIAS, &instance_id)));
    assert_eq!(
        kept,
        vec![stopped.clone(), lifted.clone()],
        "the instance's stopped remedy rounds only"
    );
    assert_eq!(
        remedy_rows(&document, None),
        vec![stopped.clone(), others, lifted.clone(), unparsed],
        "every row without an instance"
    );

    let result = diagnosis(kept)
        .render_within_budget(|_| -> Result<Value, ToolError> {
            Ok(json!({"events": [], "more": false, "snapshot_ledger_position": 121_000}))
        })
        .unwrap_or_else(|error| panic!("ac_diagnose rendering: {}", error.to_value()))
        .result;
    assert_eq!(result["remedies"], json!([stopped, lifted]), "{result:#}");
    assert!(result.get("remedies_truncated").is_none(), "{result:#}");

    let listed = tools::list_json();
    let description = listed
        .as_array()
        .and_then(|tools| {
            tools
                .iter()
                .find(|tool| tool["name"] == json!("ac_diagnose"))
        })
        .and_then(|tool| tool["description"].as_str())
        .unwrap_or_default();
    assert!(
        description.contains("remedies:"),
        "ac_diagnose's description names its remedies rows: {description}"
    );
}
