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
//!
//! One overflow rule for every answer that does not fit the output budget (review F2): the
//! verbatim answer goes to the export directory of ac_material export, and the call answers
//! `{req_id, export, overflowed: true}` (ac_record_stop keeps its binding parts inline), or,
//! for an error, keeps class, code and message with a trimmed `lab_error`.

use super::child::{self, Captured, ChildFailure};
use super::jobs::JobEnd;
use super::observer;
use super::operator::{self, CLI};
use super::runtime;
use super::tools::{self, Arguments, ToolContext, ToolError, ToolOutcome, invalid_argument};
use actingcommand_contract::{ArtifactMaterialAccumulator, ClientActionValue, PackageRef};
use serde_json::{Map, Value, json};
use std::fs;
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
/// The parts of an ac_record_stop answer kept inline when the whole answer is exported.
const BINDING_PARTS: [&str; 5] = [
    "binding_example",
    "binding_requires",
    "prerequisite_entry_example",
    "catalog_on_failure_example",
    "package_ref",
];
/// The largest exported answer ac_binding_draft reads back.
const MAX_EXPORT_READ: u64 = 64 * 1024 * 1024;

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

/// What a command may have changed, which words an answer over the budget (review F2).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Effect {
    /// The device or the recording: performed or attempted; never to be run again.
    Changed,
    /// ac_lab_observe: the answer can be narrowed with fields.
    Narrowable,
    /// Nothing.
    Unchanged,
}

/// One actinglab command and its flags (`lab2_cli.rs:1414-1667`, the package locator flags
/// of `contained_resources.rs:125-216` and the global `--instance` of `cli_parse.rs`).
struct LabCommand {
    words: &'static [&'static str],
    flags: &'static [Flag],
    required: &'static [&'static str],
    /// With `capture: true` the command is a Lab request the Runtime records.
    capture_is_lab_request: bool,
    /// What the command changes; for `do` only with capture.
    effect: Effect,
    /// ac_record_stop: an exported answer keeps its binding parts inline.
    keeps_binding: bool,
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
    "actinglab --instance: the actinglab instance alias, passed through as the CLI takes it (with capture, the Runtime instance alias); without it actinglab uses its default instance.",
);
const RECORD_INSTANCE: Flag = flag(
    "instance",
    "--instance",
    Kind::Text,
    "actinglab --instance: the actinglab instance alias of the recording, passed through as the CLI takes it; without it actinglab resolves it from its configuration.",
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
    effect: Effect::Narrowable,
    keeps_binding: false,
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
    effect: Effect::Changed,
    keeps_binding: false,
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
    effect: Effect::Changed,
    keeps_binding: false,
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
    effect: Effect::Changed,
    keeps_binding: false,
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
    effect: Effect::Changed,
    keeps_binding: true,
};

static RECORD_STATUS: LabCommand = LabCommand {
    words: &["record", "status"],
    flags: &[RECORD_INSTANCE, STATE_DIR],
    required: &[],
    capture_is_lab_request: false,
    effect: Effect::Unchanged,
    keeps_binding: false,
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
    config: PathBuf,
    installation: Option<actingcommand_contract::InstalledProcess>,
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
        location.check()?;
        let (Some(root), Some(install_root)) = (location.root, location.install_root) else {
            return Err(ToolError::usage(
                "lab_tool_unavailable",
                "actingctl does not run from an install root and mcp-serve got no --root, so tools\\actinglab.exe cannot be found",
            )
            .blocked_by("mcp-serve --root <install root>"));
        };
        let actinglab = install_root.join("tools").join("actinglab.exe");
        if !actinglab.is_file() {
            return Err(ToolError::usage(
                "lab_tool_unavailable",
                format!("{} is not a file", actinglab.display()),
            )
            .blocked_by("an install with its tools directory"));
        }
        Ok(Self {
            actinglab,
            config: location
                .config
                .unwrap_or_else(|| root.join("actingd.config.json")),
            installation: location.installation,
            root,
            state_root: location.state_root,
        })
    }

    /// `actinglab --json <arguments>`, talking to this server's Runtime when it is known.
    fn command(&self, arguments: &[String]) -> Command {
        let mut command = Command::new(&self.actinglab);
        super::runtime::pin_child(&mut command, self.installation.as_ref());
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

/// `{path, sha256, size}` of `bytes` written by the export writer as `<sha256>.json`.
fn export_hashed(bytes: &[u8]) -> Result<Value, ToolError> {
    let mut material = ArtifactMaterialAccumulator::default();
    material.update(bytes).map_err(|error| {
        ToolError::new(
            "runtime",
            "material_export_failed",
            format!("cannot hash the answer: {error}"),
        )
    })?;
    observer::export_bytes(material.finish().sha256(), "json", bytes)
}

/// The verbatim JSON `value`, exported.
fn export_json(value: &Value) -> Result<Value, ToolError> {
    let bytes = serde_json::to_vec(value).map_err(|error| {
        ToolError::new(
            "runtime",
            "material_export_failed",
            format!("cannot encode the answer: {error}"),
        )
    })?;
    export_hashed(&bytes)
}

/// The note on an answer over the budget, worded by what the command may have changed.
fn exported_note(effect: Effect, what: &str, export: Option<&Value>) -> Value {
    let place = match export
        .and_then(|export| export.get("path"))
        .and_then(Value::as_str)
    {
        Some(path) => format!("is whole in the exported file {path}"),
        None => "could not be written to a file (see the export error)".to_owned(),
    };
    let message = match effect {
        Effect::Changed => format!(
            "actinglab {what} was performed or attempted as its answer reports; the answer is larger than the output budget and {place}. Do not run it again to see the answer."
        ),
        Effect::Narrowable => format!(
            "the answer of actinglab {what} is larger than the output budget and {place}; narrow it with fields next time."
        ),
        Effect::Unchanged => {
            format!("the answer is larger than the output budget and {place}.")
        }
    };
    ToolError::new("runtime", "answer_exported", message)
        .with_detail("export", json!(export))
        .into_warning()
}

/// `{code, message, details}` of an envelope error, with only the top-level scalar members
/// of its details.
fn trimmed_lab_error(lab_error: &Value) -> Value {
    let details = lab_error
        .get("details")
        .and_then(Value::as_object)
        .map(|details| {
            details
                .iter()
                .filter(|(_, value)| !value.is_array() && !value.is_object())
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect::<Map<_, _>>()
        })
        .unwrap_or_default();
    json!({
        "code": lab_error.get("code"),
        "message": lab_error.get("message"),
        "details": details,
    })
}

/// Whether `outcome` with `warnings` fits the budget as `ac_get_run` returns the finished job,
/// the larger of the two ways the answer is read.
fn fits_as_job(handle: &str, outcome: &Value, warnings: &[Value]) -> bool {
    tools::fits_success(&json!({
        "handle": handle,
        "job": {"kind": "lab", "phase": "failed", "warnings": warnings, "outcome": outcome},
    }))
}

/// How one Lab job reports an answer over the budget.
struct Overflow<'a> {
    handle: &'a str,
    what: &'a str,
    effect: Effect,
    keeps_binding: bool,
}

/// A Lab job's end under the one overflow rule (review F2).
fn finish(
    overflow: &Overflow<'_>,
    result: Result<Value, ToolError>,
    req_id: Option<&str>,
    warnings: Vec<Value>,
) -> JobEnd {
    let handle = overflow.handle;
    match result {
        Ok(data) => {
            if fits_as_job(handle, &data, &warnings) {
                return (Ok(data), warnings);
            }
            let export = match export_json(&data) {
                Ok(export) => export,
                Err(error) => {
                    let mut notes = vec![exported_note(overflow.effect, overflow.what, None)];
                    notes.extend(warnings);
                    let error = error
                        .with_detail("req_id", json!(req_id))
                        .with_detail("warnings", Value::Array(notes));
                    return (Err(error), Vec::new());
                }
            };
            let mut notes = vec![exported_note(overflow.effect, overflow.what, Some(&export))];
            notes.extend(warnings);
            let mut answer = json!({"req_id": req_id, "export": export, "overflowed": true});
            if overflow.keeps_binding
                && let Some(lab) = data.get("lab").and_then(Value::as_object)
            {
                let kept = BINDING_PARTS
                    .iter()
                    .filter_map(|key| lab.get(*key).map(|part| ((*key).to_owned(), part.clone())))
                    .collect::<Map<_, _>>();
                if !kept.is_empty() {
                    answer["lab"] = Value::Object(kept);
                    if !fits_as_job(handle, &answer, &notes)
                        && let Some(object) = answer.as_object_mut()
                    {
                        object.remove("lab");
                    }
                }
            }
            (Ok(answer), notes)
        }
        Err(error) => {
            let mut probe = error.to_value();
            if !warnings.is_empty() {
                probe["details"]["warnings"] = Value::Array(warnings.clone());
            }
            if fits_as_job(handle, &json!({"error": probe}), &[]) {
                return ended(Err(error), warnings);
            }
            let lab_error = error
                .detail("lab_error")
                .filter(|value| !value.is_null())
                .cloned();
            let exported = lab_error.clone().unwrap_or_else(|| error.to_value());
            let (export, export_error) = match export_json(&exported) {
                Ok(export) => (Some(export), None),
                Err(error) => (None, Some(error.into_value())),
            };
            let mut notes = vec![exported_note(
                overflow.effect,
                overflow.what,
                export.as_ref(),
            )];
            notes.extend(warnings);
            let mut error = error
                .with_detail("lab_error_trimmed", json!(true))
                .with_detail("req_id", json!(req_id))
                .with_detail("export", json!(export));
            if let Some(lab_error) = &lab_error {
                error = error.with_detail("lab_error", trimmed_lab_error(lab_error));
            }
            if let Some(export_error) = export_error {
                error = error.with_detail("export_error", export_error);
            }
            (
                Err(error.with_detail("warnings", Value::Array(notes))),
                Vec::new(),
            )
        }
    }
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

/// Runs `body` (given the job's handle) as a Lab job and answers within the call budget, or
/// with its handle.
fn run_job(
    context: &ToolContext<'_>,
    body: impl FnOnce(&str) -> JobEnd + Send + 'static,
) -> ToolOutcome {
    let handle = new_handle(context);
    let own = handle.clone();
    let job = context
        .jobs
        .start_noted(handle, "lab", None, "running", Vec::new(), move || {
            body(&own)
        })?;
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
    let capture = arguments.get("capture") == Some(&Value::Bool(true));
    let records = command.capture_is_lab_request && capture;
    // `do` changes the device only with capture; offline it plans.
    let effect = if command.capture_is_lab_request && !capture && command.effect == Effect::Changed
    {
        Effect::Unchanged
    } else {
        command.effect
    };
    let instance = arguments
        .get("instance")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let program = Program::locate(context)?;
    let what = command.words.join(" ");
    run_job(context, move |handle| {
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
        let overflow = Overflow {
            handle,
            what: &what,
            effect,
            keeps_binding: command.keeps_binding,
        };
        finish(&overflow, answer.result, answer.req_id.as_deref(), warnings)
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

/// The overflow of a check that changes nothing.
fn unchanged<'a>(handle: &'a str, what: &'a str) -> Overflow<'a> {
    Overflow {
        handle,
        what,
        effect: Effect::Unchanged,
        keeps_binding: false,
    }
}

/// ac_pack_check: `package digest`, then `package preflight` with the given reference or,
/// without one, the digest's; both answers verbatim.
pub(super) fn pack_check(context: &ToolContext<'_>, arguments: &Map<String, Value>) -> ToolOutcome {
    let arguments = Arguments::new(arguments, &["package", "package_ref"])?;
    let package = lab_string(&arguments, "package")?
        .ok_or_else(|| invalid_argument("package", "is required"))?;
    let given = lab_string(&arguments, "package_ref")?;
    let program = Program::locate(context)?;
    run_job(context, move |handle| {
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
                    Ok(answer)
                }
                Err(error) => Err(error
                    .with_detail("digest", digest)
                    .with_detail("package_ref_check", json!(check))),
            }
        })();
        finish(
            &unchanged(handle, "package digest / preflight"),
            result,
            None,
            Vec::new(),
        )
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
    run_job(context, move |handle| {
        let answer = program.call(&line, "resource catalog");
        finish(
            &unchanged(handle, "resource catalog"),
            answer.result,
            None,
            Vec::new(),
        )
    })
}

/// An ac_record_stop answer this server exported: read only from the export directory and
/// only when its sha256 matches (review F2).
fn read_export(export: &Value) -> Result<Value, ToolError> {
    const FIELD: &str = "record_stop_export";
    let object = export
        .as_object()
        .ok_or_else(|| invalid_argument(FIELD, "must be {path, sha256}"))?;
    if let Some(unknown) = object
        .keys()
        .find(|key| !matches!(key.as_str(), "path" | "sha256" | "size"))
    {
        return Err(invalid_argument(
            FIELD,
            &format!("has a member {unknown}; it takes path and sha256"),
        ));
    }
    let text = |key: &str| {
        object
            .get(key)
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
            .ok_or_else(|| invalid_argument(FIELD, &format!("needs a {key} string")))
    };
    let path = text("path")?;
    let sha256 = text("sha256")?;
    let outside = || {
        invalid_argument(
            FIELD,
            "must name a file this server exported, in %TEMP%\\actingcommand-mcp\\materials",
        )
    };
    let directory = fs::canonicalize(observer::export_directory()).map_err(|_| outside())?;
    let file = fs::canonicalize(path).map_err(|_| outside())?;
    if file.parent() != Some(directory.as_path()) {
        return Err(outside());
    }
    let bytes = runtime::read_bounded(&file, MAX_EXPORT_READ)
        .map_err(|reason| invalid_argument(FIELD, &format!("cannot be read: {reason}")))?;
    let mut material = ArtifactMaterialAccumulator::default();
    material
        .update(&bytes)
        .map_err(|error| invalid_argument(FIELD, &format!("cannot be hashed: {error}")))?;
    let expected = if sha256.starts_with("sha256:") {
        sha256.to_owned()
    } else {
        format!("sha256:{sha256}")
    };
    if material.finish().sha256() != expected {
        return Err(invalid_argument(
            FIELD,
            "has a sha256 that does not match the file's content",
        ));
    }
    serde_json::from_slice(&bytes)
        .map_err(|_| invalid_argument(FIELD, "names a file that is not a JSON answer"))
}

/// ac_binding_draft: the binding parts of an ac_record_stop answer, verbatim; the package's
/// preflight for its admission scope; the current configuration's check-config report; the
/// manual steps. It merges nothing into any configuration.
pub(super) fn binding_draft(
    context: &ToolContext<'_>,
    arguments: &Map<String, Value>,
) -> ToolOutcome {
    let arguments = Arguments::new(arguments, &["record_stop", "record_stop_export"])?;
    let record_stop = match (
        arguments.value("record_stop"),
        arguments.value("record_stop_export"),
    ) {
        (Some(record_stop), None) => record_stop.clone(),
        (None, Some(export)) => read_export(export)?,
        _ => {
            return Err(invalid_argument(
                "record_stop",
                "or record_stop_export: give exactly one of them",
            ));
        }
    };
    let lab = record_stop
        .get("lab")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            invalid_argument(
                "record_stop",
                "must be the result of ac_record_stop, with its lab object; an answer exported without it is passed as record_stop_export",
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
    // Only the record stop that generated the package answers binding_requires (review F4).
    let (binding_requires, manual_steps) = match lab.get("binding_requires") {
        Some(Value::Array(items)) => {
            let mut steps = items.clone();
            steps.push(json!(MANUAL_STEP));
            (Value::Array(items.clone()), steps)
        }
        None | Some(Value::Null) => (Value::Null, vec![json!(MANUAL_STEP)]),
        Some(_) => {
            return Err(invalid_argument(
                "record_stop",
                "has a lab.binding_requires that is not a list",
            ));
        }
    };
    let program = Program::locate(context)?;
    run_job(context, move |handle| {
        let mut warnings = Vec::new();
        if binding_requires.is_null() {
            warnings.push(
                ToolError::new(
                    "runtime",
                    "binding_requires_unavailable",
                    "actinglab gives binding_requires only in the answer of the record stop that generated the package; this answer has none, so manual_steps holds only the closing step",
                )
                .into_warning(),
            );
        }
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
        finish(
            &unchanged(handle, "binding draft"),
            Ok(answer),
            None,
            warnings,
        )
    })
}

/// `<root>\runtime\actingcommand-actingd.exe check-config --config <root>\actingd.config.json`
/// on the configuration in force, which it only reads (`check_config.rs:37-130`). Its whole
/// stdout is exported; inline stay `{config, exit_code, status?, error?, report_export}` with
/// `status` and `error` copied from the report's top level (review F3).
fn check_config(program: &Program) -> Result<Value, ToolError> {
    let config = &program.config;
    let mut command = Command::new(
        program
            .root
            .join("runtime")
            .join("actingcommand-actingd.exe"),
    );
    command.arg("check-config").arg("--config").arg(config);
    super::runtime::pin_child(&mut command, program.installation.as_ref());
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
    let report_export = export_hashed(&captured.stdout)?;
    let report =
        serde_json::from_slice::<Value>(captured.stdout.trim_ascii()).map_err(|error| {
            ToolError::new(
                "runtime",
                "check_config_failed",
                format!("actingd check-config printed no JSON report: {error}"),
            )
            .with_detail("exit_code", json!(captured.status.code()))
            .with_detail("stderr_tail", json!(stderr_tail(&captured.stderr)))
            .with_detail("report_export", report_export.clone())
        })?;
    let mut inline = json!({
        "config": config.display().to_string(),
        "exit_code": captured.status.code(),
    });
    for key in ["status", "error"] {
        if let Some(value) = report.get(key) {
            inline[key] = value.clone();
        }
    }
    inline["report_export"] = report_export;
    Ok(inline)
}

// ---------------------------------------------------------------- result schemas

/// `{path, sha256, size}` of an exported answer.
fn export_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "path": {"type": "string"},
            "sha256": {"type": "string"},
            "size": {"type": "integer"},
        },
        "required": ["path", "sha256", "size"],
    })
}

/// actinglab's answer verbatim, or exported, or a job handle.
pub(super) fn lab_result() -> Value {
    json!({
        "type": "object",
        "description": "The actinglab envelope's data, unchanged. Larger than the output budget: {req_id, export {path, sha256, size}, overflowed: true} (ac_record_stop also keeps lab.binding_example, lab.binding_requires, lab.prerequisite_entry_example, lab.catalog_on_failure_example and lab.package_ref when they fit). Beyond the call budget: {handle, job_phase}.",
        "properties": {
            "req_id": {"type": ["string", "null"]},
            "export": export_schema(),
            "overflowed": {"type": "boolean"},
            "handle": {"type": "string"},
            "job_phase": {"type": "string"},
        },
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
            "req_id": {"type": ["string", "null"]},
            "export": export_schema(),
            "overflowed": {"type": "boolean"},
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
                "description": "The result of ac_record_stop (also an exported one that kept its lab parts).",
            },
            "record_stop_export": {
                "type": "object",
                "properties": {
                    "path": {"type": "string", "minLength": 1},
                    "sha256": {"type": "string", "minLength": 1},
                    "size": {"type": "integer"},
                },
                "required": ["path", "sha256"],
                "additionalProperties": false,
                "description": "The export of an ac_record_stop answer from this server: read only from %TEMP%\\actingcommand-mcp\\materials and only when its sha256 matches.",
            },
        },
        "oneOf": [
            {"required": ["record_stop"]},
            {"required": ["record_stop_export"]},
        ],
        "additionalProperties": false,
    })
}

pub(super) fn binding_draft_result() -> Value {
    json!({
        "type": "object",
        "properties": {
            "binding_example": {"type": "object"},
            "binding_requires": {"type": ["array", "null"]},
            "prerequisite_entry_example": {"type": "object"},
            "catalog_on_failure_example": {"type": "object"},
            "admission": {"type": "object"},
            "check_config": {
                "type": "object",
                "properties": {
                    "config": {"type": "string"},
                    "exit_code": {"type": ["integer", "null"]},
                    "status": {"type": "string"},
                    "error": {},
                    "report_export": export_schema(),
                },
            },
            "manual_steps": {"type": "array"},
            "req_id": {"type": ["string", "null"]},
            "export": export_schema(),
            "overflowed": {"type": "boolean"},
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
