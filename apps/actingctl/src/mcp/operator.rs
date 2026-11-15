// SPDX-License-Identifier: AGPL-3.0-only

//! Operator tools (#338 §四 工具表, S2) and the two R5 observer tools. Every write goes on its
//! own connection under one correlation of its own (`begin_interaction`), the formal operation
//! first: since Workflow #434 LAB-6S4a (0.12.63; the coordinator's ruling N8 on board #434) it
//! declares no identity card and records no `client.action` before it, the initiator `Lab-MCP`
//! its connection's binding carries names where it came from. What a run is, whether it may
//! run, whether a pause or a target policy is accepted: all of it is the Runtime's answer,
//! passed on unchanged. Nothing here pauses implicitly (C1 甲). A refusal keeps the Runtime
//! receipt's outcome on top (`client_error`, Workflow #378 A5).

use super::observer::select_instance;
use super::runtime::client_error;
use super::tools::{
    Arguments, McpBlocker, ToolContext, ToolError, ToolOutcome, ToolSuccess, invalid_argument,
};
use crate::codes::{ActingctlCode, ArgumentProblem, RunState};
use crate::{emulators, scheduling, targets};
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::{Code, IoOp, Outcome};
use actingcommand_contract::{
    ClientActionKind, ClientActionRecord, ClientActionValue, ContainedTaskRecoveryBinding,
    ContainedTaskRequest, EventActor, EventSource, OwnerEpoch, RequestId,
    SchedulingPauseExpectation, SchedulingPauseScope,
};
use actingcommand_runtime_client::ClientCode;
use actingcommand_runtime_client::{
    ContainedTaskResetOutcome, RunKey, RunStatusMode, RuntimeClient,
};
use serde_json::{Map, Value, json};
use std::path::PathBuf;
use std::time::{Duration, Instant};

const MCP_SURFACE: &str = "mcp";
const MIN_DEADLINE_S: u64 = 60;
const MAX_DEADLINE_S: u64 = 1800;
// Workflow #434 LAB-1b: `ac_pause`'s reason and drain timeout defaults and bound are
// `scheduling`'s (`MCP_PAUSE_REASON`, `MCP_DRAIN_TIMEOUT_S`, `MCP_MAX_DRAIN_TIMEOUT_S`);
// `ac_targets_set`'s validity bound, its document and the rejection's pointer bound are
// `targets`' (`MAX_VALID_DAYS`, `document`, `with_rejection`).
/// Time kept back from the call budget for answering after a job wait.
const ANSWER_RESERVE: Duration = Duration::from_secs(2);

/// Opens a connection with this origin for one write, under a correlation of its own.
fn begin_write(
    context: &ToolContext<'_>,
    origin: (EventActor, EventSource),
) -> Result<RuntimeClient, ToolError> {
    let connection = context.runtime.connect_fresh(origin.0, origin.1)?;
    write_interaction(&connection)
}

/// The correlation of one write on `connection` (`begin_interaction`). Workflow #434 LAB-6S4a
/// (0.12.63): the identity card and the `client.action` that came before the write are gone (the
/// coordinator's ruling N8), so its first request is the write itself.
pub(super) fn write_interaction(connection: &RuntimeClient) -> Result<RuntimeClient, ToolError> {
    connection
        .begin_interaction()
        .map_err(|error| client_error(&error))
}

/// On `connection`, under a correlation of its own: the `client.action` of `tool` after a Lab
/// call's capture, carrying `value` when given (the call's req_id). Workflow #434 LAB-6S4a
/// (0.12.63): it declares no identity card before it.
pub(super) fn record_capture_action(
    connection: &RuntimeClient,
    tool: &str,
    instance_alias: Option<&str>,
    value: Option<ClientActionValue>,
) -> Result<(), ToolError> {
    let client = write_interaction(connection)?;
    let action = ClientActionRecord::new(
        MCP_SURFACE,
        tool,
        ClientActionKind::Command,
        instance_alias.map(str::to_owned),
        value,
    )
    .map_err(|_| ToolError::usage(SharedCode::ClientActionInvalid))?;
    client
        .record_client_action(action)
        .map_err(|error| client_error(&error))?;
    Ok(())
}

pub(super) const CLI: (EventActor, EventSource) = (EventActor::Cli, EventSource::Cli);
const AGENT: (EventActor, EventSource) = (EventActor::Agent, EventSource::Adapter);

/// The alias of the instance an argument names.
fn resolve_alias(context: &ToolContext<'_>, selector: &str) -> Result<String, ToolError> {
    let connected = context.runtime.connect()?;
    let status = connected
        .client
        .status()
        .map_err(|error| context.runtime.failure(&connected, &error))?;
    Ok(select_instance(status.instances(), selector)?
        .instance_alias()
        .to_owned())
}

fn text_of(value: Value) -> String {
    match value {
        Value::String(text) => text,
        other => other.to_string(),
    }
}

/// Waits for `job` within the call budget; its result (the answer, or its error as the call's
/// error, unchanged), or a handle while it still runs.
pub(super) fn job_answer(
    context: &ToolContext<'_>,
    job: &super::jobs::Job,
    wait_until: Instant,
) -> ToolOutcome {
    let until = wait_until.min(
        context
            .deadline
            .checked_sub(ANSWER_RESERVE)
            .unwrap_or(context.deadline),
    );
    let ended = job.wait_until(until, context.cancelled);
    // Read after the wait: a body may add warnings as it ends.
    let warnings = job.warnings();
    if ended {
        return match job.job_result() {
            Some(Err(error)) => Err(error),
            Some(Ok(result)) => Ok(ToolSuccess { result, warnings }),
            None => Ok(ToolSuccess {
                result: Value::Null,
                warnings,
            }),
        };
    }
    // Workflow #421 MCP-ENTRY fix round 1 (review M2): the job counts for the MCP entry only if
    // this answer reaches the client; the worker marks it issued when it sends the answer.
    super::jobs::offer(&job.handle);
    Ok(ToolSuccess {
        result: json!({"handle": job.handle, "job_phase": job.phase()}),
        warnings,
    })
}

// ---------------------------------------------------------------- ac_run_pack

pub(super) fn run_pack(context: &ToolContext<'_>, arguments: &Map<String, Value>) -> ToolOutcome {
    let arguments = Arguments::new(
        arguments,
        // Workflow #336 PKG-S4 (option 丙): a package and its recovery package by their paths
        // only; `package_ref` and `recovery_package_ref` are no arguments, and no digest is
        // computed.
        &["instance", "package", "recovery_package", "deadline_s"],
    )?;
    let selector = arguments.required_string("instance")?;
    let package = package_path(arguments.required_string("package")?, "package")?;
    let deadline_s = arguments
        .integer("deadline_s", MIN_DEADLINE_S, MAX_DEADLINE_S)?
        .unwrap_or(MAX_DEADLINE_S);
    let recovery = match arguments.string("recovery_package")? {
        Some(path) => Some(package_path(path, "recovery_package")?),
        None => None,
    };
    context.jobs.has_capacity()?;
    let alias = resolve_alias(context, selector)?;
    // A refused request part is `arguments_invalid` for its argument, caused by the contract's
    // code.
    let mut request = ContainedTaskRequest::new(package.display().to_string())
        .map_err(|error| refused_argument("package", error.code()))?;
    if let Some(path) = recovery {
        let binding = ContainedTaskRecoveryBinding::new(path.display().to_string())
            .map_err(|error| refused_argument("recovery_package", error.code()))?;
        request = request
            .with_recovery(binding)
            .map_err(|error| refused_argument("recovery_package", error.code()))?;
    }
    let request = request
        .with_response_deadline_ms(deadline_s * 1000)
        .map_err(|error| refused_argument("deadline_s", error.code()))?;
    let client = begin_write(context, CLI)?;
    let prepared = client
        .prepare_contained_task(&alias, request)
        .map_err(|error| client_error(&error))?;
    let handle = text_of(json!(prepared.request_id()));
    let correlation_id = text_of(json!(prepared.correlation_id()));
    context
        .jobs
        .start(handle.clone(), "run_pack", "submitting", move || {
            client
                .submit_prepared(prepared)
                .map(|output| json!({"receipt_state": output.receipt().state()}))
                .map_err(|error| client_error(&error))
        })?;
    Ok(ToolSuccess::new(json!({
        "handle": handle,
        "correlation_id": correlation_id,
        "phase": "submitting",
    })))
}

/// `arguments_invalid` `unusable` for an argument the contract refused, caused by its code.
fn refused_argument(argument: &str, refusal: Code) -> ToolError {
    invalid_argument(argument, ArgumentProblem::Unusable).caused_by(Outcome::new(refusal))
}

/// The package path an argument names; one that cannot be read is `unusable`, caused by the
/// OS error.
fn package_path(text: &str, argument: &str) -> Result<PathBuf, ToolError> {
    std::fs::canonicalize(text).map_err(|error| {
        invalid_argument(argument, ArgumentProblem::Unusable)
            .caused_by(Outcome::from_io_error(&error, IoOp::Canonicalize).path(text))
    })
}

// ---------------------------------------------------------------- ac_stop_run

pub(super) fn stop_run(context: &ToolContext<'_>, arguments: &Map<String, Value>) -> ToolOutcome {
    let arguments = Arguments::new(arguments, &["handle", "wait_s"])?;
    let handle = arguments.required_string("handle")?;
    // A run's Runtime request_id.
    let request_id = serde_json::from_value::<RequestId>(json!(handle))
        .map_err(|_| invalid_argument("handle", ArgumentProblem::NotRecognised))?;
    let wait_until =
        Instant::now() + Duration::from_secs(arguments.integer("wait_s", 0, 25)?.unwrap_or(20));
    // A live job of this process submitted the run: only ask it to stop; the job's own
    // client resets after a cancelled run (cl:1421-1439).
    if let Some(job) = context
        .jobs
        .find(handle)
        .filter(|job| job.kind == "run_pack" && !job.finished())
    {
        let cancellation = begin_write(context, CLI)?
            .cancel_contained_task(request_id)
            .map_err(|error| stop_error(&error))?;
        return Ok(ToolSuccess::new(
            json!({"cancellation": cancellation, "job_phase": job.phase()}),
        ));
    }
    // Otherwise a background cancel_contained_task_and_reset, waiting up to the longest run
    // deadline so a Pending answer rarely leaves the touch release undone.
    context.jobs.has_capacity()?;
    let connected = context.runtime.connect()?;
    let run = connected
        .client
        .contained_run_status(RunKey::RequestId(request_id), RunStatusMode::Brief)
        .map_err(|error| context.runtime.failure(&connected, &error))?;
    let status = connected
        .client
        .status()
        .map_err(|error| context.runtime.failure(&connected, &error))?;
    let Some(alias) = run.instance_id.and_then(|instance_id| {
        status
            .instances()
            .iter()
            .find(|instance| instance.instance_id() == instance_id)
            .map(|instance| instance.instance_alias().to_owned())
    }) else {
        // The ledger names no configured instance for this handle: read it with ac_get_run.
        // The run's state as its ledger events show it travels as `run_state`.
        return Err(ToolError::usage(ActingctlCode::RunUnknown).with(|outcome| {
            outcome
                .request_id(handle)
                .run_state(RunState::of(run.state))
        }));
    };
    let client = begin_write(context, CLI)?;
    let job_handle = text_of(json!(client.correlation_id()));
    let wait = Duration::from_millis(ContainedTaskRequest::MAX_RESPONSE_DEADLINE_MS);
    let job = context
        .jobs
        .start(job_handle, "stop_run", "stopping", move || {
            let stopped = client
                .cancel_contained_task_and_reset(&alias, request_id, wait)
                .map_err(|error| stop_error(&error))?;
            let mut outcome = json!({"cancellation": stopped.status});
            match stopped.reset {
                Some(ContainedTaskResetOutcome::Done) => outcome["touch_release"] = json!("done"),
                Some(ContainedTaskResetOutcome::NotNeeded) => {
                    outcome["touch_release"] = json!("not_needed");
                }
                Some(ContainedTaskResetOutcome::Failed(error)) => {
                    outcome["touch_release"] = json!("failed");
                    outcome["touch_release_error"] = client_error(&error).into_value();
                }
                None => {}
            }
            Ok(outcome)
        })?;
    job_answer(context, &job, wait_until).map(|mut answer| {
        if answer.result.get("job_phase").is_none() {
            answer.result["job_phase"] = json!("done");
        }
        answer
    })
}

/// A scheduled run cannot be stopped by a client; an instance pause drains it (ac_pause drains
/// the instance and, when its drain times out, stops its in-flight runs): the Runtime's refusal
/// with `mcp_blocker` `scheduling_pause`.
fn stop_error(error: &actingcommand_runtime_client::RuntimeClientError) -> ToolError {
    let mapped = client_error(error);
    if mapped.is(SharedCode::ScheduledContainedTaskNotClientCancellable) {
        return mapped.blocked_by(McpBlocker::SchedulingPause);
    }
    mapped
}

// ---------------------------------------------------------------- ac_pause / ac_resume

/// The scope `selector` names, resolved to its alias with one status read; global without one.
/// Workflow #434 LAB-1b: the pause and resume themselves are `scheduling`'s.
fn pause_scope(
    context: &ToolContext<'_>,
    selector: Option<&str>,
) -> Result<SchedulingPauseScope, ToolError> {
    Ok(scheduling::pause_scope(match selector {
        Some(selector) => Some(resolve_alias(context, selector)?),
        None => None,
    }))
}

pub(super) fn pause(context: &ToolContext<'_>, arguments: &Map<String, Value>) -> ToolOutcome {
    let arguments = Arguments::new(arguments, &["instance", "reason", "drain_timeout_s"])?;
    // 1-64 bytes of [a-z0-9_.-].
    let reason = actingcommand_contract::SchedulingPauseReason::new(
        arguments
            .string("reason")?
            .unwrap_or(scheduling::MCP_PAUSE_REASON),
    )
    .map_err(|_| invalid_argument("reason", ArgumentProblem::NotRecognised))?;
    let drain_timeout_s = arguments
        .integer("drain_timeout_s", 1, scheduling::MCP_MAX_DRAIN_TIMEOUT_S)?
        .unwrap_or(scheduling::MCP_DRAIN_TIMEOUT_S);
    context.jobs.has_capacity()?;
    let scope = pause_scope(context, arguments.string("instance")?)?;
    let client = begin_write(context, CLI)?;
    let owner_epoch = client.runtime_info().owner_epoch();
    let job_handle = text_of(json!(client.correlation_id()));
    let job = context
        .jobs
        .start(job_handle, "pause", "pausing", move || {
            let paused = scheduling::pause(&client, scope, reason, drain_timeout_s * 1000)
                .map_err(|error| client_error(&error))?;
            Ok(scheduling::paused_answer(&paused, owner_epoch))
        })?;
    job_answer(context, &job, context.deadline)
}

pub(super) fn resume(context: &ToolContext<'_>, arguments: &Map<String, Value>) -> ToolOutcome {
    let arguments = Arguments::new(
        arguments,
        &["instance", "expected_owner_epoch", "expected_revision"],
    )?;
    // The owner_epoch ac_pause or ac_overview gave.
    let owner_epoch = serde_json::from_value::<OwnerEpoch>(json!(
        arguments.required_string("expected_owner_epoch")?
    ))
    .map_err(|_| invalid_argument("expected_owner_epoch", ArgumentProblem::NotRecognised))?;
    let revision = arguments
        .integer("expected_revision", 1, u64::MAX)?
        .ok_or_else(|| invalid_argument("expected_revision", ArgumentProblem::Required))?;
    context.jobs.has_capacity()?;
    let scope = pause_scope(context, arguments.string("instance")?)?;
    let client = begin_write(context, CLI)?;
    let job_handle = text_of(json!(client.correlation_id()));
    let job = context
        .jobs
        .start(job_handle, "resume", "resuming", move || {
            scheduling::resume_expected(
                &client,
                scope,
                SchedulingPauseExpectation {
                    owner_epoch,
                    revision,
                },
            )
            .map(|resumed| scheduling::resumed_answer(&resumed))
            .map_err(|error| client_error(&error))
        })?;
    job_answer(context, &job, context.deadline)
}

// ---------------------------------------------------------------- ac_emulator

/// Workflow #434 LAB-1b: the action's words and the control are `emulators`'.
pub(super) fn emulator(context: &ToolContext<'_>, arguments: &Map<String, Value>) -> ToolOutcome {
    let arguments = Arguments::new(arguments, &["instance", "action"])?;
    let action = emulators::action(arguments.required_string("action")?)
        .ok_or_else(|| invalid_argument("action", ArgumentProblem::NotRecognised))?;
    context.jobs.has_capacity()?;
    let alias = resolve_alias(context, arguments.required_string("instance")?)?;
    let client = begin_write(context, CLI)?;
    let job_handle = text_of(json!(client.correlation_id()));
    let job = context
        .jobs
        .start(job_handle, "emulator", "running", move || {
            emulators::control(&client, &alias, action)
                .map(|controlled| emulators::controlled_answer(&controlled))
                .map_err(|error| client_error(&error))
        })?;
    job_answer(context, &job, context.deadline)
}

// ---------------------------------------------------------------- ac_ladder_drill

/// Workflow #390 DRILL-1: `actingctl ladder-drill` over MCP (`crate::ladder_drill`). `instance`
/// and `device` are the CLI's `--instance` and `--device` (R2; `device` absent is false, a plan
/// drill); the result is the CLI's line without its outcome, the drill the Runtime admitted. The
/// Runtime answers at admission, before the drill climbs, so no job is needed; a refusal is the
/// Runtime's outcome on top.
pub(super) fn ladder_drill(
    context: &ToolContext<'_>,
    arguments: &Map<String, Value>,
) -> ToolOutcome {
    let arguments = Arguments::new(arguments, &["instance", "device"])?;
    let selector = arguments.required_string("instance")?;
    let device = match arguments.value("device") {
        None => false,
        Some(Value::Bool(device)) => *device,
        Some(_) => return Err(invalid_argument("device", ArgumentProblem::WrongType)),
    };
    let alias = resolve_alias(context, selector)?;
    let drill = begin_write(context, CLI)?
        .drill_recovery_ladder(&alias, crate::ladder_drill::mode(device))
        .map_err(|error| client_error(&error))?;
    Ok(ToolSuccess::new(
        serde_json::to_value(&drill).map_err(super::packs::encode_failed)?,
    ))
}

// ---------------------------------------------------------------- resource targets

/// R5's view of one instance (Workflow #434 LAB-1b: `targets::view`); an actingd older than
/// v0.11.0 answers `runtime_operation_unsupported` with `mcp_blocker` `runtime_upgrade` (read the
/// scheduling catalog's resources instead: skill actingcommand, CLI workshop).
fn target_view(
    context: &ToolContext<'_>,
    selector: &str,
) -> Result<actingcommand_contract::ResourceTargetView, ToolError> {
    let alias = resolve_alias(context, selector)?;
    let connected = context.runtime.connect()?;
    targets::view(&connected.client, &alias).map_err(|error| {
        let mapped = context.runtime.failure(&connected, &error);
        if mapped.is(ClientCode::RuntimeOperationUnsupported.code()) {
            mapped.blocked_by(McpBlocker::RuntimeUpgrade)
        } else {
            mapped
        }
    })
}

/// Workflow #434 LAB-1b: the answer is `targets::resources_answer`.
pub(super) fn resources_list(
    context: &ToolContext<'_>,
    arguments: &Map<String, Value>,
) -> ToolOutcome {
    let arguments = Arguments::new(arguments, &["instance"])?;
    let view = target_view(context, arguments.required_string("instance")?)?;
    Ok(ToolSuccess::new(targets::resources_answer(&view)))
}

/// Workflow #434 LAB-1b: the answer is `targets::active_answer`.
pub(super) fn targets_get(
    context: &ToolContext<'_>,
    arguments: &Map<String, Value>,
) -> ToolOutcome {
    let arguments = Arguments::new(arguments, &["instance"])?;
    let view = target_view(context, arguments.required_string("instance")?)?;
    Ok(ToolSuccess::new(targets::active_answer(&view)))
}

/// Workflow #434 LAB-1b: the document, the request and the refusal's values are `targets`'.
pub(super) fn targets_set(
    context: &ToolContext<'_>,
    arguments: &Map<String, Value>,
) -> ToolOutcome {
    let arguments = Arguments::new(arguments, &["instance", "targets", "valid_days"])?;
    let selector = arguments.required_string("instance")?;
    // An array of v2 targets.
    let Some(Value::Array(policy_targets)) = arguments.value("targets") else {
        return Err(invalid_argument("targets", ArgumentProblem::WrongType));
    };
    let valid_days = arguments.integer("valid_days", 1, targets::MAX_VALID_DAYS)?;
    let view = target_view(context, selector)?;
    // valid_until only when asked for; whether empty targets clear is the Runtime's answer.
    let valid_until_unix_ms = match valid_days {
        Some(days) => Some(
            targets::valid_until(days)
                .ok_or_else(|| ToolError::new("runtime", ActingctlCode::ClockUnavailable))?,
        ),
        None => None,
    };
    let document = targets::document(&view, valid_until_unix_ms, policy_targets)
        .map_err(|outcome| ToolError::outcome("usage", outcome))?;
    let client = begin_write(context, AGENT)?;
    match targets::apply(&client, document) {
        Ok(applied) => Ok(ToolSuccess::new(Value::Object(
            targets::applied_answer(&applied).map_err(super::packs::encode_failed)?,
        ))),
        // The Runtime's refusal on top, with the document position and reason its receipt
        // carries (`targets::with_rejection`).
        Err(error) => {
            Err(client_error(&error).with(|outcome| targets::with_rejection(outcome, &error)))
        }
    }
}
