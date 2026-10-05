// SPDX-License-Identifier: AGPL-3.0-only

//! Lab subprocess tools (#338 §四 author, S3): every call runs `<root>\tools\actinglab.exe
//! --json …` through the child helper and passes the envelope's `data` on unchanged. The
//! exit code gives the class (`cli_result.rs:93-100`: 2 usage, 3 safety, 4 device,
//! 5 runtime, 6 usage `not_implemented`); any other exit code, or no envelope, is runtime
//! `lab_process_failed` with the stderr tail. The envelope's error goes into
//! `details.lab_error` verbatim.
//!
//! Each call runs as a background job of this process: answered within the call budget, or
//! a handle for `ac_get_run`. The child is never killed; the handle lives only in this
//! process's memory.
//!
//! Provenance (open point 6): only `observe --capture` and `do --capture` are Lab requests
//! the Runtime records in its ledger (`lab.request`, written for Lab-origin client commands
//! and for a debug session's requested phase). After such a child returns, one
//! `client.action{surface_id: "mcp", control_id: <tool>, kind: Command, value:
//! PathSafeString(<envelope req_id>)}` is recorded on a fresh (Cli, Cli) connection. The
//! offline forms and the recording commands (start, mark, stop, status), which keep their
//! state in files and open no Runtime connection, record nothing, nor do the read-only
//! tools.

use super::child::{self, Captured, ChildFailure};
use super::jobs::JobEnd;
use super::operator::{self, CLI};
use super::runtime;
use super::tools::{self, Arguments, ToolContext, ToolError, ToolOutcome, invalid_argument};
use actingcommand_contract::{ClientActionValue, PackageRef};
use serde_json::{Map, Value, json};
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

/// Every Lab job handle starts so; one this process does not hold answers `handle_unknown`.
pub(super) const HANDLE_PREFIX: &str = "lab_job_";
/// actinglab finds the Runtime it talks to here (`state_roots.rs:11-24`).
const STATE_ROOT_ENV: &str = "ACTINGCOMMAND_RUNTIME_STATE_ROOT";
/// The tail of a failed child's stderr kept in the result.
const MAX_STDERR_TAIL: usize = 2048;
/// A string argument may not look like a flag: actinglab's flag parser takes a value that
/// starts with `--` for the next flag (`flag_args.rs:27-28`).
const NOT_A_FLAG: &str = "^([^-]|-([^-]|$))";
/// Counts this process's Lab jobs.
static NEXT_JOB: AtomicU64 = AtomicU64::new(1);
const MANUAL_STEP: &str = "edit the configuration, approve in the UI, restart the daemon";

// ---------------------------------------------------------------- command tables

#[derive(Clone, Copy)]
enum Kind {
    /// `--flag <text>`.
    Text,
    /// `--flag <integer>`.
    Integer,
    /// `--flag <number>`.
    Number,
    /// `--flag` when true.
    Switch,
    /// A positional argument right after the command words.
    Positional,
    /// `--request-json <json>`: an `actingcommand.lab-record-mark.v1` request.
    MarkRequest,
}

/// One tool argument and the actinglab flag it maps to, one to one.
struct Flag {
    property: &'static str,
    flag: &'static str,
    kind: Kind,
    description: &'static str,
}

/// One actinglab command and its flags (`lab2_cli.rs:1414-1667`, the package locator flags
/// of `contained_resources.rs:125-216`, the global `--instance` of `cli_parse.rs`, and for
/// `do` the `--verbose` / `--pretty` it reads at `lab2_cli/operation.rs:122` and
/// `lab2_cli.rs:1373`).
struct LabCommand {
    words: &'static [&'static str],
    flags: &'static [Flag],
    required: &'static [&'static str],
    /// With `capture: true` the command is a Lab request the Runtime records.
    capture_is_lab_request: bool,
}

const fn flag(
    property: &'static str,
    flag: &'static str,
    kind: Kind,
    description: &'static str,
) -> Flag {
    Flag {
        property,
        flag,
        kind,
        description,
    }
}

const LAB2_INSTANCE: Flag = flag(
    "instance",
    "--instance",
    Kind::Text,
    "actinglab --instance: with capture, the Runtime instance alias; without it actinglab uses its default instance.",
);
const RECORD_INSTANCE: Flag = flag(
    "instance",
    "--instance",
    Kind::Text,
    "actinglab --instance: the recording's instance; without it actinglab resolves it from its configuration.",
);
const SCENE: Flag = flag(
    "scene",
    "--scene",
    Kind::Text,
    "--scene: offline, the PNG scene file to read.",
);
const CAPTURE: Flag = flag(
    "capture",
    "--capture",
    Kind::Switch,
    "--capture: Runtime-backed, the Runtime takes the instance's current frame.",
);
const PACKAGE: Flag = flag(
    "package",
    "--package",
    Kind::Text,
    "--package: the resource package (directory, .zip or .json container).",
);
const ZIP: Flag = flag(
    "zip",
    "--zip",
    Kind::Text,
    "--zip: a published .zip package, with expected_sha256.",
);
const PACKAGE_REF: Flag = flag(
    "package_ref",
    "--package-ref",
    Kind::Text,
    "--package-ref: the package's reference (a sha256 or the content reference as JSON text).",
);
const EXPECTED_SHA256: Flag = flag(
    "expected_sha256",
    "--expected-sha256",
    Kind::Text,
    "--expected-sha256: the .zip package's sha256.",
);
const FIELDS: Flag = flag(
    "fields",
    "--fields",
    Kind::Text,
    "--fields: the output fields, comma separated.",
);
const TEST_CAPTURE_DELAY: Flag = flag(
    "test_capture_delay_ms",
    "--test-capture-delay-ms",
    Kind::Integer,
    "--test-capture-delay-ms: test only, a delay before the scene file is read.",
);
const LEASE_ID: Flag = flag("lease_id", "--lease-id", Kind::Text, "--lease-id.");
const LOCALE: Flag = flag("locale", "--locale", Kind::Text, "--locale.");
const STATE_DIR: Flag = flag(
    "state_dir",
    "--state-dir",
    Kind::Text,
    "--state-dir: the recording session directory; default actinglab's.",
);
const DRY_RUN: Flag = flag(
    "dry_run",
    "--dry-run",
    Kind::Switch,
    "--dry-run: check and report only.",
);

static OBSERVE: LabCommand = LabCommand {
    words: &["observe"],
    flags: &[
        LAB2_INSTANCE,
        SCENE,
        CAPTURE,
        PACKAGE,
        ZIP,
        PACKAGE_REF,
        EXPECTED_SHA256,
        flag(
            "targets",
            "--targets",
            Kind::Text,
            "--targets: target ids, comma separated.",
        ),
        flag(
            "with_frame",
            "--with-frame",
            Kind::Text,
            "--with-frame: write the observed frame to this PNG path.",
        ),
        flag(
            "require_fresh",
            "--require-fresh",
            Kind::Switch,
            "--require-fresh.",
        ),
        FIELDS,
        flag("verbose", "--verbose", Kind::Switch, "--verbose."),
        flag("pretty", "--pretty", Kind::Switch, "--pretty."),
        flag(
            "record",
            "--record",
            Kind::Switch,
            "--record: with capture, store the frame in the active recording.",
        ),
        TEST_CAPTURE_DELAY,
    ],
    required: &[],
    capture_is_lab_request: true,
};

static DO: LabCommand = LabCommand {
    words: &["do"],
    flags: &[
        LAB2_INSTANCE,
        flag(
            "target",
            "",
            Kind::Positional,
            "The target (offline, with scene) or the Runtime element id (with capture).",
        ),
        SCENE,
        CAPTURE,
        PACKAGE,
        ZIP,
        PACKAGE_REF,
        EXPECTED_SHA256,
        flag("tap", "--tap", Kind::Text, "--tap: x,y."),
        flag(
            "swipe",
            "--swipe",
            Kind::Text,
            "--swipe: x1,y1,x2,y2,duration-ms.",
        ),
        flag(
            "projection_sequence",
            "--projection-sequence",
            Kind::Integer,
            "--projection-sequence.",
        ),
        flag(
            "projection_hash",
            "--projection-hash",
            Kind::Text,
            "--projection-hash: sha256.",
        ),
        flag(
            "after_page",
            "--after-page",
            Kind::Text,
            "--after-page: the full page id to arrive at.",
        ),
        flag(
            "after_timeout_ms",
            "--after-timeout-ms",
            Kind::Integer,
            "--after-timeout-ms: 100..10000, default 5000.",
        ),
        DRY_RUN,
        flag(
            "allow_destructive",
            "--allow-destructive",
            Kind::Switch,
            "--allow-destructive.",
        ),
        flag(
            "destructive",
            "--destructive",
            Kind::Switch,
            "--destructive.",
        ),
        flag(
            "priority",
            "--priority",
            Kind::Text,
            "--priority: normal or high.",
        ),
        LEASE_ID,
        FIELDS,
        flag("verbose", "--verbose", Kind::Switch, "--verbose."),
        flag("pretty", "--pretty", Kind::Switch, "--pretty."),
        flag("no_wait", "--no-wait", Kind::Switch, "--no-wait."),
        flag(
            "recovery_timeout_ms",
            "--recovery-timeout-ms",
            Kind::Integer,
            "--recovery-timeout-ms.",
        ),
        flag(
            "recovery_poll_ms",
            "--recovery-poll-ms",
            Kind::Integer,
            "--recovery-poll-ms.",
        ),
        flag(
            "record",
            "--record",
            Kind::Switch,
            "--record: with capture, click the open recording step and record it.",
        ),
        flag(
            "tap_rect",
            "--tap-rect",
            Kind::Text,
            "--tap-rect: x,y,w,h, only with record: the step's click rectangle.",
        ),
        TEST_CAPTURE_DELAY,
    ],
    required: &[],
    capture_is_lab_request: true,
};

static RECORD_START: LabCommand = LabCommand {
    words: &["record", "start"],
    flags: &[
        RECORD_INSTANCE,
        flag("task_id", "--task-id", Kind::Text, "--task-id."),
        LOCALE,
        flag(
            "metric",
            "--metric",
            Kind::Text,
            "--metric: ccoeff_normed or ccorr_normed.",
        ),
        flag(
            "template_threshold",
            "--template-threshold",
            Kind::Number,
            "--template-threshold: 0..1.",
        ),
        flag("record_id", "--record-id", Kind::Text, "--record-id."),
        flag("holder", "--holder", Kind::Text, "--holder."),
        LEASE_ID,
        STATE_DIR,
    ],
    required: &["task_id"],
    capture_is_lab_request: false,
};

static RECORD_MARK: LabCommand = LabCommand {
    words: &["record", "mark"],
    flags: &[
        RECORD_INSTANCE,
        flag(
            "request",
            "--request-json",
            Kind::MarkRequest,
            "The actingcommand.lab-record-mark.v1 request (contracts/lab-recording.md), passed as --request-json.",
        ),
        DRY_RUN,
        STATE_DIR,
    ],
    required: &["request"],
    capture_is_lab_request: false,
};

static RECORD_STOP: LabCommand = LabCommand {
    words: &["record", "stop"],
    flags: &[
        RECORD_INSTANCE,
        flag(
            "lab_dir",
            "--lab-dir",
            Kind::Text,
            "--lab-dir: where the package directory is written.",
        ),
        flag("package_id", "--package-id", Kind::Text, "--package-id."),
        flag(
            "requires",
            "--requires",
            Kind::Text,
            "--requires: the package_id of the prerequisite package.",
        ),
        flag("game", "--game", Kind::Text, "--game."),
        flag("server", "--server", Kind::Text, "--server."),
        LOCALE,
        flag("timeout_ms", "--timeout-ms", Kind::Integer, "--timeout-ms."),
        flag(
            "arrival_timeout_ms",
            "--arrival-timeout-ms",
            Kind::Integer,
            "--arrival-timeout-ms.",
        ),
        flag(
            "application_arrival_timeout_ms",
            "--application-arrival-timeout-ms",
            Kind::Integer,
            "--application-arrival-timeout-ms.",
        ),
        DRY_RUN,
        STATE_DIR,
    ],
    required: &[],
    capture_is_lab_request: false,
};

static RECORD_STATUS: LabCommand = LabCommand {
    words: &["record", "status"],
    flags: &[RECORD_INSTANCE, STATE_DIR],
    required: &[],
    capture_is_lab_request: false,
};

/// The input schema of `command`: one property per flag, nothing else.
fn input_schema(command: &LabCommand) -> Value {
    let mut properties = Map::new();
    for flag in command.flags {
        let schema = match flag.kind {
            Kind::Text | Kind::Positional => json!({
                "type": "string",
                "minLength": 1,
                "pattern": NOT_A_FLAG,
                "description": flag.description,
            }),
            Kind::Integer => {
                json!({"type": "integer", "minimum": 0, "description": flag.description})
            }
            Kind::Number => json!({"type": "number", "description": flag.description}),
            Kind::Switch => json!({"type": "boolean", "description": flag.description}),
            Kind::MarkRequest => json!({
                "type": "object",
                "properties": {
                    "schema_version": {
                        "type": "string",
                        "const": "actingcommand.lab-record-mark.v1",
                    },
                },
                "required": ["schema_version"],
                "description": flag.description,
            }),
        };
        properties.insert(flag.property.to_owned(), schema);
    }
    json!({
        "type": "object",
        "properties": properties,
        "required": command.required,
        "additionalProperties": false,
    })
}

/// A string argument bound for actinglab's command line.
fn lab_string(arguments: &Arguments<'_>, key: &str) -> Result<Option<String>, ToolError> {
    let Some(text) = arguments.string(key)? else {
        return Ok(None);
    };
    not_a_flag(key, text)?;
    Ok(Some(text.to_owned()))
}

fn not_a_flag(key: &str, text: &str) -> Result<(), ToolError> {
    if text.starts_with("--") {
        return Err(invalid_argument(
            key,
            "must not start with --: actinglab would read it as a flag",
        ));
    }
    Ok(())
}

/// actinglab's arguments after `--json`: the command words, the positional argument, the
/// valued flags, then the switches, so no switch is ever followed by a value.
fn command_line(command: &LabCommand, map: &Map<String, Value>) -> Result<Vec<String>, ToolError> {
    let names = command
        .flags
        .iter()
        .map(|flag| flag.property)
        .collect::<Vec<_>>();
    let arguments = Arguments::new(map, &names)?;
    if let Some(missing) = command.required.iter().find(|key| !arguments.has(key)) {
        return Err(invalid_argument(missing, "is required"));
    }
    let mut line = command
        .words
        .iter()
        .map(|word| (*word).to_owned())
        .collect::<Vec<_>>();
    let mut valued = Vec::new();
    let mut switches = Vec::new();
    for flag in command.flags {
        let Some(value) = arguments.value(flag.property) else {
            continue;
        };
        match flag.kind {
            Kind::Positional => {
                if let Some(text) = lab_string(&arguments, flag.property)? {
                    line.push(text);
                }
            }
            Kind::Text => {
                if let Some(text) = lab_string(&arguments, flag.property)? {
                    valued.extend([flag.flag.to_owned(), text]);
                }
            }
            Kind::Integer => {
                if let Some(number) = arguments.integer(flag.property, 0, u64::MAX)? {
                    valued.extend([flag.flag.to_owned(), number.to_string()]);
                }
            }
            Kind::Number => {
                if !value.as_f64().is_some_and(f64::is_finite) {
                    return Err(invalid_argument(flag.property, "must be a number"));
                }
                valued.extend([flag.flag.to_owned(), value.to_string()]);
            }
            Kind::Switch => match value {
                Value::Bool(true) => switches.push(flag.flag.to_owned()),
                Value::Bool(false) => {}
                _ => return Err(invalid_argument(flag.property, "must be true or false")),
            },
            Kind::MarkRequest => {
                if !value.is_object() {
                    return Err(invalid_argument(
                        flag.property,
                        "must be an actingcommand.lab-record-mark.v1 object",
                    ));
                }
                valued.extend([flag.flag.to_owned(), value.to_string()]);
            }
        }
    }
    line.extend(valued);
    line.extend(switches);
    Ok(line)
}

// ---------------------------------------------------------------- the runner

/// The install's actinglab and the Runtime state root this server talks to.
struct Program {
    actinglab: PathBuf,
    root: PathBuf,
    state_root: Result<PathBuf, String>,
}

/// What one child answered.
pub(super) struct Answer {
    /// The envelope's `data`, or the error its exit code names.
    pub(super) result: Result<Value, ToolError>,
    /// The envelope's req_id (in `data`, or in the error's details).
    req_id: Option<String>,
    /// Whether the child ran to its end (and may have recorded a Lab request).
    returned: bool,
}

impl Program {
    fn locate(context: &ToolContext<'_>) -> Result<Self, ToolError> {
        let location = context.runtime.locate();
        let Some(root) = location.root else {
            return Err(ToolError::usage(
                "lab_tool_unavailable",
                "actingctl does not run from an install root and mcp-serve got no --root, so tools\\actinglab.exe cannot be found",
            )
            .blocked_by("mcp-serve --root <install root>"));
        };
        let actinglab = root.join("tools").join("actinglab.exe");
        if !actinglab.is_file() {
            return Err(ToolError::usage(
                "lab_tool_unavailable",
                format!("{} is not a file", actinglab.display()),
            )
            .blocked_by("an install with its tools directory"));
        }
        Ok(Self {
            actinglab,
            root,
            state_root: location.state_root,
        })
    }

    /// `actinglab --json <arguments>`, talking to this server's Runtime when it is known.
    fn command(&self, arguments: &[String]) -> Command {
        let mut command = Command::new(&self.actinglab);
        command.arg("--json").args(arguments);
        if let Ok(state_root) = &self.state_root {
            command.env(STATE_ROOT_ENV, state_root);
        }
        command
    }

    /// Runs actinglab to its end.
    fn call(&self, arguments: &[String], what: &str) -> Answer {
        match child::run_to_end(self.command(arguments)) {
            Ok(captured) => interpret(&captured, what),
            Err(failure) => Answer {
                result: Err(child_failure(&failure, what)),
                req_id: None,
                returned: false,
            },
        }
    }
}

pub(super) fn child_failure(failure: &ChildFailure, what: &str) -> ToolError {
    match failure {
        ChildFailure::Spawn(error) | ChildFailure::Io(error) => ToolError::new(
            "runtime",
            "lab_process_failed",
            format!("actinglab {what}: {error}"),
        ),
        ChildFailure::StillRunning => ToolError::new(
            "runtime",
            "lab_process_timeout",
            format!("actinglab {what} was still running at the end of the call budget"),
        ),
    }
}

/// Owned command-line words.
fn words(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| (*item).to_owned()).collect()
}

fn stderr_tail(stderr: &[u8]) -> String {
    let start = stderr.len().saturating_sub(MAX_STDERR_TAIL);
    String::from_utf8_lossy(&stderr[start..]).into_owned()
}

/// The envelope's data, or the error its exit code names (`cli_result.rs:93-100`).
pub(super) fn interpret(captured: &Captured, what: &str) -> Answer {
    let exit_code = captured.status.code();
    let envelope = serde_json::from_slice::<Value>(captured.stdout.trim_ascii()).ok();
    let ok = envelope
        .as_ref()
        .and_then(|envelope| envelope.get("ok"))
        .and_then(Value::as_bool);
    let lab_error = envelope
        .as_ref()
        .and_then(|envelope| envelope.get("error"))
        .filter(|error| error.is_object())
        .cloned();
    let req_id = match ok {
        Some(true) => envelope
            .as_ref()
            .and_then(|envelope| envelope.get("data"))
            .and_then(|data| data.get("req_id")),
        _ => lab_error
            .as_ref()
            .and_then(|error| error.get("details"))
            .and_then(|details| details.get("req_id")),
    }
    .and_then(Value::as_str)
    .map(str::to_owned);
    let failed = |reason: String| {
        ToolError::new(
            "runtime",
            "lab_process_failed",
            format!("actinglab {what} {reason}"),
        )
        .with_detail("lab_exit_code", json!(exit_code))
        .with_detail("lab_error", json!(lab_error))
        .with_detail("stderr_tail", json!(stderr_tail(&captured.stderr)))
    };
    let result = match (exit_code, ok) {
        (Some(0), Some(true)) => Ok(envelope
            .as_ref()
            .and_then(|envelope| envelope.get("data"))
            .cloned()
            .unwrap_or(Value::Null)),
        (Some(exit @ 2..=6), Some(false)) => {
            let reported = lab_error.as_ref().and_then(|error| {
                Some((
                    error.get("code")?.as_str()?.to_owned(),
                    error.get("message")?.as_str()?.to_owned(),
                ))
            });
            match reported {
                Some((code, message)) => {
                    let (class, code) = match exit {
                        3 => ("safety", code),
                        4 => ("device", code),
                        5 => ("runtime", code),
                        6 => ("usage", "not_implemented".to_owned()),
                        _ => ("usage", code),
                    };
                    Err(ToolError::new(class, code, message)
                        .with_detail("lab_exit_code", json!(exit))
                        .with_detail("lab_error", json!(lab_error)))
                }
                None => Err(failed(format!(
                    "exited with {exit} and an error without code and message"
                ))),
            }
        }
        (exit, None) => Err(failed(format!(
            "exited with {exit:?} and printed no JSON envelope"
        ))),
        (exit, Some(ok)) => Err(failed(format!("exited with {exit:?} and ok {ok}"))),
    };
    Answer {
        result,
        req_id,
        returned: true,
    }
}

/// A Lab result that fits the output budget, or the error saying it does not.
fn fitted(data: Value, what: &str, req_id: Option<&str>) -> Result<Value, ToolError> {
    if tools::fits_success(&data) {
        return Ok(data);
    }
    Err(ToolError::usage(
        "output_budget_exceeded",
        format!(
            "actinglab {what} answered, but its result is larger than the 24 KiB tool output budget; what it did stands. Narrow it with fields where the command has them, or run actinglab --json {what} for the whole answer"
        ),
    )
    .with_detail("payload_bytes", json!(data.to_string().len()))
    .with_detail("req_id", json!(req_id)))
}

/// A job's end: an error keeps the warnings found on the way in its details.
fn ended(result: Result<Value, ToolError>, warnings: Vec<Value>) -> JobEnd {
    match result {
        Err(error) if !warnings.is_empty() => (
            Err(error.with_detail("warnings", Value::Array(warnings))),
            Vec::new(),
        ),
        result => (result, warnings),
    }
}

/// A handle no other process can have issued: this server's session identity and a count.
fn new_handle(context: &ToolContext<'_>) -> String {
    let number = NEXT_JOB.fetch_add(1, Ordering::Relaxed);
    format!("{HANDLE_PREFIX}{:016x}_{number}", context.session)
}

/// Runs `body` as a Lab job and answers within the call budget, or with its handle.
fn run_job(
    context: &ToolContext<'_>,
    body: impl FnOnce() -> JobEnd + Send + 'static,
) -> ToolOutcome {
    let job = context.jobs.start_noted(
        new_handle(context),
        "lab",
        None,
        "running",
        Vec::new(),
        body,
    )?;
    operator::job_answer(context, &job, context.deadline)
}

/// The client.action of a Lab request this call made, after its child returned.
fn record_action(
    state_root: &Result<PathBuf, String>,
    tool: &str,
    instance: Option<&str>,
    req_id: Option<&str>,
) -> Vec<Value> {
    let mut warnings = Vec::new();
    if req_id.is_none() {
        warnings.push(
            ToolError::new(
                "runtime",
                "client_action_without_req_id",
                "actinglab's answer carried no req_id, so this call's client.action carries no value",
            )
            .into_warning(),
        );
    }
    let value = req_id.map(|id| ClientActionValue::PathSafeString(id.to_owned()));
    let recorded = runtime::connect_at(state_root, CLI.0, CLI.1)
        .and_then(|connection| operator::record_provenance(connection, tool, instance, CLI, value));
    match recorded {
        Ok(write) => warnings.extend(write.warnings),
        Err(error) => warnings.push(
            ToolError::new(
                "runtime",
                "client_action_not_recorded",
                "actinglab ran and answered; recording this call's client.action in the Runtime failed, and the Lab command was not run again",
            )
            .with_detail("cause", error.into_value())
            .into_warning(),
        ),
    }
    warnings
}

// ---------------------------------------------------------------- author tools

fn author(
    context: &ToolContext<'_>,
    arguments: &Map<String, Value>,
    command: &'static LabCommand,
    tool: &'static str,
) -> ToolOutcome {
    let line = command_line(command, arguments)?;
    let records =
        command.capture_is_lab_request && arguments.get("capture") == Some(&Value::Bool(true));
    let instance = arguments
        .get("instance")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let program = Program::locate(context)?;
    let what = command.words.join(" ");
    run_job(context, move || {
        let answer = program.call(&line, &what);
        let warnings = if records && answer.returned {
            record_action(
                &program.state_root,
                tool,
                instance.as_deref(),
                answer.req_id.as_deref(),
            )
        } else {
            Vec::new()
        };
        let result = answer
            .result
            .and_then(|data| fitted(data, &what, answer.req_id.as_deref()));
        ended(result, warnings)
    })
}

pub(super) fn observe(context: &ToolContext<'_>, arguments: &Map<String, Value>) -> ToolOutcome {
    author(context, arguments, &OBSERVE, "ac_lab_observe")
}

pub(super) fn act(context: &ToolContext<'_>, arguments: &Map<String, Value>) -> ToolOutcome {
    author(context, arguments, &DO, "ac_lab_do")
}

pub(super) fn record_start(
    context: &ToolContext<'_>,
    arguments: &Map<String, Value>,
) -> ToolOutcome {
    author(context, arguments, &RECORD_START, "ac_record_start")
}

pub(super) fn record_mark(
    context: &ToolContext<'_>,
    arguments: &Map<String, Value>,
) -> ToolOutcome {
    author(context, arguments, &RECORD_MARK, "ac_record_mark")
}

pub(super) fn record_stop(
    context: &ToolContext<'_>,
    arguments: &Map<String, Value>,
) -> ToolOutcome {
    author(context, arguments, &RECORD_STOP, "ac_record_stop")
}

pub(super) fn record_status(
    context: &ToolContext<'_>,
    arguments: &Map<String, Value>,
) -> ToolOutcome {
    author(context, arguments, &RECORD_STATUS, "ac_record_status")
}

// ---------------------------------------------------------------- checks

/// ac_pack_check: `package digest`, then `package preflight` with the given reference or,
/// without one, the digest's; both answers verbatim.
pub(super) fn pack_check(context: &ToolContext<'_>, arguments: &Map<String, Value>) -> ToolOutcome {
    let arguments = Arguments::new(arguments, &["package", "package_ref"])?;
    let package = lab_string(&arguments, "package")?
        .ok_or_else(|| invalid_argument("package", "is required"))?;
    let given = lab_string(&arguments, "package_ref")?;
    let program = Program::locate(context)?;
    run_job(context, move || {
        let result = (|| -> Result<Value, ToolError> {
            let mut line = words(&["package", "digest", "--package"]);
            line.push(package.clone());
            let digest = program.call(&line, "package digest").result?;
            let reference = digest
                .get("reference")
                .map(Value::to_string)
                .ok_or_else(|| {
                    ToolError::new(
                        "runtime",
                        "lab_process_failed",
                        "actinglab package digest answered without a package reference",
                    )
                })?;
            // Whether the given reference is the digest's, as the contract parses both.
            let check = given.as_ref().map(|given| {
                let matches = matches!(
                    (PackageRef::parse_argument(given), PackageRef::parse_argument(&reference)),
                    (Ok(given), Ok(digest)) if given == digest
                );
                json!({"given": given, "matches_digest": matches})
            });
            let mut line = words(&["package", "preflight", "--package"]);
            line.extend([package.clone(), "--package-ref".to_owned()]);
            line.push(given.clone().unwrap_or(reference));
            let preflight = program.call(&line, "package preflight").result;
            match preflight {
                Ok(preflight) => {
                    let mut answer = json!({"digest": digest, "preflight": preflight});
                    if let Some(check) = check {
                        answer["package_ref_check"] = check;
                    }
                    fitted(answer, "package digest/preflight", None)
                }
                Err(error) => Err(error
                    .with_detail("digest", digest)
                    .with_detail("package_ref_check", json!(check))),
            }
        })();
        (result, Vec::new())
    })
}

/// ac_catalog_check: `resource catalog`, which only reads the catalog file (#603, C2).
pub(super) fn catalog_check(
    context: &ToolContext<'_>,
    arguments: &Map<String, Value>,
) -> ToolOutcome {
    let arguments = Arguments::new(arguments, &["repo", "catalog", "server", "field"])?;
    let mut line = words(&["resource", "catalog"]);
    for (key, flag) in [
        ("repo", "--repo"),
        ("catalog", "--catalog"),
        ("server", "--catalog-server"),
    ] {
        let value =
            lab_string(&arguments, key)?.ok_or_else(|| invalid_argument(key, "is required"))?;
        line.extend([flag.to_owned(), value]);
    }
    if let Some(field) = lab_string(&arguments, "field")? {
        line.extend(["--field".to_owned(), field]);
    }
    let program = Program::locate(context)?;
    run_job(context, move || {
        let answer = program.call(&line, "resource catalog");
        let result = answer
            .result
            .and_then(|data| fitted(data, "resource catalog", None));
        (result, Vec::new())
    })
}

/// ac_binding_draft: the binding parts of an ac_record_stop answer, verbatim; the package's
/// preflight for its admission scope; the current configuration's check-config report; the
/// manual steps. It merges nothing into any configuration.
pub(super) fn binding_draft(
    context: &ToolContext<'_>,
    arguments: &Map<String, Value>,
) -> ToolOutcome {
    let arguments = Arguments::new(arguments, &["record_stop"])?;
    let lab = arguments
        .value("record_stop")
        .and_then(|stop| stop.get("lab"))
        .and_then(Value::as_object)
        .ok_or_else(|| {
            invalid_argument(
                "record_stop",
                "must be the result of ac_record_stop, with its lab object",
            )
        })?;
    let part = |key: &str| {
        lab.get(key).cloned().ok_or_else(|| {
            invalid_argument(
                "record_stop",
                &format!("has no lab.{key}: pass the result of ac_record_stop"),
            )
        })
    };
    let binding_example = part("binding_example")?;
    let binding_requires = part("binding_requires")?;
    let prerequisite_entry_example = part("prerequisite_entry_example")?;
    let catalog_on_failure_example = part("catalog_on_failure_example")?;
    let package_ref = part("package_ref")?.to_string();
    let package = binding_example
        .get("scheduled_execution")
        .and_then(|execution| execution.get("package_path"))
        .and_then(Value::as_str)
        .ok_or_else(|| {
            invalid_argument(
                "record_stop",
                "has no lab.binding_example.scheduled_execution.package_path",
            )
        })?
        .to_owned();
    not_a_flag("record_stop", &package)?;
    let mut manual_steps = binding_requires.as_array().cloned().ok_or_else(|| {
        invalid_argument(
            "record_stop",
            "has a lab.binding_requires that is not a list",
        )
    })?;
    manual_steps.push(json!(MANUAL_STEP));
    let program = Program::locate(context)?;
    run_job(context, move || {
        let mut warnings = Vec::new();
        let mut line = words(&["package", "preflight", "--package"]);
        line.extend([package.clone(), "--package-ref".to_owned(), package_ref]);
        let preflight = program.call(&line, "package preflight").result;
        let admission = match preflight {
            Ok(preflight) => json!({"package": package, "preflight": preflight}),
            Err(error) => {
                warnings.push(
                    ToolError::new(
                        "runtime",
                        "preflight_failed",
                        "actinglab package preflight did not pass the recorded package; admission shows its error",
                    )
                    .into_warning(),
                );
                json!({"package": package, "preflight_error": error.into_value()})
            }
        };
        let config_report = match check_config(&program) {
            Ok(report) => report,
            Err(error) => {
                warnings.push(
                    ToolError::new(
                        "runtime",
                        "check_config_unavailable",
                        "actingd check-config gave no report; check_config shows why",
                    )
                    .into_warning(),
                );
                json!({"error": error.into_value()})
            }
        };
        let answer = json!({
            "binding_example": binding_example,
            "binding_requires": binding_requires,
            "prerequisite_entry_example": prerequisite_entry_example,
            "catalog_on_failure_example": catalog_on_failure_example,
            "admission": admission,
            "check_config": config_report,
            "manual_steps": manual_steps,
        });
        (fitted(answer, "binding draft", None), warnings)
    })
}

/// `<root>\runtime\actingcommand-actingd.exe check-config --config <root>\actingd.config.json`:
/// the report on the configuration in force, which it only reads (`check_config.rs:37-130`).
fn check_config(program: &Program) -> Result<Value, ToolError> {
    let config = program.root.join("actingd.config.json");
    let mut command = Command::new(
        program
            .root
            .join("runtime")
            .join("actingcommand-actingd.exe"),
    );
    command.arg("check-config").arg("--config").arg(&config);
    let captured = child::run_to_end(command).map_err(|failure| match failure {
        ChildFailure::Spawn(error) | ChildFailure::Io(error) => ToolError::new(
            "runtime",
            "check_config_failed",
            format!("actingd check-config: {error}"),
        ),
        ChildFailure::StillRunning => ToolError::new(
            "runtime",
            "check_config_failed",
            "actingd check-config was still running",
        ),
    })?;
    let report =
        serde_json::from_slice::<Value>(captured.stdout.trim_ascii()).map_err(|error| {
            ToolError::new(
                "runtime",
                "check_config_failed",
                format!("actingd check-config printed no JSON report: {error}"),
            )
            .with_detail("exit_code", json!(captured.status.code()))
            .with_detail("stderr_tail", json!(stderr_tail(&captured.stderr)))
        })?;
    Ok(json!({
        "config": config.display().to_string(),
        "exit_code": captured.status.code(),
        "report": report,
    }))
}

// ---------------------------------------------------------------- result schemas

/// actinglab's answer verbatim, or a job handle.
pub(super) fn lab_result() -> Value {
    json!({
        "type": "object",
        "description": "The actinglab envelope's data, unchanged; or {handle, job_phase} when the call outlives the budget.",
    })
}

pub(super) fn pack_check_input() -> Value {
    json!({
        "type": "object",
        "properties": {
            "package": {
                "type": "string",
                "minLength": 1,
                "pattern": NOT_A_FLAG,
                "description": "The package: a directory, .zip or .json container.",
            },
            "package_ref": {
                "type": "string",
                "minLength": 1,
                "pattern": NOT_A_FLAG,
                "description": "A reference to check against the digest; preflight then uses it.",
            },
        },
        "required": ["package"],
        "additionalProperties": false,
    })
}

pub(super) fn pack_check_result() -> Value {
    json!({
        "type": "object",
        "properties": {
            "digest": {"type": "object"},
            "preflight": {"type": "object"},
            "package_ref_check": {
                "type": "object",
                "properties": {
                    "given": {"type": "string"},
                    "matches_digest": {"type": "boolean"},
                },
            },
            "handle": {"type": "string"},
            "job_phase": {"type": "string"},
        },
    })
}

pub(super) fn catalog_check_input() -> Value {
    let text = |description: &str| json!({"type": "string", "minLength": 1, "pattern": NOT_A_FLAG, "description": description});
    json!({
        "type": "object",
        "properties": {
            "repo": text("--repo: the resource repository."),
            "catalog": text("--catalog: the catalog file, relative to repo."),
            "server": text("--catalog-server: the server whose entries are compiled."),
            "field": text("--field: the policy field; default business_id."),
        },
        "required": ["repo", "catalog", "server"],
        "additionalProperties": false,
    })
}

pub(super) fn binding_draft_input() -> Value {
    json!({
        "type": "object",
        "properties": {
            "record_stop": {
                "type": "object",
                "properties": {"lab": {"type": "object"}},
                "required": ["lab"],
                "description": "The result of ac_record_stop.",
            },
        },
        "required": ["record_stop"],
        "additionalProperties": false,
    })
}

pub(super) fn binding_draft_result() -> Value {
    json!({
        "type": "object",
        "properties": {
            "binding_example": {"type": "object"},
            "binding_requires": {"type": "array"},
            "prerequisite_entry_example": {"type": "object"},
            "catalog_on_failure_example": {"type": "object"},
            "admission": {"type": "object"},
            "check_config": {"type": "object"},
            "manual_steps": {"type": "array"},
            "handle": {"type": "string"},
            "job_phase": {"type": "string"},
        },
    })
}

pub(super) fn observe_input() -> Value {
    input_schema(&OBSERVE)
}

pub(super) fn do_input() -> Value {
    input_schema(&DO)
}

pub(super) fn record_start_input() -> Value {
    input_schema(&RECORD_START)
}

pub(super) fn record_mark_input() -> Value {
    input_schema(&RECORD_MARK)
}

pub(super) fn record_stop_input() -> Value {
    input_schema(&RECORD_STOP)
}

pub(super) fn record_status_input() -> Value {
    input_schema(&RECORD_STATUS)
}
