// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #469 F1 (register §1.387; contracts/runtime-state-observation.md, "Contained-run
//! queries"). A reason word beside a code reached none of the reads a person or an agent looks
//! at first: a run's status held its terminal outcome's code only. The required state, the faces:
//! each shows, beside the code it shows, the values codes mode keeps of the run's terminal
//! outcome (`failure_values`), here a word of a vocabulary (`receipt_state`), and not the id
//! (`lease_id`) or the duration (`retry_after_ms`) the same outcome carries:
//! - `ac_get_run`: the full status's terminal (the read and the answer are `run_show`'s, which
//!   actinglab's `run show` answers too);
//! - `ac_runs` (`actingctl runs`): the row, beside its `failure_code`;
//! - the overview's run block: the instance row's `run`, a brief status;
//! - the diagnosis: each `runs` row, a brief status, while `runs_grouped` keeps grouping by the
//!   code alone.
//!
//! The Runtime is A42's fake (`gate_411_a42`), which answers Health, BindChannel, Status and the
//! event queries from one ledger, whatever their order; nothing here reads a clock or sleeps.

use super::super::jobs::Jobs;
use super::super::protocol::Era;
use super::super::run_list as runs_tool;
use super::super::runs;
use super::super::runtime::RuntimeAccess;
use super::super::tools::ToolContext;
use super::gate_411_a42::{FakeRuntime, Recorder};
use super::gate_mcpt1::unreached_deadline;
use crate::codes::ActingctlCode;
use crate::diagnose::{self, DiagnoseRequest, SuspensionReport};
use crate::overview::{self, RunWindow};
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    AuditInput, EventActor, EventLinksDraft, EventOrigin, EventSource, IssuedInstanceId,
    OriginModule, ProjectedEvent, RuntimeErrorCode, RuntimeInstanceStatus, RuntimeReceiptState,
    TaskOutcome, TaskPayloadDraft, TaskSemanticFact,
};
use actingcommand_runtime_client::{RuntimeChannel, RuntimeClient, RuntimeClientConfig};
use serde_json::{Map, Value, json};
use std::path::Path;
use std::sync::atomic::AtomicBool;
use tempfile::TempDir;

/// The run window: from the ledger's first record on (2100-01-01 plus 4 000 000 s), 24 h.
const WINDOW_START_UNIX_MS: u64 = 4_102_444_800_000 + 4_000_000_000;
const WINDOW_END_UNIX_MS: u64 = WINDOW_START_UNIX_MS + 24 * 60 * 60 * 1000;
const FAILURE: RuntimeErrorCode = RuntimeErrorCode::ContainedTaskDeadlineExceeded;
const FINAL_PAGE: &str = "neutral/page_failed";

/// The failed run's terminal outcome: a vocabulary word codes mode keeps, an id and a duration
/// it drops.
fn failure() -> Outcome {
    Outcome::new(FAILURE)
        .receipt_state(RuntimeReceiptState::Failed)
        .lease_id("lease_00000000000000000000000000000001")
        .retry_after_ms(1_500_u64)
}

/// What every face shows of it: the word alone.
fn kept() -> Value {
    json!({"receipt_state": "failed"})
}

/// One failed run of one instance in the window: its main admission and its terminal. The
/// instance, the ledger and the run's request_id.
fn ledger() -> (IssuedInstanceId, Vec<ProjectedEvent>, Value) {
    let mut ledger = Recorder::new(WINDOW_START_UNIX_MS);
    let issuer = &ledger.issuer;
    let instance = issuer.mint_instance_id().expect("instance id");
    let request = issuer.mint_request_id().expect("request id");
    let links = EventLinksDraft::default()
        .with_instance_id(instance)
        .with_request_id(request)
        .with_correlation_id(issuer.mint_correlation_id().expect("correlation id"))
        .with_task_id(issuer.mint_task_id().expect("task id"))
        .with_run_id(issuer.mint_run_id().expect("run id"));
    let origin = || {
        EventOrigin::new(
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
        )
    };
    ledger.push(
        origin(),
        links.clone(),
        TaskPayloadDraft::semantic(
            TaskSemanticFact::PackageAdmitted {
                package_label: "neutral.package".to_owned(),
                task_label: "neutral.task".to_owned(),
                response_deadline_monotonic_ms: None,
            },
            AuditInput::new(),
        )
        .into(),
    );
    ledger.push(
        origin(),
        links,
        TaskPayloadDraft::semantic(
            TaskSemanticFact::TerminalCommitted {
                task_result: TaskOutcome::Failure,
                final_page: Some(FINAL_PAGE.to_owned()),
                executed_steps: Some(1),
                outcome: Some(failure()),
                failure_severe: false,
                scheduling_disposition: None,
                task_timing: None,
            },
            AuditInput::new(),
        )
        .into(),
    );
    (instance, ledger.events, json!(request.transport()))
}

/// A CLI client of the fake Runtime under `root`.
fn connect_client(root: &Path) -> RuntimeClient {
    RuntimeClient::connect(RuntimeClientConfig::new(
        root,
        RuntimeChannel::Cli,
        EventActor::Cli,
        EventSource::Cli,
    ))
    .expect("connect to the fake Runtime")
}

/// One call of the MCP tool `run` with `arguments`, answered by the fake Runtime of `root`; its
/// result.
fn call(
    root: &Path,
    tool: fn(&ToolContext<'_>, &Map<String, Value>) -> super::super::tools::ToolOutcome,
    arguments: Value,
) -> Value {
    let runtime = RuntimeAccess::new(None, Some(root.to_path_buf()));
    let cancelled = AtomicBool::new(false);
    let jobs = Jobs::new();
    let context = ToolContext {
        runtime: &runtime,
        cancelled: &cancelled,
        deadline: unreached_deadline(),
        era: Era::Modern,
        session: 1,
        jobs: &jobs,
    };
    let Value::Object(arguments) = arguments else {
        panic!("arguments are an object");
    };
    match tool(&context, &arguments) {
        Ok(success) => success.result,
        Err(error) => panic!("the tool {arguments:?} failed: {}", error.to_value()),
    }
}

/// The terminal `face` shows: its code and, beside it, the word, without the id or the
/// duration.
#[track_caller]
fn assert_terminal(face: &str, terminal: &Value) {
    assert_eq!(
        terminal["failure_code"],
        json!(FAILURE.as_str()),
        "{face}: {terminal}"
    );
    assert_eq!(terminal["failure_values"], kept(), "{face}: {terminal}");
}

#[test]
fn f1_ac_get_run_shows_the_terminal_values_in_full() {
    let (_, events, request_id) = ledger();
    let root = TempDir::new().expect("tempdir");
    let runtime = FakeRuntime::start(root.path(), events);
    let result = call(root.path(), runs::get_run, json!({"handle": request_id}));
    assert_eq!(result["request_id"], request_id, "{result}");
    assert_eq!(result["state"], json!("failed"), "{result}");
    assert_terminal("ac_get_run", &result["terminal"]);
    drop(runtime);
}

#[test]
fn f1_ac_runs_shows_the_values_beside_the_failure_code() {
    let (_, events, request_id) = ledger();
    let root = TempDir::new().expect("tempdir");
    let runtime = FakeRuntime::start(root.path(), events);
    let result = call(
        root.path(),
        runs_tool::runs,
        json!({"since_unix_ms": WINDOW_START_UNIX_MS}),
    );
    let rows = result["runs"].as_array().cloned().unwrap_or_default();
    assert_eq!(rows.len(), 1, "{result}");
    assert_eq!(rows[0]["request_id"], request_id, "{result}");
    assert_eq!(rows[0]["task_result"], json!("failure"), "{result}");
    assert_terminal("ac_runs", &rows[0]);
    drop(runtime);
}

#[test]
fn f1_the_overview_run_shows_the_terminal_values() {
    let (instance, events, request_id) = ledger();
    let root = TempDir::new().expect("tempdir");
    let runtime = FakeRuntime::start(root.path(), events);
    let client = connect_client(root.path());
    let status = RuntimeInstanceStatus::new(
        "neutral.instance",
        *instance.transport(),
        false,
        0,
        false,
        false,
        false,
    )
    .expect("instance status");
    let mut row = json!({});
    let (mut warnings, mut incomplete) = (Vec::new(), false);
    overview::run_block(
        &client,
        &status,
        RunWindow {
            since_unix_ms: WINDOW_START_UNIX_MS,
            until_unix_ms: WINDOW_END_UNIX_MS,
        },
        unreached_deadline(),
        &mut row,
        &mut warnings,
        &mut incomplete,
    );
    assert!(
        !incomplete && warnings.is_empty(),
        "incomplete {incomplete}, warnings {}",
        warnings.len()
    );
    assert_eq!(row["run"]["request_id"], request_id, "{row}");
    assert_terminal("the overview's run", &row["run"]["terminal"]);
    drop(client);
    drop(runtime);
}

#[test]
fn f1_the_diagnosis_runs_show_the_terminal_values_and_the_groups_keep_the_code() {
    let (_, events, request_id) = ledger();
    let root = TempDir::new().expect("tempdir");
    let runtime = FakeRuntime::start(root.path(), events);
    let client = connect_client(root.path());
    let request = DiagnoseRequest {
        instance: None,
        task_id: None,
        since_unix_ms: Some(WINDOW_START_UNIX_MS),
    };
    let diagnosis = match diagnose::read(
        &client,
        &request,
        WINDOW_START_UNIX_MS,
        SuspensionReport::not_started(Outcome::new(ActingctlCode::InstallRootUnresolved)),
        None,
        unreached_deadline(),
        &diagnose::SystemClock,
    ) {
        Ok((diagnosis, _)) => diagnosis,
        Err(failure) => panic!(
            "the diagnosis failed: {}",
            failure.outcome().code().as_str()
        ),
    };
    assert_eq!(diagnosis.runs.len(), 1, "{:?}", diagnosis.runs);
    assert_eq!(diagnosis.runs[0]["request_id"], request_id);
    assert_terminal("the diagnosis's runs", &diagnosis.runs[0]["terminal"]);
    // The groups are by (catalog_task_id, failure_code, final_page), as before.
    assert_eq!(
        diagnosis.runs_grouped.len(),
        1,
        "{:?}",
        diagnosis.runs_grouped
    );
    let group = &diagnosis.runs_grouped[0];
    assert_eq!(group["failure_code"], json!(FAILURE.as_str()), "{group}");
    assert_eq!(group["final_page"], json!(FINAL_PAGE), "{group}");
    assert!(group.get("failure_values").is_none(), "{group}");
    drop(client);
    drop(runtime);
}
