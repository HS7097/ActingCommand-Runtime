// SPDX-License-Identifier: AGPL-3.0-only

#![forbid(unsafe_code)]

mod codes;

use std::borrow::Cow;
use std::error::Error;
use std::ffi::OsString;
use std::fmt;
use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

use crate::codes::{ActingledgerCode, ActingledgerLocation};
use actingcommand_contract::codes::{ContractLocation, SharedCode};
use actingcommand_contract::outcome::{Code, Location};
use actingcommand_contract::{DiagnosticCode, InstanceId, RunId};
use actingcommand_ledger_forensics::{
    ForensicCommand, ForensicEventFilter, ForensicEventsRequest, ForensicFramesRequest,
    ForensicOutput, ForensicReplayRequest, ForensicReport, ForensicRequest,
    ForensicRuntimeFactsResult, ForensicViewOptions, ForensicViewRequest, ForensicsCode,
    ForensicsLocation, FrameClassFilter, LocalDate, MAX_FORENSIC_EVENTS, TaskRecordsRequest,
};

/// Cooperative deadline of each phase of one `facts` replay, from the phase's own start
/// (Workflow #395).
const FACTS_READ_BUDGET: Duration = Duration::from_secs(30);

enum CliRequest {
    Material(Box<actingcommand_ledger_forensics::ForensicMaterialRequest>),
    Views(Box<ForensicViewRequest>),
    StateRoot(ForensicRequest),
    Replay(ForensicReplayRequest),
    Signatures(actingcommand_ledger_forensics::ForensicSignatureRequest),
    Facts { state_root: PathBuf, position: u64 },
    Frames(Box<ForensicFramesRequest>),
}

#[derive(Debug)]
pub struct CliError {
    code: Cow<'static, str>,
    operation: Cow<'static, str>,
    detail: String,
}

impl CliError {
    /// A command failure with a registered code and location.
    fn new(code: Code, operation: Location, detail: impl Into<String>) -> Self {
        Self::relayed(code.as_str(), operation.as_str(), detail)
    }

    /// A failure that relays a forensic error's code and operation, which stay strings while
    /// they carry the ledger's and the artifact store's (Workflow #378, slice A2b), or the code
    /// and operation of a reported outcome (ledger revision 3, A3).
    fn relayed(
        code: impl Into<Cow<'static, str>>,
        operation: impl Into<Cow<'static, str>>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            code: code.into(),
            operation: operation.into(),
            detail: detail.into(),
        }
    }

    pub fn code(&self) -> &str {
        &self.code
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} during {}: {}",
            self.code, self.operation, self.detail
        )
    }
}

impl Error for CliError {}

pub fn run<I, W>(args: I, output: &mut W) -> Result<(), CliError>
where
    I: IntoIterator<Item = OsString>,
    W: Write,
{
    let report = match parse_args(args)? {
        CliRequest::Material(request) => {
            return actingcommand_ledger_forensics::read_material_to(*request, output).map_err(
                |error| CliError::relayed(error.code(), error.operation(), error.to_string()),
            );
        }
        CliRequest::Views(request) => actingcommand_ledger_forensics::run_views(*request),
        CliRequest::StateRoot(request) => actingcommand_ledger_forensics::run(request),
        CliRequest::Replay(request) => actingcommand_ledger_forensics::replay(request),
        CliRequest::Signatures(request) => {
            actingcommand_ledger_forensics::replay_signatures_read_only(request)
        }
        CliRequest::Frames(request) => actingcommand_ledger_forensics::list_frames(*request),
        CliRequest::Facts {
            state_root,
            position,
        } => Ok(ForensicOutput::Machine(ForensicReport::Facts(Box::new(
            actingcommand_ledger_forensics::runtime_facts_at_per_phase(
                state_root,
                position,
                |_| FACTS_READ_BUDGET,
            ),
        )))),
    }
    .map_err(|error| CliError::relayed(error.code(), error.operation(), error.to_string()))?;
    let stability_incomplete = matches!(&report, ForensicOutput::Machine(ForensicReport::Stability(report)) if !report.gaps.is_empty());
    let task_evidence_incomplete = matches!(&report, ForensicOutput::Machine(ForensicReport::TaskEvidence(report)) if !report.gaps.is_empty());
    let signatures_incomplete = matches!(&report, ForensicOutput::Machine(ForensicReport::Signatures(report)) if !report.evidence_complete);
    let views_incomplete = matches!(&report, ForensicOutput::Machine(ForensicReport::Views(page))
        if page.read_scope().is_none_or(|scope| !scope.read_complete));
    let facts_unavailable = match &report {
        ForensicOutput::Machine(ForensicReport::Facts(result)) => match result.as_ref() {
            ForensicRuntimeFactsResult::Available { .. } => None,
            ForensicRuntimeFactsResult::NotAvailable { .. } => Some((
                Cow::Borrowed(ForensicsCode::RuntimeFactsNotAvailable.as_str()),
                Cow::Borrowed(ForensicsLocation::ReadRuntimeFactsAt.as_str()),
            )),
            // Ledger revision 3 (Workflow #378 A3): the failure is the result's outcome.
            ForensicRuntimeFactsResult::Failed { outcome, .. } => Some((
                Cow::Owned(outcome.code().as_str().to_owned()),
                Cow::Owned(
                    outcome
                        .value("operation")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or(ForensicsLocation::ReadRuntimeFactsAt.as_str())
                        .to_owned(),
                ),
            )),
        },
        _ => None,
    };
    match report {
        ForensicOutput::Machine(report) => {
            serde_json::to_writer(&mut *output, &report).map_err(serialization_error)?;
            output.write_all(b"\n").map_err(output_error)?;
        }
        ForensicOutput::Human(report) => {
            output.write_all(report.as_bytes()).map_err(output_error)?;
            if !report.ends_with('\n') {
                output.write_all(b"\n").map_err(output_error)?;
            }
        }
    }
    output.flush().map_err(output_error)?;
    if let Some((code, operation)) = facts_unavailable.filter(|_| false) {
        return Err(CliError::relayed(
            code,
            operation,
            "see the structured runtime fact result",
        ));
    }
    if views_incomplete {
        return Err(CliError::new(
            ActingledgerCode::LedgerViewSourceIncomplete.code(),
            ContractLocation::ReadLedgerViews.location(),
            "see source completeness and read-through position in the page",
        ));
    }
    if signatures_incomplete {
        return Err(CliError::new(
            SharedCode::SignatureReplayIncomplete.code(),
            ActingledgerLocation::ReplaySignatures.location(),
            "see frozen prefixes, source snapshots and explicit gaps in the report",
        ));
    }
    if stability_incomplete {
        return Err(CliError::new(
            ActingledgerCode::StabilityExportIncomplete.code(),
            ActingledgerLocation::ExportStability.location(),
            "see structured gaps and failures in the report",
        ));
    }
    if task_evidence_incomplete {
        return Err(CliError::new(
            ActingledgerCode::TaskEvidenceExportIncomplete.code(),
            ActingledgerLocation::ExportTaskEvidence.location(),
            "see explicit relations, gaps and failures in the report",
        ));
    }
    Ok(())
}

pub fn run_env() -> Result<(), CliError> {
    run(std::env::args_os().skip(1), &mut std::io::stdout().lock())
}

fn parse_args<I>(args: I) -> Result<CliRequest, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let mut args = args.into_iter();
    let entry = args
        .next()
        .ok_or_else(|| invalid_arguments("missing command or --state-root"))?
        .into_string()
        .map_err(|_| invalid_arguments("command or --state-root is not valid UTF-8"))?;
    if entry == "replay" {
        return parse_replay(args);
    }
    if entry != "--state-root" {
        return Err(invalid_arguments("expected replay or --state-root"));
    }
    let state_root = PathBuf::from(
        args.next()
            .ok_or_else(|| invalid_arguments("missing state root"))?,
    );
    if state_root.as_os_str().is_empty() {
        return Err(invalid_arguments("state root is empty"));
    }
    let command = require_utf8(args.next(), "command")?;
    let command = match command.as_str() {
        "material" => {
            require_utf8(args.next(), "--request")?;
            let json = next_value(&mut args, "--request")?;
            if json.len() > 16 * 1024 || args.next().is_some() {
                return Err(invalid_arguments(
                    "material request exceeds bound or has extra arguments",
                ));
            }
            let request = serde_json::from_str(&json)
                .map_err(|_| invalid_arguments("invalid typed material request"))?;
            return actingcommand_ledger_forensics::ForensicMaterialRequest::new(
                state_root, request,
            )
            .map(|request| CliRequest::Material(Box::new(request)))
            .map_err(|error| {
                CliError::relayed(error.code(), error.operation(), error.to_string())
            });
        }
        "views" => {
            return parse_views(state_root, args)
                .map(|request| CliRequest::Views(Box::new(request)));
        }
        "signatures" => return parse_signatures(state_root, args).map(CliRequest::Signatures),
        "frames" => {
            return parse_frames(state_root, args)
                .map(|request| CliRequest::Frames(Box::new(request)));
        }
        "facts" => {
            require_utf8(args.next(), "--at")?;
            let position = parse_u64(&next_value(&mut args, "--at")?, "--at")?;
            if args.next().is_some() {
                return Err(invalid_arguments("facts accepts only --at <sequence>"));
            }
            return Ok(CliRequest::Facts {
                state_root,
                position,
            });
        }
        "open" => ForensicCommand::Open,
        "events" => {
            return parse_events(state_root, args, ForensicCommand::Events)
                .map(CliRequest::StateRoot);
        }
        "chain" => {
            require_utf8(args.next(), "--req")?;
            let request_id = require_utf8(args.next(), "request id")?;
            if request_id.is_empty() {
                return Err(invalid_arguments("request id is empty"));
            }
            ForensicCommand::Chain { request_id }
        }
        "tail" => ForensicCommand::Tail,
        "repairs" => ForensicCommand::Repairs,
        "export" => {
            if let Some(option) = args.next() {
                let option = option
                    .into_string()
                    .map_err(|_| invalid_arguments("export mode is not valid UTF-8"))?;
                let mode = match option.as_str() {
                    "--performance" => ForensicCommand::Performance,
                    "--stability" => ForensicCommand::Stability,
                    "--task-evidence" => ForensicCommand::TaskEvidence,
                    _ => return Err(invalid_arguments("unsupported export mode")),
                };
                return parse_events(state_root, args, mode).map(CliRequest::StateRoot);
            }
            ForensicCommand::Export
        }
        _ => return Err(invalid_arguments("unsupported command")),
    };
    if args.next().is_some() {
        return Err(invalid_arguments(
            "unexpected argument or unsupported filter",
        ));
    }
    Ok(CliRequest::StateRoot(ForensicRequest::new(
        state_root, command,
    )))
}

fn parse_views<I>(state_root: PathBuf, mut args: I) -> Result<ForensicViewRequest, CliError>
where
    I: Iterator<Item = OsString>,
{
    let mut options = ForensicViewOptions::default();
    while let Some(option) = args.next() {
        let option = option
            .into_string()
            .map_err(|_| invalid_arguments("view option is not UTF-8"))?;
        let value = next_value(&mut args, &option)?;
        match option.as_str() {
            "--query" if options.query.is_none() => options.query = Some(value),
            "--profile" if options.profile.is_none() => options.profile = Some(value),
            "--cursor" if options.cursor.is_none() => options.cursor = Some(value),
            "--snapshot" if options.snapshot.is_none() => {
                options.snapshot = Some(parse_u64(&value, "--snapshot")?)
            }
            "--limit" if options.limit.is_none() => {
                options.limit = Some(
                    value
                        .parse::<u16>()
                        .map_err(|_| invalid_arguments("invalid view page limit"))?,
                )
            }
            "--instance-port" if options.instance_port.is_none() => {
                options.instance_port = Some(
                    value
                        .parse::<u16>()
                        .map_err(|_| invalid_arguments("invalid instance port"))?,
                )
            }
            "--query" | "--profile" | "--cursor" | "--snapshot" | "--limit" | "--instance-port" => {
                return Err(invalid_arguments(format!("duplicate view option {option}")));
            }
            _ => return Err(invalid_arguments(format!("unknown view option {option}"))),
        }
    }
    ForensicViewRequest::from_options(state_root, options)
        .map_err(|error| invalid_arguments(error.to_string()))
}

/// Workflow #375 R5e: `frames [--class <class>] [--before <YYYY-MM-DD>] [--instance <id>]
/// [--run <id>] [--error-sequence <n>] [--paths]`; each option at most once.
fn parse_frames<I>(state_root: PathBuf, mut args: I) -> Result<ForensicFramesRequest, CliError>
where
    I: Iterator<Item = OsString>,
{
    let mut class = None;
    let mut before = None;
    let mut instance = None;
    let mut run = None;
    let mut error_sequence = None;
    let mut paths = false;
    while let Some(option) = args.next() {
        let option = option
            .into_string()
            .map_err(|_| invalid_arguments("frames option is not valid UTF-8"))?;
        if option == "--paths" {
            if paths {
                return Err(invalid_arguments("duplicate frames option --paths"));
            }
            paths = true;
            continue;
        }
        let value = next_value(&mut args, &option)?;
        match option.as_str() {
            "--class" if class.is_none() => {
                class = Some(
                    FrameClassFilter::parse(&value)
                        .ok_or_else(|| invalid_arguments("value for --class is not a class"))?,
                );
            }
            "--before" if before.is_none() => {
                before =
                    Some(LocalDate::parse(&value).ok_or_else(|| {
                        invalid_arguments("value for --before is not YYYY-MM-DD")
                    })?);
            }
            "--instance" if instance.is_none() => {
                instance = Some(
                    serde_json::from_value::<InstanceId>(serde_json::Value::String(value))
                        .map_err(|_| invalid_arguments("value for --instance is not an id"))?,
                );
            }
            "--run" if run.is_none() => {
                run = Some(
                    serde_json::from_value::<RunId>(serde_json::Value::String(value))
                        .map_err(|_| invalid_arguments("value for --run is not an id"))?,
                );
            }
            "--error-sequence" if error_sequence.is_none() => {
                error_sequence = Some(parse_u64(&value, "--error-sequence")?);
            }
            "--class" | "--before" | "--instance" | "--run" | "--error-sequence" => {
                return Err(invalid_arguments(format!(
                    "duplicate frames option {option}"
                )));
            }
            _ => return Err(invalid_arguments(format!("unknown frames option {option}"))),
        }
    }
    Ok(ForensicFramesRequest {
        state_root,
        class: class.unwrap_or(FrameClassFilter::All),
        before,
        instance,
        run,
        error_sequence,
        paths,
    })
}

fn parse_signatures<I>(
    input_state_root: PathBuf,
    mut args: I,
) -> Result<actingcommand_ledger_forensics::ForensicSignatureRequest, CliError>
where
    I: Iterator<Item = OsString>,
{
    let mut catalog_state_root = None;
    let mut input_through = None;
    let mut catalog_through = None;
    let mut limit = None;
    let mut cursor = None;
    while let Some(option) = args.next() {
        let option = option
            .into_string()
            .map_err(|_| invalid_arguments("signature option is not UTF-8"))?;
        let value = next_value(&mut args, &option)?;
        match option.as_str() {
            "--catalog-state-root" if catalog_state_root.is_none() => {
                catalog_state_root = Some(PathBuf::from(value))
            }
            "--through" if input_through.is_none() => {
                input_through = Some(parse_u64(&value, "--through")?)
            }
            "--catalog-through" if catalog_through.is_none() => {
                catalog_through = Some(parse_u64(&value, "--catalog-through")?)
            }
            "--limit" if limit.is_none() => {
                limit = Some(
                    value
                        .parse::<u16>()
                        .map_err(|_| invalid_arguments("invalid signature limit"))?,
                )
            }
            "--cursor" if cursor.is_none() => {
                if value.len() > 4096 {
                    return Err(invalid_arguments("signature cursor exceeds bound"));
                }
                cursor = Some(
                    serde_json::from_str(&value)
                        .map_err(|_| invalid_arguments("invalid signature cursor"))?,
                );
            }
            _ => {
                return Err(invalid_arguments(format!(
                    "duplicate or unsupported signature option {option}"
                )));
            }
        }
    }
    let request = actingcommand_ledger_forensics::ForensicSignatureRequest {
        input_state_root,
        catalog_state_root: catalog_state_root
            .ok_or_else(|| invalid_arguments("missing --catalog-state-root"))?,
        input_through: input_through
            .filter(|value| *value > 0)
            .ok_or_else(|| invalid_arguments("missing positive --through"))?,
        catalog_through: catalog_through
            .filter(|value| *value > 0)
            .ok_or_else(|| invalid_arguments("missing positive --catalog-through"))?,
        page: actingcommand_ledger_forensics::SignaturePageRequest {
            limit: limit.unwrap_or(actingcommand_ledger_forensics::MAX_SIGNATURE_PAGE_ROWS),
            cursor,
        },
    };
    request
        .page
        .validate()
        .map_err(|_| invalid_arguments("invalid signature page"))?;
    Ok(request)
}

fn parse_replay<I>(mut args: I) -> Result<CliRequest, CliError>
where
    I: Iterator<Item = OsString>,
{
    let mut zip_path = None;
    let mut expected_sha256 = None;
    while let Some(option) = args.next() {
        let option = option
            .into_string()
            .map_err(|_| invalid_arguments("replay option is not valid UTF-8"))?;
        match option.as_str() {
            "--zip" if zip_path.is_none() => {
                zip_path = Some(PathBuf::from(next_value(&mut args, "--zip")?));
            }
            "--expected-sha256" if expected_sha256.is_none() => {
                expected_sha256 = Some(next_value(&mut args, "--expected-sha256")?);
            }
            "--zip" | "--expected-sha256" => {
                return Err(invalid_arguments(format!(
                    "duplicate replay option {option}"
                )));
            }
            _ => return Err(invalid_arguments(format!("unknown replay option {option}"))),
        }
    }
    let zip_path = zip_path.ok_or_else(|| invalid_arguments("missing --zip"))?;
    let expected_sha256 =
        expected_sha256.ok_or_else(|| invalid_arguments("missing --expected-sha256"))?;
    Ok(CliRequest::Replay(ForensicReplayRequest::new(
        zip_path,
        expected_sha256,
    )))
}

fn parse_events<I>(
    state_root: PathBuf,
    mut args: I,
    command: ForensicCommand,
) -> Result<ForensicRequest, CliError>
where
    I: Iterator<Item = OsString>,
{
    let mut after_sequence = None;
    let mut through_sequence = None;
    let mut limit = None;
    let mut origin_module = None;
    let mut diagnostic_code = None;
    let mut severity = None;
    let mut correlation_id = None;
    let mut exclude_event_types = Vec::new();
    let mut record_cursor = None;
    let mut record_limit = None;
    let mut include_private = false;
    while let Some(option) = args.next() {
        let option = option
            .into_string()
            .map_err(|_| invalid_arguments("event option is not valid UTF-8"))?;
        if option == "--include-private" && command == ForensicCommand::TaskEvidence {
            if include_private {
                return Err(invalid_arguments("duplicate --include-private"));
            }
            include_private = true;
            continue;
        }
        if command != ForensicCommand::Events
            && !matches!(option.as_str(), "--after" | "--through" | "--limit")
            && !(command == ForensicCommand::TaskEvidence
                && matches!(option.as_str(), "--record-cursor" | "--record-limit"))
        {
            return Err(invalid_arguments(format!(
                "unsupported export page option {option}"
            )));
        }
        let value = next_value(&mut args, &option)?;
        match option.as_str() {
            "--record-cursor"
                if command == ForensicCommand::TaskEvidence && record_cursor.is_none() =>
            {
                if value.len() > 2048 {
                    return Err(invalid_arguments("record cursor exceeds bound"));
                }
                record_cursor = Some(
                    serde_json::from_str(&value)
                        .map_err(|_| invalid_arguments("invalid record cursor"))?,
                );
            }
            "--record-limit"
                if command == ForensicCommand::TaskEvidence && record_limit.is_none() =>
            {
                record_limit = Some(parse_usize(&value, "--record-limit")?)
            }
            "--after" if after_sequence.is_none() => {
                after_sequence = Some(parse_u64(&value, "--after")?)
            }
            "--through" if through_sequence.is_none() => {
                through_sequence = Some(parse_u64(&value, "--through")?)
            }
            "--limit" if limit.is_none() => limit = Some(parse_usize(&value, "--limit")?),
            "--origin-module" if origin_module.is_none() => origin_module = Some(value),
            "--diagnostic-code" if diagnostic_code.is_none() => {
                diagnostic_code = Some(parse_diagnostic_code(&value)?)
            }
            "--severity" if severity.is_none() => severity = Some(value),
            "--correlation-id" if correlation_id.is_none() => correlation_id = Some(value),
            "--exclude-event-type" => exclude_event_types.push(value),
            "--after" | "--through" | "--limit" | "--origin-module" | "--diagnostic-code"
            | "--severity" | "--correlation-id" | "--record-cursor" | "--record-limit" => {
                return Err(invalid_arguments(format!(
                    "duplicate event option {option}"
                )));
            }
            _ => return Err(invalid_arguments(format!("unknown event option {option}"))),
        }
    }
    let filter = ForensicEventFilter::new(origin_module, diagnostic_code, severity, correlation_id)
        .and_then(|filter| filter.with_exclude_event_types(exclude_event_types))
        .map_err(|error| invalid_arguments(error.to_string()))?;
    let events = ForensicEventsRequest::new(
        filter,
        after_sequence.unwrap_or(0),
        through_sequence,
        limit.unwrap_or(MAX_FORENSIC_EVENTS),
    )
    .map_err(|error| invalid_arguments(error.to_string()))?;
    Ok(match command {
        ForensicCommand::Performance => ForensicRequest::performance(state_root, events),
        ForensicCommand::Stability => ForensicRequest::stability(state_root, events),
        ForensicCommand::TaskEvidence => ForensicRequest::task_evidence(state_root, events)
            .with_task_records(TaskRecordsRequest {
                cursor: record_cursor,
                limit: record_limit.unwrap_or(16),
                include_private,
            })
            .map_err(|error| invalid_arguments(error.to_string()))?,
        _ => ForensicRequest::events(state_root, events),
    })
}

fn next_value<I>(args: &mut I, option: &str) -> Result<String, CliError>
where
    I: Iterator<Item = OsString>,
{
    let value = args
        .next()
        .ok_or_else(|| invalid_arguments(format!("missing value for {option}")))?;
    let value = value
        .into_string()
        .map_err(|_| invalid_arguments(format!("value for {option} is not valid UTF-8")))?;
    if value.is_empty() {
        return Err(invalid_arguments(format!("value for {option} is empty")));
    }
    Ok(value)
}

/// The typed diagnostic code a `--diagnostic-code` token names (ledger revision 3, Workflow
/// #378 A3); an unknown token is a usage error.
fn parse_diagnostic_code(value: &str) -> Result<DiagnosticCode, CliError> {
    DiagnosticCode::ALL
        .iter()
        .copied()
        .find(|code| code.as_str() == value)
        .ok_or_else(|| invalid_arguments("value for --diagnostic-code is not a diagnostic code"))
}

fn parse_u64(value: &str, option: &str) -> Result<u64, CliError> {
    value
        .parse()
        .map_err(|_| invalid_arguments(format!("value for {option} is not a valid u64")))
}

fn parse_usize(value: &str, option: &str) -> Result<usize, CliError> {
    value
        .parse()
        .map_err(|_| invalid_arguments(format!("value for {option} is not a valid usize")))
}

fn require_utf8(value: Option<OsString>, expected: &str) -> Result<String, CliError> {
    let value = value.ok_or_else(|| invalid_arguments(format!("missing {expected}")))?;
    value
        .into_string()
        .map_err(|_| invalid_arguments(format!("{expected} is not valid UTF-8")))
        .and_then(|value| {
            if value == expected || expected == "command" || expected == "request id" {
                Ok(value)
            } else {
                Err(invalid_arguments(format!("expected {expected}")))
            }
        })
}

fn invalid_arguments(detail: impl Into<String>) -> CliError {
    CliError::new(
        SharedCode::InvalidArguments.code(),
        ActingledgerLocation::ParseArguments.location(),
        detail,
    )
}

fn output_error(error: std::io::Error) -> CliError {
    CliError::new(
        ActingledgerCode::OutputFailed.code(),
        ActingledgerLocation::WriteForensicOutput.location(),
        error.to_string(),
    )
}

fn serialization_error(error: serde_json::Error) -> CliError {
    if error.is_io() {
        CliError::new(
            ActingledgerCode::OutputFailed.code(),
            ActingledgerLocation::WriteForensicOutput.location(),
            error.to_string(),
        )
    } else {
        CliError::new(
            SharedCode::SerializationFailed.code(),
            ActingledgerLocation::SerializeForensicReport.location(),
            error.to_string(),
        )
    }
}
