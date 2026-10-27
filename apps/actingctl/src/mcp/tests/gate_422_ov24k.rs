// SPDX-License-Identifier: AGPL-3.0-only

//! #422 OV-24K (v0.12.20; brief rd-1027/brief-ov24k.md, audit
//! rd-1027/ac-audit-420-6442672243.md 二.3 and 二.1). After v0.12.19 five instances answer a full
//! `run` block each, and the ac_overview answer passed the 24 KiB output budget: the same JSON went
//! out twice (escaped as the text content and as structuredContent), `performance` wrote the same
//! dwell some 24 times, and the cut took `fact_refresh` first, the block whose rows are the
//! warnings an operator needs. `performance` and `fact_refresh` were read past the call's
//! deadline, and on the single-directory install an edited configuration actingd had not loaded
//! was never reported. The required states:
//! - O-1: five instances with full runs and performance answer within the budget, counted on the
//!   bytes sent in every protocol era, with every block;
//! - O-2: the performance block writes each distinct dwell once, and loses nothing;
//! - O-3: a spent call budget skips `performance` and `fact_refresh` with incomplete markers and
//!   the warning `call_budget_exhausted`, and asks the Runtime for neither;
//! - O-4: over the budget, the cut keeps `fact_refresh` past every less urgent block and says
//!   what it cut (`truncated`, each block's marker, the warning `output_budget_exceeded`);
//! - O-5: a configuration file whose sha256 is not the one actingd read warns
//!   `config_generation_mismatch`, on an install without generations;
//! - O-6: the text content and structuredContent follow the negotiated version: the text
//!   repeats the envelope only within the budget, else it is the summary beside
//!   structuredContent; 2025-03-26, which has no structuredContent, carries the envelope as text.
//!
//! Fix round 1 (rd-1027/brief-ov24k-fix1.md, review rd-1027/v01220-review/ov24k.md):
//! - M1: a client that reads only the text content still sees every warning, what was cut and
//!   how to narrow the call (`instance` or `sections`), and as many warnings as fit, with a count
//!   of the rest, where not all fit;
//! - L1: the overview is fitted to the protocol version the session negotiated, not the worst;
//! - L3: a spent call budget skips the not-running read too.
//!
//! The Runtime of O-3, O-5 and L3 is a fake on loopback that answers Status,
//! RuntimeFactSnapshot, MonitorStatus, PerformanceStatus and FactRefreshStatus from fixed answers
//! and lists the reads it was asked for.

use super::super::observer::{cut_to_fit, fit_answer};
use super::super::protocol::{self, Era, LegacyVersion};
use super::super::tools::{self, ToolDef, ToolSuccess, call_result, tool_call_result};
use super::gate_422_idle_ov::spent_deadline;
use super::gate_mcpt1::unreached_deadline;
use crate::codes::ActingctlCode;
use crate::overview::{self, Overview, OverviewRequest, Place, Sections, Warning};
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    ConfigSelection, EventActor, EventSource, IdentifierIssuer, OriginModule, OwnerEpoch,
    PerformanceMonitorHealth, PerformancePacingMode, PerformancePacingStage, PerformanceStageDwell,
    PerformanceViewStatus, RUNTIME_FACT_SCHEMA_VERSION, RUNTIME_INFO_FILE,
    RuntimeControlPlaneStatus, RuntimeFactRecord, RuntimeFactRefreshStatus, RuntimeFactSnapshot,
    RuntimeInfo, RuntimeInstanceStatus, RuntimeMonitorRegistryStatus, RuntimeOperation,
    RuntimePerformanceHostStatus, RuntimePerformanceInstanceStatus, RuntimePerformanceStatus,
    RuntimeReceipt, RuntimeReceiptState, RuntimeRequest, RuntimeResult,
};
use actingcommand_runtime_client::{RuntimeChannel, RuntimeClient, RuntimeClientConfig};
use serde_json::{Value, json};
use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Instant;
use tempfile::TempDir;

/// The fixtures' clock, 2100-01-01: no reading of a real clock.
const NOW_UNIX_MS: u64 = 4_102_444_800_000;
const DAY_MS: u64 = 24 * 60 * 60 * 1000;
/// O-1: the production fleet of v0.12.19, five instances each answering a full run.
const FLEET: usize = 5;
/// O-4: a larger fleet, whose answer does not fit even sent once.
const LARGE_FLEET: usize = 12;
/// The instances whose fact refresh waits, each for two keys.
const WAITING_INSTANCES: usize = 3;
/// The largest request the fake Runtime reads.
const MAX_REQUEST_BYTES: usize = 1024 * 1024;
/// One era of each result shape, and whether it carries structuredContent.
const ERAS: [(Era, bool); 4] = [
    (Era::Legacy(LegacyVersion::V20250326), false),
    (Era::Legacy(LegacyVersion::V20250618), true),
    (Era::Legacy(LegacyVersion::V20251125), true),
    (Era::Modern, true),
];
/// The blocks an overview of every section answers; `global_pause` only while scheduling is
/// paused.
const EVERY_BLOCK: [&str; 10] = [
    "daemon",
    "instances",
    "run_window",
    "lab_tool",
    "install",
    "performance",
    "catalog",
    "config_generation",
    "watchdog",
    "fact_refresh",
];

fn alias(index: usize) -> String {
    format!("neutral.instance.{index}")
}

/// One brief `actingcommand.run-status.v1`, a scheduled run that succeeded, as runtime-client
/// projects it for an overview row.
fn run(issuer: &IdentifierIssuer, instance_id: &Value, index: usize) -> Value {
    let sequence = 1_785_831 + index as u64;
    json!({
        "schema_version": "actingcommand.run-status.v1",
        "request_id": issuer.mint_request_id().expect("request id").transport(),
        "correlation_id": issuer.mint_correlation_id().expect("correlation id").transport(),
        "run_id": issuer.mint_run_id().expect("run id").transport(),
        "task_id": issuer.mint_task_id().expect("task id").transport(),
        "instance_id": instance_id,
        "dispatch": "scheduled",
        "origin": "scheduler",
        "package_label": "neutral.package.daily_routine",
        "recovery_package_labels": ["neutral.package.return_home", "neutral.package.startup"],
        "state": "succeeded",
        "terminal": {
            "outcome": "success",
            "final_page": "neutral.page.home",
            "executed_steps": 64,
            "sequence": sequence,
            "event_id": issuer.mint_event_id().expect("event id").transport(),
        },
        "lease": {
            "lease_id": issuer.mint_lease_id().expect("lease id").transport(),
            "terminal": "released",
        },
        "evidence": {
            "snapshot_ledger_position": 1_785_900,
            "admitted_sequence": sequence - 700,
            "restarted_after_admission": false,
        },
    })
}

/// One overview row as `overview::read` writes it for a busy instance: its identity, emulator
/// state, last backend self-checks, monitor, foreground package, full newest run and
/// not-running row.
fn row(issuer: &IdentifierIssuer, index: usize) -> Value {
    let instance_id = json!(issuer.mint_instance_id().expect("instance id").transport());
    let check = json!({"status": "passed", "checked_at_unix_ms": NOW_UNIX_MS - 3_600_000});
    let mut not_running = json!({
        "task_id": format!("neutral.task.daily_{index}"),
        "next_wake_unix_ms": NOW_UNIX_MS + 1_800_000,
        "since_unix_ms": NOW_UNIX_MS - 600_000,
    });
    not_running["why"] =
        json!({"code": "dispatch_instance_held", "category": "info", "values": {}});
    json!({
        "alias": alias(index),
        "instance_id": instance_id,
        "adb_port": 16_384 + 32 * index,
        "game_id": format!("neutral.game.{index}"),
        "lease_active": index == 0,
        "queued": 0,
        "emulator_state": {"state": "on", "observed_at_unix_ms": NOW_UNIX_MS - 60_000},
        "backend_selfcheck": {"capture": check, "input": check, "nemu": check},
        "monitor": {
            "policy": {"interval_ms": 30_000, "failure_threshold": 3},
            "state": {"consecutive_failures": 0, "last_check_unix_ms": NOW_UNIX_MS - 30_000},
        },
        "foreground": {
            "package": format!("com.neutral.application{index}"),
            "observed_at_unix_ms": NOW_UNIX_MS - 120_000,
        },
        "run": run(issuer, &instance_id, index),
        "not_running": not_running,
    })
}

/// The dwell of a view that spent the last 24 h, and 3 days in all, at s0, 20 min of it at s1.
fn dwell_24h() -> PerformanceStageDwell {
    PerformanceStageDwell {
        s0_ms: DAY_MS,
        ..PerformanceStageDwell::default()
    }
}

fn dwell_total() -> PerformanceStageDwell {
    PerformanceStageDwell {
        s0_ms: 3 * DAY_MS,
        s1_ms: 1_200_000,
        ..PerformanceStageDwell::default()
    }
}

fn view() -> PerformanceViewStatus {
    PerformanceViewStatus {
        pacing_stage: PerformancePacingStage::S0,
        share_milli: 1_000,
        dwell_24h: dwell_24h(),
        dwell_total: dwell_total(),
        stops_24h: 0,
        stops_total: 0,
    }
}

/// The shadow measurement's status of `instances` instances, every view at s0 with the same
/// dwell, as a host in mode observe answers it.
fn performance(instances: usize) -> RuntimePerformanceStatus {
    let allowed = u32::try_from(instances).expect("instance count");
    RuntimePerformanceStatus {
        observed_at_unix_ms: NOW_UNIX_MS,
        pacing_mode: PerformancePacingMode::Observe,
        pacing_phase: None,
        learning: false,
        monitor_health: PerformanceMonitorHealth::Healthy,
        host: RuntimePerformanceHostStatus {
            host_responsiveness_bp: Some(9_850),
            responsiveness_bp: 9_850,
            pacing_stage: PerformancePacingStage::S0,
            factor_milli: 1_000,
            learning_view: view(),
            learning_view_allowed: allowed,
            normal_view: view(),
            normal_view_allowed: allowed,
            effective_stage: PerformancePacingStage::S0,
            effective_factor_milli: 1_000,
        },
        instances: (0..instances)
            .map(|index| RuntimePerformanceInstanceStatus {
                instance_alias: alias(index),
                emulator_responsiveness_bp: Some(9_700),
                responsiveness_bp: 9_700,
                pacing_stage: PerformancePacingStage::S0,
                factor_milli: 1_000,
                learning_view: view(),
                learning_view_allowed: true,
                normal_view: view(),
                normal_view_allowed: true,
                effective_stage: PerformancePacingStage::S0,
                effective_factor_milli: 1_000,
            })
            .collect(),
        warnings: Vec::new(),
    }
}

/// What the fact refresh waits for: `WAITING_INSTANCES` instances whose two keys no reader is
/// bound for (`fact_refresh_unbound`), the warnings the overview must not cut first.
fn fact_refresh() -> RuntimeFactRefreshStatus {
    let demand = |alias: &str, key: &str| {
        let mut outcome = json!({"values": {
            "instance_id": alias,
            "fact_key": key,
            "unknown_reason": "fact_missing",
        }});
        outcome["code"] = json!("fact_refresh_unbound");
        outcome["category"] = json!("warning");
        json!({
            "fact_key": key,
            "outcome": outcome,
            "since_unix_ms": NOW_UNIX_MS - 7_200_000,
            "overdue": true,
        })
    };
    let rows = (0..WAITING_INSTANCES)
        .map(|index| {
            let alias = alias(index);
            json!({
                "instance_alias": alias,
                "demands": [
                    demand(&alias, "resource.neutral.stamina"),
                    demand(&alias, "timeline.neutral.dispatch"),
                ],
            })
        })
        .collect::<Vec<_>>();
    let status: RuntimeFactRefreshStatus = serde_json::from_value(json!({
        "cycle_seq": 412,
        "transition_seq": 37,
        "instances": rows,
    }))
    .expect("a fact refresh status");
    assert_eq!(
        status.validate(),
        Ok(()),
        "setup: a valid fact refresh status"
    );
    status
}

/// An ac_overview answer of every section, as `overview::read` assembles it, for `instances`
/// busy instances on a single-directory install (`config_generation` `{}`).
fn overview_answer(issuer: &IdentifierIssuer, instances: usize) -> Value {
    let mut decision =
        json!({"schema_version": "actingcommand.outcome.v1", "values": {"pid": 4_242}});
    decision["code"] = json!("watchdog_alive");
    decision["category"] = json!("info");
    json!({
        "daemon": {
            "online": true,
            "owner_epoch": issuer.mint_owner_epoch().expect("owner epoch").transport(),
        },
        "instances": (0..instances).map(|index| row(issuer, index)).collect::<Vec<_>>(),
        "run_window": {"since_unix_ms": NOW_UNIX_MS - DAY_MS},
        "lab_tool": {"present": true},
        "install": {
            "root": "neutral-install-root",
            "state_root": "neutral-install-root/state",
            "build": {
                "commit_sha": "0".repeat(40),
                "source_artifact_name": "actingcommand-neutral-x64",
            },
        },
        "performance": overview::performance_answer(&performance(instances)),
        "catalog": {
            "catalog_id": "neutral.catalog.composed",
            "catalog_version": 34,
            "catalog_hash": format!("sha256:{}", "a".repeat(64)),
            "activated_event_id": issuer.mint_event_id().expect("event id").transport(),
            "approval_id": "approval:neutral.catalog.composed:34",
            "approval_disposition": "approved",
            "previous_catalog_id": "neutral.catalog.composed",
            "previous_catalog_version": 33,
        },
        "config_generation": {},
        "watchdog": {
            "decision": decision,
            "attention": [],
            "starts_24h": 1,
            "last_tick": NOW_UNIX_MS - 45_000,
        },
        "fact_refresh": fact_refresh(),
    })
}

/// The warning a binding of the selected configuration leaves beside the answer.
fn read_warnings() -> Vec<Value> {
    vec![unreadable_binding(
        FLEET - 1,
        "neutral-install-root/packages/neutral-missing",
    )]
}

/// The warning `package_binding_unreadable` of instance `index`'s binding at `path`.
fn unreadable_binding(index: usize, path: &str) -> Value {
    let mut outcome = json!({
        "schema_version": "actingcommand.outcome.v1",
        "values": {"alias": alias(index), "path": path},
    });
    outcome["code"] = json!("package_binding_unreadable");
    outcome["category"] = json!("warning");
    json!({"outcome": outcome})
}

/// ac_overview's row of the tool table.
fn overview_tool() -> &'static ToolDef {
    tools::find("ac_overview").expect("ac_overview is in the tool table")
}

/// `{ok: true, result, warnings}`, the envelope a successful call carries.
fn envelope(result: &Value, warnings: &[Value]) -> Value {
    json!({"ok": true, "result": result, "warnings": warnings})
}

/// The bytes the `tools/call` result `answer` takes as sent in `era`.
fn sent_bytes(era: Era, answer: &Value) -> usize {
    protocol::complete(era, answer.clone()).to_string().len()
}

/// The text content of a `tools/call` result, parsed.
#[track_caller]
fn text_content(answer: &Value) -> Value {
    let text = answer["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("a tool result without text content: {answer}"));
    serde_json::from_str(text)
        .unwrap_or_else(|error| panic!("text content is not JSON ({error}): {answer}"))
}

/// How many dwell objects `value` holds at any depth.
fn dwell_objects(value: &Value) -> usize {
    match value {
        Value::Object(members) => {
            usize::from(members.contains_key("s0_ms"))
                + members.values().map(dwell_objects).sum::<usize>()
        }
        Value::Array(items) => items.iter().map(dwell_objects).sum(),
        _ => 0,
    }
}

/// A performance answer with each view's dwell written back in place, and no `dwell` list.
fn dwell_in_place(answer: &Value) -> Value {
    let mut expanded = answer.clone();
    let table = expanded["dwell"].as_array().cloned().expect("a dwell list");
    let restore = |row: &mut Value| {
        for view in ["learning_view", "normal_view"] {
            let view = row[view].as_object_mut().expect("a view");
            for member in ["dwell_24h", "dwell_total"] {
                let index = view
                    .remove(&format!("{member}_ref"))
                    .and_then(|index| index.as_u64())
                    .and_then(|index| usize::try_from(index).ok())
                    .expect("a dwell index");
                view.insert(member.to_owned(), table[index].clone());
            }
        }
    };
    restore(&mut expanded["host"]);
    for row in expanded["instances"].as_array_mut().expect("instance rows") {
        restore(row);
    }
    expanded.as_object_mut().expect("an object").remove("dwell");
    expanded
}

fn warning_codes(warnings: &[Warning]) -> Vec<String> {
    warnings
        .iter()
        .map(|warning| match warning {
            Warning::Outcome(outcome) => outcome.code().as_str().to_owned(),
            Warning::Runtime(error) => error.outcome().code().as_str().to_owned(),
        })
        .collect()
}

/// The value `member` of each warning, the block a `call_budget_exhausted` names.
fn warning_members(warnings: &[Warning]) -> Vec<Option<Value>> {
    warnings
        .iter()
        .map(|warning| match warning {
            Warning::Outcome(outcome) => outcome.value("member").cloned(),
            Warning::Runtime(_) => None,
        })
        .collect()
}

fn sections(names: &[&str]) -> Sections {
    Sections::parse(names.iter().copied()).expect("section names")
}

/// The overview `sections` asks for, read through `client` until `deadline`.
fn read(
    client: &RuntimeClient,
    place: &Place<'_>,
    sections: Sections,
    deadline: Instant,
) -> Overview {
    let request = OverviewRequest {
        instance: None,
        sections,
    };
    overview::read(Ok(client), &request, place, deadline, &tools::envelope)
        .unwrap_or_else(|failure| panic!("the overview was refused: {}", failure.outcome()))
}

#[test]
fn ov24k_o1_five_full_runs_and_performance_fit_the_budget_with_every_block() {
    let issuer = IdentifierIssuer::new().expect("identifier issuer");
    let result = overview_answer(&issuer, FLEET);
    let warnings = read_warnings();
    for block in EVERY_BLOCK {
        assert!(result.get(block).is_some(), "setup: no {block} block");
    }
    // Production's case: sent twice, as v0.12.19 sent it, the answer is over the budget.
    let envelope = envelope(&result, &warnings);
    let twice = json!({
        "content": [{"type": "text", "text": envelope.to_string()}],
        "structuredContent": envelope,
        "isError": false,
    });
    assert!(
        twice.to_string().len() > tools::RESULT_LIMIT,
        "setup: the answer sent twice takes {} bytes, within the budget",
        twice.to_string().len()
    );

    // It fits now, whole, in each protocol version: no block cut, no warning added.
    for (era, _) in ERAS {
        let answer = fit_answer(result.clone(), warnings.clone(), era);
        assert_eq!(answer.result, result, "{era:?}: the answer was cut");
        assert_eq!(answer.warnings, warnings, "{era:?}");
        let sent = tool_call_result(overview_tool(), Ok(answer), era);
        let bytes = sent_bytes(era, &sent);
        assert!(
            bytes <= tools::RESULT_LIMIT,
            "{era:?}: the answer takes {bytes} bytes as sent"
        );
        assert_eq!(sent["isError"], json!(false), "{era:?}");
        let carried = sent
            .get("structuredContent")
            .cloned()
            .unwrap_or_else(|| text_content(&sent));
        assert_eq!(
            carried, envelope,
            "{era:?}: the answer sent is not the whole answer"
        );
        for (index, row) in carried["result"]["instances"]
            .as_array()
            .expect("instance rows")
            .iter()
            .enumerate()
        {
            assert!(row.get("run").is_some(), "{era:?}: row {index} has no run");
        }
    }
}

#[test]
fn ov24k_o2_the_performance_block_writes_each_dwell_once() {
    let status = performance(FLEET);
    // The status itself writes the same two dwell objects 24 times: host and five instances, two
    // views, 24 h and total.
    assert_eq!(dwell_objects(&json!(status)), 24, "setup");

    let answer = overview::performance_answer(&status);
    assert_eq!(
        answer["dwell"],
        json!([dwell_24h(), dwell_total()]),
        "{answer}"
    );
    assert_eq!(dwell_objects(&answer), 2, "each dwell once: {answer}");
    for row in std::iter::once(&answer["host"]).chain(
        answer["instances"]
            .as_array()
            .expect("instance rows")
            .iter(),
    ) {
        for view in ["learning_view", "normal_view"] {
            assert_eq!(
                (&row[view]["dwell_24h_ref"], &row[view]["dwell_total_ref"]),
                (&json!(0), &json!(1)),
                "{row}"
            );
        }
    }
    // Nothing is lost: written back in place, it is the status.
    assert_eq!(dwell_in_place(&answer), json!(status));

    // A view whose dwell differs from every other keeps its own entry.
    let mut differing = status.clone();
    differing.instances[2].normal_view.dwell_total.s3_ms = 5_000;
    let answer = overview::performance_answer(&differing);
    assert_eq!(
        answer["dwell"].as_array().map(Vec::len),
        Some(3),
        "{answer}"
    );
    assert_eq!(
        answer["instances"][2]["normal_view"]["dwell_total_ref"],
        json!(2),
        "{answer}"
    );
    assert_eq!(dwell_in_place(&answer), json!(differing));
}

#[test]
fn ov24k_o3_a_spent_budget_skips_performance_and_fact_refresh_and_asks_the_runtime_for_neither() {
    let root = TempDir::new().expect("tempdir");
    let answers = Answers::busy();
    let runtime = FakeRuntime::start(root.path(), answers.clone());
    let client = connect(root.path());
    let place = Place {
        root: None,
        install_root: None,
        state_root: None,
    };
    let both = sections(&["performance", "fact_refresh"]);

    let skipped = read(&client, &place, both, spent_deadline());
    assert_eq!(runtime.asked(), vec!["status"], "only the status was read");
    let mut result = Value::Object(skipped.result);
    assert_eq!(result["daemon"]["online"], json!(true), "{result}");
    result.as_object_mut().expect("an object").remove("daemon");
    assert_eq!(
        result,
        json!({
            "performance_incomplete": true,
            "fact_refresh_incomplete": true,
            "incomplete": true,
        })
    );
    assert_eq!(
        warning_codes(&skipped.warnings),
        vec!["call_budget_exhausted".to_owned(); 2]
    );
    // Fix round 1 (L2): each warning names the block it skipped.
    assert_eq!(
        warning_members(&skipped.warnings),
        vec![Some(json!("performance")), Some(json!("fact_refresh"))]
    );

    // With budget left both are read, and the performance block writes each dwell once.
    let read_both = read(&client, &place, both, unreached_deadline());
    assert_eq!(
        runtime.asked(),
        vec![
            "status",
            "status",
            "performance_status",
            "fact_refresh_status"
        ]
    );
    let result = Value::Object(read_both.result);
    assert_eq!(
        result["performance"],
        overview::performance_answer(&answers.performance),
        "{result}"
    );
    assert_eq!(dwell_objects(&result), 2, "{result}");
    assert_eq!(
        result["fact_refresh"],
        json!(answers.fact_refresh),
        "{result}"
    );
    for member in [
        "performance_incomplete",
        "fact_refresh_incomplete",
        "incomplete",
    ] {
        assert!(result.get(member).is_none(), "{member}: {result}");
    }
    assert!(
        read_both.warnings.is_empty(),
        "{:?}",
        warning_codes(&read_both.warnings)
    );
    drop(client);
    drop(runtime);
}

#[test]
fn ov24k_o4_over_the_budget_the_cut_keeps_fact_refresh_and_names_what_it_cut() {
    let issuer = IdentifierIssuer::new().expect("identifier issuer");
    let result = overview_answer(&issuer, LARGE_FLEET);
    let warnings = read_warnings();
    // What may be cut, in order: the less urgent blocks, then the runs from the last row back.
    let order = [
        "/performance",
        "/watchdog",
        "/catalog",
        "/config_generation",
    ]
    .into_iter()
    .map(str::to_owned)
    .chain(
        (0..LARGE_FLEET)
            .rev()
            .map(|index| format!("/instances/{index}/run")),
    )
    .collect::<Vec<_>>();
    // Fix round 1 (L1): fitted to each protocol version as the session negotiated it.
    for (era, _) in ERAS {
        assert!(
            !tools::fits_sent(&result, &warnings, era),
            "setup: {era:?}: {LARGE_FLEET} instances fit the budget"
        );
        let whole = tools::sent_size(&result, &warnings, era);

        let answer = fit_answer(result.clone(), warnings.clone(), era);
        assert!(
            tools::fits_sent(&answer.result, &answer.warnings, era),
            "{era:?}: the cut answer does not fit"
        );
        // Never silently: after the read's own warnings, output_budget_exceeded with the whole
        // answer's size and the budget.
        assert_eq!(
            answer.warnings.len(),
            warnings.len() + 1,
            "{era:?}: {:?}",
            answer.warnings
        );
        assert_eq!(answer.warnings[..warnings.len()], warnings[..], "{era:?}");
        let budget = &answer.warnings[warnings.len()]["outcome"];
        assert_eq!(
            (
                &budget["code"],
                &budget["values"]["actual_bytes"],
                &budget["values"]["limit_bytes"]
            ),
            (
                &json!("output_budget_exceeded"),
                &json!(whole),
                &json!(tools::RESULT_LIMIT)
            ),
            "{era:?}: {budget}"
        );

        // What was cut, in that order and nothing else.
        let truncated = answer.result["truncated"]
            .as_array()
            .expect("the list of what was cut")
            .iter()
            .map(|pointer| pointer.as_str().expect("a JSON pointer").to_owned())
            .collect::<Vec<_>>();
        assert!(
            !truncated.is_empty() && truncated.len() <= order.len(),
            "{era:?}: {truncated:?}"
        );
        assert_eq!(
            truncated[..],
            order[..truncated.len()],
            "{era:?}: {truncated:?}"
        );
        // Each cut block left its marker where it was.
        for pointer in &truncated {
            let (place, block) = pointer.rsplit_once('/').expect("a JSON pointer");
            let parent = answer.result.pointer(place).unwrap_or(&answer.result);
            assert_eq!(
                (
                    parent.get(block),
                    &parent[format!("{block}_truncated").as_str()]
                ),
                (None, &json!(true)),
                "{era:?}: {pointer}"
            );
        }
        // The warnings an operator needs are whole.
        assert_eq!(
            answer.result["fact_refresh"], result["fact_refresh"],
            "{era:?}"
        );
    }

    // And fact_refresh outlasts every other block: cut to nothing, it goes last.
    let mut everything = result;
    cut_to_fit(&mut everything, |_| false);
    assert_eq!(
        everything["truncated"]
            .as_array()
            .and_then(|cut| cut.last()),
        Some(&json!("/fact_refresh")),
        "{}",
        everything["truncated"]
    );
}

#[test]
fn ov24k_o5_a_configuration_file_actingd_has_not_loaded_warns_on_an_install_without_generations() {
    // The single-directory install: its root holds actingd.config.json and no install\active.json.
    let install = TempDir::new().expect("tempdir");
    let config = install.path().join("actingd.config.json");
    let loaded = br#"{"instances": [{"alias": "node-a", "instance_id": "neutral-a"}]}"#;
    fs::write(&config, loaded).expect("write the configuration");
    let state = TempDir::new().expect("tempdir");
    let mut answers = Answers::busy();
    answers.facts.records = vec![
        ConfigSelection {
            config_generation: None,
            config_sha256: actingcommand_runtime_config::sha256_text(loaded),
            defaults_sha256: format!("sha256:{}", "c".repeat(64)),
        }
        .to_fact_record(NOW_UNIX_MS, OriginModule::Runtime),
    ];
    let runtime = FakeRuntime::start(state.path(), answers);
    let client = connect(state.path());
    let place = Place {
        root: Some(install.path()),
        install_root: Some(install.path()),
        state_root: Some(state.path()),
    };
    let generation = sections(&["config_generation"]);

    // The file actingd read: no warning.
    let unchanged = read(&client, &place, generation, unreached_deadline());
    assert_eq!(
        Value::Object(unchanged.result.clone())["config_generation"],
        json!({})
    );
    assert!(
        unchanged.warnings.is_empty(),
        "{:?}",
        warning_codes(&unchanged.warnings)
    );

    // Edited since (a second instance): actingd has not loaded it, and the overview says a
    // restart is needed.
    fs::write(
        &config,
        br#"{"instances": [{"alias": "node-a", "instance_id": "neutral-a"}, {"alias": "node-b", "instance_id": "neutral-b"}]}"#,
    )
    .expect("edit the configuration");
    let edited = read(&client, &place, generation, unreached_deadline());
    assert_eq!(
        Value::Object(edited.result.clone())["config_generation"],
        json!({})
    );
    let expected = Outcome::new(ActingctlCode::ConfigGenerationMismatch).path(&config);
    assert!(
        matches!(
            edited.warnings.as_slice(),
            [Warning::Outcome(warning)]
                if warning.is(ActingctlCode::ConfigGenerationMismatch)
                    && warning.value("path") == expected.value("path")
        ),
        "{:?}",
        warning_codes(&edited.warnings)
    );
    assert_eq!(
        runtime.asked(),
        vec![
            "status",
            "runtime_fact_snapshot",
            "status",
            "runtime_fact_snapshot"
        ]
    );
    drop(client);
    drop(runtime);
}

#[test]
fn ov24k_o6_text_and_structured_content_follow_the_negotiated_version() {
    let issuer = IdentifierIssuer::new().expect("identifier issuer");
    let large = overview_answer(&issuer, FLEET);
    let warnings = read_warnings();
    let large_envelope = envelope(&large, &warnings);
    let small = json!({"daemon": {"online": true}});
    let small_envelope = json!({"ok": true, "result": small});
    for (era, structured) in ERAS {
        // Over the budget twice, within it once: 2025-03-26 carries it once, as the text; a
        // later version as structuredContent beside the summary text.
        let answer = call_result(
            Ok(ToolSuccess {
                result: large.clone(),
                warnings: warnings.clone(),
            }),
            era,
        );
        let bytes = sent_bytes(era, &answer);
        assert!(bytes <= tools::RESULT_LIMIT, "{era:?}: {bytes} bytes");
        assert_eq!(answer["isError"], json!(false), "{era:?}");
        assert_eq!(answer["content"][0]["type"], json!("text"), "{era:?}");
        if structured {
            assert_eq!(answer["structuredContent"], large_envelope, "{era:?}");
            // Fix round 1 (M1): the summary also carries the warnings and how to narrow the call.
            assert_eq!(
                text_content(&answer),
                json!({
                    "ok": true,
                    "structured_content_only": ["result", "warnings"],
                    "warnings": warnings,
                    "narrow": tools::NARROW_HINT,
                }),
                "{era:?}"
            );
        } else {
            assert!(answer.get("structuredContent").is_none(), "{era:?}");
            assert_eq!(text_content(&answer), large_envelope, "{era:?}");
        }

        // Within the budget twice: the text repeats the envelope, as before.
        let answer = call_result(Ok(ToolSuccess::new(small.clone())), era);
        assert_eq!(
            answer["content"][0]["text"],
            json!(small_envelope.to_string()),
            "{era:?}"
        );
        assert_eq!(
            answer.get("structuredContent"),
            structured.then_some(&small_envelope),
            "{era:?}"
        );
    }
}

#[test]
fn ov24k_fix1_m1_a_text_only_client_sees_every_warning_what_was_cut_and_how_to_narrow() {
    let issuer = IdentifierIssuer::new().expect("identifier issuer");
    assert!(
        tools::OVERVIEW_NARROW_HINT.contains("instance")
            && tools::OVERVIEW_NARROW_HINT.contains("sections"),
        "{}",
        tools::OVERVIEW_NARROW_HINT
    );
    for (era, structured) in ERAS {
        if !structured {
            // 2025-03-26 has no structuredContent: its text is the envelope itself (O-6).
            continue;
        }
        // Production's five instances, sent once beside the summary, and twelve, cut as well.
        for (instances, cut) in [(FLEET, false), (LARGE_FLEET, true)] {
            let answer = fit_answer(overview_answer(&issuer, instances), read_warnings(), era);
            assert_eq!(
                answer.result.get("truncated").is_some(),
                cut,
                "setup: {era:?}, {instances} instances"
            );
            let mut expected = json!({
                "ok": true,
                "structured_content_only": ["result", "warnings"],
                "warnings": answer.warnings,
                "narrow": tools::OVERVIEW_NARROW_HINT,
            });
            if cut {
                expected["truncated"] = answer.result["truncated"].clone();
            }
            let whole = envelope(&answer.result, &answer.warnings);
            let sent = tool_call_result(overview_tool(), Ok(answer), era);
            let bytes = sent_bytes(era, &sent);
            assert!(
                bytes <= tools::RESULT_LIMIT,
                "{era:?}, {instances} instances: {bytes} bytes"
            );
            assert_eq!(sent["isError"], json!(false), "{era:?}");
            assert_eq!(
                sent["structuredContent"], whole,
                "{era:?}, {instances} instances"
            );
            // The text alone lists every warning (output_budget_exceeded too, when cut), what was
            // cut, and how to have the whole answer.
            assert_eq!(
                text_content(&sent),
                expected,
                "{era:?}, {instances} instances"
            );
        }

        // Warnings too large to repeat whole: as many as fit, in order, and how many did not.
        let small = json!({"daemon": {"online": true}});
        let warnings = vec![
            unreadable_binding(0, "neutral-install-root/packages/neutral-a"),
            unreadable_binding(1, "neutral-install-root/packages/neutral-b"),
            unreadable_binding(
                2,
                &format!("neutral-install-root/packages/{}", "n".repeat(14_000)),
            ),
            unreadable_binding(3, "neutral-install-root/packages/neutral-d"),
        ];
        let sent = tool_call_result(
            overview_tool(),
            Ok(ToolSuccess {
                result: small.clone(),
                warnings: warnings.clone(),
            }),
            era,
        );
        let bytes = sent_bytes(era, &sent);
        assert!(bytes <= tools::RESULT_LIMIT, "{era:?}: {bytes} bytes");
        assert_eq!(sent["isError"], json!(false), "{era:?}");
        assert_eq!(
            sent["structuredContent"],
            envelope(&small, &warnings),
            "{era:?}"
        );
        assert_eq!(
            text_content(&sent),
            json!({
                "ok": true,
                "structured_content_only": ["result", "warnings"],
                "warnings": warnings[..2].to_vec(),
                "warnings_omitted": 2,
                "narrow": tools::OVERVIEW_NARROW_HINT,
            }),
            "{era:?}"
        );
    }
}

#[test]
fn ov24k_fix1_l1_the_overview_is_fitted_to_the_version_the_session_negotiated() {
    let issuer = IdentifierIssuer::new().expect("identifier issuer");
    let mut result = overview_answer(&issuer, FLEET);
    let warnings = read_warnings();
    // Grow the block cut first by quotes until the answer takes the whole budget as 2026-07-28
    // sends it: structuredContent escapes a quote once, the 2025-03-26 text twice.
    result["performance"]["padding"] = json!("");
    let base = tools::sent_size(&result, &warnings, Era::Modern);
    assert!(base < tools::RESULT_LIMIT, "setup: {base} bytes");
    result["performance"]["padding"] = json!("\"".repeat((tools::RESULT_LIMIT - base) / 2));
    for (era, structured) in ERAS {
        assert_eq!(
            tools::fits_sent(&result, &warnings, era),
            structured,
            "setup: {era:?} takes {} bytes",
            tools::sent_size(&result, &warnings, era)
        );
    }

    for (era, structured) in ERAS {
        let answer = fit_answer(result.clone(), warnings.clone(), era);
        if structured {
            // Sent whole where it fits as negotiated, not cut to the 2025-03-26 size.
            assert_eq!(answer.result, result, "{era:?}: the answer was cut");
            assert_eq!(answer.warnings, warnings, "{era:?}");
        } else {
            assert!(
                answer.result["truncated"]
                    .as_array()
                    .is_some_and(|cut| cut.first() == Some(&json!("/performance"))),
                "{era:?}: {}",
                answer.result["truncated"]
            );
            assert_eq!(
                answer.warnings.last().map(|last| &last["outcome"]["code"]),
                Some(&json!("output_budget_exceeded")),
                "{era:?}"
            );
        }
        let sent = tool_call_result(overview_tool(), Ok(answer), era);
        let bytes = sent_bytes(era, &sent);
        assert!(bytes <= tools::RESULT_LIMIT, "{era:?}: {bytes} bytes");
        assert_eq!(sent["isError"], json!(false), "{era:?}");
    }
}

#[test]
fn ov24k_fix1_l3_a_spent_budget_skips_the_not_running_read() {
    let root = TempDir::new().expect("tempdir");
    let runtime = FakeRuntime::start(root.path(), Answers::busy());
    let client = connect(root.path());
    let place = Place {
        root: None,
        install_root: None,
        state_root: None,
    };

    let skipped = read(&client, &place, sections(&["instances"]), spent_deadline());
    // No ScheduleExplanation (nor a run listing) was asked for.
    assert_eq!(
        runtime.asked(),
        vec!["status", "runtime_fact_snapshot", "monitor_status"]
    );
    let result = Value::Object(skipped.result);
    let rows = result["instances"].as_array().expect("instance rows");
    assert_eq!(rows.len(), FLEET, "{result}");
    for row in rows {
        assert_eq!(row.get("not_running"), None, "{row}");
        assert_eq!(row["not_running_incomplete"], json!(true), "{row}");
    }
    assert_eq!(result["incomplete"], json!(true), "{result}");
    // One call_budget_exhausted names the not-running block; the others name each row's run.
    let not_running = skipped
        .warnings
        .iter()
        .filter(|warning| {
            matches!(
                warning,
                Warning::Outcome(outcome)
                    if outcome.is(ActingctlCode::CallBudgetExhausted)
                        && outcome.value("member") == Some(&json!("not_running"))
            )
        })
        .count();
    assert_eq!(
        not_running,
        1,
        "{:?} {:?}",
        warning_codes(&skipped.warnings),
        warning_members(&skipped.warnings)
    );
    drop(client);
    drop(runtime);
}

// ---- The fake Runtime -------------------------------------------------------------------------

/// What the fake Runtime answers: the instances of its status, its program facts, its shadow
/// measurement's status and what its fact refresh waits for.
#[derive(Clone)]
struct Answers {
    instances: Vec<RuntimeInstanceStatus>,
    facts: RuntimeFactSnapshot,
    performance: RuntimePerformanceStatus,
    fact_refresh: RuntimeFactRefreshStatus,
}

impl Answers {
    /// The production fleet: five instances, every view at s0, three of them waiting.
    fn busy() -> Self {
        let issuer = IdentifierIssuer::new().expect("identifier issuer");
        Self {
            instances: (0..FLEET)
                .map(|index| {
                    RuntimeInstanceStatus::new(
                        alias(index),
                        *issuer.mint_instance_id().expect("instance id").transport(),
                        false,
                        0,
                        false,
                        false,
                        false,
                    )
                    .expect("instance status")
                })
                .collect(),
            facts: RuntimeFactSnapshot {
                schema_version: RUNTIME_FACT_SCHEMA_VERSION.to_owned(),
                ledger_position: 1,
                taken_at_unix_ms: NOW_UNIX_MS,
                snapshot_id: 1,
                part: 1,
                parts: 1,
                records: Vec::<RuntimeFactRecord>::new(),
            },
            performance: performance(FLEET),
            fact_refresh: fact_refresh(),
        }
    }
}

/// A Runtime on loopback that answers every connection from `Answers` until dropped, and lists
/// the reads it was asked for.
struct FakeRuntime {
    address: SocketAddr,
    stopping: Arc<AtomicBool>,
    asked: Arc<Mutex<Vec<&'static str>>>,
    acceptor: Option<JoinHandle<()>>,
}

impl FakeRuntime {
    /// Listens on loopback and writes `runtime-info.json` under `root` naming it.
    fn start(root: &Path, answers: Answers) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind the fake Runtime");
        let address = listener.local_addr().expect("fake Runtime address");
        let owner_epoch = *IdentifierIssuer::new()
            .expect("identifier issuer")
            .mint_owner_epoch()
            .expect("owner epoch")
            .transport();
        let info = RuntimeInfo::new(
            std::process::id(),
            address.ip().to_string(),
            address.port(),
            owner_epoch,
            1,
        )
        .expect("fake Runtime info");
        fs::write(
            root.join(RUNTIME_INFO_FILE),
            serde_json::to_vec(&info).expect("encode the Runtime info"),
        )
        .expect("write the Runtime info");
        let status = RuntimeControlPlaneStatus::new(owner_epoch, answers.instances.clone())
            .expect("fake status");
        let stopping = Arc::new(AtomicBool::new(false));
        let asked = Arc::new(Mutex::new(Vec::new()));
        let acceptor = {
            let stopping = Arc::clone(&stopping);
            let asked = Arc::clone(&asked);
            let served = Arc::new((answers, status));
            thread::spawn(move || {
                for stream in listener.incoming() {
                    if stopping.load(Ordering::SeqCst) {
                        return;
                    }
                    let stream = stream.expect("accept a client");
                    let served = Arc::clone(&served);
                    let asked = Arc::clone(&asked);
                    thread::spawn(move || {
                        serve(stream, owner_epoch, &served.0, &served.1, &asked);
                    });
                }
            })
        };
        Self {
            address,
            stopping,
            asked,
            acceptor: Some(acceptor),
        }
    }

    /// The reads asked for so far, in order (the handshake not counted).
    fn asked(&self) -> Vec<&'static str> {
        self.asked
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl Drop for FakeRuntime {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::SeqCst);
        // Wakes the acceptor, which then sees `stopping`.
        let _ = TcpStream::connect(self.address);
        if let Some(acceptor) = self.acceptor.take() {
            let _ = acceptor.join();
        }
    }
}

/// Answers one client's requests until it hangs up.
fn serve(
    mut stream: TcpStream,
    owner_epoch: OwnerEpoch,
    answers: &Answers,
    status: &RuntimeControlPlaneStatus,
    asked: &Mutex<Vec<&'static str>>,
) {
    let record = |read: &'static str| {
        asked
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(read);
    };
    while let Some(request) = next_request(&mut stream) {
        let result = match request.operation() {
            RuntimeOperation::Health => RuntimeResult::Health { owner_epoch },
            RuntimeOperation::BindChannel { channel, via } => RuntimeResult::ChannelBound {
                channel: *channel,
                via: *via,
            },
            RuntimeOperation::Status => {
                record("status");
                RuntimeResult::Status {
                    status: status.clone(),
                }
            }
            RuntimeOperation::RuntimeFactSnapshot => {
                record("runtime_fact_snapshot");
                RuntimeResult::RuntimeFactSnapshot {
                    snapshot: answers.facts.clone(),
                }
            }
            RuntimeOperation::MonitorStatus => {
                record("monitor_status");
                RuntimeResult::MonitorStatus {
                    status: RuntimeMonitorRegistryStatus::new(owner_epoch, Vec::new())
                        .expect("fake monitor status"),
                }
            }
            RuntimeOperation::PerformanceStatus => {
                record("performance_status");
                RuntimeResult::PerformanceStatus {
                    status: Box::new(answers.performance.clone()),
                }
            }
            RuntimeOperation::FactRefreshStatus => {
                record("fact_refresh_status");
                RuntimeResult::FactRefreshStatus {
                    status: Box::new(answers.fact_refresh.clone()),
                }
            }
            other => panic!("the fake Runtime answers no {other:?}"),
        };
        let receipt =
            RuntimeReceipt::success(&request, RuntimeReceiptState::Completed, None, result)
                .expect("fake receipt");
        let body = serde_json::to_vec(&receipt).expect("encode the receipt");
        let length = u32::try_from(body.len()).expect("receipt length");
        stream
            .write_all(&length.to_be_bytes())
            .and_then(|()| stream.write_all(&body))
            .and_then(|()| stream.flush())
            .expect("write the receipt");
    }
}

/// The next request, or `None` once the client hung up.
fn next_request(stream: &mut TcpStream) -> Option<RuntimeRequest> {
    let mut header = [0_u8; 4];
    match stream.read_exact(&mut header) {
        Ok(()) => {}
        Err(error)
            if matches!(
                error.kind(),
                ErrorKind::UnexpectedEof
                    | ErrorKind::ConnectionReset
                    | ErrorKind::ConnectionAborted
            ) =>
        {
            return None;
        }
        Err(error) => panic!("fake Runtime request header: {error}"),
    }
    let length = u32::from_be_bytes(header) as usize;
    assert!(
        length > 0 && length <= MAX_REQUEST_BYTES,
        "fake Runtime request length {length}"
    );
    let mut body = vec![0_u8; length];
    stream
        .read_exact(&mut body)
        .expect("fake Runtime request body");
    Some(serde_json::from_slice(&body).expect("decode a request"))
}

/// A CLI client of the fake Runtime under `root`.
fn connect(root: &Path) -> RuntimeClient {
    RuntimeClient::connect(RuntimeClientConfig::new(
        root,
        RuntimeChannel::Cli,
        EventActor::Cli,
        EventSource::Cli,
    ))
    .expect("connect to the fake Runtime")
}
