// SPDX-License-Identifier: AGPL-3.0-only

//! Operator tools (#338 §四 工具表, S2) and the two R5 observer tools. Every write goes the
//! model's provenance way on its own connection: `begin_interaction`, then on a (Cli, Cli)
//! connection the identity card `{client: "actingctl-mcp"}` (a refused card is only the warning
//! `governance_client_not_allowed`), then `RecordClientAction{surface_id: "mcp", control_id:
//! <tool>, kind: Command}`, then the formal operation, all under that one correlation. What a
//! run is, whether it may run, whether a pause or a target policy is accepted: all of it is the
//! Runtime's answer, passed on unchanged. Nothing here pauses implicitly (C1 甲).

use super::child;
use super::lab;
use super::observer::select_instance;
use super::runtime::client_error;
use super::tools::{Arguments, ToolContext, ToolError, ToolOutcome, ToolSuccess, invalid_argument};
use actingcommand_contract::{
    ClientActionKind, ClientActionRecord, ClientActionValue, ContainedTaskRecoveryBinding,
    ContainedTaskRequest, EmulatorInstanceAction, EventActor, EventSource, GovernanceIdentityCard,
    MAX_RESOURCE_TARGETS_DOCUMENT_BYTES, OwnerEpoch, PackageRef, RequestId,
    SchedulingPauseExpectation, SchedulingPauseScope, validate_scheduling_pause_reason,
};
use actingcommand_runtime_client::{
    ContainedTaskResetOutcome, RunKey, RunStatusMode, RuntimeClient,
};
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const MCP_SURFACE: &str = "mcp";
const MCP_CLIENT: &str = "actingctl-mcp";
const DEFAULT_PAUSE_REASON: &str = "mcp.pause";
const DEFAULT_DRAIN_TIMEOUT_S: u64 = 60;
const MAX_DRAIN_TIMEOUT_S: u64 = 600;
const MIN_DEADLINE_S: u64 = 60;
const MAX_DEADLINE_S: u64 = 1800;
const DAY_MS: u64 = 24 * 60 * 60 * 1000;
const MAX_VALID_DAYS: u64 = 365;
/// Time kept back from the call budget for answering after a job wait.
const ANSWER_RESERVE: Duration = Duration::from_secs(2);
/// Time kept back from the call budget for an actinglab digest subprocess.
const DIGEST_RESERVE: Duration = Duration::from_secs(10);

/// One write's connection after its provenance was recorded.
pub(super) struct Write {
    pub(super) client: RuntimeClient,
    pub(super) warnings: Vec<Value>,
}

/// Opens a connection with this origin and records the provenance of `tool`.
fn begin_write(
    context: &ToolContext<'_>,
    tool: &str,
    instance_alias: Option<&str>,
    origin: (EventActor, EventSource),
) -> Result<Write, ToolError> {
    let connection = context.runtime.connect_fresh(origin.0, origin.1)?;
    record_provenance(connection, tool, instance_alias, origin, None)
}

/// On `connection`: `begin_interaction`, the identity card on (Cli, Cli), then the
/// `client.action` of `tool`, carrying `value` when given (a Lab call's req_id).
pub(super) fn record_provenance(
    connection: RuntimeClient,
    tool: &str,
    instance_alias: Option<&str>,
    origin: (EventActor, EventSource),
    value: Option<ClientActionValue>,
) -> Result<Write, ToolError> {
    let client = connection
        .begin_interaction()
        .map_err(|error| client_error(&error))?;
    let mut warnings = Vec::new();
    // (Agent, Adapter) declares no card (rt:3640-3646).
    if origin == (EventActor::Cli, EventSource::Cli) {
        let card = GovernanceIdentityCard {
            client: MCP_CLIENT.to_owned(),
            client_version: None,
            instance: None,
        };
        if let Err(error) = client.declare_governance_identity(&card) {
            let error = client_error(&error);
            if error.host_code() != Some("governance_client_not_allowed") {
                return Err(error);
            }
            // The Runtime recorded its warning event and still takes Cli writes.
            warnings.push(
                ToolError::new(
                    "safety",
                    "governance_client_not_allowed",
                    "actingd's allowed_clients does not list actingctl-mcp; the Runtime recorded a warning and the write went ahead",
                )
                .with_detail("card_refusal", error.into_value())
                .blocked_by(
                    "governance.allowed_clients in actingd.config.json (apps/actingd/src/config.rs:295-320)",
                )
                .into_warning(),
            );
        }
    }
    let action = ClientActionRecord::new(
        MCP_SURFACE,
        tool,
        ClientActionKind::Command,
        instance_alias.map(str::to_owned),
        value,
    )
    .map_err(|error| {
        ToolError::new(
            "usage",
            "client_action_invalid",
            format!("cannot form the client action: {error}"),
        )
    })?;
    client
        .record_client_action(action)
        .map_err(|error| client_error(&error))?;
    Ok(Write { client, warnings })
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

/// Waits for `job` within the call budget; its outcome, or a handle while it still runs.
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
        let outcome = job.outcome().unwrap_or(Value::Null);
        if let Some(error) = outcome.get("error") {
            return Err(error_from_value(error));
        }
        return Ok(ToolSuccess {
            result: outcome,
            warnings,
        });
    }
    Ok(ToolSuccess {
        result: json!({"handle": job.handle, "job_phase": job.phase()}),
        warnings,
    })
}

/// A job's stored error, answered as the call's error.
fn error_from_value(error: &Value) -> ToolError {
    fn field<'a>(error: &'a Value, key: &str) -> &'a str {
        error.get(key).and_then(Value::as_str).unwrap_or_default()
    }
    let text = |key: &str| field(error, key).to_owned();
    let class = match text("class").as_str() {
        "usage" => "usage",
        "safety" => "safety",
        "device" => "device",
        "uncertain" => "uncertain",
        _ => "runtime",
    };
    let mut mapped = ToolError::new(class, text("code"), text("message"));
    if let Some(Value::Object(details)) = error.get("details") {
        for (key, value) in details {
            mapped = mapped.with_detail(key, value.clone());
        }
    }
    match error.get("blocked_by").and_then(Value::as_str) {
        Some(blocked_by) => mapped.blocked_by(blocked_by),
        None => mapped,
    }
}

// ---------------------------------------------------------------- ac_run_pack

pub(super) fn run_pack(context: &ToolContext<'_>, arguments: &Map<String, Value>) -> ToolOutcome {
    let arguments = Arguments::new(
        arguments,
        &[
            "instance",
            "package",
            "package_ref",
            "recovery_package",
            "recovery_package_ref",
            "deadline_s",
        ],
    )?;
    let selector = arguments.required_string("instance")?;
    let package = package_path(arguments.required_string("package")?, "package")?;
    let deadline_s = arguments
        .integer("deadline_s", MIN_DEADLINE_S, MAX_DEADLINE_S)?
        .unwrap_or(MAX_DEADLINE_S);
    let recovery = match (
        arguments.string("recovery_package")?,
        arguments.string("recovery_package_ref")?,
    ) {
        (Some(path), reference) => Some((package_path(path, "recovery_package")?, reference)),
        (None, None) => None,
        (None, Some(_)) => {
            return Err(invalid_argument(
                "recovery_package_ref",
                "needs recovery_package: the recovery package is given as a pair",
            ));
        }
    };
    context.jobs.has_capacity()?;
    let alias = resolve_alias(context, selector)?;
    let package_ref = match arguments.string("package_ref")? {
        Some(reference) => reference.to_owned(),
        None => package_digest(context, &package)?,
    };
    let mut request = ContainedTaskRequest::new(
        package.display().to_string(),
        parse_package_ref(&package_ref, "package_ref")?,
    )
    .map_err(|error| invalid_argument("package", &format!("is not a contained task: {error}")))?;
    if let Some((path, reference)) = recovery {
        let reference = match reference {
            Some(reference) => reference.to_owned(),
            None => package_digest(context, &path)?,
        };
        let binding = ContainedTaskRecoveryBinding::new(
            path.display().to_string(),
            parse_package_ref(&reference, "recovery_package_ref")?,
        )
        .map_err(|error| {
            invalid_argument("recovery_package", &format!("is not usable: {error}"))
        })?;
        request = request.with_recovery(binding).map_err(|error| {
            invalid_argument("recovery_package", &format!("is not usable: {error}"))
        })?;
    }
    let request = request
        .with_response_deadline_ms(deadline_s * 1000)
        .map_err(|error| invalid_argument("deadline_s", &format!("is refused: {error}")))?;
    let write = begin_write(context, "ac_run_pack", Some(alias.as_str()), CLI)?;
    let prepared = write
        .client
        .prepare_contained_task(&alias, request)
        .map_err(|error| client_error(&error))?;
    let handle = text_of(json!(prepared.request_id()));
    let correlation_id = text_of(json!(prepared.correlation_id()));
    let client = write.client.clone();
    context.jobs.start(
        handle.clone(),
        "run_pack",
        Some(alias),
        "submitting",
        write.warnings.clone(),
        move || {
            client
                .submit_prepared(prepared)
                .map(|output| json!({"receipt_state": output.receipt().state()}))
                .map_err(|error| client_error(&error))
        },
    )?;
    Ok(ToolSuccess {
        result: json!({
            "handle": handle,
            "correlation_id": correlation_id,
            "phase": "submitting",
        }),
        warnings: write.warnings,
    })
}

fn package_path(text: &str, field: &str) -> Result<PathBuf, ToolError> {
    std::fs::canonicalize(text)
        .map_err(|error| invalid_argument(field, &format!("cannot be read: {error}")))
}

fn parse_package_ref(text: &str, field: &str) -> Result<PackageRef, ToolError> {
    PackageRef::parse_argument(text)
        .map_err(|error| invalid_argument(field, &format!("is not a package reference: {error}")))
}

/// `<root>\tools\actinglab.exe --json package digest --package <path>`: the package's content
/// reference, as actinglab computes it.
fn package_digest(context: &ToolContext<'_>, package: &Path) -> Result<String, ToolError> {
    let Some(root) = context.runtime.locate().root else {
        return Err(ToolError::usage(
            "lab_tool_unavailable",
            "no install root, so actinglab cannot compute the package_ref; pass package_ref",
        ));
    };
    let mut command = Command::new(root.join("tools").join("actinglab.exe"));
    command
        .arg("--json")
        .arg("package")
        .arg("digest")
        .arg("--package")
        .arg(package);
    let deadline = context
        .deadline
        .checked_sub(DIGEST_RESERVE)
        .unwrap_or(context.deadline);
    let captured = child::run_captured(command, deadline)
        .map_err(|failure| lab::child_failure(&failure, "package digest"))?;
    // The S3 runner's reading of the envelope: the exit code gives the class.
    let digest = lab::interpret(&captured, "package digest").result?;
    digest
        .get("reference")
        .map(Value::to_string)
        .ok_or_else(|| {
            ToolError::new(
                "runtime",
                "lab_process_failed",
                "actinglab package digest answered without a package reference",
            )
        })
}

// ---------------------------------------------------------------- ac_stop_run

pub(super) fn stop_run(context: &ToolContext<'_>, arguments: &Map<String, Value>) -> ToolOutcome {
    let arguments = Arguments::new(arguments, &["handle", "wait_s"])?;
    let handle = arguments.required_string("handle")?;
    let request_id = serde_json::from_value::<RequestId>(json!(handle))
        .map_err(|_| invalid_argument("handle", "must be a run's Runtime request_id"))?;
    let wait_until =
        Instant::now() + Duration::from_secs(arguments.integer("wait_s", 0, 25)?.unwrap_or(20));
    // A live job of this process submitted the run: only ask it to stop; the job's own
    // client resets after a cancelled run (cl:1421-1439).
    if let Some(job) = context
        .jobs
        .find(handle)
        .filter(|job| job.kind == "run_pack" && !job.finished())
    {
        let write = begin_write(context, "ac_stop_run", job.instance_alias.as_deref(), CLI)?;
        let cancellation = write
            .client
            .cancel_contained_task(request_id)
            .map_err(|error| stop_error(&error))?;
        return Ok(ToolSuccess {
            result: json!({"cancellation": cancellation, "job_phase": job.phase()}),
            warnings: write.warnings,
        });
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
        return Err(ToolError::usage(
            "run_unknown",
            "the ledger names no configured instance for this handle; read it with ac_get_run",
        )
        .with_detail("state", json!(run.state)));
    };
    let write = begin_write(context, "ac_stop_run", Some(alias.as_str()), CLI)?;
    let job_handle = text_of(json!(write.client.correlation_id()));
    let client = write.client.clone();
    let wait = Duration::from_millis(ContainedTaskRequest::MAX_RESPONSE_DEADLINE_MS);
    let job = context.jobs.start(
        job_handle,
        "stop_run",
        Some(alias.clone()),
        "stopping",
        write.warnings,
        move || {
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
        },
    )?;
    job_answer(context, &job, wait_until).map(|mut answer| {
        if answer.result.get("job_phase").is_none() {
            answer.result["job_phase"] = json!("done");
        }
        answer
    })
}

/// A scheduled run cannot be stopped by a client; an instance pause drains it.
fn stop_error(error: &actingcommand_runtime_client::RuntimeClientError) -> ToolError {
    let mapped = client_error(error);
    if mapped.host_code() == Some("scheduled_contained_task_not_client_cancellable") {
        return mapped.blocked_by(
            "ac_pause: an instance pause drains the instance and, when its drain times out, stops its in-flight runs",
        );
    }
    mapped
}

// ---------------------------------------------------------------- ac_pause / ac_resume

fn pause_scope(
    context: &ToolContext<'_>,
    selector: Option<&str>,
) -> Result<SchedulingPauseScope, ToolError> {
    Ok(match selector {
        Some(selector) => SchedulingPauseScope::Instance {
            instance_alias: resolve_alias(context, selector)?,
        },
        None => SchedulingPauseScope::Global,
    })
}

fn scope_alias(scope: &SchedulingPauseScope) -> Option<&str> {
    match scope {
        SchedulingPauseScope::Instance { instance_alias } => Some(instance_alias.as_str()),
        SchedulingPauseScope::Global => None,
    }
}

pub(super) fn pause(context: &ToolContext<'_>, arguments: &Map<String, Value>) -> ToolOutcome {
    let arguments = Arguments::new(arguments, &["instance", "reason", "drain_timeout_s"])?;
    let reason = arguments
        .string("reason")?
        .unwrap_or(DEFAULT_PAUSE_REASON)
        .to_owned();
    validate_scheduling_pause_reason(&reason)
        .map_err(|_| invalid_argument("reason", "must be 1-64 bytes of [a-z0-9_.-]"))?;
    let drain_timeout_s = arguments
        .integer("drain_timeout_s", 1, MAX_DRAIN_TIMEOUT_S)?
        .unwrap_or(DEFAULT_DRAIN_TIMEOUT_S);
    context.jobs.has_capacity()?;
    let scope = pause_scope(context, arguments.string("instance")?)?;
    let write = begin_write(context, "ac_pause", scope_alias(&scope), CLI)?;
    let owner_epoch = write.client.runtime_info().owner_epoch();
    let job_handle = text_of(json!(write.client.correlation_id()));
    let client = write.client.clone();
    let job = context.jobs.start(
        job_handle,
        "pause",
        scope_alias(&scope).map(str::to_owned),
        "pausing",
        write.warnings,
        move || {
            let paused = client
                .pause_scheduling(scope, &reason, drain_timeout_s * 1000)
                .map_err(|error| client_error(&error))?;
            Ok(json!({"paused": paused, "owner_epoch": owner_epoch}))
        },
    )?;
    job_answer(context, &job, context.deadline)
}

pub(super) fn resume(context: &ToolContext<'_>, arguments: &Map<String, Value>) -> ToolOutcome {
    let arguments = Arguments::new(
        arguments,
        &["instance", "expected_owner_epoch", "expected_revision"],
    )?;
    let owner_epoch = serde_json::from_value::<OwnerEpoch>(json!(
        arguments.required_string("expected_owner_epoch")?
    ))
    .map_err(|_| {
        invalid_argument(
            "expected_owner_epoch",
            "must be the owner_epoch ac_pause or ac_overview gave",
        )
    })?;
    let revision = arguments
        .integer("expected_revision", 1, u64::MAX)?
        .ok_or_else(|| invalid_argument("expected_revision", "is required"))?;
    context.jobs.has_capacity()?;
    let scope = pause_scope(context, arguments.string("instance")?)?;
    let write = begin_write(context, "ac_resume", scope_alias(&scope), CLI)?;
    let job_handle = text_of(json!(write.client.correlation_id()));
    let client = write.client.clone();
    let job = context.jobs.start(
        job_handle,
        "resume",
        scope_alias(&scope).map(str::to_owned),
        "resuming",
        write.warnings,
        move || {
            client
                .resume_scheduling_expected(
                    scope,
                    SchedulingPauseExpectation {
                        owner_epoch,
                        revision,
                    },
                )
                .map(|resumed| json!({"resumed": resumed}))
                .map_err(|error| client_error(&error))
        },
    )?;
    job_answer(context, &job, context.deadline)
}

// ---------------------------------------------------------------- ac_emulator

pub(super) fn emulator(context: &ToolContext<'_>, arguments: &Map<String, Value>) -> ToolOutcome {
    let arguments = Arguments::new(arguments, &["instance", "action"])?;
    let action = match arguments.required_string("action")? {
        "start" => EmulatorInstanceAction::Start,
        "stop" => EmulatorInstanceAction::Stop,
        "restart" => EmulatorInstanceAction::Restart,
        _ => return Err(invalid_argument("action", "must be start, stop or restart")),
    };
    context.jobs.has_capacity()?;
    let alias = resolve_alias(context, arguments.required_string("instance")?)?;
    let write = begin_write(context, "ac_emulator", Some(alias.as_str()), CLI)?;
    let job_handle = text_of(json!(write.client.correlation_id()));
    let client = write.client.clone();
    let job = context.jobs.start(
        job_handle,
        "emulator",
        Some(alias.clone()),
        "running",
        write.warnings,
        move || {
            client
                .control_emulator_instance(&alias, action)
                .map(|controlled| json!({"controlled": controlled}))
                .map_err(|error| client_error(&error))
        },
    )?;
    job_answer(context, &job, context.deadline)
}

// ---------------------------------------------------------------- resource targets

/// R5's view of one instance; an older daemon answers `runtime_operation_unsupported`.
fn target_view(
    context: &ToolContext<'_>,
    selector: &str,
) -> Result<actingcommand_contract::ResourceTargetView, ToolError> {
    let alias = resolve_alias(context, selector)?;
    let connected = context.runtime.connect()?;
    connected
        .client
        .resource_target_view(&alias)
        .map_err(|error| {
            let mapped = context.runtime.failure(&connected, &error);
            if mapped.code() == "runtime_operation_unsupported" {
                mapped.blocked_by(
                    "an actingd older than v0.11.0: read the scheduling catalog's resources instead (skill actingcommand, CLI workshop)",
                )
            } else {
                mapped
            }
        })
}

fn view_header(view: &actingcommand_contract::ResourceTargetView) -> Map<String, Value> {
    let mut header = Map::new();
    header.insert("instance_alias".to_owned(), json!(view.instance_alias));
    header.insert("policy_instance".to_owned(), json!(view.policy_instance));
    header.insert(
        "evaluated_at_unix_ms".to_owned(),
        json!(view.evaluated_at_unix_ms),
    );
    header.insert(
        "as_of_ledger_position".to_owned(),
        json!(view.as_of_ledger_position),
    );
    if let Some(catalog_hash) = &view.catalog_hash {
        header.insert("catalog_hash".to_owned(), json!(catalog_hash));
    }
    header
}

pub(super) fn resources_list(
    context: &ToolContext<'_>,
    arguments: &Map<String, Value>,
) -> ToolOutcome {
    let arguments = Arguments::new(arguments, &["instance"])?;
    let view = target_view(context, arguments.required_string("instance")?)?;
    let mut result = view_header(&view);
    result.insert("targetable".to_owned(), json!(view.targetable));
    result.insert("not_targetable".to_owned(), json!(view.not_targetable));
    Ok(ToolSuccess::new(Value::Object(result)))
}

pub(super) fn targets_get(
    context: &ToolContext<'_>,
    arguments: &Map<String, Value>,
) -> ToolOutcome {
    let arguments = Arguments::new(arguments, &["instance"])?;
    let view = target_view(context, arguments.required_string("instance")?)?;
    let mut result = view_header(&view);
    result.insert("active".to_owned(), json!(view.active));
    Ok(ToolSuccess::new(Value::Object(result)))
}

pub(super) fn targets_set(
    context: &ToolContext<'_>,
    arguments: &Map<String, Value>,
) -> ToolOutcome {
    let arguments = Arguments::new(arguments, &["instance", "targets", "valid_days"])?;
    let selector = arguments.required_string("instance")?;
    let Some(Value::Array(targets)) = arguments.value("targets") else {
        return Err(invalid_argument(
            "targets",
            "must be an array of v2 targets",
        ));
    };
    let valid_days = arguments.integer("valid_days", 1, MAX_VALID_DAYS)?;
    let view = target_view(context, selector)?;
    let mut document = Map::new();
    document.insert(
        "schema_version".to_owned(),
        json!("actingcommand.resource-targets.v2"),
    );
    document.insert("instance".to_owned(), json!(view.policy_instance));
    // valid_until only when asked for; whether empty targets clear is the Runtime's answer.
    if let Some(days) = valid_days {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|elapsed| u64::try_from(elapsed.as_millis()).ok())
            .ok_or_else(|| {
                ToolError::new(
                    "runtime",
                    "clock_unavailable",
                    "the system clock is before the Unix epoch or out of range",
                )
            })?;
        document.insert("valid_until_unix_ms".to_owned(), json!(now + days * DAY_MS));
    }
    document.insert("targets".to_owned(), Value::Array(targets.clone()));
    let document = Value::Object(document).to_string();
    if document.len() > MAX_RESOURCE_TARGETS_DOCUMENT_BYTES {
        return Err(ToolError::usage(
            "resource_targets_document_too_large",
            format!(
                "the policy document has {} bytes; the Runtime takes at most {MAX_RESOURCE_TARGETS_DOCUMENT_BYTES}",
                document.len()
            ),
        ));
    }
    let write = begin_write(
        context,
        "ac_targets_set",
        Some(view.instance_alias.as_str()),
        AGENT,
    )?;
    match write.client.apply_resource_targets(document) {
        Ok(applied) => Ok(ToolSuccess {
            result: json!({"applied": applied}),
            warnings: write.warnings,
        }),
        Err(error) => {
            let rejection = error
                .received_receipt()
                .and_then(|receipt| receipt.resource_targets_rejection())
                .map(|rejection| json!(rejection));
            let mapped = client_error(&error);
            Err(match rejection {
                Some(rejection) => mapped.with_detail("rejection", rejection),
                None => mapped,
            })
        }
    }
}
