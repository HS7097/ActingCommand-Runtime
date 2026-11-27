// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #503 LIST-A3 (register §1.534; `contracts/resource-readings.md`, "List readings",
//! "Offline"; `contracts/lab-operations.json`, the row `package list-check`): `actinglab package
//! list-check --package <path> --frames <directory> [--step <operation id>]`, the Lab's offline
//! check of a list reading over saved frames.
//!
//! - **What it reads.** The package as `replay` admits it (`replay_cli::admit_with`, the one
//!   recognition set-up: the Runtime's OCR provider, a pack with an OCR target refused
//!   `lab_ocr_unavailable` without one) and the frames as `replay` takes them
//!   (`replay_cli::frame_paths`: every entry of the directory a PNG file, in file-name order; the
//!   step's dispatched frame, then every capture of its scan). It opens no Runtime connection,
//!   touches no device and writes nothing.
//! - **What it checks.** The `read_list` step `--step` names, or, without it, every `read_list`
//!   step of the task in declaration order, each by the kernel's dry run of a list reading
//!   (`dry_run_list_reading`, the run's own scan over the frames, a new reading of them for each
//!   step). Each step answers its declaration, the list whole or its miss, the coverage viewports,
//!   every position the scan read (where it stands, the reads of its name with what the identity
//!   mapping says of each, and what it gives the list) and the summary of the wanted keys; the
//!   distances are the mapping's, this computes none.
//! - **How it ends.** Every list whole: the answer. A list not whole: `package_list_check_not_whole`
//!   with the whole answer in its details. A scan that failed before the list's end: the kernel's
//!   own code (the first such step's, its outcome in `details.outcome`) with the whole answer. A
//!   usage error (an option, the frames' directory, the package, a step that is none of the task's
//!   or no `read_list` step, a task without one) is refused before any frame is read. Exits as the
//!   replay's: 0, else 1 on actinglab's own line and 2 on a `--via` line.
//! - **Printed here, kept nowhere.** A card's text is what an account holds: it is in the answer
//!   this process prints, on stdout, and nowhere else (no ledger record, no log, no file). Without
//!   `--json` the answer prints as text for a person ([`people`]).

use super::codes::ActinglabCode;
use super::{CliError, CliOutcome, ErrorKind, FlagArgs, GlobalOptions};
use actingcommand_contract::outcome::Detail;
use actingcommand_lab::{LabOcr, ListReadingDryRun, PreparedContainedTask, dry_run_list_reading};
use actingcommand_lab_ops::cli_line::Exit;
use serde::Serialize;
use serde_json::{Value, json};
use std::path::Path;

/// The command's words, as its row and its refusals name it.
const COMMAND: &str = "package list-check";
/// The flags it takes, each once: its row's parameters.
pub(crate) const LIST_CHECK_FLAGS: [&str; 3] = ["--package", "--frames", "--step"];
/// The member of the answer that holds each checked step.
const STEPS: &str = "steps";

/// `package list-check`, from its command line: the flags of its list, each once with a value,
/// no positional and none of actinglab's global Runtime or device flags; then [`list_check`] with
/// the process's OCR provider.
pub(super) fn run_list_check(global: &GlobalOptions, flags: &FlagArgs) -> CliOutcome<Value> {
    flags.expect_positionals(COMMAND, 0)?;
    crate::replay_cli::offline_scope_only(global, COMMAND)?;
    flags.reject_flags(COMMAND, &LIST_CHECK_FLAGS)?;
    let package = one_value(flags, "--package")?
        .ok_or_else(|| refused("--package", "required", "requires --package <path>"))?;
    let frames = one_value(flags, "--frames")?
        .ok_or_else(|| refused("--frames", "required", "requires --frames <directory>"))?;
    let step = one_value(flags, "--step")?;
    list_check(
        Path::new(&package),
        Path::new(&frames),
        step.as_deref(),
        crate::offline_ocr::lab_ocr(),
    )
}

/// The check of the package at `package` over the frames of the directory `frames`, for the
/// `read_list` step `step` or every one of the task, with the OCR provider `ocr` (the process's;
/// a test's scripted one). See the module's description for what it answers and how it ends.
pub(crate) fn list_check(
    package: &Path,
    frames: &Path,
    step: Option<&str>,
    ocr: &LabOcr,
) -> CliOutcome<Value> {
    let paths = crate::replay_cli::frame_paths(frames)?;
    let task = crate::replay_cli::admit_with(package, ocr)?;
    let steps = checked_steps(&task, step)?;
    let mut answers = Vec::with_capacity(steps.len());
    let mut failed = None;
    let mut not_whole = Vec::new();
    for operation_id in &steps {
        // A frame is moved into the scan that reads it, so each step reads the frames anew.
        let saved = paths
            .iter()
            .map(|path| crate::package_cli::offline::read_fixture_frame(path.as_path()))
            .collect::<CliOutcome<Vec<_>>>()?;
        match dry_run_list_reading(&task, operation_id, saved) {
            Ok(dry) => {
                if dry.list.is_err() {
                    not_whole.push(operation_id.clone());
                }
                answers.push(step_answer(&dry)?);
            }
            Err(error) => {
                let outcome = encoded(&error.outcome().envelope(Detail::Full))?;
                answers.push(json!({
                    "operation_id": operation_id,
                    "status": "failed",
                    "outcome": outcome,
                }));
                if failed.is_none() {
                    failed = Some((error.code().as_str().to_owned(), error.to_string(), outcome));
                }
            }
        }
    }
    let mut answer = json!({
        "mode": "offline_list_check",
        "executed": false,
        "package_id": task.package_label(),
        "task_id": task.task_label(),
        "frames_given": paths.len(),
        "production_global_ledger_written": false,
        "whole": failed.is_none() && not_whole.is_empty(),
        STEPS: answers,
    });
    if let Some((code, message, outcome)) = failed {
        answer["outcome"] = outcome;
        return Err(CliError::new(
            ErrorKind::UsageValidation,
            code,
            format!(
                "{COMMAND}: the scan of a list reading failed over {} frames: {message}",
                paths.len()
            ),
            &[],
        )
        .with_details(answer));
    }
    if !not_whole.is_empty() {
        return Err(CliError::new(
            ErrorKind::UsageValidation,
            ActinglabCode::PackageListCheckNotWhole.as_str(),
            format!(
                "{COMMAND}: the list of {} is not whole over {} frames",
                not_whole.join(", "),
                paths.len()
            ),
            &[],
        )
        .with_details(answer));
    }
    Ok(answer)
}

/// The steps checked: `step` when it is a `read_list` step of the task, else every one of them in
/// declaration order. A step the task does not declare is `validation_failed` naming `--step`,
/// `not_recognised`; one that is no `read_list` step `unusable`; a task without one, `--package`
/// `unusable`; each before any frame is read.
fn checked_steps(task: &PreparedContainedTask, step: Option<&str>) -> CliOutcome<Vec<String>> {
    let readings = task.list_reading_steps();
    match step {
        Some(step) if readings.contains(&step) => Ok(vec![step.to_owned()]),
        Some(step) if task.declares_operation(step) => Err(refused(
            "--step",
            "unusable",
            &format!("takes a read_list step; the task's operation {step} is none"),
        )),
        Some(step) => Err(refused(
            "--step",
            "not_recognised",
            &format!("takes a read_list step; the task declares no operation {step}"),
        )),
        None if readings.is_empty() => Err(refused(
            "--package",
            "unusable",
            "takes a package whose task declares a read_list step; this one declares none",
        )),
        None => Ok(readings.into_iter().map(str::to_owned).collect()),
    }
}

/// One checked step's answer: its declaration, how it ended (`whole` or `not_whole`), the list
/// or its miss, the positions passed over (`null` for a list without `others`), the summary, the
/// coverage viewports and every position the scan read.
fn step_answer(dry: &ListReadingDryRun) -> CliOutcome<Value> {
    let (status, list, miss) = match &dry.list {
        Ok(rows) => (
            "whole",
            json!({"rows": encoded(&rows.rows)?, "confidence_milli": rows.confidence_milli}),
            Value::Null,
        ),
        Err(miss) => (
            "not_whole",
            Value::Null,
            json!({"reading_problem": miss.problem.as_str(), "count": miss.count}),
        ),
    };
    Ok(json!({
        "operation_id": dry.declaration.operation_id,
        "fact_key": dry.declaration.fact_key,
        "status": status,
        "list": list,
        "miss": miss,
        "passed_over": dry.declaration.passed_over,
        "summary": encoded(&dry.summary)?,
        "viewports": encoded(&dry.viewports)?,
        "positions": encoded(&dry.positions)?,
    }))
}

/// `value` as JSON; a value that does not encode is a failure of this command, never left out.
fn encoded<T: Serialize>(value: &T) -> CliOutcome<Value> {
    serde_json::to_value(value)
        .map_err(|error| CliError::device(format!("failed to encode the list check: {error}")))
}

/// The one value of the flag `name`, `None` when it is not given; given twice, or without a value,
/// it is `validation_failed` naming it.
fn one_value(flags: &FlagArgs, name: &str) -> CliOutcome<Option<String>> {
    match flags.values(name).as_slice() {
        [] => Ok(None),
        [value] if value != "true" && !value.trim().is_empty() => Ok(Some(value.clone())),
        [_] => Err(refused(name, "value_missing", "takes a value")),
        _ => Err(refused(name, "repeated", "takes it once")),
    }
}

/// A refused argument: `validation_failed` (exit 2 on a `--via` line) with the details
/// `{argument, argument_problem, arguments: [{argument, argument_problem}]}` of the Lab's other
/// option refusals; `what` completes the sentence that begins with the command and the argument.
fn refused(argument: &str, problem: &str, what: &str) -> CliError {
    CliError::usage(format!("{COMMAND} {argument}: {what}")).with_details(json!({
        "argument": argument,
        "argument_problem": problem,
        "arguments": [{"argument": argument, "argument_problem": problem}],
    }))
}

/// Whether `invocation` is `package list-check` without `--json` (and with no `--via`, which
/// `main` answers with the envelope): `main` then prints [`people`] of its answer.
pub(crate) fn for_people(invocation: &crate::OwnInvocation) -> bool {
    invocation
        .as_ref()
        .is_ok_and(|own| own.command.as_slice() == ["package", "list-check"] && !own.global.json)
}

/// The text for a person in a terminal of the answer `exit` of `package list-check`: a line for
/// the check (on a failure its code and message first), then for each step a line of how it
/// ended, the wanted keys a card showed and those none showed, and each position with a line of
/// what it gives the list and one for each of its reads (the mark, the text as read in quotes, its
/// confidence, why it names no key, the nearest wanted key and alias, the distance and the limit).
/// `None` for a line that holds no checked step (a usage refusal), which prints as every own
/// command's does.
pub(crate) fn people(exit: &Exit) -> Option<String> {
    let line = exit.line.as_ref().ok()?;
    let answer = line
        .get("details")
        .filter(|details| details.get(STEPS).is_some())
        .unwrap_or(line);
    let steps = answer.get(STEPS)?.as_array()?;
    let mut lines = Vec::new();
    if exit.code != 0 {
        lines.push(format!(
            "{}: {}",
            shown(line.get("outcome").and_then(|outcome| outcome.get("code"))),
            shown(line.get("message"))
        ));
    }
    lines.push(format!(
        "{COMMAND}: package {}, task {}, {} frames: {}",
        shown(answer.get("package_id")),
        shown(answer.get("task_id")),
        shown(answer.get("frames_given")),
        if answer.get("whole") == Some(&Value::Bool(true)) {
            "every list whole"
        } else {
            "not every list whole"
        }
    ));
    for step in steps {
        step_lines(step, &mut lines);
    }
    Some(lines.join("\n"))
}

/// The lines of one checked step.
fn step_lines(step: &Value, lines: &mut Vec<String>) {
    let operation = shown(step.get("operation_id"));
    match step.get("status").and_then(Value::as_str) {
        Some("failed") => {
            lines.push(format!(
                "step {operation}: failed, {}",
                shown(step.get("outcome").and_then(|outcome| outcome.get("code")))
            ));
            return;
        }
        Some("whole") => lines.push(format!(
            "step {operation} ({}): whole, {}",
            shown(step.get("fact_key")),
            counts(step.get("summary"))
        )),
        _ => lines.push(format!(
            "step {operation} ({}): not whole ({}, count {}), {}",
            shown(step.get("fact_key")),
            shown(
                step.get("miss")
                    .and_then(|miss| miss.get("reading_problem"))
            ),
            shown(step.get("miss").and_then(|miss| miss.get("count"))),
            counts(step.get("summary"))
        )),
    }
    let summary = step.get("summary");
    lines.push(format!(
        "  wanted keys a card showed: {}",
        listed(summary.and_then(|summary| summary.get("wanted_shown")))
    ));
    lines.push(format!(
        "  wanted keys no card showed: {}",
        listed(summary.and_then(|summary| summary.get("wanted_not_shown")))
    ));
    for position in items(step.get("positions")) {
        lines.push(format!(
            "  position {} (axis {}, cross {}): {}",
            shown(position.get("position_index")),
            shown(position.get("content_axis_px")),
            shown(position.get("cross_px")),
            given(position.get("answer"))
        ));
        for read in items(position.get("reads")) {
            for sample in items(read.get("samples")) {
                lines.push(format!(
                    "    viewport {}: {}",
                    shown(read.get("viewport")),
                    sample_text(sample)
                ));
            }
        }
    }
}

/// The counts of a step's summary: positions read, passed over and unread.
fn counts(summary: Option<&Value>) -> String {
    let count = |name: &str| shown(summary.and_then(|summary| summary.get(name)));
    format!(
        "{} positions read, {} passed over, {} unread",
        count("positions"),
        count("passed_over"),
        count("unread")
    )
}

/// What a position gives the list: its known key, passed over, or unread with its cause and the
/// identity it kept or why its identity read Unknown.
fn given(answer: Option<&Value>) -> String {
    let member = |name: &str| answer.and_then(|answer| answer.get(name));
    match member("kind").and_then(Value::as_str) {
        Some("known") => format!("known {}", shown(member("key"))),
        Some("passed_over") => "passed over".to_owned(),
        _ => {
            let mut text = format!("unread, {}", shown(member("cause")));
            if let Some(identity) = member("identity") {
                text.push_str(&format!(", keeps {}", shown(Some(identity))));
            }
            if let Some(reason) = member("identity_unknown") {
                text.push_str(&format!(", its identity {}", shown(Some(reason))));
            }
            text
        }
    }
}

/// One read sample: its mark, the text as read, its confidence, why it names no key and its
/// nearest wanted name.
fn sample_text(sample: &Value) -> String {
    let mut text = format!(
        "{} \"{}\", confidence {}",
        shown(sample.get("mark")),
        shown(sample.get("text")),
        shown(sample.get("confidence_milli"))
    );
    if let Some(cause) = sample.get("unread_cause") {
        text.push_str(&format!(", {}", shown(Some(cause))));
    }
    match sample.get("nearest") {
        Some(nearest) => text.push_str(&format!(
            ", nearest {} \"{}\" at {}, limit {}",
            shown(nearest.get("key")),
            shown(nearest.get("alias")),
            shown(nearest.get("distance")),
            shown(nearest.get("limit"))
        )),
        None => text.push_str(", no nearest name"),
    }
    text
}

/// The items of a JSON list; none for anything else.
fn items(value: Option<&Value>) -> &[Value] {
    value
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
}

/// A list of words, joined; `none` for an empty one.
fn listed(value: Option<&Value>) -> String {
    let words = items(value)
        .iter()
        .map(|word| shown(Some(word)))
        .collect::<Vec<_>>();
    if words.is_empty() {
        "none".to_owned()
    } else {
        words.join(", ")
    }
}

/// A scalar as a person reads it: a string without its quotes, `none` for an absent or null one.
fn shown(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(text)) => text.clone(),
        None | Some(Value::Null) => "none".to_owned(),
        Some(other) => other.to_string(),
    }
}
