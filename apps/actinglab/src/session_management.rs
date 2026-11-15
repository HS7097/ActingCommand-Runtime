use crate::commands::device_commands::{LEGACY_ROUTING_READS, capture_for_command};
use crate::flag_args::reads::{FlagReads, ReadPart};
use crate::instance_resolution::resolve_instance_id_for_flags;
use crate::runtime_slice_cli;
use crate::session_backend::RuntimeBackendRead;
use crate::state_roots::runtime_state_root;
use crate::{
    CaptureBackendChoice, CliError, CliOutcome, DeviceRuntimeConfig, FlagArgs, GlobalOptions,
    InstanceConfig, TouchBackendChoice, UserConfig, device_config, read_user_config,
    reject_legacy_session_routing, runtime_session_adapter,
};
use actingcommand_contract::{
    ApplicationLifecycleAction, EventType, LabErrorClass, RuntimeReceiptState, RuntimeResult,
};
use actingcommand_lab::{
    AfterCapture, AfterFrame, ApplicationAttempt, ApplicationExecution, ApplicationPlan,
    AttachFrameRequest, CapturedFrame, CommitApplicationOutcome, CommitApplicationRequest,
    LAB_RECORDING_MAX_NEXT_FRAME_MS, LabStatus, OpaqueJson, PlanApplicationRequest,
    RecordFramePorts, RecordingLock, record_after_frame, record_application_append_failed,
    record_before_frame, record_commit_application, record_plan_application, record_status,
};
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientError, RuntimeFlowOutput};
use serde_json::{Map, Value, json};
use std::time::Duration;

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

/// #402 HS-L: each entry's `runtime_backends` is read from the Runtime (`runtime`), never from
/// this file; `effective` stays the file's reading.
fn session_instance_registry_contract(
    config: &UserConfig,
    runtime: &RuntimeBackendRead,
) -> CliOutcome<Value> {
    let instances = config
        .instances
        .iter()
        .map(|(id, instance)| {
            session_instance_registry_entry(id, instance).map(|mut entry| {
                entry["runtime_backends"] = runtime.column(id);
                entry
            })
        })
        .collect::<CliOutcome<Vec<_>>>()?;
    Ok(json!({
        "runtime_backends_read": runtime.summary(),
        "schema_version": "session.instance_registry.v0.1",
        "source": "user_config",
        "available": true,
        "count": instances.len(),
        "required_fields": ["serial", "game", "server"],
        "recommended_fields": ["package", "adb_path", "capture_backend", "touch_backend"],
        // Workflow #411 Lab L4: the choices an actingd instance may name; actingd refuses the
        // automatic ones (`*_backend_must_be_explicit`).
        "capture_backends": ["adb", "droidcast_raw", "nemu_ipc", "scrcpy"],
        "touch_backends": ["nemu_ipc", "maatouch", "minitouch", "adb_shell_input"],
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
        "serial_configured": instance.serial.is_some(),
        "device_label": device_label(id, instance),
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

/// Workflow #398 RD-1a (L8): an instance with a configured serial is shown by its label
/// `handset:<id>`, never by the serial.
fn device_label(id: &str, instance: &InstanceConfig) -> Option<String> {
    instance
        .serial
        .as_ref()
        .map(|_| actingcommand_device::handset_label(id))
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

/// Workflow #434 LAB-5B5 (register §1.287, "Slice LAB-5B5"): the flags `session instance` reads
/// after its action word (its first word, read before; `app` hands the rest to `session app`): the
/// legacy routing it refuses; no positional after the action.
pub(crate) const SESSION_INSTANCE_READS: FlagReads = FlagReads {
    row: "session instance",
    parts: &[&LEGACY_ROUTING_READS],
    positionals: &[],
};

/// Workflow #434 LAB-5B5: the flags `session app` (and `session instance app`) reads after its
/// action word, on its plain and its `--record` path: the legacy routing and `--package`, which
/// it refuses, the instance (actinglab's global option) and `--record` (which the command finds
/// among its words, `run_session_app_or_recorded`); no positional after the action. #463 S1-3: and
/// the waits of the screen after a recorded operation, `APPLICATION_RECORD_ONLY_FLAGS`.
pub(crate) const SESSION_APP_READS: FlagReads = FlagReads {
    row: "session app",
    parts: &[
        &LEGACY_ROUTING_READS,
        &ReadPart {
            options: &["--instance"],
            switches: &["--record"],
            refused: &["--package"],
        },
        &ReadPart {
            options: &APPLICATION_RECORD_ONLY_FLAGS,
            switches: &[],
            refused: &[],
        },
    ],
    positionals: &[],
};

/// #463 S1-3 (`contracts/lab-recording.md`, "Application steps"): the flags `session app <action>`
/// takes with `--record` only: the wait before the screen after the operation and the bound of the
/// wait for a still screen (`application_after_capture`).
const APPLICATION_RECORD_ONLY_FLAGS: [&str; 2] = ["--after-ms", "--still-max-ms"];

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
    let flags = FlagArgs::parse_reading(&args[1..], &SESSION_INSTANCE_READS)?;
    reject_legacy_session_routing(&flags)?;
    let config = read_user_config()?;
    match action {
        // #402 HS-L: the column `runtime_backends` is the Runtime's touch and capture backends in
        // effect for the instance of that alias; `capture_backend` stays this file's.
        "list" => {
            let runtime = RuntimeBackendRead::now();
            Ok(json!({
                "instances": config.instances.iter().map(|(id, instance)| json!({
                    "id": id,
                    "serial_configured": instance.serial.is_some(),
                    "device_label": device_label(id, instance),
                    "game": instance.game,
                    "server": instance.server,
                    "package": instance.package,
                    "adb_path": instance.adb_path,
                    "capture_backend": instance.capture_backend,
                    "runtime_backends": runtime.column(id)
                })).collect::<Vec<_>>(),
                "runtime_backends_read": runtime.summary()
            }))
        }
        "registry" => session_instance_registry_contract(&config, &RuntimeBackendRead::now()),
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
    let flags = FlagArgs::parse_reading(&args[1..], &SESSION_APP_READS)?;
    reject_legacy_session_routing(&flags)?;
    // #463 S1-3: the waits of the screen after the operation come with `--record` only.
    if let Some(flag) = APPLICATION_RECORD_ONLY_FLAGS
        .into_iter()
        .find(|flag| flags.given(flag))
    {
        return Err(CliError::usage(format!(
            "{flag} is accepted only with session app <launch|restart|stop|force-stop> --record"
        ))
        .with_details(json!({
            "argument": flag.trim_start_matches("--").replace('-', "_"),
            "reason": "record_only"
        })));
    }
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
    let client = RuntimeClient::connect(crate::lab_client_config(runtime_state_root()?))
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
/// may have run it (one attempt recorded, `record_application_indeterminate`). #463 S1-3: the
/// operation is a step with the screen before it and the screen after it, each taken as `capture
/// --record` takes it ([`record_application_operation`]); `--after-ms` and `--still-max-ms` say how
/// the screen after it is taken ([`application_after_capture`]).
fn run_session_app_recorded(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let verb = args
        .first()
        .map(String::as_str)
        .ok_or_else(|| CliError::usage("session app requires launch|stop|force-stop|restart"))?;
    let flags = FlagArgs::parse_reading(&args[1..], &SESSION_APP_READS)?;
    reject_legacy_session_routing(&flags)?;
    if flags.optional("--package").is_some() {
        return Err(CliError::usage(
            "--package is not accepted by ActingLab; application identity is owned by Runtime configuration",
        ));
    }
    let after = application_after_capture(&flags)?;
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
    // The device configuration of the captures, resolved before anything is captured or sent; the
    // entry step takes no frame and asks for none.
    let frames = || device_config(global, &config).map(|device| ApplicationFrames { device });
    let operate = |plan: &ApplicationPlan| -> CliOutcome<Operated> {
        let client = RuntimeClient::connect(crate::lab_client_config(runtime_state_root()?))
            .map_err(runtime_slice_cli::map_runtime_error)?;
        let output = client
            .control_application(&instance_id, action)
            .map_err(|error| application_not_completed(&lock, plan, &error))?;
        performed_execution(&output)
            .and_then(|execution| {
                serde_json::to_value(&output)
                    .map(|data| Operated { execution, data })
                    .map_err(|error| {
                        CliError::usage(format!("failed to serialize Runtime receipt: {error}"))
                    })
            })
            .map_err(|error| record_application_append_failed(plan, "performed", error))
    };
    record_application_operation(
        &lock,
        &PlanApplicationRequest {
            verb: verb.to_string(),
        },
        &after,
        frames,
        operate,
    )
}

/// #463 S1-3 (`contracts/lab-recording.md`, "Application steps"): how the screen after a recorded
/// application operation is taken, as `record_after_frame` is handed it: `--after-ms` and
/// `--still-max-ms` as given, each a whole number of milliseconds from 0 to 60000, else
/// `validation_failed` with `details{argument, reason: "range"}`; without them
/// `AfterCapture::for_application`'s 2000 and 30000.
pub(crate) fn application_after_capture(flags: &FlagArgs) -> CliOutcome<AfterCapture> {
    let after_ms = wait_flag(flags, "--after-ms")?;
    let still_max_ms = wait_flag(flags, "--still-max-ms")?;
    let after = AfterCapture::for_application(after_ms);
    Ok(AfterCapture {
        still_max_ms: still_max_ms.unwrap_or(after.still_max_ms),
        ..after
    })
}

/// #463 S1-3: the milliseconds of the flag `flag`, when it is given.
fn wait_flag(flags: &FlagArgs, flag: &str) -> CliOutcome<Option<u64>> {
    let Some(value) = flags.optional(flag) else {
        return Ok(None);
    };
    match value.parse::<u64>() {
        Ok(ms) if ms <= LAB_RECORDING_MAX_NEXT_FRAME_MS => Ok(Some(ms)),
        _ => Err(CliError::usage(format!(
            "{flag} takes a whole number of milliseconds from 0 to \
             {LAB_RECORDING_MAX_NEXT_FRAME_MS}, got {value}"
        ))
        .with_details(json!({
            "argument": flag.trim_start_matches("--").replace('-', "_"),
            "reason": "range",
            "value": value
        }))),
    }
}

/// #463 S1-3: an application operation with a completed receipt: its execution as the recording
/// stores it, and the answer of the plain `session app`.
pub(crate) struct Operated {
    pub(crate) execution: ApplicationExecution,
    pub(crate) data: Value,
}

/// #463 S1-3 (`contracts/lab-recording.md`, "Application steps"): what `session app <verb>
/// --record` does under the recording lock. `operate` sends the operation once: a completed
/// receipt is [`Operated`], any other answer its error (the attempt already recorded where the
/// operation may have run). `frames` gives the caller's clock and captures.
/// - An empty recording: the operation opens the application entry step, with no screen before it
///   and none after it (the next press's or capture's frame opens the step after it); `frames` is
///   never asked.
/// - An open step: the plan on it first, whose refusals capture and send nothing. Round 2: then
///   the screen before as a press takes it, captured immediately before the operation
///   ([`record_before_frame`], its frame reference unused: an application operation names no
///   frame): on a step without marks it is the step's live primary frame, the older one
///   superseded; on a step with marks it is not stored and the stored frame stands. A step that
///   already declares the operation (an earlier attempt without a completed receipt, or `record
///   mark --application`) takes no frame, since it refuses a device frame
///   (`record_step_click_not_executed`): the frame it was declared on stands.
/// - No open step after a closed one: the screen before, which opens the step the operation
///   lands on, then the plan on that step.
///
/// Then the operation; its completed receipt recorded, naming the step's live primary frame as its
/// `before_frame` (none on an entry step, retried too); then, only for a receipt that names its
/// `before_frame`, the screen after it ([`record_after_frame`] with `after`), which opens the next
/// step and is the execution's `after_frame`, with its `after_capture`. A screen after that cannot
/// be taken answers its own code and class, and the operation and its closed step stand: its
/// details begin with `record` (the operation as recorded), `after: {after_ms, taken: false}` and
/// `operation`, then the capture's own members.
///
/// The answer is the plain command's with `record`: the operation as recorded and `after`, the
/// screen after it (`null` on the entry step).
pub(crate) fn record_application_operation<P: RecordFramePorts>(
    lock: &RecordingLock,
    request: &PlanApplicationRequest,
    after: &AfterCapture,
    frames: impl FnOnce() -> CliOutcome<P>,
    operate: impl FnOnce(&ApplicationPlan) -> CliOutcome<Operated>,
) -> CliOutcome<Value> {
    let (plan, mut ports) = match record_plan_application(lock, request) {
        Ok(plan) if plan.opens_entry_step => (plan, None),
        planned => match (planned, open_step_declares(lock)?) {
            (Ok(plan), Some(true)) => (plan, Some(frames()?)),
            (Ok(plan), Some(false)) => {
                let mut ports = frames()?;
                record_before_frame(lock, &mut ports)?;
                (plan, Some(ports))
            }
            (Err(error), Some(_)) => return Err(error),
            // A refusal without an open step is the next plan's, after the screen before.
            (_, None) => {
                let mut ports = frames()?;
                record_before_frame(lock, &mut ports)?;
                (record_plan_application(lock, request)?, Some(ports))
            }
        },
    };
    let Operated {
        execution,
        mut data,
    } = operate(&plan)?;
    let mut outcome = match record_commit_application(
        lock,
        &plan,
        &CommitApplicationRequest::Performed(execution),
    ) {
        Ok(outcome) => outcome,
        Err(error) => return Err(with_operation(error, data)),
    };
    let names_before = outcome
        .application
        .executed
        .as_ref()
        .is_some_and(|execution| execution.before_frame.is_some());
    let after_frame = match ports.as_mut().filter(|_| names_before) {
        None => None,
        Some(device) => {
            let (frame, record) = match record_after_frame(lock, device, after) {
                Ok(taken) => taken,
                Err(error) => {
                    return Err(after_frame_failed(error, &outcome, after.after_ms, data));
                }
            };
            // The operation as recorded now: its execution names the screen after it.
            if let Some(execution) = outcome.application.executed.as_mut() {
                execution.after_frame = Some(frame.frame.frame_id.clone());
                execution.after_capture = Some(record.clone());
            }
            Some(AfterFrame {
                step: frame.step,
                frame_id: frame.frame.frame_id,
                path: frame.frame.path,
                sha256: frame.frame.sha256,
                record,
            })
        }
    };
    let mut record = record_json(&outcome)?;
    record["after"] = record_json(&after_frame)?;
    data["record"] = record;
    Ok(data)
}

/// #463 S1-3 round 2: whether a step of the recording is open (`None` when none is) and, when one
/// is, whether it already declares an application operation, read as `record status` reads it.
fn open_step_declares(lock: &RecordingLock) -> CliOutcome<Option<bool>> {
    let Some(LabStatus::Recording(view)) = record_status(lock.state_dir(), lock.instance())? else {
        return Ok(None);
    };
    Ok(view.open_step.map(|index| {
        view.steps
            .iter()
            .any(|step| step.index == index && step.application.is_some())
    }))
}

/// The JSON of what the recording answered, for the command's `record`.
fn record_json<T: serde::Serialize>(value: &T) -> CliOutcome<Value> {
    serde_json::to_value(value)
        .map_err(|error| CliError::usage(format!("failed to encode the record: {error}")))
}

/// `error` with the plain command's answer `data` as the member `operation` of its details.
fn with_operation(mut error: CliError, data: Value) -> CliError {
    let mut details = error.details.take().unwrap_or_else(|| json!({}));
    if let Some(object) = details.as_object_mut() {
        object.insert("operation".to_string(), data);
    }
    error.with_details(details)
}

/// #463 S1-3: the error of a screen after an application operation that could not be taken: the
/// capture's own code and class, its message saying that the operation stands, its details
/// beginning with `record` (the operation as recorded), `after: {after_ms, taken: false}` and
/// `operation` (the plain command's answer), then the capture's own members.
fn after_frame_failed(
    mut error: CliError,
    outcome: &CommitApplicationOutcome,
    after_ms: u64,
    data: Value,
) -> CliError {
    error.message = format!(
        "{}; the application operation of step {} was performed and recorded and the step is \
         closed: take the screen after it with capture --record, do not run the operation again",
        error.message, outcome.step
    );
    let mut details = Map::new();
    details.insert(
        "record".to_owned(),
        serde_json::to_value(outcome).unwrap_or(Value::Null),
    );
    details.insert(
        "after".to_owned(),
        json!({"after_ms": after_ms, "taken": false}),
    );
    details.insert("operation".to_owned(), data);
    match error.details.take() {
        Some(Value::Object(members)) => {
            for (name, value) in members {
                details.entry(name).or_insert(value);
            }
        }
        Some(other) => {
            details.entry("details".to_owned()).or_insert(other);
        }
        None => {}
    }
    error.with_details(Value::Object(details))
}

/// #463 S1-3: the caller's clock and the Runtime's capture of `session app <verb> --record`: each
/// frame is taken as `capture --record` takes it, on the device configuration of the command's
/// instance; round 2: with the reference of the frame the Runtime committed for it, as a press's
/// capture carries it (`record_before_frame` asks for it; the operation names none).
struct ApplicationFrames {
    device: DeviceRuntimeConfig,
}

impl RecordFramePorts for ApplicationFrames {
    fn wait(&mut self, duration: Duration) {
        std::thread::sleep(duration);
    }

    fn capture(&mut self) -> CliOutcome<CapturedFrame> {
        let captured = capture_for_command(
            &self.device,
            self.device.capture_backend,
            false,
            Duration::ZERO,
        )?;
        let png = captured
            .frame
            .png_for_artifact()
            .map_err(|err| CliError::device(err.to_string()))?;
        Ok(CapturedFrame {
            frame: AttachFrameRequest {
                png,
                source: "lab_capture".to_string(),
                runtime_artifact: None,
                capture_backend: Some(captured.frame.backend_name.as_str().to_string()),
                freshness: Some(OpaqueJson::from_serializable(&captured.freshness)?),
            },
            input_frame: captured.input_frame,
        })
    }
}

/// The completed receipt of the application operation, as the recording stores it. #463 S1-3: its
/// screens before and after are the recording's to name (`commit_application`, the frame after
/// it), none here; #463 S1-5: so is its time, stamped where it is committed as a click's is.
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
        before_frame: None,
        after_frame: None,
        after_capture: None,
        executed_at_unix_ms: None,
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
