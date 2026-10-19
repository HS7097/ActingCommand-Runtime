// SPDX-License-Identifier: AGPL-3.0-only

//! Lab subprocess tools (#338 §四 author, S3): every call runs `<root>\tools\actinglab.exe
//! --json …` through the child helper and passes the envelope's `data` on unchanged. A tool
//! error is `{class, outcome}` (Workflow #378 A5, model v2.2 §7.3). An error envelope (exit
//! 2..6 with a code) keeps actinglab's class by its exit code (`cli_result.rs:93-100`: 2 usage,
//! 3 safety, 4 device, 5 runtime, 6 usage) as `lab_tool_failed {exit_code, req_id?}`, caused
//! by actinglab's code (`not_implemented` for exit 6) and, when the error has details, by a
//! `foreign_other_error` from the process holding those details' JSON as `raw_text` (actinglab's
//! data, until its own outcomes carry them); actinglab's message is its developer text and is
//! not carried. Any other exit code, or no envelope, is runtime `lab_process_failed`, caused by
//! a `foreign_other_error` holding the stderr tail.
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
//! `{req_id, export, overflowed: true}` (ac_record_stop keeps its binding parts inline) with
//! the warning `answer_exported`; when the export cannot be written, `export` is absent and
//! its `material_export_failed` is the warning instead. An error is always small and never
//! exported: the warnings found before it become its `note` links.

use super::child::{self, Captured, ChildFailure};
use super::jobs::JobEnd;
use super::observer;
use super::operator::{self, CLI};
use super::runtime;
use super::tools::{
    self, Arguments, McpBlocker, ToolContext, ToolError, ToolOutcome, invalid_argument,
};
use crate::codes::{ActingctlCode, ArgumentProblem};
use actingcommand_contract::ClientActionValue;
use actingcommand_contract::codes::{ContractCode, SharedCode};
use actingcommand_contract::outcome::{CauseRelation, IoOp, Outcome, OutcomeEnvelope, RawSource};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

/// Every Lab job handle starts so; one this process does not hold answers `handle_unknown`.
pub(super) const HANDLE_PREFIX: &str = "lab_job_";
/// actinglab finds the Runtime it talks to here (`state_roots.rs:11-24`).
const STATE_ROOT_ENV: &str = "ACTINGCOMMAND_RUNTIME_STATE_ROOT";
/// Workflow #389 U-2: actinglab's global option naming MCP as the counter that forwarded the
/// work (model section 3.2 rule 7).
pub(super) const LAB_VIA_MCP: [&str; 2] = ["--via", "mcp"];
/// The tail of a failed child's stderr kept as `raw_text`.
const MAX_STDERR_TAIL: usize = 2048;
/// A string argument may not look like a flag: actinglab's flag parser takes a value that
/// starts with `--` for the next flag (`flag_args.rs:27-28`).
const NOT_A_FLAG: &str = "^([^-]|-([^-]|$))";
/// Counts this process's Lab jobs.
static NEXT_JOB: AtomicU64 = AtomicU64::new(1);
const MANUAL_STEP: &str = "edit the configuration, approve in the UI, restart the daemon";
/// The parts of an ac_record_stop answer kept inline when the whole answer is exported
/// (Workflow #336 PKG-S5: record stop answers no package reference).
const BINDING_PARTS: [&str; 4] = [
    "binding_example",
    "binding_requires",
    "prerequisite_entry_example",
    "catalog_on_failure_example",
];
/// The largest exported answer ac_binding_draft reads back.
const MAX_EXPORT_READ: u64 = 64 * 1024 * 1024;
/// Workflow #410 item 1: the parts of an ac_lab_observe or ac_lab_do answer kept inline, in this
/// order and each while the answer still fits, when the whole answer is exported: the frame's
/// coordinate system, the paths of the frame files and the after-action attachment.
const FRAME_PARTS: [&str; 4] = [
    "frame_coordinates",
    "frame_path",
    "preview",
    "after_attachment",
];
/// #411 Lab L1: ac_lab_do's destructive guard arguments, which only offline and dry-run planning
/// read (actinglab `do`).
const DESTRUCTIVE_GUARD: [&str; 2] = ["allow_destructive", "destructive"];

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
/// of `contained_resources.rs:125-216` and the global `--instance` of `cli_parse.rs`).
struct LabCommand {
    words: &'static [&'static str],
    flags: &'static [Flag],
    required: &'static [&'static str],
    /// With `capture: true` the command is a Lab request the Runtime records.
    capture_is_lab_request: bool,
    /// ac_record_stop: an exported answer keeps its binding parts inline.
    keeps_binding: bool,
    /// ac_lab_observe, ac_lab_do (Workflow #410 item 1): an exported answer keeps its frame
    /// parts inline.
    keeps_frame: bool,
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
// Workflow #336 PKG-S4 (option 丙): a package by its path only; no reference or digest flag.
const ZIP: Flag = flag(
    "zip",
    "--zip",
    Kind::Text,
    "--zip: a published .zip package.",
);
// Workflow #411 A8: what --fields does (the Min projection's protected members,
// `crates/ledger/src/lib.rs` `shrink_to_limit`; observe's Min opt-ins, `observation.rs` `project`).
const OBSERVE_FIELDS: Flag = flag(
    "fields",
    "--fields",
    Kind::Text,
    "--fields: answer members, comma separated, that the default (Min) answer, without verbose or pretty, keeps whole when it is reduced to its byte budget: they are never removed or shortened. Of instance, arbitration, matched, standby, frame_age_ms and frame_source, which Min leaves out, the named ones are kept. It selects nothing else.",
);
const DO_FIELDS: Flag = flag(
    "fields",
    "--fields",
    Kind::Text,
    "--fields: answer members, comma separated, that the answer keeps whole when it is reduced to its byte budget: they are never removed or shortened. It selects nothing: an answer within the budget is the same without it.",
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
// Workflow #410 item 1: a JPEG preview of the frame, written as a file and answered by its path.
const OBSERVE_PREVIEW: Flag = flag(
    "preview",
    "--preview",
    Kind::Switch,
    "--preview: also write a JPEG preview of the frame, long edge at most 1024 pixels, into the Lab frame directory; preview answers {path, width, height, scale}, a preview pixel times scale being a frame pixel. Never inlined.",
);
const DO_PREVIEW: Flag = flag(
    "preview",
    "--preview",
    Kind::Switch,
    "--preview: with capture and without dry_run, also write a JPEG preview of the frame after the action when its page is known (an unknown page's preview is always written); refused otherwise. Never inlined.",
);

static OBSERVE: LabCommand = LabCommand {
    words: &["observe"],
    flags: &[
        LAB2_INSTANCE,
        SCENE,
        CAPTURE,
        PACKAGE,
        ZIP,
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
        OBSERVE_FIELDS,
        flag("verbose", "--verbose", Kind::Switch, "--verbose."),
        flag("pretty", "--pretty", Kind::Switch, "--pretty."),
        flag(
            "record",
            "--record",
            Kind::Switch,
            "--record: with capture, store the frame in the active recording.",
        ),
        OBSERVE_PREVIEW,
        TEST_CAPTURE_DELAY,
    ],
    required: &[],
    capture_is_lab_request: true,
    keeps_binding: false,
    keeps_frame: true,
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
            DESTRUCTIVE_GUARD[0],
            "--allow-destructive",
            Kind::Switch,
            "--allow-destructive: offline (scene) or with dry_run only; with capture and without dry_run it is refused.",
        ),
        flag(
            DESTRUCTIVE_GUARD[1],
            "--destructive",
            Kind::Switch,
            "--destructive: offline (scene) or with dry_run only; with capture and without dry_run it is refused.",
        ),
        DO_FIELDS,
        flag(
            "priority",
            "--priority",
            Kind::Text,
            "--priority: normal or high.",
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
        DO_PREVIEW,
        TEST_CAPTURE_DELAY,
    ],
    required: &[],
    capture_is_lab_request: true,
    keeps_binding: false,
    keeps_frame: true,
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
    keeps_binding: false,
    keeps_frame: false,
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
    keeps_binding: false,
    keeps_frame: false,
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
    keeps_binding: true,
    keeps_frame: false,
};

static RECORD_STATUS: LabCommand = LabCommand {
    words: &["record", "status"],
    flags: &[RECORD_INSTANCE, STATE_DIR],
    required: &[],
    capture_is_lab_request: false,
    keeps_binding: false,
    keeps_frame: false,
};

// ---- Workflow #389 MCP-13a (begin): ac_app, actinglab `session app` (MCP-0 command table v1.1
// §4.14), operator before U-3 and option two after it (FCM:116). ----

/// The actions of actinglab `session app` (`session_management.rs`); `force-stop` is the same
/// action as `stop`.
pub(super) const APP_ACTIONS: [&str; 4] = ["launch", "restart", "stop", "force-stop"];

/// `session app <action> --instance <alias>`, never with `--record` (no recording step).
static APP: LabCommand = LabCommand {
    words: &["session", "app"],
    flags: &[
        flag(
            "action",
            "",
            Kind::Positional,
            "launch, restart, stop or force-stop (the same action as stop).",
        ),
        flag(
            "instance",
            "--instance",
            Kind::Text,
            "actinglab --instance: the Runtime instance alias whose application is controlled.",
        ),
    ],
    required: &["action", "instance"],
    capture_is_lab_request: false,
    keeps_binding: false,
    keeps_frame: false,
};
// ---- Workflow #389 MCP-13a (end) ----

/// Workflow #411 A5 (M-PKG S5-T4): ac_pack_check, actinglab `package check --package <path>`,
/// the Runtime's structural admission of one package; it only reads.
static PACK_CHECK: LabCommand = LabCommand {
    words: &["package", "check"],
    flags: &[PACKAGE],
    required: &["package"],
    capture_is_lab_request: false,
    keeps_binding: false,
    keeps_frame: false,
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

/// actinglab would read a value that starts with `--` as a flag: `not_recognised`.
fn not_a_flag(key: &str, text: &str) -> Result<(), ToolError> {
    if text.starts_with("--") {
        return Err(invalid_argument(key, ArgumentProblem::NotRecognised));
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
        return Err(invalid_argument(missing, ArgumentProblem::Required));
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
                    return Err(invalid_argument(flag.property, ArgumentProblem::WrongType));
                }
                valued.extend([flag.flag.to_owned(), value.to_string()]);
            }
            Kind::Switch => match value {
                Value::Bool(true) => switches.push(flag.flag.to_owned()),
                Value::Bool(false) => {}
                _ => return Err(invalid_argument(flag.property, ArgumentProblem::WrongType)),
            },
            Kind::MarkRequest => {
                if !value.is_object() {
                    return Err(invalid_argument(flag.property, ArgumentProblem::WrongType));
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
    state_root: Result<PathBuf, Outcome>,
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
    /// The install's actinglab and the generation selected at this call (Workflow #389 MCP-7
    /// scope extension: `locate_now`, not the selection this server holds from its start).
    fn locate(context: &ToolContext<'_>) -> Result<Self, ToolError> {
        let location = context.runtime.locate_now()?;
        let (Some(root), Some(install_root)) = (location.root, location.install_root) else {
            return Err(ToolError::usage(ActingctlCode::LabToolUnavailable)
                .blocked_by(McpBlocker::InstallRoot));
        };
        let actinglab = install_root.join("tools").join("actinglab.exe");
        if !actinglab.is_file() {
            return Err(ToolError::usage(ActingctlCode::LabToolUnavailable)
                .with(|outcome| outcome.path(actinglab.as_path()))
                .blocked_by(McpBlocker::ToolsDir));
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

    /// `actinglab --json --via mcp <arguments>`, talking to this server's Runtime when it is
    /// known. Workflow #389 U-2 (model section 3.2 rule 7): MCP forwards the work, so the child's
    /// connections bind the Lab channel with `via = mcp`.
    fn command(&self, arguments: &[String]) -> Command {
        let mut command = Command::new(&self.actinglab);
        super::runtime::pin_child(&mut command, self.installation.as_ref());
        command.arg("--json").args(LAB_VIA_MCP).args(arguments);
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

/// An actinglab child that gave no answer: runtime `lab_process_failed`, caused by the OS
/// failure that kept it from starting or from being waited for (none when a thread reading its
/// output panicked), or `lab_process_timeout`. `_what` names the command for the caller's
/// reading; the outcome carries no text.
pub(super) fn child_failure(failure: &ChildFailure, _what: &str) -> ToolError {
    match failure {
        ChildFailure::StillRunning => ToolError::new("runtime", ActingctlCode::LabProcessTimeout),
        ChildFailure::Spawn(_) | ChildFailure::Io(_) => {
            let error = ToolError::new("runtime", ActingctlCode::LabProcessFailed);
            match failure.os_failure() {
                Some(cause) => error.caused_by(cause),
                None => error,
            }
        }
    }
}

/// Owned command-line words.
fn words(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| (*item).to_owned()).collect()
}

/// A child's stderr as foreign text: `foreign_other_error` from the process, with the last
/// `MAX_STDERR_TAIL` bytes as `raw_text` (`raw_text_truncated` when that is not all of it) and
/// the exit code when the child exited.
fn process_text(captured: &Captured) -> Outcome {
    let text = String::from_utf8_lossy(&captured.stderr);
    let mut start = text.len().saturating_sub(MAX_STDERR_TAIL);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    let link = Outcome::new(ContractCode::ForeignOtherError)
        .raw_source(RawSource::Process)
        .raw_text(&text[start..]);
    let link = if start > 0 {
        link.raw_text_truncated(true)
    } else {
        link
    };
    match captured.status.code() {
        Some(exit) => link.exit_code(exit),
        None => link,
    }
}

/// The envelope's data, or the error its exit code names (`cli_result.rs:93-100`).
/// `_what` names the command for the caller's reading; the outcome carries no text.
pub(super) fn interpret(captured: &Captured, _what: &str) -> Answer {
    let envelope = serde_json::from_slice::<Value>(captured.stdout.trim_ascii()).ok();
    let ok = envelope
        .as_ref()
        .and_then(|envelope| envelope.get("ok"))
        .and_then(Value::as_bool);
    let data = envelope.as_ref().and_then(|envelope| envelope.get("data"));
    let lab_error = envelope
        .as_ref()
        .and_then(|envelope| envelope.get("error"))
        .filter(|error| error.is_object());
    let req_id = match ok {
        Some(true) => data.and_then(|data| data.get("req_id")),
        _ => lab_error
            .and_then(|error| error.get("details"))
            .and_then(|details| details.get("req_id")),
    }
    .and_then(Value::as_str)
    .map(str::to_owned);
    // actinglab's code, as it spells it.
    let spelling = lab_error
        .and_then(|error| error.get("code"))
        .and_then(Value::as_str);
    let result = match (captured.status.code(), ok, spelling) {
        (Some(0), Some(true), _) => Ok(data.cloned().unwrap_or(Value::Null)),
        (Some(exit @ 2..=6), Some(false), Some(spelling)) => Err(command_failed(
            exit,
            spelling,
            req_id.as_deref(),
            lab_error.and_then(|error| error.get("details")),
        )),
        _ => Err(process_failed(
            captured,
            req_id.as_deref(),
            lab_error.and_then(|error| error.get("details")),
        )),
    };
    Answer {
        result,
        req_id,
        returned: true,
    }
}

/// An actinglab error envelope: `lab_tool_failed` with actinglab's exit code (and its req_id),
/// caused by actinglab's code (its `spelling`), in the class that exit code names; exit 6 is
/// `not_implemented`. The error's `details`, when it has any, are a second cause (see
/// [`details_text`]).
fn command_failed(
    exit: i32,
    spelling: &str,
    req_id: Option<&str>,
    details: Option<&Value>,
) -> ToolError {
    let (class, cause) = match exit {
        3 => ("safety", Outcome::relayed(spelling)),
        4 => ("device", Outcome::relayed(spelling)),
        5 => ("runtime", Outcome::relayed(spelling)),
        6 => ("usage", Outcome::new(SharedCode::NotImplemented)),
        _ => ("usage", Outcome::relayed(spelling)),
    };
    let outcome = Outcome::new(ActingctlCode::LabToolFailed).exit_code(exit);
    let outcome = match req_id {
        Some(req_id) => outcome.req_id(req_id),
        None => outcome,
    };
    let outcome = outcome.caused_by(cause);
    let outcome = match details_text(details, Some(exit)) {
        Some(details) => outcome.caused_by(details),
        None => outcome,
    };
    ToolError::outcome(class, outcome)
}

/// The details of an actinglab error envelope, actinglab's data about the failure: a
/// `foreign_other_error` from the process with their JSON text as `raw_text` (cut at its bound,
/// the whole text then going to the diagnostic sink) and actinglab's exit code when it exited.
/// `None` when the error has no details or empty ones.
fn details_text(details: Option<&Value>, exit: Option<i32>) -> Option<Outcome> {
    let details = details.filter(|details| match details {
        Value::Null => false,
        Value::Object(members) => !members.is_empty(),
        _ => true,
    })?;
    let link = Outcome::new(ContractCode::ForeignOtherError)
        .raw_source(RawSource::Process)
        .raw_text(details.to_string());
    Some(match exit {
        Some(exit) => link.exit_code(exit),
        None => link,
    })
}

/// actinglab answered no usable envelope (none, an unexpected exit code, or `ok` that does not
/// match it): runtime `lab_process_failed`, caused by its stderr and, when it printed an error
/// with details, by those details (see [`details_text`]).
fn process_failed(captured: &Captured, req_id: Option<&str>, details: Option<&Value>) -> ToolError {
    let outcome = Outcome::new(ActingctlCode::LabProcessFailed);
    let outcome = match captured.status.code() {
        Some(exit) => outcome.exit_code(exit),
        None => outcome,
    };
    let outcome = match req_id {
        Some(req_id) => outcome.req_id(req_id),
        None => outcome,
    };
    let outcome = outcome.caused_by(process_text(captured));
    let outcome = match details_text(details, captured.status.code()) {
        Some(details) => outcome.caused_by(details),
        None => outcome,
    };
    ToolError::outcome("runtime", outcome)
}

/// `bytes`, written whole by the export writer as `<sha256>.json`: `{path, sha256, size}`.
fn export_hashed(bytes: &[u8]) -> Result<Value, ToolError> {
    let sha256 = format!("sha256:{:x}", Sha256::digest(bytes));
    observer::export_bytes(&sha256, "json", bytes)
}

/// The warning on an answer written whole to `export` (`{path, sha256, size}`):
/// `answer_exported {path, actual_sha256, actual_bytes}`.
fn answer_exported(export: &Value) -> Outcome {
    let mut warning = Outcome::new(ActingctlCode::AnswerExported);
    if let Some(path) = export.get("path").and_then(Value::as_str) {
        warning = warning.path(path);
    }
    if let Some(sha256) = export.get("sha256").and_then(Value::as_str) {
        warning = warning.actual_sha256(sha256);
    }
    if let Some(size) = export.get("size").and_then(Value::as_u64) {
        warning = warning.actual_bytes(size);
    }
    warning
}

/// Whether `answer` with `warnings` fits the budget as `ac_get_run` returns the finished job,
/// the larger of the two ways the answer is read.
fn fits_as_job(handle: &str, answer: &Value, warnings: &[Value]) -> bool {
    tools::fits_success(&json!({
        "handle": handle,
        "job": {"kind": "lab", "phase": "failed", "warnings": warnings, "job_result": answer},
    }))
}

/// How one Lab job reports an answer over the budget.
struct Overflow<'a> {
    handle: &'a str,
    /// ac_record_stop: an exported answer keeps its binding parts inline.
    keeps_binding: bool,
}

/// The overflow of an answer without binding parts.
const fn plain(handle: &str) -> Overflow<'_> {
    Overflow {
        handle,
        keeps_binding: false,
    }
}

/// A Lab job's end under the one overflow rule (review F2). An answer over the budget is
/// exported whole and answered `{req_id, export, overflowed: true}` with the warning
/// `answer_exported` first; when the export fails, `export` is absent and the export's
/// `material_export_failed` is that warning. An error ends the job as it is.
fn finish(
    overflow: &Overflow<'_>,
    result: Result<Value, ToolError>,
    req_id: Option<&str>,
    mut warnings: Vec<Value>,
) -> JobEnd {
    let handle = overflow.handle;
    let data = match result {
        Ok(data) => data,
        Err(error) => return ended(error, warnings),
    };
    if fits_as_job(handle, &data, &warnings) {
        return (Ok(data), warnings);
    }
    let mut answer = json!({"req_id": req_id, "overflowed": true});
    let note = match export_hashed(data.to_string().as_bytes()) {
        Ok(export) => {
            let note = tools::warning(&answer_exported(&export));
            answer["export"] = export;
            note
        }
        Err(error) => error.into_warning(),
    };
    warnings.insert(0, note);
    if overflow.keeps_binding
        && let Some(lab) = data.get("lab").and_then(Value::as_object)
    {
        let kept = BINDING_PARTS
            .iter()
            .filter_map(|key| lab.get(*key).map(|part| ((*key).to_owned(), part.clone())))
            .collect::<Map<_, _>>();
        if !kept.is_empty() {
            answer["lab"] = Value::Object(kept);
            if !fits_as_job(handle, &answer, &warnings)
                && let Some(object) = answer.as_object_mut()
            {
                object.remove("lab");
            }
        }
    }
    (Ok(answer), warnings)
}

/// A job that ends in `error`: the warnings found on the way become its `note` links. A warning
/// that is not an `{outcome}` stays a warning of the job.
fn ended(mut error: ToolError, warnings: Vec<Value>) -> JobEnd {
    let mut kept = Vec::new();
    for warning in warnings {
        match warning_outcome(&warning) {
            Some(note) => error = error.with(|outcome| outcome.link(CauseRelation::Note, note)),
            None => kept.push(warning),
        }
    }
    (Err(error), kept)
}

/// The outcome a warning `{outcome}` reports.
fn warning_outcome(warning: &Value) -> Option<Outcome> {
    let envelope = warning.get("outcome")?.clone();
    serde_json::from_value::<OutcomeEnvelope>(envelope)
        .ok()
        .map(OutcomeEnvelope::into_outcome)
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

/// The client.action of a Lab request this call made, after its child returned. Its warnings:
/// `client_action_without_req_id`, the recording's own, or `client_action_not_recorded` caused
/// by the recording's failure (the Lab command is not run again).
fn record_action(
    state_root: &Result<PathBuf, Outcome>,
    tool: &str,
    instance: Option<&str>,
    req_id: Option<&str>,
) -> Vec<Value> {
    let mut warnings = Vec::new();
    if req_id.is_none() {
        warnings.push(tools::warning(&Outcome::new(
            ActingctlCode::ClientActionWithoutReqId,
        )));
    }
    let value = req_id.map(|id| ClientActionValue::PathSafeString(id.to_owned()));
    let recorded = runtime::connect_at(state_root, CLI.0, CLI.1)
        .and_then(|connection| operator::record_provenance(connection, tool, instance, CLI, value));
    match recorded {
        Ok(write) => warnings.extend(write.warnings),
        Err(error) => warnings.push(tools::warning(
            &Outcome::new(ActingctlCode::ClientActionNotRecorded).caused_by(error.into_outcome()),
        )),
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
        command_end(
            command,
            handle,
            answer.result,
            answer.req_id.as_deref(),
            warnings,
        )
    })
}

/// A Lab command's job end: the one overflow rule (review F2), with the parts its exported
/// answer keeps inline (ac_record_stop's binding parts; Workflow #410 item 1: ac_lab_observe's
/// and ac_lab_do's frame parts).
fn command_end(
    command: &LabCommand,
    handle: &str,
    result: Result<Value, ToolError>,
    req_id: Option<&str>,
    warnings: Vec<Value>,
) -> JobEnd {
    let overflow = Overflow {
        handle,
        keeps_binding: command.keeps_binding,
    };
    let parts = if command.keeps_frame {
        frame_parts(&result)
    } else {
        Vec::new()
    };
    keep_frame_parts(handle, finish(&overflow, result, req_id, warnings), parts)
}

/// Workflow #410 item 1: the frame parts of an ac_lab_observe or ac_lab_do answer, in
/// `FRAME_PARTS` order, taken before the answer may be exported.
fn frame_parts(result: &Result<Value, ToolError>) -> Vec<(&'static str, Value)> {
    let Ok(Value::Object(data)) = result else {
        return Vec::new();
    };
    FRAME_PARTS
        .iter()
        .filter_map(|key| data.get(*key).map(|part| (*key, part.clone())))
        .collect()
}

/// Workflow #410 item 1: an exported ac_lab_observe or ac_lab_do answer still states its frame's
/// coordinate system and its frame files inline: each frame part joins the answer
/// `{req_id, export, overflowed: true}` in `FRAME_PARTS` order while the answer still fits.
fn keep_frame_parts(handle: &str, end: JobEnd, parts: Vec<(&'static str, Value)>) -> JobEnd {
    let (answer, warnings) = end;
    let mut answer = match answer {
        Ok(answer) if answer.get("overflowed") == Some(&Value::Bool(true)) => answer,
        other => return (other, warnings),
    };
    for (key, part) in parts {
        let Some(object) = answer.as_object_mut() else {
            break;
        };
        object.insert(key.to_owned(), part);
        if !fits_as_job(handle, &answer, &warnings)
            && let Some(object) = answer.as_object_mut()
        {
            object.remove(key);
        }
    }
    (Ok(answer), warnings)
}

/// Workflow #411 A8: whether actinglab would open the package (`PackageInput::open`) with no
/// locator. observe opens it on its Runtime-backed branch, with `--capture` (`run_observe`), and
/// do on its own, with `--capture` and without `--dry-run` (`run_do`): every online Lab request
/// carries its package (contained-lab-operation.md). ac_lab_observe takes no dry_run.
fn opens_package_without_locator(arguments: &Map<String, Value>) -> bool {
    let set = |key: &str| arguments.get(key) == Some(&Value::Bool(true));
    set("capture") && !set("dry_run") && !given(arguments, "package") && !given(arguments, "zip")
}

/// Whether an argument is given (`null` is not), as `Arguments::has` reads it.
fn given(arguments: &Map<String, Value>, key: &str) -> bool {
    arguments.get(key).is_some_and(|value| !value.is_null())
}

/// Workflow #411 A8: such a call is refused here, before the install is located and before any
/// child runs: `arguments_invalid {argument: package, argument_problem: required}`.
fn require_locator(arguments: &Map<String, Value>) -> Result<(), ToolError> {
    if opens_package_without_locator(arguments) {
        return Err(invalid_argument("package", ArgumentProblem::Required));
    }
    Ok(())
}

pub(super) fn observe(context: &ToolContext<'_>, arguments: &Map<String, Value>) -> ToolOutcome {
    command_line(&OBSERVE, arguments)?;
    require_locator(arguments)?;
    author(context, arguments, &OBSERVE, "ac_lab_observe")
}

/// #411 Lab L1: online `do --capture` (capture, without dry_run) does not read the destructive
/// guard; a call setting either is refused here, before the install is located and before any
/// child runs: `arguments_invalid {argument, argument_problem: conflicting}` naming the first set.
fn refuse_destructive_guard_on_capture(arguments: &Map<String, Value>) -> Result<(), ToolError> {
    let set = |key: &str| arguments.get(key) == Some(&Value::Bool(true));
    if !set("capture") || !set("dry_run") {
        return Ok(());
    }
    match DESTRUCTIVE_GUARD.into_iter().find(|&key| set(key)) {
        Some(key) => Err(invalid_argument(key, ArgumentProblem::Conflicting)),
        None => Ok(()),
    }
}

/// ac_lab_do. Workflow #411 A14: target `tap` with tap names the tap twice (target is a Runtime
/// element id): `arguments_invalid {argument: tap, argument_problem: conflicting}`, before the
/// install is located and before any child runs; #411 Lab L1: so is a destructive guard argument
/// with capture and without dry_run.
pub(super) fn act(context: &ToolContext<'_>, arguments: &Map<String, Value>) -> ToolOutcome {
    command_line(&DO, arguments)?;
    if arguments.get("target").and_then(Value::as_str) == Some("tap") && given(arguments, "tap") {
        return Err(invalid_argument("tap", ArgumentProblem::Conflicting));
    }
    refuse_destructive_guard_on_capture(arguments)?;
    require_locator(arguments)?;
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

/// ac_app (Workflow #389 MCP-13a): actinglab `session app <action> --instance <alias>`, its data
/// verbatim. An action that is none of `APP_ACTIONS` is refused before actinglab runs.
pub(super) fn app(context: &ToolContext<'_>, arguments: &Map<String, Value>) -> ToolOutcome {
    let line = command_line(&APP, arguments)?;
    let action = line.get(APP.words.len()).map(String::as_str);
    if !action.is_some_and(|action| APP_ACTIONS.contains(&action)) {
        return Err(invalid_argument("action", ArgumentProblem::NotRecognised));
    }
    author(context, arguments, &APP, "ac_app")
}

// ---------------------------------------------------------------- checks

/// ac_pack_check: actinglab `package check --package <package>`, its data verbatim
/// (`{status, package_id, entry_task_id}`). Workflow #411 A5 (M-PKG S5-T4): the Runtime's
/// structural admission, no longer the interim `package preflight` of PKG-S4; no digest is
/// computed and no reference is taken.
pub(super) fn pack_check(context: &ToolContext<'_>, arguments: &Map<String, Value>) -> ToolOutcome {
    author(context, arguments, &PACK_CHECK, "ac_pack_check")
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
        let value = lab_string(&arguments, key)?
            .ok_or_else(|| invalid_argument(key, ArgumentProblem::Required))?;
        line.extend([flag.to_owned(), value]);
    }
    if let Some(field) = lab_string(&arguments, "field")? {
        line.extend(["--field".to_owned(), field]);
    }
    let program = Program::locate(context)?;
    run_job(context, move |handle| {
        let answer = program.call(&line, "resource catalog");
        finish(&plain(handle), answer.result, None, Vec::new())
    })
}

/// An ac_record_stop answer this server exported: read only from the export directory and
/// only when its sha256 matches (review F2). The argument is `wrong_type` when it is not
/// `{path, sha256, size?}`, and `unusable` (caused by the I/O or parse failure, when there is
/// one) when it names no readable export of this server, its sha256 does not match or the file
/// is not JSON.
fn read_export(export: &Value) -> Result<Value, ToolError> {
    const FIELD: &str = "record_stop_export";
    let shape = || invalid_argument(FIELD, ArgumentProblem::WrongType);
    let unusable = || invalid_argument(FIELD, ArgumentProblem::Unusable);
    let object = export.as_object().ok_or_else(shape)?;
    if object
        .keys()
        .any(|key| !matches!(key.as_str(), "path" | "sha256" | "size"))
    {
        return Err(shape());
    }
    let text = |key: &str| {
        object
            .get(key)
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
            .ok_or_else(shape)
    };
    let path = text("path")?;
    let sha256 = text("sha256")?;
    let exports = observer::export_directory();
    let directory = fs::canonicalize(&exports).map_err(|error| {
        unusable()
            .caused_by(Outcome::from_io_error(&error, IoOp::Canonicalize).path(exports.as_path()))
    })?;
    let file = fs::canonicalize(path).map_err(|error| {
        unusable().caused_by(Outcome::from_io_error(&error, IoOp::Canonicalize).path(path))
    })?;
    if file.parent() != Some(directory.as_path()) {
        return Err(unusable());
    }
    let bytes = runtime::read_bounded(&file, MAX_EXPORT_READ)
        .map_err(|cause| unusable().caused_by(cause))?;
    let expected = if sha256.starts_with("sha256:") {
        sha256.to_owned()
    } else {
        format!("sha256:{sha256}")
    };
    if format!("sha256:{:x}", Sha256::digest(&bytes)) != expected {
        return Err(unusable());
    }
    serde_json::from_slice(&bytes).map_err(|error| {
        unusable().caused_by(
            Outcome::new(ContractCode::ForeignDataError)
                .raw_source(RawSource::Json)
                .raw_text(error.to_string())
                .path(file.as_path()),
        )
    })
}

/// ac_binding_draft: the binding parts of an ac_record_stop answer, verbatim; the package's
/// preflight for its admission scope; the current configuration's check-config report; the
/// manual steps. It merges nothing into any configuration. A `record_stop` that is not an
/// ac_record_stop answer with its lab parts is `unusable`.
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
                ArgumentProblem::Conflicting,
            ));
        }
    };
    let unusable = || invalid_argument("record_stop", ArgumentProblem::Unusable);
    let lab = record_stop
        .get("lab")
        .and_then(Value::as_object)
        .ok_or_else(unusable)?;
    let part = |key: &str| lab.get(key).cloned().ok_or_else(unusable);
    let binding_example = part("binding_example")?;
    let prerequisite_entry_example = part("prerequisite_entry_example")?;
    let catalog_on_failure_example = part("catalog_on_failure_example")?;
    // actinglab would read a package path that starts with `--` as a flag.
    let package = binding_example
        .get("scheduled_execution")
        .and_then(|execution| execution.get("package_path"))
        .and_then(Value::as_str)
        .filter(|package| !package.starts_with("--"))
        .ok_or_else(unusable)?
        .to_owned();
    // Only the record stop that generated the package answers binding_requires (review F4).
    let (binding_requires, manual_steps) = match lab.get("binding_requires") {
        Some(Value::Array(items)) => {
            let mut steps = items.clone();
            steps.push(json!(MANUAL_STEP));
            (Value::Array(items.clone()), steps)
        }
        None | Some(Value::Null) => (Value::Null, vec![json!(MANUAL_STEP)]),
        Some(_) => return Err(unusable()),
    };
    let program = Program::locate(context)?;
    run_job(context, move |handle| {
        let mut warnings = Vec::new();
        if binding_requires.is_null() {
            warnings.push(tools::warning(&Outcome::new(
                ActingctlCode::BindingRequiresUnavailable,
            )));
        }
        // Workflow #336 PKG-S4: preflight takes the package by its path only.
        let mut line = words(&["package", "preflight", "--package"]);
        line.push(package.clone());
        let preflight = program.call(&line, "package preflight").result;
        let admission = match preflight {
            Ok(preflight) => json!({"package": package, "preflight": preflight}),
            Err(error) => {
                warnings.push(tools::warning(
                    &Outcome::new(ActingctlCode::PreflightFailed).path(package.as_str()),
                ));
                json!({"package": package, "preflight_error": error.into_value()})
            }
        };
        let config_report = match check_config(&program) {
            Ok(report) => report,
            Err(error) => {
                warnings.push(tools::warning(
                    &Outcome::new(ActingctlCode::CheckConfigUnavailable)
                        .path(program.config.as_path()),
                ));
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
        finish(&plain(handle), Ok(answer), None, warnings)
    })
}

/// `<root>\runtime\actingcommand-actingd.exe check-config --config <root>\actingd.config.json`
/// on the configuration in force, which it only reads (`check_config.rs:37-130`). Its whole
/// stdout is exported; inline stay `{config, exit_code, outcome?, report_export}` with `outcome`
/// copied from the report's top level (review F3; Workflow #378 A6a: check-config v2's outcome,
/// which replaced v1's `status` and `error`). A failure is
/// `check_config_failed {path, exit_code?}`, caused by the OS failure, or by the parser's
/// rejection of the exported stdout (`foreign_data_error`) and actingd's stderr.
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
    let failed = || Outcome::new(ActingctlCode::CheckConfigFailed).path(config);
    let captured = child::run_to_end(command).map_err(|failure| {
        let outcome = match failure.os_failure() {
            Some(cause) => failed().caused_by(cause),
            None => failed(),
        };
        ToolError::outcome("runtime", outcome)
    })?;
    let report_export = export_hashed(&captured.stdout)?;
    let exit_code = captured.status.code();
    let report =
        serde_json::from_slice::<Value>(captured.stdout.trim_ascii()).map_err(|error| {
            let parse = Outcome::new(ContractCode::ForeignDataError)
                .raw_source(RawSource::Json)
                .raw_text(error.to_string());
            let parse = match report_export.get("path").and_then(Value::as_str) {
                Some(path) => parse.path(path),
                None => parse,
            };
            let outcome = match exit_code {
                Some(exit) => failed().exit_code(exit),
                None => failed(),
            };
            let outcome = outcome.caused_by(parse);
            let outcome = if captured.stderr.is_empty() {
                outcome
            } else {
                outcome.caused_by(process_text(&captured))
            };
            ToolError::outcome("runtime", outcome)
        })?;
    // `exit_code` is a data member here; the members are inserted one by one, outside `json!`.
    let mut inline = Map::new();
    inline.insert(
        "config".to_owned(),
        Value::String(config.display().to_string()),
    );
    inline.insert("exit_code".to_owned(), json!(exit_code));
    if let Some(outcome) = report.get("outcome") {
        inline.insert("outcome".to_owned(), outcome.clone());
    }
    inline.insert("report_export".to_owned(), report_export);
    Ok(Value::Object(inline))
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
        "description": "The actinglab envelope's data, unchanged. Larger than the output budget: {req_id, export {path, sha256, size}, overflowed: true} with the warning answer_exported (without export, and with the warning material_export_failed, when the export could not be written; ac_record_stop also keeps lab.binding_example, lab.binding_requires, lab.prerequisite_entry_example and lab.catalog_on_failure_example when they fit). Beyond the call budget: {handle, job_phase}.",
        "properties": {
            "req_id": {"type": ["string", "null"]},
            "export": export_schema(),
            "overflowed": {"type": "boolean"},
            "handle": {"type": "string"},
            "job_phase": {"type": "string"},
        },
    })
}

/// ac_lab_observe's and ac_lab_do's result (Workflow #410 item 1): [`lab_result`], with the
/// frame parts an exported answer keeps inline.
pub(super) fn frame_result() -> Value {
    let mut schema = lab_result();
    schema["description"] = json!(
        "The actinglab envelope's data, unchanged: with frame_coordinates {device_pixels [width, height], orientation (landscape, portrait or square), applies_to [tap, tap_rect, swipe]}, the coordinate system of the frame (pixels counted from its top-left pixel) (ac_lab_do: of the frame the action was chosen on); preview {path, width, height, scale} when preview was asked; ac_lab_do after an action also after_attachment: {kind page_projection, observation} when the page after it is known, else {kind frame, frame_coordinates, frame_path, preview}. A part that could not be written is {failure {code lab_frame_export_failed, path, raw_text}} in its place. No image is inlined. Larger than the output budget: {req_id, export {path, sha256, size}, overflowed: true} with the warning answer_exported (without export, and with the warning material_export_failed, when the export could not be written), keeping frame_coordinates, frame_path, preview and after_attachment inline, each while it fits. Beyond the call budget: {handle, job_phase}."
    );
    if let Some(properties) = schema["properties"].as_object_mut() {
        properties.insert("frame_coordinates".to_owned(), json!({"type": "object"}));
        properties.insert(
            "frame_path".to_owned(),
            json!({"type": ["string", "object"]}),
        );
        properties.insert("preview".to_owned(), json!({"type": "object"}));
        properties.insert("after_attachment".to_owned(), json!({"type": "object"}));
    }
    schema
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
        },
        "required": ["package"],
        "additionalProperties": false,
    })
}

/// Workflow #411 A5: `package check`'s data, verbatim, or the one overflow rule's answer, or a
/// job handle.
pub(super) fn pack_check_result() -> Value {
    json!({
        "type": "object",
        "description": "actinglab package check's data, unchanged: {status: admitted, package_id, entry_task_id} of a package the Runtime's structural admission admitted. Beyond the call budget: {handle, job_phase}.",
        "properties": {
            "status": {"type": "string", "const": "admitted"},
            "package_id": {"type": "string"},
            "entry_task_id": {"type": "string"},
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
    // check_config's `exit_code` is a data member; its schema is inserted outside `json!`.
    let mut check_config = Map::new();
    check_config.insert("config".to_owned(), json!({"type": "string"}));
    check_config.insert("exit_code".to_owned(), json!({"type": ["integer", "null"]}));
    check_config.insert("outcome".to_owned(), json!({"type": "object"}));
    check_config.insert("report_export".to_owned(), export_schema());
    json!({
        "type": "object",
        "properties": {
            "binding_example": {"type": "object"},
            "binding_requires": {"type": ["array", "null"]},
            "prerequisite_entry_example": {"type": "object"},
            "catalog_on_failure_example": {"type": "object"},
            "admission": {"type": "object"},
            "check_config": {"type": "object", "properties": check_config},
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

/// ac_app's input: `action` names one of `APP_ACTIONS`.
pub(super) fn app_input() -> Value {
    let mut schema = input_schema(&APP);
    schema["properties"]["action"]["enum"] = json!(APP_ACTIONS);
    schema
}

// Test-only: Workflow #382 MCP-T1 (test plan A-5a), the Lab overflow rule gate; Workflow #378
// A5, how an actinglab exit becomes an answer.
#[cfg(test)]
mod tests {
    // Workflow #411 A5: ac_pack_check runs actinglab `package check` and answers its data.
    mod gate_0121_pkgcheck;
    // Workflow #389 U-2: option tools start actinglab with `--via mcp`.
    mod gate_389_u2;
    mod gate_mcpt1;
    // Workflow #389 MCP-13a: ac_app runs actinglab `session app` with the same arguments.
    mod gate_mcp13a;
    // Workflow #396 REC-4: ac_record_mark hands a click.follow request to actinglab unchanged.
    mod gate_rec4;
    // Workflow #410 item 1: ac_lab_observe and ac_lab_do pass preview to actinglab, and an
    // exported answer still states its frame inline.
    mod gate_labobs;
    // Workflow #411 A8, A14 (apps key 73): ac_lab_do and ac_lab_observe refuse a capture without
    // a package, and ac_lab_do target tap with tap, before any child runs.
    mod gate_rc2_a8;
    // #411 Lab L1 (apps key 76): ac_lab_do and ac_lab_observe take no argument actinglab does not
    // read, and ac_lab_do refuses the destructive guard with capture, before any child runs.
    mod gate_lab_l1;
}
