// SPDX-License-Identifier: AGPL-3.0-only

//! `record draft` (Workflow #434 LAB-5G4c; register §1.287, "Slice LAB-5G4c";
//! `contracts/lab-two-entries.md` §9.3; the coordinator's ruling 3 of 6775192250 on board #434, not
//! the owner's words): the binding draft of a `record stop` answer, as actinglab's own command, on
//! the command line and in `serve --stdio`. A port of today's MCP tool `ac_binding_draft`
//! (`apps/actingctl/src/mcp/lab.rs`: `binding_draft`, `read_export`, `check_config`), copied and
//! not moved: today's server keeps its tool until LAB-7 deletes it.
//!
//! - **The inputs** are the tool's: exactly one of `--record-stop <json>` (a `record stop` answer
//!   with its `lab` parts) and `--record-stop-export <json>` (the export `{path, sha256, size?}` of
//!   one), each read as the operation table's `json` type is (text that is not JSON is
//!   `wrong_type`); a JSON `null` is absent, as the tool reads it.
//! - **The members** are the tool's: the four binding parts verbatim, `binding_requires`,
//!   `admission`, `check_config` and `manual_steps`; the warnings `binding_requires_unavailable`,
//!   `preflight_failed` and `check_config_unavailable` with the tool's values, in `warnings`.
//! - **What follows from being a command.** The answer is the command's line, as every own
//!   command prints it: no envelope, no job, no handle, no overflow rule. `package preflight`
//!   runs in this process through the code its command runs; its failure, `preflight_error`, is
//!   the line that command prints for it. check-config stays a child of the installation's daemon
//!   program, started and pinned as the tool starts it, in the installation actinglab's own
//!   commands locate (`lab_ops::locate`). The refusals of the arguments are the command line's:
//!   `actingctl_usage_invalid` with `argument` and `argument_problem`, the flag's name as the
//!   argument (a flag it does not read, a word or an option without its value as its read list
//!   refuses them).
//! - **Outside an installation** it answers what the tool answers where it locates none:
//!   `lab_tool_unavailable` with `mcp_blocker` `install_root`, the location's failure in its
//!   details; a selection or a configuration that does not read is that failure itself.

use crate::cli_module::own_line;
use crate::cli_result::CliResult;
use crate::flag_args::reads::{FlagReads, ReadPart};
use crate::{CliError, CliOutcome, ErrorKind, FlagArgs, GlobalOptions};
use actingcommand_contract::InstalledProcess;
use actingcommand_contract::outcome::{Detail, IoOp, Outcome};
use actingcommand_lab_ops::codes::{ActingctlCode, ArgumentProblem};
use actingcommand_lab_ops::overview::read_bounded;
use actingcommand_lab_ops::{
    child, cli_line, foreign_data_error, lab_child, locate, material_export,
};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::PathBuf;
use std::process::Command;

/// The command's row, and its name in a refusal.
const COMMAND: &str = "record draft";
/// The word after `record` that names it.
const ACTION: &str = "draft";
/// Its two options, the tool's `record_stop` and `record_stop_export`.
const RECORD_STOP: &str = "--record-stop";
const RECORD_STOP_EXPORT: &str = "--record-stop-export";
/// The flags `record draft` reads; its command line is parsed by it (`FlagArgs::parse_reading`).
pub(crate) const RECORD_DRAFT_READS: FlagReads = FlagReads {
    row: COMMAND,
    parts: &[&ReadPart {
        options: &[RECORD_STOP, RECORD_STOP_EXPORT],
        switches: &[],
        refused: &[],
    }],
    positionals: &[],
};
/// The last manual step, after the binding's requirements: the tool's sentence, unchanged.
const MANUAL_STEP: &str = "edit the configuration, approve in the UI, restart the daemon";
/// The largest exported answer read back: the tool's.
const MAX_EXPORT_READ: u64 = 64 * 1024 * 1024;
/// `package preflight`, run in this process: its subcommand, its flag and its command's name.
const PREFLIGHT: &str = "preflight";
const PACKAGE: &str = "--package";
const PREFLIGHT_COMMAND: &str = "package preflight";
/// The installation's daemon program in its program root, and the command it is given.
const RUNTIME_DIRECTORY: &str = "runtime";
const DAEMON_PROGRAM: &str = "actingcommand-actingd.exe";
const CHECK_CONFIG: &str = "check-config";
const CONFIG: &str = "--config";
/// The class of a check-config failure, as the tool writes it beside its outcome.
const RUNTIME_CLASS: &str = "runtime";
/// What must change where no installation is located: the tool's `mcp_blocker`.
const INSTALL_ROOT: &str = "install_root";

/// actinglab's `record`: `record draft` this command; every other action is `session record`'s,
/// as before. `session record` has no `draft`.
pub(crate) fn run_record(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    match args.split_first() {
        Some((action, rest)) if action == ACTION => run_record_draft(global, rest),
        _ => crate::run_session_record(global, args),
    }
}

/// `record draft`: the binding parts of the `record stop` answer, verbatim; the package's
/// preflight for its admission scope; the check-config report of the configuration in force; the
/// manual steps. It merges nothing into any configuration. A `record stop` answer without its
/// `lab` parts, a package path that begins with `--` or a `binding_requires` that is no list is
/// `unusable`.
fn run_record_draft(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse_reading(args, &RECORD_DRAFT_READS)?;
    let record_stop = match (
        given(&flags, RECORD_STOP)?,
        given(&flags, RECORD_STOP_EXPORT)?,
    ) {
        (Some(record_stop), None) => record_stop,
        (None, Some(export)) => read_export(&export)?,
        _ => return Err(refused(RECORD_STOP, ArgumentProblem::Conflicting)),
    };
    let unusable = || refused(RECORD_STOP, ArgumentProblem::Unusable);
    let lab = record_stop
        .get("lab")
        .and_then(Value::as_object)
        .ok_or_else(unusable)?;
    let part = |key: &str| lab.get(key).cloned().ok_or_else(unusable);
    let binding_example = part("binding_example")?;
    let prerequisite_entry_example = part("prerequisite_entry_example")?;
    let catalog_on_failure_example = part("catalog_on_failure_example")?;
    // `package preflight` would read a package path that begins with `--` as a flag.
    let package = binding_example
        .get("scheduled_execution")
        .and_then(|execution| execution.get("package_path"))
        .and_then(Value::as_str)
        .filter(|package| !package.starts_with("--"))
        .ok_or_else(unusable)?
        .to_owned();
    // Only the record stop that generated the package answers binding_requires.
    let (binding_requires, manual_steps) = match lab.get("binding_requires") {
        Some(Value::Array(items)) => {
            let mut steps = items.clone();
            steps.push(json!(MANUAL_STEP));
            (Value::Array(items.clone()), steps)
        }
        None | Some(Value::Null) => (Value::Null, vec![json!(MANUAL_STEP)]),
        Some(_) => return Err(unusable()),
    };
    let installation = installation()?;
    let mut warnings = Vec::new();
    if binding_requires.is_null() {
        warnings.push(cli_line::warning(&Outcome::new(
            ActingctlCode::BindingRequiresUnavailable,
        )));
    }
    let admission = match preflight(global, &package) {
        Ok(preflight) => json!({"package": package, "preflight": preflight}),
        Err(error) => {
            warnings.push(cli_line::warning(
                &Outcome::new(ActingctlCode::PreflightFailed).path(package.as_str()),
            ));
            json!({"package": package, "preflight_error": failure_line(error)})
        }
    };
    let config_report = match check_config(&installation) {
        Ok(report) => report,
        Err(outcome) => {
            warnings.push(cli_line::warning(
                &Outcome::new(ActingctlCode::CheckConfigUnavailable)
                    .path(installation.config.as_path()),
            ));
            json!({"error": {"class": RUNTIME_CLASS, "outcome": outcome_value(&outcome)}})
        }
    };
    let mut answer = Map::new();
    for (member, value) in [
        ("binding_example", binding_example),
        ("binding_requires", binding_requires),
        ("prerequisite_entry_example", prerequisite_entry_example),
        ("catalog_on_failure_example", catalog_on_failure_example),
        ("admission", admission),
        ("check_config", config_report),
        ("manual_steps", Value::Array(manual_steps)),
    ] {
        answer.insert(member.to_owned(), value);
    }
    if !warnings.is_empty() {
        answer.insert("warnings".to_owned(), Value::Array(warnings));
    }
    Ok(Value::Object(answer))
}

/// The JSON value the option `flag` gives, when the line gives it: its text read as the table's
/// `json` type is (`wrong_type`, caused by the parser's refusal, for text that is not JSON), given
/// once (`repeated` otherwise). A JSON `null` is absent, as the tool reads it.
fn given(flags: &FlagArgs, flag: &str) -> CliOutcome<Option<Value>> {
    let values = flags.values(flag);
    match values.as_slice() {
        [] => Ok(None),
        [text] => serde_json::from_str::<Value>(text)
            .map(|value| (!value.is_null()).then_some(value))
            .map_err(|error| {
                refused_by(flag, ArgumentProblem::WrongType, foreign_data_error(&error))
            }),
        _ => Err(refused(flag, ArgumentProblem::Repeated)),
    }
}

/// A `record stop` answer exported to the export directory ([`export_directory`]): read only from
/// there, only directly inside it, only up to [`MAX_EXPORT_READ`] and only when its sha256
/// matches, as the tool reads it. `--record-stop-export` is `wrong_type` when it is not `{path,
/// sha256, size?}`, and `unusable` (caused by the I/O or parse failure, when there is one) when it
/// names no readable export, its sha256 does not match or the file is not JSON.
fn read_export(export: &Value) -> CliOutcome<Value> {
    let shape = || refused(RECORD_STOP_EXPORT, ArgumentProblem::WrongType);
    let unusable = || refused(RECORD_STOP_EXPORT, ArgumentProblem::Unusable);
    let because = |cause: Outcome| refused_by(RECORD_STOP_EXPORT, ArgumentProblem::Unusable, cause);
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
    let exports = export_directory();
    let directory = fs::canonicalize(&exports).map_err(|error| {
        because(Outcome::from_io_error(&error, IoOp::Canonicalize).path(exports.as_path()))
    })?;
    let file = fs::canonicalize(path)
        .map_err(|error| because(Outcome::from_io_error(&error, IoOp::Canonicalize).path(path)))?;
    if file.parent() != Some(directory.as_path()) {
        return Err(unusable());
    }
    let bytes = read_bounded(&file, MAX_EXPORT_READ).map_err(because)?;
    let expected = if sha256.starts_with("sha256:") {
        sha256.to_owned()
    } else {
        format!("sha256:{sha256}")
    };
    if format!("sha256:{:x}", Sha256::digest(&bytes)) != expected {
        return Err(unusable());
    }
    serde_json::from_slice(&bytes)
        .map_err(|error| because(foreign_data_error(&error).path(file.as_path())))
}

/// Where the tool writes its exports and reads `record_stop_export` from, today's MCP server's
/// export directory (`apps/actingctl/src/mcp/observer.rs`, `export_directory`): the temporary
/// directory's `actingcommand-mcp\materials`.
pub(crate) fn export_directory() -> PathBuf {
    std::env::temp_dir()
        .join("actingcommand-mcp")
        .join("materials")
}

/// `package preflight --package <package>` in this process, through the code its command runs
/// (`run_package`): its answer, or its failure.
fn preflight(global: &GlobalOptions, package: &str) -> CliOutcome<Value> {
    let words = [PACKAGE.to_owned(), package.to_owned()];
    crate::lab_package_control::run_package(PREFLIGHT, global, &words)
}

/// The line `actinglab --json package preflight` prints for its failure `error`: `{outcome,
/// message, details?}` (`own_line`). A line that does not encode is `actingctl_output_failed`
/// with the encoder's text as `message`, as the command line would say on its developer line.
fn failure_line(error: CliError) -> Value {
    let exit = own_line::exit(&CliResult::err(PREFLIGHT_COMMAND.to_owned(), error, true));
    exit.line.unwrap_or_else(|text| {
        let mut line = Map::new();
        line.insert(
            "outcome".to_owned(),
            outcome_value(&cli_line::Exit::output_failed()),
        );
        line.insert("message".to_owned(), Value::String(text));
        Value::Object(line)
    })
}

/// Where check-config runs: the program root (the selected slot, or the root of a layout without
/// slots), the configuration in force and the selection the child is pinned to.
struct Installation {
    program_root: PathBuf,
    config: PathBuf,
    selection: Option<InstalledProcess>,
}

/// The installation actinglab runs from, as its own commands locate it (`lab_ops::locate`), or
/// the refusal [`unlocated`] gives.
fn installation() -> CliOutcome<Installation> {
    let location = located().map_err(|cause| unlocated(&cause))?;
    let program_root = location
        .installation
        .as_ref()
        .map_or_else(|| location.root.clone(), InstalledProcess::program_root);
    Ok(Installation {
        program_root,
        config: location.config,
        selection: location.installation,
    })
}

/// This process's location; in test builds the place a test set
/// (`cli_module::authoring::test_place`), as the authoring option reads it.
fn located() -> Result<locate::Location, Outcome> {
    #[cfg(test)]
    if let Some(program) = crate::cli_module::authoring::test_place::program() {
        return locate::locate_from(&program, None);
    }
    locate::locate()
}

/// What the tool answers where it locates no installation, class usage, the location's failure
/// `cause` in the details as `outcome`: outside any installation (`install_root_unresolved`)
/// `lab_tool_unavailable` with `mcp_blocker` `install_root`; else that failure itself (a
/// selection or a configuration that does not read).
fn unlocated(cause: &Outcome) -> CliError {
    let value = cli_line::outcome_value(cause, Detail::Full);
    if cause.is(ActingctlCode::InstallRootUnresolved) {
        return CliError::new(
            ErrorKind::UsageValidation,
            ActingctlCode::LabToolUnavailable.as_str(),
            format!(
                "{COMMAND} runs check-config of the installation actinglab runs from, and \
                 actinglab runs from none"
            ),
            &[INSTALL_ROOT],
        )
        .with_details(json!({"mcp_blocker": INSTALL_ROOT, "outcome": value}));
    }
    CliError::new(
        ErrorKind::UsageValidation,
        cause.code().as_str(),
        format!("{COMMAND}: the installation actinglab runs from does not read"),
        &[],
    )
    .with_details(json!({"outcome": value}))
}

/// `<program root>\runtime\actingcommand-actingd.exe check-config --config <configuration>` on the
/// configuration in force, which it only reads, pinned to the selection as the tool pins it. Its
/// whole stdout is exported ([`export_hashed`]); inline stay `{config, exit_code, outcome?,
/// report_export}`, `outcome` copied from the report's top level. A failure is
/// `check_config_failed {path, exit_code?}`, caused by the OS failure, or by the parser's
/// rejection of the exported stdout (`foreign_data_error`) and actingd's stderr; or the export's
/// own failure.
fn check_config(installation: &Installation) -> Result<Value, Outcome> {
    let config = &installation.config;
    let mut command = Command::new(
        installation
            .program_root
            .join(RUNTIME_DIRECTORY)
            .join(DAEMON_PROGRAM),
    );
    command.arg(CHECK_CONFIG).arg(CONFIG).arg(config);
    child::pin_child(&mut command, installation.selection.as_ref());
    let failed = || Outcome::new(ActingctlCode::CheckConfigFailed).path(config);
    let captured = child::run_to_end(command).map_err(|failure| match failure.os_failure() {
        Some(cause) => failed().caused_by(cause),
        None => failed(),
    })?;
    let report_export = export_hashed(&captured.stdout)?;
    let exit_code = captured.status.code();
    let report =
        serde_json::from_slice::<Value>(captured.stdout.trim_ascii()).map_err(|error| {
            let parse = foreign_data_error(&error);
            let parse = match report_export.get("path").and_then(Value::as_str) {
                Some(path) => parse.path(path),
                None => parse,
            };
            let outcome = match exit_code {
                Some(exit) => failed().exit_code(exit),
                None => failed(),
            };
            let outcome = outcome.caused_by(parse);
            if captured.stderr.is_empty() {
                outcome
            } else {
                outcome.caused_by(lab_child::process_text(&captured))
            }
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

/// `bytes`, written whole by the export writer as `<sha256>.json` in the export directory:
/// `{path, sha256, size}`, or the writer's `material_export_failed`.
fn export_hashed(bytes: &[u8]) -> Result<Value, Outcome> {
    let sha256 = format!("sha256:{:x}", Sha256::digest(bytes));
    material_export::write(&export_directory(), &sha256, "json", bytes)
        .map_err(|failure| failure.outcome())
}

/// An outcome as this process writes it inside its answer: its envelope in the detail
/// `ACTINGCOMMAND_DETAIL` asks for.
fn outcome_value(outcome: &Outcome) -> Value {
    cli_line::outcome_value(outcome, cli_line::detail().unwrap_or(Detail::Full))
}

/// The command line's refusal of the argument `flag` for `problem`.
fn refused(flag: &str, problem: ArgumentProblem) -> CliError {
    refusal(flag, problem, &cli_line::usage_invalid(Some(flag), problem))
}

/// [`refused`], caused by `cause`.
fn refused_by(flag: &str, problem: ArgumentProblem, cause: Outcome) -> CliError {
    let outcome = cli_line::usage_invalid(Some(flag), problem).caused_by(cause);
    refusal(flag, problem, &outcome)
}

/// `actingctl_usage_invalid` with `argument` and `argument_problem` on the line, the outcome
/// `outcome` whole (its causes too) in the details.
fn refusal(flag: &str, problem: ArgumentProblem, outcome: &Outcome) -> CliError {
    CliError::new(
        ErrorKind::UsageValidation,
        ActingctlCode::ActingctlUsageInvalid.as_str(),
        format!("{COMMAND}: {} {flag}", problem.as_str()),
        &[],
    )
    .with_details(json!({"outcome": cli_line::outcome_value(outcome, Detail::Full)}))
}
