use crate::instance_resolution::resolve_instance_id_for_flags;
use crate::runtime_slice_cli;
use crate::state_roots::runtime_state_root;
use crate::{
    CaptureBackendChoice, CliError, CliOutcome, FlagArgs, GlobalOptions, InstanceConfig,
    TouchBackendChoice, UserConfig, read_user_config, reject_legacy_session_routing,
    runtime_session_adapter,
};
use actingcommand_contract::{
    ApplicationLifecycleAction, EventActor, EventSource, EventType, LabErrorClass,
    RuntimeReceiptState, RuntimeResult,
};
use actingcommand_lab::{
    ApplicationAttempt, ApplicationExecution, ApplicationPlan, CommitApplicationRequest,
    PlanApplicationRequest, RecordingLock, record_application_append_failed,
    record_commit_application, record_plan_application,
};
use actingcommand_runtime_client::{
    RuntimeClient, RuntimeClientConfig, RuntimeClientError, RuntimeFlowOutput,
};
use serde_json::{Value, json};

pub(super) fn run_session_monitor_policy(
    global: &GlobalOptions,
    args: &[String],
) -> CliOutcome<Value> {
    runtime_session_adapter::run_monitor_policy(global, args)
}

pub(super) fn monitor_policy_monitor_args(
    raw_args: &[String],
    flags: &FlagArgs,
) -> CliOutcome<Vec<String>> {
    if flags.optional("--max-iterations").is_some() {
        return Err(CliError::usage(
            "session monitor-policy stores monitor --once arguments; do not use --max-iterations",
        ));
    }
    if flags.bool("--via-daemon") || flags.bool("--local") {
        return Err(CliError::usage(
            "session monitor-policy set does not store --via-daemon or --local",
        ));
    }
    if !flags.bool("--capture") && flags.optional("--scene").is_none() {
        return Err(CliError::usage(
            "session monitor-policy set requires --scene <png> or --capture",
        ));
    }
    let mut out = Vec::new();
    let mut index = 0usize;
    while index < raw_args.len() {
        let arg = &raw_args[index];
        if [
            "--interval-ms",
            "--state-dir",
            "--request-timeout-ms",
            "--lease-holder",
            "--holder",
            "--lease-id",
        ]
        .contains(&arg.as_str())
        {
            index += if index + 1 < raw_args.len() && !raw_args[index + 1].starts_with("--") {
                2
            } else {
                1
            };
            continue;
        }
        if ["--recover", "--via-daemon", "--local", "--max-iterations"].contains(&arg.as_str()) {
            if arg == "--recover" {
                index += 1;
                continue;
            }
            return Err(CliError::usage(format!(
                "session monitor-policy set cannot store {arg}"
            )));
        }
        out.push(arg.clone());
        index += 1;
    }
    Ok(out)
}

pub(super) fn run_session_status(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    runtime_session_adapter::run_status(global, args)
}

fn session_instance_registry_contract(config: &UserConfig) -> CliOutcome<Value> {
    let instances = config
        .instances
        .iter()
        .map(|(id, instance)| session_instance_registry_entry(id, instance))
        .collect::<CliOutcome<Vec<_>>>()?;
    Ok(json!({
        "schema_version": "session.instance_registry.v0.1",
        "source": "user_config",
        "available": true,
        "count": instances.len(),
        "required_fields": ["serial", "game", "server"],
        "recommended_fields": ["package", "adb_path", "capture_backend", "touch_backend"],
        "capture_backends": ["auto", "adb", "droidcast_raw", "nemu_ipc", "auto-fastest"],
        "touch_backends": ["auto", "auto-fastest", "maatouch", "minitouch", "adb_shell_input"],
        "instances": instances
    }))
}

fn session_instance_registry_entry(id: &str, instance: &InstanceConfig) -> CliOutcome<Value> {
    let effective_capture_backend = match instance.capture_backend.as_deref() {
        Some(value) => CaptureBackendChoice::parse(value)
            .map_err(|err| {
                CliError::usage(format!(
                    "invalid instance.{id}.capture_backend '{value}': {err}"
                ))
            })?
            .as_str()
            .to_string(),
        None => CaptureBackendChoice::Auto.as_str().to_string(),
    };
    let effective_touch_backend = match instance.touch_backend.as_deref() {
        Some(value) => TouchBackendChoice::parse(value)
            .map_err(|err| {
                CliError::usage(format!(
                    "invalid instance.{id}.touch_backend '{value}': {err}"
                ))
            })?
            .as_str()
            .to_string(),
        None => TouchBackendChoice::Auto.as_str().to_string(),
    };
    let missing_required_fields = instance_missing_required_fields(instance);
    let missing_recommended_fields = instance_missing_recommended_fields(instance);
    Ok(json!({
        "id": id,
        "serial": instance.serial,
        "game": instance.game,
        "server": instance.server,
        "package": instance.package,
        "adb_path": instance.adb_path,
        "capture_backend": instance.capture_backend,
        "touch_backend": instance.touch_backend,
        "configured": {
            "serial": instance.serial.is_some(),
            "game": instance.game.is_some(),
            "server": instance.server.is_some(),
            "package": instance.package.is_some(),
            "adb_path": instance.adb_path.is_some(),
            "capture_backend": instance.capture_backend.is_some(),
            "touch_backend": instance.touch_backend.is_some()
        },
        "effective": {
            "capture_backend": effective_capture_backend,
            "touch_backend": effective_touch_backend,
            "adb_path": instance.adb_path,
            "adb_path_source": if instance.adb_path.is_some() { "instance_config" } else { "resolver_default" }
        },
        "validation": {
            "ready_for_device_control": missing_required_fields.is_empty(),
            "missing_required_fields": missing_required_fields,
            "missing_recommended_fields": missing_recommended_fields
        }
    }))
}

fn instance_missing_required_fields(instance: &InstanceConfig) -> Vec<&'static str> {
    [
        ("serial", instance.serial.is_none()),
        ("game", instance.game.is_none()),
        ("server", instance.server.is_none()),
    ]
    .into_iter()
    .filter_map(|(field, missing)| missing.then_some(field))
    .collect()
}

fn instance_missing_recommended_fields(instance: &InstanceConfig) -> Vec<&'static str> {
    [
        ("package", instance.package.is_none()),
        ("adb_path", instance.adb_path.is_none()),
        ("capture_backend", instance.capture_backend.is_none()),
        ("touch_backend", instance.touch_backend.is_none()),
    ]
    .into_iter()
    .filter_map(|(field, missing)| missing.then_some(field))
    .collect()
}

pub(super) fn run_session_instance(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let action = args
        .first()
        .map(String::as_str)
        .ok_or_else(|| CliError::usage("session instance requires list|registry|app"))?;
    if action == "app" {
        if args.get(1).is_none() {
            return Err(CliError::usage(
                "session instance app requires launch|stop|force-stop|restart",
            ));
        }
        return run_session_app_or_recorded(global, &args[1..]);
    }
    let flags = FlagArgs::parse(&args[1..])?;
    reject_legacy_session_routing(&flags)?;
    let config = read_user_config()?;
    match action {
        "list" => Ok(json!({
            "instances": config.instances.iter().map(|(id, instance)| json!({
                "id": id,
                "serial": instance.serial,
                "game": instance.game,
                "server": instance.server,
                "package": instance.package,
                "adb_path": instance.adb_path,
                "capture_backend": instance.capture_backend
            })).collect::<Vec<_>>()
        })),
        "registry" => session_instance_registry_contract(&config),
        "connect" | "health" | "keep-alive" | "reconnect" => Err(CliError::not_implemented(
            "actinglab_device_authority_retired",
            format!(
                "session instance {action} directly owned device state in ActingLab and is retired; use Runtime-backed status or control APIs"
            ),
        )),
        other => Err(CliError::usage(format!(
            "unknown session instance action: {other}"
        ))),
    }
}

pub(super) fn run_session_app(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let action = args
        .first()
        .map(String::as_str)
        .ok_or_else(|| CliError::usage("session app requires launch|stop|force-stop|restart"))?;
    let flags = FlagArgs::parse(&args[1..])?;
    reject_legacy_session_routing(&flags)?;
    if flags.optional("--package").is_some() {
        return Err(CliError::usage(
            "--package is not accepted by ActingLab; application identity is owned by Runtime configuration",
        ));
    }
    let config = read_user_config()?;
    let instance_id = resolve_instance_id_for_flags(global, &config, &flags)?;
    let action = match action {
        "launch" => ApplicationLifecycleAction::Launch,
        "stop" | "force-stop" => ApplicationLifecycleAction::Stop,
        "restart" => ApplicationLifecycleAction::Restart,
        other => Err(CliError::usage(format!(
            "unknown session app action: {other}"
        )))?,
    };
    crate::dry_run_gate::refuse(
        global,
        &format!("session app {}", args[0]),
        Some("session status --diagnostics"),
        json!({"action": args[0], "instance": instance_id}),
    )?;
    let client = RuntimeClient::connect(RuntimeClientConfig::new(
        runtime_state_root()?,
        EventActor::Cli,
        EventSource::Cli,
    ))
    .map_err(runtime_slice_cli::map_runtime_error)?;
    let output = client
        .control_application(&instance_id, action)
        .map_err(runtime_slice_cli::map_runtime_error)?;
    serde_json::to_value(output)
        .map_err(|error| CliError::usage(format!("failed to serialize Runtime receipt: {error}")))
}

/// `session app` and `session instance app`: with `--record` the operation becomes an
/// application step of the active Lab recording (Workflow #336 R24); without it the command
/// is `run_session_app`, unchanged.
pub(super) fn run_session_app_or_recorded(
    global: &GlobalOptions,
    args: &[String],
) -> CliOutcome<Value> {
    if args.iter().any(|arg| arg == "--record") {
        run_session_app_recorded(global, args)
    } else {
        run_session_app(global, args)
    }
}

/// `session app <launch|restart|stop|force-stop> --record`. The recording lock is held from
/// before the request until the result is recorded, and the step is planned before anything
/// is sent. The branches read the raw `RuntimeClientError`: a denied receipt never ran the
/// operation (returned as the plain command returns it, nothing recorded); any other error
/// may have run it (one attempt recorded, `record_application_indeterminate`).
fn run_session_app_recorded(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let verb = args
        .first()
        .map(String::as_str)
        .ok_or_else(|| CliError::usage("session app requires launch|stop|force-stop|restart"))?;
    let flags = FlagArgs::parse(&args[1..])?;
    reject_legacy_session_routing(&flags)?;
    if flags.optional("--package").is_some() {
        return Err(CliError::usage(
            "--package is not accepted by ActingLab; application identity is owned by Runtime configuration",
        ));
    }
    let config = read_user_config()?;
    let instance_id = resolve_instance_id_for_flags(global, &config, &flags)?;
    let action = match verb {
        "launch" => ApplicationLifecycleAction::Launch,
        "stop" | "force-stop" => ApplicationLifecycleAction::Stop,
        "restart" => ApplicationLifecycleAction::Restart,
        other => Err(CliError::usage(format!(
            "unknown session app action: {other}"
        )))?,
    };
    let lock = crate::commands::record_flag_begin(
        global,
        &flags,
        &instance_id,
        &format!("session app {verb} --record"),
    )?;
    let plan = record_plan_application(
        &lock,
        &PlanApplicationRequest {
            verb: verb.to_string(),
        },
    )?;
    let client = RuntimeClient::connect(RuntimeClientConfig::new(
        runtime_state_root()?,
        EventActor::Cli,
        EventSource::Cli,
    ))
    .map_err(runtime_slice_cli::map_runtime_error)?;
    let output = match client.control_application(&instance_id, action) {
        Ok(output) => output,
        Err(error) => return Err(application_not_completed(&lock, &plan, &error)),
    };
    let performed = performed_execution(&output).and_then(|execution| {
        serde_json::to_value(&output)
            .map(|data| (execution, data))
            .map_err(|error| {
                CliError::usage(format!("failed to serialize Runtime receipt: {error}"))
            })
    });
    let (execution, mut data) =
        performed.map_err(|error| record_application_append_failed(&plan, "performed", error))?;
    match record_commit_application(
        &lock,
        &plan,
        &CommitApplicationRequest::Performed(execution),
    ) {
        Ok(outcome) => {
            data["record"] = serde_json::to_value(outcome).map_err(|error| {
                CliError::usage(format!("failed to encode the record: {error}"))
            })?;
            Ok(data)
        }
        Err(mut error) => {
            let mut details = error.details.take().unwrap_or_else(|| json!({}));
            if let Some(object) = details.as_object_mut() {
                object.insert("operation".to_string(), data);
            }
            Err(error.with_details(details))
        }
    }
}

/// The completed receipt of the application operation, as the recording stores it.
fn performed_execution(output: &RuntimeFlowOutput) -> CliOutcome<ApplicationExecution> {
    let receipt = output.receipt();
    let Some(RuntimeResult::ApplicationLifecycleCompleted { action_id, .. }) = receipt.result()
    else {
        return Err(CliError::device(
            "the completed application receipt carries no application result",
        ));
    };
    let application_event_ids = output
        .events()
        .iter()
        .filter(|event| {
            matches!(
                event.event_type,
                EventType::ApplicationIntent
                    | EventType::ApplicationCompleted
                    | EventType::ApplicationFailed
            )
        })
        .map(|event| runtime_text(&event.event_id))
        .collect::<CliOutcome<Vec<_>>>()?;
    Ok(ApplicationExecution {
        request_id: runtime_text(&receipt.request_id())?,
        correlation_id: runtime_text(&receipt.correlation_id())?,
        action_id: runtime_text(action_id)?,
        receipt_state: runtime_text(&receipt.state())?,
        application_event_ids,
    })
}

/// `control_application` returned an error. A denied receipt means the Runtime refused before
/// executing: the error is mapped as the plain command maps it and the recording is untouched.
/// Everything else (a failed receipt, no receipt at all) may have run the operation: the
/// attempt is recorded on the planned step, which stays open.
fn application_not_completed(
    lock: &RecordingLock,
    plan: &ApplicationPlan,
    error: &RuntimeClientError,
) -> CliError {
    let receipt = error.received_receipt();
    if receipt.is_some_and(|receipt| receipt.state() == RuntimeReceiptState::Denied) {
        return runtime_slice_cli::map_runtime_error(error.clone());
    }
    let attempt = (|| -> CliOutcome<ApplicationAttempt> {
        let runtime_code = match (error.host_failure(), error.projection()) {
            (Some((code, _)), _) => code.to_string(),
            (None, Some(projection)) => runtime_text(&projection.code)?,
            (None, None) => error.code().to_string(),
        };
        let header_request = error
            .receipt_header_io()
            .and_then(|header| header.request_id());
        let request_id = match (receipt, header_request) {
            (Some(receipt), _) => Some(runtime_text(&receipt.request_id())?),
            (None, Some(request_id)) => Some(runtime_text(request_id)?),
            (None, None) => None,
        };
        Ok(ApplicationAttempt {
            receipt_state: match receipt {
                Some(receipt) => runtime_text(&receipt.state())?,
                None => "none".to_string(),
            },
            runtime_code: Some(runtime_code),
            request_id,
        })
    })();
    let attempt = match attempt {
        Ok(attempt) => attempt,
        Err(cause) => return record_application_append_failed(plan, "indeterminate", cause),
    };
    let Err(mut failure) = record_commit_application(
        lock,
        plan,
        &CommitApplicationRequest::Indeterminate(attempt),
    ) else {
        return CliError::new(
            LabErrorClass::DeviceInstance,
            "record_application_indeterminate",
            "the application operation has no completed receipt, but the Lab recording \
             reported it recorded without an error",
            &["device"],
        );
    };
    let mut details = failure.details.take().unwrap_or_else(|| json!({}));
    if let Some(object) = details.as_object_mut() {
        object.insert("runtime_error".to_string(), json!(error.to_string()));
    }
    failure.with_details(details)
}

/// The JSON string of a typed Runtime identifier, state or code.
fn runtime_text<T: serde::Serialize>(value: &T) -> CliOutcome<String> {
    match serde_json::to_value(value) {
        Ok(Value::String(text)) => Ok(text),
        Ok(other) => Err(CliError::usage(format!(
            "a Runtime value encodes as {other}, not as a string"
        ))),
        Err(error) => Err(CliError::usage(format!(
            "failed to encode a Runtime value: {error}"
        ))),
    }
}
