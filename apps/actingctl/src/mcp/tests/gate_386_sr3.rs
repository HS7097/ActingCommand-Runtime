// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #386 SR-3, test row T13 (host gate 92; frozen scheduling model v1.4.2 §2.4 item 5;
//! L726 "向外部例如MCP/CLI/人类发出提示"; SPEC §9), the MCP half: ac_diagnose carries the
//! `packs` rows of `actingd suspended` whose suspension defers a pair of the diagnosed instance
//! (every row without an instance), verbatim, and every member it shows is a code, an id or a
//! hash, never a sentence.

use super::super::runs::Diagnosis;
use super::super::tools::ToolError;
use crate::diagnose::pack_rows;
use serde_json::{Value, json};

const ALIAS: &str = "fixture-instance-a";
const OTHER_ALIAS: &str = "fixture-instance-b";
const INSTANCE_ID: &str = "instance_0192f3a4-5b6c-7d8e-9f01-23456789abcd";

/// A `packs` row of `actingd suspended` for a package suspended on `instances`.
fn pack_row(package_id: &str, instances: &[&str]) -> Value {
    json!({
        "package_id": package_id,
        "failure_code": "recognition_sample_budget_invalid",
        "runtime_commit": "0123456789abcdef0123456789abcdef01234567",
        "paused_hash_prefix": "0a1b2c3d4e5f",
        "current_hash_prefix": "0a1b2c3d4e5f",
        "instances": instances,
    })
}

/// The `actingd suspended` document with `packs`.
fn report(packs: Vec<Value>) -> Value {
    json!({
        "schema_version": "actingcommand.actingd.suspended.v2",
        "suspended": [],
        "lifted": [],
        "repeating": [],
        "warnings": [],
        "clock_slots": [],
        "packs": packs,
    })
}

/// A diagnosis with only the `packs` rows given.
fn diagnosis(packs: Vec<Value>) -> Diagnosis {
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
        packs,
        remedies: Vec::new(),
        lift: Vec::new(),
        report_warnings: Vec::new(),
        report_status: json!({"status": "ok", "through_sequence": 121_000}),
        report_outcome: None,
        clock_slots: Vec::new(),
        clock_slots_total: 0,
        runs_grouped: Vec::new(),
        failures_grouped: Vec::new(),
        // Workflow #386 SR-6b, MCP-19b: no deferred rows read, nothing warned.
        not_run: None,
        warnings: Vec::new(),
    }
}

/// Every string a value holds, at any depth.
fn strings(value: &Value, found: &mut Vec<String>) {
    match value {
        Value::String(text) => found.push(text.clone()),
        Value::Array(items) => items.iter().for_each(|item| strings(item, found)),
        Value::Object(members) => members.values().for_each(|member| strings(member, found)),
        _ => {}
    }
}

#[test]
fn sr3_t13_ac_diagnose_carries_the_instances_suspended_packages_as_codes_and_ids() {
    let ours = pack_row("neutral.suspended.task", &[ALIAS, OTHER_ALIAS]);
    let others = pack_row("neutral.other.task", &[OTHER_ALIAS]);
    let document = report(vec![ours.clone(), others.clone()]);

    let instance_id = json!(INSTANCE_ID);
    let kept = pack_rows(&document, Some((ALIAS, &instance_id)));
    assert_eq!(
        kept,
        vec![ours.clone()],
        "the instance's suspended packages only"
    );
    assert_eq!(
        pack_rows(&document, None),
        vec![ours.clone(), others],
        "every suspended package without an instance"
    );

    let result = diagnosis(kept)
        .render_within_budget(|_| -> Result<Value, ToolError> {
            Ok(json!({"events": [], "more": false, "snapshot_ledger_position": 121_000}))
        })
        .unwrap_or_else(|error| panic!("ac_diagnose rendering: {}", error.to_value()))
        .result;
    assert_eq!(result["packs"], json!([ours]), "{result:#}");
    let mut found = Vec::new();
    strings(&result["packs"], &mut found);
    assert!(
        found
            .iter()
            .all(|text| !text.is_empty() && !text.contains(char::is_whitespace)),
        "a packs member carries a sentence: {found:?}"
    );
}
