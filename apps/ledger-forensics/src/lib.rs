// SPDX-License-Identifier: AGPL-3.0-only

#![forbid(unsafe_code)]

use std::error::Error;
use std::ffi::OsString;
use std::fmt;
use std::io::Write;
use std::path::PathBuf;

use actingcommand_ledger_forensics::{
    ForensicCommand, ForensicEventFilter, ForensicEventsRequest, ForensicOutput,
    ForensicReplayRequest, ForensicRequest, MAX_FORENSIC_EVENTS,
};

enum CliRequest {
    StateRoot(ForensicRequest),
    Replay(ForensicReplayRequest),
}

#[derive(Debug)]
pub struct CliError {
    code: &'static str,
    operation: &'static str,
    detail: String,
}

impl CliError {
    fn new(code: &'static str, operation: &'static str, detail: impl Into<String>) -> Self {
        Self {
            code,
            operation,
            detail: detail.into(),
        }
    }

    pub fn code(&self) -> &'static str {
        self.code
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
        CliRequest::StateRoot(request) => actingcommand_ledger_forensics::run(request),
        CliRequest::Replay(request) => actingcommand_ledger_forensics::replay(request),
    }
    .map_err(|error| CliError::new(error.code(), error.operation(), error.to_string()))?;
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
    output.flush().map_err(output_error)
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
        "open" => ForensicCommand::Open,
        "events" => return parse_events(state_root, args, false).map(CliRequest::StateRoot),
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
                require_utf8(Some(option), "--performance")?;
                return parse_events(state_root, args, true).map(CliRequest::StateRoot);
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
    performance: bool,
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
    while let Some(option) = args.next() {
        let option = option
            .into_string()
            .map_err(|_| invalid_arguments("event option is not valid UTF-8"))?;
        if performance && !matches!(option.as_str(), "--after" | "--through" | "--limit") {
            return Err(invalid_arguments(format!(
                "unsupported performance option {option}"
            )));
        }
        let value = next_value(&mut args, &option)?;
        match option.as_str() {
            "--after" if after_sequence.is_none() => {
                after_sequence = Some(parse_u64(&value, "--after")?)
            }
            "--through" if through_sequence.is_none() => {
                through_sequence = Some(parse_u64(&value, "--through")?)
            }
            "--limit" if limit.is_none() => limit = Some(parse_usize(&value, "--limit")?),
            "--origin-module" if origin_module.is_none() => origin_module = Some(value),
            "--diagnostic-code" if diagnostic_code.is_none() => diagnostic_code = Some(value),
            "--severity" if severity.is_none() => severity = Some(value),
            "--correlation-id" if correlation_id.is_none() => correlation_id = Some(value),
            "--after" | "--through" | "--limit" | "--origin-module" | "--diagnostic-code"
            | "--severity" | "--correlation-id" => {
                return Err(invalid_arguments(format!(
                    "duplicate event option {option}"
                )));
            }
            _ => return Err(invalid_arguments(format!("unknown event option {option}"))),
        }
    }
    let filter = ForensicEventFilter::new(origin_module, diagnostic_code, severity, correlation_id)
        .map_err(|error| invalid_arguments(error.to_string()))?;
    let events = ForensicEventsRequest::new(
        filter,
        after_sequence.unwrap_or(0),
        through_sequence,
        limit.unwrap_or(MAX_FORENSIC_EVENTS),
    )
    .map_err(|error| invalid_arguments(error.to_string()))?;
    Ok(if performance {
        ForensicRequest::performance(state_root, events)
    } else {
        ForensicRequest::events(state_root, events)
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
    CliError::new("invalid_arguments", "parse_arguments", detail)
}

fn output_error(error: std::io::Error) -> CliError {
    CliError::new("output_failed", "write_forensic_output", error.to_string())
}

fn serialization_error(error: serde_json::Error) -> CliError {
    if error.is_io() {
        CliError::new("output_failed", "write_forensic_output", error.to_string())
    } else {
        CliError::new(
            "serialization_failed",
            "serialize_forensic_report",
            error.to_string(),
        )
    }
}
