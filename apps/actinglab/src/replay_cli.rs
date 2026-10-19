// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #308 SEL-L6a (FCM T-01, T-03): `actinglab replay --package <path> --frames <dir>
//! [--facts <file>]`. The whole package is replayed over a saved frame sequence with the
//! production interpreter (`replay_contained_task`): the entry check, every step with its
//! operation, its destination pages, its guarded inputs and the page it reached, every page
//! recognition, and how the run ended. OCR targets are evaluated with the Runtime's OCR provider
//! (Workflow #396 LAB-3); without an OCR model a package with an OCR target is refused with
//! `lab_ocr_unavailable` before anything is evaluated. The command reads files only: it opens
//! no Runtime connection, touches no device, writes nothing and asks nothing, so a CI job can
//! run it; a replayed run that does not complete exits with `replay_run_refused`.
//!
//! Workflow #308 SEL-L6b (FCM row SEL-L6b, T-01): with `--select <operation> --scan <file>`
//! the command replays the scan of that select step instead, through the kernel's own scan path
//! (`dry_run_scan`): the frames are the step's dispatched frame and then every capture its scan
//! took, as a forward run recorded them, and the scan's policy evaluates the same facts a whole
//! replay gives a select step. The scan declaration comes from the file, because a package
//! declares its step's `scan` only from slice SEL-L4b on. A scan that covers its list exits 0;
//! one that ends before it does, or fails, exits with `replay_scan_refused`.
//!
//! #411 Lab RP: the scan's policy reads the package's run parameters as a run's select step does
//! (`session.dispatch.parameter.<name>`): the declared defaults, each `--parameter <name>=<value>`
//! replacing its parameter's default, as a dispatch's chosen value does. A name the package does
//! not declare, a value that is not one of its declared options, a name given twice, a malformed
//! `--parameter` and `--parameter` without `--select` are refused with `validation_failed` naming
//! `--parameter`, never ignored.

use super::codes::ActinglabCode;
use super::{CliError, CliOutcome, ErrorKind, FlagArgs, GlobalOptions};
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::Detail;
use actingcommand_contract::{ContainedTaskRequest, InstanceFactSnapshot, ScanCoverageState};
use actingcommand_lab::{
    ExternallyVerifiedBundle, GivenParameterProblem, OfflineReplayEnd, OfflineReplayError,
    OfflineReplayFacts, PreparedContainedTask, ScanDeclaration, replay_contained_task, replay_scan,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The instance a replayed package is admitted for.
const REPLAY_INSTANCE: &str = "offline.replay";
/// A Runtime answers one instance's facts within one 1 MiB IPC frame; a saved answer stays well
/// inside this bound.
const MAX_FACTS_BYTES: u64 = 4 * 1024 * 1024;
/// A scan declaration is a few hundred bytes; a saved one stays well inside this bound.
const MAX_SCAN_BYTES: u64 = 1024 * 1024;

pub(super) fn run_replay(global: &GlobalOptions, flags: &FlagArgs) -> CliOutcome<Value> {
    let args = ReplayArgs::parse(global, flags)?;
    let facts = match &args.facts {
        Some(path) => read_facts(path)?,
        // A select step evaluates the empty snapshot at the instant the replay started.
        None => OfflineReplayFacts::Empty {
            now_unix_ms: process_now()?,
        },
    };
    let facts_used = facts_report(&facts);
    let paths = frame_paths(&args.frames)?;
    if let Some(select) = &args.select {
        return run_scan_replay(
            &args.package,
            select,
            &paths,
            (&facts, facts_used),
            &args.parameters,
        );
    }
    let task = admit(&args.package)?;
    let frames = paths
        .iter()
        .map(|path| crate::package_cli::offline::read_fixture_frame(path.as_path()));
    let result = replay_contained_task(&task, frames, facts).map_err(|error| match error {
        OfflineReplayError::Frames(error) => error,
        OfflineReplayError::Invariant(error) => CliError::new(
            ErrorKind::UsageValidation,
            error.code().as_str(),
            error.to_string(),
            &[],
        ),
    })?;
    let mut payload = serde_json::to_value(&result)
        .map_err(|error| CliError::device(format!("failed to encode the replay: {error}")))?;
    payload["frames_given"] = json!(paths.len());
    payload["facts"] = facts_used;
    payload["ocr"] = crate::offline_ocr::status();
    payload["production_global_ledger_written"] = json!(false);
    match &result.end {
        OfflineReplayEnd::Completed { .. } => Ok(payload),
        OfflineReplayEnd::Refused { outcome } => Err(CliError::new(
            ErrorKind::UsageValidation,
            ActinglabCode::ReplayRunRefused.as_str(),
            format!(
                "the replayed run ended with {} after {} of {} frames",
                outcome.code().as_str(),
                result.frames_read,
                paths.len()
            ),
            &[],
        )
        .with_details(payload)),
    }
}

struct ReplayArgs {
    package: PathBuf,
    frames: PathBuf,
    facts: Option<PathBuf>,
    /// Workflow #308 SEL-L6b: the select step whose scan is replayed, and its declaration.
    select: Option<ScanSelect>,
    /// #411 Lab RP: the `--parameter <name>=<value>` given, in order (read only with `select`).
    parameters: Vec<(String, String)>,
}

/// `--select <operation> --scan <file>`.
struct ScanSelect {
    operation: String,
    declaration: PathBuf,
}

/// How a replayed scan ended (`end.status`).
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum ScanReplayStatus {
    /// It covered its list: coverage `complete` or `early_stopped`.
    Covered,
    /// The selection evaluator ended it before its coverage.
    Aborted,
    /// It failed with the outcome in `end.outcome`.
    Failed,
}

impl ReplayArgs {
    fn parse(global: &GlobalOptions, flags: &FlagArgs) -> CliOutcome<Self> {
        flags.expect_positionals("replay", 0)?;
        if global.run_root.is_some()
            || global.instance.is_some()
            || global.instances.is_some()
            || global.profile.is_some()
            || global.resource_root.is_some()
            || global.dry_run
            || global.verbose
            || global.quiet
            || global.game.is_some()
            || global.server.is_some()
            || global.runtime_endpoint.is_some()
            || global.capture_backend.is_some()
            || global.touch_backend.is_some()
        {
            return Err(CliError::new(
                ErrorKind::UsageValidation,
                "offline_device_scope_forbidden",
                "replay accepts no global Runtime, device, selector, backend, resource, or execution flags",
                &[],
            ));
        }
        if let Some(name) = flags.flags.keys().find(|name| {
            !matches!(
                name.as_str(),
                "--package" | "--frames" | "--facts" | "--select" | "--scan" | "--parameter"
            )
        }) {
            return Err(CliError::usage(format!("replay does not accept {name}")));
        }
        let select = match (
            single_value(flags, "--select")?,
            single_path(flags, "--scan")?,
        ) {
            (Some(operation), Some(declaration)) => Some(ScanSelect {
                operation,
                declaration,
            }),
            (None, None) => None,
            _ => {
                return Err(CliError::usage(
                    "replay takes --select <operation> and --scan <file> together",
                ));
            }
        };
        let parameters = given_parameters(flags)?;
        if select.is_none() && !parameters.is_empty() {
            return Err(parameter_refusal(
                "conflicting",
                "replay reads --parameter only with --select <operation> and --scan <file>",
            ));
        }
        Ok(Self {
            package: single_path(flags, "--package")?
                .ok_or_else(|| CliError::usage("replay requires --package <path>"))?,
            frames: single_path(flags, "--frames")?
                .ok_or_else(|| CliError::usage("replay requires --frames <directory>"))?,
            facts: single_path(flags, "--facts")?,
            select,
            parameters,
        })
    }
}

/// #411 Lab RP: the `--parameter <name>=<value>` values as given, before anything is read, one
/// flag per distinct name: a flag without its value is `value_missing`, a value that is not
/// `<name>=<value>` is `not_recognised`, and a name given twice is `repeated` (the rule of #411
/// Lab B-2, decision Q9: never last-wins).
fn given_parameters(flags: &FlagArgs) -> CliOutcome<Vec<(String, String)>> {
    let mut given: Vec<(String, String)> = Vec::new();
    for value in flags.values("--parameter") {
        if value == "true" || value.trim().is_empty() {
            return Err(parameter_refusal(
                "value_missing",
                "replay takes --parameter <name>=<value>",
            ));
        }
        let Some((name, option)) = value
            .split_once('=')
            .filter(|(name, option)| !name.is_empty() && !option.is_empty())
        else {
            return Err(parameter_refusal(
                "not_recognised",
                format!("--parameter {value} is not <name>=<value>"),
            ));
        };
        if given.iter().any(|(given, _)| given == name) {
            return Err(parameter_refusal(
                "repeated",
                format!(
                    "replay takes --parameter {name} once; a repeated parameter is refused, never last-wins"
                ),
            ));
        }
        given.push((name.to_owned(), option.to_owned()));
    }
    Ok(given)
}

/// #411 Lab RP: a refused `--parameter`: `validation_failed` (exit 2) with the details
/// `{argument: "--parameter", argument_problem, arguments: [{argument, argument_problem}]}` of the
/// Lab's other option refusals (#411 Lab L1, B-2).
fn parameter_refusal(problem: &str, message: impl Into<String>) -> CliError {
    CliError::usage(message).with_details(json!({
        "argument": "--parameter",
        "argument_problem": problem,
        "arguments": [{"argument": "--parameter", "argument_problem": problem}],
    }))
}

/// The one path of `name`, `None` when it is not given.
fn single_path(flags: &FlagArgs, name: &str) -> CliOutcome<Option<PathBuf>> {
    Ok(single_value(flags, name)?.map(PathBuf::from))
}

/// The one value of `name`, `None` when it is not given.
fn single_value(flags: &FlagArgs, name: &str) -> CliOutcome<Option<String>> {
    match flags.values(name).as_slice() {
        [] => Ok(None),
        [value] if value != "true" && !value.trim().is_empty() => Ok(Some(value.clone())),
        _ => Err(CliError::usage(format!(
            "replay takes exactly one non-empty {name} value"
        ))),
    }
}

/// Workflow #308 SEL-L6b: the scan of the select step `select` replayed over the frames at
/// `paths` with `facts`. The package is admitted (a pack with an OCR target is refused without
/// an OCR model before any frame is read), every frame is read, and the scan runs through the
/// kernel's scan path. A scan that covers its list (`complete` or `early_stopped`) is the
/// result; a scan the evaluator ended before its coverage (its decision in `scan.decision`) or
/// that failed (its outcome in `end.outcome`) is `replay_scan_refused`, with the whole replay in
/// the details. #411 Lab RP: the scan's policy reads the package's declared parameter defaults,
/// each of `parameters` replacing its parameter's default; a parameter the package does not
/// declare, or a value that is not one of its options, is refused before any frame is read.
fn run_scan_replay(
    package: &Path,
    select: &ScanSelect,
    paths: &[PathBuf],
    (facts, facts_used): (&OfflineReplayFacts, Value),
    parameters: &[(String, String)],
) -> CliOutcome<Value> {
    let declaration = read_scan(&select.declaration)?;
    let task = admit(package)?;
    let run_parameters = task.given_run_parameters(parameters).map_err(|refusal| {
        let name = &refusal.name;
        match refusal.problem {
            GivenParameterProblem::Undeclared => parameter_refusal(
                "not_recognised",
                format!("the package declares no parameter {name}"),
            ),
            GivenParameterProblem::NotAnOption => parameter_refusal(
                "not_recognised",
                format!("the value given for {name} is not one of its declared options"),
            ),
            GivenParameterProblem::Repeated => {
                parameter_refusal("repeated", format!("replay takes --parameter {name} once"))
            }
        }
    })?;
    let frames = paths
        .iter()
        .map(|path| crate::package_cli::offline::read_fixture_frame(path.as_path()))
        .collect::<CliOutcome<Vec<_>>>()?;
    let result = replay_scan(
        &task,
        &select.operation,
        &declaration,
        frames,
        facts,
        &run_parameters,
    );
    let mut payload = json!({
        "mode": "offline_scan_replay",
        "executed": false,
        "package_id": task.package_label(),
        "task_id": task.task_label(),
        "operation_id": select.operation,
        "frames_given": paths.len(),
        "facts": facts_used,
        "ocr": crate::offline_ocr::status(),
        "production_global_ledger_written": false,
    });
    let ended = match result {
        Ok(scan) => {
            let covered = matches!(
                scan.coverage,
                ScanCoverageState::Complete | ScanCoverageState::EarlyStopped
            );
            let status = if covered {
                ScanReplayStatus::Covered
            } else {
                ScanReplayStatus::Aborted
            };
            payload["end"] = json!({"status": status, "coverage": scan.coverage});
            payload["scan"] = serde_json::to_value(&scan).map_err(|error| {
                CliError::device(format!("failed to encode the scan replay: {error}"))
            })?;
            if covered {
                return Ok(payload);
            }
            format!("the evaluator ended it with coverage {:?}", scan.coverage)
        }
        Err(error) => {
            payload["end"] = json!({
                "status": ScanReplayStatus::Failed,
                "outcome": error.outcome().envelope(Detail::Full),
            });
            format!("it failed with {}", error.code().as_str())
        }
    };
    Err(CliError::new(
        ErrorKind::UsageValidation,
        ActinglabCode::ReplayScanRefused.as_str(),
        format!(
            "the replayed scan of {} did not cover its list: {ended}",
            select.operation
        ),
        &[],
    )
    .with_details(payload))
}

/// The `--scan` file: one scan declaration of a select step (selector model S0 v2.1 §2.1), as
/// the step's `scan` member declares it.
fn read_scan(path: &Path) -> CliOutcome<ScanDeclaration> {
    let unreadable = |error: std::io::Error| {
        CliError::new(
            ErrorKind::UsageValidation,
            ActinglabCode::ReplayScanUnreadable.as_str(),
            format!("failed to read --scan {}: {error}", path.display()),
            &[],
        )
    };
    let invalid = |problem: String| {
        CliError::new(
            ErrorKind::UsageValidation,
            ActinglabCode::ReplayScanInvalid.as_str(),
            format!("--scan {}: {problem}", path.display()),
            &[],
        )
    };
    let mut bytes = Vec::new();
    fs::File::open(path)
        .map_err(unreadable)?
        .take(MAX_SCAN_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(unreadable)?;
    if bytes.len() as u64 > MAX_SCAN_BYTES {
        return Err(invalid(format!("larger than {MAX_SCAN_BYTES} bytes")));
    }
    serde_json::from_slice(&bytes)
        .map_err(|error| invalid(format!("not a scan declaration: {error}")))
}

fn process_now() -> CliOutcome<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| u64::try_from(elapsed.as_millis()).ok())
        .filter(|now| *now > 0)
        .ok_or_else(|| {
            CliError::new(
                ErrorKind::UsageValidation,
                "offline_clock_invalid",
                "the Lab process clock is before the Unix epoch",
                &[],
            )
        })
}

/// The frames of `directory`: every entry, each a PNG file, in file-name order.
fn frame_paths(directory: &Path) -> CliOutcome<Vec<PathBuf>> {
    let unreadable = |error: std::io::Error| {
        CliError::new(
            ErrorKind::UsageValidation,
            ActinglabCode::ReplayFramesUnreadable.as_str(),
            format!("failed to list --frames {}: {error}", directory.display()),
            &[],
        )
    };
    let mut paths = Vec::new();
    for entry in fs::read_dir(directory).map_err(unreadable)? {
        let entry = entry.map_err(unreadable)?;
        let path = entry.path();
        let png = entry.file_type().map_err(unreadable)?.is_file()
            && path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("png"));
        if !png {
            return Err(CliError::new(
                ErrorKind::UsageValidation,
                ActinglabCode::ReplayFramesInvalid.as_str(),
                format!(
                    "--frames {} holds {}, which is not a PNG file",
                    directory.display(),
                    path.display()
                ),
                &[],
            ));
        }
        paths.push(path);
    }
    paths.sort();
    if paths.is_empty() {
        return Err(CliError::new(
            ErrorKind::UsageValidation,
            SharedCode::OfflineFixtureMissing.as_str(),
            format!("--frames {} holds no PNG frame", directory.display()),
            &[],
        ));
    }
    Ok(paths)
}

/// The package at `package`, read by its path (Workflow #336 option 丙: no reference is given
/// or compared) with the Runtime's OCR provider when actinglab has one.
fn admit(package: &Path) -> CliOutcome<PreparedContainedTask> {
    let package = if package.is_absolute() {
        package.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| CliError::package_invalid(error.to_string()))?
            .join(package)
    };
    let deadline = Instant::now()
        .checked_add(Duration::from_millis(
            ContainedTaskRequest::DEFAULT_RESPONSE_DEADLINE_MS,
        ))
        .ok_or_else(|| CliError::package_invalid("package admission deadline overflow"))?;
    let bundle = ExternallyVerifiedBundle::load_path(
        REPLAY_INSTANCE,
        &package,
        false,
        crate::offline_ocr::lab_ocr().provider(),
        deadline,
    )
    .map_err(|error| CliError::package_invalid(error.to_string()))?;
    crate::offline_ocr::require_package(&bundle)?;
    PreparedContainedTask::from_bundle(bundle).map_err(|error| {
        CliError::package_invalid(format!(
            "{}: {}",
            error.code(),
            error.detail().unwrap_or("task preparation failed")
        ))
    })
}

/// One instance's facts as a Runtime answers them: the result of `actingctl facts --instance`
/// (the members of its line; the line's `outcome` is not read) or of MCP `ac_facts` with scope
/// `instance`.
#[derive(Deserialize)]
struct InstanceFactsAnswer {
    scope: String,
    read_at_unix_ms: u64,
    snapshot: InstanceFactSnapshot,
    records_total: u64,
    records_kept: u64,
}

/// The `--facts` snapshot, evaluated at the instant the Runtime read it. An answer read with a
/// key prefix is refused: it leaves out records a select step may read.
fn read_facts(path: &Path) -> CliOutcome<OfflineReplayFacts> {
    let unreadable = |error: std::io::Error| {
        CliError::new(
            ErrorKind::UsageValidation,
            ActinglabCode::ReplayFactsUnreadable.as_str(),
            format!("failed to read --facts {}: {error}", path.display()),
            &[],
        )
    };
    let invalid = |problem: String| {
        CliError::new(
            ErrorKind::UsageValidation,
            ActinglabCode::ReplayFactsInvalid.as_str(),
            format!("--facts {}: {problem}", path.display()),
            &[],
        )
    };
    let mut bytes = Vec::new();
    fs::File::open(path)
        .map_err(unreadable)?
        .take(MAX_FACTS_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(unreadable)?;
    if bytes.len() as u64 > MAX_FACTS_BYTES {
        return Err(invalid(format!("larger than {MAX_FACTS_BYTES} bytes")));
    }
    let answer: InstanceFactsAnswer = serde_json::from_slice(&bytes)
        .map_err(|error| invalid(format!("not an instance fact answer: {error}")))?;
    if answer.scope != "instance" {
        return Err(invalid(format!(
            "an answer of scope {}, not instance",
            answer.scope
        )));
    }
    if answer.records_kept != answer.records_total {
        return Err(invalid(format!(
            "an answer read with a key prefix: {} of {} records kept",
            answer.records_kept, answer.records_total
        )));
    }
    if answer.read_at_unix_ms == 0 {
        return Err(invalid("read_at_unix_ms is 0".to_string()));
    }
    answer
        .snapshot
        .validate()
        .map_err(|error| invalid(format!("the snapshot is invalid: {error}")))?;
    Ok(OfflineReplayFacts::Snapshot {
        snapshot: answer.snapshot,
        read_at_unix_ms: answer.read_at_unix_ms,
    })
}

/// What the replay evaluated a select step with: the given snapshot and the instant it was
/// read, or the empty snapshot and the replay's own instant.
fn facts_report(facts: &OfflineReplayFacts) -> Value {
    match facts {
        OfflineReplayFacts::Empty { now_unix_ms } => {
            json!({"source": "empty", "now_unix_ms": now_unix_ms})
        }
        OfflineReplayFacts::Snapshot {
            snapshot,
            read_at_unix_ms,
        } => json!({
            "source": "snapshot",
            "snapshot_id": snapshot.snapshot_id,
            "records": snapshot.records.len(),
            "read_at_unix_ms": read_at_unix_ms
        }),
    }
}
