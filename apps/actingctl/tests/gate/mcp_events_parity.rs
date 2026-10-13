// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #389 MCP-1 (MCP-0 command table v1.1 §2.3, §4.1): the parity test of
//! `actingctl events` and `ac_events`. On one c4 fake Runtime child
//! (`tests/support/c4_runtime.rs`), the same parameters go once to the real `actingctl events`
//! and once to the real `actingctl mcp-serve` over pipes (fixture I5, `mcp_harness`). The
//! expected value is the table's: the CLI's stdout JSON equals the MCP `result`, minus the
//! presentation members of §2.2. The page read here holds every row of its range and fits the
//! MCP output budget, so it has none of those members and the two compare whole. A last CLI read
//! of the same range shows that a filter takes effect (`exclude_event_types`), not only that both
//! sides apply it alike.

use crate::c4_runtime::{RuntimeChild, write_sealed_frame};
use crate::mcp_harness::{McpServer, accepted};
use actingcommand_contract::{
    EventActor, EventQuery, EventSource, ProjectionProfile, SchedulingPauseReason,
    SchedulingPauseScope,
};
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientConfig};
use serde_json::{Map, Value, json};
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

const CHILD_ENTRY: &str = "c4_runtime_child_process";
const ALIAS: &str = "neutral.parity";

#[test]
fn gate_actingctl_events_prints_the_ac_events_result_of_the_same_parameters() {
    let root = TempDir::new().expect("tempdir");
    write_sealed_frame(&root.path().join("sealed.png"));
    let mut runtime = RuntimeChild::spawn_for_instance(root.path(), CHILD_ENTRY, ALIAS);
    runtime.wait_ready(root.path());

    // One instance pause, lifted again, on a test-side (Cli, Cli) connection: the rows of the
    // range both reads ask for.
    let ledger = RuntimeClient::connect(RuntimeClientConfig::new(
        root.path(),
        EventActor::Cli,
        EventSource::Cli,
    ))
    .expect("connect a ledger client");
    let instance_id = ledger
        .status()
        .expect("read the Runtime status")
        .instances()
        .iter()
        .find(|instance| instance.instance_alias() == ALIAS)
        .expect("setup: the fake instance is not registered")
        .instance_id();
    let before = ledger
        .query_events(EventQuery::default(), ProjectionProfile::Forensic)
        .expect("read the ledger")
        .last()
        .map_or(0, |event| event.sequence);
    let scope = SchedulingPauseScope::Instance {
        instance_alias: ALIAS.to_owned(),
    };
    ledger
        .pause_scheduling(
            scope.clone(),
            SchedulingPauseReason::new("gate.parity").expect("the pause reason"),
            60_000,
        )
        .expect("pause the instance");
    ledger
        .resume_scheduling(scope)
        .expect("resume the instance");
    let range = ledger
        .query_events(
            EventQuery {
                from_sequence: Some(before + 1),
                ..EventQuery::default()
            },
            ProjectionProfile::Forensic,
        )
        .expect("read the range");
    drop(ledger);
    let through = range
        .last()
        .expect("setup: the pause and the resume left no rows")
        .sequence;
    // The type and module of the instance's last row there: a page of a few rows, well inside
    // the MCP output budget even with payloads.
    let last_of_instance = range
        .iter()
        .rev()
        .find(|event| event.links.instance_id() == Some(&instance_id))
        .expect("setup: no row of the range names the instance");
    let event_type = json!(last_of_instance.event_type);
    let origin_module = last_of_instance.origin.module().as_str();

    // The same parameters for both, by their MCP names; the CLI flags are the same names with
    // `--` and `-` for `_`, a list comma-separated (#389 R2).
    let parameters = [
        ("instance", json!(ALIAS)),
        ("after_sequence", json!(before)),
        ("through_sequence", json!(through)),
        ("event_type", event_type.clone()),
        ("origin_module", json!(origin_module)),
        ("exclude_event_types", json!(["runtime.started"])),
        ("detail", json!("payload")),
        ("limit", json!(20)),
    ];
    let arguments = Value::Object(
        parameters
            .iter()
            .map(|(name, value)| ((*name).to_owned(), value.clone()))
            .collect::<Map<_, _>>(),
    );

    let mut mcp = McpServer::spawn_with_tier(root.path(), None);
    mcp.initialize();
    // Every Status the instance selector sends appends one ledger record. The server resolves
    // the alias once per Runtime connection (§2.5): after this first call it sends none, while
    // the CLI process sends its one Status before it reads. The CLI and the next call then read
    // at the same ledger head.
    accepted(
        &mcp.call_tool(2, "ac_events", arguments.clone()),
        "ac_events",
    );
    let printed = cli_events(root.path(), &parameters);
    let answer = mcp.call_tool(3, "ac_events", arguments);
    let result = accepted(&answer, "ac_events");
    assert!(
        result["events"]
            .as_array()
            .is_some_and(|rows| !rows.is_empty()),
        "setup: the ac_events page is empty or was exported: {answer}"
    );
    assert!(
        result.get("next_cursor").is_none(),
        "setup: the range does not fit one page: {answer}"
    );
    assert_eq!(
        &printed, result,
        "actingctl events printed another page than ac_events answered for the same parameters"
    );

    // A filter takes effect, not only alike on both sides (§4.1: exclude_event_types leaves
    // those types out). Over the same instance and range the instance's last row, of this
    // type, would be on the page; with the type excluded no row of it is.
    let excluding = cli_events(
        root.path(),
        &[
            ("instance", json!(ALIAS)),
            ("after_sequence", json!(before)),
            ("through_sequence", json!(through)),
            ("exclude_event_types", json!([event_type.clone()])),
            ("limit", json!(256)),
        ],
    );
    assert!(
        excluding.get("next_cursor").is_none(),
        "setup: the range does not fit one CLI page: {excluding}"
    );
    let left = excluding["events"]
        .as_array()
        .unwrap_or_else(|| panic!("actingctl events printed no events member: {excluding}"));
    assert!(
        left.iter().all(|row| row["type"] != event_type),
        "exclude_event_types [{event_type}] left rows of that type on the page: {excluding}"
    );

    mcp.close_input();
    runtime.stop_clean();
    drop(mcp);
}

/// `actingctl events --state-root <root>` with `parameters` as flags; its stdout JSON.
fn cli_events(state_root: &Path, parameters: &[(&str, Value)]) -> Value {
    let mut command = Command::new(env!("CARGO_BIN_EXE_actingctl"));
    command.arg("events").arg("--state-root").arg(state_root);
    for (name, value) in parameters {
        let text = match value {
            Value::String(text) => text.clone(),
            Value::Array(items) => items
                .iter()
                .map(|item| item.as_str().expect("a list of strings"))
                .collect::<Vec<_>>()
                .join(","),
            other => other.to_string(),
        };
        command
            .arg(format!("--{}", name.replace('_', "-")))
            .arg(text);
    }
    let output = command.output().expect("run actingctl events");
    assert!(
        output.status.success(),
        "actingctl events exited with {}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "actingctl events printed no JSON ({error}): {}",
            String::from_utf8_lossy(&output.stdout)
        )
    })
}
