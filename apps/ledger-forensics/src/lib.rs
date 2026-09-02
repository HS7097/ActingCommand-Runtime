// SPDX-License-Identifier: AGPL-3.0-only

#![forbid(unsafe_code)]

use std::error::Error;
use std::ffi::OsString;
use std::fmt;
use std::io::Write;
use std::path::PathBuf;

use actingcommand_ledger_forensics::{ForensicCommand, ForensicOutput, ForensicRequest};

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
    let request = parse_args(args)?;
    let report = actingcommand_ledger_forensics::run(request)
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

fn parse_args<I>(args: I) -> Result<ForensicRequest, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let mut args = args.into_iter();
    require_utf8(args.next(), "--state-root")?;
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
        "events" => ForensicCommand::Events,
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
        "export" => ForensicCommand::Export,
        _ => return Err(invalid_arguments("unsupported command")),
    };
    if args.next().is_some() {
        return Err(invalid_arguments(
            "unexpected argument or unsupported filter",
        ));
    }
    Ok(ForensicRequest::new(state_root, command))
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
